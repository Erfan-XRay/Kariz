//! Record layer: turns a byte stream into a sequence of AEAD records.
//!
//! ```text
//! record = seal(k, n,   len (2, BE))  -> 2 + 16 bytes
//!        | seal(k, n+1, payload)      -> len + 16 bytes
//! len    <= 16383 (the top 2 bits are reserved and must be zero)
//! n      =  96-bit little-endian counter per direction, starting at 0
//! ```
//!
//! Each direction has its own key, so a counter never repeats under the same key.
//! Reordered, replayed, truncated or modified records fail to open and end the stream.

use std::io;
use std::pin::Pin;
use std::task::{ready, Context, Poll};

use ring::aead::{self, Aad, LessSafeKey, Nonce, UnboundKey};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::handshake::Key;
use super::Cipher;

/// Largest payload in one record.
pub const MAX_PAYLOAD: usize = 0x3fff;
const AEAD_TAG_LEN: usize = 16;
const HEADER_LEN: usize = 2 + AEAD_TAG_LEN;
/// Size of the largest record on the wire.
pub const MAX_RECORD: usize = HEADER_LEN + MAX_PAYLOAD + AEAD_TAG_LEN;
/// Plaintext sealed per `poll_write` call, so bulk writes need fewer syscalls.
const WRITE_BATCH: usize = 4 * MAX_PAYLOAD;
/// Ciphertext read buffer: room for a few records per read.
const READ_BUFFER: usize = 4 * MAX_RECORD;

fn algorithm(cipher: Cipher) -> io::Result<&'static aead::Algorithm> {
    match cipher {
        Cipher::Chacha20Poly1305 => Ok(&aead::CHACHA20_POLY1305),
        Cipher::Aes256Gcm => Ok(&aead::AES_256_GCM),
        Cipher::None => Err(io::Error::other("the record layer needs a cipher")),
    }
}

/// One direction's key and nonce counter.
struct Direction {
    key: LessSafeKey,
    counter: u64,
}

impl Direction {
    fn new(cipher: Cipher, key: &Key) -> io::Result<Self> {
        let key = UnboundKey::new(algorithm(cipher)?, key)
            .map_err(|_| io::Error::other("invalid record key"))?;
        Ok(Self {
            key: LessSafeKey::new(key),
            counter: 0,
        })
    }

    fn next_nonce(&mut self) -> io::Result<Nonce> {
        let n = self.counter;
        self.counter = n
            .checked_add(1)
            .ok_or_else(|| io::Error::other("record counter exhausted"))?;
        let mut nonce = [0u8; aead::NONCE_LEN];
        nonce[..8].copy_from_slice(&n.to_le_bytes());
        Ok(Nonce::assume_unique_for_key(nonce))
    }
}

/// Seals records for one direction.
pub struct Sealer(Direction);

impl Sealer {
    pub fn new(cipher: Cipher, key: &Key) -> io::Result<Self> {
        Direction::new(cipher, key).map(Self)
    }

    /// Appends one record carrying `payload` (at most [`MAX_PAYLOAD`] bytes) to `out`.
    pub fn seal(&mut self, payload: &[u8], out: &mut Vec<u8>) -> io::Result<()> {
        debug_assert!(payload.len() <= MAX_PAYLOAD);
        let d = &mut self.0;
        let start = out.len();
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        let nonce = d.next_nonce()?;
        let tag = d
            .key
            .seal_in_place_separate_tag(nonce, Aad::empty(), &mut out[start..])
            .map_err(|_| io::Error::other("seal failed"))?;
        out.extend_from_slice(tag.as_ref());

        let start = out.len();
        out.extend_from_slice(payload);
        let nonce = d.next_nonce()?;
        let tag = d
            .key
            .seal_in_place_separate_tag(nonce, Aad::empty(), &mut out[start..])
            .map_err(|_| io::Error::other("seal failed"))?;
        out.extend_from_slice(tag.as_ref());
        Ok(())
    }
}

/// Opens records for one direction.
pub struct Opener(Direction);

impl Opener {
    pub fn new(cipher: Cipher, key: &Key) -> io::Result<Self> {
        Direction::new(cipher, key).map(Self)
    }

    /// Opens a record header in place and returns the payload length.
    fn open_header(&mut self, header: &mut [u8]) -> io::Result<usize> {
        let d = &mut self.0;
        let nonce = d.next_nonce()?;
        let plain = d
            .key
            .open_in_place(nonce, Aad::empty(), header)
            .map_err(|_| bad_record())?;
        let len = u16::from_be_bytes([plain[0], plain[1]]) as usize;
        if len > MAX_PAYLOAD {
            return Err(bad_record());
        }
        Ok(len)
    }

    /// Opens a record payload (ciphertext plus tag) in place.
    fn open_payload(&mut self, payload: &mut [u8]) -> io::Result<()> {
        let d = &mut self.0;
        let nonce = d.next_nonce()?;
        d.key
            .open_in_place(nonce, Aad::empty(), payload)
            .map_err(|_| bad_record())?;
        Ok(())
    }
}

