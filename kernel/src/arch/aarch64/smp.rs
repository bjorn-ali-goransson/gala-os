//! The other CPUs. Limine has started them (its MP response): each waits
//! at EL1, on the bootloader's tables, until its `goto_address` is
//! written. `start_aps` gives each a kernel stack and its CPU index (the
//! `extra_argument`) and sends it to `gala_aarch64_ap_entry`, which turns
//! FP/SIMD on, takes the stack as SP_EL1 and the kernel's tables (MAIR,
//! TTBR1, its TLB invalidated), then `ap_main`: its per-CPU block, vectors
//! and GIC part, online, the scheduler (`sched::run_ap`, which starts its
//! tick).
//!
//! CPUs the MADT lists that the bootloader did not start are started with
//! PSCI CPU_ON (`psci`): such a CPU enters `gala_aarch64_psci_entry` at
//! its physical address with the MMU off, which loads the kernel's MAIR,
//! TCR, SCTLR and TTBR1 and a TTBR0 mapping its own page at the same
//! address (`paging::identity_root`), turns the MMU on and jumps to the
//! AP entry's virtual address with its CPU index; what it reads is its
//! `PsciStart` (in .bss, cleaned to memory: the MMU off, its loads are
//! not cached), where it also leaves how far it got. `smptest=hide`
//! tests this: the APs Limine started power themselves off
//! (`gala_aarch64_cpu_off`, PSCI CPU_OFF) and are started again so.
//!
//! IPIs are SGIs: SGI 0 reschedules (its IRQ's return is the point).
//! A TLB flush everywhere needs none: `tlbi vmalle1is` reaches every CPU
//! of the inner shareable domain.

/// What a processor's hardware id is (`boot::MpInfo::hw_id`): MPIDR_EL1's affinity fields.
pub const HW_ID_NAME: &str = "MPIDR";

use core::sync::atomic::{AtomicU64, Ordering};

use super::context::{KernelStack, StackError};
use super::unported;
use crate::boot::{BootInfo, KernelAddress};
use crate::mm::heap::AllocError;
use crate::sync::IrqMutex;

/// The reschedule IPI.
const RESCHEDULE_SGI: u32 = 0;
/// How long an AP has to come online.
const AP_START_NS: u64 = 1_000_000_000;

/// Online CPUs, one bit per index.
static ONLINE: AtomicU64 = AtomicU64::new(0);
/// Each CPU's hardware id (MPIDR affinity), by index.
static HW_IDS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// Each AP's kernel stack's top, read by its entry, by index.
static AP_STACK_TOPS: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
/// The kernel's TTBR1 root, for the APs' entry.
static AP_ROOT: AtomicU64 = AtomicU64::new(0);
/// The APs' kernel stacks, kept for as long as they run.
static AP_STACKS: IrqMutex<[Option<KernelStack>; MAX_CPUS]> =
    IrqMutex::new([const { None }; MAX_CPUS]);
/// What an AP runs once set up (`start_aps`' `main`).
static AP_MAIN: IrqMutex<Option<fn(u32) -> !>> = IrqMutex::new(None);

// `gala_aarch64_ap_entry`: x0 the AP's `limine_mp_info`, its CPU index
// in `extra_argument`; `gala_aarch64_ap_entry_index`: x0 the index (from
// the PSCI trampoline).
core::arch::global_asm!(
    ".section .text.ap_entry, \"ax\"",
    ".global gala_aarch64_ap_entry",
    "gala_aarch64_ap_entry:",
    "ldr x0, [x0, #{extra}]",
    ".global gala_aarch64_ap_entry_index",
    "gala_aarch64_ap_entry_index:",
    "mov x9, #(3 << 20)",
    "msr cpacr_el1, x9",
    "isb",
    // As the boot CPU's entry: the GIC's system registers, interrupts
    // masked by priority.
    "mrs x9, S3_0_C12_C12_5",
    "orr x9, x9, #1",
    "msr S3_0_C12_C12_5, x9",
    "isb",
    "mov x9, #{pmr_off}",
    "msr S3_0_C4_C6_0, x9",
    "mov x19, x0",
    "adrp x9, {tops}",
    "add x9, x9, :lo12:{tops}",
    "ldr x10, [x9, x19, lsl #3]",
    "msr spsel, #1",
    "mov sp, x10",
    "adrp x9, {root}",
    "add x9, x9, :lo12:{root}",
    "ldr x12, [x9]",
    "ldr x11, ={mair}",
    "msr mair_el1, x11",
    "dsb ishst",
    "msr ttbr1_el1, x12",
    "isb",
    "tlbi vmalle1",
    "dsb nsh",
    "isb",
    "mov x29, xzr",
    "mov x0, x19",
    "bl {main}",
    "udf #0",
    extra = const crate::boot::MP_INFO_EXTRA_ARGUMENT,
    tops = sym AP_STACK_TOPS,
    root = sym AP_ROOT,
    mair = const super::paging::MAIR,
    main = sym ap_main,
    pmr_off = const super::cpu::PMR_OFF,
);

