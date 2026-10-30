//! Per-CPU data reached through the GS base while in kernel mode.
//!
//! In kernel mode `IA32_GS_BASE` points at this CPU's `PerCpu` and
//! `IA32_KERNEL_GS_BASE` holds the user GS base (the TEB); `swapgs` on every
//! user/kernel transition exchanges them.

use core::mem::offset_of;

use super::cpu;
use super::gdt::{self, Tss};
use super::sync::HwCell;

const IA32_GS_BASE: u32 = 0xc000_0101;
const IA32_KERNEL_GS_BASE: u32 = 0xc000_0102;

#[repr(C)]
pub struct PerCpu {
    self_ptr: u64,
    /// Top of the current thread's kernel stack (loaded on SYSCALL).
    kernel_rsp: u64,
    /// Scratch slot for the user rsp during SYSCALL entry.
    user_rsp: u64,
    /// Index of this CPU (0 for the boot CPU), also in IA32_TSC_AUX.
    index: u64,
    /// This CPU's TSS.
    tss: *mut Tss,
}

impl PerCpu {
    pub const fn new() -> Self {
        PerCpu { self_ptr: 0, kernel_rsp: 0, user_rsp: 0, index: 0, tss: core::ptr::null_mut() }
    }
}

impl Default for PerCpu {
    fn default() -> Self {
        Self::new()
    }
}

pub const KERNEL_RSP_OFFSET: usize = offset_of!(PerCpu, kernel_rsp);
pub const USER_RSP_OFFSET: usize = offset_of!(PerCpu, user_rsp);
const INDEX_OFFSET: usize = offset_of!(PerCpu, index);

/// The boot CPU's block; other CPUs' are allocated by `smp`. Each block is
/// written only by its own CPU, with interrupts disabled.
static BSP: HwCell<PerCpu> = HwCell::new(PerCpu::new());

/// Sets up the boot CPU's block. Requires `gdt::init_bsp` first.
pub fn init_bsp() {
    // SAFETY: single-threaded boot; the boot CPU's block and TSS are used
    // by it alone.
    unsafe { init(BSP.get(), 0, gdt::bsp_tss()) };
}

/// Makes `block` this CPU's per-CPU data.
///
/// # Safety
/// `block` must stay alive and be used by this CPU only; `tss` must be this
/// CPU's TSS. Nothing on this CPU may use GS-relative addressing before.
pub unsafe fn init(block: *mut PerCpu, index: u32, tss: *mut Tss) {
    // SAFETY: forwarded to the caller; GS base MSRs exist on x86_64.
    unsafe {
        (*block).self_ptr = block as u64;
        (*block).index = u64::from(index);
        (*block).tss = tss;
        cpu::wrmsr(IA32_GS_BASE, block as u64);
        cpu::wrmsr(IA32_KERNEL_GS_BASE, 0);
    }
}

fn current() -> *mut PerCpu {
    let block: *mut PerCpu;
    // SAFETY: in kernel mode GS points at this CPU's `PerCpu`, whose first
    // field is its own address.
    unsafe {
        core::arch::asm!("mov {}, gs:[0]", out(reg) block, options(nostack, readonly, preserves_flags))
    };
    block
}

/// This CPU's index (0 for the boot CPU).
pub fn cpu_index() -> u32 {
    let index: u64;
    // SAFETY: as in `current`; a single load, so it cannot be torn by a
    // migration (the value read is the CPU the load ran on).
    unsafe {
        core::arch::asm!(
            "mov {}, gs:[{off}]",
            out(reg) index,
            off = const INDEX_OFFSET,
            options(nostack, readonly, preserves_flags)
        )
    };
    index as u32
}

/// Sets the stack used when this CPU enters the kernel from user mode
/// (SYSCALL, interrupts and exceptions).
pub fn set_kernel_stack(top: u64) {
    let block = current();
    // SAFETY: only called with interrupts disabled during a context switch,
    // so no SYSCALL entry on this CPU can read the field concurrently, and
    // the block and TSS are this CPU's.
    unsafe {
        (*block).kernel_rsp = top;
        gdt::set_rsp0((*block).tss, top);
    }
}

pub fn kernel_stack() -> u64 {
    // SAFETY: plain read of a field written only by this CPU.
    unsafe { (*current()).kernel_rsp }
}

/// Sets the GS base user mode will see after the next `swapgs` to user.
pub fn set_user_gs_base(base: u64) {
    // SAFETY: while in kernel mode this MSR only holds the user's GS base;
    // any value is harmless to the kernel.
    unsafe { cpu::wrmsr(IA32_KERNEL_GS_BASE, base) };
}
