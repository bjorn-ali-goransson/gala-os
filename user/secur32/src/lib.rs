//! gala's secur32.dll: SSPI (the functions the JDK's net.dll imports, the
//! Unicode ones and the dispatch table SQL Server uses: `sspi`). gala has
//! no security packages: acquiring credentials or a context fails with
//! SEC_E_SECPKG_NOT_FOUND (recorded), handles are invalid.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

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
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(name.as_ptr(), name.len(), 2, 0) };
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

#[unsafe(no_mangle)]
pub extern "system" fn CompleteAuthToken(_context: usize, _token: usize) -> u32 {
    SEC_E_INVALID_HANDLE
}

#[unsafe(no_mangle)]
pub extern "system" fn DeleteSecurityContext(_context: usize) -> u32 {
    SEC_E_INVALID_HANDLE
}

#[unsafe(no_mangle)]
pub extern "system" fn FreeCredentialsHandle(_credential: usize) -> u32 {
    SEC_E_INVALID_HANDLE
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(code: u32);
}

/// GetUserNameExW: the one user's names. NameSamCompatible (2) is
/// "GALA\gala"; NameDisplay (3) is "gala" (no full name is set); the
/// directory-service formats (fully qualified, principal and the like)
/// have no mapping for a local account (ERROR_NONE_MAPPED). Returns a
/// BOOLEAN; with too small a buffer, ERROR_MORE_DATA and the size needed
/// (with the NUL), else the length written (without it).
#[unsafe(no_mangle)]
pub extern "system" fn GetUserNameExW(format: u32, buf: *mut u16, size: *mut u32) -> u8 {
    let name: &str = match format {
        2 => "GALA\\gala",
        3 => "gala",
        _ => {
            // SAFETY: plain call.
            unsafe { SetLastError(1332) }; // ERROR_NONE_MAPPED
            return 0;
        }
    };
    if size.is_null() {
        // SAFETY: as above.
        unsafe { SetLastError(87) };
        return 0;
    }
    // SAFETY: the caller's size and buffer of that many characters.
    unsafe {
        if buf.is_null() || (*size as usize) < name.len() + 1 {
            *size = name.len() as u32 + 1;
            SetLastError(234); // ERROR_MORE_DATA
            return 0;
        }
        for (i, b) in name.bytes().enumerate() {
            *buf.add(i) = b.into();
        }
        *buf.add(name.len()) = 0;
        *size = name.len() as u32;
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
