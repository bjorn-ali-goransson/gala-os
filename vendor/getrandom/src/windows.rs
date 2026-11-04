//! Implementation for Windows
//!
//! gala's patch (THIRD_PARTY.md): ProcessPrng from bcryptprimitives.dll,
//! bound with `raw-dylib` so that gala's freestanding DLLs (linked
//! without import libraries) can use the crate. ProcessPrng is the
//! documented system RNG of Windows 10 and later and never fails.
use crate::Error;
use core::mem::MaybeUninit;

#[link(name = "bcryptprimitives", kind = "raw-dylib")]
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
