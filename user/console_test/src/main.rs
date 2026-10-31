//! The console of a process whose standard output is redirected (gala's
//! standard handles are the serial line, like redirected output): the
//! console itself is reachable as CONOUT$ and CONIN$, with the results
//! real Windows gives (documented contracts):
//!
//! - CreateFileW("CONOUT$"/"CONIN$"): console handles (FILE_TYPE_CHAR,
//!   GetConsoleMode succeeds: output modes include ENABLE_PROCESSED_OUTPUT
//!   and ENABLE_WRAP_AT_EOL_OUTPUT, input modes ENABLE_PROCESSED_INPUT,
//!   ENABLE_LINE_INPUT and ENABLE_ECHO_INPUT).
//! - SetConsoleMode (ENABLE_VIRTUAL_TERMINAL_PROCESSING accepted; unknown
//!   bits are ERROR_INVALID_PARAMETER).
//! - GetConsoleScreenBufferInfo(Ex): a window inside the buffer; the cursor
//!   advances by what WriteConsoleW writes; SetConsoleCursorPosition and
//!   SetConsoleTextAttribute show in it.
//! - WriteConsoleW of a non-console handle: ERROR_INVALID_HANDLE.
//! - GetNumberOfConsoleInputEvents of CONIN$ succeeds.
//!
//! Prints `console_test: ok` or exits with the number of the failing
//! check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetLastError() -> u32;
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn GetFileType(h: Handle) -> u32;
    fn GetConsoleMode(h: Handle, mode: *mut u32) -> Bool;
    fn SetConsoleMode(h: Handle, mode: u32) -> Bool;
    fn GetConsoleScreenBufferInfo(h: Handle, info: *mut ScreenBufferInfo) -> Bool;
    fn GetConsoleScreenBufferInfoEx(h: Handle, info: *mut u8) -> Bool;
    fn WriteConsoleW(
        h: Handle,
        buf: *const u16,
        len: u32,
        written: *mut u32,
        reserved: *mut c_void,
    ) -> Bool;
    fn SetConsoleCursorPosition(h: Handle, at: u32) -> Bool;
    fn SetConsoleTextAttribute(h: Handle, attributes: u16) -> Bool;
    fn GetNumberOfConsoleInputEvents(h: Handle, count: *mut u32) -> Bool;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn __chkstk();
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

/// CONSOLE_SCREEN_BUFFER_INFO.
#[repr(C)]
#[derive(Default)]
struct ScreenBufferInfo {
    size: [i16; 2],
    cursor: [i16; 2],
    attributes: u16,
    window: [i16; 4], // left, top, right, bottom
    maximum_window: [i16; 2],
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
const ERROR_INVALID_HANDLE: u64 = 6;
const ERROR_INVALID_PARAMETER: u64 = 87;

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
        out(b"console_test: check ");
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

fn w(s: &str) -> [u16; 32] {
    let mut b = [0u16; 32];
    for (d, ch) in b.iter_mut().zip(s.bytes()) {
        *d = ch.into();
    }
    b
}

fn open(name: &str) -> Handle {
    let n = w(name);
    // SAFETY: a NUL-terminated name; FILE_SHARE_READ | FILE_SHARE_WRITE.
    unsafe {
        CreateFileW(
            n.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            3,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        )
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: console calls with handles and buffers of ours.
    unsafe {
        let con = open("CONOUT$");
        c.ok(con as isize != -1, GetLastError().into())?;
        c.eq(GetFileType(con).into(), 2)?; // FILE_TYPE_CHAR
        let mut mode = 0u32;
        c.ok(GetConsoleMode(con, &mut mode) != 0, GetLastError().into())?;
        c.eq((mode & 3).into(), 3)?; // PROCESSED_OUTPUT | WRAP_AT_EOL_OUTPUT
        c.ok(SetConsoleMode(con, 7) != 0, GetLastError().into())?; // + VIRTUAL_TERMINAL_PROCESSING
        c.ok(GetConsoleMode(con, &mut mode) != 0 && mode == 7, mode.into())?;
        c.eq(SetConsoleMode(con, 0x8000_0000) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_PARAMETER)?;
        SetConsoleMode(con, 3);

        let mut info = ScreenBufferInfo::default();
        c.ok(GetConsoleScreenBufferInfo(con, &mut info) != 0, GetLastError().into())?;
        let [left, top, right, bottom] = info.window;
        c.ok(left >= 0 && top >= 0 && right > left && bottom > top, right as u64)?;
        c.ok(right < info.size[0] && bottom < info.size[1], info.size[0] as u64)?;
        c.ok(info.cursor[0] < info.size[0] && info.cursor[1] < info.size[1], 0)?;

        // The cursor follows what is written (from the start of a line).
        c.ok(SetConsoleCursorPosition(con, (info.cursor[1] as u32) << 16) != 0, 0)?;
        let text = w("abc");
        let mut written = 0u32;
        c.ok(WriteConsoleW(con, text.as_ptr(), 3, &mut written, core::ptr::null_mut()) != 0, 0)?;
        c.eq(written.into(), 3)?;
        let mut after = ScreenBufferInfo::default();
        GetConsoleScreenBufferInfo(con, &mut after);
        c.ok(after.cursor == [3, info.cursor[1]], after.cursor[0] as u64)?;
        let newline = w("\r\n");
        WriteConsoleW(con, newline.as_ptr(), 2, &mut written, core::ptr::null_mut());

        c.ok(SetConsoleTextAttribute(con, 0x1f) != 0, 0)?;
        GetConsoleScreenBufferInfo(con, &mut after);
        c.eq(after.attributes.into(), 0x1f)?;
        SetConsoleTextAttribute(con, info.attributes);

        // CONSOLE_SCREEN_BUFFER_INFOEX: cbSize 96.
        let mut ex = [0u8; 96];
        ex[0] = 96;
        c.ok(GetConsoleScreenBufferInfoEx(con, ex.as_mut_ptr()) != 0, GetLastError().into())?;

        // Not a console: a file.
        let mut written = 0u32;
        let file = w("C:\\\\Users\\\\Public\\\\console_test.txt");
        let f = CreateFileW(
            file.as_ptr(),
            GENERIC_WRITE,
            0,
            core::ptr::null_mut(),
            2,
            0,
            core::ptr::null_mut(),
        );
        c.eq(WriteConsoleW(f, text.as_ptr(), 3, &mut written, core::ptr::null_mut()) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_HANDLE)?;
        CloseHandle(f);
        CloseHandle(con);

        let input = open("CONIN$");
        c.ok(input as isize != -1, GetLastError().into())?;
        c.ok(GetConsoleMode(input, &mut mode) != 0, 0)?;
        c.eq((mode & 7).into(), 7)?; // PROCESSED, LINE, ECHO
        let mut events = 7u32;
        c.ok(GetNumberOfConsoleInputEvents(input, &mut events) != 0, 0)?;
        CloseHandle(input);
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"console_test: ok\r\n");
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
