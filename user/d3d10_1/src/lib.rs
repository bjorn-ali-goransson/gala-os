//! gala's d3d10_1.dll: Direct3D 10.1 over the program's own Direct3D 11
//! (ROADMAP OF, GPU G2; ARCHITECTURE "Direct3D 10.1"), which is the path
//! Office takes when it does not draw through Direct2D's device.
//!
//! gala has no Direct3D driver of its own: the Direct3D 11 a program
//! brings beside itself (DXVK over Vulkan on Mesa's lavapipe in gala's
//! scenarios) is the one underneath. So a device of this DLL is that
//! program's own ID3D11Device and immediate context, and a texture of
//! this DLL is a Direct3D 11 texture -- which it hands over to anyone
//! who asks for an interface Direct3D 10 does not declare, so gala's
//! Direct2D reaches the surface behind it and draws into it as it would
//! into any other (`device`, `texture`, d2d1's `device::open_surface`).
//!
//! What a device does not answer is in the ledger under its own name
//! (`d3d10_1.dll!ID3D10Device1::<method>`, not implemented), because
//! the device has a method for every slot of the interface whether or
//! not it answers it: which of them a program asks for is what says
//! what to implement next (OBSERVABILITY "Missing-features ledger").
//! Word's shared code (Mso40UIwin32client.dll) and Office's image
//! modules (OIMG.DLL, VVIEWER.DLL) delay-load this DLL, so its exports
//! have to be there whatever they answer: a delay load that cannot be
//! resolved ends the process with 0xc06d007e before it can handle
//! anything, which is how Word died (`gpu_stub_test`, ROADMAP OF).
#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

mod device;
mod texture;

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use gala_heap as _;

include!("../../gpu_common.rs");

// The DLL half of `trace_from=` and the records a generated vtable
// wrapper leaves, one file shared with gala's other traced DLLs
// (`user/trace_common.rs`, OBSERVABILITY "Call tracing").
#[path = "../../trace_common.rs"]
pub mod trace;

// The memory routines come from gala's ntdll (as other DLLs on `alloc`
// take them).
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

pub type HResult = i32;
pub type Guid = [u8; 16];
pub type Obj = *mut c_void;

pub const S_OK: HResult = 0;
pub const E_NOTIMPL: HResult = 0x8000_4001_u32 as i32;
pub const E_NOINTERFACE: HResult = 0x8000_4002_u32 as i32;
pub const E_POINTER: HResult = 0x8000_4003_u32 as i32;
pub const E_OUTOFMEMORY: HResult = 0x8007_000e_u32 as i32;

/// The ledger's kinds (GalaRecordMissing's codes).
const LEDGER_UNSUPPORTED: u32 = 2;
const LEDGER_NOT_IMPLEMENTED: u32 = 3;
const LEDGER_OTHER: u32 = 5;
const LEDGER_REACHED: u32 = 9;

/// This DLL's name as the ledger spells it: the prefix of every row
/// it writes and of every name a trace reports (`trace_common.rs`).
const DLL_NAME: &[u8] = b"d3d10_1.dll!";

fn record_kind(what: &[u8], kind: u32, detail: u64) {
    let mut name = [0u8; 96];
    let mut n = 0;
    for &b in DLL_NAME.iter().chain(what) {
        if n == name.len() {
            break;
        }
        name[n] = b;
        n += 1;
    }
    // SAFETY: a valid buffer; the service only reads it.
    unsafe { GalaRecordMissing(name.as_ptr(), n, kind, detail) };
}

/// Records `d3d10_1.dll!<what>`: something this DLL does not support (an
/// argument, a case), with a detail.
pub fn gap(what: &[u8], detail: u64) {
    record_kind(what, LEDGER_UNSUPPORTED, detail);
}

/// Records `d3d10_1.dll!<what>`: a call a program made and was answered,
/// the HRESULT it was answered with as the detail (0 for success). A
/// device that works leaves nothing behind otherwise, so a program that
/// stops after one of these shows how far it got (OBSERVABILITY
/// "Missing-features ledger").
pub fn answered(what: &[u8], hr: HResult) {
    record_kind(what, LEDGER_OTHER, hr as u32 as u64);
}

