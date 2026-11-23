//! The ARM64 parts of gala's ntdll (ROADMAP AR), with `x64.rs`'s names.
//!
//! Filled in where the public documents or gala's own design say it: the
//! TEB through x18 (Microsoft's "Overview of ARM64 ABI conventions"), the
//! 64-bit PEB and process parameters, ARM64's CONTEXT.Pc (winnt.h), the
//! memory routines and `__chkstk` (x15 = bytes / 16; only x16 and x17
//! change, as the ABI documents). The system call stubs are shaped like
//! Windows ARM64's ntdll: the service number in the SVC's immediate
//! (`svc #N`), the arguments in x0-x7 and on the stack, the result in x0
//! (the kernel's `kernel/src/arch/aarch64/syscall.rs`). The APC
//! dispatcher reads the kernel's ARM64 APC frame; exception dispatch is
//! `seh_arm64.rs`'s.

/// NtContinue on the pinned build (to be verified, like all numbers).
const NT_CONTINUE: u32 = 0x43;

/// CONTEXT.Pc (ARM64 layout): where the initial thread starts.
pub const CONTEXT_PC: usize = 0x108;
/// PEB offsets (the 64-bit layout, x64's).
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

/// The TEB's pointer-sized field at `offset`.
fn teb_read(offset: usize) -> usize {
    let v: usize;
    // SAFETY: x18 holds the TEB; [x18 + offset] is a field of it.
    unsafe {
        core::arch::asm!("ldr {}, [x18, {}]", out(reg) v, in(reg) offset, options(nostack, readonly, preserves_flags))
    };
    v
}

/// The PEB (TEB.ProcessEnvironmentBlock, +0x60).
pub fn peb() -> usize {
    teb_read(0x60)
}

/// This thread's id (TEB.ClientId.UniqueThread, +0x48).
pub fn thread_id() -> usize {
    teb_read(0x48)
}

/// TEB.ThreadLocalStoragePointer (+0x58).
pub fn tls_array() -> usize {
    teb_read(0x58)
}

pub fn set_tls_array(array: usize) {
    // SAFETY: TEB.ThreadLocalStoragePointer of this thread.
    unsafe {
        core::arch::asm!("str {}, [x18, #0x58]", in(reg) array, options(nostack, preserves_flags))
    };
}

/// TEB.LastStatusValue.
pub fn last_status() -> u32 {
    let v: u32;
    // SAFETY: the running thread's TEB.
    unsafe {
        core::arch::asm!("ldr {:w}, [x18, {}]", out(reg) v, in(reg) TEB_LAST_STATUS, options(nostack, readonly, preserves_flags))
    };
    v
}

pub fn set_last_status(status: u32) {
    // SAFETY: the running thread's TEB.
    unsafe {
        core::arch::asm!("str {:w}, [x18, {}]", in(reg) status, in(reg) TEB_LAST_STATUS, options(nostack, preserves_flags))
    };
}

/// NtContinue(Context, TestAlert): resumes `Context`; does not return on
/// success.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn NtContinue(_context: *const u8, _test_alert: u8) -> i32 {
    core::arch::naked_asm!("svc #{n}", "ret", n = const NT_CONTINUE)
}

/// System call stubs: `svc #N` (the arguments stay in x0-x7 and on the
/// stack, where the kernel expects them).
macro_rules! syscall_stubs {
    ($($name:ident = $n:expr, fn($($arg:ident: $ty:ty),*);)*) => {$(
        #[unsafe(naked)]
        #[unsafe(no_mangle)]
        pub extern "system" fn $name($(_: $ty),*) -> u32 {
            core::arch::naked_asm!("svc #{n}", "ret", n = const $n)
        }
    )*};
}

/// NtTerminateProcess on the pinned build (to be verified).
const NT_TERMINATE_PROCESS: u32 = 0x2c;

/// NtTerminateProcess(ProcessHandle, ExitStatus).
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "system" fn NtTerminateProcess(_process: isize, _status: u32) -> i32 {
    core::arch::naked_asm!("svc #{n}", "ret", n = const NT_TERMINATE_PROCESS)
}

// KiUserApcDispatcher is in `seh_arm64`, with the other entry points the
// kernel resumes a thread at: it needs unwind data, which a naked function
// does not get (it has no prologue for the compiler to describe), and a
// walk that cannot pass it ends there.

// Memory routines (ntdll exports them; the compiler also calls them).
// Byte loops in assembly, so they cannot compile back into calls.

/// memcpy(dst, src, n) -> dst
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn memcpy(_dst: *mut u8, _src: *const u8, _n: usize) -> *mut u8 {
    core::arch::naked_asm!(
        "mov x3, x0",
        "cbz x2, 2f",
        "1:",
        "ldrb w4, [x1], #1",
        "strb w4, [x3], #1",
        "subs x2, x2, #1",
        "b.ne 1b",
        "2:",
        "ret",
    )
}

/// memmove(dst, src, n) -> dst (overlap-safe)
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn memmove(_dst: *mut u8, _src: *const u8, _n: usize) -> *mut u8 {
    core::arch::naked_asm!(
        "mov x3, x0",
        "cbz x2, 3f",
        "cmp x0, x1",
        "b.ls 2f",
        // Destination above source: copy backwards.
        "add x1, x1, x2",
        "add x3, x0, x2",
        "1:",
        "ldrb w4, [x1, #-1]!",
        "strb w4, [x3, #-1]!",
        "subs x2, x2, #1",
        "b.ne 1b",
        "b 3f",
        "2:",
        "ldrb w4, [x1], #1",
        "strb w4, [x3], #1",
        "subs x2, x2, #1",
        "b.ne 2b",
        "3:",
        "ret",
    )
}

/// memset(dst, value, n) -> dst
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn memset(_dst: *mut u8, _value: i32, _n: usize) -> *mut u8 {
    core::arch::naked_asm!(
        "mov x3, x0",
        "cbz x2, 2f",
        "1:",
        "strb w1, [x3], #1",
        "subs x2, x2, #1",
        "b.ne 1b",
        "2:",
        "ret",
    )
}

/// __chkstk: the stack probe compilers call before allocating more than a
/// page of stack (x15 = bytes / 16). Touches each page below the stack
/// pointer in order, so guard pages grow the stack; changes only x16 and
/// x17 (the caller then subtracts x15 * 16 from sp). Exported by ntdll,
/// as on Windows.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn __chkstk() {
    core::arch::naked_asm!(
        "lsl x16, x15, #4",
        "mov x17, sp",
        "1:",
        "cmp x16, #0x1000",
        "b.lo 2f",
        "sub x17, x17, #0x1000",
        "ldr xzr, [x17]",
        "sub x16, x16, #0x1000",
        "b 1b",
        "2:",
        "sub x17, x17, x16",
        "ldr xzr, [x17]",
        "ret",
    )
}
