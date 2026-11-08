//! System calls (SVC from EL0) and the user registers (stub). The frame
//! and register accessors are AArch64's: x0-x30, sp, pc, pstate; the
//! number in x8 and the result in x0 (Linux's convention).

use super::unported;
use crate::mm::paging::USER_END;
use crate::personality::Personality;

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

/// The user registers at an SVC.
#[repr(C)]
#[derive(Debug)]
pub struct SyscallFrame {
    regs: UserRegs,
}

impl SyscallFrame {
    pub fn from_regs(regs: &UserRegs, number: u64) -> SyscallFrame {
        let mut regs = *regs;
        regs.x[8] = number;
        SyscallFrame { regs }
    }

    pub fn number(&self) -> u64 {
        self.regs.x[8]
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

pub fn set_handler(_personality: Personality, _name: &'static str, _handler: Handler) {
    unported("syscall::set_handler")
}

pub fn init_cpu() {
    unported("syscall::init_cpu")
}

pub fn reissue(_frame: &mut SyscallFrame) -> ! {
    unported("syscall::reissue")
}

pub fn enter_user(_entry: u64, _stack: u64, _arg: u64) -> ! {
    unported("syscall::enter_user")
}

pub fn enter_user_regs(_regs: &UserRegs) -> ! {
    unported("syscall::enter_user_regs")
}

/// Makes the current system call return to user mode with `regs`.
pub fn continue_with(regs: UserRegs) {
    crate::sched::set_pending_continue(regs);
}

pub type UserReturnHook = fn(&mut UserRegs, bool) -> bool;

pub fn set_user_return_hook(_hook: UserReturnHook) {
    unported("syscall::set_user_return_hook")
}
