//! File data written lazily (write-back caching, ROADMAP D4), as Windows'
//! cache manager does: a write goes to the cache and returns; the data
//! reaches the disk in the background within seconds, at once through
//! FlushFileBuffers, on a handle opened write-through
//! (FILE_FLAG_WRITE_THROUGH) and without buffering
//! (FILE_FLAG_NO_BUFFERING).
//!
//! Without arguments (run on Windows and gala alike; files in %TEMP%),
//! what a program sees, as Microsoft's documentation of CreateFile,
//! FlushFileBuffers and FILE_MODE_INFORMATION describes it:
//!
//! - data written through one handle is visible at once through another
//!   (contents and GetFileSizeEx), an extension (SetEndOfFile) too, and
//!   reads as zeros past the old end;
//! - FlushFileBuffers succeeds on a handle with write access and fails
//!   with ERROR_ACCESS_DENIED on a read-only one;
//! - FileModeInformation (NtQueryInformationFile, class 16): a plain
//!   handle FILE_SYNCHRONOUS_IO_NONALERT (0x20), FILE_FLAG_WRITE_THROUGH
//!   adds FILE_WRITE_THROUGH (0x2), FILE_FLAG_NO_BUFFERING
//!   FILE_NO_INTERMEDIATE_BUFFERING (0x8);
//! - without buffering, a read or write whose length is not a multiple of
//!   the sector size (512) fails with ERROR_INVALID_PARAMETER; whole
//!   sectors from a page-aligned buffer are written and read back.
//!
//! `crash` and `lazy` are the disk scenarios (gala only: the `powercut`
//! boot option stops the machine at the program's end without writing
//! what is pending, a power failure). Each tells its boots apart by what
//! it finds on C:.
//!
//! - `crash` (with `writeback_ms=600000`: no write-back by time): the
//!   first boot writes C:\wbcrash\flushed.bin (300,000 bytes,
//!   FlushFileBuffers), through.bin (a write-through handle) and
//!   sub\new.txt (renamed from old.txt before the last flush), then, not
//!   flushed, a new lost.bin, a change to flushed.bin's first page and a
//!   rename of sub\new.txt to sub\gone.txt; it prints `writeback_test:
//!   crash written`. The second boot finds what was flushed whole and
//!   nothing of what was not: the tree as it was at the last flush.
//! - `lazy` (the default cadence): the first boot writes
//!   C:\wblazy\lazy.bin and waits 4 s without flushing, then prints
//!   `writeback_test: lazy written`; the second boot finds it whole (the
//!   background write-back put it on the disk).
//!
//! Prints `writeback_test: ok`, or `writeback_test: check N got X (line
//! L)` and exits with N.

#![no_std]
#![no_main]

use core::ptr::null_mut;

use ntrt as _;

const NAME: &str = "writeback_test";

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetTempPathW(size: u32, buffer: *mut u16) -> u32;
    fn GetCommandLineW() -> *const u16;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: usize,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CreateDirectoryW(path: *const u16, security: usize) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn DeleteFileW(path: *const u16) -> i32;
    fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    fn FlushFileBuffers(h: Handle) -> i32;
    fn GetFileSizeEx(h: Handle, size: *mut i64) -> i32;
    fn SetFilePointerEx(h: Handle, distance: i64, new: *mut i64, method: u32) -> i32;
    fn SetEndOfFile(h: Handle) -> i32;
    fn GetLastError() -> u32;
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn Sleep(ms: u32);
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQueryInformationFile(
        h: Handle,
        iosb: *mut [usize; 2],
        info: *mut u8,
        length: u32,
        class: u32,
    ) -> u32;
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const SHARE_ALL: u32 = 7;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;
const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
const INVALID_HANDLE_VALUE: Handle = usize::MAX;
const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INVALID_PARAMETER: u32 = 87;
const FILE_MODE_INFORMATION: u32 = 16;
const FILE_WRITE_THROUGH: u32 = 0x2;
const FILE_NO_INTERMEDIATE_BUFFERING: u32 = 0x8;
const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x20;
const MOVEFILE_REPLACE_EXISTING: u32 = 1;

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

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

