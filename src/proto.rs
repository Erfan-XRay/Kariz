//! Messages exchanged over an authenticated tunnel connection before relaying starts.
//!
//! ```text
//! entry -> exit : kind (1) | target_len (2, big-endian) | target      kind: 1 TCP, 2 UDP,
//!                                                                      4 speed test
//!                 [ | copies (1) | gap_ms (1) ]                        kind 3: UDP with
//!                                                                      duplication
//! exit  -> entry: status (1)                                           (not with mux)
//! ```
//!
//! After that, a TCP channel carries raw bytes. A UDP channel without mux carries
//! packets as `len (2, BE) | packet` in both directions (with mux, packets travel as
//! `DGRAM` frames instead, see `src/mux/`). With duplication (v0.5), every packet starts with a 4-byte sequence number (see `src/udp.rs`);
//! older exits reject kind 3 as unknown.

use std::io;

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const KIND_TCP: u8 = 1;
/// A UDP flow (v0.3). v0.2 peers reject it as an unknown kind.
pub const KIND_UDP: u8 = 2;
/// A UDP flow with packet duplication (v0.5); decoded as [`KIND_UDP`] with
/// [`Open::duplicate`] set.
const KIND_UDP_DUPLICATED: u8 = 3;
/// A speed test stream (v0.6): the exit side does not dial anything, it produces,
/// consumes or echoes data as the target says (`down`, `up`, `echo`, `udp`), see
/// `src/speedtest.rs`. Older exits reject it as an unknown kind.
pub const KIND_SPEEDTEST: u8 = 4;

/// Packet duplication of a UDP flow: each packet is sent `copies` times in all,
/// `gap_ms` apart, in both directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Duplicate {
    pub copies: u8,
    pub gap_ms: u8,
}

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
    /// UDP only: the flow's packet duplication.
    pub duplicate: Option<Duplicate>,
}

impl Open {
    pub fn tcp(target: impl Into<String>) -> Self {
        Self {
            kind: KIND_TCP,
            target: target.into(),
            duplicate: None,
        }
    }

    pub fn udp(target: impl Into<String>, duplicate: Option<Duplicate>) -> Self {
        Self {
            kind: KIND_UDP,
            target: target.into(),
            duplicate,
        }
    }

    /// A speed test stream; `command` is what the exit side does on it.
    pub fn speedtest(command: impl Into<String>) -> Self {
        Self {
            kind: KIND_SPEEDTEST,
            target: command.into(),
            duplicate: None,
        }
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        let target = self.target.as_bytes();
        let kind = match self.duplicate {
            Some(_) if self.kind == KIND_UDP => KIND_UDP_DUPLICATED,
            _ => self.kind,
        };
        out.push(kind);
        out.extend_from_slice(&(target.len() as u16).to_be_bytes());
        out.extend_from_slice(target);
        if kind == KIND_UDP_DUPLICATED {
            let d = self.duplicate.expect("kind 3 has duplication");
            out.extend_from_slice(&[d.copies, d.gap_ms]);
        }
    }

    pub async fn read<S: AsyncRead + Unpin>(stream: &mut S) -> io::Result<Self> {
        let mut head = [0u8; 3];
        stream.read_exact(&mut head).await?;
        check_kind(head[0])?;
        let len = u16::from_be_bytes([head[1], head[2]]) as usize;
        let mut rest = vec![0u8; len + options_len(head[0])];
        stream.read_exact(&mut rest).await?;
        Self::with_rest(head[0], rest)
    }

    /// Decodes a complete open request, as carried in a mux `SYN`.
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "malformed open request");
        let (head, rest) = bytes.split_at_checked(3).ok_or_else(bad)?;
        check_kind(head[0])?;
        let len = u16::from_be_bytes([head[1], head[2]]) as usize;
        if rest.len() != len + options_len(head[0]) {
            return Err(bad());
        }
        Self::with_rest(head[0], rest.to_vec())
    }

    /// `rest`: the target, then the options of `kind`.
    fn with_rest(kind: u8, mut rest: Vec<u8>) -> io::Result<Self> {
        let options = rest.split_off(rest.len() - options_len(kind));
        let target = String::from_utf8(rest)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "target is not valid UTF-8"))?;
        if kind != KIND_UDP_DUPLICATED {
            return Ok(Self {
                kind,
                target,
                duplicate: None,
            });
        }
        let (copies, gap_ms) = (options[0], options[1]);
        if !(2..=3).contains(&copies) || gap_ms > 50 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bad duplication in open request",
            ));
        }
        Ok(Self::udp(target, Some(Duplicate { copies, gap_ms })))
    }
}

/// Bytes after the target in an open request of `kind`.
fn options_len(kind: u8) -> usize {
    if kind == KIND_UDP_DUPLICATED {
        2
    } else {
        0
    }
}

fn check_kind(kind: u8) -> io::Result<()> {
    if matches!(
        kind,
        KIND_TCP | KIND_UDP | KIND_UDP_DUPLICATED | KIND_SPEEDTEST
    ) {
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
        let open = Open::udp("[2001:db8::1]:53", None);
        let mut buf = Vec::new();
        open.encode(&mut buf);
        assert_eq!(buf[0], KIND_UDP);
        assert_eq!(Open::decode(&buf).unwrap(), open);
        buf[0] = 9;
        assert!(Open::decode(&buf).is_err());
    }

    #[tokio::test]
    async fn speedtest_open_roundtrip() {
        let open = Open::speedtest("down");
        let mut buf = Vec::new();
        open.encode(&mut buf);
        assert_eq!(buf[0], KIND_SPEEDTEST);
        assert_eq!(Open::decode(&buf).unwrap(), open);
        assert_eq!(Open::read(&mut &buf[..]).await.unwrap(), open);
    }

    #[tokio::test]
    async fn duplicated_udp_open_roundtrip() {
        let open = Open::udp(
            "10.0.0.2:27015",
            Some(Duplicate {
                copies: 2,
                gap_ms: 5,
            }),
        );
        let mut buf = Vec::new();
        open.encode(&mut buf);
        assert_eq!(buf[0], KIND_UDP_DUPLICATED);
        assert_eq!(&buf[buf.len() - 2..], [2, 5]);
        let decoded = Open::decode(&buf).unwrap();
        assert_eq!((decoded.kind, &decoded), (KIND_UDP, &open));
        assert_eq!(Open::read(&mut &buf[..]).await.unwrap(), open);
        // Without its options, or with out-of-range ones, it is malformed.
        assert!(Open::decode(&buf[..buf.len() - 2]).is_err());
        for (copies, gap) in [(1, 5), (4, 5), (2, 51)] {
            let n = buf.len();
            let mut bad = buf.clone();
            bad[n - 2..].copy_from_slice(&[copies, gap]);
            assert!(Open::decode(&bad).is_err(), "{copies} {gap}");
        }
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
