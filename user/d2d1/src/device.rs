//! The Direct2D 1.1 device (ID2D1Device) and the Direct3D 11 surfaces a
//! program's own device gives it (ROADMAP OF, GPU G2).
//!
//! `ID2D1Factory1::CreateDevice` takes the program's `IDXGIDevice` (on
//! gala, DXVK's over Mesa's lavapipe) and the device hands out device
//! contexts, which are `target`'s fourth kind of render target. Direct2D
//! itself still draws in software: what a context draws lands in the
//! pixels of the `ID2D1Bitmap1` it was given as its target, and a bitmap
//! made from a DXGI surface writes those pixels into the surface when
//! the frame ends. That write is
//! `ID3D11DeviceContext::UpdateSubresource` on the `ID3D11Texture2D`
//! behind the surface: the one way into a texture a program uses as a
//! render target that needs neither a shader nor a staging texture of
//! our own.
//!
//! Nothing here is linked against. Every call into the program's
//! Direct3D goes through the vtable slot the interface declares (dxgi.h,
//! d3d11.h), so gala's d2d1.dll imports no Direct3D at all and works
//! with whichever d3d11.dll the program brought.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::{
    ComRef, E_INVALIDARG, E_NOINTERFACE, E_POINTER, Guid, HResult, IID_DEVICE, IID_DEVICE1,
    IID_RESOURCE, Obj, Res, S_OK, VTable, add_ref, answered, gap, get_factory, give, guid,
    not_implemented, put, query, reached, release_object, slot,
};

/// The slots this DLL calls on the program's DXGI and Direct3D 11
/// objects: the index the interface declares, IUnknown's three included
/// (dxgi.h, d3d11.h).
const SURFACE_GET_DESC: usize = 8;
/// ID3D11DeviceChild::GetDevice, which ID3D11Texture2D inherits.
const TEXTURE_GET_DEVICE: usize = 3;
/// ID3D11Texture2D::GetDesc, which says what the texture may be bound as.
const TEXTURE_GET_DESC: usize = 10;
const DEVICE_GET_IMMEDIATE_CONTEXT: usize = 40;
const CONTEXT_UPDATE_SUBRESOURCE: usize = 48;

const IID_D3D11_TEXTURE2D: Guid =
    guid(0x6f15_aaf2, 0xd208, 0x4e89, [0x9a, 0xb4, 0x48, 0x95, 0x35, 0xd3, 0x4f, 0x9c]);
/// IDXGIDevice (dxgi.h): the face of a Direct3D device Direct2D takes.
const IID_DXGI_DEVICE: Guid =
    guid(0x54ec_77fa, 0x1377, 0x44e6, [0x8c, 0x32, 0x88, 0xfd, 0x5f, 0x44, 0xc8, 0x4c]);

/// D3D11_TEXTURE2D_DESC (d3d11.h). Only `bind` is read: Direct2D takes
/// a bitmap's options from it when the program gives none.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Texture2dDesc {
    width: u32,
    height: u32,
    mip_levels: u32,
    array_size: u32,
    format: u32,
    sample_count: u32,
    sample_quality: u32,
    usage: u32,
    bind: u32,
    cpu_access: u32,
    misc: u32,
}

/// DXGI_SURFACE_DESC (dxgi.h).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SurfaceDesc {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub samples: u32,
    pub quality: u32,
}

/// A DXGI surface this DLL can write pixels into: the surface, the
/// ID3D11Texture2D behind it and that texture's device's immediate
/// context, each with a reference of ours, and what the surface is.
pub struct DxgiSurface {
    pub surface: ComRef,
    /// The texture's D3D11_BIND_FLAG set, which says what a bitmap over
    /// it may be used for when the program asks for no options.
    pub bind: u32,
    texture: ComRef,
    context: ComRef,
    pub desc: SurfaceDesc,
}

/// QueryInterface on any COM object (None when it does not answer).
fn com_query(o: Obj, iid: &Guid) -> Option<Obj> {
    // SAFETY: IUnknown::QueryInterface is slot 0 of every interface.
    let f: extern "system" fn(Obj, *const Guid, *mut Obj) -> HResult =
        unsafe { core::mem::transmute(slot(o, 0)) };
    let mut out = core::ptr::null_mut();
    if f(o, iid, &mut out) == S_OK && !out.is_null() { Some(out) } else { None }
}

/// A method of a foreign object that hands out one object through an out
/// parameter and returns nothing (ID3D11DeviceChild::GetDevice,
/// ID3D11Device::GetImmediateContext). The reference is ours.
fn com_get(o: Obj, index: usize) -> Option<ComRef> {
    // SAFETY: both methods take the object and one out pointer, and the
    // caller names the slot of one of them.
    let f: extern "system" fn(Obj, *mut Obj) = unsafe { core::mem::transmute(slot(o, index)) };
    let mut out = core::ptr::null_mut();
    f(o, &mut out);
    (!out.is_null()).then(|| ComRef::owned(out))
}