/// A method gala does not implement: recorded, E_NOTIMPL.
pub fn not_implemented(method: &[u8]) -> HResult {
    record_kind(method, LEDGER_NOT_IMPLEMENTED, 0);
    E_NOTIMPL
}

/// The first dword of a GUID (a ledger detail).
pub fn data1(g: &Guid) -> u64 {
    u64::from(u32::from_le_bytes([g[0], g[1], g[2], g[3]]))
}

/// The bytes of a GUID given as DEFINE_GUID does.
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

/// The Direct3D 10 interfaces whose IIDs share their last ten bytes
/// (9b7e4cxx-342c-4106-a19f-4f2704f689f0).
pub const fn d3d10_iid(d1: u32) -> Guid {
    guid(d1, 0x342c, 0x4106, [0xa1, 0x9f, 0x4f, 0x27, 0x04, 0xf6, 0x89, 0xf0])
}
pub const IID_DEVICE_CHILD: Guid = d3d10_iid(0x9b7e_4c00);
pub const IID_RESOURCE: Guid = d3d10_iid(0x9b7e_4c01);
pub const IID_TEXTURE2D: Guid = d3d10_iid(0x9b7e_4c04);
pub const IID_DEVICE: Guid = d3d10_iid(0x9b7e_4c0f);
/// ID3D10Device1 (d3d10_1.h).
pub const IID_DEVICE1: Guid = d3d10_iid(0x9b7e_4c8f);

/// Reads the caller's GUID (None for NULL).
pub fn read_guid(g: *const Guid) -> Option<Guid> {
    if g.is_null() {
        None
    } else {
        // SAFETY: the caller's GUID.
        Some(unsafe { g.read_unaligned() })
    }
}

/// Stores `v` at the caller's output, if any.
pub fn put<T>(out: *mut T, v: T) {
    if !out.is_null() {
        // SAFETY: the caller's output.
        unsafe { out.write_unaligned(v) };
    }
}

/// A new heap object holding `v` (None when out of memory).
fn try_box<T>(v: T) -> Option<*mut T> {
    let layout = core::alloc::Layout::new::<T>();
    // SAFETY: a non-zero-sized layout (every object has a header).
    let p = unsafe { alloc::alloc::alloc(layout) }.cast::<T>();
    if p.is_null() {
        return None;
    }
    // SAFETY: fresh memory for one T.
    unsafe { p.write(v) };
    Some(p)
}

/// Frees an object from `try_box`.
///
/// # Safety
/// `p` came from `try_box::<T>` and is not used after.
unsafe fn free_box<T>(p: *mut T) {
    // SAFETY: as the caller promises.
    unsafe {
        core::ptr::drop_in_place(p);
        alloc::alloc::dealloc(p.cast(), core::alloc::Layout::new::<T>());
    }
}

/// Boxes `v` and hands it to the caller's `out` (E_OUTOFMEMORY when
/// there is no memory, `out` then NULL).
pub fn give<T>(out: *mut Obj, v: T) -> HResult {
    match try_box(v) {
        Some(p) => {
            put(out, p.cast());
            S_OK
        }
        None => {
            put(out, core::ptr::null_mut());
            E_OUTOFMEMORY
        }
    }
}

/// A vtable: the methods' addresses, in the interface's order.
#[repr(C)]
pub struct VTable<const N: usize>(pub [*const (); N]);
// SAFETY: function addresses only, never written.
unsafe impl<const N: usize> Sync for VTable<N> {}

impl<const N: usize> VTable<N> {
    pub const fn addr(&'static self) -> *const () {
        (self as *const Self).cast()
    }
}

/// What every object of this DLL starts with: its vtable and its
/// references.
#[repr(C)]
pub struct Header {
    pub vtbl: *const (),
    pub refs: AtomicU32,
}
// SAFETY: an immutable vtable pointer and an atomic.
unsafe impl Sync for Header {}

impl Header {
    pub const fn new(vtbl: *const ()) -> Self {
        Header { vtbl, refs: AtomicU32::new(1) }
    }
}

