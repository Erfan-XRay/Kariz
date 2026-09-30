//! Link emulators: a UDP and a TCP proxy between the tunnel's two sides that do to the
//! traffic what a long, lossy path does.
//!
//! Each direction of a link is a [`Path`]: a bottleneck with a rate and a bounded queue
//! (tail drop), random or bursty loss, delay, jitter and optional reordering. The UDP
//! link applies it to every datagram. A TCP proxy cannot lose bytes, so the TCP link
//! instead runs a model of the sender's TCP over the path: segments, Cubic's congestion
//! window, fast retransmission after three duplicate acks and in-order delivery. What
//! comes out is the byte stream a real TCP connection over that path would deliver, at
//! the rate and with the stalls it would have.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// What a link does to the packets of each direction. Both directions get the same
/// settings and their own random draws.
#[derive(Clone, Copy, Debug)]
pub struct Impairment {
    /// One-way delay.
    pub delay: Duration,
    /// Extra delay per packet, uniform in `0..=jitter`. Packets keep their order (jitter
    /// comes from queues on the way, which do not reorder); see `reorder`.
    pub jitter: Duration,
    /// Share of packets lost, `0.0..1.0`.
    pub loss: f64,
    /// Mean length of a loss burst in packets (Gilbert model); 1 = independent losses.
    pub burst: f64,
    /// Share of packets held back by `reorder_gap`, so the ones behind overtake them.
    pub reorder: f64,
    pub reorder_gap: Duration,
    /// Bottleneck rate in bits per second, counting IP and UDP / TCP headers; 0 means
    /// unlimited.
    pub rate: u64,
    /// Longest wait in the bottleneck queue; packets that would wait longer are dropped.
    pub queue: Duration,
    /// Seed for the random draws, so a run can be repeated.
    pub seed: u64,
}

impl Default for Impairment {
    fn default() -> Self {
        Self {
            delay: Duration::ZERO,
            jitter: Duration::ZERO,
            loss: 0.0,
            burst: 1.0,
            reorder: 0.0,
            reorder_gap: Duration::ZERO,
            rate: 0,
            queue: Duration::from_millis(50),
            seed: 0x006b_6172_697a,
        }
    }
}

/// Windows wakes sleeping threads on a 15.6 ms tick unless a process asks for a finer
/// one, which would add up to that much to every emulated delay. Lasts for the process.
pub fn fine_timers() {
    #[cfg(windows)]
    {
        #[link(name = "winmm")]
        extern "system" {
            fn timeBeginPeriod(period_ms: u32) -> u32;
        }
        // SAFETY: no pointers or state involved; the call only sets the tick period.
        unsafe {
            timeBeginPeriod(1);
        }
    }
}

/// Packet counts of a link, both directions together. For the TCP link these are
/// segments, `retransmitted` included.
#[derive(Default, Debug)]
pub struct Stats {
    passed: AtomicU64,
    lost: AtomicU64,
    dropped: AtomicU64,
    retransmitted: AtomicU64,
}

#[derive(Clone, Copy, Debug)]
pub struct Counts {
    /// Packets the path delivers (or will, once their delay is over).
    pub passed: u64,
    /// Lost at random.
    pub lost: u64,
    /// Dropped because the bottleneck queue was full.
    pub dropped: u64,
    pub retransmitted: u64,
}

impl Stats {
    pub fn counts(&self) -> Counts {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        Counts {
            passed: get(&self.passed),
            lost: get(&self.lost),
            dropped: get(&self.dropped),
            retransmitted: get(&self.retransmitted),
        }
    }
}

fn bump(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

/// SplitMix64: small, fast, and plenty for loss draws.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `0.0..1.0`.
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn chance(&mut self, p: f64) -> bool {
        p > 0.0 && self.unit() < p
    }
}

/// The bottleneck of one direction, shared by everything that goes that way (all the
/// connections through a TCP link, all the dialers of a UDP link).
struct Bottleneck {
    rate: u64,
    queue: Duration,
    /// When it finishes sending what it has queued.
    busy_until: Instant,
}

impl Bottleneck {
    fn shared(imp: &Impairment) -> Arc<std::sync::Mutex<Self>> {
        Arc::new(std::sync::Mutex::new(Self {
            rate: imp.rate,
            queue: imp.queue,
            busy_until: Instant::now(),
        }))
    }

    /// When a packet handed over at `now` has left, or `None` if the queue is full.
    fn departure(&mut self, now: Instant, wire_len: usize) -> Option<Instant> {
        if self.rate == 0 {
            return Some(now);
        }
        let start = self.busy_until.max(now);
        if start - now > self.queue {
            return None;
        }
        self.busy_until = start + Duration::from_secs_f64(wire_len as f64 * 8.0 / self.rate as f64);
        Some(self.busy_until)
    }
}

