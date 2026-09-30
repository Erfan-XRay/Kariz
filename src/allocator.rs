//! Settings for the global allocator (mimalloc, cargo feature `mimalloc`).
//!
//! The allocator itself is installed by the `kariz` binary (and the test binaries that
//! benchmark it); this only tunes it. Measured on static musl builds:
//! mimalloc's default commits its first arena eagerly, and an idle tunnel then takes
//! 15.6 MiB of memory against 6.0 MiB with musl's own allocator. Committing on demand
//! instead brings it to 8.1 MiB, with the speed gain of mimalloc intact.

/// Call first thing in `main`, before the runtime starts its threads. Does nothing without
/// the `mimalloc` feature. An explicit `MIMALLOC_ARENA_EAGER_COMMIT` in the environment
/// wins over it.
pub fn tune() {
    #[cfg(feature = "mimalloc")]
    {
        // The index of `mi_option_arena_eager_commit` in mimalloc v3's option enum (the
        // bindings do not name it). `libmimalloc-sys` is pinned to an exact version in
        // Cargo.toml, so the index cannot move underneath it.
        const ARENA_EAGER_COMMIT: libmimalloc_sys::mi_option_t = 4;
        if std::env::var_os("MIMALLOC_ARENA_EAGER_COMMIT").is_none() {
            // SAFETY: plain option setter; no pointers, and any value is accepted.
            unsafe { libmimalloc_sys::mi_option_set(ARENA_EAGER_COMMIT, 0) };
        }
    }
}
