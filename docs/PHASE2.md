# Phase 2 plan: encryption, mux, WebSocket

Target release: **v0.2.0**. Scope from [ROADMAP.md](ROADMAP.md): encryption layer,
`tcpmux`, `ws` / `wss` (CDN friendly).

This document fixes the design, the wire formats and the order of work, so each step
can land as its own PR with CI green.

## 1. Where v0.1 stands (what this phase has to fix)

| Area | v0.1 behaviour | Problem |
|---|---|---|
| Confidentiality | After auth, bytes are relayed in the clear. | Anyone on the path reads and fingerprints the inner traffic (TLS ClientHello of the user, HTTP, ...). |
| Handshake shape | `ts (8) \| nonce (16) \| tag (32)`, `ts` in plain big-endian. | The first 4 bytes of every hello are `00 00 00 00` and the next ones are nearly constant. That is a fixed-byte fingerprint, the exact thing `auth.rs` wants to avoid. Hello length is also always 56. |
| Failed auth | Connection is closed right away. | "Closes after exactly 56 bytes" is an active-probing signature. |
| Connections | One tunnel connection per user connection. | Many parallel connections to one IP is a DPI signal; every connection pays a TCP (+TLS, +HTTP upgrade) setup, which is very expensive through a CDN. |
| Transports | Only `tcp`. | No way through CDNs, nothing that looks like normal web traffic. |

## 2. Target architecture

```
 user TCP conn                                                     target
      │                                                               ▲
 [ forward listener ]  entry                               exit  [ tcp::connect ]
      │                                                               │
 [ Channel ] ── raw SecureStream   or   MuxStream (one of many) ──  [ Channel ]
      │                                                               │
 [ SecureStream ]   AEAD records, keys from handshake v2       [ SecureStream ]
      │                                                               │
 [ TunnelStream ]   tcp | ws | wss                             [ TunnelStream ]
      └──────────────────────────── network / CDN ────────────────────┘
```

Rules that keep the hot path fast:

- Everything stays **statically dispatched**: `TunnelStream` stays an enum (`Tcp`,
  `Ws`, `Wss`), `SecureStream<S>` and `MuxSession<S>` are generic over the stream.
- `relay()` does not change: `Channel` (raw or mux stream) implements
  `AsyncRead + AsyncWrite`, so `copy_bidirectional_with_sizes` keeps working.
- Encryption always sits **above** the transport. Behind a CDN, TLS terminates at the
  CDN, so `wss` alone does not hide the traffic from the CDN operator.
- Mux sits **above** encryption: one handshake and one key schedule per mux session,
  and mux headers are never visible on the wire.

## 3. Handshake v2 and key schedule

Replaces `src/auth.rs` (moved to `src/crypto/handshake.rs`). Same trust model (shared
token), adds forward secrecy and removes every fixed byte.

```text
psk      = BLAKE3-derive-key("kariz 2026-10 psk v2", token)
k_mask   = BLAKE3-derive-key("kariz v2 mask", psk)
k_tag    = BLAKE3-derive-key("kariz v2 tag",  psk)
mask(x)  = BLAKE3-keyed(k_mask, x) in XOF mode (keystream)

dialer -> acceptor
  nonce_c (16, random)
  hdr_c   (44) = [ ts (8) | eph_c (32, X25519) | cipher (1) | pad_len (2) | flags (1) ]
                 XOR mask("c" | nonce_c)
  pad_c   (pad_len random bytes, pad_len <= 512)
  tag_c   (32) = BLAKE3-keyed(k_tag, "c" | nonce_c | hdr_c | pad_c)
  [early] (optional) one AEAD record under k_early, see below

acceptor -> dialer
  hdr_s   (34) = [ eph_s (32) | pad_len (2) ] XOR mask("s" | nonce_c)
  pad_s   (pad_len random bytes)
  tag_s   (32) = BLAKE3-keyed(k_tag, "s" | transcript_c | hdr_s | pad_s)

shared    = X25519(eph_c, eph_s)
th        = BLAKE3(transcript_c | transcript_s)
k_c2s     = BLAKE3-derive-key("kariz v2 c2s", psk | shared | th)
k_s2c     = BLAKE3-derive-key("kariz v2 s2c", psk | shared | th)
k_early   = BLAKE3-derive-key("kariz v2 early", psk | nonce_c | hdr_c)
```

- The acceptor reads 60 bytes, unmasks, checks `pad_len <= 512`, reads the rest, checks
  the tag (constant time), then the timestamp window and the replay filter (both kept
  from v1).
