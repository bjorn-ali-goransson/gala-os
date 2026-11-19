//! "Change this program..." on the window (the flow's state is the
//! library's `explorer::change`): the window menu's item, the cards drawn
//! over the window, Inspect's outline and what the window's parts are
//! called, the clock that walks the steps. No coding agent is connected:
//! the last card says so and nothing is changed.

use super::*;
use explorer::change::{Change, Element, KeyResult, STEPS, Stage};

/// The window menu's item (below 0xF000, the low four bits clear).
pub const ID_CHANGE: usize = 0x0200;
/// The steps' clock (SetTimer's id) and its period.
const TIMER: usize = 0x0c4a;
const TIMER_MS: u32 = 100;

const CARD_W: i32 = 520;
const CARD_REQUEST_H: i32 = 300;
const CARD_WORK_H: i32 = 250;
const CARD_DONE_H: i32 = 190;
const BOX_H: i32 = 96;
const BUTTON_H: i32 = 32;
const BORDER: u32 = rgb(0xa0, 0xa0, 0xa0);
const ON_ACCENT: u32 = rgb(0xff, 0xff, 0xff);
const TAG_BG: u32 = rgb(0x1b, 0x1b, 0x1b);

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetSystemMenu(hwnd: Hwnd, revert: Bool) -> Handle;
    fn InsertMenuW(menu: Handle, position: u32, flags: u32, id: usize, text: *const u16) -> Bool;
    fn SetTimer(hwnd: Hwnd, id: usize, elapse: u32, proc_: *const c_void) -> usize;
    fn KillTimer(hwnd: Hwnd, id: usize) -> Bool;
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

/// The card's rectangle for the stage (x, y, w, h).
fn card(s: &State, stage: Stage) -> [i32; 4] {
    let h = match stage {
        Stage::Request | Stage::Inspecting => CARD_REQUEST_H,
        Stage::Working { .. } => CARD_WORK_H,
        Stage::Done => CARD_DONE_H,
    };
    let (w, wh) = s.size;
    [(w - CARD_W) / 2, (wh - h) / 2, CARD_W, h]
}

/// The request card's buttons: Inspect, Cancel, Make the change.
fn buttons(r: [i32; 4]) -> [[i32; 4]; 3] {
    let by = r[1] + r[3] - BUTTON_H - 16;
    let right = r[0] + r[2] - 20;
    [
        [r[0] + r[2] - 20 - 96, r[1] + 186, 96, BUTTON_H],
        [right - 150 - 8 - 90, by, 90, BUTTON_H],
        [right - 150, by, 150, BUTTON_H],
    ]
}

fn inside(r: [i32; 4], x: i32, y: i32) -> bool {
    x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3]
}

