//! gala's wofutil.dll (wofapi.h): the Windows Overlay Filter's user-mode
//! API, which installers ask whether to store files compressed (WOF's
//! file provider, "CompactOS") or backed by a WIM. gala's volume has no
//! overlay filter: no file is externally backed and nothing is
//! compressed. Click-to-Run loads it while staging (ROADMAP OF).
//! - WofShouldCompressBinaries: FALSE ("FALSE if the system does not
//!   support compression on the specified volume");
//! - WofIsExternalFile: S_OK with FALSE (provider 0, length 0) for a file
//!   that exists, as for any physical file; the Win32 error as an HRESULT
//!   for one that does not;
//! - WofGetDriverVersion, WofSetFileDataLocation, WofEnumEntries,
//!   WofFileEnumFiles: HRESULT_FROM_WIN32(ERROR_INVALID_FUNCTION), what
//!   the documentation gives when no driver is attached for the provider,
//!   recorded (`wofutil.dll!<function>(no overlay filter)`).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

#[cfg(target_arch = "x86")]
use ntrt as _;

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn GetLastError() -> u32;
}

const S_OK: i32 = 0;
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;
const ERROR_INVALID_FUNCTION: u32 = 1;
const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;

/// HRESULT_FROM_WIN32.
fn from_win32(e: u32) -> i32 {
    if e == 0 { S_OK } else { (0x8007_0000 | (e & 0xffff)) as i32 }
}

/// Records a call answered "no overlay filter" (`wofutil.dll!<name>(no
/// overlay filter)`, kind other) and returns its HRESULT.
fn no_filter(name: &[u8]) -> i32 {
    let mut text = [0u8; 80];
    let mut n = 0;
    for part in [&b"wofutil.dll!"[..], name, b"(no overlay filter)"] {
        let take = part.len().min(text.len() - n);
        text[n..n + take].copy_from_slice(&part[..take]);
        n += take;
    }
    // SAFETY: a valid buffer; 5 is the ledger's "other".
    unsafe { GalaRecordMissing(text.as_ptr(), n, 5, 0) };
    from_win32(ERROR_INVALID_FUNCTION)
}

/// WofShouldCompressBinaries(Volume, *Algorithm): FALSE.
#[unsafe(no_mangle)]
pub extern "system" fn WofShouldCompressBinaries(_volume: *const u16, _algorithm: *mut u32) -> i32 {
    0
}

/// WofIsExternalFile(FilePath, *IsExternalFile, *Provider,
/// ExternalFileInfo, *BufferLength): a physical file (module comment).
#[unsafe(no_mangle)]
pub extern "system" fn WofIsExternalFile(
    path: *const u16,
    external: *mut i32,
    provider: *mut u32,
    _info: *mut u8,
    length: *mut u32,
) -> i32 {
    if path.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's NUL-terminated path.
    if unsafe { GetFileAttributesW(path) } == INVALID_FILE_ATTRIBUTES {
        // SAFETY: a plain call.
        return from_win32(unsafe { GetLastError() });
    }
    // SAFETY: the caller's optional outputs.
    unsafe {
        if !external.is_null() {
            *external = 0;
        }
        if !provider.is_null() {
            *provider = 0;
        }
        if !length.is_null() {
            *length = 0;
        }
    }
    S_OK
}

/// WofGetDriverVersion(FileOrVolumeHandle, Provider, *WofVersion).
#[unsafe(no_mangle)]
pub extern "system" fn WofGetDriverVersion(_h: usize, _provider: u32, _version: *mut u32) -> i32 {
    no_filter(b"WofGetDriverVersion")
}

/// WofSetFileDataLocation(FileHandle, Provider, ExternalFileInfo, Length).
#[unsafe(no_mangle)]
pub extern "system" fn WofSetFileDataLocation(
    _h: usize,
    _provider: u32,
    _info: *const u8,
    _length: u32,
) -> i32 {
    no_filter(b"WofSetFileDataLocation")
}

/// WofEnumEntries(VolumeName, Provider, EnumProc, UserData).
#[unsafe(no_mangle)]
pub extern "system" fn WofEnumEntries(
    _volume: *const u16,
    _provider: u32,
    _proc: usize,
    _data: usize,
) -> i32 {
    no_filter(b"WofEnumEntries")
}

/// WofFileEnumFiles(VolumeName, Algorithm, EnumProc, UserData).
#[unsafe(no_mangle)]
pub extern "system" fn WofFileEnumFiles(
    _volume: *const u16,
    _algorithm: u32,
    _proc: usize,
    _data: usize,
) -> i32 {
    no_filter(b"WofFileEnumFiles")
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
