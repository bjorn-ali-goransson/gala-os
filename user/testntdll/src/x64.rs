//! The x64 parts of gala's ntdll: the system call stubs (`mov r10, rcx;
//! mov eax, n; syscall`), the TEB and PEB through GS and their x64
//! offsets, the APC dispatcher and the memory routines. The x86 build
//! (the 32-bit ntdll in SysWOW64) has its own (`x86.rs`).

/// NtContinue on the pinned build (to be verified, like all numbers).
const NT_CONTINUE: u32 = 0x43;

/// CONTEXT.Rip (public x64 layout): where the initial thread starts.
pub const CONTEXT_PC: usize = 0xf8;
/// PEB offsets (public x64 layout).
pub const PEB_IMAGE_BASE: usize = 0x10;
pub const PEB_LDR: usize = 0x18;
pub const PEB_PROCESS_PARAMETERS: usize = 0x20;
pub const PEB_PROCESS_HEAP: usize = 0x30;
pub const PEB_LOADER_LOCK: usize = 0x110;
/// RTL_USER_PROCESS_PARAMETERS: CurrentDirectory (its DosPath; the handle
/// follows the string) and ImagePathName.
pub const PARAMS_CURRENT_DIRECTORY: usize = 0x38;
pub const PARAMS_IMAGE_PATH: usize = 0x60;
/// The optional header's data directories (PE32+).
pub const DATA_DIRECTORIES: usize = 112;
/// The optional header's magic of this build's images (PE32+).
pub const NATIVE_MAGIC: u32 = 0x20b;
/// Where the system DLLs are (under the Windows directory).
pub const SYSTEM_DIR: &[u8] = b"\\System32\\";
/// TEB.LastStatusValue.
const TEB_LAST_STATUS: usize = 0x1250;

/// The PEB (TEB.ProcessEnvironmentBlock, gs:[0x60]).
pub fn peb() -> usize {
    let peb: usize;
    // SAFETY: gs:[0x60] is TEB.ProcessEnvironmentBlock.
    unsafe { core::arch::asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly)) };
    peb
}

/// This thread's id (TEB.ClientId.UniqueThread, gs:[0x48]).
pub fn thread_id() -> usize {
    let me: usize;
    // SAFETY: gs:[0x48] is TEB.ClientId.UniqueThread.
    unsafe { core::arch::asm!("mov {}, gs:[0x48]", out(reg) me, options(nostack, readonly)) };
    me
}

/// TEB.ThreadLocalStoragePointer (gs:[0x58]).
pub fn tls_array() -> usize {
    let array: usize;
    // SAFETY: gs:[0x58] is TEB.ThreadLocalStoragePointer.
    unsafe { core::arch::asm!("mov {}, gs:[0x58]", out(reg) array, options(nostack, readonly)) };
    array
}

pub fn set_tls_array(array: usize) {
    // SAFETY: TEB.ThreadLocalStoragePointer of this thread.
    unsafe { core::arch::asm!("mov gs:[0x58], {}", in(reg) array, options(nostack)) };
}

/// TEB.LastStatusValue.
pub fn last_status() -> u32 {
    let v: u32;
    // SAFETY: the running thread's TEB.
    unsafe {
        core::arch::asm!("mov {:e}, gs:[{}]", out(reg) v, const TEB_LAST_STATUS, options(nostack, readonly))
    };
    v
}

pub fn set_last_status(status: u32) {
    // SAFETY: the running thread's TEB.
    unsafe {
        core::arch::asm!("mov gs:[{}], {:e}", const TEB_LAST_STATUS, in(reg) status, options(nostack))
    };
}

/// NtContinue(Context, TestAlert): resumes `Context`; does not return on
/// success.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn NtContinue(_context: *const u8, _test_alert: u8) -> i32 {
    core::arch::naked_asm!("mov r10, rcx", "mov eax, {n}", "syscall", "ret", n = const NT_CONTINUE)
}

/// System call stubs shaped like ntdll's: `mov r10, rcx; mov eax, n;
/// syscall; ret` (stack arguments stay where the kernel expects them).
macro_rules! syscall_stubs {
    ($($name:ident = $n:expr, fn($($arg:ident: $ty:ty),*);)*) => {$(
        #[unsafe(naked)]
        #[unsafe(no_mangle)]
        pub extern "system" fn $name($(_: $ty),*) -> u32 {
            core::arch::naked_asm!("mov r10, rcx", "mov eax, {n}", "syscall", "ret", n = const $n)
        }
    )*};
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

/// The kernel resumes a thread here to run a user APC: rsp points at the
/// CONTEXT to return to, whose P1Home..P4Home hold NormalContext,
/// SystemArgument1, SystemArgument2 and NormalRoutine. The routine runs
/// with the CONTEXT's home area as its shadow space; NtContinue with
/// TestAlert then delivers the next APC or resumes the CONTEXT.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn KiUserApcDispatcher() -> ! {
    core::arch::naked_asm!(
        "mov rcx, [rsp]",
        "mov rdx, [rsp + 8]",
        "mov r8, [rsp + 0x10]",
        "mov rax, [rsp + 0x18]",
        "call rax",
        "mov rcx, rsp",
        "mov edx, 1",
        "call {continue}",
        "int3",
        continue = sym NtContinue,
    )
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

/// __chkstk: the stack probe compilers call before allocating more than a
/// page of stack (rax = bytes). Touches each page from the caller's stack
/// pointer down, so guard pages grow the stack in order; changes no
/// register (the caller then subtracts rax from rsp). Exported by ntdll,
/// as on Windows.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn __chkstk() {
    core::arch::naked_asm!(
        "push rcx",
        "push rax",
        "lea rcx, [rsp + 24]", // the caller's rsp (after our return)
        "cmp rax, 0x1000",
        "jb 2f",
        "1:",
        "sub rcx, 0x1000",
        "test byte ptr [rcx], 0",
        "sub rax, 0x1000",
        "cmp rax, 0x1000",
        "ja 1b",
        "2:",
        "sub rcx, rax",
        "test byte ptr [rcx], 0",
        "pop rax",
        "pop rcx",
        "ret",
    )
}
