//! Providers inside programs (UIAutomationCoreApi.h): a window answers
//! WM_GETOBJECT with UiaRootObjectId by returning
//! UiaReturnRawElementProvider(its IRawElementProviderSimple), and the
//! element for that window takes its properties from the provider first,
//! then from the window (the host provider: UiaHostProviderFromHwnd). A
//! provider that is a fragment root (IRawElementProviderFragment and
//! ...FragmentRoot) has fragments: elements under the window, found with
//! Navigate.
//!
//! Everything the client asks of a provider is an operation (`Op`) the
//! provider's process serves (GalaUiaServe): directly when the window is
//! the client's own process's, else through gala's GALA_UIA message
//! (user32's uia.rs hands it to GalaUiaServe in the window's process, on
//! the window's thread). wParam carries the operation: the property id
//! (bits 0-15), the operation (16-23), a navigation direction (24-31)
//! and a cookie or a fragment handle (32-63); lParam a UIA_VALUE_SIZE
//! buffer for a value (marshaled back), else 0.
//!
//! - The window's provider: the WM_GETOBJECT answer is a cookie
//!   (1..=0xffff) naming a slot of the answering process's answer table,
//!   which holds one reference to the provider until it is claimed (by
//!   the operation that uses it); an answer nobody claims is released
//!   when its slot is reused. Each use asks afresh (a WM_GETOBJECT, then
//!   one claim).
//! - Fragments: the provider's process keeps the fragments it handed out
//!   in a table (one reference each) and names them by a handle: the
//!   slot (10 bits) and a generation (22 bits), so a handle whose slot
//!   was reused names nothing. A fragment already in the table (the same
//!   object identity, IUnknown) keeps its handle, so elements for the
//!   same fragment are equal (CompareElements).

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::element::{self, Node};
use crate::{
    E_NOINTERFACE, E_POINTER, GetCurrentProcessId, GetWindowThreadProcessId, Guid, HResult, Hwnd,
    IID_IUNKNOWN, S_OK, SafeArrayAccessData, SafeArrayGetLBound, SafeArrayGetUBound,
    SafeArrayUnaccessData, SendMessageW, VT_ARRAY, VT_BOOL, VT_BSTR, VT_EMPTY, VT_I4, VT_R8,
    VTable, Variant, alloc, free, guid, iid_at, put,
};

const WM_GETOBJECT: u32 = 0x003d;
/// gala's message to another process's window (user32's uia.rs).
const GALA_UIA: u32 = 0x03f3;
/// The value buffer GALA_UIA fills (user32's UIA_VALUE_SIZE agrees).
const UIA_VALUE_SIZE: usize = 4096;
/// UiaRootObjectId: the object id UI Automation asks windows for.
pub const UIA_ROOT_OBJECT_ID: i32 = -25;

const RUNTIME_ID: i32 = 30000;
const BOUNDING_RECTANGLE: i32 = 30001;
/// UiaAppendRuntimeId: a fragment's runtime id goes after its host's.
const APPEND_RUNTIME_ID: i32 = 3;
/// The HWND elements' runtime-id prefix (element.rs agrees).
const RUNTIME_ID_HWND: i32 = 42;

/// IID_IRawElementProviderSimple {d6dd68d1-86fd-4332-8666-9abedea2d24c}.
pub const IID_IRAW_ELEMENT_PROVIDER_SIMPLE: Guid =
    guid(0xd6dd68d1, 0x86fd, 0x4332, [0x86, 0x66, 0x9a, 0xbe, 0xde, 0xa2, 0xd2, 0x4c]);
/// IID_IRawElementProviderFragment {f7063da8-8359-439c-9297-bbc5299a7d87}.
const IID_IRAW_ELEMENT_PROVIDER_FRAGMENT: Guid =
    guid(0xf7063da8, 0x8359, 0x439c, [0x92, 0x97, 0xbb, 0xc5, 0x29, 0x9a, 0x7d, 0x87]);
/// IID_IRawElementProviderFragmentRoot {620ce2a5-ab8f-40a9-86cb-de3c75599b58}.
const IID_IRAW_ELEMENT_PROVIDER_FRAGMENT_ROOT: Guid =
    guid(0x620ce2a5, 0xab8f, 0x40a9, [0x86, 0xcb, 0xde, 0x3c, 0x75, 0x59, 0x9b, 0x58]);

