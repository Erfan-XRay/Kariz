//! Optional obfuscation of QUIC's UDP packets (`[tunnel.quic] obfs = true`).
//!
//! QUIC's first packets can be read by anyone (their keys come from public values), so a
//! middlebox can recognise the protocol and, with the SNI in the handshake, filter it. With
//! `obfs` every datagram is sealed under a key derived from the token before it leaves the
//! socket, and opened when it arrives, the same way KCP's packets are:
//!
//! ```text
//! nonce (12, random) | ChaCha20-Poly1305(QUIC datagram) | tag (16)
//! ```
//!
//! On the wire there is no header, version or length pattern to match, and the port
//! answers nothing that was not sealed with the token. QUIC's own TLS 1.3 runs inside, so
//! this layer's fixed key does not weaken the traffic's confidentiality. Both ends must
//! agree on `obfs`: one that does not speak it sees noise.
//!
//! It is a socket below quinn ([`quinn::AsyncUdpSocket`]), so quinn sees plain QUIC
//! datagrams. The seal makes each datagram [`OVERHEAD`] bytes longer, which the MTU
//! settings take into account (`quic.rs`). Segmentation offload (GSO / GRO) still works: a
//! batch is sealed and opened one datagram at a time.

use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{ready, Context, Poll};

use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305, NONCE_LEN};

use crate::crypto::handshake::Key;

const TAG_LEN: usize = 16;

/// Bytes the seal adds to every datagram: nonce and tag.
pub const OVERHEAD: usize = NONCE_LEN + TAG_LEN;

/// What `try_send` needs between calls: the nonce stream and a buffer for the sealed
/// datagrams, kept to avoid an allocation per send.
struct Sealer {
    nonces: blake3::OutputReader,
    out: Vec<u8>,
}

/// A UDP socket whose datagrams are sealed on the way out and opened on the way in.
pub struct ObfsSocket {
    inner: Arc<dyn AsyncUdpSocket>,
    key: LessSafeKey,
    send: Mutex<Sealer>,
}

impl fmt::Debug for ObfsSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ObfsSocket").finish_non_exhaustive()
    }
}

impl ObfsSocket {
    pub fn new(inner: Arc<dyn AsyncUdpSocket>, key: &Key) -> io::Result<Arc<Self>> {
        let key = UnboundKey::new(&CHACHA20_POLY1305, key).expect("32-byte key");
        // Random nonces from a BLAKE3 output stream seeded by the OS: as unpredictable as
        // asking the OS for every datagram, without a system call each.
        let seed = crate::crypto::random_bytes::<32>()?;
        Ok(Arc::new(Self {
            inner,
            key: LessSafeKey::new(key),
            send: Mutex::new(Sealer {
                nonces: blake3::Hasher::new_keyed(&seed).finalize_xof(),
                out: Vec::new(),
            }),
        }))
    }

    /// Opens a sealed datagram in place; `None` if it was not sealed with this key.
    fn open<'a>(&self, datagram: &'a mut [u8]) -> Option<&'a mut [u8]> {
        if datagram.len() < OVERHEAD {
            return None;
        }
        let (nonce, sealed) = datagram.split_at_mut(NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(nonce).ok()?;
        self.key.open_in_place(nonce, Aad::empty(), sealed).ok()
    }

    /// Opens every datagram of one receive (`buf[..meta.len]`, `meta.stride` apart when
    /// the kernel merged several) and packs the plain ones at the front. Datagrams that
    /// do not open are dropped. Returns false when none was left.
    fn open_batch(&self, buf: &mut [u8], meta: &mut RecvMeta) -> bool {
        let stride = meta.stride.max(1);
        let (mut read, mut write) = (0, 0);
        while read < meta.len {
            let end = (read + stride).min(meta.len);
            if let Some(plain) = self.open(&mut buf[read..end]) {
                let len = plain.len();
                buf.copy_within(read + NONCE_LEN..read + NONCE_LEN + len, write);
                write += len;
            }
            read = end;
        }
        meta.len = write;
        // Every datagram but the last is a full one, so the stride shrinks by the seal.
        meta.stride = stride.saturating_sub(OVERHEAD).max(1);
        write > 0
    }
}

