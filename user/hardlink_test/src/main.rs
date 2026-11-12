//! Hard links and NT's delete dispositions, as Click-to-Run's publish
//! uses them (ROADMAP OF: `fileops::HardlinkOperation`, then
//! SetFileInformationByHandle's FileDispositionInfoEx). Expected values
//! are those of Wine's conformance tests, which pass on Windows:
//! - kernel32's test_hard_link (dlls/kernel32/tests/file.c):
//!   CreateHardLinkA/W makes a second name whose open reports its own
//!   name (FileNameInformation) and reads the same data; an existing
//!   name (in any case, or a directory) is ERROR_ALREADY_EXISTS; a
//!   directory cannot be linked (ERROR_ACCESS_DENIED). Added, as Windows
//!   documents hard links: the link count (nNumberOfLinks) and the file
//!   index are the file's, a write through one name shows through the
//!   other, deleting the first name leaves the file at the other, a
//!   directory holding a link is not empty, a missing file is
//!   ERROR_FILE_NOT_FOUND.
//! - ntdll's test_file_link_information and test_file_link_information_ex
//!   (dlls/ntdll/tests/file.c), for FileLinkInformation (11) and
//!   FileLinkInformationEx (72): a new name is STATUS_SUCCESS (the
//!   handle's own name unchanged); an existing one
//!   STATUS_OBJECT_NAME_COLLISION without FILE_LINK_REPLACE_IF_EXISTS
//!   (the IO_STATUS_BLOCK untouched), replaced with it, except an open
//!   file or a directory (STATUS_ACCESS_DENIED); a directory's handle is
//!   STATUS_FILE_IS_A_DIRECTORY; a RootDirectory with a bare name; the
//!   file's own name is a collision, or with the flag a success; a
//!   read-only target is STATUS_ACCESS_DENIED unless
//!   FILE_LINK_IGNORE_READONLY_ATTRIBUTE (Ex).
//! - ntdll's test_file_disposition_information, for
//!   FileDispositionInformationEx (64): FILE_DISPOSITION_POSIX_SEMANTICS
//!   removes the name when its handle closes though another stays open;
//!   a read-only file is STATUS_CANNOT_DELETE (FileDispositionInformation)
//!   unless FILE_DISPOSITION_IGNORE_READONLY_ATTRIBUTE; a delete set can
//!   be reset (flags 0); FILE_DISPOSITION_ON_CLOSE alone clears
//!   FILE_DELETE_ON_CLOSE. kernel32's SetFileInformationByHandle passes
//!   FileDispositionInfoEx (21) to it.
//!
//! The same source builds `wow64_hardlink_test`, the 32-bit (x86)
//! program. Prints `hardlink_test: ok` (`wow64_hardlink_test: ok`), or
//! `<name>: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "hardlink_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_hardlink_test";

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
    fn ExitProcess(code: u32) -> !;
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
    fn GetTempPathW(size: u32, buffer: *mut u16) -> u32;
    fn SetCurrentDirectoryW(path: *const u16) -> i32;
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
    fn RemoveDirectoryW(path: *const u16) -> i32;
    fn DeleteFileW(path: *const u16) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn SetFileAttributesW(path: *const u16, attributes: u32) -> i32;
    fn GetFileInformationByHandle(h: Handle, info: *mut [u32; 13]) -> i32;
    fn SetFileInformationByHandle(h: Handle, class: u32, info: *const u8, size: u32) -> i32;
    fn CreateHardLinkW(link: *const u16, existing: *const u16, security: usize) -> i32;
    fn CreateHardLinkA(link: *const u8, existing: *const u8, security: usize) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn NtSetInformationFile(
        h: Handle,
        iosb: *mut [usize; 2],
        info: *const u8,
        len: u32,
        class: u32,
    ) -> u32;
    fn NtQueryInformationFile(
        h: Handle,
        iosb: *mut [usize; 2],
        info: *mut u8,
        len: u32,
        class: u32,
    ) -> u32;
}

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_DIR_NOT_EMPTY: u32 = 145;
const ERROR_ALREADY_EXISTS: u32 = 183;

