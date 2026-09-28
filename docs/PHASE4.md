# Phase 4 plan: QUIC and KCP

Target release: **v0.4.0**. Scope from [ROADMAP.md](ROADMAP.md): `kcp` (full settings +
Reed-Solomon FEC) and `quic` (quinn; BBR/Cubic, datagrams, GSO/GRO).

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
- **Camouflage (basic):** configurable SNI (default: the remote's host name, see
  decision 10) and ALPN `h3`,
  so the handshake reads as HTTP/3. Shaping the ClientHello further is phase 6.
- **Congestion control:** `cubic` (default), `bbr`, `newreno`. quinn's BBR is marked
  experimental upstream; it is offered because it copes with random loss much better
  than Cubic, and the release benchmarks (4.6) decide whether it should be the default.
- **0-RTT** (dropped in 4.2, see decision 9): session tickets are kept per dialer; a
  reconnect resumes with 0-RTT so the first stream's data leaves in the first flight.
  0-RTT data can be replayed by an attacker; the only thing sent in it is stream opens,
  which a replay can only repeat (like re-sending a SYN), never forge. Kept, and
  documented.
- **Liveness:** QUIC keep-alive at `tuning.keepalive`, idle timeout at 3x that.
- `tunnel.encryption` must stay `auto` with QUIC (TLS always encrypts); `none` or an
  explicit cipher is a config error there.

*Status after 4.2:* `transport = "quic"` works in both modes, for TCP and UDP. The code
is in `src/transport/quic.rs` (identity, TLS configs, endpoints, dialer / listener) and
`src/session/quic.rs` (`QuicSession` / `QuicStream`: open requests, reset codes,
datagrams with stream fallback, `GOAWAY`), with `Session::Quic` / `SessionStream::Quic`
beside the kmux variants. Entry and exit reuse the pool, `maintain` and the relay
unchanged. Where the result differs from the plan above, and why:

- **Liveness:** pings every `keepalive / 3`, peer dead after `2 x keepalive` of silence.
  The dead-peer time matches kmux (silent for two periods); pings go out more often
  than kmux's because UDP NAT mappings often expire after 30 s, the default keepalive.
- **No 0-RTT** (decision 9). TLS session resumption is on (rustls defaults), which
  skips the certificate exchange on reconnects; early data is not.
- **SNI defaults to the host in `tunnel.remote`** (decision 10); with an IP address
  there, no SNI is sent, as with any client dialing an address.
