//! The OLE clipboard: OleSetClipboard, OleGetClipboard, OleFlushClipboard
//! and OleIsCurrentClipboard. Word's WWLIB reaches the last of them
//! through a delay load, which is why all four have to exist as
//! functions at all: a delay load sees a `.gstub` export as the honest
//! absence it is and raises 0xc06d007f, where a static import would only
//! have been refused by GetProcAddress.
//!
//! The init gate, as measured on Windows 11 24H2 and asserted by
//! user/ole32_test: only OleSetClipboard reads OleInitialize's own
//! per-thread count (lib.rs) and is refused with CO_E_NOTINITIALIZED
//! until it stands. The other three never look at it, and
//! OleIsCurrentClipboard answers S_FALSE in every state, a NULL argument
//! included -- which is the answer Word's delay load is after.
//!
//! The clipboard underneath is gala's Win32 one, the window station's
//! (ARCHITECTURE "The clipboard"), reached through ntdll's services
//! rather than user32: ole32 does not import user32, because programs
//! without windows use it too. OleSetClipboard(NULL), the documented way
//! to clear the clipboard, clears that one.
//!
//! What is not here yet: a data object has nowhere to go. Publishing its
//! formats means offering them without rendering them, and gala keeps a
//! delayed-rendering entry but never sends WM_RENDERFORMAT (ROADMAP LD
//! step 3), so a non-NULL object is CLIPBRD_E_CANT_SET and ledgered. By
//! the same absence OleGetClipboard has no wrapper data object over the
//! Win32 formats to hand back (E_FAIL, ledgered), and OleFlushClipboard
//! has nothing to render, which makes its S_OK the whole truth while
//! nothing can be set.

use core::ffi::c_void;

use super::{
    CO_E_NOTINITIALIZED, E_POINTER, HResult, S_FALSE, S_OK, missing, put, reached, thread_state,
};

const E_FAIL: HResult = 0x8000_4005u32 as i32;
const CLIPBRD_E_CANT_OPEN: HResult = 0x8004_01d0u32 as i32;
const CLIPBRD_E_CANT_EMPTY: HResult = 0x8004_01d1u32 as i32;
const CLIPBRD_E_CANT_SET: HResult = 0x8004_01d2u32 as i32;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaClipboardOpen(hwnd: u64) -> u32;
    fn GalaClipboardClose(locale: u32) -> u32;
    fn GalaClipboardEmpty() -> u32;
}

/// Is an OleInitialize standing on this thread? The gate reads that
/// count alone and never the COM count: an apartment without
/// OleInitialize does not open it, and tearing the apartment down under
/// a standing OleInitialize does not close it.
fn ole_initialized() -> bool {
    thread_state().is_some_and(|s| s.ole_refs > 0)
}

/// The Win32 clipboard emptied, which is what OleSetClipboard(NULL)
/// leaves behind. No window owns it after: ole32 has none to pass.
fn clear() -> HResult {
    // SAFETY: plain calls.
    let opened = unsafe { GalaClipboardOpen(0) };
    if opened != 0 {
        reached(b"ole32.dll!OleSetClipboard(the clipboard would not open)", opened as u64);
        return CLIPBRD_E_CANT_OPEN;
    }
    // SAFETY: plain calls, with the clipboard open.
    let (emptied, _) = unsafe { (GalaClipboardEmpty(), GalaClipboardClose(0)) };
    if emptied != 0 {
        reached(b"ole32.dll!OleSetClipboard(the clipboard would not empty)", emptied as u64);
        return CLIPBRD_E_CANT_EMPTY;
    }
    reached(b"ole32.dll!OleSetClipboard(cleared)", 0);
    S_OK
}

/// OleSetClipboard(pDataObj): the data object the clipboard offers from
/// now on, or NULL to clear the clipboard. Refused with
/// CO_E_NOTINITIALIZED without an OleInitialize on this thread, before
/// the argument is looked at.
#[unsafe(no_mangle)]
pub extern "system" fn OleSetClipboard(data: *mut c_void) -> HResult {
    if !ole_initialized() {
        reached(b"ole32.dll!OleSetClipboard(no OleInitialize on this thread)", 0);
        return CO_E_NOTINITIALIZED;
    }
    if !data.is_null() {
        missing(b"ole32.dll!OleSetClipboard(a data object)", 0);
        return CLIPBRD_E_CANT_SET;
    }
    clear()
}

/// OleGetClipboard(ppDataObj): a data object for what is on the
/// clipboard, Win32 formats wrapped included. gala has no such wrapper
/// yet, so there is nothing to hand back; the gate is not consulted
/// (measured).
#[unsafe(no_mangle)]
pub extern "system" fn OleGetClipboard(out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    missing(b"ole32.dll!OleGetClipboard(no data object over the clipboard)", 0);
    E_FAIL
}

/// OleFlushClipboard: what the clipboard offers rendered now, so it
/// outlives the program that set it. Nothing can put an OLE object on
/// gala's clipboard yet, so there is never anything to render, and the
/// Win32 formats that are there were rendered when they were set. The
/// gate is not consulted (measured).
#[unsafe(no_mangle)]
pub extern "system" fn OleFlushClipboard() -> HResult {
    reached(b"ole32.dll!OleFlushClipboard", 0);
    S_OK
}

/// OleIsCurrentClipboard(pDataObj): S_OK while that object is the one
/// OleSetClipboard put on the clipboard, S_FALSE otherwise -- which is
/// every state here, a NULL argument included, until a data object can
/// be set at all. The gate is not consulted (measured).
#[unsafe(no_mangle)]
pub extern "system" fn OleIsCurrentClipboard(_data: *mut c_void) -> HResult {
    reached(b"ole32.dll!OleIsCurrentClipboard", 0);
    S_FALSE
}
