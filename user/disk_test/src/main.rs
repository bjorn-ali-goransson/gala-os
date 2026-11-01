//! Raw disk access through `\\.\PhysicalDrive0`, as Windows gives it to an
//! administrator (documented contracts: CreateFile's "Physical Disks and
//! Volumes", the disk IOCTLs in winioctl.h). Runs as SYSTEM. xtask attaches a fresh 16 MiB
//! disk whose sector 1 starts with `gala disk_test sector 1`, and checks
//! afterwards that sector 2 holds what this program wrote.
//!
//! - CreateFileW opens the disk (OPEN_EXISTING, shared read/write);
//!   GetFileType is FILE_TYPE_DISK; `\\.\PhysicalDrive9` does not exist
//!   (ERROR_FILE_NOT_FOUND); a standard user (`galatest`, impersonated
//!   after LogonUserW) may not open it (ERROR_ACCESS_DENIED).
//! - IOCTL_DISK_GET_LENGTH_INFO: 16 MiB. IOCTL_DISK_GET_DRIVE_GEOMETRY:
//!   fixed media, 512-byte sectors, 255 heads and 63 sectors per track
//!   (the geometry Windows reports for disks), a too-small buffer fails
//!   with ERROR_INSUFFICIENT_BUFFER. IOCTL_DISK_GET_DRIVE_GEOMETRY_EX:
//!   the same geometry and the disk size. An IOCTL the disk does not know
//!   fails with ERROR_INVALID_FUNCTION.
//! - Reads and writes move the file pointer (SetFilePointerEx): sector 1
//!   holds xtask's text; a write of sector 2 reads back; a read of 64 KiB
//!   spans sectors; transfers that are not whole sectors, or at an offset
//!   that is not, fail with ERROR_INVALID_PARAMETER; OVERLAPPED offsets
//!   work on a synchronous handle; FlushFileBuffers succeeds.
//!
//! Prints `disk_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    event: Handle,
}

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
    fn GetFileType(h: Handle) -> u32;
    fn DeviceIoControl(
        h: Handle,
        code: u32,
        input: *const c_void,
        input_size: u32,
        output: *mut c_void,
        output_size: u32,
        returned: *mut u32,
        overlapped: *mut Overlapped,
    ) -> Bool;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut Overlapped) -> Bool;
    fn WriteFile(
        h: Handle,
        buf: *const u8,
        len: u32,
        written: *mut u32,
        ov: *mut Overlapped,
    ) -> Bool;
    fn SetFilePointerEx(h: Handle, distance: i64, new: *mut i64, method: u32) -> Bool;
    fn FlushFileBuffers(h: Handle) -> Bool;
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

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LogonUserW(
        user: *const u16,
        domain: *const u16,
        password: *const u16,
        kind: u32,
        provider: u32,
        token: *mut Handle,
    ) -> Bool;
    fn ImpersonateLoggedOnUser(token: Handle) -> Bool;
    fn RevertToSelf() -> Bool;
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const FILE_TYPE_DISK: u32 = 1;
const FILE_BEGIN: u32 = 0;
const FILE_CURRENT: u32 = 1;

const IOCTL_DISK_GET_DRIVE_GEOMETRY: u32 = 0x0007_0000;
const IOCTL_DISK_GET_DRIVE_GEOMETRY_EX: u32 = 0x0007_00a0;
const IOCTL_DISK_GET_LENGTH_INFO: u32 = 0x0007_405c;
const FIXED_MEDIA: u32 = 12;

const ERROR_INVALID_FUNCTION: u64 = 1;
const ERROR_FILE_NOT_FOUND: u64 = 2;
const ERROR_ACCESS_DENIED: u64 = 5;
const LOGON32_LOGON_INTERACTIVE: u32 = 2;
const ERROR_INVALID_PARAMETER: u64 = 87;
const ERROR_INSUFFICIENT_BUFFER: u64 = 122;

const DISK_SIZE: u64 = 16 << 20;
const SECTOR: usize = 512;
/// What xtask puts at the start of sector 1.
const SECTOR1: &[u8] = b"gala disk_test sector 1";
/// What this program writes at the start of sector 2 (xtask checks it).
const SECTOR2: &[u8] = b"written by disk_test";

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
        out(b"disk_test: check ");
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

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from(u32_at(b, at)) | u64::from(u32_at(b, at + 4)) << 32
}

