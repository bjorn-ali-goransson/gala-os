//! Exception handling in the 32-bit ntdll (WOW64), written from the
//! documented x86 SEH model (Microsoft's EXCEPTION_REGISTRATION_RECORD and
//! EXCEPTION_DISPOSITION contracts, the NT_TIB's ExceptionList at fs:[0];
//! Matt Pietrek's "A Crash Course on the Depths of Win32 Structured
//! Exception Handling", MSJ 1997) and the Rtl* functions' documented
//! contracts. x86 has no unwind tables: each frame that handles exceptions
//! links an EXCEPTION_REGISTRATION_RECORD (Next, Handler) on the stack into
//! the thread's list, newest first, ending with 0xffffffff.
//!
//! - `KiUserExceptionDispatcher`: wow64.dll hands an exception in 32-bit
//!   code here, on the 32-bit stack: esp -> [EXCEPTION_RECORD*,
//!   CONTEXT*], both x86 and above the two pointers.
//! - Dispatch: the vectored exception handlers, then each registration's
//!   handler, `handler(record, frame, context, dispatcher_context)` (cdecl),
//!   until one returns ExceptionContinueExecution (the context is
//!   resumed) ; the vectored continue handlers; unhandled: the top-level
//!   filter, then the process ends with the exception code (the kernel
//!   keeps it for the crash report).
//! - `RtlUnwind(TargetFrame, TargetIp, ExceptionRecord, ReturnValue)`:
//!   calls the handlers of the frames above the target with
//!   EXCEPTION_UNWINDING (EXCEPTION_EXIT_UNWIND without a target), unlinks
//!   them, and returns to its caller with ReturnValue in eax.
//! - `RtlRaiseException`: the caller's context (ExceptionAddress its return
//!   address), dispatched as above; a handled exception resumes after the
//!   call.
//!
//! Not yet: nested exceptions (a handler's own exception is dispatched
//! from the handler's frame, not marked EXCEPTION_NESTED_CALL) and
//! collided unwinds (dispositions 2 and 3 are taken as
//! ExceptionContinueSearch), SafeSEH's handler tables.

use crate::{ExceptionPointers, NtContinue, NtTerminateProcess, call_handlers, with_handlers};

const EXCEPTION_NONCONTINUABLE: u32 = 1;
const EXCEPTION_UNWINDING: u32 = 2;
const EXCEPTION_EXIT_UNWIND: u32 = 4;
const EXCEPTION_STACK_INVALID: u32 = 8;
const STATUS_UNWIND: u32 = 0xc000_0027;
const STATUS_NONCONTINUABLE_EXCEPTION: u32 = 0xc000_0025;

/// The end of the registration list.
const END: usize = 0xffff_ffff;

/// EXCEPTION_RECORD (x86): code, flags, the nested record, the address,
/// the parameter count, 15 parameters.
const RECORD_SIZE: usize = 0x50;
const ER_FLAGS: usize = 0x04;
const ER_RECORD: usize = 0x08;
const ER_ADDRESS: usize = 0x0c;
const ER_PARAMETERS: usize = 0x14;

/// CONTEXT (x86) offsets.
const CTX_EIP: usize = 0xb8;

/// EXCEPTION_REGISTRATION_RECORD.
#[repr(C)]
struct Registration {
    next: usize,
    handler: usize,
}

/// An exception handler (cdecl): (record, establisher frame, context,
/// dispatcher context) -> EXCEPTION_DISPOSITION.
type Handler = unsafe extern "C" fn(*mut u8, usize, *mut u8, *mut usize) -> u32;

fn exception_list() -> usize {
    let list: usize;
    // SAFETY: fs:[0] is NT_TIB.ExceptionList.
    unsafe { core::arch::asm!("mov {}, fs:[0]", out(reg) list, options(nostack, readonly)) };
    list
}

fn set_exception_list(list: usize) {
    // SAFETY: NT_TIB.ExceptionList of this thread.
    unsafe { core::arch::asm!("mov fs:[0], {}", in(reg) list, options(nostack)) };
}

