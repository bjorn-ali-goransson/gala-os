//! Windows' x64 ABI: what the NT layer does in terms of x86-64's
//! registers. The CONTEXT record (`context`), where a system call's
//! arguments are (r10, rdx, r8, r9, then the user stack above the return
//! address and the shadow area), how a thread starts (rcx = its argument,
//! rsp as if called), the frames exception and APC dispatch push for
//! ntdll's dispatchers, the CONTEXT services (NtGetContextThread,
//! NtSetContextThread, NtContinue, NtCreateThread's context), WOW64's
//! entry and the crash report's registers.

pub mod context;
mod kuser;

pub use kuser::write_kuser_processor;

use core::sync::atomic::Ordering;

use super::ArgSlot;
use crate::mm::heap::KArc;
use crate::nt::crashreport::Note;
use crate::nt::object::{ObjectBody, ObjectRef};
use crate::nt::platform::{self, Gpr, Gpr::*, SyscallFrame, TrapFrame, UserRegs};
use crate::nt::process::Process;
use crate::nt::registry::Processor;
use crate::nt::status::NtStatus;
use crate::nt::syscalls::{Call, THREAD_SET_CONTEXT, caller_regs, thread_object};
use crate::nt::thread;

const THREAD_GET_CONTEXT: u32 = 0x0008;

fn is_current_thread(object: &ObjectRef) -> bool {
    thread::current().is_some_and(|me| KArc::ptr_eq(&me, object))
}

/// Evaluates a `Result<_, NtStatus>`, returning the error status.
macro_rules! tri {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(status) => return status,
        }
    };
}

macro_rules! arg {
    ($call:expr, $i:expr) => {
        match $call.arg($i) {
            Ok(v) => v,
            Err(status) => return status,
        }
    };
}

/// Where system call argument `i` (0-based) of the call in `frame` is:
/// r10, rdx, r8, r9, then the user stack above the return address and
/// the 32-byte shadow area.
pub fn syscall_arg(frame: &SyscallFrame, i: usize) -> ArgSlot {
    match i {
        0 => ArgSlot::Value(frame.gpr(R10)),
        1 => ArgSlot::Value(frame.gpr(Rdx)),
        2 => ArgSlot::Value(frame.gpr(R8)),
        3 => ArgSlot::Value(frame.gpr(R9)),
        _ => ArgSlot::Stack(frame.sp() + 0x28 + 8 * (i as u64 - 4)),
    }
}

/// The same for the registers a thread made a system call with (as the
/// call returns with them: `thread::set_in_syscall`'s, a handover's).
pub fn saved_arg(regs: &UserRegs, i: usize) -> ArgSlot {
    match i {
        0 => ArgSlot::Value(regs.gpr(R10)),
        1 => ArgSlot::Value(regs.gpr(Rdx)),
        2 => ArgSlot::Value(regs.gpr(R8)),
        3 => ArgSlot::Value(regs.gpr(R9)),
        _ => ArgSlot::Stack(regs.sp() + 0x28 + 8 * (i as u64 - 4)),
    }
}

/// Where the system call stub a thread is in (registers `regs`, as
/// `saved_arg` takes them) was called from: the return address on top of
/// the user stack.
pub fn stub_return_address(process: &Process, regs: &UserRegs) -> Option<u64> {
    process.read_u64(regs.sp()).ok()
}

/// The registers that call `entry(arg)` on a stack whose top is
/// `stack_top`: rcx = `arg`, rsp as if called (rsp % 16 == 8, the
/// 32-byte shadow area above the null return address).
pub fn call_regs(entry: u64, arg: u64, stack_top: u64) -> UserRegs {
    let mut gpr = [0u64; 16];
    gpr[Rcx as usize] = arg;
    gpr[Rsp as usize] = stack_top - 0x28;
    UserRegs::new(gpr, entry, 0)
}

