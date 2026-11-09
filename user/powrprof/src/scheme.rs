//! The platform role and power scheme queries (powrprof.h, the API sets
//! api-ms-win-power-base/-setting that Chromium imports): gala is a
//! desktop on AC power with Windows' Balanced scheme active, whose
//! settings gala does not keep (every read reports the setting absent).

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn LocalAlloc(flags: u32, bytes: usize) -> *mut u8;
}

const ERROR_SUCCESS: u32 = 0;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_FILE_NOT_FOUND: u32 = 2;

/// PowerDeterminePlatformRoleEx(Version): PlatformRoleDesktop (1).
#[unsafe(no_mangle)]
pub extern "system" fn PowerDeterminePlatformRoleEx(version: u32) -> u32 {
    // POWER_PLATFORM_ROLE_V1 (1) and V2 (2) both know Desktop.
    if version == 0 || version > 2 {
        return 0; // PlatformRoleUnspecified
    }
    1
}

/// PowerDeterminePlatformRole: PlatformRoleDesktop.
#[unsafe(no_mangle)]
pub extern "system" fn PowerDeterminePlatformRole() -> u32 {
    1
}

/// GUID_TYPICAL_POWER_SAVINGS (Balanced):
/// 381b4222-f694-41f0-9685-ff5bb260df2e.
const BALANCED: [u8; 16] = [
    0x22, 0x42, 0x1b, 0x38, 0x94, 0xf6, 0xf0, 0x41, 0x96, 0x85, 0xff, 0x5b, 0xb2, 0x60, 0xdf, 0x2e,
];

/// PowerGetActiveScheme(UserRootPowerKey, *ActivePolicyGuid): Balanced, in
/// a LocalAlloc'd GUID the caller frees with LocalFree.
#[unsafe(no_mangle)]
pub extern "system" fn PowerGetActiveScheme(_root: usize, out: *mut *mut u8) -> u32 {
    if out.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    // SAFETY: a plain allocation of 16 bytes.
    let guid = unsafe { LocalAlloc(0, 16) };
    if guid.is_null() {
        return ERROR_NOT_ENOUGH_MEMORY;
    }
    // SAFETY: the block just allocated; the caller's output.
    unsafe {
        core::ptr::copy_nonoverlapping(BALANCED.as_ptr(), guid, 16);
        *out = guid;
    }
    ERROR_SUCCESS
}

/// PowerReadACValue / PowerReadDCValue(RootPowerKey, SchemeGuid,
/// SubGroupOfPowerSettingsGuid, PowerSettingGuid, Type, Buffer,
/// BufferSize): gala keeps no power settings (ERROR_FILE_NOT_FOUND, as for
/// a setting the scheme does not define).
#[unsafe(no_mangle)]
pub extern "system" fn PowerReadACValue(
    _root: usize,
    _scheme: *const u8,
    _subgroup: *const u8,
    _setting: *const u8,
    _kind: *mut u32,
    _buffer: *mut u8,
    _size: *mut u32,
) -> u32 {
    ERROR_FILE_NOT_FOUND
}

#[unsafe(no_mangle)]
pub extern "system" fn PowerReadDCValue(
    root: usize,
    scheme: *const u8,
    subgroup: *const u8,
    setting: *const u8,
    kind: *mut u32,
    buffer: *mut u8,
    size: *mut u32,
) -> u32 {
    PowerReadACValue(root, scheme, subgroup, setting, kind, buffer, size)
}
