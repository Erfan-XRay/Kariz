# Status

A running Kariz answers `status` on its control socket, on both sides: whether the other
side is connected, the round-trip time, the last error, and what goes through each
forwarded port. `kariz status` shows it on the server; the web panel reads it every
second.

## `kariz status`

On either server, while the tunnel runs:

```bash
kariz status -c /etc/kariz/main.toml
```

```text
  tunnel main · entry · reverse · tcpmux · up 1 d 2 h
  exit side  connected · 2 of 2 sessions · rtt 38 ms · no errors
  forward    [::]:443 -> 127.0.0.1:443 (tcp)       12 open   ↑ 1.2 Mbit/s  ↓ 41.8 Mbit/s
             [::]:2053 -> 127.0.0.1:53 (tcp+udp)    0 open   3 flows  ↑ 12 kbit/s  ↓ 30 kbit/s
```

It reads the counters twice, one second apart, to show rates.

| Option | Meaning |
|---|---|
| `-c, --config FILE` | the tunnel's config (default `/etc/kariz/config.toml`) |
| `--watch` | redraw every second until Ctrl+C |
| `--json` | print the daemon's document as is (one reading, no rates) |

Exit codes: 0 when the daemon answered, 3 when it is not running (nothing listens on the
control socket), 1 for other errors. The manager script shows the same with
`kariz-manager status [NAME]`.

## The control socket

Each side opens a Unix socket next to its config: `/etc/kariz/main.sock` for
`/etc/kariz/main.toml`, or `[control] socket`. Only its owner can open it (mode 0600).
There is none on Windows.

One request per connection, one line:

```text
client -> daemon   status\n
daemon -> client   ---\n<JSON document>\n
```

or `! <error text>\n`. The request is cheap (it reads counters and never touches the
tunnel), and any number of clients may ask at once.

## The document

```json
{
  "status_version": 1,
  "kariz": "0.7.0",
  "role": "entry",
  "mode": "reverse",
  "transport": "tcpmux",
  "profile": "balanced",
  "uptime_secs": 93600,
  "peer": {
    "connected": true,
    "sessions": 2,
    "sessions_wanted": null,
    "connected_secs": 3600,
    "rtt_ms": 38.4,
    "handshakes_ok": 14,
    "handshakes_failed": 0,
    "last_error": null
  },
  "totals": {
    "bytes_up": 104857600, "bytes_down": 2147483648,
    "tcp_open": 12, "tcp_total": 5120, "udp_flows": 3, "udp_total": 88
  },
  "forwards": [
    {
      "listen": "[::]:443", "target": "127.0.0.1:443", "protocol": "tcp",
      "tcp_open": 12, "tcp_total": 5120, "udp_flows": 0, "udp_total": 0,
      "bytes_up": 104857600, "bytes_down": 2147483648, "open_failures": 0
    }
  ]
}
```

Readers must ignore fields they do not know: later versions add fields and raise
`status_version` only for changes that break readers.

### Top level

| Field | Meaning |
|---|---|
| `status_version` | the document's format version (1) |
| `kariz` | the daemon's version |
| `role`, `mode`, `transport`, `profile` | as in its config |
| `uptime_secs` | since the daemon started |

### `peer`: the other side

| Field | Meaning |
|---|---|
| `connected` | with mux (and QUIC): at least one session is up. Without mux: the last attempt to connect or authenticate succeeded |
| `sessions` | live sessions; `null` without mux |
| `sessions_wanted` | how many the dialing side keeps up (`mux.connections`); `null` on the accepting side and without mux |
| `connected_secs` | how long the oldest live session has been up (without mux: since the last success); `null` while not connected |
| `rtt_ms` | round-trip time to the other side, averaged over the sessions: from mux pings (smoothed, 7/8 old + 1/8 new) or QUIC's own estimate. `null` without mux, and until the first ping is answered |
| `handshakes_ok`, `handshakes_failed` | tunnel handshakes since start, both directions. On a listening side, failures include connections from scanners |
| `last_error` | `{ "secs_ago": 12, "text": "authentication failed" }` for the last failed connect or handshake, or `null`. A refused or unreachable other side shows at once; a token mismatch takes longer: the listening side keeps a failed connection open for a random 5-30 s before it gives up (so probes learn nothing), and the dialing side reports it when its handshake times out (`tuning.handshake_timeout_secs`, 10 s) |

### `totals` and `forwards`

On the entry side, one entry per `[[forward]]` rule, in config order, and their sum in
`totals`. `up` is from users towards the targets, `down` is back to the users.

| Field | Meaning |
|---|---|
| `tcp_open`, `tcp_total` | user connections open now, and since start |
| `udp_flows`, `udp_total` | UDP flows (one per client address) open now, and since start |
| `bytes_up`, `bytes_down` | since start, counted as data flows (not when a connection ends) |
| `open_failures` | user connections and flows that could not be carried: no session to the exit side in time, or the exit could not reach the target |

Byte counts are totals; a reader gets rates by asking twice. UDP bytes of a busy flow are
added in batches of 64 packets or at least once a second, so a UDP rate read over a
one-second window can lag by up to a second.

On the exit side `forwards` is empty and `totals` holds the bytes to and from all
targets (connection counts are 0), and there is an `exit` object:

| Field | Meaning |
|---|---|
| `exit.streams_total` | user connections and UDP flows served since start |
| `exit.dial_failures` | targets that could not be reached |
| `exit.last_dial_error` | `{ "secs_ago", "text" }` with the target and the reason, or `null` |
