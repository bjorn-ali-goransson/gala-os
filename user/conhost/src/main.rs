//! gala's conhost.exe (not Microsoft's): draws a console session in a
//! window (ARCHITECTURE "Console windows"). The kernel starts it with the
//! session's id and the program's name; it becomes the session's host
//! (GalaConsoleHost), takes the output its programs write (GalaConsoleRead)
//! when the host event says some arrived, and keeps a grid of cells: UTF-8
//! text and the VT sequences programs use (CR, LF as a new line, BS, TAB,
//! CSI m colours, CSI K, CSI J, CSI H/f, cursor moves). The colours are
//! Windows Terminal's default scheme ("Campbell"). Typed keys go back to
//! the session as key records (GalaConsoleInput).

#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::ffi::c_void;

type Handle = *mut c_void;
type Hwnd = *mut c_void;
type WndProc = extern "system" fn(Hwnd, u32, usize, isize) -> isize;

#[repr(C)]
struct WndClassExW {
    size: u32,
    style: u32,
    proc_: WndProc,
    class_extra: i32,
    window_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
    icon_small: Handle,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct PaintStruct {
    hdc: Handle,
    erase: i32,
    paint: Rect,
    restore: i32,
    inc_update: i32,
    reserved: [u8; 32],
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: [i32; 2],
    private: u32,
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegisterClassExW(class: *const WndClassExW) -> u16;
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Hwnd,
        menu: Handle,
        instance: Handle,
        param: *mut c_void,
    ) -> Hwnd;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn BeginPaint(hwnd: Hwnd, ps: *mut PaintStruct) -> Handle;
    fn EndPaint(hwnd: Hwnd, ps: *const PaintStruct) -> i32;
    fn InvalidateRect(hwnd: Hwnd, rect: *const Rect, erase: i32) -> i32;
    fn PeekMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32, remove: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostQuitMessage(code: i32);
    fn MsgWaitForMultipleObjects(
        n: u32,
        handles: *const Handle,
        all: i32,
        ms: u32,
        mask: u32,
    ) -> u32;
    fn AdjustWindowRectEx(rect: *mut Rect, style: u32, menu: i32, ex: u32) -> i32;
    fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
    fn CreateIconIndirect(info: *const IconInfo) -> Handle;
    fn GetDC(hwnd: Hwnd) -> Handle;
}

#[link(name = "gdi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike: u32,
        charset: u32,
        precision: u32,
        clip: u32,
        quality: u32,
        pitch: u32,
        face: *const u16,
    ) -> Handle;
    fn SelectObject(hdc: Handle, object: Handle) -> Handle;
    fn CreateBitmap(w: i32, h: i32, planes: u32, bpp: u32, bits: *const c_void) -> Handle;
    fn GetTextMetricsW(hdc: Handle, tm: *mut [i32; 15]) -> i32;
    fn SetTextColor(hdc: Handle, color: u32) -> u32;
    fn SetBkColor(hdc: Handle, color: u32) -> u32;
    fn ExtTextOutW(
        hdc: Handle,
        x: i32,
        y: i32,
        options: u32,
        rect: *const Rect,
        text: *const u16,
        n: u32,
        dx: *const i32,
    ) -> i32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetCommandLineW() -> *const u16;
    fn CreateEventW(sa: *const c_void, manual: i32, initial: i32, name: *const u16) -> Handle;
    fn ExitProcess(code: u32) -> !;
    fn GetModuleHandleW(name: *const u16) -> Handle;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaConsoleHost(id: u32, event: Handle) -> u32;
    fn GalaConsoleRead(id: u32, buf: *mut u8, len: u32, count: *mut u32) -> u32;
    fn GalaConsoleInput(id: u32, records: *const u8, count: u32) -> u32;
}

// What the compiler's output expects from a C runtime.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
}

