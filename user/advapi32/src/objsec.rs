//! Object security (docs/SECURITY.md, phase S2): the descriptor functions
//! over the kernel's NtQuerySecurityObject and NtSetSecurityObject, which
//! keep a descriptor on every file, key and kernel object:
//! GetFileSecurity, SetFileSecurity, Get/SetKernelObjectSecurity,
//! GetSecurityInfo, SetSecurityInfo, GetNamedSecurityInfoW,
//! SetNamedSecurityInfoW (files and registry keys by name).
//!
//! TODO(S2+): SetSecurityInfo and SetNamedSecurityInfo of an unprotected
//! DACL should re-apply the parent's inheritable ACEs and propagate the
//! change to existing children, as Windows' advapi32 does.

use core::ffi::c_void;

use super::fail;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    pub(crate) fn NtQuerySecurityObject(
        h: usize,
        info: u32,
        sd: *mut u8,
        length: u32,
        needed: *mut u32,
    ) -> u32;
    pub(crate) fn NtSetSecurityObject(h: usize, info: u32, sd: *const u8) -> u32;
    fn NtClose(h: usize) -> u32;
    fn RtlNtStatusToDosError(status: u32) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *mut c_void,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> usize;
    fn LocalAlloc(flags: u32, bytes: usize) -> *mut u8;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
    fn GetLastError() -> u32;
    fn MultiByteToWideChar(
        page: u32,
        flags: u32,
        text: *const u8,
        length: i32,
        out: *mut u16,
        size: i32,
    ) -> i32;
}

const READ_CONTROL: u32 = 0x0002_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const WRITE_OWNER: u32 = 0x0008_0000;
const ACCESS_SYSTEM_SECURITY: u32 = 0x0100_0000;

const OWNER: u32 = 0x1;
const GROUP: u32 = 0x2;
const DACL: u32 = 0x4;
const SACL: u32 = 0x8;
const LABEL: u32 = 0x10;
const PROTECTED_DACL: u32 = 0x8000_0000;
const PROTECTED_SACL: u32 = 0x4000_0000;

const SE_DACL_PRESENT: u16 = 0x0004;
const SE_SACL_PRESENT: u16 = 0x0010;
const SE_DACL_PROTECTED: u16 = 0x1000;
const SE_SACL_PROTECTED: u16 = 0x2000;

const SE_FILE_OBJECT: u32 = 1;
const SE_REGISTRY_KEY: u32 = 4;

const STATUS_BUFFER_TOO_SMALL: u32 = 0xc000_0023;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const INVALID_HANDLE_VALUE: usize = usize::MAX;

fn dos(status: u32) -> u32 {
    // SAFETY: a plain call.
    unsafe { RtlNtStatusToDosError(status) }
}

fn result(status: u32) -> i32 {
    if status == 0 { 1 } else { fail(dos(status), 0) }
}

/// The handle access reading (`set` false) or changing `info` needs.
fn access_for(info: u32, set: bool) -> u32 {
    let mut a = 0;
    if info & (OWNER | GROUP | LABEL) != 0 {
        a |= if set { WRITE_OWNER } else { READ_CONTROL };
    }
    if info & DACL != 0 {
        a |= if set { WRITE_DAC } else { READ_CONTROL };
    }
    if info & SACL != 0 {
        a |= ACCESS_SYSTEM_SECURITY;
    }
    a
}

/// Opens a file or directory for its security (Err: the Win32 error).
fn open_file(path: *const u16, access: u32) -> Result<usize, u32> {
    // SAFETY: the caller's path; FILE_SHARE_ALL (7), OPEN_EXISTING (3),
    // FILE_FLAG_BACKUP_SEMANTICS (directories too) | FILE_FLAG_OVERLAPPED
    // (no SYNCHRONIZE: only the rights the security functions need).
    let h = unsafe { CreateFileW(path, access, 7, core::ptr::null_mut(), 3, 0x4200_0000, 0) };
    if h == INVALID_HANDLE_VALUE {
        // SAFETY: a plain call.
        return Err(unsafe { GetLastError() });
    }
    Ok(h)
}

/// An ANSI path as UTF-16 (Err: ERROR_INVALID_PARAMETER).
fn wide(path: *const u8, out: &mut [u16; 520]) -> Result<*const u16, u32> {
    if path.is_null() {
        return Err(ERROR_INVALID_PARAMETER);
    }
    // SAFETY: the caller's NUL-terminated string; our buffer (CP_ACP).
    let n = unsafe { MultiByteToWideChar(0, 0, path, -1, out.as_mut_ptr(), out.len() as i32) };
    if n == 0 { Err(ERROR_INVALID_PARAMETER) } else { Ok(out.as_ptr()) }
}

/// GetKernelObjectSecurity(Handle, RequestedInformation, pSecurityDescriptor,
/// nLength, lpnLengthNeeded)
#[unsafe(no_mangle)]
pub extern "system" fn GetKernelObjectSecurity(
    h: usize,
    info: u32,
    sd: *mut u8,
    length: u32,
    needed: *mut u32,
) -> i32 {
    // SAFETY: the caller's buffer of `length` and output.
    result(unsafe { NtQuerySecurityObject(h, info, sd, length, needed) })
}

