//! DPAPI (crypt32's CryptProtectData/CryptUnprotectData), with the
//! documented results: protected data comes back with its description; the
//! blob does not contain the plaintext; another entropy, or a changed blob,
//! fails to unprotect; the outputs are LocalAlloc'ed (LocalFree).
//!
//! Prints `dpapi_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Handle = *mut c_void;

/// DATA_BLOB.
#[repr(C)]
struct Blob {
    size: u32,
    data: *mut u8,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn LocalFree(p: *mut c_void) -> *mut c_void;
}

#[link(name = "crypt32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CryptProtectData(
        input: *const Blob,
        description: *const u16,
        entropy: *const Blob,
        reserved: *mut c_void,
        prompt: *mut c_void,
        flags: u32,
        output: *mut Blob,
    ) -> i32;
    fn CryptUnprotectData(
        input: *const Blob,
        description: *mut *mut u16,
        entropy: *const Blob,
        reserved: *mut c_void,
        prompt: *mut c_void,
        flags: u32,
        output: *mut Blob,
    ) -> i32;
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
        out(b"dpapi_test: check ");
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

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let null: *mut c_void = core::ptr::null_mut();
    // SAFETY: crypt32 calls with buffers of ours; outputs freed with
    // LocalFree.
    unsafe {
        let mut secret = *b"service master key";
        let input = Blob { size: secret.len() as u32, data: secret.as_mut_ptr() };
        let desc: [u16; 5] = [0x67, 0x61, 0x6c, 0x61, 0]; // "gala"
        let mut salt = *b"entropy";
        let entropy = Blob { size: 7, data: salt.as_mut_ptr() };
        let mut protected = Blob { size: 0, data: null.cast() };
        c.ok(
            CryptProtectData(&input, desc.as_ptr(), &entropy, null, null, 1, &mut protected) != 0,
            0,
        )?;
        let blob = core::slice::from_raw_parts(protected.data, protected.size as usize);
        c.ok(blob.len() > secret.len(), blob.len() as u64)?;
        c.ok(!blob.windows(secret.len()).any(|w| w == secret), 0)?;

        let mut out = Blob { size: 0, data: null.cast() };
        let mut d: *mut u16 = null.cast();
        c.ok(CryptUnprotectData(&protected, &mut d, &entropy, null, null, 1, &mut out) != 0, 0)?;
        c.ok(core::slice::from_raw_parts(out.data, out.size as usize) == secret, out.size.into())?;
        c.ok(!d.is_null() && *d == 0x67 && *d.add(3) == 0x61 && *d.add(4) == 0, 0)?;
        LocalFree(out.data.cast());
        LocalFree(d.cast());

        // Other entropy: no.
        let mut other = *b"Entropy";
        let wrong = Blob { size: 7, data: other.as_mut_ptr() };
        c.eq(
            CryptUnprotectData(&protected, null.cast(), &wrong, null, null, 1, &mut out) as u64,
            0,
        )?;
        // A changed byte: no.
        *protected.data.add(protected.size as usize - 1) ^= 1;
        c.eq(
            CryptUnprotectData(&protected, null.cast(), &entropy, null, null, 1, &mut out) as u64,
            0,
        )?;
        LocalFree(protected.data.cast());
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"dpapi_test: ok\r\n");
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
