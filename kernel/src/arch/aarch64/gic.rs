//! The interrupt controller: a GICv3 (Arm's GIC Architecture
//! Specification, v3 and v4), where the firmware's MADT says: its GICD
//! entry (the distributor) and GICR entry (the redistributors' range).
//! Without them, QEMU `virt`'s layout (with `gic-version=3`: the
//! distributor at 0x0800_0000, the redistributors from 0x080a_0000), which
//! the log says. One security state (QEMU's `virt` without `secure=on`):
//! GICD_CTLR's DS is set, group 1 is the kernel's.
//!
//! Brought up with its first interrupt source (`timer::start_periodic`):
//! the distributor once (affinity routing, group 1 on), then on each CPU
//! (`init_cpu`: the boot CPU's then, an AP's at its start) its
//! redistributor (awake; its private interrupts, SGIs and PPIs, enabled
//! one by one) and its CPU interface through the system registers
//! (`ICC_*`: enabled; its priority mask is `cpu`'s, interrupts' on and
//! off). `dispatch` acknowledges an IRQ (ICC_IAR1_EL1), ends it
//! (ICC_EOIR1_EL1), then runs its handler (which may switch threads).
//! SGIs are the CPUs' IPIs (`send_sgi`, ICC_SGI1R_EL1).
//!
//! The pseudo-NMI: SGI `NMI_SGI` at `NMI_PRIORITY`, above the others'
//! `IRQ_PRIORITY`, which `cpu::PMR_OFF` lets through while interrupts are
//! off. Taken, it runs `interrupts::nmi` with PSTATE.I set (NMIs do not
//! nest). An ordinary IRQ's handler runs with interrupts off by priority
//! and PSTATE.I clear, so an NMI reaches it too (the watchdog's backtrace
//! of a CPU stuck in its tick handler); its exit sets PSTATE.I and the
//! mask back to "on" (it was taken only with interrupts on).

use core::sync::atomic::{AtomicU64, Ordering};

use super::interrupts::{Handler, TrapFrame};
use crate::sync::IrqMutex;

/// QEMU `virt`'s layout, without the MADT's entries.
const VIRT_GICD: u64 = 0x0800_0000;
const VIRT_GICR: u64 = 0x080a_0000;
const VIRT_GICR_SPAN: u64 = 0x00f6_0000;
/// One redistributor: its RD_base frame, then its SGI_base frame.
const GICR_STRIDE: u64 = 0x2_0000;

const GICD_CTLR: u64 = 0x0000;
const GICD_CTLR_RWP: u32 = 1 << 31;
/// EnableGrp0, EnableGrp1, ARE (single security state: DS).
const GICD_CTLR_ENABLE: u32 = (1 << 0) | (1 << 1) | (1 << 4);

const GICR_TYPER: u64 = 0x0008;
const GICR_TYPER_LAST: u64 = 1 << 4;
const GICR_WAKER: u64 = 0x0014;
const WAKER_PROCESSOR_SLEEP: u32 = 1 << 1;
const WAKER_CHILDREN_ASLEEP: u32 = 1 << 2;
/// In the SGI_base frame (64 KiB after RD_base).
const SGI_BASE: u64 = 0x1_0000;
const GICR_IGROUPR0: u64 = SGI_BASE + 0x0080;
const GICR_ISENABLER0: u64 = SGI_BASE + 0x0100;
const GICR_IPRIORITYR: u64 = SGI_BASE + 0x0400;

/// The pseudo-NMI's SGI and the two priorities (lower is more urgent;
/// QEMU's GIC keeps 5 bits of priority: multiples of 8).
pub const NMI_SGI: u32 = 1;
pub const NMI_PRIORITY: u8 = 0x80;
pub const IRQ_PRIORITY: u8 = 0xa0;

/// INTIDs 1020-1023 are special: nothing to handle (spurious).
const SPURIOUS: u64 = 1020;
/// The private interrupts (SGIs 0-15, PPIs 16-31) handled here.
const PRIVATE: usize = 32;

