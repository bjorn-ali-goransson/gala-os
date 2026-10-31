//! The rest of what the Go runtime imports from kernel32, with the results
//! real Windows gives: I/O completion ports (CreateIoCompletionPort,
//! PostQueuedCompletionStatus, GetQueuedCompletionStatusEx removing
//! several entries, WAIT_TIMEOUT when empty), waitable timers
//! (CreateWaitableTimerExW, SetWaitableTimer with a relative due time),
//! vectored exception and continue handlers (AddVectoredExceptionHandler
//! sees a breakpoint and continues past it), and RtlLookupFunctionEntry
//! (the unwind entry of a function in this image), and FormatMessageW's
//! system messages (English text with a CRLF; ERROR_MR_MID_NOT_FOUND for
//! a number without one; ERROR_INSUFFICIENT_BUFFER). Prints
//! `kernel32_misc_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU64, Ordering};

type Bool = i32;
type Handle = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct OverlappedEntry {
    completion_key: usize,
    overlapped: usize,
    internal: usize,
    bytes: u32,
}

#[repr(C)]
struct ExceptionPointers {
    record: *const u32,
    context: *mut u8,
}

#[repr(C)]
struct RuntimeFunction {
    begin: u32,
    end: u32,
    unwind: u32,
}

type VectoredHandler = extern "system" fn(*mut ExceptionPointers) -> i32;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn FormatMessageW(
        flags: u32,
        source: *const c_void,
        message: u32,
        language: u32,
        buffer: *mut u16,
        size: u32,
        args: *mut c_void,
    ) -> u32;
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn GetLastError() -> u32;
    fn SetLastError(code: u32);
    fn CreateIoCompletionPort(file: Handle, port: Handle, key: usize, threads: u32) -> Handle;
    fn PostQueuedCompletionStatus(port: Handle, bytes: u32, key: usize, overlapped: usize) -> Bool;
    fn GetQueuedCompletionStatusEx(
        port: Handle,
        entries: *mut OverlappedEntry,
        count: u32,
        removed: *mut u32,
        ms: u32,
        alertable: Bool,
    ) -> Bool;
    fn CreateWaitableTimerExW(
        attrs: *mut c_void,
        name: *const u16,
        flags: u32,
        access: u32,
    ) -> Handle;
    fn SetWaitableTimer(
        h: Handle,
        due: *const i64,
        period: i32,
        routine: *mut c_void,
        arg: *mut c_void,
        resume: Bool,
    ) -> Bool;
    fn AddVectoredExceptionHandler(first: u32, handler: VectoredHandler) -> *mut c_void;
    fn AddVectoredContinueHandler(first: u32, handler: VectoredHandler) -> *mut c_void;
    fn RtlLookupFunctionEntry(
        pc: u64,
        base: *mut u64,
        history: *mut c_void,
    ) -> *const RuntimeFunction;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetTickCount64() -> u64;
    fn CloseHandle(h: Handle) -> Bool;
    fn ExitProcess(code: u32) -> !;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

