//! "Change this program..." on the window (the flow's state is the
//! library's `explorer::change`): the window menu's item, the cards drawn
//! over the window, Inspect's outline and what the window's parts are
//! called, the clock that walks the steps. No coding agent is connected:
//! the last card says so and nothing is changed.

use super::*;
use explorer::change::{Change, Element, KeyResult, Msg, STEPS, Stage, view};
use gala_ui::Placed;
use gala_ui::win::Gdi;

/// The window menu's item (below 0xF000, the low four bits clear).
pub const ID_CHANGE: usize = 0x0200;
/// The steps' clock (SetTimer's id) and its period.
const TIMER: usize = 0x0c4a;
const TIMER_MS: u32 = 100;

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetSystemMenu(hwnd: Hwnd, revert: Bool) -> Handle;
    fn InsertMenuW(menu: Handle, position: u32, flags: u32, id: usize, text: *const u16) -> Bool;
    fn SetTimer(hwnd: Hwnd, id: usize, elapse: u32, proc_: *const c_void) -> usize;
    fn KillTimer(hwnd: Hwnd, id: usize) -> Bool;
    fn GetDC(hwnd: Hwnd) -> Handle;
    fn ReleaseDC(hwnd: Hwnd, dc: Handle) -> i32;
}

/// The window menu gets "Change this program..." and a separator before
/// Close.
pub fn add_menu_item(hwnd: Hwnd) {
    const MF_SEPARATOR: u32 = 0x800;
    const SC_CLOSE: u32 = 0xf060;
    let text = wide("C&hange this program...");
    // SAFETY: our window; valid strings.
    let ok = unsafe {
        let menu = GetSystemMenu(hwnd, 0);
        !menu.is_null()
            && InsertMenuW(menu, SC_CLOSE, 0, ID_CHANGE, text.as_ptr()) != 0
            && InsertMenuW(menu, SC_CLOSE, MF_SEPARATOR, 0, core::ptr::null()) != 0
    };
    if !ok {
        record_missing("GetSystemMenu/InsertMenuW (Change this program)");
    }
}

/// The window menu's item was chosen: the request card.
pub fn open(s: &mut State) {
    if s.change.is_none() {
        s.menu = None;
        s.properties = None;
        s.change = Some(Change::new());
        log("change: request card opened (no coding agent is connected)");
    }
}

fn close(s: &mut State, why: &str) {
    s.change = None;
    // SAFETY: our window's timer (if set).
    unsafe { KillTimer(s.hwnd, TIMER) };
    let mut l = String::from("change: closed (");
    l.push_str(why);
    l.push(')');
    log(&l);
}

fn now() -> u32 {
    // SAFETY: a plain call.
    unsafe { GetTickCount() }
}

fn submit(s: &mut State) {
    let Some(c) = s.change.as_mut() else { return };
    if !c.submit(now()) {
        return;
    }
    let mut l = String::from("change: request \"");
    l.push_str(&c.request);
    l.push_str("\" on ");
    l.push_str(if c.target.is_some() { c.target_id() } else { "the whole window" });
    log(&l);
    let mut l = String::from("change: step ");
    l.push_str(STEPS[0]);
    log(&l);
    LAST_TICK.store(now(), core::sync::atomic::Ordering::Relaxed);
    // SAFETY: our window.
    if unsafe { SetTimer(s.hwnd, TIMER, TIMER_MS, core::ptr::null()) } == 0 {
        record_missing("SetTimer (Change this program)");
    }
}

static LAST_TICK: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// WM_TIMER: the steps move on; at the end, the note.
pub fn timer(s: &mut State, id: usize) -> bool {
    if id != TIMER {
        return false;
    }
    let Some(c) = s.change.as_mut() else { return false };
    let t = now();
    let before = LAST_TICK.swap(t, core::sync::atomic::Ordering::Relaxed);
    for k in c.tick(before, t) {
        let mut l = String::from("change: step ");
        l.push_str(STEPS[k]);
        log(&l);
    }
    if c.stage == Stage::Done {
        // SAFETY: our window's timer.
        unsafe { KillTimer(s.hwnd, TIMER) };
        log("change: no coding agent is connected, so nothing was changed");
    }
    true
}

