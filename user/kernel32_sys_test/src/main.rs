//! More of kernel32, the process-level functions the Go runtime calls at
//! startup, with the results real Windows gives when standard output is
//! redirected (as `run-all.ps1` runs it, and as gala's serial console is):
//! console mode/WriteConsoleW fail with ERROR_INVALID_HANDLE; error modes,
//! WER flags, priority boost and console control handlers are kept;
//! GetSystemInfo, GetSystemDirectoryA, GetProcessAffinityMask,
//! GetEnvironmentStringsW, CreateEventA, LoadLibraryW/LoadLibraryExW (with
//! LOAD_LIBRARY_SEARCH_SYSTEM32), DuplicateHandle, VirtualQuery,
//! WaitForMultipleObjects and SwitchToThread. Prints `kernel32_sys_test:
//! ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[repr(C)]
#[derive(Default)]
struct SystemInfo {
    processor_architecture: u16,
    reserved: u16,
    page_size: u32,
    minimum_application_address: usize,
    maximum_application_address: usize,
    active_processor_mask: usize,
    number_of_processors: u32,
    processor_type: u32,
    allocation_granularity: u32,
    processor_level: u16,
    processor_revision: u16,
}

#[repr(C)]
#[derive(Default)]
struct MemoryBasicInformation {
    base_address: usize,
    allocation_base: usize,
    allocation_protect: u32,
    partition_id: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    kind: u32,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn GetConsoleMode(h: Handle, mode: *mut u32) -> Bool;
    fn WriteConsoleW(
        h: Handle,
        buf: *const u16,
        len: u32,
        written: *mut u32,
        reserved: *mut c_void,
    ) -> Bool;
    fn GetLastError() -> u32;
    fn SetLastError(code: u32);
    fn SetErrorMode(mode: u32) -> u32;
    fn GetErrorMode() -> u32;
    fn WerSetFlags(flags: u32) -> i32;
    fn WerGetFlags(process: Handle, flags: *mut u32) -> i32;
    fn SetProcessPriorityBoost(process: Handle, disable: Bool) -> Bool;
    fn SetConsoleCtrlHandler(handler: Option<extern "system" fn(u32) -> Bool>, add: Bool) -> Bool;
    fn GetCurrentProcess() -> Handle;
    fn GetSystemInfo(info: *mut SystemInfo);
    fn GetSystemDirectoryA(buf: *mut u8, len: u32) -> u32;
    fn GetProcessAffinityMask(
        process: Handle,
        process_mask: *mut usize,
        system_mask: *mut usize,
    ) -> Bool;
    fn GetEnvironmentStringsW() -> *mut u16;
    fn FreeEnvironmentStringsW(block: *mut u16) -> Bool;
    fn CreateEventA(attrs: *mut c_void, manual: Bool, initial: Bool, name: *const u8) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn WaitForMultipleObjects(count: u32, handles: *const Handle, all: Bool, ms: u32) -> u32;
    fn SetEvent(h: Handle) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn LoadLibraryW(name: *const u16) -> Handle;
    fn LoadLibraryExW(name: *const u16, file: Handle, flags: u32) -> Handle;
    fn FreeLibrary(module: Handle) -> Bool;
    fn DuplicateHandle(
        source_process: Handle,
        source: Handle,
        target_process: Handle,
        target: *mut Handle,
        access: u32,
        inherit: Bool,
        options: u32,
    ) -> Bool;
    fn VirtualAlloc(addr: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(addr: *mut c_void, size: usize, kind: u32) -> Bool;
    fn VirtualQuery(addr: *const c_void, info: *mut MemoryBasicInformation, len: usize) -> usize;
    fn SwitchToThread() -> Bool;
    fn GetFileType(h: Handle) -> u32;
    fn GetCommandLineW() -> *const u16;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn RtlGetVersion(info: *mut u32) -> u32;
    fn RtlGetCurrentPeb() -> usize;
}

// C helpers the compiler calls; ntdll exports them (on Windows too).
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

/// MSVC's marker that floating point is used (referenced by codegen).
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

/// Referenced by the MSVC target's unwind tables; panics abort here.
#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_MOD_NOT_FOUND: u32 = 126;
const WAIT_TIMEOUT: u32 = 258;
const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x800;

/// UTF-16 for an ASCII literal, NUL-terminated.
macro_rules! w {
    ($s:literal) => {{
        const S: &str = concat!($s, "\0");
        const N: usize = S.len();
        const W: [u16; N] = {
            let b = S.as_bytes();
            let mut w = [0u16; N];
            let mut i = 0;
            while i < N {
                w[i] = b[i] as u16;
                i += 1;
            }
            w
        };
        W.as_ptr()
    }};
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
        out(b"kernel32_sys_test: check ");
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

extern "system" fn ctrl_handler(_: u32) -> Bool {
    0
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: documented kernel32 functions with valid arguments (or
    // deliberately rejected ones).
    unsafe {
        // Redirected output is not a console.
        let stdout = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode = 0;
        SetLastError(0);
        c.eq(GetConsoleMode(stdout, &mut mode) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_HANDLE.into())?;
        let mut written = 0;
        SetLastError(0);
        c.eq(WriteConsoleW(stdout, w!("x"), 1, &mut written, core::ptr::null_mut()) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_HANDLE.into())?;

        // Process settings that are kept.
        let old = SetErrorMode(0x8003);
        c.eq(GetErrorMode().into(), 0x8003)?;
        c.eq(SetErrorMode(old).into(), 0x8003)?;
        c.eq(WerSetFlags(0x20) as u64, 0)?; // WER_FAULT_REPORTING_NO_UI
        let mut flags = 0;
        c.eq(WerGetFlags(GetCurrentProcess(), &mut flags) as u64, 0)?;
        c.ok(flags & 0x20 != 0, flags.into())?;
        c.ok(SetProcessPriorityBoost(GetCurrentProcess(), 1) != 0, 0)?;
        c.ok(SetConsoleCtrlHandler(Some(ctrl_handler), 1) != 0, 0)?;

        // System information.
        let mut info = SystemInfo::default();
        GetSystemInfo(&mut info);
        let cpus = core::ptr::read_volatile(0x7ffe_03c0 as *const u32); // KUSER ActiveProcessorCount
        c.eq(info.processor_architecture.into(), 9)?; // AMD64
        c.eq(info.page_size.into(), 4096)?;
        c.eq(info.allocation_granularity.into(), 0x10000)?;
        c.eq(info.minimum_application_address as u64, 0x10000)?;
        c.eq(info.number_of_processors.into(), cpus.into())?;
        c.eq(info.active_processor_mask.count_ones().into(), cpus.into())?;
        let mut dir = [0u8; 64];
        let n = GetSystemDirectoryA(dir.as_mut_ptr(), dir.len() as u32) as usize;
        let suffix = b":\\windows\\system32";
        let ends = n >= suffix.len() && dir[n - suffix.len()..n].eq_ignore_ascii_case(suffix);
        c.ok(n > 9 && dir[n] == 0 && ends, n as u64)?;
        let (mut process_mask, mut system_mask) = (0usize, 0usize);
        c.ok(
            GetProcessAffinityMask(GetCurrentProcess(), &mut process_mask, &mut system_mask) != 0,
            0,
        )?;
        c.eq(process_mask as u64, system_mask as u64)?;
        c.eq(process_mask.count_ones().into(), cpus.into())?;

        // The environment block: NAME=value strings, an empty one ends it.
        let env = GetEnvironmentStringsW();
        c.ok(!env.is_null(), 0)?;
        let (mut i, mut found, mut strings) = (0, false, 0);
        loop {
            let start = i;
            while *env.add(i) != 0 {
                i += 1;
            }
            if i == start {
                break;
            }
            strings += 1;
            let s = core::slice::from_raw_parts(env.add(start), i - start);
            let key = b"systemroot=";
            if s.len() > key.len()
                && s[..key.len()]
                    .iter()
                    .zip(key)
                    .all(|(&a, &b)| (a as u8).to_ascii_lowercase() == b)
            {
                found = true;
            }
            i += 1;
        }
        c.ok(found && strings > 1, strings)?;
        c.ok(FreeEnvironmentStringsW(env) != 0, 0)?;

        // CreateEventA: an auto-reset event, initially signaled.
        let e = CreateEventA(core::ptr::null_mut(), 0, 1, core::ptr::null());
        c.ok(!e.is_null(), 0)?;
        c.eq(WaitForSingleObject(e, 0).into(), 0)?;
        c.eq(WaitForSingleObject(e, 0).into(), WAIT_TIMEOUT.into())?;

        // WaitForMultipleObjects: any (index of the signaled one), all.
        let e2 = CreateEventA(core::ptr::null_mut(), 1, 0, core::ptr::null());
        let both = [e, e2];
        c.eq(WaitForMultipleObjects(2, both.as_ptr(), 0, 0).into(), WAIT_TIMEOUT.into())?;
        SetEvent(e2);
        c.eq(WaitForMultipleObjects(2, both.as_ptr(), 0, 0).into(), 1)?;
        c.eq(WaitForMultipleObjects(2, both.as_ptr(), 1, 0).into(), WAIT_TIMEOUT.into())?;
        SetEvent(e);
        c.eq(WaitForMultipleObjects(2, both.as_ptr(), 1, 0).into(), 0)?;

        // DuplicateHandle (DUPLICATE_SAME_ACCESS): a second, working handle.
        let mut dup = core::ptr::null_mut();
        c.ok(
            DuplicateHandle(GetCurrentProcess(), e2, GetCurrentProcess(), &mut dup, 0, 0, 2) != 0,
            0,
        )?;
        c.ok(!dup.is_null() && dup != e2, dup as u64)?;
        c.eq(WaitForSingleObject(dup, 0).into(), 0)?; // e2 is manual-reset, still set
        for h in [e, e2, dup] {
            c.ok(CloseHandle(h) != 0, h as u64)?;
        }

        // LoadLibraryW, and LoadLibraryExW searching System32 only.
        let dll = LoadLibraryW(w!("testdll.dll"));
        c.ok(!dll.is_null(), 0)?;
        c.ok(FreeLibrary(dll) != 0, 0)?;
        let k32 =
            LoadLibraryExW(w!("kernel32.dll"), core::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
        c.ok(!k32.is_null(), 0)?;
        SetLastError(0);
        c.ok(
            LoadLibraryExW(w!("testdll.dll"), core::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32)
                .is_null(),
            0,
        )?;
        c.eq(GetLastError().into(), ERROR_MOD_NOT_FOUND.into())?;

        // VirtualQuery.
        let p = VirtualAlloc(core::ptr::null_mut(), 0x10000, 0x3000, 4);
        let mut mbi = MemoryBasicInformation::default();
        let size = core::mem::size_of::<MemoryBasicInformation>();
        c.eq(VirtualQuery(p, &mut mbi, size) as u64, size as u64)?;
        c.eq(mbi.base_address as u64, p as u64)?;
        c.eq(mbi.region_size as u64, 0x10000)?;
        c.eq(mbi.state.into(), 0x1000)?; // MEM_COMMIT
        c.eq(mbi.protect.into(), 4)?;
        VirtualFree(p, 0, 0x8000);

        SwitchToThread();

        // GetCommandLineW: the PEB's command line, NUL-terminated, naming
        // this program.
        let line = GetCommandLineW();
        let peb: usize;
        core::arch::asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly));
        let params = *((peb + 0x20) as *const usize);
        c.eq(line as u64, *((params + 0x78) as *const u64))?;
        let len = usize::from(*((params + 0x70) as *const u16)) / 2;
        c.eq((*line.add(len)).into(), 0)?;
        let text = core::slice::from_raw_parts(line, len);
        let name = b"kernel32_sys_test";
        c.ok(
            text.windows(name.len())
                .any(|w| w.iter().zip(name).all(|(&a, &b)| (a as u8).to_ascii_lowercase() == b)),
            len as u64,
        )?;

        // GetFileType: standard output is a character device, pipe or file
        // depending on where it goes (gala: the console, a character
        // device); a bad handle is FILE_TYPE_UNKNOWN with
        // ERROR_INVALID_HANDLE.
        let kind = GetFileType(stdout);
        c.ok((1..=3).contains(&kind), kind.into())?;
        SetLastError(0);
        c.eq(GetFileType(0x1234 as Handle).into(), 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_HANDLE.into())?;

        // ntdll: RtlGetVersion (OSVERSIONINFOW: size, major, minor, build,
        // platform, CSD string) agrees with KUSER_SHARED_DATA; the PEB.
        let mut version = [0u32; 69]; // 276 bytes
        version[0] = 276;
        c.eq(RtlGetVersion(version.as_mut_ptr()).into(), 0)?;
        c.eq(version[1].into(), 10)?;
        c.eq(version[2].into(), 0)?;
        c.eq(version[3].into(), core::ptr::read_volatile(0x7ffe_0260 as *const u32).into())?;
        c.eq(version[4].into(), 2)?; // VER_PLATFORM_WIN32_NT
        let peb: usize;
        core::arch::asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly));
        c.eq(RtlGetCurrentPeb() as u64, peb as u64)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"kernel32_sys_test: ok\r\n");
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
