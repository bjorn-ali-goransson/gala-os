//! Windows' ARM64 ABI: arguments in x0-x7, then the stack; a call's
//! argument in x0; ARM64's CONTEXT (`context`), which a process's first
//! thread starts LdrInitializeThunk with, NtContinue resumes and
//! NtCreateThread starts a thread from; the dispatcher frames gala's
//! ntdll reads: an exception's (`deliver_to_user`: the CONTEXT and the
//! EXCEPTION_RECORD) and a user APC's (`deliver_apc`: the CONTEXT and the
//! routine with its arguments); NtGetContextThread and NtSetContextThread
//! (a suspended thread's parked state, or the caller's); the
//! KUSER_SHARED_DATA machine and processor features.

use super::ArgSlot;
use crate::nt::crashreport::Note;
use crate::nt::object::ObjectRef;
use crate::nt::platform::{Gpr, SharedPage, SyscallFrame, TrapFrame, UserRegs};
use crate::nt::process::Process;
use crate::nt::registry::Processor;
use crate::nt::status::NtStatus;
use crate::nt::syscalls::{Call, THREAD_SET_CONTEXT, caller_regs, thread_object};

pub mod context;

use crate::nt::platform::{self, XState, xstate};

// The kernel's FP/SIMD save area is the one CONTEXT's floating-point part
// converts to and from.
const _: () = assert!(xstate::MAX_SIZE == context::FP_STATE_BYTES);

/// The kernel's user registers as CONTEXT's.
fn context_regs(regs: &UserRegs) -> context::Regs {
    let w = regs.to_words();
    let mut x = [0u64; 31];
    x.copy_from_slice(&w[..31]);
    context::Regs { x, sp: w[31], pc: w[32], cpsr: w[33] as u32 }
}

/// CONTEXT's registers as the kernel's user registers.
fn user_regs(regs: &context::Regs) -> UserRegs {
    let mut w = [0u64; UserRegs::WORDS];
    w[..31].copy_from_slice(&regs.x);
    (w[31], w[32], w[33]) = (regs.sp, regs.pc, u64::from(regs.cpsr));
    UserRegs::from_words(&w)
}

