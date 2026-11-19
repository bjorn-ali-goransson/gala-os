//! Partitioning a disk through `\\.\PhysicalDrive0`, as Windows' disk
//! IOCTLs do it (documented contracts: winioctl.h's CREATE_DISK,
//! DRIVE_LAYOUT_INFORMATION_EX and PARTITION_INFORMATION_EX; the GPT
//! layout of the UEFI specification). Runs as SYSTEM. xtask attaches an
//! empty 64 MiB disk and checks afterwards, with its own GPT reader, the
//! table this program wrote (ROADMAP IN step 1).
//!
//! - IOCTL_DISK_GET_DRIVE_LAYOUT_EX on the empty disk: PARTITION_STYLE_RAW,
//!   no partitions; a buffer smaller than the layout's header fails with
//!   ERROR_INSUFFICIENT_BUFFER.
//! - IOCTL_DISK_CREATE_DISK (GPT, a disk GUID, 128 entries): the layout
//!   is GPT with that GUID, no partitions, the usable range LBA 34 to the
//!   34th sector from the end, 128 entries at most.
//! - IOCTL_DISK_SET_DRIVE_LAYOUT_EX with an EFI system partition (1 MiB,
//!   16 MiB long) and a basic data partition (17 MiB, 32 MiB long), each
//!   with its GUID and name; IOCTL_DISK_UPDATE_PROPERTIES; the layout read
//!   back numbers them 1 and 2 with what was given.
//! - Sector 0 is the protective MBR (one partition of type 0xEE, 0x55AA),
//!   sector 1 starts "EFI PART".
//! - Partition 2 opens as `\\?\GLOBALROOT\Device\Harddisk0\Partition2`:
//!   its length (32 MiB), IOCTL_DISK_GET_PARTITION_INFO_EX (its entry,
//!   numbered 2); its first sector written through it is the disk's sector
//!   at 17 MiB (read back through PhysicalDrive0; xtask checks it too).
//!
//! Before any of it, the disk must be that scratch disk (64 MiB, no
//! partition table): otherwise the program writes nothing and exits with
//! 0xd15c (`partition_test: refused`).
//!
//! Prints `partition_test: ok` or exits with the number of the failing
//! check.

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
    fn DeviceIoControl(
        h: Handle,
        code: u32,
        input: *const c_void,
        input_size: u32,
        output: *mut c_void,
        output_size: u32,
        returned: *mut u32,
        overlapped: *mut c_void,
    ) -> Bool;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn SetFilePointerEx(h: Handle, distance: i64, new: *mut i64, method: u32) -> Bool;
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

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const FILE_BEGIN: u32 = 0;

const IOCTL_DISK_GET_LENGTH_INFO: u32 = 0x0007_405c;
const IOCTL_DISK_GET_DRIVE_LAYOUT_EX: u32 = 0x0007_0050;
const IOCTL_DISK_SET_DRIVE_LAYOUT_EX: u32 = 0x0007_c054;
const IOCTL_DISK_CREATE_DISK: u32 = 0x0007_c058;
const IOCTL_DISK_UPDATE_PROPERTIES: u32 = 0x0007_0140;
const IOCTL_DISK_GET_PARTITION_INFO_EX: u32 = 0x0007_0048;

const PARTITION_STYLE_GPT: u32 = 1;
const PARTITION_STYLE_RAW: u32 = 2;
const ERROR_INSUFFICIENT_BUFFER: u64 = 122;

const DISK_SIZE: u64 = 64 << 20;
const SECTOR: u64 = 512;
const MIB: u64 = 1 << 20;
/// DRIVE_LAYOUT_INFORMATION_EX's header: PartitionStyle, PartitionCount,
/// then the GPT part (DiskId, StartingUsableOffset, UsableLength,
/// MaxPartitionCount; 40 bytes with its padding).
const LAYOUT_HEADER: usize = 48;
/// PARTITION_INFORMATION_EX: PartitionStyle, StartingOffset,
/// PartitionLength, PartitionNumber, RewritePartition,
/// IsServicePartition, then the GPT part (PartitionType, PartitionId,
/// Attributes, Name[36]).
const PARTITION: usize = 144;

