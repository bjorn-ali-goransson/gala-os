//! kernel32's thread pool and the other functions msvcp140.dll imports,
//! with the results real Windows gives (documented contracts):
//!
//! - Work: CreateThreadpoolWork, SubmitThreadpoolWork three times,
//!   WaitForThreadpoolWorkCallbacks waits for all three (run on pool
//!   threads, with the context and the work object), CloseThreadpoolWork;
//!   TrySubmitThreadpoolCallback.
//! - Timers: a one-shot timer with a relative due time; a periodic one,
//!   cancelled by a NULL due time (no more callbacks after
//!   WaitForThreadpoolTimerCallbacks with cancellation).
//! - Timer queues: CreateTimerQueue, CreateTimerQueueTimer one-shot
//!   (WT_EXECUTEONLYONCE; the callback gets its parameter and TRUE) and
//!   periodic, ChangeTimerQueueTimer, DeleteTimerQueueTimer waiting for
//!   callbacks (INVALID_HANDLE_VALUE: no more runs afterwards), a timer on
//!   the default queue (NULL), DeleteTimerQueueEx with a completion event
//!   (set once its timers are gone).
//! - Waits: a wait on an event runs with WAIT_OBJECT_0 when it is set and
//!   with WAIT_TIMEOUT when its timeout passes first.
//! - FreeLibraryWhenCallbackReturns unloads a DLL the callback loaded.
//! - CreateEventExW (manual reset, initially set), CopyFileW (contents,
//!   ERROR_FILE_EXISTS when asked to fail), CopyFileExW,
//!   CreateSymbolicLinkW without the privilege (ERROR_PRIVILEGE_NOT_HELD).
//!
//! Prints `kernel32_pool_test: ok` or exits with the number of the failing
//! check.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

type Bool = i32;
type Handle = *mut c_void;
type WorkCallback = extern "system" fn(*mut c_void, *mut c_void, *mut c_void);
type WaitCallback = extern "system" fn(*mut c_void, *mut c_void, *mut c_void, u32);
type SimpleCallback = extern "system" fn(*mut c_void, *mut c_void);
type QueueCallback = extern "system" fn(*mut c_void, u8);

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetLastError() -> u32;
    fn CreateTimerQueue() -> Handle;
    fn CreateTimerQueueTimer(
        timer: *mut Handle,
        queue: Handle,
        callback: QueueCallback,
        parameter: *mut c_void,
        due: u32,
        period: u32,
        flags: u32,
    ) -> Bool;
    fn ChangeTimerQueueTimer(queue: Handle, timer: Handle, due: u32, period: u32) -> Bool;
    fn DeleteTimerQueueTimer(queue: Handle, timer: Handle, event: Handle) -> Bool;
    fn DeleteTimerQueueEx(queue: Handle, event: Handle) -> Bool;
    fn SetLastError(e: u32);
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn Sleep(ms: u32);
    fn GetCurrentThreadId() -> u32;
    fn CreateEventW(attrs: *mut c_void, manual: Bool, initial: Bool, name: *const u16) -> Handle;
    fn CreateEventExW(attrs: *mut c_void, name: *const u16, flags: u32, access: u32) -> Handle;
    fn SetEvent(h: Handle) -> Bool;
    fn ResetEvent(h: Handle) -> Bool;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn LoadLibraryA(name: *const u8) -> Handle;
    fn GetModuleHandleA(name: *const u8) -> Handle;
    fn CreateThreadpoolWork(f: WorkCallback, context: *mut c_void, env: *mut c_void)
    -> *mut c_void;
    fn SubmitThreadpoolWork(work: *mut c_void);
    fn WaitForThreadpoolWorkCallbacks(work: *mut c_void, cancel: Bool);
    fn CloseThreadpoolWork(work: *mut c_void);
    fn TrySubmitThreadpoolCallback(
        f: SimpleCallback,
        context: *mut c_void,
        env: *mut c_void,
    ) -> Bool;
    fn CreateThreadpoolTimer(
        f: WorkCallback,
        context: *mut c_void,
        env: *mut c_void,
    ) -> *mut c_void;
    fn SetThreadpoolTimer(timer: *mut c_void, due: *const i64, period: u32, window: u32);
    fn WaitForThreadpoolTimerCallbacks(timer: *mut c_void, cancel: Bool);
    fn CloseThreadpoolTimer(timer: *mut c_void);
    fn CreateThreadpoolWait(f: WaitCallback, context: *mut c_void, env: *mut c_void)
    -> *mut c_void;
    fn SetThreadpoolWait(wait: *mut c_void, h: Handle, timeout: *const i64);
    fn CloseThreadpoolWait(wait: *mut c_void);
    fn FreeLibraryWhenCallbackReturns(instance: *mut c_void, module: Handle);
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CopyFileW(from: *const u16, to: *const u16, fail_if_exists: Bool) -> Bool;
    fn CopyFileExW(
        from: *const u16,
        to: *const u16,
        progress: *mut c_void,
        data: *mut c_void,
        cancel: *mut Bool,
        flags: u32,
    ) -> Bool;
    fn CreateSymbolicLinkW(link: *const u16, target: *const u16, flags: u32) -> u8;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const WAIT_TIMEOUT: u32 = 258;