/// A CONTEXT's floating-point part as the kernel's FP/SIMD state.
fn xstate_of(bytes: &[u8; context::FP_STATE_BYTES]) -> XState {
    let mut state = XState::initial();
    state.bytes_mut().copy_from_slice(bytes);
    state
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

/// The start through ntdll: LdrInitializeThunk (`thunk`) with x0 = the
/// CONTEXT of `start` (CONTEXT_FULL, the floating-point part the reset
/// state), written at the top of its stack, and x1 = ntdll's base
/// `ntdll`; `start`'s sp is moved below the CONTEXT, and the thunk runs
/// on the stack below that.
pub fn ldr_thunk_regs(
    process: &Process,
    start: &UserRegs,
    stack_top: u64,
    thunk: u64,
    ntdll: u64,
) -> Result<UserRegs, NtStatus> {
    let mut entry = *start;
    let context_addr = (stack_top - context::SIZE as u64) & !0xf;
    entry.set_sp(context_addr);
    let reset = [0u8; context::FP_STATE_BYTES];
    process
        .write(context_addr, &context::to_bytes(&context_regs(&entry), Some(&reset)))
        .map_err(|_| NtStatus::NO_MEMORY)?;
    let mut regs = call_regs(thunk, context_addr, context_addr);
    regs.set_gpr(Gpr::X1, ntdll);
    Ok(regs)
}

/// What a new thread's CONTEXT says (NtCreateThread): its registers and
/// the start address ThreadQuerySetWin32StartAddress reports: as x64's
/// rule, the return register (x0, where kernel32's thunk gets the Win32
/// start routine), else its Pc. Its floating-point part is ignored: a new
/// thread starts with the reset FP/SIMD state, as on x64.
pub fn thread_start(ctx: &[u8]) -> (UserRegs, u64) {
    let regs = user_regs(&context::from_bytes(ctx));
    let x0 = regs.gpr(Gpr::X0);
    (regs, if x0 != 0 { x0 } else { regs.ip() })
}

/// gala's ARM64 dispatcher frame on the user stack: the CONTEXT at the new
/// sp, the EXCEPTION_RECORD right above it, the whole 16-byte aligned.
/// gala's ntdll's KiUserExceptionDispatcher reads it so (the frame of
/// Windows' own ARM64 dispatcher is not public).
const RECORD_OFFSET: u64 = context::SIZE as u64;
const RECORD_SIZE: u64 = 0x98;
const DISPATCH_FRAME_SIZE: u64 = (RECORD_OFFSET + RECORD_SIZE + 0xf) & !0xf;

/// EXCEPTION_RECORD (the 64-bit layout) for `status` raised at `address`.
fn exception_record(status: NtStatus, address: u64, info: &[u64]) -> [u8; RECORD_SIZE as usize] {
    let mut r = [0u8; RECORD_SIZE as usize];
    r[0..4].copy_from_slice(&status.0.to_le_bytes());
    r[0x10..0x18].copy_from_slice(&address.to_le_bytes());
    r[0x18..0x1c].copy_from_slice(&(info.len() as u32).to_le_bytes());
    for (i, v) in info.iter().enumerate() {
        r[0x20 + 8 * i..0x28 + 8 * i].copy_from_slice(&v.to_le_bytes());
    }
    r
}

/// An access violation's kind as EXCEPTION_RECORD's first parameter says
/// it: 0 read, 1 write, 8 execute (an instruction abort).
fn access_kind(frame: &TrapFrame) -> u64 {
    match frame.access() {
        platform::FaultAccess::Execute => 8,
        platform::FaultAccess::Write => 1,
        platform::FaultAccess::Read => 0,
    }
}

/// Hands a user-mode exception to ntdll!KiUserExceptionDispatcher, if the
/// process has one: the CONTEXT of the fault (CONTEXT_FULL, its pc the
/// faulting instruction's: a BRK's too, which its handler steps over) and
/// the EXCEPTION_RECORD go on the user stack (`DISPATCH_FRAME_SIZE`) and
/// the thread resumes in the dispatcher with sp at the CONTEXT. False if
/// it cannot be delivered.
pub fn deliver_to_user(
    process: &Process,
    frame: &mut TrapFrame,
    status: NtStatus,
    cr2: Option<u64>,
) -> bool {
    let dispatcher = process.exception_dispatcher.load(core::sync::atomic::Ordering::Relaxed);
    if dispatcher == 0 {
        return false;
    }
    let pc = frame.ip();
    let info: &[u64] = match status {
        NtStatus::BREAKPOINT => &[0],
        NtStatus::ACCESS_VIOLATION | NtStatus::GUARD_PAGE_VIOLATION | NtStatus::STACK_OVERFLOW => {
            &[access_kind(frame), cr2.unwrap_or(u64::MAX)]
        }
        _ => &[],
    };
    keep_note(process, frame, status, pc, cr2);
    let record = exception_record(status, pc, info);
    let state = platform::xsave();
    let mut fp = [0u8; context::FP_STATE_BYTES];
    fp.copy_from_slice(state.bytes());
    let ctx = context::to_bytes(&context_regs(&frame.user_regs()), Some(&fp));
    let stack = frame.sp();
    let at = stack.wrapping_sub(DISPATCH_FRAME_SIZE) & !0xf;
    let pushed = at < stack
        && process.write(at, &ctx).is_ok()
        && process.write(at + RECORD_OFFSET, &record).is_ok();
    if !pushed {
        warn!("user exception: cannot push the dispatcher frame (sp {stack:#x})");
        return false;
    }
    frame.set_ip(dispatcher);
    frame.set_sp(at);
    true
}

/// Keeps the exception for the process's crash report: its registers at
/// `ip`, and for an access violation the address and the access kind.
pub fn keep_note(
    process: &Process,
    frame: &TrapFrame,
    status: NtStatus,
    ip: u64,
    cr2: Option<u64>,
) {
    let access = matches!(
        status,
        NtStatus::ACCESS_VIOLATION | NtStatus::GUARD_PAGE_VIOLATION | NtStatus::STACK_OVERFLOW
    )
    .then(|| (cr2.unwrap_or(u64::MAX), access_kind(frame)));
    let mut regs = frame.user_regs();
    regs.set_ip(ip);
    *process.last_exception.lock() = Some(Note::new(process, status, access, regs, false));
}

/// The crash report's register lines (x0-x30, sp, pc, pstate) and the top
/// of the stack (`stack`, already named by module).
pub fn report_registers<T: core::fmt::Display>(note: &Note, stack: &[Option<T>]) {
    let w = note.regs.to_words();
    /// x`first`.. and their values, as one line.
    struct Row<'a>(usize, &'a [u64]);
    impl core::fmt::Display for Row<'_> {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            for (k, v) in self.1.iter().enumerate() {
                let sep = if k == 0 { "" } else { " " };
                write!(f, "{sep}x{:<2}={v:#018x}", self.0 + k)?;
            }
            Ok(())
        }
    }
    for (row, values) in w[..31].chunks(4).enumerate() {
        info!("crash report:   {}", Row(4 * row, values));
    }
    info!("crash report:   sp ={:#018x} pc ={:#018x} pstate={:#010x}", w[31], w[32], w[33]);
    for (i, v) in stack.iter().enumerate() {
        match v {
            Some(v) => info!("crash report:   [sp+{:#04x}] {v}", 8 * i),
            None => {
                info!("crash report:   [sp+{:#04x}] (unreadable)", 8 * i);
                break;
            }
        }
    }
}

