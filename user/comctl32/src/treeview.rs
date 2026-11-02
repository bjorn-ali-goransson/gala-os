//! The tree view (SysTreeView32): items (text or LPSTR_TEXTCALLBACK,
//! images, a child count or I_CHILDRENCALLBACK, a parameter, a state)
//! under the root and under each other; HTREEITEM is 0x10000 + a serial
//! number. Expanding and collapsing (TVN_ITEMEXPANDINGW/EXPANDEDW, the
//! parent may refuse), the caret (TVN_SELCHANGINGW/CHANGEDW), texts and
//! child counts asked of the parent (TVN_GETDISPINFOW), deletion with
//! TVN_DELETEITEMW. Mouse (the chevron toggles, a click selects, a double
//! click toggles; NM_CLICK, NM_DBLCLK, NM_RCLICK) and keys (arrows,
//! Home, End; TVN_KEYDOWN, NM_RETURN). Windows 11 style: chevrons
//! instead of plus boxes, a rounded light-blue selection. Not yet: label
//! editing, drag images, lines (TVS_HASLINES draws none), check boxes.

use crate::common::*;
use alloc::vec::Vec;

const TVM_INSERTITEMW: u32 = 0x1132;
const TVM_DELETEITEM: u32 = 0x1101;
const TVM_EXPAND: u32 = 0x1102;
const TVM_GETITEMRECT: u32 = 0x1104;
const TVM_GETCOUNT: u32 = 0x1105;
const TVM_GETINDENT: u32 = 0x1106;
const TVM_SETINDENT: u32 = 0x1107;
const TVM_GETIMAGELIST: u32 = 0x1108;
const TVM_SETIMAGELIST: u32 = 0x1109;
const TVM_GETNEXTITEM: u32 = 0x110a;
const TVM_SELECTITEM: u32 = 0x110b;
const TVM_GETEDITCONTROL: u32 = 0x110f;
const TVM_GETVISIBLECOUNT: u32 = 0x1110;
const TVM_HITTEST: u32 = 0x1111;
const TVM_CREATEDRAGIMAGE: u32 = 0x1112;
const TVM_SORTCHILDREN: u32 = 0x1113;
const TVM_ENSUREVISIBLE: u32 = 0x1114;
const TVM_SORTCHILDRENCB: u32 = 0x1115;
const TVM_ENDEDITLABELNOW: u32 = 0x1116;
const TVM_SETTOOLTIPS: u32 = 0x1118;
const TVM_GETTOOLTIPS: u32 = 0x1119;
const TVM_SETINSERTMARK: u32 = 0x111a;
const TVM_SETITEMHEIGHT: u32 = 0x111b;
const TVM_GETITEMHEIGHT: u32 = 0x111c;
const TVM_SETBKCOLOR: u32 = 0x111d;
const TVM_SETTEXTCOLOR: u32 = 0x111e;
const TVM_GETBKCOLOR: u32 = 0x111f;
const TVM_GETTEXTCOLOR: u32 = 0x1120;
const TVM_SETSCROLLTIME: u32 = 0x1121;
const TVM_GETITEMSTATE: u32 = 0x1127;
const TVM_SETLINECOLOR: u32 = 0x1128;
const TVM_SETEXTENDEDSTYLE: u32 = 0x112c;
const TVM_GETEXTENDEDSTYLE: u32 = 0x112d;
const TVM_GETITEMW: u32 = 0x113e;
const TVM_SETITEMW: u32 = 0x113f;
const TVM_GETISEARCHSTRINGW: u32 = 0x1140;
const TVM_EDITLABELW: u32 = 0x1141;
const CCM_SETVERSION: u32 = 0x2007;
const CCM_SETUNICODEFORMAT: u32 = 0x2005;

const TVN_SELCHANGINGW: i32 = -450;
const TVN_SELCHANGEDW: i32 = -451;
const TVN_GETDISPINFOW: i32 = -452;
const TVN_ITEMEXPANDINGW: i32 = -454;
const TVN_ITEMEXPANDEDW: i32 = -455;
const TVN_DELETEITEMW: i32 = -458;
const TVN_KEYDOWN: i32 = -412;
const NM_CLICK: i32 = -2;
const NM_DBLCLK: i32 = -3;
const NM_RETURN: i32 = -4;
const NM_RCLICK: i32 = -5;
const NM_SETFOCUS: i32 = -7;
const NM_KILLFOCUS: i32 = -8;

const TVI_ROOT: usize = (-0x10000isize) as usize;
const TVI_FIRST: usize = (-0xffffisize) as usize;
const TVI_LAST: usize = (-0xfffeisize) as usize;
const TVI_SORT: usize = (-0xfffdisize) as usize;
const TVIF_TEXT: u32 = 0x1;
const TVIF_IMAGE: u32 = 0x2;
const TVIF_PARAM: u32 = 0x4;
const TVIF_STATE: u32 = 0x8;
const TVIF_SELECTEDIMAGE: u32 = 0x20;
const TVIF_CHILDREN: u32 = 0x40;
const TVIS_SELECTED: u32 = 0x2;
const TVIS_BOLD: u32 = 0x10;
const TVIS_EXPANDED: u32 = 0x20;
const TVIS_EXPANDEDONCE: u32 = 0x40;
const TVE_COLLAPSE: usize = 0x1;
const TVE_EXPAND: usize = 0x2;
const TVE_TOGGLE: usize = 0x3;
const TVE_COLLAPSERESET: usize = 0x8000;
const TVS_HASBUTTONS: u32 = 0x1;
const TEXT_CALLBACK: usize = usize::MAX;
const CHILDREN_CALLBACK: i32 = -1;
const BASE: usize = 0x10000;

