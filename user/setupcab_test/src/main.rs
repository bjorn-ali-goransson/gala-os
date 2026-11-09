//! setupapi's SetupIterateCabinetW, which ODT's setup.exe uses to unpack
//! the Office version descriptor cabinet (ROADMAP OF).
//!
//! Expected values, and where they come from:
//! - Wine's conformance tests (dlls/setupapi/tests/setupcab.c, run
//!   against Windows): a NULL path is FALSE with ERROR_INVALID_PARAMETER,
//!   an empty one ERROR_NOT_ENOUGH_MEMORY, a missing file
//!   ERROR_FILE_NOT_FOUND, an empty file ERROR_INVALID_DATA (callback or
//!   not); a valid cabinet with no callback ERROR_INVALID_DATA. Its
//!   cabinet (`comp_cab_zip_multi`: MSZIP, three files) written as
//!   `<temp>/./testcab.cab` is TRUE, with SPFILENOTIFY_CABINETINFO
//!   first (CabinetPath the temporary directory, as GetTempPathW gives
//!   it, CabinetFile and DiskName empty, SetId and CabinetNumber 0,
//!   param2 0), then for each file in order SPFILENOTIFY_FILEINCABINET
//!   (NameInCabinet, FileSize, Win32Error 0, DosDate 14545, DosTime
//!   18672, DosAttribs FILE_ATTRIBUTE_ARCHIVE; param2 the cabinet's path
//!   as passed), FILEOP_DOIT to FullTargetName `<temp>\<name>`, and
//!   SPFILENOTIFY_FILEEXTRACTED (Source the path as passed, Target that
//!   name, Win32Error 0, param2 0).
//! - The extracted bytes are the cabinet's (7-Zip reads the same:
//!   "laurence", "uncompressed", "sterne", each with CR LF).
//! - setupapi.h's documentation of SPFILENOTIFY_FILEINCABINET:
//!   FILEOP_SKIP leaves the file out (no file, no FILEEXTRACTED for it).
//! - The stored (uncompressed) folder of `../wintrust_test/data/
//!   selfsigned.cab` (hello.txt, 29 bytes, 2026-01-01 12:00:00: DosDate
//!   23585, DosTime 24576): read from the file's own header.
//!
//! The same source builds `wow64_setupcab_test`, the 32-bit (x86)
//! program (ODT's setup.exe is 32-bit). Prints `setupcab_test: ok`
//! (`wow64_setupcab_test: ok`), or `<name>: check N got X (line L)` and
//! exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "setupcab_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_setupcab_test";

type Handle = usize;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: usize,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> Handle;
    fn CloseHandle(h: Handle) -> i32;
    fn DeleteFileW(name: *const u16) -> i32;
    fn GetFileAttributesW(name: *const u16) -> u32;
    fn GetTempPathW(len: u32, buf: *mut u16) -> u32;
    fn ExitProcess(code: u32) -> !;
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
}

type Callback = extern "system" fn(*mut c_void, u32, usize, usize) -> u32;

#[cfg_attr(
    target_arch = "x86",
    link(name = "setupapi", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "setupapi", kind = "raw-dylib"))]
unsafe extern "system" {
    fn SetupIterateCabinetW(
        cabinet: *const u16,
        reserved: u32,
        callback: Option<Callback>,
        context: *mut c_void,
    ) -> i32;
}

/// CABINET_INFO_W.
#[repr(C)]
struct CabinetInfo {
    path: *const u16,
    file: *const u16,
    disk: *const u16,
    set_id: u16,
    number: u16,
}

/// FILE_IN_CABINET_INFO_W.
#[repr(C)]
struct FileInCabinet {
    name: *const u16,
    size: u32,
    error: u32,
    date: u16,
    time: u16,
    attributes: u16,
    target: [u16; 260],
}

/// FILEPATHS_W.
#[repr(C)]
struct FilePaths {
    target: *const u16,
    source: *const u16,
    error: u32,
    flags: u32,
}

const SPFILENOTIFY_CABINETINFO: u32 = 0x10;
const SPFILENOTIFY_FILEINCABINET: u32 = 0x11;
const SPFILENOTIFY_FILEEXTRACTED: u32 = 0x13;
const FILEOP_DOIT: u32 = 1;
const FILEOP_SKIP: u32 = 2;

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_INVALID_DATA: u32 = 13;
const ERROR_INVALID_PARAMETER: u32 = 87;
const INVALID_HANDLE_VALUE: Handle = usize::MAX;
const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const FILE_ATTRIBUTE_ARCHIVE: u16 = 0x20;

