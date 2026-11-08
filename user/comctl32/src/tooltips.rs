//! Tooltips (tooltips_class32): tools (a window and an id: a rectangle of
//! the window's client area, or with TTF_IDISHWND the window `uId`
//! itself) with texts, and the tip shown on hover, as documented:
//!
//! - Mouse input reaches the tooltip through TTM_RELAYEVENT (the program
//!   forwards its mouse messages) or, for TTF_SUBCLASS tools, through a
//!   subclass of the tool's window (`subclass.rs`) that does the same.
//! - When the pointer rests on a tool for the initial delay, the tip shows
//!   (TTN_GETDISPINFOW asks the tool's hwnd for an LPSTR_TEXTCALLBACK
//!   text; TTN_SHOW to the tool's hwnd first): a desktop popup (user32
//!   makes every tooltip a WS_POPUP) below and right of the pointer, kept
//!   on the work area. Moving within the tool restarts the wait; moving to
//!   another tool while a tip shows switches after the reshow delay.
//! - It hides (TTN_POP first) when the pointer leaves the tool (checked
//!   every 100 ms from the pointer's position while the pointer is on a
//!   tool: moves over other windows are not relayed), on a click, after the
//!   autopop delay, on TTM_POP or TTM_ACTIVATE FALSE; after a click or the
//!   autopop it stays hidden until the pointer leaves the tool.
//! - The delays (TTM_SETDELAYTIME/GETDELAYTIME) default to the documented
//!   proportions of the double-click time: initial = GetDoubleClickTime(),
//!   autopop 10 times that, reshow a fifth.
//! - Windows 11's light look: a near-white face, a 1-pixel grey border,
//!   the text in the GUI font (WM_SETFONT's) with 8 x 4 pixels of padding,
//!   on one line, or wrapped at TTM_SETMAXTIPWIDTH's width when set;
//!   TTM_SETTIPBKCOLOR/TEXTCOLOR are used when given.
//! - Tracking tooltips (TTM_TRACKACTIVATE, TTM_TRACKPOSITION) show at the
//!   position given, without the timers.
//!
//! Not yet: rounded corners (desktop popups are square), balloon tips and
//! titles, TTS_ALWAYSTIP's absence (tips show for inactive windows too),
//! TTM_HITTEST/WINDOWFROMPOINT, the pointer leaving to another program's
//! window or the desktop (the autopop delay hides the tip then).

use crate::common::*;
use crate::layout::{Msg, ToolInfoW, get, set};
use crate::subclass::{RemoveWindowSubclass, SetWindowSubclass};
use alloc::vec::Vec;

const TTM_ACTIVATE: u32 = WM_USER + 1;
const TTM_SETDELAYTIME: u32 = WM_USER + 3;
const TTM_ADDTOOLA: u32 = WM_USER + 4;
const TTM_DELTOOLA: u32 = WM_USER + 5;
const TTM_NEWTOOLRECTA: u32 = WM_USER + 6;
const TTM_RELAYEVENT: u32 = WM_USER + 7;
const TTM_GETTOOLCOUNT: u32 = WM_USER + 13;
const TTM_TRACKACTIVATE: u32 = WM_USER + 17;
const TTM_TRACKPOSITION: u32 = WM_USER + 18;
const TTM_SETTIPBKCOLOR: u32 = WM_USER + 19;
const TTM_SETTIPTEXTCOLOR: u32 = WM_USER + 20;
const TTM_GETDELAYTIME: u32 = WM_USER + 21;
const TTM_GETTIPBKCOLOR: u32 = WM_USER + 22;
const TTM_GETTIPTEXTCOLOR: u32 = WM_USER + 23;
const TTM_SETMAXTIPWIDTH: u32 = WM_USER + 24;
const TTM_GETMAXTIPWIDTH: u32 = WM_USER + 25;
const TTM_POP: u32 = WM_USER + 28;
const TTM_UPDATE: u32 = WM_USER + 29;
const TTM_SETTITLEW: u32 = WM_USER + 33;
const TTM_POPUP: u32 = WM_USER + 34;
const TTM_ADDTOOLW: u32 = WM_USER + 50;
const TTM_DELTOOLW: u32 = WM_USER + 51;
const TTM_NEWTOOLRECTW: u32 = WM_USER + 52;
const TTM_GETTOOLINFOW: u32 = WM_USER + 53;
const TTM_SETTOOLINFOW: u32 = WM_USER + 54;
const TTM_GETTEXTW: u32 = WM_USER + 56;
const TTM_UPDATETIPTEXTW: u32 = WM_USER + 57;
const TTM_ENUMTOOLSW: u32 = WM_USER + 58;
const TTM_GETCURRENTTOOLW: u32 = WM_USER + 59;
const TTM_SETWINDOWTHEME: u32 = 0x200b;

/// TTN_* (TTN_FIRST = -520).
const TTN_GETDISPINFOW: u32 = (-530i32) as u32;
const TTN_SHOW: u32 = (-521i32) as u32;
const TTN_POP: u32 = (-522i32) as u32;

