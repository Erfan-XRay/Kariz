//! Kariz: a high-performance, resource-efficient tunnel core.

pub mod allocator;
pub mod config;
pub mod control;
pub mod crypto;
pub mod link;
pub mod mux;
pub mod proto;
pub mod session;
pub mod transport;

mod channel;
mod entry;
mod exit;
mod relay;
pub mod speedtest;
pub mod stats;
mod udp;

use config::{Config, Role};

/// Runs one side of the tunnel until a fatal error occurs.
pub async fn run(config: Config) -> anyhow::Result<()> {
    match config.role {
        Role::Entry => entry::run(config).await,
        Role::Exit => exit::run(config).await,
    }
}
