//! The Direct3D 10.1 device: the program's own Direct3D 11 device and
//! immediate context underneath, and the Direct3D 10 interfaces over
//! them (ARCHITECTURE "Direct3D 10.1").

use core::ffi::c_void;
use core::ptr::null_mut;

use crate::{
    ComRef, DXGI_ERROR_UNSUPPORTED, E_INVALIDARG, E_POINTER, Guid, HResult, Header, IID_DEVICE,
    IID_DEVICE1, IID_IUNKNOWN, Obj, S_OK, VTable, add_ref, answered, data1, gap, give, put,
    read_guid, release_object, slot,
};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

/// "d3d11.dll": the Direct3D 11 the program brought (DXVK beside the
/// program on gala, GPU G2). The loader finds it where the program would
/// -- its own directory before System32 -- and the program has it loaded
/// already, so this only takes a reference.
static D3D11_DLL: [u16; 10] = [
    b'd' as u16,
    b'3' as u16,
    b'd' as u16,
    b'1' as u16,
    b'1' as u16,
    b'.' as u16,
    b'd' as u16,
    b'l' as u16,
    b'l' as u16,
    0,
];

/// D3D11_SDK_VERSION (d3d11.h): what D3D11CreateDevice is called with.
const D3D11_SDK_VERSION: u32 = 7;

/// The D3D11_CREATE_DEVICE_FLAGs a D3D10_CREATE_DEVICE_FLAG of the same
/// name has the same value as: SINGLETHREADED, DEBUG and BGRA_SUPPORT.
const FLAGS_IN_BOTH: u32 = 0x1 | 0x2 | 0x20;

/// ID3D11Device's and ID3D11DeviceContext's slots, counted from IUnknown
/// as d3d11.h declares them (WORKFLOW "A vtable slot is counted from
/// IUnknown").
const DEV_CREATE_TEXTURE2D: usize = 5;
const DEV_CHECK_FORMAT_SUPPORT: usize = 29;
const DEV_CHECK_MULTISAMPLE: usize = 30;
const DEV_GET_PRIVATE_DATA: usize = 34;
const DEV_SET_PRIVATE_DATA: usize = 35;
const DEV_SET_PRIVATE_DATA_INTERFACE: usize = 36;
const DEV_GET_DEVICE_REMOVED_REASON: usize = 39;
const CTX_COPY_RESOURCE: usize = 47;
const CTX_FLUSH: usize = 111;

/// A Direct3D 10.1 device of this DLL.
#[repr(C)]
pub struct Device {
    hdr: Header,
    /// The program's ID3D11Device.
    pub d3d11: ComRef,
    /// Its immediate context: where Direct3D 10's device methods that
    /// belong to a Direct3D 11 context (CopyResource, Map) go.
    pub ctx: ComRef,
    /// What it was created with (GetCreationFlags, GetFeatureLevel).
    flags: u32,
    level: u32,
}

/// The device behind one of our object pointers.
fn dev(this: Obj) -> &'static Device {
    // SAFETY: one of our device objects.
    unsafe { &*(this as *const Device) }
}

/// The Direct3D 11 immediate context of device `this` (what its
/// textures take Map and Unmap to).
pub fn context_of(this: Obj) -> Obj {
    dev(this).ctx.0
}