const WM_DESTROY: u32 = 0x0002;
const WM_PAINT: u32 = 0x000f;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_SIZE: u32 = 0x0005;
const WM_MOUSEWHEEL: u32 = 0x020a;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_CHAR: u32 = 0x0102;
const WM_QUIT: u32 = 0x0012;
const WS_OVERLAPPEDWINDOW: u32 = 0x00cf_0000;
const SW_SHOW: i32 = 5;
const ETO_OPAQUE: u32 = 0x2;
const QS_ALLINPUT: u32 = 0x04ff;
const INFINITE: u32 = 0xffff_ffff;
const IDC_ARROW: usize = 32512;

/// The grid's size at first (the window follows it), and at most (the
/// window's size decides it after a resize).
pub const COLS: usize = 100;
pub const ROWS: usize = 28;
pub const MAX_COLS: usize = 240;
pub const MAX_ROWS: usize = 80;
const PAD: i32 = 6;

/// Windows Terminal's "Campbell" scheme (COLORREF: 0x00BBGGRR), the eight
/// colours and their bright forms.
const PALETTE: [u32; 16] = [
    rgb(0x0c, 0x0c, 0x0c),
    rgb(0xc5, 0x0f, 0x1f),
    rgb(0x13, 0xa1, 0x0e),
    rgb(0xc1, 0x9c, 0x00),
    rgb(0x00, 0x37, 0xda),
    rgb(0x88, 0x17, 0x98),
    rgb(0x3a, 0x96, 0xdd),
    rgb(0xcc, 0xcc, 0xcc),
    rgb(0x76, 0x76, 0x76),
    rgb(0xe7, 0x48, 0x56),
    rgb(0x16, 0xc6, 0x0c),
    rgb(0xf9, 0xf1, 0xa5),
    rgb(0x3b, 0x78, 0xff),
    rgb(0xb4, 0x00, 0x9e),
    rgb(0x61, 0xd6, 0xd6),
    rgb(0xf2, 0xf2, 0xf2),
];
const DEFAULT_FG: u8 = 7;
const DEFAULT_BG: u8 = 0;

const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | g << 8 | b << 16
}

#[derive(Clone, Copy)]
struct Cell {
    ch: u16,
    fg: u8,
    bg: u8,
}

const BLANK: Cell = Cell { ch: b' ' as u16, fg: DEFAULT_FG, bg: DEFAULT_BG };

/// Lines kept above the screen (the scrollback).
const HISTORY: usize = 300;

/// The scrollback: the lines that scrolled off the top, oldest first (a
/// ring), and how far up the view is (0: the screen itself). All zeros at
/// first, so it costs the program file nothing (a zero cell shows as a
/// blank).
struct History {
    lines: [[Cell; MAX_COLS]; HISTORY],
    head: usize,
    len: usize,
    view: usize,
}

impl History {
    fn push(&mut self, line: &[Cell; MAX_COLS]) {
        let at = (self.head + self.len) % HISTORY;
        self.lines[at] = *line;
        if self.len < HISTORY {
            self.len += 1;
        } else {
            self.head = (self.head + 1) % HISTORY;
        }
        // A view scrolled up keeps showing the same lines.
        if self.view > 0 {
            self.view = (self.view + 1).min(self.len);
        }
    }

    /// The `i`th line kept (0: the oldest).
    fn line(&self, i: usize) -> &[Cell; MAX_COLS] {
        &self.lines[(self.head + i) % HISTORY]
    }

    /// Scrolls the view `lines` up (negative: down), within what is kept.
    fn scroll(&mut self, lines: isize) {
        self.view = (self.view as isize + lines).clamp(0, self.len as isize) as usize;
    }
}

struct HistoryCell(UnsafeCell<History>);
// SAFETY: one thread (the window's) uses it.
unsafe impl Sync for HistoryCell {}

const ZERO: Cell = Cell { ch: 0, fg: 0, bg: 0 };
static HISTORY_LINES: HistoryCell = HistoryCell(UnsafeCell::new(History {
    lines: [[ZERO; MAX_COLS]; HISTORY],
    head: 0,
    len: 0,
    view: 0,
}));

fn history() -> &'static mut History {
    // SAFETY: only the window's thread calls it, never across a borrow.
    unsafe { &mut *HISTORY_LINES.0.get() }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Parse {
    Ground,
    Escape,
    Csi,
}

