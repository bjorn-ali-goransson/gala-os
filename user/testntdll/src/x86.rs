//! The x86 parts of gala's ntdll, built as the 32-bit ntdll in
//! `C:\Windows\SysWOW64` (ARCHITECTURE "Plan: WOW64"): the system call
//! stubs as on 64-bit Windows' 32-bit ntdll (`mov eax, n; call
//! fs:[0xc0]`, the x64 service number, through TEB32.WOW32Reserved to
//! wow64.dll's transition; stdcall: `ret` pops the arguments), the TEB32
//! and PEB32 through FS and their x86 offsets, and the memory routines
//! (cdecl). The public x86 layouts (winnt.h, winternl.h).

/// NtContinue on the pinned build (to be verified, like all numbers).
const NT_CONTINUE: u32 = 0x43;
/// NtTerminateProcess.
const NT_TERMINATE_PROCESS: u32 = 0x2c;

/// CONTEXT.Eip (x86 layout): where the initial thread starts.
pub const CONTEXT_PC: usize = 0xb8;
/// PEB32 offsets.
pub const PEB_IMAGE_BASE: usize = 0x08;
pub const PEB_LDR: usize = 0x0c;
pub const PEB_PROCESS_PARAMETERS: usize = 0x10;
pub const PEB_PROCESS_HEAP: usize = 0x18;
pub const PEB_LOADER_LOCK: usize = 0xa0;
/// RTL_USER_PROCESS_PARAMETERS (x86): CurrentDirectory (its DosPath; the
/// handle follows the string) and ImagePathName.
pub const PARAMS_CURRENT_DIRECTORY: usize = 0x24;
pub const PARAMS_IMAGE_PATH: usize = 0x38;
/// The optional header's data directories (PE32).
pub const DATA_DIRECTORIES: usize = 96;
/// The optional header's magic of this build's images (PE32).
pub const NATIVE_MAGIC: u32 = 0x10b;
/// Where the 32-bit system DLLs are (under the Windows directory).
pub const SYSTEM_DIR: &[u8] = b"\\SysWOW64\\";
/// TEB32.LastStatusValue.
const TEB_LAST_STATUS: usize = 0xbf4;

/// The PEB32 (TEB32.ProcessEnvironmentBlock, fs:[0x30]).
pub fn peb() -> usize {
    let peb: usize;
    // SAFETY: fs:[0x30] is TEB32.ProcessEnvironmentBlock.
    unsafe { core::arch::asm!("mov {}, fs:[0x30]", out(reg) peb, options(nostack, readonly)) };
    peb
}

/// This thread's id (TEB32.ClientId.UniqueThread, fs:[0x24]).
pub fn thread_id() -> usize {
    let me: usize;
    // SAFETY: fs:[0x24] is TEB32.ClientId.UniqueThread.
    unsafe { core::arch::asm!("mov {}, fs:[0x24]", out(reg) me, options(nostack, readonly)) };
    me
}

/// TEB32.ThreadLocalStoragePointer (fs:[0x2c]).
pub fn tls_array() -> usize {
    let array: usize;
    // SAFETY: fs:[0x2c] is TEB32.ThreadLocalStoragePointer.
    unsafe { core::arch::asm!("mov {}, fs:[0x2c]", out(reg) array, options(nostack, readonly)) };
    array
}

pub fn set_tls_array(array: usize) {
    // SAFETY: TEB32.ThreadLocalStoragePointer of this thread.
    unsafe { core::arch::asm!("mov fs:[0x2c], {}", in(reg) array, options(nostack)) };
}

/// TEB32.LastStatusValue.
pub fn last_status() -> u32 {
    let v: u32;
    // SAFETY: the running thread's TEB32.
    unsafe {
        core::arch::asm!("mov {}, fs:[{}]", out(reg) v, const TEB_LAST_STATUS, options(nostack, readonly))
    };
    v
}

pub fn set_last_status(status: u32) {
    // SAFETY: the running thread's TEB32.
    unsafe {
        core::arch::asm!("mov fs:[{}], {}", const TEB_LAST_STATUS, in(reg) status, options(nostack))
    };
}

/// NtContinue(Context, TestAlert): resumes `Context` (the x86 CONTEXT,
/// which wow64.dll takes); does not return on success.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn NtContinue(_context: *const u8, _test_alert: u8) -> i32 {
    core::arch::naked_asm!("mov eax, {n}", "call dword ptr fs:[0xc0]", "ret 8", n = const NT_CONTINUE)
}

