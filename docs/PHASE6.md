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

*Status after 6.2:* over `kcp` with mux, UDP flows take the datagram path. The code is in
`src/transport/kcp/mod.rs` (`PACKET_DATAGRAM`, `KcpDatagrams`), `src/channel.rs`
(`DatagramPath`: sealing with the 6.1 keys, carried by a KCP `Link` that has mux) and
`src/mux/session.rs` (placement, probes). Where the result differs from the plan above,
and why:

- **Datagrams are sent at once from `send_datagram`**, not queued for the driver or the
  mux writer. There is then no queue to share fairly with KCP segments: a datagram
  goes straight to the socket, and when the socket buffer is full it is dropped, like
  any UDP packet on a busy link. This path has the lowest latency, and it needs no lock
  beyond the sender's.
- **In an FEC data shard a datagram is marked by `!conv`** (the conversation id with
  every bit flipped) in front of it, where KCP packets start with `conv`. So the FEC
  format and the decoder are unchanged, and a rebuilt shard says what it held.
- **Probes skip FEC.** A v0.4 peer ignores the new packet type, but it would feed a
  datagram in an FEC shard to KCP. Probes are therefore always sent as plain
  `PACKET_DATAGRAM`. Real datagrams, which only go to a peer that answered, use FEC when
  it is on.
- **Probes are answered** (a probe and an answer are `DGRAM` frames for stream 0 with
  payload 0 and 1). A side whose probe was lost keeps probing at every keep-alive until
  it hears the peer, and the answer tells it the path works even when the peer has no
  datagrams to send.
- **A stream's datagrams take the path only once the peer knows the stream**: the peer
  opened it, or sent any frame on it. So the first packets of a flow the entry opens go
  in the connection, behind the `SYN`, until the exit's first reply.
- **The mux writer keeps its datagram queue** for what does not take the path: packets
  that are too large, streams the peer does not know yet, peers without the path, and
  every transport other than `kcp`.
- `kariz check` already warned about UDP forwarded without mux (phase 3), which covers
  KCP.

Tests: over local KCP sockets, datagrams in both directions from 0 bytes to the limit,
with and without FEC, beside a stream; through a link dropping 20 %, about a fifth of
200 datagrams are missing (not resent) while a stream beside them arrives whole; with
FEC 4 / 2 over a link dropping 10 %, at least 385 of 400 arrive, some rebuilt. Mux over
KCP: after the probes, datagrams in both directions take the path, and a 3,000-byte one
still arrives through the connection; the opener's first datagram goes in the
connection until the peer knows the stream; a peer that ignores the path (as v0.4 does)
gets everything in the connection. The E2E `kcp` rows, whose UDP echoes now take the
path, pass unchanged.

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

*Status after 6.3:* `[[forward]] duplicate` / `duplicate_gap_ms` work over `kcp` (with
mux) and `quic`, in both modes. The code is in `src/udp.rs` (numbering, copies, dropping
copies), `src/proto.rs` (open request) and `src/config.rs`. Where the result differs from
the plan above, and why:

- **Copies are dropped by the UDP flow, not by each transport.** A flow with duplication
  puts a sequence number in front of every packet, `seq (4, BE) | packet`, in both
  directions, and the receiving side keeps the 6.1 window over those numbers. Each copy
  is then an ordinary datagram (over KCP sealed on its own, with its own packet
  number). This one mechanism covers KCP's datagram path and QUIC datagrams alike, so
  QUIC's datagram format and kmux are unchanged. It costs 4 bytes per packet. The
  numbers are 32 bits on the wire and go on past 2^32 (each is read as the 64-bit number
  nearest the highest seen, as QUIC does with packet numbers).
- **Copies only where a packet may be lost.** The flow asks its stream whether a packet
  of that size would go unreliably (`SessionStream::sends_unreliably`: on KCP's datagram
  path, or as a QUIC datagram). Packets that go in the connection or on QUIC's stream
  fallback are numbered but sent once. Without mux, a flow is a whole channel and is
  numbered the same way, so both sides agree on the format, but nothing is copied.
- **The open request has a new kind**, 3 (`kind | target_len | target | copies | gap_ms`),
  decoded as a UDP open with duplication. A v0.4 exit rejects kind 3 as unknown, which
  the entry reports as an exit that does not support the flow (older version), and
  the client is backed off for 5 s, as with any rejected flow. Flows without
  duplication still use kind 2, so they work with v0.4.
