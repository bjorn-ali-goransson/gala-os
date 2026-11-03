//! Starting the application processors (APs).
//!
//! Limine parks every AP and starts it at the address written to its
//! `goto_address` (Limine boot protocol, MP feature), still on the
//! bootloader's page tables, GDT and stack. `gala_ap_entry` switches to the
//! kernel's tables and a kernel stack, then `ap_main` loads the CPU's own
//! GDT/TSS, per-CPU block, IDT, SYSCALL MSRs and local APIC.
//!
//! Each AP then enters the function given to `start_aps` (the scheduler's
//! per-CPU loop).

use alloc::boxed::Box;
use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use super::context::{KernelStack, StackError};
use super::gdt::{self, CpuTables};
use super::percpu::{self, PerCpu};
use super::{apboot, apic, cpu, interrupts, paging, syscall};
use crate::boot::{BootInfo, MP_INFO_EXTRA_ARGUMENT, MpInfo};
use crate::mm::heap::{AllocError, try_box};
pub use apboot::{Shelter, shelter_bootloader_aps, shelter_state};

/// Processors beyond this many are left parked (affinity masks are 64 bits).
pub const MAX_CPUS: usize = 64;

const IA32_TSC_AUX: u32 = 0xc000_0103;

/// Number of CPUs running kernel code, and their mask (bit = CPU index).
static ONLINE: AtomicU32 = AtomicU32::new(1);
static ONLINE_MASK: AtomicU64 = AtomicU64::new(1);

/// Local APIC id of each CPU index (valid for online CPUs).
static APIC_IDS: [AtomicU32; MAX_CPUS] = [const { AtomicU32::new(0) }; MAX_CPUS];

/// Vector of the reschedule IPI (wakes an idle CPU from `hlt`).
pub const RESCHEDULE_VECTOR: u8 = 0x31;

/// Vector of the TLB shootdown IPI.
pub const TLB_FLUSH_VECTOR: u8 = 0x32;

/// TLB flush requests: CPU `i` has been asked for `FLUSH_REQUESTED[i]`
/// flushes and has done all requests up to `FLUSH_DONE[i]`. A flush that
/// starts after reading the request count covers every request counted
/// (each requester changed its mappings before counting).
static FLUSH_REQUESTED: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static FLUSH_DONE: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// The page-table root each CPU has loaded (`paging::activate`), to find
/// the CPUs that may cache a user space's translations.
static CPU_ROOT: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Set once per-CPU blocks exist on the boot CPU and APs may run.
static STARTED: AtomicBool = AtomicBool::new(false);

/// Shootdowns that had to interrupt other CPUs, and IPIs sent for them.
static SHOOTDOWNS: AtomicU64 = AtomicU64::new(0);
static FLUSH_IPIS: AtomicU64 = AtomicU64::new(0);

/// (shootdowns, flush IPIs) so far.
pub fn flush_stats() -> (u64, u64) {
    (SHOOTDOWNS.load(Ordering::Relaxed), FLUSH_IPIS.load(Ordering::Relaxed))
}

/// Flushes this CPU's whole TLB, global (kernel) entries included.
fn flush_local() {
    let cr4 = cpu::read_cr4();
    const CR4_PGE: u64 = 1 << 7;
    // SAFETY: toggling CR4.PGE flushes all TLB entries and changes nothing
    // else; PGE is set again right away.
    unsafe {
        core::arch::asm!(
            "mov cr4, {off}",
            "mov cr4, {on}",
            off = in(reg) cr4 & !CR4_PGE,
            on = in(reg) cr4,
            options(nostack, preserves_flags)
        )
    };
}

/// Does the flushes requested of this CPU so far, if any.
fn serve_flush(me: usize) {
    let requested = FLUSH_REQUESTED[me].load(Ordering::Acquire);
    if requested > FLUSH_DONE[me].load(Ordering::Acquire) {
        flush_local();
        FLUSH_DONE[me].fetch_max(requested, Ordering::AcqRel);
    }
}

/// Called while spinning for a lock with interrupts off: a CPU holding
/// that lock may be waiting for this CPU to flush its TLB.
pub fn serve_pending_flush() {
    if STARTED.load(Ordering::Acquire) {
        serve_flush(percpu::cpu_index() as usize);
    }
}

/// Records that this CPU loads page-table root `root` (before loading it).
pub fn note_root(root: u64) {
    if STARTED.load(Ordering::Acquire) {
        CPU_ROOT[percpu::cpu_index() as usize].store(root, Ordering::SeqCst);
    }
}

