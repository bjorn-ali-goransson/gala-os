//! gala's UIAutomationCore.dll: the client side of UI Automation
//! (IUIAutomation, UIAutomationClient.h) over user32's window tree, as
//! Microsoft documents it, so tests can find controls by AutomationId,
//! Name and ControlType and drive them through control patterns.
//!
//! - `CUIAutomation` (CoCreateInstance through ole32 and the HKCR\CLSID
//!   entry in the registry seed): the IUIAutomation object
//!   (automation.rs).
//! - Elements (element.rs) wrap a node: an HWND, or a list control's
//!   item (items.rs: list boxes' strings and list views' items, read
//!   with the controls' messages, through memory allocated in the list's
//!   process when it is another's); the root is the desktop
//!   (GetDesktopWindow), whose children are the visible top-level
//!   windows, and a window's children are its visible child windows in z
//!   order. Properties come from the window: AutomationId the control id,
//!   Name the text (an edit's: the static label before it), ControlType
//!   from the class and style, BoundingRectangle the window rectangle.
//! - Conditions (condition.rs): true, false, property (VARIANT compare,
//!   IgnoreCase and MatchSubstring), and, or, not.
//! - Patterns (pattern.rs): Invoke (push buttons: BM_CLICK posted, as
//!   Invoke may return before the click is handled; list-view items: a
//!   double click posted; title-bar buttons: their WM_SYSCOMMAND),
//!   Value (edits: WM_SETTEXT, the window text), Toggle (check boxes:
//!   BM_GETCHECK, BM_CLICK), Selection (lists), SelectionItem and
//!   ScrollItem (list items), Window and Transform (top-level windows
//!   with a caption, frame.rs, which also makes their TitleBar
//!   elements).
//! - Tree walkers (walker.rs): the raw, control and content views and
//!   walkers with any condition.
//! - Grids (grid.rs): report-view list views with the Grid and Table
//!   patterns, their items' cells (GridItem, TableItem) and the header's
//!   items.
//! - Events (events.rs): IUIAutomation's Add/Remove*EventHandler, raised
//!   from WinEvents (SetWinEventHook) on a thread of ours.
//! - Messages to windows go with SendMessageTimeout (SMTO_ABORTIFHUNG):
//!   `SendMessageW` below.
//!
//! - CUIAutomation8: the same object with IUIAutomation2..6 (options kept,
//!   IUIAutomation3..6's events E_NOTIMPL); cache requests (cache.rs)
//!   keep what they are told, and the BuildCache forms return elements whose Cached getters
//!   read current values (nothing is cached). The not-supported value is
//!   one static IUnknown, returned for properties an element lacks when
//!   the client asks GetCurrentPropertyValueEx to ignore defaults.
//!
//! Not there yet (recorded in the ledger, E_NOTIMPL): providers inside
//! programs (WM_GETOBJECT, IRawElementProviderSimple).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

/// A method not implemented yet: records `what` and returns E_NOTIMPL
/// (it ignores its arguments, so it fits any vtable slot that returns an
/// HRESULT).
macro_rules! not_impl {
    ($name:ident, $what:literal) => {
        extern "system" fn $name(_this: usize) -> crate::HResult {
            crate::missing($what, 0);
            crate::E_NOTIMPL
        }
    };
}

mod automation;
mod cache;
mod condition;
mod element;
mod events;
mod frame;
mod grid;
mod items;
mod menus;
mod pattern;
mod walker;

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

/// MSVC-target codegen references this marker symbol directly.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapReAlloc(heap: usize, flags: u32, p: *mut u8, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
}

