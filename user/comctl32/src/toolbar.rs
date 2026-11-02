//! The toolbar (ToolbarWindow32): buttons in a row (TBBUTTON: image,
//! command id, state, style, data, text), separators, images from an
//! image list (normal, hot and disabled ones), texts (TBSTYLE_LIST: right
//! of the image; otherwise below it, when buttons have texts), check
//! buttons and groups. TB_AUTOSIZE puts it at the top of its parent, full
//! width. A click on an enabled button sends its notify window (the
//! parent, or TB_SETPARENT's) WM_COMMAND with the id. Windows 11 style:
//! a flat bar, a rounded light highlight under the pointer, a darker one
//! for a pressed or checked button.

use crate::common::*;
use alloc::vec::Vec;

const TB_ENABLEBUTTON: u32 = WM_USER + 1;
const TB_CHECKBUTTON: u32 = WM_USER + 2;
const TB_PRESSBUTTON: u32 = WM_USER + 3;
const TB_HIDEBUTTON: u32 = WM_USER + 4;
const TB_INDETERMINATE: u32 = WM_USER + 5;
const TB_MARKBUTTON: u32 = WM_USER + 6;
const TB_ISBUTTONENABLED: u32 = WM_USER + 9;
const TB_ISBUTTONCHECKED: u32 = WM_USER + 10;
const TB_ISBUTTONPRESSED: u32 = WM_USER + 11;
const TB_ISBUTTONHIDDEN: u32 = WM_USER + 12;
const TB_SETSTATE: u32 = WM_USER + 17;
const TB_GETSTATE: u32 = WM_USER + 18;
const TB_ADDBITMAP: u32 = WM_USER + 19;
const TB_DELETEBUTTON: u32 = WM_USER + 22;
const TB_GETBUTTON: u32 = WM_USER + 23;
const TB_BUTTONCOUNT: u32 = WM_USER + 24;
const TB_COMMANDTOINDEX: u32 = WM_USER + 25;
const TB_GETITEMRECT: u32 = WM_USER + 29;
const TB_BUTTONSTRUCTSIZE: u32 = WM_USER + 30;
const TB_SETBUTTONSIZE: u32 = WM_USER + 31;
const TB_SETBITMAPSIZE: u32 = WM_USER + 32;
const TB_AUTOSIZE: u32 = WM_USER + 33;
const TB_GETTOOLTIPS: u32 = WM_USER + 35;
const TB_SETTOOLTIPS: u32 = WM_USER + 36;
const TB_SETPARENT: u32 = WM_USER + 37;
const TB_SETROWS: u32 = WM_USER + 39;
const TB_GETROWS: u32 = WM_USER + 40;
const TB_SETCMDID: u32 = WM_USER + 42;
const TB_CHANGEBITMAP: u32 = WM_USER + 43;
const TB_GETBITMAP: u32 = WM_USER + 44;
const TB_SETINDENT: u32 = WM_USER + 47;
const TB_SETIMAGELIST: u32 = WM_USER + 48;
const TB_GETIMAGELIST: u32 = WM_USER + 49;
const TB_GETRECT: u32 = WM_USER + 51;
const TB_SETHOTIMAGELIST: u32 = WM_USER + 52;
const TB_GETHOTIMAGELIST: u32 = WM_USER + 53;
const TB_SETDISABLEDIMAGELIST: u32 = WM_USER + 54;
const TB_GETDISABLEDIMAGELIST: u32 = WM_USER + 55;
const TB_SETSTYLE: u32 = WM_USER + 56;
const TB_GETSTYLE: u32 = WM_USER + 57;
const TB_GETBUTTONSIZE: u32 = WM_USER + 58;
const TB_SETBUTTONWIDTH: u32 = WM_USER + 59;
const TB_SETMAXTEXTROWS: u32 = WM_USER + 60;
const TB_GETTEXTROWS: u32 = WM_USER + 61;
const TB_GETBUTTONINFOW: u32 = WM_USER + 63;
const TB_SETBUTTONINFOW: u32 = WM_USER + 64;
const TB_INSERTBUTTONW: u32 = WM_USER + 67;
const TB_ADDBUTTONSW: u32 = WM_USER + 68;
const TB_HITTEST: u32 = WM_USER + 69;
const TB_SETDRAWTEXTFLAGS: u32 = WM_USER + 70;
const TB_GETHOTITEM: u32 = WM_USER + 71;
const TB_SETHOTITEM: u32 = WM_USER + 72;
const TB_GETBUTTONTEXTW: u32 = WM_USER + 75;
const TB_ADDSTRINGW: u32 = WM_USER + 77;
const TB_GETMAXSIZE: u32 = WM_USER + 83;
const TB_SETEXTENDEDSTYLE: u32 = WM_USER + 84;
const TB_GETEXTENDEDSTYLE: u32 = WM_USER + 85;
const TB_GETPADDING: u32 = WM_USER + 86;
const TB_SETPADDING: u32 = WM_USER + 87;
const TB_SETLISTGAP: u32 = WM_USER + 96;
const TB_GETIDEALSIZE: u32 = WM_USER + 99;