/// Flushes the TLBs of the CPUs in `mask` (never this one) and waits until
/// they have. Safe with locks held: CPUs spinning for a lock serve flush
/// requests (`serve_pending_flush`), and so does this one while it waits.
fn flush_cpus(mask: u64) {
    let me = percpu::cpu_index() as usize;
    let mask = mask & online_mask() & !(1 << me);
    if mask == 0 {
        return;
    }
    SHOOTDOWNS.fetch_add(1, Ordering::Relaxed);
    let mut wanted = [0u64; MAX_CPUS];
    for cpu in (0..MAX_CPUS).filter(|c| mask & (1 << c) != 0) {
        wanted[cpu] = FLUSH_REQUESTED[cpu].fetch_add(1, Ordering::AcqRel) + 1;
        apic::send_ipi(APIC_IDS[cpu].load(Ordering::Relaxed), TLB_FLUSH_VECTOR);
        FLUSH_IPIS.fetch_add(1, Ordering::Relaxed);
    }
    for cpu in (0..MAX_CPUS).filter(|c| mask & (1 << c) != 0) {
        while FLUSH_DONE[cpu].load(Ordering::Acquire) < wanted[cpu] {
            serve_flush(me);
            core::hint::spin_loop();
        }
    }
}

/// Flushes the TLBs of all CPUs (kernel and user entries) and waits until
/// every online CPU has. Used after kernel mappings are removed, before
/// their frames or virtual addresses are reused. Interrupts stay off from
/// the local flush to the end: a thread moved to another CPU in between
/// would leave that CPU out (`flush_cpus` skips the one it runs on).
pub fn flush_tlb_everywhere() {
    let restore = cpu::disable_interrupts();
    flush_local();
    flush_cpus(u64::MAX);
    if restore {
        cpu::enable_interrupts();
    }
}

/// `tlbparanoid` (kernel command line, for telling TLB bugs from others):
/// every change to a user space flushes every CPU's whole TLB, and every
/// context switch reloads CR3.
static TLB_PARANOID: AtomicBool = AtomicBool::new(false);

pub fn set_tlb_paranoid(on: bool) {
    TLB_PARANOID.store(on, Ordering::Relaxed);
}

pub fn tlb_paranoid() -> bool {
    TLB_PARANOID.load(Ordering::Relaxed)
}

/// Shootdowns whose caller had moved to another CPU since its local
/// invalidation (that CPU was flushed as well).
static MOVED_FLUSHES: AtomicU64 = AtomicU64::new(0);

/// Shootdowns done after the changing thread moved to another CPU.
pub fn moved_flushes() -> u64 {
    MOVED_FLUSHES.load(Ordering::Relaxed)
}

/// Makes every CPU that has user space `root` loaded drop its cached
/// translations. `changed_on` is the CPU that invalidated its own entries
/// as it changed them (`invlpg`), which need not be this one: the space's
/// lock is released before this runs, and an interrupt held back while
/// it was held is taken right there and may move the thread to another
/// CPU. That CPU is then flushed too if it has the space loaded (it was
/// left out before 2026-11-03, as "this CPU", and kept translations of
/// pages whose frames were then freed and reused: the Java heap
/// corruption, `shootdown_test`). Interrupts stay off from choosing the
/// CPUs to the end, so this CPU stays this CPU. Call after changing the
/// space's page tables and before reusing frames they mapped.
pub fn flush_user_space(root: u64, changed_on: Option<u32>) {
    if !STARTED.load(Ordering::Acquire) {
        return;
    }
    if tlb_paranoid() {
        flush_tlb_everywhere();
        return;
    }
    let restore = cpu::disable_interrupts();
    // Order the page-table writes before reading CPU_ROOT; a CPU that loads
    // the root later walks the new tables.
    core::sync::atomic::fence(Ordering::SeqCst);
    let me = percpu::cpu_index();
    if changed_on.is_some_and(|c| c != me) {
        MOVED_FLUSHES.fetch_add(1, Ordering::Relaxed);
        if CPU_ROOT[me as usize].load(Ordering::SeqCst) == root {
            flush_local();
        }
    }
    let mask = (0..MAX_CPUS)
        .filter(|&c| CPU_ROOT[c].load(Ordering::SeqCst) == root)
        .fold(0u64, |m, c| m | 1 << c);
    flush_cpus(mask);
    if restore {
        cpu::enable_interrupts();
    }
}