/// {C12A7328-F81F-11D2-BA4B-00A0C93EC93B}: the EFI system partition.
const ESP_TYPE: [u8; 16] =
    guid(0xc12a_7328, 0xf81f, 0x11d2, [0xba, 0x4b, 0, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b]);
/// {EBD0A0A2-B9E5-4433-87C0-68B6B72699C7}: basic data.
const DATA_TYPE: [u8; 16] =
    guid(0xebd0_a0a2, 0xb9e5, 0x4433, [0x87, 0xc0, 0x68, 0xb6, 0xb7, 0x26, 0x99, 0xc7]);
/// The disk's and the partitions' GUIDs (xtask checks them).
const DISK_ID: [u8; 16] = guid(0x6a1a_d15c, 0x0001, 0x4000, [0x80, 0, 0, 0, 0, 0, 0, 1]);
const ESP_ID: [u8; 16] = guid(0x6a1a_d15c, 0x0002, 0x4000, [0x80, 0, 0, 0, 0, 0, 0, 2]);
const DATA_ID: [u8; 16] = guid(0x6a1a_d15c, 0x0003, 0x4000, [0x80, 0, 0, 0, 0, 0, 0, 3]);

/// A GUID's bytes as Windows' GUID structure holds them (and the GPT
/// stores them): Data1, Data2, Data3 little-endian, then Data4.
const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> [u8; 16] {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

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
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"partition_test: check ");
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

    #[track_caller]
    fn same(&mut self, got: &[u8], want: &[u8]) -> Result<(), u32> {
        self.ok(got == want, got.first().copied().unwrap_or(0).into())
    }
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

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// A partition's name as PARTITION_INFORMATION_GPT holds it (36 UTF-16
/// units, NUL-padded).
fn name36(s: &str) -> [u8; 72] {
    let mut n = [0u8; 72];
    for (i, b) in s.bytes().take(36).enumerate() {
        n[2 * i] = b;
    }
    n
}

/// DeviceIoControl with `input`, into `output`: Ok(bytes returned) or
/// Err(the last error).
fn control(disk: Handle, code: u32, input: &[u8], output: &mut [u8]) -> Result<u32, u64> {
    let mut n = 0;
    // SAFETY: valid buffers of the given lengths.
    let ok = unsafe {
        DeviceIoControl(
            disk,
            code,
            if input.is_empty() { core::ptr::null() } else { input.as_ptr().cast() },
            input.len() as u32,
            if output.is_empty() { core::ptr::null_mut() } else { output.as_mut_ptr().cast() },
            output.len() as u32,
            &mut n,
            core::ptr::null_mut(),
        )
    };
    if ok != 0 { Ok(n) } else { Err(last_error()) }
}

fn read_sector(disk: Handle, lba: u64, buf: &mut [u8; 512]) -> bool {
    let mut n = 0;
    // SAFETY: valid buffers.
    unsafe {
        SetFilePointerEx(disk, (lba * SECTOR) as i64, core::ptr::null_mut(), FILE_BEGIN) != 0
            && ReadFile(disk, buf.as_mut_ptr(), 512, &mut n, core::ptr::null_mut()) != 0
            && n == 512
    }
}

/// One GPT partition of SET_DRIVE_LAYOUT_EX's input.
fn put_partition(
    b: &mut [u8],
    at: usize,
    start: u64,
    length: u64,
    kind: &[u8; 16],
    id: &[u8; 16],
    name: &str,
) {
    put32(b, at, PARTITION_STYLE_GPT);
    put64(b, at + 8, start);
    put64(b, at + 16, length);
    b[at + 28] = 1; // RewritePartition
    b[at + 32..at + 48].copy_from_slice(kind);
    b[at + 48..at + 64].copy_from_slice(id);
    b[at + 72..at + 144].copy_from_slice(&name36(name));
}

