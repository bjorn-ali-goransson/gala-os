//! SYSCALL/SYSRET entry and return-to-user.

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicUsize, Ordering};

use super::cpu;
use super::gdt::{KERNEL_CS, USER_CS, USER_SS, USER32_CS};
use super::percpu::{self, KERNEL_RSP_OFFSET, USER_RSP_OFFSET};
use crate::mm::paging::USER_END;
use crate::personality::{Dispatchers, NO_DISPATCHER_RESULT, NoDispatcher, Personality};

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
///
/// Outside `arch` the frame is read and changed through its accessors: the
/// architecture-neutral ones (`number`, `result`, `ip`, `sp`, ...) and,
/// for the x86-64 ABIs only (`nt::abi`, `lx::abi`, gala-um), `gpr`.
#[repr(C)]
#[derive(Debug)]
pub struct SyscallFrame {
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    rbp: u64,
    rbx: u64,
    rsi: u64,
    rdi: u64,
    r9: u64,
    r8: u64,
    rdx: u64,
    r10: u64,
    /// Syscall number on entry, return value (NTSTATUS) on exit.
    rax: u64,
    /// rcx: the return address.
    rip: u64,
    /// r11: the flags.
    rflags: u64,
    rsp: u64,
}

impl SyscallFrame {
    /// The frame of system call `number` made with the registers `regs`
    /// (rcx and r11 are taken to hold the return address and the flags,
    /// as SYSCALL leaves them): a call made again (a kernel handover's
    /// carried thread) or trapped elsewhere (gala-um).
    pub fn from_regs(regs: &UserRegs, number: u64) -> SyscallFrame {
        let g = &regs.gpr;
        SyscallFrame {
            r15: g[15],
            r14: g[14],
            r13: g[13],
            r12: g[12],
            rbp: g[5],
            rbx: g[3],
            rsi: g[6],
            rdi: g[7],
            r9: g[9],
            r8: g[8],
            rdx: g[2],
            r10: g[10],
            rax: number,
            rip: regs.rip,
            rflags: regs.rflags,
            rsp: g[4],
        }
    }

    /// The system call's number.
    pub fn number(&self) -> u64 {
        self.rax
    }

    /// The value the call returns (its number until it is set).
    pub fn result(&self) -> u64 {
        self.rax
    }

    pub fn set_result(&mut self, value: u64) {
        self.rax = value;
    }

    /// Where the call returns to.
    pub fn ip(&self) -> u64 {
        self.rip
    }

    /// The user stack pointer at the call.
    pub fn sp(&self) -> u64 {
        self.rsp
    }

    /// The user flags at the call.
    pub fn flags(&self) -> u64 {
        self.rflags
    }

    /// The registers user mode returns with: the frame's, the result in
    /// the return-value register (SYSRET leaves the return address in rcx
    /// and the flags in r11).
    pub fn return_regs(&self) -> UserRegs {
        syscall_regs(self)
    }

