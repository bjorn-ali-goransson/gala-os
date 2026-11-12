//! gala's AppXDeploymentClient.dll: the Windows Runtime class
//! Windows.Management.Deployment.PackageManager (the DLL Windows registers
//! it from), answering as a machine with no MSIX/AppX packages installed
//! (the Office Deployment Tool's setup.exe looks for a Store Office with
//! it, ROADMAP OF). Registered under
//! `HKLM\SOFTWARE\Microsoft\WindowsRuntime\ActivatableClassId` in both
//! registry views (kernel/src/nt/registry.seed); ole32's
//! RoGetActivationFactory loads it and calls DllGetActivationFactory.
//!
//! - The factory (IActivationFactory) makes PackageManager objects with
//!   IPackageManager and IPackageManager2 (the interfaces' method order
//!   from Wine's include/windows.management.deployment.idl). The factory,
//!   the collections and their iterators hold no state: they are static
//!   objects whose reference counts are nominal. Every object is agile
//!   (IAgileObject), as PackageManager is (marshaling_behavior(agile)).
//! - Every Find query answers "no packages": an empty
//!   IIterable<Package> (or IIterable<PackageUserInformation> for
//!   FindUsers) whose iterator has no current item; FindPackage by full
//!   name gives NULL. Each query is recorded to the ledger ("other",
//!   `appxdeploymentclient.dll!<method>(<the name asked>)`, as much as
//!   the ledger's 64 characters keep), so a run shows what it asked
//!   and was answered "none". The all-users queries
//!   answer so for any caller (Windows wants an administrator: not
//!   checked).
//! - Deployment (AddPackageAsync and the other asynchronous operations)
//!   and SetPackageState are E_NOTIMPL, recorded ("not implemented").
//!
//! The parameterized interface IDs (IIterable<Package> and the others)
//! are computed as the Windows Runtime type system specifies them: the
//! name-based (SHA-1, version 5) UUID of the pinterface signature, e.g.
//! `pinterface({faa585ea-6214-4217-afda-7f46de5869b3};rc(Windows.ApplicationModel.Package;{163c792f-bd75-413c-bf23-b1fe7b95d825}))`,
//! in the namespace {11f47ad5-7b73-42c0-abae-878b1e16adee}.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

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
const E_BOUNDS: HResult = 0x8000_000bu32 as i32;
const CLASS_E_CLASSNOTAVAILABLE: HResult = 0x8004_0111u32 as i32;

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
/// {AF86E2E0-B12D-4C6A-9C5A-D7AA65101E90}
const IID_IINSPECTABLE: Guid =
    guid(0xaf86e2e0, 0xb12d, 0x4c6a, [0x9c, 0x5a, 0xd7, 0xaa, 0x65, 0x10, 0x1e, 0x90]);
/// {94EA2B94-E9CC-49E0-C0FF-EE64CA8F5B90}
const IID_IAGILEOBJECT: Guid =
    guid(0x94ea2b94, 0xe9cc, 0x49e0, [0xc0, 0xff, 0xee, 0x64, 0xca, 0x8f, 0x5b, 0x90]);
