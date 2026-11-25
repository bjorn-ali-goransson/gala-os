//! The input state of a thread, and the threads that share one.
//!
//! Windows gives every thread with a message queue an input state of its
//! own -- which window has the keyboard focus, which window is the
//! active one -- and `AttachThreadInput` joins two threads' into one that
//! both then read and write. Office asks for it: Mso40UIwin32client
//! delay-loads `USER32.dll!AttachThreadInput` by way of WWLIB, and
//! WINWORD ends on 0xc06d007f without it, its splash already up
//! (ROADMAP OF).
//!
//! What the calls answer is measured on Windows 11 by
//! `threadinput_test`, and the rules that are not obvious are the ones
//! it reads out of two threads at once:
//!
//! * A thread attached to itself, to or from thread 0, to a thread that
//!   does not exist, and to a thread with no message queue: 0 with
//!   ERROR_INVALID_PARAMETER.
//! * Attaching answers 1 and the joined state is the one the thread
//!   being attached to had: both threads then see its focus and its
//!   active window, and the attached thread's own are gone.
//! * `SetFocus` of another thread's window, the two apart, is 0 with
//!   ERROR_ACCESS_DENIED. Attached, it works, and it answers the window
//!   it focused rather than the window it took the focus from, because
//!   that window's top-level window was not the active one of the shared
//!   state and the call activates it first.
//! * Detaching answers 1 and does not undo the merged state: each thread
//!   keeps the focus and active window the two of them shared.
//!
//! Where gala stops short: the state follows a thread id, which the
//! kernel may reuse once the thread is gone, and only this process's
//! threads can be attached (user32's state is per process). Both are
//! ARCHITECTURE "user32" limits, and the second one is a ledger row.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::window::{Hwnd, me, set_last_error};

const ERROR_INVALID_PARAMETER: u32 = 87;

/// As many threads with an input state of their own as a program that
/// attaches has GUI threads, and a group for each: one past the table
/// falls back to group 0 with a row to say so.
const MAX_THREADS: usize = 32;
const MAX_GROUPS: usize = MAX_THREADS;

/// A thread gala has seen, and the input state it is in. `id` is 0 in a
/// free slot; thread ids are never 0.
#[derive(Clone, Copy)]
struct Entry {
    id: u32,
    group: usize,
}

/// An input state: what the threads in it share.
#[derive(Clone, Copy)]
struct Group {
    used: bool,
    focus: Hwnd,
    active: Hwnd,
}

struct Table {
    threads: [Entry; MAX_THREADS],
    groups: [Group; MAX_GROUPS],
}

struct Shared {
    lock: AtomicBool,
    table: UnsafeCell<Table>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Shared {}

static SHARED: Shared = Shared {
    lock: AtomicBool::new(false),
    table: UnsafeCell::new(Table {
        threads: [Entry { id: 0, group: 0 }; MAX_THREADS],
        groups: [Group { used: false, focus: 0, active: 0 }; MAX_GROUPS],
    }),
};

/// The window table is never read under this lock: a caller that needs
/// both looks at the windows first (`has_queue`, `crate::tree::SetFocus`).
fn with<R>(f: impl FnOnce(&mut Table) -> R) -> R {
    while SHARED.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *SHARED.table.get() });
    SHARED.lock.store(false, Ordering::Release);
    r
}

/// The input state `id` is in, made on first use: every thread starts in
/// one of its own. Past the table a thread falls back to group 0 and
/// shares whatever is there, which is what gala did before this module
/// (one focus for the process); the row says which thread found it full.
fn group_of(t: &mut Table, id: u32) -> usize {
    if let Some(e) = t.threads.iter().find(|e| e.id == id) {
        return e.group;
    }
    let free = (t.threads.iter().position(|e| e.id == 0), t.groups.iter().position(|g| !g.used));
    let (Some(ti), Some(gi)) = free else {
        crate::window::unsupported(b"user32.dll: thread input states full", u64::from(id));
        return 0;
    };
    t.groups[gi] = Group { used: true, focus: 0, active: 0 };
    t.threads[ti] = Entry { id, group: gi };
    gi
}

/// The focus of the calling thread's input state (GetFocus).
pub(crate) fn focus() -> Hwnd {
    with(|t| {
        let g = group_of(t, me());
        t.groups[g].focus
    })
}

/// The active window of the calling thread's input state
/// (GetActiveWindow).
pub(crate) fn active() -> Hwnd {
    with(|t| {
        let g = group_of(t, me());
        t.groups[g].active
    })
}

/// `hwnd` takes the focus of the calling thread's input state, `top`
/// being its top-level window (0 for no focus): the window the call
/// reports. That is the window the focus was taken from -- except when
/// `top` is not the state's active window, where the call activates it
/// first and so reports `hwnd` itself, which is what Windows answers
/// (`threadinput_test`, `setfocus-own-attached`). Activating here is the
/// state's active window and no more: the desktop's foreground window
/// and WM_ACTIVATE are `crate::window::activate`'s, and sending them
/// from here would come back through DefWindowProc into this call.
pub(crate) fn take_focus(hwnd: Hwnd, top: Hwnd) -> Hwnd {
    with(|t| {
        let g = group_of(t, me());
        let previous = t.groups[g].focus;
        t.groups[g].focus = hwnd;
        if hwnd != 0 && top != t.groups[g].active {
            t.groups[g].active = top;
            return hwnd;
        }
        previous
    })
}

