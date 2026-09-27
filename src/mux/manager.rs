//! Session management: which mux sessions a tunnel has, where new streams go, and
//! keeping sessions up on the dialing side.

use std::future::Future;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::Notify;
use tokio::time::{sleep, timeout_at, Instant};
use tracing::{debug, error, info, warn};

use super::{MuxSession, MuxStream, SessionConfig, Side, Transport};
use crate::crypto::random_below;

const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(10);
/// A session that closes sooner than this after connecting counts as a failed attempt.
const MIN_HEALTHY: Duration = Duration::from_secs(1);
/// How often a waiting `open` looks again (a full session may have freed a slot).
const RETRY: Duration = Duration::from_millis(100);

/// The live sessions of one tunnel. New streams go to the least busy one.
#[derive(Default)]
pub struct SessionPool {
    sessions: Mutex<Vec<Arc<MuxSession>>>,
    added: Notify,
}

impl SessionPool {
    fn lock(&self) -> MutexGuard<'_, Vec<Arc<MuxSession>>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn add(&self, session: Arc<MuxSession>) {
        {
            let mut sessions = self.lock();
            sessions.retain(|s| !s.is_closed());
            sessions.push(session);
        }
        self.added.notify_waiters();
    }

    /// Sessions that currently take new streams.
    pub fn available(&self) -> usize {
        self.lock().iter().filter(|s| !s.is_draining()).count()
    }

    /// Opens a stream on the live session with the fewest streams, waiting until
    /// `deadline` for one to become available.
    pub async fn open(&self, syn: Bytes, deadline: Instant) -> io::Result<MuxStream> {
        loop {
            let added = self.added.notified();
            tokio::pin!(added);
            added.as_mut().enable();
            if let Some(stream) = self.try_open(&syn) {
                return Ok(stream);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "no tunnel connection available (is the other side running?)",
                ));
            }
            let _ = timeout_at(deadline.min(Instant::now() + RETRY), added).await;
        }
    }

    fn try_open(&self, syn: &Bytes) -> Option<MuxStream> {
        let mut sessions = self.lock();
        sessions.retain(|s| !s.is_closed());
        let mut candidates: Vec<(usize, &Arc<MuxSession>)> = sessions
            .iter()
            .filter(|s| !s.is_draining())
            .map(|s| (s.stream_count(), s))
            .collect();
        candidates.sort_by_key(|(streams, _)| *streams);
        candidates
            .into_iter()
            .find_map(|(_, s)| s.open(syn.clone()).ok())
    }
}

/// Keeps one mux session up on the dialing side. `connect` opens and authenticates a
/// tunnel connection; each new session is handed to `on_session`. A closed session is
/// replaced, with backoff while connecting fails.
///
/// With `lifetime`, a session is also replaced after roughly that long (±10 %, so
/// several sessions do not rotate in lockstep): the replacement is connected first, then
/// the old session stops taking streams and closes once its streams are done.
pub async fn maintain<T, C, Fut>(
    peer: &'static str,
    mut connect: C,
    side: Side,
    config: SessionConfig,
    lifetime: Option<Duration>,
    on_session: impl Fn(Arc<MuxSession>),
) where
    C: FnMut() -> Fut,
    Fut: Future<Output = io::Result<T>>,
    T: Transport,
{
    let mut backoff = BACKOFF_MIN;
    let mut retiring: Option<Arc<MuxSession>> = None;
    loop {
        let io = match connect().await {
            Ok(io) => io,
            Err(e) => {
                match e.kind() {
                    io::ErrorKind::PermissionDenied | io::ErrorKind::Unsupported => {
                        error!(error = %e, "{peer} rejected the tunnel connection")
                    }
                    _ => warn!(error = %e, "could not connect to {peer}"),
                }
                sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_MAX);
                continue;
            }
        };
        let session = Arc::new(MuxSession::over(io, side, config.clone()));
        let started = Instant::now();
        on_session(session.clone());
        info!("mux session to {peer} established");
        if let Some(old) = retiring.take() {
            tokio::spawn(async move { old.drain().await });
        }

        let rotate = async {
            match lifetime {
                Some(l) => sleep(jitter(l)).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = session.closed() => {
                let reason = session.close_reason().unwrap_or_default();
                if started.elapsed() < MIN_HEALTHY {
                    warn!(%reason, "mux session to {peer} closed right after connecting");
                    sleep(backoff).await;
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                } else {
                    info!(%reason, "mux session to {peer} closed, reconnecting");
                    backoff = BACKOFF_MIN;
                }
            }
            _ = rotate => {
                debug!("rotating mux session to {peer}");
                backoff = BACKOFF_MIN;
                retiring = Some(session);
            }
        }
    }
}

fn jitter(d: Duration) -> Duration {
    let tenth = d / 10;
    let spread = random_below(1000).unwrap_or(500);
    d - tenth + tenth * 2 * spread / 1000
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::tests::config;

    fn session_pair() -> (Arc<MuxSession>, MuxSession) {
        let (a, b) = tokio::io::duplex(64 * 1024);
        (
            Arc::new(MuxSession::new(a, Side::Client, config())),
            MuxSession::new(b, Side::Server, config()),
        )
    }

    #[tokio::test]
    async fn open_waits_for_a_session_then_balances() {
        let pool = Arc::new(SessionPool::default());
        let deadline = Instant::now() + Duration::from_secs(5);

        // No session yet: open waits until one is added.
        let p = pool.clone();
        let waiting = tokio::spawn(async move { p.open(Bytes::from_static(b"t"), deadline).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (s1, _peer1) = session_pair();
        pool.add(s1.clone());
        let first = waiting.await.unwrap().unwrap();

        // The next stream goes to the emptier session.
        let (s2, _peer2) = session_pair();
        pool.add(s2.clone());
        let second = pool.open(Bytes::from_static(b"t"), deadline).await.unwrap();
        assert_eq!((s1.stream_count(), s2.stream_count()), (1, 1));

        // Draining and closed sessions take no new streams.
        s1.goaway();
        s2.close();
        let err = pool
            .open(
                Bytes::from_static(b"t"),
                Instant::now() + Duration::from_millis(200),
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(pool.available(), 0);
        drop((first, second));
    }

    #[tokio::test]
    async fn maintain_reconnects_and_rotates() {
        let pool = Arc::new(SessionPool::default());
        let (peers_tx, mut peers_rx) = tokio::sync::mpsc::unbounded_channel();
        let connect = move || {
            let (a, b) = tokio::io::duplex(64 * 1024);
            let _ = peers_tx.send(MuxSession::new(b, Side::Server, config()));
            async move { Ok::<_, io::Error>(a) }
        };
        let p = pool.clone();
        let lifetime = Some(Duration::from_millis(300));
        tokio::spawn(maintain(
            "test peer",
            connect,
            Side::Client,
            config(),
            lifetime,
            move |s| p.add(s),
        ));

        // First session comes up.
        let first = peers_rx.recv().await.unwrap();
        // Rotation: a second session is connected, and the first one drains and closes.
        let second = tokio::time::timeout(Duration::from_secs(2), peers_rx.recv())
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), first.closed())
            .await
            .unwrap();
        assert!(!second.is_closed());

        // Killing the peer of a session makes maintain reconnect.
        drop(second);
        let third = tokio::time::timeout(Duration::from_secs(3), peers_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(!third.is_closed());
    }
}
