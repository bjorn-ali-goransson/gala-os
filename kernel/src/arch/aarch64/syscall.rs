//! System calls (SVC from EL0), entering user mode, and the user
//! registers: x0-x30, sp, pc, pstate. The number is the personality's:
//! NT's in the SVC's immediate (ESR's ISS: Windows ARM64's ntdll makes
//! `svc #N`), Linux's in x8; the arguments in x0-x7 (and the stack), the
//! result in x0. A system call runs with interrupts on, then the pending
//! `NtContinue` and the user-return hook, then back to EL0 through the
//! vectors' `eret` (`entry`). Entering user mode builds that same frame
//! at the top of the thread's kernel stack (SP_EL1 there: the next
//! exception from EL0 starts at the top); an NT thread's x18 is its TEB
//! (`percpu::user_gs_base`), a Linux thread's TPIDR_EL0 its TLS base.

use core::sync::atomic::{AtomicUsize, Ordering};

use crate::mm::paging::USER_END;
use crate::personality::{NO_DISPATCHER_RESULT, NoDispatcher, Personality};

/// AArch64's general-purpose registers (x0-x30; x29 the frame pointer,
/// x30 the link register).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gpr {
    X0 = 0,
    X1,
    X2,
    X3,
    X4,
    X5,
    X6,
    X7,
    X8,
    X9,
    X10,
    X11,
    X12,
    X13,
    X14,
    X15,
    X16,
    X17,
    X18,
    X19,
    X20,
    X21,
    X22,
    X23,
    X24,
    X25,
    X26,
    X27,
    X28,
    X29,
    X30,
}

/// A thread's user registers.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UserRegs {
    x: [u64; 31],
    sp: u64,
    pc: u64,
    pstate: u64,
}

impl UserRegs {
    /// The words of the handover state format's saved registers.
    pub const WORDS: usize = 34;

    pub fn is_valid(&self) -> bool {
        self.pc < USER_END && self.sp < USER_END
    }

    pub fn ip(&self) -> u64 {
        self.pc
    }

    pub fn set_ip(&mut self, ip: u64) {
        self.pc = ip;
    }

    pub fn sp(&self) -> u64 {
        self.sp
    }

    pub fn set_sp(&mut self, sp: u64) {
        self.sp = sp;
    }

    /// PSTATE (NZCV and the rest of SPSR_EL1's user bits).
    pub fn flags(&self) -> u64 {
        self.pstate
    }

    pub fn set_flags(&mut self, flags: u64) {
        self.pstate = flags;
    }

    pub fn syscall_result(&self) -> u64 {
        self.x[0]
    }

    pub fn set_syscall_result(&mut self, value: u64) {
        self.x[0] = value;
    }

    pub fn gpr(&self, r: Gpr) -> u64 {
        self.x[r as usize]
    }

    pub fn set_gpr(&mut self, r: Gpr, value: u64) {
        self.x[r as usize] = value;
    }

    /// x0-x30, sp, pc, pstate.
    pub fn to_words(&self) -> [u64; Self::WORDS] {
        let mut w = [0u64; Self::WORDS];
        w[..31].copy_from_slice(&self.x);
        (w[31], w[32], w[33]) = (self.sp, self.pc, self.pstate);
        w
    }

    pub fn from_words(w: &[u64; Self::WORDS]) -> UserRegs {
        let mut x = [0u64; 31];
        x.copy_from_slice(&w[..31]);
        UserRegs { x, sp: w[31], pc: w[32], pstate: w[33] }
    }
}

/// The user registers at an SVC and its number (NT's from the SVC's
/// immediate, which no register holds; Linux's x8).
#[repr(C)]
#[derive(Debug)]
pub struct SyscallFrame {
    regs: UserRegs,
    number: u64,
}

impl SyscallFrame {
    pub fn from_regs(regs: &UserRegs, number: u64) -> SyscallFrame {
        SyscallFrame { regs: *regs, number }
    }

    pub fn number(&self) -> u64 {
        self.number
    }

