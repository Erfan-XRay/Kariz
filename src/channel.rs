//! Channels: authenticated paths through the tunnel, one per user connection.
//!
//! Entry and exit only deal with [`Channel`]s and never touch the handshake or the
//! transport directly. The handshake turns a transport connection into a [`Link`] (an
//! authenticated, normally encrypted connection). A channel is then either a whole link
//! or one stream of a mux session running over a link.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::time::{timeout, timeout_at, Instant};

use crate::crypto::handshake::{self, SessionKeys};
use crate::crypto::record::{
    Opener, Sealer, SecureReader, SecureStream, SecureWriter, MAX_PAYLOAD,
};
use crate::crypto::{random_below, Cipher, Crypto, ReplayFilter};
use crate::mux::{MuxStream, Transport};
use crate::transport::{TunnelReader, TunnelStream, TunnelWriter};

/// A failed handshake is drained for a random time in this range (seconds).
const DRAIN_SECS: (u64, u64) = (5, 30);
/// Stop reading (but keep the connection open) after this many drained bytes.
const DRAIN_MAX_BYTES: usize = 1 << 20;

/// An authenticated tunnel connection, the result of a handshake.
pub enum Link {
    /// `encryption = "none"`: authenticated, but the bytes are sent as they are.
    Plain(TunnelStream),
    /// Boxed: the cipher key schedules make it large, and it is one allocation per
    /// tunnel connection, not per packet.
    Secure(Box<SecureStream<TunnelStream>>),
}

impl Link {
    fn new(
        stream: TunnelStream,
        keys: SessionKeys,
        dialer: bool,
        early: Option<&handshake::Key>,
    ) -> io::Result<Self> {
        if keys.cipher == Cipher::None {
            return Ok(Self::Plain(stream));
        }
        let (send, recv) = if dialer {
            (&keys.c2s, &keys.s2c)
        } else {
            (&keys.s2c, &keys.c2s)
        };
        let early = early.map(|k| Opener::new(keys.cipher, k)).transpose()?;
        Ok(Self::Secure(Box::new(SecureStream::new(
            stream,
            Sealer::new(keys.cipher, send)?,
            Opener::new(keys.cipher, recv)?,
            early,
        ))))
    }

    /// Cheap, non-blocking check that an idle link has not been closed by the peer.
    /// Only meaningful while the peer is not expected to send anything.
    pub fn is_alive(&self) -> bool {
        match self {
            Self::Plain(s) => s.is_alive(),
            Self::Secure(s) => s.get_ref().is_alive(),
        }
    }
}

/// Receiving half of a [`Link`].
pub enum LinkReader {
    Plain(TunnelReader),
    Secure(Box<SecureReader<TunnelReader>>),
}

/// Sending half of a [`Link`].
pub enum LinkWriter {
    Plain(TunnelWriter),
    Secure(Box<SecureWriter<TunnelWriter>>),
}

impl Transport for Link {
    type Reader = LinkReader;
    type Writer = LinkWriter;

    fn into_halves(self) -> (LinkReader, LinkWriter) {
        match self {
            Self::Plain(s) => {
                let (r, w) = s.into_split();
                (LinkReader::Plain(r), LinkWriter::Plain(w))
            }
            Self::Secure(s) => {
                let (r, w) = s.split(TunnelStream::into_split);
                (
                    LinkReader::Secure(Box::new(r)),
                    LinkWriter::Secure(Box::new(w)),
                )
            }
        }
    }
}

/// One user connection's path through the tunnel.
pub enum Channel {
    /// A whole link used for this connection alone.
    Link(Link),
    /// One stream of a mux session.
    Mux(MuxStream),
}

impl From<Link> for Channel {
    fn from(link: Link) -> Self {
        Self::Link(link)
    }
}

impl Channel {
    /// See [`Link::is_alive`]. Mux sessions watch their connection themselves (pings).
    pub fn is_alive(&self) -> bool {
        match self {
            Self::Link(l) => l.is_alive(),
            Self::Mux(_) => true,
        }
    }
}

/// Dialing side: runs the handshake on a fresh tunnel connection. `early` (possibly
/// empty) goes out in the same write as the hello (0-RTT), so the other side can act on
/// it before the handshake round trip completes.
pub async fn connect(mut stream: TunnelStream, crypto: &Crypto, early: &[u8]) -> io::Result<Link> {
    let cipher = crypto.cipher();
    let sealed_early = cipher != Cipher::None && !early.is_empty() && early.len() <= MAX_PAYLOAD;
    let (mut msg, state) =
        handshake::client_hello(crypto.psk(), cipher, sealed_early, crypto.mux())?;
    let mut late: &[u8] = &[];
    match state.early_key() {
        Some(key) => Sealer::new(cipher, key)?.seal(early, &mut msg)?,
        // Without encryption the early data simply follows the hello.
        None if cipher == Cipher::None => msg.extend_from_slice(early),
        // Too long for one record: send it after the handshake instead.
        None => late = early,
    }
    stream.write_all(&msg).await?;
    let keys = state.read_reply(&mut stream).await?;
    let mut link = Link::new(stream, keys, true, None)?;
    if !late.is_empty() {
        link.write_all(late).await?;
        link.flush().await?;
    }
    Ok(link)
}

