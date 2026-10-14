//! PROXY protocol headers (v1 and v2, TCP only), the way HAProxy defines them
//! (`proxy-protocol.txt`). The exit side writes one to the target before any user data
//! when the forward rule asks for it, so the target (Xray with `acceptProxyProtocol`)
//! sees the user's address instead of the exit side's own.

use std::net::{IpAddr, Ipv6Addr, SocketAddr};

use serde::Deserialize;

/// Which PROXY protocol header a forward rule sends (`proxy_protocol` in `[[forward]]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyVersion {
    /// The text header: `PROXY TCP4 src dst sport dport\r\n`.
    V1,
    /// The binary header.
    V2,
}

impl ProxyVersion {
    pub fn name(self) -> &'static str {
        match self {
            Self::V1 => "v1",
            Self::V2 => "v2",
        }
    }
}

/// What a header says: the user's address and the address they connected to on the
/// entry side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyHeader {
    pub version: ProxyVersion,
    pub source: SocketAddr,
    pub destination: SocketAddr,
}

/// The v2 header starts with these 12 bytes.
pub const V2_SIGNATURE: [u8; 12] = *b"\r\n\r\n\0\r\nQUIT\n";
/// Version 2, command PROXY.
const V2_PROXY: u8 = 0x21;
/// Address family and transport: TCP over IPv4, TCP over IPv6.
const V2_TCP4: u8 = 0x11;
const V2_TCP6: u8 = 0x21;

/// The two addresses of a header in one family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Addresses {
    V4([u8; 4], [u8; 4]),
    V6([u8; 16], [u8; 16]),
}

impl ProxyHeader {
    /// Both addresses in one family. A listener on `[::]` reports an IPv4 user as an
    /// IPv4-mapped IPv6 address: that is written as IPv4, as the user really is. Should
    /// only one of the two be IPv4, both go as IPv6 (a header has one family).
    pub(crate) fn addresses(&self) -> Addresses {
        let (src, dst) = (
            self.source.ip().to_canonical(),
            self.destination.ip().to_canonical(),
        );
        match (src, dst) {
            (IpAddr::V4(s), IpAddr::V4(d)) => Addresses::V4(s.octets(), d.octets()),
            _ => Addresses::V6(as_v6(src).octets(), as_v6(dst).octets()),
        }
    }

    /// The header to write to the target.
    pub fn to_bytes(&self) -> Vec<u8> {
        let (sport, dport) = (self.source.port(), self.destination.port());
        let addresses = self.addresses();
        match self.version {
            ProxyVersion::V1 => {
                let (family, src, dst) = match addresses {
                    Addresses::V4(s, d) => ("TCP4", IpAddr::from(s), IpAddr::from(d)),
                    Addresses::V6(s, d) => ("TCP6", IpAddr::from(s), IpAddr::from(d)),
                };
                format!("PROXY {family} {src} {dst} {sport} {dport}\r\n").into_bytes()
            }
            ProxyVersion::V2 => {
                let mut out = Vec::with_capacity(16 + 36);
                out.extend_from_slice(&V2_SIGNATURE);
                out.push(V2_PROXY);
                match addresses {
                    Addresses::V4(s, d) => {
                        out.push(V2_TCP4);
                        out.extend_from_slice(&12u16.to_be_bytes());
                        out.extend_from_slice(&s);
                        out.extend_from_slice(&d);
                    }
                    Addresses::V6(s, d) => {
                        out.push(V2_TCP6);
                        out.extend_from_slice(&36u16.to_be_bytes());
                        out.extend_from_slice(&s);
                        out.extend_from_slice(&d);
                    }
                }
                out.extend_from_slice(&sport.to_be_bytes());
                out.extend_from_slice(&dport.to_be_bytes());
                out
            }
        }
    }
}