const TTF_IDISHWND: u32 = 0x1;
const TTF_CENTERTIP: u32 = 0x2;
const TTF_SUBCLASS: u32 = 0x10;
const TTF_ABSOLUTE: u32 = 0x80;
const TTF_DI_SETITEM: u32 = 0x8000;
const TTS_NOPREFIX: u32 = 0x2;

const TTDT_AUTOMATIC: usize = 0;
const TTDT_RESHOW: usize = 1;
const TTDT_AUTOPOP: usize = 2;
const TTDT_INITIAL: usize = 3;

/// LPSTR_TEXTCALLBACKW: the text comes from TTN_GETDISPINFO.
const TEXT_CALLBACK: usize = usize::MAX;

const WM_NCHITTEST: u32 = 0x0084;
const WM_MOUSEWHEEL: u32 = 0x020a;
const WM_MBUTTONDBLCLK: u32 = 0x0209;

/// The timers on the tooltip window.
const TIMER_SHOW: usize = 1;
const TIMER_POP: usize = 2;
const TIMER_LEAVE: usize = 3;
/// How often a shown tip checks that the pointer is still on its tool.
const LEAVE_CHECK_MS: u32 = 100;

/// The tip's look: the face, its border, the padding around the text
/// and how far below the pointer's hot spot it goes (the arrow's height).
const FACE: u32 = rgb(0xf9, 0xf9, 0xf9);
const BORDER: u32 = rgb(0xc8, 0xc8, 0xc8);
const PAD_X: i32 = 8;
const PAD_Y: i32 = 4;
const BELOW_POINTER: i32 = 20;
/// COLOR_INFOBK and COLOR_INFOTEXT's defaults (TTM_GETTIPBKCOLOR/
/// TEXTCOLOR before a colour is set).
const INFO_BK: u32 = rgb(0xff, 0xff, 0xe1);
const INFO_TEXT: u32 = 0;

#[derive(Clone)]
struct Tool {
    hwnd: usize,
    id: usize,
    flags: u32,
    rect: [i32; 4],
    text: Vec<u16>,
    callback: bool,
    lparam: isize,
}

/// A tool's key: its window and id.
type Key = (usize, usize);

struct Tips {
    tools: Vec<Tool>,
    max_width: i32,
    active: bool,
    /// TTDT_RESHOW, AUTOPOP, INITIAL (index 1..=3); -1: the default.
    delays: [i32; 4],
    /// The tool under the pointer (from the relayed moves).
    current: Option<Key>,
    /// The last relayed move (window, lParam).
    last: (usize, isize),
    /// The tool whose tip shows.
    shown: Option<Key>,
    /// A click or the autopop hid the tip: none until the pointer leaves.
    quiet: bool,
    /// The text shown.
    text: Vec<u16>,
    face: Option<u32>,
    ink: Option<u32>,
    font: usize,
    /// TTM_TRACKPOSITION's point and whether the shown tip tracks it.
    track: (i32, i32),
    tracking: bool,
}

static TIPS: Store<Tips> = Store::new();

fn with<R>(hwnd: Hwnd, f: impl FnOnce(&mut Tips) -> R) -> Option<R> {
    TIPS.with(
        hwnd,
        || Tips {
            tools: Vec::new(),
            max_width: -1,
            active: true,
            delays: [-1; 4],
            current: None,
            last: (0, -1),
            shown: None,
            quiet: false,
            text: Vec::new(),
            face: None,
            ink: None,
            font: 0,
            track: (0, 0),
            tracking: false,
        },
        f,
    )
}

/// A delay (TTDT_*), the default from the double-click time.
fn delay(hwnd: Hwnd, which: usize) -> u32 {
    let set = with(hwnd, |t| t.delays[which]).unwrap_or(-1);
    if set >= 0 {
        return set as u32;
    }
    // SAFETY: plain call.
    let dbl = unsafe { GetDoubleClickTime() };
    match which {
        TTDT_RESHOW => dbl / 5,
        TTDT_AUTOPOP => dbl * 10,
        _ => dbl,
    }
}

/// TOOLINFO's members: cbSize, uFlags, hwnd, uId, rect, hinst, lpszText,
/// lParam (when cbSize covers it). An ANSI text is widened (Latin-1); a resource id (with
/// hinst) is loaded.
fn tool_of(info: isize, ansi: bool) -> Option<Tool> {
    if info == 0 {
        return None;
    }
    let p = info as *const ToolInfoW;
    // SAFETY: the caller's TOOLINFO (up to lpszText; lParam when cbSize
    // says it is there).
    let (flags, hwnd, id, rect, hinst, text, lparam) = unsafe {
        let size = get!(p, size);
        (
            get!(p, flags),
            get!(p, hwnd),
            get!(p, id),
            get!(p, rect),
            get!(p, instance),
            get!(p, text),
            if ToolInfoW::has_param(size) { get!(p, param) } else { 0 },
        )
    };
    let callback = text == TEXT_CALLBACK;
    let text = if callback {
        Vec::new()
    } else if text != 0 && text < 0x10000 {
        load_string(hinst, text as u32)
    } else if ansi {
        widen(text as *const u8)
    } else {
        owned(wide(text as *const u16, 1024))
    };
    Some(Tool { hwnd, id, flags, rect, text, callback, lparam })
}

