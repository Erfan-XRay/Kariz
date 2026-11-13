//! Sockets for the ports users connect to.
//!
//! A forward on the IPv4 wildcard (`0.0.0.0:P`) is reachable over IPv6 too: one `[::]:P`
//! socket takes both families. Hosts without IPv6 get the plain IPv4 socket, and any other
//! address is bound as written.

use std::io;
use std::net::{Ipv6Addr, SocketAddr};

use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use tokio::net::{TcpListener, UdpSocket};

/// The dual-stack address for `listen`, when it is an IPv4 wildcard like `0.0.0.0:4627`.
fn dual_stack(listen: &str) -> Option<SocketAddr> {
    let addr: SocketAddr = listen.parse().ok()?;
    match addr {
        SocketAddr::V4(v4) if v4.ip().is_unspecified() => {
            Some((Ipv6Addr::UNSPECIFIED, v4.port()).into())
        }
        _ => None,
    }
}

/// One non-blocking socket on `addr` taking IPv4 as well (a `[::]` socket).
fn open(addr: SocketAddr, ty: Type, protocol: Protocol) -> io::Result<Socket> {
    let socket = Socket::new(Domain::IPV6, ty, Some(protocol))?;
    socket.set_only_v6(false)?;
    socket.set_nonblocking(true)?;
    #[cfg(unix)]
    if ty == Type::STREAM {
        socket.set_reuse_address(true)?;
    }
    socket.bind(&SockAddr::from(addr))?;
    Ok(socket)
}

/// Listens like [`TcpListener::bind`], on IPv6 as well for `0.0.0.0:P`.
pub async fn tcp(listen: &str) -> io::Result<TcpListener> {
    if let Some(addr) = dual_stack(listen) {
        let dual = open(addr, Type::STREAM, Protocol::TCP).and_then(|socket| {
            socket.listen(1024)?;
            TcpListener::from_std(socket.into())
        });
        if let Ok(listener) = dual {
            return Ok(listener);
        }
    }
    TcpListener::bind(listen).await
}

/// Binds like [`UdpSocket::bind`], on IPv6 as well for `0.0.0.0:P`. IPv4 peers show up as
/// IPv4-mapped addresses (`::ffff:a.b.c.d`) there.
pub async fn udp(listen: &str) -> io::Result<UdpSocket> {
    if let Some(addr) = dual_stack(listen) {
        let dual =
            open(addr, Type::DGRAM, Protocol::UDP).and_then(|s| UdpSocket::from_std(s.into()));
        if let Ok(socket) = dual {
            return Ok(socket);
        }
    }
    UdpSocket::bind(listen).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_ipv6() -> bool {
        std::net::TcpListener::bind("[::1]:0").is_ok()
    }

    #[test]
    fn only_the_ipv4_wildcard_is_widened() {
        assert_eq!(
            dual_stack("0.0.0.0:4627"),
            Some("[::]:4627".parse().unwrap())
        );
        assert_eq!(dual_stack("127.0.0.1:4627"), None);
        assert_eq!(dual_stack("[::]:4627"), None);
        assert_eq!(dual_stack("[::1]:4627"), None);
        assert_eq!(dual_stack("localhost:4627"), None);
    }

    #[tokio::test]
    async fn tcp_wildcard_takes_both_families() {
        let listener = tcp("0.0.0.0:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let (_, peer) = listener.accept().await.unwrap();
        assert!(peer.ip().to_canonical().is_ipv4());
        if has_ipv6() {
            tokio::net::TcpStream::connect(("::1", port)).await.unwrap();
            listener.accept().await.unwrap();
        }
    }

    #[tokio::test]
    async fn udp_wildcard_takes_both_families() {
        let socket = udp("0.0.0.0:0").await.unwrap();
        let port = socket.local_addr().unwrap().port();
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client.send_to(b"a", ("127.0.0.1", port)).await.unwrap();
        let mut buf = [0u8; 8];
        let (n, peer) = socket.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"a");
        // The reply goes back to the peer as the socket reported it.
        socket.send_to(b"b", peer).await.unwrap();
        let (n, _) = client.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"b");
        if has_ipv6() {
            let client = UdpSocket::bind("[::1]:0").await.unwrap();
            client.send_to(b"c", ("::1", port)).await.unwrap();
            let (_, peer) = socket.recv_from(&mut buf).await.unwrap();
            socket.send_to(b"d", peer).await.unwrap();
            let (n, _) = client.recv_from(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"d");
        }
    }
}
