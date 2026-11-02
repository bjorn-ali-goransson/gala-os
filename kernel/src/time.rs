//! Kernel timekeeping: a periodic tick and a TSC-based monotonic clock.
//!
//! `ticks`, `monotonic_ns` and `system_time` are the clock programs and
//! wait deadlines use: with `clock=` (clock control, `clockctl`) it is the
//! virtual clock, derived from the TSC; otherwise the real one. The
//! kernel's own diagnostics and sleeps (watchdogs, the flight recorder,
//! `sched::sleep_ms`) use `raw_ticks` and `raw_monotonic_ns`, which
//! clock control never moves.

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

/// Sets the boot time as an NT system time (a replayed recording's).
pub fn set_boot_system_time(t: u64) {
    SYSTEM_TIME_AT_INIT.store(t, Ordering::Relaxed);
}

/// NT system time when the kernel's clock started (the boot time).
pub fn boot_system_time() -> u64 {
    SYSTEM_TIME_AT_INIT.load(Ordering::Relaxed)
}

/// Current NT system time (100 ns units since 1601-01-01 UTC).
pub fn system_time() -> u64 {
    let t = SYSTEM_TIME_AT_INIT.load(Ordering::Relaxed) + monotonic_ns() / 100;
    if crate::clockctl::armed() {
        return crate::clockctl::offset_system(t, crate::clockctl::system_offset());
    }
    t
}

/// Starts the periodic tick. Interrupts must be enabled by the caller.
pub fn init(calibration: &apic::Calibration) {
    *CALIBRATION.lock() = Some(*calibration);
    TSC_HZ.store(calibration.tsc_hz, Ordering::Relaxed);
    TSC_AT_BOOT.store(cpu::rdtsc(), Ordering::Relaxed);
    apic::start_periodic_timer(calibration, TICK_HZ, on_tick);
}

static CALIBRATION: crate::sync::IrqMutex<Option<apic::Calibration>> =
    crate::sync::IrqMutex::new(None);

/// Starts the tick on a CPU other than the boot CPU (it only preempts; the
/// boot CPU's tick keeps time).
pub fn start_tick_here() {
    let calibration = *CALIBRATION.lock();
    if let Some(c) = calibration {
        apic::start_periodic_timer_here(&c, TICK_HZ);
    }
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

/// Per CPU: the thread (its id + 1; 0 for none) and the user rip its last
/// tick interrupted in user mode, for thread dumps (where a thread
/// spinning in user mode is).
static USER_RIP: [(AtomicU64, AtomicU64); crate::arch::smp::MAX_CPUS] =
    [const { (AtomicU64::new(0), AtomicU64::new(0)) }; crate::arch::smp::MAX_CPUS];

/// Where `thread` was in user mode at the last tick that interrupted it
/// there, if a CPU's latest such tick was its.
pub fn last_user_rip(thread: crate::sched::ThreadId) -> Option<u64> {
    let want = thread.as_usize() as u64 + 1;
    USER_RIP
        .iter()
        .find(|(t, _)| t.load(Ordering::Relaxed) == want)
        .map(|(_, rip)| rip.load(Ordering::Relaxed))
}

/// `hangtest`: CPU 1 spins with interrupts off for 9 s, 3 s after its
/// timer starts (the hang watchdog's scenario).
static HANG_TEST: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn set_hang_test(on: bool) {
    HANG_TEST.store(on, Ordering::Relaxed);
}

fn on_tick(frame: &mut TrapFrame) {
    crate::flight::on_tick(TICK_HZ);
    // A thread that stays in the kernel without blocking (the watchdog);
    // user mode is progress.
    if let Some((id, idle)) = crate::sched::current_if_running() {
        if frame.from_user_mode() {
            crate::flight::progress(id.as_usize());
        } else if !idle {
            crate::flight::on_kernel_tick(id.as_usize(), frame.rip);
        }
    }
    if crate::arch::percpu::cpu_index() == 1 && HANG_TEST.load(Ordering::Relaxed) {
        static BEATS: AtomicU64 = AtomicU64::new(0);
        if BEATS.fetch_add(1, Ordering::Relaxed) == 3 * TICK_HZ {
            HANG_TEST.store(false, Ordering::Relaxed);
            crate::info!("hangtest: CPU 1 spins with interrupts off for 9 s");
            let until = raw_monotonic_ns() + 9_000_000_000;
            while raw_monotonic_ns() < until {
                core::hint::spin_loop();
            }
            crate::info!("hangtest: CPU 1 goes on");
        }
    }
    if frame.from_user_mode() {
        let cpu = crate::arch::percpu::cpu_index() as usize;
        if let Some((t, rip)) = USER_RIP.get(cpu) {
            t.store(crate::sched::current().as_usize() as u64 + 1, Ordering::Relaxed);
            rip.store(frame.rip, Ordering::Relaxed);
        }
    }
    if crate::arch::percpu::cpu_index() != 0 {
        apic::eoi();
        crate::sched::on_tick_here();
        return;
    }
    let raw = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    apic::eoi();
    let now = if crate::clockctl::armed() { ticks() } else { raw };
    let callback = *TICK_CALLBACK.lock();
    if let Some(callback) = callback {
        callback(now);
    }
    crate::sched::on_tick(now, raw);
}

/// Timer ticks since `init` on the programs' clock (virtual under clock
/// control: its milliseconds).
pub fn ticks() -> u64 {
    if crate::clockctl::armed() {
        return monotonic_ns() / (1_000_000_000 / TICK_HZ);
    }
    TICKS.load(Ordering::Relaxed)
}

/// Timer interrupts since `init` (real time, whatever clock control does).
pub fn raw_ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Nanoseconds since `init` on the programs' clock (virtual under clock
/// control).
pub fn monotonic_ns() -> u64 {
    crate::clockctl::virtual_ns(raw_monotonic_ns())
}

/// Turns clock control's virtual clock on (`clock=`), continuing from
/// the current time: the larger of the TSC's and the tick count's, so
/// neither `monotonic_ns` nor `ticks` goes backwards.
pub fn arm_clock_control() {
    let raw = raw_monotonic_ns();
    let from_ticks = TICKS.load(Ordering::Relaxed) * (1_000_000_000 / TICK_HZ);
    crate::clockctl::arm(raw, raw.max(from_ticks), boot_system_time(), None);
}

/// Nanoseconds since `init`, from the TSC (0 before calibration); real
/// time, whatever clock control does.
pub fn raw_monotonic_ns() -> u64 {
    let hz = TSC_HZ.load(Ordering::Relaxed);
    if hz == 0 {
        return 0;
    }
    let delta = cpu::rdtsc().wrapping_sub(TSC_AT_BOOT.load(Ordering::Relaxed));
    (u128::from(delta) * 1_000_000_000 / u128::from(hz)) as u64
}
