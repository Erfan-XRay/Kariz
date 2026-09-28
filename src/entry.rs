//! Entry side: accepts user connections on the `[[forward]]` ports and carries them
//! through the tunnel to the exit side.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bytes::Bytes;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout, timeout_at, Instant};
use tracing::{debug, info, warn};

use crate::channel::{self, Channel, Link};
use crate::config::{Config, Forward, Mode, TransportKind, Tuning};
use crate::crypto::{Crypto, ReplayFilter};
use crate::mux::{MuxSession, SessionConfig, Side};
use crate::proto::{self, Open};
use crate::relay::{relay, relay_stream};
#[cfg(feature = "quic")]
use crate::session::quic::{accept_sessions, QuicSession};
use crate::session::{maintain, Session, SessionPool};
#[cfg(feature = "quic")]
use crate::transport::quic::{QuicDialer, QuicListener, QuicSettings};
use crate::transport::{Dialer, Listener, Settings};
use crate::udp;

/// How many idle reverse connections the entry may queue before it starts dropping them.
const POOL_CAPACITY: usize = 1024;

struct Entry {
    crypto: Crypto,
    tuning: Tuning,
    source: Source,
}

enum Source {
    /// Direct mode: dial the exit side for every user connection.
    Direct(Dialer),
    /// Reverse mode: take an idle channel the exit side opened in advance.
    Reverse(Mutex<mpsc::Receiver<Link>>),
    /// Mux, either mode: open a stream on one of the sessions to the exit side.
    Mux(Arc<SessionPool>),
}

pub async fn run(config: Config) -> Result<()> {
    let tuning = config.tuning();
    let mux = config.mux();
    let transport = Settings::new(&config.tunnel, config.kcp());
    let crypto = Crypto::new(&config.tunnel.token, config.tunnel.encryption).with_mux(mux.enabled);
    let sessions = SessionConfig::new(&mux, &tuning);
    let mut tasks = JoinSet::new();

    let source = if transport.kind == TransportKind::Quic {
        Source::Mux(quic_pool(&config, &crypto, &sessions, &mut tasks).await?)
    } else {
        match config.mode {
            Mode::Direct if mux.enabled => {
                let remote = config.tunnel.remote.as_deref().expect("validated");
                info!(
                    remote,
                    connections = mux.connections,
                    "entry: direct mode with mux, keeping sessions to the exit side"
                );
                let dialer = Arc::new(Dialer::new(&transport, remote, &tuning)?);
                let pool = Arc::new(SessionPool::default());
                for _ in 0..mux.connections {
                    let (dialer, crypto, pool) = (dialer.clone(), crypto.clone(), pool.clone());
                    let wait = tuning.dial_timeout + tuning.handshake_timeout;
                    let sessions = sessions.clone();
                    let connect = move || {
                        let (dialer, crypto, sessions) =
                            (dialer.clone(), crypto.clone(), sessions.clone());
                        async move {
                            let link = timeout(wait, channel::connect(&dialer, &crypto, &[]))
                                .await
                                .map_err(|_| {
                                    io::Error::new(io::ErrorKind::TimedOut, "handshake timed out")
                                })??;
                            Ok(Session::Kmux(MuxSession::over(
                                link,
                                Side::Client,
                                sessions,
                            )))
                        }
                    };
                    let lifetime = mux.max_lifetime;
                    tasks.spawn(async move {
                        maintain("the exit side", connect, lifetime, move |s| pool.add(s)).await;
                        Ok(())
                    });
                }
                Source::Mux(pool)
            }
            Mode::Direct => {
                let remote = config.tunnel.remote.as_deref().expect("validated");
                info!(
                    remote,
                    "entry: direct mode, dialing the exit side per connection"
                );
                Source::Direct(Dialer::new(&transport, remote, &tuning)?)
            }
            Mode::Reverse => {
                let addr = config.tunnel.listen.as_deref().expect("validated");
                let listener = Listener::bind(&transport, addr, &tuning)
                    .await
                    .with_context(|| {
                        format!("failed to listen for tunnel connections on {addr}")
                    })?;
                info!(
                    addr = %listener.local_addr()?,
                    mux = mux.enabled,
                    "entry: reverse mode, waiting for the exit side"
                );
                let hs = tuning.handshake_timeout;
                if mux.enabled {
                    let pool = Arc::new(SessionPool::default());
                    let p = pool.clone();
                    let on_link = move |link: Link, peer: SocketAddr| {
                        let session = MuxSession::over(link, Side::Client, sessions.clone());
                        p.add(Arc::new(Session::Kmux(session)));
                        info!(%peer, "mux session from the exit side established");
                    };
                    tasks.spawn(accept_reverse(listener, crypto.clone(), hs, on_link));
                    Source::Mux(pool)
                } else {
                    let (tx, rx) = mpsc::channel(POOL_CAPACITY);
                    let on_link = move |link: Link, peer: SocketAddr| {
                        debug!(%peer, "tunnel connection added to pool");
                        if tx.try_send(link).is_err() {
                            warn!(%peer, "tunnel pool is full, dropping connection");
                        }
                    };
                    tasks.spawn(accept_reverse(listener, crypto.clone(), hs, on_link));
                    Source::Reverse(Mutex::new(rx))
                }
            }
        }
    };

    let entry = Arc::new(Entry {
        crypto,
        tuning,
        source,
    });

    for forward in &config.forward {
        if forward.protocol.has_tcp() {
            let listener = TcpListener::bind(&forward.listen)
                .await
                .with_context(|| format!("failed to listen on forward port {}", forward.listen))?;
            tasks.spawn(accept_users(entry.clone(), listener, forward.clone()));
        }
        if forward.protocol.has_udp() {
            let socket = udp::bind(&forward.listen, &entry.tuning.udp)
                .await
                .with_context(|| format!("failed to bind UDP forward port {}", forward.listen))?;
            let open = encode_open(Open::udp(forward.target.clone(), forward.duplication()));
            let opener = {
                let entry = entry.clone();
                move || {
                    let (entry, open) = (entry.clone(), open.clone());
                    async move { entry.open_channel(&open).await }
                }
            };
            let (udp_tuning, target) = (entry.tuning.udp.clone(), forward.target.clone());
            let (listen, duplicate) = (forward.listen.clone(), forward.duplication());
            tasks.spawn(async move {
                udp::serve(socket, udp_tuning, target, duplicate, opener)
                    .await
                    .with_context(|| format!("UDP forward port {listen} failed"))
            });
        }
        info!(
            listen = %forward.listen,
            target = %forward.target,
            protocol = forward.protocol.name(),
            "forward ready"
        );
    }

    // Accept loops only return on fatal errors.
    match tasks.join_next().await {
        Some(Ok(result)) => result,
        Some(Err(e)) => Err(e.into()),
        None => Ok(()),
    }
}