    /// x86-64: general-purpose register `r` at the call (rcx holds the
    /// return address and r11 the flags, as SYSCALL left them).
    pub fn gpr(&self, r: Gpr) -> u64 {
        syscall_regs(self).gpr[r as usize]
    }
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

/// The system-call dispatchers, one per personality (`crate::personality`,
/// ROADMAP LX seam 2): a call goes to the one of the calling thread's
/// personality, which the context switch keeps in the per-CPU block.
static DISPATCHERS: Dispatchers = Dispatchers::new();

/// Sets the function the system calls of threads of `personality` are
/// delivered to (on every CPU); `name` names it in logs. Once per
/// personality: a second registration is refused and logged.
pub fn set_handler(personality: Personality, name: &'static str, handler: Handler) {
    if DISPATCHERS.register(personality, name, handler) {
        crate::info!("syscall: personality {personality} dispatches to {name}");
    } else {
        crate::warn!("syscall: personality {personality} already has a dispatcher; {name} ignored");
    }
}

/// Enables SYSCALL on this CPU. Each CPU calls it during its own setup
/// (MSRs are per CPU), before any thread can move between CPUs.
/// IA32_STAR: SYSCALL's kernel CS (SS is CS + 8), and 64-bit SYSRET's
/// base selector (CS = base + 16, SS = base + 8). The base carries RPL 3:
/// AMD's SYSRET loads SS as base + 8 without forcing the RPL (Intel's
/// does), and user mode with SS 0x28 faults at the next IRETQ back to it
/// (gdt.rs's tests check both vendors' rules).
pub(super) const STAR: u64 = (USER32_CS as u64) << 48 | (KERNEL_CS as u64) << 32;

pub fn init_cpu() {
    let star = STAR;
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
    let personality = percpu::personality();
    if let Err(NoDispatcher(p)) = DISPATCHERS.dispatch(personality, frame) {
        crate::warn!(
            "syscall: no dispatcher for personality {p}: system call {:#x} from rip {:#x} \
             (thread {}) fails with {NO_DISPATCHER_RESULT:#x}",
            frame.rax,
            frame.rip,
            crate::sched::current()
        );
        frame.rax = NO_DISPATCHER_RESULT;
    }
    // NtContinue: return with every register, through IRETQ. Only this
    // thread's request counts (another may have been preempted after
    // making one).
    let pending = crate::sched::take_pending_continue();
    // The return hook sees (and may change) the registers user mode gets.
    let mut regs = pending.unwrap_or_else(|| syscall_regs(frame));
    // A system call always returns to 64-bit code.
    let changed = run_user_return_hook(&mut regs, false);
    if pending.is_some() || changed {
        // Nothing on this stack needs dropping: the handler has returned.
        enter_user_regs(&regs);
    }
    // SYSRET to a non-canonical rip would fault in ring 0; never allow it.
    if frame.rip >= USER_END - 0x1000 {
        panic!("syscall return to invalid rip {:#x}", frame.rip);
    }
}

/// Runs the system call in `frame` as if the thread had just made it
/// (`frame` as the entry path saves it: number in rax, arguments in r10,
/// rdx, r8, r9 and on the user stack), then returns to user mode with its
/// result as SYSRET would (rcx = rip, r11 = rflags). A kernel handover
/// reissues a blocked system call this way in the new kernel
/// (ARCHITECTURE invariant 2). Called on the thread's own kernel stack,
/// with its user GS base and address space in place.
pub fn reissue(frame: &mut SyscallFrame) -> ! {
    dispatch(frame);
    enter_user_regs(&syscall_regs(frame))
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

/// x86-64's general-purpose registers, in CONTEXT order (the order of
/// `UserRegs`' register file and of the handover's saved registers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gpr {
    Rax = 0,
    Rcx,
    Rdx,
    Rbx,
    Rsp,
    Rbp,
    Rsi,
    Rdi,
    R8,
    R9,
    R10,
    R11,
    R12,
    R13,
    R14,
    R15,
}

/// A thread's user registers: what user mode runs with (a new thread's
/// first entry, a return through NtContinue, the state a suspended or
/// carried-over thread resumes with).
///
/// Outside `arch` they are read and changed through accessors: the
/// architecture-neutral ones (`ip`, `sp`, `flags`, the system call's
/// result, the handover state's words) and, for the x86-64 ABIs only
/// (`nt::abi`, `lx::abi`, gala-um), `new`, `gpr` and `gprs`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UserRegs {
    /// rax, rcx, rdx, rbx, rsp, rbp, rsi, rdi, r8..r15 (`Gpr` order).
    gpr: [u64; 16],
    rip: u64,
    rflags: u64,
}

/// RFLAGS bits user mode may choose: CF, PF, AF, ZF, SF, DF, OF.
const USER_SETTABLE_FLAGS: u64 = 0xcd5;
const RSP_INDEX: usize = Gpr::Rsp as usize;

