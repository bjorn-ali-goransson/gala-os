//! The window procedure: the documented rich edit messages (WM_ text
//! messages with CRLF on the way out, EM_ selection, formats, lines,
//! scrolling, options, limits, text mode, URL detection), notifications
//! to the parent (EN_CHANGE and EN_UPDATE as WM_COMMAND when the event
//! mask asks; EN_SELCHANGE and EN_LINK as WM_NOTIFY; EN_SETFOCUS,
//! EN_KILLFOCUS and EN_MAXTEXT always), keys and the mouse. Messages
//! gala does not handle yet are recorded in the ledger
//! (`riched20.dll!message`, the message number as the detail) and answer
//! 0, the documented failure.

use crate::doc::{self, CFE_LINK, CFM_ALL2, CR, Format};
use crate::view::{self, Drag, ES_MULTILINE, ES_READONLY, Editor, style};
use crate::*;
use alloc::vec::Vec;

const WM_CREATE: u32 = 0x0001;
const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_SETFOCUS: u32 = 0x0007;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_ENABLE: u32 = 0x000a;
const WM_SETTEXT: u32 = 0x000c;
const WM_GETTEXT: u32 = 0x000d;
const WM_GETTEXTLENGTH: u32 = 0x000e;
const WM_PAINT: u32 = 0x000f;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_SETCURSOR: u32 = 0x0020;
const WM_SETFONT: u32 = 0x0030;
const WM_GETFONT: u32 = 0x0031;
const WM_NOTIFY: u32 = 0x004e;
const WM_STYLECHANGED: u32 = 0x007d;
const WM_GETDLGCODE: u32 = 0x0087;
const WM_KEYDOWN: u32 = 0x0100;
const WM_CHAR: u32 = 0x0102;
const WM_COMMAND: u32 = 0x0111;
const WM_VSCROLL: u32 = 0x0115;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MOUSEWHEEL: u32 = 0x020a;
const WM_USER: u32 = 0x0400;

const EM_GETSEL: u32 = 0x00b0;
const EM_SETSEL: u32 = 0x00b1;
const EM_GETRECT: u32 = 0x00b2;
const EM_SETRECT: u32 = 0x00b3;
const EM_SETRECTNP: u32 = 0x00b4;
const EM_SCROLL: u32 = 0x00b5;
const EM_LINESCROLL: u32 = 0x00b6;
const EM_SCROLLCARET: u32 = 0x00b7;
const EM_GETMODIFY: u32 = 0x00b8;
const EM_SETMODIFY: u32 = 0x00b9;
const EM_GETLINECOUNT: u32 = 0x00ba;
const EM_LINEINDEX: u32 = 0x00bb;
const EM_LINELENGTH: u32 = 0x00c1;
const EM_REPLACESEL: u32 = 0x00c2;
const EM_GETLINE: u32 = 0x00c4;
const EM_LIMITTEXT: u32 = 0x00c5;
const EM_CANUNDO: u32 = 0x00c6;
const EM_UNDO: u32 = 0x00c7;
const EM_LINEFROMCHAR: u32 = 0x00c9;
const EM_EMPTYUNDOBUFFER: u32 = 0x00cd;
const EM_GETFIRSTVISIBLELINE: u32 = 0x00ce;
const EM_SETREADONLY: u32 = 0x00cf;
const EM_SETMARGINS: u32 = 0x00d3;
const EM_GETMARGINS: u32 = 0x00d4;
const EM_GETLIMITTEXT: u32 = 0x00d5;
const EM_POSFROMCHAR: u32 = 0x00d6;
const EM_CHARFROMPOS: u32 = 0x00d7;
const EM_GETLIMITTEXT_RE: u32 = WM_USER + 37;
const EM_POSFROMCHAR_RE: u32 = WM_USER + 38;
const EM_CHARFROMPOS_RE: u32 = WM_USER + 39;
const EM_SCROLLCARET_RE: u32 = WM_USER + 49;
const EM_CANPASTE: u32 = WM_USER + 50;
const EM_EXGETSEL: u32 = WM_USER + 52;
const EM_EXLIMITTEXT: u32 = WM_USER + 53;
const EM_EXLINEFROMCHAR: u32 = WM_USER + 54;
const EM_EXSETSEL: u32 = WM_USER + 55;
const EM_GETCHARFORMAT: u32 = WM_USER + 58;
const EM_GETEVENTMASK: u32 = WM_USER + 59;
const EM_GETOLEINTERFACE: u32 = WM_USER + 60;
const EM_GETPARAFORMAT: u32 = WM_USER + 61;
const EM_GETSELTEXT: u32 = WM_USER + 62;
const EM_HIDESELECTION: u32 = WM_USER + 63;
const EM_REQUESTRESIZE: u32 = WM_USER + 65;
const EM_SELECTIONTYPE: u32 = WM_USER + 66;
const EM_SETBKGNDCOLOR: u32 = WM_USER + 67;
const EM_SETCHARFORMAT: u32 = WM_USER + 68;
const EM_SETEVENTMASK: u32 = WM_USER + 69;
const EM_SETOLECALLBACK: u32 = WM_USER + 70;
const EM_SETPARAFORMAT: u32 = WM_USER + 71;
const EM_SETTARGETDEVICE: u32 = WM_USER + 72;
const EM_GETTEXTRANGE: u32 = WM_USER + 75;
const EM_SETOPTIONS: u32 = WM_USER + 77;
const EM_GETOPTIONS: u32 = WM_USER + 78;
const EM_SETUNDOLIMIT: u32 = WM_USER + 82;
const EM_REDO: u32 = WM_USER + 84;
const EM_CANREDO: u32 = WM_USER + 85;
const EM_STOPGROUPTYPING: u32 = WM_USER + 88;
const EM_SETTEXTMODE: u32 = WM_USER + 89;
const EM_GETTEXTMODE: u32 = WM_USER + 90;
const EM_AUTOURLDETECT: u32 = WM_USER + 91;
const EM_GETAUTOURLDETECT: u32 = WM_USER + 92;
const EM_GETTEXTEX: u32 = WM_USER + 94;
const EM_GETTEXTLENGTHEX: u32 = WM_USER + 95;
const EM_SHOWSCROLLBAR: u32 = WM_USER + 96;
const EM_SETTEXTEX: u32 = WM_USER + 97;
const EM_SETLANGOPTIONS: u32 = WM_USER + 120;
const EM_GETLANGOPTIONS: u32 = WM_USER + 121;
const EM_SETEDITSTYLE: u32 = WM_USER + 204;
const EM_GETEDITSTYLE: u32 = WM_USER + 205;
const EM_SETTYPOGRAPHYOPTIONS: u32 = WM_USER + 202;
const EM_GETTYPOGRAPHYOPTIONS: u32 = WM_USER + 203;
const EM_GETSCROLLPOS: u32 = WM_USER + 221;
const EM_SETSCROLLPOS: u32 = WM_USER + 222;
const EM_GETZOOM: u32 = WM_USER + 224;
const EM_SETZOOM: u32 = WM_USER + 225;

const EN_SETFOCUS: u32 = 0x0100;
const EN_KILLFOCUS: u32 = 0x0200;
const EN_CHANGE: u32 = 0x0300;
const EN_UPDATE: u32 = 0x0400;
const EN_MAXTEXT: u32 = 0x0501;
const EN_VSCROLL: u32 = 0x0602;
const EN_SELCHANGE: u32 = 0x0702;
const EN_LINK: u32 = 0x070b;
const ENM_CHANGE: u32 = 0x1;
const ENM_UPDATE: u32 = 0x2;
const ENM_SCROLL: u32 = 0x4;
const ENM_SELCHANGE: u32 = 0x0008_0000;
const ENM_LINK: u32 = 0x0400_0000;

const SCF_SELECTION: usize = 0x1;
const SCF_WORD: usize = 0x2;
const SCF_ALL: usize = 0x4;
const GT_USECRLF: u32 = 1;
const GT_SELECTION: u32 = 2;
const GTL_USECRLF: u32 = 1;
const GTL_PRECISE: u32 = 2;
const GTL_CLOSE: u32 = 4;
const GTL_NUMCHARS: u32 = 8;
const GTL_NUMBYTES: u32 = 16;
const TM_PLAINTEXT: u32 = 1;
const TM_RICHTEXT: u32 = 2;
const ST_SELECTION: u32 = 2;
const CP_UNICODE: u32 = 1200;
const E_INVALIDARG: isize = 0x8007_0057_u32 as i32 as isize;
const E_UNEXPECTED: isize = 0x8000_ffff_u32 as i32 as isize;
/// The style bits EM_SETOPTIONS's options are (ECO_AUTOVSCROLL ..
/// ECO_SELECTIONBAR share their values with ES_ styles).
const STYLE_OPTIONS: u32 =
    0x40 | 0x80 | 0x100 | 0x800 | 0x1000 | 0x8000 | 0x0040_0000 | 0x0100_0000;
