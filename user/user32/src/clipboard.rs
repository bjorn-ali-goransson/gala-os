//! The clipboard: the window station's, kept in the kernel
//! (`nt::clipboard`, gala's GalaClipboard* services; ARCHITECTURE "The
//! clipboard"), so every process of the station shares it.
//!
//! - SetClipboardData copies an HGLOBAL's bytes (GlobalSize of them) into
//!   the kernel; the handle is the system's from then on, as Windows
//!   documents: user32 keeps it as this process's copy of the format and
//!   frees it once the clipboard no longer holds that data. GDI and
//!   private formats (CF_BITMAP, CF_PALETTE, CF_ENHMETAFILE, the CF_DSP*
//!   handles, CF_PRIVATEFIRST..LAST, CF_GDIOBJFIRST..LAST) pass the handle
//!   value, good in the setting process only (another process gets NULL,
//!   ledgered). A NULL handle is delayed rendering, kept but never
//!   rendered (WM_RENDERFORMAT is not sent: ledgered).
//! - GetClipboardData makes a process-local HGLOBAL from the kernel's
//!   bytes, valid until the clipboard's data changes (freed by user32 at
//!   its next clipboard call after that); the synthesized text formats
//!   ("Synthesized Clipboard Formats") are converted here from the format
//!   the kernel names as their source, through the ANSI and OEM code pages
//!   (the system's: gala's CF_LOCALE is the US layout's language, whose
//!   code pages they are).
//! - The rest (the owner, the open window, the sequence number, the
//!   formats, the listeners) is asked of the kernel each time.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::window::{Hwnd, set_last_error, unsupported};

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaClipboardOpen(hwnd: u64) -> u32;
    fn GalaClipboardClose(locale: u32) -> u32;
    fn GalaClipboardEmpty() -> u32;
    fn GalaClipboardSet(
        format: u32,
        kind: u32,
        data: *const u8,
        length: u32,
        serial: *mut u32,
    ) -> u32;
    fn GalaClipboardGet(format: u32, buffer: *mut u8, length: u32, info: *mut u8) -> u32;
    fn GalaClipboardInfo(record: *mut u8) -> u32;
    fn GalaClipboardFormats(formats: *mut u32, max: u32, count: *mut u32) -> u32;
    fn GalaClipboardListener(hwnd: u64, add: u32) -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> usize;
    fn GlobalFree(mem: usize) -> usize;
    fn GlobalLock(mem: usize) -> *mut u8;
    fn GlobalUnlock(mem: usize) -> i32;
    fn GlobalSize(mem: usize) -> usize;
    fn MultiByteToWideChar(
        code_page: u32,
        flags: u32,
        src: *const u8,
        src_len: i32,
        dst: *mut u16,
        dst_len: i32,
    ) -> i32;
    fn WideCharToMultiByte(
        code_page: u32,
        flags: u32,
        src: *const u16,
        src_len: i32,
        dst: *mut u8,
        dst_len: i32,
        default: *const u8,
        used_default: *mut i32,
    ) -> i32;
}

const CF_BITMAP: u32 = 2;
const CF_METAFILEPICT: u32 = 3;
const CF_OEMTEXT: u32 = 7;
const CF_PALETTE: u32 = 9;
const CF_UNICODETEXT: u32 = 13;
const CF_ENHMETAFILE: u32 = 14;
const CF_DSPBITMAP: u32 = 0x82;
const CF_DSPENHMETAFILE: u32 = 0x8e;

/// The kernel's kinds (nt::clipboard::KIND_*).
const KIND_MEMORY: u32 = 0;
const KIND_HANDLE: u32 = 1;
const KIND_DELAYED: u32 = 2;

