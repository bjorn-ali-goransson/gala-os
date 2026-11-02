//! The header control (SysHeader32), the list view's column titles: items
//! (HDITEMW: width, text, format with the sort arrows HDF_SORTUP/DOWN,
//! image, lParam, order) in display order; HDM_LAYOUT puts it at the top
//! of a rectangle. A click on an item sends its parent HDN_ITEMCLICKW;
//! dragging a divider resizes the column (HDN_ITEMCHANGINGW/
//! HDN_ITEMCHANGEDW, HDN_ENDTRACKW). Windows 11 style: white, a thin
//! line under it and between columns, a small chevron for the sort order.

use crate::common::*;
use alloc::vec::Vec;

pub const HDM_GETITEMCOUNT: u32 = 0x1200;
pub const HDM_DELETEITEM: u32 = 0x1202;
pub const HDM_LAYOUT: u32 = 0x1205;
pub const HDM_HITTEST: u32 = 0x1206;
pub const HDM_GETITEMRECT: u32 = 0x1207;
pub const HDM_SETIMAGELIST: u32 = 0x1208;
pub const HDM_GETIMAGELIST: u32 = 0x1209;
pub const HDM_INSERTITEMW: u32 = 0x120a;
pub const HDM_GETITEMW: u32 = 0x120b;
pub const HDM_SETITEMW: u32 = 0x120c;
pub const HDM_ORDERTOINDEX: u32 = 0x120f;
pub const HDM_GETORDERARRAY: u32 = 0x1211;
pub const HDM_SETORDERARRAY: u32 = 0x1212;
const HDN_ITEMCHANGINGW: i32 = -320;
const HDN_ITEMCHANGEDW: i32 = -321;
const HDN_ITEMCLICKW: i32 = -322;
const HDN_DIVIDERDBLCLICKW: i32 = -325;
const HDN_BEGINTRACKW: i32 = -326;
const HDN_ENDTRACKW: i32 = -327;

pub const HDI_WIDTH: u32 = 0x1;
pub const HDI_TEXT: u32 = 0x2;
pub const HDI_FORMAT: u32 = 0x4;
pub const HDI_LPARAM: u32 = 0x8;
pub const HDI_IMAGE: u32 = 0x20;
pub const HDI_ORDER: u32 = 0x80;
const HDF_CENTER: i32 = 0x2;
const HDF_RIGHT: i32 = 0x1;
const HDF_SORTDOWN: i32 = 0x200;
const HDF_SORTUP: i32 = 0x400;
const HDF_IMAGE: i32 = 0x800;

/// HDITEMW (64-bit): mask (0), cxy (4), pszText (8), hbm (16),
/// cchTextMax (24), fmt (28), lParam (32), iImage (40), iOrder (44).
#[derive(Clone, Default)]
pub struct Item {
    pub width: i32,
    pub text: Vec<u16>,
    pub format: i32,
    pub param: isize,
    pub image: i32,
}

struct Header {
    /// In display order.
    items: Vec<Item>,
    /// Display position -> item index (HDM_SETORDERARRAY); items keep
    /// their indexes, the order maps them.
    order: Vec<usize>,
    images: usize,
    font: usize,
    /// A divider being dragged: the item, the pointer's start x and the
    /// width then.
    tracking: Option<(usize, i32, i32)>,
    pressed: Option<usize>,
}

static HEADERS: Store<Header> = Store::new();

fn with<R>(hwnd: Hwnd, f: impl FnOnce(&mut Header) -> R) -> Option<R> {
    HEADERS.with(
        hwnd,
        || Header {
            items: Vec::new(),
            order: Vec::new(),
            images: 0,
            font: 0,
            tracking: None,
            pressed: None,
        },
        f,
    )
}

/// The header's height for its font.
pub fn height(font: usize) -> i32 {
    line_height(font) + 10
}

/// Each item's left and right edge, by item index.
fn edges(h: &Header) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    out.resize(h.items.len(), (0, 0));
    let mut x = 0;
    for &i in &h.order {
        let w = h.items.get(i).map_or(0, |it| it.width.max(0));
        if let Some(e) = out.get_mut(i) {
            *e = (x, x + w);
        }
        x += w;
    }
    out
}

/// The item's edges (for the list view's columns).
pub fn item_edges(hwnd: Hwnd) -> Vec<(i32, i32)> {
    with(hwnd, |h| edges(h)).unwrap_or_default()
}

/// Item indexes in display order.
pub fn order(hwnd: Hwnd) -> Vec<usize> {
    with(hwnd, |h| h.order.clone()).unwrap_or_default()
}