const STATUS_SUCCESS: u32 = 0;
const STATUS_OBJECT_NAME_COLLISION: u32 = 0xc000_0035;
const STATUS_ACCESS_DENIED: u32 = 0xc000_0022;
const STATUS_FILE_IS_A_DIRECTORY: u32 = 0xc000_00ba;
const STATUS_CANNOT_DELETE: u32 = 0xc000_0121;

const FILE_LINK_INFORMATION: u32 = 11;
const FILE_LINK_INFORMATION_EX: u32 = 72;
const FILE_DISPOSITION_INFORMATION: u32 = 13;
const FILE_DISPOSITION_INFORMATION_EX: u32 = 64;
const FILE_NAME_INFORMATION: u32 = 9;
/// SetFileInformationByHandle's FileDispositionInfoEx.
const FILE_DISPOSITION_INFO_EX: u32 = 21;

const FILE_LINK_REPLACE_IF_EXISTS: u32 = 0x1;
const FILE_LINK_IGNORE_READONLY_ATTRIBUTE: u32 = 0x40;
const FILE_DISPOSITION_DELETE: u32 = 0x1;
const FILE_DISPOSITION_POSIX_SEMANTICS: u32 = 0x2;
const FILE_DISPOSITION_ON_CLOSE: u32 = 0x8;
const FILE_DISPOSITION_IGNORE_READONLY_ATTRIBUTE: u32 = 0x10;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const DELETE: u32 = 0x0001_0000;
const FILE_READ_DATA: u32 = 1;
const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;
const FILE_SHARE_DELETE: u32 = 4;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const FILE_ATTRIBUTE_READONLY: u32 = 1;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
const INVALID_HANDLE_VALUE: Handle = usize::MAX;
const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
/// What an IO_STATUS_BLOCK's Status holds before a call that leaves it.
const UNTOUCHED: usize = 0xdead_beef;

/// FILE_LINK_INFORMATION's header: Flags (ReplaceIfExists), RootDirectory,
/// FileNameLength (20 bytes on x64, 12 on x86).
const LINK_HEADER: usize = 2 * size_of::<usize>() + 4;

struct Line {
    buf: [u8; 600],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 600], n: 0 }
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
    fn eq(&mut self, got: u32, want: u32) -> Result<(), u32> {
        self.ok(got == want, got.into())
    }

    /// A Win32 call's FALSE and its last error.
    #[track_caller]
    fn fails(&mut self, ret: i32, error: u32) -> Result<(), u32> {
        // SAFETY: reads the thread's last error.
        let e = unsafe { GetLastError() };
        self.ok(ret == 0 && e == error, (u64::from(ret as u32) << 32) | u64::from(e))
    }
}

/// A NUL-terminated UTF-16 path.
#[derive(Clone, Copy)]
struct W {
    buf: [u16; 320],
    n: usize,
}

impl W {
    fn new() -> Self {
        W { buf: [0; 320], n: 0 }
    }

    fn from(s: &[u8]) -> Self {
        W::new().s(s)
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            self = self.c(u16::from(b));
        }
        self
    }

    fn c(mut self, c: u16) -> Self {
        if self.n + 1 < self.buf.len() {
            self.buf[self.n] = c;
            self.n += 1;
            self.buf[self.n] = 0;
        }
        self
    }

    fn w(mut self, other: &W) -> Self {
        for &c in other.chars() {
            self = self.c(c);
        }
        self
    }

    fn chars(&self) -> &[u16] {
        &self.buf[..self.n]
    }

    fn p(&self) -> *const u16 {
        self.buf.as_ptr()
    }
}

fn lower(c: u16) -> u16 {
    if (u16::from(b'A')..=u16::from(b'Z')).contains(&c) { c + 32 } else { c }
}

