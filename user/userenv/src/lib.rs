//! gala's userenv.dll (user profiles and environment blocks):
//! GetUserProfileDirectoryW; the other functions programs import are not
//! implemented yet. Each of those reports itself to the missing-features
//! ledger and ends the process when called (`gala_stubs`), so the ledger
//! shows which ones a program uses.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod unimplemented;

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    fn __chkstk();
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(code: u32);
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn GetCurrentProcess() -> usize;
    fn CloseHandle(h: usize) -> i32;
    fn CreateDirectoryW(path: *const u16, sa: *const SecurityAttributes) -> i32;
    fn GetLastError() -> u32;
    fn LocalFree(p: *mut u8) -> *mut u8;
}

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut u8,
    inherit: i32,
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn ConvertSidToStringSidW(sid: *const u8, text: *mut *mut u16) -> i32;
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        text: *const u16,
        revision: u32,
        sd: *mut *mut u8,
        size: *mut u32,
    ) -> i32;
    fn RegCreateKeyExW(
        key: usize,
        sub: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        access: u32,
        sa: *const SecurityAttributes,
        out: *mut usize,
        disposition: *mut u32,
    ) -> i32;
    fn RegSetValueExW(
        key: usize,
        name: *const u16,
        reserved: u32,
        kind: u32,
        data: *const u8,
        size: u32,
    ) -> i32;
    fn RegGetValueW(
        key: usize,
        sub: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: *mut u8,
        size: *mut u32,
    ) -> i32;
    fn RegCloseKey(key: usize) -> i32;
    fn GetTokenInformation(token: usize, class: u32, out: *mut u8, len: u32, ret: *mut u32) -> i32;
    fn OpenProcessToken(process: usize, access: u32, token: *mut usize) -> i32;
    fn EqualSid(a: *const u8, b: *const u8) -> i32;
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

const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_INVALID_PARAMETER: u32 = 87;

fn fail(error: u32) -> i32 {
    // SAFETY: a plain call.
    unsafe { SetLastError(error) };
    0
}

#[repr(C, align(8))]
struct Buf([u8; 256]);

/// TokenUser's SID of `token`, in `b`.
fn user_of(token: usize, b: &mut Buf) -> Option<*const u8> {
    let mut n = 0;
    // SAFETY: our buffer; TOKEN_USER starts with the SID pointer.
    unsafe {
        if GetTokenInformation(token, 1, b.0.as_mut_ptr(), 256, &mut n) == 0 {
            return None;
        }
        Some(b.0.as_ptr().cast::<*const u8>().read())
    }
}