/// Opens a DXGI surface for writing: what it is, the texture behind it
/// and its device's immediate context. The surface has to be one
/// Direct3D 11 made (it answers QueryInterface for ID3D11Texture2D),
/// which a swap chain's buffer and a render target of a program's own
/// device are; anything else is recorded and E_NOINTERFACE.
pub fn open_surface(surface: Obj) -> Result<DxgiSurface, HResult> {
    if surface.is_null() {
        return Err(E_INVALIDARG);
    }
    let mut desc = SurfaceDesc::default();
    // SAFETY: IDXGISurface::GetDesc's signature at its own slot.
    let get_desc: extern "system" fn(Obj, *mut SurfaceDesc) -> HResult =
        unsafe { core::mem::transmute(slot(surface, SURFACE_GET_DESC)) };
    let r = get_desc(surface, &mut desc);
    if r != S_OK {
        gap(b"CreateBitmapFromDxgiSurface(IDXGISurface::GetDesc)", r as u32 as u64);
        return Err(r);
    }
    let Some(texture) = com_query(surface, &IID_D3D11_TEXTURE2D).map(ComRef::owned) else {
        gap(b"CreateBitmapFromDxgiSurface(not an ID3D11Texture2D)", 0);
        return Err(E_NOINTERFACE);
    };
    // What the texture may be bound as: Direct2D derives a bitmap's
    // options from it (`brush::inferred_options`).
    let mut tex_desc = Texture2dDesc::default();
    // SAFETY: ID3D11Texture2D::GetDesc's signature at its own slot; it
    // writes the whole structure and returns nothing.
    let get_tex_desc: extern "system" fn(Obj, *mut Texture2dDesc) =
        unsafe { core::mem::transmute(slot(texture.0, TEXTURE_GET_DESC)) };
    get_tex_desc(texture.0, &mut tex_desc);
    // The device is only the way to its immediate context, which holds a
    // reference of its own to it.
    let context = com_get(texture.0, TEXTURE_GET_DEVICE)
        .and_then(|device| com_get(device.0, DEVICE_GET_IMMEDIATE_CONTEXT));
    let Some(context) = context else {
        gap(b"CreateBitmapFromDxgiSurface(no immediate context)", 0);
        return Err(E_NOINTERFACE);
    };
    Ok(DxgiSurface { surface: ComRef::new(surface), bind: tex_desc.bind, texture, context, desc })
}

/// The IDXGIDevice of the Direct3D device a surface belongs to: the
/// surface's ID3D11Texture2D asked for its device, and that device for
/// DXGI's face of itself. A Direct2D 1.0 surface render target is built
/// on it, because the target it hands the program is a device context
/// and a device context comes from a device
/// (`target::create_dxgi_target`).
pub fn dxgi_device(surface: Obj) -> Option<ComRef> {
    let texture = com_query(surface, &IID_D3D11_TEXTURE2D).map(ComRef::owned)?;
    let device = com_get(texture.0, TEXTURE_GET_DEVICE)?;
    com_query(device.0, &IID_DXGI_DEVICE).map(ComRef::owned)
}

/// Writes `pixels` (premultiplied BGRA, rows `width * 4` bytes apart)
/// into the whole surface. UpdateSubresource takes the rows as they are:
/// Direct3D copies them into the texture itself, so a frame drawn in
/// software becomes the program's render target without a shader.
pub fn write_surface(s: &DxgiSurface, pixels: &[u8]) {
    let (w, h) = (s.desc.width, s.desc.height);
    if w == 0 || h == 0 || pixels.len() < w as usize * h as usize * 4 {
        return;
    }
    // SAFETY: ID3D11DeviceContext::UpdateSubresource's signature at its
    // own slot; subresource 0, the whole resource (a NULL box), from our
    // own rows.
    let update: extern "system" fn(Obj, Obj, u32, *const u8, *const u8, u32, u32) =
        unsafe { core::mem::transmute(slot(s.context.0, CONTEXT_UPDATE_SUBRESOURCE)) };
    update(s.context.0, s.texture.0, 0, core::ptr::null(), pixels.as_ptr(), w * 4, 0);
}

// ---- ID2D1Device ---------------------------------------------------------------

#[repr(C)]
pub struct Device {
    res: Res,
    /// The program's IDXGIDevice, kept for the device's lifetime (the
    /// pixels reach Direct3D through the surfaces a context's target
    /// bitmaps are made from, not through this).
    dxgi: ComRef,
    /// SetMaximumTextureMemory's value, answered as it was given.
    texture_memory: AtomicU64,
    /// ID2D1Device1's rendering priority, answered as it was given:
    /// NORMAL (0) or LOW (1). It asks Direct2D to yield the GPU to other
    /// programs, and gala draws on the calling thread either way.
    priority: AtomicU32,
}

/// ID2D1Device1 (d2d1_2.h): ID2D1Device's nine methods, then its three.
/// One object answers both interfaces, so it has the longer vtable.
static DEVICE_VTBL: VTable<12> = VTable([
    device_query as *const (),
    add_ref as *const (),
    device_release as *const (),
    get_factory as *const (),
    d_CreateDeviceContext as *const (),
    d_CreatePrintControl as *const (),
    d_SetMaximumTextureMemory as *const (),
    d_GetMaximumTextureMemory as *const (),
    d_ClearResources as *const (),
    d1_GetRenderingPriority as *const (),
    d1_SetRenderingPriority as *const (),
    d1_CreateDeviceContext as *const (),
]);

