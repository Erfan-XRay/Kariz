//! Bidirectional byte relay between a user connection and a tunnel connection.
//!
//! Both relays add to the rule's (or the exit's) counters as data goes, not when the
//! connection ends, so a connection that stays open for hours shows up in `status`.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio::time::sleep;

use crate::session::SessionStream;

/// Where a relay counts the bytes it moves: `read` for what it reads from the socket,
/// `written` for what it writes to it.
#[derive(Clone, Copy)]
pub struct Counters<'a> {
    pub read: &'a AtomicU64,
    pub written: &'a AtomicU64,
}

/// Copies data in both directions until both sides are done, counting what is read from
/// and written to `a`. Returns `(a_to_b, b_to_a)` byte counts.
pub async fn relay<A, B>(
    a: &mut A,
    b: &mut B,
    buffer_size: usize,
    counters: Counters<'_>,
) -> io::Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    let mut a = Counted { io: a, counters };
    tokio::io::copy_bidirectional_with_sizes(&mut a, b, buffer_size, buffer_size).await
}

/// An I/O object that adds what passes through it to [`Counters`].
struct Counted<'a, 'c, T: ?Sized> {
    io: &'a mut T,
    counters: Counters<'c>,
}

impl<T: AsyncRead + Unpin + ?Sized> AsyncRead for Counted<'_, '_, T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let result = Pin::new(&mut *self.io).poll_read(cx, buf);
        let n = buf.filled().len() - before;
        if n > 0 {
            self.counters.read.fetch_add(n as u64, Relaxed);
        }
        result
    }
}

impl<T: AsyncWrite + Unpin + ?Sized> AsyncWrite for Counted<'_, '_, T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut *self.io).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = result {
            self.counters.written.fetch_add(n as u64, Relaxed);
        }
        result
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.io).poll_shutdown(cx)
    }
}

/// Relays between a TCP connection and a session stream without copying payloads in
/// user space: what is read from the socket is handed to the session as is, and what
/// arrives on the stream is written to the socket straight from the frames it came in.
/// Returns `(socket_to_stream, stream_to_socket)` byte counts.
pub async fn relay_stream(
    socket: &mut TcpStream,
    stream: &SessionStream,
    buffer_size: usize,
    counters: Counters<'_>,
) -> io::Result<(u64, u64)> {
    relay_stream_with(socket, stream, buffer_size, counters, HALF_CLOSED_IDLE).await
}

/// How long a connection may stay open after one of its directions has ended while
/// nothing moves the other way. Without it, a user that sent its FIN (or got ours) and
/// then went silent for good kept the connection, its stream and its buffers until the
/// tunnel restarted.
pub const HALF_CLOSED_IDLE: Duration = Duration::from_secs(120);

async fn relay_stream_with(
    socket: &mut TcpStream,
    stream: &SessionStream,
    buffer_size: usize,
    counters: Counters<'_>,
    idle: Duration,
) -> io::Result<(u64, u64)> {
    let (mut from_socket, mut to_socket) = socket.split();
    let ended = Ended::default();
    let up = async {
        let mut buf = BytesMut::new();
        let mut total = 0u64;
        loop {
            // Chunks already handed to the mux keep their part of the allocation alive;
            // start a fresh one once little room is left.
            if buf.capacity() < buffer_size / 4 {
                buf.reserve(buffer_size);
            }
            if ended.within(idle, from_socket.read_buf(&mut buf)).await? == 0 {
                stream.finish()?;
                ended.set();
                return Ok(total);
            }
            total += buf.len() as u64;
            counters.read.fetch_add(buf.len() as u64, Relaxed);
            ended.within(idle, stream.send(buf.split().freeze())).await?;
        }
    };
    let down = async {
        let mut total = 0u64;
        while let Some(chunk) = ended.within(idle, stream.recv()).await? {
            ended.within(idle, to_socket.write_all(&chunk)).await?;
            total += chunk.len() as u64;
            counters.written.fetch_add(chunk.len() as u64, Relaxed);
        }
        to_socket.shutdown().await?;
        ended.set();
        Ok(total)
    };
    tokio::try_join!(up, down)
}

/// Set once one direction of a relay has ended; from then on the other one has to keep
/// moving.
#[derive(Default)]
struct Ended {
    done: AtomicBool,
    notify: Notify,
}

impl Ended {
    fn set(&self) {
        self.done.store(true, Relaxed);
        self.notify.notify_waiters();
    }

