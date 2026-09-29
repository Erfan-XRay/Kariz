//! Live counters of a running side, answered by `status` on the control socket
//! (docs/PHASE10.md, docs/status.md).
//!
//! Everything is updated where the work already happens, with relaxed atomics; nothing
//! runs in the background. Reading them ([`Stats::snapshot`]) never touches the tunnel.

use std::fmt::Display;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::{mode_name, role_name, Config};
use crate::session::Session;

/// Version of the `status` document; readers ignore fields they do not know.
pub const STATUS_VERSION: u32 = 1;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Bytes moved in each direction. `up`: from users towards the targets; `down`: back.
#[derive(Debug, Default)]
pub struct Traffic {
    pub up: AtomicU64,
    pub down: AtomicU64,
}

impl Traffic {
    pub fn add_up(&self, n: usize) {
        self.up.fetch_add(n as u64, Relaxed);
    }

    pub fn add_down(&self, n: usize) {
        self.down.fetch_add(n as u64, Relaxed);
    }
}

/// Counters of one `[[forward]]` rule (entry side).
#[derive(Debug)]
pub struct ForwardStats {
    listen: String,
    target: String,
    protocol: &'static str,
    pub traffic: Arc<Traffic>,
    tcp_open: AtomicU64,
    tcp_total: AtomicU64,
    udp_open: AtomicU64,
    udp_total: AtomicU64,
    open_failures: AtomicU64,
}

impl ForwardStats {
    pub fn new(listen: &str, target: &str, protocol: &'static str) -> Arc<Self> {
        Arc::new(Self {
            listen: listen.to_string(),
            target: target.to_string(),
            protocol,
            traffic: Arc::default(),
            tcp_open: AtomicU64::new(0),
            tcp_total: AtomicU64::new(0),
            udp_open: AtomicU64::new(0),
            udp_total: AtomicU64::new(0),
            open_failures: AtomicU64::new(0),
        })
    }

    /// Counts a user connection as open until the guard is dropped.
    pub fn tcp_connection(self: &Arc<Self>) -> OpenGuard {
        self.tcp_total.fetch_add(1, Relaxed);
        self.tcp_open.fetch_add(1, Relaxed);
        OpenGuard(self.clone(), Kind::Tcp)
    }

    /// Counts a UDP flow as open until the guard is dropped.
    pub fn udp_flow(self: &Arc<Self>) -> OpenGuard {
        self.udp_total.fetch_add(1, Relaxed);
        self.udp_open.fetch_add(1, Relaxed);
        OpenGuard(self.clone(), Kind::Udp)
    }

    /// A channel for a user could not be opened, or the exit side refused it.
    pub fn open_failed(&self) {
        self.open_failures.fetch_add(1, Relaxed);
    }
}

#[derive(Debug)]
enum Kind {
    Tcp,
    Udp,
}

/// An open user connection or UDP flow; counted down when dropped.
#[derive(Debug)]
pub struct OpenGuard(Arc<ForwardStats>, Kind);

impl Drop for OpenGuard {
    fn drop(&mut self) {
        match self.1 {
            Kind::Tcp => self.0.tcp_open.fetch_sub(1, Relaxed),
            Kind::Udp => self.0.udp_open.fetch_sub(1, Relaxed),
        };
    }
}

/// Bytes of one UDP flow in one direction, added to a shared counter every
/// [`Tally::BATCH`] packets, at least once a second while packets come, and when dropped,
/// so that busy flows on many cores do not all write the same counter for every packet.
pub struct Tally<'a> {
    target: &'a AtomicU64,
    bytes: u64,
    packets: u32,
    flushed: Instant,
    /// Nothing counted yet: the first packet is added at once.
    fresh: bool,
}

impl<'a> Tally<'a> {
    const BATCH: u32 = 64;
    /// The clock is read every this many packets, not for each.
    const CLOCK_EVERY: u32 = 8;

    pub fn new(target: &'a AtomicU64) -> Self {
        Self {
            target,
            bytes: 0,
            packets: 0,
            flushed: Instant::now(),
            fresh: true,
        }
    }

    pub fn add(&mut self, n: usize) {
        self.bytes += n as u64;
        self.packets += 1;
        // The first packet shows at once, so short flows (a DNS query) are seen too.
        if self.packets >= Self::BATCH
            || std::mem::take(&mut self.fresh)
            || (self.packets % Self::CLOCK_EVERY == 0
                && self.flushed.elapsed() >= Duration::from_secs(1))
        {
            self.flush();
        }
    }