/// The page's machine fields (ARM64) and processor features: the
/// ProcessorFeatures[] entries Microsoft documents for ARM64
/// (IsProcessorFeaturePresent's PF_ARM_*), those every ARMv8-A processor
/// has always set, the optional ones from ID_AA64ISAR0/1_EL1.
pub fn write_kuser_processor(page: &SharedPage) {
    use crate::nt::kuser::{IMAGE_NUMBER_HIGH, IMAGE_NUMBER_LOW, NATIVE_PROCESSOR_ARCHITECTURE};
    const IMAGE_FILE_MACHINE_ARM64: u16 = 0xaa64;
    const PROCESSOR_ARCHITECTURE_ARM64: u16 = 12;
    const PROCESSOR_FEATURES: usize = 0x274;
    page.write(IMAGE_NUMBER_LOW, &IMAGE_FILE_MACHINE_ARM64.to_le_bytes());
    page.write(IMAGE_NUMBER_HIGH, &IMAGE_FILE_MACHINE_ARM64.to_le_bytes());
    page.write(NATIVE_PROCESSOR_ARCHITECTURE, &PROCESSOR_ARCHITECTURE_ARM64.to_le_bytes());
    let (isar0, isar1) = crate::nt::platform::id_isar();
    let field = |reg: u64, at: u32| (reg >> at) & 0xf;
    for (pf, present) in
        arm64_features(field(isar0, 4), field(isar0, 16), field(isar0, 20)).into_iter().chain([
            (PF_ARM_V82_DP_INSTRUCTIONS_AVAILABLE, field(isar0, 44) != 0),
            (PF_ARM_V83_JSCVT_INSTRUCTIONS_AVAILABLE, field(isar1, 12) != 0),
            (PF_ARM_V83_LRCPC_INSTRUCTIONS_AVAILABLE, field(isar1, 20) != 0),
        ])
    {
        if present {
            page.write(PROCESSOR_FEATURES + pf, &[1]);
        }
    }
}

// ProcessorFeatures[] indices (PF_* in winnt.h) for ARM64.
const PF_NX_ENABLED: usize = 12;
const PF_ARM_VFP_32_REGISTERS_AVAILABLE: usize = 18;
const PF_ARM_NEON_INSTRUCTIONS_AVAILABLE: usize = 19;
const PF_ARM_DIVIDE_INSTRUCTION_AVAILABLE: usize = 24;
const PF_ARM_64BIT_LOADSTORE_ATOMIC: usize = 25;
const PF_ARM_FMAC_INSTRUCTIONS_AVAILABLE: usize = 27;
const PF_ARM_V8_INSTRUCTIONS_AVAILABLE: usize = 29;
const PF_ARM_V8_CRYPTO_INSTRUCTIONS_AVAILABLE: usize = 30;
const PF_ARM_V8_CRC32_INSTRUCTIONS_AVAILABLE: usize = 31;
const PF_ARM_V81_ATOMIC_INSTRUCTIONS_AVAILABLE: usize = 34;
const PF_ARM_V82_DP_INSTRUCTIONS_AVAILABLE: usize = 43;
const PF_ARM_V83_JSCVT_INSTRUCTIONS_AVAILABLE: usize = 44;
const PF_ARM_V83_LRCPC_INSTRUCTIONS_AVAILABLE: usize = 45;

