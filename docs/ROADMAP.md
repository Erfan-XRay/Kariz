# Roadmap and design

## Layers

```
[ Port forward / UDP forward ]      user-facing listeners on the entry side
[ Session: stream + datagram ]      each TCP connection is a stream, each UDP flow a datagram flow
[ Mux (tcpmux) ]                    many sessions over few long-lived connections
[ Crypto: TLS 1.3 / AEAD + PSK ]
[ Transport ]  tcp | ws | wss | quic | kcp | udp | icmp
```

Transports fall in two groups:

- **Stream (reliable):** tcp, tcpmux, ws, wss, quic, kcp
- **Datagram (unreliable):** udp, icmp, QUIC datagrams

**UDP over any transport:** on stream transports, UDP packets are framed
(flow id + length) so UDP keeps working where UDP itself is blocked. On datagram
transports they are sent as-is for the lowest latency. UDP over TCP/WS suffers from
head-of-line blocking, so for games QUIC datagrams, KCP, raw UDP or ICMP are preferred.

Both **reverse** (exit dials entry) and **direct** (entry dials exit) modes are
supported for every transport.

## Phases

| Phase | Scope | Status |
|---|---|---|
| 1 | Project skeleton, CLI, TOML config, profiles, mutual auth, plain `tcp` transport, reverse + direct modes, tests, CI | **done (v0.1.0)** |
| 2 | Encryption layer, `tcpmux`, `ws` / `wss` (CDN friendly); plan in [PHASE2.md](PHASE2.md) | planned |
| 3 | UDP forwarding and UDP-over-stream framing | planned |
| 4 | `kcp` (full settings + Reed-Solomon FEC) and `quic` (quinn; BBR/Cubic, 0-RTT, datagrams, GSO/GRO) | planned |
| 5 | `icmp` transport (raw sockets, needs `CAP_NET_RAW`) | planned |
| 6 | Full `gaming` profile (packet duplication, DSCP), `stealth` profile (padding, timing), active-probe fallback | planned |
| 7 | Release builds (static musl for x86_64 / aarch64 / armv7, mimalloc), integration into XRayMesh as a backend | planned |

## Protocol (v1, tcp transport)

1. **Auth** (see `src/auth.rs`): dialer sends `ts | nonce | tag`, acceptor answers with
   its own tag. Mutual, replay-protected, no fixed bytes on the wire.
2. **Open** (see `src/proto.rs`): entry sends `kind | len | target`, exit dials the
   target and answers with a one-byte status.
3. **Relay**: raw bidirectional copy.

In reverse mode the auth happens when the exit fills the pool, so opening a user
connection costs one round trip plus the target dial. In direct mode the hello and the
open request go out in a single write.

Plain `tcp` is not encrypted and not camouflaged yet; phases 2 and 6 add that.