const IID_IACTIVATIONFACTORY: Guid = guid(0x35, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
/// {9A7D4B65-5E8F-4FC7-A2E5-7F6925CB8B53}
const IID_IPACKAGEMANAGER: Guid =
    guid(0x9a7d4b65, 0x5e8f, 0x4fc7, [0xa2, 0xe5, 0x7f, 0x69, 0x25, 0xcb, 0x8b, 0x53]);
/// {F7AAD08D-0840-46F2-B5D8-CAD47693A095}
const IID_IPACKAGEMANAGER2: Guid =
    guid(0xf7aad08d, 0x0840, 0x46f2, [0xb5, 0xd8, 0xca, 0xd4, 0x76, 0x93, 0xa0, 0x95]);
/// IIterable<Windows.ApplicationModel.Package>:
/// {69AD6AA7-0C49-5F27-A5EB-EF4D59467B6D}
const IID_IITERABLE_PACKAGE: Guid =
    guid(0x69ad6aa7, 0x0c49, 0x5f27, [0xa5, 0xeb, 0xef, 0x4d, 0x59, 0x46, 0x7b, 0x6d]);
/// IIterator<Windows.ApplicationModel.Package>:
/// {0217F069-025C-5EE6-A87F-E782E3B623AE}
const IID_IITERATOR_PACKAGE: Guid =
    guid(0x0217f069, 0x025c, 0x5ee6, [0xa8, 0x7f, 0xe7, 0x82, 0xe3, 0xb6, 0x23, 0xae]);
/// IIterable<Windows.Management.Deployment.PackageUserInformation>:
/// {341348B9-52C8-5B57-9E91-F19F2A05B188}
const IID_IITERABLE_USER: Guid =
    guid(0x341348b9, 0x52c8, 0x5b57, [0x9e, 0x91, 0xf1, 0x9f, 0x2a, 0x05, 0xb1, 0x88]);
/// IIterator<Windows.Management.Deployment.PackageUserInformation>:
/// {75660566-AE43-5858-ADA6-D57DDAE90277}
const IID_IITERATOR_USER: Guid =
    guid(0x75660566, 0xae43, 0x5858, [0xad, 0xa6, 0xd5, 0x7d, 0xda, 0xe9, 0x02, 0x77]);

const CLASS_NAME: &str = "Windows.Management.Deployment.PackageManager";
const PTR: usize = core::mem::size_of::<usize>();

/// A vtable (function addresses).
struct VTable<const N: usize>([*const (); N]);
// SAFETY: function addresses only.
unsafe impl<const N: usize> Sync for VTable<N> {}

/// A static object: its vtable and what QueryInterface answers.
#[repr(C)]
struct Static {
    vtable: *const (),
    /// The interface IDs this object is (beside IUnknown, IInspectable
    /// and IAgileObject).
    iids: &'static [Guid],
    /// GetRuntimeClassName's answer.
    name: &'static str,
}
// SAFETY: immutable.
unsafe impl Sync for Static {}

fn put<T>(out: *mut T, value: T) {
    // SAFETY: the caller's output pointer (checked non-null by callers).
    unsafe { out.write(value) };
}

/// Reports `text` parts to the ledger as `what` (printable ASCII,
/// truncated).
fn record(parts: &[&[u8]], what: u32) {
    let mut line = [0u8; 200];
    let mut n = 0;
    for &b in parts.iter().flat_map(|p| p.iter()) {
        if n == line.len() {
            break;
        }
        line[n] = if (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\' { b } else { b'?' };
        n += 1;
    }
    // SAFETY: a valid buffer of n bytes.
    unsafe { GalaRecordMissing(line.as_ptr(), n, what, 0) };
}

/// Reports an interface asked of `object` that it does not have
/// (ledger "other", `appxdeploymentclient.dll!QI <iid> <object>`, the
/// object past the ledger's 64 characters often cut off): a
/// caller that goes on without checking crashes there, and the IID
/// names what to implement.
fn record_no_interface(object: &[u8], iid: &Guid) {
    let hex = b"0123456789abcdef";
    let mut text = [0u8; 36];
    let mut n = 0;
    // Data1, Data2 and Data3 little-endian; Data4 in order.
    let order: [usize; 16] = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    for (k, &i) in order.iter().enumerate() {
        if matches!(k, 4 | 6 | 8 | 10) {
            text[n] = b'-';
            n += 1;
        }
        text[n] = hex[usize::from(iid[i] >> 4)];
        text[n + 1] = hex[usize::from(iid[i] & 15)];
        n += 2;
    }
    record(&[b"appxdeploymentclient.dll!QI ", &text[..n], b" ", object], 5);
}

/// An HSTRING's text as ASCII (others '?'), at most 96 characters.
fn ascii(s: HString) -> ([u8; 96], usize) {
    let mut out = [0u8; 96];
    let mut len = 0;
    // SAFETY: the caller's HSTRING (NULL is the empty string).
    let p = unsafe { WindowsGetStringRawBuffer(s, &mut len) };
    let n = (len as usize).min(out.len());
    for (i, slot) in out.iter_mut().take(n).enumerate() {
        // SAFETY: the string's `len` units.
        let c = unsafe { *p.add(i) };
        *slot = if c < 0x80 { c as u8 } else { b'?' };
    }
    (out, n)
}

fn new_hstring(text: &str, out: *mut HString) -> HResult {
    let mut buf = [0u16; 96];
    let mut n = 0;
    for c in text.encode_utf16().take(buf.len()) {
        buf[n] = c;
        n += 1;
    }
    // SAFETY: n units at buf.
    unsafe { WindowsCreateString(buf.as_ptr(), n as u32, out) }
}

// ---- IInspectable for the static objects ----

fn static_of<'a>(this: usize) -> &'a Static {
    // SAFETY: `this` is one of the static objects below.
    unsafe { &*(this as *const Static) }
}

extern "system" fn s_query(this: usize, iid: *const Guid, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    if iid.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *iid };
    if [IID_IUNKNOWN, IID_IINSPECTABLE, IID_IAGILEOBJECT].contains(&iid)
        || static_of(this).iids.contains(&iid)
    {
        put(out, this);
        return S_OK;
    }
    record_no_interface(static_of(this).name.as_bytes(), &iid);
    E_NOINTERFACE
}
extern "system" fn s_add_ref(_this: usize) -> u32 {
    2
}
extern "system" fn s_release(_this: usize) -> u32 {
    1
}
extern "system" fn get_iids(_this: usize, count: *mut u32, iids: *mut *mut Guid) -> HResult {
    if count.is_null() || iids.is_null() {
        return E_POINTER;
    }
    put(count, 0);
    put(iids, core::ptr::null_mut());
    S_OK
}
extern "system" fn s_class_name(this: usize, out: *mut HString) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    new_hstring(static_of(this).name, out)
}
extern "system" fn get_trust_level(_this: usize, level: *mut i32) -> HResult {
    if level.is_null() {
        return E_POINTER;
    }
    put(level, 0); // BaseTrust
    S_OK
}