/// The screen: cells, the cursor, the current colours and the parser.
pub struct Screen {
    cells: [[Cell; MAX_COLS]; MAX_ROWS],
    /// The grid's size in use.
    cols: usize,
    rows: usize,
    x: usize,
    y: usize,
    fg: u8,
    bg: u8,
    bold: bool,
    state: Parse,
    params: [u16; 8],
    count: usize,
    /// A UTF-8 sequence in progress: the code point so far and the bytes
    /// still to come.
    utf8: u32,
    pending: u8,
}

impl Screen {
    pub const fn new() -> Self {
        Screen {
            cells: [[BLANK; MAX_COLS]; MAX_ROWS],
            cols: COLS,
            rows: ROWS,
            x: 0,
            y: 0,
            fg: DEFAULT_FG,
            bg: DEFAULT_BG,
            bold: false,
            state: Parse::Ground,
            params: [0; 8],
            count: 0,
            utf8: 0,
            pending: 0,
        }
    }

    fn blank(&self) -> Cell {
        Cell { ch: b' ' as u16, fg: self.fg, bg: self.bg }
    }

    /// To the next line, scrolling the screen up at the bottom (the top
    /// line goes into the scrollback).
    fn line_feed(&mut self) {
        if self.y + 1 < self.rows {
            self.y += 1;
        } else {
            history().push(&self.cells[0]);
            self.cells.copy_within(1..self.rows, 0);
            self.cells[self.rows - 1] = [self.blank(); MAX_COLS];
        }
    }