/// The object's vtable (to recognise this DLL's own objects).
pub fn vtable_of(o: Obj) -> *const () {
    if o.is_null() {
        return core::ptr::null();
    }
    // SAFETY: every COM object starts with its vtable pointer.
    unsafe { *(o as *const *const ()) }
}

/// IUnknown::AddRef of this DLL's objects.
pub extern "system" fn add_ref(this: Obj) -> u32 {
    // SAFETY: one of our objects: it starts with a Header.
    unsafe { (*(this as *const Header)).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

/// Drops one reference of object `this` of type `T`, freeing it at zero.
pub fn release_object<T>(this: Obj) -> u32 {
    // SAFETY: one of our objects of type T: it starts with a Header.
    let left = unsafe { (*(this as *const Header)).refs.fetch_sub(1, Ordering::AcqRel) } - 1;
    if left == 0 {
        // SAFETY: the last reference is gone; it came from try_box::<T>.
        unsafe { free_box(this as *mut T) };
    }
    left
}

/// Slot `i` of COM object `o`'s vtable.
pub fn slot(o: Obj, i: usize) -> *const () {
    // SAFETY: a COM object: its first field points at its vtable, which
    // has at least the slots of the interface it was given as.
    unsafe { *(*(o as *const *const *const ())).add(i) }
}

/// IUnknown::AddRef of any COM object.
pub fn com_add_ref(o: Obj) {
    // SAFETY: IUnknown::AddRef's signature.
    let f: extern "system" fn(Obj) -> u32 = unsafe { core::mem::transmute(slot(o, 1)) };
    f(o);
}

/// IUnknown::Release of any COM object.
pub fn com_release(o: Obj) {
    // SAFETY: IUnknown::Release's signature.
    let f: extern "system" fn(Obj) -> u32 = unsafe { core::mem::transmute(slot(o, 2)) };
    f(o);
}

/// A counted reference to a COM object, released when dropped.
pub struct ComRef(pub Obj);

impl ComRef {
    /// A reference the caller already holds, adopted (not added): what a
    /// method that hands out an object leaves us with.
    pub fn owned(o: Obj) -> ComRef {
        ComRef(o)
    }

    /// A new reference to `o` (NULL stays NULL).
    pub fn new(o: Obj) -> ComRef {
        if !o.is_null() {
            com_add_ref(o);
        }
        ComRef(o)
    }
}

impl Drop for ComRef {
    fn drop(&mut self) {
        if !self.0.is_null() {
            com_release(self.0);
        }
    }
}

/// D3D10_1_SDK_VERSION: the only SDK version D3D10CreateDevice1 takes.
/// Windows checks it before it looks for an adapter -- measured, with
/// every other value E_INVALIDARG on a machine that has one.
const D3D10_1_SDK_VERSION: u32 = 0x20;

/// D3D10CreateDevice1(pAdapter, DriverType, Software, Flags,
/// HardwareLevel, SDKVersion, ppDevice): a Direct3D 10.1 device over the
/// program's own Direct3D 11 (`device`).
#[unsafe(no_mangle)]
pub extern "system" fn D3D10CreateDevice1(
    adapter: *mut c_void,
    driver: u32,
    software: *mut c_void,
    flags: u32,
    level: u32,
    sdk: u32,
    out: *mut *mut c_void,
) -> i32 {
    clear(out);
    if sdk != D3D10_1_SDK_VERSION {
        return E_INVALIDARG;
    }
    device::create(adapter, driver, software, flags, level, out.cast())
}

/// D3D10CreateStateBlock(pDevice, pStateBlockMask, ppStateBlock): a
/// state block records a device's state, which this DLL does not keep,
/// so this is E_INVALIDARG -- what Windows answers when asked without a
/// device (measured).
#[unsafe(no_mangle)]
pub extern "system" fn D3D10CreateStateBlock(
    _device: *mut c_void,
    _mask: *mut c_void,
    block: *mut *mut c_void,
) -> i32 {
    clear(block);
    unsupported(b"d3d10_1.dll!D3D10CreateStateBlock");
    E_INVALIDARG
}
