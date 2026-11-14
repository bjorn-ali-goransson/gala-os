//! The UCRT's C99 math beyond msvcrt's: the `float` and `long double`
//! forms (long double is double with MSVC), the complex functions on
//! `_Dcomplex`/`_Fcomplex` (complex.h: a struct of the real and imaginary
//! parts, passed as the architecture's C convention passes such a
//! struct), the floating-point environment's flag tests, `_dpcomp`, and
//! what NumPy's DLLs import of the rest (`btowc`, the new handler, the
//! time-name functions of the C++ library).
//!
//! The complex functions use the textbook identities (cexp(x+iy) =
//! e^x(cos y + i sin y), ccosh, csinh, ctanh from their real parts,
//! ccos(z) = ccosh(iz), csin(z) = -i csinh(iz), ctan(z) = -i ctanh(iz),
//! cpow(z, w) = cexp(w log z)); C99 Annex G's special cases for infinite
//! and NaN parts are not all followed (TODO).

use crate::heap::malloc;
use core::sync::atomic::{AtomicUsize, Ordering};

/// `_Dcomplex` (complex.h).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Dcomplex {
    re: f64,
    im: f64,
}

/// `_Fcomplex`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Fcomplex {
    re: f32,
    im: f32,
}

fn d(re: f64, im: f64) -> Dcomplex {
    Dcomplex { re, im }
}

fn mul(a: Dcomplex, b: Dcomplex) -> Dcomplex {
    d(a.re * b.re - a.im * b.im, a.re * b.im + a.im * b.re)
}

fn exp_c(z: Dcomplex) -> Dcomplex {
    let m = libm::exp(z.re);
    d(m * libm::cos(z.im), m * libm::sin(z.im))
}

fn log_c(z: Dcomplex) -> Dcomplex {
    d(libm::log(libm::hypot(z.re, z.im)), libm::atan2(z.im, z.re))
}

fn cosh_c(z: Dcomplex) -> Dcomplex {
    d(libm::cosh(z.re) * libm::cos(z.im), libm::sinh(z.re) * libm::sin(z.im))
}

fn sinh_c(z: Dcomplex) -> Dcomplex {
    d(libm::sinh(z.re) * libm::cos(z.im), libm::cosh(z.re) * libm::sin(z.im))
}

fn tanh_c(z: Dcomplex) -> Dcomplex {
    let den = libm::cosh(2.0 * z.re) + libm::cos(2.0 * z.im);
    d(libm::sinh(2.0 * z.re) / den, libm::sin(2.0 * z.im) / den)
}

/// i z.
fn times_i(z: Dcomplex) -> Dcomplex {
    d(-z.im, z.re)
}

/// -i z.
fn times_minus_i(z: Dcomplex) -> Dcomplex {
    d(z.im, -z.re)
}

fn cos_c(z: Dcomplex) -> Dcomplex {
    cosh_c(times_i(z))
}

fn sin_c(z: Dcomplex) -> Dcomplex {
    times_minus_i(sinh_c(times_i(z)))
}

fn tan_c(z: Dcomplex) -> Dcomplex {
    times_minus_i(tanh_c(times_i(z)))
}

fn pow_c(z: Dcomplex, w: Dcomplex) -> Dcomplex {
    if z.re == 0.0 && z.im == 0.0 {
        return if w.re == 0.0 && w.im == 0.0 { d(1.0, 0.0) } else { d(0.0, 0.0) };
    }
    exp_c(mul(w, log_c(z)))
}

fn widen(z: Fcomplex) -> Dcomplex {
    d(f64::from(z.re), f64::from(z.im))
}

fn narrow(z: Dcomplex) -> Fcomplex {
    Fcomplex { re: z.re as f32, im: z.im as f32 }
}

