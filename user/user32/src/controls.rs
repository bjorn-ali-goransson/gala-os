//! user32's built-in controls, STATIC, BUTTON and EDIT, registered as
//! system classes on first use (`register_system_classes`) and drawn in
//! the Windows 11 style (rounded light buttons and text boxes, an accent
//! colour for checked boxes, default buttons and the focused box's
//! underline).
//!
//! - STATIC: text (SS_LEFT/CENTER/RIGHT, word-wrapped unless
//!   SS_LEFTNOWORDWRAP or SS_SIMPLE, SS_CENTERIMAGE centres it
//!   vertically, SS_ENDELLIPSIS), an icon (SS_ICON, STM_SETICON/
//!   STM_SETIMAGE) or an etched line; the background is the parent's
//!   WM_CTLCOLORSTATIC brush.
//! - BUTTON: push buttons (BS_DEFPUSHBUTTON in the accent colour), check
//!   boxes (auto, three-state), radio buttons (auto ones uncheck their
//!   siblings), group boxes and owner-drawn buttons (WM_DRAWITEM); a
//!   click (mouse, space, BM_CLICK) sends BN_CLICKED.
//! - EDIT: one line or several, a selection (EM_SETSEL/GETSEL,
//!   EM_REPLACESEL), typing (WM_CHAR, the arrow keys, Home/End, Delete,
//!   Backspace, Ctrl+A), EN_CHANGE/EN_UPDATE/EN_MAXTEXT/EN_SETFOCUS/
//!   EN_KILLFOCUS, read-only, a length limit, password characters, a cue
//!   banner (EM_SETCUEBANNER). The clipboard and undo are not there yet.
//!
//! Each control's state lives in a table here (512 controls); an edit's
//! text in a 64 KiB block of its own (32767 characters at most).

use crate::window::{DefWindowProcW, Handle, Hwnd, SendMessageW};
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn VirtualAlloc(at: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(at: *mut c_void, size: usize, kind: u32) -> i32;
}

// Messages.
const WM_CREATE: u32 = 0x0001;
const WM_DESTROY: u32 = 0x0002;
const WM_SETFOCUS: u32 = 0x0007;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_ENABLE: u32 = 0x000a;
const WM_SETTEXT: u32 = 0x000c;
const WM_GETTEXT: u32 = 0x000d;
const WM_GETTEXTLENGTH: u32 = 0x000e;
const WM_PAINT: u32 = 0x000f;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_SETFONT: u32 = 0x0030;
const WM_GETFONT: u32 = 0x0031;
const WM_DRAWITEM: u32 = 0x002b;
const WM_GETDLGCODE: u32 = 0x0087;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_CHAR: u32 = 0x0102;
const WM_COMMAND: u32 = 0x0111;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_CTLCOLOREDIT: u32 = 0x0133;
const WM_CTLCOLORBTN: u32 = 0x0135;
const WM_CTLCOLORSTATIC: u32 = 0x0138;

// STATIC.
const STM_SETICON: u32 = 0x0170;
const STM_GETICON: u32 = 0x0171;
const STM_SETIMAGE: u32 = 0x0172;
const STM_GETIMAGE: u32 = 0x0173;
const SS_CENTER: u32 = 0x1;
const SS_RIGHT: u32 = 0x2;
const SS_ICON: u32 = 0x3;
const SS_SIMPLE: u32 = 0xb;
const SS_LEFTNOWORDWRAP: u32 = 0xc;
const SS_ETCHEDHORZ: u32 = 0x10;
const SS_TYPEMASK: u32 = 0x1f;
const SS_NOPREFIX: u32 = 0x80;
const SS_CENTERIMAGE: u32 = 0x200;
const SS_ENDELLIPSIS: u32 = 0x4000;

// BUTTON.
const BM_GETCHECK: u32 = 0x00f0;
const BM_SETCHECK: u32 = 0x00f1;
const BM_GETSTATE: u32 = 0x00f2;
const BM_SETSTATE: u32 = 0x00f3;
const BM_SETSTYLE: u32 = 0x00f4;
const BM_CLICK: u32 = 0x00f5;
const BM_GETIMAGE: u32 = 0x00f6;
const BM_SETIMAGE: u32 = 0x00f7;
const BS_DEFPUSHBUTTON: u32 = 0x1;
const BS_CHECKBOX: u32 = 0x2;
const BS_AUTOCHECKBOX: u32 = 0x3;
const BS_RADIOBUTTON: u32 = 0x4;
const BS_3STATE: u32 = 0x5;
const BS_AUTO3STATE: u32 = 0x6;
const BS_GROUPBOX: u32 = 0x7;
const BS_AUTORADIOBUTTON: u32 = 0x9;
const BS_OWNERDRAW: u32 = 0xb;
const BS_TYPEMASK: u32 = 0xf;
const BST_PUSHED: u32 = 0x4;
const BST_FOCUS: u32 = 0x8;
const BN_CLICKED: u32 = 0;

// EDIT.
const EM_GETSEL: u32 = 0x00b0;
const EM_SETSEL: u32 = 0x00b1;
const EM_GETRECT: u32 = 0x00b2;
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
const EM_SETPASSWORDCHAR: u32 = 0x00cc;
const EM_EMPTYUNDOBUFFER: u32 = 0x00cd;
const EM_GETFIRSTVISIBLELINE: u32 = 0x00ce;
const EM_SETREADONLY: u32 = 0x00cf;
const EM_GETPASSWORDCHAR: u32 = 0x00d2;
const EM_SETMARGINS: u32 = 0x00d3;
const EM_GETLIMITTEXT: u32 = 0x00d5;
const EM_SETCUEBANNER: u32 = 0x1501;
const EM_GETCUEBANNER: u32 = 0x1502;
const ES_CENTER: u32 = 0x1;
const ES_RIGHT: u32 = 0x2;
const ES_MULTILINE: u32 = 0x4;
const ES_UPPERCASE: u32 = 0x8;
const ES_LOWERCASE: u32 = 0x10;
const ES_PASSWORD: u32 = 0x20;
const ES_NOHIDESEL: u32 = 0x100;
const ES_READONLY: u32 = 0x800;
const ES_NUMBER: u32 = 0x2000;
const EN_SETFOCUS: u32 = 0x0100;
const EN_KILLFOCUS: u32 = 0x0200;
const EN_CHANGE: u32 = 0x0300;
const EN_UPDATE: u32 = 0x0400;
const EN_MAXTEXT: u32 = 0x0501;

const WS_BORDER: u32 = 0x0080_0000;
const WS_DISABLED: u32 = 0x0800_0000;
const WS_EX_CLIENTEDGE: u32 = 0x200;
const GWL_STYLE: i32 = -16;
const GWL_EXSTYLE: i32 = -20;
const GWLP_ID: i32 = -12;

