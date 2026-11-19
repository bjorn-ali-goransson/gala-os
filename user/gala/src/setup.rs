//! `gala setup <disk> --erase [--from <release.tar>]`: installs gala onto
//! a disk (ROADMAP IN step 4; ARCHITECTURE "Installing gala onto a
//! disk"). The disk (`\\.\PhysicalDrive<disk>`) gets a GPT through
//! Windows' disk IOCTLs: an EFI system partition of 256 MiB at 1 MiB,
//! holding a release's files (the tar `--from`, by default
//! `C:\gala\release.tar`: what the update feed publishes, Limine and
//! `EFI/BOOT/BOOTX64.EFI` among them) as FAT32 (the `setup` library), and
//! gala's C: partition over the rest, which the kernel formats at the
//! first boot from the disk. Everything on the disk is lost: without
//! `--erase` nothing is written. Each step is printed. Exit codes: 0
//! done, 1 usage, 4 failed (what failed is printed), 6 needs an
//! administrator (the raw disk).

use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;

use super::{Args, Bool, Handle, fail, out};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
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
    fn GetFileSizeEx(h: Handle, size: *mut i64) -> Bool;
    fn FlushFileBuffers(h: Handle) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetLastError() -> u32;
    fn GetSystemTimeAsFileTime(time: *mut u64);
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const SHARE_READ_WRITE: u32 = 3;
const OPEN_EXISTING: u32 = 3;
const INVALID: Handle = -1isize as Handle;
const ERROR_ACCESS_DENIED: u32 = 5;

const IOCTL_DISK_GET_LENGTH_INFO: u32 = 0x0007_405c;
const IOCTL_DISK_SET_DRIVE_LAYOUT_EX: u32 = 0x0007_c054;
const IOCTL_DISK_CREATE_DISK: u32 = 0x0007_c058;
const IOCTL_DISK_UPDATE_PROPERTIES: u32 = 0x0007_0140;
const STYLE_GPT: u32 = 1;
const LAYOUT_HEADER: usize = 48;
const PARTITION: usize = 144;

const EXIT_USAGE: u32 = 1;
const EXIT_FAILED: u32 = 4;
const EXIT_ADMIN: u32 = 6;

const MIB: u64 = 1 << 20;
/// The EFI system partition: from 1 MiB, 256 MiB (FAT32 with one sector
/// per cluster: more than its 65525 clusters).
const ESP_START: u64 = MIB;
const ESP_SIZE: u64 = 256 * MIB;
/// The least room for C: after it.
const MIN_C: u64 = 64 * MIB;

/// {C12A7328-F81F-11D2-BA4B-00A0C93EC93B}: the EFI system partition.
const ESP_TYPE: [u8; 16] = [
    0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b,
];
/// gala's C: partition, {6A1A0C0D-E000-4C3A-8000-67616C614300} (the
/// kernel's `gpt::GALA_C_TYPE`).
const GALA_C_TYPE: [u8; 16] = [
    0x0d, 0x0c, 0x1a, 0x6a, 0x00, 0xe0, 0x3a, 0x4c, 0x80, 0x00, 0x67, 0x61, 0x6c, 0x61, 0x43, 0x00,
];

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

fn last_error() -> u32 {
    // SAFETY: no arguments.
    unsafe { GetLastError() }
}

fn open(path: &str) -> Result<Handle, u32> {
    let p = wide(path);
    // SAFETY: a NUL-terminated name.
    let h = unsafe {
        CreateFileW(
            p.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            SHARE_READ_WRITE,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        )
    };
    if h == INVALID { Err(last_error()) } else { Ok(h) }
}