/// GetUserProfileDirectoryW(hToken, lpProfileDir, lpcchSize): the
/// profile directory of the token's user: the process's own user's is
/// %USERPROFILE%, another local user's C:\Users\<name>. ERROR_INSUFFICIENT_BUFFER
/// with the size (characters, with the NUL) when it does not fit.
/// TODO(S4): profiles created at first logon (LoadUserProfileW), the
/// ProfileList registry key.
#[unsafe(no_mangle)]
pub extern "system" fn GetUserProfileDirectoryW(
    token: usize,
    dir: *mut u16,
    size: *mut u32,
) -> i32 {
    if size.is_null() {
        return fail(ERROR_INVALID_PARAMETER);
    }
    let (mut b1, mut b2) = (Buf([0; 256]), Buf([0; 256]));
    let Some(user) = user_of(token, &mut b1) else { return 0 };
    let mut own = 0;
    // SAFETY: plain calls; TOKEN_QUERY.
    let mine = unsafe {
        if OpenProcessToken(GetCurrentProcess(), 8, &mut own) == 0 {
            return 0;
        }
        let sid = user_of(own, &mut b2);
        CloseHandle(own);
        sid.is_some_and(|s| EqualSid(s, user) != 0)
    };
    let mut path = [0u16; 300];
    let len = if mine {
        let name = b"USERPROFILE\0".map(u16::from);
        // SAFETY: our buffers.
        let n = unsafe { GetEnvironmentVariableW(name.as_ptr(), path.as_mut_ptr(), 300) } as usize;
        if n == 0 || n >= 300 {
            return fail(ERROR_FILE_NOT_FOUND);
        }
        n
    } else if let Some(n) = listed_profile(user, &mut path) {
        n
    } else {
        let prefix = b"C:\\Users\\".map(u16::from);
        path[..prefix.len()].copy_from_slice(&prefix);
        let (mut name_size, mut domain_size, mut kind) = (200u32, 64u32, 0u32);
        let mut domain = [0u16; 64];
        // SAFETY: our buffers of those sizes.
        let found = unsafe {
            LookupAccountSidW(
                core::ptr::null(),
                user,
                path[prefix.len()..].as_mut_ptr(),
                &mut name_size,
                domain.as_mut_ptr(),
                &mut domain_size,
                &mut kind,
            )
        };
        if found == 0 {
            return fail(ERROR_FILE_NOT_FOUND);
        }
        prefix.len() + name_size as usize
    };
    // SAFETY: the caller's size and buffer of that many characters.
    unsafe {
        let room = *size as usize;
        *size = (len + 1) as u32;
        if dir.is_null() || room < len + 1 {
            return fail(ERROR_INSUFFICIENT_BUFFER);
        }
        core::ptr::copy_nonoverlapping(path.as_ptr(), dir, len);
        *dir.add(len) = 0;
    }
    1
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

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

const PROFILE_LIST: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\ProfileList\\";
const HKEY_LOCAL_MACHINE: usize = 0x8000_0002;
const HKEY_USERS: usize = 0x8000_0003;
const KEY_ALL_ACCESS: u32 = 0x000f_003f;
const REG_EXPAND_SZ: u32 = 2;
const ERROR_PRIVILEGE_NOT_HELD: u32 = 1314;
const ERROR_ALREADY_EXISTS: u32 = 183;

/// A UTF-16 string builder (NUL-terminated as it goes).
struct W {
    b: [u16; 400],
    n: usize,
}

impl W {
    fn new() -> W {
        W { b: [0; 400], n: 0 }
    }

    fn s(mut self, s: &str) -> W {
        for c in s.encode_utf16() {
            if self.n + 1 < self.b.len() {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
        self
    }

    fn w(mut self, p: *const u16) -> W {
        let mut i = 0;
        // SAFETY: a NUL-terminated string.
        while unsafe { *p.add(i) } != 0 && self.n + 1 < self.b.len() {
            // SAFETY: as above.
            self.b[self.n] = unsafe { *p.add(i) };
            self.n += 1;
            i += 1;
        }
        self
    }

    fn p(&self) -> *const u16 {
        self.b.as_ptr()
    }
}

/// The ProfileList entry of `user` (ProfileImagePath) into `path`: its length.
fn listed_profile(user: *const u8, path: &mut [u16; 300]) -> Option<usize> {
    let mut text = core::ptr::null_mut();
    // SAFETY: the token's SID; our output.
    if unsafe { ConvertSidToStringSidW(user, &mut text) } == 0 {
        return None;
    }
    let key = W::new().s(PROFILE_LIST).w(text);
    // SAFETY: from ConvertSidToStringSidW.
    unsafe { LocalFree(text.cast()) };
    let value = W::new().s("ProfileImagePath");
    let mut size = (2 * path.len()) as u32;
    // SAFETY: our strings and buffer; RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ |
    // RRF_NOEXPAND.
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.p(),
            value.p(),
            0x1000_0006,
            core::ptr::null_mut(),
            path.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if r != 0 || size < 2 {
        return None;
    }
    Some(size as usize / 2 - 1)
}

/// Whether the process token holds privilege `luid` (enabled or not).
fn holds(luid: u32) -> bool {
    let mut token = 0;
    let mut b = [0u32; 128];
    let mut n = 0;
    // SAFETY: plain calls with our buffer.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), 8, &mut token) == 0 {
            return false;
        }
        let ok = GetTokenInformation(token, 3, b.as_mut_ptr().cast(), 512, &mut n) != 0;
        CloseHandle(token);
        ok && (0..b[0] as usize).any(|i| b[1 + 3 * i] == luid)
    }
}

/// A descriptor from SDDL (LocalAlloc'd; the caller frees it).
fn sddl(text: &W) -> *mut u8 {
    let mut sd = core::ptr::null_mut();
    // SAFETY: our string and output.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.p(),
            1,
            &mut sd,
            core::ptr::null_mut(),
        )
    };
    sd
}

