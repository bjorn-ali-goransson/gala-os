//! gala's dbghelp.dll (debugging symbols; In-Memory OLTP looks up its
//! native code with them): for now the functions SQL Server LocalDB
//! imports, none implemented yet. Each reports itself to the
//! missing-features ledger and ends the process when called
//! (`gala_stubs`), so the ledger shows which ones a program uses.

#![no_std]

mod unimplemented;

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(code: u32);
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
    // SAFETY: a valid buffer; plain calls.
    unsafe {
        GalaRecordMissing(name.as_ptr(), name.len(), 2, kind.into());
        SetLastError(50); // ERROR_NOT_SUPPORTED
    }
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