- `cipher` is chosen by the dialer and covered by the tag, so it cannot be downgraded.
  The acceptor rejects ciphers not allowed by its own config.
- **Early data (0-RTT):** the dialer may append one record encrypted with `k_early`. It
  carries only the `Open` request (non-mux) or the first mux frames. Replay is blocked by
  the nonce filter. This keeps direct non-mux mode at one round trip, as in v1. Early data
  has no forward secrecy; everything after it does.
- Hello length is `92 + pad_len`, random per connection. Phase 6 reuses `pad_len` and
  `flags` for real traffic shaping; phase 2 only picks a small random padding.
- **Failed handshake:** never close immediately. Keep reading and discarding until a
  random deadline (5-30 s), then close; after 1 MiB stop reading but keep the connection
  open until the deadline, so no byte count triggers the close. Phase 6 replaces this with a real
  fallback; phase 2 only removes the obvious signature.
- `X25519` costs ~50 µs. In reverse mode it is paid when the pool / session is filled, and
  with mux once per session, so it is off the user-visible path in almost all setups.

## 4. Record layer (`SecureStream<S>`)

```text
record  = seal(k, n,   len (2, BE))   -> 2 + 16 bytes
        | seal(k, n+1, payload)       -> len + 16 bytes
len     <= 16383 (top 2 bits reserved for phase 6 padding records)
n       = 96-bit little-endian counter, per direction, starts at 0
ciphers = chacha20-poly1305 | aes-256-gcm
```

- Separate keys per direction, so counters never collide. Counter exhaustion closes the
  connection (unreachable in practice).
- Overhead: 34 bytes per 16 KiB record, about 0.2 %.
- `poll_write` seals up to one record per call into a reused buffer and keeps partially
  written ciphertext across `Pending`. `poll_read` opens records in place in a reused
  buffer (one copy out, unavoidable with `AsyncRead`).
- `auto` picks AES-256-GCM when the CPU has AES instructions
  (`is_x86_feature_detected!("aes")` / `is_aarch64_feature_detected!("aes")`),
  otherwise ChaCha20-Poly1305 (cheap VPS on older or ARM cores).
- `encryption = "none"` exists for benchmarking and for users who already run inside
  their own encrypted layer. It is never the default and `kariz check` / startup print a
  warning for it.

## 5. Mux (`kmux`)

Own protocol instead of the `yamux` crate: it is small, fits the tokio model directly
(no poll-based `Connection` driver), and phase 3 needs a datagram frame type in the same
session for UDP-over-stream.

### Frames (inside `SecureStream`, so not obfuscated)

```text
type (1) | stream_id (4, BE) | length (2, BE) | payload
```

| Type | Name | Payload |
|---|---|---|
| 0 | `SYN` | `Open` (same encoding as `proto.rs`) |
| 1 | `DATA` | bytes |
| 2 | `FIN` | none (half close) |
| 3 | `RST` | 1-byte reason (`dial_failed`, `refused`, `protocol`, ...) |
| 4 | `WINDOW` | u32 credit increment |
| 5 | `PING` / 6 `PONG` | 8 opaque bytes |
| 7 | `GOAWAY` | none; no new streams on this session |
| 8 | `DGRAM` | reserved for phase 3 (UDP flow id + packet) |

- Stream ids: odd ids are opened by the entry side. Even ids are reserved (future
  reverse forwards opened by the exit).
- **Optimistic open (0-RTT per stream):** the entry sends `SYN` and immediately starts
  sending `DATA`. The exit buffers up to the stream window while dialing the target; on
  failure it answers `RST(dial_failed)` and the entry closes the user connection. This
  removes the round trip that v1 pays per connection for the status byte.
- **Flow control:** per-stream credit window (profile defaults below), `WINDOW` sent when
  half of the window has been consumed. Every stream starts with a fixed 64 KiB of credit
  in each direction, so the two sides never need to agree on settings: each side raises
  its receive window to its own configured size with a `WINDOW` frame (the opener right
  after its `SYN`, the acceptor as soon as it sees the `SYN`). Sending beyond the credit
  is a protocol error that closes the session. No connection-level window: a slow reader only
  stalls its own stream, never the others. Receive buffers are allocated lazily, so idle
  streams cost only their bookkeeping.
- **Session task layout:** one reader task dispatches frames to per-stream queues; one
  writer task drains a bounded frame queue, prioritises control frames (`WINDOW`, `PONG`,
  `RST`) and **coalesces** several frames into one 16 KiB record before writing.
  Payloads use `bytes::Bytes` to avoid copies between tasks.
