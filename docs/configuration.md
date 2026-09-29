# Configuration reference

One TOML file per side, `/etc/kariz/config.toml` by default. Only `role`, `mode` and
`[tunnel]` (with its `token`) are required; everything else has a default, and most
defaults come from the [profile](profiles.md). `kariz check -c <file>` validates a file
and prints the values in effect.

Unknown keys are errors, so a typo is caught rather than ignored.

## Top level

| Key | Values | Default | Notes |
|---|---|---|---|
| `role` | `entry`, `exit` | required | The entry owns the `[[forward]]` rules. |
| `mode` | `reverse`, `direct` | required | `reverse`: the exit dials the entry. `direct`: the entry dials the exit. |
| `profile` | `balanced`, `ultraspeed`, `gaming` | `balanced` | See [Profiles](profiles.md). `throughput` is accepted as the old name of `ultraspeed`. |

## `[tunnel]`

| Key | Values | Default | Notes |
|---|---|---|---|
| `transport` | `tcp`, `tcpmux`, `ws`, `wss`, `quic`, `kcp` | `tcp` | Same on both sides. See [Transports](transports.md). |
| `listen` | `host:port` | | Listening side: entry in reverse mode, exit in direct mode. |
| `remote` | `host:port` | | Dialing side: the other side's address (or a CDN edge). Resolved on every attempt. |
| `token` | at least 16 characters | required | Same on both sides. Generate with `kariz token`. |
| `encryption` | `auto`, `chacha20-poly1305`, `aes-256-gcm`, `none` | `auto` | `auto` dials with AES-256-GCM on CPUs with AES instructions, else ChaCha20-Poly1305, and accepts either. `none` only authenticates; set it on both sides or neither. `quic` always uses TLS 1.3: leave `auto`. |
| `pool` | at least 1 | `8` | Reverse mode without mux: idle connections the exit keeps open. |
| `speedtest` | `true`, `false` | `true` | The exit answers speed test streams from `kariz speedtest` on the entry side (at most 32 at once). They only produce, read and echo data. See [Speed test](speedtest.md). |

## `[tunnel.mux]`

Many user connections as streams over a few long-lived tunnel connections. Must be on
or off on both sides.

| Key | Values | Default | Notes |
|---|---|---|---|
| `enabled` | `true`, `false` | on for `tcpmux`, `ws`, `wss`, `kcp`, `quic`; off for `tcp` | `tcpmux` and `quic` cannot turn it off. |
| `connections` | 1-64 | profile (`quic`: 2) | Long-lived connections the dialing side keeps. |
| `max_streams` | 1-4096 | 512 | Concurrent streams per connection. |
| `stream_window` | 16 KiB - 16 MiB | profile | Bytes a stream may have in flight. One stream moves at most one window per round trip. |
| `max_lifetime_secs` | at least 60 | off | Replace each connection after this long (streams on it finish first). |
| `coalesce` | `true`, `false` | on (`gaming`: off) | Gather frames into larger writes (speed) or write each at once (latency). |
| `ping_interval_secs` | 1-600 | `tuning.keepalive_secs` | Pings; a peer silent for twice this long is dead. Also QUIC's keep-alive. Keep it at 90 or less behind a CDN. |
| `datagram_buffer` | 16 KiB - 64 MiB | profile | UDP bytes a session queues for sending; more are dropped. |
| `datagram_queue` | 8-65536 | profile | UDP packets each flow queues on the receiving side; the oldest are dropped. |
| `notsent_lowat` | 0, or 4 KiB - 16 MiB | 16384 | `TCP_NOTSENT_LOWAT` on TCP connections carrying mux, so UDP and new data are not stuck behind a kernel backlog. 0 turns it off. Linux only. |

## `[tunnel.ws]` (`ws`, `wss`)

| Key | Side | Default | Notes |
|---|---|---|---|
| `path` | both | `/` | Must match on both sides; any other path gets a `404` page. |
| `host` | both | | Dialer: the `Host` header (and SNI for `wss`), so `remote` can be a CDN IP. Listener: requests for any other host are rejected. |
| `early_data` | dialer | `false` | Put the tunnel hello inside the upgrade request, saving a round trip. The listener always accepts it. |
| `user_agent` | dialer | a desktop Chrome | |
| `headers` | dialer | | Extra request headers, e.g. `{ "Accept-Language" = "en-US,en;q=0.9" }`. Headers Kariz sets itself cannot be overridden. |

## `[tunnel.tls]` (`wss`)

| Key | Side | Notes |
|---|---|---|
| `cert`, `key` | listener | PEM certificate chain and private key. Reloaded when the files change, so certificate renewals need no restart. |
| `sni` | dialer | Server name in the TLS handshake. Default: `ws.host`, then the host in `remote`. |
| `pin_sha256` | dialer | Accept only this certificate (for self-signed ones). Print it with `kariz pin cert.pem`. |
| `insecure` | dialer | Accept any certificate. Warned; prefer `pin_sha256`. |

Without `pin_sha256` or `insecure`, the dialer checks the certificate against the Mozilla
root certificates.

## `[tunnel.quic]` (`quic`)

