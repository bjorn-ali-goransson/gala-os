//! gala's secur32.dll: SSPI (`sspi`: the Unicode functions and the
//! dispatch table) with one security package, NTLM (`ntlm`); the ANSI
//! functions the JDK's net.dll imports find no package (recorded).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

// The global allocator: `digest` has `alloc` in builds with bcrypt.
use gala_heap as _;

mod lsa;
mod ntlm;
mod sspi;
mod unimplemented;

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import. (It becomes a harmless extra data export.)
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

const SEC_E_INVALID_HANDLE: u32 = 0x8009_0301;
const SEC_E_SECPKG_NOT_FOUND: u32 = 0x8009_0305;

fn missing(name: &[u8]) -> u32 {
    missing_detail(name, 0)
}

/// Records an unsupported request (with a detail) in the ledger.
fn missing_detail(name: &[u8], detail: u64) -> u32 {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(name.as_ptr(), name.len(), 2, detail) };
    SEC_E_SECPKG_NOT_FOUND
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn AcquireCredentialsHandleA(
    _principal: usize,
    _package: usize,
    _use: u32,
    _logon: usize,
    _auth: usize,
    _get_key: usize,
    _arg: usize,
    _credential: usize,
    _expiry: usize,
) -> u32 {
    missing(b"secur32.dll!AcquireCredentialsHandleA")
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn InitializeSecurityContextA(
    _credential: usize,
    _context: usize,
    _target: usize,
    _flags: u32,
    _reserved: u32,
    _representation: u32,
    _input: usize,
    _reserved2: u32,
    _new: usize,
    _output: usize,
    _attributes: usize,
    _expiry: usize,
) -> u32 {
    missing(b"secur32.dll!InitializeSecurityContextA")
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(code: u32);
}

/// GetUserNameExW: the caller's user's names (advapi32's GetUserNameW,
/// the domain from LookupAccountNameW). NameSamCompatible (2) is
/// "DOMAIN\\name" ("GALA\\gala"); NameDisplay (3) the name (no full name
/// is set); the directory-service formats have no mapping for a local
/// account (ERROR_NONE_MAPPED). Returns a BOOLEAN; with too small a buffer,
/// ERROR_MORE_DATA and the size needed (with the NUL), else the length
/// written (without it). TODO: SYSTEM's NameSamCompatible is the machine
/// account (WORKGROUP\\GALA$) on Windows, NT AUTHORITY\\SYSTEM here.
#[unsafe(no_mangle)]
pub extern "system" fn GetUserNameExW(format: u32, buf: *mut u16, size: *mut u32) -> u8 {
    #[link(name = "advapi32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn GetUserNameW(buffer: *mut u16, size: *mut u32) -> i32;
        fn LookupAccountNameW(
            system: *const u16,
            name: *const u16,
            sid: *mut u8,
            sid_size: *mut u32,
            domain: *mut u16,
            domain_size: *mut u32,
            kind: *mut u32,
        ) -> i32;
    }
    if format != 2 && format != 3 {
        // SAFETY: plain call.
        unsafe { SetLastError(1332) }; // ERROR_NONE_MAPPED
        return 0;
    }
    if size.is_null() {
        // SAFETY: as above.
        unsafe { SetLastError(87) };
        return 0;
    }
    let mut user = [0u16; 256];
    let mut user_len = 256u32;
    // SAFETY: our buffer.
    if unsafe { GetUserNameW(user.as_mut_ptr(), &mut user_len) } == 0 {
        return 0;
    }
    let user_len = user_len as usize - 1;
    let mut text = [0u16; 330];
    let mut n = 0;
    if format == 2 {
        let mut sid = [0u8; 68];
        let (mut sid_size, mut domain_size, mut kind) = (68u32, 64u32, 0u32);
        let mut domain = [0u16; 64];
        // SAFETY: our buffers of those sizes.
        let found = unsafe {
            LookupAccountNameW(
                core::ptr::null(),
                user.as_ptr(),
                sid.as_mut_ptr(),
                &mut sid_size,
                domain.as_mut_ptr(),
                &mut domain_size,
                &mut kind,
            )
        };
        if found == 0 {
            return 0;
        }
        let d = domain_size as usize;
        text[..d].copy_from_slice(&domain[..d]);
        text[d] = u16::from(b'\\');
        n = d + 1;
    }
    text[n..n + user_len].copy_from_slice(&user[..user_len]);
    n += user_len;
    // SAFETY: the caller's size and buffer of that many characters.
    unsafe {
        if buf.is_null() || (*size as usize) < n + 1 {
            *size = n as u32 + 1;
            SetLastError(234); // ERROR_MORE_DATA
            return 0;
        }
        core::ptr::copy_nonoverlapping(text.as_ptr(), buf, n);
        *buf.add(n) = 0;
        *size = n as u32;
    }
    1
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
