//! A tiny DLL for image-section tests: exports a function, a data value
//! and a function that reads the value through a pointer stored in the
//! DLL's data (so the image needs a DIR64 base relocation). Three
//! more exports are what an import trace needs of a callee: one that
//! calls back out, so that a call through an import slot can be nested
//! inside another; one that reads its own return address, which is what
//! a stack walk sees of a call whose return the kernel trapped
//! (`imports_answers=`, user/iat_ret_test); and one that jumps straight
//! on to a function it is handed, so that a call through a slot can
//! raise an exception at its caller's frame (user/iat_unwind_test);
//! and one that takes four arguments, which is as many as an import
//! trace can record of a call (user/import_strings_test).

#![no_std]

#[unsafe(no_mangle)]
pub static mut GALA_VALUE: u64 = 0x1234_5678_9abc;

/// Absolute address of `GALA_VALUE`: fixed up by base relocation.
#[unsafe(no_mangle)]
// A reference to the static mut itself is denied; this one points at it.
#[allow(clippy::deref_addrof)]
pub static GALA_VALUE_PTR: &u64 = unsafe { &*(&raw const GALA_VALUE) };

#[unsafe(no_mangle)]
pub extern "C" fn gala_add(a: u64, b: u64) -> u64 {
    a.wrapping_add(b)
}

#[unsafe(no_mangle)]
pub extern "C" fn gala_get_value() -> u64 {
    // SAFETY: reads the value through the relocated pointer.
    unsafe { core::ptr::read_volatile(GALA_VALUE_PTR) }
}

/// Calls `f` and adds one to what it answered: a call into this DLL
/// that makes a call back out of it, so that a program can nest a call
/// through one import slot inside a call through another. The added one
/// is what tells the two answers apart.
#[unsafe(no_mangle)]
pub extern "C" fn gala_call_back(f: extern "C" fn(u64) -> u64, x: u64) -> u64 {
    f(x).wrapping_add(1)
}

/// The address this call returns to, read from where the call left it:
/// what a callee sees of its own caller, which is what a stack walk or
/// an unwind reads. Under `imports_answers=` that is the kernel's
/// return trap and not the caller's code, which is the limit the option
/// is read with (docs/OBSERVABILITY.md, "Import tracing").
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn gala_return_address() -> u64 {
    // The call pushed it, and nothing of ours has pushed since.
    core::arch::naked_asm!("mov rax, [rsp]", "ret")
}

/// As above, from the link register the call left it in.
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn gala_return_address() -> u64 {
    core::arch::naked_asm!("mov x0, x30", "ret")
}

/// Jumps straight on to `raiser` with `record`: a callee that reports
/// its *caller's* frame, which is what RtlRaiseException does with the
/// return address the call left it. The jump keeps the caller's frame,
/// so an exception raised this way arrives with the stack pointer that
/// call's return would have arrived with, and -- under
/// `imports_answers=` -- with the slot's return trap as the pc of the
/// frame it is raised at (user/iat_unwind_test). A tail jump and not a
/// call, so this function is in no frame of the walk that follows.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn gala_raise_at(_record: *mut u8, _raiser: usize) {
    // rcx is the record, which `raiser` takes as its own first
    // argument, and rdx is `raiser` itself.
    core::arch::naked_asm!("jmp rdx")
}

/// As above, with x0 the record and x1 the function to jump to.
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn gala_raise_at(_record: *mut u8, _raiser: usize) {
    core::arch::naked_asm!("br x1")
}

/// Adds four numbers: a callee with four arguments of its own, so that
/// a call through one import slot fills all four registers an import
/// trace can record (`imports_strings=`, user/import_strings_test).
/// None of them is read here, which is what lets the caller hand it an
/// address in no mapping.
#[unsafe(no_mangle)]
pub extern "C" fn gala_four(a: u64, b: u64, c: u64, d: u64) -> u64 {
    a.wrapping_add(b).wrapping_add(c).wrapping_add(d)
}

/// The text a filled buffer holds, which is what a reader of a trace
/// sees quoted at the return and nowhere at the call.
pub const FILLED: &[u8] = b"filled at the return";

/// Writes [`FILLED`] into the buffer it is handed and answers that
/// buffer: a call whose text is in what it wrote and not in what it was
/// given, which is what every Windows call that fills one is. `n` is
/// the buffer's length in characters; 0 where it is too short, as those
/// calls answer. What an import trace can read of such a call is at its
/// return and not at it (`imports_strings=`, user/import_strings_test).
///
/// # Safety
/// `out` must be writable for `n` characters, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gala_fill(out: *mut u16, n: u64) -> u64 {
    if out.is_null() || n <= FILLED.len() as u64 {
        return 0;
    }
    for (i, &byte) in FILLED.iter().enumerate() {
        // SAFETY: the caller's buffer of `n` characters, which the
        // check above leaves room in for the text and its terminator.
        unsafe { out.add(i).write(u16::from(byte)) };
    }
    // SAFETY: inside that same buffer, by the same check.
    unsafe { out.add(FILLED.len()).write(0) };
    out as u64
}

/// Number of DLL_PROCESS_ATTACH calls DllMain has seen.
#[unsafe(no_mangle)]
pub static mut GALA_ATTACH_COUNT: u64 = 0;

/// DllMain's lpvReserved at the last DLL_PROCESS_ATTACH: non-NULL when
/// the DLL was loaded with the program, NULL for LoadLibrary.
#[unsafe(no_mangle)]
pub static mut GALA_ATTACH_RESERVED: u64 = 0;

/// Entry point required by our link flags; a DllMain that counts attaches.
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, reason: u32, reserved: usize) -> i32 {
    if reason == 1 {
        // SAFETY: the loader calls DllMain with the loader lock held, so
        // nothing else touches the counter at the same time.
        unsafe {
            GALA_ATTACH_COUNT += 1;
            GALA_ATTACH_RESERVED = reserved as u64;
        }
    }
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