// ---- The empty collections ----

/// IIterable<T>::First: the empty iterator of the same element type.
extern "system" fn first(this: usize, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let iterator =
        if core::ptr::eq(static_of(this), &PACKAGES) { &PACKAGE_ITERATOR } else { &USER_ITERATOR };
    put(out, iterator as *const Static as usize);
    S_OK
}
/// IIterator<T>::get_Current: no current item.
extern "system" fn current(_this: usize, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    E_BOUNDS
}
/// IIterator<T>::get_HasCurrent and MoveNext: false (a WinRT boolean is
/// one byte).
extern "system" fn no_item(_this: usize, out: *mut u8) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    S_OK
}
/// IIterator<T>::GetMany: none.
extern "system" fn get_many(
    _this: usize,
    _capacity: u32,
    _items: *mut usize,
    got: *mut u32,
) -> HResult {
    if got.is_null() {
        return E_POINTER;
    }
    put(got, 0);
    S_OK
}

static ITERABLE_VTABLE: VTable<7> = VTable([
    s_query as *const (),
    s_add_ref as *const (),
    s_release as *const (),
    get_iids as *const (),
    s_class_name as *const (),
    get_trust_level as *const (),
    first as *const (),
]);
static ITERATOR_VTABLE: VTable<10> = VTable([
    s_query as *const (),
    s_add_ref as *const (),
    s_release as *const (),
    get_iids as *const (),
    s_class_name as *const (),
    get_trust_level as *const (),
    current as *const (),
    no_item as *const (),
    no_item as *const (),
    get_many as *const (),
]);

static PACKAGES: Static = Static {
    vtable: &ITERABLE_VTABLE as *const VTable<7> as *const (),
    iids: &[IID_IITERABLE_PACKAGE],
    name: "Windows.Foundation.Collections.IIterable`1<Windows.ApplicationModel.Package>",
};
static PACKAGE_ITERATOR: Static = Static {
    vtable: &ITERATOR_VTABLE as *const VTable<10> as *const (),
    iids: &[IID_IITERATOR_PACKAGE],
    name: "Windows.Foundation.Collections.IIterator`1<Windows.ApplicationModel.Package>",
};
static USERS: Static = Static {
    vtable: &ITERABLE_VTABLE as *const VTable<7> as *const (),
    iids: &[IID_IITERABLE_USER],
    name: "Windows.Foundation.Collections.IIterable`1<Windows.Management.Deployment.PackageUserInformation>",
};
static USER_ITERATOR: Static = Static {
    vtable: &ITERATOR_VTABLE as *const VTable<10> as *const (),
    iids: &[IID_IITERATOR_USER],
    name: "Windows.Foundation.Collections.IIterator`1<Windows.Management.Deployment.PackageUserInformation>",
};