/// Wine's `comp_cab_zip_multi` (dlls/setupapi/tests/setupcab.c): MSZIP,
/// files "tristram" (10 bytes), "wine" (14) and "shandy" (8).
const WINE_CAB: [u8; 156] = [
    0x4d, 0x53, 0x43, 0x46, 0x00, 0x00, 0x00, 0x00, 0x9c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x2c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x03, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x71, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x0a, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xd1, 0x38, 0xf0, 0x48, 0x20, 0x00, 0x74, 0x72, 0x69, 0x73,
    0x74, 0x72, 0x61, 0x6d, 0x00, 0x0e, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0xd1,
    0x38, 0xf0, 0x48, 0x20, 0x00, 0x77, 0x69, 0x6e, 0x65, 0x00, 0x08, 0x00, 0x00, 0x00, 0x18, 0x00,
    0x00, 0x00, 0x00, 0x00, 0xd1, 0x38, 0xf0, 0x48, 0x20, 0x00, 0x73, 0x68, 0x61, 0x6e, 0x64, 0x79,
    0x00, 0x67, 0x2c, 0x03, 0x85, 0x23, 0x00, 0x20, 0x00, 0x43, 0x4b, 0xcb, 0x49, 0x2c, 0x2d, 0x4a,
    0xcd, 0x4b, 0x4e, 0xe5, 0xe5, 0x2a, 0xcd, 0x4b, 0xce, 0xcf, 0x2d, 0x28, 0x4a, 0x2d, 0x2e, 0x4e,
    0x4d, 0xe1, 0xe5, 0x2a, 0x2e, 0x49, 0x2d, 0xca, 0x03, 0x8a, 0x02, 0x00,
];

/// One expected file: name, size, DOS date and time, contents.
struct Expected {
    name: &'static [u8],
    date: u16,
    time: u16,
    data: &'static [u8],
}

const WINE_FILES: [Expected; 3] = [
    Expected { name: b"tristram", date: 14545, time: 18672, data: b"laurence\r\n" },
    Expected { name: b"wine", date: 14545, time: 18672, data: b"uncompressed\r\n" },
    Expected { name: b"shandy", date: 14545, time: 18672, data: b"sterne\r\n" },
];

const SIGNED_FILES: [Expected; 1] = [Expected {
    name: b"hello.txt",
    date: 23585,
    time: 24576,
    data: b"hello from a signed cabinet\r\n",
}];

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
        self.report(got, core::panic::Location::caller().line());
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u32, want: u32) -> Result<(), u32> {
        self.ok(got == want, got.into())
    }

    fn report(&self, got: u64, line: u32) {
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(line))
            .s(b")\r\n")
            .print();
    }
}

/// A NUL-terminated wide string's characters.
fn wstr<'a>(p: *const u16) -> &'a [u16] {
    if p.is_null() {
        return &[];
    }
    let mut n = 0;
    // SAFETY: a NUL-terminated string the caller handed us.
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    }
}

/// A path buffer: `prefix` (wide, up to its NUL) then `tail` (ASCII).
fn path(prefix: &[u16], tail: &[u8]) -> [u16; 300] {
    let mut out = [0u16; 300];
    let head = prefix.iter().copied().take_while(|&c| c != 0);
    let rest = tail.iter().map(|&b| u16::from(b));
    for (slot, c) in out[..299].iter_mut().zip(head.chain(rest)) {
        *slot = c;
    }
    out
}

/// Whether wide string `w` is the ASCII `a`.
fn same(w: &[u16], a: &[u8]) -> bool {
    w.len() == a.len() && w.iter().zip(a).all(|(&x, &y)| x == u16::from(y))
}

/// Writes `data` to the file `path` (made anew).
fn write_file(path: &[u16], data: &[u8]) -> bool {
    // SAFETY: a NUL-terminated path; our handle and buffer.
    unsafe {
        let h = CreateFileW(path.as_ptr(), GENERIC_WRITE, 0, 0, CREATE_ALWAYS, 0, 0);
        if h == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut written = 0u32;
        let ok = WriteFile(h, data.as_ptr(), data.len() as u32, &mut written, 0) != 0
            && written as usize == data.len();
        CloseHandle(h);
        ok
    }
}