fn load_string(instance: usize, id: u32) -> Vec<u16> {
    let mut buf = [0u16; 512];
    // SAFETY: our buffer of 512 characters.
    let n = unsafe { LoadStringW(instance, id, buf.as_mut_ptr(), 512) };
    owned(&buf[..n.clamp(0, 511) as usize])
}

/// An ANSI string as UTF-16 (Latin-1).
fn widen(p: *const u8) -> Vec<u16> {
    let mut v = Vec::new();
    if p.is_null() || (p as usize) < 0x10000 {
        return v;
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    while n < 1024 && unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    if v.try_reserve(n).is_ok() {
        // SAFETY: `n` bytes were read.
        v.extend(unsafe { core::slice::from_raw_parts(p, n) }.iter().map(|&b| u16::from(b)));
    }
    v
}

/// Writes a tool back into a TOOLINFOW: flags, window, id, rectangle,
/// lParam, and its text into lpszText's buffer (LPSTR_TEXTCALLBACKW for a
/// callback tool).
fn fill_info(info: isize, tool: &Tool) {
    if info == 0 {
        return;
    }
    let p = info as *mut ToolInfoW;
    // SAFETY: the caller's TOOLINFOW; the text buffer is the caller's (large
    // enough, as the messages require).
    unsafe {
        let size = get!(p, size);
        set!(p, flags, tool.flags);
        set!(p, hwnd, tool.hwnd);
        set!(p, id, tool.id);
        set!(p, rect, tool.rect);
        if ToolInfoW::has_param(size) {
            set!(p, param, tool.lparam);
        }
        let out = get!(p, text);
        if tool.callback {
            set!(p, text, TEXT_CALLBACK);
        } else if out != 0 && out != TEXT_CALLBACK {
            copy_text(&tool.text, out as *mut u16, usize::MAX);
        }
    }
}

/// Copies `text` and a NUL into `out` (at most `room` characters with it).
fn copy_text(text: &[u16], out: *mut u16, room: usize) {
    if room == 0 {
        return;
    }
    let n = text.len().min(room - 1);
    // SAFETY: the caller's buffer of `room` characters.
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), out, n);
        *out.add(n) = 0;
    }
}

/// The window whose mouse messages a subclassing tool relays.
fn subclassed(tool: &Tool) -> usize {
    if tool.flags & TTF_IDISHWND != 0 { tool.id } else { tool.hwnd }
}

/// Relays a mouse message of `hwnd` to the tooltip `tt` (TTM_RELAYEVENT);
/// other messages are left out.
pub fn relay(tt: Hwnd, hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) {
    if tt == 0 || !(WM_MOUSEMOVE..=WM_MBUTTONDBLCLK).contains(&msg) || msg == WM_MOUSEWHEEL {
        return;
    }
    let mut pt = [0i32; 2];
    // SAFETY: plain calls; our MSG.
    unsafe {
        GetCursorPos(&mut pt);
        let m = Msg {
            hwnd,
            message: msg,
            wparam,
            lparam,
            time: GetMessageTime() as u32,
            pt,
            private: 0,
        };
        SendMessageW(tt, TTM_RELAYEVENT, 0, &m as *const Msg as isize);
    }
}

/// Adds or replaces a tool of `hwnd` with a callback text (the toolbar's
/// buttons).
pub fn add_callback_tool(tt: Hwnd, hwnd: Hwnd, id: usize, rect: [i32; 4]) {
    add(tt, Tool { hwnd, id, flags: 0, rect, text: Vec::new(), callback: true, lparam: 0 });
}

/// Deletes a tool.
pub fn delete_tool(tt: Hwnd, hwnd: Hwnd, id: usize) {
    delete(tt, (hwnd, id));
}

/// Whether `code` is a tooltip notification (TTN_FIRST..TTN_LAST).
pub fn is_tip_note(code: u32) -> bool {
    (-549..=-520).contains(&(code as i32))
}

/// NMTTDISPINFOW's text after the program answered: whether it has one
/// (a string, the szText buffer or a resource id); if not, `fallback`
/// goes into szText.
pub fn fill_disp_text(info: isize, fallback: &[u16]) {
    if info == 0 || fallback.is_empty() {
        return;
    }
    // SAFETY: the caller's NMTTDISPINFOW.
    let di = unsafe { &mut *(info as *mut DispInfo) };
    let own = di.buf.as_ptr() as usize;
    let has =
        if di.text == 0 || di.text == own { di.buf[0] != 0 } else { di.text != TEXT_CALLBACK };
    if !has {
        let n = fallback.len().min(79);
        di.buf[..n].copy_from_slice(&fallback[..n]);
        di.buf[n] = 0;
        di.text = di.buf.as_ptr() as usize;
    }
}

