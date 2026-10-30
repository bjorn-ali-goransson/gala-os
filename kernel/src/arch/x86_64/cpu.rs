//! CPU control.

use core::arch::asm;

/// Disables interrupts and halts this CPU forever.
pub fn halt_forever() -> ! {
    loop {
        // SAFETY: `cli; hlt` only stops this CPU; it touches no memory.
        unsafe { asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

/// Faulting linear address of the most recent page fault.
pub fn read_cr2() -> u64 {
    let value;
    // SAFETY: reading CR2 has no side effects.
    unsafe { asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

/// # Safety
/// `msr` must exist on this CPU.
pub unsafe fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: forwarded to the caller.
    unsafe {
        asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags))
    };
    u64::from(hi) << 32 | u64::from(lo)
}

/// # Safety
/// `msr` must exist, and writing `value` must not break memory safety.
pub unsafe fn wrmsr(msr: u32, value: u64) {
    // SAFETY: forwarded to the caller.
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") value as u32, in("edx") (value >> 32) as u32,
             options(nostack, preserves_flags));
    }
}

pub struct CpuidResult {
    pub eax: u32,
    pub ebx: u32,
    pub ecx: u32,
    pub edx: u32,
}

pub fn cpuid(leaf: u32, subleaf: u32) -> CpuidResult {
    let r = core::arch::x86_64::__cpuid_count(leaf, subleaf);
    CpuidResult { eax: r.eax, ebx: r.ebx, ecx: r.ecx, edx: r.edx }
}

pub fn read_cr3() -> u64 {
    let value;
    // SAFETY: reading CR3 has no side effects.
    unsafe { asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

pub fn read_cr4() -> u64 {
    let value;
    // SAFETY: reading CR4 has no side effects.
    unsafe { asm!("mov {}, cr4", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

const RFLAGS_IF: u64 = 1 << 9;

pub fn interrupts_enabled() -> bool {
    let rflags: u64;
    // SAFETY: reading RFLAGS has no side effects.
    unsafe { asm!("pushfq", "pop {}", out(reg) rflags, options(nomem, preserves_flags)) };
    rflags & RFLAGS_IF != 0
}

/// Disables interrupts and returns whether they were enabled before.
pub fn disable_interrupts() -> bool {
    let was = interrupts_enabled();
    // SAFETY: masking interrupts cannot break memory safety.
    unsafe { asm!("cli", options(nomem, nostack)) };
    was
}

/// Enables interrupts. Handlers must be installed (`interrupts::init`).
pub fn enable_interrupts() {
    // SAFETY: the IDT is loaded before anyone can call this (it is only used
    // after `interrupts::init`, and every vector has a handler).
    unsafe { asm!("sti", options(nomem, nostack)) };
}

/// Atomically enables interrupts and halts until the next one arrives.
pub fn enable_interrupts_and_halt() {
    // SAFETY: as for `enable_interrupts`; `sti; hlt` leaves no window in
    // which a wakeup interrupt could be missed.
    unsafe { asm!("sti", "hlt", options(nomem, nostack)) };
}

pub fn rdtsc() -> u64 {
    // SAFETY: RDTSC has no side effects.
    unsafe { core::arch::x86_64::_rdtsc() }
}
