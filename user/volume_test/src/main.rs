//! The volume a program's files live on, as Windows reports it and as it
//! behaves when large files are written (documented contracts:
//! GetDiskFreeSpaceExW, GetDiskFreeSpaceW, GetVolumeInformationW,
//! WriteFile's ERROR_DISK_FULL; Wine's kernel32 drive.c tests, which run on
//! Windows). On gala the scenario boots with a disk-backed C: (`persist` on
//! a fresh 1 GiB disk) and passes a file size larger than the machine's
//! memory, so the file can only be written and read back if its contents
//! live on the disk.
//!
//! - GetDiskFreeSpaceExW of `C:\`: total > 0, free <= total, available to
//!   the caller <= free (no quotas), whole sectors; of NULL (the current
//!   directory's volume) and `C:\Windows\` (any directory, not only a root:
//!   the same totals as `C:\`) succeeds; of a missing directory fails with
//!   ERROR_FILE_NOT_FOUND or ERROR_PATH_NOT_FOUND (which one: unverified).
//! - GetDiskFreeSpaceW: of "" fails with ERROR_PATH_NOT_FOUND; of NULL,
//!   `\`, `/` and `C:\` succeeds (Wine's test_GetDiskFreeSpaceW); 512- or
//!   4096-byte sectors, a power-of-two cluster; its total covers at least
//!   GetDiskFreeSpaceExW's (Wine's test_GetDiskFreeSpaceA); its free
//!   clusters are GetDiskFreeSpaceExW's free bytes (within 64 MiB: other
//!   programs write meanwhile on Windows).
//! - GetVolumeInformationW of `C:\`: NTFS, 255-character names, case
//!   preserved and Unicode names on disk.
//! - A file of `<MiB>` (the first argument; 256 by default) written in
//!   1 MiB pieces in the temporary directory: free space (of that
//!   directory, given as a non-root path) drops by at least its size less
//!   16 MiB once it is written and flushed; every 512-byte block reads back
//!   (its offset at both ends); its first MiB overwritten reads back; once
//!   deleted (after the next part) the space is free again (within 16 MiB).
//! - With the large file still there, when that volume has less than
//!   2 GiB free (gala's test disk; a
//!   Windows machine's C: is skipped and says so): writing past the free
//!   space fails with ERROR_DISK_FULL, not before the free space (less
//!   64 MiB) is written; with the volume full, overwriting the file's
//!   first MiB (already allocated) still succeeds; deleting it frees the
//!   space again.
//!
//! Prints `volume_test: ok` (and `volume_test: disk full part skipped`
//! where it does not apply) or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
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
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn SetFilePointerEx(h: Handle, distance: i64, new: *mut i64, method: u32) -> Bool;
    fn GetFileSizeEx(h: Handle, size: *mut i64) -> Bool;
    fn FlushFileBuffers(h: Handle) -> Bool;
    fn DeleteFileW(name: *const u16) -> Bool;
    fn GetTempPathW(size: u32, buf: *mut u16) -> u32;
    fn GetDiskFreeSpaceExW(
        directory: *const u16,
        available: *mut u64,
        total: *mut u64,
        free: *mut u64,
    ) -> Bool;
    fn GetDiskFreeSpaceW(
        root: *const u16,
        sectors_per_cluster: *mut u32,
        bytes_per_sector: *mut u32,
        free_clusters: *mut u32,
        total_clusters: *mut u32,
    ) -> Bool;
    fn GetVolumeInformationW(
        root: *const u16,
        name: *mut u16,
        name_size: u32,
        serial: *mut u32,
        max_component: *mut u32,
        flags: *mut u32,
        fs_name: *mut u16,
        fs_name_size: u32,
    ) -> Bool;
    fn GetCommandLineW() -> *const u16;
    fn GetLastError() -> u32;
    fn GetStdHandle(which: u32) -> Handle;
    fn ExitProcess(code: u32) -> !;
}

// The compiler's memcpy/memset/memcmp, from ntdll.
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

// What the compiler's output expects from a C runtime.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_ALWAYS: u32 = 2;
const FILE_BEGIN: u32 = 0;

const ERROR_FILE_NOT_FOUND: u64 = 2;
const ERROR_PATH_NOT_FOUND: u64 = 3;
const ERROR_DISK_FULL: u64 = 112;
const FILE_CASE_PRESERVED_NAMES: u32 = 2;
const FILE_UNICODE_ON_DISK: u32 = 4;

