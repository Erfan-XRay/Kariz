//! Channels: authenticated paths through the tunnel, one per user connection.
//!
//! Entry and exit only deal with [`Channel`]s and never touch the handshake or the
//! transport directly. Today a channel is a whole tunnel connection; encryption wraps it
//! in a record layer and mux adds channels that share one connection, without changing
//! the callers.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};

use crate::auth::{self, AuthKey, ReplayFilter};
use crate::transport::TunnelStream;

pub enum Channel {
    /// A tunnel connection used for a single user connection.
    Raw(TunnelStream),
}

impl Channel {
    /// Cheap, non-blocking check that an idle channel has not been closed by the peer.
    /// Only meaningful while the peer is not expected to send anything.
    pub fn is_alive(&self) -> bool {
        match self {
            Self::Raw(s) => s.is_alive(),
        }
    }
}

/// Dialing side: authenticates a fresh tunnel connection. `early` (possibly empty) goes
/// out in the same write as the hello, to save a round trip.
pub async fn connect(mut stream: TunnelStream, key: &AuthKey, early: &[u8]) -> io::Result<Channel> {
    let (hello, pending) = key.hello()?;
    let mut msg = Vec::with_capacity(hello.len() + early.len());
    msg.extend_from_slice(&hello);
    msg.extend_from_slice(early);
    stream.write_all(&msg).await?;
    auth::read_reply(&mut stream, key, &pending).await?;
    Ok(Channel::Raw(stream))
}

/// Listening side: authenticates a tunnel connection the other side opened.
pub async fn accept(
    mut stream: TunnelStream,
    key: &AuthKey,
    replay: &ReplayFilter,
) -> io::Result<Channel> {
    auth::server_handshake(&mut stream, key, replay).await?;
    Ok(Channel::Raw(stream))
}

impl AsyncRead for Channel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Raw(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Channel {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Raw(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Raw(s) => Pin::new(s).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Raw(s) => s.is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Raw(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Raw(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}
