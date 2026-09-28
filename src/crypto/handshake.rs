//! Handshake v2: mutual authentication with the shared token, X25519 for forward
//! secrecy, and nothing on the wire that is not indistinguishable from random bytes.
//!
//! ```text
//! psk    = BLAKE3-derive-key("kariz 2026-10 psk v2", token)
//! mask_k = BLAKE3-derive-key("kariz v2 mask", psk)       keystream: BLAKE3-keyed XOF
//! tag_k  = BLAKE3-derive-key("kariz v2 tag", psk)
//!
//! dialer -> acceptor
//!   nonce_c (16)
//!   hdr_c   (44) = [ ts (8) | eph_c (32) | cipher (1) | pad_len (2) | flags (1) ]
//!                  XOR mask("c" | nonce_c)
//!   pad_c   (pad_len <= 512, random)
//!   tag_c   (32) = BLAKE3-keyed(tag_k, "c" | nonce_c | hdr_c | pad_c)
//!
//! acceptor -> dialer
//!   hdr_s   (34) = [ eph_s (32) | pad_len (2) ] XOR mask("s" | nonce_c)
//!   pad_s
//!   tag_s   (32) = BLAKE3-keyed(tag_k, "s" | transcript_c | hdr_s | pad_s)
//!
//! th      = BLAKE3(transcript_c | transcript_s)
//! c2s/s2c = BLAKE3-derive-key("kariz v2 c2s" / "kariz v2 s2c", psk | X25519 | th)
//! early   = BLAKE3-derive-key("kariz v2 early", psk | nonce_c | hdr_c)
//! ```
//!
//! The acceptor rejects stale timestamps and remembers recent nonces, so a captured
//! hello (and any early data after it) cannot be replayed. `cipher` is covered by the tag,
//! so it cannot be downgraded on the path.

use std::collections::HashMap;
use std::io;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use ring::agreement::{self, EphemeralPrivateKey, UnparsedPublicKey, X25519};
use ring::rand::SystemRandom;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::{random_below, random_bytes, Cipher};

const PSK_CONTEXT: &str = "kariz 2026-10 psk v2";
const MASK_CONTEXT: &str = "kariz v2 mask";
const TAG_CONTEXT: &str = "kariz v2 tag";
const C2S_CONTEXT: &str = "kariz v2 c2s";
const S2C_CONTEXT: &str = "kariz v2 s2c";
const EARLY_CONTEXT: &str = "kariz v2 early";

/// Maximum allowed clock difference between the two servers, in seconds.
pub const MAX_CLOCK_SKEW: u64 = 120;
const REPLAY_PRUNE_THRESHOLD: usize = 4096;

const NONCE_LEN: usize = 16;
const CLIENT_HDR_LEN: usize = 8 + 32 + 1 + 2 + 1;
const SERVER_HDR_LEN: usize = 32 + 2;
const TAG_LEN: usize = 32;
/// Longest padding a peer may send.
pub const MAX_PAD: usize = 512;
/// Longest padding we send. Phase 6 shapes this; for now it only varies the length.
const SEND_PAD_MAX: u32 = 256;

/// Bytes the acceptor reads before it knows the padding length.
pub const HELLO_HEAD_LEN: usize = NONCE_LEN + CLIENT_HDR_LEN;

/// `flags` bit: one early-data record follows the hello.
const FLAG_EARLY: u8 = 1;
/// `flags` bit: the connection carries a mux session. Both sides must agree.
const FLAG_MUX: u8 = 2;

pub type Key = [u8; 32];

/// Keys derived from the shared token.
#[derive(Clone)]
pub struct Psk {
    psk: Key,
    mask: Key,
    tag: Key,
}

impl Psk {
    pub fn new(token: &str) -> Self {
        let psk = blake3::derive_key(PSK_CONTEXT, token.as_bytes());
        Self {
            psk,
            mask: blake3::derive_key(MASK_CONTEXT, &psk),
            tag: blake3::derive_key(TAG_CONTEXT, &psk),
        }
    }