/// NT_TIB.StackBase (fs:[4]) and StackLimit (fs:[8]).
fn stack_bounds() -> (usize, usize) {
    let (base, limit): (usize, usize);
    // SAFETY: fields of this thread's TEB32.
    unsafe {
        core::arch::asm!("mov {}, fs:[4]", out(reg) base, options(nostack, readonly));
        core::arch::asm!("mov {}, fs:[8]", out(reg) limit, options(nostack, readonly));
    }
    (base, limit)
}

/// Whether a registration at `frame` lies on this thread's stack, aligned.
fn frame_ok(frame: usize) -> bool {
    let (base, limit) = stack_bounds();
    frame & 3 == 0 && frame >= limit && frame.checked_add(8).is_some_and(|end| end <= base)
}

fn record_flags(record: *mut u8) -> u32 {
    // SAFETY: the record's ExceptionFlags.
    unsafe { record.add(ER_FLAGS).cast::<u32>().read_unaligned() }
}

fn set_record_flags(record: *mut u8, flags: u32) {
    // SAFETY: as above.
    unsafe { record.add(ER_FLAGS).cast::<u32>().write_unaligned(flags) }
}

/// Calls the handler of the registration at `frame`.
fn call_frame(record: *mut u8, frame: usize, context: *mut u8) -> u32 {
    // SAFETY: a registration on this thread's stack (`frame_ok`).
    let handler = unsafe { (*(frame as *const Registration)).handler };
    let mut dispatcher_context = 0usize;
    // SAFETY: the handler the frame registered, called as documented.
    unsafe {
        let f: Handler = core::mem::transmute(handler);
        f(record, frame, context, &mut dispatcher_context)
    }
}

/// The frames' handlers, newest first: whether one continued execution.
fn dispatch_frames(record: *mut u8, context: *mut u8) -> bool {
    let mut frame = exception_list();
    for _ in 0..4096 {
        if frame == END || frame == 0 {
            return false;
        }
        if !frame_ok(frame) {
            set_record_flags(record, record_flags(record) | EXCEPTION_STACK_INVALID);
            return false;
        }
        // ExceptionContinueExecution; anything else is taken as
        // ExceptionContinueSearch (ExceptionNestedException and
        // ExceptionCollidedUnwind too: see the module).
        if call_frame(record, frame, context) == 0 {
            if record_flags(record) & EXCEPTION_NONCONTINUABLE != 0 {
                raise_status(STATUS_NONCONTINUABLE_EXCEPTION, record);
            }
            return true;
        }
        // SAFETY: as in `call_frame`.
        frame = unsafe { (*(frame as *const Registration)).next };
    }
    false
}

/// Raises a new noncontinuable exception `code` about `nested` (from the
/// dispatcher: a handler continued a noncontinuable exception).
fn raise_status(code: u32, nested: *mut u8) -> ! {
    let mut record = [0u8; RECORD_SIZE];
    record[..4].copy_from_slice(&code.to_le_bytes());
    record[ER_FLAGS..ER_FLAGS + 4].copy_from_slice(&EXCEPTION_NONCONTINUABLE.to_le_bytes());
    record[ER_RECORD..ER_RECORD + 4].copy_from_slice(&(nested as u32).to_le_bytes());
    RtlRaiseException(record.as_mut_ptr());
    // A noncontinuable exception is never resumed.
    NtTerminateProcess(-1, code);
    loop {
        core::hint::spin_loop();
    }
}

