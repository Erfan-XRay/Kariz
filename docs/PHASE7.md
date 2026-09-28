# Phase 7 plan: stealth profile

Target release: **v0.6.0**. Scope: a `stealth` profile that hides the tunnel's size
and timing patterns, and an active-probe fallback that makes a listening port behave
like an ordinary web server to anyone without the token. Also user-settable mux
settings (7.1, done).

Out of scope: making the TLS ClientHello look like a browser's (decided with the user:
not needed), and the UDP transports (`kcp` is already silent to probes; QUIC's
handshake is quinn's).

## 1. Why, and what for

The contents are already opaque: the hello has no fixed bytes or length, records are
AEAD with encrypted lengths, and a failed handshake is drained rather than closed. What
still shows is **shape**:

- **Sizes.** Records follow the data. An interactive session gives many small records,
  a download a run of full ones. Record sizes, and so packet sizes, carry the traffic's
  pattern.
- **Timing.** Writes go out when the data comes. Bursts and silences line up with what
  the user does.
- **Probes.** A connection that fails the handshake gets silence (plain TCP) or an
  nginx-style `404` (WebSocket). Silence on a port that accepted a connection is itself
  unusual. A real web server would answer.

This phase addresses each, at a cost in bandwidth and latency that the profile makes
explicit and the other profiles do not pay.

## 2. Mux settings (7.1, done)

*Status after 7.1:* `[tunnel.mux]` also takes `coalesce`, `ping_interval_secs` (default
`tuning.keepalive_secs`; QUIC's keep-alive follows it too), `datagram_buffer`,
`datagram_queue` and `notsent_lowat` (0 turns it off). Each defaults to the value it had
before, has bounds, and is shown by `kariz check`. The initial window and the frame size
stay fixed: they are part of the wire format.

## 3. Record padding (7.2)

- **Padded records.** A record's plaintext becomes `payload | padding`, with the payload
  length inside the encrypted header. The header's two reserved bits (always zero until
  now) mark a padded record, so an unpadded record keeps today's format.
- **Sizes from a distribution, not from the data.** The writer rounds each record up to
  a size drawn from a configured distribution: `none` (today), `buckets` (a few fixed
  sizes, so every record is one of them), or `random` (uniform up to a cap). Large
  writes are also split at drawn sizes, so a download does not show as a run of maximum
  records.
- **Negotiation.** A hello flag announces padding. Both sides must agree, like `mux`: a
  mismatch fails the handshake with a clear error. Peers without the flag (v0.5) get
  today's records.
- **Cost.** Measured in 7.5: bytes on the wire per payload byte, and throughput on
  localhost and on the emulated link.

## 4. Timing (7.3)

- **Bounded write jitter.** Under the stealth profile the mux writer may hold a write
  for a short random delay (a few milliseconds, capped), gathering frames so that
  write times depend less on when the data came.
- **Idle cover traffic.** Optional `PAD` frames on an idle session, at a low, randomised
  rate with a byte cap per minute. A peer ignores them (they are a new mux frame type,
  sent only when negotiated).
- Both are off outside the stealth profile. Their latency cost is measured in 7.5 and
  shown by `kariz check`.

## 5. Active-probe fallback (7.4)

- **`[tunnel.fallback] target = "127.0.0.1:8080"`** (listening side): a connection that
  does not complete the tunnel handshake is handed to this target instead of being
  drained. The bytes already read are passed on first, then data is relayed both ways,
  so the client talks to the target as if directly.
- **Per transport:**
  - `tcp` / `tcpmux`: the raw connection is relayed to the target.
  - `ws` / `wss`: requests that are not a valid upgrade on the tunnel path go to the
    target over HTTP, in place of today's built-in `404` page. With `wss`, TLS ends at
    Kariz, which then acts as a reverse proxy to the target.
  - `kcp` stays silent; `quic` is unchanged.
- **Timing of the decision.** The handshake decides after the hello (or the upgrade
  request) is read, within the handshake timeout. A client that sends nothing is
  relayed after that timeout, as a slow client of the target would be.
- Without `[tunnel.fallback]`, behaviour is today's (drain, or the `404` page).

## 6. The `stealth` profile (7.5)

A fourth profile turning on padding (`buckets`), bounded write jitter and idle cover
traffic, on the TCP-based transports. The profile's concrete values come from
measurements: packet size histograms before and after, bytes of overhead, throughput,
and the latency added to interactive traffic. Every part can also be set on its own in
another profile.

## 7. Configuration

```toml
profile = "stealth"

[tunnel]
transport = "wss"

[tunnel.stealth]                  # all optional; the profile sets the defaults
padding = "buckets"               # none | buckets | random
# padding_max = 1400              # random: largest padded record
jitter_ms = 5                     # writes held up to this long (0: off)
cover = true                      # PAD frames on idle sessions
# cover_max_bytes_per_min = 65536

[tunnel.fallback]                 # listening side
target = "127.0.0.1:8080"         # a real web server on this machine
```

## 8. Tests

- Padding: every record size is one the distribution allows; split writes arrive
  whole; tampering with the padding flag or length fails authentication; a v0.5 peer
  and a mismatch fail with clear errors.
- Timing: jitter never exceeds its cap; cover traffic stays within its byte budget and
  stops when data flows.
- Fallback: for each transport, a client without the token gets from Kariz exactly what
  it gets from the target directly (bytes compared), including requests split across
  packets and a client that sends nothing.
- E2E rows with the stealth profile on `tcp`, `tcpmux`, `ws`, `wss`.

## 9. Work breakdown

| Step | Content | Done when |
|---|---|---|
| **7.0** Plan | This document; roadmap updated. | Reviewed with the user. |
| **7.1** Mux settings (done) | `coalesce`, `ping_interval_secs`, `datagram_buffer`, `datagram_queue`, `notsent_lowat`. | Defaults unchanged; bounds; `kariz check`. |
| **7.2** Record padding | Padded records, size distributions, hello flag. | Tests of section 8; v0.5 compatibility without padding. |
| **7.3** Timing | Write jitter, idle `PAD` frames. | Caps hold; no change outside the profile. |
| **7.4** Active-probe fallback | `[tunnel.fallback]` for `tcp`, `tcpmux`, `ws`, `wss`. | Probes get the target's own answers. |
| **7.5** Stealth profile | Profile values from measurements; size histograms, overhead, latency. | Table in this document. |
| **7.6** Release | README / README_FA, samples, CHANGELOG, `0.6.0`. | Release tagged. |

## 10. Decisions

1. **No TLS ClientHello mimicry** (user decision). It would need a TLS stack other than
   rustls and is the costliest part to keep up to date.
2. **Padding and timing are negotiated and off by default.** They cost bandwidth and
   latency, which the other profiles are built to avoid.
3. **The fallback relays to a real server rather than imitating one.** An imitation
   (like today's `404` page) can be told apart by anyone who compares it to the real
   thing. A relay answers as that server does, because it is that server.
