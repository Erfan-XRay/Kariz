use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use kariz_panel::agent::{self, AgentConfig, DEFAULT_AGENT_CONFIG};
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
    /// Run the agent that connects this server to a panel. The first time, give it the
    /// join code the panel shows (*Add server*); after that it remembers who it is.
    Agent {
        #[arg(short, long, default_value = DEFAULT_AGENT_CONFIG)]
        config: PathBuf,
        /// The join code from the panel.
        #[arg(long)]
        join: Option<String>,
        /// Only write the settings from `--join`, then stop (the installer runs the
        /// agent as a service).
        #[arg(long, requires = "join")]
        no_run: bool,
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
    /// Private network links on this server, by hand (what the agent does when the panel
    /// asks; for debugging and for the tests). Needs root and Linux.
    Net {
        #[command(subcommand)]
        command: NetCommand,
    },
}

#[derive(clap::Subcommand)]
enum NetCommand {
    /// Make the link described in a TOML file (fields of `NetSpec`).
    Up {
        file: PathBuf,
        /// Keep the link in this file so it can be made again (default: not kept).
        #[arg(long)]
        state: Option<PathBuf>,
    },
    /// Remove a link's interface.
    Down { name: String },
    /// Ping the far end of a link (the link must be kept in `--state`).
    Ping {
        name: String,
        #[arg(long)]
        state: PathBuf,
    },
    /// Whether GRE can be made here.
    Status,
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
        Command::Net { command } => net_command(command),
        Command::Agent {
            config,
            join,
            no_run,
        } => run_agent(&config, join.as_deref(), no_run),
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

fn net_command(command: NetCommand) -> Result<()> {
    use kariz_panel::net::Net;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        match command {
            NetCommand::Up { file, state } => {
                let spec: kariz_panel::wire::NetSpec =
                    toml::from_str(&std::fs::read_to_string(&file)?)?;
                let name = spec.name.clone();
                Net::new(state).up(spec).await?;
                println!("{name} is up");
            }
            NetCommand::Down { name } => {
                Net::new(None).down(&name).await?;
                println!("{name} is down");
            }
            NetCommand::Ping { name, state } => {
                let reply = Net::new(Some(state)).ping(&name).await;
                println!("{}", serde_json::to_string(&reply)?);
                if !reply.ok {
                    std::process::exit(1);
                }
            }
            NetCommand::Status => {
                println!("{}", serde_json::to_string(&Net::new(None).status().await)?);
            }
        }
        Ok(())
    })
}

fn serve(config: Config) -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let db = Db::open(&config.database())?;
        config.ensure_cert()?;
        let listener = tokio::net::TcpListener::bind(&config.listen)
            .await
            .with_context(|| format!("failed to listen on {}", config.listen))?;
        let hub = kariz_panel::hub::Hub::with_state_dir(
            db.clone(),
            config.kariz_dir.clone(),
            config.services.services(&config.kariz_dir),
            config.data_dir.clone(),
        );
        tokio::spawn(hub.clone().run_local());
        let mut state = AppState::new(db);
        state.hub = hub.clone();
        if let Some(listen) = &config.agent_listen {
            let acceptor = kariz::link::Acceptor::bind(listen, &hub.link_token()?)
                .await
                .with_context(|| format!("failed to listen for agents on {listen}"))?;
            state.agent_port = config.agent_port();
            tokio::spawn(hub.clone().serve_agents(acceptor));
        }
        let app = http::router(&config.path, state);
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

fn run_agent(path: &std::path::Path, join: Option<&str>, no_run: bool) -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    let config = match join {
        Some(code) => {
            if path.exists() && AgentConfig::load(path).is_ok_and(|c| c.id.is_some()) {
                anyhow::bail!(
                    "{} is registered already; delete it to join a panel again",
                    path.display()
                );
            }
            let config = agent::enroll_from_code(code, path)?;
            eprintln!("settings written to {}", path.display());
            config
        }
        None => AgentConfig::load(path)?,
    };
    if no_run {
        return Ok(());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let services = config.services.services(&config.kariz_dir);
        let agent = agent::Agent::with_services(path, config, services);
        tokio::select! {
            result = agent.run() => result,
            _ = shutdown_signal() => Ok(()),
        }
    })
}
