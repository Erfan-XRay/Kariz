//! The synthetic IPv4 + TCP framing of a Paqet packet.
//!
//! One sealed KCP packet goes into one TCP segment. There is no TCP handshake, no
//! retransmission and no stream: KCP is the reliable layer. The segment only has to be
//! well-formed enough for the IPv4/TCP checksum, and look like ordinary established TCP
//! data to a middlebox.

use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};

/// IPv4 header without options.
pub const IPV4_HEADER: usize = 20;
/// TCP header without options.
pub const TCP_HEADER: usize = 20;
/// The most a TCP segment can carry.
pub const MAX_PAYLOAD: usize = (u16::MAX as usize) - TCP_HEADER;

const IPPROTO_TCP: u8 = 6;
const FLAG_FIN: u8 = 0x01;
const FLAG_SYN: u8 = 0x02;
const FLAG_RST: u8 = 0x04;
const FLAG_ACK: u8 = 0x10;
const FLAG_PSH: u8 = 0x08;
const WINDOW: u16 = u16::MAX;

/// A segment received for the local TCP port.
pub struct Incoming<'a> {
    pub payload: &'a [u8],
    pub peer: SocketAddrV4,
}

/// Builds a TCP segment (header and `payload`) for `source` -> `destination`.
///
/// The IP header is added by the kernel for an IPv4 raw socket, so this writes only the
/// TCP header and payload. The sequence and acknowledgment numbers are arbitrary but
/// monotonic per sender; the checksum covers the TCP pseudo-header with both addresses.
pub fn build_tcp_segment(
    source: SocketAddrV4,
    destination: SocketAddrV4,
    sequence: u32,
    acknowledgment: u32,
    payload: &[u8],
    out: &mut Vec<u8>,
) -> io::Result<()> {
    let length = TCP_HEADER + payload.len();
    u16::try_from(length).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "paqet payload does not fit in a TCP segment",
        )
    })?;
    out.clear();
    out.reserve(length);
    out.extend_from_slice(&source.port().to_be_bytes());
    out.extend_from_slice(&destination.port().to_be_bytes());
    out.extend_from_slice(&sequence.to_be_bytes());
    out.extend_from_slice(&acknowledgment.to_be_bytes());
    out.push(5 << 4);
    out.push(FLAG_ACK | FLAG_PSH);
    out.extend_from_slice(&WINDOW.to_be_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(payload);
    let checksum = tcp_checksum(*source.ip(), *destination.ip(), out);
    out[16..18].copy_from_slice(&checksum.to_be_bytes());
    Ok(())
}

/// Parses a received IPv4 packet and returns the TCP payload and where it came from.
///
/// Raw IPv4 sockets receive the IP header as well as the segment. Packets for another
/// address or port, fragments (the kernel reassembles before a raw socket sees them),
/// non-data segments and segments with a bad checksum are rejected. A rejected packet
/// never reaches the AEAD or KCP.
pub fn parse_ipv4_tcp(packet: &[u8], local_ip: Ipv4Addr, local_port: u16) -> Option<Incoming<'_>> {
    if packet.len() < IPV4_HEADER + TCP_HEADER || packet[0] >> 4 != 4 {
        return None;
    }
    let header_len = usize::from(packet[0] & 0x0f) * 4;
    if header_len < IPV4_HEADER || packet.len() < header_len + TCP_HEADER {
        return None;
    }
    let total_len = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
    if total_len < header_len + TCP_HEADER || total_len > packet.len() {
        return None;
    }
    // Only unfragmented packets: raw sockets get reassembled datagrams, but an MF bit
    // here would mean a fragment was delivered on its own.
    let fragment = u16::from_be_bytes([packet[6], packet[7]]);
    if fragment & 0x3fff != 0 {
        return None;
    }
    if packet[9] != IPPROTO_TCP {
        return None;
    }
    let source_ip = Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]);
    let destination_ip = Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]);
    if destination_ip != local_ip
        || source_ip.is_unspecified()
        || source_ip.is_multicast()
        || source_ip.is_broadcast()
    {
        return None;
    }
    let tcp = &packet[header_len..total_len];
    let source_port = u16::from_be_bytes([tcp[0], tcp[1]]);
    let destination_port = u16::from_be_bytes([tcp[2], tcp[3]]);
    if source_port == 0 || destination_port != local_port {
        return None;
    }
    let data_offset = usize::from(tcp[12] >> 4) * 4;
    if data_offset < TCP_HEADER || data_offset > tcp.len() {
        return None;
    }
    let flags = tcp[13];
    if flags & (FLAG_SYN | FLAG_RST | FLAG_FIN) != 0 || flags & FLAG_ACK == 0 {
        return None;
    }
    if !tcp_checksum_valid(source_ip, local_ip, tcp) {
        return None;
    }
    let payload = &tcp[data_offset..];
    if payload.is_empty() {
        return None;
    }
    Some(Incoming {
        payload,
        peer: SocketAddrV4::new(source_ip, source_port),
    })
}

