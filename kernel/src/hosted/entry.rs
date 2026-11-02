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
use crate::arch::interrupts::TrapFrame;
use crate::arch::syscall::{SyscallFrame, UserRegs};
use crate::arch::xstate::{self, XState};
use crate::sim::{SpinCapture, SpinState, SpinStep};

const HOST_END: u64 = 0x8000_0000_0000;
const PR_SET_SYSCALL_USER_DISPATCH: i32 = 59;
const PR_SYS_DISPATCH_ON: u64 = 1;
const SYSCALL_DISPATCH_FILTER_BLOCK: u8 = 1;
/// asm/prctl.h.
const ARCH_SET_GS: i32 = 0x1001;
/// asm/prctl.h: with 0, CPUID faults (#GP, SIGSEGV) on this thread.
const ARCH_SET_CPUID: i32 = 0x1012;
/// RFLAGS.TF: single-step.
const RFLAGS_TF: u64 = 1 << 8;
/// RFLAGS.RF: resume (set by the processor around debug traps).
const RFLAGS_RF: u64 = 1 << 16;
/// Spin captures of one spin episode that may give up before the
/// watchdog preempts where it interrupted (not reproducible): with the
/// watchdog's 10 ms between them, a thread computes for about 100 ms of
/// processor time without a system call before that.
const SPIN_GIVE_UPS: u32 = 10;
/// KUSER_SHARED_DATA's page.
const KUSER: u64 = crate::nt::kuser::ADDRESS;
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
    /// Simulation: this thread is single-stepping an instruction that
    /// reads KUSER_SHARED_DATA, whose page is readable until it is done.
    static STEPPING: Cell<bool> = const { Cell::new(false) };
    /// Simulation: this thread spins in program code and is being
    /// single-stepped to its loop's canonical point (`on_interrupt`).
    static CAPTURE: Cell<Option<SpinCapture>> = const { Cell::new(None) };
    /// Simulation: (scheduling points as of the capture that gave up,
    /// captures that gave up since then).
    static GAVE_UP: Cell<(u64, u32)> = const { Cell::new((0, 0)) };
}

/// The registers a spin capture compares.
fn spin_state(gr: &Gregs) -> SpinState {
    use libc::*;
    let order = [
        REG_RAX, REG_RCX, REG_RDX, REG_RBX, REG_RSP, REG_RBP, REG_RSI, REG_RDI, REG_R8, REG_R9,
        REG_R10, REG_R11, REG_R12, REG_R13, REG_R14, REG_R15,
    ];
    let mut state = [0; 18];
    state[0] = g(gr, REG_RIP);
    state[1] = g(gr, REG_EFL) & !(RFLAGS_TF | RFLAGS_RF);
    for (i, reg) in order.into_iter().enumerate() {
        state[2 + i] = g(gr, reg);
    }
    state
}

/// The interrupted program's registers.
fn interrupted_regs(gr: &Gregs) -> UserRegs {
    use libc::*;
    let order = [
        REG_RAX, REG_RCX, REG_RDX, REG_RBX, REG_RSP, REG_RBP, REG_RSI, REG_RDI, REG_R8, REG_R9,
        REG_R10, REG_R11, REG_R12, REG_R13, REG_R14, REG_R15,
    ];
    let mut regs = UserRegs { rip: g(gr, REG_RIP), rflags: g(gr, REG_EFL), ..UserRegs::default() };
    for (i, reg) in order.into_iter().enumerate() {
        regs.gpr[i] = g(gr, reg);
    }
    regs
}

/// Simulation: the watchdog interrupted this thread spinning in program
/// code. Rather than preempt it here (a point that depends on the host's
/// timing), single-step it to its spin loop's canonical point
/// (`crate::sim::SpinCapture`; the steps arrive in `on_fault`). False if
/// captures already gave up too often in this spin episode (the thread
/// computes, or its loop does not repeat exactly): then it is preempted
/// where it is.
fn begin_capture(gr: &mut Gregs) -> bool {
    let (at, n) = GAVE_UP.with(Cell::get);
    if at == super::sim::steps() && n >= SPIN_GIVE_UPS {
        return false;
    }
    CAPTURE.with(|c| c.set(Some(SpinCapture::new(&spin_state(gr)))));
    set(gr, libc::REG_EFL, g(gr, libc::REG_EFL) | RFLAGS_TF);
    true
}

/// Simulation: a spin capture in progress ends (another signal came: the
/// thread made a system call or faulted, so it was not spinning).
fn end_capture(gr: &mut Gregs) {
    if CAPTURE.with(Cell::take).is_some() {
        set(gr, libc::REG_EFL, g(gr, libc::REG_EFL) & !RFLAGS_TF);
    }
}

