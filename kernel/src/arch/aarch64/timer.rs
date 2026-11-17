//! The tick source and the counter's rate: the generic timer's virtual
//! channel (CNTV, PPI 27 at the GIC), its counter CNTVCT_EL0 at the rate
//! the firmware put in CNTFRQ_EL0, which is architected: nothing to
//! calibrate. The tick is periodic by compare value (CNTV_CVAL_EL0 moved
//! on by one period at each tick, so the period does not drift with the
//! handler's latency).
//!
//! Every CPU has its own virtual timer (a PPI is private): the boot CPU's
//! keeps time (`start_periodic`), an AP's only preempts
//! (`start_periodic_here`), both calling the same handler.
//!
//! The clock check's wall-clock reference is the PL031 RTC (`rtc_seconds`:
//! its data register counts seconds), QEMU `virt`'s at 0x0901_0000, the
//! board's fixed layout like the UART's and the GIC's (ROADMAP AR: from
//! the firmware). CNTFRQ_EL0 is the firmware's statement of the rate, not
//! a measurement (`reliable` is false): when the RTC measures otherwise,
//! the RTC's rate wins (`time::recalibrate`).
//!
//! Limits: entered at EL2 with VHE the CNTV registers would be EL2's
//! virtual timer (PPI 28), not handled.

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use super::interrupts;
use crate::boot::BootInfo;
use crate::sync::IrqMutex;

/// The cycle counter's and the tick timer's names in the clock's log.
pub const COUNTER_NAME: &str = "generic counter";
pub const TIMER_NAME: &str = "generic timer";

/// The PL031 RTC and its data register (seconds since it was set).
const PL031_PHYS: u64 = 0x0901_0000;
const RTCDR: u64 = 0x000;
/// The RTC, mapped (0: not yet, or the mapping failed).
static RTC: AtomicU64 = AtomicU64::new(0);

/// The virtual timer's interrupt (a PPI).
const VIRTUAL_TIMER_PPI: u32 = 27;
/// CNTV_CTL_EL0: ENABLE (IMASK clear).
const CTL_ENABLE: u64 = 1;

/// Counter ticks per timer tick (0: not started).
static PERIOD: AtomicU64 = AtomicU64::new(0);
/// Each CPU's next compare value, by CPU index.
static NEXT: [AtomicU64; super::smp::MAX_CPUS] =
    [const { AtomicU64::new(0) }; super::smp::MAX_CPUS];

fn next_slot() -> &'static AtomicU64 {
    &NEXT[(super::percpu::cpu_index() as usize).min(super::smp::MAX_CPUS - 1)]
}
static HANDLER: IrqMutex<Option<interrupts::Handler>> = IrqMutex::new(None);

fn counter_frequency() -> u64 {
    let f: u64;
    // SAFETY: reads CNTFRQ_EL0.
    unsafe {
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) f, options(nomem, nostack, preserves_flags))
    };
    f
}

fn set_compare(value: u64) {
    // SAFETY: programs this CPU's virtual timer (compare value, enabled).
    unsafe {
        core::arch::asm!(
            "msr cntv_cval_el0, {cval}",
            "msr cntv_ctl_el0, {ctl}",
            "isb",
            cval = in(reg) value,
            ctl = in(reg) CTL_ENABLE,
            options(nomem, nostack, preserves_flags)
        )
    };
}

