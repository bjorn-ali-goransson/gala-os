//! Implementation for Windows
//!
//! gala's patch (THIRD_PARTY.md): ProcessPrng from bcryptprimitives.dll,
//! bound with `raw-dylib` so that gala's freestanding DLLs (linked
//! without import libraries) can use the crate. ProcessPrng is the
//! documented system RNG of Windows 10 and later and never fails.
use crate::Error;
use core::mem::MaybeUninit;

// On x86 the import is the undecorated name, as bcryptprimitives.dll
// exports it (a stdcall import is otherwise named `_ProcessPrng@8`).
#[cfg_attr(
    target_arch = "x86",
    link(name = "bcryptprimitives", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "bcryptprimitives", kind = "raw-dylib"))]
extern "system" {
    fn ProcessPrng(pbData: *mut u8, cbData: usize) -> i32;
}

pub fn getrandom_inner(dest: &mut [MaybeUninit<u8>]) -> Result<(), Error> {
    // SAFETY: a buffer of `dest.len()` bytes, written by the call.
    let ok = unsafe { ProcessPrng(dest.as_mut_ptr() as *mut u8, dest.len()) };
    if ok == 0 {
        return Err(Error::UNEXPECTED);
    }
    Ok(())
}
