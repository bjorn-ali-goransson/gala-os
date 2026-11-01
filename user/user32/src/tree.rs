//! Child windows and the window tree: WS_CHILD windows have a parent, an
//! id, a position relative to the parent's client area and no frame (the
//! client area is the whole window). They have no pixels of their own:
//! their DCs draw into the top-level window's buffer at their offset,
//! limited to their visible rectangle (gdi32's GalaCreateChildDC), and
//! presenting shows that part. Painting goes parent first: a window's
//! children are invalid with it (they paint over its area). Mouse input
//! goes to the deepest visible, enabled child under the pointer (or the
//! capture window), keys to the focus window. Siblings are in z order
//! (children added at the bottom, so the first created is first: the
//! dialog tab order). Also here: the window queries (GetParent,
//! GetWindow, GetAncestor, EnumChildWindows, GetDlgItem...), coordinates
//! (Get(Client|Window)Rect, ClientToScreen...), window longs, text
//! (WM_SETTEXT/WM_GETTEXT), enabling, focus and SetWindowPos/MoveWindow
//! with WM_WINDOWPOSCHANGED and DefWindowProc's WM_MOVE/WM_SIZE.

use crate::window::{
    BORDER, CAPTION, Handle, Hwnd, MAX_WINDOWS, NO_WINDOW, SendMessageW, State, Window, WndProc,
    find_class, finish_create, hwnd_of, index_of, me, set_last_error, window, with,
};
use core::ffi::c_void;
use core::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaWindowPlace(id: u32, x: i32, y: i32, w: u32, h: u32) -> u32;
    fn GalaWindowSetTitle(id: u32, title: *const u16, len: usize) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn VirtualAlloc(at: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(at: *mut c_void, size: usize, kind: u32) -> i32;
}

const WM_MOVE: u32 = 0x0003;
const WM_SIZE: u32 = 0x0005;
const WM_SETFOCUS: u32 = 0x0007;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_ENABLE: u32 = 0x000a;
const WM_SETTEXT: u32 = 0x000c;
const WM_GETTEXT: u32 = 0x000d;
const WM_GETTEXTLENGTH: u32 = 0x000e;
const WM_WINDOWPOSCHANGING: u32 = 0x0046;
const WM_WINDOWPOSCHANGED: u32 = 0x0047;
const WM_CLOSE: u32 = 0x0010;
const WM_SYSCOMMAND: u32 = 0x0112;
const SC_MINIMIZE: usize = 0xf020;
const SC_MAXIMIZE: usize = 0xf030;
const SC_CLOSE: usize = 0xf060;
const SC_RESTORE: usize = 0xf120;
const WS_MAXIMIZE: u32 = 0x0100_0000;
const WS_DISABLED: u32 = 0x0800_0000;
const CW_USEDEFAULT: i32 = 0x8000_0000u32 as i32;
const ERROR_INVALID_WINDOW_HANDLE: u32 = 1400;
const ERROR_TLW_WITH_WSCHILD: u32 = 1406;

/// Z order: top-level windows go on top (ever smaller), children at the
/// bottom (ever larger).
static TOP: AtomicI64 = AtomicI64::new(0);
static BOTTOM: AtomicI64 = AtomicI64::new(0);
static FOCUS: AtomicUsize = AtomicUsize::new(0);

pub fn next_z(child: bool) -> i64 {
    if child {
        BOTTOM.fetch_add(1, Ordering::Relaxed) + 1
    } else {
        TOP.fetch_sub(1, Ordering::Relaxed) - 1
    }
}

// ---- The tree ----

fn parent_index(s: &State, i: usize) -> Option<usize> {
    let p = s.windows[i].parent;
    (p != 0).then(|| index_of(p)).flatten().filter(|&j| s.windows[j].used)
}

/// Whether the window and all its ancestors are visible.
pub fn shown(s: &State, i: usize) -> bool {
    let mut i = i;
    for _ in 0..64 {
        if !s.windows[i].visible {
            return false;
        }
        match parent_index(s, i) {
            Some(p) => i = p,
            None => return true,
        }
    }
    false
}

pub fn ancestor_invalid(s: &State, i: usize) -> bool {
    let mut i = i;
    for _ in 0..64 {
        match parent_index(s, i) {
            Some(p) if s.windows[p].invalid => return true,
            Some(p) => i = p,
            None => return false,
        }
    }
    false
}

/// The direct children of `parent` (window indices) in z order, top first.
fn children(s: &State, parent: Hwnd, out: &mut [usize; 64]) -> usize {
    let mut n = 0;
    for i in 0..MAX_WINDOWS {
        if s.windows[i].used && s.windows[i].parent == parent && parent != 0 && n < out.len() {
            out[n] = i;
            n += 1;
        }
    }
    out[..n].sort_unstable_by_key(|&i| s.windows[i].z);
    n
}

/// The children of `hwnd` (handles; 0 after the last), top first.
pub fn children_of(hwnd: Hwnd) -> [Hwnd; 64] {
    let mut out = [0; 64];
    with(|s| {
        let mut idx = [0usize; 64];
        let n = children(s, hwnd, &mut idx);
        for k in 0..n {
            out[k] = hwnd_of(idx[k]);
        }
    });
    out
}

