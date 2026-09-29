use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use kariz_panel::config::{Config, DEFAULT_CONFIG, DEFAULT_DATA_DIR};
use kariz_panel::db::Db;
use kariz_panel::http::{self, AppState};

#[derive(Parser)]
#[command(name = "kariz-panel", version, about = "The Kariz web panel")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the panel.
    Serve {
        #[arg(short, long, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
    },
    /// Set the panel up: its settings (a random port and secret path), the database and
    /// the certificate. Safe to run again.
    Init {
        #[arg(short, long, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
        /// Where the database and the certificate go.
        #[arg(long, default_value = DEFAULT_DATA_DIR)]
        data_dir: PathBuf,
        /// The port, instead of a random one (20000-59999).
        #[arg(long)]
        port: Option<u16>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Init {
            config,
            data_dir,
            port,
        } => {
            let done = kariz_panel::init(&config, &data_dir, port)?;
            println!("panel set up");
            println!("  settings    : {}", config.display());
            println!("  listen      : {}", done.config.listen);
            println!("  secret path : /{}/", done.config.path);
            println!("  certificate : SHA-256 {}", done.fingerprint);
            Ok(())
        }
        Command::Serve { config } => serve(Config::load(&config)?),
    }
}

fn serve(config: Config) -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let db = Db::open(&config.database())?;
        kariz_panel::cert::ensure(&config.cert(), &config.key())?;
        let listener = tokio::net::TcpListener::bind(&config.listen)
            .await
            .with_context(|| format!("failed to listen on {}", config.listen))?;
        let app = http::router(&config.path, AppState { db });
        tokio::select! {
            result = http::serve(listener, &config, app) => result,
            _ = shutdown_signal() => {
                tracing::info!("shutting down");
                Ok(())
            }
        }
    })
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => return std::future::pending().await,
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
