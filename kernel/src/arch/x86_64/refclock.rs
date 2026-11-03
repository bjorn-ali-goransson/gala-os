//! Wall-clock references for calibrating the TSC and the LAPIC timer
//! (ARCHITECTURE "Interrupts and time", calibration), best first: the
//! HPET's main counter (MMIO, usually 100 MHz or 14.318 MHz), the ACPI
//! PM timer (an I/O port at 3.579545 MHz, 24 or 32 bits), the PIT's
//! channel 2 as a free-running 16-bit counter (read by latching it). Each
//! is read as a monotonic count that extends the hardware's wrap-around
//! (`read`), so a sample's window can be longer than the counter's
//! period as long as reads come more often than one wrap (the PIT's 55 ms
//! is the tightest; a pause longer than that gives an outlier, which the
//! median filter drops).
//!
//! Also the CMOS RTC's seconds register (`rtc_seconds`), the slow check
//! after boot, and the CPUID leaves that may state the TSC's rate.
//!
//! Under QEMU every one of them follows the host's clock, as the TSC
//! does, so a vCPU paused between two reads does not change the ratio.

use super::{cpu, paging, pit, port::Port};

/// Which reference a clock is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Hpet,
    PmTimer,
    Pit,
    /// Not measured: the rates a kernel handover carried.
    Carried,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Hpet => "HPET",
            Kind::PmTimer => "ACPI PM timer",
            Kind::Pit => "PIT",
            Kind::Carried => "the previous kernel",
        }
    }
}

const HPET_CAPABILITIES: u64 = 0x00;
const HPET_CONFIG: u64 = 0x10;
const HPET_COUNTER: u64 = 0xf0;
const HPET_ENABLE: u32 = 1;
/// The HPET specification's longest tick: 100 ns in femtoseconds.
const HPET_MAX_PERIOD_FS: u64 = 100_000_000;
const PM_TIMER_HZ: u64 = 3_579_545;
/// PIT channel 2's latch command (channel 2, counter latch).
const PIT_LATCH_2: u8 = 0b1000_0000;

/// A reference clock being read.
pub struct RefClock {
    kind: Kind,
    hz: u64,
    /// HPET: the mapped registers; PM timer: the port.
    addr: u64,
    mask: u64,
    last: u64,
    total: u64,
}

