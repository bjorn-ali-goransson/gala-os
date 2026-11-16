//! gala's msi.dll: the Windows Installer functions programs ask before
//! (or instead of) installing anything, and the reading of an installer
//! database. What Windows Installer has installed, gala has not: there
//! is no installer database of its own and no installer service, so no
//! product or component is known. What the Office Deployment Tool's
//! setup.exe and its integrator.exe import (by ordinal: build.rs), as
//! Windows answers for products and components it does not have
//! (`msi_test`, `wow64_msi_test`; Wine's conformance tests, and the
//! answers measured on Windows 11 build 26200):
//! - MsiQueryProductStateW, MsiGetComponentPathW, MsiLocateComponentW:
//!   INSTALLSTATE_UNKNOWN, INSTALLSTATE_INVALIDARG for an argument that
//!   is missing or not a braced GUID;
//! - MsiEnumProductsW: no products (ERROR_NO_MORE_ITEMS);
//! - MsiConfigureProductExW: ERROR_UNKNOWN_PRODUCT;
//! - MsiProvideQualifiedComponentExW, MsiEnumComponentQualifiersW:
//!   ERROR_UNKNOWN_COMPONENT;
//! - MsiSetInternalUI: the process's UI level and window, kept.
//!
//! Each answer that stands for "Windows Installer has nothing here" is
//! recorded in the ledger (`msi.dll!<function>: nothing installed`, kind
//! "other", detail the call's ordinal), so a program that goes on to fail
//! because it expected an installed product says what it asked.
//!
//! A database on disk is a different matter: it is a file in a published
//! format, and reading one asks nothing of an installer. MsiOpenDatabaseW
//! opens one for reading and the view, cursor and record functions read
//! it (`db`, over galapkg's MSI reader) - which is what Office's
//! integrator.exe does to the Click-to-Run MSIs. Writing one is not
//! implemented: the create modes answer ERROR_CREATE_FAILED, which is
//! Windows' answer when it cannot create a database but not what Windows
//! does where it can, and a query that would change a database runs as
//! ERROR_FUNCTION_FAILED, which is Windows' own answer for a database
//! opened read-only. Both are recorded in the ledger as gaps (ROADMAP
//! OF).
//!
//! Installing a package is not implemented: gala has no installer
//! engine, that being galapkg's business (ROADMAP PK) rather than
//! msi.dll's. MsiInstallProductW is exported all the same, because an
//! absent export is worse than a refusal: it is delay-loaded, and a
//! delay load that cannot be resolved ends the process with
//! 0xc06d007f before it can handle anything, which is how Office's
//! integrator.exe died. It answers Windows' own code for every path
//! that is no package to begin with, and for one that is, the failure
//! of an install that cannot be done (ERROR_INSTALL_FAILURE) with the
//! package named in the ledger. The other installing functions stay
//! absent until a run shows one imported.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use gala_heap as _;

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

mod db;