const TBSTATE_CHECKED: u8 = 0x1;
const TBSTATE_PRESSED: u8 = 0x2;
const TBSTATE_ENABLED: u8 = 0x4;
const TBSTATE_HIDDEN: u8 = 0x8;
const TBSTATE_INDETERMINATE: u8 = 0x10;
const TBSTATE_MARKED: u8 = 0x80;
const BTNS_SEP: u8 = 0x1;
const BTNS_CHECK: u8 = 0x2;
const BTNS_GROUP: u8 = 0x4;
const BTNS_SHOWTEXT: u8 = 0x40;
const TBSTYLE_TOOLTIPS: u32 = 0x100;
const TBSTYLE_LIST: u32 = 0x1000;
/// TBSTYLE_EX_MIXEDBUTTONS: texts only on BTNS_SHOWTEXT buttons.
const TBSTYLE_EX_MIXEDBUTTONS: u32 = 0x8;

#[derive(Clone)]
struct Button {
    image: i32,
    id: i32,
    state: u8,
    style: u8,
    data: usize,
    text: Vec<u16>,
}

struct Bar {
    buttons: Vec<Button>,
    strings: Vec<Vec<u16>>,
    images: usize,
    hot_images: usize,
    disabled_images: usize,
    bitmap: (i32, i32),
    /// TB_SETBUTTONSIZE's size (0: from the images and texts).
    button: (i32, i32),
    indent: i32,
    padding: (i32, i32),
    ex_style: u32,
    text_rows: i32,
    notify: usize,
    tooltips: usize,
    hot: i32,
    pressed: i32,
    font: usize,
}

fn new_bar() -> Bar {
    Bar {
        buttons: Vec::new(),
        strings: Vec::new(),
        images: 0,
        hot_images: 0,
        disabled_images: 0,
        bitmap: (16, 15),
        button: (0, 0),
        indent: 0,
        padding: (7, 6),
        ex_style: 0,
        text_rows: 1,
        notify: 0,
        tooltips: 0,
        hot: -1,
        pressed: -1,
        font: 0,
    }
}

static BARS: Store<Bar> = Store::new();

fn with<R>(hwnd: Hwnd, f: impl FnOnce(&mut Bar) -> R) -> Option<R> {
    BARS.with(hwnd, new_bar, f)
}

/// TBBUTTON (64-bit): iBitmap, idCommand (i32 at 0, 4), fsState, fsStyle
/// (u8 at 8, 9), dwData (16), iString (24).
fn button_of(p: *const u8, strings: &[Vec<u16>]) -> Button {
    // SAFETY: the caller's TBBUTTON.
    let (image, id, state, style, data, string) = unsafe {
        (
            p.cast::<i32>().read_unaligned(),
            p.add(4).cast::<i32>().read_unaligned(),
            *p.add(8),
            *p.add(9),
            p.add(16).cast::<usize>().read_unaligned(),
            p.add(24).cast::<isize>().read_unaligned(),
        )
    };
    let text = if (0..0x10000).contains(&string) {
        strings.get(string as usize).cloned().unwrap_or_default()
    } else if string != -1 {
        owned(wide(string as *const u16, 256))
    } else {
        Vec::new()
    };
    Button { image, id, state, style, data, text }
}

