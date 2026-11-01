//! kernel32 files and directories, as Go's `os` package uses them, with
//! the results (and error codes) real Windows gives: CreateDirectoryW,
//! CreateFileW (CREATE_NEW, OPEN_EXISTING, directories with
//! FILE_FLAG_BACKUP_SEMANTICS), WriteFile/ReadFile/SetFilePointerEx,
//! GetFileInformationByHandle, GetFileSizeEx, GetFileAttributes(Ex)W,
//! FindFirstFileW/FindNextFileW, GetFileInformationByHandleEx's directory
//! classes, GetVolumeInformationByHandleW, GetFinalPathNameByHandleW,
//! GetFileTime/SetFileTime, GetFullPathNameW, GetShortPathNameW,
//! GetCompressedFileSizeW, DeleteFileW, QueryDosDeviceW (C: and NUL),
//! WriteFileGather/ReadFileScatter (page segments, overlapped),
//! WriteFileEx/ReadFileEx (the completion routine runs in the next
//! alertable wait, SleepEx returning WAIT_IO_COMPLETION, with error 0, the
//! count and the OVERLAPPED; not before),
//! GetVolumeNameForVolumeMountPointW (C:\'s \\?\Volume{GUID}\),
//! RemoveDirectoryW, all inside the temporary directory. Prints
//! `kernel32_file_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;
type Completion = extern "system" fn(u32, u32, *mut usize);

/// What the completion routine saw: calls, error, count, OVERLAPPED.
static DONE: [core::sync::atomic::AtomicUsize; 4] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; 4];

