//! An executable importing functions kernel32.dll does not export. As on
//! Windows, the process must not start: it ends with
//! STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139) before its entry point runs.
//! On gala the loader also reports every missing import to the kernel's
//! missing-features ledger (the boot scenario checks both names).

#![no_std]
#![no_main]

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaNoSuchFunctionOne() -> u32;
    fn GalaNoSuchFunctionTwo() -> u32;
    fn ExitProcess(code: u32) -> !;
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    // SAFETY: never reached if the loader behaves; if it is, the exit
    // status (1) shows that the imports were wrongly bound.
    unsafe {
        let _ = GalaNoSuchFunctionOne() + GalaNoSuchFunctionTwo();
        ExitProcess(1)
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
