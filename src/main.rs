use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use kariz::config::{mode_name, role_name, Config};

#[derive(Parser)]
#[command(name = "kariz", version, about = "High-performance tunnel core")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run one side of the tunnel.
    Run {
        #[arg(short, long, default_value = "/etc/kariz/config.toml")]
        config: PathBuf,
    },
    /// Validate a config file and print a summary.
    Check {
        #[arg(short, long, default_value = "/etc/kariz/config.toml")]
        config: PathBuf,
    },
    /// Generate a random token for `tunnel.token`.
    Token,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Run { config } => run(Config::load(&config)?),
        Command::Check { config } => {
            let config = Config::load(&config)?;
            print_summary(&config);
            Ok(())
        }
        Command::Token => {
            let mut bytes = [0u8; 24];
            getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
            println!(
                "{}",
                bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
            );
            Ok(())
        }
    }
}

fn run(config: Config) -> Result<()> {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.log.level));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let mut builder = tokio::runtime::Builder::new_multi_thread();
    if let Some(threads) = config.tuning().threads {
        builder.worker_threads(threads);
    }
    let runtime = builder.enable_all().build()?;

    runtime.block_on(async move {
        tokio::select! {
            result = kariz::run(config) => result,
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

fn print_summary(config: &Config) {
    let tuning = config.tuning();
    println!("config OK");
    println!("  role      : {}", role_name(config.role));
    println!("  mode      : {}", mode_name(config.mode));
    println!("  profile   : {:?}", config.profile);
    println!("  transport : {:?}", config.tunnel.transport);
    if let Some(listen) = &config.tunnel.listen {
        println!("  listen    : {listen}");
    }
    if let Some(remote) = &config.tunnel.remote {
        println!("  remote    : {remote}");
    }
    println!(
        "  tuning    : nodelay={} buffer={}B keepalive={}s",
        tuning.nodelay,
        tuning.buffer_size,
        tuning.keepalive.as_secs()
    );
    for f in &config.forward {
        println!(
            "  forward   : {} -> {} ({:?})",
            f.listen, f.target, f.protocol
        );
    }
}