extern "system" fn completed(error: u32, count: u32, ov: *mut usize) {
    use core::sync::atomic::Ordering;
    DONE[0].fetch_add(1, Ordering::SeqCst);
    DONE[1].store(error as usize, Ordering::SeqCst);
    DONE[2].store(count as usize, Ordering::SeqCst);
    DONE[3].store(ov as usize, Ordering::SeqCst);
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn GetLastError() -> u32;
    fn QueryDosDeviceW(name: *const u16, target: *mut u16, max: u32) -> u32;
    fn GetVolumeNameForVolumeMountPointW(mount: *const u16, name: *mut u16, size: u32) -> Bool;
    fn SetLastError(code: u32);
    fn WriteFileEx(
        h: Handle,
        buf: *const u8,
        len: u32,
        ov: *mut usize,
        routine: Completion,
    ) -> Bool;
    fn ReadFileEx(h: Handle, buf: *mut u8, len: u32, ov: *mut usize, routine: Completion) -> Bool;
    fn SleepEx(ms: u32, alertable: Bool) -> u32;
    fn GetTempPathW(size: u32, buf: *mut u16) -> u32;
    fn CreateDirectoryW(path: *const u16, attrs: *mut c_void) -> Bool;
    fn RemoveDirectoryW(path: *const u16) -> Bool;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        attrs: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn SetFilePointerEx(h: Handle, distance: i64, new: *mut i64, method: u32) -> Bool;
    fn GetFileInformationByHandle(h: Handle, info: *mut u32) -> Bool;
    fn GetFileSizeEx(h: Handle, size: *mut i64) -> Bool;
    fn FlushFileBuffers(h: Handle) -> Bool;
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn GetFileAttributesExW(path: *const u16, level: u32, info: *mut u32) -> Bool;
    fn FindFirstFileW(pattern: *const u16, data: *mut u16) -> Handle;
    fn FindNextFileW(find: Handle, data: *mut u16) -> Bool;
    fn FindClose(find: Handle) -> Bool;
    fn GetFileInformationByHandleEx(h: Handle, class: u32, buf: *mut u8, len: u32) -> Bool;
    fn GetVolumeInformationByHandleW(
        h: Handle,
        name: *mut u16,
        name_len: u32,
        serial: *mut u32,
        max_component: *mut u32,
        flags: *mut u32,
        fs_name: *mut u16,
        fs_name_len: u32,
    ) -> Bool;
    fn GetFinalPathNameByHandleW(h: Handle, buf: *mut u16, len: u32, flags: u32) -> u32;
    fn GetFileTime(h: Handle, created: *mut u64, accessed: *mut u64, written: *mut u64) -> Bool;
    fn SetFileTime(
        h: Handle,
        created: *const u64,
        accessed: *const u64,
        written: *const u64,
    ) -> Bool;
    fn GetCurrentDirectoryW(len: u32, buf: *mut u16) -> u32;
    fn SetCurrentDirectoryW(path: *const u16) -> Bool;
    fn GetShortPathNameW(long: *const u16, short: *mut u16, len: u32) -> u32;
    fn GetCompressedFileSizeW(path: *const u16, high: *mut u32) -> u32;
    fn GetFullPathNameW(path: *const u16, len: u32, buf: *mut u16, file_part: *mut *mut u16)
    -> u32;
    fn DeleteFileW(path: *const u16) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn VirtualAlloc(address: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut u8;
    fn WriteFileGather(
        h: Handle,
        segments: *const u64,
        length: u32,
        reserved: *mut u32,
        overlapped: *mut [usize; 4],
    ) -> Bool;
    fn ReadFileScatter(
        h: Handle,
        segments: *const u64,
        length: u32,
        reserved: *mut u32,
        overlapped: *mut [usize; 4],
    ) -> Bool;
    fn GetOverlappedResult(h: Handle, ov: *mut [usize; 4], count: *mut u32, wait: Bool) -> Bool;
    fn ExitProcess(code: u32) -> !;
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
const SHARE_ALL: u32 = 7;
const CREATE_NEW: u32 = 1;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const INVALID_FILE_ATTRIBUTES: u32 = 0xffff_ffff;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_NO_MORE_FILES: u32 = 18;
const ERROR_FILE_EXISTS: u32 = 80;
const ERROR_ALREADY_EXISTS: u32 = 183;
const FILE_ID_BOTH_DIRECTORY_RESTART_INFO: u32 = 11;
const FILE_FULL_DIRECTORY_RESTART_INFO: u32 = 15;
const FILE_FULL_DIRECTORY_INFO: u32 = 14;

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

fn hex(v: u32) -> [u8; 8] {
    let mut s = [0u8; 8];
    for (i, c) in s.iter_mut().enumerate() {
        let d = (v >> (28 - 4 * i)) & 0xf;
        *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
    }
    s
}

struct Checks(u32);

impl Checks {
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"kernel32_file_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

/// A NUL-terminated UTF-16 path: `base` + `rest` (ASCII).
struct Path {
    buf: [u16; 300],
}

impl Path {
    fn new(base: &[u16], rest: &[u8]) -> Path {
        let mut buf = [0u16; 300];
        buf[..base.len()].copy_from_slice(base);
        for (i, &b) in rest.iter().enumerate() {
            buf[base.len() + i] = u16::from(b);
        }
        Path { buf }
    }

    fn ptr(&self) -> *const u16 {
        self.buf.as_ptr()
    }
}

fn names_eq(name: &[u16], s: &[u8]) -> bool {
    name.len() == s.len() && name.iter().zip(s).all(|(&a, &b)| a == u16::from(b))
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let mut temp = [0u16; 300];
    // SAFETY: documented kernel32 calls with valid arguments.
    unsafe {
        let n = GetTempPathW(300, temp.as_mut_ptr()) as usize;
        let temp = &temp[..n]; // ends with a backslash
        let dir = Path::new(temp, b"gala_k32");
        let file = Path::new(temp, b"gala_k32\\a.txt");

        // Directories.
        c.ok(CreateDirectoryW(dir.ptr(), core::ptr::null_mut()) != 0, 0)?;
        SetLastError(0);
        c.eq(CreateDirectoryW(dir.ptr(), core::ptr::null_mut()) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_ALREADY_EXISTS.into())?;

        // A new file: write, seek, read back, information.
        let h = CreateFileW(
            file.ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            core::ptr::null_mut(),
            CREATE_NEW,
            0x80,
            core::ptr::null_mut(),
        );
        c.ok(h != INVALID_HANDLE_VALUE, 0)?;

        // GetFinalPathNameByHandleW: "\\?\X:\...\gala_k32\a.txt" (the
        // temporary directory itself may be given in short names, which
        // the final path does not use, so only its ends are checked);
        // VOLUME_NAME_NONE (4) without the drive; a short buffer gets the
        // size needed (with the NUL).
        let mut fin = [0u16; 300];
        let n = GetFinalPathNameByHandleW(h, fin.as_mut_ptr(), 300, 0) as usize;
        let tail: [u16; 15] = (*b"\\gala_k32\\a.txt").map(u16::from);
        c.ok(n > 20 && n < 300, n as u64)?;
        c.ok(fin[..4] == (*b"\\\\?\\").map(u16::from) && fin[5] == u16::from(b':'), fin[4].into())?;
        c.ok(fin[n - 15..n] == tail && fin[n] == 0, n as u64)?;
        c.eq(GetFinalPathNameByHandleW(h, fin.as_mut_ptr(), 4, 0).into(), (n + 1) as u64)?;
        let none = GetFinalPathNameByHandleW(h, fin.as_mut_ptr(), 300, 4) as usize;
        c.ok(
            none == n - 6 && fin[0] == u16::from(b'\\') && fin[none - 15..none] == tail,
            none as u64,
        )?;

        SetLastError(0);
        let h2 = CreateFileW(
            file.ptr(),
            GENERIC_READ,
            SHARE_ALL,
            core::ptr::null_mut(),
            CREATE_NEW,
            0x80,
            core::ptr::null_mut(),
        );
        c.ok(h2 == INVALID_HANDLE_VALUE, 0)?;
        c.eq(GetLastError().into(), ERROR_FILE_EXISTS.into())?;
        let text = b"hello, gala";
        let mut done = 0;
        c.ok(
            WriteFile(h, text.as_ptr(), 11, &mut done, core::ptr::null_mut()) != 0 && done == 11,
            done.into(),
        )?;
        let mut pos = -1i64;
        c.ok(SetFilePointerEx(h, 0, &mut pos, 0) != 0 && pos == 0, pos as u64)?;
        let mut back = [0u8; 32];
        c.ok(
            ReadFile(h, back.as_mut_ptr(), 32, &mut done, core::ptr::null_mut()) != 0 && done == 11,
            done.into(),
        )?;
        c.ok(&back[..11] == text, 0)?;
        c.ok(
            ReadFile(h, back.as_mut_ptr(), 32, &mut done, core::ptr::null_mut()) != 0 && done == 0,
            done.into(),
        )?; // EOF
        let mut info = [0u32; 13];
        c.ok(GetFileInformationByHandle(h, info.as_mut_ptr()) != 0, 0)?;
        c.eq((info[0] & FILE_ATTRIBUTE_DIRECTORY).into(), 0)?;
        c.eq(info[9].into(), 11)?; // nFileSizeLow
        c.eq(info[10].into(), 1)?; // nNumberOfLinks
        let mut size = 0i64;
        c.ok(GetFileSizeEx(h, &mut size) != 0 && size == 11, size as u64)?;
        // SetFileTime: the write time (2020-01-01), then the creation time
        // (2019-01-01); a null pointer keeps a time.
        const JAN_2020: u64 = 132_223_104_000_000_000;
        const JAN_2019: u64 = 131_907_744_000_000_000;
        let null = core::ptr::null_mut();
        let (mut created, mut written) = (0u64, 0u64);
        c.ok(GetFileTime(h, &mut created, null, &mut written) != 0, GetLastError().into())?;
        c.ok(created != 0 && written >= created, written)?;
        c.ok(SetFileTime(h, null, null, &JAN_2020) != 0, GetLastError().into())?;
        let (mut created2, mut written2) = (0u64, 0u64);
        c.ok(GetFileTime(h, &mut created2, null, &mut written2) != 0, GetLastError().into())?;
        c.ok(written2 == JAN_2020 && created2 == created, written2)?;
        c.ok(GetFileInformationByHandle(h, info.as_mut_ptr()) != 0, 0)?;
        c.eq(u64::from(info[5]) | u64::from(info[6]) << 32, JAN_2020)?; // ftLastWriteTime
        c.ok(SetFileTime(h, &JAN_2019, null, null) != 0, GetLastError().into())?;
        c.ok(GetFileTime(h, &mut created2, null, &mut written2) != 0, GetLastError().into())?;
        c.ok(created2 == JAN_2019 && written2 == JAN_2020, created2)?;
        c.ok(FlushFileBuffers(h) != 0, 0)?;
        c.ok(CloseHandle(h) != 0, 0)?;

        // Attributes by path.
        let a = GetFileAttributesW(file.ptr());
        c.ok(a != INVALID_FILE_ATTRIBUTES && a & FILE_ATTRIBUTE_DIRECTORY == 0, a.into())?;
        c.ok(GetFileAttributesW(dir.ptr()) & FILE_ATTRIBUTE_DIRECTORY != 0, 0)?;
        let mut data = [0u32; 9];
        c.ok(
            GetFileAttributesExW(file.ptr(), 0, data.as_mut_ptr()) != 0 && data[8] == 11,
            data[8].into(),
        )?;
        SetLastError(0);
        c.eq(
            GetFileAttributesW(Path::new(temp, b"gala_k32\\missing").ptr()).into(),
            INVALID_FILE_ATTRIBUTES.into(),
        )?;
        c.eq(GetLastError().into(), ERROR_FILE_NOT_FOUND.into())?;
        SetLastError(0);
        c.eq(
            GetFileAttributesW(Path::new(temp, b"gala_nodir\\missing").ptr()).into(),
            INVALID_FILE_ATTRIBUTES.into(),
        )?;
        c.eq(GetLastError().into(), ERROR_PATH_NOT_FOUND.into())?;

        // FindFirstFileW/FindNextFileW: a.txt is listed; then no more.
        let mut find = [0u16; 296]; // WIN32_FIND_DATAW
        let f = FindFirstFileW(Path::new(temp, b"gala_k32\\*").ptr(), find.as_mut_ptr());
        c.ok(f != INVALID_HANDLE_VALUE, 0)?;
        let mut seen = false;
        loop {
            let name = &find[22..]; // cFileName at byte 44
            let len = name.iter().position(|&ch| ch == 0).unwrap_or(0);
            seen |= names_eq(&name[..len], b"a.txt");
            if FindNextFileW(f, find.as_mut_ptr()) == 0 {
                break;
            }
        }
        c.ok(seen, 0)?;
        c.eq(GetLastError().into(), ERROR_NO_MORE_FILES.into())?;
        c.ok(FindClose(f) != 0, 0)?;

        // The directory as a handle: volume flags and Go's listing classes.
        let d = CreateFileW(
            dir.ptr(),
            GENERIC_READ,
            SHARE_ALL,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            core::ptr::null_mut(),
        );
        c.ok(d != INVALID_HANDLE_VALUE, 0)?;
        let (mut serial, mut max, mut flags) = (0, 0, 0);
        c.ok(
            GetVolumeInformationByHandleW(
                d,
                core::ptr::null_mut(),
                0,
                &mut serial,
                &mut max,
                &mut flags,
                core::ptr::null_mut(),
                0,
            ) != 0,
            0,
        )?;
        c.ok(max >= 255, max.into())?;
        let mut listing = [0u8; 2048];
        let mut found = false;
        for class in [FILE_FULL_DIRECTORY_RESTART_INFO, FILE_ID_BOTH_DIRECTORY_RESTART_INFO] {
            c.ok(
                GetFileInformationByHandleEx(d, class, listing.as_mut_ptr(), 2048) != 0,
                class.into(),
            )?;
            // Walk FILE_FULL_DIR_INFO / FILE_ID_BOTH_DIR_INFO entries.
            let name_at = if class == FILE_FULL_DIRECTORY_RESTART_INFO { 68 } else { 104 };
            let mut at = 0;
            loop {
                let rd =
                    |o: usize| u32::from_le_bytes(listing[at + o..at + o + 4].try_into().unwrap());
                let len = rd(60) as usize / 2;
                let name = core::slice::from_raw_parts(
                    listing.as_ptr().add(at + name_at) as *const u16,
                    len,
                );
                found |= names_eq(name, b"a.txt");
                if rd(0) == 0 {
                    break;
                }
                at += rd(0) as usize;
            }
        }
        c.ok(found, 0)?;
        SetLastError(0);
        // Continuing (not restarting) past the end.
        while GetFileInformationByHandleEx(d, FILE_FULL_DIRECTORY_INFO, listing.as_mut_ptr(), 2048)
            != 0
        {}
        c.eq(GetLastError().into(), ERROR_NO_MORE_FILES.into())?;
        c.ok(CloseHandle(d) != 0, 0)?;

        // GetFullPathNameW: ".." resolved, the file part found.
        let mut full = [0u16; 300];
        let mut part = core::ptr::null_mut();
        let n = GetFullPathNameW(
            Path::new(temp, b"gala_k32\\..\\gala_k32\\a.txt").ptr(),
            300,
            full.as_mut_ptr(),
            &mut part,
        ) as usize;
        let expect = Path::new(temp, b"gala_k32\\a.txt");
        let expect_len = expect.buf.iter().position(|&ch| ch == 0).unwrap_or(0);
        c.ok(n == expect_len && full[..n] == expect.buf[..n], n as u64)?;
        c.ok(part == full.as_mut_ptr().add(n - 5), 0)?;

        // GetShortPathNameW: a path of an existing file (its 8.3 form where
        // there is one, else as given); the size needed with the NUL when
        // the buffer is short; ERROR_FILE_NOT_FOUND for a missing file.
        let mut short = [0u16; 300];
        let a = Path::new(temp, b"gala_k32\\a.txt");
        let n = GetShortPathNameW(a.ptr(), short.as_mut_ptr(), 300) as usize;
        c.ok(n > 0 && n < 300 && short[n] == 0, n as u64)?;
        c.eq(GetShortPathNameW(a.ptr(), short.as_mut_ptr(), 1).into(), n as u64 + 1)?;
        // GetCompressedFileSizeW: the size on disk, which for an
        // uncompressed file is its size (as GetFileAttributesExW reports).
        let mut data = [0u32; 9];
        c.ok(GetFileAttributesExW(a.ptr(), 0, data.as_mut_ptr()) != 0, 0)?;
        let mut high = 7u32;
        c.eq(GetCompressedFileSizeW(a.ptr(), &mut high).into(), data[8].into())?;
        c.eq(high.into(), data[7].into())?;
        let missing = Path::new(temp, b"gala_k32\\no-such-file.txt");
        c.eq(GetShortPathNameW(missing.ptr(), short.as_mut_ptr(), 300).into(), 0)?;
        c.eq(GetLastError().into(), ERROR_FILE_NOT_FOUND.into())?;
        c.eq(GetCompressedFileSizeW(missing.ptr(), &mut high).into(), u32::MAX.into())?;
        c.eq(GetLastError().into(), ERROR_FILE_NOT_FOUND.into())?;

        // SetCurrentDirectoryW to a longer path than the one the process
        // started in, and back (a directory in use cannot be removed).
        let mut before = [0u16; 300];
        let nb = GetCurrentDirectoryW(300, before.as_mut_ptr()) as usize;
        c.ok(nb > 0 && nb < 300, nb as u64)?;
        c.ok(SetCurrentDirectoryW(dir.ptr()) != 0, GetLastError().into())?;
        let mut now = [0u16; 300];
        let nn = GetCurrentDirectoryW(300, now.as_mut_ptr()) as usize;
        let dir_len = dir.buf.iter().position(|&ch| ch == 0).unwrap_or(0);
        c.ok(nn == dir_len && now[..nn] == dir.buf[..nn], nn as u64)?;
        c.ok(SetCurrentDirectoryW(before.as_ptr()) != 0, GetLastError().into())?;

        // Deleting.
        c.ok(DeleteFileW(file.ptr()) != 0, 0)?;
        SetLastError(0);
        c.eq(DeleteFileW(file.ptr()) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_FILE_NOT_FOUND.into())?;
        c.ok(RemoveDirectoryW(dir.ptr()) != 0, 0)?;
        c.eq(GetFileAttributesW(dir.ptr()).into(), INVALID_FILE_ATTRIBUTES.into())?;

        // Gather and scatter (as SQL Server writes its pages): an
        // overlapped handle without buffering, page-sized buffers, an
        // offset in the OVERLAPPED; the count from GetOverlappedResult.
        let pages = Path::new(temp, b"gala_pages.mdf");
        let h = CreateFileW(
            pages.ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            core::ptr::null_mut(),
            CREATE_NEW,
            0x4000_0000 | 0x2000_0000, // FILE_FLAG_OVERLAPPED | FILE_FLAG_NO_BUFFERING
            core::ptr::null_mut(),
        );
        c.ok(h != INVALID_HANDLE_VALUE, GetLastError().into())?;
        let mem = VirtualAlloc(core::ptr::null_mut(), 4 * 4096, 0x3000, 4);
        c.ok(!mem.is_null(), 0)?;
        for i in 0..8192 {
            *mem.add(i) = (i / 4096) as u8 + 1; // page 0: 1s, page 1: 2s
        }
        let segments = [mem as u64, mem as u64 + 4096, 0];
        let mut ov = [0usize, 0, 8192, 0]; // Offset 8192 (OffsetHigh 0), no event
        let wrote = WriteFileGather(h, segments.as_ptr(), 8192, core::ptr::null_mut(), &mut ov);
        c.ok(wrote != 0 || GetLastError() == 997, GetLastError().into())?; // or ERROR_IO_PENDING
        let mut n = 0u32;
        c.ok(GetOverlappedResult(h, &mut ov, &mut n, 1) != 0, GetLastError().into())?;
        c.eq(n.into(), 8192)?;
        let back = [mem as u64 + 8192, mem as u64 + 12288, 0];
        let mut ov = [0usize, 0, 8192, 0];
        let read = ReadFileScatter(h, back.as_ptr(), 8192, core::ptr::null_mut(), &mut ov);
        c.ok(read != 0 || GetLastError() == 997, GetLastError().into())?;
        c.ok(GetOverlappedResult(h, &mut ov, &mut n, 1) != 0, GetLastError().into())?;
        c.eq(n.into(), 8192)?;
        c.ok(*mem.add(8192) == 1 && *mem.add(12288) == 2 && *mem.add(16383) == 2, 0)?;
        CloseHandle(h);
        DeleteFileW(pages.ptr());

        // WriteFileEx / ReadFileEx: the routine runs in an alertable wait.
        use core::sync::atomic::Ordering;
        let exfile = Path::new(temp, b"gala_ex.bin");
        let h = CreateFileW(
            exfile.ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            core::ptr::null_mut(),
            CREATE_NEW,
            0x4000_0000, // FILE_FLAG_OVERLAPPED
            core::ptr::null_mut(),
        );
        c.ok(h != INVALID_HANDLE_VALUE, GetLastError().into())?;
        let mut ov = [0usize, 0, 16, 0]; // at offset 16
        c.ok(
            WriteFileEx(h, b"completion".as_ptr(), 10, ov.as_mut_ptr(), completed) != 0,
            GetLastError().into(),
        )?;
        c.eq(DONE[0].load(Ordering::SeqCst) as u64, 0)?; // not yet
        c.eq(SleepEx(1000, 1).into(), 0xc0)?; // WAIT_IO_COMPLETION
        c.eq(DONE[0].load(Ordering::SeqCst) as u64, 1)?;
        c.eq(DONE[1].load(Ordering::SeqCst) as u64, 0)?;
        c.eq(DONE[2].load(Ordering::SeqCst) as u64, 10)?;
        c.eq(DONE[3].load(Ordering::SeqCst) as u64, ov.as_ptr() as u64)?;
        let mut back = [0u8; 10];
        let mut ov = [0usize, 0, 16, 0];
        c.ok(
            ReadFileEx(h, back.as_mut_ptr(), 10, ov.as_mut_ptr(), completed) != 0,
            GetLastError().into(),
        )?;
        c.eq(SleepEx(1000, 1).into(), 0xc0)?;
        c.ok(DONE[0].load(Ordering::SeqCst) == 2 && DONE[2].load(Ordering::SeqCst) == 10, 0)?;
        c.ok(&back == b"completion", back[0].into())?;
        CloseHandle(h);
        DeleteFileW(exfile.ptr());

        // DOS device names: C: is the volume's device, NUL the null
        // device; the target is followed by two NULs (counted).
        let mut target = [0u16; 64];
        let c_drive = b"C:\0".map(u16::from);
        let n = QueryDosDeviceW(c_drive.as_ptr(), target.as_mut_ptr(), 64) as usize;
        let volume = b"\\Device\\HarddiskVolume1\0\0".map(u16::from);
        c.ok(n == volume.len() && target[..n] == volume, n as u64)?;
        let null = b"NUL\0".map(u16::from);
        let n = QueryDosDeviceW(null.as_ptr(), target.as_mut_ptr(), 64) as usize;
        let device = b"\\Device\\Null\0\0".map(u16::from);
        c.ok(n == device.len() && target[..n] == device, n as u64)?;
        c.eq(QueryDosDeviceW(c_drive.as_ptr(), target.as_mut_ptr(), 8).into(), 0)?;
        c.eq(GetLastError().into(), 122)?; // ERROR_INSUFFICIENT_BUFFER
        // The volume's GUID path: \\?\Volume{GUID}\ (49 characters).
        let root = b"C:\\\0".map(u16::from);
        let mut name = [0u16; 64];
        c.ok(GetVolumeNameForVolumeMountPointW(root.as_ptr(), name.as_mut_ptr(), 64) != 0, 0)?;
        let prefix = b"\\\\?\\Volume{".map(u16::from);
        let len = name.iter().position(|&ch| ch == 0).unwrap_or(0);
        c.ok(len == 49 && name[..11] == prefix && name[47..49] == [0x7d, 0x5c], len as u64)?;
        let none = b"Q:\0".map(u16::from);
        c.eq(QueryDosDeviceW(none.as_ptr(), target.as_mut_ptr(), 64).into(), 0)?;
        c.eq(GetLastError().into(), ERROR_FILE_NOT_FOUND.into())?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"kernel32_file_test: ok\r\n");
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
