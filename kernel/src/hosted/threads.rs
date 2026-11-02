//! gala-um's threads, waits and clock: one host thread per NT thread (and
//! per NT-layer helper thread), a mutex and condition variable per thread
//! for `prepare_wait`/`wait`/`wake`, and the host's clocks. With `--sim`
//! the waits, the clock and the processor numbers come from `sim.rs`
//! instead (one thread at a time, virtual time).

use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::vec::Vec;

use crate::sched::{SpawnError, ThreadId, WakeReason};

/// Host stack of each thread: the NT layer's code runs here (system calls
/// run on the signal stack, see entry.rs).
const HOST_STACK: usize = 1 << 20;

#[derive(Default)]
struct WaitState {
    woken: bool,
    deadline: Option<u64>,
}

#[derive(Default)]
struct Slot {
    state: Mutex<WaitState>,
    wake: Condvar,
    /// The host thread (0 until it has started).
    pthread: Mutex<libc::pthread_t>,
    /// Processor affinity (NT mask; 0: all), applied when it starts.
    affinity: Mutex<u64>,
}

static SLOTS: Mutex<Vec<Arc<Slot>>> = Mutex::new(Vec::new());

std::thread_local! {
    static CURRENT: std::cell::Cell<Option<ThreadId>> = const { std::cell::Cell::new(None) };
}

fn new_id() -> ThreadId {
    // Host code: std allocates infallibly (no injected heap failures).
    let _host = super::HostCode::enter();
    let mut slots = SLOTS.lock().unwrap();
    slots.push(Arc::new(Slot::default()));
    // Id 0 is never used (the kernel's boot thread).
    ThreadId::from_raw(slots.len())
}

fn slot(id: ThreadId) -> Option<Arc<Slot>> {
    SLOTS.lock().unwrap().get(id.as_usize().checked_sub(1)?).cloned()
}

/// The calling host thread's id (one is assigned on first use, e.g. to
/// the main thread that waits for the program).
pub fn current() -> ThreadId {
    CURRENT.with(|c| match c.get() {
        Some(id) => id,
        None => {
            let id = new_id();
            c.set(Some(id));
            id
        }
    })
}

/// The calling host thread's id, if it has one (no new id is made).
pub fn current_if_known() -> Option<ThreadId> {
    CURRENT.with(core::cell::Cell::get)
}

/// Starts `entry(arg)` on a new host thread; `setup` runs first on it.
/// `user`: a program's thread (not an NT-layer helper), for the
/// simulation's idle limit.
pub fn spawn(
    name: &'static str,
    entry: fn(usize),
    arg: usize,
    user: bool,
    setup: impl FnOnce() + Send + 'static,
) -> Result<ThreadId, SpawnError> {
    let id = new_id();
    if super::sim::on() {
        super::sim::spawned(id, user, name);
    }
    let _host = super::HostCode::enter();
    std::thread::Builder::new()
        .name(name.into())
        .stack_size(HOST_STACK)
        .spawn(move || {
            CURRENT.with(|c| c.set(Some(id)));
            if let Some(s) = slot(id) {
                // SAFETY: pthread_self has no preconditions.
                *s.pthread.lock().unwrap() = unsafe { libc::pthread_self() };
            }
            // Simulation: nothing more runs before this thread's turn.
            if super::sim::on() {
                super::sim::enter(id);
            }
            if let Some(s) = slot(id) {
                // Always: with no affinity of its own the thread still runs
                // only on the processors the program sees (`--cpus`), so
                // RDTSCP never reports one it does not have.
                let mask = *s.affinity.lock().unwrap();
                pin(id, &s, mask);
            }
            setup();
            super::nt_code(|| entry(arg));
            exit()
        })
        .map_err(|_| {
            if super::sim::on() {
                super::sim::forget(id);
            }
            SpawnError::OutOfMemory
        })?;
    Ok(id)
}

/// Ends the calling thread. The host thread stays parked (it may be
/// inside a signal handler, where it cannot unwind); its id is never
/// reused, as in the kernel.
pub fn exit() -> ! {
    if super::sim::on() {
        super::sim::exit();
    }
    loop {
        std::thread::park();
    }
}

pub fn prepare_wait(deadline: Option<u64>) {
    if super::sim::on() {
        return super::sim::prepare_wait(deadline);
    }
    if let Some(s) = slot(current()) {
        *s.state.lock().unwrap() = WaitState { woken: false, deadline };
    }
}

pub fn wait() -> WakeReason {
    if super::sim::on() {
        return super::sim::wait();
    }
    let Some(s) = slot(current()) else { return WakeReason::Woken };
    let mut state = s.state.lock().unwrap();
    loop {
        if state.woken {
            state.woken = false;
            return WakeReason::Woken;
        }
        match state.deadline {
            Some(d) => {
                let now = ticks();
                if now >= d {
                    return WakeReason::TimedOut;
                }
                let ms = Duration::from_millis(d - now);
                state = s.wake.wait_timeout(state, ms).unwrap().0;
            }
            None => state = s.wake.wait(state).unwrap(),
        }
    }
}

pub fn wake(id: ThreadId) {
    if super::sim::on() {
        return super::sim::wake(id);
    }
    if let Some(s) = slot(id) {
        s.state.lock().unwrap().woken = true;
        s.wake.notify_all();
    }
}

/// Sends `signal` to thread `id`'s host thread, if it has started.
pub fn signal(id: ThreadId, signal: i32) {
    let Some(s) = slot(id) else { return };
    let pthread = *s.pthread.lock().unwrap();
    if pthread != 0 {
        // SAFETY: a host thread that never exits (see `exit`).
        unsafe { libc::pthread_kill(pthread, signal) };
    }
}