pub const GETDISPINFOW: u32 = TTN_GETDISPINFOW;

/// The subclass of TTF_SUBCLASS tools' windows: mouse messages are
/// relayed to the tooltip (`data`) first.
extern "system" fn relay_proc(
    hwnd: Hwnd,
    msg: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    data: usize,
) -> isize {
    relay(data, hwnd, msg, wparam, lparam);
    crate::subclass::DefSubclassProc(hwnd, msg, wparam, lparam)
}

/// A tool's rectangle on the screen (none when its window is hidden).
fn screen_rect(tool: &Tool) -> Option<[i32; 4]> {
    let mut r = [0i32; 4];
    // SAFETY: plain calls; our RECT and POINTs.
    unsafe {
        if tool.flags & TTF_IDISHWND != 0 {
            if IsWindowVisible(tool.id) == 0 || GetWindowRect(tool.id, &mut r) == 0 {
                return None;
            }
        } else {
            if IsWindowVisible(tool.hwnd) == 0 {
                return None;
            }
            let mut a = [tool.rect[0], tool.rect[1]];
            let mut b = [tool.rect[2], tool.rect[3]];
            ClientToScreen(tool.hwnd, &mut a);
            ClientToScreen(tool.hwnd, &mut b);
            r = [a[0], a[1], b[0], b[1]];
        }
    }
    Some(r)
}

fn pointer() -> (i32, i32) {
    let mut p = [0i32; 2];
    // SAFETY: our POINT.
    unsafe { GetCursorPos(&mut p) };
    (p[0], p[1])
}

fn inside(r: [i32; 4], x: i32, y: i32) -> bool {
    x >= r[0] && x < r[2] && y >= r[1] && y < r[3]
}

fn find(hwnd: Hwnd, key: Key) -> Option<Tool> {
    with(hwnd, |t| t.tools.iter().find(|x| (x.hwnd, x.id) == key).cloned()).flatten()
}

/// Whether the pointer is on the tool now.
fn pointer_on(tool: &Tool) -> bool {
    let (x, y) = pointer();
    screen_rect(tool).is_some_and(|r| inside(r, x, y))
}

fn set_timer(hwnd: Hwnd, id: usize, ms: u32) {
    // SAFETY: plain call; a timer of our window.
    unsafe { SetTimer(hwnd, id, ms.max(10), 0) };
}

fn kill_timer(hwnd: Hwnd, id: usize) {
    // SAFETY: plain call.
    unsafe { KillTimer(hwnd, id) };
}

fn send_note(tt: Hwnd, key: Key, code: u32) -> isize {
    let mut hdr = NmHdr { from: tt, id: key.1, code };
    // SAFETY: plain call; the NMHDR lives across it.
    unsafe { SendMessageW(key.0, WM_NOTIFY, key.1, &mut hdr as *mut NmHdr as isize) }
}

/// NMTTDISPINFOW.
#[repr(C)]
struct DispInfo {
    hdr: NmHdr,
    text: usize,
    buf: [u16; 80],
    instance: usize,
    flags: u32,
    lparam: isize,
}

/// A callback tool's text: TTN_GETDISPINFOW to the tool's window (a
/// string, the szText buffer or a resource id with hinst); kept with
/// TTF_DI_SETITEM.
fn ask_text(tt: Hwnd, tool: &Tool) -> Vec<u16> {
    let mut di = DispInfo {
        hdr: NmHdr { from: tt, id: tool.id, code: TTN_GETDISPINFOW },
        text: 0,
        buf: [0; 80],
        instance: 0,
        flags: tool.flags,
        lparam: tool.lparam,
    };
    // SAFETY: plain call; the structure lives across it.
    unsafe { SendMessageW(tool.hwnd, WM_NOTIFY, tool.id, &mut di as *mut DispInfo as isize) };
    let own = di.buf.as_ptr() as usize;
    let text = if di.instance != 0 && di.text != 0 && di.text < 0x10000 {
        load_string(di.instance, di.text as u32)
    } else if di.text == 0 || di.text == own {
        let n = di.buf.iter().position(|&c| c == 0).unwrap_or(80);
        owned(&di.buf[..n])
    } else if di.text == TEXT_CALLBACK {
        Vec::new()
    } else {
        owned(wide(di.text as *const u16, 1024))
    };
    if di.flags & TTF_DI_SETITEM != 0 {
        let copy = owned(&text);
        with(tt, |t| {
            if let Some(x) = t.tools.iter_mut().find(|x| (x.hwnd, x.id) == (tool.hwnd, tool.id)) {
                x.text = copy;
                x.callback = false;
            }
        });
    }
    text
}

