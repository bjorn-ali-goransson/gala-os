//! The list view (SysListView32), drawn as the report view (the other
//! views show the same rows without a header): columns live in its header
//! control (`header.rs`; LVM_*COLUMN become HDM_*), items have a text per
//! subitem, an image, a parameter and a state (selected, focused, ...).
//! A virtual list (LVS_OWNERDATA) keeps only the count and the states and
//! asks its parent for texts and images (LVN_GETDISPINFOW), as do items
//! with LPSTR_TEXTCALLBACK texts. Selection by mouse (Ctrl and Shift as
//! usual) and keys (arrows, Home, End, Page Up/Down) with
//! LVN_ITEMCHANGING/LVN_ITEMCHANGED; NM_CLICK, NM_DBLCLK, NM_RCLICK,
//! NM_RETURN, LVN_ITEMACTIVATE, LVN_KEYDOWN, LVN_COLUMNCLICK (a header
//! click); the wheel and WM_VSCROLL scroll. Windows 11 style: rounded
//! light-blue selection, a hover tint, a thin scroll indicator.
//! Not yet: label editing, groups, custom draw, the icon views' layouts,
//! a horizontal scroll.

use crate::common::*;
use crate::header;
use alloc::vec::Vec;

const LVM_GETBKCOLOR: u32 = 0x1000;
const LVM_SETBKCOLOR: u32 = 0x1001;
const LVM_GETIMAGELIST: u32 = 0x1002;
const LVM_SETIMAGELIST: u32 = 0x1003;
const LVM_GETITEMCOUNT: u32 = 0x1004;
const LVM_DELETEITEM: u32 = 0x1008;
const LVM_DELETEALLITEMS: u32 = 0x1009;
const LVM_GETCALLBACKMASK: u32 = 0x100a;
const LVM_SETCALLBACKMASK: u32 = 0x100b;
const LVM_GETNEXTITEM: u32 = 0x100c;
const LVM_GETITEMRECT: u32 = 0x100e;
const LVM_HITTEST: u32 = 0x1012;
const LVM_ENSUREVISIBLE: u32 = 0x1013;
const LVM_SCROLL: u32 = 0x1014;
const LVM_REDRAWITEMS: u32 = 0x1015;
const LVM_ARRANGE: u32 = 0x1016;
const LVM_GETEDITCONTROL: u32 = 0x1018;
const LVM_DELETECOLUMN: u32 = 0x101c;
const LVM_GETCOLUMNWIDTH: u32 = 0x101d;
const LVM_SETCOLUMNWIDTH: u32 = 0x101e;
const LVM_GETHEADER: u32 = 0x101f;
const LVM_GETVIEWRECT: u32 = 0x1022;
const LVM_GETTEXTCOLOR: u32 = 0x1023;
const LVM_SETTEXTCOLOR: u32 = 0x1024;
const LVM_GETTEXTBKCOLOR: u32 = 0x1025;
const LVM_SETTEXTBKCOLOR: u32 = 0x1026;
const LVM_GETTOPINDEX: u32 = 0x1027;
const LVM_GETCOUNTPERPAGE: u32 = 0x1028;
const LVM_GETORIGIN: u32 = 0x1029;
const LVM_UPDATE: u32 = 0x102a;
const LVM_SETITEMSTATE: u32 = 0x102b;
const LVM_GETITEMSTATE: u32 = 0x102c;
const LVM_SETITEMCOUNT: u32 = 0x102f;
const LVM_SORTITEMS: u32 = 0x1030;
const LVM_GETSELECTEDCOUNT: u32 = 0x1032;
const LVM_GETITEMSPACING: u32 = 0x1033;
const LVM_SETEXTENDEDLISTVIEWSTYLE: u32 = 0x1036;
const LVM_GETEXTENDEDLISTVIEWSTYLE: u32 = 0x1037;
const LVM_GETSUBITEMRECT: u32 = 0x1038;
const LVM_SUBITEMHITTEST: u32 = 0x1039;
const LVM_SETCOLUMNORDERARRAY: u32 = 0x103a;
const LVM_GETCOLUMNORDERARRAY: u32 = 0x103b;
const LVM_SETHOTITEM: u32 = 0x103c;
const LVM_GETHOTITEM: u32 = 0x103d;
const LVM_GETSELECTIONMARK: u32 = 0x1042;
const LVM_SETSELECTIONMARK: u32 = 0x1043;
const LVM_GETITEMW: u32 = 0x104b;
const LVM_SETITEMW: u32 = 0x104c;
const LVM_INSERTITEMW: u32 = 0x104d;
const LVM_SORTITEMSEX: u32 = 0x1051;
const LVM_FINDITEMW: u32 = 0x1053;
const LVM_GETSTRINGWIDTHW: u32 = 0x1057;
const LVM_GETCOLUMNW: u32 = 0x105f;
const LVM_SETCOLUMNW: u32 = 0x1060;
const LVM_INSERTCOLUMNW: u32 = 0x1061;
const LVM_GETITEMTEXTW: u32 = 0x1073;
const LVM_SETITEMTEXTW: u32 = 0x1074;
const LVM_EDITLABELW: u32 = 0x1076;
const LVM_SETBKIMAGEW: u32 = 0x108a;
const LVM_SETSELECTEDCOLUMN: u32 = 0x108c;
const LVM_SETVIEW: u32 = 0x108e;
const LVM_GETVIEW: u32 = 0x108f;
const LVM_ENABLEGROUPVIEW: u32 = 0x109d;
const LVM_SETINFOTIP: u32 = 0x10ad;
const LVM_ISGROUPVIEWENABLED: u32 = 0x10af;
const CCM_SETUNICODEFORMAT: u32 = 0x2005;
const CCM_SETVERSION: u32 = 0x2007;

const LVN_ITEMCHANGING: i32 = -100;
const LVN_ITEMCHANGED: i32 = -101;
const LVN_INSERTITEM: i32 = -102;
const LVN_DELETEITEM: i32 = -103;
const LVN_DELETEALLITEMS: i32 = -104;
const LVN_COLUMNCLICK: i32 = -108;
const LVN_ITEMACTIVATE: i32 = -114;
const LVN_KEYDOWN: i32 = -155;
const LVN_GETDISPINFOW: i32 = -177;
const NM_CLICK: i32 = -2;
const NM_DBLCLK: i32 = -3;
const NM_RETURN: i32 = -4;
const NM_RCLICK: i32 = -5;
const NM_SETFOCUS: i32 = -7;
const NM_KILLFOCUS: i32 = -8;
const HDN_ITEMCLICKW: i32 = -322;

const LVS_TYPEMASK: u32 = 0x3;
const LVS_REPORT: u32 = 0x1;
const LVS_SINGLESEL: u32 = 0x4;
const LVS_SHOWSELALWAYS: u32 = 0x8;
const LVS_OWNERDATA: u32 = 0x1000;
const LVS_NOCOLUMNHEADER: u32 = 0x4000;
const LVS_EX_GRIDLINES: u32 = 0x1;

const LVIF_TEXT: u32 = 0x1;
const LVIF_IMAGE: u32 = 0x2;
const LVIF_PARAM: u32 = 0x4;
const LVIF_STATE: u32 = 0x8;
const LVIF_INDENT: u32 = 0x10;
const LVIS_FOCUSED: u32 = 0x1;
const LVIS_SELECTED: u32 = 0x2;
const LVCF_FMT: u32 = 0x1;
const LVCF_WIDTH: u32 = 0x2;
const LVCF_TEXT: u32 = 0x4;
const LVCF_IMAGE: u32 = 0x10;
const LVCF_ORDER: u32 = 0x20;
const TEXT_CALLBACK: usize = usize::MAX;

