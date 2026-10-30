//! Starting the application processors (APs).
//!
//! Limine parks every AP and starts it at the address written to its
//! `goto_address` (Limine boot protocol, MP feature), still on the
//! bootloader's page tables, GDT and stack. `gala_ap_entry` switches to the
//! kernel's tables and a kernel stack, then `ap_main` loads the CPU's own
//! GDT/TSS, per-CPU block, IDT, SYSCALL MSRs and local APIC.
//!
//! For now the APs only idle (`hlt` with interrupts enabled); the scheduler
//! does not use them yet.

use alloc::boxed::Box;
use core::arch::global_asm;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

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
    ONLINE_MASK.fetch_or(1 << start.index, Ordering::AcqRel);
    ONLINE.fetch_add(1, Ordering::AcqRel);
    loop {
        cpu::enable_interrupts_and_halt();
    }
}

fn has_rdtscp() -> bool {
    cpu::cpuid(0x8000_0001, 0).edx & (1 << 27) != 0
}

/// Sets the boot CPU's processor number (0) for RDTSCP.
pub fn init_bsp() {
    if has_rdtscp() {
        // SAFETY: see `ap_main`.
        unsafe { cpu::wrmsr(IA32_TSC_AUX, 0) };
    }
}

/// Starts the APs and waits (up to a second) for them to come online;
/// returns the number of CPUs online. Call once, after paging, the heap and
/// the boot CPU's local APIC are set up.
pub fn start_aps(processors: &Processors) -> Result<u32, SmpError> {
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
