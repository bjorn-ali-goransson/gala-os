//! Tiny runtime for freestanding Windows test programs: raw system call
//! stubs (shaped like ntdll's) and access to the PEB. No imports, no CRT.
//! Programs written against it run unchanged on real Windows of the pinned
//! build, which is what differential testing needs.
//!
//! Built for x64 (`x64.rs`, every test program) and for x86 (`x86.rs`, a
//! smaller runtime for 32-bit programs run through WOW64: ARCHITECTURE
//! "Plan: WOW64").

#![no_std]

#[cfg(target_arch = "x86_64")]
mod x64;
#[cfg(target_arch = "x86_64")]
pub use x64::*;

#[cfg(target_arch = "x86")]
mod x86;
#[cfg(target_arch = "x86")]
pub use x86::*;

/// The running Windows' build number: KUSER_SHARED_DATA's NtBuildNumber
/// (+0x260; the page is at 0x7ffe0000 in every process, 32-bit ones
/// included). Tests that check a build elsewhere (the PEB, the registry,
/// GetVersionEx) compare with this rather than with gala's pinned 26100,
/// so they hold on the newer builds a real machine runs; kuser_test checks
/// the number itself is plausible.
pub fn os_build() -> u32 {
    // SAFETY: KUSER_SHARED_DATA is mapped read-only in every process.
    unsafe { core::ptr::read_volatile(0x7ffe_0260 as *const u32) }
}

/// [`os_build`]'s five decimal digits (builds have had five since 10240).
pub fn os_build_digits() -> [u8; 5] {
    let mut n = os_build();
    let mut d = [b'0'; 5];
    for c in d.iter_mut().rev() {
        *c = b'0' + (n % 10) as u8;
        n /= 10;
    }
    d
}
