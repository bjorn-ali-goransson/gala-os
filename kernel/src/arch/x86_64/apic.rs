//! Local APIC: x2APIC (MSR access) when available, otherwise xAPIC (MMIO).
//! Provides EOI and the periodic timer, calibrated against a wall-clock
//! reference (`refclock`; the arithmetic in `crate::clockcal`).

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::refclock::{self, RefClock};
use super::{cpu, interrupts, paging};
use crate::clockcal::{self, Sample};

const IA32_APIC_BASE: u32 = 0x1b;
const APIC_BASE_ENABLE: u64 = 1 << 11;
const APIC_BASE_X2APIC: u64 = 1 << 10;
const X2APIC_MSR_BASE: u32 = 0x800;

// Register offsets (xAPIC MMIO byte offset; x2APIC MSR = 0x800 + offset / 16).
const REG_ID: u32 = 0x20;
const REG_EOI: u32 = 0xb0;
const REG_SPURIOUS: u32 = 0xf0;
const REG_LVT_TIMER: u32 = 0x320;
const REG_LVT_LINT1: u32 = 0x360;
const REG_TIMER_INITIAL: u32 = 0x380;
const REG_TIMER_CURRENT: u32 = 0x390;
const REG_TIMER_DIVIDE: u32 = 0x3e0;
const REG_ICR_LOW: u32 = 0x300;
const REG_ICR_HIGH: u32 = 0x310;
const ICR_DELIVERY_PENDING: u32 = 1 << 12;

const SPURIOUS_ENABLE: u32 = 1 << 8;
const LVT_MASKED: u32 = 1 << 16;
const LVT_TIMER_PERIODIC: u32 = 1 << 17;
/// LVT delivery mode NMI: the PC's NMI line (QEMU's monitor `nmi`
/// raises it on every CPU) comes in as an NMI, which the kernel answers
/// with a state dump (kernel/src/statedump.rs). Masked at reset.
const LVT_DELIVERY_NMI: u32 = 0b100 << 8;
const DIVIDE_BY_16: u32 = 0b0011;

pub const TIMER_VECTOR: u8 = 0x30;
pub const SPURIOUS_VECTOR: u8 = 0xff;

static X2APIC: AtomicBool = AtomicBool::new(false);
/// Virtual address of the xAPIC registers (0 in x2APIC mode).
static MMIO_BASE: AtomicU64 = AtomicU64::new(0);

fn read(reg: u32) -> u32 {
    if X2APIC.load(Ordering::Relaxed) {
        // SAFETY: x2APIC mode is enabled and `reg` is a valid register.
        unsafe { cpu::rdmsr(X2APIC_MSR_BASE + reg / 16) as u32 }
    } else {
        let addr = MMIO_BASE.load(Ordering::Relaxed) + u64::from(reg);
        // SAFETY: `MMIO_BASE` maps the 4 KiB xAPIC register page uncached.
        unsafe { core::ptr::with_exposed_provenance::<u32>(addr as usize).read_volatile() }
    }
}

fn write(reg: u32, value: u32) {
    if X2APIC.load(Ordering::Relaxed) {
        // SAFETY: as in `read`; APIC register writes do not touch memory.
        unsafe { cpu::wrmsr(X2APIC_MSR_BASE + reg / 16, u64::from(value)) }
    } else {
        let addr = MMIO_BASE.load(Ordering::Relaxed) + u64::from(reg);
        // SAFETY: as in `read`.
        unsafe {
            core::ptr::with_exposed_provenance_mut::<u32>(addr as usize).write_volatile(value)
        }
    }
}

#[derive(Debug)]
pub enum ApicError {
    NotPresent,
    Map(paging::PagingError),
    CalibrationFailed,
}

/// Timer and TSC rates measured at boot.
#[derive(Clone, Copy, Debug)]
pub struct Calibration {
    pub x2apic: bool,
    pub apic_id: u32,
    /// LAPIC timer ticks per second (after the divide-by-16).
    pub timer_hz: u64,
    pub tsc_hz: u64,
    /// What it was measured against, and how well.
    pub report: CalibrationReport,
    /// For a recalibration.
    pub inputs: CalibrationInputs,
}

/// What the calibration may use, and its test switch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CalibrationInputs {
    /// The HPET's physical base (ACPI HPET table).
    pub hpet: Option<u64>,
    /// The ACPI PM timer's port and whether it is 32 bits wide.
    pub pm_timer: Option<(u16, bool)>,
    /// `calstall`: stall the boot CPU inside the calibration as a
    /// descheduled vCPU would be (scenario clockcal_test).
    pub stall: bool,
    /// After a kernel handover: the first boot's (TSC, timer) rates, used
    /// as they are (one clock across kernels; measuring again would only
    /// slow the handover).
    pub carried: Option<(u64, u64)>,
}