/// Each button's rectangle (hidden ones empty), in order.
fn layout(hwnd: Hwnd) -> Vec<[i32; 4]> {
    let s = style(hwnd);
    let (font, buttons, bitmap, size, indent, padding, ex) = match with(hwnd, |b| {
        (b.font, b.buttons.clone(), b.bitmap, b.button, b.indent, b.padding, b.ex_style)
    }) {
        Some(v) => v,
        None => return Vec::new(),
    };
    // SAFETY: plain calls on the window's DC.
    let hdc = unsafe { GetDC(hwnd) };
    let old = select(hdc, if font != 0 { font } else { default_font() });
    let line = text_height(hdc);
    let list = s & TBSTYLE_LIST != 0;
    let any_text = buttons.iter().any(|b| !b.text.is_empty());
    let shows_text = |b: &Button| {
        !b.text.is_empty() && (ex & TBSTYLE_EX_MIXEDBUTTONS == 0 || b.style & BTNS_SHOWTEXT != 0)
    };
    // The buttons' height: the image (and a line of text below it).
    let height = if size.1 > 0 {
        size.1
    } else if any_text && !list {
        bitmap.1 + padding.1 + line + 2
    } else {
        bitmap.1.max(if list && any_text { line } else { 0 }) + padding.1 + 2
    };
    let mut rects = Vec::new();
    let _ = rects.try_reserve(buttons.len());
    let mut x = indent;
    for b in &buttons {
        if b.state & TBSTATE_HIDDEN != 0 {
            rects.push([x, 0, x, 0]);
            continue;
        }
        let w = if b.style & BTNS_SEP != 0 {
            if b.image > 0 { b.image } else { 8 }
        } else if size.0 > 0 {
            size.0
        } else {
            let tw = if shows_text(b) { text_width(hdc, &b.text) } else { 0 };
            if list {
                bitmap.0 + padding.0 + if tw > 0 { tw + 6 } else { 0 }
            } else {
                (bitmap.0 + padding.0).max(if tw > 0 { tw + 10 } else { 0 })
            }
        };
        rects.push([x, 2, x + w, 2 + height]);
        x += w;
    }
    select(hdc, old);
    // SAFETY: the DC from GetDC.
    unsafe { ReleaseDC(hwnd, hdc) };
    rects
}

fn bar_height(hwnd: Hwnd) -> i32 {
    layout(hwnd).iter().map(|r| r[3]).max().unwrap_or(24).max(24) + 2
}

/// TB_AUTOSIZE: at the parent's top (CCS_TOP, the default) or bottom,
/// full width.
fn autosize(hwnd: Hwnd) {
    let s = style(hwnd);
    if s & (CCS_NORESIZE | CCS_NOPARENTALIGN) != 0 {
        return;
    }
    // SAFETY: plain call.
    let parent = unsafe { GetParent(hwnd) };
    if parent == 0 {
        return;
    }
    let pc = client(parent);
    let h = bar_height(hwnd);
    let y = if s & 0x3 == 0x3 { pc[3] - h } else { 0 }; // CCS_BOTTOM
    // SAFETY: plain call.
    unsafe { SetWindowPos(hwnd, 0, 0, y, pc[2], h, SWP_NOZORDER | SWP_NOACTIVATE) };
}

fn index_of(hwnd: Hwnd, id: usize) -> Option<usize> {
    with(hwnd, |b| b.buttons.iter().position(|x| x.id == id as i32)).flatten()
}

fn hit(hwnd: Hwnd, x: i32, y: i32) -> Option<usize> {
    layout(hwnd).iter().position(|r| x >= r[0] && x < r[2] && y >= r[1] && y < r[3])
}

/// Changes a button's state bits by command id; whether it exists.
fn set_bits(hwnd: Hwnd, id: usize, bits: u8, on: bool) -> isize {
    let done = with(hwnd, |b| match b.buttons.iter_mut().find(|x| x.id == id as i32) {
        Some(x) => {
            if on {
                x.state |= bits;
            } else {
                x.state &= !bits;
            }
            true
        }
        None => false,
    })
    .unwrap_or(false);
    redraw(hwnd);
    isize::from(done)
}

fn state_of(hwnd: Hwnd, id: usize) -> Option<u8> {
    with(hwnd, |b| b.buttons.iter().find(|x| x.id == id as i32).map(|x| x.state)).flatten()
}

/// A click on button `i`: a check button changes (in a group, the others
/// go off), then the notify window gets WM_COMMAND.
fn click(hwnd: Hwnd, i: usize) {
    let Some((id, target)) = with(hwnd, |b| {
        let n = b.buttons.len();
        let style = b.buttons.get(i)?.style;
        if style & BTNS_CHECK != 0 {
            if style & BTNS_GROUP != 0 {
                // The group: the run of BTNS_GROUP check buttons around i.
                let mut a = i;
                while a > 0
                    && b.buttons[a - 1].style & (BTNS_CHECK | BTNS_GROUP) == BTNS_CHECK | BTNS_GROUP
                {
                    a -= 1;
                }
                let mut z = i;
                while z + 1 < n
                    && b.buttons[z + 1].style & (BTNS_CHECK | BTNS_GROUP) == BTNS_CHECK | BTNS_GROUP
                {
                    z += 1;
                }
                for k in a..=z {
                    b.buttons[k].state &= !TBSTATE_CHECKED;
                }
                b.buttons[i].state |= TBSTATE_CHECKED;
            } else {
                b.buttons[i].state ^= TBSTATE_CHECKED;
            }
        }
        Some((b.buttons[i].id, b.notify))
    })
    .flatten() else {
        return;
    };
    redraw(hwnd);
    // SAFETY: plain call.
    let target = if target != 0 { target } else { unsafe { GetParent(hwnd) } };
    // SAFETY: plain call.
    unsafe { SendMessageW(target, WM_COMMAND, id as u32 as usize & 0xffff, hwnd as isize) };
}

