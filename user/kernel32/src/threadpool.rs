//! The thread pool (ARCHITECTURE "Thread pool"), from the documented
//! contracts and Wine's conformance tests (`threadpool_test`,
//! `kernel32_pool_test`):
//!
//! - **Pools** (TP_POOL): each has its own completion port and worker
//!   threads, started as work waits (at most `max`, at least `min` kept),
//!   named `threadpool worker (pool N)` (the default pool is pool 0). A
//!   callback is queued on its pool, in one of three priority lists
//!   (TP_CALLBACK_PRIORITY), and a token packet is posted to the port; a
//!   worker taking a token runs the first queued callback. I/O
//!   completions of a TP_IO's handle arrive on the same port with the
//!   object as their key. A worker idle for `IDLE_MS` leaves while more
//!   than the minimum remain.
//! - **Objects** (TP_WORK, TP_TIMER, TP_WAIT, TP_IO and simple callbacks)
//!   carry what their callback environment said: the pool, the cleanup
//!   group and its cancel callback, the finalization callback, the
//!   library kept loaded, the activation context, long-function and
//!   priority. They count queued (`pending`), running and associated
//!   callbacks under their pool's lock and are freed when the last
//!   reference goes: the creator's (until Close*), one per queued
//!   callback and per started I/O, and a cleanup group's while it closes
//!   its members. WaitFor*Callbacks sleeps on the object's condition
//!   variable until the counts are zero.
//! - **Timers**: one thread (`threadpool timers`) for every pool's timers,
//!   sorted by due time (the interrupt time, 100 ns); the window is not
//!   used (a callback at its due time is within any window).
//! - **Waits**: threads (`threadpool waits`) of up to 63 wait objects
//!   each, every one waiting on its own duplicate of the handle (so the
//!   caller may close it, as Windows allows) with WaitForMultipleObjects.
//! - **Cleanup groups**: a list of their members;
//!   CloseThreadpoolCleanupGroupMembers stops them, cancels their queued
//!   callbacks if asked (then calling each one's cancel callback on the
//!   calling thread), waits for their running callbacks and closes them.
//!
//! Then CreateTimerQueue and RegisterWaitForSingleObject on these objects,
//! and QueueUserWorkItem on simple callbacks.

use super::sync::{AcquireSRWLockExclusive, ReleaseSRWLockExclusive, WakeAllConditionVariable};
use super::*;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;
const WAIT_ABANDONED_0: u32 = 0x80;
const STATUS_TOO_MANY_THREADS: u32 = 0xc000_0129;
const ERROR_IO_PENDING: u32 = 997;
const DUPLICATE_SAME_ACCESS: u32 = 2;
const STACK_SIZE_PARAM_IS_A_RESERVATION: u32 = 0x1_0000;
/// A new pool's maximum (Windows' and Wine's default).
const DEFAULT_MAXIMUM: u32 = 500;
/// How long a worker above the minimum waits for work before it leaves.
const IDLE_MS: u32 = 20_000;
/// Completion keys of the pool's port: a queued callback, a worker's end
/// (anything else is the TP_IO whose I/O completed).
const KEY_TOKEN: usize = 0;
const KEY_EXIT: usize = 1;
/// TP_CALLBACK_PRIORITY_HIGH, NORMAL, LOW.
const PRIORITIES: usize = 3;
const NORMAL: usize = 1;
/// Wait objects per wait thread (WaitForMultipleObjects' 64 with the
/// thread's own event).
const BUCKET_WAITS: u32 = 63;

type SimpleCallback = extern "system" fn(*mut c_void, *mut c_void);
type WorkCallback = extern "system" fn(*mut c_void, *mut c_void, *mut c_void);
type WaitCallback = extern "system" fn(*mut c_void, *mut c_void, *mut c_void, u32);
type IoCallback =
    extern "system" fn(*mut c_void, *mut c_void, *mut c_void, u32, usize, *mut c_void);
type CancelCallback = extern "system" fn(*mut c_void, *mut c_void);

fn lock(word: *mut usize) {
    AcquireSRWLockExclusive(word);
}

fn unlock(word: *mut usize) {
    ReleaseSRWLockExclusive(word);
}

/// `v` in a new process-heap block (null when out of memory).
fn alloc<T>(v: T) -> *mut T {
    let p = super::heap::HeapAlloc(super::heap::GetProcessHeap(), 0, size_of::<T>()).cast::<T>();
    if !p.is_null() {
        // SAFETY: fresh memory of T's size.
        unsafe { p.write(v) };
    }
    p
}

fn free<T>(p: *mut T) {
    super::heap::HeapFree(super::heap::GetProcessHeap(), 0, p.cast());
}

/// The interrupt time (100 ns since boot): due times and timeouts.
fn now() -> u64 {
    let mut t = 0u64;
    super::mapping::QueryUnbiasedInterruptTime(&mut t);
    t
}

/// A FILETIME due time (negative: relative; positive: absolute, UTC; 0:
/// now) as an interrupt time.
fn deadline(due: i64) -> u64 {
    let now = now();
    if due <= 0 {
        return now + due.unsigned_abs();
    }
    let mut system = 0u64;
    super::mapping::GetSystemTimeAsFileTime(&mut system);
    now + (due as u64).saturating_sub(system)
}

/// Milliseconds from `now` to `at`, rounded up (below INFINITE).
fn ms_until(at: u64, now: u64) -> u32 {
    at.saturating_sub(now).div_ceil(10_000).min(u64::from(INFINITE - 1)) as u32
}

