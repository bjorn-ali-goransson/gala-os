//! gala's UIAutomationCore.dll: the client side of UI Automation
//! (IUIAutomation, UIAutomationClient.h) over user32's window tree, as
//! Microsoft documents it, so tests can find controls by AutomationId,
//! Name and ControlType and drive them through control patterns.
//!
//! - `CUIAutomation` (CoCreateInstance through ole32 and the HKCR\CLSID
//!   entry in the registry seed): the IUIAutomation object
//!   (automation.rs).
//! - Elements (element.rs) wrap an HWND; the root is the desktop
//!   (GetDesktopWindow), whose children are the visible top-level
//!   windows, and a window's children are its visible child windows in z
//!   order. Properties come from the window: AutomationId the control id,
//!   Name the text (an edit's: the static label before it), ControlType
//!   from the class and style, BoundingRectangle the window rectangle.
//! - Conditions (condition.rs): true, false, property (VARIANT compare,
//!   IgnoreCase and MatchSubstring), and, or, not.
//! - Patterns (pattern.rs): Invoke (push buttons: BM_CLICK posted, as
//!   Invoke may return before the click is handled), Value (edits:
//!   WM_SETTEXT, the window text), Toggle (check boxes: BM_GETCHECK,
//!   BM_CLICK).
//! - Tree walkers (walker.rs): the raw, control and content views and
//!   walkers with any condition.
//!
//! Not there yet (recorded in the ledger, E_NOTIMPL): providers inside
//! programs (WM_GETOBJECT, IRawElementProviderSimple), events, cache
//! requests (the BuildCache forms ignore the request and the Cached
//! getters read current values), the IUIAutomation2+ interfaces
//! (CUIAutomation8), other processes' windows.

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
mod condition;
mod element;
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
    pub fn GetWindowThreadProcessId(hwnd: Hwnd, pid: *mut u32) -> u32;
    pub fn GetFocus() -> Hwnd;
    pub fn SetFocus(hwnd: Hwnd) -> Hwnd;
    pub fn SetForegroundWindow(hwnd: Hwnd) -> i32;
    pub fn WindowFromPoint(point: Point) -> Hwnd;
    pub fn PostMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> i32;
    pub fn SendMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
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

/// A list of handles on the heap (grows; a failed push reports false).
pub struct List {
    p: *mut usize,
    len: usize,
    cap: usize,
}

impl List {
    pub const fn new() -> List {
        List { p: core::ptr::null_mut(), len: 0, cap: 0 }
    }

    pub fn push(&mut self, value: usize) -> bool {
        if self.len == self.cap {
            let cap = (self.cap * 2).max(16);
            let p = realloc(self.p.cast(), cap * size_of::<usize>()).cast::<usize>();
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

    pub fn as_slice(&self) -> &[usize] {
        if self.p.is_null() {
            return &[];
        }
        // SAFETY: `len` items written.
        unsafe { core::slice::from_raw_parts(self.p, self.len) }
    }
}

impl Default for List {
    fn default() -> List {
        List::new()
    }
}

impl Drop for List {
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
    _this: *mut c_void,
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
    automation::create(iid, out)
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

/// The class object: only its vtable pointer, which callers read.
#[allow(dead_code)]
struct Factory(*const VTable<5>);
// SAFETY: an immutable pointer to a static.
unsafe impl Sync for Factory {}
static FACTORY: Factory = Factory(&FACTORY_VTABLE);

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
    if iid_at(clsid) != CLSID_CUIAUTOMATION {
        missing(b"UIAutomationCore.dll!DllGetClassObject(class)", u64::from(iid_at(clsid)[0]));
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    factory_query(&FACTORY as *const Factory as *mut c_void, iid, out)
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