const WM_SETFOCUS: u32 = 0x0007;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_GETDLGCODE: u32 = 0x0087;
const WM_KEYDOWN: u32 = 0x0100;
const WM_VSCROLL: u32 = 0x0115;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_MOUSEWHEEL: u32 = 0x020a;

const SELECTED: u32 = rgb(0xcc, 0xe4, 0xf7);
const SELECTED_UNFOCUSED: u32 = rgb(0xe5, 0xe5, 0xe5);
const HOVER: u32 = rgb(0xf3, 0xf3, 0xf3);
const SCROLL_THUMB: u32 = rgb(0xc2, 0xc2, 0xc2);

#[derive(Clone, Default)]
struct Row {
    /// Per subitem: the text, or None for LPSTR_TEXTCALLBACK.
    texts: Vec<Option<Vec<u16>>>,
    image: i32,
    param: isize,
    state: u32,
    indent: i32,
}

struct List {
    header: Hwnd,
    rows: Vec<Row>,
    /// LVS_OWNERDATA: the count and each item's state.
    count: usize,
    states: Vec<u32>,
    top: usize,
    images: [usize; 3],
    ex_style: u32,
    font: usize,
    background: u32,
    mark: i32,
    hot: i32,
    callback_mask: u32,
}

static LISTS: Store<List> = Store::new();

fn new_list() -> List {
    List {
        header: 0,
        rows: Vec::new(),
        count: 0,
        states: Vec::new(),
        top: 0,
        images: [0; 3],
        ex_style: 0,
        font: 0,
        background: WHITE,
        mark: -1,
        hot: -1,
        callback_mask: 0,
    }
}

fn with<R>(hwnd: Hwnd, f: impl FnOnce(&mut List) -> R) -> Option<R> {
    LISTS.with(hwnd, new_list, f)
}

fn owner_data(hwnd: Hwnd) -> bool {
    style(hwnd) & LVS_OWNERDATA != 0
}

fn count(hwnd: Hwnd) -> usize {
    let od = owner_data(hwnd);
    with(hwnd, |l| if od { l.count } else { l.rows.len() }).unwrap_or(0)
}

fn state(hwnd: Hwnd, i: usize) -> u32 {
    let od = owner_data(hwnd);
    with(hwnd, |l| {
        if od {
            l.states.get(i).copied().unwrap_or(0)
        } else {
            l.rows.get(i).map_or(0, |r| r.state)
        }
    })
    .unwrap_or(0)
}

fn put_state(hwnd: Hwnd, i: usize, s: u32) {
    let od = owner_data(hwnd);
    with(hwnd, |l| {
        if od {
            if l.states.len() <= i && l.states.try_reserve(i + 1 - l.states.len()).is_ok() {
                l.states.resize(i + 1, 0);
            }
            if let Some(x) = l.states.get_mut(i) {
                *x = s;
            }
        } else if let Some(r) = l.rows.get_mut(i) {
            r.state = s;
        }
    });
}

fn header_of(hwnd: Hwnd) -> Hwnd {
    with(hwnd, |l| l.header).unwrap_or(0)
}

fn font_of(hwnd: Hwnd) -> usize {
    with(hwnd, |l| l.font).unwrap_or(0)
}

fn row_height(hwnd: Hwnd) -> i32 {
    (line_height(font_of(hwnd)) + 8).max(20)
}

fn header_height(hwnd: Hwnd) -> i32 {
    let s = style(hwnd);
    if s & LVS_TYPEMASK != LVS_REPORT || s & LVS_NOCOLUMNHEADER != 0 {
        0
    } else {
        header::height(font_of(hwnd))
    }
}

fn per_page(hwnd: Hwnd) -> usize {
    let c = client(hwnd);
    ((c[3] - header_height(hwnd)) / row_height(hwnd)).max(1) as usize
}

/// Columns: (left, right) by column index (one full-width column when
/// there are none).
fn columns(hwnd: Hwnd) -> Vec<(i32, i32)> {
    let e = header::item_edges(header_of(hwnd));
    if e.is_empty() {
        let mut v = Vec::new();
        if v.try_reserve(1).is_ok() {
            v.push((0, client(hwnd)[2]));
        }
        return v;
    }
    e
}

/// Item `i`'s row: top and bottom.
fn row_span(hwnd: Hwnd, i: usize) -> (i32, i32) {
    let top = with(hwnd, |l| l.top).unwrap_or(0) as i32;
    let rh = row_height(hwnd);
    let y = header_height(hwnd) + (i as i32 - top) * rh;
    (y, y + rh)
}

fn row_at(hwnd: Hwnd, y: i32) -> Option<usize> {
    let hh = header_height(hwnd);
    if y < hh {
        return None;
    }
    let top = with(hwnd, |l| l.top).unwrap_or(0);
    let i = top + ((y - hh) / row_height(hwnd)) as usize;
    (i < count(hwnd)).then_some(i)
}

// ---- Notifications ----

/// NMLISTVIEW (and NMITEMACTIVATE, which adds uKeyFlags).
#[repr(C)]
struct NmListView {
    hdr: NmHdr,
    item: i32,
    sub: i32,
    new_state: u32,
    old_state: u32,
    changed: u32,
    point: [i32; 2],
    param: isize,
    keys: u32,
}

/// LVITEMW.
#[repr(C)]
#[derive(Clone, Copy)]
struct LvItem {
    mask: u32,
    item: i32,
    sub: i32,
    state: u32,
    state_mask: u32,
    text: *mut u16,
    text_max: i32,
    image: i32,
    param: isize,
    indent: i32,
    group_id: i32,
    columns: u32,
    pu_columns: *mut u32,
    col_fmt: *mut i32,
    group: i32,
}

#[repr(C)]
struct NmDispInfo {
    hdr: NmHdr,
    item: LvItem,
}

fn parent_of(hwnd: Hwnd) -> Hwnd {
    // SAFETY: plain call.
    unsafe { GetParent(hwnd) }
}

#[allow(clippy::too_many_arguments)]
fn send_lv(
    hwnd: Hwnd,
    code: i32,
    item: i32,
    sub: i32,
    new_state: u32,
    old_state: u32,
    changed: u32,
    point: [i32; 2],
) -> isize {
    let param = if item >= 0 && !owner_data(hwnd) {
        with(hwnd, |l| l.rows.get(item as usize).map_or(0, |r| r.param)).unwrap_or(0)
    } else {
        0
    };
    let mut n = NmListView {
        hdr: NmHdr { from: hwnd, id: id_of(hwnd), code: code as u32 },
        item,
        sub,
        new_state,
        old_state,
        changed,
        point,
        param,
        keys: 0,
    };
    notify(parent_of(hwnd), &mut n)
}

fn simple_notify(hwnd: Hwnd, code: i32) -> isize {
    let mut h = NmHdr { from: hwnd, id: id_of(hwnd), code: code as u32 };
    notify(parent_of(hwnd), &mut h)
}

