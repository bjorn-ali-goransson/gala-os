//! The page file (ROADMAP OF, the memory; ARCHITECTURE "The page file"),
//! with the results real Windows gives on a machine with a system-managed
//! page file on C::
//!
//! - GlobalMemoryStatusEx's ullTotalPageFile is the commit limit: physical
//!   memory plus the page file, so more than ullTotalPhys, and
//!   NtQuerySystemInformation(SystemPerformanceInformation)'s CommitLimit
//!   says the same;
//! - NtQuerySystemInformation(SystemPageFileInformation) lists
//!   `\??\C:\pagefile.sys` with its size in pages (a buffer too small:
//!   STATUS_INFO_LENGTH_MISMATCH with the length needed);
//! - `C:\pagefile.sys` is a hidden system file of that size, listed by
//!   FindFirstFileW, which no program can open for its data
//!   (ERROR_SHARING_VIOLATION: the system holds it);
//! - more private memory than the machine has can be committed, written
//!   page by page and read back intact (twice, in both directions, so
//!   pages that went to the page file come back), and charged as
//!   PrivateUsage; freeing it gives the charge back;
//! - a commit past the commit limit grows a system-managed page file:
//!   the commit succeeds and the limit is higher after it (a fixed-size
//!   page file refuses it: an `info` line).
//!
//! The page file is gala's `C:\pagefile.sys`; on Windows the one
//! SystemPageFileInformation names (CI's is on D:).
//!
//! The argument is the private memory to commit, in MiB (default: one and
//! a quarter times physical memory, 1 GiB on a machine of more). Prints `pagefile_test: ok` or exits
//! with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

const MEM_COMMIT: u32 = 0x1000;
const MEM_RESERVE: u32 = 0x2000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_READWRITE: u32 = 0x04;
const GENERIC_READ: u32 = 0x8000_0000;
const OPEN_EXISTING: u32 = 3;
const FILE_SHARE_ALL: u32 = 7;
const ERROR_SHARING_VIOLATION: u32 = 32;
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x02;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x04;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xc000_0004_u32 as i32;
const INVALID_HANDLE: Handle = -1isize as Handle;
const PAGE: usize = 4096;
const MIB: u64 = 1 << 20;

#[repr(C)]
#[derive(Default)]
struct MemoryStatusEx {
    length: u32,
    load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page_file: u64,
    avail_page_file: u64,
    total_virtual: u64,
    avail_virtual: u64,
    avail_extended_virtual: u64,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn VirtualAlloc(at: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(at: *mut c_void, size: usize, kind: u32) -> Bool;
    fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> Bool;
    fn K32GetProcessMemoryInfo(process: Handle, counters: *mut u64, size: u32) -> Bool;
    fn GetCurrentProcess() -> Handle;
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn GetLastError() -> u32;
    fn SetLastError(code: u32);
    fn GetFileAttributesW(name: *const u16) -> u32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CloseHandle(h: Handle) -> Bool;
    fn FindFirstFileW(name: *const u16, data: *mut u8) -> Handle;
    fn FindClose(h: Handle) -> Bool;
    fn GetCommandLineW() -> *const u16;
    fn ExitProcess(code: u32) -> !;
}

// What the compiler's own code calls (no CRT here).
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, buf: *mut u8, len: u32, ret: *mut u32) -> i32;
}

fn out(s: &[u8]) {
    let mut written = 0;
    // SAFETY: writes `s` to the standard output.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            s.as_ptr(),
            s.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        );
    }
}

fn hex(mut v: u64) {
    let mut buf = [0u8; 18];
    buf[0] = b'0';
    buf[1] = b'x';
    for i in (2..18).rev() {
        buf[i] = b"0123456789abcdef"[(v & 15) as usize];
        v >>= 4;
    }
    out(&buf);
}

fn dec(mut v: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    out(&buf[i..]);
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"pagefile_test: check ");
        hex(self.0.into());
        out(b" failed, got ");
        hex(got);
        out(b" (line ");
        dec(core::panic::Location::caller().line().into());
        out(b")\r\n");
        Err(self.0)
    }
}

fn status() -> MemoryStatusEx {
    let mut s = MemoryStatusEx { length: 64, ..Default::default() };
    // SAFETY: a MEMORYSTATUSEX.
    unsafe { GlobalMemoryStatusEx(&mut s) };
    s
}

