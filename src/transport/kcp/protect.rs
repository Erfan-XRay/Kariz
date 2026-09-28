//! Packet protection for KCP: every UDP packet is sealed with a key derived from the
//! token, so the port answers nothing that was not made with the token, and KCP's
//! otherwise plain headers (conversation, sequence numbers, window) are hidden.
//!
//! ```text
//! nonce (12, random) | ChaCha20-Poly1305(type (1) | body) | tag (16)
//! ```
//!
//! The tunnel's handshake and record layer still run inside KCP, so this layer's key
//! being fixed (no forward secrecy here) does not weaken the traffic's confidentiality.
//! ChaCha20-Poly1305 rather than AES-GCM: nonces are random under one long-lived key,
//! and if two ever collided, ChaCha20-Poly1305 gives away only that one packet's
//! authentication key where GCM would give away the key for every packet.

use std::fmt;
use std::io;

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305, NONCE_LEN};

use crate::crypto::handshake::Key;

const TAG_LEN: usize = 16;

/// Bytes a sealed packet adds: nonce and tag.
pub const OVERHEAD: usize = NONCE_LEN + TAG_LEN;

/// Seals and opens packets.
pub struct Protection {
    key: LessSafeKey,
}

impl fmt::Debug for Protection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Protection")
    }
}

impl Protection {
    pub fn new(key: &Key) -> Self {
        let key = UnboundKey::new(&CHACHA20_POLY1305, key).expect("32-byte key");
        Self {
            key: LessSafeKey::new(key),
        }
    }

    /// Seals `plain` into `out` (replacing what it held).
    pub fn seal(&self, nonces: &mut Nonces, plain: &[u8], out: &mut Vec<u8>) {
        let mut nonce = [0u8; NONCE_LEN];
        nonces.fill(&mut nonce);
        out.clear();
        out.extend_from_slice(&nonce);
        out.extend_from_slice(plain);
        let tag = self
            .key
            .seal_in_place_separate_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::empty(),
                &mut out[NONCE_LEN..],
            )
            .expect("packet within AEAD limits");
        out.extend_from_slice(tag.as_ref());
    }

    /// Opens `packet` in place and returns the plaintext, or `None` if it was not sealed
    /// with this key (or was changed on the way).
    pub fn open<'a>(&self, packet: &'a mut [u8]) -> Option<&'a mut [u8]> {
        if packet.len() < OVERHEAD {
            return None;
        }
        let (nonce, sealed) = packet.split_at_mut(NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(nonce).ok()?;
        self.key.open_in_place(nonce, Aad::empty(), sealed).ok()
    }
}

/// Random nonces from a BLAKE3 output stream seeded by the OS: as unpredictable as
/// asking the OS for every packet, without a system call per packet.
pub struct Nonces(blake3::OutputReader);

impl Nonces {
    pub fn new() -> io::Result<Self> {
        let seed = crate::crypto::random_bytes::<32>()?;
        Ok(Self(blake3::Hasher::new_keyed(&seed).finalize_xof()))
    }

    fn fill(&mut self, out: &mut [u8]) {
        self.0.fill(out);
    }

    /// A random `u32` (conversation ids).
    pub fn next_u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        self.0.fill(&mut b);
        u32::from_le_bytes(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (Protection, Nonces) {
        (Protection::new(&[7u8; 32]), Nonces::new().unwrap())
    }

    #[test]
    fn seal_then_open() {
        let (p, mut n) = pair();
        let mut sealed = Vec::new();
        for len in [0, 1, 100, 1400] {
            let plain: Vec<u8> = (0..len).map(|i| i as u8).collect();
            p.seal(&mut n, &plain, &mut sealed);
            assert_eq!(sealed.len(), len + OVERHEAD);
            assert_eq!(p.open(&mut sealed.clone()).unwrap(), &plain[..]);
        }
    }

    #[test]
    fn same_plaintext_seals_differently() {
        let (p, mut n) = pair();
        let (mut a, mut b) = (Vec::new(), Vec::new());
        p.seal(&mut n, b"hello", &mut a);
        p.seal(&mut n, b"hello", &mut b);
        assert_ne!(a, b);
        assert_ne!(a[..NONCE_LEN], b[..NONCE_LEN]);
    }

    #[test]
    fn any_change_is_rejected() {
        let (p, mut n) = pair();
        let mut sealed = Vec::new();
        p.seal(&mut n, b"some kcp segment", &mut sealed);
        for i in 0..sealed.len() {
            let mut tampered = sealed.clone();
            tampered[i] ^= 0x01;
            assert!(p.open(&mut tampered).is_none(), "byte {i} changed");
        }
        assert!(p.open(&mut sealed[..sealed.len() - 1].to_vec()).is_none());
        assert!(p.open(&mut [0u8; OVERHEAD - 1]).is_none());
    }

    #[test]
    fn another_key_is_rejected() {
        let (p, mut n) = pair();
        let mut sealed = Vec::new();
        p.seal(&mut n, b"segment", &mut sealed);
        assert!(Protection::new(&[8u8; 32]).open(&mut sealed).is_none());
    }
}