/// DrawTextW's format for the tip's text.
fn text_format(hwnd: Hwnd, max_width: i32) -> u32 {
    const DT_WORDBREAK: u32 = 0x10;
    const DT_SINGLELINE: u32 = 0x20;
    const DT_NOPREFIX: u32 = 0x800;
    let mut f = if max_width >= 0 { DT_WORDBREAK } else { DT_SINGLELINE };
    if style(hwnd) & TTS_NOPREFIX != 0 {
        f |= DT_NOPREFIX;
    }
    f
}

/// The tip's outer size for `text`.
fn measure(hwnd: Hwnd, text: &[u16], font: usize, max_width: i32, work: [i32; 4]) -> (i32, i32) {
    const DT_CALCRECT: u32 = 0x400;
    let wrap = if max_width > 0 { max_width } else { work[2] - work[0] } - 2 * (PAD_X + 1);
    let mut r = [0, 0, wrap.max(1), 0];
    // SAFETY: plain calls on our window's DC; our text and RECT.
    unsafe {
        let hdc = GetDC(hwnd);
        let old = select(hdc, if font != 0 { font } else { default_font() });
        DrawTextW(
            hdc,
            text.as_ptr(),
            text.len() as i32,
            &mut r,
            text_format(hwnd, max_width) | DT_CALCRECT,
        );
        select(hdc, old);
        ReleaseDC(hwnd, hdc);
    }
    let w = (r[2] - r[0]).clamp(1, (work[2] - work[0]).max(1));
    let h = (r[3] - r[1]).max(1);
    (w + 2 * (PAD_X + 1), h + 2 * (PAD_Y + 1))
}

/// The work area (SPI_GETWORKAREA).
fn work_area() -> [i32; 4] {
    let mut r = [0i32; 4];
    // SAFETY: our RECT.
    let ok = unsafe { SystemParametersInfoW(0x30, 0, r.as_mut_ptr().cast(), 0) };
    if ok == 0 || r[2] <= r[0] || r[3] <= r[1] { [0, 0, 1280, 800] } else { r }
}

/// Shows the tool's tip: below and right of the pointer (under the tool,
/// centred, with TTF_CENTERTIP; at `at` when tracking), kept on the work
/// area. Nothing shows for an empty text.
fn show(tt: Hwnd, key: Key, at: Option<(i32, i32)>) {
    let Some(tool) = find(tt, key) else { return };
    let text = if tool.callback { ask_text(tt, &tool) } else { tool.text.clone() };
    if text.is_empty() {
        return;
    }
    let Some((font, max_width)) = with(tt, |t| (t.font, t.max_width)) else { return };
    let work = work_area();
    let (w, h) = measure(tt, &text, font, max_width, work);
    let (px, py) = pointer();
    let (mut x, mut y) = match at {
        Some(p) if tool.flags & TTF_ABSOLUTE != 0 => p,
        Some((ax, ay)) => (ax, ay + BELOW_POINTER),
        None if tool.flags & TTF_CENTERTIP != 0 => match screen_rect(&tool) {
            Some(r) => ((r[0] + r[2] - w) / 2, r[3] + 2),
            None => (px, py + BELOW_POINTER),
        },
        None => (px, py + BELOW_POINTER),
    };
    if at.is_none() || tool.flags & TTF_ABSOLUTE == 0 {
        if x + w > work[2] {
            x = work[2] - w;
        }
        if y + h > work[3] {
            // No room below: above the pointer.
            y = py - h - 2;
        }
        x = x.max(work[0]);
        y = y.max(work[1]);
    }
    let tracking = at.is_some();
    with(tt, |t| {
        t.shown = Some(key);
        t.text = text;
        t.tracking = tracking;
    });
    // SAFETY: plain call on our window.
    unsafe { SetWindowPos(tt, 0, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE) };
    // TTN_SHOW before it shows: a nonzero answer means the program has
    // placed the tip itself (it keeps that place).
    send_note(tt, key, TTN_SHOW);
    if with(tt, |t| t.shown != Some(key)).unwrap_or(true) {
        return; // hidden meanwhile (by the notification)
    }
    redraw(tt);
    // SAFETY: plain call; SW_SHOWNOACTIVATE.
    unsafe { ShowWindow(tt, 4) };
    if !tracking {
        set_timer(tt, TIMER_POP, delay(tt, TTDT_AUTOPOP));
        set_timer(tt, TIMER_LEAVE, LEAVE_CHECK_MS);
    }
}

/// Hides the tip (TTN_POP first).
fn hide(tt: Hwnd) {
    let Some(Some(key)) = with(tt, |t| t.shown.take()) else { return };
    kill_timer(tt, TIMER_POP);
    send_note(tt, key, TTN_POP);
    // SAFETY: plain call.
    unsafe { ShowWindow(tt, 0) };
}

