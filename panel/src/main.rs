use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use kariz_panel::auth;
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
    /// Make a one-time login link (valid for 60 minutes, works once).
    LoginLink {
        #[arg(short, long, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
        /// This server's address or domain, as the browser reaches it.
        #[arg(long, default_value = "<this-server>")]
        host: String,
    },
    /// Set a new admin password and sign every session out. Without --stdin, a random
    /// password is made and shown.
    ResetPassword {
        #[arg(short, long, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
        /// Read the new password from the first line of standard input.
        #[arg(long)]
        stdin: bool,
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
        Command::LoginLink { config, host } => {
            let config = Config::load(&config)?;
            let db = Db::open(&config.database())?;
            let token = auth::create_link(&db, auth::now())?;
            db.audit("cli", None, "made a login link")?;
            let port = config
                .listen
                .rsplit(':')
                .next()
                .unwrap_or_default()
                .to_owned();
            let host = if host.contains(':') && !host.starts_with('[') {
                format!("[{host}]")
            } else {
                host
            };
            println!("https://{host}:{port}/{}/#t={token}", config.path);
            eprintln!("valid for 60 minutes; it works once");
            Ok(())
        }
        Command::ResetPassword { config, stdin } => {
            let config = Config::load(&config)?;
            let db = Db::open(&config.database())?;
            let (password, shown) = if stdin {
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                (line.trim_end_matches(&['\r', '\n'][..]).to_owned(), false)
            } else {
                (kariz_panel::config::random_hex(10)?, true)
            };
            auth::set_password(&db, &password, None)?;
            db.audit("cli", None, "reset the password")?;
            if shown {
                println!("{password}");
                eprintln!("the new admin password (every session was signed out)");
            } else {
                eprintln!("password set (every session was signed out)");
            }
            Ok(())
        }
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
