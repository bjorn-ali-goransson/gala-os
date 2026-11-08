//! gala's dxgi.dll: a factory with no adapters (ROADMAP GPU G0 and G2).
//! gala has no Direct3D device (`../../gpu_common.rs` says how the
//! device functions fail); a Windows machine always has at least the
//! Basic Render Driver's adapter, gala has none. CreateDXGIFactory,
//! CreateDXGIFactory1 and CreateDXGIFactory2 still give a factory
//! (IDXGIFactory1), as on Windows, because callers need one to ask: the
//! Khronos Vulkan loader verifies registered drivers through it (and
//! adds none without it), Chromium's GPU process lists adapters with it.
//! Its EnumAdapters/EnumAdapters1 find none (DXGI_ERROR_NOT_FOUND), so
//! Chromium still takes the machine as having no GPU. What a factory
//! cannot do without an adapter (swap chains, software adapters,
//! IDXGIFactory2 and later) fails and is in the ledger
//! (`dxgi.dll!<what>`). DXVK's dxgi.dll, beside a program that ships it,
//! is that program's instead (ARCHITECTURE "Software 3D (G2)").
#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

include!("../../gpu_common.rs");

const S_OK: i32 = 0;
const E_NOINTERFACE: i32 = 0x8000_4002_u32 as i32;
const E_NOTIMPL: i32 = 0x8000_4001_u32 as i32;
const E_OUTOFMEMORY: i32 = 0x8007_000e_u32 as i32;
const DXGI_ERROR_INVALID_CALL: i32 = 0x887a_0001_u32 as i32;
const DXGI_ERROR_NOT_FOUND: i32 = 0x887a_0002_u32 as i32;

/// A GUID's bytes as in memory (Data1-3 little-endian).
const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> [u8; 16] {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

/// The interfaces a factory has (dxgi.h): IUnknown, IDXGIObject,
/// IDXGIFactory, IDXGIFactory1.
const INTERFACES: [[u8; 16]; 4] = [
    guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]),
    guid(0xaec2_2fb8, 0x76f3, 0x4639, [0x9b, 0xe0, 0x28, 0xeb, 0x43, 0xa6, 0x7a, 0x2e]),
    guid(0x7b71_66ec, 0x21c7, 0x44ae, [0xb2, 0x1a, 0xc9, 0xae, 0x32, 0x1a, 0xe3, 0x69]),
    guid(0x770a_ae78, 0xf26f, 0x4dba, [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87]),
];

/// The IID at `iid` is one of INTERFACES; otherwise it is in the ledger
/// as `what` with the IID's first 8 bytes (Data1-3) as the detail.
fn known(iid: *const u8, what: &[u8]) -> bool {
    if iid.is_null() {
        return false;
    }
    // SAFETY: the caller's IID, 16 bytes.
    let id = unsafe { core::ptr::read_unaligned(iid.cast::<[u8; 16]>()) };
    if INTERFACES.contains(&id) {
        return true;
    }
    let head = u64::from_le_bytes([id[0], id[1], id[2], id[3], id[4], id[5], id[6], id[7]]);
    // SAFETY: a valid buffer; "unsupported" (2).
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, head) };
    false
}

/// A factory: its vtable, its references and its window association.
#[repr(C)]
struct Factory {
    vtbl: *const Vtbl,
    refs: AtomicU32,
    window: AtomicUsize,
}

// SAFETY: the fields are atomics and a pointer to a static vtable.
unsafe impl Sync for Factory {}

type This = *mut Factory;