/// Enables the boot CPU's local APIC.
pub fn init_bsp(inputs: CalibrationInputs) -> Result<Calibration, ApicError> {
    let features = cpu::cpuid(1, 0);
    const CPUID_APIC: u32 = 1 << 9;
    const CPUID_X2APIC: u32 = 1 << 21;
    if features.edx & CPUID_APIC == 0 {
        return Err(ApicError::NotPresent);
    }
    let x2apic = features.ecx & CPUID_X2APIC != 0;
    // SAFETY: IA32_APIC_BASE exists when CPUID reports an APIC. Setting the
    // enable (and x2APIC) bits is the architected way to enable it; the
    // base address is left unchanged.
    let base = unsafe { cpu::rdmsr(IA32_APIC_BASE) };
    if x2apic {
        // SAFETY: see above.
        unsafe { cpu::wrmsr(IA32_APIC_BASE, base | APIC_BASE_ENABLE | APIC_BASE_X2APIC) };
        X2APIC.store(true, Ordering::Relaxed);
    } else {
        // SAFETY: see above.
        unsafe { cpu::wrmsr(IA32_APIC_BASE, base | APIC_BASE_ENABLE) };
        let phys = base & 0x000f_ffff_ffff_f000;
        let virt = paging::map_mmio(phys, 4096).map_err(ApicError::Map)?;
        MMIO_BASE.store(virt, Ordering::Relaxed);
    }

    interrupts::set_handler(SPURIOUS_VECTOR, |_| {});
    write(REG_SPURIOUS, SPURIOUS_ENABLE | u32::from(SPURIOUS_VECTOR));
    write(REG_LVT_TIMER, LVT_MASKED);
    write(REG_LVT_LINT1, LVT_DELIVERY_NMI);

    let (timer_hz, tsc_hz, report) = match inputs.carried {
        Some((tsc, timer)) if tsc != 0 && timer != 0 => (timer, tsc, carried_report(tsc)),
        _ => calibrate(inputs)?,
    };
    let id = read(REG_ID);
    let apic_id = if x2apic { id } else { id >> 24 };
    Ok(Calibration { x2apic, apic_id, timer_hz, tsc_hz, report, inputs })
}

/// Enables the local APIC of the calling CPU (not the boot CPU) in the
/// boot CPU's mode, with its timer masked. Requires `init_bsp` first.
pub fn init_ap() {
    // SAFETY: as in `init_bsp`; every CPU has its own IA32_APIC_BASE, and
    // in xAPIC mode its registers sit at the same physical (and so the
    // same mapped) address as the boot CPU's.
    unsafe {
        let base = cpu::rdmsr(IA32_APIC_BASE);
        let mode = if X2APIC.load(Ordering::Relaxed) { APIC_BASE_X2APIC } else { 0 };
        cpu::wrmsr(IA32_APIC_BASE, base | APIC_BASE_ENABLE | mode);
    }
    write(REG_SPURIOUS, SPURIOUS_ENABLE | u32::from(SPURIOUS_VECTOR));
    write(REG_LVT_TIMER, LVT_MASKED);
    write(REG_LVT_LINT1, LVT_DELIVERY_NMI);
}

