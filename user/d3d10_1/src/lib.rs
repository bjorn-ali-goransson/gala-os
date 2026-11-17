//! gala's d3d10_1.dll: no adapter yet (ROADMAP GPU G0;
//! `../../gpu_common.rs` says how the stubs fail). Word's shared code
//! (Mso40UIwin32client.dll) and Office's image modules (OIMG.DLL,
//! VVIEWER.DLL) delay-load it, so the exports have to be there whatever
//! they answer: a delay load that cannot be resolved ends the process
//! with 0xc06d007e before it can handle anything, which is how Word died
//! (`gpu_stub_test`, ROADMAP OF).
#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

include!("../../gpu_common.rs");

/// D3D10_1_SDK_VERSION: the only SDK version D3D10CreateDevice1 takes.
/// Windows checks it before it looks for an adapter -- measured, with
/// every other value E_INVALIDARG on a machine that has one.
const D3D10_1_SDK_VERSION: u32 = 0x20;

/// D3D10CreateDevice1(pAdapter, DriverType, Software, Flags,
/// HardwareLevel, SDKVersion, ppDevice): no driver type is available.
#[unsafe(no_mangle)]
pub extern "system" fn D3D10CreateDevice1(
    _adapter: *mut c_void,
    _driver: u32,
    _software: *mut c_void,
    _flags: u32,
    _level: u32,
    sdk: u32,
    device: *mut *mut c_void,
) -> i32 {
    clear(device);
    if sdk != D3D10_1_SDK_VERSION {
        return E_INVALIDARG;
    }
    unsupported(b"d3d10_1.dll!D3D10CreateDevice1");
    DXGI_ERROR_UNSUPPORTED
}

/// D3D10CreateStateBlock(pDevice, pStateBlockMask, ppStateBlock): a
/// state block records a device's state, and no device came from here,
/// so this is E_INVALIDARG -- what Windows answers when asked without
/// one (measured).
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
