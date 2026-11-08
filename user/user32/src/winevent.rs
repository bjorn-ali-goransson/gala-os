//! WinEvents (SetWinEventHook, UnhookWinEvent, NotifyWinEvent,
//! IsWinEventHookInstalled), as Microsoft documents them, across
//! processes through the kernel (nt::winevent): a hook is the kernel's,
//! NotifyWinEvent queues the event for every hook on the desktop that
//! wants it, and the hooking thread's callback runs when that thread next
//! reads its messages (GetMessage, PeekMessage...: `deliver`, from the
//! pump), as an out-of-context hook's does on Windows. WINEVENT_INCONTEXT
//! hooks (with their DLL's module, as documented) are delivered the same
//! way, on the hooking thread: gala maps no DLL into other processes.
//!
//! user32 raises the events Windows' user32 and controls raise for the
//! window tree (EVENT_OBJECT_CREATE, DESTROY, SHOW, HIDE, FOCUS,
//! NAMECHANGE, VALUECHANGE, STATECHANGE, SELECTION, INVOKED,
//! EVENT_SYSTEM_FOREGROUND): `raise`.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::window::Hwnd;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaWinEventHook(min: u32, max: u32, pid: u32, tid: u32, flags: u32, id: *mut u32) -> u32;
    fn GalaWinEventUnhook(id: u32) -> u32;
    fn GalaWinEventNotify(event: u32, hwnd: u64, object: i32, child: i32) -> u32;
    fn GalaWinEventTake(records: *mut u8, max: u32, count: *mut u32) -> u32;
}

pub type WinEventProc = extern "system" fn(usize, u32, Hwnd, i32, i32, u32, u32);

pub const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
pub const EVENT_OBJECT_CREATE: u32 = 0x8000;
pub const EVENT_OBJECT_DESTROY: u32 = 0x8001;
pub const EVENT_OBJECT_SHOW: u32 = 0x8002;
pub const EVENT_OBJECT_HIDE: u32 = 0x8003;
pub const EVENT_OBJECT_FOCUS: u32 = 0x8005;
pub const EVENT_OBJECT_SELECTION: u32 = 0x8006;
pub const EVENT_OBJECT_STATECHANGE: u32 = 0x800a;
pub const EVENT_OBJECT_NAMECHANGE: u32 = 0x800c;
pub const EVENT_OBJECT_VALUECHANGE: u32 = 0x800e;
pub const EVENT_OBJECT_INVOKED: u32 = 0x8013;
pub const OBJID_WINDOW: i32 = 0;
pub const OBJID_CLIENT: i32 = -4;
pub const CHILDID_SELF: i32 = 0;

const WINEVENT_INCONTEXT: u32 = 4;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_INVALID_FILTER_PROC: u32 = 1427;
const ERROR_HOOK_NEEDS_HMOD: u32 = 1428;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_INVALID_PARAMETER: u32 = 87;
const STATUS_INVALID_PARAMETER: u32 = 0xc000_000d;

const MAX_HOOKS: usize = 64;
const MAX_PENDING: usize = 256;
/// GalaWinEventTake's record (nt::winevent::Record::to_bytes).
const RECORD: usize = 40;

/// Our hooks (id 0: free) and the events taken from the kernel for one
/// of our threads that it has not been given yet.
#[derive(Clone, Copy)]
struct Hook {
    id: u32,
    proc_: Option<WinEventProc>,
}

#[derive(Clone, Copy)]
struct Pending {
    tid: u32,
    hook: u32,
    event: u32,
    hwnd: Hwnd,
    object: i32,
    child: i32,
    thread: u32,
    time: u32,
}

struct Table {
    lock: AtomicBool,
    hooks: UnsafeCell<[Hook; MAX_HOOKS]>,
    pending: UnsafeCell<[Option<Pending>; MAX_PENDING]>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table = Table {
    lock: AtomicBool::new(false),
    hooks: UnsafeCell::new([Hook { id: 0, proc_: None }; MAX_HOOKS]),
    pending: UnsafeCell::new([None; MAX_PENDING]),
};
/// How many hooks this process has (the pump asks the kernel only then).
static HOOKS: AtomicU32 = AtomicU32::new(0);

fn with<R>(f: impl FnOnce(&mut [Hook; MAX_HOOKS], &mut [Option<Pending>; MAX_PENDING]) -> R) -> R {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *TABLE.hooks.get() }, unsafe { &mut *TABLE.pending.get() });
    TABLE.lock.store(false, Ordering::Release);
    r
}

