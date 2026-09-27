//! Mux frame encoding.
//!
//! ```text
//! type (1) | stream_id (4, BE) | length (2, BE) | payload
//! ```
//!
//! Frames travel inside the record layer, so they are not obfuscated themselves.

use std::io;

pub const HEADER_LEN: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    /// Opens a stream; the payload is the open request.
    Syn,
    Data,
    /// Half close: the sender will send no more data on this stream.
    Fin,
    /// Aborts a stream; 1-byte [`super::ResetReason`].
    Rst,
    /// Flow-control credit; u32 increment.
    Window,
    /// Liveness check; 8 opaque bytes, echoed in a `Pong`.
    Ping,
    Pong,
    /// No new streams on this session, in either direction.
    GoAway,
    /// Reserved for phase 3 (UDP over the mux); ignored for now.
    Dgram,
}

impl FrameType {
    fn id(self) -> u8 {
        match self {
            Self::Syn => 0,
            Self::Data => 1,
            Self::Fin => 2,
            Self::Rst => 3,
            Self::Window => 4,
            Self::Ping => 5,
            Self::Pong => 6,
            Self::GoAway => 7,
            Self::Dgram => 8,
        }
    }

    fn from_id(id: u8) -> Option<Self> {
        Some(match id {
            0 => Self::Syn,
            1 => Self::Data,
            2 => Self::Fin,
            3 => Self::Rst,
            4 => Self::Window,
            5 => Self::Ping,
            6 => Self::Pong,
            7 => Self::GoAway,
            8 => Self::Dgram,
            _ => return None,
        })
    }

    /// Exact payload length for fixed-size frames.
    fn fixed_len(self) -> Option<usize> {
        match self {
            Self::Fin | Self::GoAway => Some(0),
            Self::Rst => Some(1),
            Self::Window => Some(4),
            Self::Ping | Self::Pong => Some(8),
            Self::Syn | Self::Data | Self::Dgram => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub kind: FrameType,
    pub stream: u32,
    pub len: u16,
}

impl Header {
    pub fn decode(bytes: &[u8; HEADER_LEN]) -> io::Result<Self> {
        let kind = FrameType::from_id(bytes[0]).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown mux frame type {}", bytes[0]),
            )
        })?;
        let stream = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        let len = u16::from_be_bytes([bytes[5], bytes[6]]);
        if kind.fixed_len().is_some_and(|l| l != len as usize) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("mux {kind:?} frame with length {len}"),
            ));
        }
        Ok(Self { kind, stream, len })
    }
}

/// Appends one frame to `out`. `payload` must fit in 16 bits.
pub fn put(out: &mut Vec<u8>, kind: FrameType, stream: u32, payload: &[u8]) {
    out.extend_from_slice(&header(kind, stream, payload.len()));
    out.extend_from_slice(payload);
}

/// Encodes a frame header for a payload of `len` bytes (at most 16 bits).
pub fn header(kind: FrameType, stream: u32, len: usize) -> [u8; HEADER_LEN] {
    debug_assert!(len <= u16::MAX as usize);
    let mut h = [0u8; HEADER_LEN];
    h[0] = kind.id();
    h[1..5].copy_from_slice(&stream.to_be_bytes());
    h[5..].copy_from_slice(&(len as u16).to_be_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for (kind, payload) in [
            (FrameType::Syn, &b"example.com:443"[..]),
            (FrameType::Data, &[1, 2, 3][..]),
            (FrameType::Fin, &[][..]),
            (FrameType::Rst, &[1][..]),
            (FrameType::Window, &[0, 1, 0, 0][..]),
            (FrameType::Ping, &[9; 8][..]),
            (FrameType::GoAway, &[][..]),
        ] {
            let mut out = Vec::new();
            put(&mut out, kind, 0xdead_beef, payload);
            let h = Header::decode(out[..HEADER_LEN].try_into().unwrap()).unwrap();
            assert_eq!(
                h,
                Header {
                    kind,
                    stream: 0xdead_beef,
                    len: payload.len() as u16
                }
            );
            assert_eq!(&out[HEADER_LEN..], payload);
        }
    }

    #[test]
    fn rejects_unknown_types_and_bad_fixed_lengths() {
        assert!(Header::decode(&[9, 0, 0, 0, 1, 0, 0]).is_err());
        assert!(Header::decode(&[255, 0, 0, 0, 1, 0, 0]).is_err());
        // WINDOW must carry exactly 4 bytes, FIN none.
        assert!(Header::decode(&[4, 0, 0, 0, 1, 0, 3]).is_err());
        assert!(Header::decode(&[2, 0, 0, 0, 1, 0, 1]).is_err());
    }
}