fn same_ignoring_case(a: &[u16], b: &[u16]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| lower(x) == lower(y))
}

fn open(path: &W, access: u32, share: u32, disposition: u32, flags: u32) -> Handle {
    // SAFETY: a NUL-terminated path.
    unsafe { CreateFileW(path.p(), access, share, 0, disposition, flags, 0) }
}

fn close(h: Handle) {
    // SAFETY: a handle we opened.
    unsafe { CloseHandle(h) };
}

/// A file named `path` holding `data`.
fn create_file(path: &W, data: &[u8]) -> bool {
    let h = open(path, GENERIC_WRITE, 0, CREATE_ALWAYS, 0);
    if h == INVALID_HANDLE_VALUE {
        return false;
    }
    let mut written = 0;
    // SAFETY: our buffer.
    let ok = unsafe { WriteFile(h, data.as_ptr(), data.len() as u32, &mut written, 0) };
    close(h);
    ok != 0 && written as usize == data.len()
}

/// The first bytes of file `path` (opened sharing everything).
fn read_file(path: &W, buf: &mut [u8]) -> usize {
    let all = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
    let h = open(path, GENERIC_READ, all, OPEN_EXISTING, 0);
    if h == INVALID_HANDLE_VALUE {
        return 0;
    }
    let mut read = 0;
    // SAFETY: our buffer.
    unsafe { ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut read, 0) };
    close(h);
    read as usize
}

fn exists(path: &W) -> bool {
    // SAFETY: a NUL-terminated path.
    unsafe { GetFileAttributesW(path.p()) != INVALID_FILE_ATTRIBUTES }
}

/// Gone as Wine's tests check it: INVALID_FILE_ATTRIBUTES with
/// ERROR_FILE_NOT_FOUND.
fn gone(path: &W) -> bool {
    // SAFETY: a NUL-terminated path; then the thread's last error.
    unsafe { GetFileAttributesW(path.p()) == INVALID_FILE_ATTRIBUTES && GetLastError() == 2 }
}

/// Removes file or directory `path` if it is there (read-only too).
fn remove(path: &W) {
    // SAFETY: NUL-terminated paths.
    unsafe {
        SetFileAttributesW(path.p(), FILE_ATTRIBUTE_NORMAL);
        if DeleteFileW(path.p()) == 0 {
            RemoveDirectoryW(path.p());
        }
    }
}

/// BY_HANDLE_FILE_INFORMATION's nNumberOfLinks and file index.
fn links_and_index(h: Handle) -> (u32, u64) {
    let mut info = [0u32; 13];
    // SAFETY: a 52-byte BY_HANDLE_FILE_INFORMATION.
    if unsafe { GetFileInformationByHandle(h, &mut info) } == 0 {
        return (0, 0);
    }
    (info[10], (u64::from(info[11]) << 32) | u64::from(info[12]))
}

/// FileNameInformation of `h`: the name without the drive.
fn handle_name(h: Handle, out: &mut W) -> u32 {
    #[repr(C, align(8))]
    struct Buf([u8; 1024]);
    let mut b = Buf([0; 1024]);
    let mut iosb = [0usize; 2];
    // SAFETY: our buffer.
    let status = unsafe {
        NtQueryInformationFile(h, &mut iosb, b.0.as_mut_ptr(), 1024, FILE_NAME_INFORMATION)
    };
    let len = u32::from_le_bytes([b.0[0], b.0[1], b.0[2], b.0[3]]) as usize / 2;
    *out = W::new();
    for i in 0..len.min(300) {
        *out = out.c(u16::from_le_bytes([b.0[4 + 2 * i], b.0[5 + 2 * i]]));
    }
    status
}

