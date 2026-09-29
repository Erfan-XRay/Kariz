//! A management link: one authenticated, encrypted mux session between two Kariz programs,
//! made the way a tunnel's connections are (the token handshake, the encrypted records,
//! the mux), so it resists DPI like a tunnel does. The web panel uses it to talk to its
//! agents (docs/PHASE11.md, section 4); nothing in the tunnel core uses it.
//!
//! The transport is `tcpmux`. Which side dials and which opens streams are independent:
//! an agent dials the panel, but the panel is the one that opens streams (requests).

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;

use crate::channel;
use crate::config::{Config, Tuning};
use crate::crypto::{Crypto, ReplayFilter};
use crate::mux::{MuxSession, SessionConfig, Side};
use crate::transport::{Dialer as TransportDialer, Incoming, Listener, Settings};

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
    listener: Listener,
    replay: Arc<ReplayFilter>,
    common: Arc<Common>,
}

impl Acceptor {
    /// Listens on `addr`; only a peer that knows `token` gets a session.
    pub async fn bind(addr: &str, token: &str) -> io::Result<Self> {
        let toml = format!(
            "role = \"exit\"\nmode = \"direct\"\n[tunnel]\ntransport = \"tcpmux\"\nlisten = {addr:?}\ntoken = {token:?}\n"
        );
        let common = common(toml)?;
        let listener = Listener::bind(&common.settings, addr, &common.tuning).await?;
        Ok(Self {
            listener,
            replay: Arc::new(ReplayFilter::default()),
            common: Arc::new(common),
        })
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
        let toml = format!(
            "role = \"exit\"\nmode = \"reverse\"\n[tunnel]\ntransport = \"tcpmux\"\nremote = {addr:?}\ntoken = {token:?}\n"
        );
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