/// The timer's IRQ: the next compare value one period on (from the last,
/// or from now if the handler fell more than a period behind), then the
/// tick's handler.
fn on_interrupt(frame: &mut interrupts::TrapFrame) {
    let period = PERIOD.load(Ordering::Relaxed);
    let now = super::cpu::cycles();
    let slot = next_slot();
    let mut next = slot.load(Ordering::Relaxed) + period;
    if next <= now {
        next = now + period;
    }
    slot.store(next, Ordering::Relaxed);
    set_compare(next);
    let handler = *HANDLER.lock();
    if let Some(h) = handler {
        h(frame);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Calibration {
    counter_hz: u64,
    timer_hz: u64,
}

/// The counter's and the timer's rate: CNTFRQ_EL0's (one counter drives
/// both). Nothing is measured, so no stall applies; a kernel handover's
/// carried rates are the same counter's.
pub fn calibrate(_info: &BootInfo, _stall: bool, carried: Option<(u64, u64)>) -> Calibration {
    map_rtc();
    let hz = counter_frequency();
    match carried {
        Some((counter_hz, timer_hz)) => Calibration { counter_hz, timer_hz },
        None => Calibration { counter_hz: hz, timer_hz: hz },
    }
}

/// The architected rate again (what `calibrate` takes): the clock check
/// found the rate in use wrong.
pub fn recalibrate(_old: &Calibration) -> Option<Calibration> {
    let hz = counter_frequency();
    Some(Calibration { counter_hz: hz, timer_hz: hz })
}

fn map_rtc() {
    match super::paging::map_mmio(PL031_PHYS, 0x1000) {
        Ok(virt) => RTC.store(virt, Ordering::Release),
        Err(e) => crate::warn!(
            "timer: the PL031 RTC at {PL031_PHYS:#x} not mapped ({e:?}): the clock is not checked"
        ),
    }
}

/// Starts the boot CPU's tick at `hz`, calling `handler` at each (the GIC
/// is brought up first: the timer is its first source).
pub fn start_periodic(c: &Calibration, hz: u64, handler: interrupts::Handler) {
    if let Err(e) = super::gic::init() {
        panic!("timer: no interrupt controller: {e:?}");
    }
    // The console's break (a state dump's request) from now on.
    super::gic::enable_console();
    *HANDLER.lock() = Some(handler);
    let period = (c.timer_hz() / hz.max(1)).max(1);
    PERIOD.store(period, Ordering::Relaxed);
    super::gic::enable_private(VIRTUAL_TIMER_PPI, on_interrupt);
    let first = super::cpu::cycles() + period;
    next_slot().store(first, Ordering::Relaxed);
    set_compare(first);
}

/// Starts (or, after a recalibration, reprograms) this CPU's tick at
/// `hz` from `c`'s rate, with the handler `start_periodic` installed on
/// the boot CPU (an AP's PPI was enabled by the GIC's `init_cpu`).
pub fn start_periodic_here(c: &Calibration, hz: u64) {
    if PERIOD.load(Ordering::Relaxed) == 0 {
        crate::warn!(
            "timer: CPU {}'s tick not started: none on the boot CPU",
            super::percpu::cpu_index()
        );
        return;
    }
    // The same for every CPU: each stores what the others do.
    let period = (c.timer_hz() / hz.max(1)).max(1);
    PERIOD.store(period, Ordering::Relaxed);
    let first = super::cpu::cycles() + period;
    next_slot().store(first, Ordering::Relaxed);
    set_compare(first);
}

pub fn eoi() {}

/// Stops this CPU's tick (the virtual timer disabled): a kernel handover,
/// whose next kernel starts its own.
pub fn stop_here() {
    // SAFETY: disables this CPU's virtual timer; nothing else changes.
    unsafe {
        core::arch::asm!("msr cntv_ctl_el0, xzr", "isb", options(nomem, nostack, preserves_flags))
    };
}

/// The wall clock's seconds within its minute (the PL031's count), None
/// without the RTC: the clock check's reference.
pub fn rtc_seconds() -> Option<u8> {
    let rtc = RTC.load(Ordering::Acquire);
    if rtc == 0 {
        return None;
    }
    // SAFETY: the PL031's data register in the mapping `map_rtc` made
    // (Device memory); reading it has no side effect.
    let seconds = unsafe { ((rtc + RTCDR) as *const u32).read_volatile() };
    Some((seconds % 60) as u8)
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

    /// CNTFRQ_EL0 is the firmware's statement, not a measurement.
    pub fn reliable(&self) -> bool {
        false
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

    /// The boot log's line: the counter's rate and where it came from.
    pub fn log(&self) {
        crate::info!(
            "timer: generic timer (virtual channel, PPI {VIRTUAL_TIMER_PPI}), counter at {} kHz \
             ({}, architected: not measured)",
            self.counter_hz / 1000,
            self.source_name()
        );
    }

    pub fn log_started(&self) {}

    pub fn emit_event(&self) {}
}
