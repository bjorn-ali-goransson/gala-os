//! `fmodf`, which `%` on `f32` calls (Taffy's grid and rounding use it):
//! gala's programs are freestanding, with no C runtime to give it, so
//! gala_ui does (Windows builds; the host tests check it against std's
//! `%`).

/// x mod y, exactly, with x's sign (C's fmodf): NaN for a zero y, an
/// infinite x or a NaN; x itself when |x| < |y|.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn rem(x: f32, y: f32) -> f32 {
    if y == 0.0 || x.is_nan() || y.is_nan() || x.is_infinite() {
        return f32::NAN;
    }
    let (mut r, ay) = (x.abs(), y.abs());
    if r < ay {
        return x;
    }
    // The largest |y| * 2^k not above |x|; each step takes it off when it
    // fits, then halves it back down to |y|. Every subtraction is exact
    // (d <= r < 2d) and so is every scaling (powers of two, no overflow:
    // d <= r).
    let mut d = ay;
    while d <= r - d {
        d *= 2.0;
    }
    loop {
        if r >= d {
            r -= d;
        }
        if d == ay {
            break;
        }
        d *= 0.5;
    }
    if x.is_sign_negative() { -r } else { r }
}

#[cfg(all(windows, not(test)))]
#[unsafe(no_mangle)]
pub extern "C" fn fmodf(x: f32, y: f32) -> f32 {
    rem(x, y)
}

#[cfg(test)]
mod tests {
    use super::rem;

    #[test]
    fn matches_std_remainder() {
        let vals = [
            0.0f32, -0.0, 1.0, -1.0, 0.5, 2.5, 3.0, 7.25, -7.25, 1e-30, 1e30, -1e30, 123.456, 0.1,
            0.3, 1.0e-45, 3.4e38, 16777217.0, 360.0, -0.75,
        ];
        for &x in &vals {
            for &y in &vals {
                let (a, b) = (rem(x, y), x % y);
                assert!(
                    (a.is_nan() && b.is_nan()) || a.to_bits() == b.to_bits(),
                    "{x} % {y}: {a} vs {b}"
                );
            }
        }
        assert!(rem(f32::INFINITY, 1.0).is_nan());
        assert_eq!(rem(5.0, f32::INFINITY), 5.0);
    }
}
