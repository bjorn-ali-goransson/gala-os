//! gala-um's user entry: NT system calls through syscall user dispatch
//! (SIGSYS), CPU exceptions through the fault signals, and entering user
//! mode by returning from a signal with the program's registers.
//!
//! Syscall user dispatch: the prctl's always-allowed range is the host's
//! part of the address space (`HOST_START..`), and program code is always
//! below it, so the selector stays at BLOCK: host system calls, also
//! rt_sigreturn and those of the handlers, just run. It is per thread:
//! every thread that runs program code turns it on (`enable_dispatch`).
//!
//! Every handler runs on the thread's alternate signal stack and treats
//! the interrupted user state in the ucontext as the kernel treats a
//! SyscallFrame or TrapFrame: the NT layer reads and changes it, and the
//! return from the handler resumes the program with it.

use core::cell::Cell;
use core::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

use super::HOST_START;
use crate::arch::cpu::FxState;
use crate::arch::interrupts::TrapFrame;
use crate::arch::syscall::{SyscallFrame, UserRegs};

const HOST_END: u64 = 0x8000_0000_0000;
const PR_SET_SYSCALL_USER_DISPATCH: i32 = 59;
const PR_SYS_DISPATCH_ON: u64 = 1;
const SYSCALL_DISPATCH_FILTER_BLOCK: u8 = 1;
/// asm/prctl.h.
const ARCH_SET_GS: i32 = 0x1001;
/// The signal that enters user mode (see `enter_user`).
const SIG_ENTER: i32 = libc::SIGUSR2;
/// The signal that sends a thread in user code through the user-return
/// hook (`Threads::interrupt`; the kernel's timer tick does this).
pub const SIG_INTERRUPT: i32 = libc::SIGURG;
/// RFLAGS bits user mode may choose (as the kernel's syscall return).
const USER_SETTABLE_FLAGS: u64 = 0xcd5;
const ALT_STACK: usize = 256 * 1024;

static SELECTOR: AtomicU8 = AtomicU8::new(SYSCALL_DISPATCH_FILTER_BLOCK);

static SYSCALL_HANDLER: OnceLock<fn(&mut SyscallFrame)> = OnceLock::new();
static RETURN_HOOK: OnceLock<fn(&mut UserRegs) -> bool> = OnceLock::new();
static EXCEPTION_HANDLER: OnceLock<fn(&mut TrapFrame, Option<u64>)> = OnceLock::new();

std::thread_local! {
    /// The ucontext of the signal being handled on this thread.
    static CONTEXT: Cell<*mut libc::ucontext_t> = const { Cell::new(core::ptr::null_mut()) };
    /// Registers the current system call returns with (NtContinue).
    static PENDING: Cell<Option<UserRegs>> = const { Cell::new(None) };
    /// Registers `enter_user` hands to its signal handler.
    static ENTER: Cell<Option<UserRegs>> = const { Cell::new(None) };
    /// This thread's user GS base (its TEB).
    static GS_BASE: Cell<u64> = const { Cell::new(0) };
}

pub fn set_syscall_handler(handler: fn(&mut SyscallFrame)) {
    let _ = SYSCALL_HANDLER.set(handler);
}

pub fn set_user_return_hook(hook: fn(&mut UserRegs) -> bool) {
    let _ = RETURN_HOOK.set(hook);
}

pub fn set_user_exception_handler(handler: fn(&mut TrapFrame, Option<u64>)) {
    let _ = EXCEPTION_HANDLER.set(handler);
}

pub fn continue_with(regs: UserRegs) {
    PENDING.with(|p| p.set(Some(regs)));
}

pub fn set_gs_base(base: u64) {
    GS_BASE.with(|g| g.set(base));
}

type Gregs = [libc::greg_t; 23];

fn g(gregs: &Gregs, r: i32) -> u64 {
    gregs[r as usize] as u64
}

fn set(gregs: &mut Gregs, r: i32, v: u64) {
    gregs[r as usize] = v as libc::greg_t;
}

