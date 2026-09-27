//! Tunnel transports.
//!
//! Each transport provides a [`Listener`] and a [`Dialer`] that produce [`TunnelStream`]s.
//! Dispatch is done with enums instead of trait objects so the hot path stays
//! statically dispatched. New transports (ws, wss, quic, kcp, udp, icmp) are added as
//! new variants. `tcpmux` is the `tcp` transport with mux on top, so it has no variant.

pub mod tcp;

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::config::{TransportKind, Tuning};

pub enum Listener {
    Tcp(tcp::TcpTransportListener),
}

impl Listener {
    pub async fn bind(kind: TransportKind, addr: &str, tuning: &Tuning) -> io::Result<Self> {
        match kind {
            TransportKind::Tcp | TransportKind::Tcpmux => Ok(Self::Tcp(
                tcp::TcpTransportListener::bind(addr, tuning).await?,
            )),
            TransportKind::Ws | TransportKind::Wss => Err(unsupported(kind)),
        }
    }

    pub async fn accept(&self) -> io::Result<(TunnelStream, SocketAddr)> {
        match self {
            Self::Tcp(l) => {
                let (s, peer) = l.accept().await?;
                Ok((TunnelStream::Tcp(s), peer))
            }
        }
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        match self {
            Self::Tcp(l) => l.local_addr(),
        }
    }
}

pub enum Dialer {
    Tcp(tcp::TcpTransportDialer),
}

impl Dialer {
    pub fn new(kind: TransportKind, addr: &str, tuning: &Tuning) -> io::Result<Self> {
        match kind {
            TransportKind::Tcp | TransportKind::Tcpmux => {
                Ok(Self::Tcp(tcp::TcpTransportDialer::new(addr, tuning)))
            }
            TransportKind::Ws | TransportKind::Wss => Err(unsupported(kind)),
        }
    }

    pub async fn dial(&self) -> io::Result<TunnelStream> {
        match self {
            Self::Tcp(d) => Ok(TunnelStream::Tcp(d.dial().await?)),
        }
    }
}

fn unsupported(kind: TransportKind) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("transport {} is not implemented yet", kind.name()),
    )
}

/// A single tunnel connection, whatever transport carries it.
pub enum TunnelStream {
    Tcp(tokio::net::TcpStream),
}

impl TunnelStream {
    /// Cheap, non-blocking check that an idle connection has not been closed by the peer.
    /// Only meaningful while the peer is not expected to send anything.
    pub fn is_alive(&self) -> bool {
        match self {
            Self::Tcp(s) => {
                let mut probe = [0u8; 1];
                matches!(s.try_read(&mut probe), Err(e) if e.kind() == io::ErrorKind::WouldBlock)
            }
        }
    }
}

impl AsyncRead for TunnelStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for TunnelStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Tcp(s) => s.is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

impl TunnelStream {
    /// Splits into independently usable halves (no lock between them).
    pub fn into_split(self) -> (TunnelReader, TunnelWriter) {
        match self {
            Self::Tcp(s) => {
                let (r, w) = s.into_split();
                (TunnelReader::Tcp(r), TunnelWriter::Tcp(w))
            }
        }
    }
}

/// Receiving half of a [`TunnelStream`].
pub enum TunnelReader {
    Tcp(tokio::net::tcp::OwnedReadHalf),
}

/// Sending half of a [`TunnelStream`].
pub enum TunnelWriter {
    Tcp(tokio::net::tcp::OwnedWriteHalf),
}

impl AsyncRead for TunnelReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for TunnelWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Tcp(s) => s.is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}