/// ID2D1Factory1::CreateDevice: a Direct2D device over the program's own
/// Direct3D 11 device, given as its IDXGIDevice. Which interface the
/// program asked through is in the ledger, so a run says whether it took
/// 1.1's factory or 1.2's.
pub extern "system" fn f1_CreateDevice(factory: Obj, dxgi: Obj, out: *mut Obj) -> HResult {
    let r = create_device(factory, dxgi, out);
    answered(b"ID2D1Factory1::CreateDevice", r);
    r
}

/// ID2D1Factory2::CreateDevice: the same device, which answers
/// ID2D1Device1 as well (its vtable has 1.2's three methods and its
/// QueryInterface the interface).
pub extern "system" fn f2_CreateDevice(factory: Obj, dxgi: Obj, out: *mut Obj) -> HResult {
    let r = create_device(factory, dxgi, out);
    answered(b"ID2D1Factory2::CreateDevice", r);
    r
}

/// The device itself, without a ledger record: what the factory's two
/// methods above do, and what a surface render target does for itself
/// (the program asked for a render target, not for a device, and the
/// ledger says what the program asked for).
pub fn create_device(factory: Obj, dxgi: Obj, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if dxgi.is_null() {
        return E_INVALIDARG;
    }
    give(
        out,
        Device {
            res: Res::new(DEVICE_VTBL.addr(), factory),
            dxgi: ComRef::new(dxgi),
            texture_memory: AtomicU64::new(0),
            priority: AtomicU32::new(RENDERING_PRIORITY_NORMAL),
        },
    )
}

fn device<'a>(o: Obj) -> &'a Device {
    // SAFETY: one of our devices.
    unsafe { &*(o as *const Device) }
}

/// The factory of one of our devices (a context takes its own from it).
pub fn factory_of(o: Obj) -> Obj {
    device(o).res.factory.0
}

extern "system" fn device_query(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    query(this, b"ID2D1Device", &[IID_RESOURCE, IID_DEVICE, IID_DEVICE1], riid, out)
}

extern "system" fn device_release(this: Obj) -> u32 {
    release_object::<Device>(this)
}

extern "system" fn d_CreateDeviceContext(this: Obj, options: u32, out: *mut Obj) -> HResult {
    let r = crate::target::create_device_context(this, options, out);
    answered(b"ID2D1Device::CreateDeviceContext", r);
    r
}

extern "system" fn d_CreatePrintControl(
    _this: Obj,
    _wic: Obj,
    _target: Obj,
    _props: *const u8,
    out: *mut Obj,
) -> HResult {
    reached!(b"ID2D1Device::CreatePrintControl");
    put(out, core::ptr::null_mut());
    not_implemented(b"ID2D1Device::CreatePrintControl")
}

extern "system" fn d_SetMaximumTextureMemory(this: Obj, bytes: u64) {
    reached!(b"ID2D1Device::SetMaximumTextureMemory");
    device(this).texture_memory.store(bytes, Ordering::Relaxed);
}

extern "system" fn d_GetMaximumTextureMemory(this: Obj) -> u64 {
    reached!(b"ID2D1Device::GetMaximumTextureMemory");
    device(this).texture_memory.load(Ordering::Relaxed)
}

/// ClearResources: gala's Direct2D keeps no cache of its own to drop.
extern "system" fn d_ClearResources(_this: Obj, _milliseconds_since_use: u32) {
    reached!(b"ID2D1Device::ClearResources");
}

// ---- ID2D1Device1 (Direct2D 1.2) -----------------------------------------------

/// D2D1_RENDERING_PRIORITY_NORMAL (d2d1_2.h); LOW is 1.
const RENDERING_PRIORITY_NORMAL: u32 = 0;
const RENDERING_PRIORITY_LOW: u32 = 1;

extern "system" fn d1_GetRenderingPriority(this: Obj) -> u32 {
    reached!(b"ID2D1Device1::GetRenderingPriority");
    device(this).priority.load(Ordering::Relaxed)
}

/// A priority other than NORMAL or LOW is not one Direct2D has: it is
/// recorded and the device keeps the priority it had.
extern "system" fn d1_SetRenderingPriority(this: Obj, priority: u32) {
    reached!(b"ID2D1Device1::SetRenderingPriority");
    if priority != RENDERING_PRIORITY_NORMAL && priority != RENDERING_PRIORITY_LOW {
        gap(b"ID2D1Device1::SetRenderingPriority(unknown)", priority.into());
        return;
    }
    device(this).priority.store(priority, Ordering::Relaxed);
}

/// ID2D1Device1::CreateDeviceContext: the same context, which answers
/// ID2D1DeviceContext1 as well (`target`).
extern "system" fn d1_CreateDeviceContext(this: Obj, options: u32, out: *mut Obj) -> HResult {
    let r = crate::target::create_device_context(this, options, out);
    answered(b"ID2D1Device1::CreateDeviceContext", r);
    r
}