/// What a relayed move asks for.
enum Move {
    Nothing,
    /// Off every tool.
    Left,
    /// Onto a tool (or moved on it) with no tip showing: wait.
    Wait,
    /// Onto another tool while a tip shows: switch after the reshow delay.
    Switch,
    /// Moved on the tool whose tip shows: the autopop starts again.
    Stay,
}

fn relayed(tt: Hwnd, msg: isize) {
    if msg == 0 {
        return;
    }
    let p = msg as *const Msg;
    // SAFETY: the caller's MSG.
    let (hwnd, message, lparam) = unsafe { (get!(p, hwnd), get!(p, message), get!(p, lparam)) };
    if message == WM_MOUSEMOVE {
        moved(tt, hwnd, lparam);
    } else if (WM_LBUTTONDOWN..=WM_MBUTTONDBLCLK).contains(&message) && message != WM_MOUSEWHEEL {
        // A click hides the tip; none until the pointer leaves the tool.
        with(tt, |t| t.quiet = true);
        kill_timer(tt, TIMER_SHOW);
        hide(tt);
    }
}

fn moved(tt: Hwnd, hwnd: usize, lparam: isize) {
    let (x, y) = (lparam as i16 as i32, (lparam >> 16) as i16 as i32);
    let Some((what, active)) = with(tt, |t| {
        let hit = t
            .tools
            .iter()
            .find(|e| e.flags & TTF_IDISHWND == 0 && e.hwnd == hwnd && inside(e.rect, x, y))
            .or_else(|| t.tools.iter().find(|e| e.flags & TTF_IDISHWND != 0 && e.id == hwnd))
            .map(|e| (e.hwnd, e.id));
        let moved = t.last != (hwnd, lparam);
        t.last = (hwnd, lparam);
        let what = if hit != t.current {
            t.current = hit;
            t.quiet = false;
            match (hit, t.shown) {
                (None, _) => Move::Left,
                (Some(_), Some(_)) => Move::Switch,
                (Some(_), None) => Move::Wait,
            }
        } else if hit.is_none() || !moved {
            Move::Nothing
        } else if t.shown.is_some() {
            Move::Stay
        } else if t.quiet {
            Move::Nothing
        } else {
            Move::Wait
        };
        (what, t.active)
    }) else {
        return;
    };
    match what {
        Move::Nothing => {}
        Move::Left => {
            kill_timer(tt, TIMER_SHOW);
            kill_timer(tt, TIMER_LEAVE);
            hide(tt);
        }
        Move::Switch => {
            hide(tt);
            set_timer(tt, TIMER_LEAVE, LEAVE_CHECK_MS);
            if active {
                set_timer(tt, TIMER_SHOW, delay(tt, TTDT_RESHOW));
            }
        }
        Move::Wait => {
            set_timer(tt, TIMER_LEAVE, LEAVE_CHECK_MS);
            if active {
                set_timer(tt, TIMER_SHOW, delay(tt, TTDT_INITIAL));
            }
        }
        Move::Stay => set_timer(tt, TIMER_POP, delay(tt, TTDT_AUTOPOP)),
    }
}

fn timer(tt: Hwnd, id: usize) {
    match id {
        TIMER_SHOW => {
            kill_timer(tt, TIMER_SHOW);
            let Some((current, active, shown)) = with(tt, |t| (t.current, t.active, t.shown))
            else {
                return;
            };
            let Some(key) = current else { return };
            if !active || shown.is_some() {
                return;
            }
            match find(tt, key) {
                Some(tool) if pointer_on(&tool) => show(tt, key, None),
                _ => {
                    with(tt, |t| t.current = None);
                }
            }
        }
        TIMER_POP => {
            kill_timer(tt, TIMER_POP);
            with(tt, |t| t.quiet = true);
            hide(tt);
        }
        TIMER_LEAVE => {
            // The pointer's tool (shown or waited for, or after a click):
            // when the pointer has left it, all of that ends.
            let Some(Some(key)) = with(tt, |t| t.shown.or(t.current)) else {
                kill_timer(tt, TIMER_LEAVE);
                return;
            };
            if !find(tt, key).is_some_and(|tool| pointer_on(&tool)) {
                with(tt, |t| {
                    t.current = None;
                    t.quiet = false;
                });
                kill_timer(tt, TIMER_SHOW);
                kill_timer(tt, TIMER_LEAVE);
                hide(tt);
            }
        }
        _ => {}
    }
}

fn paint_tip(hwnd: Hwnd) {
    let Some((text, face, ink, font, max_width)) =
        with(hwnd, |t| (t.text.clone(), t.face, t.ink, t.font, t.max_width))
    else {
        return;
    };
    let format = text_format(hwnd, max_width);
    paint(hwnd, font, |hdc, r| {
        fill(hdc, r[0], r[1], r[2], r[3], BORDER);
        fill(hdc, r[0] + 1, r[1] + 1, r[2] - 1, r[3] - 1, face.unwrap_or(FACE));
        transparent(hdc);
        text_color(hdc, ink.unwrap_or(TEXT));
        draw_text(
            hdc,
            &text,
            [r[0] + 1 + PAD_X, r[1] + 1 + PAD_Y, r[2] - 1 - PAD_X, r[3] - 1 - PAD_Y],
            format,
        );
    });
}