fn read_item(p: *const u8, item: &mut Item) {
    // SAFETY: the caller's HDITEMW.
    unsafe {
        let mask = p.cast::<u32>().read_unaligned();
        if mask & HDI_WIDTH != 0 {
            item.width = p.add(4).cast::<i32>().read_unaligned();
        }
        if mask & HDI_TEXT != 0 {
            let t = p.add(8).cast::<*const u16>().read_unaligned();
            item.text = if t as isize == -1 { Vec::new() } else { owned(wide(t, 260)) };
        }
        if mask & HDI_FORMAT != 0 {
            item.format = p.add(28).cast::<i32>().read_unaligned();
        }
        if mask & HDI_LPARAM != 0 {
            item.param = p.add(32).cast::<isize>().read_unaligned();
        }
        if mask & HDI_IMAGE != 0 {
            item.image = p.add(40).cast::<i32>().read_unaligned();
        }
    }
}

fn write_item(p: *mut u8, item: &Item, position: usize) {
    // SAFETY: the caller's HDITEMW.
    unsafe {
        let mask = p.cast::<u32>().read_unaligned();
        if mask & HDI_WIDTH != 0 {
            p.add(4).cast::<i32>().write_unaligned(item.width);
        }
        if mask & HDI_TEXT != 0 {
            let out = p.add(8).cast::<*mut u16>().read_unaligned();
            let room = p.add(24).cast::<i32>().read_unaligned().max(0) as usize;
            if !out.is_null() && room > 0 {
                let n = item.text.len().min(room - 1);
                core::ptr::copy_nonoverlapping(item.text.as_ptr(), out, n);
                *out.add(n) = 0;
            }
        }
        if mask & HDI_FORMAT != 0 {
            p.add(28).cast::<i32>().write_unaligned(item.format);
        }
        if mask & HDI_LPARAM != 0 {
            p.add(32).cast::<isize>().write_unaligned(item.param);
        }
        if mask & HDI_IMAGE != 0 {
            p.add(40).cast::<i32>().write_unaligned(item.image);
        }
        if mask & HDI_ORDER != 0 {
            p.add(44).cast::<i32>().write_unaligned(position as i32);
        }
    }
}

/// NMHEADERW: NMHDR, iItem (24), iButton (28), pitem (32).
#[repr(C)]
struct NmHeader {
    hdr: NmHdr,
    item: i32,
    button: i32,
    pitem: *mut u8,
}

fn send(hwnd: Hwnd, code: i32, item: usize, hd: Option<&mut [u8; 72]>) -> isize {
    // SAFETY: plain call.
    let parent = unsafe { GetParent(hwnd) };
    let mut n = NmHeader {
        hdr: NmHdr { from: hwnd, id: id_of(hwnd), code: code as u32 },
        item: item as i32,
        button: 0,
        pitem: hd.map_or(core::ptr::null_mut(), |b| b.as_mut_ptr()),
    };
    notify(parent, &mut n)
}

fn at(lparam: isize) -> (i32, i32) {
    (lparam as i16 as i32, (lparam >> 16) as i16 as i32)
}

/// The item under x, and whether x is on its right divider.
fn hit(hwnd: Hwnd, x: i32) -> Option<(usize, bool)> {
    let e = item_edges(hwnd);
    let mut best = None;
    for (i, &(l, r)) in e.iter().enumerate() {
        if (r - 4..r + 4).contains(&x) && r > l {
            return Some((i, true));
        }
        if x >= l && x < r {
            best = Some((i, false));
        }
    }
    best
}

