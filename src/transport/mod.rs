//! Tunnel transports.
//!
//! Each transport provides a [`Listener`] and a [`Dialer`] that produce [`TunnelStream`]s.
//! Dispatch is done with enums instead of trait objects so the hot path stays
//! statically dispatched. New transports (quic, kcp, udp, icmp) are added as new
//! variants. `tcpmux` is the `tcp` transport with mux on top, so it has no variant;
//! `ws` and `wss` share the WebSocket code and differ only in the TLS layer.

pub mod tcp;
pub mod tls;
pub mod ws;

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, ReadHalf, WriteHalf};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_rustls::TlsAcceptor;

use crate::config::{TlsConfig, TransportKind, Tuning, TunnelConfig, WsConfig};

/// A TLS connection over TCP, either side.
pub type TlsTcp = tokio_rustls::TlsStream<TcpStream>;

/// The transport part of `[tunnel]`: everything listeners and dialers need besides the
/// address.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub kind: TransportKind,
    pub ws: Option<WsConfig>,
    pub tls: Option<TlsConfig>,
}

impl Settings {
    pub fn new(tunnel: &TunnelConfig) -> Self {
        Self {
            kind: tunnel.transport,
            ws: tunnel.ws.clone(),
            tls: tunnel.tls.clone(),
        }
    }
}

pub enum Listener {
    Tcp(tcp::TcpTransportListener),
    /// `ws`, or `wss` with the acceptor.
    Ws(
        tcp::TcpTransportListener,
        Arc<ws::ServerConfig>,
        Option<TlsAcceptor>,
    ),
}

impl Listener {
    pub async fn bind(settings: &Settings, addr: &str, tuning: &Tuning) -> io::Result<Self> {
        // Certificate problems are reported before the port is taken.
        let tls = match settings.kind {
            TransportKind::Wss => {
                let tls = settings.tls.as_ref();
                let (Some(cert), Some(key)) = (
                    tls.and_then(|t| t.cert.as_deref()),
                    tls.and_then(|t| t.key.as_deref()),
                ) else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "wss needs tunnel.tls.cert and tunnel.tls.key",
                    ));
                };
                Some(tls::acceptor(cert, key)?)
            }
            _ => None,
        };
        let tcp = tcp::TcpTransportListener::bind(addr, tuning).await?;
        match settings.kind {
            TransportKind::Tcp | TransportKind::Tcpmux => Ok(Self::Tcp(tcp)),
            TransportKind::Ws | TransportKind::Wss => Ok(Self::Ws(
                tcp,
                Arc::new(ws::ServerConfig::new(settings.ws.as_ref())),
                tls,
            )),
        }
    }

    /// Accepts a connection. Transports with their own handshake (TLS, the WebSocket
    /// upgrade) finish it in [`Incoming::establish`], so a slow client never holds up
    /// the accept loop.
    pub async fn accept(&self) -> io::Result<(Incoming, SocketAddr)> {
        match self {
            Self::Tcp(l) => {
                let (s, peer) = l.accept().await?;
                Ok((Incoming::Tcp(s), peer))
            }
            Self::Ws(l, config, tls) => {
                let (stream, peer) = l.accept().await?;
                let incoming = Incoming::Ws {
                    stream,
                    config: config.clone(),
                    tls: tls.clone(),
                    peer,
                };
                Ok((incoming, peer))
            }
        }
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        match self {
            Self::Tcp(l) | Self::Ws(l, ..) => l.local_addr(),
        }
    }
}

/// An accepted connection whose transport handshake may still be pending.
pub enum Incoming {
    Tcp(TcpStream),
    Ws {
        stream: TcpStream,
        config: Arc<ws::ServerConfig>,
        tls: Option<TlsAcceptor>,
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
                tls: None,
                peer,
            } => {
                let read_ahead = ws::upgrade::accept(&mut stream, &config, peer).await?;
                let (r, w) = stream.into_split();
                let ws = ws::WsStream::new(r, w, ws::Role::Server, &read_ahead)?;
                Ok(TunnelStream::Ws(ws))
            }
            Self::Ws {
                stream,
                config,
                tls: Some(acceptor),
                peer,
            } => {
                let mut stream = TlsTcp::Server(acceptor.accept(stream).await?);
                let read_ahead = ws::upgrade::accept(&mut stream, &config, peer).await?;
                let (r, w) = tokio::io::split(stream);
                let ws = ws::WsStream::new(r, w, ws::Role::Server, &read_ahead)?;
                Ok(TunnelStream::Wss(Box::new(ws)))
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
    /// Set for `wss`.
    tls: Option<tls::Client>,
    /// Bound on the TLS handshake plus the upgrade round trip.
    timeout: Duration,
}

impl Dialer {
    pub fn new(settings: &Settings, addr: &str, tuning: &Tuning) -> io::Result<Self> {
        let tcp = tcp::TcpTransportDialer::new(addr, tuning);
        let ws = settings.ws.as_ref();
        let (config, tls) = match settings.kind {
            TransportKind::Tcp | TransportKind::Tcpmux => return Ok(Self::Tcp(tcp)),
            TransportKind::Ws => (ws::ClientConfig::new(ws, addr, "http", 80), None),
            TransportKind::Wss => {
                // SNI: tls.sni, else the Host we send, else the host we dial.
                let host = ws.and_then(|w| w.host.as_deref()).unwrap_or(addr);
                let name = ws::upgrade::split_port(host).0;
                let name = name.trim_start_matches('[').trim_end_matches(']');
                let tls = tls::Client::new(settings.tls.as_ref(), name)?;
                (ws::ClientConfig::new(ws, addr, "https", 443), Some(tls))
            }
        };
        let dialer = WsDialer {
            config,
            tls,
            timeout: tuning.handshake_timeout,
        };
        Ok(Self::Ws(tcp, Box::new(dialer)))
    }

