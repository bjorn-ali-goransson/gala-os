//! SetupIterateCabinetW(CabinetFile, Reserved, MsgHandler, Context): each
//! file of a cabinet through the caller's callback, extracted where it
//! says (setupapi.h; Wine's setupcab.c tests, run on Windows, for the
//! errors and the notifications' contents).
//!
//! - A NULL path is ERROR_INVALID_PARAMETER, an empty one
//!   ERROR_NOT_ENOUGH_MEMORY (Windows' answers); a file that does not
//!   open fails with CreateFileW's error; one that is not a cabinet
//!   galapkg can read is ERROR_INVALID_DATA (recorded with the reason),
//!   and so is a valid one with no callback.
//! - The notifications, in order: SPFILENOTIFY_CABINETINFO (CABINET_INFO_W:
//!   the cabinet's directory with its trailing backslash, the next
//!   cabinet's and the disk's names empty, the set id and index from the
//!   header); a non-zero answer stops with that error. Then for each file
//!   (folder by folder, each folder decompressed once):
//!   SPFILENOTIFY_FILEINCABINET (FILE_IN_CABINET_INFO_W, param2 the path
//!   as passed): FILEOP_DOIT extracts to FullTargetName and notifies
//!   SPFILENOTIFY_FILEEXTRACTED (FILEPATHS_W with the write's error; a
//!   non-zero answer stops with it), FILEOP_SKIP leaves the file out,
//!   FILEOP_ABORT stops with the info's Win32Error (ERROR_OPERATION_ABORTED
//!   when the callback set none). TRUE when every file was offered.
//! - An extracted file gets the cabinet's DOS date and time (local time)
//!   as its last-write time and its read-only, hidden, system and archive
//!   attributes; neither is confirmed against Windows yet.
//!
//! Not here (recorded): cabinets that continue into others
//! (SPFILENOTIFY_NEEDNEWCABINET: galapkg refuses them, ERROR_INVALID_DATA),
//! Quantum compression, a callback that faults (Windows answers
//! ERROR_INVALID_DATA; here the fault ends the process as any other).

use alloc::vec::Vec;
use core::ffi::c_void;

use galapkg::cab::{CabFile, Cabinet, Error, ExtractError};

use crate::{fail, observed, record};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetFullPathNameW(name: *const u16, len: u32, buf: *mut u16, file_part: *mut *mut u16)
    -> u32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> usize;
    fn ReadFile(h: usize, buf: *mut u8, n: u32, read: *mut u32, overlapped: *mut c_void) -> i32;
    fn WriteFile(
        h: usize,
        buf: *const u8,
        n: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn GetFileSizeEx(h: usize, size: *mut i64) -> i32;
    fn SetFileTime(h: usize, created: *const u64, accessed: *const u64, written: *const u64)
    -> i32;
    fn LocalFileTimeToFileTime(local: *const u64, utc: *mut u64) -> i32;
    fn CloseHandle(h: usize) -> i32;
    fn GetLastError() -> u32;
}

type Callback = extern "system" fn(*mut c_void, u32, usize, usize) -> u32;

/// CABINET_INFO_W.
#[repr(C)]
struct CabinetInfo {
    path: *const u16,
    file: *const u16,
    disk: *const u16,
    set_id: u16,
    number: u16,
}

/// FILE_IN_CABINET_INFO_W.
#[repr(C)]
struct FileInCabinet {
    name: *const u16,
    size: u32,
    error: u32,
    date: u16,
    time: u16,
    attributes: u16,
    target: [u16; MAX_PATH],
}

/// FILEPATHS_W.
#[repr(C)]
struct FilePaths {
    target: *const u16,
    source: *const u16,
    error: u32,
    flags: u32,
}

const MAX_PATH: usize = 260;
const SPFILENOTIFY_CABINETINFO: u32 = 0x10;
const SPFILENOTIFY_FILEINCABINET: u32 = 0x11;
const SPFILENOTIFY_FILEEXTRACTED: u32 = 0x13;
const FILEOP_ABORT: u32 = 0;
const FILEOP_DOIT: u32 = 1;
const FILEOP_SKIP: u32 = 2;

const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_INVALID_DATA: u32 = 13;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_OPERATION_ABORTED: u32 = 995;
const INVALID_HANDLE_VALUE: usize = usize::MAX;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 1;
const OPEN_EXISTING: u32 = 3;
const CREATE_ALWAYS: u32 = 2;
/// The DOS attributes a file in a cabinet carries over (read-only,
/// hidden, system, archive).
const KEPT_ATTRIBUTES: u16 = 0x27;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

