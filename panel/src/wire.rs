//! What the panel and its agents say to each other.
//!
//! A request is one mux stream: the panel opens it with the request as JSON in the open
//! bytes, the agent answers with one JSON document and finishes the stream. The panel is
//! always the one that asks; the requests are a fixed list, and none of them runs a
//! command the panel names: there is no remote shell.

use serde::{Deserialize, Serialize};

/// Largest request the panel sends (the open bytes of a stream hold 64 KiB). A tunnel with
/// the most ports the wizard allows (200 forwards) is 15 KB with IPv4 addresses and more
/// than 20 KB with IPv6 ones.
pub const MAX_REQUEST: usize = 60 * 1024;
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
    /// Is this tunnel spec valid here, and are its ports free?
    TunnelCheck { spec: Spec },
    /// Write the tunnel's config from a spec (the agent renders the file).
    TunnelPut { spec: Spec },
    /// One tunnel's spec, without its token.
    TunnelGet { name: String },
    /// Stop it, remove its config.
    TunnelDelete { name: String },
    /// `start`, `stop`, `restart`, `enable` or `disable` the tunnel's service.
    TunnelCtl { name: String, action: String },
    /// The listening ports of this server and who owns them.
    Ports,
    /// The last lines of a tunnel's log.
    Logs { name: String, lines: u32 },
    /// Run the entry side's speed test.
    Speedtest {
        name: String,
        seconds: u32,
        streams: u32,
        udp: bool,
    },
    /// Make (or make again) the GRE interface of a private network link.
    NetUp { net: NetSpec },
    /// Remove a link's interface.
    NetDown { name: String },
    /// The panel's whole list of this server's links: make the ones that are missing or
    /// changed, remove the ones that are not in it (sent when an agent connects).
    NetSync { links: Vec<NetSpec> },
    /// Ping the far end of a link across it.
    NetPing { name: String },
    /// Whether GRE can be made here, and the state of this server's links.
    NetStatus,
    /// A release is about to be sent (its three files and their sizes).
    UpdateBegin {
        version: String,
        files: Vec<UpdateFile>,
    },
    /// One piece of one of the files: a base64 chunk at an offset.
    UpdateChunk {
        version: String,
        name: String,
        offset: u64,
        data: String,
    },
    /// Everything has been sent: check it, unpack it and swap the programs.
    UpdateApply { version: String },
}

/// A file of a release that is being sent to an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateFile {
    pub name: String,
    pub size: u64,
}

/// One private network link on this server, as data. The
/// agent checks every field before it runs anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetSpec {
    /// `kz-` and up to eight small letters or digits.
    pub name: String,
    /// This server's public address, and the other server's.
    pub local: String,
    pub remote: String,
    /// Tells two GRE tunnels between the same two addresses apart.
    pub key: u32,
    /// This end's private address, the other end's, and the prefix length (30 or 31).
    pub address: String,
    pub peer: String,
    pub prefix: u8,
    pub mtu: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetIface {
    pub name: String,
    pub address: String,
    pub exists: bool,
    pub up: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetStatus {
    /// This server can make GRE interfaces (root, the `ip_gre` module, no container limit).
    pub gre: bool,
    /// Why not, when it cannot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub links: Vec<NetIface>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PingReply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What a tunnel is, as data: everything the manager's `add` takes, without free text
/// going into the file. The agent turns it into the config.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub name: String,
    pub role: String,
    pub mode: String,
    pub transport: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Sent when a tunnel is made or its token changes; `None` keeps the one in the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ws_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ws_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_sni: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_pin: Option<String>,
    #[serde(default)]
    pub forwards: Vec<ForwardInfo>,
}

/// Something that listens on a port of this server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortOwner {
    /// `tcp` or `udp`.
    pub proto: String,
    pub addr: String,
    pub port: u16,
    /// The process name (`sshd`, `kariz`), when it could be found.
    pub process: Option<String>,
    pub pid: Option<u32>,
}

/// The answer to `tunnel_put`: the certificate pin of a listening wss side, for the
/// dialing side's config.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PutReply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
}

/// The answer to `tunnel_check`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CheckReply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Ports the spec wants that something else already listens on.
    #[serde(default)]
    pub conflicts: Vec<PortOwner>,
}

/// The answer to `logs` and `speedtest`: text, or why there is none.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TextReply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub text: String,
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
    /// The IPv4 networks this server already has routes for (`172.17.0.0/16`, a cloud's
    /// private network...), without the default route and Kariz's own `kz-` links. Private
    /// networks are never given a range that overlaps one.
    #[serde(default)]
    pub routes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    fn a_tunnel_with_the_most_ports_the_wizard_allows_fits_one_request() {
        let spec = Spec {
            name: "big".into(),
            role: "entry".into(),
            mode: "reverse".into(),
            transport: "tcpmux".into(),
            listen: Some("0.0.0.0:3080".into()),
            token: Some("t".repeat(48)),
            forwards: (0..200)
                .map(|i| ForwardInfo {
                    listen: format!("[::]:{}", 10000 + i),
                    target: format!("[2001:db8:85a3::8a2e:370:7334]:{}", 10000 + i),
                    protocol: "tcp+udp".into(),
                })
                .collect(),
            ..Default::default()
        };
        let size = serde_json::to_vec(&Request::TunnelPut { spec })
            .unwrap()
            .len();
        assert!(size < MAX_REQUEST, "{size} bytes");
        assert!(
            size > 16 * 1024,
            "the old limit would have refused it: {size} bytes"
        );
    }

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
