use config::Config;
use std::path::Path;
use tracing::{debug, info, warn};

pub struct State {
    pub config: Config,
}

impl State {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn dump_info(&self) {
        // TODO: dump state info
        info!("{:?}", self.config);
    }

    pub fn reload_config(&mut self, path: impl AsRef<Path>) {
        self.config = match Config::load(path) {
            Ok(config) => config,
            Err(err) => {
                warn!(?err, "failed to load config");
                todo!()
            }
        };
        debug!(?self.config, "new config");
    }
}
