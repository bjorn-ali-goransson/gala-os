//! Always-on counters of where the NT layer spends time: system calls
//! (waits apart: their time is mostly sleeping), user faults the kernel
//! handles, and process creation. Two clock reads per event; printed in
//! the end-of-run `stats` line, so a run can be broken down without the
//! event stream (whose own cost, one debug-port write per event, changes
//! what it measures).

use super::platform;
use core::sync::atomic::{AtomicU64, Ordering};

pub struct Counter {
    count: AtomicU64,
    ns: AtomicU64,
}

impl Counter {
    const fn new() -> Self {
        Self { count: AtomicU64::new(0), ns: AtomicU64::new(0) }
    }

    /// Counts one event that began at `started` (monotonic ns).
    pub fn add_since(&self, started: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.ns.fetch_add(platform::monotonic_ns().saturating_sub(started), Ordering::Relaxed);
    }

    /// (count, total milliseconds).
    pub fn get(&self) -> (u64, u64) {
        (self.count.load(Ordering::Relaxed), self.ns.load(Ordering::Relaxed) / 1_000_000)
    }
}

pub static SYSCALLS: Counter = Counter::new();
pub static WAITS: Counter = Counter::new();
pub static FAULTS: Counter = Counter::new();
pub static PROCESSES: Counter = Counter::new();

/// Whether service `name` mostly sleeps (its time is waiting, not work).
pub fn is_wait(name: &str) -> bool {
    name.starts_with("NtWait")
        || matches!(name, "NtDelayExecution" | "NtRemoveIoCompletion" | "NtRemoveIoCompletionEx")
}

/// One line for the end of a run (`info!("nt: {}", Summary)`).
pub struct Summary;

impl core::fmt::Display for Summary {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let (calls, calls_ms) = SYSCALLS.get();
        let (waits, waits_ms) = WAITS.get();
        let (faults, faults_ms) = FAULTS.get();
        let (spawns, spawns_ms) = PROCESSES.get();
        write!(
            out,
            "{calls} system calls in {calls_ms} ms (and {waits} waits, {waits_ms} ms), \
             {faults} user faults in {faults_ms} ms, {spawns} processes created in {spawns_ms} ms"
        )
    }
}
