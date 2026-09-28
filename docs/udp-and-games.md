# UDP forwarding and games

## UDP rules

```toml
[[forward]]
listen = "0.0.0.0:51820"
target = "127.0.0.1:51820"        # e.g. WireGuard on the exit
protocol = "udp"                  # or "tcp+udp" for both on one port
```

- **Each client address is a flow.** The exit gives every flow its own socket to the
  target, so replies go back to the right client.
- **Packets keep their boundaries** and are never merged or split.
- **A full queue drops, never stalls.** When the tunnel cannot keep up, packets are
  dropped as the network would drop them.
- **Flows end after `tuning.udp_timeout_secs`** (default 60) without packets.
  `tuning.udp_max_flows` (default 1024 per rule) limits clients.
- **Use mux for UDP.** Without it, every flow needs a whole tunnel connection, and
  `kariz check` warns.

How a packet travels depends on the transport:

| Transport | UDP packets travel | A lost packet |
|---|---|---|
| `tcp`, `tcpmux`, `ws`, `wss` | inside the TCP stream | is resent; everything behind it waits |
| `quic` | as QUIC datagrams (large ones on a stream) | is lost; nothing waits |
| `kcp` | beside KCP's stream, sealed with the session keys | is lost, or rebuilt by FEC; nothing waits |

## Games

The recommended setup, in [`configs/entry-gaming.toml`](../configs/entry-gaming.toml)
and [`configs/exit-gaming.toml`](../configs/exit-gaming.toml):

```toml
profile = "gaming"                # on both sides

[tunnel]
transport = "kcp"

[[forward]]
listen = "0.0.0.0:27015"
target = "10.0.0.2:27015"
protocol = "tcp+udp"
duplicate = 2                     # each UDP packet twice, both ways
```

Measured over an emulated 60 ms, 50 Mbit/s path. Game traffic is 128-byte packets at 64
per second, each echoed. The table shows round-trip p50 / p99 in ms and the share of
packets that came back.

| Loss | Transport | Idle tunnel | Next to 4 downloads |
|---|---|---|---|
| 1 % | `tcpmux` | 62 / 169, 100 % | 395 / 734, 100 % |
| 1 % | `kcp`, UDP in the stream (v0.4) | 63 / 141, 100 % | 86 / 219, 100 % |
| 1 % | `kcp`, gaming profile | 63 / 69, 100 % | 66 / 82, 100 % |
| 5 % | `kcp`, UDP in the stream (v0.4) | 63 / 236, 100 % | 144 / 347, 100 % |
| 5 % | `kcp`, gaming profile | 63 / 84, 99.0 % | 68 / 94, 99.4 % |
| 5 % | `kcp`, gaming profile, 2 copies | 63 / 70, 99.5 % | 67 / 91, 100 % |
| 5 % | `quic`, 2 copies | 63 / 78, 99.2 % | 91 / 154, 94.4 % |

What makes the difference:

- **Game packets never wait for a lost one.** Over `kcp` they travel beside the reliable
  stream; over TCP-based transports every packet waits for the lost segment's resend.
- **FEC rebuilds most losses** within one KCP interval (20 ms with `fast2`), instead of
  losing them.
- **The gaming profile keeps downloads from filling the path.** Its 64 KiB stream window
  keeps the path's queue short, so game packets do not wait in it.
- **`quic` is weaker for games under loss.** quinn sends datagrams under the same
  congestion control as the streams, which slows down under random loss. After a long
  loss burst, it holds datagrams for a few hundred milliseconds.

Full numbers, including bursty loss and the other rows: [PHASE6.md](PHASE6.md).

## Packet duplication

`duplicate = 2` (or 3) on a UDP rule sends each packet that many times in all, in both
directions, `duplicate_gap_ms` apart (default 5). The receiver drops the copies.

- **Where it helps:** only over `kcp` and `quic`, and only where a packet may be lost.
  Packets that travel reliably (in a stream) are sent once.
- **What it does:** over a path losing 10 % each way, echoed game packets lost 0.5-3.5 %
  of round trips with 2 copies, against 21-23 % without.
- **What it costs:** that rule's bandwidth times the copies. About 100 kbit/s for a
  typical game, but real for WireGuard or bulk UDP, so set it only on game or voice
  rules.
- **Compatibility:** the exit must be v0.5 or newer; a v0.4 exit rejects those flows,
  and the entry logs it.

## DSCP

`[tuning] dscp = "ef"` (or `af41`, `cs4`, ..., or a number 0-63) marks the tunnel's
sockets and the exit's UDP sockets to targets, for networks that prioritise marked
traffic.

- **It is off by default.** On the public internet the mark is usually cleared or
  ignored, some networks treat marked traffic worse, and an unusual mark makes a flow
  stand out.
- **It helps on links you run yourself**, or with a queueing discipline such as `fq` or
  `cake` on the server.
- **QUIC sockets are not marked:** quinn sets the TOS byte of every packet itself.
- **Windows ignores the mark** unless a QoS policy allows it.

## MTU

`kcp` fits UDP packets of up to about 1,300 bytes beside its stream; larger ones go
through the stream. `quic` datagrams hold about 1,200 bytes. For WireGuard over either,
set the WireGuard interface's MTU to 1280 so its packets stay on the fast path.
