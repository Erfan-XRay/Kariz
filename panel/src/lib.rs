//! The Kariz web panel.

pub mod agent;
pub mod agent_update;
pub mod api;
pub mod auth;
pub mod backup;
pub mod cert;
pub mod collect;
pub mod config;
pub mod db;
pub mod history;
pub mod http;
pub mod hub;
pub mod join;
pub mod manage;
pub mod net;
pub mod netops;
pub mod networks;
pub mod pair;
pub mod sign;
pub mod update;
pub mod updater;
pub mod wire;

use std::path::Path;

/// This build's version. `KARIZ_BUILD_VERSION` at build time overrides Cargo's (the CI test
/// of updating builds a second panel that calls itself newer).
pub fn version() -> &'static str {
    option_env!("KARIZ_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

use anyhow::{Context, Result};

use config::{random_hex, random_port, Config};

/// What `kariz-panel init` made.
pub struct Installed {
    pub config: Config,
    pub fingerprint: String,
}

/// Sets the panel up: the settings file (a random port and secret path unless
/// `config_path` already exists), the database and the certificate. Safe to run again:
/// it keeps what is there.
pub fn init(config_path: &Path, data_dir: &Path, port: Option<u16>) -> Result<Installed> {
    let config = if config_path.exists() {
        Config::load(config_path)?
    } else {
        let config = Config {
            listen: format!("0.0.0.0:{}", port.map_or_else(random_port, Ok)?),
            path: format!("k-{}", random_hex(4)?),
            data_dir: data_dir.to_path_buf(),
            agent_listen: Some(format!("0.0.0.0:{}", random_port()?)),
            kariz_dir: std::path::PathBuf::from("/etc/kariz"),
            services: Default::default(),
            cert_file: None,
            key_file: None,
            release_api: None,
            release_key: None,
        };
        config.validate()?;
        if let Some(dir) = config_path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        std::fs::write(config_path, toml::to_string_pretty(&config)?)
            .with_context(|| format!("failed to write {}", config_path.display()))?;
        config
    };
    let db = db::Db::open(&config.database())?;
    db.audit("cli", None, "panel set up")?;
    let fingerprint = config.ensure_cert()?;
    Ok(Installed {
        config,
        fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_makes_everything_and_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let (config_path, data) = (
            dir.path().join("etc").join("panel.toml"),
            dir.path().join("data"),
        );
        let first = init(&config_path, &data, Some(24_680)).unwrap();
        assert_eq!(first.config.listen, "0.0.0.0:24680");
        assert!(first.config.path.starts_with("k-") && first.config.path.len() == 10);
        assert!(config_path.exists() && first.config.database().exists());

        let second = init(&config_path, &data, Some(1)).unwrap();
        assert_eq!(second.config.listen, first.config.listen);
        assert_eq!(second.config.path, first.config.path);
        assert_eq!(second.fingerprint, first.fingerprint);
    }
}
