//! Kernel executable: the boot entry symbol and the panic handler. Everything
//! else lives in the `kernel` library so that it can be unit-tested on the host.

#![no_std]
#![no_main]

/// Entry point jumped to by Limine (see `ENTRY` in `linker.ld`).
#[unsafe(no_mangle)]
extern "C" fn kmain() -> ! {
    kernel::kernel_main()
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    kernel::panic(info)
}