/// An item's text in subitem `sub` and (subitem 0) its image: stored, or
/// from the parent (LVN_GETDISPINFOW).
fn text_and_image(hwnd: Hwnd, i: usize, sub: usize) -> (Vec<u16>, i32) {
    let stored = if owner_data(hwnd) {
        None
    } else {
        with(hwnd, |l| {
            l.rows
                .get(i)
                .map(|r| (r.texts.get(sub).cloned().unwrap_or(Some(Vec::new())), r.image, r.param))
        })
        .flatten()
    };
    let (text, image, param) = match stored {
        Some((Some(t), image, _)) if image != -1 || sub != 0 => return (t, image),
        Some((t, image, param)) => (t, image, param),
        None => (None, -1, 0),
    };
    // Ask the parent.
    let mut buf = [0u16; 260];
    let mut mask = 0;
    if text.is_none() {
        mask |= LVIF_TEXT;
    }
    if sub == 0 && image == -1 {
        mask |= LVIF_IMAGE;
    }
    let mut di = NmDispInfo {
        hdr: NmHdr { from: hwnd, id: id_of(hwnd), code: LVN_GETDISPINFOW as u32 },
        item: LvItem {
            mask,
            item: i as i32,
            sub: sub as i32,
            state: 0,
            state_mask: 0,
            text: buf.as_mut_ptr(),
            text_max: buf.len() as i32,
            image: 0,
            param,
            indent: 0,
            group_id: 0,
            columns: 0,
            pu_columns: core::ptr::null_mut(),
            col_fmt: core::ptr::null_mut(),
            group: 0,
        },
    };
    notify(parent_of(hwnd), &mut di);
    let text = match text {
        Some(t) => t,
        // The parent may point pszText at its own buffer.
        None => owned(wide(di.item.text, 1024)),
    };
    let image = if mask & LVIF_IMAGE != 0 { di.item.image } else { image };
    (text, image)
}

/// Sets item `i`'s state bits under `mask` to `value` (LVN_ITEMCHANGING,
/// then LVN_ITEMCHANGED when they change).
fn change_state(hwnd: Hwnd, i: usize, mask: u32, value: u32) {
    let old = state(hwnd, i);
    let new = (old & !mask) | (value & mask);
    if new == old {
        return;
    }
    if send_lv(hwnd, LVN_ITEMCHANGING, i as i32, 0, new, old, LVIF_STATE, [0, 0]) != 0 {
        return;
    }
    // One focused item at most.
    if new & LVIS_FOCUSED != 0 && old & LVIS_FOCUSED == 0 {
        let n = count(hwnd);
        for k in 0..n {
            if k != i {
                let s = state(hwnd, k);
                if s & LVIS_FOCUSED != 0 {
                    put_state(hwnd, k, s & !LVIS_FOCUSED);
                }
            }
        }
    }
    put_state(hwnd, i, new);
    send_lv(hwnd, LVN_ITEMCHANGED, i as i32, 0, new, old, LVIF_STATE, [0, 0]);
    redraw(hwnd);
    // Accessibility: the item (child id index + 1) was selected (added,
    // removed) or took the focus.
    let child = i as i32 + 1;
    if (new ^ old) & LVIS_SELECTED != 0 {
        // The only selected item: EVENT_OBJECT_SELECTION; one more of
        // several: SELECTIONADD; one less: SELECTIONREMOVE.
        let only = !multi_select(hwnd) || with_state(hwnd, LVIS_SELECTED).len() == 1;
        let event = match new & LVIS_SELECTED != 0 {
            true if only => EVENT_OBJECT_SELECTION,
            true => EVENT_OBJECT_SELECTIONADD,
            false => EVENT_OBJECT_SELECTIONREMOVE,
        };
        crate::common::win_event(event, hwnd, child);
    }
    if new & LVIS_FOCUSED != 0 && old & LVIS_FOCUSED == 0 {
        crate::common::win_event(EVENT_OBJECT_FOCUS, hwnd, child);
    }
}

const EVENT_OBJECT_FOCUS: u32 = 0x8005;
const EVENT_OBJECT_SELECTION: u32 = 0x8006;
const EVENT_OBJECT_SELECTIONADD: u32 = 0x8007;
const EVENT_OBJECT_SELECTIONREMOVE: u32 = 0x8008;

/// Whether the list lets several items be selected (no LVS_SINGLESEL).
fn multi_select(hwnd: Hwnd) -> bool {
    // SAFETY: a plain call.
    (unsafe { crate::common::GetWindowLongW(hwnd, -16) } as u32 & 0x0004) == 0
}

/// The items whose state has `mask` (for owner data: those changed).
fn with_state(hwnd: Hwnd, mask: u32) -> Vec<usize> {
    let od = owner_data(hwnd);
    with(hwnd, |l| {
        let mut v = Vec::new();
        if od {
            for (i, &s) in l.states.iter().enumerate().take(l.count) {
                if s & mask != 0 && v.try_reserve(1).is_ok() {
                    v.push(i);
                }
            }
        } else {
            for (i, r) in l.rows.iter().enumerate() {
                if r.state & mask != 0 && v.try_reserve(1).is_ok() {
                    v.push(i);
                }
            }
        }
        v
    })
    .unwrap_or_default()
}

fn select_only(hwnd: Hwnd, i: usize) {
    for k in with_state(hwnd, LVIS_SELECTED) {
        if k != i {
            change_state(hwnd, k, LVIS_SELECTED, 0);
        }
    }
    change_state(hwnd, i, LVIS_SELECTED | LVIS_FOCUSED, LVIS_SELECTED | LVIS_FOCUSED);
    with(hwnd, |l| l.mark = i as i32);
}

fn ensure_visible(hwnd: Hwnd, i: usize) {
    let page = per_page(hwnd);
    with(hwnd, |l| {
        if i < l.top {
            l.top = i;
        } else if i >= l.top + page {
            l.top = i + 1 - page;
        }
    });
    redraw(hwnd);
}

fn scroll_to(hwnd: Hwnd, top: isize) {
    let n = count(hwnd);
    let page = per_page(hwnd);
    let max = n.saturating_sub(page) as isize;
    with(hwnd, |l| l.top = top.clamp(0, max.max(0)) as usize);
    redraw(hwnd);
}

// ---- Columns (through the header) ----

/// LVCOLUMNW (64-bit): mask (0), fmt (4), cx (8), pszText (16),
/// cchTextMax (24), iSubItem (28), iImage (32), iOrder (36).
fn hd_from_lv(p: *const u8) -> [u8; 72] {
    let mut hd = [0u8; 72];
    // SAFETY: the caller's LVCOLUMNW.
    unsafe {
        let mask = p.cast::<u32>().read_unaligned();
        let mut hmask = 0u32;
        if mask & LVCF_WIDTH != 0 {
            hmask |= header::HDI_WIDTH;
            hd[4..8].copy_from_slice(&p.add(8).cast::<i32>().read_unaligned().to_le_bytes());
        }
        if mask & LVCF_TEXT != 0 {
            hmask |= header::HDI_TEXT;
            hd[8..16].copy_from_slice(&p.add(16).cast::<usize>().read_unaligned().to_le_bytes());
        }
        let mut fmt = 0i32;
        if mask & LVCF_FMT != 0 {
            hmask |= header::HDI_FORMAT;
            fmt = p.add(4).cast::<i32>().read_unaligned() & 0x3;
        }
        if mask & LVCF_IMAGE != 0 {
            hmask |= header::HDI_IMAGE | header::HDI_FORMAT;
            fmt |= 0x800;
            hd[40..44].copy_from_slice(&p.add(32).cast::<i32>().read_unaligned().to_le_bytes());
        }
        hd[28..32].copy_from_slice(&fmt.to_le_bytes());
        hd[..4].copy_from_slice(&hmask.to_le_bytes());
    }
    hd
}

