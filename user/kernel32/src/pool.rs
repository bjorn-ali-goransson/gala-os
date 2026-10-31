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
    _env: *mut c_void,
) -> *mut c_void {
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
    _env: *mut c_void,
) -> Bool {
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
    _env: *mut c_void,
) -> *mut c_void {
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
    _env: *mut c_void,
) -> *mut c_void {
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