const MIB: u64 = 1 << 20;
/// What other activity may change free space by between two queries.
const SLACK: u64 = 16 * MIB;
/// Below this much free space the disk-full part runs.
const FULL_PART_BELOW: u64 = 2 << 30;

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

fn hex(v: u64) -> [u8; 16] {
    let mut s = [0u8; 16];
    for (i, c) in s.iter_mut().enumerate() {
        let d = (v >> (60 - 4 * i)) & 0xf;
        *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
    }
    s
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"volume_test: check ");
        out(&hex(self.0.into())[8..]);
        out(b" got ");
        out(&hex(got));
        check_line(core::panic::Location::caller().line());
        out(b"\r\n");
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

/// A NUL-terminated UTF-16 copy of an ASCII string.
fn wide<const N: usize>(s: &str) -> [u16; N] {
    let mut w = [0u16; N];
    for (d, b) in w.iter_mut().zip(s.bytes()) {
        *d = u16::from(b);
    }
    w
}

fn last_error() -> u64 {
    // SAFETY: no arguments.
    unsafe { GetLastError() }.into()
}

/// GetDiskFreeSpaceExW of `dir`: (available, total, free), or the error.
fn free_space(dir: *const u16) -> Result<(u64, u64, u64), u64> {
    let (mut available, mut total, mut free) = (0, 0, 0);
    // SAFETY: valid output pointers; `dir` is NUL-terminated or null.
    let ok = unsafe { GetDiskFreeSpaceExW(dir, &mut available, &mut total, &mut free) };
    if ok == 0 { Err(last_error()) } else { Ok((available, total, free)) }
}

/// GetDiskFreeSpaceW of `root`: (sectors per cluster, bytes per sector,
/// free clusters, total clusters), or the error.
fn clusters(root: *const u16) -> Result<(u32, u32, u32, u32), u64> {
    let (mut spc, mut bps, mut free, mut total) = (0, 0, 0, 0);
    // SAFETY: valid output pointers; `root` is NUL-terminated or null.
    let ok = unsafe { GetDiskFreeSpaceW(root, &mut spc, &mut bps, &mut free, &mut total) };
    if ok == 0 { Err(last_error()) } else { Ok((spc, bps, free, total)) }
}

fn api_forms(c: &mut Checks) -> Result<(), u32> {
    let root: [u16; 4] = wide("C:\\");
    let windows: [u16; 12] = wide("C:\\Windows\\");
    let missing: [u16; 32] = wide("C:\\volume_test no such dir\\");
    let r = free_space(root.as_ptr());
    c.ok(r.is_ok(), r.err().unwrap_or(0))?;
    let (available, total, free) = r.unwrap_or_default();
    c.ok(total > 0, total)?;
    c.ok(free <= total, free)?;
    c.ok(available <= free, available)?;
    c.eq(total % 512, 0)?;
    let r = free_space(core::ptr::null());
    c.ok(r.is_ok(), r.err().unwrap_or(0))?;
    let r = free_space(windows.as_ptr());
    c.ok(r.is_ok(), r.err().unwrap_or(0))?;
    c.eq(r.map_or(0, |(_, t, _)| t), total)?;
    let r = free_space(missing.as_ptr());
    let error = r.err().unwrap_or(0);
    c.ok(error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND, error)?;

    // GetDiskFreeSpaceW (Wine's drive.c).
    let empty = [0u16; 1];
    c.eq(clusters(empty.as_ptr()).err().unwrap_or(0), ERROR_PATH_NOT_FOUND)?;
    for name in [wide::<2>("\\"), wide::<2>("/")] {
        let r = clusters(name.as_ptr());
        c.ok(r.is_ok(), r.err().unwrap_or(0))?;
    }
    let r = clusters(core::ptr::null());
    c.ok(r.is_ok(), r.err().unwrap_or(0))?;
    let r = clusters(root.as_ptr());
    c.ok(r.is_ok(), r.err().unwrap_or(0))?;
    let (spc, bps, free_clusters, total_clusters) = r.unwrap_or_default();
    c.ok(bps == 512 || bps == 4096, bps.into())?;
    c.ok(spc.is_power_of_two(), spc.into())?;
    let cluster = u64::from(spc) * u64::from(bps);
    c.ok(cluster * u64::from(total_clusters) >= total, total_clusters.into())?;
    let free_now = free_space(root.as_ptr()).map_or(0, |(_, _, f)| f);
    c.ok(
        (cluster * u64::from(free_clusters)).abs_diff(free_now) <= 64 * MIB,
        free_clusters.into(),
    )?;

    let (mut serial, mut max_component, mut flags) = (0, 0, 0);
    let mut fs_name = [0u16; 32];
    // SAFETY: valid buffers and output pointers.
    let ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            core::ptr::null_mut(),
            0,
            &mut serial,
            &mut max_component,
            &mut flags,
            fs_name.as_mut_ptr(),
            32,
        )
    };
    c.ok(ok != 0, last_error())?;
    c.ok(fs_name[..5] == wide::<5>("NTFS"), fs_name[0].into())?;
    c.eq(max_component.into(), 255)?;
    let want = FILE_CASE_PRESERVED_NAMES | FILE_UNICODE_ON_DISK;
    c.eq((flags & want).into(), want.into())
}