    /// `io`, or a timeout once the other direction has ended and `io` waited `idle`.
    async fn within<T>(
        &self,
        idle: Duration,
        io: impl Future<Output = io::Result<T>>,
    ) -> io::Result<T> {
        let half_closed = async {
            loop {
                // Made before the flag is read, so a `set` in between still wakes it.
                let woken = self.notify.notified();
                if self.done.load(Relaxed) {
                    break;
                }
                woken.await;
            }
            sleep(idle).await;
        };
        tokio::select! {
            result = io => result,
            () = half_closed => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "half-closed connection was idle",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::{MuxSession, Side};
    use bytes::Bytes;

    /// The exit side ends its half (a target that answered and closed), the user takes
    /// our FIN and then never sends nor closes: the relay gives up instead of waiting.
    #[tokio::test]
    async fn a_half_closed_connection_that_goes_silent_is_closed() {
        let config = crate::mux::tests::config();
        let (a, b) = tokio::io::duplex(64 * 1024);
        let (near, far) = (
            MuxSession::new(a, Side::Client, config.clone()),
            MuxSession::new(b, Side::Server, config),
        );
        let remote = near.open(Bytes::from_static(b"open")).unwrap();
        let (stream, _) = far.accept().await.unwrap();
        let stream = SessionStream::Kmux(stream);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut user = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (mut socket, _) = listener.accept().await.unwrap();

        remote.send(Bytes::from_static(b"answer")).await.unwrap();
        remote.finish().unwrap();
        let (read, written) = (AtomicU64::new(0), AtomicU64::new(0));
        let counters = Counters {
            read: &read,
            written: &written,
        };
        let relaying = relay_stream_with(
            &mut socket,
            &stream,
            256,
            counters,
            Duration::from_millis(200),
        );
        let result = tokio::time::timeout(Duration::from_secs(5), relaying)
            .await
            .expect("the relay waited for the silent user for good");
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        // The user got the answer and the end of it; its own side stays open.
        let mut got = Vec::new();
        user.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"answer");
    }

    /// Data that keeps moving holds a half-closed connection open.
    #[tokio::test]
    async fn a_half_closed_connection_that_still_moves_stays() {
        let config = crate::mux::tests::config();
        let (a, b) = tokio::io::duplex(64 * 1024);
        let (near, far) = (
            MuxSession::new(a, Side::Client, config.clone()),
            MuxSession::new(b, Side::Server, config),
        );
        let remote = near.open(Bytes::from_static(b"open")).unwrap();
        let (stream, _) = far.accept().await.unwrap();
        let stream = SessionStream::Kmux(stream);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut user = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (mut socket, _) = listener.accept().await.unwrap();
        let relaying = tokio::spawn(async move {
            let (read, written) = (AtomicU64::new(0), AtomicU64::new(0));
            let counters = Counters {
                read: &read,
                written: &written,
            };
            relay_stream_with(
                &mut socket,
                &stream,
                256,
                counters,
                Duration::from_millis(300),
            )
            .await
        });
        // The user is done sending; the answer comes slowly, a chunk every 100 ms.
        user.shutdown().await.unwrap();
        for _ in 0..8 {
            remote.send(Bytes::from_static(b"x")).await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        remote.finish().unwrap();
        let mut got = Vec::new();
        user.read_to_end(&mut got).await.unwrap();
        assert_eq!(got.len(), 8);
        assert_eq!(relaying.await.unwrap().unwrap(), (0, 8));
    }

    #[tokio::test]
    async fn relay_counts_both_ways_as_it_goes() {
        let (mut user, mut a) = tokio::io::duplex(1024);
        let (mut b, mut target) = tokio::io::duplex(1024);
        let relaying = tokio::spawn(async move {
            let (read, written) = (AtomicU64::new(0), AtomicU64::new(0));
            let counters = Counters {
                read: &read,
                written: &written,
            };
            relay(&mut a, &mut b, 256, counters).await.unwrap();
            (read.into_inner(), written.into_inner())
        });
        // More than the pipes hold, so writing and reading go on together.
        let mut got = vec![0; 3000];
        let (sent, received) =
            tokio::join!(user.write_all(&[1; 3000]), target.read_exact(&mut got));
        sent.unwrap();
        received.unwrap();
        target.write_all(&[2; 500]).await.unwrap();
        let mut back = vec![0; 500];
        user.read_exact(&mut back).await.unwrap();
        drop((user, target));
        // Read from the user's side: 3000; written to it: 500.
        assert_eq!(relaying.await.unwrap(), (3000, 500));
    }
}