/// NavigateDirection (the ones the tree uses: siblings are walked
/// forward from the first child).
pub const PARENT: u8 = 0;
pub const NEXT_SIBLING: u8 = 1;
pub const FIRST_CHILD: u8 = 3;

/// The operations GalaUiaServe serves.
#[derive(Clone, Copy)]
#[repr(u8)]
enum Op {
    /// The window's provider's value of a property (cookie; a value).
    RootProperty = 0,
    /// The root's first or last fragment (cookie, direction; a handle).
    RootChild = 1,
    /// A fragment's value of a property (handle; a value).
    Property = 2,
    /// The fragment in a direction from a fragment (handle, direction; a
    /// handle, -1 for the root, 0 for none).
    Navigate = 3,
    /// Whether a handle still names a fragment (1) or not (0).
    Alive = 4,
    /// Whether a fragment has a pattern (handle, id the pattern; 1 or 0).
    HasPattern = 5,
    /// The Invoke pattern's Invoke (handle; 1, or the failure HRESULT).
    Invoke = 6,
    /// The Value pattern's SetValue (handle; the text, NUL-terminated
    /// UTF-16, in the buffer, which goes in: GALA_UIA_INPUT; as Invoke).
    SetValue = 7,
}

/// wParam's flag (the direction byte's top bit) for an operation whose
/// buffer goes in, not out (user32's uia.rs agrees).
const GALA_UIA_INPUT: u64 = 1 << 31;

pub const INVOKE: i32 = 10000;
pub const VALUE: i32 = 10002;
const VALUE_VALUE: i32 = 30045;
const VALUE_IS_READ_ONLY: i32 = 30046;
/// IID_IInvokeProvider {54fcb24b-e18e-47a2-b4d3-eccbe77599a2}.
const IID_IINVOKE_PROVIDER: Guid =
    guid(0x54fcb24b, 0xe18e, 0x47a2, [0xb4, 0xd3, 0xec, 0xcb, 0xe7, 0x75, 0x99, 0xa2]);
/// IID_IValueProvider {c7935180-6fb3-4201-b174-7df73adbf64a}.
const IID_IVALUE_PROVIDER: Guid =
    guid(0xc7935180, 0x6fb3, 0x4201, [0xb1, 0x74, 0x7d, 0xf7, 0x3a, 0xdb, 0xf6, 0x4a]);

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

/// QueryInterface: the interface (one reference) or null.
fn query(p: *mut c_void, iid: &Guid) -> *mut c_void {
    // SAFETY: IUnknown::QueryInterface.
    let f: extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult =
        unsafe { core::mem::transmute(method(p, 0)) };
    let mut out = core::ptr::null_mut();
    if f(p, iid, &mut out) != S_OK {
        return core::ptr::null_mut();
    }
    out
}

/// The object's identity: its IUnknown's address (0 when it has none).
fn identity(p: *mut c_void) -> usize {
    let u = query(p, &IID_IUNKNOWN);
    if u.is_null() {
        return 0;
    }
    release(u);
    u as usize
}

