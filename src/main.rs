use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use kariz::config::{mode_name, role_name, Config, Dscp, Encryption, TransportKind};
use kariz::crypto::Cipher;

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
    /// Print the `tunnel.tls.pin_sha256` value of a certificate (PEM file), for dialing
    /// a `wss` server with a self-signed certificate.
    Pin {
        /// The certificate file (`tunnel.tls.cert` of the listening side).
        cert: PathBuf,
    },
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
        Command::Pin { cert } => {
            println!("{}", kariz::transport::tls::pin_of_file(&cert)?);
            Ok(())
        }
    }
}

fn run(config: Config) -> Result<()> {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.log.level));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    for warning in config.warnings() {
        tracing::warn!("{warning}");
    }

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
    println!("  transport : {}", config.tunnel.transport.name());
    if let Some(listen) = &config.tunnel.listen {
        println!("  listen    : {listen}");
    }
    if let Some(remote) = &config.tunnel.remote {
        println!("  remote    : {remote}");
    }
    let cipher = Cipher::for_config(config.tunnel.encryption);
    match config.tunnel.encryption {
        _ if config.tunnel.transport == TransportKind::Quic => {
            println!("  encryption: TLS 1.3 (QUIC), both sides authenticated by the token")
        }
        Encryption::Auto if config.is_acceptor() => {
            println!("  encryption: auto (accepts chacha20-poly1305 and aes-256-gcm)")
        }
        Encryption::Auto => println!("  encryption: auto ({} on this CPU)", cipher.name()),
        e => println!("  encryption: {}", e.name()),
    }
    let mux = config.mux();
    if mux.enabled {
        println!(
            "  mux       : connections={} max_streams={} window={}B",
            mux.connections, mux.max_streams, mux.stream_window
        );
    } else {
        println!("  mux       : off");
    }
    if matches!(
        config.tunnel.transport,
        TransportKind::Ws | TransportKind::Wss
    ) {
        let ws = config.tunnel.ws.as_ref();
        let path = ws.map_or("/", |w| w.path.as_str());
        let host = ws.and_then(|w| w.host.as_deref()).unwrap_or("-");
        println!("  ws        : path={path} host={host}");
    }
    if config.tunnel.transport == TransportKind::Quic {
        let quic = config.tunnel.quic.clone().unwrap_or_default();
        let sni = match &quic.sni {
            Some(sni) => format!(" sni={sni}"),
            None => String::new(),
        };
        println!(
            "  quic      : congestion={} alpn={}{sni}",
            quic.congestion.name(),
            quic.alpn
        );
    }
    if config.tunnel.transport == TransportKind::Kcp {
        let kcp = config.tunnel.kcp.clone().unwrap_or_default();
        let t = kcp.timing();
        println!(
            "  kcp       : mode={} (nodelay={} interval={}ms resend={} no_congestion={}) \
             window={}/{} mtu={} fec={}, packets sealed with the token",
            kcp.mode.name(),
            t.nodelay,
            t.interval_ms,
            t.resend,
            t.no_congestion,
            kcp.send_window,
            kcp.recv_window,
            kcp.mtu,
            match kcp.fec() {
                Some((data, parity)) => format!("{data}+{parity}"),
                None => "off".into(),
            }
        );
    }
    if config.tunnel.transport == TransportKind::Wss {
        let tls = config.tunnel.tls.clone().unwrap_or_default();
        if let Some(cert) = &tls.cert {
            println!(
                "  tls cert  : {} (reloaded when it changes)",
                cert.display()
            );
        } else {
            let verify = if tls.pin_sha256.is_some() {
                "pinned certificate (pin_sha256)"
            } else if tls.insecure {
                "none (insecure)"
            } else {
                "Mozilla root certificates"
            };
            println!("  tls verify: {verify}");
            if let Some(sni) = &tls.sni {
                println!("  tls sni   : {sni}");
            }
        }
    }
    println!(
        "  tuning    : nodelay={} buffer={}B keepalive={}s",
        tuning.nodelay,
        tuning.buffer_size,
        tuning.keepalive.as_secs()
    );
    if let Some(dscp) = tuning.dscp {
        let name = Dscp::name_of(dscp).map_or(String::new(), |n| format!(" ({n})"));
        let sockets = if config.tunnel.transport == TransportKind::Quic {
            "UDP sockets to targets (not QUIC's)"
        } else {
            "tunnel sockets and UDP sockets to targets"
        };
        println!("  dscp      : {dscp}{name} on {sockets}");
    }
    if config.forward.iter().any(|f| f.protocol.has_udp()) {
        println!(
            "  udp       : timeout={}s max_flows={} per rule",
            tuning.udp.timeout.as_secs(),
            tuning.udp.max_flows
        );
    }
    for f in &config.forward {
        let copies = f
            .duplication()
            .map(|d| format!(", {} copies {} ms apart", d.copies, d.gap_ms))
            .unwrap_or_default();
        println!(
            "  forward   : {} -> {} ({}{copies})",
            f.listen,
            f.target,
            f.protocol.name()
        );
    }
    for warning in config.warnings() {
        println!("warning: {warning}");
    }
}
