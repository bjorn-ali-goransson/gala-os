//! Exceptions and interrupts: the trap frame (what the vectors save,
//! `entry`) and its accessors over AArch64's registers and syndrome
//! (ESR_EL1: the exception class). IRQs go to the GIC's dispatch (`gic`);
//! other exceptions are reported and stop the kernel for now.

use super::syscall::UserRegs;
use super::unported;
pub use crate::arch::trap::{CpuMode, Fault, FaultAccess, TrapKind};

/// ESR_EL1.EC values the accessors read.
const EC_UNKNOWN: u64 = 0x00;
const EC_ILLEGAL_STATE: u64 = 0x0e;
/// An MSR, MRS or system instruction EL0 may not use (a register the
/// kernel keeps from it): an illegal instruction, as an undefined one.
const EC_SYSREG: u64 = 0x18;
const EC_INSTRUCTION_ABORT_LOWER: u64 = 0x20;
const EC_DATA_ABORT_LOWER: u64 = 0x24;
const EC_INSTRUCTION_ABORT_SAME: u64 = 0x21;
const EC_DATA_ABORT_SAME: u64 = 0x25;
const EC_SVC64: u64 = 0x15;
const EC_BRK: u64 = 0x3c;
/// ESR_EL1.ISS's WnR bit for data aborts.
const ISS_WNR: u64 = 1 << 6;

/// The state an exception saved, as the vectors write it (`entry`): x0-x30,
/// the interrupted SP, ELR_EL1 and SPSR_EL1 (`UserRegs`' layout), then
/// ESR_EL1 and FAR_EL1. 36 words: SP stays 16-byte aligned.
#[repr(C)]
#[derive(Debug)]
pub struct TrapFrame {
    regs: UserRegs,
    /// ESR_EL1: the exception's syndrome.
    esr: u64,
    /// FAR_EL1: the faulting address.
    far: u64,
}

/// The frame's size, which `entry`'s assembly allocates.
pub const TRAP_FRAME_SIZE: usize = 36 * 8;
const _: () = assert!(size_of::<TrapFrame>() == TRAP_FRAME_SIZE);

impl TrapFrame {
    /// Taken from EL0 (SPSR_EL1.M[3:0] is EL0t).
    pub fn from_user_mode(&self) -> bool {
        self.regs.flags() & 0xf == 0
    }