/// Names the calling thread (SetThreadDescription): `text`, then `n` in
/// decimal and `)` if given.
fn name_thread(text: &[u8], n: Option<u32>) {
    let mut buf = [0u16; 48];
    let mut len = 0;
    for &b in text {
        buf[len] = u16::from(b);
        len += 1;
    }
    if let Some(mut n) = n {
        let mut digits = [0u8; 10];
        let mut d = digits.len();
        loop {
            d -= 1;
            digits[d] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        for &b in digits[d..].iter().chain(b")") {
            buf[len] = u16::from(b);
            len += 1;
        }
    }
    super::runtime::SetThreadDescription(GetCurrentThread(), buf.as_ptr());
}

// ---- Pools ----

/// TP_POOL (opaque to programs).
struct Pool {
    lock: usize,
    port: Handle,
    id: u32,
    /// The creator's reference (none for the default pool's, which never
    /// goes) and one per object.
    refs: u32,
    max: u32,
    min: u32,
    workers: u32,
    /// Workers waiting on the port, or starting.
    idle: u32,
    /// Token packets posted and not yet taken.
    tokens: u32,
    /// Started I/O operations not yet completed: a worker stays for them.
    io: u32,
    /// TP_POOL_STACK_INFORMATION (reserve, commit); the image's at first.
    stack: [usize; 2],
    stack_set: bool,
    queues: [(*mut Object, *mut Object); PRIORITIES],
    /// The ledger has heard that work waited at the maximum.
    noted_full: bool,
}

static DEFAULT_POOL: AtomicUsize = AtomicUsize::new(0);
static NEXT_POOL_ID: AtomicU32 = AtomicU32::new(1);

fn new_pool(id: u32) -> *mut Pool {
    let port = CreateIoCompletionPort(INVALID_HANDLE_VALUE, core::ptr::null_mut(), 0, 0);
    if port.is_null() {
        return core::ptr::null_mut();
    }
    let p = alloc(Pool {
        lock: 0,
        port,
        id,
        refs: 1,
        max: DEFAULT_MAXIMUM,
        min: 0,
        workers: 0,
        idle: 0,
        tokens: 0,
        io: 0,
        stack: [default_stack(), default_stack_commit()],
        stack_set: false,
        queues: [(core::ptr::null_mut(), core::ptr::null_mut()); PRIORITIES],
        noted_full: false,
    });
    if p.is_null() {
        CloseHandle(port);
        return fail_with(ERROR_NOT_ENOUGH_MEMORY, p);
    }
    p
}

/// The process's default pool, made at first use (null when out of
/// memory).
fn default_pool() -> *mut Pool {
    let p = DEFAULT_POOL.load(Ordering::Acquire);
    if p != 0 {
        return p as *mut Pool;
    }
    let fresh = new_pool(0);
    if fresh.is_null() {
        return fresh;
    }
    match DEFAULT_POOL.compare_exchange(0, fresh as usize, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => fresh,
        Err(other) => {
            // SAFETY: our unused pool.
            CloseHandle(unsafe { (*fresh).port });
            free(fresh);
            other as *mut Pool
        }
    }
}

/// Starts a worker; the pool's lock held. It counts as idle until it has
/// waited once.
fn spawn_locked(pool: *mut Pool) -> bool {
    // SAFETY: a live pool, locked by the caller.
    let p = unsafe { &mut *pool };
    let (size, flags) =
        if p.stack_set { (p.stack[0], STACK_SIZE_PARAM_IS_A_RESERVATION) } else { (0, 0) };
    let t = CreateThread(
        core::ptr::null_mut(),
        size,
        worker,
        pool.cast(),
        flags,
        core::ptr::null_mut(),
    );
    if t.is_null() {
        unsupported(b"kernel32.dll!thread pool(worker not started)", u64::from(GetLastError()));
        return false;
    }
    CloseHandle(t);
    p.workers += 1;
    p.idle += 1;
    true
}

/// Drops a reference to `pool`; the last one (and no worker left) frees
/// it, otherwise its workers are told to end.
fn pool_release(pool: *mut Pool) {
    lock(pool_lock(pool));
    // SAFETY: a live pool, locked.
    let (gone, workers, port) = unsafe {
        (*pool).refs -= 1;
        ((*pool).refs == 0, (*pool).workers, (*pool).port)
    };
    unlock(pool_lock(pool));
    if !gone {
        return;
    }
    if workers == 0 {
        free_pool(pool);
        return;
    }
    for _ in 0..workers {
        PostQueuedCompletionStatus(port, 0, KEY_EXIT, 0);
    }
}

fn free_pool(pool: *mut Pool) {
    // SAFETY: a pool nothing refers to any more.
    CloseHandle(unsafe { (*pool).port });
    free(pool);
}

fn pool_lock(pool: *mut Pool) -> *mut usize {
    // SAFETY: a live pool's lock word.
    unsafe { &raw mut (*pool).lock }
}

/// The first queued object of the highest priority, taken off its list
/// (it goes back to the end if it has more callbacks queued).
fn pop(p: &mut Pool) -> *mut Object {
    for q in 0..PRIORITIES {
        let o = p.queues[q].0;
        if !o.is_null() {
            unlink_queue(p, o);
            return o;
        }
    }
    core::ptr::null_mut()
}

fn push_queue(p: &mut Pool, o: *mut Object) {
    // SAFETY: a live object of this pool, under its lock.
    unsafe {
        let q = &mut p.queues[(*o).priority];
        (*o).qnext = core::ptr::null_mut();
        (*o).qprev = q.1;
        if q.1.is_null() {
            q.0 = o;
        } else {
            (*q.1).qnext = o;
        }
        q.1 = o;
        (*o).queued = true;
    }
}

fn unlink_queue(p: &mut Pool, o: *mut Object) {
    // SAFETY: a queued object of this pool, under its lock.
    unsafe {
        if !(*o).queued {
            return;
        }
        let q = &mut p.queues[(*o).priority];
        if (*o).qprev.is_null() {
            q.0 = (*o).qnext;
        } else {
            (*(*o).qprev).qnext = (*o).qnext;
        }
        if (*o).qnext.is_null() {
            q.1 = (*o).qprev;
        } else {
            (*(*o).qnext).qprev = (*o).qprev;
        }
        (*o).queued = false;
    }
}

/// A worker: takes packets from its pool's port and runs callbacks.
extern "system" fn worker(param: *mut c_void) -> u32 {
    let pool = param.cast::<Pool>();
    // SAFETY: the pool that started us (alive while it has workers).
    let (port, id) = unsafe { ((*pool).port, (*pool).id) };
    name_thread(b"threadpool worker (pool ", Some(id));
    let mut counted = true;
    loop {
        if !counted {
            lock(pool_lock(pool));
            // SAFETY: locked.
            unsafe { (*pool).idle += 1 };
            unlock(pool_lock(pool));
        }
        counted = false;
        let packet = remove_completion(port, IDLE_MS);
        lock(pool_lock(pool));
        // SAFETY: locked; the pool lives while we are one of its workers.
        let p = unsafe { &mut *pool };
        p.idle -= 1;
        match packet {
            Err(status) => {
                let keep = p.min.max(u32::from(p.io > 0));
                if p.workers > keep && p.tokens == 0 {
                    p.workers -= 1;
                    let last = p.workers == 0 && p.refs == 0;
                    unlock(pool_lock(pool));
                    if last {
                        free_pool(pool);
                    }
                    return 0;
                }
                unlock(pool_lock(pool));
                if status != STATUS_TIMEOUT {
                    note(b"kernel32.dll!thread pool(port wait failed)", u64::from(status));
                    Sleep(10);
                }
            }
            Ok(e) if e.completion_key == KEY_EXIT => {
                p.workers -= 1;
                let last = p.workers == 0 && p.refs == 0;
                unlock(pool_lock(pool));
                if last {
                    free_pool(pool);
                }
                return 0;
            }
            Ok(e) if e.completion_key == KEY_TOKEN => {
                p.tokens -= 1;
                // More waiting and nobody free: one more worker comes.
                if p.tokens > 0 && p.idle == 0 {
                    if p.workers < p.max {
                        spawn_locked(pool);
                    } else if !p.noted_full {
                        p.noted_full = true;
                        note(
                            b"kernel32.dll!thread pool(callbacks wait at the maximum)",
                            u64::from(p.max),
                        );
                    }
                }
                let o = pop(p);
                if o.is_null() {
                    // Cancelled since.
                    unlock(pool_lock(pool));
                    continue;
                }
                execute(p, o);
            }
            Ok(e) => complete_io(p, e.completion_key as *mut Object, &e),
        }
    }
}

// ---- Objects ----

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Work,
    Timer,
    Wait,
    Io,
    /// TrySubmitThreadpoolCallback: run once, then freed.
    Simple,
}

/// TP_WORK, TP_TIMER, TP_WAIT, TP_IO (opaque to programs).
struct Object {
    kind: Kind,
    callback: usize,
    context: *mut c_void,
    pool: *mut Pool,
    refs: AtomicU32,
    /// Closed (by Close*, its cleanup group, or a simple callback's run).
    shutdown: bool,
    group: *mut Group,
    cancel: Option<CancelCallback>,
    finalization: Option<SimpleCallback>,
    race_dll: Handle,
    actctx: Handle,
    long: bool,
    priority: usize,
    /// The group's member list (the group's lock).
    gnext: *mut Object,
    gprev: *mut Object,
    in_group: bool,
    /// Joined `group` (holds a reference to it).
    joined: bool,
    /// The pool's queue and the counts (the pool's lock).
    qnext: *mut Object,
    qprev: *mut Object,
    queued: bool,
    pending: u32,
    running: u32,
    /// Running and not disassociated.
    associated: u32,
    /// Waits: queued callbacks of a signaled wait.
    signaled: u32,
    /// I/O: started operations; operations whose completion is dropped.
    io_pending: u32,
    io_skip: u32,
    /// Condition variable: the counts went down.
    done: usize,
    /// Timers (the timers' lock): due time, period (ms), set, in the list.
    due: u64,
    period: u32,
    set: bool,
    armed: bool,
    tnext: *mut Object,
    /// Waits (the waits' lock): the duplicated handle, the deadline,
    /// waiting, a count of settings, the wait thread and its list.
    handle: Handle,
    timeout: Option<u64>,
    waiting: bool,
    generation: u32,
    bucket: *mut Bucket,
    wnext: *mut Object,
    wprev: *mut Object,
}

/// TP_CALLBACK_ENVIRON_V3 (version 1 is its first eight fields).
#[repr(C)]
struct Environment {
    version: u32,
    pool: *mut Pool,
    group: *mut Group,
    cancel: Option<CancelCallback>,
    race_dll: Handle,
    actctx: Handle,
    finalization: Option<SimpleCallback>,
    flags: u32,
    priority: u32,
    size: u32,
}

