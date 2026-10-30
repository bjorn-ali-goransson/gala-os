//! Local APIC: x2APIC (MSR access) when available, otherwise xAPIC (MMIO).
//! Provides EOI and the periodic timer, calibrated against the PIT.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::{cpu, interrupts, paging, pit};

const IA32_APIC_BASE: u32 = 0x1b;
const APIC_BASE_ENABLE: u64 = 1 << 11;
const APIC_BASE_X2APIC: u64 = 1 << 10;
const X2APIC_MSR_BASE: u32 = 0x800;

// Register offsets (xAPIC MMIO byte offset; x2APIC MSR = 0x800 + offset / 16).
const REG_ID: u32 = 0x20;
const REG_EOI: u32 = 0xb0;
const REG_SPURIOUS: u32 = 0xf0;
const REG_LVT_TIMER: u32 = 0x320;
const REG_TIMER_INITIAL: u32 = 0x380;
const REG_TIMER_CURRENT: u32 = 0x390;
const REG_TIMER_DIVIDE: u32 = 0x3e0;

const SPURIOUS_ENABLE: u32 = 1 << 8;
const LVT_MASKED: u32 = 1 << 16;
const LVT_TIMER_PERIODIC: u32 = 1 << 17;
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
}

/// Enables the boot CPU's local APIC.
pub fn init_bsp() -> Result<Calibration, ApicError> {
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

    let (timer_hz, tsc_hz) = calibrate()?;
    let id = read(REG_ID);
    let apic_id = if x2apic { id } else { id >> 24 };
    Ok(Calibration { x2apic, apic_id, timer_hz, tsc_hz })
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
}

/// Measures the LAPIC timer and TSC over a PIT-timed interval.
fn calibrate() -> Result<(u64, u64), ApicError> {
    const SAMPLE_MS: u64 = 10;
    write(REG_TIMER_DIVIDE, DIVIDE_BY_16);
    let pit_ticks = (pit::FREQUENCY_HZ * SAMPLE_MS / 1000) as u16;
    pit::start_oneshot(pit_ticks);
    write(REG_TIMER_INITIAL, u32::MAX);
    let tsc_start = cpu::rdtsc();
    if !pit::wait_oneshot() {
        return Err(ApicError::CalibrationFailed);
    }
    let elapsed = u64::from(u32::MAX - read(REG_TIMER_CURRENT));
    let tsc = cpu::rdtsc() - tsc_start;
    write(REG_TIMER_INITIAL, 0);
    // Scale by the exact PIT interval rather than the nominal SAMPLE_MS.
    let scale = |count: u64| count * pit::FREQUENCY_HZ / u64::from(pit_ticks);
    let (timer_hz, tsc_hz) = (scale(elapsed), scale(tsc));
    if timer_hz == 0 {
        return Err(ApicError::CalibrationFailed);
    }
    Ok((timer_hz, tsc_hz))
}

/// Starts the periodic timer at `hz` on vector [`TIMER_VECTOR`].
pub fn start_periodic_timer(calibration: &Calibration, hz: u64, handler: interrupts::Handler) {
    interrupts::set_handler(TIMER_VECTOR, handler);
    let count = (calibration.timer_hz / hz).clamp(1, u64::from(u32::MAX)) as u32;
    write(REG_TIMER_DIVIDE, DIVIDE_BY_16);
    write(REG_LVT_TIMER, LVT_TIMER_PERIODIC | u32::from(TIMER_VECTOR));
    write(REG_TIMER_INITIAL, count);
}

/// Signals end of interrupt to the local APIC.
pub fn eoi() {
    write(REG_EOI, 0);
}