    fn apply_mask(&self, side: u8, nonce: &[u8], data: &mut [u8]) {
        let mut h = blake3::Hasher::new_keyed(&self.mask);
        h.update(&[side]);
        h.update(nonce);
        let mut keystream = [0u8; CLIENT_HDR_LEN];
        let keystream = &mut keystream[..data.len()];
        h.finalize_xof().fill(keystream);
        for (d, k) in data.iter_mut().zip(keystream.iter()) {
            *d ^= k;
        }
    }

    fn tag(&self, side: u8, parts: &[&[u8]]) -> blake3::Hash {
        let mut h = blake3::Hasher::new_keyed(&self.tag);
        h.update(&[side]);
        for p in parts {
            h.update(p);
        }
        h.finalize()
    }

    /// A key for another use of the token (e.g. the QUIC identity), independent of the
    /// handshake's keys thanks to its own `context`.
    pub(crate) fn subkey(&self, context: &str) -> Key {
        self.derive(context, &[])
    }

    fn derive(&self, context: &str, parts: &[&[u8]]) -> Key {
        let mut h = blake3::Hasher::new_derive_key(context);
        h.update(&self.psk);
        for p in parts {
            h.update(p);
        }
        *h.finalize().as_bytes()
    }
}

/// Keys for the record layer, one per direction.
pub struct SessionKeys {
    pub cipher: Cipher,
    pub c2s: Key,
    pub s2c: Key,
}

/// State the dialer keeps until it has verified the acceptor's reply.
pub struct ClientState {
    psk: Psk,
    secret: EphemeralPrivateKey,
    nonce: [u8; NONCE_LEN],
    transcript: Vec<u8>,
    cipher: Cipher,
    early_key: Option<Key>,
}

/// Builds the dialer's hello. With `early`, the hello announces one early-data record,
/// to be sealed with [`ClientState::early_key`] and sent right after it. `mux` announces
/// a mux session on this connection.
pub fn client_hello(
    psk: &Psk,
    cipher: Cipher,
    early: bool,
    mux: bool,
) -> io::Result<(Vec<u8>, ClientState)> {
    client_hello_at(psk, cipher, early, mux, unix_now())
}

fn client_hello_at(
    psk: &Psk,
    cipher: Cipher,
    early: bool,
    mux: bool,
    ts: u64,
) -> io::Result<(Vec<u8>, ClientState)> {
    let (secret, public) = ephemeral()?;
    let nonce: [u8; NONCE_LEN] = random_bytes()?;
    let pad = padding()?;

    let mut hdr = [0u8; CLIENT_HDR_LEN];
    hdr[..8].copy_from_slice(&ts.to_be_bytes());
    hdr[8..40].copy_from_slice(&public);
    hdr[40] = cipher.id();
    hdr[41..43].copy_from_slice(&(pad.len() as u16).to_be_bytes());
    hdr[43] = if early { FLAG_EARLY } else { 0 } | if mux { FLAG_MUX } else { 0 };
    psk.apply_mask(b'c', &nonce, &mut hdr);

    let mut msg = Vec::with_capacity(HELLO_HEAD_LEN + pad.len() + TAG_LEN);
    msg.extend_from_slice(&nonce);
    msg.extend_from_slice(&hdr);
    msg.extend_from_slice(&pad);
    let tag = psk.tag(b'c', &[&msg]);
    msg.extend_from_slice(tag.as_bytes());

    let early_key = early.then(|| psk.derive(EARLY_CONTEXT, &[&nonce, &hdr]));
    let state = ClientState {
        psk: psk.clone(),
        secret,
        nonce,
        transcript: msg.clone(),
        cipher,
        early_key,
    };
    Ok((msg, state))
}

impl ClientState {
    /// Key for the early-data record, if the hello announced one.
    pub fn early_key(&self) -> Option<&Key> {
        self.early_key.as_ref()
    }