fn bad_record() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "tunnel record failed authentication",
    )
}

/// An encrypted stream on top of `S`.
pub struct SecureStream<S> {
    inner: S,
    sealer: Sealer,
    opener: Opener,
    /// Opener for an early-data record that precedes everything else, if one was announced.
    early: Option<Opener>,

    /// Sealed records not yet written to `inner`, from `wpos`.
    wbuf: Vec<u8>,
    wpos: usize,

    /// Ciphertext read from `inner`, valid in `rstart..rend`. Allocated on first read.
    rbuf: Vec<u8>,
    rstart: usize,
    rend: usize,
    /// Decrypted bytes not yet handed out, in `rbuf[plain_start..plain_end]`.
    plain_start: usize,
    plain_end: usize,
    /// Payload length of the record whose header has been opened.
    pending_len: Option<usize>,
    read_eof: bool,
}

impl<S> SecureStream<S> {
    pub fn new(inner: S, sealer: Sealer, opener: Opener, early: Option<Opener>) -> Self {
        Self {
            inner,
            sealer,
            opener,
            early,
            wbuf: Vec::new(),
            wpos: 0,
            rbuf: Vec::new(),
            rstart: 0,
            rend: 0,
            plain_start: 0,
            plain_end: 0,
            pending_len: None,
            read_eof: false,
        }
    }

    pub fn get_ref(&self) -> &S {
        &self.inner
    }
}