const WM_SETFOCUS: u32 = 0x0007;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_GETDLGCODE: u32 = 0x0087;
const WM_KEYDOWN: u32 = 0x0100;
const WM_VSCROLL: u32 = 0x0115;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_MOUSEWHEEL: u32 = 0x020a;

const SELECTED: u32 = rgb(0xcc, 0xe4, 0xf7);
const SELECTED_UNFOCUSED: u32 = rgb(0xe5, 0xe5, 0xe5);
const SCROLL_THUMB: u32 = rgb(0xc2, 0xc2, 0xc2);

#[derive(Clone)]
struct Node {
    id: usize,
    parent: usize,
    children: Vec<usize>,
    text: Option<Vec<u16>>,
    image: i32,
    selected_image: i32,
    child_count: i32,
    param: isize,
    state: u32,
}

struct Tree {
    nodes: Vec<Node>,
    roots: Vec<usize>,
    next: usize,
    caret: usize,
    top: usize,
    images: [usize; 2],
    indent: i32,
    item_height: i32,
    font: usize,
    tooltips: usize,
    ex_style: u32,
}

static TREES: Store<Tree> = Store::new();

fn with<R>(hwnd: Hwnd, f: impl FnOnce(&mut Tree) -> R) -> Option<R> {
    TREES.with(
        hwnd,
        || Tree {
            nodes: Vec::new(),
            roots: Vec::new(),
            next: 1,
            caret: 0,
            top: 0,
            images: [0; 2],
            indent: 19,
            item_height: 0,
            font: 0,
            tooltips: 0,
            ex_style: 0,
        },
        f,
    )
}

impl Tree {
    fn node(&self, h: usize) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == h)
    }

    fn node_mut(&mut self, h: usize) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == h)
    }

    fn siblings(&self, parent: usize) -> &Vec<usize> {
        if parent == 0 {
            &self.roots
        } else {
            self.node(parent).map_or(&self.roots, |n| &n.children)
        }
    }

    fn siblings_mut(&mut self, parent: usize) -> Option<&mut Vec<usize>> {
        if parent == 0 {
            Some(&mut self.roots)
        } else {
            self.node_mut(parent).map(|n| &mut n.children)
        }
    }

    /// The visible items (under expanded parents), in order, with their
    /// depths.
    fn visible(&self) -> Vec<(usize, i32)> {
        let mut out = Vec::new();
        let mut stack: Vec<(usize, i32)> = Vec::new();
        for &r in self.roots.iter().rev() {
            stack.push((r, 0));
        }
        while let Some((h, d)) = stack.pop() {
            if out.try_reserve(1).is_err() {
                break;
            }
            out.push((h, d));
            if let Some(n) = self.node(h)
                && n.state & TVIS_EXPANDED != 0
            {
                for &c in n.children.iter().rev() {
                    stack.push((c, d + 1));
                }
            }
        }
        out
    }

    /// `h` and everything under it.
    fn subtree(&self, h: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = Vec::new();
        stack.push(h);
        while let Some(x) = stack.pop() {
            if out.try_reserve(1).is_err() {
                break;
            }
            out.push(x);
            if let Some(n) = self.node(x) {
                stack.extend_from_slice(&n.children);
            }
        }
        out
    }
}

fn handle(id: usize) -> isize {
    if id == 0 { 0 } else { (BASE + id) as isize }
}

fn id_of_handle(h: usize) -> usize {
    h.saturating_sub(BASE)
}

fn parent_of(hwnd: Hwnd) -> Hwnd {
    // SAFETY: plain call.
    unsafe { GetParent(hwnd) }
}

// ---- Notifications ----

/// TVITEMW.
#[repr(C)]
#[derive(Clone, Copy)]
struct TvItem {
    mask: u32,
    item: usize,
    state: u32,
    state_mask: u32,
    text: *mut u16,
    text_max: i32,
    image: i32,
    selected_image: i32,
    children: i32,
    param: isize,
}

const EMPTY_ITEM: TvItem = TvItem {
    mask: 0,
    item: 0,
    state: 0,
    state_mask: 0,
    text: core::ptr::null_mut(),
    text_max: 0,
    image: 0,
    selected_image: 0,
    children: 0,
    param: 0,
};

#[repr(C)]
struct NmTreeView {
    hdr: NmHdr,
    action: u32,
    old: TvItem,
    new: TvItem,
    drag: [i32; 2],
}

fn item_for(hwnd: Hwnd, id: usize) -> TvItem {
    let (state, param) =
        with(hwnd, |t| t.node(id).map_or((0, 0), |n| (n.state, n.param))).unwrap_or((0, 0));
    TvItem {
        mask: TVIF_STATE | TVIF_PARAM | 0x10,
        item: handle(id) as usize,
        state,
        state_mask: u32::MAX,
        param,
        ..EMPTY_ITEM
    }
}

fn send_tv(hwnd: Hwnd, code: i32, action: u32, old: usize, new: usize) -> isize {
    let mut n = NmTreeView {
        hdr: NmHdr { from: hwnd, id: id_of(hwnd), code: code as u32 },
        action,
        old: if old != 0 { item_for(hwnd, old) } else { EMPTY_ITEM },
        new: if new != 0 { item_for(hwnd, new) } else { EMPTY_ITEM },
        drag: [0, 0],
    };
    notify(parent_of(hwnd), &mut n)
}

fn simple_notify(hwnd: Hwnd, code: i32) -> isize {
    let mut h = NmHdr { from: hwnd, id: id_of(hwnd), code: code as u32 };
    notify(parent_of(hwnd), &mut h)
}