/// The features every ARMv8-A processor has, and those its ID fields
/// (ID_AA64ISAR0_EL1's AES, CRC32 and Atomic) say: (PF_ index, present).
fn arm64_features(aes: u64, crc32: u64, atomic: u64) -> [(usize, bool); 10] {
    [
        (PF_NX_ENABLED, true),
        (PF_ARM_VFP_32_REGISTERS_AVAILABLE, true),
        (PF_ARM_NEON_INSTRUCTIONS_AVAILABLE, true),
        (PF_ARM_DIVIDE_INSTRUCTION_AVAILABLE, true),
        (PF_ARM_64BIT_LOADSTORE_ATOMIC, true),
        (PF_ARM_FMAC_INSTRUCTIONS_AVAILABLE, true),
        (PF_ARM_V8_INSTRUCTIONS_AVAILABLE, true),
        (PF_ARM_V8_CRYPTO_INSTRUCTIONS_AVAILABLE, aes != 0),
        (PF_ARM_V8_CRC32_INSTRUCTIONS_AVAILABLE, crc32 != 0),
        // LSE atomics: Atomic 0b0010.
        (PF_ARM_V81_ATOMIC_INSTRUCTIONS_AVAILABLE, atomic >= 2),
    ]
}

pub fn processor() -> Processor {
    Processor { name: [0; 48], vendor: [0; 12], family: 0, model: 0, stepping: 0, mhz: 0 }
}

/// NtGetContextThread's access right.
const THREAD_GET_CONTEXT: u32 = 0x0008;

/// Whether `object` is the calling thread.
fn is_current_thread(object: &ObjectRef) -> bool {
    crate::nt::thread::current().is_some_and(|me| crate::mm::heap::KArc::ptr_eq(&me, object))
}

/// NtGetContextThread(ThreadHandle, *Context): the parts of the user
/// registers ContextFlags names, of the caller (as it returns from the
/// call) or of a suspended thread once it has stopped.
pub(in crate::nt) fn nt_get_context_thread(call: &mut Call<'_>) -> NtStatus {
    let handle = match call.arg(0) {
        Ok(v) => v,
        Err(status) => return status,
    };
    let ptr = match call.arg(1) {
        Ok(v) => v,
        Err(status) => return status,
    };
    let object = match thread_object(call, handle, THREAD_GET_CONTEXT) {
        Ok(o) => o,
        Err(status) => return status,
    };
    let mut ctx = [0u8; context::SIZE];
    if call.process.read(ptr, &mut ctx).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let (regs, fx) = if is_current_thread(&object) {
        (caller_regs(call), platform::xsave())
    } else {
        match crate::nt::thread::parked_state(&object) {
            Ok(p) => (p.regs, p.fx),
            Err(status) => return status,
        }
    };
    let mut fp = [0u8; context::FP_STATE_BYTES];
    fp.copy_from_slice(fx.bytes());
    let flags = context::flags(&ctx);
    context::fill(&mut ctx, flags, &context_regs(&regs), &fp);
    if call.process.write(ptr, &ctx).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    NtStatus::SUCCESS
}

