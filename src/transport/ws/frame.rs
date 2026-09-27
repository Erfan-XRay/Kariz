//! WebSocket frame headers and masking (RFC 6455, section 5.2).
//!
//! ```text
//! FIN (1) | RSV (3) | opcode (4) | MASK (1) | len (7) | [len (16 or 64, BE)] | [mask key (4)]
//! ```

use std::io;

pub const OP_CONTINUATION: u8 = 0x0;
pub const OP_TEXT: u8 = 0x1;
pub const OP_BINARY: u8 = 0x2;
pub const OP_CLOSE: u8 = 0x8;
pub const OP_PING: u8 = 0x9;
pub const OP_PONG: u8 = 0xa;

/// Largest possible header: 2 bytes, 64-bit length, mask key.
pub const MAX_HEADER_LEN: usize = 14;
/// Control frames (close, ping, pong) carry at most this many payload bytes.
pub const MAX_CONTROL_PAYLOAD: usize = 125;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub fin: bool,
    pub opcode: u8,
    pub mask: Option<[u8; 4]>,
    pub len: u64,
}

impl Header {
    pub fn is_control(&self) -> bool {
        self.opcode & 0x8 != 0
    }

    /// Parses a header from the start of `buf`. Returns the header and its encoded
    /// length, or `None` when `buf` does not hold the whole header yet.
    pub fn decode(buf: &[u8]) -> io::Result<Option<(Self, usize)>> {
        let [b0, b1, ..] = *buf else {
            return Ok(None);
        };
        if b0 & 0x70 != 0 {
            return Err(invalid(
                "websocket frame with RSV bits set (no extension was agreed)",
            ));
        }
        let fin = b0 & 0x80 != 0;
        let opcode = b0 & 0x0f;
        if !matches!(
            opcode,
            OP_CONTINUATION | OP_TEXT | OP_BINARY | OP_CLOSE | OP_PING | OP_PONG
        ) {
            return Err(invalid("websocket frame with a reserved opcode"));
        }
        let masked = b1 & 0x80 != 0;
        let (len, mut pos) = match b1 & 0x7f {
            126 => match buf.get(2..4) {
                Some(b) => (u16::from_be_bytes([b[0], b[1]]) as u64, 4),
                None => return Ok(None),
            },
            127 => match buf.get(2..10) {
                Some(b) => (u64::from_be_bytes(b.try_into().expect("8 bytes")), 10),
                None => return Ok(None),
            },
            n => (n as u64, 2),
        };
        if len >> 63 != 0 {
            return Err(invalid("websocket frame length has the top bit set"));
        }
        let mask = if masked {
            let Some(key) = buf.get(pos..pos + 4) else {
                return Ok(None);
            };
            pos += 4;
            Some(key.try_into().expect("4 bytes"))
        } else {
            None
        };
        let header = Self {
            fin,
            opcode,
            mask,
            len,
        };
        if header.is_control() && (!fin || len > MAX_CONTROL_PAYLOAD as u64) {
            return Err(invalid(
                "websocket control frame is fragmented or longer than 125 bytes",
            ));
        }
        Ok(Some((header, pos)))
    }

    /// Appends the encoded header to `out`, using the shortest length encoding.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.push(if self.fin { 0x80 } else { 0 } | self.opcode);
        let mask_bit = if self.mask.is_some() { 0x80 } else { 0 };
        match self.len {
            0..=125 => out.push(mask_bit | self.len as u8),
            126..=0xffff => {
                out.push(mask_bit | 126);
                out.extend_from_slice(&(self.len as u16).to_be_bytes());
            }
            _ => {
                out.push(mask_bit | 127);
                out.extend_from_slice(&self.len.to_be_bytes());
            }
        }
        if let Some(key) = self.mask {
            out.extend_from_slice(&key);
        }
    }
}

