//! The thread pool (CreateThreadpoolWork/Timer/Wait and friends), from the
//! documented contracts. Callbacks run on worker threads that take packets
//! from one completion port; workers are started as needed, up to
//! `MAX_WORKERS`. Each timer and wait object has a helper thread that
//! sleeps until its due time or handle and then queues the callback.
//!
//! Each object counts its queued (`pending`) and running callbacks under
//! its lock; WaitFor*Callbacks with cancellation marks the queued ones to
//! be skipped, then waits for both counts to reach zero.
//!
//! Not yet: callback environments (pools, cleanup groups, priorities;
//! the default pool is used), and objects are not freed when closed (a
//! closed object's helper thread ends; the memory stays).

use super::sync::{AcquireSRWLockExclusive, ReleaseSRWLockExclusive};
use super::*;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

const MAX_WORKERS: u32 = 8;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;

type WorkCallback = extern "system" fn(*mut c_void, *mut c_void, *mut c_void);
type WaitCallback = extern "system" fn(*mut c_void, *mut c_void, *mut c_void, u32);
type SimpleCallback = extern "system" fn(*mut c_void, *mut c_void);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Work,
    Timer,
    Wait,
    /// TrySubmitThreadpoolCallback: run once, then freed.
    Simple,
}

/// TP_WORK, TP_TIMER and TP_WAIT (opaque to programs).
struct Object {
    kind: Kind,
    callback: usize,
    context: *mut c_void,
    lock: usize,
    pending: u32,
    running: u32,
    skip: u32,
    closed: bool,
    /// Timers and waits: the helper's control event and thread.
    control: Handle,
    /// Timers: due (tick count, ms) and period; armed if Some.
    due: Option<u64>,
    period: u32,
    /// Waits: the handle and the timeout's tick count (None: INFINITE).
    handle: Handle,
    deadline: Option<u64>,
    armed: bool,
}

/// TP_CALLBACK_INSTANCE: what to do when the callback returns.
struct Instance {
    free_library: Handle,
    set_event: Handle,
    release_semaphore: (Handle, i32),
    release_mutex: Handle,
    leave_section: *mut sync::CriticalSection,
}

static PORT: AtomicUsize = AtomicUsize::new(0);
static WORKERS: AtomicU32 = AtomicU32::new(0);
static IDLE: AtomicU32 = AtomicU32::new(0);
/// Serializes creating the completion port.
static PORT_LOCK: AtomicUsize = AtomicUsize::new(0);

fn with_lock<R>(o: *mut Object, f: impl FnOnce(&mut Object) -> R) -> R {
    // SAFETY: a live object of ours; its lock serializes the fields.
    unsafe {
        AcquireSRWLockExclusive(&raw mut (*o).lock);
        let r = f(&mut *o);
        ReleaseSRWLockExclusive(&raw mut (*o).lock);
        r
    }
}

fn port() -> Handle {
    let p = PORT.load(Ordering::Acquire);
    if p != 0 {
        return p as Handle;
    }
    AcquireSRWLockExclusive(PORT_LOCK.as_ptr());
    let mut p = PORT.load(Ordering::Acquire);
    if p == 0 {
        p = CreateIoCompletionPort(INVALID_HANDLE_VALUE, core::ptr::null_mut(), 0, 0) as usize;
        PORT.store(p, Ordering::Release);
    }
    ReleaseSRWLockExclusive(PORT_LOCK.as_ptr());
    p as Handle
}

extern "system" fn worker(_: *mut c_void) -> u32 {
    let port = port();
    loop {
        let (mut bytes, mut key, mut overlapped) = (0u32, 0usize, 0usize);
        IDLE.fetch_add(1, Ordering::AcqRel);
        let ok = GetQueuedCompletionStatus(port, &mut bytes, &mut key, &mut overlapped, INFINITE);
        IDLE.fetch_sub(1, Ordering::AcqRel);
        if ok != FALSE && key != 0 {
            run(key as *mut Object, bytes);
        }
    }
}

/// Queues one callback of `o` (with `result` for waits).
fn queue(o: *mut Object, result: u32) {
    with_lock(o, |o| o.pending += 1);
    let port = port();
    // Another worker when none is idle, up to MAX_WORKERS.
    let n = WORKERS.load(Ordering::Acquire);
    if IDLE.load(Ordering::Acquire) == 0
        && n < MAX_WORKERS
        && WORKERS.compare_exchange(n, n + 1, Ordering::AcqRel, Ordering::Relaxed).is_ok()
    {
        let t = CreateThread(
            core::ptr::null_mut(),
            0,
            worker,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
        );
        if t.is_null() {
            WORKERS.fetch_sub(1, Ordering::AcqRel);
        } else {
            CloseHandle(t);
        }
    }
    PostQueuedCompletionStatus(port, result, o as usize, 0);
}