/// The start through ntdll: LdrInitializeThunk (`thunk`) with rcx = the
/// CONTEXT of `start`, written at the top of its stack, and rdx = ntdll's
/// base `ntdll`; `start`'s rsp is moved below the CONTEXT.
pub fn ldr_thunk_regs(
    process: &Process,
    start: &UserRegs,
    stack_top: u64,
    thunk: u64,
    ntdll: u64,
) -> Result<UserRegs, NtStatus> {
    let mut entry = *start;
    let context_addr = (stack_top - context::SIZE as u64) & !0xf;
    entry.set_sp(context_addr - 0x28);
    process
        .write(context_addr, &context::to_bytes(&entry, None))
        .map_err(|_| NtStatus::NO_MEMORY)?;
    let mut gpr = [0u64; 16];
    gpr[Rcx as usize] = context_addr;
    gpr[Rdx as usize] = ntdll;
    gpr[Rsp as usize] = entry.sp() - 0x30; // call-like alignment, shadow space
    Ok(UserRegs::new(gpr, thunk, 0))
}

/// What a new thread's CONTEXT says (NtCreateThread): its registers and
/// the start address ThreadQuerySetWin32StartAddress reports. Its
/// floating-point part (MxCsr, FltSave) is ignored: a new thread starts
/// with a fresh x87/SSE state, MXCSR 0x1f80 and XMM 0, whatever the
/// CONTEXT and its flags say (Windows 11, fpu_test).
/// As ReactOS does: the start address is the "return register" (kernel32's
/// thunks pass the Win32 start routine there); a context without it
/// reports its start rip (never 0: the thread's events and
/// ThreadQuerySetWin32StartAddress name where it starts).
pub fn thread_start(ctx: &[u8]) -> (UserRegs, u64) {
    let regs = context::from_bytes(ctx);
    let win32_start = if regs.gpr(Rax) != 0 { regs.gpr(Rax) } else { regs.ip() };
    (regs, win32_start)
}

/// A trap in 32-bit code (CS 0x23: a WOW64 thread's).
fn is_compat(frame: &TrapFrame) -> bool {
    frame.segments().0 == u64::from(crate::nt::layout::wow64::CS32)
}

/// An access violation's kind as EXCEPTION_RECORD's first parameter says
/// it: 0 read, 1 write, 8 execute (DEP).
fn access_kind(frame: &TrapFrame) -> u64 {
    match frame.access() {
        platform::FaultAccess::Execute => 8,
        platform::FaultAccess::Write => 1,
        platform::FaultAccess::Read => 0,
    }
}

/// The crash report's register lines: x64's, or for 32-bit code (WOW64)
/// x86's in their low halves, then the top of the stack (`stack`, already
/// named by module).
pub fn report_registers<T: core::fmt::Display>(note: &Note, stack: &[Option<T>]) {
    let r = note.regs.gprs();
    if note.wow64 {
        let r = r.map(|v| v as u32);
        info!(
            "crash report:   eax={:#010x} ebx={:#010x} ecx={:#010x} edx={:#010x}",
            r[0], r[3], r[1], r[2]
        );
        info!(
            "crash report:   esi={:#010x} edi={:#010x} ebp={:#010x} esp={:#010x}",
            r[6], r[7], r[5], r[4]
        );
        info!("crash report:   eip={:#010x}", note.regs.ip() as u32);
        for (i, v) in stack.iter().enumerate() {
            match v {
                Some(v) => info!("crash report:   [esp+{:#04x}] {v}", 4 * i),
                None => {
                    info!("crash report:   [esp+{:#04x}] (unreadable)", 4 * i);
                    break;
                }
            }
        }
        return;
    }
    info!(
        "crash report:   rax={:#018x} rbx={:#018x} rcx={:#018x} rdx={:#018x}",
        r[0], r[3], r[1], r[2]
    );
    info!(
        "crash report:   rsi={:#018x} rdi={:#018x} rbp={:#018x} rsp={:#018x}",
        r[6], r[7], r[5], r[4]
    );
    info!(
        "crash report:   r8 ={:#018x} r9 ={:#018x} r10={:#018x} r11={:#018x}",
        r[8], r[9], r[10], r[11]
    );
    info!(
        "crash report:   r12={:#018x} r13={:#018x} r14={:#018x} r15={:#018x}",
        r[12], r[13], r[14], r[15]
    );
    for (i, v) in stack.iter().enumerate() {
        match v {
            Some(v) => info!("crash report:   [rsp+{:#04x}] {v}", 8 * i),
            None => {
                info!("crash report:   [rsp+{:#04x}] (unreadable)", 8 * i);
                break;
            }
        }
    }
}

