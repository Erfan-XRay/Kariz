//! UDP forwarding (phase 3, see `docs/PHASE3.md`).
//!
//! A **flow** is one client address talking to one UDP forward rule. The entry keeps a
//! flow table per rule and opens a tunnel path per flow; the exit gives each flow its own
//! UDP socket connected to the target. On a mux session a flow is a stream carrying
//! `DGRAM` frames; without mux it is a whole channel carrying length-prefixed packets.
//! Either way [`relay`] moves packets both ways until the flow has been idle for the
//! configured timeout or one side ends it.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use socket2::SockRef;
use tokio::io::AsyncWriteExt;
use tokio::net::UdpSocket;
use tokio::sync::Notify;
use tokio::time::{sleep, timeout, Instant};
use tracing::{debug, warn};

use crate::channel::Channel;
use crate::config::{Tuning, UdpTuning};
use crate::mux::Transport;
use crate::proto::{self, MAX_DATAGRAM};
use crate::session::ResetReason;

/// Receive buffer for one packet: the largest UDP payload fits.
const PACKET_BUFFER: usize = 64 * 1024;
/// Packets gathered into one write on a channel without mux.
const WRITE_BATCH: usize = 64 * 1024;
/// A client whose flow could not be opened is ignored this long.
const FAILED_BACKOFF: Duration = Duration::from_secs(5);
/// At most one "too many flows" warning per rule in this interval.
const FULL_WARN_INTERVAL: Duration = Duration::from_secs(10);

/// Where a flow's packets come from outside the tunnel.
pub trait PacketSource: Send {
    /// The next packet; `None` when the source is gone.
    fn recv(&mut self) -> impl Future<Output = io::Result<Option<Bytes>>> + Send;

    /// A packet that is already waiting, if any (to gather several into one write).
    fn try_recv(&mut self) -> Option<Bytes> {
        None
    }
}

/// Where a flow's packets go outside the tunnel.
pub trait PacketSink: Send + Sync {
    fn send(&self, packet: &[u8]) -> impl Future<Output = io::Result<()>> + Send;
}

/// Moves packets between `channel` and the local side until the flow has seen no packet
/// in either direction for `idle`, or either side ends it. Ends the tunnel side cleanly
/// (`FIN` or shutdown) so the peer ends its side too.
pub async fn relay<Src, Snk>(
    channel: Channel,
    mut source: Src,
    sink: Snk,
    idle: Duration,
) -> io::Result<()>
where
    Src: PacketSource,
    Snk: PacketSink,
{
    let start = Instant::now();
    let last = AtomicU64::new(0);
    let touch = || last.store(start.elapsed().as_millis() as u64, Ordering::Relaxed);
    let watchdog = async {
        loop {
            let quiet = start.elapsed() - Duration::from_millis(last.load(Ordering::Relaxed));
            if quiet >= idle {
                return io::Result::Ok(());
            }
            sleep(idle - quiet).await;
        }
    };
    let deliver = |packet: Bytes| {
        let sink = &sink;
        async move {
            if let Err(e) = sink.send(&packet).await {
                debug!(error = %e, "UDP packet not delivered");
            }
        }
    };

    match channel {
        Channel::Stream(stream) => {
            let up = async {
                while let Some(packet) = source.recv().await? {
                    touch();
                    // A full session queue drops the packet (counted by the session).
                    stream.send_datagram(packet);
                }
                Ok(())
            };
            let down = async {
                loop {
                    match stream.recv_datagram().await {
                        Ok(Some(packet)) => {
                            touch();
                            deliver(packet).await;
                        }
                        Ok(None) => return Ok(()),
                        // v0.2 peers reset UDP opens as a protocol error.
                        Err(_) if stream.reset_reason() == Some(ResetReason::Protocol) => {
                            return Err(ResetReason::Unsupported.to_error())
                        }
                        Err(e) => return Err(e),
                    }
                }
            };
            let result = tokio::select! {
                r = up => r,
                r = down => r,
                r = watchdog => r,
            };
            let _ = stream.finish();
            result
        }
        Channel::Link(link) => {
            let (mut reader, mut writer) = link.into_halves();
            let up = async {
                let mut batch = Vec::with_capacity(WRITE_BATCH);
                while let Some(packet) = source.recv().await? {
                    touch();
                    batch.clear();
                    let mut next = Some(packet);
                    while let Some(packet) = next {
                        // Larger than UDP over IPv4 allows (IPv6 jumbo); cannot be framed.
                        if packet.len() <= MAX_DATAGRAM {
                            proto::put_datagram(&packet, &mut batch)?;
                        }
                        next = if batch.len() < WRITE_BATCH {
                            source.try_recv()
                        } else {
                            None
                        };
                    }
                    writer.write_all(&batch).await?;
                    writer.flush().await?;
                }
                Ok(())
            };
            let down = async {
                while let Some(packet) = proto::read_datagram(&mut reader).await? {
                    touch();
                    deliver(packet).await;
                }
                Ok(())
            };
            let result = tokio::select! {
                r = up => r,
                r = down => r,
                r = watchdog => r,
            };
            let _ = timeout(Duration::from_secs(1), writer.shutdown()).await;
            result
        }
    }
}

