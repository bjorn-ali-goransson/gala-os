//! gala's Windows.UI.dll: the Windows Runtime class
//! Windows.UI.ViewManagement.UISettings (the DLL Windows registers it
//! from), which desktop programs ask for the system's colours and UI
//! preferences (ODT's setup.exe colours its window with it, ROADMAP OF).
//! Registered under `HKLM\SOFTWARE\Microsoft\WindowsRuntime\
//! ActivatableClassId` in both registry views (kernel/src/nt/
//! registry.seed); ole32's RoGetActivationFactory loads it and calls
//! DllGetActivationFactory.
//!
//! What Windows answers, as Wine's dlls/windows.ui/tests/uisettings.c
//! checks it on Windows (and `uisettings_test` here, x64 and x86):
//! - The factory (IActivationFactory, not agile) makes UISettings
//!   objects with IUISettings to IUISettings6 (the interfaces' method
//!   order from Wine's include/windows.ui.viewmanagement.idl), agile
//!   (IAgileObject) and with weak references (IWeakReferenceSource; the
//!   weak reference resolves to the object while it lives, to NULL
//!   after).
//! - GetColorValue: Foreground and Background from the apps' theme
//!   (`HKCU\...\Themes\Personalize` AppsUseLightTheme, light when
//!   absent: black on white; dark: white on black); the accent shades
//!   from `HKCU\...\Explorer\Accent` AccentPalette (eight 0x00BBGGRR
//!   entries: Light3, Light2, Light1, Accent, Dark1, Dark2, Dark3), which
//!   is written with Windows' default palette when absent.
//! - UIElementColor: the thirteen classic elements are GetSysColor's
//!   colours (alpha 255); the others, and unknown values, are all zero.
//! - AnimationsEnabled is SPI_GETCLIENTAREAANIMATION; TextScaleFactor is
//!   `HKCU\Software\Microsoft\Accessibility` TextScaleFactor (percent,
//!   100 when absent).
//!
//! From the Win32 settings each one mirrors (not checked against
//! Windows: printed by the test): CursorSize (SM_CXCURSOR/SM_CYCURSOR),
//! the scroll bar sizes (SM_CXVSCROLL/SM_CYHSCROLL, SM_CXHSCROLL/
//! SM_CYVSCROLL, SM_CXHTHUMB/SM_CYVTHUMB), MessageDuration
//! (SPI_GETMESSAGEDURATION, Windows' default 5 when user32 has no
//! answer), CaretWidth (SPI_GETCARETWIDTH), CaretBlinkRate (530 ms,
//! Windows' default; gala's caret does not blink), DoubleClickTime,
//! MouseHoverTime (SPI_GETMOUSEHOVERTIME), HandPreference (right-handed,
//! menus dropping left: SPI_GETMENUDROPALIGNMENT FALSE), CaretBrowsing
//! off, AdvancedEffectsEnabled (`...\Themes\Personalize`
//! EnableTransparency, on when absent), AutoHideScrollBars (`HKCU\
//! Control Panel\Accessibility` DynamicScrollbars, on when absent).
//!
//! The change events (TextScaleFactorChanged, ColorValuesChanged and the
//! others) keep their handlers until removed, but are never raised: gala
//! changes none of these settings while a program runs (recorded once
//! per event, `windows.ui.dll!UISettings(<event> never raised)`).
//! GetColorValue(Complement), which the documentation says is not
//! supported, is E_NOTIMPL, recorded.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

#[cfg(target_arch = "x86")]
use ntrt as _;

#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn __CxxFrameHandler3();
    fn __chkstk();
}

#[cfg(target_arch = "x86_64")]
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

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "user32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "user32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetSysColor(index: i32) -> u32;
    fn GetSystemMetrics(index: i32) -> i32;
    fn GetDoubleClickTime() -> u32;
    fn SystemParametersInfoW(action: u32, param: u32, out: *mut c_void, update: u32) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "advapi32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "advapi32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn RegOpenKeyExW(
        key: usize,
        name: *const u16,
        options: u32,
        access: u32,
        out: *mut usize,
    ) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn RegCreateKeyExW(
        key: usize,
        name: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        access: u32,
        security: *const c_void,
        out: *mut usize,
        disposition: *mut u32,
    ) -> i32;
    fn RegQueryValueExW(
        key: usize,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        len: *mut u32,
    ) -> i32;
    fn RegSetValueExW(
        key: usize,
        name: *const u16,
        reserved: u32,
        kind: u32,
        data: *const u8,
        len: u32,
    ) -> i32;
    fn RegCloseKey(key: usize) -> i32;
}

