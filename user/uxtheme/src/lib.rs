//! gala's uxtheme.dll (visual styles): gala runs with themes off, the
//! documented "classic" state: IsAppThemed and IsThemeActive are FALSE,
//! OpenThemeData gives NULL (programs then draw controls themselves), the
//! functions taking a theme fail with E_HANDLE for NULL except
//! GetThemeSysColor/GetThemeSysFont, which give the system's (documented
//! for a NULL theme); SetWindowTheme is accepted.

#![no_std]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetSysColor(index: i32) -> u32;
    fn SystemParametersInfoW(action: u32, param: u32, out: *mut u8, update: u32) -> i32;
}

const S_OK: i32 = 0;
const E_HANDLE: i32 = 0x8007_0006u32 as i32;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;

#[unsafe(no_mangle)]
pub extern "system" fn IsAppThemed() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn IsThemeActive() -> i32 {
    0
}

/// OpenThemeData: NULL (no theme is active).
#[unsafe(no_mangle)]
pub extern "system" fn OpenThemeData(_hwnd: usize, _classes: *const u16) -> usize {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn CloseThemeData(theme: usize) -> i32 {
    if theme == 0 { E_HANDLE } else { S_OK }
}

/// SetWindowTheme: accepted (no theme is drawn).
#[unsafe(no_mangle)]
pub extern "system" fn SetWindowTheme(_hwnd: usize, _app: *const u16, _list: *const u16) -> i32 {
    S_OK
}

/// GetThemeSysColor: with a NULL theme, the system colour.
#[unsafe(no_mangle)]
pub extern "system" fn GetThemeSysColor(_theme: usize, index: i32) -> u32 {
    // SAFETY: a plain call.
    unsafe { GetSysColor(index) }
}

/// GetThemeSysFont(theme, TMT_*FONT, LOGFONTW*): with a NULL theme, the
/// system's (the non-client metrics' fonts).
#[unsafe(no_mangle)]
pub extern "system" fn GetThemeSysFont(_theme: usize, id: i32, out: *mut u8) -> i32 {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // NONCLIENTMETRICSW (504 bytes): the caption, small caption, menu,
    // status and message fonts at 24, 124, 224, 316 and 408.
    let at = match id {
        801 => 24,        // TMT_CAPTIONFONT
        802 => 124,       // TMT_SMALLCAPTIONFONT
        803 => 224,       // TMT_MENUFONT
        804 => 316,       // TMT_STATUSFONT
        805 | 806 => 408, // TMT_MSGBOXFONT, TMT_ICONTITLEFONT
        _ => return E_INVALIDARG,
    };
    let mut ncm = [0u8; 504];
    ncm[0..4].copy_from_slice(&504u32.to_le_bytes());
    // SAFETY: our buffer of cbSize bytes; the caller's LOGFONTW.
    unsafe {
        if SystemParametersInfoW(0x29, 504, ncm.as_mut_ptr(), 0) == 0 {
            return E_INVALIDARG;
        }
        core::ptr::copy_nonoverlapping(ncm.as_ptr().add(at), out, 92);
    }
    S_OK
}

/// The theme functions on a theme handle: none is ever open (E_HANDLE).
macro_rules! no_theme {
    ($($name:ident)*) => {
        $(
            #[unsafe(no_mangle)]
            pub extern "system" fn $name() -> i32 {
                E_HANDLE
            }
        )*
    };
}

no_theme!(
    DrawThemeBackground GetThemeBackgroundContentRect GetThemeBackgroundExtent GetThemeColor
    GetThemeFont GetThemeInt GetThemeMargins GetThemePartSize DrawThemeParentBackground
);

#[unsafe(no_mangle)]
pub extern "system" fn IsThemeBackgroundPartiallyTransparent(
    _theme: usize,
    _part: i32,
    _state: i32,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn IsThemePartDefined(_theme: usize, _part: i32, _state: i32) -> i32 {
    0
}

/// GetCurrentThemeName: no theme (E_FAIL is what an inactive theme gives).
#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentThemeName(
    _name: *mut u16,
    _n: i32,
    _color: *mut u16,
    _cn: i32,
    _size: *mut u16,
    _sn: i32,
) -> i32 {
    0x8000_4005u32 as i32
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

/// DllMain (the entry point our link flags name).
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
