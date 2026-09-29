# Phase 10 plan: status for the panel

Target release: **v0.7.0**. Scope from [PHASE9.md](PHASE9.md#3-the-panel-plan-phases-10-to-13):
counters, `status` on the control socket of both sides, `kariz status`,
`kariz check --json`.

## 1. Why

The panel (phases 11 to 13) shows each tunnel live: traffic per forwarded port, open
connections, whether the other side is connected, the round-trip time, the last error.
Today a running Kariz says none of that except in its log lines, and only the entry side
has a control socket, which knows one request (`speedtest`). The panel's agent will read
a tunnel's state from its control socket; this phase makes that state exist and puts it
there. It is useful on its own too: `kariz status` on a server answers "is it up, and how
much goes through it" without reading logs.

Nothing in this phase changes the wire between the two sides: v0.7 works with v0.6,
v0.5 and v0.4 peers, every transport.

## 2. Counters (10.1)

A `Stats` value per running side, shared by the tasks that already exist. Everything is a
relaxed atomic, updated where the work already happens; nothing new runs in the
background.

**Per `[[forward]]` rule (entry side):**

| Counter | Where it is counted |
|---|---|
| `tcp_open`, `tcp_total` | `accept_users`: up when a user connection is taken, down when its relay ends |
| `udp_flows` (open), `udp_total` | `udp::serve`: when a flow starts and ends |
| `bytes_up` (users to targets), `bytes_down` | the relays, per chunk (see below) |
| `open_failures` | a channel that could not be opened (no session, the exit could not reach the target) |

**Per side:**

| Counter | Meaning |
|---|---|
| `started` | when the daemon started (uptime) |
| `sessions`, `sessions_wanted` | live sessions to the other side, and how many the dialing side keeps (`mux.connections`); without mux, idle pooled links (reverse) or none (direct) |
| `connected_since` | when the first of the current sessions came up; empty while none is up |
| `handshakes_ok`, `handshakes_failed` | tunnel handshakes, in either direction |
| `last_error` | the last failure to connect or authenticate, with its time: "authentication failed", "connection refused", ... |
| `rtt` | the latest round-trip time to the other side |
| exit only: `streams_total`, `dial_failures`, `last_dial_error` | streams served, targets that could not be reached, and the last such error (target and reason) |

**Bytes as they flow, not when a connection ends.** A connection that stays open for
hours would otherwise show nothing until it closes. `relay_stream` adds each chunk to the
rule's counters as it goes (one atomic add per chunk of up to `buffer_size`, 16 to 256
KiB). `relay` (plain `tcp`, no mux) wraps the user socket in a small counting reader and
writer, for the same effect. UDP adds per packet into a per-flow total that is folded into
the rule's counter every 64 packets and when the flow ends, so busy flows on many cores do
not all write the same cache line.

