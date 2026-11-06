//! A plain 32-bit (x86) program on kernel32 (ROADMAP WOW stage 3): what
//! such a program calls first, with the results of 64-bit Windows' WOW64.
//!
//! - GetStdHandle and WriteFile to the console (the bytes written).
//! - CreateFileW (a new file in C:\Users\Public), WriteFile, SetFilePointer,
//!   ReadFile, CloseHandle, DeleteFileW; a missing file:
//!   INVALID_HANDLE_VALUE and ERROR_FILE_NOT_FOUND.
//! - CreateThread: the thread gets its parameter; WaitForSingleObject
//!   returns WAIT_OBJECT_0; GetExitCodeThread its result.
//! - What a 32-bit process is told: IsWow64Process TRUE; GetSystemInfo
//!   PROCESSOR_ARCHITECTURE_INTEL and the 4 GiB range of a
//!   large-address-aware image, GetNativeSystemInfo AMD64;
//!   GetSystemDirectoryW `C:\Windows\SysWOW64`; PROCESSOR_ARCHITECTURE x86;
//!   Wow64DisableWow64FsRedirection and Wow64RevertWow64FsRedirection
//!   succeed.
//! - It ends with ExitProcess(0).
//!
//! Prints `wow64_k32_test: ok` and exits with 0, or exits with the number
//! of the failing check (`(line 0x...) wow64_k32_test: check N failed`
//! first).

#![no_std]
#![no_main]

type Handle = usize;
const INVALID_HANDLE_VALUE: Handle = usize::MAX;

#[link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: usize,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn SetFilePointer(h: Handle, distance: i32, high: *mut i32, method: u32) -> u32;
    fn CloseHandle(h: Handle) -> i32;
    fn DeleteFileW(name: *const u16) -> i32;
    fn GetLastError() -> u32;
    fn CreateThread(
        security: usize,
        stack: usize,
        start: extern "system" fn(usize) -> u32,
        param: usize,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeThread(h: Handle, code: *mut u32) -> i32;
    fn GetCurrentProcess() -> Handle;
    fn IsWow64Process(process: Handle, wow64: *mut i32) -> i32;
    fn GetSystemInfo(info: *mut [u32; 9]);
    fn GetNativeSystemInfo(info: *mut [u32; 9]);
    fn GetSystemDirectoryW(buf: *mut u16, len: u32) -> u32;
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, len: u32) -> u32;
    fn Wow64DisableWow64FsRedirection(old: *mut usize) -> i32;
    fn Wow64RevertWow64FsRedirection(old: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
}

// The memory routines the compiler calls: ntrt's x86 runtime (nothing
// else of it is used).
use ntrt as _;

const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const INFINITE: u32 = u32::MAX;

/// A NUL-terminated UTF-16 copy of ASCII `s`.
fn wide(s: &str) -> [u16; 64] {
    let mut out = [0u16; 64];
    for (d, b) in out.iter_mut().zip(s.bytes()) {
        *d = u16::from(b);
    }
    out
}

fn print(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid arguments.
    unsafe {
        WriteFile(
            GetStdHandle(STD_OUTPUT_HANDLE),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            0,
        )
    };
}

fn hex32(v: u32) -> [u8; 8] {
    let mut out = [0u8; 8];
    for (i, d) in out.iter_mut().enumerate() {
        *d = b"0123456789abcdef"[((v >> (28 - 4 * i)) & 0xf) as usize];
    }
    out
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn eq(&mut self, got: u32, want: u32) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// A failure names its check, the source line and what was got.
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u32) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        let mut at = *b"(line 0x00000000) ";
        at[8..16].copy_from_slice(&hex32(core::panic::Location::caller().line()));
        print(&at);
        let mut line = *b"wow64_k32_test: check 00000000 failed, got 00000000\r\n";
        line[22..30].copy_from_slice(&hex32(self.0));
        line[43..51].copy_from_slice(&hex32(got));
        print(&line);
        Err(self.0)
    }
}

static mut SEEN: usize = 0;