/// The TCP checksum for `source` -> `destination` with the segment's checksum field
/// zeroed.
fn tcp_checksum(source: Ipv4Addr, destination: Ipv4Addr, tcp: &[u8]) -> u16 {
    !checksum16(pseudo_sum(source, destination, tcp))
}

/// Whether a received TCP segment's checksum is correct.
fn tcp_checksum_valid(source: Ipv4Addr, destination: Ipv4Addr, tcp: &[u8]) -> bool {
    checksum16(pseudo_sum(source, destination, tcp)) == u16::MAX
}

/// The one's-complement sum of the TCP pseudo-header and `tcp`.
fn pseudo_sum(source: Ipv4Addr, destination: Ipv4Addr, tcp: &[u8]) -> u32 {
    let mut sum = 0u32;
    add_words(&mut sum, &source.octets());
    add_words(&mut sum, &destination.octets());
    sum += u32::from(IPPROTO_TCP);
    sum += tcp.len() as u32;
    add_words(&mut sum, tcp);
    sum
}

fn add_words(sum: &mut u32, bytes: &[u8]) {
    let mut words = bytes.chunks_exact(2);
    for word in &mut words {
        *sum += u32::from(u16::from_be_bytes([word[0], word[1]]));
    }
    if let [last] = words.remainder() {
        *sum += u32::from(*last) << 8;
    }
}