/// XORs `data` with the mask `key`; `offset` is the position of `data[0]` in the frame
/// payload. Masking and unmasking are the same operation. Works on 8-byte words, which
/// the compiler turns into SIMD.
pub fn apply_mask(data: &mut [u8], key: [u8; 4], offset: usize) {
    let mut k = key;
    k.rotate_left(offset % 4);
    let word = u64::from_ne_bytes([k[0], k[1], k[2], k[3], k[0], k[1], k[2], k[3]]);
    let mut chunks = data.chunks_exact_mut(8);
    for chunk in &mut chunks {
        let v = u64::from_ne_bytes((&*chunk).try_into().expect("8 bytes")) ^ word;
        chunk.copy_from_slice(&v.to_ne_bytes());
    }
    // Whole words keep the key phase, so the tail starts at k[0] again.
    for (b, m) in chunks.into_remainder().iter_mut().zip(k.iter().cycle()) {
        *b ^= m;
    }
}

/// Mask keys for the client side. RFC 6455 wants them unpredictable (so a browser
/// script cannot choose the bytes on the wire); a generator seeded from the OS is
/// enough for that and avoids a syscall per frame.
pub struct MaskKeys(u64);

impl MaskKeys {
    pub fn new() -> io::Result<Self> {
        Ok(Self(u64::from_le_bytes(crate::crypto::random_bytes()?)))
    }

    /// wyrand.
    pub fn next_key(&mut self) -> [u8; 4] {
        self.0 = self.0.wrapping_add(0xa076_1d64_78bd_642f);
        let t = u128::from(self.0) * u128::from(self.0 ^ 0xe703_7ed1_a0b4_28db);
        let r = ((t >> 64) as u64) ^ (t as u64);
        (r as u32).to_ne_bytes()
    }
}

