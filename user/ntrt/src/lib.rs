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