/// Listening side: runs the handshake on a tunnel connection the other side opened.
///
/// A connection that fails the handshake (or does not finish it within
/// `handshake_timeout`) is not closed right away: it is read and discarded for a random
/// time first, so probing does not reveal a Kariz server by how fast it hangs up.
pub async fn accept(
    mut stream: TunnelStream,
    crypto: &Crypto,
    replay: &ReplayFilter,
    handshake_timeout: Duration,
) -> io::Result<Link> {
    let result = timeout(
        handshake_timeout,
        handshake::accept(
            &mut stream,
            crypto.psk(),
            replay,
            |c| crypto.allows(c),
            crypto.mux(),
        ),
    )
    .await
    .unwrap_or_else(|_| {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "handshake timed out",
        ))
    });
    match result {
        Ok(accepted) => Link::new(stream, accepted.keys, false, accepted.early_key.as_ref()),
        // The peer proved it knows the token; it is misconfigured, not probing.
        Err(e) if e.kind() == io::ErrorKind::Unsupported => Err(e),
        Err(e) => {
            drain(stream).await;
            Err(e)
        }
    }
}

async fn drain(mut stream: TunnelStream) {
    let (min, max) = DRAIN_SECS;
    let secs = min + random_below((max - min + 1) as u32).map_or(0, u64::from);
    let deadline = Instant::now() + Duration::from_secs(secs);
    let _ = timeout_at(deadline, async {
        let mut buf = [0u8; 4096];
        let mut total = 0;
        while total < DRAIN_MAX_BYTES {
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => total += n,
            }
        }
        std::future::pending::<()>().await
    })
    .await;
}

