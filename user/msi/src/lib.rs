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
//! Word's shared code (MSO.DLL, Mso30win32client.dll,
//! Mso40UIwin32client.dll, Mso98win32client.dll) delay-loads eleven
//! more, which answer the same way for what is not installed:
//! MsiQueryFeatureStateW, MsiUseFeatureW and MsiUseFeatureExW
//! (INSTALLSTATE_UNKNOWN), MsiReinstallFeatureW
//! (ERROR_UNKNOWN_PRODUCT), MsiGetUserInfoW (USERINFOSTATE_UNKNOWN),
//! MsiEnumClientsW, MsiGetProductCodeW, MsiGetProductCodeA and
//! MsiProvideQualifiedComponentW (ERROR_UNKNOWN_COMPONENT),
//! MsiGetComponentPathA (as the W form) -- and MsiSetExternalUIW, which
//! is not an answer about an install but a handler for one: it keeps
//! what it is given for the process and gives back the handler set
//! before it. Nothing calls that handler, because nothing here installs
//! anything (MsiInstallProductW below); a program learns that from the
//! install's own failure, not from the handler's silence.
//!
//! Each answer that stands for "Windows Installer has nothing here" is
//! recorded in the ledger, and the row names the codes the call was
//! asked about: `msi.dll!MsiGetProductCodeW({A9D5E2C4-...-2E8D7B1F4A03}):
//! nothing installed`, kind "other", detail the call's ordinal. The
//! ledger keys its rows on that text, so one run lists every component
//! a program wanted instead of counting hits on a row that says only
//! that something was asked -- Word resolves its resource files through
//! Installer components, and the row is what says which ones. A code is
//! named as the caller spelled it (38 characters at most, a braced
//! GUID's length), a feature name or qualifier up to 32, and anything
//! outside printable ASCII as `?`.
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
const USERINFOSTATE_INVALIDARG: i32 = -2;
const USERINFOSTATE_UNKNOWN: i32 = -1;

/// INSTALLMODE_NODETECTION, the only install mode MsiUseFeatureExW takes
/// (msiquery.h); another is INSTALLSTATE_INVALIDARG, measured with
/// INSTALLMODE_EXISTING, which the qualified-component calls do take
/// (`msi_test`).
const INSTALLMODE_NODETECTION: u32 = -2i32 as u32;

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
    let mut row = Row::new(name);
    row.put(tail);
    row.record(what, detail);
}

/// The longest row the ledger keeps whole (its `USER_NAME_LEN`).
const ROW_LEN: usize = 160;

/// How much of a code a row names: a braced GUID's 38 characters.
const CODE_LEN: usize = 38;

/// How much of a feature name or qualifier a row names. Neither is a
/// GUID or bounded by anything, and the codes beside them have to fit in
/// the same row.
const NAME_LEN: usize = 32;

/// A gap row being built: `msi.dll!<function>` and whatever the call
/// names after it.
struct Row {
    text: [u8; ROW_LEN],
    n: usize,
}

impl Row {
    fn new(name: &[u8]) -> Self {
        let mut row = Row { text: [0; ROW_LEN], n: 0 };
        row.put(b"msi.dll!");
        row.put(name);
        row
    }

    /// Appends as much of `part` as there is room for.
    fn put(&mut self, part: &[u8]) {
        let take = part.len().min(self.text.len() - self.n);
        self.text[self.n..self.n + take].copy_from_slice(&part[..take]);
        self.n += take;
    }

    /// Appends a string the caller named, `cap` bytes of it at most,
    /// with anything outside printable ASCII written as `?` so the row
    /// stays one readable line. One body for both string widths, as
    /// `is_guid`: the ANSI functions name their own bytes.
    fn put_asked<T: Copy + Into<u32>>(&mut self, p: *const T, cap: usize) {
        if p.is_null() {
            self.put(b"null");
            return;
        }
        for i in 0..cap {
            // SAFETY: the caller's NUL-terminated string, read no
            // further than its NUL.
            let c: u32 = unsafe { *p.add(i) }.into();
            if c == 0 {
                return;
            }
            self.put(&[if (0x20..0x7f).contains(&c) { c as u8 } else { b'?' }]);
        }
    }

    /// Records the row (`GalaRecordMissing`'s kind and detail).
    fn record(&self, what: u32, detail: u64) {
        // SAFETY: a valid buffer; the service only reads it.
        unsafe { GalaRecordMissing(self.text.as_ptr(), self.n, what, detail) };
    }
}

/// Records an answer of "nothing installed" (module comment) for a call
/// that was asked about nothing in particular.
fn nothing_installed(name: &[u8], ordinal: u64) {
    record_gap(name, b": nothing installed", 5, ordinal);
}

/// The same, naming the one code the call was asked about.
fn asked_about<T: Copy + Into<u32>>(name: &[u8], ordinal: u64, code: *const T) {
    let mut row = Row::new(name);
    row.put(b"(");
    row.put_asked(code, CODE_LEN);
    row.put(b"): nothing installed");
    row.record(5, ordinal);
}

