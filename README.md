# Kariz

**English** | [فارسی](README_FA.md)

Kariz (کاریز, the ancient Persian underground water channel) is a tunnel core for
linking servers together. It is written in Rust and built around three goals:

1. **Low resource usage**: no garbage collector, about 5 MiB of RAM per side, suitable for cheap VPS.
2. **DPI resistance**: encrypted traffic without fixed bytes, browser-like WebSocket, works through CDNs.
3. **Maximum speed**: statically dispatched hot path, tuned sockets, per-use-case profiles.

> Status: **v0.5.0**, the gaming release. v0.5 works with v0.4 over every transport; UDP
> rules with `duplicate` need both sides at v0.5. See [docs/ROADMAP.md](docs/ROADMAP.md)
> for what comes next and [CHANGELOG.md](CHANGELOG.md) for what changed.

## Features

- **Transports:**

  | Transport | What goes over the network | Use it when |
  |---|---|---|
  | `tcp` | One TCP connection per user connection | Simple setups, maximum single-connection speed |
  | `tcpmux` | A few long-lived TCP connections carrying all user connections | Many user connections; fewer connections between the servers |
  | `ws` | WebSocket over TCP (HTTP) | Behind a CDN or reverse proxy that talks HTTP to the origin |
  | `wss` | WebSocket over TLS (HTTPS) | Behind a CDN, or on its own to look like an HTTPS site |
  | `quic` | QUIC over UDP: streams and datagrams, TLS 1.3 | UDP flows without head-of-line blocking; paths where UDP works well |
  | `kcp` | KCP over UDP, every packet encrypted, optional FEC | Lossy links where TCP collapses |

  `quic` and `kcp` run over UDP, which some networks throttle or block (UDP 443 in
  particular). They are options for the paths where UDP works, not replacements;
  switching transport is a one-line change.

- **Encryption** (`tunnel.encryption`, default `auto`): mutual authentication with a shared
  token that never goes over the wire. X25519 key exchange gives forward secrecy. Traffic
  is carried in AES-256-GCM or ChaCha20-Poly1305 records. The hello has no fixed bytes and
  no fixed length, and replayed hellos are rejected. A connection that fails the handshake
  is not closed at once, so probes learn nothing from the timing.
- **Mux** (`[tunnel.mux]`): user connections become streams on a few long-lived
  connections. Each stream has its own flow control, so a slow stream never blocks the
  others. Streams open without a round trip. Pings detect dead connections and keep CDNs
  from cutting idle ones. Connections can be rotated periodically.
- **WebSocket:**
  - The upgrade request looks like a browser's.
  - Early data (the handshake inside the upgrade request) saves one round trip through
    a CDN.
  - Anything that is not a valid upgrade on the configured path gets the `404` page of a
    stock nginx.
- **TLS** (`wss`): rustls. The dialer verifies the server certificate against the
  Mozilla roots, or accepts one pinned self-signed certificate (`kariz pin`). The
  listener reloads its certificate when the files change, so Let's Encrypt renewals
  need no restart.
- **UDP forwarding** (`protocol = "udp"` or `"tcp+udp"` on a `[[forward]]` rule) over
  every transport, so UDP (WireGuard, games, DNS, QUIC) keeps working where UDP itself
  is blocked. Each client address is a flow with its own socket on the exit side.
  Packets keep their boundaries, go out ahead of bulk TCP data sharing the connection,
  and are dropped rather than queued when the tunnel cannot keep up. Inside a TCP-based
  transport a lost segment still delays the packets behind it; over `quic` and `kcp`
  each packet is a datagram of its own and a loss delays nothing else.
- **QUIC** (`quic`): each user connection is a QUIC stream (no head-of-line blocking
  between them). Both sides authenticate with keys derived from the token (mutual TLS
  1.3, no certificate files). Congestion control: Cubic (default), BBR or NewReno. The
  handshake reads as HTTP/3 (ALPN `h3`, configurable SNI).