    /// A new grid size (the window's): the cursor's line stays in view
    /// (the lines above scroll off when there are fewer rows); lines are
    /// cut, not rewrapped, when there are fewer columns.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.clamp(1, MAX_COLS), rows.clamp(1, MAX_ROWS));
        if self.y >= rows {
            let up = self.y + 1 - rows;
            for line in &self.cells[..up] {
                history().push(line);
            }
            self.cells.copy_within(up.., 0);
            for line in &mut self.cells[MAX_ROWS - up..] {
                *line = [BLANK; MAX_COLS];
            }
            self.y -= up;
        }
        // What was outside the old grid is blank in the new one.
        for line in &mut self.cells[..rows] {
            for c in &mut line[self.cols.min(cols)..cols] {
                *c = BLANK;
            }
        }
        for line in &mut self.cells[self.rows.min(rows)..rows] {
            *line = [BLANK; MAX_COLS];
        }
        (self.cols, self.rows) = (cols, rows);
        self.x = self.x.min(cols);
    }

    fn put(&mut self, ch: u16) {
        if self.x >= self.cols {
            self.x = 0;
            self.line_feed();
        }
        let fg = if self.bold && self.fg < 8 { self.fg + 8 } else { self.fg };
        self.cells[self.y][self.x] = Cell { ch, fg, bg: self.bg };
        self.x += 1;
    }

    /// One code point of text or a control character.
    fn char(&mut self, c: u32) {
        match c {
            0x0a => {
                // The console's default mode: a line feed starts the line.
                self.x = 0;
                self.line_feed();
            }
            0x0d => self.x = 0,
            0x08 => self.x = self.x.saturating_sub(1),
            0x09 => {
                let next = (self.x / 8 + 1) * 8;
                while self.x < next.min(self.cols) {
                    self.put(b' ' as u16);
                }
            }
            0x1b => {
                self.state = Parse::Escape;
            }
            0x00..=0x1f | 0x7f => {}
            _ => {
                let mut units = [0u16; 2];
                for &u in char::from_u32(c).unwrap_or('?').encode_utf16(&mut units).iter() {
                    self.put(u);
                }
            }
        }
    }

    fn param(&self, i: usize, default: u16) -> u16 {
        if i < self.count && self.params[i] != 0 { self.params[i] } else { default }
    }

    /// A CSI sequence ending in `final_`.
    fn csi(&mut self, final_: u8) {
        match final_ {
            b'm' => {
                let n = self.count.max(1);
                for i in 0..n {
                    let p = if i < self.count { self.params[i] } else { 0 };
                    match p {
                        0 => {
                            self.fg = DEFAULT_FG;
                            self.bg = DEFAULT_BG;
                            self.bold = false;
                        }
                        1 => self.bold = true,
                        22 => self.bold = false,
                        30..=37 => self.fg = (p - 30) as u8,
                        39 => self.fg = DEFAULT_FG,
                        40..=47 => self.bg = (p - 40) as u8,
                        49 => self.bg = DEFAULT_BG,
                        90..=97 => self.fg = (p - 90) as u8 + 8,
                        100..=107 => self.bg = (p - 100) as u8 + 8,
                        _ => {}
                    }
                }
            }
            b'K' => {
                let blank = self.blank();
                let (from, to) = match self.param(0, 0) {
                    1 => (0, self.x.min(self.cols - 1) + 1),
                    2 => (0, self.cols),
                    _ => (self.x.min(self.cols), self.cols),
                };
                for c in &mut self.cells[self.y][from..to] {
                    *c = blank;
                }
            }
            b'J' => {
                if self.param(0, 0) == 2 || self.param(0, 0) == 3 {
                    let blank = self.blank();
                    self.cells = [[blank; MAX_COLS]; MAX_ROWS];
                }
            }
            b'H' | b'f' => {
                self.y = (self.param(0, 1) as usize - 1).min(self.rows - 1);
                self.x = (self.param(1, 1) as usize - 1).min(self.cols - 1);
            }
            b'A' => self.y = self.y.saturating_sub(self.param(0, 1) as usize),
            b'B' => self.y = (self.y + self.param(0, 1) as usize).min(self.rows - 1),
            b'C' => self.x = (self.x + self.param(0, 1) as usize).min(self.cols - 1),
            b'D' => self.x = self.x.saturating_sub(self.param(0, 1) as usize),
            b'G' => self.x = (self.param(0, 1) as usize - 1).min(self.cols - 1),
            _ => {}
        }
    }

    /// Bytes the session's programs wrote (UTF-8).
    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            match self.state {
                Parse::Escape => {
                    if b == b'[' {
                        self.state = Parse::Csi;
                        self.params = [0; 8];
                        self.count = 0;
                    } else {
                        self.state = Parse::Ground;
                    }
                    continue;
                }
                Parse::Csi => {
                    match b {
                        b'0'..=b'9' => {
                            if self.count == 0 {
                                self.count = 1;
                            }
                            let i = self.count - 1;
                            if i < 8 {
                                self.params[i] = self.params[i]
                                    .saturating_mul(10)
                                    .saturating_add(u16::from(b - b'0'));
                            }
                        }
                        b';' => self.count = (self.count.max(1) + 1).min(8),
                        b'?' | b'>' | b'=' => {}
                        0x40..=0x7e => {
                            self.state = Parse::Ground;
                            self.csi(b);
                        }
                        _ => self.state = Parse::Ground,
                    }
                    continue;
                }
                Parse::Ground => {}
            }
            // UTF-8.
            if self.pending > 0 {
                if b & 0xc0 == 0x80 {
                    self.utf8 = self.utf8 << 6 | u32::from(b & 0x3f);
                    self.pending -= 1;
                    if self.pending == 0 {
                        self.char(self.utf8);
                    }
                    continue;
                }
                self.pending = 0;
                self.char(0xfffd);
            }
            match b {
                0x00..=0x7f => self.char(u32::from(b)),
                0xc0..=0xdf => {
                    self.utf8 = u32::from(b & 0x1f);
                    self.pending = 1;
                }
                0xe0..=0xef => {
                    self.utf8 = u32::from(b & 0x0f);
                    self.pending = 2;
                }
                0xf0..=0xf7 => {
                    self.utf8 = u32::from(b & 0x07);
                    self.pending = 3;
                }
                _ => self.char(0xfffd),
            }
        }
    }
}

struct State {
    screen: Screen,
    font: Handle,
    cell_w: i32,
    cell_h: i32,
    session: u32,
}

struct Shared(UnsafeCell<State>);
// SAFETY: one thread (the window's) uses it.
unsafe impl Sync for Shared {}

static STATE: Shared = Shared(UnsafeCell::new(State {
    screen: Screen::new(),
    font: core::ptr::null_mut(),
    cell_w: 8,
    cell_h: 16,
    session: 0,
}));

