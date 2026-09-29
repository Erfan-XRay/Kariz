# Speed test

`kariz speedtest` measures what a tunnel really gives you: download and upload speed,
latency (idle and under load), and UDP loss and jitter. It sends its test traffic **through
the running tunnel**, over the same transport, encryption, mux and profile as your
users' traffic, so the numbers are the tunnel's, not the raw network's.

```bash
kariz speedtest -c /etc/kariz/main.toml
```

With the manager script: `kariz-manager speedtest main`, or option 6 of its menu.

```text
  ▸ tcpmux · ultraspeed   entry · direct mode
  ▸ 4 streams × 10 s each way

  ↓ download        612.3 Mbit/s   best second 655.0 Mbit/s
  ↑ upload          587.1 Mbit/s   best second 640.2 Mbit/s

  ◆ latency          median        p99     jitter
    idle            61.2 ms    64.0 ms     1.1 ms
    during download 79.0 ms   121.0 ms     6.2 ms
    during upload   72.4 ms   108.7 ms     5.0 ms

  ✦ UDP   0.4 % lost (2 of 320)   median 62.1 ms · p99 70.3 ms · jitter 1.8 ms
```

## Run it on the entry side

The entry is the server users connect to. It works in **both modes**, reverse and direct,
because the test runs inside the entry's own daemon: `kariz speedtest` connects to the
running daemon over a local socket and asks it to run the test through its live sessions.
So the tunnel has to be up (`kariz run`, or its service), and the command needs the same
config file as the daemon.

On the exit side the command explains this and stops.

## What it measures

| Part | How |
|---|---|
| **Latency, idle** | a small message every 100 ms for 2 s, each echoed by the exit |
| **Download** | several streams (`--streams`, default 4) on which the exit sends data as fast as it can, for `--seconds` (default 10). The first second is not counted (slow start) |
| **Upload** | the same, the other way |
| **Latency under load** | one more stream pings during the download and during the upload. A large rise is bufferbloat: queues in the path that make interactive traffic wait |
| **UDP** | game-like packets (128 bytes, 64 per second, 5 s) through the datagram path that UDP forwarding uses, echoed by the exit: loss, median, p99 and jitter. Needs mux; without it the report says so |

Loss is coloured: green under 1 %, yellow up to 5 %, red above.

## Options

| Option | Meaning |
|---|---|
| `-c, --config FILE` | the tunnel's config (default `/etc/kariz/config.toml`) |
| `--seconds N` | length of the download and upload phases, 1-60 (default 10) |
| `--streams N` | parallel streams in those phases, 1-16 (default 4) |
| `--no-udp` | skip the UDP test |

A whole test takes about `2 + 2 x seconds + 7` seconds.

## Reading the numbers

- **One test is one sample.** On a shared path the result depends on the load of that
  moment; run it a few times and look at the middle.
- **Streams matter.** One stream is limited to one window per round trip (see
  [Profiles](profiles.md)); several show what the path itself can carry. If 1 stream is
  much slower than 4, raise `tunnel.mux.stream_window` or use `profile = "ultraspeed"`.
- **Latency under load close to idle is good.** A jump of 100 ms or more means a queue
  fills up. The gaming profile keeps its windows small for this reason.
- **UDP loss is not a bug.** UDP over `quic` and `kcp` is lost when the path loses it
  (over `kcp` with FEC, most losses are rebuilt). Over TCP-based transports nothing is
  lost, but a loss shows as latency.

## Good to know

- **It uses real bandwidth**, in both directions, for the length of the test. It only
  produces and reads data on the exit: nothing is dialed and nothing is stored.
- **One test at a time** per daemon; a second request is refused until the first ends.
- **The exit must be v0.6 or newer.** An older exit cannot answer the test streams, and
  the command says so. `tunnel.speedtest = false` on the exit turns the answering off.
  The exit serves at most 32 test streams at once and ends each one after two minutes.
- **The control socket** is a Unix socket next to the config file
  (`/etc/kariz/main.toml` gives `/etc/kariz/main.sock`), readable only by its owner. Both
  sides have one since v0.7, for `kariz status`; the exit's answers no speed tests.
  `[control] socket = "..."` puts it elsewhere. If it cannot be made, the tunnel runs on
  without it and logs a warning. It exists on Linux only.
