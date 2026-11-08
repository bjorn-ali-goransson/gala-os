//! State dumps on request (docs/OBSERVABILITY.md, "State
//! dump"): a boot test that times out used to end with "timed out
//! after 300s" and nothing else. Now xtask first injects an NMI (QEMU's
//! monitor command `nmi`, delivered to every CPU) and waits for the dump;
//! gala-um gets SIGQUIT (`timeout -s QUIT`). The dump has two parts:
//!
//! 1. From the NMI itself, which may interrupt any code holding any lock:
//!    each CPU records where it was (rip, kernel backtrace); the first one
//!    in waits for the others, then prints with the lock-free console
//!    (`statedump: cpu N ...`, `statedump: cpu N backtrace: ...` which xtask
//!    symbolizes), which of the main locks are held (a deadlock shows as a
//!    lock held for the whole dump), heap and frame figures, and the
//!    flight recorder. Only atomics and `try_lock` here.
//! 2. From a normal context, which may take locks: the boot thread's wait
//!    for its `run=` program wakes once a second and, when asked, logs every
//!    NT thread (system call, wait object), every process (commit, pool
//!    and handle quotas), every scheduler thread, the ledger and the
//!    statistics, then `statedump: end`. A machine too stuck
//!    for that still has part 1.
//!
//! Cheap when off: nothing runs until an NMI or SIGQUIT arrives.

use core::sync::atomic::{AtomicBool, Ordering};

/// Set when a dump was asked for; the normal-context part takes it.
static REQUESTED: AtomicBool = AtomicBool::new(false);

/// Marks a dump as wanted (from the NMI or a signal handler: one store).
pub fn request() {
    REQUESTED.store(true, Ordering::Release);
}

/// Whether a dump was asked for since the last call (clears it).
pub fn take_request() -> bool {
    REQUESTED.swap(false, Ordering::AcqRel)
}

/// Part 2: everything that needs locks, from a thread that may wait.
pub fn full_dump() {
    info!("statedump: threads (NT, then scheduler), the ledger, the statistics:");
    crate::nt::thread::dump();
    // Each process's commit and its pool and handle quotas (nt::quota).
    crate::nt::process::dump();
    #[cfg(not(feature = "hosted"))]
    crate::sched::dump();
    crate::nt::ledger::dump();
    // Linux threads (masks, pending signals, blocked or not), epoll sets
    // and futex waiters (ROADMAP LX).
    crate::lx::dump();
    crate::unix::dump();
    // The Wayland server's clients and their objects (ROADMAP LD).
    crate::wayland::dump();
    // Each session's windows (RS step B2).
    crate::nt::session::dump();
    // gala-um: the host's allocator, no frame figures worth printing.
    #[cfg(not(feature = "hosted"))]
    stats_line(Some(crate::mm::heap::stats()), crate::mm::try_frame_stats());
    info!("statedump: end");
}

/// The heap and frame figures as one line (xtask's triage reads it);
/// `None` for a lock that was held.
pub fn stats_line(heap: Option<crate::mm::heap::HeapStats>, frames: Option<(u64, u64, u64)>) {
    struct Line(Option<crate::mm::heap::HeapStats>, Option<(u64, u64, u64)>);
    impl core::fmt::Display for Line {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self.0 {
                Some(h) => write!(
                    f,
                    "statedump: heap {} KiB in use (peak {} KiB, {} large pages)",
                    h.in_use / 1024,
                    h.peak_in_use / 1024,
                    h.large_pages
                )?,
                None => f.write_str("statedump: heap (lock held)")?,
            }
            match self.1 {
                Some((free, total, low)) => {
                    write!(f, "; frames free {free} of {total} (fewest free {low})")
                }
                None => f.write_str("; frames (lock held)"),
            }
        }
    }
    crate::console::emergency_line(format_args!("{}", Line(heap, frames)));
}

#[cfg(not(any(test, feature = "hosted")))]
pub use nmi::{init, on_nmi, trace_cpu};

