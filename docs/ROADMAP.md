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
| 2 | Encryption layer, `tcpmux`, `ws` / `wss` (CDN friendly); plan in [PHASE2.md](PHASE2.md) | in progress (2.0-2.6 done) |
| 3 | UDP forwarding and UDP-over-stream framing | planned |
| 4 | `kcp` (full settings + Reed-Solomon FEC) and `quic` (quinn; BBR/Cubic, 0-RTT, datagrams, GSO/GRO) | planned |
| 5 | `icmp` transport (raw sockets, needs `CAP_NET_RAW`) | planned |
| 6 | Full `gaming` profile (packet duplication, DSCP), `stealth` profile (padding, timing), active-probe fallback | planned |
| 7 | Release builds (static musl for x86_64 / aarch64 / armv7, mimalloc), integration into XRayMesh as a backend | planned |

## Protocol (v2)

1. **Handshake** (see `src/crypto/handshake.rs`): mutual authentication with the shared
   token plus an X25519 exchange for forward secrecy. The hello is masked and padded to a
   random length, so it has no fixed bytes and no fixed size; replays are rejected.
2. **Records** (see `src/crypto/record.rs`): everything after the handshake is sent as
   AEAD records (ChaCha20-Poly1305 or AES-256-GCM, lengths encrypted too), with one key
   per direction. `encryption = "none"` keeps the handshake but skips the records.
3. **Open** (see `src/proto.rs`): entry sends `kind | len | target`, exit dials the
   target and answers with a one-byte status.
4. **Relay**: bidirectional copy through the record layer.

In reverse mode the handshake happens when the exit fills the pool, so opening a user
connection costs one round trip plus the target dial. In direct mode the open request is
sent as 0-RTT early data right behind the hello.

A connection that fails the handshake is not closed at once but drained for a random
5-30 s. Wire format v2 is not compatible with v0.1: upgrade both sides together.
Camouflage beyond that (padding and timing shaping, active-probe fallback) is phase 6.

Full design: [PHASE2.md](PHASE2.md).