// The HSTRING functions (the API set resolves to the DLL that has them).
#[cfg_attr(
    target_arch = "x86",
    link(
        name = "api-ms-win-core-winrt-string-l1-1-0",
        kind = "raw-dylib",
        import_name_type = "undecorated"
    )
)]
#[cfg_attr(
    not(target_arch = "x86"),
    link(name = "api-ms-win-core-winrt-string-l1-1-0", kind = "raw-dylib")
)]
unsafe extern "system" {
    fn WindowsCreateString(text: *const u16, len: u32, out: *mut HString) -> HResult;
    fn WindowsGetStringRawBuffer(s: HString, len: *mut u32) -> *const u16;
}

type HResult = i32;
type Guid = [u8; 16];
type HString = *mut c_void;

const S_OK: HResult = 0;
const S_FALSE: HResult = 1;
const E_NOTIMPL: HResult = 0x8000_4001u32 as i32;
const E_NOINTERFACE: HResult = 0x8000_4002u32 as i32;
const E_POINTER: HResult = 0x8000_4003u32 as i32;
const E_INVALIDARG: HResult = 0x8007_0057u32 as i32;
const E_OUTOFMEMORY: HResult = 0x8007_000eu32 as i32;
const CLASS_E_CLASSNOTAVAILABLE: HResult = 0x8004_0111u32 as i32;

/// The ledger's kinds (GalaRecordMissing's codes).
const LEDGER_UNSUPPORTED: u32 = 2;
const LEDGER_NOT_IMPLEMENTED: u32 = 3;
const LEDGER_OTHER: u32 = 5;

/// A GUID in its binary layout from its text form's fields.
const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

const IID_IUNKNOWN: Guid = guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IINSPECTABLE: Guid =
    guid(0xaf86e2e0, 0xb12d, 0x4c6a, [0x9c, 0x5a, 0xd7, 0xaa, 0x65, 0x10, 0x1e, 0x90]);
const IID_IAGILEOBJECT: Guid =
    guid(0x94ea2b94, 0xe9cc, 0x49e0, [0xc0, 0xff, 0xee, 0x64, 0xca, 0x8f, 0x5b, 0x90]);
