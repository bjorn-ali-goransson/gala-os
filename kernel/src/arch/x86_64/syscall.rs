//! SYSCALL/SYSRET entry and return-to-user.

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicUsize, Ordering};

use super::cpu;
use super::gdt::{KERNEL_CS, USER_CS, USER_SS, USER32_CS};
use super::percpu::{self, KERNEL_RSP_OFFSET, USER_RSP_OFFSET};
use crate::mm::paging::USER_END;

const IA32_EFER: u32 = 0xc000_0080;
const EFER_SCE: u64 = 1 << 0;
const IA32_STAR: u32 = 0xc000_0081;
const IA32_LSTAR: u32 = 0xc000_0082;
const IA32_FMASK: u32 = 0xc000_0084;
/// RFLAGS bits cleared on SYSCALL: TF, IF, DF, AC.
const FMASK: u64 = (1 << 8) | (1 << 9) | (1 << 10) | (1 << 18);
const USER_RFLAGS: u64 = 0x202; // IF | reserved bit 1

/// User registers at SYSCALL time, in stack order. The Windows x64 syscall
/// convention passes the number in rax and arguments in r10, rdx, r8, r9,
/// then on the user stack above a 0x28-byte area (return address + shadow).
#[repr(C)]
#[derive(Debug)]
pub struct SyscallFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdx: u64,
    pub r10: u64,
    /// Syscall number on entry, return value (NTSTATUS) on exit.
    pub rax: u64,
    pub rip: u64,
    pub rflags: u64,
    pub rsp: u64,
}

const _: () = assert!(core::mem::size_of::<SyscallFrame>() == 16 * 8);

global_asm!(
    r#"
    .section .text.syscall, "ax"
    .global gala_syscall_entry
gala_syscall_entry:
    swapgs
    mov gs:[{user_rsp}], rsp
    mov rsp, gs:[{kernel_rsp}]
    push qword ptr gs:[{user_rsp}]
    push r11
    push rcx
    push rax
    push r10
    push rdx
    push r8
    push r9
    push rdi
    push rsi
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15
    mov rdi, rsp
    sti
    cld
    call {dispatch}
    cli
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    pop rsi
    pop rdi
    pop r9
    pop r8
    pop rdx
    pop r10
    pop rax
    pop rcx
    pop r11
    swapgs
    pop rsp
    sysretq
    "#,
    user_rsp = const USER_RSP_OFFSET,
    kernel_rsp = const KERNEL_RSP_OFFSET,
    dispatch = sym dispatch,
);

unsafe extern "C" {
    fn gala_syscall_entry();
}

pub type Handler = fn(&mut SyscallFrame);

static HANDLER: AtomicUsize = AtomicUsize::new(0);

/// Sets the function system calls are delivered to (on every CPU).
pub fn set_handler(handler: Handler) {
    HANDLER.store(handler as usize, Ordering::Release);
}

/// Enables SYSCALL on this CPU. Each CPU calls it during its own setup
/// (MSRs are per CPU), before any thread can move between CPUs.
pub fn init_cpu() {
    let star = u64::from(USER32_CS & !3) << 48 | u64::from(KERNEL_CS) << 32;
    // SAFETY: architected MSRs. The entry point switches to the kernel
    // stack set by `percpu::set_kernel_stack` before touching memory, and
    // FMASK keeps interrupts off until it has.
    unsafe {
        cpu::wrmsr(IA32_STAR, star);
        cpu::wrmsr(IA32_LSTAR, gala_syscall_entry as *const () as u64);
        cpu::wrmsr(IA32_FMASK, FMASK);
        cpu::wrmsr(IA32_EFER, cpu::rdmsr(IA32_EFER) | EFER_SCE);
    }
}