#[cfg(not(any(test, feature = "hosted")))]
mod nmi {
    use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};

    use crate::arch::interrupts::{CpuMode, TrapFrame};
    use crate::arch::smp::MAX_CPUS;
    use crate::console::{emergency_line, emergency_println};

    /// Return addresses kept per CPU.
    const FRAMES: usize = 24;
    const NS: u64 = 1_000_000_000;

    /// What each CPU was doing when the NMI came: the instruction and
    /// stack pointers, the mode (`CpuMode` as a number), then the return
    /// addresses of a kernel backtrace (0 ends them).
    struct Snapshot {
        ip: AtomicU64,
        sp: AtomicU64,
        mode: AtomicU8,
        frames: [AtomicU64; FRAMES],
    }

    static SNAPSHOTS: [Snapshot; MAX_CPUS] = [const {
        Snapshot {
            ip: AtomicU64::new(0),
            sp: AtomicU64::new(0),
            mode: AtomicU8::new(0),
            frames: [const { AtomicU64::new(0) }; FRAMES],
        }
    }; MAX_CPUS];
    /// CPUs that recorded their snapshot for this dump.
    static ANSWERED: AtomicU64 = AtomicU64::new(0);
    /// 1 while a CPU prints the dump.
    static DUMPING: AtomicU64 = AtomicU64::new(0);
    /// When (ns) the last dump ended: an NMI just after it is a slow CPU's
    /// part of the same request, not a new one.
    static LAST_END: AtomicU64 = AtomicU64::new(0);
    /// CPUs asked for their own backtrace only (`trace_cpu`), not a dump.
    static TRACE_ONLY: AtomicU64 = AtomicU64::new(0);

    /// Asks CPU `cpu` (by NMI, which reaches it with interrupts off) to
    /// print where it is: the hang watchdog's stuck CPU.
    pub fn trace_cpu(cpu: usize) {
        let Some((_, hw_id)) = crate::arch::smp::online_cpus().find(|&(i, _)| i as usize == cpu)
        else {
            return;
        };
        TRACE_ONLY.fetch_or(1 << cpu, Ordering::AcqRel);
        crate::arch::smp::send_nmi(hw_id);
    }

    /// Installs the NMI handler (once the per-CPU data and the clock are
    /// set up).
    pub fn init() {
        crate::arch::interrupts::set_nmi_handler(on_nmi);
    }

    /// This CPU's index (the arch NMI entry made GS the kernel's).
    fn me() -> usize {
        crate::arch::percpu::cpu_index() as usize
    }

    pub fn on_nmi(frame: &TrapFrame) {
        // A profiler sample (asked by another CPU's tick) is no dump
        // request. (A dump NMI that arrives while one is pending merges
        // with it: that CPU then misses the dump, which waits for it at
        // most half a second.)
        if crate::time::on_sample_nmi(frame) {
            return;
        }
        let cpu = me();
        let Some(snap) = SNAPSHOTS.get(cpu) else { return };
        snap.ip.store(frame.ip(), Ordering::Relaxed);
        snap.sp.store(frame.sp(), Ordering::Relaxed);
        snap.mode.store(frame.mode() as u8, Ordering::Relaxed);
        let mut n = 0;
        if !frame.from_user_mode() {
            crate::arch::backtrace::walk_trap(frame, |ret| {
                if n < FRAMES {
                    snap.frames[n].store(ret, Ordering::Relaxed);
                    n += 1;
                }
            });
        }
        if n < FRAMES {
            snap.frames[n].store(0, Ordering::Relaxed);
        }
        if TRACE_ONLY.fetch_and(!(1 << cpu), Ordering::AcqRel) & (1 << cpu) != 0 {
            print_cpu("watchdog: CPU", cpu, snap);
            return;
        }
        ANSWERED.fetch_or(1 << cpu, Ordering::AcqRel);
        let now = crate::time::raw_monotonic_ns();
        let last = LAST_END.load(Ordering::Relaxed);
        if last != 0 && now.saturating_sub(last) < 2 * NS {
            return;
        }
        if DUMPING.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed).is_err() {
            return;
        }
        // This CPU prints. The others' NMIs arrive at about the same time:
        // wait (at most half a second) until every online CPU has answered.
        let online = crate::arch::smp::online_mask();
        let start = crate::time::raw_monotonic_ns();
        while ANSWERED.load(Ordering::Acquire) & online != online
            && crate::time::raw_monotonic_ns().saturating_sub(start) < NS / 2
        {
            core::hint::spin_loop();
        }
        let answered = ANSWERED.swap(0, Ordering::AcqRel);
        print(cpu, answered, online);
        LAST_END.store(crate::time::raw_monotonic_ns().max(1), Ordering::Relaxed);
        DUMPING.store(0, Ordering::Release);
        super::request();
    }

    /// One CPU's snapshot: `<prefix> N user|kernel rip .. rsp ..`, then
    /// `<prefix> N backtrace: ..` for kernel code (xtask symbolizes it).
    fn print_cpu(prefix: &str, cpu: usize, snap: &Snapshot) {
        let rip = snap.ip.load(Ordering::Relaxed);
        // 32-bit code (a WOW64 thread): its eip and esp.
        let mode = CpuMode::from_u8(snap.mode.load(Ordering::Relaxed));
        emergency_line(format_args!(
            "{prefix} {cpu} {} rip {rip:#x} rsp {:#x}",
            mode.name(),
            snap.sp.load(Ordering::Relaxed)
        ));
        if mode != CpuMode::Kernel {
            return;
        }
        // As a panic's (rip first).
        struct Trace<'a>(u64, &'a Snapshot);
        impl core::fmt::Display for Trace<'_> {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, " {:#x}", self.0)?;
                for ret in self.1.frames.iter().map(|r| r.load(Ordering::Relaxed)) {
                    if ret == 0 {
                        break;
                    }
                    write!(f, " {ret:#x}")?;
                }
                Ok(())
            }
        }
        emergency_line(format_args!("{prefix} {cpu} backtrace:{}", Trace(rip, snap)));
    }

    fn print(me: usize, answered: u64, online: u64) {
        emergency_println(format_args!(
            "statedump: begin (NMI on CPU {me}; CPUs answered {answered:#x} of online {online:#x}; \
             now {} ms since boot)",
            crate::time::raw_monotonic_ns() / 1_000_000
        ));
        for (cpu, snap) in SNAPSHOTS.iter().enumerate().filter(|(c, _)| answered & (1 << c) != 0) {
            print_cpu("statedump: cpu", cpu, snap);
        }
        for cpu in (0..MAX_CPUS).filter(|c| online & !answered & (1 << c) != 0) {
            emergency_line(format_args!(
                "statedump: cpu {cpu} did not answer the NMI within half a second"
            ));
        }
        // The other CPUs went on after their snapshot: a lock still held
        // after 100 ms of trying is probably what everything waits for.
        let held = |try_it: &dyn Fn() -> bool| {
            let start = crate::time::raw_monotonic_ns();
            loop {
                if try_it() {
                    return "free";
                }
                if crate::time::raw_monotonic_ns().saturating_sub(start) > NS / 10 {
                    return "HELD";
                }
                core::hint::spin_loop();
            }
        };
        let sched = held(&|| {
            crate::sched::try_cpu_threads(|cpu, id, name, state| {
                emergency_line(format_args!(
                    "statedump: cpu {cpu} runs thread {id} ({name}) {state}"
                ));
            })
        });
        let heap = held(&|| crate::mm::heap::try_stats().is_some());
        let frames = held(&|| crate::mm::try_frame_stats().is_some());
        let console = held(&|| crate::arch::serial::COM1.try_lock_untracked().is_some());
        emergency_line(format_args!(
            "statedump: locks: scheduler {sched}, heap {heap}, frames {frames}, console {console}"
        ));
        super::stats_line(crate::mm::heap::try_stats(), crate::mm::try_frame_stats());
        crate::flight::dump(u64::MAX, 64, emergency_line);
        emergency_line(format_args!(
            "statedump: NMI part done; the threads follow from the boot thread if it still runs"
        ));
    }
}

/// Host builds: no NMIs.
#[cfg(any(test, feature = "hosted"))]
pub fn init() {}
