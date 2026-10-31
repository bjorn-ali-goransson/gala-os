//! stdio: the three console FILE objects of `__iob_func` (stdin, stdout,
//! stderr) and files opened with fopen/_wfopen (unbuffered for writing; a
//! read buffer and one pushed-back byte for reading; no text-mode CRLF
//! translation), the low-level descriptors (`_fileno`, `_read`, ...),
//! `_stat64`, and the printf family with its formatting engine, writing to
//! a handle, a byte buffer or a wide buffer.
//!
//! fprintf, printf and sprintf are C-variadic, which stable Rust cannot
//! define: the assembly at the end stores the register arguments in the
//! caller's home area (which, with the stack arguments after it, is the
//! Win64 `va_list`) and calls the `v` variant.

use crate::heap::{free, malloc};
use core::ffi::c_void;
use core::ptr::null_mut;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> *mut c_void;
    fn WriteFile(
        h: *mut c_void,
        buf: *const u8,
        len: u32,
        written: *mut u32,
        ov: *mut c_void,
    ) -> i32;
    fn ReadFile(h: *mut c_void, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> i32;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        attributes: *mut c_void,
        disposition: u32,
        flags: u32,
        template: *mut c_void,
    ) -> *mut c_void;
    fn CloseHandle(h: *mut c_void) -> i32;
    fn SetFilePointerEx(h: *mut c_void, distance: i64, new: *mut i64, method: u32) -> i32;
    fn GetFileAttributesExW(path: *const u16, level: u32, info: *mut u8) -> i32;
    fn GetLastError() -> u32;
}

const INVALID_HANDLE: *mut c_void = -1isize as *mut c_void;

// ---- errno -----------------------------------------------------------------

/// errno (per process here; the CRT keeps it per thread).
static mut ERRNO: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn _errno() -> *mut i32 {
    &raw mut ERRNO
}

fn set_errno(e: i32) {
    // SAFETY: a plain integer.
    unsafe { ERRNO = e };
}

const ENOENT: i32 = 2;
const EBADF: i32 = 9;
const EACCES: i32 = 13;
const EEXIST: i32 = 17;
const EINVAL: i32 = 22;

/// errno for the thread's last Win32 error.
fn errno_of_last_error() -> i32 {
    // SAFETY: plain call.
    match unsafe { GetLastError() } {
        2 | 3 => ENOENT,
        5 | 32 => EACCES,
        80 | 183 => EEXIST,
        6 => EBADF,
        _ => EINVAL,
    }
}

// ---- FILE objects ------------------------------------------------------------

/// msvcrt's FILE is 48 bytes on x64: programs index `__iob_func()` with
/// that size, so the console streams are 48-byte slots; their state lives
/// in STD.
const FILE_SIZE: usize = 48;
static mut IOB: [[u8; FILE_SIZE]; 3] = [[0; FILE_SIZE]; 3];

const MAGIC: u64 = 0x656c_6966_616c_6167; // "galafile"
const READ_BUF: usize = 4096;

/// The state of a stream. Files from fopen are this struct itself.
#[repr(C)]
struct Stream {
    magic: u64,
    handle: *mut c_void,
    fd: i32,
    eof: bool,
    error: bool,
    unget: Option<u8>,
    pos: usize,
    len: usize,
    buf: [u8; READ_BUF],
}

impl Stream {
    const fn new(handle: *mut c_void, fd: i32) -> Self {
        Self {
            magic: MAGIC,
            handle,
            fd,
            eof: false,
            error: false,
            unget: None,
            pos: 0,
            len: 0,
            buf: [0; READ_BUF],
        }
    }
}

static mut STD: [Stream; 3] =
    [Stream::new(null_mut(), 0), Stream::new(null_mut(), 1), Stream::new(null_mut(), 2)];

/// __iob_func(): the stdin, stdout and stderr FILE objects.
#[unsafe(no_mangle)]
pub extern "C" fn __iob_func() -> *mut u8 {
    (&raw mut IOB).cast()
}

/// The stream behind a FILE pointer (a console slot or one of ours).
fn stream<'a>(file: *mut u8) -> Option<&'a mut Stream> {
    let base = (&raw mut IOB) as usize;
    if let Some(at) = (file as usize).checked_sub(base)
        && at < 3 * FILE_SIZE
    {
        let i = at / FILE_SIZE;
        // SAFETY: the console streams' state; the handle is looked up now
        // (SetStdHandle may change it).
        unsafe {
            let s = &mut (*(&raw mut STD))[i];
            s.handle = GetStdHandle((-10 - i as i32) as u32);
            return Some(s);
        }
    }
    if file.is_null() {
        return None;
    }
    // SAFETY: a FILE from fopen starts with our magic.
    let s = unsafe { &mut *file.cast::<Stream>() };
    (s.magic == MAGIC).then_some(s)
}