// ---- PackageManager ----

/// A PackageManager: IPackageManager, then IPackageManager2.
#[repr(C)]
struct Manager {
    v1: *const (),
    v2: *const (),
    refs: AtomicU32,
}

/// The manager behind interface pointer `this` at slot `slot`.
fn manager<'a>(this: usize, slot: usize) -> &'a Manager {
    // SAFETY: `this` is a manager's interface pointer number `slot`.
    unsafe { &*((this - slot * PTR) as *const Manager) }
}

fn m_query(m: &Manager, iid: *const Guid, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    if iid.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's IID.
    let iid = unsafe { *iid };
    let base = m as *const Manager as usize;
    let p =
        if [IID_IUNKNOWN, IID_IINSPECTABLE, IID_IAGILEOBJECT, IID_IPACKAGEMANAGER].contains(&iid) {
            base
        } else if iid == IID_IPACKAGEMANAGER2 {
            base + PTR
        } else {
            record_no_interface(b"PackageManager", &iid);
            return E_NOINTERFACE;
        };
    m.refs.fetch_add(1, Ordering::Relaxed);
    put(out, p);
    S_OK
}
fn m_release(m: &Manager) -> u32 {
    let left = m.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        // SAFETY: the block ActivateInstance allocated.
        unsafe { HeapFree(GetProcessHeap(), 0, m as *const Manager as *mut u8) };
    }
    left
}
extern "system" fn m1_query(this: usize, iid: *const Guid, out: *mut usize) -> HResult {
    m_query(manager(this, 0), iid, out)
}
extern "system" fn m1_add_ref(this: usize) -> u32 {
    manager(this, 0).refs.fetch_add(1, Ordering::Relaxed) + 1
}
extern "system" fn m1_release(this: usize) -> u32 {
    m_release(manager(this, 0))
}
extern "system" fn m2_query(this: usize, iid: *const Guid, out: *mut usize) -> HResult {
    m_query(manager(this, 1), iid, out)
}
extern "system" fn m2_add_ref(this: usize) -> u32 {
    manager(this, 1).refs.fetch_add(1, Ordering::Relaxed) + 1
}
extern "system" fn m2_release(this: usize) -> u32 {
    m_release(manager(this, 1))
}
extern "system" fn m_class_name(_this: usize, out: *mut HString) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    new_hstring(CLASS_NAME, out)
}

/// A Find query's answer: `collection`, the query recorded with `name`.
fn answer(method: &[u8], name: HString, collection: &'static Static, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let (text, n) = ascii(name);
    record(&[b"appxdeploymentclient.dll!", method, b"(", &text[..n], b")"], 5);
    put(out, collection as *const Static as usize);
    S_OK
}

/// A method gala does not implement: recorded, E_NOTIMPL.
fn not_implemented(method: &[u8], out: *mut usize) -> HResult {
    if !out.is_null() {
        put(out, 0);
    }
    record(&[b"appxdeploymentclient.dll!IPackageManager::", method], 3);
    E_NOTIMPL
}

const NO_STRING: HString = core::ptr::null_mut();

