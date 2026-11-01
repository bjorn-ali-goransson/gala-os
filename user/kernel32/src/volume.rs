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

/// GetDriveTypeW: as GetDriveTypeA.
#[unsafe(no_mangle)]
pub extern "system" fn GetDriveTypeW(root: *const u16) -> u32 {
    if root.is_null() {
        return DRIVE_FIXED;
    }
    // SAFETY: the caller's NUL-terminated string.
    let (c0, c1) = unsafe { (*root, if *root == 0 { 0 } else { *root.add(1) }) };
    if c0 & !0x20 == u16::from(b'C') && c1 == u16::from(b':') {
        DRIVE_FIXED
    } else {
        DRIVE_NO_ROOT_DIR
    }
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

/// The computer's name (ws2_32's gethostname agrees): NetBIOS names are
/// upper case, DNS host names as given; there is no DNS domain.
const HOST_NAME: &[u8] = b"gala";
const ERROR_MORE_DATA: u32 = 234;

/// COMPUTER_NAME_FORMAT: 0 NetBIOS, 1 DnsHostname, 2 DnsDomain,
/// 3 DnsFullyQualified, 4-7 the Physical variants of the same; the name
/// for `kind` as bytes (None: an invalid kind).
fn computer_name(kind: u32, out: &mut [u8; 16]) -> Option<usize> {
    let name: &[u8] = match kind % 4 {
        2 => b"",
        _ => HOST_NAME,
    };
    if kind > 7 {
        return None;
    }
    for (o, &c) in out.iter_mut().zip(name) {
        *o = if kind.is_multiple_of(4) { c.to_ascii_uppercase() } else { c };
    }
    Some(name.len())
}

/// Copies the name into the caller's buffer of `*size` characters: the
/// length without the NUL, or ERROR_MORE_DATA with `*size` the size needed.
fn put_name<T: From<u8> + Copy>(name: &[u8], buf: *mut T, size: *mut u32) -> Bool {
    if size.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's size pointer and buffer of `*size` characters.
    unsafe {
        if buf.is_null() || (*size as usize) <= name.len() {
            *size = name.len() as u32 + 1;
            return fail_with(ERROR_MORE_DATA, FALSE);
        }
        for (i, &c) in name.iter().enumerate() {
            *buf.add(i) = T::from(c);
        }
        *buf.add(name.len()) = T::from(0);
        *size = name.len() as u32;
    }
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn GetComputerNameExW(kind: u32, buf: *mut u16, size: *mut u32) -> Bool {
    let mut name = [0u8; 16];
    let Some(n) = computer_name(kind, &mut name) else {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    };
    put_name(&name[..n], buf, size)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetComputerNameExA(kind: u32, buf: *mut u8, size: *mut u32) -> Bool {
    let mut name = [0u8; 16];
    let Some(n) = computer_name(kind, &mut name) else {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    };
    put_name(&name[..n], buf, size)
}

/// GetComputerNameW / A: the NetBIOS name.
#[unsafe(no_mangle)]
pub extern "system" fn GetComputerNameW(buf: *mut u16, size: *mut u32) -> Bool {
    GetComputerNameExW(0, buf, size)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetComputerNameA(buf: *mut u8, size: *mut u32) -> Bool {
    GetComputerNameExA(0, buf, size)
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtOpenSymbolicLinkObject(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
    ) -> u32;
    fn NtQuerySymbolicLinkObject(
        handle: Handle,
        target: *mut UnicodeString,
        returned: *mut u32,
    ) -> u32;
}

/// QueryDosDeviceW: the target of the DOS device name `name` (the symbolic
/// link `\??\name`, such as `\Device\HarddiskVolume1` for C:), followed by
/// two NULs; returns the characters stored, both NULs included. Listing
/// every name (a null `name`) is not supported yet.
#[unsafe(no_mangle)]
pub extern "system" fn QueryDosDeviceW(name: *const u16, target: *mut u16, max: u32) -> u32 {
    if name.is_null() {
        unsupported(b"kernel32.dll!QueryDosDeviceW(all names)", 0);
        return fail_with(ERROR_NOT_SUPPORTED, 0);
    }
    let mut path = [0u16; 264];
    let prefix = b"\\??\\".map(u16::from);
    path[..4].copy_from_slice(&prefix);
    let mut n = 4;
    // SAFETY: the caller's NUL-terminated name, read up to its NUL.
    while unsafe { *name.add(n - 4) } != 0 {
        if n == path.len() {
            return fail_with(2, 0); // ERROR_FILE_NOT_FOUND
        }
        path[n] = unsafe { *name.add(n - 4) };
        n += 1;
    }
    let us = UnicodeString {
        length: (2 * n) as u16,
        maximum_length: (2 * n) as u16,
        buffer: path.as_ptr().cast(),
    };
    let attributes = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root_directory: 0,
        object_name: &us,
        attributes: 0x40, // OBJ_CASE_INSENSITIVE
        security_descriptor: 0,
        security_quality_of_service: 0,
    };
    let mut link: Handle = core::ptr::null_mut();
    // SAFETY: our attributes and output handle; 1 = SYMBOLIC_LINK_QUERY.
    let status = unsafe { NtOpenSymbolicLinkObject(&mut link, 1, &attributes) };
    if status != STATUS_SUCCESS {
        return fail(status, 0);
    }
    let mut buf = [0u16; 512];
    let mut out = UnicodeString {
        length: 0,
        maximum_length: (2 * buf.len()) as u16,
        buffer: buf.as_mut_ptr().cast(),
    };
    // SAFETY: our handle and buffer.
    let status = unsafe { NtQuerySymbolicLinkObject(link, &mut out, core::ptr::null_mut()) };
    // SAFETY: the handle just opened.
    unsafe { NtClose(link) };
    if status != STATUS_SUCCESS {
        return fail(status, 0);
    }
    let len = usize::from(out.length) / 2;
    if len + 2 > max as usize || target.is_null() {
        return fail_with(122, 0); // ERROR_INSUFFICIENT_BUFFER
    }
    // SAFETY: the caller's buffer of `max` characters (len + 2 fit).
    unsafe {
        core::ptr::copy_nonoverlapping(buf.as_ptr(), target, len);
        target.add(len).write(0);
        target.add(len + 1).write(0);
    }
    (len + 2) as u32
}

/// The GUID path of gala's one volume (C:), as the mount manager names
/// volumes: `\\?\Volume{GUID}\`. The GUID is gala's own, fixed.
pub(crate) const VOLUME_GUID_PATH: &[u8] = b"\\\\?\\Volume{9a1a0c00-0000-4000-8000-00000000c0de}\\";

/// GetVolumeNameForVolumeMountPointW: the volume GUID path of the
/// volume mounted at `mount` (a root with its trailing backslash; only
/// C:\ here).
#[unsafe(no_mangle)]
pub extern "system" fn GetVolumeNameForVolumeMountPointW(
    mount: *const u16,
    name: *mut u16,
    size: u32,
) -> Bool {
    if mount.is_null() || name.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's NUL-terminated string (read up to 4 units).
    let given: [u16; 4] = unsafe {
        let mut g = [0u16; 4];
        for (i, slot) in g.iter_mut().enumerate() {
            *slot = *mount.add(i);
            if *slot == 0 {
                break;
            }
        }
        g
    };
    let upper = |c: u16| if (0x61..=0x7a).contains(&c) { c - 32 } else { c };
    if upper(given[0]) != u16::from(b'C') || given[1] != u16::from(b':') {
        return fail_with(2, 0); // ERROR_FILE_NOT_FOUND: no such volume
    }
    if given[2] != u16::from(b'\\') || given[3] != 0 {
        return fail_with(123, 0); // ERROR_INVALID_NAME: not a root
    }
    let n = VOLUME_GUID_PATH.len();
    if (size as usize) < n + 1 {
        return fail_with(206, 0); // ERROR_FILENAME_EXCED_RANGE
    }
    // SAFETY: the caller's buffer of `size` > n characters.
    unsafe {
        for (i, &b) in VOLUME_GUID_PATH.iter().enumerate() {
            name.add(i).write(b.into());
        }
        name.add(n).write(0);
    }
    1
}