/// PROCESS_MEMORY_COUNTERS_EX's PrivateUsage.
fn private_usage() -> u64 {
    let mut counters = [0u64; 10];
    // SAFETY: a PROCESS_MEMORY_COUNTERS_EX (80 bytes).
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), counters.as_mut_ptr(), 80) };
    counters[9]
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// SystemPerformanceInformation's CommitLimit, in pages.
fn commit_limit_pages() -> u64 {
    let mut perf = [0u8; 0x158];
    let mut ret = 0;
    // SAFETY: a buffer of the class's size.
    unsafe { NtQuerySystemInformation(2, perf.as_mut_ptr(), 0x158, &mut ret) };
    u32_at(&perf, 0x34).into()
}

/// The argument (MiB), if one was given.
fn argument() -> Option<u64> {
    // SAFETY: the process's command line, NUL-terminated.
    let line = unsafe { GetCommandLineW() };
    let mut i = 0;
    let mut last_space = None;
    // SAFETY: read up to the terminator.
    while unsafe { *line.add(i) } != 0 {
        if unsafe { *line.add(i) } == u16::from(b' ') {
            last_space = Some(i);
        }
        i += 1;
    }
    let mut v = 0u64;
    let mut digits = 0;
    for j in last_space? + 1..i {
        // SAFETY: inside the line.
        let c = unsafe { *line.add(j) };
        if !(u16::from(b'0')..=u16::from(b'9')).contains(&c) {
            return None;
        }
        v = v * 10 + u64::from(c - u16::from(b'0'));
        digits += 1;
    }
    (digits > 0).then_some(v)
}