static mut BUF: [u8; MIB as usize] = [0; MIB as usize];

fn buf() -> &'static mut [u8] {
    // SAFETY: the program's only buffer, used by one thread.
    unsafe { &mut *core::ptr::addr_of_mut!(BUF) }
}

/// Puts the pattern of the MiB at file offset `base` into BUF: each
/// 512-byte block starts with its offset xor `key` and ends with its
/// offset's complement.
fn fill(base: u64, key: u64) {
    for (j, block) in buf().chunks_mut(512).enumerate() {
        let at = base + j as u64 * 512;
        block[..8].copy_from_slice(&(at ^ key).to_le_bytes());
        block[504..].copy_from_slice(&(!at).to_le_bytes());
    }
}

/// Whether BUF holds `fill(base, key)`'s pattern.
fn matches(base: u64, key: u64) -> bool {
    buf().chunks(512).enumerate().all(|(j, block)| {
        let at = base + j as u64 * 512;
        block[..8] == (at ^ key).to_le_bytes() && block[504..] == (!at).to_le_bytes()
    })
}

fn write_mib(h: Handle) -> (bool, u32) {
    let mut n = 0;
    let b = buf();
    // SAFETY: valid buffer and count.
    let ok = unsafe { WriteFile(h, b.as_ptr(), b.len() as u32, &mut n, core::ptr::null_mut()) };
    (ok != 0, n)
}

fn read_mib(h: Handle) -> (bool, u32) {
    let mut n = 0;
    let b = buf();
    // SAFETY: valid buffer and count.
    let ok = unsafe { ReadFile(h, b.as_mut_ptr(), b.len() as u32, &mut n, core::ptr::null_mut()) };
    (ok != 0, n)
}

fn seek(h: Handle, to: i64) -> bool {
    let mut at = -1;
    // SAFETY: valid output pointer.
    unsafe { SetFilePointerEx(h, to, &mut at, FILE_BEGIN) != 0 && at == to }
}

fn create(name: &[u16]) -> Handle {
    // SAFETY: a NUL-terminated name.
    unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            core::ptr::null(),
            CREATE_ALWAYS,
            0,
            core::ptr::null_mut(),
        )
    }
}

/// `dir` (NUL-terminated, ending in a backslash) followed by `name`.
fn join(dir: &[u16], name: &str) -> [u16; 300] {
    let mut path = [0u16; 300];
    let n = dir.iter().position(|&c| c == 0).unwrap_or(0).min(260);
    path[..n].copy_from_slice(&dir[..n]);
    for (d, b) in path[n..299].iter_mut().zip(name.bytes()) {
        *d = u16::from(b);
    }
    path
}