const IID_IACTIVATIONFACTORY: Guid = guid(0x35, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IWEAKREFERENCE: Guid = guid(0x37, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IWEAKREFERENCESOURCE: Guid = guid(0x38, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
/// IUISettings to IUISettings6, in slot order.
const IID_IUISETTINGS: [Guid; 6] = [
    guid(0x85361600, 0x1c63, 0x4627, [0xbc, 0xb1, 0x3a, 0x89, 0xe0, 0xbc, 0x9c, 0x55]),
    guid(0xbad82401, 0x2721, 0x44f9, [0xbb, 0x91, 0x2b, 0xb2, 0x28, 0xbe, 0x44, 0x2f]),
    guid(0x03021be4, 0x5254, 0x4781, [0x81, 0x94, 0x51, 0x68, 0xf7, 0xd0, 0x6d, 0x7b]),
    guid(0x52bb3002, 0x919b, 0x4d6b, [0x9b, 0x78, 0x8d, 0xd6, 0x6f, 0xf4, 0xb9, 0x3b]),
    guid(0x5349d588, 0x0cb5, 0x5f05, [0xbd, 0x34, 0x70, 0x6b, 0x32, 0x31, 0xf0, 0xbd]),
    guid(0xaef19bd7, 0xfe31, 0x5a04, [0xad, 0xa4, 0x46, 0x9a, 0xae, 0xc6, 0xdf, 0xa9]),
];

const CLASS_NAME: &str = "Windows.UI.ViewManagement.UISettings";
const PTR: usize = core::mem::size_of::<usize>();
/// HKEY_CURRENT_USER.
const HKCU: usize = 0x8000_0001;
const KEY_QUERY_VALUE: u32 = 1;
const KEY_SET_VALUE: u32 = 2;
const REG_BINARY: u32 = 3;
const REG_DWORD: u32 = 4;

/// A vtable (function addresses).
struct VTable<const N: usize>([*const (); N]);
// SAFETY: function addresses only.
unsafe impl<const N: usize> Sync for VTable<N> {}

fn put<T>(out: *mut T, value: T) {
    // SAFETY: the caller's output pointer (checked non-null by callers).
    unsafe { out.write(value) };
}

fn record(text: &[u8], what: u32, detail: u64) {
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(text.as_ptr(), text.len(), what, detail) };
}

/// A NUL-terminated UTF-16 copy of ASCII `text`.
fn wide<const N: usize>(text: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (slot, b) in out.iter_mut().zip(text.bytes()) {
        *slot = u16::from(b);
    }
    out
}

/// HKCU\`key` value `name` as a DWORD, or None.
fn reg_dword(key: &str, name: &str) -> Option<u32> {
    let (k, n) = (wide::<128>(key), wide::<64>(name));
    let mut h = 0;
    // SAFETY: NUL-terminated names, our out-parameters.
    unsafe {
        if RegOpenKeyExW(HKCU, k.as_ptr(), 0, KEY_QUERY_VALUE, &mut h) != 0 {
            return None;
        }
        let (mut kind, mut v, mut len) = (0u32, 0u32, 4u32);
        let r = RegQueryValueExW(
            h,
            n.as_ptr(),
            core::ptr::null_mut(),
            &mut kind,
            (&raw mut v).cast(),
            &mut len,
        );
        RegCloseKey(h);
        (r == 0 && kind == REG_DWORD && len == 4).then_some(v)
    }
}

const PERSONALIZE: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";
const ACCENT: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent";

/// Windows' default accent palette (AccentPalette as Windows writes it
/// when absent; Wine's uisettings.c): 0x00BBGGRR, lightest first.
const DEFAULT_PALETTE: [u32; 8] =
    [0x00ff_d8a6, 0x00ed_b976, 0x00e3_9c42, 0x00d7_7800, 0x009e_5a00, 0x0075_4200, 0x0042_2600, 0];

/// AccentPalette entry `i`, writing the default palette when the value is
/// absent.
fn accent(i: usize) -> u32 {
    let k = wide::<128>(ACCENT);
    let n = wide::<16>("AccentPalette");
    let mut palette = [0u32; 8];
    let mut h = 0;
    // SAFETY: NUL-terminated names, our buffers.
    unsafe {
        if RegCreateKeyExW(
            HKCU,
            k.as_ptr(),
            0,
            core::ptr::null(),
            0,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            core::ptr::null(),
            &mut h,
            core::ptr::null_mut(),
        ) != 0
        {
            return DEFAULT_PALETTE[i];
        }
        let (mut kind, mut len) = (0u32, 32u32);
        let r = RegQueryValueExW(
            h,
            n.as_ptr(),
            core::ptr::null_mut(),
            &mut kind,
            palette.as_mut_ptr().cast(),
            &mut len,
        );
        if r != 0 || kind != REG_BINARY || (len as usize) < 4 * (i + 1) {
            palette = DEFAULT_PALETTE;
            RegSetValueExW(h, n.as_ptr(), 0, REG_BINARY, palette.as_ptr().cast(), 32);
        }
        RegCloseKey(h);
    }
    palette[i]
}

/// Windows.UI.Color {A, R, G, B} from 0x00BBGGRR (a COLORREF or palette
/// entry), opaque.
fn color(bgr: u32) -> [u8; 4] {
    [255, bgr as u8, (bgr >> 8) as u8, (bgr >> 16) as u8]
}

// ---- The UISettings object ----

/// Its interface slots: IUISettings..IUISettings6 (0-5), then
/// IWeakReferenceSource (6).
const SLOTS: usize = 7;
const WEAK_SOURCE: usize = 6;
/// Event handlers it can hold at once.
const HANDLERS: usize = 32;

#[repr(C)]
struct Settings {
    vtables: [*const (); SLOTS],
    control: *mut Control,
    lock: AtomicBool,
    /// (token, handler) pairs; token 0 is a free entry.
    handlers: [(i64, usize); HANDLERS],
}

/// The weak reference's control block (its IWeakReference): the
/// object's strong count and the block's own count (one for the object
/// while it lives, one per weak reference handed out).
#[repr(C)]
struct Control {
    vtable: *const (),
    strong: AtomicU32,
    weak: AtomicU32,
    object: *mut Settings,
}

fn object<'a, const K: usize>(this: usize) -> &'a mut Settings {
    // SAFETY: `this` is interface slot K of a live Settings (COM's
    // contract: callers hold a reference).
    unsafe { &mut *((this - K * PTR) as *mut Settings) }
}

fn control<'a>(s: &Settings) -> &'a Control {
    // SAFETY: the block outlives the object (the object holds a count).
    unsafe { &*s.control }
}

fn free(p: *mut u8) {
    // SAFETY: a block from HeapAlloc.
    unsafe { HeapFree(GetProcessHeap(), 0, p) };
}

/// Calls IUnknown method `index` (1 AddRef, 2 Release) of `p`.
fn unknown_call(p: usize, index: usize) -> u32 {
    // SAFETY: a COM object the caller handed us; its vtable's slot.
    unsafe {
        let vtable = *(p as *const *const usize);
        let f: extern "system" fn(usize) -> u32 = core::mem::transmute(*vtable.add(index));
        f(p)
    }
}

fn query_object(s: &mut Settings, iid: *const Guid, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    if iid.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *iid };
    let slot = if [IID_IUNKNOWN, IID_IINSPECTABLE, IID_IAGILEOBJECT].contains(&iid) {
        0
    } else if let Some(i) = IID_IUISETTINGS.iter().position(|g| *g == iid) {
        i
    } else if iid == IID_IWEAKREFERENCESOURCE {
        WEAK_SOURCE
    } else {
        record_iid(&iid);
        return E_NOINTERFACE;
    };
    control(s).strong.fetch_add(1, Ordering::Relaxed);
    put(out, &s.vtables[slot] as *const *const () as usize);
    S_OK
}

/// An interface UISettings is asked for and is not, to the ledger
/// (`windows.ui.dll!UISettings QueryInterface({iid})`, kind other): what
/// a program looks for shows in the run.
fn record_iid(iid: &Guid) {
    let mut line =
        *b"windows.ui.dll!UISettings QueryInterface({00000000-0000-0000-0000-000000000000})";
    let d1 = u32::from_le_bytes([iid[0], iid[1], iid[2], iid[3]]);
    let d2 = u16::from_le_bytes([iid[4], iid[5]]);
    let d3 = u16::from_le_bytes([iid[6], iid[7]]);
    let hex = b"0123456789abcdef";
    let mut at = 42;
    let mut put_hex = |v: u64, digits: usize, at: &mut usize| {
        for k in (0..digits).rev() {
            line[*at] = hex[((v >> (4 * k)) & 0xf) as usize];
            *at += 1;
        }
    };
    put_hex(d1.into(), 8, &mut at);
    at += 1;
    put_hex(d2.into(), 4, &mut at);
    at += 1;
    put_hex(d3.into(), 4, &mut at);
    at += 1;
    put_hex(u64::from(iid[8]) << 8 | u64::from(iid[9]), 4, &mut at);
    at += 1;
    for b in &iid[10..16] {
        put_hex((*b).into(), 2, &mut at);
    }
    record(&line, LEDGER_OTHER, 0);
}

fn release_object(s: &mut Settings) -> u32 {
    let c = control(s);
    let left = c.strong.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        for (token, handler) in s.handlers {
            if token != 0 {
                unknown_call(handler, 2);
            }
        }
        let block = s.control;
        free((s as *mut Settings).cast());
        release_control(block);
    }
    left
}

