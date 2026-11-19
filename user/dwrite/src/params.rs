//! Text-rendering parameters (IDWriteRenderingParams, dwrite.h): the
//! gamma, the enhanced contrast, the ClearType level, the pixel geometry
//! and the rendering mode a program reads before it draws text, which
//! Direct2D's drawing state blocks carry. Word asks the factory for the
//! system's set before it draws its first frame.
//!
//! One object answers all four interfaces, as Windows' does
//! (IDWriteRenderingParams, ...1's grayscale enhanced contrast, ...2's
//! grid fit mode and ...3's rendering mode; measured, `dwrite_test`'s
//! `params`). gala has one display and no monitor profile, so the
//! factory's own set and a monitor's are the same: the values Windows
//! answers for a plain sRGB display.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::{
    E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, HResult, Iid, S_OK, VTable, answered, guid, query,
    reached, vt,
};

pub const IID_PARAMS: Iid =
    guid(0x2f0d_a53a, 0x2add, 0x47cd, [0x82, 0xee, 0xd9, 0xec, 0x34, 0x68, 0x8e, 0x75]);
pub const IID_PARAMS1: Iid =
    guid(0x9441_3cf4, 0xa6fc, 0x4248, [0x8b, 0x50, 0x66, 0x74, 0x34, 0x8f, 0xca, 0xd3]);
pub const IID_PARAMS2: Iid =
    guid(0xf9d7_11c3, 0x9777, 0x40ae, [0x87, 0xe8, 0x3e, 0x5a, 0xf9, 0xbf, 0x09, 0x48]);
pub const IID_PARAMS3: Iid =
    guid(0xb792_4baa, 0x391b, 0x412a, [0x8c, 0x5c, 0xe4, 0x4c, 0xc2, 0xd8, 0x67, 0xdc]);

/// DWRITE_PIXEL_GEOMETRY_RGB.
const PIXEL_GEOMETRY_RGB: u32 = 1;
/// The last DWRITE_PIXEL_GEOMETRY (BGR).
const PIXEL_GEOMETRY_LAST: u32 = 2;
/// DWRITE_RENDERING_MODE_DEFAULT.
const RENDERING_MODE_DEFAULT: u32 = 0;
/// The last DWRITE_RENDERING_MODE (OUTLINE).
const RENDERING_MODE_LAST: u32 = 6;
/// DWRITE_GRID_FIT_MODE_DEFAULT.
const GRID_FIT_DEFAULT: u32 = 0;
/// The largest gamma DirectWrite takes (measured: 256 is accepted,
/// 256.1 is E_INVALIDARG).
const GAMMA_MAX: f32 = 256.0;

/// The system's parameters, which is what the factory and a monitor both
/// answer: the values Windows gives for a plain sRGB display, measured
/// on the pinned build.
const SYSTEM: Values = Values {
    gamma: 1.8,
    contrast: 0.5,
    grayscale_contrast: 1.0,
    cleartype_level: 1.0,
    geometry: PIXEL_GEOMETRY_RGB,
    mode: RENDERING_MODE_DEFAULT,
    grid_fit: GRID_FIT_DEFAULT,
};

#[derive(Clone, Copy)]
struct Values {
    gamma: f32,
    contrast: f32,
    grayscale_contrast: f32,
    cleartype_level: f32,
    geometry: u32,
    mode: u32,
    grid_fit: u32,
}

/// A parameter set: a counted object, as a program holds it past the
/// call that made it (a render target or a drawing state block keeps a
/// reference).
#[repr(C)]
struct Params {
    vtbl: *const (),
    refs: AtomicU32,
    values: Values,
}

static PARAMS_VTBL: VTable<11> = VTable([
    params_qi as *const (),
    params_add_ref as *const (),
    params_release as *const (),
    params_GetGamma as *const (),
    params_GetEnhancedContrast as *const (),
    params_GetClearTypeLevel as *const (),
    params_GetPixelGeometry as *const (),
    params_GetRenderingMode as *const (),
    params_GetGrayscaleEnhancedContrast as *const (),
    params_GetGridFitMode as *const (),
    params_GetRenderingMode1 as *const (),
]);

/// IDWriteFactory::CreateRenderingParams(&params): the system's set.
pub extern "system" fn factory_CreateRenderingParams(
    _this: *const c_void,
    out: *mut *const c_void,
) -> HResult {
    let r = make(SYSTEM, out);
    answered(b"IDWriteFactory::CreateRenderingParams", r);
    r
}

/// IDWriteFactory::CreateMonitorRenderingParams(monitor, &params): the
/// system's set, whichever monitor is asked about (gala has one display
/// and no monitor profiles). Windows takes a NULL monitor as well
/// (measured), so the handle is not checked.
pub extern "system" fn factory_CreateMonitorRenderingParams(
    _this: *const c_void,
    _monitor: *const c_void,
    out: *mut *const c_void,
) -> HResult {
    let r = make(SYSTEM, out);
    answered(b"IDWriteFactory::CreateMonitorRenderingParams", r);
    r
}

