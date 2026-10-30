//! IO shell: sockets, threads, timers, wakeups. Drives the protocol core.
//!
//! A first-class, specified, tested component, not glue. See docs/02-io-shell.md.

//! **The system calls are one module per platform, chosen when the crate is
//! built**, under the same names on every platform: the loop, the attempt
//! thread, the send batching, the browser transport and the address choice
//! above them are written once. Linux's and Windows' are written, so what
//! drives a socket is built on those two. That module is also the only one
//! here containing `unsafe`.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
// Check the runtime graph. Test-only CA generation is intentionally unused when
// the optional stream feature is disabled in a UDP-only test invocation.
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
// Tests may panic freely: a failing assertion is the point, and a fixture that
// cannot be built is a broken test rather than hostile input.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        // Fixtures build small values from loop counters; a truncating cast
        // there is obviously fine and spelling out try_from obscures the test.
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )
)]

// The host candidates wait on the platform's interface walk.
#[cfg_attr(not(any(target_os = "linux", target_os = "android", windows)), allow(dead_code))]
pub mod addrs;
#[cfg(any(target_os = "linux", target_os = "android", windows))]
mod carrier;
#[cfg(all(feature = "turn-stream", any(target_os = "linux", target_os = "android", windows)))]
pub mod stream_io;
#[cfg(all(feature = "turn-stream", any(target_os = "linux", target_os = "android", windows)))]
mod tcp;
pub mod turn_stream;

#[cfg(any(target_os = "linux", target_os = "android"))]
#[path = "linux/mod.rs"]
mod sys;
#[cfg(windows)]
#[path = "windows/mod.rs"]
mod sys;

#[cfg(any(target_os = "linux", target_os = "android", windows))]
pub mod guest;
#[cfg(any(target_os = "linux", target_os = "android", windows))]
mod send;
#[cfg(any(target_os = "linux", target_os = "android", windows))]
pub mod shell;
#[cfg(any(target_os = "linux", target_os = "android", windows))]
pub mod socket;
#[cfg(any(target_os = "linux", target_os = "android", windows))]
pub mod wake;

pub use addrs::MAX_HOST_ADDRESSES;
#[cfg(any(target_os = "linux", target_os = "android", windows))]
pub use addrs::host_addresses;
#[cfg(any(target_os = "linux", target_os = "android", windows))]
pub use guest::{Guest, Running};
#[cfg(any(target_os = "linux", target_os = "android", windows))]
pub use shell::{Shell, Stats, Turn, Woke};
#[cfg(any(target_os = "linux", target_os = "android", windows))]
pub use socket::{DEFAULT_TTL, RECV_BATCH, RECV_SLOT, Socket};
#[cfg(any(target_os = "linux", target_os = "android", windows))]
pub use wake::{Wake, WakeHandle};
