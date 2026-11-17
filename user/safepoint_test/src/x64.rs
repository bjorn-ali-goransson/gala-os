//! safepoint_test's x64 parts: the mutator's loop and the safepoint stub
//! (with their CONTEXT and save-area offsets), the "compiled code" and
//! its constants, NtReleaseSemaphore's stub.

use core::arch::global_asm;
use ntrt::{Handle, NtStatus};

use super::{GPRS, VALUES};

/// CONTEXT offsets (x64) of the registers `sp_mutate` loads, in its
/// order: rax, rbx, rcx, rdx, rsi, rdi, rbp, r8, r9, r10, r12, r13, r14,
/// r15; then Rsp, Rip and Xmm8.
pub const CTX_GPR: [usize; GPRS] =
    [0x78, 0x90, 0x80, 0x88, 0xa8, 0xb0, 0xa0, 0xb8, 0xc0, 0xc8, 0xd8, 0xe0, 0xe8, 0xf0];
pub const CTX_SP: usize = 0x98;
pub const CTX_PC: usize = 0xf8;
pub const CTX_VEC: usize = 0x1a0 + 8 * 16;

// sp_mutate(values: *const [u64; VALUES], out: *mut [u64; VALUES],
//           polls: u64): loads the values into registers and stack slots,
// runs `polls` safepoint polls (with a little spinning between them), and
// stores what the registers and slots hold then.
global_asm!(
    ".globl sp_mutate",
    "sp_mutate:",
    "push rbx",
    "push rbp",
    "push rsi",
    "push rdi",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "sub rsp, 72",
    "movdqu [rsp], xmm8",
    "movdqu [rsp + 16], xmm9",
    "movdqu [rsp + 32], xmm10",
    "movdqu [rsp + 48], xmm11",
    "mov [rsp + 64], rdx",
    "push r8",
    "sub rsp, 64",
    // Stack slots: values[14..22] at [rsp + 8 * i].
    "mov r11, rcx",
    "mov rax, [r11 + 112]",
    "mov [rsp], rax",
    "mov rax, [r11 + 120]",
    "mov [rsp + 8], rax",
    "mov rax, [r11 + 128]",
    "mov [rsp + 16], rax",
    "mov rax, [r11 + 136]",
    "mov [rsp + 24], rax",
    "mov rax, [r11 + 144]",
    "mov [rsp + 32], rax",
    "mov rax, [r11 + 152]",
    "mov [rsp + 40], rax",
    "mov rax, [r11 + 160]",
    "mov [rsp + 48], rax",
    "mov rax, [r11 + 168]",
    "mov [rsp + 56], rax",
    "movq xmm8, [r11 + 176]",
    "movq xmm9, [r11 + 184]",
    "movq xmm10, [r11 + 192]",
    "movq xmm11, [r11 + 200]",
    "mov rax, [r11]",
    "mov rbx, [r11 + 8]",
    "mov rcx, [r11 + 16]",
    "mov rdx, [r11 + 24]",
    "mov rsi, [r11 + 32]",
    "mov rdi, [r11 + 40]",
    "mov rbp, [r11 + 48]",
    "mov r8, [r11 + 56]",
    "mov r9, [r11 + 64]",
    "mov r10, [r11 + 72]",
    "mov r12, [r11 + 80]",
    "mov r13, [r11 + 88]",
    "mov r14, [r11 + 96]",
    "mov r15, [r11 + 104]",
    "2:",
    "mov r11, [rip + SP_POLL]",
    // The poll (41 85 03): faults while the poll points at the bad page.
    "test dword ptr [r11], eax",
    "mov r11d, 300",
    "3:",
    "dec r11d",
    "jnz 3b",
    "dec qword ptr [rsp + 64]",
    "jnz 2b",
    "mov r11, [rsp + 136]",
    "mov [r11], rax",
    "mov [r11 + 8], rbx",
    "mov [r11 + 16], rcx",
    "mov [r11 + 24], rdx",
    "mov [r11 + 32], rsi",
    "mov [r11 + 40], rdi",
    "mov [r11 + 48], rbp",
    "mov [r11 + 56], r8",
    "mov [r11 + 64], r9",
    "mov [r11 + 72], r10",
    "mov [r11 + 80], r12",
    "mov [r11 + 88], r13",
    "mov [r11 + 96], r14",
    "mov [r11 + 104], r15",
    "mov rax, [rsp]",
    "mov [r11 + 112], rax",
    "mov rax, [rsp + 8]",
    "mov [r11 + 120], rax",
    "mov rax, [rsp + 16]",
    "mov [r11 + 128], rax",
    "mov rax, [rsp + 24]",
    "mov [r11 + 136], rax",
    "mov rax, [rsp + 32]",
    "mov [r11 + 144], rax",
    "mov rax, [rsp + 40]",
    "mov [r11 + 152], rax",
    "mov rax, [rsp + 48]",
    "mov [r11 + 160], rax",
    "mov rax, [rsp + 56]",
    "mov [r11 + 168], rax",
    "movq [r11 + 176], xmm8",
    "movq [r11 + 184], xmm9",
    "movq [r11 + 192], xmm10",
    "movq [r11 + 200], xmm11",
    "add rsp, 72",
    "movdqu xmm8, [rsp]",
    "movdqu xmm9, [rsp + 16]",
    "movdqu xmm10, [rsp + 32]",
    "movdqu xmm11, [rsp + 48]",
    "add rsp, 72",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rdi",
    "pop rsi",
    "pop rbp",
    "pop rbx",
    "ret",
    // The safepoint stub (as HotSpot's safepoint blob), entered through
    // NtContinue: makes room for the resume address (sp_block fills it
    // in), saves every register on the stack, blocks in sp_block(save
    // area), restores them and returns to the instruction after the poll.
    ".globl sp_stub",
    "sp_stub:",
    "sub rsp, 8",
    "push rax",
    "push rbx",
    "push rcx",
    "push rdx",
    "push rsi",
    "push rdi",
    "push rbp",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "sub rsp, 64",
    "movdqu [rsp], xmm8",
    "movdqu [rsp + 16], xmm9",
    "movdqu [rsp + 32], xmm10",
    "movdqu [rsp + 48], xmm11",
    "mov rcx, rsp",
    "sub rsp, 40",
    "call {block}",
    "add rsp, 40",
    "movdqu xmm8, [rsp]",
    "movdqu xmm9, [rsp + 16]",
    "movdqu xmm10, [rsp + 32]",
    "movdqu xmm11, [rsp + 48]",
    "add rsp, 64",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rbp",
    "pop rdi",
    "pop rsi",
    "pop rdx",
    "pop rcx",
    "pop rbx",
    "pop rax",
    "ret",
    // NtReleaseSemaphore: its x64 number (0x0a) is long-stable
    // (NtCreateSemaphore's is not: ntrt's create_semaphore).
    ".globl sp_release_semaphore",
    "sp_release_semaphore:",
    "mov r10, rcx",
    "mov eax, 0x0a",
    "syscall",
    "ret",
    block = sym super::sp_block,
);