extern "system" fn query<const K: usize>(
    this: usize,
    iid: *const Guid,
    out: *mut usize,
) -> HResult {
    query_object(object::<K>(this), iid, out)
}
extern "system" fn add_ref<const K: usize>(this: usize) -> u32 {
    control(object::<K>(this)).strong.fetch_add(1, Ordering::Relaxed) + 1
}
extern "system" fn release<const K: usize>(this: usize) -> u32 {
    release_object(object::<K>(this))
}
extern "system" fn get_iids(_this: usize, count: *mut u32, iids: *mut *mut Guid) -> HResult {
    if count.is_null() || iids.is_null() {
        return E_POINTER;
    }
    put(count, 0);
    put(iids, core::ptr::null_mut());
    S_OK
}
extern "system" fn class_name(_this: usize, out: *mut HString) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let mut buf = [0u16; 64];
    let mut n = 0;
    for c in CLASS_NAME.encode_utf16() {
        buf[n] = c;
        n += 1;
    }
    // SAFETY: n units at buf.
    unsafe { WindowsCreateString(buf.as_ptr(), n as u32, out) }
}
extern "system" fn get_trust_level(_this: usize, level: *mut i32) -> HResult {
    if level.is_null() {
        return E_POINTER;
    }
    put(level, 0); // BaseTrust
    S_OK
}