/// DISK_GEOMETRY: Cylinders, MediaType, TracksPerCylinder,
/// SectorsPerTrack, BytesPerSector.
fn check_geometry(c: &mut Checks, g: &[u8]) -> Result<(), u32> {
    c.eq(u32_at(g, 8).into(), FIXED_MEDIA.into())?;
    c.eq(u32_at(g, 12).into(), 255)?;
    c.eq(u32_at(g, 16).into(), 63)?;
    c.eq(u32_at(g, 20).into(), SECTOR as u64)?;
    // Whole cylinders: what the geometry covers is not more than the disk.
    let cylinder = 255 * 63 * SECTOR as u64;
    c.eq(u64_at(g, 0), DISK_SIZE / cylinder)
}

fn ioctls(c: &mut Checks, disk: Handle) -> Result<(), u32> {
    let mut buf = [0u8; 256];
    let mut n = 0;
    // SAFETY: valid buffers for each call.
    unsafe {
        let p = buf.as_mut_ptr().cast();
        let null = core::ptr::null();
        let none = core::ptr::null_mut();
        let ok = DeviceIoControl(disk, IOCTL_DISK_GET_LENGTH_INFO, null, 0, p, 256, &mut n, none);
        c.ok(ok != 0, last_error())?;
        c.eq(n.into(), 8)?;
        c.eq(u64_at(&buf, 0), DISK_SIZE)?;

        buf = [0; 256];
        let ok =
            DeviceIoControl(disk, IOCTL_DISK_GET_DRIVE_GEOMETRY, null, 0, p, 256, &mut n, none);
        c.ok(ok != 0, last_error())?;
        c.eq(n.into(), 24)?;
        check_geometry(c, &buf)?;
        let ok = DeviceIoControl(disk, IOCTL_DISK_GET_DRIVE_GEOMETRY, null, 0, p, 8, &mut n, none);
        c.eq(ok as u64, 0)?;
        c.eq(last_error(), ERROR_INSUFFICIENT_BUFFER)?;

        buf = [0; 256];
        let ok =
            DeviceIoControl(disk, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, null, 0, p, 256, &mut n, none);
        c.ok(ok != 0, last_error())?;
        c.ok(n >= 32, n.into())?;
        check_geometry(c, &buf)?;
        c.eq(u64_at(&buf, 24), DISK_SIZE)?;

        // Function 0xfff of the disk's device type, which it does not have.
        let ok = DeviceIoControl(disk, 0x0007_3ffc, null, 0, p, 256, &mut n, none);
        c.eq(ok as u64, 0)?;
        c.eq(last_error(), ERROR_INVALID_FUNCTION)?;
    }
    Ok(())
}

fn read(disk: Handle, buf: &mut [u8]) -> (bool, u32) {
    let mut n = 0;
    // SAFETY: valid buffer and count.
    let ok = unsafe {
        ReadFile(disk, buf.as_mut_ptr(), buf.len() as u32, &mut n, core::ptr::null_mut())
    };
    (ok != 0, n)
}

fn write(disk: Handle, data: &[u8]) -> (bool, u32) {
    let mut n = 0;
    // SAFETY: valid buffer and count.
    let ok =
        unsafe { WriteFile(disk, data.as_ptr(), data.len() as u32, &mut n, core::ptr::null_mut()) };
    (ok != 0, n)
}

fn seek(disk: Handle, to: i64, method: u32) -> Option<i64> {
    let mut at = -1;
    // SAFETY: valid output pointer.
    (unsafe { SetFilePointerEx(disk, to, &mut at, method) } != 0).then_some(at)
}