/// One direction of a link, as one flow of packets sees it: the shared bottleneck,
/// then loss, delay and jitter of its own.
struct Path {
    imp: Impairment,
    rng: Rng,
    /// Gilbert model: in a loss burst.
    bad: bool,
    bottleneck: Arc<std::sync::Mutex<Bottleneck>>,
    last_arrival: Instant,
    stats: Arc<Stats>,
}

impl Path {
    /// `salt` tells apart the paths that share a seed (directions, connections).
    fn new(
        imp: Impairment,
        salt: u64,
        bottleneck: Arc<std::sync::Mutex<Bottleneck>>,
        stats: Arc<Stats>,
    ) -> Self {
        Self {
            imp,
            rng: Rng(imp.seed ^ salt.wrapping_mul(0x2545_f491_4f6c_dd1d)),
            bad: false,
            bottleneck,
            last_arrival: Instant::now(),
            stats,
        }
    }

    /// When a packet of `wire_len` bytes (headers included) handed to the path at `now`
    /// arrives at the other end, or `None` if it does not.
    fn send(&mut self, now: Instant, wire_len: usize) -> Option<Instant> {
        let departure = self.bottleneck.lock().unwrap().departure(now, wire_len);
        let Some(departure) = departure else {
            bump(&self.stats.dropped);
            return None;
        };
        if self.lose() {
            bump(&self.stats.lost);
            return None;
        }
        let mut arrival = departure + self.imp.delay + self.imp.jitter.mul_f64(self.rng.unit());
        if self.rng.chance(self.imp.reorder) {
            // Held back without holding up the packets behind it.
            arrival += self.imp.reorder_gap;
        } else {
            arrival = arrival.max(self.last_arrival);
            self.last_arrival = arrival;
        }
        bump(&self.stats.passed);
        Some(arrival)
    }

    /// Gilbert model: bursts start with probability `q` and end with `r = 1 / burst`
    /// per packet, every packet in a burst is lost. `q` is chosen so the long-run share
    /// of lost packets is `loss`.
    fn lose(&mut self) -> bool {
        let p = self.imp.loss;
        if self.imp.burst <= 1.0 {
            return self.rng.chance(p);
        }
        let r = 1.0 / self.imp.burst;
        let q = p * r / (1.0 - p);
        self.bad = if self.bad {
            !self.rng.chance(r)
        } else {
            self.rng.chance(q)
        };
        self.bad
    }
}

/// An item due at a point in time, for a min-heap (ties in insertion order).
struct Timed<T> {
    at: Instant,
    tie: u64,
    item: T,
}

impl<T> PartialEq for Timed<T> {
    fn eq(&self, other: &Self) -> bool {
        (self.at, self.tie) == (other.at, other.tie)
    }
}

impl<T> Eq for Timed<T> {}

impl<T> PartialOrd for Timed<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Timed<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.at, self.tie).cmp(&(other.at, other.tie))
    }
}

/// Items ordered by due time.
struct Schedule<T> {
    heap: BinaryHeap<Reverse<Timed<T>>>,
    tie: u64,
}

impl<T> Schedule<T> {
    fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            tie: 0,
        }
    }

    fn push(&mut self, at: Instant, item: T) {
        self.tie += 1;
        self.heap.push(Reverse(Timed {
            at,
            tie: self.tie,
            item,
        }));
    }

    fn next_at(&self) -> Option<Instant> {
        self.heap.peek().map(|Reverse(t)| t.at)
    }

    /// The earliest item due by `now`, with its due time.
    fn pop_due(&mut self, now: Instant) -> Option<(Instant, T)> {
        if self.next_at()? > now {
            return None;
        }
        self.heap.pop().map(|Reverse(t)| (t.at, t.item))
    }
}

async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// A task that is aborted when this is dropped.
struct Task(JoinHandle<()>);

impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn spawn(future: impl Future<Output = ()> + Send + 'static) -> Task {
    Task(tokio::spawn(future))
}

fn localhost() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 0))
}

/// Buffers large enough that the proxy itself never drops a packet.
fn udp_socket(connect: Option<SocketAddr>) -> UdpSocket {
    let socket = std::net::UdpSocket::bind(localhost()).unwrap();
    let sock = socket2::SockRef::from(&socket);
    let _ = sock.set_recv_buffer_size(4 << 20);
    let _ = sock.set_send_buffer_size(4 << 20);
    if let Some(peer) = connect {
        socket.connect(peer).unwrap();
    }
    socket.set_nonblocking(true).unwrap();
    UdpSocket::from_std(socket).unwrap()
}

/// On Windows a UDP socket reports an ICMP "port unreachable" for an earlier send as an
/// error on the next receive; that is no reason to stop.
fn keeps_receiving(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionRefused
    )
}

/// IPv4 + UDP headers.
const UDP_OVERHEAD: usize = 28;

