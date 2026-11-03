//! The arithmetic of the clock's calibration (ARCHITECTURE "Interrupts
//! and time", calibration): which samples to believe, and whether the RTC
//! agrees afterwards. Pure and allocation-free (host-tested); the
//! hardware side is `arch::apic::measure` and `time::rtc_check`.
//!
//! A sample measures the TSC and the LAPIC timer against a wall-clock
//! reference (HPET, ACPI PM timer, PIT) between two *brackets*: each reads
//! the reference and the timer between two TSC reads. Under an emulator
//! the vCPU can be descheduled at any instruction, for milliseconds; a
//! pause between the brackets is harmless (all three clocks follow the
//! host's time), a pause inside one makes it wide and the sample is
//! rejected. The median of the samples left, with those far from it
//! dropped, is the result.

/// At most this many samples are taken.
pub const MAX_SAMPLES: usize = 16;
/// Samples wanted before stopping early.
pub const WANT_SAMPLES: usize = 5;
/// Fewer samples left than this: no estimate.
pub const MIN_SAMPLES: usize = 3;
/// A bracket wider than 1/BRACKET_LIMIT of the sample's TSC interval
/// rejects the sample (0.1%: the vCPU paused while reading).
pub const BRACKET_LIMIT: u64 = 1000;
/// Samples further than this from the median (parts per million) are
/// outliers.
pub const OUTLIER_PPM: u64 = 5000;

/// One measurement over a window of the reference clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sample {
    /// TSC ticks between the brackets' midpoints.
    pub tsc: u64,
    /// LAPIC timer counts over the same interval.
    pub timer: u64,
    /// Reference ticks over the same interval.
    pub reference: u64,
    /// The wider bracket, in TSC ticks.
    pub bracket: u64,
}

/// What `estimate` makes of the samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Estimate {
    pub tsc_hz: u64,
    pub timer_hz: u64,
    /// Samples used.
    pub used: usize,
    /// Rejected: a wide bracket (the vCPU paused while reading).
    pub wide: usize,
    /// Rejected: too far from the median.
    pub outliers: usize,
    /// (max - min) / median of the TSC rates used, in ppm.
    pub spread_ppm: u64,
}

fn rate(count: u64, reference: u64, reference_hz: u64) -> u64 {
    (u128::from(count) * u128::from(reference_hz) / u128::from(reference.max(1))) as u64
}

fn median(v: &mut [u64]) -> u64 {
    v.sort_unstable();
    v.get(v.len() / 2).copied().unwrap_or(0)
}

fn near(x: u64, m: u64, ppm: u64) -> bool {
    u128::from(x.abs_diff(m)) * 1_000_000 <= u128::from(m) * u128::from(ppm)
}

/// Whether a sample's brackets were tight enough to use it.
pub fn tight(s: &Sample) -> bool {
    s.tsc > 0 && s.reference > 0 && s.bracket.saturating_mul(BRACKET_LIMIT) <= s.tsc
}

