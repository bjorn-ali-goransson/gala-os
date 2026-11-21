//! The Direct3D 10 textures of a device of this DLL: a Direct3D 11
//! texture underneath, and the Direct3D 10 interface over it.
//!
//! Everything Direct3D 10 does not declare is answered with the real
//! texture: an IDXGISurface, an ID3D11Texture2D, whatever a program
//! asks for. That is what lets gala's Direct2D draw into a texture of
//! ours -- it asks the surface it was given for the Direct3D 11 texture
//! behind it and writes the frame into that (d2d1's
//! `device::open_surface`) -- and it is also how Direct3D 10 on Windows
//! behaves, where the two interfaces are two faces of one object.

use core::ffi::c_void;
use core::ptr::null_mut;

use crate::{
    ComRef, E_INVALIDARG, E_POINTER, Guid, HResult, Header, IID_DEVICE_CHILD, IID_IUNKNOWN,
    IID_RESOURCE, IID_TEXTURE2D, Obj, S_OK, VTable, add_ref, gap, give, put, read_guid,
    release_object, slot, vtable_of,
};

/// ID3D11DeviceChild's, ID3D11Resource's and ID3D11Texture2D's slots,
/// counted from IUnknown as d3d11.h declares them (WORKFLOW "A vtable
/// slot is counted from IUnknown").
const CHILD_GET_PRIVATE_DATA: usize = 4;
const CHILD_SET_PRIVATE_DATA: usize = 5;
const CHILD_SET_PRIVATE_DATA_INTERFACE: usize = 6;
const RES_GET_TYPE: usize = 7;
const RES_SET_EVICTION_PRIORITY: usize = 8;
const RES_GET_EVICTION_PRIORITY: usize = 9;
const TEX_GET_DESC: usize = 10;
/// ID3D11DeviceContext's, where Direct3D 11 keeps Map and Unmap.
const CTX_MAP: usize = 14;
const CTX_UNMAP: usize = 15;

/// D3D11_MAPPED_SUBRESOURCE: what Direct3D 11 answers Map with.
#[repr(C)]
struct Mapped11 {
    data: *mut c_void,
    row: u32,
    depth: u32,
}

/// D3D10_MAPPED_TEXTURE2D: what Direct3D 10 answers Map with -- the
/// same without the depth pitch, which a 2D texture has none of.
#[repr(C)]
struct Mapped10 {
    data: *mut c_void,
    row: u32,
}

/// A Direct3D 10 texture of this DLL.
#[repr(C)]
pub struct Texture {
    hdr: Header,
    /// The device it came from (ID3D10DeviceChild::GetDevice).
    device: ComRef,
    /// That device's Direct3D 11 immediate context (Map, Unmap).
    ctx: ComRef,
    /// The Direct3D 11 texture itself.
    tex: ComRef,
}

/// The texture behind one of our object pointers.
fn tex(this: Obj) -> &'static Texture {
    // SAFETY: one of our texture objects.
    unsafe { &*(this as *const Texture) }
}

/// Wraps the Direct3D 11 texture `real` (a reference adopted) as a
/// Direct3D 10 texture of `device`.
pub fn wrap(device: Obj, real: Obj, out: *mut Obj) -> HResult {
    let t = Texture {
        hdr: Header::new(TEXTURE_VTBL.addr()),
        device: ComRef::new(device),
        ctx: ComRef::new(crate::device::context_of(device)),
        tex: ComRef::owned(real),
    };
    give(out, t)
}

/// The Direct3D 11 texture behind `o`, when `o` is one of ours (None
/// for anything else, which this DLL cannot reach into).
pub fn real(o: Obj) -> Option<Obj> {
    if vtable_of(o) == TEXTURE_VTBL.addr() { Some(tex(o).tex.0) } else { None }
}