/// An item's text, image (selected or not) and whether it has children,
/// asking the parent for callback parts (TVN_GETDISPINFOW).
fn display(hwnd: Hwnd, id: usize) -> (Vec<u16>, i32, bool) {
    let Some(n) = with(hwnd, |t| t.node(id).cloned()).flatten() else {
        return (Vec::new(), -1, false);
    };
    let selected = n.state & TVIS_SELECTED != 0;
    let mut mask = 0;
    if n.text.is_none() {
        mask |= TVIF_TEXT;
    }
    if n.child_count == CHILDREN_CALLBACK {
        mask |= TVIF_CHILDREN;
    }
    if n.image == -1 {
        mask |= TVIF_IMAGE;
    }
    if n.selected_image == -1 {
        mask |= TVIF_SELECTEDIMAGE;
    }
    let mut text = n.text.clone().unwrap_or_default();
    let mut image = if selected { n.selected_image } else { n.image };
    let mut has_children = !n.children.is_empty() || n.child_count > 0;
    if mask != 0 {
        let mut buf = [0u16; 260];
        #[repr(C)]
        struct DispInfo {
            hdr: NmHdr,
            item: TvItem,
        }
        let mut di = DispInfo {
            hdr: NmHdr { from: hwnd, id: id_of(hwnd), code: TVN_GETDISPINFOW as u32 },
            item: TvItem {
                mask,
                item: handle(id) as usize,
                state: n.state,
                state_mask: u32::MAX,
                text: buf.as_mut_ptr(),
                text_max: 260,
                image: n.image,
                selected_image: n.selected_image,
                children: n.child_count,
                param: n.param,
            },
        };
        notify(parent_of(hwnd), &mut di);
        if mask & TVIF_TEXT != 0 {
            text = owned(wide(di.item.text, 1024));
        }
        if mask & TVIF_CHILDREN != 0 {
            has_children = di.item.children > 0 || !n.children.is_empty();
        }
        if selected && mask & TVIF_SELECTEDIMAGE != 0 {
            image = di.item.selected_image;
        } else if !selected && mask & TVIF_IMAGE != 0 {
            image = di.item.image;
        }
    }
    (text, image, has_children)
}

// ---- Operations ----

/// TVINSERTSTRUCTW: hParent (0), hInsertAfter (8), the item from 16
/// (mask 16, hItem 24, state 32, stateMask 36, pszText 40, cchTextMax
/// 48, iImage 52, iSelectedImage 56, cChildren 60, lParam 64).
fn insert(hwnd: Hwnd, p: *const u8) -> isize {
    if p.is_null() {
        return 0;
    }
    // SAFETY: the caller's TVINSERTSTRUCTW.
    let (parent, after, mask, state, state_mask, text, image, selected_image, children, param) = unsafe {
        (
            p.cast::<usize>().read_unaligned(),
            p.add(8).cast::<usize>().read_unaligned(),
            p.add(16).cast::<u32>().read_unaligned(),
            p.add(32).cast::<u32>().read_unaligned(),
            p.add(36).cast::<u32>().read_unaligned(),
            p.add(40).cast::<usize>().read_unaligned(),
            p.add(52).cast::<i32>().read_unaligned(),
            p.add(56).cast::<i32>().read_unaligned(),
            p.add(60).cast::<i32>().read_unaligned(),
            p.add(64).cast::<isize>().read_unaligned(),
        )
    };
    let parent_id = if parent == TVI_ROOT || parent == 0 { 0 } else { id_of_handle(parent) };
    let text = if mask & TVIF_TEXT == 0 {
        Some(Vec::new())
    } else if text == TEXT_CALLBACK {
        None
    } else {
        Some(owned(wide(text as *const u16, 1024)))
    };
    let node_text = text.clone();
    let id = with(hwnd, |t| {
        if parent_id != 0 && t.node(parent_id).is_none() {
            return 0;
        }
        if t.nodes.try_reserve(1).is_err() {
            return 0;
        }
        let id = t.next;
        t.next += 1;
        let lower = |c: u16| if (0x41..=0x5a).contains(&c) { c + 32 } else { c };
        let sort_key = node_text.clone().unwrap_or_default();
        let at = {
            let sibs = t.siblings(parent_id).clone();
            if after == TVI_FIRST {
                0
            } else if after == TVI_SORT {
                sibs.iter()
                    .position(|&s| {
                        let other = t.node(s).and_then(|n| n.text.clone()).unwrap_or_default();
                        other
                            .iter()
                            .map(|&c| lower(c))
                            .cmp(sort_key.iter().map(|&c| lower(c)))
                            .is_gt()
                    })
                    .unwrap_or(sibs.len())
            } else if after == TVI_LAST || after == 0 {
                sibs.len()
            } else {
                sibs.iter().position(|&s| s == id_of_handle(after)).map_or(sibs.len(), |i| i + 1)
            }
        };
        t.nodes.push(Node {
            id,
            parent: parent_id,
            children: Vec::new(),
            text: node_text,
            image: if mask & TVIF_IMAGE != 0 { image } else { 0 },
            selected_image: if mask & TVIF_SELECTEDIMAGE != 0 { selected_image } else { 0 },
            child_count: if mask & TVIF_CHILDREN != 0 { children } else { 0 },
            param: if mask & TVIF_PARAM != 0 { param } else { 0 },
            state: if mask & TVIF_STATE != 0 { state & state_mask } else { 0 },
        });
        if let Some(s) = t.siblings_mut(parent_id)
            && s.try_reserve(1).is_ok()
        {
            s.insert(at, id);
        }
        id
    })
    .unwrap_or(0);
    let _ = text;
    redraw(hwnd);
    handle(id)
}

