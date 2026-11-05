//! gala's d3d12.dll: no adapter yet (ROADMAP GPU G0; `../../gpu_common.rs`
//! says how the stubs fail). chrome.dll imports D3D12CreateDevice by
//! ordinal (101, pinned in build.rs with D3D12GetDebugInterface's 102, as
//! Wine's d3d12.spec numbers them).
#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

include!("../../gpu_common.rs");

/// D3D12CreateDevice(pAdapter, MinimumFeatureLevel, riid, ppDevice).
#[unsafe(no_mangle)]
pub extern "system" fn D3D12CreateDevice(
    _adapter: *mut c_void,
    _level: u32,
    _iid: *const u8,
    device: *mut *mut c_void,
) -> i32 {
    clear(device);
    unsupported(b"d3d12.dll!D3D12CreateDevice");
    DXGI_ERROR_UNSUPPORTED
}

/// D3D12GetDebugInterface(riid, ppvDebug): no debug layer.
#[unsafe(no_mangle)]
pub extern "system" fn D3D12GetDebugInterface(_iid: *const u8, debug: *mut *mut c_void) -> i32 {
    clear(debug);
    unsupported(b"d3d12.dll!D3D12GetDebugInterface");
    DXGI_ERROR_UNSUPPORTED
}

/// D3D12EnableExperimentalFeatures(NumFeatures, pIIDs, pConfigurationStructs,
/// pConfigurationStructSizes).
#[unsafe(no_mangle)]
pub extern "system" fn D3D12EnableExperimentalFeatures(
    _count: u32,
    _iids: *const u8,
    _configs: *mut c_void,
    _sizes: *const u32,
) -> i32 {
    unsupported(b"d3d12.dll!D3D12EnableExperimentalFeatures");
    DXGI_ERROR_UNSUPPORTED
}

/// D3D12GetInterface(rclsid, riid, ppvDebug).
#[unsafe(no_mangle)]
pub extern "system" fn D3D12GetInterface(
    _clsid: *const u8,
    _iid: *const u8,
    out: *mut *mut c_void,
) -> i32 {
    clear(out);
    unsupported(b"d3d12.dll!D3D12GetInterface");
    DXGI_ERROR_UNSUPPORTED
}