fn insert(hwnd: Hwnd, at: Option<usize>, count: usize, p: *const u8) -> isize {
    if p.is_null() {
        return 0;
    }
    // TBBUTTON's size on x64 (TB_BUTTONSTRUCTSIZE is always this).
    let size = 32;
    with(hwnd, |b| {
        if b.buttons.try_reserve(count).is_err() {
            return 0;
        }
        for k in 0..count {
            // SAFETY: the caller's array of `count` TBBUTTONs.
            let button = button_of(unsafe { p.add(k * size) }, &b.strings);
            match at {
                Some(i) if i <= b.buttons.len() => b.buttons.insert(i + k, button),
                _ => b.buttons.push(button),
            }
        }
        1
    })
    .unwrap_or(0)
}

fn paint_bar(hwnd: Hwnd) {
    let rects = layout(hwnd);
    let s = style(hwnd);
    let Some((font, buttons, images, hot_images, disabled_images, hot, pressed, bitmap, ex)) =
        with(hwnd, |b| {
            (
                b.font,
                b.buttons.clone(),
                b.images,
                b.hot_images,
                b.disabled_images,
                b.hot,
                b.pressed,
                b.bitmap,
                b.ex_style,
            )
        })
    else {
        return;
    };
    paint(hwnd, font, |hdc, r| {
        fill(hdc, r[0], r[1], r[2], r[3], BAR);
        transparent(hdc);
        let list = s & TBSTYLE_LIST != 0;
        for (i, (b, br)) in buttons.iter().zip(&rects).enumerate() {
            if b.state & TBSTATE_HIDDEN != 0 {
                continue;
            }
            if b.style & BTNS_SEP != 0 {
                let x = (br[0] + br[2]) / 2;
                fill(hdc, x, br[1] + 4, x + 1, br[3] - 4, LINE);
                continue;
            }
            let enabled = b.state & TBSTATE_ENABLED != 0;
            let down = b.state & (TBSTATE_PRESSED | TBSTATE_CHECKED) != 0
                || (pressed == i as i32 && hot == i as i32);
            if enabled && down {
                round_fill(
                    hdc,
                    [br[0] + 1, br[1] + 1, br[2] - 1, br[3] - 1],
                    4,
                    if b.state & TBSTATE_CHECKED != 0 { CHECKED } else { PRESSED },
                );
            } else if enabled && hot == i as i32 {
                round_fill(hdc, [br[0] + 1, br[1] + 1, br[2] - 1, br[3] - 1], 4, HOT);
            }
            let shows = !b.text.is_empty()
                && (ex & TBSTYLE_EX_MIXEDBUTTONS == 0 || b.style & BTNS_SHOWTEXT != 0);
            let (ix, iy) = if list || !shows {
                (
                    br[0] + if list && shows { 4 } else { (br[2] - br[0] - bitmap.0) / 2 },
                    br[1] + (br[3] - br[1] - bitmap.1) / 2,
                )
            } else {
                (br[0] + (br[2] - br[0] - bitmap.0) / 2, br[1] + 3)
            };
            let list_to_use = if !enabled && disabled_images != 0 {
                disabled_images
            } else if hot == i as i32 && hot_images != 0 {
                hot_images
            } else {
                images
            };
            if list_to_use != 0 && b.image >= 0 {
                crate::imagelist::ImageList_Draw(list_to_use, b.image, hdc, ix, iy, 1);
            }
            if shows {
                text_color(hdc, if enabled { TEXT } else { TEXT_DISABLED });
                if list {
                    draw_text(hdc, &b.text, [ix + bitmap.0 + 4, br[1], br[2], br[3]], 0x24 | 0x800);
                } else {
                    draw_text(
                        hdc,
                        &b.text,
                        [br[0], iy + bitmap.1 + 1, br[2], br[3]],
                        0x1 | 0x20 | 0x800,
                    );
                }
            }
        }
    });
}