/// Exception dispatch (`KiUserExceptionDispatcher`, `RtlRaiseException`):
/// the vectored exception handlers, then the frames' handlers; the
/// vectored continue handlers run both when one continued execution and
/// when none did (as the x64 ntdll). Unhandled: the top-level filter,
/// then the process ends with the exception code.
extern "system" fn dispatch_exception(record: *mut u8, context: *mut u8) -> ! {
    let mut pointers = ExceptionPointers { record, context };
    // Snapshots, so that handlers may (un)register handlers themselves.
    let (exception, continue_) = with_handlers(|t| (t.exception, t.continue_));
    let handled = call_handlers(&exception, &mut pointers) || dispatch_frames(record, context);
    if call_handlers(&continue_, &mut pointers) || handled {
        NtContinue(context, 0);
    }
    // Unhandled: the kernel keeps it for the crash report (wow64.dll
    // passes it on with the 32-bit registers); the top-level filter may
    // continue execution; otherwise the process ends.
    crate::GalaUnhandledException(record, context);
    let filter = TOP_FILTER.load(core::sync::atomic::Ordering::Acquire);
    if filter != 0 {
        // SAFETY: a filter registered with RtlSetUnhandledExceptionFilter.
        let f: extern "system" fn(*mut ExceptionPointers) -> i32 =
            unsafe { core::mem::transmute(filter) };
        if f(&mut pointers) == -1 {
            crate::GalaUnhandledException(core::ptr::null(), core::ptr::null());
            NtContinue(context, 0);
        }
    }
    // SAFETY: the record's code, address and second parameter.
    let (code, address, info) = unsafe {
        (
            record.cast::<u32>().read_unaligned(),
            record.add(ER_ADDRESS).cast::<u32>().read_unaligned(),
            record.add(ER_PARAMETERS + 4).cast::<u32>().read_unaligned(),
        )
    };
    // Gaps are never silent: the ledger has the code and where it was
    // raised (and for access violations, the address accessed).
    let what = b"ntdll.dll!unhandled exception";
    crate::GalaRecordMissing(
        what.as_ptr(),
        what.len(),
        2,
        u64::from(code) << 32 | u64::from(address),
    );
    if code == 0xc000_0005 {
        let what = b"ntdll.dll!unhandled access violation at";
        crate::GalaRecordMissing(what.as_ptr(), what.len(), 2, info.into());
    }
    NtTerminateProcess(-1, code);
    loop {
        core::hint::spin_loop();
    }
}

/// The top-level unhandled exception filter (0: none).
static TOP_FILTER: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// RtlSetUnhandledExceptionFilter: the filter unhandled exceptions go to
/// (kernel32 sets its UnhandledExceptionFilter).
#[unsafe(no_mangle)]
pub extern "system" fn RtlSetUnhandledExceptionFilter(filter: usize) {
    TOP_FILTER.store(filter, core::sync::atomic::Ordering::Release);
}

/// `RtlRaiseException`'s work, from the caller's context.
extern "system" fn raise(record: *mut u8, context: *mut u8) -> ! {
    // SAFETY: the caller's record; the context this function's caller
    // captured.
    unsafe {
        let eip = context.add(CTX_EIP).cast::<u32>().read_unaligned();
        record.add(ER_ADDRESS).cast::<u32>().write_unaligned(eip);
    }
    dispatch_exception(record, context)
}

/// `RtlUnwind`'s work, from the caller's context (which resumes it).
extern "system" fn unwind(target: usize, record: *mut u8, context: *mut u8) -> ! {
    let mut own = [0u8; RECORD_SIZE];
    let record = if record.is_null() {
        own[..4].copy_from_slice(&STATUS_UNWIND.to_le_bytes());
        own.as_mut_ptr()
    } else {
        record
    };
    let mut flags = record_flags(record) | EXCEPTION_UNWINDING;
    if target == 0 {
        flags |= EXCEPTION_EXIT_UNWIND;
    }
    set_record_flags(record, flags);
    let mut frame = exception_list();
    for _ in 0..4096 {
        if frame == END || frame == 0 || frame == target {
            break;
        }
        if !frame_ok(frame) {
            let what = b"ntdll.dll!RtlUnwind(invalid frame)";
            crate::GalaRecordMissing(what.as_ptr(), what.len(), 2, frame as u64);
            break;
        }
        call_frame(record, frame, context);
        // SAFETY: as in `call_frame`.
        let next = unsafe { (*(frame as *const Registration)).next };
        set_exception_list(next);
        frame = next;
    }
    if target != 0 && frame != target {
        let what = b"ntdll.dll!RtlUnwind(target not in the chain)";
        crate::GalaRecordMissing(what.as_ptr(), what.len(), 2, target as u64);
    }
    NtContinue(context, 0);
    loop {
        core::hint::spin_loop();
    }
}

