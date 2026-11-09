//! Linux IPv4 raw socket carrier: a nonblocking `IPPROTO_TCP` socket whose TCP header
//! and checksum Paqet writes itself, and whose IPv4 header the kernel writes and reads.
//!
//! A classic BPF program attached to the socket drops every TCP packet for another local
//! port before it reaches userspace. The socket's receive side is driven by Tokio's
//! `AsyncFd`; sends are direct nonblocking `sendto` calls, so a full socket buffer drops
//! a packet instead of blocking a KCP timer or a writer. The dialing side's socket is
//! connected to its peer, so the kernel hands it only that peer's packets (and does not
//! copy every other TCP packet of the host to it first).
//!
//! Each flow (peer address and port) keeps TCP numbers of its own, as a real connection
//! would: the sequence number grows by what this side sent, the acknowledgment follows
//! what the peer sent. A stateful firewall on the path checks both against a window per
//! flow; one counter shared by several mux connections, or a fixed acknowledgment,
//! leaves that window and gets the segments dropped as invalid.
//!
//! A listener may be on every IPv4 address (`0.0.0.0`), like the other transports. Each
//! flow then remembers the address its packets came to and is answered from it
//! (`IP_PKTINFO` on the send), so a server with several addresses answers on the one
//! that was dialed and the TCP checksum covers the right one.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::os::fd::{AsRawFd, RawFd};
use std::sync::Mutex;

use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use tokio::io::unix::AsyncFd;
use tokio::task::yield_now;
use tracing::debug;

use super::{firewall, packet};

/// Invalid packets handled before yielding to the async runtime, so a flood of junk
/// cannot keep one driver task running forever.
const INVALID_BEFORE_YIELD: u32 = 32;
const IPPROTO_TCP: u8 = 6;
/// What the BPF filter returns for a packet it accepts: the bytes of it to keep. Any
/// smaller cap would quietly cut a packet longer than it, which then fails
/// authentication.
const ACCEPT_WHOLE: u32 = u16::MAX as u32;
/// Flows a socket keeps TCP numbers for. Segments are counted before they are
/// authenticated, so a flood of forged sources must not grow the table without end;
/// past this, flows are forgotten and start again from new numbers.
const MAX_FLOWS: usize = 4096;

/// The TCP numbers of one flow.
#[derive(Debug, Clone, Copy)]
struct Flow {
    /// Sequence number of this side's next segment.
    sequence: u32,
    /// What this side acknowledges: the end of the peer's latest segment.
    acknowledgment: u32,
    /// This side's address in the flow: where the peer's packets arrive, and the source
    /// of the replies.
    local: Ipv4Addr,
}

/// A raw IPv4 socket bound to one local address (or every one, on the listening side)
/// and TCP port.
pub(crate) struct RawSocket {
    fd: AsyncFd<Socket>,
    local: SocketAddrV4,
    /// Set on the dialing side: sends go here unless a peer is passed explicitly.
    peer: Option<SocketAddrV4>,
    flows: Mutex<HashMap<SocketAddrV4, Flow>>,
    /// A normal TCP socket bound to the same port but never listening. It reserves the
    /// port for this process; it does not suppress the kernel's RST (`_firewall` does).
    _reserved: Socket,
    /// The host firewall rules this socket needs (see [`firewall`]); removed with it.
    _firewall: firewall::Rules,
}

