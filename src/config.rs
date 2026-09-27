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
    Throughput,
    Gaming,
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
}

impl TransportKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Tcpmux => "tcpmux",
            Self::Ws => "ws",
            Self::Wss => "wss",
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
#[serde(rename_all = "lowercase")]
pub enum ForwardProtocol {
    #[default]
    Tcp,
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
}

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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
        }
    }
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
}

impl Tuning {
    pub fn for_profile(profile: Profile) -> Self {
        let buffer_size = match profile {
            Profile::Balanced => 64 * 1024,
            Profile::Throughput => 256 * 1024,
            Profile::Gaming => 16 * 1024,
        };
        Self {
            nodelay: true,
            buffer_size,
            keepalive: Duration::from_secs(if profile == Profile::Gaming { 10 } else { 30 }),
            dial_timeout: Duration::from_secs(10),
            handshake_timeout: Duration::from_secs(10),
            threads: None,
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
}

impl MuxSettings {
    fn for_profile(profile: Profile, transport: TransportKind) -> Self {
        let (stream_window, connections) = match profile {
            Profile::Balanced => (256 * 1024, 4),
            Profile::Throughput => (1024 * 1024, 8),
            Profile::Gaming => (64 * 1024, 2),
        };
        Self {
            enabled: transport != TransportKind::Tcp,
            connections,
            max_streams: 512,
            stream_window,
            max_lifetime: None,
            coalesce: profile != Profile::Gaming,
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
        self
    }
}

const MUX_CONNECTIONS: std::ops::RangeInclusive<usize> = 1..=64;
const MUX_MAX_STREAMS: std::ops::RangeInclusive<usize> = 1..=4096;
const MUX_STREAM_WINDOW: std::ops::RangeInclusive<usize> = 16 * 1024..=16 * 1024 * 1024;
const MUX_MIN_LIFETIME_SECS: u64 = 60;

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
        Tuning::for_profile(self.profile).apply(&self.tuning)
    }

    pub fn mux(&self) -> MuxSettings {
        MuxSettings::for_profile(self.profile, self.tunnel.transport).apply(&self.tunnel.mux)
    }

    /// Settings that are valid but weaken the tunnel; shown by `kariz check` and at startup.
    pub fn warnings(&self) -> Vec<&'static str> {
        let mut warnings = Vec::new();
        if self.tunnel.encryption == Encryption::None {
            warnings
                .push("tunnel.encryption = \"none\": traffic between the servers is not encrypted");
        }
        if self.tunnel.tls.as_ref().is_some_and(|t| t.insecure) {
            warnings.push(
                "tunnel.tls.insecure = true: the server certificate is not verified; \
                 prefer tls.pin_sha256",
            );
        }
        warnings
    }

    pub fn validate(&self) -> Result<()> {
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
        }

        let tuning = self.tuning();
        if tuning.buffer_size < 1024 {
            bail!("tuning.buffer_size must be at least 1024 bytes");
        }
        if tuning.threads == Some(0) {
            bail!("tuning.threads must be at least 1");
        }

        self.validate_mux()?;
        self.validate_ws()?;
        self.validate_tls()?;
        self.check_implemented()
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
            if ws.user_agent.is_some() || !ws.headers.is_empty() {
                bail!(
                    "tunnel.ws.user_agent and tunnel.ws.headers are only used by the dialing side"
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

    /// Rejects settings that parse and validate but are not implemented yet.
    /// Each phase 2 step removes the part it implements.
    fn check_implemented(&self) -> Result<()> {
        let not_yet = |what: &str| -> Result<()> {
            bail!("{what} is not implemented yet (planned for v0.2, see docs/PHASE2.md)")
        };
        if self.tunnel.transport.is_websocket() {
            return not_yet(&format!("transport = \"{}\"", self.tunnel.transport.name()));
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

    /// Parses without validation, for checking effective values of settings that are
    /// not implemented yet.
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
    fn unimplemented_features_are_rejected_after_validation() {
        for (transport, extra) in [("ws", ""), ("wss", "")] {
            let err = parse_err(&with_transport("entry", transport, extra));
            assert!(
                err.contains("not implemented yet"),
                "{transport} {extra}: {err}"
            );
        }
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

        // A valid dialer section only fails on the "not implemented" check.
        let ok = "[tunnel.ws]\npath = \"/api/v1\"\nhost = \"a.example\"\n\
                  headers = { \"Accept-Language\" = \"en-US\" }";
        assert!(parse_err(&with_transport("entry", "ws", ok)).contains("not implemented yet"));

        let err = parse_err(&with_transport(
            "exit",
            "ws",
            "[tunnel.ws]\nuser_agent = \"x\"",
        ));
        assert!(err.contains("dialing side"), "{err}");
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
        assert!(parse_err(&with_transport("exit", "wss", ok)).contains("not implemented yet"));

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
        assert!(parse_err(&with_transport("entry", "wss", &ok)).contains("not implemented yet"));

        let insecure = parse_unchecked(&with_transport(
            "entry",
            "wss",
            "[tunnel.tls]\ninsecure = true",
        ));
        assert!(insecure.validate_tls().is_ok());
        assert_eq!(insecure.warnings().len(), 1);
    }
}
