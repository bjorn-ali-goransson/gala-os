//! The rest of what the Go runtime imports from kernel32, with the results
//! real Windows gives: I/O completion ports (CreateIoCompletionPort,
//! PostQueuedCompletionStatus, GetQueuedCompletionStatusEx removing
//! several entries, WAIT_TIMEOUT when empty), waitable timers
//! (CreateWaitableTimerExW/A/W, SetWaitableTimer with a relative due time),
//! vectored exception and continue handlers (AddVectoredExceptionHandler
//! sees a breakpoint and continues past it, and RaiseException's record),
//! RtlPcToFileHeader, and RtlLookupFunctionEntry
//! (the unwind entry of a function in this image), and FormatMessageW's
//! system messages (English text with a CRLF; ERROR_MR_MID_NOT_FOUND for
//! a number without one; ERROR_INSUFFICIENT_BUFFER); heaps (GetProcessHeap
//! is the PEB's ProcessHeap; HeapAlloc with HEAP_ZERO_MEMORY, HeapSize as
//! asked, HeapReAlloc keeping the contents and zeroing the growth,
//! HEAP_REALLOC_IN_PLACE_ONLY failing, a private heap from HeapCreate,
//! HeapValidate, HeapDestroy); GetDriveTypeW, AreFileApisANSI; winmm's
//! timeBeginPeriod/timeEndPeriod/timeGetTime; ntdll's
//! RtlGetNtVersionNumbers (10.0, build 26100 with the high bits); advapi32
//! RegOpenKeyExW/A of a key that does not exist, RegCloseKey of a
//! predefined key; GetComputerNameW/ExW; ncrypt's NCryptFreeObject of a
//! null handle. Prints
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
    fn GetProcessHeap() -> Handle;
    fn HeapCreate(options: u32, initial: usize, maximum: usize) -> Handle;
    fn HeapDestroy(heap: Handle) -> Bool;
    fn HeapAlloc(heap: Handle, flags: u32, bytes: usize) -> *mut u8;
    fn HeapReAlloc(heap: Handle, flags: u32, p: *mut u8, bytes: usize) -> *mut u8;
    fn HeapFree(heap: Handle, flags: u32, p: *mut u8) -> Bool;
    fn HeapSize(heap: Handle, flags: u32, p: *const u8) -> usize;
    fn HeapValidate(heap: Handle, flags: u32, p: *const u8) -> Bool;
    fn GetDriveTypeW(root: *const u16) -> u32;
    fn GetComputerNameExW(kind: u32, buf: *mut u16, size: *mut u32) -> Bool;
    fn GetComputerNameW(buf: *mut u16, size: *mut u32) -> Bool;
    fn AreFileApisANSI() -> Bool;
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
    fn CreateWaitableTimerA(attrs: *mut c_void, manual: Bool, name: *const u8) -> Handle;
    fn CreateWaitableTimerW(attrs: *mut c_void, manual: Bool, name: *const u16) -> Handle;
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
    fn RaiseException(code: u32, flags: u32, count: u32, args: *const usize);
    fn RtlPcToFileHeader(pc: u64, base: *mut u64) -> u64;
    fn GetModuleHandleW(name: *const u16) -> u64;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetTickCount64() -> u64;
    fn GetModuleHandleA(name: *const u8) -> Handle;
    fn GetVersionExW(info: *mut u32) -> Bool;
    fn GetVersion() -> u32;
    fn FreeLibrary(module: Handle) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegOpenKeyExW(
        key: usize,
        subkey: *const u16,
        options: u32,
        access: u32,
        out: *mut usize,
    ) -> i32;
    fn RegOpenKeyExA(
        key: usize,
        subkey: *const u8,
        options: u32,
        access: u32,
        out: *mut usize,
    ) -> i32;
    fn RegCloseKey(key: usize) -> i32;
}