impl RawSocket {
    /// Listens on one local IPv4 address, or on every one (`0.0.0.0`); `port` 0 picks a
    /// free TCP port.
    pub(crate) fn listen(local: SocketAddr, buffer: usize, dscp: Option<u8>) -> io::Result<Self> {
        let SocketAddr::V4(local) = local else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "paqet supports IPv4 only in this phase",
            ));
        };
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
        set_buffers(&raw, buffer);
        if let Some(dscp) = dscp {
            if let Err(e) = raw.set_tos_v4(u32::from(dscp) << 2) {
                debug!(error = %e, dscp, "could not set the paqet DSCP mark");
            }
        }
        raw.bind(&SockAddr::from(SocketAddr::from((ip, 0))))?;
        if let Some(peer) = peer {
            // The kernel then delivers only the peer's packets to this socket.
            raw.connect(&SockAddr::from(SocketAddr::from((*peer.ip(), 0))))?;
        }
        attach_port_filter(&raw, local.port(), peer.map(|p| *p.ip()))?;
        let fd = AsyncFd::new(raw)?;
        let firewall = match peer {
            Some(peer) => firewall::Rules::dialer(ip, peer),
            None => firewall::Rules::listener(local),
        };

        Ok(Self {
            fd,
            local,
            peer,
            flows: Mutex::new(HashMap::new()),
            _reserved: reserved,
            _firewall: firewall,
        })
    }

    fn flows(&self) -> std::sync::MutexGuard<'_, HashMap<SocketAddrV4, Flow>> {
        self.flows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether this is a listener on every address, which answers each flow from the
    /// address its packets came to.
    fn on_every_address(&self) -> bool {
        self.local.ip().is_unspecified()
    }

    /// Records that `peer` sent a segment ending at `end` to `local`: the acknowledgment
    /// and the source address of the next reply. A segment that arrived late (ending
    /// before the latest one) does not move the acknowledgment back.
    fn heard(&self, peer: SocketAddrV4, end: u32, local: Ipv4Addr) {
        let mut flows = self.flows();
        if let Some(flow) = flows.get_mut(&peer) {
            if later(end, flow.acknowledgment) {
                flow.acknowledgment = end;
            }
            flow.local = local;
        } else if flows.len() < MAX_FLOWS {
            flows.insert(
                peer,
                Flow {
                    sequence: random_u32(),
                    acknowledgment: end,
                    local,
                },
            );
        }
    }

    /// The sequence and acknowledgment numbers of a segment of `length` bytes to `peer`
    /// and the address it goes from, and the flow's sequence number moved past it.
    fn numbers(&self, peer: SocketAddrV4, length: usize) -> (u32, u32, Ipv4Addr) {
        let mut flows = self.flows();
        if flows.len() >= MAX_FLOWS && !flows.contains_key(&peer) {
            flows.clear();
        }
        let flow = flows.entry(peer).or_insert_with(|| Flow {
            sequence: random_u32(),
            acknowledgment: random_u32(),
            // A peer not heard from yet (only after the table was cleared): the address
            // the route to it leaves from.
            local: if self.on_every_address() {
                route_local_ip(peer).unwrap_or(Ipv4Addr::UNSPECIFIED)
            } else {
                *self.local.ip()
            },
        });
        let sequence = flow.sequence;
        flow.sequence = sequence.wrapping_add(length as u32);
        (sequence, flow.acknowledgment, flow.local)
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
            let Some(Payload {
                start,
                length,
                peer,
                sequence,
                local,
            }) = parse_payload(&buf[..length], *self.local.ip(), self.local.port())
                .filter(|p| self.peer.map_or(true, |remote| remote == p.peer))
            else {
                invalid += 1;
                if invalid % INVALID_BEFORE_YIELD == 0 {
                    yield_now().await;
                }
                continue;
            };
            self.heard(peer, sequence.wrapping_add(length as u32), local);
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
        let (sequence, acknowledgment, from) = self.numbers(peer, payload.len());
        let source = SocketAddrV4::new(from, self.local.port());
        packet::build_tcp_segment(source, peer, sequence, acknowledgment, payload, scratch)?;
        let address = libc::sockaddr_in {
            sin_family: libc::AF_INET as libc::sa_family_t,
            sin_port: 0,
            sin_addr: libc::in_addr {
                s_addr: u32::from_ne_bytes(peer.ip().octets()),
            },
            sin_zero: [0; 8],
        };
        let fd = self.fd.get_ref().as_raw_fd();
        let sent = loop {
            let sent = if self.on_every_address() && !from.is_unspecified() {
                send_from(fd, scratch, &address, from)
            } else {
                unsafe {
                    libc::sendto(
                        fd,
                        scratch.as_ptr().cast(),
                        scratch.len(),
                        0,
                        (&address as *const libc::sockaddr_in).cast(),
                        std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
                    )
                }
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

/// `sendmsg` of `data` to `to` with `from` as the IPv4 source (`IP_PKTINFO`'s
/// `ipi_spec_dst`, which a raw socket's send honours): a listener on every address
/// answers from the address the flow came to, not the one the route would pick. The
/// return value is `sendmsg`'s.
fn send_from(fd: RawFd, data: &[u8], to: &libc::sockaddr_in, from: Ipv4Addr) -> isize {
    let info_len = std::mem::size_of::<libc::in_pktinfo>() as libc::c_uint;
    // Room for one control message, aligned for `cmsghdr`.
    let mut control = [0u64; 8];
    let space = unsafe { libc::CMSG_SPACE(info_len) } as usize;
    debug_assert!(space <= std::mem::size_of_val(&control));
    let mut iov = libc::iovec {
        iov_base: data.as_ptr().cast_mut().cast(),
        iov_len: data.len(),
    };
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_name = (to as *const libc::sockaddr_in).cast_mut().cast();
    msg.msg_namelen = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    let info = libc::in_pktinfo {
        ipi_ifindex: 0,
        ipi_spec_dst: libc::in_addr {
            s_addr: u32::from_ne_bytes(from.octets()),
        },
        ipi_addr: libc::in_addr { s_addr: 0 },
    };
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&msg);
        (*header).cmsg_level = libc::IPPROTO_IP;
        (*header).cmsg_type = libc::IP_PKTINFO;
        (*header).cmsg_len = libc::CMSG_LEN(info_len) as _;
        std::ptr::write_unaligned(libc::CMSG_DATA(header).cast::<libc::in_pktinfo>(), info);
        libc::sendmsg(fd, &msg, 0)
    }
}

/// Whether TCP number `a` comes after `b` (modulo 2^32, as TCP compares them).
fn later(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

fn random_u32() -> u32 {
    crate::crypto::random_bytes::<4>().map_or(0x5eed_1e55, u32::from_le_bytes)
}

/// Sets the socket buffers to `buffer` bytes. The plain options are capped at
/// `net.core.rmem_max` / `wmem_max` (often about 200 KB: a few dozen raw packets, so a
/// burst is lost and KCP resends it); the tunnel service runs as root, which may set the
/// size past the cap with the `FORCE` options.
fn set_buffers(socket: &Socket, buffer: usize) {
    let size = libc::c_int::try_from(buffer).unwrap_or(libc::c_int::MAX);
    for (force, plain, name) in [
        (libc::SO_RCVBUFFORCE, libc::SO_RCVBUF, "receive"),
        (libc::SO_SNDBUFFORCE, libc::SO_SNDBUF, "send"),
    ] {
        if setsockopt_int(socket, force, size).is_ok() {
            continue;
        }
        if let Err(e) = setsockopt_int(socket, plain, size) {
            debug!(error = %e, buffer = name, "could not set a paqet raw socket buffer");
        }
    }
}

fn setsockopt_int(socket: &Socket, option: libc::c_int, value: libc::c_int) -> io::Result<()> {
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            option,
            (&value as *const libc::c_int).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
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

/// Where a received packet's payload is in the buffer, and what the flow needs of it.
struct Payload {
    start: usize,
    length: usize,
    peer: SocketAddrV4,
    sequence: u32,
    /// The address it was sent to.
    local: Ipv4Addr,
}

/// The payload of a received IPv4 packet, if it is usable.
fn parse_payload(packet: &[u8], local_ip: Ipv4Addr, local_port: u16) -> Option<Payload> {
    let incoming = packet::parse_ipv4_tcp(packet, local_ip, local_port)?;
    Some(Payload {
        start: incoming.payload.as_ptr() as usize - packet.as_ptr() as usize,
        length: incoming.payload.len(),
        peer: incoming.peer,
        sequence: incoming.sequence,
        local: incoming.local,
    })
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
            instruction(libc::BPF_RET | libc::BPF_K, 0, 0, ACCEPT_WHOLE),
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
            instruction(libc::BPF_RET | libc::BPF_K, 0, 0, ACCEPT_WHOLE),
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