/// `hwnd` and its descendants, parents before children and, among
/// siblings, bottom first (the painting order); how many (at most
/// `out.len()`).
pub fn subtree(hwnd: Hwnd, out: &mut [usize; 64]) -> usize {
    out[0] = hwnd;
    let mut n = 1;
    let mut at = 0;
    while at < n {
        let kids = children_of(out[at]);
        let count = kids.iter().position(|&k| k == 0).unwrap_or(kids.len());
        for &k in kids[..count].iter().rev() {
            if n < out.len() {
                out[n] = k;
                n += 1;
            }
        }
        at += 1;
    }
    n
}

/// The window's client origin in screen coordinates.
fn screen_origin(s: &State, i: usize) -> (i32, i32) {
    let w = &s.windows[i];
    match parent_index(s, i) {
        None => (w.x + BORDER, w.y + BORDER + CAPTION),
        Some(p) => {
            let (px, py) = screen_origin(s, p);
            (px + w.x, py + w.y)
        }
    }
}

/// The top-level window above `i`, `i`'s client origin in its client area,
/// and the part of that area `i` shows in (its rectangle within every
/// ancestor's).
fn place(s: &State, i: usize) -> (usize, (i32, i32), [i32; 4]) {
    let mut chain = [0usize; 64];
    let mut n = 0;
    let mut k = i;
    loop {
        chain[n] = k;
        n += 1;
        match parent_index(s, k) {
            Some(p) if n < 64 => k = p,
            _ => break,
        }
    }
    let root = chain[n - 1];
    let r = &s.windows[root];
    let mut visible = [0, 0, r.cw, r.ch];
    let mut origin = (0, 0);
    for &c in chain[..n - 1].iter().rev() {
        let w = &s.windows[c];
        origin = (origin.0 + w.x, origin.1 + w.y);
        let rect = [origin.0, origin.1, origin.0 + w.cw, origin.1 + w.ch];
        visible = [
            visible[0].max(rect[0]),
            visible[1].max(rect[1]),
            visible[2].min(rect[2]),
            visible[3].min(rect[3]),
        ];
    }
    (root, origin, visible)
}

/// For drawing: the top-level window, the offset and the visible part.
pub fn placement(hwnd: Hwnd) -> Option<(Window, (i32, i32), [i32; 4])> {
    let i = index_of(hwnd)?;
    with(|s| {
        if !s.windows[i].used {
            return None;
        }
        let (root, origin, visible) = place(s, i);
        Some((s.windows[root], origin, visible))
    })
}

fn root_of(s: &State, i: usize) -> usize {
    place(s, i).0
}

// ---- Creation, showing, destruction ----

/// CreateWindowEx for WS_CHILD: needs a parent; no frame, no buffer.
#[allow(clippy::too_many_arguments)]
pub fn create_child(
    ex_style: u32,
    class: *const u16,
    name: *const u16,
    style: u32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    parent: Hwnd,
    id: usize,
    instance: Handle,
    param: *mut c_void,
) -> Hwnd {
    if window(parent).is_none() {
        set_last_error(ERROR_TLW_WITH_WSCHILD);
        return 0;
    }
    let (x, y) = if x == CW_USEDEFAULT { (0, 0) } else { (x, y) };
    let (w, h) = if w == CW_USEDEFAULT { (0, 0) } else { (w.max(0), h.max(0)) };
    let mut title = [0u16; 128];
    if !name.is_null() && name as usize > 0xffff {
        for (i, t) in title.iter_mut().take(127).enumerate() {
            // SAFETY: the caller's NUL-terminated string.
            let c = unsafe { *name.add(i) };
            if c == 0 {
                break;
            }
            *t = c;
        }
    }
    let created = with(|s| {
        let class = find_class(s, class)?;
        let i = s.windows.iter().position(|w| !w.used)?;
        s.windows[i] = Window {
            used: true,
            class,
            proc_: s.classes[class].proc_,
            x,
            y,
            w,
            h,
            cw: w,
            ch: h,
            style,
            invalid: true,
            erase: true,
            thread: me(),
            title,
            parent,
            id,
            ex_style,
            disabled: style & WS_DISABLED != 0,
            z: next_z(true),
            instance: instance as usize,
            ..NO_WINDOW
        };
        Some(i)
    });
    let Some(i) = created else {
        set_last_error(1411); // ERROR_CLASS_DOES_NOT_EXIST
        return 0;
    };
    finish_create(
        i,
        ex_style,
        class,
        name,
        style,
        x,
        y,
        w,
        h,
        parent,
        id as Handle,
        instance,
        param,
    )
}

