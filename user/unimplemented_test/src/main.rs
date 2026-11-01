//! An executable calling a function gala exports but does not implement
//! yet (xolehlp.dll's DtcGetTransactionManagerExW, which SQL Server
//! imports). Such exports exist so that programs load (Windows would bind
//! the import); calling one cannot fail honestly without knowing what the
//! function returns, so it reports `dll!function` to the kernel's
//! missing-features ledger and ends the process with
//! STATUS_NOT_IMPLEMENTED (0xc0000002). The boot scenario checks both.
//! GetProcAddress does not return such a stub (ERROR_PROC_NOT_FOUND), so
//! programs probing for optional functions keep taking their fallbacks.

#![no_std]
#![no_main]

#[link(name = "xolehlp", kind = "raw-dylib")]
unsafe extern "system" {
    fn DtcGetTransactionManagerExW(
        host: *const u16,
        tm: *const u16,
        iid: *const u8,
        options: u32,
        config: *mut u8,
        out: *mut *mut u8,
    ) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn ExitProcess(code: u32) -> !;
    fn GetModuleHandleA(name: *const u8) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
    fn GetLastError() -> u32;
}

const ERROR_PROC_NOT_FOUND: u32 = 127;

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    // SAFETY: NUL-terminated names.
    unsafe {
        let module = GetModuleHandleA(c"xolehlp.dll".as_ptr().cast());
        if module == 0 {
            ExitProcess(2);
        }
        if GetProcAddress(module, c"DtcGetTransactionManagerExW".as_ptr().cast()) != 0 {
            ExitProcess(3);
        }
        if GetLastError() != ERROR_PROC_NOT_FOUND {
            ExitProcess(4);
        }
    }
    let mut out = core::ptr::null_mut();
    // SAFETY: a call with null optional arguments; on gala it does not
    // return. If it does, the exit status (1) shows it.
    unsafe {
        let _ = DtcGetTransactionManagerExW(
            core::ptr::null(),
            core::ptr::null(),
            core::ptr::null(),
            0,
            core::ptr::null_mut(),
            &mut out,
        );
        ExitProcess(1)
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