fn delete(hwnd: Hwnd, h: usize) -> isize {
    let ids = if h == TVI_ROOT || h == 0 {
        with(hwnd, |t| t.roots.iter().flat_map(|&r| t.subtree(r)).collect::<Vec<_>>())
            .unwrap_or_default()
    } else {
        let id = id_of_handle(h);
        if with(hwnd, |t| t.node(id).is_none()).unwrap_or(true) {
            return 0;
        }
        with(hwnd, |t| t.subtree(id)).unwrap_or_default()
    };
    for &id in ids.iter().rev() {
        send_tv(hwnd, TVN_DELETEITEMW, 0, id, 0);
    }
    with(hwnd, |t| {
        for &id in &ids {
            if let Some(i) = t.nodes.iter().position(|n| n.id == id) {
                let parent = t.nodes[i].parent;
                t.nodes.swap_remove(i);
                if let Some(s) = t.siblings_mut(parent) {
                    s.retain(|&x| x != id);
                }
            }
        }
        if ids.contains(&t.caret) {
            t.caret = 0;
        }
        t.top = 0;
    });
    redraw(hwnd);
    1
}

fn next_item(hwnd: Hwnd, code: usize, h: usize) -> isize {
    let id = id_of_handle(h);
    let r = with(hwnd, |t| -> usize {
        match code {
            0 => t.roots.first().copied().unwrap_or(0), // TVGN_ROOT
            1 | 2 => {
                // TVGN_NEXT, TVGN_PREVIOUS
                let Some(n) = t.node(id) else { return 0 };
                let sibs = t.siblings(n.parent);
                let Some(i) = sibs.iter().position(|&s| s == id) else { return 0 };
                if code == 1 {
                    sibs.get(i + 1).copied().unwrap_or(0)
                } else {
                    i.checked_sub(1).map_or(0, |j| sibs[j])
                }
            }
            3 => t.node(id).map_or(0, |n| n.parent), // TVGN_PARENT
            4 => {
                // TVGN_CHILD (of the root for NULL)
                if h == 0 || h == TVI_ROOT {
                    t.roots.first().copied().unwrap_or(0)
                } else {
                    t.node(id).and_then(|n| n.children.first().copied()).unwrap_or(0)
                }
            }
            5 => t.visible().get(t.top).map_or(0, |v| v.0), // TVGN_FIRSTVISIBLE
            6 | 7 => {
                // TVGN_NEXTVISIBLE, TVGN_PREVIOUSVISIBLE
                let v = t.visible();
                let Some(i) = v.iter().position(|x| x.0 == id) else { return 0 };
                if code == 6 {
                    v.get(i + 1).map_or(0, |x| x.0)
                } else {
                    i.checked_sub(1).map_or(0, |j| v[j].0)
                }
            }
            9 => t.caret,                                // TVGN_CARET
            10 => t.visible().last().map_or(0, |x| x.0), // TVGN_LASTVISIBLE
            _ => 0,                                      // TVGN_DROPHILITE etc.
        }
    })
    .unwrap_or(0);
    handle(r)
}

fn expand(hwnd: Hwnd, code: usize, h: usize, action: u32) -> isize {
    let id = id_of_handle(h);
    let Some(state) = with(hwnd, |t| t.node(id).map(|n| n.state)).flatten() else { return 0 };
    let (_, _, has_children) = display(hwnd, id);
    let open = state & TVIS_EXPANDED != 0;
    let want = match code & 0x3 {
        TVE_EXPAND => true,
        TVE_COLLAPSE => false,
        TVE_TOGGLE => !open,
        _ => return 0,
    };
    if want == open || (want && !has_children) {
        if code & TVE_COLLAPSERESET != 0 && !want {
            let children =
                with(hwnd, |t| t.node(id).map(|n| n.children.clone()).unwrap_or_default())
                    .unwrap_or_default();
            for c in children {
                delete(hwnd, handle(c) as usize);
            }
        }
        return 0;
    }
    let act = if want { 2 } else { 1 };
    if send_tv(hwnd, TVN_ITEMEXPANDINGW, act, 0, id) != 0 {
        return 0;
    }
    with(hwnd, |t| {
        if let Some(n) = t.node_mut(id) {
            if want {
                n.state |= TVIS_EXPANDED | TVIS_EXPANDEDONCE;
            } else {
                n.state &= !TVIS_EXPANDED;
            }
        }
    });
    if !want && code & TVE_COLLAPSERESET != 0 {
        let children = with(hwnd, |t| t.node(id).map(|n| n.children.clone()).unwrap_or_default())
            .unwrap_or_default();
        for c in children {
            delete(hwnd, handle(c) as usize);
        }
    }
    send_tv(hwnd, TVN_ITEMEXPANDEDW, act, 0, id);
    let _ = action;
    redraw(hwnd);
    1
}

fn select_item(hwnd: Hwnd, h: usize, action: u32) -> isize {
    let id = id_of_handle(h);
    let old = with(hwnd, |t| t.caret).unwrap_or(0);
    if id == old {
        return 1;
    }
    if id != 0 && with(hwnd, |t| t.node(id).is_none()).unwrap_or(true) {
        return 0;
    }
    if send_tv(hwnd, TVN_SELCHANGINGW, action, old, id) != 0 {
        return 0;
    }
    // The ancestors open so the item shows.
    let mut ancestors = Vec::new();
    let mut p = with(hwnd, |t| t.node(id).map_or(0, |n| n.parent)).unwrap_or(0);
    while p != 0 {
        if ancestors.try_reserve(1).is_ok() {
            ancestors.push(p);
        }
        p = with(hwnd, |t| t.node(p).map_or(0, |n| n.parent)).unwrap_or(0);
    }
    for a in ancestors {
        expand(hwnd, TVE_EXPAND, handle(a) as usize, 0);
    }
    with(hwnd, |t| {
        if let Some(n) = t.node_mut(old) {
            n.state &= !TVIS_SELECTED;
        }
        if let Some(n) = t.node_mut(id) {
            n.state |= TVIS_SELECTED;
        }
        t.caret = id;
    });
    send_tv(hwnd, TVN_SELCHANGEDW, action, old, id);
    ensure_visible(hwnd, id);
    redraw(hwnd);
    1
}

