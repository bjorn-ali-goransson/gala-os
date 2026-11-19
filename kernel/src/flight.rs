//! The flight recorder (docs/OBSERVABILITY.md "Flight recorder", ROADMAP R4): an
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
    /// A system call starts: a = number | pid << 32, b = thread id
    /// (a Linux call's number carries [`LINUX`]).
    Syscall = 1,
    /// It ends: a = number (with [`LINUX`] for a Linux call) | thread
    /// id << 32, b = its status (a Linux call's return value). The thread
    /// is named because a call need not end on the CPU, or right after
    /// the records, it started with: a call that blocks or is preempted
    /// resumes on whichever CPU picks the thread, after other threads'
    /// records.
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

/// In a system call record's number: a Linux call (its number in the
/// machine's Linux table, not NT's).
pub const LINUX: u64 = 1 << 31;

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

/// What a recorded call's number is called: a system service of the NT
/// table, or a Linux call from the machine's Linux table.
pub fn call_name(number: u64) -> &'static str {
    if number & LINUX != 0 {
        crate::lx::syscalls::name(number & !LINUX).unwrap_or("an unknown Linux call")
    } else {
        crate::nt::syscalls::service_name(number as u32)
    }
}

/// One system call a thread made: when it started (ns), its number and,
/// when it returned before the records were read, its status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Call {
    pub at: u64,
    pub number: u64,
    pub status: Option<u32>,
}

/// Calls kept for a crash report: enough to say what a thread was doing
/// before the exception, over the calls the dispatcher's own search adds
/// after it (eleven in a `crash_raise` run).
pub const CALLS: usize = 24;

/// Call ends kept while pairing them with their calls: one per call kept,
/// and room for ends of calls older than those (a call a callback was made
/// from, whose end comes after the calls inside it).
const ENDS: usize = CALLS + 8;

/// The last calls of thread `tid` in `rings`, oldest first, into `out`;
/// the count written. A call's status is the `syscall-done` record of the
/// same thread and number that followed it in time, on any CPU: a call
/// that blocks or is preempted resumes on whichever CPU picks its thread,
/// and other threads' records come between its start and its end even on
/// one CPU. A call with no such end had not returned when the records were
/// read (the call the thread died in, one a frame above is still in, or one
/// that does not return, as NtContinue).
#[cfg_attr(feature = "hosted", allow(dead_code))]
fn calls_of<'a>(rings: impl Iterator<Item = &'a Ring>, tid: u64, out: &mut [Call]) -> usize {
    // The latest of `item` by its time into `kept` (holding `*n`).
    fn keep<T: Copy>(kept: &mut [T], n: &mut usize, item: T, at: impl Fn(&T) -> u64) {
        if *n < kept.len() {
            kept[*n] = item;
            *n += 1;
        } else if let Some(oldest) = kept[..*n].iter_mut().min_by_key(|c| at(c))
            && at(oldest) < at(&item)
        {
            *oldest = item;
        }
    }
    let mut n = 0;
    // The thread's call ends: (time, number, status).
    let mut ends = [(0u64, 0u64, 0u32); ENDS];
    let mut m = 0;
    let room = ENDS.min(out.len() + (ENDS - CALLS));
    for ring in rings {
        for (at, kind, a, b) in ring.records() {
            match kind {
                k if k == Kind::Syscall as u8 && b == tid => {
                    let call = Call { at, number: a & 0xffff_ffff, status: None };
                    keep(out, &mut n, call, |c| c.at);
                }
                k if k == Kind::SyscallDone as u8 && a >> 32 == tid => {
                    keep(&mut ends[..room], &mut m, (at, a & 0xffff_ffff, b as u32), |e| e.0);
                }
                _ => {}
            }
        }
    }
    out[..n].sort_unstable_by_key(|c| c.at);
    ends[..m].sort_unstable_by_key(|e| e.0);
    // Each end, oldest first, closes the latest call of its number that
    // started before it and is still open (a callback's calls end before
    // the call they were made from).
    for &(at, number, status) in &ends[..m] {
        if let Some(call) = out[..n]
            .iter_mut()
            .rev()
            .skip_while(|c| c.at > at)
            .find(|c| c.number == number && c.status.is_none())
        {
            call.status = Some(status);
        }
    }
    n
}