/// A new object for `env` (null and the last error on failure).
fn create(
    kind: Kind,
    callback: usize,
    context: *mut c_void,
    env: *const Environment,
) -> *mut Object {
    let (mut pool, mut group, mut cancel, mut finalization) =
        (core::ptr::null_mut(), core::ptr::null_mut(), None, None);
    let (mut race_dll, mut actctx, mut long, mut priority) =
        (core::ptr::null_mut(), core::ptr::null_mut(), false, NORMAL);
    if !env.is_null() {
        // SAFETY: the caller's environment (version 3 read only as such).
        let e = unsafe { &*env };
        // Any other version is read as version 1 (Windows accepts version
        // 9999, threadpool_test; whether it reads its priority is not
        // measured).
        if e.version == 3 {
            if e.priority as usize >= PRIORITIES {
                return fail_with(ERROR_INVALID_PARAMETER, core::ptr::null_mut());
            }
            priority = e.priority as usize;
        }
        (pool, group, cancel, finalization) = (e.pool, e.group, e.cancel, e.finalization);
        (race_dll, actctx, long) = (e.race_dll, e.actctx, e.flags & 1 != 0);
        if e.flags & 2 != 0 {
            // TP_CALLBACK_ENVIRON's Persistent: a thread that never ends
            // (for APCs); the callbacks run on ordinary workers.
            unsupported(b"kernel32.dll!thread pool(persistent callback)", 0);
        }
    }
    if pool.is_null() {
        pool = default_pool();
        if pool.is_null() {
            return core::ptr::null_mut();
        }
    }
    let o = alloc(Object {
        kind,
        callback,
        context,
        pool,
        refs: AtomicU32::new(1),
        shutdown: false,
        group,
        cancel,
        finalization,
        race_dll: core::ptr::null_mut(),
        actctx,
        long,
        priority,
        gnext: core::ptr::null_mut(),
        gprev: core::ptr::null_mut(),
        in_group: false,
        joined: false,
        qnext: core::ptr::null_mut(),
        qprev: core::ptr::null_mut(),
        queued: false,
        pending: 0,
        running: 0,
        associated: 0,
        signaled: 0,
        io_pending: 0,
        io_skip: 0,
        done: 0,
        due: 0,
        period: 0,
        set: false,
        armed: false,
        tnext: core::ptr::null_mut(),
        handle: core::ptr::null_mut(),
        timeout: None,
        waiting: false,
        generation: 0,
        bucket: core::ptr::null_mut(),
        wnext: core::ptr::null_mut(),
        wprev: core::ptr::null_mut(),
    });
    if o.is_null() {
        return fail_with(ERROR_NOT_ENOUGH_MEMORY, o);
    }
    lock(pool_lock(pool));
    // SAFETY: a live pool, locked.
    unsafe { (*pool).refs += 1 };
    unlock(pool_lock(pool));
    // The library stays loaded while the object lives
    // (SetThreadpoolCallbackLibrary).
    if !race_dll.is_null() {
        let mut h = core::ptr::null_mut();
        // GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: a reference to the module.
        if super::text::GetModuleHandleExW(4, race_dll.cast(), &mut h) != FALSE {
            // SAFETY: our new object.
            unsafe { (*o).race_dll = h };
        }
    }
    if !actctx.is_null() {
        super::actctx::AddRefActCtx(actctx);
    }
    o
}

/// Adds `o` to its environment's cleanup group, if any.
fn join_group(o: *mut Object) {
    // SAFETY: our new object and the caller's group.
    unsafe {
        let g = (*o).group;
        if g.is_null() {
            return;
        }
        (*g).refs.fetch_add(1, Ordering::AcqRel);
        lock(&raw mut (*g).lock);
        (*o).gnext = (*g).head;
        if !(*g).head.is_null() {
            (*(*g).head).gprev = o;
        }
        (*g).head = o;
        (*o).in_group = true;
        (*o).joined = true;
        unlock(&raw mut (*g).lock);
    }
}

fn add_ref(o: *mut Object) {
    // SAFETY: a live object.
    unsafe { (*o).refs.fetch_add(1, Ordering::AcqRel) };
}

/// Drops a reference to `o`; the last frees it.
fn release(o: *mut Object) {
    // SAFETY: a live object; the last reference makes it ours alone.
    unsafe {
        if (*o).refs.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        let g = (*o).group;
        if (*o).joined {
            lock(&raw mut (*g).lock);
            if (*o).in_group {
                if (*o).gprev.is_null() {
                    (*g).head = (*o).gnext;
                } else {
                    (*(*o).gprev).gnext = (*o).gnext;
                }
                if !(*o).gnext.is_null() {
                    (*(*o).gnext).gprev = (*o).gprev;
                }
                (*o).in_group = false;
            }
            unlock(&raw mut (*g).lock);
            group_release(g);
        }
        if !(*o).race_dll.is_null() {
            FreeLibrary((*o).race_dll);
        }
        if !(*o).actctx.is_null() {
            super::actctx::ReleaseActCtx((*o).actctx);
        }
        let pool = (*o).pool;
        free(o);
        pool_release(pool);
    }
}

/// Queues one callback of `o` (a wait's with `signaled`); `reference`:
/// the callback holds a reference of its own (all but a simple
/// callback's single run, which holds the creator's).
fn submit(o: *mut Object, signaled: bool, reference: bool) {
    if reference {
        add_ref(o);
    }
    // SAFETY: a live object and its pool.
    let pool = unsafe { (*o).pool };
    lock(pool_lock(pool));
    // SAFETY: locked.
    let port = unsafe {
        let p = &mut *pool;
        (*o).pending += 1;
        if signaled {
            (*o).signaled += 1;
        }
        if !(*o).queued {
            push_queue(p, o);
        }
        p.tokens += 1;
        if p.idle == 0 {
            if p.workers < p.max {
                spawn_locked(pool);
            } else if !p.noted_full {
                p.noted_full = true;
                note(b"kernel32.dll!thread pool(callbacks wait at the maximum)", u64::from(p.max));
            }
        }
        p.port
    };
    unlock(pool_lock(pool));
    PostQueuedCompletionStatus(port, 0, KEY_TOKEN, 0);
}

/// TP_CALLBACK_INSTANCE: the running callback and what to do when it
/// returns.
struct Instance {
    object: *mut Object,
    thread: u32,
    associated: bool,
    may_run_long: bool,
    free_library: Handle,
    set_event: Handle,
    release_semaphore: (Handle, u32),
    release_mutex: Handle,
    leave_section: *mut sync::CriticalSection,
}

/// Runs one queued callback of `o`; `p` (its pool) locked, unlocked on
/// return.
fn execute(p: &mut Pool, o: *mut Object) {
    // SAFETY: a live object with a queued callback (its reference held),
    // under its pool's lock.
    let result = unsafe {
        (*o).pending -= 1;
        if (*o).pending > 0 {
            push_queue(p, o);
        }
        let result = if (*o).kind == Kind::Wait && (*o).signaled > 0 {
            (*o).signaled -= 1;
            WAIT_OBJECT_0
        } else {
            WAIT_TIMEOUT
        };
        (*o).running += 1;
        (*o).associated += 1;
        result
    };
    unlock(&raw mut p.lock);
    run(o, |instance, o| {
        // SAFETY: the callback the program registered for this kind.
        unsafe {
            let (callback, context) = ((*o).callback, (*o).context);
            match (*o).kind {
                Kind::Work | Kind::Timer => core::mem::transmute::<usize, WorkCallback>(callback)(
                    instance,
                    context,
                    o.cast(),
                ),
                Kind::Wait => core::mem::transmute::<usize, WaitCallback>(callback)(
                    instance,
                    context,
                    o.cast(),
                    result,
                ),
                Kind::Simple => {
                    core::mem::transmute::<usize, SimpleCallback>(callback)(instance, context)
                }
                Kind::Io => {}
            }
        }
    });
    release(o);
}

/// A completion on `o`'s handle; `p` locked, unlocked on return.
fn complete_io(p: &mut Pool, o: *mut Object, e: &OverlappedEntry) {
    // SAFETY: a TP_IO with a started operation (its reference held),
    // under its pool's lock.
    unsafe {
        if (*o).io_pending > 0 {
            (*o).io_pending -= 1;
            p.io -= 1;
        } else if (*o).io_skip > 0 {
            // Its wait cancelled it: dropped.
            (*o).io_skip -= 1;
            WakeAllConditionVariable(&raw mut (*o).done);
            unlock(&raw mut p.lock);
            release(o);
            return;
        } else {
            // A completion nobody announced with StartThreadpoolIo.
            note(b"kernel32.dll!StartThreadpoolIo(not called before the I/O)", 0);
            add_ref(o);
        }
        (*o).running += 1;
        (*o).associated += 1;
    }
    unlock(&raw mut p.lock);
    let status = e.internal as u32;
    // SAFETY: plain call.
    let error = if status == STATUS_SUCCESS { 0 } else { unsafe { RtlNtStatusToDosError(status) } };
    let (overlapped, bytes) = (e.overlapped as *mut c_void, e.bytes as usize);
    run(o, |instance, o| {
        // SAFETY: the program's PTP_WIN32_IO_CALLBACK.
        unsafe {
            core::mem::transmute::<usize, IoCallback>((*o).callback)(
                instance,
                (*o).context,
                overlapped,
                error,
                bytes,
                o.cast(),
            )
        }
    });
    release(o);
}

