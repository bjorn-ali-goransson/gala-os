//! Flat scroll bars (InitializeFlatSB, UninitializeFlatSB,
//! FlatSB_GetScrollProp/SetScrollProp and the FlatSB_ counterparts of
//! user32's scroll bar functions), as documented for comctl32 6: version
//! 6 and later "do not support flat scroll bars", so a window's flat
//! scroll bars are its standard ones (every FlatSB_ scroll call is
//! user32's) and only the properties are comctl32's: InitializeFlatSB
//! gives the window a record of them (the system metrics' sizes, the
//! flat style the documentation names as the default), which
//! FlatSB_SetScrollProp changes and FlatSB_GetScrollProp reads back;
//! FlatSB_GetScrollProp(WSB_PROP_HSTYLE) is FALSE for a window that was
//! not initialized, and WSB_PROP_WINSTYLE gives the window's
//! WS_HSCROLL/WS_VSCROLL bits. Delphi's VCL (Inno Setup) initializes
//! every scrolling control's window and sets its styles and sizes.
//!
//! Records of windows that are gone are dropped when the next window is
//! initialized (no subclass: the program's window procedure stays its
//! own, as programs that compare GWLP_WNDPROC expect).

use crate::common::*;

#[cfg_attr(
    target_arch = "x86",
    link(name = "user32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "user32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetSystemMetrics(index: i32) -> i32;
    fn GetSysColor(index: i32) -> u32;
    fn SetScrollInfo(hwnd: Hwnd, bar: i32, info: *const u8, redraw: i32) -> i32;
    fn GetScrollInfo(hwnd: Hwnd, bar: i32, info: *mut u8) -> i32;
    fn SetScrollPos(hwnd: Hwnd, bar: i32, pos: i32, redraw: i32) -> i32;
    fn GetScrollPos(hwnd: Hwnd, bar: i32) -> i32;
    fn SetScrollRange(hwnd: Hwnd, bar: i32, min: i32, max: i32, redraw: i32) -> i32;
    fn GetScrollRange(hwnd: Hwnd, bar: i32, min: *mut i32, max: *mut i32) -> i32;
    fn ShowScrollBar(hwnd: Hwnd, bar: i32, show: i32) -> i32;
    fn EnableScrollBar(hwnd: Hwnd, bar: u32, arrows: u32) -> i32;
}

const WSB_PROP_CYVSCROLL: u32 = 0x1;
const WSB_PROP_CXHSCROLL: u32 = 0x2;
const WSB_PROP_CYHSCROLL: u32 = 0x4;
const WSB_PROP_CXVSCROLL: u32 = 0x8;
const WSB_PROP_CXHTHUMB: u32 = 0x10;
const WSB_PROP_CYVTHUMB: u32 = 0x20;
const WSB_PROP_VBKGCOLOR: u32 = 0x40;
const WSB_PROP_HBKGCOLOR: u32 = 0x80;
const WSB_PROP_VSTYLE: u32 = 0x100;
const WSB_PROP_HSTYLE: u32 = 0x200;
const WSB_PROP_WINSTYLE: u32 = 0x400;
const WSB_PROP_PALETTE: u32 = 0x800;
const FSB_FLAT_MODE: isize = 2;

const GWL_STYLE: i32 = -16;
const WS_VSCROLL: i32 = 0x0020_0000;
const WS_HSCROLL: i32 = 0x0010_0000;
const S_OK: i32 = 0;
const S_FALSE: i32 = 1;

/// One window's flat scroll bar properties, in WSB_PROP_* bit order
/// (CYVSCROLL, CXHSCROLL, CYHSCROLL, CXVSCROLL, CXHTHUMB, CYVTHUMB,
/// VBKGCOLOR, HBKGCOLOR, VSTYLE, HSTYLE, WINSTYLE (unused: read from the
/// window), PALETTE).
#[derive(Clone, Copy)]
struct Props([isize; 12]);

static WINDOWS: Store<Props> = Store::new();