/// Simulation: a single step of a spin capture (`on_fault`'s debug trap).
fn capture_step(gr: &mut Gregs, mut capture: SpinCapture) {
    match capture.step(&spin_state(gr)) {
        SpinStep::Continue => CAPTURE.with(|c| c.set(Some(capture))),
        SpinStep::Preempt => {
            set(gr, libc::REG_EFL, g(gr, libc::REG_EFL) & !RFLAGS_TF);
            super::sim::preempt(true);
            return_to_user(gr, interrupted_regs(gr));
        }
        SpinStep::GiveUp => {
            set(gr, libc::REG_EFL, g(gr, libc::REG_EFL) & !RFLAGS_TF);
            let steps = super::sim::steps();
            GAVE_UP.with(|c| {
                let (at, n) = c.get();
                c.set((steps, if at == steps { n + 1 } else { 1 }));
            });
        }
    }
}

/// Simulation: KUSER_SHARED_DATA's page is inaccessible to program code
/// (`hide_kuser`), so reading the time from it faults and goes through
/// `sim::clock_read` (virtual time moves; maybe another thread runs). The
/// faulting instruction then runs with the page readable for that one
/// instruction (the trap flag set; the debug trap that follows makes it
/// inaccessible again). Only the baton holder runs program code, so no
/// other thread sees the page readable.
fn kuser_protect(prot: i32) {
    // SAFETY: KUSER_SHARED_DATA's page is gala-um's own mapping of the
    // shared frame (memory.rs); changing its protection affects only the
    // program's view of it (the NT layer writes it through the HHDM).
    unsafe { libc::mprotect(KUSER as *mut libc::c_void, 0x1000, prot) };
}

/// Simulation: makes KUSER_SHARED_DATA's page inaccessible to the program
/// (memory.rs, when it is mapped).
pub fn hide_kuser() {
    kuser_protect(libc::PROT_NONE);
}

/// Simulation: CPUID in program code faults (CPUID faulting) and is
/// answered here: the processor's own answer (CPUID faulting off for the
/// one instruction) with RDRAND and RDSEED cleared, so programs use
/// ProcessPrng, which draws on the seed, and with the APIC ids of the
/// simulated processor (as RDTSCP's TSC_AUX). Returns (eax, ebx, ecx, edx).
fn simulated_cpuid(leaf: u32, sub: u32) -> (u32, u32, u32, u32) {
    // SAFETY: plain arch_prctl calls on this thread; CPUID has no side
    // effects.
    let r = unsafe {
        libc::syscall(libc::SYS_arch_prctl, ARCH_SET_CPUID, 1);
        let r = core::arch::x86_64::__cpuid_count(leaf, sub);
        libc::syscall(libc::SYS_arch_prctl, ARCH_SET_CPUID, 0);
        r
    };
    let cpu = super::threads::current_cpu();
    let (eax, mut ebx, mut ecx, mut edx) = (r.eax, r.ebx, r.ecx, r.edx);
    match leaf {
        1 => {
            ecx &= !(1 << 30); // RDRAND
            ebx = (ebx & 0x00ff_ffff) | (cpu << 24); // initial APIC id
        }
        7 if sub == 0 => ebx &= !(1 << 18), // RDSEED
        0xb | 0x1f => edx = cpu,            // x2APIC id
        _ => {}
    }
    (eax, ebx, ecx, edx)
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
    // The handlers run NT-layer code (system calls, exceptions, the
    // user-return hook): injected heap failures apply.
    // SAFETY: the kernel passes the interrupted thread's ucontext, valid
    // until the handler returns.
    super::nt_code(|| f(unsafe { &mut *uc }));
    CONTEXT.with(|c| c.set(core::ptr::null_mut()));
}

