# Phase 3 plan: UDP forwarding

Target release: **v0.3.0** (all steps done; see the status notes in each section). Scope from [ROADMAP.md](ROADMAP.md): UDP forwarding and
UDP-over-stream framing.

This document fixes the design, the wire formats and the order of work, so each step
can land as its own PR with CI green.

## 1. Goals and non-goals

Goals:

- `[[forward]]` rules for UDP (`protocol = "udp"`), and for TCP and UDP on the same port
  (`protocol = "tcp+udp"`, for DNS, QUIC / HTTP/3 on 443, some games).
- UDP works over **every transport that exists after phase 2** (`tcp`, `tcpmux`, `ws`,
  `wss`), in both modes, so it keeps working where UDP itself is blocked or throttled.
- UDP keeps UDP's character inside the tunnel: packets are never merged or split, a full
  queue drops packets instead of stalling, and packets do not wait behind bulk TCP data
  sharing the same connection.
- No breaking change: TCP between v0.3 and v0.2 keeps working; UDP needs both sides at
  v0.3 and fails cleanly otherwise.

Non-goals (later phases):

- Datagram transports (raw UDP, QUIC datagrams, KCP, ICMP): phases 4 and 5. The flow
  layer designed here is what they will carry, so they only add a new way to move the
  same datagrams.
- Recovering from loss or head-of-line blocking on stream transports. Inside `tcp` / `ws`
  / `wss`, a lost TCP segment still delays every packet behind it; that is inherent to
  carrying UDP over TCP and is the reason phase 4 exists.
- NAT behaviour beyond port forwarding (full-cone, STUN, P2P hole punching).

## 2. Model

```
 client ──UDP──> [ entry: UDP listener ]           [ exit: UDP socket per flow ] ──UDP──> target
                   flow table                        connected to the target
                   (client addr -> flow)                    ▲
                        │                                   │
                        └── one flow = one stream (mux) or one channel (no mux) ─┘
```

- A **flow** is one client address (`ip:port`) talking to one forward rule. It is the
  UDP equivalent of a TCP connection: the entry opens a flow when the first packet from a
  new client address arrives, and closes it after `udp_timeout` without traffic in
  either direction.
- On the exit, each flow gets its own UDP socket, bound to an ephemeral port and
  **connected** to the target (the target sees one source port per client, as behind a
  NAT, and packets from any other address are dropped by the kernel).
- The target name is resolved once per flow, on the exit, like TCP targets.

## 3. Wire format

### Open request

`proto.rs` gains a kind; the encoding is unchanged:

```text
kind (1) | target_len (2, BE) | target        kind: 1 = TCP (v0.2), 2 = UDP
```

### Over mux: stream + `DGRAM` frames

A flow is a mux stream opened with `SYN(Open{kind: UDP})`. Its packets travel as the
`DGRAM` frames phase 2 reserved, one packet per frame, with the flow's stream id:

```text
type = 8 (DGRAM) | stream_id (4, BE) | length (2, BE) | packet
```

- `DATA` is not used on UDP streams. The session does not look inside the `SYN`, so it
  does not police this: `DATA` sent there only fills that stream's own window, which
  nobody reads, and harms no other stream. `FIN` / `RST` end the flow as for TCP
  streams; a failed target lookup is `RST(dial_failed)`.
- `DGRAM` frames are **not** counted against the stream's flow-control window: a
  datagram that cannot be queued is dropped, never waited for.
- A `DGRAM` for an unknown or closed stream is dropped silently (it can race with a
  close).
- Optimistic open, as for TCP: the entry sends `SYN` and the first packets right after
  it. The exit queues up to a small number of packets per flow while it resolves the
  target and binds the socket.
- v0.2 peers ignore `DGRAM` frames and reset the unknown open kind, so a v0.3 entry with
  a v0.2 exit sees the flow reset and logs that the exit side does not support UDP.

### Without mux: one channel per flow, length-prefixed packets

The flow takes a whole channel (a pooled connection in reverse mode, a new connection
in direct mode), sends `Open{kind: UDP}`, waits for the status byte as TCP does, then
both directions carry

```text
len (2, BE) | packet
```

This is the "UDP-over-stream framing" of the roadmap. It costs a tunnel connection per
flow, like TCP without mux; mux is recommended for UDP and the docs say so.

## 4. Datagram path in the mux session

The session writer gets a third queue next to control frames and stream data:

| Priority | Queue | Bounded by | When full |
|---|---|---|---|
| 1 | control frames (`WINDOW`, `RST`, `PING`, ...) | existing backlog limit | as today |
| 2 | **datagrams** (all UDP flows of the session, FIFO) | `udp_session_buffer` bytes (default 256 KiB) | drop the new packet, count it |
| 3 | stream data (TCP), round robin | per-stream credit | as today |