/// SetKernelObjectSecurity(Handle, SecurityInformation, SecurityDescriptor)
#[unsafe(no_mangle)]
pub extern "system" fn SetKernelObjectSecurity(h: usize, info: u32, sd: *const u8) -> i32 {
    // SAFETY: the caller's descriptor.
    result(unsafe { NtSetSecurityObject(h, info, sd) })
}

/// GetFileSecurityW(lpFileName, RequestedInformation, pSecurityDescriptor,
/// nLength, lpnLengthNeeded): ERROR_INSUFFICIENT_BUFFER with the size when
/// it does not fit.
#[unsafe(no_mangle)]
pub extern "system" fn GetFileSecurityW(
    path: *const u16,
    info: u32,
    sd: *mut u8,
    length: u32,
    needed: *mut u32,
) -> i32 {
    let h = match open_file(path, access_for(info, false)) {
        Ok(h) => h,
        Err(e) => return fail(e, 0),
    };
    let mut size = 0;
    let needed = if needed.is_null() { &raw mut size } else { needed };
    let r = GetKernelObjectSecurity(h, info, sd, length, needed);
    // SAFETY: ours.
    unsafe { NtClose(h) };
    r
}

#[unsafe(no_mangle)]
pub extern "system" fn GetFileSecurityA(
    path: *const u8,
    info: u32,
    sd: *mut u8,
    length: u32,
    needed: *mut u32,
) -> i32 {
    let mut w = [0u16; 520];
    match wide(path, &mut w) {
        Ok(p) => GetFileSecurityW(p, info, sd, length, needed),
        Err(e) => fail(e, 0),
    }
}

/// SetFileSecurityW(lpFileName, SecurityInformation, pSecurityDescriptor)
#[unsafe(no_mangle)]
pub extern "system" fn SetFileSecurityW(path: *const u16, info: u32, sd: *const u8) -> i32 {
    let h = match open_file(path, access_for(info, true)) {
        Ok(h) => h,
        Err(e) => return fail(e, 0),
    };
    let r = SetKernelObjectSecurity(h, info, sd);
    // SAFETY: ours.
    unsafe { NtClose(h) };
    r
}

#[unsafe(no_mangle)]
pub extern "system" fn SetFileSecurityA(path: *const u8, info: u32, sd: *const u8) -> i32 {
    let mut w = [0u16; 520];
    match wide(path, &mut w) {
        Ok(p) => SetFileSecurityW(p, info, sd),
        Err(e) => fail(e, 0),
    }
}

/// The handle GetSecurityInfo / SetSecurityInfo use for `handle` of
/// `kind` (a predefined registry key resolves to its NT handle).
fn object_handle(handle: usize, kind: u32) -> Result<usize, u32> {
    if handle == 0 {
        return Err(6); // ERROR_INVALID_HANDLE
    }
    if kind == SE_REGISTRY_KEY {
        return super::registry::resolve(handle).map_err(|e| e as u32);
    }
    Ok(handle)
}

/// The descriptor of `h` (the parts `info` asks for), LocalAlloc'd.
fn query(h: usize, info: u32) -> Result<*mut u8, u32> {
    let mut size = 0;
    // SAFETY: a size query.
    let status = unsafe { NtQuerySecurityObject(h, info, core::ptr::null_mut(), 0, &mut size) };
    if status != STATUS_BUFFER_TOO_SMALL && status != 0 {
        return Err(dos(status));
    }
    // SAFETY: a plain allocation (LMEM_ZEROINIT).
    let p = unsafe { LocalAlloc(0x40, size as usize) };
    if p.is_null() {
        return Err(ERROR_NOT_ENOUGH_MEMORY);
    }
    // SAFETY: our buffer of `size`.
    let status = unsafe { NtQuerySecurityObject(h, info, p, size, &mut size) };
    if status != 0 {
        // SAFETY: ours.
        unsafe { LocalFree(p.cast()) };
        return Err(dos(status));
    }
    Ok(p)
}

/// The parts of a self-relative descriptor at `sd` into the caller's
/// outputs (null where absent or not asked for).
fn parts(sd: *mut u8, info: u32, outs: [*mut *mut u8; 4]) {
    let at = |field: usize| {
        // SAFETY: a self-relative descriptor from the kernel.
        let off = unsafe { sd.add(field).cast::<u32>().read_unaligned() } as usize;
        if off == 0 { core::ptr::null_mut() } else { sd.wrapping_add(off) }
    };
    for (i, (bit, field)) in
        [(OWNER, 4), (GROUP, 8), (DACL, 16), (SACL, 12)].into_iter().enumerate()
    {
        if !outs[i].is_null() && info & bit != 0 {
            // SAFETY: the caller's output.
            unsafe { *outs[i] = at(field) };
        }
    }
}

