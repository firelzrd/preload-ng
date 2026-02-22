#![deny(unsafe_code)]

mod cli;
mod priority;
mod signals;

use clap::Parser;
use cli::Cli;
use config::Config;
use orchestrator::{
    ControlEvent, PreloadEngine, ReloadBundle, Services,
    clock::SystemClock,
    observation::{DefaultAdmissionPolicy, DefaultModelUpdater, FanotifyWatcher, ProcfsScanner},
    persistence::{NoopRepository, SqliteRepository},
    prediction::MarkovPredictor,
    prefetch::{
        GreedyPrefetchPlanner, MadvisePrefetcher, NoopPrefetcher, Prefetcher, ReadPrefetcher,
        ReadaheadPrefetcher,
    },
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    priority::lower_process_priority();
    let config = load_config_from_cli(&cli)?;

    let fanotify = if config.system.fanotify {
        FanotifyWatcher::try_new()
    } else {
        None
    };

    let repo = if cli.no_persist {
        Box::new(NoopRepository) as Box<dyn orchestrator::persistence::StateRepository>
    } else if let Some(path) = &config.persistence.state_path {
        let repo = SqliteRepository::new(path.clone()).await?;
        Box::new(repo) as Box<dyn orchestrator::persistence::StateRepository>
    } else {
        warn!("no persistence path resolved (HOME not set?); using in-memory state only");
        Box::new(NoopRepository) as Box<dyn orchestrator::persistence::StateRepository>
    };

    let reload_bundle = build_reload_bundle(config.clone(), cli.no_prefetch);

    let services = Services {
        scanner: Box::new(ProcfsScanner::new(fanotify)),
        admission: reload_bundle.admission,
        updater: reload_bundle.updater,
        predictor: reload_bundle.predictor,
        planner: reload_bundle.planner,
        prefetcher: reload_bundle.prefetcher,
        repo,
        clock: Box::new(SystemClock),
    };

    let mut engine = PreloadEngine::load(config, services).await?;

    if cli.once {
        let report = engine.tick().await?;
        info!(?report, "tick completed");
        return Ok(());
    }

    let cancel = CancellationToken::new();
    signals::install_ctrl_c(cancel.clone());

    let (control_tx, control_rx) = mpsc::unbounded_channel();
    install_signal_handlers(cli.clone(), control_tx);

    engine.run_until(cancel, control_rx).await?;
    Ok(())
}

fn init_tracing(verbosity: u8) {
    let default_level = match verbosity {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };

    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    tracing_subscriber::fmt().with_env_filter(env_filter).init();
}

/// Load configuration files and apply CLI overrides.
fn load_config_from_cli(cli: &Cli) -> anyhow::Result<Config> {
    let config_paths = cli.resolve_config_paths()?;
    let mut config = if config_paths.is_empty() {
        warn!("no config files found; falling back to defaults");
        Config::default()
    } else {
        Config::load_multiple(config_paths)?
    };

    if let Some(path) = cli.state.clone().or(config.persistence.state_path.clone()) {
        config.persistence.state_path = Some(path);
    }

    if config.persistence.state_path.is_none() {
        config.persistence.state_path = default_state_path();
    }

    Ok(config)
}

/// Resolve the default state database path via XDG Base Directory.
fn default_state_path() -> Option<std::path::PathBuf> {
    let cache_dir = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache"))
        })?;
    Some(cache_dir.join("preload-ng").join("state.db"))
}

/// Construct runtime services for a new configuration snapshot.
fn build_reload_bundle(config: Config, no_prefetch: bool) -> ReloadBundle {
    ReloadBundle {
        admission: Box::new(DefaultAdmissionPolicy::new(&config)),
        updater: Box::new(DefaultModelUpdater::new(&config)),
        predictor: Box::new(MarkovPredictor::new(&config)),
        planner: Box::new(GreedyPrefetchPlanner::new(&config)),
        prefetcher: build_prefetcher(&config, no_prefetch),
        config,
    }
}

/// Select the prefetcher implementation based on configuration and CLI flags.
fn build_prefetcher(config: &Config, no_prefetch: bool) -> Box<dyn Prefetcher> {
    use config::PrefetchBackend;

    if no_prefetch {
        return Box::new(NoopPrefetcher);
    }

    let concurrency = match config.system.prefetch_concurrency {
        Some(0) => return Box::new(NoopPrefetcher),
        Some(value) => value,
        None => std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
    };

    match config.system.prefetch_backend {
        PrefetchBackend::Readahead => {
            info!(concurrency, backend = "readahead", "prefetcher selected");
            Box::new(ReadaheadPrefetcher::new(concurrency))
        }
        PrefetchBackend::Madvise => {
            info!(concurrency, backend = "madvise", "prefetcher selected");
            Box::new(MadvisePrefetcher::new(concurrency))
        }
        PrefetchBackend::Read => {
            info!(concurrency, backend = "read", "prefetcher selected");
            Box::new(ReadPrefetcher::new(concurrency))
        }
        PrefetchBackend::Auto => {
            // Probe available backends: readahead → madvise → read
            if ReadaheadPrefetcher::probe() {
                info!(concurrency, backend = "readahead", "prefetcher auto-selected");
                Box::new(ReadaheadPrefetcher::new(concurrency))
            } else if MadvisePrefetcher::probe() {
                info!(concurrency, backend = "madvise", "prefetcher auto-selected");
                Box::new(MadvisePrefetcher::new(concurrency))
            } else {
                info!(concurrency, backend = "read", "prefetcher auto-selected (fallback)");
                Box::new(ReadPrefetcher::new(concurrency))
            }
        }
    }
}

/// Install signal handlers for runtime control (reload, dump, save).
fn install_signal_handlers(cli: Cli, control_tx: mpsc::UnboundedSender<ControlEvent>) {
    #[cfg(unix)]
    {
        let reload_tx = control_tx.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let mut hup = match signal(SignalKind::hangup()) {
                Ok(stream) => stream,
                Err(err) => {
                    warn!(?err, "failed to install SIGHUP handler");
                    return;
                }
            };
            while hup.recv().await.is_some() {
                match load_config_from_cli(&cli) {
                    Ok(config) => {
                        let bundle = build_reload_bundle(config, cli.no_prefetch);
                        if reload_tx
                            .send(ControlEvent::Reload(Box::new(bundle)))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(err) => {
                        warn!(?err, "failed to reload config");
                    }
                }
            }
        });

        let usr_tx = control_tx.clone();
        tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let mut usr1 = match signal(SignalKind::user_defined1()) {
                Ok(stream) => stream,
                Err(err) => {
                    warn!(?err, "failed to install SIGUSR1 handler");
                    return;
                }
            };
            while usr1.recv().await.is_some() {
                if usr_tx.send(ControlEvent::DumpStatus).is_err() {
                    break;
                }
            }
        });

        tokio::spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let mut usr2 = match signal(SignalKind::user_defined2()) {
                Ok(stream) => stream,
                Err(err) => {
                    warn!(?err, "failed to install SIGUSR2 handler");
                    return;
                }
            };
            while usr2.recv().await.is_some() {
                if control_tx.send(ControlEvent::SaveNow).is_err() {
                    break;
                }
            }
        });
    }

    #[cfg(not(unix))]
    {
        let _ = (cli, control_tx);
    }
}
