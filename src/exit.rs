//! Exit side: receives open requests through the tunnel and connects to the targets.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bytes::Bytes;
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, warn};

use crate::channel::{self, Channel, Link};
use crate::config::{Config, Mode, Tuning};
use crate::crypto::{Crypto, ReplayFilter};
use crate::mux::{maintain, MuxSession, MuxStream, ResetReason, SessionConfig, Side};
use crate::proto::{self, Open, KIND_UDP, STATUS_DIAL_FAILED, STATUS_OK};
use crate::relay::{relay, relay_mux};
use crate::transport::{tcp, Dialer, Listener, Settings};
use crate::udp;

const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(10);

struct Exit {
    crypto: Crypto,
    tuning: Tuning,
}

pub async fn run(config: Config) -> Result<()> {
    let tuning = config.tuning();
    let mux = config.mux();
    let transport = Settings::new(&config.tunnel);
    let exit = Arc::new(Exit {
        crypto: Crypto::new(&config.tunnel.token, config.tunnel.encryption).with_mux(mux.enabled),
        tuning: tuning.clone(),
    });
    let sessions = SessionConfig::new(&mux, &tuning);
    let mut tasks = JoinSet::new();

    match config.mode {
        Mode::Reverse if mux.enabled => {
            let remote = config.tunnel.remote.as_deref().expect("validated");
            info!(
                remote,
                connections = mux.connections,
                "exit: reverse mode with mux, keeping sessions to the entry side"
            );
            let dialer = Arc::new(Dialer::new(&transport, remote, &tuning)?);
            for _ in 0..mux.connections {
                let (exit, dialer) = (exit.clone(), dialer.clone());
                let connect = {
                    let exit = exit.clone();
                    move || {
                        let (exit, dialer) = (exit.clone(), dialer.clone());
                        async move { connect_once(&exit, &dialer).await }
                    }
                };
                let (sessions, lifetime) = (sessions.clone(), mux.max_lifetime);
                tasks.spawn(async move {
                    maintain(
                        "the entry side",
                        connect,
                        Side::Server,
                        sessions,
                        lifetime,
                        move |s| {
                            tokio::spawn(run_session(exit.clone(), s));
                        },
                    )
                    .await;
                    Ok(())
                });
            }
        }
        Mode::Reverse => {
            let remote = config.tunnel.remote.as_deref().expect("validated");
            info!(
                remote,
                pool = config.tunnel.pool,
                "exit: reverse mode, connecting to the entry side"
            );
            let dialer = Arc::new(Dialer::new(&transport, remote, &tuning)?);
            for _ in 0..config.tunnel.pool {
                tasks.spawn(pool_worker(exit.clone(), dialer.clone()));
            }
        }
        Mode::Direct => {
            let addr = config.tunnel.listen.as_deref().expect("validated");
            let listener = Listener::bind(&transport, addr, &tuning)
                .await
                .with_context(|| format!("failed to listen for tunnel connections on {addr}"))?;
            info!(
                addr = %listener.local_addr()?,
                mux = mux.enabled,
                "exit: direct mode, waiting for the entry side"
            );
            let sessions = mux.enabled.then_some(sessions);
            tasks.spawn(accept_direct(exit.clone(), listener, sessions));
        }
    }

    match tasks.join_next().await {
        Some(Ok(result)) => result,
        Some(Err(e)) => Err(e.into()),
        None => Ok(()),
    }
}

/// Reverse mode: keeps one idle, authenticated channel open towards the entry side.
/// As soon as it is used, a new one is dialed.
async fn pool_worker(exit: Arc<Exit>, dialer: Arc<Dialer>) -> Result<()> {
    let mut backoff = BACKOFF_MIN;
    loop {
        let mut channel = match connect_once(&exit, &dialer).await {
            Ok(link) => {
                backoff = BACKOFF_MIN;
                Channel::from(link)
            }
            Err(e) => {
                if matches!(
                    e.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::Unsupported
                ) {
                    error!(error = %e, "entry side rejected us or failed authentication");
                } else {
                    warn!(error = %e, "could not connect to the entry side");
                }
                sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_MAX);
                continue;
            }
        };

        // Waits as long as needed; TCP keepalive notices a dead entry side.
        match Open::read(&mut channel).await {
            Ok(open) => {
                let exit = exit.clone();
                tokio::spawn(async move { serve(&exit, channel, open).await });
            }
            Err(e) => debug!(error = %e, "idle tunnel connection closed"),
        }
    }
}

async fn connect_once(exit: &Exit, dialer: &Dialer) -> io::Result<Link> {
    timeout(
        exit.tuning.dial_timeout + exit.tuning.handshake_timeout,
        channel::connect(dialer, &exit.crypto, &[]),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "handshake timed out"))?
}