// The PSCI trampoline (one page of its own: TTBR0 maps it at its
// physical address). x0: its `PsciStart`'s physical address. Not at EL1
// (a firmware entering at EL2): it says so in `state` and waits.
core::arch::global_asm!(
    ".section .text.psci_entry, \"ax\"",
    ".balign 4096",
    ".global gala_aarch64_psci_entry",
    "gala_aarch64_psci_entry:",
    "mrs x9, CurrentEL",
    "cmp x9, #(1 << 2)",
    "b.eq 1f",
    "mov x9, #{wrong_el}",
    "str x9, [x0, #{state}]",
    "0:",
    "wfe",
    "b 0b",
    "1:",
    "mov x9, #{reached}",
    "str x9, [x0, #{state}]",
    "ldr x9, [x0, #{mair}]",
    "msr mair_el1, x9",
    "ldr x9, [x0, #{tcr}]",
    "msr tcr_el1, x9",
    "ldr x9, [x0, #{ttbr0}]",
    "msr ttbr0_el1, x9",
    "ldr x9, [x0, #{ttbr1}]",
    "msr ttbr1_el1, x9",
    "isb",
    "tlbi vmalle1",
    "dsb nsh",
    "isb",
    "ldr x9, [x0, #{sctlr}]",
    "ldr x10, [x0, #{entry}]",
    "ldr x11, [x0, #{index}]",
    "msr sctlr_el1, x9",
    "isb",
    "mov x0, x11",
    "br x10",
    wrong_el = const STATE_WRONG_EL,
    reached = const STATE_REACHED,
    state = const core::mem::offset_of!(PsciStart, state),
    mair = const core::mem::offset_of!(PsciStart, mair),
    tcr = const core::mem::offset_of!(PsciStart, tcr),
    ttbr0 = const core::mem::offset_of!(PsciStart, ttbr0),
    ttbr1 = const core::mem::offset_of!(PsciStart, ttbr1),
    sctlr = const core::mem::offset_of!(PsciStart, sctlr),
    entry = const core::mem::offset_of!(PsciStart, entry),
    index = const core::mem::offset_of!(PsciStart, index),
);

// `smptest=hide`: where the bootloader's APs are sent (on its tables, x0
// its `limine_mp_info`): PSCI CPU_OFF through the conduit `psci::init`
// set; it does not return.
core::arch::global_asm!(
    ".section .text.ap_entry, \"ax\"",
    ".global gala_aarch64_cpu_off",
    "gala_aarch64_cpu_off:",
    "adrp x9, GALA_PSCI_CONDUIT",
    "ldrb w9, [x9, :lo12:GALA_PSCI_CONDUIT]",
    "movz x0, #{lo}",
    "movk x0, #{hi}, lsl #16",
    "cmp w9, #{hvc}",
    "b.ne 1f",
    "hvc #0",
    "b 2f",
    "1:",
    "smc #0",
    "2:",
    "wfe",
    "b 2b",
    lo = const super::psci::CPU_OFF & 0xffff,
    hi = const super::psci::CPU_OFF >> 16,
    hvc = const super::psci::CONDUIT_HVC,
);

unsafe extern "C" {
    fn gala_aarch64_ap_entry();
    fn gala_aarch64_ap_entry_index();
    fn gala_aarch64_psci_entry();
    fn gala_aarch64_cpu_off();
}

/// What a CPU started by PSCI reads before its MMU is on (by physical
/// address), and where it says how far it got.
#[repr(C, align(64))]
struct PsciStart {
    mair: AtomicU64,
    tcr: AtomicU64,
    ttbr0: AtomicU64,
    ttbr1: AtomicU64,
    sctlr: AtomicU64,
    /// `gala_aarch64_ap_entry_index`'s virtual address.
    entry: AtomicU64,
    index: AtomicU64,
    state: AtomicU64,
}

/// `PsciStart::state`: not reached, the trampoline reached at EL1, or
/// reached at another exception level (not handled).
const STATE_NOT_REACHED: u64 = 0;
const STATE_REACHED: u64 = 1;
const STATE_WRONG_EL: u64 = 2;

