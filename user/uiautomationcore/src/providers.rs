//! Providers inside programs (UIAutomationCoreApi.h): a window answers
//! WM_GETOBJECT with UiaRootObjectId by returning
//! UiaReturnRawElementProvider(its IRawElementProviderSimple), and the
//! element for that window takes its properties from the provider first,
//! then from the window (the host provider: UiaHostProviderFromHwnd).
//!
//! The answer is a cookie (1..=0xffff) naming a slot of the answering
//! process's table, which holds one reference to the provider until it
//! is claimed; an answer nobody claims is released when its slot is
//! reused. A client in the same process claims it itself (`of`). For
//! another process's window the client sends gala's GALA_UIA message
//! (user32's uia.rs: the property id and the cookie in wParam, a value
//! buffer in lParam, marshaled back); user32 there hands it to this DLL's
//! GalaUiaServe, which claims the answer, asks the provider on the
//! window's thread and writes the value in the wire form below. Each read
//! asks afresh (a WM_GETOBJECT, then one claim).

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::element::{self, Node};
use crate::{
    E_NOINTERFACE, E_POINTER, GetCurrentProcessId, GetWindowThreadProcessId, Guid, HResult, Hwnd,
    IID_IUNKNOWN, S_OK, SendMessageW, VT_BOOL, VT_BSTR, VT_EMPTY, VT_I4, VT_R8, VTable, Variant,
    alloc, free, guid, iid_at, put,
};

const WM_GETOBJECT: u32 = 0x003d;
/// gala's message to another process's window (user32's uia.rs).
const GALA_UIA: u32 = 0x03f3;
/// The value buffer GALA_UIA fills (user32's UIA_VALUE_SIZE agrees).
const UIA_VALUE_SIZE: usize = 4096;
/// UiaRootObjectId: the object id UI Automation asks windows for.
pub const UIA_ROOT_OBJECT_ID: i32 = -25;

/// IID_IRawElementProviderSimple {d6dd68d1-86fd-4332-8666-9abedea2d24c}.
pub const IID_IRAW_ELEMENT_PROVIDER_SIMPLE: Guid =
    guid(0xd6dd68d1, 0x86fd, 0x4332, [0x86, 0x66, 0x9a, 0xbe, 0xde, 0xa2, 0xd2, 0x4c]);

// ---- COM calls on a program's provider ----

fn method(p: *mut c_void, i: usize) -> *const () {
    // SAFETY: a COM object: its first word points at its vtable.
    unsafe { *(*(p as *const *const *const ())).add(i) }
}

fn add_ref(p: *mut c_void) {
    // SAFETY: IUnknown::AddRef.
    let f: extern "system" fn(*mut c_void) -> u32 = unsafe { core::mem::transmute(method(p, 1)) };
    f(p);
}

fn release(p: *mut c_void) {
    // SAFETY: IUnknown::Release.
    let f: extern "system" fn(*mut c_void) -> u32 = unsafe { core::mem::transmute(method(p, 2)) };
    f(p);
}

/// IRawElementProviderSimple::GetPropertyValue (slot 5): the provider's
/// value, VT_EMPTY when it has none or fails.
pub fn property(p: *mut c_void, id: i32) -> Variant {
    let mut v = Variant::EMPTY;
    // SAFETY: IRawElementProviderSimple::GetPropertyValue.
    let f: extern "system" fn(*mut c_void, i32, *mut Variant) -> HResult =
        unsafe { core::mem::transmute(method(p, 5)) };
    if f(p, id, &mut v) != S_OK {
        // A failed call leaves nothing we own (the provider cleared it or
        // never wrote it).
        return Variant::EMPTY;
    }
    v
}

// ---- The table of answers ----

const SLOTS: usize = 32;

#[derive(Clone, Copy)]
struct Answer {
    hwnd: Hwnd,
    provider: *mut c_void,
    cookie: u32,
}

struct Table {
    lock: AtomicBool,
    slots: core::cell::UnsafeCell<[Option<Answer>; SLOTS]>,
}

// SAFETY: every access is under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table =
    Table { lock: AtomicBool::new(false), slots: core::cell::UnsafeCell::new([None; SLOTS]) };
/// How many cookies were handed out (cookies are 1..=0xffff, never 0:
/// 0 is "no answer").
static COOKIES: AtomicU32 = AtomicU32::new(0);

fn with<R>(f: impl FnOnce(&mut [Option<Answer>; SLOTS]) -> R) -> R {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *TABLE.slots.get() });
    TABLE.lock.store(false, Ordering::Release);
    r
}

/// Keeps a window's answer (one reference to `provider`) and returns its
/// cookie, the WM_GETOBJECT result.
fn keep(hwnd: Hwnd, provider: *mut c_void) -> isize {
    add_ref(provider);
    // 16 bits: the cookie travels in GALA_UIA's wParam with the property
    // id (32 bits in a 32-bit process).
    let cookie = COOKIES.fetch_add(1, Ordering::Relaxed) % 0xffff + 1;
    let evicted = with(|slots| {
        let i = (cookie as usize) % SLOTS;
        let old = slots[i].take();
        slots[i] = Some(Answer { hwnd, provider, cookie });
        old
    });
    if let Some(old) = evicted {
        // An answer nobody claimed.
        release(old.provider);
    }
    cookie as isize
}