/// What page `i` holds at offset `o` (a qword).
fn pattern(i: usize, o: usize) -> u64 {
    (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ (o as u64) ^ 0x6761_6c61
}

const OFFSETS: [usize; 3] = [0, 2048, PAGE - 8];

/// Checks page `i` of `base`: its pattern, and on every 64th page that
/// the rest is zero.
fn page_intact(base: *mut u8, i: usize) -> bool {
    // SAFETY: page `i` of the committed region.
    let page = unsafe { core::slice::from_raw_parts(base.add(i * PAGE), PAGE) };
    for o in OFFSETS {
        let mut q = [0u8; 8];
        q.copy_from_slice(&page[o..o + 8]);
        if u64::from_le_bytes(q) != pattern(i, o) {
            return false;
        }
    }
    if i.is_multiple_of(64) {
        for (o, &b) in page.iter().enumerate() {
            let ours = OFFSETS.iter().any(|&p| (p..p + 8).contains(&o));
            if !ours && b != 0 {
                return false;
            }
        }
    }
    true
}

fn wide(s: &[u8], buf: &mut [u16]) -> *const u16 {
    for (i, &c) in s.iter().enumerate() {
        buf[i] = c.into();
    }
    buf[s.len()] = 0;
    buf.as_ptr()
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // The commit limit is memory plus the page file.
    let s = status();
    c.ok(s.total_page_file > s.total_phys, s.total_page_file)?;
    c.ok(commit_limit_pages() * PAGE as u64 == s.total_page_file, commit_limit_pages())?;
    out(b"pagefile_test: memory ");
    dec(s.total_phys / MIB);
    out(b" MiB, commit limit ");
    dec(s.total_page_file / MIB);
    out(b" MiB\r\n");

    // SystemPageFileInformation: \??\C:\pagefile.sys and its size.
    let mut info = [0u8; 256];
    let mut need = 0u32;
    // SAFETY: a buffer of the length given.
    let st = unsafe { NtQuerySystemInformation(18, info.as_mut_ptr(), 8, &mut need) };
    c.ok(st == STATUS_INFO_LENGTH_MISMATCH, st as u32 as u64)?;
    c.ok(need as usize >= 32 + 38 && (need as usize) <= info.len(), need.into())?;
    // SAFETY: as above.
    let st = unsafe { NtQuerySystemInformation(18, info.as_mut_ptr(), need, &mut need) };
    c.ok(st == 0, st as u32 as u64)?;
    c.ok(u32_at(&info, 0) == 0, u32_at(&info, 0).into())?;
    let total = u64::from(u32_at(&info, 4));
    c.ok(total > 0 && total * PAGE as u64 <= s.total_page_file, total)?;
    let name_len = u16::from_le_bytes([info[16], info[17]]) as usize;
    c.ok(name_len == 38, name_len as u64)?;
    let mut ptr = [0u8; 8];
    ptr.copy_from_slice(&info[24..32]);
    let name = u64::from_le_bytes(ptr) as *const u16;
    // `\??\X:\pagefile.sys` (gala's on C:; a Windows machine may keep it
    // on another volume, as CI's on D:).
    let expected = "\\??\\C:\\pagefile.sys";
    // SAFETY: the name the call returned, in `info`.
    let same =
        expected.bytes().enumerate().all(|(i, b)| i == 4 || unsafe { *name.add(i) } == b.into());
    c.ok(same, 0)?;
    // SAFETY: as above.
    let drive = unsafe { *name.add(4) } as u8;
    c.ok(drive.is_ascii_uppercase(), drive.into())?;

    // X:\pagefile.sys: hidden, system, of that size, and held.
    let mut text = *b"C:\\pagefile.sys";
    text[0] = drive;
    let mut buf = [0u16; 64];
    let path = wide(&text, &mut buf);
    // SAFETY: a NUL-terminated name.
    let attributes = unsafe { GetFileAttributesW(path) };
    let hs = FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM;
    c.ok(attributes != u32::MAX && attributes & hs == hs, attributes.into())?;
    // SAFETY: plain calls; a handle that opens is closed.
    unsafe {
        SetLastError(0);
        let h = CreateFileW(
            path,
            GENERIC_READ,
            FILE_SHARE_ALL,
            core::ptr::null(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        );
        let error = GetLastError();
        if h != INVALID_HANDLE {
            CloseHandle(h);
        }
        c.ok(h == INVALID_HANDLE && error == ERROR_SHARING_VIOLATION, error.into())?;
    }
    let mut find = [0u8; 592];
    // SAFETY: a WIN32_FIND_DATAW buffer.
    let h = unsafe { FindFirstFileW(path, find.as_mut_ptr()) };
    c.ok(h != INVALID_HANDLE, 0)?;
    // SAFETY: the search handle.
    unsafe { FindClose(h) };
    let size = u64::from(u32_at(&find, 28)) << 32 | u64::from(u32_at(&find, 32));
    c.ok(size == total * PAGE as u64, size)?;

    // More private memory than the machine has: committed, written, read
    // back (twice, both ways), charged.
    // (A machine of more than 1 GiB commits 1 GiB unless told: CI's
    // Windows has 16.)
    let memory = s.total_phys / MIB;
    let mib = argument().unwrap_or(if memory <= 1024 { memory + memory / 4 } else { 1024 });
    let bytes = (mib * MIB) as usize;
    let pages = bytes / PAGE;
    let before = private_usage();
    // SAFETY: a request for memory.
    let base = unsafe {
        VirtualAlloc(core::ptr::null_mut(), bytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE)
    } as *mut u8;
    c.ok(!base.is_null(), unsafe { GetLastError() }.into())?;
    c.ok(private_usage() >= before + bytes as u64, private_usage())?;
    out(b"pagefile_test: committed ");
    dec(mib);
    out(b" MiB\r\n");
    for i in 0..pages {
        for o in OFFSETS {
            // SAFETY: inside the committed region.
            unsafe { base.add(i * PAGE + o).cast::<u64>().write_unaligned(pattern(i, o)) };
        }
    }
    out(b"pagefile_test: written\r\n");
    for i in 0..pages {
        c.ok(page_intact(base, i), i as u64)?;
    }
    for i in (0..pages).rev() {
        c.ok(page_intact(base, i), i as u64)?;
    }
    out(b"pagefile_test: read back twice\r\n");
    // SAFETY: the region allocated above.
    c.ok(unsafe { VirtualFree(base.cast(), 0, MEM_RELEASE) } != 0, 0)?;
    c.ok(private_usage() < before + MIB, private_usage())?;

    // Past the commit limit: the page file grows.
    let s = status();
    let used = s.total_page_file - s.avail_page_file;
    let past = (s.total_page_file - used + 64 * MIB) as usize;
    // SAFETY: a request for memory, given back.
    let p = unsafe {
        VirtualAlloc(core::ptr::null_mut(), past, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE)
    };
    if p.is_null() {
        // A page file of a fixed size (Windows' setting) does not grow.
        out(b"pagefile_test: info: a commit past the limit was refused (error ");
        dec(unsafe { GetLastError() }.into());
        out(b"): the page file is not system-managed\r\n");
        return Ok(());
    }
    let grown = status();
    c.ok(grown.total_page_file > s.total_page_file, grown.total_page_file)?;
    // SAFETY: the region allocated above.
    c.ok(unsafe { VirtualFree(p, 0, MEM_RELEASE) } != 0, 0)?;
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"pagefile_test: ok\r\n");
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
    unsafe { ExitProcess(0xdead) }
}