/// NtSetInformationFile(FileLinkInformation(Ex)) of `h` to `name` (an NT
/// path, or a bare name under `root`): the status and the
/// IO_STATUS_BLOCK's Status (set to UNTOUCHED before).
fn link(h: Handle, root: Handle, name: &[u16], flags: u32, class: u32) -> (u32, usize) {
    #[repr(C, align(8))]
    struct Buf([u8; LINK_HEADER + 640]);
    let mut b = Buf([0; LINK_HEADER + 640]);
    b.0[0..4].copy_from_slice(&flags.to_le_bytes());
    b.0[size_of::<usize>()..2 * size_of::<usize>()].copy_from_slice(&root.to_le_bytes());
    let length_at = LINK_HEADER - 4;
    b.0[length_at..LINK_HEADER].copy_from_slice(&((name.len() * 2) as u32).to_le_bytes());
    for (i, c) in name.iter().enumerate() {
        b.0[LINK_HEADER + 2 * i..LINK_HEADER + 2 * i + 2].copy_from_slice(&c.to_le_bytes());
    }
    let mut iosb = [UNTOUCHED, 0];
    let len = (LINK_HEADER + 2 * name.len()) as u32;
    // SAFETY: our buffer of `len` bytes.
    let status = unsafe { NtSetInformationFile(h, &mut iosb, b.0.as_ptr(), len, class) };
    (status, iosb[0] as u32 as usize)
}

/// NtSetInformationFile of a 4-byte disposition (Ex flags, or the BOOLEAN).
fn dispose(h: Handle, value: u32, class: u32) -> u32 {
    let info = value.to_le_bytes();
    let mut iosb = [0usize; 2];
    let len = if class == FILE_DISPOSITION_INFORMATION { 1 } else { 4 };
    // SAFETY: our 4-byte buffer.
    unsafe { NtSetInformationFile(h, &mut iosb, info.as_ptr(), len, class) }
}