impl UserRegs {
    /// The words of the handover state format's saved registers
    /// (`to_words`).
    pub const WORDS: usize = 18;

    /// x86-64: the registers from their values (`gpr` in `Gpr` order).
    pub const fn new(gpr: [u64; 16], rip: u64, rflags: u64) -> UserRegs {
        UserRegs { gpr, rip, rflags }
    }

    /// Whether rip and rsp are user addresses (iretq to a non-canonical
    /// rip would fault in the kernel).
    pub fn is_valid(&self) -> bool {
        self.rip < USER_END && self.gpr[RSP_INDEX] < USER_END
    }

    /// The instruction pointer.
    pub fn ip(&self) -> u64 {
        self.rip
    }

    pub fn set_ip(&mut self, ip: u64) {
        self.rip = ip;
    }

    /// The stack pointer.
    pub fn sp(&self) -> u64 {
        self.gpr[RSP_INDEX]
    }

    pub fn set_sp(&mut self, sp: u64) {
        self.gpr[RSP_INDEX] = sp;
    }

    /// The flags (RFLAGS).
    pub fn flags(&self) -> u64 {
        self.rflags
    }

    pub fn set_flags(&mut self, flags: u64) {
        self.rflags = flags;
    }

    /// The register a system call returns its value in (rax).
    pub fn syscall_result(&self) -> u64 {
        self.gpr[Gpr::Rax as usize]
    }

    pub fn set_syscall_result(&mut self, value: u64) {
        self.gpr[Gpr::Rax as usize] = value;
    }

    /// x86-64: register `r`.
    pub fn gpr(&self, r: Gpr) -> u64 {
        self.gpr[r as usize]
    }

    pub fn set_gpr(&mut self, r: Gpr, value: u64) {
        self.gpr[r as usize] = value;
    }

    /// x86-64: the general-purpose registers (`Gpr` order).
    pub fn gprs(&self) -> &[u64; 16] {
        &self.gpr
    }

    pub fn gprs_mut(&mut self) -> &mut [u64; 16] {
        &mut self.gpr
    }

    /// The registers as the handover state format saves them (the 16
    /// general-purpose registers in `Gpr` order, rip, rflags).
    pub fn to_words(&self) -> [u64; Self::WORDS] {
        let mut w = [0u64; Self::WORDS];
        w[..16].copy_from_slice(&self.gpr);
        w[16] = self.rip;
        w[17] = self.rflags;
        w
    }

    /// The registers `to_words` saved.
    pub fn from_words(w: &[u64; Self::WORDS]) -> UserRegs {
        let mut gpr = [0u64; 16];
        gpr.copy_from_slice(&w[..16]);
        UserRegs { gpr, rip: w[16], rflags: w[17] }
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

/// What the user-return hook gets: the registers user mode resumes with
/// and whether that is 32-bit code (an interrupt in compatibility mode,
/// CS 0x23: a WOW64 thread); it returns true if it changed them.
pub type UserReturnHook = fn(&mut UserRegs, bool) -> bool;

/// Registers a function run on every return to user mode from a system
/// call or interrupt, in kernel context (e.g. to act on a pending thread
/// termination or suspension). It may not return.
pub fn set_user_return_hook(hook: UserReturnHook) {
    USER_RETURN_HOOK.store(hook as usize, Ordering::Release);
}

pub(super) fn run_user_return_hook(regs: &mut UserRegs, compat: bool) -> bool {
    let raw = USER_RETURN_HOOK.load(Ordering::Acquire);
    if raw == 0 {
        return false;
    }
    // SAFETY: only `set_user_return_hook` stores non-zero values, and it
    // stores `UserReturnHook`s.
    let hook = unsafe { core::mem::transmute::<usize, UserReturnHook>(raw) };
    let changed = hook(regs, compat);
    if crate::sched::CHECK_USER_RETURN.load(Ordering::Relaxed) {
        crate::sched::check_user_return();
    }
    changed
}
