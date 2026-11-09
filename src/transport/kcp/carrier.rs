//! The datagram socket KCP rides on: UDP for `transport = "kcp"`, the raw IPv4 TCP
//! carrier for `transport = "paqet"`. Everything above this file (protection, KCP, FEC,
//! datagrams, keepalive, demultiplexing) is shared unchanged.

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use tokio::net::UdpSocket;
use tracing::debug;

use crate::transport::mark_dscp;

#[cfg(target_os = "linux")]
use crate::transport::paqet::raw;

/// One socket for one KCP or Paqet stream: a Datagram-type interface over either a UDP
/// socket or a Linux raw IPv4 socket. Cloning is cheap (the underlying socket is shared).
#[derive(Clone)]
pub(crate) struct PacketSocket {
    kind: Kind,
    local: SocketAddr,
    peer: Option<SocketAddr>,
}

#[derive(Clone)]
enum Kind {
    Udp(Arc<UdpSocket>),
    #[cfg(target_os = "linux")]
    Paqet(Arc<raw::RawSocket>),
}

impl PacketSocket {
    /// Binds a UDP socket for the listening side.
    pub(crate) fn udp_bind(addr: SocketAddr, buffer: usize, dscp: Option<u8>) -> io::Result<Self> {
        let socket = bind_udp(addr, buffer, dscp)?;
        let local = socket.local_addr()?;
        Ok(Self {
            kind: Kind::Udp(Arc::new(socket)),
            local,
            peer: None,
        })
    }

    /// Binds and connects a UDP socket for the dialing side.
    pub(crate) async fn udp_connect(
        peer: SocketAddr,
        buffer: usize,
        dscp: Option<u8>,
    ) -> io::Result<Self> {
        let local = match peer {
            SocketAddr::V4(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
            SocketAddr::V6(_) => SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
        };
        let socket = bind_udp(local, buffer, dscp)?;
        let local = socket.local_addr()?;
        socket.connect(peer).await?;
        Ok(Self {
            kind: Kind::Udp(Arc::new(socket)),
            local,
            peer: Some(peer),
        })
    }

    /// Binds the raw socket listener (Linux IPv4 only).
    pub(crate) fn paqet_listen(
        addr: SocketAddr,
        buffer: usize,
        dscp: Option<u8>,
    ) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            let socket = Arc::new(raw::RawSocket::listen(addr, buffer, dscp)?);
            let local = socket.local_addr();
            Ok(Self {
                kind: Kind::Paqet(socket),
                local,
                peer: None,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (addr, buffer, dscp);
            Err(unsupported())
        }
    }

    /// Creates the dialing side of the raw carrier (Linux IPv4 only).
    pub(crate) fn paqet_dial(
        peer: SocketAddr,
        buffer: usize,
        dscp: Option<u8>,
    ) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            let socket = Arc::new(raw::RawSocket::dial(peer, buffer, dscp)?);
            let local = socket.local_addr();
            Ok(Self {
                kind: Kind::Paqet(socket),
                local,
                peer: Some(peer),
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (peer, buffer, dscp);
            Err(unsupported())
        }
    }

    pub(crate) fn local_addr(&self) -> SocketAddr {
        self.local
    }

    pub(crate) fn peer_addr(&self) -> Option<SocketAddr> {
        self.peer
    }

    pub(crate) fn is_paqet(&self) -> bool {
        match &self.kind {
            Kind::Udp(_) => false,
            #[cfg(target_os = "linux")]
            Kind::Paqet(_) => true,
        }
    }

    /// The next packet from the configured peer.
    pub(crate) async fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        match &self.kind {
            Kind::Udp(socket) => socket.recv(buf).await,
            #[cfg(target_os = "linux")]
            Kind::Paqet(socket) => socket.recv(buf).await.map(|(len, _)| len),
        }
    }

    /// The next packet and where it came from.
    pub(crate) async fn recv_from(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        match &self.kind {
            Kind::Udp(socket) => socket.recv_from(buf).await,
            #[cfg(target_os = "linux")]
            Kind::Paqet(socket) => socket.recv(buf).await,
        }
    }

    /// Sends to the configured peer without waiting. `scratch` is reusable packet space
    /// for the raw carrier; UDP ignores it.
    pub(crate) fn try_send(&self, payload: &[u8], _scratch: &mut Vec<u8>) -> io::Result<usize> {
        match &self.kind {
            Kind::Udp(socket) => socket.try_send(payload),
            #[cfg(target_os = "linux")]
            Kind::Paqet(socket) => socket.try_send(payload, _scratch),
        }
    }

    /// Sends to `peer` without waiting.
    pub(crate) fn try_send_to(
        &self,
        payload: &[u8],
        peer: SocketAddr,
        _scratch: &mut Vec<u8>,
    ) -> io::Result<usize> {
        match &self.kind {
            Kind::Udp(socket) => socket.try_send_to(payload, peer),
            #[cfg(target_os = "linux")]
            Kind::Paqet(socket) => socket.try_send_to(payload, peer, _scratch),
        }
    }
}

pub(super) fn bind_udp(addr: SocketAddr, buffer: usize, dscp: Option<u8>) -> io::Result<UdpSocket> {
    let socket = std::net::UdpSocket::bind(addr)?;
    let sock = socket2::SockRef::from(&socket);
    if let Err(e) = sock
        .set_recv_buffer_size(buffer)
        .and_then(|_| sock.set_send_buffer_size(buffer))
    {
        debug!(error = %e, "could not set KCP socket buffers");
    }
    mark_dscp(&sock, dscp);
    socket.set_nonblocking(true)?;
    UdpSocket::from_std(socket)
}

#[cfg(not(target_os = "linux"))]
fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "paqet is only supported on Linux",
    )
}
