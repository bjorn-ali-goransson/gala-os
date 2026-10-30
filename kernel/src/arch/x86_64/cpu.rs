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

/// Enables SSE for code that uses it (user mode; the kernel is soft-float):
/// CR0.EM off, CR0.MP on, CR4.OSFXSR and CR4.OSXMMEXCPT on.
pub fn enable_sse() {
    // SAFETY: these control bits only affect how SSE/x87 instructions
    // behave; the kernel itself executes none outside FXSAVE/FXRSTOR.
    unsafe {
        let mut cr0: u64;
        asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        cr0 = (cr0 & !(1 << 2)) | (1 << 1);
        asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
        let cr4 = read_cr4() | (1 << 9) | (1 << 10);
        asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
        asm!("fninit", options(nomem, nostack));
    }
}

/// An FXSAVE image of the x87/SSE state (the layout of CONTEXT.FltSave).
#[repr(C, align(16))]
#[derive(Clone)]
pub struct FxState(pub [u8; 512]);

/// MXCSR bits the CPU supports (FXSAVE's MXCSR_MASK; 0 means the default).
fn mxcsr_mask(state: &FxState) -> u32 {
    match u32::from_le_bytes([state.0[28], state.0[29], state.0[30], state.0[31]]) {
        0 => 0xffbf,
        m => m,
    }
}

/// The current x87/SSE register state. The kernel itself never uses
/// these registers, so in a trap or system call they hold the user's.
pub fn fxsave() -> FxState {
    let mut state = FxState([0; 512]);
    // SAFETY: FXSAVE writes 512 bytes to a 16-byte aligned buffer.
    unsafe { asm!("fxsave64 [{}]", in(reg) state.0.as_mut_ptr(), options(nostack)) };
    state
}

/// Loads `state` (as user code may supply it: reserved MXCSR bits are
/// cleared, since FXRSTOR would fault on them).
pub fn fxrstor(state: &FxState) {
    let mask = mxcsr_mask(&fxsave());
    let mut s = state.clone();
    let mxcsr = u32::from_le_bytes([s.0[24], s.0[25], s.0[26], s.0[27]]) & mask;
    s.0[24..28].copy_from_slice(&mxcsr.to_le_bytes());
    // SAFETY: FXRSTOR reads 512 bytes from a 16-byte aligned buffer whose
    // MXCSR has only supported bits set.
    unsafe { asm!("fxrstor64 [{}]", in(reg) s.0.as_ptr(), options(nostack, readonly)) };
}

/// Sets MXCSR (reserved bits cleared).
pub fn set_mxcsr(value: u32) {
    let mut state = fxsave();
    state.0[24..28].copy_from_slice(&value.to_le_bytes());
    fxrstor(&state);
}
