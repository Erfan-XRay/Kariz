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

use std::path::Path;
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
        Ok(())
    }
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
}
