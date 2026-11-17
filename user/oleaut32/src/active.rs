//! The running objects of this process (RegisterActiveObject,
//! RevokeActiveObject, GetActiveObject), as Office's wwlib uses them: it
//! publishes its Application object at startup so that another program
//! can take hold of the running Word.
//!
//! On Windows these go through COM's running object table, which is
//! machine-wide: a moniker in a shared table, reached through
//! IRunningObjectTable, so another *process* finds the object. gala's
//! ole32 has no monikers and no shared table (ROADMAP OF), so this table
//! is process-local: a program finds what it registered itself, and one
//! program does not see another's. ARCHITECTURE "Running objects" says
//! what that limits.
//!
//! What the calls answer is Windows' own, measured on Windows 11:
//! CO_E_NOTINITIALIZED before any apartment exists, E_INVALIDARG for a
//! null object, CLSID, cookie or output and for a cookie that was never
//! given, a non-zero cookie of its own for every registration,
//! MK_S_MONIKERALREADYREGISTERED for a CLSID already registered,
//! MK_E_UNAVAILABLE when nothing holds the CLSID, one reference held
//! from registration until revocation (weak as well as strong), and
//! GetActiveObject AddRefs the first registration still in force.

use super::{E_INVALIDARG, GetModuleHandleW, GetProcAddress, HResult, S_OK, missing, put};
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

const CO_E_NOTINITIALIZED: HResult = 0x8004_01f0u32 as i32;
const MK_E_UNAVAILABLE: HResult = 0x8004_01e3u32 as i32;
const MK_S_MONIKERALREADYREGISTERED: HResult = 0x0004_01e7;

type Guid = [u8; 16];

