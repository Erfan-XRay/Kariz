//! Mutual authentication handshake for tunnel connections.
//!
//! The dialer proves it knows the token, and then the acceptor proves it does too.
//! The token itself never goes over the wire, and the messages have no fixed bytes
//! a DPI box could match on.
//!
//! ```text
//! dialer   -> acceptor : ts (8, big-endian unix secs) | nonce (16) | tag_c (32)
//! acceptor -> dialer   : tag_s (32)
//!
//! key   = BLAKE3-derive-key(CONTEXT, token)
//! tag_c = BLAKE3-keyed(key, "c" | ts | nonce)
//! tag_s = BLAKE3-keyed(key, "s" | ts | nonce)
//! ```
//!
//! The acceptor rejects stale timestamps and remembers recent nonces, so a captured
//! hello cannot be replayed.

use std::collections::HashMap;
use std::io;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const CONTEXT: &str = "kariz 2026-09 tunnel auth v1";
/// Maximum allowed clock difference between the two servers, in seconds.
pub const MAX_CLOCK_SKEW: u64 = 120;
const REPLAY_PRUNE_THRESHOLD: usize = 4096;

pub const HELLO_LEN: usize = 8 + 16 + 32;
pub const REPLY_LEN: usize = 32;

#[derive(Clone)]
pub struct AuthKey {
    key: [u8; 32],
}

impl AuthKey {
    pub fn new(token: &str) -> Self {
        Self {
            key: blake3::derive_key(CONTEXT, token.as_bytes()),
        }
    }

    fn tag(&self, side: u8, ts: [u8; 8], nonce: [u8; 16]) -> blake3::Hash {
        let mut h = blake3::Hasher::new_keyed(&self.key);
        h.update(&[side]);
        h.update(&ts);
        h.update(&nonce);
        h.finalize()
    }

    /// Builds the dialer's hello message.
    pub fn hello(&self) -> io::Result<([u8; HELLO_LEN], Pending)> {
        let ts = unix_now().to_be_bytes();
        let mut nonce = [0u8; 16];
        getrandom::fill(&mut nonce).map_err(io::Error::other)?;
        let mut msg = [0u8; HELLO_LEN];
        msg[..8].copy_from_slice(&ts);
        msg[8..24].copy_from_slice(&nonce);
        msg[24..].copy_from_slice(self.tag(b'c', ts, nonce).as_bytes());
        Ok((msg, Pending { ts, nonce }))
    }
}

/// State the dialer keeps until it has verified the acceptor's reply.
pub struct Pending {
    ts: [u8; 8],
    nonce: [u8; 16],
}

impl Pending {
    pub fn verify_reply(&self, key: &AuthKey, reply: &[u8; REPLY_LEN]) -> io::Result<()> {
        if key.tag(b's', self.ts, self.nonce) == *reply {
            Ok(())
        } else {
            Err(auth_error("peer failed authentication (token mismatch?)"))
        }
    }
}

/// Remembers recently seen nonces to reject replayed hellos.
#[derive(Default)]
pub struct ReplayFilter {
    seen: Mutex<HashMap<[u8; 16], u64>>,
}

impl ReplayFilter {
    fn check_and_insert(&self, nonce: [u8; 16], ts: u64, now: u64) -> bool {
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        if seen.len() >= REPLAY_PRUNE_THRESHOLD {
            seen.retain(|_, t| now.abs_diff(*t) <= MAX_CLOCK_SKEW);
        }
        seen.insert(nonce, ts).is_none()
    }
}

/// Verifies a hello on the acceptor side and returns the reply to send back.
pub fn accept_hello(
    key: &AuthKey,
    replay: &ReplayFilter,
    hello: &[u8; HELLO_LEN],
) -> io::Result<[u8; REPLY_LEN]> {
    let ts: [u8; 8] = hello[..8].try_into().unwrap();
    let nonce: [u8; 16] = hello[8..24].try_into().unwrap();
    let tag: [u8; 32] = hello[24..].try_into().unwrap();

    // blake3::Hash equality is constant-time.
    if key.tag(b'c', ts, nonce) != tag {
        return Err(auth_error("invalid hello tag"));
    }
    let ts_val = u64::from_be_bytes(ts);
    let now = unix_now();
    if now.abs_diff(ts_val) > MAX_CLOCK_SKEW {
        return Err(auth_error(
            "hello timestamp out of range (are both server clocks in sync?)",
        ));
    }
    if !replay.check_and_insert(nonce, ts_val, now) {
        return Err(auth_error("replayed hello"));
    }
    Ok(*key.tag(b's', ts, nonce).as_bytes())
}

/// Acceptor side of the handshake over a stream.
pub async fn server_handshake<S>(
    stream: &mut S,
    key: &AuthKey,
    replay: &ReplayFilter,
) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut hello = [0u8; HELLO_LEN];
    stream.read_exact(&mut hello).await?;
    let reply = accept_hello(key, replay, &hello)?;
    stream.write_all(&reply).await
}

/// Reads and verifies the acceptor's reply on the dialer side.
pub async fn read_reply<S>(stream: &mut S, key: &AuthKey, pending: &Pending) -> io::Result<()>
where
    S: AsyncRead + Unpin,
{
    let mut reply = [0u8; REPLY_LEN];
    stream.read_exact(&mut reply).await?;
    pending.verify_reply(key, &reply)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn auth_error(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let key = AuthKey::new("same-token-on-both-sides");
        let replay = ReplayFilter::default();
        let (hello, pending) = key.hello().unwrap();
        let reply = accept_hello(&key, &replay, &hello).unwrap();
        pending.verify_reply(&key, &reply).unwrap();
    }

    #[test]
    fn wrong_token_is_rejected_both_ways() {
        let a = AuthKey::new("token-a-token-a-token-a");
        let b = AuthKey::new("token-b-token-b-token-b");
        let replay = ReplayFilter::default();
        let (hello, pending) = a.hello().unwrap();
        assert!(accept_hello(&b, &replay, &hello).is_err());

        // A fake acceptor that does not know the token cannot produce a valid reply.
        let (hello, _) = b.hello().unwrap();
        let forged = accept_hello(&b, &replay, &hello).unwrap();
        assert!(pending.verify_reply(&a, &forged).is_err());
    }

    #[test]
    fn replay_is_rejected() {
        let key = AuthKey::new("same-token-on-both-sides");
        let replay = ReplayFilter::default();
        let (hello, _) = key.hello().unwrap();
        accept_hello(&key, &replay, &hello).unwrap();
        assert!(accept_hello(&key, &replay, &hello).is_err());
    }

    #[test]
    fn stale_timestamp_is_rejected() {
        let key = AuthKey::new("same-token-on-both-sides");
        let replay = ReplayFilter::default();
        let ts = (unix_now() - MAX_CLOCK_SKEW - 5).to_be_bytes();
        let nonce = [7u8; 16];
        let mut hello = [0u8; HELLO_LEN];
        hello[..8].copy_from_slice(&ts);
        hello[8..24].copy_from_slice(&nonce);
        hello[24..].copy_from_slice(key.tag(b'c', ts, nonce).as_bytes());
        assert!(accept_hello(&key, &replay, &hello).is_err());
    }

    #[test]
    fn tampered_hello_is_rejected() {
        let key = AuthKey::new("same-token-on-both-sides");
        let replay = ReplayFilter::default();
        let (mut hello, _) = key.hello().unwrap();
        hello[10] ^= 1;
        assert!(accept_hello(&key, &replay, &hello).is_err());
    }
}