// IPackageManager.
extern "system" fn add_package(
    _t: usize,
    _u: usize,
    _d: usize,
    _o: u32,
    out: *mut usize,
) -> HResult {
    not_implemented(b"AddPackageAsync", out)
}
extern "system" fn update_package(
    _t: usize,
    _u: usize,
    _d: usize,
    _o: u32,
    out: *mut usize,
) -> HResult {
    not_implemented(b"UpdatePackageAsync", out)
}
extern "system" fn remove_package(_t: usize, _n: HString, out: *mut usize) -> HResult {
    not_implemented(b"RemovePackageAsync", out)
}
extern "system" fn stage_package(_t: usize, _u: usize, _d: usize, out: *mut usize) -> HResult {
    not_implemented(b"StagePackageAsync", out)
}
extern "system" fn register_package(
    _t: usize,
    _u: usize,
    _d: usize,
    _o: u32,
    out: *mut usize,
) -> HResult {
    not_implemented(b"RegisterPackageAsync", out)
}
extern "system" fn find_packages(_t: usize, out: *mut usize) -> HResult {
    answer(b"FindPackages", NO_STRING, &PACKAGES, out)
}
extern "system" fn find_by_sid(_t: usize, sid: HString, out: *mut usize) -> HResult {
    answer(b"FindPackagesByUserSecurityId", sid, &PACKAGES, out)
}
extern "system" fn find_by_name_publisher(
    _t: usize,
    name: HString,
    _p: HString,
    out: *mut usize,
) -> HResult {
    answer(b"FindPackagesByNamePublisher", name, &PACKAGES, out)
}
extern "system" fn find_by_sid_name_publisher(
    _t: usize,
    _sid: HString,
    name: HString,
    _p: HString,
    out: *mut usize,
) -> HResult {
    answer(b"FindPackagesByUserSecurityIdNamePublisher", name, &PACKAGES, out)
}
extern "system" fn find_users(_t: usize, name: HString, out: *mut usize) -> HResult {
    answer(b"FindUsers", name, &USERS, out)
}
extern "system" fn set_package_state(_t: usize, _n: HString, _s: i32) -> HResult {
    not_implemented(b"SetPackageState", core::ptr::null_mut())
}
/// FindPackage: no package of that full name (S_OK and NULL).
fn find_one(method: &[u8], name: HString, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let (text, n) = ascii(name);
    record(&[b"appxdeploymentclient.dll!", method, b"(", &text[..n], b")"], 5);
    put(out, 0);
    S_OK
}
extern "system" fn find_by_full_name(_t: usize, name: HString, out: *mut usize) -> HResult {
    find_one(b"FindPackageByPackageFullName", name, out)
}
extern "system" fn cleanup_for_user(
    _t: usize,
    _n: HString,
    _s: HString,
    out: *mut usize,
) -> HResult {
    not_implemented(b"CleanupPackageForUserAsync", out)
}
extern "system" fn find_by_family(_t: usize, family: HString, out: *mut usize) -> HResult {
    answer(b"FindPackagesByPackageFamilyName", family, &PACKAGES, out)
}
extern "system" fn find_by_sid_family(
    _t: usize,
    _sid: HString,
    family: HString,
    out: *mut usize,
) -> HResult {
    answer(b"FindPackagesByUserSecurityIdPackageFamilyName", family, &PACKAGES, out)
}
extern "system" fn find_by_sid_full_name(
    _t: usize,
    _sid: HString,
    name: HString,
    out: *mut usize,
) -> HResult {
    find_one(b"FindPackageByUserSecurityIdPackageFullName", name, out)
}

// IPackageManager2.
extern "system" fn remove_with_options(
    _t: usize,
    _n: HString,
    _o: u32,
    out: *mut usize,
) -> HResult {
    not_implemented(b"RemovePackageWithOptionsAsync", out)
}
extern "system" fn stage_with_options(
    _t: usize,
    _u: usize,
    _d: usize,
    _o: u32,
    out: *mut usize,
) -> HResult {
    not_implemented(b"StagePackageWithOptionsAsync", out)
}
extern "system" fn register_by_full_name(
    _t: usize,
    _n: HString,
    _d: usize,
    _o: u32,
    out: *mut usize,
) -> HResult {
    not_implemented(b"RegisterPackageByFullNameAsync", out)
}
extern "system" fn find_with_types(_t: usize, _ty: u32, out: *mut usize) -> HResult {
    answer(b"FindPackagesWithPackageTypes", NO_STRING, &PACKAGES, out)
}
extern "system" fn find_by_sid_with_types(
    _t: usize,
    sid: HString,
    _ty: u32,
    out: *mut usize,
) -> HResult {
    answer(b"FindPackagesByUserSecurityIdWithPackageTypes", sid, &PACKAGES, out)
}
extern "system" fn find_by_name_publisher_with_types(
    _t: usize,
    name: HString,
    _p: HString,
    _ty: u32,
    out: *mut usize,
) -> HResult {
    answer(b"FindPackagesByNamePublisherWithPackageTypes", name, &PACKAGES, out)
}
extern "system" fn find_by_sid_name_publisher_with_types(
    _t: usize,
    _sid: HString,
    name: HString,
    _p: HString,
    _ty: u32,
    out: *mut usize,
) -> HResult {
    answer(b"FindPackagesByUserSecurityIdNamePublisherWithPackageTypes", name, &PACKAGES, out)
}
extern "system" fn find_by_family_with_types(
    _t: usize,
    family: HString,
    _ty: u32,
    out: *mut usize,
) -> HResult {
    answer(b"FindPackagesByPackageFamilyNameWithPackageTypes", family, &PACKAGES, out)
}
extern "system" fn find_by_sid_family_with_types(
    _t: usize,
    _sid: HString,
    family: HString,
    _ty: u32,
    out: *mut usize,
) -> HResult {
    answer(b"FindPackagesByUserSecurityIdPackageFamilyNameWithPackageTypes", family, &PACKAGES, out)
}
extern "system" fn stage_user_data(_t: usize, _n: HString, out: *mut usize) -> HResult {
    not_implemented(b"StageUserDataAsync", out)
}