Datagrams go out before stream data so a game packet does not sit behind a bulk
download sharing the same connection; with coalescing on they still share writes with
whatever else is queued. Received datagrams go into a per-flow queue of at most
`udp_flow_queue` packets (default 128); when the consumer (the UDP socket) falls behind,
the oldest packets are dropped, since for real-time traffic a fresh packet is worth more
than a stale one.

*Status after 3.2:* implemented as described, with two details settled on the way:

- **Fairness.** Strict priority would let a UDP flood starve TCP streams on the same
  session. While streams have data queued, datagrams get at most half of each 64 KiB
  write; without coalescing (the `gaming` profile, one frame per write) datagrams and
  streams take turns.
- **`SYN` order.** Datagrams leave before stream data, but a stream's `SYN` must precede
  its first datagram, and the peer requires `SYN`s in increasing id order. Pending
  `SYN`s therefore have their own queue, sent in id order right after control frames
  and before any datagram or data. (A first version pulled a stream's `SYN` forward
  with its first datagram, which could overtake an older stream's `SYN`; the parallel
  UDP end-to-end tests in 3.3 caught it as sessions closed for "invalid id".)
- Drops are counted per session (`MuxSession::datagram_stats`).
- Tests (over a writer the test holds shut, so frame order is deterministic):
  boundaries and order for 0 to 65,535-byte packets with and without coalescing;
  datagrams leave before stream data queued earlier; floods do not starve streams;
  a full send queue drops new packets; a full receive queue drops the oldest; eight
  windows' worth of datagrams pass while stream data keeps its own credit; datagrams
  for unknown or reset streams are ignored.

API:

```rust
impl MuxStream {
    /// Queues one datagram; false if it was dropped (queue full, stream closed).
    pub fn send_datagram(&self, packet: Bytes) -> bool;
    /// Next datagram, or None when the flow has ended.
    pub async fn recv_datagram(&self) -> io::Result<Option<Bytes>>;
}
```

## 5. Entry side

- One `UdpSocket` per UDP forward rule, `SO_RCVBUF` / `SO_SNDBUF` raised to
  `udp_socket_buffer` (profile default, below) so bursts are not dropped by the kernel.
- One receive task per socket reads packets (64 KiB buffer, enough for any UDP packet)
  and dispatches by source address through the flow table:
  - existing flow: hand the packet to the flow's bounded queue (drop if full);
  - new address: create the flow if the rule has fewer than `udp_max_flows` flows,
    otherwise drop the packet and log once per interval.
- Each flow task opens the channel (mux stream or whole channel), relays packets both
  ways, and sends replies with `send_to(client)` on the shared socket.
- Idle timeout: a flow ends after `udp_timeout` with no packet in either direction; the
  entry then closes the stream (`FIN`) or channel.
- A flow whose open fails (target unresolvable, exit unreachable) is remembered as
  failed for a few seconds, so a client that keeps sending does not open a flow per
  packet.

## 6. Exit side

- On `Open{kind: UDP}`: resolve the target, bind a UDP socket of the matching family
  (`0.0.0.0:0` / `[::]:0`), `connect()` it, then relay: packets from the tunnel go out
  with `send()`, packets from `recv()` go back as datagrams.
- The exit also applies `udp_timeout` (in case the entry vanished without closing the
  stream), and ends the flow when the stream is reset or the session closes.

*Status after 3.3 (sections 5 and 6):* implemented in `src/udp.rs` as described. Notes:

- One `relay` serves both paths (mux stream with `DGRAM` frames, or a whole channel with
  length-prefixed packets) and both sides. Each direction runs as its own loop next to
  an idle watchdog, so no read is ever cancelled halfway through a framed packet.
- Without mux, packets waiting on the local side are gathered into one write (up to
  64 KiB) before flushing.
- The exit keeps a flow when the target answers with ICMP "port unreachable" (the
  service may come back); the entry ignores the same errors on its listening socket.
- The flow table tells flows apart by a generation number, so a flow ending just as
  the same client starts a new one never removes the new one.
- A failed open (exit unreachable, target unresolvable, rejected as unsupported) makes
  the entry ignore that client for 5 s instead of retrying on every packet. With mux the
  open is optimistic, so a rejection arrives as a reset after the fact and is treated
  the same; a reset for "protocol error" on a UDP open (what v0.2 sends) counts as
  "exit side does not support UDP".