/// The redistributors' region, mapped (0: the distributor is not up),
/// its physical base and its length (searched for each CPU's).
static GICR: AtomicU64 = AtomicU64::new(0);
static GICR_PHYS: AtomicU64 = AtomicU64::new(0);
static GICR_SPAN: AtomicU64 = AtomicU64::new(0);
static GICD_PHYS: AtomicU64 = AtomicU64::new(0);
/// Each CPU's redistributor's RD_base, virtual (0: not up), by CPU index.
static RD_BASE: [AtomicU64; super::smp::MAX_CPUS] =
    [const { AtomicU64::new(0) }; super::smp::MAX_CPUS];

fn my_rd() -> u64 {
    let cpu = super::percpu::cpu_index() as usize;
    RD_BASE.get(cpu).map_or(0, |r| r.load(Ordering::Acquire))
}
static HANDLERS: IrqMutex<[Option<Handler>; PRIVATE]> = IrqMutex::new([None; PRIVATE]);

/// GIC bring-up failed: the boot cannot have interrupts.
#[derive(Debug)]
pub enum GicError {
    Map(super::paging::PagingError),
    /// No redistributor has this CPU's affinity.
    NoRedistributor(u64),
}

fn read32(addr: u64) -> u32 {
    // SAFETY: a GIC register in a mapping `init` made (Device memory).
    unsafe { (addr as *const u32).read_volatile() }
}

fn write32(addr: u64, value: u32) {
    // SAFETY: as read32.
    unsafe { (addr as *mut u32).write_volatile(value) }
}

fn read64(addr: u64) -> u64 {
    // SAFETY: as read32.
    unsafe { (addr as *const u64).read_volatile() }
}

/// This CPU's MPIDR affinity (Aff3 at 32, Aff2-Aff0 at 16, 8, 0): its
/// hardware id, as Limine's MP response and the MADT give it.
fn affinity() -> u64 {
    let mpidr: u64;
    // SAFETY: reads MPIDR_EL1.
    unsafe {
        core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack, preserves_flags))
    };
    mpidr & 0xff_00ff_ffff
}

/// An MPIDR affinity in GICR_TYPER's form: Aff3.Aff2.Aff1.Aff0 in 32 bits.
fn packed(affinity: u64) -> u64 {
    (affinity & 0xff_ffff) | ((affinity >> 8) & 0xff00_0000)
}