const STATUS_ACCESS_DENIED: u32 = 0xc000_0022;
const STATUS_BUFFER_TOO_SMALL: u32 = 0xc000_0023;
const STATUS_INVALID_LOCK_SEQUENCE: u32 = 0xc000_001e;
const STATUS_QUOTA_EXCEEDED: u32 = 0xc000_0044;
const STATUS_INVALID_HANDLE: u32 = 0xc000_0008;
const STATUS_NOT_FOUND: u32 = 0xc000_0225;

const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_WINDOW_HANDLE: u32 = 1400;
const ERROR_CLIPBOARD_NOT_OPEN: u32 = 1418;
const ERROR_NOT_ENOUGH_QUOTA: u32 = 1816;
const ERROR_NOT_FOUND: u32 = 1168;

const GMEM_MOVEABLE: u32 = 2;
const CP_ACP: u32 = 0;
const CP_OEMCP: u32 = 1;

/// The most formats user32 asks the kernel for (nt::clipboard::MAX_FORMATS).
const MAX_FORMATS: usize = 256;

/// Whether `format` is passed as a handle value rather than memory.
fn is_handle_format(format: u32) -> bool {
    matches!(format, CF_BITMAP | CF_PALETTE | CF_ENHMETAFILE | CF_DSPBITMAP | CF_DSPENHMETAFILE)
        || (0x200..=0x3ff).contains(&format)
}

/// The kernel's clipboard state (GalaClipboardInfo).
#[derive(Default)]
struct Info {
    sequence: u32,
    open_by_caller: bool,
    open_hwnd: u64,
    owner_hwnd: u64,
    count: u32,
    emptied: u32,
}

fn info() -> Option<Info> {
    let mut r = [0u8; 32];
    // SAFETY: our 32-byte record.
    if unsafe { GalaClipboardInfo(r.as_mut_ptr()) } != 0 {
        return None;
    }
    let u32_at = |at: usize| u32::from_le_bytes([r[at], r[at + 1], r[at + 2], r[at + 3]]);
    let u64_at = |at: usize| u64::from(u32_at(at)) | u64::from(u32_at(at + 4)) << 32;
    Some(Info {
        sequence: u32_at(0),
        open_by_caller: u32_at(4) & 1 != 0,
        open_hwnd: u64_at(8),
        owner_hwnd: u64_at(16),
        count: u32_at(24),
        emptied: u32_at(28),
    })
}

/// The formats, in enumeration order, into `out`; how many there are.
fn formats(out: &mut [u32; MAX_FORMATS]) -> usize {
    let mut n = 0u32;
    // SAFETY: our array of MAX_FORMATS.
    if unsafe { GalaClipboardFormats(out.as_mut_ptr(), MAX_FORMATS as u32, &mut n) } != 0 {
        return 0;
    }
    (n as usize).min(MAX_FORMATS)
}

// ---- This process's handles for the clipboard's data ----
//
// (format, serial, handle): the HGLOBAL this process set for a format, or
// made for it from the kernel's data with that serial. Freed once the
// clipboard no longer holds that data (an EmptyClipboard since, or the
// format set again).

const CACHED: usize = 32;

struct Cache {
    lock: AtomicBool,
    entries: core::cell::UnsafeCell<[(u32, u32, usize); CACHED]>,
}

// SAFETY: `entries` is only used under `lock`.
unsafe impl Sync for Cache {}

static CACHE: Cache = Cache {
    lock: AtomicBool::new(false),
    entries: core::cell::UnsafeCell::new([(0, 0, 0); CACHED]),
};

fn with_cache<R>(f: impl FnOnce(&mut [(u32, u32, usize); CACHED]) -> R) -> R {
    while CACHE
        .lock
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *CACHE.entries.get() });
    CACHE.lock.store(false, Ordering::Release);
    r
}

