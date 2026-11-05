//! gala's dxgi.dll: no adapter yet (ROADMAP GPU G0; `../../gpu_common.rs`
//! says how the stubs fail). Chromium's GPU process asks for a factory to
//! list adapters and takes the failure as "no GPU".
#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

include!("../../gpu_common.rs");

fn no_factory(name: &[u8], factory: *mut *mut c_void) -> i32 {
    clear(factory);
    unsupported(name);
    if factory.is_null() { E_INVALIDARG } else { DXGI_ERROR_UNSUPPORTED }
}

/// CreateDXGIFactory(riid, ppFactory).
#[unsafe(no_mangle)]
pub extern "system" fn CreateDXGIFactory(_iid: *const u8, factory: *mut *mut c_void) -> i32 {
    no_factory(b"dxgi.dll!CreateDXGIFactory", factory)
}

/// CreateDXGIFactory1(riid, ppFactory).
#[unsafe(no_mangle)]
pub extern "system" fn CreateDXGIFactory1(_iid: *const u8, factory: *mut *mut c_void) -> i32 {
    no_factory(b"dxgi.dll!CreateDXGIFactory1", factory)
}

/// CreateDXGIFactory2(Flags, riid, ppFactory).
#[unsafe(no_mangle)]
pub extern "system" fn CreateDXGIFactory2(
    _flags: u32,
    _iid: *const u8,
    factory: *mut *mut c_void,
) -> i32 {
    no_factory(b"dxgi.dll!CreateDXGIFactory2", factory)
}

/// DXGIGetDebugInterface1(Flags, riid, pDebug): no debug layer.
#[unsafe(no_mangle)]
pub extern "system" fn DXGIGetDebugInterface1(
    _flags: u32,
    _iid: *const u8,
    debug: *mut *mut c_void,
) -> i32 {
    no_factory(b"dxgi.dll!DXGIGetDebugInterface1", debug)
}