fn get_column(hwnd: Hwnd, i: usize, p: *mut u8) -> isize {
    let h = header_of(hwnd);
    // SAFETY: the caller's LVCOLUMNW.
    let (mask, out, room) = unsafe {
        (
            p.cast::<u32>().read_unaligned(),
            p.add(16).cast::<usize>().read_unaligned(),
            p.add(24).cast::<i32>().read_unaligned(),
        )
    };
    let mut hd = [0u8; 72];
    let hmask = header::HDI_WIDTH
        | header::HDI_FORMAT
        | header::HDI_IMAGE
        | header::HDI_ORDER
        | if mask & LVCF_TEXT != 0 { header::HDI_TEXT } else { 0 };
    hd[..4].copy_from_slice(&hmask.to_le_bytes());
    hd[8..16].copy_from_slice(&out.to_le_bytes());
    hd[24..28].copy_from_slice(&room.to_le_bytes());
    // SAFETY: plain call; our HDITEMW.
    if unsafe { SendMessageW(h, header::HDM_GETITEMW, i, hd.as_mut_ptr() as isize) } == 0 {
        return 0;
    }
    let field = |o: usize| i32::from_le_bytes([hd[o], hd[o + 1], hd[o + 2], hd[o + 3]]);
    // SAFETY: as above.
    unsafe {
        if mask & LVCF_WIDTH != 0 {
            p.add(8).cast::<i32>().write_unaligned(field(4));
        }
        if mask & LVCF_FMT != 0 {
            p.add(4).cast::<i32>().write_unaligned(field(28) & 0x3);
        }
        if mask & LVCF_IMAGE != 0 {
            p.add(32).cast::<i32>().write_unaligned(field(40));
        }
        if mask & LVCF_ORDER != 0 {
            p.add(36).cast::<i32>().write_unaligned(field(44));
        }
        if mask & 0x8 != 0 {
            // LVCF_SUBITEM
            p.add(28).cast::<i32>().write_unaligned(i as i32);
        }
    }
    1
}

fn column_width(hwnd: Hwnd, i: usize) -> i32 {
    columns(hwnd).get(i).map_or(0, |&(l, r)| r - l)
}

fn set_column_width(hwnd: Hwnd, i: usize, cx: i32) -> isize {
    let cx = if cx < 0 {
        // LVSCW_AUTOSIZE (-1): the widest text; _USEHEADER (-2): also
        // the title (the last column fills the rest).
        let hdc_font = font_of(hwnd);
        // SAFETY: plain calls on the window's DC.
        let hdc = unsafe { GetDC(hwnd) };
        let old = select(hdc, if hdc_font != 0 { hdc_font } else { default_font() });
        let mut w = 0;
        for k in 0..count(hwnd).min(500) {
            w = w.max(text_width(hdc, &text_and_image(hwnd, k, i).0));
        }
        if cx == -2 {
            let mut title = [0u16; 260];
            let mut col = [0u8; 56];
            col[..4].copy_from_slice(&LVCF_TEXT.to_le_bytes());
            col[16..24].copy_from_slice(&(title.as_mut_ptr() as usize).to_le_bytes());
            col[24..28].copy_from_slice(&260i32.to_le_bytes());
            get_column(hwnd, i, col.as_mut_ptr());
            let n = title.iter().position(|&c| c == 0).unwrap_or(0);
            w = w.max(text_width(hdc, &title[..n]) + 8);
        }
        select(hdc, old);
        // SAFETY: the DC from GetDC.
        unsafe { ReleaseDC(hwnd, hdc) };
        w + if i == 0 { 30 } else { 16 }
    } else {
        cx
    };
    let mut hd = [0u8; 72];
    hd[..4].copy_from_slice(&header::HDI_WIDTH.to_le_bytes());
    hd[4..8].copy_from_slice(&cx.to_le_bytes());
    // SAFETY: plain call; our HDITEMW.
    let r =
        unsafe { SendMessageW(header_of(hwnd), header::HDM_SETITEMW, i, hd.as_mut_ptr() as isize) };
    redraw(hwnd);
    r
}

/// Places the header at the top, the client's width.
fn layout(hwnd: Hwnd) {
    let h = header_of(hwnd);
    if h == 0 {
        return;
    }
    let c = client(hwnd);
    let hh = header_height(hwnd);
    // SAFETY: plain call.
    unsafe { SetWindowPos(h, 0, 0, 0, c[2], hh, SWP_NOZORDER | SWP_NOACTIVATE) };
}

// ---- Items ----

/// Reads an LVITEMW's fields into a row.
fn read_item(it: &LvItem, row: &mut Row) {
    if it.mask & LVIF_TEXT != 0 {
        let text =
            if it.text as usize == TEXT_CALLBACK { None } else { Some(owned(wide(it.text, 1024))) };
        let sub = it.sub.max(0) as usize;
        if row.texts.len() <= sub && row.texts.try_reserve(sub + 1 - row.texts.len()).is_ok() {
            row.texts.resize(sub + 1, Some(Vec::new()));
        }
        if let Some(t) = row.texts.get_mut(sub) {
            *t = text;
        }
    }
    if it.sub == 0 {
        if it.mask & LVIF_IMAGE != 0 {
            row.image = it.image;
        }
        if it.mask & LVIF_PARAM != 0 {
            row.param = it.param;
        }
        if it.mask & LVIF_STATE != 0 {
            row.state = (row.state & !it.state_mask) | (it.state & it.state_mask);
        }
        if it.mask & LVIF_INDENT != 0 {
            row.indent = it.indent;
        }
    }
}

fn item_at(lparam: isize) -> Option<LvItem> {
    // SAFETY: the caller's LVITEMW.
    (lparam != 0).then(|| unsafe { *(lparam as *const LvItem) })
}

fn insert_item(hwnd: Hwnd, lparam: isize) -> isize {
    let Some(it) = item_at(lparam) else { return -1 };
    if owner_data(hwnd) {
        return -1;
    }
    let mut row = Row { image: -1, ..Row::default() };
    if row.texts.try_reserve(1).is_err() {
        return -1;
    }
    row.texts.push(Some(Vec::new()));
    let sub0 = LvItem { sub: 0, ..it };
    read_item(&sub0, &mut row);
    if it.mask & LVIF_IMAGE == 0 {
        row.image = 0;
    }
    let at = with(hwnd, |l| {
        if l.rows.try_reserve(1).is_err() {
            return -1;
        }
        let at = (it.item.max(0) as usize).min(l.rows.len());
        l.rows.insert(at, row);
        at as isize
    })
    .unwrap_or(-1);
    if at >= 0 {
        send_lv(hwnd, LVN_INSERTITEM, at as i32, 0, 0, 0, 0, [0, 0]);
        redraw(hwnd);
    }
    at
}

fn get_item(hwnd: Hwnd, lparam: isize) -> isize {
    if lparam == 0 {
        return 0;
    }
    let p = lparam as *mut LvItem;
    // SAFETY: the caller's LVITEMW.
    let it = unsafe { *p };
    let i = it.item.max(0) as usize;
    if i >= count(hwnd) {
        return 0;
    }
    let (text, image) = text_and_image(hwnd, i, it.sub.max(0) as usize);
    let (param, indent) =
        with(hwnd, |l| l.rows.get(i).map_or((0, 0), |r| (r.param, r.indent))).unwrap_or((0, 0));
    let st = state(hwnd, i);
    // SAFETY: as above.
    unsafe {
        if it.mask & LVIF_TEXT != 0 && !it.text.is_null() && it.text_max > 0 {
            let n = text.len().min(it.text_max as usize - 1);
            core::ptr::copy_nonoverlapping(text.as_ptr(), it.text, n);
            *it.text.add(n) = 0;
        }
        if it.mask & LVIF_IMAGE != 0 {
            (*p).image = image;
        }
        if it.mask & LVIF_PARAM != 0 {
            (*p).param = param;
        }
        if it.mask & LVIF_STATE != 0 {
            (*p).state = st & it.state_mask;
        }
        if it.mask & LVIF_INDENT != 0 {
            (*p).indent = indent;
        }
    }
    1
}

