//! gala-um's deterministic simulation (`um --sim=<seed>`, ROADMAP R5):
//! one baton for all NT threads, handed on at scheduling points (system
//! call entry and exit, waits, yields, sleeps, thread exit, the release of
//! a thread's last IrqMutex) to a thread chosen by
//! `crate::sim::Scheduler`'s seeded PRNG, and a virtual clock that
//! advances only there. Design and limits: docs/ARCHITECTURE.md,
//! "Deterministic simulation"; use: docs/GALA_UM.md.
//!
//! Every host thread that runs NT code is a simulated thread: the main
//! thread (`run`), the NT layer's helpers (timers, net) and the program's
//! threads. A thread not holding the baton waits on its own condition
//! variable. The tick callback (KUSER_SHARED_DATA's times) runs on the
//! baton holder when a scheduling point moves virtual time past a tick,
//! instead of on a host timer thread. Program code reading the clock
//! (KUSER_SHARED_DATA, RDTSC) and CPUID trap (entry.rs); the program's
//! random bytes (GalaRandom) come from the seed; a thread spinning in
//! program code is preempted at its loop's canonical point.
//!
//! Off (the default), each entry point is one relaxed load of `ON`.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::Duration;
use std::vec::Vec;

use crate::sched::{ThreadId, WakeReason};
use crate::sim::{NS_PER_TICK, Next, Scheduler, State};

static ON: AtomicBool = AtomicBool::new(false);
/// Virtual time (ns), mirrored from the scheduler for lock-free reads.
static NOW: AtomicU64 = AtomicU64::new(crate::sim::START_NS);
/// Scheduling points so far (the watchdog watches it).
static STEPS: AtomicU64 = AtomicU64::new(0);
/// The baton holder's id (the watchdog's target).
static RUNNING: AtomicUsize = AtomicUsize::new(0);
/// Preemptions by the watchdog (a thread ran program code for 10 ms
/// of processor time without a system call): these are not reproducible.
static PREEMPTIONS: AtomicU64 = AtomicU64::new(0);
/// Preemptions of spinning threads at their loop's canonical point
/// (reproducible).
static SPIN_PREEMPTIONS: AtomicU64 = AtomicU64::new(0);
static TICK_CALLBACK: OnceLock<fn(u64)> = OnceLock::new();
/// The tick the callback last saw.
static LAST_TICK: AtomicU64 = AtomicU64::new(crate::sim::START_NS / NS_PER_TICK);

struct Sim {
    sched: Scheduler,
    /// GalaRandom's numbers: a stream of their own from the seed, so
    /// drawing them does not change the scheduler's choices.
    random: crate::sim::Rng,
    /// Per thread id: where it waits for the baton.
    baton: Vec<Option<Arc<Condvar>>>,
    names: Vec<&'static str>,
}

static SIM: Mutex<Option<Sim>> = Mutex::new(None);

/// Whether `--sim` is on.
#[inline]
pub fn on() -> bool {
    ON.load(Relaxed)
}

/// Separates the random numbers' stream from the scheduler's.
const RANDOM_STREAM: u64 = 0x5241_4e44_4f4d_2121;

/// Random numbers from the seed (GalaRandom under `--sim`): only the
/// baton holder asks, so the sequence repeats with the seed.
pub fn fill_random(buf: &mut [u8]) {
    let mut guard = lock();
    let Some(sim) = guard.as_mut() else { return };
    for chunk in buf.chunks_mut(8) {
        chunk.copy_from_slice(&sim.random.next_u64().to_le_bytes()[..chunk.len()]);
    }
}

/// Turns the simulation on with `seed` (before `run`; um's `--sim=`).
pub fn enable(seed: u64) {
    let _host = super::HostCode::enter();
    *lock() = Some(Sim {
        sched: Scheduler::new(seed),
        random: crate::sim::Rng::new(seed ^ RANDOM_STREAM),
        baton: Vec::new(),
        names: Vec::new(),
    });
    ON.store(true, Relaxed);
}

