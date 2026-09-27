# Changelog

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

### Changed

- The token is turned into a key with BLAKE3 and a version label; the v0.1 `ts | nonce
  | tag` hello is gone.

## 0.1.0

- First release: CLI (`run`, `check`, `token`), TOML config with profiles, mutual
  authentication, plain `tcp` transport, reverse and direct modes.
