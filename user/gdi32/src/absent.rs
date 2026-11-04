//! gdi32 functions gala has no use for yet, as real exports (Chromium's
//! chrome.dll resolves its gdi32 delay imports with GetProcAddress, which
//! does not return `gala_stubs` stubs). Each fails the way its
//! documentation gives for failure, sets ERROR_NOT_SUPPORTED and reports
//! itself to the ledger (`gdi32.dll!<name>`, "unsupported"):
//!
//! - Paths (BeginPath .. WidenPath, PolyBezierTo, SetMiterLimit,
//!   SetArcDirection): FALSE (0 for SetArcDirection). Drawing is direct;
//!   no path is recorded.
//! - Enhanced metafiles: creation and loading give NULL, playing and the
//!   queries FALSE or 0.
//! - Printing (StartDocW .. CancelDC, SetAbortProc): no DC is a printer
//!   DC: SP_ERROR (-1) for the documents and pages, FALSE for CancelDC.
//! - Pixel formats (OpenGL): no ICD, so no formats: 0 / FALSE.
//! - GetICMProfileW (no colour profiles), GetOutlineTextMetricsW (no
//!   outline metrics yet): FALSE / 0. EnumFontFamiliesExA: 0 (no
//!   callback; the W version enumerates).
//! - ExtEscape: 0, "escape not implemented", as documented; GdiComment
//!   (no metafile DC to record it in): FALSE.

use super::unsupported;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetLastError(code: u32);
}

const ERROR_NOT_SUPPORTED: u32 = 50;

/// Reports `what` and sets ERROR_NOT_SUPPORTED.
fn absent(what: &str) {
    unsupported(what.as_bytes(), 0);
    // SAFETY: plain call.
    unsafe { SetLastError(ERROR_NOT_SUPPORTED) };
}

/// Exports that report themselves and return `value` (of type `ty`).
/// They take no parameters: under the x64 calling convention the caller
/// owns its arguments, so any call is safe to receive.
macro_rules! absent_exports {
    ($($ty:ty = $value:expr => $($name:ident)*;)*) => {
        $($(
            #[allow(non_snake_case)]
            #[unsafe(no_mangle)]
            pub extern "system" fn $name() -> $ty {
                absent(concat!("gdi32.dll!", stringify!($name)));
                $value
            }
        )*)*
    };
}

absent_exports! {
    i32 = 0 =>
        BeginPath EndPath CloseFigure AbortPath FillPath StrokePath StrokeAndFillPath
        SelectClipPath WidenPath FlattenPath PolyBezierTo SetMiterLimit SetArcDirection
        PlayEnhMetaFile EnumEnhMetaFile PlayEnhMetaFileRecord DeleteEnhMetaFile
        CancelDC ChoosePixelFormat DescribePixelFormat GetPixelFormat SetPixelFormat
        SwapBuffers GetICMProfileW GdiComment ExtEscape EnumFontFamiliesExA;
    u32 = 0 => GetEnhMetaFileHeader GetEnhMetaFileBits GetOutlineTextMetricsW;
    usize = 0 => CreateEnhMetaFileW CloseEnhMetaFile SetEnhMetaFileBits;
    i32 = -1 => StartDocW EndDoc StartPage EndPage SetAbortProc;
}
