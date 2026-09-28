//! Configuration file model (TOML) and validation.
//!
//! A tunnel always has two sides:
//!
//! * **entry**: the public-facing server users connect to (usually the Iran server).
//!   It owns the `[[forward]]` rules.
//! * **exit**: the server that reaches the real targets (usually the server abroad).
//!
//! The `mode` decides which side opens the tunnel connections:
//!
//! * `reverse`: exit dials entry (entry listens on `tunnel.listen`).
//! * `direct`: entry dials exit (exit listens on `tunnel.listen`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::proto::Duplicate;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Entry,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Reverse,
    Direct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    #[default]
    Balanced,
    /// The most speed: large buffers and windows, more connections. Also accepted under
    /// its old name, `throughput`.
    #[serde(alias = "throughput")]
    Ultraspeed,
    Gaming,
}

impl Profile {
    pub fn name(self) -> &'static str {
        match self {
            Self::Balanced => "balanced",
            Self::Ultraspeed => "ultraspeed",
            Self::Gaming => "gaming",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportKind {
    #[default]
    Tcp,
    /// `tcp` with mux always on.
    Tcpmux,
    /// WebSocket over plain TCP (for example behind a CDN that talks HTTP to the origin).
    Ws,
    /// WebSocket over TLS.
    Wss,
    /// QUIC: its own streams, datagrams and TLS 1.3 (see docs/PHASE4.md).
    Quic,
    /// KCP over UDP, every packet encrypted with a key from the token (see
    /// docs/PHASE4.md).
    Kcp,
}

impl TransportKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Tcpmux => "tcpmux",
            Self::Ws => "ws",
            Self::Wss => "wss",
            Self::Quic => "quic",
            Self::Kcp => "kcp",
        }
    }

    fn is_websocket(self) -> bool {
        matches!(self, Self::Ws | Self::Wss)
    }
}

/// Encryption of the tunnel traffic, above the transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub enum Encryption {
    /// AES-256-GCM when the CPU has AES instructions, otherwise ChaCha20-Poly1305.
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "chacha20-poly1305")]
    Chacha20Poly1305,
    #[serde(rename = "aes-256-gcm")]
    Aes256Gcm,
    #[serde(rename = "none")]
    None,
}

impl Encryption {
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Chacha20Poly1305 => "chacha20-poly1305",
            Self::Aes256Gcm => "aes-256-gcm",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub enum ForwardProtocol {
    #[default]
    #[serde(rename = "tcp")]
    Tcp,
    #[serde(rename = "udp")]
    Udp,
    /// Both, on the same port.
    #[serde(rename = "tcp+udp")]
    TcpUdp,
}

impl ForwardProtocol {
    pub fn name(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
            Self::TcpUdp => "tcp+udp",
        }
    }

    pub fn has_tcp(self) -> bool {
        matches!(self, Self::Tcp | Self::TcpUdp)
    }

    pub fn has_udp(self) -> bool {
        matches!(self, Self::Udp | Self::TcpUdp)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub role: Role,
    pub mode: Mode,
    #[serde(default)]
    pub profile: Profile,
    pub tunnel: TunnelConfig,
    #[serde(default)]
    pub forward: Vec<Forward>,
    #[serde(default)]
    pub tuning: TuningOverrides,
    #[serde(default)]
    pub log: LogConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunnelConfig {
    #[serde(default)]
    pub transport: TransportKind,
    /// Address to listen on for tunnel connections (entry in reverse mode, exit in direct mode).
    pub listen: Option<String>,
    /// Address of the other side (exit in reverse mode, entry in direct mode).
    pub remote: Option<String>,
    /// Shared secret. Both sides must use the same value.
    pub token: String,
    /// Reverse mode only: idle tunnel connections the exit keeps open towards the entry.
    #[serde(default = "default_pool")]
    pub pool: usize,
    #[serde(default)]
    pub encryption: Encryption,
    #[serde(default)]
    pub mux: MuxConfig,
    /// Only for `ws` and `wss`.
    pub ws: Option<WsConfig>,
    /// Only for `wss`.
    pub tls: Option<TlsConfig>,
    /// Only for `quic`.
    pub quic: Option<QuicConfig>,
    /// Only for `kcp`.
    pub kcp: Option<KcpConfig>,
}

/// `[tunnel.kcp]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KcpConfig {
    #[serde(default)]
    pub mode: KcpMode,
    /// `mode = "manual"` only; unset ones are taken from `fast2`.
    pub nodelay: Option<bool>,
    pub interval_ms: Option<u32>,
    pub resend: Option<u32>,
    pub no_congestion: Option<bool>,
    /// In packets.
    #[serde(default = "default_kcp_window")]
    pub send_window: u16,
    #[serde(default = "default_kcp_window")]
    pub recv_window: u16,
    /// KCP packet size (its 24-byte header included); the UDP packet adds 29 bytes of
    /// protection, 43 with FEC.
    #[serde(default = "default_kcp_mtu")]
    pub mtu: usize,
    /// Reed-Solomon FEC: parity packets per group of data packets; both 0 = off. Unset:
    /// the profile's default (`Config::kcp`).
    pub fec_data: Option<usize>,
    pub fec_parity: Option<usize>,
    /// UDP flows take the datagram path beside KCP (docs/PHASE6.md, section 2). Off:
    /// they stay in the reliable stream, as in v0.4 (never lost, but a lost segment makes
    /// them wait for its retransmission).
    #[serde(default = "default_true")]
    pub datagrams: bool,
}

fn default_true() -> bool {
    true
}

fn default_kcp_window() -> u16 {
    1024
}

fn default_kcp_mtu() -> usize {
    1350
}

/// Same values as a `[tunnel.kcp]` table with nothing in it.
impl Default for KcpConfig {
    fn default() -> Self {
        Self {
            mode: KcpMode::default(),
            nodelay: None,
            interval_ms: None,
            resend: None,
            no_congestion: None,
            send_window: default_kcp_window(),
            recv_window: default_kcp_window(),
            mtu: default_kcp_mtu(),
            fec_data: None,
            fec_parity: None,
            datagrams: true,
        }
    }
}

/// KCP presets, as in kcp-go / kcptun: from gentle to aggressive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KcpMode {
    Normal,
    Fast,
    #[default]
    Fast2,
    Fast3,
    /// `nodelay`, `interval_ms`, `resend` and `no_congestion` set by hand.
    Manual,
}

impl KcpMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Fast => "fast",
            Self::Fast2 => "fast2",
            Self::Fast3 => "fast3",
            Self::Manual => "manual",
        }
    }
}

/// The KCP parameters a mode stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KcpTiming {
    /// Minimum retransmission timeout 30 ms instead of 100 ms, and gentler backoff.
    pub nodelay: bool,
    /// How often KCP flushes: acks and retransmissions wait up to this long.
    pub interval_ms: u32,
    /// Retransmit after this many acks skip a packet (0: only on timeout).
    pub resend: u32,
    /// Send the whole window regardless of loss.
    pub no_congestion: bool,
}

impl KcpConfig {
    /// `(data, parity)` packets per FEC group, or `None` when FEC is off.
    pub fn fec(&self) -> Option<(usize, usize)> {
        let (data, parity) = (self.fec_data.unwrap_or(0), self.fec_parity.unwrap_or(0));
        (data > 0).then_some((data, parity))
    }

    pub fn timing(&self) -> KcpTiming {
        let preset = |nodelay, interval_ms, resend, no_congestion| KcpTiming {
            nodelay,
            interval_ms,
            resend,
            no_congestion,
        };
        match self.mode {
            KcpMode::Normal => preset(false, 40, 2, true),
            KcpMode::Fast => preset(false, 30, 2, true),
            KcpMode::Fast2 => preset(true, 20, 2, true),
            KcpMode::Fast3 => preset(true, 10, 2, true),
            KcpMode::Manual => {
                let fast2 = KcpConfig::default().timing();
                preset(
                    self.nodelay.unwrap_or(fast2.nodelay),
                    self.interval_ms.unwrap_or(fast2.interval_ms),
                    self.resend.unwrap_or(fast2.resend),
                    self.no_congestion.unwrap_or(fast2.no_congestion),
                )
            }
        }
    }
}

/// `[tunnel.quic]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuicConfig {
    #[serde(default)]
    pub congestion: Congestion,
    /// Dialing side: server name in the handshake. Default: the host of `remote` when it
    /// is a name, none when it is an IP address (as browsers do).
    pub sni: Option<String>,
    /// Application protocol announced in the handshake (both sides must agree).
    #[serde(default = "default_alpn")]
    pub alpn: String,
}

fn default_alpn() -> String {
    "h3".into()
}

/// Same values as a `[tunnel.quic]` table with nothing in it.
impl Default for QuicConfig {
    fn default() -> Self {
        Self {
            congestion: Congestion::default(),
            sni: None,
            alpn: default_alpn(),
        }
    }
}

/// QUIC congestion controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Congestion {
    #[default]
    Cubic,
    /// Copes much better with random loss; experimental in quinn.
    Bbr,
    NewReno,
}

impl Congestion {
    pub fn name(self) -> &'static str {
        match self {
            Self::Cubic => "cubic",
            Self::Bbr => "bbr",
            Self::NewReno => "newreno",
        }
    }
}