impl AsyncRead for Link {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Self::Secure(s) => Pin::new(&mut **s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Link {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Self::Secure(s) => Pin::new(&mut **s).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_write_vectored(cx, bufs),
            Self::Secure(s) => Pin::new(&mut **s).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Plain(s) => s.is_write_vectored(),
            Self::Secure(s) => s.is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_flush(cx),
            Self::Secure(s) => Pin::new(&mut **s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Self::Secure(s) => Pin::new(&mut **s).poll_shutdown(cx),
        }
    }
}

impl AsyncRead for Channel {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Link(s) => Pin::new(s).poll_read(cx, buf),
            Self::Mux(s) => Pin::new(s).poll_read(cx, buf),
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
            Self::Link(s) => Pin::new(s).poll_write(cx, buf),
            Self::Mux(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Link(s) => Pin::new(s).poll_write_vectored(cx, bufs),
            Self::Mux(s) => Pin::new(s).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Link(s) => s.is_write_vectored(),
            Self::Mux(s) => s.is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Link(s) => Pin::new(s).poll_flush(cx),
            Self::Mux(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Link(s) => Pin::new(s).poll_shutdown(cx),
            Self::Mux(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

impl AsyncRead for LinkReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Self::Secure(s) => Pin::new(&mut **s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for LinkWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Self::Secure(s) => Pin::new(&mut **s).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_write_vectored(cx, bufs),
            Self::Secure(s) => Pin::new(&mut **s).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Plain(s) => s.is_write_vectored(),
            Self::Secure(s) => s.is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_flush(cx),
            Self::Secure(s) => Pin::new(&mut **s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Self::Secure(s) => Pin::new(&mut **s).poll_shutdown(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Encryption, TransportKind, Tuning};
    use crate::transport::{Dialer, Listener};

    async fn listener() -> (Listener, Dialer) {
        let tuning = Tuning::for_profile(Default::default());
        let l = Listener::bind(TransportKind::Tcp, "127.0.0.1:0", &tuning)
            .await
            .unwrap();
        let addr = l.local_addr().unwrap().to_string();
        (l, Dialer::new(TransportKind::Tcp, &addr, &tuning).unwrap())
    }

    const TOKEN: &str = "channel-test-token-0123";

    #[tokio::test]
    async fn early_data_arrives_first_then_both_directions_work() {
        for (enc, early_len) in [
            (Encryption::Auto, 20),
            (Encryption::Chacha20Poly1305, MAX_PAYLOAD + 10),
            (Encryption::None, 20),
            (Encryption::Auto, 0),
        ] {
            let (l, d) = listener().await;
            let crypto = Crypto::new(TOKEN, enc);
            let c2 = crypto.clone();
            let early = vec![7u8; early_len];
            let e2 = early.clone();
            let server = tokio::spawn(async move {
                let (s, _) = l.accept().await.unwrap();
                let mut ch = accept(s, &c2, &ReplayFilter::default(), Duration::from_secs(5))
                    .await
                    .unwrap();
                let mut got = vec![0u8; e2.len() + 5];
                ch.read_exact(&mut got).await.unwrap();
                ch.write_all(b"pong").await.unwrap();
                ch.flush().await.unwrap();
                got
            });
            let mut ch = connect(d.dial().await.unwrap(), &crypto, &early)
                .await
                .unwrap();
            assert_eq!(matches!(ch, Link::Plain(_)), enc == Encryption::None);
            ch.write_all(b"hello").await.unwrap();
            ch.flush().await.unwrap();
            let mut pong = [0u8; 4];
            ch.read_exact(&mut pong).await.unwrap();
            assert_eq!(&pong, b"pong");
            assert_eq!(server.await.unwrap(), [early, b"hello".to_vec()].concat());
        }
    }

    #[tokio::test]
    async fn failed_handshake_is_not_closed_right_away() {
        let (l, d) = listener().await;
        tokio::spawn(async move {
            let (s, _) = l.accept().await.unwrap();
            let crypto = Crypto::new(TOKEN, Encryption::Auto);
            let _ = accept(s, &crypto, &ReplayFilter::default(), Duration::from_secs(5)).await;
        });
        let mut probe = d.dial().await.unwrap();
        probe.write_all(&[0x16; 600]).await.unwrap();
        let mut buf = [0u8; 16];
        let read = tokio::time::timeout(Duration::from_secs(2), probe.read(&mut buf)).await;
        assert!(read.is_err(), "probe got an answer or a close: {read:?}");
    }

    #[tokio::test]
    async fn cipher_mismatch_fails_fast() {
        let (l, d) = listener().await;
        let server = tokio::spawn(async move {
            let (s, _) = l.accept().await.unwrap();
            let crypto = Crypto::new(TOKEN, Encryption::Chacha20Poly1305);
            accept(s, &crypto, &ReplayFilter::default(), Duration::from_secs(5))
                .await
                .err()
                .unwrap()
        });
        let crypto = Crypto::new(TOKEN, Encryption::Aes256Gcm);
        let started = std::time::Instant::now();
        assert!(connect(d.dial().await.unwrap(), &crypto, &[])
            .await
            .is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(server.await.unwrap().kind(), io::ErrorKind::Unsupported);
    }

    /// Forwards one connection to `to` and returns everything that crossed it.
    async fn spy(to: std::net::SocketAddr) -> (u16, tokio::task::JoinHandle<Vec<u8>>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut a, _) = l.accept().await.unwrap();
            let mut b = tokio::net::TcpStream::connect(to).await.unwrap();
            let (mut ar, mut aw) = a.split();
            let (mut br, mut bw) = b.split();
            let mut seen = Vec::new();
            let (mut up, mut down) = (vec![0u8; 4096], vec![0u8; 4096]);
            let (mut up_open, mut down_open) = (true, true);
            while up_open || down_open {
                tokio::select! {
                    n = ar.read(&mut up), if up_open => match n.unwrap_or(0) {
                        0 => { up_open = false; let _ = bw.shutdown().await; }
                        n => { seen.extend_from_slice(&up[..n]); bw.write_all(&up[..n]).await.unwrap(); }
                    },
                    n = br.read(&mut down), if down_open => match n.unwrap_or(0) {
                        0 => { down_open = false; let _ = aw.shutdown().await; }
                        n => { seen.extend_from_slice(&down[..n]); aw.write_all(&down[..n]).await.unwrap(); }
                    },
                }
            }
            seen
        });
        (port, task)
    }

    #[tokio::test]
    async fn nothing_readable_on_the_wire() {
        const MARKER: &[u8] = b"KARIZ-PLAINTEXT-MARKER";
        for enc in [Encryption::Auto, Encryption::None] {
            let (l, _) = listener().await;
            let (port, captured) = spy(l.local_addr().unwrap()).await;
            let tuning = Tuning::for_profile(Default::default());
            let dialer =
                Dialer::new(TransportKind::Tcp, &format!("127.0.0.1:{port}"), &tuning).unwrap();
            let crypto = Crypto::new(TOKEN, enc);
            let c2 = crypto.clone();
            let server = tokio::spawn(async move {
                let (s, _) = l.accept().await.unwrap();
                let mut ch = accept(s, &c2, &ReplayFilter::default(), Duration::from_secs(5))
                    .await
                    .unwrap();
                let mut got = vec![0u8; MARKER.len() * 2];
                ch.read_exact(&mut got).await.unwrap();
                ch.write_all(MARKER).await.unwrap();
                ch.shutdown().await.unwrap();
            });
            let mut ch = connect(dialer.dial().await.unwrap(), &crypto, MARKER)
                .await
                .unwrap();
            ch.write_all(MARKER).await.unwrap();
            ch.flush().await.unwrap();
            let mut back = Vec::new();
            ch.read_to_end(&mut back).await.unwrap();
            assert_eq!(back, MARKER);
            server.await.unwrap();
            drop(ch);
            let wire = captured.await.unwrap();
            let visible = wire.windows(MARKER.len()).filter(|w| *w == MARKER).count();
            match enc {
                Encryption::None => assert_eq!(visible, 3, "control: plain mode shows the marker"),
                _ => assert_eq!(visible, 0, "marker visible on the wire with {enc:?}"),
            }
        }
    }
}
