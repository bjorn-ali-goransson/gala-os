//! Providers inside programs (UIAutomationCoreApi.h): a window answers
//! WM_GETOBJECT with UiaRootObjectId by returning
//! UiaReturnRawElementProvider(its IRawElementProviderSimple), and the
//! element for that window takes its properties from the provider first,
//! then from the window (the host provider: UiaHostProviderFromHwnd).
//!
//! In-process only for now: the answer is a cookie naming a slot of
//! this process's table, which holds one reference to the provider until
//! the asker claims it (`of`); an answer nobody claims is released when
//! its slot is reused. Windows of other processes are not asked (their
//! answers would have to cross processes: the next step).

use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::element::{self, Node};
use crate::{
    E_NOINTERFACE, E_POINTER, GetCurrentProcessId, GetWindowThreadProcessId, Guid, HResult, Hwnd,
    IID_IUNKNOWN, S_OK, SendMessageW, VTable, Variant, alloc, free, guid, iid_at, put,
};

const WM_GETOBJECT: u32 = 0x003d;
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

pub fn release(p: *mut c_void) {
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
/// The last cookie handed out (cookies count from 1, never 0: 0 is "no
/// answer").
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
    let cookie = loop {
        let c = COOKIES.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        if c != 0 {
            break c;
        }
    };
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

/// The provider of a window of this process (one reference, for the
/// caller to release), asked with WM_GETOBJECT; None when the window has
/// none or is another process's.
pub fn of(hwnd: Hwnd) -> Option<*mut c_void> {
    let mut pid = 0;
    // SAFETY: our output.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    // SAFETY: a plain call.
    if pid == 0 || pid != unsafe { GetCurrentProcessId() } {
        return None;
    }
    // SAFETY: no pointers in the message.
    let r = unsafe { SendMessageW(hwnd, WM_GETOBJECT, 0, UIA_ROOT_OBJECT_ID as isize) };
    if r == 0 {
        return None;
    }
    let p = claim(hwnd, r);
    if p.is_none() {
        // An answer that is not one of ours (an MSAA object's LRESULT,
        // or one already claimed).
        crate::missing(b"UIAutomationCore.dll!WM_GETOBJECT(answer not ours)", r as u64);
    }
    p
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