/// A path: `%TEMP%<name>` (`temp`) or a fixed one (`path`), NUL-terminated.
struct Path([u16; 300]);

impl Path {
    fn temp(name: &str) -> Self {
        let mut p = [0u16; 300];
        // SAFETY: our buffer of 260 characters.
        let n = unsafe { GetTempPathW(260, p.as_mut_ptr()) } as usize;
        for (i, b) in name.bytes().enumerate() {
            p[n + i] = b.into();
        }
        Self(p)
    }

    fn of(s: &str) -> Self {
        let mut p = [0u16; 300];
        for (i, b) in s.bytes().enumerate() {
            p[i] = b.into();
        }
        Self(p)
    }

    fn ptr(&self) -> *const u16 {
        self.0.as_ptr()
    }

    fn exists(&self) -> bool {
        // SAFETY: a NUL-terminated path.
        unsafe { GetFileAttributesW(self.ptr()) != INVALID_FILE_ATTRIBUTES }
    }
}

fn open(path: &Path, access: u32, disposition: u32, flags: u32) -> Handle {
    // SAFETY: a NUL-terminated path.
    unsafe { CreateFileW(path.ptr(), access, SHARE_ALL, 0, disposition, flags, 0) }
}

/// Writes all of `data` at the handle's position; whether it did.
fn write(h: Handle, data: &[u8]) -> bool {
    let mut n = 0u32;
    // SAFETY: our buffer and handle.
    let ok = unsafe { WriteFile(h, data.as_ptr(), data.len() as u32, &mut n, 0) };
    ok != 0 && n as usize == data.len()
}

/// Reads up to `buf.len()` bytes at the handle's position: how many, or
/// None with the last error.
fn read(h: Handle, buf: &mut [u8]) -> Result<usize, u32> {
    let mut n = 0u32;
    // SAFETY: our buffer and handle.
    let ok = unsafe { ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut n, 0) };
    // SAFETY: reads the thread's last error.
    if ok == 0 { Err(unsafe { GetLastError() }) } else { Ok(n as usize) }
}

fn seek(h: Handle, at: i64) {
    // SAFETY: our handle.
    unsafe { SetFilePointerEx(h, at, null_mut(), 0) };
}

fn size(h: Handle) -> u64 {
    let mut s = -1i64;
    // SAFETY: our handle and output.
    unsafe { GetFileSizeEx(h, &mut s) };
    s as u64
}

/// FileModeInformation of `h` (u32::MAX on a failure).
fn mode(h: Handle) -> u32 {
    let mut iosb = [0usize; 2];
    let mut m = [0u8; 4];
    // SAFETY: our handle, status block and buffer of the class's size.
    let status =
        unsafe { NtQueryInformationFile(h, &mut iosb, m.as_mut_ptr(), 4, FILE_MODE_INFORMATION) };
    if status != 0 { u32::MAX } else { u32::from_le_bytes(m) }
}

/// Byte `i` of the pattern a test file holds (`seed` tells files apart).
fn pattern(seed: u8, i: usize) -> u8 {
    (i.wrapping_mul(31) ^ (i >> 8)) as u8 ^ seed
}

fn close(h: Handle) {
    // SAFETY: our handle.
    unsafe { CloseHandle(h) };
}

/// A page-aligned buffer (FILE_FLAG_NO_BUFFERING's transfers).
#[repr(C, align(4096))]
struct Aligned([u8; 8192]);

