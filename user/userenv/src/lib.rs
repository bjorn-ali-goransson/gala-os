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
    fn __chkstk();
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(code: u32);
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn GetCurrentProcess() -> usize;
    fn CloseHandle(h: usize) -> i32;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
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