/// QUIC, either mode: the pool of QUIC sessions to the exit side. Direct mode keeps
/// `mux.connections` of them up; reverse mode takes the ones the exit side opens.
#[cfg(feature = "quic")]
async fn quic_pool(
    config: &Config,
    crypto: &Crypto,
    sessions: &SessionConfig,
    tasks: &mut JoinSet<Result<()>>,
) -> Result<Arc<SessionPool>> {
    let (tuning, mux) = (config.tuning(), config.mux());
    let quic_config = config.tunnel.quic.clone().unwrap_or_default();
    let quic = QuicSettings::new(crypto.psk(), &quic_config, &mux, &tuning)?;
    let pool = Arc::new(SessionPool::default());
    let open_timeout = tuning.handshake_timeout;
    match config.mode {
        Mode::Direct => {
            let remote = config.tunnel.remote.as_deref().expect("validated");
            let dialer = Arc::new(QuicDialer::new(remote, quic_config.sni.as_deref(), &quic)?);
            info!(
                remote,
                connections = mux.connections,
                congestion = quic_config.congestion.name(),
                "entry: direct mode over QUIC, keeping sessions to the exit side"
            );
            for _ in 0..mux.connections {
                let (dialer, pool, sessions) = (dialer.clone(), pool.clone(), sessions.clone());
                let connect = move || {
                    let (dialer, sessions) = (dialer.clone(), sessions.clone());
                    async move {
                        let conn = dialer.connect().await?;
                        Ok(Session::Quic(QuicSession::new(
                            conn,
                            &sessions,
                            open_timeout,
                        )))
                    }
                };
                let lifetime = mux.max_lifetime;
                tasks.spawn(async move {
                    maintain("the exit side", connect, lifetime, move |s| pool.add(s)).await;
                    Ok(())
                });
            }
        }
        Mode::Reverse => {
            let addr = config.tunnel.listen.as_deref().expect("validated");
            let listener = QuicListener::bind(addr, &quic)
                .await
                .with_context(|| format!("failed to listen for QUIC on {addr}"))?;
            info!(
                addr = %listener.local_addr()?,
                "entry: reverse mode over QUIC, waiting for the exit side"
            );
            let p = pool.clone();
            let on_session = move |session: QuicSession, peer: SocketAddr| {
                p.add(Arc::new(Session::Quic(session)));
                info!(%peer, "quic session from the exit side established");
            };
            let sessions = sessions.clone();
            tasks.spawn(async move {
                accept_sessions(listener, sessions, open_timeout, on_session).await;
                Ok(())
            });
        }
    }
    Ok(pool)
}