/// `[tunnel.mux]`: many user connections over a few long-lived tunnel connections.
/// Unset values come from the profile, see [`MuxSettings`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MuxConfig {
    /// Default: on for `tcpmux`, `ws` and `wss`, off for `tcp`.
    pub enabled: Option<bool>,
    /// Long-lived tunnel connections to keep open.
    pub connections: Option<usize>,
    /// Maximum concurrent streams per tunnel connection.
    pub max_streams: Option<usize>,
    /// Per-stream flow-control window in bytes.
    pub stream_window: Option<usize>,
    /// Replace each tunnel connection after this many seconds.
    pub max_lifetime_secs: Option<u64>,
    /// Gather queued frames into larger writes (throughput) or write each at once
    /// (latency). Default: on, off in the gaming profile.
    pub coalesce: Option<bool>,
    /// Seconds between pings; a peer silent for twice as long is dead. Default:
    /// `tuning.keepalive_secs`.
    pub ping_interval_secs: Option<u64>,
    /// Bytes of UDP datagrams a session queues for sending before dropping. Default: the
    /// profile's.
    pub datagram_buffer: Option<usize>,
    /// UDP datagrams each flow queues on the receiving side before dropping the oldest.
    /// Default: the profile's.
    pub datagram_queue: Option<usize>,
    /// `TCP_NOTSENT_LOWAT` in bytes on TCP connections that carry mux; 0 turns it off.
    /// Default: 16384.
    pub notsent_lowat: Option<u32>,
}

/// `[tunnel.ws]`: WebSocket upgrade request.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WsConfig {
    #[serde(default = "default_ws_path")]
    pub path: String,
    /// `Host` header. Dialing side: sent instead of the `remote` host (lets `remote` be a
    /// CDN IP). Listening side: when set, requests for another host are rejected.
    pub host: Option<String>,
    /// Dialing side only.
    pub user_agent: Option<String>,
    /// Dialing side only: extra request headers.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Dialing side only: send the tunnel hello inside the upgrade request (in
    /// `Sec-WebSocket-Protocol`), saving a round trip. The listening side always
    /// accepts it.
    #[serde(default)]
    pub early_data: bool,
}

fn default_ws_path() -> String {
    "/".into()
}

/// `[tunnel.tls]`: TLS settings for `wss`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TlsConfig {
    /// Dialing side: server name to present, instead of the `remote` host.
    pub sni: Option<String>,
    /// Dialing side: accept only this certificate (hex SHA-256 of its DER encoding).
    pub pin_sha256: Option<String>,
    /// Dialing side: skip certificate verification. Unsafe; prefer `pin_sha256`.
    #[serde(default)]
    pub insecure: bool,
    /// Listening side: certificate chain (PEM).
    pub cert: Option<PathBuf>,
    /// Listening side: private key (PEM).
    pub key: Option<PathBuf>,
}

fn default_pool() -> usize {
    8
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Forward {
    /// Address users connect to on the entry side, e.g. `0.0.0.0:443`.
    pub listen: String,
    /// Address the exit side connects to, e.g. `127.0.0.1:443`.
    pub target: String,
    #[serde(default)]
    pub protocol: ForwardProtocol,
    /// UDP rules: how many times each packet is sent in all, both ways (1-3; default 1).
    /// Copies go only where packets may be lost (KCP's datagram path, QUIC datagrams).
    pub duplicate: Option<u8>,
    /// Milliseconds between the copies of a packet (0-50; default 5).
    pub duplicate_gap_ms: Option<u8>,
}

impl Forward {
    /// Packet duplication for this rule's UDP flows; `None` with a single copy.
    pub fn duplication(&self) -> Option<Duplicate> {
        let copies = self.duplicate.unwrap_or(1);
        (copies > 1).then(|| Duplicate {
            copies,
            gap_ms: self.duplicate_gap_ms.unwrap_or(DEFAULT_DUPLICATE_GAP_MS),
        })
    }
}

const DEFAULT_DUPLICATE_GAP_MS: u8 = 5;
const DUPLICATE_COPIES: std::ops::RangeInclusive<u8> = 1..=3;
const MAX_DUPLICATE_GAP_MS: u8 = 50;

/// Optional per-field overrides applied on top of the selected profile.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TuningOverrides {
    pub nodelay: Option<bool>,
    pub buffer_size: Option<usize>,
    pub keepalive_secs: Option<u64>,
    pub dial_timeout_secs: Option<u64>,
    pub handshake_timeout_secs: Option<u64>,
    /// Number of worker threads. Defaults to the number of CPU cores.
    pub threads: Option<usize>,
    /// Idle time after which a UDP flow is closed.
    pub udp_timeout_secs: Option<u64>,
    /// Concurrent UDP flows (client addresses) per UDP forward rule.
    pub udp_max_flows: Option<usize>,
    /// DSCP mark for tunnel sockets and the exit's UDP sockets to targets (unset: none).
    pub dscp: Option<Dscp>,
}

/// A DSCP codepoint as written in `[tuning] dscp`: a name (`"ef"`, `"af41"`, `"cs4"`,
/// ...) or a number 0-63.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Dscp {
    Name(String),
    Value(u64),
}

impl Dscp {
    /// The 6-bit codepoint; `None` for an unknown name or a number above 63.
    pub fn codepoint(&self) -> Option<u8> {
        match self {
            Self::Value(v) => u8::try_from(*v).ok().filter(|v| *v < 64),
            Self::Name(name) => DSCP_NAMES
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|&(_, v)| v),
        }
    }

    /// The name of `codepoint`, if it has one.
    pub fn name_of(codepoint: u8) -> Option<&'static str> {
        DSCP_NAMES
            .iter()
            .find(|&&(_, v)| v == codepoint)
            .map(|&(n, _)| n)
    }
}

/// Codepoint names of RFC 2474 (class selectors), RFC 2597 (assured forwarding), RFC
/// 3246 (expedited forwarding), RFC 5865 (voice admit) and RFC 8622 (lower effort).
const DSCP_NAMES: [(&str, u8); 24] = [
    ("cs0", 0),
    ("le", 1),
    ("cs1", 8),
    ("af11", 10),
    ("af12", 12),
    ("af13", 14),
    ("cs2", 16),
    ("af21", 18),
    ("af22", 20),
    ("af23", 22),
    ("cs3", 24),
    ("af31", 26),
    ("af32", 28),
    ("af33", 30),
    ("cs4", 32),
    ("af41", 34),
    ("af42", 36),
    ("af43", 38),
    ("cs5", 40),
    ("va", 44),
    ("ef", 46),
    ("cs6", 48),
    ("cs7", 56),
    ("default", 0),
];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    /// Colours and the startup banner's art: on a terminal (`auto`), always or never.
    #[serde(default)]
    pub color: LogColor,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            color: LogColor::default(),
        }
    }
}

/// `[log] color`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogColor {
    /// Colours on a terminal, unless `NO_COLOR` is set.
    #[default]
    Auto,
    Always,
    Never,
}

fn default_log_level() -> String {
    "info".into()
}

/// Effective tuning values after applying profile defaults and overrides.
#[derive(Debug, Clone)]
pub struct Tuning {
    pub nodelay: bool,
    pub buffer_size: usize,
    pub keepalive: Duration,
    pub dial_timeout: Duration,
    pub handshake_timeout: Duration,
    pub threads: Option<usize>,
    pub udp: UdpTuning,
    /// `TCP_NOTSENT_LOWAT` for tunnel connections, set when they carry mux. It keeps the
    /// kernel from queueing much unsent data, so what is sent next is decided by the mux
    /// writer (datagrams first, streams in turn) rather than by the order data entered
    /// the socket. Without it, a UDP packet waits behind everything already queued in the
    /// kernel on a slow link.
    pub notsent_lowat: Option<u32>,
    /// DSCP codepoint for tunnel sockets and the exit's UDP sockets to targets
    /// (docs/PHASE6.md, section 5); `None` leaves the OS default (0).
    pub dscp: Option<u8>,
}

/// UDP forwarding limits and buffers (phase 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpTuning {
    /// A flow with no packet in either direction for this long is closed.
    pub timeout: Duration,
    /// Concurrent flows per UDP forward rule; packets from further clients are dropped.
    pub max_flows: usize,
    /// `SO_RCVBUF` / `SO_SNDBUF` of UDP sockets.
    pub socket_buffer: usize,
    /// Packets queued per flow and direction before dropping.
    pub flow_queue: usize,
    /// Datagram bytes a mux session queues for sending before dropping.
    pub session_buffer: usize,
}

impl UdpTuning {
    fn for_profile(profile: Profile) -> Self {
        let (socket_buffer, flow_queue, session_buffer) = match profile {
            Profile::Balanced => (1 << 20, 128, 256 * 1024),
            Profile::Ultraspeed => (4 << 20, 512, 1 << 20),
            Profile::Gaming => (1 << 20, 64, 128 * 1024),
        };
        Self {
            timeout: Duration::from_secs(60),
            max_flows: 1024,
            socket_buffer,
            flow_queue,
            session_buffer,
        }
    }
}

