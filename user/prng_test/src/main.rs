//! bcryptprimitives.dll!ProcessPrng, the random-number source the Go
//! runtime (and the Windows CRT) uses, loaded as Go loads it
//! (LoadLibraryExW with LOAD_LIBRARY_SEARCH_SYSTEM32, GetProcAddress):
//! it fills buffers of any length, always succeeds, and two fills differ.
//! Prints `prng_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;
type ProcessPrng = extern "system" fn(*mut u8, usize) -> Bool;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn LoadLibraryExW(name: *const u16, file: Handle, flags: u32) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    fn ExitProcess(code: u32) -> !;
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

fn run() -> u32 {
    let name: [u16; 21] = {
        let s = b"bcryptprimitives.dll\0";
        let mut w = [0u16; 21];
        let mut i = 0;
        while i < s.len() {
            w[i] = s[i] as u16;
            i += 1;
        }
        w
    };
    // SAFETY: documented calls with valid arguments.
    unsafe {
        let dll = LoadLibraryExW(name.as_ptr(), core::ptr::null_mut(), 0x800);
        if dll.is_null() {
            return 1;
        }
        let f = GetProcAddress(dll, c"ProcessPrng".as_ptr().cast());
        if f.is_null() {
            return 2;
        }
        let prng = core::mem::transmute::<*mut c_void, ProcessPrng>(f);
        let (mut a, mut b) = ([0u8; 61], [0u8; 61]); // not a multiple of 8
        if prng(a.as_mut_ptr(), a.len()) != 1 || prng(b.as_mut_ptr(), b.len()) != 1 {
            return 3;
        }
        if a == b || a.iter().all(|&x| x == 0) || a[56..].iter().all(|&x| x == 0) {
            return 4;
        }
        let mut big = [0u8; 1024];
        if prng(big.as_mut_ptr(), big.len()) != 1 || big[1000..].iter().all(|&x| x == 0) {
            return 5;
        }
        if prng(core::ptr::null_mut(), 0) != 1 {
            return 6;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = run();
    if code == 0 {
        out(b"prng_test: ok\r\n");
    }
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