extern "system" fn t_QueryInterface(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let Some(iid) = read_guid(riid) else {
        put(out, null_mut());
        return E_INVALIDARG;
    };
    if iid == IID_IUNKNOWN || iid == IID_TEXTURE2D || iid == IID_RESOURCE || iid == IID_DEVICE_CHILD
    {
        add_ref(this);
        put(out, this);
        return S_OK;
    }
    // The surface, the Direct3D 11 texture, the shared handle: the real
    // texture answers them, and that is what a program (or gala's
    // Direct2D) needs to draw into it.
    let real = tex(this).tex.0;
    // SAFETY: IUnknown::QueryInterface's signature.
    let f: extern "system" fn(Obj, *const Guid, *mut Obj) -> HResult =
        unsafe { core::mem::transmute(slot(real, 0)) };
    let r = f(real, riid, out);
    if r < 0 {
        gap(b"ID3D10Texture2D QueryInterface", crate::data1(&iid));
    }
    r
}

extern "system" fn t_Release(this: Obj) -> u32 {
    release_object::<Texture>(this)
}

extern "system" fn t_GetDevice(this: Obj, out: *mut Obj) {
    let device = tex(this).device.0;
    add_ref(device);
    put(out, device);
}

extern "system" fn t_GetPrivateData(
    this: Obj,
    guid: *const Guid,
    size: *mut u32,
    data: *mut c_void,
) -> HResult {
    let real = tex(this).tex.0;
    // SAFETY: ID3D11DeviceChild::GetPrivateData's signature.
    let f: extern "system" fn(Obj, *const Guid, *mut u32, *mut c_void) -> HResult =
        unsafe { core::mem::transmute(slot(real, CHILD_GET_PRIVATE_DATA)) };
    f(real, guid, size, data)
}

extern "system" fn t_SetPrivateData(
    this: Obj,
    guid: *const Guid,
    size: u32,
    data: *const c_void,
) -> HResult {
    let real = tex(this).tex.0;
    // SAFETY: ID3D11DeviceChild::SetPrivateData's signature.
    let f: extern "system" fn(Obj, *const Guid, u32, *const c_void) -> HResult =
        unsafe { core::mem::transmute(slot(real, CHILD_SET_PRIVATE_DATA)) };
    f(real, guid, size, data)
}

extern "system" fn t_SetPrivateDataInterface(this: Obj, guid: *const Guid, data: Obj) -> HResult {
    let real = tex(this).tex.0;
    // SAFETY: ID3D11DeviceChild::SetPrivateDataInterface's signature.
    let f: extern "system" fn(Obj, *const Guid, Obj) -> HResult =
        unsafe { core::mem::transmute(slot(real, CHILD_SET_PRIVATE_DATA_INTERFACE)) };
    f(real, guid, data)
}

/// ID3D10Resource::GetType. A D3D10_RESOURCE_DIMENSION is the
/// D3D11_RESOURCE_DIMENSION of the same name (TEXTURE2D is 3 in both),
/// so what Direct3D 11 answers is the answer.
extern "system" fn t_GetType(this: Obj, out: *mut u32) {
    let real = tex(this).tex.0;
    // SAFETY: ID3D11Resource::GetType's signature.
    let f: extern "system" fn(Obj, *mut u32) =
        unsafe { core::mem::transmute(slot(real, RES_GET_TYPE)) };
    f(real, out);
}

/// ID3D10Resource::SetEvictionPriority. A D3D10_RESOURCE_PRIORITY is
/// the D3D11_RESOURCE_PRIORITY of the same name.
extern "system" fn t_SetEvictionPriority(this: Obj, priority: u32) {
    let real = tex(this).tex.0;
    // SAFETY: ID3D11Resource::SetEvictionPriority's signature.
    let f: extern "system" fn(Obj, u32) =
        unsafe { core::mem::transmute(slot(real, RES_SET_EVICTION_PRIORITY)) };
    f(real, priority);
}

extern "system" fn t_GetEvictionPriority(this: Obj) -> u32 {
    let real = tex(this).tex.0;
    // SAFETY: ID3D11Resource::GetEvictionPriority's signature.
    let f: extern "system" fn(Obj) -> u32 =
        unsafe { core::mem::transmute(slot(real, RES_GET_EVICTION_PRIORITY)) };
    f(real)
}

