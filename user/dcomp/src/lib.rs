//! gala's dcomp.dll (DirectComposition): no adapter yet (ROADMAP GPU G0;
//! `../../gpu_common.rs` says how the stubs fail). Chromium uses
//! DirectComposition only over a Direct3D 11 device, which it does not get.
#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

include!("../../gpu_common.rs");

fn no_device(name: &[u8], out: *mut *mut c_void) -> i32 {
    clear(out);
    unsupported(name);
    DXGI_ERROR_UNSUPPORTED
}

/// DCompositionCreateDevice(dxgiDevice, iid, dcompositionDevice).
#[unsafe(no_mangle)]
pub extern "system" fn DCompositionCreateDevice(
    _dxgi: *mut c_void,
    _iid: *const u8,
    out: *mut *mut c_void,
) -> i32 {
    no_device(b"dcomp.dll!DCompositionCreateDevice", out)
}

/// DCompositionCreateDevice2(renderingDevice, iid, dcompositionDevice).
#[unsafe(no_mangle)]
pub extern "system" fn DCompositionCreateDevice2(
    _device: *mut c_void,
    _iid: *const u8,
    out: *mut *mut c_void,
) -> i32 {
    no_device(b"dcomp.dll!DCompositionCreateDevice2", out)
}

/// DCompositionCreateDevice3(renderingDevice, iid, dcompositionDevice).
#[unsafe(no_mangle)]
pub extern "system" fn DCompositionCreateDevice3(
    _device: *mut c_void,
    _iid: *const u8,
    out: *mut *mut c_void,
) -> i32 {
    no_device(b"dcomp.dll!DCompositionCreateDevice3", out)
}

/// DCompositionCreateSurfaceHandle(desiredAccess, securityAttributes,
/// surfaceHandle).
#[unsafe(no_mangle)]
pub extern "system" fn DCompositionCreateSurfaceHandle(
    _access: u32,
    _security: *mut c_void,
    handle: *mut *mut c_void,
) -> i32 {
    no_device(b"dcomp.dll!DCompositionCreateSurfaceHandle", handle)
}