/// Runs one queued callback of `o` (unless cancelled).
fn run(o: *mut Object, result: u32) {
    let go = with_lock(o, |o| {
        o.pending -= 1;
        if o.skip > 0 {
            o.skip -= 1;
            return false;
        }
        o.running += 1;
        true
    });
    if !go {
        return;
    }
    let mut instance = Instance {
        free_library: core::ptr::null_mut(),
        set_event: core::ptr::null_mut(),
        release_semaphore: (core::ptr::null_mut(), 0),
        release_mutex: core::ptr::null_mut(),
        leave_section: core::ptr::null_mut(),
    };
    let inst = (&mut instance as *mut Instance).cast::<c_void>();
    // SAFETY: the callback the program registered for this kind.
    let (kind, callback, context) = unsafe { ((*o).kind, (*o).callback, (*o).context) };
    // SAFETY: as above.
    unsafe {
        match kind {
            Kind::Work | Kind::Timer => {
                core::mem::transmute::<usize, WorkCallback>(callback)(inst, context, o.cast())
            }
            Kind::Wait => core::mem::transmute::<usize, WaitCallback>(callback)(
                inst,
                context,
                o.cast(),
                result,
            ),
            Kind::Simple => core::mem::transmute::<usize, SimpleCallback>(callback)(inst, context),
        }
    }
    if !instance.leave_section.is_null() {
        sync::LeaveCriticalSection(instance.leave_section);
    }
    if !instance.release_mutex.is_null() {
        sync::ReleaseMutex(instance.release_mutex);
    }
    if !instance.release_semaphore.0.is_null() {
        sync::ReleaseSemaphore(
            instance.release_semaphore.0,
            instance.release_semaphore.1,
            core::ptr::null_mut(),
        );
    }
    if !instance.set_event.is_null() {
        SetEvent(instance.set_event);
    }
    if !instance.free_library.is_null() {
        FreeLibrary(instance.free_library);
    }
    with_lock(o, |o| o.running -= 1);
    if kind == Kind::Simple {
        super::heap::HeapFree(super::heap::GetProcessHeap(), 0, o.cast());
    }
}

/// Callback environments (custom pools, cleanup groups, priorities) are
/// not supported: the default pool runs the callbacks; recorded.
fn env_ignored(what: &[u8], env: *mut c_void) {
    if !env.is_null() {
        unsupported(what, 0);
    }
}

/// A new object, or null (ERROR_NOT_ENOUGH_MEMORY).
fn create(kind: Kind, callback: usize, context: *mut c_void) -> *mut Object {
    let o = super::heap::HeapAlloc(super::heap::GetProcessHeap(), 0, size_of::<Object>())
        .cast::<Object>();
    if o.is_null() {
        return fail_with(8, core::ptr::null_mut());
    }
    // SAFETY: fresh memory of the object's size.
    unsafe {
        o.write(Object {
            kind,
            callback,
            context,
            lock: 0,
            pending: 0,
            running: 0,
            skip: 0,
            closed: false,
            control: core::ptr::null_mut(),
            due: None,
            period: 0,
            handle: core::ptr::null_mut(),
            deadline: None,
            armed: false,
        })
    };
    o
}

/// Waits until no callback of `o` is queued or running, first cancelling
/// the queued ones if asked.
fn wait_callbacks(o: *mut Object, cancel: Bool) {
    if o.is_null() {
        return;
    }
    if cancel != FALSE {
        with_lock(o, |o| o.skip = o.pending);
    }
    let mut spins = 0u32;
    while with_lock(o, |o| o.pending + o.running) != 0 {
        spins += 1;
        if spins < 64 {
            SwitchToThread();
        } else {
            Sleep(1);
        }
    }
}

// ---- Work ----

#[unsafe(no_mangle)]
pub extern "system" fn CreateThreadpoolWork(
    callback: WorkCallback,
    context: *mut c_void,
    env: *mut c_void,
) -> *mut c_void {
    env_ignored(b"kernel32.dll!CreateThreadpoolWork(callback environment)", env);
    create(Kind::Work, callback as usize, context).cast()
}

