//! A program on gala's ntdll and kernel32 carried across a kernel handover
//! while it works (ROADMAP R9 milestone (c), docs/ARCHITECTURE.md
//! "Programs across a handover"): it finds its state and results as if
//! nothing had happened, but for the time the handover took.
//!
//! Before asking for the handover (gala's test-only GalaRequestHandover)
//! the program sets up:
//! - two busy threads computing a checksum in user mode (integer and SSE
//!   floating-point state in registers, no system calls) until told to
//!   stop; afterwards the main thread computes the same checksum over the
//!   same number of rounds and compares (their full user context, with
//!   the vector registers, came across);
//! - a thread blocked in ReadFile on an anonymous pipe (it read a first
//!   message before; the second is written after the handover), and a
//!   second pipe holding bytes nobody has read yet;
//! - a thread writing numbered lines to the console with short sleeps
//!   between them, all through the handover;
//! - a user APC queued to a thread in a non-alertable wait (it runs after
//!   the handover, when the thread waits alertably), and a waitable timer
//!   with a completion routine due after the handover, its thread asleep
//!   alertably;
//! - a thread spinning for a critical section the main thread holds
//!   across the handover;
//! - kernel32 state in user memory: an environment variable, a TLS slot,
//!   VirtualAlloc'd memory with a pattern, the DLLs' own data;
//! - a thread waiting on a keyed event (released after the handover), a
//!   thread created suspended (resumed after), and one of the busy
//!   threads suspended across the handover.
//!
//! The main thread sleeps 4000 ms across the handover; the sleep must end
//! at its deadline (the call made again with its absolute deadline).
//!
//! With the argument `twice`, it asks again afterwards (pipes, DLL views
//! and kernel32's state go on into a third kernel).
//!
//! On Windows GalaRequestHandover (0x7f5a) is not a system service: the
//! program reports itself skipped and exits 0. Prints
//! `handover_k32_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};

type Bool = i32;
type Handle = *mut c_void;
type StartRoutine = extern "system" fn(*mut c_void) -> u32;