/// LoadUserProfileW(hToken, lpProfileInfo): for a caller holding
/// SeBackupPrivilege and SeRestorePrivilege (services; else
/// ERROR_PRIVILEGE_NOT_HELD): the profile directory C:\\Users\\<lpUserName>
/// (created with full control for the user, SYSTEM and Administrators,
/// inherited), its ProfileList entry, and the user's hive
/// HKEY_USERS\\<SID> (created if new), returned as hProfile.
/// TODO: copying the default profile, NTUSER.DAT on a disk (D3).
#[unsafe(no_mangle)]
pub extern "system" fn LoadUserProfileW(token: usize, info: *mut u8) -> i32 {
    if info.is_null() {
        return fail(ERROR_INVALID_PARAMETER);
    }
    // SAFETY: the caller's PROFILEINFOW (dwSize, dwFlags, lpUserName at 8).
    let (size, name) =
        unsafe { (info.cast::<u32>().read(), info.add(8).cast::<*const u16>().read()) };
    if size < 56 || name.is_null() {
        return fail(ERROR_INVALID_PARAMETER);
    }
    if !holds(17) || !holds(18) {
        return fail(ERROR_PRIVILEGE_NOT_HELD);
    }
    let mut b = Buf([0; 256]);
    let Some(user) = user_of(token, &mut b) else { return 0 };
    let mut text = core::ptr::null_mut();
    // SAFETY: the token's SID; our output.
    if unsafe { ConvertSidToStringSidW(user, &mut text) } == 0 {
        return 0;
    }
    let dir = W::new().s("C:\\Users\\").w(name);
    let users = W::new().s("C:\\Users");
    let dacl = sddl(&W::new().s("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;").w(text).s(")"));
    let keys = sddl(&W::new().s("D:P(A;CI;KA;;;SY)(A;CI;KA;;;BA)(A;CI;KA;;;").w(text).s(")"));
    let result = (|| {
        // SAFETY: our strings and descriptors.
        unsafe {
            if CreateDirectoryW(users.p(), core::ptr::null()) == 0
                && GetLastError() != ERROR_ALREADY_EXISTS
            {
                return 0;
            }
            let sa = SecurityAttributes { length: 24, descriptor: dacl, inherit: 0 };
            if CreateDirectoryW(dir.p(), &sa) == 0 && GetLastError() != ERROR_ALREADY_EXISTS {
                return 0;
            }
            // ProfileList\<SID>: ProfileImagePath.
            let list = W::new().s(PROFILE_LIST).w(text);
            let mut key = 0;
            let mut disposition = 0;
            let r = RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                list.p(),
                0,
                core::ptr::null(),
                0,
                KEY_ALL_ACCESS,
                core::ptr::null(),
                &mut key,
                &mut disposition,
            );
            if r != 0 {
                return fail(r as u32);
            }
            let value = W::new().s("ProfileImagePath");
            RegSetValueExW(
                key,
                value.p(),
                0,
                REG_EXPAND_SZ,
                dir.p().cast(),
                (2 * (dir.n + 1)) as u32,
            );
            RegCloseKey(key);
            // The hive.
            let sa = SecurityAttributes { length: 24, descriptor: keys, inherit: 0 };
            let mut hive = 0;
            let r = RegCreateKeyExW(
                HKEY_USERS,
                text,
                0,
                core::ptr::null(),
                0,
                KEY_ALL_ACCESS,
                &sa,
                &mut hive,
                &mut disposition,
            );
            if r != 0 {
                return fail(r as u32);
            }
            info.add(48).cast::<usize>().write(hive);
        }
        1
    })();
    // SAFETY: LocalAlloc'd by advapi32.
    unsafe {
        LocalFree(text.cast());
        LocalFree(dacl);
        LocalFree(keys);
    }
    result
}

/// UnloadUserProfile(hToken, hProfile): closes the hive key.
#[unsafe(no_mangle)]
pub extern "system" fn UnloadUserProfile(_token: usize, profile: usize) -> i32 {
    if profile == 0 {
        return fail(ERROR_INVALID_PARAMETER);
    }
    // SAFETY: the key LoadUserProfileW returned.
    let r = unsafe { RegCloseKey(profile) };
    if r == 0 { 1 } else { fail(r as u32) }
}
