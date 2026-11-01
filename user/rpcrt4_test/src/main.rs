//! rpcrt4's UUID functions, with the results real Windows gives:
//! UuidCreate makes random (version 4, RFC 4122 variant) UUIDs, a new one
//! each call; UuidToStringW/A write the 36-character lowercase form,
//! freed by RpcStringFreeW/A (which null the pointer); UuidFromStringW
//! parses it back (either case), gives the nil UUID for a null string and
//! RPC_S_INVALID_STRING_UUID for a malformed one.
//!
//! Prints `rpcrt4_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Handle = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Uuid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

const NIL: Uuid = Uuid { data1: 0, data2: 0, data3: 0, data4: [0; 8] };

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "rpcrt4", kind = "raw-dylib")]
unsafe extern "system" {
    fn UuidCreate(uuid: *mut Uuid) -> i32;
    fn UuidToStringW(uuid: *const Uuid, out: *mut *mut u16) -> i32;
    fn UuidToStringA(uuid: *const Uuid, out: *mut *mut u8) -> i32;
    fn UuidFromStringW(text: *const u16, uuid: *mut Uuid) -> i32;
    fn RpcStringFreeW(text: *mut *mut u16) -> i32;
    fn RpcStringFreeA(text: *mut *mut u8) -> i32;
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

const RPC_S_OK: u64 = 0;
const RPC_S_INVALID_STRING_UUID: u64 = 1705;

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
        out(b"rpcrt4_test: check ");
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

/// The string at `p` (`n` units).
fn units<T: Copy>(p: *const T, n: usize) -> &'static [T] {
    // SAFETY: callers pass strings of at least `n` units.
    unsafe { core::slice::from_raw_parts(p, n) }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: rpcrt4 calls with buffers of ours.
    unsafe {
        let (mut a, mut b) = (NIL, NIL);
        c.eq(UuidCreate(&mut a) as u64, RPC_S_OK)?;
        c.eq(UuidCreate(&mut b) as u64, RPC_S_OK)?;
        c.ok(a != b && a != NIL, 0)?;
        c.eq((a.data3 >> 12).into(), 4)?; // version 4: random
        c.eq((a.data4[0] & 0xc0).into(), 0x80)?; // RFC 4122 variant

        // A known UUID's string forms.
        let known = Uuid {
            data1: 0x0123_abcd,
            data2: 0x4567,
            data3: 0x89ef,
            data4: [0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32, 0x10],
        };
        let want = b"0123abcd-4567-89ef-fedc-ba9876543210";
        let mut w: *mut u16 = core::ptr::null_mut();
        c.eq(UuidToStringW(&known, &mut w) as u64, RPC_S_OK)?;
        c.ok(!w.is_null(), 0)?;
        c.ok(units(w, 37).iter().map(|&u| u as u8).eq(want.iter().copied().chain([0])), 0)?;
        let mut parsed = NIL;
        c.eq(UuidFromStringW(w, &mut parsed) as u64, RPC_S_OK)?;
        c.ok(parsed == known, parsed.data1.into())?;
        c.eq(RpcStringFreeW(&mut w) as u64, RPC_S_OK)?;
        c.ok(w.is_null(), 0)?;
        let mut s: *mut u8 = core::ptr::null_mut();
        c.eq(UuidToStringA(&known, &mut s) as u64, RPC_S_OK)?;
        c.ok(units(s, 37).iter().copied().eq(want.iter().copied().chain([0])), 0)?;
        c.eq(RpcStringFreeA(&mut s) as u64, RPC_S_OK)?;
        c.ok(s.is_null(), 0)?;

        // Upper case parses too; null is the nil UUID; malformed fails.
        let upper = b"0123ABCD-4567-89EF-FEDC-BA9876543210\0".map(u16::from);
        c.eq(UuidFromStringW(upper.as_ptr(), &mut parsed) as u64, RPC_S_OK)?;
        c.ok(parsed == known, parsed.data1.into())?;
        c.eq(UuidFromStringW(core::ptr::null(), &mut parsed) as u64, RPC_S_OK)?;
        c.ok(parsed == NIL, parsed.data1.into())?;
        let bad = b"0123abcd-4567-89ef-fedc-ba98765432\0".map(u16::from);
        c.eq(UuidFromStringW(bad.as_ptr(), &mut parsed) as u64, RPC_S_INVALID_STRING_UUID)?;
        let bad = b"0123abcd+4567-89ef-fedc-ba9876543210\0".map(u16::from);
        c.eq(UuidFromStringW(bad.as_ptr(), &mut parsed) as u64, RPC_S_INVALID_STRING_UUID)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"rpcrt4_test: ok\r\n");
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