#[link(name = "oleaut32", kind = "raw-dylib")]
unsafe extern "system" {
    pub fn SysAllocStringLen(s: *const u16, len: u32) -> *mut u16;
    pub fn SysFreeString(s: *mut u16);
    pub fn SysStringLen(s: *const u16) -> u32;
    pub fn VariantClear(v: *mut Variant) -> HResult;
    pub fn VariantCopy(dst: *mut Variant, src: *const Variant) -> HResult;
    pub fn SafeArrayCreateVector(vt: u16, lower: i32, count: u32) -> *mut c_void;
    pub fn SafeArrayPutElement(sa: *mut c_void, index: *const i32, value: *const c_void)
    -> HResult;
    pub fn SafeArrayGetDim(sa: *mut c_void) -> u32;
    pub fn SafeArrayGetElemsize(sa: *mut c_void) -> u32;
    pub fn SafeArrayGetLBound(sa: *mut c_void, dim: u32, out: *mut i32) -> HResult;
    pub fn SafeArrayGetUBound(sa: *mut c_void, dim: u32, out: *mut i32) -> HResult;
    pub fn SafeArrayAccessData(sa: *mut c_void, out: *mut *mut c_void) -> HResult;
    pub fn SafeArrayUnaccessData(sa: *mut c_void) -> HResult;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    pub fn GetCurrentProcessId() -> u32;
    pub fn GetLastError() -> u32;
    pub fn OpenProcess(access: u32, inherit: i32, pid: u32) -> usize;
    pub fn CloseHandle(h: usize) -> i32;
    pub fn VirtualAllocEx(
        process: usize,
        address: *mut c_void,
        size: usize,
        kind: u32,
        protect: u32,
    ) -> *mut c_void;
    pub fn VirtualFreeEx(process: usize, address: *mut c_void, size: usize, kind: u32) -> i32;
    pub fn ReadProcessMemory(
        process: usize,
        address: *const c_void,
        buf: *mut c_void,
        size: usize,
        done: *mut usize,
    ) -> i32;
    pub fn WriteProcessMemory(
        process: usize,
        address: *mut c_void,
        buf: *const c_void,
        size: usize,
        done: *mut usize,
    ) -> i32;
}

#[link(name = "ole32", kind = "raw-dylib")]
unsafe extern "system" {
    pub fn CoTaskMemAlloc(size: usize) -> *mut u8;
}

pub type Hwnd = usize;

#[repr(C)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

pub type EnumProc = extern "system" fn(Hwnd, isize) -> i32;

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    pub fn GetDesktopWindow() -> Hwnd;
    pub fn IsWindow(hwnd: Hwnd) -> i32;
    pub fn IsWindowVisible(hwnd: Hwnd) -> i32;
    pub fn IsWindowEnabled(hwnd: Hwnd) -> i32;
    pub fn GetWindow(hwnd: Hwnd, cmd: u32) -> Hwnd;
    pub fn GetAncestor(hwnd: Hwnd, flags: u32) -> Hwnd;
    pub fn EnumWindows(proc_: EnumProc, data: isize) -> i32;
    pub fn GetClassNameW(hwnd: Hwnd, buf: *mut u16, n: i32) -> i32;
    pub fn GetWindowTextW(hwnd: Hwnd, buf: *mut u16, n: i32) -> i32;
    pub fn GetWindowTextLengthW(hwnd: Hwnd) -> i32;
    pub fn GetDlgCtrlID(hwnd: Hwnd) -> i32;
    pub fn GetWindowLongW(hwnd: Hwnd, index: i32) -> i32;
    pub fn GetWindowRect(hwnd: Hwnd, rect: *mut [i32; 4]) -> i32;
    pub fn GetClientRect(hwnd: Hwnd, rect: *mut [i32; 4]) -> i32;
    pub fn ClientToScreen(hwnd: Hwnd, point: *mut [i32; 2]) -> i32;
    pub fn GetParent(hwnd: Hwnd) -> Hwnd;
    pub fn GetWindowThreadProcessId(hwnd: Hwnd, pid: *mut u32) -> u32;
    pub fn GetFocus() -> Hwnd;
    pub fn SetFocus(hwnd: Hwnd) -> Hwnd;
    pub fn SetForegroundWindow(hwnd: Hwnd) -> i32;
    pub fn WindowFromPoint(point: Point) -> Hwnd;
    pub fn PostMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn SendMessageTimeoutW(
        hwnd: Hwnd,
        msg: u32,
        wparam: usize,
        lparam: isize,
        flags: u32,
        ms: u32,
        result: *mut usize,
    ) -> isize;
    pub fn IsZoomed(hwnd: Hwnd) -> i32;
    pub fn IsIconic(hwnd: Hwnd) -> i32;
    pub fn GetSystemMetrics(index: i32) -> i32;
    pub fn SetWindowPos(
        hwnd: Hwnd,
        after: Hwnd,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
}

