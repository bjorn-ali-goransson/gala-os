//! A 32-bit (x86) DLL for WOW64's loader (`wow64_import_test`): stdcall
//! and cdecl exports, exported data, a pointer in its data that needs a
//! HIGHLOW base relocation (its preferred base is the executable's, so it
//! is always relocated: build.rs), static TLS with a callback, and a
//! DllMain that records its notifications and lpvReserved. The TLS values
//! are reached as compilers do: fs:[0x2c] (TEB32.ThreadLocalStoragePointer)
//! [_tls_index] + the offset in the template.

#![no_std]
#![allow(non_upper_case_globals)]

use core::sync::atomic::{AtomicU32, Ordering};

#[unsafe(no_mangle)]
pub static mut GALA_VALUE32: u32 = 0x1234_5678;

/// The absolute address of `GALA_VALUE32`: fixed up by base relocation.
#[unsafe(no_mangle)]
// A reference to the static mut itself is denied; this one points at it.
#[allow(clippy::deref_addrof)]
pub static GALA_VALUE32_PTR: &u32 = unsafe { &*(&raw const GALA_VALUE32) };

/// stdcall (the callee pops its arguments).
#[unsafe(no_mangle)]
pub extern "system" fn gala_add32(a: u32, b: u32) -> u32 {
    a.wrapping_add(b)
}

/// cdecl (the caller pops).
#[unsafe(no_mangle)]
pub extern "C" fn gala_sub32(a: u32, b: u32) -> u32 {
    a.wrapping_sub(b)
}

/// `GALA_VALUE32` read through the relocated pointer.
#[unsafe(no_mangle)]
pub extern "system" fn gala_get_value32() -> u32 {
    // SAFETY: the relocated pointer to our own static.
    unsafe { core::ptr::read_volatile(GALA_VALUE32_PTR) }
}

/// The TLS template: TLS_START, the values, then TLS_END marks its end.
#[used]
#[unsafe(link_section = ".tls$AAA")]
static TLS_START: u32 = 0x5050;
#[used]
#[unsafe(link_section = ".tls$B")]
static TLS_VALUES: [u32; 2] = [0x5151, 0x5252];
#[used]
#[unsafe(link_section = ".tls$ZZZ")]
static TLS_END: u8 = 0;

/// Set by the loader.
#[unsafe(no_mangle)]
pub static mut _tls_index: u32 = 0;

type Callback = extern "system" fn(usize, u32, usize);

/// IMAGE_TLS_DIRECTORY32.
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
    zero_fill: 8,
    characteristics: 0,
};

/// TLS callback and DllMain notifications seen, by reason (0..=3).
static CALLBACK_SEEN: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
static DLLMAIN_SEEN: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
/// Notifications in order: 1 for a TLS callback's DLL_PROCESS_ATTACH, 2
/// for DllMain's (the callback must come first).
static ORDER: AtomicU32 = AtomicU32::new(0);
/// DllMain's lpvReserved at DLL_PROCESS_ATTACH (non-null when loaded with
/// the program).
static ATTACH_RESERVED: AtomicU32 = AtomicU32::new(0);

extern "system" fn tls_callback(_module: usize, reason: u32, _reserved: usize) {
    if let Some(c) = CALLBACK_SEEN.get(reason as usize) {
        c.fetch_add(1, Ordering::SeqCst);
    }
    if reason == 1 {
        let _ = ORDER.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst);
    }
}

/// This thread's copy of 4-byte slot `i` of the TLS block: 0 is
/// TLS_START, 1 and 2 the values, 3 and 4 the zero fill.
fn slot(i: usize) -> *mut u32 {
    let array: usize;
    // SAFETY: fs:[0x2c] is TEB32.ThreadLocalStoragePointer.
    unsafe { core::arch::asm!("mov {}, fs:[0x2c]", out(reg) array, options(nostack, readonly)) };
    // SAFETY: the loader set the index and this thread's block.
    unsafe {
        let index = core::ptr::read_volatile(&raw const _tls_index) as usize;
        let block = *((array + 4 * index) as *const usize);
        let start = &raw const TLS_START as usize;
        let offset = match i {
            0 => 0,
            1 | 2 => &raw const TLS_VALUES as usize - start + 4 * (i - 1),
            _ => _tls_used.end as usize - start + 4 * (i - 3),
        };
        (block + offset) as *mut u32
    }
}

/// Slot `i` of this thread's TLS block (see `slot`).
#[unsafe(no_mangle)]
pub extern "system" fn gala_tls_get32(i: usize) -> u32 {
    // SAFETY: inside this thread's block.
    unsafe { slot(i).read_volatile() }
}

/// TLS callbacks (`dllmain` 0) or DllMain calls (1) seen for `reason`.
#[unsafe(no_mangle)]
pub extern "system" fn gala_seen32(dllmain: u32, reason: u32) -> u32 {
    let table = if dllmain != 0 { &DLLMAIN_SEEN } else { &CALLBACK_SEEN };
    table.get(reason as usize).map_or(0, |c| c.load(Ordering::SeqCst))
}

/// 2 if the TLS callback's DLL_PROCESS_ATTACH came before DllMain's.
#[unsafe(no_mangle)]
pub extern "system" fn gala_order32() -> u32 {
    ORDER.load(Ordering::SeqCst)
}

/// DllMain's lpvReserved at DLL_PROCESS_ATTACH.
#[unsafe(no_mangle)]
pub extern "system" fn gala_attach_reserved32() -> u32 {
    ATTACH_RESERVED.load(Ordering::SeqCst)
}

/// DllMain (the image's entry point): counts each notification.
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, reason: u32, reserved: usize) -> i32 {
    if let Some(c) = DLLMAIN_SEEN.get(reason as usize) {
        c.fetch_add(1, Ordering::SeqCst);
    }
    if reason == 1 {
        ATTACH_RESERVED.store(reserved as u32, Ordering::SeqCst);
        let _ = ORDER.compare_exchange(1, 2, Ordering::SeqCst, Ordering::SeqCst);
    }
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
