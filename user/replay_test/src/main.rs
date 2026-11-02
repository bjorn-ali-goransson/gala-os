//! Inputs from outside the program, for record/replay (ROADMAP R7,
//! docs/ARCHITECTURE.md "Record/replay"): random numbers from
//! bcryptprimitives.dll!ProcessPrng, the performance counter across a
//! Sleep and the system time. On Windows, as on gala: two random fills
//! differ, the counter moves forward by at least 4 ms across Sleep(5),
//! and the system time is after 2020; it prints what it read as
//! `replay_test: <what> <hex>` lines and `replay_test: ok`.
//!
//! `cargo xtask replay-test` runs it in gala-um with `--record=<file>`,
//! then with `--replay=<file>`, and checks that the replay printed the
//! same lines (the same inputs), with and without `--sim`.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn Sleep(ms: u32);
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "bcryptprimitives", kind = "raw-dylib")]
unsafe extern "system" {
    fn ProcessPrng(data: *mut u8, length: usize) -> Bool;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQueryPerformanceCounter(counter: *mut i64, frequency: *mut i64) -> u32;
    fn NtQuerySystemTime(time: *mut i64) -> u32;
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

/// Named by core's unwind tables; never called (panics abort).
#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
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

/// Prints `replay_test: <what> <hex of bytes>`.
fn show(what: &[u8], bytes: &[u8]) {
    let mut line = [0u8; 128];
    let mut n = 0;
    for &b in b"replay_test: ".iter().chain(what).chain(b" ") {
        line[n] = b;
        n += 1;
    }
    for &b in bytes {
        line[n] = b"0123456789abcdef"[usize::from(b >> 4)];
        line[n + 1] = b"0123456789abcdef"[usize::from(b & 15)];
        n += 2;
    }
    line[n] = b'\r';
    line[n + 1] = b'\n';
    out(&line[..n + 2]);
}

fn counter() -> (i64, i64) {
    let (mut c, mut f) = (0, 0);
    // SAFETY: valid output pointers.
    unsafe { NtQueryPerformanceCounter(&mut c, &mut f) };
    (c, f)
}

fn run() -> u32 {
    let (mut a, mut b) = ([0u8; 16], [0u8; 16]);
    // SAFETY: valid buffers of those lengths.
    if unsafe { ProcessPrng(a.as_mut_ptr(), a.len()) != 1 || ProcessPrng(b.as_mut_ptr(), 5) != 1 } {
        return 1;
    }
    if a == b {
        return 2;
    }
    show(b"random", &a);
    show(b"random", &b[..5]);
    let (before, frequency) = counter();
    // SAFETY: no preconditions.
    unsafe { Sleep(5) };
    let (after, _) = counter();
    if frequency <= 0 || after - before < 4 * frequency / 1000 {
        return 3;
    }
    show(b"counter", &before.to_be_bytes());
    show(b"counter", &after.to_be_bytes());
    let mut time = 0;
    // SAFETY: a valid output pointer.
    unsafe { NtQuerySystemTime(&mut time) };
    // 2020-01-01 in 100 ns units since 1601.
    if time < 132_223_104_000_000_000 {
        return 4;
    }
    show(b"time", &time.to_be_bytes());
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = run();
    if code == 0 {
        out(b"replay_test: ok\r\n");
    }
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
