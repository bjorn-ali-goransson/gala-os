//! The ARM64 parts of msvcrt, with `x64.rs`'s names: the floating-point
//! control word on FPCR and the raised-exception flags on FPSR (Arm's
//! Architecture Reference Manual for the registers; Microsoft's
//! `_controlfp` documentation for the abstract bits). The C-variadic entry
//! points and `_setjmp`/`longjmp` are not here yet (ARM64's variadic
//! convention and _JUMP_BUFFER): build.rs exports none of them for ARM64.

/// Where the port has not been written yet: recorded, then the process
/// ends (the panic handler's abort).
#[allow(dead_code)]
#[track_caller]
pub fn unported(what: &[u8]) -> ! {
    super::missing(what, 0);
    panic!("msvcrt on ARM64: not ported yet")
}

fn fpcr() -> u64 {
    let v: u64;
    // SAFETY: reads FPCR.
    unsafe {
        core::arch::asm!("mrs {}, fpcr", out(reg) v, options(nomem, nostack, preserves_flags))
    };
    v
}

fn set_fpcr(v: u64) {
    // SAFETY: writes FPCR (rounding, flush-to-zero, trap enables).
    unsafe {
        core::arch::asm!("msr fpcr, {}", "isb", in(reg) v, options(nomem, nostack, preserves_flags))
    };
}

fn fpsr() -> u64 {
    let v: u64;
    // SAFETY: reads FPSR.
    unsafe {
        core::arch::asm!("mrs {}, fpsr", out(reg) v, options(nomem, nostack, preserves_flags))
    };
    v
}

fn set_fpsr(v: u64) {
    // SAFETY: writes FPSR (the cumulative exception flags).
    unsafe {
        core::arch::asm!("msr fpsr, {}", in(reg) v, options(nomem, nostack, preserves_flags))
    };
}

/// (abstract _EM_* bit, FPCR trap-enable bit): set in the word = masked =
/// the trap disabled. Then _DN_FLUSH and FPCR.FZ (same sense).
const TRAPS: [(u32, u64); 6] = [
    (0x10, 1 << 8),      // _EM_INVALID: IOE
    (0x8, 1 << 9),       // _EM_ZERODIVIDE: DZE
    (0x4, 1 << 10),      // _EM_OVERFLOW: OFE
    (0x2, 1 << 11),      // _EM_UNDERFLOW: UFE
    (0x1, 1 << 12),      // _EM_INEXACT: IXE
    (0x8_0000, 1 << 15), // _EM_DENORMAL: IDE
];
const DN_FLUSH: u32 = 0x0100_0000;
const FPCR_FZ: u64 = 1 << 24;
/// FPCR.RMode (bits 23:22): 0 nearest, 1 toward +inf, 2 toward -inf,
/// 3 toward zero; `_RC_*`: _RC_NEAR 0, _RC_UP 0x200, _RC_DOWN 0x100,
/// _RC_CHOP 0x300.
const RC: [(u32, u64); 4] = [(0, 0), (0x200, 1), (0x100, 2), (0x300, 3)];

fn word_of(fpcr: u64) -> u32 {
    let mut word = TRAPS.iter().filter(|(_, t)| fpcr & t == 0).fold(0, |w, (a, _)| w | a);
    if fpcr & FPCR_FZ != 0 {
        word |= DN_FLUSH;
    }
    let mode = (fpcr >> 22) & 3;
    word | RC.iter().find(|(_, m)| *m == mode).map_or(0, |(a, _)| *a)
}

/// _controlfp(new, mask): the floating-point control word, the bits of
/// `mask` replaced by `new`'s (precision control, x86's, is ignored, as
/// on x64), returned as FPCR then holds it: a trap enable the processor
/// does not implement reads back as masked.
#[unsafe(no_mangle)]
pub extern "C" fn _controlfp(new: u32, mask: u32) -> u32 {
    let old = fpcr();
    let mask = mask & !0x3_0000; // _MCW_PC
    let word = (word_of(old) & !mask) | (new & mask);
    let mut out = old & !(TRAPS.iter().fold(0, |m, (_, t)| m | t) | FPCR_FZ | (3 << 22));
    for (a, t) in TRAPS {
        if word & a == 0 {
            out |= t;
        }
    }
    if word & DN_FLUSH != 0 {
        out |= FPCR_FZ;
    }
    out |= RC.iter().find(|(a, _)| *a == word & 0x300).map_or(0, |(_, m)| m << 22);
    set_fpcr(out);
    word_of(fpcr())
}

/// (FPSR cumulative flag, abstract _SW_* bit).
const FLAGS: [(u64, u32); 6] = [
    (1 << 0, 0x10),     // IOC: _SW_INVALID
    (1 << 1, 0x8),      // DZC: _SW_ZERODIVIDE
    (1 << 2, 0x4),      // OFC: _SW_OVERFLOW
    (1 << 3, 0x2),      // UFC: _SW_UNDERFLOW
    (1 << 4, 0x1),      // IXC: _SW_INEXACT
    (1 << 7, 0x8_0000), // IDC: _SW_DENORMAL
];

/// The raised-exception flags (FPSR's, as _SW_* bits), all cleared when
/// `clear`.
#[cfg(ucrt)]
pub fn fp_status(clear: bool) -> u32 {
    let sr = fpsr();
    if clear {
        fp_clear(u32::MAX);
    }
    FLAGS.iter().filter(|(f, _)| sr & f != 0).fold(0, |s, (_, a)| s | a)
}

/// Clears the raised-exception flags of `abstract_mask` (_SW_* bits).
#[cfg_attr(not(ucrt), allow(dead_code))]
pub fn fp_clear(abstract_mask: u32) {
    let bits = FLAGS.iter().filter(|(_, a)| abstract_mask & a != 0).fold(0, |m, (f, _)| m | f);
    set_fpsr(fpsr() & !bits);
}

/// _fpreset: the default control (round to nearest, no traps, no
/// flush-to-zero) and no raised flags.
pub fn fp_reset() {
    let keep = fpcr() & !(TRAPS.iter().fold(0, |m, (_, t)| m | t) | FPCR_FZ | (3 << 22));
    set_fpcr(keep);
    set_fpsr(0);
}