/// A datagram and where it goes when it comes out of the path.
struct Datagram {
    data: Vec<u8>,
    via: Arc<UdpSocket>,
    /// `None`: `via` is connected.
    to: Option<SocketAddr>,
}

/// A UDP proxy on a local port: datagrams from each dialer address go to `upstream`
/// from a socket of their own (like a NAT), answers come back to that address.
pub struct UdpLink {
    pub port: u16,
    stats: Arc<Stats>,
    _tasks: [Task; 3],
}

impl UdpLink {
    pub fn start(upstream: u16, imp: Impairment) -> Self {
        let stats = Arc::new(Stats::default());
        let front = Arc::new(udp_socket(None));
        let port = front.local_addr().unwrap().port();
        let (to_listener, forward) =
            udp_path(Path::new(imp, 1, Bottleneck::shared(&imp), stats.clone()));
        let (to_dialer, back) =
            udp_path(Path::new(imp, 2, Bottleneck::shared(&imp), stats.clone()));
        let upstream = SocketAddr::from(([127, 0, 0, 1], upstream));
        let front_task = spawn(async move {
            // Dropped with this task, which stops the readers.
            let mut peers: HashMap<SocketAddr, (Arc<UdpSocket>, Task)> = HashMap::new();
            let mut buf = vec![0u8; 65_536];
            loop {
                let (n, from) = match front.recv_from(&mut buf).await {
                    Ok(r) => r,
                    Err(e) if keeps_receiving(&e) => continue,
                    Err(_) => return,
                };
                let (sock, _) = peers.entry(from).or_insert_with(|| {
                    let sock = Arc::new(udp_socket(Some(upstream)));
                    let reader = spawn(udp_answers(
                        sock.clone(),
                        front.clone(),
                        from,
                        to_dialer.clone(),
                    ));
                    (sock, reader)
                });
                let _ = to_listener.send(Datagram {
                    data: buf[..n].to_vec(),
                    via: sock.clone(),
                    to: None,
                });
            }
        });
        Self {
            port,
            stats,
            _tasks: [front_task, forward, back],
        }
    }

    pub fn stats(&self) -> Arc<Stats> {
        self.stats.clone()
    }
}

/// Answers from the listening side to one dialer address.
async fn udp_answers(
    sock: Arc<UdpSocket>,
    front: Arc<UdpSocket>,
    dialer: SocketAddr,
    to_dialer: mpsc::UnboundedSender<Datagram>,
) {
    let mut buf = vec![0u8; 65_536];
    loop {
        let n = match sock.recv(&mut buf).await {
            Ok(n) => n,
            Err(e) if keeps_receiving(&e) => continue,
            Err(_) => return,
        };
        let _ = to_dialer.send(Datagram {
            data: buf[..n].to_vec(),
            via: front.clone(),
            to: Some(dialer),
        });
    }
}

/// Runs datagrams through `path` and sends them on when they arrive.
fn udp_path(mut path: Path) -> (mpsc::UnboundedSender<Datagram>, Task) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Datagram>();
    let task = spawn(async move {
        let mut schedule = Schedule::new();
        loop {
            tokio::select! {
                datagram = rx.recv() => {
                    let Some(d) = datagram else { return };
                    if let Some(at) = path.send(Instant::now(), d.data.len() + UDP_OVERHEAD) {
                        schedule.push(at, d);
                    }
                }
                _ = sleep_until(schedule.next_at()) => {
                    while let Some((_, d)) = schedule.pop_due(Instant::now()) {
                        let _ = match d.to {
                            Some(to) => d.via.send_to(&d.data, to).await,
                            None => d.via.send(&d.data).await,
                        };
                    }
                }
            }
        }
    });
    (tx, task)
}

/// A TCP proxy on a local port to `upstream`; each direction of each connection runs
/// through a model of TCP over the path (see [`TcpModel`]).
pub struct TcpLink {
    pub port: u16,
    stats: Arc<Stats>,
    _task: Task,
}

impl TcpLink {
    pub fn start(upstream: u16, imp: Impairment) -> Self {
        let stats = Arc::new(Stats::default());
        let listener = std::net::TcpListener::bind(localhost()).unwrap();
        listener.set_nonblocking(true).unwrap();
        let listener = TcpListener::from_std(listener).unwrap();
        let port = listener.local_addr().unwrap().port();
        let link_stats = stats.clone();
        let (to_listener, to_dialer) = (Bottleneck::shared(&imp), Bottleneck::shared(&imp));
        let task = spawn(async move {
            // Dropped with this task, which stops the connections.
            let mut connections = Vec::new();
            let mut salt = 0;
            while let Ok((down, _)) = listener.accept().await {
                let Ok(up) = TcpStream::connect(("127.0.0.1", upstream)).await else {
                    continue;
                };
                let _ = (down.set_nodelay(true), up.set_nodelay(true));
                let (down_read, down_write) = down.into_split();
                let (up_read, up_write) = up.into_split();
                for (from, to, bottleneck) in [
                    (down_read, up_write, &to_listener),
                    (up_read, down_write, &to_dialer),
                ] {
                    salt += 1;
                    let path = Path::new(imp, salt, bottleneck.clone(), link_stats.clone());
                    connections.push(spawn(tcp_path(from, to, path)));
                }
            }
        });
        Self {
            port,
            stats,
            _task: task,
        }
    }