/// The last calls of thread `tid` on every CPU, oldest first (the crash
/// report: what the thread was doing before it died, which an exception
/// a program raises itself leaves nothing else to go on).
#[cfg(not(any(test, feature = "hosted")))]
pub fn thread_calls(tid: u64, out: &mut [Call]) -> usize {
    calls_of(RINGS.iter(), tid, out)
}

/// Host unit tests and gala-um: nothing is kept, so there is nothing to
/// say.
#[cfg(any(test, feature = "hosted"))]
pub fn thread_calls(_tid: u64, _out: &mut [Call]) -> usize {
    0
}

#[cfg(not(any(test, feature = "hosted")))]
static RINGS: [Ring; crate::arch::smp::MAX_CPUS] =
    [const { Ring::new() }; crate::arch::smp::MAX_CPUS];

/// Records an event on this CPU's ring.
#[cfg(not(any(test, feature = "hosted")))]
pub fn record(kind: Kind, a: u64, b: u64) {
    let cpu = crate::arch::percpu::cpu_index() as usize;
    if let Some(ring) = RINGS.get(cpu) {
        ring.push(crate::time::raw_monotonic_ns(), kind, a, b);
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
        // System calls by name, with the caller (xtask's triage reads
        // these to show each thread's last calls).
        let service = call_name;
        match kind {
            1 => print(format_args!(
                "flight: {t} cpu {cpu} {} {a:#x} {b:#x} ({} pid {} tid {b})",
                Kind::name(kind),
                service(a & 0xffff_ffff),
                a >> 32
            )),
            2 => print(format_args!(
                "flight: {t} cpu {cpu} {} {a:#x} {b:#x} ({} status {b:#010x} tid {})",
                Kind::name(kind),
                service(a & 0xffff_ffff),
                a >> 32
            )),
            _ => print(format_args!("flight: {t} cpu {cpu} {} {a:#x} {b:#x}", Kind::name(kind))),
        }
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
    let now = crate::time::raw_monotonic_ns();
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
        // And where it is (an NMI gets through with interrupts off).
        crate::statedump::trace_cpu(next);
    }
}

#[cfg(any(test, feature = "hosted"))]
pub fn on_tick(_hz: u64) {}

/// Seconds a thread may stay in the kernel without blocking or reaching
/// user mode before the watchdog reports it (a loop in the kernel; being
/// preempted does not count as progress).
pub const KERNEL_RUN_SECONDS: u64 = 20;

/// Whether a thread whose last progress (blocking, user mode) was at
/// `last` (ns; 0: never seen) and that a tick at `now` finds in the
/// kernel is reported: the new last-progress time and the verdict
/// (reported again KERNEL_RUN_SECONDS later if it still has not moved).
pub fn kernel_run(now: u64, last: u64) -> (u64, bool) {
    // Never seen, or it blocked since: counted from now.
    if last == 0 || last == BLOCKED {
        return (now, false);
    }
    if now.saturating_sub(last) >= KERNEL_RUN_SECONDS * NS { (now, true) } else { (last, false) }
}

/// Per thread (by id, modulo the table): when it last made progress.
#[cfg(not(any(test, feature = "hosted")))]
static PROGRESS: [AtomicU64; 4096] = [const { AtomicU64::new(0) }; 4096];

/// A thread's last progress while it is blocked: the first tick that finds
/// it running again starts its count (a long wait is not time in the
/// kernel).
pub const BLOCKED: u64 = u64::MAX;

/// Thread `id` blocks (waits, sleeps): counted afresh when it runs again.
#[cfg(not(any(test, feature = "hosted")))]
pub fn blocked(id: usize) {
    PROGRESS[id % PROGRESS.len()].store(BLOCKED, Ordering::Relaxed);
}