/// Where the CONTEXT at `ctx_ptr` (with CONTEXT_XSTATE) has its XSTATE
/// area, from the CONTEXT_EX after it: (address, length), the length at
/// most what gala reads or writes. None without extended features or with
/// a CONTEXT_EX that does not describe one.
pub(in crate::nt) fn user_xstate_area(
    call: &Call<'_>,
    ctx_ptr: u64,
    flags: u32,
) -> Option<(u64, usize)> {
    use self::context::{self as ctx, CONTEXT_XSTATE};
    if !ctx::has(flags, CONTEXT_XSTATE) || ctx::xstate_len() == 0 {
        return None;
    }
    let mut ex = [0u8; ctx::EX_SIZE];
    call.process.read(ctx_ptr.checked_add(ctx::SIZE as u64)?, &mut ex).ok()?;
    let (offset, len) = ctx::xstate_area(&ex)?;
    Some((ctx_ptr.checked_add(offset)?, len.min(ctx::xstate_len())))
}

/// The x87/SSE/AVX state a thread resumes with when given the CONTEXT
/// `ctx` (read from `ctx_ptr`): `base` with the CONTEXT's FltSave
/// (CONTEXT_FLOATING_POINT) and XSTATE area (CONTEXT_XSTATE) applied.
/// Whether anything was applied, too.
pub(in crate::nt) fn resume_state(
    call: &Call<'_>,
    ctx_ptr: u64,
    ctx: &[u8],
    mut state: platform::XState,
) -> Result<(platform::XState, bool), NtStatus> {
    use self::context as c;
    let mut changed = false;
    if let Some(legacy) = c::float_state(ctx) {
        state.set_legacy(&legacy);
        changed = true;
    }
    if let Some((addr, len)) = user_xstate_area(call, ctx_ptr, c::flags(ctx)) {
        let mut area = [0u8; c::MAX_XSTATE];
        let area = &mut area[..len];
        call.process.read(addr, area).map_err(|_| NtStatus::ACCESS_VIOLATION)?;
        c::apply_xstate(&mut state, area);
        changed = true;
    }
    Ok((state, changed))
}

/// NtGetContextThread(ThreadHandle, *Context): the parts of the user
/// registers that ContextFlags names, of the caller or of a suspended
/// thread once it has stopped; with CONTEXT_XSTATE, the features the
/// XSTATE area's Mask asks for (the Mask then says which of them hold
/// values, the rest being in their initial state).
pub(in crate::nt) fn nt_get_context_thread(call: &mut Call<'_>) -> NtStatus {
    use self::context as c;
    let object = tri!(thread_object(call, arg!(call, 0), THREAD_GET_CONTEXT));
    let ptr = arg!(call, 1);
    let mut ctx = [0u8; c::SIZE];
    if call.process.read(ptr, &mut ctx).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let flags = c::flags(&ctx);
    let (regs, fx, compat) = if is_current_thread(&object) {
        (caller_regs(call), platform::xsave(), false)
    } else {
        let p = tri!(thread::parked_state(&object));
        (p.regs, p.fx, p.compat)
    };
    c::fill(&mut ctx, flags, &regs, Some(&fx));
    // A WOW64 thread stopped in 32-bit code: its selectors (CS 0x23, FS
    // the TEB32's), as Windows reports them.
    if compat {
        c::mark_compat(&mut ctx, flags);
    }
    if call.process.write(ptr, &ctx).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    if let Some((addr, len)) = user_xstate_area(call, ptr, flags) {
        let Ok(wanted) = call.process.read_u64(addr) else { return NtStatus::ACCESS_VIOLATION };
        let area = c::xstate_bytes(&fx, wanted, len);
        if call.process.write(addr, &area[..len]).is_err() {
            return NtStatus::ACCESS_VIOLATION;
        }
    }
    NtStatus::SUCCESS
}

