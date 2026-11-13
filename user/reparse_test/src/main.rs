//! NT symbolic links and reparse points on files, with the results real
//! Windows gives (CreateSymbolicLinkW's documentation, Wine's ntdll
//! test_reparse_points as run on Windows, NTFS's FSCTL documentation):
//!
//! - Without SeCreateSymbolicLinkPrivilege (a standard user):
//!   CreateSymbolicLinkW fails with ERROR_PRIVILEGE_NOT_HELD and leaves no
//!   file; the test ends there (`no symbolic link privilege`). With
//!   SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE a standard user's
//!   Windows made the link (developer mode's state on that machine
//!   unknown): the result and the developer-mode switch are `info` lines
//!   until a run settles what decides it.
//! - With it (an administrator, SYSTEM; CreateSymbolicLinkW enables it):
//!   an absolute link (its reparse data: IO_REPARSE_TAG_SYMLINK, the
//!   substitute name `\??\` and the full path, the print name as given,
//!   flags 0) and a relative one (SYMLINK_FLAG_RELATIVE, both names as
//!   given) read the target's contents; writing through one writes the
//!   target. The link itself (FILE_FLAG_OPEN_REPARSE_POINT) is empty, has
//!   FILE_ATTRIBUTE_REPARSE_POINT and the tag (FileAttributeTagInfo,
//!   FindFirstFileW's dwReserved0, GetFileAttributesW and
//!   GetFileAttributesExW, which do not follow it); the target has tag 0.
//!   A directory link (SYMBOLIC_LINK_FLAG_DIRECTORY) is followed in the
//!   middle of a path. A second link of the same name is
//!   ERROR_ALREADY_EXISTS; a dangling link's open ERROR_FILE_NOT_FOUND; a
//!   loop of links ERROR_CANT_RESOLVE_FILENAME. FSCTL_GET_REPARSE_POINT on
//!   a plain file is ERROR_NOT_A_REPARSE_POINT; FSCTL_DELETE_REPARSE_POINT
//!   makes the link a plain empty file. DeleteFileW and RemoveDirectoryW
//!   remove a link, not its target.
//!
//! Prints `reparse_test: links made` (with the privilege), then
//! `reparse_test: ok`, or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = u32;
type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetTempPathW(size: u32, buf: *mut u16) -> u32;
    fn GetFullPathNameW(path: *const u16, size: u32, buf: *mut u16, part: *mut *mut u16) -> u32;
    fn CreateDirectoryW(path: *const u16, sa: *mut c_void) -> Bool;
    fn RemoveDirectoryW(path: *const u16) -> Bool;
    fn DeleteFileW(path: *const u16) -> Bool;
    fn CreateSymbolicLinkW(link: *const u16, target: *const u16, flags: u32) -> u8;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        sa: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn GetFileSize(h: Handle, high: *mut u32) -> u32;
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn GetFileAttributesExW(path: *const u16, level: u32, info: *mut u32) -> Bool;
    fn GetFileInformationByHandleEx(h: Handle, class: u32, buf: *mut c_void, size: u32) -> Bool;
    fn DeviceIoControl(
        h: Handle,
        code: u32,
        input: *const c_void,
        input_size: u32,
        output: *mut c_void,
        output_size: u32,
        returned: *mut u32,
        ov: *mut c_void,
    ) -> Bool;
    fn FindFirstFileW(path: *const u16, data: *mut u16) -> Handle;
    fn FindClose(h: Handle) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
    fn GetStdHandle(which: u32) -> Handle;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegGetValueW(
        key: isize,
        subkey: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> u32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn __chkstk();
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const DELETE: u32 = 0x1_0000;
const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
const SHARE_ALL: u32 = 7;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x20;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const INVALID_FILE_ATTRIBUTES: u32 = 0xffff_ffff;
const SYMBOLIC_LINK_FLAG_DIRECTORY: u32 = 1;
const SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE: u32 = 2;
const IO_REPARSE_TAG_SYMLINK: u32 = 0xa000_000c;
const FSCTL_GET_REPARSE_POINT: u32 = 0x0009_00a8;
const FSCTL_DELETE_REPARSE_POINT: u32 = 0x0009_00ac;
const FILE_ATTRIBUTE_TAG_INFO: u32 = 9;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_PRIVILEGE_NOT_HELD: u32 = 1314;
const ERROR_CANT_RESOLVE_FILENAME: u32 = 1921;
const ERROR_NOT_A_REPARSE_POINT: u32 = 4390;

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

/// An `info` line: what Windows answers, for the record (not checked).
fn info(what: &[u8], v: u32) {
    out(b"reparse_test: info ");
    out(what);
    out(b" ");
    out(&hex(v));
    out(b"\r\n");
}

/// Developer mode's switch: AllowDevelopmentWithoutDevLicense (a DWORD)
/// of `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock`,
/// or 0xffff0000 and RegGetValueW's error.
fn developer_mode() -> u32 {
    const HKEY_LOCAL_MACHINE: isize = 0x8000_0002_u32 as i32 as isize;
    const RRF_RT_REG_DWORD: u32 = 0x10;
    let key = Path::new(&[], b"SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\AppModelUnlock");
    let value = Path::new(&[], b"AllowDevelopmentWithoutDevLicense");
    let (mut v, mut size) = (0u32, 4u32);
    // SAFETY: NUL-terminated names; a DWORD and its size.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.ptr(),
            value.ptr(),
            RRF_RT_REG_DWORD,
            core::ptr::null_mut(),
            (&raw mut v).cast(),
            &mut size,
        )
    };
    if r == 0 { v } else { 0xffff_0000 | r }
}

