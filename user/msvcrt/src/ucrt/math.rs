//! The UCRT's math additions: classification (`_dclass`: FP_NORMAL -1,
//! FP_SUBNORMAL -2, FP_ZERO 0, FP_INFINITE 1, FP_NAN 2, as the UCRT's
//! math.h defines them), the sign tests, C99's rounding functions,
//! `_controlfp_s`, `_clearfp`/`_statusfp`, and on x86 the
//! `_libm_sse2_*_precise` functions that 32-bit code built with SSE2
//! calls (argument and result in xmm0, a second argument in xmm1).

use super::{EINVAL, invalid_parameter};

/// The class of a value with `exp_bits` all set (`inf_or_nan`), zero
/// exponent (`small`) and a zero mantissa (`zero_mantissa`).
fn class(inf_or_nan: bool, small: bool, zero_mantissa: bool) -> i16 {
    match (inf_or_nan, small, zero_mantissa) {
        (true, _, true) => 1,       // FP_INFINITE
        (true, _, false) => 2,      // FP_NAN
        (false, true, true) => 0,   // FP_ZERO
        (false, true, false) => -2, // FP_SUBNORMAL
        (false, false, _) => -1,    // FP_NORMAL
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _dclass(x: f64) -> i16 {
    let b = x.to_bits();
    let e = (b >> 52) & 0x7ff;
    class(e == 0x7ff, e == 0, b & ((1 << 52) - 1) == 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn _ldclass(x: f64) -> i16 {
    _dclass(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn _fdclass(x: f32) -> i16 {
    let b = x.to_bits();
    let e = (b >> 23) & 0xff;
    class(e == 0xff, e == 0, b & ((1 << 23) - 1) == 0)
}

/// `_dsign(x)`: nonzero (0x8000) when the sign bit is set.
#[unsafe(no_mangle)]
pub extern "C" fn _dsign(x: f64) -> i32 {
    if x.is_sign_negative() { 0x8000 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn _ldsign(x: f64) -> i32 {
    _dsign(x)
}

#[unsafe(no_mangle)]
pub extern "C" fn _fdsign(x: f32) -> i32 {
    if x.is_sign_negative() { 0x8000 } else { 0 }
}

/// `_dtest(&x)`: `_dclass` through a pointer.
#[unsafe(no_mangle)]
pub extern "C" fn _dtest(x: *const f64) -> i16 {
    // SAFETY: the caller's double.
    _dclass(unsafe { *x })
}

macro_rules! unary {
    ($($name:ident => $f:path),* $(,)?) => {$(
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(x: f64) -> f64 {
            $f(x)
        }
    )*};
}

macro_rules! unary_f {
    ($($name:ident => $f:path),* $(,)?) => {$(
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(x: f32) -> f32 {
            $f(x)
        }
    )*};
}

unary! {
    round => libm::round,
    trunc => libm::trunc,
    rint => libm::rint,
    nearbyint => libm::rint,
    cbrt => libm::cbrt,
    exp2 => libm::exp2,
    expm1 => libm::expm1,
    log2 => libm::log2,
    log1p => libm::log1p,
    asinh => libm::asinh,
    acosh => libm::acosh,
    atanh => libm::atanh,
    erf => libm::erf,
    erfc => libm::erfc,
    tgamma => libm::tgamma,
    lgamma => libm::lgamma,
}

unary_f! {
    roundf => libm::roundf,
    truncf => libm::truncf,
    sqrtf => libm::sqrtf,
    floorf => libm::floorf,
    ceilf => libm::ceilf,
    fabsf => libm::fabsf,
    expf => libm::expf,
    logf => libm::logf,
    log10f => libm::log10f,
    sinf => libm::sinf,
    cosf => libm::cosf,
    tanf => libm::tanf,
    atanf => libm::atanf,
}

#[unsafe(no_mangle)]
pub extern "C" fn powf(x: f32, y: f32) -> f32 {
    libm::powf(x, y)
}

#[unsafe(no_mangle)]
pub extern "C" fn fmodf(x: f32, y: f32) -> f32 {
    libm::fmodf(x, y)
}

#[unsafe(no_mangle)]
pub extern "C" fn atan2f(y: f32, x: f32) -> f32 {
    libm::atan2f(y, x)
}

#[unsafe(no_mangle)]
pub extern "C" fn fmax(x: f64, y: f64) -> f64 {
    libm::fmax(x, y)
}

#[unsafe(no_mangle)]
pub extern "C" fn fmin(x: f64, y: f64) -> f64 {
    libm::fmin(x, y)
}

#[unsafe(no_mangle)]
pub extern "C" fn copysign(x: f64, y: f64) -> f64 {
    libm::copysign(x, y)
}

#[unsafe(no_mangle)]
pub extern "C" fn _copysign(x: f64, y: f64) -> f64 {
    libm::copysign(x, y)
}

#[unsafe(no_mangle)]
pub extern "C" fn hypot(x: f64, y: f64) -> f64 {
    libm::hypot(x, y)
}

#[unsafe(no_mangle)]
pub extern "C" fn nextafter(x: f64, y: f64) -> f64 {
    libm::nextafter(x, y)
}

#[unsafe(no_mangle)]
pub extern "C" fn remainder(x: f64, y: f64) -> f64 {
    libm::remainder(x, y)
}

#[unsafe(no_mangle)]
pub extern "C" fn scalbn(x: f64, n: i32) -> f64 {
    libm::scalbn(x, n)
}

/// `lround(x)` / `llround(x)`: rounded half away from zero, as integers.
#[unsafe(no_mangle)]
pub extern "C" fn lround(x: f64) -> i32 {
    libm::round(x) as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn llround(x: f64) -> i64 {
    libm::round(x) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn lrint(x: f64) -> i32 {
    libm::rint(x) as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn llrint(x: f64) -> i64 {
    libm::rint(x) as i64
}

/// `_controlfp_s(&current, new, mask)`: `_controlfp`'s change, the new
/// word through `current`; EINVAL for bits no control field has.
#[unsafe(no_mangle)]
pub extern "C" fn _controlfp_s(current: *mut u32, new: u32, mask: u32) -> i32 {
    // _MCW_EM | _MCW_IC | _MCW_RC | _MCW_PC | _MCW_DN
    const VALID: u32 = 0x8_001f | 0x4_0000 | 0x300 | 0x3_0000 | 0x300_0000;
    if mask & !VALID != 0 {
        if !current.is_null() {
            // SAFETY: the caller's word.
            unsafe { *current = crate::arch::_controlfp(0, 0) };
        }
        invalid_parameter();
        return EINVAL;
    }
    let word = crate::arch::_controlfp(new, mask);
    if !current.is_null() {
        // SAFETY: the caller's word.
        unsafe { *current = word };
    }
    0
}

/// The floating-point status in the abstract form (_SW_INEXACT 0x1,
/// _SW_UNDERFLOW 0x2, _SW_OVERFLOW 0x4, _SW_ZERODIVIDE 0x8, _SW_INVALID
/// 0x10, _SW_DENORMAL 0x80000) of the hardware flags (bits 0-5 of MXCSR
/// and of the x87 status word: invalid, denormal, zero divide, overflow,
/// underflow, precision).
fn abstract_status(flags: u32) -> u32 {
    const MAP: [(u32, u32); 6] =
        [(0x1, 0x10), (0x2, 0x8_0000), (0x4, 0x8), (0x8, 0x4), (0x10, 0x2), (0x20, 0x1)];
    MAP.iter().filter(|(hw, _)| flags & hw != 0).fold(0, |s, (_, a)| s | a)
}

/// The hardware's raised-exception flags (MXCSR's, and on x86 the x87's
/// too), cleared when `clear`.
fn status(clear: bool) -> u32 {
    let mut mxcsr = 0u32;
    // SAFETY: reads MXCSR.
    unsafe { core::arch::asm!("stmxcsr [{}]", in(reg) &raw mut mxcsr, options(nostack)) };
    #[allow(unused_mut)]
    let mut flags = mxcsr & 0x3f;
    #[cfg(target_arch = "x86")]
    {
        let mut sw = 0u16;
        // SAFETY: reads the x87 status word.
        unsafe { core::arch::asm!("fnstsw [{}]", in(reg) &raw mut sw, options(nostack)) };
        flags |= u32::from(sw) & 0x3f;
        if clear {
            // SAFETY: clears the x87 exception flags.
            unsafe { core::arch::asm!("fnclex", options(nostack)) };
        }
    }
    if clear {
        let cleared = mxcsr & !0x3f;
        // SAFETY: writes MXCSR with only the flags cleared.
        unsafe { core::arch::asm!("ldmxcsr [{}]", in(reg) &raw const cleared, options(nostack)) };
    }
    abstract_status(flags)
}

/// `_clearfp()`: the status, then the flags cleared.
#[unsafe(no_mangle)]
pub extern "C" fn _clearfp() -> u32 {
    status(true)
}

/// `_statusfp()`: the status.
#[unsafe(no_mangle)]
pub extern "C" fn _statusfp() -> u32 {
    status(false)
}

/// `__fpe_flt_rounds()`: FLT_ROUNDS from the rounding mode (1 nearest,
/// 0 toward zero, 2 up, 3 down).
#[unsafe(no_mangle)]
pub extern "C" fn __fpe_flt_rounds() -> i32 {
    match crate::arch::_controlfp(0, 0) & 0x300 {
        0 => 1,
        0x100 => 3,
        0x200 => 2,
        _ => 0,
    }
}

// ---- x86: the SSE2 helpers -------------------------------------------------------

#[cfg(target_arch = "x86")]
mod sse2 {
    use core::arch::naked_asm;

    extern "C" fn exp(x: f64) -> f64 {
        libm::exp(x)
    }
    extern "C" fn log(x: f64) -> f64 {
        libm::log(x)
    }
    extern "C" fn log10(x: f64) -> f64 {
        libm::log10(x)
    }
    extern "C" fn pow(x: f64, y: f64) -> f64 {
        libm::pow(x, y)
    }
    extern "C" fn sin(x: f64) -> f64 {
        libm::sin(x)
    }
    extern "C" fn cos(x: f64) -> f64 {
        libm::cos(x)
    }
    extern "C" fn tan(x: f64) -> f64 {
        libm::tan(x)
    }
    extern "C" fn atan(x: f64) -> f64 {
        libm::atan(x)
    }
    extern "C" fn asin(x: f64) -> f64 {
        libm::asin(x)
    }
    extern "C" fn acos(x: f64) -> f64 {
        libm::acos(x)
    }

    /// `name`: xmm0 in, xmm0 out, through the cdecl function `target`
    /// (the argument on the stack, the result in st0).
    macro_rules! one {
        ($($name:ident => $target:path;)*) => {$(
            #[unsafe(naked)]
            #[unsafe(no_mangle)]
            pub extern "C" fn $name() {
                naked_asm!(
                    "sub esp, 8",
                    "movsd qword ptr [esp], xmm0",
                    "call {f}",
                    "fstp qword ptr [esp]",
                    "movsd xmm0, qword ptr [esp]",
                    "add esp, 8",
                    "ret",
                    f = sym $target,
                )
            }
        )*};
    }

    one! {
        _libm_sse2_exp_precise => exp;
        _libm_sse2_log_precise => log;
        _libm_sse2_log10_precise => log10;
        _libm_sse2_sin_precise => sin;
        _libm_sse2_cos_precise => cos;
        _libm_sse2_tan_precise => tan;
        _libm_sse2_atan_precise => atan;
        _libm_sse2_asin_precise => asin;
        _libm_sse2_acos_precise => acos;
    }

    /// `_libm_sse2_pow_precise`: xmm0 to the power xmm1, into xmm0.
    #[unsafe(naked)]
    #[unsafe(no_mangle)]
    pub extern "C" fn _libm_sse2_pow_precise() {
        naked_asm!(
            "sub esp, 16",
            "movsd qword ptr [esp], xmm0",
            "movsd qword ptr [esp + 8], xmm1",
            "call {f}",
            "fstp qword ptr [esp]",
            "movsd xmm0, qword ptr [esp]",
            "add esp, 16",
            "ret",
            f = sym pow,
        )
    }
}
