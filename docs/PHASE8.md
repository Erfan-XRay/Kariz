# Phase 8 plan: release builds

Target release: **v0.6.0**. Scope from [ROADMAP.md](ROADMAP.md): static musl builds for
x86_64, aarch64 and armv7, and mimalloc.

## 1. Why

- **More machines.** Releases ship one binary today, for x86_64. Many cheap VPS and ARM
  boards are aarch64 (Ampere, Graviton, Raspberry Pi 4 and 5) or 32-bit armv7 (older
  boards, some routers).
- **musl's allocator is slow.** Static musl binaries run anywhere, but musl's `malloc`
  is built for size, not speed. Its cost shows most under many small allocations (every
  packet and frame), which is Kariz's hot path. mimalloc is a small, fast allocator that
  links statically.

## 2. mimalloc (8.1)

- **The global allocator** of the `kariz` binary (not the library), behind a cargo
  feature `mimalloc`, on by default. `--no-default-features` still builds without it.
- **Measured** on Linux musl with and without it: the localhost throughput benchmark,
  UDP packets per second, and memory (RSS) idle and with 100 connections. A temporary CI
  job does this, since the development machine is Windows.
- **Kept only if it pays.** mimalloc usually uses a little more memory. If the gain is
  small, it goes off by default and stays available.

*Status after 8.1:* mimalloc is the binary's global allocator, feature `mimalloc`, on by
default. Measured on GitHub Actions runners (2 vCPU, static musl builds, temporary CI job,
removed): the throughput benchmark (28 setups, 3 alternating rounds each) and the memory
script, with and without it.

| | mimalloc | musl's allocator |
|---|---|---|
| Throughput, mean over the 28 setups | +34 % | |
| With mux, `wss`, `quic`, `kcp` | +25 to +100 % | |
| Plain `tcp`, `ws` without mux | -12 to +8 % | |
| Idle memory | 15.6 MiB untuned, **8.1 MiB** tuned | 6.0 MiB |
| 100 idle connections | +1 MiB | +1.7 to +5 MiB (varied between runs) |
| 1,000 idle UDP flows | +6 to +9 MiB | +4 MiB |

- **The gain pays**, so it stays the default, with its memory documented.
- **The eager arena commit is the memory problem.** Setting
  `MIMALLOC_ARENA_EAGER_COMMIT=0` takes the idle side from 15.6 to 8.1 MiB.
  `kariz::allocator::tune` sets it first thing in `main` (the option is named by its
  index in mimalloc v3, so `libmimalloc-sys` is pinned to an exact version); CI showed the
  built-in setting gives the same 8.1 MiB as the environment variable. An explicit
  `MIMALLOC_ARENA_EAGER_COMMIT` in the environment wins.
- **Turning arenas off** too (`MIMALLOC_DISALLOW_ARENA_ALLOC=1`) gets the idle side to
  6.5 MiB, but 1,000 UDP flows then cost as much as before; not adopted.
- **The benchmarks use the same setting** (`tune()` at the top of `throughput` and
  `udp_throughput`), so they measure what the binary runs.

## 3. More targets (8.2)

| Target | Machines |
|---|---|
| `x86_64-unknown-linux-musl` | as today |
| `aarch64-unknown-linux-musl` | ARM64 servers and boards |
| `armv7-unknown-linux-musleabihf` | 32-bit ARM with hardware floating point |

- **Built with `cross`** (Docker images with the C cross-compilers that `ring` and
  mimalloc need), in a job matrix in `release.yml`. Each target becomes its own archive,
  `kariz-vX.Y.Z-<arch>-linux.tar.gz`, with its SHA-256.
- **Checked on every PR:** a CI job cross-builds the ARM targets (x86_64 is built by the
  ordinary jobs) and runs the library tests for aarch64 and armv7 under QEMU, so a
  target-specific break (32-bit atomics, alignment) shows before a release.

*Status after 8.2:* `release.yml` builds the three targets in a matrix (x86_64 natively
with `musl-tools`, the ARM ones with `cross`), and a `publish` job collects the archives,
checks that there are three and that each matches its SHA-256, and makes the release.
The archives also carry `scripts/` and `assets/` now. The `cross` CI job passed for
armv7, tests included. For aarch64 it built, and 193 of 194 library tests passed under
QEMU; the one failure was `many_connections_on_one_listener`, which starts 20 KCP
dialers at once: on the emulated CPU (the run took 110 s) the listener's socket buffer
overflowed, a dialer's opening packet was lost, and its keep-alive ping (every second in
that test) reached the listener before KCP's resend did. A ping for a conversation the
listener does not know ends it by design (a restarted listener closes old conversations
that way), so the dialer saw `ConnectionReset`. Not an ARM bug: the test now uses a long
keep-alive, since pings are not what it is about.

## 4. Work breakdown

| Step | Content | Done when |
|---|---|---|
| **8.0** Plan (done) | This document. | |
| **8.1** mimalloc (done) | Global allocator behind a feature; musl measurements. | Numbers in this document; default decided. |
| **8.2** Targets (done) | `cross` builds for three targets in CI and in the release. | CI builds all three and tests two under QEMU. |
| **8.3** Release (done) | README / docs (downloads per architecture), CHANGELOG, `0.6.0`. | Release with three archives. |

Beyond the plan, v0.6.0 also carries what was asked for along the way: the manager
script (`scripts/kariz.sh`), the startup banner and new log lines, `kariz speedtest`
(docs/speedtest.md) and the `ultraspeed` profile name, mux settings and documentation of
v0.5.1.
