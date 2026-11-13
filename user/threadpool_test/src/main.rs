//! The thread pool's whole family as Click-to-Run's DLLs use it (ROADMAP
//! OF: AppVIsvApi.dll imports CreateThreadpool, the pool limits and the
//! cleanup groups): private pools and callback environments, work, timer,
//! wait and I/O objects, cleanup groups, the callback-instance functions,
//! TrySubmitThreadpoolCallback. Expected values are those of Wine's
//! conformance tests, which pass on Windows (dlls/ntdll/tests/threadpool.c
//! through the kernel32 functions over the same Tp* calls, and
//! dlls/kernel32/tests/thread.c test_threadpool), and Microsoft's
//! documentation of each function:
//! - Pools: CreateThreadpool; a pool limited to one thread
//!   (SetThreadpoolThreadMaximum) runs one callback at a time and its
//!   CallbackMayRunLong is FALSE while that thread is busy;
//!   SetThreadpoolThreadMinimum is TRUE; Query/SetThreadpoolStackInformation
//!   (the image's stack sizes at first; not validated).
//! - Environments (TP_CALLBACK_ENVIRON, versions 1 and 3): the pool, the
//!   cleanup group and its cancel callback, the finalization callback;
//!   version 3's priorities 0-2; priority 10 and version 9999 are
//!   ERROR_INVALID_PARAMETER.
//! - Work: five posts run five times, one at a time on a one-thread pool;
//!   WaitForThreadpoolWorkCallbacks(TRUE) drops queued ones.
//! - Cleanup groups (test_tp_group_wait, test_tp_group_cancel):
//!   CloseThreadpoolCleanupGroupMembers waits for running callbacks; with
//!   cancellation it drops queued ones and calls each member's cancel
//!   callback once, on the calling thread, with the object's context and
//!   the call's, after the member's running callbacks have returned; not
//!   for an object its callback closed, nor for a finished simple
//!   callback.
//! - Instances (test_tp_instance, test_tp_disassociate):
//!   ReleaseSemaphoreWhenCallbackReturns, the finalization callback;
//!   DisassociateCurrentThreadFromCallback lets WaitForThreadpool*Callbacks
//!   return while the callback goes on, but not the cleanup group's wait.
//! - Timers (test_tp_timer): IsThreadpoolTimerSet is TRUE from
//!   SetThreadpoolTimer until a NULL due time, also after a one-shot fired;
//!   relative, absolute and zero due times; a 200 ms period fires three
//!   times in about 600 ms; SetThreadpoolTimerEx says whether the timer
//!   was set.
//! - Waits (test_tp_wait, test_tp_wait_early_closure): the callback's
//!   result is WAIT_OBJECT_0 (the wait took the semaphore's count) or
//!   WAIT_TIMEOUT; a zero timeout fires at once; a NULL handle stops the
//!   wait; two waits on one semaphore released once fire once; a handle
//!   closed after SetThreadpoolWait is still waited on;
//!   SetThreadpoolWaitEx is TRUE when it stopped a pending wait
//!   (documented).
//! - I/O (test_kernel32_tp_io): completions of a pipe's overlapped reads
//!   run the callback with the OVERLAPPED, NO_ERROR and the byte count,
//!   also for a read that completed at once; WaitForThreadpoolIoCallbacks
//!   waits for each StartThreadpoolIo, (TRUE) drops them;
//!   CancelThreadpoolIo; CancelIo's completion is ERROR_OPERATION_ABORTED.
//!
//! `info` lines record what Windows answers where no test has measured
//! it (a pool thread's description, the default pool's
//! CallbackMayRunLong).
//!
//! The same source builds `wow64_threadpool_test`, the 32-bit (x86)
//! program. Prints `threadpool_test: ok` (`wow64_threadpool_test: ok`),
//! or `<name>: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "threadpool_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_threadpool_test";

type Handle = usize;
type Ptr = *mut c_void;
type SimpleCallback = extern "system" fn(Ptr, Ptr);
type WorkCallback = extern "system" fn(Ptr, Ptr, Ptr);
type WaitCallback = extern "system" fn(Ptr, Ptr, Ptr, u32);
type IoCallback = extern "system" fn(Ptr, Ptr, Ptr, u32, usize, Ptr);
type CancelCallback = extern "system" fn(Ptr, Ptr);

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
    fn CloseHandle(h: Handle) -> i32;
    fn Sleep(ms: u32);
    fn GetTickCount() -> u32;
    fn GetCurrentThreadId() -> u32;
    fn GetCurrentThread() -> Handle;
    fn GetModuleHandleW(name: *const u16) -> usize;
    fn GetSystemTimeAsFileTime(t: *mut i64);
    fn CreateSemaphoreW(attrs: usize, initial: i32, max: i32, name: *const u16) -> Handle;
    fn ReleaseSemaphore(h: Handle, count: i32, previous: *mut i32) -> i32;
    fn CreateEventW(attrs: usize, manual: i32, initial: i32, name: *const u16) -> Handle;
    fn CreateWaitableTimerW(attrs: usize, manual: i32, name: *const u16) -> Handle;
    fn SetWaitableTimer(h: Handle, due: *const i64, period: i32, f: usize, a: usize, r: i32)
    -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn CreateNamedPipeW(
        name: *const u16,
        open_mode: u32,
        pipe_mode: u32,
        instances: u32,
        out_size: u32,
        in_size: u32,
        timeout: u32,
        security: usize,
    ) -> Handle;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: usize,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CancelIo(h: Handle) -> i32;
    fn GetThreadDescription(h: Handle, out: *mut *mut u16) -> i32;
    fn LocalFree(p: Ptr) -> Ptr;

    fn CreateThreadpool(reserved: Ptr) -> Ptr;
    fn CloseThreadpool(pool: Ptr);
    fn SetThreadpoolThreadMaximum(pool: Ptr, max: u32);
    fn SetThreadpoolThreadMinimum(pool: Ptr, min: u32) -> i32;
    fn SetThreadpoolStackInformation(pool: Ptr, info: *const [usize; 2]) -> i32;
    fn QueryThreadpoolStackInformation(pool: Ptr, info: *mut [usize; 2]) -> i32;
    fn CreateThreadpoolCleanupGroup() -> Ptr;
    fn CloseThreadpoolCleanupGroup(group: Ptr);
    fn CloseThreadpoolCleanupGroupMembers(group: Ptr, cancel: i32, context: Ptr);
    fn TrySubmitThreadpoolCallback(cb: SimpleCallback, context: Ptr, env: *const Env) -> i32;
    fn CreateThreadpoolWork(cb: WorkCallback, context: Ptr, env: *const Env) -> Ptr;
    fn SubmitThreadpoolWork(work: Ptr);
    fn WaitForThreadpoolWorkCallbacks(work: Ptr, cancel: i32);
    fn CloseThreadpoolWork(work: Ptr);
    fn CreateThreadpoolTimer(cb: WorkCallback, context: Ptr, env: *const Env) -> Ptr;
    fn SetThreadpoolTimer(timer: Ptr, due: *const i64, period: u32, window: u32);
    fn SetThreadpoolTimerEx(timer: Ptr, due: *const i64, period: u32, window: u32) -> i32;
    fn IsThreadpoolTimerSet(timer: Ptr) -> i32;
    fn WaitForThreadpoolTimerCallbacks(timer: Ptr, cancel: i32);
    fn CloseThreadpoolTimer(timer: Ptr);
    fn CreateThreadpoolWait(cb: WaitCallback, context: Ptr, env: *const Env) -> Ptr;
    fn SetThreadpoolWait(wait: Ptr, h: Handle, timeout: *const i64);
    fn SetThreadpoolWaitEx(wait: Ptr, h: Handle, timeout: *const i64, reserved: Ptr) -> i32;
    fn WaitForThreadpoolWaitCallbacks(wait: Ptr, cancel: i32);
    fn CloseThreadpoolWait(wait: Ptr);
    fn CreateThreadpoolIo(h: Handle, cb: IoCallback, context: Ptr, env: *const Env) -> Ptr;
    fn StartThreadpoolIo(io: Ptr);
    fn CancelThreadpoolIo(io: Ptr);
    fn WaitForThreadpoolIoCallbacks(io: Ptr, cancel: i32);
    fn CloseThreadpoolIo(io: Ptr);
    fn CallbackMayRunLong(instance: Ptr) -> i32;
    fn DisassociateCurrentThreadFromCallback(instance: Ptr);
    fn ReleaseSemaphoreWhenCallbackReturns(instance: Ptr, sem: Handle, count: u32);
    fn SetEventWhenCallbackReturns(instance: Ptr, event: Handle);
}