fn hex(v: u32) -> [u8; 8] {
    let mut s = [0u8; 8];
    for (i, c) in s.iter_mut().enumerate() {
        let d = (v >> (28 - 4 * i)) & 0xf;
        *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
    }
    s
}

/// ` (line L)`: the source line of the failing check.
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

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"reparse_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        check_line(core::panic::Location::caller().line());
        out(b"\r\n");
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

/// A NUL-terminated UTF-16 path: `base` + `rest` (ASCII).
struct Path {
    buf: [u16; 300],
    len: usize,
}

impl Path {
    fn new(base: &[u16], rest: &[u8]) -> Path {
        let mut buf = [0u16; 300];
        buf[..base.len()].copy_from_slice(base);
        for (i, &b) in rest.iter().enumerate() {
            buf[base.len() + i] = u16::from(b);
        }
        Path { buf, len: base.len() + rest.len() }
    }

    fn ptr(&self) -> *const u16 {
        self.buf.as_ptr()
    }

    fn units(&self) -> &[u16] {
        &self.buf[..self.len]
    }
}

/// The directory's files (best effort, links first: a link's removal
/// leaves its target).
///
/// # Safety
/// `dir` is a path ending with a backslash.
unsafe fn clean(dir: &[u16]) {
    let names: [&[u8]; 12] = [
        b"abs.lnk",
        b"rel.lnk",
        b"dang.lnk",
        b"a.lnk",
        b"b.lnk",
        b"cleared.lnk",
        b"nopriv.lnk",
        b"t.txt",
        b"d\\in.txt",
        b"made.txt",
        b"x",
        b"y",
    ];
    // SAFETY: documented calls with NUL-terminated paths.
    unsafe {
        RemoveDirectoryW(Path::new(dir, b"dl").ptr());
        for n in names {
            DeleteFileW(Path::new(dir, n).ptr());
        }
        RemoveDirectoryW(Path::new(dir, b"d").ptr());
    }
}

/// Opens `path` (Win32 flags `flags`).
fn open(path: &Path, access: u32, disposition: u32, flags: u32) -> Handle {
    // SAFETY: a NUL-terminated path.
    unsafe {
        CreateFileW(
            path.ptr(),
            access,
            SHARE_ALL,
            core::ptr::null_mut(),
            disposition,
            flags,
            core::ptr::null_mut(),
        )
    }
}

/// The contents of the file `path` names (following links), at most 32
/// bytes, or the error.
fn contents(path: &Path) -> Result<([u8; 32], usize), u32> {
    let h = open(path, GENERIC_READ, OPEN_EXISTING, 0);
    if h == INVALID_HANDLE_VALUE {
        // SAFETY: plain call.
        return Err(unsafe { GetLastError() });
    }
    let (mut buf, mut n) = ([0u8; 32], 0);
    // SAFETY: our handle and buffer.
    unsafe {
        ReadFile(h, buf.as_mut_ptr(), 32, &mut n, core::ptr::null_mut());
        CloseHandle(h);
    }
    Ok((buf, n as usize))
}