/// ShowWindow for a child: its area repaints (shown: itself; hidden: the
/// parent's). Whether it was visible.
pub fn show_child(hwnd: Hwnd, show: bool) -> i32 {
    let Some(i) = index_of(hwnd) else { return 0 };
    let (was, parent) = with(|s| {
        let w = &mut s.windows[i];
        (core::mem::replace(&mut w.visible, show), w.parent)
    });
    if was != show {
        crate::window::InvalidateRect(if show { hwnd } else { parent }, core::ptr::null(), 1);
    }
    i32::from(was)
}

/// A top-level window was shown: it gets the focus unless one of its
/// windows has it.
pub fn shown_top_level(top: Hwnd) {
    let focus = FOCUS.load(Ordering::Relaxed);
    let inside = focus != 0
        && index_of(focus)
            .is_some_and(|f| with(|s| s.windows[f].used && hwnd_of(root_of(s, f)) == top));
    if !inside {
        SetFocus(top);
    }
}

/// A window is going: nothing keeps it.
pub fn forget(hwnd: Hwnd) {
    let _ = FOCUS.compare_exchange(hwnd, 0, Ordering::Relaxed, Ordering::Relaxed);
    crate::window::release_capture_of(hwnd);
}

// ---- Input ----

/// The desktop moved top-level window `top`: lParam is its client area's
/// screen position.
pub fn moved_by_desktop(top: Hwnd, lparam: isize) {
    let (cx, cy) = (lparam as i16 as i32, (lparam >> 16) as i16 as i32);
    if let Some(i) = index_of(top) {
        with(|s| {
            s.windows[i].x = cx - BORDER;
            s.windows[i].y = cy - BORDER - CAPTION;
        });
    }
}

/// Where input for top-level window `top` goes, with lParam in the
/// target's client coordinates for mouse messages.
pub fn route(top: Hwnd, message: u32, lparam: isize) -> (Hwnd, isize) {
    if (0x0100..=0x0109).contains(&message) {
        let focus = FOCUS.load(Ordering::Relaxed);
        let ok = focus != 0
            && index_of(focus)
                .is_some_and(|f| with(|s| s.windows[f].used && hwnd_of(root_of(s, f)) == top));
        return (if ok { focus } else { top }, lparam);
    }
    if !(0x0200..=0x020d).contains(&message) {
        return (top, lparam);
    }
    let (x, y) = (lparam as i16 as i32, (lparam >> 16) as i16 as i32);
    let pack = |x: i32, y: i32| ((y as i16 as u16 as isize) << 16) | (x as i16 as u16 as isize);
    // The capture (a window of this top-level window) gets it all.
    let capture = crate::window::GetCapture();
    if capture != 0 && capture != top {
        let found = index_of(capture).and_then(|c| {
            with(|s| {
                if !s.windows[c].used || hwnd_of(root_of(s, c)) != top {
                    return None;
                }
                Some(place(s, c).1)
            })
        });
        if let Some((ox, oy)) = found {
            return (capture, pack(x - ox, y - oy));
        }
    }
    // The deepest visible, enabled child under the point.
    let (target, ox, oy) = with(|s| {
        let mut at = top;
        let (mut ox, mut oy) = (0, 0);
        'down: for _ in 0..64 {
            let mut kids = [0usize; 64];
            let n = children(s, at, &mut kids);
            for &k in &kids[..n] {
                let w = &s.windows[k];
                let (l, t) = (ox + w.x, oy + w.y);
                if w.visible && x >= l && y >= t && x < l + w.cw && y < t + w.ch {
                    if w.disabled {
                        break 'down;
                    }
                    at = hwnd_of(k);
                    ox = l;
                    oy = t;
                    continue 'down;
                }
            }
            break;
        }
        (at, ox, oy)
    });
    (target, pack(x - ox, y - oy))
}

/// SetFocus: WM_KILLFOCUS to the old focus, WM_SETFOCUS to the new; the
/// old one.
#[unsafe(no_mangle)]
pub extern "system" fn SetFocus(hwnd: Hwnd) -> Hwnd {
    if hwnd != 0 && window(hwnd).is_none() {
        set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return 0;
    }
    let old = FOCUS.swap(hwnd, Ordering::Relaxed);
    if old != hwnd {
        if old != 0 && window(old).is_some() {
            SendMessageW(old, WM_KILLFOCUS, hwnd, 0);
        }
        if hwnd != 0 {
            SendMessageW(hwnd, WM_SETFOCUS, old, 0);
        }
    }
    old
}

#[unsafe(no_mangle)]
pub extern "system" fn GetFocus() -> Hwnd {
    let f = FOCUS.load(Ordering::Relaxed);
    if window(f).is_some() { f } else { 0 }
}

// ---- Queries ----

#[unsafe(no_mangle)]
pub extern "system" fn GetParent(hwnd: Hwnd) -> Hwnd {
    window(hwnd).map_or(0, |w| w.parent)
}