/// Takes the answer `cookie` from `hwnd` out of the table (its reference
/// becomes the caller's).
fn claim(hwnd: Hwnd, cookie: isize) -> Option<*mut c_void> {
    let cookie = u32::try_from(cookie).ok()?;
    with(|slots| {
        let i = (cookie as usize) % SLOTS;
        match slots[i] {
            Some(a) if a.cookie == cookie && a.hwnd == hwnd => {
                slots[i] = None;
                Some(a.provider)
            }
            _ => None,
        }
    })
}

/// A window's provider's value of property `id`: None when the window
/// has no provider (or it gives VT_EMPTY). The window is asked with
/// WM_GETOBJECT; its answer is claimed here (this process's window) or
/// through GALA_UIA (another's).
pub fn provided(hwnd: Hwnd, id: i32) -> Option<Variant> {
    let mut pid = 0;
    // SAFETY: our output.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == 0 {
        return None;
    }
    // SAFETY: no pointers in the message.
    let r = unsafe { SendMessageW(hwnd, WM_GETOBJECT, 0, UIA_ROOT_OBJECT_ID as isize) };
    if r == 0 {
        return None;
    }
    // SAFETY: a plain call.
    let v = if pid == unsafe { GetCurrentProcessId() } {
        let Some(p) = claim(hwnd, r) else {
            // An answer that is not one of ours (an MSAA object's
            // LRESULT, or one already claimed).
            crate::missing(b"UIAutomationCore.dll!WM_GETOBJECT(answer not ours)", r as u64);
            return None;
        };
        let v = property(p, id);
        release(p);
        v
    } else {
        remote(hwnd, r, id)?
    };
    (v.vt != VT_EMPTY).then_some(v)
}

/// Another process's provider's value, through GALA_UIA.
fn remote(hwnd: Hwnd, cookie: isize, id: i32) -> Option<Variant> {
    if !(1..=0xffff).contains(&cookie) || !(0..=0xffff).contains(&id) {
        crate::missing(b"UIAutomationCore.dll!WM_GETOBJECT(answer not ours)", cookie as u64);
        return None;
    }
    let buf = alloc(UIA_VALUE_SIZE);
    if buf.is_null() {
        return None;
    }
    let wparam = id as usize | (cookie as usize) << 16;
    // SAFETY: our buffer of UIA_VALUE_SIZE bytes (user32 marshals it back).
    let served = unsafe { SendMessageW(hwnd, GALA_UIA, wparam, buf as isize) };
    let v = if served == 1 {
        // SAFETY: the buffer, filled by the window's process.
        Some(decode(unsafe { core::slice::from_raw_parts(buf, UIA_VALUE_SIZE) }))
    } else {
        crate::missing(b"UIAutomationCore.dll!GALA_UIA(not served)", served as u64);
        None
    };
    free(buf);
    v
}

// ---- The wire form of a value (GALA_UIA's buffer) ----
//
// vt (u16), 2 bytes reserved, a length (u32: a BSTR's characters), the
// value (u64: VT_I4's and VT_BOOL's bits, VT_R8's), then a BSTR's
// characters from byte 16. Other types (arrays, objects) are not carried
// (VT_EMPTY, recorded where the provider gave them).

const WIRE_HEAD: usize = 16;
const WIRE_CHARS: usize = (UIA_VALUE_SIZE - WIRE_HEAD) / 2;