/// A click while the flow is open (it takes them all).
pub fn click(s: &mut State, x: i32, y: i32) {
    let Some(c) = s.change.as_ref() else { return };
    match c.stage {
        Stage::Inspecting => {
            let e = element_at(s, x, y);
            let Some(c) = s.change.as_mut() else { return };
            c.pick(e);
            let mut l = String::from("change: picked ");
            l.push_str(if c.target.is_some() { c.target_id() } else { "nothing" });
            log(&l);
        }
        Stage::Request => {
            let [inspect, cancel, make] = buttons(card(s, c.stage));
            if inside(inspect, x, y) {
                if let Some(c) = s.change.as_mut() {
                    c.inspect();
                }
                log("change: inspecting");
            } else if inside(cancel, x, y) {
                close(s, "Cancel");
            } else if inside(make, x, y) {
                submit(s);
            }
        }
        Stage::Working { .. } => {}
        Stage::Done => {
            let r = card(s, c.stage);
            let ok = [r[0] + r[2] - 20 - 90, r[1] + r[3] - BUTTON_H - 16, 90, BUTTON_H];
            if inside(ok, x, y) {
                close(s, "Close");
            }
        }
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

/// Text wrapped to `max` pixels in `font`, at most `lines` lines (the
/// last cut with "...").
fn wrapped(dc: &Dc, text: &str, font: Handle, max: i32, lines: usize) -> Vec<String> {
    // SAFETY: our DC and font.
    unsafe { SelectObject(dc.0, font) };
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let mut next = line.clone();
        if !next.is_empty() {
            next.push(' ');
        }
        next.push_str(word);
        let fits = dc.width(&next.encode_utf16().collect::<Vec<u16>>()) <= max;
        if fits || line.is_empty() {
            line = next;
        } else {
            out.push(core::mem::take(&mut line));
            line.push_str(word);
        }
    }
    out.push(line);
    if out.len() > lines {
        // The end of the text is what was typed last: keep it in view.
        out.drain(..out.len() - lines);
    }
    out
}

fn primary(dc: &Dc, r: [i32; 4], label: &str, font: Handle, on: bool) {
    dc.fill(r[0], r[1], r[2], r[3], if on { ACCENT } else { LINE });
    let text = if on { ON_ACCENT } else { TEXT_DIM };
    dc.text(r[0] + 12, r[1] + 8, r[2] - 24, label, font, text, false);
}

fn secondary(dc: &Dc, r: [i32; 4], label: &str, font: Handle) {
    dc.fill(r[0], r[1], r[2], r[3], CONTENT);
    dc.frame(r[0], r[1], r[2], r[3], rgb(0xd0, 0xd0, 0xd0));
    dc.text(r[0] + 12, r[1] + 8, r[2] - 24, label, font, TEXT, false);
}

/// Draws the flow over the window.
pub fn paint(s: &State, dc: &Dc) {
    let Some(c) = &s.change else { return };
    let (w, h) = s.size;
    if c.stage == Stage::Inspecting {
        // A band over the status bar says what to do; the element under
        // the pointer is outlined, its id in a tag above it.
        dc.fill(0, h - STATUS_H, w, STATUS_H, ACCENT);
        dc.text(
            12,
            h - STATUS_H + 5,
            w - 24,
            "Inspect: click the part of File Explorer you want to change. Esc goes back.",
            s.small,
            ON_ACCENT,
            false,
        );
        if let Some(e) = &c.hover {
            let [x, y, ew, eh] = e.rect;
            dc.frame(x, y, ew, eh, ACCENT);
            dc.frame(x + 1, y + 1, ew - 2, eh - 2, ACCENT);
            let tag: Vec<u16> = e.id.encode_utf16().collect();
            // SAFETY: our DC and font.
            unsafe { SelectObject(dc.0, s.small) };
            let tw = dc.width(&tag) + 16;
            let ty = if y >= 22 { y - 22 } else { y + eh + 2 };
            dc.fill(x, ty, tw, 20, TAG_BG);
            dc.text(x + 8, ty + 3, tw - 8, &e.id, s.small, ON_ACCENT, false);
        }
        return;
    }
    let r = card(s, c.stage);
    let [x, y, cw, ch] = r;
    dc.fill(x, y, cw, ch, MENU_BG);
    dc.frame(x, y, cw, ch, BORDER);
    dc.fill(x, y, cw, 3, ACCENT);
    let inner = cw - 40;
    match c.stage {
        Stage::Request | Stage::Inspecting => {
            dc.text(x + 20, y + 18, inner, "Change File Explorer", s.bold, TEXT, false);
            dc.text(
                x + 20,
                y + 44,
                inner,
                "Describe what you would like File Explorer to do differently.",
                s.small,
                TEXT_DIM,
                false,
            );
            // The request's box.
            let (bx, by) = (x + 20, y + 72);
            dc.fill(bx, by, inner, BOX_H, CONTENT);
            dc.frame(bx, by, inner, BOX_H, rgb(0xd0, 0xd0, 0xd0));
            dc.fill(bx + 1, by + BOX_H - 2, inner - 2, 2, ACCENT);
            let mut text = c.request.clone();
            text.push('|');
            for (k, l) in wrapped(dc, &text, s.font, inner - 20, 4).iter().enumerate() {
                dc.text(bx + 10, by + 8 + 20 * k as i32, inner - 20, l, s.font, TEXT, false);
            }
            // The part, and Inspect to pick another.
            let [inspect, cancel, make] = buttons(r);
            dc.text(x + 20, y + 182, 60, "Part", s.small, TEXT_DIM, false);
            dc.text(x + 20, y + 200, inspect[0] - x - 32, c.target_label(), s.font, TEXT, false);
            secondary(dc, inspect, "Inspect", s.font);
            dc.text(
                x + 20,
                y + 232,
                inner,
                "Preview: no coding agent is connected yet, so nothing will be changed.",
                s.small,
                TEXT_DIM,
                false,
            );
            secondary(dc, cancel, "Cancel", s.font);
            primary(dc, make, "Make the change", s.font, c.can_submit());
        }
        Stage::Working { started } => {
            dc.text(x + 20, y + 18, inner, "Changing File Explorer", s.bold, TEXT, false);
            let mut quoted = String::from("\"");
            quoted.push_str(&c.request);
            quoted.push('"');
            dc.text(x + 20, y + 44, inner, &quoted, s.small, TEXT_DIM, false);
            let t = now();
            let done = c.steps_done(t);
            for (k, step) in STEPS.iter().enumerate() {
                let sy = y + 76 + 30 * k as i32;
                if k < done {
                    dc.fill(x + 20, sy + 3, 12, 12, ACCENT);
                } else if k == done {
                    dc.frame(x + 20, sy + 3, 12, 12, ACCENT);
                    dc.fill(x + 23, sy + 6, 6, 6, ACCENT);
                    let ms = t.wrapping_sub(started) % explorer::change::STEP_MS;
                    let mut secs = String::new();
                    let _ = write!(secs, "{}.{} s", ms / 1000, ms / 100 % 10);
                    dc.text(x + cw - 80, sy, 60, &secs, s.small, TEXT_DIM, true);
                } else {
                    dc.frame(x + 20, sy + 3, 12, 12, BORDER);
                }
                let color = if k <= done { TEXT } else { TEXT_DIM };
                dc.text(x + 44, sy, inner - 100, step, s.font, color, false);
            }
            // The bar under the steps.
            let by = y + ch - 40;
            dc.fill(x + 20, by, inner, 4, LINE);
            dc.fill(x + 20, by, inner * done as i32 / STEPS.len() as i32, 4, ACCENT);
        }
        Stage::Done => {
            dc.text(x + 20, y + 18, inner, "Nothing was changed", s.bold, TEXT, false);
            let lines = [
                "No coding agent is connected yet, so File Explorer was not changed.",
                "When one is, it will edit File Explorer's source, build it and",
                "restart it here, keeping your folder and selection.",
            ];
            for (k, l) in lines.iter().enumerate() {
                dc.text(x + 20, y + 50 + 22 * k as i32, inner, l, s.font, TEXT, false);
            }
            let ok = [x + cw - 20 - 90, y + ch - BUTTON_H - 16, 90, BUTTON_H];
            primary(dc, ok, "Close", s.font, true);
        }
    }
}