/// Writes `regs` into the interrupted context (rip, rsp, all GPRs, the
/// user-settable flags).
fn set_regs(gregs: &mut Gregs, r: &UserRegs) {
    use libc::*;
    let order = [
        REG_RAX, REG_RCX, REG_RDX, REG_RBX, REG_RSP, REG_RBP, REG_RSI, REG_RDI, REG_R8, REG_R9,
        REG_R10, REG_R11, REG_R12, REG_R13, REG_R14, REG_R15,
    ];
    for (i, reg) in order.into_iter().enumerate() {
        set(gregs, reg, r.gpr[i]);
    }
    set(gregs, REG_RIP, r.rip);
    let flags = (g(gregs, REG_EFL) & !USER_SETTABLE_FLAGS) | (r.rflags & USER_SETTABLE_FLAGS);
    set(gregs, REG_EFL, flags);
}

fn return_to_user(gregs: &mut Gregs, mut regs: UserRegs) {
    if let Some(hook) = RETURN_HOOK.get() {
        hook(&mut regs);
    }
    set_regs(gregs, &regs);
}

fn with_context(context: *mut libc::c_void, f: impl FnOnce(&mut libc::ucontext_t)) {
    let uc = context.cast::<libc::ucontext_t>();
    CONTEXT.with(|c| c.set(uc));
    // SAFETY: the kernel passes the interrupted thread's ucontext, valid
    // until the handler returns.
    f(unsafe { &mut *uc });
    CONTEXT.with(|c| c.set(core::ptr::null_mut()));
}

extern "C" fn on_sigsys(_sig: i32, _info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    use libc::*;
    with_context(context, |uc| {
        let gr = &mut uc.uc_mcontext.gregs;
        let mut frame = SyscallFrame {
            r15: g(gr, REG_R15),
            r14: g(gr, REG_R14),
            r13: g(gr, REG_R13),
            r12: g(gr, REG_R12),
            rbp: g(gr, REG_RBP),
            rbx: g(gr, REG_RBX),
            rsi: g(gr, REG_RSI),
            rdi: g(gr, REG_RDI),
            r9: g(gr, REG_R9),
            r8: g(gr, REG_R8),
            rdx: g(gr, REG_RDX),
            r10: g(gr, REG_R10),
            rax: g(gr, REG_RAX),
            rip: g(gr, REG_RIP),
            rflags: g(gr, REG_EFL),
            rsp: g(gr, REG_RSP),
        };
        if let Some(handler) = SYSCALL_HANDLER.get() {
            handler(&mut frame);
        }
        // As the kernel's SYSRET: rcx = rip, r11 = rflags.
        let f = &frame;
        let regs = PENDING.with(Cell::take).unwrap_or(UserRegs {
            gpr: [
                f.rax, f.rip, f.rdx, f.rbx, f.rsp, f.rbp, f.rsi, f.rdi, f.r8, f.r9, f.r10,
                f.rflags, f.r12, f.r13, f.r14, f.r15,
            ],
            rip: f.rip,
            rflags: f.rflags,
        });
        return_to_user(gr, regs);
    });
}

