//! Join codes: what *Add server* shows and `kariz-panel agent --join` reads.
//!
//! `kz1_` and the URL-safe base64 of a small JSON document: where the panel listens for
//! agents, the link token, and a join secret that works once (10 minutes). The token is in
//! it, so a code is as secret as a password until it is used.
//!
//! A code for a server that reaches the panel only through a private (GRE) network also
//! carries that server's end of the link: the agent makes it first, then dials the panel's
//! private address across it.
//!
//! A code for a server the panel connects to (reverse) has no panel address: it says where
//! the agent listens (`l`), and the panel dials it there. An agent from before reverse links
//! finds the code incomplete instead of dialing nowhere.

use anyhow::{anyhow, bail, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::wire::NetSpec;

pub const PREFIX: &str = "kz1_";
/// A join code works for this long, once.
pub const JOIN_TTL: i64 = 600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinCode {
    /// Where the panel listens for agents: `host:port`. Empty in a reverse code.
    pub p: String,
    /// The panel's link token.
    pub t: String,
    /// The join secret.
    pub j: String,
    /// The name the server is to have, if it was named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<String>,
    /// The one link transport to use (`tcpmux`, `kcp`, `wss`, `quic`); none: try them all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    /// The GRE link to make before dialing (this server's end of it): `p` is then the
    /// panel's private address on that link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub g: Option<NetSpec>,
    /// Reverse: where the agent listens for the panel (`0.0.0.0:PORT`, its link transports
    /// on that port and the next, as a panel's agents port); the panel dials it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub l: Option<String>,
}

/// Whether `addr` is an address the agent can listen on: an IP address (or `[v6]`) and a
/// port below 65535 (the next port is used too).
pub fn valid_listen(addr: &str) -> bool {
    addr.rsplit_once(':').is_some_and(|(host, port)| {
        let host = host.trim_start_matches('[').trim_end_matches(']');
        host.parse::<std::net::IpAddr>().is_ok()
            && port
                .parse::<u16>()
                .is_ok_and(|p| (1..u16::MAX).contains(&p))
    })
}

/// This server's public addresses, as other servers would dial them: the source address
/// the system picks for the Internet, over IPv4 and over IPv6. Nothing is sent (a UDP
/// socket is only connected). Private, loopback and link-local addresses are left out.
pub fn own_addresses() -> (Option<std::net::Ipv4Addr>, Option<std::net::Ipv6Addr>) {
    use std::net::{IpAddr, UdpSocket};
    let probe = |bind: &str, to: &str| -> Option<IpAddr> {
        let s = UdpSocket::bind(bind).ok()?;
        s.connect(to).ok()?;
        Some(s.local_addr().ok()?.ip())
    };
    let v4 = match probe("0.0.0.0:0", "8.8.8.8:53") {
        Some(IpAddr::V4(a))
            if !a.is_private() && !a.is_loopback() && !a.is_link_local() && !a.is_unspecified() =>
        {
            Some(a)
        }
        _ => None,
    };
    let v6 = match probe("[::]:0", "[2001:4860:4860::8888]:53") {
        Some(IpAddr::V6(a)) if global_v6(&a) => Some(a),
        _ => None,
    };
    (v4, v6)
}

/// A global unicast IPv6 address (2000::/3), not a unique-local or link-local one.
fn global_v6(a: &std::net::Ipv6Addr) -> bool {
    (a.segments()[0] & 0xe000) == 0x2000
}

/// A link transport a join code may name.
pub fn valid_transport(name: &str) -> bool {
    kariz::link::LINK_TRANSPORTS
        .iter()
        .any(|k| k.name() == name)
}

pub fn encode(code: &JoinCode) -> String {
    let json = serde_json::to_vec(code).expect("a join code serializes");
    format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(json))
}

pub fn decode(text: &str) -> Result<JoinCode> {
    let body = text
        .trim()
        .strip_prefix(PREFIX)
        .ok_or_else(|| anyhow!("this is not a join code (it starts with {PREFIX})"))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(body)
        .map_err(|_| anyhow!("this join code is damaged (copy all of it)"))?;
    let code: JoinCode = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow!("this join code is damaged (copy all of it)"))?;
    if (code.p.is_empty() && code.l.is_none()) || code.t.len() < 16 || code.j.is_empty() {
        bail!("this join code is incomplete");
    }
    if code.l.as_deref().is_some_and(|l| !valid_listen(l)) {
        bail!("this join code says to listen on an address that is not valid");
    }
    if code.x.as_deref().is_some_and(|x| !valid_transport(x)) {
        bail!("this join code names a transport this agent does not know: update it");
    }
    if let Some(link) = &code.g {
        crate::net::validate(link).map_err(|e| {
            anyhow!("this join code has a private network link that is not valid: {e}")
        })?;
    }
    Ok(code)
}

/// A host name or address the panel's address can be built from: no spaces, slashes or
/// anything else that could change what `host:port` means.
pub fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
}

