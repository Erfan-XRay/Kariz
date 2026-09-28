# Changelog

## Unreleased

### Added

- **mimalloc** as the global allocator of the `kariz` binary (cargo feature
  `mimalloc`, on by default).
- **A startup banner and new log lines.** `kariz run` starts with the Kariz logo, the
  version, the author and a summary of this side's setup. On a terminal, log lines
  carry the local time, coloured level badges and highlighted fields. Under systemd
  they carry priority prefixes instead, so `journalctl` highlights warnings and errors
  and `-p warning` filters them. `[log] color = "auto" | "always" | "never"`; `NO_COLOR`
  is respected.

## 0.5.1 - 2026-09-29

Works with v0.5 and v0.4 as before; the new settings only change the side they are set on.

### Added

- **More mux settings in `[tunnel.mux]`**, each defaulting to what it was before (the
  profile's value): `coalesce`, `ping_interval_secs` (default `tuning.keepalive_secs`;
  also QUIC's keep-alive), `datagram_buffer`, `datagram_queue` and `notsent_lowat` (0
  turns it off). `kariz check` shows the values in effect.
- **Documentation** in [`docs/`](docs/README.md): getting started, a configuration
  reference with every setting, transports, profiles, UDP and games, performance,
  security and troubleshooting.
- **A logo and a new README** (English and Persian).

### Changed

- **The `throughput` profile is now `ultraspeed`.** `profile = "throughput"` is still
  accepted.
- The stealth profile (planned for later) is dropped from the roadmap.

## 0.5.0 - 2026-09-28

The gaming release. Works with v0.4 over every transport: the new datagram path over
`kcp` is negotiated inside the session and stays off with a v0.4 peer. UDP rules with
`duplicate` need the exit at v0.5 (a v0.4 exit rejects those flows, and the entry logs
it).

### Added

- **Datagram path over `kcp`:** UDP flows travel beside KCP instead of inside its
  reliable stream, so a lost packet never holds up the ones behind it. On an emulated
  60 ms path the p99 round trip of game traffic stays at the path's RTT under loss
  (64 ms, against 141 ms at 1 % loss and 236 ms at 5 % in v0.4).
  - Sealed with keys from the handshake (forward secrecy, as everything else) and an
    explicit packet number; replays and copies are dropped.
  - Protected by FEC when FEC is on.
  - Packets too large for one KCP packet (above about 1,300 bytes) still go through
    the stream.
- **Packet duplication per forward rule:** `duplicate = 2 | 3` and `duplicate_gap_ms`
  (default 5). Each UDP packet is sent again, both ways, where it may be lost (KCP's
  datagram path, QUIC datagrams); the receiver drops the copies. Over a path losing
  10 % each way, echoed game packets lost 0.5-3.5 % of round trips with 2 copies,
  against 21-23 % without.
- **DSCP:** `[tuning] dscp = "ef"` (or `af41`, `cs4`, ..., or 0-63) marks TCP tunnel
  sockets, KCP sockets and the exit's UDP sockets to targets. Off by default: most of
  the internet clears or ignores the mark. QUIC sockets are not marked (quinn sets the
  TOS byte of every packet itself); `kariz check` says so.
- **`[tunnel.kcp] datagrams`** (default `true`): `false` keeps UDP flows in the
  reliable stream, as in v0.4.
- **Samples:** `entry-gaming.toml` / `exit-gaming.toml`.
- **Game-traffic benchmark:** 128-byte packets at 64 Hz, echoed, on an idle tunnel and
  next to four downloads, at 0 / 1 / 5 % and bursty loss (`cargo test --release --test
  tunnel game_traffic -- --ignored --nocapture`). Results in the README and
  docs/PHASE6.md.

### Changed

- **UDP over `kcp` can now lose packets, as UDP does**, rather than waiting for
  retransmissions. On a link that a bulk transfer keeps full (the balanced profile's
  large windows over `kcp`), UDP packets are dropped at the link's queue instead of
  waiting behind it (5-13 % lost in the phase 4 benchmark, p99 about 115 ms instead of
  200-340 ms). `[tunnel.kcp] datagrams = false` restores the old behaviour.
