//! Kariz: a high-performance, resource-efficient tunnel core.

pub mod auth;
pub mod config;
pub mod proto;
pub mod transport;

mod entry;
mod exit;
mod relay;

use config::{Config, Role};

/// Runs one side of the tunnel until a fatal error occurs.
pub async fn run(config: Config) -> anyhow::Result<()> {
    match config.role {
        Role::Entry => entry::run(config).await,
        Role::Exit => exit::run(config).await,
    }
}