const ECO_AUTOWORDSELECTION: u32 = 0x1;
const GWLP_ID: i32 = -12;
/// The default limit for 0 (EM_EXLIMITTEXT, EM_LIMITTEXT): 64K.
const LIMIT_ZERO: usize = 65536;

static EDITORS: Store<Editor> = Store::new();

fn with<R>(hwnd: Hwnd, ansi: bool, f: impl FnOnce(&mut Editor) -> R) -> Option<R> {
    EDITORS.with(hwnd, || Editor::new(ansi), f)
}

fn redraw(hwnd: Hwnd) {
    // SAFETY: plain call.
    unsafe { InvalidateRect(hwnd, core::ptr::null(), 0) };
}

fn id_of(hwnd: Hwnd) -> usize {
    // SAFETY: plain call.
    unsafe { GetWindowLongPtrW(hwnd, GWLP_ID) as usize }
}

/// WM_COMMAND to the parent.
fn command(hwnd: Hwnd, code: u32) {
    // SAFETY: plain calls.
    unsafe {
        let parent = GetParent(hwnd);
        if parent != 0 {
            SendMessageW(
                parent,
                WM_COMMAND,
                (id_of(hwnd) & 0xffff) | (code as usize) << 16,
                hwnd as isize,
            );
        }
    }
}

/// WM_NOTIFY to the parent with `data` after the NMHDR (richedit.h's
/// structures are packed to 4 bytes); its answer.
fn notify(hwnd: Hwnd, code: u32, data: &[u8]) -> isize {
    let mut block = [0u64; 10];
    block[0] = hwnd as u64;
    block[1] = id_of(hwnd) as u64;
    block[2] = u64::from(code);
    // SAFETY: the bytes after the 24-byte NMHDR, within the block.
    let tail =
        unsafe { core::slice::from_raw_parts_mut(block.as_mut_ptr().add(3).cast::<u8>(), 56) };
    for (d, &v) in tail.iter_mut().zip(data) {
        *d = v;
    }
    // SAFETY: plain calls; the block (an NMHDR and what follows) lives
    // across the call.
    unsafe {
        let parent = GetParent(hwnd);
        if parent == 0 {
            return 0;
        }
        SendMessageW(parent, WM_NOTIFY, id_of(hwnd), block.as_mut_ptr() as isize)
    }
}

/// The scroll bar's information for the window, kept up to date.
fn update_bar(hwnd: Hwnd, ansi: bool) {
    let info = with(hwnd, ansi, |e| {
        view::layout(hwnd, e);
        view::scroll_info(hwnd, e)
    });
    if let Some(info) = info
        && style(hwnd) & view::WS_VSCROLL != 0
    {
        // SAFETY: our SCROLLINFO.
        unsafe { SetScrollInfo(hwnd, 1, &info, 1) };
    }
}

/// EN_SELCHANGE when the selection is not what the parent last heard.
fn selchange(hwnd: Hwnd, ansi: bool) {
    let send = with(hwnd, ansi, |e| {
        let sel = e.selection();
        if e.mask & ENM_SELCHANGE == 0 || sel == e.notified {
            return None;
        }
        e.notified = sel;
        Some(sel)
    })
    .flatten();
    if let Some((a, b)) = send {
        // SELCHANGE: CHARRANGE, then seltyp (SEL_EMPTY 0, SEL_TEXT 1).
        let mut data = [0u8; 10];
        data[..4].copy_from_slice(&(a as i32).to_le_bytes());
        data[4..8].copy_from_slice(&(b as i32).to_le_bytes());
        data[8] = u8::from(a != b);
        notify(hwnd, EN_SELCHANGE, &data);
    }
}

/// After the text changed: repaint, EN_UPDATE (when visible) and
/// EN_CHANGE as the event mask asks (ENM_CHANGE is masked off while
/// EN_CHANGE is sent), then EN_SELCHANGE.
fn changed(hwnd: Hwnd, ansi: bool) {
    redraw(hwnd);
    update_bar(hwnd, ansi);
    let mask = with(hwnd, ansi, |e| e.mask).unwrap_or(0);
    // SAFETY: plain call.
    if mask & ENM_UPDATE != 0 && unsafe { IsWindowVisible(hwnd) } != 0 {
        command(hwnd, EN_UPDATE);
    }
    if mask & ENM_CHANGE != 0 {
        with(hwnd, ansi, |e| e.mask &= !ENM_CHANGE);
        command(hwnd, EN_CHANGE);
        with(hwnd, ansi, |e| {
            if e.mask == mask & !ENM_CHANGE {
                e.mask = mask;
            }
        });
    }
    selchange(hwnd, ansi);
}

/// A NUL-terminated string from a message (ANSI for an ANSI window).
fn read_text(p: isize, ansi: bool) -> Vec<u16> {
    let mut out = Vec::new();
    if p == 0 {
        return out;
    }
    if ansi {
        // SAFETY: the caller's NUL-terminated string.
        let n = unsafe { strlen(p as *const u8) };
        return from_ansi(p as *const u8, n, 0);
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    while unsafe { *(p as *const u16).add(n) } != 0 {
        n += 1;
    }
    if out.try_reserve(n).is_ok() {
        // SAFETY: `n` characters.
        out.extend_from_slice(unsafe { core::slice::from_raw_parts(p as *const u16, n) });
    }
    out
}

/// `n` bytes in code page `cp` as UTF-16.
fn from_ansi(s: *const u8, n: usize, cp: u32) -> Vec<u16> {
    let mut out = Vec::new();
    if n == 0 || out.try_reserve(n).is_err() {
        return out;
    }
    out.resize(n, 0);
    // SAFETY: `n` bytes in; room for `n` characters out.
    let got = unsafe { MultiByteToWideChar(cp, 0, s, n as i32, out.as_mut_ptr(), n as i32) };
    out.truncate(got.max(0) as usize);
    out
}

/// `s` in code page `cp` (CP_ACP: 0).
fn to_ansi(s: &[u16], cp: u32) -> Vec<u8> {
    let mut out = Vec::new();
    if s.is_empty() {
        return out;
    }
    // SAFETY: plain calls with our buffers.
    unsafe {
        let need = WideCharToMultiByte(
            cp,
            0,
            s.as_ptr(),
            s.len() as i32,
            core::ptr::null_mut(),
            0,
            core::ptr::null(),
            core::ptr::null_mut(),
        )
        .max(0) as usize;
        if out.try_reserve(need).is_err() {
            return out;
        }
        out.resize(need, 0);
        let got = WideCharToMultiByte(
            cp,
            0,
            s.as_ptr(),
            s.len() as i32,
            out.as_mut_ptr(),
            need as i32,
            core::ptr::null(),
            core::ptr::null_mut(),
        );
        out.truncate(got.max(0) as usize);
    }
    out
}

/// `s` with CR as CRLF.
fn with_crlf(s: &[u16]) -> Vec<u16> {
    let mut out = Vec::new();
    if out.try_reserve(doc::crlf_len(s)).is_ok() {
        for &c in s {
            out.push(c);
            if c == CR {
                out.push(doc::LF);
            }
        }
    }
    out
}

/// Copies `s` into a caller's buffer of `room` units (characters, or
/// bytes for ANSI) with a NUL; the units copied.
fn copy_text(s: &[u16], crlf: bool, out: isize, room: usize, ansi: bool, cp: u32) -> usize {
    if out == 0 || room == 0 {
        return 0;
    }
    if !ansi {
        return doc::copy_out(s, crlf, out as *mut u16, room);
    }
    let wide = if crlf { with_crlf(s) } else { owned(s) };
    let bytes = to_ansi(&wide, cp);
    let n = bytes.len().min(room - 1);
    // SAFETY: the caller's buffer of `room` bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out as *mut u8, n);
        *(out as *mut u8).add(n) = 0;
    }
    n
}

