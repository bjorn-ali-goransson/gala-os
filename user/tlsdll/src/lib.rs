//! A DLL with static TLS, for tls_test: a TLS directory (`_tls_used`,
//! which the linker points the image's TLS data directory at) with a
//! template of two values and 16 zero-filled bytes, an index the loader
//! fills in, and a TLS callback; its DllMain counts notifications too.
//! The thread-local values are reached the way compilers do it:
//! gs:[0x58] (TEB.ThreadLocalStoragePointer)[_tls_index] + the offset in
//! the template.

#![no_std]
#![allow(non_upper_case_globals)]

use core::sync::atomic::{AtomicU32, Ordering};

/// The template: TLS_START, the values, then TLS_END marks its end.
#[used]
#[unsafe(link_section = ".tls$AAA")]
static TLS_START: u64 = 0x5050;
#[used]
#[unsafe(link_section = ".tls$B")]
static TLS_VALUES: [u64; 2] = [0x5151, 0x5252];
#[used]
#[unsafe(link_section = ".tls$ZZZ")]
static TLS_END: u8 = 0;

/// Set by the loader.
#[unsafe(no_mangle)]
pub static mut _tls_index: u32 = 0;

type Callback = extern "system" fn(usize, u32, usize);

/// IMAGE_TLS_DIRECTORY64.
#[repr(C)]
pub struct TlsDirectory {
    start: *const u8,
    end: *const u8,
    index: *const u32,
    callbacks: *const Option<Callback>,
    zero_fill: u32,
    characteristics: u32,
}

// SAFETY: read-only addresses.
unsafe impl Sync for TlsDirectory {}

struct Callbacks([Option<Callback>; 2]);

// SAFETY: read-only function pointers.
unsafe impl Sync for Callbacks {}

static CALLBACKS: Callbacks = Callbacks([Some(tls_callback), None]);

#[used]
#[unsafe(no_mangle)]
pub static _tls_used: TlsDirectory = TlsDirectory {
    start: (&raw const TLS_START).cast(),
    end: &raw const TLS_END,
    index: &raw const _tls_index,
    callbacks: CALLBACKS.0.as_ptr(),
    zero_fill: 16,
    characteristics: 0,
};

/// TLS callback and DllMain notifications seen, by reason (0..=3).
static CALLBACK_SEEN: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
static DLLMAIN_SEEN: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];

extern "system" fn tls_callback(_module: usize, reason: u32, _reserved: usize) {
    if let Some(c) = CALLBACK_SEEN.get(reason as usize) {
        c.fetch_add(1, Ordering::SeqCst);
    }
}

/// This thread's copy of 8-byte slot `i` of the TLS block: 0 is
/// TLS_START, 1 and 2 the values, 3 and 4 the zero fill (after the
/// template's end).
fn slot(i: usize) -> *mut u64 {
    let array: usize;
    // SAFETY: gs:[0x58] is TEB.ThreadLocalStoragePointer.
    unsafe { core::arch::asm!("mov {}, gs:[0x58]", out(reg) array, options(nostack, readonly)) };
    // SAFETY: the loader set the index and this thread's block.
    unsafe {
        let index = core::ptr::read_volatile(&raw const _tls_index) as usize;
        let block = *((array + 8 * index) as *const usize);
        let start = &raw const TLS_START as usize;
        let offset = match i {
            0 => 0,
            1 | 2 => &raw const TLS_VALUES as usize - start + 8 * (i - 1),
            _ => _tls_used.end as usize - start + 8 * (i - 3),
        };
        (block + offset) as *mut u64
    }
}

/// Slot `i` of this thread's TLS block (see `slot`).
#[unsafe(no_mangle)]
pub extern "C" fn tls_get(i: usize) -> u64 {
    // SAFETY: inside this thread's block.
    unsafe { slot(i).read_volatile() }
}

#[unsafe(no_mangle)]
pub extern "C" fn tls_set(i: usize, v: u64) {
    // SAFETY: inside this thread's block.
    unsafe { slot(i).write_volatile(v) }
}

/// TLS callbacks (`dllmain` 0) or DllMain calls (1) seen for `reason`.
#[unsafe(no_mangle)]
pub extern "C" fn tls_seen(dllmain: u32, reason: u32) -> u32 {
    let table = if dllmain != 0 { &DLLMAIN_SEEN } else { &CALLBACK_SEEN };
    table.get(reason as usize).map_or(0, |c| c.load(Ordering::SeqCst))
}

/// DllMain: counts each notification.
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, reason: u32, _reserved: usize) -> i32 {
    if let Some(c) = DLLMAIN_SEEN.get(reason as usize) {
        c.fetch_add(1, Ordering::SeqCst);
    }
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