/// D3D10CreateDevice1: a Direct3D 10.1 device over the program's own
/// Direct3D 11.
///
/// The driver type, the flags and the feature level asked for are in the
/// ledger as the detail of the call (level << 32 | flags << 8 | driver),
/// because which of them a program asks for is what says why it got no
/// device (OBSERVABILITY "Missing-features ledger").
pub fn create(
    adapter: Obj,
    driver: u32,
    software: Obj,
    flags: u32,
    level: u32,
    out: *mut Obj,
) -> HResult {
    put(out, null_mut());
    let detail = u64::from(level) << 32 | u64::from(flags) << 8 | u64::from(driver & 0xff);
    // A Direct3D 11 driver type of the same name has a different value
    // (UNKNOWN is 0 there, HARDWARE 0 here); with an adapter the type
    // must be UNKNOWN, as Direct3D 11 requires.
    let kind = match (adapter.is_null(), driver) {
        (false, _) => 0,
        (true, 0) => 1,
        (true, 1) => 2,
        (true, 2) => 3,
        (true, 3) => 4,
        (true, 5) => 5,
        _ => {
            gap(b"D3D10CreateDevice1 driver type", detail);
            return E_INVALIDARG;
        }
    };
    if level == 0 {
        gap(b"D3D10CreateDevice1 feature level", detail);
        return E_INVALIDARG;
    }
    if flags & !FLAGS_IN_BOTH != 0 {
        gap(b"D3D10CreateDevice1 flags", u64::from(flags & !FLAGS_IN_BOTH));
    }

    // SAFETY: a NUL-terminated wide name; the module stays loaded.
    let module = unsafe { LoadLibraryW(D3D11_DLL.as_ptr()) };
    if module.is_null() {
        answered(b"D3D10CreateDevice1 (no d3d11.dll)", DXGI_ERROR_UNSUPPORTED);
        return DXGI_ERROR_UNSUPPORTED;
    }
    // SAFETY: a NUL-terminated symbol name and a loaded module.
    let entry = unsafe { GetProcAddress(module, c"D3D11CreateDevice".as_ptr().cast()) };
    if entry.is_null() {
        answered(b"D3D10CreateDevice1 (no D3D11CreateDevice)", DXGI_ERROR_UNSUPPORTED);
        return DXGI_ERROR_UNSUPPORTED;
    }
    type CreateD3D11 = extern "system" fn(
        Obj,
        u32,
        Obj,
        u32,
        *const u32,
        u32,
        u32,
        *mut Obj,
        *mut u32,
        *mut Obj,
    ) -> HResult;
    // SAFETY: D3D11CreateDevice's signature (d3d11.h).
    let create_d3d11: CreateD3D11 = unsafe { core::mem::transmute(entry) };

    // A Direct3D 10 feature level has the value its Direct3D 11 name
    // has (D3D10_FEATURE_LEVEL_10_1 and D3D_FEATURE_LEVEL_10_1 are both
    // 0xa100), so the level asked for is the one asked of Direct3D 11:
    // a device of a later level would answer calls this one must not.
    let levels = [level];
    let mut d3d11 = null_mut();
    let mut got = 0u32;
    let mut ctx = null_mut();
    let hr = create_d3d11(
        adapter,
        kind,
        software,
        (flags & FLAGS_IN_BOTH) | 0x20,
        levels.as_ptr(),
        1,
        D3D11_SDK_VERSION,
        &mut d3d11,
        &mut got,
        &mut ctx,
    );
    if hr < 0 || d3d11.is_null() || ctx.is_null() {
        answered(b"D3D10CreateDevice1 (Direct3D 11 refused)", hr);
        return hr;
    }
    let device = Device {
        hdr: Header::new(DEVICE_VTBL.addr()),
        d3d11: ComRef::owned(d3d11),
        ctx: ComRef::owned(ctx),
        flags,
        level: got,
    };
    let r = give(out, device);
    answered(b"D3D10CreateDevice1", r);
    r
}

extern "system" fn d_QueryInterface(this: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let Some(iid) = read_guid(riid) else {
        put(out, null_mut());
        return E_INVALIDARG;
    };
    if iid == IID_IUNKNOWN || iid == IID_DEVICE || iid == IID_DEVICE1 {
        add_ref(this);
        put(out, this);
        return S_OK;
    }
    // Everything else of this device is the Direct3D 11 device's: its
    // IDXGIDevice (the adapter behind it), its own interfaces, the
    // debug ones. A program that asks for one gets the real object,
    // which is also how gala's Direct2D reaches the surface underneath
    // a texture of ours (`texture`).
    let r = query_through(dev(this).d3d11.0, riid, out);
    if r < 0 {
        gap(b"ID3D10Device1 QueryInterface", data1(&iid));
    }
    r
}