extern "C" fn on_sigsys(_sig: i32, _info: *mut libc::siginfo_t, context: *mut libc::c_void) {
    use libc::*;
    with_context(context, |uc| {
        let gr = &mut uc.uc_mcontext.gregs;
        if super::sim::on() {
            end_capture(gr);
        }
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
        // Simulation: scheduling points before and after the service,
        // the system call's number and result in the trace.
        if super::sim::on() {
            super::sim::point(&[0, frame.rax]);
        }
        if let Some(handler) = SYSCALL_HANDLER.get() {
            handler(&mut frame);
        }
        if super::sim::on() {
            super::sim::point(&[1, frame.rax]);
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
        // Simulation: a step of a spin capture, or (another signal) its end.
        if super::sim::on()
            && let Some(capture) = CAPTURE.with(Cell::take)
        {
            if sig == SIGTRAP && g(gr, REG_TRAPNO) == 1 {
                capture_step(gr, capture);
                return;
            }
            set(gr, REG_EFL, g(gr, REG_EFL) & !RFLAGS_TF);
        }
        // Simulation: the end of a single-stepped read of KUSER_SHARED_DATA
        // (or a fault of that instruction, handled as usual below).
        let stepped = super::sim::on() && STEPPING.with(|s| s.replace(false));
        if stepped {
            hide_kuser();
            set(gr, REG_EFL, g(gr, REG_EFL) & !RFLAGS_TF);
            if sig == SIGTRAP && g(gr, REG_TRAPNO) == 1 {
                return;
            }
        }
        // Simulation: a read of KUSER_SHARED_DATA (the time) faulted. (A
        // write, or a fault of the stepped instruction itself, is the
        // program's access violation, below.)
        if super::sim::on()
            && !stepped
            && sig == SIGSEGV
            && g(gr, REG_TRAPNO) == 14
            && g(gr, REG_ERR) & 2 == 0
            && (KUSER..KUSER + 0x1000).contains(&addr)
        {
            super::sim::clock_read();
            // Another thread may have run (and suspended this one, or
            // queued an APC): the user-return hook, as after a system call.
            return_to_user(gr, interrupted_regs(gr));
            kuser_protect(libc::PROT_READ);
            set(gr, REG_EFL, g(gr, REG_EFL) | RFLAGS_TF);
            STEPPING.with(|s| s.set(true));
            return;
        }
        // Simulation: RDTSC/RDTSCP fault (PR_TSC_SIGSEGV) and read the
        // virtual clock, and CPUID faults (CPUID faulting), in program code
        // and in the host's alike.
        if super::sim::on() && sig == SIGSEGV && g(gr, REG_TRAPNO) == 13 {
            // SAFETY: the instruction at rip was just fetched (a #GP, not
            // a page fault), so its bytes are readable.
            let code = unsafe { core::ptr::read_unaligned(rip as *const [u8; 3]) };
            if code[..2] == [0x0f, 0xa2] {
                let (a, b, c, d) = simulated_cpuid(g(gr, REG_RAX) as u32, g(gr, REG_RCX) as u32);
                for (reg, value) in [(REG_RAX, a), (REG_RBX, b), (REG_RCX, c), (REG_RDX, d)] {
                    set(gr, reg, u64::from(value));
                }
                set(gr, REG_RIP, rip + 2);
                return;
            }
            let length = match code {
                [0x0f, 0x31, _] => Some(2),
                [0x0f, 0x01, 0xf9] => Some(3),
                _ => None,
            };
            if let Some(length) = length {
                let tsc = super::sim::read_tsc();
                set(gr, REG_RAX, tsc & 0xffff_ffff);
                set(gr, REG_RDX, tsc >> 32);
                if length == 3 {
                    // TSC_AUX: Linux keeps the processor number there.
                    set(gr, REG_RCX, u64::from(super::threads::current_cpu()));
                }
                set(gr, REG_RIP, rip + length);
                return;
            }
        }
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
    // Simulation: only the watchdog interrupts program code (a thread
    // spinning without system calls); others get the processor, at the
    // spin loop's canonical point if it has one (`begin_capture`).
    if super::sim::on() {
        if STEPPING.with(Cell::get) || CAPTURE.with(|c| c.get().is_some()) {
            // Single-stepping (a read of KUSER_SHARED_DATA, or a capture
            // already under way).
            return;
        }
        // SAFETY: the kernel passes the interrupted thread's ucontext,
        // valid until the handler returns.
        let gr = unsafe { &mut (*context.cast::<ucontext_t>()).uc_mcontext.gregs };
        if begin_capture(gr) {
            return;
        }
        super::sim::preempt(false);
    }
    with_context(context, |uc| {
        let gr = &mut uc.uc_mcontext.gregs;
        return_to_user(gr, interrupted_regs(gr));
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
    // Simulation: RDTSC and RDTSCP raise SIGSEGV, and `on_fault` answers
    // from the virtual clock (they would read the host's).
    // CPUID faults too (CPUID faulting), and `on_fault` answers it with
    // RDRAND and RDSEED hidden (they cannot be trapped).
    if super::sim::on() {
        // SAFETY: a plain prctl on this thread.
        unsafe { libc::prctl(libc::PR_SET_TSC, libc::PR_TSC_SIGSEGV, 0, 0, 0) };
        // SAFETY: a plain arch_prctl on this thread.
        if unsafe { libc::syscall(libc::SYS_arch_prctl, ARCH_SET_CPUID, 0) } != 0 {
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                std::eprintln!(
                    "sim: no CPUID faulting on this host ({}): RDRAND stays visible, and \
                     programs that use it (Go's, through ProcessPrng) are not reproducible",
                    std::io::Error::last_os_error()
                )
            });
        }
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

/// Linux's signal-frame extension of the FXSAVE area (`_fpx_sw_bytes`
/// in its software-reserved bytes 464..512): magic1, extended_size,
/// xfeatures, xstate_size. With the magic, the XSAVE header and the
/// extended components follow the legacy area (standard format).
const SW_BYTES: usize = 464;
const FP_XSTATE_MAGIC1: u32 = 0x4650_5853;

/// The XSAVE area's size in the signal frame (0: FXSAVE only).
fn frame_xstate_size(fp: *const u8) -> usize {
    // SAFETY: the 512-byte legacy area of the signal frame.
    let (magic, size) = unsafe {
        (
            fp.add(SW_BYTES).cast::<u32>().read_unaligned(),
            fp.add(SW_BYTES + 16).cast::<u32>().read_unaligned(),
        )
    };
    if magic == FP_XSTATE_MAGIC1 { size as usize } else { 0 }
}

/// The components programs can use under gala-um: the host's XCR0 (Linux
/// enables XSAVE) as gala would enable it (`xstate::usable`).
pub fn xstate_enabled() -> u64 {
    static ENABLED: OnceLock<u64> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        if crate::arch::cpu::cpuid(1, 0).ecx & (1 << 27) == 0 {
            return xstate::LEGACY;
        }
        let (lo, hi): (u32, u32);
        // SAFETY: XGETBV(0) is allowed in user mode with CR4.OSXSAVE set
        // (CPUID.1:ECX.OSXSAVE, checked above).
        unsafe {
            core::arch::asm!("xgetbv", in("ecx") 0, out("eax") lo, out("edx") hi,
                             options(nomem, nostack))
        };
        xstate::usable(u64::from(hi) << 32 | u64::from(lo))
    })
}

/// The program's x87/SSE/AVX state: the interrupted context's area in
/// the signal frame (the handler itself runs with a fresh one).
pub fn xsave() -> XState {
    let mut state = XState::initial();
    let enabled = xstate_enabled();
    if let Some(fp) = fpstate() {
        // SAFETY: the 512-byte legacy FXSAVE area of the signal frame.
        unsafe { core::ptr::copy_nonoverlapping(fp, state.0.as_mut_ptr(), xstate::LEGACY_SIZE) };
        let size = frame_xstate_size(fp).min(xstate::size_of(enabled));
        if size > xstate::LEGACY_SIZE {
            // SAFETY: Linux's frame has `xstate_size` bytes of XSAVE area.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    fp.add(xstate::LEGACY_SIZE),
                    state.0.as_mut_ptr().add(xstate::LEGACY_SIZE),
                    size - xstate::LEGACY_SIZE,
                )
            };
            state.sanitize(enabled, u32::MAX);
        } else {
            state.set_bv(xstate::LEGACY);
        }
        state.0[SW_BYTES..xstate::LEGACY_SIZE].fill(0);
    }
    state.normalize(enabled);
    state
}

/// Makes `state` what the program resumes with: the signal frame's area
/// (the return from the handler loads it). Linux's own bytes (the
/// software-reserved part, the header's other components) stay.
pub fn xrstor(state: &XState) {
    let Some(fp) = fpstate() else { return };
    let enabled = xstate_enabled();
    let mut s = state.clone();
    s.normalize(enabled);
    // SAFETY: the legacy area of the signal frame, up to Linux's bytes;
    // MXCSR reserved bits are masked by the kernel's NtContinue path.
    unsafe { core::ptr::copy_nonoverlapping(s.0.as_ptr(), fp, SW_BYTES) };
    let size = frame_xstate_size(fp);
    if size < xstate::size_of(enabled) {
        return;
    }
    let ext = enabled & !xstate::LEGACY;
    // SAFETY: the frame's XSAVE header and components, `size` bytes in all;
    // each component lies within `size_of(enabled)`.
    unsafe {
        let bv = fp.add(xstate::LEGACY_SIZE).cast::<u64>();
        let old = bv.read_unaligned();
        bv.write_unaligned((old & !ext) | (s.bv() & ext) | xstate::LEGACY);
        for (i, &(offset, len)) in xstate::COMPONENTS.iter().enumerate().skip(2) {
            if ext & (1 << i) != 0 {
                core::ptr::copy_nonoverlapping(s.0.as_ptr().add(offset), fp.add(offset), len);
            }
        }
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