/// How long a message sent to a window waits for its answer (ms): the
/// automation object's TransactionTimeout (IUIAutomation2, 20 s by
/// default; the last one set by any client object of the process).
pub static TRANSACTION_MS: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(20_000);

/// A message to a window, as Windows' UIA proxies send them:
/// SendMessageTimeout with SMTO_ABORTIFHUNG, so a hung program (a thread
/// that has left a sent message waiting 5 s) or one that does not answer
/// within the transaction time-out does not hang the client; then 0
/// (recorded).
///
/// # Safety
/// Pointers in `lparam` must be valid for the message: in the window's
/// process (above WM_USER) or ours (messages user32 marshals).
#[allow(non_snake_case)]
pub unsafe fn SendMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    const SMTO_ABORTIFHUNG: u32 = 2;
    const ERROR_TIMEOUT: u32 = 1460;
    let mut result = 0usize;
    let ms = TRANSACTION_MS.load(core::sync::atomic::Ordering::Relaxed);
    // SAFETY: the caller's pointers; our result.
    let ok = unsafe {
        SendMessageTimeoutW(hwnd, msg, wparam, lparam, SMTO_ABORTIFHUNG, ms, &mut result)
    };
    if ok == 0 {
        // SAFETY: a plain call.
        if unsafe { GetLastError() } == ERROR_TIMEOUT {
            missing(b"UIAutomationCore.dll!SendMessageTimeout(timed out)", u64::from(msg));
        }
        return 0;
    }
    result as isize
}

pub type HResult = i32;
pub type Guid = [u8; 16];

pub const S_OK: HResult = 0;
pub const S_FALSE: HResult = 1;
pub const E_NOTIMPL: HResult = 0x8000_4001u32 as i32;
pub const E_NOINTERFACE: HResult = 0x8000_4002u32 as i32;
pub const E_POINTER: HResult = 0x8000_4003u32 as i32;
pub const E_OUTOFMEMORY: HResult = 0x8007_000eu32 as i32;
pub const E_INVALIDARG: HResult = 0x8007_0057u32 as i32;
const CLASS_E_NOAGGREGATION: HResult = 0x8004_0110u32 as i32;
const CLASS_E_CLASSNOTAVAILABLE: HResult = 0x8004_0111u32 as i32;
pub const UIA_E_ELEMENTNOTENABLED: HResult = 0x8004_0200u32 as i32;
pub const UIA_E_ELEMENTNOTAVAILABLE: HResult = 0x8004_0201u32 as i32;
pub const UIA_E_INVALIDOPERATION: HResult = 0x8013_1509u32 as i32;

/// A GUID from its textual parts.
pub const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

pub const IID_IUNKNOWN: Guid = guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_ICLASSFACTORY: Guid = guid(1, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
/// CLSID_CUIAutomation {ff48dba4-60ef-4201-aa87-54103eef594e}.
const CLSID_CUIAUTOMATION: Guid =
    guid(0xff48dba4, 0x60ef, 0x4201, [0xaa, 0x87, 0x54, 0x10, 0x3e, 0xef, 0x59, 0x4e]);
/// CLSID_CUIAutomation8 {e22ad333-b25f-460c-83d0-0581107395c9}: the same
/// object with IUIAutomation2..6 too (Windows 8 and later).
const CLSID_CUIAUTOMATION8: Guid =
    guid(0xe22ad333, 0xb25f, 0x460c, [0x83, 0xd0, 0x05, 0x81, 0x10, 0x73, 0x95, 0xc9]);

/// Reports an unsupported case to the missing-features ledger.
pub fn missing(function: &[u8], detail: u64) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(function.as_ptr(), function.len(), 2, detail) };
}

// ---- Memory ----

pub fn alloc(size: usize) -> *mut u8 {
    // SAFETY: plain call.
    unsafe { HeapAlloc(GetProcessHeap(), 0, size) }
}

pub fn realloc(p: *mut u8, size: usize) -> *mut u8 {
    if p.is_null() {
        return alloc(size);
    }
    // SAFETY: memory from `alloc`.
    unsafe { HeapReAlloc(GetProcessHeap(), 0, p, size) }
}