/// SetParent(child, new): the old parent (0 for none: top-level windows
/// cannot become children here).
#[unsafe(no_mangle)]
pub extern "system" fn SetParent(hwnd: Hwnd, parent: Hwnd) -> Hwnd {
    let Some(i) = index_of(hwnd) else { return 0 };
    if parent == 0 || window(parent).is_none() || window(hwnd).is_none_or(|w| w.parent == 0) {
        crate::window::unsupported(b"user32.dll!SetParent(top-level)", 0);
        return 0;
    }
    let old = with(|s| {
        let w = &mut s.windows[i];
        w.z = next_z(true);
        core::mem::replace(&mut w.parent, parent)
    });
    crate::window::InvalidateRect(old, core::ptr::null(), 1);
    crate::window::InvalidateRect(hwnd, core::ptr::null(), 1);
    old
}

#[unsafe(no_mangle)]
pub extern "system" fn IsChild(parent: Hwnd, hwnd: Hwnd) -> i32 {
    let mut h = hwnd;
    for _ in 0..64 {
        let p = GetParent(h);
        if p == 0 {
            return 0;
        }
        if p == parent {
            return 1;
        }
        h = p;
    }
    0
}

/// GetAncestor(hwnd, GA_PARENT 1 / GA_ROOT 2 / GA_ROOTOWNER 3).
#[unsafe(no_mangle)]
pub extern "system" fn GetAncestor(hwnd: Hwnd, flags: u32) -> Hwnd {
    let Some(i) = index_of(hwnd) else { return 0 };
    with(|s| {
        if !s.windows[i].used {
            return 0;
        }
        match flags {
            1 => s.windows[i].parent,
            2 | 3 => hwnd_of(root_of(s, i)),
            _ => 0,
        }
    })
}

/// The siblings of `hwnd` (with it) in z order, and its position.
fn siblings(hwnd: Hwnd) -> Option<([Hwnd; 64], usize, usize)> {
    let w = window(hwnd)?;
    let mut out = [0; 64];
    let n = with(|s| {
        let mut idx = [0usize; 64];
        let n = if w.parent != 0 {
            children(s, w.parent, &mut idx)
        } else {
            let mut n = 0;
            for i in 0..MAX_WINDOWS {
                if s.windows[i].used && s.windows[i].parent == 0 && n < 64 {
                    idx[n] = i;
                    n += 1;
                }
            }
            idx[..n].sort_unstable_by_key(|&i| s.windows[i].z);
            n
        };
        for k in 0..n {
            out[k] = hwnd_of(idx[k]);
        }
        n
    });
    let at = out[..n].iter().position(|&h| h == hwnd)?;
    Some((out, n, at))
}