fn on_flush_ipi(_: &mut interrupts::TrapFrame) {
    serve_flush(percpu::cpu_index() as usize);
    apic::eoi();
}

/// Where an AP goes once it is set up (the scheduler's per-CPU start).
static AP_MAIN: AtomicU64 = AtomicU64::new(0);

/// The kernel's PML4, for `gala_ap_entry` to load.
static AP_CR3: AtomicU64 = AtomicU64::new(0);

pub fn online_count() -> u32 {
    ONLINE.load(Ordering::Acquire)
}

pub fn online_mask() -> u64 {
    ONLINE_MASK.load(Ordering::Acquire)
}

/// What an AP needs to set itself up; leaked, as the CPU uses it forever.
/// `stack_top` must stay first (read by `gala_ap_entry`).
#[repr(C)]
struct ApStart {
    stack_top: u64,
    index: u32,
    /// Set by the CPU that runs on this: a second one (an AP the kernel
    /// gave up on that arrives late through the kernel's trampoline) halts.
    taken: AtomicBool,
    percpu: *mut PerCpu,
    tables: *mut CpuTables,
    ist_tops: [u64; 3],
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

/// A kernel stack that is never freed; returns its top.
fn leaked_stack() -> Result<u64, SmpError> {
    let stack = try_box(KernelStack::new()?)?;
    Ok(Box::leak(stack).top())
}

fn prepare(index: u32) -> Result<&'static ApStart, SmpError> {
    let percpu = Box::leak(try_box(PerCpu::new())?);
    let tables = Box::leak(try_box(CpuTables::new())?);
    let ist_tops = [leaked_stack()?, leaked_stack()?, leaked_stack()?];
    let start = ApStart {
        stack_top: leaked_stack()?,
        index,
        taken: AtomicBool::new(false),
        percpu,
        tables,
        ist_tops,
    };
    Ok(Box::leak(try_box(start)?))
}

unsafe extern "C" {
    fn gala_ap_entry(info: *const MpInfo) -> !;
}

// Entered with rdi = the AP's `limine_mp_info`, interrupts disabled, on
// the bootloader's page tables. Enables what the kernel's mappings need
// (NX, the kernel's PAT, global pages) as `paging::init` did on the boot
// CPU, then loads the kernel's tables and stack.
global_asm!(
    ".globl gala_ap_entry",
    "gala_ap_entry:",
    "cli",
    "mov ecx, 0xc0000080", // IA32_EFER |= NXE
    "rdmsr",
    "or eax, 1 << 11",
    "wrmsr",
    "mov ecx, 0x277", // IA32_PAT
    "mov eax, {pat_low}",
    "mov edx, {pat_high}",
    "wrmsr",
    "mov rax, cr4", // CR4.PGE
    "or rax, 1 << 7",
    "mov cr4, rax",
    "mov rax, [rdi + {extra}]",
    "mov rcx, [rip + {cr3}]",
    "mov cr3, rcx",
    "mov rsp, [rax]",
    "xor ebp, ebp",
    "mov rdi, rax",
    "call {main}",
    "ud2",
    extra = const MP_INFO_EXTRA_ARGUMENT,
    pat_low = const paging::PAT_VALUE as u32,
    pat_high = const (paging::PAT_VALUE >> 32) as u32,
    cr3 = sym AP_CR3,
    main = sym ap_main,
);

extern "C" fn ap_main(start: &'static ApStart) -> ! {
    if start.taken.swap(true, Ordering::AcqRel) {
        // Interrupts are off (gala_ap_entry): this CPU stays here.
        cpu::halt_forever();
    }
    // SAFETY: `prepare` made the tables, stacks and per-CPU block for this
    // CPU alone and leaked them, so they live forever.
    unsafe {
        let tss = gdt::load(start.tables, start.ist_tops);
        percpu::init(start.percpu, start.index, tss);
    }
    cpu::enable_sse();
    interrupts::load();
    syscall::init_cpu();
    apic::init_ap();
    if has_rdtscp() {
        // SAFETY: IA32_TSC_AUX is architected where RDTSCP exists; it only
        // changes what RDTSCP/RDPID report.
        unsafe { cpu::wrmsr(IA32_TSC_AUX, u64::from(start.index)) };
    }
    APIC_IDS[start.index as usize].store(apic::id(), Ordering::Relaxed);
    CPU_ROOT[start.index as usize].store(paging::kernel_root(), Ordering::SeqCst);
    ONLINE_MASK.fetch_or(1 << start.index, Ordering::AcqRel);
    ONLINE.fetch_add(1, Ordering::AcqRel);
    super::bootphase::mark_n("ap_online", u64::from(start.index));
    let main = AP_MAIN.load(Ordering::Acquire);
    // SAFETY: only `start_aps` stores into AP_MAIN, and it stores a
    // `fn(u32) -> !` before starting any AP.
    let main = unsafe { core::mem::transmute::<u64, fn(u32) -> !>(main) };
    main(start.index)
}

