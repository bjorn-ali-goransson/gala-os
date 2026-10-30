//! Per-CPU data reached through the GS base while in kernel mode.
//!
//! In kernel mode `IA32_GS_BASE` points at this CPU's `PerCpu` and
//! `IA32_KERNEL_GS_BASE` holds the user GS base (the TEB); `swapgs` on every
//! user/kernel transition exchanges them.

use core::mem::offset_of;

use super::cpu;
use super::gdt;
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
}

pub const KERNEL_RSP_OFFSET: usize = offset_of!(PerCpu, kernel_rsp);
pub const USER_RSP_OFFSET: usize = offset_of!(PerCpu, user_rsp);

// TODO(smp): one per CPU. Written only with interrupts disabled.
static BSP: HwCell<PerCpu> = HwCell::new(PerCpu { self_ptr: 0, kernel_rsp: 0, user_rsp: 0 });

pub fn init_bsp() {
    let ptr = BSP.get();
    // SAFETY: single-threaded boot; GS base MSRs exist on x86_64. Nothing
    // uses GS-relative addressing before this point.
    unsafe {
        (*ptr).self_ptr = ptr as u64;
        cpu::wrmsr(IA32_GS_BASE, ptr as u64);
        cpu::wrmsr(IA32_KERNEL_GS_BASE, 0);
    }
}

/// Sets the stack used when this CPU enters the kernel from user mode
/// (SYSCALL, interrupts and exceptions).
pub fn set_kernel_stack(top: u64) {
    // SAFETY: only called with interrupts disabled during a context switch,
    // so no SYSCALL entry on this CPU can read the field concurrently.
    unsafe { (*BSP.get()).kernel_rsp = top };
    gdt::set_rsp0(top);
}

pub fn kernel_stack() -> u64 {
    // SAFETY: plain read of a field written only by this CPU.
    unsafe { (*BSP.get()).kernel_rsp }
}

/// Sets the GS base user mode will see after the next `swapgs` to user.
pub fn set_user_gs_base(base: u64) {
    // SAFETY: while in kernel mode this MSR only holds the user's GS base;
    // any value is harmless to the kernel.
    unsafe { cpu::wrmsr(IA32_KERNEL_GS_BASE, base) };
}
