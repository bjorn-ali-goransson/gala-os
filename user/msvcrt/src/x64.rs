//! The x64 parts of msvcrt (System32's): the C-variadic entry points,
//! setjmp/longjmp and the floating-point control word (MXCSR).
//!
//! The variadic functions store the register arguments in the caller's
//! home area, which with the stack arguments after it is the Win64
//! `va_list` (8-byte slots, `va.rs`), and call the `v` variant. They and
//! `_setjmp`/`longjmp` are `global_asm!` symbols, exported by name in
//! build.rs.

use crate::scan::vsscanf;
use crate::stdio::{_vsnprintf, vfprintf, vfwprintf, vprintf, vsprintf};
use crate::wide::{_execv, _execvp, _gala_vfscanf, _gala_vswprintf};

// fprintf(file, fmt, ...): rcx = file, rdx = fmt, r8/r9 and the stack =
// the variadic arguments. Spill rdx, r8, r9 to the caller's home area, so
// [rsp+24] (r8's slot) starts the va_list, then vfprintf(file, fmt, list).
// sprintf(buf, fmt, ...) is the same shape (vsprintf). printf(fmt, ...):
// the list starts at [rsp+16].
core::arch::global_asm!(
    ".globl fprintf",
    "fprintf:",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea r8, [rsp+24]",
    "sub rsp, 40",
    "call {vfprintf}",
    "add rsp, 40",
    "ret",
    ".globl sprintf",
    "sprintf:",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea r8, [rsp+24]",
    "sub rsp, 40",
    "call {vsprintf}",
    "add rsp, 40",
    "ret",
    // _snprintf(buf, count, fmt, ...): the list starts at r9's slot.
    ".globl _snprintf",
    "_snprintf:",
    "mov [rsp+32], r9",
    "lea r9, [rsp+32]",
    "sub rsp, 40",
    "call {_vsnprintf}",
    "add rsp, 40",
    "ret",
    // fwprintf(file, fmt, ...): fprintf's shape.
    ".globl fwprintf",
    "fwprintf:",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea r8, [rsp+24]",
    "sub rsp, 40",
    "call {vfwprintf}",
    "add rsp, 40",
    "ret",
    ".globl printf",
    "printf:",
    "mov [rsp+8], rcx",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea rdx, [rsp+16]",
    "sub rsp, 40",
    "call {vprintf}",
    "add rsp, 40",
    "ret",
    vfprintf = sym vfprintf,
    vfwprintf = sym vfwprintf,
    vsprintf = sym vsprintf,
    vprintf = sym vprintf,
    _vsnprintf = sym _vsnprintf,
);

// sscanf(input, fmt, ...): sprintf's shape (vsscanf).
core::arch::global_asm!(
    ".globl sscanf",
    "sscanf:",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea r8, [rsp+24]",
    "sub rsp, 40",
    "call {vsscanf}",
    "add rsp, 40",
    "ret",
    vsscanf = sym vsscanf,
);

// swprintf(buf, fmt, ...) and fscanf(file, fmt, ...): sprintf's shape
// (above). _execl(path, arg0, ...) and _execlp: the arguments after the
// path, spilled to the home area next to the stack ones, are the NULL-
// terminated argv.
core::arch::global_asm!(
    ".globl swprintf",
    "swprintf:",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea r8, [rsp+24]",
    "sub rsp, 40",
    "call {vswprintf}",
    "add rsp, 40",
    "ret",
    ".globl fscanf",
    "fscanf:",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea r8, [rsp+24]",
    "sub rsp, 40",
    "call {vfscanf}",
    "add rsp, 40",
    "ret",
    ".globl _execl",
    "_execl:",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea rdx, [rsp+16]",
    "sub rsp, 40",
    "call {execv}",
    "add rsp, 40",
    "ret",
    ".globl _execlp",
    "_execlp:",
    "mov [rsp+16], rdx",
    "mov [rsp+24], r8",
    "mov [rsp+32], r9",
    "lea rdx, [rsp+16]",
    "sub rsp, 40",
    "call {execvp}",
    "add rsp, 40",
    "ret",
    vswprintf = sym _gala_vswprintf,
    vfscanf = sym _gala_vfscanf,
    execv = sym _execv,
    execvp = sym _execvp,
);

// _setjmp/longjmp with msvcrt's x64 _JUMP_BUFFER layout (256 bytes):
// Frame, Rbx, Rsp, Rbp, Rsi, Rdi, R12-R15, Rip, MxCsr, FpCsr, Spare,
// Xmm6-Xmm15. mingw's setjmp passes the frame as the second argument;
// msvcrt then unwinds the frames in between with SEH on longjmp. C code
// has nothing to unwind, so this longjmp restores the registers and jumps
// (as msvcrt's with a null frame).