// Event handlers: kept (AddRef) until removed; never raised.

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

/// The ledger's name of each event (add_* methods pass their index).
const EVENTS: [&[u8]; 6] = [
    b"windows.ui.dll!UISettings(TextScaleFactorChanged never raised)",
    b"windows.ui.dll!UISettings(ColorValuesChanged never raised)",
    b"windows.ui.dll!UISettings(AdvancedEffectsEnabledChanged never raised)",
    b"windows.ui.dll!UISettings(AutoHideScrollBarsChanged never raised)",
    b"windows.ui.dll!UISettings(AnimationsEnabledChanged never raised)",
    b"windows.ui.dll!UISettings(MessageDurationChanged never raised)",
];

fn add_handler(s: &mut Settings, event: usize, handler: usize, token: *mut i64) -> HResult {
    if token.is_null() {
        return E_POINTER;
    }
    if handler == 0 {
        return E_INVALIDARG;
    }
    while s.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    let slot = s.handlers.iter().position(|h| h.0 == 0);
    let r = match slot {
        Some(i) => {
            let t = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed) as i64;
            unknown_call(handler, 1);
            s.handlers[i] = (t, handler);
            put(token, t);
            S_OK
        }
        None => E_OUTOFMEMORY,
    };
    s.lock.store(false, Ordering::Release);
    if r == S_OK {
        record(EVENTS[event], LEDGER_UNSUPPORTED, 0);
    } else {
        record(b"windows.ui.dll!UISettings(handlers full)", LEDGER_OTHER, HANDLERS as u64);
    }
    r
}

fn remove_handler(s: &mut Settings, token: i64) -> HResult {
    while s.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    let handler = s.handlers.iter_mut().find(|h| h.0 == token && token != 0).map(|h| {
        let handler = h.1;
        *h = (0, 0);
        handler
    });
    s.lock.store(false, Ordering::Release);
    if let Some(handler) = handler {
        unknown_call(handler, 2);
    }
    S_OK
}

extern "system" fn add_event<const K: usize, const E: usize>(
    this: usize,
    handler: usize,
    token: *mut i64,
) -> HResult {
    add_handler(object::<K>(this), E, handler, token)
}
extern "system" fn remove_event<const K: usize>(this: usize, token: i64) -> HResult {
    remove_handler(object::<K>(this), token)
}

// IUISettings.

fn out_u32(out: *mut u32, v: u32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, v);
    S_OK
}
fn out_bool(out: *mut u8, v: bool) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, u8::from(v));
    S_OK
}
/// A Windows.Foundation.Size from two system metrics.
fn out_size(out: *mut [f32; 2], x: i32, y: i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: plain calls.
    let (w, h) = unsafe { (GetSystemMetrics(x), GetSystemMetrics(y)) };
    put(out, [w as f32, h as f32]);
    S_OK
}
/// SystemParametersInfoW's UINT/BOOL answer for `action`, or None.
fn spi(action: u32) -> Option<u32> {
    let mut v = 0u32;
    // SAFETY: our out-parameter.
    (unsafe { SystemParametersInfoW(action, 0, (&raw mut v).cast(), 0) } != 0).then_some(v)
}

const SPI_GETMENUDROPALIGNMENT: u32 = 0x001b;
const SPI_GETMOUSEHOVERTIME: u32 = 0x0066;
const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042;
const SPI_GETCARETWIDTH: u32 = 0x2006;
const SPI_GETMESSAGEDURATION: u32 = 0x2016;

