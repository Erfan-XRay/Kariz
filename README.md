# Kariz

**English** | [فارسی](README_FA.md)

Kariz (کاریز, the ancient Persian underground water channel) is a tunnel core for
linking servers together. It is written in Rust and built around three goals:

1. **Low resource usage**: no garbage collector, about 5 MiB of RAM per side, suitable for cheap VPS.
2. **DPI resistance**: encrypted traffic without fixed bytes, browser-like WebSocket, works through CDNs.
3. **Maximum speed**: statically dispatched hot path, tuned sockets, per-use-case profiles.

> Status: **v0.2.0.** The wire format changed after v0.1.0: upgrade both servers
> together. See [docs/ROADMAP.md](docs/ROADMAP.md) for what comes next (UDP, KCP, QUIC,
> ICMP, stealth profile) and [CHANGELOG.md](CHANGELOG.md) for what changed.

## Features

- **Transports:**

  | Transport | What goes over the network | Use it when |
  |---|---|---|
  | `tcp` | One TCP connection per user connection | Simple setups, maximum single-connection speed |
  | `tcpmux` | A few long-lived TCP connections carrying all user connections | Many user connections; fewer connections between the servers |
  | `ws` | WebSocket over TCP (HTTP) | Behind a CDN or reverse proxy that talks HTTP to the origin |
  | `wss` | WebSocket over TLS (HTTPS) | Behind a CDN, or on its own to look like an HTTPS site |

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
| `entry-wss-direct.toml`, `exit-wss-direct.toml` | `wss` straight to your own server, self-signed certificate pinned |
| `entry-wss-cdn.toml`, `exit-wss-cdn.toml` | `wss` through a CDN such as Cloudflare (guide: [docs/CDN.md](docs/CDN.md)) |

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
transport = "wss"               # tcp | tcpmux | ws | wss
remote = "203.0.113.1:443"      # dialing side: the other side (or a CDN edge)
# listen = "0.0.0.0:443"        # listening side
token = "..."                   # same on both sides, at least 16 characters
encryption = "auto"             # auto | chacha20-poly1305 | aes-256-gcm | none (both sides)
# pool = 8                      # reverse mode without mux: idle connections the exit keeps

[tunnel.mux]                    # must be on or off on both sides
enabled = true                  # default: on for tcpmux, ws, wss; off for tcp
connections = 4                 # long-lived connections (dialing side)
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

[[forward]]                     # entry only; as many as needed
listen = "0.0.0.0:443"          # where users connect
target = "127.0.0.1:443"        # dialed by the exit side

[tuning]                        # overrides of the profile values
# nodelay = true
# buffer_size = 65536
# keepalive_secs = 30           # TCP keepalive and mux ping interval (<= 90 behind a CDN)
# dial_timeout_secs = 10
# handshake_timeout_secs = 10
# threads = 2                   # default: one per CPU core

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

Memory (RSS, release build, `scripts/rss.sh`):

| Transport | Idle, per side | 100 idle user connections |
|---|---|---|
| `tcp` | 4.8-5.0 MiB | +4.1 MiB |
| `tcpmux` | 5.7-5.8 MiB | +0.6 to +1.0 MiB |
| `ws` | 5.7 MiB | +0.6 to +1.0 MiB |

## Security notes

- The token is the only secret. Anyone who has it can use the tunnel, so generate it
  with `kariz token` and keep it out of shared places.
- `encryption = "none"` only authenticates; the traffic is readable on the wire. Use it
  only inside another encrypted layer.
- Behind a CDN, TLS ends at the CDN. The tunnel's own encryption still protects the
  content from the CDN.
- The TLS ClientHello of rustls does not look like a browser's, and a plain HTTP request
  to a `wss` port gets a TLS error rather than an nginx page. Both are for the planned
  stealth work (phase 6).

## Development

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test                          # nginx tests run too when nginx is installed
cargo test --release --test tunnel throughput -- --ignored --nocapture
scripts/rss.sh tcpmux 100           # memory, after cargo build --release
```

Design documents: [docs/ROADMAP.md](docs/ROADMAP.md), [docs/PHASE2.md](docs/PHASE2.md).