fn state() -> &'static mut State {
    // SAFETY: only the window's thread (this program has one) calls it,
    // and never re-entrantly across a borrow.
    unsafe { &mut *STATE.0.get() }
}

fn paint(hwnd: Hwnd) {
    let st = state();
    let mut ps = PaintStruct {
        hdc: core::ptr::null_mut(),
        erase: 0,
        paint: Rect::default(),
        restore: 0,
        inc_update: 0,
        reserved: [0; 32],
    };
    // SAFETY: our window and DC.
    unsafe {
        let hdc = BeginPaint(hwnd, &mut ps);
        if hdc.is_null() {
            return;
        }
        SelectObject(hdc, st.font);
        let (cw, ch) = (st.cell_w, st.cell_h);
        let (cols, rows) = (st.screen.cols, st.screen.rows);
        // The margin around the grid and what the grid doesn't fill.
        let total = Rect { left: 0, top: 0, right: 0x7fff, bottom: 0x7fff };
        SetBkColor(hdc, PALETTE[DEFAULT_BG as usize]);
        ExtTextOutW(hdc, 0, 0, ETO_OPAQUE, &total, core::ptr::null(), 0, core::ptr::null());
        let hist = history();
        // The view: the scrollback's last `view` lines, then the screen.
        let first = hist.len - hist.view;
        for row in 0..rows {
            let i = first + row;
            let cells = if i < hist.len { hist.line(i) } else { &st.screen.cells[i - hist.len] };
            let y = PAD + row as i32 * ch;
            let mut start = 0;
            while start < cols {
                let (fg, bg) = (cells[start].fg, cells[start].bg);
                let mut end = start;
                let mut text = [0u16; MAX_COLS];
                while end < cols && cells[end].fg == fg && cells[end].bg == bg {
                    text[end - start] =
                        if cells[end].ch == 0 { b' ' as u16 } else { cells[end].ch };
                    end += 1;
                }
                let r = Rect {
                    left: PAD + start as i32 * cw,
                    top: y,
                    right: PAD + end as i32 * cw,
                    bottom: y + ch,
                };
                SetTextColor(hdc, PALETTE[fg as usize & 15]);
                SetBkColor(hdc, PALETTE[bg as usize & 15]);
                let mut dx = [cw; MAX_COLS];
                dx[0] = cw;
                ExtTextOutW(
                    hdc,
                    r.left,
                    y,
                    ETO_OPAQUE,
                    &r,
                    text.as_ptr(),
                    (end - start) as u32,
                    dx.as_ptr(),
                );
                start = end;
            }
        }
        // The cursor: an underline in the text colour (not when scrolled
        // up).
        let s = &st.screen;
        if s.x < cols && hist.view == 0 {
            let cx = PAD + s.x as i32 * cw;
            let cy = PAD + s.y as i32 * ch + ch - 2;
            let r = Rect { left: cx, top: cy, right: cx + cw, bottom: cy + 2 };
            SetBkColor(hdc, PALETTE[DEFAULT_FG as usize]);
            ExtTextOutW(hdc, 0, 0, ETO_OPAQUE, &r, core::ptr::null(), 0, core::ptr::null());
        }
        EndPaint(hwnd, &ps);
    }
}

/// A KEY_EVENT_RECORD for the session (16 bytes).
fn send_key(down: bool, vk: u16, ch: u16) {
    let mut r = [0u8; 16];
    r[0] = u8::from(down);
    r[4..6].copy_from_slice(&1u16.to_le_bytes());
    r[6..8].copy_from_slice(&vk.to_le_bytes());
    r[10..12].copy_from_slice(&ch.to_le_bytes());
    // SAFETY: one record of ours.
    unsafe { GalaConsoleInput(state().session, r.as_ptr(), 1) };
}

/// ICONINFO.
#[repr(C)]
struct IconInfo {
    icon: i32,
    x: u32,
    y: u32,
    mask: Handle,
    color: Handle,
}

