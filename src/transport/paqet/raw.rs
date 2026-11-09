//! Linux IPv4 raw socket carrier: a nonblocking `IPPROTO_TCP` socket whose TCP header
//! and checksum Paqet writes itself, and whose IPv4 header the kernel writes and reads.
//!
//! A classic BPF program attached to the socket drops every TCP packet for another local
//! port before it reaches userspace. The socket's receive side is driven by Tokio's
//! `AsyncFd`; sends are direct nonblocking `sendto` calls, so a full socket buffer drops
//! a packet instead of blocking a KCP timer or a writer.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicU32, Ordering};

use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use tokio::io::unix::AsyncFd;
use tokio::task::yield_now;
use tracing::debug;

use super::packet;

/// Invalid packets handled before yielding to the async runtime, so a flood of junk
/// cannot keep one driver task running forever.
const INVALID_BEFORE_YIELD: u32 = 32;
const IPPROTO_TCP: u8 = 6;

/// A raw IPv4 socket bound to one local address and TCP port.
pub(crate) struct RawSocket {
    fd: AsyncFd<Socket>,
    local: SocketAddrV4,
    /// Set on the dialing side: sends go here unless a peer is passed explicitly.
    peer: Option<SocketAddrV4>,
    /// TCP sequence number of the next segment.
    sequence: AtomicU32,
    acknowledgment: u32,
    /// A normal TCP socket bound to the same port but never listening. It reserves the
    /// port for this process; it does not suppress the kernel's RST (the host firewall
    /// must, see `docs/paqet.md`).
    _reserved: Socket,
}