/// Checks that `h` reads from the start as `len` bytes of `seed`'s pattern.
fn holds(c: &mut Checks, h: Handle, seed: u8, len: usize) -> Result<(), u32> {
    c.eq(size(h), len as u64)?;
    seek(h, 0);
    let mut buf = [0u8; 4096];
    let mut at = 0;
    while at < len {
        let n = read(h, &mut buf).unwrap_or(0);
        c.ok(n > 0, at as u64)?;
        for (k, &b) in buf[..n].iter().enumerate() {
            if b != pattern(seed, at + k) {
                return c.ok(false, (at + k) as u64);
            }
        }
        at += n;
    }
    Ok(())
}

/// Writes `len` bytes of `seed`'s pattern from the handle's position 0.
fn fill(h: Handle, seed: u8, len: usize) -> bool {
    let mut buf = [0u8; 4096];
    let mut at = 0;
    while at < len {
        let n = (len - at).min(buf.len());
        for (k, b) in buf[..n].iter_mut().enumerate() {
            *b = pattern(seed, at + k);
        }
        if !write(h, &buf[..n]) {
            return false;
        }
        at += n;
    }
    true
}

fn visible(c: &mut Checks) -> Result<(), u32> {
    let path = Path::temp("writeback_test_a.bin");
    let a = open(&path, GENERIC_READ | GENERIC_WRITE, CREATE_ALWAYS, 0);
    c.ok(a != INVALID_HANDLE_VALUE, 0)?;
    let b = open(&path, GENERIC_READ, OPEN_EXISTING, 0);
    c.ok(b != INVALID_HANDLE_VALUE, 0)?;
    c.eq(mode(a).into(), FILE_SYNCHRONOUS_IO_NONALERT.into())?;
    // Written through one handle, seen through the other at once.
    c.ok(fill(a, 1, 10_000), 0)?;
    holds(c, b, 1, 10_000)?;
    // An extension: the size, and zeros past the old end.
    seek(a, 20_000);
    // SAFETY: our handle.
    c.ok(unsafe { SetEndOfFile(a) } != 0, 0)?;
    c.eq(size(b), 20_000)?;
    seek(b, 15_000);
    let mut buf = [0xffu8; 100];
    c.eq(read(b, &mut buf).unwrap_or(0) as u64, 100)?;
    c.ok(buf.iter().all(|&x| x == 0), buf[0].into())?;
    // FlushFileBuffers: write access needed.
    // SAFETY: our handles.
    unsafe {
        c.ok(FlushFileBuffers(a) != 0, GetLastError().into())?;
        c.eq(FlushFileBuffers(b) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_ACCESS_DENIED.into())?;
    }
    close(a);
    close(b);
    // SAFETY: a NUL-terminated path.
    unsafe { DeleteFileW(path.ptr()) };
    Ok(())
}

fn write_through(c: &mut Checks) -> Result<(), u32> {
    let path = Path::temp("writeback_test_w.bin");
    let w = open(&path, GENERIC_READ | GENERIC_WRITE, CREATE_ALWAYS, FILE_FLAG_WRITE_THROUGH);
    c.ok(w != INVALID_HANDLE_VALUE, 0)?;
    c.eq(mode(w).into(), (FILE_SYNCHRONOUS_IO_NONALERT | FILE_WRITE_THROUGH).into())?;
    c.ok(fill(w, 2, 5000), 0)?;
    let r = open(&path, GENERIC_READ, OPEN_EXISTING, 0);
    c.ok(r != INVALID_HANDLE_VALUE, 0)?;
    c.eq(mode(r).into(), FILE_SYNCHRONOUS_IO_NONALERT.into())?;
    holds(c, r, 2, 5000)?;
    close(r);
    close(w);
    // SAFETY: a NUL-terminated path.
    unsafe { DeleteFileW(path.ptr()) };
    Ok(())
}