/// A name for a server: letters, digits, `-`, `_` and `.`, up to 40.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code() -> JoinCode {
        JoinCode {
            p: "203.0.113.5:29001".into(),
            t: "t".repeat(64),
            j: "j".repeat(32),
            n: Some("istanbul-1".into()),
            x: None,
            g: None,
            l: None,
        }
    }

    fn link() -> NetSpec {
        NetSpec {
            name: "kz-ab12c".into(),
            local: "198.51.100.7".into(),
            remote: "203.0.113.5".into(),
            key: 1,
            address: "10.77.0.1".into(),
            peer: "10.77.0.2".into(),
            prefix: 30,
            mtu: 1476,
        }
    }

    #[test]
    fn a_code_round_trips() {
        let text = encode(&code());
        assert!(text.starts_with("kz1_"));
        assert!(!text.contains(['=', '+', '/', ' ']), "{text}");
        assert_eq!(decode(&text).unwrap(), code());
        assert_eq!(decode(&format!("  {text}\n")).unwrap(), code());
    }

    #[test]
    fn damaged_and_foreign_codes_are_refused_with_a_reason() {
        let text = encode(&code());
        for (bad, why) in [
            ("hello", "not a join code"),
            ("kz1_!!!", "damaged"),
            (&text[..text.len() - 7], "damaged"),
            ("kz1_e30", "damaged"), // {} : missing fields
        ] {
            let err = decode(bad).unwrap_err().to_string();
            assert!(err.contains(why), "{bad}: {err}");
        }
        let short = JoinCode {
            t: "short".into(),
            ..code()
        };
        assert!(decode(&encode(&short))
            .unwrap_err()
            .to_string()
            .contains("incomplete"));
        let unknown = JoinCode {
            x: Some("carrier-pigeon".into()),
            ..code()
        };
        assert!(decode(&encode(&unknown))
            .unwrap_err()
            .to_string()
            .contains("update it"));
    }

    #[test]
    fn only_global_ipv6_addresses_are_offered() {
        let ok = |s: &str| global_v6(&s.parse().unwrap());
        assert!(ok("2001:db8::1") && ok("2a01:4f8::5"));
        assert!(!ok("fe80::1") && !ok("fd00::1") && !ok("::1") && !ok("::"));
    }

    #[test]
    fn a_code_can_name_one_transport() {
        for x in ["tcpmux", "kcp", "wss", "quic"] {
            let one = JoinCode {
                x: Some(x.into()),
                ..code()
            };
            assert_eq!(decode(&encode(&one)).unwrap().x.as_deref(), Some(x));
        }
        // Without one (auto) the code is what agents before 1.4 read too.
        assert!(!encode(&code()).is_empty());
        assert!(!serde_json::to_string(&code()).unwrap().contains("\"x\""));
    }

    #[test]
    fn a_code_can_carry_the_gre_link_to_make_first() {
        let with = JoinCode {
            p: "10.77.0.2:29001".into(),
            g: Some(link()),
            ..code()
        };
        assert_eq!(decode(&encode(&with)).unwrap().g, Some(link()));
        // Without one, nothing about it is in the code (what older agents read too).
        assert!(!serde_json::to_string(&code()).unwrap().contains("\"g\""));
        // A link the agent could not make is refused before anything is written.
        let bad = JoinCode {
            g: Some(NetSpec {
                name: "eth0".into(),
                ..link()
            }),
            ..with
        };
        assert!(decode(&encode(&bad))
            .unwrap_err()
            .to_string()
            .contains("private network link"));
    }

    #[test]
    fn a_reverse_code_says_where_to_listen_and_has_no_panel_address() {
        let reverse = JoinCode {
            p: String::new(),
            l: Some("0.0.0.0:29001".into()),
            ..code()
        };
        let back = decode(&encode(&reverse)).unwrap();
        assert_eq!(
            (back.p.as_str(), back.l.as_deref()),
            ("", Some("0.0.0.0:29001"))
        );
        // Without either, it is incomplete; so is a reverse code to an agent that reads no `l`.
        let neither = JoinCode {
            p: String::new(),
            ..code()
        };
        assert!(decode(&encode(&neither))
            .unwrap_err()
            .to_string()
            .contains("incomplete"));
        for bad in [
            "0.0.0.0",
            "0.0.0.0:0",
            "0.0.0.0:65535",
            "example.com:29001",
            "0.0.0.0:x",
        ] {
            let code = JoinCode {
                l: Some(bad.into()),
                ..reverse.clone()
            };
            assert!(decode(&encode(&code)).is_err(), "{bad}");
        }
        assert!(valid_listen("[::]:29001") && valid_listen("192.0.2.4:443"));
    }

    #[test]
    fn hosts_and_names_are_checked() {
        for ok in [
            "203.0.113.5",
            "panel.example.com",
            "[2001:db8::1]",
            "2001:db8::1",
            "localhost",
        ] {
            assert!(valid_host(ok), "{ok}");
        }
        for bad in ["", "a b", "a/b", "a;b", "a@b", "http://x", &"x".repeat(300)] {
            assert!(!valid_host(bad), "{bad}");
        }
        assert!(valid_name("istanbul-1") && valid_name("a.b_c"));
        for bad in ["", "a b", "a/b", "é", &"x".repeat(41)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }
}