/// IUnknown::QueryInterface of another COM object.
fn query_through(o: Obj, riid: *const Guid, out: *mut Obj) -> HResult {
    // SAFETY: IUnknown::QueryInterface's signature.
    let f: extern "system" fn(Obj, *const Guid, *mut Obj) -> HResult =
        unsafe { core::mem::transmute(slot(o, 0)) };
    f(o, riid, out)
}

extern "system" fn d_Release(this: Obj) -> u32 {
    release_object::<Device>(this)
}

/// ID3D10Device::CreateTexture2D. A D3D10_TEXTURE2D_DESC and a
/// D3D10_SUBRESOURCE_DATA have the fields, the order and the values of
/// the Direct3D 11 structures of the same names (d3d10.h, d3d11.h), so
/// the caller's own structures go through as they are.
extern "system" fn d_CreateTexture2D(
    this: Obj,
    desc: *const c_void,
    data: *const c_void,
    out: *mut Obj,
) -> HResult {
    put(out, null_mut());
    let d = dev(this);
    // SAFETY: ID3D11Device::CreateTexture2D's signature.
    let f: extern "system" fn(Obj, *const c_void, *const c_void, *mut Obj) -> HResult =
        unsafe { core::mem::transmute(slot(d.d3d11.0, DEV_CREATE_TEXTURE2D)) };
    let mut tex = null_mut();
    let hr = f(d.d3d11.0, desc, data, &mut tex);
    if hr < 0 || tex.is_null() {
        answered(b"ID3D10Device::CreateTexture2D", hr);
        return hr;
    }
    let r = crate::texture::wrap(this, tex, out);
    answered(b"ID3D10Device::CreateTexture2D", r);
    r
}

/// ID3D10Device::CopyResource, which is a Direct3D 11 context's call.
/// A resource that did not come from this device is recorded and
/// nothing is copied (the method answers nothing).
extern "system" fn d_CopyResource(this: Obj, dst: Obj, src: Obj) {
    let d = dev(this);
    let (Some(to), Some(from)) = (crate::texture::real(dst), crate::texture::real(src)) else {
        gap(b"ID3D10Device::CopyResource of a foreign resource", 0);
        return;
    };
    // SAFETY: ID3D11DeviceContext::CopyResource's signature.
    let f: extern "system" fn(Obj, Obj, Obj) =
        unsafe { core::mem::transmute(slot(d.ctx.0, CTX_COPY_RESOURCE)) };
    f(d.ctx.0, to, from);
    answered(b"ID3D10Device::CopyResource", S_OK);
}

extern "system" fn d_Flush(this: Obj) {
    let d = dev(this);
    // SAFETY: ID3D11DeviceContext::Flush's signature.
    let f: extern "system" fn(Obj) = unsafe { core::mem::transmute(slot(d.ctx.0, CTX_FLUSH)) };
    f(d.ctx.0);
}

extern "system" fn d_GetFeatureLevel(this: Obj) -> u32 {
    dev(this).level
}

extern "system" fn d_GetCreationFlags(this: Obj) -> u32 {
    dev(this).flags
}

extern "system" fn d_GetDeviceRemovedReason(this: Obj) -> HResult {
    let d = dev(this);
    // SAFETY: ID3D11Device::GetDeviceRemovedReason's signature.
    let f: extern "system" fn(Obj) -> HResult =
        unsafe { core::mem::transmute(slot(d.d3d11.0, DEV_GET_DEVICE_REMOVED_REASON)) };
    f(d.d3d11.0)
}

extern "system" fn d_GetPrivateData(
    this: Obj,
    guid: *const Guid,
    size: *mut u32,
    data: *mut c_void,
) -> HResult {
    let d = dev(this);
    // SAFETY: ID3D11Device::GetPrivateData's signature.
    let f: extern "system" fn(Obj, *const Guid, *mut u32, *mut c_void) -> HResult =
        unsafe { core::mem::transmute(slot(d.d3d11.0, DEV_GET_PRIVATE_DATA)) };
    f(d.d3d11.0, guid, size, data)
}