fn no_buffering(c: &mut Checks) -> Result<(), u32> {
    let path = Path::temp("writeback_test_n.bin");
    let h = open(&path, GENERIC_READ | GENERIC_WRITE, CREATE_ALWAYS, FILE_FLAG_NO_BUFFERING);
    c.ok(h != INVALID_HANDLE_VALUE, 0)?;
    c.eq(mode(h).into(), (FILE_SYNCHRONOUS_IO_NONALERT | FILE_NO_INTERMEDIATE_BUFFERING).into())?;
    let mut aligned = Aligned([0; 8192]);
    let buf = &mut aligned;
    for (i, b) in buf.0.iter_mut().enumerate() {
        *b = pattern(3, i);
    }
    let mut n = 0u32;
    // Not a whole number of sectors.
    // SAFETY: our handle and aligned buffer.
    unsafe {
        c.eq(WriteFile(h, buf.0.as_ptr(), 100, &mut n, 0) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_PARAMETER.into())?;
        c.ok(WriteFile(h, buf.0.as_ptr(), 4096, &mut n, 0) != 0, GetLastError().into())?;
    }
    c.eq(n.into(), 4096)?;
    c.eq(size(h), 4096)?;
    seek(h, 0);
    let back = &mut buf.0[4096..];
    c.eq(read(h, &mut back[..100]).err().unwrap_or(0).into(), ERROR_INVALID_PARAMETER.into())?;
    seek(h, 0);
    c.eq(read(h, &mut back[..4096]).unwrap_or(0) as u64, 4096)?;
    let (first, second) = buf.0.split_at(4096);
    c.ok(first == second, 0)?;
    close(h);
    // SAFETY: a NUL-terminated path.
    unsafe { DeleteFileW(path.ptr()) };
    Ok(())
}

/// `crash`'s first boot (see the module comment).
fn crash_write(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: NUL-terminated paths.
    unsafe {
        c.ok(CreateDirectoryW(Path::of("C:\\wbcrash").ptr(), 0) != 0, GetLastError().into())?;
        c.ok(CreateDirectoryW(Path::of("C:\\wbcrash\\sub").ptr(), 0) != 0, 0)?;
    }
    let old = open(&Path::of("C:\\wbcrash\\old.txt"), GENERIC_WRITE, CREATE_ALWAYS, 0);
    c.ok(old != INVALID_HANDLE_VALUE, 0)?;
    c.ok(fill(old, 4, 700), 0)?;
    close(old);
    // SAFETY: NUL-terminated paths.
    c.ok(
        unsafe {
            MoveFileExW(
                Path::of("C:\\wbcrash\\old.txt").ptr(),
                Path::of("C:\\wbcrash\\sub\\new.txt").ptr(),
                0,
            )
        } != 0,
        0,
    )?;
    let f = open(&Path::of("C:\\wbcrash\\flushed.bin"), GENERIC_WRITE, CREATE_ALWAYS, 0);
    c.ok(f != INVALID_HANDLE_VALUE, 0)?;
    c.ok(fill(f, 5, 300_000), 0)?;
    // SAFETY: our handle.
    c.ok(unsafe { FlushFileBuffers(f) } != 0, 0)?;
    let t = open(
        &Path::of("C:\\wbcrash\\through.bin"),
        GENERIC_WRITE,
        CREATE_ALWAYS,
        FILE_FLAG_WRITE_THROUGH,
    );
    c.ok(t != INVALID_HANDLE_VALUE, 0)?;
    c.ok(fill(t, 6, 5000), 0)?;
    close(t);
    // Not flushed from here on.
    let l = open(&Path::of("C:\\wbcrash\\lost.bin"), GENERIC_WRITE, CREATE_ALWAYS, 0);
    c.ok(l != INVALID_HANDLE_VALUE, 0)?;
    c.ok(fill(l, 7, 100_000), 0)?;
    close(l);
    seek(f, 0);
    c.ok(write(f, &[0xee; 4096]), 0)?;
    close(f);
    // SAFETY: NUL-terminated paths.
    c.ok(
        unsafe {
            MoveFileExW(
                Path::of("C:\\wbcrash\\sub\\new.txt").ptr(),
                Path::of("C:\\wbcrash\\sub\\gone.txt").ptr(),
                MOVEFILE_REPLACE_EXISTING,
            )
        } != 0,
        0,
    )?;
    Line::new().s(NAME.as_bytes()).s(b": crash written\r\n").print();
    Ok(())
}