fn write_file(path: &Path, data: &[u8]) -> bool {
    let h = open(path, GENERIC_WRITE, CREATE_ALWAYS, 0);
    if h == INVALID_HANDLE_VALUE {
        return false;
    }
    let mut n = 0;
    // SAFETY: our handle and buffer.
    unsafe {
        WriteFile(h, data.as_ptr(), data.len() as u32, &mut n, core::ptr::null_mut());
        CloseHandle(h);
    }
    n as usize == data.len()
}

/// UTF-16 units of the name at (`offset`, `length`) in a symbolic link's
/// PathBuffer (REPARSE_DATA_BUFFER at `b`).
fn link_name(b: &[u8], offset_field: usize) -> [u16; 300] {
    let off = usize::from(u16::from_le_bytes([b[offset_field], b[offset_field + 1]]));
    let len = usize::from(u16::from_le_bytes([b[offset_field + 2], b[offset_field + 3]]));
    let mut name = [0u16; 300];
    for (i, unit) in name.iter_mut().take((len / 2).min(299)).enumerate() {
        let at = 20 + off + 2 * i;
        *unit = u16::from_le_bytes([b[at], b[at + 1]]);
    }
    name
}

fn units_of(name: &[u16; 300]) -> &[u16] {
    &name[..name.iter().position(|&c| c == 0).unwrap_or(300)]
}

fn ascii_eq(units: &[u16], text: &[u8]) -> bool {
    units.len() == text.len() && units.iter().zip(text).all(|(&u, &t)| u == u16::from(t))
}

/// FSCTL_GET_REPARSE_POINT of the link `path` (opened itself).
fn reparse_data(path: &Path) -> Result<([u8; 1024], u32), u32> {
    let h = open(
        path,
        GENERIC_READ,
        OPEN_EXISTING,
        FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
    );
    if h == INVALID_HANDLE_VALUE {
        // SAFETY: plain call.
        return Err(unsafe { GetLastError() });
    }
    let (mut b, mut n) = ([0u8; 1024], 0);
    // SAFETY: our handle and buffer.
    let ok = unsafe {
        DeviceIoControl(
            h,
            FSCTL_GET_REPARSE_POINT,
            core::ptr::null(),
            0,
            b.as_mut_ptr().cast(),
            1024,
            &mut n,
            core::ptr::null_mut(),
        )
    };
    // SAFETY: plain calls.
    let e = unsafe { GetLastError() };
    // SAFETY: our handle.
    unsafe { CloseHandle(h) };
    if ok == 0 { Err(e) } else { Ok((b, n)) }
}

