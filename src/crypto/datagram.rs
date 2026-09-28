//! Datagram sealing: user datagrams that travel beside the record layer instead of
//! inside it (the datagram path over KCP, docs/PHASE6.md section 3).
//!
//! ```text
//! datagram = pn (8, LE) | seal(k, pn, payload)     -> 8 + payload + 16 bytes
//! k        = BLAKE3-derive-key("kariz v2 dgram", record key of the direction)
//! pn       = packet number per direction, from 0; the nonce is pn as in records
//! ```
//!
//! The keys come from the handshake's record keys, so datagrams keep forward secrecy.
//! Unlike records, datagrams may be lost or reordered: each carries its own number, and
//! the receiver keeps a window of the numbers it has seen, which drops replays and the
//! copies that packet duplication sends. With `encryption = "none"` the number is kept
//! (for the window) and the payload is sent as is.

use std::io;

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, NONCE_LEN};

use super::handshake::Key;
use super::record::algorithm;
use super::Cipher;

const CONTEXT: &str = "kariz v2 dgram";
const NUMBER_LEN: usize = 8;
const TAG_LEN: usize = 16;
/// Numbers this far behind the highest one seen are too old to tell from replays.
const WINDOW: u64 = 1024;

fn datagram_key(cipher: Cipher, record_key: &Key) -> io::Result<Option<LessSafeKey>> {
    if cipher == Cipher::None {
        return Ok(None);
    }
    let mut h = blake3::Hasher::new_derive_key(CONTEXT);
    h.update(record_key);
    let key = UnboundKey::new(algorithm(cipher)?, h.finalize().as_bytes())
        .map_err(|_| io::Error::other("invalid datagram key"))?;
    Ok(Some(LessSafeKey::new(key)))
}

fn nonce(number: u64) -> Nonce {
    let mut nonce = [0u8; NONCE_LEN];
    nonce[..8].copy_from_slice(&number.to_le_bytes());
    Nonce::assume_unique_for_key(nonce)
}

/// Bytes a sealed datagram adds to its payload.
pub fn overhead(cipher: Cipher) -> usize {
    match cipher {
        Cipher::None => NUMBER_LEN,
        _ => NUMBER_LEN + TAG_LEN,
    }
}

/// Seals datagrams for one direction.
pub struct DatagramSealer {
    key: Option<LessSafeKey>,
    next: u64,
}

impl DatagramSealer {
    /// `record_key` is the record layer's key for the same direction.
    pub fn new(cipher: Cipher, record_key: &Key) -> io::Result<Self> {
        Ok(Self {
            key: datagram_key(cipher, record_key)?,
            next: 0,
        })
    }

    /// Appends one sealed datagram carrying `payload` to `out`. Copies of a packet are
    /// the same sealed bytes sent again, so they share its number.
    pub fn seal(&mut self, payload: &[u8], out: &mut Vec<u8>) -> io::Result<()> {
        let number = self.next;
        self.next = number
            .checked_add(1)
            .ok_or_else(|| io::Error::other("datagram counter exhausted"))?;
        out.extend_from_slice(&number.to_le_bytes());
        let start = out.len();
        out.extend_from_slice(payload);
        if let Some(key) = &self.key {
            let tag = key
                .seal_in_place_separate_tag(nonce(number), Aad::empty(), &mut out[start..])
                .map_err(|_| io::Error::other("seal failed"))?;
            out.extend_from_slice(tag.as_ref());
        }
        Ok(())
    }
}

/// Opens datagrams for one direction and drops replays and copies.
pub struct DatagramOpener {
    key: Option<LessSafeKey>,
    window: ReplayWindow,
}

impl DatagramOpener {
    /// `record_key` is the record layer's key for the same direction.
    pub fn new(cipher: Cipher, record_key: &Key) -> io::Result<Self> {
        Ok(Self {
            key: datagram_key(cipher, record_key)?,
            window: ReplayWindow::default(),
        })
    }

    /// Opens `datagram` in place and returns its payload; `None` if it fails
    /// authentication, was seen before, or is too old to tell.
    pub fn open<'a>(&mut self, datagram: &'a mut [u8]) -> Option<&'a mut [u8]> {
        if datagram.len() < NUMBER_LEN {
            return None;
        }
        let (number, body) = datagram.split_at_mut(NUMBER_LEN);
        let number = u64::from_le_bytes(number.try_into().ok()?);
        if !self.window.is_new(number) {
            return None;
        }
        let payload = match &self.key {
            Some(key) => key.open_in_place(nonce(number), Aad::empty(), body).ok()?,
            None => body,
        };
        // Only after authentication, so a forged number cannot move the window.
        self.window.insert(number);
        Some(payload)
    }
}

/// The numbers seen among the last [`WINDOW`] up to the highest one: bit `n % WINDOW`.
/// Also drops copies of duplicated UDP packets (`src/udp.rs`).
#[derive(Default)]
pub(crate) struct ReplayWindow {
    /// One more than the highest number seen; 0 before the first.
    end: u64,
    bits: [u64; (WINDOW / 64) as usize],
}

impl ReplayWindow {
    fn bit(n: u64) -> (usize, u64) {
        let i = n % WINDOW;
        ((i / 64) as usize, 1 << (i % 64))
    }

    /// One more than the highest number seen; 0 before the first.
    pub(crate) fn end(&self) -> u64 {
        self.end
    }

    pub(crate) fn is_new(&self, n: u64) -> bool {
        if n >= self.end {
            return true;
        }
        if self.end - n > WINDOW {
            return false;
        }
        let (word, mask) = Self::bit(n);
        self.bits[word] & mask == 0
    }