/// NtSetContextThread(ThreadHandle, *Context): the parts ContextFlags
/// names; for the caller they take effect on return (as NtContinue's), for
/// a suspended thread when it resumes. x18 stays the TEB whatever the
/// CONTEXT says (`syscall::for_user`).
pub(in crate::nt) fn nt_set_context_thread(call: &mut Call<'_>) -> NtStatus {
    let handle = match call.arg(0) {
        Ok(v) => v,
        Err(status) => return status,
    };
    let ptr = match call.arg(1) {
        Ok(v) => v,
        Err(status) => return status,
    };
    let object = match thread_object(call, handle, THREAD_SET_CONTEXT) {
        Ok(o) => o,
        Err(status) => return status,
    };
    let mut ctx = [0u8; context::SIZE];
    if call.process.read(ptr, &mut ctx).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let flags = context::flags(&ctx);
    let fp = context::float_state(&ctx);
    if is_current_thread(&object) {
        let mut regs = context_regs(&caller_regs(call));
        context::apply(&mut regs, &ctx, flags);
        let regs = user_regs(&regs);
        if !regs.is_valid() {
            return NtStatus::INVALID_PARAMETER;
        }
        if let Some(fp) = fp {
            platform::xrstor(&xstate_of(&fp));
        }
        platform::continue_with(regs);
        return NtStatus::SUCCESS;
    }
    let parked = match crate::nt::thread::parked_state(&object) {
        Ok(p) => p,
        Err(status) => return status,
    };
    let mut regs = context_regs(&parked.regs);
    context::apply(&mut regs, &ctx, flags);
    let regs = user_regs(&regs);
    if !regs.is_valid() {
        return NtStatus::INVALID_PARAMETER;
    }
    match crate::nt::thread::set_parked_state(&object, regs, fp.map(|fp| xstate_of(&fp))) {
        Ok(()) => NtStatus::SUCCESS,
        Err(e) => e,
    }
}

/// NtContinue(Context, TestAlert): the thread resumes with the parts of
/// `Context` its flags name over the registers it called with (the
/// floating-point part loaded now: the kernel does not use those
/// registers, so they are what the thread resumes with). x18 stays the
/// TEB whatever the CONTEXT's X18 says (`syscall::for_user`). With
/// TestAlert, a queued user APC runs first (`deliver_apc`), the resulting
/// context its frame's.
pub(in crate::nt) fn nt_continue(call: &mut Call<'_>) -> NtStatus {
    let ctx_ptr = match call.arg(0) {
        Ok(v) => v,
        Err(status) => return status,
    };
    let mut ctx = [0u8; context::SIZE];
    if call.process.read(ctx_ptr, &mut ctx).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let flags = context::flags(&ctx);
    if !context::has(flags, context::CONTEXT_ARM64) {
        return NtStatus::INVALID_PARAMETER;
    }
    let mut regs = context_regs(&call.frame.return_regs());
    context::apply(&mut regs, &ctx, flags);
    let regs = user_regs(&regs);
    if !regs.is_valid() {
        return NtStatus::INVALID_PARAMETER;
    }
    let fp = context::float_state(&ctx);
    let test_alert = match call.arg(1) {
        Ok(v) => v as u8 != 0,
        Err(status) => return status,
    };
    if test_alert
        && crate::nt::thread::has_user_apc()
        && deliver_apc(call, &regs, &fp.unwrap_or_else(current_fp), false)
    {
        return NtStatus::SUCCESS;
    }
    if let Some(fp) = fp {
        platform::xrstor(&xstate_of(&fp));
    }
    platform::continue_with(regs);
    NtStatus::SUCCESS
}

/// GalaUnhandledException(Record, Context): gala's ntdll reports an
/// exception no handler took before the top-level filter runs (which may
/// end the process with a status of its own), so the crash report still
/// describes it; a null record withdraws it (the filter continued
/// execution). As x64's, from ARM64's CONTEXT; a C++ throw's and a
/// delay load's details are x64's only yet.
pub(in crate::nt) fn gala_unhandled_exception(call: &mut Call<'_>) -> NtStatus {
    let record = match call.arg(0) {
        Ok(v) => v,
        Err(status) => return status,
    };
    let context = match call.arg(1) {
        Ok(v) => v,
        Err(status) => return status,
    };
    let walk = match call.arg(2) {
        Ok(v) => v,
        Err(status) => return status,
    };
    if record == 0 {
        *call.process.unhandled_exception.lock() = None;
        return NtStatus::SUCCESS;
    }
    // EXCEPTION_RECORD: code +0, flags +4, address +0x10, parameter count
    // +0x18, parameters +0x20.
    let mut rec = [0u8; RECORD_SIZE as usize];
    let mut ctx = [0u8; context::SIZE];
    if call.process.read(record, &mut rec).is_err() || call.process.read(context, &mut ctx).is_err()
    {
        return NtStatus::ACCESS_VIOLATION;
    }
    let word = |at: usize| u64::from_le_bytes(rec[at..at + 8].try_into().unwrap_or([0; 8]));
    let code = word(0) as u32;
    let count = word(0x18) as u32;
    let access =
        (code == NtStatus::ACCESS_VIOLATION.0 && count >= 2).then(|| (word(0x28), word(0x20)));
    let mut regs = user_regs(&context::from_bytes(&ctx));
    regs.set_ip(word(0x10));
    let mut note = Note::new(&call.process, NtStatus(code), access, regs, false);
    note.flags = (word(0) >> 32) as u32;
    note.walk = crate::nt::abi::walk_of(call, walk);
    // The parameters: RECORD_SIZE has room for all 15 of them.
    note.params_count = count as usize;
    note.params_len = note.params_count.min(crate::nt::crashreport::PARAMS);
    let kept = note.params_len;
    for (i, p) in note.params.iter_mut().take(kept).enumerate() {
        *p = word(0x20 + 8 * i);
    }
    *call.process.unhandled_exception.lock() = Some(note);
    NtStatus::SUCCESS
}

