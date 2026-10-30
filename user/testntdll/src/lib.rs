//! A minimal stand-in for ntdll.dll, for testing the kernel's side of
//! process startup (written from the documented contract, Windows
//! Internals): the initial thread of a process starts at
//! `LdrInitializeThunk(Context, NtdllBase)`, where `Context` holds the
//! thread's real start; once user-mode initialization is done, the thunk
//! resumes that context with `NtContinue`.
//!
//! This one only records that it ran (`GalaTestNtdllState`, which
//! `startup_test` reads) and continues.

#![no_std]
#![allow(non_snake_case, non_upper_case_globals)]

/// "galantdl": marks the state as written by this DLL.
const MAGIC: u64 = 0x6761_6c61_6e74_646c;
/// CONTEXT.Rip (public x64 layout).
const CONTEXT_RIP: usize = 0xf8;
/// NtContinue on the pinned build (to be verified, like all numbers).
const NT_CONTINUE: u32 = 0x43;

/// [magic, NtdllBase argument, Context->Rip, number of thunk calls]
#[unsafe(no_mangle)]
pub static mut GalaTestNtdllState: [u64; 4] = [0; 4];

/// NtContinue(Context, TestAlert): resumes `Context`; does not return on
/// success.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn NtContinue(_context: *const u8, _test_alert: u8) -> i32 {
    core::arch::naked_asm!("mov r10, rcx", "mov eax, {n}", "syscall", "ret", n = const NT_CONTINUE)
}

/// The initial thread's first user-mode instruction.
///
/// # Safety
/// Called by the kernel with a valid CONTEXT.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn LdrInitializeThunk(context: *const u8, ntdll_base: u64) -> ! {
    // SAFETY: the kernel passes a readable CONTEXT; the state is only
    // written here, before any other thread exists.
    unsafe {
        let rip = (context.add(CONTEXT_RIP) as *const u64).read_unaligned();
        let state = &raw mut GalaTestNtdllState;
        let calls = (*state)[3];
        state.write_volatile([MAGIC, ntdll_base, rip, calls + 1]);
    }
    NtContinue(context, 0);
    // NtContinue only returns on failure.
    loop {
        core::hint::spin_loop();
    }
}

/// NtTerminateProcess on the pinned build (to be verified).
const NT_TERMINATE_PROCESS: u32 = 0x2c;

/// NtTerminateProcess(ProcessHandle, ExitStatus).
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn NtTerminateProcess(_process: isize, _status: u32) -> i32 {
    core::arch::naked_asm!(
        "mov r10, rcx",
        "mov eax, {n}",
        "syscall",
        "ret",
        n = const NT_TERMINATE_PROCESS
    )
}

/// PVECTORED_EXCEPTION_HANDLER: LONG (PEXCEPTION_POINTERS).
type VectoredHandler = extern "system" fn(*mut ExceptionPointers) -> i32;

#[repr(C)]
pub struct ExceptionPointers {
    record: *mut u8,
    context: *mut u8,
}

const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
const MAX_HANDLERS: usize = 16;

/// Registered vectored handlers in call order, as (cookie, handler), and
/// the next cookie.
// TODO: a lock once processes have more than one thread using this.
struct Handlers(core::cell::UnsafeCell<([(usize, usize); MAX_HANDLERS], usize)>);

// SAFETY: only used single-threaded so far (see the TODO above).
unsafe impl Sync for Handlers {}

static HANDLERS: Handlers = Handlers(core::cell::UnsafeCell::new(([(0, 0); MAX_HANDLERS], 1)));

/// RtlAddVectoredExceptionHandler(First, Handler): the cookie, or null if
/// the table is full.
#[unsafe(no_mangle)]
pub extern "system" fn RtlAddVectoredExceptionHandler(
    first: u32,
    handler: VectoredHandler,
) -> *mut core::ffi::c_void {
    // SAFETY: single-threaded use (see HANDLERS).
    unsafe {
        let (table, next) = &mut *HANDLERS.0.get();
        let used = table.iter().take_while(|e| e.0 != 0).count();
        if used == MAX_HANDLERS {
            return core::ptr::null_mut();
        }
        let cookie = *next;
        *next += 1;
        let entry = (cookie, handler as usize);
        if first != 0 {
            table.copy_within(0..used, 1);
            table[0] = entry;
        } else {
            table[used] = entry;
        }
        cookie as *mut core::ffi::c_void
    }
}