/// (CPU index, local APIC id) of every online CPU.
pub fn online_cpus() -> impl Iterator<Item = (u32, u32)> {
    let mask = online_mask();
    (0..MAX_CPUS as u32)
        .filter(move |&i| mask & (1 << i) != 0)
        .map(|i| (i, APIC_IDS[i as usize].load(Ordering::Relaxed)))
}

/// Interrupts CPU `index` (if online) so that it reschedules; an idle CPU
/// leaves `hlt` and looks for work.
pub fn send_reschedule(index: u32) {
    if index < MAX_CPUS as u32 && online_mask() & (1 << index) != 0 {
        apic::send_ipi(APIC_IDS[index as usize].load(Ordering::Relaxed), RESCHEDULE_VECTOR);
    }
}

fn has_rdtscp() -> bool {
    cpu::cpuid(0x8000_0001, 0).edx & (1 << 27) != 0
}

/// Sets the boot CPU's processor number (0) for RDTSCP and the reschedule
/// vector (its handler does nothing: the interrupt itself ends `hlt`).
pub fn init_bsp() {
    APIC_IDS[0].store(apic::id(), Ordering::Relaxed);
    interrupts::set_handler(RESCHEDULE_VECTOR, |_| apic::eoi());
    interrupts::set_handler(TLB_FLUSH_VECTOR, on_flush_ipi);
    super::handover::init();
    if has_rdtscp() {
        // SAFETY: see `ap_main`.
        unsafe { cpu::wrmsr(IA32_TSC_AUX, 0) };
    }
}

/// Boot-test variants of AP start-up (`smptest=`): `Hide` treats the APs
/// the bootloader started as missed (the kernel starts them itself);
/// `Late` also makes one run the bootloader's trampoline again first, as
/// an AP that answers the bootloader too late does (`apboot`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmpTest {
    None,
    Hide,
    Late,
}

/// What `start_aps` found and did, for the boot log.
#[derive(Debug, Default)]
pub struct SmpReport {
    pub online: u32,
    /// CPUs the bootloader reported (the boot CPU included).
    pub reported: usize,
    /// CPUs the firmware's MADT lists as enabled (`None`: not read).
    pub firmware: Option<usize>,
    /// APs the bootloader did not start that the kernel started.
    pub started_here: u32,
    /// APs the bootloader did not start that did not start at all.
    pub failed: u32,
    /// The first of those, by local APIC id.
    pub first_failed: Option<u32>,
    /// What `smptest=late` saw.
    pub late_replay: Option<&'static str>,
    /// The CPUs the bootloader started took over a second to come.
    pub slow: bool,
}