/// The rates the samples give (`reference_hz`: the reference's rate), or
/// None when fewer than [`MIN_SAMPLES`] survive the filters.
pub fn estimate(samples: &[Sample], reference_hz: u64) -> Option<Estimate> {
    let samples = &samples[..samples.len().min(MAX_SAMPLES)];
    let mut tsc = [0u64; MAX_SAMPLES];
    let mut timer = [0u64; MAX_SAMPLES];
    let mut n = 0;
    for s in samples.iter().filter(|s| tight(s)) {
        tsc[n] = rate(s.tsc, s.reference, reference_hz);
        timer[n] = rate(s.timer, s.reference, reference_hz);
        n += 1;
    }
    let wide = samples.len() - n;
    let (mut tsc_sorted, mut timer_sorted) = (tsc, timer);
    let tsc_median = median(&mut tsc_sorted[..n]);
    let timer_median = median(&mut timer_sorted[..n]);
    let mut kept_tsc = [0u64; MAX_SAMPLES];
    let mut kept_timer = [0u64; MAX_SAMPLES];
    let mut used = 0;
    for i in 0..n {
        if near(tsc[i], tsc_median, OUTLIER_PPM) && near(timer[i], timer_median, OUTLIER_PPM) {
            kept_tsc[used] = tsc[i];
            kept_timer[used] = timer[i];
            used += 1;
        }
    }
    if used < MIN_SAMPLES {
        return None;
    }
    let tsc_hz = median(&mut kept_tsc[..used]);
    let timer_hz = median(&mut kept_timer[..used]);
    if tsc_hz == 0 || timer_hz == 0 {
        return None;
    }
    let spread = kept_tsc[used - 1] - kept_tsc[0];
    Some(Estimate {
        tsc_hz,
        timer_hz,
        used,
        wide,
        outliers: n - used,
        spread_ppm: (u128::from(spread) * 1_000_000 / u128::from(tsc_hz)) as u64,
    })
}

/// A CPUID-reported TSC rate is believed when the measurement agrees
/// within this (ppm): an emulator may report a rate its TSC does not run
/// at (QEMU's TCG runs the guest's TSC at the host's).
pub const CPUID_AGREE_PPM: u64 = 10_000;

/// The TSC rate to use: CPUID's when it agrees with the measurement
/// (more precise), else the measurement's.
pub fn choose(measured: u64, cpuid: Option<u64>) -> (u64, bool) {
    match cpuid {
        Some(c) if c > 0 && near(c, measured, CPUID_AGREE_PPM) => (c, true),
        _ => (measured, false),
    }
}

/// When an RTC second began, as seen by polling: after the TSC read
/// before the last poll that still showed the old second, before the TSC
/// read after the first poll that showed the new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub after: u64,
    pub before: u64,
}

/// What the RTC says about the TSC's calibrated rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RtcVerdict {
    /// Within [`RTC_AGREE_PPM`] of what the RTC allows.
    Agrees { measured_hz: u64, off_ppm: i64 },
    /// Outside the RTC's bounds by more than [`RTC_WRONG_PPM`].
    Wrong { measured_hz: u64, off_ppm: i64 },
    /// Not yet clear (the polls were too coarse for this interval).
    Unsure,
}

/// The calibrated rate may be this far (ppm) outside the RTC's bounds and
/// still agree.
pub const RTC_AGREE_PPM: u64 = 10_000;
/// Further than this outside the RTC's bounds is wrong (3%).
pub const RTC_WRONG_PPM: u64 = 30_000;
/// Agreement needs the bounds this tight (ppm of their middle).
pub const RTC_PRECISION_PPM: u64 = 20_000;

/// Judges `calibrated_hz` by two RTC second edges `seconds` apart.
pub fn rtc_verdict(calibrated_hz: u64, first: Edge, last: Edge, seconds: u64) -> RtcVerdict {
    if seconds == 0 {
        return RtcVerdict::Unsure;
    }
    // The true interval lies between the inner and the outer bounds.
    let inner = last.after.saturating_sub(first.before);
    let outer = last.before.saturating_sub(first.after);
    let (lo, hi) = (inner / seconds, outer / seconds);
    let mid = (lo + hi) / 2;
    if mid == 0 {
        return RtcVerdict::Unsure;
    }
    let off_ppm =
        ((i128::from(calibrated_hz) - i128::from(mid)) * 1_000_000 / i128::from(mid)) as i64;
    let widen = |x: u64, ppm: u64| (u128::from(x) * u128::from(ppm) / 1_000_000) as u64;
    let outside = |ppm: u64| {
        calibrated_hz < lo.saturating_sub(widen(lo, ppm)) || calibrated_hz > hi + widen(hi, ppm)
    };
    if outside(RTC_WRONG_PPM) {
        return RtcVerdict::Wrong { measured_hz: mid, off_ppm };
    }
    let precise =
        u128::from(hi - lo) * 1_000_000 <= u128::from(mid) * u128::from(RTC_PRECISION_PPM);
    if precise && !outside(RTC_AGREE_PPM) {
        return RtcVerdict::Agrees { measured_hz: mid, off_ppm };
    }
    RtcVerdict::Unsure
}