/// NtTerminateProcess(ProcessHandle, ExitStatus).
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn NtTerminateProcess(_process: isize, _status: u32) -> i32 {
    core::arch::naked_asm!(
        "mov eax, {n}",
        "call dword ptr fs:[0xc0]",
        "ret 8",
        n = const NT_TERMINATE_PROCESS
    )
}

/// System call stubs shaped like the 32-bit ntdll's on 64-bit Windows:
/// `mov eax, n; call fs:[0xc0]; ret <bytes>` (stdcall: the arguments stay
/// on the stack for wow64.dll, above the two return addresses, and the
/// stub pops them: each takes 4 bytes, an 8-byte value 8).
macro_rules! syscall_stubs {
    ($($name:ident = $n:expr, fn($($arg:ident: $ty:ty),*);)*) => {$(
        #[unsafe(naked)]
        #[unsafe(no_mangle)]
        pub extern "system" fn $name($(_: $ty),*) -> u32 {
            core::arch::naked_asm!(
                "mov eax, {n}",
                "call dword ptr fs:[0xc0]",
                "ret {bytes}",
                n = const $n,
                bytes = const 0 $(+ core::mem::size_of::<$ty>().next_multiple_of(4))*,
            )
        }
    )*};
}

/// wow64.dll resumes a 32-bit thread here to run a user APC (gala's
/// contract between the two): esp -> [NormalRoutine, NormalContext,
/// SystemArgument1, SystemArgument2, CONTEXT*], the x86 CONTEXT to return
/// to (the interrupted system call's caller, eax STATUS_USER_APC) above
/// them. The routine (stdcall, three arguments) runs, then NtContinue with
/// TestAlert delivers the next APC or resumes the CONTEXT.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn KiUserApcDispatcher() -> ! {
    core::arch::naked_asm!(
        // The CONTEXT, in a register the routine preserves.
        "mov esi, [esp + 16]",
        "pop eax",
        "call eax",
        "push 1",
        "push esi",
        "call {continue}",
        "int3",
        continue = sym NtContinue,
    )
}

// Memory routines (ntdll exports them; the compiler also calls them):
// cdecl, `rep` string instructions so they cannot compile back into calls.

/// memcpy(dst, src, n) -> dst
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn memcpy(_dst: *mut u8, _src: *const u8, _n: usize) -> *mut u8 {
    core::arch::naked_asm!(
        "push esi",
        "push edi",
        "mov edi, [esp + 12]",
        "mov esi, [esp + 16]",
        "mov ecx, [esp + 20]",
        "mov eax, edi",
        "rep movsb",
        "pop edi",
        "pop esi",
        "ret",
    )
}

/// memmove(dst, src, n) -> dst (overlap-safe)
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn memmove(_dst: *mut u8, _src: *const u8, _n: usize) -> *mut u8 {
    core::arch::naked_asm!(
        "push esi",
        "push edi",
        "mov edi, [esp + 12]",
        "mov esi, [esp + 16]",
        "mov ecx, [esp + 20]",
        "mov eax, edi",
        "cmp edi, esi",
        "jbe 2f",
        // Destination above source: copy backwards.
        "lea esi, [esi + ecx - 1]",
        "lea edi, [edi + ecx - 1]",
        "std",
        "rep movsb",
        "cld",
        "jmp 3f",
        "2:",
        "rep movsb",
        "3:",
        "pop edi",
        "pop esi",
        "ret",
    )
}

/// memset(dst, value, n) -> dst
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn memset(_dst: *mut u8, _value: i32, _n: usize) -> *mut u8 {
    core::arch::naked_asm!(
        "push edi",
        "mov edi, [esp + 8]",
        "mov eax, [esp + 12]",
        "mov ecx, [esp + 16]",
        "mov edx, edi",
        "rep stosb",
        "mov eax, edx",
        "pop edi",
        "ret",
    )
}