// The 64-bit builds take the memory routines from gala's ntdll, which
// exports them.
#[cfg(not(target_arch = "x86"))]
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly (ntrt's
/// x86 runtime defines it on x86).
#[cfg(not(target_arch = "x86"))]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

// rpcrt4.dll, which Windows' msi imports (RpcStringFreeW, NdrClientCall3
// and the proxy and stub helpers; its import table, measured on 26100).
// gala's msi calls nothing in it, but the dependency is part of what a
// program sees: rpcrt4 is mapped in every process that loads msi, and
// programs read the module graph -- Click-to-Run's AppV ISV server takes
// rpcrt4's handle with GetModuleHandle and gives up with
// ERROR_MOD_NOT_FOUND when it is NULL (ARCHITECTURE "The module graph",
// ROADMAP OF; `kernel32_test`). gala has no 32-bit rpcrt4 yet, so the
// 32-bit build goes without it.
// Nothing here calls it: build.rs pins the import on the link command
// line, so the linker keeps it.
#[cfg(not(target_arch = "x86"))]
#[allow(dead_code)]
#[link(name = "rpcrt4", kind = "raw-dylib")]
unsafe extern "system" {
    fn RpcStringFreeW(text: *mut *mut u16) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn SetLastError(code: u32);
    fn ExitProcess(code: u32) -> !;
}

const ERROR_SUCCESS: u32 = 0;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_OPEN_FAILED: u32 = 110;
const ERROR_NO_MORE_ITEMS: u32 = 259;
const ERROR_UNKNOWN_PRODUCT: u32 = 1605;
const ERROR_UNKNOWN_COMPONENT: u32 = 1607;
const ERROR_INSTALL_FAILURE: u32 = 1603;
const ERROR_INSTALL_PACKAGE_OPEN_FAILED: u32 = 1619;
const ERROR_INSTALL_PACKAGE_INVALID: u32 = 1620;
const ERROR_CREATE_FAILED: u32 = 1631;

/// MSIDBOPEN_CREATE, the first of the persistence modes that make a
/// database rather than open one (MSIDBOPEN_CREATEDIRECT is the other);
/// MSIDBOPEN_PATCHFILE is a flag above them (msiquery.h). A mode is a
/// small integer cast to a string pointer, so a pointer above them is a
/// path to persist to, which is a database to create as well.
const MSIDBOPEN_CREATE: usize = 3;
const MSIDBOPEN_PATCHFILE: usize = 32;
const INSTALLSTATE_INVALIDARG: i32 = -2;
const INSTALLSTATE_UNKNOWN: i32 = -1;

// INSTALLUILEVEL: a level in the low nibble and flags above it.
const INSTALLUILEVEL_NOCHANGE: u32 = 0;
const INSTALLUILEVEL_DEFAULT: u32 = 1;
const INSTALLUILEVEL_FULL: u32 = 5;
/// INSTALLUILEVEL_ENDDIALOG, _PROGRESSONLY, _HIDECANCEL, _SOURCERESONLY,
/// _UACONLY.
const INSTALLUILEVEL_FLAGS: u32 = 0x80 | 0x40 | 0x20 | 0x100 | 0x200;

/// The process's internal UI level (MsiSetInternalUI) and window.
static UI_LEVEL: AtomicU32 = AtomicU32::new(INSTALLUILEVEL_DEFAULT);
static UI_WINDOW: AtomicUsize = AtomicUsize::new(0);

/// Records a gap in the ledger as `msi.dll!<name><tail>`, of the
/// ledger's kind `what` (2 unsupported, 5 other) with the call's
/// ordinal as the detail, so a program that fails because of the gap
/// says what it asked for.
fn record_gap(name: &[u8], tail: &[u8], what: u32, detail: u64) {
    let mut text = [0u8; 96];
    let mut n = 0;
    for part in [&b"msi.dll!"[..], name, tail] {
        let take = part.len().min(text.len() - n);
        text[n..n + take].copy_from_slice(&part[..take]);
        n += take;
    }
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(text.as_ptr(), n, what, detail) };
}

/// Records an answer of "nothing installed" (module comment).
fn nothing_installed(name: &[u8], ordinal: u64) {
    record_gap(name, b": nothing installed", 5, ordinal);
}

/// Whether `p` is a braced GUID string, `{XXXXXXXX-XXXX-XXXX-XXXX-
/// XXXXXXXXXXXX}` (hexadecimal digits of either case), as product and
/// component codes are.
fn is_guid(p: *const u16) -> bool {
    if p.is_null() {
        return false;
    }
    for i in 0..39 {
        // SAFETY: the caller's NUL-terminated string, read up to its NUL
        // (every unit before index 38 is checked to be non-zero).
        let c = unsafe { *p.add(i) };
        let ok = match i {
            0 => c == u16::from(b'{'),
            9 | 14 | 19 | 24 => c == u16::from(b'-'),
            37 => c == u16::from(b'}'),
            38 => c == 0,
            _ => u8::try_from(c).is_ok_and(|b| b.is_ascii_hexdigit()),
        };
        if !ok {
            return false;
        }
    }
    true
}