#[cfg(any(test, feature = "hosted"))]
pub fn blocked(_id: usize) {}

/// Thread `id` made progress (returned from a system call, runs user code,
/// yields).
#[cfg(not(any(test, feature = "hosted")))]
pub fn progress(id: usize) {
    PROGRESS[id % PROGRESS.len()].store(crate::time::raw_monotonic_ns().max(1), Ordering::Relaxed);
}

#[cfg(any(test, feature = "hosted"))]
pub fn progress(_id: usize) {}

/// From the timer interrupt: thread `id` was found running kernel code
/// (not idle) at `rip`; one that has neither blocked nor reached user mode
/// for KERNEL_RUN_SECONDS is reported with its name and this CPU's last
/// events.
#[cfg(not(any(test, feature = "hosted")))]
pub fn on_kernel_tick(id: usize, rip: u64) {
    let slot = &PROGRESS[id % PROGRESS.len()];
    let now = crate::time::raw_monotonic_ns().max(1);
    let (last, report) = kernel_run(now, slot.load(Ordering::Relaxed));
    slot.store(last, Ordering::Relaxed);
    if report {
        let me = crate::arch::percpu::cpu_index();
        record(Kind::Watchdog, u64::from(me), id as u64);
        crate::console::emergency_println(format_args!(
            "watchdog: thread {id} ({}) has been in the kernel for {KERNEL_RUN_SECONDS} s \
             without blocking or reaching user mode (CPU {me}, rip {rip:#x}); this CPU's last events:",
            crate::sched::current_name()
        ));
        dump(1 << me, 32, crate::console::emergency_println);
    }
}

