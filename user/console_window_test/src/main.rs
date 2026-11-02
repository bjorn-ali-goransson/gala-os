//! A console program shown in a console window (`console=` on the kernel
//! command line: gala's conhost.exe draws its session): it writes plain
//! text, then text in colours with VT sequences (green, bright red), as
//! console programs do on Windows 10 and later, prints
//! `console_window_test: printed`, waits 20 seconds (xtask checks the
//! window meanwhile) and ends.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn Sleep(ms: u32);
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

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
    // SAFETY: plain calls.
    unsafe {
        Sleep(20_000);
        ExitProcess(0)
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
