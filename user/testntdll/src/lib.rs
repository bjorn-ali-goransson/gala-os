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
