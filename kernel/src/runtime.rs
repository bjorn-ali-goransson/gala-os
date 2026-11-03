//! Threads, waits and time for core subsystems that also run in gala-um
//! (today `net`): the kernel's scheduler and clock, or, with the `hosted`
//! feature, gala-um's host threads and clocks.
//!
//! The NT layer reaches the machine through its own seam, `nt::platform`;
//! core code does not go through it, so that a second personality can sit
//! on the same core (ARCHITECTURE "Plan: a second personality (Linux)",
//! seam 6). xtask's unit test `net_does_not_depend_on_nt` keeps `net/` off
//! `crate::nt`. Each function here is the one `nt::platform` forwards to on
//! the same platform, so the two seams agree on threads and time.

pub use crate::sched::{SpawnError, ThreadId, WakeReason};
/// Ticks per second (1000 on both platforms).
pub const TICK_HZ: u64 = crate::time::TICK_HZ;

#[cfg(not(feature = "hosted"))]
pub use crate::sched::{current, prepare_wait, sleep_ms, spawn, wait, wake};
#[cfg(not(feature = "hosted"))]
pub use crate::time::{monotonic_ns, ticks};

#[cfg(feature = "hosted")]
pub use crate::hosted::threads::{current, monotonic_ns, prepare_wait, ticks, wait, wake};

/// A kernel thread (no user mode) running `entry(arg)`.
#[cfg(feature = "hosted")]
pub fn spawn(name: &'static str, entry: fn(usize), arg: usize) -> Result<ThreadId, SpawnError> {
    crate::hosted::threads::spawn(name, entry, arg, false, || {})
}

/// Blocks the current thread for at least `ms` milliseconds.
#[cfg(feature = "hosted")]
pub fn sleep_ms(ms: u64) {
    crate::hosted::sleep_ms(ms)
}