/// IUnknown's first three entries, which is all this table calls.
#[repr(C)]
struct Unknown {
    query: unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

/// How many objects one process can publish at a time. Windows has no
/// such limit; a program that reaches gala's is reported to the ledger
/// (nothing seen needs more than a handful: Word publishes one).
const SLOTS: usize = 32;

/// A registration: the cookie that revokes it, the class it answers for
/// and the object itself (as an address, so the table is a plain
/// `static`). A free slot has cookie 0.
#[derive(Clone, Copy)]
struct Slot {
    cookie: u32,
    clsid: Guid,
    object: usize,
}

struct Table {
    slots: [Slot; SLOTS],
    /// The sequence number of the next cookie, which Windows keeps in
    /// the cookie's high word (the low word is the slot).
    next: u32,
}

/// The table, under `LOCK`.
struct Shared(UnsafeCell<Table>);

// SAFETY: the table is only ever reached with `LOCK` taken, and the
// objects' own methods are called after it is given back.
unsafe impl Sync for Shared {}

static TABLE: Shared = Shared(UnsafeCell::new(Table {
    slots: [Slot { cookie: 0, clsid: [0; 16], object: 0 }; SLOTS],
    next: 1,
}));
static LOCK: AtomicU32 = AtomicU32::new(0);

/// Runs `with` on the table, holding `LOCK`. Nothing inside calls an
/// object's methods, so the lock is never held across a callback.
fn locked<T>(with: impl FnOnce(&mut Table) -> T) -> T {
    while LOCK.compare_exchange_weak(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: this thread holds the lock, which is the only way in.
    let out = with(unsafe { &mut *TABLE.0.get() });
    LOCK.store(0, Ordering::Release);
    out
}

/// Whether this process has initialized COM: ole32 keeps the
/// apartments, so without it loaded nothing has, and with it loaded its
/// CoGetApartmentType answers for this thread (an implicit MTA counts,
/// as it does on Windows).
fn com_initialized() -> bool {
    let name: [u16; 10] = [b'o', b'l', b'e', b'3', b'2', b'.', b'd', b'l', b'l', 0].map(u16::from);
    // SAFETY: a NUL-terminated name.
    let module = unsafe { GetModuleHandleW(name.as_ptr()) };
    if module == 0 {
        return false;
    }
    // SAFETY: a NUL-terminated name and a module handle.
    let entry = unsafe { GetProcAddress(module, c"CoGetApartmentType".as_ptr().cast()) };
    if entry == 0 {
        return false;
    }
    // SAFETY: ole32's CoGetApartmentType(APTTYPE*, APTTYPEQUALIFIER*).
    let call: extern "system" fn(*mut i32, *mut i32) -> HResult =
        unsafe { core::mem::transmute(entry) };
    let (mut kind, mut qualifier) = (0, 0);
    call(&mut kind, &mut qualifier) == S_OK
}

/// The object's IUnknown vtable.
///
/// # Safety
/// `object` is a COM object whose first field is its vtable pointer.
unsafe fn unknown(object: usize) -> &'static Unknown {
    // SAFETY: the caller's COM object: a pointer to its vtable.
    unsafe { &**(object as *const *const Unknown) }
}

/// RegisterActiveObject(unk, clsid, flags, cookie): publishes `unk` as
/// the running object of `clsid` and holds a reference to it until it is
/// revoked. ACTIVEOBJECT_WEAK (1) is taken as ACTIVEOBJECT_STRONG (0) is:
/// Windows holds a reference for either (measured), and gala has no
/// marshalling to make the difference visible.
#[unsafe(no_mangle)]
pub extern "system" fn RegisterActiveObject(
    unk: *mut c_void,
    clsid: *const Guid,
    _flags: u32,
    cookie: *mut u32,
) -> HResult {
    if !com_initialized() {
        return CO_E_NOTINITIALIZED;
    }
    if unk.is_null() || clsid.is_null() || cookie.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's CLSID.
    let clsid = unsafe { *clsid };
    let taken = locked(|table| {
        let free = table.slots.iter().position(|s| s.cookie == 0)?;
        let already = table.slots.iter().any(|s| s.cookie != 0 && s.clsid == clsid);
        let given = (table.next << 16) | free as u32;
        table.next += 1;
        table.slots[free] = Slot { cookie: given, clsid, object: unk as usize };
        Some((given, already))
    });
    let Some((given, already)) = taken else {
        missing(b"oleaut32.dll!RegisterActiveObject(running objects at once)", SLOTS as u64);
        return E_INVALIDARG;
    };
    // Outside the lock: the object's AddRef may call back into COM.
    // SAFETY: the caller's COM object.
    unsafe { (unknown(unk as usize).add_ref)(unk) };
    put(cookie, given);
    if already { MK_S_MONIKERALREADYREGISTERED } else { S_OK }
}

/// RevokeActiveObject(cookie, reserved): takes the registration back and
/// gives up the reference it held. A cookie that is not in force is
/// E_INVALIDARG, so revoking twice fails the second time.
#[unsafe(no_mangle)]
pub extern "system" fn RevokeActiveObject(cookie: u32, _reserved: *mut c_void) -> HResult {
    if !com_initialized() {
        return CO_E_NOTINITIALIZED;
    }
    if cookie == 0 {
        return E_INVALIDARG;
    }
    let found = locked(|table| {
        let at = table.slots.iter().position(|s| s.cookie == cookie)?;
        let object = table.slots[at].object;
        table.slots[at] = Slot { cookie: 0, clsid: [0; 16], object: 0 };
        Some(object)
    });
    let Some(object) = found else { return E_INVALIDARG };
    // Outside the lock: Release may run the object's destructor.
    // SAFETY: the object that was registered, still alive: the table
    // held a reference to it.
    unsafe { (unknown(object).release)(object as *mut c_void) };
    S_OK
}

/// GetActiveObject(clsid, reserved, out): the running object of `clsid`,
/// AddRef'd, or MK_E_UNAVAILABLE when none is registered. Of several
/// registrations of one CLSID the first one still in force answers, as
/// on Windows.
#[unsafe(no_mangle)]
pub extern "system" fn GetActiveObject(
    clsid: *const Guid,
    _reserved: *mut c_void,
    out: *mut *mut c_void,
) -> HResult {
    if !com_initialized() {
        return CO_E_NOTINITIALIZED;
    }
    if clsid.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's CLSID.
    let clsid = unsafe { *clsid };
    // The cookie's high word counts up, so the smallest cookie of the
    // class is its oldest registration.
    let found = locked(|table| {
        table
            .slots
            .iter()
            .filter(|s| s.cookie != 0 && s.clsid == clsid)
            .min_by_key(|s| s.cookie)
            .map(|s| s.object)
    });
    let Some(object) = found else {
        put(out, core::ptr::null_mut());
        missing(b"oleaut32.dll!GetActiveObject(no running object of that class)", 0);
        return MK_E_UNAVAILABLE;
    };
    // SAFETY: the object that was registered, still alive: the table
    // holds a reference to it.
    unsafe { (unknown(object).add_ref)(object as *mut c_void) };
    put(out, object as *mut c_void);
    S_OK
}
