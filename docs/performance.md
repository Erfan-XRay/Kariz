# Performance

## Localhost

User → entry → exit → echo server, 256 MiB echoed, both sides and the echo server on one
4-core Xeon (2.1 GHz) VM. Everything shares the CPU, so this compares setups. A real
link between two servers is usually limited by the network first.

| Setup | Mbit/s each way |
|---|---|
| `tcp`, no encryption | 5,200-6,600 |
| `tcp`, AES-256-GCM | 3,800-4,000 |
| `tcp`, ChaCha20-Poly1305 | 2,100-2,800 |
| `tcpmux`, AES-256-GCM | 3,200-3,300 |
| `ws` + mux, AES-256-GCM | 2,800-3,100 |
| `wss` + mux, AES-256-GCM + TLS | 2,300-2,500 |

On a GitHub Actions runner (2 vCPU): `tcpmux` 2,960-3,420, `quic` 810-950, `kcp` 600-670
Mbit/s. QUIC and KCP handle every packet in user space.

UDP through `tcpmux` (8 clients, 32 packets in flight each): about 140,000 packets per
second each way. An idle tunnel adds about 90 µs to a UDP round trip.

Memory (RSS, release build): about 5-6 MiB per side idle over TCP transports, 6.5-7.5 MiB
over `quic` / `kcp`. 100 idle user connections over mux add under 1 MiB; 1,000 idle UDP
flows about 3-4.5 MiB.

## Over a lossy path

The tests carry a link emulator: delay, random or bursty loss, and a 50 Mbit/s
bottleneck with a 50 ms queue. Its TCP side models the sender's TCP, so loss slows
`tcpmux` as on a real path. At a 60 ms round trip, with loss in both directions:

| Transport | Download at 0 / 1 / 5 % loss |
|---|---|
| `tcpmux` | 48.1 / 2.3 / 0.9 Mbit/s |
| `quic` (Cubic) | 47.9 / 2.7 / 1.0 Mbit/s |
| `quic` (BBR) | 47.5 / 47.4 / 45.7 Mbit/s |
| `kcp` | 31.4 / 28.5 / 22.5 Mbit/s |
| `kcp`, FEC 10 / 3 | 25.4 / 26.1 / 26.6 Mbit/s |

- **Loss-based congestion control collapses under random loss**, TCP and QUIC Cubic
  alike. `quic` with BBR and `kcp` keep their speed.
- **BBR fills the path's queue.** During a download, about half of the UDP packets
  sharing its connection were lost, so Cubic stays the default.
- **KCP's default window (1024 packets) overfills small paths.** A window near
  bandwidth x RTT / 1300 packets queues less, but keeps less speed under loss.

## Tuning for speed

1. **Choose the transport for the path:** `tcp` / `tcpmux` on clean paths, `kcp` or
   `quic` with BBR on lossy ones ([Transports](transports.md)).
2. **Use `profile = "ultraspeed"`,** or raise `tunnel.mux.stream_window`. One stream
   moves at most one window per round trip, so a stream needs about bandwidth x RTT
   (100 Mbit/s at 100 ms: 1.25 MB).
3. **Leave `encryption = "auto"`.** It picks AES-256-GCM on CPUs with AES instructions,
   the fastest cipher there. `none` is faster still but only authenticates.
4. **Add connections for many parallel users:** `tunnel.mux.connections` spreads streams
   over more TCP connections, each with its own congestion window.
5. **For many connections,** raise the open-file limit (the systemd unit sets
   `LimitNOFILE=1048576`).

## Running the benchmarks

```bash
cargo test --release --test tunnel throughput -- --ignored --nocapture     # localhost
cargo test --release --test tunnel udp_ -- --ignored --nocapture           # UDP rates
cargo test --release --test tunnel lossy_link -- --ignored --nocapture     # lossy path
cargo test --release --test tunnel game_traffic -- --ignored --nocapture   # games, ~45 min
scripts/rss.sh tcpmux 100                                                  # memory
```

`KARIZ_BENCH_ONLY=kcp` limits `lossy_link` and `game_traffic` to rows whose name
contains `kcp`. `KARIZ_BENCH_LOSS=1` runs one loss rate.