/// The largest cabinet read (it is read whole: Office's are a few KiB to
/// a few MiB).
const MAX_CABINET: i64 = 1 << 30;

/// The file at `path` in a new buffer, or the Win32 error.
fn read_whole(path: *const u16) -> Result<Vec<u8>, u32> {
    // SAFETY: the caller's NUL-terminated path; our handle and buffer.
    unsafe {
        let h = CreateFileW(
            path,
            GENERIC_READ,
            FILE_SHARE_READ,
            core::ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        );
        if h == INVALID_HANDLE_VALUE {
            return Err(GetLastError());
        }
        let r = (|| {
            let mut size = 0i64;
            if GetFileSizeEx(h, &mut size) == 0 {
                return Err(GetLastError());
            }
            if size > MAX_CABINET {
                record(b"setupapi.dll!SetupIterateCabinetW(cabinet over 1 GiB)", size as u64);
                return Err(ERROR_NOT_ENOUGH_MEMORY);
            }
            let mut data = Vec::new();
            data.try_reserve_exact(size as usize).map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
            data.resize(size as usize, 0);
            let mut at = 0usize;
            while at < data.len() {
                let n = (data.len() - at).min(1 << 20) as u32;
                let mut got = 0u32;
                if ReadFile(h, data.as_mut_ptr().add(at), n, &mut got, core::ptr::null_mut()) == 0 {
                    return Err(GetLastError());
                }
                if got == 0 {
                    data.truncate(at);
                    break;
                }
                at += got as usize;
            }
            Ok(data)
        })();
        CloseHandle(h);
        r
    }
}

/// A file's name in the cabinet as a NUL-terminated wide string (UTF-8
/// or Latin-1 as galapkg read it; both are valid Rust strings now).
fn wide_name(f: &CabFile) -> Result<Vec<u16>, u32> {
    let mut out = Vec::new();
    out.try_reserve_exact(f.name.len() + 1).map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
    out.extend(f.name.encode_utf16());
    out.push(0);
    Ok(out)
}

/// The days from 1601-01-01 to the civil date (proleptic Gregorian).
fn days_since_1601(year: i64, month: i64, day: i64) -> i64 {
    // Days from 1970-01-01 (the era-based formula over March-first
    // years), then the 134774 days from 1601 to 1970.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468 + 134_774
}