- **Liveness:** `PING` every `keepalive`; no `PONG` within `2 x keepalive` marks the
  session dead and resets its streams. This also keeps CDN WebSocket connections from
  idling out (Cloudflare drops idle WS after ~100 s).
- **Session manager:**
  - direct mode: the entry keeps `mux.connections` sessions to the exit, reconnecting
    with the same backoff as `pool_worker`;
  - reverse mode: the exit keeps `mux.connections` sessions to the entry (replaces the
    idle pool); the entry puts new streams on the live session with the fewest streams;
  - `mux.max_streams` per session; when all are full a new stream waits up to
    `handshake_timeout` and then fails the user connection.
  - optional `mux.max_lifetime_secs`: rotate sessions (send `GOAWAY`, open a new one,
    let the old one drain). Useful against long-lived-connection heuristics.

Mux is a per-tunnel switch, not a separate transport, because `ws` / `wss` need it much
more than `tcp` does. `transport = "tcpmux"` is kept as an alias for
`transport = "tcp"` + `mux.enabled = true`, matching the roadmap name.

## 6. WebSocket transports (`ws`, `wss`)

Own minimal RFC 6455 implementation (`src/transport/ws.rs`), not `tokio-tungstenite`:
`tungstenite` is message based (allocation and copy per message) and needs an adapter
to become `AsyncRead + AsyncWrite`. We only need binary frames on one connection.

**Client (dialer):**

- Connects to `tunnel.remote`, which may be a clean CDN IP, while `ws.host` / `tls.sni`
  carry the domain. Separating "where to connect" from "what name to present" is the most
  important CDN feature for users in Iran.
- Sends a browser-like upgrade request: `GET {path}`, `Host`, `User-Agent` (configurable,
  sane browser default), `Upgrade`, `Connection`, `Sec-WebSocket-Key`,
  `Sec-WebSocket-Version: 13`, plus optional extra headers.
- Masks every frame as RFC 6455 requires (CDNs enforce it); masking is done word-wise.

**Server (acceptor):**

- Parses the request with `httparse` (bounded: 8 KiB header limit, handshake timeout).
- Wrong path, wrong host, missing upgrade headers: answer a plain nginx-style `404`
  and close. Nothing Kariz-specific ever reaches an unauthenticated client.
- Takes the real client IP from `CF-Connecting-IP` / `X-Forwarded-For` for logs only.

**Framing:** outgoing binary frames of up to one record; incoming handles continuation
frames, `ping -> pong`, `close -> close + EOF`, rejects text frames, caps incoming frame
size (1 MiB).

**`wss`:** `rustls` + `tokio-rustls` with the `ring` provider (same crate as the AEAD and
X25519, and it builds cleanly for musl in phase 7).

- Server: `tls.cert` / `tls.key` PEM files; certificate files re-read when their mtime
  changes, so Let's Encrypt renewals need no restart.
- Client: `webpki-roots` verification by default; `tls.pin_sha256` for self-signed
  certificates; `tls.insecure = true` allowed with a loud warning. ALPN `http/1.1`.
- Known limitation: the rustls ClientHello does not look like a browser (JA3/JA4). Browser
  fingerprint mimicry (e.g. an optional BoringSSL backend) is deferred to phase 6.

*Status after 2.4:* `ws` is implemented as above (`src/transport/ws/`). Notes from the
implementation:

- The upgrade runs in the connection's own task (`Incoming::establish`), inside the same
  `handshake_timeout` as the tunnel handshake, so slow clients never block the accept loop.
- The request follows Chrome's header order (`Host`, `Connection`, `Pragma`,
  `Cache-Control`, `User-Agent`, `Upgrade`, `Origin`, `Sec-WebSocket-Version`,
  `Accept-Encoding`, `Accept-Language`, `Sec-WebSocket-Key`); `ws.headers` replace
  defaults of the same name in place and are otherwise appended.
- Rejections: `404` for a wrong path or host and for anything that is not a WebSocket
  upgrade, `400` for malformed or oversized (> 8 KiB) heads, both as nginx sends them
  (`Server: nginx`, `Date`, stock error page). The query string is ignored when
  matching the path.
- Outgoing data frames carry up to 128 KiB (one record-layer write batch fits in one
  frame); vectored writes are gathered into one frame. Incoming payloads are streamed
  into the caller's buffer, not buffered per frame.
- Client mask keys come from a wyrand generator seeded from the OS, not a syscall per
  frame; masking works on 8-byte words.