/// Wine's test_hard_link through CreateHardLinkA/W, in the current
/// directory.
fn kernel32_links(c: &mut Checks) -> Result<(), u32> {
    let (dir1, dir2) = (W::from(b"winetest_dir1"), W::from(b"winetest_dir2"));
    let (file1, file2, file3) =
        (W::from(b"winetest_file1"), W::from(b"winetest_file2"), W::from(b"winetest_file3"));
    // SAFETY: NUL-terminated paths throughout.
    unsafe {
        c.ok(CreateDirectoryW(dir1.p(), 0) != 0, GetLastError().into())?;
        c.ok(CreateDirectoryW(dir2.p(), 0) != 0, GetLastError().into())?;
    }
    c.ok(create_file(&file1, b"winetest_file1"), 0)?;
    c.ok(create_file(&file2, b"winetest_file2"), 0)?;

    // SAFETY: NUL-terminated paths.
    let ret = unsafe { CreateHardLinkW(file3.p(), file1.p(), 0) };
    c.ok(ret != 0, unsafe { GetLastError() }.into())?;
    let h = open(&file3, FILE_READ_DATA, 0, OPEN_EXISTING, 0);
    c.ok(h != INVALID_HANDLE_VALUE, unsafe { GetLastError() }.into())?;
    let mut name = W::new();
    c.eq(handle_name(h, &mut name), STATUS_SUCCESS)?;
    let want = W::from(b"\\winetest_file3");
    let tail = name.chars().len().saturating_sub(want.n);
    c.ok(same_ignoring_case(&name.chars()[tail..], want.chars()), name.n as u64)?;
    let mut buf = [0u8; 20];
    let mut read = 0;
    // SAFETY: our buffer.
    c.ok(unsafe { ReadFile(h, buf.as_mut_ptr(), 20, &mut read, 0) } != 0, 0)?;
    c.ok(&buf[..read as usize] == b"winetest_file1", read.into())?;
    // The file's: two names, the first name's index.
    let (links, index) = links_and_index(h);
    c.eq(links, 2)?;
    close(h);
    let h1 = open(&file1, FILE_READ_DATA, FILE_SHARE_READ, OPEN_EXISTING, 0);
    c.ok(h1 != INVALID_HANDLE_VALUE, 0)?;
    let (links1, index1) = links_and_index(h1);
    c.eq(links1, 2)?;
    c.ok(index1 == index, index1)?;
    close(h1);

    // SAFETY: NUL-terminated paths.
    c.ok(unsafe { DeleteFileW(file3.p()) } != 0, unsafe { GetLastError() }.into())?;
    c.ok(gone(&file3), 0)?;
    c.ok(exists(&file1), 0)?;
    let h1 = open(&file1, FILE_READ_DATA, FILE_SHARE_READ, OPEN_EXISTING, 0);
    c.eq(links_and_index(h1).0, 1)?;
    close(h1);

    // SAFETY: NUL-terminated paths and our last error throughout.
    unsafe {
        SetLastError(0xdead_beef);
        c.fails(CreateHardLinkW(file2.p(), file1.p(), 0), ERROR_ALREADY_EXISTS)?;
        SetLastError(0xdead_beef);
        c.fails(
            CreateHardLinkA(
                c"WineTest_File1".as_ptr().cast(),
                c"winetest_file1".as_ptr().cast(),
                0,
            ),
            ERROR_ALREADY_EXISTS,
        )?;
        SetLastError(0xdead_beef);
        c.fails(CreateHardLinkW(file3.p(), dir1.p(), 0), ERROR_ACCESS_DENIED)?;
        SetLastError(0xdead_beef);
        c.fails(CreateHardLinkW(dir2.p(), dir1.p(), 0), ERROR_ACCESS_DENIED)?;
        SetLastError(0xdead_beef);
        c.fails(CreateHardLinkW(dir1.p(), file1.p(), 0), ERROR_ALREADY_EXISTS)?;
        let missing = W::from(b"winetest_missing");
        SetLastError(0xdead_beef);
        c.fails(CreateHardLinkW(file3.p(), missing.p(), 0), ERROR_FILE_NOT_FOUND)?;
    }

    // A write through the link shows through the first name; the first
    // name deleted, the file lives on at the link.
    let inner = W::from(b"winetest_dir2\\inner");
    // SAFETY: NUL-terminated paths.
    c.ok(
        unsafe {
            CreateHardLinkA(
                c"winetest_dir2\\inner".as_ptr().cast(),
                c"winetest_file1".as_ptr().cast(),
                0,
            )
        } != 0,
        unsafe { GetLastError() }.into(),
    )?;
    c.ok(create_file(&inner, b"changed"), 0)?;
    let n = read_file(&file1, &mut buf);
    c.ok(&buf[..n] == b"changed", n as u64)?;
    // SAFETY: NUL-terminated paths and our last error.
    unsafe {
        c.fails(RemoveDirectoryW(dir2.p()), ERROR_DIR_NOT_EMPTY)?;
        c.ok(DeleteFileW(file1.p()) != 0, GetLastError().into())?;
    }
    c.ok(gone(&file1), 0)?;
    let n = read_file(&inner, &mut buf);
    c.ok(&buf[..n] == b"changed", n as u64)?;
    // SAFETY: NUL-terminated paths.
    unsafe {
        c.ok(DeleteFileW(inner.p()) != 0, GetLastError().into())?;
        c.ok(RemoveDirectoryW(dir1.p()) != 0, GetLastError().into())?;
        c.ok(RemoveDirectoryW(dir2.p()) != 0, GetLastError().into())?;
        c.ok(DeleteFileW(file2.p()) != 0, GetLastError().into())?;
    }
    Ok(())
}