/// The UTC FILETIME of a DOS date and time taken as local time, or None
/// for an invalid one.
fn file_time(date: u16, time: u16) -> Option<u64> {
    let (year, month, day) =
        (1980 + i64::from(date >> 9), i64::from((date >> 5) & 15), i64::from(date & 31));
    let (hour, minute, second) =
        (i64::from(time >> 11), i64::from((time >> 5) & 63), 2 * i64::from(time & 31));
    if !(1..=12).contains(&month) || day == 0 || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let seconds = days_since_1601(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
    let local = (seconds as u64).checked_mul(10_000_000)?;
    let mut utc = 0u64;
    // SAFETY: our FILETIMEs.
    (unsafe { LocalFileTimeToFileTime(&local, &mut utc) } != 0).then_some(utc)
}

/// Writes `data` to `target` with the file's time and attributes: the
/// Win32 error, 0 on success.
fn write_target(target: &[u16], f: &CabFile, data: &[u8]) -> u32 {
    let attributes = u32::from(f.attributes & KEPT_ATTRIBUTES);
    let attributes = if attributes == 0 { FILE_ATTRIBUTE_NORMAL } else { attributes };
    // SAFETY: a NUL-terminated path (FullTargetName, terminated below);
    // our handle and buffer.
    unsafe {
        let h = CreateFileW(
            target.as_ptr(),
            GENERIC_WRITE,
            0,
            core::ptr::null(),
            CREATE_ALWAYS,
            attributes,
            0,
        );
        if h == INVALID_HANDLE_VALUE {
            return GetLastError();
        }
        let mut error = 0;
        let mut at = 0usize;
        while at < data.len() {
            let n = (data.len() - at).min(1 << 20) as u32;
            let mut written = 0u32;
            if WriteFile(h, data.as_ptr().add(at), n, &mut written, core::ptr::null_mut()) == 0 {
                error = GetLastError();
                break;
            }
            at += written as usize;
        }
        if error == 0
            && let Some(t) = file_time(f.date, f.time)
        {
            SetFileTime(h, core::ptr::null(), core::ptr::null(), &t);
        }
        CloseHandle(h);
        error
    }
}

/// Why galapkg could not read the cabinet, for the ledger.
fn refuse(e: &Error, size: usize) -> u32 {
    match e {
        Error::Unsupported(_) => {
            record(b"setupapi.dll!SetupIterateCabinetW(cabinet feature)", size as u64)
        }
        Error::OutOfMemory(_) => return ERROR_NOT_ENOUGH_MEMORY,
        _ => observed(b"setupapi.dll!SetupIterateCabinetW: not a valid cabinet", size as u64),
    }
    ERROR_INVALID_DATA
}

/// SetupIterateCabinetW (module comment).
#[unsafe(no_mangle)]
pub extern "system" fn SetupIterateCabinetW(
    cabinet: *const u16,
    _reserved: u32,
    callback: Option<Callback>,
    context: *mut c_void,
) -> i32 {
    match iterate(cabinet, callback, context) {
        Ok(()) => 1,
        Err(e) => fail(e, 0),
    }
}

fn iterate(
    cabinet: *const u16,
    callback: Option<Callback>,
    context: *mut c_void,
) -> Result<(), u32> {
    if cabinet.is_null() {
        return Err(ERROR_INVALID_PARAMETER);
    }
    // SAFETY: the caller's NUL-terminated path.
    if unsafe { *cabinet } == 0 {
        return Err(ERROR_NOT_ENOUGH_MEMORY);
    }
    let data = read_whole(cabinet)?;
    let cab = Cabinet::open(&data).map_err(|e| refuse(&e, data.len()))?;
    let Some(callback) = callback else {
        return Err(ERROR_INVALID_DATA);
    };

    // The cabinet's directory, with its trailing backslash.
    let mut full = [0u16; 2 * MAX_PATH];
    let mut file_part: *mut u16 = core::ptr::null_mut();
    // SAFETY: the caller's path; our buffer.
    let n =
        unsafe { GetFullPathNameW(cabinet, full.len() as u32, full.as_mut_ptr(), &mut file_part) }
            as usize;
    if n == 0 || n >= full.len() {
        // SAFETY: plain call.
        return Err(unsafe { GetLastError() });
    }
    let dir_end = full[..n].iter().rposition(|&c| c == u16::from(b'\\')).map_or(0, |i| i + 1);
    full[dir_end] = 0;
    let empty = [0u16; 1];
    let info = CabinetInfo {
        path: full.as_ptr(),
        file: empty.as_ptr(),
        disk: empty.as_ptr(),
        set_id: cab.set_id,
        number: cab.index,
    };
    let r = callback(context, SPFILENOTIFY_CABINETINFO, (&raw const info) as usize, 0);
    if r != 0 {
        return Err(r);
    }

    let each = |f: &CabFile, bytes: &[u8]| -> Result<(), u32> {
        let name = wide_name(f)?;
        let mut info = FileInCabinet {
            name: name.as_ptr(),
            size: f.size,
            error: 0,
            date: f.date,
            time: f.time,
            attributes: f.attributes,
            target: [0; MAX_PATH],
        };
        let r = callback(
            context,
            SPFILENOTIFY_FILEINCABINET,
            (&raw mut info) as usize,
            cabinet as usize,
        );
        match r {
            FILEOP_DOIT => {}
            FILEOP_SKIP => return Ok(()),
            FILEOP_ABORT => {
                return Err(if info.error != 0 { info.error } else { ERROR_OPERATION_ABORTED });
            }
            other => {
                record(b"setupapi.dll!SetupIterateCabinetW(FILEINCABINET answer)", other.into());
                return Err(ERROR_INVALID_PARAMETER);
            }
        }
        info.target[MAX_PATH - 1] = 0;
        let error = write_target(&info.target, f, bytes);
        let paths = FilePaths { target: info.target.as_ptr(), source: cabinet, error, flags: 0 };
        let r = callback(context, SPFILENOTIFY_FILEEXTRACTED, (&raw const paths) as usize, 0);
        if r != 0 { Err(r) } else { Ok(()) }
    };
    cab.extract_all(each).map_err(|e| match e {
        ExtractError::Cab(e) => refuse(&e, data.len()),
        ExtractError::Caller(e) => e,
    })
}