/// The same for a call asked about two things -- a product and the
/// component, feature or qualifier under it -- named in the call's own
/// order and parted by a space, as the ledger's other rows do
/// (`gdi32.dll!D3DKMTQueryAdapterInfo(QUERYREGISTRY VulkanDriverName)`).
fn asked_about_two<T: Copy + Into<u32>>(
    name: &[u8],
    ordinal: u64,
    first: *const T,
    second: *const T,
    second_cap: usize,
) {
    let mut row = Row::new(name);
    row.put(b"(");
    row.put_asked(first, CODE_LEN);
    row.put(b" ");
    row.put_asked(second, second_cap);
    row.put(b"): nothing installed");
    row.record(5, ordinal);
}

/// Whether `p` is a braced GUID string, `{XXXXXXXX-XXXX-XXXX-XXXX-
/// XXXXXXXXXXXX}` (hexadecimal digits of either case), as product and
/// component codes are. One body for both string widths: the ANSI
/// functions ask it of their own bytes.
fn is_guid<T: Copy + Into<u32>>(p: *const T) -> bool {
    if p.is_null() {
        return false;
    }
    for i in 0..39 {
        // SAFETY: the caller's NUL-terminated string, read up to its NUL
        // (every unit before index 38 is checked to be non-zero).
        let c: u32 = unsafe { *p.add(i) }.into();
        let ok = match i {
            0 => c == u32::from(b'{'),
            9 | 14 | 19 | 24 => c == u32::from(b'-'),
            37 => c == u32::from(b'}'),
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
    asked_about(b"MsiQueryProductStateW", 113, product);
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
    asked_about_two(b"MsiGetComponentPathW", 173, product, component, CODE_LEN);
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
    asked_about(b"MsiLocateComponentW", 90, component);
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
    asked_about(b"MsiConfigureProductExW", 190, product);
    ERROR_UNKNOWN_PRODUCT
}

/// MsiProvideQualifiedComponentExW (#203): ERROR_UNKNOWN_COMPONENT (a
/// missing or malformed component is ERROR_INVALID_PARAMETER).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn MsiProvideQualifiedComponentExW(
    component: *const u16,
    qualifier: *const u16,
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
    asked_about_two(b"MsiProvideQualifiedComponentExW", 203, component, qualifier, NAME_LEN);
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
    asked_about(b"MsiEnumComponentQualifiersW", 39, component);
    ERROR_UNKNOWN_COMPONENT
}

/// The process's external UI handler (MsiSetExternalUIW): the handler,
/// the messages it asked for and the context it is to be handed back.
/// Kept as plain addresses, since nothing here reads them to call: the
/// handler hears from a running install, and gala runs none (module
/// comment).
static EXTERNAL_UI: AtomicUsize = AtomicUsize::new(0);
static EXTERNAL_UI_FILTER: AtomicU32 = AtomicU32::new(0);
static EXTERNAL_UI_CONTEXT: AtomicUsize = AtomicUsize::new(0);

/// MsiSetExternalUIW (#137): keeps the handler, its message filter and
/// its context for the process, and gives back the handler set before it
/// -- 0 in a process that has set none, which is what Word's mso chain
/// reads to find out whether it had one, and what it hands back to put
/// the old one in place. A NULL handler takes the current one away. The
/// handler is taken and returned as an address: ABI-wise that is the
/// INSTALLUI_HANDLERW pointer itself, and an address is what this can
/// honestly promise about it, nothing here being able to call it.
#[unsafe(no_mangle)]
pub extern "system" fn MsiSetExternalUIW(handler: usize, filter: u32, context: *mut u8) -> usize {
    EXTERNAL_UI_FILTER.store(filter, Ordering::Relaxed);
    EXTERNAL_UI_CONTEXT.store(context.addr(), Ordering::Relaxed);
    EXTERNAL_UI.swap(handler, Ordering::Relaxed)
}

/// What the three feature calls answer for a product nothing installed:
/// INSTALLSTATE_INVALIDARG for a product that is not a braced GUID or a
/// feature that is not there, INSTALLSTATE_UNKNOWN otherwise.
fn feature_state(product: *const u16, feature: *const u16, name: &[u8], ordinal: u64) -> i32 {
    if !is_guid(product) || feature.is_null() {
        return INSTALLSTATE_INVALIDARG;
    }
    asked_about_two(name, ordinal, product, feature, NAME_LEN);
    INSTALLSTATE_UNKNOWN
}

/// MsiQueryFeatureStateW (#111): whether a feature of a product is
/// installed -- INSTALLSTATE_UNKNOWN, no product being.
#[unsafe(no_mangle)]
pub extern "system" fn MsiQueryFeatureStateW(product: *const u16, feature: *const u16) -> i32 {
    feature_state(product, feature, b"MsiQueryFeatureStateW", 111)
}

/// MsiUseFeatureW (#155): as MsiQueryFeatureStateW, and on Windows it
/// also counts the use of the feature it finds. There is none to count.
#[unsafe(no_mangle)]
pub extern "system" fn MsiUseFeatureW(product: *const u16, feature: *const u16) -> i32 {
    feature_state(product, feature, b"MsiUseFeatureW", 155)
}

/// MsiUseFeatureExW (#193): MsiUseFeatureW with an install mode, which
/// must be INSTALLMODE_NODETECTION.
#[unsafe(no_mangle)]
pub extern "system" fn MsiUseFeatureExW(
    product: *const u16,
    feature: *const u16,
    mode: u32,
    _reserved: u32,
) -> i32 {
    if mode != INSTALLMODE_NODETECTION {
        return INSTALLSTATE_INVALIDARG;
    }
    feature_state(product, feature, b"MsiUseFeatureExW", 193)
}

/// MsiReinstallFeatureW (#129): ERROR_UNKNOWN_PRODUCT for a product
/// code and a feature, ERROR_INVALID_PARAMETER without either.
#[unsafe(no_mangle)]
pub extern "system" fn MsiReinstallFeatureW(
    product: *const u16,
    feature: *const u16,
    _mode: u32,
) -> u32 {
    if !is_guid(product) || feature.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    asked_about_two(b"MsiReinstallFeatureW", 129, product, feature, NAME_LEN);
    ERROR_UNKNOWN_PRODUCT
}

/// MsiGetUserInfoW (#82): who a product was registered to --
/// USERINFOSTATE_UNKNOWN, none being installed. Every one of the three
/// sizes is required, with or without its buffer
/// (USERINFOSTATE_INVALIDARG without one, measured), as is a product
/// code.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn MsiGetUserInfoW(
    product: *const u16,
    _user: *mut u16,
    user_size: *mut u32,
    _organization: *mut u16,
    organization_size: *mut u32,
    _serial: *mut u16,
    serial_size: *mut u32,
) -> i32 {
    if !is_guid(product)
        || user_size.is_null()
        || organization_size.is_null()
        || serial_size.is_null()
    {
        return USERINFOSTATE_INVALIDARG;
    }
    asked_about(b"MsiGetUserInfoW", 82, product);
    USERINFOSTATE_UNKNOWN
}