/// NtSetContextThread(ThreadHandle, *Context): the parts ContextFlags
/// names; for the caller it takes effect on return (as NtContinue), for
/// a suspended thread on resume. CONTEXT_XSTATE: the features in the
/// XSTATE area's Mask take its values, the others their initial state.
pub(in crate::nt) fn nt_set_context_thread(call: &mut Call<'_>) -> NtStatus {
    use self::context as c;
    let object = tri!(thread_object(call, arg!(call, 0), THREAD_SET_CONTEXT));
    let ptr = arg!(call, 1);
    let mut ctx = [0u8; c::SIZE];
    if call.process.read(ptr, &mut ctx).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let flags = c::flags(&ctx);
    if is_current_thread(&object) {
        let mut regs = caller_regs(call);
        c::apply(&mut regs, &ctx, flags);
        if !regs.is_valid() {
            return NtStatus::INVALID_PARAMETER;
        }
        let (state, changed) = tri!(resume_state(call, ptr, &ctx, platform::xsave()));
        if changed {
            platform::xrstor(&state);
        }
        platform::continue_with(regs);
        return NtStatus::SUCCESS;
    }
    let parked = tri!(thread::parked_state(&object));
    let mut regs = parked.regs;
    c::apply(&mut regs, &ctx, flags);
    if !regs.is_valid() {
        return NtStatus::INVALID_PARAMETER;
    }
    let (state, changed) = tri!(resume_state(call, ptr, &ctx, parked.fx));
    match thread::set_parked_state(&object, regs, changed.then_some(state)) {
        Ok(()) => NtStatus::SUCCESS,
        Err(e) => e,
    }
}

/// After an alertable wait ended with STATUS_USER_APC, makes the system
/// call return through ntdll!KiUserApcDispatcher to run the first queued
/// APC; the CONTEXT it gets resumes the caller with rax = STATUS_USER_APC.
pub(in crate::nt) fn deliver_apc_after(call: &Call<'_>, status: NtStatus) -> NtStatus {
    if status != NtStatus::USER_APC {
        return status;
    }
    // SYSCALL left the return address in rcx and the flags in r11.
    let mut regs = call.frame.return_regs();
    regs.set_syscall_result(u64::from(status.0));
    let state = platform::xsave();
    let context = self::context::to_bytes(&regs, Some(&state));
    deliver_apc(call, context, call.frame.sp(), &state);
    status
}

/// Where an APC frame has its XSTATE area: after the CONTEXT and its
/// CONTEXT_EX, 64-byte aligned (the frame is).
const APC_XSTATE_AT: usize = 0x500;

/// Runs the next user APC, if any, by resuming the thread in
/// ntdll!KiUserApcDispatcher with `context` (the state to return to, with
/// stack pointer `rsp`, and extended state `state`) at the top of a new
/// frame below `rsp`. The x64 contract (Windows Internals): rsp points to
/// the CONTEXT, whose P1Home..P4Home hold NormalContext, SystemArgument1,
/// SystemArgument2 and NormalRoutine; the dispatcher calls the routine and
/// NtContinues the CONTEXT with TestAlert set, which delivers the next
/// APC. With extended features the CONTEXT has CONTEXT_XSTATE, its
/// CONTEXT_EX after it and the XSTATE area at APC_XSTATE_AT, so the
/// routine's use of the YMM registers does not reach the resumed code.
// TODO: verify the frame against the pinned build's ntdll.
pub(in crate::nt) fn deliver_apc(
    call: &Call<'_>,
    mut context: [u8; self::context::SIZE],
    rsp: u64,
    state: &platform::XState,
) -> bool {
    use self::context as c;
    let dispatcher = call.process.apc_dispatcher.load(Ordering::Relaxed);
    if dispatcher == 0 {
        return false;
    }
    let Some(apc) = thread::take_user_apc() else { return false };
    for (i, v) in [apc.args[0], apc.args[1], apc.args[2], apc.routine].iter().enumerate() {
        context[8 * i..8 * i + 8].copy_from_slice(&v.to_le_bytes());
    }
    let xlen = c::xstate_len();
    let size = if xlen == 0 { c::SIZE } else { APC_XSTATE_AT + xlen };
    let frame = (rsp.wrapping_sub(size as u64)) & !0x3f;
    if xlen != 0 {
        let flags = c::flags(&context) | c::CONTEXT_XSTATE;
        c::set_flags(&mut context, flags);
    }
    if frame >= rsp || call.process.write(frame, &context).is_err() {
        return false;
    }
    if xlen != 0 {
        let ex = c::context_ex(APC_XSTATE_AT, xlen);
        let area = c::xstate_bytes(state, u64::MAX, xlen);
        if call.process.write(frame + c::SIZE as u64, &ex).is_err()
            || call.process.write(frame + APC_XSTATE_AT as u64, &area[..xlen]).is_err()
        {
            return false;
        }
    }
    let mut regs = UserRegs::default();
    regs.set_sp(frame);
    regs.set_ip(dispatcher);
    platform::continue_with(regs);
    true
}

