//! Messages exchanged over an authenticated tunnel connection before relaying starts.
//!
//! ```text
//! entry -> exit : kind (1) | target_len (2, big-endian) | target      kind: 1 TCP, 2 UDP
//! exit  -> entry: status (1)                                           (not with mux)
//! ```
//!
//! After that, a TCP channel carries raw bytes. A UDP channel without mux carries
//! packets as `len (2, BE) | packet` in both directions (with mux, packets travel as
//! `DGRAM` frames instead, see `src/mux/`).

use std::io;

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const KIND_TCP: u8 = 1;
/// A UDP flow (v0.3). v0.2 peers reject it as an unknown kind.
pub const KIND_UDP: u8 = 2;

pub const STATUS_OK: u8 = 0;
pub const STATUS_DIAL_FAILED: u8 = 1;
/// The exit side does not support this kind (for example UDP not enabled there).
pub const STATUS_UNSUPPORTED: u8 = 2;

/// Largest UDP payload (65,535 minus the IPv4 and UDP headers).
pub const MAX_DATAGRAM: usize = 65_507;

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

    pub fn udp(target: impl Into<String>) -> Self {
        Self {
            kind: KIND_UDP,
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
    if kind == KIND_TCP || kind == KIND_UDP {
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
        STATUS_UNSUPPORTED => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "exit side does not support this kind of connection (older version?)",
        )),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown status {other}"),
        )),
    }
}

/// Appends one UDP packet in the framing of UDP channels without mux. Several packets
/// can be gathered into one write this way.
pub fn put_datagram(packet: &[u8], out: &mut Vec<u8>) -> io::Result<()> {
    if packet.len() > MAX_DATAGRAM {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "UDP packet larger than 65,507 bytes",
        ));
    }
    out.extend_from_slice(&(packet.len() as u16).to_be_bytes());
    out.extend_from_slice(packet);
    Ok(())
}

/// Reads one UDP packet framed by [`put_datagram`]. `None` when the channel ends
/// cleanly between packets.
pub async fn read_datagram<R: AsyncRead + Unpin>(stream: &mut R) -> io::Result<Option<Bytes>> {
    let mut head = [0u8; 2];
    if stream.read(&mut head[..1]).await? == 0 {
        return Ok(None);
    }
    stream.read_exact(&mut head[1..]).await?;
    let len = u16::from_be_bytes(head) as usize;
    if len > MAX_DATAGRAM {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "UDP packet larger than 65,507 bytes",
        ));
    }
    let mut packet = vec![0u8; len];
    stream.read_exact(&mut packet).await?;
    Ok(Some(packet.into()))
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

    #[tokio::test]
    async fn udp_open_roundtrip() {
        let open = Open::udp("[2001:db8::1]:53");
        let mut buf = Vec::new();
        open.encode(&mut buf);
        assert_eq!(buf[0], KIND_UDP);
        assert_eq!(Open::decode(&buf).unwrap(), open);
        buf[0] = 3;
        assert!(Open::decode(&buf).is_err());
    }

    #[tokio::test]
    async fn datagram_framing_keeps_packet_boundaries() {
        let packets: Vec<Vec<u8>> = vec![
            vec![],
            vec![1],
            (0..1400).map(|i| i as u8).collect(),
            vec![7; MAX_DATAGRAM],
        ];
        let mut wire = Vec::new();
        for p in &packets {
            put_datagram(p, &mut wire).unwrap();
        }
        // Delivered in awkward pieces, as a stream would.
        let (mut r, mut w) = tokio::io::duplex(7);
        let writer = tokio::spawn(async move {
            for chunk in wire.chunks(5) {
                w.write_all(chunk).await.unwrap();
            }
        });
        for p in &packets {
            let got = read_datagram(&mut r).await.unwrap().unwrap();
            assert_eq!(&got[..], &p[..]);
        }
        writer.await.unwrap();
        assert!(read_datagram(&mut r).await.unwrap().is_none(), "clean end");
    }

    #[tokio::test]
    async fn datagram_framing_limits() {
        let mut out = Vec::new();
        assert!(put_datagram(&vec![0; MAX_DATAGRAM + 1], &mut out).is_err());
        assert!(out.is_empty());
        // A length above the maximum, or a packet cut short, is an error.
        let too_long = (MAX_DATAGRAM as u16 + 1).to_be_bytes();
        assert!(read_datagram(&mut &too_long[..]).await.is_err());
        let cut = [0u8, 5, 1, 2];
        assert!(read_datagram(&mut &cut[..]).await.is_err());
        let half_header = [0u8];
        assert!(read_datagram(&mut &half_header[..]).await.is_err());
    }

    #[tokio::test]
    async fn unsupported_status() {
        let err = read_status(&mut &[STATUS_UNSUPPORTED][..])
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }
}