/// RtlRemoveVectoredExceptionHandler(Handle): nonzero if it was registered.
#[unsafe(no_mangle)]
pub extern "system" fn RtlRemoveVectoredExceptionHandler(cookie: *mut core::ffi::c_void) -> u32 {
    // SAFETY: single-threaded use (see HANDLERS).
    unsafe {
        let (table, _) = &mut *HANDLERS.0.get();
        let Some(i) = table.iter().position(|e| e.0 != 0 && e.0 == cookie as usize) else {
            return 0;
        };
        table.copy_within(i + 1.., i);
        table[MAX_HANDLERS - 1] = (0, 0);
        1
    }
}

/// Where the kernel puts the EXCEPTION_RECORD relative to the CONTEXT at
/// rsp (gala's dispatcher frame; to be checked against the real layout).
const RECORD_OFFSET: usize = 0x4f0;

/// The kernel resumes a thread here after a user-mode exception, with rsp
/// pointing at the CONTEXT of the fault and the EXCEPTION_RECORD above it.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn KiUserExceptionDispatcher() -> ! {
    core::arch::naked_asm!(
        "cld",
        "mov rdx, rsp",
        "lea rcx, [rsp + {record}]",
        "sub rsp, 0x20",
        "call {dispatch}",
        "int3",
        record = const RECORD_OFFSET,
        dispatch = sym dispatch_exception,
    )
}

/// Calls the vectored handlers; resumes the context if one continues
/// execution, else ends the process with the exception code.
extern "system" fn dispatch_exception(record: *mut u8, context: *mut u8) -> ! {
    let mut pointers = ExceptionPointers { record, context };
    // SAFETY: single-threaded use (see HANDLERS); handlers were registered
    // as `VectoredHandler`s.
    let table = unsafe { (*HANDLERS.0.get()).0 };
    for &(cookie, raw) in &table {
        if cookie == 0 {
            break;
        }
        let handler: VectoredHandler = unsafe { core::mem::transmute(raw) };
        if handler(&mut pointers) == EXCEPTION_CONTINUE_EXECUTION {
            NtContinue(context, 0);
        }
    }
    // SAFETY: the record starts with its ExceptionCode.
    let code = unsafe { (record as *const u32).read_unaligned() };
    NtTerminateProcess(-1, code);
    loop {
        core::hint::spin_loop();
    }
}

// Memory routines (ntdll exports them; the compiler also calls them).
// `rep` string instructions, so they cannot compile back into calls.

/// memcpy(dst, src, n) -> dst
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn memcpy(_dst: *mut u8, _src: *const u8, _n: usize) -> *mut u8 {
    core::arch::naked_asm!(
        "mov rax, rcx",
        "mov r9, rdi",
        "mov r10, rsi",
        "mov rdi, rcx",
        "mov rsi, rdx",
        "mov rcx, r8",
        "rep movsb",
        "mov rdi, r9",
        "mov rsi, r10",
        "ret",
    )
}

/// memmove(dst, src, n) -> dst (overlap-safe)
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn memmove(_dst: *mut u8, _src: *const u8, _n: usize) -> *mut u8 {
    core::arch::naked_asm!(
        "mov rax, rcx",
        "mov r9, rdi",
        "mov r10, rsi",
        "mov rdi, rcx",
        "mov rsi, rdx",
        "mov rcx, r8",
        "cmp rdi, rsi",
        "jbe 2f",
        // Destination above source: copy backwards.
        "lea rsi, [rsi + rcx - 1]",
        "lea rdi, [rdi + rcx - 1]",
        "std",
        "rep movsb",
        "cld",
        "jmp 3f",
        "2:",
        "rep movsb",
        "3:",
        "mov rdi, r9",
        "mov rsi, r10",
        "ret",
    )
}

/// memset(dst, value, n) -> dst
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn memset(_dst: *mut u8, _value: i32, _n: usize) -> *mut u8 {
    core::arch::naked_asm!(
        "mov r9, rdi",
        "mov r10, rcx",
        "mov rdi, rcx",
        "mov eax, edx",
        "mov rcx, r8",
        "rep stosb",
        "mov rdi, r9",
        "mov rax, r10",
        "ret",
    )
}

/// Entry point required by our link flags; a DllMain that does nothing.
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
