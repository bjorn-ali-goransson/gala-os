//! The tick source and the cycle counter's rate, as `crate::time` uses
//! them (architecture-neutral): a periodic timer interrupt on every CPU,
//! started from a calibration that also gives the cycle counter's
//! (`cpu::cycles`) rate, a wall-clock seconds reference for the clock
//! check, and the calibration's log line and event. x86-64: the local
//! APIC's timer and the TSC, calibrated against the HPET or the ACPI PM
//! timer (`apic`, `refclock`).

use core::fmt;

use super::apic::{self, CalibrationInputs};
use super::{bootphase, interrupts, refclock};
use crate::boot::BootInfo;
use crate::clockcal::{self, Mhz};

/// How the tick source and the cycle counter were calibrated.
#[derive(Clone, Copy, Debug)]
pub struct Calibration(apic::Calibration);

/// Calibrates and starts nothing yet: the boot CPU's timer and cycle
/// counter measured against the firmware's reference clock (`info`'s
/// ACPI tables), or the rates a kernel handover carried (`carried`:
/// cycle counter, timer). `stall` (`calstall`): the calibration's test
/// stalls.
pub fn calibrate(info: &BootInfo, stall: bool, carried: Option<(u64, u64)>) -> Calibration {
    let inputs = CalibrationInputs {
        hpet: info.clock_tables.hpet,
        pm_timer: info.clock_tables.pm_timer.map(|t| (t.port, t.bits32)),
        stall,
        carried,
    };
    match apic::init_bsp(inputs) {
        Ok(c) => Calibration(c),
        Err(e) => panic!("local APIC: {e:?}"),
    }
}

/// Measures again on this (the boot) CPU, with the inputs of `old`.
pub fn recalibrate(old: &Calibration) -> Option<Calibration> {
    apic::recalibrate(&old.0).ok().map(Calibration)
}

/// Starts the periodic tick (`hz`) on this CPU, `handler` its interrupt.
pub fn start_periodic(c: &Calibration, hz: u64, handler: interrupts::Handler) {
    apic::start_periodic_timer(&c.0, hz, handler)
}

/// Starts (or reprograms) this CPU's periodic tick with the handler the
/// boot CPU installed.
pub fn start_periodic_here(c: &Calibration, hz: u64) {
    apic::start_periodic_timer_here(&c.0, hz)
}

/// Ends the tick interrupt being handled.
pub fn eoi() {
    apic::eoi()
}

/// The wall clock's seconds (the CMOS RTC), None while it updates: the
/// clock check's reference.
pub fn rtc_seconds() -> Option<u8> {
    refclock::rtc_seconds()
}

impl Calibration {
    /// The cycle counter's rate (TSC ticks per second).
    pub fn counter_hz(&self) -> u64 {
        self.0.tsc_hz
    }

    /// The tick source's rate (APIC timer ticks per second).
    pub fn timer_hz(&self) -> u64 {
        self.0.timer_hz
    }

    /// Replaces both rates (a test's wrong calibration, a handover's
    /// carried rates).
    pub fn set_rates(&mut self, counter_hz: u64, timer_hz: u64) {
        (self.0.tsc_hz, self.0.timer_hz) = (counter_hz, timer_hz);
    }

    /// Takes `counter_hz` (a reference measured it) and scales the timer's
    /// rate alike.
    pub fn rescale(&mut self, counter_hz: u64) {
        self.0.timer_hz = (u128::from(self.0.timer_hz) * u128::from(counter_hz)
            / u128::from(self.0.tsc_hz.max(1))) as u64;
        self.0.tsc_hz = counter_hz;
    }

    /// Whether enough good samples agreed (else the best one was used).
    pub fn reliable(&self) -> bool {
        self.0.report.estimate.is_some()
    }

    /// What it was measured against.
    pub fn source_name(&self) -> &'static str {
        self.0.report.reference.name()
    }

    /// The calibration's test stalls (`calstall`).
    pub fn stalls(&self) -> u32 {
        self.0.report.stalls
    }

    /// How the samples went (logs).
    pub fn describe(&self) -> impl fmt::Display + '_ {
        How(&self.0.report)
    }

    /// Logs how the clock was calibrated (`tsc:` line, `phase tsc_mhz`).
    pub fn log(&self) {
        let (c, r) = (&self.0, &self.0.report);
        let source = if r.cpuid_used { "CPUID" } else { r.reference.name() };
        let stall = if r.stalls > 0 { " (calstall: stalls injected)" } else { "" };
        if r.estimate.is_some() || r.reference == refclock::Kind::Carried {
            crate::info!(
                "tsc: {} from {source} ({} kHz; measured {}), {}; APIC timer {} kHz{stall}",
                Mhz(c.tsc_hz),
                r.reference_hz / 1000,
                Mhz(r.measured_tsc_hz),
                How(r),
                c.timer_hz / 1000
            );
        } else {
            crate::warn!(
                "tsc: {} from {source} ({} kHz; measured {}), {}; APIC timer {} kHz{stall}",
                Mhz(c.tsc_hz),
                r.reference_hz / 1000,
                Mhz(r.measured_tsc_hz),
                How(r),
                c.timer_hz / 1000
            );
        }
        bootphase::mark_n("tsc_mhz", c.tsc_hz / 1_000_000);
    }

    /// Logs the tick source as started (`apic:` line).
    pub fn log_started(&self) {
        let c = &self.0;
        crate::info!(
            "apic: id {} ({}), timer {} kHz, TSC {} MHz",
            c.apic_id,
            if c.x2apic { "x2APIC" } else { "xAPIC" },
            c.timer_hz / 1000,
            c.tsc_hz / 1_000_000
        );
    }

    /// The calibration as an event (`clock_cal`, category `clock`).
    pub fn emit_event(&self) {
        let (c, r) = (&self.0, &self.0.report);
        let (used, wide, outliers, spread) =
            r.estimate.map_or((0, 0, 0, 0), |e| (e.used, e.wide, e.outliers, e.spread_ppm));
        crate::events::emit(
            crate::events::Category::Clock,
            "clock_cal",
            format_args!(
                r#""source":{},"ref_hz":{},"tsc_hz":{},"measured_tsc_hz":{},"timer_hz":{},"samples":{},"used":{used},"paused":{wide},"outliers":{outliers},"spread_ppm":{spread},"cpuid_hz":{},"cpuid_used":{},"reliable":{}"#,
                crate::events::Str(r.reference.name()),
                r.reference_hz,
                c.tsc_hz,
                r.measured_tsc_hz,
                c.timer_hz,
                r.samples,
                r.cpuid.map_or(0, |x| x.0),
                r.cpuid_used,
                r.estimate.is_some()
            ),
        );
    }
}

/// How the samples went.
struct How<'a>(&'a apic::CalibrationReport);

impl fmt::Display for How<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let r = self.0;
        if r.reference == refclock::Kind::Carried {
            return write!(f, "carried over by the kernel handover, not measured");
        }
        match r.estimate {
            Some(e) => {
                let (p, h) = clockcal::percent(e.spread_ppm);
                write!(
                    f,
                    "{} samples of {} ({} paused, {} outliers), spread {p}.{h:02}%",
                    e.used, r.samples, e.wide, e.outliers
                )
            }
            None => {
                write!(f, "UNRELIABLE: too few good samples of {}, the best one used", r.samples)
            }
        }?;
        match r.cpuid {
            Some((hz, leaf)) => write!(
                f,
                "; {leaf} says {} ({})",
                Mhz(hz),
                if r.cpuid_used { "used" } else { "disagrees, not used" }
            ),
            None => write!(f, "; CPUID: no TSC rate"),
        }
    }
}