pub extern "system" fn proc_(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        HDM_GETITEMCOUNT => with(hwnd, |h| h.items.len() as isize).unwrap_or(0),
        HDM_INSERTITEMW => {
            if lparam == 0 {
                return -1;
            }
            let mut item = Item::default();
            read_item(lparam as *const u8, &mut item);
            let r = with(hwnd, |h| {
                if h.items.try_reserve(1).is_err() || h.order.try_reserve(1).is_err() {
                    return -1;
                }
                let at = wparam.min(h.items.len());
                h.items.insert(at, item);
                for o in h.order.iter_mut() {
                    if *o >= at {
                        *o += 1;
                    }
                }
                let position = at.min(h.order.len());
                h.order.insert(position, at);
                at as isize
            })
            .unwrap_or(-1);
            redraw(hwnd);
            r
        }
        HDM_DELETEITEM => {
            let ok = with(hwnd, |h| {
                if wparam >= h.items.len() {
                    return false;
                }
                h.items.remove(wparam);
                h.order.retain(|&o| o != wparam);
                for o in h.order.iter_mut() {
                    if *o > wparam {
                        *o -= 1;
                    }
                }
                true
            })
            .unwrap_or(false);
            redraw(hwnd);
            isize::from(ok)
        }
        HDM_GETITEMW => {
            let Some(Some((item, position))) = with(hwnd, |h| {
                h.items
                    .get(wparam)
                    .cloned()
                    .map(|it| (it, h.order.iter().position(|&o| o == wparam).unwrap_or(0)))
            }) else {
                return 0;
            };
            if lparam != 0 {
                write_item(lparam as *mut u8, &item, position);
            }
            1
        }
        HDM_SETITEMW => {
            if lparam == 0 {
                return 0;
            }
            let ok = with(hwnd, |h| match h.items.get_mut(wparam) {
                Some(it) => {
                    read_item(lparam as *const u8, it);
                    true
                }
                None => false,
            })
            .unwrap_or(false);
            redraw(hwnd);
            isize::from(ok)
        }
        HDM_GETITEMRECT => {
            let e = item_edges(hwnd);
            let c = client(hwnd);
            match e.get(wparam) {
                Some(&(l, r)) if lparam != 0 => {
                    // SAFETY: the caller's RECT.
                    unsafe { *(lparam as *mut [i32; 4]) = [l, 0, r, c[3]] };
                    1
                }
                _ => 0,
            }
        }
        HDM_LAYOUT => {
            // HDLAYOUT: the rectangle (in, shrunk) and a WINDOWPOS (out).
            if lparam == 0 {
                return 0;
            }
            let font = with(hwnd, |h| h.font).unwrap_or(0);
            let hidden = style(hwnd) & 0x8 != 0; // HDS_HIDDEN
            let h = if hidden { 0 } else { height(font) };
            // SAFETY: the caller's HDLAYOUT.
            unsafe {
                let [rect, pos] = *(lparam as *const [*mut i32; 2]);
                let [l, t, r, _] = *(rect as *const [i32; 4]);
                *rect.add(1) = t + h;
                let p = pos.cast::<u8>();
                p.cast::<usize>().write_unaligned(hwnd);
                p.add(8).cast::<usize>().write_unaligned(0);
                p.add(16).cast::<[i32; 4]>().write_unaligned([l, t, r - l, h]);
                p.add(32).cast::<u32>().write_unaligned(0);
            }
            1
        }
        HDM_HITTEST => {
            // HDHITTESTINFO: pt, flags, iItem.
            if lparam == 0 {
                return -1;
            }
            // SAFETY: the caller's HDHITTESTINFO.
            let [x, _y] = unsafe { *(lparam as *const [i32; 2]) };
            let found = hit(hwnd, x);
            // SAFETY: as above.
            unsafe {
                let p = lparam as *mut i32;
                *p.add(2) = match found {
                    Some((_, true)) => 0x4, // HHT_ONDIVIDER
                    Some(_) => 0x2,         // HHT_ONHEADER
                    None => 0x1,            // HHT_NOWHERE
                };
                *p.add(3) = found.map_or(-1, |(i, _)| i as i32);
            }
            found.map_or(-1, |(i, _)| i as isize)
        }
        HDM_SETIMAGELIST => {
            with(hwnd, |h| core::mem::replace(&mut h.images, lparam as usize) as isize).unwrap_or(0)
        }
        HDM_GETIMAGELIST => with(hwnd, |h| h.images as isize).unwrap_or(0),
        HDM_ORDERTOINDEX => {
            with(hwnd, |h| h.order.get(wparam).map_or(wparam, |&o| o) as isize).unwrap_or(0)
        }
        HDM_GETORDERARRAY => {
            let order = order(hwnd);
            if lparam == 0 || wparam < order.len() {
                return 0;
            }
            for (k, &o) in order.iter().enumerate() {
                // SAFETY: the caller's array of `wparam` ints.
                unsafe { *(lparam as *mut i32).add(k) = o as i32 };
            }
            1
        }
        HDM_SETORDERARRAY => {
            if lparam == 0 {
                return 0;
            }
            // SAFETY: the caller's array of `wparam` ints.
            let given = unsafe { core::slice::from_raw_parts(lparam as *const i32, wparam) };
            let ok = with(hwnd, |h| {
                if given.len() != h.items.len()
                    || given.iter().any(|&o| o < 0 || o as usize >= h.items.len())
                {
                    return false;
                }
                h.order.clear();
                h.order.extend(given.iter().map(|&o| o as usize));
                true
            })
            .unwrap_or(false);
            redraw(hwnd);
            isize::from(ok)
        }
        WM_SETFONT => {
            with(hwnd, |h| h.font = wparam);
            redraw(hwnd);
            0
        }
        WM_GETFONT => with(hwnd, |h| h.font as isize).unwrap_or(0),
        WM_ERASEBKGND => 1,
        WM_LBUTTONDOWN => {
            let (x, _) = at(lparam);
            match hit(hwnd, x) {
                Some((i, true)) => {
                    let w = with(hwnd, |h| h.items.get(i).map_or(0, |it| it.width)).unwrap_or(0);
                    with(hwnd, |h| h.tracking = Some((i, x, w)));
                    send(hwnd, HDN_BEGINTRACKW, i, None);
                    // SAFETY: plain call.
                    unsafe { SetCapture(hwnd) };
                }
                Some((i, false)) => {
                    with(hwnd, |h| h.pressed = Some(i));
                    // SAFETY: plain call.
                    unsafe { SetCapture(hwnd) };
                    redraw(hwnd);
                }
                None => {}
            }
            0
        }
        WM_MOUSEMOVE => {
            let (x, _) = at(lparam);
            if let Some(Some((i, x0, w0))) = with(hwnd, |h| h.tracking) {
                let w = (w0 + x - x0).max(0);
                let mut hd = [0u8; 72];
                hd[..4].copy_from_slice(&HDI_WIDTH.to_le_bytes());
                hd[4..8].copy_from_slice(&w.to_le_bytes());
                if send(hwnd, HDN_ITEMCHANGINGW, i, Some(&mut hd)) == 0 {
                    with(hwnd, |h| {
                        if let Some(it) = h.items.get_mut(i) {
                            it.width = w;
                        }
                    });
                    send(hwnd, HDN_ITEMCHANGEDW, i, Some(&mut hd));
                    redraw(hwnd);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            let (x, _) = at(lparam);
            let (tracking, pressed) =
                with(hwnd, |h| (h.tracking.take(), h.pressed.take())).unwrap_or((None, None));
            // SAFETY: plain calls.
            unsafe {
                if GetCapture() == hwnd {
                    ReleaseCapture();
                }
            }
            if let Some((i, _, _)) = tracking {
                send(hwnd, HDN_ENDTRACKW, i, None);
            } else if let Some(i) = pressed
                && hit(hwnd, x).map(|(j, _)| j) == Some(i)
            {
                send(hwnd, HDN_ITEMCLICKW, i, None);
            }
            redraw(hwnd);
            0
        }
        WM_LBUTTONDBLCLK => {
            let (x, _) = at(lparam);
            if let Some((i, true)) = hit(hwnd, x) {
                send(hwnd, HDN_DIVIDERDBLCLICKW, i, None);
            }
            0
        }
        WM_DESTROY => {
            HEADERS.remove(hwnd);
            // SAFETY: plain call.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_PAINT => {
            let Some((items, font, images, pressed)) =
                with(hwnd, |h| (h.items.clone(), h.font, h.images, h.pressed))
            else {
                return 0;
            };
            let e = item_edges(hwnd);
            paint(hwnd, font, |hdc, r| {
                fill(hdc, r[0], r[1], r[2], r[3], WHITE);
                fill(hdc, r[0], r[3] - 1, r[2], r[3], LINE);
                transparent(hdc);
                text_color(hdc, TEXT);
                for (i, (it, &(l, rr))) in items.iter().zip(&e).enumerate() {
                    if rr <= l {
                        continue;
                    }
                    if pressed == Some(i) {
                        fill(hdc, l, r[1], rr, r[3] - 1, HOT);
                    }
                    fill(hdc, rr - 1, r[1] + 5, rr, r[3] - 5, LINE);
                    let mut left = l + 8;
                    if it.format & HDF_IMAGE != 0 && images != 0 && it.image >= 0 {
                        crate::imagelist::ImageList_Draw(
                            images,
                            it.image,
                            hdc,
                            left,
                            (r[3] - 16) / 2,
                            1,
                        );
                        left += 20;
                    }
                    let sorted = it.format & (HDF_SORTUP | HDF_SORTDOWN) != 0;
                    let right = rr - if sorted { 20 } else { 8 };
                    let align = if it.format & HDF_RIGHT != 0 {
                        0x2
                    } else if it.format & HDF_CENTER != 0 {
                        0x1
                    } else {
                        0
                    };
                    draw_text(
                        hdc,
                        &it.text,
                        [left, r[1], right, r[3]],
                        align | 0x24 | 0x800 | 0x8000,
                    );
                    if sorted {
                        // A chevron: up for ascending, down for descending.
                        let cx = rr - 13;
                        let cy = (r[3] - 1) / 2;
                        for k in 0..4 {
                            let y =
                                if it.format & HDF_SORTUP != 0 { cy - 2 + k } else { cy + 1 - k };
                            fill(hdc, cx - k, y, cx - k + 1, y + 1, TEXT_DIM);
                            fill(hdc, cx + k, y, cx + k + 1, y + 1, TEXT_DIM);
                        }
                    }
                }
            });
            0
        }
        // SAFETY: plain call.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
