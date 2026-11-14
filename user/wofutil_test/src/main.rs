//! wofutil.dll, the Windows Overlay Filter's API, as Click-to-Run asks it
//! while staging (ROADMAP OF). Expected values are those Microsoft's
//! documentation states (and Wine's wofutil gives): on a volume where
//! the system does not compress binaries WofShouldCompressBinaries is
//! FALSE; a file just written is a physical file (WofIsExternalFile:
//! S_OK, FALSE). What depends on the machine (whether the overlay filter
//! is attached: WofGetDriverVersion; a path that does not exist) is
//! printed as `wofutil_test: info ...` lines, not checked.
//!
//! Prints `wofutil_test: ok`, or `wofutil_test: check N got X (line L)`
//! and exits with N.

#![no_std]
#![no_main]

use core::ptr::null_mut;

use ntrt as _;

const NAME: &str = "wofutil_test";

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetTempPathW(size: u32, buffer: *mut u16) -> u32;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: usize,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CloseHandle(h: Handle) -> i32;
    fn DeleteFileW(path: *const u16) -> i32;
}

#[link(name = "wofutil", kind = "raw-dylib")]
unsafe extern "system" {
    fn WofShouldCompressBinaries(volume: *const u16, algorithm: *mut u32) -> i32;
    fn WofIsExternalFile(
        path: *const u16,
        external: *mut i32,
        provider: *mut u32,
        info: *mut u8,
        length: *mut u32,
    ) -> i32;
    fn WofGetDriverVersion(h: Handle, provider: u32, version: *mut u32) -> i32;
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const INVALID_HANDLE_VALUE: Handle = usize::MAX;
/// WOF_PROVIDER_FILE.
const WOF_PROVIDER_FILE: u32 = 2;

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
}

/// `%TEMP%<name>`, NUL-terminated.
fn temp(name: &str) -> [u16; 300] {
    let mut p = [0u16; 300];
    // SAFETY: our buffer of 260 characters.
    let n = unsafe { GetTempPathW(260, p.as_mut_ptr()) } as usize;
    for (i, b) in name.bytes().enumerate() {
        p[n + i] = b.into();
    }
    p
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let volume: [u16; 4] = [b'C' as u16, b':' as u16, b'\\' as u16, 0];
    let mut algorithm = 0xdead_beef_u32;
    // SAFETY: a NUL-terminated volume path and our output.
    let compress = unsafe { WofShouldCompressBinaries(volume.as_ptr(), &mut algorithm) };
    c.ok(compress == 0, compress as u64)?;

    let file = temp("wofutil_test.txt");
    // SAFETY: a NUL-terminated path.
    let h = unsafe { CreateFileW(file.as_ptr(), GENERIC_WRITE, 0, 0, CREATE_ALWAYS, 0, 0) };
    c.ok(h != INVALID_HANDLE_VALUE, 0)?;
    let mut data = 0u32;
    // SAFETY: our handle and buffer.
    unsafe {
        WriteFile(h, b"wofutil".as_ptr(), 7, &mut data, 0);
        CloseHandle(h);
    }
    let (mut external, mut provider, mut length) = (7i32, 7u32, 0u32);
    // SAFETY: a NUL-terminated path and our outputs (no info buffer).
    let r = unsafe {
        WofIsExternalFile(file.as_ptr(), &mut external, &mut provider, null_mut(), &mut length)
    };
    c.ok(r == 0, r as u32 as u64)?;
    c.ok(external == 0, external as u64)?;
    // Without the optional outputs.
    // SAFETY: a NUL-terminated path.
    let r =
        unsafe { WofIsExternalFile(file.as_ptr(), null_mut(), null_mut(), null_mut(), null_mut()) };
    c.ok(r == 0, r as u32 as u64)?;

    let missing = temp("wofutil_test_missing.txt");
    // SAFETY: NUL-terminated paths; our outputs.
    let r = unsafe {
        DeleteFileW(missing.as_ptr());
        WofIsExternalFile(missing.as_ptr(), &mut external, null_mut(), null_mut(), null_mut())
    };
    Line::new()
        .s(NAME.as_bytes())
        .s(b": info missing file ")
        .hex(r as u32 as u64)
        .s(b"\r\n")
        .print();

    // SAFETY: a NUL-terminated path; our handle and output.
    let (r, version) = unsafe {
        let v = CreateFileW(
            volume.as_ptr(),
            GENERIC_READ,
            7,
            0,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            0,
        );
        let mut version = 0u32;
        let r = WofGetDriverVersion(v, WOF_PROVIDER_FILE, &mut version);
        if v != INVALID_HANDLE_VALUE {
            CloseHandle(v);
        }
        (r, version)
    };
    Line::new()
        .s(NAME.as_bytes())
        .s(b": info driver version ")
        .hex(r as u32 as u64)
        .s(b" ")
        .hex(version.into())
        .s(b"\r\n")
        .print();
    // SAFETY: a NUL-terminated path.
    unsafe { DeleteFileW(file.as_ptr()) };
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