extern "system" fn d_SetPrivateData(
    this: Obj,
    guid: *const Guid,
    size: u32,
    data: *const c_void,
) -> HResult {
    let d = dev(this);
    // SAFETY: ID3D11Device::SetPrivateData's signature.
    let f: extern "system" fn(Obj, *const Guid, u32, *const c_void) -> HResult =
        unsafe { core::mem::transmute(slot(d.d3d11.0, DEV_SET_PRIVATE_DATA)) };
    f(d.d3d11.0, guid, size, data)
}

extern "system" fn d_SetPrivateDataInterface(this: Obj, guid: *const Guid, data: Obj) -> HResult {
    let d = dev(this);
    // SAFETY: ID3D11Device::SetPrivateDataInterface's signature.
    let f: extern "system" fn(Obj, *const Guid, Obj) -> HResult =
        unsafe { core::mem::transmute(slot(d.d3d11.0, DEV_SET_PRIVATE_DATA_INTERFACE)) };
    f(d.d3d11.0, guid, data)
}

/// ID3D10Device::CheckFormatSupport. A D3D10_FORMAT_SUPPORT bit is the
/// D3D11_FORMAT_SUPPORT bit of the same name (d3d10.h, d3d11.h), and
/// Direct3D 11 reports what the device it created can do, which is what
/// this device can do.
extern "system" fn d_CheckFormatSupport(this: Obj, format: u32, support: *mut u32) -> HResult {
    let d = dev(this);
    // SAFETY: ID3D11Device::CheckFormatSupport's signature.
    let f: extern "system" fn(Obj, u32, *mut u32) -> HResult =
        unsafe { core::mem::transmute(slot(d.d3d11.0, DEV_CHECK_FORMAT_SUPPORT)) };
    f(d.d3d11.0, format, support)
}

extern "system" fn d_CheckMultisampleQualityLevels(
    this: Obj,
    format: u32,
    count: u32,
    levels: *mut u32,
) -> HResult {
    let d = dev(this);
    // SAFETY: ID3D11Device::CheckMultisampleQualityLevels's signature.
    let f: extern "system" fn(Obj, u32, u32, *mut u32) -> HResult =
        unsafe { core::mem::transmute(slot(d.d3d11.0, DEV_CHECK_MULTISAMPLE)) };
    f(d.d3d11.0, format, count, levels)
}

/// The methods this device does not answer: each records its own name,
/// so one run of a program names what it asked for (OBSERVABILITY
/// "Missing-features ledger"). The ABI lets a method of any signature
/// be called through one of these: the arguments it was given stay in
/// the caller's own frame, and a method whose answer is nothing or a
/// number has its answer ignored or reads as a failure.
///
/// A stub reports its call to an armed trace as well
/// (`traced_recorded!`, OBSERVABILITY "Call tracing"), so a run shows
/// in what order a program asked for these and not only how often:
/// a method added here is traced by having been added, which is why
/// the generator leaves these slots alone.
macro_rules! recorders {
    ($($name:ident),* $(,)?) => {
        $(
            pub extern "system" fn $name(_this: crate::Obj) -> crate::HResult {
                const NAME: &[u8] = concat!("ID3D10Device1::", stringify!($name)).as_bytes();
                crate::traced_recorded!(NAME, [_this]);
                let answer = crate::not_implemented(NAME);
                crate::answered_call!(NAME, answer);
                answer
            }
        )*
    };
}