fn control(h: Handle, code: u32, input: &[u8], output: &mut [u8]) -> Result<u32, u32> {
    let mut n = 0;
    // SAFETY: our buffers with their lengths.
    let ok = unsafe {
        DeviceIoControl(
            h,
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

fn write_at(h: Handle, sector: u64, data: &[u8]) -> Result<(), u32> {
    let mut n = 0;
    // SAFETY: our buffer with its length.
    let ok = unsafe {
        SetFilePointerEx(h, (sector * 512) as i64, core::ptr::null_mut(), 0) != 0
            && WriteFile(h, data.as_ptr(), data.len() as u32, &mut n, core::ptr::null_mut()) != 0
    };
    if ok && n as usize == data.len() { Ok(()) } else { Err(last_error()) }
}

fn read_whole(path: &str) -> Result<Vec<u8>, u32> {
    let p = wide(path);
    // SAFETY: plain calls with our buffers; the handle is closed.
    unsafe {
        let h = CreateFileW(
            p.as_ptr(),
            GENERIC_READ,
            SHARE_READ_WRITE,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        );
        if h == INVALID {
            return Err(last_error());
        }
        let mut size = 0i64;
        let mut data = Vec::new();
        let ok = GetFileSizeEx(h, &mut size) != 0 && data.try_reserve_exact(size as usize).is_ok();
        if !ok {
            CloseHandle(h);
            return Err(8);
        }
        data.resize(size as usize, 0);
        let mut at = 0usize;
        while at < data.len() {
            let mut got = 0u32;
            let chunk = (data.len() - at).min(1 << 20) as u32;
            if ReadFile(h, data.as_mut_ptr().add(at), chunk, &mut got, core::ptr::null_mut()) == 0
                || got == 0
            {
                let e = last_error();
                CloseHandle(h);
                return Err(e);
            }
            at += got as usize;
        }
        CloseHandle(h);
        Ok(data)
    }
}

/// A GUID for the disk or a partition: the clock and `salt` mixed
/// (version-4 and variant bits set).
fn new_guid(salt: u64) -> [u8; 16] {
    let mut t = 0u64;
    // SAFETY: an output pointer.
    unsafe { GetSystemTimeAsFileTime(&mut t) };
    let mut x = t ^ salt.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let mut g = [0u8; 16];
    for chunk in g.chunks_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        chunk.copy_from_slice(&x.to_le_bytes());
    }
    g[7] = (g[7] & 0x0f) | 0x40;
    g[8] = (g[8] & 0x3f) | 0x80;
    g
}

fn put_partition(
    b: &mut [u8],
    at: usize,
    start: u64,
    length: u64,
    kind: &[u8; 16],
    id: [u8; 16],
    name: &str,
) {
    b[at..at + 4].copy_from_slice(&STYLE_GPT.to_le_bytes());
    b[at + 8..at + 16].copy_from_slice(&start.to_le_bytes());
    b[at + 16..at + 24].copy_from_slice(&length.to_le_bytes());
    b[at + 28] = 1;
    b[at + 32..at + 48].copy_from_slice(kind);
    b[at + 48..at + 64].copy_from_slice(&id);
    for (i, u) in name.encode_utf16().take(36).enumerate() {
        b[at + 72 + 2 * i..at + 74 + 2 * i].copy_from_slice(&u.to_le_bytes());
    }
}

fn say(parts: &[&str]) {
    for p in parts {
        out(p);
    }
    out("\r\n");
}

fn num(v: u64) -> String {
    let mut s = String::new();
    let mut digits = [0u8; 20];
    let (mut n, mut len) = (v, 0);
    loop {
        digits[len] = b'0' + (n % 10) as u8;
        len += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for i in (0..len).rev() {
        s.push(digits[i] as char);
    }
    s
}

pub fn run(a: &Args) -> u32 {
    let usage = "gala setup <disk> --erase [--from <release.tar>]";
    let mut disk = None;
    let mut erase = false;
    let mut from = String::from("C:\\gala\\release.tar");
    let mut i = 2;
    while i < a.count {
        if a.is(i, "--erase") {
            erase = true;
        } else if a.is(i, "--from") && i + 1 < a.count {
            i += 1;
            from = String::from_utf16_lossy(a.get(i));
        } else if disk.is_none() {
            disk = super::number(a.get(i)).and_then(|n| u32::try_from(n).ok());
            if disk.is_none() {
                return fail(EXIT_USAGE, &[usage]);
            }
        } else {
            return fail(EXIT_USAGE, &[usage]);
        }
        i += 1;
    }
    let Some(disk) = disk else { return fail(EXIT_USAGE, &[usage]) };
    if !erase {
        return fail(
            EXIT_USAGE,
            &["gala setup erases the whole disk: say so with --erase (", usage, ")"],
        );
    }
    let disk_n = num(u64::from(disk));
    let tar = match read_whole(&from) {
        Ok(t) => t,
        Err(e) => {
            return fail(
                EXIT_FAILED,
                &["gala setup: cannot read ", &from, " (error ", &num(u64::from(e)), ")"],
            );
        }
    };
    let entries = match setup::tar::entries(&tar) {
        Ok(e) => e,
        Err(e) => {
            return fail(
                EXIT_FAILED,
                &[
                    "gala setup: ",
                    &from,
                    " is not a release tar (",
                    if e == setup::tar::Error::Truncated {
                        "cut short"
                    } else {
                        "a damaged header"
                    },
                    ")",
                ],
            );
        }
    };
    let bytes: u64 = entries.iter().map(|e| e.data.len() as u64).sum();
    say(&[
        "gala setup: release ",
        &from,
        ": ",
        &num(entries.len() as u64),
        " files, ",
        &num(bytes / MIB),
        " MiB",
    ]);

    let h = match open(&alloc::format!("\\\\.\\PhysicalDrive{disk}")) {
        Ok(h) => h,
        Err(ERROR_ACCESS_DENIED) => {
            return fail(EXIT_ADMIN, &["gala setup: the disk needs an administrator"]);
        }
        Err(e) => {
            return fail(
                EXIT_FAILED,
                &["gala setup: no disk ", &disk_n, " (error ", &num(u64::from(e)), ")"],
            );
        }
    };
    let result = install(h, disk, &entries);
    // SAFETY: an open handle.
    unsafe { CloseHandle(h) };
    match result {
        Ok(c_mib) => {
            say(&[
                "gala setup: done: disk ",
                &disk_n,
                " boots gala; C: (",
                &num(c_mib),
                " MiB) is formatted at its first boot",
            ]);
            0
        }
        Err(what) => fail(EXIT_FAILED, &["gala setup: ", &what]),
    }
}

/// Partitions the disk and fills its EFI system partition; C:'s size in
/// MiB, or what failed.
fn install(h: Handle, disk: u32, entries: &[setup::tar::Entry<'_>]) -> Result<u64, String> {
    let err = |what: &str, e: u32| alloc::format!("{what} failed (error {e})");
    let mut len = [0u8; 8];
    control(h, IOCTL_DISK_GET_LENGTH_INFO, &[], &mut len)
        .map_err(|e| err("reading the disk's size", e))?;
    let size = u64::from_le_bytes(len);
    // The GPT's usable range ends 34 sectors before the disk's end.
    let usable_end = size - 34 * 512;
    let c_start = ESP_START + ESP_SIZE;
    if usable_end < c_start + MIN_C {
        return Err(alloc::format!(
            "the disk ({} MiB) is too small: gala needs {} MiB",
            size / MIB,
            (c_start + MIN_C) / MIB + 1
        ));
    }
    let c_size = (usable_end - c_start) / MIB * MIB;

    let mut create = [0u8; 24];
    create[0..4].copy_from_slice(&STYLE_GPT.to_le_bytes());
    create[4..20].copy_from_slice(&new_guid(1));
    create[20..24].copy_from_slice(&128u32.to_le_bytes());
    control(h, IOCTL_DISK_CREATE_DISK, &create, &mut []).map_err(|e| err("writing a GPT", e))?;
    let mut layout = [0u8; LAYOUT_HEADER + 2 * PARTITION];
    layout[0..4].copy_from_slice(&STYLE_GPT.to_le_bytes());
    layout[4..8].copy_from_slice(&2u32.to_le_bytes());
    layout[8..24].copy_from_slice(&create[4..20]);
    layout[40..44].copy_from_slice(&128u32.to_le_bytes());
    put_partition(
        &mut layout,
        LAYOUT_HEADER,
        ESP_START,
        ESP_SIZE,
        &ESP_TYPE,
        new_guid(2),
        "EFI system partition",
    );
    put_partition(
        &mut layout,
        LAYOUT_HEADER + PARTITION,
        c_start,
        c_size,
        &GALA_C_TYPE,
        new_guid(3),
        "gala C:",
    );
    control(h, IOCTL_DISK_SET_DRIVE_LAYOUT_EX, &layout, &mut [])
        .map_err(|e| err("writing the partitions", e))?;
    control(h, IOCTL_DISK_UPDATE_PROPERTIES, &[], &mut [])
        .map_err(|e| err("updating the partitions", e))?;
    say(&[
        "gala setup: disk partitioned: EFI system partition ",
        &num(ESP_SIZE / MIB),
        " MiB, gala's C: ",
        &num(c_size / MIB),
        " MiB",
    ]);

    let esp = open(&alloc::format!("\\\\?\\GLOBALROOT\\Device\\Harddisk{disk}\\Partition1"))
        .map_err(|e| err("opening the EFI system partition", e))?;
    let files: Vec<(&str, &[u8])> = entries.iter().map(|e| (e.path.as_str(), e.data)).collect();
    let volume = setup::fat32::Volume {
        sectors: ESP_SIZE / 512,
        hidden: (ESP_START / 512) as u32,
        id: u32::from_le_bytes([create[4], create[5], create[6], create[7]]),
        label: *b"GALA ESP   ",
    };
    let made = setup::fat32::write(&volume, &files, |sector, data| write_at(esp, sector, data));
    // SAFETY: open handles.
    let flushed = unsafe { FlushFileBuffers(esp) != 0 && FlushFileBuffers(h) != 0 };
    // SAFETY: an open handle.
    unsafe { CloseHandle(esp) };
    match made {
        Ok(m) => say(&[
            "gala setup: EFI system partition written: FAT32, ",
            &num(files.len() as u64),
            " files, ",
            &num(u64::from(m.used) / 2048),
            " of ",
            &num(u64::from(m.clusters) / 2048),
            " MiB",
        ]),
        Err(setup::fat32::Error::Write(e)) => {
            return Err(err("writing the EFI system partition", e));
        }
        Err(setup::fat32::Error::Full) => {
            return Err(String::from("the release does not fit the EFI system partition"));
        }
        Err(_) => return Err(String::from("the EFI system partition could not be laid out")),
    }
    if !flushed {
        return Err(err("flushing the disk", last_error()));
    }
    Ok(c_size / MIB)
}
