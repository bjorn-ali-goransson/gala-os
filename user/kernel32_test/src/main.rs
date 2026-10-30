//! kernel32.dll basics, as a normal Windows program uses them (imported,
//! bound by the loader): console output, last-error, process and thread
//! ids, virtual memory, events and waits, sleeping and tick counts, the
//! performance counter, LoadLibrary/GetProcAddress/FreeLibrary, and
//! ExitProcess. Expected results, last-error codes included, are those of
//! real Windows (its kernel32); on gala the test runs against gala's own
//! kernel32.dll. Prints `kernel32_test: ok` and ends with ExitProcess(0),
//! or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn GetLastError() -> u32;
    fn SetLastError(code: u32);
    fn GetCurrentProcessId() -> u32;
    fn GetCurrentThreadId() -> u32;
    fn VirtualAlloc(addr: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(addr: *mut c_void, size: usize, kind: u32) -> Bool;
    fn CreateEventW(attrs: *mut c_void, manual: Bool, initial: Bool, name: *const u16) -> Handle;
    fn SetEvent(h: Handle) -> Bool;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn CloseHandle(h: Handle) -> Bool;
    fn Sleep(ms: u32);
    fn GetTickCount64() -> u64;
    fn QueryPerformanceCounter(count: *mut i64) -> Bool;
    fn QueryPerformanceFrequency(freq: *mut i64) -> Bool;
    fn LoadLibraryA(name: *const u8) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    fn FreeLibrary(module: Handle) -> Bool;
    fn ExitProcess(code: u32) -> !;
}

const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
const MEM_COMMIT: u32 = 0x1000;
const MEM_RESERVE: u32 = 0x2000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_READWRITE: u32 = 0x04;
const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_MOD_NOT_FOUND: u32 = 126;
const ERROR_PROC_NOT_FOUND: u32 = 127;
const ERROR_INVALID_ADDRESS: u32 = 487;

fn teb_u32(offset: usize) -> u32 {
    let v: u64;
    // SAFETY: gs:[offset] reads the current TEB (x64 Windows layout).
    unsafe {
        core::arch::asm!("mov {}, gs:[{}]", out(reg) v, in(reg) offset, options(nostack, readonly))
    };
    v as u32
}

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
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
        out(b"kernel32_test: check ");
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

type Add = extern "C" fn(u64, u64) -> u64;

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: plain calls of documented kernel32 functions with valid
    // arguments (or deliberately invalid handles/names, which they reject).
    unsafe {
        // Console output and its byte count.
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        c.ok(!h.is_null(), 0)?;
        let text = b"kernel32_test: hello from kernel32\r\n";
        let mut written = 0;
        c.eq(
            WriteFile(h, text.as_ptr(), text.len() as u32, &mut written, core::ptr::null_mut())
                as u64,
            1,
        )?;
        c.eq(written.into(), text.len() as u64)?;

        // Last error lives in the TEB.
        SetLastError(1234);
        c.eq(GetLastError().into(), 1234)?;
        c.eq(teb_u32(0x68).into(), 1234)?;

        // Ids come from the TEB's ClientId.
        c.eq(GetCurrentProcessId().into(), teb_u32(0x40).into())?;
        c.eq(GetCurrentThreadId().into(), teb_u32(0x48).into())?;
        c.ok(GetCurrentProcessId() != 0 && GetCurrentThreadId() != 0, 0)?;

        // Virtual memory.
        let p =
            VirtualAlloc(core::ptr::null_mut(), 0x10000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
        c.ok(!p.is_null(), 0)?;
        (p as *mut u64).write_volatile(0x1234);
        c.eq((p as *const u64).read_volatile(), 0x1234)?;
        c.eq(VirtualFree(p, 0, MEM_RELEASE) as u64, 1)?;
        SetLastError(0);
        c.eq(VirtualFree(p, 0, MEM_RELEASE) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_ADDRESS.into())?;

        // Events and waits.
        let e = CreateEventW(core::ptr::null_mut(), 1, 0, core::ptr::null());
        c.ok(!e.is_null(), 0)?;
        c.eq(WaitForSingleObject(e, 0).into(), WAIT_TIMEOUT.into())?;
        c.eq(SetEvent(e) as u64, 1)?;
        c.eq(WaitForSingleObject(e, 1000).into(), WAIT_OBJECT_0.into())?;
        c.eq(CloseHandle(e) as u64, 1)?;
        SetLastError(0);
        c.eq(CloseHandle(0x1234 as Handle) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_HANDLE.into())?;

        // Time.
        let t0 = GetTickCount64();
        Sleep(50);
        let t1 = GetTickCount64();
        c.ok(t1 >= t0 + 30 && t1 < t0 + 5000, t1 - t0)?;
        let (mut f, mut q0, mut q1) = (0i64, 0i64, 0i64);
        c.eq(QueryPerformanceFrequency(&mut f) as u64, 1)?;
        c.ok(f > 0, f as u64)?;
        c.eq(QueryPerformanceCounter(&mut q0) as u64, 1)?;
        Sleep(1);
        c.eq(QueryPerformanceCounter(&mut q1) as u64, 1)?;
        c.ok(q1 > q0, (q1 - q0) as u64)?;

        // Dynamic loading.
        let dll = LoadLibraryA(c"testdll.dll".as_ptr().cast());
        c.ok(!dll.is_null(), 0)?;
        let add = GetProcAddress(dll, c"gala_add".as_ptr().cast());
        c.ok(!add.is_null(), 0)?;
        c.eq(core::mem::transmute::<*mut c_void, Add>(add)(40, 2), 42)?;
        SetLastError(0);
        c.ok(GetProcAddress(dll, c"no_such_function".as_ptr().cast()).is_null(), 0)?;
        c.eq(GetLastError().into(), ERROR_PROC_NOT_FOUND.into())?;
        c.eq(FreeLibrary(dll) as u64, 1)?;
        SetLastError(0);
        c.ok(LoadLibraryA(c"no_such_dll.dll".as_ptr().cast()).is_null(), 0)?;
        c.eq(GetLastError().into(), ERROR_MOD_NOT_FOUND.into())?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"kernel32_test: ok\r\n");
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