/// NtContinue(*Context, TestAlert): resumes the calling thread with the
/// registers in `Context` (returned to user mode through IRETQ, since
/// SYSRET cannot restore rcx and r11).
pub(in crate::nt) fn nt_continue(call: &mut Call<'_>) -> NtStatus {
    let ctx_ptr = arg!(call, 0);
    let mut ctx = [0u8; self::context::SIZE];
    if call.process.read(ctx_ptr, &mut ctx).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let regs = self::context::from_bytes(&ctx);
    if !regs.is_valid() {
        return NtStatus::INVALID_PARAMETER;
    }
    let (state, changed) = tri!(resume_state(call, ctx_ptr, &ctx, platform::xsave()));
    // TestAlert: run the next queued user APC before resuming the context.
    if arg!(call, 1) as u8 != 0
        && thread::has_user_apc()
        && deliver_apc(call, ctx, regs.sp(), &state)
    {
        return NtStatus::SUCCESS;
    }
    // The kernel does not use x87/SSE/AVX, so the state loaded now is what
    // the thread resumes with (context switches save and restore it).
    if changed {
        platform::xrstor(&state);
    }
    platform::continue_with(regs);
    NtStatus::SUCCESS
}

/// NtCreateThread in a WOW64 process, where every thread runs 32-bit
/// code: wow64.dll converts its 32-bit caller's x86 CONTEXT and
/// INITIAL_TEB into the 64-bit ones, with SegCs 0x23 and the 32-bit
/// registers and stack zero-extended ("Plan: WOW64"). The thread gets a
/// 64-bit stack for wow64.dll's code, a TEB32, and starts in wow64.dll,
/// which enters 32-bit code with those registers.
pub(in crate::nt) fn create_wow64_thread(
    call: &Call<'_>,
    ctx: &[u8],
    (base, limit, bottom): (u64, u64, u64),
    suspended: bool,
) -> Result<ObjectRef, NtStatus> {
    use self::context as c;
    if u32::from(c::seg_cs(ctx)) != crate::nt::layout::wow64::CS32 {
        // A 64-bit thread would have no 64-bit ntdll to run.
        crate::nt::ledger::record("NtCreateThread", "64-bit thread in a WOW64 process", 0);
        return Err(NtStatus::INVALID_PARAMETER);
    }
    let regs = c::from_bytes(ctx);
    let low = |r: Gpr| regs.gpr(r) as u32;
    let stack64 = crate::nt::wow64::reserve_stack64(&mut call.process.vad.lock())?;
    let start = crate::nt::wow64::Start32 {
        eip: regs.ip() as u32,
        esp: low(Rsp),
        // eax, ecx, edx, ebx, esi, edi, ebp.
        gpr: [low(Rax), low(Rcx), low(Rdx), low(Rbx), low(Rsi), low(Rdi), low(Rbp)],
        stack: (base as u32, limit as u32, bottom as u32),
        stack64: stack64.0,
    };
    let created = thread::create_wow64(&call.process, start, stack64, suspended);
    if created.is_err() {
        let _ = crate::nt::vm::free(&call.process, stack64.2, 0, crate::nt::vm::MEM_RELEASE);
    }
    created
}