impl RawSocket {
    /// Listens on a concrete local IPv4 address (`port` 0 picks a free TCP port).
    pub(crate) fn listen(local: SocketAddr, buffer: usize, dscp: Option<u8>) -> io::Result<Self> {
        let SocketAddr::V4(local) = local else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "paqet supports IPv4 only in this phase",
            ));
        };
        if local.ip().is_unspecified() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "paqet needs a concrete IPv4 listen address, not 0.0.0.0: a wildcard \
                 socket could send replies from the wrong address",
            ));
        }
        Self::bind(*local.ip(), local.port(), None, buffer, dscp)
    }

    /// Dials `peer` from the address the route to it selects.
    pub(crate) fn dial(peer: SocketAddr, buffer: usize, dscp: Option<u8>) -> io::Result<Self> {
        let SocketAddr::V4(peer) = peer else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "paqet supports IPv4 only in this phase",
            ));
        };
        if peer.port() == 0
            || peer.ip().is_unspecified()
            || peer.ip().is_multicast()
            || peer.ip().is_broadcast()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("paqet cannot dial {peer}"),
            ));
        }
        let local_ip = route_local_ip(peer)?;
        Self::bind(local_ip, 0, Some(peer), buffer, dscp)
    }

    fn bind(
        ip: Ipv4Addr,
        port: u16,
        peer: Option<SocketAddrV4>,
        buffer: usize,
        dscp: Option<u8>,
    ) -> io::Result<Self> {
        // Reserve the TCP port with an ordinary socket. Raw sends put this source port in
        // the TCP header, so another process must not listen on it.
        let reserved = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
        reserved
            .bind(&SockAddr::from(SocketAddr::from((ip, port))))
            .map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!("paqet could not reserve TCP port {port} on {ip}: {e}"),
                )
            })?;
        let Some(SocketAddr::V4(local)) = reserved.local_addr()?.as_socket() else {
            return Err(io::Error::other("paqet reserved a non-IPv4 address"));
        };

        let raw = raw_socket().map_err(|e| {
            if e.kind() == io::ErrorKind::PermissionDenied {
                io::Error::new(
                    e.kind(),
                    format!("paqet needs root or CAP_NET_RAW to open a raw TCP socket: {e}"),
                )
            } else {
                e
            }
        })?;
        raw.set_nonblocking(true)?;
        if let Err(e) = raw
            .set_recv_buffer_size(buffer)
            .and_then(|_| raw.set_send_buffer_size(buffer))
        {
            debug!(error = %e, "could not set paqet raw socket buffers");
        }
        if let Some(dscp) = dscp {
            if let Err(e) = raw.set_tos_v4(u32::from(dscp) << 2) {
                debug!(error = %e, dscp, "could not set the paqet DSCP mark");
            }
        }
        raw.bind(&SockAddr::from(SocketAddr::from((ip, 0))))?;
        attach_port_filter(&raw, local.port(), peer.map(|p| *p.ip()))?;
        let fd = AsyncFd::new(raw)?;

        let [s0, s1, s2, s3, a0, a1, a2, a3] = crate::crypto::random_bytes::<8>()?;
        let sequence = u32::from_le_bytes([s0, s1, s2, s3]);
        let acknowledgment = u32::from_le_bytes([a0, a1, a2, a3]);

        Ok(Self {
            fd,
            local,
            peer,
            sequence: AtomicU32::new(sequence),
            acknowledgment,
            _reserved: reserved,
        })
    }

    pub(crate) fn local_addr(&self) -> SocketAddr {
        SocketAddr::V4(self.local)
    }

    /// Reads one valid TCP payload. Invalid packets are skipped; a sustained flood yields
    /// to the runtime every [`INVALID_BEFORE_YIELD`] of them.
    pub(crate) async fn recv(&self, buf: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        let mut invalid = 0u32;
        loop {
            let mut ready = self.fd.readable().await?;
            let received = ready.try_io(|fd| recv_once(fd.get_ref(), buf));
            let length = match received {
                Ok(result) => result?,
                Err(_) => continue,
            };
            let Some((start, length, peer)) =
                parse_payload(&buf[..length], *self.local.ip(), self.local.port())
                    .filter(|&(_, _, peer)| self.peer.map_or(true, |remote| remote == peer))
            else {
                invalid += 1;
                if invalid % INVALID_BEFORE_YIELD == 0 {
                    yield_now().await;
                }
                continue;
            };
            buf.copy_within(start..start + length, 0);
            return Ok((length, SocketAddr::V4(peer)));
        }
    }

    /// Sends to the configured peer without waiting.
    pub(crate) fn try_send(&self, payload: &[u8], scratch: &mut Vec<u8>) -> io::Result<usize> {
        let peer = self.peer.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "a paqet listener has no default peer",
            )
        })?;
        self.try_send_to_addr(payload, peer, scratch)
    }

    /// Sends to `peer` without waiting.
    pub(crate) fn try_send_to(
        &self,
        payload: &[u8],
        peer: SocketAddr,
        scratch: &mut Vec<u8>,
    ) -> io::Result<usize> {
        let SocketAddr::V4(peer) = peer else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "paqet supports IPv4 only in this phase",
            ));
        };
        self.try_send_to_addr(payload, peer, scratch)
    }

    fn try_send_to_addr(
        &self,
        payload: &[u8],
        peer: SocketAddrV4,
        scratch: &mut Vec<u8>,
    ) -> io::Result<usize> {
        let sequence = self
            .sequence
            .fetch_add(payload.len() as u32, Ordering::Relaxed);
        packet::build_tcp_segment(
            self.local,
            peer,
            sequence,
            self.acknowledgment,
            payload,
            scratch,
        )?;
        let address = libc::sockaddr_in {
            sin_family: libc::AF_INET as libc::sa_family_t,
            sin_port: 0,
            sin_addr: libc::in_addr {
                s_addr: u32::from_ne_bytes(peer.ip().octets()),
            },
            sin_zero: [0; 8],
        };
        let sent = loop {
            let sent = unsafe {
                libc::sendto(
                    self.fd.get_ref().as_raw_fd(),
                    scratch.as_ptr().cast(),
                    scratch.len(),
                    0,
                    (&address as *const libc::sockaddr_in).cast(),
                    std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
                )
            };
            if sent >= 0 {
                break sent as usize;
            }
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        };
        if sent != scratch.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "paqet raw socket accepted only part of a segment",
            ));
        }
        Ok(payload.len())
    }
}

fn raw_socket() -> io::Result<Socket> {
    Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::TCP))
}

/// The address the kernel routes to `peer` from, learned without sending a packet.
fn route_local_ip(peer: SocketAddrV4) -> io::Result<Ipv4Addr> {
    let probe = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    probe.connect(SocketAddr::V4(peer))?;
    let local = probe.local_addr()?;
    let IpAddr::V4(ip) = local.ip() else {
        return Err(io::Error::other(
            "a UDP route probe chose a non-IPv4 address",
        ));
    };
    if ip.is_unspecified() {
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("no route to {peer}"),
        ));
    }
    Ok(ip)
}

