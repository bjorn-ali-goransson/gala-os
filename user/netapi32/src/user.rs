//! NetUserGetInfo (lmaccess.h) over the local account database: the
//! accounts advapi32's LookupAccountNameW/LookupAccountSidW know (the boot
//! user `gala` and the users under HKLM\SAM\Gala\Users). Chromium's
//! password manager reads the logged-on user's password age with it.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LookupAccountNameW(
        system: *const u16,
        name: *const u16,
        sid: *mut u8,
        sid_size: *mut u32,
        domain: *mut u16,
        domain_size: *mut u32,
        kind: *mut u32,
    ) -> i32;
    fn LookupAccountSidW(
        system: *const u16,
        sid: *const u8,
        name: *mut u16,
        name_size: *mut u32,
        domain: *mut u16,
        domain_size: *mut u32,
        kind: *mut u32,
    ) -> i32;
}

const NERR_SUCCESS: u32 = 0;
const NERR_USER_NOT_FOUND: u32 = 2221;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_LEVEL: u32 = 124;
const ERROR_BAD_NETPATH: u32 = 53;
const SID_TYPE_USER: u32 = 1;
/// gala's accounts are all standard users (the boot user is one; the
/// database keeps no alias memberships yet).
const USER_PRIV_USER: u32 = 1;
const UF_SCRIPT: u32 = 0x1;
const UF_NORMAL_ACCOUNT: u32 = 0x200;

/// USER_INFO_1; level 0 (USER_INFO_0) is its first field.
#[repr(C)]
struct UserInfo1 {
    name: *mut u16,
    password: *mut u16,
    password_age: u32,
    privilege: u32,
    home_dir: *mut u16,
    comment: *mut u16,
    flags: u32,
    script_path: *mut u16,
}

/// The stored name of user account `name` (its case), if it is one.
fn account(name: *const u16, out: &mut [u16; 256]) -> Option<usize> {
    let (mut sid, mut domain) = ([0u8; 68], [0u16; 64]);
    let (mut sid_size, mut domain_size, mut kind) = (68u32, 64u32, 0u32);
    // SAFETY: our buffers and their sizes; `name` is the caller's string.
    let found = unsafe {
        LookupAccountNameW(
            core::ptr::null(),
            name,
            sid.as_mut_ptr(),
            &mut sid_size,
            domain.as_mut_ptr(),
            &mut domain_size,
            &mut kind,
        )
    };
    if found == 0 || kind != SID_TYPE_USER {
        return None;
    }
    let (mut size, mut domain_size) = (out.len() as u32, 64u32);
    // SAFETY: as above.
    let named = unsafe {
        LookupAccountSidW(
            core::ptr::null(),
            sid.as_ptr(),
            out.as_mut_ptr(),
            &mut size,
            domain.as_mut_ptr(),
            &mut domain_size,
            &mut kind,
        )
    };
    (named != 0).then_some(size as usize)
}

/// NetUserGetInfo(servername, username, level, bufptr): levels 0
/// (USER_INFO_0: the name) and 1 (USER_INFO_1: no password, its age 0 as
/// gala keeps no change times, USER_PRIV_USER, UF_SCRIPT |
/// UF_NORMAL_ACCOUNT, empty home directory, comment and script path) of a
/// local user account, in a buffer for NetApiBufferFree.
/// NERR_UserNotFound for a name that is not a user's; only the local
/// machine (a null or empty server name).
#[unsafe(no_mangle)]
pub extern "system" fn NetUserGetInfo(
    server: *const u16,
    user: *const u16,
    level: u32,
    out: *mut *mut u8,
) -> u32 {
    if out.is_null() || user.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    // SAFETY: the caller's server name, if any.
    if !server.is_null() && unsafe { *server } != 0 {
        return ERROR_BAD_NETPATH;
    }
    if !matches!(level, 0 | 1) {
        crate::unsupported(b"netapi32.dll!NetUserGetInfo(level)", level.into());
        return ERROR_INVALID_LEVEL;
    }
    let mut name = [0u16; 256];
    let Some(len) = account(user, &mut name) else { return NERR_USER_NOT_FOUND };
    let header = if level == 0 { 8 } else { size_of::<UserInfo1>() };
    // The name, then one empty string the other fields share.
    let total = header + 2 * (len + 1) + 2;
    // SAFETY: plain heap allocation.
    let block = unsafe { HeapAlloc(GetProcessHeap(), 0, total) };
    if block.is_null() {
        return ERROR_NOT_ENOUGH_MEMORY;
    }
    // SAFETY: inside the block just allocated, u16-aligned (the header's
    // size is a multiple of 8); the caller's output pointer.
    unsafe {
        let text = block.add(header).cast::<u16>();
        core::ptr::copy_nonoverlapping(name.as_ptr(), text, len);
        text.add(len).write(0);
        let empty = text.add(len + 1);
        empty.write(0);
        if level == 0 {
            block.cast::<*mut u16>().write(text);
        } else {
            block.cast::<UserInfo1>().write(UserInfo1 {
                name: text,
                password: core::ptr::null_mut(),
                password_age: 0,
                privilege: USER_PRIV_USER,
                home_dir: empty,
                comment: empty,
                flags: UF_SCRIPT | UF_NORMAL_ACCOUNT,
                script_path: empty,
            });
        }
        out.write(block);
    }
    NERR_SUCCESS
}