static PSCI_STARTS: [PsciStart; MAX_CPUS] = [const {
    PsciStart {
        mair: AtomicU64::new(0),
        tcr: AtomicU64::new(0),
        ttbr0: AtomicU64::new(0),
        ttbr1: AtomicU64::new(0),
        sctlr: AtomicU64::new(0),
        entry: AtomicU64::new(0),
        index: AtomicU64::new(0),
        state: AtomicU64::new(0),
    }
}; MAX_CPUS];

/// Cleans (`clean`) or cleans and invalidates the data cache lines of `p`
/// to the point of coherency: what a CPU with its MMU off (uncached) reads
/// or wrote.
fn cache_line(p: &PsciStart, clean_only: bool) {
    let at = core::ptr::from_ref(p) as u64;
    // SAFETY: cache maintenance of a line of a static; no data changes.
    unsafe {
        if clean_only {
            core::arch::asm!("dc cvac, {}", "dsb sy", in(reg) at, options(nostack, preserves_flags));
        } else {
            core::arch::asm!("dc civac, {}", "dsb sy", in(reg) at, options(nostack, preserves_flags));
        }
    }
}

fn read_sysreg_tcr_sctlr() -> (u64, u64) {
    let (tcr, sctlr): (u64, u64);
    // SAFETY: reads TCR_EL1 and SCTLR_EL1.
    unsafe {
        core::arch::asm!(
            "mrs {}, tcr_el1",
            "mrs {}, sctlr_el1",
            out(reg) tcr,
            out(reg) sctlr,
            options(nomem, nostack, preserves_flags)
        )
    };
    (tcr, sctlr)
}

/// An AP on its own stack and the kernel's tables: its per-CPU block,
/// vectors and GIC part, then online and the scheduler.
extern "C" fn ap_main(index: u64) -> ! {
    let index = index as u32;
    super::percpu::install(index);
    super::entry::install_vectors();
    super::cpu::allow_user_access();
    if let Err(e) = super::gic::init_cpu() {
        crate::warn!("smp: CPU {index}: no interrupt controller ({e:?}): it stays offline");
        super::cpu::halt_forever();
    }
    ONLINE.fetch_or(1 << index, Ordering::AcqRel);
    let main = *AP_MAIN.lock();
    match main {
        Some(main) => main(index),
        None => super::cpu::halt_forever(),
    }
}

pub const MAX_CPUS: usize = 64;

/// What became of the bootloader's AP tables (x86-64's late-AP shelter;
/// none here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shelter {
    Added,
    NoTrampoline,
    AlreadyMapped,
    Unsupported,
    UnknownTables,
}

pub fn shelter_state() -> Option<Shelter> {
    None
}

pub fn shelter_bootloader_aps(
    _hhdm_offset: u64,
    _kernel: KernelAddress,
    _reclaimable: impl Iterator<Item = (u64, u64)>,
) {
}

/// (TLB shootdowns, IPIs sent for them).
pub fn flush_stats() -> (u64, u64) {
    (0, 0)
}

/// Every CPU's TLB, by broadcast (`tlbi vmalle1is`: no IPI).
pub fn flush_tlb_everywhere() {
    // SAFETY: TLB maintenance only.
    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            options(nostack, preserves_flags)
        )
    };
}

pub fn set_tlb_paranoid(_on: bool) {}

pub fn tlb_paranoid() -> bool {
    false
}

pub fn moved_flushes() -> u64 {
    0
}

/// Nothing left at a space lock's release: every change was dropped from
/// every CPU's TLB under the lock (`paging::invalidate_user_page`, a
/// broadcast completed by its DSB).
pub fn flush_user_space(_root: u64, _changed_on: Option<u32>) {}

pub fn online_count() -> u32 {
    online_mask().count_ones()
}

pub fn online_mask() -> u64 {
    ONLINE.load(Ordering::Acquire).max(1)
}

#[derive(Debug)]
pub enum SmpError {
    OutOfMemory,
    Stack(StackError),
}

impl From<AllocError> for SmpError {
    fn from(_: AllocError) -> Self {
        SmpError::OutOfMemory
    }
}

impl From<StackError> for SmpError {
    fn from(e: StackError) -> Self {
        SmpError::Stack(e)
    }
}

/// (CPU index, hardware id: MPIDR's affinity) of every online CPU.
pub fn online_cpus() -> impl Iterator<Item = (u32, u32)> {
    let mask = online_mask();
    (0..MAX_CPUS as u32)
        .filter(move |i| mask & (1 << i) != 0)
        .map(|i| (i, HW_IDS[i as usize].load(Ordering::Relaxed) as u32))
}

