//! Kernel timekeeping: a periodic tick and a TSC-based monotonic clock.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::{apic, cpu, interrupts::TrapFrame};

/// Timer interrupt rate.
pub const TICK_HZ: u64 = 1000;

static TICKS: AtomicU64 = AtomicU64::new(0);
static TSC_HZ: AtomicU64 = AtomicU64::new(0);
static TSC_AT_BOOT: AtomicU64 = AtomicU64::new(0);
/// NT system time (100 ns units since 1601-01-01 UTC) at `init`.
static SYSTEM_TIME_AT_INIT: AtomicU64 = AtomicU64::new(0);

/// Seconds between 1601-01-01 and 1970-01-01.
const EPOCH_DIFFERENCE_SECS: u64 = 11_644_473_600;

/// Sets the wall clock from the UNIX time read at boot.
pub fn set_boot_time(unix_secs: i64) {
    let secs = (unix_secs.max(0) as u64) + EPOCH_DIFFERENCE_SECS;
    SYSTEM_TIME_AT_INIT.store(secs * 10_000_000, Ordering::Relaxed);
}

/// NT system time when the kernel's clock started (the boot time).
pub fn boot_system_time() -> u64 {
    SYSTEM_TIME_AT_INIT.load(Ordering::Relaxed)
}

/// Current NT system time (100 ns units since 1601-01-01 UTC).
pub fn system_time() -> u64 {
    SYSTEM_TIME_AT_INIT.load(Ordering::Relaxed) + monotonic_ns() / 100
}

/// Starts the periodic tick. Interrupts must be enabled by the caller.
pub fn init(calibration: &apic::Calibration) {
    TSC_HZ.store(calibration.tsc_hz, Ordering::Relaxed);
    TSC_AT_BOOT.store(cpu::rdtsc(), Ordering::Relaxed);
    apic::start_periodic_timer(calibration, TICK_HZ, on_tick);
}

/// Performance counter frequency: 10 MHz (100 ns resolution), as on
/// current Windows.
pub const PERFORMANCE_FREQUENCY: u64 = 10_000_000;

static TICK_CALLBACK: crate::sync::IrqMutex<Option<fn(u64)>> = crate::sync::IrqMutex::new(None);

/// Registers a function called on every tick with the tick count (in
/// interrupt context, before scheduling decisions).
pub fn set_tick_callback(callback: fn(u64)) {
    *TICK_CALLBACK.lock() = Some(callback);
}

fn on_tick(_frame: &mut TrapFrame) {
    let now = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    apic::eoi();
    let callback = *TICK_CALLBACK.lock();
    if let Some(callback) = callback {
        callback(now);
    }
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