    /// Reads and verifies the acceptor's reply and derives the session keys.
    pub async fn read_reply<S: AsyncRead + Unpin>(self, stream: &mut S) -> io::Result<SessionKeys> {
        let mut masked = [0u8; SERVER_HDR_LEN];
        stream.read_exact(&mut masked).await?;
        let mut hdr = masked;
        self.psk.apply_mask(b's', &self.nonce, &mut hdr);
        let pad_len = u16::from_be_bytes([hdr[32], hdr[33]]) as usize;
        if pad_len > MAX_PAD {
            return Err(auth_error("peer failed authentication (token mismatch?)"));
        }
        let mut rest = vec![0u8; pad_len + TAG_LEN];
        stream.read_exact(&mut rest).await?;
        let (pad, tag) = rest.split_at(pad_len);
        if !tag_matches(self.psk.tag(b's', &[&self.transcript, &masked, pad]), tag) {
            return Err(auth_error("peer failed authentication (token mismatch?)"));
        }

        let shared = agree(self.secret, &hdr[..32])?;
        let th = transcript_hash(&self.transcript, &[&masked, &rest]);
        Ok(session_keys(&self.psk, self.cipher, &shared, &th))
    }
}

/// What the acceptor learned from a verified hello.
pub struct Accepted {
    pub keys: SessionKeys,
    /// Set when the dialer sent an early-data record after the hello.
    pub early_key: Option<Key>,
}

/// Acceptor side: reads and verifies the hello, answers, and derives the session keys.
/// `allows` decides which ciphers this side accepts; `mux` is whether this side expects a
/// mux session.
///
/// Authentication failures are `PermissionDenied`; an authenticated peer asking for a
/// cipher this side does not allow, or disagreeing on mux, is `Unsupported`.
pub async fn accept<S>(
    stream: &mut S,
    psk: &Psk,
    replay: &ReplayFilter,
    allows: impl Fn(Cipher) -> bool,
    mux: bool,
) -> io::Result<Accepted>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut head = [0u8; HELLO_HEAD_LEN];
    stream.read_exact(&mut head).await?;
    let (nonce, masked) = head.split_at(NONCE_LEN);
    let mut hdr = [0u8; CLIENT_HDR_LEN];
    hdr.copy_from_slice(masked);
    psk.apply_mask(b'c', nonce, &mut hdr);
    let pad_len = u16::from_be_bytes([hdr[41], hdr[42]]) as usize;
    if pad_len > MAX_PAD {
        return Err(auth_error("invalid hello"));
    }
    let mut rest = vec![0u8; pad_len + TAG_LEN];
    stream.read_exact(&mut rest).await?;
    let (pad, tag) = rest.split_at(pad_len);
    if !tag_matches(psk.tag(b'c', &[&head, pad]), tag) {
        return Err(auth_error("invalid hello tag"));
    }

    let ts = u64::from_be_bytes(hdr[..8].try_into().unwrap());
    let now = unix_now();
    if now.abs_diff(ts) > MAX_CLOCK_SKEW {
        return Err(auth_error(
            "hello timestamp out of range (are both server clocks in sync?)",
        ));
    }
    let nonce: [u8; NONCE_LEN] = nonce.try_into().unwrap();
    if !replay.check_and_insert(nonce, ts, now) {
        return Err(auth_error("replayed hello"));
    }
    let cipher = match Cipher::from_id(hdr[40]) {
        Some(c) if allows(c) => c,
        Some(c) => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                "peer uses encryption {:?}, which tunnel.encryption on this side does not allow",
                c.name()
            ),
            ))
        }
        None => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "peer asked for an unknown cipher (different kariz versions?)",
            ))
        }
    };

    if (hdr[43] & FLAG_MUX != 0) != mux {
        let (peer, us) = if mux { ("off", "on") } else { ("on", "off") };
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("peer has mux {peer} but this side has it {us}; set tunnel.mux the same on both sides"),
        ));
    }

    let (secret, public) = ephemeral()?;
    let pad_s = padding()?;
    let mut reply = Vec::with_capacity(SERVER_HDR_LEN + pad_s.len() + TAG_LEN);
    reply.extend_from_slice(&public);
    reply.extend_from_slice(&(pad_s.len() as u16).to_be_bytes());
    psk.apply_mask(b's', &nonce, &mut reply);
    reply.extend_from_slice(&pad_s);
    let transcript_c = [head.as_slice(), &rest].concat();
    let tag_s = psk.tag(b's', &[&transcript_c, &reply]);
    reply.extend_from_slice(tag_s.as_bytes());
    stream.write_all(&reply).await?;
    stream.flush().await?;

    let shared = agree(secret, &hdr[8..40])?;
    let th = transcript_hash(&transcript_c, &[&reply]);
    let early_key = (hdr[43] & FLAG_EARLY != 0)
        .then(|| psk.derive(EARLY_CONTEXT, &[&nonce, &head[NONCE_LEN..]]));
    Ok(Accepted {
        keys: session_keys(psk, cipher, &shared, &th),
        early_key,
    })
}

