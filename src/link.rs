//! A management link: one authenticated, encrypted mux session between two Kariz programs,
//! made the way a tunnel's connections are (the token handshake, the encrypted records,
//! the mux), so it resists DPI like a tunnel does. The web panel uses it to talk to its
//! agents; nothing in the tunnel core uses it.
//!
//! The transport is `tcpmux`, `kcp` or `wss` ([`LINK_TRANSPORTS`]): a panel listens on all
//! three (TCP and UDP of its agents port, and TLS on the next port, [`wss_addr`]), and an
//! agent uses the one that gets through. Which side dials and which opens streams are
//! independent: an agent dials the panel, but the panel is the one that opens streams
//! (requests).
//!
//! Over `wss` the dialing side does not check the certificate: TLS is only there to look
//! like a web site. Who is on the other end is proven by the token handshake inside, as on
//! the other transports.

use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;

use crate::channel;
use crate::config::{Config, TransportKind, Tuning};
use crate::crypto::{Crypto, ReplayFilter};
use crate::mux::{MuxSession, SessionConfig, Side};
use crate::transport::{Dialer as TransportDialer, Incoming, Listener, Settings};

/// The transports a management link can use, in the order an agent tries them. Some
/// networks let a TCP connection open and then stall it; KCP (over UDP) often gets
/// through those, and WebSocket over TLS looks like an ordinary web site.
pub const LINK_TRANSPORTS: [TransportKind; 3] = [
    TransportKind::Tcpmux,
    TransportKind::Kcp,
    TransportKind::Wss,
];

/// The `wss` address of a panel whose agents port is in `addr`: the next port, as the
/// other transports use the agents port itself (TCP and UDP).
pub fn wss_addr(addr: &str) -> io::Result<String> {
    next_port(addr)
}

/// `addr` with its port one higher.
pub fn next_port(addr: &str) -> io::Result<String> {
    let bad = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not an address with a port: {addr}"),
        )
    };
    let (host, port) = addr.rsplit_once(':').ok_or_else(bad)?;
    let port: u16 = port.parse().map_err(|_| bad())?;
    let next = port.checked_add(1).ok_or_else(bad)?;
    Ok(format!("{host}:{next}"))
}

/// The WebSocket path of the link: made from the token, so both ends know it and a
/// scanner does not.
fn ws_path(token: &str) -> String {
    let hash = blake3::hash(format!("kariz link ws {token}").as_bytes());
    format!("/{}", &hash.to_hex()[..16])
}

/// The synthetic config of one end. `tls`: the listening side's certificate and key for
/// `wss`.
fn toml(
    kind: TransportKind,
    listening: bool,
    addr: &str,
    token: &str,
    tls: Option<(&Path, &Path)>,
) -> io::Result<String> {
    check(kind)?;
    let (mode, key) = if listening {
        ("direct", "listen")
    } else {
        ("reverse", "remote")
    };
    let mut toml = format!(
        "role = \"exit\"
mode = \"{mode}\"
[tunnel]
transport = \"{}\"
{key} = {addr:?}
token = {token:?}
",
        kind.name()
    );
    if kind == TransportKind::Wss {
        toml.push_str(&format!(
            "[tunnel.ws]
path = {:?}
[tunnel.tls]
",
            ws_path(token)
        ));
        if listening {
            let (cert, key) = tls.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a wss link needs a certificate and its key",
                )
            })?;
            toml.push_str(&format!(
                "cert = {:?}
key = {:?}
",
                cert.display().to_string(),
                key.display().to_string()
            ));
        } else {
            toml.push_str(
                "insecure = true
",
            );
        }
    }
    Ok(toml)
}

fn check(kind: TransportKind) -> io::Result<()> {
    if LINK_TRANSPORTS.contains(&kind) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("a management link cannot use {}", kind.name()),
        ))
    }
}

/// What both ends share, worked out once from a small synthetic config so the link
/// follows the same defaults as a `tcpmux` tunnel.
struct Common {
    crypto: Crypto,
    tuning: Tuning,
    settings: Settings,
    sessions: SessionConfig,
}

fn common(toml: String) -> io::Result<Common> {
    let config = Config::parse(&toml)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("{e:#}")))?;
    let mux = config.mux();
    Ok(Common {
        crypto: Crypto::new(&config.tunnel.token, config.tunnel.encryption).with_mux(true),
        tuning: config.tuning(),
        settings: Settings::new(&config.tunnel, config.kcp()),
        sessions: SessionConfig::new(&mux),
    })
}

