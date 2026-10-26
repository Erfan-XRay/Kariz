//! What a running side really moves on the network: every byte its sockets send and
//! receive, with the IP, TCP and UDP headers and TCP's retransmissions, so the panel can
//! show the volume a server's provider counts (docs/status.md, `wire`).
//!
//! A process runs one side of one tunnel, so the counters belong to the process. TCP
//! sockets are read from the kernel (`TCP_INFO`: bytes sent with their retransmissions,
//! bytes received, segments each way) while they are open and once more just before they
//! close; UDP packets are counted as they are sent and received. Nothing runs in the
//! background: open TCP sockets are read when the status is asked for.
//!
//! Sockets over the loopback are counted apart (`local`): their bytes never reach a
//! network card.

// Elsewhere TCP sockets cannot be read, and that part goes unused.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

/// Which part of the path a socket is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leg {
    /// Between the users and the entry side.
    User,
    /// Between the two sides of the tunnel.
    Link,
    /// Between the exit side and the targets.
    Target,
}

/// Index of the loopback counters, after the three legs.
const LOCAL: usize = 3;
/// Header bytes of a packet below the transport: IPv4 and IPv6 without options.
const IPV4: u64 = 20;
const IPV6: u64 = 40;
const UDP: u64 = 8;
/// A TCP header, and the timestamps option Linux puts in every segment when both ends
/// agreed to it (10 bytes, padded to 12).
const TCP: u64 = 20;
const TCP_TIMESTAMPS: u64 = 12;

/// Where a packet goes: its leg (or the loopback) and the bytes of headers it carries
/// below the transport's payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Path {
    index: usize,
    ip: u64,
}

impl Path {
    pub fn new(leg: Leg, peer: SocketAddr) -> Self {
        let ip = peer.ip().to_canonical();
        Self {
            index: if ip.is_loopback() {
                LOCAL
            } else {
                leg as usize
            },
            ip: match ip {
                IpAddr::V4(_) => IPV4,
                IpAddr::V6(_) => IPV6,
            },
        }
    }

    /// Header bytes of one UDP packet on this path.
    fn udp(self) -> u64 {
        self.ip + UDP
    }
}

struct Dir {
    bytes: AtomicU64,
    packets: AtomicU64,
    headers: AtomicU64,
    resend: AtomicU64,
}

impl Dir {
    const fn new() -> Self {
        Self {
            bytes: AtomicU64::new(0),
            packets: AtomicU64::new(0),
            headers: AtomicU64::new(0),
            resend: AtomicU64::new(0),
        }
    }

    fn add(&self, bytes: u64, packets: u64, headers: u64) {
        self.bytes.fetch_add(bytes, Relaxed);
        self.packets.fetch_add(packets, Relaxed);
        self.headers.fetch_add(headers, Relaxed);
    }

    fn snapshot(&self) -> DirStatus {
        DirStatus {
            bytes: self.bytes.load(Relaxed),
            packets: self.packets.load(Relaxed),
            headers: self.headers.load(Relaxed),
            resend: self.resend.load(Relaxed),
        }
    }
}

struct Counters {
    sent: Dir,
    received: Dir,
}

impl Counters {
    const fn new() -> Self {
        Self {
            sent: Dir::new(),
            received: Dir::new(),
        }
    }

    fn snapshot(&self) -> LegStatus {
        LegStatus {
            sent: self.sent.snapshot(),
            received: self.received.snapshot(),
        }
    }
}

/// The counters of this process.
pub struct Meter {
    legs: [Counters; 4],
    tests: AtomicU64,
    /// Something was counted less exactly than it should be (no `TCP_INFO`, or an old
    /// kernel without the retransmitted bytes in it).
    inexact: AtomicBool,
    sockets: Mutex<Table>,
    /// One refresh at a time, so two readings of a socket are folded in order.
    refreshing: Mutex<()>,
}

static METER: Meter = Meter::new();

/// The process's counters.
pub fn meter() -> &'static Meter {
    &METER
}

impl Meter {
    const fn new() -> Self {
        Self {
            legs: [
                Counters::new(),
                Counters::new(),
                Counters::new(),
                Counters::new(),
            ],
            tests: AtomicU64::new(0),
            inexact: AtomicBool::new(false),
            sockets: Mutex::new(Table {
                entries: Vec::new(),
                free: Vec::new(),
            }),
            refreshing: Mutex::new(()),
        }
    }

