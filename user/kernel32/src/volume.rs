//! Drives and volumes: gala has one, C: (a fixed disk). Volume GUID
//! enumeration and mount points are not there yet (they fail as when no
//! volume is found, and are recorded).

use super::*;

const DRIVE_NO_ROOT_DIR: u32 = 1;
const DRIVE_FIXED: u32 = 3;
const ERROR_NO_MORE_FILES: u32 = 18;

/// GetLogicalDrives: bit n is drive 'A' + n; only C:.
#[unsafe(no_mangle)]
pub extern "system" fn GetLogicalDrives() -> u32 {
    1 << 2
}

/// GetDriveTypeA: C: (or the current drive, for null) is fixed.
#[unsafe(no_mangle)]
pub extern "system" fn GetDriveTypeA(root: *const u8) -> u32 {
    if root.is_null() {
        return DRIVE_FIXED;
    }
    // SAFETY: the caller's NUL-terminated string.
    let (c0, c1) = unsafe { (*root, if *root == 0 { 0 } else { *root.add(1) }) };
    if c0 & !0x20 == b'C' && c1 == b':' { DRIVE_FIXED } else { DRIVE_NO_ROOT_DIR }
}

/// GetVolumeInformationW: of the volume with root `root` (C:\ or null),
/// as GetVolumeInformationByHandleW reports it.
#[unsafe(no_mangle)]
pub extern "system" fn GetVolumeInformationW(
    root: *const u16,
    name: *mut u16,
    name_size: u32,
    serial: *mut u32,
    max_component: *mut u32,
    flags: *mut u32,
    fs_name: *mut u16,
    fs_name_size: u32,
) -> Bool {
    let c_root = [u16::from(b'C'), u16::from(b':'), u16::from(b'\\'), 0];
    let root = if root.is_null() { c_root.as_ptr() } else { root };
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let h = CreateFileW(
        root,
        0,
        7,
        core::ptr::null_mut(),
        3,
        FILE_FLAG_BACKUP_SEMANTICS,
        core::ptr::null_mut(),
    );
    if h == INVALID_HANDLE_VALUE {
        return FALSE;
    }
    let ok = GetVolumeInformationByHandleW(
        h,
        name,
        name_size,
        serial,
        max_component,
        flags,
        fs_name,
        fs_name_size,
    );
    CloseHandle(h);
    ok
}

/// GetVolumeInformationA: the W function, names converted from UTF-16.
#[unsafe(no_mangle)]
pub extern "system" fn GetVolumeInformationA(
    root: *const u8,
    name: *mut u8,
    name_size: u32,
    serial: *mut u32,
    max_component: *mut u32,
    flags: *mut u32,
    fs_name: *mut u8,
    fs_name_size: u32,
) -> Bool {
    let mut wroot = [0u16; 64];
    let root = if root.is_null() {
        core::ptr::null()
    } else {
        match super::sync::ansi_to_wide(root, &mut wroot[..63]) {
            Some(n) => {
                wroot[n] = 0;
                wroot.as_ptr()
            }
            None => return fail_with(ERROR_INVALID_PARAMETER, FALSE),
        }
    };
    let (mut wname, mut wfs) = ([0u16; 261], [0u16; 261]);
    let ok = GetVolumeInformationW(
        root,
        wname.as_mut_ptr(),
        261,
        serial,
        max_component,
        flags,
        wfs.as_mut_ptr(),
        261,
    );
    if ok == FALSE {
        return FALSE;
    }
    for (w, out, size) in [(&wname, name, name_size), (&wfs, fs_name, fs_name_size)] {
        if out.is_null() || size == 0 {
            continue;
        }
        let n = w.iter().position(|&c| c == 0).unwrap_or(0);
        let len = if n == 0 {
            0
        } else {
            super::text::WideCharToMultiByte(
                65001,
                0,
                w.as_ptr(),
                n as i32,
                out,
                size as i32 - 1,
                core::ptr::null(),
                core::ptr::null_mut(),
            )
        };
        if len < 0 || (n > 0 && len == 0) {
            return FALSE;
        }
        // SAFETY: len < size bytes were written.
        unsafe { *out.add(len as usize) = 0 };
    }
    TRUE
}

/// Volume GUID paths and mount points: not yet.
macro_rules! no_volume_names {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $ret:ty = $fail:expr;)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "system" fn $name($($arg: $ty),*) -> $ret {
            $(let _ = $arg;)*
            unsupported(concat!("kernel32.dll!", stringify!($name)).as_bytes(), 0);
            fail_with(ERROR_NO_MORE_FILES, $fail)
        }
    )*};
}

no_volume_names! {
    FindFirstVolumeA(name: *mut u8, size: u32) -> Handle = INVALID_HANDLE_VALUE;
    FindFirstVolumeW(name: *mut u16, size: u32) -> Handle = INVALID_HANDLE_VALUE;
    FindNextVolumeA(find: Handle, name: *mut u8, size: u32) -> Bool = FALSE;
    FindNextVolumeW(find: Handle, name: *mut u16, size: u32) -> Bool = FALSE;
    GetVolumeNameForVolumeMountPointA(mount: *const u8, name: *mut u8, size: u32) -> Bool = FALSE;
    GetVolumePathNamesForVolumeNameA(volume: *const u8, names: *mut u8, size: u32, length: *mut u32) -> Bool = FALSE;
}

#[unsafe(no_mangle)]
pub extern "system" fn FindVolumeClose(_find: Handle) -> Bool {
    TRUE
}