    pub fn mode(&self) -> CpuMode {
        if self.from_user_mode() { CpuMode::User } else { CpuMode::Kernel }
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

    /// Replaces the user registers (a return to EL0 takes them).
    pub(super) fn set_user_regs(&mut self, regs: UserRegs) {
        self.regs = regs;
    }

    /// A frame that returns to EL0 with `regs` (`syscall::enter_user_regs`).
    pub(super) fn to_user(regs: UserRegs) -> Self {
        TrapFrame { regs, esr: 0, far: 0 }
    }

    /// The exception class (ESR_EL1.EC).
    pub fn vector(&self) -> u8 {
        (self.esr >> 26) as u8 & 0x3f
    }

    /// The syndrome's ISS.
    pub fn error_code(&self) -> u64 {
        self.esr & 0x1ff_ffff
    }

    /// A BRK's kind is its immediate's, as Windows ARM64 assigns them:
    /// 0xf000 a breakpoint (`__debugbreak`), 0xf001 a failed assertion,
    /// 0xf003 a fast fail, 0xf004 an integer division by zero (the
    /// compiler's check: AArch64's division does not trap); any other an
    /// illegal instruction (as Wine's arm64 signal handling maps them).
    pub fn kind(&self) -> TrapKind {
        match u64::from(self.vector()) {
            EC_BRK => match self.esr & 0xffff {
                0xf000 => TrapKind::Breakpoint,
                0xf001 => TrapKind::Assertion,
                0xf003 => TrapKind::FastFail,
                0xf004 => TrapKind::DivideError,
                _ => TrapKind::InvalidOpcode,
            },
            EC_UNKNOWN | EC_SYSREG => TrapKind::InvalidOpcode,
            EC_ILLEGAL_STATE => TrapKind::Protection,
            EC_INSTRUCTION_ABORT_LOWER | EC_DATA_ABORT_LOWER => TrapKind::PageFault,
            _ => TrapKind::Other,
        }
    }

    /// BRK leaves the pc on the instruction.
    pub fn breakpoint_address(&self) -> u64 {
        self.regs.ip()
    }

    /// A fast fail's code (`__fastfail`'s argument: x0).
    pub fn fast_fail_code(&self) -> u64 {
        self.regs.gpr(super::syscall::Gpr::X0)
    }

    /// The registers a call's first four integer arguments arrive in,
    /// as x86-64's (ARM64 passes the first eight integer arguments in
    /// x0-x7, of which this reads four: docs/OBSERVABILITY.md, "Import
    /// tracing").
    pub fn arg_regs(&self) -> [u64; 4] {
        use super::syscall::Gpr;
        [
            self.regs.gpr(Gpr::X0),
            self.regs.gpr(Gpr::X1),
            self.regs.gpr(Gpr::X2),
            self.regs.gpr(Gpr::X3),
        ]
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

/// An IRQ (the vectors' IRQ entries): to the GIC's dispatch.
/// An IRQ (the IRQ vectors'): the GIC's dispatch, then, back to EL0, the
/// user-return hook; an ordinary one's exit sets PSTATE.I and the mask
/// "on" before the `eret`.
pub(super) extern "C" fn irq(frame: &mut TrapFrame) {
    if super::gic::dispatch(frame) == super::gic::Taken::Irq {
        if frame.from_user_mode() {
            user_return(frame);
        }
        super::cpu::irq_exit();
    }
}

/// A synchronous exception from EL0: an SVC (`syscall::from_svc`, with
/// interrupts on), else the user-exception handler (an abort's faulting
/// address from FAR; interrupts off by priority, NMIs let through), then
/// the user-return hook; back to EL0 with PSTATE.I set and the mask "on".
pub(super) extern "C" fn lower_sync(frame: &mut TrapFrame) {
    if u64::from(frame.vector()) == EC_SVC64 {
        super::syscall::from_svc(frame);
    } else {
        super::cpu::pmr_off();
        super::cpu::unmask_daif_irq();
        let raw = USER_EXCEPTION_HANDLER.load(core::sync::atomic::Ordering::Acquire);
        let abort =
            matches!(u64::from(frame.vector()), EC_DATA_ABORT_LOWER | EC_INSTRUCTION_ABORT_LOWER);
        let address = abort.then_some(frame.far);
        if raw == 0 {
            panic!(
                "aarch64: user exception {} (ESR {:#x}) at {:#x} with no handler",
                exception_name(frame.vector()),
                frame.esr,
                frame.ip()
            );
        }
        // SAFETY: only `set_user_exception_handler` stores non-zero
        // values, and it stores `UserExceptionHandler`s.
        let handler = unsafe { core::mem::transmute::<usize, UserExceptionHandler>(raw) };
        handler(frame, address);
        user_return(frame);
    }
    super::cpu::irq_exit();
}

/// Runs the user-return hook on a frame from EL0 and leaves it as user
/// mode resumes with it (`syscall::for_user`: the handler before may have
/// changed it too).
fn user_return(frame: &mut TrapFrame) {
    let mut regs = frame.regs;
    super::syscall::run_user_return_hook(&mut regs);
    frame.regs = super::syscall::for_user(regs);
}

pub type Handler = fn(&mut TrapFrame);
pub type NmiHandler = fn(&TrapFrame);
pub type UserExceptionHandler = fn(&mut TrapFrame, Option<u64>);

/// The state dump's handler, for the NMI that asks for one (xtask's, at a
/// timeout). AArch64 has no NMI as x86-64's: the GICv3's pseudo-NMI (an
/// interrupt above the masking priority) comes with the port, so nothing
/// calls it yet, which the log says once.
/// The NMI's handler, as a function's address (0: none): an atomic, not
/// a lock, since the NMI may interrupt any lock's holder.
static NMI_HANDLER: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// The names of the instruction and stack pointers in the state dump's
/// lines.
pub const IP_NAME: &str = "pc";
pub const SP_NAME: &str = "sp";

pub fn set_nmi_handler(handler: NmiHandler) {
    NMI_HANDLER.store(handler as usize, core::sync::atomic::Ordering::Release);
}

/// The pseudo-NMI (`gic::NMI_SGI`), taken with PSTATE.I set.
pub(super) fn nmi(frame: &TrapFrame) {
    let at = NMI_HANDLER.load(core::sync::atomic::Ordering::Acquire);
    if at != 0 {
        // SAFETY: only `set_nmi_handler` stores here, an `NmiHandler`'s
        // address.
        let handler: NmiHandler = unsafe { core::mem::transmute::<usize, NmiHandler>(at) };
        handler(frame);
    }
}

/// What handles an exception from EL0 (the NT and Linux layers' fault
/// handling); kept for the EL0 vectors user mode will have.
/// The user-exception handler's address (0: none).
static USER_EXCEPTION_HANDLER: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

pub fn set_user_exception_handler(handler: UserExceptionHandler) {
    USER_EXCEPTION_HANDLER.store(handler as usize, core::sync::atomic::Ordering::Release);
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
        EC_SYSREG => "system register or instruction not allowed",
        EC_ILLEGAL_STATE => "illegal execution state",
        EC_INSTRUCTION_ABORT_LOWER => "instruction abort from EL0",
        EC_DATA_ABORT_LOWER => "data abort from EL0",
        EC_INSTRUCTION_ABORT_SAME => "instruction abort from EL1",
        EC_DATA_ABORT_SAME => "data abort from EL1",
        EC_SVC64 => "SVC (system call)",
        EC_BRK => "BRK instruction",
        _ => "exception",
    }
}

/// Raises `fault` on purpose (the exception path's tests): an undefined
/// instruction (`udf`, ESR class 0), a data abort reading the unmapped
/// first page (Limine maps no lower half: TTBR0 is unspecified, so the
/// read faults at translation or as a TTBR0 walk; ESR class 0x25). A stack
/// overflow needs the port's guard pages.
pub fn provoke(fault: Fault) -> ! {
    match fault {
        // SAFETY: `udf` raises an undefined instruction exception.
        Fault::InvalidOpcode => unsafe { core::arch::asm!("udf #0", options(nomem, nostack)) },
        // SAFETY: the read faults (nothing maps the first page).
        Fault::PageFault => unsafe {
            core::ptr::read_volatile(core::ptr::without_provenance::<u64>(0x8));
        },
        Fault::StackOverflow => unported("interrupts::provoke(stack overflow: no guard pages yet)"),
    }
    unreachable!("provoked {fault:?} did not fault")
}