/// How the rates were found, for the log and the event stream.
#[derive(Clone, Copy, Debug)]
pub struct CalibrationReport {
    pub reference: refclock::Kind,
    pub reference_hz: u64,
    /// The filtered samples' result (None: too few good samples; the
    /// least disturbed one was used).
    pub estimate: Option<clockcal::Estimate>,
    /// Samples taken.
    pub samples: usize,
    /// The TSC rate the measurement gave.
    pub measured_tsc_hz: u64,
    /// The rate CPUID states, with the leaf.
    pub cpuid: Option<(u64, &'static str)>,
    /// Whether the CPUID rate was used (it agreed with the measurement).
    pub cpuid_used: bool,
    /// `calstall`: stalls injected.
    pub stalls: u32,
}

/// How long one sample's window is, in reference time.
const SAMPLE_WINDOW_MS: u64 = 10;
/// A `calstall` stall: as long as the vCPU pauses seen under load.
const STALL_MS: u64 = 8;

/// One bracket: TSC, then the reference and the timer, then TSC again.
/// `stall` reference ticks are spent between the reads (`calstall`).
struct Bracket {
    tsc: u64,
    reference: u64,
    timer: u32,
    width: u64,
}

fn bracket(r: &mut RefClock, stall: u64) -> Bracket {
    let t0 = cpu::rdtsc();
    let reference = r.read();
    let timer = read(REG_TIMER_CURRENT);
    if stall > 0 {
        spin_reference(r, stall);
    }
    let t1 = cpu::rdtsc();
    Bracket { tsc: t0 + (t1 - t0) / 2, reference, timer, width: t1 - t0 }
}

/// Spins until the reference has moved `ticks`.
fn spin_reference(r: &mut RefClock, ticks: u64) {
    let start = r.read();
    while r.read().wrapping_sub(start) < ticks {
        core::hint::spin_loop();
    }
}

/// Measures the LAPIC timer (one-shot, masked, from its maximum count)
/// and the TSC against `r` (clockcal.rs: brackets, median, outliers).
/// `stall`: pause inside the first sample's opening bracket, between
/// the second's brackets and inside the third's closing bracket.
fn measure(r: &mut RefClock, stall: bool) -> (Option<clockcal::Estimate>, Sample, usize, u32) {
    let window = (r.hz() * SAMPLE_WINDOW_MS / 1000).max(1);
    let stall_ticks = r.hz() * STALL_MS / 1000;
    let mut samples = [Sample::default(); clockcal::MAX_SAMPLES];
    let mut n = 0;
    let mut tight = 0;
    let mut stalls = 0;
    write(REG_LVT_TIMER, LVT_MASKED);
    write(REG_TIMER_DIVIDE, DIVIDE_BY_16);
    while n < clockcal::MAX_SAMPLES && tight < clockcal::WANT_SAMPLES {
        let at = |i: usize| if stall && n == i { stall_ticks } else { 0 };
        stalls += [0, 1, 2].iter().filter(|&&i| at(i) > 0).count() as u32;
        write(REG_TIMER_INITIAL, u32::MAX);
        let start = bracket(r, at(0));
        if at(1) > 0 {
            spin_reference(r, at(1));
        }
        while r.read().wrapping_sub(start.reference) < window {
            core::hint::spin_loop();
        }
        let end = bracket(r, at(2));
        let s = Sample {
            tsc: end.tsc.wrapping_sub(start.tsc),
            timer: u64::from(start.timer.wrapping_sub(end.timer)),
            reference: end.reference.wrapping_sub(start.reference),
            bracket: start.width.max(end.width),
        };
        samples[n] = s;
        n += 1;
        if clockcal::tight(&s) {
            tight += 1;
        }
    }
    write(REG_TIMER_INITIAL, 0);
    let taken = &samples[..n];
    // Without enough good samples: the least disturbed one.
    let best = taken
        .iter()
        .copied()
        .filter(|s| s.tsc > 0 && s.reference > 0)
        .min_by_key(|s| u128::from(s.bracket) * 1_000_000 / u128::from(s.tsc))
        .unwrap_or_default();
    (clockcal::estimate(taken, r.hz()), best, n, stalls)
}

/// Measures the LAPIC timer's and the TSC's rates against the best
/// wall-clock reference (ARCHITECTURE "Interrupts and time").
fn calibrate(inputs: CalibrationInputs) -> Result<(u64, u64, CalibrationReport), ApicError> {
    let mut r = RefClock::best(inputs.hpet, inputs.pm_timer);
    let (mut estimate, mut best, mut samples, stalls) = measure(&mut r, inputs.stall);
    if estimate.is_none() {
        // Once more, without the test's stalls.
        let again = measure(&mut r, false);
        (estimate, best, samples) = (again.0, again.1, samples + again.2);
    }
    let (measured_tsc, timer_hz) = match estimate {
        Some(e) => (e.tsc_hz, e.timer_hz),
        None if best.reference > 0 => (
            (u128::from(best.tsc) * u128::from(r.hz()) / u128::from(best.reference)) as u64,
            (u128::from(best.timer) * u128::from(r.hz()) / u128::from(best.reference)) as u64,
        ),
        None => return Err(ApicError::CalibrationFailed),
    };
    if timer_hz == 0 || measured_tsc == 0 {
        return Err(ApicError::CalibrationFailed);
    }
    let cpuid = refclock::cpuid_tsc_hz();
    let (tsc_hz, cpuid_used) = clockcal::choose(measured_tsc, cpuid.map(|c| c.0));
    let report = CalibrationReport {
        reference: r.kind(),
        reference_hz: r.hz(),
        estimate,
        samples,
        measured_tsc_hz: measured_tsc,
        cpuid,
        cpuid_used,
        stalls,
    };
    Ok((timer_hz, tsc_hz, report))
}

fn carried_report(tsc_hz: u64) -> CalibrationReport {
    CalibrationReport {
        reference: refclock::Kind::Carried,
        reference_hz: 0,
        estimate: None,
        samples: 0,
        measured_tsc_hz: tsc_hz,
        cpuid: None,
        cpuid_used: false,
        stalls: 0,
    }
}

/// Calibrates again on this CPU (the RTC disagreed: time::recalibrate):
/// its timer stops for the measurement; the caller restarts it. Runs with
/// interrupts off.
pub fn recalibrate(old: &Calibration) -> Result<Calibration, ApicError> {
    stop_timer();
    let inputs = CalibrationInputs { stall: false, carried: None, ..old.inputs };
    let (timer_hz, tsc_hz, report) = calibrate(inputs)?;
    Ok(Calibration { timer_hz, tsc_hz, report, ..*old })
}

/// Starts the periodic timer at `hz` on vector [`TIMER_VECTOR`].
pub fn start_periodic_timer(calibration: &Calibration, hz: u64, handler: interrupts::Handler) {
    interrupts::set_handler(TIMER_VECTOR, handler);
    let count = (calibration.timer_hz / hz).clamp(1, u64::from(u32::MAX)) as u32;
    write(REG_TIMER_DIVIDE, DIVIDE_BY_16);
    write(REG_LVT_TIMER, LVT_TIMER_PERIODIC | u32::from(TIMER_VECTOR));
    write(REG_TIMER_INITIAL, count);
}

/// Starts this CPU's periodic timer at `hz` on [`TIMER_VECTOR`] (whose
/// handler `start_periodic_timer` installed), for CPUs other than the boot
/// CPU; local APIC timers tick at the same rate on every CPU.
pub fn start_periodic_timer_here(calibration: &Calibration, hz: u64) {
    let count = (calibration.timer_hz / hz).clamp(1, u64::from(u32::MAX)) as u32;
    write(REG_TIMER_DIVIDE, DIVIDE_BY_16);
    write(REG_LVT_TIMER, LVT_TIMER_PERIODIC | u32::from(TIMER_VECTOR));
    write(REG_TIMER_INITIAL, count);
}

/// Stops this CPU's timer (masked, count 0): before a kernel handover.
pub fn stop_timer() {
    write(REG_LVT_TIMER, LVT_MASKED);
    write(REG_TIMER_INITIAL, 0);
}

/// Sends a fixed interrupt with `vector` to the CPU whose local APIC id is
/// `apic_id` (physical destination mode).
pub fn send_ipi(apic_id: u32, vector: u8) {
    send_icr(apic_id, u32::from(vector));
}

/// ICR delivery modes (bits 8-10) with "level: assert" (bit 14).
const ICR_INIT: u32 = 0b101 << 8 | 1 << 14;
const ICR_STARTUP: u32 = 0b110 << 8 | 1 << 14;

/// Sends INIT to `apic_id`: the CPU resets and waits for a startup IPI
/// (from wherever it was, a bootloader's parking loop included).
pub fn send_init(apic_id: u32) {
    send_icr(apic_id, ICR_INIT);
}

/// Sends a startup IPI: a CPU waiting after INIT starts in real mode at
/// physical `page << 12`.
pub fn send_startup(apic_id: u32, page: u8) {
    send_icr(apic_id, ICR_STARTUP | u32::from(page));
}

/// Sends an NMI to the CPU whose local APIC id is `apic_id` (the hang
/// watchdog asks a stuck CPU where it is: kernel/src/statedump.rs).
pub fn send_nmi(apic_id: u32) {
    send_icr(apic_id, LVT_DELIVERY_NMI);
}

/// Writes the interrupt command register: `low` is its low half (vector
/// and delivery mode), the destination a physical APIC id.
fn send_icr(apic_id: u32, low: u32) {
    // An interrupt handler sending an IPI between the two xAPIC writes
    // would garble the ICR.
    let restore = cpu::disable_interrupts();
    if X2APIC.load(Ordering::Relaxed) {
        // SAFETY: the x2APIC ICR is a single 64-bit MSR (destination in the
        // high half); writing it only sends the interrupt.
        unsafe {
            cpu::wrmsr(
                X2APIC_MSR_BASE + REG_ICR_LOW / 16,
                u64::from(apic_id) << 32 | u64::from(low),
            )
        };
    } else {
        while read(REG_ICR_LOW) & ICR_DELIVERY_PENDING != 0 {
            core::hint::spin_loop();
        }
        // The write to the low half sends it.
        write(REG_ICR_HIGH, apic_id << 24);
        write(REG_ICR_LOW, low);
    }
    if restore {
        cpu::enable_interrupts();
    }
}

/// This CPU's local APIC id.
pub fn id() -> u32 {
    let id = read(REG_ID);
    if X2APIC.load(Ordering::Relaxed) { id } else { id >> 24 }
}

/// Signals end of interrupt to the local APIC.
pub fn eoi() {
    write(REG_EOI, 0);
}
