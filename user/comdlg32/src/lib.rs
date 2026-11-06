//! gala's comdlg32.dll, the common dialogs. gala draws none of them yet:
//! what does not need a dialog is done (GetFileTitleW, the parameter
//! checks every dialog function makes first, CommDlgExtendedError, the
//! default printer of PrintDlgW: gala has no printers), and a call that
//! would show a dialog fails as Windows documents a dialog that cannot be
//! created (CDERR_DIALOGFAILURE) and is recorded in the ledger. The rest
//! are stubs (`unimplemented.rs`) that report themselves to the ledger
//! and end the process when called.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod unimplemented;

use core::sync::atomic::{AtomicU32, Ordering};

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

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn TlsAlloc() -> u32;
    fn TlsGetValue(index: u32) -> usize;
    fn TlsSetValue(index: u32, value: usize) -> i32;
    fn FindResourceW(module: usize, name: *const u16, kind: *const u16) -> usize;
    fn LoadLibraryW(name: *const u16) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

/// Reports `what` to the ledger as unsupported, with `detail`.
fn unsupported(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; 2 is GalaRecordMissing's "unsupported".
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

const CDERR_STRUCTSIZE: u32 = 0x0001;
const CDERR_INITIALIZATION: u32 = 0x0002;
const CDERR_NOHINSTANCE: u32 = 0x0004;
const CDERR_FINDRESFAILURE: u32 = 0x0006;
const CDERR_NOHOOK: u32 = 0x000b;
const CDERR_DIALOGFAILURE: u32 = 0xffff;
const PDERR_NODEFAULTPRN: u32 = 0x1008;
const FRERR_BUFFERLENGTHZERO: u32 = 0x4001;
/// RT_DIALOG as a MAKEINTRESOURCE pointer.
const RT_DIALOG: *const u16 = 5 as *const u16;

// ---- CommDlgExtendedError: the calling thread's last dialog error ----

const TLS_OUT_OF_INDEXES: u32 = u32::MAX;
/// The TLS slot holding each thread's error (+1: 0 is "not allocated").
static SLOT: AtomicU32 = AtomicU32::new(0);

fn slot() -> Option<u32> {
    let s = SLOT.load(Ordering::Acquire);
    if s != 0 {
        return Some(s - 1);
    }
    // SAFETY: no arguments.
    let index = unsafe { TlsAlloc() };
    if index == TLS_OUT_OF_INDEXES {
        return None;
    }
    match SLOT.compare_exchange(0, index + 1, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => Some(index),
        // Another thread allocated one first (ours stays unused).
        Err(other) => Some(other - 1),
    }
}

/// Sets the thread's dialog error; FALSE (0) for the caller to return.
fn set_error(code: u32) -> i32 {
    if let Some(index) = slot() {
        // SAFETY: our TLS slot.
        unsafe { TlsSetValue(index, code as usize) };
    }
    0
}

/// CommDlgExtendedError: the error of the thread's last common dialog
/// call (0: none, or the user cancelled).
#[unsafe(no_mangle)]
pub extern "system" fn CommDlgExtendedError() -> u32 {
    // SAFETY: our TLS slot.
    slot().map_or(0, |index| unsafe { TlsGetValue(index) } as u32)
}

/// A dialog gala cannot show: CDERR_DIALOGFAILURE (Windows' "the dialog
/// box could not be created"), recorded under `what`.
fn no_dialog(what: &[u8]) -> i32 {
    unsupported(what, 0);
    set_error(CDERR_DIALOGFAILURE)
}

/// The template checks every dialog makes: a hook asked for but missing
/// (CDERR_NOHOOK), a template handle without hInstance
/// (CDERR_NOHINSTANCE), a template resource the module lacks
/// (CDERR_FINDRESFAILURE). `flags`' bits for hook, template and template
/// handle are given.
fn template_error(
    flags: u32,
    bits: [u32; 3],
    hook: usize,
    instance: usize,
    template: *const u16,
) -> Option<u32> {
    let [enable_hook, enable_template, enable_handle] = bits;
    if flags & enable_hook != 0 && hook == 0 {
        return Some(CDERR_NOHOOK);
    }
    if flags & enable_handle != 0 {
        return (instance == 0).then_some(CDERR_NOHINSTANCE);
    }
    if flags & enable_template != 0 {
        // SAFETY: FindResourceW checks its arguments.
        if instance == 0
            || template.is_null()
            || unsafe { FindResourceW(instance, template, RT_DIALOG) } == 0
        {
            return Some(CDERR_FINDRESFAILURE);
        }
    }
    None
}

// ---- GetFileTitleW ----

/// GetFileTitleW: the file's name without its directory, in `title`
/// (`size` characters): 0; the size needed (NUL included) when `title`
/// is NULL or too small; -1 for a name that is no file (empty, or ending
/// in a separator or a colon, or with characters no file name may hold).
/// The extension is kept: gala's shell has no preference to hide it.
#[unsafe(no_mangle)]
pub extern "system" fn GetFileTitleW(file: *const u16, title: *mut u16, size: u16) -> i16 {
    if file.is_null() {
        return -1;
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    while unsafe { *file.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: `n` units of the caller's string.
    let path = unsafe { core::slice::from_raw_parts(file, n) };
    let is = |c: u16, s: &str| s.encode_utf16().any(|x| x == c);
    let start = path.iter().rposition(|&c| is(c, "\\/:")).map_or(0, |i| i + 1);
    let name = &path[start..];
    if name.is_empty() || name.iter().any(|&c| c < 0x20 || is(c, "*?\"<>|")) {
        return -1;
    }
    let needed = name.len() + 1;
    if title.is_null() || usize::from(size) < needed {
        return needed as i16;
    }
    // SAFETY: the caller's buffer of `size` >= needed characters.
    unsafe {
        core::ptr::copy_nonoverlapping(name.as_ptr(), title, name.len());
        *title.add(name.len()) = 0;
    }
    0
}

// ---- PrintDlgW ----

const PRINTDLGW_SIZE: u32 = 120;
const PD_RETURNDEFAULT: u32 = 0x400;

/// PrintDlgW: NULL is CDERR_INITIALIZATION, a wrong lStructSize
/// CDERR_STRUCTSIZE. gala's winspool has no printers: with no default
/// printer both the dialog and PD_RETURNDEFAULT end in PDERR_NODEFAULTPRN,
/// as on a Windows without printers. (Were there one, the dialog is
/// recorded and CDERR_DIALOGFAILURE.)
#[unsafe(no_mangle)]
pub extern "system" fn PrintDlgW(pd: *mut u8) -> i32 {
    if pd.is_null() {
        return set_error(CDERR_INITIALIZATION);
    }
    // SAFETY: the caller's PRINTDLGW: its size first, then (checked) the rest.
    let size = unsafe { (pd as *const u32).read_unaligned() };
    if size != PRINTDLGW_SIZE {
        return set_error(CDERR_STRUCTSIZE);
    }
    // SAFETY: Flags at offset 40 of a PRINTDLGW (size checked).
    let flags = unsafe { (pd.add(40) as *const u32).read_unaligned() };
    if !has_default_printer() {
        return set_error(PDERR_NODEFAULTPRN);
    }
    if flags & PD_RETURNDEFAULT != 0 {
        return no_dialog(b"comdlg32.dll!PrintDlgW(PD_RETURNDEFAULT with a printer)");
    }
    no_dialog(b"comdlg32.dll!PrintDlgW(dialog)")
}

/// Whether winspool names a default printer.
fn has_default_printer() -> bool {
    let name: [u16; 13] = wide(b"winspool.drv\0");
    // SAFETY: a NUL-terminated name.
    let module = unsafe { LoadLibraryW(name.as_ptr()) };
    if module == 0 {
        return false;
    }
    // SAFETY: a NUL-terminated name.
    let f = unsafe { GetProcAddress(module, c"GetDefaultPrinterW".as_ptr().cast()) };
    if f == 0 {
        return false;
    }
    // SAFETY: GetDefaultPrinterW's documented signature.
    let get: extern "system" fn(*mut u16, *mut u32) -> i32 = unsafe { core::mem::transmute(f) };
    let mut buf = [0u16; 260];
    let mut n = buf.len() as u32;
    get(buf.as_mut_ptr(), &mut n) != 0
}

fn wide<const N: usize>(s: &[u8; N]) -> [u16; N] {
    let mut w = [0u16; N];
    for (d, &b) in w.iter_mut().zip(s) {
        *d = b.into();
    }
    w
}

// ---- FindTextW and ReplaceTextW ----

const FINDREPLACEW_SIZE: u32 = 80;
const FR_ENABLEHOOK: u32 = 0x100;
const FR_ENABLETEMPLATE: u32 = 0x200;
const FR_ENABLETEMPLATEHANDLE: u32 = 0x2000;

/// The checks FindTextW and ReplaceTextW make before their dialog, in
/// Windows' order (Wine's finddlg.c test records them): NULL, the size,
/// the owner, the search buffer, the replacement buffer (ReplaceText),
/// hook and template.
fn find_replace(fr: *const u8, replace: bool, what: &[u8]) -> usize {
    if fr.is_null() {
        return set_error(CDERR_INITIALIZATION) as usize;
    }
    // SAFETY: the caller's FINDREPLACEW, read field by field after its
    // size is checked (offsets from the x64 header).
    unsafe {
        let u32_at = |o: usize| (fr.add(o) as *const u32).read_unaligned();
        let ptr_at = |o: usize| (fr.add(o) as *const usize).read_unaligned();
        let u16_at = |o: usize| (fr.add(o) as *const u16).read_unaligned();
        if u32_at(0) != FINDREPLACEW_SIZE {
            return set_error(CDERR_STRUCTSIZE) as usize;
        }
        if ptr_at(8) == 0 {
            return set_error(CDERR_DIALOGFAILURE) as usize;
        }
        if u16_at(48) == 0 || ptr_at(32) == 0 {
            return set_error(FRERR_BUFFERLENGTHZERO) as usize;
        }
        if replace && ptr_at(40) == 0 {
            return set_error(FRERR_BUFFERLENGTHZERO) as usize;
        }
        let bits = [FR_ENABLEHOOK, FR_ENABLETEMPLATE, FR_ENABLETEMPLATEHANDLE];
        if let Some(e) =
            template_error(u32_at(24), bits, ptr_at(64), ptr_at(16), ptr_at(72) as *const u16)
        {
            return set_error(e) as usize;
        }
    }
    no_dialog(what) as usize
}

/// FindTextW: the modeless Find dialog (its window), or NULL with
/// CommDlgExtendedError's reason.
#[unsafe(no_mangle)]
pub extern "system" fn FindTextW(fr: *const u8) -> usize {
    find_replace(fr, false, b"comdlg32.dll!FindTextW(dialog)")
}

/// ReplaceTextW: the modeless Replace dialog (its window), or NULL with
/// CommDlgExtendedError's reason.
#[unsafe(no_mangle)]
pub extern "system" fn ReplaceTextW(fr: *const u8) -> usize {
    find_replace(fr, true, b"comdlg32.dll!ReplaceTextW(dialog)")
}

// ---- ChooseColorW ----

const CHOOSECOLORW_SIZE: u32 = 72;
const CC_ENABLEHOOK: u32 = 0x10;
const CC_ENABLETEMPLATE: u32 = 0x20;
const CC_ENABLETEMPLATEHANDLE: u32 = 0x40;

/// ChooseColorW: NULL is CDERR_INITIALIZATION, a wrong size
/// CDERR_STRUCTSIZE; the hook and template checks; then the dialog.
#[unsafe(no_mangle)]
pub extern "system" fn ChooseColorW(cc: *mut u8) -> i32 {
    if cc.is_null() {
        return set_error(CDERR_INITIALIZATION);
    }
    // SAFETY: the caller's CHOOSECOLORW, read after its size is checked
    // (offsets from the x64 header).
    unsafe {
        let u32_at = |o: usize| (cc.add(o) as *const u32).read_unaligned();
        let ptr_at = |o: usize| (cc.add(o) as *const usize).read_unaligned();
        if u32_at(0) != CHOOSECOLORW_SIZE {
            return set_error(CDERR_STRUCTSIZE);
        }
        let bits = [CC_ENABLEHOOK, CC_ENABLETEMPLATE, CC_ENABLETEMPLATEHANDLE];
        if let Some(e) =
            template_error(u32_at(40), bits, ptr_at(56), ptr_at(16), ptr_at(64) as *const u16)
        {
            return set_error(e);
        }
    }
    no_dialog(b"comdlg32.dll!ChooseColorW(dialog)")
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