/// Runs a callback of `o` (counted as running by the caller) with an
/// instance, then the finalization callback and what the instance asked
/// for; counts it as done.
fn run(o: *mut Object, call: impl FnOnce(*mut c_void, *mut Object)) {
    // SAFETY: a live object (a reference held by the caller).
    let (long, actctx, finalization, context, pool, kind) =
        unsafe { ((*o).long, (*o).actctx, (*o).finalization, (*o).context, (*o).pool, (*o).kind) };
    let mut instance = Instance {
        object: o,
        thread: GetCurrentThreadId(),
        associated: true,
        may_run_long: long,
        free_library: core::ptr::null_mut(),
        set_event: core::ptr::null_mut(),
        release_semaphore: (core::ptr::null_mut(), 0),
        release_mutex: core::ptr::null_mut(),
        leave_section: core::ptr::null_mut(),
    };
    let inst = (&raw mut instance).cast::<c_void>();
    let mut cookie = 0usize;
    let activated =
        !actctx.is_null() && super::actctx::ActivateActCtx(actctx, &mut cookie) != FALSE;
    call(inst, o);
    if let Some(f) = finalization {
        f(inst, context);
    }
    if activated {
        super::actctx::DeactivateActCtx(0, cookie);
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
            instance.release_semaphore.1 as i32,
            core::ptr::null_mut(),
        );
    }
    if !instance.set_event.is_null() {
        SetEvent(instance.set_event);
    }
    if !instance.free_library.is_null() {
        FreeLibrary(instance.free_library);
    }
    lock(pool_lock(pool));
    // SAFETY: locked; the object is alive (the caller's reference).
    unsafe {
        if kind == Kind::Simple {
            (*o).shutdown = true;
        }
        (*o).running -= 1;
        if instance.associated {
            (*o).associated -= 1;
        }
        WakeAllConditionVariable(&raw mut (*o).done);
    }
    unlock(pool_lock(pool));
}

/// Drops `o`'s queued callbacks (and, for I/O, the started operations'
/// completions).
fn cancel_pending(o: *mut Object) {
    // SAFETY: a live object and its pool.
    let pool = unsafe { (*o).pool };
    lock(pool_lock(pool));
    // SAFETY: locked.
    let n = unsafe {
        let p = &mut *pool;
        let n = (*o).pending;
        if n > 0 {
            unlink_queue(p, o);
            (*o).pending = 0;
            (*o).signaled = 0;
        }
        (*o).io_skip += (*o).io_pending;
        p.io -= (*o).io_pending;
        (*o).io_pending = 0;
        WakeAllConditionVariable(&raw mut (*o).done);
        n
    };
    unlock(pool_lock(pool));
    for _ in 0..n {
        release(o);
    }
}

/// Waits until `o` has no queued callback or started I/O and none
/// running (`group`) or associated (WaitFor*Callbacks: a disassociated
/// callback no longer counts).
fn wait_finished(o: *mut Object, group: bool) {
    // SAFETY: a live object and its pool.
    let pool = unsafe { (*o).pool };
    lock(pool_lock(pool));
    // SAFETY: the counts under the pool's lock; the condition variable is
    // woken under it whenever they go down.
    unsafe {
        while (*o).pending != 0
            || (*o).io_pending != 0
            || (if group { (*o).running } else { (*o).associated }) != 0
        {
            sync::SleepConditionVariableSRW(&raw mut (*o).done, pool_lock(pool), INFINITE, 0);
        }
    }
    unlock(pool_lock(pool));
}

/// No more callbacks are queued by `o`'s timer or wait.
fn stop(o: *mut Object) {
    // SAFETY: a live object.
    match unsafe { (*o).kind } {
        Kind::Timer => disarm_timer(o),
        Kind::Wait => leave_bucket(o),
        _ => {}
    }
}

/// Close*: stopped, then the creator's reference dropped (queued
/// callbacks still run; the object goes after them).
fn close(o: *mut Object) {
    if o.is_null() {
        return;
    }
    stop(o);
    // SAFETY: a live object and its pool.
    let pool = unsafe { (*o).pool };
    lock(pool_lock(pool));
    // SAFETY: locked.
    let was = unsafe { core::mem::replace(&mut (*o).shutdown, true) };
    unlock(pool_lock(pool));
    if !was {
        release(o);
    }
}

fn wait_callbacks(o: *mut Object, cancel: Bool) {
    if o.is_null() {
        return;
    }
    if cancel != FALSE {
        cancel_pending(o);
    }
    wait_finished(o, false);
}

// ---- Pools: the API ----

/// CreateThreadpool(reserved): a new private pool.
#[unsafe(no_mangle)]
pub extern "system" fn CreateThreadpool(_reserved: *mut c_void) -> *mut c_void {
    new_pool(NEXT_POOL_ID.fetch_add(1, Ordering::Relaxed)).cast()
}

/// CloseThreadpool: the pool goes once its objects have (their queued
/// callbacks still run).
#[unsafe(no_mangle)]
pub extern "system" fn CloseThreadpool(pool: *mut c_void) {
    if !pool.is_null() {
        pool_release(pool.cast());
    }
}

/// SetThreadpoolThreadMaximum: at least 1; the minimum comes down to it.
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadpoolThreadMaximum(pool: *mut c_void, most: u32) {
    let pool = pool.cast::<Pool>();
    if pool.is_null() {
        return;
    }
    lock(pool_lock(pool));
    // SAFETY: locked.
    unsafe {
        (*pool).max = most.max(1);
        (*pool).min = (*pool).min.min((*pool).max);
    }
    unlock(pool_lock(pool));
}

/// SetThreadpoolThreadMinimum: that many workers started now (FALSE if
/// one cannot be); the maximum goes up to it.
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadpoolThreadMinimum(pool: *mut c_void, least: u32) -> Bool {
    let pool = pool.cast::<Pool>();
    if pool.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    lock(pool_lock(pool));
    // SAFETY: locked.
    let ok = unsafe {
        while (*pool).workers < least && spawn_locked(pool) {}
        let ok = (*pool).workers >= least;
        if ok {
            (*pool).min = least;
            (*pool).max = (*pool).max.max(least);
        }
        ok
    };
    unlock(pool_lock(pool));
    Bool::from(ok)
}