pub fn free(p: *mut u8) {
    if !p.is_null() {
        // SAFETY: memory from `alloc`.
        unsafe { HeapFree(GetProcessHeap(), 0, p) };
    }
}

/// Moves `value` to the heap; null without memory.
pub fn boxed<T>(value: T) -> *mut T {
    let p = alloc(size_of::<T>()).cast::<T>();
    if !p.is_null() {
        // SAFETY: fresh memory for one T (the heap aligns to 16).
        unsafe { p.write(value) };
    }
    p
}

/// Stores `value` at the caller's output pointer.
pub fn put<T>(out: *mut T, value: T) {
    // SAFETY: the caller's output pointer (checked non-null by callers).
    unsafe { out.write(value) };
}

/// The IID at the caller's pointer.
pub fn iid_at(p: *const Guid) -> Guid {
    if p.is_null() {
        return [0xff; 16];
    }
    // SAFETY: the caller's GUID.
    unsafe { *p }
}

/// A vtable (function addresses).
pub struct VTable<const N: usize>(pub [*const (); N]);
// SAFETY: function addresses only.
unsafe impl<const N: usize> Sync for VTable<N> {}

/// A list of plain values (handles, ids, elements' nodes) on the heap
/// (grows; a failed push reports false).
pub struct List<T: Copy = usize> {
    p: *mut T,
    len: usize,
    cap: usize,
}

impl<T: Copy> List<T> {
    pub const fn new() -> List<T> {
        List { p: core::ptr::null_mut(), len: 0, cap: 0 }
    }

    pub fn push(&mut self, value: T) -> bool {
        if self.len == self.cap {
            let cap = (self.cap * 2).max(16);
            let p = realloc(self.p.cast(), cap * size_of::<T>()).cast::<T>();
            if p.is_null() {
                return false;
            }
            self.p = p;
            self.cap = cap;
        }
        // SAFETY: room for the item.
        unsafe { self.p.add(self.len).write(value) };
        self.len += 1;
        true
    }

    pub fn as_slice(&self) -> &[T] {
        if self.p.is_null() {
            return &[];
        }
        // SAFETY: `len` items written.
        unsafe { core::slice::from_raw_parts(self.p, self.len) }
    }
}

impl<T: Copy> Default for List<T> {
    fn default() -> List<T> {
        List::new()
    }
}

impl<T: Copy> Drop for List<T> {
    fn drop(&mut self) {
        free(self.p.cast());
    }
}

// ---- VARIANTs and BSTRs ----

pub const VT_EMPTY: u16 = 0;
pub const VT_I4: u16 = 3;
pub const VT_R8: u16 = 5;
pub const VT_BSTR: u16 = 8;
pub const VT_BOOL: u16 = 11;
pub const VT_UNKNOWN: u16 = 13;
pub const VT_ARRAY: u16 = 0x2000;

/// VARIANT (24 bytes on x64): the type, three reserved words, the value.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Variant {
    pub vt: u16,
    pub reserved: [u16; 3],
    pub data: [u64; 2],
}

impl Variant {
    pub const EMPTY: Variant = Variant { vt: VT_EMPTY, reserved: [0; 3], data: [0; 2] };

    pub fn int(i: i32) -> Variant {
        Variant { vt: VT_I4, reserved: [0; 3], data: [u64::from(i as u32), 0] }
    }

    pub fn boolean(b: bool) -> Variant {
        // VARIANT_TRUE is -1.
        Variant { vt: VT_BOOL, reserved: [0; 3], data: [if b { 0xffff } else { 0 }, 0] }
    }

    /// Takes ownership of `s` (a BSTR; EMPTY when null).
    pub fn bstr(s: *mut u16) -> Variant {
        if s.is_null() {
            return Variant::EMPTY;
        }
        Variant { vt: VT_BSTR, reserved: [0; 3], data: [s as u64, 0] }
    }

    pub fn as_int(&self) -> i32 {
        self.data[0] as u32 as i32
    }

    pub fn as_bool(&self) -> bool {
        self.data[0] as u16 != 0
    }