    pub fn result(&self) -> u64 {
        self.regs.x[0]
    }

    pub fn set_result(&mut self, value: u64) {
        self.regs.x[0] = value;
    }

    pub fn ip(&self) -> u64 {
        self.regs.pc
    }

    pub fn sp(&self) -> u64 {
        self.regs.sp
    }

    pub fn flags(&self) -> u64 {
        self.regs.pstate
    }

    pub fn return_regs(&self) -> UserRegs {
        self.regs
    }

    pub fn gpr(&self, r: Gpr) -> u64 {
        self.regs.gpr(r)
    }
}

pub type Handler = fn(&mut SyscallFrame);

/// The personalities' system call dispatchers (registered at boot; the
/// SVC path from EL0 that calls them comes with user mode, ROADMAP AR).
static DISPATCHERS: crate::personality::Dispatchers = crate::personality::Dispatchers::new();

pub fn set_handler(personality: Personality, name: &'static str, handler: Handler) {
    if DISPATCHERS.register(personality, name, handler) {
        crate::info!("syscall: personality {personality} dispatches to {name}");
    } else {
        crate::warn!("syscall: personality {personality} already has a dispatcher; {name} ignored");
    }
}

/// Nothing to set up per CPU: an SVC from EL0 arrives through the
/// exception vectors (x86-64 programs its SYSCALL MSRs here).
pub fn init_cpu() {}

/// PSTATE's bits user mode may set: NZCV. The rest is EL0t with every
/// exception unmasked.
const USER_PSTATE: u64 = 0xf000_0000;

/// `regs` as user mode resumes with them, whatever set them (a system
/// call's NtContinue, an exception dispatched to user mode, a hook):
/// EL0t with its condition flags only, and an NT thread's x18 its TEB
/// (Windows' ARM64 ABI reserves x18 for it; a CONTEXT's X18 does not
/// move it). Every return to EL0 goes through here.
pub(super) fn for_user(mut regs: UserRegs) -> UserRegs {
    regs.pstate &= USER_PSTATE;
    if super::percpu::personality() == Personality::Nt {
        regs.x[18] = super::percpu::user_gs_base();
    }
    regs
}

/// An SVC from EL0 (the sync vector's, `interrupts::lower_sync`): its
/// number the personality's, dispatched with interrupts on; then the
/// pending `NtContinue` or the result, and the user-return hook, written
/// back into `frame` for the `eret`.
pub(super) fn from_svc(frame: &mut super::interrupts::TrapFrame) {
    let personality = super::percpu::personality();
    let regs = frame.user_regs();
    let number = match personality {
        Personality::Nt => frame.error_code() & 0xffff,
        Personality::Linux => regs.x[8],
    };
    let mut call = SyscallFrame { regs, number };
    super::cpu::enable_interrupts();
    dispatch(personality, &mut call);
    let mut regs = crate::sched::take_pending_continue().unwrap_or(call.regs);
    run_user_return_hook(&mut regs);
    super::cpu::disable_interrupts();
    frame.set_user_regs(for_user(regs));
}

fn dispatch(personality: Personality, call: &mut SyscallFrame) {
    if let Err(NoDispatcher(p)) = DISPATCHERS.dispatch(personality, call) {
        crate::warn!(
            "syscall: no dispatcher for personality {p}: system call {:#x} from pc {:#x} \
             (thread {}) fails with {NO_DISPATCHER_RESULT:#x}",
            call.number,
            call.regs.pc,
            crate::sched::current()
        );
        call.regs.x[0] = NO_DISPATCHER_RESULT;
    }
}

/// Runs the system call in `frame` as if the thread had just made it,
/// then returns to user mode with its result (a kernel handover's
/// reissue, as x86-64's).
pub fn reissue(frame: &mut SyscallFrame) -> ! {
    let personality = super::percpu::personality();
    super::cpu::enable_interrupts();
    dispatch(personality, frame);
    let regs = crate::sched::take_pending_continue().unwrap_or(frame.regs);
    enter_user_regs(&regs)
}