extern "C" fn on_fault(sig: i32, info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    use libc::*;
    with_context(context, |uc| {
        let gr = &mut uc.uc_mcontext.gregs;
        let rip = g(gr, REG_RIP);
        // SAFETY: si_addr is valid for the fault signals.
        let addr = unsafe { (*info).si_addr() } as u64;
        if rip >= HOST_START {
            std::eprintln!(
                "[error] gala-um: signal {sig} in the host at {rip:#x} (address {addr:#x})"
            );
            std::process::abort();
        }
        let vector = g(gr, REG_TRAPNO);
        let mut tf = TrapFrame {
            r15: g(gr, REG_R15),
            r14: g(gr, REG_R14),
            r13: g(gr, REG_R13),
            r12: g(gr, REG_R12),
            r11: g(gr, REG_R11),
            r10: g(gr, REG_R10),
            r9: g(gr, REG_R9),
            r8: g(gr, REG_R8),
            rbp: g(gr, REG_RBP),
            rdi: g(gr, REG_RDI),
            rsi: g(gr, REG_RSI),
            rdx: g(gr, REG_RDX),
            rcx: g(gr, REG_RCX),
            rbx: g(gr, REG_RBX),
            rax: g(gr, REG_RAX),
            vector,
            error_code: g(gr, REG_ERR),
            rip,
            cs: 0x33,
            rflags: g(gr, REG_EFL),
            rsp: g(gr, REG_RSP),
            ss: 0x2b,
        };
        let cr2 = (vector == 14).then_some(addr);
        if let Some(handler) = EXCEPTION_HANDLER.get() {
            handler(&mut tf, cr2);
        }
        let regs = UserRegs {
            gpr: [
                tf.rax, tf.rcx, tf.rdx, tf.rbx, tf.rsp, tf.rbp, tf.rsi, tf.rdi, tf.r8, tf.r9,
                tf.r10, tf.r11, tf.r12, tf.r13, tf.r14, tf.r15,
            ],
            rip: tf.rip,
            rflags: tf.rflags,
        };
        return_to_user(gr, regs);
    });
}

extern "C" fn on_interrupt(_sig: i32, _info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    use libc::*;
    // SAFETY: the kernel passes the interrupted thread's ucontext.
    let rip = unsafe { (*context.cast::<ucontext_t>()).uc_mcontext.gregs[REG_RIP as usize] } as u64;
    if rip >= HOST_START {
        // Host code (a system call in progress, a wait): the hook runs when
        // it returns to the program anyway.
        return;
    }
    with_context(context, |uc| {
        let gr = &mut uc.uc_mcontext.gregs;
        let order = [
            REG_RAX, REG_RCX, REG_RDX, REG_RBX, REG_RSP, REG_RBP, REG_RSI, REG_RDI, REG_R8, REG_R9,
            REG_R10, REG_R11, REG_R12, REG_R13, REG_R14, REG_R15,
        ];
        let mut regs = UserRegs { rip, rflags: g(gr, REG_EFL), ..UserRegs::default() };
        for (i, reg) in order.into_iter().enumerate() {
            regs.gpr[i] = g(gr, reg);
        }
        return_to_user(gr, regs);
    });
}

extern "C" fn on_enter(_sig: i32, _info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    with_context(context, |uc| {
        if let Some(regs) = ENTER.with(Cell::take) {
            set_regs(&mut uc.uc_mcontext.gregs, &regs);
        }
    });
}

fn install(sig: i32, handler: extern "C" fn(i32, *mut libc::siginfo_t, *mut libc::c_void)) {
    // SAFETY: a zeroed sigaction is valid; handler and flags are filled in.
    unsafe {
        let mut sa: libc::sigaction = core::mem::zeroed();
        sa.sa_sigaction = handler as usize;
        sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
        libc::sigemptyset(&mut sa.sa_mask);
        if libc::sigaction(sig, &sa, core::ptr::null_mut()) != 0 {
            panic!("sigaction {sig}: {}", std::io::Error::last_os_error());
        }
    }
}

/// Installs the process's signal handlers (once, before any thread runs
/// program code).
pub fn init() {
    install(libc::SIGSYS, on_sigsys);
    for sig in [libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGFPE, libc::SIGTRAP] {
        install(sig, on_fault);
    }
    install(SIG_ENTER, on_enter);
    install(SIG_INTERRUPT, on_interrupt);
}