- A ping is answered with a pong on the next write or flush (control frames go between
  data frames, never inside one); mux pings keep that regular. Shutting the writer down
  sends a close frame; a received close frame reads as end of stream. Half close works
  end to end without a proxy; behind a CDN, mux (the default for `ws`) does not depend on
  it.
- Localhost throughput (`tests/tunnel.rs` throughput, release): `ws` without mux is on par
  with `tcp`; with mux 85-90 % of `tcp` + mux.

**Optional (step 2.6):** WebSocket early data. The first bytes the upper layer writes
(the hello) go base64url-encoded in `Sec-WebSocket-Protocol`, as Xray does. Saves one
round trip through the CDN, and lets the server answer `404` instead of `101` to any
upgrade whose hello does not verify.

## 7. Configuration

All new fields are optional; v0.1 configs still parse. Example (entry in Iran dialing the
exit through Cloudflare, direct mode):

```toml
role = "entry"
mode = "direct"
profile = "balanced"

[tunnel]
transport = "wss"                   # tcp | tcpmux | ws | wss
remote = "104.16.0.1:443"           # clean CDN IP, or a hostname
token = "..."
encryption = "auto"                 # auto | chacha20-poly1305 | aes-256-gcm | none

[tunnel.mux]
enabled = true                      # default: true for ws/wss, false for tcp
connections = 4
max_streams = 512
# max_lifetime_secs = 3600

[tunnel.ws]
path = "/api/v1/stream"
host = "tunnel.example.com"
# user_agent = "..."
# headers = { "Accept-Language" = "en-US,en;q=0.9" }

[tunnel.tls]
sni = "tunnel.example.com"
# pin_sha256 = "..."                # client, for self-signed certificates
# cert = "/etc/kariz/cert.pem"      # server
# key  = "/etc/kariz/key.pem"       # server
```

New validation rules: `wss` acceptor needs `tls.cert` + `tls.key`; `ws.path` starts with
`/`; mux values have lower and upper bounds; `tls.*` / `ws.*` only with a matching
transport; `pin_sha256` and `insecure` are mutually exclusive.

Profile defaults added to `Tuning`:

| | balanced | throughput | gaming |
|---|---|---|---|
| mux stream window | 256 KiB | 1 MiB | 64 KiB |
| mux connections | 4 | 8 | 2 |
| writer coalescing | on | on | off (flush per frame) |

`kariz check` prints the new settings and all warnings (no encryption, insecure TLS).

## 8. Code layout

```
src/crypto/mod.rs          Cipher, key schedule
src/crypto/handshake.rs    handshake v2 (replaces auth.rs, keeps ReplayFilter)
src/crypto/record.rs       SecureStream<S>
src/mux/frame.rs           frame encode/decode
src/mux/session.rs         MuxSession, MuxStream, reader/writer tasks
src/mux/manager.rs         N sessions, placement, reconnect, rotation
src/transport/ws.rs        HTTP upgrade + WsStream<S>
src/transport/tls.rs       rustls client/server config builders, cert reload
src/channel.rs             Channel = Raw(SecureStream<TunnelStream>) | Mux(MuxStream)
```

`entry.rs` / `exit.rs` keep their structure: `Source` gains mux variants, and the
handshake / open code moves behind one `open_channel()` / `accept_channel()` pair.

New dependencies: `ring`, `rustls`, `tokio-rustls`, `webpki-roots`, `httparse`,
`base64`, `bytes`. Dev: `rcgen` (test certificates).

## 9. Work breakdown

Each step is one PR, keeps CI green, and keeps both modes working.