    pub fn stats(&self) -> Arc<Stats> {
        self.stats.clone()
    }
}

/// Payload per segment (1500-byte MTU, TCP timestamps on).
const MSS: usize = 1448;
/// IPv4 + TCP headers with timestamps.
const TCP_OVERHEAD: usize = 52;
/// Largest window the receiver offers (Linux autotuning goes up to about 6 MB).
const RECEIVE_WINDOW: f64 = 4.0 * 1024.0 * 1024.0;
/// Data the sender holds that is not yet in the window. Kariz sets `TCP_NOTSENT_LOWAT`
/// to 16 KiB on connections that carry mux, so a real sender holds no more than that;
/// the rest waits in the tunnel, where datagrams can go ahead of it.
const UNSENT_LIMIT: usize = 16 * 1024;
/// Duplicate acks that make a sender retransmit.
const DUPACK_THRESHOLD: u32 = 3;

/// Cubic (RFC 9438), Linux's default, in the parts that decide throughput on a lossy
/// path: slow start, the decrease to 0.7 on loss, the cubic growth back and the
/// Reno-friendly estimate that dominates when losses come often. Windows in bytes.
struct Cubic {
    cwnd: f64,
    ssthresh: f64,
    /// Window before the last decrease.
    w_max: f64,
    /// Start of the current growth period and its `K`, in seconds.
    epoch: Option<(Instant, f64)>,
    /// The window a Reno sender would have, in segments.
    w_est: f64,
}

impl Cubic {
    const BETA: f64 = 0.7;
    const C: f64 = 0.4;
    /// Reno-friendly growth per RTT, in segments.
    const ALPHA: f64 = 3.0 * (1.0 - Self::BETA) / (1.0 + Self::BETA);

    fn new() -> Self {
        Self {
            cwnd: 10.0 * MSS as f64,
            ssthresh: f64::INFINITY,
            w_max: 0.0,
            epoch: None,
            w_est: 0.0,
        }
    }

    fn on_ack(&mut self, acked: usize, now: Instant, rtt: Duration) {
        let acked = acked as f64;
        if self.cwnd < self.ssthresh {
            self.cwnd += acked;
            return;
        }
        let mss = MSS as f64;
        let cwnd = self.cwnd / mss;
        let (start, k) = *self.epoch.get_or_insert_with(|| {
            let k = ((self.w_max - self.cwnd).max(0.0) / mss / Self::C).cbrt();
            (now, k)
        });
        let t = (now - start + rtt).as_secs_f64();
        let cubic = Self::C * (t - k).powi(3) + self.w_max / mss;
        self.w_est += Self::ALPHA * acked / self.cwnd;
        let target = cubic.max(self.w_est).clamp(cwnd, 1.5 * cwnd);
        self.cwnd += (target - cwnd) / cwnd * acked;
    }

    fn on_loss(&mut self) {
        self.w_max = self.cwnd;
        self.cwnd = (self.cwnd * Self::BETA).max(2.0 * MSS as f64);
        self.ssthresh = self.cwnd;
        self.epoch = None;
        self.w_est = self.cwnd / MSS as f64;
    }
}

struct Segment {
    data: Bytes,
    /// When the latest transmission arrives; `None` while it is lost.
    arrival: Option<Instant>,
    /// Transmissions so far (a timeout for an earlier one is stale).
    tries: u32,
    /// Acks for later segments since the latest transmission was lost.
    dupacks: u32,
}

enum Event {
    /// A segment's ack reaches the sender.
    Ack { seq: u64, len: usize, sent: Instant },
    /// A lost transmission not found by duplicate acks: the tail loss probe finds it.
    Probe { seq: u64, tries: u32 },
}

/// The sender's TCP for one direction of a connection, over a [`Path`]. Acks travel back
/// with the path's delay and are never lost. Recovery is modelled as SACK-based fast
/// retransmission: a segment is resent after three acks for later segments, or, when no
/// later ones follow (a tail loss), after three smoothed RTTs (tail loss probe, then its
/// ack). The receiver delivers in order, so a lost segment holds up everything behind it
/// until its retransmission arrives. Not modelled: retransmission timeouts and their
/// backoff, delayed acks, and a path that loses acks; the model is therefore somewhat
/// kinder to TCP than a real path.
struct TcpModel {
    path: Path,
    cc: Cubic,
    srtt: Option<Duration>,
    unsent: BytesMut,
    /// Segments not yet delivered, `segs[0]` has sequence number `first_seq`.
    segs: VecDeque<Segment>,
    first_seq: u64,
    next_seq: u64,
    in_flight: usize,
    /// Segments whose latest transmission was lost, oldest first.
    lost: Vec<u64>,
    /// No second window decrease for losses among segments sent before this one.
    recovery_until: u64,
    events: Schedule<Event>,
}