core::arch::global_asm!(
    ".globl _setjmp",
    "_setjmp:",
    "mov [rcx], rdx",
    "mov [rcx+8], rbx",
    "lea rax, [rsp+8]",
    "mov [rcx+16], rax",
    "mov [rcx+24], rbp",
    "mov [rcx+32], rsi",
    "mov [rcx+40], rdi",
    "mov [rcx+48], r12",
    "mov [rcx+56], r13",
    "mov [rcx+64], r14",
    "mov [rcx+72], r15",
    "mov rax, [rsp]",
    "mov [rcx+80], rax",
    "stmxcsr [rcx+88]",
    "fnstcw [rcx+92]",
    "movdqu [rcx+96], xmm6",
    "movdqu [rcx+112], xmm7",
    "movdqu [rcx+128], xmm8",
    "movdqu [rcx+144], xmm9",
    "movdqu [rcx+160], xmm10",
    "movdqu [rcx+176], xmm11",
    "movdqu [rcx+192], xmm12",
    "movdqu [rcx+208], xmm13",
    "movdqu [rcx+224], xmm14",
    "movdqu [rcx+240], xmm15",
    "xor eax, eax",
    "ret",
    ".globl longjmp",
    "longjmp:",
    "mov eax, edx",
    "test eax, eax",
    "jnz 2f",
    "mov eax, 1",
    "2:",
    "mov rbx, [rcx+8]",
    "mov rbp, [rcx+24]",
    "mov rsi, [rcx+32]",
    "mov rdi, [rcx+40]",
    "mov r12, [rcx+48]",
    "mov r13, [rcx+56]",
    "mov r14, [rcx+64]",
    "mov r15, [rcx+72]",
    "ldmxcsr [rcx+88]",
    "fldcw [rcx+92]",
    "movdqu xmm6, [rcx+96]",
    "movdqu xmm7, [rcx+112]",
    "movdqu xmm8, [rcx+128]",
    "movdqu xmm9, [rcx+144]",
    "movdqu xmm10, [rcx+160]",
    "movdqu xmm11, [rcx+176]",
    "movdqu xmm12, [rcx+192]",
    "movdqu xmm13, [rcx+208]",
    "movdqu xmm14, [rcx+224]",
    "movdqu xmm15, [rcx+240]",
    "mov rsp, [rcx+16]",
    "jmp qword ptr [rcx+80]",
);

/// _controlfp(new, mask): the floating-point control word in msvcrt's
/// abstract form, its `mask` bits set from `new`; the new word. On x64 it
/// is MXCSR: exception masks (_MCW_EM), rounding (_MCW_RC) and
/// denormal flushing (_MCW_DN); precision (_MCW_PC) does not apply.
#[unsafe(no_mangle)]
pub extern "C" fn _controlfp(new: u32, mask: u32) -> u32 {
    // (abstract bit, MXCSR bit): the exception masks, then FTZ.
    const BITS: [(u32, u32); 7] = [
        (0x10, 0x80),          // _EM_INVALID
        (0x8_0000, 0x100),     // _EM_DENORMAL
        (0x8, 0x200),          // _EM_ZERODIVIDE
        (0x4, 0x400),          // _EM_OVERFLOW
        (0x2, 0x800),          // _EM_UNDERFLOW
        (0x1, 0x1000),         // _EM_INEXACT
        (0x0100_0000, 0x8000), // _DN_FLUSH
    ];
    // _RC_NEAR 0, _RC_DOWN 0x100, _RC_UP 0x200, _RC_CHOP 0x300; MXCSR's
    // RC field (bits 13-14) is 0 nearest, 1 down, 2 up, 3 toward zero.
    const MCW_RC: u32 = 0x300;
    let mut mxcsr = 0u32;
    // SAFETY: reads MXCSR into a local.
    unsafe { core::arch::asm!("stmxcsr [{}]", in(reg) &raw mut mxcsr, options(nostack)) };
    let mut word = BITS.iter().filter(|(_, m)| mxcsr & m != 0).fold(0, |w, (a, _)| w | a);
    word |= ((mxcsr >> 13) & 3) << 8;
    let mask = mask & !0x3_0000; // _MCW_PC
    word = (word & !mask) | (new & mask);
    let mut out = mxcsr & !(0x1f80 | 0x8000 | 0x6000);
    for (a, m) in BITS {
        if word & a != 0 {
            out |= m;
        }
    }
    out |= ((word & MCW_RC) >> 8) << 13;
    // SAFETY: loads the updated MXCSR (only control bits changed).
    unsafe { core::arch::asm!("ldmxcsr [{}]", in(reg) &raw const out, options(nostack)) };
    word
}
