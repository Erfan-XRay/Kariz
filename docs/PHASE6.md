# Phase 6 plan: gaming profile

Target release: **v0.5.0** (phase 5 was dropped, so the version number does not skip).
Scope: the `gaming` profile done properly, meaning low and steady latency for UDP flows
such as game traffic and voice. Phase 6 in the roadmap also named a `stealth` profile and
an active-probe fallback. Those are not part of this phase.

This document fixes the design and the order of work, so each step can land as its own
commit with CI green. As in phase 4, the decisions are settled here with their
reasoning (section 12).

## 1. Why, and what for

Game traffic is small packets (50-300 bytes) at a steady rate (20-128 per second, both
ways). What players notice is the worst round trips (p99 and above) and bursts of loss,
not throughput. Phase 4 measured where Kariz stands:

- **`quic`**: UDP flows travel as QUIC datagrams. A lost packet delays nothing, and p99
  stays at the link RTT (64 ms at a 60 ms RTT with 1 % loss). The lost packet is gone,
  though (about 2 % per round trip at 1 % loss).
- **`kcp`**: UDP flows travel as kmux `DGRAM` frames *inside the reliable KCP stream*.
  Nothing is lost, but a lost segment holds everything behind it for a retransmission:
  idle p99 142 ms at 1 % loss and 262 ms at 5 %. FEC 10 / 3 brings those down to 65 and
  142 ms. KCP is the transport that keeps its rate on lossy paths, so it is the one
  games most often end up on.
- **`tcp` / `tcpmux` / `ws` / `wss`**: head-of-line blocking is inherent (phase 3). The
  gaming profile cannot fix that; the docs point games to `kcp` or `quic`.
- **The `gaming` profile today** only shrinks buffers and windows, turns off write
  coalescing and sets a 10 s keep-alive.

Goals:

1. **An unreliable datagram path over KCP.** UDP packets skip KCP's ARQ, travel beside
   it in the same packet layer (same socket, protection and FEC), and are never held
   behind a lost segment.
2. **Packet duplication**, per forward rule: each UDP packet is sent 2 or 3 times, a
   few milliseconds apart, and the receiver drops the copies. Random and short-burst loss
   mostly disappears, at a bandwidth cost only chosen flows pay.
3. **DSCP marking** of tunnel sockets (and the exit's sockets to the target), for
   networks that honour it (the operator's own links, the host's qdisc).
4. **Measured defaults for `gaming`**: a game-traffic pattern in the lossy-link
   benchmark, and the profile's KCP mode, window, FEC and duplication chosen from it.

Non-goals: fixing head-of-line blocking on TCP-based transports; latency-based path
selection across several servers; changing the other profiles' defaults (the datagram
path is the exception; see decision 4).

## 2. The datagram path over KCP

```
 UDP flow (src/udp.rs)                 unchanged: send_datagram / recv_datagram
 [ kmux ]                              DGRAM frame: fits the path?  yes ──┐   no
   │ stream data, control, large DGRAMs                                   │   │
 [ records ]                                                              │   │
 [ KcpStream (ARQ) ]                   [ datagram sealing (6.1) ] <───────┘   │
   │                                     │                                    │
 [ KCP packet layer: type | body ] <─────┘   (large ones stay in the stream) <┘
 [ FEC ] [ packet protection ] [ UDP socket ]
```

- **A new packet type** in the KCP packet layer, beside KCP segments, pings and FEC
  shards: `PACKET_DATAGRAM: conv (4) | sealed datagram`. The packet goes through FEC
  like any other when FEC is on, so a lost game packet can be rebuilt from parity
  without a retransmission. v0.4 ignores unknown packet types (checked in `on_packet`),
  so a v0.4 peer that receives one drops it.
- **kmux places `DGRAM` frames.** When the link under a kmux session has a datagram path,
  a `DGRAM` frame that fits goes there; one that does not fit goes in the stream as today.
  This is the same datagram-or-stream rule QUIC flows follow (phase 4, decision 7).
  UDP forwarding (`src/udp.rs`) and the session layer do not change: flows still call
  `send_datagram` / `recv_datagram`.
- **Room per packet:** KCP MTU 1350 less protection (29), the type and conversation id,
  the mux frame header (7) and the datagram seal (section 3) leaves about 1,280 bytes. Game
  packets are far below that. WireGuard packets (up to about 1,420 bytes) take the stream
  unless the tunnel MTU is lowered, and the docs say so, as they already do for QUIC.