**Round-trip time without a wire change.** A mux ping already carries an 8-byte sequence
number that the peer echoes in its pong. The session keeps the send time of the latest
ping; when the matching pong arrives, the difference is the RTT (kept as a smoothed value,
like TCP's SRTT: 7/8 old + 1/8 new). Old peers answer pings the same way, so this works
against v0.4 to v0.6. QUIC sessions report quinn's own RTT estimate. Plain `tcp` and
`ws` without mux have no pings: no RTT there.

**Cost.** Measured with the throughput benchmark (A/B against `main`, alternating rounds,
first round thrown away, as in PHASE8) and the UDP packet benchmark. Kept only if both
lose less than 1 %; otherwise the per-chunk and per-packet adds get batched further.

*Status after 10.1:* `src/stats.rs` holds the counters; the entry, the exit, both relays
and the UDP flows fill them; `maintain` and every accept loop record sessions,
handshakes and failures. A UDP flow adds its bytes to the rule every 64 packets, at
least once a second, at once for its first packet (so a lone DNS query shows), and when
it ends. Mux sessions keep a smoothed RTT from their pings (the first ping goes out as a
session starts); QUIC sessions report quinn's. Tests: the counters, the batching, RTT
from pings (and a pong with a wrong number ignored), the counting relay.

Measured on the Windows development machine, release builds, 4 alternating rounds of
`main` and the branch, the first thrown away, mean of the other three:

| Benchmark | Change |
|---|---|
| Throughput, 28 setups | mean +1.2 %, median +1.6 % (single setups -12.6 % to +16.7 %: run-to-run noise, both ways) |
| UDP packets per second, 12 cases | mean -0.8 %, median -0.6 % (worst -3.8 %, tcpmux 100-byte) |

Within the noise of the machine and under the 1 % bound on average, so the counters stay
as they are. The UDP benchmark is bound by its clients here (about 110,000 packets per
second in every setup), so it would not show a small cost per packet; the CI benchmark
on Linux (10.4) is the better check.

## 3. `status` on the control socket (10.2)

**Both sides get a control socket.** The exit side opens one too, at the same default
place (`/etc/kariz/<name>.sock` for `/etc/kariz/<name>.toml`, owner only). On the exit,
`speedtest` answers "run the speed test on the entry side".

**The request** is one line, as today:

```text
client -> daemon   status\n
daemon -> client   ---\n<JSON>\n
```

JSON rather than TOML (which `speedtest` answers with): the panel's agent and scripts read
it as is, and `kariz status --json` prints the same document. A version number at the top
lets later releases add fields without breaking readers (readers ignore fields they do not
know).

```json
{
  "status_version": 1,
  "kariz": "0.7.0",
  "role": "entry", "mode": "reverse", "transport": "tcpmux", "profile": "balanced",
  "uptime_secs": 86400,
  "peer": {
    "connected": true,
    "sessions": 2, "sessions_wanted": 2,
    "connected_secs": 3600,
    "rtt_ms": 38.4,
    "handshakes_ok": 14, "handshakes_failed": 0,
    "last_error": null
  },
  "totals": { "bytes_up": 0, "bytes_down": 0, "tcp_open": 0, "tcp_total": 0, "udp_flows": 0, "udp_total": 0 },
  "forwards": [
    { "listen": "[::]:443", "target": "127.0.0.1:443", "protocol": "tcp",
      "tcp_open": 12, "tcp_total": 5120, "udp_flows": 0, "udp_total": 0,
      "bytes_up": 104857600, "bytes_down": 2147483648, "open_failures": 0 }
  ]
}
```

`last_error` is `{ "secs_ago": 12, "text": "authentication failed" }` or `null`. Byte
counts are totals since start; a reader gets rates by asking twice (the panel does, every
second). The document is the full format reference, in `docs/status.md`.

**Cheap and safe to ask often.** Answering reads the atomics and the rules; it does not
touch the tunnel. The panel asks every second; many clients at once are fine (unlike
`speedtest`, which runs one at a time).

*Status after 10.2:* both sides open the control socket; the exit's refuses speed tests
with "run the speed test on the entry side". `status` answers `---` and the document on
one line; `kariz::control::status` is the client. `docs/status.md` describes every
field. Tests: the request parser, a status answer read back into a `Status`, the exit's
refusal, and (Linux, in CI) two tunnel tests: traffic counted exactly on both sides for
tcpmux reverse, tcp direct without mux, quic reverse and kcp direct (3 MiB each way and a
UDP packet, round trips measured with mux), and a token mismatch showing up as "not
connected" with the last error on both sides.

## 4. `kariz status` and `kariz check --json` (10.3)

**`kariz status [-c CONFIG]`** asks the running daemon of that config and shows, on
either side:

```text
  tunnel main · entry · reverse · tcpmux · up 1 d 2 h
  exit side  connected · 2 of 2 sessions · rtt 38 ms · last error none
  forward    [::]:443 -> 127.0.0.1:443 (tcp)       12 open   ↑ 1.2 Mbit/s  ↓ 41.8 Mbit/s
             [::]:2053 -> 127.0.0.1:53 (tcp+udp)    0 open   3 flows  ↑ 12 kbit/s  ↓ 30 kbit/s
```

Rates come from two readings one second apart. `--watch` redraws every second until
Ctrl+C; `--json` prints the daemon's document unchanged (one reading, no rates). If the
daemon is not running it says so, with the systemd unit to look at, and exits with 3 (1
stays "error").

**`kariz check --json [-c CONFIG]`** prints one JSON document instead of the summary:

```json
{ "ok": true, "role": "entry", "mode": "reverse", "transport": "tcpmux", "...": "...",
  "warnings": ["..."] }
{ "ok": false, "error": "tunnel.token must be at least 16 characters ...",
  "key": "tunnel.token", "line": 7, "column": 9 }
```

`key` names the setting at fault when the message starts with one (every validation
message does); `line` and `column` come with TOML syntax errors. The panel puts the error
under the right field. The exit code stays 0 / 1.

**`-c -` reads the config from standard input**, for `check` only: the panel's agent
checks a config before writing it, without a temporary file.

**The manager script** gets a Status entry (and `kariz-manager status [NAME]`) that runs
`kariz status` for one tunnel or all of them.

## 5. Work breakdown

| Step | Content | Done when |
|---|---|---|
| **10.0** Plan (done) | This document. | |
| **10.1** Counters (done) | `Stats`, counting in entry, exit, relays and UDP; mux RTT from pings; QUIC RTT. | Unit tests for every counter; benchmark A/B within 1 %. |
| **10.2** `status` (done) | Control socket on the exit side too; the `status` request; `docs/status.md`. | Tests: a tunnel in memory answers `status` on both sides with the right numbers. |
| **10.3** CLI | `kariz status` (`--watch`, `--json`), `kariz check --json`, `-c -`, manager Status. | Tests for the output and the error cases; manager tty test covers Status. |
| **10.4** Release | README / docs, CHANGELOG, `0.7.0`. | CI green; release v0.7.0 with three archives. |