#[repr(C)]
struct Vtbl {
    query_interface: extern "system" fn(This, *const u8, *mut *mut c_void) -> i32,
    add_ref: extern "system" fn(This) -> u32,
    release: extern "system" fn(This) -> u32,
    set_private_data: extern "system" fn(This, *const u8, u32, *const c_void) -> i32,
    set_private_data_interface: extern "system" fn(This, *const u8, *mut c_void) -> i32,
    get_private_data: extern "system" fn(This, *const u8, *mut u32, *mut c_void) -> i32,
    get_parent: extern "system" fn(This, *const u8, *mut *mut c_void) -> i32,
    enum_adapters: extern "system" fn(This, u32, *mut *mut c_void) -> i32,
    make_window_association: extern "system" fn(This, usize, u32) -> i32,
    get_window_association: extern "system" fn(This, *mut usize) -> i32,
    create_swap_chain: extern "system" fn(This, *mut c_void, *mut c_void, *mut *mut c_void) -> i32,
    create_software_adapter: extern "system" fn(This, *mut c_void, *mut *mut c_void) -> i32,
    enum_adapters1: extern "system" fn(This, u32, *mut *mut c_void) -> i32,
    is_current: extern "system" fn(This) -> i32,
}

static VTBL: Vtbl = Vtbl {
    query_interface,
    add_ref,
    release,
    set_private_data,
    set_private_data_interface,
    get_private_data,
    get_parent,
    enum_adapters,
    make_window_association,
    get_window_association,
    create_swap_chain,
    create_software_adapter,
    enum_adapters1: enum_adapters,
    is_current,
};

/// Factories live in a fixed pool (no heap in this DLL); a slot whose
/// references reach 0 is free again.
const FACTORIES: usize = 64;

#[allow(clippy::declare_interior_mutable_const)]
const FREE: Factory = Factory { vtbl: &VTBL, refs: AtomicU32::new(0), window: AtomicUsize::new(0) };

static POOL: [Factory; FACTORIES] = [FREE; FACTORIES];

fn new_factory(name: &[u8], iid: *const u8, factory: *mut *mut c_void) -> i32 {
    clear(factory);
    if factory.is_null() {
        return E_INVALIDARG;
    }
    if !known(iid, name) {
        return E_NOINTERFACE;
    }
    let Some(slot) = POOL
        .iter()
        .find(|f| f.refs.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed).is_ok())
    else {
        unsupported(b"dxgi.dll!CreateDXGIFactory(more factories than the pool)");
        return E_OUTOFMEMORY;
    };
    slot.window.store(0, Ordering::Relaxed);
    // SAFETY: the caller's output pointer (checked above).
    unsafe { *factory = core::ptr::from_ref(slot).cast_mut().cast() };
    S_OK
}

/// The factory behind a method's `this`.
fn this(f: This) -> &'static Factory {
    // SAFETY: `this` is one of POOL's factories (only those are handed
    // out), which are static.
    unsafe { &*f }
}

extern "system" fn query_interface(f: This, iid: *const u8, out: *mut *mut c_void) -> i32 {
    clear(out);
    if out.is_null() {
        return E_INVALIDARG;
    }
    if !known(iid, b"dxgi.dll!IDXGIFactory::QueryInterface(interface)") {
        return E_NOINTERFACE;
    }
    add_ref(f);
    // SAFETY: the caller's output pointer (checked above).
    unsafe { *out = f.cast() };
    S_OK
}

extern "system" fn add_ref(f: This) -> u32 {
    this(f).refs.fetch_add(1, Ordering::AcqRel) + 1
}

extern "system" fn release(f: This) -> u32 {
    this(f).refs.fetch_sub(1, Ordering::AcqRel) - 1
}

extern "system" fn set_private_data(
    _f: This,
    _id: *const u8,
    _size: u32,
    _data: *const c_void,
) -> i32 {
    unsupported(b"dxgi.dll!IDXGIFactory::SetPrivateData");
    E_NOTIMPL
}

extern "system" fn set_private_data_interface(_f: This, _id: *const u8, _data: *mut c_void) -> i32 {
    unsupported(b"dxgi.dll!IDXGIFactory::SetPrivateDataInterface");
    E_NOTIMPL
}

/// Nothing was stored (SetPrivateData keeps nothing).
extern "system" fn get_private_data(
    _f: This,
    _id: *const u8,
    size: *mut u32,
    _data: *mut c_void,
) -> i32 {
    if !size.is_null() {
        // SAFETY: the caller's output.
        unsafe { *size = 0 };
    }
    DXGI_ERROR_NOT_FOUND
}

