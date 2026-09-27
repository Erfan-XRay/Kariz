//! Plain TCP transport.

use std::io;
use std::net::SocketAddr;

use socket2::{SockRef, TcpKeepalive};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

use crate::config::Tuning;

pub struct TcpTransportListener {
    inner: TcpListener,
    tuning: Tuning,
}

impl TcpTransportListener {
    pub async fn bind(addr: &str, tuning: &Tuning) -> io::Result<Self> {
        Ok(Self {
            inner: TcpListener::bind(addr).await?,
            tuning: tuning.clone(),
        })
    }

    pub async fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let (stream, peer) = self.inner.accept().await?;
        tune_tunnel_socket(&stream, &self.tuning)?;
        Ok((stream, peer))
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }
}

pub struct TcpTransportDialer {
    addr: String,
    tuning: Tuning,
}

impl TcpTransportDialer {
    pub fn new(addr: &str, tuning: &Tuning) -> Self {
        Self {
            addr: addr.to_owned(),
            tuning: tuning.clone(),
        }
    }

    pub async fn dial(&self) -> io::Result<TcpStream> {
        let stream = connect(&self.addr, &self.tuning).await?;
        tune_tunnel_socket(&stream, &self.tuning)?;
        Ok(stream)
    }
}

/// Connects to `addr` (resolved on every call) within the configured dial timeout.
pub async fn connect(addr: &str, tuning: &Tuning) -> io::Result<TcpStream> {
    let stream = timeout(tuning.dial_timeout, TcpStream::connect(addr))
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("connect to {addr} timed out"),
            )
        })??;
    stream.set_nodelay(tuning.nodelay)?;
    Ok(stream)
}

/// Socket options for long-lived tunnel connections: nodelay plus TCP keepalive,
/// so NAT mappings stay open and dead peers are noticed.
fn tune_tunnel_socket(stream: &TcpStream, tuning: &Tuning) -> io::Result<()> {
    stream.set_nodelay(tuning.nodelay)?;
    let keepalive = TcpKeepalive::new()
        .with_time(tuning.keepalive)
        .with_interval(tuning.keepalive);
    SockRef::from(stream).set_tcp_keepalive(&keepalive)
}