/// Per-thread setup: no blocked signals (a thread created inside another
/// thread's SIGSYS handler inherits its mask, and a blocked SIGSYS from
/// syscall user dispatch kills the process), an alternate signal stack
/// (handlers must not run on the program's stack) and syscall user
/// dispatch.
pub fn init_thread() {
    // SAFETY: an empty signal set, applied to this thread.
    unsafe {
        let mut none: libc::sigset_t = core::mem::zeroed();
        libc::sigemptyset(&mut none);
        libc::pthread_sigmask(libc::SIG_SETMASK, &none, core::ptr::null_mut());
    }
    let stack = alloc::vec![0u8; ALT_STACK].leak();
    let ss = libc::stack_t { ss_sp: stack.as_mut_ptr().cast(), ss_flags: 0, ss_size: ALT_STACK };
    // SAFETY: a leaked buffer that lives as long as the process; a plain
    // prctl naming a static selector byte.
    unsafe {
        if libc::sigaltstack(&ss, core::ptr::null_mut()) != 0 {
            panic!("sigaltstack: {}", std::io::Error::last_os_error());
        }
        if libc::prctl(
            PR_SET_SYSCALL_USER_DISPATCH,
            PR_SYS_DISPATCH_ON,
            HOST_START,
            HOST_END - HOST_START,
            SELECTOR.as_ptr(),
        ) != 0
        {
            panic!("syscall user dispatch: {}", std::io::Error::last_os_error());
        }
    }
    SELECTOR.store(SYSCALL_DISPATCH_FILTER_BLOCK, Ordering::SeqCst);
}

/// Enters user mode with `regs`: sets GS to the thread's TEB, then sends
/// itself SIG_ENTER, whose handler replaces the interrupted registers with
/// `regs`; the return from the handler (rt_sigreturn) loads them all.
pub fn enter_user(regs: &UserRegs) -> ! {
    ENTER.with(|e| e.set(Some(*regs)));
    let gs = GS_BASE.with(Cell::get);
    // SAFETY: GS is unused by Rust and glibc on x86_64 (thread-local storage
    // uses FS), so the program owns it. raise() delivers SIG_ENTER to this
    // thread before it returns.
    unsafe {
        libc::syscall(libc::SYS_arch_prctl, ARCH_SET_GS, gs);
        libc::raise(SIG_ENTER);
    }
    unreachable!("returned from user-mode entry")
}

/// The interrupted context's FXSAVE area: the program's floating-point
/// state (the handler itself runs with a fresh one).
fn fpstate() -> Option<*mut u8> {
    let uc = CONTEXT.with(Cell::get);
    if uc.is_null() {
        return None;
    }
    // SAFETY: the context of the signal being handled on this thread.
    let fp = unsafe { (*uc).uc_mcontext.fpregs };
    (!fp.is_null()).then_some(fp.cast())
}

pub fn fxsave() -> FxState {
    let mut state = FxState([0; 512]);
    match fpstate() {
        // SAFETY: the 512-byte legacy FXSAVE area of the signal frame.
        Some(fp) => unsafe { core::ptr::copy_nonoverlapping(fp, state.0.as_mut_ptr(), 512) },
        None => {
            // Default state: FCW 0x37f, MXCSR 0x1f80.
            state.0[0..2].copy_from_slice(&0x37fu16.to_le_bytes());
            state.0[24..28].copy_from_slice(&0x1f80u32.to_le_bytes());
        }
    }
    state
}

pub fn fxrstor(state: &FxState) {
    if let Some(fp) = fpstate() {
        // SAFETY: as in `fxsave`; MXCSR reserved bits are masked by the
        // kernel's NtContinue path before this.
        unsafe { core::ptr::copy_nonoverlapping(state.0.as_ptr(), fp, 512) };
    }
}

pub fn set_mxcsr(value: u32) {
    match fpstate() {
        // SAFETY: MXCSR is at offset 24 of the FXSAVE area.
        Some(fp) => unsafe { fp.add(24).cast::<u32>().write_unaligned(value) },
        // SAFETY: loading MXCSR on this host thread, before it enters the
        // program (the signal frame then carries it).
        None => unsafe { core::arch::asm!("ldmxcsr [{}]", in(reg) &value, options(nostack)) },
    }
}
