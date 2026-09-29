//! What the panel and its agents say to each other (docs/PHASE11.md, section 4).
//!
//! A request is one mux stream: the panel opens it with the request as JSON in the open
//! bytes, the agent answers with one JSON document and finishes the stream. The panel is
//! always the one that asks; the requests are a fixed list, and none of them runs a
//! command the panel names: there is no remote shell.

use serde::{Deserialize, Serialize};

/// Largest request the panel sends (the open bytes of a stream hold 64 KiB).
pub const MAX_REQUEST: usize = 16 * 1024;
/// Largest answer the panel reads.
pub const MAX_REPLY: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Who are you? `challenge` is random; a registered agent proves its key over it.
    Hello { challenge: String },
    /// A new agent's identity: keep it and use it from now on.
    Enroll { id: String, key: String },
    /// CPU, memory, network, uptime, load.
    Health,
    /// The tunnels on this server (the `/etc/kariz/*.toml` files) and their state.
    Tunnels,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloReply {
    /// Set once the agent is registered.
    pub id: Option<String>,
    /// `blake3::keyed_hash(key, challenge)` in hex, with the agent's key.
    pub proof: Option<String>,
    /// The join secret from the join code, until the agent is registered.
    pub join: Option<String>,
    pub hostname: String,
    pub version: String,
    pub arch: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ack {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Health {
    /// Busy share of all CPUs since the previous sample, 0-100.
    pub cpu_pct: Option<f64>,
    pub mem_total: Option<u64>,
    pub mem_used: Option<u64>,
    /// Bytes per second over the network interfaces (not the loopback), since the
    /// previous sample.
    pub rx_bps: Option<f64>,
    pub tx_bps: Option<f64>,
    pub uptime_secs: Option<u64>,
    pub load1: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForwardInfo {
    pub listen: String,
    pub target: String,
    pub protocol: String,
}

/// One tunnel config on a server. Never carries the token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TunnelInfo {
    /// The file name without `.toml`, which is also the service (`kariz@NAME`).
    pub name: String,
    pub role: String,
    pub mode: String,
    pub transport: String,
    pub profile: String,
    pub listen: Option<String>,
    pub remote: Option<String>,
    pub forwards: Vec<ForwardInfo>,
    /// systemd says the service runs. `None` where systemd is not there to ask.
    pub active: Option<bool>,
    /// From the running daemon's control socket (`kariz status`).
    pub status: Option<kariz::stats::Status>,
    /// Why the file could not be read, if it could not.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_tagged_json() {
        let json = serde_json::to_string(&Request::Hello {
            challenge: "ab".into(),
        })
        .unwrap();
        assert_eq!(json, r#"{"op":"hello","challenge":"ab"}"#);
        assert_eq!(
            serde_json::to_string(&Request::Health).unwrap(),
            r#"{"op":"health"}"#
        );
        for text in [
            r#"{"op":"health"}"#,
            r#"{"op":"tunnels"}"#,
            r#"{"op":"enroll","id":"a","key":"b"}"#,
        ] {
            serde_json::from_str::<Request>(text).unwrap();
        }
        // Only these operations exist: there is no way to name a command. (A request
        // with extra fields is read as its operation and the extras are ignored; no
        // operation takes a command, a path or a name.)
        for text in [
            r#"{"op":"exec","cmd":"ls"}"#,
            r#"{"op":"shell"}"#,
            r#"{"op":"enroll"}"#,
            "{}",
            "[]",
        ] {
            assert!(serde_json::from_str::<Request>(text).is_err(), "{text}");
        }
    }
}
