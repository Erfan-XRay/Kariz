//! The session layer: many streams (user TCP connections) and datagrams (UDP flows) over
//! one tunnel connection.
//!
//! Entry, exit and the UDP code work with [`Session`] and [`SessionStream`] only. Today
//! the one implementation is kmux (`src/mux/`), running over an authenticated link;
//! QUIC, which brings its own streams and datagrams, becomes a second variant (see
//! `docs/PHASE4.md`). Enums rather than trait objects keep dispatch static.

mod manager;

use std::io;

use bytes::Bytes;

use crate::mux::{MuxSession, MuxStream};

pub use crate::mux::ResetReason;
pub use manager::{maintain, SessionPool};

/// One tunnel connection carrying streams and datagrams.
pub enum Session {
    Kmux(MuxSession),
}

impl Session {
    /// Short name for logs.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Kmux(_) => "mux",
        }
    }

    /// Opens a stream whose first bytes are `syn` (the open request). Data can be sent
    /// right away; the peer answers a failed open with a reset.
    pub fn open(&self, syn: Bytes) -> io::Result<SessionStream> {
        match self {
            Self::Kmux(s) => s.open(syn).map(SessionStream::Kmux),
        }
    }

    /// Waits for a stream the peer opened, with its open request. `None` once the
    /// session is closed or going away.
    pub async fn accept(&self) -> Option<(SessionStream, Bytes)> {
        match self {
            Self::Kmux(s) => s
                .accept()
                .await
                .map(|(stream, syn)| (SessionStream::Kmux(stream), syn)),
        }
    }

    /// No new streams in either direction; existing ones continue.
    pub fn goaway(&self) {
        match self {
            Self::Kmux(s) => s.goaway(),
        }
    }

    pub fn is_closed(&self) -> bool {
        match self {
            Self::Kmux(s) => s.is_closed(),
        }
    }

    /// Going away or closed: takes no new streams.
    pub fn is_draining(&self) -> bool {
        match self {
            Self::Kmux(s) => s.is_draining(),
        }
    }

    pub fn stream_count(&self) -> usize {
        match self {
            Self::Kmux(s) => s.stream_count(),
        }
    }

    /// Why the session closed, once it has.
    pub fn close_reason(&self) -> Option<String> {
        match self {
            Self::Kmux(s) => s.close_reason(),
        }
    }

    /// Resolves once the session is closed.
    pub async fn closed(&self) {
        match self {
            Self::Kmux(s) => s.closed().await,
        }
    }

    pub fn close(&self) {
        match self {
            Self::Kmux(s) => s.close(),
        }
    }

    /// Stops new streams, waits for the existing ones to finish, then closes.
    pub async fn drain(&self) {
        match self {
            Self::Kmux(s) => s.drain().await,
        }
    }
}

/// One stream of a [`Session`]: a user TCP connection, or a UDP flow (whose packets
/// travel as datagrams).
#[derive(Debug)]
pub enum SessionStream {
    Kmux(MuxStream),
}

impl SessionStream {
    /// Sends `data`, waiting for flow-control credit as needed.
    pub async fn send(&self, data: Bytes) -> io::Result<()> {
        match self {
            Self::Kmux(s) => s.send(data).await,
        }
    }

    /// Receives the next chunk of data; `None` at the end of the stream.
    pub async fn recv(&self) -> io::Result<Option<Bytes>> {
        match self {
            Self::Kmux(s) => s.recv().await,
        }
    }

    /// Half close: no more data from this side.
    pub fn finish(&self) -> io::Result<()> {
        match self {
            Self::Kmux(s) => s.finish(),
        }
    }

    /// Aborts the stream and tells the peer why.
    pub fn reset(self, reason: ResetReason) {
        match self {
            Self::Kmux(s) => s.reset(reason),
        }
    }

    /// Why the stream was reset, once it has been (by either side).
    pub fn reset_reason(&self) -> Option<ResetReason> {
        match self {
            Self::Kmux(s) => s.reset_reason(),
        }
    }

    /// Queues one datagram; never waits. False if it was dropped or cannot be sent.
    pub fn send_datagram(&self, packet: Bytes) -> bool {
        match self {
            Self::Kmux(s) => s.send_datagram(packet),
        }
    }

    /// Receives the next datagram, whole; `None` once the peer finished the stream.
    pub async fn recv_datagram(&self) -> io::Result<Option<Bytes>> {
        match self {
            Self::Kmux(s) => s.recv_datagram().await,
        }
    }
}

impl tokio::io::AsyncRead for SessionStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Kmux(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for SessionStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Kmux(s) => std::pin::Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> std::task::Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Kmux(s) => std::pin::Pin::new(s).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Kmux(s) => s.is_write_vectored(),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Kmux(s) => std::pin::Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Kmux(s) => std::pin::Pin::new(s).poll_shutdown(cx),
        }
    }
}