/// GetWindow(hwnd, GW_HWNDFIRST 0 / LAST 1 / NEXT 2 / PREV 3 / OWNER 4 /
/// CHILD 5).
#[unsafe(no_mangle)]
pub extern "system" fn GetWindow(hwnd: Hwnd, cmd: u32) -> Hwnd {
    if cmd == 5 {
        return children_of(hwnd)[0];
    }
    if cmd == 4 {
        return 0;
    }
    let Some((list, n, at)) = siblings(hwnd) else { return 0 };
    match cmd {
        0 => list[0],
        1 => list[n - 1],
        2 => list.get(at + 1).copied().filter(|_| at + 1 < n).unwrap_or(0),
        3 => at.checked_sub(1).map_or(0, |p| list[p]),
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn GetTopWindow(hwnd: Hwnd) -> Hwnd {
    children_of(hwnd)[0]
}

type EnumProc = extern "system" fn(Hwnd, isize) -> i32;

/// EnumChildWindows: every descendant (depth first), until the procedure
/// returns FALSE.
#[unsafe(no_mangle)]
pub extern "system" fn EnumChildWindows(parent: Hwnd, proc_: Option<EnumProc>, data: isize) -> i32 {
    let Some(p) = proc_ else { return 0 };
    fn walk(h: Hwnd, p: EnumProc, data: isize) -> bool {
        for &c in children_of(h).iter().take_while(|&&c| c != 0) {
            if p(c, data) == 0 || !walk(c, p, data) {
                return false;
            }
        }
        true
    }
    walk(parent, p, data);
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn GetDlgItem(parent: Hwnd, id: i32) -> Hwnd {
    children_of(parent)
        .iter()
        .copied()
        .take_while(|&c| c != 0)
        .find(|&c| window(c).is_some_and(|w| w.id == id as usize))
        .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetDlgCtrlID(hwnd: Hwnd) -> i32 {
    window(hwnd).map_or(0, |w| w.id as i32)
}

// ---- Coordinates ----

#[unsafe(no_mangle)]
pub extern "system" fn GetClientRect(hwnd: Hwnd, rect: *mut [i32; 4]) -> i32 {
    let Some(w) = window(hwnd) else { return 0 };
    // SAFETY: the caller's RECT.
    unsafe { *rect = [0, 0, w.cw, w.ch] };
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn GetWindowRect(hwnd: Hwnd, rect: *mut [i32; 4]) -> i32 {
    let Some(i) = index_of(hwnd) else { return 0 };
    let r = with(|s| {
        let w = s.windows[i];
        if !w.used {
            return None;
        }
        if w.parent == 0 {
            return Some([w.x, w.y, w.x + w.w, w.y + w.h]);
        }
        let (x, y) = screen_origin(s, i);
        Some([x, y, x + w.w, y + w.h])
    });
    let Some(r) = r else { return 0 };
    // SAFETY: the caller's RECT.
    unsafe { *rect = r };
    1
}

fn origin(hwnd: Hwnd) -> Option<(i32, i32)> {
    let i = index_of(hwnd)?;
    with(|s| s.windows[i].used.then(|| screen_origin(s, i)))
}

#[unsafe(no_mangle)]
pub extern "system" fn ClientToScreen(hwnd: Hwnd, p: *mut [i32; 2]) -> i32 {
    let Some((x, y)) = origin(hwnd) else { return 0 };
    // SAFETY: the caller's POINT.
    unsafe { *p = [(*p)[0] + x, (*p)[1] + y] };
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn ScreenToClient(hwnd: Hwnd, p: *mut [i32; 2]) -> i32 {
    let Some((x, y)) = origin(hwnd) else { return 0 };
    // SAFETY: the caller's POINT.
    unsafe { *p = [(*p)[0] - x, (*p)[1] - y] };
    1
}

/// MapWindowPoints(from, to, points, n): from one client area (NULL: the
/// screen) to another; the offset (x in the low word, y in the high).
#[unsafe(no_mangle)]
pub extern "system" fn MapWindowPoints(from: Hwnd, to: Hwnd, points: *mut [i32; 2], n: u32) -> i32 {
    let a = if from == 0 { Some((0, 0)) } else { origin(from) };
    let b = if to == 0 { Some((0, 0)) } else { origin(to) };
    let (Some(a), Some(b)) = (a, b) else { return 0 };
    let (dx, dy) = (a.0 - b.0, a.1 - b.1);
    for k in 0..n as usize {
        // SAFETY: the caller's `n` POINTs.
        unsafe {
            let p = points.add(k);
            *p = [(*p)[0] + dx, (*p)[1] + dy];
        }
    }
    (dy << 16) | (dx & 0xffff)
}

// ---- Window longs ----

const GWLP_WNDPROC: i32 = -4;
const GWLP_HINSTANCE: i32 = -6;
const GWLP_HWNDPARENT: i32 = -8;
const GWLP_ID: i32 = -12;
const GWL_STYLE: i32 = -16;
const GWL_EXSTYLE: i32 = -20;
const GWLP_USERDATA: i32 = -21;

#[unsafe(no_mangle)]
pub extern "system" fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize {
    let Some(w) = window(hwnd) else {
        set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return 0;
    };
    match index {
        GWLP_WNDPROC => w.proc_.map_or(0, |p| p as usize as isize),
        GWLP_HINSTANCE => w.instance as isize,
        GWLP_HWNDPARENT => w.parent as isize,
        GWLP_ID => w.id as isize,
        GWL_STYLE => {
            (w.style
                | if w.visible { 0x1000_0000 } else { 0 }
                | if w.disabled { WS_DISABLED } else { 0 }
                | if w.zoomed { WS_MAXIMIZE } else { 0 }) as i32 as isize
        }
        GWL_EXSTYLE => w.ex_style as i32 as isize,
        GWLP_USERDATA => w.user_data,
        _ => {
            crate::window::unsupported(b"user32.dll!GetWindowLongPtrW(extra bytes)", index as u64);
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize {
    let Some(i) = index_of(hwnd) else { return 0 };
    let r = with(|s| {
        let w = &mut s.windows[i];
        if !w.used {
            return None;
        }
        Some(match index {
            GWLP_WNDPROC => {
                let old = w.proc_.map_or(0, |p| p as usize as isize);
                // SAFETY: the caller's window procedure.
                w.proc_ =
                    (value != 0).then(|| unsafe { core::mem::transmute::<isize, WndProc>(value) });
                old
            }
            GWLP_HINSTANCE => core::mem::replace(&mut w.instance, value as usize) as isize,
            GWLP_ID => core::mem::replace(&mut w.id, value as usize) as isize,
            GWL_STYLE => {
                core::mem::replace(&mut w.style, value as u32 & !0x1000_0000) as i32 as isize
            }
            GWL_EXSTYLE => core::mem::replace(&mut w.ex_style, value as u32) as i32 as isize,
            GWLP_USERDATA => core::mem::replace(&mut w.user_data, value),
            GWLP_HWNDPARENT => 0,
            _ => isize::MIN,
        })
    });
    match r {
        None => {
            set_last_error(ERROR_INVALID_WINDOW_HANDLE);
            0
        }
        Some(isize::MIN) => {
            crate::window::unsupported(b"user32.dll!SetWindowLongPtrW(extra bytes)", index as u64);
            0
        }
        Some(old) => old,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn GetWindowLongPtrA(hwnd: Hwnd, index: i32) -> isize {
    GetWindowLongPtrW(hwnd, index)
}

#[unsafe(no_mangle)]
pub extern "system" fn SetWindowLongPtrA(hwnd: Hwnd, index: i32, value: isize) -> isize {
    SetWindowLongPtrW(hwnd, index, value)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetWindowLongW(hwnd: Hwnd, index: i32) -> i32 {
    GetWindowLongPtrW(hwnd, index) as i32
}

#[unsafe(no_mangle)]
pub extern "system" fn SetWindowLongW(hwnd: Hwnd, index: i32, value: i32) -> i32 {
    SetWindowLongPtrW(hwnd, index, value as isize) as i32
}

#[unsafe(no_mangle)]
pub extern "system" fn GetWindowLongA(hwnd: Hwnd, index: i32) -> i32 {
    GetWindowLongW(hwnd, index)
}

#[unsafe(no_mangle)]
pub extern "system" fn SetWindowLongA(hwnd: Hwnd, index: i32, value: i32) -> i32 {
    SetWindowLongW(hwnd, index, value)
}

// ---- Text ----

#[unsafe(no_mangle)]
pub extern "system" fn SetWindowTextW(hwnd: Hwnd, text: *const u16) -> i32 {
    if window(hwnd).is_none() {
        set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return 0;
    }
    i32::from(SendMessageW(hwnd, WM_SETTEXT, 0, text as isize) != 0)
}

#[unsafe(no_mangle)]
pub extern "system" fn SetWindowTextA(hwnd: Hwnd, text: *const u8) -> i32 {
    let mut w = [0u16; 256];
    SetWindowTextW(hwnd, crate::window::ansi_into_ptr(text, &mut w))
}

#[unsafe(no_mangle)]
pub extern "system" fn GetWindowTextW(hwnd: Hwnd, buf: *mut u16, n: i32) -> i32 {
    if window(hwnd).is_none() || buf.is_null() || n <= 0 {
        return 0;
    }
    SendMessageW(hwnd, WM_GETTEXT, n as usize, buf as isize) as i32
}

#[unsafe(no_mangle)]
pub extern "system" fn GetWindowTextA(hwnd: Hwnd, buf: *mut u8, n: i32) -> i32 {
    let mut w = [0u16; 256];
    let len = GetWindowTextW(hwnd, w.as_mut_ptr(), 256);
    if buf.is_null() || n <= 0 {
        return 0;
    }
    let k = (len.max(0) as usize).min(n as usize - 1);
    for (i, &c) in w.iter().take(k).enumerate() {
        // SAFETY: the caller's buffer of `n` bytes.
        unsafe { *buf.add(i) = if c < 0x80 { c as u8 } else { b'?' } };
    }
    // SAFETY: as above.
    unsafe { *buf.add(k) = 0 };
    k as i32
}

#[unsafe(no_mangle)]
pub extern "system" fn GetWindowTextLengthW(hwnd: Hwnd) -> i32 {
    if window(hwnd).is_none() {
        return 0;
    }
    SendMessageW(hwnd, WM_GETTEXTLENGTH, 0, 0) as i32
}

#[unsafe(no_mangle)]
pub extern "system" fn GetWindowTextLengthA(hwnd: Hwnd) -> i32 {
    GetWindowTextLengthW(hwnd)
}

// ---- Enabling ----

/// EnableWindow: nonzero if the window was disabled; WM_ENABLE when it
/// changes.
#[unsafe(no_mangle)]
pub extern "system" fn EnableWindow(hwnd: Hwnd, enable: i32) -> i32 {
    let Some(i) = index_of(hwnd) else { return 0 };
    let Some(was) = with(|s| {
        s.windows[i].used.then(|| !core::mem::replace(&mut s.windows[i].disabled, enable == 0))
    }) else {
        return 0;
    };
    if was != (enable != 0) {
        SendMessageW(hwnd, WM_ENABLE, usize::from(enable != 0), 0);
    }
    i32::from(!was)
}

#[unsafe(no_mangle)]
pub extern "system" fn IsWindowEnabled(hwnd: Hwnd) -> i32 {
    i32::from(window(hwnd).is_some_and(|w| !w.disabled))
}

// ---- Position and size ----

/// WINDOWPOS.
#[repr(C)]
struct WindowPos {
    hwnd: Hwnd,
    after: Hwnd,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    flags: u32,
}

const SWP_NOSIZE: u32 = 0x1;
const SWP_NOMOVE: u32 = 0x2;
const SWP_NOZORDER: u32 = 0x4;
const SWP_SHOWWINDOW: u32 = 0x40;
const SWP_HIDEWINDOW: u32 = 0x80;

/// A top-level window's new client size: a buffer of that size with what
/// fits of the old pixels (white elsewhere); the old one is freed.
fn resize_buffer(i: usize, cw: i32, ch: i32) {
    let (old, ow, oh) = with(|s| (s.windows[i].pixels, s.windows[i].cw, s.windows[i].ch));
    // SAFETY: a plain call.
    let new = unsafe {
        VirtualAlloc(core::ptr::null_mut(), (cw.max(1) * ch.max(1) * 4) as usize, 0x3000, 4)
    }
    .cast::<u32>();
    if new.is_null() {
        return;
    }
    for y in 0..ch {
        for x in 0..cw {
            let v = if x < ow && y < oh && !old.is_null() {
                // SAFETY: inside the old buffer.
                unsafe { *old.add((y * ow + x) as usize) }
            } else {
                0x00ff_ffff
            };
            // SAFETY: inside the new buffer.
            unsafe { *new.add((y * cw + x) as usize) = v };
        }
    }
    with(|s| {
        s.windows[i].pixels = new;
        s.windows[i].cw = cw;
        s.windows[i].ch = ch;
    });
    if !old.is_null() {
        // SAFETY: the window's former buffer, no longer used.
        unsafe { VirtualFree(old.cast(), 0, 0x8000) };
    }
}

/// SetWindowPos: moves, sizes, reorders, shows or hides; WM_WINDOWPOSCHANGING
/// and WM_WINDOWPOSCHANGED (DefWindowProc then sends WM_MOVE and WM_SIZE).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn SetWindowPos(
    hwnd: Hwnd,
    after: Hwnd,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    flags: u32,
) -> i32 {
    let Some(i) = index_of(hwnd) else { return 0 };
    let Some(w) = window(hwnd) else {
        set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return 0;
    };
    let (nx, ny) = if flags & SWP_NOMOVE != 0 { (w.x, w.y) } else { (x, y) };
    let (nw, nh) = if flags & SWP_NOSIZE != 0 { (w.w, w.h) } else { (cx.max(0), cy.max(0)) };
    let mut pos = WindowPos { hwnd, after, x: nx, y: ny, cx: nw, cy: nh, flags };
    SendMessageW(hwnd, WM_WINDOWPOSCHANGING, 0, &mut pos as *mut WindowPos as isize);
    let top_level = w.parent == 0;
    if top_level {
        let (ncw, nch) = ((nw - 2 * BORDER).max(1), (nh - 2 * BORDER - CAPTION).max(1));
        if (nw, nh) != (w.w, w.h) {
            resize_buffer(i, ncw, nch);
        }
        with(|s| {
            let v = &mut s.windows[i];
            v.x = nx;
            v.y = ny;
            v.w = nw;
            v.h = nh;
        });
        if w.desktop != 0 {
            // SAFETY: our desktop window.
            unsafe { GalaWindowPlace(w.desktop, nx, ny, nw as u32, nh as u32) };
        }
        crate::window::InvalidateRect(hwnd, core::ptr::null(), 1);
    } else {
        with(|s| {
            let v = &mut s.windows[i];
            v.x = nx;
            v.y = ny;
            v.w = nw;
            v.h = nh;
            v.cw = nw;
            v.ch = nh;
        });
        if (nx, ny, nw, nh) != (w.x, w.y, w.w, w.h) {
            crate::window::InvalidateRect(w.parent, core::ptr::null(), 1);
        }
    }
    if flags & SWP_NOZORDER == 0 {
        let z = match after {
            0 | usize::MAX => next_z(false), // HWND_TOP, HWND_TOPMOST
            1 => next_z(true),               // HWND_BOTTOM
            sibling => window(sibling).map_or(w.z, |s| s.z + 1),
        };
        with(|s| s.windows[i].z = z);
    }
    if flags & SWP_SHOWWINDOW != 0 {
        crate::window::ShowWindow(hwnd, 5);
    }
    if flags & SWP_HIDEWINDOW != 0 {
        crate::window::ShowWindow(hwnd, 0);
    }
    SendMessageW(hwnd, WM_WINDOWPOSCHANGED, 0, &mut pos as *mut WindowPos as isize);
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn MoveWindow(
    hwnd: Hwnd,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    _repaint: i32,
) -> i32 {
    SetWindowPos(hwnd, 0, x, y, w, h, SWP_NOZORDER | 0x10)
}

// ---- Scrolling ----

/// Moves `hwnd`'s children by (dx, dy).
fn offset_children(hwnd: Hwnd, dx: i32, dy: i32) {
    for &c in children_of(hwnd).iter().take_while(|&&c| c != 0) {
        if let Some(i) = index_of(c) {
            with(|s| {
                s.windows[i].x += dx;
                s.windows[i].y += dy;
            });
        }
    }
}

/// ScrollWindow(hwnd, dx, dy, rect, clip): the client area's contents move
/// by (dx, dy); with no rectangle the children move too. gala repaints the
/// whole client area (what scrolling shows after the uncovered part is
/// painted).
#[unsafe(no_mangle)]
pub extern "system" fn ScrollWindow(
    hwnd: Hwnd,
    dx: i32,
    dy: i32,
    rect: *const [i32; 4],
    _clip: *const [i32; 4],
) -> i32 {
    if window(hwnd).is_none() {
        return 0;
    }
    if rect.is_null() {
        offset_children(hwnd, dx, dy);
    }
    crate::window::InvalidateRect(hwnd, core::ptr::null(), 1);
    1
}

/// ScrollWindowEx: SW_SCROLLCHILDREN (1) moves the children; the area is
/// repainted with SW_INVALIDATE (2); the region and update rectangle are
/// not reported (NULLREGION... SIMPLEREGION: 2).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn ScrollWindowEx(
    hwnd: Hwnd,
    dx: i32,
    dy: i32,
    _scroll: *const [i32; 4],
    _clip: *const [i32; 4],
    _region: Handle,
    update: *mut [i32; 4],
    flags: u32,
) -> i32 {
    let Some(w) = window(hwnd) else { return 0 };
    if flags & 1 != 0 {
        offset_children(hwnd, dx, dy);
    }
    if !update.is_null() {
        // SAFETY: the caller's RECT.
        unsafe { *update = [0, 0, w.cw, w.ch] };
    }
    if flags & 2 != 0 {
        crate::window::InvalidateRect(hwnd, core::ptr::null(), i32::from(flags & 4 != 0));
    }
    2
}

// ---- Maximizing ----

/// IsZoomed: the window is maximized.
#[unsafe(no_mangle)]
pub extern "system" fn IsZoomed(hwnd: Hwnd) -> i32 {
    i32::from(window(hwnd).is_some_and(|w| w.zoomed))
}

/// SC_MAXIMIZE/SC_RESTORE: the top-level window fills the work area, its
/// rectangle kept for restoring.
fn maximize(hwnd: Hwnd, on: bool) {
    let Some(i) = index_of(hwnd) else { return };
    let Some(w) = window(hwnd) else { return };
    if w.parent != 0 || w.zoomed == on {
        return;
    }
    let target = if on {
        let (_, work) = crate::monitor::screen();
        with(|s| {
            s.windows[i].restore = [w.x, w.y, w.w, w.h];
            s.windows[i].zoomed = true;
        });
        [work[0], work[1], work[2] - work[0], work[3] - work[1]]
    } else {
        with(|s| s.windows[i].zoomed = false);
        w.restore
    };
    SetWindowPos(hwnd, 0, target[0], target[1], target[2], target[3], SWP_NOZORDER | 0x10);
}

// ---- DefWindowProc's part ----

/// What DefWindowProc does for the messages of this module.
pub fn def_window_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> Option<isize> {
    match msg {
        // The window menu's commands (and the caption buttons'): close,
        // maximize, restore; minimizing is the desktop's own.
        WM_SYSCOMMAND => {
            match wparam & 0xfff0 {
                SC_CLOSE => {
                    SendMessageW(hwnd, WM_CLOSE, 0, 0);
                }
                SC_MAXIMIZE => maximize(hwnd, true),
                SC_RESTORE => maximize(hwnd, false),
                SC_MINIMIZE => {}
                _ => {}
            }
            Some(0)
        }
        WM_SETTEXT => {
            let i = index_of(hwnd)?;
            let mut title = [0u16; 128];
            let text = lparam as *const u16;
            let mut len = 0;
            if !text.is_null() {
                while len < 127 {
                    // SAFETY: the caller's NUL-terminated string.
                    let c = unsafe { *text.add(len) };
                    if c == 0 {
                        break;
                    }
                    title[len] = c;
                    len += 1;
                }
            }
            let desktop = with(|s| {
                s.windows[i].title = title;
                (s.windows[i].parent == 0).then_some(s.windows[i].desktop)
            });
            if let Some(d) = desktop.filter(|&d| d != 0) {
                // SAFETY: our desktop window; `len` characters.
                unsafe { GalaWindowSetTitle(d, title.as_ptr(), len) };
            }
            Some(1)
        }
        WM_GETTEXT => {
            let w = window(hwnd)?;
            let buf = lparam as *mut u16;
            if buf.is_null() || wparam == 0 {
                return Some(0);
            }
            let len = w.title.iter().position(|&c| c == 0).unwrap_or(127).min(wparam - 1);
            // SAFETY: the caller's buffer of wParam characters.
            unsafe {
                core::ptr::copy_nonoverlapping(w.title.as_ptr(), buf, len);
                *buf.add(len) = 0;
            }
            Some(len as isize)
        }
        WM_GETTEXTLENGTH => {
            let w = window(hwnd)?;
            Some(w.title.iter().position(|&c| c == 0).unwrap_or(127) as isize)
        }
        WM_WINDOWPOSCHANGED => {
            let i = index_of(hwnd)?;
            let (moved, size) = with(|s| {
                let w = &s.windows[i];
                let at = if w.parent == 0 { screen_origin(s, i) } else { (w.x, w.y) };
                (at, (w.cw, w.ch))
            });
            // SAFETY: the WINDOWPOS SetWindowPos passes.
            let flags =
                if lparam == 0 { 0 } else { unsafe { (*(lparam as *const WindowPos)).flags } };
            if flags & SWP_NOMOVE == 0 {
                let lp =
                    ((moved.1 as i16 as u16 as isize) << 16) | (moved.0 as i16 as u16 as isize);
                SendMessageW(hwnd, WM_MOVE, 0, lp);
            }
            if flags & SWP_NOSIZE == 0 {
                let lp = ((size.1 as u16 as isize) << 16) | (size.0 as u16 as isize);
                SendMessageW(hwnd, WM_SIZE, 0, lp);
            }
            Some(0)
        }
        _ => None,
    }
}