/// Frees the handles whose data an EmptyClipboard (sequence number
/// `emptied`) took away.
fn prune(emptied: u32) {
    let mut free = [0usize; CACHED];
    with_cache(|c| {
        for (e, f) in c.iter_mut().zip(free.iter_mut()) {
            if e.2 != 0 && e.1 <= emptied {
                *f = e.2;
                *e = (0, 0, 0);
            }
        }
    });
    for h in free.into_iter().filter(|&h| h != 0) {
        // SAFETY: a handle this process made or was given for the clipboard.
        unsafe { GlobalFree(h) };
    }
}

/// The handle kept for `format`'s data with `serial`, if any.
fn cached(format: u32, serial: u32) -> Option<usize> {
    with_cache(|c| c.iter().find(|e| e.0 == format && e.1 == serial && e.2 != 0).map(|e| e.2))
}

/// Keeps `handle` for `format`'s data with `serial`, freeing the one kept
/// for older data of the format; false if the table is full (the caller
/// keeps the handle: it lives on unfreed).
fn keep(format: u32, serial: u32, handle: usize) -> bool {
    let (kept, old) = with_cache(|c| {
        let old = match c.iter_mut().find(|e| e.0 == format && e.2 != 0) {
            Some(e) => {
                let old = e.2;
                *e = (format, serial, handle);
                return (true, old);
            }
            None => 0,
        };
        match c.iter_mut().find(|e| e.2 == 0) {
            Some(e) => {
                *e = (format, serial, handle);
                (true, old)
            }
            None => (false, old),
        }
    });
    if old != 0 && old != handle {
        // SAFETY: a handle this process made or was given for the clipboard.
        unsafe { GlobalFree(old) };
    }
    kept
}

/// Maps a clipboard service's failure to the last error.
fn fail(status: u32) {
    set_last_error(match status {
        STATUS_ACCESS_DENIED => ERROR_ACCESS_DENIED,
        STATUS_INVALID_LOCK_SEQUENCE => ERROR_CLIPBOARD_NOT_OPEN,
        STATUS_QUOTA_EXCEEDED => ERROR_NOT_ENOUGH_QUOTA,
        STATUS_INVALID_HANDLE => ERROR_INVALID_WINDOW_HANDLE,
        STATUS_NOT_FOUND => ERROR_NOT_FOUND,
        _ => ERROR_NOT_ENOUGH_MEMORY,
    });
}

/// OpenClipboard(hWndNewOwner): FALSE with ERROR_ACCESS_DENIED while
/// another window or thread has the clipboard open.
#[unsafe(no_mangle)]
pub extern "system" fn OpenClipboard(owner: Hwnd) -> i32 {
    if owner != 0 && crate::window::IsWindow(owner) == 0 {
        set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return 0;
    }
    // SAFETY: a plain call.
    match unsafe { GalaClipboardOpen(owner as u64) } {
        0 => 1,
        s => {
            fail(s);
            0
        }
    }
}

/// CloseClipboard: the input language goes with it (CF_LOCALE, if text
/// was set without one).
#[unsafe(no_mangle)]
pub extern "system" fn CloseClipboard() -> i32 {
    let locale = (crate::keyboard::GetKeyboardLayout(0) & 0xffff) as u32;
    // SAFETY: a plain call.
    match unsafe { GalaClipboardClose(locale) } {
        0 => 1,
        s => {
            fail(s);
            0
        }
    }
}

/// EmptyClipboard: every format goes and the open window becomes the
/// owner.
#[unsafe(no_mangle)]
pub extern "system" fn EmptyClipboard() -> i32 {
    // SAFETY: a plain call.
    match unsafe { GalaClipboardEmpty() } {
        0 => {
            if let Some(i) = info() {
                prune(i.emptied);
            }
            1
        }
        s => {
            fail(s);
            0
        }
    }
}