    pub fn flush(&mut self) {
        if self.bytes > 0 {
            self.target.fetch_add(self.bytes, Relaxed);
        }
        self.bytes = 0;
        self.packets = 0;
        self.flushed = Instant::now();
    }
}

impl Drop for Tally<'_> {
    fn drop(&mut self) {
        self.flush();
    }
}

/// The link to the other side: its sessions, handshakes and the last error.
#[derive(Debug, Default)]
pub struct Peer {
    /// Sessions the dialing side keeps up (`mux.connections`); 0 without mux.
    wanted: u64,
    /// Sessions seen come up, with when; the closed ones are pruned when read.
    live: Mutex<Vec<(Instant, Weak<Session>)>>,
    handshakes_ok: AtomicU64,
    handshakes_failed: AtomicU64,
    last_ok: Mutex<Option<Instant>>,
    last_error: Mutex<Option<(Instant, String)>>,
}

impl Peer {
    /// A session to the other side is up (the handshake succeeded).
    pub fn session_up(&self, session: &Arc<Session>) {
        self.link_up();
        let mut live = lock(&self.live);
        live.retain(|(_, s)| s.upgrade().is_some_and(|s| !s.is_closed()));
        live.push((Instant::now(), Arc::downgrade(session)));
    }

    /// A tunnel connection without mux passed its handshake.
    pub fn link_up(&self) {
        self.handshakes_ok.fetch_add(1, Relaxed);
        *lock(&self.last_ok) = Some(Instant::now());
    }

    /// Connecting to or authenticating the other side failed.
    pub fn failed(&self, error: &dyn Display) {
        self.handshakes_failed.fetch_add(1, Relaxed);
        *lock(&self.last_error) = Some((Instant::now(), error.to_string()));
    }

    fn snapshot(&self, mux: bool) -> PeerStatus {
        let now = Instant::now();
        let mut live = lock(&self.live);
        live.retain(|(_, s)| s.upgrade().is_some_and(|s| !s.is_closed()));
        let sessions: Vec<(Instant, Arc<Session>)> = live
            .iter()
            .filter_map(|(since, s)| Some((*since, s.upgrade()?)))
            .collect();
        drop(live);
        let last_error = lock(&self.last_error).clone();
        let last_ok = *lock(&self.last_ok);
        let (connected, connected_since) = if mux {
            let since = sessions.iter().map(|(t, _)| *t).min();
            (!sessions.is_empty(), since)
        } else {
            // Without sessions, "connected" means the last attempt worked.
            let ok = last_ok.is_some_and(|ok| last_error.as_ref().map_or(true, |(e, _)| ok > *e));
            (ok, if ok { last_ok } else { None })
        };
        let rtts: Vec<Duration> = sessions.iter().filter_map(|(_, s)| s.rtt()).collect();
        let rtt_ms = (!rtts.is_empty()).then(|| {
            let mean = rtts.iter().sum::<Duration>() / rtts.len() as u32;
            (mean.as_secs_f64() * 10_000.0).round() / 10.0
        });
        PeerStatus {
            connected,
            sessions: mux.then_some(sessions.len() as u64),
            sessions_wanted: (mux && self.wanted > 0).then_some(self.wanted),
            connected_secs: connected_since.map(|t| now.duration_since(t).as_secs()),
            rtt_ms,
            handshakes_ok: self.handshakes_ok.load(Relaxed),
            handshakes_failed: self.handshakes_failed.load(Relaxed),
            last_error: last_error.map(|(at, text)| LastError {
                secs_ago: now.duration_since(at).as_secs(),
                text,
            }),
        }
    }
}

/// What the exit side counts about the targets it reaches.
#[derive(Debug, Default)]
pub struct Targets {
    streams_total: AtomicU64,
    dial_failures: AtomicU64,
    last_dial_error: Mutex<Option<(Instant, String)>>,
    pub traffic: Arc<Traffic>,
}

impl Targets {
    /// A stream (a user connection or a UDP flow) arrived to be served.
    pub fn stream(&self) {
        self.streams_total.fetch_add(1, Relaxed);
    }

    pub fn dial_failed(&self, target: &str, error: &dyn Display) {
        self.dial_failures.fetch_add(1, Relaxed);
        *lock(&self.last_dial_error) = Some((Instant::now(), format!("{target}: {error}")));
    }
}