#[cfg(any(test, feature = "hosted"))]
pub fn on_kernel_tick(_id: usize, _rip: u64) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_threads_last_calls_keep_each_calls_own_status() {
        let ring = Ring::new();
        // Two threads on one CPU: 16 opens a key that is not there, 20
        // waits, then 16 writes.
        ring.push(10, Kind::Syscall, 0x12, 16);
        ring.push(20, Kind::SyscallDone, 0x12 | 16 << 32, 0xc000_0034);
        ring.push(30, Kind::Syscall, 0x4, 20);
        ring.push(40, Kind::SyscallDone, 0x4 | 20 << 32, 0x102);
        ring.push(50, Kind::Syscall, 0x8, 16);
        ring.push(60, Kind::SyscallDone, 0x8 | 16 << 32, 0);
        let mut out = [Call::default(); CALLS];
        let n = calls_of([&ring].into_iter(), 16, &mut out);
        assert_eq!(
            &out[..n],
            &[
                Call { at: 10, number: 0x12, status: Some(0xc000_0034) },
                Call { at: 50, number: 0x8, status: Some(0) },
            ]
        );
    }

    #[test]
    fn a_call_that_never_returned_is_kept_without_a_status() {
        let ring = Ring::new();
        ring.push(10, Kind::Syscall, 0x91, 16);
        let mut out = [Call::default(); CALLS];
        let n = calls_of([&ring].into_iter(), 16, &mut out);
        assert_eq!(&out[..n], &[Call { at: 10, number: 0x91, status: None }]);
    }

    #[test]
    fn a_call_that_ends_on_another_cpu_keeps_its_status() {
        // arm_crash_raise: thread 8's NtWriteFile (0x8) blocks on CPU 0,
        // thread 12 makes a call there, and 8 resumes on CPU 1, where its
        // call ends and the next one starts.
        let (cpu0, cpu1) = (Ring::new(), Ring::new());
        cpu0.push(10, Kind::Syscall, 0x8, 8);
        cpu0.push(20, Kind::Syscall, 0x4, 12);
        cpu1.push(30, Kind::SyscallDone, 0x8 | 8 << 32, 0);
        cpu1.push(40, Kind::Syscall, 0x23, 8);
        cpu1.push(50, Kind::SyscallDone, 0x23 | 8 << 32, 0);
        cpu0.push(60, Kind::SyscallDone, 0x4 | 12 << 32, 0x102);
        let mut out = [Call::default(); CALLS];
        let n = calls_of([&cpu0, &cpu1].into_iter(), 8, &mut out);
        assert_eq!(
            &out[..n],
            &[
                Call { at: 10, number: 0x8, status: Some(0) },
                Call { at: 40, number: 0x23, status: Some(0) },
            ]
        );
    }

    #[test]
    fn another_threads_call_between_a_call_and_its_end_takes_nothing_from_it() {
        // One CPU: 16's wait (0x4) is preempted, 20 makes the same call
        // and it ends with its own status, then 16's ends.
        let ring = Ring::new();
        ring.push(10, Kind::Syscall, 0x4, 16);
        ring.push(20, Kind::Syscall, 0x4, 20);
        ring.push(30, Kind::SyscallDone, 0x4 | 20 << 32, 0x102);
        ring.push(40, Kind::SyscallDone, 0x4 | 16 << 32, 0);
        let mut out = [Call::default(); CALLS];
        let n = calls_of([&ring].into_iter(), 16, &mut out);
        assert_eq!(&out[..n], &[Call { at: 10, number: 0x4, status: Some(0) }]);
    }

    #[test]
    fn a_callbacks_calls_end_before_the_call_they_were_made_from() {
        // 16 makes 0x50, a callback inside it makes 0x50 and 0x8 (which
        // return) and 0x1 (the callback's return, which does not), then
        // the outer 0x50 ends.
        let ring = Ring::new();
        ring.push(10, Kind::Syscall, 0x50, 16);
        ring.push(20, Kind::Syscall, 0x50, 16);
        ring.push(30, Kind::SyscallDone, 0x50 | 16 << 32, 0xc000_000d);
        ring.push(40, Kind::Syscall, 0x8, 16);
        ring.push(50, Kind::SyscallDone, 0x8 | 16 << 32, 0);
        ring.push(60, Kind::Syscall, 0x1, 16);
        ring.push(70, Kind::SyscallDone, 0x50 | 16 << 32, 0);
        let mut out = [Call::default(); CALLS];
        let n = calls_of([&ring].into_iter(), 16, &mut out);
        assert_eq!(
            &out[..n],
            &[
                Call { at: 10, number: 0x50, status: Some(0) },
                Call { at: 20, number: 0x50, status: Some(0xc000_000d) },
                Call { at: 40, number: 0x8, status: Some(0) },
                Call { at: 60, number: 0x1, status: None },
            ]
        );
    }

    #[test]
    fn the_calls_kept_are_the_last_ones_whatever_cpu_they_ran_on() {
        let (first, second) = (Ring::new(), Ring::new());
        for i in 0..20u64 {
            let ring = if i % 2 == 0 { &first } else { &second };
            ring.push(100 + i, Kind::Syscall, i, 16);
            ring.push(100 + i, Kind::SyscallDone, i | 16 << 32, 0);
        }
        let mut out = [Call::default(); 4];
        let n = calls_of([&first, &second].into_iter(), 16, &mut out);
        assert_eq!(n, 4);
        assert_eq!(out.map(|c| c.number), [16, 17, 18, 19]);
    }

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
    fn a_thread_in_the_kernel_without_progress_is_reported_every_twenty_seconds() {
        // First seen: no verdict.
        assert_eq!(kernel_run(5 * NS, 0), (5 * NS, false));
        // Still within the limit: last progress kept.
        assert_eq!(kernel_run(24 * NS, 5 * NS), (5 * NS, false));
        // Blocked since: counted afresh, however long ago.
        assert_eq!(kernel_run(90 * NS, BLOCKED), (90 * NS, false));
        // At the limit: reported, counted again from now.
        assert_eq!(kernel_run(25 * NS, 5 * NS), (25 * NS, true));
        assert_eq!(kernel_run(30 * NS, 25 * NS), (25 * NS, false));
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
