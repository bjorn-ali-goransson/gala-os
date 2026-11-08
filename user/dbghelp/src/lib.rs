//! gala's dbghelp.dll (debugging symbols; In-Memory OLTP looks up its
//! native code with them, SqlDumper writes crash dumps): ImageNtHeader
//! and SymSrvGetFileIndexInfoW (`image`); a symbol handler over the
//! modules' exports (`symbols`); StackWalk64 and its callbacks (`stack`).
//! MiniDumpWriteDump is not implemented yet. Each of those reports
//! itself to the missing-features ledger and ends the process when
//! called (`gala_stubs`), so the ledger shows which ones a program uses.

#![no_std]

mod image;
mod stack;

// ChromeDriver links these (crash stacks' lines and module names); none
// is called in a normal run.
gala_stubs::unimplemented_exports!("dbghelp.dll":
    SymGetLineFromAddr64
    SymGetModuleInfo64
    SymGetSearchPathW
    SymSetSearchPathW
);
mod symbols;

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

/// The thread's last error (TEB.LastErrorValue, x64 +0x68): dbghelp
/// imports only ntdll.
fn set_last_error(code: u32) {
    // SAFETY: this thread's TEB.
    unsafe { core::arch::asm!("mov gs:[0x68], {:e}", in(reg) code, options(nostack)) };
}

/// MiniDumpWriteDump(hProcess, ProcessId, hFile, DumpType, ...): gala
/// cannot write minidumps yet (reported, with the dump type; FALSE,
/// ERROR_NOT_SUPPORTED). Crash handlers (Chromium's Crashpad fallback
/// handler) then report the failure; gala's own crash report lines
/// still describe the crash.
#[unsafe(no_mangle)]
pub extern "system" fn MiniDumpWriteDump(
    _process: usize,
    _id: u32,
    _file: usize,
    kind: u32,
    _exception: usize,
    _user_streams: usize,
    _callback: usize,
) -> i32 {
    let name = b"dbghelp.dll!MiniDumpWriteDump";
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(name.as_ptr(), name.len(), 2, kind.into()) };
    set_last_error(50); // ERROR_NOT_SUPPORTED
    0
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