/// The counters of one running side.
#[derive(Debug)]
pub struct Stats {
    started: Instant,
    role: &'static str,
    mode: &'static str,
    transport: &'static str,
    profile: &'static str,
    mux: bool,
    pub peer: Arc<Peer>,
    /// One per `[[forward]]` rule, in config order (entry side).
    pub forwards: Vec<Arc<ForwardStats>>,
    /// Exit side.
    pub targets: Targets,
}

impl Stats {
    pub fn new(config: &Config) -> Arc<Self> {
        let mux = config.mux();
        let wanted = if mux.enabled && !config.is_acceptor() {
            mux.connections as u64
        } else {
            0
        };
        Arc::new(Self {
            started: Instant::now(),
            role: role_name(config.role),
            mode: mode_name(config.mode),
            transport: config.tunnel.transport.name(),
            profile: config.profile.name(),
            mux: mux.enabled,
            peer: Arc::new(Peer {
                wanted,
                ..Peer::default()
            }),
            forwards: config
                .forward
                .iter()
                .map(|f| ForwardStats::new(&f.listen, &f.target, f.protocol.name()))
                .collect(),
            targets: Targets::default(),
        })
    }

    /// The `status` document.
    pub fn snapshot(&self) -> Status {
        let forwards: Vec<ForwardStatus> = self
            .forwards
            .iter()
            .map(|f| ForwardStatus {
                listen: f.listen.clone(),
                target: f.target.clone(),
                protocol: f.protocol.to_string(),
                tcp_open: f.tcp_open.load(Relaxed),
                tcp_total: f.tcp_total.load(Relaxed),
                udp_flows: f.udp_open.load(Relaxed),
                udp_total: f.udp_total.load(Relaxed),
                bytes_up: f.traffic.up.load(Relaxed),
                bytes_down: f.traffic.down.load(Relaxed),
                open_failures: f.open_failures.load(Relaxed),
            })
            .collect();
        let mut totals = Totals::default();
        for f in &forwards {
            totals.bytes_up += f.bytes_up;
            totals.bytes_down += f.bytes_down;
            totals.tcp_open += f.tcp_open;
            totals.tcp_total += f.tcp_total;
            totals.udp_flows += f.udp_flows;
            totals.udp_total += f.udp_total;
        }
        let exit = (self.role == "exit").then(|| {
            let t = &self.targets;
            totals.bytes_up = t.traffic.up.load(Relaxed);
            totals.bytes_down = t.traffic.down.load(Relaxed);
            ExitStatus {
                streams_total: t.streams_total.load(Relaxed),
                dial_failures: t.dial_failures.load(Relaxed),
                last_dial_error: lock(&t.last_dial_error)
                    .clone()
                    .map(|(at, text)| LastError {
                        secs_ago: at.elapsed().as_secs(),
                        text,
                    }),
            }
        });
        Status {
            status_version: STATUS_VERSION,
            kariz: env!("CARGO_PKG_VERSION").to_string(),
            role: self.role.to_string(),
            mode: self.mode.to_string(),
            transport: self.transport.to_string(),
            profile: self.profile.to_string(),
            uptime_secs: self.started.elapsed().as_secs(),
            peer: self.peer.snapshot(self.mux),
            totals,
            forwards,
            exit,
        }
    }
}