| Step | Content | Done when |
|---|---|---|
| **2.0** Prep (done) | `Channel` abstraction, config sections (`mux`, `ws`, `tls`, `encryption`) parsed and validated but not yet used, test helper parameterised over transport / mux / mode. | No behaviour change, all v0.1 tests pass through the new helper. |
| **2.1** Crypto (done) | Handshake v2, `SecureStream`, early data, drain-on-failure, cipher `auto`. | Unit tests: roundtrip, tamper, wrong token both ways, replay, stale ts, downgrade attempt, record split across reads, counter per direction. E2E tests pass with encryption on (default) and `none`. Hello bytes pass a simple byte-distribution check (no fixed offsets). |
| **2.2** Mux core (done) | Frames, session, streams, flow control, ping, `GOAWAY`. Tested in isolation over `tokio::io::duplex`. | Tests: 1000 parallel streams, half close, `RST`, slow reader does not block a fast stream, window accounting, dead peer detected by ping. |
| **2.3** Mux integration (done) | Session manager in both modes, optimistic open, `tcpmux` alias, rotation. | E2E matrix `{reverse, direct} x {mux on, off}` green; unreachable-target test still closes the user connection; killing the exit mid-transfer resets streams and the entry reconnects. |
| **2.4** `ws` (done) | Upgrade client/server, frame codec, 404 on mismatch. | Codec tests with RFC 6455 vectors (masking, 16/64-bit lengths, fragmentation, control frames); E2E over `ws`; probe test: plain HTTP GET and wrong-path upgrade get a 404. |
| **2.5** `wss` | rustls, cert/key, SNI vs connect address, pin / insecure, cert reload. | E2E over `wss` with an `rcgen` certificate (pinned); wrong pin fails; cert reload test. |
| **2.6** CDN hardening | WS early data (optional flag), CI job with nginx as a stand-in CDN (`proxy_pass` with upgrade, idle timeout), manual Cloudflare checklist. | Tunnel survives nginx in the middle and idle periods longer than the proxy timeout. |
| **2.7** Release | Sample configs (`entry-wss-cdn.toml`, `exit-wss-cdn.toml`, ...), README / README_FA, ROADMAP update, benchmark table, version `0.2.0`. | Docs reviewed, tagged. |

2.4 depends only on 2.0, so it can be developed in parallel with 2.1-2.3.

## 10. Performance and resource targets

Measured with the existing `throughput` test (extended to the matrix) on the CI runner
and on a 1 vCPU VPS:

- Encryption on, single stream: at least 70 % of v0.1 plain `tcp` throughput.
- Mux on, single stream: at least 85 % of the same setup without mux.
  *Status after 2.2:* 60-64 % plain and 53-56 % encrypted, measured with
  `cargo test --release --lib mux_throughput -- --ignored --nocapture` (both ends and the
  echo on one 4-core machine, so it is CPU-bound). Profiling shows the mux-specific cost
  is the copy into and out of each stream (unavoidable with `AsyncRead`/`AsyncWrite`)
  plus extra task hops; to be revisited in 2.3 with end-to-end numbers (lock-free
  connection split, fewer hops).
  *Status after 2.3:* reading and writing now run in separate tasks over independent
  connection halves (no lock between encryption and decryption), and the mux relay
  hands `Bytes` through without user-space copies. Isolated: 67-71 %. End to end
  (`tests/tunnel.rs` throughput, both sides on one 4-core machine): about 70 % with the
  default 256 KiB window, about 80 % with a 4 MiB window. Mux stays off by default for
  `tcp`; window auto-tuning is a candidate follow-up.
- 100 idle mux streams: under 2 MiB extra RSS.
- Idle process RSS stays under 10 MiB.
- Stream open latency in reverse + mux: no extra round trip over the target dial
  (optimistic open).

## 11. Compatibility

- **Wire format v2 is not compatible with v0.1.** Both sides must be upgraded together;
  a mismatched pair fails the handshake and logs "handshake failed", like a wrong token.
- Config files from v0.1 keep working; they get encryption automatically.

## 12. Risks

| Risk | Mitigation |
|---|---|
| Own mux has subtle flow-control or close bugs. | Isolated tests over `duplex` first (2.2), randomised stress test, optional `cargo fuzz` targets for the frame and ws decoders. |
| CDN quirks (frame splitting, header rewriting, idle kills). | Codec accepts any legal fragmentation; ping below 100 s; nginx CI job; manual Cloudflare checklist. |
| rustls TLS fingerprint is recognisable. | Documented; fingerprint work is in phase 6. Behind a CDN the DPI only sees a connection to the CDN, which reduces the impact. |
| Memory growth with many streams. | Lazy buffers, `max_streams`, bounded frame queues, RSS target in CI. |
| Early data replay. | Nonce + timestamp replay filter; early data carries only the open request. |

## 13. Decisions to confirm

1. **Mux default for `tcp`:** proposed off (keeps v0.1 behaviour and maximum
   single-connection throughput), on for `ws` / `wss`.
2. **Own `ws` codec** instead of `tokio-tungstenite`: proposed yes, for speed and
   direct `AsyncRead`/`AsyncWrite`.
3. **Forward secrecy (X25519)** in the handshake: proposed yes.
4. **`encryption = "none"`:** proposed allowed, never default, always warned.
5. **Primary CDN for testing:** Cloudflare; ArvanCloud as the second target for
   setups where the CDN sits in front of the Iran server (reverse mode).