/// MsiOpenDatabaseW (#92): opens an installer database for reading.
/// Windows' answers, measured: a NULL path is ERROR_INVALID_PARAMETER
/// and leaves the handle alone; past that the handle is set to 0 first,
/// and an empty path or no handle at all is ERROR_INVALID_PARAMETER; a
/// path that is not there and a file that is not a database are both
/// ERROR_OPEN_FAILED. The read-only modes (MSIDBOPEN_READONLY,
/// _TRANSACT, _DIRECT) open the file for `db` to read; the create modes,
/// and a persist path above them, answer ERROR_CREATE_FAILED, since gala
/// has no engine to write a database with - the one place its answer is
/// not Windows' (`msi_test`, `wow64_msi_test`, ROADMAP OF).
#[unsafe(no_mangle)]
pub extern "system" fn MsiOpenDatabaseW(
    path: *const u16,
    persist: *const u16,
    database: *mut u32,
) -> u32 {
    if path.is_null() || database.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    // SAFETY: the caller's handle, not null.
    unsafe { *database = 0 };
    // SAFETY: the caller's string, its first unit.
    if unsafe { *path } == 0 {
        return ERROR_INVALID_PARAMETER;
    }
    if persist.addr() & !MSIDBOPEN_PATCHFILE >= MSIDBOPEN_CREATE {
        record_gap(b"MsiOpenDatabaseW", b": no engine to create a database", 2, 92);
        return ERROR_CREATE_FAILED;
    }
    match db::open(path) {
        Ok(handle) => {
            // SAFETY: the caller's handle, not null.
            unsafe { *database = handle };
            ERROR_SUCCESS
        }
        Err(e) => e,
    }
}

/// MsiInstallProductW (#88): installs a package, which gala cannot do.
/// Windows' answers, measured on build 26200, for every path that is no
/// package: ERROR_INVALID_PARAMETER for a NULL one, ERROR_PATH_NOT_FOUND
/// for an empty one, and for the rest what the file itself says
/// (`db::for_install`). A package gala does read is the gap: it answers
/// ERROR_INSTALL_FAILURE, Windows' own code for an install that failed,
/// and the ledger carries which package was asked for -- the one answer
/// here that is not Windows', since Windows would install it
/// (`msi_test`, ROADMAP OF).
#[unsafe(no_mangle)]
pub extern "system" fn MsiInstallProductW(package: *const u16, _command_line: *const u16) -> u32 {
    if package.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    // SAFETY: the caller's string, its first unit.
    if unsafe { *package } == 0 {
        return ERROR_PATH_NOT_FOUND;
    }
    match db::for_install(package) {
        Err(e) => e,
        Ok(()) => {
            record_gap(b"MsiInstallProductW", b": no engine to install a package", 2, 88);
            ERROR_INSTALL_FAILURE
        }
    }
}

/// MsiDatabaseIsTablePersistentW (#165): whether the database holds such
/// a table (`db`).
#[unsafe(no_mangle)]
pub extern "system" fn MsiDatabaseIsTablePersistentW(database: u32, table: *const u16) -> i32 {
    db::is_table_persistent(database, table)
}

/// MsiDatabaseOpenViewW (#32): a view over a query (`db`).
#[unsafe(no_mangle)]
pub extern "system" fn MsiDatabaseOpenViewW(
    database: u32,
    query: *const u16,
    view: *mut u32,
) -> u32 {
    db::open_view(database, query, view)
}

/// MsiViewExecute (#159): runs the view's query, which is where its rows
/// are read (`db`).
#[unsafe(no_mangle)]
pub extern "system" fn MsiViewExecute(view: u32, record: u32) -> u32 {
    db::execute(view, record)
}

/// MsiViewFetch (#160): the next row as a record handle (`db`).
#[unsafe(no_mangle)]
pub extern "system" fn MsiViewFetch(view: u32, record: *mut u32) -> u32 {
    db::fetch(view, record)
}