/// IDWriteFactory::CreateCustomRenderingParams(gamma, contrast,
/// clearTypeLevel, pixelGeometry, renderingMode, &params). What Windows
/// refuses it refuses (measured): a gamma outside (0, 256], a negative
/// contrast or ClearType level, and a pixel geometry or rendering mode
/// that is not one of the enumeration's. The grayscale contrast and the
/// grid fit mode are the system's, as Windows leaves them.
pub extern "system" fn factory_CreateCustomRenderingParams(
    _this: *const c_void,
    gamma: f32,
    contrast: f32,
    cleartype_level: f32,
    geometry: u32,
    mode: u32,
    out: *mut *const c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller's output.
    unsafe { *out = core::ptr::null() };
    let refused = gamma.is_nan()
        || gamma <= 0.0
        || gamma > GAMMA_MAX
        || contrast.is_nan()
        || contrast < 0.0
        || cleartype_level.is_nan()
        || cleartype_level < 0.0
        || geometry > PIXEL_GEOMETRY_LAST
        || mode > RENDERING_MODE_LAST;
    if refused {
        answered(b"IDWriteFactory::CreateCustomRenderingParams", E_INVALIDARG);
        return E_INVALIDARG;
    }
    let r = make(
        Values {
            gamma,
            contrast,
            grayscale_contrast: SYSTEM.grayscale_contrast,
            cleartype_level,
            geometry,
            mode,
            grid_fit: SYSTEM.grid_fit,
        },
        out,
    );
    answered(b"IDWriteFactory::CreateCustomRenderingParams", r);
    r
}

/// A fresh parameter set for the caller (Windows hands out a new object
/// per call, not a shared one).
fn make(values: Values, out: *mut *const c_void) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let layout = core::alloc::Layout::new::<Params>();
    // SAFETY: a non-zero-sized layout.
    let p = unsafe { alloc::alloc::alloc(layout) }.cast::<Params>();
    if p.is_null() {
        // SAFETY: the caller's output.
        unsafe { *out = core::ptr::null() };
        return E_OUTOFMEMORY;
    }
    // SAFETY: fresh memory for one Params.
    unsafe { p.write(Params { vtbl: vt(&PARAMS_VTBL), refs: AtomicU32::new(1), values }) };
    // SAFETY: the caller's output.
    unsafe { *out = p.cast() };
    S_OK
}

fn values(this: *const c_void) -> Values {
    // SAFETY: one of our parameter sets.
    unsafe { (*(this as *const Params)).values }
}

extern "system" fn params_qi(
    this: *const c_void,
    riid: *const Iid,
    out: *mut *const c_void,
) -> HResult {
    let r = query(this, &[IID_PARAMS, IID_PARAMS1, IID_PARAMS2, IID_PARAMS3], riid, out);
    if r == S_OK {
        params_add_ref(this);
    }
    r
}

extern "system" fn params_add_ref(this: *const c_void) -> u32 {
    // SAFETY: one of our parameter sets.
    unsafe { (*(this as *const Params)).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

extern "system" fn params_release(this: *const c_void) -> u32 {
    // SAFETY: one of our parameter sets.
    let left = unsafe { (*(this as *const Params)).refs.fetch_sub(1, Ordering::AcqRel) } - 1;
    if left == 0 {
        // SAFETY: the last reference is gone; it came from `make`.
        unsafe {
            core::ptr::drop_in_place(this as *mut Params);
            alloc::alloc::dealloc(this as *mut u8, core::alloc::Layout::new::<Params>());
        }
    }
    left
}

extern "system" fn params_GetGamma(this: *const c_void) -> f32 {
    reached!(b"IDWriteRenderingParams::GetGamma");
    values(this).gamma
}

extern "system" fn params_GetEnhancedContrast(this: *const c_void) -> f32 {
    reached!(b"IDWriteRenderingParams::GetEnhancedContrast");
    values(this).contrast
}

extern "system" fn params_GetClearTypeLevel(this: *const c_void) -> f32 {
    reached!(b"IDWriteRenderingParams::GetClearTypeLevel");
    values(this).cleartype_level
}

extern "system" fn params_GetPixelGeometry(this: *const c_void) -> u32 {
    reached!(b"IDWriteRenderingParams::GetPixelGeometry");
    values(this).geometry
}

extern "system" fn params_GetRenderingMode(this: *const c_void) -> u32 {
    reached!(b"IDWriteRenderingParams::GetRenderingMode");
    values(this).mode
}

/// IDWriteRenderingParams1::GetGrayscaleEnhancedContrast.
extern "system" fn params_GetGrayscaleEnhancedContrast(this: *const c_void) -> f32 {
    reached!(b"IDWriteRenderingParams1::GetGrayscaleEnhancedContrast");
    values(this).grayscale_contrast
}

/// IDWriteRenderingParams2::GetGridFitMode.
extern "system" fn params_GetGridFitMode(this: *const c_void) -> u32 {
    reached!(b"IDWriteRenderingParams2::GetGridFitMode");
    values(this).grid_fit
}

/// IDWriteRenderingParams3::GetRenderingMode1, which is the same
/// rendering mode (DWRITE_RENDERING_MODE1 extends the enumeration gala's
/// values come from, and Windows answers the mode the set was made with).
extern "system" fn params_GetRenderingMode1(this: *const c_void) -> u32 {
    reached!(b"IDWriteRenderingParams3::GetRenderingMode1");
    values(this).mode
}