- **Copies wait in two queues** (second copies and third copies), each in sending
  order, served by the flow's sending loop when due. There is no task or timer per
  packet. With `duplicate_gap_ms = 0` copies go out at once.
- **`kariz check`** prints each rule's copies and gap. It warns when duplication is set
  on a transport where nothing is copied.

Tests: numbering and dropping copies (out of order, each packet twice, too short),
sequence numbers across the 32-bit wrap; open requests of kind 3 (round trip, missing
or out-of-range options rejected); config (UDP rules only, bounds, the warning). E2E
`duplication_hides_random_loss`: 100-byte packets every 10 ms, echoed, over a link
dropping 10 % each way (about 19 % of round trips lost without duplication; 21-23 %
measured). With 2 copies 5 ms apart, over `kcp` direct and reverse and `quic` direct,
0.5-3.5 % were lost in eight runs (the square law predicts about 2 %).

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

*Status after 6.4:* `[tuning] dscp` marks TCP tunnel sockets (both ends, so `tcp`,
`tcpmux`, `ws`, `wss`), KCP sockets (dialer and listener) and the exit's UDP sockets to
targets. The code is `transport::mark_dscp`, called where each of those sockets is
set up. Names follow RFC 2474, 2597, 3246, 5865 and 8622 (`cs0`-`cs7`, `af11`-`af43`,
`ef`, `va`, `le`, `default`), case-insensitive, or a number 0-63. Where the result
differs from the plan above, and why:

- **QUIC sockets are not marked.** On Linux, quinn-udp sends every packet with an
  `IP_TOS` / `IPV6_TCLASS` control message that holds the ECN bits alone (0 when ECN is
  off). That per-packet value overrides the socket's, so a mark set on the socket would
  never reach the wire. Rather than set an option that does nothing, Kariz leaves QUIC
  sockets alone. `kariz check` warns that the tunnel's own packets are not marked over
  QUIC and that the exit's sockets to targets still are. Marking QUIC would take a
  socket wrapper that writes DSCP and ECN into that control message together, and
  that is only worth building if 6.5 shows marks matter.
- **IPv6 only on Unix.** `IPV6_TCLASS` is set on Unix, where a dual-stack socket also
  gets `IP_TOS` for its IPv4 peers. socket2 has no traffic-class call on Windows, where
  marks need a QoS policy anyway.
- A socket that refuses the option loses only the mark: it is logged at debug level,
  never an error.

Tests: the mark read back from plain UDP sockets, TCP tunnel sockets (dialed and
accepted), KCP sockets and the exit's target sockets, over IPv4 everywhere and IPv6
on Unix, with the mark set and unset; config names, numbers, case, bad values, and the
QUIC warning. The Windows development machine reads the IPv4 mark back as set (it is
the QoS policy, not the option, that decides whether it goes out). Nobody has checked
yet whether marks survive a real path: that is for 6.5, and for the operator's own
network.

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

*Decided in 6.5* (section 10): the gaming profile turns FEC 10 / 3 on over `kcp` unless
`[tunnel.kcp]` sets `fec_data` / `fec_parity`; mode, window, duplication and DSCP keep
their general defaults. `[tunnel.kcp] datagrams = false` keeps UDP flows in the
reliable stream, as in v0.4.

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

*Status after 6.5:* `cargo test --release --test tunnel game_traffic -- --ignored
--nocapture` (options in its doc comment: rows, one loss rate, length, seed, and a
detail line saying where the losses and the slow packets were). Every row uses the
gaming profile. The download is four parallel downloads on the same tunnel, since one
alone is capped by the profile's 64 KiB stream window. Bursty loss is 2 % with a mean
burst of 3 packets. To show v0.4 behaviour on the same code, `[tunnel.kcp] datagrams =
false` (new, default `true`) keeps UDP flows in the reliable stream. Release build on
the Windows 11 development machine, 30 s per measurement (1,920 packets), round trips
p50 / p99 / p99.9 in ms, share of packets that came back:

