//! The Schannel package lives in schannel.dll (`user/schannel`): secur32
//! loads it on first use and forwards Schannel's credentials and
//! contexts to it, recognised by the tags schannel.dll puts in
//! SecHandle.dwUpper.

use core::sync::atomic::{AtomicUsize, Ordering};

use super::sspi::SecHandle;

/// schannel.dll's handle tags ("galaSCHC", "galaSCHX").
const CREDENTIAL_TAG: usize = 0x6761_6c61_5343_4843_u64 as usize; // its low half on x86
const CONTEXT_TAG: usize = 0x6761_6c61_5343_4858_u64 as usize; // its low half on x86

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
}

static MODULE: AtomicUsize = AtomicUsize::new(0);

/// schannel.dll's export `name` (NUL-terminated); records a missing DLL
/// or function.
pub fn function(name: &[u8]) -> Option<usize> {
    let mut module = MODULE.load(Ordering::Acquire);
    if module == 0 {
        let dll: [u16; 13] = {
            let mut w = [0u16; 13];
            for (d, &b) in w.iter_mut().zip(b"schannel.dll") {
                *d = b.into();
            }
            w
        };
        // SAFETY: a NUL-terminated name; the reference is kept.
        module = unsafe { LoadLibraryW(dll.as_ptr()) };
        if module == 0 {
            let _ = super::missing(b"secur32.dll!schannel.dll");
            return None;
        }
        MODULE.store(module, Ordering::Release);
    }
    // SAFETY: the module; a NUL-terminated name.
    let f = unsafe { GetProcAddress(module, name.as_ptr()) };
    if f == 0 {
        let _ = super::missing(&name[..name.len() - 1]);
        return None;
    }
    Some(f)
}

fn tagged(h: *const SecHandle, tag: usize) -> bool {
    // SAFETY: the caller's handle, if any.
    !h.is_null() && unsafe { (*h).upper } == tag
}

/// A Schannel credential handle.
pub fn credential(h: *const SecHandle) -> bool {
    tagged(h, CREDENTIAL_TAG)
}

/// A Schannel context handle.
pub fn context(h: *const SecHandle) -> bool {
    tagged(h, CONTEXT_TAG)
}

/// Calls schannel.dll's `$name` (a NUL-terminated byte string) with the
/// signature `$ty`; SEC_E_SECPKG_NOT_FOUND without the DLL.
#[macro_export]
macro_rules! forward {
    ($name:literal, $ty:ty, $($arg:expr),*) => {{
        match $crate::schannel::function($name) {
            Some(f) => {
                // SAFETY: schannel.dll's export of that documented signature.
                let f = unsafe { core::mem::transmute::<usize, $ty>(f) };
                f($($arg),*)
            }
            None => $crate::SEC_E_SECPKG_NOT_FOUND,
        }
    }};
}
