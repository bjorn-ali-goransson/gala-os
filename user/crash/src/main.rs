//! Writes through a null pointer. With no exception handler the process
//! must end with STATUS_ACCESS_VIOLATION (0xC0000005), as on Windows.

#![no_std]
#![no_main]

fn main() -> u32 {
    ntrt::print("crash: writing to address 0\r\n");
    // SAFETY: deliberately invalid; this faults and never returns.
    unsafe { core::ptr::write_volatile(core::ptr::null_mut::<u32>(), 1) };
    ntrt::print("crash: still alive?!\r\n");
    1
}

ntrt::entry!(main);
