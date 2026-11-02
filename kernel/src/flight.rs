//! The flight recorder (docs/OBSERVABILITY.md §5, ROADMAP R4): an
//! always-on ring per CPU of the last events, kept in memory whether the
//! event stream is on or not, and printed when the kernel panics or the
//! hang watchdog finds a CPU that stopped taking timer interrupts. A record
//! is three words: the time (ns since boot) shifted left 8 with its kind in
//! the low byte, then two values whose meaning depends on the kind.
//! Recording costs one atomic increment and three stores; a record being
//! written while the ring is dumped may show torn (it is for people).

use core::sync::atomic::{AtomicU64, Ordering};

/// What a record is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// A system call starts: a = number | pid << 32, b = thread id.
    Syscall = 1,
    /// It ends: a = number, b = its status.
    SyscallDone = 2,
    /// A process ended: a = pid, b = exit status.
    ProcessExit = 3,
    /// The watchdog's own mark: a = the CPU it found stuck.
    Watchdog = 4,
}

impl Kind {
    #[cfg_attr(any(test, feature = "hosted"), allow(dead_code))]
    fn name(byte: u8) -> &'static str {
        match byte {
            1 => "syscall",
            2 => "syscall-done",
            3 => "process-exit",
            4 => "watchdog",
            _ => "?",
        }
    }
}

/// Records kept per CPU.
pub const PER_CPU: usize = 256;

/// One CPU's ring.
pub struct Ring {
    head: AtomicU64,
    slots: [[AtomicU64; 3]; PER_CPU],
}

impl Ring {
    pub const fn new() -> Self {
        Ring {
            head: AtomicU64::new(0),
            slots: [const { [const { AtomicU64::new(0) }; 3] }; PER_CPU],
        }
    }

    /// Adds a record at time `t` (ns).
    pub fn push(&self, t: u64, kind: Kind, a: u64, b: u64) {
        let at = self.head.fetch_add(1, Ordering::Relaxed) as usize % PER_CPU;
        let s = &self.slots[at];
        s[0].store((t << 8) | kind as u64, Ordering::Relaxed);
        s[1].store(a, Ordering::Relaxed);
        s[2].store(b, Ordering::Relaxed);
    }

    /// The records, oldest first: (time, kind byte, a, b).
    pub fn records(&self) -> impl Iterator<Item = (u64, u8, u64, u64)> + '_ {
        let head = self.head.load(Ordering::Relaxed) as usize;
        let n = head.min(PER_CPU);
        (head - n..head).map(move |i| {
            let s = &self.slots[i % PER_CPU];
            let w = s[0].load(Ordering::Relaxed);
            (w >> 8, w as u8, s[1].load(Ordering::Relaxed), s[2].load(Ordering::Relaxed))
        })
    }
}

impl Default for Ring {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(any(test, feature = "hosted")))]
static RINGS: [Ring; crate::arch::smp::MAX_CPUS] =
    [const { Ring::new() }; crate::arch::smp::MAX_CPUS];

/// Records an event on this CPU's ring.
#[cfg(not(any(test, feature = "hosted")))]
pub fn record(kind: Kind, a: u64, b: u64) {
    let cpu = crate::arch::percpu::cpu_index() as usize;
    if let Some(ring) = RINGS.get(cpu) {
        ring.push(crate::time::monotonic_ns(), kind, a, b);
    }
}

/// Host unit tests and gala-um: nothing kept (the host has its own tools).
#[cfg(any(test, feature = "hosted"))]
pub fn record(_kind: Kind, _a: u64, _b: u64) {}

/// The last `max` records of the CPUs in `cpus` (a bit mask), merged by
/// time, oldest first, through `print`.
#[cfg(not(any(test, feature = "hosted")))]
pub fn dump(cpus: u64, max: usize, mut print: impl FnMut(core::fmt::Arguments)) {
    let mut last = [(0u64, 0u8, 0u64, 0u64, 0usize); 64];
    let mut n = 0;
    // The newest `max` overall: walk each ring and keep the latest.
    for (cpu, ring) in RINGS.iter().enumerate().filter(|(c, _)| cpus & (1 << c) != 0) {
        for (t, kind, a, b) in ring.records() {
            if kind == 0 {
                continue;
            }
            let entry = (t, kind, a, b, cpu);
            if n < max.min(last.len()) {
                last[n] = entry;
                n += 1;
            } else if let Some(oldest) = last[..n].iter_mut().min_by_key(|e| e.0)
                && oldest.0 < t
            {
                *oldest = entry;
            }
        }
    }
    last[..n].sort_unstable_by_key(|e| e.0);
    print(format_args!("flight: the last {n} events (time ns, cpu, kind, values):"));
    for &(t, kind, a, b, cpu) in &last[..n] {
        print(format_args!("flight: {t} cpu {cpu} {} {a:#x} {b:#x}", Kind::name(kind)));
    }
}

#[cfg(any(test, feature = "hosted"))]
pub fn dump(_cpus: u64, _max: usize, _print: impl FnMut(core::fmt::Arguments)) {}

// ---- The hang watchdog ----