impl RefClock {
    /// The best reference the machine has.
    pub fn best(hpet: Option<u64>, pm_timer: Option<(u16, bool)>) -> RefClock {
        hpet.and_then(Self::hpet)
            .or_else(|| pm_timer.and_then(|(port, bits32)| Self::pm_timer(port, bits32)))
            .unwrap_or_else(Self::pit)
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Ticks per second.
    pub fn hz(&self) -> u64 {
        self.hz
    }

    fn new(kind: Kind, hz: u64, addr: u64, bits: u32) -> RefClock {
        let mut c = RefClock { kind, hz, addr, mask: (1u64 << bits) - 1, last: 0, total: 0 };
        c.last = c.raw();
        c
    }

    /// The HPET at physical `phys`, enabled; None if its period is out of
    /// specification or its counter does not move.
    fn hpet(phys: u64) -> Option<RefClock> {
        let virt = paging::map_mmio(phys, 0x400).ok()?;
        let period_fs = u64::from(mmio_read(virt + HPET_CAPABILITIES + 4));
        if period_fs == 0 || period_fs > HPET_MAX_PERIOD_FS {
            return None;
        }
        let config = mmio_read(virt + HPET_CONFIG);
        if config & HPET_ENABLE == 0 {
            // Only the main counter: no timer's interrupt is enabled, and
            // legacy replacement (bit 1) stays off.
            mmio_write(virt + HPET_CONFIG, config | HPET_ENABLE);
        }
        // The low 32 bits (a 64-bit read may not be supported).
        Self::new(Kind::Hpet, 1_000_000_000_000_000 / period_fs, virt, 32).moving()
    }

    fn pm_timer(port: u16, bits32: bool) -> Option<RefClock> {
        Self::new(Kind::PmTimer, PM_TIMER_HZ, u64::from(port), if bits32 { 32 } else { 24 })
            .moving()
    }

    /// PIT channel 2 counting down from 65536 for ever (mode 2).
    fn pit() -> RefClock {
        pit::start_free_running();
        Self::new(Kind::Pit, pit::FREQUENCY_HZ, 0, 16)
    }

    /// Self, if the counter advances within a short spin (a disabled or
    /// absent device reads constant).
    fn moving(mut self) -> Option<RefClock> {
        let start = self.read();
        for _ in 0..1_000_000 {
            if self.read() != start {
                return Some(self);
            }
            core::hint::spin_loop();
        }
        None
    }

    fn raw(&self) -> u64 {
        match self.kind {
            Kind::Hpet => u64::from(mmio_read(self.addr + HPET_COUNTER)),
            Kind::PmTimer => {
                // SAFETY: the FADT's PM timer port; reading it has no side
                // effect.
                let mut port = unsafe { Port::<u32>::new(self.addr as u16) };
                u64::from(port.read())
            }
            // Counting down: negated to count up.
            Kind::Pit | Kind::Carried => u64::from(pit::read_count_2(PIT_LATCH_2)).wrapping_neg(),
        }
    }

    /// Ticks since the clock was set up (wrap-arounds counted).
    pub fn read(&mut self) -> u64 {
        let raw = self.raw();
        self.total += raw.wrapping_sub(self.last) & self.mask;
        self.last = raw;
        self.total
    }
}

fn mmio_read(addr: u64) -> u32 {
    // SAFETY: `addr` is a register of the HPET mapped (uncached) by
    // `map_mmio`; reading it has no side effect.
    unsafe { core::ptr::with_exposed_provenance::<u32>(addr as usize).read_volatile() }
}

fn mmio_write(addr: u64, value: u32) {
    // SAFETY: as above; the only write sets the HPET's enable bit.
    unsafe { core::ptr::with_exposed_provenance_mut::<u32>(addr as usize).write_volatile(value) }
}

const CMOS_INDEX: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;
const CMOS_SECONDS: u8 = 0x00;
const CMOS_STATUS_A: u8 = 0x0a;
const CMOS_UPDATING: u8 = 0x80;
/// Bit 7 of the index port disables NMIs while set: kept clear.
const CMOS_NMI_ENABLED: u8 = 0;

fn cmos_read(reg: u8) -> u8 {
    // SAFETY: the CMOS index/data ports; selecting a register and reading
    // it has no side effect beyond the selection (NMIs stay enabled). The
    // caller runs with interrupts off, so the pair is not split.
    let (mut index, mut data) =
        unsafe { (Port::<u8>::new(CMOS_INDEX), Port::<u8>::new(CMOS_DATA)) };
    index.write(reg | CMOS_NMI_ENABLED);
    data.read()
}

/// The RTC's seconds (0-59, decoded from BCD unless status B says
/// binary), or None while the RTC is updating them.
pub fn rtc_seconds() -> Option<u8> {
    const CMOS_STATUS_B: u8 = 0x0b;
    const BINARY: u8 = 1 << 2;
    let restore = cpu::disable_interrupts();
    let r = if cmos_read(CMOS_STATUS_A) & CMOS_UPDATING != 0 {
        None
    } else {
        let raw = cmos_read(CMOS_SECONDS);
        let binary = cmos_read(CMOS_STATUS_B) & BINARY != 0;
        Some(if binary { raw } else { (raw >> 4) * 10 + (raw & 0xf) })
    };
    if restore {
        cpu::enable_interrupts();
    }
    r
}

/// The TSC's rate as CPUID states it, with the leaf's name: leaf 0x15
/// (crystal clock times the TSC/crystal ratio, when the crystal's rate is
/// given) or the hypervisor timing leaf 0x40000010 (TSC kHz; QEMU/KVM,
/// VMware). Not to be believed alone: an emulator may state a rate its
/// TSC does not run at (`clockcal::choose`).
pub fn cpuid_tsc_hz() -> Option<(u64, &'static str)> {
    let max = cpu::cpuid(0, 0).eax;
    if max >= 0x15 {
        let l = cpu::cpuid(0x15, 0);
        if l.eax != 0 && l.ebx != 0 && l.ecx != 0 {
            return Some((u64::from(l.ecx) * u64::from(l.ebx) / u64::from(l.eax), "CPUID 0x15"));
        }
    }
    const HYPERVISOR: u32 = 1 << 31;
    if cpu::cpuid(1, 0).ecx & HYPERVISOR != 0 && cpu::cpuid(0x4000_0000, 0).eax >= 0x4000_0010 {
        let khz = cpu::cpuid(0x4000_0010, 0).eax;
        if khz != 0 {
            return Some((u64::from(khz) * 1000, "CPUID 0x40000010"));
        }
    }
    None
}