/// MsiEnumClientsW (#37): the products that use a component --
/// ERROR_UNKNOWN_COMPONENT, no component being installed, whatever the
/// index.
#[unsafe(no_mangle)]
pub extern "system" fn MsiEnumClientsW(
    component: *const u16,
    _index: u32,
    product: *mut u16,
) -> u32 {
    if !is_guid(component) || product.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    asked_about(b"MsiEnumClientsW", 37, component);
    ERROR_UNKNOWN_COMPONENT
}

/// MsiGetProductCodeW (#66): which product installed a component --
/// ERROR_UNKNOWN_COMPONENT, none having.
#[unsafe(no_mangle)]
pub extern "system" fn MsiGetProductCodeW(component: *const u16, product: *mut u16) -> u32 {
    if !is_guid(component) || product.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    asked_about(b"MsiGetProductCodeW", 66, component);
    ERROR_UNKNOWN_COMPONENT
}

/// MsiGetProductCodeA (#65): the ANSI form, asked of ANSI strings.
#[unsafe(no_mangle)]
pub extern "system" fn MsiGetProductCodeA(component: *const u8, product: *mut u8) -> u32 {
    if !is_guid(component) || product.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    asked_about(b"MsiGetProductCodeA", 65, component);
    ERROR_UNKNOWN_COMPONENT
}

/// MsiProvideQualifiedComponentW (#109): MsiProvideQualifiedComponentExW
/// without a product to charge the use to -- ERROR_UNKNOWN_COMPONENT.
#[unsafe(no_mangle)]
pub extern "system" fn MsiProvideQualifiedComponentW(
    component: *const u16,
    qualifier: *const u16,
    _mode: u32,
    _path: *mut u16,
    _size: *mut u32,
) -> u32 {
    if !is_guid(component) {
        return ERROR_INVALID_PARAMETER;
    }
    asked_about_two(b"MsiProvideQualifiedComponentW", 109, component, qualifier, NAME_LEN);
    ERROR_UNKNOWN_COMPONENT
}

/// MsiGetComponentPathA (#172): MsiGetComponentPathW of ANSI strings,
/// with the same answers.
#[unsafe(no_mangle)]
pub extern "system" fn MsiGetComponentPathA(
    product: *const u8,
    component: *const u8,
    path: *mut u8,
    size: *mut u32,
) -> i32 {
    if !is_guid(product) || !is_guid(component) || (!path.is_null() && size.is_null()) {
        return INSTALLSTATE_INVALIDARG;
    }
    asked_about_two(b"MsiGetComponentPathA", 172, product, component, CODE_LEN);
    INSTALLSTATE_UNKNOWN
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
