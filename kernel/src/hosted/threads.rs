//! gala-um's threads, waits and clock: one host thread per NT thread (and
//! per NT-layer helper thread), a mutex and condition variable per thread
//! for `prepare_wait`/`wait`/`wake`, and the host's clocks.

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
}

static SLOTS: Mutex<Vec<Arc<Slot>>> = Mutex::new(Vec::new());

std::thread_local! {
    static CURRENT: std::cell::Cell<Option<ThreadId>> = const { std::cell::Cell::new(None) };
}

fn new_id() -> ThreadId {
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

/// Starts `entry(arg)` on a new host thread; `setup` runs first on it.
pub fn spawn(
    name: &'static str,
    entry: fn(usize),
    arg: usize,
    setup: impl FnOnce() + Send + 'static,
) -> Result<ThreadId, SpawnError> {
    let id = new_id();
    std::thread::Builder::new()
        .name(name.into())
        .stack_size(HOST_STACK)
        .spawn(move || {
            CURRENT.with(|c| c.set(Some(id)));
            if let Some(s) = slot(id) {
                // SAFETY: pthread_self has no preconditions.
                *s.pthread.lock().unwrap() = unsafe { libc::pthread_self() };
            }
            setup();
            entry(arg);
            exit()
        })
        .map_err(|_| SpawnError::OutOfMemory)?;
    Ok(id)
}

/// Ends the calling thread. The host thread stays parked (it may be
/// inside a signal handler, where it cannot unwind); its id is never
/// reused, as in the kernel.
pub fn exit() -> ! {
    loop {
        std::thread::park();
    }
}

pub fn prepare_wait(deadline: Option<u64>) {
    if let Some(s) = slot(current()) {
        *s.state.lock().unwrap() = WaitState { woken: false, deadline };
    }
}

pub fn wait() -> WakeReason {
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
    start().0.elapsed().as_nanos() as u64
}

/// Milliseconds since start (TICK_HZ = 1000, as in the kernel).
pub fn ticks() -> u64 {
    monotonic_ns() / 1_000_000
}

pub fn boot_system_time() -> u64 {
    start().1
}

pub fn system_time() -> u64 {
    boot_system_time() + monotonic_ns() / 100
}

/// Calls `callback(ticks)` every millisecond on a host thread.
pub fn start_ticks(callback: fn(u64)) {
    let _ = std::thread::Builder::new().name("tick".into()).spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(1));
            callback(ticks());
        }
    });
}
