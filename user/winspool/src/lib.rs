//! gala's winspool.drv (the print spooler's client). gala has no print
//! spooler and no printers, and the functions say so as Windows does on
//! a machine without printers: none enumerated, no default printer, no
//! printer to open. Delphi's VCL imports them (its Printers unit), which
//! Inno Setup's Setup links.

#![no_std]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_PRINTER_NAME: u32 = 1801;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn SetLastError(error: u32);
}

fn fail<T>(error: u32, value: T) -> T {
    // SAFETY: a plain call.
    unsafe { SetLastError(error) };
    value
}

/// EnumPrintersW(flags, name, level, buffer, size, needed, returned):
/// no printers: TRUE with nothing returned and nothing needed.
#[unsafe(no_mangle)]
pub extern "system" fn EnumPrintersW(
    _flags: u32,
    _name: *const u16,
    _level: u32,
    _buffer: *mut u8,
    _size: u32,
    needed: *mut u32,
    returned: *mut u32,
) -> i32 {
    if needed.is_null() || returned.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's outputs.
    unsafe {
        *needed = 0;
        *returned = 0;
    }
    1
}

/// GetDefaultPrinterW(buffer, size): there is no default printer
/// (ERROR_FILE_NOT_FOUND, as documented).
#[unsafe(no_mangle)]
pub extern "system" fn GetDefaultPrinterW(_buffer: *mut u16, size: *mut u32) -> i32 {
    if size.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    fail(ERROR_FILE_NOT_FOUND, 0)
}

/// OpenPrinterW(name, handle, defaults): no printer of any name
/// (ERROR_INVALID_PRINTER_NAME); the handle is set to NULL.
#[unsafe(no_mangle)]
pub extern "system" fn OpenPrinterW(
    _name: *const u16,
    handle: *mut usize,
    _defaults: *const u8,
) -> i32 {
    if !handle.is_null() {
        // SAFETY: the caller's output.
        unsafe { *handle = 0 };
    }
    fail(ERROR_INVALID_PRINTER_NAME, 0)
}

/// ClosePrinter(handle): OpenPrinterW never gives a handle, so none is
/// valid.
#[unsafe(no_mangle)]
pub extern "system" fn ClosePrinter(_handle: usize) -> i32 {
    fail(ERROR_INVALID_HANDLE, 0)
}

/// DocumentPropertiesW(hwnd, printer, name, out, in, mode): no printer
/// to ask (a negative result).
#[unsafe(no_mangle)]
pub extern "system" fn DocumentPropertiesW(
    _hwnd: usize,
    _printer: usize,
    _name: *const u16,
    _out: *mut u8,
    _in: *const u8,
    _mode: u32,
) -> i32 {
    fail(ERROR_INVALID_HANDLE, -1)
}

/// DllMain (the entry point our link flags name).
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