#[unsafe(no_mangle)]
pub extern "system" fn SubmitThreadpoolWork(work: *mut c_void) {
    if !work.is_null() {
        queue(work.cast(), 0);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn WaitForThreadpoolWorkCallbacks(work: *mut c_void, cancel: Bool) {
    wait_callbacks(work.cast(), cancel);
}

/// CloseThreadpoolWork: queued callbacks still run (the object is not
/// freed; see the module comment).
#[unsafe(no_mangle)]
pub extern "system" fn CloseThreadpoolWork(work: *mut c_void) {
    if !work.is_null() {
        with_lock(work.cast(), |o| o.closed = true);
    }
}

/// TrySubmitThreadpoolCallback: runs `callback(instance, context)` once.
#[unsafe(no_mangle)]
pub extern "system" fn TrySubmitThreadpoolCallback(
    callback: SimpleCallback,
    context: *mut c_void,
    env: *mut c_void,
) -> Bool {
    env_ignored(b"kernel32.dll!TrySubmitThreadpoolCallback(callback environment)", env);
    let o = create(Kind::Simple, callback as usize, context);
    if o.is_null() {
        return FALSE;
    }
    queue(o, 0);
    TRUE
}

// ---- Timers and waits: helper threads ----

/// The helper's control event, started on first use.
fn helper(o: *mut Object, f: StartRoutine) -> bool {
    let need = with_lock(o, |o| o.control.is_null());
    if !need {
        return true;
    }
    let control = super::sync::CreateEventW(core::ptr::null_mut(), FALSE, FALSE, core::ptr::null());
    if control.is_null() {
        return false;
    }
    with_lock(o, |o| o.control = control);
    let t = CreateThread(core::ptr::null_mut(), 0, f, o.cast(), 0, core::ptr::null_mut());
    if t.is_null() {
        return false;
    }
    CloseHandle(t);
    true
}

fn wake(o: *mut Object) {
    let control = with_lock(o, |o| o.control);
    if !control.is_null() {
        SetEvent(control);
    }
}

/// Milliseconds from now until a FILETIME due time (negative: relative,
/// in 100 ns units; positive: absolute, UTC).
fn due_ms(due: i64) -> u64 {
    if due <= 0 {
        return due.unsigned_abs().div_ceil(10_000);
    }
    let mut now = 0u64;
    super::mapping::GetSystemTimeAsFileTime(&mut now);
    (due as u64).saturating_sub(now).div_ceil(10_000)
}

/// The timer helper: sleeps until the due time, queues the callback,
/// re-arms periodic timers.
extern "system" fn timer_helper(param: *mut c_void) -> u32 {
    let o = param.cast::<Object>();
    loop {
        let (closed, due, control) = with_lock(o, |o| (o.closed, o.due, o.control));
        if closed {
            return 0;
        }
        let now = GetTickCount64();
        match due {
            Some(at) if at <= now => {
                with_lock(o, |o| {
                    o.due = if o.period != 0 { Some(at + u64::from(o.period)) } else { None };
                });
                queue(o, 0);
            }
            Some(at) => {
                WaitForSingleObject(control, (at - now).min(u64::from(INFINITE - 1)) as u32);
            }
            None => {
                WaitForSingleObject(control, INFINITE);
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateThreadpoolTimer(
    callback: WorkCallback,
    context: *mut c_void,
    env: *mut c_void,
) -> *mut c_void {
    env_ignored(b"kernel32.dll!CreateThreadpoolTimer(callback environment)", env);
    create(Kind::Timer, callback as usize, context).cast()
}

/// SetThreadpoolTimer: a NULL due time cancels the timer; 0 (or a time
/// passed) fires at once; msPeriod 0 is one-shot. The window is ignored.
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadpoolTimer(
    timer: *mut c_void,
    due: *const i64,
    period: u32,
    _window: u32,
) {
    let o = timer.cast::<Object>();
    if o.is_null() || !helper(o, timer_helper) {
        return;
    }
    let at = if due.is_null() {
        None
    } else {
        // SAFETY: the caller's FILETIME.
        Some(GetTickCount64() + due_ms(unsafe { due.read_unaligned() }))
    };
    with_lock(o, |o| {
        o.due = at;
        o.period = period;
    });
    wake(o);
}

#[unsafe(no_mangle)]
pub extern "system" fn IsThreadpoolTimerSet(timer: *mut c_void) -> Bool {
    if timer.is_null() {
        return FALSE;
    }
    Bool::from(with_lock(timer.cast(), |o| o.due.is_some()))
}

#[unsafe(no_mangle)]
pub extern "system" fn WaitForThreadpoolTimerCallbacks(timer: *mut c_void, cancel: Bool) {
    wait_callbacks(timer.cast(), cancel);
}

#[unsafe(no_mangle)]
pub extern "system" fn CloseThreadpoolTimer(timer: *mut c_void) {
    if timer.is_null() {
        return;
    }
    with_lock(timer.cast(), |o| {
        o.closed = true;
        o.due = None;
    });
    wake(timer.cast());
}

/// The wait helper: waits for the handle (or its timeout), queues the
/// callback with WAIT_OBJECT_0 or WAIT_TIMEOUT, then waits to be set again.
extern "system" fn wait_helper(param: *mut c_void) -> u32 {
    let o = param.cast::<Object>();
    loop {
        let (closed, armed, handle, deadline, control) =
            with_lock(o, |o| (o.closed, o.armed, o.handle, o.deadline, o.control));
        if closed {
            return 0;
        }
        if !armed {
            WaitForSingleObject(control, INFINITE);
            continue;
        }
        let ms = match deadline {
            Some(at) => at.saturating_sub(GetTickCount64()).min(u64::from(INFINITE - 1)) as u32,
            None => INFINITE,
        };
        let handles = [control, handle];
        let r = WaitForMultipleObjects(2, handles.as_ptr(), FALSE, ms);
        let fired = match r {
            1 => Some(WAIT_OBJECT_0),
            WAIT_TIMEOUT => Some(WAIT_TIMEOUT),
            _ => None, // the control event: settings changed
        };
        if let Some(result) = fired {
            // Still the same setting (not changed meanwhile)?
            let current = with_lock(o, |o| {
                let same = o.armed && o.handle == handle && !o.closed;
                if same {
                    o.armed = false;
                }
                same
            });
            if current {
                queue(o, result);
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateThreadpoolWait(
    callback: WaitCallback,
    context: *mut c_void,
    env: *mut c_void,
) -> *mut c_void {
    env_ignored(b"kernel32.dll!CreateThreadpoolWait(callback environment)", env);
    create(Kind::Wait, callback as usize, context).cast()
}

/// SetThreadpoolWait: waits once for `handle` (NULL stops waiting), with
/// an optional timeout (a FILETIME due time; NULL: INFINITE).
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadpoolWait(wait: *mut c_void, handle: Handle, timeout: *const i64) {
    let o = wait.cast::<Object>();
    if o.is_null() || !helper(o, wait_helper) {
        return;
    }
    let deadline = if timeout.is_null() {
        None
    } else {
        // SAFETY: the caller's FILETIME.
        Some(GetTickCount64() + due_ms(unsafe { timeout.read_unaligned() }))
    };
    with_lock(o, |o| {
        o.handle = handle;
        o.deadline = deadline;
        o.armed = !handle.is_null();
    });
    wake(o);
}

#[unsafe(no_mangle)]
pub extern "system" fn WaitForThreadpoolWaitCallbacks(wait: *mut c_void, cancel: Bool) {
    wait_callbacks(wait.cast(), cancel);
}

#[unsafe(no_mangle)]
pub extern "system" fn CloseThreadpoolWait(wait: *mut c_void) {
    if wait.is_null() {
        return;
    }
    with_lock(wait.cast(), |o| {
        o.closed = true;
        o.armed = false;
    });
    wake(wait.cast());
}

// ---- Callback instances ----

fn instance(i: *mut c_void) -> Option<&'static mut Instance> {
    // SAFETY: the instance pointer passed to a running callback.
    (!i.is_null()).then(|| unsafe { &mut *i.cast::<Instance>() })
}

#[unsafe(no_mangle)]
pub extern "system" fn FreeLibraryWhenCallbackReturns(i: *mut c_void, module: Handle) {
    if let Some(i) = instance(i) {
        i.free_library = module;
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn SetEventWhenCallbackReturns(i: *mut c_void, event: Handle) {
    if let Some(i) = instance(i) {
        i.set_event = event;
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn ReleaseSemaphoreWhenCallbackReturns(
    i: *mut c_void,
    semaphore: Handle,
    count: u32,
) {
    if let Some(i) = instance(i) {
        i.release_semaphore = (semaphore, count as i32);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn ReleaseMutexWhenCallbackReturns(i: *mut c_void, mutex: Handle) {
    if let Some(i) = instance(i) {
        i.release_mutex = mutex;
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn LeaveCriticalSectionWhenCallbackReturns(
    i: *mut c_void,
    cs: *mut sync::CriticalSection,
) {
    if let Some(i) = instance(i) {
        i.leave_section = cs;
    }
}

/// CallbackMayRunLong: TRUE (another worker can be started).
#[unsafe(no_mangle)]
pub extern "system" fn CallbackMayRunLong(_i: *mut c_void) -> Bool {
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn DisassociateCurrentThreadFromCallback(_i: *mut c_void) {}

// ---- Other functions msvcp140 imports ----

/// CreateEventExW: CREATE_EVENT_MANUAL_RESET (1), CREATE_EVENT_INITIAL_SET
/// (2); the access mask is not narrowed.
#[unsafe(no_mangle)]
pub extern "system" fn CreateEventExW(
    attributes: *mut c_void,
    name: *const u16,
    flags: u32,
    _access: u32,
) -> Handle {
    super::sync::CreateEventW(
        attributes,
        Bool::from(flags & 1 != 0),
        Bool::from(flags & 2 != 0),
        name,
    )
}

/// CreateSymbolicLinkW: creating symbolic links needs
/// SeCreateSymbolicLinkPrivilege (or developer mode), which processes do
/// not hold here: ERROR_PRIVILEGE_NOT_HELD.
#[unsafe(no_mangle)]
pub extern "system" fn CreateSymbolicLinkW(
    _link: *const u16,
    _target: *const u16,
    _flags: u32,
) -> u8 {
    fail_with(1314, 0)
}

/// CreateHardLinkW: gala's file system has no hard links (reported).
#[unsafe(no_mangle)]
pub extern "system" fn CreateHardLinkW(
    _link: *const u16,
    _existing: *const u16,
    _security: *mut c_void,
) -> Bool {
    unsupported(b"kernel32.dll!CreateHardLinkW", 0);
    fail_with(ERROR_NOT_SUPPORTED, FALSE)
}

const ERROR_FILE_EXISTS: u32 = 80;
const COPY_FILE_FAIL_IF_EXISTS: u32 = 1;

/// CopyFileW: the contents and attributes; with `fail_if_exists`,
/// ERROR_FILE_EXISTS if the target exists.
#[unsafe(no_mangle)]
pub extern "system" fn CopyFileW(from: *const u16, to: *const u16, fail_if_exists: Bool) -> Bool {
    copy_file(from, to, fail_if_exists != FALSE)
}

/// CopyFileExW: CopyFileW (COPY_FILE_FAIL_IF_EXISTS; no progress
/// callbacks or cancellation).
#[unsafe(no_mangle)]
pub extern "system" fn CopyFileExW(
    from: *const u16,
    to: *const u16,
    _progress: *mut c_void,
    _data: *mut c_void,
    _cancel: *mut Bool,
    flags: u32,
) -> Bool {
    copy_file(from, to, flags & COPY_FILE_FAIL_IF_EXISTS != 0)
}

fn copy_file(from: *const u16, to: *const u16, fail_if_exists: bool) -> Bool {
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    let src =
        CreateFileW(from, GENERIC_READ, 1, core::ptr::null_mut(), 3, 0, core::ptr::null_mut());
    if src == INVALID_HANDLE_VALUE {
        return FALSE;
    }
    let attributes = GetFileAttributesW(from);
    let disposition = if fail_if_exists { 1 } else { 2 }; // CREATE_NEW / CREATE_ALWAYS
    let dst = CreateFileW(
        to,
        GENERIC_WRITE,
        0,
        core::ptr::null_mut(),
        disposition,
        0,
        core::ptr::null_mut(),
    );
    if dst == INVALID_HANDLE_VALUE {
        let e = GetLastError();
        CloseHandle(src);
        return fail_with(if e == ERROR_ALREADY_EXISTS { ERROR_FILE_EXISTS } else { e }, FALSE);
    }
    let mut buf = [0u8; 4096];
    let ok = loop {
        let (mut n, mut written) = (0u32, 0u32);
        if ReadFile(src, buf.as_mut_ptr(), buf.len() as u32, &mut n, core::ptr::null_mut()) == FALSE
        {
            break false;
        }
        if n == 0 {
            break true;
        }
        if WriteFile(dst, buf.as_ptr(), n, &mut written, core::ptr::null_mut()) == FALSE
            || written != n
        {
            break false;
        }
    };
    CloseHandle(src);
    CloseHandle(dst);
    if !ok {
        return FALSE;
    }
    if attributes != u32::MAX {
        super::sysmem::SetFileAttributesW(to, attributes);
    }
    TRUE
}

// ---- Timer queues ----
//
// CreateTimerQueue and friends on top of the pool's timers: each queue
// timer is a thread-pool timer whose callback calls the
// WAITORTIMERCALLBACK with its parameter and TRUE. A queue keeps its
// timers in a list (under its lock) so DeleteTimerQueueEx can delete
// them; NULL is the default queue. WT_EXECUTE* flags other than
// WT_EXECUTEONLYONCE only choose threads on Windows and are ignored.
// Deleted timers and queues are not freed (as pool objects).

type QueueCallback = extern "system" fn(*mut c_void, u8);

const QUEUE_MAGIC: u32 = 0x6761_7471; // "galq"
const WT_EXECUTEONLYONCE: u32 = 0x8;

struct Queue {
    magic: u32,
    lock: usize,
    timers: *mut QueueTimer,
}

struct QueueTimer {
    magic: u32,
    next: *mut QueueTimer,
    callback: QueueCallback,
    parameter: *mut c_void,
    timer: *mut Object,
}

static DEFAULT_QUEUE: AtomicUsize = AtomicUsize::new(0);

fn new_queue() -> *mut Queue {
    let q = super::heap::HeapAlloc(super::heap::GetProcessHeap(), 0, size_of::<Queue>())
        .cast::<Queue>();
    if !q.is_null() {
        // SAFETY: fresh memory of the queue's size.
        unsafe { q.write(Queue { magic: QUEUE_MAGIC, lock: 0, timers: core::ptr::null_mut() }) };
    }
    q
}

/// The queue a handle names (NULL: the default queue), if valid.
fn queue_of(h: Handle) -> Option<*mut Queue> {
    if h.is_null() {
        let mut q = DEFAULT_QUEUE.load(Ordering::Acquire);
        if q == 0 {
            let fresh = new_queue();
            if fresh.is_null() {
                return None;
            }
            q = match DEFAULT_QUEUE.compare_exchange(
                0,
                fresh as usize,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => fresh as usize,
                Err(other) => other,
            };
        }
        return Some(q as *mut Queue);
    }
    let q = h.cast::<Queue>();
    // SAFETY: a handle the caller got from CreateTimerQueue (tagged).
    (unsafe { (*q).magic } == QUEUE_MAGIC).then_some(q)
}

fn queue_timer_of(h: Handle) -> Option<*mut QueueTimer> {
    let t = h.cast::<QueueTimer>();
    // SAFETY: a handle the caller got from CreateTimerQueueTimer (tagged).
    (!t.is_null() && unsafe { (*t).magic } == QUEUE_MAGIC).then_some(t)
}

extern "system" fn queue_timer_callback(
    _instance: *mut c_void,
    context: *mut c_void,
    _t: *mut c_void,
) {
    let t = context.cast::<QueueTimer>();
    // SAFETY: the queue timer this pool timer was created for.
    unsafe { ((*t).callback)((*t).parameter, 1) };
}

/// Arms pool timer `o`: `due` ms from now, then every `period` ms.
fn arm(o: *mut Object, due: u32, period: u32) {
    let due = -10_000 * i64::from(due);
    SetThreadpoolTimer(o.cast(), &due, period, 0);
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateTimerQueue() -> Handle {
    let q = new_queue();
    if q.is_null() {
        return fail_with(8, core::ptr::null_mut());
    }
    q.cast()
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateTimerQueueTimer(
    out: *mut Handle,
    queue: Handle,
    callback: QueueCallback,
    parameter: *mut c_void,
    due: u32,
    period: u32,
    flags: u32,
) -> Bool {
    let Some(q) = queue_of(queue) else { return fail_with(ERROR_INVALID_HANDLE, FALSE) };
    if out.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let t = super::heap::HeapAlloc(super::heap::GetProcessHeap(), 0, size_of::<QueueTimer>())
        .cast::<QueueTimer>();
    if t.is_null() {
        return fail_with(8, FALSE);
    }
    let o = create(Kind::Timer, queue_timer_callback as WorkCallback as usize, t.cast());
    if o.is_null() {
        return FALSE;
    }
    // SAFETY: fresh memory; the queue's list under its lock.
    unsafe {
        t.write(QueueTimer {
            magic: QUEUE_MAGIC,
            next: core::ptr::null_mut(),
            callback,
            parameter,
            timer: o,
        });
        AcquireSRWLockExclusive(&raw mut (*q).lock);
        (*t).next = (*q).timers;
        (*q).timers = t;
        ReleaseSRWLockExclusive(&raw mut (*q).lock);
        *out = t.cast();
    }
    arm(o, due, if flags & WT_EXECUTEONLYONCE != 0 { 0 } else { period });
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn ChangeTimerQueueTimer(
    queue: Handle,
    timer: Handle,
    due: u32,
    period: u32,
) -> Bool {
    let (Some(_), Some(t)) = (queue_of(queue), queue_timer_of(timer)) else {
        return fail_with(ERROR_INVALID_HANDLE, FALSE);
    };
    // SAFETY: a live queue timer.
    arm(unsafe { (*t).timer }, due, period);
    TRUE
}

/// Stops `t`, takes it off `q`'s list and cancels its queued callbacks;
/// whether a callback is still running.
fn stop(q: *mut Queue, t: *mut QueueTimer) -> bool {
    // SAFETY: a live queue and timer; the list under the queue's lock.
    let o = unsafe {
        AcquireSRWLockExclusive(&raw mut (*q).lock);
        let mut link: *mut *mut QueueTimer = &raw mut (*q).timers;
        while !(*link).is_null() {
            if *link == t {
                *link = (*t).next;
                break;
            }
            link = &raw mut (**link).next;
        }
        ReleaseSRWLockExclusive(&raw mut (*q).lock);
        (*t).magic = 0;
        (*t).timer
    };
    SetThreadpoolTimer(o.cast(), core::ptr::null(), 0, 0);
    with_lock(o, |o| {
        o.skip = o.pending;
        o.running > 0
    })
}

/// Waits for `t`'s callbacks, closes it and sets `event` (if any).
fn finish(t: *mut QueueTimer, event: Handle) {
    // SAFETY: a stopped queue timer (its pool timer stays allocated).
    let o = unsafe { (*t).timer };
    wait_callbacks(o, TRUE);
    CloseThreadpoolTimer(o.cast());
    if !event.is_null() && event != INVALID_HANDLE_VALUE {
        SetEvent(event);
    }
}

/// What a helper thread finishes: the timers (a list) and the event.
struct Finish {
    timers: *mut QueueTimer,
    event: Handle,
}

extern "system" fn finisher(param: *mut c_void) -> u32 {
    let f = param.cast::<Finish>();
    // SAFETY: the Finish the deleting call allocated for us.
    unsafe {
        let mut t = (*f).timers;
        while !t.is_null() {
            let next = (*t).next;
            finish(t, core::ptr::null_mut());
            t = next;
        }
        if !(*f).event.is_null() {
            SetEvent((*f).event);
        }
        super::heap::HeapFree(super::heap::GetProcessHeap(), 0, f.cast());
    }
    0
}

/// Finishes `timers` (linked by `next`) on a new thread, then sets `event`.
fn finish_later(timers: *mut QueueTimer, event: Handle) -> bool {
    let f = super::heap::HeapAlloc(super::heap::GetProcessHeap(), 0, size_of::<Finish>())
        .cast::<Finish>();
    if f.is_null() {
        return false;
    }
    // SAFETY: fresh memory of its size.
    unsafe { f.write(Finish { timers, event }) };
    let h = CreateThread(core::ptr::null_mut(), 0, finisher, f.cast(), 0, core::ptr::null_mut());
    if h.is_null() {
        return false;
    }
    CloseHandle(h);
    true
}

/// DeleteTimerQueueTimer: INVALID_HANDLE_VALUE waits for a running
/// callback; otherwise the call returns at once, failing with
/// ERROR_IO_PENDING if a callback is still running, and `event` (if any)
/// is set once it has returned.
#[unsafe(no_mangle)]
pub extern "system" fn DeleteTimerQueueTimer(queue: Handle, timer: Handle, event: Handle) -> Bool {
    let (Some(q), Some(t)) = (queue_of(queue), queue_timer_of(timer)) else {
        return fail_with(ERROR_INVALID_HANDLE, FALSE);
    };
    let running = stop(q, t);
    if event == INVALID_HANDLE_VALUE || !running {
        finish(t, event);
        return TRUE;
    }
    // SAFETY: a stopped timer, now alone.
    unsafe { (*t).next = core::ptr::null_mut() };
    if !finish_later(t, event) {
        finish(t, event);
        return TRUE;
    }
    fail_with(997, FALSE) // ERROR_IO_PENDING
}

/// DeleteTimerQueueEx: deletes every timer of the queue (waiting for them
/// with INVALID_HANDLE_VALUE; otherwise on another thread, which then
/// sets `event`).
#[unsafe(no_mangle)]
pub extern "system" fn DeleteTimerQueueEx(queue: Handle, event: Handle) -> Bool {
    if queue.is_null() {
        return fail_with(ERROR_INVALID_HANDLE, FALSE);
    }
    let Some(q) = queue_of(queue) else { return fail_with(ERROR_INVALID_HANDLE, FALSE) };
    // Take the whole list, then stop each timer.
    // SAFETY: a live queue; its list under its lock.
    let list = unsafe {
        AcquireSRWLockExclusive(&raw mut (*q).lock);
        let l = (*q).timers;
        (*q).timers = core::ptr::null_mut();
        (*q).magic = 0;
        ReleaseSRWLockExclusive(&raw mut (*q).lock);
        l
    };
    let mut t = list;
    while !t.is_null() {
        // SAFETY: timers of the taken list.
        unsafe {
            let o = (*t).timer;
            (*t).magic = 0;
            SetThreadpoolTimer(o.cast(), core::ptr::null(), 0, 0);
            with_lock(o, |o| o.skip = o.pending);
            t = (*t).next;
        }
    }
    if event == INVALID_HANDLE_VALUE {
        let mut t = list;
        while !t.is_null() {
            // SAFETY: as above.
            let next = unsafe { (*t).next };
            finish(t, core::ptr::null_mut());
            t = next;
        }
        return TRUE;
    }
    if !finish_later(list, event) {
        return fail_with(8, FALSE);
    }
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn DeleteTimerQueue(queue: Handle) -> Bool {
    DeleteTimerQueueEx(queue, core::ptr::null_mut())
}

// ---- Registered waits (RegisterWaitForSingleObject) ----
//
// Each one is a thread pool wait on the object; its callback calls the
// caller's WAITORTIMERCALLBACK and, unless WT_EXECUTEONLYONCE, waits again.

const REGISTERED_MAGIC: u32 = 0x6761_7277; // "galw"

struct RegisteredWait {
    magic: u32,
    callback: QueueCallback,
    context: *mut c_void,
    wait: *mut c_void,
    object: Handle,
    /// The timeout (ms; u32::MAX: none), and whether it runs once.
    timeout: u32,
    once: bool,
}

/// Arms the pool wait of `r`.
fn arm_wait(r: *mut RegisteredWait) {
    // SAFETY: a live registered wait.
    unsafe {
        if (*r).timeout == u32::MAX {
            SetThreadpoolWait((*r).wait, (*r).object, core::ptr::null());
        } else {
            let due = -10_000 * i64::from((*r).timeout);
            SetThreadpoolWait((*r).wait, (*r).object, &due);
        }
    }
}

extern "system" fn registered_wait_callback(
    _instance: *mut c_void,
    context: *mut c_void,
    _wait: *mut c_void,
    result: u32,
) {
    const WAIT_TIMEOUT: u32 = 0x102;
    let r = context.cast::<RegisteredWait>();
    // SAFETY: the registered wait this pool wait was created for (it lives
    // until UnregisterWait, which waits for this callback).
    unsafe {
        ((*r).callback)((*r).context, u8::from(result == WAIT_TIMEOUT));
        if !(*r).once {
            arm_wait(r);
        }
    }
}

/// RegisterWaitForSingleObject(phNewWaitObject, hObject, Callback, Context,
/// dwMilliseconds, dwFlags): the callback runs on a pool thread when the
/// object is signaled (TimerOrWaitFired FALSE) or the timeout passes
/// (TRUE); again each time unless WT_EXECUTEONLYONCE.
#[unsafe(no_mangle)]
pub extern "system" fn RegisterWaitForSingleObject(
    out: *mut Handle,
    object: Handle,
    callback: QueueCallback,
    context: *mut c_void,
    ms: u32,
    flags: u32,
) -> Bool {
    if out.is_null() || object.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let r = super::heap::HeapAlloc(super::heap::GetProcessHeap(), 0, size_of::<RegisteredWait>())
        .cast::<RegisteredWait>();
    if r.is_null() {
        return fail_with(8, FALSE);
    }
    let wait = CreateThreadpoolWait(registered_wait_callback, r.cast(), core::ptr::null_mut());
    if wait.is_null() {
        super::heap::HeapFree(super::heap::GetProcessHeap(), 0, r.cast());
        return FALSE;
    }
    // SAFETY: fresh memory; the caller's output.
    unsafe {
        r.write(RegisteredWait {
            magic: REGISTERED_MAGIC,
            callback,
            context,
            wait,
            object,
            timeout: ms,
            once: flags & WT_EXECUTEONLYONCE != 0,
        });
        *out = r.cast();
    }
    arm_wait(r);
    TRUE
}

/// UnregisterWaitEx(WaitHandle, CompletionEvent): no more callbacks; it
/// waits for one that is running (gala does whatever CompletionEvent
/// says), then sets CompletionEvent if it is an event.
#[unsafe(no_mangle)]
pub extern "system" fn UnregisterWaitEx(wait: Handle, completion: Handle) -> Bool {
    let r = wait.cast::<RegisteredWait>();
    // SAFETY: a handle the caller got from RegisterWaitForSingleObject.
    if r.is_null() || unsafe { (*r).magic } != REGISTERED_MAGIC {
        return fail_with(ERROR_INVALID_HANDLE, FALSE);
    }
    // SAFETY: a live registered wait; freed once its callbacks are done.
    unsafe {
        let w = (*r).wait;
        (*r).once = true;
        SetThreadpoolWait(w, core::ptr::null_mut(), core::ptr::null());
        WaitForThreadpoolWaitCallbacks(w, TRUE);
        CloseThreadpoolWait(w);
        (*r).magic = 0;
        super::heap::HeapFree(super::heap::GetProcessHeap(), 0, r.cast());
        if !completion.is_null() && completion as isize != -1 {
            super::SetEvent(completion);
        }
    }
    TRUE
}

/// UnregisterWait(WaitHandle): UnregisterWaitEx without an event.
#[unsafe(no_mangle)]
pub extern "system" fn UnregisterWait(wait: Handle) -> Bool {
    UnregisterWaitEx(wait, core::ptr::null_mut())
}
