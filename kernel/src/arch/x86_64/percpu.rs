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
const IA32_FS_BASE: u32 = 0xc000_0100;

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
    /// The personality of the thread running here (`Personality::index`),
    /// whose dispatcher its system calls go to (`syscall::dispatch`).
    personality: u64,
    /// The FS base loaded for user mode (IA32_FS_BASE), kept here so a
    /// switch between threads with the same one (every NT thread: 0)
    /// costs no MSR write.
    user_fs_base: u64,
    /// The client id of the thread running here (an NT thread id, a Linux
    /// tid; 0 for kernel threads): what the event stream names as the
    /// thread doing the work (`"by"`).
    client: u64,
}

impl PerCpu {
    pub const fn new() -> Self {
        PerCpu {
            self_ptr: 0,
            kernel_rsp: 0,
            user_rsp: 0,
            index: 0,
            tss: core::ptr::null_mut(),
            personality: 0,
            user_fs_base: 0,
            client: 0,
        }
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
const PERSONALITY_OFFSET: usize = offset_of!(PerCpu, personality);
const CLIENT_OFFSET: usize = offset_of!(PerCpu, client);

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
        cpu::wrmsr(IA32_FS_BASE, 0);
        (*block).user_fs_base = 0;
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

/// Sets the client id of the thread this CPU runs (0: a kernel thread).
pub fn set_client(client: u64) {
    let block = current();
    // SAFETY: only called with interrupts disabled during a context switch;
    // the block is this CPU's.
    unsafe { (*block).client = client };
}

/// The client id of the thread running here (0: a kernel thread).
pub fn client() -> u64 {
    let raw: u64;
    // SAFETY: as in `cpu_index`: a single GS-relative load, so a migration
    // cannot tear it: whichever CPU the load runs on is running the
    // calling thread, whose id the switch to it stored there.
    unsafe {
        core::arch::asm!(
            "mov {}, gs:[{off}]",
            out(reg) raw,
            off = const CLIENT_OFFSET,
            options(nostack, readonly, preserves_flags)
        )
    };
    raw
}

/// Sets the personality of the thread this CPU runs (its system calls'
/// dispatcher).
pub fn set_personality(personality: crate::personality::Personality) {
    let block = current();
    // SAFETY: only called with interrupts disabled during a context switch;
    // the block is this CPU's and only this CPU reads the field.
    unsafe { (*block).personality = personality.index() as u64 };
}

/// The personality of the thread this CPU runs (NT if the field holds a
/// number no personality has, which `set_personality` never writes).
pub fn personality() -> crate::personality::Personality {
    let raw: u64;
    // SAFETY: as in `cpu_index`: a single GS-relative load, so a migration
    // cannot tear it (system calls run with interrupts on): whichever CPU
    // the load runs on is running the calling thread, whose personality
    // the switch to it stored there.
    unsafe {
        core::arch::asm!(
            "mov {}, gs:[{off}]",
            out(reg) raw,
            off = const PERSONALITY_OFFSET,
            options(nostack, readonly, preserves_flags)
        )
    };
    crate::personality::Personality::from_index(raw as usize).unwrap_or_default()
}

pub fn kernel_stack() -> u64 {
    // SAFETY: plain read of a field written only by this CPU.
    unsafe { (*current()).kernel_rsp }
}

/// The GS base user mode will see after the next `swapgs` to user.
pub fn user_gs_base() -> u64 {
    // SAFETY: reading an architected MSR has no side effects.
    unsafe { cpu::rdmsr(IA32_KERNEL_GS_BASE) }
}

/// Sets the GS base user mode will see after the next `swapgs` to user.
pub fn set_user_gs_base(base: u64) {
    // SAFETY: while in kernel mode this MSR only holds the user's GS base;
    // any value is harmless to the kernel.
    unsafe { cpu::wrmsr(IA32_KERNEL_GS_BASE, base) };
}

/// Sets the FS base user mode sees (the kernel does not use FS; a Linux
/// thread's TLS pointer). Written to the MSR only when it changes.
/// Called with interrupts disabled (a context switch, or under the
/// scheduler's lock); `base` must be canonical.
pub fn set_user_fs_base(base: u64) {
    let block = current();
    // SAFETY: the block is this CPU's and interrupts are off, so no switch
    // reads or writes the field meanwhile; IA32_FS_BASE only affects
    // FS-relative accesses, which the kernel never makes, and a canonical
    // value cannot fault.
    unsafe {
        if (*block).user_fs_base != base {
            cpu::wrmsr(IA32_FS_BASE, base);
            (*block).user_fs_base = base;
        }
    }
}