#[repr(C)]
struct CriticalSection {
    debug_info: usize,
    lock_count: i32,
    recursion: i32,
    owner: usize,
    semaphore: usize,
    spin_count: usize,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn CreatePipe(read: *mut Handle, write: *mut Handle, attrs: *mut c_void, size: u32) -> Bool;
    fn CreateThread(
        attrs: *mut c_void,
        stack: usize,
        start: StartRoutine,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn GetExitCodeThread(h: Handle, code: *mut u32) -> Bool;
    fn GetCurrentThreadId() -> u32;
    fn GetCurrentProcessId() -> u32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn CloseHandle(h: Handle) -> Bool;
    fn Sleep(ms: u32);
    fn SleepEx(ms: u32, alertable: Bool) -> u32;
    fn ExitProcess(code: u32) -> !;
    fn GetTickCount64() -> u64;
    fn CreateEventW(attrs: *mut c_void, manual: Bool, initial: Bool, name: *const u16) -> Handle;
    fn SetEvent(h: Handle) -> Bool;
    fn QueueUserAPC(routine: usize, thread: Handle, data: usize) -> u32;
    fn CreateWaitableTimerW(attrs: *mut c_void, manual: Bool, name: *const u16) -> Handle;
    fn SetWaitableTimer(
        h: Handle,
        due: *const i64,
        period: i32,
        routine: usize,
        arg: usize,
        resume: Bool,
    ) -> Bool;
    fn VirtualAlloc(addr: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut u8;
    fn SetEnvironmentVariableW(name: *const u16, value: *const u16) -> Bool;
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn TlsAlloc() -> u32;
    fn TlsSetValue(index: u32, value: *mut c_void) -> Bool;
    fn TlsGetValue(index: u32) -> *mut c_void;
    fn InitializeCriticalSection(cs: *mut CriticalSection);
    fn EnterCriticalSection(cs: *mut CriticalSection);
    fn LeaveCriticalSection(cs: *mut CriticalSection);
    fn SuspendThread(h: Handle) -> u32;
    fn ResumeThread(h: Handle) -> u32;
}

// The memory functions core needs, from ntdll (as the other kernel32
// tests take them).
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtCreateKeyedEvent(h: *mut Handle, access: u32, attrs: *const c_void, flags: u32) -> u32;
    fn NtWaitForKeyedEvent(h: Handle, key: usize, alertable: u8, timeout: *const i64) -> u32;
    fn NtReleaseKeyedEvent(h: Handle, key: usize, alertable: u8, timeout: *const i64) -> u32;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
const INFINITE: u32 = u32::MAX;
const WAIT_TIMEOUT: u32 = 0x102;
const WAIT_IO_COMPLETION: u32 = 0xc0;
const STILL_ACTIVE: u32 = 0x103;
const STATUS_INVALID_SYSTEM_SERVICE: u32 = 0xc000_001c;

/// How long the main thread sleeps across the handover (ms).
const SLEEP_MS: u64 = 4000;
/// When the timer with a completion routine is due (ms after it is set):
/// after the handover.
const TIMER_MS: i64 = 2500;
/// How much later than asked the sleep may end (ms): a busy machine. A
/// sleep made again with its relative timeout would end later by the
/// handover's time (a few hundred ms).
const SLACK_MS: u64 = 150;
const BUSY: usize = 2;
const PATTERN_BYTES: usize = 3 * 4096 + 100;

static STOP: AtomicU32 = AtomicU32::new(0);
static ROUNDS: [AtomicU64; BUSY] = [const { AtomicU64::new(0) }; BUSY];
static SUM: [AtomicU64; BUSY] = [const { AtomicU64::new(0) }; BUSY];
static FSUM: [AtomicU64; BUSY] = [const { AtomicU64::new(0) }; BUSY];
static PIPE_READ: AtomicUsize = AtomicUsize::new(0);
static GATE: AtomicUsize = AtomicUsize::new(0);
static TIMER: AtomicUsize = AtomicUsize::new(0);
static APC_RAN: AtomicUsize = AtomicUsize::new(0);
static TIMER_APC_RAN: AtomicUsize = AtomicUsize::new(0);
static STARTED: AtomicU32 = AtomicU32::new(0);
static LINES: AtomicU32 = AtomicU32::new(0);
static CS_TAKEN: AtomicI32 = AtomicI32::new(0);
static KEYED: AtomicUsize = AtomicUsize::new(0);
static LATE_RAN: AtomicU32 = AtomicU32::new(0);
/// The key the keyed-event thread waits for.
const KEY: usize = 0x5a50;
static mut CS: CriticalSection = CriticalSection {
    debug_info: 0,
    lock_count: -1,
    recursion: 0,
    owner: 0,
    semaphore: 0,
    spin_count: 0,
};

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: a valid buffer; the standard output handle.
    unsafe {
        WriteFile(
            GetStdHandle(STD_OUTPUT_HANDLE),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
}

/// "handover_k32_test: <what> <n>\r\n" in one write.
fn say_n(what: &[u8], n: u64) -> u32 {
    let mut line = [0u8; 96];
    let mut len = 0;
    for &b in b"handover_k32_test: ".iter().chain(what).chain(b" ") {
        line[len] = b;
        len += 1;
    }
    let mut digits = [0u8; 20];
    let mut d = 0;
    let mut v = n;
    loop {
        digits[d] = b'0' + (v % 10) as u8;
        d += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    for i in (0..d).rev() {
        line[len] = digits[i];
        len += 1;
    }
    line[len] = b'\r';
    line[len + 1] = b'\n';
    let mut written = 0;
    // SAFETY: as in `out`.
    let ok = unsafe {
        WriteFile(
            GetStdHandle(STD_OUTPUT_HANDLE),
            line.as_ptr(),
            len as u32 + 2,
            &mut written,
            core::ptr::null_mut(),
        )
    };
    if ok != 0 && written == len as u32 + 2 { written } else { 0 }
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetCommandLineW() -> *const u16;
}

/// Whether the command line's last word is `word`.
fn last_arg_is(word: &str) -> bool {
    // SAFETY: GetCommandLineW's string is NUL-terminated and stays.
    let cmd = unsafe {
        let p = GetCommandLineW();
        let mut len = 0;
        while *p.add(len) != 0 {
            len += 1;
        }
        core::slice::from_raw_parts(p, len)
    };
    let mut end = cmd.len();
    while end > 0 && cmd[end - 1] == u16::from(b' ') {
        end -= 1;
    }
    let word = word.as_bytes();
    end > word.len()
        && cmd[end - word.len() - 1] == u16::from(b' ')
        && cmd[end - word.len()..end].iter().zip(word).all(|(&a, &b)| a == u16::from(b))
}

/// GalaRequestHandover (gala's test-only service 0x7f5a).
fn request_handover() -> u32 {
    let status: u32;
    // SAFETY: a system call with no arguments; the registers it may
    // change are declared.
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("eax") 0x7f5au32 => status,
            inout("r10") 0usize => _,
            lateout("rcx") _,
            lateout("rdx") _,
            lateout("r8") _,
            lateout("r9") _,
            lateout("r11") _,
            options(nostack),
        )
    };
    status
}

/// One round of the busy threads' work: 1024 steps of xorshift with a
/// floating-point accumulation (SSE) of each value.
#[inline(never)]
fn round(mut x: u64, mut f: f64) -> (u64, f64) {
    for _ in 0..1024 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        f = f * 0.999_999_7 + (x & 0xffff) as f64 * 1.0e-3;
    }
    (x, f)
}

fn seed(i: usize) -> u64 {
    0x9e37_79b9_7f4a_7c15 ^ ((i as u64 + 1) * 0x1234_5678_9abc_def1)
}

/// Rounds until STOP, keeping the state in registers.
extern "system" fn busy_thread(param: *mut c_void) -> u32 {
    let i = param as usize;
    let (mut x, mut f) = (seed(i), 1.0f64);
    let mut n = 0u64;
    STARTED.fetch_add(1, Ordering::SeqCst);
    while STOP.load(Ordering::Relaxed) == 0 {
        (x, f) = round(x, f);
        n += 1;
        ROUNDS[i].store(n, Ordering::Relaxed);
    }
    SUM[i].store(x, Ordering::SeqCst);
    FSUM[i].store(f.to_bits(), Ordering::SeqCst);
    ROUNDS[i].store(n, Ordering::SeqCst);
    0x11
}

/// Reads a first message, then blocks in ReadFile for the second, which
/// is written after the handover.
extern "system" fn reader_thread(_: *mut c_void) -> u32 {
    let h = PIPE_READ.load(Ordering::SeqCst) as Handle;
    let mut buf = [0u8; 64];
    let mut n = 0;
    // SAFETY: a valid buffer and handle.
    if unsafe { ReadFile(h, buf.as_mut_ptr(), 64, &mut n, core::ptr::null_mut()) } == 0
        || &buf[..n as usize] != b"before"
    {
        return 0x401;
    }
    STARTED.fetch_add(1, Ordering::SeqCst);
    // SAFETY: as above.
    if unsafe { ReadFile(h, buf.as_mut_ptr(), 64, &mut n, core::ptr::null_mut()) } == 0 {
        return 0x402;
    }
    if &buf[..n as usize] != b"after the handover" {
        return 0x403;
    }
    0x44
}

/// Writes numbered lines to the console until STOP, a few ms apart.
extern "system" fn writer_thread(_: *mut c_void) -> u32 {
    STARTED.fetch_add(1, Ordering::SeqCst);
    let mut n = 0;
    while STOP.load(Ordering::SeqCst) == 0 {
        if say_n(b"line", u64::from(n)) == 0 {
            return 0x501;
        }
        n += 1;
        LINES.store(n, Ordering::SeqCst);
        // SAFETY: plain call.
        unsafe { Sleep(20) };
    }
    0x55
}

extern "system" fn apc_routine(data: usize) {
    APC_RAN.store(data, Ordering::SeqCst);
}

/// Waits (not alertably) for GATE with a user APC queued, then lets it
/// run with an alertable SleepEx.
extern "system" fn apc_thread(_: *mut c_void) -> u32 {
    STARTED.fetch_add(1, Ordering::SeqCst);
    // SAFETY: a valid event handle.
    if unsafe { WaitForSingleObject(GATE.load(Ordering::SeqCst) as Handle, INFINITE) } != 0 {
        return 0x601;
    }
    if APC_RAN.load(Ordering::SeqCst) != 0 {
        return 0x602; // ran during a non-alertable wait
    }
    // SAFETY: plain call.
    if unsafe { SleepEx(0, 1) } != WAIT_IO_COMPLETION {
        return 0x603;
    }
    if APC_RAN.load(Ordering::SeqCst) != 0x66 {
        return 0x604;
    }
    0x66
}

extern "system" fn timer_routine(context: usize, _low: u32, _high: u32) {
    TIMER_APC_RAN.store(context, Ordering::SeqCst);
}

/// Sets the timer with a completion routine (its APC comes to this
/// thread) and sleeps alertably until it runs.
extern "system" fn timer_thread(_: *mut c_void) -> u32 {
    let due = -TIMER_MS * 10_000;
    let h = TIMER.load(Ordering::SeqCst) as Handle;
    // SAFETY: a valid timer handle and due time.
    if unsafe { SetWaitableTimer(h, &due, 0, timer_routine as *const () as usize, 0x77, 0) } == 0 {
        return 0x701;
    }
    STARTED.fetch_add(1, Ordering::SeqCst);
    // SAFETY: plain call.
    if unsafe { SleepEx(30_000, 1) } != WAIT_IO_COMPLETION {
        return 0x702;
    }
    if TIMER_APC_RAN.load(Ordering::SeqCst) != 0x77 {
        return 0x703;
    }
    0x77
}

/// Spins (yielding) for the critical section the main thread holds.
extern "system" fn cs_thread(_: *mut c_void) -> u32 {
    STARTED.fetch_add(1, Ordering::SeqCst);
    // SAFETY: an initialized critical section.
    unsafe { EnterCriticalSection(&raw mut CS) };
    CS_TAKEN.store(1, Ordering::SeqCst);
    // SAFETY: we own it.
    unsafe { LeaveCriticalSection(&raw mut CS) };
    0x88
}

/// Waits on a keyed event for KEY (released after the handover).
extern "system" fn keyed_thread(_: *mut c_void) -> u32 {
    STARTED.fetch_add(1, Ordering::SeqCst);
    let h = KEYED.load(Ordering::SeqCst) as Handle;
    // SAFETY: a valid keyed event handle; no timeout.
    match unsafe { NtWaitForKeyedEvent(h, KEY, 0, core::ptr::null()) } {
        0 => 0x99,
        st => 0x900 | (st & 0xff),
    }
}

/// Created suspended; resumed only after the handover.
extern "system" fn late_thread(param: *mut c_void) -> u32 {
    LATE_RAN.store(param as u32, Ordering::SeqCst);
    0xaa
}

fn pattern(i: usize) -> u8 {
    (i.wrapping_mul(31) ^ (i >> 8)) as u8
}

fn spawn(start: StartRoutine, param: usize) -> Result<Handle, u32> {
    spawn_flags(start, param, 0)
}

fn spawn_flags(start: StartRoutine, param: usize, flags: u32) -> Result<Handle, u32> {
    let mut id = 0;
    // SAFETY: a valid start routine.
    let h = unsafe {
        CreateThread(core::ptr::null_mut(), 0, start, param as *mut c_void, flags, &mut id)
    };
    if h.is_null() { Err(90) } else { Ok(h) }
}

fn check(ok: bool, n: u32) -> Result<(), u32> {
    if ok { Ok(()) } else { Err(n) }
}

fn exit_code(h: Handle) -> u32 {
    let mut code = 0;
    // SAFETY: a thread handle.
    unsafe { GetExitCodeThread(h, &mut code) };
    code
}

fn run() -> Result<&'static str, u32> {
    // SAFETY: documented kernel32 calls with valid arguments throughout.
    unsafe {
        // kernel32 state in user memory.
        let name = [b'G' as u16, b'A' as u16, b'L' as u16, b'A' as u16, 0];
        let value = [b'y' as u16, b'e' as u16, b's' as u16, 0];
        check(SetEnvironmentVariableW(name.as_ptr(), value.as_ptr()) != 0, 1)?;
        let tls = TlsAlloc();
        check(tls != u32::MAX, 2)?;
        check(TlsSetValue(tls, 0x1234_5678 as *mut c_void) != 0, 3)?;
        let memory = VirtualAlloc(core::ptr::null_mut(), 64 * 1024, 0x3000, 4);
        check(!memory.is_null(), 4)?;
        for i in 0..PATTERN_BYTES {
            memory.add(i).write_volatile(pattern(i));
        }
        let (pid, tid) = (GetCurrentProcessId(), GetCurrentThreadId());

        // Pipes: one for the blocked reader, one holding unread bytes.
        let (mut r1, mut w1, mut r2, mut w2) = (
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        check(CreatePipe(&mut r1, &mut w1, core::ptr::null_mut(), 0) != 0, 5)?;
        check(CreatePipe(&mut r2, &mut w2, core::ptr::null_mut(), 0) != 0, 6)?;
        PIPE_READ.store(r1 as usize, Ordering::SeqCst);
        let mut n = 0;
        check(WriteFile(w1, b"before".as_ptr(), 6, &mut n, core::ptr::null_mut()) != 0, 7)?;
        check(WriteFile(w2, b"carried bytes".as_ptr(), 13, &mut n, core::ptr::null_mut()) != 0, 8)?;

        // Objects for the APC thread and the timer thread.
        let gate = CreateEventW(core::ptr::null_mut(), 1, 0, core::ptr::null());
        check(!gate.is_null(), 9)?;
        GATE.store(gate as usize, Ordering::SeqCst);
        let timer = CreateWaitableTimerW(core::ptr::null_mut(), 0, core::ptr::null());
        check(!timer.is_null(), 10)?;
        TIMER.store(timer as usize, Ordering::SeqCst);
        InitializeCriticalSection(&raw mut CS);
        EnterCriticalSection(&raw mut CS);
        let mut keyed = core::ptr::null_mut();
        check(NtCreateKeyedEvent(&mut keyed, 0x1f_0003, core::ptr::null(), 0) == 0, 46)?;
        KEYED.store(keyed as usize, Ordering::SeqCst);

        let mut busy = [core::ptr::null_mut(); BUSY];
        for (i, h) in busy.iter_mut().enumerate() {
            *h = spawn(busy_thread, i)?;
        }
        let reader = spawn(reader_thread, 0)?;
        let writer = spawn(writer_thread, 0)?;
        let apc = spawn(apc_thread, 0)?;
        let timed = spawn(timer_thread, 0)?;
        let cs = spawn(cs_thread, 0)?;
        let waiter = spawn(keyed_thread, 0)?;
        // CREATE_SUSPENDED: never runs before the handover.
        let late = spawn_flags(late_thread, 0x5e, 4)?;
        while STARTED.load(Ordering::SeqCst) < BUSY as u32 + 6 {
            Sleep(1);
        }
        // The APC thread is (about to be) in its non-alertable wait.
        Sleep(50);
        check(QueueUserAPC(apc_routine as *const () as usize, apc, 0x66) != 0, 11)?;
        while (0..BUSY).any(|i| ROUNDS[i].load(Ordering::SeqCst) == 0) {
            Sleep(1);
        }
        // The second busy thread is suspended across the handover.
        check(SuspendThread(busy[1]) == 0, 47)?;

        // The handover, while the busy threads compute, the reader waits
        // in ReadFile, the writer writes and sleeps.
        let rounds_before = [ROUNDS[0].load(Ordering::SeqCst), ROUNDS[1].load(Ordering::SeqCst)];
        let lines_before = LINES.load(Ordering::SeqCst);
        out(b"handover_k32_test: ready, asking for a handover\r\n");
        let before = GetTickCount64();
        match request_handover() {
            0 => {}
            STATUS_INVALID_SYSTEM_SERVICE => {
                STOP.store(1, Ordering::SeqCst);
                return Ok("skipped (no GalaRequestHandover: not gala)");
            }
            0xc000_0022 => {
                STOP.store(1, Ordering::SeqCst);
                return Ok("skipped (no handover allowed)");
            }
            _ => return Err(12),
        }
        Sleep(SLEEP_MS as u32);
        let slept = GetTickCount64() - before;
        say_n(b"slept", slept);
        check(slept >= SLEEP_MS, 13)?;
        check(slept < SLEEP_MS + SLACK_MS, 14)?;

        // Work went on across (the suspended one once resumed).
        check(ROUNDS[0].load(Ordering::SeqCst) > rounds_before[0], 15)?;
        let suspended_at = ROUNDS[1].load(Ordering::SeqCst);
        check(suspended_at >= rounds_before[1], 16)?;
        check(ResumeThread(busy[1]) == 1, 48)?;
        while ROUNDS[1].load(Ordering::SeqCst) == suspended_at {
            Sleep(1);
        }
        check(LINES.load(Ordering::SeqCst) > lines_before, 17)?;
        STOP.store(1, Ordering::SeqCst);
        for (i, &h) in busy.iter().enumerate() {
            check(WaitForSingleObject(h, 30_000) == 0, 18)?;
            check(exit_code(h) == 0x11, 19)?;
            // The same work again, here: the same result.
            let (mut x, mut f) = (seed(i), 1.0f64);
            for _ in 0..ROUNDS[i].load(Ordering::SeqCst) {
                (x, f) = round(x, f);
            }
            check(x == SUM[i].load(Ordering::SeqCst), 20)?;
            check(f.to_bits() == FSUM[i].load(Ordering::SeqCst), 21)?;
            CloseHandle(h);
        }
        check(WaitForSingleObject(writer, 10_000) == 0 && exit_code(writer) == 0x55, 22)?;
        say_n(b"writer wrote lines:", u64::from(LINES.load(Ordering::SeqCst)));

        // The reader is still in ReadFile; the second message reaches it.
        check(WaitForSingleObject(reader, 0) == WAIT_TIMEOUT, 23)?;
        check(exit_code(reader) == STILL_ACTIVE, 24)?;
        let msg = b"after the handover";
        check(
            WriteFile(w1, msg.as_ptr(), msg.len() as u32, &mut n, core::ptr::null_mut()) != 0,
            25,
        )?;
        check(WaitForSingleObject(reader, 10_000) == 0, 26)?;
        check(exit_code(reader) == 0x44, 27)?;
        // The unread bytes are still in the other pipe.
        let mut buf = [0u8; 32];
        check(ReadFile(r2, buf.as_mut_ptr(), 32, &mut n, core::ptr::null_mut()) != 0, 28)?;
        check(&buf[..n as usize] == b"carried bytes", 29)?;

        // The queued APC runs once its thread waits alertably.
        check(APC_RAN.load(Ordering::SeqCst) == 0, 30)?;
        check(SetEvent(gate) != 0, 31)?;
        check(WaitForSingleObject(apc, 10_000) == 0, 32)?;
        check(exit_code(apc) == 0x66, 33)?;
        // The timer's completion routine ran in its thread.
        check(WaitForSingleObject(timed, 30_000) == 0, 34)?;
        check(exit_code(timed) == 0x77, 35)?;
        // The critical section: the spinning thread gets it now.
        check(CS_TAKEN.load(Ordering::SeqCst) == 0, 36)?;
        LeaveCriticalSection(&raw mut CS);
        check(WaitForSingleObject(cs, 10_000) == 0 && exit_code(cs) == 0x88, 37)?;
        check(CS_TAKEN.load(Ordering::SeqCst) == 1, 38)?;

        // The keyed event's waiter is still waiting; the release pairs.
        check(WaitForSingleObject(waiter, 0) == WAIT_TIMEOUT, 49)?;
        check(NtReleaseKeyedEvent(keyed, KEY, 0, core::ptr::null()) == 0, 50)?;
        check(WaitForSingleObject(waiter, 10_000) == 0 && exit_code(waiter) == 0x99, 51)?;
        // The thread created suspended has not run; it starts now.
        check(LATE_RAN.load(Ordering::SeqCst) == 0, 52)?;
        check(ResumeThread(late) == 1, 53)?;
        check(WaitForSingleObject(late, 10_000) == 0 && exit_code(late) == 0xaa, 54)?;
        check(LATE_RAN.load(Ordering::SeqCst) == 0x5e, 55)?;

        // kernel32's state.
        let mut got = [0u16; 8];
        check(GetEnvironmentVariableW(name.as_ptr(), got.as_mut_ptr(), 8) == 3, 39)?;
        check(got[..4] == value, 40)?;
        check(TlsGetValue(tls) as usize == 0x1234_5678, 41)?;
        check((0..PATTERN_BYTES).all(|i| memory.add(i).read_volatile() == pattern(i)), 42)?;
        check(memory.add(PATTERN_BYTES).read_volatile() == 0, 43)?;
        check(GetCurrentProcessId() == pid && GetCurrentThreadId() == tid, 44)?;
        if last_arg_is("twice") {
            // Again, with only this thread left and the pipes, events and
            // DLL views as they are now: on into a third kernel.
            check(WriteFile(w2, b"again".as_ptr(), 5, &mut n, core::ptr::null_mut()) != 0, 56)?;
            check(request_handover() == 0, 57)?;
            let before = GetTickCount64();
            Sleep(1000);
            let slept = GetTickCount64() - before;
            say_n(b"slept again", slept);
            // This sleep spans the handover itself, during which the thread
            // is frozen and the clock goes on (under a loaded host the
            // handover alone took 232 ms): never short, at most a second late.
            check((1000..2000 + SLACK_MS).contains(&slept), 58)?;
            check(ReadFile(r2, buf.as_mut_ptr(), 32, &mut n, core::ptr::null_mut()) != 0, 59)?;
            check(&buf[..n as usize] == b"again", 60)?;
            check(GetEnvironmentVariableW(name.as_ptr(), got.as_mut_ptr(), 8) == 3, 61)?;
            check(TlsGetValue(tls) as usize == 0x1234_5678, 62)?;
            check((0..PATTERN_BYTES).all(|i| memory.add(i).read_volatile() == pattern(i)), 63)?;
            check(exit_code(waiter) == 0x99 && exit_code(late) == 0xaa, 64)?;
        }
        for h in [reader, writer, apc, timed, cs, waiter, late, keyed, gate, timer, r1, w1, r2, w2]
        {
            check(CloseHandle(h) != 0, 45)?;
        }
    }
    Ok("ok")
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(what) => {
            let mut line = [0u8; 80];
            let mut n = 0;
            for &b in b"handover_k32_test: ".iter().chain(what.as_bytes()).chain(b"\r\n") {
                line[n] = b;
                n += 1;
            }
            out(&line[..n]);
            0
        }
        Err(n) => {
            say_n(b"failed check", u64::from(n));
            n
        }
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