- **The gaming profile turns FEC 10 / 3 on over `kcp`**, unless `[tunnel.kcp]` sets
  `fec_data` or `fec_parity` (0 and 0 turn it off). At 5 % loss 99 % of game packets
  came back instead of 90 %, and downloads next to the game went 2.3x faster.
- `kariz check` shows each rule's duplication and the DSCP mark, and warns where they
  have no effect.
- Roadmap: the `icmp` transport (phase 5) is dropped; the stealth profile and
  active-probe fallback are later work, not scheduled.

## 0.4.0 - 2026-09-28

Works with v0.3 over `tcp`, `tcpmux`, `ws` and `wss` (their wire format is unchanged);
the new transports need both sides at v0.4.

### Added

- **`transport = "quic"`** (quinn), in both modes:
  - Each user connection is a QUIC stream. UDP flows travel as QUIC datagrams, so a lost
    packet delays nothing else. Packets above the datagram limit (about 1,200 bytes,
    depending on the path) go on the flow's stream instead: they still arrive, just
    reliably.
  - Mutual TLS 1.3 with an identity derived from the token. There are no certificate
    files, and a wrong token fails the handshake in both directions.
  - `[tunnel.quic]`: `congestion` (`cubic`, the default, `bbr` or `newreno`), `sni`
    (dialer; default: the remote host), `alpn` (default `h3`).
  - A restarted endpoint resets the old connections at once (stateless resets with a key
    from the token).
  - No 0-RTT: sessions are pooled before users arrive, and early data could be replayed.
- **`transport = "kcp"`**, in both modes, with or without mux:
  - KCP (an ARQ protocol over UDP that keeps its rate under loss) as a stream under the
    usual handshake, encryption and mux.
  - Every UDP packet is sealed with a key from the token. The port answers nothing else
    (probes, other tokens, tampered packets), and KCP's headers are hidden.
  - Conversations have an explicit open, half-close and close, and keep-alive with a
    silence timeout. A restarted listener closes old conversations at once.
  - `[tunnel.kcp]`: `mode` (`normal`, `fast`, `fast2` (default), `fast3` or `manual` with
    `nodelay`, `interval_ms`, `resend`, `no_congestion`), `send_window`, `recv_window`,
    `mtu`.
  - **FEC**: `fec_data` / `fec_parity` (off by default; `10` / `3` to start). A lost
    packet is rebuilt from Reed-Solomon parity instead of resent. On a lossy link this
    halves the p99 latency of sparse traffic; bulk throughput does not improve.
- **Cargo features** `quic` and `kcp` (both on by default), to build without them.
- **Samples:** `entry-quic-direct.toml` / `exit-quic-direct.toml` (with WireGuard over
  datagrams), `entry-kcp-reverse.toml` / `exit-kcp-reverse.toml`.
- **Lossy-link benchmark:** the tests carry a UDP and a TCP link emulator (delay,
  jitter, random or bursty loss, reordering, a rate-limited bottleneck with a queue).
  The TCP one models the sender's TCP, so loss slows `tcpmux` as it would on a real
  path. See the Performance section of the README and docs/PHASE4.md.

### Changed

- A session layer between entry / exit and the multiplexers (kmux, QUIC). No behaviour
  change for the existing transports.

## 0.3.0 - 2026-09-28

Works with v0.2 for TCP; UDP forwarding needs both sides at v0.3 (a v0.2 exit rejects
UDP flows, and the entry logs that the exit side does not support UDP).

### Added

- **UDP forwarding:** `[[forward]] protocol = "udp"` or `"tcp+udp"` (both on one port),
  over every transport and in both modes.
  - Each client address is a flow: a mux stream carrying `DGRAM` frames, or without mux
    a whole tunnel connection carrying length-prefixed packets. On the exit, each flow
    has its own socket connected to the target.
  - Flows end after `tuning.udp_timeout_secs` (default 60) without packets.
    `tuning.udp_max_flows` (default 1024 per rule) limits clients.
  - A client whose flow cannot be opened is ignored for 5 s instead of retried on every
    packet.
- **Mux datagrams:** `DGRAM` frames need no flow-control credit. They are sent ahead of
  stream data, but take at most half of a write while streams wait, so UDP cannot starve
  TCP. They are dropped instead of queued when the session's buffer is full (new packets
  on send, the oldest on receive).