fn set_item(hwnd: Hwnd, lparam: isize) -> isize {
    let Some(it) = item_at(lparam) else { return 0 };
    let i = it.item.max(0) as usize;
    if i >= count(hwnd) {
        return 0;
    }
    if it.mask & LVIF_STATE != 0 && it.sub == 0 {
        change_state(hwnd, i, it.state_mask, it.state);
    }
    if owner_data(hwnd) {
        return 1;
    }
    let ok = with(hwnd, |l| match l.rows.get_mut(i) {
        Some(r) => {
            let without_state = LvItem { mask: it.mask & !LVIF_STATE, ..it };
            read_item(&without_state, r);
            true
        }
        None => false,
    })
    .unwrap_or(false);
    redraw(hwnd);
    isize::from(ok)
}

fn next_item(hwnd: Hwnd, start: isize, flags: u32) -> isize {
    let n = count(hwnd) as isize;
    let want = (if flags & 0x1 != 0 { LVIS_FOCUSED } else { 0 })
        | (if flags & 0x2 != 0 { LVIS_SELECTED } else { 0 });
    let above = flags & 0x100 != 0; // LVNI_ABOVE
    let mut i = if above { start - 1 } else { start + 1 };
    while i >= 0 && i < n {
        if want == 0 || state(hwnd, i as usize) & want == want {
            return i;
        }
        i = if above { i - 1 } else { i + 1 };
    }
    -1
}

/// LVM_GETITEMRECT's and LVM_GETSUBITEMRECT's rectangles.
fn item_rect(hwnd: Hwnd, i: usize, sub: Option<usize>, code: i32) -> Option<[i32; 4]> {
    if i >= count(hwnd) {
        return None;
    }
    let (top, bottom) = row_span(hwnd, i);
    let cols = columns(hwnd);
    let right = cols.iter().map(|c| c.1).max().unwrap_or(0).max(client(hwnd)[2]);
    let (l, r) = match sub {
        Some(s) if s > 0 => *cols.get(s)?,
        _ => cols.first().copied().unwrap_or((0, right)),
    };
    let has_image = with(hwnd, |l| l.images[1] != 0).unwrap_or(false);
    let icon = if has_image { [l + 4, top, l + 20, bottom] } else { [l, top, l, bottom] };
    Some(match code {
        1 => icon,                          // LVIR_ICON
        2 => [icon[2] + 2, top, r, bottom], // LVIR_LABEL
        3 => [l, top, r, bottom],           // LVIR_SELECTBOUNDS
        _ if sub.is_some_and(|s| s > 0) => [l, top, r, bottom],
        _ => [0, top, right, bottom], // LVIR_BOUNDS
    })
}

/// LVHITTESTINFO: pt (0), flags (8), iItem (12), iSubItem (16).
fn hit_test(hwnd: Hwnd, lparam: isize, sub: bool) -> isize {
    if lparam == 0 {
        return -1;
    }
    let p = lparam as *mut i32;
    // SAFETY: the caller's LVHITTESTINFO.
    let (x, y) = unsafe { (*p, *p.add(1)) };
    let found = row_at(hwnd, y);
    let column = columns(hwnd).iter().position(|&(l, r)| x >= l && x < r);
    // SAFETY: as above.
    unsafe {
        *p.add(2) = match found {
            Some(_) => 0xe, // LVHT_ONITEM
            None => 0x1,    // LVHT_NOWHERE
        };
        *p.add(3) = found.map_or(-1, |i| i as i32);
        if sub {
            *p.add(4) = column.map_or(-1, |c| c as i32);
        }
    }
    found.map_or(-1, |i| i as isize)
}

fn delete_item(hwnd: Hwnd, i: usize) -> isize {
    if owner_data(hwnd) || i >= count(hwnd) {
        return 0;
    }
    send_lv(hwnd, LVN_DELETEITEM, i as i32, 0, 0, 0, 0, [0, 0]);
    with(hwnd, |l| {
        if i < l.rows.len() {
            l.rows.remove(i);
        }
        if l.top > 0 && l.top >= l.rows.len() {
            l.top = l.rows.len().saturating_sub(1);
        }
    });
    redraw(hwnd);
    1
}

fn sort(hwnd: Hwnd, data: isize, compare: usize, by_index: bool) -> isize {
    if compare == 0 || owner_data(hwnd) {
        return 0;
    }
    type Compare = extern "system" fn(isize, isize, isize) -> i32;
    // SAFETY: the caller's comparison function.
    let compare: Compare = unsafe { core::mem::transmute::<usize, Compare>(compare) };
    let n = count(hwnd);
    // Insertion sort, the callback outside the lock.
    for i in 1..n {
        let mut j = i;
        while j > 0 {
            let (a, b) = if by_index {
                ((j - 1) as isize, j as isize)
            } else {
                with(hwnd, |l| (l.rows[j - 1].param, l.rows[j].param)).unwrap_or((0, 0))
            };
            if compare(a, b, data) <= 0 {
                break;
            }
            with(hwnd, |l| l.rows.swap(j - 1, j));
            j -= 1;
        }
    }
    redraw(hwnd);
    1
}

/// LVFINDINFOW: flags (0), psz (8), lParam (16).
fn find_item(hwnd: Hwnd, start: isize, lparam: isize) -> isize {
    if lparam == 0 {
        return -1;
    }
    let p = lparam as *const u8;
    // SAFETY: the caller's LVFINDINFOW.
    let (flags, text, param) = unsafe {
        (
            p.cast::<u32>().read_unaligned(),
            p.add(8).cast::<*const u16>().read_unaligned(),
            p.add(16).cast::<isize>().read_unaligned(),
        )
    };
    let want = wide(text, 1024);
    let lower = |c: u16| if (0x41..=0x5a).contains(&c) { c + 32 } else { c };
    let n = count(hwnd);
    let first = (start + 1).max(0) as usize;
    let order = (first..n).chain(if flags & 0x20 != 0 { 0..first.min(n) } else { 0..0 });
    for i in order {
        if flags & 0x1 != 0 {
            // LVFI_PARAM
            if with(hwnd, |l| l.rows.get(i).is_some_and(|r| r.param == param)).unwrap_or(false) {
                return i as isize;
            }
        } else if flags & (0x2 | 0x8) != 0 {
            let t = text_and_image(hwnd, i, 0).0;
            let matches = if flags & 0x8 != 0 {
                t.len() >= want.len() && t.iter().zip(want).all(|(&a, &b)| lower(a) == lower(b))
            } else {
                t.len() == want.len() && t.iter().zip(want).all(|(&a, &b)| lower(a) == lower(b))
            };
            if matches {
                return i as isize;
            }
        }
    }
    -1
}

// ---- Input ----

fn at(lparam: isize) -> (i32, i32) {
    (lparam as i16 as i32, (lparam >> 16) as i16 as i32)
}