fn invalid(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(buf: &[u8]) -> (Header, usize) {
        Header::decode(buf).unwrap().unwrap()
    }

    #[test]
    fn rfc6455_single_frame_unmasked() {
        // Section 5.7: "Hello", unmasked text frame.
        let frame = [0x81, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f];
        let (h, n) = decode_all(&frame);
        assert_eq!(
            (h.fin, h.opcode, h.mask, h.len, n),
            (true, OP_TEXT, None, 5, 2)
        );
        assert_eq!(&frame[n..], b"Hello");
    }

    #[test]
    fn rfc6455_single_frame_masked() {
        // Section 5.7: "Hello", masked with 37 fa 21 3d.
        let mut frame = [
            0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
        ];
        let (h, n) = decode_all(&frame);
        assert_eq!(h.mask, Some([0x37, 0xfa, 0x21, 0x3d]));
        assert_eq!((h.len, n), (5, 6));
        apply_mask(&mut frame[n..], h.mask.unwrap(), 0);
        assert_eq!(&frame[n..], b"Hello");
    }

    #[test]
    fn rfc6455_fragmented_text() {
        // Section 5.7: "Hel" then "lo".
        let first = [0x01, 0x03, 0x48, 0x65, 0x6c];
        let second = [0x80, 0x02, 0x6c, 0x6f];
        let (h, _) = decode_all(&first);
        assert_eq!((h.fin, h.opcode), (false, OP_TEXT));
        let (h, _) = decode_all(&second);
        assert_eq!((h.fin, h.opcode, h.len), (true, OP_CONTINUATION, 2));
    }

    #[test]
    fn rfc6455_ping_pong() {
        // Section 5.7: unmasked ping, masked pong, both "Hello".
        let ping = [0x89, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f];
        let (h, _) = decode_all(&ping);
        assert!(h.is_control());
        assert_eq!(h.opcode, OP_PING);
        let mut pong = [
            0x8a, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
        ];
        let (h, n) = decode_all(&pong);
        assert_eq!(h.opcode, OP_PONG);
        apply_mask(&mut pong[n..], h.mask.unwrap(), 0);
        assert_eq!(&pong[n..], b"Hello");
    }

    #[test]
    fn rfc6455_extended_lengths() {
        // Section 5.7: 256 bytes (16-bit length) and 64 KiB (64-bit length), unmasked.
        let (h, n) = decode_all(&[0x82, 0x7e, 0x01, 0x00]);
        assert_eq!((h.opcode, h.len, n), (OP_BINARY, 256, 4));
        let (h, n) = decode_all(&[0x82, 0x7f, 0, 0, 0, 0, 0, 0x01, 0x00, 0x00]);
        assert_eq!((h.len, n), (65536, 10));
    }

    #[test]
    fn encode_uses_shortest_length_and_roundtrips() {
        for (len, header_len) in [
            (0u64, 2),
            (125, 2),
            (126, 4),
            (0xffff, 4),
            (0x1_0000, 10),
            (1 << 40, 10),
        ] {
            for mask in [None, Some([1, 2, 3, 4])] {
                let h = Header {
                    fin: true,
                    opcode: OP_BINARY,
                    mask,
                    len,
                };
                let mut out = Vec::new();
                h.encode(&mut out);
                let expected = header_len + if mask.is_some() { 4 } else { 0 };
                assert_eq!(out.len(), expected, "len {len}");
                assert_eq!(decode_all(&out), (h, expected));
            }
        }
    }

    #[test]
    fn incomplete_headers_ask_for_more() {
        let full = {
            let mut out = Vec::new();
            Header {
                fin: true,
                opcode: OP_BINARY,
                mask: Some([9, 9, 9, 9]),
                len: 70000,
            }
            .encode(&mut out);
            out
        };
        for cut in 0..full.len() {
            assert!(Header::decode(&full[..cut]).unwrap().is_none(), "cut {cut}");
        }
    }

    #[test]
    fn rejects_invalid_headers() {
        for frame in [
            &[0xc2, 0x00][..],                        // RSV1 set
            &[0x83, 0x00],                            // reserved data opcode
            &[0x8b, 0x00],                            // reserved control opcode
            &[0x09, 0x00],                            // fragmented ping
            &[0x88, 0x7e, 0x00, 0x7e],                // close longer than 125
            &[0x82, 0x7f, 0x80, 0, 0, 0, 0, 0, 0, 0], // 64-bit length top bit
        ] {
            assert!(Header::decode(frame).is_err(), "{frame:02x?}");
        }
    }

    #[test]
    fn mask_matches_bytewise_at_any_offset() {
        let key = [0x12, 0x34, 0x56, 0x78];
        let data: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        for offset in 0..8 {
            for len in [0, 1, 3, 7, 8, 9, 31, 1000] {
                let mut fast = data[..len].to_vec();
                apply_mask(&mut fast, key, offset);
                let slow: Vec<u8> = data[..len]
                    .iter()
                    .enumerate()
                    .map(|(i, b)| b ^ key[(offset + i) % 4])
                    .collect();
                assert_eq!(fast, slow, "offset {offset} len {len}");
            }
        }
    }

    #[test]
    fn mask_in_pieces_equals_mask_in_one_go() {
        let key = [0xde, 0xad, 0xbe, 0xef];
        let data: Vec<u8> = (0..100u8).collect();
        let mut whole = data.clone();
        apply_mask(&mut whole, key, 0);
        let mut pieces = data.clone();
        let mut pos = 0;
        for len in [3, 1, 17, 8, 40, 31] {
            apply_mask(&mut pieces[pos..pos + len], key, pos);
            pos += len;
        }
        assert_eq!(whole, pieces);
    }

    #[test]
    fn mask_keys_vary() {
        let mut keys = MaskKeys::new().unwrap();
        let a: Vec<[u8; 4]> = (0..64).map(|_| keys.next_key()).collect();
        let distinct: std::collections::HashSet<_> = a.iter().collect();
        assert!(distinct.len() > 60);
    }
}