- **`TCP_NOTSENT_LOWAT` (16 KiB)** on tunnel connections that carry mux, so the kernel
  does not hold a backlog that datagrams would wait behind. Over a throttled 20 Mbit/s
  link with four bulk transfers, UDP round trips went from about 340 ms to about 60 ms.
  No throughput cost measured.
- **Wire format:** open kind 2 (UDP), status 2 and reset reason 4 ("unsupported").
- **Samples and checks:**
  - `entry-udp-reverse.toml` / `exit-udp-reverse.toml` (WireGuard, a game server on
    TCP+UDP).
  - `kariz check` shows UDP settings and warns when UDP is forwarded without mux.
- **Tests and benchmarks:**
  - A UDP echo scenario in every row of the end-to-end matrix; idle flows, max flows,
    UDP through nginx.
  - Benchmarks for packets per second, idle latency, and latency under load over a
    throttled link.
  - `scripts/rss.sh` measures UDP flows.
  - `KARIZ_TEST_LOG=1` shows the sides' logs in end-to-end tests.

### Fixed

- The mux writer sends pending `SYN`s in stream id order before any datagram or data. A
  first datagram could otherwise pull its stream's `SYN` ahead of an older stream's,
  and the peer closed the session for an invalid id. This came from the phase 3 work,
  so no release is affected.
- An empty payload (a 0-byte datagram) no longer adds an empty buffer to a vectored
  write, which read as a dead connection.

## 0.2.0 - 2026-09-27

**Breaking:** the wire format changed. v0.2 and v0.1 cannot talk to each other; upgrade
both servers together. Config files from v0.1 still work and get encryption
automatically.

### Added

- **Encryption:**
  - A new handshake with mutual authentication (shared token) and X25519 forward
    secrecy. The hello is masked and randomly padded, so it has no fixed bytes and no
    fixed length.
  - AEAD records after the handshake (`tunnel.encryption`: `auto`, `chacha20-poly1305`,
    `aes-256-gcm`, `none`), with one key per direction.
  - 0-RTT open requests in direct mode.
  - A failed handshake is drained for a random 5-30 s instead of being closed at once.
- **Mux** (`transport = "tcpmux"`, `[tunnel.mux]`):
  - Many user connections over a few long-lived connections, with per-stream flow
    control and no round trip to open a stream.
  - Pings to detect dead connections, and optional rotation (`max_lifetime_secs`).
  - Reconnects with backoff, in both modes.
- **`ws` transport:**
  - A browser-like WebSocket upgrade (path, Host, User-Agent and extra headers are
    configurable).
  - An nginx-style `404` for anything else.
  - Early data (`ws.early_data`): the hello rides in the upgrade request, and an upgrade
    whose hello does not verify gets a `404` too.
- **`wss` transport:**
  - TLS with rustls (ring). The dialer checks the server against the Mozilla roots,
    one pinned certificate (`tls.pin_sha256`) or none (`tls.insecure`, warned).
  - Separate connect address and SNI, for CDN edge IPs.
  - The listener reloads `tls.cert` / `tls.key` when they change.
- **Commands and checks:**
  - `kariz pin <cert.pem>` prints a certificate's pin.
  - `kariz check` shows the new settings and warns about `encryption = "none"`,
    `tls.insecure`, and keepalives above 90 s on WebSocket transports.
- **Docs and samples:**
  - Samples for mux, `wss` with a pinned certificate, and `wss` through a CDN.
  - [docs/CDN.md](docs/CDN.md) with a Cloudflare checklist.
- **Tests and CI:**
  - An end-to-end test matrix over modes, transports, mux and ciphers.
  - A CI job that runs the tunnel through nginx as a stand-in CDN (idle timeouts,
    TLS termination).
  - A test that the sample configs are valid and paired.
  - `scripts/rss.sh` for memory measurements.
  - A release workflow: a tag push publishes a static x86_64 Linux binary (musl) with
    the samples and docs.

### Changed

- The token is turned into a key with BLAKE3 and a version label; the v0.1 `ts | nonce
  | tag` hello is gone.

## 0.1.0

- First release: CLI (`run`, `check`, `token`), TOML config with profiles, mutual
  authentication, plain `tcp` transport, reverse and direct modes.