    pub(crate) fn insert(&mut self, n: u64) {
        if n >= self.end {
            // Numbers skipped on the way to `n` have not been seen: clear their bits,
            // which still hold numbers a whole window older.
            if n - self.end >= WINDOW {
                self.bits = Default::default();
            } else {
                for skipped in self.end..n {
                    let (word, mask) = Self::bit(skipped);
                    self.bits[word] &= !mask;
                }
            }
            self.end = n + 1;
        }
        let (word, mask) = Self::bit(n);
        self.bits[word] |= mask;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const K1: Key = [1u8; 32];
    const K2: Key = [2u8; 32];
    const CIPHERS: [Cipher; 3] = [Cipher::Chacha20Poly1305, Cipher::Aes256Gcm, Cipher::None];

    fn sealed(sealer: &mut DatagramSealer, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        sealer.seal(payload, &mut out).unwrap();
        out
    }

    fn pair(cipher: Cipher) -> (DatagramSealer, DatagramOpener) {
        (
            DatagramSealer::new(cipher, &K1).unwrap(),
            DatagramOpener::new(cipher, &K1).unwrap(),
        )
    }

    #[test]
    fn roundtrip_every_cipher_and_size() {
        for cipher in CIPHERS {
            let (mut s, mut o) = pair(cipher);
            for len in [0, 1, 100, 1300, 65_535] {
                let payload: Vec<u8> = (0..len).map(|i| (i * 7 % 256) as u8).collect();
                let mut d = sealed(&mut s, &payload);
                assert_eq!(d.len(), len + overhead(cipher));
                assert_eq!(o.open(&mut d).as_deref(), Some(&payload[..]), "{cipher:?}");
            }
        }
    }

    #[test]
    fn every_changed_byte_is_rejected() {
        for cipher in [Cipher::Chacha20Poly1305, Cipher::Aes256Gcm] {
            let (mut s, _) = pair(cipher);
            let d = sealed(&mut s, b"game state");
            for i in 0..d.len() {
                let mut bad = d.clone();
                bad[i] ^= 0x01;
                let mut o = DatagramOpener::new(cipher, &K1).unwrap();
                assert!(o.open(&mut bad).is_none(), "{cipher:?}, byte {i}");
            }
        }
    }

    #[test]
    fn truncated_and_other_keys_are_rejected() {
        for cipher in [Cipher::Chacha20Poly1305, Cipher::Aes256Gcm] {
            let (mut s, mut o) = pair(cipher);
            let d = sealed(&mut s, b"x");
            for len in 0..d.len() {
                assert!(o.open(&mut d[..len].to_vec()).is_none());
            }
            let mut other = DatagramOpener::new(cipher, &K2).unwrap();
            assert!(other.open(&mut d.clone()).is_none());
            // Not the record key itself either: datagram keys are derived from it.
            let mut record = Vec::new();
            crate::crypto::record::Sealer::new(cipher, &K1)
                .unwrap()
                .seal(b"x", &mut record)
                .unwrap();
            assert!(o.open(&mut record).is_none());
        }
    }

    #[test]
    fn replays_and_copies_are_dropped() {
        for cipher in CIPHERS {
            let (mut s, mut o) = pair(cipher);
            let d = sealed(&mut s, b"once");
            assert!(o.open(&mut d.clone()).is_some());
            assert!(o.open(&mut d.clone()).is_none(), "{cipher:?}");
            assert!(o.open(&mut d.clone()).is_none(), "{cipher:?}");
        }
    }

    #[test]
    fn reordering_within_the_window_is_accepted() {
        let (mut s, mut o) = pair(Cipher::Chacha20Poly1305);
        let all: Vec<Vec<u8>> = (0..3000).map(|i| sealed(&mut s, &[i as u8])).collect();
        let mut open = |n: usize| o.open(&mut all[n].clone()).is_some();
        // Newest first, a whole window: all arrive, once.
        for n in (0..WINDOW as usize).rev() {
            assert!(open(n), "{n}");
        }
        for n in 0..WINDOW as usize {
            assert!(!open(n), "{n}");
        }
        // A jump ahead, then late ones just inside the window and just outside it.
        assert!(open(2500));
        assert!(open(2501 - WINDOW as usize));
        assert!(!open(2500 - WINDOW as usize));
        // A shorter jump: the numbers skipped on the way are new, even where their bits
        // held numbers a window older (1500 and 2524 share one).
        assert!(open(1500));
        assert!(open(2600));
        assert!(open(1500 + WINDOW as usize));
        assert!(!open(1500 + WINDOW as usize));
    }

    #[test]
    fn a_jump_past_the_window_forgets_everything_older() {
        let mut w = ReplayWindow::default();
        for n in 0..10 {
            w.insert(n);
        }
        w.insert(5000);
        assert!(!w.is_new(5000));
        assert!(!w.is_new(9));
        for n in 5000 - WINDOW + 1..5000 {
            assert!(w.is_new(n), "{n}");
        }
        assert!(!w.is_new(5000 - WINDOW));
    }

    #[test]
    fn a_forged_number_does_not_move_the_window() {
        let (mut s, mut o) = pair(Cipher::Aes256Gcm);
        let first = sealed(&mut s, b"a");
        let mut forged = sealed(&mut s, b"b");
        forged[..8].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(o.open(&mut forged).is_none());
        assert!(o.open(&mut first.clone()).is_some());
    }
}
