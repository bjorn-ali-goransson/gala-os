//! pdh.dll's real-time queries, as documented (pdh.h; what Chromium's CPU
//! probe uses): PdhOpenQueryW with no data source; PdhAddEnglishCounterW
//! of `\Processor(_Total)\% Processor Time`; an unknown object is
//! PDH_CSTATUS_NO_OBJECT and an unknown counter of a known object
//! PDH_CSTATUS_NO_COUNTER; collecting a query without counters is
//! PDH_NO_DATA; a rate counter has no value after one collection and a
//! valid one (CStatus PDH_CSTATUS_VALID_DATA) between 0 and 100 after two,
//! as a double and as a long; a removed counter and a closed query are
//! PDH_INVALID_HANDLE.
//!
//! Prints `pdh_test: ok` or exits with the number of the failing check.

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

#[link(name = "pdh", kind = "raw-dylib")]
unsafe extern "system" {
    fn PdhOpenQueryW(source: *const u16, user: usize, query: *mut Handle) -> u32;
    fn PdhAddEnglishCounterW(
        query: Handle,
        path: *const u16,
        user: usize,
        counter: *mut Handle,
    ) -> u32;
    fn PdhCollectQueryData(query: Handle) -> u32;
    fn PdhGetFormattedCounterValue(
        counter: Handle,
        format: u32,
        kind: *mut u32,
        value: *mut CounterValue,
    ) -> u32;
    fn PdhRemoveCounter(counter: Handle) -> u32;
    fn PdhCloseQuery(query: Handle) -> u32;
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

/// PDH_FMT_COUNTERVALUE: the status, then the value (a union of long,
/// double and LONGLONG, 8-aligned).
#[repr(C)]
struct CounterValue {
    status: u32,
    value: u64,
}

const PDH_FMT_LONG: u32 = 0x100;
const PDH_FMT_DOUBLE: u32 = 0x200;
const PDH_CSTATUS_NO_OBJECT: u64 = 0xc000_0bb8;
const PDH_CSTATUS_NO_COUNTER: u64 = 0xc000_0bb9;
const PDH_INVALID_HANDLE: u64 = 0xc000_0bbc;
const PDH_NO_DATA: u64 = 0x8000_07d5;

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

fn hex(v: u64) -> [u8; 16] {
    let mut s = [b'0'; 16];
    for (i, c) in s.iter_mut().enumerate() {
        *c = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 15) as usize];
    }
    s
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"pdh_test: check ");
        out(&hex(self.0.into()));
        out(b" failed, got 0x");
        out(&hex(got));
        check_line(core::panic::Location::caller().line());
        out(b"\r\n");
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

/// `text` as a NUL-terminated UTF-16 string.
fn wide<const N: usize>(text: &str) -> [u16; N] {
    let mut w = [0u16; N];
    for (slot, b) in w.iter_mut().zip(text.bytes()) {
        *slot = b.into();
    }
    w
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let null = core::ptr::null_mut::<c_void>();
    // SAFETY: pdh calls with handles and buffers of ours.
    unsafe {
        let mut query = null;
        c.eq(PdhOpenQueryW(core::ptr::null(), 0, &mut query).into(), 0)?;
        c.ok(!query.is_null(), 0)?;
        c.eq(PdhCollectQueryData(query).into(), PDH_NO_DATA)?;

        let mut counter = null;
        let path = wide::<64>("\\Processor(_Total)\\% Processor Time");
        c.eq(PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter).into(), 0)?;
        c.ok(!counter.is_null(), 0)?;
        let mut other = null;
        let path = wide::<64>("\\No Such Object(_Total)\\% Processor Time");
        let st = PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut other);
        c.eq(st.into(), PDH_CSTATUS_NO_OBJECT)?;
        let path = wide::<64>("\\Processor(_Total)\\No Such Counter");
        let st = PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut other);
        c.eq(st.into(), PDH_CSTATUS_NO_COUNTER)?;

        // One collection: a rate counter has no value yet.
        c.eq(PdhCollectQueryData(query).into(), 0)?;
        let mut value = CounterValue { status: 0, value: 0 };
        let st =
            PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, core::ptr::null_mut(), &mut value);
        c.ok(st != 0, st.into())?;
        // Two: a percentage, a second later (PdhCollectQueryData's
        // documentation: rate counters need at least one second between
        // collections; 100 ms gave PDH_CALC_NEGATIVE_DENOMINATOR on CI's
        // Windows once).
        Sleep(1000);
        c.eq(PdhCollectQueryData(query).into(), 0)?;
        let mut kind = 0;
        let st = PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, &mut kind, &mut value);
        c.eq(st.into(), 0)?;
        c.eq(value.status.into(), 0)?; // PDH_CSTATUS_VALID_DATA
        let percent = f64::from_bits(value.value);
        c.ok((0.0..=100.0).contains(&percent), value.value)?;
        let st =
            PdhGetFormattedCounterValue(counter, PDH_FMT_LONG, core::ptr::null_mut(), &mut value);
        c.eq(st.into(), 0)?;
        c.ok((value.value as u32 as i32) >= 0 && (value.value as u32) <= 100, value.value)?;

        c.eq(PdhRemoveCounter(counter).into(), 0)?;
        let st =
            PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, core::ptr::null_mut(), &mut value);
        c.eq(st.into(), PDH_INVALID_HANDLE)?;
        c.eq(PdhCloseQuery(query).into(), 0)?;
        c.eq(PdhCloseQuery(query).into(), PDH_INVALID_HANDLE)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"pdh_test: ok\r\n");
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

/// ` (line L)`: the source line of the failing check, from
/// `#[track_caller]` on Checks' methods (so a report names the line
/// without anyone counting checks).
fn check_line(line: u32) {
    let mut b = *b" (line 0000000000)";
    let mut digits = [0u8; 10];
    let (mut n, mut len) = (line, 0);
    loop {
        digits[len] = b'0' + (n % 10) as u8;
        len += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for i in 0..len {
        b[7 + i] = digits[len - 1 - i];
    }
    b[7 + len] = b')';
    out(&b[..8 + len]);
}
