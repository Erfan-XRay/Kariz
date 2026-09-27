//! Entry side: accepts user connections on the `[[forward]]` ports and carries them
//! through the tunnel to the exit side.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout, timeout_at, Instant};
use tracing::{debug, info, warn};

use crate::auth::{self, AuthKey, ReplayFilter};
use crate::config::{Config, Forward, Mode, Tuning};
use crate::proto::{self, Open};
use crate::relay::relay;
use crate::transport::{Dialer, Listener, TunnelStream};

/// How many idle reverse connections the entry may queue before it starts dropping them.
const POOL_CAPACITY: usize = 1024;

struct Entry {
    key: AuthKey,
    tuning: Tuning,
    source: Source,
}

enum Source {
    /// Direct mode: dial the exit side for every user connection.
    Direct(Dialer),
    /// Reverse mode: take an idle connection the exit side opened in advance.
    Reverse(Mutex<mpsc::Receiver<TunnelStream>>),
}

pub async fn run(config: Config) -> Result<()> {
    let tuning = config.tuning();
    let key = AuthKey::new(&config.tunnel.token);
    let mut tasks = JoinSet::new();

    let source = match config.mode {
        Mode::Direct => {
            let remote = config.tunnel.remote.as_deref().expect("validated");
            info!(
                remote,
                "entry: direct mode, dialing the exit side per connection"
            );
            Source::Direct(Dialer::new(config.tunnel.transport, remote, &tuning))
        }
        Mode::Reverse => {
            let addr = config.tunnel.listen.as_deref().expect("validated");
            let listener = Listener::bind(config.tunnel.transport, addr, &tuning)
                .await
                .with_context(|| format!("failed to listen for tunnel connections on {addr}"))?;
            info!(addr = %listener.local_addr()?, "entry: reverse mode, waiting for the exit side");
            let (tx, rx) = mpsc::channel(POOL_CAPACITY);
            tasks.spawn(accept_reverse(listener, key.clone(), tuning.clone(), tx));
            Source::Reverse(Mutex::new(rx))
        }
    };

    let entry = Arc::new(Entry {
        key,
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

/// Reverse mode: authenticates incoming tunnel connections and puts them in the pool.
async fn accept_reverse(
    listener: Listener,
    key: AuthKey,
    tuning: Tuning,
    pool: mpsc::Sender<TunnelStream>,
) -> Result<()> {
    let replay = Arc::new(ReplayFilter::default());
    loop {
        let (mut stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "tunnel accept failed");
                sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let (key, replay, pool) = (key.clone(), replay.clone(), pool.clone());
        let handshake_timeout = tuning.handshake_timeout;
        tokio::spawn(async move {
            match timeout(
                handshake_timeout,
                auth::server_handshake(&mut stream, &key, &replay),
            )
            .await
            {
                Ok(Ok(())) => {
                    debug!(%peer, "tunnel connection added to pool");
                    if pool.try_send(stream).is_err() {
                        warn!(%peer, "tunnel pool is full, dropping connection");
                    }
                }
                Ok(Err(e)) => warn!(%peer, error = %e, "tunnel handshake failed"),
                Err(_) => warn!(%peer, "tunnel handshake timed out"),
            }
        });
    }
}

async fn accept_users(entry: Arc<Entry>, listener: TcpListener, forward: Forward) -> Result<()> {
    let open = Arc::new(encode_open(&forward));
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

fn encode_open(forward: &Forward) -> Vec<u8> {
    let mut frame = Vec::new();
    Open::tcp(forward.target.clone()).encode(&mut frame);
    frame
}

async fn handle_user(entry: &Entry, mut user: TcpStream, open: &[u8]) -> io::Result<()> {
    user.set_nodelay(entry.tuning.nodelay)?;
    let mut tunnel = match &entry.source {
        Source::Direct(dialer) => open_direct(entry, dialer, open).await?,
        Source::Reverse(pool) => open_reverse(entry, pool, open).await?,
    };
    relay(&mut user, &mut tunnel, entry.tuning.buffer_size).await?;
    Ok(())
}

/// Dials the exit side, then sends the hello and the open request in one write
/// to save a round trip.
async fn open_direct(entry: &Entry, dialer: &Dialer, open: &[u8]) -> io::Result<TunnelStream> {
    let mut stream = dialer.dial().await?;
    let (hello, pending) = entry.key.hello()?;
    let mut msg = Vec::with_capacity(hello.len() + open.len());
    msg.extend_from_slice(&hello);
    msg.extend_from_slice(open);
    stream.write_all(&msg).await?;

    let wait = entry.tuning.handshake_timeout + entry.tuning.dial_timeout;
    timeout(wait, async {
        auth::read_reply(&mut stream, &entry.key, &pending).await?;
        proto::read_status(&mut stream).await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "exit side did not answer in time"))??;
    Ok(stream)
}

/// Takes pooled connections until one accepts the open request. Pooled connections
/// may have died while idle, so failures other than "target unreachable" are retried.
async fn open_reverse(
    entry: &Entry,
    pool: &Mutex<mpsc::Receiver<TunnelStream>>,
    open: &[u8],
) -> io::Result<TunnelStream> {
    let deadline = Instant::now() + entry.tuning.handshake_timeout + entry.tuning.dial_timeout;
    loop {
        let mut stream = timeout_at(deadline, async { pool.lock().await.recv().await })
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "no tunnel connection available (is the exit side running?)",
                )
            })?
            .ok_or_else(|| io::Error::other("tunnel pool closed"))?;

        if !stream.is_alive() {
            continue;
        }
        if stream.write_all(open).await.is_err() {
            continue;
        }
        match timeout_at(deadline, proto::read_status(&mut stream)).await {
            Ok(Ok(())) => return Ok(stream),
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