fn font_of(hwnd: Hwnd) -> usize {
    with(hwnd, |t| t.font).unwrap_or(0)
}

fn row_height(hwnd: Hwnd) -> i32 {
    let set = with(hwnd, |t| t.item_height).unwrap_or(0);
    if set > 0 { set } else { (line_height(font_of(hwnd)) + 8).max(20) }
}

fn per_page(hwnd: Hwnd) -> usize {
    (client(hwnd)[3] / row_height(hwnd)).max(1) as usize
}

fn ensure_visible(hwnd: Hwnd, id: usize) {
    let page = per_page(hwnd);
    with(hwnd, |t| {
        let v = t.visible();
        if let Some(i) = v.iter().position(|x| x.0 == id) {
            if i < t.top {
                t.top = i;
            } else if i >= t.top + page {
                t.top = i + 1 - page;
            }
        }
    });
}

fn scroll_to(hwnd: Hwnd, top: isize) {
    let page = per_page(hwnd);
    with(hwnd, |t| {
        let n = t.visible().len();
        t.top = top.clamp(0, n.saturating_sub(page) as isize) as usize;
    });
    redraw(hwnd);
}

/// The row of a visible item: its index from the top and its depth.
fn row_of(hwnd: Hwnd, id: usize) -> Option<(i32, i32)> {
    with(hwnd, |t| {
        let v = t.visible();
        v.iter().position(|x| x.0 == id).map(|i| (i as i32 - t.top as i32, v[i].1))
    })
    .flatten()
}

/// The x where an item of `depth` starts (its chevron).
fn indent_x(hwnd: Hwnd, depth: i32) -> i32 {
    let indent = with(hwnd, |t| t.indent).unwrap_or(19);
    4 + depth * indent
}

fn item_rect(hwnd: Hwnd, id: usize, text_only: bool) -> Option<[i32; 4]> {
    let (row, depth) = row_of(hwnd, id)?;
    let rh = row_height(hwnd);
    let y = row * rh;
    if !text_only {
        return Some([0, y, client(hwnd)[2], y + rh]);
    }
    let x = indent_x(hwnd, depth)
        + 16
        + if with(hwnd, |t| t.images[0]).unwrap_or(0) != 0 { 20 } else { 0 };
    let (text, _, _) = display(hwnd, id);
    // SAFETY: plain calls on the window's DC.
    let hdc = unsafe { GetDC(hwnd) };
    let font = font_of(hwnd);
    let old = select_object(hdc, font);
    let w = text_width(hdc, &text);
    crate::common::select(hdc, old);
    // SAFETY: the DC from GetDC.
    unsafe { ReleaseDC(hwnd, hdc) };
    Some([x, y, x + w + 6, y + rh])
}

fn select_object(hdc: usize, font: usize) -> usize {
    crate::common::select(hdc, if font != 0 { font } else { default_font() })
}

/// The item at y, and where on it x is: TVHT_ONITEMBUTTON (0x10),
/// TVHT_ONITEMICON (0x2), TVHT_ONITEMLABEL (0x4), TVHT_ONITEMINDENT
/// (0x8), TVHT_ONITEMRIGHT (0x20).
fn hit(hwnd: Hwnd, x: i32, y: i32) -> Option<(usize, u32)> {
    if y < 0 {
        return None;
    }
    let rh = row_height(hwnd);
    let (id, depth) =
        with(hwnd, |t| t.visible().get(t.top + (y / rh) as usize).copied()).flatten()?;
    let bx = indent_x(hwnd, depth);
    let has_images = with(hwnd, |t| t.images[0] != 0).unwrap_or(false);
    let flags = if x < bx {
        0x8
    } else if x < bx + 16 {
        0x10
    } else if has_images && x < bx + 36 {
        0x2
    } else {
        0x4
    };
    Some((id, flags))
}

fn at(lparam: isize) -> (i32, i32) {
    (lparam as i16 as i32, (lparam >> 16) as i16 as i32)
}

fn sort_children(hwnd: Hwnd, parent: usize, compare: Option<(usize, isize)>) -> isize {
    let pid = if parent == TVI_ROOT || parent == 0 { 0 } else { id_of_handle(parent) };
    let Some(mut kids) = with(hwnd, |t| t.siblings(pid).clone()) else { return 0 };
    type Compare = extern "system" fn(isize, isize, isize) -> i32;
    let lower = |c: u16| if (0x41..=0x5a).contains(&c) { c + 32 } else { c };
    // Insertion sort; the callback (or text comparison) outside the lock.
    for i in 1..kids.len() {
        let mut j = i;
        while j > 0 {
            let greater = match compare {
                Some((f, data)) => {
                    // SAFETY: the caller's comparison function.
                    let f: Compare = unsafe { core::mem::transmute::<usize, Compare>(f) };
                    let (a, b) = with(hwnd, |t| {
                        (
                            t.node(kids[j - 1]).map_or(0, |n| n.param),
                            t.node(kids[j]).map_or(0, |n| n.param),
                        )
                    })
                    .unwrap_or((0, 0));
                    f(a, b, data) > 0
                }
                None => {
                    let (a, b) = (display(hwnd, kids[j - 1]).0, display(hwnd, kids[j]).0);
                    a.iter().map(|&c| lower(c)).cmp(b.iter().map(|&c| lower(c))).is_gt()
                }
            };
            if !greater {
                break;
            }
            kids.swap(j - 1, j);
            j -= 1;
        }
    }
    with(hwnd, |t| {
        if let Some(s) = t.siblings_mut(pid) {
            *s = kids;
        }
    });
    redraw(hwnd);
    1
}