// The MSVC run-time library's 64-bit arithmetic helpers, which the
// compiler calls on x86 and Windows' 32-bit ntdll exports (compiled
// programs import them from it; build.rs exports them). The multiply and
// divide helpers take both operands on the stack, low dword first, pop
// them and return the result in edx:eax (the dvrm forms the remainder in
// ebx:ecx too), on top of compiler_builtins' __udivdi3/__umoddi3/
// __divdi3/__moddi3 (cdecl); the shifts take the value in edx:eax and the
// count in cl (64 or more shifts everything out).
core::arch::global_asm!(
    ".globl __allmul",
    "__allmul:",
    "mov eax, [esp + 8]",
    "mov ecx, [esp + 16]",
    "or ecx, eax",
    "mov ecx, [esp + 12]",
    "jnz 2f",
    "mov eax, [esp + 4]",
    "mul ecx",
    "ret 16",
    "2:",
    "push ebx",
    "mul ecx",
    "mov ebx, eax",
    "mov eax, [esp + 8]",
    "mul dword ptr [esp + 20]",
    "add ebx, eax",
    "mov eax, [esp + 8]",
    "mul ecx",
    "add edx, ebx",
    "pop ebx",
    "ret 16",
    ".globl __alldiv",
    "__alldiv:",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "call ___divdi3",
    "add esp, 16",
    "ret 16",
    ".globl __allrem",
    "__allrem:",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "call ___moddi3",
    "add esp, 16",
    "ret 16",
    ".globl __alldvrm",
    "__alldvrm:",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "call ___moddi3",
    "add esp, 16",
    "push eax",
    "push edx",
    "push dword ptr [esp + 24]",
    "push dword ptr [esp + 24]",
    "push dword ptr [esp + 24]",
    "push dword ptr [esp + 24]",
    "call ___divdi3",
    "add esp, 16",
    "pop ebx",
    "pop ecx",
    "ret 16",
    ".globl __aulldvrm",
    "__aulldvrm:",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "call ___umoddi3",
    "add esp, 16",
    "push eax",
    "push edx",
    "push dword ptr [esp + 24]",
    "push dword ptr [esp + 24]",
    "push dword ptr [esp + 24]",
    "push dword ptr [esp + 24]",
    "call ___udivdi3",
    "add esp, 16",
    "pop ebx",
    "pop ecx",
    "ret 16",
    ".globl __allshl",
    "__allshl:",
    "cmp cl, 64",
    "jae 3f",
    "cmp cl, 32",
    "jae 2f",
    "shld edx, eax, cl",
    "shl eax, cl",
    "ret",
    "2:",
    "mov edx, eax",
    "xor eax, eax",
    "and cl, 31",
    "shl edx, cl",
    "ret",
    "3:",
    "xor eax, eax",
    "xor edx, edx",
    "ret",
    ".globl __allshr",
    "__allshr:",
    "cmp cl, 64",
    "jae 3f",
    "cmp cl, 32",
    "jae 2f",
    "shrd eax, edx, cl",
    "sar edx, cl",
    "ret",
    "2:",
    "mov eax, edx",
    "sar edx, 31",
    "and cl, 31",
    "sar eax, cl",
    "ret",
    "3:",
    "sar edx, 31",
    "mov eax, edx",
    "ret",
    ".globl __aullshr",
    "__aullshr:",
    "cmp cl, 64",
    "jae 3f",
    "cmp cl, 32",
    "jae 2f",
    "shrd eax, edx, cl",
    "shr edx, cl",
    "ret",
    "2:",
    "mov eax, edx",
    "xor edx, edx",
    "and cl, 31",
    "shr eax, cl",
    "ret",
    "3:",
    "xor eax, eax",
    "xor edx, edx",
    "ret",
);

core::arch::global_asm!(
    ".globl __aulldiv",
    "__aulldiv:",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "call ___udivdi3",
    "add esp, 16",
    "ret 16",
    ".globl __aullrem",
    "__aullrem:",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "push dword ptr [esp + 16]",
    "call ___umoddi3",
    "add esp, 16",
    "ret 16",
);

/// GalaWow64FsRedirection(Disable, *OldValue): wow64.dll's own service
/// (the kernel has none of this number): this thread's file system
/// redirection flag, TEB64.TlsSlots[8] (WOW64_TLS_FILESYSREDIR), which
/// 32-bit code cannot reach.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn GalaWow64FsRedirection(_disable: usize, _old: *mut usize) -> u32 {
    core::arch::naked_asm!("mov eax, 0x7ffe", "call dword ptr fs:[0xc0]", "ret 8")
}

/// RtlWow64EnableFsRedirectionEx(Disable, *OldValue): turns this thread's
/// file system redirection off (Disable non-zero) or back on; the previous
/// Disable value goes to OldValue (kernel32's
/// Wow64DisableWow64FsRedirection and Wow64RevertWow64FsRedirection).
#[unsafe(no_mangle)]
pub extern "system" fn RtlWow64EnableFsRedirectionEx(disable: usize, old: *mut usize) -> u32 {
    GalaWow64FsRedirection(disable, old)
}

/// RtlWow64EnableFsRedirection(Enable): redirection on or off for this
/// thread (kernel32's Wow64EnableWow64FsRedirection).
#[unsafe(no_mangle)]
pub extern "system" fn RtlWow64EnableFsRedirection(enable: u8) -> u32 {
    let mut old = 0usize;
    GalaWow64FsRedirection(usize::from(enable == 0), &mut old)
}