/// A typed character while the flow is open (it takes them all).
pub fn character(s: &mut State, c: u16) {
    let Some(ch) = s.change.as_mut() else { return };
    match ch.key(c) {
        KeyResult::Submitted => submit(s),
        KeyResult::Closed => close(s, "Esc or Enter"),
        KeyResult::InspectCancelled => log("change: inspect cancelled"),
        KeyResult::Edited | KeyResult::Nothing => {}
    }
}

/// The pointer moved: while inspecting, the element under it is
/// outlined. Whether to draw again.
pub fn mouse_move(s: &mut State, x: i32, y: i32) -> bool {
    let hit = element_at(s, x, y);
    s.change.as_mut().is_some_and(|c| c.hover(hit))
}

/// The view laid out over the window, measured with `g`; None (logged)
/// when gala_ui refuses it.
fn placed(c: &Change, size: (i32, i32), g: &mut Gdi) -> Option<Vec<Placed<Msg>>> {
    let v = view(c, size, now());
    match gala_ui::place(&v, size.0 as f32, Some(size.1 as f32), g) {
        Ok(p) => Some(p),
        Err(e) => {
            let mut l = String::from("change: view: ");
            let _ = write!(l, "{e}");
            log(&l);
            None
        }
    }
}

/// A click while the flow is open (it takes them all).
pub fn click(s: &mut State, x: i32, y: i32) {
    let Some(c) = s.change.as_ref() else { return };
    if c.stage == Stage::Inspecting {
        let e = element_at(s, x, y);
        let Some(c) = s.change.as_mut() else { return };
        c.pick(e);
        let mut l = String::from("change: picked ");
        l.push_str(if c.target.is_some() { c.target_id() } else { "nothing" });
        log(&l);
        return;
    }
    // The cards as they are drawn: laid out again, measured on the
    // window's DC.
    // SAFETY: our window's DC, released below after the Gdi is dropped.
    let hdc = unsafe { GetDC(s.hwnd) };
    if hdc.is_null() {
        record_missing("GetDC (Change this program)");
        return;
    }
    let p = placed(c, s.size, &mut Gdi::new(hdc));
    // SAFETY: the DC GetDC gave.
    unsafe { ReleaseDC(s.hwnd, hdc) };
    let Some((id, msg)) = p.as_deref().and_then(|p| gala_ui::click(p, x, y)) else { return };
    let mut l = String::from("change: click ");
    l.push_str(id);
    log(&l);
    match msg {
        Msg::Inspect => {
            if let Some(c) = s.change.as_mut() {
                c.inspect();
            }
            log("change: inspecting");
        }
        Msg::Cancel => close(s, "Cancel"),
        Msg::Make => submit(s),
        Msg::Close => close(s, "Close"),
    }
}

fn element(id: &str, label: &str, rect: [i32; 4]) -> Option<Element> {
    Some(Element { id: String::from(id), label: String::from(label), rect })
}

fn named(id: &str, name: &str, kind: &str, rect: [i32; 4]) -> Option<Element> {
    let mut i = String::from(id);
    i.push('.');
    i.push_str(&name.replace(' ', ""));
    let mut l = String::from(kind);
    l.push_str(" \"");
    l.push_str(name);
    l.push('"');
    element(&i, &l, rect)
}