fn click(hwnd: Hwnd, lparam: isize, keys: usize, code: i32) {
    // SAFETY: plain call.
    unsafe { crate::common::SetFocus(hwnd) };
    let (x, y) = at(lparam);
    let found = row_at(hwnd, y);
    let single = style(hwnd) & LVS_SINGLESEL != 0;
    match found {
        Some(i) if keys & 0x8 != 0 && !single => {
            // Ctrl: toggles.
            let s = state(hwnd, i);
            change_state(hwnd, i, LVIS_SELECTED | LVIS_FOCUSED, (s ^ LVIS_SELECTED) | LVIS_FOCUSED);
            with(hwnd, |l| l.mark = i as i32);
        }
        Some(i) if keys & 0x4 != 0 && !single => {
            // Shift: from the mark.
            let mark = with(hwnd, |l| l.mark).unwrap_or(-1).max(0) as usize;
            let (a, b) = (mark.min(i), mark.max(i));
            for k in with_state(hwnd, LVIS_SELECTED) {
                if k < a || k > b {
                    change_state(hwnd, k, LVIS_SELECTED, 0);
                }
            }
            for k in a..=b {
                change_state(hwnd, k, LVIS_SELECTED, LVIS_SELECTED);
            }
            change_state(hwnd, i, LVIS_FOCUSED, LVIS_FOCUSED);
        }
        Some(i) => {
            if code != NM_RCLICK || state(hwnd, i) & LVIS_SELECTED == 0 {
                select_only(hwnd, i);
            }
        }
        None => {
            if keys & 0x8 == 0 && code != NM_RCLICK {
                for k in with_state(hwnd, LVIS_SELECTED) {
                    change_state(hwnd, k, LVIS_SELECTED, 0);
                }
            }
        }
    }
    let item = found.map_or(-1, |i| i as i32);
    let sub = columns(hwnd).iter().position(|&(l, r)| x >= l && x < r).map_or(0, |c| c as i32);
    send_lv(hwnd, code, item, sub, 0, 0, 0, [x, y]);
}

fn key(hwnd: Hwnd, vk: usize) {
    #[repr(C)]
    struct NmKey {
        hdr: NmHdr,
        vk: u16,
        flags: u32,
    }
    let mut k = NmKey {
        hdr: NmHdr { from: hwnd, id: id_of(hwnd), code: LVN_KEYDOWN as u32 },
        vk: vk as u16,
        flags: 0,
    };
    notify(parent_of(hwnd), &mut k);
    let n = count(hwnd);
    if n == 0 {
        return;
    }
    let focused = next_item(hwnd, -1, 0x1);
    let page = per_page(hwnd) as isize;
    let target = match vk {
        0x26 => focused - 1,    // VK_UP
        0x28 => focused + 1,    // VK_DOWN
        0x24 => 0,              // VK_HOME
        0x23 => n as isize - 1, // VK_END
        0x21 => focused - page, // VK_PRIOR
        0x22 => focused + page, // VK_NEXT
        0x0d => {
            simple_notify(hwnd, NM_RETURN);
            send_lv(hwnd, LVN_ITEMACTIVATE, focused as i32, 0, 0, 0, 0, [0, 0]);
            return;
        }
        _ => return,
    };
    let target = target.clamp(0, n as isize - 1) as usize;
    select_only(hwnd, target);
    ensure_visible(hwnd, target);
}

// ---- Painting ----

fn paint_list(hwnd: Hwnd) {
    let Some((font, top, images, ex, background, hot)) =
        with(hwnd, |l| (l.font, l.top, l.images, l.ex_style, l.background, l.hot))
    else {
        return;
    };
    let s = style(hwnd);
    let n = count(hwnd);
    let hh = header_height(hwnd);
    let rh = row_height(hwnd);
    let cols = columns(hwnd);
    let order = header::order(header_of(hwnd));
    let order: Vec<usize> = if order.is_empty() { (0..cols.len()).collect() } else { order };
    let formats: Vec<i32> = (0..cols.len())
        .map(|c| {
            let mut col = [0u8; 56];
            col[..4].copy_from_slice(&LVCF_FMT.to_le_bytes());
            get_column(hwnd, c, col.as_mut_ptr());
            i32::from_le_bytes([col[4], col[5], col[6], col[7]])
        })
        .collect();
    // SAFETY: plain call.
    let focused = unsafe { crate::common::GetFocus() } == hwnd;
    let page = per_page(hwnd);
    // Gather the visible rows' texts first (they may ask the parent).
    let mut rows = Vec::new();
    let _ = rows.try_reserve(page + 1);
    for i in top..(top + page + 1).min(n) {
        let st = state(hwnd, i);
        let cells: Vec<(Vec<u16>, i32)> =
            (0..cols.len()).map(|c| text_and_image(hwnd, i, c)).collect();
        rows.push((i, st, cells));
    }
    paint(hwnd, font, |hdc, r| {
        fill(hdc, r[0], hh, r[2], r[3], background);
        transparent(hdc);
        text_color(hdc, TEXT);
        let width = cols.iter().map(|c| c.1).max().unwrap_or(r[2]).max(r[2]);
        for (i, st, cells) in &rows {
            let y = hh + (*i as i32 - top as i32) * rh;
            if st & LVIS_SELECTED != 0 && (focused || s & LVS_SHOWSELALWAYS != 0) {
                round_fill(
                    hdc,
                    [r[0] + 2, y + 1, width.min(r[2]) - 2, y + rh - 1],
                    4,
                    if focused { SELECTED } else { SELECTED_UNFOCUSED },
                );
            } else if hot == *i as i32 {
                round_fill(hdc, [r[0] + 2, y + 1, width.min(r[2]) - 2, y + rh - 1], 4, HOVER);
            }
            for &c in &order {
                let Some(&(l, rr)) = cols.get(c) else { continue };
                let Some((text, image)) = cells.get(c) else { continue };
                let mut left = l + 6;
                if c == 0 && images[1] != 0 && *image >= 0 {
                    crate::imagelist::ImageList_Draw(
                        images[1],
                        *image,
                        hdc,
                        l + 4,
                        y + (rh - 16) / 2,
                        1,
                    );
                    left = l + 24;
                }
                let align = match formats.get(c).copied().unwrap_or(0) & 0x3 {
                    1 => 0x2,
                    2 => 0x1,
                    _ => 0,
                };
                draw_text(hdc, text, [left, y, rr - 6, y + rh], align | 0x24 | 0x800 | 0x8000);
                if ex & LVS_EX_GRIDLINES != 0 {
                    fill(hdc, rr - 1, y, rr, y + rh, LINE);
                }
            }
            if ex & LVS_EX_GRIDLINES != 0 {
                fill(hdc, r[0], y + rh - 1, width, y + rh, LINE);
            }
        }
        // A thin scroll indicator when not everything fits.
        if n > page {
            let track = r[3] - hh - 4;
            let thumb = (track * page as i32 / n as i32).max(16);
            let y = hh + 2 + (track - thumb) * top as i32 / (n - page).max(1) as i32;
            round_fill(hdc, [r[2] - 5, y, r[2] - 2, y + thumb], 1, SCROLL_THUMB);
        }
    });
}

// ---- The window procedure ----