const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_IO_PENDING: u32 = 997;
const ERROR_OPERATION_ABORTED: u32 = 995;
const INVALID_HANDLE_VALUE: Handle = usize::MAX;

/// TP_CALLBACK_ENVIRON_V3 (version 1 is its first eight fields).
#[repr(C)]
struct Env {
    version: u32,
    pool: Ptr,
    group: Ptr,
    cancel: Option<CancelCallback>,
    race_dll: usize,
    actctx: usize,
    finalization: Option<SimpleCallback>,
    flags: u32,
    priority: u32,
    size: u32,
}

impl Env {
    fn v1(pool: Ptr) -> Env {
        Env {
            version: 1,
            pool,
            group: core::ptr::null_mut(),
            cancel: None,
            race_dll: 0,
            actctx: 0,
            finalization: None,
            flags: 0,
            priority: 0,
            size: 0,
        }
    }

    /// InitializeThreadpoolEnvironment's version 3 (normal priority).
    fn v3(pool: Ptr) -> Env {
        Env { version: 3, priority: 1, size: size_of::<Env>() as u32, ..Env::v1(pool) }
    }

    fn group(self, group: Ptr, cancel: Option<CancelCallback>) -> Env {
        Env { group, cancel, ..self }
    }
}

struct Line {
    buf: [u8; 300],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 300], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: our buffer; the standard output handle.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            )
        };
    }
}

/// An `info` line: what Windows answers, for the record (not checked).
fn info(what: &str, v: u64) {
    Line::new()
        .s(NAME.as_bytes())
        .s(b": info ")
        .s(what.as_bytes())
        .s(b" ")
        .hex(v)
        .s(b"\r\n")
        .print();
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u32, want: u32) -> Result<(), u32> {
        self.ok(got == want, got.into())
    }

    /// A Win32 call's FALSE (or NULL) and its last error.
    #[track_caller]
    fn fails(&mut self, ret: usize, error: u32) -> Result<(), u32> {
        // SAFETY: reads the thread's last error.
        let e = unsafe { GetLastError() };
        self.ok(ret == 0 && e == error, ((ret as u64) << 32) | u64::from(e))
    }

    /// `h` signaled within `ms` (or, with `want` WAIT_TIMEOUT, not).
    #[track_caller]
    fn wait(&mut self, h: Handle, ms: u32, want: u32) -> Result<(), u32> {
        // SAFETY: a handle of ours.
        let r = unsafe { WaitForSingleObject(h, ms) };
        self.eq(r, want)
    }
}

fn semaphore(max: i32) -> Handle {
    // SAFETY: plain call.
    unsafe { CreateSemaphoreW(0, 0, max, core::ptr::null()) }
}

fn release(h: Handle, n: i32) {
    // SAFETY: a semaphore of ours.
    unsafe { ReleaseSemaphore(h, n, core::ptr::null_mut()) };
}

fn p<T>(v: &T) -> Ptr {
    (v as *const T).cast_mut().cast()
}

/// What the callbacks saw (each test resets what it uses).
static COUNT: AtomicU32 = AtomicU32::new(0);
static COUNT2: AtomicU32 = AtomicU32::new(0);
static RUNNING: AtomicU32 = AtomicU32::new(0);
static MOST: AtomicU32 = AtomicU32::new(0);
static VALUE: AtomicU32 = AtomicU32::new(0);
static TID: AtomicU32 = AtomicU32::new(0);
static BAD: AtomicU32 = AtomicU32::new(0);
static SEEN: AtomicUsize = AtomicUsize::new(0);

extern "system" fn release_cb(_i: Ptr, sem: Ptr) {
    release(sem as Handle, 1);
}

/// Counts runs and the most running at once; 30 ms each.
extern "system" fn busy_cb(_i: Ptr, _context: Ptr, _work: Ptr) {
    let now = RUNNING.fetch_add(1, Ordering::SeqCst) + 1;
    MOST.fetch_max(now, Ordering::SeqCst);
    // SAFETY: plain call.
    unsafe { Sleep(30) };
    RUNNING.fetch_sub(1, Ordering::SeqCst);
    COUNT.fetch_add(1, Ordering::SeqCst);
}