- **A wrong token fails with a `bad_certificate` alert** on whichever side checks first
  (the client, for the server's certificate). A client that accepts any server but
  presents another key completes its side (TLS 1.3 clients finish before the server
  checks them) and is closed by the server's alert; the server opens nothing on a
  connection before its handshake is complete.
- **Stateless resets survive a restart** (not in the plan): the reset key and the key
  that marks our connection IDs are derived from the token. An endpoint restarted on
  the same port then answers packets of the old connections with a stateless reset,
  and the peer reconnects at once instead of after the idle timeout (quinn's defaults
  are random per process: the new endpoint would drop those packets unanswered).
  Unit-tested by killing a listener's runtime and binding a new one on its port.
- **Cargo features** (decision 8) are left to 4.6: quinn and rcgen add 24 crates
  (82 to 106), well short of the "doubles" expected, and gating `quic` and `kcp` at
  once, with the binary sizes measured, is less churn than twice.
- `SessionStream` no longer implements `AsyncRead` / `AsyncWrite` (added in 4.1): a
  QUIC stream would have needed a second buffering layer for it, and only the relay
  used them, which now calls `send` / `recv` directly.

Tests: the `quic_reverse` / `quic_direct` rows of the E2E matrix (concurrent echoes,
an 8 MiB transfer, wrong token, unreachable target, exit restart, UDP echo with packets
from 1 byte to 60 KB, the larger ones on the stream fallback); unit tests for the
identity, the token check in both directions, every congestion controller, the
stateless reset after a restart, streams and reset codes, datagram sizes from 0 to
65,535 bytes (across the datagram limit), early datagrams and `GOAWAY` / drain.

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

*Status after 4.4:* `transport = "kcp"` works in both modes, with and without mux, for
TCP and UDP. The code is in `src/transport/kcp/` (`mod.rs`: `KcpStream` and its halves,
the driver, listener and dialer; `protect.rs`: packet protection), with `TunnelStream::Kcp`
beside the other stream transports. Handshake, records and kmux run on it unchanged.
Where the result differs from the plan above, and why:

- **A packet type inside the protection.** The plaintext is `type (1) | body`: KCP
  segments, a ping, or a close. That makes the overhead 29 bytes, not 28. KCP has no
  keep-alive and no way to end a conversation, and without those a conversation whose
  peer is gone stays open forever.
- **KCP runs in message mode, each message tagged:** data, FIN (end of the stream in that
  direction, so half-close works as over TCP), or the dialer's opening message. The
  opening message makes every conversation start with a data segment numbered 0, whoever
  speaks first above.
- **Starting and ending conversations.** The listener starts a conversation only for an
  authenticated packet whose first segment is data segment 0, and only while its accept
  queue has room. A packet for a conversation it does not know gets a close when it comes
  from an earlier conversation: a ping, or segments whose `una` shows the dialer has heard
  from us. So a restarted listener ends old conversations at once, as a restarted QUIC
  endpoint does with a stateless reset. A dialer that has heard nothing yet only lost its
  opening packet, which KCP resends; answering it with a close failed the first version
  of the test with 20 connections, whose bursts overflowed the listener's socket buffer.
  Both sides send a close when they are done: both FINs delivered and acked, or nobody
  reading any more. A close that comes before the peer's FIN is a reset.
- **Liveness as with QUIC:** a ping every `keepalive / 3`, and a peer silent for
  `2 x keepalive` is dead. KCP's own dead-link check (20 retransmissions of one segment)
  takes minutes with backoff.
- **ICMP errors are ignored** (port unreachable, reported on the next receive). They come
  from a peer that is restarting, or from anyone, since ICMP is easy to forge. A peer
  that is really gone falls silent.
- **ChaCha20-Poly1305 only**, no AES-GCM option. Nonces are random under one long-lived
  key (from BLAKE3 in XOF mode, seeded by the OS, so there is no system call per
  packet). If two nonces ever collided, ChaCha20-Poly1305 would give away that one
  packet's authentication key; GCM would give away the key for every packet.
- **Flow control:** writers queue at most 64 KiB for the driver, which holds at most two
  send windows in KCP. The driver takes at most 256 KiB out of KCP ahead of the reader;
  beyond that KCP's receive window fills and the peer slows down.

Tests: unit tests for the protection (round trip, every changed byte rejected, another
key rejected) and the conversation rules; over local sockets: echo with half-close, an
8 MiB transfer, 20 connections on one listener, probes (random bytes, a KCP packet in
the clear) get no answer and open nothing, a wrong key opens nothing, 1 MiB through a
proxy that corrupts a fifth of the packets, a restarted listener closing old
conversations, a dropped stream ending cleanly, a vanished peer timing out. E2E: rows
`kcp_reverse`, `kcp_direct`, `kcp_reverse_no_mux`, `kcp_direct_no_mux_chacha`, and 4 MiB
echoed intact over a link with 5 % loss each way (direct and reverse, with and without
mux).

Lossy-link benchmark (section 8, same machine and setup, stream window 4 MiB),
`kcp fast2` next to the rows there:

| Loss | Download | UDP idle p50 / p99, lost | UDP during download p50 / p99, lost | Queue drops |
|---|---|---|---|---|
| 0 % | 30.4 Mbit/s | 63 / 64 ms, 0 % | 81 / 251 ms, 0 % | 36,549 |
| 1 % | 28.3 Mbit/s | 62 / 142 ms, 0 % | 89 / 285 ms, 0 % | 31,483 |
| 5 % | 25.8 Mbit/s | 63 / 263 ms, 0 % | 127 / 348 ms, 0 % | 26,690 |

KCP keeps most of its rate under loss: 14x `tcpmux` at 1 % and 32x at 5 %, without FEC
(targets: 3x, and 5x with FEC). Its UDP flows lose nothing, since they ride the reliable
stream, but a lost segment costs them a retransmission (p99 142-263 ms idle). The
default window (1024 packets, about 1.4 MB) is well above this path's bandwidth-delay
product plus queue (about 690 KB). With congestion control off (every preset), KCP
keeps the queue overflowing: about 30,000 drops in 10 s. That costs it the clean-link
rate (30 against 48 Mbit/s) and the p99 of datagrams during a download. A smaller default
window, or `no_congestion = false`, are for 4.6 to measure; FEC (4.5) does not help
with drops that KCP causes itself.

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

*Status after 4.5:* `[tunnel.kcp] fec_data` / `fec_parity` turn FEC on. The code is in
`src/transport/kcp/fec.rs` (encoder and decoder, without sockets) and in `mod.rs`
(packet types, sending, feeding rebuilt packets to KCP). Where the result differs from
the plan above, and why:

- **Two packet types of their own** (inside the protection): a data shard
  (`conv | group | index | len | KCP segments`) and a parity shard
  (`conv | group | index | data count | parity count | shard`). The conversation id
  comes first, so the listener routes parity packets like any other; the counts let a
  group closed early be decoded. Packets say what they are, so only the sender's setting
  matters: a side without FEC still reads FEC packets. The largest packet (a parity
  shard) adds 43 bytes, so the MTU limit with FEC is 1429. The limit without FEC is
  1443, not the plan's 1450, which with the 29 bytes of 4.4 would have gone past 1472.
- **Groups closed early get proportional parity:** `ceil(parity x k / data)`, at least
  one, so a lone packet (an ack, a game packet) gets one parity packet after the
  interval, not three.
- **Rebuilt packets are cleaned before KCP sees them.** They arrive late, and the `kcp`
  crate, unlike kcp-go, cannot tell them from regular packets. Their data segments that
  came meanwhile (resent) are left out, since KCP would ack them again with their old
  send time, which inflates the peer's round-trip time. Their acks get a send time in the
  future, which KCP takes as no round-trip sample at all (kcp-go skips that sample for
  FEC packets in the same way).
- **What FEC saves, measured:** 2 MiB one way, 5 % loss each way, 20 ms RTT, preset
  `normal`: without FEC the sender resent 51-175 segments, with 10 / 3 it resent 0 in
  most runs (57 in one). On the `fast` presets (minimum timeout 30 ms) KCP also resends
  segments whose acks were merely delayed (51-192 duplicates at the receiver in the same
  test). FEC cannot save those, so the unit test uses `normal`.

Tests: the encoder and decoder alone (a full group sends data then parity; *every* way
of losing up to `parity` of the `data + parity` shards rebuilds the group, and more
losses do not; a slow group is closed after the delay, counted from its first packet;
parity before data, duplicates, old groups, nonsense parity); over local sockets (fewer
retransmissions with FEC, as above; FEC on one side only; a lone packet's group closed
on time). E2E: row `kcp_reverse_fec`, and FEC in the 5 % loss transfer. Config: bounds,
both-or-neither, MTU with FEC.

Lossy-link benchmark (section 8, same machine and setup), `kcp fast2` with and without
FEC 10 / 3:

| Loss | FEC | Download | UDP idle p50 / p99 | UDP during download p50 / p99 | Queue drops |
|---|---|---|---|---|---|
| 0 % | off | 33.5 Mbit/s | 62 / 64 ms | 92 / 256 ms | 35,163 |
| 0 % | 10 / 3 | 23.1 Mbit/s | 62 / 64 ms | 83 / 294 ms | 52,134 |
| 1 % | off | 29.4 Mbit/s | 62 / **142** ms | 94 / 293 ms | 30,217 |
| 1 % | 10 / 3 | 24.1 Mbit/s | 63 / **65** ms | 88 / 259 ms | 50,278 |
| 5 % | off | 24.7 Mbit/s | 63 / **262** ms | 118 / 420 ms | 26,127 |
| 5 % | 10 / 3 | 25.1 Mbit/s | 63 / **142** ms | 87 / 415 ms | 48,318 |

(UDP packets over KCP are never lost: they ride the reliable stream.) What this says:

- **FEC's gain here is latency, not throughput.** A lost packet of a sparse flow is
  rebuilt from parity within a KCP interval instead of waiting for a retransmission:
  the idle UDP p99 falls from 142 to 65 ms at 1 % loss (the link RTT is 60) and from 262
  to 142 ms at 5 %. That is what the gaming profile (phase 6) wants.
- **Throughput does not improve, and without loss it drops** (33.5 to 23.1 Mbit/s).
  KCP's 1024-packet window without congestion control already keeps this bottleneck's
  queue overflowing (4.4), and 30 % more packets overflow it further (about 50,000
  drops instead of 35,000). Losses KCP causes itself are not what FEC is for.
  Against `tcpmux`, KCP with FEC is still 10x faster at 1 % loss and 31x at 5 % (the
  target was 5x), but no faster than KCP without it.
- For 4.6: a window sized to the path (or KCP's congestion control on) is the first
  lever for KCP's throughput; FEC stays off by default and is advised for latency-
  sensitive traffic on lossy links.

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
only with the matching transport; bounds on windows, MTU (576-1443, 1429 with FEC) and FEC
shards (data 1-64, parity 1-32, or both 0); QUIC with `encryption` other than `auto`
rejected.

## 7. Code layout

```
src/session.rs            Session / SessionStream enums, generic pool and maintain
src/transport/quic.rs     endpoint setup, token-derived mutual TLS, QuicSession
src/transport/kcp/mod.rs  KcpStream (TunnelStream variant), listener / dialer, driver
src/transport/kcp/protect.rs   packet protection
src/transport/kcp/tests.rs     KCP over local sockets
src/transport/kcp/fec.rs  Reed-Solomon groups
tests/link/mod.rs         UDP / TCP link emulators: loss, delay, jitter, reordering, rate
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

*Status after 4.3:* `tests/link/mod.rs` holds a `UdpLink` and a `TcpLink`, local proxies
that the E2E harness puts between the two sides (`start_via`). Each direction has one
**bottleneck** (rate counting IP / UDP / TCP headers, a queue bounded in time, tail drop)
that everything going that way shares. Every flow then gets its own **loss** (random, or
in bursts with a Gilbert model of a given mean length), **delay**, **jitter** (order kept)
and **reordering** (a share of packets held back by a gap), from a seeded generator so a
run can be repeated. Where the result differs from the plan above, and why:

- **The TCP link models TCP instead of adding delay spikes.** A proxy that forwards bytes
  without loss leaves the TCP stacks at both ends unaware of any loss. They never shrink
  their windows, so `tcpmux` would get almost the full rate at 5 % loss, the opposite of
  what happens on a real path. Delay spikes reproduce the stalls but not the rate
  collapse. So each direction of each proxied connection runs a model of the sender's TCP
  over the path: 1448-byte segments, Cubic (RFC 9438: slow start, the 0.7 decrease, the
  cubic growth and the Reno-friendly estimate), SACK-style fast retransmission after
  three duplicate acks, a tail loss probe after three smoothed RTTs, and in-order delivery
  (a lost segment holds back everything after it). The sender holds at most 16 KiB not
  yet sent, like Kariz's `TCP_NOTSENT_LOWAT` on mux connections. Checked against Mathis
  et al. (window of about 1.22 / sqrt(p) segments): at 2 % loss and 20 ms RTT the model
  gets about 0.8 of that (unit test bounds: 0.5-2x). Not modelled: retransmission
  timeouts and their backoff, delayed acks, lost acks. The model is therefore somewhat
  kinder to TCP than a real path, so if anything the comparison favours `tcpmux`.
- **Model events run at the time they are due**, not when the timer fires, so a late
  wake-up does not stretch the emulated RTT. The benchmark also asks Windows for 1 ms
  timer ticks (`fine_timers`): the default 15.6 ms tick added up to 30 ms to p99 round
  trips.

Tests: loss rate and burst length from the loss model (pure, 2 M draws), queue drops and
rate spacing, and over real sockets: UDP loss rate, delay within bounds with jitter
keeping order, reordering, rate limit, several dialers behind one link. For TCP: a
stream that stays intact under 5 % bursty loss, the round trip, three downloads sharing
and filling the bottleneck, and the Mathis check above.

Benchmark: `cargo test --release --test tunnel lossy_link -- --ignored --nocapture`
(`KARIZ_BENCH_LOSS=1` for one loss rate). Direct mode; a 10 s download (one user
connection) after 3 s of warm-up; a UDP flow sending 100-byte packets every 20 ms
without waiting for answers, first on the idle tunnel, then during the download. Streams
get a 4 MiB window so congestion control, not flow control, sets the rate (rows marked
*256 KiB* use the default). Baseline, release build on the Windows 11 development
machine (Linux numbers follow in 4.6):

| Loss | Transport | Download | UDP idle p50 / p99, lost | UDP during download p50 / p99, lost |
|---|---|---|---|---|
| 0 % | `tcpmux` | 48.1 Mbit/s | 63 / 64 ms, 0 % | 104 / 113 ms, 0 % |
| 0 % | `quic` Cubic | 47.9 Mbit/s | 63 / 64 ms, 0 % | 108 / 112 ms, 0 % |
| 0 % | `quic` BBR | 46.9 Mbit/s | 63 / 64 ms, 0 % | 110 / 113 ms, **52 %** |
| 0 % | `tcpmux`, 256 KiB | 25.1 Mbit/s | 63 / 64 ms, 0 % | 64 / 84 ms, 0 % |
| 0 % | `quic` Cubic, 256 KiB | 30.5 Mbit/s | 63 / 64 ms, 0 % | 64 / 69 ms, 0 % |
| 1 % | `tcpmux` | 2.2 Mbit/s | 63 / 164 ms, 0 % | 63 / 183 ms, 0 % |
| 1 % | `quic` Cubic | 2.4 Mbit/s | 63 / 64 ms, 1.0 % | 63 / 64 ms, 1.4 % |
| 1 % | `quic` BBR | 46.2 Mbit/s | 63 / 64 ms, 1.2 % | 110 / 113 ms, **54 %** |
| 5 % | `tcpmux` | 1.0 Mbit/s | 64 / 282 ms, 0 % | 65 / 263 ms, 0 % |
| 5 % | `quic` Cubic | 1.0 Mbit/s | 63 / 64 ms, 10 % | 63 / 64 ms, 8.8 % |
| 5 % | `quic` BBR | 44.7 Mbit/s | 63 / 64 ms, 8.8 % | 110 / 113 ms, **52 %** |

(UDP loss on `quic` is per round trip, so about twice the link's one-way loss; `tcpmux`
retransmits instead and pays in p99.) What the baseline says, for 4.4-4.6:

- **Loss-based congestion control collapses on random loss, QUIC or not.** quinn's Cubic
  does no better than TCP: 2.4 against 2.2 Mbit/s at 1 %. QUIC's gain there is the UDP
  flow: datagrams keep p99 at the link RTT, where `tcpmux` waits for retransmissions
  (164 ms at 1 %, 282 ms at 5 %).
- **quinn's BBR holds the rate under loss** (46 and 45 Mbit/s at 1 and 5 %, 20-45x the
  others), well past the 3x target of section 10. **But it overfills the bottleneck
  queue:** about 60,000 packets dropped at the queue in 10 s, and half of the UDP
  datagrams on the same connection with them. BBR as the default needs this fixed (or a
  pacing / cwnd cap) first; 4.6 decides.
- **The default 256 KiB stream window caps a stream at 256 KiB per round trip**, about
  25-30 Mbit/s on this path. The `throughput` profile (1 MiB) lifts that; whether the
  default should grow is for 4.6.

## 9. Work breakdown

| Step | Content | Done when |
|---|---|---|
| **4.0** Plan (done) | This document. | Decisions settled (section 12). |
| **4.1** Session layer (done) | `Session` / `SessionStream` over kmux; pool, `maintain`, entry, exit and UDP code use it. | No behaviour change; every existing test passes unchanged. |
| **4.2** QUIC (done) | quinn endpoints in both modes, token-derived mutual TLS, streams and datagrams (stream fallback for large packets), congestion choice, keep-alive, 0-RTT, config and validation. | E2E matrix rows for `quic` (reverse / direct), TCP and UDP scenarios pass; wrong token fails the TLS handshake; a packet above the datagram limit still arrives; the TLS identity tests (wrong key rejected both ways). |
| **4.3** Lossy link harness (done) | UDP / TCP link emulator in tests; baseline numbers for `tcpmux` vs `quic`. | Emulator unit tests (loss rate, delay within bounds); benchmark prints the matrix. |
| **4.4** KCP (done) | Packet protection, KCP driver, listener / dialer, settings and presets. | E2E rows for `kcp` (reverse / direct, mux on / off); probes get no answer; tampered packets are dropped; transfer integrity under 5 % loss. |
| **4.5** FEC (done) | Reed-Solomon groups, early group close, recovery. | Unit tests: any `data` of `data + parity` rebuild the group; delay bound; E2E under loss with fewer retransmissions than without FEC. |
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
9. **No QUIC 0-RTT** (reversed in 4.2). Sessions are pooled and dialed before users
   arrive, so 0-RTT would only shorten reconnects that no user is waiting on. What it
   would put in the first flight is a stream's open request *and the user's first
   bytes* (streams are opened optimistically), and 0-RTT data can be replayed: a
   repeated non-idempotent request to the target is too high a price for a round trip
   nobody waits for. The acceptor also never sends before the handshake completes (no
   0.5-RTT), so nothing reaches a peer before its certificate is checked.
10. **QUIC SNI defaults to the `tunnel.remote` host**, not to a well-known name. A
    borrowed name is easy to check against the server's address and says something
    false about the connection; the dialer's own host name is what an ordinary client
    sends. `tunnel.quic.sni` sets any other.
