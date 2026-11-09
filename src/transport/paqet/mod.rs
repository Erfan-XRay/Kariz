//! `transport = "paqet"`: KCP packets carried in the payload of synthetic TCP
//! segments over a Linux IPv4 raw socket.
//!
//! The transport is not wire-compatible with the upstream Go project of the same name
//! (`hanselime/paqet`): it reuses Kariz's own packet protection, KCP, FEC, datagram and
//! keepalive code, with a domain-separated key, and only the carrier below them is new.
//!
//! ```text
//!  handshake / records / kmux      unchanged, over a KcpStream
//!  KCP / protection / FEC          unchanged (Paqet changes only the packet key)
//!  synthetic IPv4 + TCP            one sealed packet per segment, no handshake
//!  raw IP socket (IPPROTO_TCP)     kernel writes and reads the IPv4 header
//! ```
//!
//! This phase supports IPv4 only and requires `CAP_NET_RAW` (root in practice). The
//! kernel's TCP stack also sees the synthetic segments and answers unknown ones with
//! RST, so the host needs scoped firewall rules; see `docs/paqet.md`.

pub mod packet;

#[cfg(all(target_os = "linux", feature = "kcp"))]
pub(crate) mod raw;