fn write_handle(h: *mut c_void, data: &[u8]) -> bool {
    let mut written = 0;
    // SAFETY: a valid buffer.
    unsafe { WriteFile(h, data.as_ptr(), data.len() as u32, &mut written, null_mut()) != 0 }
}

pub(crate) fn write_stderr(data: &[u8]) {
    // SAFETY: plain call.
    write_handle(unsafe { GetStdHandle(-12i32 as u32) }, data);
}

/// UTF-8 (the ANSI code page here) to NUL-terminated UTF-16.
fn wide_path(s: &[u8], out: &mut [u16; 1024]) -> Option<()> {
    let text = core::str::from_utf8(s).ok()?;
    let mut n = 0;
    for c in text.encode_utf16() {
        *out.get_mut(n)? = c;
        n += 1;
    }
    *out.get_mut(n)? = 0;
    Some(())
}

/// fopen's mode: (access, disposition, append).
fn parse_mode(mode: &[u8]) -> Option<(u32, u32, bool)> {
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    let plus = mode.contains(&b'+');
    Some(match mode.first()? {
        b'r' => (if plus { GENERIC_READ | GENERIC_WRITE } else { GENERIC_READ }, 3, false), // OPEN_EXISTING
        b'w' => (if plus { GENERIC_READ | GENERIC_WRITE } else { GENERIC_WRITE }, 2, false), // CREATE_ALWAYS
        b'a' => (if plus { GENERIC_READ | GENERIC_WRITE } else { GENERIC_WRITE }, 4, true), // OPEN_ALWAYS
        _ => return None,
    })
}