extern "system" fn thread(param: usize) -> u32 {
    // SAFETY: written once by this thread, read after the wait.
    unsafe { SEEN = param };
    param as u32 + 1
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: valid arguments throughout.
    unsafe {
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        c.ok(out != 0 && out != INVALID_HANDLE_VALUE, out as u32)?;
        let hello = b"wow64_k32_test: hello\r\n";
        let mut n = 0;
        c.eq(WriteFile(out, hello.as_ptr(), hello.len() as u32, &mut n, 0) as u32, 1)?;
        c.eq(n, hello.len() as u32)?;

        // A file.
        let path = wide("C:\\Users\\Public\\wow64_k32_test.txt");
        let f = CreateFileW(
            path.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            0,
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            0,
        );
        c.ok(f != INVALID_HANDLE_VALUE, GetLastError())?;
        c.eq(WriteFile(f, b"gala!".as_ptr(), 5, &mut n, 0) as u32, 1)?;
        c.eq(n, 5)?;
        c.eq(SetFilePointer(f, 0, core::ptr::null_mut(), 0), 0)?;
        let mut back = [0u8; 8];
        c.eq(ReadFile(f, back.as_mut_ptr(), 8, &mut n, 0) as u32, 1)?;
        c.eq(n, 5)?;
        c.ok(&back[..5] == b"gala!", u32::from(back[0]))?;
        c.eq(CloseHandle(f) as u32, 1)?;
        c.eq(DeleteFileW(path.as_ptr()) as u32, 1)?;
        let f = CreateFileW(path.as_ptr(), GENERIC_READ, 0, 0, OPEN_EXISTING, 0, 0);
        c.eq(f as u32, INVALID_HANDLE_VALUE as u32)?;
        c.eq(GetLastError(), ERROR_FILE_NOT_FOUND)?;

        // A thread.
        let mut id = 0;
        let t = CreateThread(0, 0, thread, 0x41, 0, &mut id);
        c.ok(t != 0, GetLastError())?;
        c.ok(id != 0, id)?;
        c.eq(WaitForSingleObject(t, INFINITE), 0)?;
        let mut code = 0;
        c.eq(GetExitCodeThread(t, &mut code) as u32, 1)?;
        c.eq(code, 0x42)?;
        c.eq(core::ptr::read_volatile(&raw const SEEN) as u32, 0x41)?;
        c.eq(CloseHandle(t) as u32, 1)?;

        // What a 32-bit process is told.
        let mut wow64 = 0;
        c.eq(IsWow64Process(GetCurrentProcess(), &mut wow64) as u32, 1)?;
        c.eq(wow64 as u32, 1)?;
        let mut info = [0u32; 9];
        GetSystemInfo(&mut info);
        c.eq(info[0] & 0xffff, 0)?; // PROCESSOR_ARCHITECTURE_INTEL
        c.eq(info[3], 0xfffe_ffff)?; // lpMaximumApplicationAddress
        GetNativeSystemInfo(&mut info);
        c.eq(info[0] & 0xffff, 9)?; // PROCESSOR_ARCHITECTURE_AMD64
        let mut dir = [0u16; 64];
        let len = GetSystemDirectoryW(dir.as_mut_ptr(), 64);
        let want = wide("C:\\Windows\\SysWOW64");
        c.eq(len, 19)?;
        c.ok(dir[..19].iter().zip(&want).all(|(a, b)| (*a | 0x20) == (*b | 0x20)), dir[11].into())?;
        let mut arch = [0u16; 16];
        let name = wide("PROCESSOR_ARCHITECTURE");
        c.eq(GetEnvironmentVariableW(name.as_ptr(), arch.as_mut_ptr(), 16), 3)?;
        c.ok(arch[..3] == wide("x86")[..3], arch[0].into())?;
        let mut old = 0;
        c.eq(Wow64DisableWow64FsRedirection(&mut old) as u32, 1)?;
        c.eq(Wow64RevertWow64FsRedirection(old) as u32, 1)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            print(b"wow64_k32_test: ok\r\n");
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