fn session_keys(psk: &Psk, cipher: Cipher, shared: &Key, th: &blake3::Hash) -> SessionKeys {
    SessionKeys {
        cipher,
        c2s: psk.derive(C2S_CONTEXT, &[shared, th.as_bytes()]),
        s2c: psk.derive(S2C_CONTEXT, &[shared, th.as_bytes()]),
    }
}

fn transcript_hash(transcript_c: &[u8], transcript_s: &[&[u8]]) -> blake3::Hash {
    let mut h = blake3::Hasher::new();
    h.update(transcript_c);
    for p in transcript_s {
        h.update(p);
    }
    h.finalize()
}

/// Constant-time tag comparison (`blake3::Hash` equality is constant-time).
fn tag_matches(expected: blake3::Hash, got: &[u8]) -> bool {
    <[u8; TAG_LEN]>::try_from(got).is_ok_and(|got| expected == got)
}

fn ephemeral() -> io::Result<(EphemeralPrivateKey, [u8; 32])> {
    let secret = EphemeralPrivateKey::generate(&X25519, &SystemRandom::new())
        .map_err(|_| io::Error::other("failed to generate an X25519 key"))?;
    let public = secret
        .compute_public_key()
        .map_err(|_| io::Error::other("failed to compute an X25519 public key"))?;
    let public: [u8; 32] = public.as_ref().try_into().unwrap();
    Ok((secret, public))
}

fn agree(secret: EphemeralPrivateKey, peer_public: &[u8]) -> io::Result<Key> {
    agreement::agree_ephemeral(secret, &UnparsedPublicKey::new(&X25519, peer_public), |s| {
        <Key>::try_from(s).unwrap()
    })
    .map_err(|_| auth_error("invalid key exchange"))
}

fn padding() -> io::Result<Vec<u8>> {
    let len = random_below(SEND_PAD_MAX)? as usize;
    let mut pad = vec![0u8; len];
    getrandom::fill(&mut pad).map_err(io::Error::other)?;
    Ok(pad)
}

/// Remembers recently seen nonces to reject replayed hellos.
#[derive(Default)]
pub struct ReplayFilter {
    seen: Mutex<HashMap<[u8; NONCE_LEN], u64>>,
}

impl ReplayFilter {
    fn check_and_insert(&self, nonce: [u8; NONCE_LEN], ts: u64, now: u64) -> bool {
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        if seen.len() >= REPLAY_PRUNE_THRESHOLD {
            seen.retain(|_, t| now.abs_diff(*t) <= MAX_CLOCK_SKEW);
        }
        seen.insert(nonce, ts).is_none()
    }
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

    const TOKEN: &str = "same-token-on-both-sides";

    fn any_cipher(c: Cipher) -> bool {
        c != Cipher::None
    }

    /// Runs `hello` through an acceptor and, if accepted, the reply through the dialer.
    async fn run(
        hello: &[u8],
        state: ClientState,
        acceptor: &Psk,
        replay: &ReplayFilter,
        allows: impl Fn(Cipher) -> bool,
    ) -> (io::Result<Accepted>, Option<io::Result<SessionKeys>>) {
        let (mut client, mut server) = tokio::io::duplex(4096);
        client.write_all(hello).await.unwrap();
        // Close our write side so a hello that claims more bytes than it has fails
        // instead of waiting forever.
        client.shutdown().await.unwrap();
        let accepted = accept(&mut server, acceptor, replay, allows, false).await;
        if accepted.is_err() {
            return (accepted, None);
        }
        let keys = state.read_reply(&mut client).await;
        (accepted, Some(keys))
    }