/// SetClipboardData(uFormat, hMem): `hMem` on success (the system's from
/// now on), NULL on failure.
#[unsafe(no_mangle)]
pub extern "system" fn SetClipboardData(format: u32, memory: usize) -> usize {
    if format == 0 {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    let mut serial = 0u32;
    let status = if memory == 0 {
        // SAFETY: no data.
        unsafe { GalaClipboardSet(format, KIND_DELAYED, core::ptr::null(), 0, &mut serial) }
    } else if is_handle_format(format) {
        let value = (memory as u64).to_le_bytes();
        // SAFETY: our 8 bytes.
        unsafe { GalaClipboardSet(format, KIND_HANDLE, value.as_ptr(), 8, &mut serial) }
    } else {
        if format == CF_METAFILEPICT {
            // Its METAFILEPICT holds an HMETAFILE, a handle of this process.
            unsupported(b"user32.dll!SetClipboardData(CF_METAFILEPICT across processes)", 0);
        }
        // SAFETY: the caller's HGLOBAL, read within its size while locked.
        unsafe {
            let size = GlobalSize(memory);
            let p = GlobalLock(memory);
            if p.is_null() {
                set_last_error(ERROR_INVALID_HANDLE);
                return 0;
            }
            let s = GalaClipboardSet(format, KIND_MEMORY, p, size as u32, &mut serial);
            GlobalUnlock(memory);
            s
        }
    };
    if status != 0 {
        fail(status);
        return 0;
    }
    if memory != 0 && !is_handle_format(format) {
        keep(format, serial, memory);
    }
    memory
}

/// The kernel's record for `format` (GalaClipboardGet's): (size, serial,
/// source, kind), or the failure's status.
fn data_info(format: u32) -> Result<(u32, u32, u32, u32), u32> {
    let mut r = [0u8; 16];
    // SAFETY: no buffer; our record.
    let s = unsafe { GalaClipboardGet(format, core::ptr::null_mut(), 0, r.as_mut_ptr()) };
    if s != 0 && s != STATUS_BUFFER_TOO_SMALL {
        return Err(s);
    }
    let at = |i: usize| u32::from_le_bytes([r[i], r[i + 1], r[i + 2], r[i + 3]]);
    Ok((at(0), at(4), at(8), at(12)))
}

/// `format`'s bytes (`size` of them) into `buffer`.
fn fetch(format: u32, buffer: *mut u8, size: u32) -> bool {
    let mut r = [0u8; 16];
    // SAFETY: the caller's buffer of `size` bytes; our record.
    unsafe { GalaClipboardGet(format, buffer, size, r.as_mut_ptr()) == 0 }
}

/// A new moveable HGLOBAL holding `bytes`.
fn global(bytes: &[u8]) -> usize {
    // SAFETY: documented calls; the block holds `bytes.len()` bytes.
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
        if h == 0 {
            return 0;
        }
        let p = GlobalLock(h);
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        GlobalUnlock(h);
        h
    }
}

/// Text of `format` (CF_TEXT, CF_OEMTEXT or CF_UNICODETEXT) in `bytes`,
/// up to its NUL, as UTF-16.
fn to_wide(format: u32, bytes: &[u8]) -> Option<Vec<u16>> {
    let mut wide = Vec::new();
    if format == CF_UNICODETEXT {
        for c in bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])) {
            if c == 0 {
                break;
            }
            wide.try_reserve(1).ok()?;
            wide.push(c);
        }
        return Some(wide);
    }
    let len = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    if len == 0 {
        return Some(wide);
    }
    let cp = if format == CF_OEMTEXT { CP_OEMCP } else { CP_ACP };
    // SAFETY: our buffers and their lengths.
    unsafe {
        let n = MultiByteToWideChar(cp, 0, bytes.as_ptr(), len as i32, core::ptr::null_mut(), 0);
        wide.try_reserve(n.max(0) as usize).ok()?;
        wide.resize(n.max(0) as usize, 0);
        MultiByteToWideChar(cp, 0, bytes.as_ptr(), len as i32, wide.as_mut_ptr(), n);
    }
    Some(wide)
}