/// GalaUnhandledException(ExceptionRecord, ContextRecord): ntdll's
/// report of an exception no handler took, before it calls the top-level
/// filter; (0, 0) when the filter continued execution. Kept as the
/// process's crash report, printed when it ends (nt::crashreport).
pub(in crate::nt) fn gala_unhandled_exception(call: &mut Call<'_>) -> NtStatus {
    let (record, context) = (arg!(call, 0), arg!(call, 1));
    if record == 0 {
        *call.process.unhandled_exception.lock() = None;
        return NtStatus::SUCCESS;
    }
    // EXCEPTION_RECORD: code +0, address +0x10, parameter count +0x18,
    // parameters +0x20; CONTEXT: rax..r15 from +0x78.
    let code = tri!(call.read_u64(record)) as u32;
    let rip = tri!(call.read_u64(record + 0x10));
    let count = tri!(call.read_u64(record + 0x18)) as u32;
    let access = if code == NtStatus::ACCESS_VIOLATION.0 && count >= 2 {
        Some((tri!(call.read_u64(record + 0x28)), tri!(call.read_u64(record + 0x20))))
    } else {
        None
    };
    let mut gpr = [0u64; 16];
    for (i, r) in gpr.iter_mut().enumerate() {
        *r = tri!(call.read_u64(context + 0x78 + 8 * i as u64));
    }
    let regs = UserRegs::new(gpr, rip, 0);
    // From a 32-bit ntdll, wow64.dll passes the x86 registers in a 64-bit
    // CONTEXT that says CS 0x23 (the report shows them as 32-bit).
    let mut cs = [0u8; 2];
    tri!(call.process.read(context + 0x38, &mut cs).map_err(|_| NtStatus::ACCESS_VIOLATION));
    let x86 = u32::from(u16::from_le_bytes(cs)) == crate::nt::layout::wow64::CS32;
    let mut note =
        crate::nt::crashreport::Note::new(&call.process, NtStatus(code), access, regs, x86);
    // A C++ throw: what was thrown, read now (the object and the
    // runtime's type data are gone once the process has ended).
    if code == crate::nt::crashreport::CXX_EXCEPTION {
        let mut params = [0u64; 4];
        for (i, p) in params.iter_mut().enumerate().take(count.min(4) as usize) {
            *p = tri!(call.read_u64(record + 0x20 + 8 * i as u64));
        }
        let read = |addr: u64, out: &mut [u8]| call.process.read(addr, out).is_ok();
        let params = &params[..count.min(4) as usize];
        note.cxx = crate::nt::crashreport::CxxThrow::decode(params, x86, &read);
    }
    // A delay-load failure: the DLL and function it was looking for.
    use crate::nt::crashreport::{DELAY_DLL, DELAY_PROC, DelayLoad};
    if matches!(code, DELAY_DLL | DELAY_PROC) && count >= 1 {
        let info = tri!(call.read_u64(record + 0x20));
        let read = |addr: u64, out: &mut [u8]| call.process.read(addr, out).is_ok();
        note.delay = DelayLoad::decode(info, x86, &read);
    }
    note.flags = tri!(call.read_u64(record)).wrapping_shr(32) as u32;
    // 32-bit code: the SEH registrations the dispatcher walked (the list
    // is unchanged by a search that found no handler).
    if x86 && let Some((teb32, _, _)) = crate::nt::thread::current_stack32(&call.process) {
        let word = |at: u64| {
            let mut b = [0u8; 4];
            call.process.read(at, &mut b).ok().map(|()| u32::from_le_bytes(b))
        };
        note.seh_head = word(teb32);
        let mut frame = note.seh_head.unwrap_or(0);
        while note.seh_len < note.seh.len() {
            if frame == u32::MAX {
                note.seh_end = true;
                break;
            }
            let (Some(next), Some(handler)) = (word(u64::from(frame)), word(u64::from(frame) + 4))
            else {
                break;
            };
            note.seh[note.seh_len] = (frame, handler);
            note.seh_len += 1;
            frame = next;
        }
        note.seh_end |= frame == u32::MAX;
    }
    *call.process.unhandled_exception.lock() = Some(note);
    NtStatus::SUCCESS
}

/// Keeps the exception for a crash report (printed if it ends the
/// process).
pub(in crate::nt) fn keep_note(
    process: &Process,
    frame: &TrapFrame,
    status: NtStatus,
    rip: u64,
    cr2: Option<u64>,
) {
    let access = matches!(
        status,
        NtStatus::ACCESS_VIOLATION | NtStatus::GUARD_PAGE_VIOLATION | NtStatus::STACK_OVERFLOW
    )
    .then(|| (cr2.unwrap_or(u64::MAX), access_kind(frame)));
    let mut regs = frame.user_regs();
    regs.set_ip(rip);
    let note = crate::nt::crashreport::Note::new(process, status, access, regs, is_compat(frame));
    *process.last_exception.lock() = Some(note);
}