/// The double, float and long double forms of a complex function.
macro_rules! complex1 {
    ($($f:ident, $ff:ident, $fl:ident => $op:ident;)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "C" fn $f(z: Dcomplex) -> Dcomplex {
            $op(z)
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn $ff(z: Fcomplex) -> Fcomplex {
            narrow($op(widen(z)))
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn $fl(z: Dcomplex) -> Dcomplex {
            $op(z)
        }
    )*};
}

complex1! {
    cexp, cexpf, cexpl => exp_c;
    clog, clogf, clogl => log_c;
    ccos, ccosf, ccosl => cos_c;
    ccosh, ccoshf, ccoshl => cosh_c;
    csin, csinf, csinl => sin_c;
    csinh, csinhf, csinhl => sinh_c;
    ctan, ctanf, ctanl => tan_c;
    ctanh, ctanhf, ctanhl => tanh_c;
}

#[unsafe(no_mangle)]
pub extern "C" fn cpow(z: Dcomplex, w: Dcomplex) -> Dcomplex {
    pow_c(z, w)
}

#[unsafe(no_mangle)]
pub extern "C" fn cpowl(z: Dcomplex, w: Dcomplex) -> Dcomplex {
    pow_c(z, w)
}

#[unsafe(no_mangle)]
pub extern "C" fn cpowf(z: Fcomplex, w: Fcomplex) -> Fcomplex {
    narrow(pow_c(widen(z), widen(w)))
}

macro_rules! complex_real {
    ($($f:ident, $ff:ident, $fl:ident => |$z:ident| $e:expr;)*) => {$(
        #[unsafe(no_mangle)]
        pub extern "C" fn $f($z: Dcomplex) -> f64 {
            $e
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn $ff(z: Fcomplex) -> f32 {
            let $z = widen(z);
            ($e) as f32
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn $fl($z: Dcomplex) -> f64 {
            $e
        }
    )*};
}

complex_real! {
    cabs, cabsf, cabsl => |z| libm::hypot(z.re, z.im);
    carg, cargf, cargl => |z| libm::atan2(z.im, z.re);
    creal, crealf, creall => |z| z.re;
    cimag, cimagf, cimagl => |z| z.im;
}

/// `_cabs(struct _complex)`: the same struct of two doubles.
#[unsafe(no_mangle)]
pub extern "C" fn _cabs(z: Dcomplex) -> f64 {
    libm::hypot(z.re, z.im)
}

// ---- Real functions: float and long double forms ----------------------------

macro_rules! unary_f {
    ($($name:ident => $f:path),* $(,)?) => {$(
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(x: f32) -> f32 {
            $f(x)
        }
    )*};
}

macro_rules! unary_l {
    ($($name:ident => $f:path),* $(,)?) => {$(
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(x: f64) -> f64 {
            $f(x)
        }
    )*};
}

unary_f! {
    acosf => libm::acosf,
    acoshf => libm::acoshf,
    asinf => libm::asinf,
    asinhf => libm::asinhf,
    atanhf => libm::atanhf,
    cbrtf => libm::cbrtf,
    coshf => libm::coshf,
    sinhf => libm::sinhf,
    tanhf => libm::tanhf,
    exp2f => libm::exp2f,
    expm1f => libm::expm1f,
    log1pf => libm::log1pf,
    log2f => libm::log2f,
    rintf => libm::rintf,
}

unary_l! {
    acoshl => libm::acosh,
    asinhl => libm::asinh,
    atanhl => libm::atanh,
    cbrtl => libm::cbrt,
    exp2l => libm::exp2,
    expm1l => libm::expm1,
    log1pl => libm::log1p,
    rintl => libm::rint,
    truncl => libm::trunc,
}

#[unsafe(no_mangle)]
pub extern "C" fn copysignf(x: f32, y: f32) -> f32 {
    libm::copysignf(x, y)
}

#[unsafe(no_mangle)]
pub extern "C" fn copysignl(x: f64, y: f64) -> f64 {
    libm::copysign(x, y)
}

#[unsafe(no_mangle)]
pub extern "C" fn _hypotf(x: f32, y: f32) -> f32 {
    libm::hypotf(x, y)
}

#[unsafe(no_mangle)]
pub extern "C" fn fmaxf(x: f32, y: f32) -> f32 {
    libm::fmaxf(x, y)
}

#[unsafe(no_mangle)]
pub extern "C" fn fminf(x: f32, y: f32) -> f32 {
    libm::fminf(x, y)
}

#[unsafe(no_mangle)]
pub extern "C" fn nextafterf(x: f32, y: f32) -> f32 {
    libm::nextafterf(x, y)
}

#[unsafe(no_mangle)]
pub extern "C" fn nextafterl(x: f64, y: f64) -> f64 {
    libm::nextafter(x, y)
}

/// `modff(x, &i)`.
#[unsafe(no_mangle)]
pub extern "C" fn modff(x: f32, i: *mut f32) -> f32 {
    let (fraction, integral) = libm::modff(x);
    // SAFETY: the caller's float.
    unsafe { *i = integral };
    fraction
}

/// `_dpcomp(x, y)` / `_fdpcomp`: _FP_LT (1), _FP_EQ (2) or _FP_GT (4);
/// 0 when unordered (a NaN).
#[unsafe(no_mangle)]
pub extern "C" fn _dpcomp(x: f64, y: f64) -> i32 {
    match x.partial_cmp(&y) {
        Some(core::cmp::Ordering::Less) => 1,
        Some(core::cmp::Ordering::Equal) => 2,
        Some(core::cmp::Ordering::Greater) => 4,
        None => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _fdpcomp(x: f32, y: f32) -> i32 {
    _dpcomp(f64::from(x), f64::from(y))
}

// ---- The floating-point environment ----------------------------------------------

/// `fetestexcept(excepts)`: which of the FE_* flags (fenv.h: FE_INEXACT 1,
/// FE_UNDERFLOW 2, FE_OVERFLOW 4, FE_DIVBYZERO 8, FE_INVALID 0x10, the
/// _SW_* bits) are raised.
#[unsafe(no_mangle)]
pub extern "C" fn fetestexcept(excepts: i32) -> i32 {
    (super::math::_statusfp() as i32) & excepts & 0x1f
}

/// `feclearexcept(excepts)`: clears those flags; 0.
#[unsafe(no_mangle)]
pub extern "C" fn feclearexcept(excepts: i32) -> i32 {
    super::math::clear_flags((excepts & 0x1f) as u32);
    0
}

// ---- The rest of what NumPy's DLLs import ------------------------------------------

/// `btowc(c)`: a single byte as a wide character: ASCII itself; others
/// begin multi-byte UTF-8 characters, so WEOF.
#[unsafe(no_mangle)]
pub extern "C" fn btowc(c: i32) -> u16 {
    if (0..0x80).contains(&c) { c as u16 } else { 0xffff }
}

static NEW_HANDLER: AtomicUsize = AtomicUsize::new(0);

/// `_set_new_handler(h)`: the handler `_callnewh` would call; the
/// previous one. (gala's malloc does not call it: `_callnewh` says none
/// ran.)
#[unsafe(no_mangle)]
pub extern "C" fn _set_new_handler(h: usize) -> usize {
    NEW_HANDLER.swap(h, Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn _query_new_handler() -> usize {
    NEW_HANDLER.load(Ordering::Relaxed)
}

/// The "C" locale's day and month names in `_Getdays`'s and
/// `_Getmonths`'s form: `:Sun:Sunday:Mon:Monday:...`.
const DAYS: &str =
    ":Sun:Sunday:Mon:Monday:Tue:Tuesday:Wed:Wednesday:Thu:Thursday:Fri:Friday:Sat:Saturday";
const MONTHS: &str = ":Jan:January:Feb:February:Mar:March:Apr:April:May:May:Jun:June:Jul:July:Aug:August:Sep:September:Oct:October:Nov:November:Dec:December";

/// A malloc'd NUL-terminated copy (the caller frees it).
fn copy_narrow(s: &str) -> *mut u8 {
    let p = malloc(s.len() + 1);
    if !p.is_null() {
        // SAFETY: room for the text and its NUL.
        unsafe {
            core::ptr::copy_nonoverlapping(s.as_ptr(), p, s.len());
            *p.add(s.len()) = 0;
        }
    }
    p
}

fn copy_wide(s: &str) -> *mut u16 {
    let p = malloc(2 * (s.len() + 1)).cast::<u16>();
    if !p.is_null() {
        for (i, b) in s.bytes().enumerate() {
            // SAFETY: room for the text and its NUL (ASCII).
            unsafe { *p.add(i) = u16::from(b) };
        }
        // SAFETY: as above.
        unsafe { *p.add(s.len()) = 0 };
    }
    p
}

#[unsafe(no_mangle)]
pub extern "C" fn _Getdays() -> *mut u8 {
    copy_narrow(DAYS)
}

#[unsafe(no_mangle)]
pub extern "C" fn _Getmonths() -> *mut u8 {
    copy_narrow(MONTHS)
}

#[unsafe(no_mangle)]
pub extern "C" fn _W_Getdays() -> *mut u16 {
    copy_wide(DAYS)
}

#[unsafe(no_mangle)]
pub extern "C" fn _W_Getmonths() -> *mut u16 {
    copy_wide(MONTHS)
}

/// `_Gettnames()` / `_W_Gettnames()`: the locale's time-name table
/// (`__lc_time_data`, the C++ library's time facets). Not built here:
/// recorded, NULL.
#[unsafe(no_mangle)]
pub extern "C" fn _Gettnames() -> *mut u8 {
    crate::missing(b"ucrtbase.dll!_Gettnames", 0);
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn _W_Gettnames() -> *mut u8 {
    crate::missing(b"ucrtbase.dll!_W_Gettnames", 0);
    core::ptr::null_mut()
}

/// `_Strftime(buf, size, format, tm, lc_time)`: strftime in the "C"
/// locale (the time-name table is not used).
#[unsafe(no_mangle)]
pub extern "C" fn _Strftime(
    buf: *mut u8,
    size: usize,
    format: *const u8,
    tm: *const crate::time::Tm,
    _names: *mut u8,
) -> usize {
    crate::time::strftime(buf, size, format, tm)
}

/// `_Wcsftime(buf, size, format, tm, lc_time)`: wcsftime likewise.
#[unsafe(no_mangle)]
pub extern "C" fn _Wcsftime(
    buf: *mut u16,
    size: usize,
    format: *const u16,
    tm: *const crate::time::Tm,
    _names: *mut u8,
) -> usize {
    crate::wide_more::wcsftime(buf, size, format, tm)
}