/// Per CPU: its timer interrupts so far; for the CPU watching it, the
/// heartbeat it saw last, since when (ns) it has not moved, and whether
/// that stall was reported.
#[cfg(not(any(test, feature = "hosted")))]
static HEARTBEAT: [AtomicU64; crate::arch::smp::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::arch::smp::MAX_CPUS];
#[cfg(not(any(test, feature = "hosted")))]
static SEEN: [[AtomicU64; 3]; crate::arch::smp::MAX_CPUS] =
    [const { [const { AtomicU64::new(0) }; 3] }; crate::arch::smp::MAX_CPUS];
/// Per CPU: when (ns) it last looked at the CPU it watches.
#[cfg(not(any(test, feature = "hosted")))]
static LOOKED: [AtomicU64; crate::arch::smp::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::arch::smp::MAX_CPUS];

/// Seconds without a timer interrupt before a CPU is reported.
pub const STUCK_SECONDS: u64 = 5;
const NS: u64 = 1_000_000_000;

/// What the watcher knows of one CPU: the heartbeat seen, since when it
/// has not moved (ns), whether that stall was reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    pub beat: u64,
    pub since: u64,
    pub reported: bool,
}

/// Looks at a CPU whose heartbeat is `beat` at time `now` (ns, the TSC
/// clock: timer ticks themselves may come late when the host is busy):
/// the new view and whether to report it (once per stall, when it has
/// not moved for STUCK_SECONDS).
pub fn watch(beat: u64, now: u64, seen: Seen) -> (Seen, bool) {
    if beat != seen.beat {
        return (Seen { beat, since: now, reported: false }, false);
    }
    let stuck = !seen.reported && now.saturating_sub(seen.since) >= STUCK_SECONDS * NS;
    (Seen { reported: seen.reported || stuck, ..seen }, stuck)
}

/// From every CPU's timer interrupt: counts this CPU's heartbeat; about
/// once a second (by the TSC clock) looks at the next online CPU's and
/// reports it if it stopped (a CPU spinning with interrupts off: a lock
/// never released, a loop in an interrupt handler).
#[cfg(not(any(test, feature = "hosted")))]
pub fn on_tick(_hz: u64) {
    let me = crate::arch::percpu::cpu_index() as usize;
    let Some(beat) = HEARTBEAT.get(me) else { return };
    beat.fetch_add(1, Ordering::Relaxed);
    let now = crate::time::monotonic_ns();
    if now.saturating_sub(LOOKED[me].load(Ordering::Relaxed)) < NS {
        return;
    }
    LOOKED[me].store(now, Ordering::Relaxed);
    let online = crate::arch::smp::online_mask();
    if online.count_ones() < 2 {
        return;
    }
    // The next online CPU after this one (round the mask).
    let next = (1..64).map(|k| (me + k) % 64).find(|&c| online & (1 << c) != 0);
    let Some(next) = next else { return };
    let slot = &SEEN[next];
    let seen = Seen {
        beat: slot[0].load(Ordering::Relaxed),
        since: slot[1].load(Ordering::Relaxed),
        reported: slot[2].load(Ordering::Relaxed) != 0,
    };
    let current = HEARTBEAT[next].load(Ordering::Relaxed);
    let (seen, report) = watch(current, now, seen);
    slot[0].store(seen.beat, Ordering::Relaxed);
    slot[1].store(seen.since, Ordering::Relaxed);
    slot[2].store(u64::from(seen.reported), Ordering::Relaxed);
    if report {
        record(Kind::Watchdog, next as u64, current);
        crate::console::emergency_println(format_args!(
            "watchdog: CPU {next} has taken no timer interrupt for {STUCK_SECONDS} s \
             (interrupts off: a lock never released?); its last events:"
        ));
        dump(1 << next, 32, crate::console::emergency_println);
    }
}

#[cfg(any(test, feature = "hosted"))]
pub fn on_tick(_hz: u64) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_keeps_the_last_records_in_order() {
        let r = Ring::new();
        for i in 0..(PER_CPU as u64 + 10) {
            r.push(100 + i, Kind::Syscall, i, 0);
        }
        let all: alloc::vec::Vec<_> = r.records().collect();
        assert_eq!(all.len(), PER_CPU);
        assert_eq!(all[0], (110, Kind::Syscall as u8, 10, 0));
        assert_eq!(all[PER_CPU - 1].2, PER_CPU as u64 + 9);
    }

    #[test]
    fn the_watchdog_reports_a_heartbeat_stopped_for_five_seconds_once() {
        let mut seen = Seen { beat: 0, since: 0, reported: false };
        let mut reports = 0;
        // Moving: never reported.
        for k in 1..10u64 {
            let (s, r) = watch(k * 1000, k * NS, seen);
            seen = s;
            reports += usize::from(r);
        }
        assert_eq!(reports, 0);
        // Stopped at 9 s: reported once 5 s later, not again.
        for k in 10..30u64 {
            let (s, r) = watch(9000, k * NS, seen);
            seen = s;
            reports += usize::from(r);
            assert_eq!(r, k == 14);
        }
        assert_eq!(reports, 1);
        // Moving again, then stopping again: reported again.
        let (s, _) = watch(9001, 30 * NS, seen);
        seen = s;
        let (_, r) = watch(9001, 35 * NS, seen);
        assert!(r);
    }
}