/// Sets the socket buffer sizes; the kernel may cap them (`net.core.rmem_max`).
fn tune(socket: &UdpSocket, udp: &UdpTuning) {
    let sock = SockRef::from(socket);
    if let Err(e) = sock
        .set_recv_buffer_size(udp.socket_buffer)
        .and_then(|_| sock.set_send_buffer_size(udp.socket_buffer))
    {
        debug!(error = %e, "could not set UDP socket buffers");
    }
}

// ---- Exit side ----

/// Exit side: a UDP socket connected to `target` (resolved now), for one flow.
pub async fn connect(target: &str, tuning: &Tuning) -> io::Result<UdpSocket> {
    let addr = timeout(tuning.dial_timeout, tokio::net::lookup_host(target))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "resolving the target timed out"))??
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "target has no address"))?;
    let local: SocketAddr = if addr.is_ipv4() {
        (Ipv4Addr::UNSPECIFIED, 0).into()
    } else {
        (Ipv6Addr::UNSPECIFIED, 0).into()
    };
    let socket = UdpSocket::bind(local).await?;
    tune(&socket, &tuning.udp);
    socket.connect(addr).await?;
    Ok(socket)
}

thread_local! {
    /// Receive buffer for the exit's flow sockets, one per worker thread instead of one
    /// per flow: a packet is read only once the socket is readable, and copied out at its
    /// real size right away.
    static RECV_BUFFER: std::cell::RefCell<Box<[u8]>> =
        std::cell::RefCell::new(vec![0u8; PACKET_BUFFER].into_boxed_slice());
}

/// A connected socket as both ends of the local side.
pub struct Connected {
    socket: Arc<UdpSocket>,
}

impl Connected {
    pub fn new(socket: UdpSocket) -> (Self, ConnectedSink) {
        let socket = Arc::new(socket);
        (
            Self {
                socket: socket.clone(),
            },
            ConnectedSink(socket),
        )
    }

    fn read_now(&self) -> io::Result<Bytes> {
        RECV_BUFFER.with_borrow_mut(|buf| {
            let n = self.socket.try_recv(buf)?;
            Ok(Bytes::copy_from_slice(&buf[..n]))
        })
    }
}

impl PacketSource for Connected {
    async fn recv(&mut self) -> io::Result<Option<Bytes>> {
        loop {
            self.socket.readable().await?;
            match self.read_now() {
                Ok(packet) => return Ok(Some(packet)),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                // An ICMP "port unreachable" from an earlier packet: the target may come
                // back, so keep the flow.
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => continue,
                Err(e) => return Err(e),
            }
        }
    }

    fn try_recv(&mut self) -> Option<Bytes> {
        self.read_now().ok()
    }
}

pub struct ConnectedSink(Arc<UdpSocket>);

impl PacketSink for ConnectedSink {
    async fn send(&self, packet: &[u8]) -> io::Result<()> {
        match self.0.send(packet).await {
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => Ok(()),
            r => r.map(|_| ()),
        }
    }
}

// ---- Entry side ----

/// Packets from one client waiting for its flow. Bounded, and it allocates only as
/// packets arrive (a tokio channel reserves room for 32 packets up front, which adds up
/// with thousands of mostly idle flows).
struct FlowQueue {
    state: Mutex<QueueState>,
    ready: Notify,
    capacity: usize,
}

#[derive(Default)]
struct QueueState {
    packets: VecDeque<Bytes>,
    /// The flow has ended; no more packets are taken.
    closed: bool,
}

enum PushError {
    Full,
    Closed(Bytes),
}