- **KCP** (`kcp`): resends aggressively and keeps its rate under loss, with presets from
  gentle to aggressive. Every UDP packet is encrypted with a key from the token, so the
  port answers nothing else and KCP's headers are hidden. Optional Reed-Solomon FEC
  rebuilds lost packets without a resend. UDP flows travel beside KCP's reliable stream,
  not inside it.
- **Games and real-time UDP** (see [Games](#games)): the `gaming` profile (FEC on over
  `kcp`, small buffers), packet duplication per forward rule (`duplicate = 2`), and
  optional DSCP marks.
- **Reverse and direct modes** for every transport, with automatic reconnects.

## Concepts

| Term | Meaning |
|---|---|
| **entry** | The public server users connect to (for example in Iran). Owns the `[[forward]]` rules. |
| **exit** | The server that connects to the real targets (for example abroad). |
| **reverse** mode | The exit side dials the entry side and keeps connections ready. |
| **direct** mode | The entry side dials the exit side. |

The side that dials needs `tunnel.remote`, the other side `tunnel.listen`.

## Quick start

Download the static Linux binary (x86_64) from
[Releases](https://github.com/Erfan-XRay/Kariz/releases), or build it:

```bash
cargo build --release
sudo cp target/release/kariz /usr/local/bin/
kariz token                               # generate a shared token
```

Pick a pair of sample configs, put the token in both, and adjust addresses:

| Samples (`configs/`) | Setup |
|---|---|
| `entry-reverse.toml`, `exit-reverse.toml` | Reverse mode over `tcp`: the exit connects to the entry |
| `entry-direct.toml`, `exit-direct.toml` | Direct mode over `tcp`: the entry connects to the exit |
| `entry-tcpmux-reverse.toml`, `exit-tcpmux-reverse.toml` | Reverse mode with mux |
| `entry-udp-reverse.toml`, `exit-udp-reverse.toml` | UDP forwarding (WireGuard, a game server on TCP+UDP) with mux |
| `entry-wss-direct.toml`, `exit-wss-direct.toml` | `wss` straight to your own server, self-signed certificate pinned |
| `entry-wss-cdn.toml`, `exit-wss-cdn.toml` | `wss` through a CDN such as Cloudflare (guide: [docs/CDN.md](docs/CDN.md)) |
| `entry-quic-direct.toml`, `exit-quic-direct.toml` | `quic`, with WireGuard over QUIC datagrams |
| `entry-kcp-reverse.toml`, `exit-kcp-reverse.toml` | `kcp` for a lossy link (FEC in comments) |
| `entry-gaming.toml`, `exit-gaming.toml` | A game server over `kcp` with the gaming profile and duplication |

```bash
kariz check -c /etc/kariz/config.toml     # validate, print a summary and warnings
kariz run -c /etc/kariz/config.toml       # run (or use systemd, below)
```

A systemd unit is in [`systemd/kariz.service`](systemd/kariz.service) (it reads
`/etc/kariz/config.toml`):

```bash
sudo cp systemd/kariz.service /etc/systemd/system/
sudo systemctl enable --now kariz
journalctl -u kariz -f
```

## Commands

| Command | Does |
|---|---|
| `kariz run -c <file>` | Run one side of the tunnel. |
| `kariz check -c <file>` | Validate a config and print a summary and warnings. |
| `kariz token` | Print a random token for `tunnel.token`. |
| `kariz pin <cert.pem>` | Print a certificate's `tunnel.tls.pin_sha256`. |

Set `RUST_LOG=kariz=debug` for detailed logs (it overrides `[log] level`).

## Configuration

Every table except `[tunnel]` is optional; the samples show each field in context.
Config files from v0.1 still work.

```toml
role = "entry"                  # entry | exit
mode = "direct"                 # direct | reverse
profile = "balanced"            # balanced | throughput | gaming

[tunnel]
transport = "wss"               # tcp | tcpmux | ws | wss | quic | kcp
remote = "203.0.113.1:443"      # dialing side: the other side (or a CDN edge)
# listen = "0.0.0.0:443"        # listening side
token = "..."                   # same on both sides, at least 16 characters
encryption = "auto"             # auto | chacha20-poly1305 | aes-256-gcm | none (both sides;
                                #   quic: always TLS 1.3, leave at auto)
# pool = 8                      # reverse mode without mux: idle connections the exit keeps

[tunnel.mux]                    # must be on or off on both sides
enabled = true                  # default: on for tcpmux, ws, wss, kcp; off for tcp; quic: always
connections = 4                 # long-lived connections (dialing side; quic default 2)
max_streams = 512               # user connections per connection
stream_window = 262144          # per-stream window in bytes
# max_lifetime_secs = 3600      # rotate connections (at least 60)

[tunnel.ws]                     # ws / wss
path = "/api/v1/stream"         # same on both sides
host = "tunnel.example.com"     # dialer: Host header (and SNI); listener: reject other hosts
early_data = true               # dialer: hello inside the upgrade request
# user_agent = "..."            # dialer: default is a desktop Chrome
# headers = { "Accept-Language" = "en-US,en;q=0.9" }

[tunnel.tls]                    # wss
# sni = "tunnel.example.com"    # dialer: default is ws.host, then the remote host
# pin_sha256 = "..."            # dialer: accept only this certificate (kariz pin)
# insecure = false              # dialer: accept any certificate (warned)
# cert = "/etc/kariz/cert.pem"  # listener: certificate chain, reloaded on change
# key = "/etc/kariz/key.pem"    # listener: private key

[tunnel.quic]                   # quic
congestion = "cubic"            # cubic | bbr | newreno (bbr: experimental in quinn)
# sni = "www.example.com"       # dialer: server name in the handshake; default: remote host
# alpn = "h3"                   # same on both sides

[tunnel.kcp]                    # kcp
mode = "fast2"                  # normal | fast | fast2 | fast3 | manual (then nodelay,
                                #   interval_ms, resend, no_congestion)
# send_window = 1024            # packets; smaller: less queueing, less speed under loss
# recv_window = 1024
# mtu = 1350                    # at most 1443 (1429 with FEC)
# fec_data = 10                 # FEC, per sending side: parity packets per group of
# fec_parity = 3                #   data packets; both 0 = off. Default: off, 10 / 3 with
                                #   the gaming profile
# datagrams = true              # UDP flows beside KCP; false: inside its stream (v0.4)

[[forward]]                     # entry only; as many as needed
listen = "0.0.0.0:443"          # where users connect
target = "127.0.0.1:443"        # dialed by the exit side
protocol = "tcp"                # tcp | udp | tcp+udp (use mux for UDP)
# duplicate = 2                 # UDP: send each packet 2 or 3 times, both ways (kcp, quic)
# duplicate_gap_ms = 5          #   time between the copies (0-50)

[tuning]                        # overrides of the profile values
# nodelay = true
# buffer_size = 65536
# keepalive_secs = 30           # TCP keepalive and mux ping interval (<= 90 behind a CDN);
                                #   quic / kcp: ping every third, peer dead after twice
# dial_timeout_secs = 10
# handshake_timeout_secs = 10
# threads = 2                   # default: one per CPU core
# udp_timeout_secs = 60         # a UDP flow ends after this long without packets
# udp_max_flows = 1024          # UDP flows (client addresses) per rule
# dscp = "ef"                   # DSCP mark (name or 0-63) on tunnel and target sockets

[log]
level = "info"                  # error | warn | info | debug | trace
```

### Profiles

| | `balanced` (default) | `throughput` | `gaming` |
|---|---|---|---|
| Relay buffer | 64 KiB | 256 KiB | 16 KiB |
| Keepalive / mux ping | 30 s | 30 s | 10 s |
| Mux stream window | 256 KiB | 1 MiB | 64 KiB |
| Mux connections | 4 | 8 | 2 |
| Mux write coalescing | on | on | off |
| KCP FEC | off | off | 10 / 3 |

## Performance

Localhost measurements: user → entry → exit → echo server, 256 MiB echoed, with both
sides and the echo server on one 4-core Xeon (2.1 GHz) VM. Everything shares the CPU,
so this compares setups; a real link between two servers is usually limited by the
network first. Reproduce with
`cargo test --release --test tunnel throughput -- --ignored --nocapture`.

| Setup | Mbit/s each way |
|---|---|
| `tcp`, no encryption | 5,200-6,600 |
| `tcp`, AES-256-GCM | 3,800-4,000 |
| `tcp`, ChaCha20-Poly1305 | 2,100-2,800 |
| `tcpmux`, no encryption | 4,800-5,000 |
| `tcpmux`, AES-256-GCM | 3,200-3,300 |
| `ws`, no mux, AES-256-GCM | 4,600-4,700 |
| `ws` + mux, AES-256-GCM | 2,800-3,100 |
| `wss`, no mux, AES-256-GCM + TLS | 2,800-3,200 |
| `wss` + mux, AES-256-GCM + TLS | 2,300-2,500 |

UDP through the tunnel (8 clients, each with 32 packets in flight;
`cargo test --release --test tunnel udp_ -- --ignored --nocapture`):

| Setup | 100-byte packets | 1,400-byte packets |
|---|---|---|
| `tcpmux` | 143,000 packets/s | 126,000 packets/s (1.4 Gbit/s) |
| `tcp`, no mux | 141,000 packets/s | 123,000 packets/s |
| `ws` + mux | 140,000 packets/s | 125,000 packets/s |
| `wss` + mux | 147,000 packets/s | 120,000 packets/s |

UDP round trip: an idle tunnel adds about 90 µs (125 µs through the tunnel versus
37 µs straight to the target). With four bulk TCP transfers sharing the same mux
connection over a throttled 20 Mbit/s link, UDP round trips stay around 60 ms (p50) /
100 ms (p99), most of which is the emulated link's own buffer; without the unsent-data
limit Kariz sets on mux connections (`TCP_NOTSENT_LOWAT`) they were about 340 ms.

Memory (RSS, release build, `scripts/rss.sh`):

| Transport | Idle, per side | 100 idle user connections |
|---|---|---|
| `tcp` | 4.8-5.0 MiB | +4.1 MiB |
| `tcpmux` | 5.7-5.8 MiB | +0.6 to +1.0 MiB |
| `ws` | 5.7 MiB | +0.6 to +1.0 MiB |

1,000 idle UDP flows over `tcpmux` (`scripts/rss.sh tcpmux 1000 target/release/kariz udp`):
+3.1 to 3.5 MiB per side.

### `quic` and `kcp`

Measured on GitHub Actions runners (2 vCPU AMD EPYC), a different machine from the
tables above, so `tcpmux` from the same runs is given for comparison.

| Setup | Localhost, Mbit/s each way | UDP, 100 / 1,400-byte packets per second | Idle UDP round trip added |
|---|---|---|---|
| `tcpmux`, AES-256-GCM | 2,960-3,420 | 69,000 / 68,000 | +66 µs |
| `quic` | 810-950 | 72,000 / 63,000 | +74 µs |
| `kcp` + mux, AES-256-GCM | 600-670 | 69,000 / 44,000 | +71 µs |
| `kcp`, FEC 10 / 3 | 500 | | |

QUIC and KCP run their protocol in user space, a packet at a time, so on localhost they
reach a fraction of what TCP (with the kernel doing that work) does. Between two
servers this only matters above several hundred Mbit/s.

| Transport | Idle, per side | 100 idle user connections | 1,000 idle UDP flows |
|---|---|---|---|
| `quic` | 7.3-7.4 MiB | +0.9 to +1.1 MiB | +4.1 to +4.5 MiB |
| `kcp` | 6.4-6.7 MiB | +0.8 to +1.0 MiB | +3.2 to +3.6 MiB |

### Over a lossy link

The tests carry a link emulator (delay, random loss, a 50 Mbit/s bottleneck with a
50 ms queue). Its TCP side models the sender's TCP, so loss slows `tcpmux` the way it
would on a real path. Measured on the same runner, 60 ms round trip, loss in both
directions: a 10 s download, and a UDP flow of 100-byte packets every 20 ms, first on
the idle tunnel, then during the download. Streams use a 4 MiB window here.
Reproduce with
`cargo test --release --test tunnel lossy_link -- --ignored --nocapture`.

| Transport | Download, 0 % / 1 % / 5 % loss | UDP p99 on the idle tunnel, 1 % / 5 % loss |
|---|---|---|
| `tcpmux` | 48.1 / 2.3 / 0.9 Mbit/s | 164 / 243 ms |
| `quic` (Cubic) | 47.9 / 2.7 / 1.0 Mbit/s | 64 / 64 ms (1.2 / 9.8 % of packets lost) |
| `quic` (BBR) | 47.5 / 47.4 / 45.7 Mbit/s | 64 / 64 ms (same) |
| `kcp` | 31.4 / 28.5 / 22.5 Mbit/s | 64 / 64 ms (1.4 / 9.8 % lost)\* |
| `kcp`, FEC 10 / 3 | 25.4 / 26.1 / 26.6 Mbit/s | 64 / 84 ms (0 / 0.6 % lost)\* |

\* v0.5, where UDP travels beside KCP (v0.4: 141 / 202 and 64 / 143 ms, none lost),
measured on the development machine; its downloads were within 5 % of v0.4's.

- **Loss-based congestion control collapses on random loss**, TCP or QUIC alike
  (Cubic: 2-3 Mbit/s at 1 %). `quic` with BBR and `kcp` keep their rate.
- **UDP over `quic` and `kcp` never waits** for a lost packet (it is lost instead, as on
  the path itself; over `kcp`, FEC rebuilds most of them). Over `tcpmux` nothing is
  lost, but a loss costs a resend.
- **On a link kept full by a download**, UDP packets over `kcp` are now dropped at the
  link's queue instead of waiting behind it: 5-13 % lost with this benchmark's 4 MiB
  windows, p99 about 115 ms instead of 200-340 ms. The gaming profile does not fill the
  link (see [Games](#games)); `[tunnel.kcp] datagrams = false` brings back waiting.
- **BBR (experimental in quinn) overfills the bottleneck's queue.** During a download
  about half of the UDP packets on the same connection were lost, so Cubic stays the
  default. BBR is for bulk transfer over lossy paths without real-time UDP.
- **KCP's window (1024 packets) overfills small paths too.** On this one it caused
  about 35,000 queue drops in 10 s, and a UDP p99 of 230-440 ms during the download.
  A window near bandwidth x RTT / 1300 packets queues far less (256 here: p99 about
  100-200 ms), but keeps less of its rate under loss (17 Mbit/s at 1 %). KCP's own
  congestion control (`no_congestion = false`) reached only 0.4-1.2 Mbit/s.
- **The default stream window (256 KiB) caps one stream at 256 KiB per round trip**
  (about 25-30 Mbit/s at 60 ms). The `throughput` profile (1 MiB) or
  `tunnel.mux.stream_window` raise it.

### Games

Game traffic over the emulated path (60 ms round trip, 50 Mbit/s): 128-byte packets at
64 Hz, each echoed by the server, 30 s, with the `gaming` profile on both sides. Round
trip p50 / p99 in ms and the share of packets that came back, on an idle tunnel and next
to four downloads on the same tunnel. Development machine; reproduce with
`cargo test --release --test tunnel game_traffic -- --ignored --nocapture`.

| Loss | Transport | Idle tunnel | Next to 4 downloads | Downloads |
|---|---|---|---|---|
| 1 % | `tcpmux` | 62 / 169, 100 % | 395 / 734, 100 % | 4.6 Mbit/s |
| 1 % | `kcp` as in v0.4 | 63 / 141, 100 % | 86 / 219, 100 % | 18.0 Mbit/s |
| 1 % | `kcp` (gaming: FEC 10 / 3) | 63 / 69, 100 % | 66 / 82, 100 % | 29.2 Mbit/s |
| 1 % | `quic` | 63 / 64, 98.2 % | 77 / 113, 97.5 % | 4.2 Mbit/s |
| 1 % | `quic`, 2 copies | 62 / 68, 100 % | 76 / 112, 99.2 % | 4.0 Mbit/s |
| 5 % | `tcpmux` | 77 / 242, 100 % | 1,112 / 1,616, 100 % | 1.9 Mbit/s |
| 5 % | `kcp` as in v0.4 | 63 / 236, 100 % | 144 / 347, 100 % | 11.5 Mbit/s |
| 5 % | `kcp`, FEC off | 63 / 64, 90.2 % | 63 / 83, 89.5 % | 11.3 Mbit/s |
| 5 % | `kcp` (gaming: FEC 10 / 3) | 63 / 84, 99.0 % | 68 / 94, 99.4 % | 26.5 Mbit/s |
| 5 % | `kcp` (gaming), 2 copies | 63 / 70, 99.5 % | 67 / 91, 100 % | 26.5 Mbit/s |
| 5 % | `quic`, 2 copies | 63 / 78, 99.2 % | 91 / 154, 94.4 % | 1.7 Mbit/s |

- **Use `kcp` with the `gaming` profile on both sides** (`entry-gaming.toml`). Game
  packets never wait for a lost one, FEC rebuilds most losses within about 20 ms, and
  downloads next to the game keep their speed. Its 64 KiB stream window keeps each
  download from filling the link, so the game's p99 stays under twice the round trip.
- **`duplicate = 2`** on the game's forward rule sends each packet twice, both ways. It
  matters most over `quic`, which has no FEC, and costs twice that rule's traffic
  (about 100 kbit/s for a typical game), so set it only on game or voice rules, not on
  WireGuard.
- **`quic` is the weaker choice for games under loss.** quinn sends datagrams under the
  same congestion control as the streams, which collapses under random loss, and after
  a long loss burst it holds datagrams for a few hundred milliseconds.
- **TCP-based transports** (`tcp`, `tcpmux`, `ws`, `wss`) make every packet wait for a
  lost segment, whatever the profile.
- **DSCP** (`[tuning] dscp = "ef"`) only helps where the network honours it: links you
  run yourself, or a qdisc such as `fq` or `cake` on the server. On the public internet
  the mark is usually cleared, and some networks treat marked traffic worse.

### Build size

Static musl release binary (x86_64), with and without the optional transports (cargo
features `quic` and `kcp`, both on by default):

| Features | Size | Crates |
|---|---|---|
| default (`quic` + `kcp`) | 6.0 MB | 110 |
| `kcp` only | 4.8 MB | 88 |
| `quic` only | 5.8 MB | 106 |
| neither | 4.5 MB | 82 |

## Security notes

- The token is the only secret. Anyone who has it can use the tunnel, so generate it
  with `kariz token` and keep it out of shared places.
- `encryption = "none"` only authenticates; the traffic is readable on the wire. Use it
  only inside another encrypted layer.
- Behind a CDN, TLS ends at the CDN. The tunnel's own encryption still protects the
  content from the CDN.
- The TLS ClientHello of rustls does not look like a browser's, and a plain HTTP request
  to a `wss` port gets a TLS error rather than an nginx page. The same goes for quinn's
  QUIC handshake. Shaping these is later work (a stealth profile, not scheduled).
- A `kcp` port answers nothing that was not sealed with the token. The key for this
  packet layer comes from the token and has no forward secrecy of its own; the
  tunnel's handshake and encryption run inside it as over TCP, and UDP packets beside
  KCP are sealed with keys from the same handshake.

## Development

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test                          # nginx tests run too when nginx is installed
cargo build --release --no-default-features   # without quic and kcp (features `quic`, `kcp`)
cargo test --release --test tunnel throughput -- --ignored --nocapture
cargo test --release --test tunnel lossy_link -- --ignored --nocapture   # KARIZ_BENCH_ONLY=kcp
cargo test --release --test tunnel game_traffic -- --ignored --nocapture # about 45 min
scripts/rss.sh tcpmux 100           # memory, after cargo build --release
scripts/rss.sh tcpmux 1000 target/release/kariz udp   # memory per UDP flow
KARIZ_TEST_LOG=1 cargo test --test tunnel <name>        # with the tunnel's debug logs
```

Design documents: [docs/ROADMAP.md](docs/ROADMAP.md), [docs/PHASE2.md](docs/PHASE2.md),
[docs/PHASE3.md](docs/PHASE3.md), [docs/PHASE4.md](docs/PHASE4.md),
[docs/PHASE6.md](docs/PHASE6.md).