impl Tuning {
    pub fn for_profile(profile: Profile) -> Self {
        let buffer_size = match profile {
            Profile::Balanced => 64 * 1024,
            Profile::Ultraspeed => 256 * 1024,
            Profile::Gaming => 16 * 1024,
        };
        Self {
            nodelay: true,
            buffer_size,
            keepalive: Duration::from_secs(if profile == Profile::Gaming { 10 } else { 30 }),
            dial_timeout: Duration::from_secs(10),
            handshake_timeout: Duration::from_secs(10),
            threads: None,
            udp: UdpTuning::for_profile(profile),
            notsent_lowat: None,
            dscp: None,
        }
    }

    fn apply(mut self, o: &TuningOverrides) -> Self {
        if let Some(v) = o.nodelay {
            self.nodelay = v;
        }
        if let Some(v) = o.buffer_size {
            self.buffer_size = v;
        }
        if let Some(v) = o.keepalive_secs {
            self.keepalive = Duration::from_secs(v);
        }
        if let Some(v) = o.dial_timeout_secs {
            self.dial_timeout = Duration::from_secs(v);
        }
        if let Some(v) = o.handshake_timeout_secs {
            self.handshake_timeout = Duration::from_secs(v);
        }
        if o.threads.is_some() {
            self.threads = o.threads;
        }
        if let Some(v) = o.udp_timeout_secs {
            self.udp.timeout = Duration::from_secs(v);
        }
        if let Some(v) = o.udp_max_flows {
            self.udp.max_flows = v;
        }
        // An invalid value is reported by `Config::validate`.
        if let Some(d) = &o.dscp {
            self.dscp = d.codepoint();
        }
        self
    }
}

/// Effective mux values after applying profile defaults and `[tunnel.mux]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuxSettings {
    pub enabled: bool,
    pub connections: usize,
    pub max_streams: usize,
    pub stream_window: usize,
    pub max_lifetime: Option<Duration>,
    /// Coalesce queued frames into one record before writing; off means flush every frame.
    pub coalesce: bool,
    /// Ping interval (kmux pings, QUIC keep-alives); a peer silent for twice as long is
    /// dead.
    pub ping_interval: Duration,
    /// UDP datagram bytes a session queues for sending.
    pub datagram_buffer: usize,
    /// UDP datagrams each flow queues on the receiving side.
    pub datagram_queue: usize,
    /// `TCP_NOTSENT_LOWAT` for TCP connections carrying mux (see
    /// [`Tuning::notsent_lowat`]); `None`: off.
    pub notsent_lowat: Option<u32>,
}

impl MuxSettings {
    /// `tuning`: the profile's tuning with `[tuning]` applied, for the values mux takes
    /// from it by default.
    fn for_profile(profile: Profile, transport: TransportKind, tuning: &Tuning) -> Self {
        let (stream_window, connections) = match profile {
            Profile::Balanced => (256 * 1024, 4),
            Profile::Ultraspeed => (1024 * 1024, 8),
            Profile::Gaming => (64 * 1024, 2),
        };
        // QUIC streams do not block each other, so more connections only buy more
        // congestion windows; two keep a spare while one reconnects.
        let connections = if transport == TransportKind::Quic {
            2
        } else {
            connections
        };
        Self {
            enabled: transport != TransportKind::Tcp,
            connections,
            max_streams: 512,
            stream_window,
            max_lifetime: None,
            coalesce: profile != Profile::Gaming,
            ping_interval: tuning.keepalive,
            datagram_buffer: tuning.udp.session_buffer,
            datagram_queue: tuning.udp.flow_queue,
            notsent_lowat: Some(NOTSENT_LOWAT),
        }
    }

    fn apply(mut self, o: &MuxConfig) -> Self {
        if let Some(v) = o.enabled {
            self.enabled = v;
        }
        if let Some(v) = o.connections {
            self.connections = v;
        }
        if let Some(v) = o.max_streams {
            self.max_streams = v;
        }
        if let Some(v) = o.stream_window {
            self.stream_window = v;
        }
        if let Some(v) = o.max_lifetime_secs {
            self.max_lifetime = Some(Duration::from_secs(v));
        }
        if let Some(v) = o.coalesce {
            self.coalesce = v;
        }
        if let Some(v) = o.ping_interval_secs {
            self.ping_interval = Duration::from_secs(v);
        }
        if let Some(v) = o.datagram_buffer {
            self.datagram_buffer = v;
        }
        if let Some(v) = o.datagram_queue {
            self.datagram_queue = v;
        }
        if let Some(v) = o.notsent_lowat {
            self.notsent_lowat = (v > 0).then_some(v);
        }
        self
    }
}

const MUX_CONNECTIONS: std::ops::RangeInclusive<usize> = 1..=64;
const MUX_MAX_STREAMS: std::ops::RangeInclusive<usize> = 1..=4096;
const MUX_STREAM_WINDOW: std::ops::RangeInclusive<usize> = 16 * 1024..=16 * 1024 * 1024;
const KCP_INTERVAL_MS: std::ops::RangeInclusive<u32> = 10..=1000;
const KCP_MAX_RESEND: u32 = 10;
const KCP_SEND_WINDOW: std::ops::RangeInclusive<u16> = 16..=32768;
/// KCP needs 128 to receive its largest messages.
const KCP_RECV_WINDOW: std::ops::RangeInclusive<u16> = 128..=32768;
/// The upper bound keeps the UDP packet (29 bytes of protection added) within 1472 bytes,
/// the most a 1500-byte path carries unfragmented.
const KCP_MTU: std::ops::RangeInclusive<usize> = 576..=1443;
/// With FEC the largest packet is a parity shard: 43 bytes added.
const KCP_MTU_FEC: usize = 1429;
const KCP_FEC_DATA: std::ops::RangeInclusive<usize> = 1..=64;
const KCP_FEC_PARITY: std::ops::RangeInclusive<usize> = 1..=32;
const MUX_MIN_LIFETIME_SECS: u64 = 60;
const MUX_PING_INTERVAL_SECS: std::ops::RangeInclusive<u64> = 1..=600;
const MUX_DATAGRAM_BUFFER: std::ops::RangeInclusive<usize> = 16 * 1024..=64 * 1024 * 1024;
const MUX_DATAGRAM_QUEUE: std::ops::RangeInclusive<usize> = 8..=65536;
const MUX_NOTSENT_LOWAT: std::ops::RangeInclusive<u32> = 4 * 1024..=16 * 1024 * 1024;
/// See [`Tuning::notsent_lowat`]. Over a throttled 20 Mbit/s link with four bulk
/// transfers next to UDP pings (`udp_latency_under_load`), it cut the UDP round trip
/// from about 340 ms to about 60 ms (the rest is the link's own buffer), with no
/// throughput cost on localhost. 16 KiB is also what large HTTP/2 deployments use.
const NOTSENT_LOWAT: u32 = 16 * 1024;
const UDP_TIMEOUT_SECS: std::ops::RangeInclusive<u64> = 5..=3600;
const UDP_MAX_FLOWS: std::ops::RangeInclusive<usize> = 1..=65536;
/// Keepalives (mux pings) at most this far apart keep a WebSocket through a CDN.
const CDN_IDLE_SAFE: Duration = Duration::from_secs(90);