/// After an alertable wait that ended with STATUS_USER_APC: the thread
/// runs its APCs (`deliver_apc`) and then returns from the wait with that
/// status (x0 of the CONTEXT it resumes).
pub(in crate::nt) fn deliver_apc_after(call: &Call<'_>, status: NtStatus) -> NtStatus {
    if status != NtStatus::USER_APC {
        return status;
    }
    let mut regs = call.frame.return_regs();
    regs.set_syscall_result(u64::from(status.0));
    deliver_apc(call, &regs, &current_fp(), false);
    status
}

/// A special user APC (QueueUserAPC2) on the way back to user mode from
/// a system call that returned `status`: the thread runs it and the
/// CONTEXT it resumes returns `status` from the call, as it would have.
pub(in crate::nt) fn deliver_special_apc(call: &Call<'_>, status: NtStatus) {
    let mut regs = call.frame.return_regs();
    regs.set_syscall_result(u64::from(status.0));
    deliver_apc(call, &regs, &current_fp(), true);
}

/// The running thread's FP/SIMD state as CONTEXT's floating-point part.
fn current_fp() -> [u8; context::FP_STATE_BYTES] {
    let mut fp = [0u8; context::FP_STATE_BYTES];
    fp.copy_from_slice(platform::xsave().bytes());
    fp
}

/// gala's ARM64 APC frame: the CONTEXT to resume at sp, then
/// NormalRoutine, NormalContext, SystemArgument1 and SystemArgument2 right
/// above it (ARM64's CONTEXT has no home area to hold them, as x64's
/// does), the whole 16-byte aligned. gala's ntdll's KiUserApcDispatcher
/// reads it so.
const APC_ARGS_OFFSET: u64 = context::SIZE as u64;
const APC_FRAME_SIZE: u64 = APC_ARGS_OFFSET + 32;

/// Runs the next user APC, if any, by resuming the thread in
/// ntdll!KiUserApcDispatcher with the APC frame below `regs`' sp: the
/// dispatcher calls the routine and NtContinues the CONTEXT (`regs` and
/// `fp`) with TestAlert set, which delivers the next APC.
pub(in crate::nt) fn deliver_apc(
    call: &Call<'_>,
    regs: &UserRegs,
    fp: &[u8; context::FP_STATE_BYTES],
    special_only: bool,
) -> bool {
    let dispatcher = call.process.apc_dispatcher.load(core::sync::atomic::Ordering::Relaxed);
    if dispatcher == 0 {
        return false;
    }
    let Some(apc) = crate::nt::thread::take_user_apc(special_only) else { return false };
    let ctx = context::to_bytes(&context_regs(regs), Some(fp));
    let mut args = [0u8; 32];
    for (i, v) in [apc.routine, apc.args[0], apc.args[1], apc.args[2]].iter().enumerate() {
        args[8 * i..8 * i + 8].copy_from_slice(&v.to_le_bytes());
    }
    let sp = regs.sp();
    let frame = sp.wrapping_sub(APC_FRAME_SIZE) & !0xf;
    if frame >= sp
        || call.process.write(frame, &ctx).is_err()
        || call.process.write(frame + APC_ARGS_OFFSET, &args).is_err()
    {
        return false;
    }
    let mut next = UserRegs::default();
    next.set_sp(frame);
    next.set_ip(dispatcher);
    platform::continue_with(next);
    true
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
