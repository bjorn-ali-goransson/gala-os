//! The x64 runtime's own parts (the rest is `nt64.rs`, shared with
//! ARM64): system call stubs shaped like ntdll's, the TEB through gs, x64's
//! CONTEXT and the memory intrinsics.

use super::nt64::*;
use core::arch::{asm, global_asm};

/// Defines a system call stub shaped exactly like ntdll's:
/// `mov r10, rcx; mov eax, N; syscall; ret`.
macro_rules! stubs {
    ($($name:ident = $nr:expr;)*) => {
        core::arch::global_asm!(
            $(
                concat!(".globl ", stringify!($name)),
                concat!(stringify!($name), ":"),
                "mov r10, rcx",
                concat!("mov eax, ", stringify!($nr)),
                "syscall",
                "ret",
            )*
        );
    };
}

/// GalaHotswapProbe (gala's own service, number 0x7ff0; the kernel's
/// nt::hotswap_probe, ROADMAP SN step 1e): op 0 sets the probe's static,
/// 1 makes its object, 2 reads `[version, calls, object, global]`, 3
/// waits two seconds in the kernel. STATUS_INVALID_SYSTEM_SERVICE on
/// Windows.
/// A function of its own, not a `stubs!` entry: the linker drops it from
/// the programs that do not call it.
pub fn gala_hotswap_probe(op: u32, value: u64) -> (NtStatus, [u64; 4]) {
    let mut out = [0u64; 4];
    let status: u64;
    // SAFETY: the system call as ntdll's stubs make it (arguments in r10,
    // rdx, r8; rcx and r11 taken by `syscall`, the other volatile
    // registers the kernel's to change); `out` is ours, four u64 as the
    // service writes.
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") 0x7ff0u64 => status,
            in("r10") u64::from(op),
            in("rdx") value,
            in("r8") out.as_mut_ptr(),
            clobber_abi("win64"),
        );
    }
    (status as NtStatus, out)
}

/// x64 CONTEXT (1232 bytes, 16-byte aligned); only the fields used here are
/// named, the rest is padding with the public layout's offsets.
#[repr(C, align(16))]
pub struct Context {
    pub home: [u64; 6],
    pub context_flags: u32,
    pub mx_csr: u32,
    pub seg_cs: u16,
    pub seg_ds: u16,
    pub seg_es: u16,
    pub seg_fs: u16,
    pub seg_gs: u16,
    pub seg_ss: u16,
    pub eflags: u32,
    pub dr: [u64; 6],
    /// Rax, Rcx, Rdx, Rbx, Rsp, Rbp, Rsi, Rdi, R8..R15.
    pub gpr: [u64; 16],
    pub rip: u64,
    pub rest: [u8; 0x4d0 - 0x100],
}

pub const CONTEXT_FULL: u32 = 0x10_000b;

impl Context {
    /// A context that starts at `entry(arg)` on the stack ending at `stack_top`.
    pub fn for_thread(entry: extern "C" fn(usize) -> !, arg: usize, stack_top: usize) -> Self {
        let mut c = Context {
            home: [0; 6],
            context_flags: CONTEXT_FULL,
            mx_csr: 0x1f80,
            seg_cs: 0x33,
            seg_ds: 0x2b,
            seg_es: 0x2b,
            seg_fs: 0x53,
            seg_gs: 0x2b,
            seg_ss: 0x2b,
            eflags: 0x200,
            dr: [0; 6],
            gpr: [0; 16],
            rip: entry as usize as u64,
            rest: [0; 0x4d0 - 0x100],
        };
        c.gpr[1] = arg as u64; // rcx
        c.gpr[4] = (stack_top - 0x28) as u64; // rsp: as if called
        c
    }

    /// Its stack: rsp as if called at the top of a stack ending at
    /// `stack_top`.
    pub fn set_stack(&mut self, stack_top: usize) {
        self.gpr[4] = (stack_top - 0x28) as u64;
    }
}

/// Reads a pointer-sized TEB field (`gs:[offset]`).
pub fn teb_field(offset: usize) -> usize {
    let v: usize;
    // SAFETY: gs points to the TEB in user mode on x64 Windows.
    unsafe {
        asm!("mov {}, gs:[{}]", out(reg) v, in(reg) offset, options(nostack, readonly, preserves_flags))
    };
    v
}

/// PEB of the current process (`gs:[0x60]` on x64).
pub fn peb() -> *const u8 {
    let peb: *const u8;
    // SAFETY: on x64 Windows, gs:[0x60] holds the current PEB pointer.
    unsafe { asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly, preserves_flags)) };
    peb
}

// Memory intrinsics that the compiler may call. Without a CRT we provide
// them; copy/fill use `rep` string instructions so they cannot be compiled
// back into calls to themselves.
global_asm!(
    ".globl memcpy",
    "memcpy:",
    "mov rax, rcx",
    "mov r9, rdi",
    "mov r10, rsi",
    "mov rdi, rcx",
    "mov rsi, rdx",
    "mov rcx, r8",
    "rep movsb",
    "mov rdi, r9",
    "mov rsi, r10",
    "ret",
    ".globl memmove",
    "memmove:",
    "mov rax, rcx",
    "mov r9, rdi",
    "mov r10, rsi",
    "mov rdi, rcx",
    "mov rsi, rdx",
    "mov rcx, r8",
    "cmp rdi, rsi",
    "jbe 2f",
    // Overlapping with dst above src: copy backwards.
    "lea rsi, [rsi + rcx - 1]",
    "lea rdi, [rdi + rcx - 1]",
    "std",
    "rep movsb",
    "cld",
    "jmp 3f",
    "2:",
    "rep movsb",
    "3:",
    "mov rdi, r9",
    "mov rsi, r10",
    "ret",
    ".globl memset",
    "memset:",
    "mov r9, rdi",
    "mov r10, rcx",
    "mov rdi, rcx",
    "mov eax, edx",
    "mov rcx, r8",
    "rep stosb",
    "mov rdi, r9",
    "mov rax, r10",
    "ret",
);

// Stack probe for frames larger than a page. Contract (MSVC x64): RAX
// holds the frame size; every page from the caller's RSP down to RSP - RAX
// is touched in order, so a guard page below the stack grows it one page at
// a time; RSP and all other registers are left unchanged.
global_asm!(
    ".globl __chkstk",
    "__chkstk:",
    "push rcx",
    "push rax",
    // The caller's RSP: above our two pushes and the return address.
    "lea rcx, [rsp + 0x18]",
    "2:",
    "cmp rax, 0x1000",
    "jb 3f",
    "sub rcx, 0x1000",
    "test byte ptr [rcx], 0",
    "sub rax, 0x1000",
    "jmp 2b",
    "3:",
    "sub rcx, rax",
    "test byte ptr [rcx], 0",
    "pop rax",
    "pop rcx",
    "ret",
);