/// The longest edit text (characters; the block holds one more).
const EDIT_CAP: usize = 32767;
/// Windows' default limit for a new edit control.
const EDIT_DEFAULT_LIMIT: usize = 30000;

/// Colours (COLORREF: 0x00BBGGRR).
const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | g << 8 | b << 16
}
const ACCENT: u32 = rgb(0x00, 0x5f, 0xb8);
const ACCENT_HOT: u32 = rgb(0x19, 0x6e, 0xbf);
const ACCENT_PRESSED: u32 = rgb(0x31, 0x83, 0xca);
const TEXT: u32 = rgb(0x1b, 0x1b, 0x1b);
const TEXT_DISABLED: u32 = rgb(0xa0, 0xa0, 0xa0);
const BUTTON_FACE: u32 = rgb(0xfb, 0xfb, 0xfb);
const BUTTON_HOT: u32 = rgb(0xf6, 0xf6, 0xf6);
const BUTTON_PRESSED: u32 = rgb(0xf0, 0xf0, 0xf0);
const BUTTON_BORDER: u32 = rgb(0xd0, 0xd0, 0xd0);
const BOX_BORDER: u32 = rgb(0x8a, 0x8a, 0x8a);
const EDIT_BORDER: u32 = rgb(0xe0, 0xe0, 0xe0);
const EDIT_UNDERLINE: u32 = rgb(0x86, 0x86, 0x86);
const SELECTION: u32 = rgb(0x00, 0x78, 0xd4);
const WHITE: u32 = rgb(0xff, 0xff, 0xff);
const CUE: u32 = rgb(0x8a, 0x8a, 0x8a);

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Static,
    Button,
    Edit,
}

/// One control's state.
#[derive(Clone, Copy)]
struct Ctl {
    hwnd: Hwnd,
    kind: Kind,
    font: usize,
    /// STATIC/BUTTON: an icon or bitmap and its type (IMAGE_BITMAP 0,
    /// IMAGE_ICON 1).
    image: (usize, u32),
    /// BUTTON: the check state; whether pressed (mouse or space held),
    /// and whether the pointer is over it.
    check: u32,
    pressed: bool,
    over: bool,
    /// EDIT: the text block, its length, the selection (anchor, caret),
    /// the limit, the password character, whether modified, the first
    /// pixel shown (horizontal scroll), and the cue banner.
    text: *mut u16,
    len: usize,
    anchor: usize,
    caret: usize,
    limit: usize,
    password: u16,
    modified: bool,
    scroll: i32,
    cue: [u16; 48],
    margins: (i32, i32),
}

const NONE: Ctl = Ctl {
    hwnd: 0,
    kind: Kind::Static,
    font: 0,
    image: (0, 0),
    check: 0,
    pressed: false,
    over: false,
    text: core::ptr::null_mut(),
    len: 0,
    anchor: 0,
    caret: 0,
    limit: EDIT_DEFAULT_LIMIT,
    password: 0,
    modified: false,
    scroll: 0,
    cue: [0; 48],
    margins: (0, 0),
};

const MAX_CONTROLS: usize = 512;

struct Table {
    lock: AtomicBool,
    ctls: UnsafeCell<[Ctl; MAX_CONTROLS]>,
}

// SAFETY: `ctls` is only touched under `lock`.
unsafe impl Sync for Table {}

static TABLE: Table =
    Table { lock: AtomicBool::new(false), ctls: UnsafeCell::new([NONE; MAX_CONTROLS]) };

fn with_table<R>(f: impl FnOnce(&mut [Ctl; MAX_CONTROLS]) -> R) -> R {
    while TABLE.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        core::hint::spin_loop();
    }
    // SAFETY: under the lock.
    let r = f(unsafe { &mut *TABLE.ctls.get() });
    TABLE.lock.store(false, Ordering::Release);
    r
}

/// The control's state (made on first use; an edit gets its text block
/// and the window's text).
fn get(hwnd: Hwnd, kind: Kind) -> Ctl {
    if let Some(c) = with_table(|t| t.iter().find(|c| c.hwnd == hwnd).copied()) {
        return c;
    }
    let mut c = Ctl { hwnd, kind, ..NONE };
    if kind == Kind::Edit {
        // SAFETY: plain call; MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE.
        c.text =
            unsafe { VirtualAlloc(core::ptr::null_mut(), (EDIT_CAP + 1) * 2, 0x3000, 4) }.cast();
        if !c.text.is_null() {
            // The text the window was created with (kept by DefWindowProc).
            let n = DefWindowProcW(hwnd, WM_GETTEXT, EDIT_CAP + 1, c.text as isize);
            c.len = (n.max(0) as usize).min(EDIT_CAP);
        }
        if style(hwnd) & ES_PASSWORD != 0 {
            c.password = u16::from(b'*');
        }
    }
    with_table(|t| {
        if let Some(slot) = t.iter_mut().find(|s| s.hwnd == 0) {
            *slot = c;
        }
    });
    c
}

fn put(c: &Ctl) {
    with_table(|t| {
        if let Some(slot) = t.iter_mut().find(|s| s.hwnd == c.hwnd) {
            *slot = *c;
        }
    });
}

fn forget(hwnd: Hwnd) {
    let gone =
        with_table(|t| t.iter_mut().find(|s| s.hwnd == hwnd).map(|s| core::mem::replace(s, NONE)));
    if let Some(c) = gone
        && !c.text.is_null()
    {
        // SAFETY: the block `get` allocated; MEM_RELEASE.
        unsafe { VirtualFree(c.text.cast(), 0, 0x8000) };
    }
}

fn text_of(c: &Ctl) -> &'static [u16] {
    if c.text.is_null() {
        return &[];
    }
    // SAFETY: the control's block holds `len` characters.
    unsafe { core::slice::from_raw_parts(c.text, c.len) }
}

fn style(hwnd: Hwnd) -> u32 {
    crate::tree::GetWindowLongW(hwnd, GWL_STYLE) as u32
}

fn notify(hwnd: Hwnd, code: u32) {
    let parent = crate::tree::GetParent(hwnd);
    if parent == 0 {
        return;
    }
    let id = crate::tree::GetWindowLongPtrW(hwnd, GWLP_ID) as usize & 0xffff;
    SendMessageW(parent, WM_COMMAND, id | (code as usize) << 16, hwnd as isize);
}

fn redraw(hwnd: Hwnd) {
    crate::window::InvalidateRect(hwnd, core::ptr::null(), 1);
}

// ---- Drawing ----

macro_rules! gdi {
    ($name:literal, $ty:ty) => {{
        static CACHE: AtomicUsize = AtomicUsize::new(0);
        let f = crate::icon::gdi_proc(&CACHE, concat!($name, "\0").as_bytes());
        // SAFETY: gdi32's function of that name has this signature.
        (f != 0).then(|| unsafe { core::mem::transmute::<usize, $ty>(f) })
    }};
}

