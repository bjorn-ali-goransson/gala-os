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

/// CPUID's vendor string ("GenuineIntel", "AuthenticAMD", ...): where the
/// vendors' architectures differ (SYSRET's SS), the boot log says which.
pub fn vendor() -> [u8; 12] {
    let r = cpuid(0, 0);
    let mut v = [0u8; 12];
    v[..4].copy_from_slice(&r.ebx.to_le_bytes());
    v[4..8].copy_from_slice(&r.edx.to_le_bytes());
    v[8..].copy_from_slice(&r.ecx.to_le_bytes());
    v
}

/// One random word from the CPU's generator: RDRAND (10 tries, as
/// Intel recommends), else RDSEED; None without either.
pub fn rdrand() -> Option<u64> {
    if cpuid(1, 0).ecx & (1 << 30) != 0 {
        for _ in 0..10 {
            let (value, ok): (u64, u8);
            // SAFETY: RDRAND exists (CPUID.1:ECX[30]); it only writes the
            // register and the carry flag.
            unsafe {
                asm!("rdrand {}", "setc {}", out(reg) value, out(reg_byte) ok, options(nomem, nostack))
            };
            if ok != 0 {
                return Some(value);
            }
        }
    }
    if cpuid(7, 0).ebx & (1 << 18) != 0 {
        for _ in 0..1000 {
            let (value, ok): (u64, u8);
            // SAFETY: RDSEED exists (CPUID.7.0:EBX[18]).
            unsafe {
                asm!("rdseed {}", "setc {}", out(reg) value, out(reg_byte) ok, options(nomem, nostack))
            };
            if ok != 0 {
                return Some(value);
            }
            core::hint::spin_loop();
        }
    }
    None
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

/// Called by `IrqMutex` while it spins for a held lock.
pub fn lock_contended() {
    super::smp::serve_pending_flush();
}

pub fn rdtsc() -> u64 {
    // SAFETY: RDTSC has no side effects.
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// Enables the x87/SSE/AVX state for code that uses it (user mode; the
/// kernel is soft-float): CR0.EM off, CR0.MP on, CR4.OSFXSR and
/// CR4.OSXMMEXCPT on, then XSAVE (CR4.OSXSAVE, XCR0: `xstate`). Every
/// CPU runs this in its own setup path (after a kernel handover too).
pub fn enable_sse() {
    // SAFETY: these control bits only affect how SSE/x87 instructions
    // behave; the kernel itself executes none outside the state saves and
    // loads (`xstate`).
    unsafe {
        let mut cr0: u64;
        asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        cr0 = (cr0 & !(1 << 2)) | (1 << 1);
        asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
        let cr4 = read_cr4() | (1 << 9) | (1 << 10);
        asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
        asm!("fninit", options(nomem, nostack));
    }
    super::xstate::enable_here();
}

/// The free-running cycle counter the clock, the flight recorder and spin
/// deadlines read (the TSC; `crate::time` calibrates its rate).
#[inline]
pub fn cycles() -> u64 {
    rdtsc()
}

/// One random word from the processor's generator, None without one
/// (RDRAND, else RDSEED).
pub fn hw_random() -> Option<u64> {
    rdrand()
}