/// Adds a tool (replacing one of the same window and id); a TTF_SUBCLASS
/// tool's window is subclassed.
fn add(hwnd: Hwnd, tool: Tool) -> isize {
    let target = (tool.flags & TTF_SUBCLASS != 0).then(|| subclassed(&tool));
    let key = (tool.hwnd, tool.id);
    let added = with(hwnd, |t| {
        if t.tools.try_reserve(1).is_err() {
            return false;
        }
        t.tools.retain(|x| (x.hwnd, x.id) != key);
        t.tools.push(tool);
        true
    })
    .unwrap_or(false);
    if added && let Some(target) = target {
        SetWindowSubclass(target, Some(relay_proc), hwnd, hwnd);
    }
    isize::from(added)
}

/// Deletes a tool (its tip hides); the subclass goes with the last
/// subclassing tool of its window.
fn delete(hwnd: Hwnd, key: Key) {
    let Some((gone, shown, others)) = with(hwnd, |t| {
        let gone = t.tools.iter().position(|x| (x.hwnd, x.id) == key).map(|i| t.tools.remove(i));
        if t.current == Some(key) {
            t.current = None;
            t.quiet = false;
        }
        let target = gone.as_ref().map(subclassed);
        let others =
            t.tools.iter().any(|x| x.flags & TTF_SUBCLASS != 0 && Some(subclassed(x)) == target);
        (gone, t.shown == Some(key), others)
    }) else {
        return;
    };
    if shown {
        hide(hwnd);
    }
    if let Some(tool) = gone
        && tool.flags & TTF_SUBCLASS != 0
        && !others
    {
        RemoveWindowSubclass(subclassed(&tool), Some(relay_proc), hwnd);
    }
}

