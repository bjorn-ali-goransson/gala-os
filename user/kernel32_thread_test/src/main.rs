//! kernel32 threads as the Go runtime uses them, with the results real
//! Windows gives: CreateThread (running at once, or CREATE_SUSPENDED until
//! ResumeThread), thread exit codes, TLS slots (TlsAlloc/TlsSetValue/
//! TlsGetValue, stored in the TEB's TlsSlots at +0x1480, zero in a new
//! thread), SuspendThread/ResumeThread counts, and redirecting a suspended
//! thread with GetThreadContext/SetThreadContext (how Go preempts
//! goroutines). Prints `kernel32_thread_test: ok` or exits with the number
//! of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU64, Ordering};

type Bool = i32;
type Handle = *mut c_void;
type StartRoutine = extern "system" fn(*mut c_void) -> u32;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn CreateThread(
        attrs: *mut c_void,
        stack: usize,
        start: StartRoutine,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn ExitThread(code: u32) -> !;
    fn GetExitCodeThread(h: Handle, code: *mut u32) -> Bool;
    fn GetCurrentThreadId() -> u32;
    fn SuspendThread(h: Handle) -> u32;
    fn ResumeThread(h: Handle) -> u32;
    fn GetThreadContext(h: Handle, context: *mut Context) -> Bool;
    fn SetThreadContext(h: Handle, context: *const Context) -> Bool;
    fn TlsAlloc() -> u32;
    fn TlsGetValue(index: u32) -> *mut c_void;
    fn TlsSetValue(index: u32, value: *mut c_void) -> Bool;
    fn TlsFree(index: u32) -> Bool;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn CloseHandle(h: Handle) -> Bool;
    fn Sleep(ms: u32);
    fn ExitProcess(code: u32) -> !;
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

/// CONTEXT (x64), 16-byte aligned as the API requires.
#[repr(C, align(16))]
struct Context {
    home: [u64; 6],
    context_flags: u32,
    mx_csr: u32,
    segs: [u16; 6],
    eflags: u32,
    dr: [u64; 6],
    /// Rax, Rcx, Rdx, Rbx, Rsp, Rbp, Rsi, Rdi, R8..R15.
    gpr: [u64; 16],
    rip: u64,
    rest: [u8; 0x4d0 - 0x100],
}

const CONTEXT_CONTROL_INTEGER: u32 = 0x10_0003;
const CREATE_SUSPENDED: u32 = 4;
const TLS_OUT_OF_INDEXES: u32 = 0xffff_ffff;
const INFINITE: u32 = 0xffff_ffff;
const STILL_ACTIVE: u32 = 259;

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
        out(b"kernel32_thread_test: check ");
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

static RESULT: AtomicU64 = AtomicU64::new(0);
static THREAD_ID: AtomicU64 = AtomicU64::new(0);
static TLS_SEEN: AtomicU64 = AtomicU64::new(u64::MAX);
static TLS_INDEX: AtomicU64 = AtomicU64::new(0);
static SPINS: AtomicU64 = AtomicU64::new(0);
static REDIRECTED: AtomicU64 = AtomicU64::new(0);

extern "system" fn worker(param: *mut c_void) -> u32 {
    // SAFETY: plain kernel32 calls.
    unsafe {
        THREAD_ID.store(GetCurrentThreadId().into(), Ordering::SeqCst);
        TLS_SEEN
            .store(TlsGetValue(TLS_INDEX.load(Ordering::SeqCst) as u32) as u64, Ordering::SeqCst);
    }
    RESULT.store(param as u64 * 6, Ordering::SeqCst);
    99
}

extern "system" fn spinner(_: *mut c_void) -> u32 {
    loop {
        SPINS.fetch_add(1, Ordering::SeqCst);
        core::hint::spin_loop();
    }
}

/// Where the suspended spinner is sent with SetThreadContext.
extern "C" fn redirected() -> ! {
    REDIRECTED.store(1, Ordering::SeqCst);
    // SAFETY: ends this thread.
    unsafe { ExitThread(5) }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: documented kernel32 calls with valid arguments.
    unsafe {
        // TLS: an index, a slot in this thread's TEB.
        let index = TlsAlloc();
        c.ok(index != TLS_OUT_OF_INDEXES && index < 64, index.into())?;
        TLS_INDEX.store(index.into(), Ordering::SeqCst);
        c.eq(TlsGetValue(index) as u64, 0)?;
        c.ok(TlsSetValue(index, 0x1234 as *mut c_void) != 0, 0)?;
        c.eq(TlsGetValue(index) as u64, 0x1234)?;
        let slot: u64;
        core::arch::asm!("mov {}, gs:[{}]", out(reg) slot, in(reg) 0x1480 + 8 * index as usize, options(nostack, readonly));
        c.eq(slot, 0x1234)?;

        // A thread that runs at once.
        let mut tid = 0;
        let h = CreateThread(core::ptr::null_mut(), 0, worker, 7 as *mut c_void, 0, &mut tid);
        c.ok(!h.is_null() && tid != 0, tid.into())?;
        c.eq(WaitForSingleObject(h, INFINITE).into(), 0)?;
        c.eq(RESULT.load(Ordering::SeqCst), 42)?;
        c.eq(THREAD_ID.load(Ordering::SeqCst), tid.into())?;
        c.eq(TLS_SEEN.load(Ordering::SeqCst), 0)?; // a new thread's slots are zero
        let mut code = 0;
        c.ok(GetExitCodeThread(h, &mut code) != 0, 0)?;
        c.eq(code.into(), 99)?;
        CloseHandle(h);

        // CREATE_SUSPENDED: nothing runs until ResumeThread.
        RESULT.store(0, Ordering::SeqCst);
        let h = CreateThread(
            core::ptr::null_mut(),
            0,
            worker,
            2 as *mut c_void,
            CREATE_SUSPENDED,
            &mut tid,
        );
        c.ok(!h.is_null(), 0)?;
        Sleep(50);
        c.eq(RESULT.load(Ordering::SeqCst), 0)?;
        c.ok(GetExitCodeThread(h, &mut code) != 0 && code == STILL_ACTIVE, code.into())?;
        c.eq(ResumeThread(h).into(), 1)?;
        c.eq(WaitForSingleObject(h, INFINITE).into(), 0)?;
        c.eq(RESULT.load(Ordering::SeqCst), 12)?;
        CloseHandle(h);

        // Suspend a running thread, read and change its registers.
        let h = CreateThread(core::ptr::null_mut(), 0, spinner, core::ptr::null_mut(), 0, &mut tid);
        while SPINS.load(Ordering::SeqCst) < 1000 {
            Sleep(1);
        }
        c.eq(SuspendThread(h).into(), 0)?;
        c.eq(SuspendThread(h).into(), 1)?;
        c.eq(ResumeThread(h).into(), 2)?;
        // SuspendThread is asynchronous (the thread may run on another CPU
        // for a moment); GetThreadContext returns once it has stopped, so
        // read the counter after it, as the Win32 documentation advises.
        let mut ctx: Context = core::mem::zeroed();
        ctx.context_flags = CONTEXT_CONTROL_INTEGER;
        c.ok(GetThreadContext(h, &mut ctx) != 0, 0)?;
        let spins = SPINS.load(Ordering::SeqCst);
        Sleep(20);
        c.eq(SPINS.load(Ordering::SeqCst), spins)?; // still suspended (count 1)
        c.ok(GetThreadContext(h, &mut ctx) != 0, 0)?;
        c.ok(ctx.rip != 0 && ctx.gpr[4] != 0, ctx.rip)?;
        ctx.rip = redirected as *const () as u64;
        ctx.gpr[4] = (ctx.gpr[4] & !0xf) - 8; // as if called
        c.ok(SetThreadContext(h, &ctx) != 0, 0)?;
        c.eq(ResumeThread(h).into(), 1)?;
        c.eq(WaitForSingleObject(h, 5000).into(), 0)?;
        c.eq(REDIRECTED.load(Ordering::SeqCst), 1)?;
        c.ok(GetExitCodeThread(h, &mut code) != 0 && code == 5, code.into())?;
        CloseHandle(h);
        c.ok(TlsFree(index) != 0, 0)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"kernel32_thread_test: ok\r\n");
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