fn pools(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: the calls under test, with our own objects.
    unsafe {
        // kernel32's test_threadpool: the default pool's work, then a pool.
        COUNT.store(0, Ordering::SeqCst);
        let work = CreateThreadpoolWork(busy_cb, core::ptr::null_mut(), core::ptr::null());
        c.ok(!work.is_null(), 0)?;
        SubmitThreadpoolWork(work);
        WaitForThreadpoolWorkCallbacks(work, 0);
        CloseThreadpoolWork(work);
        c.eq(COUNT.load(Ordering::SeqCst), 1)?;

        let pool = CreateThreadpool(core::ptr::null_mut());
        c.ok(!pool.is_null(), 0)?;
        c.eq(SetThreadpoolThreadMinimum(pool, 2) as u32, 1)?;
        SetThreadpoolThreadMaximum(pool, 4);
        // The stack sizes start as the image's (SizeOfStackReserve and
        // SizeOfStackCommit of the optional header) and are not validated.
        let image = GetModuleHandleW(core::ptr::null());
        let nt = image + (image as *const u8).add(0x3c).cast::<u32>().read_unaligned() as usize;
        let opt = (nt + 24) as *const u8;
        let (reserve, commit) = if cfg!(target_arch = "x86_64") {
            let r = opt.add(72).cast::<u64>().read_unaligned();
            (r as usize, opt.add(80).cast::<u64>().read_unaligned() as usize)
        } else {
            let r = opt.add(72).cast::<u32>().read_unaligned();
            (r as usize, opt.add(76).cast::<u32>().read_unaligned() as usize)
        };
        let mut stack = [0usize; 2];
        c.eq(QueryThreadpoolStackInformation(pool, &mut stack) as u32, 1)?;
        c.ok(stack == [reserve, commit], stack[0] as u64)?;
        c.eq(SetThreadpoolStackInformation(pool, &[1, 1]) as u32, 1)?;
        c.eq(QueryThreadpoolStackInformation(pool, &mut stack) as u32, 1)?;
        c.ok(stack == [1, 1], stack[0] as u64)?;
        CloseThreadpool(pool);
    }
    Ok(())
}

fn environments(c: &mut Checks) -> Result<(), u32> {
    let sem = semaphore(1);
    // SAFETY: the calls under test, with our own objects.
    unsafe {
        let pool = CreateThreadpool(core::ptr::null_mut());
        c.ok(!pool.is_null(), 0)?;
        // test_tp_simple: the default pool through a version 1
        // environment, the new pool, version 3's three priorities.
        let env = Env::v1(core::ptr::null_mut());
        c.eq(TrySubmitThreadpoolCallback(release_cb, sem as Ptr, &env) as u32, 1)?;
        c.wait(sem, 1000, WAIT_OBJECT_0)?;
        let env = Env::v1(pool);
        c.eq(TrySubmitThreadpoolCallback(release_cb, sem as Ptr, &env) as u32, 1)?;
        c.wait(sem, 1000, WAIT_OBJECT_0)?;
        for priority in 0..3 {
            let env = Env { priority, ..Env::v3(pool) };
            c.eq(TrySubmitThreadpoolCallback(release_cb, sem as Ptr, &env) as u32, 1)?;
            c.wait(sem, 1000, WAIT_OBJECT_0)?;
        }
        let env = Env { priority: 10, ..Env::v3(pool) };
        SetLastError(0);
        c.fails(
            TrySubmitThreadpoolCallback(release_cb, sem as Ptr, &env) as usize,
            ERROR_INVALID_PARAMETER,
        )?;
        let env = Env { version: 9999, ..Env::v1(pool) };
        SetLastError(0);
        c.fails(
            TrySubmitThreadpoolCallback(release_cb, sem as Ptr, &env) as usize,
            ERROR_INVALID_PARAMETER,
        )?;
        c.wait(sem, 50, WAIT_TIMEOUT)?;
        // Not measured: an object made with such an environment.
        SetLastError(0);
        let work = CreateThreadpoolWork(busy_cb, core::ptr::null_mut(), &env);
        info("CreateThreadpoolWork(version 9999) error", u64::from(GetLastError()));
        if !work.is_null() {
            CloseThreadpoolWork(work);
        }
        CloseThreadpool(pool);
        CloseHandle(sem);
    }
    Ok(())
}

extern "system" fn may_run_long_cb(instance: Ptr, sem: Ptr) {
    // SAFETY: the instance of this callback.
    VALUE.store(unsafe { CallbackMayRunLong(instance) } as u32, Ordering::SeqCst);
    // SAFETY: plain call.
    SEEN.store(unsafe { GetLastError() } as usize, Ordering::SeqCst);
    release(sem as Handle, 1);
}

extern "system" fn describe_cb(_instance: Ptr, sem: Ptr) {
    let mut text: *mut u16 = core::ptr::null_mut();
    // SAFETY: our thread; the result is LocalAlloc'ed.
    unsafe {
        let hr = GetThreadDescription(GetCurrentThread(), &mut text);
        SEEN.store(hr as u32 as usize, Ordering::SeqCst);
        if hr >= 0 && !text.is_null() {
            let mut n = 0;
            while *text.add(n) != 0 {
                n += 1;
            }
            VALUE.store(n as u32, Ordering::SeqCst);
            LocalFree(text.cast());
        }
    }
    release(sem as Handle, 1);
}

/// A pool of one thread runs one callback at a time.
fn one_thread(c: &mut Checks) -> Result<(), u32> {
    let sem = semaphore(1);
    // SAFETY: the calls under test, with our own objects.
    unsafe {
        let pool = CreateThreadpool(core::ptr::null_mut());
        SetThreadpoolThreadMaximum(pool, 1);
        // test_tp_work: five posts, five runs, never two at once.
        let env = Env::v3(pool);
        let work = CreateThreadpoolWork(busy_cb, core::ptr::null_mut(), &env);
        c.ok(!work.is_null(), 0)?;
        COUNT.store(0, Ordering::SeqCst);
        MOST.store(0, Ordering::SeqCst);
        for _ in 0..5 {
            SubmitThreadpoolWork(work);
        }
        WaitForThreadpoolWorkCallbacks(work, 0);
        c.eq(COUNT.load(Ordering::SeqCst), 5)?;
        c.eq(MOST.load(Ordering::SeqCst), 1)?;
        // Ten more, cancelled: fewer run.
        COUNT.store(0, Ordering::SeqCst);
        for _ in 0..10 {
            SubmitThreadpoolWork(work);
        }
        WaitForThreadpoolWorkCallbacks(work, 1);
        c.ok(COUNT.load(Ordering::SeqCst) < 10, COUNT.load(Ordering::SeqCst).into())?;
        CloseThreadpoolWork(work);
        // test_tp_group_cancel: no other thread for a long callback.
        VALUE.store(7, Ordering::SeqCst);
        c.eq(TrySubmitThreadpoolCallback(may_run_long_cb, sem as Ptr, &env) as u32, 1)?;
        c.wait(sem, 1000, WAIT_OBJECT_0)?;
        c.eq(VALUE.load(Ordering::SeqCst), 0)?;
        info("CallbackMayRunLong at the maximum: error", SEEN.load(Ordering::SeqCst) as u64);
        // Not measured: the default pool's answer, a pool thread's name.
        VALUE.store(7, Ordering::SeqCst);
        TrySubmitThreadpoolCallback(may_run_long_cb, sem as Ptr, core::ptr::null());
        c.wait(sem, 1000, WAIT_OBJECT_0)?;
        info("CallbackMayRunLong in the default pool", VALUE.load(Ordering::SeqCst).into());
        VALUE.store(0, Ordering::SeqCst);
        TrySubmitThreadpoolCallback(describe_cb, sem as Ptr, &env);
        c.wait(sem, 1000, WAIT_OBJECT_0)?;
        info("pool thread description length", VALUE.load(Ordering::SeqCst).into());
        info("pool thread GetThreadDescription", SEEN.load(Ordering::SeqCst) as u64);
        CloseThreadpool(pool);
        CloseHandle(sem);
    }
    Ok(())
}

