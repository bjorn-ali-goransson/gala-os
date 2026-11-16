//! The per-CPU block: TPIDR_EL1 holds the address of this CPU's entry in
//! `BLOCKS` (set by `init_bsp` on the boot CPU; the other CPUs' with SMP).
//! It keeps what the scheduler sets for the running thread: the kernel
//! stack an exception from EL0 runs on, the client, the personality, and
//! the user thread-pointer bases the return to EL0 loads. Those bases keep
//! x86-64's names in the interface (the TEB base, the FS base: ROADMAP
//! PT's remaining items); on aarch64 they are the values for TPIDR_EL0 and
//! x18, which user mode's entry and return (the port) will use.

use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

use super::smp::MAX_CPUS;
use super::unported;
use crate::personality::Personality;

struct Block {
    index: AtomicU32,
    kernel_stack: AtomicU64,
    client: AtomicU64,
    personality: AtomicU8,
    user_gs_base: AtomicU64,
    user_fs_base: AtomicU64,
}

static BLOCKS: [Block; MAX_CPUS] = [const {
    Block {
        index: AtomicU32::new(0),
        kernel_stack: AtomicU64::new(0),
        client: AtomicU64::new(0),
        personality: AtomicU8::new(Personality::Nt as u8),
        user_gs_base: AtomicU64::new(0),
        user_fs_base: AtomicU64::new(0),
    }
}; MAX_CPUS];

/// This CPU's block (the boot CPU's before `init_bsp`).
fn block() -> &'static Block {
    let at: u64;
    // SAFETY: reads TPIDR_EL1 (0, or a `BLOCKS` entry's address).
    unsafe {
        core::arch::asm!("mrs {}, tpidr_el1", out(reg) at, options(nomem, nostack, preserves_flags))
    };
    if at == 0 {
        return &BLOCKS[0];
    }
    // SAFETY: only `install` writes TPIDR_EL1, with an entry of `BLOCKS`
    // (a static).
    unsafe { &*core::ptr::with_exposed_provenance::<Block>(at as usize) }
}

/// Points TPIDR_EL1 at block `index` on this CPU.
pub(super) fn install(index: u32) {
    let b = &BLOCKS[index as usize];
    b.index.store(index, Ordering::Relaxed);
    let at = core::ptr::from_ref(b).expose_provenance() as u64;
    // SAFETY: TPIDR_EL1 is the kernel's own register; `block` reads it.
    unsafe {
        core::arch::asm!("msr tpidr_el1, {}", in(reg) at, options(nomem, nostack, preserves_flags))
    };
}

/// The boot CPU's block (index 0).
pub fn init_bsp() {
    install(0);
}

/// This CPU's index (0 for the boot CPU).
pub fn cpu_index() -> u32 {
    block().index.load(Ordering::Relaxed)
}

/// The stack an exception from EL0 runs on (the running thread's kernel
/// stack's top).
pub fn set_kernel_stack(top: u64) {
    block().kernel_stack.store(top, Ordering::Relaxed);
}

pub fn kernel_stack() -> u64 {
    block().kernel_stack.load(Ordering::Relaxed)
}

/// The client id of the thread this CPU runs (0: a kernel thread).
pub fn set_client(client: u64) {
    block().client.store(client, Ordering::Relaxed);
}

pub fn client() -> u64 {
    block().client.load(Ordering::Relaxed)
}

/// The personality of the thread this CPU runs (its system calls'
/// dispatcher).
pub fn set_personality(personality: Personality) {
    block().personality.store(personality as u8, Ordering::Relaxed);
}

/// The personality of the thread this CPU runs (NT for a number no
/// personality has, which `set_personality` never writes).
pub fn personality() -> Personality {
    match block().personality.load(Ordering::Relaxed) {
        x if x == Personality::Linux as u8 => Personality::Linux,
        _ => Personality::Nt,
    }
}

/// The thread-pointer base user mode will see (x86-64's GS base: an NT
/// thread's TEB).
pub fn user_gs_base() -> u64 {
    block().user_gs_base.load(Ordering::Relaxed)
}

pub fn set_user_gs_base(base: u64) {
    block().user_gs_base.store(base, Ordering::Relaxed);
}

/// The other thread-pointer base (x86-64's FS base: a Linux thread's TLS).
pub fn set_user_fs_base(base: u64) {
    block().user_fs_base.store(base, Ordering::Relaxed);
}

pub fn user_fs_base() -> u64 {
    block().user_fs_base.load(Ordering::Relaxed)
}

/// No 32-bit code here.
pub fn load_teb32(_teb32: u32) {
    unported("percpu::load_teb32")
}
