//! slc.dll, the Software Licensing Client, as a program finds it on
//! Windows: Office's Click-to-Run service loads it by name while it
//! brings up the App-V ISV API (ROADMAP OF). Expected values are those
//! Windows answers (measured on Windows 11 24H2, build 26200):
//!
//! - slc.dll forwards most of its exports to SPPC, so SLOpen through it
//!   gives S_OK and a handle, and SLClose S_OK (as `sppc_test` has it
//!   of sppc.dll itself);
//! - SLGetWindowsInformation and SLGetWindowsInformationDWORD fail for
//!   a value name no system has;
//! - a NULL name or output pointer fails;
//! - SLIsWindowsGenuineLocal gives S_OK and a genuine state (which one
//!   is the machine's own: it is printed, not checked).
//!
//! The HRESULTs of those failures, the genuine state and whether the
//! six exports whose parameters are in no public header are present are
//! printed as `slc_test: info` lines, to be read beside a Windows run
//! (ARCHITECTURE "slc").
//!
//! Prints `slc_test: ok`, or `slc_test: check N got X (line L)` and
//! exits with N.

#![no_std]
#![no_main]

use core::ptr::{null, null_mut};

use ntrt as _;

const NAME: &str = "slc_test";

type Handle = usize;

const S_OK: i32 = 0;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn LoadLibraryW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> usize;
}

#[link(name = "slc", kind = "raw-dylib")]
unsafe extern "system" {
    fn SLOpen(handle: *mut Handle) -> i32;
    fn SLClose(handle: Handle) -> i32;
    fn SLGetWindowsInformation(
        name: *const u16,
        data_type: *mut u32,
        size: *mut u32,
        value: *mut *mut u8,
    ) -> i32;
    fn SLGetWindowsInformationDWORD(name: *const u16, value: *mut u32) -> i32;
    fn SLIsWindowsGenuineLocal(state: *mut u32) -> i32;
}

struct Line {
    buf: [u8; 200],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 200], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: our buffer; the standard output handle.
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

/// `slc_test: info <what> <hr>`.
fn info(what: &[u8], hr: i32) {
    Line::new()
        .s(NAME.as_bytes())
        .s(b": info ")
        .s(what)
        .s(b" ")
        .hex(hr as u32 as u64)
        .s(b"\r\n")
        .print();
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    /// A failed HRESULT, printed as `info <what>`.
    #[track_caller]
    fn failed(&mut self, what: &[u8], hr: i32) -> Result<(), u32> {
        info(what, hr);
        self.ok(hr < 0, hr as u32 as u64)
    }
}

/// A NUL-terminated UTF-16 copy of `s` (at most 63 characters).
fn wide(s: &str) -> [u16; 64] {
    let mut w = [0u16; 64];
    for (i, b) in s.bytes().take(63).enumerate() {
        w[i] = b.into();
    }
    w
}

/// The names Windows' slc.dll exports whose parameters are in no public
/// header, which gala therefore does not export: present here or not is
/// printed, not checked (ARCHITECTURE "slc").
const UNDOCUMENTED: [&[u8]; 6] = [
    b"SLConsumeWindowsRight\0",
    b"SLReArmWindows\0",
    b"SLRegisterWindowsEvent\0",
    b"SLUnregisterWindowsEvent\0",
    b"SLpCheckProductKey\0",
    b"SLpUpdateComponentTokens\0",
];

fn run() -> Result<(), u32> {
    let mut c = Checks(0);

    // The forwarders: slc.dll's SLOpen and SLClose are SPPC's.
    let mut h: Handle = 0;
    // SAFETY: our output.
    let hr = unsafe { SLOpen(&mut h) };
    c.ok(hr == S_OK, hr as u32 as u64)?;
    c.ok(h != 0, h as u64)?;
    // SAFETY: the handle SLOpen gave.
    let hr = unsafe { SLClose(h) };
    c.ok(hr == S_OK, hr as u32 as u64)?;

    // The genuine state: which one is the machine's own.
    let mut state = u32::MAX;
    // SAFETY: our output.
    let hr = unsafe { SLIsWindowsGenuineLocal(&mut state) };
    c.ok(hr == S_OK, hr as u32 as u64)?;
    Line::new()
        .s(NAME.as_bytes())
        .s(b": info genuine state ")
        .dec(u64::from(state))
        .s(b"\r\n")
        .print();

    // A value name no system has.
    let name = wide("Gala-NoSuchValue");
    let mut value = 0u32;
    // SAFETY: our name and output.
    let hr = unsafe { SLGetWindowsInformationDWORD(name.as_ptr(), &mut value) };
    c.failed(b"SLGetWindowsInformationDWORD of a name no system has", hr)?;
    let (mut data_type, mut size, mut blob) = (0u32, 0u32, null_mut::<u8>());
    // SAFETY: our name and outputs.
    let hr =
        unsafe { SLGetWindowsInformation(name.as_ptr(), &mut data_type, &mut size, &mut blob) };
    c.failed(b"SLGetWindowsInformation of a name no system has", hr)?;

    // NULL arguments.
    // SAFETY: a NULL name is the case under test.
    let hr = unsafe { SLGetWindowsInformationDWORD(null(), &mut value) };
    c.failed(b"SLGetWindowsInformationDWORD(NULL name)", hr)?;
    // SAFETY: a NULL output is the case under test.
    let hr = unsafe { SLGetWindowsInformationDWORD(name.as_ptr(), null_mut()) };
    c.failed(b"SLGetWindowsInformationDWORD(NULL value)", hr)?;
    // SAFETY: a NULL name is the case under test.
    let hr = unsafe { SLGetWindowsInformation(null(), &mut data_type, &mut size, &mut blob) };
    c.failed(b"SLGetWindowsInformation(NULL name)", hr)?;
    // SAFETY: a NULL output is the case under test.
    let hr =
        unsafe { SLGetWindowsInformation(name.as_ptr(), &mut data_type, null_mut(), &mut blob) };
    c.failed(b"SLGetWindowsInformation(NULL size)", hr)?;

    // Which of the undocumented names this slc.dll has.
    let dll = wide("slc.dll");
    // SAFETY: a NUL-terminated name.
    let module = unsafe { LoadLibraryW(dll.as_ptr()) };
    c.ok(module != 0, module as u64)?;
    for name in UNDOCUMENTED {
        // SAFETY: a NUL-terminated name; the module just loaded.
        let found = unsafe { GetProcAddress(module, name.as_ptr()) };
        Line::new()
            .s(NAME.as_bytes())
            .s(b": info ")
            .s(&name[..name.len() - 1])
            .s(if found == 0 { b" absent\r\n" } else { b" present\r\n" })
            .print();
    }
    Ok(())
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
