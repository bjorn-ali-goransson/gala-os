//! Timers (SetTimer/KillTimer): WM_TIMER is not posted but made when the
//! queue is asked for a message and a timer of the calling thread is due,
//! after posted messages and WM_PAINT, as on Windows; one WM_TIMER per due
//! timer however late (they do not pile up). GetMessage sleeps until the
//! next timer is due. A TIMERPROC goes in WM_TIMER's lParam and
//! DispatchMessage calls it instead of the window procedure. Intervals
//! are clamped to USER_TIMER_MINIMUM (10 ms) and USER_TIMER_MAXIMUM.
//!
//! A window's timer belongs to the window's thread, whichever thread of
//! the process sets or kills it (Wine's msg.c test_timers, run on
//! Windows: a second thread kills the window's timer and sets another,
//! which the window's thread then kills); a window of another process is
//! ERROR_ACCESS_DENIED (Wine's server). A timer without a window is the
//! calling thread's. ODT's setup.exe sets its splash's timer from its
//! main thread.

use crate::window::{Hwnd, Msg, me};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetTickCount() -> u32;
}

pub const WM_TIMER: u32 = 0x0113;
const MAX_TIMERS: usize = 128;

#[derive(Clone, Copy)]
struct Timer {
    used: bool,
    hwnd: Hwnd,
    id: usize,
    thread: u32,
    interval: u32,
    /// GetTickCount when it is due next.
    due: u32,
    proc_: usize,
}

const NO_TIMER: Timer =
    Timer { used: false, hwnd: 0, id: 0, thread: 0, interval: 0, due: 0, proc_: 0 };

struct Table {
    lock: AtomicBool,
    timers: core::cell::UnsafeCell<[Timer; MAX_TIMERS]>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table = Table {
    lock: AtomicBool::new(false),
    timers: core::cell::UnsafeCell::new([NO_TIMER; MAX_TIMERS]),
};
/// Ids for timers without a window.
static NEXT_ID: AtomicUsize = AtomicUsize::new(0x7ffe_0001);

fn with<R>(f: impl FnOnce(&mut [Timer; MAX_TIMERS]) -> R) -> R {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *TABLE.timers.get() });
    TABLE.lock.store(false, Ordering::Release);
    r
}

fn now() -> u32 {
    // SAFETY: a plain call.
    unsafe { GetTickCount() }
}

/// Whether tick `t` has come (wrapping).
fn reached(t: u32, now: u32) -> bool {
    now.wrapping_sub(t) < 0x8000_0000
}

/// SetTimer(hwnd, id, elapse, proc): the timer's id (for a window: `id`;
/// without one: a new id), replacing the window's timer of that id; 0 for
/// a window that is not one or is another process's.
#[unsafe(no_mangle)]
pub extern "system" fn SetTimer(hwnd: Hwnd, id: usize, elapse: u32, proc_: usize) -> usize {
    let Some(thread) = owner(hwnd) else { return 0 };
    let interval = elapse.clamp(10, 0x7fff_ffff);
    let id = if hwnd == 0 {
        // An existing window-less timer of this thread keeps its id.
        if id != 0
            && with(|t| t.iter().any(|x| x.used && x.hwnd == 0 && x.id == id && x.thread == thread))
        {
            id
        } else {
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        }
    } else {
        id
    };
    let due = now().wrapping_add(interval);
    with(|t| {
        let slot = t
            .iter()
            .position(|x| x.used && x.hwnd == hwnd && x.id == id && x.thread == thread)
            .or_else(|| t.iter().position(|x| !x.used))?;
        t[slot] = Timer { used: true, hwnd, id, thread, interval, due, proc_ };
        Some(id)
    })
    .inspect(|_| {
        // The window's thread may be waiting in GetMessage for its own
        // next timer: it looks again.
        if thread != me() {
            crate::window::wake();
        }
    })
    .unwrap_or(0)
}

/// The thread a timer of `hwnd` belongs to: the calling thread's for no
/// window, else the window's; None (the last error set) for a window
/// that is not one or is another process's.
fn owner(hwnd: Hwnd) -> Option<u32> {
    if hwnd == 0 {
        return Some(me());
    }
    if let Some(w) = crate::window::window(hwnd) {
        return Some(w.thread);
    }
    crate::window::set_last_error(if crate::window::IsWindow(hwnd) != 0 {
        5 // ERROR_ACCESS_DENIED: another process's window
    } else {
        1400 // ERROR_INVALID_WINDOW_HANDLE
    });
    None
}

/// SetCoalescableTimer(hwnd, id, elapse, proc, tolerance): SetTimer; the
/// tolerance (how late the system may fire it to batch timers) is not
/// used: gala fires every timer when it is due, which any tolerance
/// allows. Office's Click-to-Run client sets its timers this way.
#[unsafe(no_mangle)]
pub extern "system" fn SetCoalescableTimer(
    hwnd: Hwnd,
    id: usize,
    elapse: u32,
    proc_: usize,
    _tolerance: u32,
) -> usize {
    SetTimer(hwnd, id, elapse, proc_)
}

/// KillTimer: TRUE if there was such a timer (a window's, set by any
/// thread of the process; a window-less one of the calling thread's).
#[unsafe(no_mangle)]
pub extern "system" fn KillTimer(hwnd: Hwnd, id: usize) -> i32 {
    let Some(thread) = owner(hwnd) else { return 0 };
    with(|t| {
        match t.iter_mut().find(|x| x.used && x.hwnd == hwnd && x.id == id && x.thread == thread) {
            Some(x) => {
                *x = NO_TIMER;
                1
            }
            None => 0,
        }
    })
}

/// The window's timers go with it.
pub fn window_destroyed(hwnd: Hwnd) {
    with(|t| {
        for x in t.iter_mut().filter(|x| x.used && x.hwnd == hwnd) {
            *x = NO_TIMER;
        }
    });
}

/// The thread ended: its timers go (a window's went with the window).
pub fn thread_ended(thread: u32) {
    with(|t| {
        for x in t.iter_mut().filter(|x| x.used && x.thread == thread) {
            *x = NO_TIMER;
        }
    });
}

/// A due timer's WM_TIMER for the calling thread passing `fits`; with
/// `remove`, the timer's next due time moves on.
pub fn due(fits: impl Fn(&Msg) -> bool, remove: bool) -> Option<Msg> {
    let (thread, now) = (me(), now());
    with(|t| {
        for x in t.iter_mut().filter(|x| x.used && x.thread == thread && reached(x.due, now)) {
            let m = crate::window::message(x.hwnd, WM_TIMER, x.id, x.proc_ as isize);
            if fits(&m) {
                if remove {
                    x.due = now.wrapping_add(x.interval);
                }
                return Some(m);
            }
        }
        None
    })
}

/// Milliseconds until the calling thread's next timer (INFINITE without).
pub fn wait_ms() -> u32 {
    let (thread, now) = (me(), now());
    with(|t| {
        t.iter()
            .filter(|x| x.used && x.thread == thread)
            .map(|x| if reached(x.due, now) { 0 } else { x.due.wrapping_sub(now) })
            .min()
            .unwrap_or(0xffff_ffff)
    })
}