    /// Takes ownership of one reference to `p` (EMPTY when null).
    pub fn unknown(p: *mut c_void) -> Variant {
        if p.is_null() {
            return Variant::EMPTY;
        }
        Variant { vt: VT_UNKNOWN, reserved: [0; 3], data: [p as u64, 0] }
    }

    pub fn as_bstr(&self) -> *mut u16 {
        self.data[0] as usize as *mut u16
    }

    pub fn clear(&mut self) {
        // SAFETY: a VARIANT we own.
        unsafe { VariantClear(self) };
    }
}

/// A new BSTR holding `s` (null without memory).
pub fn bstr(s: &[u16]) -> *mut u16 {
    // SAFETY: `s.len()` characters at its pointer.
    unsafe { SysAllocStringLen(s.as_ptr(), s.len() as u32) }
}

/// The characters of a BSTR.
pub fn bstr_chars<'a>(s: *const u16) -> &'a [u16] {
    if s.is_null() {
        return &[];
    }
    // SAFETY: a BSTR: its length prefix counts its characters.
    unsafe { core::slice::from_raw_parts(s, SysStringLen(s) as usize) }
}

/// ASCII text as UTF-16, in a fixed buffer.
pub fn wide<const N: usize>(s: &str) -> ([u16; N], usize) {
    let mut out = [0u16; N];
    let mut n = 0;
    for c in s.encode_utf16().take(N) {
        out[n] = c;
        n += 1;
    }
    (out, n)
}

/// A BSTR of ASCII text.
pub fn bstr_str(s: &str) -> *mut u16 {
    let (w, n) = wide::<64>(s);
    bstr(&w[..n])
}

// ---- The class object ----

extern "system" fn factory_query(
    this: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    match iid_at(iid) {
        IID_IUNKNOWN | IID_ICLASSFACTORY => {
            put(out, this);
            S_OK
        }
        _ => {
            put(out, core::ptr::null_mut());
            E_NOINTERFACE
        }
    }
}

/// The class object is static: its references are not counted.
extern "system" fn factory_add_ref(_this: *mut c_void) -> u32 {
    2
}

extern "system" fn factory_release(_this: *mut c_void) -> u32 {
    1
}

extern "system" fn factory_create(
    this: *mut Factory,
    outer: *mut c_void,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if !outer.is_null() {
        return CLASS_E_NOAGGREGATION;
    }
    // SAFETY: one of our two class objects.
    automation::create(unsafe { (*this).v8 }, iid, out)
}

extern "system" fn factory_lock(_this: *mut c_void, _lock: i32) -> HResult {
    S_OK
}

static FACTORY_VTABLE: VTable<5> = VTable([
    factory_query as *const (),
    factory_add_ref as *const (),
    factory_release as *const (),
    factory_create as *const (),
    factory_lock as *const (),
]);

/// A class object: its vtable pointer, which callers read, and which
/// class it makes (CUIAutomation8 answers IUIAutomation2..6 too).
#[repr(C)]
struct Factory {
    vtable: *const VTable<5>,
    v8: bool,
}
// SAFETY: an immutable pointer to a static.
unsafe impl Sync for Factory {}
static FACTORY: Factory = Factory { vtable: &FACTORY_VTABLE, v8: false };
static FACTORY8: Factory = Factory { vtable: &FACTORY_VTABLE, v8: true };

/// DllGetClassObject: the class object of CUIAutomation.
#[unsafe(no_mangle)]
pub extern "system" fn DllGetClassObject(
    clsid: *const Guid,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let factory = match iid_at(clsid) {
        CLSID_CUIAUTOMATION => &FACTORY,
        CLSID_CUIAUTOMATION8 => &FACTORY8,
        other => {
            missing(b"UIAutomationCore.dll!DllGetClassObject(class)", u64::from(other[0]));
            return CLASS_E_CLASSNOTAVAILABLE;
        }
    };
    factory_query(factory as *const Factory as *mut c_void, iid, out)
}

// ---- The reserved values ----

/// The not-supported and mixed-attribute values (IUIAutomation's
/// ReservedNotSupportedValue and ReservedMixedAttributeValue): one static
/// IUnknown each, whose identity is what clients compare (FlaUI compares
/// a property's value with the not-supported object). Their references
/// are not counted.
#[repr(C)]
pub struct Reserved {
    vtable: *const VTable<3>,
}
// SAFETY: an immutable pointer to a static.
unsafe impl Sync for Reserved {}