- **Queueing:** datagrams go out ahead of KCP's pending segments, but never more than
  half the packets of one driver pass while segments wait, so bulk TCP still makes
  progress. This is the same rule as kmux `DGRAM` frames in phase 3. They skip KCP's send
  window and are dropped, not queued, when the socket buffer is full.
- **Negotiation, in band.** The handshake reply has no flags field, so the path is
  switched on inside the session instead. Each side sends a datagram probe when the kmux
  session starts, and uses the path only after it has received the peer's probe or
  any datagram from it. Until then, and forever with a v0.4 peer, `DGRAM` frames stay in
  the stream. A v0.5 pair therefore needs no configuration. The exact probe (a kmux frame
  or a packet type) is settled in 6.2.
- **Without mux** (`[tunnel.mux] enabled = false`), a UDP flow is a whole KCP
  connection carrying length-prefixed packets, and it stays reliable. The `gaming`
  profile keeps mux on; `kariz check` warns about UDP over KCP without mux, as it
  already warns about UDP without mux.

## 3. Sealing datagrams

Packet protection (the token key) hides KCP's headers but gives no forward secrecy.
Everything a user sends has so far also gone through the record layer, whose keys come
from the X25519 handshake. Datagrams keep that property:

- **Keys:** one per direction, derived from the session's record keys with a new context
  (`BLAKE3-derive-key("kariz v2 dgram c2s" / "... s2c", record key)`), with the cipher
  chosen for the records (ChaCha20-Poly1305 or AES-256-GCM).
- **Nonce:** an explicit 8-byte packet number per direction, sent in the clear inside
  the packet protection (so it is not visible on the wire), counting up from 0.
- **Replay and duplicate window:** the receiver keeps the highest number seen plus a
  1,024-bit window behind it. A number seen before, or older than the window, is
  dropped. The same window removes the copies that duplication sends (section 4).
- **Overhead:** 8 + 16 bytes per datagram.
- `encryption = "none"` sends datagrams unsealed with the packet number only. They are
  still inside the packet protection, as KCP segments are.

*Status after 6.1:* `src/crypto/datagram.rs` holds `DatagramSealer` / `DatagramOpener`
and the receive window; nothing uses them yet. Where the result differs from the plan
above, and why:

- **One context, `kariz v2 dgram`**, not one per direction. The record keys already
  differ per direction, so a second label would add nothing.
- **Copies are the same sealed bytes sent again**, so they share one packet number and
  the window drops them without a separate duplicate filter. The sealer has no call to
  seal with a chosen number, so a number is never used twice for different contents.
- **The window only moves after authentication**, so a forged packet with a huge number
  cannot make later genuine packets look old.

Tests: round trip for both ciphers and `none`, sizes 0 to 65,535; every changed byte
rejected; truncated datagrams, another key and a record sealed with the record key
itself rejected; replays and copies dropped; a whole window arriving newest first;
late packets just inside and just outside the window; skipped numbers that share a bit
with older ones; a jump past the window; a forged number that leaves the window alone.

## 4. Packet duplication

- **Per forward rule:** `[[forward]] duplicate = 1 | 2 | 3` (UDP rules only, default 1)
  and `duplicate_gap_ms` (0-50, default 5). Each packet of the rule's flows is sent
  `duplicate` times in total. The copies go `duplicate_gap_ms` apart, so a short burst
  of loss does not take them all.
- **Both directions:** the entry duplicates packets towards the exit, and the exit
  duplicates the flow's return packets. The setting travels in the flow's open request.
  A v0.4 exit does not know the field; the open-request layout is chosen in 6.3 so that
  such an exit rejects the flow cleanly or ignores the field.
- **Over KCP's datagram path:** each copy is its own datagram with the same packet
  number, and the receive window keeps the first.
- **Over QUIC datagrams:** QUIC datagrams have no packet number of ours, so with
  duplication on, the flow's datagrams carry a varint sequence number after the stream
  id, and the receiver uses the same window. Flows without duplication keep today's
  format.
- **On stream transports and on KCP's stream fallback**, duplication is ignored: the
  stream already delivers every packet. `kariz check` says so.
- **Cost:** `duplicate` times the flow's bandwidth. For 64 packets a second of 200
  bytes, 2 copies add about 100 kbit/s, nothing on any link games run on. For
  WireGuard or bulk UDP it would be real, which is why duplication is set per rule.

## 5. DSCP