    pub async fn dial(&self) -> io::Result<TunnelStream> {
        match self {
            Self::Tcp(d) => Ok(TunnelStream::Tcp(d.dial().await?)),
            Self::Ws(d, ws) => {
                let stream = d.dial().await?;
                timeout(ws.timeout, ws.establish(stream))
                    .await
                    .map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::TimedOut,
                            "TLS or websocket handshake timed out",
                        )
                    })?
            }
        }
    }
}

impl WsDialer {
    async fn establish(&self, mut stream: TcpStream) -> io::Result<TunnelStream> {
        let Some(tls) = &self.tls else {
            let read_ahead = ws::upgrade::connect(&mut stream, &self.config).await?;
            let (r, w) = stream.into_split();
            let ws = ws::WsStream::new(r, w, ws::Role::Client, &read_ahead)?;
            return Ok(TunnelStream::Ws(ws));
        };
        let mut stream = TlsTcp::Client(tls.connect(stream).await?);
        let read_ahead = ws::upgrade::connect(&mut stream, &self.config).await?;
        let (r, w) = tokio::io::split(stream);
        let ws = ws::WsStream::new(r, w, ws::Role::Client, &read_ahead)?;
        Ok(TunnelStream::Wss(Box::new(ws)))
    }
}

/// A single tunnel connection, whatever transport carries it.
pub enum TunnelStream {
    Tcp(TcpStream),
    Ws(ws::WsStream<OwnedReadHalf, OwnedWriteHalf>),
    /// Boxed: the TLS state is large, and it is one allocation per connection.
    /// TLS state is shared by both directions, so its halves share a lock.
    Wss(Box<ws::WsStream<ReadHalf<TlsTcp>, WriteHalf<TlsTcp>>>),
}

impl TunnelStream {
    /// Cheap, non-blocking check that an idle connection has not been closed by the peer.
    /// Only meaningful while the peer is not expected to send anything: whatever
    /// arrives counts as "not idle", and may be consumed by the check.
    pub fn is_alive(&mut self) -> bool {
        let mut probe = [0u8; 1];
        let would_block =
            |r: io::Result<usize>| matches!(r, Err(e) if e.kind() == io::ErrorKind::WouldBlock);
        match self {
            Self::Tcp(s) => would_block(s.try_read(&mut probe)),
            // Anything from the peer (even a ping) means the connection is not idle.
            Self::Ws(s) => {
                !s.reader().has_buffered() && would_block(s.reader().get_ref().try_read(&mut probe))
            }
            // TLS may carry records that are not data (tickets, key updates), so read
            // through it; pending means idle and open.
            Self::Wss(s) => {
                let waker = Waker::from(Arc::new(NoopWaker));
                let mut buf = ReadBuf::new(&mut probe);
                Pin::new(&mut **s)
                    .poll_read(&mut Context::from_waker(&waker), &mut buf)
                    .is_pending()
            }
        }
    }
}

struct NoopWaker;

impl Wake for NoopWaker {
    fn wake(self: Arc<Self>) {}
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
            Self::Wss(s) => Pin::new(&mut **s).poll_read(cx, buf),
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
            Self::Wss(s) => Pin::new(&mut **s).poll_write(cx, buf),
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
            Self::Wss(s) => Pin::new(&mut **s).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Tcp(s) => s.is_write_vectored(),
            Self::Ws(s) => s.is_write_vectored(),
            Self::Wss(s) => s.is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_flush(cx),
            Self::Ws(s) => Pin::new(s).poll_flush(cx),
            Self::Wss(s) => Pin::new(&mut **s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_shutdown(cx),
            Self::Ws(s) => Pin::new(s).poll_shutdown(cx),
            Self::Wss(s) => Pin::new(&mut **s).poll_shutdown(cx),
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
            Self::Wss(s) => {
                let (r, w) = s.into_split();
                (
                    TunnelReader::Wss(Box::new(r)),
                    TunnelWriter::Wss(Box::new(w)),
                )
            }
        }
    }
}

/// Receiving half of a [`TunnelStream`].
pub enum TunnelReader {
    Tcp(OwnedReadHalf),
    Ws(ws::WsReader<OwnedReadHalf>),
    Wss(Box<ws::WsReader<ReadHalf<TlsTcp>>>),
}

/// Sending half of a [`TunnelStream`].
pub enum TunnelWriter {
    Tcp(OwnedWriteHalf),
    Ws(ws::WsWriter<OwnedWriteHalf>),
    Wss(Box<ws::WsWriter<WriteHalf<TlsTcp>>>),
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
            Self::Wss(s) => Pin::new(&mut **s).poll_read(cx, buf),
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
            Self::Wss(s) => Pin::new(&mut **s).poll_write(cx, buf),
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
            Self::Wss(s) => Pin::new(&mut **s).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Tcp(s) => s.is_write_vectored(),
            Self::Ws(s) => s.is_write_vectored(),
            Self::Wss(s) => s.is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_flush(cx),
            Self::Ws(s) => Pin::new(s).poll_flush(cx),
            Self::Wss(s) => Pin::new(&mut **s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Tcp(s) => Pin::new(s).poll_shutdown(cx),
            Self::Ws(s) => Pin::new(s).poll_shutdown(cx),
            Self::Wss(s) => Pin::new(&mut **s).poll_shutdown(cx),
        }
    }
}