    #[tokio::test]
    async fn roundtrip_agrees_on_keys() {
        let psk = Psk::new(TOKEN);
        for cipher in [Cipher::Chacha20Poly1305, Cipher::Aes256Gcm, Cipher::None] {
            for early in [false, true] {
                let (hello, state) = client_hello(&psk, cipher, early, false).unwrap();
                let early_c = state.early_key().copied();
                let (acc, keys) =
                    run(&hello, state, &psk, &ReplayFilter::default(), |_| true).await;
                let acc = acc.unwrap();
                let keys = keys.unwrap().unwrap();
                assert_eq!(acc.keys.cipher, cipher);
                assert_eq!(keys.cipher, cipher);
                assert_eq!(acc.keys.c2s, keys.c2s);
                assert_eq!(acc.keys.s2c, keys.s2c);
                assert_ne!(keys.c2s, keys.s2c);
                assert_eq!(acc.early_key, early_c);
                assert_eq!(early_c.is_some(), early);
            }
        }
    }

    #[tokio::test]
    async fn sessions_get_fresh_keys() {
        let psk = Psk::new(TOKEN);
        let replay = ReplayFilter::default();
        let mut seen = Vec::new();
        for _ in 0..2 {
            let (hello, state) = client_hello(&psk, Cipher::Aes256Gcm, false, false).unwrap();
            let (_, keys) = run(&hello, state, &psk, &replay, any_cipher).await;
            seen.push(keys.unwrap().unwrap().c2s);
        }
        assert_ne!(seen[0], seen[1]);
    }