extern "C" fn dispatch(frame: &mut SyscallFrame) {
    let raw = HANDLER.load(Ordering::Acquire);
    // SAFETY: only `set_handler` stores into HANDLER, and it stores a `Handler`.
    let handler: Handler = unsafe { core::mem::transmute::<usize, Handler>(raw) };
    handler(frame);
    // NtContinue: return with every register, through IRETQ. Only this
    // thread's request counts (another may have been preempted after
    // making one).
    let pending = crate::sched::take_pending_continue();
    // The return hook sees (and may change) the registers user mode gets.
    let mut regs = pending.unwrap_or_else(|| syscall_regs(frame));
    let changed = run_user_return_hook(&mut regs);
    if pending.is_some() || changed {
        // Nothing on this stack needs dropping: the handler has returned.
        enter_user_regs(&regs);
    }
    // SYSRET to a non-canonical rip would fault in ring 0; never allow it.
    if frame.rip >= USER_END - 0x1000 {
        panic!("syscall return to invalid rip {:#x}", frame.rip);
    }
}

/// Starts executing user code at `entry` with stack `stack` and `rcx = arg`,
/// using the GS base set by `percpu::set_user_gs_base`. Never returns; the
/// thread re-enters the kernel only through syscalls and interrupts.
pub fn enter_user(entry: u64, stack: u64, arg: u64) -> ! {
    assert!(entry < USER_END && stack < USER_END, "user entry outside user space");
    assert!(percpu::kernel_stack() != 0, "no kernel stack for user entry");
    // SAFETY: switches to ring 3 at a user address with user selectors;
    // user code can only reach memory mapped user-accessible, and returns to
    // the kernel via the IDT/SYSCALL paths with the kernel stack checked above.
    unsafe {
        asm!(
            "cli",
            "swapgs",
            "push {ss}",
            "push rsi",
            "push {rflags}",
            "push {cs}",
            "push rdi",
            "xor eax, eax",
            "xor ebx, ebx",
            "xor edx, edx",
            "xor esi, esi",
            "xor edi, edi",
            "xor ebp, ebp",
            "xor r8d, r8d",
            "xor r9d, r9d",
            "xor r10d, r10d",
            "xor r11d, r11d",
            "xor r12d, r12d",
            "xor r13d, r13d",
            "xor r14d, r14d",
            "xor r15d, r15d",
            "iretq",
            ss = const USER_SS as u64,
            cs = const USER_CS as u64,
            rflags = const USER_RFLAGS,
            in("rdi") entry,
            in("rsi") stack,
            in("rcx") arg,
            options(noreturn),
        )
    }
}

/// User register state for entering a new thread (CONTEXT order:
/// rax, rcx, rdx, rbx, rsp, rbp, rsi, rdi, r8..r15).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UserRegs {
    pub gpr: [u64; 16],
    pub rip: u64,
    pub rflags: u64,
}

/// RFLAGS bits user mode may choose: CF, PF, AF, ZF, SF, DF, OF.
const USER_SETTABLE_FLAGS: u64 = 0xcd5;
const RSP_INDEX: usize = 4;

impl UserRegs {
    /// Whether rip and rsp are user addresses (iretq to a non-canonical
    /// rip would fault in the kernel).
    pub fn is_valid(&self) -> bool {
        self.rip < USER_END && self.gpr[RSP_INDEX] < USER_END
    }
}