/// Whether the file `path` holds exactly `want`.
fn holds(path: &[u16], want: &[u8]) -> bool {
    let mut buf = [0u8; 64];
    // SAFETY: a NUL-terminated path; our handle and buffer.
    unsafe {
        let h = CreateFileW(path.as_ptr(), GENERIC_READ, 1, 0, OPEN_EXISTING, 0, 0);
        if h == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut got = 0u32;
        let ok = ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut got, 0) != 0;
        CloseHandle(h);
        ok && &buf[..got as usize] == want
    }
}

/// What a callback saw: the first mismatch's source line (0: none) and
/// the notifications counted.
struct Context {
    temp: [u16; 300],
    /// The cabinet's path as passed.
    cabinet: [u16; 300],
    files: &'static [Expected],
    /// Wine's cabinet (its CABINET_INFO_W is checked whole).
    wine: bool,
    /// The file (index) to answer FILEOP_SKIP, if any.
    skip: usize,
    index: usize,
    cabinet_infos: u32,
    in_cabinet: u32,
    extracted: u32,
    target: [u16; 300],
    mismatch: u32,
}

impl Context {
    #[track_caller]
    fn expect(&mut self, cond: bool) {
        if !cond && self.mismatch == 0 {
            self.mismatch = core::panic::Location::caller().line();
        }
    }
}

/// The notifications of a successful iteration, checked as Wine's
/// simple_callbackW checks them.
extern "system" fn callback(context: *mut c_void, message: u32, p1: usize, p2: usize) -> u32 {
    // SAFETY: the Context `run` passed.
    let c = unsafe { &mut *context.cast::<Context>() };
    match message {
        SPFILENOTIFY_CABINETINFO => {
            // SAFETY: the CABINET_INFO_W setupapi passes.
            let info = unsafe { &*(p1 as *const CabinetInfo) };
            c.cabinet_infos += 1;
            let temp = wstr(c.temp.as_ptr());
            c.expect(wstr(info.path) == temp);
            c.expect(wstr(info.file).is_empty());
            c.expect(wstr(info.disk).is_empty());
            // Wine's cabinet's header has set 0, index 0 (whether
            // Windows takes them from the header, which the self-signed
            // cabinet's 0x1234 would show, is not known).
            c.expect(!c.wine || (info.set_id == 0 && info.number == 0));
            c.expect(p2 == 0);
            0
        }
        SPFILENOTIFY_FILEINCABINET => {
            // SAFETY: the FILE_IN_CABINET_INFO_W setupapi passes.
            let info = unsafe { &mut *(p1 as *mut FileInCabinet) };
            c.in_cabinet += 1;
            let Some(want) = c.files.get(c.index) else {
                c.expect(false);
                return 0;
            };
            c.expect(same(wstr(info.name), want.name));
            c.expect(info.size as usize == want.data.len());
            c.expect(info.error == 0);
            c.expect(info.date == want.date && info.time == want.time);
            c.expect(info.attributes == FILE_ATTRIBUTE_ARCHIVE);
            c.expect(wstr(p2 as *const u16) == wstr(c.cabinet.as_ptr()));
            if c.index == c.skip {
                c.index += 1;
                return FILEOP_SKIP;
            }
            // `<temp>\<name>`, as Wine's test writes it.
            c.target = path(&c.temp, b"\\");
            let mut name = [0u8; 32];
            name[..want.name.len()].copy_from_slice(want.name);
            c.target = path(&c.target, &name[..want.name.len()]);
            info.target.copy_from_slice(&c.target[..260]);
            FILEOP_DOIT
        }
        SPFILENOTIFY_FILEEXTRACTED => {
            // SAFETY: the FILEPATHS_W setupapi passes.
            let info = unsafe { &*(p1 as *const FilePaths) };
            c.extracted += 1;
            c.expect(wstr(info.source) == wstr(c.cabinet.as_ptr()));
            c.expect(wstr(info.target) == wstr(c.target.as_ptr()));
            c.expect(info.error == 0);
            c.expect(p2 == 0);
            c.index += 1;
            0
        }
        _ => {
            c.expect(false);
            120 // ERROR_CALL_NOT_IMPLEMENTED
        }
    }
}

/// SetupIterateCabinetW(path, 0, callback?, NULL): FALSE and the last
/// error.
fn refused(c: &mut Checks, cabinet: *const u16, with_callback: bool) -> Result<u32, u32> {
    let cb: Option<Callback> = if with_callback { Some(callback) } else { None };
    // SAFETY: plain calls; the callback is never reached for these.
    unsafe {
        SetLastError(0xdead_beef);
        let r = SetupIterateCabinetW(cabinet, 0, cb, core::ptr::null_mut()) as u32;
        c.eq(r, 0)?;
        Ok(GetLastError())
    }
}