fn owned(s: &[u16]) -> Vec<u16> {
    let mut v = Vec::new();
    if v.try_reserve(s.len()).is_ok() {
        v.extend_from_slice(s);
    }
    v
}

fn single_line(hwnd: Hwnd) -> bool {
    style(hwnd) & ES_MULTILINE == 0
}

/// Replaces the selection with `text` (paragraph marks already CR), cut
/// to the limit; then the notifications.
fn replace_selection(hwnd: Hwnd, ansi: bool, text: &[u16]) {
    let r = with(hwnd, ansi, |e| {
        let (a, b) = e.selection();
        let room = e.limit.saturating_sub(e.doc.len() - (b - a));
        let n = text.len().min(room);
        let format = e.insert_format();
        if (n == 0 && a == b) || !e.doc.replace(a, b, &text[..n], format) {
            return (false, n < text.len());
        }
        e.anchor = a + n;
        e.caret = a + n;
        e.insert = None;
        e.modified = true;
        e.changed();
        view::caret_into_view(hwnd, e);
        (true, n < text.len())
    });
    let Some((done, cut)) = r else { return };
    if cut {
        command(hwnd, EN_MAXTEXT);
    }
    if done {
        changed(hwnd, ansi);
    }
}

/// EM_SETSEL's and EM_EXSETSEL's selection: (0, -1) all, a negative
/// start no selection (the caret at its end), a negative end the end of
/// the text; the larger end.
fn set_selection(hwnd: Hwnd, ansi: bool, start: i64, end: i64) -> isize {
    let r = with(hwnd, ansi, |e| {
        let len = e.doc.len();
        let old = (e.anchor, e.caret);
        if start == 0 && end == -1 {
            e.anchor = 0;
            e.caret = len;
        } else if start < 0 {
            let (_, b) = e.selection();
            e.anchor = b;
            e.caret = b;
        } else {
            let clamp = |v: i64| if v < 0 { len } else { (v as usize).min(len) };
            e.anchor = clamp(start);
            e.caret = clamp(end);
        }
        if (e.anchor, e.caret) != old {
            e.insert = None;
        }
        e.selection().1 as isize
    });
    redraw(hwnd);
    selchange(hwnd, ansi);
    r.unwrap_or(0)
}

/// The format of `a..b` as EM_GETCHARFORMAT reports it: the first
/// character's (with CFE_LINK in a detected URL) and the attributes all
/// characters share.
fn range_format(e: &Editor, a: usize, b: usize) -> (Format, u32) {
    let effective = |pos: usize| {
        let mut f = *e.format(e.doc.format_at(pos));
        if e.links.iter().any(|l| l.0 <= pos && pos < l.1) {
            f.effects |= CFE_LINK;
        }
        f
    };
    if a >= b {
        let mut f = *e.format(e.insert_format());
        if a < e.doc.len() && e.links.iter().any(|l| l.0 <= a && a < l.1) {
            f.effects |= CFE_LINK;
        }
        return (f, CFM_ALL2);
    }
    let first = effective(a);
    let mut mask = CFM_ALL2;
    // Every run start and link edge inside the range.
    let mut points: Vec<usize> = e.doc.pieces(a, b).map(|p| p.0).collect();
    for &(s, t) in &e.links {
        for p in [s, t] {
            if p > a && p < b && points.try_reserve(1).is_ok() {
                points.push(p);
            }
        }
    }
    for p in points {
        mask &= doc::agree(&first, &effective(p));
    }
    (first, mask)
}

/// The word around `pos` (letters and digits).
fn word_at(e: &Editor, pos: usize) -> (usize, usize) {
    let t = &e.doc.text;
    let is_word = |c: u16| {
        c == 0x5f
            || (0x30..=0x39).contains(&c)
            || (0x41..=0x5a).contains(&c)
            || (0x61..=0x7a).contains(&c)
            || c >= 0xc0
    };
    let mut a = pos.min(t.len());
    while a > 0 && is_word(t[a - 1]) {
        a -= 1;
    }
    let mut b = pos.min(t.len());
    while b < t.len() && is_word(t[b]) {
        b += 1;
    }
    (a, b)
}

fn set_style_bits(hwnd: Hwnd, bits: u32, on: bool) {
    let s = style(hwnd);
    let new = if on { s | bits } else { s & !bits };
    if new != s {
        // SAFETY: plain call.
        unsafe { SetWindowLongW(hwnd, view::GWL_STYLE, new as i32) };
    }
}

/// The client point of a mouse message's lParam.
fn point(lparam: isize) -> (i32, i32) {
    (lparam as i16 as i32, (lparam >> 16) as i16 as i32)
}

/// The character position nearest a client point.
fn pos_at(hwnd: Hwnd, e: &mut Editor, x: i32, y: i32) -> usize {
    view::layout(hwnd, e);
    let v = view::view(hwnd, e);
    let line = e.line_at_y(y - v[1] + e.top);
    e.pos_in_line(line, x - v[0] + e.left)
}

/// The character under a client point (none past a line's end).
fn char_under(hwnd: Hwnd, e: &mut Editor, x: i32, y: i32) -> Option<usize> {
    view::layout(hwnd, e);
    let v = view::view(hwnd, e);
    let dy = y - v[1] + e.top;
    let line = e.line_at_y(dy);
    let l = &e.lines[line];
    if dy < l.y || dy >= l.y + l.h {
        return None;
    }
    let dx = x - v[0] + e.left - l.indent;
    (0..l.end - l.start).find(|&i| l.xs[i] <= dx && dx < l.xs[i + 1]).map(|i| l.start + i)
}

/// EN_LINK for a mouse message over a link (when ENM_LINK asks): true
/// when the parent answered nonzero (the control then ignores it).
fn link_notify(hwnd: Hwnd, ansi: bool, msg: u32, wparam: usize, lparam: isize) -> bool {
    let (x, y) = point(lparam);
    let link = with(hwnd, ansi, |e| {
        if e.mask & ENM_LINK == 0 {
            return None;
        }
        let at = char_under(hwnd, e, x, y)?;
        e.link_at(at)
    })
    .flatten();
    let Some((a, b)) = link else { return false };
    // ENLINK: msg, wParam, lParam, CHARRANGE (packed to 4 bytes).
    let mut data = [0u8; 28];
    data[..4].copy_from_slice(&msg.to_le_bytes());
    data[4..12].copy_from_slice(&(wparam as u64).to_le_bytes());
    data[12..20].copy_from_slice(&(lparam as u64).to_le_bytes());
    data[20..24].copy_from_slice(&(a as i32).to_le_bytes());
    data[24..28].copy_from_slice(&(b as i32).to_le_bytes());
    notify(hwnd, EN_LINK, &data) != 0
}

/// Scrolls to `top` (pixels, clamped); EN_VSCROLL first when `user` and
/// ENM_SCROLL ask.
fn scroll_to(hwnd: Hwnd, ansi: bool, top: impl FnOnce(&Editor, i32) -> i32, user: bool) {
    let r = with(hwnd, ansi, |e| {
        view::layout(hwnd, e);
        let max = view::max_top(hwnd, e);
        let new = top(e, max).clamp(0, max);
        let moved = new != e.top;
        e.top = new;
        (moved, e.mask)
    });
    let Some((moved, mask)) = r else { return };
    if moved {
        if user && mask & ENM_SCROLL != 0 {
            command(hwnd, EN_VSCROLL);
        }
        update_bar(hwnd, ansi);
        redraw(hwnd);
    }
}

/// WM_VSCROLL and EM_SCROLL: the new top for a scroll bar request.
fn scroll_request(e: &Editor, code: usize, track: i32, view_h: i32) -> i32 {
    let first = e.first_visible();
    let line_h = e.lines.get(first).map_or(16, |l| l.h);
    match code {
        0 => e.top - line_h,
        1 => e.lines.get(first + 1).map_or(e.top + line_h, |l| l.y),
        2 => e.top - view_h,
        3 => e.top + view_h,
        4 | 5 => track,
        6 => 0,
        7 => i32::MAX,
        _ => e.top,
    }
}