/// Processor time used by thread `id`'s host thread, if it has started.
pub fn cpu_time_ns(id: ThreadId) -> Option<u64> {
    let pthread = *slot(id)?.pthread.lock().unwrap();
    if pthread == 0 {
        return None;
    }
    let mut clock: libc::clockid_t = 0;
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: a host thread that never exits (see `exit`); valid pointers.
    let ok = unsafe {
        libc::pthread_getcpuclockid(pthread, &mut clock) == 0
            && libc::clock_gettime(clock, &mut ts) == 0
    };
    ok.then(|| ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64)
}

/// Processors: NT processor n is host CPU n, for the CPUs 0..n that the
/// process may run on (the contiguous start of its affinity, at most 64;
/// one if that is empty), so RDTSCP's processor number (Linux's TSC_AUX)
/// and NtGetCurrentProcessorNumber agree, as on Windows.
pub fn cpu_count() -> u32 {
    *CPUS.get_or_init(|| {
        // SAFETY: a zeroed cpu_set_t filled by the kernel.
        let mask = unsafe {
            let mut set: libc::cpu_set_t = core::mem::zeroed();
            if libc::sched_getaffinity(0, core::mem::size_of_val(&set), &mut set) != 0 {
                return 1;
            }
            (0..64).filter(|&i| libc::CPU_ISSET(i, &set)).fold(0u64, |m, i| m | 1 << i)
        };
        mask.trailing_ones().max(1).min(CPU_LIMIT.load(core::sync::atomic::Ordering::Relaxed))
    })
}

static CPUS: OnceLock<u32> = OnceLock::new();
static CPU_LIMIT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(64);

/// Shows at most `n` processors (before the program starts), e.g. as many
/// as the QEMU boot tests have.
pub fn limit_cpus(n: u32) {
    CPU_LIMIT.store(n.clamp(1, 64), core::sync::atomic::Ordering::Relaxed);
}

pub fn online_mask() -> u64 {
    match cpu_count() {
        64 => u64::MAX,
        n => (1 << n) - 1,
    }
}

pub fn current_cpu() -> u32 {
    if super::sim::on() {
        let id = current();
        return slot(id).map_or(0, |s| sim_cpu(id, *s.affinity.lock().unwrap()));
    }
    // SAFETY: no preconditions.
    let cpu = unsafe { libc::sched_getcpu() };
    if cpu >= 0 && (cpu as u32) < cpu_count() { cpu as u32 } else { 0 }
}

/// Restricts thread `id` to the processors in `mask` (now if it runs,
/// else when it starts).
pub fn set_affinity(id: ThreadId, mask: u64) {
    if let Some(s) = slot(id) {
        *s.affinity.lock().unwrap() = mask;
        if *s.pthread.lock().unwrap() != 0 {
            pin(id, &s, mask);
        }
    }
}

/// Simulation: the one processor thread `id` runs on, chosen from its
/// affinity `mask` (0: all) by its id, so it does not depend on the host.
fn sim_cpu(id: ThreadId, mask: u64) -> u32 {
    let allowed = match mask & online_mask() {
        0 => online_mask(),
        m => m,
    };
    let k = id.as_usize() % allowed.count_ones() as usize;
    (0..64).filter(|i| allowed & (1 << i) != 0).nth(k).unwrap_or(0)
}

fn pin(id: ThreadId, s: &Slot, mask: u64) {
    let pthread = *s.pthread.lock().unwrap();
    // Simulation: one processor per thread (RDTSCP's TSC_AUX then agrees
    // with `current_cpu`).
    let mask = match mask {
        _ if super::sim::on() => 1 << sim_cpu(id, mask),
        0 => online_mask(),
        m => m,
    };
    // SAFETY: a zeroed cpu_set_t with bits set; a host thread that never
    // exits.
    unsafe {
        let mut set: libc::cpu_set_t = core::mem::zeroed();
        for i in 0..64 {
            if mask & online_mask() & (1 << i) != 0 {
                libc::CPU_SET(i, &mut set);
            }
        }
        libc::pthread_setaffinity_np(pthread, core::mem::size_of_val(&set), &set);
    }
}

static START: OnceLock<(Instant, u64)> = OnceLock::new();

/// 100 ns units between 1601-01-01 and 1970-01-01.
const EPOCH_DIFFERENCE: u64 = 11_644_473_600 * 10_000_000;

fn start() -> &'static (Instant, u64) {
    START.get_or_init(|| {
        let unix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        (Instant::now(), EPOCH_DIFFERENCE + (unix.as_nanos() / 100) as u64)
    })
}

pub fn monotonic_ns() -> u64 {
    if super::sim::on() {
        return super::sim::monotonic_ns();
    }
    start().0.elapsed().as_nanos() as u64
}

/// Milliseconds since start (TICK_HZ = 1000, as in the kernel).
pub fn ticks() -> u64 {
    monotonic_ns() / 1_000_000
}

pub fn boot_system_time() -> u64 {
    if super::sim::on() {
        return super::sim::boot_system_time();
    }
    start().1
}

pub fn system_time() -> u64 {
    boot_system_time() + monotonic_ns() / 100
}

/// Calls `callback(ticks)` every millisecond on a host thread.
pub fn start_ticks(callback: fn(u64)) {
    if super::sim::on() {
        // Called at scheduling points as virtual time passes.
        return super::sim::set_tick_callback(callback);
    }
    let _host = super::HostCode::enter();
    let _ = std::thread::Builder::new().name("tick".into()).spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(1));
            super::nt_code(|| callback(ticks()));
        }
    });
}