unsafe extern "C" {
    pub fn sp_mutate(values: *const [u64; VALUES], out: *mut [u64; VALUES], polls: u64);
    pub fn sp_release_semaphore(handle: Handle, count: i32, previous: *mut i32) -> NtStatus;
    pub fn sp_stub();
}

/// Offsets in sp_stub's save area of the registers `sp_mutate` loads (in
/// its order: rax, rbx, rcx, rdx, rsi, rdi, rbp, r8, r9, r10, r12, r13,
/// r14, r15), of xmm8 and of the return address (the stack slots follow).
pub const STUB_GPR: [usize; GPRS] = [
    64 + 8 * 14,
    64 + 8 * 13,
    64 + 8 * 12,
    64 + 8 * 11,
    64 + 8 * 10,
    64 + 8 * 9,
    64 + 8 * 8,
    64 + 8 * 7,
    64 + 8 * 6,
    64 + 8 * 5,
    64 + 8 * 3,
    64 + 8 * 2,
    64 + 8,
    64,
];
pub const STUB_VEC: usize = 0;
pub const STUB_RETURN: usize = 64 + 8 * 15;
/// The mutator's stack pointer at the poll, from the save area.
pub const STUB_SP: usize = STUB_RETURN + 8;

/// The poll's bytes (`test dword ptr [r11], eax`: 41 85 03).
pub const POLL: &[u8] = &[0x41, 0x85, 0x03];

/// Writes mutator compiled code returning `value` at `at`: `movabs rax,
/// imm64; ret`.
///
/// # Safety
/// `at` is 11 writable bytes of the code page.
pub unsafe fn write_code(at: usize, value: u64) {
    // SAFETY: the caller's.
    unsafe {
        (at as *mut [u8; 2]).write([0x48, 0xb8]);
        ((at + 2) as *mut u64).write_unaligned(value);
        ((at + 10) as *mut u8).write(0xc3);
    }
}

/// The constant the compiled code at `at` returns.
///
/// # Safety
/// `at` holds `write_code`'s code.
pub unsafe fn read_code(at: usize) -> u64 {
    // SAFETY: the caller's.
    unsafe { ((at + 2) as *const u64).read_unaligned() }
}

/// The loopers' code: `loop: mov r11, [r9]; test [r11], eax (the poll);
/// mov r8d, imm32; mov [rcx], r8d; dec rdx; jnz loop; ret`, called as
/// f(out: rcx, iterations: rdx, poll pointer: r9). After each stop at
/// the poll it stores the constant again, so `*out` must be the newest.
pub const LOOP_CODE: &[u8] = &[
    0x4d, 0x8b, 0x19, 0x41, 0x85, 0x03, 0x41, 0xb8, 0, 0, 0, 0, 0x44, 0x89, 0x01, 0x48, 0xff, 0xca,
    0x75, 0xec, 0xc3,
];
/// Offset of LOOP_CODE in the code page (not aligned, as x64 code need
/// not be).
pub const LOOP_CODE_AT: usize = 0x101;
const LOOP_IMM: usize = 8;

/// The 32-bit constant LOOP_CODE at `at` stores.
///
/// # Safety
/// `at` holds LOOP_CODE.
pub unsafe fn read_loop(at: usize) -> u32 {
    // SAFETY: the caller's.
    unsafe { ((at + LOOP_IMM) as *const u32).read_unaligned() }
}

/// Makes LOOP_CODE at `at` store `value`.
///
/// # Safety
/// `at` holds LOOP_CODE; no thread runs it.
pub unsafe fn write_loop(at: usize, value: u32) {
    // SAFETY: the caller's.
    unsafe { ((at + LOOP_IMM) as *mut u32).write_unaligned(value) }
}

/// x64 keeps instruction fetch coherent with stores: nothing to do.
pub fn sync_code(_base: usize, _size: usize) {}