/// test_tp_group_wait's callbacks: signal, give the main thread time to
/// reach CloseThreadpoolCleanupGroupMembers, close their own object.
extern "system" fn work_close_cb(_i: Ptr, sem: Ptr, work: Ptr) {
    release(sem as Handle, 1);
    // SAFETY: plain calls; the object of this callback.
    unsafe {
        Sleep(100);
        CloseThreadpoolWork(work);
    }
}

extern "system" fn timer_close_cb(_i: Ptr, sem: Ptr, timer: Ptr) {
    release(sem as Handle, 1);
    // SAFETY: plain calls; the object of this callback.
    unsafe {
        Sleep(100);
        CloseThreadpoolTimer(timer);
    }
}

extern "system" fn wait_close_cb(_i: Ptr, sem: Ptr, wait: Ptr, _result: u32) {
    release(sem as Handle, 1);
    // SAFETY: plain calls; the object of this callback.
    unsafe {
        Sleep(100);
        CloseThreadpoolWait(wait);
    }
}

extern "system" fn unexpected_simple(_i: Ptr, _c: Ptr) {
    BAD.fetch_add(1, Ordering::SeqCst);
}

extern "system" fn unexpected_work(_i: Ptr, _c: Ptr, _o: Ptr) {
    BAD.fetch_add(1, Ordering::SeqCst);
}

extern "system" fn unexpected_wait(_i: Ptr, _c: Ptr, _o: Ptr, _r: u32) {
    BAD.fetch_add(1, Ordering::SeqCst);
}

extern "system" fn unexpected_cancel(_o: Ptr, _c: Ptr) {
    BAD.fetch_add(1, Ordering::SeqCst);
}

/// Holds the one thread: no long callback possible, then waits for the
/// four cancel callbacks.
extern "system" fn hold_cb(instance: Ptr, sems: Ptr) {
    let sems = sems.cast::<[Handle; 2]>();
    // SAFETY: the instance of this callback; the caller's semaphores.
    unsafe {
        VALUE.store(CallbackMayRunLong(instance) as u32, Ordering::SeqCst);
        release((*sems)[1], 1);
        for _ in 0..4 {
            if WaitForSingleObject((*sems)[0], 1000) != WAIT_OBJECT_0 {
                BAD.fetch_add(0x100, Ordering::SeqCst);
            }
        }
        release((*sems)[1], 1);
    }
}

/// A cancel callback: the object's context is 0xdeadbeef.
extern "system" fn cancel_release_cb(object: Ptr, sems: Ptr) {
    // SAFETY: plain call.
    TID.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
    if object as usize != 0xdead_beef {
        BAD.fetch_add(0x10000, Ordering::SeqCst);
    }
    // SAFETY: the caller's semaphores.
    release(unsafe { (*sems.cast::<[Handle; 2]>())[0] }, 1);
}

/// A cancel callback: the object's context is the cleanup context.
extern "system" fn cancel_release2_cb(object: Ptr, sems: Ptr) {
    // SAFETY: plain call.
    TID.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
    if object != sems {
        BAD.fetch_add(0x10000, Ordering::SeqCst);
    }
    // SAFETY: the caller's semaphores.
    release(unsafe { (*sems.cast::<[Handle; 2]>())[0] }, 1);
}

extern "system" fn cancel_count_cb(_object: Ptr, counter: Ptr) {
    // SAFETY: plain call.
    TID.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
    // SAFETY: the caller's counter.
    unsafe { (*counter.cast::<AtomicU32>()).fetch_add(1, Ordering::SeqCst) };
}

/// Signals, then (still running when the members are closed) its wait
/// must time out: the cancel callbacks come after it returns.
extern "system" fn work_cancel_cb(_i: Ptr, sems: Ptr, _work: Ptr) {
    // SAFETY: the caller's semaphores.
    unsafe {
        let sems = sems.cast::<[Handle; 2]>();
        release((*sems)[1], 1);
        VALUE.store(WaitForSingleObject((*sems)[0], 200), Ordering::SeqCst);
    }
}

extern "system" fn simple_close_cb(_i: Ptr, sem: Ptr) {
    release(sem as Handle, 1);
    // SAFETY: plain call.
    unsafe { Sleep(100) };
}