/// Starts user code at `entry` with stack `stack` and x0 = `arg`.
pub fn enter_user(entry: u64, stack: u64, arg: u64) -> ! {
    let mut regs = UserRegs::default();
    regs.x[0] = arg;
    regs.sp = stack;
    regs.pc = entry;
    enter_user_regs(&regs)
}

/// Enters user mode with `regs` (x18 the NT thread's TEB; a Linux
/// thread's TLS base in TPIDR_EL0): a `TrapFrame` at the top of the
/// thread's kernel stack, then the vectors' return path. Never returns.
pub fn enter_user_regs(regs: &UserRegs) -> ! {
    assert!(regs.is_valid(), "user entry outside user space");
    let top = super::percpu::kernel_stack();
    assert!(top != 0, "no kernel stack for user entry");
    let r = for_user(*regs);
    if super::percpu::personality() == Personality::Linux {
        let tls = super::percpu::user_fs_base();
        // SAFETY: TPIDR_EL0 is user mode's thread pointer; only its value
        // changes.
        unsafe {
            core::arch::asm!("msr tpidr_el0, {}", in(reg) tls, options(nomem, nostack, preserves_flags))
        };
    }
    let frame = super::interrupts::TrapFrame::to_user(r);
    let at = top - super::interrupts::TRAP_FRAME_SIZE as u64;
    // Interrupts off for the `eret` (PSTATE.I; the mask "on" for EL0).
    super::cpu::irq_exit();
    // SAFETY: copies the frame to [at, top), the top of this thread's
    // kernel stack, word by word in the direction its overlap with the
    // source (on this same stack) needs, then returns to EL0 from it. No
    // code runs on this stack afterwards (the function never returns), so
    // overwriting its frames is harmless; the frame's registers are user
    // mode's, its pc and sp in user space (`is_valid`).
    unsafe {
        core::arch::asm!(
            "cmp x0, x1",
            "b.lo 2f",
            "1:",
            "ldr x4, [x0], #8",
            "str x4, [x1], #8",
            "subs x2, x2, #1",
            "b.ne 1b",
            "b 3f",
            "2:",
            "add x0, x0, x2, lsl #3",
            "add x1, x1, x2, lsl #3",
            "4:",
            "ldr x4, [x0, #-8]!",
            "str x4, [x1, #-8]!",
            "subs x2, x2, #1",
            "b.ne 4b",
            "3:",
            "mov sp, x3",
            "b gala_aarch64_frame_return",
            in("x0") core::ptr::from_ref(&frame) as u64,
            in("x1") at,
            in("x2") (super::interrupts::TRAP_FRAME_SIZE / 8) as u64,
            in("x3") at,
            options(noreturn)
        )
    }
}

/// Makes the current system call return to user mode with `regs`.
pub fn continue_with(regs: UserRegs) {
    crate::sched::set_pending_continue(regs);
}

/// What the user-return hook gets: the registers user mode resumes with
/// and whether that is 32-bit code (never here); it returns true if it
/// changed them.
pub type UserReturnHook = fn(&mut UserRegs, bool) -> bool;

/// The hook's address (0: none): the NT layer's pending work before each
/// return to user mode (it may not return).
static USER_RETURN_HOOK: AtomicUsize = AtomicUsize::new(0);

pub fn set_user_return_hook(hook: UserReturnHook) {
    USER_RETURN_HOOK.store(hook as usize, Ordering::Release);
}

/// Runs the user-return hook on `regs`: whether it changed them.
pub(super) fn run_user_return_hook(regs: &mut UserRegs) -> bool {
    let raw = USER_RETURN_HOOK.load(Ordering::Acquire);
    if raw == 0 {
        return false;
    }
    // SAFETY: only `set_user_return_hook` stores non-zero values, and it
    // stores `UserReturnHook`s.
    let hook = unsafe { core::mem::transmute::<usize, UserReturnHook>(raw) };
    let changed = hook(regs, false);
    if crate::sched::CHECK_USER_RETURN.load(Ordering::Relaxed) {
        crate::sched::check_user_return();
    }
    changed
}
