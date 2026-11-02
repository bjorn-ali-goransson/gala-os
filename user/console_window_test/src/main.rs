//! A console program shown in a console window (`console=` on the kernel
//! command line: gala's conhost.exe draws its session): it writes plain
//! text, then text in colours with VT sequences (green, bright red), as
//! console programs do on Windows 10 and later, prints
//! `console_window_test: printed`, then reads a line from standard input
//! (xtask types "hi" and Enter in the window; the console echoes it) and
//! prints `console_window_test: read hi` (and ends 8 seconds later, so
//! xtask sees the window). ReadFile on a console returns
//! the line with "\r\n", as Windows' cooked mode does.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> i32;
    fn Sleep(ms: u32);
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

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

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    out(b"Hello from a console window on gala\n");
    out(b"\x1b[32mgreen text\x1b[0m and \x1b[1;31mbright red\x1b[0m\n");
    out("UTF-8: \u{e5}\u{e4}\u{f6} \u{e9}\u{f1}\n".as_bytes());
    out(b"console_window_test: printed\n");
    let mut line = [0u8; 64];
    let mut got = 0u32;
    // SAFETY: our buffer; the standard input handle.
    let ok = unsafe {
        ReadFile(
            GetStdHandle(-10i32 as u32),
            line.as_mut_ptr(),
            line.len() as u32,
            &mut got,
            core::ptr::null_mut(),
        )
    };
    let n = got as usize;
    if ok == 0 || n < 2 || line[n - 2] != b'\r' || line[n - 1] != b'\n' {
        out(b"console_window_test: no line\n");
        // SAFETY: ends the process.
        unsafe { ExitProcess(1) }
    }
    out(b"console_window_test: read ");
    out(&line[..n - 2]);
    out(b"\n");
    // Time for xtask to look at the window, then the end.
    // SAFETY: plain calls.
    unsafe {
        Sleep(8000);
        ExitProcess(0)
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