/// Direct mode: authenticates connections from the entry side and serves them, each as
/// one channel or, with `sessions`, as a mux session.
async fn accept_direct(
    exit: Arc<Exit>,
    listener: Listener,
    sessions: Option<SessionConfig>,
) -> Result<()> {
    let replay = Arc::new(ReplayFilter::default());
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "tunnel accept failed");
                sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let (exit, replay, sessions) = (exit.clone(), replay.clone(), sessions.clone());
        tokio::spawn(async move {
            let hs_timeout = exit.tuning.handshake_timeout;
            let link = match channel::accept(stream, &exit.crypto, &replay, hs_timeout).await {
                Ok(link) => link,
                Err(e) => return warn!(%peer, error = %e, "tunnel handshake failed"),
            };
            if let Some(config) = sessions {
                info!(%peer, "mux session from the entry side established");
                let session = Arc::new(MuxSession::over(link, Side::Server, config));
                return run_session(exit, session).await;
            }
            let mut channel = Channel::from(link);
            match timeout(hs_timeout, Open::read(&mut channel)).await {
                Ok(Ok(open)) => serve(&exit, channel, open).await,
                Ok(Err(e)) => warn!(%peer, error = %e, "bad open request"),
                Err(_) => warn!(%peer, "open request timed out"),
            }
        });
    }
}

async fn serve(exit: &Exit, mut tunnel: Channel, open: Open) {
    if open.kind == KIND_UDP {
        return serve_udp(exit, tunnel, open).await;
    }
    let mut target = match tcp::connect(&open.target, &exit.tuning).await {
        Ok(t) => t,
        Err(e) => {
            warn!(target = %open.target, error = %e, "could not connect to target");
            let _ = proto::write_status(&mut tunnel, STATUS_DIAL_FAILED).await;
            return;
        }
    };
    if proto::write_status(&mut tunnel, STATUS_OK).await.is_err() {
        return;
    }
    if let Err(e) = relay(&mut tunnel, &mut target, exit.tuning.buffer_size).await {
        debug!(target = %open.target, error = %e, "connection ended with error");
    }
}

/// Serves every stream the entry side opens on `session`, then lets the session drain.
async fn run_session(exit: Arc<Exit>, session: Arc<MuxSession>) {
    while let Some((stream, syn)) = session.accept().await {
        let exit = exit.clone();
        tokio::spawn(async move { serve_stream(&exit, stream, syn).await });
    }
    session.drain().await;
    debug!(
        reason = session.close_reason().unwrap_or_default(),
        "mux session ended"
    );
}

/// Mux counterpart of [`serve`]: no status byte, a failed dial resets the stream.
async fn serve_stream(exit: &Exit, stream: MuxStream, syn: Bytes) {
    let open = match Open::decode(&syn) {
        Ok(open) => open,
        Err(e) => {
            warn!(error = %e, "bad open request");
            return stream.reset(ResetReason::Protocol);
        }
    };
    if open.kind == KIND_UDP {
        let socket = match udp::connect(&open.target, &exit.tuning).await {
            Ok(s) => s,
            Err(e) => {
                warn!(target = %open.target, error = %e, "could not open UDP to target");
                return stream.reset(ResetReason::DialFailed);
            }
        };
        return relay_udp(exit, Channel::Mux(stream), socket, &open).await;
    }
    let mut target = match tcp::connect(&open.target, &exit.tuning).await {
        Ok(t) => t,
        Err(e) => {
            warn!(target = %open.target, error = %e, "could not connect to target");
            return stream.reset(ResetReason::DialFailed);
        }
    };
    if let Err(e) = relay_mux(&mut target, &stream, exit.tuning.buffer_size).await {
        debug!(target = %open.target, error = %e, "connection ended with error");
    }
}

/// UDP counterpart of [`serve`] (a whole channel): the status byte says whether the
/// target could be resolved, then packets flow length-prefixed.
async fn serve_udp(exit: &Exit, mut tunnel: Channel, open: Open) {
    let socket = match udp::connect(&open.target, &exit.tuning).await {
        Ok(s) => s,
        Err(e) => {
            warn!(target = %open.target, error = %e, "could not open UDP to target");
            let _ = proto::write_status(&mut tunnel, STATUS_DIAL_FAILED).await;
            return;
        }
    };
    if proto::write_status(&mut tunnel, STATUS_OK).await.is_err() {
        return;
    }
    relay_udp(exit, tunnel, socket, &open).await;
}

async fn relay_udp(exit: &Exit, tunnel: Channel, socket: tokio::net::UdpSocket, open: &Open) {
    debug!(target = %open.target, "UDP flow opened");
    let (source, sink) = udp::Connected::new(socket);
    match udp::relay(tunnel, source, sink, exit.tuning.udp.timeout).await {
        Ok(()) => debug!(target = %open.target, "UDP flow closed"),
        Err(e) => debug!(target = %open.target, error = %e, "UDP flow ended with error"),
    }
}