/// Brings the GIC up: the distributor (once), then this CPU's part.
pub fn init() -> Result<(), GicError> {
    if GICR.load(Ordering::Acquire) == 0 {
        let madt = crate::boot::early_addresses()
            .and_then(|(hhdm, _)| crate::boot::acpi::read_gic(hhdm, crate::boot::early_rsdp()))
            .unwrap_or_default();
        let (gicd_phys, (gicr_phys, span)) = match (madt.distributor, madt.redistributors) {
            (Some(d), Some((r, len))) => {
                crate::info!(
                    "gic: GICv3 distributor at {d:#x}, redistributors from {r:#x} ({len:#x} bytes) \
                     (the firmware's MADT)"
                );
                (d, (r, u64::from(len)))
            }
            _ => {
                crate::warn!(
                    "gic: GICv3 distributor at {VIRT_GICD:#x}, redistributors from {VIRT_GICR:#x} \
                     (QEMU virt's: the firmware's MADT names no GICD and GICR)"
                );
                (VIRT_GICD, (VIRT_GICR, VIRT_GICR_SPAN))
            }
        };
        GICD_PHYS.store(gicd_phys, Ordering::Relaxed);
        GICR_PHYS.store(gicr_phys, Ordering::Relaxed);
        GICR_SPAN.store(span, Ordering::Relaxed);
        let gicd = super::paging::map_mmio(gicd_phys, 0x1_0000).map_err(GicError::Map)?;
        write32(gicd + GICD_CTLR, GICD_CTLR_ENABLE);
        for _ in 0..1_000_000 {
            if read32(gicd + GICD_CTLR) & GICD_CTLR_RWP == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        let gicr = super::paging::map_mmio(gicr_phys, span).map_err(GicError::Map)?;
        GICR.store(gicr, Ordering::Release);
    }
    init_cpu()
}

/// This CPU's redistributor (found by its affinity: woken, its private
/// interrupts in group 1) and CPU interface. Once per CPU, after `init`.
pub fn init_cpu() -> Result<(), GicError> {
    let cpu = super::percpu::cpu_index() as usize;
    if my_rd() != 0 {
        return Ok(());
    }
    let gicr = GICR.load(Ordering::Acquire);
    let span = GICR_SPAN.load(Ordering::Relaxed);
    let me = packed(affinity());
    let mut rd = None;
    let mut at = gicr;
    while gicr != 0 && at + GICR_STRIDE <= gicr + span {
        let typer = read64(at + GICR_TYPER);
        if typer >> 32 == me {
            rd = Some(at);
            break;
        }
        if typer & GICR_TYPER_LAST != 0 {
            break;
        }
        at += GICR_STRIDE;
    }
    let rd = rd.ok_or(GicError::NoRedistributor(me))?;
    // Awake: the redistributor forwards interrupts to the CPU interface.
    write32(rd + GICR_WAKER, read32(rd + GICR_WAKER) & !WAKER_PROCESSOR_SLEEP);
    for _ in 0..1_000_000 {
        if read32(rd + GICR_WAKER) & WAKER_CHILDREN_ASLEEP == 0 {
            break;
        }
        core::hint::spin_loop();
    }
    // Every private interrupt in group 1 (none enabled yet).
    write32(rd + GICR_IGROUPR0, u32::MAX);
    // SAFETY: the CPU interface's system registers (ICC_SRE_EL1,
    // ICC_BPR1_EL1, ICC_IGRPEN1_EL1 by their encodings): system register
    // access on (the entry turned it on already), every priority bit a
    // group priority (the NMI preempts an IRQ's handler), group 1 on. The
    // priority mask (ICC_PMR_EL1) is `cpu`'s.
    unsafe {
        core::arch::asm!(
            "mrs {t}, S3_0_C12_C12_5",
            "orr {t}, {t}, #1",
            "msr S3_0_C12_C12_5, {t}",
            "isb",
            "msr S3_0_C12_C12_3, xzr",
            "mov {t}, #1",
            "msr S3_0_C12_C12_7, {t}",
            "isb",
            t = out(reg) _,
            options(nostack, preserves_flags)
        )
    };
    if let Some(slot) = RD_BASE.get(cpu) {
        slot.store(rd, Ordering::Release);
    }
    // The pseudo-NMI on every CPU; private interrupts already handled
    // (the reschedule SGI, the timer) are enabled here too.
    enable_here(rd, NMI_SGI, NMI_PRIORITY);
    let handlers = *HANDLERS.lock();
    for (intid, h) in handlers.iter().enumerate() {
        if h.is_some() {
            enable_here(rd, intid as u32, IRQ_PRIORITY);
        }
    }
    crate::info!(
        "gic: CPU {cpu}: GICv3 redistributor (affinity {me:#x}) at {:#x}, distributor at {:#x}",
        GICR_PHYS.load(Ordering::Relaxed) + (rd - gicr),
        GICD_PHYS.load(Ordering::Relaxed)
    );
    Ok(())
}

fn enable_here(rd: u64, intid: u32, priority: u8) {
    // SAFETY: one byte of GICR_IPRIORITYR per interrupt.
    unsafe { ((rd + GICR_IPRIORITYR + u64::from(intid)) as *mut u8).write_volatile(priority) };
    write32(rd + GICR_ISENABLER0, 1 << intid);
}

/// Sends SGI `intid` (0-15) to the CPU with MPIDR affinity `target`
/// through ICC_SGI1R_EL1: Aff3, Aff2, Aff1 and the target list's bit for
/// Aff0 (below 16, as on QEMU `virt`).
pub fn send_sgi(intid: u32, target: u64) {
    let aff0 = target & 0xff;
    if aff0 >= 16 || intid >= 16 {
        crate::warn!("gic: SGI {intid} to affinity {target:#x} not sent (Aff0 above 15)");
        return;
    }
    let value = ((target >> 32) & 0xff) << 48
        | ((target >> 16) & 0xff) << 32
        | u64::from(intid) << 24
        | ((target >> 8) & 0xff) << 16
        | 1 << aff0;
    // SAFETY: ICC_SGI1R_EL1 generates the SGI; nothing else changes.
    unsafe {
        core::arch::asm!("dsb ishst", "msr S3_0_C12_C11_5, {}", "isb", in(reg) value, options(nostack, preserves_flags))
    };
}

/// This CPU's MPIDR affinity in `affinity`'s form (its hardware id).
pub fn my_affinity() -> u64 {
    affinity()
}

/// Enables private interrupt `intid` (an SGI or PPI) on this CPU, at
/// `IRQ_PRIORITY`, with `handler` (the same on every CPU: the others
/// enable it when they start, `init_cpu`).
pub fn enable_private(intid: u32, handler: Handler) {
    let rd = my_rd();
    let i = intid as usize;
    if rd == 0 || i >= PRIVATE || intid == NMI_SGI {
        crate::warn!("gic: interrupt {intid} not enabled (GIC up here: {})", rd != 0);
        return;
    }
    HANDLERS.lock()[i] = Some(handler);
    enable_here(rd, intid, IRQ_PRIORITY);
}

/// Handles one IRQ (the IRQ vector's): acknowledged, ended, then the NMI
/// or its handler run with the interrupted state. An interrupt without a
/// handler is logged once per INTID. What it took: the caller ends an
/// ordinary IRQ (`cpu::irq_exit`, after the user-return hook for one from
/// EL0); an NMI or a spurious interrupt leaves the mask as it found it.
pub fn dispatch(frame: &mut TrapFrame) -> Taken {
    let iar: u64;
    // SAFETY: ICC_IAR1_EL1: acknowledges the highest pending group 1 IRQ.
    unsafe {
        core::arch::asm!("mrs {}, S3_0_C12_C12_0", out(reg) iar, options(nomem, nostack, preserves_flags))
    };
    let intid = iar & 0xff_ffff;
    if intid >= SPURIOUS {
        return Taken::Nothing;
    }
    // Ended before its handler runs: a handler may switch threads (the
    // tick's preemption), and until the EOI the CPU's running priority
    // keeps every interrupt of this priority back, so the thread switched
    // to would run without ticks. IRQs stay masked until the vector's
    // return, and the handler first clears what raised a level-sensitive
    // interrupt (the timer moves its compare value), so it is not taken
    // again.
    // SAFETY: ICC_EOIR1_EL1: ends the interrupt acknowledged above.
    unsafe {
        core::arch::asm!("msr S3_0_C12_C12_1, {}", in(reg) iar, options(nomem, nostack, preserves_flags))
    };
    if intid == u64::from(NMI_SGI) {
        // PSTATE.I stays set (the vector's): no NMI within an NMI.
        super::interrupts::nmi(frame);
        return Taken::Nmi;
    }
    // An ordinary IRQ is taken only with interrupts on; its handler runs
    // with them off by priority, the NMI let through.
    super::cpu::pmr_off();
    super::cpu::unmask_daif_irq();
    let handler = HANDLERS.lock().get(intid as usize).copied().flatten();
    match handler {
        Some(h) => h(frame),
        None => {
            static UNHANDLED: AtomicU64 = AtomicU64::new(0);
            if intid < 64 && UNHANDLED.fetch_or(1 << intid, Ordering::Relaxed) & (1 << intid) == 0 {
                crate::warn!("gic: interrupt {intid} has no handler");
            }
        }
    }
    Taken::Irq
}

/// What `dispatch` took.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taken {
    /// None (spurious): nothing changed.
    Nothing,
    /// The NMI: run with PSTATE.I set, the mask untouched.
    Nmi,
    /// An ordinary IRQ: its handler ran with the mask "off" and PSTATE.I
    /// clear; the caller exits it.
    Irq,
}
