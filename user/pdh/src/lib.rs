//! gala's pdh.dll (performance counters): real-time queries of the
//! processors' times (`query`); PdhGetRawCounterArrayW, which SQL Server
//! LocalDB imports, is not implemented yet: it reports itself to the
//! missing-features ledger and ends the process when called
//! (`gala_stubs`).

#![no_std]

mod query;
mod unimplemented;

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

/// Records an unsupported case in the missing-features ledger.
fn unsupported(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

/// The same with a name inside: `prefix` + `name` + ")", backslashes as
/// slashes (the ledger takes neither those nor quotes) and cut to the 64
/// bytes it keeps.
fn unsupported_named(prefix: &[u8], name: &[u8], detail: u64) {
    let mut text = [0u8; 64];
    let mut n = 0;
    for &b in prefix.iter().chain(name).chain(b")") {
        if n < text.len() {
            text[n] = match b {
                b'\\' => b'/',
                b'"' => b'\'',
                b => b,
            };
            n += 1;
        }
    }
    unsupported(&text[..n], detail);
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

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