/// UTF-16 `wide` as `format`'s bytes, with its NUL.
fn from_wide(format: u32, wide: &[u16]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    if format == CF_UNICODETEXT {
        out.try_reserve(2 * wide.len() + 2).ok()?;
        for c in wide.iter().chain(&[0]) {
            out.extend_from_slice(&c.to_le_bytes());
        }
        return Some(out);
    }
    let cp = if format == CF_OEMTEXT { CP_OEMCP } else { CP_ACP };
    // SAFETY: our buffers and their lengths.
    unsafe {
        let n = if wide.is_empty() {
            0
        } else {
            WideCharToMultiByte(
                cp,
                0,
                wide.as_ptr(),
                wide.len() as i32,
                core::ptr::null_mut(),
                0,
                core::ptr::null(),
                core::ptr::null_mut(),
            )
            .max(0)
        };
        out.try_reserve(n as usize + 1).ok()?;
        out.resize(n as usize + 1, 0);
        if n > 0 {
            WideCharToMultiByte(
                cp,
                0,
                wide.as_ptr(),
                wide.len() as i32,
                out.as_mut_ptr(),
                n,
                core::ptr::null(),
                core::ptr::null_mut(),
            );
        }
    }
    Some(out)
}

/// GetClipboardData(uFormat): a handle to the format's data (the
/// clipboard open), NULL if it is not there.
#[unsafe(no_mangle)]
pub extern "system" fn GetClipboardData(format: u32) -> usize {
    if let Some(i) = info() {
        prune(i.emptied);
    }
    let (size, serial, source, kind) = match data_info(format) {
        Ok(r) => r,
        Err(STATUS_INVALID_LOCK_SEQUENCE) => {
            set_last_error(ERROR_CLIPBOARD_NOT_OPEN);
            return 0;
        }
        Err(_) => return 0,
    };
    if kind == KIND_HANDLE {
        let mut value = [0u8; 8];
        if size != 8 || !fetch(format, value.as_mut_ptr(), 8) {
            return 0;
        }
        return u64::from_le_bytes(value) as usize;
    }
    if let Some(h) = cached(format, serial) {
        return h;
    }
    let mut bytes = Vec::new();
    if bytes.try_reserve(size as usize).is_err() {
        set_last_error(ERROR_NOT_ENOUGH_MEMORY);
        return 0;
    }
    bytes.resize(size as usize, 0);
    // Asked as `format`: a synthesized format gets its source's bytes.
    if !fetch(format, bytes.as_mut_ptr(), size) {
        return 0;
    }
    let h = if source == format {
        global(&bytes)
    } else {
        match to_wide(source, &bytes).and_then(|w| from_wide(format, &w)) {
            Some(converted) => global(&converted),
            None => 0,
        }
    };
    if h == 0 {
        set_last_error(ERROR_NOT_ENOUGH_MEMORY);
        return 0;
    }
    keep(format, serial, h);
    h
}

/// IsClipboardFormatAvailable(format): whether the clipboard has it (or
/// can synthesize it); the clipboard need not be open.
#[unsafe(no_mangle)]
pub extern "system" fn IsClipboardFormatAvailable(format: u32) -> i32 {
    let mut list = [0u32; MAX_FORMATS];
    let n = formats(&mut list);
    i32::from(list[..n].contains(&format))
}

/// EnumClipboardFormats(format): the format after `format` (0: the
/// first); 0 at the end (last error 0) or when the clipboard is not open
/// by this thread (ERROR_CLIPBOARD_NOT_OPEN).
#[unsafe(no_mangle)]
pub extern "system" fn EnumClipboardFormats(format: u32) -> u32 {
    if !info().is_some_and(|i| i.open_by_caller) {
        set_last_error(ERROR_CLIPBOARD_NOT_OPEN);
        return 0;
    }
    let mut list = [0u32; MAX_FORMATS];
    let n = formats(&mut list);
    let list = &list[..n];
    let next = if format == 0 {
        list.first()
    } else {
        list.iter().position(|&f| f == format).and_then(|i| list.get(i + 1))
    };
    match next {
        Some(&f) => f,
        None => {
            set_last_error(0);
            0
        }
    }
}

