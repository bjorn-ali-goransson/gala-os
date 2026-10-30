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
use super::{apic, cpu, interrupts, paging, syscall};
use crate::boot::{MP_INFO_EXTRA_ARGUMENT, MpInfo, Processors};
use crate::mm::heap::{AllocError, try_box};

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
/// their frames or virtual addresses are reused.
pub fn flush_tlb_everywhere() {
    flush_local();
    flush_cpus(u64::MAX);
}

/// Makes every other CPU that has user space `root` loaded drop its cached
/// translations (this CPU invalidates its own entries as it changes them).
/// Call after changing the space's page tables and before reusing frames
/// they mapped.
pub fn flush_user_space(root: u64) {
    if !STARTED.load(Ordering::Acquire) {
        return;
    }
    // Order the page-table writes before reading CPU_ROOT; a CPU that loads
    // the root later walks the new tables.
    core::sync::atomic::fence(Ordering::SeqCst);
    let mask = (0..MAX_CPUS)
        .filter(|&c| CPU_ROOT[c].load(Ordering::SeqCst) == root)
        .fold(0u64, |m, c| m | 1 << c);
    flush_cpus(mask);
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
    let start = ApStart { stack_top: leaked_stack()?, index, percpu, tables, ist_tops };
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
    let main = AP_MAIN.load(Ordering::Acquire);
    // SAFETY: only `start_aps` stores into AP_MAIN, and it stores a
    // `fn(u32) -> !` before starting any AP.
    let main = unsafe { core::mem::transmute::<u64, fn(u32) -> !>(main) };
    main(start.index)
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
    if has_rdtscp() {
        // SAFETY: see `ap_main`.
        unsafe { cpu::wrmsr(IA32_TSC_AUX, 0) };
    }
}

/// Starts the APs and waits (up to a second) for them to come online;
/// returns the number of CPUs online. Each AP ends in `main(index)`. Call
/// once, after paging, the heap and the boot CPU's local APIC are set up.
pub fn start_aps(processors: &Processors, main: fn(u32) -> !) -> Result<u32, SmpError> {
    CPU_ROOT[0].store(cpu::read_cr3() & !0xfff, Ordering::SeqCst);
    STARTED.store(true, Ordering::Release);
    AP_MAIN.store(main as usize as u64, Ordering::Release);
    AP_CR3.store(paging::kernel_root(), Ordering::Release);
    let mut started = 1;
    for info in processors.iter().filter(|p| p.lapic_id != processors.bsp_lapic_id) {
        if started >= MAX_CPUS as u32 {
            break;
        }
        let start = prepare(started)?;
        info.extra_argument.store(start as *const ApStart as u64, Ordering::Release);
        // Release: the AP reads `extra_argument` and AP_CR3 after this.
        info.goto_address.store(gala_ap_entry as *const () as u64, Ordering::Release);
        started += 1;
    }
    let deadline = crate::time::monotonic_ns() + 1_000_000_000;
    while online_count() < started && crate::time::monotonic_ns() < deadline {
        core::hint::spin_loop();
    }
    Ok(online_count())
}