// The slots of this module record every call of their own
// (`recorders!` above), so `cargo xtask gen-trace` has nothing to
// add to them and does not wrap them.
// gen-trace records rec
mod rec {
    recorders!(
        VSSetConstantBuffers,
        PSSetShaderResources,
        PSSetShader,
        PSSetSamplers,
        VSSetShader,
        DrawIndexed,
        Draw,
        PSSetConstantBuffers,
        IASetInputLayout,
        IASetVertexBuffers,
        IASetIndexBuffer,
        DrawIndexedInstanced,
        DrawInstanced,
        GSSetConstantBuffers,
        GSSetShader,
        IASetPrimitiveTopology,
        VSSetShaderResources,
        VSSetSamplers,
        SetPredication,
        GSSetShaderResources,
        GSSetSamplers,
        OMSetRenderTargets,
        OMSetBlendState,
        OMSetDepthStencilState,
        SOSetTargets,
        DrawAuto,
        RSSetState,
        RSSetViewports,
        RSSetScissorRects,
        CopySubresourceRegion,
        UpdateSubresource,
        ClearRenderTargetView,
        ClearDepthStencilView,
        GenerateMips,
        ResolveSubresource,
        VSGetConstantBuffers,
        PSGetShaderResources,
        PSGetShader,
        PSGetSamplers,
        VSGetShader,
        PSGetConstantBuffers,
        IAGetInputLayout,
        IAGetVertexBuffers,
        IAGetIndexBuffer,
        GSGetConstantBuffers,
        GSGetShader,
        IAGetPrimitiveTopology,
        VSGetShaderResources,
        VSGetSamplers,
        GetPredication,
        GSGetShaderResources,
        GSGetSamplers,
        OMGetRenderTargets,
        OMGetBlendState,
        OMGetDepthStencilState,
        SOGetTargets,
        RSGetState,
        RSGetViewports,
        RSGetScissorRects,
        SetExceptionMode,
        GetExceptionMode,
        ClearState,
        CreateBuffer,
        CreateTexture1D,
        CreateTexture3D,
        CreateShaderResourceView,
        CreateRenderTargetView,
        CreateDepthStencilView,
        CreateInputLayout,
        CreateVertexShader,
        CreateGeometryShader,
        CreateGeometryShaderWithStreamOutput,
        CreatePixelShader,
        CreateBlendState,
        CreateDepthStencilState,
        CreateRasterizerState,
        CreateSamplerState,
        CreateQuery,
        CreatePredicate,
        CreateCounter,
        CheckCounterInfo,
        CheckCounter,
        OpenSharedResource,
        SetTextFilterSize,
        GetTextFilterSize,
        CreateShaderResourceView1,
        CreateBlendState1
    );
}

