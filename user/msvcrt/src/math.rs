//! The C math library msvcrt exports (sin, pow, floor, ...), from the
//! `libm` crate (a Rust port of musl's libm; THIRD_PARTY.md). Results are
//! within an ulp of the exact value; msvcrt's own may differ in the last
//! bit for the transcendental functions.

macro_rules! unary {
    ($($name:ident),* $(,)?) => {$(
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(x: f64) -> f64 {
            libm::$name(x)
        }
    )*};
}

macro_rules! binary {
    ($($name:ident),* $(,)?) => {$(
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(x: f64, y: f64) -> f64 {
            libm::$name(x, y)
        }
    )*};
}

unary!(sin, cos, tan, asin, acos, atan, sinh, cosh, tanh, exp, log, log10, sqrt, ceil, floor, fabs);
binary!(pow, atan2, fmod);

/// _hypot(x, y): sqrt(x*x + y*y) without overflow.
#[unsafe(no_mangle)]
pub extern "C" fn _hypot(x: f64, y: f64) -> f64 {
    libm::hypot(x, y)
}

/// ldexp(x, e): x * 2^e.
#[unsafe(no_mangle)]
pub extern "C" fn ldexp(x: f64, e: i32) -> f64 {
    libm::ldexp(x, e)
}

/// frexp(x, &e): the mantissa in [0.5, 1) and the exponent.
#[unsafe(no_mangle)]
pub extern "C" fn frexp(x: f64, e: *mut i32) -> f64 {
    let (m, exp) = libm::frexp(x);
    // SAFETY: the caller's int pointer.
    unsafe { *e = exp };
    m
}

/// modf(x, &i): the fractional part, the integral part through `i`.
#[unsafe(no_mangle)]
pub extern "C" fn modf(x: f64, i: *mut f64) -> f64 {
    let (fraction, integral) = libm::modf(x);
    // SAFETY: the caller's double pointer.
    unsafe { *i = integral };
    fraction
}

#[unsafe(no_mangle)]
pub extern "C" fn abs(x: i32) -> i32 {
    x.wrapping_abs()
}

#[unsafe(no_mangle)]
pub extern "C" fn labs(x: i32) -> i32 {
    x.wrapping_abs()
}

#[unsafe(no_mangle)]
pub extern "C" fn _isnan(x: f64) -> i32 {
    i32::from(x.is_nan())
}

#[unsafe(no_mangle)]
pub extern "C" fn _finite(x: f64) -> i32 {
    i32::from(x.is_finite())
}