/// Whether `thread` shares the calling thread's input state: a window of
/// a thread that does not cannot be given the focus
/// (ERROR_ACCESS_DENIED), which is what `AttachThreadInput` is asked for.
pub(crate) fn shares(thread: u32) -> bool {
    let mine = me();
    if thread == 0 || thread == mine {
        return true;
    }
    with(|t| group_of(t, mine) == group_of(t, thread))
}

/// Top-level window `hwnd` of thread `thread` has been activated and
/// `old` of `old_thread` is no longer (`crate::window::activate`): the
/// active window of each one's input state follows. The foreground
/// window is the desktop's single notion; the active window is per input
/// state, which is the one `AttachThreadInput` joins.
pub(crate) fn activated(thread: u32, hwnd: Hwnd, old_thread: u32, old: Hwnd) {
    with(|t| {
        if thread != 0 {
            let g = group_of(t, thread);
            t.groups[g].active = hwnd;
            if old != 0 && old_thread != 0 {
                let o = group_of(t, old_thread);
                if o != g {
                    t.groups[o].active = 0;
                }
            }
        } else if old != 0 && old_thread != 0 {
            // Activated away from our windows: nothing of ours is active.
            let o = group_of(t, old_thread);
            t.groups[o].active = 0;
        }
    });
}

/// A window is going: no input state keeps it (`crate::tree::forget`).
pub(crate) fn forget(hwnd: Hwnd) {
    with(|t| {
        for g in t.groups.iter_mut() {
            if g.focus == hwnd {
                g.focus = 0;
            }
            if g.active == hwnd {
                g.active = 0;
            }
        }
    });
}

/// Whether gala can see a message queue for `id`: a thread with an input
/// state of its own, or one that owns a window. A thread that has never
/// called user32 and owns no window has none, which is what Windows
/// refuses (`threadinput_test`, `to-queueless`). Reads the window table,
/// so never called under this module's lock.
fn has_queue(id: u32) -> bool {
    if with(|t| t.threads.iter().any(|e| e.id == id)) {
        return true;
    }
    crate::window::with(|s| s.windows.iter().any(|w| w.used && w.thread == id))
}

/// Thread `id` joins the input state `to`, leaving `from`: the joined
/// state is the one the thread being attached to had, as measured, so
/// nothing of `from` is carried over and an empty `from` is given back.
fn join(t: &mut Table, id: u32, from: usize, to: usize) {
    for e in t.threads.iter_mut() {
        if e.id == id {
            e.group = to;
        }
    }
    if !t.threads.iter().any(|e| e.id != 0 && e.group == from) {
        t.groups[from] = Group { used: false, focus: 0, active: 0 };
    }
}

/// Thread `id` leaves the state it shared and keeps what that state
/// held: detaching does not undo the merged focus and active window, as
/// measured. Past the table the threads stay in one state, with a row.
fn part(t: &mut Table, id: u32, group: usize) {
    let kept = t.groups[group];
    let Some(gi) = t.groups.iter().position(|g| !g.used) else {
        crate::window::unsupported(
            b"user32.dll!AttachThreadInput(detach: input states full)",
            u64::from(id),
        );
        return;
    };
    t.groups[gi] = Group { used: true, focus: kept.focus, active: kept.active };
    for e in t.threads.iter_mut() {
        if e.id == id {
            e.group = gi;
        }
    }
}

/// AttachThreadInput(idAttach, idAttachTo, fAttach): the two threads
/// share one input state, or stop sharing it; nonzero on success. The
/// module's own notes say what the calls answer and why.
#[unsafe(no_mangle)]
pub extern "system" fn AttachThreadInput(attach: u32, attach_to: u32, fattach: i32) -> i32 {
    if attach == 0 || attach_to == 0 || attach == attach_to {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    // Both before the lock: `has_queue` reads the window table.
    let (a, b) = (has_queue(attach), has_queue(attach_to));
    if !a || !b {
        // Windows refuses a thread with no message queue here, and so
        // does gala -- but gala also cannot see a thread of another
        // process, whose queue Windows would accept. The row says what
        // gala saw, so a program refused for the second reason is not
        // read as the first.
        crate::window::unsupported(
            b"user32.dll!AttachThreadInput(no window or input state of ours)",
            u64::from(if a { attach_to } else { attach }),
        );
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    with(|t| {
        let (ga, gb) = (group_of(t, attach), group_of(t, attach_to));
        if fattach != 0 {
            if ga != gb {
                join(t, attach, ga, gb);
            }
        } else if ga == gb {
            part(t, attach, ga);
        }
    });
    1
}
