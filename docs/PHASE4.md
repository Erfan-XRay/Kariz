# Phase 4 plan: QUIC and KCP

Target release: **v0.4.0**. Scope from [ROADMAP.md](ROADMAP.md): `kcp` (full settings +
Reed-Solomon FEC) and `quic` (quinn; BBR/Cubic, 0-RTT, datagrams, GSO/GRO).

This document fixes the design and the order of work, so each step can land as its own
PR with CI green. As in phase 3, the decisions are settled here with their reasoning
(section 12).

## 1. Why, and what for

Every transport so far runs over TCP. That has two costs phase 3 measured and
documented:

- **Head-of-line blocking.** One lost TCP segment delays everything behind it, for every
  stream and every UDP flow on the connection. On a lossy path a game packet waits a
  whole retransmission round trip even though a newer packet already arrived.
- **Loss-based throughput collapse.** TCP (Cubic) halves its rate on every loss. On the
  links this project is for (long international paths with random loss, often with
  throttling that drops rather than queues), throughput falls far below the link rate.

UDP-based transports address both:

| | `quic` | `kcp` |
|---|---|---|
| What it is | QUIC (RFC 9000) via `quinn`: independent streams, unreliable datagrams (RFC 9221), TLS 1.3 built in, modern congestion control | KCP: an ARQ protocol over UDP that trades bandwidth for latency (aggressive retransmission, no slow start, optional congestion control off) |
| Best for | Mixed traffic, UDP flows (native datagrams, no head-of-line blocking), paths where UDP 443 looks normal (HTTP/3) | Lossy paths where TCP collapses; with FEC it recovers losses without waiting for retransmission |
| Looks like | HTTP/3 to a DPI (QUIC is common) | Opaque UDP (every packet encrypted, no fixed bytes) |

Non-goals (later phases): raw `udp` and `icmp` transports (phase 5); an unreliable
datagram path inside KCP, packet duplication and DSCP (phase 6, gaming profile); making
the QUIC handshake look like a browser's (phase 6, stealth).

Reality check: UDP is throttled or blocked on some Iranian networks, QUIC to port 443
in particular. These transports are options for the paths where UDP works well, not
replacements for `tcp` / `ws` / `wss`; the docs say so, and switching transport stays a
one-line config change.

## 2. The session layer

The roadmap's layer diagram has a `Session: stream + datagram` layer. Today that layer
is kmux alone: entry, exit and the UDP code call `MuxSession` / `MuxStream` directly.
QUIC brings its own streams and datagrams, and running kmux over one QUIC stream would
throw away exactly what QUIC is for (independent streams, unreliable datagrams). So
step 4.1 introduces the layer as an enum with two implementations:

```rust
pub enum Session { Kmux(Arc<MuxSession>), Quic(QuicSession) }
pub enum SessionStream { Kmux(MuxStream), Quic(QuicStream) }
```

with what the rest of the code uses today: `open(syn)` / `accept()` / `goaway()` /
`is_closed()` / `stream_count()` on sessions; `send(Bytes)` / `recv()` / `finish()` /
`reset(reason)` / `send_datagram` / `recv_datagram` / `reset_reason()` on streams. The
session pool, `maintain` (reconnect, rotation) and placement become generic over it.
Statically dispatched, like `TunnelStream`.

```
               tcp / tcpmux / ws / wss / kcp                 quic
 user conn ─> [ Session::Kmux ]                        [ Session::Quic ]
              kmux streams + DGRAM frames               QUIC streams + QUIC datagrams
              [ record layer (handshake v2) ]           [ TLS 1.3 inside QUIC ]
              [ TunnelStream: tcp | ws | wss | kcp ]    [ UDP socket (quinn) ]
```

