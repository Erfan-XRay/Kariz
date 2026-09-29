# Kariz documentation

For Kariz **v0.9.0**. Start with [Getting started](getting-started.md); come back to the
reference pages when you need a specific setting.

| Page | What it covers |
|---|---|
| [Getting started](getting-started.md) | Install, a token, a first tunnel in reverse or direct mode, systemd |
| [The manager script](manager.md) | One-line install, a menu, and commands to add and run tunnels |
| [Configuration reference](configuration.md) | Every setting, its default and its limits |
| [Speed test](speedtest.md) | `kariz speedtest`: speed, latency and UDP through the live tunnel, in either mode |
| [The web panel](panel.md) | Install the panel, sign in, connect servers with agents, what it shows |
| [Status](status.md) | `kariz status`: connection, round trip, last error and traffic per port, on either side; the JSON document |
| [Transports](transports.md) | `tcp`, `tcpmux`, `ws`, `wss`, `quic`, `kcp`: how each works and when to pick it |
| [Profiles](profiles.md) | `balanced`, `ultraspeed`, `gaming`, and overriding their values |
| [UDP forwarding and games](udp-and-games.md) | UDP rules, the datagram path, packet duplication, DSCP |
| [Performance](performance.md) | Measurements, tuning for speed, running the benchmarks |
| [Running behind a CDN](CDN.md) | `ws` / `wss` through Cloudflare or ArvanCloud |
| [Security](security.md) | What the token protects, encryption, what is visible on the wire |
| [Troubleshooting](troubleshooting.md) | `kariz check` warnings, common errors, logs |

Design documents, for how and why things were built:
[ROADMAP](ROADMAP.md), [phase 2](PHASE2.md) (encryption, mux, WebSocket),
[phase 3](PHASE3.md) (UDP), [phase 4](PHASE4.md) (QUIC, KCP),
[phase 6](PHASE6.md) (gaming).

In Persian: [README_FA.md](../README_FA.md) covers the same ground in short.
