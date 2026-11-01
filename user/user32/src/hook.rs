//! Windows hooks (SetWindowsHookExW) for the calling thread's own windows:
//! a chain per kind and thread, the newest hook called first;
//! CallNextHookEx passes on to the next older one (its hook argument is
//! ignored, as documented: the chain position is the hook running now) and
//! returns 0 after the last. Called where Windows calls them: WH_CBT on
//! window creation (HCBT_CREATEWND, a nonzero result refuses it) and
//! destruction (HCBT_DESTROYWND), WH_GETMESSAGE when GetMessage or
//! PeekMessage return a message (the hook may change it), WH_CALLWNDPROC
//! before SendMessage calls a window procedure. Other kinds are kept but
//! not called yet (keyboard and mouse hooks need input routing through
//! them). Global hooks (another thread, a DLL's module) are not supported.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub type HookProc = extern "system" fn(i32, usize, isize) -> isize;

pub const WH_GETMESSAGE: i32 = 3;
pub const WH_CALLWNDPROC: i32 = 4;
pub const WH_CBT: i32 = 5;
pub const HCBT_CREATEWND: i32 = 3;
pub const HCBT_DESTROYWND: i32 = 4;

const MAX_HOOKS: usize = 64;
const HHOOK_BASE: usize = 0x0002_0000;

#[derive(Clone, Copy)]
struct Hook {
    used: bool,
    kind: i32,
    thread: u32,
    proc_: Option<HookProc>,
    /// Installation order (newer: larger).
    seq: u32,
}

const NO_HOOK: Hook = Hook { used: false, kind: 0, thread: 0, proc_: None, seq: 0 };

struct Table {
    lock: AtomicBool,
    hooks: core::cell::UnsafeCell<[Hook; MAX_HOOKS]>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table = Table {
    lock: AtomicBool::new(false),
    hooks: core::cell::UnsafeCell::new([NO_HOOK; MAX_HOOKS]),
};
static SEQ: AtomicU32 = AtomicU32::new(1);

fn with<R>(f: impl FnOnce(&mut [Hook; MAX_HOOKS]) -> R) -> R {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *TABLE.hooks.get() });
    TABLE.lock.store(false, Ordering::Release);
    r
}

/// The hook each thread is running now (thread, slot), for CallNextHookEx.
static RUNNING: [(AtomicU32, AtomicU32); 16] =
    [const { (AtomicU32::new(0), AtomicU32::new(0)) }; 16];

fn running_slot(thread: u32) -> Option<&'static (AtomicU32, AtomicU32)> {
    RUNNING.iter().find(|r| r.0.load(Ordering::Relaxed) == thread)
}

/// The next hook to call for (kind, thread) older than `before` (u32::MAX:
/// the newest): its slot and procedure.
fn next_hook(kind: i32, thread: u32, before: u32) -> Option<(usize, HookProc, u32)> {
    with(|h| {
        h.iter()
            .enumerate()
            .filter(|(_, k)| k.used && k.kind == kind && k.thread == thread && k.seq < before)
            .max_by_key(|(_, k)| k.seq)
            .and_then(|(i, k)| k.proc_.map(|p| (i, p, k.seq)))
    })
}

/// Calls hook `slot`'s procedure, noting it as this thread's running hook.
fn run(thread: u32, seq: u32, p: HookProc, code: i32, wparam: usize, lparam: isize) -> isize {
    let entry = running_slot(thread).or_else(|| {
        RUNNING
            .iter()
            .find(|r| r.0.compare_exchange(0, thread, Ordering::AcqRel, Ordering::Relaxed).is_ok())
    });
    let previous = entry.map(|e| e.1.swap(seq, Ordering::Relaxed));
    let r = p(code, wparam, lparam);
    if let (Some(e), Some(prev)) = (entry, previous) {
        e.1.store(prev, Ordering::Relaxed);
    }
    r
}

/// Calls the chain of `kind` hooks of the calling thread; 0 without any.
pub fn call(kind: i32, code: i32, wparam: usize, lparam: isize) -> isize {
    let thread = crate::window::me();
    match next_hook(kind, thread, u32::MAX) {
        Some((_, p, seq)) => run(thread, seq, p, code, wparam, lparam),
        None => 0,
    }
}

/// Whether the calling thread has hooks of `kind` (cheap skip).
pub fn any(kind: i32) -> bool {
    let thread = crate::window::me();
    with(|h| h.iter().any(|k| k.used && k.kind == kind && k.thread == thread))
}

/// SetWindowsHookExW(kind, proc, module, thread): a hook on the calling
/// thread (thread 0 or another thread: not supported, NULL).
#[unsafe(no_mangle)]
pub extern "system" fn SetWindowsHookExW(
    kind: i32,
    proc_: Option<HookProc>,
    _module: usize,
    thread: u32,
) -> usize {
    let me = crate::window::me();
    if proc_.is_none() || (thread != 0 && thread != me) || thread == 0 {
        if thread == 0 || thread != me {
            crate::window::unsupported(b"user32.dll!SetWindowsHookExW(global)", kind as u64);
        }
        return 0;
    }
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    with(|h| {
        let i = h.iter().position(|k| !k.used)?;
        h[i] = Hook { used: true, kind, thread: me, proc_, seq };
        Some(HHOOK_BASE + i * 0x10)
    })
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn SetWindowsHookExA(
    kind: i32,
    proc_: Option<HookProc>,
    module: usize,
    thread: u32,
) -> usize {
    SetWindowsHookExW(kind, proc_, module, thread)
}

/// UnhookWindowsHookEx: TRUE once; FALSE for a handle that is not a hook.
#[unsafe(no_mangle)]
pub extern "system" fn UnhookWindowsHookEx(hook: usize) -> i32 {
    let Some(i) = hook.checked_sub(HHOOK_BASE).filter(|d| d % 0x10 == 0).map(|d| d / 0x10) else {
        return 0;
    };
    with(|h| match h.get_mut(i) {
        Some(k) if k.used => {
            *k = NO_HOOK;
            1
        }
        _ => 0,
    })
}

/// CallNextHookEx(ignored, code, wParam, lParam): the next older hook of
/// the one running on this thread, or 0.
#[unsafe(no_mangle)]
pub extern "system" fn CallNextHookEx(
    _hook: usize,
    code: i32,
    wparam: usize,
    lparam: isize,
) -> isize {
    let thread = crate::window::me();
    let Some(seq) = running_slot(thread).map(|r| r.1.load(Ordering::Relaxed)).filter(|&s| s != 0)
    else {
        return 0;
    };
    let kind = with(|h| h.iter().find(|k| k.used && k.seq == seq).map(|k| k.kind));
    // The running hook may have unhooked itself: find its kind by seq if
    // still there; otherwise nothing follows.
    let Some(kind) = kind else { return 0 };
    match next_hook(kind, thread, seq) {
        Some((_, p, next)) => run(thread, next, p, code, wparam, lparam),
        None => 0,
    }
}