/// FileAttributeTagInfo (attributes, tag) of `path`, opened itself if
/// `link`.
fn tag_info(path: &Path, link: bool) -> (u32, u32) {
    let flags = FILE_FLAG_BACKUP_SEMANTICS | if link { FILE_FLAG_OPEN_REPARSE_POINT } else { 0 };
    let h = open(path, GENERIC_READ, OPEN_EXISTING, flags);
    if h == INVALID_HANDLE_VALUE {
        return (u32::MAX, u32::MAX);
    }
    let mut info = [0u32; 2];
    // SAFETY: our handle and an 8-byte FILE_ATTRIBUTE_TAG_INFO.
    unsafe {
        GetFileInformationByHandleEx(h, FILE_ATTRIBUTE_TAG_INFO, info.as_mut_ptr().cast(), 8);
        CloseHandle(h);
    }
    (info[0], info[1])
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let mut temp = [0u16; 260];
    // SAFETY: documented calls with valid buffers.
    let n = unsafe { GetTempPathW(260, temp.as_mut_ptr()) } as usize;
    c.ok(n > 0 && n < 200, n as u64)?;
    let dir = Path::new(&temp[..n], b"gala_rp\\");
    let d = dir.units();
    // SAFETY: documented calls with NUL-terminated paths.
    unsafe {
        clean(d);
        CreateDirectoryW(dir.ptr(), core::ptr::null_mut());
    }
    c.ok(write_file(&Path::new(d, b"t.txt"), b"hello"), 0)?;

    // Without the privilege: refused, nothing left behind.
    let nopriv = Path::new(d, b"nopriv.lnk");
    // SAFETY: NUL-terminated paths.
    let made = unsafe { CreateSymbolicLinkW(nopriv.ptr(), Path::new(&[], b"t.txt").ptr(), 0) };
    if made == 0 {
        // SAFETY: plain calls.
        unsafe {
            c.eq(GetLastError().into(), ERROR_PRIVILEGE_NOT_HELD.into())?;
            c.eq(GetFileAttributesW(nopriv.ptr()).into(), INVALID_FILE_ATTRIBUTES.into())?;
            // ALLOW_UNPRIVILEGED_CREATE: a standard user's Windows made the
            // link, on a machine whose developer mode is not known; both
            // are printed until a run says what decides it.
            SetLastError(0);
            let target = Path::new(&[], b"t.txt");
            let flags = SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE;
            let made = CreateSymbolicLinkW(nopriv.ptr(), target.ptr(), flags);
            let error = if made == 0 { GetLastError() } else { 0 };
            info(
                b"ALLOW_UNPRIVILEGED_CREATE without the privilege (0: made, else the error)",
                error,
            );
            info(b"developer mode (AllowDevelopmentWithoutDevLicense)", developer_mode());
            if made != 0 {
                DeleteFileW(nopriv.ptr());
            } else {
                c.eq(GetFileAttributesW(nopriv.ptr()).into(), INVALID_FILE_ATTRIBUTES.into())?;
            }
            clean(d);
            RemoveDirectoryW(dir.ptr());
        }
        out(b"reparse_test: no symbolic link privilege\r\n");
        return Ok(());
    }
    // SAFETY: plain call.
    unsafe { DeleteFileW(nopriv.ptr()) };

    // An absolute link and a relative one.
    let target = Path::new(d, b"t.txt");
    let mut full = [0u16; 300];
    // SAFETY: valid buffers.
    let full_len =
        unsafe { GetFullPathNameW(target.ptr(), 300, full.as_mut_ptr(), core::ptr::null_mut()) }
            as usize;
    c.ok(full_len > 0 && full_len < 290, full_len as u64)?;
    let abs = Path::new(d, b"abs.lnk");
    let rel = Path::new(d, b"rel.lnk");
    // SAFETY: NUL-terminated paths.
    unsafe {
        c.eq(CreateSymbolicLinkW(abs.ptr(), full.as_ptr(), 0).into(), 1)?;
        c.eq(CreateSymbolicLinkW(rel.ptr(), Path::new(&[], b"t.txt").ptr(), 0).into(), 1)?;
    }
    out(b"reparse_test: links made\r\n");
    for link in [&abs, &rel] {
        let (buf, len) = contents(link).map_err(|e| c.eq(e.into(), 0).err().unwrap_or(1))?;
        c.ok(&buf[..len] == b"hello", len as u64)?;
        // SAFETY: NUL-terminated path.
        c.eq(
            unsafe { GetFileAttributesW(link.ptr()) }.into(),
            (FILE_ATTRIBUTE_ARCHIVE | FILE_ATTRIBUTE_REPARSE_POINT).into(),
        )?;
        let mut data = [0u32; 9];
        // SAFETY: a WIN32_FILE_ATTRIBUTE_DATA.
        c.eq(unsafe { GetFileAttributesExW(link.ptr(), 0, data.as_mut_ptr()) }.into(), 1)?;
        c.eq(data[0].into(), (FILE_ATTRIBUTE_ARCHIVE | FILE_ATTRIBUTE_REPARSE_POINT).into())?;
        c.eq(u64::from(data[7]) << 32 | u64::from(data[8]), 0)?;
        let (attributes, tag) = tag_info(link, true);
        c.eq(attributes.into(), (FILE_ATTRIBUTE_ARCHIVE | FILE_ATTRIBUTE_REPARSE_POINT).into())?;
        c.eq(tag.into(), IO_REPARSE_TAG_SYMLINK.into())?;
        // Opened itself: empty.
        let h = open(link, GENERIC_READ, OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT);
        c.ok(h != INVALID_HANDLE_VALUE, 0)?;
        // SAFETY: our handle.
        unsafe {
            c.eq(GetFileSize(h, core::ptr::null_mut()).into(), 0)?;
            CloseHandle(h);
        }
        let mut find = [0u16; 296];
        // SAFETY: a WIN32_FIND_DATAW.
        let f = unsafe { FindFirstFileW(link.ptr(), find.as_mut_ptr()) };
        c.ok(f != INVALID_HANDLE_VALUE, 0)?;
        // SAFETY: our find handle.
        unsafe { FindClose(f) };
        let ints = |i: usize| u32::from(find[2 * i]) | u32::from(find[2 * i + 1]) << 16;
        c.eq(ints(0).into(), (FILE_ATTRIBUTE_ARCHIVE | FILE_ATTRIBUTE_REPARSE_POINT).into())?;
        c.eq(ints(9).into(), IO_REPARSE_TAG_SYMLINK.into())?;
    }
    c.eq(tag_info(&target, false).0.into(), FILE_ATTRIBUTE_ARCHIVE.into())?;
    c.eq(tag_info(&target, false).1.into(), 0)?;
    c.eq(tag_info(&abs, false).1.into(), 0)?;

    // The reparse data: the absolute link's substitute name is the NT path.
    let (b, n) = reparse_data(&abs).map_err(|e| c.eq(e.into(), 0).err().unwrap_or(1))?;
    c.eq(u32::from_le_bytes([b[0], b[1], b[2], b[3]]).into(), IO_REPARSE_TAG_SYMLINK.into())?;
    c.eq(u64::from(u16::from_le_bytes([b[4], b[5]])) + 8, n.into())?;
    c.eq(u32::from_le_bytes([b[16], b[17], b[18], b[19]]).into(), 0)?;
    let sub = link_name(&b, 8);
    let sub = units_of(&sub);
    c.ok(
        sub.len() == 4 + full_len && ascii_eq(&sub[..4], b"\\??\\") && sub[4..] == full[..full_len],
        sub.len() as u64,
    )?;
    let print = link_name(&b, 12);
    c.ok(units_of(&print) == &full[..full_len], 0)?;
    let (b, _) = reparse_data(&rel).map_err(|e| c.eq(e.into(), 0).err().unwrap_or(1))?;
    c.eq(u32::from_le_bytes([b[16], b[17], b[18], b[19]]).into(), 1)?;
    c.ok(ascii_eq(units_of(&link_name(&b, 8)), b"t.txt"), 0)?;
    c.ok(ascii_eq(units_of(&link_name(&b, 12)), b"t.txt"), 0)?;
    c.eq(reparse_data(&target).err().unwrap_or(0).into(), ERROR_NOT_A_REPARSE_POINT.into())?;

    // Writing through a link writes the target.
    let h = open(&rel, GENERIC_WRITE, OPEN_EXISTING, 0);
    c.ok(h != INVALID_HANDLE_VALUE, 0)?;
    let mut w = 0;
    // SAFETY: our handle and buffer.
    unsafe {
        WriteFile(h, b"HELLO!".as_ptr(), 6, &mut w, core::ptr::null_mut());
        CloseHandle(h);
    }
    let (buf, len) = contents(&target).map_err(|e| c.eq(e.into(), 0).err().unwrap_or(1))?;
    c.ok(&buf[..len] == b"HELLO!", len as u64)?;

    // A directory link, followed in the middle of a path.
    let sub_dir = Path::new(d, b"d");
    let dl = Path::new(d, b"dl");
    // SAFETY: NUL-terminated paths.
    unsafe {
        c.eq(CreateDirectoryW(sub_dir.ptr(), core::ptr::null_mut()).into(), 1)?;
        c.ok(write_file(&Path::new(d, b"d\\in.txt"), b"inner"), 0)?;
        let flags = SYMBOLIC_LINK_FLAG_DIRECTORY;
        c.eq(CreateSymbolicLinkW(dl.ptr(), Path::new(&[], b"d").ptr(), flags).into(), 1)?;
        c.eq(
            GetFileAttributesW(dl.ptr()).into(),
            (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT).into(),
        )?;
        c.eq(
            GetFileAttributesW(Path::new(d, b"dl\\in.txt").ptr()).into(),
            FILE_ATTRIBUTE_ARCHIVE.into(),
        )?;
    }
    let (buf, len) =
        contents(&Path::new(d, b"dl\\in.txt")).map_err(|e| c.eq(e.into(), 0).err().unwrap_or(1))?;
    c.ok(&buf[..len] == b"inner", len as u64)?;
    // A file created through the directory link lands in its target.
    c.ok(write_file(&Path::new(d, b"dl\\made.txt"), b"m"), 0)?;
    // SAFETY: NUL-terminated paths.
    unsafe {
        c.eq(
            GetFileAttributesW(Path::new(d, b"d\\made.txt").ptr()).into(),
            FILE_ATTRIBUTE_ARCHIVE.into(),
        )?;
        c.eq(DeleteFileW(Path::new(d, b"d\\made.txt").ptr()).into(), 1)?;
    }

    // The same name again; a dangling link; a loop.
    // SAFETY: NUL-terminated paths.
    unsafe {
        SetLastError(0);
        c.eq(CreateSymbolicLinkW(abs.ptr(), full.as_ptr(), 0).into(), 0)?;
        c.eq(GetLastError().into(), ERROR_ALREADY_EXISTS.into())?;
        let dang = Path::new(d, b"dang.lnk");
        c.eq(CreateSymbolicLinkW(dang.ptr(), Path::new(&[], b"missing.txt").ptr(), 0).into(), 1)?;
        c.eq(contents(&dang).err().unwrap_or(0).into(), ERROR_FILE_NOT_FOUND.into())?;
        let (a, b) = (Path::new(d, b"a.lnk"), Path::new(d, b"b.lnk"));
        c.eq(CreateSymbolicLinkW(a.ptr(), Path::new(&[], b"b.lnk").ptr(), 0).into(), 1)?;
        c.eq(CreateSymbolicLinkW(b.ptr(), Path::new(&[], b"a.lnk").ptr(), 0).into(), 1)?;
        c.eq(contents(&a).err().unwrap_or(0).into(), ERROR_CANT_RESOLVE_FILENAME.into())?;
    }

    // FSCTL_DELETE_REPARSE_POINT: a plain empty file is left.
    let cleared = Path::new(d, b"cleared.lnk");
    // SAFETY: NUL-terminated paths; our handle and buffer.
    unsafe {
        c.eq(CreateSymbolicLinkW(cleared.ptr(), Path::new(&[], b"t.txt").ptr(), 0).into(), 1)?;
        let h = open(
            &cleared,
            GENERIC_WRITE | FILE_WRITE_ATTRIBUTES | DELETE,
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
        );
        c.ok(h != INVALID_HANDLE_VALUE, 0)?;
        // REPARSE_GUID_DATA_BUFFER_HEADER_SIZE: the tag, no data.
        let mut header = [0u8; 24];
        header[..4].copy_from_slice(&IO_REPARSE_TAG_SYMLINK.to_le_bytes());
        let mut n = 0;
        let ok = DeviceIoControl(
            h,
            FSCTL_DELETE_REPARSE_POINT,
            header.as_ptr().cast(),
            24,
            core::ptr::null_mut(),
            0,
            &mut n,
            core::ptr::null_mut(),
        );
        c.eq(ok.into(), 1)?;
        CloseHandle(h);
        c.eq(GetFileAttributesW(cleared.ptr()).into(), FILE_ATTRIBUTE_ARCHIVE.into())?;
        c.eq(contents(&cleared).map(|(_, n)| n).unwrap_or(99) as u64, 0)?;
    }

    // Deleting a link leaves its target.
    // SAFETY: NUL-terminated paths.
    unsafe {
        c.eq(DeleteFileW(abs.ptr()).into(), 1)?;
        c.eq(GetFileAttributesW(abs.ptr()).into(), INVALID_FILE_ATTRIBUTES.into())?;
        c.eq(GetFileAttributesW(target.ptr()).into(), FILE_ATTRIBUTE_ARCHIVE.into())?;
        c.eq(RemoveDirectoryW(dl.ptr()).into(), 1)?;
        c.eq(
            GetFileAttributesW(Path::new(d, b"d\\in.txt").ptr()).into(),
            FILE_ATTRIBUTE_ARCHIVE.into(),
        )?;
        clean(d);
        c.eq(RemoveDirectoryW(dir.ptr()).into(), 1)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"reparse_test: ok\r\n");
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