/// ID3D10Texture2D::Map, which is a Direct3D 11 context's call. A
/// D3D10_MAP is the D3D11_MAP of the same name (READ is 1 in both).
extern "system" fn t_Map(
    this: Obj,
    subresource: u32,
    kind: u32,
    flags: u32,
    out: *mut Mapped10,
) -> HResult {
    let t = tex(this);
    // SAFETY: ID3D11DeviceContext::Map's signature.
    let f: extern "system" fn(Obj, Obj, u32, u32, u32, *mut Mapped11) -> HResult =
        unsafe { core::mem::transmute(slot(t.ctx.0, CTX_MAP)) };
    let mut m = Mapped11 { data: null_mut(), row: 0, depth: 0 };
    let hr = f(t.ctx.0, t.tex.0, subresource, kind, flags, &mut m);
    if hr < 0 {
        return hr;
    }
    put(out, Mapped10 { data: m.data, row: m.row });
    hr
}

extern "system" fn t_Unmap(this: Obj, subresource: u32) {
    let t = tex(this);
    // SAFETY: ID3D11DeviceContext::Unmap's signature.
    let f: extern "system" fn(Obj, Obj, u32) =
        unsafe { core::mem::transmute(slot(t.ctx.0, CTX_UNMAP)) };
    f(t.ctx.0, t.tex.0, subresource);
}

/// ID3D10Texture2D::GetDesc. A D3D10_TEXTURE2D_DESC has the fields, the
/// order and the values of a D3D11_TEXTURE2D_DESC (d3d10.h, d3d11.h),
/// so Direct3D 11 fills the caller's own structure.
extern "system" fn t_GetDesc(this: Obj, out: *mut c_void) {
    let real = tex(this).tex.0;
    // SAFETY: ID3D11Texture2D::GetDesc's signature.
    let f: extern "system" fn(Obj, *mut c_void) =
        unsafe { core::mem::transmute(slot(real, TEX_GET_DESC)) };
    f(real, out);
}

// The interfaces this module's vtable slots belong to, which name the
// wrappers `cargo xtask gen-trace` generates (xtask/src/gentrace.rs):
// one object answers ID3D10Texture2D and the two interfaces it
// inherits, and each slot is named by the one that declares it (the
// doc comment of each method says which).
// gen-trace prefix t_ ID3D10Texture2D
// gen-trace fn t_GetDevice ID3D10DeviceChild::GetDevice
// gen-trace fn t_GetPrivateData ID3D10DeviceChild::GetPrivateData
// gen-trace fn t_SetPrivateData ID3D10DeviceChild::SetPrivateData
// gen-trace fn t_SetPrivateDataInterface ID3D10DeviceChild::SetPrivateDataInterface
// gen-trace fn t_GetType ID3D10Resource::GetType
// gen-trace fn t_SetEvictionPriority ID3D10Resource::SetEvictionPriority
// gen-trace fn t_GetEvictionPriority ID3D10Resource::GetEvictionPriority
static TEXTURE_VTBL: VTable<13> = VTable([
    t_QueryInterface as *const (),                  // 0 QueryInterface
    add_ref as *const (),                           // 1 AddRef
    t_Release as *const (),                         // 2 Release
    traced::t_GetDevice as *const (),               // 3 GetDevice
    traced::t_GetPrivateData as *const (),          // 4 GetPrivateData
    traced::t_SetPrivateData as *const (),          // 5 SetPrivateData
    traced::t_SetPrivateDataInterface as *const (), // 6 SetPrivateDataInterface
    traced::t_GetType as *const (),                 // 7 GetType
    traced::t_SetEvictionPriority as *const (),     // 8 SetEvictionPriority
    traced::t_GetEvictionPriority as *const (),     // 9 GetEvictionPriority
    traced::t_Map as *const (),                     // 10 Map
    traced::t_Unmap as *const (),                   // 11 Unmap
    traced::t_GetDesc as *const (),                 // 12 GetDesc
]);