/// A factory has no parent.
extern "system" fn get_parent(_f: This, _iid: *const u8, out: *mut *mut c_void) -> i32 {
    clear(out);
    E_NOINTERFACE
}

/// EnumAdapters/EnumAdapters1: there is no adapter.
extern "system" fn enum_adapters(_f: This, _index: u32, out: *mut *mut c_void) -> i32 {
    if out.is_null() {
        return E_INVALIDARG;
    }
    clear(out);
    DXGI_ERROR_NOT_FOUND
}

/// Kept for GetWindowAssociation; gala handles Alt+Enter nowhere.
extern "system" fn make_window_association(f: This, hwnd: usize, _flags: u32) -> i32 {
    this(f).window.store(hwnd, Ordering::Relaxed);
    S_OK
}

extern "system" fn get_window_association(f: This, hwnd: *mut usize) -> i32 {
    if hwnd.is_null() {
        return DXGI_ERROR_INVALID_CALL;
    }
    // SAFETY: the caller's output.
    unsafe { *hwnd = this(f).window.load(Ordering::Relaxed) };
    S_OK
}

/// No device can exist, so no swap chain.
extern "system" fn create_swap_chain(
    _f: This,
    device: *mut c_void,
    desc: *mut c_void,
    out: *mut *mut c_void,
) -> i32 {
    clear(out);
    if device.is_null() || desc.is_null() || out.is_null() {
        return DXGI_ERROR_INVALID_CALL;
    }
    unsupported(b"dxgi.dll!IDXGIFactory::CreateSwapChain");
    DXGI_ERROR_UNSUPPORTED
}

/// No software rasterizer (WARP is not redistributable).
extern "system" fn create_software_adapter(
    _f: This,
    module: *mut c_void,
    out: *mut *mut c_void,
) -> i32 {
    clear(out);
    if module.is_null() || out.is_null() {
        return DXGI_ERROR_INVALID_CALL;
    }
    unsupported(b"dxgi.dll!IDXGIFactory::CreateSoftwareAdapter");
    DXGI_ERROR_UNSUPPORTED
}

/// Nothing about the adapters ever changes.
extern "system" fn is_current(_f: This) -> i32 {
    1
}

/// CreateDXGIFactory(riid, ppFactory).
#[unsafe(no_mangle)]
pub extern "system" fn CreateDXGIFactory(iid: *const u8, factory: *mut *mut c_void) -> i32 {
    new_factory(b"dxgi.dll!CreateDXGIFactory(interface)", iid, factory)
}

/// CreateDXGIFactory1(riid, ppFactory).
#[unsafe(no_mangle)]
pub extern "system" fn CreateDXGIFactory1(iid: *const u8, factory: *mut *mut c_void) -> i32 {
    new_factory(b"dxgi.dll!CreateDXGIFactory1(interface)", iid, factory)
}

/// CreateDXGIFactory2(Flags, riid, ppFactory): the flags (DXGI_CREATE_
/// FACTORY_DEBUG) change nothing.
#[unsafe(no_mangle)]
pub extern "system" fn CreateDXGIFactory2(
    _flags: u32,
    iid: *const u8,
    factory: *mut *mut c_void,
) -> i32 {
    new_factory(b"dxgi.dll!CreateDXGIFactory2(interface)", iid, factory)
}

/// DXGIGetDebugInterface1(Flags, riid, pDebug): no debug layer.
#[unsafe(no_mangle)]
pub extern "system" fn DXGIGetDebugInterface1(
    _flags: u32,
    _iid: *const u8,
    debug: *mut *mut c_void,
) -> i32 {
    clear(debug);
    unsupported(b"dxgi.dll!DXGIGetDebugInterface1");
    if debug.is_null() { E_INVALIDARG } else { DXGI_ERROR_UNSUPPORTED }
}