extern "system" fn reserved_query(
    this: *mut Reserved,
    iid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    if iid_at(iid) == IID_IUNKNOWN {
        put(out, this.cast());
        S_OK
    } else {
        put(out, core::ptr::null_mut());
        E_NOINTERFACE
    }
}

extern "system" fn reserved_add_ref(_this: *mut Reserved) -> u32 {
    2
}

extern "system" fn reserved_release(_this: *mut Reserved) -> u32 {
    1
}

static RESERVED_VTABLE: VTable<3> = VTable([
    reserved_query as *const (),
    reserved_add_ref as *const (),
    reserved_release as *const (),
]);
// Two statics: two addresses, so two identities.
static NOT_SUPPORTED: Reserved = Reserved { vtable: &RESERVED_VTABLE };
static MIXED_ATTRIBUTE: Reserved = Reserved { vtable: &RESERVED_VTABLE };

/// The not-supported value's IUnknown.
pub fn not_supported() -> *mut c_void {
    &NOT_SUPPORTED as *const Reserved as *mut c_void
}

/// The mixed-attribute value's IUnknown.
pub fn mixed_attribute() -> *mut c_void {
    &MIXED_ATTRIBUTE as *const Reserved as *mut c_void
}

/// Whether `v` is the not-supported value (VT_UNKNOWN holding it).
pub fn is_not_supported(v: &Variant) -> bool {
    v.vt == VT_UNKNOWN && v.data[0] as usize == not_supported() as usize
}

impl Variant {
    /// The not-supported value as a VARIANT (VT_UNKNOWN; its reference
    /// needs no release).
    pub fn not_supported() -> Variant {
        Variant { vt: VT_UNKNOWN, reserved: [0; 3], data: [not_supported() as u64, 0] }
    }
}

/// DllCanUnloadNow: S_FALSE (the DLL stays loaded).
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HResult {
    S_FALSE
}

// ---- The provider API (UIAutomationCoreApi.h) ----

/// UiaClientsAreListening: no client listens for events (gala's client
/// has no events yet).
#[unsafe(no_mangle)]
pub extern "system" fn UiaClientsAreListening() -> i32 {
    0
}

/// UiaReturnRawElementProvider (a window's WM_GETOBJECT answer): providers
/// in programs are not used yet, so the message stays unanswered (0).
#[unsafe(no_mangle)]
pub extern "system" fn UiaReturnRawElementProvider(
    _hwnd: Hwnd,
    _wparam: usize,
    _lparam: isize,
    _el: *mut c_void,
) -> isize {
    missing(b"UIAutomationCore.dll!UiaReturnRawElementProvider", 0);
    0
}

/// UiaHostProviderFromHwnd: not yet (E_NOTIMPL, recorded).
#[unsafe(no_mangle)]
pub extern "system" fn UiaHostProviderFromHwnd(_hwnd: Hwnd, out: *mut *mut c_void) -> HResult {
    if !out.is_null() {
        put(out, core::ptr::null_mut());
    }
    missing(b"UIAutomationCore.dll!UiaHostProviderFromHwnd", 0);
    E_NOTIMPL
}

/// UiaRaiseAutomationEvent and friends: no client listens, so there is
/// nobody to tell (S_OK, as documented when no client listens).
#[unsafe(no_mangle)]
pub extern "system" fn UiaRaiseAutomationEvent(_provider: *mut c_void, _id: i32) -> HResult {
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn UiaRaiseAutomationPropertyChangedEvent(
    _provider: *mut c_void,
    _id: i32,
    _old: *const Variant,
    _new: *const Variant,
) -> HResult {
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn UiaRaiseStructureChangedEvent(
    _provider: *mut c_void,
    _kind: i32,
    _id: *const i32,
    _len: i32,
) -> HResult {
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn UiaDisconnectProvider(_provider: *mut c_void) -> HResult {
    S_OK
}

#[unsafe(no_mangle)]
pub extern "system" fn UiaDisconnectAllProviders() -> HResult {
    S_OK
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
