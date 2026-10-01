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

use crate::auto::{self, Dial};
use crate::channel::{self, Channel, Link};
use crate::config::{Config, Forward, Mode, TransportKind, Tuning};
use crate::crypto::{Crypto, ReplayFilter};
use crate::mux::{MuxSession, SessionConfig, Side};
use crate::proto::{self, Open};
use crate::relay::{relay, relay_stream, Counters};
#[cfg(feature = "quic")]
use crate::session::quic::{accept_sessions, QuicSession};
use crate::session::{maintain, Session, SessionPool};
use crate::stats::{ForwardStats, Stats};
#[cfg(feature = "quic")]
use crate::transport::quic::{QuicDialer, QuicListener, QuicSettings};
use crate::transport::{Dialer, Listener, Settings};
use crate::udp;

/// How many idle reverse connections the entry may queue before it starts dropping them.
const POOL_CAPACITY: usize = 1024;
/// Direct mode without mux: how often the entry tries the exit side when idle, so the
/// tunnel's state is the truth.
const PROBE_EVERY: Duration = Duration::from_secs(10);

struct Entry {
    crypto: Crypto,
    tuning: Tuning,
    source: Source,
    stats: Arc<Stats>,
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
    let sessions = SessionConfig::new(&mux);
    let stats = Stats::new(&config);
    let mut tasks = JoinSet::new();