fn encode(v: &Variant, out: &mut [u8]) {
    let mut vt = v.vt;
    let mut len = 0usize;
    match v.vt {
        VT_I4 | VT_BOOL | VT_R8 | VT_EMPTY => {}
        VT_BSTR => {
            let chars = crate::bstr_chars(v.as_bstr());
            if chars.len() > WIRE_CHARS {
                crate::missing(b"UIAutomationCore.dll!GALA_UIA(string cut)", chars.len() as u64);
            }
            len = chars.len().min(WIRE_CHARS);
            for (i, c) in chars[..len].iter().enumerate() {
                out[WIRE_HEAD + 2 * i..WIRE_HEAD + 2 * i + 2].copy_from_slice(&c.to_le_bytes());
            }
        }
        other => {
            crate::missing(b"UIAutomationCore.dll!GALA_UIA(type not carried)", u64::from(other));
            vt = VT_EMPTY;
        }
    }
    out[0..2].copy_from_slice(&vt.to_le_bytes());
    out[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    out[8..16].copy_from_slice(&v.data[0].to_le_bytes());
}

fn decode(b: &[u8]) -> Variant {
    let vt = u16::from_le_bytes([b[0], b[1]]);
    let len = (u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize).min(WIRE_CHARS);
    let mut value = [0u8; 8];
    value.copy_from_slice(&b[8..16]);
    let value = u64::from_le_bytes(value);
    match vt {
        VT_I4 | VT_BOOL | VT_R8 => Variant { vt, reserved: [0; 3], data: [value, 0] },
        VT_BSTR => {
            let mut chars = crate::List::<u16>::new();
            for i in 0..len {
                let at = WIRE_HEAD + 2 * i;
                if !chars.push(u16::from_le_bytes([b[at], b[at + 1]])) {
                    return Variant::EMPTY;
                }
            }
            Variant::bstr(crate::bstr(chars.as_slice()))
        }
        _ => Variant::EMPTY,
    }
}

/// GalaUiaServe (user32 calls it for GALA_UIA, on our window's thread):
/// claims the answer `wparam >> 16` our window gave, writes its
/// provider's value of property `wparam & 0xffff` into the
/// UIA_VALUE_SIZE buffer at `lparam`; 1, or 0 when there is no such
/// answer.
#[unsafe(no_mangle)]
pub extern "system" fn GalaUiaServe(hwnd: Hwnd, wparam: usize, lparam: isize) -> isize {
    let (id, cookie) = ((wparam & 0xffff) as i32, ((wparam >> 16) & 0xffff) as isize);
    if lparam == 0 {
        return 0;
    }
    let Some(p) = claim(hwnd, cookie) else {
        crate::missing(b"UIAutomationCore.dll!GalaUiaServe(no such answer)", cookie as u64);
        return 0;
    };
    let mut v = property(p, id);
    release(p);
    // SAFETY: user32's buffer of UIA_VALUE_SIZE bytes.
    let out = unsafe { core::slice::from_raw_parts_mut(lparam as *mut u8, UIA_VALUE_SIZE) };
    encode(&v, out);
    v.clear();
    1
}

/// UiaReturnRawElementProvider: the window's answer to WM_GETOBJECT.
/// UiaRootObjectId with a provider gives a cookie the asker claims; any
/// other object id is not UI Automation's (0).
pub fn answer(hwnd: Hwnd, lparam: isize, provider: *mut c_void) -> isize {
    if provider.is_null() || lparam as i32 != UIA_ROOT_OBJECT_ID {
        return 0;
    }
    keep(hwnd, provider)
}

// ---- The host provider (UiaHostProviderFromHwnd) ----

/// An IRawElementProviderSimple whose properties are the window's (what
/// the client reads from the HWND alone).
#[repr(C)]
struct Host {
    vtable: *const VTable<7>,
    refs: AtomicU32,
    hwnd: Hwnd,
}

extern "system" fn host_query(this: *mut Host, iid: *const Guid, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    match iid_at(iid) {
        IID_IUNKNOWN | IID_IRAW_ELEMENT_PROVIDER_SIMPLE => {
            host_add_ref(this);
            put(out, this.cast());
            S_OK
        }
        _ => {
            put(out, core::ptr::null_mut());
            E_NOINTERFACE
        }
    }
}

extern "system" fn host_add_ref(this: *mut Host) -> u32 {
    // SAFETY: a live host object.
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

extern "system" fn host_release(this: *mut Host) -> u32 {
    // SAFETY: a live host object.
    let left = unsafe { (*this).refs.fetch_sub(1, Ordering::AcqRel) } - 1;
    if left == 0 {
        free(this.cast());
    }
    left
}

/// ProviderOptions_ClientSideProvider: the host is the client's own view
/// of the window.
extern "system" fn host_options(_this: *mut Host, out: *mut i32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 1);
    S_OK
}

/// No pattern objects from the host (the client's own elements carry the
/// window's patterns).
extern "system" fn host_pattern(_this: *mut Host, _id: i32, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    S_OK
}

extern "system" fn host_property(this: *mut Host, id: i32, out: *mut Variant) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: a live host object.
    let hwnd = unsafe { (*this).hwnd };
    put(out, element::window_property(Node::window(hwnd), id));
    S_OK
}

/// A host provider has no host of its own.
extern "system" fn host_host(_this: *mut Host, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    S_OK
}

static HOST_VTABLE: VTable<7> = VTable([
    host_query as *const (),
    host_add_ref as *const (),
    host_release as *const (),
    host_options as *const (),
    host_pattern as *const (),
    host_property as *const (),
    host_host as *const (),
]);

/// UiaHostProviderFromHwnd: a new host provider for `hwnd`.
pub fn host(hwnd: Hwnd, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    // SAFETY: a plain call.
    if unsafe { crate::IsWindow(hwnd) } == 0 {
        return crate::E_INVALIDARG;
    }
    let p = alloc(size_of::<Host>()).cast::<Host>();
    if p.is_null() {
        return crate::E_OUTOFMEMORY;
    }
    // SAFETY: fresh memory for one Host.
    unsafe { p.write(Host { vtable: &HOST_VTABLE, refs: AtomicU32::new(1), hwnd }) };
    put(out, p.cast());
    S_OK
}