const ERROR_FILE_EXISTS: u32 = 80;
const ERROR_PRIVILEGE_NOT_HELD: u32 = 1314;
const INVALID_HANDLE_VALUE: Handle = usize::MAX as Handle;
const CREATE_EVENT_MANUAL_RESET: u32 = 1;
const CREATE_EVENT_INITIAL_SET: u32 = 2;
const EVENT_ALL_ACCESS: u32 = 0x1f_0003;

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
}

fn hex(v: u32) -> [u8; 8] {
    let mut s = [0u8; 8];
    for (i, c) in s.iter_mut().enumerate() {
        let d = (v >> (28 - 4 * i)) & 0xf;
        *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
    }
    s
}

struct Checks(u32);

impl Checks {
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"kernel32_pool_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

/// A NUL-terminated UTF-16 copy of an ASCII string.
fn w(s: &str) -> [u16; 64] {
    let mut b = [0u16; 64];
    for (d, c) in b.iter_mut().zip(s.bytes()) {
        *d = c.into();
    }
    b
}

static WORK_RUNS: AtomicU32 = AtomicU32::new(0);
static WORK_BAD: AtomicU32 = AtomicU32::new(0);
static MAIN_TID: AtomicU32 = AtomicU32::new(0);
static WORK_OBJECT: AtomicUsize = AtomicUsize::new(0);
static TIMER_RUNS: AtomicU32 = AtomicU32::new(0);
static DONE: AtomicUsize = AtomicUsize::new(0);
static WAIT_RESULT: AtomicU32 = AtomicU32::new(u32::MAX);

extern "system" fn work(_instance: *mut c_void, context: *mut c_void, object: *mut c_void) {
    // SAFETY: a plain call.
    let tid = unsafe { GetCurrentThreadId() };
    if context as usize != 0x1234
        || object as usize != WORK_OBJECT.load(Ordering::SeqCst)
        || tid == MAIN_TID.load(Ordering::SeqCst)
    {
        WORK_BAD.fetch_add(1, Ordering::SeqCst);
    }
    // SAFETY: a plain call.
    unsafe { Sleep(5) };
    WORK_RUNS.fetch_add(1, Ordering::SeqCst);
}

/// Sets the DONE event.
fn done() {
    // SAFETY: the event created by the test.
    unsafe { SetEvent(DONE.load(Ordering::SeqCst) as Handle) };
}

extern "system" fn simple(_instance: *mut c_void, context: *mut c_void) {
    if context as usize == 0x5678 {
        done();
    }
}

extern "system" fn timer(_instance: *mut c_void, _context: *mut c_void, _object: *mut c_void) {
    TIMER_RUNS.fetch_add(1, Ordering::SeqCst);
    done();
}

extern "system" fn waited(
    _instance: *mut c_void,
    _context: *mut c_void,
    _object: *mut c_void,
    result: u32,
) {
    WAIT_RESULT.store(result, Ordering::SeqCst);
    done();
}

extern "system" fn loads_library(
    instance: *mut c_void,
    _context: *mut c_void,
    _object: *mut c_void,
) {
    // SAFETY: plain calls; the DLL is freed after this callback returns.
    unsafe {
        let h = LoadLibraryA(c"winmm.dll".as_ptr().cast());
        if !h.is_null() {
            FreeLibraryWhenCallbackReturns(instance, h);
        }
    }
}

fn works(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: thread pool calls with objects of ours.
    unsafe {
        MAIN_TID.store(GetCurrentThreadId(), Ordering::SeqCst);
        let w = CreateThreadpoolWork(work, 0x1234 as *mut c_void, core::ptr::null_mut());
        c.ok(!w.is_null(), GetLastError().into())?;
        WORK_OBJECT.store(w as usize, Ordering::SeqCst);
        for _ in 0..3 {
            SubmitThreadpoolWork(w);
        }
        WaitForThreadpoolWorkCallbacks(w, 0);
        c.eq(WORK_RUNS.load(Ordering::SeqCst).into(), 3)?;
        c.eq(WORK_BAD.load(Ordering::SeqCst).into(), 0)?;
        CloseThreadpoolWork(w);

        let event = DONE.load(Ordering::SeqCst) as Handle;
        c.ok(
            TrySubmitThreadpoolCallback(simple, 0x5678 as *mut c_void, core::ptr::null_mut()) != 0,
            0,
        )?;
        c.eq(WaitForSingleObject(event, 10_000).into(), 0)?;

        // FreeLibraryWhenCallbackReturns: winmm is unloaded afterwards.
        c.ok(GetModuleHandleA(c"winmm.dll".as_ptr().cast()).is_null(), 0)?;
        let l = CreateThreadpoolWork(loads_library, core::ptr::null_mut(), core::ptr::null_mut());
        SubmitThreadpoolWork(l);
        WaitForThreadpoolWorkCallbacks(l, 0);
        CloseThreadpoolWork(l);
        c.ok(GetModuleHandleA(c"winmm.dll".as_ptr().cast()).is_null(), 0)?;
    }
    Ok(())
}

fn timers(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: thread pool calls with objects of ours.
    unsafe {
        let event = DONE.load(Ordering::SeqCst) as Handle;
        let t = CreateThreadpoolTimer(timer, core::ptr::null_mut(), core::ptr::null_mut());
        c.ok(!t.is_null(), GetLastError().into())?;
        let due = -10 * 10_000i64; // 10 ms from now
        SetThreadpoolTimer(t, &due, 0, 0);
        c.eq(WaitForSingleObject(event, 10_000).into(), 0)?;
        WaitForThreadpoolTimerCallbacks(t, 0);
        c.eq(TIMER_RUNS.load(Ordering::SeqCst).into(), 1)?;
        // Periodic, then cancelled.
        SetThreadpoolTimer(t, &due, 10, 0);
        let mut waited = 0;
        while TIMER_RUNS.load(Ordering::SeqCst) < 4 && waited < 10_000 {
            Sleep(5);
            waited += 5;
        }
        c.ok(TIMER_RUNS.load(Ordering::SeqCst) >= 4, TIMER_RUNS.load(Ordering::SeqCst).into())?;
        SetThreadpoolTimer(t, core::ptr::null(), 0, 0);
        WaitForThreadpoolTimerCallbacks(t, 1);
        let runs = TIMER_RUNS.load(Ordering::SeqCst);
        Sleep(60);
        c.eq(TIMER_RUNS.load(Ordering::SeqCst).into(), runs.into())?;
        CloseThreadpoolTimer(t);
    }
    Ok(())
}

static QUEUE_RUNS: AtomicU32 = AtomicU32::new(0);
static QUEUE_BAD: AtomicU32 = AtomicU32::new(0);

extern "system" fn queued(parameter: *mut c_void, fired: u8) {
    if parameter as usize != 0x4321 || fired == 0 {
        QUEUE_BAD.fetch_add(1, Ordering::SeqCst);
    }
    QUEUE_RUNS.fetch_add(1, Ordering::SeqCst);
    done();
}

/// Sleeps until `QUEUE_RUNS` reaches `n` (or 10 s).
fn until_runs(n: u32) -> u32 {
    let mut waited = 0;
    while QUEUE_RUNS.load(Ordering::SeqCst) < n && waited < 10_000 {
        // SAFETY: a plain call.
        unsafe { Sleep(5) };
        waited += 5;
    }
    QUEUE_RUNS.load(Ordering::SeqCst)
}

fn timer_queues(c: &mut Checks) -> Result<(), u32> {
    const WT_EXECUTEONLYONCE: u32 = 8;
    let invalid = usize::MAX as Handle;
    let param = 0x4321 as *mut c_void;
    // SAFETY: kernel32 calls with objects of ours.
    unsafe {
        let event = DONE.load(Ordering::SeqCst) as Handle;
        ResetEvent(event);
        let queue = CreateTimerQueue();
        c.ok(!queue.is_null(), GetLastError().into())?;
        let mut t = core::ptr::null_mut();
        c.ok(
            CreateTimerQueueTimer(&mut t, queue, queued, param, 10, 0, WT_EXECUTEONLYONCE) != 0,
            GetLastError().into(),
        )?;
        c.ok(!t.is_null(), 0)?;
        c.eq(WaitForSingleObject(event, 10_000).into(), 0)?;
        c.ok(DeleteTimerQueueTimer(queue, t, invalid) != 0, GetLastError().into())?;
        c.eq(QUEUE_RUNS.load(Ordering::SeqCst).into(), 1)?;

        // Periodic; changed; deleted.
        c.ok(
            CreateTimerQueueTimer(&mut t, queue, queued, param, 5, 5, 0) != 0,
            GetLastError().into(),
        )?;
        c.ok(until_runs(4) >= 4, QUEUE_RUNS.load(Ordering::SeqCst).into())?;
        c.ok(ChangeTimerQueueTimer(queue, t, 1, 10) != 0, GetLastError().into())?;
        let now = QUEUE_RUNS.load(Ordering::SeqCst);
        c.ok(until_runs(now + 2) >= now + 2, QUEUE_RUNS.load(Ordering::SeqCst).into())?;
        c.ok(DeleteTimerQueueTimer(queue, t, invalid) != 0, GetLastError().into())?;
        let runs = QUEUE_RUNS.load(Ordering::SeqCst);
        Sleep(60);
        c.eq(QUEUE_RUNS.load(Ordering::SeqCst).into(), runs.into())?;

        // The default queue.
        ResetEvent(event);
        c.ok(
            CreateTimerQueueTimer(&mut t, core::ptr::null_mut(), queued, param, 1, 0, 0) != 0,
            GetLastError().into(),
        )?;
        c.eq(WaitForSingleObject(event, 10_000).into(), 0)?;
        c.ok(DeleteTimerQueueTimer(core::ptr::null_mut(), t, invalid) != 0, GetLastError().into())?;

        // Deleting the queue with its periodic timer.
        c.ok(
            CreateTimerQueueTimer(&mut t, queue, queued, param, 5, 5, 0) != 0,
            GetLastError().into(),
        )?;
        let runs = QUEUE_RUNS.load(Ordering::SeqCst);
        c.ok(until_runs(runs + 1) > runs, 0)?;
        let gone = CreateEventW(core::ptr::null_mut(), 1, 0, core::ptr::null());
        let ok = DeleteTimerQueueEx(queue, gone);
        c.ok(ok != 0 || GetLastError() == 997, GetLastError().into())?; // ERROR_IO_PENDING allowed
        c.eq(WaitForSingleObject(gone, 10_000).into(), 0)?;
        let runs = QUEUE_RUNS.load(Ordering::SeqCst);
        Sleep(60);
        c.eq(QUEUE_RUNS.load(Ordering::SeqCst).into(), runs.into())?;
        c.eq(QUEUE_BAD.load(Ordering::SeqCst).into(), 0)?;
        CloseHandle(gone);
    }
    Ok(())
}

fn waits(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: thread pool calls with objects of ours.
    unsafe {
        let event = DONE.load(Ordering::SeqCst) as Handle;
        ResetEvent(event); // the periodic timer's callbacks left it set
        let wait = CreateThreadpoolWait(waited, core::ptr::null_mut(), core::ptr::null_mut());
        c.ok(!wait.is_null(), GetLastError().into())?;
        let target = CreateEventW(core::ptr::null_mut(), 0, 0, core::ptr::null());
        SetThreadpoolWait(wait, target, core::ptr::null());
        Sleep(10);
        c.eq(WAIT_RESULT.load(Ordering::SeqCst).into(), u32::MAX.into())?; // not yet
        SetEvent(target);
        c.eq(WaitForSingleObject(event, 10_000).into(), 0)?;
        c.eq(WAIT_RESULT.load(Ordering::SeqCst).into(), 0)?; // WAIT_OBJECT_0
        let timeout = -20 * 10_000i64;
        SetThreadpoolWait(wait, target, &timeout);
        c.eq(WaitForSingleObject(event, 10_000).into(), 0)?;
        c.eq(WAIT_RESULT.load(Ordering::SeqCst).into(), WAIT_TIMEOUT.into())?;
        CloseThreadpoolWait(wait);
        CloseHandle(target);
    }
    Ok(())
}

fn files(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain calls with buffers and handles of ours.
    unsafe {
        let e = CreateEventExW(
            core::ptr::null_mut(),
            core::ptr::null(),
            CREATE_EVENT_MANUAL_RESET | CREATE_EVENT_INITIAL_SET,
            EVENT_ALL_ACCESS,
        );
        c.ok(!e.is_null(), GetLastError().into())?;
        c.eq(WaitForSingleObject(e, 0).into(), 0)?;
        c.eq(WaitForSingleObject(e, 0).into(), 0)?; // still set
        CloseHandle(e);

        let (a, b, d) = (w("C:\\pool-a.txt"), w("C:\\pool-b.txt"), w("C:\\pool-c.txt"));
        let h = CreateFileW(
            a.as_ptr(),
            0xc000_0000,
            0,
            core::ptr::null_mut(),
            2,
            0,
            core::ptr::null_mut(),
        );
        c.ok(h != INVALID_HANDLE_VALUE, GetLastError().into())?;
        let mut n = 0;
        WriteFile(h, b"hello".as_ptr(), 5, &mut n, core::ptr::null_mut());
        CloseHandle(h);
        c.ok(CopyFileW(a.as_ptr(), b.as_ptr(), 1) != 0, GetLastError().into())?;
        let h = CreateFileW(
            b.as_ptr(),
            0x8000_0000,
            1,
            core::ptr::null_mut(),
            3,
            0,
            core::ptr::null_mut(),
        );
        c.ok(h != INVALID_HANDLE_VALUE, GetLastError().into())?;
        let mut buf = [0u8; 16];
        ReadFile(h, buf.as_mut_ptr(), 16, &mut n, core::ptr::null_mut());
        CloseHandle(h);
        c.ok(n == 5 && &buf[..5] == b"hello", n.into())?;
        SetLastError(0);
        c.eq(CopyFileW(a.as_ptr(), b.as_ptr(), 1) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_FILE_EXISTS.into())?;
        c.ok(CopyFileW(a.as_ptr(), b.as_ptr(), 0) != 0, GetLastError().into())?;
        c.ok(
            CopyFileExW(
                a.as_ptr(),
                d.as_ptr(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                1,
            ) != 0,
            GetLastError().into(),
        )?;

        let link = w("C:\\pool-link.txt");
        c.eq(CreateSymbolicLinkW(link.as_ptr(), a.as_ptr(), 0).into(), 0)?;
        c.eq(GetLastError().into(), ERROR_PRIVILEGE_NOT_HELD.into())?;
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: a plain call.
    let event = unsafe { CreateEventW(core::ptr::null_mut(), 0, 0, core::ptr::null()) };
    c.ok(!event.is_null(), 0)?;
    DONE.store(event as usize, Ordering::SeqCst);
    works(&mut c)?;
    timers(&mut c)?;
    timer_queues(&mut c)?;
    waits(&mut c)?;
    files(&mut c)
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"kernel32_pool_test: ok\r\n");
            0
        }
        Err(id) => id,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
