//! A DLL of known contents for `imagetrim_test`: 1 MiB of read-only data
//! whose every word is a function of its index (`TRIM_DATA`), and a table
//! of pointers into it (`TRIM_PTRS`), which the loader relocates when the
//! DLL is not at its preferred base. The test reads both before and after
//! memory pressure: pages an image drops are built again with the same
//! bytes (ROADMAP OF, the memory).

#![no_std]

/// Words of `TRIM_DATA` (1 MiB: 256 pages).
pub const WORDS: usize = 256 * 1024;
/// Entries of `TRIM_PTRS`, one per 16 words (128 KiB on x64).
pub const PTRS: usize = WORDS / 16;

/// The word at index `i` (the test computes the same).
pub const fn word(i: usize) -> u32 {
    (i as u32).wrapping_mul(2_654_435_761) ^ 0x5a5a_0f0f
}

const fn data() -> [u32; WORDS] {
    let mut out = [0u32; WORDS];
    let mut i = 0;
    while i < WORDS {
        out[i] = word(i);
        i += 1;
    }
    out
}

#[unsafe(no_mangle)]
pub static TRIM_DATA: [u32; WORDS] = data();

const fn pointers() -> [&'static u32; PTRS] {
    let mut out = [&TRIM_DATA[0]; PTRS];
    let mut i = 0;
    while i < PTRS {
        out[i] = &TRIM_DATA[i * 16];
        i += 1;
    }
    out
}

/// Pointers to every 16th word of `TRIM_DATA`: each one a base
/// relocation.
#[unsafe(no_mangle)]
pub static TRIM_PTRS: [&u32; PTRS] = pointers();

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