/// KiUserExceptionDispatcher: esp -> [EXCEPTION_RECORD*, CONTEXT*] (x86;
/// wow64.dll put both above them).
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn KiUserExceptionDispatcher() -> ! {
    core::arch::naked_asm!(
        "mov ecx, [esp]",
        "mov edx, [esp + 4]",
        "push edx",
        "push ecx",
        "call {dispatch}",
        "int3",
        dispatch = sym dispatch_exception,
    )
}

/// RtlCaptureContext(*Context): the caller's registers (CONTEXT_FULL), as
/// they are after this call returns.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn RtlCaptureContext(_context: *mut u8) {
    core::arch::naked_asm!(
        "push ecx",
        "mov ecx, [esp + 8]",
        "mov [ecx + 0xb0], eax",
        "mov [ecx + 0xa8], edx",
        "mov [ecx + 0xa4], ebx",
        "mov [ecx + 0xa0], esi",
        "mov [ecx + 0x9c], edi",
        "mov [ecx + 0xb4], ebp",
        "pop dword ptr [ecx + 0xac]",
        // Eip: the return address; Esp: after the return and the argument.
        "mov eax, [esp]",
        "mov [ecx + 0xb8], eax",
        "lea eax, [esp + 8]",
        "mov [ecx + 0xc4], eax",
        "pushfd",
        "pop dword ptr [ecx + 0xc0]",
        "xor eax, eax",
        "mov ax, cs",
        "mov [ecx + 0xbc], eax",
        "mov ax, ss",
        "mov [ecx + 0xc8], eax",
        "mov ax, ds",
        "mov [ecx + 0x98], eax",
        "mov ax, es",
        "mov [ecx + 0x94], eax",
        "mov ax, fs",
        "mov [ecx + 0x90], eax",
        "mov ax, gs",
        "mov [ecx + 0x8c], eax",
        // CONTEXT_i386 | CONTROL | INTEGER | SEGMENTS.
        "mov dword ptr [ecx], 0x10007",
        "mov eax, [ecx + 0xb0]",
        "mov ecx, [ecx + 0xac]",
        "ret 4",
    )
}

/// RtlRaiseException(*ExceptionRecord): dispatches the exception from the
/// caller's context (ExceptionAddress: the return address); a handled one
/// resumes after the call.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn RtlRaiseException(_record: *mut u8) {
    core::arch::naked_asm!(
        "push ebp",
        "mov ebp, esp",
        "sub esp, 0x2d0",
        "and esp, -16",
        "push esp",
        "call {capture}",
        "mov eax, [ebp + 4]",
        "mov [esp + 0xb8], eax",
        "lea eax, [ebp + 12]",
        "mov [esp + 0xc4], eax",
        "mov eax, [ebp]",
        "mov [esp + 0xb4], eax",
        "mov eax, esp",
        "push eax",
        "push dword ptr [ebp + 8]",
        "call {raise}",
        "int3",
        capture = sym RtlCaptureContext,
        raise = sym raise,
    )
}

/// RtlUnwind(TargetFrame, TargetIp, *ExceptionRecord, ReturnValue): the
/// frames above TargetFrame are unwound (their handlers called with
/// EXCEPTION_UNWINDING, then unlinked); returns to the caller with
/// ReturnValue in eax. TargetIp is not used (as on x86 Windows, the caller
/// goes on itself).
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn RtlUnwind(_frame: usize, _ip: usize, _record: *mut u8, _value: usize) {
    core::arch::naked_asm!(
        "push ebp",
        "mov ebp, esp",
        "sub esp, 0x2d0",
        "and esp, -16",
        "push esp",
        "call {capture}",
        "mov eax, [ebp + 4]",
        "mov [esp + 0xb8], eax",
        "lea eax, [ebp + 24]",
        "mov [esp + 0xc4], eax",
        "mov eax, [ebp]",
        "mov [esp + 0xb4], eax",
        "mov eax, [ebp + 20]",
        "mov [esp + 0xb0], eax",
        "mov eax, esp",
        "push eax",
        "push dword ptr [ebp + 16]",
        "push dword ptr [ebp + 8]",
        "call {unwind}",
        "int3",
        capture = sym RtlCaptureContext,
        unwind = sym unwind,
    )
}