// ---- Items in and out ----

fn get_item(hwnd: Hwnd, p: *mut TvItem) -> isize {
    if p.is_null() {
        return 0;
    }
    // SAFETY: the caller's TVITEMW.
    let it = unsafe { *p };
    let id = id_of_handle(it.item);
    let Some(n) = with(hwnd, |t| t.node(id).cloned()).flatten() else { return 0 };
    let needs_display = it.mask & (TVIF_TEXT | TVIF_CHILDREN) != 0;
    let (text, _, has_children) =
        if needs_display { display(hwnd, id) } else { (Vec::new(), 0, false) };
    // SAFETY: as above.
    unsafe {
        if it.mask & TVIF_TEXT != 0 && !it.text.is_null() && it.text_max > 0 {
            let m = text.len().min(it.text_max as usize - 1);
            core::ptr::copy_nonoverlapping(text.as_ptr(), it.text, m);
            *it.text.add(m) = 0;
        }
        if it.mask & TVIF_IMAGE != 0 {
            (*p).image = n.image;
        }
        if it.mask & TVIF_SELECTEDIMAGE != 0 {
            (*p).selected_image = n.selected_image;
        }
        if it.mask & TVIF_CHILDREN != 0 {
            (*p).children = i32::from(has_children);
        }
        if it.mask & TVIF_PARAM != 0 {
            (*p).param = n.param;
        }
        if it.mask & TVIF_STATE != 0 {
            (*p).state = n.state & it.state_mask;
        }
    }
    1
}

fn set_item(hwnd: Hwnd, p: *const TvItem) -> isize {
    if p.is_null() {
        return 0;
    }
    // SAFETY: the caller's TVITEMW.
    let it = unsafe { *p };
    let id = id_of_handle(it.item);
    let text = if it.mask & TVIF_TEXT != 0 {
        Some(if it.text as usize == TEXT_CALLBACK {
            None
        } else {
            Some(owned(wide(it.text, 1024)))
        })
    } else {
        None
    };
    let ok = with(hwnd, |t| {
        let Some(n) = t.node_mut(id) else { return false };
        if let Some(text) = text {
            n.text = text;
        }
        if it.mask & TVIF_IMAGE != 0 {
            n.image = it.image;
        }
        if it.mask & TVIF_SELECTEDIMAGE != 0 {
            n.selected_image = it.selected_image;
        }
        if it.mask & TVIF_CHILDREN != 0 {
            n.child_count = it.children;
        }
        if it.mask & TVIF_PARAM != 0 {
            n.param = it.param;
        }
        if it.mask & TVIF_STATE != 0 {
            let m = it.state_mask & !(TVIS_SELECTED | TVIS_EXPANDED);
            n.state = (n.state & !m) | (it.state & m);
        }
        true
    })
    .unwrap_or(false);
    // Expanding through TVIF_STATE goes through TVM_EXPAND's path.
    if ok && it.mask & TVIF_STATE != 0 && it.state_mask & TVIS_EXPANDED != 0 {
        let code = if it.state & TVIS_EXPANDED != 0 { TVE_EXPAND } else { TVE_COLLAPSE };
        expand(hwnd, code, it.item, 0);
    }
    redraw(hwnd);
    isize::from(ok)
}

// ---- Input ----

fn key(hwnd: Hwnd, vk: usize) {
    #[repr(C)]
    struct NmKey {
        hdr: NmHdr,
        vk: u16,
        flags: u32,
    }
    let mut k = NmKey {
        hdr: NmHdr { from: hwnd, id: id_of(hwnd), code: TVN_KEYDOWN as u32 },
        vk: vk as u16,
        flags: 0,
    };
    notify(parent_of(hwnd), &mut k);
    let caret = with(hwnd, |t| t.caret).unwrap_or(0);
    let target = match vk {
        0x26 => next_item(hwnd, 7, handle(caret) as usize), // VK_UP
        0x28 => next_item(hwnd, 6, handle(caret) as usize), // VK_DOWN
        0x24 => next_item(hwnd, 0, 0),                      // VK_HOME
        0x23 => next_item(hwnd, 10, 0),                     // VK_END
        0x25 => {
            // VK_LEFT: collapse, or go to the parent.
            if expand(hwnd, TVE_COLLAPSE, handle(caret) as usize, 0) != 0 {
                return;
            }
            next_item(hwnd, 3, handle(caret) as usize)
        }
        0x27 => {
            // VK_RIGHT: expand, or go to the first child.
            if expand(hwnd, TVE_EXPAND, handle(caret) as usize, 0) != 0 {
                return;
            }
            next_item(hwnd, 4, handle(caret) as usize)
        }
        0x0d => {
            simple_notify(hwnd, NM_RETURN);
            return;
        }
        _ => return,
    };
    if target != 0 {
        select_item(hwnd, target as usize, 2); // TVC_BYKEYBOARD
    }
}

// ---- Painting ----