/// Starts the APs and waits (up to a second) for them to come online;
/// then starts, with the kernel's own trampoline, the processors the
/// firmware lists that the bootloader did not start (`apboot`). Each AP
/// ends in `main(index)`. Call once, after paging, the heap and the boot
/// CPU's local APIC are set up.
pub fn start_aps(
    info: &BootInfo,
    test: SmpTest,
    main: fn(u32) -> !,
) -> Result<SmpReport, SmpError> {
    let processors = &info.processors;
    // After a handover the CPUs are the previous kernel's, all parked.
    let test = if info.handover.is_some() { SmpTest::None } else { test };
    CPU_ROOT[0].store(cpu::read_cr3() & !0xfff, Ordering::SeqCst);
    STARTED.store(true, Ordering::Release);
    AP_MAIN.store(main as usize as u64, Ordering::Release);
    AP_CR3.store(paging::kernel_root(), Ordering::Release);
    // `smptest`: as if the bootloader had reported the boot CPU alone.
    let reported = if test == SmpTest::None { processors.len() } else { 1 };
    let mut report = SmpReport { reported, ..SmpReport::default() };
    let mut started = 1;
    let mut by_bootloader = [0u32; MAX_CPUS];
    let mut bootloader_count = 0;
    for info in processors.iter().filter(|p| p.lapic_id != processors.bsp_lapic_id) {
        if started >= MAX_CPUS as u32 || test != SmpTest::None {
            break;
        }
        let start = prepare(started)?;
        info.extra_argument.store(start as *const ApStart as u64, Ordering::Release);
        // Release: the AP reads `extra_argument` and AP_CR3 after this.
        info.goto_address.store(gala_ap_entry as *const () as u64, Ordering::Release);
        by_bootloader[bootloader_count] = info.lapic_id;
        bootloader_count += 1;
        started += 1;
    }
    // A started AP's vCPU may be slow to run on a loaded emulator host
    // (a boot once had 1 of 2 CPUs online after a second): a second is
    // the normal wait, ten the most.
    let t0 = crate::time::raw_monotonic_ns();
    let mut warned = false;
    while online_count() < started && crate::time::raw_monotonic_ns() < t0 + 10_000_000_000 {
        if !warned && crate::time::raw_monotonic_ns() >= t0 + 1_000_000_000 {
            warned = true;
            crate::warn!(
                "smp: {} of {started} CPUs online after 1 s; waiting up to 10 s for the rest",
                online_count()
            );
        }
        core::hint::spin_loop();
    }
    report.slow = warned;
    // After a kernel handover the previous kernel's CPUs are all there is.
    let Some(Ok(firmware)) = info.firmware_cpus else {
        report.online = online_count();
        return Ok(report);
    };
    report.firmware = Some(firmware.len());
    let bsp = processors.bsp_lapic_id;
    let by_bootloader = &by_bootloader[..bootloader_count];
    let mut missed =
        firmware.ids().iter().copied().filter(|id| *id != bsp && !by_bootloader.contains(id));
    let Some(first) = missed.next() else {
        report.online = online_count();
        return Ok(report);
    };
    let hhdm = crate::mm::hhdm_offset();
    let Some(low) = low_pages(info) else {
        crate::warn!("smp: no free pages below 1 MiB for the AP trampoline");
        report.online = online_count();
        return Ok(report);
    };
    let entry = gala_ap_entry as *const () as u64;
    let Some(vector) = apboot::prepare_trampoline(hhdm, low, paging::kernel_root(), entry) else {
        crate::warn!("smp: cannot set up the AP trampoline at {:#x}", low.0[0]);
        report.online = online_count();
        return Ok(report);
    };
    if test == SmpTest::Late {
        report.late_replay = Some(apboot::replay_late_start(hhdm, first));
    }
    for id in core::iter::once(first).chain(missed) {
        if started >= MAX_CPUS as u32 {
            break;
        }
        let start = prepare(started)?;
        let mp = Box::leak(try_box(MpInfo::new(0, id))?);
        mp.extra_argument.store(start as *const ApStart as u64, Ordering::Release);
        apboot::set_trampoline_info(hhdm, low, mp as *const MpInfo as u64);
        let bit = 1u64 << started;
        // An INIT also takes the AP out of wherever it is (the
        // bootloader's parking loop, or its trampoline still pending).
        for round in 0..4 {
            // A startup IPI of the bootloader's still pending would send
            // it through Limine's trampoline once more: keep that safe.
            apboot::repair_limine_block(hhdm);
            apboot::init_sipi_sipi(id, vector);
            let deadline = crate::time::SpinDeadline::after_ns(250_000_000 << round);
            while online_mask() & bit == 0 && !deadline.passed() {
                core::hint::spin_loop();
            }
            if online_mask() & bit != 0 {
                break;
            }
        }
        started += 1;
        if online_mask() & bit != 0 {
            report.started_here += 1;
        } else {
            report.failed += 1;
            report.first_failed.get_or_insert(id);
            crate::warn!(
                "smp: CPU with APIC id {id} did not start (reached long mode: {})",
                apboot::trampoline_reached(hhdm, low)
            );
        }
    }
    report.online = online_count();
    Ok(report)
}

/// Four usable pages below 1 MiB (the frame allocator never hands these
/// out: `boot::LOW_MEMORY_END`).
fn low_pages(info: &BootInfo) -> Option<apboot::LowPages> {
    let mut pages = [0u64; 4];
    let mut n = 0;
    for r in info.memory_map.iter().filter(|r| r.kind == crate::mm::memmap::RegionKind::Usable) {
        let start = r.base.max(0x1000).next_multiple_of(4096);
        let end = r.end().min(crate::boot::LOW_MEMORY_END) & !0xfff;
        let mut p = start;
        while p < end && n < pages.len() {
            pages[n] = p;
            n += 1;
            p += 4096;
        }
    }
    (n == pages.len()).then_some(apboot::LowPages(pages))
}