fn open(path: &[u16], mode: &[u8]) -> *mut u8 {
    let Some((access, disposition, append)) = parse_mode(mode) else {
        set_errno(EINVAL);
        return null_mut();
    };
    // SAFETY: a NUL-terminated path.
    let h =
        unsafe { CreateFileW(path.as_ptr(), access, 3, null_mut(), disposition, 0x80, null_mut()) };
    if h == INVALID_HANDLE {
        set_errno(errno_of_last_error());
        return null_mut();
    }
    if append {
        // SAFETY: our handle.
        unsafe { SetFilePointerEx(h, 0, null_mut(), 2) };
    }
    let file = malloc(size_of::<Stream>()).cast::<Stream>();
    if file.is_null() {
        // SAFETY: our handle.
        unsafe { CloseHandle(h) };
        return null_mut();
    }
    let fd = new_fd(h);
    // SAFETY: a fresh allocation of the right size.
    unsafe { file.write(Stream::new(h, fd)) };
    file.cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn fopen(path: *const u8, mode: *const u8) -> *mut u8 {
    let mut wide = [0u16; 1024];
    // SAFETY: the caller's NUL-terminated strings.
    let (path, mode) = unsafe { (crate::c_str(path), crate::c_str(mode)) };
    if wide_path(path, &mut wide).is_none() {
        set_errno(EINVAL);
        return null_mut();
    }
    open(&wide, mode)
}

#[unsafe(no_mangle)]
pub extern "C" fn _wfopen(path: *const u16, mode: *const u16) -> *mut u8 {
    let mut wide = [0u16; 1024];
    let mut narrow_mode = [0u8; 16];
    // SAFETY: the caller's NUL-terminated wide strings.
    unsafe {
        let mut n = 0;
        while *path.add(n) != 0 && n < 1023 {
            wide[n] = *path.add(n);
            n += 1;
        }
        let mut m = 0;
        while *mode.add(m) != 0 && m < 15 {
            narrow_mode[m] = *mode.add(m) as u8;
            m += 1;
        }
        open(&wide, &narrow_mode[..m])
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn fclose(file: *mut u8) -> i32 {
    let Some(s) = stream(file) else { return -1 };
    if s.fd > 2 {
        close_fd(s.fd);
        s.magic = 0;
        free(file);
    }
    0
}

/// Reads up to `out.len()` bytes (the pushed-back byte, the buffer, then
/// the handle); 0 at end of file.
fn read_some(s: &mut Stream, out: &mut [u8]) -> usize {
    let mut n = 0;
    if let Some(c) = s.unget.take()
        && !out.is_empty()
    {
        out[0] = c;
        n = 1;
    }
    while n < out.len() {
        if s.pos == s.len {
            let mut got = 0u32;
            // SAFETY: our buffer.
            let ok = unsafe {
                ReadFile(s.handle, s.buf.as_mut_ptr(), READ_BUF as u32, &mut got, null_mut())
            };
            if ok == 0 {
                s.error = true;
                break;
            }
            if got == 0 {
                s.eof = true;
                break;
            }
            (s.pos, s.len) = (0, got as usize);
        }
        let k = (s.len - s.pos).min(out.len() - n);
        out[n..n + k].copy_from_slice(&s.buf[s.pos..s.pos + k]);
        s.pos += k;
        n += k;
    }
    n
}

#[unsafe(no_mangle)]
pub extern "C" fn fread(buf: *mut u8, size: usize, count: usize, file: *mut u8) -> usize {
    let Some(s) = stream(file) else { return 0 };
    let total = size.saturating_mul(count);
    if total == 0 {
        return 0;
    }
    // SAFETY: the caller's buffer of size * count bytes.
    let out = unsafe { core::slice::from_raw_parts_mut(buf, total) };
    read_some(s, out) / size
}

#[unsafe(no_mangle)]
pub extern "C" fn getc(file: *mut u8) -> i32 {
    let Some(s) = stream(file) else { return -1 };
    let mut c = [0u8];
    if read_some(s, &mut c) == 1 { i32::from(c[0]) } else { -1 }
}

#[unsafe(no_mangle)]
pub extern "C" fn fgetc(file: *mut u8) -> i32 {
    getc(file)
}

#[unsafe(no_mangle)]
pub extern "C" fn ungetc(c: i32, file: *mut u8) -> i32 {
    let Some(s) = stream(file) else { return -1 };
    if c < 0 || s.unget.is_some() {
        return -1;
    }
    s.unget = Some(c as u8);
    s.eof = false;
    c
}

/// fgets: up to `n - 1` bytes, through the first newline.
#[unsafe(no_mangle)]
pub extern "C" fn fgets(buf: *mut u8, n: i32, file: *mut u8) -> *mut u8 {
    if n <= 0 {
        return null_mut();
    }
    let mut i = 0;
    while i + 1 < n as usize {
        let c = getc(file);
        if c < 0 {
            break;
        }
        // SAFETY: i < n - 1 bytes of the caller's buffer.
        unsafe { *buf.add(i) = c as u8 };
        i += 1;
        if c == i32::from(b'\n') {
            break;
        }
    }
    if i == 0 {
        return null_mut();
    }
    // SAFETY: i < n.
    unsafe { *buf.add(i) = 0 };
    buf
}

#[unsafe(no_mangle)]
pub extern "C" fn feof(file: *mut u8) -> i32 {
    stream(file).map_or(0, |s| i32::from(s.eof))
}

#[unsafe(no_mangle)]
pub extern "C" fn ferror(file: *mut u8) -> i32 {
    stream(file).map_or(1, |s| i32::from(s.error))
}

#[unsafe(no_mangle)]
pub extern "C" fn fwrite(buf: *const u8, size: usize, count: usize, file: *mut u8) -> usize {
    let Some(s) = stream(file) else { return 0 };
    let total = size.saturating_mul(count);
    if total == 0 {
        return 0;
    }
    // SAFETY: the caller's buffer of size * count bytes.
    let data = unsafe { core::slice::from_raw_parts(buf, total) };
    if write_handle(s.handle, data) {
        count
    } else {
        s.error = true;
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn fputc(c: i32, file: *mut u8) -> i32 {
    let b = [c as u8];
    if fwrite(b.as_ptr(), 1, 1, file) == 1 { c & 0xff } else { -1 }
}

#[unsafe(no_mangle)]
pub extern "C" fn fputs(s: *const u8, file: *mut u8) -> i32 {
    // SAFETY: the caller's NUL-terminated string.
    let s = unsafe { crate::c_str(s) };
    if fwrite(s.as_ptr(), 1, s.len(), file) == s.len() { 0 } else { -1 }
}

#[unsafe(no_mangle)]
pub extern "C" fn fflush(_file: *mut u8) -> i32 {
    0 // writes are unbuffered
}

/// fseek: drops what was read ahead; SEEK_SET/CUR/END (0, 1, 2).
#[unsafe(no_mangle)]
pub extern "C" fn fseek(file: *mut u8, offset: i32, whence: i32) -> i32 {
    let Some(s) = stream(file) else { return -1 };
    let mut offset = i64::from(offset);
    if whence == 1 {
        // Relative to the caller's position, not the read-ahead.
        offset -= (s.len - s.pos) as i64 + i64::from(s.unget.is_some());
    }
    (s.pos, s.len, s.unget, s.eof) = (0, 0, None, false);
    // SAFETY: our handle.
    if unsafe { SetFilePointerEx(s.handle, offset, null_mut(), whence as u32) } == 0 {
        set_errno(EINVAL);
        return -1;
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ftell(file: *mut u8) -> i32 {
    let Some(s) = stream(file) else { return -1 };
    let mut at = 0i64;
    // SAFETY: our handle.
    if unsafe { SetFilePointerEx(s.handle, 0, &mut at, 1) } == 0 {
        return -1;
    }
    (at - (s.len - s.pos) as i64 - i64::from(s.unget.is_some())) as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn _fileno(file: *mut u8) -> i32 {
    stream(file).map_or(-1, |s| s.fd)
}

// ---- Low-level descriptors ------------------------------------------------------

const MAX_FDS: usize = 64;
/// Handles of descriptors 3 and up (0-2 are the console's).
static mut FDS: [usize; MAX_FDS] = [0; MAX_FDS];

fn new_fd(h: *mut c_void) -> i32 {
    // SAFETY: the table (descriptors are rare; races would need two
    // threads opening files at once).
    unsafe {
        let fds = &mut *(&raw mut FDS);
        match (3..MAX_FDS).find(|&i| fds[i] == 0) {
            Some(i) => {
                fds[i] = h as usize;
                i as i32
            }
            None => -1,
        }
    }
}

fn close_fd(fd: i32) {
    if let Some(h) = handle_of_fd(fd) {
        // SAFETY: the descriptor's handle.
        unsafe {
            CloseHandle(h);
            (*(&raw mut FDS))[fd as usize] = 0;
        }
    }
}

fn handle_of_fd(fd: i32) -> Option<*mut c_void> {
    match fd {
        // SAFETY: plain calls.
        0..=2 => Some(unsafe { GetStdHandle((-10 - fd) as u32) }),
        3.. if (fd as usize) < MAX_FDS => {
            // SAFETY: the table.
            let h = unsafe { (*(&raw const FDS))[fd as usize] };
            (h != 0).then_some(h as *mut c_void)
        }
        _ => None,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _get_osfhandle(fd: i32) -> isize {
    handle_of_fd(fd).map_or(-1, |h| h as isize)
}

#[unsafe(no_mangle)]
pub extern "C" fn _read(fd: i32, buf: *mut u8, count: u32) -> i32 {
    let Some(h) = handle_of_fd(fd) else {
        set_errno(EBADF);
        return -1;
    };
    let mut got = 0u32;
    // SAFETY: the caller's buffer.
    if unsafe { ReadFile(h, buf, count, &mut got, null_mut()) } == 0 {
        set_errno(errno_of_last_error());
        return -1;
    }
    got as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn _write(fd: i32, buf: *const u8, count: u32) -> i32 {
    let Some(h) = handle_of_fd(fd) else {
        set_errno(EBADF);
        return -1;
    };
    // SAFETY: the caller's buffer.
    let data = unsafe { core::slice::from_raw_parts(buf, count as usize) };
    if write_handle(h, data) { count as i32 } else { -1 }
}

#[unsafe(no_mangle)]
pub extern "C" fn _lseek(fd: i32, offset: i32, whence: i32) -> i32 {
    let Some(h) = handle_of_fd(fd) else {
        set_errno(EBADF);
        return -1;
    };
    let mut at = 0i64;
    // SAFETY: the descriptor's handle.
    if unsafe { SetFilePointerEx(h, i64::from(offset), &mut at, whence as u32) } == 0 {
        set_errno(EINVAL);
        return -1;
    }
    at as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn _close(fd: i32) -> i32 {
    if fd <= 2 || handle_of_fd(fd).is_none() {
        set_errno(EBADF);
        return -1;
    }
    close_fd(fd);
    0
}

/// _setmode: files are always binary here (no CRLF translation); the
/// previous mode is reported as _O_BINARY.
#[unsafe(no_mangle)]
pub extern "C" fn _setmode(_fd: i32, _mode: i32) -> i32 {
    0x8000
}

/// _stat64(path, buf): size, times and mode from GetFileAttributesExW.
#[unsafe(no_mangle)]
pub extern "C" fn _stat64(path: *const u8, buf: *mut u8) -> i32 {
    let mut wide = [0u16; 1024];
    // SAFETY: the caller's NUL-terminated path.
    if wide_path(unsafe { crate::c_str(path) }, &mut wide).is_none() {
        set_errno(ENOENT);
        return -1;
    }
    // WIN32_FILE_ATTRIBUTE_DATA: attributes, three FILETIMEs, size high/low.
    let mut info = [0u8; 36];
    // SAFETY: a 36-byte buffer.
    if unsafe { GetFileAttributesExW(wide.as_ptr(), 0, info.as_mut_ptr()) } == 0 {
        set_errno(errno_of_last_error());
        return -1;
    }
    let u32_at = |at: usize| u32::from_le_bytes(info[at..at + 4].try_into().unwrap_or([0; 4]));
    let time = |at: usize| {
        let ft = u64::from(u32_at(at)) | (u64::from(u32_at(at + 4)) << 32);
        (ft / 10_000_000) as i64 - 11_644_473_600
    };
    let directory = u32_at(0) & 0x10 != 0;
    let mode: u16 = if directory { 0x4000 | 0o555 } else { 0x8000 | 0o444 }
        | if u32_at(0) & 1 == 0 { 0o222 } else { 0 };
    let size = (u64::from(u32_at(28)) << 32) | u64::from(u32_at(32));
    // struct _stat64: st_dev u32, st_ino u16, st_mode u16, st_nlink i16,
    // st_uid i16, st_gid i16, (pad), st_rdev u32, (pad), st_size i64,
    // st_atime, st_mtime, st_ctime (i64): 56 bytes.
    // SAFETY: the caller's struct _stat64.
    unsafe {
        core::ptr::write_bytes(buf, 0, 56);
        core::ptr::write_unaligned(buf.add(6).cast::<u16>(), mode);
        core::ptr::write_unaligned(buf.add(8).cast::<i16>(), 1);
        core::ptr::write_unaligned(buf.add(24).cast::<i64>(), size as i64);
        core::ptr::write_unaligned(buf.add(32).cast::<i64>(), time(12));
        core::ptr::write_unaligned(buf.add(40).cast::<i64>(), time(20));
        core::ptr::write_unaligned(buf.add(48).cast::<i64>(), time(4));
    }
    0
}

// ---- Formatting ---------------------------------------------------------------------

/// Where formatted output goes.
enum Sink {
    /// A handle, in 256-byte pieces.
    Handle { handle: *mut c_void, buf: [u8; 256], len: usize },
    /// A byte buffer of `cap` bytes (the rest is counted, not written).
    Bytes { ptr: *mut u8, cap: usize },
}

struct Out {
    sink: Sink,
    total: usize,
}

impl Out {
    fn put(&mut self, mut data: &[u8]) {
        let start = self.total;
        self.total += data.len();
        match &mut self.sink {
            Sink::Handle { handle, buf, len } => {
                while !data.is_empty() {
                    let n = data.len().min(buf.len() - *len);
                    buf[*len..*len + n].copy_from_slice(&data[..n]);
                    *len += n;
                    data = &data[n..];
                    if *len == buf.len() {
                        write_handle(*handle, &buf[..]);
                        *len = 0;
                    }
                }
            }
            Sink::Bytes { ptr, cap } => {
                for (i, &b) in data.iter().enumerate() {
                    if start + i < *cap {
                        // SAFETY: inside the caller's buffer.
                        unsafe { *ptr.add(start + i) = b };
                    }
                }
            }
        }
    }

    fn flush(&mut self) {
        if let Sink::Handle { handle, buf, len } = &mut self.sink
            && *len > 0
        {
            write_handle(*handle, &buf[..*len]);
            *len = 0;
        }
    }
}

/// The variadic arguments: 8-byte slots (Win64 `va_list`).
struct Args(*const u64);

impl Args {
    fn next(&mut self) -> u64 {
        // SAFETY: the caller passed an argument for each conversion.
        unsafe {
            let v = *self.0;
            self.0 = self.0.add(1);
            v
        }
    }
}

/// Formats `fmt` with the C printf rules (flags `-+ #0`, width and
/// precision with `*`, lengths hh h l ll I I32 I64 z j t w, conversions
/// d i u o x X c s p % and f/e/g as fixed point). With `wide`, `%s` and
/// `%c` take UTF-16 (and `%hs` narrow), as in the wprintf functions; the
/// output is UTF-8 either way.
fn format(out: &mut Out, fmt: &[u8], mut args: Args, wide: bool) {
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            let start = i;
            while i < fmt.len() && fmt[i] != b'%' {
                i += 1;
            }
            out.put(&fmt[start..i]);
            continue;
        }
        i += 1;
        let (mut left, mut plus, mut space, mut alt, mut zero) =
            (false, false, false, false, false);
        while i < fmt.len() {
            match fmt[i] {
                b'-' => left = true,
                b'+' => plus = true,
                b' ' => space = true,
                b'#' => alt = true,
                b'0' => zero = true,
                _ => break,
            }
            i += 1;
        }
        let mut width = 0usize;
        if i < fmt.len() && fmt[i] == b'*' {
            let w = args.next() as i32;
            if w < 0 {
                left = true;
            }
            width = w.unsigned_abs() as usize;
            i += 1;
        }
        while i < fmt.len() && fmt[i].is_ascii_digit() {
            width = width * 10 + usize::from(fmt[i] - b'0');
            i += 1;
        }
        let mut precision = None;
        if i < fmt.len() && fmt[i] == b'.' {
            i += 1;
            let mut p = 0usize;
            if i < fmt.len() && fmt[i] == b'*' {
                let v = args.next() as i32;
                p = if v < 0 { 0 } else { v as usize };
                i += 1;
            }
            while i < fmt.len() && fmt[i].is_ascii_digit() {
                p = p * 10 + usize::from(fmt[i] - b'0');
                i += 1;
            }
            precision = Some(p);
        }
        // Length: 64-bit or not (`l` is 32-bit on Windows); `h`/`l`/`w`
        // choose narrow or wide strings.
        let (mut wide64, mut narrow, mut wide_str) = (false, false, false);
        loop {
            let rest = &fmt[i..];
            if rest.starts_with(b"I64") {
                wide64 = true;
                i += 3;
            } else if rest.starts_with(b"I32") {
                i += 3;
            } else if rest.starts_with(b"ll") || rest.starts_with(b"hh") {
                wide64 |= rest[0] == b'l';
                i += 2;
            } else if matches!(rest.first(), Some(b'I' | b'z' | b'j' | b't')) {
                wide64 = true;
                i += 1;
            } else if matches!(rest.first(), Some(b'h')) {
                narrow = true;
                i += 1;
            } else if matches!(rest.first(), Some(b'l' | b'w')) {
                wide_str = true;
                i += 1;
            } else if matches!(rest.first(), Some(b'L')) {
                i += 1;
            } else {
                break;
            }
        }
        let Some(&conv) = fmt.get(i) else { break };
        i += 1;
        // Strings and characters in UTF-16: %S (narrow printf), %s/%c
        // (wide printf), %ls/%ws.
        let utf16 = match conv {
            b's' | b'c' => wide_str || (wide && !narrow),
            b'S' | b'C' => !(wide && !narrow) || wide_str,
            _ => false,
        };
        let mut digits = [0u8; 72];
        let mut utf8 = [0u8; 4];
        let (body, prefix): (&[u8], &[u8]) = match conv {
            b'd' | b'i' => {
                let raw = args.next();
                let v = if wide64 { raw as i64 } else { i64::from(raw as i32) };
                let s = number(&mut digits, v.unsigned_abs(), 10, false, precision);
                let sign: &[u8] = if v < 0 {
                    b"-"
                } else if plus {
                    b"+"
                } else if space {
                    b" "
                } else {
                    b""
                };
                (s, sign)
            }
            b'u' | b'o' | b'x' | b'X' => {
                let raw = args.next();
                let v = if wide64 { raw } else { u64::from(raw as u32) };
                let base = match conv {
                    b'o' => 8,
                    b'u' => 10,
                    _ => 16,
                };
                let s = number(&mut digits, v, base, conv == b'X', precision);
                let p: &[u8] = match conv {
                    b'x' if alt && v != 0 => b"0x",
                    b'X' if alt && v != 0 => b"0X",
                    b'o' if alt => b"0",
                    _ => b"",
                };
                (s, p)
            }
            b'p' => (number(&mut digits, args.next(), 16, true, Some(16)), b""),
            b'c' | b'C' if utf16 => {
                let c = char::from_u32(args.next() as u32 & 0xffff).unwrap_or('?');
                (c.encode_utf8(&mut utf8).as_bytes(), b"")
            }
            b'c' | b'C' => {
                digits[0] = args.next() as u8;
                (&digits[..1], b"")
            }
            b's' | b'S' if utf16 => {
                let p = args.next() as *const u16;
                put_wide_str(out, p, precision, width, left);
                continue;
            }
            b's' | b'S' => {
                let p = args.next() as *const u8;
                // SAFETY: the caller's NUL-terminated string (or NULL).
                let s: &[u8] = if p.is_null() { b"(null)" } else { unsafe { crate::c_str(p) } };
                (&s[..precision.map_or(s.len(), |n| n.min(s.len()))], b"")
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                let v = f64::from_bits(args.next());
                let (s, negative) = fixed(&mut digits, v, precision.unwrap_or(6));
                (s, if negative { b"-" } else { b"" })
            }
            b'%' => (b"%".as_slice(), b""),
            b'n' => {
                // %n writes memory: not supported (as in the secure CRT).
                args.next();
                crate::missing(b"msvcrt.dll!printf(%n)", 0);
                continue;
            }
            other => {
                crate::missing(b"msvcrt.dll!printf(conversion)", u64::from(other));
                continue;
            }
        };
        let len = prefix.len() + body.len();
        let pad = width.saturating_sub(len);
        let zero_pad =
            zero && !left && precision.is_none() && !matches!(conv, b's' | b'c' | b'S' | b'C');
        if !left && !zero_pad {
            pad_with(out, b' ', pad);
        }
        out.put(prefix);
        if zero_pad {
            pad_with(out, b'0', pad);
        }
        out.put(body);
        if left {
            pad_with(out, b' ', pad);
        }
    }
}

/// A UTF-16 string argument, as UTF-8, padded to `width` characters.
fn put_wide_str(out: &mut Out, p: *const u16, precision: Option<usize>, width: usize, left: bool) {
    let mut chars = 0;
    let limit = precision.unwrap_or(usize::MAX);
    if p.is_null() {
        pad_with(out, b' ', if left { 0 } else { width.saturating_sub(6) });
        out.put(b"(null)");
        pad_with(out, b' ', if left { width.saturating_sub(6) } else { 0 });
        return;
    }
    // SAFETY: the caller's NUL-terminated UTF-16 string.
    let s = unsafe {
        let mut n = 0;
        while *p.add(n) != 0 && n < limit {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    };
    let pad = width.saturating_sub(s.len());
    if !left {
        pad_with(out, b' ', pad);
    }
    for c in char::decode_utf16(s.iter().copied()) {
        let mut b = [0u8; 4];
        out.put(c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut b).as_bytes());
        chars += 1;
    }
    let _ = chars;
    if left {
        pad_with(out, b' ', pad);
    }
}

fn pad_with(out: &mut Out, c: u8, n: usize) {
    for _ in 0..n {
        out.put(&[c]);
    }
}

/// Digits of `v` in `base`, at least `precision` of them.
fn number(
    buf: &mut [u8; 72],
    mut v: u64,
    base: u64,
    upper: bool,
    precision: Option<usize>,
) -> &[u8] {
    let set: &[u8; 16] = if upper { b"0123456789ABCDEF" } else { b"0123456789abcdef" };
    let mut at = buf.len();
    let min = precision.unwrap_or(1).min(64);
    while v > 0 || buf.len() - at < min {
        at -= 1;
        buf[at] = set[(v % base) as usize];
        v /= base;
    }
    &buf[at..]
}

/// `v` in fixed point with `precision` decimals (values up to 2^64; the
/// last digits may be off by rounding).
fn fixed(buf: &mut [u8; 72], v: f64, precision: usize) -> (&[u8], bool) {
    let precision = precision.min(18);
    let negative = v.is_sign_negative();
    let v = if negative { -v } else { v };
    if v.is_nan() || v.is_infinite() {
        let s: &[u8] = if v.is_nan() { b"nan" } else { b"inf" };
        buf[..s.len()].copy_from_slice(s);
        return (&buf[..s.len()], negative && !v.is_nan());
    }
    let mut scale = 1u64;
    for _ in 0..precision {
        scale *= 10;
    }
    let scaled = v * scale as f64 + 0.5;
    let whole = (scaled / scale as f64) as u64;
    let frac = (scaled as u64).wrapping_sub(whole.wrapping_mul(scale));
    let mut tmp = [0u8; 72];
    let int_part = number(&mut tmp, whole, 10, false, None);
    let mut n = int_part.len();
    buf[..n].copy_from_slice(int_part);
    if precision > 0 {
        buf[n] = b'.';
        n += 1;
        let mut tmp2 = [0u8; 72];
        let f = number(&mut tmp2, frac, 10, false, Some(precision));
        buf[n..n + f.len()].copy_from_slice(f);
        n += f.len();
    }
    (&buf[..n], negative)
}

#[unsafe(no_mangle)]
pub extern "C" fn vfprintf(file: *mut u8, fmt: *const u8, args: *const u64) -> i32 {
    let Some(s) = stream(file) else {
        crate::missing(b"msvcrt.dll!vfprintf(file)", 0);
        return -1;
    };
    let mut out = Out { sink: Sink::Handle { handle: s.handle, buf: [0; 256], len: 0 }, total: 0 };
    // SAFETY: the caller's NUL-terminated format.
    format(&mut out, unsafe { crate::c_str(fmt) }, Args(args), false);
    out.flush();
    out.total as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn vprintf(fmt: *const u8, args: *const u64) -> i32 {
    // SAFETY: inside IOB.
    vfprintf(unsafe { __iob_func().add(FILE_SIZE) }, fmt, args)
}

/// vsprintf: no size limit (as documented; the caller's buffer must fit).
#[unsafe(no_mangle)]
pub extern "C" fn vsprintf(buf: *mut u8, fmt: *const u8, args: *const u64) -> i32 {
    let mut out = Out { sink: Sink::Bytes { ptr: buf, cap: usize::MAX }, total: 0 };
    // SAFETY: the caller's NUL-terminated format.
    format(&mut out, unsafe { crate::c_str(fmt) }, Args(args), false);
    out.put(&[0]);
    out.total as i32 - 1
}

/// _vsnprintf(buf, count, fmt, args): at most `count` bytes; the length,
/// or -1 if it did not fit (then without a terminator), as documented.
#[unsafe(no_mangle)]
pub extern "C" fn _vsnprintf(buf: *mut u8, count: usize, fmt: *const u8, args: *const u64) -> i32 {
    let mut out = Out { sink: Sink::Bytes { ptr: buf, cap: count }, total: 0 };
    // SAFETY: the caller's NUL-terminated format.
    format(&mut out, unsafe { crate::c_str(fmt) }, Args(args), false);
    let n = out.total;
    if n < count {
        // SAFETY: room for the terminator.
        unsafe { *buf.add(n) = 0 };
        n as i32
    } else if n == count {
        n as i32
    } else {
        -1
    }
}

/// _vsnwprintf(buf, count, fmt, args): the wide version (counts in
/// characters).
#[unsafe(no_mangle)]
pub extern "C" fn _vsnwprintf(
    buf: *mut u16,
    count: usize,
    fmt: *const u16,
    args: *const u64,
) -> i32 {
    // The format as UTF-8 (formats are ASCII in practice).
    let mut narrow_fmt = [0u8; 1024];
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated wide format.
    unsafe {
        while *fmt.add(n) != 0 && n < narrow_fmt.len() - 1 {
            narrow_fmt[n] = (*fmt.add(n)).min(0x7f) as u8;
            n += 1;
        }
    }
    let mut text = [0u8; 2048];
    let mut out = Out { sink: Sink::Bytes { ptr: text.as_mut_ptr(), cap: text.len() }, total: 0 };
    format(&mut out, &narrow_fmt[..n], Args(args), true);
    let produced = out.total.min(text.len());
    let s = core::str::from_utf8(&text[..produced]).unwrap_or("");
    let mut w = 0;
    for c in s.encode_utf16() {
        if w < count {
            // SAFETY: inside the caller's buffer of `count` characters.
            unsafe { *buf.add(w) = c };
        }
        w += 1;
    }
    if w < count {
        // SAFETY: room for the terminator.
        unsafe { *buf.add(w) = 0 };
        w as i32
    } else if w == count {
        w as i32
    } else {
        -1
    }
}

// fprintf(file, fmt, ...): rcx = file, rdx = fmt, r8/r9 and the stack =
// the variadic arguments. Spill rdx, r8, r9 to the caller's home area, so
// [rsp+24] (r8's slot) starts the va_list, then vfprintf(file, fmt, list).
// sprintf(buf, fmt, ...) is the same shape (vsprintf). printf(fmt, ...):
// the list starts at [rsp+16].
core::arch::global_asm!(
    ".globl fprintf",
    "fprintf:",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea r8, [rsp+24]",
    "sub rsp, 40",
    "call {vfprintf}",
    "add rsp, 40",
    "ret",
    ".globl sprintf",
    "sprintf:",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea r8, [rsp+24]",
    "sub rsp, 40",
    "call {vsprintf}",
    "add rsp, 40",
    "ret",
    ".globl printf",
    "printf:",
    "mov [rsp+8], rcx",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea rdx, [rsp+16]",
    "sub rsp, 40",
    "call {vprintf}",
    "add rsp, 40",
    "ret",
    vfprintf = sym vfprintf,
    vsprintf = sym vsprintf,
    vprintf = sym vprintf,
);