fn groups(c: &mut Checks) -> Result<(), u32> {
    let sems = [semaphore(4), semaphore(1)];
    let sem = sems[1];
    // SAFETY: the calls under test, with our own objects.
    unsafe {
        let me = GetCurrentThreadId();
        let pool = CreateThreadpool(core::ptr::null_mut());
        let group = CreateThreadpoolCleanupGroup();
        c.ok(!group.is_null(), 0)?;
        // test_tp_group_wait: objects closed by their own callbacks while
        // the members are closed.
        let env = Env::v3(pool).group(group, None);
        let work = CreateThreadpoolWork(work_close_cb, sem as Ptr, &env);
        c.ok(!work.is_null(), 0)?;
        SubmitThreadpoolWork(work);
        c.wait(sem, 1000, WAIT_OBJECT_0)?;
        CloseThreadpoolCleanupGroupMembers(group, 0, core::ptr::null_mut());
        let timer = CreateThreadpoolTimer(timer_close_cb, sem as Ptr, &env);
        c.ok(!timer.is_null(), 0)?;
        SetThreadpoolTimer(timer, &0, 0, 0);
        c.wait(sem, 1000, WAIT_OBJECT_0)?;
        CloseThreadpoolCleanupGroupMembers(group, 0, core::ptr::null_mut());
        let wait = CreateThreadpoolWait(wait_close_cb, sem as Ptr, &env);
        c.ok(!wait.is_null(), 0)?;
        SetThreadpoolWait(wait, INVALID_HANDLE_VALUE, &0);
        c.wait(sem, 1000, WAIT_OBJECT_0)?;
        CloseThreadpoolCleanupGroupMembers(group, 0, core::ptr::null_mut());
        // Without cancellation the members' queued callbacks run first.
        COUNT.store(0, Ordering::SeqCst);
        let work = CreateThreadpoolWork(busy_cb, core::ptr::null_mut(), &env);
        for _ in 0..3 {
            SubmitThreadpoolWork(work);
        }
        CloseThreadpoolCleanupGroupMembers(group, 0, core::ptr::null_mut());
        c.eq(COUNT.load(Ordering::SeqCst), 3)?;

        CloseThreadpool(pool);
        // test_tp_group_cancel on a pool of one thread, held by a
        // callback outside the group.
        let pool = CreateThreadpool(core::ptr::null_mut());
        SetThreadpoolThreadMaximum(pool, 1);
        BAD.store(0, Ordering::SeqCst);
        let env = Env::v3(pool);
        c.eq(TrySubmitThreadpoolCallback(hold_cb, p(&sems), &env) as u32, 1)?;
        c.wait(sem, 1000, WAIT_OBJECT_0)?;
        c.eq(VALUE.load(Ordering::SeqCst), 0)?;
        let env = Env::v3(pool).group(group, Some(cancel_release_cb));
        let context = 0xdead_beef as Ptr;
        c.eq(TrySubmitThreadpoolCallback(unexpected_simple, context, &env) as u32, 1)?;
        c.ok(!CreateThreadpoolWork(unexpected_work, context, &env).is_null(), 0)?;
        c.ok(!CreateThreadpoolTimer(unexpected_work, context, &env).is_null(), 0)?;
        c.ok(!CreateThreadpoolWait(unexpected_wait, context, &env).is_null(), 0)?;
        TID.store(0, Ordering::SeqCst);
        CloseThreadpoolCleanupGroupMembers(group, 1, p(&sems));
        c.wait(sem, 1000, WAIT_OBJECT_0)?;
        c.eq(TID.load(Ordering::SeqCst), me)?;
        c.eq(BAD.load(Ordering::SeqCst), 0)?;

        // The cancel callback comes after the running callback returned.
        let env = Env::v3(pool).group(group, Some(cancel_release2_cb));
        let work = CreateThreadpoolWork(work_cancel_cb, p(&sems), &env);
        c.ok(!work.is_null(), 0)?;
        VALUE.store(7, Ordering::SeqCst);
        SubmitThreadpoolWork(work);
        SubmitThreadpoolWork(work);
        c.wait(sem, 1000, WAIT_OBJECT_0)?;
        TID.store(0, Ordering::SeqCst);
        CloseThreadpoolCleanupGroupMembers(group, 1, p(&sems));
        c.wait(sems[0], 1000, WAIT_OBJECT_0)?;
        c.eq(TID.load(Ordering::SeqCst), me)?;
        c.eq(VALUE.load(Ordering::SeqCst), WAIT_TIMEOUT)?;
        c.eq(BAD.load(Ordering::SeqCst), 0)?;

        // No cancel callback for an object its callback closed, nor for a
        // simple callback that ran.
        let env = Env::v3(pool).group(group, Some(unexpected_cancel));
        let work = CreateThreadpoolWork(work_close_cb, sem as Ptr, &env);
        SubmitThreadpoolWork(work);
        c.wait(sem, 1000, WAIT_OBJECT_0)?;
        CloseThreadpoolCleanupGroupMembers(group, 1, core::ptr::null_mut());
        c.eq(TrySubmitThreadpoolCallback(simple_close_cb, sem as Ptr, &env) as u32, 1)?;
        c.wait(sem, 1000, WAIT_OBJECT_0)?;
        CloseThreadpoolCleanupGroupMembers(group, 1, p(&sems));
        c.eq(BAD.load(Ordering::SeqCst), 0)?;

        // One cancel callback for an object with many queued callbacks.
        let counter = AtomicU32::new(0);
        let env = Env::v3(pool).group(group, Some(cancel_count_cb));
        COUNT.store(0, Ordering::SeqCst);
        let work = CreateThreadpoolWork(busy_cb, core::ptr::null_mut(), &env);
        for _ in 0..10 {
            SubmitThreadpoolWork(work);
        }
        TID.store(0, Ordering::SeqCst);
        CloseThreadpoolCleanupGroupMembers(group, 1, p(&counter));
        c.ok(COUNT.load(Ordering::SeqCst) < 10, COUNT.load(Ordering::SeqCst).into())?;
        c.eq(counter.load(Ordering::SeqCst), 1)?;
        c.eq(TID.load(Ordering::SeqCst), me)?;

        CloseThreadpoolCleanupGroup(group);
        CloseThreadpool(pool);
        CloseHandle(sems[0]);
        CloseHandle(sems[1]);
    }
    Ok(())
}

extern "system" fn semaphore_on_return_cb(instance: Ptr, sems: Ptr) {
    // SAFETY: the instance of this callback; the caller's semaphores.
    unsafe { ReleaseSemaphoreWhenCallbackReturns(instance, (*sems.cast::<[Handle; 2]>())[0], 1) };
}

extern "system" fn finalization_cb(_instance: Ptr, sems: Ptr) {
    // SAFETY: the caller's semaphores.
    release(unsafe { (*sems.cast::<[Handle; 2]>())[1] }, 1);
}

extern "system" fn event_on_return_cb(instance: Ptr, event: Ptr, _work: Ptr) {
    // SAFETY: the instance of this callback.
    unsafe { SetEventWhenCallbackReturns(instance, event as Handle) };
}

/// Disassociates, then waits for the main thread's release (or, with
/// `TIMEOUT` set, expects none within 100 ms).
extern "system" fn disassociate_cb(instance: Ptr, sems: Ptr, _work: Ptr) {
    // SAFETY: the instance of this callback; the caller's semaphores.
    unsafe {
        let sems = sems.cast::<[Handle; 2]>();
        DisassociateCurrentThreadFromCallback(instance);
        let ms = if COUNT2.load(Ordering::SeqCst) != 0 { 100 } else { 1000 };
        VALUE.store(WaitForSingleObject((*sems)[0], ms), Ordering::SeqCst);
        release((*sems)[1], 1);
    }
}