/// Wine's test_invalid_parametersW and test_invalid_callbackW.
fn invalid(c: &mut Checks, temp: &[u16]) -> Result<(), u32> {
    for cb in [false, true] {
        let e = refused(c, core::ptr::null(), cb)?;
        c.eq(e, ERROR_INVALID_PARAMETER)?;
        let empty = [0u16; 1];
        let e = refused(c, empty.as_ptr(), cb)?;
        c.eq(e, ERROR_NOT_ENOUGH_MEMORY)?;
        let missing = path(temp, b"gala_setupcab_missing.cab");
        let e = refused(c, missing.as_ptr(), cb)?;
        c.eq(e, ERROR_FILE_NOT_FOUND)?;
        let empty_file = path(temp, b"gala_setupcab_empty.cab");
        c.ok(write_file(&empty_file, &[]), 0)?;
        let e = refused(c, empty_file.as_ptr(), cb)?;
        c.eq(e, ERROR_INVALID_DATA)?;
        // SAFETY: a NUL-terminated path.
        unsafe { DeleteFileW(empty_file.as_ptr()) };
    }
    let cab = path(temp, b"gala_setupcab_valid.cab");
    c.ok(write_file(&cab, &WINE_CAB), 0)?;
    let e = refused(c, cab.as_ptr(), false)?;
    c.eq(e, ERROR_INVALID_DATA)?;
    // SAFETY: a NUL-terminated path.
    unsafe { DeleteFileW(cab.as_ptr()) };
    Ok(())
}

/// Iterates `data` written as `<temp>/./testcab.cab`, extracting each
/// file to `<temp>\<name>` but the `skip`th (usize::MAX: none), and
/// checks what was notified and extracted.
fn iterate(
    c: &mut Checks,
    temp: &[u16],
    data: &[u8],
    files: &'static [Expected],
    skip: usize,
) -> Result<(), u32> {
    let mut context = Context {
        temp: path(temp, b""),
        cabinet: path(temp, b"/./testcab.cab"),
        files,
        wine: core::ptr::eq(files, &WINE_FILES[..]),
        skip,
        index: 0,
        cabinet_infos: 0,
        in_cabinet: 0,
        extracted: 0,
        target: [0; 300],
        mismatch: 0,
    };
    c.ok(write_file(&context.cabinet, data), 0)?;
    // SAFETY: a NUL-terminated path; our callback and its context.
    let r = unsafe {
        SetupIterateCabinetW(context.cabinet.as_ptr(), 0, Some(callback), (&raw mut context).cast())
    };
    c.eq(r as u32, 1)?;
    c.eq(context.mismatch, 0)?;
    c.eq(context.cabinet_infos, 1)?;
    c.eq(context.in_cabinet, files.len() as u32)?;
    let skipped = u32::from(skip < files.len());
    c.eq(context.extracted, files.len() as u32 - skipped)?;
    for (i, f) in files.iter().enumerate() {
        let mut name = path(temp, b"\\");
        let mut tail = [0u8; 32];
        tail[..f.name.len()].copy_from_slice(f.name);
        name = path(&name, &tail[..f.name.len()]);
        if i == skip {
            // SAFETY: a NUL-terminated path.
            let attributes = unsafe { GetFileAttributesW(name.as_ptr()) };
            c.eq(attributes, INVALID_FILE_ATTRIBUTES)?;
        } else {
            c.ok(holds(&name, f.data), i as u64)?;
            // SAFETY: a NUL-terminated path.
            c.ok(unsafe { DeleteFileW(name.as_ptr()) } != 0, i as u64)?;
        }
    }
    // SAFETY: a NUL-terminated path.
    c.ok(unsafe { DeleteFileW(context.cabinet.as_ptr()) } != 0, 0)?;
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let mut temp = [0u16; 300];
    // SAFETY: our buffer.
    let n = unsafe { GetTempPathW(260, temp.as_mut_ptr()) };
    c.ok(n > 0 && n < 260, n.into())?;
    invalid(&mut c, &temp)?;
    iterate(&mut c, &temp, &WINE_CAB, &WINE_FILES, usize::MAX)?;
    iterate(&mut c, &temp, &WINE_CAB, &WINE_FILES, 1)?;
    let signed = include_bytes!("../../wintrust_test/data/selfsigned.cab");
    iterate(&mut c, &temp, signed, &SIGNED_FILES, usize::MAX)?;
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
