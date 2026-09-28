//! `kmux`: many streams over one tunnel connection.
//!
//! See `docs/PHASE2.md`, section 5, for the design and the frame format.

pub mod frame;
mod manager;
mod session;

use std::io;
use std::time::Duration;

pub use manager::{maintain, SessionPool};
pub use session::{MuxSession, MuxStream, INITIAL_WINDOW, MAX_DATA_FRAME};

use crate::config::{MuxSettings, Tuning};

/// A connection a session can run over. It is split into halves that work
/// independently, so reading (and decrypting) and writing (and encrypting) run in
/// parallel.
pub trait Transport: Send + 'static {
    type Reader: tokio::io::AsyncRead + Unpin + Send + 'static;
    type Writer: tokio::io::AsyncWrite + Unpin + Send + 'static;

    fn into_halves(self) -> (Self::Reader, Self::Writer);
}

/// Which end of the tunnel connection a session runs on. It only decides the stream id
/// space (client: odd, server: even), so both ends can open streams without clashing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Client,
    Server,
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Receive window per stream, in bytes.
    pub stream_window: u32,
    pub max_streams: usize,
    /// Ping interval; a peer silent for twice this long is considered dead.
    pub keepalive: Duration,
    /// Gather queued frames into larger writes (off: one frame per write).
    pub coalesce: bool,
}

impl SessionConfig {
    pub fn new(mux: &MuxSettings, tuning: &Tuning) -> Self {
        Self {
            stream_window: mux.stream_window.min(u32::MAX as usize) as u32,
            max_streams: mux.max_streams,
            keepalive: tuning.keepalive,
            coalesce: mux.coalesce,
        }
    }
}

/// Why a stream was reset, carried in `RST` frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetReason {
    /// The other side gave up on the stream.
    Cancel,
    /// The exit side could not connect to the target.
    DialFailed,
    /// The peer did not accept a new stream (going away or too many streams).
    Refused,
    Protocol,
    /// The peer does not support what the `SYN` asked for (e.g. a UDP flow).
    Unsupported,
}

impl ResetReason {
    fn id(self) -> u8 {
        match self {
            Self::Cancel => 0,
            Self::DialFailed => 1,
            Self::Refused => 2,
            Self::Protocol => 3,
            Self::Unsupported => 4,
        }
    }

    fn from_id(id: u8) -> Self {
        match id {
            1 => Self::DialFailed,
            2 => Self::Refused,
            3 => Self::Protocol,
            4 => Self::Unsupported,
            _ => Self::Cancel,
        }
    }

    /// The error a reset stream reports. `DialFailed` maps to `ConnectionRefused`, like
    /// the status byte of non-mux channels.
    fn to_error(self) -> io::Error {
        match self {
            Self::DialFailed => io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "exit side could not connect to the target",
            ),
            Self::Refused => {
                io::Error::new(io::ErrorKind::ConnectionReset, "stream refused by the peer")
            }
            Self::Cancel => {
                io::Error::new(io::ErrorKind::ConnectionReset, "stream reset by the peer")
            }
            Self::Protocol => io::Error::new(
                io::ErrorKind::ConnectionReset,
                "stream reset: protocol error",
            ),
            Self::Unsupported => io::Error::new(
                io::ErrorKind::Unsupported,
                "exit side does not support this kind of connection (older version?)",
            ),
        }
    }
}

#[cfg(test)]
mod tests;