/// The slot of a single WSB_PROP_* bit.
fn slot(index: u32) -> Option<usize> {
    (index.count_ones() == 1 && index <= WSB_PROP_PALETTE && index != WSB_PROP_WINSTYLE)
        .then(|| index.trailing_zeros() as usize)
}

fn defaults() -> Props {
    const SM_CXVSCROLL: i32 = 2;
    const SM_CYHSCROLL: i32 = 3;
    const SM_CYVSCROLL: i32 = 20;
    const SM_CXHSCROLL: i32 = 21;
    const SM_CYVTHUMB: i32 = 9;
    const SM_CXHTHUMB: i32 = 10;
    const COLOR_SCROLLBAR: i32 = 0;
    // SAFETY: plain calls.
    let m = |i| isize::from(unsafe { GetSystemMetrics(i) } as i16);
    // SAFETY: plain call.
    let bkg = unsafe { GetSysColor(COLOR_SCROLLBAR) } as isize;
    let mut p = Props([0; 12]);
    for (index, value) in [
        (WSB_PROP_CYVSCROLL, m(SM_CYVSCROLL)),
        (WSB_PROP_CXHSCROLL, m(SM_CXHSCROLL)),
        (WSB_PROP_CYHSCROLL, m(SM_CYHSCROLL)),
        (WSB_PROP_CXVSCROLL, m(SM_CXVSCROLL)),
        (WSB_PROP_CXHTHUMB, m(SM_CXHTHUMB)),
        (WSB_PROP_CYVTHUMB, m(SM_CYVTHUMB)),
        (WSB_PROP_VBKGCOLOR, bkg),
        (WSB_PROP_HBKGCOLOR, bkg),
        (WSB_PROP_VSTYLE, FSB_FLAT_MODE),
        (WSB_PROP_HSTYLE, FSB_FLAT_MODE),
    ] {
        if let Some(i) = slot(index) {
            p.0[i] = value;
        }
    }
    p
}

/// InitializeFlatSB(hwnd): TRUE for a window (its properties made, or
/// kept when it already has them), FALSE otherwise.
#[unsafe(no_mangle)]
pub extern "system" fn InitializeFlatSB(hwnd: Hwnd) -> i32 {
    // SAFETY: plain call.
    if hwnd == 0 || unsafe { IsWindow(hwnd) } == 0 {
        return 0;
    }
    // SAFETY: plain call (records of windows that are gone are dropped).
    WINDOWS.retain(|w| unsafe { IsWindow(w) } != 0);
    let props = defaults();
    i32::from(WINDOWS.with(hwnd, || props, |_| ()).is_some())
}

/// UninitializeFlatSB(hwnd): S_OK when the window had flat scroll bars
/// (it has standard ones from now on), S_FALSE when it had none.
#[unsafe(no_mangle)]
pub extern "system" fn UninitializeFlatSB(hwnd: Hwnd) -> i32 {
    if WINDOWS.remove(hwnd).is_some() { S_OK } else { S_FALSE }
}

/// FlatSB_GetScrollProp(hwnd, index, *value): the property; FALSE for a
/// window that was not initialized, an unknown index or no pointer.
/// WSB_PROP_WINSTYLE is the window's WS_HSCROLL and WS_VSCROLL bits with
/// a low bit per scroll bar, 0x2 vertical and 0x4 horizontal (Windows 11
/// 25H2, comctl32 5: 0x00200002 for a WS_VSCROLL window).
#[unsafe(no_mangle)]
pub extern "system" fn FlatSB_GetScrollProp(hwnd: Hwnd, index: u32, value: *mut i32) -> i32 {
    let Some(props) = WINDOWS.find(hwnd, |p| *p) else { return 0 };
    if value.is_null() {
        return 0;
    }
    let v = if index == WSB_PROP_WINSTYLE {
        // SAFETY: plain call.
        let bars = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } & (WS_HSCROLL | WS_VSCROLL);
        let low = if bars & WS_VSCROLL != 0 { 0x2 } else { 0 }
            | if bars & WS_HSCROLL != 0 { 0x4 } else { 0 };
        (bars | low) as isize
    } else {
        let Some(i) = slot(index) else {
            crate::unsupported(b"comctl32.dll!FlatSB_GetScrollProp(index)", u64::from(index));
            return 0;
        };
        props.0[i]
    };
    if index == WSB_PROP_PALETTE {
        // FlatSB_GetScrollPropPtr: the INT_PTR the palette is.
        // SAFETY: the caller's INT_PTR (the header's Ptr variant is this
        // function).
        unsafe { (value as *mut isize).write_unaligned(v) };
    } else {
        // SAFETY: the caller's int.
        unsafe { *value = v as i32 };
    }
    1
}