extern "system" fn hand_preference(_this: usize, out: *mut u32) -> HResult {
    // HandPreference: LeftHanded 0, RightHanded 1.
    out_u32(out, if spi(SPI_GETMENUDROPALIGNMENT).unwrap_or(0) != 0 { 0 } else { 1 })
}
extern "system" fn cursor_size(_this: usize, out: *mut [f32; 2]) -> HResult {
    out_size(out, 13, 14) // SM_CXCURSOR, SM_CYCURSOR
}
extern "system" fn scroll_bar_size(_this: usize, out: *mut [f32; 2]) -> HResult {
    out_size(out, 2, 3) // SM_CXVSCROLL, SM_CYHSCROLL
}
extern "system" fn scroll_bar_arrow_size(_this: usize, out: *mut [f32; 2]) -> HResult {
    out_size(out, 21, 20) // SM_CXHSCROLL, SM_CYVSCROLL
}
extern "system" fn scroll_bar_thumb_box_size(_this: usize, out: *mut [f32; 2]) -> HResult {
    out_size(out, 10, 9) // SM_CXHTHUMB, SM_CYVTHUMB
}
extern "system" fn message_duration(_this: usize, out: *mut u32) -> HResult {
    out_u32(out, spi(SPI_GETMESSAGEDURATION).unwrap_or(5))
}
extern "system" fn animations_enabled(_this: usize, out: *mut u8) -> HResult {
    out_bool(out, spi(SPI_GETCLIENTAREAANIMATION).unwrap_or(1) != 0)
}
extern "system" fn caret_browsing_enabled(_this: usize, out: *mut u8) -> HResult {
    out_bool(out, false)
}
extern "system" fn caret_blink_rate(_this: usize, out: *mut u32) -> HResult {
    out_u32(out, 530)
}
extern "system" fn caret_width(_this: usize, out: *mut u32) -> HResult {
    out_u32(out, spi(SPI_GETCARETWIDTH).unwrap_or(1))
}
extern "system" fn double_click_time(_this: usize, out: *mut u32) -> HResult {
    // SAFETY: a plain call.
    out_u32(out, unsafe { GetDoubleClickTime() })
}
extern "system" fn mouse_hover_time(_this: usize, out: *mut u32) -> HResult {
    out_u32(out, spi(SPI_GETMOUSEHOVERTIME).unwrap_or(400))
}
/// UIElementColor(element): UIElementType 0-12 map to these system
/// colours (COLOR_*); the rest are all zero.
const ELEMENT_COLORS: [i32; 13] = [2, 1, 15, 18, 9, 17, 13, 14, 26, 3, 19, 5, 8];
extern "system" fn ui_element_color(_this: usize, element: u32, out: *mut [u8; 4]) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let value = match ELEMENT_COLORS.get(element as usize) {
        // SAFETY: a plain call.
        Some(&index) => color(unsafe { GetSysColor(index) }),
        None => [0; 4],
    };
    put(out, value);
    S_OK
}

static V1: VTable<19> = VTable([
    query::<0> as *const (),
    add_ref::<0> as *const (),
    release::<0> as *const (),
    get_iids as *const (),
    class_name as *const (),
    get_trust_level as *const (),
    hand_preference as *const (),
    cursor_size as *const (),
    scroll_bar_size as *const (),
    scroll_bar_arrow_size as *const (),
    scroll_bar_thumb_box_size as *const (),
    message_duration as *const (),
    animations_enabled as *const (),
    caret_browsing_enabled as *const (),
    caret_blink_rate as *const (),
    caret_width as *const (),
    double_click_time as *const (),
    mouse_hover_time as *const (),
    ui_element_color as *const (),
]);

// IUISettings2.

extern "system" fn text_scale_factor(_this: usize, out: *mut f64) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let percent = reg_dword("Software\\Microsoft\\Accessibility", "TextScaleFactor")
        .filter(|p| (100..=225).contains(p))
        .unwrap_or(100);
    put(out, f64::from(percent) / 100.0);
    S_OK
}

static V2: VTable<9> = VTable([
    query::<1> as *const (),
    add_ref::<1> as *const (),
    release::<1> as *const (),
    get_iids as *const (),
    class_name as *const (),
    get_trust_level as *const (),
    text_scale_factor as *const (),
    add_event::<1, 0> as *const (),
    remove_event::<1> as *const (),
]);

// IUISettings3.

/// UIColorType: Background 0, Foreground 1, AccentDark3..AccentDark1
/// 2-4, Accent 5, AccentLight1..3 6-8, Complement 9.
extern "system" fn get_color_value(_this: usize, kind: u32, out: *mut [u8; 4]) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let light = reg_dword(PERSONALIZE, "AppsUseLightTheme").unwrap_or(1) != 0;
    let value = match kind {
        0 => color(if light { 0x00ff_ffff } else { 0 }),
        1 => color(if light { 0 } else { 0x00ff_ffff }),
        // AccentPalette: Light3, Light2, Light1, Accent, Dark1, Dark2, Dark3.
        2..=8 => color(accent([6, 5, 4, 3, 2, 1, 0][kind as usize - 2])),
        _ => {
            record(
                b"windows.ui.dll!UISettings::GetColorValue(type)",
                LEDGER_NOT_IMPLEMENTED,
                kind.into(),
            );
            return E_NOTIMPL;
        }
    };
    put(out, value);
    S_OK
}

