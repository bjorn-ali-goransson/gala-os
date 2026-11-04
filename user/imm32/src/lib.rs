//! gala's imm32.dll (the Input Method Manager, imm.h): gala has no input
//! methods, so no keyboard layout is an IME.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

/// ImmGetIMEFileNameW(hKL, lpszFileName, uBufLen): 0, the layout is not
/// an IME.
#[unsafe(no_mangle)]
pub extern "system" fn ImmGetIMEFileNameW(_layout: usize, name: *mut u16, length: u32) -> u32 {
    if !name.is_null() && length > 0 {
        // SAFETY: the caller's buffer of at least one character.
        unsafe { *name = 0 };
    }
    0
}

/// ImmIsIME(hKL): FALSE.
#[unsafe(no_mangle)]
pub extern "system" fn ImmIsIME(_layout: usize) -> i32 {
    0
}

/// ImmGetContext(hWnd): no input context (NULL).
#[unsafe(no_mangle)]
pub extern "system" fn ImmGetContext(_hwnd: usize) -> usize {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn ImmReleaseContext(_hwnd: usize, _context: usize) -> i32 {
    1
}

/// ImmAssociateContextEx: nothing to associate; TRUE.
#[unsafe(no_mangle)]
pub extern "system" fn ImmAssociateContextEx(_hwnd: usize, _context: usize, _flags: u32) -> i32 {
    1
}

/// The DLL entry point: nothing to set up.
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