- `[tuning] dscp = "ef" | "af41" | "cs4" | ... | <0-63>` (default: unset, the socket
  keeps the OS default of 0). It is set on tunnel sockets (TCP, KCP and QUIC UDP
  sockets) with `IP_TOS` / `IPV6_TCLASS`, and on the exit's UDP sockets to targets.
  `socket2` has both calls, so no new dependency.
- **Per socket, not per packet.** A KCP socket carries both the datagrams and the bulk
  streams, so they all get the mark. Marking per packet (`sendmsg` with a control
  message) is possible on Linux, but only worth it if the benchmark shows the mark
  matters on a real path.
- **Off by default in every profile, `gaming` included.** On the public internet DSCP
  is mostly cleared or ignored. Some networks treat marked traffic worse, and an unusual
  mark makes a flow stand out. It helps on links the operator controls and on the
  host's own egress qdisc (`fq`, `cake`). The README says this plainly.
- Windows ignores `IP_TOS` without a QoS policy. The option is accepted there, and
  `kariz check` notes that it has no effect.

## 6. Configuration

```toml
profile = "gaming"

[tunnel]
transport = "kcp"                 # kcp or quic for games; TCP-based transports have
                                  # head-of-line blocking whatever the profile

[tuning]
# dscp = "ef"                     # optional, off by default

[[forward]]
protocol = "udp"
listen = "0.0.0.0:27015"
target = "10.0.0.2:27015"
duplicate = 2                     # 1 (default), 2 or 3 copies of each packet
duplicate_gap_ms = 5              # between copies
```

The gaming profile's KCP defaults (mode, window, FEC) and whether it turns duplication
on by itself are decided in 6.5 from the benchmark, and set as profile values that
`[tunnel.kcp]` and `[[forward]]` override. Validation: `duplicate` 1-3 and
`duplicate_gap_ms` 0-50 on UDP rules only; `dscp` a known name or 0-63.

## 7. Code layout

```
src/crypto/datagram.rs       datagram keys, sealing, packet numbers, receive window (6.1)
src/transport/kcp/mod.rs     PACKET_DATAGRAM, the driver's datagram queue, KcpStream's
                             datagram half (6.2)
src/mux/session.rs           DGRAM placement: datagram path or stream, probe (6.2)
src/session/quic.rs          sequence numbers when duplicating (6.3)
src/udp.rs                   duplication on send (6.3)
src/transport/mod.rs, tcp.rs, kcp, quic   DSCP on sockets (6.4)
tests/tunnel.rs              game-traffic scenario and benchmark (6.5)
```

## 8. Tests and benchmark

**Correctness** (every step, every OS, over local sockets and the link emulator of
phase 4):

- Datagram sealing: round trip, every changed byte rejected, replays and copies dropped,
  numbers older than the window dropped, both ciphers.
- Datagram path: over a link that drops one packet in ten, a stream of UDP packets
  arrives in order of sending apart from the lost ones, and none waits for a
  retransmission. A packet above the limit still arrives (stream fallback). A v0.4-style
  peer (probe disabled) gets everything through the stream.
- Duplication: with `duplicate = 2`, each packet is delivered exactly once; loss at 5 %
  random falls to about 0.25 % at the receiver; copies are really `duplicate_gap_ms`
  apart. Over QUIC as well.
- DSCP: the option is set on each socket kind, read back with `getsockopt`.
- E2E: rows `kcp_reverse` / `kcp_direct` with the gaming profile; UDP echo with packets
  from 1 byte to 60 KB (the large ones on the stream).

**Benchmark** (6.5): the lossy-link harness (60 ms RTT, 50 Mbit/s) gets a
**game-traffic pattern**: 128-byte packets at 64 Hz both ways for 30 s, reporting
round-trip p50 / p99 / p99.9, delivered share, and the longest run of lost packets. It
runs on an idle tunnel and during a download on the same tunnel, at 0 / 1 / 5 % random
loss and with bursty loss (mean burst 3). Rows: `tcpmux` (baseline), `kcp` as in v0.4,
`kcp` + datagram path, + FEC, + duplication 2; `quic` ± duplication 2.

## 9. Work breakdown