// The interfaces this module's vtable slots belong to, which name the
// wrappers `cargo xtask gen-trace` generates (xtask/src/gentrace.rs).
// One object answers ID3D10Device and ID3D10Device1 alike, so each
// slot is named by the interface that declares it, the way the
// ledger's own rows of these methods are spelled -- which is what a
// `trace_from=` value is read off.
// gen-trace prefix d_ ID3D10Device
// gen-trace fn d_GetFeatureLevel ID3D10Device1::GetFeatureLevel
static DEVICE_VTBL: VTable<101> = VTable([
    d_QueryInterface as *const (),                  // 0 QueryInterface
    crate::add_ref as *const (),                    // 1 AddRef
    d_Release as *const (),                         // 2 Release
    rec::VSSetConstantBuffers as *const (),         // 3 VSSetConstantBuffers
    rec::PSSetShaderResources as *const (),         // 4 PSSetShaderResources
    rec::PSSetShader as *const (),                  // 5 PSSetShader
    rec::PSSetSamplers as *const (),                // 6 PSSetSamplers
    rec::VSSetShader as *const (),                  // 7 VSSetShader
    rec::DrawIndexed as *const (),                  // 8 DrawIndexed
    rec::Draw as *const (),                         // 9 Draw
    rec::PSSetConstantBuffers as *const (),         // 10 PSSetConstantBuffers
    rec::IASetInputLayout as *const (),             // 11 IASetInputLayout
    rec::IASetVertexBuffers as *const (),           // 12 IASetVertexBuffers
    rec::IASetIndexBuffer as *const (),             // 13 IASetIndexBuffer
    rec::DrawIndexedInstanced as *const (),         // 14 DrawIndexedInstanced
    rec::DrawInstanced as *const (),                // 15 DrawInstanced
    rec::GSSetConstantBuffers as *const (),         // 16 GSSetConstantBuffers
    rec::GSSetShader as *const (),                  // 17 GSSetShader
    rec::IASetPrimitiveTopology as *const (),       // 18 IASetPrimitiveTopology
    rec::VSSetShaderResources as *const (),         // 19 VSSetShaderResources
    rec::VSSetSamplers as *const (),                // 20 VSSetSamplers
    rec::SetPredication as *const (),               // 21 SetPredication
    rec::GSSetShaderResources as *const (),         // 22 GSSetShaderResources
    rec::GSSetSamplers as *const (),                // 23 GSSetSamplers
    rec::OMSetRenderTargets as *const (),           // 24 OMSetRenderTargets
    rec::OMSetBlendState as *const (),              // 25 OMSetBlendState
    rec::OMSetDepthStencilState as *const (),       // 26 OMSetDepthStencilState
    rec::SOSetTargets as *const (),                 // 27 SOSetTargets
    rec::DrawAuto as *const (),                     // 28 DrawAuto
    rec::RSSetState as *const (),                   // 29 RSSetState
    rec::RSSetViewports as *const (),               // 30 RSSetViewports
    rec::RSSetScissorRects as *const (),            // 31 RSSetScissorRects
    rec::CopySubresourceRegion as *const (),        // 32 CopySubresourceRegion
    traced::d_CopyResource as *const (),            // 33 CopyResource
    rec::UpdateSubresource as *const (),            // 34 UpdateSubresource
    rec::ClearRenderTargetView as *const (),        // 35 ClearRenderTargetView
    rec::ClearDepthStencilView as *const (),        // 36 ClearDepthStencilView
    rec::GenerateMips as *const (),                 // 37 GenerateMips
    rec::ResolveSubresource as *const (),           // 38 ResolveSubresource
    rec::VSGetConstantBuffers as *const (),         // 39 VSGetConstantBuffers
    rec::PSGetShaderResources as *const (),         // 40 PSGetShaderResources
    rec::PSGetShader as *const (),                  // 41 PSGetShader
    rec::PSGetSamplers as *const (),                // 42 PSGetSamplers
    rec::VSGetShader as *const (),                  // 43 VSGetShader
    rec::PSGetConstantBuffers as *const (),         // 44 PSGetConstantBuffers
    rec::IAGetInputLayout as *const (),             // 45 IAGetInputLayout
    rec::IAGetVertexBuffers as *const (),           // 46 IAGetVertexBuffers
    rec::IAGetIndexBuffer as *const (),             // 47 IAGetIndexBuffer
    rec::GSGetConstantBuffers as *const (),         // 48 GSGetConstantBuffers
    rec::GSGetShader as *const (),                  // 49 GSGetShader
    rec::IAGetPrimitiveTopology as *const (),       // 50 IAGetPrimitiveTopology
    rec::VSGetShaderResources as *const (),         // 51 VSGetShaderResources
    rec::VSGetSamplers as *const (),                // 52 VSGetSamplers
    rec::GetPredication as *const (),               // 53 GetPredication
    rec::GSGetShaderResources as *const (),         // 54 GSGetShaderResources
    rec::GSGetSamplers as *const (),                // 55 GSGetSamplers
    rec::OMGetRenderTargets as *const (),           // 56 OMGetRenderTargets
    rec::OMGetBlendState as *const (),              // 57 OMGetBlendState
    rec::OMGetDepthStencilState as *const (),       // 58 OMGetDepthStencilState
    rec::SOGetTargets as *const (),                 // 59 SOGetTargets
    rec::RSGetState as *const (),                   // 60 RSGetState
    rec::RSGetViewports as *const (),               // 61 RSGetViewports
    rec::RSGetScissorRects as *const (),            // 62 RSGetScissorRects
    traced::d_GetDeviceRemovedReason as *const (),  // 63 GetDeviceRemovedReason
    rec::SetExceptionMode as *const (),             // 64 SetExceptionMode
    rec::GetExceptionMode as *const (),             // 65 GetExceptionMode
    traced::d_GetPrivateData as *const (),          // 66 GetPrivateData
    traced::d_SetPrivateData as *const (),          // 67 SetPrivateData
    traced::d_SetPrivateDataInterface as *const (), // 68 SetPrivateDataInterface
    rec::ClearState as *const (),                   // 69 ClearState
    traced::d_Flush as *const (),                   // 70 Flush
    rec::CreateBuffer as *const (),                 // 71 CreateBuffer
    rec::CreateTexture1D as *const (),              // 72 CreateTexture1D
    traced::d_CreateTexture2D as *const (),         // 73 CreateTexture2D
    rec::CreateTexture3D as *const (),              // 74 CreateTexture3D
    rec::CreateShaderResourceView as *const (),     // 75 CreateShaderResourceView
    rec::CreateRenderTargetView as *const (),       // 76 CreateRenderTargetView
    rec::CreateDepthStencilView as *const (),       // 77 CreateDepthStencilView
    rec::CreateInputLayout as *const (),            // 78 CreateInputLayout
    rec::CreateVertexShader as *const (),           // 79 CreateVertexShader
    rec::CreateGeometryShader as *const (),         // 80 CreateGeometryShader
    rec::CreateGeometryShaderWithStreamOutput as *const (), // 81 CreateGeometryShaderWithStreamOutput
    rec::CreatePixelShader as *const (),                    // 82 CreatePixelShader
    rec::CreateBlendState as *const (),                     // 83 CreateBlendState
    rec::CreateDepthStencilState as *const (),              // 84 CreateDepthStencilState
    rec::CreateRasterizerState as *const (),                // 85 CreateRasterizerState
    rec::CreateSamplerState as *const (),                   // 86 CreateSamplerState
    rec::CreateQuery as *const (),                          // 87 CreateQuery
    rec::CreatePredicate as *const (),                      // 88 CreatePredicate
    rec::CreateCounter as *const (),                        // 89 CreateCounter
    traced::d_CheckFormatSupport as *const (),              // 90 CheckFormatSupport
    traced::d_CheckMultisampleQualityLevels as *const (),   // 91 CheckMultisampleQualityLevels
    rec::CheckCounterInfo as *const (),                     // 92 CheckCounterInfo
    rec::CheckCounter as *const (),                         // 93 CheckCounter
    traced::d_GetCreationFlags as *const (),                // 94 GetCreationFlags
    rec::OpenSharedResource as *const (),                   // 95 OpenSharedResource
    rec::SetTextFilterSize as *const (),                    // 96 SetTextFilterSize
    rec::GetTextFilterSize as *const (),                    // 97 GetTextFilterSize
    rec::CreateShaderResourceView1 as *const (),            // 98 CreateShaderResourceView1
    rec::CreateBlendState1 as *const (),                    // 99 CreateBlendState1
    traced::d_GetFeatureLevel as *const (),                 // 100 GetFeatureLevel
]);