| Loss | Transport | Idle tunnel | During 4 downloads | Downloads |
|---|---|---|---|---|
| 1 % | `tcpmux` | 62 / 169 / 171, 100 % | 395 / 734 / 837, 100 % | 4.6 Mbit/s |
| 1 % | `kcp`, datagrams off (v0.4) | 63 / 141 / 156, 100 % | 86 / 219 / 255, 100 % | 18.0 Mbit/s |
| 1 % | `kcp`, FEC off | 63 / **64** / 65, 98.18 % | 63 / 88 / 95, 97.50 % | 17.2 Mbit/s |
| 1 % | `kcp`, FEC 10 / 3 | 63 / 69 / 84, **100 %** | 66 / 82 / 90, 100 % | **29.2 Mbit/s** |
| 1 % | `kcp`, FEC off, 2 copies | 63 / 68 / 70, 100 % | 63 / 84 / 93, 100 % | 17.7 Mbit/s |
| 1 % | `kcp`, FEC 10 / 3, 2 copies | 63 / 68 / 70, 100 % | 66 / 81 / 89, 100 % | 28.9 Mbit/s |
| 1 % | `quic` | 63 / 64 / 64, 98.23 % | 77 / 113 / 121, 97.50 % | 4.2 Mbit/s |
| 1 % | `quic`, 2 copies | 62 / 68 / 69, 100 % | 76 / 112 / 126, 99.22 % | 4.0 Mbit/s |
| 5 % | `tcpmux` | 77 / 242 / 290, 100 % | 1,112 / 1,616 / 1,676, 100 % | 1.9 Mbit/s |
| 5 % | `kcp`, datagrams off (v0.4) | 63 / 236 / 265, 100 % | 144 / 347 / 424, 100 % | 11.5 Mbit/s |
| 5 % | `kcp`, FEC off | 63 / **64** / 66, 90.21 % | 63 / 83 / 112, 89.53 % | 11.3 Mbit/s |
| 5 % | `kcp`, FEC 10 / 3 | 63 / 84 / 89, **98.96 %** | 68 / 94 / 110, 99.43 % | **26.5 Mbit/s** |
| 5 % | `kcp`, FEC off, 2 copies | 63 / 70 / 193, 99.22 % | 63 / 84 / 94, 99.48 % | 11.1 Mbit/s |
| 5 % | `kcp`, FEC 10 / 3, 2 copies | 63 / 70 / 84, 99.53 % | 67 / 91 / 99, 100 % | 26.5 Mbit/s |
| 5 % | `quic` | 63 / 64 / 65, 90.00 % | 87 / 154 / 184, 87.81 % | 1.8 Mbit/s |
| 5 % | `quic`, 2 copies | 63 / 78 / 85, 99.22 % | 91 / 154 / 183, 94.43 % | 1.7 Mbit/s |
| 2 % bursts | `tcpmux` | 62 / 202 / 244, 100 % | 352 / 713 / 770, 100 % | 5.5 Mbit/s |
| 2 % bursts | `kcp`, datagrams off (v0.4) | 63 / 219 / 328, 100 % | 81 / 251 / 328, 100 % | 18.8 Mbit/s |
| 2 % bursts | `kcp`, FEC off | 62 / 64 / 64, 95.57 % | 63 / 86 / 95, 95.62 % | 18.8 Mbit/s |
| 2 % bursts | `kcp`, FEC 10 / 3 | 63 / 65 / 85, 96.88 % | 66 / 98 / 108, 98.07 % | 23.2 Mbit/s |
| 2 % bursts | `kcp`, FEC off, 2 copies | 63 / 68 / 70, 97.55 % | 63 / 87 / 91, 98.07 % | 19.0 Mbit/s |
| 2 % bursts | `kcp`, FEC 10 / 3, 2 copies | 63 / 68 / 90, 98.12 % | 66 / 96 / 107, 98.96 % | 23.1 Mbit/s |
| 2 % bursts | `quic` | 62 / 116 / 213, 95.42 % | 78 / 119 / 125, 95.99 % | 4.6 Mbit/s |
| 2 % bursts | `quic`, 2 copies | 63 / 941 / 1,035, 96.51 % | 77 / 118 / 127, 96.93 % | 4.2 Mbit/s |

Without loss every row is alike: p99 64-65 ms idle and 67-73 ms during the downloads,
which reach 30-32 Mbit/s. A KCP window of 256 and the `fast3` preset changed nothing
(within a millisecond or a packet of the `kcp`, FEC off rows) and are left out. What
the table says:

