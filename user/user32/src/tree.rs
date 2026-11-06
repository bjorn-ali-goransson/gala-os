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
    Handle, Hwnd, MAX_WINDOWS, NO_WINDOW, SendMessageW, State, Window, WndProc, find_class,
    finish_create, hwnd_of, index_of, me, set_last_error, window, with,
};
use core::ffi::c_void;
use core::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaWindowPlace(id: u32, x: i32, y: i32, w: u32, h: u32) -> u32;
    fn GalaWindowSetTitle(id: u32, title: *const u16, len: usize) -> u32;
    fn GalaWindowShow(id: u32, show: u8) -> u32;
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
const WS_MINIMIZE: u32 = 0x2000_0000;
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
    // Another process's window: its children from the registry.
    if hwnd != 0 && index_of(hwnd).is_none() && crate::foreign::remote(hwnd).is_some() {
        let mut all = [0; 256];
        let n = crate::foreign::children(hwnd, &mut all).min(64);
        out[..n].copy_from_slice(&all[..n]);
        return out;
    }
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
        None => {
            let (b, top) = crate::window::frame(w);
            (w.x + b, w.y + top)
        }
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

/// `place` for code that holds the state: the top-level window's index,
/// `i`'s client origin in its client area and the part it shows in.
pub fn place_in(s: &State, i: usize) -> (usize, (i32, i32), [i32; 4]) {
    place(s, i)
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
    crate::foreign::assign(i);
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
    crate::foreign::sync(hwnd);
    if was != show {
        crate::window::InvalidateRect(if show { hwnd } else { parent }, core::ptr::null(), 1);
        use crate::winevent::{
            CHILDID_SELF, EVENT_OBJECT_HIDE, EVENT_OBJECT_SHOW, OBJID_WINDOW, raise,
        };
        let event = if show { EVENT_OBJECT_SHOW } else { EVENT_OBJECT_HIDE };
        raise(event, hwnd, OBJID_WINDOW, CHILDID_SELF);
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
            let (b, top) = crate::window::desktop_frame(&s.windows[i]);
            s.windows[i].x = cx - b;
            s.windows[i].y = cy - top;
        });
        crate::foreign::sync_tree(top);
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
    // The desktop's client area starts with the menu bar.
    let bar = window(top).map_or(0, |w| w.bar);
    let (x, y) = (lparam as i16 as i32, (lparam >> 16) as i16 as i32 - bar);
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
            use crate::winevent::{CHILDID_SELF, EVENT_OBJECT_FOCUS, OBJID_CLIENT, raise};
            raise(EVENT_OBJECT_FOCUS, hwnd, OBJID_CLIENT, CHILDID_SELF);
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

/// The desktop window's handle: below the window table, so no window of
/// it. The desktop is only a name so far (GetDesktopWindow, IsWindow,
/// GetWindowRect: the screen); top-level windows have parent 0, and a
/// window made with it as parent is not treated as top-level yet.
pub const DESKTOP: Hwnd = 0xfff0;

#[unsafe(no_mangle)]
pub extern "system" fn GetDesktopWindow() -> Hwnd {
    DESKTOP
}

/// GetWindowThreadProcessId: the creating thread, and this process (every
/// window gala's user32 knows is the process's own).
#[unsafe(no_mangle)]
pub extern "system" fn GetWindowThreadProcessId(hwnd: Hwnd, pid: *mut u32) -> u32 {
    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn GetCurrentProcessId() -> u32;
    }
    let Some(w) = window(hwnd) else {
        // Another process's window.
        if let Some(info) = crate::foreign::remote(hwnd) {
            if !pid.is_null() {
                // SAFETY: the caller's DWORD.
                unsafe { *pid = info.pid };
            }
            return info.tid;
        }
        set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return 0;
    };
    if !pid.is_null() {
        // SAFETY: the caller's DWORD; a plain call.
        unsafe { *pid = GetCurrentProcessId() };
    }
    w.thread
}