    /// A UDP packet of `len` bytes (its payload) was sent on `path`.
    pub fn sent(&self, path: Path, len: usize) {
        self.sent_many(path, len as u64, 1);
    }

    /// `packets` UDP packets with `bytes` of payload together were sent on `path`.
    pub fn sent_many(&self, path: Path, bytes: u64, packets: u64) {
        let headers = packets * path.udp();
        self.legs[path.index]
            .sent
            .add(bytes + headers, packets, headers);
    }

    /// A UDP packet of `len` bytes was received on `path`.
    pub fn received(&self, path: Path, len: usize) {
        self.received_many(path, len as u64, 1);
    }

    pub fn received_many(&self, path: Path, bytes: u64, packets: u64) {
        let headers = packets * path.udp();
        self.legs[path.index]
            .received
            .add(bytes + headers, packets, headers);
    }

    /// `len` bytes of what was sent on `path` (and counted with [`Self::sent`]) were
    /// there again: a resend, or redundancy such as FEC parity.
    pub fn resent(&self, path: Path, len: usize) {
        self.legs[path.index]
            .sent
            .resend
            .fetch_add(len as u64, Relaxed);
    }

    /// `n` bytes of a speed test went through the tunnel (they are in the link's bytes).
    pub fn tested(&self, n: usize) {
        self.tests.fetch_add(n as u64, Relaxed);
    }