static V3: VTable<9> = VTable([
    query::<2> as *const (),
    add_ref::<2> as *const (),
    release::<2> as *const (),
    get_iids as *const (),
    class_name as *const (),
    get_trust_level as *const (),
    get_color_value as *const (),
    add_event::<2, 1> as *const (),
    remove_event::<2> as *const (),
]);

// IUISettings4.

extern "system" fn advanced_effects_enabled(_this: usize, out: *mut u8) -> HResult {
    out_bool(out, reg_dword(PERSONALIZE, "EnableTransparency").unwrap_or(1) != 0)
}

static V4: VTable<9> = VTable([
    query::<3> as *const (),
    add_ref::<3> as *const (),
    release::<3> as *const (),
    get_iids as *const (),
    class_name as *const (),
    get_trust_level as *const (),
    advanced_effects_enabled as *const (),
    add_event::<3, 2> as *const (),
    remove_event::<3> as *const (),
]);

// IUISettings5.

extern "system" fn auto_hide_scroll_bars(_this: usize, out: *mut u8) -> HResult {
    let v = reg_dword("Control Panel\\Accessibility", "DynamicScrollbars").unwrap_or(1);
    out_bool(out, v != 0)
}

static V5: VTable<9> = VTable([
    query::<4> as *const (),
    add_ref::<4> as *const (),
    release::<4> as *const (),
    get_iids as *const (),
    class_name as *const (),
    get_trust_level as *const (),
    auto_hide_scroll_bars as *const (),
    add_event::<4, 3> as *const (),
    remove_event::<4> as *const (),
]);

// IUISettings6.

static V6: VTable<10> = VTable([
    query::<5> as *const (),
    add_ref::<5> as *const (),
    release::<5> as *const (),
    get_iids as *const (),
    class_name as *const (),
    get_trust_level as *const (),
    add_event::<5, 4> as *const (),
    remove_event::<5> as *const (),
    add_event::<5, 5> as *const (),
    remove_event::<5> as *const (),
]);

// IWeakReferenceSource.

extern "system" fn get_weak_reference(this: usize, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let s = object::<WEAK_SOURCE>(this);
    control(s).weak.fetch_add(1, Ordering::Relaxed);
    put(out, s.control as usize);
    S_OK
}

static WEAK_SOURCE_VTABLE: VTable<4> = VTable([
    query::<WEAK_SOURCE> as *const (),
    add_ref::<WEAK_SOURCE> as *const (),
    release::<WEAK_SOURCE> as *const (),
    get_weak_reference as *const (),
]);

// IWeakReference (the control block).

fn control_of<'a>(this: usize) -> &'a Control {
    // SAFETY: `this` is a live control block (its caller holds a count).
    unsafe { &*(this as *const Control) }
}

fn release_control(block: *mut Control) -> u32 {
    // SAFETY: a live block; freed when its last count goes.
    let left = unsafe { &*block }.weak.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        free(block.cast());
    }
    left
}

extern "system" fn weak_query(this: usize, iid: *const Guid, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    if iid.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *iid };
    if iid != IID_IUNKNOWN && iid != IID_IWEAKREFERENCE {
        return E_NOINTERFACE;
    }
    control_of(this).weak.fetch_add(1, Ordering::Relaxed);
    put(out, this);
    S_OK
}
extern "system" fn weak_add_ref(this: usize) -> u32 {
    control_of(this).weak.fetch_add(1, Ordering::Relaxed) + 1
}
extern "system" fn weak_release(this: usize) -> u32 {
    release_control(this as *mut Control)
}
/// Resolve(iid, out): the object's interface while it lives (a strong
/// reference taken only from a count above zero), else NULL with S_OK.
extern "system" fn resolve(this: usize, iid: *const Guid, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    let c = control_of(this);
    let mut n = c.strong.load(Ordering::Acquire);
    loop {
        if n == 0 {
            return S_OK;
        }
        match c.strong.compare_exchange(n, n + 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => break,
            Err(now) => n = now,
        }
    }
    // SAFETY: the object lives: we hold a strong count.
    let s = unsafe { &mut *c.object };
    let r = query_object(s, iid, out);
    release_object(s);
    r
}