- **The datagram path does what it is for.** On `kcp`, p99 on an idle tunnel stays at
  the link RTT whatever the random loss (64 ms), where v0.4 waited for retransmissions
  (141 ms at 1 %, 236 ms at 5 %). The price is UDP's: a lost packet is lost (98.2 %
  and 90.2 % of round trips come back, two legs each).
- **FEC 10 / 3 is what the gaming profile wants over `kcp`.** It brings back nearly
  every lost packet (100 % at 1 %, 99 % at 5 %) for at most one KCP interval of delay
  on the rebuilt ones (p99 69 and 84 ms). It also rebuilds the streams' lost
  segments, so downloads next to the game go 1.7x and 2.3x faster under loss (17 to 29,
  11 to 26 Mbit/s). Without loss it costs 30 % more packets and about 1 Mbit/s. **The
  gaming profile now turns it on over `kcp`** (below).
- **Duplication is the lever for QUIC**, which has no FEC: 98.2 to 100 % at 1 %, 90 to
  99.2 % at 5 %. Over `kcp` it adds little to FEC (99.53 against 98.96 % at 5 %). With
  bursty loss, copies 5 ms apart often fall into the same burst, because on an idle flow
  the copy is the next packet on the link, and the emulator draws bursts per packet.
- **During the downloads the gaming profile keeps p99 under 2x the RTT on `kcp`**
  (81-98 ms at any loss). Its 64 KiB stream window caps each download at about 64 KiB
  per round trip, so four of them (about 31 Mbit/s) do not fill the 50 Mbit/s link, and
  the bottleneck queue that KCP without congestion control fills in phase 4 stays
  short. The KCP window does not matter at that point, so `fast2` and 1024 stay.
- **QUIC is the weaker choice for games.** quinn sends datagrams under the same
  congestion control as the streams, and Cubic collapses under loss (downloads of
  1.7-4.6 Mbit/s): during the downloads the game's p50 rises to 77-91 ms and its p99 to
  112-154 ms. After a long loss burst, quinn holds datagrams while it recovers, and the
  ones queued meanwhile arrive late: with detail on, 37 packets sent in the 0.6 s after a
  15-packet burst came back 150-560 ms late (seed 33). In the table, with twice the
  traffic (2 copies), that reached a p99 of 941 ms. Other seeds gave 70-79 ms, so it
  depends on where the bursts fall. The datagram send buffer (`session_buffer`, 128 KiB
  in the gaming profile) holds seconds of game traffic, so stale datagrams are sent
  late rather than dropped. A smaller buffer for QUIC under the gaming profile is a
  lever for later; the docs recommend `kcp` for games.
- **The long runs under bursty loss are the loss model.** The emulator counts bursts in
  packets, and on an idle tunnel the game is almost the only traffic, so one long burst
  (geometric, mean 3) takes many game packets in a row. With `kcp` and FEC off, the
  longest runs were 23, 13, 7 and 14 packets under four seeds, and only 3-8 during the
  downloads, when the same bursts land mostly on download packets.

## 9. Work breakdown

| Step | Content | Done when |
|---|---|---|
| **6.0** Plan (done) | This document; roadmap updated. | Decisions settled (section 12). |
| **6.1** Datagram sealing (done) | `src/crypto/datagram.rs`: keys from the record keys, explicit packet numbers, receive window. No transport changes yet. | Unit tests of section 8 pass; no other behaviour change. |
| **6.2** KCP datagram path (done) | `PACKET_DATAGRAM`, driver queue and fairness, `KcpStream` datagram half, kmux placement and in-band probe, `kariz check` warning without mux. | Loss on the link does not delay other UDP packets; large ones fall back; without the probe everything stays in the stream; every existing test passes. |
| **6.3** Duplication (done) | `duplicate` / `duplicate_gap_ms` on UDP rules, both directions; dedup over KCP datagrams and QUIC datagrams (sequence numbers there). | Exactly-once delivery with copies; residual loss about the square of the link's under random loss; config validation. |
| **6.4** DSCP (done) | `tuning.dscp` on tunnel sockets and the exit's UDP target sockets, v4 and v6. | Option read back on every socket kind; `kariz check` shows it. |
| **6.5** Benchmark and defaults (done) | Game-traffic pattern and matrix; the gaming profile's KCP mode, window, FEC and duplication decided from it. | Table in this document; targets of section 10 checked. |
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

