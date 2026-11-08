//! Windows' ARM64 ABI (stub): where the kernel's aarch64 check needs the
//! NT layer's per-architecture part to exist. What the ABI says plainly
//! is written down (arguments in x0-x7, then the stack; a call's argument
//! in x0); the records (ARM64's CONTEXT, the exception and APC frames, the
//! KUSER_SHARED_DATA processor fields) are not, and their services answer
//! STATUS_NOT_IMPLEMENTED.

use super::ArgSlot;
use crate::nt::crashreport::Note;
use crate::nt::object::ObjectRef;
use crate::nt::platform::{Gpr, SharedPage, SyscallFrame, TrapFrame, UserRegs};
use crate::nt::process::Process;
use crate::nt::registry::Processor;
use crate::nt::status::NtStatus;
use crate::nt::syscalls::Call;

/// ARM64's CONTEXT record.
pub mod context {
    /// sizeof(ARM64_NT_CONTEXT).
    pub const SIZE: usize = 0x390;
}

#[track_caller]
fn unported(what: &str) -> ! {
    panic!("nt on aarch64: {what} is not ported yet")
}

const REGISTER_ARGS: usize = 8;

/// System call argument `i`: x0-x7, then the user stack.
pub fn syscall_arg(frame: &SyscallFrame, i: usize) -> ArgSlot {
    saved_arg(&frame.return_regs(), i)
}

pub fn saved_arg(regs: &UserRegs, i: usize) -> ArgSlot {
    const X: [Gpr; REGISTER_ARGS] =
        [Gpr::X0, Gpr::X1, Gpr::X2, Gpr::X3, Gpr::X4, Gpr::X5, Gpr::X6, Gpr::X7];
    match X.get(i) {
        Some(&r) => ArgSlot::Value(regs.gpr(r)),
        None => ArgSlot::Stack(regs.sp() + 8 * (i - REGISTER_ARGS) as u64),
    }
}

/// The stub's caller: the link register.
pub fn stub_return_address(_process: &Process, regs: &UserRegs) -> Option<u64> {
    Some(regs.gpr(Gpr::X30))
}

/// The registers that call `entry(arg)` on a stack whose top is
/// `stack_top`: x0 = `arg`, sp 16-aligned, lr 0.
pub fn call_regs(entry: u64, arg: u64, stack_top: u64) -> UserRegs {
    let mut regs = UserRegs::default();
    regs.set_gpr(Gpr::X0, arg);
    regs.set_sp(stack_top & !0xf);
    regs.set_ip(entry);
    regs
}

pub fn ldr_thunk_regs(
    _process: &Process,
    _start: &UserRegs,
    _stack_top: u64,
    _thunk: u64,
    _ntdll: u64,
) -> Result<UserRegs, NtStatus> {
    Err(NtStatus::NOT_IMPLEMENTED)
}

pub fn thread_start(_ctx: &[u8]) -> (UserRegs, u64, Option<u32>) {
    unported("NtCreateThread's ARM64 CONTEXT")
}

pub fn deliver_to_user(
    _process: &Process,
    _frame: &mut TrapFrame,
    _status: NtStatus,
    _cr2: Option<u64>,
) -> bool {
    false
}

pub fn keep_note(
    process: &Process,
    frame: &TrapFrame,
    status: NtStatus,
    ip: u64,
    _cr2: Option<u64>,
) {
    let mut regs = frame.user_regs();
    regs.set_ip(ip);
    *process.last_exception.lock() = Some(Note::new(process, status, None, regs, false));
}

pub fn report_registers<T: core::fmt::Display>(_note: &Note, _stack: &[Option<T>]) {}

/// The page's machine fields (ARM64); its processor features are not
/// written yet.
pub fn write_kuser_processor(page: &SharedPage) {
    use crate::nt::kuser::{IMAGE_NUMBER_HIGH, IMAGE_NUMBER_LOW, NATIVE_PROCESSOR_ARCHITECTURE};
    const IMAGE_FILE_MACHINE_ARM64: u16 = 0xaa64;
    const PROCESSOR_ARCHITECTURE_ARM64: u16 = 12;
    page.write(IMAGE_NUMBER_LOW, &IMAGE_FILE_MACHINE_ARM64.to_le_bytes());
    page.write(IMAGE_NUMBER_HIGH, &IMAGE_FILE_MACHINE_ARM64.to_le_bytes());
    page.write(NATIVE_PROCESSOR_ARCHITECTURE, &PROCESSOR_ARCHITECTURE_ARM64.to_le_bytes());
}

pub fn processor() -> Processor {
    Processor { name: [0; 48], vendor: [0; 12], family: 0, model: 0, stepping: 0, mhz: 0 }
}

pub(in crate::nt) fn nt_get_context_thread(_call: &mut Call<'_>) -> NtStatus {
    NtStatus::NOT_IMPLEMENTED
}

pub(in crate::nt) fn nt_set_context_thread(_call: &mut Call<'_>) -> NtStatus {
    NtStatus::NOT_IMPLEMENTED
}

pub(in crate::nt) fn nt_continue(_call: &mut Call<'_>) -> NtStatus {
    NtStatus::NOT_IMPLEMENTED
}

pub(in crate::nt) fn gala_unhandled_exception(_call: &mut Call<'_>) -> NtStatus {
    NtStatus::NOT_IMPLEMENTED
}

/// No APC frame yet: the wait's status as it is.
pub(in crate::nt) fn deliver_apc_after(_call: &Call<'_>, status: NtStatus) -> NtStatus {
    status
}

/// No WOW64 on aarch64.
pub(in crate::nt) fn create_wow64_thread(
    _call: &Call<'_>,
    _ctx: &[u8],
    _stack: (u64, u64, u64),
    _suspended: bool,
) -> Result<ObjectRef, NtStatus> {
    Err(NtStatus::NOT_SUPPORTED)
}