#[link(name = "ncrypt", kind = "raw-dylib")]
unsafe extern "system" {
    fn NCryptFreeObject(object: usize) -> i32;
}

#[link(name = "winmm", kind = "raw-dylib")]
unsafe extern "system" {
    fn timeBeginPeriod(period: u32) -> u32;
    fn timeEndPeriod(period: u32) -> u32;
    fn timeGetTime() -> u32;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn RtlGetNtVersionNumbers(major: *mut u32, minor: *mut u32, build: *mut u32);
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
/// The code RaiseException raises in this test.
const CODE_RAISED: u32 = 0xe000_0123;
/// What the handler saw of it: flags, ExceptionAddress, parameter count,
/// the first and the last parameter.
static RAISED: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

/// Continues past breakpoints (rip points at the int3) and after
/// CODE_RAISED (RaiseException returns).
extern "system" fn on_exception(pointers: *mut ExceptionPointers) -> i32 {
    // SAFETY: the dispatcher passes valid EXCEPTION_POINTERS.
    unsafe {
        let record = (*pointers).record;
        if *record == CODE_RAISED {
            let r = record as *const u8;
            let rd = |off: usize| (r.add(off) as *const u64).read_unaligned();
            let count = rd(0x18) & 0xffff_ffff;
            let values = [
                rd(0x04) & 0xffff_ffff,
                rd(0x10),
                count,
                rd(0x20),
                rd(0x20 + 8 * (count as usize).saturating_sub(1)),
            ];
            for (slot, v) in RAISED.iter().zip(values) {
                slot.store(v, Ordering::SeqCst);
            }
            return EXCEPTION_CONTINUE_EXECUTION;
        }
        if *record != STATUS_BREAKPOINT {
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

const HEAP_ZERO_MEMORY: u32 = 0x8;
const HEAP_REALLOC_IN_PLACE_ONLY: u32 = 0x10;
const DRIVE_FIXED: u32 = 3;
const DRIVE_NO_ROOT_DIR: u32 = 1;

/// Heaps: the process heap and a private one.
fn heaps(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: documented kernel32 calls; the blocks are used within their
    // sizes.
    unsafe {
        let process = GetProcessHeap();
        let peb: usize;
        core::arch::asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly));
        c.ok(!process.is_null(), 0)?;
        c.eq(process as u64, *((peb + 0x30) as *const u64))?; // PEB.ProcessHeap
        let p = HeapAlloc(process, HEAP_ZERO_MEMORY, 100);
        c.ok(!p.is_null() && (p as usize).is_multiple_of(16), p as u64)?;
        c.ok((0..100).all(|i| *p.add(i) == 0), 0)?;
        c.eq(HeapSize(process, 0, p) as u64, 100)?;
        for i in 0..100 {
            *p.add(i) = i as u8;
        }
        let q = HeapReAlloc(process, HEAP_ZERO_MEMORY, p, 5000);
        c.ok(!q.is_null(), 0)?;
        c.eq(HeapSize(process, 0, q) as u64, 5000)?;
        c.ok((0..100).all(|i| *q.add(i) == i as u8) && (100..5000).all(|i| *q.add(i) == 0), 0)?;
        c.ok(HeapReAlloc(process, HEAP_REALLOC_IN_PLACE_ONLY, q, 1 << 20).is_null(), 0)?;
        c.ok(HeapValidate(process, 0, q) != 0, 0)?;
        c.ok(HeapFree(process, 0, q) != 0, 0)?;
        c.ok(HeapFree(process, 0, core::ptr::null_mut()) != 0, 0)?;

        let private = HeapCreate(0, 0, 0);
        c.ok(!private.is_null() && private != process, private as u64)?;
        let mut blocks = [core::ptr::null_mut(); 64];
        for (i, b) in blocks.iter_mut().enumerate() {
            *b = HeapAlloc(private, 0, 16 << (i % 12)); // up to 32 KiB
            c.ok(!b.is_null(), i as u64)?;
            **b = i as u8;
        }
        let big = HeapAlloc(private, 0, 3 << 20);
        c.ok(!big.is_null(), 0)?;
        *big.add((3 << 20) - 1) = 7;
        c.eq(HeapSize(private, 0, big) as u64, 3 << 20)?;
        c.ok(blocks.iter().enumerate().all(|(i, &b)| *b == i as u8), 0)?;
        c.ok(HeapValidate(private, 0, core::ptr::null()) != 0, 0)?;
        c.ok(HeapDestroy(private) != 0, GetLastError().into())?;

        let c_root: [u16; 4] = [b'C' as u16, b':' as u16, b'\\' as u16, 0];
        let q_root: [u16; 4] = [b'Q' as u16, b':' as u16, b'\\' as u16, 0];
        c.eq(GetDriveTypeW(c_root.as_ptr()).into(), DRIVE_FIXED.into())?;
        c.eq(GetDriveTypeW(q_root.as_ptr()).into(), DRIVE_NO_ROOT_DIR.into())?;
        c.ok(AreFileApisANSI() != 0, 0)?;

        // winmm: timer resolution requests and the millisecond clock.
        const TIMERR_NOCANDO: u32 = 97;
        c.eq(timeBeginPeriod(1).into(), 0)?;
        c.eq(timeEndPeriod(1).into(), 0)?;
        c.eq(timeBeginPeriod(0).into(), TIMERR_NOCANDO.into())?;
        let (t1, ticks) = (timeGetTime(), GetTickCount64() as u32);
        let t2 = timeGetTime();
        c.ok(t2.wrapping_sub(t1) < 1000 && t1.abs_diff(ticks) < 1000, u64::from(t1))?;
        // Without a compatibility manifest: version 6.2, build 9200 (as
        // Windows answers programs that do not declare Windows 10).
        let mut v = [0u32; 71];
        v[0] = 284;
        c.ok(GetVersionExW(v.as_mut_ptr()) != 0, 0)?;
        c.ok(v[1] == 6 && v[2] == 2 && v[3] == 9200, v[1].into())?;
        c.eq(GetVersion().into(), (9200 << 16) | (2 << 8) | 6)?;

        // A DLL loaded at startup as a static import stays for the
        // process's lifetime: FreeLibrary on its handle does not unload it.
        let winmm = GetModuleHandleA(c"winmm.dll".as_ptr().cast());
        c.ok(!winmm.is_null(), 0)?;
        FreeLibrary(winmm);
        FreeLibrary(winmm);
        c.eq(GetModuleHandleA(c"winmm.dll".as_ptr().cast()) as u64, winmm as u64)?;
        c.ok(timeGetTime().wrapping_sub(t2) < 1000, 0)?;

        // The computer's names: the DNS host name (what Go's os.Hostname
        // asks for, ComputerNamePhysicalDnsHostname) is the NetBIOS name up
        // to case; too small a buffer gets the size needed.
        let (mut dns, mut netbios) = ([0u16; 64], [0u16; 64]);
        let (mut n_dns, mut n_bios) = (64u32, 64u32);
        c.ok(GetComputerNameExW(5, dns.as_mut_ptr(), &mut n_dns) != 0, GetLastError().into())?;
        c.ok(GetComputerNameW(netbios.as_mut_ptr(), &mut n_bios) != 0, GetLastError().into())?;
        c.ok(n_dns > 0 && n_bios > 0 && n_bios <= 15, u64::from(n_dns))?;
        let upper = |c: u16| if (97..=122).contains(&c) { c - 32 } else { c };
        let same = dns[..n_bios as usize].iter().zip(&netbios).all(|(&a, &b)| upper(a) == b);
        c.ok(same && dns[n_dns as usize] == 0, 0)?;
        let mut small = 1u32;
        c.ok(GetComputerNameExW(5, dns.as_mut_ptr(), &mut small) == 0, 0)?;
        c.eq(GetLastError().into(), 234)?; // ERROR_MORE_DATA
        c.eq(small.into(), u64::from(n_dns) + 1)?;

        // ncrypt: no object behind a null handle (NTE_INVALID_HANDLE).
        c.eq(NCryptFreeObject(0) as u32 as u64, 0x8009_0026)?;

        // The registry: a key that does not exist, closing a predefined key.
        const HKEY_LOCAL_MACHINE: usize = 0x8000_0002;
        const KEY_READ: u32 = 0x2_0019;
        let name: [u16; 28] = (*b"SOFTWARE\\gala-os-no-such-ke\0").map(u16::from);
        let mut key = 0usize;
        c.eq(RegOpenKeyExW(HKEY_LOCAL_MACHINE, name.as_ptr(), 0, KEY_READ, &mut key) as u64, 2)?;
        let name_a = b"SOFTWARE\\gala-os-no-such-key\0";
        c.eq(RegOpenKeyExA(HKEY_LOCAL_MACHINE, name_a.as_ptr(), 0, KEY_READ, &mut key) as u64, 2)?;
        c.eq(RegCloseKey(HKEY_LOCAL_MACHINE) as u64, 0)?;

        // ntdll: the version numbers, build with 0xF0000000 (a free build).
        let (mut major, mut minor, mut build) = (0, 0, 0);
        RtlGetNtVersionNumbers(&mut major, &mut minor, &mut build);
        c.ok(major == 10 && minor == 0 && build == 0xf000_0000 | 26100, u64::from(build))?;
    }
    Ok(())
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
        // CreateWaitableTimerA/W: a manual-reset timer stays signaled.
        let timer = CreateWaitableTimerA(core::ptr::null_mut(), 1, core::ptr::null());
        c.ok(!timer.is_null(), GetLastError().into())?;
        let due = -10_000i64; // 1 ms
        c.ok(
            SetWaitableTimer(timer, &due, 0, core::ptr::null_mut(), core::ptr::null_mut(), 0) != 0,
            0,
        )?;
        c.eq(WaitForSingleObject(timer, 5000).into(), 0)?;
        c.eq(WaitForSingleObject(timer, 0).into(), 0)?; // still signaled
        CloseHandle(timer);
        let timer = CreateWaitableTimerW(core::ptr::null_mut(), 0, core::ptr::null());
        c.ok(!timer.is_null(), GetLastError().into())?;
        CloseHandle(timer);

        // Vectored handlers: a breakpoint handled, execution continues.
        c.ok(!AddVectoredExceptionHandler(1, on_exception).is_null(), 0)?;
        c.ok(!AddVectoredContinueHandler(1, on_continue).is_null(), 0)?;
        core::arch::asm!("int3");
        c.eq(BREAKPOINTS.load(Ordering::SeqCst), 1)?;

        // RaiseException: the record as given (more than
        // EXCEPTION_MAXIMUM_PARAMETERS = 15 parameters are cut to 15), the
        // address inside kernel32 (where it raised), continued.
        let args: [usize; 17] = core::array::from_fn(|i| 100 + i);
        RaiseException(CODE_RAISED, 0, 17, args.as_ptr());
        let raised = |i: usize| RAISED[i].load(Ordering::SeqCst);
        c.eq(raised(0), 0)?; // flags
        c.eq(raised(2), 15)?;
        c.eq(raised(3), 100)?;
        c.eq(raised(4), 114)?;
        let mut name = [0u16; 13];
        for (d, &b) in name.iter_mut().zip(b"kernel32.dll") {
            *d = b.into();
        }
        let kernel32 = GetModuleHandleW(name.as_ptr());
        let mut base = 0;
        c.eq(RtlPcToFileHeader(raised(1), &mut base), kernel32)?;
        c.eq(base, kernel32)?;

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
    heaps(&mut c)?;
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