/// The listening end.
pub struct Acceptor {
    kind: TransportKind,
    listener: Listener,
    replay: Arc<ReplayFilter>,
    common: Arc<Common>,
}

impl Acceptor {
    /// Listens on `addr`; only a peer that knows `token` gets a session.
    pub async fn bind(addr: &str, token: &str) -> io::Result<Self> {
        Self::bind_via(addr, token, TransportKind::Tcpmux).await
    }

    /// Like [`Acceptor::bind`], over `tcpmux` or `kcp`.
    pub async fn bind_via(addr: &str, token: &str, kind: TransportKind) -> io::Result<Self> {
        Self::bind_with(addr, token, kind, None).await
    }

    /// Like [`Acceptor::bind`], over `wss` with this certificate (reloaded when its files
    /// change).
    pub async fn bind_wss(addr: &str, token: &str, cert: &Path, key: &Path) -> io::Result<Self> {
        Self::bind_with(addr, token, TransportKind::Wss, Some((cert, key))).await
    }

    async fn bind_with(
        addr: &str,
        token: &str,
        kind: TransportKind,
        tls: Option<(&Path, &Path)>,
    ) -> io::Result<Self> {
        let toml = toml(kind, true, addr, token, tls)?;
        let common = common(toml)?;
        let listener = Listener::bind(&common.settings, addr, &common.tuning).await?;
        Ok(Self {
            kind,
            listener,
            replay: Arc::new(ReplayFilter::default()),
            common: Arc::new(common),
        })
    }

    /// The transport it takes links over.
    pub fn kind(&self) -> TransportKind {
        self.kind
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// The next connection. It is not authenticated yet: hand it to a task and call
    /// [`Pending::establish`], so a slow or hostile peer never holds up this loop.
    pub async fn accept(&self) -> io::Result<Pending> {
        let (incoming, peer) = self.listener.accept().await?;
        Ok(Pending {
            incoming,
            peer,
            replay: self.replay.clone(),
            common: self.common.clone(),
        })
    }
}

/// A connection that has not finished its handshake.
pub struct Pending {
    incoming: Incoming,
    peer: SocketAddr,
    replay: Arc<ReplayFilter>,
    common: Arc<Common>,
}

impl Pending {
    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// Runs the handshake and starts the session, with `side` as this end's mux role.
    /// A peer with a wrong token is kept waiting for a random 5-30 s before it is let go,
    /// like a tunnel does.
    pub async fn establish(self, side: Side) -> io::Result<MuxSession> {
        let c = &self.common;
        let link = channel::accept(
            self.incoming,
            &c.crypto,
            &self.replay,
            c.tuning.handshake_timeout,
        )
        .await?;
        Ok(MuxSession::over(link, side, c.sessions.clone()))
    }
}

/// The dialing end.
pub struct Dialer {
    dialer: TransportDialer,
    common: Common,
}

impl Dialer {
    /// Dials `addr` with `token`.
    pub fn new(addr: &str, token: &str) -> io::Result<Self> {
        Self::via(addr, token, TransportKind::Tcpmux)
    }

    /// Like [`Dialer::new`], over `kind` (one of [`LINK_TRANSPORTS`]; for `wss`, `addr` is
    /// the panel's [`wss_addr`]).
    pub fn via(addr: &str, token: &str, kind: TransportKind) -> io::Result<Self> {
        let toml = toml(kind, false, addr, token, None)?;
        let common = common(toml)?;
        let dialer = TransportDialer::new(&common.settings, addr, &common.tuning)?;
        Ok(Self { dialer, common })
    }