/// Wine's test_file_link_information (and _ex for class 72) in
/// directory `base` (a full DOS path ending in `\`).
fn nt_links(c: &mut Checks, base: &W, class: u32) -> Result<(), u32> {
    let old = W::new().w(base).s(b"old");
    let new = W::new().w(base).s(b"new");
    let nt_new = W::from(b"\\??\\").w(&new);
    let fresh = |c: &mut Checks| -> Result<Handle, u32> {
        remove(&old);
        remove(&new);
        let h = open(&old, GENERIC_WRITE | DELETE, 0, CREATE_ALWAYS, 0);
        c.ok(h != INVALID_HANDLE_VALUE, 0)?;
        Ok(h)
    };

    // A new name.
    let h = fresh(c)?;
    let (status, io) = link(h, 0, nt_new.chars(), 0, class);
    c.eq(status, STATUS_SUCCESS)?;
    c.ok(io == STATUS_SUCCESS as usize, io as u64)?;
    c.ok(exists(&old) && exists(&new), 0)?;
    let mut name = W::new();
    c.eq(handle_name(h, &mut name), STATUS_SUCCESS)?;
    c.ok(same_ignoring_case(name.chars(), &old.chars()[2..]), name.n as u64)?;
    close(h);

    // An existing name: a collision, then replaced.
    let h = fresh(c)?;
    c.ok(create_file(&new, b"x"), 0)?;
    let (status, io) = link(h, 0, nt_new.chars(), 0, class);
    c.eq(status, STATUS_OBJECT_NAME_COLLISION)?;
    c.ok(io == UNTOUCHED, io as u64)?;
    c.ok(exists(&old) && exists(&new), 0)?;
    let (status, io) = link(h, 0, nt_new.chars(), FILE_LINK_REPLACE_IF_EXISTS, class);
    c.eq(status, STATUS_SUCCESS)?;
    c.ok(io == STATUS_SUCCESS as usize, io as u64)?;
    c.ok(exists(&old) && exists(&new), 0)?;
    close(h);
    let mut buf = [0u8; 4];
    c.eq(read_file(&new, &mut buf) as u32, 0)?;

    // An existing name held open: a collision, and not replaced.
    let h = fresh(c)?;
    let h2 = open(&new, GENERIC_WRITE | DELETE, 0, CREATE_ALWAYS, 0);
    c.ok(h2 != INVALID_HANDLE_VALUE, 0)?;
    c.eq(link(h, 0, nt_new.chars(), 0, class).0, STATUS_OBJECT_NAME_COLLISION)?;
    let (status, io) = link(h, 0, nt_new.chars(), FILE_LINK_REPLACE_IF_EXISTS, class);
    c.eq(status, STATUS_ACCESS_DENIED)?;
    c.ok(io == UNTOUCHED, io as u64)?;
    c.ok(exists(&old) && exists(&new), 0)?;
    close(h);
    close(h2);

    // A directory's handle.
    remove(&old);
    remove(&new);
    // SAFETY: a NUL-terminated path.
    c.ok(unsafe { CreateDirectoryW(old.p(), 0) } != 0, 0)?;
    let h = open(&old, GENERIC_WRITE | DELETE, 0, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS);
    c.ok(h != INVALID_HANDLE_VALUE, 0)?;
    c.eq(link(h, 0, nt_new.chars(), 0, class).0, STATUS_FILE_IS_A_DIRECTORY)?;
    c.ok(gone(&new), 0)?;
    c.ok(create_file(&new, b""), 0)?;
    c.eq(
        link(h, 0, nt_new.chars(), FILE_LINK_REPLACE_IF_EXISTS, class).0,
        STATUS_FILE_IS_A_DIRECTORY,
    )?;
    close(h);

    // A file onto a directory.
    let h = fresh(c)?;
    // SAFETY: a NUL-terminated path.
    c.ok(unsafe { CreateDirectoryW(new.p(), 0) } != 0, 0)?;
    c.eq(link(h, 0, nt_new.chars(), 0, class).0, STATUS_OBJECT_NAME_COLLISION)?;
    c.eq(link(h, 0, nt_new.chars(), FILE_LINK_REPLACE_IF_EXISTS, class).0, STATUS_ACCESS_DENIED)?;
    close(h);

    // A RootDirectory and a bare name.
    let h = fresh(c)?;
    let dir = open(base, 0, 0, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS);
    c.ok(dir != INVALID_HANDLE_VALUE, 0)?;
    let (status, io) = link(h, dir, W::from(b"new").chars(), 0, class);
    c.eq(status, STATUS_SUCCESS)?;
    c.ok(io == STATUS_SUCCESS as usize, io as u64)?;
    c.ok(exists(&old) && exists(&new), 0)?;
    c.eq(handle_name(h, &mut name), STATUS_SUCCESS)?;
    c.ok(same_ignoring_case(name.chars(), &old.chars()[2..]), name.n as u64)?;
    close(dir);
    close(h);

    // Its own name.
    let h = fresh(c)?;
    let nt_old = W::from(b"\\??\\").w(&old);
    let (status, io) = link(h, 0, nt_old.chars(), 0, class);
    c.eq(status, STATUS_OBJECT_NAME_COLLISION)?;
    c.ok(io == UNTOUCHED, io as u64)?;
    c.eq(link(h, 0, nt_old.chars(), FILE_LINK_REPLACE_IF_EXISTS, class).0, STATUS_SUCCESS)?;
    c.ok(exists(&old), 0)?;
    close(h);

    if class == FILE_LINK_INFORMATION_EX {
        // A read-only target.
        let h = fresh(c)?;
        let h2 = open(&new, GENERIC_WRITE, 0, CREATE_ALWAYS, FILE_ATTRIBUTE_READONLY);
        c.ok(h2 != INVALID_HANDLE_VALUE, 0)?;
        close(h2);
        let (status, io) = link(h, 0, nt_new.chars(), FILE_LINK_REPLACE_IF_EXISTS, class);
        c.eq(status, STATUS_ACCESS_DENIED)?;
        c.ok(io == UNTOUCHED, io as u64)?;
        c.ok(exists(&old) && exists(&new), 0)?;
        let flags = FILE_LINK_REPLACE_IF_EXISTS | FILE_LINK_IGNORE_READONLY_ATTRIBUTE;
        let (status, io) = link(h, 0, nt_new.chars(), flags, class);
        c.eq(status, STATUS_SUCCESS)?;
        c.ok(io == STATUS_SUCCESS as usize, io as u64)?;
        c.ok(exists(&old) && exists(&new), 0)?;
        close(h);
    }
    remove(&old);
    remove(&new);
    Ok(())
}