/// Moves the caret by a key; true when it was a movement key.
fn move_key(hwnd: Hwnd, ansi: bool, vk: usize) -> bool {
    // SAFETY: plain calls.
    let (shift, ctrl) = unsafe { (GetKeyState(0x10) < 0, GetKeyState(0x11) < 0) };
    let r = with(hwnd, ansi, |e| {
        view::layout(hwnd, e);
        let len = e.doc.len();
        let (a, b) = e.selection();
        let line = e.line_of(e.caret);
        let x = e.x_of(line, e.caret);
        let v = view::view(hwnd, e);
        let page = (v[3] - v[1]).max(1);
        let to = match vk {
            0x25 if !shift && a != b => a,
            0x27 if !shift && a != b => b,
            0x25 if ctrl => {
                let mut p = e.caret;
                while p > 0 && e.doc.text[p - 1] == 0x20 {
                    p -= 1;
                }
                word_at(e, p.saturating_sub(1)).0.min(p.saturating_sub(1))
            }
            0x27 if ctrl => {
                let mut p = word_at(e, e.caret).1.max((e.caret + 1).min(len));
                while p < len && e.doc.text[p] == 0x20 {
                    p += 1;
                }
                p
            }
            0x25 => e.caret.saturating_sub(1),
            0x27 => (e.caret + 1).min(len),
            0x26 if line > 0 => e.pos_in_line(line - 1, x),
            0x26 => 0,
            0x28 if line + 1 < e.lines.len() => e.pos_in_line(line + 1, x),
            0x28 => len,
            0x24 if ctrl => 0,
            0x23 if ctrl => len,
            0x24 => e.lines[line].start,
            0x23 => e.lines[line].end,
            0x21 => {
                let y = e.lines[line].y - page;
                let l = e.line_at_y(y.max(0));
                e.pos_in_line(l, x)
            }
            0x22 => {
                let y = e.lines[line].y + page;
                let l = e.line_at_y(y);
                e.pos_in_line(l, x)
            }
            _ => return false,
        };
        e.caret = to;
        if !shift {
            e.anchor = to;
        }
        e.insert = None;
        view::caret_into_view(hwnd, e);
        true
    });
    if r != Some(true) {
        return false;
    }
    update_bar(hwnd, ansi);
    redraw(hwnd);
    selchange(hwnd, ansi);
    true
}

/// Deletes the selection, or the character before (`back`) or after the
/// caret, unless read-only.
fn delete_key(hwnd: Hwnd, ansi: bool, back: bool) {
    if style(hwnd) & ES_READONLY != 0 {
        return;
    }
    let ok = with(hwnd, ansi, |e| {
        let (a, b) = e.selection();
        if a == b {
            if back && a > 0 {
                e.anchor = a - 1;
            } else if !back && b < e.doc.len() {
                e.caret = b + 1;
            } else {
                return false;
            }
        }
        true
    });
    if ok == Some(true) {
        replace_selection(hwnd, ansi, &[]);
    }
}

/// Text ranges from EM_GETTEXTRANGE's TEXTRANGE: (start, end), or None.
fn text_range(e: &Editor, at: isize) -> Option<(usize, usize, isize)> {
    if at == 0 {
        return None;
    }
    // SAFETY: the caller's TEXTRANGE: CHARRANGE, then the buffer.
    let (a, b, buf) = unsafe {
        let p = at as *const i32;
        (
            p.read_unaligned(),
            p.add(1).read_unaligned(),
            (at as *const isize).add(1).read_unaligned(),
        )
    };
    let len = e.doc.len();
    let b = if b < 0 { len } else { (b as usize).min(len) };
    if a < 0 || a as usize > b || buf == 0 {
        return None;
    }
    Some((a as usize, b, buf))
}

/// EM_SETTEXTEX's and EM_STREAMIN's RTF is not read yet.
fn is_rtf(s: &[u16]) -> bool {
    s.len() >= 5 && s[..5].iter().zip(b"{\\rtf").all(|(&c, &b)| c == u16::from(b))
}

/// WM_SETFONT: the font's face, size and weight become the default
/// format's, and (rich text) all text's.
fn set_font(hwnd: Hwnd, ansi: bool, font: usize, redraw_now: bool) {
    let mut lf = [0u8; 92];
    // SAFETY: a LOGFONTW-sized buffer.
    if font == 0 || unsafe { GetObjectW(font, 92, lf.as_mut_ptr()) } < 92 {
        with(hwnd, ansi, |e| e.font = font);
        return;
    }
    // SAFETY: the LOGFONTW GetObjectW wrote.
    let lf = unsafe { (lf.as_ptr() as *const LogFont).read_unaligned() };
    let change = move |f: &Format| {
        let mut g = *f;
        g.face = [0; 32];
        let n = lf.face.iter().position(|&c| c == 0).unwrap_or(31).min(31);
        g.face[..n].copy_from_slice(&lf.face[..n]);
        g.height = view::px_to_twips(lf.height);
        g.weight = if lf.weight == 0 { 400 } else { lf.weight as u16 };
        g.charset = lf.char_set;
        g.pitch = lf.pitch_and_family;
        let bit = |on: u8, b: u32, e: u32| if on != 0 { e | b } else { e & !b };
        g.effects = bit(u8::from(g.weight >= 700), doc::CFE_BOLD, g.effects);
        g.effects = bit(lf.italic, 0x2, g.effects);
        g.effects = bit(lf.underline, doc::CFE_UNDERLINE, g.effects);
        g.effects = bit(lf.strike_out, doc::CFE_STRIKEOUT, g.effects);
        g
    };
    with(hwnd, ansi, |e| {
        e.font = font;
        e.doc.formats[0] = change(&e.doc.formats[0]);
        if e.mode & TM_RICHTEXT != 0 {
            let len = e.doc.len();
            e.doc.format_range(0, len, change);
        }
        e.dirty = true;
    });
    update_bar(hwnd, ansi);
    if redraw_now {
        redraw(hwnd);
    }
}

const PARAFORMAT_SIZE: usize = 156;
const PARAFORMAT2_SIZE: usize = 188;
const PFM_STARTINDENT: u32 = 0x1;
const PFM_RIGHTINDENT: u32 = 0x2;
const PFM_OFFSET: u32 = 0x4;
const PFM_ALIGNMENT: u32 = 0x8;
const PFM_TABSTOPS: u32 = 0x10;
const PFM_OFFSETINDENT: u32 = 0x8000_0000;
/// What gala keeps of a paragraph format.
const PARA_KEPT: u32 =
    PFM_STARTINDENT | PFM_RIGHTINDENT | PFM_OFFSET | PFM_ALIGNMENT | PFM_TABSTOPS;

/// EM_SETPARAFORMAT: indents, alignment and tab stops, for the whole
/// control (gala keeps one paragraph format); other attributes are
/// recorded as unsupported.
fn set_para(hwnd: Hwnd, ansi: bool, p: usize) -> isize {
    if p == 0 {
        return 0;
    }
    let size = rd32(p, 0) as usize;
    if size != PARAFORMAT_SIZE && size != PARAFORMAT2_SIZE {
        return 0;
    }
    let mask = rd32(p, 4);
    let other = mask & !(PARA_KEPT | PFM_OFFSETINDENT);
    if other != 0 {
        unsupported(b"riched20.dll!EM_SETPARAFORMAT", u64::from(other));
    }
    let field = |at: usize| rd32(p, at) as i32;
    with(hwnd, ansi, |e| {
        let para = &mut e.para;
        if mask & PFM_OFFSETINDENT != 0 {
            para.start += field(12);
        } else if mask & PFM_STARTINDENT != 0 {
            para.start = field(12);
        }
        if mask & PFM_RIGHTINDENT != 0 {
            para.right = field(16);
        }
        if mask & PFM_OFFSET != 0 {
            para.offset = field(20);
        }
        if mask & PFM_ALIGNMENT != 0 {
            let align = (rd32(p, 24) & 0xffff) as u16;
            if (1..=4).contains(&align) {
                para.align = align;
            }
        }
        if mask & PFM_TABSTOPS != 0 {
            let count = ((rd32(p, 24) >> 16) as usize).min(32);
            para.tab_count = count;
            for i in 0..count {
                para.tabs[i] = field(28 + 4 * i);
            }
        }
        e.dirty = true;
    });
    update_bar(hwnd, ansi);
    redraw(hwnd);
    1
}

pub extern "system" fn proc_w(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    window_proc(hwnd, msg, wparam, lparam, false)
}

pub extern "system" fn proc_a(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    window_proc(hwnd, msg, wparam, lparam, true)
}

