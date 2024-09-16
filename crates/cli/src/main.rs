mod cli;

use clap::Parser;
use tracing::debug;
use tracing_log::AsTrace;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    tracing_subscriber::fmt()
        .with_max_level(cli.verbosity.log_level_filter().as_trace())
        .with_level(true)
        .with_file(true)
        .with_line_number(true)
        .init();

    debug!("CLI: {:#?}", cli);
    Ok(())
}
