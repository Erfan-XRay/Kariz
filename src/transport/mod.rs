//! Tunnel transports.
//!
//! Each transport provides a [`Listener`] and a [`Dialer`] that produce [`TunnelStream`]s.
//! Dispatch is done with enums instead of trait objects so the hot path stays
//! statically dispatched. New transports (wss, quic, kcp, udp, icmp) are added as
//! new variants. `tcpmux` is the `tcp` transport with mux on top, so it has no variant.

pub mod tcp;
pub mod ws;

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::config::{TransportKind, Tuning, TunnelConfig, WsConfig};

/// The transport part of `[tunnel]`: everything listeners and dialers need besides the
/// address.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub kind: TransportKind,
    pub ws: Option<WsConfig>,
}

impl Settings {
    pub fn new(tunnel: &TunnelConfig) -> Self {
        Self {
            kind: tunnel.transport,
            ws: tunnel.ws.clone(),
        }
    }
}

pub enum Listener {
    Tcp(tcp::TcpTransportListener),
    Ws(tcp::TcpTransportListener, Arc<ws::ServerConfig>),
}

impl Listener {
    pub async fn bind(settings: &Settings, addr: &str, tuning: &Tuning) -> io::Result<Self> {
        let tcp = tcp::TcpTransportListener::bind(addr, tuning).await?;
        match settings.kind {
            TransportKind::Tcp | TransportKind::Tcpmux => Ok(Self::Tcp(tcp)),
            TransportKind::Ws => Ok(Self::Ws(
                tcp,
                Arc::new(ws::ServerConfig::new(settings.ws.as_ref())),
            )),
            TransportKind::Wss => Err(unsupported(settings.kind)),
        }
    }

    /// Accepts a connection. Transports with their own handshake (the WebSocket
    /// upgrade) finish it in [`Incoming::establish`], so a slow client never holds up
    /// the accept loop.
    pub async fn accept(&self) -> io::Result<(Incoming, SocketAddr)> {
        match self {
            Self::Tcp(l) => {
                let (s, peer) = l.accept().await?;
                Ok((Incoming::Tcp(s), peer))
            }
            Self::Ws(l, config) => {
                let (stream, peer) = l.accept().await?;
                let config = config.clone();
                Ok((
                    Incoming::Ws {
                        stream,
                        config,
                        peer,
                    },
                    peer,
                ))
            }
        }
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        match self {
            Self::Tcp(l) | Self::Ws(l, _) => l.local_addr(),
        }
    }
}

/// An accepted connection whose transport handshake may still be pending.
pub enum Incoming {
    Tcp(TcpStream),
    Ws {
        stream: TcpStream,
        config: Arc<ws::ServerConfig>,
        peer: SocketAddr,
    },
}

impl Incoming {
    /// Finishes the transport handshake. A rejected WebSocket upgrade has already been
    /// answered (`404`) and closed when this returns an error. Callers bound the time.
    pub async fn establish(self) -> io::Result<TunnelStream> {
        match self {
            Self::Tcp(s) => Ok(TunnelStream::Tcp(s)),
            Self::Ws {
                mut stream,
                config,
                peer,
            } => {
                let read_ahead = ws::upgrade::accept(&mut stream, &config, peer).await?;
                let (r, w) = stream.into_split();
                Ok(TunnelStream::Ws(ws::WsStream::new(
                    r,
                    w,
                    ws::Role::Server,
                    &read_ahead,
                )?))
            }
        }
    }
}

pub enum Dialer {
    Tcp(tcp::TcpTransportDialer),
    Ws(tcp::TcpTransportDialer, Box<WsDialer>),
}

/// WebSocket part of a [`Dialer`].
pub struct WsDialer {
    config: ws::ClientConfig,
    /// Bound on the upgrade round trip.
    timeout: std::time::Duration,
}

impl Dialer {
    pub fn new(settings: &Settings, addr: &str, tuning: &Tuning) -> io::Result<Self> {
        let tcp = tcp::TcpTransportDialer::new(addr, tuning);
        match settings.kind {
            TransportKind::Tcp | TransportKind::Tcpmux => Ok(Self::Tcp(tcp)),
            TransportKind::Ws => Ok(Self::Ws(
                tcp,
                Box::new(WsDialer {
                    config: ws::ClientConfig::new(settings.ws.as_ref(), addr, "http", 80),
                    timeout: tuning.handshake_timeout,
                }),
            )),
            TransportKind::Wss => Err(unsupported(settings.kind)),
        }
    }

    pub async fn dial(&self) -> io::Result<TunnelStream> {
        match self {
            Self::Tcp(d) => Ok(TunnelStream::Tcp(d.dial().await?)),
            Self::Ws(d, ws) => {
                let mut stream = d.dial().await?;
                let read_ahead = timeout(ws.timeout, ws::upgrade::connect(&mut stream, &ws.config))
                    .await
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::TimedOut, "websocket upgrade timed out")
                    })??;
                let (r, w) = stream.into_split();
                Ok(TunnelStream::Ws(ws::WsStream::new(
                    r,
                    w,
                    ws::Role::Client,
                    &read_ahead,
                )?))
            }
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
    Tcp(TcpStream),
    Ws(ws::WsStream<OwnedReadHalf, OwnedWriteHalf>),
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
            // Anything from the peer (even a ping) means the connection is not idle.
            Self::Ws(s) => {
                let mut probe = [0u8; 1];
                !s.reader().has_buffered()
                    && matches!(
                        s.reader().get_ref().try_read(&mut probe),
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock
                    )
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
            Self::Ws(s) => Pin::new(s).poll_read(cx, buf),
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
            Self::Ws(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_write_vectored(cx, bufs),
            Self::Ws(s) => Pin::new(s).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Tcp(s) => s.is_write_vectored(),
            Self::Ws(s) => s.is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_flush(cx),
            Self::Ws(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_shutdown(cx),
            Self::Ws(s) => Pin::new(s).poll_shutdown(cx),
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
            Self::Ws(s) => {
                let (r, w) = s.into_split();
                (TunnelReader::Ws(r), TunnelWriter::Ws(w))
            }
        }
    }
}

/// Receiving half of a [`TunnelStream`].
pub enum TunnelReader {
    Tcp(OwnedReadHalf),
    Ws(ws::WsReader<OwnedReadHalf>),
}

/// Sending half of a [`TunnelStream`].
pub enum TunnelWriter {
    Tcp(OwnedWriteHalf),
    Ws(ws::WsWriter<OwnedWriteHalf>),
}

impl AsyncRead for TunnelReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_read(cx, buf),
            Self::Ws(s) => Pin::new(s).poll_read(cx, buf),
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
            Self::Ws(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_write_vectored(cx, bufs),
            Self::Ws(s) => Pin::new(s).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Tcp(s) => s.is_write_vectored(),
            Self::Ws(s) => s.is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_flush(cx),
            Self::Ws(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_shutdown(cx),
            Self::Ws(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}
