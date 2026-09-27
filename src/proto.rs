//! Messages exchanged over an authenticated tunnel connection before relaying starts.
//!
//! ```text
//! entry -> exit : kind (1) | target_len (2, big-endian) | target
//! exit  -> entry: status (1)
//! ```

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const KIND_TCP: u8 = 1;

pub const STATUS_OK: u8 = 0;
pub const STATUS_DIAL_FAILED: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Open {
    pub kind: u8,
    pub target: String,
}

impl Open {
    pub fn tcp(target: impl Into<String>) -> Self {
        Self {
            kind: KIND_TCP,
            target: target.into(),
        }
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        let target = self.target.as_bytes();
        out.push(self.kind);
        out.extend_from_slice(&(target.len() as u16).to_be_bytes());
        out.extend_from_slice(target);
    }

    pub async fn read<S: AsyncRead + Unpin>(stream: &mut S) -> io::Result<Self> {
        let mut head = [0u8; 3];
        stream.read_exact(&mut head).await?;
        check_kind(head[0])?;
        let len = u16::from_be_bytes([head[1], head[2]]) as usize;
        let mut target = vec![0u8; len];
        stream.read_exact(&mut target).await?;
        Self::with_target(head[0], target)
    }

    /// Decodes a complete open request, as carried in a mux `SYN`.
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "malformed open request");
        let (head, target) = bytes.split_at_checked(3).ok_or_else(bad)?;
        check_kind(head[0])?;
        if target.len() != u16::from_be_bytes([head[1], head[2]]) as usize {
            return Err(bad());
        }
        Self::with_target(head[0], target.to_vec())
    }

    fn with_target(kind: u8, target: Vec<u8>) -> io::Result<Self> {
        let target = String::from_utf8(target)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "target is not valid UTF-8"))?;
        Ok(Self { kind, target })
    }
}

fn check_kind(kind: u8) -> io::Result<()> {
    if kind == KIND_TCP {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported stream kind {kind}"),
        ))
    }
}

/// Writes a whole message and flushes it: channels may buffer writes (the record layer
/// does), and the peer waits for this message before sending anything back.
pub async fn send<S: AsyncWrite + Unpin>(stream: &mut S, msg: &[u8]) -> io::Result<()> {
    stream.write_all(msg).await?;
    stream.flush().await
}

pub async fn write_status<S: AsyncWrite + Unpin>(stream: &mut S, status: u8) -> io::Result<()> {
    send(stream, &[status]).await
}

pub async fn read_status<S: AsyncRead + Unpin>(stream: &mut S) -> io::Result<()> {
    let mut status = [0u8; 1];
    stream.read_exact(&mut status).await?;
    match status[0] {
        STATUS_OK => Ok(()),
        STATUS_DIAL_FAILED => Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "exit side could not connect to the target",
        )),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown status {other}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn open_roundtrip() {
        let open = Open::tcp("example.com:443");
        let mut buf = Vec::new();
        open.encode(&mut buf);
        let decoded = Open::read(&mut buf.as_slice()).await.unwrap();
        assert_eq!(decoded, open);
        assert_eq!(Open::decode(&buf).unwrap(), open);
        assert!(Open::decode(&buf[..buf.len() - 1]).is_err());
        buf.push(0);
        assert!(Open::decode(&buf).is_err());
    }
}