/// The dispatcher frame on the user stack: the CONTEXT at the new rsp,
/// the EXCEPTION_RECORD at `RECORD_OFFSET`, then room for a machine frame.
/// The layout follows the documented KiUserExceptionDispatcher contract as
/// far as it is public (rsp -> CONTEXT, record above it).
// TODO: check the offsets against the pinned build's ntdll.
const RECORD_OFFSET: u64 = 0x4f0;
const RECORD_SIZE: u64 = 0x98;
/// The machine frame (rip, cs, rflags, rsp, ss of the fault) above the
/// record: ntdll's dispatcher describes it in its unwind data
/// (UWOP_PUSH_MACHFRAME), so unwinding continues into the faulting frame.
const MACHINE_FRAME_OFFSET: u64 = 0x590;
const DISPATCH_FRAME_SIZE: u64 = MACHINE_FRAME_OFFSET + 0x30;
/// With extended features (AVX): the CONTEXT has CONTEXT_XSTATE, its
/// CONTEXT_EX follows it (at 0x4d0, before the record) and the XSTATE
/// area starts here, 64-byte aligned (the frame is).
const XSTATE_OFFSET: u64 = DISPATCH_FRAME_SIZE;

/// EXCEPTION_RECORD (x64) for `status` raised at `address`.
pub(in crate::nt) fn exception_record(
    status: NtStatus,
    address: u64,
    info: &[u64],
) -> [u8; RECORD_SIZE as usize] {
    let mut r = [0u8; RECORD_SIZE as usize];
    r[0..4].copy_from_slice(&status.0.to_le_bytes());
    r[0x10..0x18].copy_from_slice(&address.to_le_bytes());
    r[0x18..0x1c].copy_from_slice(&(info.len() as u32).to_le_bytes());
    for (i, v) in info.iter().enumerate() {
        r[0x20 + 8 * i..0x28 + 8 * i].copy_from_slice(&v.to_le_bytes());
    }
    r
}

/// MACHINE_FRAME: rip, cs, rflags, rsp, ss of the interrupted code.
pub(in crate::nt) fn machine_frame(rip: u64, frame: &TrapFrame) -> [u8; 40] {
    let mut m = [0u8; 40];
    let (cs, ss) = if is_compat(frame) {
        (frame.segments().0, u64::from(crate::nt::layout::wow64::DS32))
    } else {
        (self::context::USER_CS.into(), self::context::USER_DS.into())
    };
    let values = [rip, cs, frame.flags(), frame.sp(), ss];
    for (chunk, v) in m.chunks_exact_mut(8).zip(values) {
        chunk.copy_from_slice(&v.to_le_bytes());
    }
    m
}

