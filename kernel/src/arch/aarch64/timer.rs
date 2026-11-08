//! The tick source and the cycle counter's rate (stub: the generic timer,
//! CNTV with CNTFRQ_EL0's rate, needs no calibration once ported).

use core::fmt;

use super::{interrupts, unported};
use crate::boot::BootInfo;

#[derive(Clone, Copy, Debug)]
pub struct Calibration {
    counter_hz: u64,
    timer_hz: u64,
}

pub fn calibrate(_info: &BootInfo, _stall: bool, _carried: Option<(u64, u64)>) -> Calibration {
    unported("timer::calibrate")
}

pub fn recalibrate(_old: &Calibration) -> Option<Calibration> {
    None
}

pub fn start_periodic(_c: &Calibration, _hz: u64, _handler: interrupts::Handler) {
    unported("timer::start_periodic")
}

pub fn start_periodic_here(_c: &Calibration, _hz: u64) {
    unported("timer::start_periodic_here")
}

pub fn eoi() {}

/// The wall clock's seconds (a PL031 RTC).
pub fn rtc_seconds() -> Option<u8> {
    None
}

impl Calibration {
    pub fn counter_hz(&self) -> u64 {
        self.counter_hz
    }

    pub fn timer_hz(&self) -> u64 {
        self.timer_hz
    }

    pub fn set_rates(&mut self, counter_hz: u64, timer_hz: u64) {
        (self.counter_hz, self.timer_hz) = (counter_hz, timer_hz);
    }

    pub fn rescale(&mut self, counter_hz: u64) {
        self.timer_hz = (u128::from(self.timer_hz) * u128::from(counter_hz)
            / u128::from(self.counter_hz.max(1))) as u64;
        self.counter_hz = counter_hz;
    }

    pub fn reliable(&self) -> bool {
        true
    }

    pub fn source_name(&self) -> &'static str {
        "CNTFRQ_EL0"
    }

    pub fn stalls(&self) -> u32 {
        0
    }

    pub fn describe(&self) -> impl fmt::Display + '_ {
        "the architected rate"
    }

    pub fn log(&self) {}

    pub fn log_started(&self) {}

    pub fn emit_event(&self) {}
}