/// The `status` document (docs/status.md).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub status_version: u32,
    pub kariz: String,
    pub role: String,
    pub mode: String,
    pub transport: String,
    pub profile: String,
    pub uptime_secs: u64,
    pub peer: PeerStatus,
    pub totals: Totals,
    #[serde(default)]
    pub forwards: Vec<ForwardStatus>,
    /// Exit side only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<ExitStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeerStatus {
    pub connected: bool,
    /// Live sessions; `null` without mux.
    pub sessions: Option<u64>,
    /// Sessions the dialing side keeps up; `null` on the accepting side and without mux.
    pub sessions_wanted: Option<u64>,
    /// Seconds since the current connection came up; `null` while down.
    pub connected_secs: Option<u64>,
    /// Round-trip time to the other side, from mux pings or QUIC; `null` when unknown.
    pub rtt_ms: Option<f64>,
    pub handshakes_ok: u64,
    pub handshakes_failed: u64,
    pub last_error: Option<LastError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LastError {
    pub secs_ago: u64,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Totals {
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub tcp_open: u64,
    pub tcp_total: u64,
    pub udp_flows: u64,
    pub udp_total: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForwardStatus {
    pub listen: String,
    pub target: String,
    pub protocol: String,
    pub tcp_open: u64,
    pub tcp_total: u64,
    pub udp_flows: u64,
    pub udp_total: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub open_failures: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExitStatus {
    pub streams_total: u64,
    pub dial_failures: u64,
    pub last_dial_error: Option<LastError>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRY: &str = r#"
        role = "entry"
        mode = "reverse"
        [tunnel]
        transport = "tcpmux"
        listen = "0.0.0.0:3080"
        token = "0123456789abcdef0123"
        [[forward]]
        listen = "0.0.0.0:443"
        target = "127.0.0.1:443"
        [[forward]]
        listen = "0.0.0.0:53"
        target = "127.0.0.1:53"
        protocol = "tcp+udp"
    "#;

    #[test]
    fn forward_counters_add_up() {
        let stats = Stats::new(&Config::parse(ENTRY).unwrap());
        let (web, dns) = (&stats.forwards[0], &stats.forwards[1]);
        let a = web.tcp_connection();
        let b = web.tcp_connection();
        web.traffic.add_up(100);
        web.traffic.add_down(1000);
        drop(a);
        let flow = dns.udp_flow();
        {
            let mut tally = Tally::new(&dns.traffic.down);
            for _ in 0..70 {
                tally.add(10);
            }
            // The first packet at once, then a batch of 64; the rest when dropped.
            assert_eq!(dns.traffic.down.load(Relaxed), 650);
        }
        dns.open_failed();

        let s = stats.snapshot();
        assert_eq!((s.role.as_str(), s.transport.as_str()), ("entry", "tcpmux"));
        assert_eq!((s.forwards[0].tcp_open, s.forwards[0].tcp_total), (1, 2));
        assert_eq!((s.forwards[1].udp_flows, s.forwards[1].udp_total), (1, 1));
        assert_eq!(s.forwards[1].bytes_down, 700);
        assert_eq!(s.forwards[1].open_failures, 1);
        assert_eq!(s.forwards[1].protocol, "tcp+udp");
        assert_eq!(s.totals.bytes_up, 100);
        assert_eq!(s.totals.bytes_down, 1700);
        assert_eq!(s.totals.tcp_open, 1);
        assert!(s.exit.is_none());
        drop((b, flow));
        let s = stats.snapshot();
        assert_eq!((s.totals.tcp_open, s.totals.udp_flows), (0, 0));
    }

    #[test]
    fn the_peer_is_connected_while_a_session_lives() {
        let stats = Stats::new(&Config::parse(ENTRY).unwrap());
        let before = stats.snapshot().peer;
        assert!(!before.connected);
        assert_eq!(before.sessions, Some(0));
        // The accepting side does not choose how many sessions there are.
        assert_eq!(before.sessions_wanted, None);

        stats.peer.failed(&"authentication failed");
        let (a, _b) = tokio::io::duplex(1024);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let session = Arc::new(Session::Kmux(crate::mux::MuxSession::new(
                a,
                crate::mux::Side::Client,
                crate::mux::tests::config(),
            )));
            stats.peer.session_up(&session);
            let p = stats.snapshot().peer;
            assert!(p.connected);
            assert_eq!(
                (p.sessions, p.handshakes_ok, p.handshakes_failed),
                (Some(1), 1, 1)
            );
            assert_eq!(p.last_error.unwrap().text, "authentication failed");
            session.close();
            drop(session);
            let p = stats.snapshot().peer;
            assert!(!p.connected);
            assert_eq!(p.sessions, Some(0));
            assert_eq!(p.connected_secs, None);
        });
    }

    #[test]
    fn without_mux_the_last_attempt_decides() {
        let config = Config::parse(&ENTRY.replace("tcpmux", "tcp")).unwrap();
        let stats = Stats::new(&config);
        stats.peer.link_up();
        let p = stats.snapshot().peer;
        assert!(p.connected);
        assert_eq!((p.sessions, p.rtt_ms), (None, None));
        std::thread::sleep(Duration::from_millis(5));
        stats.peer.failed(&"connection refused");
        assert!(!stats.snapshot().peer.connected);
    }

    #[test]
    fn the_document_round_trips_through_json() {
        let stats = Stats::new(&Config::parse(ENTRY).unwrap());
        let s = stats.snapshot();
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains("\"status_version\":1"), "{text}");
        assert!(!text.contains("\"exit\""), "{text}");
        assert_eq!(serde_json::from_str::<Status>(&text).unwrap(), s);
    }
}