/// Request headers the WebSocket dialer sets itself.
const RESERVED_WS_HEADERS: &[&str] = &[
    "host",
    "upgrade",
    "connection",
    "content-length",
    "transfer-encoding",
    "sec-websocket-key",
    "sec-websocket-version",
    "sec-websocket-accept",
    "sec-websocket-protocol",
    "sec-websocket-extensions",
];

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("invalid config file {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let config: Config = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    /// Whether this side accepts tunnel connections (as opposed to dialing them).
    pub fn is_acceptor(&self) -> bool {
        matches!(
            (self.role, self.mode),
            (Role::Entry, Mode::Reverse) | (Role::Exit, Mode::Direct)
        )
    }

    pub fn tuning(&self) -> Tuning {
        let mut tuning = Tuning::for_profile(self.profile).apply(&self.tuning);
        let mux = self.mux();
        if mux.enabled {
            tuning.notsent_lowat = mux.notsent_lowat;
        }
        tuning
    }

    pub fn mux(&self) -> MuxSettings {
        let tuning = Tuning::for_profile(self.profile).apply(&self.tuning);
        MuxSettings::for_profile(self.profile, self.tunnel.transport, &tuning)
            .apply(&self.tunnel.mux)
    }

    /// Settings that are valid but weaken the tunnel; shown by `kariz check` and at startup.
    pub fn warnings(&self) -> Vec<&'static str> {
        let mut warnings = Vec::new();
        if self.tunnel.encryption == Encryption::None {
            warnings
                .push("tunnel.encryption = \"none\": traffic between the servers is not encrypted");
        }
        let mux = self.mux();
        let ping = if mux.enabled {
            mux.ping_interval
        } else {
            self.tuning().keepalive
        };
        if self.tunnel.transport.is_websocket() && ping > CDN_IDLE_SAFE {
            warnings.push(
                "the ping interval (tunnel.mux.ping_interval_secs, or tuning.keepalive_secs) \
                 is above 90 s: CDNs close WebSocket connections that are idle for about \
                 100 s (Cloudflare)",
            );
        }
        if self.forward.iter().any(|f| f.protocol.has_udp()) && !self.mux().enabled {
            warnings.push(
                "UDP is forwarded without mux: every UDP flow uses its own tunnel connection; \
                 enable tunnel.mux (or use tcpmux, ws, wss) for UDP",
            );
        }
        let unreliable = matches!(
            self.tunnel.transport,
            TransportKind::Kcp | TransportKind::Quic
        ) && self.mux().enabled;
        if self.forward.iter().any(|f| f.duplication().is_some()) && !unreliable {
            warnings.push(
                "forward.duplicate has no effect here: copies are only sent over kcp (with mux) \
                 or quic, where packets may be lost; other transports deliver every packet",
            );
        }
        if self
            .tunnel
            .quic
            .as_ref()
            .is_some_and(|q| q.congestion == Congestion::Bbr)
        {
            warnings.push("tunnel.quic.congestion = \"bbr\": quinn marks its BBR as experimental");
        }
        if self.tunnel.tls.as_ref().is_some_and(|t| t.insecure) {
            warnings.push(
                "tunnel.tls.insecure = true: the server certificate is not verified; \
                 prefer tls.pin_sha256",
            );
        }
        if self.tuning().dscp.is_some() {
            if self.tunnel.transport == TransportKind::Quic {
                warnings.push(
                    "tuning.dscp does not mark QUIC tunnel packets: quinn sets the TOS byte of \
                     every packet itself (for ECN); the exit's UDP sockets to targets are \
                     still marked",
                );
            }
            if cfg!(windows) {
                warnings.push("tuning.dscp has no effect on Windows unless a QoS policy allows it");
            }
        }
        warnings
    }

    pub fn validate(&self) -> Result<()> {
        let built_without = match self.tunnel.transport {
            TransportKind::Quic if !cfg!(feature = "quic") => Some("quic"),
            TransportKind::Kcp if !cfg!(feature = "kcp") => Some("kcp"),
            _ => None,
        };
        if let Some(feature) = built_without {
            bail!(
                "transport = \"{feature}\" is not in this build (built without the \
                 `{feature}` cargo feature)"
            );
        }
        if self.tunnel.token.trim().len() < 16 {
            bail!("tunnel.token must be at least 16 characters (generate one with `kariz token`)");
        }
        if self.tunnel.token.contains("CHANGE-ME") {
            bail!("tunnel.token still has the sample value; generate one with `kariz token`");
        }

        if self.is_acceptor() {
            if self.tunnel.listen.is_none() {
                bail!(
                    "tunnel.listen is required for role = \"{}\" in mode = \"{}\"",
                    role_name(self.role),
                    mode_name(self.mode)
                );
            }
        } else if self.tunnel.remote.is_none() {
            bail!(
                "tunnel.remote is required for role = \"{}\" in mode = \"{}\"",
                role_name(self.role),
                mode_name(self.mode)
            );
        }

        match self.role {
            Role::Entry => {
                if self.forward.is_empty() {
                    bail!("the entry side needs at least one [[forward]] rule");
                }
            }
            Role::Exit => {
                if !self.forward.is_empty() {
                    bail!("[[forward]] rules belong on the entry side, not on the exit side");
                }
            }
        }

        if self.mode == Mode::Reverse && self.role == Role::Exit && self.tunnel.pool == 0 {
            bail!("tunnel.pool must be at least 1");
        }

        for f in &self.forward {
            if f.target.len() > u16::MAX as usize {
                bail!("forward target is too long: {}", f.target);
            }
            if (f.duplicate.is_some() || f.duplicate_gap_ms.is_some()) && !f.protocol.has_udp() {
                bail!(
                    "forward {}: duplicate and duplicate_gap_ms are for UDP rules",
                    f.listen
                );
            }
            if !DUPLICATE_COPIES.contains(&f.duplicate.unwrap_or(1)) {
                bail!("forward.duplicate must be between 1 and 3");
            }
            if f.duplicate_gap_ms.unwrap_or(0) > MAX_DUPLICATE_GAP_MS {
                bail!("forward.duplicate_gap_ms must be at most {MAX_DUPLICATE_GAP_MS}");
            }
        }

        let tuning = self.tuning();
        if tuning.buffer_size < 1024 {
            bail!("tuning.buffer_size must be at least 1024 bytes");
        }
        if tuning.threads == Some(0) {
            bail!("tuning.threads must be at least 1");
        }
        let udp_timeout = tuning.udp.timeout.as_secs();
        if !UDP_TIMEOUT_SECS.contains(&udp_timeout) {
            bail!(
                "tuning.udp_timeout_secs must be between {} and {}",
                UDP_TIMEOUT_SECS.start(),
                UDP_TIMEOUT_SECS.end()
            );
        }
        if !UDP_MAX_FLOWS.contains(&tuning.udp.max_flows) {
            bail!(
                "tuning.udp_max_flows must be between {} and {}",
                UDP_MAX_FLOWS.start(),
                UDP_MAX_FLOWS.end()
            );
        }
        if let Some(d) = self
            .tuning
            .dscp
            .as_ref()
            .filter(|d| d.codepoint().is_none())
        {
            bail!(
                "tuning.dscp must be a codepoint name (ef, af41, cs4, ...) or a number from \
                 0 to 63, not {d:?}"
            );
        }

        self.validate_mux()?;
        self.validate_ws()?;
        self.validate_tls()?;
        self.validate_quic()?;
        self.validate_kcp()
    }

    fn validate_kcp(&self) -> Result<()> {
        let Some(k) = &self.tunnel.kcp else {
            return Ok(());
        };
        if self.tunnel.transport != TransportKind::Kcp {
            bail!("[tunnel.kcp] is only used with transport = \"kcp\"");
        }
        let manual = [
            k.nodelay.is_some(),
            k.interval_ms.is_some(),
            k.resend.is_some(),
            k.no_congestion.is_some(),
        ];
        if k.mode != KcpMode::Manual && manual.contains(&true) {
            bail!(
                "tunnel.kcp.nodelay, interval_ms, resend and no_congestion need \
                 mode = \"manual\" (mode = \"{}\" sets them)",
                k.mode.name()
            );
        }
        let timing = k.timing();
        if !KCP_INTERVAL_MS.contains(&timing.interval_ms) {
            bail!(
                "tunnel.kcp.interval_ms must be between {} and {}",
                KCP_INTERVAL_MS.start(),
                KCP_INTERVAL_MS.end()
            );
        }
        if timing.resend > KCP_MAX_RESEND {
            bail!("tunnel.kcp.resend must be at most {KCP_MAX_RESEND}");
        }
        for (name, window, range) in [
            ("send_window", k.send_window, KCP_SEND_WINDOW),
            ("recv_window", k.recv_window, KCP_RECV_WINDOW),
        ] {
            if !range.contains(&window) {
                bail!(
                    "tunnel.kcp.{name} must be between {} and {} packets",
                    range.start(),
                    range.end()
                );
            }
        }
        if !KCP_MTU.contains(&k.mtu) {
            bail!(
                "tunnel.kcp.mtu must be between {} and {} bytes",
                KCP_MTU.start(),
                KCP_MTU.end()
            );
        }
        match (k.fec_data.unwrap_or(0), k.fec_parity.unwrap_or(0)) {
            (0, 0) => {}
            (data, parity) if KCP_FEC_DATA.contains(&data) && KCP_FEC_PARITY.contains(&parity) => {
                if k.mtu > KCP_MTU_FEC {
                    bail!("tunnel.kcp.mtu must be at most {KCP_MTU_FEC} bytes with FEC");
                }
            }
            _ => bail!(
                "tunnel.kcp.fec_data must be between {} and {} and fec_parity between {} and \
                 {}, or both 0 (FEC off)",
                KCP_FEC_DATA.start(),
                KCP_FEC_DATA.end(),
                KCP_FEC_PARITY.start(),
                KCP_FEC_PARITY.end()
            ),
        }
        Ok(())
    }

    /// `[tunnel.kcp]` with the profile's defaults filled in: the gaming profile turns FEC
    /// on (10 data + 3 parity packets per group) unless the table sets `fec_data` or
    /// `fec_parity`, or an MTU too large for FEC (docs/PHASE6.md, 6.5).
    pub fn kcp(&self) -> KcpConfig {
        let mut k = self.tunnel.kcp.clone().unwrap_or_default();
        let unset = k.fec_data.is_none() && k.fec_parity.is_none();
        if self.profile == Profile::Gaming && unset && k.mtu <= KCP_MTU_FEC {
            (k.fec_data, k.fec_parity) = (Some(10), Some(3));
        }
        k
    }

    fn validate_quic(&self) -> Result<()> {
        let quic = self.tunnel.transport == TransportKind::Quic;
        if self.tunnel.quic.is_some() && !quic {
            bail!("[tunnel.quic] is only used with transport = \"quic\"");
        }
        if !quic {
            return Ok(());
        }
        if self.tunnel.encryption != Encryption::Auto {
            bail!(
                "transport = \"quic\" always encrypts with TLS 1.3; leave tunnel.encryption \
                 at \"auto\""
            );
        }
        if !self.mux().enabled {
            bail!("transport = \"quic\" always multiplexes; tunnel.mux cannot be disabled");
        }
        let Some(q) = &self.tunnel.quic else {
            return Ok(());
        };
        if q.alpn.is_empty() || q.alpn.len() > 255 || !is_visible_ascii(&q.alpn) {
            bail!("tunnel.quic.alpn must be 1 to 255 visible ASCII characters");
        }
        if self.is_acceptor() && q.sni.is_some() {
            bail!("tunnel.quic.sni is only used by the dialing side");
        }
        if q.sni
            .as_deref()
            .is_some_and(|s| s.is_empty() || !is_visible_ascii(s) || s.contains('/'))
        {
            bail!("tunnel.quic.sni must be a host name");
        }
        Ok(())
    }

    fn validate_mux(&self) -> Result<()> {
        if self.tunnel.transport == TransportKind::Tcpmux && self.tunnel.mux.enabled == Some(false)
        {
            bail!("transport = \"tcpmux\" always uses mux; use transport = \"tcp\" to disable it");
        }
        let mux = self.mux();
        if !MUX_CONNECTIONS.contains(&mux.connections) {
            bail!(
                "tunnel.mux.connections must be between {} and {}",
                MUX_CONNECTIONS.start(),
                MUX_CONNECTIONS.end()
            );
        }
        if !MUX_MAX_STREAMS.contains(&mux.max_streams) {
            bail!(
                "tunnel.mux.max_streams must be between {} and {}",
                MUX_MAX_STREAMS.start(),
                MUX_MAX_STREAMS.end()
            );
        }
        if !MUX_STREAM_WINDOW.contains(&mux.stream_window) {
            bail!(
                "tunnel.mux.stream_window must be between {} and {} bytes",
                MUX_STREAM_WINDOW.start(),
                MUX_STREAM_WINDOW.end()
            );
        }
        if mux
            .max_lifetime
            .is_some_and(|d| d.as_secs() < MUX_MIN_LIFETIME_SECS)
        {
            bail!("tunnel.mux.max_lifetime_secs must be at least {MUX_MIN_LIFETIME_SECS}");
        }
        if !MUX_PING_INTERVAL_SECS.contains(&mux.ping_interval.as_secs()) {
            bail!(
                "tunnel.mux.ping_interval_secs must be between {} and {}",
                MUX_PING_INTERVAL_SECS.start(),
                MUX_PING_INTERVAL_SECS.end()
            );
        }
        if !MUX_DATAGRAM_BUFFER.contains(&mux.datagram_buffer) {
            bail!(
                "tunnel.mux.datagram_buffer must be between {} and {} bytes",
                MUX_DATAGRAM_BUFFER.start(),
                MUX_DATAGRAM_BUFFER.end()
            );
        }
        if !MUX_DATAGRAM_QUEUE.contains(&mux.datagram_queue) {
            bail!(
                "tunnel.mux.datagram_queue must be between {} and {} packets",
                MUX_DATAGRAM_QUEUE.start(),
                MUX_DATAGRAM_QUEUE.end()
            );
        }
        if mux
            .notsent_lowat
            .is_some_and(|v| !MUX_NOTSENT_LOWAT.contains(&v))
        {
            bail!(
                "tunnel.mux.notsent_lowat must be 0 (off) or between {} and {} bytes",
                MUX_NOTSENT_LOWAT.start(),
                MUX_NOTSENT_LOWAT.end()
            );
        }
        Ok(())
    }

    fn validate_ws(&self) -> Result<()> {
        let Some(ws) = &self.tunnel.ws else {
            return Ok(());
        };
        if !self.tunnel.transport.is_websocket() {
            bail!("[tunnel.ws] is only used with transport = \"ws\" or \"wss\"");
        }
        if !ws.path.starts_with('/') || !is_visible_ascii(&ws.path) {
            bail!("tunnel.ws.path must start with '/' and contain no spaces or control characters");
        }
        if ws
            .host
            .as_deref()
            .is_some_and(|h| h.is_empty() || !is_visible_ascii(h) || h.contains('/'))
        {
            bail!("tunnel.ws.host must be a host name, optionally with a port");
        }
        if self.is_acceptor() {
            if ws.user_agent.is_some() || !ws.headers.is_empty() || ws.early_data {
                bail!(
                    "tunnel.ws.user_agent, headers and early_data are only used by the \
                     dialing side"
                );
            }
            return Ok(());
        }
        if ws
            .user_agent
            .as_deref()
            .is_some_and(|ua| !is_header_value(ua))
        {
            bail!("tunnel.ws.user_agent contains invalid characters");
        }
        for (name, value) in &ws.headers {
            if !is_header_name(name) {
                bail!("tunnel.ws.headers: invalid header name {name:?}");
            }
            if RESERVED_WS_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
                bail!("tunnel.ws.headers: {name} is set by kariz itself");
            }
            if !is_header_value(value) {
                bail!("tunnel.ws.headers: invalid value for {name}");
            }
        }
        Ok(())
    }

    fn validate_tls(&self) -> Result<()> {
        let tls = match (
            &self.tunnel.tls,
            self.tunnel.transport == TransportKind::Wss,
        ) {
            (Some(_), false) => bail!("[tunnel.tls] is only used with transport = \"wss\""),
            (None, false) => return Ok(()),
            (tls, true) => tls.clone().unwrap_or_default(),
        };
        if self.is_acceptor() {
            if tls.cert.is_none() || tls.key.is_none() {
                bail!(
                    "tunnel.tls.cert and tunnel.tls.key are required on the listening side of wss"
                );
            }
            if tls.sni.is_some() || tls.pin_sha256.is_some() || tls.insecure {
                bail!("tunnel.tls.sni, pin_sha256 and insecure are only used by the dialing side");
            }
            return Ok(());
        }
        if tls.cert.is_some() || tls.key.is_some() {
            bail!("tunnel.tls.cert and tunnel.tls.key are only used by the listening side");
        }
        if tls
            .sni
            .as_deref()
            .is_some_and(|s| s.is_empty() || !is_visible_ascii(s) || s.contains('/'))
        {
            bail!("tunnel.tls.sni must be a host name");
        }
        if let Some(pin) = &tls.pin_sha256 {
            if tls.insecure {
                bail!("tunnel.tls.pin_sha256 and tunnel.tls.insecure cannot be used together");
            }
            if pin.len() != 64 || !pin.bytes().all(|b| b.is_ascii_hexdigit()) {
                bail!(
                    "tunnel.tls.pin_sha256 must be 64 hex characters (SHA-256 of the certificate)"
                );
            }
        }
        Ok(())
    }
}