fn select(hdc: Handle, object: usize) -> usize {
    gdi!("SelectObject", extern "system" fn(Handle, usize) -> usize).map_or(0, |f| f(hdc, object))
}

fn text_color(hdc: Handle, color: u32) {
    if let Some(f) = gdi!("SetTextColor", extern "system" fn(Handle, u32) -> u32) {
        f(hdc, color);
    }
}

fn transparent(hdc: Handle) {
    if let Some(f) = gdi!("SetBkMode", extern "system" fn(Handle, i32) -> i32) {
        f(hdc, 1);
    }
}

fn default_font() -> usize {
    // DEFAULT_GUI_FONT (Segoe UI, here Selawik).
    gdi!("GetStockObject", extern "system" fn(i32) -> usize).map_or(0, |f| f(17))
}

fn fill(hdc: Handle, l: i32, t: i32, r: i32, b: i32, color: u32) {
    if r <= l || b <= t {
        return;
    }
    let (Some(brush), Some(delete)) = (
        gdi!("CreateSolidBrush", extern "system" fn(u32) -> Handle),
        gdi!("DeleteObject", extern "system" fn(Handle) -> i32),
    ) else {
        return;
    };
    let b0 = brush(color);
    crate::window::FillRect(hdc, &[l, t, r, b], b0);
    delete(b0);
}

fn isqrt(v: i32) -> i32 {
    let mut x = 0;
    while (x + 1) * (x + 1) <= v {
        x += 1;
    }
    x
}

/// A rectangle with corners of `radius`, in `color`.
fn round_fill(hdc: Handle, r: [i32; 4], radius: i32, color: u32) {
    let [l, t, rr, b] = r;
    let radius = radius.min((rr - l) / 2).min((b - t) / 2).max(0);
    for y in t..b {
        // The row's distance into the corner zone.
        let k = if y < t + radius {
            t + radius - y
        } else if y >= b - radius {
            y - (b - radius) + 1
        } else {
            0
        };
        let inset = if k == 0 { 0 } else { radius - isqrt(radius * radius - (k - 1) * (k - 1)) };
        fill(hdc, l + inset, y, rr - inset, y + 1, color);
    }
}

/// A rounded box: `border` around `face`.
fn round_box(hdc: Handle, r: [i32; 4], radius: i32, face: u32, border: u32) {
    round_fill(hdc, r, radius, border);
    round_fill(hdc, [r[0] + 1, r[1] + 1, r[2] - 1, r[3] - 1], (radius - 1).max(0), face);
}

/// The brush the parent gives for `msg` (WM_CTLCOLOR*), which also sets
/// the DC's colours; the system colour's if it gives none.
fn parent_brush(hwnd: Hwnd, hdc: Handle, msg: u32, fallback: i32) -> Handle {
    let parent = crate::tree::GetParent(hwnd);
    let b = if parent == 0 { 0 } else { SendMessageW(parent, msg, hdc as usize, hwnd as isize) };
    if b != 0 { b as Handle } else { crate::window::GetSysColorBrush(fallback) }
}

/// Paints the control: BeginPaint, its font selected, `f(hdc, client)`.
fn paint(hwnd: Hwnd, font: usize, f: impl FnOnce(Handle, [i32; 4])) {
    let mut ps = crate::window::PaintStruct::EMPTY;
    let hdc = crate::window::BeginPaint(hwnd, &mut ps);
    if hdc.is_null() {
        return;
    }
    let mut client = [0i32; 4];
    crate::tree::GetClientRect(hwnd, &mut client);
    let font = if font != 0 { font } else { default_font() };
    let old = select(hdc, font);
    f(hdc, client);
    if old != 0 {
        select(hdc, old);
    }
    crate::window::EndPaint(hwnd, &ps);
}

/// The window's text (up to 255 characters).
fn window_text(hwnd: Hwnd, buf: &mut [u16; 256]) -> usize {
    DefWindowProcW(hwnd, WM_GETTEXT, buf.len(), buf.as_mut_ptr() as isize).max(0) as usize
}

fn draw_text(hdc: Handle, text: &[u16], r: [i32; 4], format: u32) {
    let mut r = r;
    crate::text::DrawTextW(hdc, text.as_ptr(), text.len() as i32, &mut r, format);
}

fn text_width(hdc: Handle, s: &[u16]) -> i32 {
    if s.is_empty() {
        return 0;
    }
    let mut size = [0i32; 2];
    if let Some(f) = gdi!(
        "GetTextExtentPoint32W",
        extern "system" fn(Handle, *const u16, i32, *mut [i32; 2]) -> i32
    ) {
        f(hdc, s.as_ptr(), s.len() as i32, &mut size);
    }
    size[0]
}

fn text_height(hdc: Handle) -> i32 {
    let mut tm = [0i32; 15];
    if let Some(f) = gdi!("GetTextMetricsW", extern "system" fn(Handle, *mut [i32; 15]) -> i32) {
        f(hdc, &mut tm);
    }
    tm[0].max(1)
}

fn text_out(hdc: Handle, x: i32, y: i32, s: &[u16]) {
    if let Some(f) = gdi!("TextOutW", extern "system" fn(Handle, i32, i32, *const u16, i32) -> i32)
    {
        f(hdc, x, y, s.as_ptr(), s.len() as i32);
    }
}

// ---- Registration ----

static REGISTERED: AtomicBool = AtomicBool::new(false);

/// Registers STATIC, BUTTON and EDIT (once).
pub fn register_system_classes() {
    if REGISTERED.swap(true, Ordering::AcqRel) {
        return;
    }
    let classes: [(&[u8], crate::window::WndProc); 3] =
        [(b"STATIC\0", static_proc), (b"BUTTON\0", button_proc), (b"EDIT\0", edit_proc)];
    for (name, proc_) in classes {
        let mut wide = [0u16; 8];
        for (w, &b) in wide.iter_mut().zip(name) {
            *w = u16::from(b);
        }
        let wc = crate::window::WndClassExW::system(wide.as_ptr(), proc_);
        crate::window::RegisterClassExW(&wc);
    }
}

/// The messages every control handles alike: its font and its end.
fn common(hwnd: Hwnd, kind: Kind, msg: u32, wparam: usize, lparam: isize) -> Option<isize> {
    match msg {
        WM_SETFONT => {
            let mut c = get(hwnd, kind);
            c.font = wparam;
            put(&c);
            if lparam != 0 {
                redraw(hwnd);
            }
            Some(0)
        }
        WM_GETFONT => Some(get(hwnd, kind).font as isize),
        WM_DESTROY => {
            forget(hwnd);
            Some(DefWindowProcW(hwnd, msg, wparam, lparam))
        }
        WM_ERASEBKGND => Some(1),
        WM_ENABLE => {
            redraw(hwnd);
            Some(0)
        }
        _ => None,
    }
}