fn transfers(c: &mut Checks, disk: Handle) -> Result<(), u32> {
    let mut sector = [0u8; SECTOR];
    // Sector 0 of the fresh disk is zeros; the pointer moves on.
    let (ok, n) = read(disk, &mut sector);
    c.ok(ok, last_error())?;
    c.eq(n.into(), SECTOR as u64)?;
    c.ok(sector.iter().all(|&b| b == 0), 0)?;
    c.eq(seek(disk, 0, FILE_CURRENT).unwrap_or(-1) as u64, SECTOR as u64)?;
    // Sector 1: xtask's text.
    let (ok, n) = read(disk, &mut sector);
    c.ok(ok && n as usize == SECTOR, last_error())?;
    c.ok(sector.starts_with(SECTOR1), sector[0].into())?;

    // Sector 2: write, then read back.
    let mut data = [0u8; SECTOR];
    data[..SECTOR2.len()].copy_from_slice(SECTOR2);
    for (i, b) in data.iter_mut().enumerate().skip(SECTOR2.len()) {
        *b = i as u8;
    }
    let (ok, n) = write(disk, &data);
    c.ok(ok, last_error())?;
    c.eq(n.into(), SECTOR as u64)?;
    c.eq(seek(disk, 2 * SECTOR as i64, FILE_BEGIN).unwrap_or(-1) as u64, 2 * SECTOR as u64)?;
    sector = [0; SECTOR];
    let (ok, _) = read(disk, &mut sector);
    c.ok(ok && sector == data, sector[0].into())?;

    // 64 KiB from offset 0 covers sectors 0-127.
    static mut BIG: [u8; 65536] = [0; 65536];
    // SAFETY: the only user of BIG, single-threaded.
    let big = unsafe { &mut *core::ptr::addr_of_mut!(BIG) };
    c.eq(seek(disk, 0, FILE_BEGIN).unwrap_or(-1) as u64, 0)?;
    let (ok, n) = read(disk, big);
    c.ok(ok, last_error())?;
    c.eq(n.into(), 65536)?;
    c.ok(big[SECTOR..].starts_with(SECTOR1), big[SECTOR].into())?;
    c.ok(big[2 * SECTOR..3 * SECTOR] == data, big[2 * SECTOR].into())?;

    // Not whole sectors: refused.
    c.eq(seek(disk, 0, FILE_BEGIN).unwrap_or(-1) as u64, 0)?;
    let (ok, _) = read(disk, &mut sector[..100]);
    c.eq(ok as u64, 0)?;
    c.eq(last_error(), ERROR_INVALID_PARAMETER)?;
    let (ok, _) = write(disk, &data[..100]);
    c.eq(ok as u64, 0)?;
    c.eq(last_error(), ERROR_INVALID_PARAMETER)?;
    c.eq(seek(disk, 7, FILE_BEGIN).unwrap_or(-1) as u64, 7)?;
    let (ok, _) = read(disk, &mut sector);
    c.eq(ok as u64, 0)?;
    c.eq(last_error(), ERROR_INVALID_PARAMETER)?;

    // An explicit offset (OVERLAPPED on a synchronous handle).
    let mut ov = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: 512,
        offset_high: 0,
        event: core::ptr::null_mut(),
    };
    sector = [0; SECTOR];
    let mut n = 0;
    // SAFETY: valid buffer, count and OVERLAPPED.
    let ok = unsafe { ReadFile(disk, sector.as_mut_ptr(), SECTOR as u32, &mut n, &mut ov) };
    c.ok(ok != 0, last_error())?;
    c.ok(sector.starts_with(SECTOR1), sector[0].into())?;

    // SAFETY: a valid handle.
    c.ok(unsafe { FlushFileBuffers(disk) } != 0, last_error())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let name: [u16; 20] = wide("\\\\.\\PhysicalDrive0");
    let missing: [u16; 20] = wide("\\\\.\\PhysicalDrive9");
    // SAFETY: valid names; the handles are closed below.
    unsafe {
        let access = GENERIC_READ | GENERIC_WRITE;
        let share = FILE_SHARE_READ | FILE_SHARE_WRITE;
        let none = core::ptr::null_mut();
        let disk =
            CreateFileW(name.as_ptr(), access, share, core::ptr::null(), OPEN_EXISTING, 0, none);
        c.ok(disk as isize != -1, last_error())?;
        c.eq(GetFileType(disk).into(), FILE_TYPE_DISK.into())?;
        ioctls(&mut c, disk)?;
        transfers(&mut c, disk)?;
        CloseHandle(disk);
        let h =
            CreateFileW(missing.as_ptr(), access, share, core::ptr::null(), OPEN_EXISTING, 0, none);
        c.eq(h as isize as u64, u64::MAX)?;
        c.eq(last_error(), ERROR_FILE_NOT_FOUND)?;

        // A standard user is refused.
        let (user, dot, password): ([u16; 9], [u16; 2], [u16; 12]) =
            (wide("galatest"), wide("."), wide("Gala-Test-1"));
        let mut token = none;
        let kind = LOGON32_LOGON_INTERACTIVE;
        let ok = LogonUserW(user.as_ptr(), dot.as_ptr(), password.as_ptr(), kind, 0, &mut token);
        c.ok(ok != 0, last_error())?;
        c.ok(ImpersonateLoggedOnUser(token) != 0, last_error())?;
        let h = CreateFileW(
            name.as_ptr(),
            GENERIC_READ,
            share,
            core::ptr::null(),
            OPEN_EXISTING,
            0,
            none,
        );
        let error = last_error();
        RevertToSelf();
        c.eq(h as isize as u64, u64::MAX)?;
        c.eq(error, ERROR_ACCESS_DENIED)?;
        CloseHandle(token);
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"disk_test: ok\r\n");
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
