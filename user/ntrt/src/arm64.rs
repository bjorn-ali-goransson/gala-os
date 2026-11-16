//! The ARM64 runtime's own parts (the rest is `nt64.rs`, shared with
//! x64). System call stubs are shaped like Windows ARM64's ntdll: the
//! service number in the SVC's immediate (`svc #N`), the arguments in
//! x0-x7 and on the stack, the result in x0; the numbers are the pinned
//! build's, the same as x64's (`kernel/src/nt/syscalls.rs`). The TEB is
//! x18's (Microsoft's "Overview of ARM64 ABI conventions"), the PEB at
//! TEB+0x60 as on x64.

use super::nt64::*;
use core::arch::{asm, global_asm};

/// Defines system call stubs shaped like ntdll's: `svc #N; ret`.
macro_rules! stubs {
    ($($name:ident = $nr:expr;)*) => {
        core::arch::global_asm!(
            $(
                concat!(".globl ", stringify!($name)),
                ".p2align 2",
                concat!(stringify!($name), ":"),
                concat!("svc #", stringify!($nr)),
                "ret",
            )*
        );
    };
}

/// GalaHotswapProbe (gala's own service 0x7ff0; x64.rs says what each
/// op does). STATUS_INVALID_SYSTEM_SERVICE on Windows.
pub fn gala_hotswap_probe(op: u32, value: u64) -> (NtStatus, [u64; 4]) {
    let mut out = [0u64; 4];
    let status: u64;
    // SAFETY: the system call as ntdll's stubs make it (arguments in
    // x0-x2, the result in x0, the other volatile registers the
    // kernel's to change); `out` is ours, four u64 as the service writes.
    unsafe {
        asm!(
            "svc #0x7ff0",
            inlateout("x0") u64::from(op) => status,
            in("x1") value,
            in("x2") out.as_mut_ptr(),
            clobber_abi("C"),
        );
    }
    (status as NtStatus, out)
}

/// ARM64's CONTEXT (ARM64_NT_CONTEXT, winnt.h: 0x390 bytes, 16-byte
/// aligned); the fields after Pc (the NEON registers, FPCR/FPSR and the
/// debug registers) are kept as bytes.
#[repr(C, align(16))]
pub struct Context {
    pub context_flags: u32,
    pub cpsr: u32,
    /// X0..X28.
    pub x: [u64; 29],
    pub fp: u64,
    pub lr: u64,
    pub sp: u64,
    pub pc: u64,
    pub rest: [u8; 0x390 - 0x110],
}

/// CONTEXT_ARM64 | CONTROL | INTEGER | FLOATING_POINT.
pub const CONTEXT_FULL: u32 = 0x40_0007;

impl Context {
    /// A context that starts at `entry(arg)` on the stack ending at
    /// `stack_top`.
    pub fn for_thread(entry: extern "C" fn(usize) -> !, arg: usize, stack_top: usize) -> Self {
        let mut c = Context {
            context_flags: CONTEXT_FULL,
            cpsr: 0,
            x: [0; 29],
            fp: 0,
            lr: 0,
            sp: 0,
            pc: entry as usize as u64,
            rest: [0; 0x390 - 0x110],
        };
        c.x[0] = arg as u64;
        c.set_stack(stack_top);
        c
    }

    /// Its stack: sp at the 16-byte aligned top of a stack ending at
    /// `stack_top`.
    pub fn set_stack(&mut self, stack_top: usize) {
        self.sp = (stack_top & !0xf) as u64;
    }
}

/// The current thread's TEB (x18).
pub fn teb() -> *const u8 {
    let teb: *const u8;
    // SAFETY: on ARM64 Windows, x18 holds the current TEB.
    unsafe { asm!("mov {}, x18", out(reg) teb, options(nomem, nostack, preserves_flags)) };
    teb
}

/// Reads a pointer-sized TEB field.
pub fn teb_field(offset: usize) -> usize {
    // SAFETY: the TEB is mapped for the thread's life; `offset` is one of
    // its pointer-sized fields.
    unsafe { (teb().add(offset) as *const usize).read() }
}

/// PEB of the current process (TEB+0x60).
pub fn peb() -> *const u8 {
    teb_field(0x60) as *const u8
}

// Memory intrinsics the compiler may call (no CRT): byte loops in
// assembly, so they cannot be compiled back into calls to themselves.
global_asm!(
    ".globl memcpy",
    ".p2align 2",
    "memcpy:",
    "mov x3, x0",
    "cbz x2, 2f",
    "1:",
    "ldrb w4, [x1], #1",
    "strb w4, [x3], #1",
    "subs x2, x2, #1",
    "b.ne 1b",
    "2:",
    "ret",
    ".globl memmove",
    ".p2align 2",
    "memmove:",
    "cmp x0, x1",
    "b.ls memcpy",
    "add x3, x0, x2",
    "add x1, x1, x2",
    "cbz x2, 2f",
    "1:",
    "ldrb w4, [x1, #-1]!",
    "strb w4, [x3, #-1]!",
    "subs x2, x2, #1",
    "b.ne 1b",
    "2:",
    "ret",
    ".globl memset",
    ".p2align 2",
    "memset:",
    "mov x3, x0",
    "cbz x2, 2f",
    "1:",
    "strb w1, [x3], #1",
    "subs x2, x2, #1",
    "b.ne 1b",
    "2:",
    "ret",
);

// Stack probe for frames larger than a page (the ARM64 ABI: x15 holds the
// frame size / 16; each page below sp is touched in order, so a guard page
// grows the stack; only x16 and x17 change, and the caller then moves sp).
global_asm!(
    ".globl __chkstk",
    ".p2align 2",
    "__chkstk:",
    "lsl x16, x15, #4",
    "mov x17, sp",
    "1:",
    "cmp x16, #0x1000",
    "b.lo 2f",
    "sub x17, x17, #0x1000",
    "ldr xzr, [x17]",
    "sub x16, x16, #0x1000",
    "b 1b",
    "2:",
    "sub x17, x17, x16",
    "ldr xzr, [x17]",
    "ret",
);
