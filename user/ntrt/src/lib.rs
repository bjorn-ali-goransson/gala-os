//! Tiny runtime for freestanding Windows test programs: raw system call
//! stubs (shaped like ntdll's) and access to the PEB. No imports, no CRT.
//! Programs written against it run unchanged on real Windows of the pinned
//! build, which is what differential testing needs.
//!
//! Built for x64 and ARM64 (`nt64.rs`, the 64-bit runtime they share,
//! with each one's own part in `x64.rs` and `arm64.rs`: the system call
//! stub's shape, the TEB's register, CONTEXT, the memory intrinsics) and
//! for x86 (`x86.rs`, a smaller runtime for 32-bit programs run through
//! WOW64: ARCHITECTURE "Plan: WOW64").

#![no_std]

// The architecture's own part comes first: it defines the `stubs!` macro
// the shared 64-bit runtime (`nt64.rs`) lists its services with.
#[cfg(target_arch = "x86_64")]
#[macro_use]
mod x64;
#[cfg(target_arch = "x86_64")]
pub use x64::*;

#[cfg(target_arch = "aarch64")]
#[macro_use]
mod arm64;
#[cfg(target_arch = "aarch64")]
pub use arm64::*;

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
mod nt64;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub use nt64::*;

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

/// What the panic handler `entry!` defines prints before it ends the
/// process with 0xC0000409: `<program>: panic at <file>:<line>:<column>:
/// <message>`. Without it a panic (a bounds check, an `unwrap`) leaves
/// only the exit status, which says nothing of where; a program that
/// panicked once in forty runs on real Windows could not be explained.
/// Formatted into a buffer on the stack (no allocator here), cut at its
/// end with `...`.
pub fn report_panic(info: &core::panic::PanicInfo) {
    struct Line {
        buf: [u8; 400],
        len: usize,
        cut: bool,
    }
    impl core::fmt::Write for Line {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            for &b in s.as_bytes() {
                if self.len < self.buf.len() {
                    self.buf[self.len] = b;
                    self.len += 1;
                } else {
                    self.cut = true;
                }
            }
            Ok(())
        }
    }
    let mut line = Line { buf: [0; 400], len: 0, cut: false };
    let name = program_name();
    let _ = core::fmt::Write::write_fmt(&mut line, format_args!("{}: panic", Units(name)));
    if let Some(at) = info.location() {
        let _ = core::fmt::Write::write_fmt(
            &mut line,
            format_args!(" at {}:{}:{}", at.file(), at.line(), at.column()),
        );
    }
    let _ = core::fmt::Write::write_fmt(&mut line, format_args!(": {}", info.message()));
    let end = if line.cut { "...\r\n" } else { "\r\n" };
    print(core::str::from_utf8(&line.buf[..line.len]).unwrap_or("panic (the report is not UTF-8)"));
    print(end);
}

/// UTF-16 units shown as text (ASCII; anything else as `?`).
struct Units(&'static [u16]);

impl core::fmt::Display for Units {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        for &u in self.0 {
            let c = u8::try_from(u).ok().filter(u8::is_ascii).map_or('?', char::from);
            core::fmt::Write::write_char(f, c)?;
        }
        Ok(())
    }
}

/// The executable's base name without `.exe`, from the PEB's
/// ProcessParameters' ImagePathName (x64 and ARM64: the parameters at PEB
/// +0x20, the name at +0x60; x86: +0x10 and +0x38).
fn program_name() -> &'static [u16] {
    let (params_at, name_at, buf_at) =
        if cfg!(target_pointer_width = "64") { (0x20, 0x60, 0x68) } else { (0x10, 0x38, 0x3c) };
    // SAFETY: the loader always provides ProcessParameters, whose
    // ImagePathName is a UNICODE_STRING it filled.
    let path = unsafe {
        let params = *(peb().add(params_at) as *const *const u8);
        if params.is_null() {
            return &[];
        }
        let len = usize::from(*(params.add(name_at) as *const u16)) / 2;
        let buf = *(params.add(buf_at) as *const *const u16);
        if buf.is_null() {
            return &[];
        }
        core::slice::from_raw_parts(buf, len)
    };
    let start = path.iter().rposition(|&u| u == u16::from(b'\\')).map_or(0, |i| i + 1);
    let name = &path[start..];
    let dot = name.iter().rposition(|&u| u == u16::from(b'.')).unwrap_or(name.len());
    &name[..dot]
}