fn paint_tree(hwnd: Hwnd) {
    let Some((font, top, images)) = with(hwnd, |t| (t.font, t.top, t.images)) else { return };
    let rh = row_height(hwnd);
    let page = per_page(hwnd);
    let visible = with(hwnd, |t| t.visible()).unwrap_or_default();
    let s = style(hwnd);
    // SAFETY: plain call.
    let focused = unsafe { GetFocus() } == hwnd;
    let mut rows = Vec::new();
    let _ = rows.try_reserve(page + 1);
    for (k, &(id, depth)) in visible.iter().enumerate().skip(top).take(page + 1) {
        let (text, image, has_children) = display(hwnd, id);
        let st = with(hwnd, |t| t.node(id).map_or(0, |n| n.state)).unwrap_or(0);
        rows.push((k, depth, text, image, has_children, st));
    }
    let total = visible.len();
    paint(hwnd, font, |hdc, r| {
        fill(hdc, r[0], r[1], r[2], r[3], WHITE);
        transparent(hdc);
        for (k, depth, text, image, has_children, st) in &rows {
            let y = (*k as i32 - top as i32) * rh;
            if st & TVIS_SELECTED != 0 {
                round_fill(
                    hdc,
                    [r[0] + 2, y + 1, r[2] - 2, y + rh - 1],
                    4,
                    if focused { SELECTED } else { SELECTED_UNFOCUSED },
                );
            }
            let bx = indent_x(hwnd, *depth);
            if s & TVS_HASBUTTONS != 0 && *has_children {
                // A chevron: right when closed, down when open.
                let (cx, cy) = (bx + 7, y + rh / 2);
                for i in 0..4 {
                    if st & TVIS_EXPANDED != 0 {
                        fill(hdc, cx - i, cy + 1 - i, cx - i + 1, cy + 2 - i, TEXT_DIM);
                        fill(hdc, cx + i, cy + 1 - i, cx + i + 1, cy + 2 - i, TEXT_DIM);
                    } else {
                        // ">": the strokes lean left away from the tip.
                        fill(hdc, cx + 1 - i, cy - i, cx + 2 - i, cy - i + 1, TEXT_DIM);
                        fill(hdc, cx + 1 - i, cy + i, cx + 2 - i, cy + i + 1, TEXT_DIM);
                    }
                }
            }
            let mut x = bx + 16;
            if images[0] != 0 {
                if *image >= 0 {
                    crate::imagelist::ImageList_Draw(
                        images[0],
                        *image,
                        hdc,
                        x + 2,
                        y + (rh - 16) / 2,
                        1,
                    );
                }
                x += 20;
            }
            text_color(hdc, TEXT);
            let _ = TVIS_BOLD;
            draw_text(hdc, text, [x + 2, y, r[2] - 4, y + rh], 0x24 | 0x800 | 0x8000);
        }
        if total > page {
            let track = r[3] - 4;
            let thumb = (track * page as i32 / total as i32).max(16);
            let y = 2 + (track - thumb) * top as i32 / (total - page).max(1) as i32;
            round_fill(hdc, [r[2] - 5, y, r[2] - 2, y + thumb], 1, SCROLL_THUMB);
        }
    });
}

// ---- The window procedure ----

