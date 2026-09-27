# Kariz

**English** | [فارسی](README_FA.md)

Kariz (کاریز, the ancient Persian underground water channel) is a tunnel core for
linking servers together. It is written in Rust and built around three goals:

1. **Low resource usage**: no garbage collector, small memory footprint, suitable for cheap VPS.
2. **DPI resistance**: pluggable transports and camouflage, so a blocked method can be swapped without touching the core.
3. **Maximum speed**: statically dispatched hot path, tuned sockets, per-use-case profiles.

> Status: **early development (v0.2 in progress).** The `tcp`, `tcpmux`, `ws` and `wss`
> (WebSocket, plain or over TLS) transports are available, with an encrypted,
> forward-secret record layer (`tunnel.encryption`, default `auto`).
> The wire format changed after v0.1.0: upgrade both servers together.
> See [docs/ROADMAP.md](docs/ROADMAP.md) for the plan, and [docs/CDN.md](docs/CDN.md) for
> running behind a CDN such as Cloudflare.

## Concepts

| Term | Meaning |
|---|---|
| **entry** | The public server users connect to (for example in Iran). Owns the `[[forward]]` rules. |
| **exit** | The server that connects to the real targets (for example abroad). |
| **reverse** mode | The exit side dials the entry side and keeps a pool of idle connections ready. |
| **direct** mode | The entry side dials the exit side for each user connection. |

Both sides authenticate each other with a shared token. The token never goes over the wire;
see [`src/auth.rs`](src/auth.rs).

## Quick start

```bash
cargo build --release
./target/release/kariz token          # generate a shared token
```

Put the token in both configs (samples in [`configs/`](configs)), then:

```bash
# on the entry server
kariz run -c configs/entry-reverse.toml
# on the exit server
kariz run -c configs/exit-reverse.toml
```

`kariz check -c <file>` validates a config and prints a summary.
A systemd unit is in [`systemd/kariz.service`](systemd/kariz.service).

## Profiles

| Profile | Use |
|---|---|
| `balanced` | Default. |
| `throughput` | Large buffers for bulk transfer. |
| `gaming` | Small buffers, shorter keepalive, lowest latency. |

Any value can be overridden in the `[tuning]` table.

## Development

```bash
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo test --release --test tunnel -- --ignored --nocapture   # localhost throughput
```