static WEAK_VTABLE: VTable<4> = VTable([
    weak_query as *const (),
    weak_add_ref as *const (),
    weak_release as *const (),
    resolve as *const (),
]);

// ---- The factory ----

extern "system" fn factory_query(this: usize, iid: *const Guid, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    if iid.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *iid };
    if [IID_IUNKNOWN, IID_IINSPECTABLE, IID_IACTIVATIONFACTORY].contains(&iid) {
        put(out, this);
        return S_OK;
    }
    E_NOINTERFACE
}
/// The factory is static: its counts are nominal (one held by the
/// DLL), as Windows' factory gives 1 after a caller's last Release.
extern "system" fn factory_add_ref(_this: usize) -> u32 {
    2
}
extern "system" fn factory_release(_this: usize) -> u32 {
    1
}

/// IActivationFactory::ActivateInstance: a new UISettings.
extern "system" fn activate_instance(_this: usize, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    // SAFETY: plain calls; fresh memory of each object's size.
    unsafe {
        let heap = GetProcessHeap();
        let s = HeapAlloc(heap, 0, core::mem::size_of::<Settings>()).cast::<Settings>();
        let c = HeapAlloc(heap, 0, core::mem::size_of::<Control>()).cast::<Control>();
        if s.is_null() || c.is_null() {
            if !s.is_null() {
                free(s.cast());
            }
            if !c.is_null() {
                free(c.cast());
            }
            return E_OUTOFMEMORY;
        }
        c.write(Control {
            vtable: WEAK_VTABLE.0.as_ptr().cast(),
            strong: AtomicU32::new(1),
            weak: AtomicU32::new(1),
            object: s,
        });
        s.write(Settings {
            vtables: [
                V1.0.as_ptr().cast(),
                V2.0.as_ptr().cast(),
                V3.0.as_ptr().cast(),
                V4.0.as_ptr().cast(),
                V5.0.as_ptr().cast(),
                V6.0.as_ptr().cast(),
                WEAK_SOURCE_VTABLE.0.as_ptr().cast(),
            ],
            control: c,
            lock: AtomicBool::new(false),
            handlers: [(0, 0); HANDLERS],
        });
        put(out, &(*s).vtables[0] as *const *const () as usize);
    }
    S_OK
}

static FACTORY_VTABLE: VTable<7> = VTable([
    factory_query as *const (),
    factory_add_ref as *const (),
    factory_release as *const (),
    get_iids as *const (),
    class_name as *const (),
    get_trust_level as *const (),
    activate_instance as *const (),
]);

/// The factory object: its vtable pointer.
#[repr(C)]
struct Factory(*const ());
// SAFETY: immutable.
unsafe impl Sync for Factory {}
static FACTORY: Factory = Factory(&FACTORY_VTABLE as *const VTable<7> as *const ());

/// DllGetActivationFactory: UISettings' factory; CLASS_E_CLASSNOTAVAILABLE
/// for any other class (recorded with its name).
#[unsafe(no_mangle)]
pub extern "system" fn DllGetActivationFactory(class: HString, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    let mut len = 0u32;
    // SAFETY: the caller's HSTRING (NULL is the empty string).
    let p = unsafe { WindowsGetStringRawBuffer(class, &mut len) };
    let mut text = [0u8; 96];
    let n = (len as usize).min(text.len());
    for (i, slot) in text.iter_mut().take(n).enumerate() {
        // SAFETY: the string's `len` units.
        let c = unsafe { *p.add(i) };
        *slot = if (0x20..0x7f).contains(&c) { c as u8 } else { b'?' };
    }
    if &text[..n] != CLASS_NAME.as_bytes() || len as usize != CLASS_NAME.len() {
        let mut line = [0u8; 160];
        let head = b"windows.ui.dll!DllGetActivationFactory(";
        line[..head.len()].copy_from_slice(head);
        line[head.len()..head.len() + n].copy_from_slice(&text[..n]);
        line[head.len() + n] = b')';
        record(&line[..head.len() + n + 1], LEDGER_UNSUPPORTED, 0);
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    put(out, &FACTORY as *const Factory as usize);
    S_OK
}

/// DllCanUnloadNow: never (its factory is static; objects may live).
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HResult {
    S_FALSE
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