/// Whole seconds from RTC second `from` to `to` (0-59 each; an interval
/// under a minute).
pub fn rtc_seconds_between(from: u8, to: u8) -> u64 {
    u64::from((u16::from(to) + 60 - u16::from(from) % 60) % 60)
}

/// A rate as "2800.1 MHz" parts: (MHz, tenths).
pub fn mhz(hz: u64) -> (u64, u64) {
    (hz / 1_000_000, hz / 100_000 % 10)
}

/// Parts per million as "0.25%" parts: (whole percent, hundredths).
pub fn percent(ppm: u64) -> (u64, u64) {
    (ppm / 10_000, ppm / 100 % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REF_HZ: u64 = 100_000_000; // an HPET
    const TSC_HZ: u64 = 2_800_000_000;
    const TIMER_HZ: u64 = 62_500_000;

    /// A 10 ms sample with `factor` ppm error and a bracket of `bracket`
    /// TSC ticks.
    fn sample(error_ppm: i64, bracket: u64) -> Sample {
        let reference = REF_HZ / 100;
        let scale =
            |hz: u64| ((hz / 100) as i64 + (hz / 100) as i64 * error_ppm / 1_000_000) as u64;
        Sample { tsc: scale(TSC_HZ), timer: scale(TIMER_HZ), reference, bracket }
    }

    #[test]
    fn clean_samples() {
        let s = [
            sample(0, 1000),
            sample(100, 900),
            sample(-100, 1200),
            sample(50, 800),
            sample(0, 700),
        ];
        let e = estimate(&s, REF_HZ).unwrap();
        assert_eq!((e.used, e.wide, e.outliers), (5, 0, 0));
        assert!(e.tsc_hz.abs_diff(TSC_HZ) < TSC_HZ / 10_000);
        assert!(e.timer_hz.abs_diff(TIMER_HZ) < TIMER_HZ / 10_000);
        assert_eq!(e.spread_ppm / 10, 20);
    }

    #[test]
    fn a_pause_inside_a_bracket_rejects_the_sample() {
        // The vCPU paused 5 ms between reading the reference and the TSC:
        // the bracket is half the window, and the sample's figures are off.
        let paused = Sample { tsc: TSC_HZ / 200, timer: TIMER_HZ / 200, ..sample(0, TSC_HZ / 200) };
        let s = [paused, sample(0, 1000), paused, sample(10, 1000), sample(-10, 1000)];
        let e = estimate(&s, REF_HZ).unwrap();
        assert_eq!((e.used, e.wide, e.outliers), (3, 2, 0));
        assert!(e.tsc_hz.abs_diff(TSC_HZ) < TSC_HZ / 10_000);
    }

    #[test]
    fn outliers_are_dropped() {
        // A PIT reference that wrapped unseen: one sample 20x off with a
        // tight bracket.
        let s =
            [sample(0, 10), sample(19_000_000, 10), sample(30, 10), sample(-30, 10), sample(0, 10)];
        let e = estimate(&s, REF_HZ).unwrap();
        assert_eq!((e.used, e.wide, e.outliers), (4, 0, 1));
        assert!(e.tsc_hz.abs_diff(TSC_HZ) < TSC_HZ / 10_000);
        // The timer's rate alone off rejects the sample too.
        let mut odd = sample(0, 10);
        odd.timer *= 2;
        let e = estimate(&[odd, sample(0, 10), sample(0, 10), sample(0, 10)], REF_HZ).unwrap();
        assert_eq!((e.used, e.outliers), (3, 1));
    }

    #[test]
    fn too_few_samples() {
        assert_eq!(estimate(&[], REF_HZ), None);
        assert_eq!(estimate(&[sample(0, 10), sample(0, 10)], REF_HZ), None);
        let wide = sample(0, TSC_HZ);
        assert_eq!(estimate(&[wide, wide, wide, sample(0, 10)], REF_HZ), None);
        // Nothing agrees with anything.
        let s = [sample(0, 10), sample(100_000, 10), sample(300_000, 10)];
        assert_eq!(estimate(&s, REF_HZ), None);
        // Zero reference ticks never divide.
        let zero = Sample { reference: 0, ..sample(0, 10) };
        assert_eq!(estimate(&[zero, zero, zero], REF_HZ), None);
    }

    #[test]
    fn cpuid_rate_only_when_it_agrees() {
        assert_eq!(choose(2_800_100_000, Some(2_800_000_000)), (2_800_000_000, true));
        assert_eq!(choose(2_800_000_000, Some(1_000_000_000)), (2_800_000_000, false));
        assert_eq!(choose(2_800_000_000, None), (2_800_000_000, false));
        assert_eq!(choose(2_800_000_000, Some(0)), (2_800_000_000, false));
    }

    fn edge(at: u64, width: u64) -> Edge {
        Edge { after: at, before: at + width }
    }

    #[test]
    fn rtc_agrees_with_a_right_clock() {
        // Edges 3 s apart, polled to 1 ms.
        let (first, last) =
            (edge(1_000_000, TSC_HZ / 1000), edge(1_000_000 + 3 * TSC_HZ, TSC_HZ / 1000));
        match rtc_verdict(TSC_HZ, first, last, 3) {
            RtcVerdict::Agrees { measured_hz, off_ppm } => {
                assert!(measured_hz.abs_diff(TSC_HZ) < TSC_HZ / 1000);
                assert!(off_ppm.abs() < 1000);
            }
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn rtc_catches_a_wrong_clock() {
        // Calibrated at 135 MHz: the RTC sees 2.8 GHz worth of ticks a
        // second, even with coarse polls (100 ms) over 1 s.
        let (first, last) = (edge(0, TSC_HZ / 10), edge(TSC_HZ, TSC_HZ / 10));
        match rtc_verdict(135_000_000, first, last, 1) {
            RtcVerdict::Wrong { measured_hz, off_ppm } => {
                assert!(measured_hz.abs_diff(TSC_HZ) < TSC_HZ / 5);
                assert!(off_ppm < -900_000);
            }
            v => panic!("{v:?}"),
        }
        // 5% fast over 4 s polled to 1 ms.
        let (first, last) = (edge(0, TSC_HZ / 1000), edge(4 * TSC_HZ, TSC_HZ / 1000));
        assert!(matches!(
            rtc_verdict(TSC_HZ * 105 / 100, first, last, 4),
            RtcVerdict::Wrong { .. }
        ));
    }

    #[test]
    fn rtc_unsure_while_coarse() {
        // A right clock, but polls 100 ms apart over 1 s: bounds +-10%.
        let (first, last) = (edge(0, TSC_HZ / 10), edge(TSC_HZ, TSC_HZ / 10));
        assert_eq!(rtc_verdict(TSC_HZ, first, last, 1), RtcVerdict::Unsure);
        assert_eq!(rtc_verdict(TSC_HZ, first, last, 0), RtcVerdict::Unsure);
    }

    #[test]
    fn rtc_seconds_wrap() {
        assert_eq!(rtc_seconds_between(10, 13), 3);
        assert_eq!(rtc_seconds_between(58, 1), 3);
        assert_eq!(rtc_seconds_between(7, 7), 0);
    }

    #[test]
    fn formatting_parts() {
        assert_eq!(mhz(2_811_234_567), (2811, 2));
        assert_eq!(percent(2500), (0, 25));
        assert_eq!(percent(1_234_567), (123, 45));
    }
}