/// Wine's test_file_disposition_information's Ex cases, and kernel32's
/// FileDispositionInfoEx, in directory `base`.
fn dispositions(c: &mut Checks, base: &W) -> Result<(), u32> {
    let file = W::new().w(base).s(b"dis");
    let ex = FILE_DISPOSITION_INFORMATION_EX;
    let both = |c: &mut Checks| -> Result<(Handle, Handle), u32> {
        remove(&file);
        let h = open(&file, GENERIC_WRITE | DELETE, FILE_SHARE_DELETE, CREATE_ALWAYS, 0);
        c.ok(h != INVALID_HANDLE_VALUE, 0)?;
        let h2 = open(&file, DELETE, FILE_SHARE_DELETE | FILE_SHARE_WRITE, OPEN_EXISTING, 0);
        c.ok(h2 != INVALID_HANDLE_VALUE, 0)?;
        Ok((h, h2))
    };

    // POSIX semantics: the name goes when its handle closes.
    let (h, h2) = both(c)?;
    c.eq(
        dispose(h, FILE_DISPOSITION_DELETE | FILE_DISPOSITION_POSIX_SEMANTICS, ex),
        STATUS_SUCCESS,
    )?;
    close(h);
    c.ok(gone(&file), 0)?;
    close(h2);
    c.ok(gone(&file), 0)?;
    // ... and not before.
    let (h, h2) = both(c)?;
    c.eq(
        dispose(h, FILE_DISPOSITION_DELETE | FILE_DISPOSITION_POSIX_SEMANTICS, ex),
        STATUS_SUCCESS,
    )?;
    close(h2);
    c.ok(!gone(&file), 0)?;
    close(h);
    c.ok(gone(&file), 0)?;

    // A read-only file.
    remove(&file);
    let h = open(&file, GENERIC_WRITE | DELETE, 0, CREATE_ALWAYS, FILE_ATTRIBUTE_READONLY);
    c.ok(h != INVALID_HANDLE_VALUE, 0)?;
    c.eq(dispose(h, 1, FILE_DISPOSITION_INFORMATION), STATUS_CANNOT_DELETE)?;
    c.eq(dispose(h, FILE_DISPOSITION_DELETE, ex), STATUS_CANNOT_DELETE)?;
    close(h);
    c.ok(!gone(&file), 0)?;
    remove(&file);
    let h = open(&file, GENERIC_WRITE | DELETE, 0, CREATE_ALWAYS, FILE_ATTRIBUTE_READONLY);
    c.ok(h != INVALID_HANDLE_VALUE, 0)?;
    let flags = FILE_DISPOSITION_DELETE | FILE_DISPOSITION_IGNORE_READONLY_ATTRIBUTE;
    c.eq(dispose(h, flags, ex), STATUS_SUCCESS)?;
    close(h);
    c.ok(gone(&file), 0)?;

    // Set, then reset.
    let h = open(&file, GENERIC_WRITE | DELETE, 0, CREATE_ALWAYS, 0);
    c.ok(h != INVALID_HANDLE_VALUE, 0)?;
    c.eq(dispose(h, FILE_DISPOSITION_DELETE, ex), STATUS_SUCCESS)?;
    c.eq(dispose(h, 0, ex), STATUS_SUCCESS)?;
    close(h);
    c.ok(!gone(&file), 0)?;

    // FILE_DISPOSITION_ON_CLOSE alone clears FILE_DELETE_ON_CLOSE.
    remove(&file);
    let h = open(&file, GENERIC_WRITE | DELETE, 0, CREATE_ALWAYS, FILE_FLAG_DELETE_ON_CLOSE);
    c.ok(h != INVALID_HANDLE_VALUE, 0)?;
    c.eq(dispose(h, FILE_DISPOSITION_ON_CLOSE, ex), STATUS_SUCCESS)?;
    close(h);
    c.ok(!gone(&file), 0)?;

    // kernel32's FileDispositionInfoEx.
    let (h, h2) = both(c)?;
    let flags = (FILE_DISPOSITION_DELETE | FILE_DISPOSITION_POSIX_SEMANTICS).to_le_bytes();
    // SAFETY: a 4-byte FILE_DISPOSITION_INFO_EX.
    let ret = unsafe { SetFileInformationByHandle(h, FILE_DISPOSITION_INFO_EX, flags.as_ptr(), 4) };
    c.ok(ret != 0, unsafe { GetLastError() }.into())?;
    close(h);
    c.ok(gone(&file), 0)?;
    close(h2);
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let mut temp = [0u16; 260];
    // SAFETY: our buffer.
    let n = unsafe { GetTempPathW(260, temp.as_mut_ptr()) } as usize;
    c.ok(n > 0 && n < 200, n as u64)?;
    let mut base = W::new();
    for &ch in &temp[..n] {
        base = base.c(ch);
    }
    let base = base.s(NAME.as_bytes());
    // SAFETY: NUL-terminated paths.
    unsafe {
        CreateDirectoryW(base.p(), 0);
        c.ok(SetCurrentDirectoryW(base.p()) != 0, GetLastError().into())?;
    }
    let base = base.c(u16::from(b'\\'));
    kernel32_links(&mut c)?;
    nt_links(&mut c, &base, FILE_LINK_INFORMATION)?;
    nt_links(&mut c, &base, FILE_LINK_INFORMATION_EX)?;
    dispositions(&mut c, &base)?;
    remove(&W::new().w(&base).s(b"dis"));
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