fn instances(c: &mut Checks) -> Result<(), u32> {
    let sems = [semaphore(1), semaphore(1)];
    // SAFETY: the calls under test, with our own objects.
    unsafe {
        let pool = CreateThreadpool(core::ptr::null_mut());
        // test_tp_instance.
        let env = Env::v3(pool);
        c.eq(TrySubmitThreadpoolCallback(semaphore_on_return_cb, p(&sems), &env) as u32, 1)?;
        c.wait(sems[0], 1000, WAIT_OBJECT_0)?;
        let env = Env { finalization: Some(finalization_cb), ..Env::v3(pool) };
        c.eq(TrySubmitThreadpoolCallback(semaphore_on_return_cb, p(&sems), &env) as u32, 1)?;
        c.wait(sems[0], 1000, WAIT_OBJECT_0)?;
        c.wait(sems[1], 1000, WAIT_OBJECT_0)?;
        let event = CreateEventW(0, 1, 0, core::ptr::null());
        let work = CreateThreadpoolWork(event_on_return_cb, event as Ptr, &Env::v3(pool));
        SubmitThreadpoolWork(work);
        WaitForThreadpoolWorkCallbacks(work, 0);
        c.wait(event, 0, WAIT_OBJECT_0)?;
        CloseThreadpoolWork(work);
        CloseHandle(event);

        // test_tp_disassociate: WaitForThreadpoolWorkCallbacks returns
        // while the disassociated callback still waits.
        COUNT2.store(0, Ordering::SeqCst);
        let work = CreateThreadpoolWork(disassociate_cb, p(&sems), &Env::v3(pool));
        SubmitThreadpoolWork(work);
        WaitForThreadpoolWorkCallbacks(work, 0);
        c.wait(sems[1], 100, WAIT_TIMEOUT)?;
        release(sems[0], 1);
        c.wait(sems[1], 1000, WAIT_OBJECT_0)?;
        c.eq(VALUE.load(Ordering::SeqCst), WAIT_OBJECT_0)?;
        CloseThreadpoolWork(work);
        // In a cleanup group, closing the members waits for it.
        let group = CreateThreadpoolCleanupGroup();
        COUNT2.store(1, Ordering::SeqCst);
        let env = Env::v3(pool).group(group, None);
        let work = CreateThreadpoolWork(disassociate_cb, p(&sems), &env);
        SubmitThreadpoolWork(work);
        CloseThreadpoolCleanupGroupMembers(group, 0, core::ptr::null_mut());
        release(sems[0], 1);
        c.wait(sems[1], 1000, WAIT_OBJECT_0)?;
        c.eq(VALUE.load(Ordering::SeqCst), WAIT_TIMEOUT)?;
        c.wait(sems[0], 1000, WAIT_OBJECT_0)?;
        CloseThreadpoolCleanupGroup(group);
        CloseThreadpool(pool);
        CloseHandle(sems[0]);
        CloseHandle(sems[1]);
    }
    Ok(())
}

extern "system" fn timer_cb(_i: Ptr, sem: Ptr, _timer: Ptr) {
    release(sem as Handle, 1);
}

fn timers(c: &mut Checks) -> Result<(), u32> {
    let sem = semaphore(3);
    // SAFETY: the calls under test, with our own objects.
    unsafe {
        let pool = CreateThreadpool(core::ptr::null_mut());
        let env = Env::v3(pool);
        let timer = CreateThreadpoolTimer(timer_cb, sem as Ptr, &env);
        c.ok(!timer.is_null(), 0)?;
        c.eq(IsThreadpoolTimerSet(timer) as u32, 0)?;
        // Relative: not within 100 ms, then within 200 more; still set.
        SetThreadpoolTimer(timer, &(-200 * 10_000), 0, 0);
        c.eq(IsThreadpoolTimerSet(timer) as u32, 1)?;
        WaitForThreadpoolTimerCallbacks(timer, 0);
        c.wait(sem, 100, WAIT_TIMEOUT)?;
        c.wait(sem, 200, WAIT_OBJECT_0)?;
        c.eq(IsThreadpoolTimerSet(timer) as u32, 1)?;
        // Absolute.
        let mut now = 0i64;
        GetSystemTimeAsFileTime(&mut now);
        SetThreadpoolTimer(timer, &(now + 200 * 10_000), 0, 0);
        c.eq(IsThreadpoolTimerSet(timer) as u32, 1)?;
        c.wait(sem, 100, WAIT_TIMEOUT)?;
        c.wait(sem, 200, WAIT_OBJECT_0)?;
        c.eq(IsThreadpoolTimerSet(timer) as u32, 1)?;
        // Zero: at once.
        SetThreadpoolTimer(timer, &0, 0, 0);
        c.eq(IsThreadpoolTimerSet(timer) as u32, 1)?;
        c.wait(sem, 50, WAIT_OBJECT_0)?;
        SetThreadpoolTimer(timer, core::ptr::null(), 0, 0);
        c.eq(IsThreadpoolTimerSet(timer) as u32, 0)?;
        WaitForThreadpoolTimerCallbacks(timer, 1);
        // Periodic: three times in about 600 ms.
        SetThreadpoolTimer(timer, &(-200 * 10_000), 200, 0);
        let start = GetTickCount();
        for _ in 0..3 {
            c.wait(sem, 1000, WAIT_OBJECT_0)?;
        }
        let ticks = GetTickCount().wrapping_sub(start);
        c.ok(ticks >= 500, ticks.into())?;
        info("three periods of 200 ms took (ms)", ticks.into());
        SetThreadpoolTimer(timer, core::ptr::null(), 0, 0);
        c.eq(IsThreadpoolTimerSet(timer) as u32, 0)?;
        WaitForThreadpoolTimerCallbacks(timer, 1);
        // SetThreadpoolTimerEx: whether it was set.
        c.eq(SetThreadpoolTimerEx(timer, &(-200 * 10_000), 200, 0) as u32, 0)?;
        c.eq(IsThreadpoolTimerSet(timer) as u32, 1)?;
        c.eq(SetThreadpoolTimerEx(timer, core::ptr::null(), 0, 0) as u32, 1)?;
        c.eq(IsThreadpoolTimerSet(timer) as u32, 0)?;
        WaitForThreadpoolTimerCallbacks(timer, 1);
        CloseThreadpoolTimer(timer);
        CloseThreadpool(pool);
        // Drain what a cancelled period may have left.
        while WaitForSingleObject(sem, 0) == WAIT_OBJECT_0 {}
        CloseHandle(sem);
    }
    Ok(())
}