    let source = if transport.kind == TransportKind::Quic {
        Source::Mux(quic_pool(&config, &crypto, &sessions, &stats, &mut tasks).await?)
    } else {
        match config.mode {
            Mode::Direct if mux.enabled => {
                let remote = config.tunnel.remote.as_deref().expect("validated");
                info!(
                    remote,
                    connections = mux.connections,
                    "entry: direct mode with mux, keeping sessions to the exit side"
                );
                let dialer = Arc::new(if config.tunnel.transport == TransportKind::Auto {
                    Dial::auto(&transport, &config.tunnel.token, remote, &tuning)?
                } else {
                    Dial::one(&transport, remote, &tuning)?
                });
                let pool = Arc::new(SessionPool::default());
                for _ in 0..mux.connections {
                    let (dialer, crypto, pool) = (dialer.clone(), crypto.clone(), pool.clone());
                    let wait = tuning.dial_timeout + tuning.handshake_timeout;
                    let sessions = sessions.clone();
                    let connect = move || {
                        let (dialer, crypto, sessions) =
                            (dialer.clone(), crypto.clone(), sessions.clone());
                        async move {
                            let (link, via) = dialer.connect(&crypto, wait).await?;
                            let session = MuxSession::over(link, Side::Client, sessions);
                            if let Some(via) = via {
                                session.set_transport(via);
                            }
                            Ok(Session::Kmux(session))
                        }
                    };
                    let (lifetime, link) = (mux.max_lifetime, stats.peer.clone());
                    tasks.spawn(async move {
                        maintain("the exit side", link, connect, lifetime, move |s| {
                            pool.add(s)
                        })
                        .await;
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
                // Nothing keeps a connection open here, so without this the tunnel would
                // not say whether the exit side is reachable until someone used it (and a
                // panel creating the tunnel would wait for a "connected" that never comes).
                let probe = Dialer::new(&transport, remote, &tuning)?;
                let (crypto, link) = (crypto.clone(), stats.peer.clone());
                let wait = tuning.dial_timeout + tuning.handshake_timeout;
                tasks.spawn(async move {
                    loop {
                        match timeout(wait, channel::connect(&probe, &crypto, &[])).await {
                            Ok(Ok(_)) => link.link_up(),
                            Ok(Err(e)) => link.failed(&e),
                            Err(_) => link.failed(&"handshake timed out"),
                        }
                        sleep(PROBE_EVERY).await;
                    }
                });
                Source::Direct(Dialer::new(&transport, remote, &tuning)?)
            }
            Mode::Reverse => {
                let addr = config.tunnel.listen.as_deref().expect("validated");
                let listeners = if config.tunnel.transport == TransportKind::Auto {
                    auto::bind_all(&transport, &config.tunnel.token, addr, &tuning).await?
                } else {
                    vec![(
                        "",
                        Listener::bind(&transport, addr, &tuning)
                            .await
                            .with_context(|| {
                                format!("failed to listen for tunnel connections on {addr}")
                            })?,
                    )]
                };
                info!(
                    addr = %listeners[0].1.local_addr()?,
                    mux = mux.enabled,
                    "entry: reverse mode, waiting for the exit side"
                );
                let hs = tuning.handshake_timeout;
                if mux.enabled {
                    let pool = Arc::new(SessionPool::default());
                    let (p, link_stats) = (pool.clone(), stats.peer.clone());
                    let on_link =
                        Arc::new(move |link: Link, peer: SocketAddr, via: &'static str| {
                            let session = MuxSession::over(link, Side::Client, sessions.clone());
                            if !via.is_empty() {
                                session.set_transport(via);
                            }
                            let session = Arc::new(Session::Kmux(session));
                            link_stats.session_up(&session);
                            p.add(session);
                            info!(%peer, "mux session from the exit side established");
                        });
                    for (via, listener) in listeners {
                        let on_link = on_link.clone();
                        let accept = accept_reverse(
                            listener,
                            crypto.clone(),
                            hs,
                            stats.clone(),
                            move |link, peer| on_link(link, peer, via),
                        );
                        tasks.spawn(accept);
                    }
                    Source::Mux(pool)
                } else {
                    let (_, listener) = listeners.into_iter().next().expect("a listener");
                    let (tx, rx) = mpsc::channel(POOL_CAPACITY);
                    let link_stats = stats.peer.clone();
                    let on_link = move |link: Link, peer: SocketAddr| {
                        link_stats.link_up();
                        debug!(%peer, "tunnel connection added to pool");
                        if tx.try_send(link).is_err() {
                            warn!(%peer, "tunnel pool is full, dropping connection");
                        }
                    };
                    let accept =
                        accept_reverse(listener, crypto.clone(), hs, stats.clone(), on_link);
                    tasks.spawn(accept);
                    Source::Reverse(Mutex::new(rx))
                }
            }
        }
    };

    let entry = Arc::new(Entry {
        crypto,
        tuning,
        source,
        stats: stats.clone(),
    });

    // `kariz status` reads the counters and `kariz speedtest` reaches the live sessions
    // through this (Unix only).
    #[cfg(unix)]
    if let Some(socket) = config.control_socket() {
        let entry = entry.clone();
        let open = move |open: Bytes| {
            let entry = entry.clone();
            async move { entry.open_channel(&open).await }
        };
        tasks.spawn(crate::control::serve(socket, Some(open), stats.clone()));
    }

    for (forward, counters) in config.forward.iter().zip(&stats.forwards) {
        if forward.protocol.has_tcp() {
            let listener = TcpListener::bind(&forward.listen)
                .await
                .with_context(|| format!("failed to listen on forward port {}", forward.listen))?;
            let (entry, forward, counters) = (entry.clone(), forward.clone(), counters.clone());
            tasks.spawn(accept_users(entry, listener, forward, counters));
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
            let counters = counters.clone();
            tasks.spawn(async move {
                udp::serve(socket, udp_tuning, target, duplicate, counters, opener)
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
    stats: &Arc<Stats>,
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
                let (lifetime, link) = (mux.max_lifetime, stats.peer.clone());
                tasks.spawn(async move {
                    maintain("the exit side", link, connect, lifetime, move |s| {
                        pool.add(s)
                    })
                    .await;
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
            let (p, link) = (pool.clone(), stats.peer.clone());
            let on_session = move |session: QuicSession, peer: SocketAddr| {
                let session = Arc::new(Session::Quic(session));
                link.session_up(&session);
                p.add(session);
                info!(%peer, "quic session from the exit side established");
            };
            let (sessions, link) = (sessions.clone(), stats.peer.clone());
            tasks.spawn(async move {
                accept_sessions(listener, sessions, open_timeout, link, on_session).await;
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
    _: &Arc<Stats>,
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
    stats: Arc<Stats>,
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
        let stats = stats.clone();
        tokio::spawn(async move {
            match channel::accept(stream, &crypto, &replay, handshake_timeout).await {
                Ok(link) => on_link(link, peer),
                Err(e) => {
                    stats.peer.failed(&e);
                    warn!(%peer, error = %e, "tunnel handshake failed")
                }
            }
        });
    }
}

async fn accept_users(
    entry: Arc<Entry>,
    listener: TcpListener,
    forward: Forward,
    counters: Arc<ForwardStats>,
) -> Result<()> {
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
        let (target, counters) = (forward.target.clone(), counters.clone());
        tokio::spawn(async move {
            if let Err(e) = handle_user(&entry, user, &open, &counters).await {
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

async fn handle_user(
    entry: &Entry,
    mut user: TcpStream,
    open: &Bytes,
    stats: &Arc<ForwardStats>,
) -> io::Result<()> {
    let _open = stats.tcp_connection();
    user.set_nodelay(entry.tuning.nodelay)?;
    let size = entry.tuning.buffer_size;
    let channel = entry
        .open_channel(open)
        .await
        .inspect_err(|_| stats.open_failed())?;
    let counters = Counters {
        read: &stats.traffic.up,
        written: &stats.traffic.down,
    };
    let result = match channel {
        Channel::Stream(stream) => relay_stream(&mut user, &stream, size, counters).await,
        Channel::Link(mut link) => relay(&mut user, &mut link, size, counters).await,
    };
    // With mux the stream is used before the exit side answers; it resets the stream
    // when it cannot reach the target.
    if result
        .as_ref()
        .is_err_and(|e| e.kind() == io::ErrorKind::ConnectionRefused)
    {
        stats.open_failed();
    }
    result.map(|_| ())
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
            let connected = channel::connect(dialer, &self.crypto, open).await;
            let mut link = connected.inspect_err(|e| self.stats.peer.failed(e))?;
            self.stats.peer.link_up();
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