    /// Connects, authenticates and starts a session, with `side` as this end's mux role.
    pub async fn connect(&self, side: Side) -> io::Result<MuxSession> {
        let c = &self.common;
        let wait: Duration = c.tuning.dial_timeout + c.tuning.handshake_timeout;
        let link = timeout(wait, channel::connect(&self.dialer, &c.crypto, &[]))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "handshake timed out"))??;
        Ok(MuxSession::over(link, side, c.sessions.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    #[tokio::test]
    async fn a_dialer_and_an_acceptor_share_streams_in_either_direction() {
        let acceptor = Acceptor::bind("127.0.0.1:0", TOKEN).await.unwrap();
        let addr = acceptor.local_addr().unwrap().to_string();
        // The dialing end is the mux server: the accepting end opens the streams.
        let dialer = Dialer::new(&addr, TOKEN).unwrap();
        let (dialed, accepted) = tokio::join!(dialer.connect(Side::Server), async {
            let pending = acceptor.accept().await.unwrap();
            pending.establish(Side::Client).await
        });
        let (dialed, accepted) = (dialed.unwrap(), accepted.unwrap());

        let opened = accepted.open(Bytes::from_static(b"ping")).unwrap();
        let (stream, syn) = dialed.accept().await.unwrap();
        assert_eq!(&syn[..], b"ping");
        stream.send(Bytes::from_static(b"pong")).await.unwrap();
        stream.finish().unwrap();
        assert_eq!(
            opened.recv().await.unwrap().unwrap(),
            Bytes::from_static(b"pong")
        );
    }

    /// Three requests from the accepting end, answered by the dialing end.
    async fn round_trips(acceptor: Acceptor, dialer: Dialer) {
        let (dialed, accepted) = tokio::join!(dialer.connect(Side::Server), async {
            let pending = acceptor.accept().await.unwrap();
            pending.establish(Side::Client).await
        });
        let (dialed, accepted) = (dialed.unwrap(), accepted.unwrap());
        for round in 0..3u8 {
            let opened = accepted.open(Bytes::from(vec![round; 3])).unwrap();
            opened.finish().unwrap();
            let (stream, syn) = dialed.accept().await.unwrap();
            assert_eq!(&syn[..], &[round; 3]);
            stream.send(Bytes::from_static(b"pong")).await.unwrap();
            stream.finish().unwrap();
            assert_eq!(
                opened.recv().await.unwrap().unwrap(),
                Bytes::from_static(b"pong")
            );
        }
    }

    #[cfg(feature = "kcp")]
    #[tokio::test]
    async fn a_link_works_over_kcp_too() {
        let acceptor = Acceptor::bind_via("127.0.0.1:0", TOKEN, TransportKind::Kcp)
            .await
            .unwrap();
        let addr = acceptor.local_addr().unwrap().to_string();
        let dialer = Dialer::via(&addr, TOKEN, TransportKind::Kcp).unwrap();
        round_trips(acceptor, dialer).await;
        assert!(Dialer::via(&addr, TOKEN, TransportKind::Ws).is_err());
    }

    #[tokio::test]
    async fn a_link_works_over_wss_with_any_certificate() {
        // A self-signed certificate for another name: the dialing end does not check it.
        let c = rcgen::generate_simple_self_signed(vec!["panel.example".to_owned()]).unwrap();
        let dir = std::env::temp_dir().join(format!("kariz-link-wss-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
        std::fs::write(&cert, c.cert.pem()).unwrap();
        std::fs::write(&key, c.signing_key.serialize_pem()).unwrap();
        let acceptor = Acceptor::bind_wss("127.0.0.1:0", TOKEN, &cert, &key)
            .await
            .unwrap();
        let addr = acceptor.local_addr().unwrap().to_string();
        let dialer = Dialer::via(&addr, TOKEN, TransportKind::Wss).unwrap();
        round_trips(acceptor, dialer).await;
        // Without a certificate there is no wss listener.
        assert!(Acceptor::bind_via("127.0.0.1:0", TOKEN, TransportKind::Wss)
            .await
            .is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_wss_port_is_the_next_one() {
        assert_eq!(wss_addr("203.0.113.5:29001").unwrap(), "203.0.113.5:29002");
        assert_eq!(
            wss_addr("[2001:db8::1]:29001").unwrap(),
            "[2001:db8::1]:29002"
        );
        assert!(wss_addr("host").is_err());
        assert!(wss_addr("host:65535").is_err());
    }

    #[tokio::test]
    async fn a_wrong_token_never_gets_a_session() {
        let acceptor = Acceptor::bind("127.0.0.1:0", TOKEN).await.unwrap();
        let addr = acceptor.local_addr().unwrap().to_string();
        let dialer = Dialer::new(&addr, "another-token-0123456789abcdef").unwrap();
        tokio::spawn(async move {
            if let Ok(pending) = acceptor.accept().await {
                let _ = pending.establish(Side::Client).await;
            }
        });
        let result =
            tokio::time::timeout(Duration::from_secs(15), dialer.connect(Side::Server)).await;
        assert!(
            !matches!(result, Ok(Ok(_))),
            "a wrong token must not connect"
        );
    }
}
