//! Exit side: receives open requests through the tunnel and connects to the targets.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, warn};

use crate::channel::{self, Channel};
use crate::config::{Config, Mode, Tuning};
use crate::crypto::{Crypto, ReplayFilter};
use crate::proto::{self, Open, STATUS_DIAL_FAILED, STATUS_OK};
use crate::relay::relay;
use crate::transport::{tcp, Dialer, Listener};

const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(10);

struct Exit {
    crypto: Crypto,
    tuning: Tuning,
}

pub async fn run(config: Config) -> Result<()> {
    let tuning = config.tuning();
    let exit = Arc::new(Exit {
        crypto: Crypto::new(&config.tunnel.token, config.tunnel.encryption),
        tuning: tuning.clone(),
    });
    let mut tasks = JoinSet::new();

    match config.mode {
        Mode::Reverse => {
            let remote = config.tunnel.remote.as_deref().expect("validated");
            info!(
                remote,
                pool = config.tunnel.pool,
                "exit: reverse mode, connecting to the entry side"
            );
            let dialer = Arc::new(Dialer::new(config.tunnel.transport, remote, &tuning)?);
            for _ in 0..config.tunnel.pool {
                tasks.spawn(pool_worker(exit.clone(), dialer.clone()));
            }
        }
        Mode::Direct => {
            let addr = config.tunnel.listen.as_deref().expect("validated");
            let listener = Listener::bind(config.tunnel.transport, addr, &tuning)
                .await
                .with_context(|| format!("failed to listen for tunnel connections on {addr}"))?;
            info!(addr = %listener.local_addr()?, "exit: direct mode, waiting for the entry side");
            tasks.spawn(accept_direct(exit.clone(), listener));
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
            Ok(s) => {
                backoff = BACKOFF_MIN;
                s
            }
            Err(e) => {
                if e.kind() == io::ErrorKind::PermissionDenied {
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

async fn connect_once(exit: &Exit, dialer: &Dialer) -> io::Result<Channel> {
    let stream = dialer.dial().await?;
    timeout(
        exit.tuning.handshake_timeout,
        channel::connect(stream, &exit.crypto, &[]),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "handshake timed out"))?
}

/// Direct mode: authenticates connections from the entry side and serves them.
async fn accept_direct(exit: Arc<Exit>, listener: Listener) -> Result<()> {
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
        let (exit, replay) = (exit.clone(), replay.clone());
        tokio::spawn(async move {
            let hs_timeout = exit.tuning.handshake_timeout;
            let mut channel = match channel::accept(stream, &exit.crypto, &replay, hs_timeout).await
            {
                Ok(c) => c,
                Err(e) => return warn!(%peer, error = %e, "tunnel handshake failed"),
            };
            match timeout(hs_timeout, Open::read(&mut channel)).await {
                Ok(Ok(open)) => serve(&exit, channel, open).await,
                Ok(Err(e)) => warn!(%peer, error = %e, "bad open request"),
                Err(_) => warn!(%peer, "open request timed out"),
            }
        });
    }
}

async fn serve(exit: &Exit, mut tunnel: Channel, open: Open) {
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
