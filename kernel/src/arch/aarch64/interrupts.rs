//! Exceptions and interrupts (stub): the trap frame's accessors over
//! AArch64's registers and syndrome (ESR_EL1: the exception class),
//! nothing installed.

use super::syscall::UserRegs;
use super::unported;
pub use crate::arch::trap::{CpuMode, Fault, FaultAccess, TrapKind};

/// ESR_EL1.EC values the accessors read.
const EC_UNKNOWN: u64 = 0x00;
const EC_ILLEGAL_STATE: u64 = 0x0e;
const EC_INSTRUCTION_ABORT_LOWER: u64 = 0x20;
const EC_DATA_ABORT_LOWER: u64 = 0x24;
const EC_BRK: u64 = 0x3c;
/// ESR_EL1.ISS's WnR bit for data aborts.
const ISS_WNR: u64 = 1 << 6;

/// The state an exception saved.
#[repr(C)]
#[derive(Debug)]
pub struct TrapFrame {
    regs: UserRegs,
    /// ESR_EL1: the exception's syndrome.
    esr: u64,
    /// FAR_EL1: the faulting address.
    far: u64,
    /// SPSR_EL1.M: from EL0.
    from_el0: bool,
}

impl TrapFrame {
    pub fn from_user_mode(&self) -> bool {
        self.from_el0
    }

    pub fn mode(&self) -> CpuMode {
        if self.from_el0 { CpuMode::User } else { CpuMode::Kernel }
    }

    pub fn ip(&self) -> u64 {
        self.regs.ip()
    }

    pub fn set_ip(&mut self, ip: u64) {
        self.regs.set_ip(ip);
    }

    pub fn sp(&self) -> u64 {
        self.regs.sp()
    }

    pub fn set_sp(&mut self, sp: u64) {
        self.regs.set_sp(sp);
    }

    pub fn flags(&self) -> u64 {
        self.regs.flags()
    }

    pub fn user_regs(&self) -> UserRegs {
        self.regs
    }

    /// The exception class (ESR_EL1.EC).
    pub fn vector(&self) -> u8 {
        (self.esr >> 26) as u8 & 0x3f
    }

    /// The syndrome's ISS.
    pub fn error_code(&self) -> u64 {
        self.esr & 0x1ff_ffff
    }

    pub fn kind(&self) -> TrapKind {
        match u64::from(self.vector()) {
            EC_BRK => TrapKind::Breakpoint,
            EC_UNKNOWN => TrapKind::InvalidOpcode,
            EC_ILLEGAL_STATE => TrapKind::Protection,
            EC_INSTRUCTION_ABORT_LOWER | EC_DATA_ABORT_LOWER => TrapKind::PageFault,
            _ => TrapKind::Other,
        }
    }

    /// BRK leaves the pc on the instruction.
    pub fn breakpoint_address(&self) -> u64 {
        self.regs.ip()
    }

    pub fn page_fault_access(&self) -> Option<FaultAccess> {
        (self.kind() == TrapKind::PageFault).then(|| self.access())
    }

    pub fn access(&self) -> FaultAccess {
        match u64::from(self.vector()) {
            EC_INSTRUCTION_ABORT_LOWER => FaultAccess::Execute,
            _ if self.esr & ISS_WNR != 0 => FaultAccess::Write,
            _ => FaultAccess::Read,
        }
    }

    /// The faulting address (FAR_EL1).
    pub fn fault_address(&self) -> u64 {
        self.far
    }

    /// The frame pointer and stack pointer a kernel backtrace starts from.
    pub(in crate::arch) fn frame_pointers(&self) -> (u64, u64) {
        (self.regs.gpr(super::syscall::Gpr::X29), self.regs.sp())
    }
}

pub fn init() {
    unported("interrupts::init")
}

pub type Handler = fn(&mut TrapFrame);
pub type NmiHandler = fn(&TrapFrame);
pub type UserExceptionHandler = fn(&mut TrapFrame, Option<u64>);

pub fn set_nmi_handler(_handler: NmiHandler) {
    unported("interrupts::set_nmi_handler")
}

pub fn set_user_exception_handler(_handler: UserExceptionHandler) {
    unported("interrupts::set_user_exception_handler")
}

pub fn set_handler(_vector: u8, _handler: Handler) {
    unported("interrupts::set_handler")
}

pub fn breakpoint() {
    unported("interrupts::breakpoint")
}

pub fn breakpoint_count() -> u64 {
    0
}

/// The exception class's name.
pub fn exception_name(vector: u8) -> &'static str {
    match u64::from(vector) {
        EC_UNKNOWN => "unknown reason (undefined instruction)",
        EC_ILLEGAL_STATE => "illegal execution state",
        EC_INSTRUCTION_ABORT_LOWER => "instruction abort from EL0",
        EC_DATA_ABORT_LOWER => "data abort from EL0",
        EC_BRK => "BRK instruction",
        _ => "exception",
    }
}

pub fn provoke(_fault: Fault) -> ! {
    unported("interrupts::provoke")
}