    /// Counts the TCP socket `stream` on `leg` (or the loopback) from when it was made
    /// until the guard and its clones are gone.
    pub fn track(&self, stream: &tokio::net::TcpStream, leg: Leg) -> TcpGuard {
        let Ok(peer) = stream.peer_addr() else {
            return TcpGuard::default();
        };
        let path = Path::new(leg, peer);
        match self.track_socket(stream, path) {
            Some(id) => TcpGuard {
                _slot: Some(Arc::new(Slot(id))),
            },
            None => {
                self.inexact.store(true, Relaxed);
                TcpGuard::default()
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn track_socket(&self, stream: &tokio::net::TcpStream, path: Path) -> Option<usize> {
        use std::os::fd::AsRawFd;
        let fd = stream.as_raw_fd();
        let tracked = Tracked {
            fd,
            ino: linux::inode(fd)?,
            path,
            // From zero: the counters cover the socket from its first segment, the
            // handshake too.
            last: TcpInfo::default(),
        };
        let mut table = self.table();
        Some(match table.free.pop() {
            Some(id) => {
                table.entries[id] = Some(tracked);
                id
            }
            None => {
                table.entries.push(Some(tracked));
                table.entries.len() - 1
            }
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn track_socket(&self, _: &tokio::net::TcpStream, _: Path) -> Option<usize> {
        None
    }

    fn table(&self) -> MutexGuard<'_, Table> {
        self.sockets.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Adds what the open TCP sockets did since they were last read. The sockets are read
    /// without holding the table, so thousands of them do not hold up new connections;
    /// a reading is used only if its socket is still in the table, unchanged.
    fn refresh(&self) {
        let _one = self.refreshing.lock().unwrap_or_else(|e| e.into_inner());
        let open: Vec<(usize, i32, u64)> = {
            let table = self.table();
            let entries = table.entries.iter().enumerate();
            entries
                .filter_map(|(id, t)| t.as_ref().map(|t| (id, t.fd, t.ino)))
                .collect()
        };
        let readings: Vec<(usize, u64, TcpInfo)> = open
            .into_iter()
            .filter_map(|(id, fd, ino)| Some((id, ino, read_info(fd, ino)?)))
            .collect();
        let mut table = self.table();
        for (id, ino, now) in readings {
            if let Some(Some(tracked)) = table.entries.get_mut(id) {
                if tracked.ino == ino {
                    self.apply(tracked, now);
                }
            }
        }
    }

    fn apply(&self, tracked: &mut Tracked, now: TcpInfo) {
        if !now.exact {
            self.inexact.store(true, Relaxed);
        }
        self.fold(tracked.path, &tracked.last, &now);
        tracked.last = now;
    }

    /// Adds the difference between two readings of one socket.
    fn fold(&self, path: Path, before: &TcpInfo, now: &TcpInfo) {
        let header = path.ip + TCP + if now.timestamps { TCP_TIMESTAMPS } else { 0 };
        let counters = &self.legs[path.index];
        let segs_out = u64::from(now.segs_out.wrapping_sub(before.segs_out));
        let segs_in = u64::from(now.segs_in.wrapping_sub(before.segs_in));
        let sent = now.sent.saturating_sub(before.sent);
        let received = now.received.saturating_sub(before.received);
        counters
            .sent
            .add(sent + segs_out * header, segs_out, segs_out * header);
        counters
            .sent
            .resend
            .fetch_add(now.retrans.saturating_sub(before.retrans), Relaxed);
        counters
            .received
            .add(received + segs_in * header, segs_in, segs_in * header);
    }

    /// The last reading of socket `id`, which then stops being counted.
    fn untrack(&self, id: usize) {
        let mut table = self.table();
        if let Some(mut tracked) = table.entries.get_mut(id).and_then(Option::take) {
            if let Some(now) = read_info(tracked.fd, tracked.ino) {
                self.apply(&mut tracked, now);
            }
            table.free.push(id);
        }
    }

    /// The `wire` part of the status document.
    pub fn snapshot(&self) -> WireStatus {
        self.refresh();
        WireStatus {
            exact: cfg!(target_os = "linux") && !self.inexact.load(Relaxed),
            user: self.legs[Leg::User as usize].snapshot(),
            link: self.legs[Leg::Link as usize].snapshot(),
            target: self.legs[Leg::Target as usize].snapshot(),
            local: self.legs[LOCAL].snapshot(),
            tests: self.tests.load(Relaxed),
        }
    }
}

struct Table {
    entries: Vec<Option<Tracked>>,
    free: Vec<usize>,
}

/// An open TCP socket being counted.
struct Tracked {
    fd: i32,
    /// The socket's inode: an fd closed and reused for another socket is not read.
    ino: u64,
    path: Path,
    last: TcpInfo,
}

/// Reads the socket `fd`, if it is still the one with inode `ino` (checked before and
/// after, so a reused fd is never read as this socket).
#[cfg(target_os = "linux")]
fn read_info(fd: i32, ino: u64) -> Option<TcpInfo> {
    if linux::inode(fd)? != ino {
        return None;
    }
    let info = linux::tcp_info(fd)?;
    (linux::inode(fd)? == ino).then_some(info)
}

#[cfg(not(target_os = "linux"))]
fn read_info(_: i32, _: u64) -> Option<TcpInfo> {
    None
}

/// Keeps a TCP socket counted. Clones share it; the last one to go reads the socket a
/// final time, so drop it before the socket closes (keep it in a field before the
/// socket's, or a local made after it). Read after the socket closed, the last moments
/// are lost, never someone else's counted.
#[derive(Clone, Default)]
pub struct TcpGuard {
    _slot: Option<Arc<Slot>>,
}

struct Slot(usize);

impl Drop for Slot {
    fn drop(&mut self) {
        METER.untrack(self.0);
    }
}

/// What the kernel says about one TCP socket, the parts counted here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct TcpInfo {
    /// Bytes sent, retransmissions included.
    sent: u64,
    retrans: u64,
    received: u64,
    segs_out: u32,
    segs_in: u32,
    timestamps: bool,
    /// The kernel gave the retransmitted bytes (Linux 4.19); without them `sent` holds
    /// the acknowledged bytes alone.
    exact: bool,
}

// Offsets in `struct tcp_info` (linux/tcp.h).
const OPTIONS: usize = 5;
const BYTES_ACKED: usize = 120;
const BYTES_RECEIVED: usize = 128;
const SEGS_OUT: usize = 136;
const SEGS_IN: usize = 140;
const BYTES_SENT: usize = 200;
const BYTES_RETRANS: usize = 208;
/// Size up to the last field read.
const TCP_INFO_LEN: usize = BYTES_RETRANS + 8;
const TCPI_OPT_TIMESTAMPS: u8 = 1;

impl TcpInfo {
    /// Reads the fields from what `getsockopt(TCP_INFO)` wrote; `None` from a kernel too
    /// old to count segments (before 4.2).
    fn parse(b: &[u8]) -> Option<Self> {
        let u32_at = |at: usize| Some(u32::from_ne_bytes(b.get(at..at + 4)?.try_into().ok()?));
        let u64_at = |at: usize| Some(u64::from_ne_bytes(b.get(at..at + 8)?.try_into().ok()?));
        let (sent, retrans, exact) = if b.len() >= TCP_INFO_LEN {
            (u64_at(BYTES_SENT)?, u64_at(BYTES_RETRANS)?, true)
        } else {
            (u64_at(BYTES_ACKED)?, 0, false)
        };
        Some(Self {
            sent,
            retrans,
            received: u64_at(BYTES_RECEIVED)?,
            segs_out: u32_at(SEGS_OUT)?,
            segs_in: u32_at(SEGS_IN)?,
            timestamps: b.get(OPTIONS)? & TCPI_OPT_TIMESTAMPS != 0,
            exact,
        })
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{TcpInfo, TCP_INFO_LEN};

    /// The inode of the socket `fd` names.
    // `ino_t` is 32 bits on some targets.
    #[allow(clippy::useless_conversion)]
    pub fn inode(fd: i32) -> Option<u64> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::zeroed();
        // SAFETY: fstat writes a `stat` into the buffer and reads nothing else.
        let r = unsafe { libc::fstat(fd, stat.as_mut_ptr()) };
        // SAFETY: zeroed, and filled in when it succeeded.
        (r == 0).then(|| u64::from(unsafe { stat.assume_init() }.st_ino))
    }

    pub fn tcp_info(fd: i32) -> Option<TcpInfo> {
        let mut buf = [0u8; TCP_INFO_LEN];
        let mut len = buf.len() as libc::socklen_t;
        // SAFETY: the kernel writes at most `len` bytes into `buf` and sets `len`.
        let r = unsafe {
            libc::getsockopt(
                fd,
                libc::IPPROTO_TCP,
                libc::TCP_INFO,
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        if r != 0 {
            return None;
        }
        TcpInfo::parse(&buf[..(len as usize).min(buf.len())])
    }
}

/// The `wire` part of the status document: what this side's sockets moved, by leg. Every
/// count is from when the side started.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WireStatus {
    /// TCP was counted from the kernel's own counters (Linux 4.19 or newer). False: some
    /// counts are short (the retransmissions, or a socket that could not be read).
    pub exact: bool,
    pub user: LegStatus,
    pub link: LegStatus,
    pub target: LegStatus,
    /// Over the loopback (a target or user on this server): on no network card.
    pub local: LegStatus,
    /// Bytes of speed tests, which are part of the link's.
    pub tests: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LegStatus {
    pub sent: DirStatus,
    pub received: DirStatus,
}

/// One direction of one leg. `bytes` is everything at the IP level: `headers` (IP and
/// TCP or UDP), `resend` (sent again: TCP retransmissions, KCP resends, FEC parity) and
/// the payload the rest is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirStatus {
    pub bytes: u64,
    pub packets: u64,
    pub headers: u64,
    pub resend: u64,
}

impl DirStatus {
    /// What the transport carried: neither headers nor resends.
    pub fn payload(&self) -> u64 {
        self.bytes.saturating_sub(self.headers + self.resend)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info_bytes(len: usize) -> Vec<u8> {
        let mut b = vec![0u8; len];
        b[OPTIONS] = TCPI_OPT_TIMESTAMPS;
        let mut put = |at: usize, v: u64, n: usize| {
            if let Some(field) = b.get_mut(at..at + n) {
                field.copy_from_slice(&v.to_ne_bytes()[..n]);
            }
        };
        put(BYTES_ACKED, 900, 8);
        put(BYTES_RECEIVED, 5000, 8);
        put(SEGS_OUT, 7, 4);
        put(SEGS_IN, 9, 4);
        if len >= TCP_INFO_LEN {
            put(BYTES_SENT, 1000, 8);
            put(BYTES_RETRANS, 100, 8);
        }
        b
    }

    #[test]
    fn tcp_info_is_read_at_the_kernel_offsets() {
        let info = TcpInfo::parse(&info_bytes(TCP_INFO_LEN + 16)).unwrap();
        assert_eq!(
            info,
            TcpInfo {
                sent: 1000,
                retrans: 100,
                received: 5000,
                segs_out: 7,
                segs_in: 9,
                timestamps: true,
                exact: true,
            }
        );
    }

    #[test]
    fn an_older_kernel_gives_the_acked_bytes() {
        let info = TcpInfo::parse(&info_bytes(160)).unwrap();
        assert_eq!((info.sent, info.retrans, info.exact), (900, 0, false));
        // Before segments were counted: nothing usable.
        assert_eq!(TcpInfo::parse(&info_bytes(104)), None);
    }

    #[test]
    fn segments_carry_their_headers() {
        let meter = Meter::new();
        let path = Path::new(Leg::Link, "192.0.2.1:443".parse().unwrap());
        let now = TcpInfo {
            sent: 10_000,
            retrans: 1_000,
            received: 2_000,
            segs_out: 10,
            segs_in: 4,
            timestamps: true,
            exact: true,
        };
        meter.fold(path, &TcpInfo::default(), &now);
        let link = meter.snapshot().link;
        // 20 (IPv4) + 20 (TCP) + 12 (timestamps) a segment.
        assert_eq!(
            link.sent,
            DirStatus {
                bytes: 10_000 + 520,
                packets: 10,
                headers: 520,
                resend: 1_000
            }
        );
        assert_eq!(link.sent.payload(), 9_000);
        assert_eq!((link.received.bytes, link.received.headers), (2_208, 208));
        // A second reading adds only what changed; the segment count may wrap.
        let later = TcpInfo {
            segs_out: 1,
            sent: 10_100,
            ..now
        };
        let before = TcpInfo {
            segs_out: u32::MAX,
            ..now
        };
        meter.fold(path, &before, &later);
        let link = meter.snapshot().link;
        assert_eq!(
            (link.sent.packets, link.sent.bytes),
            (12, 10_520 + 100 + 104)
        );
    }

    #[test]
    fn udp_packets_and_the_loopback() {
        let meter = Meter::new();
        let v6 = Path::new(Leg::User, "[2001:db8::1]:53".parse().unwrap());
        meter.sent(v6, 100);
        meter.received_many(v6, 1000, 4);
        let local = Path::new(Leg::Target, "127.0.0.1:8080".parse().unwrap());
        let mapped = Path::new(Leg::Target, "[::ffff:127.0.0.1]:8080".parse().unwrap());
        assert_eq!(local, mapped);
        meter.sent(local, 10);
        let s = meter.snapshot();
        assert_eq!((s.user.sent.bytes, s.user.sent.headers), (148, 48));
        assert_eq!((s.user.received.bytes, s.user.received.packets), (1192, 4));
        assert_eq!(s.target, LegStatus::default());
        assert_eq!(s.local.sent.bytes, 38);
    }

    #[test]
    fn resends_are_part_of_the_bytes_sent() {
        let meter = Meter::new();
        let path = Path::new(Leg::Link, "192.0.2.1:4000".parse().unwrap());
        meter.sent(path, 1200);
        meter.resent(path, 1200);
        let sent = meter.snapshot().link.sent;
        assert_eq!((sent.bytes, sent.resend, sent.payload()), (1228, 1200, 0));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_tcp_socket_is_read_from_the_kernel() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (dialed, accepted) = tokio::join!(tokio::net::TcpStream::connect(addr), async {
            listener.accept().await.unwrap().0
        });
        let (mut dialed, mut accepted) = (dialed.unwrap(), accepted);
        let before = METER.snapshot().local;
        let guard = METER.track(&dialed, Leg::Link);
        dialed.write_all(&[7u8; 100_000]).await.unwrap();
        let mut got = vec![0u8; 100_000];
        accepted.read_exact(&mut got).await.unwrap();
        accepted.write_all(b"done").await.unwrap();
        dialed.read_exact(&mut [0u8; 4]).await.unwrap();
        drop(guard);
        let after = METER.snapshot().local;
        let sent = after.sent.bytes - before.sent.bytes;
        let headers = after.sent.headers - before.sent.headers;
        // Other tests may count loopback sockets meanwhile, so at least.
        assert!(sent - headers >= 100_000, "{sent} {headers}");
        assert!(headers >= 52 * 3, "{headers}");
        assert!(after.received.bytes - before.received.bytes >= 4);
    }
}