impl FlowQueue {
    fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(QueueState::default()),
            ready: Notify::new(),
            capacity,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn push(&self, packet: Bytes) -> Result<(), PushError> {
        let mut st = self.lock();
        if st.closed {
            return Err(PushError::Closed(packet));
        }
        if st.packets.len() >= self.capacity {
            return Err(PushError::Full);
        }
        st.packets.push_back(packet);
        drop(st);
        // One consumer: a stored permit covers a push that races with its wait.
        self.ready.notify_one();
        Ok(())
    }
}

/// The flow's end of a [`FlowQueue`]; dropping it closes the queue.
struct FlowReceiver(Arc<FlowQueue>);

impl Drop for FlowReceiver {
    fn drop(&mut self) {
        let mut st = self.0.lock();
        st.closed = true;
        st.packets.clear();
    }
}

impl PacketSource for FlowReceiver {
    async fn recv(&mut self) -> io::Result<Option<Bytes>> {
        loop {
            if let Some(packet) = self.0.lock().packets.pop_front() {
                return Ok(Some(packet));
            }
            self.0.ready.notified().await;
        }
    }

    fn try_recv(&mut self) -> Option<Bytes> {
        self.0.lock().packets.pop_front()
    }
}

/// Replies to one client through the rule's shared socket.
struct ReplyTo {
    socket: Arc<UdpSocket>,
    client: SocketAddr,
}

impl PacketSink for ReplyTo {
    async fn send(&self, packet: &[u8]) -> io::Result<()> {
        self.socket.send_to(packet, self.client).await.map(|_| ())
    }
}

/// Binds the UDP socket of a forward rule.
pub async fn bind(listen: &str, udp: &UdpTuning) -> io::Result<UdpSocket> {
    let socket = UdpSocket::bind(listen).await?;
    tune(&socket, udp);
    Ok(socket)
}

enum FlowState {
    Active {
        queue: Arc<FlowQueue>,
        id: u64,
    },
    /// Opening failed; packets from this client are dropped until then.
    Failed {
        until: Instant,
    },
}

#[derive(Default)]
struct Flows {
    map: HashMap<SocketAddr, FlowState>,
    active: usize,
    next_id: u64,
    last_full_warning: Option<Instant>,
}

impl Flows {
    /// Removes flow `id` of `client` (not a newer one for the same client), marking the
    /// client as failed for a while if the flow could not be opened.
    fn finish(&mut self, client: SocketAddr, id: u64, open_failed: bool) {
        if !matches!(self.map.get(&client), Some(FlowState::Active { id: i, .. }) if *i == id) {
            return;
        }
        self.active -= 1;
        if open_failed {
            let until = Instant::now() + FAILED_BACKOFF;
            self.map.insert(client, FlowState::Failed { until });
        } else {
            self.map.remove(&client);
        }
    }
}

/// Entry side: serves one UDP forward rule on `socket`. `open` opens a tunnel path for a
/// new flow (the open request is the caller's). Runs until the socket fails.
pub async fn serve<O, F>(
    socket: UdpSocket,
    udp: UdpTuning,
    target: String,
    open: O,
) -> io::Result<()>
where
    O: Fn() -> F + Send + Sync + 'static,
    F: Future<Output = io::Result<Channel>> + Send + 'static,
{
    let socket = Arc::new(socket);
    let flows = Arc::new(Mutex::new(Flows::default()));
    let open = Arc::new(open);
    let mut buf = vec![0u8; PACKET_BUFFER];
    loop {
        let (n, client) = match socket.recv_from(&mut buf).await {
            Ok(v) => v,
            // Linux reports ICMP errors for earlier replies here; not fatal.
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => continue,
            Err(e) => return Err(e),
        };
        let packet = Bytes::copy_from_slice(&buf[..n]);
        let mut guard = flows.lock().unwrap_or_else(|e| e.into_inner());
        let table = &mut *guard;
        let now = Instant::now();
        match table.map.get(&client) {
            Some(FlowState::Active { queue, .. }) => match queue.push(packet) {
                Ok(()) => continue,
                // The flow is behind: drop, as the network would.
                Err(PushError::Full) => continue,
                // The flow just ended; start a new one.
                Err(PushError::Closed(p)) => {
                    start_flow(table, client, p, &socket, &udp, &target, &flows, &open);
                    continue;
                }
            },
            Some(FlowState::Failed { until }) if now < *until => continue,
            _ => {}
        }
        if table.active >= udp.max_flows {
            let due = table
                .last_full_warning
                .map_or(true, |t| now.duration_since(t) >= FULL_WARN_INTERVAL);
            if due {
                table.last_full_warning = Some(now);
                warn!(%target, max = udp.max_flows, "too many UDP flows, dropping packets from new clients");
            }
            continue;
        }
        start_flow(table, client, packet, &socket, &udp, &target, &flows, &open);
    }
}