fn recv_once(socket: &Socket, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        let received =
            unsafe { libc::recv(socket.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        if received >= 0 {
            return Ok(received as usize);
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(error);
    }
}

/// The payload's offset and length and its sender, if the IPv4 packet is usable.
fn parse_payload(
    packet: &[u8],
    local_ip: Ipv4Addr,
    local_port: u16,
) -> Option<(usize, usize, SocketAddrV4)> {
    let incoming = packet::parse_ipv4_tcp(packet, local_ip, local_port)?;
    let start = incoming.payload.as_ptr() as usize - packet.as_ptr() as usize;
    Some((start, incoming.payload.len(), incoming.peer))
}

/// Attaches a classic BPF filter for IPv4 TCP packets whose destination port is `port`.
/// With `peer`, the source address must be that peer too (the dialing side knows it).
///
/// ```text
/// ip[0] & 0xf0 == 0x40
/// x = 4 * (ip[0] & 0xf)
/// tcp[x + 2] == port
/// ip[9] == 6
/// ip[12..16] == peer (dialing side)
/// ```
///
/// Loads are bounds-checked by the kernel filter engine: a short packet returns 0.
fn attach_port_filter(socket: &Socket, port: u16, peer: Option<Ipv4Addr>) -> io::Result<()> {
    let port = u32::from(port);
    let filter = if let Some(peer) = peer {
        // 0..7: version, port and protocol; 8..9: source address; 10 drops, 11 accepts.
        vec![
            instruction(libc::BPF_LD | libc::BPF_B | libc::BPF_ABS, 0, 0, 0),
            instruction(libc::BPF_ALU | libc::BPF_AND | libc::BPF_K, 0, 0, 0xf0),
            instruction(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 0, 7, 0x40),
            instruction(libc::BPF_LDX | libc::BPF_B | libc::BPF_MSH, 0, 0, 0),
            instruction(libc::BPF_LD | libc::BPF_H | libc::BPF_IND, 0, 0, 2),
            instruction(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 0, 4, port),
            instruction(libc::BPF_LD | libc::BPF_B | libc::BPF_ABS, 0, 0, 9),
            instruction(
                libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
                0,
                2,
                u32::from(IPPROTO_TCP),
            ),
            instruction(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, 12),
            instruction(
                libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
                1,
                0,
                u32::from_be_bytes(peer.octets()),
            ),
            instruction(libc::BPF_RET | libc::BPF_K, 0, 0, 0),
            instruction(libc::BPF_RET | libc::BPF_K, 0, 0, 1500),
        ]
    } else {
        // 8 drops, 9 accepts.
        vec![
            instruction(libc::BPF_LD | libc::BPF_B | libc::BPF_ABS, 0, 0, 0),
            instruction(libc::BPF_ALU | libc::BPF_AND | libc::BPF_K, 0, 0, 0xf0),
            instruction(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 0, 5, 0x40),
            instruction(libc::BPF_LDX | libc::BPF_B | libc::BPF_MSH, 0, 0, 0),
            instruction(libc::BPF_LD | libc::BPF_H | libc::BPF_IND, 0, 0, 2),
            instruction(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, 0, 2, port),
            instruction(libc::BPF_LD | libc::BPF_B | libc::BPF_ABS, 0, 0, 9),
            instruction(
                libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
                1,
                0,
                u32::from(IPPROTO_TCP),
            ),
            instruction(libc::BPF_RET | libc::BPF_K, 0, 0, 0),
            instruction(libc::BPF_RET | libc::BPF_K, 0, 0, 1500),
        ]
    };
    let program = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_ptr().cast_mut(),
    };
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_ATTACH_FILTER,
            (&program as *const libc::sock_fprog).cast(),
            std::mem::size_of::<libc::sock_fprog>() as libc::socklen_t,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn instruction(code: u32, jt: u8, jf: u8, k: u32) -> libc::sock_filter {
    libc::sock_filter {
        code: code as u16,
        jt,
        jf,
        k,
    }
}
