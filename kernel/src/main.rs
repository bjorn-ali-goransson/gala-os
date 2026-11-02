//! Kernel executable: the boot entry symbol and the panic handler. Everything
//! else lives in the `kernel` library so that it can be unit-tested on the host.

#![no_std]
#![no_main]

/// Entry point jumped to by Limine (see `ENTRY` in `linker.ld`), with
/// these registers zero, or by the previous kernel after a kernel handover
/// (magic, the handover record's physical address, the HHDM offset).
#[unsafe(no_mangle)]
extern "C" fn kmain(magic: u64, record: u64, hhdm_offset: u64) -> ! {
    kernel::kernel_main(kernel::boot::handover_entry(magic, record, hhdm_offset))
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    kernel::panic(info)
}