/// MsiViewClose (#158): drops the view's rows, keeping the view (`db`).
#[unsafe(no_mangle)]
pub extern "system" fn MsiViewClose(view: u32) -> u32 {
    db::close_view(view)
}

/// MsiRecordGetFieldCount (#115): how many fields the record has (`db`).
#[unsafe(no_mangle)]
pub extern "system" fn MsiRecordGetFieldCount(record: u32) -> u32 {
    db::field_count(record)
}

/// MsiRecordGetStringW (#118): a field as text (`db`).
#[unsafe(no_mangle)]
pub extern "system" fn MsiRecordGetStringW(
    record: u32,
    field: u32,
    buf: *mut u16,
    size: *mut u32,
) -> u32 {
    db::get_string(record, field, buf, size)
}

/// MsiRecordGetInteger (#116): a field as an integer, or
/// MSI_NULL_INTEGER (`db`).
#[unsafe(no_mangle)]
pub extern "system" fn MsiRecordGetInteger(record: u32, field: u32) -> i32 {
    db::get_integer(record, field)
}

/// MsiRecordIsNull (#119): whether a field holds no value (`db`).
#[unsafe(no_mangle)]
pub extern "system" fn MsiRecordIsNull(record: u32, field: u32) -> i32 {
    db::is_null(record, field)
}

/// MsiCloseHandle (#8): closes a database, view or record handle; 0
/// closes nothing and succeeds (`db`).
#[unsafe(no_mangle)]
pub extern "system" fn MsiCloseHandle(handle: u32) -> u32 {
    db::close(handle)
}

/// MsiSetInternalUI (#141): sets the process's UI level and returns the
/// previous one; INSTALLUILEVEL_NOCHANGE keeps it; a level above
/// INSTALLUILEVEL_FULL or an unknown flag returns INSTALLUILEVEL_NOCHANGE
/// and keeps the level, but its known flags still replace the current
/// ones (as Windows does: 0xdeadbeef after DEFAULT leaves DEFAULT with
/// flags 0x2e0, `msi_test`). A window passed is swapped with the previous.
#[unsafe(no_mangle)]
pub extern "system" fn MsiSetInternalUI(level: u32, window: *mut usize) -> u32 {
    if level & 0xf > INSTALLUILEVEL_FULL || level & !(0xf | INSTALLUILEVEL_FLAGS) != 0 {
        let current = UI_LEVEL.load(Ordering::Relaxed);
        UI_LEVEL.store(current & 0xf | level & INSTALLUILEVEL_FLAGS, Ordering::Relaxed);
        return INSTALLUILEVEL_NOCHANGE;
    }
    if !window.is_null() {
        // SAFETY: the caller's in/out window handle.
        unsafe { *window = UI_WINDOW.swap(*window, Ordering::Relaxed) };
    }
    if level & 0xf == INSTALLUILEVEL_NOCHANGE && level & INSTALLUILEVEL_FLAGS == 0 {
        return UI_LEVEL.load(Ordering::Relaxed);
    }
    UI_LEVEL.swap(level, Ordering::Relaxed)
}

/// MsiQueryProductStateW (#113): INSTALLSTATE_UNKNOWN for a product code,
/// INSTALLSTATE_INVALIDARG for anything else; the last error 0 either way
/// (as Windows answers a NULL code).
#[unsafe(no_mangle)]
pub extern "system" fn MsiQueryProductStateW(product: *const u16) -> i32 {
    // SAFETY: plain call.
    unsafe { SetLastError(ERROR_SUCCESS) };
    if !is_guid(product) {
        return INSTALLSTATE_INVALIDARG;
    }
    nothing_installed(b"MsiQueryProductStateW", 113);
    INSTALLSTATE_UNKNOWN
}

/// MsiEnumProductsW (#45): no products.
#[unsafe(no_mangle)]
pub extern "system" fn MsiEnumProductsW(index: u32, product: *mut u16) -> u32 {
    if product.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    if index == 0 {
        nothing_installed(b"MsiEnumProductsW", 45);
    }
    ERROR_NO_MORE_ITEMS
}