/// The console's icon (32 x 32): a dark rounded square with a grey
/// outline and a prompt, ">_", in the text colour.
fn console_icon() -> Handle {
    static mut PIXELS: [u32; 32 * 32] = [0; 32 * 32];
    static MASK: [u8; 4 * 32] = [0; 4 * 32];
    // SAFETY: one thread, before the window; plain calls on our data.
    unsafe {
        let px = &mut *core::ptr::addr_of_mut!(PIXELS);
        for y in 0..32i32 {
            for x in 0..32i32 {
                // The corners (radius 4) are transparent.
                let (dx, dy) = ((3 - x).max(x - 28).max(0), (3 - y).max(y - 28).max(0));
                let p = if dx * dx + dy * dy > 16 {
                    0
                } else if x == 0 || y == 0 || x == 31 || y == 31 || dx * dx + dy * dy > 9 {
                    0xff76_7676
                } else {
                    0xff0c_0c0c
                };
                px[(y * 32 + x) as usize] = p;
            }
        }
        let text = 0xffcc_cccc;
        // ">": two diagonals from (7, 9) to (14, 15) to (7, 21), 2 wide.
        for i in 0..7 {
            for w in 0..2 {
                px[((9 + i) * 32 + 7 + i + w) as usize] = text;
                px[((21 - i) * 32 + 7 + i + w) as usize] = text;
            }
        }
        // "_": a bar under it.
        for x in 16..25 {
            px[(21 * 32 + x) as usize] = text;
            px[(22 * 32 + x) as usize] = text;
        }
        let info = IconInfo {
            icon: 1,
            x: 0,
            y: 0,
            mask: CreateBitmap(32, 32, 1, 1, MASK.as_ptr().cast()),
            color: CreateBitmap(32, 32, 1, 32, px.as_ptr().cast()),
        };
        CreateIconIndirect(&info)
    }
}