fn rd32(p: usize, at: usize) -> u32 {
    // SAFETY: the caller's structure, `at` inside it.
    unsafe { ((p + at) as *const u32).read_unaligned() }
}

fn window_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize, ansi: bool) -> isize {
    match msg {
        WM_CREATE => {
            with(hwnd, ansi, |_| ());
            // The window's name is the initial text (gala's CREATESTRUCT is
            // always wide: user32 has no ANSI windows).
            let name = if lparam != 0 {
                // SAFETY: the CREATESTRUCTW: lpszName at offset 56.
                unsafe { ((lparam as usize + 56) as *const isize).read_unaligned() }
            } else {
                0
            };
            if name != 0 && (name as usize) >= 0x10000 {
                let text = doc::to_marks(&read_text(name, false), single_line(hwnd));
                with(hwnd, ansi, |e| {
                    e.doc.replace(0, 0, &text, 0);
                    e.changed();
                });
            }
            update_bar(hwnd, ansi);
            0
        }
        WM_DESTROY => {
            if let Some(mut e) = EDITORS.remove(hwnd) {
                e.drop_fonts();
            }
            0
        }
        WM_SIZE | WM_STYLECHANGED => {
            with(hwnd, ansi, |e| e.dirty = true);
            update_bar(hwnd, ansi);
            redraw(hwnd);
            // SAFETY: plain call.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut ps = [0u8; 72];
            // SAFETY: a PAINTSTRUCT-sized buffer.
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            if hdc != 0 {
                // SAFETY: plain call.
                let focused = unsafe { GetFocus() } == hwnd;
                with(hwnd, ansi, |e| view::paint(hwnd, e, hdc, focused));
                // SAFETY: the PAINTSTRUCT BeginPaint filled.
                unsafe { EndPaint(hwnd, &ps) };
            }
            0
        }
        WM_ENABLE => {
            redraw(hwnd);
            0
        }
        WM_SETFOCUS => {
            redraw(hwnd);
            command(hwnd, EN_SETFOCUS);
            0
        }
        WM_KILLFOCUS => {
            redraw(hwnd);
            command(hwnd, EN_KILLFOCUS);
            0
        }
        WM_SETFONT => {
            set_font(hwnd, ansi, wparam, lparam & 0xffff != 0);
            0
        }
        WM_GETFONT => with(hwnd, ansi, |e| e.font as isize).unwrap_or(0),
        WM_GETDLGCODE => {
            // DLGC_WANTCHARS | DLGC_HASSETSEL | DLGC_WANTARROWS, and every
            // key for several lines.
            0x80 | 0x8 | 0x1 | if single_line(hwnd) { 0 } else { 0x4 }
        }
        WM_SETTEXT => {
            let text = doc::to_marks(&read_text(lparam, ansi), single_line(hwnd));
            let ok = with(hwnd, ansi, |e| {
                let len = e.doc.len();
                let format = if e.mode & TM_PLAINTEXT != 0 { e.doc.format_at(0) } else { 0 };
                if !e.doc.replace(0, len, &text, format) {
                    return false;
                }
                e.anchor = 0;
                e.caret = 0;
                e.insert = None;
                e.top = 0;
                e.left = 0;
                e.modified = false;
                e.changed();
                true
            });
            if ok != Some(true) {
                return 0;
            }
            changed(hwnd, ansi);
            1
        }
        WM_GETTEXT => {
            with(hwnd, ansi, |e| copy_text(&e.doc.text, true, lparam, wparam, e.ansi, 0) as isize)
                .unwrap_or(0)
        }
        WM_GETTEXTLENGTH => with(hwnd, ansi, |e| {
            if e.ansi {
                to_ansi(&with_crlf(&e.doc.text), 0).len() as isize
            } else {
                doc::crlf_len(&e.doc.text) as isize
            }
        })
        .unwrap_or(0),
        EM_GETTEXTEX => {
            if wparam == 0 || lparam == 0 {
                return 0;
            }
            let (cb, flags, cp) = (rd32(wparam, 0) as usize, rd32(wparam, 4), rd32(wparam, 8));
            with(hwnd, ansi, |e| {
                let (a, b) =
                    if flags & GT_SELECTION != 0 { e.selection() } else { (0, e.doc.len()) };
                let crlf = flags & GT_USECRLF != 0;
                let s = e.doc.slice(a, b);
                if cp == CP_UNICODE {
                    doc::copy_out(s, crlf, lparam as *mut u16, cb / 2) as isize
                } else {
                    copy_text(s, crlf, lparam, cb, true, cp) as isize
                }
            })
            .unwrap_or(0)
        }
        EM_GETTEXTLENGTHEX => {
            if wparam == 0 {
                return E_INVALIDARG;
            }
            let (flags, cp) = (rd32(wparam, 0), rd32(wparam, 4));
            if flags & (GTL_NUMCHARS | GTL_NUMBYTES) == GTL_NUMCHARS | GTL_NUMBYTES
                || flags & (GTL_PRECISE | GTL_CLOSE) == GTL_PRECISE | GTL_CLOSE
            {
                return E_INVALIDARG;
            }
            with(hwnd, ansi, |e| {
                let crlf = flags & GTL_USECRLF != 0;
                let chars = if crlf { doc::crlf_len(&e.doc.text) } else { e.doc.len() };
                if flags & GTL_NUMBYTES == 0 {
                    chars as isize
                } else if cp == CP_UNICODE {
                    2 * chars as isize
                } else {
                    let s = if crlf { with_crlf(&e.doc.text) } else { owned(&e.doc.text) };
                    to_ansi(&s, cp).len() as isize
                }
            })
            .unwrap_or(0)
        }
        EM_SETTEXTEX => {
            let (flags, cp) = if wparam != 0 { (rd32(wparam, 0), rd32(wparam, 4)) } else { (0, 0) };
            let raw = if lparam == 0 {
                Vec::new()
            } else if cp == CP_UNICODE || (!ansi && cp == 0xffff_ffff) {
                read_text(lparam, false)
            } else {
                // SAFETY: the caller's NUL-terminated string.
                let n = unsafe { strlen(lparam as *const u8) };
                from_ansi(lparam as *const u8, n, if cp == 0xffff_ffff { 0 } else { cp })
            };
            if is_rtf(&raw) {
                unsupported(b"riched20.dll!EM_SETTEXTEX(RTF)", u64::from(flags));
                return 0;
            }
            let text = doc::to_marks(&raw, single_line(hwnd));
            if flags & ST_SELECTION != 0 {
                replace_selection(hwnd, ansi, &text);
                text.len() as isize
            } else {
                with(hwnd, ansi, |e| {
                    let len = e.doc.len();
                    if e.doc.replace(0, len, &text, 0) {
                        e.anchor = 0;
                        e.caret = 0;
                        e.insert = None;
                        e.top = 0;
                        e.changed();
                    }
                });
                changed(hwnd, ansi);
                1
            }
        }
        EM_REPLACESEL => {
            let text = doc::to_marks(&read_text(lparam, ansi), single_line(hwnd));
            replace_selection(hwnd, ansi, &text);
            0
        }
        EM_GETSEL => with(hwnd, ansi, |e| {
            let (a, b) = e.selection();
            // SAFETY: the caller's DWORDs, when given.
            unsafe {
                if wparam != 0 {
                    (wparam as *mut u32).write_unaligned(a as u32);
                }
                if lparam != 0 {
                    (lparam as *mut u32).write_unaligned(b as u32);
                }
            }
            if b > 0xffff { -1 } else { (a | b << 16) as isize }
        })
        .unwrap_or(0),
        EM_SETSEL => set_selection(hwnd, ansi, wparam as isize as i64, lparam as i64),
        EM_EXGETSEL => {
            if lparam != 0 {
                let (a, b) = with(hwnd, ansi, |e| e.selection()).unwrap_or((0, 0));
                // SAFETY: the caller's CHARRANGE.
                unsafe {
                    (lparam as *mut i32).write_unaligned(a as i32);
                    (lparam as *mut i32).add(1).write_unaligned(b as i32);
                }
            }
            0
        }
        EM_EXSETSEL => {
            if lparam == 0 {
                return 0;
            }
            // SAFETY: the caller's CHARRANGE.
            let (a, b) = unsafe {
                (
                    (lparam as *const i32).read_unaligned(),
                    (lparam as *const i32).add(1).read_unaligned(),
                )
            };
            set_selection(hwnd, ansi, i64::from(a), i64::from(b))
        }
        EM_GETSELTEXT => with(hwnd, ansi, |e| {
            let (a, b) = e.selection();
            copy_text(e.doc.slice(a, b), false, lparam, usize::MAX, e.ansi, 0) as isize
        })
        .unwrap_or(0),
        EM_GETTEXTRANGE => with(hwnd, ansi, |e| match text_range(e, lparam) {
            Some((a, b, buf)) => {
                copy_text(e.doc.slice(a, b), false, buf, usize::MAX, e.ansi, 0) as isize
            }
            None => 0,
        })
        .unwrap_or(0),
        EM_SELECTIONTYPE => with(hwnd, ansi, |e| {
            let (a, b) = e.selection();
            // SEL_EMPTY, SEL_TEXT | SEL_MULTICHAR
            if a == b {
                0
            } else if b - a == 1 {
                1
            } else {
                1 | 4
            }
        })
        .unwrap_or(0),
        EM_SETCHARFORMAT => {
            if doc::kind(lparam as *const u8).is_none() {
                return 0;
            }
            let cf = lparam as *const u8;
            let ok = with(hwnd, ansi, |e| {
                let len = e.doc.len();
                let plain = e.mode & TM_PLAINTEXT != 0;
                if wparam & SCF_ALL != 0 || (plain && wparam & SCF_SELECTION != 0) {
                    e.doc.format_range(0, len, |f| doc::apply(cf, f));
                    let f = doc::apply(cf, e.format(e.insert_format()));
                    if len == 0 {
                        e.insert = Some(e.doc.intern(f));
                    }
                } else if wparam & SCF_SELECTION != 0 {
                    let (mut a, mut b) = e.selection();
                    if a == b && wparam & SCF_WORD != 0 {
                        (a, b) = word_at(e, a);
                    }
                    if a == b {
                        let f = doc::apply(cf, e.format(e.insert_format()));
                        e.insert = Some(e.doc.intern(f));
                    } else {
                        e.doc.format_range(a, b, |f| doc::apply(cf, f));
                    }
                } else {
                    e.doc.formats[0] = doc::apply(cf, &e.doc.formats[0]);
                }
                e.dirty = true;
                true
            });
            redraw(hwnd);
            update_bar(hwnd, ansi);
            isize::from(ok == Some(true))
        }
        EM_GETCHARFORMAT => {
            if doc::kind(lparam as *const u8).is_none() {
                return 0;
            }
            with(hwnd, ansi, |e| {
                let (f, mask) = if wparam == 0 {
                    (e.doc.formats[0], CFM_ALL2)
                } else {
                    let (a, b) = e.selection();
                    range_format(e, a, b)
                };
                doc::fill(lparam as *mut u8, &f, mask) as isize
            })
            .unwrap_or(0)
        }
        EM_SETBKGNDCOLOR => {
            let old = with(hwnd, ansi, |e| {
                let old = e.bg;
                e.bg = if wparam != 0 { None } else { Some(lparam as u32) };
                old
            })
            .flatten();
            redraw(hwnd);
            // SAFETY: plain call (COLOR_WINDOW).
            old.unwrap_or_else(|| unsafe { GetSysColor(5) }) as isize
        }
        EM_SETEVENTMASK => with(hwnd, ansi, |e| {
            let old = e.mask;
            e.mask = lparam as u32;
            e.notified = e.selection();
            old as isize
        })
        .unwrap_or(0),
        EM_GETEVENTMASK => with(hwnd, ansi, |e| e.mask as isize).unwrap_or(0),
        EM_SETREADONLY => {
            set_style_bits(hwnd, ES_READONLY, wparam != 0);
            redraw(hwnd);
            1
        }
        EM_SETOPTIONS => {
            let s = style(hwnd);
            let word = with(hwnd, ansi, |e| e.word_select).unwrap_or(false);
            let cur = (s & STYLE_OPTIONS) | if word { ECO_AUTOWORDSELECTION } else { 0 };
            let options = lparam as u32;
            let new = match wparam {
                1 => options,
                2 => cur | options,
                3 => cur & options,
                4 => cur ^ options,
                _ => return 0,
            } & (STYLE_OPTIONS | ECO_AUTOWORDSELECTION);
            let ns = (s & !STYLE_OPTIONS) | (new & STYLE_OPTIONS);
            if ns != s {
                // SAFETY: plain call.
                unsafe { SetWindowLongW(hwnd, view::GWL_STYLE, ns as i32) };
            }
            with(hwnd, ansi, |e| e.word_select = new & ECO_AUTOWORDSELECTION != 0);
            redraw(hwnd);
            new as isize
        }
        EM_GETOPTIONS => {
            let word = with(hwnd, ansi, |e| e.word_select).unwrap_or(false);
            ((style(hwnd) & STYLE_OPTIONS) | if word { ECO_AUTOWORDSELECTION } else { 0 }) as isize
        }
        EM_EXLIMITTEXT | EM_LIMITTEXT => {
            let n = if msg == EM_LIMITTEXT { wparam } else { lparam as usize };
            with(hwnd, ansi, |e| e.limit = if n == 0 { LIMIT_ZERO } else { n });
            0
        }
        EM_GETLIMITTEXT | EM_GETLIMITTEXT_RE => with(hwnd, ansi, |e| e.limit as isize).unwrap_or(0),
        EM_GETLINECOUNT => with(hwnd, ansi, |e| {
            view::layout(hwnd, e);
            e.lines.len() as isize
        })
        .unwrap_or(1),
        EM_LINEINDEX => with(hwnd, ansi, |e| {
            view::layout(hwnd, e);
            let line = if wparam as isize == -1 { e.line_of(e.caret) } else { wparam };
            e.lines.get(line).map_or(-1, |l| l.start as isize)
        })
        .unwrap_or(-1),
        EM_LINEFROMCHAR | EM_EXLINEFROMCHAR => with(hwnd, ansi, |e| {
            view::layout(hwnd, e);
            let at = if msg == EM_EXLINEFROMCHAR { lparam } else { wparam as isize };
            let pos = if at < 0 {
                let (a, b) = e.selection();
                if a != b { a } else { e.caret }
            } else {
                at as usize
            };
            e.line_of(pos) as isize
        })
        .unwrap_or(0),
        EM_LINELENGTH => with(hwnd, ansi, |e| {
            view::layout(hwnd, e);
            if wparam as isize == -1 {
                let (a, b) = e.selection();
                let (la, lb) = (e.line_of(a), e.line_of(b));
                if a == b {
                    let l = &e.lines[la];
                    return (l.end - l.start) as isize;
                }
                ((a - e.lines[la].start) + e.lines[lb].end.saturating_sub(b)) as isize
            } else {
                let l = &e.lines[e.line_of(wparam.min(e.doc.len()))];
                (l.end - l.start) as isize
            }
        })
        .unwrap_or(0),
        EM_GETLINE => with(hwnd, ansi, |e| {
            view::layout(hwnd, e);
            let Some(l) = e.lines.get(wparam) else { return 0 };
            if lparam == 0 {
                return 0;
            }
            // SAFETY: the caller's buffer: its size in the first WORD.
            let room = usize::from(unsafe { (lparam as *const u16).read_unaligned() });
            let s = e.doc.slice(l.start, l.end + usize::from(l.brk));
            if e.ansi {
                let bytes = to_ansi(s, 0);
                let n = bytes.len().min(room);
                // SAFETY: `n` bytes into the caller's buffer.
                unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), lparam as *mut u8, n) };
                n as isize
            } else {
                let n = s.len().min(room);
                // SAFETY: `n` characters into the caller's buffer.
                unsafe { core::ptr::copy_nonoverlapping(s.as_ptr(), lparam as *mut u16, n) };
                n as isize
            }
        })
        .unwrap_or(0),
        EM_GETFIRSTVISIBLELINE => with(hwnd, ansi, |e| {
            view::layout(hwnd, e);
            e.first_visible() as isize
        })
        .unwrap_or(0),
        EM_LINESCROLL => {
            let (dx, dy) = (wparam as isize, lparam);
            with(hwnd, ansi, |e| {
                if !view::wraps(hwnd, e) {
                    e.left = (e.left + dx as i32 * 8).clamp(0, e.widest.max(0));
                }
            });
            scroll_to(
                hwnd,
                ansi,
                |e, _| {
                    let first = e.first_visible() as isize;
                    let target = (first + dy).clamp(0, e.lines.len() as isize - 1) as usize;
                    e.lines[target].y
                },
                false,
            );
            redraw(hwnd);
            isize::from(!single_line(hwnd))
        }
        EM_SCROLL => {
            let before = with(hwnd, ansi, |e| {
                view::layout(hwnd, e);
                e.first_visible()
            })
            .unwrap_or(0);
            let vh = with(hwnd, ansi, |e| {
                let v = view::view(hwnd, e);
                v[3] - v[1]
            })
            .unwrap_or(0);
            if wparam > 3 {
                return 0;
            }
            scroll_to(hwnd, ansi, |e, _| scroll_request(e, wparam, 0, vh), false);
            let after = with(hwnd, ansi, |e| e.first_visible()).unwrap_or(0);
            let lines = after as isize - before as isize;
            (lines & 0xffff) | 1 << 16
        }
        EM_SCROLLCARET | EM_SCROLLCARET_RE => {
            with(hwnd, ansi, |e| view::caret_into_view(hwnd, e));
            update_bar(hwnd, ansi);
            redraw(hwnd);
            0
        }
        WM_VSCROLL => {
            let code = wparam & 0xffff;
            let track = (wparam >> 16) as i32;
            let vh = with(hwnd, ansi, |e| {
                let v = view::view(hwnd, e);
                v[3] - v[1]
            })
            .unwrap_or(0);
            scroll_to(hwnd, ansi, |e, _| scroll_request(e, code, track, vh), true);
            0
        }
        WM_MOUSEWHEEL => {
            let notches = (wparam >> 16) as i16 as i32 / 120;
            scroll_to(
                hwnd,
                ansi,
                |e, _| {
                    let h = e.lines.get(e.first_visible()).map_or(16, |l| l.h);
                    e.top - notches * 3 * h
                },
                true,
            );
            0
        }
        EM_GETSCROLLPOS => {
            if lparam != 0 {
                let (x, y) = with(hwnd, ansi, |e| (e.left, e.top)).unwrap_or((0, 0));
                // SAFETY: the caller's POINT.
                unsafe {
                    (lparam as *mut i32).write_unaligned(x);
                    (lparam as *mut i32).add(1).write_unaligned(y);
                }
            }
            1
        }
        EM_SETSCROLLPOS => {
            if lparam != 0 {
                // SAFETY: the caller's POINT.
                let (x, y) = unsafe {
                    (
                        (lparam as *const i32).read_unaligned(),
                        (lparam as *const i32).add(1).read_unaligned(),
                    )
                };
                with(hwnd, ansi, |e| e.left = x.clamp(0, e.widest.max(0)));
                scroll_to(hwnd, ansi, |_, _| y, false);
                redraw(hwnd);
            }
            1
        }
        EM_POSFROMCHAR | EM_POSFROMCHAR_RE => {
            // Rich edit 2.0 on: wParam a POINTL to fill, lParam the
            // position; the edit control's form returns a packed point.
            let pointer = wparam >= 0x10000;
            let pos = if pointer { lparam as usize } else { wparam };
            let p = with(hwnd, ansi, |e| {
                view::layout(hwnd, e);
                let v = view::view(hwnd, e);
                let pos = pos.min(e.doc.len());
                let line = e.line_of(pos);
                (v[0] - e.left + e.x_of(line, pos), v[1] + e.lines[line].y - e.top)
            });
            let Some((x, y)) = p else { return 0 };
            if pointer {
                // SAFETY: the caller's POINTL.
                unsafe {
                    (wparam as *mut i32).write_unaligned(x);
                    (wparam as *mut i32).add(1).write_unaligned(y);
                }
                0
            } else {
                ((x & 0xffff) | (y & 0xffff) << 16) as isize
            }
        }
        EM_CHARFROMPOS | EM_CHARFROMPOS_RE => {
            if lparam == 0 {
                return 0;
            }
            // SAFETY: the caller's POINTL.
            let (x, y) = unsafe {
                (
                    (lparam as *const i32).read_unaligned(),
                    (lparam as *const i32).add(1).read_unaligned(),
                )
            };
            with(hwnd, ansi, |e| pos_at(hwnd, e, x, y) as isize).unwrap_or(0)
        }
        EM_SETTEXTMODE => {
            let mode = wparam as u32;
            let groups = [(TM_PLAINTEXT | TM_RICHTEXT), (4 | 8), (16 | 32)];
            for g in groups {
                if mode & g == g {
                    return E_INVALIDARG;
                }
            }
            with(hwnd, ansi, |e| {
                if e.doc.len() != 0 {
                    return E_UNEXPECTED;
                }
                for g in groups {
                    if mode & g != 0 {
                        e.mode = (e.mode & !g) | (mode & g);
                    }
                }
                0
            })
            .unwrap_or(E_UNEXPECTED)
        }
        EM_GETTEXTMODE => with(hwnd, ansi, |e| e.mode as isize).unwrap_or(0),
        EM_AUTOURLDETECT => {
            with(hwnd, ansi, |e| {
                e.auto_url = wparam as u32;
                e.changed();
            });
            redraw(hwnd);
            0
        }
        EM_GETAUTOURLDETECT => with(hwnd, ansi, |e| e.auto_url as isize).unwrap_or(0),
        EM_GETMODIFY => with(hwnd, ansi, |e| isize::from(e.modified)).unwrap_or(0),
        EM_SETMODIFY => {
            with(hwnd, ansi, |e| e.modified = wparam != 0);
            0
        }
        EM_CANUNDO | EM_CANREDO | EM_CANPASTE => 0,
        EM_UNDO | EM_REDO => {
            unsupported(b"riched20.dll!EM_UNDO", u64::from(msg));
            0
        }
        EM_EMPTYUNDOBUFFER | EM_STOPGROUPTYPING | EM_SHOWSCROLLBAR | EM_REQUESTRESIZE => 0,
        EM_SETUNDOLIMIT => {
            with(hwnd, ansi, |e| e.undo_limit = wparam);
            wparam as isize
        }
        EM_HIDESELECTION => {
            with(hwnd, ansi, |e| e.hide_selection = wparam != 0);
            redraw(hwnd);
            0
        }
        EM_SETTARGETDEVICE => {
            with(hwnd, ansi, |e| {
                e.no_wrap = lparam == 1;
                e.dirty = true;
            });
            update_bar(hwnd, ansi);
            redraw(hwnd);
            1
        }
        EM_SETMARGINS => {
            with(hwnd, ansi, |e| {
                if wparam & 1 != 0 {
                    e.margins.0 = (lparam & 0xffff) as i32;
                }
                if wparam & 2 != 0 {
                    e.margins.1 = ((lparam >> 16) & 0xffff) as i32;
                }
                e.dirty = true;
            });
            redraw(hwnd);
            0
        }
        EM_GETMARGINS => {
            with(hwnd, ansi, |e| ((e.margins.0 & 0xffff) | (e.margins.1 & 0xffff) << 16) as isize)
                .unwrap_or(0)
        }
        EM_GETRECT => {
            if lparam != 0
                && let Some(v) = with(hwnd, ansi, |e| view::view(hwnd, e))
            {
                // SAFETY: the caller's RECT.
                unsafe { (lparam as *mut [i32; 4]).write_unaligned(v) };
            }
            0
        }
        EM_SETRECT | EM_SETRECTNP => {
            // SAFETY: the caller's RECT, when given.
            let r = (lparam != 0).then(|| unsafe { (lparam as *const [i32; 4]).read_unaligned() });
            with(hwnd, ansi, |e| {
                e.rect = r;
                e.dirty = true;
            });
            update_bar(hwnd, ansi);
            redraw(hwnd);
            0
        }
        EM_SETPARAFORMAT => set_para(hwnd, ansi, lparam as usize),
        EM_GETPARAFORMAT => {
            if lparam == 0 {
                return 0;
            }
            let p = lparam as usize;
            let size = rd32(p, 0) as usize;
            if size != PARAFORMAT_SIZE && size != PARAFORMAT2_SIZE {
                return 0;
            }
            let para = with(hwnd, ansi, |e| e.para).unwrap_or(view::DEFAULT_PARA);
            // SAFETY: the caller's PARAFORMAT of `size` bytes (packed to 4).
            unsafe {
                core::ptr::write_bytes((p as *mut u8).add(4), 0, size - 4);
                ((p + 4) as *mut u32).write_unaligned(PARA_KEPT);
                ((p + 12) as *mut i32).write_unaligned(para.start);
                ((p + 16) as *mut i32).write_unaligned(para.right);
                ((p + 20) as *mut i32).write_unaligned(para.offset);
                ((p + 24) as *mut u16).write_unaligned(para.align);
                ((p + 26) as *mut u16).write_unaligned(para.tab_count as u16);
                for (i, &t) in para.tabs[..para.tab_count].iter().enumerate() {
                    ((p + 28 + 4 * i) as *mut i32).write_unaligned(t);
                }
            }
            PARA_KEPT as isize
        }
        EM_SETLANGOPTIONS => {
            with(hwnd, ansi, |e| e.lang = lparam as u32);
            1
        }
        EM_GETLANGOPTIONS => with(hwnd, ansi, |e| e.lang as isize).unwrap_or(0),
        EM_SETEDITSTYLE => with(hwnd, ansi, |e| {
            let mask = lparam as u32;
            e.edit_style = (e.edit_style & !mask) | (wparam as u32 & mask);
            e.edit_style as isize
        })
        .unwrap_or(0),
        EM_GETEDITSTYLE => with(hwnd, ansi, |e| e.edit_style as isize).unwrap_or(0),
        EM_SETTYPOGRAPHYOPTIONS => 1,
        EM_GETTYPOGRAPHYOPTIONS => 0,
        EM_GETZOOM => {
            // SAFETY: the caller's numerator and denominator.
            unsafe {
                if wparam != 0 {
                    (wparam as *mut i32).write_unaligned(0);
                }
                if lparam != 0 {
                    (lparam as *mut i32).write_unaligned(0);
                }
            }
            1
        }
        EM_SETZOOM => {
            if wparam == 0 && lparam == 0 {
                return 1;
            }
            unsupported(b"riched20.dll!EM_SETZOOM", wparam as u64);
            0
        }
        EM_SETOLECALLBACK => {
            unsupported(b"riched20.dll!EM_SETOLECALLBACK", 0);
            0
        }
        EM_GETOLEINTERFACE => {
            if lparam != 0 {
                // SAFETY: the caller's pointer slot.
                unsafe { (lparam as *mut usize).write_unaligned(0) };
            }
            unsupported(b"riched20.dll!EM_GETOLEINTERFACE", 0);
            0
        }
        WM_CHAR => {
            let c = wparam as u16;
            let s = style(hwnd);
            match c {
                1 => {
                    // Ctrl+A: everything.
                    set_selection(hwnd, ansi, 0, -1);
                }
                3 | 0x16 | 0x18 => {
                    // Ctrl+C/V/X: no clipboard yet.
                    unsupported(b"riched20.dll!clipboard", u64::from(c));
                }
                8 => delete_key(hwnd, ansi, true),
                0x0d | 0x0a if s & ES_MULTILINE != 0 && s & ES_READONLY == 0 => {
                    replace_selection(hwnd, ansi, &[CR]);
                }
                0x09 if s & ES_MULTILINE != 0 && s & ES_READONLY == 0 => {
                    replace_selection(hwnd, ansi, &[0x09]);
                }
                c if c >= 0x20 && s & ES_READONLY == 0 => replace_selection(hwnd, ansi, &[c]),
                _ => {}
            }
            0
        }
        WM_KEYDOWN => {
            if wparam == 0x2e {
                delete_key(hwnd, ansi, false);
            } else {
                move_key(hwnd, ansi, wparam);
            }
            0
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            if link_notify(hwnd, ansi, msg, wparam, lparam) {
                return 0;
            }
            // SAFETY: plain call.
            unsafe {
                if GetFocus() != hwnd {
                    SetFocus(hwnd);
                }
            }
            let (x, y) = point(lparam);
            // The scroll bar: the thumb is grabbed, the track pages.
            let c = view::client(hwnd);
            let bar = style(hwnd) & view::WS_VSCROLL != 0 && x >= c[2] - view::BAR;
            if bar {
                let r = with(hwnd, ansi, |e| {
                    view::layout(hwnd, e);
                    view::thumb(hwnd, e)
                })
                .flatten();
                if let Some((ty, th)) = r {
                    if y < ty {
                        window_proc(hwnd, WM_VSCROLL, 2, 0, ansi);
                    } else if y >= ty + th {
                        window_proc(hwnd, WM_VSCROLL, 3, 0, ansi);
                    } else {
                        with(hwnd, ansi, |e| e.drag = Drag::Thumb(y - ty));
                        // SAFETY: plain call.
                        unsafe { SetCapture(hwnd) };
                    }
                }
                return 0;
            }
            // SAFETY: plain call.
            let shift = unsafe { GetKeyState(0x10) } < 0;
            with(hwnd, ansi, |e| {
                let at = pos_at(hwnd, e, x, y);
                if msg == WM_LBUTTONDBLCLK {
                    let (a, b) = word_at(e, at);
                    e.anchor = a;
                    e.caret = b;
                } else {
                    e.caret = at;
                    if !shift {
                        e.anchor = at;
                    }
                    e.drag = Drag::Select;
                }
                e.insert = None;
            });
            if msg == WM_LBUTTONDOWN {
                // SAFETY: plain call.
                unsafe { SetCapture(hwnd) };
            }
            redraw(hwnd);
            selchange(hwnd, ansi);
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = point(lparam);
            // SAFETY: plain call.
            let captured = unsafe { GetCapture() } == hwnd;
            let drag = with(hwnd, ansi, |e| e.drag).unwrap_or(Drag::None);
            if captured && drag != Drag::None {
                if let Drag::Thumb(grab) = drag {
                    scroll_to(
                        hwnd,
                        ansi,
                        |e, max| {
                            let c = view::client(hwnd);
                            let (_, th) = view::thumb(hwnd, e).unwrap_or((0, 0));
                            let track = (c[3] - c[1] - 4 - th).max(1);
                            (y - grab - c[1] - 2) * max / track
                        },
                        true,
                    );
                } else {
                    let moved = with(hwnd, ansi, |e| {
                        let at = pos_at(hwnd, e, x, y);
                        let moved = at != e.caret;
                        e.caret = at;
                        view::caret_into_view(hwnd, e);
                        moved
                    });
                    if moved == Some(true) {
                        update_bar(hwnd, ansi);
                        redraw(hwnd);
                        selchange(hwnd, ansi);
                    }
                }
                return 0;
            }
            link_notify(hwnd, ansi, msg, wparam, lparam);
            0
        }
        WM_LBUTTONUP => {
            if link_notify(hwnd, ansi, msg, wparam, lparam) {
                return 0;
            }
            with(hwnd, ansi, |e| e.drag = Drag::None);
            // SAFETY: plain calls.
            unsafe {
                if GetCapture() == hwnd {
                    ReleaseCapture();
                }
            }
            0
        }
        WM_RBUTTONDOWN | WM_RBUTTONUP => {
            if link_notify(hwnd, ansi, msg, wparam, lparam) {
                return 0;
            }
            // SAFETY: plain call.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_SETCURSOR => {
            let mut p = [0i32; 2];
            // SAFETY: plain calls with our POINT.
            unsafe {
                GetCursorPos(&mut p);
                ScreenToClient(hwnd, &mut p);
            }
            let on_link = with(hwnd, ansi, |e| {
                char_under(hwnd, e, p[0], p[1]).and_then(|at| e.link_at(at)).is_some()
            })
            .unwrap_or(false);
            // IDC_HAND over links, IDC_IBEAM over text, IDC_ARROW on the
            // scroll bar.
            let c = view::client(hwnd);
            let id = if on_link {
                32649
            } else if style(hwnd) & view::WS_VSCROLL != 0 && p[0] >= c[2] - view::BAR {
                32512
            } else {
                32513
            };
            // SAFETY: plain calls.
            unsafe { SetCursor(LoadCursorW(0, id)) };
            1
        }
        WM_NOTIFY => 0,
        // The edit and rich edit message ranges: not handled yet.
        0x00b0..=0x00df | 0x0400..=0x0600 => {
            unsupported(b"riched20.dll!message", u64::from(msg));
            0
        }
        // SAFETY: plain call.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