/// IRawElementProviderSimple::GetPropertyValue (slot 5): the provider's
/// value, VT_EMPTY when it has none or fails.
fn property(p: *mut c_void, id: i32) -> Variant {
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

/// IRawElementProviderFragment::Navigate (slot 3): the fragment in
/// direction `d` (one reference), or null.
fn navigate(f: *mut c_void, d: u8) -> *mut c_void {
    let mut out = core::ptr::null_mut();
    // SAFETY: IRawElementProviderFragment::Navigate.
    let call: extern "system" fn(*mut c_void, i32, *mut *mut c_void) -> HResult =
        unsafe { core::mem::transmute(method(f, 3)) };
    if call(f, i32::from(d), &mut out) != S_OK {
        return core::ptr::null_mut();
    }
    out
}

/// A fragment's RuntimeId (GetRuntimeId, slot 4): after its host's
/// ({42, window}) when it starts with UiaAppendRuntimeId; {42, window,
/// handle} when it gives none.
fn runtime_id(f: *mut c_void, hwnd: Hwnd, handle: u32) -> Variant {
    let mut sa: *mut c_void = core::ptr::null_mut();
    // SAFETY: IRawElementProviderFragment::GetRuntimeId.
    let call: extern "system" fn(*mut c_void, *mut *mut c_void) -> HResult =
        unsafe { core::mem::transmute(method(f, 4)) };
    let mut ids = crate::List::<i32>::new();
    ids.push(RUNTIME_ID_HWND);
    ids.push(hwnd as i32);
    if call(f, &mut sa) == S_OK && !sa.is_null() {
        let given = i4s(sa);
        let given = given.as_slice();
        // SAFETY: the provider's SAFEARRAY, ours now.
        unsafe { crate::SafeArrayDestroy(sa) };
        if given.first() == Some(&APPEND_RUNTIME_ID) {
            for &i in &given[1..] {
                ids.push(i);
            }
            return element::array(VT_I4, ids.as_slice());
        }
        if !given.is_empty() {
            return element::array(VT_I4, given);
        }
    }
    ids.push(handle as i32);
    element::array(VT_I4, ids.as_slice())
}

/// The elements of a one-dimensional SAFEARRAY of 4-byte values.
fn i4s(sa: *mut c_void) -> crate::List<i32> {
    elements::<i32>(sa)
}

fn elements<T: Copy>(sa: *mut c_void) -> crate::List<T> {
    let mut out = crate::List::new();
    let (mut lo, mut hi) = (0, -1);
    let mut data: *mut c_void = core::ptr::null_mut();
    // SAFETY: a SAFEARRAY of T (the caller's type check); bounds read
    // before the data is walked.
    unsafe {
        if SafeArrayGetLBound(sa, 1, &mut lo) != S_OK
            || SafeArrayGetUBound(sa, 1, &mut hi) != S_OK
            || SafeArrayAccessData(sa, &mut data) != S_OK
        {
            return out;
        }
        for i in 0..(hi - lo + 1).max(0) as usize {
            if !out.push(data.cast::<T>().add(i).read_unaligned()) {
                break;
            }
        }
        SafeArrayUnaccessData(sa);
    }
    out
}

/// The doubles of a VT_ARRAY | VT_R8 value (none for any other).
pub fn doubles(v: &Variant) -> crate::List<f64> {
    if v.vt != VT_ARRAY | VT_R8 {
        return crate::List::new();
    }
    elements::<f64>(v.data[0] as usize as *mut c_void)
}

/// The fragment's pattern object for `pattern` as interface `iid` (one
/// reference), or null (GetPatternProvider, slot 4 of its
/// IRawElementProviderSimple).
fn pattern_provider(f: *mut c_void, pattern: i32, iid: &Guid) -> *mut c_void {
    let s = query(f, &IID_IRAW_ELEMENT_PROVIDER_SIMPLE);
    if s.is_null() {
        return core::ptr::null_mut();
    }
    let mut unk: *mut c_void = core::ptr::null_mut();
    // SAFETY: IRawElementProviderSimple::GetPatternProvider.
    let call: extern "system" fn(*mut c_void, i32, *mut *mut c_void) -> HResult =
        unsafe { core::mem::transmute(method(s, 4)) };
    let hr = call(s, pattern, &mut unk);
    release(s);
    if hr != S_OK || unk.is_null() {
        return core::ptr::null_mut();
    }
    let p = query(unk, iid);
    release(unk);
    p
}

/// The interface the client's pattern objects use for `pattern` (None:
/// a pattern gala does not carry for providers yet).
fn pattern_iid(pattern: i32) -> Option<Guid> {
    match pattern {
        INVOKE => Some(IID_IINVOKE_PROVIDER),
        VALUE => Some(IID_IVALUE_PROVIDER),
        _ => None,
    }
}

/// A Value pattern property from the fragment's pattern object (VT_EMPTY
/// without the pattern).
fn value_property(f: *mut c_void, id: i32) -> Variant {
    let v = pattern_provider(f, VALUE, &IID_IVALUE_PROVIDER);
    if v.is_null() {
        return Variant::EMPTY;
    }
    let r = if id == VALUE_VALUE {
        let mut s: *mut u16 = core::ptr::null_mut();
        // SAFETY: IValueProvider::get_Value (slot 4).
        let call: extern "system" fn(*mut c_void, *mut *mut u16) -> HResult =
            unsafe { core::mem::transmute(method(v, 4)) };
        if call(v, &mut s) == S_OK { Variant::bstr(s) } else { Variant::EMPTY }
    } else {
        let mut b = 0i32;
        // SAFETY: IValueProvider::get_IsReadOnly (slot 5).
        let call: extern "system" fn(*mut c_void, *mut i32) -> HResult =
            unsafe { core::mem::transmute(method(v, 5)) };
        if call(v, &mut b) == S_OK { Variant::boolean(b != 0) } else { Variant::EMPTY }
    };
    release(v);
    r
}

/// An HRESULT as an operation's result: 1 for S_OK (0 means "not
/// served"), else the failure.
fn op_result(hr: HResult) -> isize {
    if hr == S_OK { 1 } else { hr as isize }
}

// ---- The table of answers ----

const SLOTS: usize = 32;

#[derive(Clone, Copy)]
struct Answer {
    hwnd: Hwnd,
    provider: *mut c_void,
    cookie: u32,
}

struct Table<T, const N: usize> {
    lock: AtomicBool,
    slots: core::cell::UnsafeCell<[Option<T>; N]>,
}

// SAFETY: every access is under `lock`.
unsafe impl<T, const N: usize> Sync for Table<T, N> {}

impl<T, const N: usize> Table<T, N> {
    fn with<R>(&self, f: impl FnOnce(&mut [Option<T>; N]) -> R) -> R {
        while self.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err()
        {
            core::hint::spin_loop();
        }
        // SAFETY: under the lock.
        let r = f(unsafe { &mut *self.slots.get() });
        self.lock.store(false, Ordering::Release);
        r
    }
}

static ANSWERS: Table<Answer, SLOTS> =
    Table { lock: AtomicBool::new(false), slots: core::cell::UnsafeCell::new([None; SLOTS]) };
/// How many cookies were handed out (cookies are 1..=0xffff, never 0:
/// 0 is "no answer").
static COOKIES: AtomicU32 = AtomicU32::new(0);

/// Keeps a window's answer (one reference to `provider`) and returns its
/// cookie, the WM_GETOBJECT result.
fn keep(hwnd: Hwnd, provider: *mut c_void) -> isize {
    add_ref(provider);
    let cookie = COOKIES.fetch_add(1, Ordering::Relaxed) % 0xffff + 1;
    let evicted = ANSWERS.with(|slots| {
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
fn claim(hwnd: Hwnd, cookie: u32) -> Option<*mut c_void> {
    ANSWERS.with(|slots| {
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

// ---- The table of fragments ----

const FRAGMENTS: usize = 1024;
const SLOT_BITS: u32 = 10;

#[derive(Clone, Copy)]
struct Fragment {
    hwnd: Hwnd,
    /// IRawElementProviderFragment (one reference).
    ptr: *mut c_void,
    identity: usize,
    handle: u32,
}

static FRAGS: Table<Fragment, FRAGMENTS> =
    Table { lock: AtomicBool::new(false), slots: core::cell::UnsafeCell::new([None; FRAGMENTS]) };
/// The next slot to fill (round robin) and the generations handed out.
static NEXT_SLOT: AtomicU32 = AtomicU32::new(0);
static GENERATIONS: AtomicU32 = AtomicU32::new(0);

/// Keeps fragment `f` (its reference becomes the table's) of window
/// `hwnd`: its handle (the one it has when it is there already).
fn keep_fragment(hwnd: Hwnd, f: *mut c_void) -> u32 {
    let id = identity(f);
    let known = FRAGS.with(|slots| {
        slots
            .iter()
            .flatten()
            .find(|x| x.hwnd == hwnd && id != 0 && x.identity == id)
            .map(|x| x.handle)
    });
    if let Some(handle) = known {
        release(f);
        return handle;
    }
    let slot = NEXT_SLOT.fetch_add(1, Ordering::Relaxed) % FRAGMENTS as u32;
    let generation =
        GENERATIONS.fetch_add(1, Ordering::Relaxed) % ((1 << (32 - SLOT_BITS)) - 1) + 1;
    let handle = generation << SLOT_BITS | slot;
    let evicted = FRAGS.with(|slots| {
        slots[slot as usize].replace(Fragment { hwnd, ptr: f, identity: id, handle })
    });
    if let Some(old) = evicted {
        release(old.ptr);
    }
    handle
}

/// The fragment `handle` names in window `hwnd` (one reference for the
/// caller), or None.
fn fragment(hwnd: Hwnd, handle: u32) -> Option<*mut c_void> {
    FRAGS.with(|slots| {
        let x = slots[(handle & ((1 << SLOT_BITS) - 1)) as usize]?;
        if x.handle != handle || x.hwnd != hwnd {
            return None;
        }
        add_ref(x.ptr);
        Some(x.ptr)
    })
}

/// The handle for the fragment Navigate gave from `from` (taking its
/// reference): -1 for a fragment root (the window's element), 0 for none.
fn handle_of(hwnd: Hwnd, to: *mut c_void) -> isize {
    if to.is_null() {
        return 0;
    }
    let root = query(to, &IID_IRAW_ELEMENT_PROVIDER_FRAGMENT_ROOT);
    if !root.is_null() {
        release(root);
        release(to);
        return -1;
    }
    keep_fragment(hwnd, to) as isize
}

// ---- Serving (in the provider's process) ----

/// GalaUiaServe: one operation (the module's comment) on window `hwnd`'s
/// provider, for this process's client or (through user32's GALA_UIA)
/// another's.
#[unsafe(no_mangle)]
pub extern "system" fn GalaUiaServe(hwnd: Hwnd, wparam: usize, lparam: isize) -> isize {
    let w = wparam as u64;
    let id = (w & 0xffff) as i32;
    let (op, dir, arg) = ((w >> 16) as u8, (w >> 24) as u8 & 0x7f, (w >> 32) as u32);
    let out = if lparam == 0 {
        None
    } else {
        // SAFETY: the caller's buffer of UIA_VALUE_SIZE bytes.
        Some(unsafe { core::slice::from_raw_parts_mut(lparam as *mut u8, UIA_VALUE_SIZE) })
    };
    match op {
        0 | 1 => {
            let Some(p) = claim(hwnd, arg) else {
                crate::missing(
                    b"UIAutomationCore.dll!GalaUiaServe(no such answer)",
                    u64::from(arg),
                );
                return 0;
            };
            let r = if op == 0 {
                match out {
                    Some(out) => {
                        let mut v = property(p, id);
                        encode(&v, out);
                        v.clear();
                        1
                    }
                    None => 0,
                }
            } else {
                let f = query(p, &IID_IRAW_ELEMENT_PROVIDER_FRAGMENT);
                if f.is_null() {
                    0
                } else {
                    let to = navigate(f, dir);
                    release(f);
                    handle_of(hwnd, to).max(0)
                }
            };
            release(p);
            r
        }
        2 => {
            let (Some(f), Some(out)) = (fragment(hwnd, arg), out) else { return 0 };
            let mut v = match id {
                RUNTIME_ID => runtime_id(f, hwnd, arg),
                BOUNDING_RECTANGLE => bounding_rectangle(f),
                VALUE_VALUE | VALUE_IS_READ_ONLY => value_property(f, id),
                _ => {
                    let s = query(f, &IID_IRAW_ELEMENT_PROVIDER_SIMPLE);
                    if s.is_null() {
                        Variant::EMPTY
                    } else {
                        let v = property(s, id);
                        release(s);
                        v
                    }
                }
            };
            release(f);
            encode(&v, out);
            v.clear();
            1
        }
        3 => {
            let Some(f) = fragment(hwnd, arg) else { return 0 };
            let to = navigate(f, dir);
            release(f);
            handle_of(hwnd, to)
        }
        4 => isize::from(fragment(hwnd, arg).map(release).is_some()),
        5 => {
            let Some(f) = fragment(hwnd, arg) else { return 0 };
            let Some(iid) = pattern_iid(id) else {
                release(f);
                return 0;
            };
            let p = pattern_provider(f, id, &iid);
            release(f);
            if p.is_null() {
                return 0;
            }
            release(p);
            1
        }
        6 | 7 => {
            let Some(f) = fragment(hwnd, arg) else { return 0 };
            let (pattern, iid) =
                if op == 6 { (INVOKE, IID_IINVOKE_PROVIDER) } else { (VALUE, IID_IVALUE_PROVIDER) };
            let p = pattern_provider(f, pattern, &iid);
            release(f);
            if p.is_null() {
                return op_result(crate::UIA_E_ELEMENTNOTAVAILABLE);
            }
            let empty = [0u16];
            let hr = if op == 6 {
                // SAFETY: IInvokeProvider::Invoke (slot 3).
                let call: extern "system" fn(*mut c_void) -> HResult =
                    unsafe { core::mem::transmute(method(p, 3)) };
                call(p)
            } else {
                // The text: NUL-terminated in the buffer (its last
                // character forced to NUL).
                let text = match out {
                    Some(buf) => {
                        let n = buf.len();
                        buf[n - 2..].fill(0);
                        buf.as_ptr().cast::<u16>()
                    }
                    None => empty.as_ptr(),
                };
                // SAFETY: IValueProvider::SetValue (slot 3) with a
                // NUL-terminated string.
                let call: extern "system" fn(*mut c_void, *const u16) -> HResult =
                    unsafe { core::mem::transmute(method(p, 3)) };
                call(p, text)
            };
            release(p);
            op_result(hr)
        }
        _ => {
            crate::missing(b"UIAutomationCore.dll!GalaUiaServe(operation)", u64::from(op));
            0
        }
    }
}

/// A fragment's BoundingRectangle (get_BoundingRectangle, slot 5: a
/// UiaRect of doubles: left, top, width, height).
fn bounding_rectangle(f: *mut c_void) -> Variant {
    let mut r = [0f64; 4];
    // SAFETY: IRawElementProviderFragment::get_BoundingRectangle.
    let call: extern "system" fn(*mut c_void, *mut [f64; 4]) -> HResult =
        unsafe { core::mem::transmute(method(f, 5)) };
    if call(f, &mut r) != S_OK {
        return Variant::EMPTY;
    }
    element::array(VT_R8, &r)
}

// ---- Asking (in the client's process) ----

/// Operation `op` on window `hwnd`'s provider: served here for this
/// process's window, else through GALA_UIA. `buf` is a UIA_VALUE_SIZE
/// buffer for a value, or null.
fn ask(hwnd: Hwnd, op: Op, dir: u8, arg: u32, id: i32, buf: *mut u8) -> isize {
    let input = if matches!(op, Op::SetValue) { GALA_UIA_INPUT } else { 0 };
    let wparam = (id as u16 as u64
        | (op as u64) << 16
        | u64::from(dir) << 24
        | input
        | u64::from(arg) << 32) as usize;
    let mut pid = 0;
    // SAFETY: our output.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    // SAFETY: a plain call.
    if pid == unsafe { GetCurrentProcessId() } {
        return GalaUiaServe(hwnd, wparam, buf as isize);
    }
    // SAFETY: our buffer of UIA_VALUE_SIZE bytes or none (user32
    // marshals it back).
    unsafe { SendMessageW(hwnd, GALA_UIA, wparam, buf as isize) }
}

/// The window's answer to WM_GETOBJECT: a cookie of ours, or None (no
/// provider; an answer that is not one of ours is recorded).
fn cookie(hwnd: Hwnd) -> Option<u32> {
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
    if !(1..=0xffff).contains(&r) {
        // An MSAA object's LRESULT, or another library's.
        crate::missing(b"UIAutomationCore.dll!WM_GETOBJECT(answer not ours)", r as u64);
        return None;
    }
    Some(r as u32)
}

/// A value asked with `op`: None when there is none (VT_EMPTY, or the
/// operation was not served: recorded).
fn value(hwnd: Hwnd, op: Op, arg: u32, id: i32) -> Option<Variant> {
    let buf = alloc(UIA_VALUE_SIZE);
    if buf.is_null() {
        return None;
    }
    let served = ask(hwnd, op, 0, arg, id, buf);
    let v = if served == 1 {
        // SAFETY: the buffer, filled by the serving side.
        Some(decode(unsafe { core::slice::from_raw_parts(buf, UIA_VALUE_SIZE) }))
    } else {
        crate::missing(b"UIAutomationCore.dll!GALA_UIA(not served)", op as u64);
        None
    };
    free(buf);
    v.filter(|v| v.vt != VT_EMPTY)
}

/// A window's provider's value of property `id`: None when the window
/// has no provider, or it gives VT_EMPTY.
pub fn provided(hwnd: Hwnd, id: i32) -> Option<Variant> {
    value(hwnd, Op::RootProperty, cookie(hwnd)?, id)
}

/// A fragment's value of property `id` (None: none given).
pub fn fragment_property(hwnd: Hwnd, handle: u32, id: i32) -> Option<Variant> {
    value(hwnd, Op::Property, handle, id)
}

/// Window `hwnd`'s first or last fragment (FIRST_CHILD, LAST_CHILD).
pub fn root_child(hwnd: Hwnd, dir: u8) -> Option<u32> {
    let c = cookie(hwnd)?;
    match ask(hwnd, Op::RootChild, dir, c, 0, core::ptr::null_mut()) {
        h if h > 0 => Some(h as u32),
        _ => None,
    }
}

/// Where Navigate goes from a fragment.
pub enum Toward {
    Nothing,
    /// The fragment root: the window's element.
    Window,
    Fragment(u32),
}

pub fn navigate_from(hwnd: Hwnd, handle: u32, dir: u8) -> Toward {
    match ask(hwnd, Op::Navigate, dir, handle, 0, core::ptr::null_mut()) {
        -1 => Toward::Window,
        h if h > 0 => Toward::Fragment(h as u32),
        _ => Toward::Nothing,
    }
}

/// Whether fragment `handle` has pattern `pattern` (one gala carries for
/// providers: Invoke, Value).
pub fn has_pattern(hwnd: Hwnd, handle: u32, pattern: i32) -> bool {
    pattern_iid(pattern).is_some()
        && ask(hwnd, Op::HasPattern, 0, handle, pattern, core::ptr::null_mut()) == 1
}

/// An operation's result as an HRESULT (not served: the element is
/// gone).
fn hresult(r: isize) -> HResult {
    match r {
        1 => S_OK,
        0 => crate::UIA_E_ELEMENTNOTAVAILABLE,
        e => e as HResult,
    }
}

/// The fragment's Invoke pattern's Invoke.
pub fn invoke(hwnd: Hwnd, handle: u32) -> HResult {
    hresult(ask(hwnd, Op::Invoke, 0, handle, 0, core::ptr::null_mut()))
}

/// The fragment's Value pattern's SetValue (`text` NUL-terminated, null
/// for empty; cut at the buffer's 2047 characters, recorded).
pub fn set_value(hwnd: Hwnd, handle: u32, text: *const u16) -> HResult {
    let buf = alloc(UIA_VALUE_SIZE);
    if buf.is_null() {
        return crate::E_OUTOFMEMORY;
    }
    let chars = UIA_VALUE_SIZE / 2;
    // SAFETY: our buffer of UIA_VALUE_SIZE bytes; the caller's
    // NUL-terminated string, read up to its NUL or the buffer's end.
    unsafe {
        let dst = buf.cast::<u16>();
        let mut n = 0;
        while !text.is_null() && n < chars - 1 && *text.add(n) != 0 {
            dst.add(n).write(*text.add(n));
            n += 1;
        }
        if n == chars - 1 && *text.add(n) != 0 {
            crate::missing(b"UIAutomationCore.dll!SetValue(text cut)", n as u64);
        }
        dst.add(n).write(0);
    }
    let r = hresult(ask(hwnd, Op::SetValue, 0, handle, 0, buf));
    free(buf);
    r
}

/// Whether `handle` still names a fragment of window `hwnd`.
pub fn alive(hwnd: Hwnd, handle: u32) -> bool {
    ask(hwnd, Op::Alive, 0, handle, 0, core::ptr::null_mut()) == 1
}

// ---- The wire form of a value (GALA_UIA's buffer) ----
//
// vt (u16), 2 bytes reserved, a count (u32: a BSTR's characters, an
// array's elements), the value (u64: VT_I4's and VT_BOOL's bits, VT_R8's),
// then from byte 16 a BSTR's characters or an array's elements (VT_I4 or
// VT_R8 vectors). Other types (objects, other arrays) are not carried
// (VT_EMPTY, recorded where the provider gave them).

const WIRE_HEAD: usize = 16;
const WIRE_BYTES: usize = UIA_VALUE_SIZE - WIRE_HEAD;

fn encode(v: &Variant, out: &mut [u8]) {
    let mut vt = v.vt;
    let mut count = 0usize;
    let body = &mut out[WIRE_HEAD..];
    match v.vt {
        VT_I4 | VT_BOOL | VT_R8 | VT_EMPTY => {}
        VT_BSTR => {
            let chars = crate::bstr_chars(v.as_bstr());
            if 2 * chars.len() > WIRE_BYTES {
                crate::missing(b"UIAutomationCore.dll!GALA_UIA(string cut)", chars.len() as u64);
            }
            count = chars.len().min(WIRE_BYTES / 2);
            for (i, c) in chars[..count].iter().enumerate() {
                body[2 * i..2 * i + 2].copy_from_slice(&c.to_le_bytes());
            }
        }
        t if t == VT_ARRAY | VT_I4 => {
            let items = elements::<i32>(v.data[0] as usize as *mut c_void);
            count = items.as_slice().len().min(WIRE_BYTES / 4);
            for (i, x) in items.as_slice()[..count].iter().enumerate() {
                body[4 * i..4 * i + 4].copy_from_slice(&x.to_le_bytes());
            }
        }
        t if t == VT_ARRAY | VT_R8 => {
            let items = elements::<f64>(v.data[0] as usize as *mut c_void);
            count = items.as_slice().len().min(WIRE_BYTES / 8);
            for (i, x) in items.as_slice()[..count].iter().enumerate() {
                body[8 * i..8 * i + 8].copy_from_slice(&x.to_le_bytes());
            }
        }
        other => {
            crate::missing(b"UIAutomationCore.dll!GALA_UIA(type not carried)", u64::from(other));
            vt = VT_EMPTY;
        }
    }
    out[0..2].copy_from_slice(&vt.to_le_bytes());
    out[4..8].copy_from_slice(&(count as u32).to_le_bytes());
    let scalar = if matches!(vt, VT_I4 | VT_BOOL | VT_R8) { v.data[0] } else { 0 };
    out[8..16].copy_from_slice(&scalar.to_le_bytes());
}

fn decode(b: &[u8]) -> Variant {
    let vt = u16::from_le_bytes([b[0], b[1]]);
    let count = u32::from_le_bytes([b[4], b[5], b[6], b[7]]) as usize;
    let mut value = [0u8; 8];
    value.copy_from_slice(&b[8..16]);
    let value = u64::from_le_bytes(value);
    let body = &b[WIRE_HEAD..];
    let word = |at: usize, n: usize| {
        let mut w = [0u8; 8];
        w[..n].copy_from_slice(&body[at..at + n]);
        u64::from_le_bytes(w)
    };
    match vt {
        VT_I4 | VT_BOOL | VT_R8 => Variant { vt, reserved: [0; 3], data: [value, 0] },
        VT_BSTR => {
            let mut chars = crate::List::<u16>::new();
            for i in 0..count.min(WIRE_BYTES / 2) {
                if !chars.push(word(2 * i, 2) as u16) {
                    return Variant::EMPTY;
                }
            }
            Variant::bstr(crate::bstr(chars.as_slice()))
        }
        t if t == VT_ARRAY | VT_I4 => {
            let mut items = crate::List::<i32>::new();
            for i in 0..count.min(WIRE_BYTES / 4) {
                if !items.push(word(4 * i, 4) as u32 as i32) {
                    return Variant::EMPTY;
                }
            }
            element::array(VT_I4, items.as_slice())
        }
        t if t == VT_ARRAY | VT_R8 => {
            let mut items = crate::List::<f64>::new();
            for i in 0..count.min(WIRE_BYTES / 8) {
                if !items.push(f64::from_bits(word(8 * i, 8))) {
                    return Variant::EMPTY;
                }
            }
            element::array(VT_R8, items.as_slice())
        }
        _ => Variant::EMPTY,
    }
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