impl<S: AsyncWrite + Unpin> SecureStream<S> {
    /// Writes out sealed records that are still buffered.
    fn poll_write_buffered(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.wpos < self.wbuf.len() {
            let n = ready!(Pin::new(&mut self.inner).poll_write(cx, &self.wbuf[self.wpos..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.wpos += n;
        }
        self.wbuf.clear();
        self.wpos = 0;
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for SecureStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.plain_start < this.plain_end {
                let n = (this.plain_end - this.plain_start).min(buf.remaining());
                buf.put_slice(&this.rbuf[this.plain_start..this.plain_start + n]);
                this.plain_start += n;
                return Poll::Ready(Ok(()));
            }

            let need = match this.pending_len {
                None => HEADER_LEN,
                Some(len) => len + AEAD_TAG_LEN,
            };
            if this.rend - this.rstart < need {
                if this.read_eof {
                    return if this.rstart == this.rend && this.pending_len.is_none() {
                        Poll::Ready(Ok(()))
                    } else {
                        Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "tunnel closed in the middle of a record",
                        )))
                    };
                }
                if this.rbuf.is_empty() {
                    this.rbuf = vec![0u8; READ_BUFFER];
                }
                if this.rstart + need > this.rbuf.len() {
                    this.rbuf.copy_within(this.rstart..this.rend, 0);
                    this.rend -= this.rstart;
                    this.rstart = 0;
                }
                let mut rb = ReadBuf::new(&mut this.rbuf[this.rend..]);
                ready!(Pin::new(&mut this.inner).poll_read(cx, &mut rb))?;
                let n = rb.filled().len();
                if n == 0 {
                    this.read_eof = true;
                }
                this.rend += n;
                continue;
            }

            let opener = this.early.as_mut().unwrap_or(&mut this.opener);
            let record = &mut this.rbuf[this.rstart..this.rstart + need];
            match this.pending_len {
                None => this.pending_len = Some(opener.open_header(record)?),
                Some(len) => {
                    opener.open_payload(record)?;
                    this.early = None;
                    this.pending_len = None;
                    this.plain_start = this.rstart;
                    this.plain_end = this.rstart + len;
                }
            }
            this.rstart += need;
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for SecureStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        ready!(this.poll_write_buffered(cx))?;
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let take = buf.len().min(WRITE_BATCH);
        for chunk in buf[..take].chunks(MAX_PAYLOAD) {
            this.sealer.seal(chunk, &mut this.wbuf)?;
        }
        // Start sending right away; whatever does not fit goes out on the next write or
        // flush. The data is ours now either way.
        if let Poll::Ready(Err(e)) = this.poll_write_buffered(cx) {
            return Poll::Ready(Err(e));
        }
        Poll::Ready(Ok(take))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_write_buffered(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_write_buffered(cx))?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    const K1: Key = [1u8; 32];
    const K2: Key = [2u8; 32];

    fn pair(
        cipher: Cipher,
        pipe: usize,
    ) -> (SecureStream<DuplexStream>, SecureStream<DuplexStream>) {
        let (a, b) = tokio::io::duplex(pipe);
        let a = SecureStream::new(
            a,
            Sealer::new(cipher, &K1).unwrap(),
            Opener::new(cipher, &K2).unwrap(),
            None,
        );
        let b = SecureStream::new(
            b,
            Sealer::new(cipher, &K2).unwrap(),
            Opener::new(cipher, &K1).unwrap(),
            None,
        );
        (a, b)
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    /// Sends `data` from `a` to `b` in writes of `chunk` bytes, reads it back in
    /// reads of `read_chunk` bytes.
    async fn transfer(
        mut a: SecureStream<DuplexStream>,
        mut b: SecureStream<DuplexStream>,
        data: Vec<u8>,
        chunk: usize,
    ) -> Vec<u8> {
        let writer = tokio::spawn(async move {
            for c in data.chunks(chunk) {
                a.write_all(c).await.unwrap();
            }
            a.shutdown().await.unwrap();
            a
        });
        let mut got = Vec::new();
        b.read_to_end(&mut got).await.unwrap();
        writer.await.unwrap();
        got
    }

    #[tokio::test]
    async fn roundtrip_both_ciphers_any_split() {
        for cipher in [Cipher::Chacha20Poly1305, Cipher::Aes256Gcm] {
            // Tiny pipes force records to be split across reads and writes.
            for (pipe, chunk, len) in [
                (7, 1, 3_000),
                (64, 1000, 100_000),
                (1 << 16, 200_000, 300_000),
                (3, 70_000, 100_000),
            ] {
                let (a, b) = pair(cipher, pipe);
                let data = pattern(len);
                assert_eq!(
                    transfer(a, b, data.clone(), chunk).await,
                    data,
                    "{cipher:?} {pipe} {chunk}"
                );
            }
        }
    }

    #[tokio::test]
    async fn both_directions_at_once() {
        let (mut a, mut b) = pair(Cipher::Chacha20Poly1305, 1024);
        let data = pattern(100_000);
        let d = data.clone();
        let t = tokio::spawn(async move {
            let (mut r, mut w) = tokio::io::split(&mut b);
            let mut got = vec![0u8; d.len()];
            let write = async {
                w.write_all(&d).await?;
                w.flush().await
            };
            let (x, y) = tokio::join!(write, r.read_exact(&mut got));
            x.unwrap();
            y.unwrap();
            got
        });
        let (mut r, mut w) = tokio::io::split(&mut a);
        let mut got = vec![0u8; data.len()];
        let write = async {
            w.write_all(&data).await?;
            w.flush().await
        };
        let (x, y) = tokio::join!(write, r.read_exact(&mut got));
        x.unwrap();
        y.unwrap();
        assert_eq!(got, data);
        assert_eq!(t.await.unwrap(), data);
    }

    /// Seals `payloads` as consecutive records with `key`.
    fn records(key: &Key, payloads: &[&[u8]]) -> Vec<Vec<u8>> {
        let mut s = Sealer::new(Cipher::Aes256Gcm, key).unwrap();
        payloads
            .iter()
            .map(|p| {
                let mut out = Vec::new();
                s.seal(p, &mut out).unwrap();
                out
            })
            .collect()
    }

    async fn read_wire(wire: &[u8], early: Option<&Key>) -> io::Result<Vec<u8>> {
        let (mut raw, b) = tokio::io::duplex(1 << 20);
        raw.write_all(wire).await.unwrap();
        drop(raw);
        let mut b = SecureStream::new(
            b,
            Sealer::new(Cipher::Aes256Gcm, &K2).unwrap(),
            Opener::new(Cipher::Aes256Gcm, &K1).unwrap(),
            early.map(|k| Opener::new(Cipher::Aes256Gcm, k).unwrap()),
        );
        let mut got = Vec::new();
        b.read_to_end(&mut got).await.map(|_| got)
    }

    #[tokio::test]
    async fn detects_tampering_reordering_truncation_and_wrong_key() {
        let r = records(&K1, &[b"first", b"second"]);
        let good = r.concat();
        assert_eq!(read_wire(&good, None).await.unwrap(), b"firstsecond");

        for i in 0..good.len() {
            let mut bad = good.clone();
            bad[i] ^= 0x80;
            assert!(
                read_wire(&bad, None).await.is_err(),
                "byte {i} flipped but accepted"
            );
        }
        let swapped = [r[1].clone(), r[0].clone()].concat();
        assert!(read_wire(&swapped, None).await.is_err());
        let replayed = [r[0].clone(), r[0].clone()].concat();
        assert!(read_wire(&replayed, None).await.is_err());
        let err = read_wire(&good[..good.len() - 1], None).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        assert!(read_wire(&records(&K2, &[b"x"]).concat(), None)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn early_record_uses_its_own_key() {
        const EARLY: Key = [9u8; 32];
        let wire = [
            records(&EARLY, &[b"early "]).concat(),
            records(&K1, &[b"late"]).concat(),
        ]
        .concat();
        assert_eq!(read_wire(&wire, Some(&EARLY)).await.unwrap(), b"early late");
        assert!(read_wire(&wire, None).await.is_err());
    }

    #[test]
    fn overhead_is_small() {
        let r = records(&K1, &[&[0u8; MAX_PAYLOAD]]);
        assert_eq!(r[0].len(), MAX_RECORD);
        assert!((MAX_RECORD - MAX_PAYLOAD) as f64 / (MAX_PAYLOAD as f64) < 0.0025);
    }

    #[tokio::test]
    async fn empty_stream_is_a_clean_eof() {
        assert!(read_wire(&[], None).await.unwrap().is_empty());
    }
}