/// SetWinEventHook: events `min..=max` raised by process `pid` and thread
/// `tid` (0: any) go to `proc_` on the calling thread, which must read
/// its messages. No callback: ERROR_INVALID_FILTER_PROC; in context
/// without a module: ERROR_HOOK_NEEDS_HMOD (as Wine's tests check).
#[unsafe(no_mangle)]
pub extern "system" fn SetWinEventHook(
    min: u32,
    max: u32,
    module: usize,
    proc_: Option<WinEventProc>,
    pid: u32,
    tid: u32,
    flags: u32,
) -> usize {
    let Some(proc_) = proc_ else {
        crate::window::set_last_error(ERROR_INVALID_FILTER_PROC);
        return 0;
    };
    if flags & WINEVENT_INCONTEXT != 0 && module == 0 {
        crate::window::set_last_error(ERROR_HOOK_NEEDS_HMOD);
        return 0;
    }
    // The thread hears of events through its process's queue event.
    crate::window::listen();
    let mut id = 0u32;
    // SAFETY: our output.
    let status = unsafe { GalaWinEventHook(min, max, pid, tid, flags & 3, &mut id) };
    if status != 0 || id == 0 {
        crate::window::set_last_error(if status == STATUS_INVALID_PARAMETER {
            ERROR_INVALID_PARAMETER
        } else {
            ERROR_NOT_ENOUGH_MEMORY
        });
        return 0;
    }
    let stored = with(|hooks, _| match hooks.iter_mut().find(|h| h.id == 0) {
        Some(h) => {
            *h = Hook { id, proc_: Some(proc_) };
            true
        }
        None => false,
    });
    if !stored {
        // SAFETY: our hook.
        unsafe { GalaWinEventUnhook(id) };
        crate::window::set_last_error(ERROR_NOT_ENOUGH_MEMORY);
        return 0;
    }
    HOOKS.fetch_add(1, Ordering::AcqRel);
    id as usize
}

/// UnhookWinEvent: FALSE (ERROR_INVALID_HANDLE) for no hook of ours.
#[unsafe(no_mangle)]
pub extern "system" fn UnhookWinEvent(hook: usize) -> i32 {
    let id = hook as u32;
    let found = hook != 0
        && (hook as u64) >> 32 == 0
        && with(|hooks, pending| {
            let Some(h) = hooks.iter_mut().find(|h| h.id == id) else { return false };
            h.id = 0;
            h.proc_ = None;
            for p in pending.iter_mut() {
                if p.is_some_and(|p| p.hook == id) {
                    *p = None;
                }
            }
            true
        });
    if !found {
        crate::window::set_last_error(ERROR_INVALID_HANDLE);
        return 0;
    }
    HOOKS.fetch_sub(1, Ordering::AcqRel);
    // SAFETY: a plain call.
    unsafe { GalaWinEventUnhook(id) };
    1
}

/// NotifyWinEvent: every hook that wants the event hears of it.
#[unsafe(no_mangle)]
pub extern "system" fn NotifyWinEvent(event: u32, hwnd: Hwnd, object: i32, child: i32) {
    // SAFETY: a plain call.
    unsafe { GalaWinEventNotify(event, hwnd as u64, object, child) };
}

/// IsWinEventHookInstalled: documented to answer TRUE when it cannot
/// tell; gala's hooks are the kernel's, so TRUE.
#[unsafe(no_mangle)]
pub extern "system" fn IsWinEventHookInstalled(_event: u32) -> i32 {
    1
}

/// An event user32 or a control raises.
pub(crate) fn raise(event: u32, hwnd: Hwnd, object: i32, child: i32) {
    NotifyWinEvent(event, hwnd, object, child);
}

/// From the pump: takes the events for this process's hooks from the
/// kernel and calls the callbacks of the calling thread's hooks (the
/// others' wait for their threads, which are woken).
pub(crate) fn deliver() {
    if HOOKS.load(Ordering::Acquire) == 0 {
        return;
    }
    let me = crate::window::me();
    let mut raw = [0u8; RECORD * 16];
    let mut others = false;
    loop {
        let mut n = 0u32;
        // SAFETY: our buffer of 16 records.
        if unsafe { GalaWinEventTake(raw.as_mut_ptr(), 16, &mut n) } != 0 || n == 0 {
            break;
        }
        for r in raw.chunks_exact(RECORD).take(n as usize) {
            let u32_at = |at: usize| u32::from_le_bytes(r[at..at + 4].try_into().unwrap_or([0; 4]));
            let p = Pending {
                hook: u32_at(0),
                event: u32_at(4),
                hwnd: u64::from_le_bytes(r[8..16].try_into().unwrap_or([0; 8])) as Hwnd,
                object: u32_at(16) as i32,
                child: u32_at(20) as i32,
                thread: u32_at(24),
                time: u32_at(28),
                tid: u32_at(32),
            };
            others |= p.tid != me;
            with(|_, pending| {
                // Full: the oldest goes.
                let slot = match pending.iter().position(Option::is_none) {
                    Some(i) => i,
                    None => {
                        pending.copy_within(1.., 0);
                        MAX_PENDING - 1
                    }
                };
                pending[slot] = Some(p);
            });
        }
        if n < 16 {
            break;
        }
    }
    if others {
        crate::window::wake();
    }
    // Ours, oldest first, each callback outside the lock (it may read
    // messages, and so come back here).
    for _ in 0..MAX_PENDING {
        let next = with(|hooks, pending| {
            let i = pending.iter().position(|p| p.is_some_and(|p| p.tid == me))?;
            let p = pending[i].take()?;
            // Keep the order: the later ones move up.
            pending.copy_within(i + 1.., i);
            pending[MAX_PENDING - 1] = None;
            let proc_ = hooks.iter().find(|h| h.id == p.hook && h.id != 0)?.proc_?;
            Some((p, proc_))
        });
        let Some((p, proc_)) = next else {
            if with(|_, pending| pending.iter().any(|p| p.is_some_and(|p| p.tid == me))) {
                continue;
            }
            break;
        };
        proc_(p.hook as usize, p.event, p.hwnd, p.object, p.child, p.thread, p.time);
    }
}
