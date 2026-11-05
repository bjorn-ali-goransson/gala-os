//! gala's d3d11.dll: no adapter yet (ROADMAP GPU G0; `../../gpu_common.rs`
//! says how the stubs fail).
#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

include!("../../gpu_common.rs");

/// D3D11CreateDevice(pAdapter, DriverType, Software, Flags,
/// pFeatureLevels, FeatureLevels, SDKVersion, ppDevice, pFeatureLevel,
/// ppImmediateContext): no driver type is available.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn D3D11CreateDevice(
    _adapter: *mut c_void,
    _driver: u32,
    _software: *mut c_void,
    _flags: u32,
    _levels: *const u32,
    _count: u32,
    _sdk: u32,
    device: *mut *mut c_void,
    level: *mut u32,
    context: *mut *mut c_void,
) -> i32 {
    clear(device);
    clear(context);
    if !level.is_null() {
        // SAFETY: the caller's output.
        unsafe { *level = 0 };
    }
    unsupported(b"d3d11.dll!D3D11CreateDevice");
    DXGI_ERROR_UNSUPPORTED
}

/// D3D11CreateDeviceAndSwapChain: D3D11CreateDevice plus a swap chain.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn D3D11CreateDeviceAndSwapChain(
    _adapter: *mut c_void,
    _driver: u32,
    _software: *mut c_void,
    _flags: u32,
    _levels: *const u32,
    _count: u32,
    _sdk: u32,
    _desc: *const c_void,
    swap_chain: *mut *mut c_void,
    device: *mut *mut c_void,
    level: *mut u32,
    context: *mut *mut c_void,
) -> i32 {
    clear(swap_chain);
    clear(device);
    clear(context);
    if !level.is_null() {
        // SAFETY: the caller's output.
        unsafe { *level = 0 };
    }
    unsupported(b"d3d11.dll!D3D11CreateDeviceAndSwapChain");
    DXGI_ERROR_UNSUPPORTED
}