/// FlatSB_SetScrollProp(hwnd, index, value, redraw): TRUE when the
/// window was initialized and `index` names one settable property.
#[unsafe(no_mangle)]
pub extern "system" fn FlatSB_SetScrollProp(
    hwnd: Hwnd,
    index: u32,
    value: isize,
    _redraw: i32,
) -> i32 {
    let Some(i) = slot(index) else {
        crate::unsupported(b"comctl32.dll!FlatSB_SetScrollProp(index)", u64::from(index));
        return 0;
    };
    i32::from(WINDOWS.find(hwnd, |p| p.0[i] = value).is_some())
}

// The scroll bars themselves are user32's (comctl32 6 draws no flat ones).

#[unsafe(no_mangle)]
pub extern "system" fn FlatSB_EnableScrollBar(hwnd: Hwnd, bar: u32, arrows: u32) -> i32 {
    // SAFETY: plain call.
    unsafe { EnableScrollBar(hwnd, bar, arrows) }
}

#[unsafe(no_mangle)]
pub extern "system" fn FlatSB_ShowScrollBar(hwnd: Hwnd, bar: i32, show: i32) -> i32 {
    // SAFETY: plain call.
    unsafe { ShowScrollBar(hwnd, bar, show) }
}

#[unsafe(no_mangle)]
pub extern "system" fn FlatSB_GetScrollRange(
    hwnd: Hwnd,
    bar: i32,
    min: *mut i32,
    max: *mut i32,
) -> i32 {
    // SAFETY: the caller's pointers, passed on.
    unsafe { GetScrollRange(hwnd, bar, min, max) }
}

#[unsafe(no_mangle)]
pub extern "system" fn FlatSB_GetScrollInfo(hwnd: Hwnd, bar: i32, info: *mut u8) -> i32 {
    // SAFETY: the caller's SCROLLINFO, passed on.
    unsafe { GetScrollInfo(hwnd, bar, info) }
}

#[unsafe(no_mangle)]
pub extern "system" fn FlatSB_GetScrollPos(hwnd: Hwnd, bar: i32) -> i32 {
    // SAFETY: plain call.
    unsafe { GetScrollPos(hwnd, bar) }
}

#[unsafe(no_mangle)]
pub extern "system" fn FlatSB_SetScrollPos(hwnd: Hwnd, bar: i32, pos: i32, redraw: i32) -> i32 {
    // SAFETY: plain call.
    unsafe { SetScrollPos(hwnd, bar, pos, redraw) }
}

#[unsafe(no_mangle)]
pub extern "system" fn FlatSB_SetScrollInfo(
    hwnd: Hwnd,
    bar: i32,
    info: *const u8,
    redraw: i32,
) -> i32 {
    // SAFETY: the caller's SCROLLINFO, passed on.
    unsafe { SetScrollInfo(hwnd, bar, info, redraw) }
}

#[unsafe(no_mangle)]
pub extern "system" fn FlatSB_SetScrollRange(
    hwnd: Hwnd,
    bar: i32,
    min: i32,
    max: i32,
    redraw: i32,
) -> i32 {
    // SAFETY: plain call.
    unsafe { SetScrollRange(hwnd, bar, min, max, redraw) }
}
