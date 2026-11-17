//! gala's d3d10.dll: no adapter yet (ROADMAP GPU G0;
//! `../../gpu_common.rs` says how the stubs fail). Office's image module
//! (OIMG.DLL) delay-loads D3D10CreateEffectFromMemory, which is why the
//! export is here at all: a delay load that cannot be resolved ends the
//! process (`gpu_stub_test`, ROADMAP OF). Everything else of Direct3D 10
//! lives in d3d10_1.dll, which is what Office's modules bind the device
//! call against.
#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

include!("../../gpu_common.rs");

/// D3D10CreateEffectFromMemory(pData, DataLength, FXFlags, pDevice,
/// pEffectPool, ppEffect): an effect belongs to a device, and no device
/// came from here, so this is E_FAIL -- what Windows answers when asked
/// without one (measured).
#[unsafe(no_mangle)]
pub extern "system" fn D3D10CreateEffectFromMemory(
    _data: *const u8,
    _length: usize,
    _flags: u32,
    _device: *mut c_void,
    _pool: *mut c_void,
    effect: *mut *mut c_void,
) -> i32 {
    clear(effect);
    unsupported(b"d3d10.dll!D3D10CreateEffectFromMemory");
    E_FAIL
}