#[unsafe(no_mangle)]
pub extern "system" fn GetParent(hwnd: Hwnd) -> Hwnd {
    match window(hwnd) {
        Some(w) => w.parent,
        None => crate::foreign::remote(hwnd).map_or(0, |i| i.parent),
    }
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
    crate::foreign::sync_tree(hwnd);
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
    let Some(i) = index_of(hwnd) else {
        // Another process's window: up its parents in the registry.
        let Some(info) = crate::foreign::remote(hwnd) else { return 0 };
        return match flags {
            1 => info.parent,
            2 | 3 => {
                let mut cur = info;
                for _ in 0..64 {
                    match crate::foreign::query(cur.parent) {
                        Some(p) if cur.parent != 0 => cur = p,
                        _ => break,
                    }
                }
                cur.hwnd
            }
            _ => 0,
        };
    };
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
    let Some(w) = window(hwnd) else {
        // Another process's window: its siblings from the registry.
        let info = crate::foreign::remote(hwnd)?;
        let mut all = [0; 256];
        let n = crate::foreign::children(info.parent, &mut all).min(64);
        let mut out = [0; 64];
        out[..n].copy_from_slice(&all[..n]);
        let at = out[..n].iter().position(|&h| h == hwnd)?;
        return Some((out, n, at));
    };
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
        return match window(hwnd) {
            Some(w) => w.owner,
            None => crate::foreign::remote(hwnd).map_or(0, |i| i.owner),
        };
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

/// The top-level windows (of thread `thread`, or all with 0), highest
/// first; how many.
fn top_levels(thread: u32, out: &mut [Hwnd; 256]) -> usize {
    with(|s| {
        let mut n = 0;
        for (i, w) in s.windows.iter().enumerate() {
            if n < out.len() && w.used && w.parent == 0 && (thread == 0 || w.thread == thread) {
                out[n] = hwnd_of(i);
                n += 1;
            }
        }
        out[..n].sort_unstable_by_key(|&h| index_of(h).map_or(0, |i| s.windows[i].z));
        n
    })
}

/// EnumThreadWindows(thread, proc, data): the thread's top-level windows;
/// FALSE when the procedure stops it or there are none.
#[unsafe(no_mangle)]
pub extern "system" fn EnumThreadWindows(thread: u32, proc_: Option<EnumProc>, data: isize) -> i32 {
    let Some(p) = proc_ else { return 0 };
    if thread == 0 {
        return 0;
    }
    let mut all = [0; 256];
    let n = top_levels(thread, &mut all);
    i32::from(n > 0 && all[..n].iter().all(|&h| p(h, data) != 0))
}

/// EnumWindows(proc, data): every top-level window; FALSE when the
/// procedure stops it.
#[unsafe(no_mangle)]
pub extern "system" fn EnumWindows(proc_: Option<EnumProc>, data: isize) -> i32 {
    let Some(p) = proc_ else { return 0 };
    // Every process's top-level windows (the kernel's registry).
    if let Some(r) = crate::foreign::enum_top_levels(p, data) {
        return r;
    }
    let mut all = [0; 256];
    let n = top_levels(0, &mut all);
    i32::from(all[..n].iter().all(|&h| p(h, data) != 0))
}

#[unsafe(no_mangle)]
pub extern "system" fn GetDlgItem(parent: Hwnd, id: i32) -> Hwnd {
    children_of(parent)
        .iter()
        .copied()
        .take_while(|&c| c != 0)
        .find(|&c| GetDlgCtrlID(c) == id)
        .unwrap_or(0)
}

/// SetDlgItemTextW: SetWindowTextW of the dialog's control `id` (FALSE,
/// ERROR_CONTROL_ID_NOT_FOUND, when there is none).
#[unsafe(no_mangle)]
pub extern "system" fn SetDlgItemTextW(dialog: Hwnd, id: i32, text: *const u16) -> i32 {
    match GetDlgItem(dialog, id) {
        0 => {
            crate::set_last_error(1421);
            0
        }
        c => SetWindowTextW(c, text),
    }
}

/// GetDlgItemTextW: GetWindowTextW of the dialog's control `id` (0 and an
/// empty buffer when there is none).
#[unsafe(no_mangle)]
pub extern "system" fn GetDlgItemTextW(dialog: Hwnd, id: i32, buf: *mut u16, n: i32) -> u32 {
    match GetDlgItem(dialog, id) {
        0 => {
            if !buf.is_null() && n > 0 {
                // SAFETY: the caller's buffer of n characters.
                unsafe { *buf = 0 };
            }
            crate::set_last_error(1421);
            0
        }
        c => GetWindowTextW(c, buf, n).max(0) as u32,
    }
}

/// CheckDlgButton: BM_SETCHECK to the dialog's button `id`.
#[unsafe(no_mangle)]
pub extern "system" fn CheckDlgButton(dialog: Hwnd, id: i32, check: u32) -> i32 {
    const BM_SETCHECK: u32 = 0x00f1;
    match GetDlgItem(dialog, id) {
        0 => {
            crate::set_last_error(1421);
            0
        }
        c => {
            crate::window::SendMessageW(c, BM_SETCHECK, check as usize, 0);
            1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn GetDlgCtrlID(hwnd: Hwnd) -> i32 {
    match window(hwnd) {
        Some(w) => w.id as i32,
        None => crate::foreign::remote(hwnd).map_or(0, |i| i.id as i32),
    }
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
    if hwnd == DESKTOP {
        // SAFETY: the caller's RECT.
        unsafe { *rect = crate::monitor::screen().0 };
        return 1;
    }
    let Some(i) = index_of(hwnd) else {
        let Some(info) = crate::foreign::remote(hwnd) else { return 0 };
        // SAFETY: the caller's RECT.
        unsafe { *rect = info.rect };
        return 1;
    };
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
        if let Some(info) = crate::foreign::remote(hwnd) {
            return match index {
                GWLP_HWNDPARENT => info.parent as isize,
                GWLP_ID => info.id as isize,
                GWL_STYLE => info.style as i32 as isize,
                GWL_EXSTYLE => info.ex_style as i32 as isize,
                _ => 0,
            };
        }
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
                | if w.zoomed { WS_MAXIMIZE } else { 0 }
                | if w.iconic { WS_MINIMIZE } else { 0 }) as i32 as isize
        }
        GWL_EXSTYLE => w.ex_style as i32 as isize,
        GWLP_USERDATA => w.user_data,
        _ => match crate::dialog::get_long(hwnd, index) {
            Some(v) => v,
            None => {
                crate::window::unsupported(
                    b"user32.dll!GetWindowLongPtrW(extra bytes)",
                    index as u64,
                );
                0
            }
        },
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
        Some(isize::MIN) => match crate::dialog::set_long(hwnd, index, value) {
            Some(old) => old,
            None => {
                crate::window::unsupported(
                    b"user32.dll!SetWindowLongPtrW(extra bytes)",
                    index as u64,
                );
                0
            }
        },
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
    if window(hwnd).is_none() {
        // Another process's window: its text as last mirrored. TODO:
        // WM_GETTEXT sent across processes (controls keep their own).
        return crate::foreign::remote(hwnd)
            .map_or(0, |i| crate::foreign::copy_out(i.text(), buf, n));
    }
    if buf.is_null() || n <= 0 {
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
        return crate::foreign::remote(hwnd).map_or(0, |i| i.text_len as i32);
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
        crate::foreign::sync(hwnd);
        SendMessageW(hwnd, WM_ENABLE, usize::from(enable != 0), 0);
    }
    i32::from(!was)
}

#[unsafe(no_mangle)]
pub extern "system" fn IsWindowEnabled(hwnd: Hwnd) -> i32 {
    if let Some(info) = crate::foreign::remote(hwnd) {
        return i32::from(info.enabled);
    }
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
const SWP_NOSENDCHANGING: u32 = 0x400;
const SWP_FRAMECHANGED: u32 = 0x20;

/// A top-level window's new client size (and menu bar height): a buffer
/// of that size with what fits of the old client pixels (white
/// elsewhere); the old one is freed.
pub(crate) fn resize_client(i: usize, cw: i32, ch: i32) {
    resize_buffer(i, cw, ch);
}

fn resize_buffer(i: usize, cw: i32, ch: i32) {
    let bar = with(|s| s.windows[i].bar);
    relayout(i, cw, ch, bar);
}

fn relayout(i: usize, cw: i32, ch: i32, bar: i32) {
    let w = with(|s| s.windows[i]);
    let (old, ow, oh) = (w.pixels, w.cw, w.ch);
    let old_base = crate::window::buffer_base(&w);
    let (cw, ch) = (cw.max(1), ch.max(1));
    let Some((base, section)) = crate::surface::alloc(cw, ch, bar) else {
        return;
    };
    // SAFETY: the bar's rows come first.
    let new = unsafe { base.add((bar * cw) as usize) };
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
        s.windows[i].section = section;
        s.windows[i].cw = cw;
        s.windows[i].ch = ch;
        s.windows[i].bar = bar;
    });
    crate::surface::register(hwnd_of(i), section, cw, ch, bar);
    // The window's former buffer, no longer used.
    crate::surface::free(old_base, w.section);
    if bar != 0 {
        crate::track::draw_bar(hwnd_of(i));
    }
}

/// SetMenu's part: a top-level window gets (or loses) its menu bar, which
/// takes (or gives back) the top of its client area: WM_SIZE, a repaint.
pub fn set_menu_bar(hwnd: Hwnd, on: bool) {
    let Some(i) = index_of(hwnd) else { return };
    let w = with(|s| s.windows[i]);
    if !w.used || w.parent != 0 || w.popup {
        return;
    }
    let bar = if on { crate::window::MENU_BAR } else { 0 };
    if bar == w.bar {
        crate::track::draw_bar(hwnd);
        return;
    }
    let ch = (w.ch + w.bar - bar).max(1);
    relayout(i, w.cw, ch, bar);
    let (cw, ch) = with(|s| (s.windows[i].cw, s.windows[i].ch));
    SendMessageW(hwnd, WM_SIZE, 0, ((ch as isize) << 16) | cw as isize);
    crate::window::InvalidateRect(hwnd, core::ptr::null(), 1);
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
    if hwnd != 0 && index_of(hwnd).is_none() && crate::foreign::remote(hwnd).is_some() {
        return set_window_pos_remote(hwnd, after, x, y, cx, cy, flags);
    }
    let Some(i) = index_of(hwnd) else { return 0 };
    let Some(w) = window(hwnd) else {
        set_last_error(ERROR_INVALID_WINDOW_HANDLE);
        return 0;
    };
    let (nx, ny) = if flags & SWP_NOMOVE != 0 { (w.x, w.y) } else { (x, y) };
    let (nw, nh) = if flags & SWP_NOSIZE != 0 { (w.w, w.h) } else { (cx.max(0), cy.max(0)) };
    // As on Windows, an unchanged size or position counts as not asked
    // for (no WM_SIZE or WM_MOVE follows): a control that places itself
    // on WM_SIZE does not loop.
    let mut flags = flags;
    if (nw, nh) == (w.w, w.h) {
        flags |= SWP_NOSIZE;
    }
    if (nx, ny) == (w.x, w.y) {
        flags |= SWP_NOMOVE;
    }
    let mut pos = WindowPos { hwnd, after, x: nx, y: ny, cx: nw, cy: nh, flags };
    // The window procedure may change the WINDOWPOS: the window gets what
    // it says then (a size it does not allow, say). Not asked with
    // SWP_NOSENDCHANGING (wx's status bar moves itself so: it answers
    // WM_WINDOWPOSCHANGING with the rectangle it had).
    if flags & SWP_NOSENDCHANGING == 0 {
        SendMessageW(hwnd, WM_WINDOWPOSCHANGING, 0, &mut pos as *mut WindowPos as isize);
    }
    let (nx, ny) = if pos.flags & SWP_NOMOVE != 0 { (w.x, w.y) } else { (pos.x, pos.y) };
    let (nw, nh) =
        if pos.flags & SWP_NOSIZE != 0 { (w.w, w.h) } else { (pos.cx.max(0), pos.cy.max(0)) };
    let mut flags = pos.flags;
    if (nw, nh) == (w.w, w.h) {
        flags |= SWP_NOSIZE;
    }
    if (nx, ny) == (w.x, w.y) {
        flags |= SWP_NOMOVE;
    }
    let mut pos = WindowPos { hwnd, after: pos.after, x: nx, y: ny, cx: nw, cy: nh, flags };
    let after = pos.after;
    let top_level = w.parent == 0;
    if top_level {
        let (b, top) = crate::window::frame(&w);
        let (ncw, nch) = ((nw - 2 * b).max(1), (nh - top - b).max(1));
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
            // The record other processes read (GetWindowRect of a foreign
            // window) first: once the desktop lists the new rectangle,
            // it must be what they read too. Published after it, a lister
            // could see the move and then the old rectangle (gala_cli's
            // check 0x1f under load, 2026-11-04).
            crate::foreign::sync_tree(hwnd);
            let (dx, dy, dw, dh) = crate::window::desktop_rect(&w, nx, ny, nw, nh);
            // SAFETY: our desktop window.
            unsafe { GalaWindowPlace(w.desktop, dx, dy, dw as u32, dh as u32) };
            // The desktop's new buffer needs the bar again.
            crate::track::draw_bar(hwnd);
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
    // The frame changed (SetWindowLong of the style, or a program's own
    // frame): WM_NCCALCSIZE again.
    if top_level && flags & SWP_FRAMECHANGED != 0 {
        crate::window::calc_frame(hwnd, true);
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
    crate::foreign::sync_tree(hwnd);
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

// ---- Maximizing and minimizing ----

/// IsZoomed: the window is maximized (any process's: the registry's
/// record says so for another's).
#[unsafe(no_mangle)]
pub extern "system" fn IsZoomed(hwnd: Hwnd) -> i32 {
    if let Some(info) = crate::foreign::remote(hwnd) {
        return i32::from(info.zoomed);
    }
    i32::from(window(hwnd).is_some_and(|w| w.zoomed))
}

/// IsIconic: the window is minimized (any process's, as IsZoomed).
#[unsafe(no_mangle)]
pub extern "system" fn IsIconic(hwnd: Hwnd) -> i32 {
    if let Some(info) = crate::foreign::remote(hwnd) {
        return i32::from(info.iconic);
    }
    i32::from(window(hwnd).is_some_and(|w| w.iconic))
}

/// Snap (the desktop's GALA_WM_PLACE): the top-level window moves to the
/// rectangle (a maximized one is no longer).
pub(crate) fn snap(hwnd: Hwnd, x: i32, y: i32, w: i32, h: i32) {
    let Some(i) = index_of(hwnd) else { return };
    if window(hwnd).is_none_or(|w| w.parent != 0) || w <= 0 || h <= 0 {
        return;
    }
    with(|s| s.windows[i].zoomed = false);
    // The desktop places its own window: for a frame the program draws,
    // that is the client area, inside the program's borders.
    let (x, y, w, h) = match window(hwnd).and_then(|w| w.custom) {
        Some((b, t)) => (x - b, y - t, w + 2 * b, h + t + b),
        None => (x, y, w, h),
    };
    SetWindowPos(hwnd, 0, x, y, w, h, SWP_NOZORDER | 0x10);
}

/// SC_MAXIMIZE/SC_RESTORE: the top-level window fills the work area, its
/// rectangle kept for restoring.
pub(crate) fn maximize(hwnd: Hwnd, on: bool) {
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
    crate::foreign::sync(hwnd);
}

/// SIZE_MINIMIZED, SIZE_RESTORED, SIZE_MAXIMIZED (WM_SIZE's wParam).
const SIZE_RESTORED: usize = 0;
const SIZE_MINIMIZED: usize = 1;
const SIZE_MAXIMIZED: usize = 2;

/// SC_MINIMIZE (`on`) and SC_RESTORE of a minimized window: the
/// top-level window leaves the screen (the desktop keeps it on the
/// taskbar) or comes back as it was (maximized or not); it stays
/// visible (IsWindowVisible), as on Windows. Its rectangle is kept
/// (Windows moves a minimized window to -32000, -32000: not done here).
/// WM_SIZE SIZE_MINIMIZED (0 x 0), then SIZE_RESTORED or SIZE_MAXIMIZED
/// with the client size, tell the program. `tell_desktop` is false when
/// the desktop did it itself (its minimize button).
pub(crate) fn minimize(hwnd: Hwnd, on: bool, tell_desktop: bool) {
    let Some(i) = index_of(hwnd) else { return };
    let Some(w) = window(hwnd) else { return };
    if w.parent != 0 || w.iconic == on {
        return;
    }
    with(|s| s.windows[i].iconic = on);
    crate::foreign::sync(hwnd);
    if tell_desktop && w.desktop != 0 && w.visible {
        // SAFETY: our desktop window; 2 shows it minimized.
        unsafe { GalaWindowShow(w.desktop, if on { 2 } else { 1 }) };
    }
    if on {
        SendMessageW(hwnd, WM_SIZE, SIZE_MINIMIZED, 0);
    } else {
        let lp = ((w.ch as u16 as isize) << 16) | (w.cw as u16 as isize);
        SendMessageW(hwnd, WM_SIZE, if w.zoomed { SIZE_MAXIMIZED } else { SIZE_RESTORED }, lp);
        crate::window::InvalidateRect(hwnd, core::ptr::null(), 1);
    }
}

/// ShowWindow's commands that change the window's state: SW_SHOWNORMAL
/// and SW_RESTORE restore a minimized or maximized window,
/// SW_SHOWMINIMIZED, SW_MINIMIZE, SW_SHOWMINNOACTIVE and SW_FORCEMINIMIZE
/// minimize it, SW_SHOWMAXIMIZED maximizes it (top-level windows).
pub(crate) fn show_state(hwnd: Hwnd, cmd: i32) {
    match cmd {
        1 | 9 => {
            if window(hwnd).is_some_and(|w| w.iconic) {
                minimize(hwnd, false, true);
            } else {
                maximize(hwnd, false);
            }
        }
        2 | 6 | 7 | 11 => minimize(hwnd, true, true),
        3 => {
            minimize(hwnd, false, true);
            maximize(hwnd, true);
        }
        _ => {}
    }
}

/// gala's SetWindowPos message (0x03f1, in the system range): another
/// process's SetWindowPos of one of our windows (lParam: hwnd-after, x,
/// y, cx, cy, flags as 8 + 5 * 4 bytes, marshaled by `foreign`); user32
/// answers it itself, never the window procedure.
pub(crate) const GALA_SETWINDOWPOS: u32 = 0x03f1;
pub(crate) const SETWINDOWPOS_SIZE: usize = 28;
/// ShowWindow of another process's window: sent to it as GALA_SHOWWINDOW
/// (wParam the SW_ command); user32 answers it itself (ShowWindow's
/// result), never the window procedure. UIPI applies (the kernel lets no
/// filter open gala's messages).
pub(crate) const GALA_SHOWWINDOW: u32 = 0x03f2;

pub(crate) fn set_window_pos_message(hwnd: Hwnd, lparam: isize) -> isize {
    if lparam == 0 {
        return 0;
    }
    let mut b = [0u8; SETWINDOWPOS_SIZE];
    // SAFETY: the marshaled request (SETWINDOWPOS_SIZE bytes).
    unsafe { core::ptr::copy_nonoverlapping(lparam as *const u8, b.as_mut_ptr(), b.len()) };
    let i32_at = |at: usize| i32::from_le_bytes(b[at..at + 4].try_into().unwrap_or([0; 4]));
    let after = u64::from_le_bytes(b[0..8].try_into().unwrap_or([0; 8])) as Hwnd;
    SetWindowPos(hwnd, after, i32_at(8), i32_at(12), i32_at(16), i32_at(20), i32_at(24) as u32)
        as isize
}

/// SetWindowPos of another process's window: sent to it as
/// GALA_SETWINDOWPOS (Windows moves any process's window; gala's windows
/// live in their program's user32).
fn set_window_pos_remote(
    hwnd: Hwnd,
    after: Hwnd,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    flags: u32,
) -> i32 {
    let mut b = [0u8; SETWINDOWPOS_SIZE];
    b[0..8].copy_from_slice(&(after as u64).to_le_bytes());
    for (k, v) in [x, y, cx, cy, flags as i32].iter().enumerate() {
        b[8 + 4 * k..12 + 4 * k].copy_from_slice(&v.to_le_bytes());
    }
    i32::from(SendMessageW(hwnd, GALA_SETWINDOWPOS, 0, b.as_ptr() as isize) != 0)
}

// ---- DefWindowProc's part ----

const WM_NCLBUTTONDOWN: u32 = 0x00a1;
const HTMENU: usize = 5;
const SC_MOUSEMENU: usize = 0xf090;

/// What DefWindowProc does for the messages of this module.
pub fn def_window_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> Option<isize> {
    match msg {
        // WM_MOUSEWHEEL/WM_MOUSEHWHEEL: on to the parent (a child that
        // doesn't scroll leaves it to the window around it).
        0x020a | 0x020e => {
            let parent = window(hwnd).map_or(0, |w| w.parent);
            Some(if parent != 0 { SendMessageW(parent, msg, wparam, lparam) } else { 0 })
        }
        // The window menu's commands (and the caption buttons' and UI
        // Automation's): close, maximize, minimize, restore.
        WM_SYSCOMMAND => {
            match wparam & 0xfff0 {
                SC_CLOSE => {
                    SendMessageW(hwnd, WM_CLOSE, 0, 0);
                }
                SC_MAXIMIZE => {
                    minimize(hwnd, false, true);
                    maximize(hwnd, true);
                }
                // A minimized window comes back as it was; a maximized
                // one gets its rectangle back.
                SC_RESTORE => {
                    if window(hwnd).is_some_and(|w| w.iconic) {
                        minimize(hwnd, false, true);
                    } else {
                        maximize(hwnd, false);
                    }
                }
                // As ShowWindow SW_MINIMIZE: a hidden window is shown
                // minimized.
                SC_MINIMIZE if window(hwnd).is_some_and(|w| !w.visible) => {
                    crate::window::ShowWindow(hwnd, 6);
                }
                SC_MINIMIZE => minimize(hwnd, true, true),
                // A click on the menu bar (lParam: the screen point).
                SC_MOUSEMENU => {
                    crate::track::track_bar(
                        hwnd,
                        lparam as i16 as i32,
                        (lparam >> 16) as i16 as i32,
                    );
                }
                _ => {}
            }
            Some(0)
        }
        // A press on the menu bar: the menu (WM_SYSCOMMAND SC_MOUSEMENU).
        WM_NCLBUTTONDOWN if wparam == HTMENU => {
            SendMessageW(hwnd, WM_SYSCOMMAND, SC_MOUSEMENU | HTMENU, lparam);
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
            crate::foreign::sync(hwnd);
            if let Some(d) = desktop.filter(|&d| d != 0) {
                // SAFETY: our desktop window; `len` characters.
                unsafe { GalaWindowSetTitle(d, title.as_ptr(), len) };
            }
            use crate::winevent::{CHILDID_SELF, EVENT_OBJECT_NAMECHANGE, OBJID_WINDOW, raise};
            raise(EVENT_OBJECT_NAMECHANGE, hwnd, OBJID_WINDOW, CHILDID_SELF);
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
                // SIZE_MAXIMIZED (2) for a maximized window, else
                // SIZE_RESTORED.
                let kind = if window(hwnd).is_some_and(|w| w.zoomed) { 2 } else { 0 };
                SendMessageW(hwnd, WM_SIZE, kind, lp);
            }
            Some(0)
        }
        _ => None,
    }
}

// ---- Deferred positioning ----

/// One recorded DeferWindowPos: window, insert-after, x, y, cx, cy, flags.
#[derive(Clone, Copy)]
struct Deferred {
    hwnd: Hwnd,
    after: Hwnd,
    rect: [i32; 4],
    flags: u32,
}

const DEFER_LISTS: usize = 8;
const DEFER_ENTRIES: usize = 256;

/// HDWP is 0x2000 + 16 * list; a list in use has `used` set.
struct DeferTable {
    lock: core::sync::atomic::AtomicBool,
    used: core::cell::UnsafeCell<[bool; DEFER_LISTS]>,
    counts: core::cell::UnsafeCell<[usize; DEFER_LISTS]>,
    entries: core::cell::UnsafeCell<[[Deferred; DEFER_ENTRIES]; DEFER_LISTS]>,
}

// SAFETY: the cells are only touched under `lock`.
unsafe impl Sync for DeferTable {}

static DEFER: DeferTable = DeferTable {
    lock: core::sync::atomic::AtomicBool::new(false),
    used: core::cell::UnsafeCell::new([false; DEFER_LISTS]),
    counts: core::cell::UnsafeCell::new([0; DEFER_LISTS]),
    entries: core::cell::UnsafeCell::new(
        [[Deferred { hwnd: 0, after: 0, rect: [0; 4], flags: 0 }; DEFER_ENTRIES]; DEFER_LISTS],
    ),
};

fn with_defer<R>(
    f: impl FnOnce(
        &mut [bool; DEFER_LISTS],
        &mut [usize; DEFER_LISTS],
        &mut [[Deferred; DEFER_ENTRIES]; DEFER_LISTS],
    ) -> R,
) -> R {
    use core::sync::atomic::Ordering;
    while DEFER.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r =
        unsafe { f(&mut *DEFER.used.get(), &mut *DEFER.counts.get(), &mut *DEFER.entries.get()) };
    DEFER.lock.store(false, Ordering::Release);
    r
}

fn defer_list(h: usize) -> Option<usize> {
    h.checked_sub(0x2000).filter(|v| v % 16 == 0).map(|v| v / 16).filter(|&i| i < DEFER_LISTS)
}

/// BeginDeferWindowPos(n): an empty list of positions (up to 256 each;
/// eight lists at a time).
#[unsafe(no_mangle)]
pub extern "system" fn BeginDeferWindowPos(n: i32) -> usize {
    if n < 0 {
        crate::window::set_last_error(87);
        return 0;
    }
    with_defer(|used, counts, _| match used.iter().position(|u| !u) {
        Some(i) => {
            used[i] = true;
            counts[i] = 0;
            0x2000 + 16 * i
        }
        None => {
            crate::window::set_last_error(8); // ERROR_NOT_ENOUGH_MEMORY
            0
        }
    })
}

/// DeferWindowPos: records SetWindowPos's arguments; the list (NULL, and
/// the list freed, when the window is not one or the list is full).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn DeferWindowPos(
    list: usize,
    hwnd: Hwnd,
    after: Hwnd,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    flags: u32,
) -> usize {
    let Some(i) = defer_list(list) else { return 0 };
    let valid = crate::window::IsWindow(hwnd) != 0;
    with_defer(|used, counts, entries| {
        if !used[i] {
            return 0;
        }
        if !valid || counts[i] == DEFER_ENTRIES {
            used[i] = false;
            return 0;
        }
        entries[i][counts[i]] = Deferred { hwnd, after, rect: [x, y, cx, cy], flags };
        counts[i] += 1;
        list
    })
}

/// EndDeferWindowPos: applies every recorded position (SetWindowPos, in
/// order) and frees the list.
#[unsafe(no_mangle)]
pub extern "system" fn EndDeferWindowPos(list: usize) -> i32 {
    let Some(i) = defer_list(list) else { return 0 };
    let Some(n) = with_defer(|used, counts, _| used[i].then_some(counts[i])) else { return 0 };
    // One entry at a time (the list stays in use until the end).
    for k in 0..n {
        let m = with_defer(|_, _, entries| entries[i][k]);
        let [x, y, cx, cy] = m.rect;
        SetWindowPos(m.hwnd, m.after, x, y, cx, cy, m.flags);
    }
    with_defer(|used, _, _| used[i] = false);
    1
}

// ---- Dialog navigation ----

/// The dialog's direct children that take the focus by Tab: visible,
/// enabled, WS_TABSTOP; in sibling order.
fn tab_stops(dlg: Hwnd) -> ([Hwnd; 128], usize) {
    let mut out = [0; 128];
    let mut n = 0;
    let mut c = GetWindow(dlg, 5); // GW_CHILD
    while c != 0 && n < out.len() {
        let s = GetWindowLongW(c, -16) as u32;
        if s & 0x0001_0000 != 0 && s & 0x1000_0000 != 0 && s & 0x0800_0000 == 0 {
            out[n] = c;
            n += 1;
        }
        c = GetWindow(c, 2); // GW_HWNDNEXT
    }
    (out, n)
}

/// GetNextDlgTabItem(dlg, control, previous): the next (or previous)
/// WS_TABSTOP control after `control`, wrapping; the first (last) for
/// NULL.
#[unsafe(no_mangle)]
pub extern "system" fn GetNextDlgTabItem(dlg: Hwnd, control: Hwnd, previous: i32) -> Hwnd {
    let (stops, n) = tab_stops(dlg);
    if n == 0 {
        return control;
    }
    let at = stops[..n].iter().position(|&h| h == control);
    let i = match (at, previous != 0) {
        (Some(i), false) => (i + 1) % n,
        (Some(i), true) => (i + n - 1) % n,
        (None, false) => 0,
        (None, true) => n - 1,
    };
    stops[i]
}

/// IsDialogMessageW(dlg, msg): the dialog's keyboard: Tab (Shift+Tab)
/// moves the focus between its WS_TABSTOP controls, Enter sends IDOK and
/// Esc IDCANCEL (unless the focused control wants the key, by
/// WM_GETDLGCODE); its windows' other messages are translated and
/// dispatched. TRUE when the message was the dialog's.
#[unsafe(no_mangle)]
pub extern "system" fn IsDialogMessageW(dlg: Hwnd, msg: *mut crate::window::Msg) -> i32 {
    if msg.is_null() || dlg == 0 {
        return 0;
    }
    // SAFETY: the caller's MSG.
    let m = unsafe { *msg };
    if m.hwnd != dlg && IsChild(dlg, m.hwnd) == 0 {
        return 0;
    }
    const WM_KEYDOWN: u32 = 0x0100;
    const WM_GETDLGCODE: u32 = 0x0087;
    if m.message == WM_KEYDOWN {
        let code = SendMessageW(m.hwnd, WM_GETDLGCODE, m.wparam, msg as isize) as u32;
        let wants_all = code & 0x4 != 0; // DLGC_WANTALLKEYS / DLGC_WANTMESSAGE
        match m.wparam {
            0x09 if code & 0x2 == 0 && !wants_all => {
                // VK_TAB (not DLGC_WANTTAB)
                let shift = crate::window::GetKeyState(0x10) < 0;
                let focus = GetFocus();
                let next = GetNextDlgTabItem(
                    dlg,
                    if focus != 0 { focus } else { m.hwnd },
                    i32::from(shift),
                );
                if next != 0 {
                    SetFocus(next);
                }
                return 1;
            }
            0x0d if !wants_all => {
                // VK_RETURN: the default button (DM_GETDEFID), else IDOK.
                let def = SendMessageW(dlg, 0x0400, 0, 0) as usize;
                let id = if def >> 16 == 0x534b { def & 0xffff } else { 1 };
                SendMessageW(dlg, 0x0111, id, GetDlgItem(dlg, id as i32) as isize);
                return 1;
            }
            0x1b if !wants_all => {
                // VK_ESCAPE: IDCANCEL.
                SendMessageW(dlg, 0x0111, 2, GetDlgItem(dlg, 2) as isize);
                return 1;
            }
            _ => {}
        }
    }
    crate::window::TranslateMessage(msg);
    crate::window::DispatchMessageW(msg);
    1
}

/// IsDialogMessageA: as IsDialogMessageW (the ANSI code page is UTF-8).
#[unsafe(no_mangle)]
pub extern "system" fn IsDialogMessageA(dlg: Hwnd, msg: *mut crate::window::Msg) -> i32 {
    IsDialogMessageW(dlg, msg)
}

/// POINT, passed by value.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Point {
    x: i32,
    y: i32,
}

/// WindowFromPoint(point): the deepest visible window at a screen point:
/// the topmost top-level window containing it (its frame included), then
/// the topmost visible child containing it, and so on; NULL if none.
#[unsafe(no_mangle)]
pub extern "system" fn WindowFromPoint(point: Point) -> Hwnd {
    let Point { x, y } = point;
    with(|s| {
        let inside_top = |i: usize| {
            let w = &s.windows[i];
            x >= w.x && y >= w.y && x < w.x + w.w && y < w.y + w.h
        };
        let inside_child = |i: usize| {
            let w = &s.windows[i];
            let (ox, oy) = screen_origin(s, i);
            x >= ox && y >= oy && x < ox + w.w && y < oy + w.h
        };
        let shown = |i: usize| s.windows[i].used && s.windows[i].visible;
        let mut best: Option<usize> = None;
        for i in 0..s.windows.len() {
            if shown(i)
                && s.windows[i].parent == 0
                && inside_top(i)
                && best.is_none_or(|b| s.windows[i].z > s.windows[b].z)
            {
                best = Some(i);
            }
        }
        let mut at = best?;
        loop {
            let me = crate::window::hwnd_of(at);
            let mut next: Option<usize> = None;
            for i in 0..s.windows.len() {
                if shown(i)
                    && s.windows[i].parent == me
                    && inside_child(i)
                    && next.is_none_or(|b| s.windows[i].z > s.windows[b].z)
                {
                    next = Some(i);
                }
            }
            match next {
                Some(n) => at = n,
                None => return Some(me),
            }
        }
    })
    .unwrap_or(0)
}