/// GetSecurityInfo(handle, ObjectType, SecurityInfo, *ppsidOwner,
/// *ppsidGroup, *ppDacl, *ppSacl, *ppSecurityDescriptor): the parts point
/// into the descriptor, LocalAlloc'd for the caller to free. Returns the
/// error itself.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn GetSecurityInfo(
    handle: usize,
    kind: u32,
    info: u32,
    owner: *mut *mut u8,
    group: *mut *mut u8,
    dacl: *mut *mut u8,
    sacl: *mut *mut u8,
    descriptor: *mut *mut u8,
) -> u32 {
    let h = match object_handle(handle, kind) {
        Ok(h) => h,
        Err(e) => return e,
    };
    let sd = match query(h, info & 0x1f) {
        Ok(p) => p,
        Err(e) => return e,
    };
    parts(sd, info, [owner, group, dacl, sacl]);
    if descriptor.is_null() {
        // Nothing will free it; the parts stay valid (as documented, the
        // caller should ask for the descriptor when it asks for parts).
    } else {
        // SAFETY: the caller's output.
        unsafe { *descriptor = sd };
    }
    0
}

/// Opens an object named as GetNamedSecurityInfo/SetNamedSecurityInfo
/// name them: files by path, registry keys as `CLASSES_ROOT`,
/// `CURRENT_USER`, `MACHINE` or `USERS` and a subkey path.
fn open_named(name: *const u16, kind: u32, access: u32) -> Result<usize, u32> {
    if name.is_null() {
        return Err(ERROR_INVALID_PARAMETER);
    }
    match kind {
        SE_FILE_OBJECT => open_file(name, access),
        SE_REGISTRY_KEY => super::registry::open_named(name, access).map_err(|e| e as u32),
        _ => {
            super::missing(b"advapi32.dll!NamedSecurityInfo(object type)");
            Err(ERROR_INVALID_PARAMETER)
        }
    }
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn GetNamedSecurityInfoW(
    name: *const u16,
    kind: u32,
    info: u32,
    owner: *mut *mut u8,
    group: *mut *mut u8,
    dacl: *mut *mut u8,
    sacl: *mut *mut u8,
    descriptor: *mut *mut u8,
) -> u32 {
    let h = match open_named(name, kind, access_for(info, false)) {
        Ok(h) => h,
        Err(e) => return e,
    };
    let r = match query(h, info & 0x1f) {
        Ok(sd) => {
            parts(sd, info, [owner, group, dacl, sacl]);
            if !descriptor.is_null() {
                // SAFETY: the caller's output.
                unsafe { *descriptor = sd };
            }
            0
        }
        Err(e) => e,
    };
    // SAFETY: ours.
    unsafe { NtClose(h) };
    r
}

/// An absolute descriptor (x64 layout) with the parts SetSecurityInfo was
/// given.
#[repr(C)]
struct Absolute {
    revision: u8,
    sbz1: u8,
    control: u16,
    owner: *const u8,
    group: *const u8,
    sacl: *const u8,
    dacl: *const u8,
}

fn set_parts(
    h: usize,
    info: u32,
    owner: *const u8,
    group: *const u8,
    dacl: *const u8,
    sacl: *const u8,
) -> u32 {
    let mut control = 0;
    if info & DACL != 0 {
        control |= SE_DACL_PRESENT;
        if info & PROTECTED_DACL != 0 {
            control |= SE_DACL_PROTECTED;
        }
    }
    if info & SACL != 0 {
        control |= SE_SACL_PRESENT;
        if info & PROTECTED_SACL != 0 {
            control |= SE_SACL_PROTECTED;
        }
    }
    let sd = Absolute { revision: 1, sbz1: 0, control, owner, group, sacl, dacl };
    // SAFETY: our descriptor pointing at the caller's parts.
    match unsafe { NtSetSecurityObject(h, info, (&raw const sd).cast()) } {
        0 => 0,
        status => dos(status),
    }
}

/// SetSecurityInfo(handle, ObjectType, SecurityInfo, psidOwner,
/// psidGroup, pDacl, pSacl): returns the error itself.
#[unsafe(no_mangle)]
pub extern "system" fn SetSecurityInfo(
    handle: usize,
    kind: u32,
    info: u32,
    owner: *const u8,
    group: *const u8,
    dacl: *const u8,
    sacl: *const u8,
) -> u32 {
    match object_handle(handle, kind) {
        Ok(h) => set_parts(h, info, owner, group, dacl, sacl),
        Err(e) => e,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn SetNamedSecurityInfoW(
    name: *const u16,
    kind: u32,
    info: u32,
    owner: *const u8,
    group: *const u8,
    dacl: *const u8,
    sacl: *const u8,
) -> u32 {
    let h = match open_named(name, kind, access_for(info, true)) {
        Ok(h) => h,
        Err(e) => return e,
    };
    let r = set_parts(h, info, owner, group, dacl, sacl);
    // SAFETY: ours.
    unsafe { NtClose(h) };
    r
}