/// Wakes CPU `index` to reschedule (SGI 0).
pub fn send_reschedule(index: u32) {
    if index < MAX_CPUS as u32 && online_mask() & (1 << index) != 0 {
        super::gic::send_sgi(RESCHEDULE_SGI, HW_IDS[index as usize].load(Ordering::Relaxed));
    }
}

/// The pseudo-NMI (`gic::NMI_SGI`) to the CPU with hardware id `hw_id`:
/// it reaches the CPU with its interrupts off too.
pub fn send_nmi(hw_id: u32) {
    super::gic::send_sgi(super::gic::NMI_SGI, u64::from(hw_id));
}

/// The boot CPU online (index 0), and the reschedule SGI's handler: its
/// IRQ's return is the point (the scheduler runs then).
pub fn init_bsp() {
    HW_IDS[0].store(super::gic::my_affinity(), Ordering::Relaxed);
    ONLINE.fetch_or(1, Ordering::AcqRel);
    super::gic::enable_private(RESCHEDULE_SGI, |_| {});
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmpTest {
    None,
    Hide,
    Late,
}

#[derive(Debug, Default)]
pub struct SmpReport {
    pub online: u32,
    pub reported: usize,
    pub firmware: Option<usize>,
    pub started_here: u32,
    pub failed: u32,
    pub first_failed: Option<u32>,
    pub late_replay: Option<&'static str>,
    pub slow: bool,
}

/// Starts the CPUs the bootloader reported (all but this one), each
/// running `main(index)` once set up; waits up to 1 s each to see it
/// online. `test` is x86-64's (hiding or delaying an AP from the
/// bootloader): not offered here.
pub fn start_aps(
    info: &BootInfo,
    test: SmpTest,
    main: fn(u32) -> !,
) -> Result<SmpReport, SmpError> {
    if test == SmpTest::Late {
        crate::warn!("smp: smptest=late is x86-64's (Limine's trampoline replayed): not done here");
    }
    let hide = test == SmpTest::Hide;
    super::psci::init(info.psci);
    *AP_MAIN.lock() = Some(main);
    AP_ROOT.store(super::paging::kernel_root(), Ordering::Release);
    let mut report = SmpReport {
        reported: if hide { 1 } else { info.processors.len() },
        firmware: info.firmware_cpus.as_ref().and_then(|r| r.as_ref().ok()).map(|f| f.len()),
        ..SmpReport::default()
    };
    let mut by_bootloader = [0u64; MAX_CPUS];
    let mut bootloader_count = 0;
    let mut next = 1u32;
    for cpu in info.processors.iter() {
        if cpu.hw_id() == info.processors.bsp_id {
            continue;
        }
        if hide {
            // As if the bootloader had not started it: it powers off.
            cpu.goto_address.store(gala_aarch64_cpu_off as *const () as u64, Ordering::Release);
            continue;
        }
        if next as usize >= MAX_CPUS {
            break;
        }
        if let Some(slot) = by_bootloader.get_mut(bootloader_count) {
            *slot = cpu.hw_id();
            bootloader_count += 1;
        }
        let index = next;
        next += 1;
        let stack = KernelStack::new()?;
        AP_STACK_TOPS[index as usize].store(stack.top(), Ordering::Release);
        HW_IDS[index as usize].store(cpu.hw_id(), Ordering::Relaxed);
        AP_STACKS.lock()[index as usize] = Some(stack);
        cpu.extra_argument.store(u64::from(index), Ordering::Release);
        cpu.goto_address.store(gala_aarch64_ap_entry as *const () as u64, Ordering::Release);
        let until = crate::time::raw_monotonic_ns() + AP_START_NS;
        while ONLINE.load(Ordering::Acquire) & (1 << index) == 0
            && crate::time::raw_monotonic_ns() < until
        {
            core::hint::spin_loop();
        }
        if ONLINE.load(Ordering::Acquire) & (1 << index) == 0 {
            report.failed += 1;
            report.first_failed.get_or_insert(cpu.hw_id() as u32);
            crate::warn!(
                "smp: CPU {index} (MPIDR {:#x}) did not come online within 1 s",
                cpu.hw_id()
            );
        }
    }
    let Some(Ok(firmware)) = info.firmware_cpus else {
        report.online = online_count();
        return Ok(report);
    };
    let bsp = info.processors.bsp_id;
    let by_bootloader = &by_bootloader[..bootloader_count];
    let mut missed = firmware
        .ids()
        .iter()
        .copied()
        .filter(|id| *id != bsp && !by_bootloader.contains(id))
        .peekable();
    if missed.peek().is_none() {
        report.online = online_count();
        return Ok(report);
    }
    if !super::psci::available() {
        report.online = online_count();
        return Ok(report);
    }
    let Some(kernel) = info.kernel_address else {
        crate::warn!("smp: the kernel's physical address is unknown: no CPU started with PSCI");
        report.online = online_count();
        return Ok(report);
    };
    let phys = |virt: u64| virt - kernel.virtual_base + kernel.physical_base;
    let trampoline = phys(gala_aarch64_psci_entry as *const () as u64);
    let ttbr0 = match super::paging::identity_root(trampoline) {
        Ok(root) => root,
        Err(e) => {
            crate::warn!("smp: no identity tables for the PSCI trampoline ({e:?})");
            report.online = online_count();
            return Ok(report);
        }
    };
    // The kernel's translation regime, TTBR0's walks on (EPD0 clear).
    let (tcr, sctlr) = read_sysreg_tcr_sctlr();
    let tcr = tcr & !(1 << 7);
    for id in missed {
        if next as usize >= MAX_CPUS {
            break;
        }
        if hide {
            wait_off(id);
        }
        let index = next;
        next += 1;
        let stack = KernelStack::new()?;
        AP_STACK_TOPS[index as usize].store(stack.top(), Ordering::Release);
        HW_IDS[index as usize].store(id, Ordering::Relaxed);
        AP_STACKS.lock()[index as usize] = Some(stack);
        let start = &PSCI_STARTS[index as usize];
        start.mair.store(super::paging::MAIR, Ordering::Relaxed);
        start.tcr.store(tcr, Ordering::Relaxed);
        start.ttbr0.store(ttbr0, Ordering::Relaxed);
        start.ttbr1.store(super::paging::kernel_root(), Ordering::Relaxed);
        start.sctlr.store(sctlr, Ordering::Relaxed);
        start.entry.store(gala_aarch64_ap_entry_index as *const () as u64, Ordering::Relaxed);
        start.index.store(u64::from(index), Ordering::Relaxed);
        start.state.store(STATE_NOT_REACHED, Ordering::Release);
        cache_line(start, true);
        let context = phys(core::ptr::from_ref(start) as u64);
        let result = super::psci::call(super::psci::CPU_ON, id, trampoline, context);
        let until = crate::time::raw_monotonic_ns() + AP_START_NS;
        while result == 0
            && ONLINE.load(Ordering::Acquire) & (1 << index) == 0
            && crate::time::raw_monotonic_ns() < until
        {
            core::hint::spin_loop();
        }
        if ONLINE.load(Ordering::Acquire) & (1 << index) != 0 {
            report.started_here += 1;
            continue;
        }
        report.failed += 1;
        report.first_failed.get_or_insert(id as u32);
        cache_line(start, false);
        let how = match start.state.load(Ordering::Acquire) {
            _ if result != 0 => "",
            STATE_REACHED => "; it reached the trampoline at EL1",
            STATE_WRONG_EL => "; it entered the trampoline at an EL other than EL1",
            _ => "; it never reached the trampoline",
        };
        crate::warn!(
            "smp: CPU {index} (MPIDR {id:#x}) did not start: PSCI CPU_ON returned {result} ({}){how}",
            super::psci::error_name(result)
        );
    }
    report.online = online_count();
    Ok(report)
}

/// `smptest=hide`: waits (up to 1 s) for the bootloader's AP with MPIDR
/// `id` to be off after its CPU_OFF (AFFINITY_INFO), and says so.
fn wait_off(id: u64) {
    let until = crate::time::raw_monotonic_ns() + AP_START_NS;
    loop {
        let state = super::psci::call(super::psci::AFFINITY_INFO, id, 0, 0);
        if state == super::psci::AFFINITY_OFF {
            crate::info!(
                "smptest: the bootloader's AP (MPIDR {id:#x}) powered itself off (PSCI CPU_OFF)"
            );
            return;
        }
        if crate::time::raw_monotonic_ns() >= until {
            crate::warn!(
                "smptest: the bootloader's AP (MPIDR {id:#x}) is not off after 1 s (AFFINITY_INFO {state})"
            );
            return;
        }
        core::hint::spin_loop();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotStopped(pub u64);

/// Not on aarch64 yet: its one user is the hot swap (ROADMAP SN), which
/// is x86-64's; it comes with it (an ordinary SGI, as x86-64's IPI).
pub fn stop_others<R>(
    _expired: impl Fn() -> bool,
    _f: impl FnOnce(&[u64; MAX_CPUS]) -> R,
) -> Result<R, NotStopped> {
    unported("smp::stop_others")
}

pub fn interrupt_stacks(_cpu: usize) -> [(u64, u64); 3] {
    [(0, 0); 3]
}

pub fn ap_stack_top(_cpu: usize) -> u64 {
    0
}