/// CountClipboardFormats: how many formats the clipboard holds (the
/// synthesized ones included).
#[unsafe(no_mangle)]
pub extern "system" fn CountClipboardFormats() -> i32 {
    info().map_or(0, |i| i.count as i32)
}

/// GetClipboardSequenceNumber: the window station's clipboard serial
/// number (grows with every change).
#[unsafe(no_mangle)]
pub extern "system" fn GetClipboardSequenceNumber() -> u32 {
    info().map_or(0, |i| i.sequence)
}

/// GetOpenClipboardWindow: the window the clipboard is open with (NULL:
/// closed, or open with none).
#[unsafe(no_mangle)]
pub extern "system" fn GetOpenClipboardWindow() -> Hwnd {
    info().map_or(0, |i| i.open_hwnd as Hwnd)
}

/// GetClipboardOwner: the window that last emptied the clipboard (NULL:
/// none, or its process ended).
#[unsafe(no_mangle)]
pub extern "system" fn GetClipboardOwner() -> Hwnd {
    info().map_or(0, |i| i.owner_hwnd as Hwnd)
}

/// GetPriorityClipboardFormat(paFormatPriorityList, cFormats): the first
/// listed format the clipboard has; NULL if it is empty, -1 if it has
/// none of them.
#[unsafe(no_mangle)]
pub extern "system" fn GetPriorityClipboardFormat(priority: *const u32, count: i32) -> i32 {
    let mut list = [0u32; MAX_FORMATS];
    let n = formats(&mut list);
    if n == 0 {
        return 0;
    }
    for i in 0..count.max(0) as usize {
        // SAFETY: the caller's array of `count` formats.
        let f = unsafe { *priority.add(i) };
        if list[..n].contains(&f) {
            return f as i32;
        }
    }
    -1
}

/// GetUpdatedClipboardFormats(lpuiFormats, cFormats, pcFormatsOut): the
/// formats; FALSE with ERROR_INSUFFICIENT_BUFFER when they do not fit (the
/// count written).
#[unsafe(no_mangle)]
pub extern "system" fn GetUpdatedClipboardFormats(out: *mut u32, max: u32, count: *mut u32) -> i32 {
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    let mut list = [0u32; MAX_FORMATS];
    let n = formats(&mut list);
    if count.is_null() {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    // SAFETY: the caller's count.
    unsafe { *count = n as u32 };
    if n > max as usize || (n > 0 && out.is_null()) {
        set_last_error(ERROR_INSUFFICIENT_BUFFER);
        return 0;
    }
    // SAFETY: the caller's array of `max` formats.
    unsafe { core::ptr::copy_nonoverlapping(list.as_ptr(), out, n) };
    1
}

/// AddClipboardFormatListener(hwnd): `hwnd` gets WM_CLIPBOARDUPDATE when
/// the clipboard's contents change.
#[unsafe(no_mangle)]
pub extern "system" fn AddClipboardFormatListener(hwnd: Hwnd) -> i32 {
    listener(hwnd, true)
}

#[unsafe(no_mangle)]
pub extern "system" fn RemoveClipboardFormatListener(hwnd: Hwnd) -> i32 {
    listener(hwnd, false)
}

fn listener(hwnd: Hwnd, add: bool) -> i32 {
    if hwnd == 0 || crate::window::IsWindow(hwnd) == 0 {
        set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return 0;
    }
    // SAFETY: a plain call.
    match unsafe { GalaClipboardListener(hwnd as u64, u32::from(add)) } {
        0 => 1,
        STATUS_ACCESS_DENIED => {
            set_last_error(ERROR_ACCESS_DENIED);
            0
        }
        s => {
            fail(s);
            0
        }
    }
}