*Status after 4.1:* `src/session/` holds the two enums (one variant, `Kmux`, for now)
and the manager moved from `src/mux/`: the pool places streams on any `Session`, and
`maintain` takes a `connect` that returns a ready `Session` (for kmux: dial, handshake,
start kmux over the link), so it no longer needs to know how a session is built.
`Channel::Mux` became `Channel::Stream(SessionStream)` and `relay_mux` became
`relay_stream`. `SessionStream` also implements `AsyncRead` / `AsyncWrite`. No test
changed; throughput is unchanged (checked A/B against `main`, alternating runs on the
same machine, since the machine's own speed varied between sessions).

KCP is a reliable byte stream, so it slots in under the existing stack as a new
`TunnelStream` variant (handshake, records and kmux unchanged). QUIC replaces the three
middle layers with its own.

## 3. QUIC (`transport = "quic"`)

- **Library:** `quinn` 0.11 with `rustls` 0.23 and the `ring` provider (no `aws-lc`), on
  tokio. GSO / GRO and path MTU discovery come from `quinn-udp` / `quinn-proto` and are
  on by default on Linux.
- **One QUIC connection = one session.** `mux.connections` QUIC connections are kept
  (default 2, see config), with the same reconnect / rotation logic as kmux sessions.
- **Streams:** each user TCP connection is a bidirectional QUIC stream. The open request
  is the first bytes on the stream; data follows without waiting (optimistic open, as
  with kmux). A failed target dial resets the stream with the `dial_failed` code.
  `mux.stream_window` and `mux.max_streams` map to quinn's per-stream receive window and
  concurrent stream limit.
- **UDP flows:** a flow opens a stream (open request with kind UDP, so the flow has a
  lifecycle), and its packets travel as **QUIC datagrams** prefixed with the stream id
  (QUIC varint). A packet larger than the current datagram limit (path MTU minus QUIC
  overhead, about 1,200-1,350 bytes) goes on the flow's stream instead, length-prefixed:
  it still arrives, just reliably. Tunnelled WireGuard (packets up to about 1,450 bytes)
  therefore wants its MTU lowered to about 1,280 to stay on datagrams; the docs say so.
- **Authentication, no extra round trip:** mutual TLS with identities derived from the
  token. Both sides derive the same Ed25519 key pair from the PSK (`BLAKE3-derive-key`,
  new context), each presents a self-signed certificate for that key, and each side's
  verifier accepts exactly that public key (checked on the certificate's
  SubjectPublicKeyInfo; the TLS handshake signature proves possession). Knowing the
  token is what makes a valid peer, as everywhere else in Kariz; TLS 1.3 gives forward
  secrecy; no certificate files to manage. Certificates are encrypted in TLS 1.3, so they
  are not visible on the wire.
- **Camouflage (basic):** configurable SNI (default: a common host name) and ALPN `h3`,
  so the handshake reads as HTTP/3. Shaping the ClientHello further is phase 6.
- **Congestion control:** `cubic` (default), `bbr`, `newreno`. quinn's BBR is marked
  experimental upstream; it is offered because it copes with random loss much better
  than Cubic, and the release benchmarks (4.6) decide whether it should be the default.
- **0-RTT:** session tickets are kept per dialer; a reconnect resumes with 0-RTT so the
  first stream's data leaves in the first flight. 0-RTT data can be replayed by an
  attacker; the only thing sent in it is stream opens, which a replay can only repeat
  (like re-sending a SYN), never forge. Kept, and documented.
- **Liveness:** QUIC keep-alive at `tuning.keepalive`, idle timeout at 3x that.
- `tunnel.encryption` must stay `auto` with QUIC (TLS always encrypts); `none` or an
  explicit cipher is a config error there.

## 4. KCP (`transport = "kcp"`)

```
 kmux / records / handshake (unchanged)
 [ TunnelStream::Kcp ]           AsyncRead + AsyncWrite over the KCP ARQ
 [ KCP segments ]                `kcp` crate (ikcp port), driven by our own task
 [ FEC ]                         Reed-Solomon, optional (4.5)
 [ packet protection ]           AEAD per UDP packet, key from the token
 [ UDP socket ]
```

- **ARQ core:** the `kcp` crate (MIT, a port of the reference `ikcp.c`), driven by our
  own tokio task: `update()` on its interval, `input()` for received packets, output
  through the layers below. We do not use `tokio_kcp`, because FEC and packet
  protection must sit between KCP and the socket.
- **Packet protection:** every UDP packet is `nonce (12, random) | AEAD(ciphertext) |
  tag (16)` under a key derived from the token (ChaCha20-Poly1305, or AES-256-GCM with
  hardware AES). KCP's plaintext header (conversation id, sequence numbers, window) is
  otherwise a clear fingerprint; this hides it, gives every packet a random-looking
  content and length variation from the payload, and lets the listener drop anything not
  made with the token before it reaches KCP: to a probe the port stays silent, like a
  closed UDP port. 28 bytes per packet. The tunnel's own handshake and records still
  run on top, so the protection key being token-derived (no forward secrecy at this
  layer) does not weaken the traffic's confidentiality.
- **Connections:** the listener has one UDP socket and demultiplexes by peer address and
  KCP conversation id; the dialer uses a fresh socket (random source port) per
  connection. A new conversation starts only from a packet that authenticates.
- **Settings**, as in kcp-go: `mode = "normal" | "fast" | "fast2" | "fast3" | "manual"`
  presets for (`nodelay`, `interval`, `resend`, `no_congestion`), plus `send_window`,
  `recv_window`, `mtu`. Default `fast2`, window 1024, MTU 1350 (the UDP packet with
  protection and FEC headers stays under 1,472 bytes, so it is not fragmented on a 1500
  byte path).
- **UDP flows over KCP** travel as kmux `DGRAM` frames inside the KCP stream: reliable
  and ordered, so a lost KCP segment still delays them, but only by KCP's fast
  retransmission (typically one RTT or less, or zero with FEC) rather than TCP's. An
  unreliable path next to KCP is part of phase 6's gaming work.

## 5. FEC (KCP)

- Reed-Solomon over groups of `fec_data` consecutive KCP packets plus `fec_parity`
  parity packets (`reed-solomon-simd`: SIMD, O(n log n)). A receiver that gets any
  `fec_data` of the `fec_data + fec_parity` packets of a group rebuilds the missing data
  packets at once, without a retransmission round trip.
- Header per packet (inside the protection): group id (4) | index (1) | original length
  (2), shards padded to the longest packet of the group (even length, as the library
  requires).
- A group that does not fill within one KCP interval is closed early (parity computed
  over the packets it has), so FEC never adds more than an interval of delay.
- Default off (`fec_data = 0`); `10 / 3` is the suggested starting point for lossy links
  (30 % more packets, recovers up to 3 losses per 13).

## 6. Configuration

```toml
[tunnel]
transport = "quic"                # ... | quic | kcp
remote = "203.0.113.1:443"

[tunnel.quic]                     # all optional
congestion = "cubic"              # cubic | bbr | newreno
sni = "www.example.com"           # server name in the handshake (camouflage)
alpn = "h3"

[tunnel.kcp]                      # all optional
mode = "fast2"                    # normal | fast | fast2 | fast3 | manual
# nodelay = true                  # manual mode only: nodelay, interval_ms, resend,
# interval_ms = 10                #   no_congestion
# resend = 2
# no_congestion = true
send_window = 1024
recv_window = 1024
mtu = 1350
fec_data = 0                      # e.g. 10 data + 3 parity packets per group
fec_parity = 0
```

`[tunnel.mux]` keeps its meaning for QUIC (connections, max streams, stream window); its
default for QUIC is on with 2 connections. Validation: `[tunnel.quic]` / `[tunnel.kcp]`
only with the matching transport; bounds on windows, MTU (576-1450) and FEC shards
(data 1-64, parity 1-32, or both 0); QUIC with `encryption` other than `auto` rejected.

## 7. Code layout

```
src/session.rs            Session / SessionStream enums, generic pool and maintain
src/transport/quic.rs     endpoint setup, token-derived mutual TLS, QuicSession
src/transport/kcp/mod.rs  KcpStream (TunnelStream variant), listener / dialer, driver
src/transport/kcp/protect.rs   packet protection
src/transport/kcp/fec.rs  Reed-Solomon groups
tests/lossy.rs (or in tunnel.rs)  UDP link emulator: loss, delay, jitter, reordering
```

Cargo features `quic` and `kcp`, both on by default, so a minimal build can leave them
out (quinn roughly doubles the dependency tree).

## 8. Test harness: a lossy link

The claims of this phase are about lossy paths, so they get measured on one. A UDP proxy
in the test suite forwards packets between the sides with configurable **loss**
(random, and in bursts), **delay** and **jitter**, **reordering** and a **rate limit**.
A TCP proxy with the same knobs (drop is emulated with delay spikes, since a TCP proxy
cannot lose bytes) is used for the TCP-based transports. Matrix for the release
benchmark: RTT 60 ms, loss 0 / 1 / 5 %, rate 50 Mbit/s; measure bulk throughput and UDP
flow round trips (p50 / p99) for `tcpmux`, `quic` (Cubic and BBR) and `kcp` (with and
without FEC).

## 9. Work breakdown

| Step | Content | Done when |
|---|---|---|
| **4.0** Plan (done) | This document. | Decisions settled (section 12). |
| **4.1** Session layer (done) | `Session` / `SessionStream` over kmux; pool, `maintain`, entry, exit and UDP code use it. | No behaviour change; every existing test passes unchanged. |
| **4.2** QUIC | quinn endpoints in both modes, token-derived mutual TLS, streams and datagrams (stream fallback for large packets), congestion choice, keep-alive, 0-RTT, config and validation. | E2E matrix rows for `quic` (reverse / direct), TCP and UDP scenarios pass; wrong token fails the TLS handshake; a packet above the datagram limit still arrives; the TLS identity tests (wrong key rejected both ways). |
| **4.3** Lossy link harness | UDP / TCP link emulator in tests; baseline numbers for `tcpmux` vs `quic`. | Emulator unit tests (loss rate, delay within bounds); benchmark prints the matrix. |
| **4.4** KCP | Packet protection, KCP driver, listener / dialer, settings and presets. | E2E rows for `kcp` (reverse / direct, mux on / off); probes get no answer; tampered packets are dropped; transfer integrity under 5 % loss. |
| **4.5** FEC | Reed-Solomon groups, early group close, recovery. | Unit tests: any `data` of `data + parity` rebuild the group; delay bound; E2E under loss with fewer retransmissions than without FEC. |
| **4.6** Release | Lossy-link benchmark table, defaults decided from it (QUIC congestion, KCP mode), sample configs, README / README_FA, CHANGELOG, version `0.4.0`. | Numbers in the README; release tagged. |

## 10. Targets

On the emulated link (60 ms RTT, 50 Mbit/s):

- At 1 % loss, `quic` (BBR) and `kcp` reach at least 3x the bulk throughput of
  `tcpmux`; at 5 % loss, `kcp` with FEC at least 5x.
- UDP flows over `quic` datagrams: p99 round trip under 1.5x the link RTT at 1 % loss
  (no head-of-line blocking), against several RTTs for `tcpmux`.
- No loss: `quic` within 80 % of `tcpmux` throughput on localhost.
- Memory: an idle QUIC or KCP session under 1 MiB per side; 100 idle streams no worse
  than kmux.

## 11. Risks

| Risk | Mitigation |
|---|---|
| UDP throttled or blocked on the path (common for QUIC on 443 in Iran). | Documented; transports stay a config switch away; phase 6 looks at port hopping. |
| QUIC fingerprint (quinn's transport parameters, ClientHello). | SNI / ALPN configurable now; deeper shaping in phase 6. |
| KCP spends bandwidth (aggressive retransmission, FEC overhead) and CPU (userspace ARQ). | Presets from gentle (`normal`) to aggressive (`fast3`); FEC off by default; CPU measured in 4.6. |
| quinn BBR is experimental. | Cubic default unless 4.6 shows BBR clearly better without problems. |
| Datagram size limit breaks large UDP packets. | Stream fallback (they still arrive); MTU advice in the docs. |
| Bigger binary and dependency tree. | Cargo features to build without QUIC / KCP. |

## 12. Decisions

1. **A session layer (enum over kmux and QUIC) instead of kmux over one QUIC stream.**
   Running kmux inside a single QUIC stream would reintroduce head-of-line blocking
   and make datagrams reliable, which is exactly what QUIC is chosen to avoid. The
   roadmap already names this layer; the enum keeps dispatch static.
2. **QUIC authenticates with mutual TLS derived from the token**, not with handshake v2
   on a first stream. Same trust model (the token is the identity), no extra round
   trip, no certificate files, and nothing to bind between two protocols (a handshake on
   top of TLS would need channel binding to stop a relay in the middle).
3. **No second encryption layer inside QUIC.** TLS 1.3 already gives confidentiality,
   integrity and forward secrecy with keys only the two token holders can reach;
   adding records on top would cost CPU for nothing.
4. **KCP is a stream transport under the existing stack**, with its own packet
   protection. It then reuses the handshake, records and kmux unchanged, and the
   protection layer hides KCP's otherwise obvious headers and keeps the port silent to
   probes. An unreliable datagram path for KCP is deferred to phase 6 (gaming).
5. **Own KCP driver on the `kcp` crate** rather than `tokio_kcp`: FEC and protection
   have to sit between KCP and the socket.
6. **`reed-solomon-simd`** for FEC: fastest option, no compatibility constraint (the
   wire format is ours).
7. **Datagram-or-stream for QUIC UDP flows**: packets that fit go as datagrams, larger
   ones on the flow's stream, so nothing is silently dropped because of path MTU.
8. **Cargo features** `quic` and `kcp`, on by default.