pub extern "system" fn proc_(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        TTM_ADDTOOLW | TTM_ADDTOOLA => match tool_of(lparam, msg == TTM_ADDTOOLA) {
            Some(tool) => add(hwnd, tool),
            None => 0,
        },
        TTM_DELTOOLW | TTM_DELTOOLA => {
            if let Some(tool) = tool_of(lparam, msg == TTM_DELTOOLA) {
                delete(hwnd, (tool.hwnd, tool.id));
            }
            0
        }
        TTM_GETTOOLCOUNT => with(hwnd, |t| t.tools.len() as isize).unwrap_or(0),
        TTM_UPDATETIPTEXTW | TTM_SETTOOLINFOW => {
            if let Some(new) = tool_of(lparam, false) {
                let key = (new.hwnd, new.id);
                let shown = with(hwnd, |t| {
                    if let Some(x) = t.tools.iter_mut().find(|x| (x.hwnd, x.id) == key) {
                        x.text = new.text;
                        x.callback = new.callback;
                        if msg == TTM_SETTOOLINFOW {
                            x.rect = new.rect;
                            x.lparam = new.lparam;
                            // TTF_SUBCLASS stays as it was added.
                            x.flags = (new.flags & !TTF_SUBCLASS) | (x.flags & TTF_SUBCLASS);
                        }
                    }
                    t.shown == Some(key) && !t.tracking
                })
                .unwrap_or(false);
                if shown {
                    // The new text shows at once.
                    with(hwnd, |t| t.shown = None);
                    // SAFETY: plain call.
                    unsafe { ShowWindow(hwnd, 0) };
                    show(hwnd, key, None);
                }
            }
            0
        }
        TTM_NEWTOOLRECTW | TTM_NEWTOOLRECTA => {
            if let Some(new) = tool_of(lparam, msg == TTM_NEWTOOLRECTA) {
                with(hwnd, |t| {
                    if let Some(x) =
                        t.tools.iter_mut().find(|x| (x.hwnd, x.id) == (new.hwnd, new.id))
                    {
                        x.rect = new.rect;
                    }
                });
            }
            0
        }
        TTM_GETTOOLINFOW | TTM_GETTEXTW => {
            let Some(want) = tool_of(lparam, false) else { return 0 };
            let Some(tool) = find(hwnd, (want.hwnd, want.id)) else { return 0 };
            if msg == TTM_GETTOOLINFOW {
                fill_info(lparam, &tool);
            } else {
                // SAFETY: the caller's TOOLINFOW.
                let out = unsafe { get!(lparam as *const ToolInfoW, text) };
                if out != 0 && out != TEXT_CALLBACK {
                    let room = if wparam > 0 { wparam } else { usize::MAX };
                    copy_text(&tool.text, out as *mut u16, room);
                }
            }
            1
        }
        TTM_ENUMTOOLSW => match with(hwnd, |t| t.tools.get(wparam).cloned()).flatten() {
            Some(tool) => {
                fill_info(lparam, &tool);
                1
            }
            None => 0,
        },
        TTM_GETCURRENTTOOLW => {
            let shown = with(hwnd, |t| t.shown).flatten();
            match shown.and_then(|key| find(hwnd, key)) {
                Some(tool) => {
                    fill_info(lparam, &tool);
                    1
                }
                None => 0,
            }
        }
        TTM_SETMAXTIPWIDTH => {
            with(hwnd, |t| core::mem::replace(&mut t.max_width, lparam as i32) as isize)
                .unwrap_or(-1)
        }
        TTM_GETMAXTIPWIDTH => with(hwnd, |t| t.max_width as isize).unwrap_or(-1),
        TTM_SETDELAYTIME => {
            let v = lparam as i32;
            let v = if v < 0 { -1 } else { v & 0xffff };
            with(hwnd, |t| match wparam {
                TTDT_AUTOMATIC => {
                    t.delays = if v < 0 { [-1; 4] } else { [-1, v / 5, v * 10, v] };
                }
                TTDT_RESHOW | TTDT_AUTOPOP | TTDT_INITIAL => t.delays[wparam] = v,
                _ => {}
            });
            0
        }
        TTM_GETDELAYTIME => match wparam {
            TTDT_RESHOW | TTDT_AUTOPOP | TTDT_INITIAL => delay(hwnd, wparam) as isize,
            // TTDT_AUTOMATIC: the initial delay.
            _ => delay(hwnd, TTDT_INITIAL) as isize,
        },
        TTM_ACTIVATE => {
            let active = wparam != 0;
            with(hwnd, |t| t.active = active);
            if !active {
                kill_timer(hwnd, TIMER_SHOW);
                hide(hwnd);
            }
            0
        }
        TTM_RELAYEVENT => {
            relayed(hwnd, lparam);
            0
        }
        TTM_POP => {
            kill_timer(hwnd, TIMER_SHOW);
            hide(hwnd);
            0
        }
        TTM_POPUP => {
            // The tool under the pointer, now.
            let (x, y) = pointer();
            let tools = with(hwnd, |t| t.tools.clone()).unwrap_or_default();
            if let Some(tool) =
                tools.iter().find(|tool| screen_rect(tool).is_some_and(|r| inside(r, x, y)))
            {
                let key = (tool.hwnd, tool.id);
                hide(hwnd);
                with(hwnd, |t| t.current = Some(key));
                show(hwnd, key, None);
            }
            0
        }
        TTM_TRACKACTIVATE => {
            if wparam != 0 {
                if let Some(tool) = tool_of(lparam, false) {
                    let at = with(hwnd, |t| t.track).unwrap_or((0, 0));
                    hide(hwnd);
                    show(hwnd, (tool.hwnd, tool.id), Some(at));
                }
            } else {
                hide(hwnd);
            }
            0
        }
        TTM_TRACKPOSITION => {
            let at = (lparam as i16 as i32, (lparam >> 16) as i16 as i32);
            let shown = with(hwnd, |t| {
                t.track = at;
                if t.tracking { t.shown } else { None }
            })
            .flatten();
            if let Some(key) = shown {
                with(hwnd, |t| t.shown = None);
                show(hwnd, key, Some(at));
            }
            0
        }
        TTM_UPDATE => {
            redraw(hwnd);
            0
        }
        TTM_SETTIPBKCOLOR => {
            with(hwnd, |t| t.face = Some(wparam as u32));
            redraw(hwnd);
            0
        }
        TTM_SETTIPTEXTCOLOR => {
            with(hwnd, |t| t.ink = Some(wparam as u32));
            redraw(hwnd);
            0
        }
        TTM_GETTIPBKCOLOR => with(hwnd, |t| t.face.unwrap_or(INFO_BK) as isize).unwrap_or(0),
        TTM_GETTIPTEXTCOLOR => with(hwnd, |t| t.ink.unwrap_or(INFO_TEXT) as isize).unwrap_or(0),
        TTM_SETWINDOWTHEME => 0,
        TTM_SETTITLEW => 1,
        WM_SETFONT => {
            with(hwnd, |t| t.font = wparam);
            0
        }
        WM_GETFONT => with(hwnd, |t| t.font as isize).unwrap_or(0),
        WM_TIMER => {
            timer(hwnd, wparam);
            0
        }
        WM_PAINT => {
            paint_tip(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        // HTTRANSPARENT: the tip is not a target.
        WM_NCHITTEST => -1,
        WM_DESTROY => {
            for id in [TIMER_SHOW, TIMER_POP, TIMER_LEAVE] {
                kill_timer(hwnd, id);
            }
            if let Some(t) = TIPS.remove(hwnd) {
                for tool in t.tools.iter().filter(|x| x.flags & TTF_SUBCLASS != 0) {
                    RemoveWindowSubclass(subclassed(tool), Some(relay_proc), hwnd);
                }
            }
            // SAFETY: plain call.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        // SAFETY: plain call.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
