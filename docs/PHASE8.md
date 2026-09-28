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

## 3. More targets (8.2)

| Target | Machines |
|---|---|
| `x86_64-unknown-linux-musl` | as today |
| `aarch64-unknown-linux-musl` | ARM64 servers and boards |
| `armv7-unknown-linux-musleabihf` | 32-bit ARM with hardware floating point |

- **Built with `cross`** (Docker images with the C cross-compilers that `ring` and
  mimalloc need), in a job matrix in `release.yml`. Each target becomes its own archive,
  `kariz-vX.Y.Z-<arch>-linux.tar.gz`, with its SHA-256.
- **Checked on every PR:** a CI job cross-builds the three targets and runs the library
  tests for aarch64 and armv7 under QEMU, so a target-specific break (32-bit atomics,
  alignment) shows before a release.

## 4. Work breakdown

| Step | Content | Done when |
|---|---|---|
| **8.0** Plan | This document. | |
| **8.1** mimalloc | Global allocator behind a feature; musl measurements. | Numbers in this document; default decided. |
| **8.2** Targets | `cross` builds for three targets in CI and in the release. | CI builds all three and tests two under QEMU. |
| **8.3** Release | README / docs (downloads per architecture), CHANGELOG, `0.6.0`. | Release with three archives. |