/// SetThreadpoolStackInformation: the stack of workers started from now
/// (not validated, as on Windows).
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadpoolStackInformation(
    pool: *mut c_void,
    info: *const [usize; 2],
) -> Bool {
    let pool = pool.cast::<Pool>();
    if pool.is_null() || info.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    lock(pool_lock(pool));
    // SAFETY: locked; the caller's TP_POOL_STACK_INFORMATION.
    unsafe {
        (*pool).stack = info.read_unaligned();
        (*pool).stack_set = true;
    }
    unlock(pool_lock(pool));
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn QueryThreadpoolStackInformation(
    pool: *mut c_void,
    info: *mut [usize; 2],
) -> Bool {
    let pool = pool.cast::<Pool>();
    if pool.is_null() || info.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    lock(pool_lock(pool));
    // SAFETY: locked; the caller's TP_POOL_STACK_INFORMATION.
    unsafe { info.write_unaligned((*pool).stack) };
    unlock(pool_lock(pool));
    TRUE
}

// ---- Cleanup groups ----

/// TP_CLEANUP_GROUP: its members, linked by `gnext`/`gprev`.
struct Group {
    lock: usize,
    /// The creator's and one per object made with it.
    refs: AtomicU32,
    head: *mut Object,
}

fn group_release(g: *mut Group) {
    // SAFETY: a live group; the last reference makes it ours alone.
    if unsafe { (*g).refs.fetch_sub(1, Ordering::AcqRel) } == 1 {
        free(g);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateThreadpoolCleanupGroup() -> *mut c_void {
    let g = alloc(Group { lock: 0, refs: AtomicU32::new(1), head: core::ptr::null_mut() });
    if g.is_null() {
        return fail_with(ERROR_NOT_ENOUGH_MEMORY, core::ptr::null_mut());
    }
    g.cast()
}

/// CloseThreadpoolCleanupGroup: the group goes once its objects have.
#[unsafe(no_mangle)]
pub extern "system" fn CloseThreadpoolCleanupGroup(group: *mut c_void) {
    if !group.is_null() {
        group_release(group.cast());
    }
}

/// CloseThreadpoolCleanupGroupMembers(group, fCancelPendingCallbacks,
/// pvCleanupContext): every member stopped; with cancellation its queued
/// callbacks dropped; its running callbacks waited for; then, with
/// cancellation, its cancel callback called here (object context, cleanup
/// context) unless it was closed meanwhile; then closed.
#[unsafe(no_mangle)]
pub extern "system" fn CloseThreadpoolCleanupGroupMembers(
    group: *mut c_void,
    cancel: Bool,
    context: *mut c_void,
) {
    let g = group.cast::<Group>();
    if g.is_null() {
        return;
    }
    // Take the members (each with a reference of ours; one already going
    // is left to go).
    let mut members: *mut Object = core::ptr::null_mut();
    // SAFETY: a live group; its list under its lock.
    unsafe {
        lock(&raw mut (*g).lock);
        let mut o = (*g).head;
        (*g).head = core::ptr::null_mut();
        while !o.is_null() {
            let next = (*o).gnext;
            (*o).in_group = false;
            let mut n = (*o).refs.load(Ordering::Acquire);
            let alive = loop {
                if n == 0 {
                    break false;
                }
                match (*o).refs.compare_exchange(n, n + 1, Ordering::AcqRel, Ordering::Acquire) {
                    Ok(_) => break true,
                    Err(seen) => n = seen,
                }
            };
            if alive {
                (*o).gnext = members;
                members = o;
            }
            o = next;
        }
        unlock(&raw mut (*g).lock);
    }
    let each = |f: &mut dyn FnMut(*mut Object)| {
        let mut o = members;
        while !o.is_null() {
            // SAFETY: a member we hold a reference to.
            let next = unsafe { (*o).gnext };
            f(o);
            o = next;
        }
    };
    each(&mut |o| stop(o));
    if cancel != FALSE {
        each(&mut |o| cancel_pending(o));
    }
    each(&mut |o| {
        wait_finished(o, true);
        // SAFETY: a member we hold a reference to.
        let (pool, kind, callback, object_context) =
            unsafe { ((*o).pool, (*o).kind, (*o).cancel, (*o).context) };
        lock(pool_lock(pool));
        // SAFETY: locked.
        let was = unsafe { core::mem::replace(&mut (*o).shutdown, true) };
        unlock(pool_lock(pool));
        if !was {
            if cancel != FALSE
                && let Some(f) = callback
            {
                f(object_context, context);
            }
            // The creator's reference (a simple callback's went with its
            // cancelled run).
            if kind != Kind::Simple {
                release(o);
            }
        }
        release(o);
    });
}

// ---- Work and simple callbacks ----

#[unsafe(no_mangle)]
pub extern "system" fn CreateThreadpoolWork(
    callback: WorkCallback,
    context: *mut c_void,
    env: *const c_void,
) -> *mut c_void {
    let o = create(Kind::Work, callback as usize, context, env.cast());
    if !o.is_null() {
        join_group(o);
    }
    o.cast()
}

#[unsafe(no_mangle)]
pub extern "system" fn SubmitThreadpoolWork(work: *mut c_void) {
    if !work.is_null() {
        submit(work.cast(), false, true);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn WaitForThreadpoolWorkCallbacks(work: *mut c_void, cancel: Bool) {
    wait_callbacks(work.cast(), cancel);
}

#[unsafe(no_mangle)]
pub extern "system" fn CloseThreadpoolWork(work: *mut c_void) {
    close(work.cast());
}

/// TrySubmitThreadpoolCallback: runs `callback(instance, context)` once.
#[unsafe(no_mangle)]
pub extern "system" fn TrySubmitThreadpoolCallback(
    callback: SimpleCallback,
    context: *mut c_void,
    env: *const c_void,
) -> Bool {
    let o = create(Kind::Simple, callback as usize, context, env.cast());
    if o.is_null() {
        return FALSE;
    }
    // Queued before it joins the group: the group may close it at once.
    submit(o, false, false);
    join_group(o);
    TRUE
}

// ---- Timers: one thread for all ----

static TIMERS_LOCK: AtomicUsize = AtomicUsize::new(0);
/// The armed timers by due time (`tnext`), and the thread's event.
static TIMERS: AtomicUsize = AtomicUsize::new(0);
static TIMERS_EVENT: AtomicUsize = AtomicUsize::new(0);

/// The timer thread's event, the thread started at first use (the
/// timers' lock held).
fn timers_event() -> Handle {
    let e = TIMERS_EVENT.load(Ordering::Acquire);
    if e != 0 {
        return e as Handle;
    }
    let event = super::sync::CreateEventW(core::ptr::null_mut(), FALSE, FALSE, core::ptr::null());
    if event.is_null() {
        return event;
    }
    let t = CreateThread(
        core::ptr::null_mut(),
        0,
        timer_thread,
        core::ptr::null_mut(),
        0,
        core::ptr::null_mut(),
    );
    if t.is_null() {
        unsupported(
            b"kernel32.dll!thread pool(timer thread not started)",
            u64::from(GetLastError()),
        );
        CloseHandle(event);
        return core::ptr::null_mut();
    }
    CloseHandle(t);
    TIMERS_EVENT.store(event as usize, Ordering::Release);
    event
}

/// Puts `o` in the sorted list (the timers' lock held).
fn arm_timer(o: *mut Object) {
    // SAFETY: live objects under the timers' lock.
    unsafe {
        let mut link = TIMERS.as_ptr().cast::<*mut Object>();
        while !(*link).is_null() && (**link).due <= (*o).due {
            link = &raw mut (**link).tnext;
        }
        (*o).tnext = *link;
        *link = o;
        (*o).armed = true;
    }
}

/// Takes `o` off the list (the timers' lock held).
fn unarm_timer(o: *mut Object) {
    // SAFETY: live objects under the timers' lock.
    unsafe {
        if !(*o).armed {
            return;
        }
        let mut link = TIMERS.as_ptr().cast::<*mut Object>();
        while !(*link).is_null() {
            if *link == o {
                *link = (*o).tnext;
                break;
            }
            link = &raw mut (**link).tnext;
        }
        (*o).armed = false;
    }
}

fn disarm_timer(o: *mut Object) {
    lock(TIMERS_LOCK.as_ptr());
    unarm_timer(o);
    // SAFETY: under the timers' lock.
    unsafe { (*o).set = false };
    unlock(TIMERS_LOCK.as_ptr());
}

extern "system" fn timer_thread(_: *mut c_void) -> u32 {
    name_thread(b"threadpool timers", None);
    loop {
        lock(TIMERS_LOCK.as_ptr());
        let t = now();
        let wait = loop {
            let head = TIMERS.load(Ordering::Relaxed) as *mut Object;
            if head.is_null() {
                break INFINITE;
            }
            // SAFETY: an armed timer, under the timers' lock (Close takes
            // it off first).
            unsafe {
                if (*head).due > t {
                    break ms_until((*head).due, t);
                }
                unarm_timer(head);
                if (*head).period != 0 {
                    let period = u64::from((*head).period) * 10_000;
                    (*head).due += period;
                    if (*head).due <= t {
                        (*head).due = t + period;
                    }
                    arm_timer(head);
                }
            }
            submit(head, false, true);
        };
        let event = TIMERS_EVENT.load(Ordering::Acquire) as Handle;
        unlock(TIMERS_LOCK.as_ptr());
        WaitForSingleObject(event, wait);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateThreadpoolTimer(
    callback: WorkCallback,
    context: *mut c_void,
    env: *const c_void,
) -> *mut c_void {
    let o = create(Kind::Timer, callback as usize, context, env.cast());
    if !o.is_null() {
        join_group(o);
    }
    o.cast()
}

/// Sets `timer`; whether it was set before.
fn set_timer(o: *mut Object, due: *const i64, period: u32) -> bool {
    if o.is_null() {
        return false;
    }
    lock(TIMERS_LOCK.as_ptr());
    let event = timers_event();
    // SAFETY: a live timer, under the timers' lock; the caller's FILETIME.
    let (was, now_too) = unsafe {
        let was = (*o).set;
        unarm_timer(o);
        (*o).set = !due.is_null();
        (*o).period = period;
        let mut now_too = false;
        if !due.is_null() {
            let d = due.read_unaligned();
            if d == 0 {
                // Now, then (periodic) every period.
                now_too = true;
                if period != 0 {
                    (*o).due = now() + u64::from(period) * 10_000;
                    arm_timer(o);
                }
            } else {
                (*o).due = deadline(d);
                arm_timer(o);
            }
        }
        (was, now_too)
    };
    if !event.is_null() {
        SetEvent(event);
    }
    unlock(TIMERS_LOCK.as_ptr());
    if now_too {
        submit(o, false, true);
    }
    was
}

/// SetThreadpoolTimer: a NULL due time stops the timer (queued callbacks
/// still run); 0 fires at once; msPeriod 0 is one-shot. The timer stays
/// set (IsThreadpoolTimerSet) after a one-shot fired.
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadpoolTimer(
    timer: *mut c_void,
    due: *const i64,
    period: u32,
    _window: u32,
) {
    set_timer(timer.cast(), due, period);
}

/// SetThreadpoolTimerEx: as SetThreadpoolTimer; TRUE if it was set.
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadpoolTimerEx(
    timer: *mut c_void,
    due: *const i64,
    period: u32,
    _window: u32,
) -> Bool {
    Bool::from(set_timer(timer.cast(), due, period))
}

#[unsafe(no_mangle)]
pub extern "system" fn IsThreadpoolTimerSet(timer: *mut c_void) -> Bool {
    if timer.is_null() {
        return FALSE;
    }
    lock(TIMERS_LOCK.as_ptr());
    // SAFETY: a live timer, under the timers' lock.
    let set = unsafe { (*timer.cast::<Object>()).set };
    unlock(TIMERS_LOCK.as_ptr());
    Bool::from(set)
}

#[unsafe(no_mangle)]
pub extern "system" fn WaitForThreadpoolTimerCallbacks(timer: *mut c_void, cancel: Bool) {
    wait_callbacks(timer.cast(), cancel);
}

#[unsafe(no_mangle)]
pub extern "system" fn CloseThreadpoolTimer(timer: *mut c_void) {
    close(timer.cast());
}

// ---- Waits: threads of up to 63 ----

/// A wait thread: its event (settings changed), its wait objects
/// (`wnext`/`wprev`) and the handles of its current wait.
///
/// **Invariant: a handle the wait thread is waiting on stays open until
/// that wait returns.** The thread takes its handles under the waits'
/// lock and waits after letting it go; a setting changed meanwhile must
/// not close one of them under it (the wait would fail with
/// ERROR_INVALID_HANDLE, or wait on whatever object the value names
/// next). `drop_handle` marks such a handle `retired` instead, and the
/// thread closes it when its wait returns.
struct Bucket {
    next: *mut Bucket,
    event: Handle,
    count: u32,
    head: *mut Object,
    /// While the thread waits: the handles it waits on (`handles[1..n]`,
    /// `handles[0]` is `event`) and which of them are to be closed when
    /// the wait returns.
    in_wait: bool,
    n: usize,
    handles: [Handle; 64],
    retired: [bool; 64],
}

static WAITS_LOCK: AtomicUsize = AtomicUsize::new(0);
/// The wait threads (`next`).
static BUCKETS: AtomicUsize = AtomicUsize::new(0);

/// Gives `o` a wait thread (a new one when all have 63).
fn join_bucket(o: *mut Object) -> bool {
    lock(WAITS_LOCK.as_ptr());
    // SAFETY: the buckets under the waits' lock.
    let ok = unsafe {
        let mut b = BUCKETS.load(Ordering::Relaxed) as *mut Bucket;
        while !b.is_null() && (*b).count >= BUCKET_WAITS {
            b = (*b).next;
        }
        if b.is_null() {
            b = new_bucket();
        }
        if !b.is_null() {
            (*o).bucket = b;
            (*o).wprev = core::ptr::null_mut();
            (*o).wnext = (*b).head;
            if !(*b).head.is_null() {
                (*(*b).head).wprev = o;
            }
            (*b).head = o;
            (*b).count += 1;
        }
        !b.is_null()
    };
    unlock(WAITS_LOCK.as_ptr());
    ok
}

/// A new wait thread (the waits' lock held).
fn new_bucket() -> *mut Bucket {
    let event = super::sync::CreateEventW(core::ptr::null_mut(), FALSE, FALSE, core::ptr::null());
    if event.is_null() {
        return core::ptr::null_mut();
    }
    let b = alloc(Bucket {
        next: core::ptr::null_mut(),
        event,
        count: 0,
        head: core::ptr::null_mut(),
        in_wait: false,
        n: 0,
        handles: [core::ptr::null_mut(); 64],
        retired: [false; 64],
    });
    if b.is_null() {
        CloseHandle(event);
        return fail_with(ERROR_NOT_ENOUGH_MEMORY, b);
    }
    let t = CreateThread(core::ptr::null_mut(), 0, wait_thread, b.cast(), 0, core::ptr::null_mut());
    if t.is_null() {
        unsupported(
            b"kernel32.dll!thread pool(wait thread not started)",
            u64::from(GetLastError()),
        );
        CloseHandle(event);
        free(b);
        return core::ptr::null_mut();
    }
    CloseHandle(t);
    // SAFETY: our new bucket; the list under the waits' lock.
    unsafe { (*b).next = BUCKETS.load(Ordering::Relaxed) as *mut Bucket };
    BUCKETS.store(b as usize, Ordering::Relaxed);
    b
}

/// Drops `o`'s handle (the waits' lock held): closed now, or, if its wait
/// thread is waiting on it, when that wait returns (`Bucket`'s invariant).
fn drop_handle(o: *mut Object) {
    // SAFETY: a live wait and its bucket under the waits' lock.
    unsafe {
        let (h, b) = ((*o).handle, (*o).bucket);
        if !h.is_null() {
            let waited = if !b.is_null() && (*b).in_wait {
                (1..(*b).n).find(|&i| (*b).handles[i] == h)
            } else {
                None
            };
            match waited {
                Some(i) => (*b).retired[i] = true,
                None => {
                    CloseHandle(h);
                }
            }
        }
        (*o).handle = core::ptr::null_mut();
    }
}

/// Takes `o` off its wait thread: no more callbacks from it.
fn leave_bucket(o: *mut Object) {
    lock(WAITS_LOCK.as_ptr());
    // SAFETY: a live wait and its bucket under the waits' lock.
    unsafe {
        let b = (*o).bucket;
        if !b.is_null() {
            if (*o).wprev.is_null() {
                (*b).head = (*o).wnext;
            } else {
                (*(*o).wprev).wnext = (*o).wnext;
            }
            if !(*o).wnext.is_null() {
                (*(*o).wnext).wprev = (*o).wprev;
            }
            (*b).count -= 1;
            // While `bucket` still names the thread that may wait on it.
            drop_handle(o);
            (*o).bucket = core::ptr::null_mut();
            (*o).waiting = false;
            SetEvent((*b).event);
        }
    }
    unlock(WAITS_LOCK.as_ptr());
}

extern "system" fn wait_thread(param: *mut c_void) -> u32 {
    let b = param.cast::<Bucket>();
    name_thread(b"threadpool waits", None);
    let mut objects = [core::ptr::null_mut::<Object>(); 64];
    let mut generations = [0u32; 64];
    loop {
        lock(WAITS_LOCK.as_ptr());
        // SAFETY: our bucket and its waits under the waits' lock.
        let (n, wait) = unsafe {
            if (*b).count == 0 {
                // Unused: leave the list and end.
                let mut link = BUCKETS.as_ptr().cast::<*mut Bucket>();
                while !(*link).is_null() && *link != b {
                    link = &raw mut (**link).next;
                }
                if *link == b {
                    *link = (*b).next;
                }
                unlock(WAITS_LOCK.as_ptr());
                CloseHandle((*b).event);
                free(b);
                return 0;
            }
            (*b).handles[0] = (*b).event;
            let mut n = 1;
            let mut first: Option<u64> = None;
            let mut o = (*b).head;
            while !o.is_null() {
                if (*o).waiting {
                    (*b).handles[n] = (*o).handle;
                    (*b).retired[n] = false;
                    objects[n] = o;
                    generations[n] = (*o).generation;
                    n += 1;
                    if let Some(t) = (*o).timeout {
                        first = Some(first.map_or(t, |f| f.min(t)));
                    }
                }
                o = (*o).wnext;
            }
            (*b).n = n;
            (*b).in_wait = true;
            (n, first.map_or(INFINITE, |f| ms_until(f, now())))
        };
        unlock(WAITS_LOCK.as_ptr());
        // SAFETY: the bucket's handles, which no other thread changes (or
        // closes) while `in_wait` is set.
        let r = WaitForMultipleObjects(n as u32, unsafe { (*b).handles.as_ptr() }, FALSE, wait);
        let error = GetLastError();
        lock(WAITS_LOCK.as_ptr());
        let i = r as usize;
        // SAFETY: as above; an object is used only if it is still ours and
        // set as it was when the wait began.
        unsafe {
            (*b).in_wait = false;
            // WAIT_ABANDONED_0 + i (a mutex its owner left) satisfies the
            // wait too.
            let i = if (WAIT_ABANDONED_0 as usize..WAIT_ABANDONED_0 as usize + n).contains(&i) {
                i - WAIT_ABANDONED_0 as usize
            } else {
                i
            };
            if (1..n).contains(&i) {
                let o = objects[i];
                if o_is_current(b, o, generations[i]) {
                    fired(o, true);
                }
            } else if r == WAIT_FAILED {
                // Cannot happen: every handle is our own duplicate, found
                // waitable when it was set, and open until this wait
                // returned. Should it, the thread's waits stop rather
                // than fail again at once, and the ledger says so.
                note(b"kernel32.dll!thread pool(wait thread's wait failed)", u64::from(error));
                for k in 1..n {
                    if o_is_current(b, objects[k], generations[k]) {
                        (*objects[k]).waiting = false;
                        drop_handle(objects[k]);
                    }
                }
            }
            for k in 1..n {
                if (*b).retired[k] {
                    (*b).retired[k] = false;
                    CloseHandle((*b).handles[k]);
                }
            }
            let t = now();
            let mut o = (*b).head;
            while !o.is_null() {
                let next = (*o).wnext;
                if (*o).waiting && (*o).timeout.is_some_and(|d| d <= t) {
                    fired(o, false);
                }
                o = next;
            }
        }
        unlock(WAITS_LOCK.as_ptr());
    }
}

/// `o`'s wait ended (`signaled`, or timed out): its callback queued and
/// its handle closed, as Windows keeps no reference to the object once a
/// wait is over (the waits' lock held; the thread is not waiting).
fn fired(o: *mut Object, signaled: bool) {
    // SAFETY: a live wait under the waits' lock.
    unsafe { (*o).waiting = false };
    drop_handle(o);
    submit(o, signaled, true);
}

/// `o` still waits on bucket `b` with the setting of `generation` (the
/// waits' lock held).
fn o_is_current(b: *mut Bucket, o: *mut Object, generation: u32) -> bool {
    // SAFETY: an object of the bucket's list (objects leave it under the
    // waits' lock before they can go).
    let mut p = unsafe { (*b).head };
    while !p.is_null() {
        if p == o {
            // SAFETY: as above.
            return unsafe { (*o).waiting && (*o).generation == generation };
        }
        // SAFETY: as above.
        p = unsafe { (*p).wnext };
    }
    false
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateThreadpoolWait(
    callback: WaitCallback,
    context: *mut c_void,
    env: *const c_void,
) -> *mut c_void {
    let o = create(Kind::Wait, callback as usize, context, env.cast());
    if o.is_null() {
        return o.cast();
    }
    if !join_bucket(o) {
        let e = GetLastError();
        release(o);
        return fail_with(e, core::ptr::null_mut());
    }
    join_group(o);
    o.cast()
}

/// Sets `wait`; whether it was waiting.
fn set_wait(o: *mut Object, handle: Handle, timeout: *const i64) -> bool {
    if o.is_null() {
        return false;
    }
    lock(WAITS_LOCK.as_ptr());
    // SAFETY: a live wait under the waits' lock; the caller's FILETIME.
    let was = unsafe {
        let b = (*o).bucket;
        let was = (*o).waiting;
        if !b.is_null() {
            drop_handle(o);
            (*o).generation = (*o).generation.wrapping_add(1);
            (*o).waiting = false;
            if !handle.is_null() {
                // A duplicate: the caller may close its handle meanwhile.
                let mut dup = core::ptr::null_mut();
                let process = GetCurrentProcess();
                let duplicated = DuplicateHandle(
                    process,
                    handle,
                    process,
                    &mut dup,
                    0,
                    FALSE,
                    DUPLICATE_SAME_ACCESS,
                ) != FALSE;
                // Whether it can be waited on, before the wait thread
                // waits on it with the others (one handle that cannot
                // would fail all their waits): a poll, which is the wait
                // itself when the object is signaled already.
                let poll = if duplicated { WaitForSingleObject(dup, 0) } else { WAIT_FAILED };
                if poll == WAIT_FAILED {
                    note(
                        b"kernel32.dll!SetThreadpoolWait(handle not waitable)",
                        u64::from(GetLastError()),
                    );
                    if duplicated {
                        CloseHandle(dup);
                    }
                } else {
                    (*o).handle = dup;
                    (*o).timeout = if timeout.is_null() {
                        None
                    } else {
                        Some(deadline(timeout.read_unaligned()))
                    };
                    (*o).waiting = true;
                    if poll != WAIT_TIMEOUT {
                        // WAIT_OBJECT_0 or WAIT_ABANDONED: satisfied now.
                        fired(o, true);
                    }
                }
            }
            SetEvent((*b).event);
        }
        was
    };
    unlock(WAITS_LOCK.as_ptr());
    was
}

/// SetThreadpoolWait: waits once for `handle` (NULL stops waiting), with
/// an optional timeout (a FILETIME due time; NULL: none).
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadpoolWait(wait: *mut c_void, handle: Handle, timeout: *const i64) {
    set_wait(wait.cast(), handle, timeout);
}

/// SetThreadpoolWaitEx: as SetThreadpoolWait; TRUE if a pending wait was
/// replaced or stopped.
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadpoolWaitEx(
    wait: *mut c_void,
    handle: Handle,
    timeout: *const i64,
    _reserved: *mut c_void,
) -> Bool {
    Bool::from(set_wait(wait.cast(), handle, timeout))
}

#[unsafe(no_mangle)]
pub extern "system" fn WaitForThreadpoolWaitCallbacks(wait: *mut c_void, cancel: Bool) {
    wait_callbacks(wait.cast(), cancel);
}

#[unsafe(no_mangle)]
pub extern "system" fn CloseThreadpoolWait(wait: *mut c_void) {
    close(wait.cast());
}

// ---- I/O ----

/// CreateThreadpoolIo(handle, callback, context, env): `handle` bound to
/// the pool's completion port with the object as the key.
#[unsafe(no_mangle)]
pub extern "system" fn CreateThreadpoolIo(
    file: Handle,
    callback: IoCallback,
    context: *mut c_void,
    env: *const c_void,
) -> *mut c_void {
    let o = create(Kind::Io, callback as usize, context, env.cast());
    if o.is_null() {
        return o.cast();
    }
    // SAFETY: our new object and its pool.
    let port = unsafe { (*(*o).pool).port };
    if CreateIoCompletionPort(file, port, o as usize, 0).is_null() {
        let e = GetLastError();
        release(o);
        return fail_with(e, core::ptr::null_mut());
    }
    join_group(o);
    o.cast()
}

/// StartThreadpoolIo: one more operation whose completion runs the
/// callback (a worker stays for it).
#[unsafe(no_mangle)]
pub extern "system" fn StartThreadpoolIo(io: *mut c_void) {
    let o = io.cast::<Object>();
    if o.is_null() {
        return;
    }
    add_ref(o);
    // SAFETY: a live TP_IO and its pool.
    let pool = unsafe { (*o).pool };
    lock(pool_lock(pool));
    // SAFETY: locked.
    unsafe {
        (*o).io_pending += 1;
        (*pool).io += 1;
        if (*pool).workers == 0 {
            spawn_locked(pool);
        }
    }
    unlock(pool_lock(pool));
}

/// CancelThreadpoolIo: the operation started last did not start after
/// all.
#[unsafe(no_mangle)]
pub extern "system" fn CancelThreadpoolIo(io: *mut c_void) {
    let o = io.cast::<Object>();
    if o.is_null() {
        return;
    }
    // SAFETY: a live TP_IO and its pool.
    let pool = unsafe { (*o).pool };
    lock(pool_lock(pool));
    // SAFETY: locked.
    let had = unsafe {
        let had = (*o).io_pending > 0;
        if had {
            (*o).io_pending -= 1;
            (*pool).io -= 1;
            WakeAllConditionVariable(&raw mut (*o).done);
        }
        had
    };
    unlock(pool_lock(pool));
    if had {
        release(o);
    }
}

/// WaitForThreadpoolIoCallbacks: until every started operation's callback
/// has run; with cancellation the started ones' completions are dropped.
#[unsafe(no_mangle)]
pub extern "system" fn WaitForThreadpoolIoCallbacks(io: *mut c_void, cancel: Bool) {
    wait_callbacks(io.cast(), cancel);
}

#[unsafe(no_mangle)]
pub extern "system" fn CloseThreadpoolIo(io: *mut c_void) {
    close(io.cast());
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
        i.release_semaphore = (semaphore, count);
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

/// CallbackMayRunLong: TRUE if another worker is free or can be started
/// (it is); FALSE (STATUS_TOO_MANY_THREADS) at the pool's maximum. The
/// callback counts as long either way.
#[unsafe(no_mangle)]
pub extern "system" fn CallbackMayRunLong(i: *mut c_void) -> Bool {
    let Some(i) = instance(i) else { return fail_with(ERROR_INVALID_PARAMETER, FALSE) };
    if i.may_run_long {
        return TRUE;
    }
    i.may_run_long = true;
    // SAFETY: the running callback's object and pool.
    let pool = unsafe { (*i.object).pool };
    lock(pool_lock(pool));
    // SAFETY: locked.
    let ok = unsafe { (*pool).idle > 0 || ((*pool).workers < (*pool).max && spawn_locked(pool)) };
    unlock(pool_lock(pool));
    if ok { TRUE } else { fail(STATUS_TOO_MANY_THREADS, FALSE) }
}

/// DisassociateCurrentThreadFromCallback: WaitFor*Callbacks no longer
/// waits for this callback (a cleanup group still does).
#[unsafe(no_mangle)]
pub extern "system" fn DisassociateCurrentThreadFromCallback(i: *mut c_void) {
    let Some(i) = instance(i) else { return };
    if !i.associated || i.thread != GetCurrentThreadId() {
        return;
    }
    let o = i.object;
    // SAFETY: the running callback's object and pool.
    let pool = unsafe { (*o).pool };
    lock(pool_lock(pool));
    // SAFETY: locked.
    unsafe {
        (*o).associated -= 1;
        WakeAllConditionVariable(&raw mut (*o).done);
    }
    unlock(pool_lock(pool));
    i.associated = false;
}

/// Whether a callback of `o` is running.
fn running(o: *mut Object) -> bool {
    // SAFETY: a live object and its pool.
    let pool = unsafe { (*o).pool };
    lock(pool_lock(pool));
    // SAFETY: locked.
    let r = unsafe { (*o).running > 0 };
    unlock(pool_lock(pool));
    r
}

// ---- Timer queues ----
//
// CreateTimerQueue and friends on the pool's timers: each queue timer is
// a thread-pool timer whose callback calls the WAITORTIMERCALLBACK with
// its parameter and TRUE. A queue keeps its timers in a list (under its
// lock) so DeleteTimerQueueEx can delete them; NULL is the default
// queue. WT_EXECUTE* flags other than WT_EXECUTEONLYONCE only choose
// threads on Windows and are ignored.

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
    timer: *mut c_void,
}

static DEFAULT_QUEUE: AtomicUsize = AtomicUsize::new(0);

fn new_queue() -> *mut Queue {
    alloc(Queue { magic: QUEUE_MAGIC, lock: 0, timers: core::ptr::null_mut() })
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
                Err(other) => {
                    free(fresh);
                    other
                }
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
    // SAFETY: the queue timer this pool timer was created for (freed only
    // after its callbacks).
    unsafe { ((*t).callback)((*t).parameter, 1) };
}

/// Arms pool timer `o`: `due` ms from now, then every `period` ms.
fn arm(o: *mut c_void, due: u32, period: u32) {
    let due = -10_000 * i64::from(due);
    SetThreadpoolTimer(o, &due, period, 0);
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateTimerQueue() -> Handle {
    let q = new_queue();
    if q.is_null() {
        return fail_with(ERROR_NOT_ENOUGH_MEMORY, core::ptr::null_mut());
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
    let t = alloc(QueueTimer {
        magic: QUEUE_MAGIC,
        next: core::ptr::null_mut(),
        callback,
        parameter,
        timer: core::ptr::null_mut(),
    });
    if t.is_null() {
        return fail_with(ERROR_NOT_ENOUGH_MEMORY, FALSE);
    }
    let o = CreateThreadpoolTimer(queue_timer_callback, t.cast(), core::ptr::null());
    if o.is_null() {
        free(t);
        return FALSE;
    }
    // SAFETY: our new timer; the queue's list under its lock; the caller's
    // output.
    unsafe {
        (*t).timer = o;
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

/// Stops `t`, takes it off `q`'s list (if any) and cancels its queued
/// callbacks; whether a callback is still running.
fn stop_queue_timer(q: *mut Queue, t: *mut QueueTimer) -> bool {
    // SAFETY: a live queue and timer; the list under the queue's lock.
    let o = unsafe {
        if !q.is_null() {
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
        }
        (*t).magic = 0;
        (*t).timer.cast::<Object>()
    };
    SetThreadpoolTimer(o.cast(), core::ptr::null(), 0, 0);
    cancel_pending(o);
    running(o)
}

/// Waits for `t`'s callbacks, closes and frees it and sets `event` (if
/// any).
fn finish(t: *mut QueueTimer, event: Handle) {
    // SAFETY: a stopped queue timer, ours alone.
    let o = unsafe { (*t).timer };
    WaitForThreadpoolTimerCallbacks(o, TRUE);
    CloseThreadpoolTimer(o);
    free(t);
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
    }
    free(f);
    0
}

/// Finishes `timers` (linked by `next`) on a new thread, then sets `event`.
fn finish_later(timers: *mut QueueTimer, event: Handle) -> bool {
    let f = alloc(Finish { timers, event });
    if f.is_null() {
        return false;
    }
    let h = CreateThread(core::ptr::null_mut(), 0, finisher, f.cast(), 0, core::ptr::null_mut());
    if h.is_null() {
        free(f);
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
    let busy = stop_queue_timer(q, t);
    if event == INVALID_HANDLE_VALUE || !busy {
        finish(t, event);
        return TRUE;
    }
    // SAFETY: a stopped timer, now alone.
    unsafe { (*t).next = core::ptr::null_mut() };
    if !finish_later(t, event) {
        finish(t, event);
        return TRUE;
    }
    fail_with(ERROR_IO_PENDING, FALSE)
}

/// DeleteTimerQueueEx: deletes every timer of the queue (waiting for them
/// with INVALID_HANDLE_VALUE; otherwise on another thread, which then
/// sets `event`), then the queue.
#[unsafe(no_mangle)]
pub extern "system" fn DeleteTimerQueueEx(queue: Handle, event: Handle) -> Bool {
    if queue.is_null() {
        return fail_with(ERROR_INVALID_HANDLE, FALSE);
    }
    let Some(q) = queue_of(queue) else { return fail_with(ERROR_INVALID_HANDLE, FALSE) };
    // Take the whole list, then stop each timer.
    // SAFETY: a live queue; its list under its lock. It is ours alone
    // once its handle is invalid.
    let list = unsafe {
        AcquireSRWLockExclusive(&raw mut (*q).lock);
        let l = (*q).timers;
        (*q).timers = core::ptr::null_mut();
        (*q).magic = 0;
        ReleaseSRWLockExclusive(&raw mut (*q).lock);
        l
    };
    free(q);
    let mut t = list;
    while !t.is_null() {
        // SAFETY: timers of the taken list.
        let next = unsafe { (*t).next };
        stop_queue_timer(core::ptr::null_mut(), t);
        t = next;
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
        return fail_with(ERROR_NOT_ENOUGH_MEMORY, FALSE);
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
    let r = alloc(RegisteredWait {
        magic: REGISTERED_MAGIC,
        callback,
        context,
        wait: core::ptr::null_mut(),
        object,
        timeout: ms,
        once: flags & WT_EXECUTEONLYONCE != 0,
    });
    if r.is_null() {
        return fail_with(ERROR_NOT_ENOUGH_MEMORY, FALSE);
    }
    let wait = CreateThreadpoolWait(registered_wait_callback, r.cast(), core::ptr::null());
    if wait.is_null() {
        free(r);
        return FALSE;
    }
    // SAFETY: our new registration; the caller's output.
    unsafe {
        (*r).wait = wait;
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
    }
    free(r);
    if !completion.is_null() && completion != INVALID_HANDLE_VALUE {
        SetEvent(completion);
    }
    TRUE
}

/// UnregisterWait(WaitHandle): UnregisterWaitEx without an event.
#[unsafe(no_mangle)]
pub extern "system" fn UnregisterWait(wait: Handle) -> Bool {
    UnregisterWaitEx(wait, core::ptr::null_mut())
}

/// What QueueUserWorkItem queued: the function and its context.
struct WorkItem {
    function: extern "system" fn(*mut c_void) -> u32,
    context: *mut c_void,
}

extern "system" fn run_work_item(_instance: *mut c_void, item: *mut c_void) {
    let item = item.cast::<WorkItem>();
    // SAFETY: allocated and filled by QueueUserWorkItem, freed here once.
    let (f, context) = unsafe { ((*item).function, (*item).context) };
    free(item);
    f(context);
}

/// QueueUserWorkItem(function, context, flags): runs `function(context)`
/// on a thread-pool thread (the default pool's simple callbacks). The
/// flags only say what kind of work it is (WT_EXECUTELONGFUNCTION, ...);
/// the pool runs every kind the same way.
#[unsafe(no_mangle)]
pub extern "system" fn QueueUserWorkItem(
    function: extern "system" fn(*mut c_void) -> u32,
    context: *mut c_void,
    _flags: u32,
) -> Bool {
    let item = alloc(WorkItem { function, context });
    if item.is_null() {
        return fail_with(ERROR_NOT_ENOUGH_MEMORY, FALSE);
    }
    if TrySubmitThreadpoolCallback(run_work_item, item.cast(), core::ptr::null()) == FALSE {
        free(item);
        return FALSE;
    }
    TRUE
}