| Key | Default | Notes |
|---|---|---|
| `congestion` | `cubic` | `cubic`, `bbr` or `newreno`. BBR keeps its speed under random loss but is experimental in quinn and fills queues (warned). |
| `sni` | the host in `remote` (none for an IP) | Dialer only. |
| `alpn` | `h3` | Must match on both sides. |

## `[tunnel.kcp]` (`kcp`)

| Key | Values | Default | Notes |
|---|---|---|---|
| `mode` | `normal`, `fast`, `fast2`, `fast3`, `manual` | `fast2` | Presets from gentle to aggressive (below). |
| `nodelay`, `interval_ms`, `resend`, `no_congestion` | | from `fast2` | `manual` mode only. `interval_ms` 10-1000, `resend` at most 10. |
| `send_window` | 16-32768 packets | 1024 | About bandwidth x RTT / 1300 fills the path; smaller queues less but keeps less speed under loss. |
| `recv_window` | 128-32768 packets | 1024 | |
| `mtu` | 576-1443 | 1350 | KCP packet size; at most 1429 with FEC. |
| `fec_data`, `fec_parity` | 1-64 and 1-32, or both 0 | off (`gaming`: 10 and 3) | Reed-Solomon FEC per sending side: `fec_parity` parity packets per `fec_data` data packets. Both 0 turns it off. |
| `datagrams` | `true`, `false` | `true` | UDP flows travel beside KCP. `false`: inside its reliable stream, as in v0.4 (never lost, but they wait for retransmissions). |

| Preset | nodelay | interval | resend | congestion control |
|---|---|---|---|---|
| `normal` | off | 40 ms | 2 | off |
| `fast` | off | 30 ms | 2 | off |
| `fast2` | on | 20 ms | 2 | off |
| `fast3` | on | 10 ms | 2 | off |

## `[[forward]]` (entry only)

One table per rule; as many as needed.

| Key | Values | Default | Notes |
|---|---|---|---|
| `listen` | `host:port` | required | Where users connect, on the entry. |
| `target` | `host:port` | required | Resolved and dialed on the exit. |
| `protocol` | `tcp`, `udp`, `tcp+udp` | `tcp` | `tcp+udp` serves both on the same port. Use mux for UDP. |
| `duplicate` | 1-3 | 1 | UDP: send each packet this many times in all, both ways. Copies go only where packets may be lost (`kcp`, `quic`). |
| `duplicate_gap_ms` | 0-50 | 5 | Time between the copies of a packet. |

## `[tuning]`

Overrides of the profile's values.

| Key | Values | Default | Notes |
|---|---|---|---|
| `nodelay` | `true`, `false` | `true` | `TCP_NODELAY` on user and target connections. |
| `buffer_size` | at least 1024 bytes | profile | Relay buffer per connection. |
| `keepalive_secs` | | 30 (`gaming`: 10) | TCP keep-alive, and the default ping interval. QUIC and KCP ping every third of it. |
| `dial_timeout_secs` | | 10 | Connecting to the other side or a target. |
| `handshake_timeout_secs` | | 10 | The tunnel handshake. |
| `threads` | at least 1 | one per CPU core | Worker threads. |
| `udp_timeout_secs` | 5-3600 | 60 | A UDP flow ends after this long without packets. |
| `udp_max_flows` | 1-65536 | 1024 | UDP flows (client addresses) per rule; packets from more clients are dropped. |
| `dscp` | a name (`ef`, `af41`, `cs4`, ...) or 0-63 | off | DSCP mark on tunnel sockets and the exit's UDP sockets to targets. Not on QUIC sockets. See [UDP and games](udp-and-games.md#dscp). |

## `[control]`

A local socket on each side, through which `kariz status` reads the running daemon's
counters (both sides, see [Status](status.md)) and `kariz speedtest` uses its sessions
(the entry side, see [Speed test](speedtest.md)). Linux only.

| Key | Default | Notes |
|---|---|---|
| `socket` | the config file's path with `.sock` (`/etc/kariz/main.toml` gives `/etc/kariz/main.sock`) | Owner only. If it cannot be made, the tunnel runs without it and logs a warning. |

## `[log]`

| Key | Values | Default |
|---|---|---|
| `level` | `error`, `warn`, `info`, `debug`, `trace` | `info` |
| `color` | `auto`, `always`, `never` | `auto` |

`RUST_LOG=kariz=debug` in the environment overrides `level`. With `color = "auto"`, log
lines are coloured on a terminal unless `NO_COLOR` is set.

`kariz run` starts with a banner: the version, and what this side is about to do (its
role and mode, transport, profile, addresses and forward rules). On a terminal, each
line then shows the local time, a coloured level badge and the fields. Under systemd,
lines carry systemd's priority prefix instead, so `journalctl` highlights warnings and
errors itself and `journalctl -u kariz -p warning` shows only those.

## Settings that must match on both sides

`transport`, `token`, mux on or off, `ws.path`, `quic.alpn`, and `encryption = "none"`
(both or neither). A mismatch fails the handshake with an error saying which setting
differs, where the protocol can tell.