pub extern "system" fn proc_(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        0x0001 => {
            // WM_CREATE: a tooltip control for TBSTYLE_TOOLTIPS.
            with(hwnd, |_| ());
            if style(hwnd) & TBSTYLE_TOOLTIPS != 0 {
                let class: [u16; 17] = core::array::from_fn(|i| {
                    u16::from(*b"tooltips_class32\0".get(i).unwrap_or(&0))
                });
                // SAFETY: plain call.
                let tt = unsafe {
                    CreateWindowExW(
                        0,
                        class.as_ptr(),
                        core::ptr::null(),
                        0x8000_0000,
                        0,
                        0,
                        0,
                        0,
                        hwnd,
                        0,
                        0,
                        core::ptr::null_mut(),
                    )
                };
                with(hwnd, |b| b.tooltips = tt);
            }
            0
        }
        WM_DESTROY => {
            if let Some(b) = BARS.remove(hwnd)
                && b.tooltips != 0
            {
                // SAFETY: plain call.
                unsafe { DestroyWindow(b.tooltips) };
            }
            // SAFETY: plain call.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_SETFONT => {
            with(hwnd, |b| b.font = wparam);
            redraw(hwnd);
            0
        }
        WM_GETFONT => with(hwnd, |b| b.font as isize).unwrap_or(0),
        WM_ERASEBKGND => 1,
        WM_SIZE => {
            redraw(hwnd);
            0
        }
        TB_BUTTONSTRUCTSIZE => 0,
        TB_ADDBUTTONSW => insert(hwnd, None, wparam, lparam as *const u8),
        TB_INSERTBUTTONW => {
            let at = if (wparam as isize) < 0 { None } else { Some(wparam) };
            let r = insert(hwnd, at, 1, lparam as *const u8);
            redraw(hwnd);
            r
        }
        TB_DELETEBUTTON => {
            let ok = with(hwnd, |b| {
                if wparam < b.buttons.len() {
                    b.buttons.remove(wparam);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
            redraw(hwnd);
            isize::from(ok)
        }
        TB_BUTTONCOUNT => with(hwnd, |b| b.buttons.len() as isize).unwrap_or(0),
        TB_COMMANDTOINDEX => index_of(hwnd, wparam).map_or(-1, |i| i as isize),
        TB_GETBUTTON => {
            let Some(b) = with(hwnd, |b| b.buttons.get(wparam).cloned()).flatten() else {
                return 0;
            };
            if lparam != 0 {
                let p = lparam as *mut u8;
                // SAFETY: the caller's TBBUTTON.
                unsafe {
                    core::ptr::write_bytes(p, 0, 32);
                    p.cast::<i32>().write_unaligned(b.image);
                    p.add(4).cast::<i32>().write_unaligned(b.id);
                    *p.add(8) = b.state;
                    *p.add(9) = b.style;
                    p.add(16).cast::<usize>().write_unaligned(b.data);
                    p.add(24).cast::<isize>().write_unaligned(-1);
                }
            }
            1
        }
        TB_GETBUTTONINFOW | TB_SETBUTTONINFOW => button_info(hwnd, msg, wparam, lparam),
        TB_GETBUTTONTEXTW => {
            let Some(text) = with(hwnd, |b| {
                b.buttons.iter().find(|x| x.id == wparam as i32).map(|x| x.text.clone())
            })
            .flatten() else {
                return -1;
            };
            if lparam != 0 {
                // SAFETY: the caller's buffer (large enough, as the
                // message requires).
                unsafe {
                    core::ptr::copy_nonoverlapping(text.as_ptr(), lparam as *mut u16, text.len());
                    *(lparam as *mut u16).add(text.len()) = 0;
                }
            }
            text.len() as isize
        }
        TB_ADDSTRINGW => {
            if wparam != 0 {
                // A string resource: not supported (no index).
                crate::unsupported(b"comctl32.dll!TB_ADDSTRINGW(resource)", lparam as u64);
                return -1;
            }
            // Strings separated by NULs, ending with an empty one.
            let mut p = lparam as *const u16;
            with(hwnd, |b| {
                let first = b.strings.len() as isize;
                loop {
                    let s = wide(p, 1024);
                    if s.is_empty() {
                        break;
                    }
                    if b.strings.try_reserve(1).is_err() {
                        break;
                    }
                    b.strings.push(owned(s));
                    // SAFETY: past the string's NUL, inside the list.
                    p = unsafe { p.add(s.len() + 1) };
                }
                first
            })
            .unwrap_or(-1)
        }
        TB_ENABLEBUTTON => set_bits(hwnd, wparam, TBSTATE_ENABLED, lparam & 0xffff != 0),
        TB_CHECKBUTTON => set_bits(hwnd, wparam, TBSTATE_CHECKED, lparam & 0xffff != 0),
        TB_PRESSBUTTON => set_bits(hwnd, wparam, TBSTATE_PRESSED, lparam & 0xffff != 0),
        TB_HIDEBUTTON => set_bits(hwnd, wparam, TBSTATE_HIDDEN, lparam & 0xffff != 0),
        TB_INDETERMINATE => set_bits(hwnd, wparam, TBSTATE_INDETERMINATE, lparam & 0xffff != 0),
        TB_MARKBUTTON => set_bits(hwnd, wparam, TBSTATE_MARKED, lparam & 0xffff != 0),
        TB_ISBUTTONENABLED => {
            state_of(hwnd, wparam).map_or(0, |s| isize::from(s & TBSTATE_ENABLED != 0))
        }
        TB_ISBUTTONCHECKED => {
            state_of(hwnd, wparam).map_or(0, |s| isize::from(s & TBSTATE_CHECKED != 0))
        }
        TB_ISBUTTONPRESSED => {
            state_of(hwnd, wparam).map_or(0, |s| isize::from(s & TBSTATE_PRESSED != 0))
        }
        TB_ISBUTTONHIDDEN => {
            state_of(hwnd, wparam).map_or(0, |s| isize::from(s & TBSTATE_HIDDEN != 0))
        }
        TB_GETSTATE => state_of(hwnd, wparam).map_or(-1, isize::from),
        TB_SETSTATE => {
            let ok = with(hwnd, |b| match b.buttons.iter_mut().find(|x| x.id == wparam as i32) {
                Some(x) => {
                    x.state = lparam as u8;
                    true
                }
                None => false,
            })
            .unwrap_or(false);
            redraw(hwnd);
            isize::from(ok)
        }
        TB_SETCMDID => {
            with(hwnd, |b| {
                if let Some(x) = b.buttons.get_mut(wparam) {
                    x.id = lparam as i32;
                }
            });
            1
        }
        TB_CHANGEBITMAP => {
            with(hwnd, |b| {
                if let Some(x) = b.buttons.iter_mut().find(|x| x.id == wparam as i32) {
                    x.image = lparam as i32;
                }
            });
            redraw(hwnd);
            1
        }
        TB_GETBITMAP => with(hwnd, |b| {
            b.buttons.iter().find(|x| x.id == wparam as i32).map_or(-1, |x| x.image as isize)
        })
        .unwrap_or(-1),
        TB_GETITEMRECT => match layout(hwnd).get(wparam) {
            Some(&r) if lparam != 0 => {
                // SAFETY: the caller's RECT.
                unsafe { *(lparam as *mut [i32; 4]) = r };
                1
            }
            _ => 0,
        },
        TB_GETRECT => match index_of(hwnd, wparam).and_then(|i| layout(hwnd).get(i).copied()) {
            Some(r) if lparam != 0 => {
                // SAFETY: the caller's RECT.
                unsafe { *(lparam as *mut [i32; 4]) = r };
                1
            }
            _ => 0,
        },
        TB_HITTEST => {
            // SAFETY: the caller's POINT.
            let [x, y] = unsafe { *(lparam as *const [i32; 2]) };
            hit(hwnd, x, y).map_or(-1, |i| i as isize)
        }
        TB_SETBUTTONSIZE => {
            with(hwnd, |b| b.button = ((lparam & 0xffff) as i32, ((lparam >> 16) & 0xffff) as i32));
            1
        }
        TB_SETBUTTONWIDTH => 1,
        TB_SETBITMAPSIZE => {
            with(hwnd, |b| b.bitmap = ((lparam & 0xffff) as i32, ((lparam >> 16) & 0xffff) as i32));
            1
        }
        TB_GETBUTTONSIZE => {
            let r = layout(hwnd).into_iter().find(|r| r[2] > r[0]).unwrap_or([0, 0, 23, 22]);
            (((r[2] - r[0]) & 0xffff) | ((r[3] - r[1]) << 16)) as isize
        }
        TB_AUTOSIZE => {
            autosize(hwnd);
            redraw(hwnd);
            0
        }
        TB_GETMAXSIZE | TB_GETIDEALSIZE => {
            let rects = layout(hwnd);
            let w = rects.iter().map(|r| r[2]).max().unwrap_or(0);
            let h = rects.iter().map(|r| r[3]).max().unwrap_or(0);
            if lparam != 0 {
                // SAFETY: the caller's SIZE.
                unsafe { *(lparam as *mut [i32; 2]) = [w, h] };
            }
            1
        }
        TB_SETIMAGELIST | TB_SETHOTIMAGELIST | TB_SETDISABLEDIMAGELIST => {
            let old = with(hwnd, |b| {
                let slot = match msg {
                    TB_SETIMAGELIST => &mut b.images,
                    TB_SETHOTIMAGELIST => &mut b.hot_images,
                    _ => &mut b.disabled_images,
                };
                let old = core::mem::replace(slot, lparam as usize);
                if msg == TB_SETIMAGELIST && lparam != 0 {
                    let (mut cx, mut cy) = (0, 0);
                    crate::imagelist::ImageList_GetIconSize(lparam as usize, &mut cx, &mut cy);
                    if cx > 0 {
                        b.bitmap = (cx, cy);
                    }
                }
                old
            })
            .unwrap_or(0);
            redraw(hwnd);
            old as isize
        }
        TB_GETIMAGELIST => with(hwnd, |b| b.images as isize).unwrap_or(0),
        TB_GETHOTIMAGELIST => with(hwnd, |b| b.hot_images as isize).unwrap_or(0),
        TB_GETDISABLEDIMAGELIST => with(hwnd, |b| b.disabled_images as isize).unwrap_or(0),
        TB_ADDBITMAP => {
            // TBADDBITMAP: hInst (NULL: nID is an HBITMAP strip of
            // `wparam` images of the bitmap size), nID. System bitmaps
            // (HINST_COMMCTRL) and resources are not there yet.
            if lparam == 0 {
                return -1;
            }
            // SAFETY: the caller's TBADDBITMAP.
            let [instance, bitmap] = unsafe { *(lparam as *const [usize; 2]) };
            if instance != 0 {
                crate::unsupported(b"comctl32.dll!TB_ADDBITMAP(resource)", bitmap as u64);
                return -1;
            }
            let (images, size) = with(hwnd, |b| (b.images, b.bitmap)).unwrap_or((0, (16, 15)));
            let list = if images != 0 {
                images
            } else {
                // ILC_COLOR32 | ILC_MASK.
                let l = crate::imagelist::ImageList_Create(size.0, size.1, 0x21, wparam as i32, 4);
                with(hwnd, |b| b.images = l);
                l
            };
            if list == 0 {
                return -1;
            }
            let r = crate::imagelist::ImageList_Add(list, bitmap, 0) as isize;
            redraw(hwnd);
            r
        }
        TB_SETINDENT => {
            with(hwnd, |b| b.indent = wparam as i32);
            1
        }
        TB_SETPADDING => {
            let old = with(hwnd, |b| {
                let old = b.padding;
                b.padding = ((lparam & 0xffff) as i32, ((lparam >> 16) & 0xffff) as i32);
                old
            })
            .unwrap_or((7, 6));
            ((old.0 & 0xffff) | old.1 << 16) as isize
        }
        TB_GETPADDING => {
            with(hwnd, |b| ((b.padding.0 & 0xffff) | b.padding.1 << 16) as isize).unwrap_or(0)
        }
        TB_SETEXTENDEDSTYLE => {
            with(hwnd, |b| core::mem::replace(&mut b.ex_style, lparam as u32) as isize).unwrap_or(0)
        }
        TB_GETEXTENDEDSTYLE => with(hwnd, |b| b.ex_style as isize).unwrap_or(0),
        TB_SETMAXTEXTROWS => {
            with(hwnd, |b| b.text_rows = wparam as i32);
            1
        }
        TB_GETTEXTROWS => with(hwnd, |b| b.text_rows as isize).unwrap_or(1),
        TB_SETROWS => 0,
        TB_GETROWS => 1,
        TB_SETDRAWTEXTFLAGS | TB_SETLISTGAP => 0,
        TB_SETSTYLE => {
            // SAFETY: plain call.
            unsafe { SetWindowLongW(hwnd, GWL_STYLE, lparam as i32) };
            redraw(hwnd);
            0
        }
        TB_GETSTYLE => style(hwnd) as isize,
        TB_GETTOOLTIPS => with(hwnd, |b| b.tooltips as isize).unwrap_or(0),
        TB_SETTOOLTIPS => {
            with(hwnd, |b| b.tooltips = wparam);
            0
        }
        TB_SETPARENT => {
            with(hwnd, |b| core::mem::replace(&mut b.notify, wparam) as isize).unwrap_or(0)
        }
        TB_GETHOTITEM => with(hwnd, |b| b.hot as isize).unwrap_or(-1),
        TB_SETHOTITEM => {
            let old = with(hwnd, |b| core::mem::replace(&mut b.hot, wparam as i32) as isize)
                .unwrap_or(-1);
            redraw(hwnd);
            old
        }
        WM_MOUSEMOVE => {
            let (x, y) = (lparam as i16 as i32, (lparam >> 16) as i16 as i32);
            let at = hit(hwnd, x, y).map_or(-1, |i| i as i32);
            let changed = with(hwnd, |b| core::mem::replace(&mut b.hot, at) != at).unwrap_or(false);
            if changed {
                redraw(hwnd);
            }
            0
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let (x, y) = (lparam as i16 as i32, (lparam >> 16) as i16 as i32);
            if let Some(i) = hit(hwnd, x, y) {
                let usable = with(hwnd, |b| {
                    b.buttons
                        .get(i)
                        .is_some_and(|x| x.state & TBSTATE_ENABLED != 0 && x.style & BTNS_SEP == 0)
                })
                .unwrap_or(false);
                if usable {
                    with(hwnd, |b| {
                        b.pressed = i as i32;
                        b.hot = i as i32;
                    });
                    // SAFETY: plain call.
                    unsafe { SetCapture(hwnd) };
                    redraw(hwnd);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = (lparam as i16 as i32, (lparam >> 16) as i16 as i32);
            let pressed = with(hwnd, |b| core::mem::replace(&mut b.pressed, -1)).unwrap_or(-1);
            // SAFETY: plain calls.
            unsafe {
                if GetCapture() == hwnd {
                    ReleaseCapture();
                }
            }
            redraw(hwnd);
            if pressed >= 0 && hit(hwnd, x, y) == Some(pressed as usize) {
                click(hwnd, pressed as usize);
            }
            0
        }
        WM_PAINT => {
            paint_bar(hwnd);
            0
        }
        // SAFETY: plain call.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// TB_GETBUTTONINFOW/TB_SETBUTTONINFOW: TBBUTTONINFOW (cbSize, dwMask,
/// idCommand, iImage, fsState, fsStyle, cx, lParam, pszText, cchText) by
/// command id (or index with TBIF_BYINDEX).
fn button_info(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    const TBIF_IMAGE: u32 = 0x1;
    const TBIF_TEXT: u32 = 0x2;
    const TBIF_STATE: u32 = 0x4;
    const TBIF_STYLE: u32 = 0x8;
    const TBIF_LPARAM: u32 = 0x10;
    const TBIF_COMMAND: u32 = 0x20;
    const TBIF_BYINDEX: u32 = 0x8000_0000;
    if lparam == 0 {
        return -1;
    }
    let p = lparam as *mut u8;
    // SAFETY: the caller's TBBUTTONINFOW: dwMask at 4, idCommand 8,
    // iImage 12, fsState 16, fsStyle 17, cx 18 (u16), lParam 24,
    // pszText 32, cchText 40.
    let mask = unsafe { p.add(4).cast::<u32>().read_unaligned() };
    let index = if mask & TBIF_BYINDEX != 0 { Some(wparam) } else { index_of(hwnd, wparam) };
    let Some(i) = index else { return -1 };
    let found = with(hwnd, |b| {
        let Some(x) = b.buttons.get_mut(i) else { return false };
        // SAFETY: as above.
        unsafe {
            if msg == TB_GETBUTTONINFOW {
                if mask & TBIF_COMMAND != 0 {
                    p.add(8).cast::<i32>().write_unaligned(x.id);
                }
                if mask & TBIF_IMAGE != 0 {
                    p.add(12).cast::<i32>().write_unaligned(x.image);
                }
                if mask & TBIF_STATE != 0 {
                    *p.add(16) = x.state;
                }
                if mask & TBIF_STYLE != 0 {
                    *p.add(17) = x.style;
                }
                if mask & TBIF_LPARAM != 0 {
                    p.add(24).cast::<usize>().write_unaligned(x.data);
                }
                if mask & TBIF_TEXT != 0 {
                    let out = p.add(32).cast::<*mut u16>().read_unaligned();
                    let room = p.add(40).cast::<i32>().read_unaligned().max(0) as usize;
                    if !out.is_null() && room > 0 {
                        let n = x.text.len().min(room - 1);
                        core::ptr::copy_nonoverlapping(x.text.as_ptr(), out, n);
                        *out.add(n) = 0;
                    }
                }
            } else {
                if mask & TBIF_COMMAND != 0 {
                    x.id = p.add(8).cast::<i32>().read_unaligned();
                }
                if mask & TBIF_IMAGE != 0 {
                    x.image = p.add(12).cast::<i32>().read_unaligned();
                }
                if mask & TBIF_STATE != 0 {
                    x.state = *p.add(16);
                }
                if mask & TBIF_STYLE != 0 {
                    x.style = *p.add(17);
                }
                if mask & TBIF_LPARAM != 0 {
                    x.data = p.add(24).cast::<usize>().read_unaligned();
                }
                if mask & TBIF_TEXT != 0 {
                    x.text = owned(wide(p.add(32).cast::<*const u16>().read_unaligned(), 256));
                }
            }
        }
        true
    })
    .unwrap_or(false);
    redraw(hwnd);
    if !found {
        -1
    } else if msg == TB_GETBUTTONINFOW {
        i as isize
    } else {
        1
    }
}