static MANAGER_VTABLE: VTable<22> = VTable([
    m1_query as *const (),
    m1_add_ref as *const (),
    m1_release as *const (),
    get_iids as *const (),
    m_class_name as *const (),
    get_trust_level as *const (),
    add_package as *const (),
    update_package as *const (),
    remove_package as *const (),
    stage_package as *const (),
    register_package as *const (),
    find_packages as *const (),
    find_by_sid as *const (),
    find_by_name_publisher as *const (),
    find_by_sid_name_publisher as *const (),
    find_users as *const (),
    set_package_state as *const (),
    find_by_full_name as *const (),
    cleanup_for_user as *const (),
    find_by_family as *const (),
    find_by_sid_family as *const (),
    find_by_sid_full_name as *const (),
]);
static MANAGER2_VTABLE: VTable<16> = VTable([
    m2_query as *const (),
    m2_add_ref as *const (),
    m2_release as *const (),
    get_iids as *const (),
    m_class_name as *const (),
    get_trust_level as *const (),
    remove_with_options as *const (),
    stage_with_options as *const (),
    register_by_full_name as *const (),
    find_with_types as *const (),
    find_by_sid_with_types as *const (),
    find_by_name_publisher_with_types as *const (),
    find_by_sid_name_publisher_with_types as *const (),
    find_by_family_with_types as *const (),
    find_by_sid_family_with_types as *const (),
    stage_user_data as *const (),
]);

// ---- The factory ----

/// IActivationFactory::ActivateInstance: a new PackageManager.
extern "system" fn activate_instance(_this: usize, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    // SAFETY: plain call.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 0, core::mem::size_of::<Manager>()) }
        .cast::<Manager>();
    if p.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: fresh memory of the object's size.
    unsafe {
        p.write(Manager {
            v1: MANAGER_VTABLE.0.as_ptr().cast(),
            v2: MANAGER2_VTABLE.0.as_ptr().cast(),
            refs: AtomicU32::new(1),
        })
    };
    put(out, p as usize);
    S_OK
}

static FACTORY_VTABLE: VTable<7> = VTable([
    s_query as *const (),
    s_add_ref as *const (),
    s_release as *const (),
    get_iids as *const (),
    s_class_name as *const (),
    get_trust_level as *const (),
    activate_instance as *const (),
]);
static FACTORY: Static = Static {
    vtable: &FACTORY_VTABLE as *const VTable<7> as *const (),
    iids: &[IID_IACTIVATIONFACTORY],
    name: CLASS_NAME,
};

/// DllGetActivationFactory: PackageManager's factory;
/// CLASS_E_CLASSNOTAVAILABLE for any other class.
#[unsafe(no_mangle)]
pub extern "system" fn DllGetActivationFactory(class: HString, out: *mut usize) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, 0);
    let (text, n) = ascii(class);
    if &text[..n] != CLASS_NAME.as_bytes() {
        record(&[b"appxdeploymentclient.dll!DllGetActivationFactory(", &text[..n], b")"], 2);
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    put(out, &FACTORY as *const Static as usize);
    S_OK
}

/// DllCanUnloadNow: never (its objects are static).
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
