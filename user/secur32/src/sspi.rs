//! SSPI's Unicode entry points and its dispatch table
//! (InitSecurityInterfaceW, SecurityFunctionTableW as sspi.h lays it
//! out). gala has no security packages: none are listed, looking one up
//! or acquiring credentials gives SEC_E_SECPKG_NOT_FOUND (recorded), and
//! since no credential or context handle can exist, the functions taking
//! one give SEC_E_INVALID_HANDLE.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::too_many_arguments)]

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use super::{
    CompleteAuthToken, DeleteSecurityContext, FreeCredentialsHandle, SEC_E_INVALID_HANDLE,
    SEC_E_SECPKG_NOT_FOUND, missing,
};

const SEC_E_OK: u32 = 0;
const SEC_E_UNSUPPORTED_FUNCTION: u32 = 0x8009_0302;

/// EnumerateSecurityPackagesW: no packages.
#[unsafe(no_mangle)]
pub extern "system" fn EnumerateSecurityPackagesW(count: *mut u32, info: *mut usize) -> u32 {
    // SAFETY: the caller's outputs.
    unsafe {
        if !count.is_null() {
            count.write(0);
        }
        if !info.is_null() {
            info.write(0);
        }
    }
    SEC_E_OK
}

/// QuerySecurityPackageInfoW: no package of any name (the name asked for
/// goes to the ledger: `secur32.dll!package <name>`).
#[unsafe(no_mangle)]
pub extern "system" fn QuerySecurityPackageInfoW(name: *const u16, info: *mut usize) -> u32 {
    let mut text = [0u8; 64];
    let prefix = b"secur32.dll!package ";
    text[..prefix.len()].copy_from_slice(prefix);
    let mut n = prefix.len();
    if !name.is_null() {
        // SAFETY: the caller's NUL-terminated name, read up to its NUL.
        while n < text.len() && unsafe { *name.add(n - prefix.len()) } != 0 {
            text[n] = unsafe { *name.add(n - prefix.len()) } as u8;
            n += 1;
        }
    }
    let _ = missing(&text[..n]);
    if !info.is_null() {
        // SAFETY: the caller's output.
        unsafe { info.write(0) };
    }
    SEC_E_SECPKG_NOT_FOUND
}

/// AcquireCredentialsHandleW: no package to give credentials (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn AcquireCredentialsHandleW(
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
    missing(b"secur32.dll!AcquireCredentialsHandleW")
}

/// FreeContextBuffer: no buffer is ever handed out; NULL is fine.
#[unsafe(no_mangle)]
pub extern "system" fn FreeContextBuffer(_buffer: usize) -> u32 {
    SEC_E_OK
}

/// The functions that take a credential or context handle.
macro_rules! invalid_handle {
    ($($name:ident)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "system" fn $name() -> u32 {
            SEC_E_INVALID_HANDLE
        }
    )*};
}

invalid_handle! {
    QueryCredentialsAttributesW InitializeSecurityContextW AcceptSecurityContext
    ApplyControlToken QueryContextAttributesW ImpersonateSecurityContext
    RevertSecurityContext MakeSignature VerifySignature ExportSecurityContext
    ImportSecurityContextW AddCredentialsW QuerySecurityContextToken EncryptMessage
    DecryptMessage SetContextAttributesW SetCredentialsAttributesW
    QueryContextAttributesExW QueryCredentialsAttributesExW
}

/// ChangeAccountPasswordW: no package can change a password.
#[unsafe(no_mangle)]
pub extern "system" fn ChangeAccountPasswordW() -> u32 {
    SEC_E_UNSUPPORTED_FUNCTION
}

/// SecurityFunctionTableW (dwVersion, then 31 function pointers): atomics
/// have the layout of the plain fields, so the table can be filled in
/// place at run time (function addresses are only known after relocation)
/// and read by callers as the plain structure.
#[repr(C)]
pub struct SecurityFunctionTable {
    version: AtomicU32,
    functions: [AtomicUsize; 31],
}

static TABLE: SecurityFunctionTable = SecurityFunctionTable {
    version: AtomicU32::new(0),
    functions: [const { AtomicUsize::new(0) }; 31],
};

/// InitSecurityInterfaceW: the dispatch table (static; SSPI version 4,
/// with the two version-5 entries filled in too).
#[unsafe(no_mangle)]
pub extern "system" fn InitSecurityInterfaceW() -> *const SecurityFunctionTable {
    let functions: [usize; 31] = [
        EnumerateSecurityPackagesW as *const () as usize,
        QueryCredentialsAttributesW as *const () as usize,
        AcquireCredentialsHandleW as *const () as usize,
        FreeCredentialsHandle as *const () as usize,
        0, // Reserved2
        InitializeSecurityContextW as *const () as usize,
        AcceptSecurityContext as *const () as usize,
        CompleteAuthToken as *const () as usize,
        DeleteSecurityContext as *const () as usize,
        ApplyControlToken as *const () as usize,
        QueryContextAttributesW as *const () as usize,
        ImpersonateSecurityContext as *const () as usize,
        RevertSecurityContext as *const () as usize,
        MakeSignature as *const () as usize,
        VerifySignature as *const () as usize,
        FreeContextBuffer as *const () as usize,
        QuerySecurityPackageInfoW as *const () as usize,
        0, // Reserved3
        0, // Reserved4
        ExportSecurityContext as *const () as usize,
        ImportSecurityContextW as *const () as usize,
        AddCredentialsW as *const () as usize,
        0, // Reserved8
        QuerySecurityContextToken as *const () as usize,
        EncryptMessage as *const () as usize,
        DecryptMessage as *const () as usize,
        SetContextAttributesW as *const () as usize,
        SetCredentialsAttributesW as *const () as usize,
        ChangeAccountPasswordW as *const () as usize,
        QueryContextAttributesExW as *const () as usize,
        QueryCredentialsAttributesExW as *const () as usize,
    ];
    for (slot, f) in TABLE.functions.iter().zip(functions) {
        slot.store(f, Ordering::Relaxed);
    }
    TABLE.version.store(4, Ordering::Release);
    &TABLE
}