/// The part of the window at client point (x, y), by its automation id
/// (the id an agent maps to the source that draws it).
pub fn element_at(s: &State, x: i32, y: i32) -> Option<Element> {
    let (w, h) = s.size;
    if x < 0 || y < 0 || x >= w || y >= h {
        return None;
    }
    if (BAR_Y..BAR_Y + BAR_H).contains(&y) {
        let aw = address_w(w);
        let sx = w - SEARCH_W - 12;
        return if (BACK_X..BACK_X + 32).contains(&x) {
            element("BackButton", "The Back button", [BACK_X, BAR_Y, 32, BAR_H])
        } else if (UP_X..UP_X + 32).contains(&x) {
            element("UpButton", "The Up button", [UP_X, BAR_Y, 32, BAR_H])
        } else if (ADDRESS_X..ADDRESS_X + aw).contains(&x) {
            element("AddressBar", "The address bar", [ADDRESS_X, BAR_Y, aw, BAR_H])
        } else if (sx..sx + SEARCH_W).contains(&x) {
            element("SearchBox", "The search box", [sx, BAR_Y, SEARCH_W, BAR_H])
        } else {
            None
        };
    }
    if (CMD_Y..CMD_Y + CMD_H).contains(&y) {
        let mut bx = 8;
        for (_, label, bw) in COMMANDS {
            if (bx..bx + bw).contains(&x) {
                return named("CommandBar", label, "The command", [bx, CMD_Y, bw, CMD_H]);
            }
            bx += bw + 4;
        }
        let mut bx = w - 12;
        for (_, label, bw) in VIEW_BUTTONS.iter().rev() {
            bx -= bw;
            if (bx..bx + bw).contains(&x) {
                return named("ViewButton", label, "The view button", [bx, CMD_Y, *bw, CMD_H]);
            }
            bx -= 4;
        }
        return element("CommandBar", "The command bar", [0, CMD_Y, w, CMD_H]);
    }
    if y >= h - STATUS_H {
        return element("StatusBar", "The status bar", [0, h - STATUS_H, w, STATUS_H]);
    }
    if y < PANE_Y {
        return None;
    }
    let pane_h = h - PANE_Y - STATUS_H;
    if x < NAV_W {
        let k = (y - PANE_Y - 4) / NAV_ROW;
        if y >= PANE_Y + 4
            && let Some(row) = s.nav.get(k as usize)
        {
            let ry = PANE_Y + 4 + NAV_ROW * k;
            return named(
                "NavigationPane",
                &row.name,
                "The navigation pane's",
                [4, ry, NAV_W - 8, NAV_ROW - 2],
            );
        }
        return element("NavigationPane", "The navigation pane", [0, PANE_Y, NAV_W, pane_h]);
    }
    let cx = content_x();
    let cw = w - cx;
    if s.view == View::Details && y < PANE_Y + HEADER_H {
        let mut hx = cx + 12;
        for (_, label, width) in COLUMNS {
            if (hx..hx + width).contains(&x) {
                return named(
                    "ColumnHeader",
                    label,
                    "The column header",
                    [hx - 6, PANE_Y + 1, width, HEADER_H - 2],
                );
            }
            hx += width;
        }
        return element("ColumnHeaders", "The column headers", [cx, PANE_Y, cw, HEADER_H]);
    }
    if let Some(i) = row_at(s, x, y) {
        let k = s.rows.iter().position(|&r| r == i).unwrap_or(0).saturating_sub(s.scroll) as i32;
        let rect = match s.view {
            View::Details => [cx + 4, rows_y(View::Details) + ROW_H * k, cw - 8, ROW_H - 1],
            View::Icons => {
                let (tx, ty) = explorer::icon_grid(k as usize, cw - 16);
                [cx + 8 + tx + 4, rows_y(View::Icons) + ty, TILE_W - 8, TILE_H - 4]
            }
        };
        return named("ItemsView.Item", &s.entries[i].name, "The item", rect);
    }
    element("ItemsView", "The list of items", [cx, PANE_Y, cw, pane_h])
}

/// Draws the flow over the window: the view laid out and painted with
/// gdi32.
pub fn paint(s: &State, dc: &Dc) {
    let Some(c) = &s.change else { return };
    let mut g = Gdi::new(dc.0);
    if let Some(p) = placed(c, s.size, &mut g) {
        gala_ui::paint(&p, None, &mut g);
    }
}