// ==== generated by `cargo xtask gen-trace`: do not edit ====
/// The wrapper of every vtable slot of this module: each records
/// the first call that reaches its method, and every call at all
/// once `trace_from=` has armed one (`cargo xtask gen-trace`).
pub(crate) mod traced {
    use super::*;
    /// ID3D10Texture2D::GetDesc, traced.
    pub(super) extern "system" fn t_GetDesc(this: Obj, out: *mut c_void) {
        crate::traced!(b"ID3D10Texture2D::GetDesc", [this, out]);
        super::t_GetDesc(this, out)
    }
    /// ID3D10DeviceChild::GetDevice, traced.
    pub(super) extern "system" fn t_GetDevice(this: Obj, out: *mut Obj) {
        crate::traced!(b"ID3D10DeviceChild::GetDevice", [this, out]);
        super::t_GetDevice(this, out)
    }
    /// ID3D10Resource::GetEvictionPriority, traced.
    pub(super) extern "system" fn t_GetEvictionPriority(this: Obj) -> u32 {
        crate::traced!(b"ID3D10Resource::GetEvictionPriority", [this]);
        let answer = super::t_GetEvictionPriority(this);
        crate::answered_call!(b"ID3D10Resource::GetEvictionPriority", answer);
        answer
    }
    /// ID3D10DeviceChild::GetPrivateData, traced.
    pub(super) extern "system" fn t_GetPrivateData(
        this: Obj,
        guid: *const Guid,
        size: *mut u32,
        data: *mut c_void,
    ) -> HResult {
        crate::traced!(b"ID3D10DeviceChild::GetPrivateData", [this, guid, size, data]);
        let answer = super::t_GetPrivateData(this, guid, size, data);
        crate::answered_call!(b"ID3D10DeviceChild::GetPrivateData", answer);
        answer
    }
    /// ID3D10Resource::GetType, traced.
    pub(super) extern "system" fn t_GetType(this: Obj, out: *mut u32) {
        crate::traced!(b"ID3D10Resource::GetType", [this, out]);
        super::t_GetType(this, out)
    }
    /// ID3D10Texture2D::Map, traced.
    pub(super) extern "system" fn t_Map(
        this: Obj,
        subresource: u32,
        kind: u32,
        flags: u32,
        out: *mut Mapped10,
    ) -> HResult {
        crate::traced!(b"ID3D10Texture2D::Map", [this, subresource, kind, flags, out]);
        let answer = super::t_Map(this, subresource, kind, flags, out);
        crate::answered_call!(b"ID3D10Texture2D::Map", answer);
        answer
    }
    /// ID3D10Resource::SetEvictionPriority, traced.
    pub(super) extern "system" fn t_SetEvictionPriority(this: Obj, priority: u32) {
        crate::traced!(b"ID3D10Resource::SetEvictionPriority", [this, priority]);
        super::t_SetEvictionPriority(this, priority)
    }
    /// ID3D10DeviceChild::SetPrivateData, traced.
    pub(super) extern "system" fn t_SetPrivateData(
        this: Obj,
        guid: *const Guid,
        size: u32,
        data: *const c_void,
    ) -> HResult {
        crate::traced!(b"ID3D10DeviceChild::SetPrivateData", [this, guid, size, data]);
        let answer = super::t_SetPrivateData(this, guid, size, data);
        crate::answered_call!(b"ID3D10DeviceChild::SetPrivateData", answer);
        answer
    }
    /// ID3D10DeviceChild::SetPrivateDataInterface, traced.
    pub(super) extern "system" fn t_SetPrivateDataInterface(
        this: Obj,
        guid: *const Guid,
        data: Obj,
    ) -> HResult {
        crate::traced!(b"ID3D10DeviceChild::SetPrivateDataInterface", [this, guid, data]);
        let answer = super::t_SetPrivateDataInterface(this, guid, data);
        crate::answered_call!(b"ID3D10DeviceChild::SetPrivateDataInterface", answer);
        answer
    }
    /// ID3D10Texture2D::Unmap, traced.
    pub(super) extern "system" fn t_Unmap(this: Obj, subresource: u32) {
        crate::traced!(b"ID3D10Texture2D::Unmap", [this, subresource]);
        super::t_Unmap(this, subresource)
    }
}