/// Config validation rejects `transport = "quic"` in a build without it.
#[cfg(not(feature = "quic"))]
async fn quic_pool(
    _: &Config,
    _: &Crypto,
    _: &SessionConfig,
    _: &mut JoinSet<Result<()>>,
) -> Result<Arc<SessionPool>> {
    anyhow::bail!("this build has no QUIC support (the `quic` feature is off)")
}

/// Reverse mode: authenticates incoming tunnel connections and hands them to
/// `on_link` (the idle pool, or a new mux session).
async fn accept_reverse(
    listener: Listener,
    crypto: Crypto,
    handshake_timeout: Duration,
    on_link: impl Fn(Link, SocketAddr) + Send + Sync + 'static,
) -> Result<()> {
    let replay = Arc::new(ReplayFilter::default());
    let on_link = Arc::new(on_link);
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "tunnel accept failed");
                sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let (crypto, replay, on_link) = (crypto.clone(), replay.clone(), on_link.clone());
        tokio::spawn(async move {
            match channel::accept(stream, &crypto, &replay, handshake_timeout).await {
                Ok(link) => on_link(link, peer),
                Err(e) => warn!(%peer, error = %e, "tunnel handshake failed"),
            }
        });
    }
}

async fn accept_users(entry: Arc<Entry>, listener: TcpListener, forward: Forward) -> Result<()> {
    let open = encode_open(Open::tcp(forward.target.clone()));
    loop {
        let (user, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, listen = %forward.listen, "user accept failed");
                sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let (entry, open) = (entry.clone(), open.clone());
        let target = forward.target.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_user(&entry, user, &open).await {
                debug!(%peer, %target, error = %e, "connection ended with error");
            }
        });
    }
}

fn encode_open(open: Open) -> Bytes {
    let mut frame = Vec::new();
    open.encode(&mut frame);
    frame.into()
}

async fn handle_user(entry: &Entry, mut user: TcpStream, open: &Bytes) -> io::Result<()> {
    user.set_nodelay(entry.tuning.nodelay)?;
    let size = entry.tuning.buffer_size;
    match entry.open_channel(open).await? {
        Channel::Stream(stream) => relay_stream(&mut user, &stream, size).await?,
        Channel::Link(mut link) => relay(&mut user, &mut link, size).await?,
    };
    Ok(())
}

impl Entry {
    /// Gets a channel to the exit side and has the exit connect it to the target
    /// described by the encoded `open` request.
    async fn open_channel(&self, open: &Bytes) -> io::Result<Channel> {
        match &self.source {
            Source::Direct(dialer) => self.open_direct(dialer, open).await.map(Channel::Link),
            Source::Reverse(pool) => self.open_reverse(pool, open).await.map(Channel::Link),
            // Optimistic: the stream is used right away; if the exit cannot reach the
            // target it resets the stream, which ends the relay.
            Source::Mux(pool) => {
                let deadline =
                    Instant::now() + self.tuning.handshake_timeout + self.tuning.dial_timeout;
                Ok(Channel::Stream(pool.open(open.clone(), deadline).await?))
            }
        }
    }

    /// Dials the exit side and sends the open request along with the hello.
    async fn open_direct(&self, dialer: &Dialer, open: &[u8]) -> io::Result<Link> {
        let wait = self.tuning.handshake_timeout + self.tuning.dial_timeout;
        timeout(wait, async {
            let mut link = channel::connect(dialer, &self.crypto, open).await?;
            proto::read_status(&mut link).await?;
            Ok(link)
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "exit side did not answer in time"))?
    }

    /// Takes pooled links until one accepts the open request. Pooled links may have
    /// died while idle, so failures other than "target unreachable" are retried.
    async fn open_reverse(
        &self,
        pool: &Mutex<mpsc::Receiver<Link>>,
        open: &[u8],
    ) -> io::Result<Link> {
        let deadline = Instant::now() + self.tuning.handshake_timeout + self.tuning.dial_timeout;
        loop {
            let mut channel = timeout_at(deadline, async { pool.lock().await.recv().await })
                .await
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        "no tunnel connection available (is the exit side running?)",
                    )
                })?
                .ok_or_else(|| io::Error::other("tunnel pool closed"))?;

            if !channel.is_alive() {
                continue;
            }
            if proto::send(&mut channel, open).await.is_err() {
                continue;
            }
            match timeout_at(deadline, proto::read_status(&mut channel)).await {
                Ok(Ok(())) => return Ok(channel),
                Ok(Err(e)) if e.kind() == io::ErrorKind::ConnectionRefused => return Err(e),
                Ok(Err(_)) => continue,
                Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "exit side did not answer in time",
                    ))
                }
            }
        }
    }
}