impl AsyncUdpSocket for ObfsSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.inner.clone().create_io_poller()
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        let mut sealer = self.send.lock().unwrap_or_else(|e| e.into_inner());
        let Sealer { nonces, out } = &mut *sealer;
        out.clear();
        let segment = transmit
            .segment_size
            .unwrap_or(transmit.contents.len())
            .max(1);
        let mut segments = 0;
        for plain in transmit.contents.chunks(segment) {
            let start = out.len();
            let mut nonce = [0u8; NONCE_LEN];
            nonces.fill(&mut nonce);
            out.extend_from_slice(&nonce);
            out.extend_from_slice(plain);
            let tag = self
                .key
                .seal_in_place_separate_tag(
                    Nonce::assume_unique_for_key(nonce),
                    Aad::empty(),
                    &mut out[start + NONCE_LEN..],
                )
                .map_err(|_| io::Error::other("datagram too long to seal"))?;
            out.extend_from_slice(tag.as_ref());
            segments += 1;
        }
        self.inner.try_send(&Transmit {
            contents: out,
            segment_size: transmit
                .segment_size
                .filter(|_| segments > 1)
                .map(|s| s + OVERHEAD),
            ..transmit.clone()
        })
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        loop {
            let received = ready!(self.inner.poll_recv(cx, bufs, meta))?;
            // Receives with nothing of ours in them are dropped, the rest moved up.
            let mut kept = 0;
            for i in 0..received {
                if self.open_batch(&mut bufs[i], &mut meta[i]) {
                    meta.swap(kept, i);
                    bufs.swap(kept, i);
                    kept += 1;
                }
            }
            if kept > 0 {
                return Poll::Ready(Ok(kept));
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.inner.max_transmit_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.inner.max_receive_segments()
    }

    fn may_fragment(&self) -> bool {
        self.inner.may_fragment()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A socket that sends nothing and receives nothing, for its seal and open.
    #[derive(Debug)]
    struct Null;

    #[derive(Debug)]
    struct NullPoller;

    impl UdpPoller for NullPoller {
        fn poll_writable(self: Pin<&mut Self>, _: &mut Context) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncUdpSocket for Null {
        fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
            Box::pin(NullPoller)
        }

        fn try_send(&self, _: &Transmit) -> io::Result<()> {
            Ok(())
        }

        fn poll_recv(
            &self,
            _: &mut Context,
            _: &mut [IoSliceMut<'_>],
            _: &mut [RecvMeta],
        ) -> Poll<io::Result<usize>> {
            Poll::Pending
        }

        fn local_addr(&self) -> io::Result<SocketAddr> {
            Ok(([127, 0, 0, 1], 0).into())
        }
    }

    fn socket(key: u8) -> Arc<ObfsSocket> {
        ObfsSocket::new(Arc::new(Null), &[key; 32]).unwrap()
    }

    fn seal(s: &ObfsSocket, plain: &[u8]) -> Vec<u8> {
        let mut sealer = s.send.lock().unwrap();
        let mut nonce = [0u8; NONCE_LEN];
        sealer.nonces.fill(&mut nonce);
        let mut out = nonce.to_vec();
        out.extend_from_slice(plain);
        let tag = s
            .key
            .seal_in_place_separate_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::empty(),
                &mut out[NONCE_LEN..],
            )
            .unwrap();
        out.extend_from_slice(tag.as_ref());
        out
    }

    fn meta(len: usize, stride: usize) -> RecvMeta {
        RecvMeta {
            addr: ([127, 0, 0, 1], 9).into(),
            len,
            stride,
            ecn: None,
            dst_ip: None,
        }
    }

    #[test]
    fn a_sealed_datagram_opens_and_grows_by_the_overhead() {
        let s = socket(7);
        for len in [0, 1, 100, 1200, 1400] {
            let plain: Vec<u8> = (0..len).map(|i| i as u8).collect();
            let mut sealed = seal(&s, &plain);
            assert_eq!(sealed.len(), len + OVERHEAD);
            assert_eq!(s.open(&mut sealed).unwrap(), &plain[..]);
        }
    }

    #[test]
    fn another_key_or_any_change_is_rejected() {
        let (a, b) = (socket(7), socket(8));
        let sealed = seal(&a, b"a quic datagram");
        assert!(b.open(&mut sealed.clone()).is_none());
        for i in 0..sealed.len() {
            let mut tampered = sealed.clone();
            tampered[i] ^= 1;
            assert!(a.open(&mut tampered).is_none(), "byte {i} changed");
        }
        assert!(a.open(&mut [0u8; OVERHEAD - 1]).is_none());
    }

    #[test]
    fn a_merged_receive_is_opened_per_datagram() {
        let s = socket(7);
        // Three datagrams the kernel merged (GRO): two full ones and a shorter last.
        let plains: [&[u8]; 3] = [&[1; 50], &[2; 50], &[3; 20]];
        let mut buf: Vec<u8> = plains.iter().flat_map(|p| seal(&s, p)).collect();
        let sealed_len = buf.len();
        let mut m = meta(sealed_len, 50 + OVERHEAD);
        assert!(s.open_batch(&mut buf, &mut m));
        assert_eq!((m.len, m.stride), (120, 50));
        assert_eq!(&buf[..120], plains.concat());
    }

    #[test]
    fn a_receive_without_one_of_ours_is_dropped() {
        let s = socket(7);
        // A datagram that is not sealed is dropped, the one after it is kept.
        let mut buf = vec![9u8; 60 + OVERHEAD];
        buf.extend(seal(&s, &[4; 60]));
        let mut m = meta(buf.len(), 60 + OVERHEAD);
        assert!(s.open_batch(&mut buf, &mut m));
        assert_eq!((m.len, m.stride), (60, 60));
        assert_eq!(&buf[..60], &[4; 60]);
        let mut noise = vec![9u8; 100];
        let mut m = meta(100, 100);
        assert!(!s.open_batch(&mut noise, &mut m));
    }
}