fn as_v6(ip: IpAddr) -> Ipv6Addr {
    match ip {
        IpAddr::V4(v4) => v4.to_ipv6_mapped(),
        IpAddr::V6(v6) => v6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(version: ProxyVersion, source: &str, destination: &str) -> Vec<u8> {
        ProxyHeader {
            version,
            source: source.parse().unwrap(),
            destination: destination.parse().unwrap(),
        }
        .to_bytes()
    }

    #[test]
    fn v1_text() {
        let v4 = header(ProxyVersion::V1, "192.168.0.1:56324", "192.168.0.11:443");
        assert_eq!(v4, b"PROXY TCP4 192.168.0.1 192.168.0.11 56324 443\r\n");
        let v6 = header(
            ProxyVersion::V1,
            "[2001:db8::1]:40000",
            "[2001:db8::2]:8443",
        );
        assert_eq!(v6, b"PROXY TCP6 2001:db8::1 2001:db8::2 40000 8443\r\n");
        // An IPv4 user seen by a dual-stack listener is written as IPv4.
        let mapped = header(
            ProxyVersion::V1,
            "[::ffff:10.1.2.3]:5000",
            "[::ffff:10.0.0.1]:443",
        );
        assert_eq!(mapped, b"PROXY TCP4 10.1.2.3 10.0.0.1 5000 443\r\n");
        // The longest header the spec allows is 107 bytes.
        let long = header(
            ProxyVersion::V1,
            "[ffff:ffff:ffff:ffff:ffff:ffff:ffff:fffe]:65535",
            "[ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff]:65535",
        );
        assert!(long.len() <= 107, "{}", long.len());
    }

    #[test]
    fn v2_ipv4_layout() {
        let h = header(ProxyVersion::V2, "192.168.0.1:56324", "192.168.0.11:443");
        let mut want = vec![
            0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54,
            0x0A, // signature
            0x21, // version 2, PROXY
            0x11, // TCP over IPv4
            0x00, 0x0C, // 12 bytes follow
            192, 168, 0, 1, // source
            192, 168, 0, 11, // destination
        ];
        want.extend_from_slice(&56324u16.to_be_bytes());
        want.extend_from_slice(&443u16.to_be_bytes());
        assert_eq!(h, want);
        assert_eq!(h.len(), 16 + 12);
    }

    #[test]
    fn v2_ipv6_layout() {
        let h = header(
            ProxyVersion::V2,
            "[2001:db8::1]:40000",
            "[2001:db8::2]:8443",
        );
        assert_eq!(&h[..12], V2_SIGNATURE);
        assert_eq!(&h[12..16], [0x21, 0x21, 0x00, 0x24]);
        let src: Ipv6Addr = "2001:db8::1".parse().unwrap();
        let dst: Ipv6Addr = "2001:db8::2".parse().unwrap();
        assert_eq!(&h[16..32], src.octets());
        assert_eq!(&h[32..48], dst.octets());
        assert_eq!(&h[48..50], 40000u16.to_be_bytes());
        assert_eq!(&h[50..52], 8443u16.to_be_bytes());
        assert_eq!(h.len(), 16 + 36);
    }

    #[test]
    fn v2_mapped_and_mixed_families() {
        let mapped = header(
            ProxyVersion::V2,
            "[::ffff:10.1.2.3]:5000",
            "[::ffff:10.0.0.1]:443",
        );
        assert_eq!(mapped[13], 0x11);
        assert_eq!(&mapped[16..24], [10, 1, 2, 3, 10, 0, 0, 1]);
        // One header has one family: an IPv4 address beside an IPv6 one goes as IPv6.
        let mixed = header(ProxyVersion::V2, "10.1.2.3:5000", "[2001:db8::2]:443");
        assert_eq!(mixed[13], 0x21);
        let src = "10.1.2.3".parse::<std::net::Ipv4Addr>().unwrap();
        assert_eq!(&mixed[16..32], src.to_ipv6_mapped().octets());
    }
}