/// MsiGetComponentPathW (#173): INSTALLSTATE_UNKNOWN for a product and
/// component (the size untouched); INSTALLSTATE_INVALIDARG for a missing
/// or malformed code, or a buffer without its size.
#[unsafe(no_mangle)]
pub extern "system" fn MsiGetComponentPathW(
    product: *const u16,
    component: *const u16,
    path: *mut u16,
    size: *mut u32,
) -> i32 {
    if !is_guid(product) || !is_guid(component) || (!path.is_null() && size.is_null()) {
        return INSTALLSTATE_INVALIDARG;
    }
    nothing_installed(b"MsiGetComponentPathW", 173);
    INSTALLSTATE_UNKNOWN
}

/// MsiLocateComponentW (#90): as MsiGetComponentPathW without a product.
#[unsafe(no_mangle)]
pub extern "system" fn MsiLocateComponentW(
    component: *const u16,
    path: *mut u16,
    size: *mut u32,
) -> i32 {
    if !is_guid(component) || (!path.is_null() && size.is_null()) {
        return INSTALLSTATE_INVALIDARG;
    }
    nothing_installed(b"MsiLocateComponentW", 90);
    INSTALLSTATE_UNKNOWN
}

/// MsiConfigureProductExW (#190): ERROR_UNKNOWN_PRODUCT for a product
/// code, ERROR_INVALID_PARAMETER for anything else.
#[unsafe(no_mangle)]
pub extern "system" fn MsiConfigureProductExW(
    product: *const u16,
    _level: i32,
    _state: i32,
    _command_line: *const u16,
) -> u32 {
    if !is_guid(product) {
        return ERROR_INVALID_PARAMETER;
    }
    nothing_installed(b"MsiConfigureProductExW", 190);
    ERROR_UNKNOWN_PRODUCT
}

/// MsiProvideQualifiedComponentExW (#203): ERROR_UNKNOWN_COMPONENT (a
/// missing or malformed component is ERROR_INVALID_PARAMETER).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn MsiProvideQualifiedComponentExW(
    component: *const u16,
    _qualifier: *const u16,
    _mode: u32,
    _product: *const u16,
    _unused1: u32,
    _unused2: u32,
    _path: *mut u16,
    _size: *mut u32,
) -> u32 {
    if !is_guid(component) {
        return ERROR_INVALID_PARAMETER;
    }
    nothing_installed(b"MsiProvideQualifiedComponentExW", 203);
    ERROR_UNKNOWN_COMPONENT
}

/// MsiEnumComponentQualifiersW (#39): ERROR_UNKNOWN_COMPONENT (a missing
/// or malformed component is ERROR_INVALID_PARAMETER).
#[unsafe(no_mangle)]
pub extern "system" fn MsiEnumComponentQualifiersW(
    component: *const u16,
    _index: u32,
    _qualifier: *mut u16,
    _qualifier_size: *mut u32,
    _data: *mut u16,
    _data_size: *mut u32,
) -> u32 {
    if !is_guid(component) {
        return ERROR_INVALID_PARAMETER;
    }
    nothing_installed(b"MsiEnumComponentQualifiersW", 39);
    ERROR_UNKNOWN_COMPONENT
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

/// A panic cannot be recovered from here: it is recorded (so the run
/// says which file and line) and the process ends with
/// STATUS_STACK_BUFFER_OVERRUN (fail-fast), as gala's other DLLs do.
/// `alloc`'s own failure to allocate ends here too, which is why the
/// reading of a database reserves what it needs (`db`) instead of
/// leaving the heap to panic.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut text = [0u8; 80];
    let mut n = 0;
    let line = info.location().map_or(0, |l| {
        for &b in b"panic ".iter().chain(l.file().as_bytes()) {
            if n < text.len() {
                text[n] = b;
                n += 1;
            }
        }
        l.line()
    });
    record_gap(&text[..n], b"", 5, line.into());
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