pub extern "system" fn proc_(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        TVM_INSERTITEMW => insert(hwnd, lparam as *const u8),
        TVM_DELETEITEM => delete(hwnd, lparam as usize),
        TVM_EXPAND => expand(hwnd, wparam, lparam as usize, 0),
        TVM_GETCOUNT => with(hwnd, |t| t.nodes.len() as isize).unwrap_or(0),
        TVM_GETNEXTITEM => next_item(hwnd, wparam, lparam as usize),
        TVM_SELECTITEM => match wparam {
            9 => select_item(hwnd, lparam as usize, 0), // TVGN_CARET
            5 => {
                // TVGN_FIRSTVISIBLE
                let id = id_of_handle(lparam as usize);
                let i = with(hwnd, |t| t.visible().iter().position(|x| x.0 == id)).flatten();
                if let Some(i) = i {
                    scroll_to(hwnd, i as isize);
                }
                isize::from(i.is_some())
            }
            _ => 1,
        },
        TVM_GETITEMW => get_item(hwnd, lparam as *mut TvItem),
        TVM_SETITEMW => set_item(hwnd, lparam as *const TvItem),
        TVM_GETITEMSTATE => {
            let id = id_of_handle(wparam);
            with(hwnd, |t| t.node(id).map_or(0, |n| (n.state & lparam as u32) as isize))
                .unwrap_or(0)
        }
        TVM_GETITEMRECT => {
            if lparam == 0 {
                return 0;
            }
            // SAFETY: the caller's RECT, holding the HTREEITEM.
            let h = unsafe { (lparam as *const usize).read_unaligned() };
            match item_rect(hwnd, id_of_handle(h), wparam != 0) {
                Some(r) => {
                    // SAFETY: as above.
                    unsafe { *(lparam as *mut [i32; 4]) = r };
                    1
                }
                None => 0,
            }
        }
        TVM_HITTEST => {
            if lparam == 0 {
                return 0;
            }
            // TVHITTESTINFO: pt (0), flags (8), hItem (16).
            // SAFETY: the caller's TVHITTESTINFO.
            let [x, y] = unsafe { *(lparam as *const [i32; 2]) };
            let found = hit(hwnd, x, y);
            // SAFETY: as above.
            unsafe {
                let p = lparam as *mut u8;
                p.add(8).cast::<u32>().write_unaligned(found.map_or(0x1, |f| f.1));
                p.add(16)
                    .cast::<usize>()
                    .write_unaligned(found.map_or(0, |f| handle(f.0) as usize));
            }
            found.map_or(0, |f| handle(f.0))
        }
        TVM_ENSUREVISIBLE => {
            let id = id_of_handle(lparam as usize);
            let mut p = with(hwnd, |t| t.node(id).map_or(0, |n| n.parent)).unwrap_or(0);
            while p != 0 {
                expand(hwnd, TVE_EXPAND, handle(p) as usize, 0);
                p = with(hwnd, |t| t.node(p).map_or(0, |n| n.parent)).unwrap_or(0);
            }
            ensure_visible(hwnd, id);
            redraw(hwnd);
            1
        }
        TVM_GETVISIBLECOUNT => per_page(hwnd) as isize,
        TVM_SETIMAGELIST => {
            let k = if wparam == 2 { 1 } else { 0 };
            let old =
                with(hwnd, |t| core::mem::replace(&mut t.images[k], lparam as usize)).unwrap_or(0);
            redraw(hwnd);
            old as isize
        }
        TVM_GETIMAGELIST => {
            with(hwnd, |t| t.images[if wparam == 2 { 1 } else { 0 }] as isize).unwrap_or(0)
        }
        TVM_SETINDENT => {
            with(hwnd, |t| t.indent = (wparam as i32).max(8));
            redraw(hwnd);
            0
        }
        TVM_GETINDENT => with(hwnd, |t| t.indent as isize).unwrap_or(19),
        TVM_SETITEMHEIGHT => {
            let old = row_height(hwnd);
            with(hwnd, |t| t.item_height = if wparam as isize == -1 { 0 } else { wparam as i32 });
            redraw(hwnd);
            old as isize
        }
        TVM_GETITEMHEIGHT => row_height(hwnd) as isize,
        TVM_SORTCHILDREN => sort_children(hwnd, lparam as usize, None),
        TVM_SORTCHILDRENCB => {
            if lparam == 0 {
                return 0;
            }
            // TVSORTCB: hParent, lpfnCompare, lParam.
            // SAFETY: the caller's TVSORTCB.
            let [parent, f, data] = unsafe { *(lparam as *const [usize; 3]) };
            sort_children(hwnd, parent, Some((f, data as isize)))
        }
        TVM_SETTOOLTIPS => {
            with(hwnd, |t| core::mem::replace(&mut t.tooltips, wparam) as isize).unwrap_or(0)
        }
        TVM_GETTOOLTIPS => with(hwnd, |t| t.tooltips as isize).unwrap_or(0),
        TVM_SETEXTENDEDSTYLE => {
            let mask = if wparam == 0 { u32::MAX } else { wparam as u32 };
            with(hwnd, |t| {
                let old = t.ex_style;
                t.ex_style = (old & !mask) | (lparam as u32 & mask);
                0
            })
            .unwrap_or(0)
        }
        TVM_GETEXTENDEDSTYLE => with(hwnd, |t| t.ex_style as isize).unwrap_or(0),
        TVM_SETBKCOLOR | TVM_SETTEXTCOLOR | TVM_SETLINECOLOR => -1,
        TVM_GETBKCOLOR | TVM_GETTEXTCOLOR => -1,
        TVM_SETINSERTMARK
        | TVM_SETSCROLLTIME
        | TVM_ENDEDITLABELNOW
        | TVM_GETISEARCHSTRINGW
        | TVM_CREATEDRAGIMAGE
        | TVM_GETEDITCONTROL => 0,
        TVM_EDITLABELW => {
            crate::unsupported(b"comctl32.dll!TVM_EDITLABELW", 0);
            0
        }
        CCM_SETVERSION => 6,
        CCM_SETUNICODEFORMAT => 1,
        WM_SETFONT => {
            with(hwnd, |t| t.font = wparam);
            redraw(hwnd);
            0
        }
        WM_GETFONT => font_of(hwnd) as isize,
        WM_ERASEBKGND => 1,
        WM_SIZE => {
            redraw(hwnd);
            0
        }
        WM_PAINT => {
            paint_tree(hwnd);
            0
        }
        WM_DESTROY => {
            delete(hwnd, TVI_ROOT);
            TREES.remove(hwnd);
            // SAFETY: plain call.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_LBUTTONDOWN => {
            // SAFETY: plain call.
            unsafe { SetFocus(hwnd) };
            let (x, y) = at(lparam);
            match hit(hwnd, x, y) {
                Some((id, 0x10)) => {
                    expand(hwnd, TVE_TOGGLE, handle(id) as usize, 1);
                }
                Some((id, _)) => {
                    select_item(hwnd, handle(id) as usize, 1); // TVC_BYMOUSE
                }
                None => {}
            }
            simple_notify(hwnd, NM_CLICK);
            0
        }
        WM_LBUTTONDBLCLK => {
            let (x, y) = at(lparam);
            if simple_notify(hwnd, NM_DBLCLK) == 0
                && let Some((id, _)) = hit(hwnd, x, y)
            {
                expand(hwnd, TVE_TOGGLE, handle(id) as usize, 1);
            }
            0
        }
        WM_RBUTTONDOWN => {
            simple_notify(hwnd, NM_RCLICK);
            0
        }
        WM_LBUTTONUP | WM_MOUSEMOVE => 0,
        WM_MOUSEWHEEL => {
            let delta = (wparam >> 16) as i16 as isize;
            let top = with(hwnd, |t| t.top).unwrap_or(0) as isize;
            scroll_to(hwnd, top - delta / 120 * 3);
            0
        }
        WM_VSCROLL => {
            let top = with(hwnd, |t| t.top).unwrap_or(0) as isize;
            let page = per_page(hwnd) as isize;
            let to = match wparam & 0xffff {
                0 => top - 1,
                1 => top + 1,
                2 => top - page,
                3 => top + page,
                4 | 5 => (wparam >> 16) as isize,
                6 => 0,
                7 => isize::MAX / 2,
                _ => top,
            };
            scroll_to(hwnd, to);
            0
        }
        WM_KEYDOWN => {
            key(hwnd, wparam);
            0
        }
        WM_GETDLGCODE => 0x1 | 0x80,
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
        // SAFETY: plain call.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