/// Partition `i` of a layout read back: as put_partition gave it, numbered
/// `i + 1`.
#[allow(clippy::too_many_arguments)]
fn check_partition(
    c: &mut Checks,
    layout: &[u8],
    i: usize,
    start: u64,
    length: u64,
    kind: &[u8; 16],
    id: &[u8; 16],
    name: &str,
) -> Result<(), u32> {
    let at = LAYOUT_HEADER + i * PARTITION;
    c.eq(u32_at(layout, at).into(), PARTITION_STYLE_GPT.into())?;
    c.eq(u64_at(layout, at + 8), start)?;
    c.eq(u64_at(layout, at + 16), length)?;
    c.eq(u32_at(layout, at + 24).into(), i as u64 + 1)?;
    c.same(&layout[at + 32..at + 48], kind)?;
    c.same(&layout[at + 48..at + 64], id)?;
    c.eq(u64_at(layout, at + 64), 0)?;
    c.same(&layout[at + 72..at + 144], &name36(name))
}

const REFUSED: u32 = 0xd15c;
/// What the program writes at the start of partition 2 (xtask checks it).
const PART2_TEXT: &[u8] = b"written by partition_test to partition 2";

/// CreateFileW of `name` for reading and writing, shared.
fn open(name: &str) -> Handle {
    let mut w = [0u16; 64];
    for (d, b) in w.iter_mut().zip(name.bytes()) {
        *d = u16::from(b);
    }
    // SAFETY: a NUL-terminated name.
    unsafe {
        CreateFileW(
            w.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            core::ptr::null(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        )
    }
}

fn run(c: &mut Checks) -> Result<(), u32> {
    let path: [u16; 19] = {
        let mut w = [0u16; 19];
        for (d, b) in w.iter_mut().zip(b"\\\\.\\PhysicalDrive0") {
            *d = u16::from(*b);
        }
        w
    };
    // SAFETY: a NUL-terminated name.
    let disk = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            core::ptr::null(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        )
    };
    c.ok(disk as isize != -1, last_error())?;
    let mut layout = [0u8; LAYOUT_HEADER + 4 * PARTITION];

    // The scratch disk, or nothing is written.
    let mut len = [0u8; 8];
    let scratch = control(disk, IOCTL_DISK_GET_LENGTH_INFO, &[], &mut len).is_ok()
        && u64_at(&len, 0) == DISK_SIZE
        && control(disk, IOCTL_DISK_GET_DRIVE_LAYOUT_EX, &[], &mut layout).is_ok()
        && u32_at(&layout, 0) == PARTITION_STYLE_RAW;
    if !scratch {
        out(b"partition_test: refused (not an empty 64 MiB disk)\r\n");
        return Err(REFUSED);
    }
    c.eq(u32_at(&layout, 4).into(), 0)?;
    let mut small = [0u8; 16];
    c.eq(
        control(disk, IOCTL_DISK_GET_DRIVE_LAYOUT_EX, &[], &mut small).err().unwrap_or(0),
        ERROR_INSUFFICIENT_BUFFER,
    )?;

    // CREATE_DISK { PartitionStyle, { DiskId, MaxPartitionCount } }.
    let mut create = [0u8; 24];
    put32(&mut create, 0, PARTITION_STYLE_GPT);
    create[4..20].copy_from_slice(&DISK_ID);
    put32(&mut create, 20, 128);
    c.ok(control(disk, IOCTL_DISK_CREATE_DISK, &create, &mut []).is_ok(), last_error())?;
    layout = [0; LAYOUT_HEADER + 4 * PARTITION];
    c.ok(control(disk, IOCTL_DISK_GET_DRIVE_LAYOUT_EX, &[], &mut layout).is_ok(), last_error())?;
    c.eq(u32_at(&layout, 0).into(), PARTITION_STYLE_GPT.into())?;
    c.eq(u32_at(&layout, 4).into(), 0)?;
    c.same(&layout[8..24], &DISK_ID)?;
    let sectors = DISK_SIZE / SECTOR;
    c.eq(u64_at(&layout, 24), 34 * SECTOR)?;
    c.eq(u64_at(&layout, 32), (sectors - 34 - 34 + 1) * SECTOR)?;
    c.eq(u32_at(&layout, 40).into(), 128)?;

    // SET_DRIVE_LAYOUT_EX: the header as read, two partitions.
    let mut set = [0u8; LAYOUT_HEADER + 2 * PARTITION];
    set[..LAYOUT_HEADER].copy_from_slice(&layout[..LAYOUT_HEADER]);
    put32(&mut set, 4, 2);
    put_partition(
        &mut set,
        LAYOUT_HEADER,
        MIB,
        16 * MIB,
        &ESP_TYPE,
        &ESP_ID,
        "EFI system partition",
    );
    put_partition(
        &mut set,
        LAYOUT_HEADER + PARTITION,
        17 * MIB,
        32 * MIB,
        &DATA_TYPE,
        &DATA_ID,
        "gala",
    );
    c.ok(control(disk, IOCTL_DISK_SET_DRIVE_LAYOUT_EX, &set, &mut []).is_ok(), last_error())?;
    c.ok(control(disk, IOCTL_DISK_UPDATE_PROPERTIES, &[], &mut []).is_ok(), last_error())?;
    layout = [0; LAYOUT_HEADER + 4 * PARTITION];
    let n = control(disk, IOCTL_DISK_GET_DRIVE_LAYOUT_EX, &[], &mut layout);
    c.ok(n.is_ok(), n.err().unwrap_or(0))?;
    c.eq(u32_at(&layout, 0).into(), PARTITION_STYLE_GPT.into())?;
    c.eq(u32_at(&layout, 4).into(), 2)?;
    c.same(&layout[8..24], &DISK_ID)?;
    check_partition(c, &layout, 0, MIB, 16 * MIB, &ESP_TYPE, &ESP_ID, "EFI system partition")?;
    check_partition(c, &layout, 1, 17 * MIB, 32 * MIB, &DATA_TYPE, &DATA_ID, "gala")?;

    // On the disk: the protective MBR and the GPT header.
    let mut sector = [0u8; 512];
    c.ok(read_sector(disk, 0, &mut sector), last_error())?;
    c.eq(sector[446 + 4].into(), 0xee)?;
    c.eq(u32_at(&sector, 446 + 8).into(), 1)?;
    c.eq(sector[510].into(), 0x55)?;
    c.eq(sector[511].into(), 0xaa)?;
    c.ok(read_sector(disk, 1, &mut sector), last_error())?;
    c.same(&sector[..8], b"EFI PART")?;

    // Partition 2 as its own device: offsets are the partition's.
    let part = open(r"\\?\GLOBALROOT\Device\Harddisk0\Partition2");
    c.ok(part as isize != -1, last_error())?;
    c.ok(control(part, IOCTL_DISK_GET_LENGTH_INFO, &[], &mut len).is_ok(), last_error())?;
    c.eq(u64_at(&len, 0), 32 * MIB)?;
    let mut info = [0u8; PARTITION];
    let n = control(part, IOCTL_DISK_GET_PARTITION_INFO_EX, &[], &mut info);
    c.ok(n.is_ok(), n.err().unwrap_or(0))?;
    c.eq(u32_at(&info, 0).into(), PARTITION_STYLE_GPT.into())?;
    c.eq(u64_at(&info, 8), 17 * MIB)?;
    c.eq(u64_at(&info, 16), 32 * MIB)?;
    c.eq(u32_at(&info, 24).into(), 2)?;
    c.same(&info[32..48], &DATA_TYPE)?;
    c.same(&info[48..64], &DATA_ID)?;
    let mut text = [0u8; 512];
    text[..PART2_TEXT.len()].copy_from_slice(PART2_TEXT);
    let mut n = 0;
    // SAFETY: valid buffers.
    let wrote = unsafe { WriteFile(part, text.as_ptr(), 512, &mut n, core::ptr::null_mut()) };
    c.ok(wrote != 0 && n == 512, last_error())?;
    c.ok(read_sector(disk, 17 * MIB / SECTOR, &mut sector), last_error())?;
    c.same(&sector[..PART2_TEXT.len()], PART2_TEXT)?;
    // SAFETY: open handles.
    unsafe {
        CloseHandle(part);
        CloseHandle(disk);
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let mut c = Checks(0);
    let code = match run(&mut c) {
        Ok(()) => {
            out(b"partition_test: ok\r\n");
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