// ==== generated by `cargo xtask gen-trace`: do not edit ====
/// The wrapper of every vtable slot of this module: each records
/// the first call that reaches its method, and every call at all
/// once `trace_from=` has armed one (`cargo xtask gen-trace`).
pub(crate) mod traced {
    use super::*;
    /// ID3D10Device::CheckFormatSupport, traced.
    pub(super) extern "system" fn d_CheckFormatSupport(
        this: Obj,
        format: u32,
        support: *mut u32,
    ) -> HResult {
        crate::traced!(b"ID3D10Device::CheckFormatSupport", [this, format, support]);
        let answer = super::d_CheckFormatSupport(this, format, support);
        crate::answered_call!(b"ID3D10Device::CheckFormatSupport", answer);
        answer
    }
    /// ID3D10Device::CheckMultisampleQualityLevels, traced.
    pub(super) extern "system" fn d_CheckMultisampleQualityLevels(
        this: Obj,
        format: u32,
        count: u32,
        levels: *mut u32,
    ) -> HResult {
        crate::traced!(
            b"ID3D10Device::CheckMultisampleQualityLevels",
            [this, format, count, levels]
        );
        let answer = super::d_CheckMultisampleQualityLevels(this, format, count, levels);
        crate::answered_call!(b"ID3D10Device::CheckMultisampleQualityLevels", answer);
        answer
    }
    /// ID3D10Device::CopyResource, traced.
    pub(super) extern "system" fn d_CopyResource(this: Obj, dst: Obj, src: Obj) {
        crate::traced!(b"ID3D10Device::CopyResource", [this, dst, src]);
        super::d_CopyResource(this, dst, src)
    }
    /// ID3D10Device::CreateTexture2D, traced.
    pub(super) extern "system" fn d_CreateTexture2D(
        this: Obj,
        desc: *const c_void,
        data: *const c_void,
        out: *mut Obj,
    ) -> HResult {
        crate::traced!(b"ID3D10Device::CreateTexture2D", [this, desc, data, out]);
        let answer = super::d_CreateTexture2D(this, desc, data, out);
        crate::answered_call!(b"ID3D10Device::CreateTexture2D", answer);
        answer
    }
    /// ID3D10Device::Flush, traced.
    pub(super) extern "system" fn d_Flush(this: Obj) {
        crate::traced!(b"ID3D10Device::Flush", [this]);
        super::d_Flush(this)
    }
    /// ID3D10Device::GetCreationFlags, traced.
    pub(super) extern "system" fn d_GetCreationFlags(this: Obj) -> u32 {
        crate::traced!(b"ID3D10Device::GetCreationFlags", [this]);
        let answer = super::d_GetCreationFlags(this);
        crate::answered_call!(b"ID3D10Device::GetCreationFlags", answer);
        answer
    }
    /// ID3D10Device::GetDeviceRemovedReason, traced.
    pub(super) extern "system" fn d_GetDeviceRemovedReason(this: Obj) -> HResult {
        crate::traced!(b"ID3D10Device::GetDeviceRemovedReason", [this]);
        let answer = super::d_GetDeviceRemovedReason(this);
        crate::answered_call!(b"ID3D10Device::GetDeviceRemovedReason", answer);
        answer
    }
    /// ID3D10Device1::GetFeatureLevel, traced.
    pub(super) extern "system" fn d_GetFeatureLevel(this: Obj) -> u32 {
        crate::traced!(b"ID3D10Device1::GetFeatureLevel", [this]);
        let answer = super::d_GetFeatureLevel(this);
        crate::answered_call!(b"ID3D10Device1::GetFeatureLevel", answer);
        answer
    }
    /// ID3D10Device::GetPrivateData, traced.
    pub(super) extern "system" fn d_GetPrivateData(
        this: Obj,
        guid: *const Guid,
        size: *mut u32,
        data: *mut c_void,
    ) -> HResult {
        crate::traced!(b"ID3D10Device::GetPrivateData", [this, guid, size, data]);
        let answer = super::d_GetPrivateData(this, guid, size, data);
        crate::answered_call!(b"ID3D10Device::GetPrivateData", answer);
        answer
    }
    /// ID3D10Device::SetPrivateData, traced.
    pub(super) extern "system" fn d_SetPrivateData(
        this: Obj,
        guid: *const Guid,
        size: u32,
        data: *const c_void,
    ) -> HResult {
        crate::traced!(b"ID3D10Device::SetPrivateData", [this, guid, size, data]);
        let answer = super::d_SetPrivateData(this, guid, size, data);
        crate::answered_call!(b"ID3D10Device::SetPrivateData", answer);
        answer
    }
    /// ID3D10Device::SetPrivateDataInterface, traced.
    pub(super) extern "system" fn d_SetPrivateDataInterface(
        this: Obj,
        guid: *const Guid,
        data: Obj,
    ) -> HResult {
        crate::traced!(b"ID3D10Device::SetPrivateDataInterface", [this, guid, data]);
        let answer = super::d_SetPrivateDataInterface(this, guid, data);
        crate::answered_call!(b"ID3D10Device::SetPrivateDataInterface", answer);
        answer
    }
}
