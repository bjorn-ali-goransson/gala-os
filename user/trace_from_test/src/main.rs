//! `trace_from=` (docs/OBSERVABILITY.md, "Call tracing"): that a
//! *repeat* of a call into one of gala's DLLs is recorded, which the
//! one-shot `reached!` record cannot show.
//!
//! The program makes the same COM call [`CALLS`] times through a vtable
//! slot of d2d1.dll -- ID2D1Factory::GetDesktopDpi, which needs no
//! device and answers the same thing every time -- and checks that each
//! answer is the 96 DPI gala's screen reports. Everything the test is
//! about is in the kernel's own summary at the end of the run, which the
//! boot scenarios match:
//!
//! - `trace_from_calls`: with no budget asked for, all [`CALLS`] calls
//!   are recorded. One record would be the old behaviour; [`CALLS`] of
//!   them is the point of the knob.
//! - `trace_from_budget`: with `:10`, ten are recorded and the trace
//!   stops itself there, so a long run cannot be made to pay a system
//!   call a call for longer than it was asked to.
//! - `trace_from_answer`: armed one method earlier, at
//!   ID2D1Factory::ReloadSystemMetrics, every call is recorded and the
//!   [`RELOADS`] made to the method that returns an HRESULT also record
//!   what it answered, while the [`CALLS`] made to the one that returns
//!   nothing record no answer -- there is none to record.
//!
//! A count that reaches the kernel at all also says the rest of the
//! chain holds: the value parsed, the DLL asked for it and armed the
//! slot whose name matched, and `events=call` named a category the event
//! stream knows -- a call the stream drops is never counted.
//!
//! Prints `trace_from_test: ok`, or `trace_from_test: check N got X` and
//! exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::{null, null_mut};

use ntrt as _;

const NAME: &str = "trace_from_test";

/// How many times the traced method is called. More than one is the
/// whole point; small enough that the `:10` scenario's budget bites.
const CALLS: usize = 25;

/// How many times the method that answers is called, before the one
/// that does not. Before, so that a trace armed at the one that does
/// not never sees these: the two counts stay each other's control.
const RELOADS: usize = 5;

type Handle = usize;
type Hr = i32;
type Guid = [u8; 16];
type Obj = *mut c_void;

const S_OK: Hr = 0;

/// ID2D1Factory's slot for GetDesktopDpi (d2d1.h: IUnknown's three, then
/// ReloadSystemMetrics).
const SLOT_GET_DESKTOP_DPI: usize = 4;

/// ID2D1Factory's slot for ReloadSystemMetrics, which answers an
/// HRESULT where GetDesktopDpi answers nothing at all.
const SLOT_RELOAD_SYSTEM_METRICS: usize = 3;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "d2d1", kind = "raw-dylib")]
unsafe extern "system" {
    fn D2D1CreateFactory(kind: u32, iid: *const Guid, options: *const u32, out: *mut Obj) -> Hr;
}

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

const IID_FACTORY: Guid =
    guid(0x0615_2247, 0x6f50, 0x465a, [0x92, 0x45, 0x11, 0x8b, 0xfd, 0x3b, 0x60, 0x07]);

fn run() -> Result<(), u32> {
    let mut factory: Obj = null_mut();
    // SAFETY: a single-threaded factory, ID2D1Factory, no options, into
    // a pointer this frame owns.
    let hr = unsafe { D2D1CreateFactory(0, &IID_FACTORY, null(), &mut factory) };
    check(1, hr == S_OK, hr as u32 as u64)?;
    check(2, !factory.is_null(), 0)?;

    // SAFETY: `factory` is a live ID2D1Factory, whose slot 3 is
    // ReloadSystemMetrics(this) -> HRESULT.
    let reload: extern "system" fn(Obj) -> Hr = unsafe {
        let vtable = *(factory as *const *const usize);
        core::mem::transmute_copy(&*vtable.add(SLOT_RELOAD_SYSTEM_METRICS))
    };
    for _ in 0..RELOADS {
        let hr = reload(factory);
        check(4, hr == S_OK, hr as u32 as u64)?;
    }

    // SAFETY: `factory` is a live ID2D1Factory, whose slot 4 is
    // GetDesktopDpi(this, *mut f32, *mut f32).
    let dpi: extern "system" fn(Obj, *mut f32, *mut f32) = unsafe {
        let vtable = *(factory as *const *const usize);
        core::mem::transmute_copy(&*vtable.add(SLOT_GET_DESKTOP_DPI))
    };
    for i in 0..CALLS {
        let (mut x, mut y) = (0f32, 0f32);
        dpi(factory, &mut x, &mut y);
        // Every call answers, so a trace that recorded fewer than it
        // was asked for stopped on its own account, not on the
        // program's.
        check(3, x == 96.0 && y == 96.0, i as u64)?;
    }
    Ok(())
}

fn check(n: u32, ok: bool, got: u64) -> Result<(), u32> {
    if ok {
        return Ok(());
    }
    Line::new()
        .s(NAME.as_bytes())
        .s(b": check ")
        .dec(u64::from(n))
        .s(b" got ")
        .hex(got)
        .s(b"\r\n")
        .print();
    Err(n)
}

/// A line of output, built without allocating.
struct Line {
    buf: [u8; 128],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Line { buf: [0; 128], n: 0 }
    }

    fn s(mut self, bytes: &[u8]) -> Self {
        for &b in bytes {
            if self.n == self.buf.len() {
                break;
            }
            self.buf[self.n] = b;
            self.n += 1;
        }
        self
    }

    fn dec(self, mut v: u64) -> Self {
        let mut digits = [0u8; 20];
        let mut n = 0;
        loop {
            digits[n] = b'0' + (v % 10) as u8;
            n += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        let mut out = self;
        while n > 0 {
            n -= 1;
            out = out.s(&digits[n..n + 1]);
        }
        out
    }

    fn hex(self, v: u64) -> Self {
        let mut text = [0u8; 18];
        text[0] = b'0';
        text[1] = b'x';
        for i in 0..16 {
            let nibble = (v >> (60 - i * 4)) & 0xf;
            text[2 + i] = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + nibble as u8 - 10 };
        }
        self.s(&text)
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: this frame's buffer, and the standard output handle.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            )
        };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
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