/// test_tp_wait's callback: 1 per WAIT_OBJECT_0, 0x10000 per timeout.
extern "system" fn wait_cb(_i: Ptr, sem: Ptr, _wait: Ptr, result: u32) {
    match result {
        WAIT_OBJECT_0 => COUNT.fetch_add(1, Ordering::SeqCst),
        WAIT_TIMEOUT => COUNT.fetch_add(0x10000, Ordering::SeqCst),
        _ => BAD.fetch_add(1, Ordering::SeqCst),
    };
    release(sem as Handle, 1);
}

fn waits(c: &mut Checks) -> Result<(), u32> {
    let done = semaphore(2);
    let mut sem = semaphore(1);
    // SAFETY: the calls under test, with our own objects.
    unsafe {
        let pool = CreateThreadpool(core::ptr::null_mut());
        let env = Env::v3(pool);
        let wait1 = CreateThreadpoolWait(wait_cb, done as Ptr, &env);
        let wait2 = CreateThreadpoolWait(wait_cb, done as Ptr, &env);
        c.ok(!wait1.is_null() && !wait2.is_null(), 0)?;
        BAD.store(0, Ordering::SeqCst);
        // Signaled: the wait takes the count.
        COUNT.store(0, Ordering::SeqCst);
        SetThreadpoolWait(wait1, sem, core::ptr::null());
        release(sem, 1);
        c.wait(done, 1000, WAIT_OBJECT_0)?;
        c.eq(COUNT.load(Ordering::SeqCst), 1)?;
        c.wait(sem, 0, WAIT_TIMEOUT)?;
        // A relative timeout.
        COUNT.store(0, Ordering::SeqCst);
        SetThreadpoolWait(wait1, sem, &(-200 * 10_000));
        c.wait(done, 100, WAIT_TIMEOUT)?;
        c.wait(done, 300, WAIT_OBJECT_0)?;
        c.eq(COUNT.load(Ordering::SeqCst), 0x10000)?;
        // An absolute timeout, signaled first.
        COUNT.store(0, Ordering::SeqCst);
        let mut now = 0i64;
        GetSystemTimeAsFileTime(&mut now);
        SetThreadpoolWait(wait1, sem, &(now + 200 * 10_000));
        c.wait(done, 100, WAIT_TIMEOUT)?;
        release(sem, 1);
        c.wait(done, 100, WAIT_OBJECT_0)?;
        c.eq(COUNT.load(Ordering::SeqCst), 1)?;
        c.wait(sem, 0, WAIT_TIMEOUT)?;
        // A zero timeout fires at once.
        COUNT.store(0, Ordering::SeqCst);
        SetThreadpoolWait(wait1, sem, &0);
        c.wait(done, 100, WAIT_OBJECT_0)?;
        c.eq(COUNT.load(Ordering::SeqCst), 0x10000)?;
        // A NULL handle stops the wait (the timeout argument unused).
        SetThreadpoolWait(wait1, sem, &(-250 * 10_000));
        c.wait(done, 100, WAIT_TIMEOUT)?;
        SetThreadpoolWait(wait1, 0, 0xdead_beef as *const i64);
        Sleep(50);
        release(sem, 1);
        c.wait(done, 100, WAIT_TIMEOUT)?;
        c.wait(sem, 0, WAIT_OBJECT_0)?;
        // INVALID_HANDLE_VALUE (the process) with a zero timeout.
        SetThreadpoolWait(wait1, INVALID_HANDLE_VALUE, &0);
        c.wait(done, 100, WAIT_OBJECT_0)?;
        // Two waits on one semaphore released once: one callback.
        CloseHandle(sem);
        sem = semaphore(2);
        COUNT.store(0, Ordering::SeqCst);
        SetThreadpoolWait(wait1, sem, core::ptr::null());
        SetThreadpoolWait(wait2, sem, core::ptr::null());
        Sleep(50);
        release(sem, 1);
        c.wait(done, 100, WAIT_OBJECT_0)?;
        c.wait(done, 100, WAIT_TIMEOUT)?;
        c.wait(sem, 0, WAIT_TIMEOUT)?;
        // ... and released twice: two.
        release(sem, 1);
        c.wait(done, 100, WAIT_OBJECT_0)?;
        c.eq(COUNT.load(Ordering::SeqCst), 2)?;
        // test_tp_wait_early_closure: a handle closed after
        // SetThreadpoolWait is still waited on.
        let timer = CreateWaitableTimerW(0, 1, core::ptr::null());
        SetWaitableTimer(timer, &(-100 * 10_000), 0, 0, 0, 0);
        COUNT.store(0, Ordering::SeqCst);
        SetThreadpoolWait(wait1, timer, core::ptr::null());
        CloseHandle(timer);
        c.wait(done, 300, WAIT_OBJECT_0)?;
        c.eq(COUNT.load(Ordering::SeqCst), 1)?;
        // SetThreadpoolWaitEx: TRUE when it stopped a pending wait.
        SetThreadpoolWait(wait1, sem, core::ptr::null());
        c.eq(SetThreadpoolWaitEx(wait1, 0, core::ptr::null(), core::ptr::null_mut()) as u32, 1)?;
        c.eq(SetThreadpoolWaitEx(wait1, 0, core::ptr::null(), core::ptr::null_mut()) as u32, 0)?;
        WaitForThreadpoolWaitCallbacks(wait1, 1);
        WaitForThreadpoolWaitCallbacks(wait2, 1);
        c.eq(BAD.load(Ordering::SeqCst), 0)?;
        CloseThreadpoolWait(wait1);
        CloseThreadpoolWait(wait2);
        CloseThreadpool(pool);
        CloseHandle(sem);
        CloseHandle(done);
    }
    Ok(())
}

/// What the I/O callback saw.
struct IoSeen {
    count: AtomicU32,
    overlapped: AtomicUsize,
    result: AtomicU32,
    bytes: AtomicUsize,
    io: AtomicUsize,
}

extern "system" fn io_cb(_i: Ptr, seen: Ptr, ov: Ptr, result: u32, bytes: usize, io: Ptr) {
    // SAFETY: the caller's record.
    let seen = unsafe { &*seen.cast::<IoSeen>() };
    seen.overlapped.store(ov as usize, Ordering::SeqCst);
    seen.result.store(result, Ordering::SeqCst);
    seen.bytes.store(bytes, Ordering::SeqCst);
    seen.io.store(io as usize, Ordering::SeqCst);
    seen.count.fetch_add(1, Ordering::SeqCst);
}