*Results (6.5),* game-traffic numbers from section 8, same machine:

| Target | Result | |
|---|---|---|
| Idle, 1 % loss: `kcp` p99 at most 1.1x the RTT (66 ms) | 64 ms (v0.4: 141 ms) | met |
| Idle, 1 % loss, 2 copies: at least 99.9 % delivered | 100 % | met |
| 5 % loss, 2 copies: at least 99.5 % delivered, p99 at most 72 ms | FEC off: 99.22 %, 70 ms; with the gaming profile's FEC 10 / 3: 99.53 %, 70 ms | met with FEC; missed by copies alone |
| During downloads, gaming profile: p99 at most 2x the RTT (120 ms) | `kcp` 81-98 ms at every loss rate (v0.4: 219-347 ms) | met |
| No regression in the phase 4 benchmark | within 5 % in every cell (below) | met |
| v0.5 with v0.4 | datagram path negotiated in band (6.2), new open kind only with duplication (6.3); tested with a peer that ignores the path | met |

The regression check ran the phase 4 `lossy_link` benchmark (balanced profile, 4 MiB
stream window) on this branch and on `main` (v0.4) in turn, twice each, on the rows
`kcp fast2` (without FEC, with 10 / 3, window 256) and `quic cubic`. Download rates in
Mbit/s at 0 / 1 / 5 % loss:

| Row | v0.4, runs 1 and 2 | v0.5, run 2 | Change against the v0.4 mean |
|---|---|---|---|
| `kcp fast2` | 33.3, 32.6 / 28.0, 29.0 / 25.4, 23.5 | 32.6 / 27.5 / 23.4 | -1 / -4 / -4 % |
| `kcp fast2`, FEC 10 / 3 | 23.3, 24.8 / 25.4, 25.1 / 23.6, 23.6 | 24.3 / 25.7 / 23.8 | +1 / +2 / +1 % |
| `kcp fast2`, window 256 | 29.8, 28.7 / 17.2, 16.7 / 12.8, 12.6 | 29.1 / 16.4 / 12.4 | -1 / -3 / -2 % |
| `quic cubic` | 47.9, 47.9 / 2.3, 2.8 / 0.9, 0.9 | 47.9 / 2.6 / 0.9 | 0 / +2 / 0 % |

The first v0.5 run is left out. It ran right after both release builds and was off in
every row, `quic` included, whose code this phase does not change (idle p99 93 ms instead
of 64, downloads up to 26 % lower). The second run, on a quiet machine, matched v0.4.

The same benchmark shows the other side of the datagram path. With the balanced
profile's 4 MiB window, a download over `kcp` (no congestion control) keeps the
bottleneck queue overflowing, as in phase 4. The UDP pings next to it used to wait in
the stream: none lost, p99 204-343 ms. Now they are dropped at that queue: 4.8-12.8 %
lost (2.2-5.6 % with FEC), p99 112-120 ms. This is UDP's normal behaviour on a full link. The
gaming profile avoids the full queue altogether (its downloads stay under the link
rate); for the other profiles `[tunnel.kcp] datagrams = false` restores the v0.4
behaviour.

Defaults decided from these numbers:

- **The gaming profile turns FEC 10 / 3 on over `kcp`** unless `[tunnel.kcp]` sets
  `fec_data` or `fec_parity` (0 turns it off), or sets an MTU too large for FEC.
  `fec_data` / `fec_parity` became optional for that; `Config::kcp` fills them in. The
  other profiles keep FEC off (phase 4: without loss it costs throughput when windows
  are large).
- **KCP mode and window unchanged** (`fast2`, 1024): with the gaming profile's stream
  window, neither a window of 256 nor `fast3` changed anything measurable.
- **Duplication stays opt-in per rule**, since it doubles a rule's traffic. The README
  recommends `duplicate = 2` for game rules over `quic`, and over `kcp` when FEC is off.
- **DSCP stays off.**
- **`[tunnel.kcp] datagrams`** (new, default `true`) keeps the v0.4 behaviour available.

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
