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
use crate::config::{Config, Forward, Mode, Tuning};
use crate::crypto::{Crypto, ReplayFilter};
use crate::mux::{maintain, MuxSession, SessionConfig, SessionPool, Side};
use crate::proto::{self, Open};
use crate::relay::{relay, relay_mux};
use crate::transport::{Dialer, Listener, Settings};

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
    Reverse(Mutex<mpsc::Receiver<Channel>>),
    /// Mux, either mode: open a stream on one of the sessions to the exit side.
    Mux(Arc<SessionPool>),
}

pub async fn run(config: Config) -> Result<()> {
    let tuning = config.tuning();
    let mux = config.mux();
    let transport = Settings::new(&config.tunnel);
    let crypto = Crypto::new(&config.tunnel.token, config.tunnel.encryption).with_mux(mux.enabled);
    let sessions = SessionConfig::new(&mux, &tuning);
    let mut tasks = JoinSet::new();

    let source = match config.mode {
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
                let handshake_timeout = tuning.handshake_timeout;
                let connect = move || {
                    let (dialer, crypto) = (dialer.clone(), crypto.clone());
                    async move {
                        let stream = dialer.dial().await?;
                        timeout(handshake_timeout, channel::connect(stream, &crypto, &[]))
                            .await
                            .map_err(|_| {
                                io::Error::new(io::ErrorKind::TimedOut, "handshake timed out")
                            })?
                    }
                };
                let (sessions, lifetime) = (sessions.clone(), mux.max_lifetime);
                tasks.spawn(async move {
                    maintain(
                        "the exit side",
                        connect,
                        Side::Client,
                        sessions,
                        lifetime,
                        move |s| pool.add(s),
                    )
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
            Source::Direct(Dialer::new(&transport, remote, &tuning)?)
        }
        Mode::Reverse => {
            let addr = config.tunnel.listen.as_deref().expect("validated");
            let listener = Listener::bind(&transport, addr, &tuning)
                .await
                .with_context(|| format!("failed to listen for tunnel connections on {addr}"))?;
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
                    p.add(Arc::new(MuxSession::over(
                        link,
                        Side::Client,
                        sessions.clone(),
                    )));
                    info!(%peer, "mux session from the exit side established");
                };
                tasks.spawn(accept_reverse(listener, crypto.clone(), hs, on_link));
                Source::Mux(pool)
            } else {
                let (tx, rx) = mpsc::channel(POOL_CAPACITY);
                let on_link = move |link: Link, peer: SocketAddr| {
                    debug!(%peer, "tunnel connection added to pool");
                    if tx.try_send(Channel::from(link)).is_err() {
                        warn!(%peer, "tunnel pool is full, dropping connection");
                    }
                };
                tasks.spawn(accept_reverse(listener, crypto.clone(), hs, on_link));
                Source::Reverse(Mutex::new(rx))
            }
        }
    };

    let entry = Arc::new(Entry {
        crypto,
        tuning,
        source,
    });

    for forward in &config.forward {
        let listener = TcpListener::bind(&forward.listen)
            .await
            .with_context(|| format!("failed to listen on forward port {}", forward.listen))?;
        info!(listen = %forward.listen, target = %forward.target, "forward ready");
        tasks.spawn(accept_users(entry.clone(), listener, forward.clone()));
    }

    // Accept loops only return on fatal errors.
    match tasks.join_next().await {
        Some(Ok(result)) => result,
        Some(Err(e)) => Err(e.into()),
        None => Ok(()),
    }
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
    let open = encode_open(&forward);
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

fn encode_open(forward: &Forward) -> Bytes {
    let mut frame = Vec::new();
    Open::tcp(forward.target.clone()).encode(&mut frame);
    frame.into()
}

async fn handle_user(entry: &Entry, mut user: TcpStream, open: &Bytes) -> io::Result<()> {
    user.set_nodelay(entry.tuning.nodelay)?;
    let size = entry.tuning.buffer_size;
    match entry.open_channel(open).await? {
        Channel::Mux(stream) => relay_mux(&mut user, &stream, size).await?,
        mut channel => relay(&mut user, &mut channel, size).await?,
    };
    Ok(())
}

impl Entry {
    /// Gets a channel to the exit side and has the exit connect it to the target
    /// described by the encoded `open` request.
    async fn open_channel(&self, open: &Bytes) -> io::Result<Channel> {
        match &self.source {
            Source::Direct(dialer) => self.open_direct(dialer, open).await,
            Source::Reverse(pool) => self.open_reverse(pool, open).await,
            // Optimistic: the stream is used right away; if the exit cannot reach the
            // target it resets the stream, which ends the relay.
            Source::Mux(pool) => {
                let deadline =
                    Instant::now() + self.tuning.handshake_timeout + self.tuning.dial_timeout;
                Ok(Channel::Mux(pool.open(open.clone(), deadline).await?))
            }
        }
    }

    /// Dials the exit side and sends the open request along with the hello.
    async fn open_direct(&self, dialer: &Dialer, open: &[u8]) -> io::Result<Channel> {
        let stream = dialer.dial().await?;
        let wait = self.tuning.handshake_timeout + self.tuning.dial_timeout;
        timeout(wait, async {
            let mut channel = Channel::from(channel::connect(stream, &self.crypto, open).await?);
            proto::read_status(&mut channel).await?;
            Ok(channel)
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "exit side did not answer in time"))?
    }

    /// Takes pooled channels until one accepts the open request. Pooled channels may
    /// have died while idle, so failures other than "target unreachable" are retried.
    async fn open_reverse(
        &self,
        pool: &Mutex<mpsc::Receiver<Channel>>,
        open: &[u8],
    ) -> io::Result<Channel> {
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
