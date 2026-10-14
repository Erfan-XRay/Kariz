//! Session management: which sessions a tunnel has, where new streams go, and keeping
//! sessions up on the dialing side. Works for any [`Session`] kind.

use std::future::Future;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::Notify;
use tokio::time::{sleep, timeout_at, Instant};
use tracing::{debug, error, info, warn};

use super::{Session, SessionStream};
use crate::crypto::random_below;
use crate::stats::Peer;

const BACKOFF_MIN: Duration = Duration::from_millis(500);
const BACKOFF_MAX: Duration = Duration::from_secs(10);
/// A session that closes sooner than this after connecting counts as a failed attempt.
const MIN_HEALTHY: Duration = Duration::from_secs(1);
/// How often a waiting `open` looks again (a full session may have freed a slot).
const RETRY: Duration = Duration::from_millis(100);

/// The live sessions of one tunnel. New streams go to the least busy one.
#[derive(Default)]
pub struct SessionPool {
    sessions: Mutex<Vec<Arc<Session>>>,
    added: Notify,
}

impl SessionPool {
    fn lock(&self) -> MutexGuard<'_, Vec<Arc<Session>>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn add(&self, session: Arc<Session>) {
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
    /// `deadline` for one to become available. When none did because every session was
    /// full, the error is [`is_full`].
    pub async fn open(&self, syn: Bytes, deadline: Instant) -> io::Result<SessionStream> {
        loop {
            let added = self.added.notified();
            tokio::pin!(added);
            added.as_mut().enable();
            let streams = match self.try_open(&syn) {
                Ok(stream) => return Ok(stream),
                Err(streams) => streams,
            };
            if Instant::now() >= deadline {
                return Err(match streams {
                    Some(streams) => io::Error::other(Full { streams }),
                    None => io::Error::new(
                        io::ErrorKind::TimedOut,
                        "no tunnel connection available (is the other side running?)",
                    ),
                });
            }
            let _ = timeout_at(deadline.min(Instant::now() + RETRY), added).await;
        }
    }

    /// `Err(Some(streams))`: there are sessions, but none took the stream (they carry
    /// `streams` in all); `Err(None)`: there is no session to try.
    fn try_open(&self, syn: &Bytes) -> Result<SessionStream, Option<usize>> {
        let mut sessions = self.lock();
        sessions.retain(|s| !s.is_closed());
        let mut candidates: Vec<(usize, &Arc<Session>)> = sessions
            .iter()
            .filter(|s| !s.is_draining())
            .map(|s| (s.stream_count(), s))
            .collect();
        candidates.sort_by_key(|(streams, _)| *streams);
        let streams = (!candidates.is_empty()).then(|| candidates.iter().map(|c| c.0).sum());
        candidates
            .into_iter()
            .find_map(|(_, s)| s.open(syn.clone()).ok())
            .ok_or(streams)
    }
}

/// Every session to the other side had `mux.max_streams` open for as long as a new one
/// was waited for.
#[derive(Debug)]
struct Full {
    streams: usize,
}

impl std::fmt::Display for Full {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "every tunnel connection is full ({} streams open); raise tunnel.mux.max_streams \
             or tunnel.mux.connections",
            self.streams
        )
    }
}

impl std::error::Error for Full {}

/// Whether `e` says every session was full (see [`SessionPool::open`]).
pub fn is_full(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|e| e.is::<Full>())
}

/// Keeps one session up on the dialing side. `connect` opens, authenticates and starts a
/// session; each new one is handed to `on_session`. A closed session is replaced, with
/// backoff while connecting fails.
///
/// With `lifetime`, a session is also replaced after roughly that long (±10 %, so
/// several sessions do not rotate in lockstep): the replacement is connected first, then
/// the old session stops taking streams and closes once its streams are done.
///
/// Sessions coming up and failed attempts are counted in `link`.
pub async fn maintain<C, Fut>(
    peer: &'static str,
    link: Arc<Peer>,
    mut connect: C,
    lifetime: Option<Duration>,
    on_session: impl Fn(Arc<Session>),
) where
    C: FnMut() -> Fut,
    Fut: Future<Output = io::Result<Session>>,
{
    let mut backoff = BACKOFF_MIN;
    let mut retiring: Option<Arc<Session>> = None;
    loop {
        let session = match connect().await {
            Ok(session) => Arc::new(session),
            Err(e) => {
                link.failed(&e);
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
        let kind = session.kind();
        let started = Instant::now();
        link.session_up(&session);
        on_session(session.clone());
        info!("{kind} session to {peer} established");
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
                    warn!(%reason, "{kind} session to {peer} closed right after connecting");
                    sleep(backoff).await;
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                } else {
                    info!(%reason, "{kind} session to {peer} closed, reconnecting");
                    backoff = BACKOFF_MIN;
                }
            }
            _ = rotate => {
                debug!("rotating {kind} session to {peer}");
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
    use crate::mux::{MuxSession, Side};

    fn session_pair() -> (Arc<Session>, MuxSession) {
        let (a, b) = tokio::io::duplex(64 * 1024);
        (
            Arc::new(Session::Kmux(MuxSession::new(a, Side::Client, config()))),
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
        assert!(!is_full(&err), "no session is not a full session");
        assert_eq!(pool.available(), 0);
        drop((first, second));
    }

    #[tokio::test]
    async fn a_pool_whose_sessions_are_all_full_says_so() {
        let (a, b) = tokio::io::duplex(64 * 1024);
        let one_stream = crate::mux::SessionConfig {
            max_streams: 1,
            ..config()
        };
        let session = Arc::new(Session::Kmux(MuxSession::new(a, Side::Client, one_stream)));
        let _peer = MuxSession::new(b, Side::Server, config());
        let pool = SessionPool::default();
        pool.add(session);
        let soon = || Instant::now() + Duration::from_millis(200);
        let held = pool.open(Bytes::from_static(b"t"), soon()).await.unwrap();
        let err = pool
            .open(Bytes::from_static(b"t"), soon())
            .await
            .unwrap_err();
        assert!(is_full(&err), "{err}");
        assert!(err.to_string().contains("1 streams open"), "{err}");
        // A slot that frees up while waiting is taken.
        let wait = Instant::now() + Duration::from_secs(5);
        let (next, ()) = tokio::join!(pool.open(Bytes::from_static(b"t"), wait), async {
            tokio::time::sleep(Duration::from_millis(150)).await;
            drop(held);
        });
        assert!(next.is_ok());
    }

    #[tokio::test]
    async fn maintain_reconnects_and_rotates() {
        let pool = Arc::new(SessionPool::default());
        let (peers_tx, mut peers_rx) = tokio::sync::mpsc::unbounded_channel();
        let connect = move || {
            let (a, b) = tokio::io::duplex(64 * 1024);
            let _ = peers_tx.send(MuxSession::new(b, Side::Server, config()));
            async move { Ok(Session::Kmux(MuxSession::new(a, Side::Client, config()))) }
        };
        let p = pool.clone();
        let lifetime = Some(Duration::from_millis(300));
        let link = Arc::new(Peer::default());
        tokio::spawn(maintain("test peer", link, connect, lifetime, move |s| {
            p.add(s)
        }));

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