unsafe extern "C" {
    static __ImageBase: u8;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
const WAIT_TIMEOUT: u32 = 258;
const TIMER_ALL_ACCESS: u32 = 0x1f_0003;
const STATUS_BREAKPOINT: u32 = 0x8000_0003;
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
const CTX_RIP: usize = 0xf8;

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
        out(b"kernel32_misc_test: check ");
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

static BREAKPOINTS: AtomicU64 = AtomicU64::new(0);

/// Continues past breakpoints (rip points at the int3).
extern "system" fn on_exception(pointers: *mut ExceptionPointers) -> i32 {
    // SAFETY: the dispatcher passes valid EXCEPTION_POINTERS.
    unsafe {
        if *(*pointers).record != STATUS_BREAKPOINT {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        BREAKPOINTS.fetch_add(1, Ordering::SeqCst);
        let rip = (*pointers).context.add(CTX_RIP) as *mut u64;
        *rip += 1;
    }
    EXCEPTION_CONTINUE_EXECUTION
}

extern "system" fn on_continue(_: *mut ExceptionPointers) -> i32 {
    EXCEPTION_CONTINUE_SEARCH
}

#[inline(never)]
extern "C" fn some_function() -> u64 {
    core::hint::black_box(7)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: documented kernel32 calls with valid arguments.
    unsafe {
        // I/O completion ports.
        let port = CreateIoCompletionPort(INVALID_HANDLE_VALUE, core::ptr::null_mut(), 0, 1);
        c.ok(!port.is_null(), 0)?;
        let mut entries = [OverlappedEntry::default(); 4];
        let mut removed = 0;
        SetLastError(0);
        c.eq(
            GetQueuedCompletionStatusEx(port, entries.as_mut_ptr(), 4, &mut removed, 0, 0) as u64,
            0,
        )?;
        c.eq(GetLastError().into(), WAIT_TIMEOUT.into())?;
        for i in 0..3 {
            c.ok(
                PostQueuedCompletionStatus(port, 10 + i, 7 + i as usize, 0x1000 * (i as usize + 1))
                    != 0,
                0,
            )?;
        }
        c.ok(
            GetQueuedCompletionStatusEx(port, entries.as_mut_ptr(), 4, &mut removed, 1000, 0) != 0,
            0,
        )?;
        c.eq(removed.into(), 3)?;
        c.eq(entries[0].completion_key as u64, 7)?;
        c.eq(entries[0].overlapped as u64, 0x1000)?;
        c.eq(entries[0].bytes.into(), 10)?;
        c.eq(entries[2].completion_key as u64, 9)?;
        c.eq(entries[2].bytes.into(), 12)?;
        c.ok(CloseHandle(port) != 0, 0)?;

        // A waitable timer, relative due time (never early).
        let timer =
            CreateWaitableTimerExW(core::ptr::null_mut(), core::ptr::null(), 0, TIMER_ALL_ACCESS);
        c.ok(!timer.is_null(), 0)?;
        let start = GetTickCount64();
        let due = -300_000i64; // 30 ms
        c.ok(
            SetWaitableTimer(timer, &due, 0, core::ptr::null_mut(), core::ptr::null_mut(), 0) != 0,
            0,
        )?;
        c.eq(WaitForSingleObject(timer, 5000).into(), 0)?;
        let elapsed = GetTickCount64() - start;
        c.ok((10..3000).contains(&elapsed), elapsed)?;
        CloseHandle(timer);

        // Vectored handlers: a breakpoint handled, execution continues.
        c.ok(!AddVectoredExceptionHandler(1, on_exception).is_null(), 0)?;
        c.ok(!AddVectoredContinueHandler(1, on_continue).is_null(), 0)?;
        core::arch::asm!("int3");
        c.eq(BREAKPOINTS.load(Ordering::SeqCst), 1)?;

        // The unwind entry covering a function of this image.
        let image = &raw const __ImageBase as u64;
        let pc = some_function as *const () as u64;
        let mut base = 0;
        let entry = RtlLookupFunctionEntry(pc, &mut base, core::ptr::null_mut());
        c.ok(!entry.is_null(), 0)?;
        c.eq(base, image)?;
        let rva = (pc - image) as u32;
        c.ok((*entry).begin <= rva && rva < (*entry).end, (*entry).begin.into())?;
        c.eq(some_function(), 7)?;

        // FormatMessageW: system messages.
        const FROM_SYSTEM_IGNORE_INSERTS: u32 = 0x1000 | 0x200;
        let expect = |text: &[u8], got: &[u16]| {
            text.len() == got.len() && text.iter().zip(got).all(|(&a, &b)| u16::from(a) == b)
        };
        let mut buf = [0u16; 256];
        let n = FormatMessageW(
            FROM_SYSTEM_IGNORE_INSERTS,
            core::ptr::null(),
            2,
            0,
            buf.as_mut_ptr(),
            256,
            core::ptr::null_mut(),
        );
        c.ok(
            expect(b"The system cannot find the file specified.\r\n", &buf[..n as usize]),
            n.into(),
        )?;
        let n = FormatMessageW(
            FROM_SYSTEM_IGNORE_INSERTS,
            core::ptr::null(),
            10061,
            0,
            buf.as_mut_ptr(),
            256,
            core::ptr::null_mut(),
        );
        let refused =
            b"No connection could be made because the target machine actively refused it.\r\n";
        c.ok(expect(refused, &buf[..n as usize]), n.into())?;
        // A number with no message.
        let n = FormatMessageW(
            FROM_SYSTEM_IGNORE_INSERTS,
            core::ptr::null(),
            0x2fff_fff0,
            0,
            buf.as_mut_ptr(),
            256,
            core::ptr::null_mut(),
        );
        c.eq(n.into(), 0)?;
        c.eq(GetLastError().into(), 317)?; // ERROR_MR_MID_NOT_FOUND
        // Too small a buffer.
        let n = FormatMessageW(
            FROM_SYSTEM_IGNORE_INSERTS,
            core::ptr::null(),
            2,
            0,
            buf.as_mut_ptr(),
            8,
            core::ptr::null_mut(),
        );
        c.eq(n.into(), 0)?;
        c.eq(GetLastError().into(), 122)?; // ERROR_INSUFFICIENT_BUFFER
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"kernel32_misc_test: ok\r\n");
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