extern "system" fn window_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    match msg {
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        // The grid follows the window (Snap, maximize): as many cells as
        // the client area holds.
        WM_SIZE => {
            let st = state();
            let (w, h) = (lparam as u16 as i32, (lparam >> 16) as u16 as i32);
            if w > 0 && h > 0 {
                let cols = ((w - 2 * PAD) / st.cell_w.max(1)).max(1) as usize;
                let rows = ((h - 2 * PAD) / st.cell_h.max(1)).max(1) as usize;
                st.screen.resize(cols, rows);
                // SAFETY: our window.
                unsafe { InvalidateRect(hwnd, core::ptr::null(), 0) };
            }
            0
        }
        // The wheel scrolls through the scrollback, three lines a notch.
        WM_MOUSEWHEEL => {
            let delta = (wparam >> 16) as u16 as i16;
            history().scroll(isize::from(delta) / 40);
            // SAFETY: our window.
            unsafe { InvalidateRect(hwnd, core::ptr::null(), 0) };
            0
        }
        WM_CHAR => {
            // Typing goes back to the screen.
            if core::mem::take(&mut history().view) > 0 {
                // SAFETY: our window.
                unsafe { InvalidateRect(hwnd, core::ptr::null(), 0) };
            }
            send_key(true, 0, wparam as u16);
            0
        }
        WM_KEYDOWN | WM_KEYUP => {
            // Keys without a character (arrows, function keys) as key
            // records; characters come with WM_CHAR.
            if matches!(wparam, 0x21..=0x28 | 0x2d | 0x2e | 0x70..=0x7b) {
                send_key(msg == WM_KEYDOWN, wparam as u16, 0);
            }
            0
        }
        WM_DESTROY => {
            // SAFETY: a plain call.
            unsafe { PostQuitMessage(0) };
            0
        }
        // SAFETY: the default processing.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// The command line after the program's own path: "<id> <title>".
fn arguments(out: &mut [u16; 260]) -> (u32, usize) {
    // SAFETY: the process's NUL-terminated command line.
    let line = unsafe { GetCommandLineW() };
    let at = |i: usize| unsafe { *line.add(i) };
    let mut i = 0;
    // The program's path (maybe quoted).
    if at(0) == u16::from(b'"') {
        i = 1;
        while at(i) != 0 && at(i) != u16::from(b'"') {
            i += 1;
        }
        if at(i) != 0 {
            i += 1;
        }
    } else {
        while at(i) != 0 && at(i) != u16::from(b' ') {
            i += 1;
        }
    }
    while at(i) == u16::from(b' ') {
        i += 1;
    }
    let mut id = 0u32;
    while (u16::from(b'0')..=u16::from(b'9')).contains(&at(i)) {
        id = id * 10 + u32::from(at(i) - u16::from(b'0'));
        i += 1;
    }
    while at(i) == u16::from(b' ') {
        i += 1;
    }
    let mut n = 0;
    while at(i) != 0 && n < out.len() - 1 {
        out[n] = at(i);
        n += 1;
        i += 1;
    }
    out[n] = 0;
    (id, n)
}

fn wide<const N: usize>(s: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (o, c) in out.iter_mut().zip(s.encode_utf16()) {
        *o = c;
    }
    out
}

fn run() -> u32 {
    let mut title = [0u16; 260];
    let (session, _) = arguments(&mut title);
    let st = state();
    st.session = session;
    // SAFETY: plain calls with valid arguments.
    unsafe {
        let event = CreateEventW(core::ptr::null(), 0, 0, core::ptr::null());
        if GalaConsoleHost(session, event) != 0 {
            return 1;
        }
        let instance = GetModuleHandleW(core::ptr::null());
        let class: [u16; 16] = wide("GalaConsole");
        let wc = WndClassExW {
            size: size_of::<WndClassExW>() as u32,
            style: 0,
            proc_: window_proc,
            class_extra: 0,
            window_extra: 0,
            instance,
            icon: console_icon(),
            cursor: LoadCursorW(core::ptr::null_mut(), IDC_ARROW as *const u16),
            background: core::ptr::null_mut(),
            menu_name: core::ptr::null(),
            class_name: class.as_ptr(),
            icon_small: core::ptr::null_mut(),
        };
        if RegisterClassExW(&wc) == 0 {
            return 2;
        }
        // Cascadia Mono / Consolas: gdi32 gives its fixed-pitch face.
        let face: [u16; 16] = wide("Consolas");
        st.font = CreateFontW(-14, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 0, 0x31, face.as_ptr());
        let screen_dc = GetDC(core::ptr::null_mut());
        if !screen_dc.is_null() {
            let old = SelectObject(screen_dc, st.font);
            let mut tm = [0i32; 15];
            if GetTextMetricsW(screen_dc, &mut tm) != 0 {
                st.cell_h = tm[0].max(8);
                st.cell_w = tm[5].max(4); // tmAveCharWidth
            }
            SelectObject(screen_dc, old);
        }
        let mut r = Rect {
            left: 0,
            top: 0,
            right: 2 * PAD + st.cell_w * COLS as i32,
            bottom: 2 * PAD + st.cell_h * ROWS as i32,
        };
        AdjustWindowRectEx(&mut r, WS_OVERLAPPEDWINDOW, 0, 0);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            60,
            60,
            r.right - r.left,
            r.bottom - r.top,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            instance,
            core::ptr::null_mut(),
        );
        if hwnd.is_null() {
            return 3;
        }
        ShowWindow(hwnd, SW_SHOW);
        let mut buf = [0u8; 1024];
        let mut msg = Msg {
            hwnd: core::ptr::null_mut(),
            message: 0,
            wparam: 0,
            lparam: 0,
            time: 0,
            pt: [0; 2],
            private: 0,
        };
        loop {
            // Output first, then the window's messages.
            let mut changed = false;
            loop {
                let mut n = 0u32;
                if GalaConsoleRead(session, buf.as_mut_ptr(), buf.len() as u32, &mut n) != 0
                    || n == 0
                {
                    break;
                }
                st.screen.feed(&buf[..n as usize]);
                changed = true;
            }
            if changed {
                InvalidateRect(hwnd, core::ptr::null(), 0);
            }
            while PeekMessageW(&mut msg, core::ptr::null_mut(), 0, 0, 1) != 0 {
                if msg.message == WM_QUIT {
                    return 0;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            MsgWaitForMultipleObjects(1, &event, 0, INFINITE, QS_ALLINPUT);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = run();
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