/// The large file (see the module's doc); whether the disk-full part ran
/// (it runs while the file is there, so it fills only the rest).
fn large_file(c: &mut Checks, dir: &[u16], mib: u64) -> Result<bool, u32> {
    let path = join(dir, "volume_test.bin");
    let free0 = free_space(dir.as_ptr()).map_or(0, |(_, _, f)| f);
    let h = create(&path);
    c.ok(h as isize != -1, last_error())?;
    for i in 0..mib {
        fill(i * MIB, 0x5a5a_0000_0000_0000);
        let (ok, n) = write_mib(h);
        c.ok(ok && u64::from(n) == MIB, if ok { i } else { last_error() })?;
    }
    // SAFETY: a valid handle.
    c.ok(unsafe { FlushFileBuffers(h) } != 0, last_error())?;
    let mut size = 0;
    // SAFETY: a valid handle and output pointer.
    c.ok(unsafe { GetFileSizeEx(h, &mut size) } != 0, last_error())?;
    c.eq(size as u64, mib * MIB)?;
    let free1 = free_space(dir.as_ptr()).map_or(0, |(_, _, f)| f);
    c.ok(free0.saturating_sub(free1) + SLACK >= mib * MIB, free0.saturating_sub(free1))?;

    // Every block reads back.
    c.ok(seek(h, 0), last_error())?;
    for i in 0..mib {
        let (ok, n) = read_mib(h);
        c.ok(ok && u64::from(n) == MIB, if ok { i } else { last_error() })?;
        c.ok(matches(i * MIB, 0x5a5a_0000_0000_0000), i)?;
    }
    // The first MiB overwritten reads back.
    c.ok(seek(h, 0), last_error())?;
    fill(0, 0x1234);
    let (ok, n) = write_mib(h);
    c.ok(ok && u64::from(n) == MIB, last_error())?;
    buf().fill(0);
    c.ok(seek(h, 0), last_error())?;
    let (ok, _) = read_mib(h);
    c.ok(ok && matches(0, 0x1234), last_error())?;
    // SAFETY: a valid handle.
    unsafe { CloseHandle(h) };
    // With it still there, the rest of the volume is filled.
    let full = disk_full(c, dir)?;
    // SAFETY: a NUL-terminated name.
    c.ok(unsafe { DeleteFileW(path.as_ptr()) } != 0, last_error())?;
    let free2 = free_space(dir.as_ptr()).map_or(0, |(_, _, f)| f);
    c.ok(free2.saturating_sub(free1) + SLACK >= mib * MIB, free2.saturating_sub(free1))?;
    Ok(full)
}

/// Fills the volume (see the module's doc); false when skipped.
fn disk_full(c: &mut Checks, dir: &[u16]) -> Result<bool, u32> {
    let free0 = free_space(dir.as_ptr()).map_or(0, |(_, _, f)| f);
    if free0 >= FULL_PART_BELOW {
        return Ok(false);
    }
    let path = join(dir, "volume_full.bin");
    let h = create(&path);
    c.ok(h as isize != -1, last_error())?;
    let mut written = 0u64;
    let error = loop {
        fill(written, 0);
        let (ok, n) = write_mib(h);
        if !ok {
            break last_error();
        }
        written += u64::from(n);
        if written > free0 + 64 * MIB {
            break 0;
        }
    };
    c.eq(error, ERROR_DISK_FULL)?;
    c.ok(written + 64 * MIB >= free0, written)?;
    // Full, but rewriting what is allocated needs no more room.
    c.ok(seek(h, 0), last_error())?;
    fill(0, 0x77);
    let (ok, n) = write_mib(h);
    c.ok(ok && u64::from(n) == MIB, last_error())?;
    // SAFETY: a valid handle; a NUL-terminated name.
    unsafe {
        CloseHandle(h);
        c.ok(DeleteFileW(path.as_ptr()) != 0, last_error())?;
    }
    let free1 = free_space(dir.as_ptr()).map_or(0, |(_, _, f)| f);
    c.ok(free1 + SLACK >= free0, free1)?;
    Ok(true)
}

/// The first argument as a number of MiB (256 without one).
fn size_argument() -> u64 {
    // SAFETY: the process's command line, NUL-terminated.
    let line = unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    };
    // Past the program's name (quoted or not).
    let mut rest = line;
    if rest.first() == Some(&u16::from(b'"')) {
        let end =
            rest[1..].iter().position(|&c| c == u16::from(b'"')).map_or(rest.len(), |e| e + 2);
        rest = &rest[end.min(rest.len())..];
    } else {
        let end = rest.iter().position(|&c| c == u16::from(b' ')).unwrap_or(rest.len());
        rest = &rest[end..];
    }
    let digits = rest.iter().skip_while(|&&c| c == u16::from(b' '));
    let mut v = 0u64;
    let mut any = false;
    for &c in digits.take_while(|&&c| (u16::from(b'0')..=u16::from(b'9')).contains(&c)) {
        v = v * 10 + u64::from(c - u16::from(b'0'));
        any = true;
    }
    if any && v > 0 { v } else { 256 }
}

fn run() -> Result<bool, u32> {
    let mut c = Checks(0);
    api_forms(&mut c)?;
    let mut dir = [0u16; 261];
    // SAFETY: a buffer of 261 characters.
    let n = unsafe { GetTempPathW(261, dir.as_mut_ptr()) };
    c.ok(n > 0 && n < 261, last_error())?;
    large_file(&mut c, &dir, size_argument())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(full) => {
            if !full {
                out(b"volume_test: disk full part skipped (2 GiB or more free)\r\n");
            }
            out(b"volume_test: ok\r\n");
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

/// ` (line L)`: the source line of the failing check, from
/// `#[track_caller]` on Checks' methods (so a report names the line
/// without anyone counting checks).
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
