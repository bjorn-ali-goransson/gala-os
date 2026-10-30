//! Kernel timekeeping: a periodic tick and a TSC-based monotonic clock.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::{apic, cpu, interrupts::TrapFrame};

/// Timer interrupt rate.
pub const TICK_HZ: u64 = 1000;

static TICKS: AtomicU64 = AtomicU64::new(0);
static TSC_HZ: AtomicU64 = AtomicU64::new(0);
static TSC_AT_BOOT: AtomicU64 = AtomicU64::new(0);

/// Starts the periodic tick. Interrupts must be enabled by the caller.
pub fn init(calibration: &apic::Calibration) {
    TSC_HZ.store(calibration.tsc_hz, Ordering::Relaxed);
    TSC_AT_BOOT.store(cpu::rdtsc(), Ordering::Relaxed);
    apic::start_periodic_timer(calibration, TICK_HZ, on_tick);
}

fn on_tick(_frame: &mut TrapFrame) {
    let now = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    apic::eoi();
    crate::sched::on_tick(now);
}

/// Timer ticks since `init`.
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Nanoseconds since `init`, from the TSC (0 before calibration).
pub fn monotonic_ns() -> u64 {
    let hz = TSC_HZ.load(Ordering::Relaxed);
    if hz == 0 {
        return 0;
    }
    let delta = cpu::rdtsc().wrapping_sub(TSC_AT_BOOT.load(Ordering::Relaxed));
    (u128::from(delta) * 1_000_000_000 / u128::from(hz)) as u64
}