/// `crash`'s second boot: the tree as it was at the last flush.
fn crash_check(c: &mut Checks) -> Result<(), u32> {
    let f = open(&Path::of("C:\\wbcrash\\flushed.bin"), GENERIC_READ, OPEN_EXISTING, 0);
    c.ok(f != INVALID_HANDLE_VALUE, 0)?;
    holds(c, f, 5, 300_000)?;
    close(f);
    let t = open(&Path::of("C:\\wbcrash\\through.bin"), GENERIC_READ, OPEN_EXISTING, 0);
    c.ok(t != INVALID_HANDLE_VALUE, 0)?;
    holds(c, t, 6, 5000)?;
    close(t);
    let n = open(&Path::of("C:\\wbcrash\\sub\\new.txt"), GENERIC_READ, OPEN_EXISTING, 0);
    c.ok(n != INVALID_HANDLE_VALUE, 0)?;
    holds(c, n, 4, 700)?;
    close(n);
    c.ok(!Path::of("C:\\wbcrash\\old.txt").exists(), 0)?;
    c.ok(!Path::of("C:\\wbcrash\\sub\\gone.txt").exists(), 0)?;
    c.ok(!Path::of("C:\\wbcrash\\lost.bin").exists(), 0)?;
    Ok(())
}

/// `lazy`: the first boot writes and waits, the second finds the data.
fn lazy(c: &mut Checks) -> Result<(), u32> {
    let path = Path::of("C:\\wblazy\\lazy.bin");
    if Path::of("C:\\wblazy").exists() {
        let h = open(&path, GENERIC_READ, OPEN_EXISTING, 0);
        c.ok(h != INVALID_HANDLE_VALUE, 0)?;
        holds(c, h, 8, 200_000)?;
        close(h);
        return Ok(());
    }
    // SAFETY: a NUL-terminated path.
    c.ok(unsafe { CreateDirectoryW(Path::of("C:\\wblazy").ptr(), 0) } != 0, 0)?;
    let h = open(&path, GENERIC_WRITE, CREATE_ALWAYS, 0);
    c.ok(h != INVALID_HANDLE_VALUE, 0)?;
    c.ok(fill(h, 8, 200_000), 0)?;
    close(h);
    // SAFETY: waits.
    unsafe { Sleep(4000) };
    Line::new().s(NAME.as_bytes()).s(b": lazy written\r\n").print();
    Ok(())
}

/// The first argument after the program's name, if any.
fn argument() -> [u8; 8] {
    let mut out = [0u8; 8];
    // SAFETY: the process's NUL-terminated command line.
    unsafe {
        let p = GetCommandLineW();
        let mut i = 0;
        let mut quoted = false;
        // Past the program's name.
        while *p.add(i) != 0 && (quoted || *p.add(i) != u16::from(b' ')) {
            if *p.add(i) == u16::from(b'"') {
                quoted = !quoted;
            }
            i += 1;
        }
        while *p.add(i) == u16::from(b' ') {
            i += 1;
        }
        let mut k = 0;
        while *p.add(i) != 0 && *p.add(i) != u16::from(b' ') && k < out.len() {
            out[k] = *p.add(i) as u8;
            i += 1;
            k += 1;
        }
    }
    out
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let arg = argument();
    if arg.starts_with(b"crash\0") {
        return if Path::of("C:\\wbcrash").exists() {
            crash_check(&mut c)
        } else {
            crash_write(&mut c)
        };
    }
    if arg.starts_with(b"lazy\0") {
        return lazy(&mut c);
    }
    visible(&mut c)?;
    write_through(&mut c)?;
    no_buffering(&mut c)?;
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