/// The simulation's state, locked. While held, IrqMutex releases (an
/// allocation's frame allocator, say) are not lock points: one would
/// take this lock again.
struct Guard(Option<MutexGuard<'static, Option<Sim>>>);

impl core::ops::Deref for Guard {
    type Target = Option<Sim>;
    fn deref(&self) -> &Option<Sim> {
        self.0.as_ref().expect("held")
    }
}

impl core::ops::DerefMut for Guard {
    fn deref_mut(&mut self) -> &mut Option<Sim> {
        self.0.as_mut().expect("held")
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.0 = None;
        NO_POINTS.with(|n| n.set(n.get() - 1));
    }
}

fn lock() -> Guard {
    NO_POINTS.with(|n| n.set(n.get() + 1));
    Guard(Some(SIM.lock().unwrap_or_else(|e| e.into_inner())))
}

fn register(sim: &mut Sim, id: usize, user: bool, name: &'static str) {
    if sim.baton.len() <= id {
        sim.baton.resize(id + 1, None);
        sim.names.resize(id + 1, "");
    }
    sim.baton[id] = Some(Arc::new(Condvar::new()));
    sim.names[id] = name;
    sim.sched.add(id, user);
}

/// The calling thread (gala-um's main thread) takes the baton: called by
/// `run` before the NT layer starts. Also starts the watchdog.
pub fn start() {
    let id = super::threads::current().as_usize();
    {
        let _host = super::HostCode::enter();
        let mut guard = lock();
        let Some(sim) = guard.as_mut() else { return };
        register(sim, id, false, "main");
        RUNNING.store(id, Relaxed);
    }
    start_watchdog();
}

/// A thread was created (by the baton holder); it is runnable and its
/// host thread calls `enter` before running NT code.
pub fn spawned(id: ThreadId, user: bool, name: &'static str) {
    let _host = super::HostCode::enter();
    if let Some(sim) = lock().as_mut() {
        register(sim, id.as_usize(), user, name);
    }
}

/// A thread whose host thread could not be created.
pub fn forget(id: ThreadId) {
    if let Some(sim) = lock().as_mut() {
        sim.sched.remove(id.as_usize());
    }
}

/// First thing on a new thread's host thread: waits for the baton.
pub fn enter(id: ThreadId) {
    let guard = lock();
    let guard = wait_for_baton(guard, id.as_usize());
    drop(guard);
    after_switch();
}

fn wait_for_baton(mut guard: Guard, me: usize) -> Guard {
    let Some(cv) = guard.as_ref().and_then(|s| s.baton.get(me).cloned().flatten()) else {
        return guard;
    };
    while guard.as_ref().is_some_and(|s| s.sched.running != Some(me)) {
        let inner = guard.0.take().expect("held");
        guard.0 = Some(cv.wait(inner).unwrap_or_else(|e| e.into_inner()));
    }
    guard
}

/// Chooses the next thread and hands it the baton; then, if `me` is
/// still alive, waits until the baton comes back.
fn switch(guard: Guard, me: Option<usize>) -> Guard {
    switch_avoiding(guard, me, None)
}

/// `switch`, choosing another thread than `avoid` if one can run.
fn switch_avoiding(mut guard: Guard, me: Option<usize>, avoid: Option<usize>) -> Guard {
    let Some(sim) = guard.as_mut() else { return guard };
    match sim.sched.pick_avoiding(avoid) {
        Next::Run(next) => {
            NOW.store(sim.sched.now_ns, Relaxed);
            STEPS.store(sim.sched.steps, Relaxed);
            RUNNING.store(next, Relaxed);
            if Some(next) != me
                && let Some(Some(cv)) = sim.baton.get(next)
            {
                cv.notify_one();
            }
        }
        Next::Deadlock => {
            drop(guard);
            deadlock();
        }
    }
    match me {
        Some(me) => wait_for_baton(guard, me),
        None => guard,
    }
}

/// After getting the baton: the tick callback, if virtual time moved past
/// a tick (as the kernel's timer interrupt would have run it).
fn after_switch() {
    let tick = NOW.load(Relaxed) / NS_PER_TICK;
    if LAST_TICK.load(Relaxed) != tick {
        LAST_TICK.store(tick, Relaxed);
        if let Some(callback) = TICK_CALLBACK.get() {
            // An interrupt handler: no preemption inside it.
            without_lock_points(|| super::nt_code(|| callback(tick)));
        }
    }
}

fn me() -> usize {
    super::threads::current().as_usize()
}

std::thread_local! {
    /// IrqMutexes this thread holds (the kernel's "interrupts off").
    static LOCKS: core::cell::Cell<u32> = const { core::cell::Cell::new(0) };
    /// Inside the simulation's own code that runs NT code (the tick
    /// callback, the deadlock dump): no lock points.
    static NO_POINTS: core::cell::Cell<u32> = const { core::cell::Cell::new(0) };
    /// Spins on a held IrqMutex since the last acquisition.
    static SPINS: core::cell::Cell<u64> = const { core::cell::Cell::new(0) };
}

fn without_lock_points<R>(f: impl FnOnce() -> R) -> R {
    NO_POINTS.with(|n| n.set(n.get() + 1));
    let r = f();
    NO_POINTS.with(|n| n.set(n.get() - 1));
    r
}

/// An IrqMutex was taken (sync.rs, with the simulation on).
pub fn lock_taken() {
    LOCKS.with(|n| n.set(n.get() + 1));
    SPINS.with(|s| s.set(0));
}

/// An IrqMutex taken with the simulation on was released; the last one
/// is a lock point.
pub fn lock_released() {
    let left = LOCKS.with(|n| {
        n.set(n.get().saturating_sub(1));
        n.get()
    });
    if left == 0 {
        lock_point();
    }
}

/// Of the lock points (the last IrqMutex released, where the kernel's
/// pending timer interrupt would preempt), one in LOCK_POINT_ODDS
/// switches threads, chosen by the PRNG: interleavings inside system
/// calls, at lock granularity, as two processors would make them. Not
/// recorded in the trace and no virtual time passes.
pub fn lock_point() {
    if NO_POINTS.with(core::cell::Cell::get) > 0 {
        return;
    }
    let Some(me) = super::threads::current_if_known() else { return };
    let me = me.as_usize();
    let mut guard = lock();
    let Some(sim) = guard.as_mut() else { return };
    if sim.sched.running != Some(me) || !sim.sched.chance(LOCK_POINT_ODDS) {
        return;
    }
    sim.sched.yield_now(me);
    drop(switch(guard, Some(me)));
    after_switch();
}

/// One lock point in this many switches threads.
const LOCK_POINT_ODDS: u64 = 8;

/// A thread spins on a held IrqMutex. Only one thread runs, and lock
/// points come only after a thread's last lock, so the holder must have
/// passed a scheduling point (a wait, a system call's end) with the lock
/// held: a bug (in the kernel: sleeping with interrupts off). Reported
/// after a second or so.
pub fn lock_contended() {
    let spins = SPINS.with(|s| {
        s.set(s.get() + 1);
        s.get()
    });
    if spins == 1_000_000 {
        let _host = super::HostCode::enter();
        std::eprintln!(
            "sim: a lock is held by a thread that is not running (held across a wait or a \
             system call's end?); here:\n{}",
            std::backtrace::Backtrace::force_capture()
        );
        deadlock();
    }
}

/// A scheduling point of the running thread: `event` goes into the trace
/// (a system call's number or result), virtual time advances one step,
/// and the PRNG chooses who runs next (maybe the same thread).
pub fn point(event: &[u64]) {
    let me = me();
    let mut guard = lock();
    let Some(sim) = guard.as_mut() else { return };
    if !event.is_empty() {
        let mut words = [0u64; 4];
        words[0] = me as u64;
        let n = event.len().min(3);
        words[1..=n].copy_from_slice(&event[..n]);
        sim.sched.record(&words[..=n]);
    }
    sim.sched.step();
    sim.sched.yield_now(me);
    drop(switch(guard, Some(me)));
    after_switch();
}

/// `Threads::yield_now` (NtYieldExecution, a zero delay): the others go
/// first under priorities.
pub fn yield_now() {
    if let Some(me) = super::threads::current_if_known()
        && let Some(sim) = lock().as_mut()
    {
        sim.sched.demote(me.as_usize());
    }
    point(&[]);
}

pub fn prepare_wait(deadline_ticks: Option<u64>) {
    let me = me();
    if let Some(sim) = lock().as_mut() {
        sim.sched.prepare(me, deadline_ticks.map(|t| t.saturating_mul(NS_PER_TICK)));
    }
}

pub fn wait() -> WakeReason {
    let me = me();
    let mut guard = lock();
    let Some(sim) = guard.as_mut() else { return WakeReason::Woken };
    sim.sched.step();
    sim.sched.block(me);
    guard = switch(guard, Some(me));
    let (woken, _) = guard.as_mut().map_or((true, false), |s| s.sched.wait_result(me));
    drop(guard);
    after_switch();
    if woken { WakeReason::Woken } else { WakeReason::TimedOut }
}

pub fn wake(id: ThreadId) {
    if let Some(sim) = lock().as_mut() {
        sim.sched.wake(id.as_usize());
    }
}

pub fn sleep_ms(ms: u64) {
    prepare_wait(Some(ticks() + ms.max(1)));
    wait();
}

/// The calling thread ends: the baton goes on; the caller then parks
/// for good.
pub fn exit() {
    let me = me();
    let mut guard = lock();
    if let Some(sim) = guard.as_mut() {
        sim.sched.exit(me);
        drop(switch(guard, None));
    }
}

/// The running thread spun in program code (no system call for 10 ms
/// of processor time, the watchdog found): it is preempted, if another
/// program thread could run (else the thread computes, it does not wait
/// for anyone). `reproducible`: at its spin loop's canonical point
/// (`crate::sim::SpinCapture`, entry.rs), the same in every run; else
/// where the watchdog interrupted it, which depends on the host's timing
/// (counted and reported). Virtual time jumps to the next deadline (the
/// spinner is taken to have used the time until then: a thread polling
/// with Sleep(1) for its progress gets its turn) and another thread runs.
/// Not recorded in the trace.
pub fn preempt(reproducible: bool) {
    let me = me();
    {
        let mut guard = lock();
        let Some(sim) = guard.as_mut() else { return };
        if !sim.sched.others_could_run(me) {
            return;
        }
        if reproducible {
            SPIN_PREEMPTIONS.fetch_add(1, Relaxed);
        } else {
            PREEMPTIONS.fetch_add(1, Relaxed);
        }
        sim.sched.skip_to_next_deadline();
        sim.sched.step();
        sim.sched.yield_now(me);
        // Another thread: the spinner would only spin on.
        drop(switch_avoiding(guard, Some(me), Some(me)));
    }
    after_switch();
}

/// Scheduling points so far, as of the last switch: a thread's spin
/// episode is the stretch in which this does not change.
pub fn steps() -> u64 {
    STEPS.load(Relaxed)
}

/// Whether the watchdog should preempt thread `id` (see `preempt`).
fn worth_preempting(id: usize) -> bool {
    lock().as_ref().is_some_and(|s| s.sched.running == Some(id) && s.sched.others_could_run(id))
}

/// RDTSC in a simulated thread (it faults, see entry.rs): the virtual
/// clock in ns (a 1 GHz counter), and a step of virtual time passes, so
/// a loop that waits for the counter to move ends. Deterministic: only
/// the baton holder runs instructions.
pub fn read_tsc() -> u64 {
    let mut guard = lock();
    let Some(sim) = guard.as_mut() else { return NOW.load(Relaxed) };
    sim.sched.step();
    NOW.store(sim.sched.now_ns, Relaxed);
    sim.sched.now_ns
}

/// Program code read KUSER_SHARED_DATA (its page is inaccessible in the
/// simulation, so every read faults; entry.rs): a step of virtual time
/// passes and, one read in `CLOCK_READ_ODDS` (by the PRNG), another thread
/// may run; the page's times follow when time passes a tick. A
/// loop that polls the time without system calls (Go's spin-waits) sees
/// it move and ends, the same way under the same seed.
pub fn clock_read() {
    let Some(me) = super::threads::current_if_known() else { return };
    let me = me.as_usize();
    {
        let mut guard = lock();
        let Some(sim) = guard.as_mut() else { return };
        if sim.sched.running != Some(me) {
            return;
        }
        let give_up = sim.sched.clock_read();
        NOW.store(sim.sched.now_ns, Relaxed);
        if give_up {
            sim.sched.yield_now(me);
            guard = switch(guard, Some(me));
        }
        drop(guard);
    }
    // KUSER_SHARED_DATA's times, if time passed a tick (they change at
    // ticks, as on Windows).
    after_switch();
}

pub fn monotonic_ns() -> u64 {
    NOW.load(Relaxed)
}

pub fn ticks() -> u64 {
    NOW.load(Relaxed) / NS_PER_TICK
}

pub fn boot_system_time() -> u64 {
    crate::sim::BOOT_SYSTEM_TIME
}

pub fn system_time() -> u64 {
    crate::sim::BOOT_SYSTEM_TIME + NOW.load(Relaxed) / 100
}

pub fn set_tick_callback(callback: fn(u64)) {
    let _ = TICK_CALLBACK.set(callback);
}

/// Clock control (`crate::clockctl`) in the simulation: the clock is
/// virtual already, so a rate or a jump applies to it directly (a frozen
/// clock reaches no deadline: every thread waiting for one is a
/// deadlock); the system time's offset is clockctl's.
pub fn clock_control(op: crate::clockctl::Op) -> crate::clockctl::State {
    use crate::clockctl::{Op, State};
    let (now, rate, advanced, real) = {
        let mut guard = lock();
        let Some(sim) = guard.as_mut() else {
            return State {
                rate: crate::clockctl::Rate::REAL,
                advanced: 0,
                system_offset: 0,
                raw: 0,
                simulated: true,
            };
        };
        match op {
            Op::Advance(ns) => sim.sched.advance(ns),
            Op::SetRate(rate) => sim.sched.rate = rate,
            _ => {}
        }
        NOW.store(sim.sched.now_ns, Relaxed);
        (sim.sched.now_ns, sim.sched.rate, sim.sched.advanced_ns, sim.sched.real_ns)
    };
    let offset = match op {
        Op::ShiftSystem(delta) => crate::clockctl::system_offset().saturating_add(delta),
        Op::SetSystem(target) => target as i64 - (crate::sim::BOOT_SYSTEM_TIME + now / 100) as i64,
        _ => crate::clockctl::system_offset(),
    };
    if matches!(op, Op::ShiftSystem(_) | Op::SetSystem(_)) {
        crate::clockctl::set_system_offset(offset);
    }
    State { rate, advanced, system_offset: offset, raw: real, simulated: true }
}

/// The simulation's (unscaled, virtual) clocks for `clockctl::arm`.
pub fn clocks() -> (u64, u64) {
    lock().as_ref().map_or((0, 0), |s| (s.sched.real_ns, s.sched.now_ns))
}

/// The summary line sim-test compares: `sim: seed S trace H`, plus the
/// number of scheduling points and the virtual time.
pub fn report() {
    let guard = lock();
    let Some(sim) = guard.as_ref() else { return };
    let preempted = PREEMPTIONS.load(Relaxed);
    let spins = SPIN_PREEMPTIONS.load(Relaxed);
    std::eprintln!(
        "sim: seed {} trace {:016x} ({} scheduling points, {} ms virtual{}{})",
        sim.sched.seed,
        sim.sched.trace,
        sim.sched.steps,
        sim.sched.now_ns / NS_PER_TICK,
        if spins > 0 {
            alloc::format!("; {spins} spin loops preempted")
        } else {
            alloc::string::String::new()
        },
        if preempted > 0 {
            alloc::format!(
                "; {preempted} watchdog preemptions of threads spinning in program code: \
                 not reproducible"
            )
        } else {
            alloc::string::String::new()
        }
    );
}

/// Processor time a thread may spend in program code without a
/// scheduling point before the watchdog interrupts it (entry.rs then
/// looks for its spin loop and preempts it there).
const WATCHDOG_CPU_NS: u64 = 10_000_000;

/// Exit status of um when the simulation finds a deadlock.
pub const DEADLOCK_EXIT: i32 = 4;

/// Every thread waits and nothing will wake one: prints the seed, every
/// simulated thread's state and the NT threads' waits, and ends the
/// process.
fn deadlock() -> ! {
    let _host = super::HostCode::enter();
    {
        let guard = lock();
        if let Some(sim) = guard.as_ref() {
            std::eprintln!(
                "sim: seed {}: DEADLOCK at {} ms virtual: every thread waits and none has a \
                 deadline (or only the NT layer's helpers ran for {} s)",
                sim.sched.seed,
                sim.sched.now_ns / NS_PER_TICK,
                crate::sim::IDLE_LIMIT_NS / 1_000_000_000
            );
            for (id, t) in sim.sched.threads() {
                if t.state == State::Exited {
                    continue;
                }
                std::eprintln!(
                    "sim:   thread {id} ({}{}): {:?}{}",
                    sim.names.get(id).copied().unwrap_or(""),
                    if t.user { ", program" } else { "" },
                    t.state,
                    t.deadline.map_or(alloc::string::String::new(), |d| alloc::format!(
                        " until {} ms",
                        d / NS_PER_TICK
                    ))
                );
            }
        }
    }
    // The NT layer's view: each thread's system call and wait objects.
    without_lock_points(|| super::nt_code(crate::nt::thread::dump));
    report();
    crate::replay::finish();
    std::process::exit(DEADLOCK_EXIT)
}

/// A host thread (outside the simulation) that interrupts a thread running
/// program code for 10 ms of processor time without a scheduling point (a spin-wait on
/// another thread, which would otherwise never end).
fn start_watchdog() {
    let _host = super::HostCode::enter();
    let _ = std::thread::Builder::new().name("sim-watchdog".into()).spawn(|| {
        // Processor time, not wall time: a host thread waiting for a CPU
        // (parallel runs) is not spinning.
        let (mut last, mut since) = (u64::MAX, 0);
        loop {
            std::thread::sleep(Duration::from_millis(10));
            let steps = STEPS.load(Relaxed);
            let id = ThreadId::from_raw(RUNNING.load(Relaxed));
            let Some(cpu) = super::threads::cpu_time_ns(id) else { continue };
            if steps != last {
                (last, since) = (steps, cpu);
            } else if cpu.saturating_sub(since) >= WATCHDOG_CPU_NS {
                since = cpu;
                if worth_preempting(id.as_usize()) {
                    super::threads::signal(id, super::entry::SIG_INTERRUPT);
                }
            }
        }
    });
}