impl TcpModel {
    fn new(path: Path) -> Self {
        Self {
            path,
            cc: Cubic::new(),
            srtt: None,
            unsent: BytesMut::new(),
            segs: VecDeque::new(),
            first_seq: 0,
            next_seq: 0,
            in_flight: 0,
            lost: Vec::new(),
            recovery_until: 0,
            events: Schedule::new(),
        }
    }

    fn srtt(&self) -> Duration {
        self.srtt
            .unwrap_or(self.path.imp.delay * 2)
            .max(Duration::from_millis(1))
    }

    /// Sends new segments while the window has room.
    fn send_new(&mut self, now: Instant) {
        while !self.unsent.is_empty() {
            let len = self.unsent.len().min(MSS);
            if (self.in_flight + len) as f64 > self.cc.cwnd.min(RECEIVE_WINDOW) {
                return;
            }
            self.segs.push_back(Segment {
                data: self.unsent.split_to(len).freeze(),
                arrival: None,
                tries: 0,
                dupacks: 0,
            });
            let seq = self.next_seq;
            self.next_seq += 1;
            self.in_flight += len;
            self.transmit(seq, now);
        }
    }

    fn transmit(&mut self, seq: u64, now: Instant) {
        let seg = &mut self.segs[(seq - self.first_seq) as usize];
        seg.tries += 1;
        seg.dupacks = 0;
        let len = seg.data.len();
        match self.path.send(now, len + TCP_OVERHEAD) {
            Some(at) => {
                seg.arrival = Some(at);
                let ack_at = at + self.path.imp.delay;
                self.events.push(
                    ack_at,
                    Event::Ack {
                        seq,
                        len,
                        sent: now,
                    },
                );
            }
            None => {
                let tries = seg.tries;
                self.lost.push(seq);
                let probe_at = now + self.srtt() * 3;
                self.events.push(probe_at, Event::Probe { seq, tries });
            }
        }
    }

    fn retransmit(&mut self, seq: u64, now: Instant) {
        self.lost.retain(|&l| l != seq);
        bump(&self.path.stats.retransmitted);
        if seq >= self.recovery_until {
            self.cc.on_loss();
            self.recovery_until = self.next_seq;
        }
        self.transmit(seq, now);
    }

    /// Handles an event at the time it was due, so a late timer does not skew the model.
    fn on_event(&mut self, at: Instant, event: Event) {
        match event {
            Event::Ack { seq, len, sent } => {
                self.in_flight -= len;
                let sample = at - sent;
                self.srtt = Some(match self.srtt {
                    Some(srtt) => srtt.mul_f64(0.875) + sample.mul_f64(0.125),
                    None => sample,
                });
                self.cc.on_ack(len, at, self.srtt());
                let mut found = Vec::new();
                for &l in self.lost.iter().filter(|&&l| l < seq) {
                    let seg = &mut self.segs[(l - self.first_seq) as usize];
                    seg.dupacks += 1;
                    if seg.dupacks >= DUPACK_THRESHOLD {
                        found.push(l);
                    }
                }
                for l in found {
                    self.retransmit(l, at);
                }
            }
            Event::Probe { seq, tries } => {
                let current = seq >= self.first_seq
                    && self.segs[(seq - self.first_seq) as usize].tries == tries;
                if current && self.lost.contains(&seq) {
                    self.retransmit(seq, at);
                }
            }
        }
    }

    /// Takes the segments that have arrived in order by `now`.
    fn arrived(&mut self, now: Instant) -> Option<BytesMut> {
        let mut out: Option<BytesMut> = None;
        while let Some(seg) = self.segs.front() {
            if !seg.arrival.is_some_and(|at| at <= now) {
                break;
            }
            let seg = self.segs.pop_front().unwrap();
            self.first_seq += 1;
            out.get_or_insert_with(BytesMut::new)
                .extend_from_slice(&seg.data);
        }
        out
    }