- Tests: a UDP echo scenario in every row of the end-to-end matrix (8 clients at once,
  packets from 1 byte to 60 KB, forward rules are `tcp+udp` so every row also covers
  TCP and UDP on one port); idle flows closed and reopened on a new exit socket, with
  and without mux; `udp_max_flows`; UDP through nginx; flow table unit tests (one flow
  per client, failed-open backoff, max flows).
- The parallel end-to-end runs found a bug in 3.2 (a datagram could pull its stream's
  `SYN` ahead of an older stream's, and the peer closed the session over the id order);
  fixed in the mux writer with a regression test, see section 4.
- `KARIZ_TEST_LOG=1 cargo test --test tunnel ...` prints the tunnel sides' debug logs.

## 7. Configuration

```toml
[[forward]]
listen = "0.0.0.0:51820"      # e.g. WireGuard
target = "127.0.0.1:51820"
protocol = "udp"              # tcp (default) | udp | tcp+udp

[tuning]
# udp_timeout_secs = 60       # idle time before a flow is closed
# udp_max_flows = 1024        # per UDP forward rule
```

Profile defaults added to `Tuning` (new fields are optional overrides, as today):

| | balanced | throughput | gaming |
|---|---|---|---|
| `udp_timeout_secs` | 60 | 60 | 60 |
| `udp_max_flows` (per rule) | 1024 | 1024 | 1024 |
| `udp_socket_buffer` | 1 MiB | 4 MiB | 1 MiB |
| `udp_flow_queue` (packets, each direction) | 128 | 512 | 64 |
| `udp_session_buffer` (bytes per mux session) | 256 KiB | 1 MiB | 128 KiB |

Only `udp_timeout_secs` and `udp_max_flows` are exposed in `[tuning]` at first; the
others stay internal until measurements show users need them.

Validation: `protocol` values; UDP forward rules only on the entry side (like TCP);
`udp_timeout_secs` between 5 and 3600; `udp_max_flows` between 1 and 65536.
`kariz check` prints UDP rules and warns when UDP is forwarded without mux ("each UDP
flow uses its own tunnel connection").

## 8. Code layout

```
src/proto.rs              KIND_UDP, datagram framing for non-mux channels
src/mux/session.rs        DGRAM queue and priority, send_datagram / recv_datagram
src/udp/mod.rs            Flow abstraction over a mux stream or a whole channel
src/udp/entry.rs          UDP listener, flow table, idle timeouts, max flows
src/udp/exit.rs           per-flow connected sockets
src/entry.rs / exit.rs    wire UDP forward rules and UDP opens into the existing paths
tests/tunnel.rs           UDP scenarios in the setup matrix
```

The flow abstraction (`send(packet) -> bool`, `recv() -> Option<packet>`) is the seam
phase 4 plugs QUIC datagrams and KCP into.

## 9. Work breakdown

Each step is one PR, keeps CI green, and keeps TCP working in every setup.

| Step | Content | Done when |
|---|---|---|
| **3.0** Plan (done) | This document. | Decisions below settled. |
| **3.1** Config and wire (done) | `KIND_UDP`, `protocol = "udp" \| "tcp+udp"`, UDP tuning fields and validation, non-mux datagram framing in `proto.rs`, "unsupported" status / reset. UDP rules are rejected as "not implemented yet" until 3.3. | Unit tests: open request round trip for both kinds, framing round trip (0-byte and 65,507-byte packets, split reads), config validation. No behaviour change for TCP. |
| **3.2** Mux datagrams (done) | `DGRAM` queue with priority over stream data, drop policies, `send_datagram` / `recv_datagram`, `DATA` on UDP streams rejected. Tested over `duplex`. | Tests: datagrams keep packet boundaries; they overtake queued bulk data; a full queue drops instead of blocking; unknown ids are ignored; credit is untouched. |
| **3.3** UDP end to end (done) | Entry flow table and listener, exit per-flow sockets, both over mux and whole channels, idle timeouts, max flows, failed-open backoff, clean failure against a v0.2 exit. | E2E UDP echo for every setup row (`tcp`, `tcpmux`, `ws`, `wss` × reverse / direct, mux on / off); many concurrent flows; idle flows closed on both sides; unreachable target; `tcp+udp` on one port. |
| **3.4** Hardening and release (done) | Latency test (UDP round trips during a bulk TCP transfer on the same session), packets-per-second and latency benchmarks, memory per flow, sample config (e.g. WireGuard / game server), README / README_FA, CHANGELOG, version `0.3.0`. | UDP p99 round trip under bulk load stays within a few ms of idle on localhost; numbers in the README; release tagged. |

## 10. Performance and resource targets

Measured on localhost like phase 2 (both sides on one machine), with an echo target:

- Latency: an idle tunnel adds under 0.2 ms to a UDP round trip on localhost; with a
  bulk TCP transfer on the same mux session, UDP p99 round trip stays under 5 ms.
- Throughput: at least 100k packets/s of 100-byte packets through `tcpmux` on one core
  pair, and no packet corruption, reordering within a flow, or merging.
- Memory: 1,000 idle UDP flows add under 4 MiB per side.
- A flooded flow (more packets than the tunnel can carry) drops its own packets and
  does not delay other flows or TCP streams by more than its fair share.

*Status at release (3.4):* measured on the 4-core VM of phase 2, both sides on it.

| Target | Result | |
|---|---|---|
| Idle latency added | about 90 µs per round trip (125 µs through the tunnel, 37 µs direct) | met |
| p99 under bulk TCP on the same session | localhost: not measurable (buffers drain in microseconds). Over a throttled 20 Mbit/s link with four bulk transfers: p50 60 ms / p99 100 ms, most of it the emulated link's own buffer | met in spirit; see below |
| 100k packets/s of 100 bytes via `tcpmux` | 143k packets/s each way (140-147k in every setup, 1.4 Gbit/s with 1,400-byte packets) | met |
| 1,000 idle flows under 4 MiB per side | +3.1 to 3.5 MiB | met |
| Floods drop their own packets without starving others | mux unit tests (half-batch limit, turns without coalescing) | met |

Found and fixed on the way:

- **Kernel queue behind the mux.** Datagram priority only reorders what is still in
  Kariz's queue. On a slow link the kernel send buffer held up to a megabyte of bulk
  data (several streams, 256 KiB window each), and UDP waited behind all of it: 340 ms
  round trips at 20 Mbit/s. Mux connections now set `TCP_NOTSENT_LOWAT` to 16 KiB, which
  keeps unsent data in Kariz's queue where the writer's priorities apply. In-flight data
  is not limited, so throughput is not affected (measured). The benchmark
  (`udp_latency_under_load`) puts a throttling proxy with small buffers between the
  sides; the p99 target of 5 ms was written for localhost, where this effect does not
  show at all.
- **Memory per flow.** A first measurement gave 7 to 11 KiB per flow. Three causes:
  - each exit flow had its own 64 KiB receive buffer (now one per worker thread, read
    after readiness);
  - each entry flow task kept the state of the open future, dial and handshake
    included, for its whole life (now boxed and freed once open);
  - a tokio channel per flow reserves room for 32 packets up front (replaced by a
    small queue that allocates on use).

## 11. Risks

| Risk | Mitigation |
|---|---|
| UDP over TCP suffers from loss (retransmit delay, head-of-line blocking). | Documented; datagram priority and drop-instead-of-queue keep the damage bounded; real fix is phase 4. |
| One busy UDP socket per rule becomes a bottleneck at very high packet rates. | Measure in 3.4; `SO_REUSEPORT` with several sockets, `recvmmsg` / GRO are follow-ups (phase 4 needs them anyway). |
| Flow table abuse (spoofed source addresses creating flows). | `udp_max_flows`, flows only live `udp_timeout`, failed-open backoff. |
| Mixing v0.2 and v0.3. | TCP unaffected; UDP opens are reset by v0.2 and logged clearly. |

## 12. Decisions

Settled when the plan was accepted (the owner left the design to the implementation,
asking for the principled choice in each case):

1. **Flow = mux stream + `DGRAM` frames**, not packets inside the stream's byte data.
   The stream gives the flow a lifecycle (open with a target, reset on dial failure,
   close) using machinery that already exists; the separate frame type keeps what makes
   UDP UDP: no flow control, packets can be dropped, and they can overtake bulk data.
2. **UDP without mux is supported**, one tunnel connection per flow with length-prefixed
   packets. Every setup from phase 2 then carries UDP, and "no mux" keeps meaning the
   same thing for TCP and UDP. Mux is recommended, and `kariz check` warns when UDP is
   forwarded without it.
3. **Drop policy**: a new packet is dropped when a session's datagram buffer is full
   (the sender side cannot usefully wait); the oldest packet is dropped when a flow's
   receive queue is full (for real-time traffic a fresh packet is worth more).
4. **`protocol = "tcp+udp"`** forwards both on one port.
5. **Idle timeout 60 s** for all profiles, `tuning.udp_timeout_secs` to change it.
6. Added in 3.1: a dedicated "unsupported" answer (status 2 without mux,
   `RST(unsupported)` with mux) instead of reusing "protocol error", so an entry can tell
   "the exit does not do UDP" from a real fault. v0.2 peers send "protocol error" for the
   same case; the entry treats both as unsupported for UDP opens.