    #[tokio::test]
    async fn wrong_token_is_rejected_both_ways() {
        let a = Psk::new("token-a-token-a-token-a");
        let b = Psk::new("token-b-token-b-token-b");
        let (hello, state) = client_hello(&a, Cipher::Aes256Gcm, false, false).unwrap();
        let (acc, _) = run(&hello, state, &b, &ReplayFilter::default(), any_cipher).await;
        assert_eq!(acc.err().unwrap().kind(), io::ErrorKind::PermissionDenied);

        // A fake acceptor that does not know the token cannot produce a valid reply.
        let (_, state) = client_hello(&a, Cipher::Aes256Gcm, false, false).unwrap();
        let (fake_hello, _) = client_hello(&b, Cipher::Aes256Gcm, false, false).unwrap();
        let (mut client, mut server) = tokio::io::duplex(4096);
        client.write_all(&fake_hello).await.unwrap();
        accept(&mut server, &b, &ReplayFilter::default(), any_cipher, false)
            .await
            .unwrap();
        let err = state.read_reply(&mut client).await.err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn replay_is_rejected() {
        let psk = Psk::new(TOKEN);
        let replay = ReplayFilter::default();
        let (hello, state) = client_hello(&psk, Cipher::Aes256Gcm, true, false).unwrap();
        let (acc, _) = run(&hello, state, &psk, &replay, any_cipher).await;
        acc.unwrap();
        let (_, state) = client_hello(&psk, Cipher::Aes256Gcm, true, false).unwrap();
        let (acc, _) = run(&hello, state, &psk, &replay, any_cipher).await;
        assert!(acc.is_err());
    }

    #[tokio::test]
    async fn stale_timestamp_is_rejected() {
        let psk = Psk::new(TOKEN);
        for ts in [
            unix_now() - MAX_CLOCK_SKEW - 5,
            unix_now() + MAX_CLOCK_SKEW + 5,
        ] {
            let (hello, state) =
                client_hello_at(&psk, Cipher::Aes256Gcm, false, false, ts).unwrap();
            let (acc, _) = run(&hello, state, &psk, &ReplayFilter::default(), any_cipher).await;
            assert!(acc.is_err());
        }
    }

    #[tokio::test]
    async fn any_tampered_byte_is_rejected() {
        let psk = Psk::new(TOKEN);
        let (hello, _) = client_hello(&psk, Cipher::Aes256Gcm, false, false).unwrap();
        // Every header byte (including the cipher byte) and the tag.
        let positions = (0..HELLO_HEAD_LEN).chain(hello.len() - TAG_LEN..hello.len());
        for i in positions {
            let mut bad = hello.clone();
            bad[i] ^= 0x01;
            let (_, state) = client_hello(&psk, Cipher::Aes256Gcm, false, false).unwrap();
            let (acc, _) = run(&bad, state, &psk, &ReplayFilter::default(), any_cipher).await;
            assert!(acc.is_err(), "byte {i} tampered but accepted");
        }
    }

    #[tokio::test]
    async fn cipher_policy_is_enforced() {
        let psk = Psk::new(TOKEN);
        for (cipher, allowed) in [
            (Cipher::Aes256Gcm, Cipher::Chacha20Poly1305),
            (Cipher::None, Cipher::Aes256Gcm),
            (Cipher::Chacha20Poly1305, Cipher::None),
        ] {
            let (hello, state) = client_hello(&psk, cipher, false, false).unwrap();
            let (acc, _) = run(&hello, state, &psk, &ReplayFilter::default(), |c| {
                c == allowed
            })
            .await;
            assert_eq!(acc.err().unwrap().kind(), io::ErrorKind::Unsupported);
        }
    }

    #[tokio::test]
    async fn mux_mismatch_is_rejected_both_ways() {
        let psk = Psk::new(TOKEN);
        for (dialer, acceptor) in [(true, false), (false, true)] {
            let (hello, _) = client_hello(&psk, Cipher::Aes256Gcm, false, dialer).unwrap();
            let (mut client, mut server) = tokio::io::duplex(4096);
            client.write_all(&hello).await.unwrap();
            let err = accept(
                &mut server,
                &psk,
                &ReplayFilter::default(),
                any_cipher,
                acceptor,
            )
            .await
            .err()
            .unwrap();
            assert_eq!(err.kind(), io::ErrorKind::Unsupported);
            assert!(err.to_string().contains("mux"), "{err}");
        }
        // Agreeing on mux works.
        let (hello, state) = client_hello(&psk, Cipher::Aes256Gcm, false, true).unwrap();
        let (mut client, mut server) = tokio::io::duplex(4096);
        client.write_all(&hello).await.unwrap();
        accept(
            &mut server,
            &psk,
            &ReplayFilter::default(),
            any_cipher,
            true,
        )
        .await
        .unwrap();
        state.read_reply(&mut client).await.unwrap();
    }

    #[tokio::test]
    async fn random_bytes_are_rejected() {
        let psk = Psk::new(TOKEN);
        for _ in 0..50 {
            let junk: [u8; 600] = random_bytes().unwrap();
            let (mut client, mut server) = tokio::io::duplex(4096);
            client.write_all(&junk).await.unwrap();
            drop(client);
            let err = accept(
                &mut server,
                &psk,
                &ReplayFilter::default(),
                any_cipher,
                false,
            )
            .await
            .err()
            .unwrap();
            assert!(matches!(
                err.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::UnexpectedEof
            ));
        }
    }

    /// v1 hellos started with a plain timestamp; no offset may be (nearly) constant now.
    #[test]
    fn hello_bytes_look_random_at_every_offset() {
        let psk = Psk::new(TOKEN);
        let hellos: Vec<Vec<u8>> = (0..256)
            .map(|_| {
                client_hello(&psk, Cipher::Aes256Gcm, false, false)
                    .unwrap()
                    .0
            })
            .collect();
        let lengths: std::collections::HashSet<usize> = hellos.iter().map(Vec::len).collect();
        assert!(lengths.len() > 50, "hello length should vary");
        for offset in 0..HELLO_HEAD_LEN + TAG_LEN {
            let distinct: std::collections::HashSet<u8> =
                hellos.iter().map(|h| h[offset]).collect();
            // 256 uniform samples give ~162 distinct values on average.
            assert!(
                distinct.len() > 100,
                "offset {offset}: only {} values",
                distinct.len()
            );
        }
    }
}