/// Hands a user-mode exception to ntdll!KiUserExceptionDispatcher, if the
/// process has one: CONTEXT and EXCEPTION_RECORD go on the user stack and
/// the thread resumes in the dispatcher. False if it cannot be delivered.
///
/// A WOW64 thread's exception in 32-bit code (CS 0x23) goes to wow64.dll's
/// dispatcher (the process's) in 64-bit code, on the thread's 64-bit
/// stack, with the CONTEXT saying CS 0x23; wow64.dll converts it and hands
/// it to the 32-bit ntdll's dispatcher on the 32-bit stack. Without a
/// 32-bit ntdll it is not delivered.
pub(in crate::nt) fn deliver_to_user(
    process: &Process,
    frame: &mut TrapFrame,
    status: NtStatus,
    cr2: Option<u64>,
) -> bool {
    let dispatcher = process.exception_dispatcher.load(core::sync::atomic::Ordering::Relaxed);
    if dispatcher == 0 {
        return false;
    }
    let x86 = is_compat(frame);
    let stack = if x86 {
        let teb = thread::current().and_then(|t| match &t.body {
            ObjectBody::Thread(t) => Some(t.teb),
            _ => None,
        });
        match teb.and_then(|teb| crate::nt::wow64::exception_stack(process, teb)) {
            Some(top) => top,
            None => return false,
        }
    } else {
        frame.sp()
    };
    // Breakpoints report the int3 itself (the trap left rip after it).
    let rip = if status == NtStatus::BREAKPOINT { frame.breakpoint_address() } else { frame.ip() };
    let info: &[u64] = match status {
        NtStatus::BREAKPOINT => &[0],
        NtStatus::ACCESS_VIOLATION | NtStatus::GUARD_PAGE_VIOLATION | NtStatus::STACK_OVERFLOW => {
            &[access_kind(frame), cr2.unwrap_or(u64::MAX)]
        }
        _ => &[],
    };
    keep_note(process, frame, status, rip, cr2);
    let record = exception_record(status, rip, info);
    let mut regs = frame.user_regs();
    regs.set_ip(rip);
    use self::context as c;
    let state = platform::xsave();
    let mut context = c::to_bytes(&regs, Some(&state));
    if x86 {
        let flags = c::flags(&context);
        c::mark_compat(&mut context, flags);
    }
    let machine = machine_frame(rip, frame);
    // The frame without the XSTATE area if there is no room for it (a
    // stack overflow's last page: an AVX-512 area makes the frame 3.6 KiB).
    let push = |xlen: usize| -> Option<u64> {
        let mut context = context;
        let size = if xlen == 0 { DISPATCH_FRAME_SIZE } else { XSTATE_OFFSET + xlen as u64 };
        if xlen != 0 {
            let flags = c::flags(&context) | c::CONTEXT_XSTATE;
            c::set_flags(&mut context, flags);
        }
        let at = (stack.wrapping_sub(size)) & !0x3f;
        if at >= stack {
            return None;
        }
        if xlen != 0 {
            let area = c::xstate_bytes(&state, u64::MAX, xlen);
            process.write(at + XSTATE_OFFSET, &area[..xlen]).ok()?;
            process
                .write(at + c::SIZE as u64, &c::context_ex(XSTATE_OFFSET as usize, xlen))
                .ok()?;
        }
        process.write(at, &context).ok()?;
        process.write(at + RECORD_OFFSET, &record).ok()?;
        process.write(at + MACHINE_FRAME_OFFSET, &machine).ok()?;
        Some(at)
    };
    let xlen = c::xstate_len();
    let pushed = push(xlen).or_else(|| if xlen == 0 { None } else { push(0) });
    let Some(new_rsp) = pushed else {
        warn!("user exception: cannot push the dispatcher frame (rsp {stack:#x})");
        return false;
    };
    frame.set_ip(dispatcher);
    frame.set_sp(new_rsp);
    if x86 {
        // wow64.dll's dispatcher is 64-bit code.
        frame.set_segments(self::context::USER_CS.into(), self::context::USER_DS.into());
    }
    true
}

/// What the registry's CentralProcessor key says about this processor:
/// CPUID's vendor (leaf 0), family, model and stepping (leaf 1), brand
/// string (0x80000002-4) and base frequency (0x16).
pub fn processor() -> Processor {
    use crate::nt::platform::cpuid;
    let (max, b, c, d) = cpuid(0, 0);
    let mut vendor = [0u8; 12];
    vendor[..4].copy_from_slice(&b.to_le_bytes());
    vendor[4..8].copy_from_slice(&d.to_le_bytes());
    vendor[8..].copy_from_slice(&c.to_le_bytes());
    let (signature, ..) = cpuid(1, 0);
    let base_family = (signature >> 8) & 0xf;
    let mut family = base_family;
    let mut model = (signature >> 4) & 0xf;
    if base_family == 0xf {
        family += (signature >> 20) & 0xff;
    }
    if base_family == 0x6 || base_family == 0xf {
        model |= ((signature >> 16) & 0xf) << 4;
    }
    let mut name = [0u8; 48];
    if cpuid(0x8000_0000, 0).0 >= 0x8000_0004 {
        for (i, leaf) in (0x8000_0002u32..=0x8000_0004).enumerate() {
            let (a, b, c, d) = cpuid(leaf, 0);
            for (j, r) in [a, b, c, d].iter().enumerate() {
                name[16 * i + 4 * j..16 * i + 4 * j + 4].copy_from_slice(&r.to_le_bytes());
            }
        }
    }
    let mhz = if max >= 0x16 { cpuid(0x16, 0).0 & 0xffff } else { 0 };
    Processor { name, vendor, family, model, stepping: signature & 0xf, mhz }
}