/// Enters user mode with every general-purpose register from `regs`, using
/// the GS base set by `percpu::set_user_gs_base`. Never returns.
pub fn enter_user_regs(regs: &UserRegs) -> ! {
    assert!(regs.is_valid(), "user entry outside user space");
    assert!(percpu::kernel_stack() != 0, "no kernel stack for user entry");
    let mut r = *regs;
    r.rflags = (r.rflags & USER_SETTABLE_FLAGS) | USER_RFLAGS;
    // SAFETY: as in `enter_user`; `r` lives on this stack, which stays
    // mapped until the `iretq` (the last instruction reading it).
    unsafe {
        asm!(
            "cli",
            "swapgs",
            "push {ss}",
            "push qword ptr [rdi + 32]", // rsp
            "push qword ptr [rdi + 136]", // rflags
            "push {cs}",
            "push qword ptr [rdi + 128]", // rip
            "mov rax, [rdi + 0]",
            "mov rcx, [rdi + 8]",
            "mov rdx, [rdi + 16]",
            "mov rbx, [rdi + 24]",
            "mov rbp, [rdi + 40]",
            "mov rsi, [rdi + 48]",
            "mov r8, [rdi + 64]",
            "mov r9, [rdi + 72]",
            "mov r10, [rdi + 80]",
            "mov r11, [rdi + 88]",
            "mov r12, [rdi + 96]",
            "mov r13, [rdi + 104]",
            "mov r14, [rdi + 112]",
            "mov r15, [rdi + 120]",
            "mov rdi, [rdi + 56]",
            "iretq",
            ss = const USER_SS as u64,
            cs = const USER_CS as u64,
            in("rdi") &r,
            options(noreturn),
        )
    }
}

/// Makes the current system call return to user mode with `regs` (all
/// general-purpose registers, unlike SYSRET) instead of its frame.
pub fn continue_with(regs: UserRegs) {
    crate::sched::set_pending_continue(regs);
}

/// The user registers a SYSCALL frame returns with (SYSRET leaves the
/// return address in rcx and the flags in r11).
fn syscall_regs(f: &SyscallFrame) -> UserRegs {
    UserRegs {
        gpr: [
            f.rax, f.rip, f.rdx, f.rbx, f.rsp, f.rbp, f.rsi, f.rdi, f.r8, f.r9, f.r10, f.rflags,
            f.r12, f.r13, f.r14, f.r15,
        ],
        rip: f.rip,
        rflags: f.rflags,
    }
}

/// The user registers in an interrupt frame from user mode.
pub(super) fn trap_regs(f: &super::interrupts::TrapFrame) -> UserRegs {
    UserRegs {
        gpr: [
            f.rax, f.rcx, f.rdx, f.rbx, f.rsp, f.rbp, f.rsi, f.rdi, f.r8, f.r9, f.r10, f.r11,
            f.r12, f.r13, f.r14, f.r15,
        ],
        rip: f.rip,
        rflags: f.rflags,
    }
}

/// Writes `r` back into an interrupt frame from user mode (IRETQ then
/// returns with it; only user-settable flags are taken).
pub(super) fn set_trap_regs(f: &mut super::interrupts::TrapFrame, r: &UserRegs) {
    let g = &r.gpr;
    (f.rax, f.rcx, f.rdx, f.rbx, f.rsp, f.rbp, f.rsi, f.rdi) =
        (g[0], g[1], g[2], g[3], g[4], g[5], g[6], g[7]);
    (f.r8, f.r9, f.r10, f.r11, f.r12, f.r13, f.r14, f.r15) =
        (g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15]);
    f.rip = r.rip;
    f.rflags = (r.rflags & USER_SETTABLE_FLAGS) | USER_RFLAGS;
}

static USER_RETURN_HOOK: AtomicUsize = AtomicUsize::new(0);

/// What the user-return hook gets: the registers user mode resumes with;
/// it returns true if it changed them.
pub type UserReturnHook = fn(&mut UserRegs) -> bool;

/// Registers a function run on every return to user mode from a system
/// call or interrupt, in kernel context (e.g. to act on a pending thread
/// termination or suspension). It may not return.
pub fn set_user_return_hook(hook: UserReturnHook) {
    USER_RETURN_HOOK.store(hook as usize, Ordering::Release);
}

pub(super) fn run_user_return_hook(regs: &mut UserRegs) -> bool {
    let raw = USER_RETURN_HOOK.load(Ordering::Acquire);
    if raw == 0 {
        return false;
    }
    // SAFETY: only `set_user_return_hook` stores non-zero values, and it
    // stores `UserReturnHook`s.
    let hook = unsafe { core::mem::transmute::<usize, UserReturnHook>(raw) };
    hook(regs)
}
