//! "Change this program..." on the window (the flow's state is the
//! library's `explorer::change`): the window menu's item, the cards drawn
//! over the window, Inspect's outline and what the window's parts are
//! called. "Make the change" hands the request to the docked agent
//! panel (agent.exe) by WM_COPYDATA (`agent::handoff`) and closes the
//! card. Without a panel to take it, the stand-in remains: a clock walks
//! the steps an agent would take, and the last card says no coding agent
//! is connected and nothing was changed.

use super::*;
use explorer::change::{Change, Element, KeyResult, Msg, STEPS, Stage, view};

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
    fn FindWindowW(class: *const u16, title: *const u16) -> Hwnd;
    fn SendMessageW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, max: i32) -> i32;
}

/// COPYDATASTRUCT.
#[repr(C)]
struct CopyData {
    kind: usize,
    size: u32,
    data: *const u8,
}

/// The docked agent panel's window (null when there is none).
fn find_panel() -> Hwnd {
    let (class, title) = (wide(agent::handoff::CLASS), wide(agent::handoff::TITLE));
    // SAFETY: valid strings.
    unsafe { FindWindowW(class.as_ptr(), title.as_ptr()) }
}

/// The request goes to the docked agent panel: whether it took it
/// (WM_COPYDATA's TRUE). Logged either way.
fn hand_off(s: &State, c: &Change) -> bool {
    use agent::handoff::{self, Handoff};
    let panel = find_panel();
    if panel.is_null() {
        log("change: no agent panel to hand the request to");
        return false;
    }
    let mut t = [0u16; 256];
    // SAFETY: our window; a buffer of its size.
    let n = unsafe { GetWindowTextW(s.hwnd, t.as_mut_ptr(), t.len() as i32) };
    let h = Handoff {
        program: String::from("File Explorer"),
        title: String::from_utf16_lossy(&t[..n.clamp(0, 256) as usize]),
        window: s.hwnd as u64,
        text: c.request.clone(),
        element: c.target.as_ref().map(|e| handoff::Element {
            id: e.id.clone(),
            label: e.label.clone(),
            rect: e.rect,
        }),
    };
    let bytes = handoff::encode(&h);
    let cds = CopyData { kind: handoff::KIND, size: bytes.len() as u32, data: bytes.as_ptr() };
    // SAFETY: the panel's window; the structure and its bytes live across
    // the call (user32 copies them to the panel's process).
    let taken = unsafe { SendMessageW(panel, 0x004a, s.hwnd as usize, &cds as *const _ as isize) };
    let mut l = String::new();
    let _ = write!(
        l,
        "change: {} the agent panel (window {:#x}, {} bytes)",
        if taken != 0 { "handed to" } else { "not taken by" },
        panel as usize,
        bytes.len()
    );
    log(&l);
    taken != 0
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
        let mut c = Change::new();
        c.panel = !find_panel().is_null();
        log(if c.panel {
            "change: request card opened (for the agent panel)"
        } else {
            "change: request card opened (no agent panel: a preview)"
        });
        s.change = Some(c);
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
    let Some(c) = s.change.as_ref() else { return };
    if !c.can_submit() {
        return;
    }
    let mut l = String::from("change: request \"");
    l.push_str(&c.request);
    l.push_str("\" on ");
    l.push_str(if c.target.is_some() { c.target_id() } else { "the whole window" });
    log(&l);
    if hand_off(s, c) {
        close(s, "handed to the agent panel");
        return;
    }
    let Some(c) = s.change.as_mut() else { return };
    c.submit(now());
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
    if s.change.as_ref().is_none_or(|c| c.stage != Stage::Inspecting) {
        return false;
    }
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

/// The part of the window at client point (x, y), by its automation id
/// (the id an agent maps to the source that draws it): the window's view
/// hit where it is drawn.
pub fn element_at(s: &State, x: i32, y: i32) -> Option<Element> {
    with_layout(s, |p| {
        let e = gala_ui::hit(p, x, y)?;
        let id = e.id.as_deref()?;
        Some(Element { id: String::from(id), label: window::describe(&look(s), id), rect: e.rect })
    })?
}

/// Draws the flow over the window: the view laid out and painted with
/// gdi32.
pub fn paint(s: &State, g: &mut Gdi) {
    let Some(c) = &s.change else { return };
    if let Some(p) = placed(c, s.size, g) {
        gala_ui::paint(&p, None, g);
    }
}