pub extern "system" fn proc_(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        0x0001 => {
            // WM_CREATE: the header.
            with(hwnd, |_| ());
            let class: [u16; 12] =
                core::array::from_fn(|i| u16::from(*b"SysHeader32\0".get(i).unwrap_or(&0)));
            let s = style(hwnd);
            let shown = s & LVS_TYPEMASK == LVS_REPORT && s & LVS_NOCOLUMNHEADER == 0;
            // WS_CHILD | HDS_BUTTONS | HDS_HORZ, visible for the report
            // view.
            let hs = 0x4000_0000 | 0x2 | if shown { 0x1000_0000 } else { 0 };
            // SAFETY: plain call.
            let h = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    core::ptr::null(),
                    hs,
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
            with(hwnd, |l| l.header = h);
            layout(hwnd);
            0
        }
        WM_DESTROY => {
            LISTS.remove(hwnd);
            // SAFETY: plain call.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_SIZE => {
            layout(hwnd);
            redraw(hwnd);
            0
        }
        WM_SETFONT => {
            with(hwnd, |l| l.font = wparam);
            let h = header_of(hwnd);
            // SAFETY: plain call.
            unsafe { SendMessageW(h, WM_SETFONT, wparam, lparam) };
            layout(hwnd);
            redraw(hwnd);
            0
        }
        WM_GETFONT => font_of(hwnd) as isize,
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint_list(hwnd);
            0
        }
        WM_NOTIFY => {
            // The header's notifications: a click is LVN_COLUMNCLICK; the
            // others go on to the parent, and sizes redraw.
            // SAFETY: the NMHDR WM_NOTIFY carries.
            let code = unsafe { (*(lparam as *const NmHdr)).code } as i32;
            if code == HDN_ITEMCLICKW {
                // SAFETY: NMHEADERW: iItem after the NMHDR.
                let item = unsafe { *(lparam as *const u8).add(24).cast::<i32>() };
                send_lv(hwnd, LVN_COLUMNCLICK, -1, item, 0, 0, 0, [0, 0]);
                return 0;
            }
            redraw(hwnd);
            // SAFETY: plain call; the same NMHDR.
            unsafe { SendMessageW(parent_of(hwnd), WM_NOTIFY, wparam, lparam) }
        }
        WM_LBUTTONDOWN => {
            click(hwnd, lparam, wparam, NM_CLICK);
            0
        }
        WM_LBUTTONDBLCLK => {
            let (_, y) = at(lparam);
            let item = row_at(hwnd, y).map_or(-1, |i| i as i32);
            send_lv(hwnd, NM_DBLCLK, item, 0, 0, 0, 0, [0, 0]);
            if item >= 0 {
                send_lv(hwnd, LVN_ITEMACTIVATE, item, 0, 0, 0, 0, [0, 0]);
            }
            0
        }
        WM_RBUTTONDOWN => {
            click(hwnd, lparam, wparam, NM_RCLICK);
            0
        }
        WM_LBUTTONUP => 0,
        WM_MOUSEMOVE => {
            let (_, y) = at(lparam);
            let hot = row_at(hwnd, y).map_or(-1, |i| i as i32);
            if with(hwnd, |l| core::mem::replace(&mut l.hot, hot) != hot).unwrap_or(false) {
                redraw(hwnd);
            }
            0
        }
        WM_MOUSEWHEEL => {
            let delta = (wparam >> 16) as i16 as isize;
            let top = with(hwnd, |l| l.top).unwrap_or(0) as isize;
            scroll_to(hwnd, top - delta / 120 * 3);
            0
        }
        WM_VSCROLL => {
            let top = with(hwnd, |l| l.top).unwrap_or(0) as isize;
            let page = per_page(hwnd) as isize;
            let to = match wparam & 0xffff {
                0 => top - 1,
                1 => top + 1,
                2 => top - page,
                3 => top + page,
                4 | 5 => (wparam >> 16) as isize,
                6 => 0,
                7 => count(hwnd) as isize,
                _ => top,
            };
            scroll_to(hwnd, to);
            0
        }
        WM_KEYDOWN => {
            key(hwnd, wparam);
            0
        }
        WM_GETDLGCODE => 0x1 | 0x80, // DLGC_WANTARROWS | DLGC_WANTCHARS
        WM_SETFOCUS => {
            simple_notify(hwnd, NM_SETFOCUS);
            redraw(hwnd);
            0
        }
        WM_KILLFOCUS => {
            simple_notify(hwnd, NM_KILLFOCUS);
            redraw(hwnd);
            0
        }
        LVM_GETHEADER => header_of(hwnd) as isize,
        LVM_INSERTCOLUMNW => {
            if lparam == 0 {
                return -1;
            }
            let mut hd = hd_from_lv(lparam as *const u8);
            // SAFETY: plain call; our HDITEMW.
            let r = unsafe {
                SendMessageW(
                    header_of(hwnd),
                    header::HDM_INSERTITEMW,
                    wparam,
                    hd.as_mut_ptr() as isize,
                )
            };
            redraw(hwnd);
            r
        }
        LVM_SETCOLUMNW => {
            if lparam == 0 {
                return 0;
            }
            let mut hd = hd_from_lv(lparam as *const u8);
            // SAFETY: plain call; our HDITEMW.
            let r = unsafe {
                SendMessageW(
                    header_of(hwnd),
                    header::HDM_SETITEMW,
                    wparam,
                    hd.as_mut_ptr() as isize,
                )
            };
            redraw(hwnd);
            r
        }
        LVM_GETCOLUMNW => {
            if lparam == 0 {
                return 0;
            }
            get_column(hwnd, wparam, lparam as *mut u8)
        }
        LVM_DELETECOLUMN => {
            // SAFETY: plain call.
            let r = unsafe { SendMessageW(header_of(hwnd), header::HDM_DELETEITEM, wparam, 0) };
            with(hwnd, |l| {
                for row in &mut l.rows {
                    if wparam < row.texts.len() {
                        row.texts.remove(wparam);
                    }
                }
            });
            redraw(hwnd);
            r
        }
        LVM_GETCOLUMNWIDTH => column_width(hwnd, wparam) as isize,
        LVM_SETCOLUMNWIDTH => set_column_width(hwnd, wparam, lparam as i16 as i32),
        LVM_SETCOLUMNORDERARRAY => {
            // SAFETY: plain call.
            let r =
                unsafe { SendMessageW(header_of(hwnd), header::HDM_SETORDERARRAY, wparam, lparam) };
            redraw(hwnd);
            r
        }
        // SAFETY: plain call.
        LVM_GETCOLUMNORDERARRAY => unsafe {
            SendMessageW(header_of(hwnd), header::HDM_GETORDERARRAY, wparam, lparam)
        },
        LVM_INSERTITEMW => insert_item(hwnd, lparam),
        LVM_GETITEMW => get_item(hwnd, lparam),
        LVM_SETITEMW => set_item(hwnd, lparam),
        LVM_SETITEMTEXTW => {
            let Some(it) = item_at(lparam) else { return 0 };
            set_item(
                hwnd,
                &LvItem { mask: LVIF_TEXT, item: wparam as i32, ..it } as *const LvItem as isize,
            )
        }
        LVM_GETITEMTEXTW => {
            let Some(it) = item_at(lparam) else { return 0 };
            if wparam >= count(hwnd) || it.text.is_null() || it.text_max <= 0 {
                return 0;
            }
            let (text, _) = text_and_image(hwnd, wparam, it.sub.max(0) as usize);
            let n = text.len().min(it.text_max as usize - 1);
            // SAFETY: the caller's buffer of cchTextMax characters.
            unsafe {
                core::ptr::copy_nonoverlapping(text.as_ptr(), it.text, n);
                *it.text.add(n) = 0;
            }
            n as isize
        }
        LVM_GETITEMCOUNT => count(hwnd) as isize,
        LVM_SETITEMCOUNT => {
            if owner_data(hwnd) {
                with(hwnd, |l| {
                    l.count = wparam;
                    l.states.truncate(wparam);
                    if l.top >= wparam {
                        l.top = 0;
                    }
                });
            }
            redraw(hwnd);
            1
        }
        LVM_DELETEITEM => delete_item(hwnd, wparam),
        LVM_DELETEALLITEMS => {
            send_lv(hwnd, LVN_DELETEALLITEMS, -1, 0, 0, 0, 0, [0, 0]);
            with(hwnd, |l| {
                l.rows.clear();
                l.states.clear();
                l.count = 0;
                l.top = 0;
                l.mark = -1;
            });
            redraw(hwnd);
            1
        }
        LVM_GETNEXTITEM => next_item(hwnd, wparam as isize, lparam as u32),
        LVM_SETITEMSTATE => {
            let Some(it) = item_at(lparam) else { return 0 };
            if wparam as isize == -1 {
                for i in 0..count(hwnd) {
                    change_state(hwnd, i, it.state_mask, it.state);
                }
                return 1;
            }
            if wparam >= count(hwnd) {
                return 0;
            }
            change_state(hwnd, wparam, it.state_mask, it.state);
            1
        }
        LVM_GETITEMSTATE => (state(hwnd, wparam) & lparam as u32) as isize,
        LVM_GETSELECTEDCOUNT => with_state(hwnd, LVIS_SELECTED).len() as isize,
        LVM_GETSELECTIONMARK => with(hwnd, |l| l.mark as isize).unwrap_or(-1),
        LVM_SETSELECTIONMARK => {
            with(hwnd, |l| core::mem::replace(&mut l.mark, lparam as i32) as isize).unwrap_or(-1)
        }
        LVM_GETHOTITEM => with(hwnd, |l| l.hot as isize).unwrap_or(-1),
        LVM_SETHOTITEM => {
            with(hwnd, |l| core::mem::replace(&mut l.hot, wparam as i32) as isize).unwrap_or(-1)
        }
        LVM_GETITEMRECT | LVM_GETSUBITEMRECT => {
            if lparam == 0 {
                return 0;
            }
            // SAFETY: the caller's RECT: left is the code, top (for a
            // subitem) the subitem.
            let [code, sub, _, _] = unsafe { *(lparam as *const [i32; 4]) };
            let sub = if msg == LVM_GETSUBITEMRECT { Some(sub.max(0) as usize) } else { None };
            match item_rect(hwnd, wparam, sub, code) {
                Some(r) => {
                    // SAFETY: as above.
                    unsafe { *(lparam as *mut [i32; 4]) = r };
                    1
                }
                None => 0,
            }
        }
        LVM_HITTEST => hit_test(hwnd, lparam, false),
        LVM_SUBITEMHITTEST => hit_test(hwnd, lparam, true),
        LVM_ENSUREVISIBLE => {
            if wparam < count(hwnd) {
                ensure_visible(hwnd, wparam);
            }
            1
        }
        LVM_GETTOPINDEX => with(hwnd, |l| l.top as isize).unwrap_or(0),
        LVM_GETCOUNTPERPAGE => per_page(hwnd) as isize,
        LVM_SCROLL => {
            let top = with(hwnd, |l| l.top).unwrap_or(0) as isize;
            scroll_to(hwnd, top + lparam as i32 as isize / row_height(hwnd) as isize);
            1
        }
        LVM_REDRAWITEMS | LVM_UPDATE | LVM_ARRANGE => {
            redraw(hwnd);
            1
        }
        LVM_SETEXTENDEDLISTVIEWSTYLE => {
            let mask = if wparam == 0 { u32::MAX } else { wparam as u32 };
            let old = with(hwnd, |l| {
                let old = l.ex_style;
                l.ex_style = (old & !mask) | (lparam as u32 & mask);
                old
            })
            .unwrap_or(0);
            redraw(hwnd);
            old as isize
        }
        LVM_GETEXTENDEDLISTVIEWSTYLE => with(hwnd, |l| l.ex_style as isize).unwrap_or(0),
        LVM_SETIMAGELIST => {
            let k = wparam.min(2);
            let old =
                with(hwnd, |l| core::mem::replace(&mut l.images[k], lparam as usize)).unwrap_or(0);
            redraw(hwnd);
            old as isize
        }
        LVM_GETIMAGELIST => with(hwnd, |l| l.images[wparam.min(2)] as isize).unwrap_or(0),
        LVM_SETBKCOLOR => {
            with(hwnd, |l| {
                l.background = if lparam as u32 == 0xffff_ffff { WHITE } else { lparam as u32 }
            });
            redraw(hwnd);
            1
        }
        LVM_GETBKCOLOR => with(hwnd, |l| l.background as isize).unwrap_or(0),
        LVM_SETTEXTCOLOR | LVM_SETTEXTBKCOLOR => 1,
        LVM_GETTEXTCOLOR => TEXT as isize,
        LVM_GETTEXTBKCOLOR => WHITE as isize,
        LVM_SETCALLBACKMASK => {
            with(hwnd, |l| l.callback_mask = wparam as u32);
            1
        }
        LVM_GETCALLBACKMASK => with(hwnd, |l| l.callback_mask as isize).unwrap_or(0),
        LVM_SORTITEMS => sort(hwnd, wparam as isize, lparam as usize, false),
        LVM_SORTITEMSEX => sort(hwnd, wparam as isize, lparam as usize, true),
        LVM_FINDITEMW => find_item(hwnd, wparam as isize, lparam),
        LVM_GETSTRINGWIDTHW => {
            let s = wide(lparam as *const u16, 1024);
            // SAFETY: plain calls on the window's DC.
            let hdc = unsafe { GetDC(hwnd) };
            let font = font_of(hwnd);
            let old = select(hdc, if font != 0 { font } else { default_font() });
            let w = text_width(hdc, s);
            select(hdc, old);
            // SAFETY: the DC from GetDC.
            unsafe { ReleaseDC(hwnd, hdc) };
            w as isize
        }
        LVM_GETITEMSPACING => ((row_height(hwnd) as isize) << 16) | 75,
        LVM_GETVIEWRECT | LVM_GETORIGIN => {
            if lparam != 0 {
                if msg == LVM_GETORIGIN {
                    // SAFETY: the caller's POINT.
                    unsafe { *(lparam as *mut [i32; 2]) = [0, 0] };
                } else {
                    let width = columns(hwnd).iter().map(|c| c.1).max().unwrap_or(0);
                    let h = header_height(hwnd) + count(hwnd) as i32 * row_height(hwnd);
                    // SAFETY: the caller's RECT.
                    unsafe { *(lparam as *mut [i32; 4]) = [0, 0, width, h] };
                }
            }
            1
        }
        LVM_GETVIEW => {
            let v = style(hwnd) & LVS_TYPEMASK;
            // LV_VIEW_*: icon 0, details 1, small icon 2, list 3.
            v as isize
        }
        LVM_SETVIEW => {
            let s = (style(hwnd) & !LVS_TYPEMASK) | (wparam as u32 & LVS_TYPEMASK);
            // SAFETY: plain call.
            unsafe { SetWindowLongW(hwnd, GWL_STYLE, s as i32) };
            layout(hwnd);
            redraw(hwnd);
            1
        }
        LVM_EDITLABELW => {
            crate::unsupported(b"comctl32.dll!LVM_EDITLABELW", wparam as u64);
            0
        }
        LVM_GETEDITCONTROL | LVM_SETBKIMAGEW | LVM_SETINFOTIP | LVM_SETSELECTEDCOLUMN => 0,
        LVM_ENABLEGROUPVIEW => -1,
        LVM_ISGROUPVIEWENABLED => 0,
        CCM_SETUNICODEFORMAT => 1,
        CCM_SETVERSION => 6,
        // SAFETY: plain call.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
