//! gala's msimg32.dll: AlphaBlend, TransparentBlt and GradientFill, which
//! are gdi32's GdiAlphaBlend, GdiTransparentBlt and GdiGradientFill (as on
//! Windows, where msimg32 forwards to them).

#![no_std]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[cfg_attr(
    target_arch = "x86",
    link(name = "gdi32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "gdi32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GdiAlphaBlend(
        dst: usize,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        src: usize,
        sx: i32,
        sy: i32,
        sw: i32,
        sh: i32,
        blend: u32,
    ) -> i32;
    fn GdiTransparentBlt(
        dst: usize,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        src: usize,
        sx: i32,
        sy: i32,
        sw: i32,
        sh: i32,
        key: u32,
    ) -> i32;
    fn GdiGradientFill(
        hdc: usize,
        vertices: *const u8,
        n: u32,
        mesh: *const u8,
        count: u32,
        mode: u32,
    ) -> i32;
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn AlphaBlend(
    dst: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    src: usize,
    sx: i32,
    sy: i32,
    sw: i32,
    sh: i32,
    blend: u32,
) -> i32 {
    // SAFETY: passed on as given.
    unsafe { GdiAlphaBlend(dst, x, y, w, h, src, sx, sy, sw, sh, blend) }
}

#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn TransparentBlt(
    dst: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    src: usize,
    sx: i32,
    sy: i32,
    sw: i32,
    sh: i32,
    key: u32,
) -> i32 {
    // SAFETY: passed on as given.
    unsafe { GdiTransparentBlt(dst, x, y, w, h, src, sx, sy, sw, sh, key) }
}

#[unsafe(no_mangle)]
pub extern "system" fn GradientFill(
    hdc: usize,
    vertices: *const u8,
    n: u32,
    mesh: *const u8,
    count: u32,
    mode: u32,
) -> i32 {
    // SAFETY: passed on as given (gdi32 reads the caller's arrays).
    unsafe { GdiGradientFill(hdc, vertices, n, mesh, count, mode) }
}

/// The DLL entry point: nothing to set up.
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