/// Starts a flow for `client` with `packet` as its first packet.
#[allow(clippy::too_many_arguments)]
fn start_flow<O, F>(
    table: &mut Flows,
    client: SocketAddr,
    packet: Bytes,
    socket: &Arc<UdpSocket>,
    udp: &UdpTuning,
    target: &str,
    flows: &Arc<Mutex<Flows>>,
    open: &Arc<O>,
) where
    O: Fn() -> F + Send + Sync + 'static,
    F: Future<Output = io::Result<Channel>> + Send + 'static,
{
    if matches!(table.map.get(&client), Some(FlowState::Active { .. })) {
        table.active -= 1;
    }
    let queue = FlowQueue::new(udp.flow_queue);
    let _ = queue.push(packet);
    let rx = FlowReceiver(queue.clone());
    table.next_id += 1;
    let id = table.next_id;
    table.map.insert(client, FlowState::Active { queue, id });
    table.active += 1;

    let (socket, flows, open) = (socket.clone(), flows.clone(), open.clone());
    let (idle, target) = (udp.timeout, target.to_owned());
    tokio::spawn(async move {
        debug!(%client, %target, "UDP flow opened");
        let mut opened = false;
        let result = async {
            // Boxed: opening (dial, handshake, waiting for a session) needs far more state
            // than relaying, and an unboxed future would reserve that for the flow's life.
            let channel = Box::pin(open()).await?;
            opened = true;
            relay(channel, rx, ReplyTo { socket, client }, idle).await
        }
        .await;
        let open_failed = match &result {
            Err(e) if !opened => {
                warn!(%client, %target, error = %e, "could not open a UDP flow");
                true
            }
            // Rejected by the exit side right after an optimistic (mux) open.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionRefused | io::ErrorKind::Unsupported
                ) =>
            {
                warn!(%client, %target, error = %e, "UDP flow rejected by the exit side");
                true
            }
            Err(e) => {
                debug!(%client, %target, error = %e, "UDP flow ended with error");
                false
            }
            Ok(()) => {
                debug!(%client, %target, "UDP flow closed");
                false
            }
        };
        flows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .finish(client, id, open_failed);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn tuning(max_flows: usize) -> UdpTuning {
        UdpTuning {
            timeout: Duration::from_secs(60),
            max_flows,
            socket_buffer: 1 << 20,
            flow_queue: 16,
            session_buffer: 64 * 1024,
        }
    }

    /// Serves a rule whose opener only counts calls and fails (or never finishes).
    async fn serve_counting(max_flows: usize, fail: bool) -> (SocketAddr, Arc<AtomicUsize>) {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let opener = move || {
            c.fetch_add(1, Ordering::SeqCst);
            async move {
                if fail {
                    Err(io::Error::other("exit side unreachable"))
                } else {
                    std::future::pending::<io::Result<Channel>>().await
                }
            }
        };
        tokio::spawn(serve(socket, tuning(max_flows), "target".into(), opener));
        (addr, calls)
    }

    async fn client(to: SocketAddr) -> UdpSocket {
        let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        s.connect(to).await.unwrap();
        s
    }

    #[tokio::test]
    async fn packets_of_one_client_share_a_flow() {
        let (addr, calls) = serve_counting(10, false).await;
        let c = client(addr).await;
        for _ in 0..20 {
            c.send(b"x").await.unwrap();
        }
        sleep(Duration::from_millis(200)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_opens_back_off() {
        let (addr, calls) = serve_counting(10, true).await;
        let c = client(addr).await;
        for _ in 0..20 {
            c.send(b"x").await.unwrap();
            sleep(Duration::from_millis(20)).await;
        }
        // One attempt, then the client is ignored for FAILED_BACKOFF.
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn clients_beyond_max_flows_are_dropped() {
        let (addr, calls) = serve_counting(2, false).await;
        let mut clients = Vec::new();
        for _ in 0..5 {
            let c = client(addr).await;
            c.send(b"x").await.unwrap();
            clients.push(c);
        }
        sleep(Duration::from_millis(200)).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