fn is_visible_ascii(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_graphic())
}

/// RFC 9110 token.
fn is_header_name(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

fn is_header_value(s: &str) -> bool {
    s.bytes().all(|b| b == b'\t' || (b' '..=b'~').contains(&b))
}

pub fn role_name(role: Role) -> &'static str {
    match role {
        Role::Entry => "entry",
        Role::Exit => "exit",
    }
}

pub fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Reverse => "reverse",
        Mode::Direct => "direct",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123";

    fn entry_reverse() -> String {
        format!(
            r#"
            role = "entry"
            mode = "reverse"
            [tunnel]
            listen = "0.0.0.0:3080"
            token = "{TOKEN}"
            [[forward]]
            listen = "0.0.0.0:443"
            target = "127.0.0.1:443"
            "#
        )
    }

    #[test]
    fn parses_entry_reverse() {
        let c = Config::parse(&entry_reverse()).unwrap();
        assert!(c.is_acceptor());
        assert_eq!(c.profile, Profile::Balanced);
        assert_eq!(c.forward.len(), 1);
    }

    #[test]
    fn exit_reverse_requires_remote() {
        let text = format!(
            r#"
            role = "exit"
            mode = "reverse"
            [tunnel]
            token = "{TOKEN}"
            "#
        );
        let err = Config::parse(&text).unwrap_err().to_string();
        assert!(err.contains("tunnel.remote"), "{err}");
    }

    #[test]
    fn rejects_short_token() {
        let text = entry_reverse().replace(TOKEN, "short");
        assert!(Config::parse(&text).is_err());
    }

    #[test]
    fn rejects_forward_on_exit() {
        let text = format!(
            r#"
            role = "exit"
            mode = "direct"
            [tunnel]
            listen = "0.0.0.0:3080"
            token = "{TOKEN}"
            [[forward]]
            listen = "0.0.0.0:443"
            target = "127.0.0.1:443"
            "#
        );
        assert!(Config::parse(&text).is_err());
    }

    #[test]
    fn rejects_unknown_fields() {
        let text = entry_reverse().replace("mode = \"reverse\"", "mode = \"reverse\"\ntypo = 1");
        assert!(Config::parse(&text).is_err());
    }

    #[test]
    fn overrides_apply_on_top_of_profile() {
        let text = format!(
            "profile = \"gaming\"\n{}\n[tuning]\nbuffer_size = 4096\n",
            entry_reverse()
        );
        let c = Config::parse(&text).unwrap();
        let t = c.tuning();
        assert_eq!(t.buffer_size, 4096);
        assert_eq!(t.keepalive, Duration::from_secs(10));
    }

    /// A config for `transport`, with extra `[tunnel]` lines and sub-tables appended.
    fn with_transport(role: &str, transport: &str, extra: &str) -> String {
        let (tunnel, forward) = if role == "entry" {
            (
                "remote = \"203.0.113.1:443\"",
                "[[forward]]\nlisten = \"0.0.0.0:443\"\ntarget = \"127.0.0.1:443\"",
            )
        } else {
            ("listen = \"0.0.0.0:443\"", "")
        };
        format!(
            "role = \"{role}\"\nmode = \"direct\"\n{forward}\n\
             [tunnel]\ntransport = \"{transport}\"\n{tunnel}\ntoken = \"{TOKEN}\"\n{extra}\n"
        )
    }

    fn parse_err(text: &str) -> String {
        format!("{:#}", Config::parse(text).unwrap_err())
    }

    #[test]
    fn ultraspeed_profile_also_under_its_old_name() {
        for name in ["ultraspeed", "throughput"] {
            let text = format!("profile = \"{name}\"\n{}", entry_reverse());
            let c = Config::parse(&text).unwrap();
            assert_eq!(c.profile, Profile::Ultraspeed, "{name}");
            assert_eq!(c.profile.name(), "ultraspeed");
            assert_eq!(c.mux().stream_window, 1024 * 1024);
            assert_eq!(c.tuning().buffer_size, 256 * 1024);
        }
    }

    #[test]
    fn gaming_profile_turns_kcp_fec_on_unless_set() {
        let kcp = |profile: &str, table: &str| {
            let text = format!(
                "profile = \"{profile}\"\n{}",
                with_transport("entry", "kcp", &format!("[tunnel.kcp]\n{table}"))
            );
            Config::parse(&text).unwrap().kcp().fec()
        };
        assert_eq!(kcp("gaming", ""), Some((10, 3)));
        assert_eq!(kcp("balanced", ""), None);
        assert_eq!(kcp("ultraspeed", ""), None);
        assert_eq!(kcp("throughput", ""), None);
        // Set in the table: the table wins, off included.
        assert_eq!(kcp("gaming", "fec_data = 0\nfec_parity = 0"), None);
        assert_eq!(kcp("gaming", "fec_data = 4\nfec_parity = 2"), Some((4, 2)));
        // An MTU too large for FEC keeps it off rather than making the config invalid.
        assert_eq!(kcp("gaming", "mtu = 1440"), None);
        assert_eq!(kcp("gaming", "mtu = 1429"), Some((10, 3)));
    }

    /// Parses without validation, for checking effective values on their own.
    fn parse_unchecked(text: &str) -> Config {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn v01_config_gets_defaults() {
        let c = Config::parse(&entry_reverse()).unwrap();
        assert_eq!(c.tunnel.transport, TransportKind::Tcp);
        assert_eq!(c.tunnel.encryption, Encryption::Auto);
        assert!(!c.mux().enabled);
        assert!(c.warnings().is_empty());
    }

    #[test]
    fn mux_defaults_follow_transport_and_profile() {
        let tcpmux = parse_unchecked(&with_transport("entry", "tcpmux", ""));
        assert!(tcpmux.mux().enabled);
        assert_eq!(tcpmux.mux().stream_window, 256 * 1024);
        assert_eq!(tcpmux.mux().connections, 4);

        let ws = parse_unchecked(&format!(
            "profile = \"gaming\"\n{}",
            with_transport("entry", "ws", "")
        ));
        let mux = ws.mux();
        assert!(mux.enabled);
        assert_eq!(mux.stream_window, 64 * 1024);
        assert!(!mux.coalesce);

        let off = parse_unchecked(&with_transport(
            "entry",
            "ws",
            "[tunnel.mux]\nenabled = false\nconnections = 3",
        ));
        assert!(!off.mux().enabled);
        assert_eq!(off.mux().connections, 3);
    }

    #[test]
    fn long_keepalive_over_websocket_is_warned() {
        let slow = "[tuning]\nkeepalive_secs = 120";
        let c = Config::parse(&format!("{}{slow}", with_transport("entry", "ws", ""))).unwrap();
        assert_eq!(c.warnings().len(), 1);
        let c = Config::parse(&format!("{}{slow}", with_transport("entry", "tcp", ""))).unwrap();
        assert!(c.warnings().is_empty());
    }

    /// An entry config with one forward rule using `protocol` and extra top-level lines.
    fn with_forward(protocol: &str, transport: &str, extra: &str) -> String {
        with_transport("entry", transport, extra).replace(
            "target = \"127.0.0.1:443\"",
            &format!("target = \"127.0.0.1:443\"\nprotocol = \"{protocol}\""),
        )
    }

    #[test]
    fn forward_protocols() {
        for (name, tcp, udp) in [
            ("tcp", true, false),
            ("udp", false, true),
            ("tcp+udp", true, true),
        ] {
            let c = parse_unchecked(&with_forward(name, "tcpmux", ""));
            let p = c.forward[0].protocol;
            assert_eq!((p.name(), p.has_tcp(), p.has_udp()), (name, tcp, udp));
        }
        assert_eq!(
            Config::parse(&entry_reverse()).unwrap().forward[0].protocol,
            ForwardProtocol::Tcp
        );
        assert!(toml::from_str::<Config>(&with_forward("sctp", "tcp", "")).is_err());
    }

    #[test]
    fn udp_tuning_defaults_and_overrides() {
        let t = Tuning::for_profile(Profile::Balanced);
        assert_eq!(t.udp.timeout, Duration::from_secs(60));
        assert_eq!(t.udp.max_flows, 1024);
        assert!(Tuning::for_profile(Profile::Gaming).udp.flow_queue < t.udp.flow_queue);
        let c = parse_unchecked(&format!(
            "{}[tuning]\nudp_timeout_secs = 30\nudp_max_flows = 10\n",
            with_forward("udp", "tcpmux", "")
        ));
        assert_eq!(c.tuning().udp.timeout, Duration::from_secs(30));
        assert_eq!(c.tuning().udp.max_flows, 10);
        for bad in [
            "udp_timeout_secs = 1",
            "udp_timeout_secs = 4000",
            "udp_max_flows = 0",
        ] {
            let err = parse_err(&format!("{}[tuning]\n{bad}\n", entry_reverse()));
            assert!(err.contains("tuning.udp_"), "{bad}: {err}");
        }
    }

    #[test]
    fn udp_forward_rules_are_valid() {
        for protocol in ["udp", "tcp+udp"] {
            let c = Config::parse(&with_forward(protocol, "tcpmux", "")).unwrap();
            assert!(c.forward[0].protocol.has_udp());
        }
        // UDP rules belong to the entry side like TCP ones.
        let exit = with_transport("exit", "tcpmux", "");
        assert!(Config::parse(&exit).is_ok());
    }

    #[test]
    fn udp_without_mux_is_warned() {
        let c = parse_unchecked(&with_forward("udp", "tcp", ""));
        assert_eq!(c.warnings().len(), 1);
        assert!(c.warnings()[0].contains("mux"));
        assert!(parse_unchecked(&with_forward("udp", "tcpmux", ""))
            .warnings()
            .is_empty());
        assert!(parse_unchecked(&with_forward("tcp", "tcp", ""))
            .warnings()
            .is_empty());
    }

    #[test]
    fn dscp_names_and_numbers() {
        let with = |value: &str| format!("{}[tuning]\ndscp = {value}\n", entry_reverse());
        for (value, codepoint) in [
            ("\"ef\"", 46),
            ("\"EF\"", 46),
            ("\"af41\"", 34),
            ("\"cs4\"", 32),
            ("\"le\"", 1),
            ("\"default\"", 0),
            ("0", 0),
            ("63", 63),
        ] {
            let c = Config::parse(&with(value)).unwrap();
            assert_eq!(c.tuning().dscp, Some(codepoint), "{value}");
        }
        assert_eq!(Config::parse(&entry_reverse()).unwrap().tuning().dscp, None);
        for bad in ["\"fast\"", "64", "-1", "\"\""] {
            let err = parse_err(&with(bad));
            assert!(err.contains("dscp"), "{bad}: {err}");
        }
        assert_eq!(Dscp::name_of(46), Some("ef"));
        assert_eq!(Dscp::name_of(0), Some("cs0"));
        assert_eq!(Dscp::name_of(7), None);
        // Over QUIC the tunnel's own packets cannot be marked: said, not hidden.
        let quic = Config::parse(&format!(
            "{}[tuning]\ndscp = \"ef\"\n",
            with_transport("entry", "quic", "")
        ))
        .unwrap();
        assert!(quic.warnings().iter().any(|w| w.contains("QUIC")));
    }

    #[test]
    fn duplication_on_udp_rules() {
        let dup = |protocol: &str, transport: &str, lines: &str| {
            with_forward(protocol, transport, "").replace(
                &format!("protocol = \"{protocol}\""),
                &format!("protocol = \"{protocol}\"\n{lines}"),
            )
        };
        let c = Config::parse(&dup("udp", "kcp", "duplicate = 2")).unwrap();
        assert_eq!(
            c.forward[0].duplication(),
            Some(Duplicate {
                copies: 2,
                gap_ms: 5
            })
        );
        assert!(c.warnings().is_empty(), "{:?}", c.warnings());
        let c = Config::parse(&dup(
            "tcp+udp",
            "quic",
            "duplicate = 3\nduplicate_gap_ms = 0",
        ))
        .unwrap();
        assert_eq!(
            c.forward[0].duplication(),
            Some(Duplicate {
                copies: 3,
                gap_ms: 0
            })
        );
        let one = Config::parse(&dup("udp", "kcp", "duplicate = 1")).unwrap();
        assert_eq!(one.forward[0].duplication(), None);
        // Accepted, but pointless over transports that lose nothing.
        let c = Config::parse(&dup("udp", "tcpmux", "duplicate = 2")).unwrap();
        assert!(c.warnings().iter().any(|w| w.contains("duplicate")));
        for (protocol, lines, needle) in [
            ("tcp", "duplicate = 2", "for UDP rules"),
            ("udp", "duplicate = 0", "between 1 and 3"),
            ("udp", "duplicate = 4", "between 1 and 3"),
            ("udp", "duplicate = 2\nduplicate_gap_ms = 51", "at most 50"),
        ] {
            let err = parse_err(&dup(protocol, "kcp", lines));
            assert!(err.contains(needle), "{lines}: {err}");
        }
    }

    #[test]
    fn notsent_lowat_only_with_mux() {
        let tcp = Config::parse(&with_transport("entry", "tcp", "")).unwrap();
        assert_eq!(tcp.tuning().notsent_lowat, None);
        for transport in ["tcpmux", "ws", "wss"] {
            let c = parse_unchecked(&with_transport("entry", transport, ""));
            assert_eq!(c.tuning().notsent_lowat, Some(NOTSENT_LOWAT), "{transport}");
        }
    }

    #[test]
    fn mux_settings_default_to_the_profile_and_can_be_set() {
        let mux_of = |top: &str, table: &str| {
            let text = format!(
                "{top}\n{}",
                with_transport("entry", "tcpmux", &format!("[tunnel.mux]\n{table}"))
            );
            let c = Config::parse(&text).unwrap();
            (c.mux(), c.tuning())
        };
        // Defaults: the profile's values, and the keepalive for pings.
        let (mux, tuning) = mux_of("", "");
        assert!(mux.coalesce);
        assert_eq!(mux.ping_interval, tuning.keepalive);
        assert_eq!(mux.datagram_buffer, tuning.udp.session_buffer);
        assert_eq!(mux.datagram_queue, tuning.udp.flow_queue);
        assert_eq!(tuning.notsent_lowat, Some(NOTSENT_LOWAT));
        let (gaming, _) = mux_of("profile = \"gaming\"", "");
        assert!(!gaming.coalesce);
        assert_eq!(gaming.ping_interval, Duration::from_secs(10));
        // [tuning] keepalive_secs still sets the ping when the mux table does not.
        let text = format!(
            "{}[tuning]\nkeepalive_secs = 20\n",
            with_transport("entry", "tcpmux", "")
        );
        let c = Config::parse(&text).unwrap();
        assert_eq!(c.mux().ping_interval, Duration::from_secs(20));
        // A 1 s keepalive, valid before the mux setting existed, stays valid.
        let text = format!(
            "{}[tuning]
keepalive_secs = 1
",
            with_transport("entry", "ws", "")
        );
        assert!(Config::parse(&text).is_ok());

        // Set by hand.
        let (mux, tuning) = mux_of(
            "",
            "coalesce = false\nping_interval_secs = 15\ndatagram_buffer = 65536\n\
             datagram_queue = 32\nnotsent_lowat = 65536",
        );
        assert!(!mux.coalesce);
        assert_eq!(mux.ping_interval, Duration::from_secs(15));
        assert_eq!((mux.datagram_buffer, mux.datagram_queue), (65536, 32));
        assert_eq!(tuning.notsent_lowat, Some(65536));
        let (mux, tuning) = mux_of("", "notsent_lowat = 0");
        assert_eq!((mux.notsent_lowat, tuning.notsent_lowat), (None, None));

        for (table, needle) in [
            ("ping_interval_secs = 0", "ping_interval_secs"),
            ("ping_interval_secs = 601", "ping_interval_secs"),
            ("datagram_buffer = 1024", "datagram_buffer"),
            ("datagram_queue = 4", "datagram_queue"),
            ("notsent_lowat = 100", "notsent_lowat"),
        ] {
            let text = with_transport("entry", "tcpmux", &format!("[tunnel.mux]\n{table}"));
            let err = parse_err(&text);
            assert!(err.contains(needle), "{table}: {err}");
        }
        // A CDN cuts idle WebSockets after about 100 s: warned for the mux ping too.
        let ws = with_transport("entry", "ws", "[tunnel.mux]\nping_interval_secs = 120");
        assert!(Config::parse(&ws).unwrap().warnings()[0].contains("90"));
    }

    #[test]
    fn encryption_none_is_allowed_with_a_warning() {
        let c = Config::parse(&with_transport("entry", "tcp", "encryption = \"none\"")).unwrap();
        assert_eq!(c.tunnel.encryption, Encryption::None);
        assert_eq!(c.warnings().len(), 1);
        assert!(Config::parse(&with_transport("entry", "tcp", "encryption = \"rot13\"")).is_err());
        for cipher in ["chacha20-poly1305", "aes-256-gcm", "auto"] {
            let text = with_transport("entry", "tcp", &format!("encryption = \"{cipher}\""));
            let c = Config::parse(&text).unwrap();
            assert_eq!(c.tunnel.encryption.name(), cipher);
            assert!(c.warnings().is_empty());
        }
    }

    #[test]
    fn mux_configs_parse() {
        let c = Config::parse(&with_transport("entry", "tcpmux", "")).unwrap();
        assert!(c.mux().enabled);
        let text = with_transport(
            "exit",
            "tcp",
            "[tunnel.mux]\nenabled = true\nmax_lifetime_secs = 600",
        );
        let c = Config::parse(&text).unwrap();
        assert_eq!(c.mux().max_lifetime, Some(Duration::from_secs(600)));
    }

    #[test]
    fn tcpmux_cannot_disable_mux() {
        let err = parse_err(&with_transport(
            "entry",
            "tcpmux",
            "[tunnel.mux]\nenabled = false",
        ));
        assert!(err.contains("tcpmux"), "{err}");
    }

    #[test]
    fn mux_bounds() {
        for extra in [
            "connections = 0",
            "connections = 65",
            "max_streams = 0",
            "stream_window = 1024",
            "stream_window = 1073741824",
            "max_lifetime_secs = 5",
        ] {
            let err = parse_err(&with_transport(
                "entry",
                "tcpmux",
                &format!("[tunnel.mux]\n{extra}"),
            ));
            assert!(err.contains("tunnel.mux."), "{extra}: {err}");
        }
    }

    #[test]
    fn ws_and_tls_sections_need_a_matching_transport() {
        let err = parse_err(&with_transport(
            "entry",
            "tcp",
            "[tunnel.ws]\npath = \"/x\"",
        ));
        assert!(err.contains("[tunnel.ws]"), "{err}");
        let err = parse_err(&with_transport(
            "entry",
            "ws",
            "[tunnel.tls]\nsni = \"a.example\"",
        ));
        assert!(err.contains("[tunnel.tls]"), "{err}");
    }

    #[test]
    fn ws_validation() {
        for (extra, expect) in [
            ("path = \"no-slash\"", "tunnel.ws.path"),
            ("path = \"/a b\"", "tunnel.ws.path"),
            ("host = \"\"", "tunnel.ws.host"),
            ("user_agent = \"a\\r\\nX: y\"", "user_agent"),
            ("headers = { \"Bad Name\" = \"v\" }", "invalid header name"),
            (
                "headers = { \"Sec-WebSocket-Key\" = \"v\" }",
                "set by kariz",
            ),
            ("headers = { \"X-A\" = \"a\\nb\" }", "invalid value"),
        ] {
            let err = parse_err(&with_transport(
                "entry",
                "ws",
                &format!("[tunnel.ws]\n{extra}"),
            ));
            assert!(err.contains(expect), "{extra}: {err}");
        }

        let ok = "[tunnel.ws]\npath = \"/api/v1\"\nhost = \"a.example\"\n\
                  headers = { \"Accept-Language\" = \"en-US\" }";
        let c = Config::parse(&with_transport("entry", "ws", ok)).unwrap();
        assert_eq!(c.tunnel.ws.unwrap().path, "/api/v1");
        // `[tunnel.ws]` is optional: path "/", any host.
        let c = Config::parse(&with_transport("exit", "ws", "")).unwrap();
        assert!(c.mux().enabled);

        for extra in ["user_agent = \"x\"", "early_data = true"] {
            let err = parse_err(&with_transport(
                "exit",
                "ws",
                &format!("[tunnel.ws]\n{extra}"),
            ));
            assert!(err.contains("dialing side"), "{extra}: {err}");
        }
        let c = Config::parse(&with_transport(
            "entry",
            "ws",
            "[tunnel.ws]\nearly_data = true",
        ))
        .unwrap();
        assert!(c.tunnel.ws.unwrap().early_data);
    }

    #[test]
    fn tls_validation() {
        // Listening side of wss.
        let err = parse_err(&with_transport("exit", "wss", ""));
        assert!(err.contains("tunnel.tls.cert"), "{err}");
        let err = parse_err(&with_transport(
            "exit",
            "wss",
            "[tunnel.tls]\ncert = \"c.pem\"\nkey = \"k.pem\"\nsni = \"a.example\"",
        ));
        assert!(err.contains("dialing side"), "{err}");
        let ok = "[tunnel.tls]\ncert = \"c.pem\"\nkey = \"k.pem\"";
        assert!(Config::parse(&with_transport("exit", "wss", ok)).is_ok());

        // Dialing side of wss.
        let pin = "ab".repeat(32);
        for (extra, expect) in [
            ("cert = \"c.pem\"".to_string(), "listening side"),
            ("pin_sha256 = \"abc\"".to_string(), "64 hex"),
            (
                format!("pin_sha256 = \"{pin}\"\ninsecure = true"),
                "together",
            ),
            ("sni = \"\"".to_string(), "tunnel.tls.sni"),
        ] {
            let err = parse_err(&with_transport(
                "entry",
                "wss",
                &format!("[tunnel.tls]\n{extra}"),
            ));
            assert!(err.contains(expect), "{extra}: {err}");
        }
        let ok = format!("[tunnel.tls]\nsni = \"a.example\"\npin_sha256 = \"{pin}\"");
        assert!(Config::parse(&with_transport("entry", "wss", &ok)).is_ok());
        // Without [tunnel.tls] the dialer verifies against the bundled Mozilla roots.
        assert!(Config::parse(&with_transport("entry", "wss", "")).is_ok());

        let insecure = parse_unchecked(&with_transport(
            "entry",
            "wss",
            "[tunnel.tls]\ninsecure = true",
        ));
        assert!(insecure.validate_tls().is_ok());
        assert_eq!(insecure.warnings().len(), 1);
    }

    #[cfg(feature = "quic")]
    #[test]
    fn quic_configs_parse() {
        let c = Config::parse(&with_transport("entry", "quic", "")).unwrap();
        assert!(c.mux().enabled);
        assert_eq!(c.mux().connections, 2);
        assert!(c.tunnel.quic.is_none());
        assert!(c.warnings().is_empty());
        let q = QuicConfig::default();
        assert_eq!(
            (q.alpn.as_str(), q.congestion, q.sni),
            ("h3", Congestion::Cubic, None)
        );

        let text = with_transport(
            "entry",
            "quic",
            "[tunnel.quic]\ncongestion = \"bbr\"\nsni = \"a.example\"\nalpn = \"hq-29\"",
        );
        let q = Config::parse(&text).unwrap().tunnel.quic.unwrap();
        assert_eq!(q.congestion, Congestion::Bbr);
        assert_eq!(q.sni.as_deref(), Some("a.example"));
        assert_eq!(q.alpn, "hq-29");
        let text = with_transport("entry", "quic", "[tunnel.quic]\ncongestion = \"bbr\"");
        assert_eq!(Config::parse(&text).unwrap().warnings().len(), 1);
        // An empty table means the defaults, as no table does.
        let text = with_transport("exit", "quic", "[tunnel.quic]");
        assert_eq!(
            Config::parse(&text).unwrap().tunnel.quic.unwrap().alpn,
            "h3"
        );
    }

    #[cfg(feature = "quic")]
    #[test]
    fn quic_validation() {
        let err = parse_err(&with_transport("entry", "tcp", "[tunnel.quic]"));
        assert!(err.contains("[tunnel.quic]"), "{err}");
        let err = parse_err(&with_transport("entry", "quic", "encryption = \"none\""));
        assert!(err.contains("TLS 1.3"), "{err}");
        let err = parse_err(&with_transport(
            "entry",
            "quic",
            "[tunnel.mux]\nenabled = false",
        ));
        assert!(err.contains("multiplexes"), "{err}");
        for (role, extra, expect) in [
            ("entry", "alpn = \"\"", "tunnel.quic.alpn"),
            ("entry", "alpn = \"a b\"", "tunnel.quic.alpn"),
            ("entry", "sni = \"\"", "tunnel.quic.sni"),
            ("entry", "sni = \"a.example/x\"", "tunnel.quic.sni"),
            ("exit", "sni = \"a.example\"", "dialing side"),
            ("entry", "congestion = \"vegas\"", "congestion"),
        ] {
            let err = parse_err(&with_transport(
                role,
                "quic",
                &format!("[tunnel.quic]\n{extra}"),
            ));
            assert!(err.contains(expect), "{extra}: {err}");
        }
    }

    #[cfg(feature = "kcp")]
    #[test]
    fn kcp_configs_parse() {
        let c = Config::parse(&with_transport("entry", "kcp", "")).unwrap();
        assert!(c.mux().enabled, "mux on by default, as for ws");
        assert!(c.warnings().is_empty());
        let k = KcpConfig::default();
        assert_eq!(k.mode, KcpMode::Fast2);
        assert_eq!((k.send_window, k.recv_window, k.mtu), (1024, 1024, 1350));
        assert_eq!(
            k.timing(),
            KcpTiming {
                nodelay: true,
                interval_ms: 20,
                resend: 2,
                no_congestion: true
            }
        );

        // Without mux, and with an explicit cipher: KCP is a stream like tcp.
        let text = with_transport(
            "exit",
            "kcp",
            "encryption = \"chacha20-poly1305\"\n[tunnel.mux]\nenabled = false",
        );
        assert!(!Config::parse(&text).unwrap().mux().enabled);

        let text = with_transport(
            "entry",
            "kcp",
            "[tunnel.kcp]\nmode = \"normal\"\nsend_window = 256\nrecv_window = 512\nmtu = 1200",
        );
        let k = Config::parse(&text).unwrap().tunnel.kcp.unwrap();
        assert_eq!((k.send_window, k.recv_window, k.mtu), (256, 512, 1200));
        assert_eq!(k.fec(), None);
        assert_eq!(k.timing().interval_ms, 40);
        assert!(!k.timing().nodelay);

        let text = with_transport(
            "entry",
            "kcp",
            "[tunnel.kcp]\nfec_data = 10\nfec_parity = 3\nmtu = 1429",
        );
        let k = Config::parse(&text).unwrap().tunnel.kcp.unwrap();
        assert_eq!(k.fec(), Some((10, 3)));

        // Manual: what is set, and fast2 for the rest.
        let text = with_transport(
            "entry",
            "kcp",
            "[tunnel.kcp]\nmode = \"manual\"\ninterval_ms = 15\nno_congestion = false",
        );
        let t = Config::parse(&text).unwrap().tunnel.kcp.unwrap().timing();
        assert_eq!(
            t,
            KcpTiming {
                nodelay: true,
                interval_ms: 15,
                resend: 2,
                no_congestion: false
            }
        );
    }

    /// Only in builds without the transports: their configs are rejected up front.
    #[cfg(not(all(feature = "quic", feature = "kcp")))]
    #[test]
    fn transports_not_built_in_are_rejected() {
        for (transport, built) in [
            ("quic", cfg!(feature = "quic")),
            ("kcp", cfg!(feature = "kcp")),
        ] {
            if !built {
                let err = parse_err(&with_transport("entry", transport, ""));
                assert!(err.contains("not in this build"), "{err}");
            }
        }
    }

    #[cfg(feature = "kcp")]
    #[test]
    fn kcp_validation() {
        let err = parse_err(&with_transport("entry", "quic", "[tunnel.kcp]"));
        assert!(err.contains("[tunnel.kcp]"), "{err}");
        for (extra, expect) in [
            ("nodelay = false", "mode = \"manual\""),
            ("mode = \"fast3\"\nresend = 0", "mode = \"manual\""),
            ("mode = \"turbo\"", "mode"),
            (
                "mode = \"manual\"\ninterval_ms = 5",
                "tunnel.kcp.interval_ms",
            ),
            (
                "mode = \"manual\"\ninterval_ms = 2000",
                "tunnel.kcp.interval_ms",
            ),
            ("mode = \"manual\"\nresend = 11", "tunnel.kcp.resend"),
            ("send_window = 8", "tunnel.kcp.send_window"),
            ("recv_window = 64", "tunnel.kcp.recv_window"),
            ("recv_window = 40000", "tunnel.kcp.recv_window"),
            ("mtu = 500", "tunnel.kcp.mtu"),
            ("mtu = 1444", "tunnel.kcp.mtu"),
            ("fec = 1", "unknown field"),
            ("fec_data = 10", "fec_parity between"),
            ("fec_parity = 3", "fec_parity between"),
            ("fec_data = 65\nfec_parity = 3", "fec_parity between"),
            ("fec_data = 10\nfec_parity = 33", "fec_parity between"),
            ("fec_data = 10\nfec_parity = 3\nmtu = 1430", "with FEC"),
        ] {
            let err = parse_err(&with_transport(
                "entry",
                "kcp",
                &format!("[tunnel.kcp]\n{extra}"),
            ));
            assert!(err.contains(expect), "{extra}: {err}");
        }
    }
}
