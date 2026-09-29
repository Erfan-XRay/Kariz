//! Join codes: what *Add server* shows and `kariz-panel agent --join` reads.
//!
//! `kz1_` and the URL-safe base64 of a small JSON document: where the panel listens for
//! agents, the link token, and a join secret that works once (10 minutes). The token is in
//! it, so a code is as secret as a password until it is used.

use anyhow::{anyhow, bail, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};

pub const PREFIX: &str = "kz1_";
/// A join code works for this long, once.
pub const JOIN_TTL: i64 = 600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoinCode {
    /// Where the panel listens for agents: `host:port`.
    pub p: String,
    /// The panel's link token.
    pub t: String,
    /// The join secret.
    pub j: String,
    /// The name the server is to have, if it was named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<String>,
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
    if code.p.is_empty() || code.t.len() < 16 || code.j.is_empty() {
        bail!("this join code is incomplete");
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