/// OVERLAPPED.
#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    event: Handle,
}

fn wide<const N: usize>(s: &[u8]) -> [u16; N] {
    let mut w = [0u16; N];
    for (i, &b) in s.iter().enumerate() {
        w[i] = u16::from(b);
    }
    w
}

fn io(c: &mut Checks) -> Result<(), u32> {
    const PIPE_ACCESS_DUPLEX: u32 = 3;
    const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    let name: [u16; 40] = wide(b"\\\\.\\pipe\\gala_threadpool_test");
    let seen = IoSeen {
        count: AtomicU32::new(0),
        overlapped: AtomicUsize::new(0),
        result: AtomicU32::new(7),
        bytes: AtomicUsize::new(7),
        io: AtomicUsize::new(0),
    };
    // SAFETY: the calls under test, with our own objects and buffers.
    unsafe {
        let pool = CreateThreadpool(core::ptr::null_mut());
        let server = CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            0,
            1,
            1024,
            1024,
            0,
            0,
        );
        c.ok(server != INVALID_HANDLE_VALUE, GetLastError().into())?;
        let client = CreateFileW(name.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, 0, 3, 0, 0);
        c.ok(client != INVALID_HANDLE_VALUE, GetLastError().into())?;
        let io = CreateThreadpoolIo(server, io_cb, p(&seen), &Env::v3(pool));
        c.ok(!io.is_null(), GetLastError().into())?;
        WaitForThreadpoolIoCallbacks(io, 0);
        let event = CreateEventW(0, 1, 0, core::ptr::null());
        let mut ov = Overlapped { internal: 0, internal_high: 0, offset: 0, offset_high: 0, event };
        let mut ov2 =
            Overlapped { internal: 0, internal_high: 0, offset: 0, offset_high: 0, event: 0 };
        let (mut a, mut b) = ([0u8; 1], [0u8; 1]);
        let mut n = 0u32;
        // One read, completed by the client's write.
        StartThreadpoolIo(io);
        SetLastError(0);
        c.fails(
            ReadFile(server, a.as_mut_ptr(), 1, core::ptr::null_mut(), (&raw mut ov) as usize)
                as usize,
            ERROR_IO_PENDING,
        )?;
        c.eq(WriteFile(client, b"x".as_ptr(), 1, &mut n, 0) as u32, 1)?;
        WaitForThreadpoolIoCallbacks(io, 0);
        c.eq(seen.count.load(Ordering::SeqCst), 1)?;
        c.ok(seen.overlapped.load(Ordering::SeqCst) == (&raw const ov) as usize, 0)?;
        c.eq(seen.result.load(Ordering::SeqCst), 0)?;
        c.eq(seen.bytes.load(Ordering::SeqCst) as u32, 1)?;
        c.ok(seen.io.load(Ordering::SeqCst) == io as usize, 0)?;
        // Two at once.
        seen.count.store(0, Ordering::SeqCst);
        StartThreadpoolIo(io);
        StartThreadpoolIo(io);
        c.fails(
            ReadFile(server, a.as_mut_ptr(), 1, core::ptr::null_mut(), (&raw mut ov) as usize)
                as usize,
            ERROR_IO_PENDING,
        )?;
        c.fails(
            ReadFile(server, b.as_mut_ptr(), 1, core::ptr::null_mut(), (&raw mut ov2) as usize)
                as usize,
            ERROR_IO_PENDING,
        )?;
        c.eq(WriteFile(client, b"y".as_ptr(), 1, &mut n, 0) as u32, 1)?;
        c.eq(WriteFile(client, b"z".as_ptr(), 1, &mut n, 0) as u32, 1)?;
        WaitForThreadpoolIoCallbacks(io, 0);
        c.eq(seen.count.load(Ordering::SeqCst), 2)?;
        c.eq(seen.result.load(Ordering::SeqCst), 0)?;
        // Cancelled by the wait: no callback.
        seen.count.store(0, Ordering::SeqCst);
        StartThreadpoolIo(io);
        WaitForThreadpoolIoCallbacks(io, 1);
        c.eq(seen.count.load(Ordering::SeqCst), 0)?;
        // A read that completes at once still runs the callback.
        StartThreadpoolIo(io);
        c.eq(WriteFile(client, b"w".as_ptr(), 1, &mut n, 0) as u32, 1)?;
        c.eq(
            ReadFile(server, a.as_mut_ptr(), 1, core::ptr::null_mut(), (&raw mut ov) as usize)
                as u32,
            1,
        )?;
        WaitForThreadpoolIoCallbacks(io, 0);
        c.eq(seen.count.load(Ordering::SeqCst), 1)?;
        c.ok(seen.overlapped.load(Ordering::SeqCst) == (&raw const ov) as usize, 0)?;
        c.eq(seen.bytes.load(Ordering::SeqCst) as u32, 1)?;
        // CancelThreadpoolIo: no operation after all.
        seen.count.store(0, Ordering::SeqCst);
        StartThreadpoolIo(io);
        CancelThreadpoolIo(io);
        WaitForThreadpoolIoCallbacks(io, 0);
        c.eq(seen.count.load(Ordering::SeqCst), 0)?;
        // CancelIo: ERROR_OPERATION_ABORTED, no bytes.
        StartThreadpoolIo(io);
        c.fails(
            ReadFile(server, a.as_mut_ptr(), 1, core::ptr::null_mut(), (&raw mut ov) as usize)
                as usize,
            ERROR_IO_PENDING,
        )?;
        c.eq(CancelIo(server) as u32, 1)?;
        WaitForThreadpoolIoCallbacks(io, 0);
        c.eq(seen.count.load(Ordering::SeqCst), 1)?;
        c.ok(seen.overlapped.load(Ordering::SeqCst) == (&raw const ov) as usize, 0)?;
        c.eq(seen.result.load(Ordering::SeqCst), ERROR_OPERATION_ABORTED)?;
        c.eq(seen.bytes.load(Ordering::SeqCst) as u32, 0)?;
        CloseHandle(event);
        CloseHandle(client);
        CloseHandle(server);
        CloseThreadpoolIo(io);
        CloseThreadpool(pool);
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    pools(&mut c)?;
    environments(&mut c)?;
    one_thread(&mut c)?;
    groups(&mut c)?;
    instances(&mut c)?;
    timers(&mut c)?;
    waits(&mut c)?;
    io(&mut c)?;
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
