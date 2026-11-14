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

/// The kernel heap by owner (`heap by owner: ...`): what the registry, the
/// file system and the image cache's layouts hold, from their
/// structures' capacities, and the rest
/// of the heap in use (`in_use`, bytes) as "other" (processes, handles,
/// sections' bookkeeping, buffers, allocator rounding). The lines that
/// print it: the end of a run, the state dump, `persist`'s low-memory
/// warning (OBSERVABILITY "Serial log").
pub struct HeapOwners {
    pub in_use: usize,
    /// None: its lock was held (the state dump does not wait).
    pub registry: Option<super::registry::HeapUse>,
    pub files: Option<crate::fs::ramfs::HeapUse>,
    /// The image cache's layouts: bytes, count.
    pub images: Option<(usize, usize)>,
}

impl HeapOwners {
    /// Measures now (try-locks: safe from the state dump).
    pub fn now(in_use: usize) -> Self {
        Self {
            in_use,
            registry: super::registry::try_heap_use(),
            files: super::file::try_heap_use(),
            images: super::section::try_image_heap(),
        }
    }
}

impl core::fmt::Display for HeapOwners {
    fn fmt(&self, out: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut known = 0;
        match self.registry {
            Some(r) => {
                known += r.bytes;
                write!(
                    out,
                    "registry {} KiB ({} keys, {} values, {} KiB of value data)",
                    r.bytes / 1024,
                    r.keys,
                    r.values,
                    r.data / 1024
                )?;
            }
            None => out.write_str("registry (lock held)")?,
        }
        match self.files {
            Some(f) => {
                known += f.tree + f.pages;
                write!(
                    out,
                    ", file tree {} KiB ({} nodes), files' pages {} KiB",
                    f.tree / 1024,
                    f.nodes,
                    f.pages / 1024
                )?;
            }
            None => out.write_str(", files (lock held)")?,
        }
        match self.images {
            Some((bytes, count)) => {
                known += bytes;
                write!(out, ", image layouts {} KiB ({count} images)", bytes / 1024)?;
            }
            None => out.write_str(", image layouts (lock held)")?,
        }
        write!(
            out,
            ", other {} KiB, of {} KiB in use",
            self.in_use.saturating_sub(known) / 1024,
            self.in_use / 1024
        )
    }
}