| Step | Content | Done when |
|---|---|---|
| **6.0** Plan (done) | This document; roadmap updated. | Decisions settled (section 12). |
| **6.1** Datagram sealing (done) | `src/crypto/datagram.rs`: keys from the record keys, explicit packet numbers, receive window. No transport changes yet. | Unit tests of section 8 pass; no other behaviour change. |
| **6.2** KCP datagram path | `PACKET_DATAGRAM`, driver queue and fairness, `KcpStream` datagram half, kmux placement and in-band probe, `kariz check` warning without mux. | Loss on the link does not delay other UDP packets; large ones fall back; without the probe everything stays in the stream; every existing test passes. |
| **6.3** Duplication | `duplicate` / `duplicate_gap_ms` on UDP rules, both directions; dedup over KCP datagrams and QUIC datagrams (sequence numbers there). | Exactly-once delivery with copies; residual loss about the square of the link's under random loss; config validation. |
| **6.4** DSCP | `tuning.dscp` on tunnel sockets and the exit's UDP target sockets, v4 and v6. | Option read back on every socket kind; `kariz check` shows it. |
| **6.5** Benchmark and defaults | Game-traffic pattern and matrix; the gaming profile's KCP mode, window, FEC and duplication decided from it. | Table in this document; targets of section 10 checked. |
| **6.6** Release | Sample configs (`entry-gaming.toml` / `exit-gaming.toml`), README / README_FA (gaming section, DSCP caveats), CHANGELOG, version `0.5.0`, roadmap. | Numbers in the README; release tagged. |

## 10. Targets

On the emulated link (60 ms RTT, 50 Mbit/s), game-traffic pattern:

- **Idle tunnel, 1 % random loss:** over `kcp` with the datagram path, round-trip p99 at
  most 1.1x the link RTT (v0.4: 142 ms, 2.4x). With duplication 2, the delivered share is
  at least 99.9 %.
- **5 % random loss, duplication 2:** delivered share at least 99.5 %, p99 at most 1.2x
  the link RTT.
- **During a download on the same tunnel, gaming profile:** p99 at most 2x the link RTT
  (v0.4 `kcp`: 285 ms, about 4.7x). This depends on the window, since KCP without
  congestion control fills the bottleneck queue (phase 4, 4.6); 6.5 sizes the gaming
  profile's window for it.
- **No regression:** bulk throughput of `kcp` and `quic` within 5 % of v0.4 in the
  phase 4 benchmark; v0.5 with v0.4 still works over every transport.

## 11. Risks

| Risk | Mitigation |
|---|---|
| The bottleneck queue KCP fills adds latency no datagram path can skip. | The gaming profile sizes KCP's window for the path (6.5); the README explains the trade-off with throughput. |
| Duplication wastes bandwidth if set on the wrong rule (WireGuard, bulk UDP). | Per rule, off by default; `kariz check` shows the cost per rule. |
| DSCP marks are cleared, ignored or treated worse on the path. | Off by default; the docs say where it helps. |
| An unreliable path changes what v0.4 users relied on (UDP over KCP never lost packets). | That is UDP's normal behaviour and what applications expect. The change is in the CHANGELOG; the stream path stays for packets that do not fit. |
| Compatibility with v0.4 peers. | In-band probe; v0.4 ignores the new packet type; tested with the probe disabled. |

## 12. Decisions

1. **The datagram path lives in the KCP packet layer**, beside KCP segments, not on a
   second socket or conversation. One socket keeps one NAT mapping and one port, the
   packet protection and FEC cover game packets too, and the conversation's liveness
   and close already apply.
2. **Datagrams are sealed with keys from the handshake**, not only by packet protection.
   Everything a user sends keeps forward secrecy, as it has since phase 2. The explicit
   packet number costs 8 bytes and doubles as the duplicate filter.
3. **kmux decides placement** (datagram path if it fits, stream otherwise), so the UDP
   forwarding code and the session layer do not change, and large packets are never
   silently dropped. This is the same rule as QUIC's in phase 4.
4. **The datagram path is on for every profile over KCP**, not only for `gaming`. An
   unreliable path is what UDP is. The reliable-in-stream behaviour of v0.4 was a
   limitation, noted in phase 4 as deferred to this phase.
5. **In-band negotiation** instead of a handshake change. The handshake reply has no
   flags, and changing it would break v0.4 peers on every transport, not only on KCP.
6. **Duplication per forward rule**, because its cost is proportional to the flow's
   bandwidth and only some flows deserve it.
7. **DSCP per socket and off by default.** It is honest about how rarely the mark
   survives the internet. Per-packet marking waits for evidence.
8. **Stealth and active-probe fallback are out of this phase.** The roadmap lists them
   as later work, not scheduled.