// ---- STATIC ----

extern "system" fn static_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    if let Some(r) = common(hwnd, Kind::Static, msg, wparam, lparam) {
        return r;
    }
    match msg {
        WM_CREATE => {
            get(hwnd, Kind::Static);
            0
        }
        WM_SETTEXT => {
            let r = DefWindowProcW(hwnd, msg, wparam, lparam);
            redraw(hwnd);
            r
        }
        STM_SETICON | STM_SETIMAGE => {
            let mut c = get(hwnd, Kind::Static);
            let kind = if msg == STM_SETICON { 1 } else { wparam as u32 };
            let image = if msg == STM_SETICON { wparam } else { lparam as usize };
            let old = core::mem::replace(&mut c.image, (image, kind));
            put(&c);
            redraw(hwnd);
            old.0 as isize
        }
        STM_GETICON | STM_GETIMAGE => get(hwnd, Kind::Static).image.0 as isize,
        WM_PAINT => {
            let c = get(hwnd, Kind::Static);
            let s = style(hwnd);
            paint(hwnd, c.font, |hdc, r| {
                let brush = parent_brush(hwnd, hdc, WM_CTLCOLORSTATIC, 15);
                crate::window::FillRect(hdc, &r, brush);
                let kind = s & SS_TYPEMASK;
                if kind == SS_ETCHEDHORZ {
                    fill(hdc, r[0], r[1], r[2], r[1] + 1, EDIT_BORDER);
                    return;
                }
                if kind == SS_ICON || c.image.0 != 0 {
                    if c.image.0 != 0 && c.image.1 == 1 {
                        crate::icon::DrawIconEx(
                            hdc,
                            r[0],
                            r[1],
                            c.image.0,
                            0,
                            0,
                            0,
                            core::ptr::null_mut(),
                            3,
                        );
                    }
                    return;
                }
                if s & WS_DISABLED != 0 {
                    text_color(hdc, TEXT_DISABLED);
                }
                transparent(hdc);
                let mut buf = [0u16; 256];
                let n = window_text(hwnd, &mut buf);
                let mut format = match kind {
                    SS_CENTER => 0x1,
                    SS_RIGHT => 0x2,
                    _ => 0,
                };
                if kind != SS_LEFTNOWORDWRAP && kind != SS_SIMPLE {
                    format |= 0x10; // DT_WORDBREAK
                }
                if s & SS_NOPREFIX != 0 {
                    format |= 0x800;
                }
                if s & SS_CENTERIMAGE != 0 {
                    format = (format & !0x10) | 0x24; // DT_SINGLELINE | DT_VCENTER
                }
                if s & SS_ENDELLIPSIS != 0 {
                    format = (format & !0x10) | 0x8000;
                }
                draw_text(hdc, &buf[..n], r, format);
            });
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ---- BUTTON ----

fn button_type(hwnd: Hwnd) -> u32 {
    style(hwnd) & BS_TYPEMASK
}

/// A click: auto check boxes and radio buttons change, then BN_CLICKED.
fn click(hwnd: Hwnd) {
    let kind = button_type(hwnd);
    let mut c = get(hwnd, Kind::Button);
    match kind {
        BS_AUTOCHECKBOX => c.check = u32::from(c.check == 0),
        BS_AUTO3STATE => c.check = (c.check + 1) % 3,
        BS_AUTORADIOBUTTON => {
            c.check = 1;
            // The other auto radio buttons of the same parent go off.
            let parent = crate::tree::GetParent(hwnd);
            let mut sibling = crate::tree::GetWindow(hwnd, 0); // GW_HWNDFIRST
            while sibling != 0 {
                if sibling != hwnd
                    && crate::tree::GetParent(sibling) == parent
                    && with_table(|t| t.iter().any(|s| s.hwnd == sibling && s.kind == Kind::Button))
                    && button_type(sibling) == BS_AUTORADIOBUTTON
                {
                    let mut o = get(sibling, Kind::Button);
                    if o.check != 0 {
                        o.check = 0;
                        put(&o);
                        redraw(sibling);
                    }
                }
                sibling = crate::tree::GetWindow(sibling, 2); // GW_HWNDNEXT
            }
        }
        _ => {}
    }
    put(&c);
    redraw(hwnd);
    notify(hwnd, BN_CLICKED);
}

fn inside(hwnd: Hwnd, lparam: isize) -> bool {
    let (x, y) = (lparam as i16 as i32, (lparam >> 16) as i16 as i32);
    let mut r = [0i32; 4];
    crate::tree::GetClientRect(hwnd, &mut r);
    x >= r[0] && y >= r[1] && x < r[2] && y < r[3]
}

extern "system" fn button_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    if let Some(r) = common(hwnd, Kind::Button, msg, wparam, lparam) {
        return r;
    }
    match msg {
        WM_CREATE => {
            get(hwnd, Kind::Button);
            0
        }
        WM_SETTEXT => {
            let r = DefWindowProcW(hwnd, msg, wparam, lparam);
            redraw(hwnd);
            r
        }
        BM_GETCHECK => get(hwnd, Kind::Button).check as isize,
        BM_SETCHECK => {
            let mut c = get(hwnd, Kind::Button);
            c.check = (wparam as u32).min(2);
            put(&c);
            redraw(hwnd);
            0
        }
        BM_GETSTATE => {
            let c = get(hwnd, Kind::Button);
            let focus = if crate::tree::GetFocus() == hwnd { BST_FOCUS } else { 0 };
            (c.check | if c.pressed { BST_PUSHED } else { 0 } | focus) as isize
        }
        BM_SETSTATE => {
            let mut c = get(hwnd, Kind::Button);
            c.pressed = wparam != 0;
            put(&c);
            redraw(hwnd);
            0
        }
        BM_SETSTYLE => {
            let s = (style(hwnd) & !0xffff) | (wparam as u32 & 0xffff);
            crate::tree::SetWindowLongW(hwnd, GWL_STYLE, s as i32);
            if lparam != 0 {
                redraw(hwnd);
            }
            0
        }
        BM_SETIMAGE => {
            let mut c = get(hwnd, Kind::Button);
            let old = core::mem::replace(&mut c.image, (lparam as usize, wparam as u32));
            put(&c);
            redraw(hwnd);
            old.0 as isize
        }
        BM_GETIMAGE => get(hwnd, Kind::Button).image.0 as isize,
        BM_CLICK => {
            click(hwnd);
            0
        }
        WM_GETDLGCODE => {
            let kind = button_type(hwnd);
            let code = match kind {
                BS_DEFPUSHBUTTON => 0x10,                    // DLGC_DEFPUSHBUTTON
                BS_RADIOBUTTON | BS_AUTORADIOBUTTON => 0x40, // DLGC_RADIOBUTTON
                BS_GROUPBOX => 0x100,                        // DLGC_STATIC
                0 => 0x20,                                   // DLGC_UNDEFPUSHBUTTON
                _ => 0,
            };
            code | 0x2000 // DLGC_BUTTON
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            if button_type(hwnd) == BS_GROUPBOX {
                return 0;
            }
            let mut c = get(hwnd, Kind::Button);
            c.pressed = true;
            c.over = true;
            put(&c);
            crate::tree::SetFocus(hwnd);
            crate::window::SetCapture(hwnd);
            redraw(hwnd);
            0
        }
        WM_MOUSEMOVE => {
            let mut c = get(hwnd, Kind::Button);
            let over = inside(hwnd, lparam);
            if over != c.over {
                c.over = over;
                put(&c);
                redraw(hwnd);
            }
            0
        }
        WM_LBUTTONUP => {
            let mut c = get(hwnd, Kind::Button);
            let was = c.pressed;
            c.pressed = false;
            put(&c);
            if crate::window::GetCapture() == hwnd {
                crate::window::ReleaseCapture();
            }
            redraw(hwnd);
            if was && inside(hwnd, lparam) {
                click(hwnd);
            }
            0
        }
        WM_KEYDOWN if wparam == 0x20 => {
            let mut c = get(hwnd, Kind::Button);
            c.pressed = true;
            put(&c);
            redraw(hwnd);
            0
        }
        WM_KEYUP if wparam == 0x20 => {
            let mut c = get(hwnd, Kind::Button);
            let was = core::mem::replace(&mut c.pressed, false);
            put(&c);
            if was {
                click(hwnd);
            }
            0
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            redraw(hwnd);
            0
        }
        WM_PAINT => {
            paint_button(hwnd);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// DRAWITEMSTRUCT.
#[repr(C)]
struct DrawItem {
    kind: u32,
    id: u32,
    item: u32,
    action: u32,
    state: u32,
    hwnd: Hwnd,
    hdc: Handle,
    rect: [i32; 4],
    data: usize,
}

fn paint_button(hwnd: Hwnd) {
    let c = get(hwnd, Kind::Button);
    let s = style(hwnd);
    let kind = s & BS_TYPEMASK;
    let disabled = s & WS_DISABLED != 0;
    paint(hwnd, c.font, |hdc, r| {
        if kind == BS_OWNERDRAW {
            let parent = crate::tree::GetParent(hwnd);
            let id = crate::tree::GetWindowLongPtrW(hwnd, GWLP_ID) as u32;
            let mut state = 0;
            if c.pressed {
                state |= 1; // ODS_SELECTED
            }
            if disabled {
                state |= 4; // ODS_DISABLED
            }
            if crate::tree::GetFocus() == hwnd {
                state |= 0x10; // ODS_FOCUS
            }
            let di =
                DrawItem { kind: 4, id, item: 0, action: 1, state, hwnd, hdc, rect: r, data: 0 };
            SendMessageW(parent, WM_DRAWITEM, id as usize, &di as *const DrawItem as isize);
            return;
        }
        let brush = parent_brush(
            hwnd,
            hdc,
            if kind <= BS_DEFPUSHBUTTON { WM_CTLCOLORBTN } else { WM_CTLCOLORSTATIC },
            15,
        );
        crate::window::FillRect(hdc, &r, brush);
        transparent(hdc);
        let mut buf = [0u16; 256];
        let n = window_text(hwnd, &mut buf);
        let label = &buf[..n];
        let pressed = c.pressed && c.over;
        match kind {
            0 | BS_DEFPUSHBUTTON => {
                let default = kind == BS_DEFPUSHBUTTON && !disabled;
                let (face, border, ink) = if disabled {
                    (BUTTON_PRESSED, BUTTON_BORDER, TEXT_DISABLED)
                } else if default {
                    let face = if pressed {
                        ACCENT_PRESSED
                    } else if c.over {
                        ACCENT_HOT
                    } else {
                        ACCENT
                    };
                    (face, face, WHITE)
                } else {
                    let face = if pressed {
                        BUTTON_PRESSED
                    } else if c.over {
                        BUTTON_HOT
                    } else {
                        BUTTON_FACE
                    };
                    (face, BUTTON_BORDER, TEXT)
                };
                round_box(hdc, r, 4, face, border);
                text_color(hdc, ink);
                draw_text(hdc, label, r, 0x25); // DT_CENTER | DT_VCENTER | DT_SINGLELINE
            }
            BS_CHECKBOX | BS_AUTOCHECKBOX | BS_3STATE | BS_AUTO3STATE | BS_RADIOBUTTON
            | BS_AUTORADIOBUTTON => {
                let radio = kind == BS_RADIOBUTTON || kind == BS_AUTORADIOBUTTON;
                let size = 14;
                let top = r[1] + (r[3] - r[1] - size) / 2;
                let b = [r[0], top, r[0] + size, top + size];
                let radius = if radio { size / 2 } else { 3 };
                if c.check != 0 {
                    let face = if disabled {
                        TEXT_DISABLED
                    } else if pressed {
                        ACCENT_PRESSED
                    } else {
                        ACCENT
                    };
                    round_fill(hdc, b, radius, face);
                    if radio {
                        round_fill(hdc, [b[0] + 4, b[1] + 4, b[2] - 4, b[3] - 4], 3, WHITE);
                    } else if c.check == 2 {
                        fill(hdc, b[0] + 3, b[1] + 6, b[2] - 3, b[1] + 8, WHITE);
                    } else {
                        // A check mark: a short stroke down, a long one up.
                        for i in 0..3 {
                            fill(
                                hdc,
                                b[0] + 3 + i,
                                b[1] + 6 + i,
                                b[0] + 5 + i,
                                b[1] + 8 + i,
                                WHITE,
                            );
                        }
                        for i in 0..6 {
                            fill(
                                hdc,
                                b[0] + 5 + i,
                                b[1] + 8 - i,
                                b[0] + 7 + i,
                                b[1] + 10 - i,
                                WHITE,
                            );
                        }
                    }
                } else {
                    let face = if pressed {
                        BUTTON_PRESSED
                    } else if c.over {
                        BUTTON_HOT
                    } else {
                        WHITE
                    };
                    round_box(
                        hdc,
                        b,
                        radius,
                        face,
                        if disabled { TEXT_DISABLED } else { BOX_BORDER },
                    );
                }
                text_color(hdc, if disabled { TEXT_DISABLED } else { TEXT });
                draw_text(hdc, label, [r[0] + size + 8, r[1], r[2], r[3]], 0x24); // DT_VCENTER | DT_SINGLELINE
            }
            BS_GROUPBOX => {
                let h = text_height(hdc);
                round_fill(hdc, [r[0], r[1] + h / 2, r[2], r[3]], 6, EDIT_BORDER);
                crate::window::FillRect(
                    hdc,
                    &[r[0] + 1, r[1] + h / 2 + 1, r[2] - 1, r[3] - 1],
                    brush,
                );
                let w = text_width(hdc, label);
                if w > 0 {
                    crate::window::FillRect(hdc, &[r[0] + 6, r[1], r[0] + 14 + w, r[1] + h], brush);
                    text_color(hdc, if disabled { TEXT_DISABLED } else { TEXT });
                    draw_text(hdc, label, [r[0] + 10, r[1], r[2], r[1] + h], 0x20);
                }
            }
            _ => {
                text_color(hdc, TEXT);
                draw_text(hdc, label, r, 0x25);
            }
        }
    });
}

// ---- EDIT ----

fn edit(hwnd: Hwnd) -> Ctl {
    get(hwnd, Kind::Edit)
}

fn selection(c: &Ctl) -> (usize, usize) {
    (c.anchor.min(c.caret), c.anchor.max(c.caret))
}

/// Replaces the selection with `with` (cut to the limit); whether the
/// text changed, and whether the limit cut it.
fn replace(c: &mut Ctl, with: &[u16]) -> (bool, bool) {
    if c.text.is_null() {
        return (false, false);
    }
    let (a, b) = selection(c);
    let room = c.limit.min(EDIT_CAP).saturating_sub(c.len - (b - a));
    let n = with.len().min(room);
    if n == 0 && a == b {
        return (false, n < with.len());
    }
    // SAFETY: the block holds EDIT_CAP + 1 characters; len - (b - a) + n
    // stays within the limit.
    unsafe {
        core::ptr::copy(c.text.add(b), c.text.add(a + n), c.len - b);
        core::ptr::copy_nonoverlapping(with.as_ptr(), c.text.add(a), n);
    }
    c.len = c.len - (b - a) + n;
    c.anchor = a + n;
    c.caret = a + n;
    c.modified = true;
    (true, n < with.len())
}

fn changed(hwnd: Hwnd) {
    redraw(hwnd);
    notify(hwnd, EN_UPDATE);
    notify(hwnd, EN_CHANGE);
}

/// The character position nearest x (client pixels) on a one-line edit.
fn char_at(hwnd: Hwnd, c: &Ctl, x: i32) -> usize {
    let hdc = crate::window::GetDC(hwnd);
    let font = if c.font != 0 { c.font } else { default_font() };
    let old = select(hdc, font);
    let shown = shown_text(c);
    let left = text_left(hwnd) - c.scroll;
    let mut best = 0;
    for i in 0..=shown.len().min(c.len) {
        let w = text_width(hdc, &shown[..i]);
        if left + w <= x {
            best = i;
        }
        // Past the middle of the next character: that side.
        if i < shown.len() {
            let next = text_width(hdc, &shown[..i + 1]);
            if x >= left + w && x < left + (w + next) / 2 {
                best = i;
                break;
            }
            if x >= left + (w + next) / 2 && x < left + next {
                best = i + 1;
                break;
            }
        }
    }
    select(hdc, old);
    crate::window::ReleaseDC(hwnd, hdc);
    best
}

/// What the edit shows: its text, or as many password characters.
fn shown_text(c: &Ctl) -> &'static [u16] {
    static STARS: [u16; 256] = [b'*' as u16; 256];
    if c.password != 0 { &STARS[..c.len.min(STARS.len())] } else { text_of(c) }
}

/// The text's left edge inside the box.
fn text_left(hwnd: Hwnd) -> i32 {
    let boxed = crate::tree::GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_CLIENTEDGE != 0
        || style(hwnd) & WS_BORDER != 0;
    if boxed { 6 } else { 2 }
}

extern "system" fn edit_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    if let Some(r) = common(hwnd, Kind::Edit, msg, wparam, lparam) {
        return r;
    }
    let s = style(hwnd);
    match msg {
        WM_CREATE => {
            edit(hwnd);
            0
        }
        WM_SETTEXT => {
            let mut c = edit(hwnd);
            if c.text.is_null() {
                return 0;
            }
            let mut n = 0;
            if lparam != 0 {
                // SAFETY: the caller's NUL-terminated text.
                while n < EDIT_CAP && unsafe { *(lparam as *const u16).add(n) } != 0 {
                    n += 1;
                }
                // SAFETY: `n` characters into the block.
                unsafe { core::ptr::copy_nonoverlapping(lparam as *const u16, c.text, n) };
            }
            c.len = n;
            c.anchor = 0;
            c.caret = 0;
            c.scroll = 0;
            c.modified = false;
            put(&c);
            redraw(hwnd);
            if s & ES_MULTILINE == 0 {
                notify(hwnd, EN_UPDATE);
                notify(hwnd, EN_CHANGE);
            }
            1
        }
        WM_GETTEXT => {
            let c = edit(hwnd);
            if wparam == 0 || lparam == 0 {
                return 0;
            }
            let n = c.len.min(wparam - 1);
            // SAFETY: the caller's buffer of `wparam` characters.
            unsafe {
                core::ptr::copy_nonoverlapping(c.text, lparam as *mut u16, n);
                *(lparam as *mut u16).add(n) = 0;
            }
            n as isize
        }
        WM_GETTEXTLENGTH => edit(hwnd).len as isize,
        EM_GETSEL => {
            let c = edit(hwnd);
            let (a, b) = selection(&c);
            // SAFETY: the caller's DWORDs, when given.
            unsafe {
                if wparam != 0 {
                    *(wparam as *mut u32) = a as u32;
                }
                if lparam != 0 {
                    *(lparam as *mut u32) = b as u32;
                }
            }
            if b > 0xffff { -1 } else { (a | b << 16) as isize }
        }
        EM_SETSEL => {
            let mut c = edit(hwnd);
            let (start, end) = (wparam as isize as i64, lparam as i64);
            if start == -1 {
                c.anchor = c.caret;
            } else {
                let clamp = |v: i64| if v < 0 { c.len } else { (v as usize).min(c.len) };
                c.anchor = clamp(start);
                c.caret = clamp(end);
            }
            put(&c);
            redraw(hwnd);
            0
        }
        EM_REPLACESEL => {
            let mut c = edit(hwnd);
            let mut n = 0;
            if lparam != 0 {
                // SAFETY: the caller's NUL-terminated text.
                while n < EDIT_CAP && unsafe { *(lparam as *const u16).add(n) } != 0 {
                    n += 1;
                }
            }
            let with: &[u16] = if n == 0 {
                &[]
            } else {
                // SAFETY: `n` characters.
                unsafe { core::slice::from_raw_parts(lparam as *const u16, n) }
            };
            let (done, cut) = replace(&mut c, with);
            put(&c);
            if done {
                changed(hwnd);
            }
            if cut {
                notify(hwnd, EN_MAXTEXT);
            }
            0
        }
        EM_LIMITTEXT => {
            let mut c = edit(hwnd);
            c.limit = if wparam == 0 { EDIT_CAP } else { wparam.min(EDIT_CAP) };
            put(&c);
            0
        }
        EM_GETLIMITTEXT => edit(hwnd).limit as isize,
        EM_SETREADONLY => {
            let s = if wparam != 0 { s | ES_READONLY } else { s & !ES_READONLY };
            crate::tree::SetWindowLongW(hwnd, GWL_STYLE, s as i32);
            redraw(hwnd);
            1
        }
        EM_SETPASSWORDCHAR => {
            let mut c = edit(hwnd);
            c.password = wparam as u16;
            put(&c);
            redraw(hwnd);
            0
        }
        EM_GETPASSWORDCHAR => edit(hwnd).password as isize,
        EM_GETMODIFY => isize::from(edit(hwnd).modified),
        EM_SETMODIFY => {
            let mut c = edit(hwnd);
            c.modified = wparam != 0;
            put(&c);
            0
        }
        EM_GETLINECOUNT => {
            let c = edit(hwnd);
            1 + text_of(&c).iter().filter(|&&ch| ch == u16::from(b'\n')).count() as isize
        }
        EM_LINEINDEX | EM_LINELENGTH | EM_LINEFROMCHAR | EM_GETLINE => {
            edit_lines(hwnd, msg, wparam, lparam)
        }
        EM_GETFIRSTVISIBLELINE => 0,
        EM_SCROLLCARET | EM_EMPTYUNDOBUFFER => 0,
        EM_CANUNDO | EM_UNDO => 0,
        EM_SETMARGINS => {
            let mut c = edit(hwnd);
            if wparam & 1 != 0 {
                c.margins.0 = (lparam & 0xffff) as i32;
            }
            if wparam & 2 != 0 {
                c.margins.1 = ((lparam >> 16) & 0xffff) as i32;
            }
            put(&c);
            0
        }
        EM_GETRECT => {
            if lparam != 0 {
                let mut r = [0i32; 4];
                crate::tree::GetClientRect(hwnd, &mut r);
                let l = text_left(hwnd);
                // SAFETY: the caller's RECT.
                unsafe { *(lparam as *mut [i32; 4]) = [r[0] + l, r[1] + 1, r[2] - l, r[3] - 1] };
            }
            0
        }
        EM_SETCUEBANNER => {
            let mut c = edit(hwnd);
            c.cue = [0; 48];
            let mut n = 0;
            // SAFETY: the caller's NUL-terminated text.
            while lparam != 0 && n < 47 && unsafe { *(lparam as *const u16).add(n) } != 0 {
                c.cue[n] = unsafe { *(lparam as *const u16).add(n) };
                n += 1;
            }
            put(&c);
            redraw(hwnd);
            1
        }
        EM_GETCUEBANNER => {
            let c = edit(hwnd);
            let n =
                c.cue.iter().position(|&ch| ch == 0).unwrap_or(0).min(lparam.max(1) as usize - 1);
            if wparam != 0 {
                // SAFETY: the caller's buffer of `lparam` characters.
                unsafe {
                    core::ptr::copy_nonoverlapping(c.cue.as_ptr(), wparam as *mut u16, n);
                    *(wparam as *mut u16).add(n) = 0;
                }
            }
            1
        }
        WM_GETDLGCODE => {
            // DLGC_WANTCHARS | DLGC_HASSETSEL | DLGC_WANTARROWS, and all
            // keys for several lines.
            0x80 | 0x8 | 0x1 | if s & ES_MULTILINE != 0 { 0x4 } else { 0 }
        }
        WM_CHAR => {
            let mut c = edit(hwnd);
            let ch = wparam as u16;
            match ch {
                1 => {
                    // Ctrl+A: everything.
                    c.anchor = 0;
                    c.caret = c.len;
                    put(&c);
                    redraw(hwnd);
                    return 0;
                }
                8 => {
                    if s & ES_READONLY != 0 {
                        return 0;
                    }
                    if c.anchor == c.caret && c.caret > 0 {
                        c.anchor = c.caret - 1;
                    }
                    let (done, _) = replace(&mut c, &[]);
                    put(&c);
                    if done {
                        changed(hwnd);
                    }
                    return 0;
                }
                0x0d if s & ES_MULTILINE != 0 => {
                    if s & ES_READONLY != 0 {
                        return 0;
                    }
                    let (done, cut) = replace(&mut c, &[0x0d, 0x0a]);
                    put(&c);
                    if done {
                        changed(hwnd);
                    }
                    if cut {
                        notify(hwnd, EN_MAXTEXT);
                    }
                    return 0;
                }
                _ => {}
            }
            if ch < 0x20 || s & ES_READONLY != 0 {
                return 0;
            }
            if s & ES_NUMBER != 0 && !(u16::from(b'0')..=u16::from(b'9')).contains(&ch) {
                return 0;
            }
            let ch = if s & ES_UPPERCASE != 0 && (0x61..=0x7a).contains(&ch) {
                ch - 32
            } else if s & ES_LOWERCASE != 0 && (0x41..=0x5a).contains(&ch) {
                ch + 32
            } else {
                ch
            };
            let (done, cut) = replace(&mut c, &[ch]);
            put(&c);
            if done {
                changed(hwnd);
            }
            if cut {
                notify(hwnd, EN_MAXTEXT);
            }
            0
        }
        WM_KEYDOWN => {
            let mut c = edit(hwnd);
            let shift = crate::window::GetKeyState(0x10) < 0;
            let caret = match wparam {
                0x25 => c.caret.saturating_sub(1), // VK_LEFT
                0x27 => (c.caret + 1).min(c.len),  // VK_RIGHT
                0x24 => 0,                         // VK_HOME
                0x23 => c.len,                     // VK_END
                0x2e => {
                    // VK_DELETE
                    if s & ES_READONLY == 0 {
                        if c.anchor == c.caret && c.caret < c.len {
                            c.caret += 1;
                        }
                        let (done, _) = replace(&mut c, &[]);
                        put(&c);
                        if done {
                            changed(hwnd);
                        }
                    }
                    return 0;
                }
                _ => return 0,
            };
            c.caret = caret;
            if !shift {
                c.anchor = caret;
            }
            put(&c);
            redraw(hwnd);
            0
        }
        WM_LBUTTONDOWN => {
            crate::tree::SetFocus(hwnd);
            let mut c = edit(hwnd);
            let at = char_at(hwnd, &c, lparam as i16 as i32);
            c.caret = at;
            if crate::window::GetKeyState(0x10) >= 0 {
                c.anchor = at;
            }
            put(&c);
            crate::window::SetCapture(hwnd);
            redraw(hwnd);
            0
        }
        WM_MOUSEMOVE => {
            if crate::window::GetCapture() == hwnd {
                let mut c = edit(hwnd);
                let at = char_at(hwnd, &c, lparam as i16 as i32);
                if at != c.caret {
                    c.caret = at;
                    put(&c);
                    redraw(hwnd);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            if crate::window::GetCapture() == hwnd {
                crate::window::ReleaseCapture();
            }
            0
        }
        WM_LBUTTONDBLCLK => {
            let mut c = edit(hwnd);
            c.anchor = 0;
            c.caret = c.len;
            put(&c);
            redraw(hwnd);
            0
        }
        WM_SETFOCUS => {
            redraw(hwnd);
            notify(hwnd, EN_SETFOCUS);
            0
        }
        WM_KILLFOCUS => {
            redraw(hwnd);
            notify(hwnd, EN_KILLFOCUS);
            0
        }
        WM_PAINT => {
            paint_edit(hwnd);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// EM_LINEINDEX, EM_LINELENGTH, EM_LINEFROMCHAR and EM_GETLINE over the
/// text's lines (split at line feeds).
fn edit_lines(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    let c = edit(hwnd);
    let text = text_of(&c);
    let starts = || {
        core::iter::once(0).chain(
            text.iter().enumerate().filter(|&(_, &ch)| ch == u16::from(b'\n')).map(|(i, _)| i + 1),
        )
    };
    let line_end = |start: usize| {
        let e = text[start..]
            .iter()
            .position(|&ch| ch == u16::from(b'\n'))
            .map_or(text.len(), |i| start + i);
        if e > start && text[e - 1] == 0x0d { e - 1 } else { e }
    };
    match msg {
        EM_LINEINDEX => {
            let line = if wparam as isize == -1 {
                starts().filter(|&s| s <= c.caret).count() - 1
            } else {
                wparam
            };
            starts().nth(line).map_or(-1, |s| s as isize)
        }
        EM_LINEFROMCHAR => {
            let at = if wparam as isize == -1 { c.caret } else { wparam };
            (starts().filter(|&s| s <= at).count() - 1) as isize
        }
        EM_LINELENGTH => {
            let at = if wparam as isize == -1 { c.caret } else { wparam.min(text.len()) };
            let start = starts().filter(|&s| s <= at).last().unwrap_or(0);
            (line_end(start) - start) as isize
        }
        EM_GETLINE => {
            let Some(start) = starts().nth(wparam) else { return 0 };
            let end = line_end(start);
            if lparam == 0 {
                return 0;
            }
            // SAFETY: the caller's buffer: its size in the first WORD.
            let room = usize::from(unsafe { *(lparam as *const u16) });
            let n = (end - start).min(room);
            // SAFETY: `n` characters into the caller's buffer.
            unsafe {
                core::ptr::copy_nonoverlapping(text.as_ptr().add(start), lparam as *mut u16, n)
            };
            n as isize
        }
        _ => 0,
    }
}

fn paint_edit(hwnd: Hwnd) {
    let mut c = edit(hwnd);
    let s = style(hwnd);
    let ex = crate::tree::GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    let focused = crate::tree::GetFocus() == hwnd;
    let disabled = s & WS_DISABLED != 0;
    let readonly = s & ES_READONLY != 0;
    paint(hwnd, c.font, |hdc, r| {
        let msg = if readonly || disabled { WM_CTLCOLORSTATIC } else { WM_CTLCOLOREDIT };
        let brush = parent_brush(hwnd, hdc, msg, if readonly || disabled { 15 } else { 5 });
        crate::window::FillRect(hdc, &r, brush);
        let boxed = ex & WS_EX_CLIENTEDGE != 0 || s & WS_BORDER != 0;
        if boxed {
            let face = if focused || readonly { WHITE } else { BUTTON_FACE };
            round_box(hdc, r, 4, face, EDIT_BORDER);
            // The underline: the accent, two pixels, when focused.
            if focused && !readonly {
                fill(hdc, r[0] + 2, r[3] - 2, r[2] - 2, r[3], ACCENT);
            } else {
                fill(hdc, r[0] + 2, r[3] - 1, r[2] - 2, r[3], EDIT_UNDERLINE);
            }
        }
        transparent(hdc);
        let h = text_height(hdc);
        let left = text_left(hwnd) + c.margins.0;
        let right = r[2] - text_left(hwnd) - c.margins.1;
        if s & ES_MULTILINE != 0 {
            // Several lines from the top; no scrolling or selection yet.
            text_color(hdc, if disabled { TEXT_DISABLED } else { TEXT });
            let text = text_of(&c);
            let mut y = r[1] + 3;
            for line in text.split(|&ch| ch == u16::from(b'\n')) {
                let line = line.strip_suffix(&[0x0d]).unwrap_or(line);
                text_out(hdc, left, y, line);
                y += h;
                if y > r[3] {
                    break;
                }
            }
            return;
        }
        let y = r[1] + (r[3] - r[1] - h) / 2;
        let shown = shown_text(&c);
        if c.len == 0 && !focused && c.cue[0] != 0 {
            let n = c.cue.iter().position(|&ch| ch == 0).unwrap_or(48);
            text_color(hdc, CUE);
            text_out(hdc, left, y, &c.cue[..n]);
            return;
        }
        // Keep the caret in view.
        let caret_x = text_width(hdc, &shown[..c.caret.min(shown.len())]);
        let room = (right - left).max(1);
        if caret_x - c.scroll > room {
            c.scroll = caret_x - room;
        } else if caret_x < c.scroll {
            c.scroll = caret_x;
        }
        let total = text_width(hdc, shown);
        let x0 = match s & 3 {
            ES_CENTER if total < room => left + (room - total) / 2,
            ES_RIGHT if total < room => right - total,
            _ => left - c.scroll,
        };
        let (a, b) = selection(&c);
        let (a, b) = (a.min(shown.len()), b.min(shown.len()));
        let show_selection = a != b && (focused || s & ES_NOHIDESEL != 0);
        let ink = if disabled { TEXT_DISABLED } else { TEXT };
        let wa = text_width(hdc, &shown[..a]);
        let wb = text_width(hdc, &shown[..b]);
        if show_selection {
            fill(hdc, x0 + wa, y, x0 + wb, y + h, SELECTION);
        }
        text_color(hdc, ink);
        text_out(hdc, x0, y, &shown[..a]);
        text_color(hdc, if show_selection { WHITE } else { ink });
        text_out(hdc, x0 + wa, y, &shown[a..b]);
        text_color(hdc, ink);
        text_out(hdc, x0 + wb, y, &shown[b..]);
        if focused && !readonly {
            let x = x0 + caret_x;
            fill(hdc, x, y, x + 1, y + h, TEXT);
        }
    });
    put(&c);
}
