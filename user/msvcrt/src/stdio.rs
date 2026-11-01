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
    fn GetFileType(h: *mut c_void) -> u32;
    fn SetStdHandle(which: u32, h: *mut c_void) -> i32;
    fn GetHandleInformation(h: *mut c_void, flags: *mut u32) -> i32;
    fn GetStartupInfoA(info: *mut u8);
    fn GetCurrentProcess() -> *mut c_void;
    fn DuplicateHandle(
        source_process: *mut c_void,
        source: *mut c_void,
        target_process: *mut c_void,
        target: *mut *mut c_void,
        access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
    fn GetFileSizeEx(h: *mut c_void, size: *mut i64) -> i32;
    fn CreatePipe(
        read: *mut *mut c_void,
        write: *mut *mut c_void,
        attributes: *mut c_void,
        size: u32,
    ) -> i32;
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

const ERROR_BROKEN_PIPE: u32 = 109;
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

/// The state of a stream. Files from fopen are this struct itself. It
/// starts with msvcrt's 48-byte FILE (`_ptr`, `_cnt`, `_base`, `_flag`,
/// `_file`, `_charbuf`, `_bufsiz`, `_tmpfname`) because mingw's inline
/// `getc` reads `_cnt` and `_ptr`: `_cnt` stays 0, so every inline getc
/// calls `_filbuf`, which reads through the stream.
///
/// After the FILE comes a CRITICAL_SECTION, as in msvcrt.dll's `_FILEX`
/// for streams that fopen allocates: mingw's `_lock_file` enters it
/// directly (for the console slots it calls `_lock` instead).
#[repr(C)]
struct Stream {
    iob: [u8; FILE_SIZE],
    lock: CriticalSection,
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
        let mut iob = [0u8; FILE_SIZE];
        let f = fd.to_le_bytes();
        iob[24] = f[0];
        iob[25] = f[1];
        iob[26] = f[2];
        iob[27] = f[3];
        Self {
            iob,
            lock: CriticalSection::FREE,
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

/// CRITICAL_SECTION (40 bytes): DebugInfo, LockCount, RecursionCount,
/// OwningThread, LockSemaphore, SpinCount. FREE is what
/// InitializeCriticalSection makes of it: unowned, LockCount -1.
#[repr(C)]
struct CriticalSection {
    debug_info: usize,
    lock_count: i32,
    recursion: i32,
    owner: usize,
    semaphore: usize,
    spin_count: usize,
}

impl CriticalSection {
    const FREE: Self =
        Self { debug_info: 0, lock_count: -1, recursion: 0, owner: 0, semaphore: 0, spin_count: 0 };
}

static mut STD: [Stream; 3] =
    [Stream::new(null_mut(), 0), Stream::new(null_mut(), 1), Stream::new(null_mut(), 2)];

/// __iob_func(): the stdin, stdout and stderr FILE objects.
#[unsafe(no_mangle)]
pub extern "C" fn __iob_func() -> *mut u8 {
    // SAFETY: the console slots; `_file` (offset 24) is their descriptor.
    unsafe {
        for (i, slot) in (*(&raw mut IOB)).iter_mut().enumerate() {
            slot[24..28].copy_from_slice(&(i as i32).to_le_bytes());
        }
    }
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
    // 'D': a temporary file, deleted when it is closed (as msvcrt documents).
    let (share, flags) = if mode.contains(&b'D') { (7, 0x80 | 0x0400_0000) } else { (3, 0x80) };
    // SAFETY: a NUL-terminated path.
    let h = unsafe {
        CreateFileW(path.as_ptr(), access, share, null_mut(), disposition, flags, null_mut())
    };
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

/// freopen(path, mode, file): `file` now reads or writes the file at
/// `path` on the same descriptor (so freopen of stdout redirects fd 1 and
/// the standard handle); its old file is closed. `file`, or NULL.
#[unsafe(no_mangle)]
pub extern "C" fn freopen(path: *const u8, mode: *const u8, file: *mut u8) -> *mut u8 {
    let Some(s) = stream(file) else { return null_mut() };
    fflush(file);
    let fresh = fopen(path, mode);
    let Some(new) = stream(fresh) else { return null_mut() };
    let (fd, new_fd) = (s.fd, new.fd);
    if _dup2(new_fd, fd) != 0 {
        fclose(fresh);
        return null_mut();
    }
    fclose(fresh);
    let Some(h) = handle_of_fd(fd) else { return null_mut() };
    // SAFETY: the caller's stream, reset to the new file.
    unsafe { file.cast::<Stream>().write(Stream::new(h, fd)) };
    file
}

/// tmpnam(buf): a name for a temporary file in the root directory of the
/// current drive (`\s<n>.`, as msvcrt names them) that does not exist
/// yet; in `buf` (L_tmpnam = 14 bytes) or a static buffer.
#[unsafe(no_mangle)]
pub extern "C" fn tmpnam(buf: *mut u8) -> *mut u8 {
    static mut NAME: [u8; 14] = [0; 14];
    static NEXT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);
    let out = if buf.is_null() { (&raw mut NAME).cast::<u8>() } else { buf };
    for _ in 0..1000 {
        let mut n = NEXT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        let mut digits = [0u8; 8];
        let mut len = 0;
        loop {
            digits[len] = b"0123456789abcdefghijklmnopqrstuv"[(n % 32) as usize];
            len += 1;
            n /= 32;
            if n == 0 {
                break;
            }
        }
        // SAFETY: at most 2 + 7 + 2 bytes of the 14 (L_tmpnam).
        unsafe {
            *out = b'\\';
            *out.add(1) = b's';
            for i in 0..len.min(7) {
                *out.add(2 + i) = digits[len - 1 - i];
            }
            *out.add(2 + len.min(7)) = b'.';
            *out.add(3 + len.min(7)) = 0;
        }
        if crate::posix::_access(out, 0) != 0 {
            return out;
        }
    }
    null_mut()
}

/// tmpfile(): a new temporary file ("w+b"), deleted when closed.
#[unsafe(no_mangle)]
pub extern "C" fn tmpfile() -> *mut u8 {
    let mut name = [0u8; 14];
    if tmpnam(name.as_mut_ptr()).is_null() {
        set_errno(EEXIST_);
        return null_mut();
    }
    fopen(name.as_ptr(), c"w+bD".as_ptr().cast())
}

const EEXIST_: i32 = 17;

/// _popen(command, mode): needs a command interpreter connected through a
/// pipe; not yet (recorded). Fails with ENOSYS.
#[unsafe(no_mangle)]
pub extern "C" fn _popen(_command: *const u8, _mode: *const u8) -> *mut u8 {
    crate::missing(b"msvcrt.dll!_popen", 0);
    set_errno(40); // ENOSYS
    null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn _pclose(_file: *mut u8) -> i32 {
    set_errno(22); // EINVAL
    -1
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
            // The end of a pipe (ERROR_BROKEN_PIPE) is end of file.
            // SAFETY: plain call.
            if ok == 0 && unsafe { GetLastError() } != ERROR_BROKEN_PIPE {
                s.error = true;
                break;
            }
            if ok == 0 || got == 0 {
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

pub const MAX_FDS: usize = 64;
/// Handles of descriptors 3 and up (0-2 are the console's).
static mut FDS: [usize; MAX_FDS] = [0; MAX_FDS];

/// Which of descriptors 0-2 were closed (_close): free again for the
/// next descriptor, as POSIX and msvcrt hand out the lowest free one.
static STD_CLOSED: [core::sync::atomic::AtomicBool; 3] = [
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
    core::sync::atomic::AtomicBool::new(false),
];

fn set_std_closed(fd: i32, closed: bool) {
    STD_CLOSED[fd as usize].store(closed, core::sync::atomic::Ordering::Relaxed);
}

fn std_closed(fd: i32) -> bool {
    STD_CLOSED[fd as usize].load(core::sync::atomic::Ordering::Relaxed)
}

/// A descriptor for handle `h`: the lowest free one (a closed 0-2 becomes
/// the process's standard handle again).
fn new_fd(h: *mut c_void) -> i32 {
    if let Some(fd) = (0..3).find(|&fd| std_closed(fd)) {
        set_std_closed(fd, false);
        // SAFETY: the process's standard handle.
        unsafe { SetStdHandle((-10 - fd) as u32, h) };
        return fd;
    }
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

/// The C runtime's descriptor inheritance (STARTUPINFO cbReserved2 /
/// lpReserved2, the layout Wine's msvcrt documents): a u32 count, a flags
/// byte per descriptor (FOPEN 0x01), then a HANDLE per descriptor, packed.
const FOPEN: u8 = 0x01;

/// The block for a child: our open descriptors whose handles it inherits
/// (descriptors 0-2 are its standard handles anyway). Written to `out`;
/// its length, or 0 if there is nothing to pass.
pub fn inheritance_block(out: &mut [u8; 4 + 9 * MAX_FDS]) -> usize {
    let mut count = 0;
    for fd in 3..MAX_FDS {
        if handle_of_fd(fd as i32).is_some() {
            count = fd + 1;
        }
    }
    if count == 0 {
        return 0;
    }
    out[..4].copy_from_slice(&(count as u32).to_le_bytes());
    for fd in 0..count {
        let h = if fd >= 3 { handle_of_fd(fd as i32) } else { None };
        let mut flags = 0u32;
        // SAFETY: plain call on our handle.
        let inheritable = h
            .is_some_and(|h| unsafe { GetHandleInformation(h, &mut flags) } != 0 && flags & 1 != 0);
        let (flag, value) = match h {
            Some(h) if inheritable => (FOPEN, h as u64),
            _ => (0, u64::MAX),
        };
        out[4 + fd] = flag;
        let at = 4 + count + 8 * fd;
        out[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }
    4 + 9 * count
}

/// At startup: takes the descriptors (3 and up) a parent's CRT passed.
pub fn inherit_descriptors() {
    let mut info = [0u8; 104];
    // SAFETY: a STARTUPINFOA-sized buffer; the block it names stays in our
    // process parameters.
    unsafe {
        GetStartupInfoA(info.as_mut_ptr());
        let len = usize::from(u16::from_le_bytes([info[66], info[67]]));
        let block = u64::from_le_bytes(info[72..80].try_into().unwrap_or([0; 8])) as *const u8;
        if block.is_null() || len < 4 {
            return;
        }
        let count = (block.cast::<u32>().read_unaligned() as usize).min(MAX_FDS);
        if 4 + 9 * count > len {
            return;
        }
        let fds = &mut *(&raw mut FDS);
        for fd in 3..count {
            let h = block.add(4 + count + 8 * fd).cast::<u64>().read_unaligned();
            if *block.add(4 + fd) & FOPEN != 0 && h != 0 && h != u64::MAX {
                fds[fd] = h as usize;
            }
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
        0..=2 if std_closed(fd) => None,
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
        // The end of a pipe is end of file (as msvcrt's _read).
        // SAFETY: plain call.
        if unsafe { GetLastError() } == ERROR_BROKEN_PIPE {
            return 0;
        }
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
    let Some(h) = handle_of_fd(fd) else {
        set_errno(EBADF);
        return -1;
    };
    if fd <= 2 {
        // A standard descriptor: its handle goes, and so does the process's
        // standard handle (as msvcrt does).
        // SAFETY: the standard handle we own.
        unsafe {
            CloseHandle(h);
            SetStdHandle((-10 - fd) as u32, INVALID_HANDLE);
        }
        set_std_closed(fd, true);
        return 0;
    }
    close_fd(fd);
    0
}

/// A duplicate of `h` (inheritable, like msvcrt's descriptors).
fn duplicate(h: *mut c_void) -> Option<*mut c_void> {
    let mut new = null_mut();
    // SAFETY: plain call on our handles.
    let ok = unsafe {
        let me = GetCurrentProcess();
        DuplicateHandle(me, h, me, &mut new, 0, 1, 2) // DUPLICATE_SAME_ACCESS
    };
    (ok != 0).then_some(new)
}

/// _dup(fd): a new descriptor for the same file.
#[unsafe(no_mangle)]
pub extern "C" fn _dup(fd: i32) -> i32 {
    let Some(new) = handle_of_fd(fd).and_then(duplicate) else {
        set_errno(EBADF);
        return -1;
    };
    let n = new_fd(new);
    if n < 0 {
        // SAFETY: our duplicate.
        unsafe { CloseHandle(new) };
        set_errno(24); // EMFILE
    }
    n
}

/// _dup2(fd, target): `target` (closed first if open) becomes a duplicate
/// of `fd`. For 0-2 the process's standard handle changes too (msvcrt
/// does this), so children started later get the redirection.
#[unsafe(no_mangle)]
pub extern "C" fn _dup2(fd: i32, target: i32) -> i32 {
    if target < 0 || target as usize >= MAX_FDS {
        set_errno(EBADF);
        return -1;
    }
    let Some(new) = handle_of_fd(fd).and_then(duplicate) else {
        set_errno(EBADF);
        return -1;
    };
    if let Some(old) = handle_of_fd(target) {
        // SAFETY: the descriptor's old handle, replaced below.
        unsafe { CloseHandle(old) };
    }
    // SAFETY: the table / the process's standard handle.
    unsafe {
        if target <= 2 {
            SetStdHandle((-10 - target) as u32, new);
            set_std_closed(target, false);
        } else {
            (*(&raw mut FDS))[target as usize] = new as usize;
        }
    }
    0
}

/// _creat(path, mode): _open for writing, created or truncated.
#[unsafe(no_mangle)]
pub extern "C" fn _creat(path: *const u8, mode: i32) -> i32 {
    _open(path, O_WRONLY | O_CREAT | O_TRUNC, mode)
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

/// _filbuf: the refill behind mingw's inline getc (see Stream): the next
/// byte, or -1 (EOF); `_cnt` is set back to 0.
#[unsafe(no_mangle)]
pub extern "C" fn _filbuf(file: *mut u8) -> i32 {
    let c = fgetc(file);
    if !file.is_null() {
        // SAFETY: a FILE (console slot or ours): `_cnt` at offset 8.
        unsafe { file.add(8).cast::<i32>().write_unaligned(0) };
    }
    c
}

#[unsafe(no_mangle)]
pub extern "C" fn putc(c: i32, file: *mut u8) -> i32 {
    fputc(c, file)
}

/// puts(s): `s` and a newline to standard output.
#[unsafe(no_mangle)]
pub extern "C" fn puts(s: *const u8) -> i32 {
    // SAFETY: stdout's slot; the caller's NUL-terminated string.
    unsafe {
        let out = __iob_func().add(FILE_SIZE);
        if fputs(s, out) < 0 || fputc(i32::from(b'\n'), out) < 0 {
            return -1;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn putchar(c: i32) -> i32 {
    // SAFETY: stdout's slot.
    fputc(c, unsafe { __iob_func().add(FILE_SIZE) })
}

/// setvbuf/setbuf: writes are unbuffered here and reads use the stream's
/// own buffer, so the request changes nothing (and succeeds).
#[unsafe(no_mangle)]
pub extern "C" fn setvbuf(_file: *mut u8, _buf: *mut u8, _mode: i32, _size: usize) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn setbuf(_file: *mut u8, _buf: *mut u8) {}

#[unsafe(no_mangle)]
pub extern "C" fn clearerr(file: *mut u8) {
    if let Some(s) = stream(file) {
        s.eof = false;
        s.error = false;
    }
}

/// _vscprintf(fmt, args): the length the formatted text would have.
#[unsafe(no_mangle)]
pub extern "C" fn _vscprintf(fmt: *const u8, args: *const u64) -> i32 {
    let mut out = Out { sink: Sink::Bytes { ptr: null_mut(), cap: 0 }, total: 0 };
    // SAFETY: the caller's NUL-terminated format.
    format(&mut out, unsafe { crate::c_str(fmt) }, Args(args), false);
    out.total as i32
}

/// A new stream on descriptor `fd` (its handle), or null.
fn stream_on(fd: i32) -> *mut u8 {
    let Some(h) = handle_of_fd(fd) else {
        set_errno(EBADF);
        return null_mut();
    };
    let file = malloc(size_of::<Stream>()).cast::<Stream>();
    if !file.is_null() {
        // SAFETY: a fresh allocation of the right size.
        unsafe { file.write(Stream::new(h, fd)) };
    }
    file.cast()
}

/// _fdopen(fd, mode): a stream on an open descriptor.
#[unsafe(no_mangle)]
pub extern "C" fn _fdopen(fd: i32, _mode: *const u8) -> *mut u8 {
    stream_on(fd)
}

/// _open_osfhandle(handle, flags): a descriptor for a Win32 handle.
#[unsafe(no_mangle)]
pub extern "C" fn _open_osfhandle(handle: isize, _flags: i32) -> i32 {
    if handle == -1 || handle == 0 {
        set_errno(EBADF);
        return -1;
    }
    new_fd(handle as *mut c_void)
}

const O_WRONLY: i32 = 0x1;
const O_RDWR: i32 = 0x2;
const O_APPEND: i32 = 0x8;
const O_NOINHERIT: i32 = 0x80;
const O_CREAT: i32 = 0x100;
const O_TRUNC: i32 = 0x200;
const O_EXCL: i32 = 0x400;

/// _open(path, flags, mode): CreateFile with the flags' access and
/// disposition (the permission `mode` only matters for read-only files).
#[unsafe(no_mangle)]
pub extern "C" fn _open(path: *const u8, flags: i32, _mode: i32) -> i32 {
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    let mut wide = [0u16; 1024];
    // SAFETY: the caller's NUL-terminated path.
    if wide_path(unsafe { crate::c_str(path) }, &mut wide).is_none() {
        set_errno(ENOENT);
        return -1;
    }
    let access = match flags & 3 {
        O_WRONLY => GENERIC_WRITE,
        O_RDWR => GENERIC_READ | GENERIC_WRITE,
        _ => GENERIC_READ,
    };
    // CREATE_NEW 1, CREATE_ALWAYS 2, OPEN_EXISTING 3, OPEN_ALWAYS 4,
    // TRUNCATE_EXISTING 5.
    let disposition = match (flags & O_CREAT != 0, flags & O_EXCL != 0, flags & O_TRUNC != 0) {
        (true, true, _) => 1,
        (true, false, true) => 2,
        (true, false, false) => 4,
        (false, _, true) => 5,
        (false, _, false) => 3,
    };
    let inherit = flags & O_NOINHERIT == 0;
    // SECURITY_ATTRIBUTES { nLength, lpSecurityDescriptor, bInheritHandle }.
    let mut sa = [24u64, 0, u64::from(inherit)];
    // SAFETY: a NUL-terminated path; a valid SECURITY_ATTRIBUTES.
    let h = unsafe {
        CreateFileW(wide.as_ptr(), access, 3, sa.as_mut_ptr().cast(), disposition, 0x80, null_mut())
    };
    if h == INVALID_HANDLE {
        set_errno(errno_of_last_error());
        return -1;
    }
    if flags & O_APPEND != 0 {
        // SAFETY: our handle.
        unsafe { SetFilePointerEx(h, 0, null_mut(), 2) };
    }
    let fd = new_fd(h);
    if fd < 0 {
        // SAFETY: our handle.
        unsafe { CloseHandle(h) };
        set_errno(24); // EMFILE
    }
    fd
}

/// _pipe(fds[2], size, mode): an anonymous pipe as two descriptors
/// (inheritable unless _O_NOINHERIT).
#[unsafe(no_mangle)]
pub extern "C" fn _pipe(fds: *mut i32, size: u32, mode: i32) -> i32 {
    let (mut r, mut w) = (null_mut(), null_mut());
    let mut sa = [24u64, 0, u64::from(mode & O_NOINHERIT == 0)];
    // SAFETY: valid output pointers and SECURITY_ATTRIBUTES.
    if unsafe { CreatePipe(&mut r, &mut w, sa.as_mut_ptr().cast(), size) } == 0 {
        set_errno(errno_of_last_error());
        return -1;
    }
    let (a, b) = (new_fd(r), new_fd(w));
    if a < 0 || b < 0 {
        set_errno(24); // EMFILE
        return -1;
    }
    // SAFETY: the caller's int[2].
    unsafe {
        *fds = a;
        *fds.add(1) = b;
    }
    0
}

/// _isatty: a character device (the console, NUL), as msvcrt decides.
#[unsafe(no_mangle)]
pub extern "C" fn _isatty(fd: i32) -> i32 {
    // SAFETY: plain call.
    handle_of_fd(fd).map_or(0, |h| i32::from(unsafe { GetFileType(h) } == 2))
}

/// _lseeki64(fd, offset, origin).
#[unsafe(no_mangle)]
pub extern "C" fn _lseeki64(fd: i32, offset: i64, origin: i32) -> i64 {
    let Some(h) = handle_of_fd(fd) else {
        set_errno(EBADF);
        return -1;
    };
    let mut new = 0i64;
    // SAFETY: our descriptor's handle.
    if unsafe { SetFilePointerEx(h, offset, &mut new, origin as u32) } == 0 {
        set_errno(EINVAL);
        return -1;
    }
    new
}

#[unsafe(no_mangle)]
pub extern "C" fn _telli64(fd: i32) -> i64 {
    _lseeki64(fd, 0, 1)
}

/// _fstat64(fd, buf): pipes are FIFOs and character devices (the console,
/// NUL) are character devices, with no times; files report their size.
#[unsafe(no_mangle)]
pub extern "C" fn _fstat64(fd: i32, buf: *mut u8) -> i32 {
    let Some(h) = handle_of_fd(fd) else {
        set_errno(EBADF);
        return -1;
    };
    // SAFETY: plain calls on our descriptor's handle.
    let (kind, size) = unsafe {
        let mut size = 0i64;
        let kind = GetFileType(h);
        if kind == 1 {
            GetFileSizeEx(h, &mut size);
        }
        (kind, size)
    };
    let mode: u16 = match kind {
        3 => 0x1000 | 0o666, // _S_IFIFO
        2 => 0x2000 | 0o666, // _S_IFCHR
        _ => 0x8000 | 0o666, // _S_IFREG
    };
    // SAFETY: the caller's struct _stat64 (see _stat64).
    unsafe {
        core::ptr::write_bytes(buf, 0, 56);
        core::ptr::write_unaligned(buf.add(6).cast::<u16>(), mode);
        core::ptr::write_unaligned(buf.add(8).cast::<i16>(), 1);
        core::ptr::write_unaligned(buf.add(24).cast::<i64>(), size);
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
/// d i u o x X c s p %, and f e g (float.rs)). With `wide`, `%s` and
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
        let mut float_buf = [0u8; crate::float::MAX_OUT];
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
                let (n, negative) = crate::float::format(v, conv, precision, alt, &mut float_buf);
                let sign: &[u8] = if negative {
                    b"-"
                } else if plus {
                    b"+"
                } else if space {
                    b" "
                } else {
                    b""
                };
                (&float_buf[..n], sign)
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
        // 0 pads numbers; an integer precision turns it off (not a float's).
        let float = matches!(conv, b'f' | b'F' | b'e' | b'E' | b'g' | b'G');
        let zero_pad = zero
            && !left
            && (precision.is_none() || float)
            && !matches!(conv, b's' | b'c' | b'S' | b'C');
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
    if buf.is_null() && count == 0 {
        // The documented way to measure: the length the output needs.
        n as i32
    } else if n < count {
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

/// vfwprintf(file, fmt, args): the wide format's output written to a
/// narrow stream in the ANSI code page (UTF-8 on gala), as msvcrt does
/// for a stream not in a Unicode mode; the count of wide characters.
#[unsafe(no_mangle)]
pub extern "C" fn vfwprintf(file: *mut u8, fmt: *const u16, args: *const u64) -> i32 {
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
    if fwrite(text.as_ptr(), 1, produced, file) != produced {
        return -1;
    }
    core::str::from_utf8(&text[..produced]).map_or(produced, |s| s.encode_utf16().count()) as i32
}

/// fputwc(c, file): the character in the ANSI code page (UTF-8); `c`, or
/// WEOF on failure.
#[unsafe(no_mangle)]
pub extern "C" fn fputwc(c: u16, file: *mut u8) -> u16 {
    let ch = char::from_u32(u32::from(c)).unwrap_or(char::REPLACEMENT_CHARACTER);
    let mut b = [0u8; 4];
    let bytes = ch.encode_utf8(&mut b).as_bytes();
    if fwrite(bytes.as_ptr(), 1, bytes.len(), file) == bytes.len() { c } else { 0xffff }
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
    // _snprintf(buf, count, fmt, ...): the list starts at r9's slot.
    ".globl _snprintf",
    "_snprintf:",
    "mov [rsp+32], r9",
    "lea r9, [rsp+32]",
    "sub rsp, 40",
    "call {_vsnprintf}",
    "add rsp, 40",
    "ret",
    // fwprintf(file, fmt, ...): fprintf's shape.
    ".globl fwprintf",
    "fwprintf:",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea r8, [rsp+24]",
    "sub rsp, 40",
    "call {vfwprintf}",
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
    vfwprintf = sym vfwprintf,
    vsprintf = sym vsprintf,
    vprintf = sym vprintf,
    _vsnprintf = sym _vsnprintf,
);