fn checksum16(mut sum: u32) -> u16 {
    while sum > u32::from(u16::MAX) {
        sum = (sum & u32::from(u16::MAX)) + (sum >> 16);
    }
    sum as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> SocketAddrV4 {
        SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 10), 40_000)
    }

    fn destination() -> SocketAddrV4 {
        SocketAddrV4::new(Ipv4Addr::new(198, 51, 100, 20), 3_080)
    }

    fn segment(payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        build_tcp_segment(source(), destination(), 7, 9, payload, &mut out).unwrap();
        out
    }

    /// An IPv4 packet around `tcp`, with no options.
    fn ipv4(tcp: &[u8]) -> Vec<u8> {
        let mut packet = vec![0u8; IPV4_HEADER];
        packet[0] = 0x45;
        packet[2..4].copy_from_slice(&(IPV4_HEADER as u16 + tcp.len() as u16).to_be_bytes());
        packet[6..8].copy_from_slice(&0x4000u16.to_be_bytes()); // DF
        packet[8] = 64;
        packet[9] = IPPROTO_TCP;
        packet[12..16].copy_from_slice(&source().ip().octets());
        packet[16..20].copy_from_slice(&destination().ip().octets());
        packet.extend_from_slice(tcp);
        packet
    }

    #[test]
    fn builds_a_valid_segment() {
        let tcp = segment(b"hello");
        assert_eq!(tcp.len(), TCP_HEADER + 5);
        assert_eq!(&tcp[0..2], &source().port().to_be_bytes());
        assert_eq!(&tcp[2..4], &destination().port().to_be_bytes());
        assert_eq!(&tcp[4..8], &7u32.to_be_bytes());
        assert_eq!(&tcp[8..12], &9u32.to_be_bytes());
        assert_eq!(tcp[12], 5 << 4);
        assert_eq!(tcp[13], FLAG_ACK | FLAG_PSH);
        assert_eq!(&tcp[14..16], &WINDOW.to_be_bytes());
        assert_eq!(&tcp[18..20], &[0, 0]);
        assert!(tcp_checksum_valid(
            *source().ip(),
            *destination().ip(),
            &tcp
        ));
        assert_eq!(&tcp[TCP_HEADER..], b"hello");
    }

    #[test]
    fn parses_a_built_segment() {
        let tcp = segment(b"a packet");
        let packet = ipv4(&tcp);
        let incoming = parse_ipv4_tcp(&packet, *destination().ip(), destination().port()).unwrap();
        assert_eq!(incoming.payload, b"a packet");
        assert_eq!(incoming.peer, source());
    }

    #[test]
    fn parses_ip_options() {
        let tcp = segment(b"optioned");
        let mut packet = vec![0u8; 24];
        packet[0] = 0x46; // 24-byte IPv4 header
        packet[2..4].copy_from_slice(&(24u16 + tcp.len() as u16).to_be_bytes());
        packet[8] = 64;
        packet[9] = IPPROTO_TCP;
        packet[12..16].copy_from_slice(&source().ip().octets());
        packet[16..20].copy_from_slice(&destination().ip().octets());
        packet[20..24].copy_from_slice(&[1, 1, 1, 1]); // NOP padding
        packet.extend_from_slice(&tcp);
        let incoming = parse_ipv4_tcp(&packet, *destination().ip(), destination().port()).unwrap();
        assert_eq!(incoming.payload, b"optioned");
    }

    #[test]
    fn endian_and_odd_lengths_round_trip() {
        for len in 0..=64usize {
            let payload: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let tcp = segment(&payload);
            assert!(tcp_checksum_valid(
                *source().ip(),
                *destination().ip(),
                &tcp
            ));
            let packet = ipv4(&tcp);
            let incoming = parse_ipv4_tcp(&packet, *destination().ip(), destination().port());
            if len == 0 {
                assert!(incoming.is_none());
            } else {
                assert_eq!(incoming.unwrap().payload, &payload[..]);
            }
        }
    }

    fn fix_checksum(packet: &mut [u8]) {
        packet[IPV4_HEADER + 16..IPV4_HEADER + 18].fill(0);
        let checksum = !checksum16(pseudo_sum(
            *source().ip(),
            *destination().ip(),
            &packet[IPV4_HEADER..],
        ));
        packet[IPV4_HEADER + 16..IPV4_HEADER + 18].copy_from_slice(&checksum.to_be_bytes());
    }

    #[test]
    fn a_changed_byte_is_rejected() {
        let mut packet = ipv4(&segment(b"authenticated later"));
        let last = packet.len() - 1;
        packet[last] ^= 0x01;
        assert!(parse_ipv4_tcp(&packet, *destination().ip(), destination().port()).is_none());
    }

    #[test]
    fn wrong_address_or_port_is_rejected() {
        let tcp = segment(b"data");
        let packet = ipv4(&tcp);
        assert!(
            parse_ipv4_tcp(&packet, Ipv4Addr::new(203, 0, 113, 1), destination().port()).is_none()
        );
        assert!(parse_ipv4_tcp(&packet, *destination().ip(), destination().port() + 1).is_none());
    }

    #[test]
    fn non_data_segments_are_rejected() {
        let tcp = segment(b"data");
        let packet = ipv4(&tcp);
        for flag in [FLAG_SYN, FLAG_FIN, FLAG_RST] {
            let mut changed = packet.clone();
            changed[IPV4_HEADER + 13] |= flag;
            // The checksum must be corrected for the packet to be rejected on its flags,
            // not on its checksum.
            fix_checksum(&mut changed);
            assert!(
                parse_ipv4_tcp(&changed, *destination().ip(), destination().port()).is_none(),
                "flag {flag:#x}"
            );
        }
        // No ACK: a stray SYN-like segment, never data.
        let mut no_ack = packet;
        no_ack[IPV4_HEADER + 13] = FLAG_PSH;
        fix_checksum(&mut no_ack);
        assert!(parse_ipv4_tcp(&no_ack, *destination().ip(), destination().port()).is_none());
    }

    #[test]
    fn fragments_are_rejected() {
        let tcp = segment(b"data");
        let mut packet = ipv4(&tcp);
        packet[6..8].copy_from_slice(&0x2000u16.to_be_bytes()); // MF
        assert!(parse_ipv4_tcp(&packet, *destination().ip(), destination().port()).is_none());
        packet[6..8].copy_from_slice(&0x0001u16.to_be_bytes()); // offset 1
        assert!(parse_ipv4_tcp(&packet, *destination().ip(), destination().port()).is_none());
    }

    #[test]
    fn truncated_or_empty_packets_are_rejected() {
        let tcp = segment(b"data");
        let packet = ipv4(&tcp);
        for len in 0..packet.len() {
            assert!(
                parse_ipv4_tcp(&packet[..len], *destination().ip(), destination().port()).is_none(),
                "length {len}"
            );
        }
        let empty = segment(b"");
        let packet = ipv4(&empty);
        assert!(parse_ipv4_tcp(&packet, *destination().ip(), destination().port()).is_none());
    }

    #[test]
    fn a_bad_data_offset_is_rejected() {
        let tcp = segment(b"data");
        let mut packet = ipv4(&tcp);
        packet[IPV4_HEADER + 12] = 4 << 4;
        assert!(parse_ipv4_tcp(&packet, *destination().ip(), destination().port()).is_none());
        packet[IPV4_HEADER + 12] = 15 << 4;
        assert!(parse_ipv4_tcp(&packet, *destination().ip(), destination().port()).is_none());
    }

    #[test]
    fn an_oversized_segment_is_refused() {
        let payload = vec![0u8; MAX_PAYLOAD + 1];
        let mut out = Vec::new();
        assert!(build_tcp_segment(source(), destination(), 0, 0, &payload, &mut out).is_err());
        let payload = vec![0u8; MAX_PAYLOAD];
        assert!(build_tcp_segment(source(), destination(), 0, 0, &payload, &mut out).is_ok());
        assert_eq!(out.len(), u16::MAX as usize);
    }
}