    fn next_wake(&self) -> Option<Instant> {
        let front = self.segs.front().and_then(|s| s.arrival);
        match (self.events.next_at(), front) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn idle(&self) -> bool {
        self.unsent.is_empty() && self.segs.is_empty()
    }
}

/// One direction of a proxied connection.
async fn tcp_path(mut from: OwnedReadHalf, mut to: OwnedWriteHalf, path: Path) {
    let mut model = TcpModel::new(path);
    let mut buf = vec![0u8; 64 * 1024];
    let mut eof = false;
    loop {
        model.send_new(Instant::now());
        if let Some(data) = model.arrived(Instant::now()) {
            if to.write_all(&data).await.is_err() {
                return;
            }
        }
        if eof && model.idle() {
            let _ = to.shutdown().await;
            return;
        }
        let room = UNSENT_LIMIT.saturating_sub(model.unsent.len());
        tokio::select! {
            read = from.read(&mut buf[..room]), if room > 0 && !eof => match read {
                Ok(n @ 1..) => model.unsent.extend_from_slice(&buf[..n]),
                _ => eof = true,
            },
            _ = sleep_until(model.next_wake()) => {
                // Acks open the window at their own time, even if the timer fired late.
                while let Some((at, event)) = model.events.pop_due(Instant::now()) {
                    model.on_event(at, event);
                    model.send_new(at);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    fn path(imp: Impairment) -> Path {
        Path::new(imp, 0, Bottleneck::shared(&imp), Arc::new(Stats::default()))
    }

    /// Loss draws alone, without timing: the share lost and the mean burst length.
    fn loss_pattern(imp: Impairment, packets: usize) -> (f64, f64) {
        let mut path = path(imp);
        let now = Instant::now();
        let (mut lost, mut bursts, mut in_burst) = (0, 0, false);
        for _ in 0..packets {
            let gone = path.send(now, 100).is_none();
            lost += gone as usize;
            bursts += (gone && !in_burst) as usize;
            in_burst = gone;
        }
        (
            lost as f64 / packets as f64,
            lost as f64 / bursts.max(1) as f64,
        )
    }

    #[test]
    fn random_loss_has_the_configured_rate() {
        for loss in [0.01, 0.05, 0.2] {
            let imp = Impairment {
                loss,
                ..Default::default()
            };
            let (rate, burst) = loss_pattern(imp, 200_000);
            assert!((rate - loss).abs() < loss * 0.1, "{loss}: lost {rate}");
            assert!(burst < 1.0 + loss * 2.0, "{loss}: bursts of {burst}");
        }
        let (rate, _) = loss_pattern(Impairment::default(), 10_000);
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn burst_loss_has_the_configured_rate_and_length() {
        for (loss, burst) in [(0.01, 3.0), (0.05, 5.0)] {
            let imp = Impairment {
                loss,
                burst,
                ..Default::default()
            };
            let (rate, mean) = loss_pattern(imp, 2_000_000);
            assert!(
                (rate - loss).abs() < loss * 0.1,
                "{loss}/{burst}: lost {rate}"
            );
            assert!(
                (mean - burst).abs() < burst * 0.1,
                "{loss}/{burst}: bursts of {mean}"
            );
        }
    }

    #[test]
    fn full_queue_drops_and_rate_spaces_departures() {
        // 8 Mbit/s: a 1000-byte packet every millisecond; a 10 ms queue holds 10.
        let imp = Impairment {
            rate: 8_000_000,
            queue: ms(10),
            ..Default::default()
        };
        let mut path = path(imp);
        let now = Instant::now();
        let arrivals: Vec<_> = (0..100).filter_map(|_| path.send(now, 1000)).collect();
        assert_eq!(arrivals.len(), 11, "the one on the wire and ten waiting");
        for (i, at) in arrivals.iter().enumerate() {
            assert_eq!(*at - now, ms(i as u64 + 1));
        }
        let counts = path.stats.counts();
        assert_eq!((counts.passed, counts.dropped), (11, 89));
        // Once the queue has drained, packets pass again.
        assert!(path.send(now + ms(11), 1000).is_some());
    }

    /// A listener that records when each datagram (numbered by its first 4 bytes) came.
    fn udp_recorder() -> (u16, mpsc::UnboundedReceiver<(u32, Instant)>, Task) {
        let sock = udp_socket(None);
        let port = sock.local_addr().unwrap().port();
        let (tx, rx) = mpsc::unbounded_channel();
        let task = spawn(async move {
            let mut buf = vec![0u8; 65_536];
            while let Ok(4..) = sock.recv(&mut buf).await {
                let seq = u32::from_be_bytes(buf[..4].try_into().unwrap());
                let _ = tx.send((seq, Instant::now()));
            }
        });
        (port, rx, task)
    }

    /// Sends `count` numbered datagrams of `len` bytes, `gap` apart, through a UDP link;
    /// returns their send times and what arrived (in arrival order) within `wait`.
    async fn udp_run(
        imp: Impairment,
        count: u32,
        len: usize,
        gap: Duration,
        wait: Duration,
    ) -> (Vec<Instant>, Vec<(u32, Instant)>, Counts) {
        fine_timers();
        let (port, mut rx, _recorder) = udp_recorder();
        let link = UdpLink::start(port, imp);
        let client = udp_socket(Some(SocketAddr::from(([127, 0, 0, 1], link.port))));
        let mut sent = Vec::new();
        let mut packet = vec![0u8; len];
        for seq in 0..count {
            packet[..4].copy_from_slice(&seq.to_be_bytes());
            sent.push(Instant::now());
            client.send(&packet).await.unwrap();
            if !gap.is_zero() {
                tokio::time::sleep(gap).await;
            } else if seq % 100 == 99 {
                // A short pause per 100 packets: in one long burst, Linux's cap on socket
                // buffers (net.core.rmem_max) would drop packets on its own, which the
                // tests would take for the link's doing.
                tokio::time::sleep(ms(2)).await;
            }
        }
        let mut got = Vec::new();
        let deadline = Instant::now() + wait;
        while let Ok(Some(r)) = tokio::time::timeout_at(deadline, rx.recv()).await {
            got.push(r);
        }
        (sent, got, link.stats().counts())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn udp_loss_rate() {
        let imp = Impairment {
            loss: 0.1,
            ..Default::default()
        };
        let (_, got, counts) = udp_run(imp, 5000, 100, Duration::ZERO, ms(500)).await;
        assert_eq!(counts.lost + counts.passed, 5000, "{counts:?}");
        let rate = counts.lost as f64 / 5000.0;
        assert!((0.08..0.12).contains(&rate), "lost {rate}");
        assert_eq!(counts.passed, got.len() as u64);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn udp_delay_is_within_bounds_and_jitter_keeps_order() {
        let imp = Impairment {
            delay: ms(30),
            jitter: ms(10),
            ..Default::default()
        };
        let (sent, got, _) = udp_run(imp, 200, 100, ms(2), ms(500)).await;
        assert_eq!(got.len(), 200);
        let mut delays: Vec<_> = got
            .iter()
            .map(|&(seq, at)| at - sent[seq as usize])
            .collect();
        assert!(
            got.windows(2).all(|w| w[0].0 < w[1].0),
            "reordered without reorder"
        );
        delays.sort();
        assert!(delays[0] >= ms(30), "early: {:?}", delays[0]);
        let median = delays[delays.len() / 2];
        assert!(median <= ms(30 + 10 + 10), "median {median:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn udp_reordering() {
        let imp = Impairment {
            reorder: 0.2,
            reorder_gap: ms(10),
            ..Default::default()
        };
        let (_, got, _) = udp_run(imp, 200, 100, Duration::ZERO, ms(500)).await;
        assert_eq!(got.len(), 200);
        // Packets that arrive after a later one.
        let mut newest = 0;
        let mut late = 0;
        for &(seq, _) in &got {
            late += (seq < newest) as usize;
            newest = newest.max(seq);
        }
        assert!((20..=60).contains(&late), "{late} of 200 overtaken");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn udp_rate_limit() {
        // 1000 bytes on the wire per packet at 8 Mbit/s: 500 packets take 0.5 s.
        let imp = Impairment {
            rate: 8_000_000,
            queue: ms(2000),
            ..Default::default()
        };
        let len = 1000 - UDP_OVERHEAD;
        let (_, got, _) = udp_run(imp, 500, len, Duration::ZERO, ms(1500)).await;
        assert_eq!(got.len(), 500);
        let span = got.last().unwrap().1 - got[0].1;
        assert!(ms(420) < span && span < ms(600), "{span:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn udp_answers_reach_each_dialer() {
        let echo = udp_socket(None);
        let port = echo.local_addr().unwrap().port();
        let _echo = spawn(async move {
            let mut buf = vec![0u8; 2048];
            while let Ok((n, from)) = echo.recv_from(&mut buf).await {
                let _ = echo.send_to(&buf[..n], from).await;
            }
        });
        let imp = Impairment {
            delay: ms(5),
            ..Default::default()
        };
        let link = UdpLink::start(port, imp);
        let to = SocketAddr::from(([127, 0, 0, 1], link.port));
        for i in 0..3u8 {
            let client = udp_socket(Some(to));
            let mut buf = [0u8; 16];
            for round in 0..3u8 {
                client.send(&[i, round]).await.unwrap();
                let n = tokio::time::timeout(ms(1000), client.recv(&mut buf))
                    .await
                    .expect("answer")
                    .unwrap();
                assert_eq!(&buf[..n], &[i, round]);
            }
        }
    }

    /// A listener that reads one connection to the end and hands over what it got.
    async fn tcp_sink() -> (u16, tokio::sync::oneshot::Receiver<Vec<u8>>) {
        let listener = TcpListener::bind(localhost()).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut all = Vec::new();
            s.read_to_end(&mut all).await.unwrap();
            let _ = tx.send(all);
        });
        (port, rx)
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tcp_stream_arrives_intact_despite_loss() {
        let (port, received) = tcp_sink().await;
        let imp = Impairment {
            delay: ms(5),
            loss: 0.05,
            burst: 2.0,
            rate: 50_000_000,
            ..Default::default()
        };
        let link = TcpLink::start(port, imp);
        let payload = pattern(2 << 20);
        let mut s = TcpStream::connect(("127.0.0.1", link.port)).await.unwrap();
        s.write_all(&payload).await.unwrap();
        s.shutdown().await.unwrap();
        let got = tokio::time::timeout(Duration::from_secs(30), received)
            .await
            .expect("transfer finished")
            .unwrap();
        assert!(got == payload, "stream corrupted");
        let counts = link.stats().counts();
        assert!(counts.lost > 0 && counts.retransmitted >= counts.lost);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tcp_round_trip_has_the_link_delay() {
        fine_timers();
        let listener = TcpListener::bind(localhost()).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let (mut r, mut w) = s.split();
            let _ = tokio::io::copy(&mut r, &mut w).await;
        });
        let imp = Impairment {
            delay: ms(30),
            ..Default::default()
        };
        let link = TcpLink::start(port, imp);
        let mut s = TcpStream::connect(("127.0.0.1", link.port)).await.unwrap();
        s.set_nodelay(true).unwrap();
        let mut rtts = Vec::new();
        let mut buf = [0u8; 4];
        for i in 0..10u32 {
            let start = Instant::now();
            s.write_all(&i.to_be_bytes()).await.unwrap();
            s.read_exact(&mut buf).await.unwrap();
            assert_eq!(buf, i.to_be_bytes());
            rtts.push(start.elapsed());
        }
        rtts.sort();
        assert!(rtts[0] >= ms(60), "early: {:?}", rtts[0]);
        assert!(rtts[5] <= ms(80), "median {:?}", rtts[5]);
    }

    /// Bytes per second that `connections` downloads at once through `imp` reach
    /// together after `warmup`, measured over `window`.
    async fn tcp_goodput(
        imp: Impairment,
        connections: usize,
        warmup: Duration,
        window: Duration,
    ) -> f64 {
        fine_timers();
        let listener = TcpListener::bind(localhost()).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let _source = spawn(async move {
            let mut writers = Vec::new();
            while let Ok((mut s, _)) = listener.accept().await {
                writers.push(spawn(async move {
                    let chunk = vec![7u8; 64 * 1024];
                    while s.write_all(&chunk).await.is_ok() {}
                }));
            }
        });
        let link = TcpLink::start(port, imp);
        let link_port = link.port;
        let (start, end) = (Instant::now() + warmup, Instant::now() + warmup + window);
        let downloads = (0..connections).map(|_| {
            tokio::spawn(async move {
                let mut s = TcpStream::connect(("127.0.0.1", link_port)).await.unwrap();
                let mut buf = vec![0u8; 256 * 1024];
                let mut total = 0;
                while let Ok(read) = tokio::time::timeout_at(end, s.read(&mut buf)).await {
                    let n = read.unwrap();
                    assert!(n > 0, "download ended");
                    if Instant::now() >= start {
                        total += n;
                    }
                }
                total
            })
        });
        let mut total = 0;
        for d in downloads.collect::<Vec<_>>() {
            total += d.await.unwrap();
        }
        total as f64 / window.as_secs_f64()
    }

    /// Three downloads share the link's rate between them, and together fill it.
    #[tokio::test(flavor = "multi_thread")]
    async fn tcp_fills_a_clean_link() {
        let imp = Impairment {
            delay: ms(10),
            rate: 20_000_000,
            ..Default::default()
        };
        let goodput = tcp_goodput(imp, 3, ms(1000), ms(2000)).await;
        let payload_rate = 20e6 / 8.0 * MSS as f64 / (MSS + TCP_OVERHEAD) as f64;
        assert!(
            goodput > 0.7 * payload_rate && goodput < 1.05 * payload_rate,
            "{:.1} Mbit/s",
            goodput * 8e-6
        );
    }

    /// With random loss `p`, Reno and Cubic (in its Reno-friendly region) average a
    /// window of about 1.22 / sqrt(p) segments (Mathis et al.).
    #[tokio::test(flavor = "multi_thread")]
    async fn tcp_throughput_falls_with_loss_as_tcp_does() {
        let (rtt, loss) = (ms(20), 0.02);
        let imp = Impairment {
            delay: rtt / 2,
            loss,
            rate: 100_000_000,
            ..Default::default()
        };
        let goodput = tcp_goodput(imp, 1, ms(1000), ms(3000)).await;
        let expected = MSS as f64 / rtt.as_secs_f64() * 1.22 / f64::sqrt(loss);
        assert!(
            goodput > 0.5 * expected && goodput < 2.0 * expected,
            "{:.1} Mbit/s, expected about {:.1}",
            goodput * 8e-6,
            expected * 8e-6
        );
    }
}
