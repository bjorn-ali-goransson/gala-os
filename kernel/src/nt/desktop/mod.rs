//! The desktop: top-level windows of Win32 programs, composited on the
//! screen, with a small shell (taskbar, start menu) and the mouse.
//!
//! The kernel keeps each window's frame (rounded corners, a shaded caption
//! with the title and minimize/close buttons, drawn here) and a copy of its
//! client area, which the program's user32/gdi32 draw in their own memory
//! and present (`present`). Every pixel is composed in layers: the
//! wallpaper, the windows bottom to top (each with a soft shadow), the
//! taskbar (translucent), the start menu (with its shadow) and the pointer.
//! Changes collect damage rectangles which `with`/`pointer_event`/`tick`
//! redraw.
//!
//! The mouse: a click raises and activates a window, its caption drags
//! it, the caption buttons minimize it or ask it to close (WM_CLOSE to the
//! program), the taskbar's buttons restore, activate or minimize windows,
//! the start button opens the start menu, whose items start programs.
//! Clicks and moves in a client area become input for its program.
//! Design: docs/ARCHITECTURE.md, "Windows on the screen".

mod paint;

use super::object::ObjectRef;
use super::platform;
use super::status::NtStatus;
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;
use alloc::vec::Vec;
use paint::{Canvas, Font, Wallpaper, blend, lerp, over, round_coverage2, shadow_alpha};

/// Frame metrics (what user32's GetSystemMetrics reports).
pub const BORDER: u32 = 1;
pub const CAPTION: u32 = 30;
/// Corner radii of a window: above and below.
const RADIUS: i64 = 8;
const RADIUS_BOTTOM: i64 = 6;
/// The caption buttons' width.
const CAPTION_BUTTON: i64 = 46;
/// Soft shadows: how far they reach, how far down they fall, how dark.
const SHADOW_REACH: i64 = 22;
const SHADOW_DROP: i64 = 8;
const SHADOW_ACTIVE: i64 = 110;
const SHADOW_INACTIVE: i64 = 70;
/// Without a known screen (host tests of the windows alone).
pub const DESKTOP_COLOR: u32 = 0x00_3a_6e_a5;

/// The theme. Captions: the active window's deep indigo, the others
/// slate; the outline matches.
const CAPTION_ACTIVE: (u32, u32) = (0x00_34_47_6e, 0x00_22_30_4d);
const CAPTION_INACTIVE: (u32, u32) = (0x00_4a_52_62, 0x00_3c_43_51);
const CAPTION_HIGHLIGHT: u32 = 0x00_6c_80_a8;
const OUTLINE_ACTIVE: u32 = 0x00_15_1f_33;
const OUTLINE_INACTIVE: u32 = 0x00_2c_32_3c;
const TITLE_ACTIVE: u32 = 0x00_f5_f7_fa;
const TITLE_INACTIVE: u32 = 0x00_b4_bb_c8;
const CLOSE_HOVER: u32 = 0x00_e5_48_4d;
/// The camel's orange: the active indicators.
const ACCENT: u32 = 0x00_f5_a5_24;

/// The taskbar: the bottom rows of the screen.
pub const TASKBAR: u32 = 48;
const TASKBAR_TOP: u32 = 0x00_1e_26_38;
const TASKBAR_BOTTOM: u32 = 0x00_11_16_22;
const TASKBAR_ALPHA: u32 = 232;
const TASKBAR_LINE: u32 = 0x00_3e_4b_66;
const START: Rect = Rect { x: 4, y: 4, w: 44, h: 40 };
const TASKS_X: i64 = 56;
const TASK_W: i64 = 200;
const CLOCK_W: i64 = 104;

/// The start menu: above the start button.
pub const MENU_W: u32 = 380;
pub const MENU_H: u32 = 460;
const MENU_ITEMS_Y: i64 = 146;
const MENU_ITEM_H: i64 = 44;

/// The start button's picture at 20, 40 and 64 pixels, RGBA (straight
/// alpha), made from kernel/assets/camel.png (docs/WORKFLOW.md).
static CAMEL40: &[u8; 40 * 40 * 4] = include_bytes!("../../../assets/camel40.rgba");
static CAMEL64: &[u8; 64 * 64 * 4] = include_bytes!("../../../assets/camel64.rgba");
static CAMEL20: &[u8; 20 * 20 * 4] = include_bytes!("../../../assets/camel20.rgba");

/// The mouse pointer: '#' outline, '.' fill, ' ' transparent.
const CURSOR: [&[u8; 12]; 19] = [
    b"#           ",
    b"##          ",
    b"#.#         ",
    b"#..#        ",
    b"#...#       ",
    b"#....#      ",
    b"#.....#     ",
    b"#......#    ",
    b"#.......#   ",
    b"#........#  ",
    b"#.........# ",
    b"#..........#",
    b"#......#####",
    b"#...#..#    ",
    b"#..# #..#   ",
    b"#.#  #..#   ",
    b"##    #..#  ",
    b"      #..#  ",
    b"       ##   ",
];

/// What the start menu offers: shown when the file exists.
pub struct Program {
    pub name: &'static str,
    pub path: &'static str,
    /// The icon's gradient.
    pub colors: (u32, u32),
}

pub const PROGRAMS: &[Program] = &[
    Program {
        name: "FileZilla",
        path: "C:\\filezilla\\filezilla.exe",
        colors: (0x00_e0_4f_3f, 0x00_a3_2a_20),
    },
    Program {
        name: "Hello window",
        path: "C:\\gui_test.exe",
        colors: (0x00_5b_9b_ff, 0x00_2f_5f_d8),
    },
    Program {
        name: "Hello window",
        path: "C:\\gala\\gui_test.exe",
        colors: (0x00_5b_9b_ff, 0x00_2f_5f_d8),
    },
];

/// The largest window gala accepts (each pixel is kept twice).
const MAX_SIDE: u32 = 4096;
/// Input waiting for programs (oldest dropped beyond this).
const MAX_INPUT: usize = 512;

/// Window messages the desktop sends to programs.
pub const WM_MOVE: u32 = 0x0003;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const WM_SYSKEYUP: u32 = 0x0105;
const VK_MENU: u8 = 0x12;

/// Input for a program's window: a window message and its parameters
/// (mouse positions in client coordinates).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputEvent {
    pub owner: u64,
    pub window: u32,
    pub message: u32,
    pub wparam: u64,
    pub lparam: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptionButton {
    Minimize,
    Close,
}

/// What is under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Nothing,
    Start,
    Taskbar,
    Task(u32),
    Menu,
    MenuItem(usize),
    Caption(u32),
    Button(u32, CaptionButton),
    Client(u32),
    Frame(u32),
}

pub struct Window {
    pub id: u32,
    pub owner: u64,
    x: i32,
    y: i32,
    /// The outer size (frame included).
    w: u32,
    h: u32,
    /// Shown by its program.
    visible: bool,
    /// Minimized by the user (on the taskbar only).
    minimized: bool,
    /// The caption bar's pixels (w x CAPTION) and the state they show.
    caption: Option<Canvas>,
    caption_state: Option<(bool, Option<CaptionButton>)>,
    /// The client area's pixels (client_w * client_h), white at first.
    client: Vec<u32>,
    /// The title (UTF-16).
    title: Vec<u16>,
}

impl Window {
    fn client_size(&self) -> (u32, u32) {
        (self.w - 2 * BORDER, self.h - 2 * BORDER - CAPTION)
    }

    fn shown(&self) -> bool {
        self.visible && !self.minimized
    }

    /// The screen rectangle it covers.
    fn rect(&self) -> Rect {
        Rect { x: i64::from(self.x), y: i64::from(self.y), w: self.w.into(), h: self.h.into() }
    }

    /// With its shadow.
    fn bounds(&self) -> Rect {
        let r = self.rect();
        Rect {
            x: r.x - SHADOW_REACH,
            y: r.y - SHADOW_REACH,
            w: r.w + 2 * SHADOW_REACH,
            h: r.h + 2 * SHADOW_REACH + SHADOW_DROP,
        }
    }

    fn client_origin(&self) -> (i64, i64) {
        (i64::from(self.x) + i64::from(BORDER), i64::from(self.y) + i64::from(BORDER + CAPTION))
    }

    fn caption_rect(&self) -> Rect {
        Rect {
            x: i64::from(self.x),
            y: i64::from(self.y),
            w: self.w.into(),
            h: i64::from(BORDER + CAPTION),
        }
    }

    /// How much of window point (lx, ly) its rounded shape covers.
    fn coverage(&self, lx: i64, ly: i64) -> u8 {
        round_coverage2(lx, ly, self.w.into(), self.h.into(), RADIUS, RADIUS_BOTTOM)
    }

    /// The colour of window point (lx, ly) inside its shape.
    fn body(&self, lx: i64, ly: i64, active: bool) -> u32 {
        let (w, h) = (i64::from(self.w), i64::from(self.h));
        let b = i64::from(BORDER);
        let inner =
            round_coverage2(lx - b, ly - b, w - 2 * b, h - 2 * b, RADIUS - 1, RADIUS_BOTTOM - 1);
        let outline = if active { OUTLINE_ACTIVE } else { OUTLINE_INACTIVE };
        if inner == 0 {
            return outline;
        }
        let c = if ly < b + i64::from(CAPTION) {
            self.caption.as_ref().map_or(CAPTION_INACTIVE.0, |c| c.get(lx, ly - b) & 0x00ff_ffff)
        } else {
            let (cw, _) = self.client_size();
            let (cx, cy) = (lx - b, ly - b - i64::from(CAPTION));
            self.client.get((cy * i64::from(cw) + cx) as usize).copied().unwrap_or(0x00ff_ffff)
        };
        blend(outline, c, inner)
    }

    /// Draws the caption bar for its state.
    fn render_caption(&mut self, active: bool, hover: Option<CaptionButton>) {
        if self.caption_state == Some((active, hover)) && self.caption.is_some() {
            return;
        }
        let (w, h) = (i64::from(self.w), i64::from(CAPTION));
        let (top, bottom) = if active { CAPTION_ACTIVE } else { CAPTION_INACTIVE };
        let Some(mut c) = Canvas::new(w, h, 0xff00_0000) else { return };
        for y in 0..h {
            let row = if y == 0 { CAPTION_HIGHLIGHT } else { lerp(top, bottom, y, h - 1) };
            for x in 0..w {
                c.put(x, y, row, 255);
            }
        }
        // The buttons: minimize, close.
        let close_x = w - CAPTION_BUTTON - 1;
        let min_x = close_x - CAPTION_BUTTON;
        let glyph = if active { TITLE_ACTIVE } else { TITLE_INACTIVE };
        match hover {
            Some(CaptionButton::Close) => {
                for y in 0..h {
                    for x in close_x..close_x + CAPTION_BUTTON {
                        c.put(x, y, CLOSE_HOVER, 255);
                    }
                }
            }
            Some(CaptionButton::Minimize) => {
                for y in 0..h {
                    for x in min_x..min_x + CAPTION_BUTTON {
                        c.put(x, y, 0x00ff_ffff, 34);
                    }
                }
            }
            None => {}
        }
        let (cx, cy) = (close_x + CAPTION_BUTTON / 2, h / 2);
        let close_glyph = if hover == Some(CaptionButton::Close) { 0x00ff_ffff } else { glyph };
        for t in -5..=5 {
            c.put(cx + t, cy + t, close_glyph, 255);
            c.put(cx + t, cy - t, close_glyph, 255);
            // A little softness on the diagonals.
            c.put(cx + t + 1, cy + t, close_glyph, 70);
            c.put(cx + t + 1, cy - t, close_glyph, 70);
        }
        let mx = min_x + CAPTION_BUTTON / 2;
        for t in -5..=5 {
            c.put(mx + t, cy, glyph, 255);
        }
        // The title, with a dot in the accent colour on the active window.
        let mut x = 14;
        if active {
            c.round_rect(x, h / 2 - 4, 8, 8, 4, ACCENT, ACCENT, 255);
        } else {
            c.round_rect(x, h / 2 - 4, 8, 8, 4, TITLE_INACTIVE, TITLE_INACTIVE, 140);
        }
        x += 16;
        let title = char::decode_utf16(self.title.iter().copied()).map(|c| c.unwrap_or('?'));
        let font = if active { Font::Bold } else { Font::Regular };
        let text = if active { TITLE_ACTIVE } else { TITLE_INACTIVE };
        c.text(x, (h - font.height()) / 2, title, text, font, min_x - 8);
        self.caption = Some(c);
        self.caption_state = Some((active, hover));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

impl Rect {
    fn contains(&self, x: i64, y: i64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }

    /// Clipped to the screen `(0, 0, sw, sh)`.
    fn clip(self, sw: i64, sh: i64) -> Option<Rect> {
        let (x0, y0) = (self.x.max(0), self.y.max(0));
        let (x1, y1) = ((self.x + self.w).min(sw), (self.y + self.h).min(sh));
        (x1 > x0 && y1 > y0).then_some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }

    fn offset(self, dx: i64, dy: i64) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, ..self }
    }
}

pub struct Desktop {
    /// Bottom to top.
    windows: Vec<Window>,
    next_id: u32,
    /// The whole screen has been painted once (the console drew it before).
    painted: bool,
    /// The screen's size (0 x 0 until known: no shell or pointer).
    screen: (i64, i64),
    wallpaper: Option<Wallpaper>,
    /// The taskbar's pixels (screen width x TASKBAR, ARGB).
    taskbar: Option<Canvas>,
    taskbar_dirty: bool,
    /// The start menu's pixels while it is open (ARGB) and the programs
    /// it lists (indices into PROGRAMS).
    menu: Option<Canvas>,
    menu_dirty: bool,
    programs: Vec<usize>,
    /// The mouse pointer's hot spot and the buttons held (bit 0 left,
    /// bit 1 right).
    pointer: (i64, i64),
    buttons: u8,
    hover: Hit,
    /// Where the left button went down.
    pressed: Hit,
    /// A window being dragged by its caption: (id, grab offset).
    drag: Option<(u32, i64, i64)>,
    active: Option<u32>,
    /// The minute the clock shows.
    minute: u64,
    damage: Vec<Rect>,
    input: Vec<InputEvent>,
    /// A program the start menu asked to start (index into PROGRAMS).
    launch: Option<usize>,
    /// Each process's event to signal when input arrives for it
    /// (GalaWindowListen), and the processes with new input.
    listeners: Vec<(u64, ObjectRef)>,
    woken: Vec<u64>,
    /// Alt is held (WM_SYSKEYDOWN).
    alt: bool,
}

impl Default for Desktop {
    fn default() -> Self {
        Self::new()
    }
}

impl Desktop {
    pub const fn new() -> Self {
        Desktop {
            windows: Vec::new(),
            next_id: 1,
            painted: false,
            screen: (0, 0),
            wallpaper: None,
            taskbar: None,
            taskbar_dirty: true,
            menu: None,
            menu_dirty: false,
            programs: Vec::new(),
            pointer: (0, 0),
            buttons: 0,
            hover: Hit::Nothing,
            pressed: Hit::Nothing,
            drag: None,
            active: None,
            minute: 0,
            damage: Vec::new(),
            input: Vec::new(),
            launch: None,
            listeners: Vec::new(),
            woken: Vec::new(),
            alt: false,
        }
    }

    /// The screen is w x h: lays out the wallpaper and the taskbar and
    /// puts the pointer in the middle.
    pub fn set_screen(&mut self, w: u32, h: u32) -> Result<(), NtStatus> {
        let (w, h) = (i64::from(w), i64::from(h));
        self.wallpaper = Some(Wallpaper::new(w, h).ok_or(NtStatus::NO_MEMORY)?);
        self.screen = (w, h);
        self.pointer = (w / 2, h / 2);
        self.taskbar_dirty = true;
        self.refresh();
        Ok(())
    }

    fn add_damage(&mut self, r: Rect) {
        if self.damage.len() >= 64 {
            self.damage.clear();
            let _ = self.damage.try_push(Rect { x: 0, y: 0, w: self.screen.0, h: self.screen.1 });
        } else if !self
            .damage
            .iter()
            .any(|d| d.contains(r.x, r.y) && d.contains(r.x + r.w - 1, r.y + r.h - 1))
        {
            let _ = self.damage.try_push(r);
        }
    }

    fn find(&mut self, id: u32, owner: u64) -> Result<&mut Window, NtStatus> {
        self.windows
            .iter_mut()
            .find(|w| w.id == id && w.owner == owner)
            .ok_or(NtStatus::INVALID_HANDLE)
    }

    fn window(&self, id: u32) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    fn index(&self, id: u32) -> Option<usize> {
        self.windows.iter().position(|w| w.id == id)
    }

    // ---- What programs do ----------------------------------------------

    /// A new hidden window of outer size w x h at (x, y) titled `title`.
    pub fn create(
        &mut self,
        owner: u64,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        title: &[u16],
    ) -> Result<u32, NtStatus> {
        let min_w = 2 * BORDER + 2 * CAPTION_BUTTON as u32 + 40;
        if w < min_w || h <= 2 * BORDER + CAPTION || w > MAX_SIDE || h > MAX_SIDE {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let mut client = Vec::new();
        let n = ((w - 2 * BORDER) * (h - 2 * BORDER - CAPTION)) as usize;
        client.try_reserve_exact(n).map_err(|_| NtStatus::NO_MEMORY)?;
        client.resize(n, 0x00_ff_ff_ff);
        let mut name = Vec::new();
        name.try_extend_from_slice(title).map_err(|_| NtStatus::NO_MEMORY)?;
        let id = self.next_id;
        let window = Window {
            id,
            owner,
            x,
            y,
            w,
            h,
            visible: false,
            minimized: false,
            caption: None,
            caption_state: None,
            client,
            title: name,
        };
        self.windows.try_reserve(1).map_err(|_| NtStatus::NO_MEMORY)?;
        self.windows.try_push(window).map_err(|_| NtStatus::NO_MEMORY)?;
        self.next_id += 1;
        Ok(id)
    }

    /// Shows or hides a window (shown: on top and active).
    pub fn show(&mut self, id: u32, owner: u64, visible: bool) -> Result<Rect, NtStatus> {
        let win = self.find(id, owner)?;
        let was = win.visible;
        win.visible = visible;
        win.minimized = false;
        let bounds = win.bounds();
        if visible && !was {
            self.raise(id);
            self.active = Some(id);
        }
        if !visible && self.active == Some(id) {
            self.activate_top();
        }
        self.taskbar_dirty = true;
        self.add_damage(bounds);
        self.refresh();
        Ok(bounds)
    }

    /// Copies a w x h rectangle at (x, y) of the client area from `pixels`
    /// (rows of `stride` pixels); the screen area to redraw.
    pub fn present(
        &mut self,
        id: u32,
        owner: u64,
        r: Rect,
        pixels: &[u32],
        stride: usize,
    ) -> Result<Rect, NtStatus> {
        let win = self.find(id, owner)?;
        let (cw, ch) = win.client_size();
        let clipped = r.clip(cw.into(), ch.into()).ok_or(NtStatus::INVALID_PARAMETER)?;
        let (dx, dy) = (clipped.x - r.x, clipped.y - r.y);
        for row in 0..clipped.h {
            let src = ((dy + row) as usize) * stride + dx as usize;
            let dst = ((clipped.y + row) as u32 * cw) as usize + clipped.x as usize;
            let n = clipped.w as usize;
            let (Some(from), Some(to)) =
                (pixels.get(src..src + n), win.client.get_mut(dst..dst + n))
            else {
                return Err(NtStatus::INVALID_PARAMETER);
            };
            to.copy_from_slice(from);
        }
        let origin = win.client_origin();
        let area = clipped.offset(origin.0, origin.1);
        if win.shown() {
            self.add_damage(area);
        }
        Ok(area)
    }

    /// Moves and resizes a window (outer rectangle); its client pixels
    /// keep what fits, new ones white. The area to redraw.
    pub fn place(
        &mut self,
        id: u32,
        owner: u64,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    ) -> Result<Rect, NtStatus> {
        let min_w = 2 * BORDER + 2 * CAPTION_BUTTON as u32 + 40;
        if w < min_w || h <= 2 * BORDER + CAPTION || w > MAX_SIDE || h > MAX_SIDE {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let win = self.find(id, owner)?;
        let old = win.bounds();
        if (w, h) != (win.w, win.h) {
            let (ow, oh) = win.client_size();
            let (nw, nh) = (w - 2 * BORDER, h - 2 * BORDER - CAPTION);
            let mut client = Vec::new();
            client.try_reserve_exact((nw * nh) as usize).map_err(|_| NtStatus::NO_MEMORY)?;
            client.resize((nw * nh) as usize, 0x00_ff_ff_ff);
            for y in 0..oh.min(nh) {
                for x in 0..ow.min(nw) {
                    client[(y * nw + x) as usize] = win.client[(y * ow + x) as usize];
                }
            }
            win.client = client;
            win.w = w;
            win.h = h;
            win.caption = None;
            win.caption_state = None;
        }
        win.x = x;
        win.y = y;
        let new = win.bounds();
        self.add_damage(old);
        self.add_damage(new);
        self.refresh();
        Ok(new)
    }

    /// A window's new title (the caption and the taskbar show it).
    pub fn set_title(&mut self, id: u32, owner: u64, title: &[u16]) -> Result<Rect, NtStatus> {
        let mut name = Vec::new();
        name.try_extend_from_slice(title).map_err(|_| NtStatus::NO_MEMORY)?;
        let win = self.find(id, owner)?;
        win.title = name;
        win.caption_state = None;
        let r = win.caption_rect();
        self.taskbar_dirty = true;
        self.add_damage(r);
        self.refresh();
        Ok(r)
    }

    pub fn destroy(&mut self, id: u32, owner: u64) -> Result<Rect, NtStatus> {
        let i = self.windows.iter().position(|w| w.id == id && w.owner == owner);
        let i = i.ok_or(NtStatus::INVALID_HANDLE)?;
        let bounds = self.windows.remove(i).bounds();
        self.forget(id);
        self.add_damage(bounds);
        self.refresh();
        Ok(bounds)
    }

    /// Removes `owner`'s windows; the areas to redraw.
    pub fn destroy_owned(&mut self, owner: u64) -> Vec<Rect> {
        let mut gone = Vec::new();
        let mut ids = Vec::new();
        self.windows.retain(|w| {
            if w.owner == owner {
                let _ = gone.try_push(w.bounds());
                let _ = ids.try_push(w.id);
                false
            } else {
                true
            }
        });
        for id in ids {
            self.forget(id);
        }
        self.input.retain(|e| e.owner != owner);
        self.listeners.retain(|(o, _)| *o != owner);
        self.woken.retain(|o| *o != owner);
        for &r in &gone {
            self.add_damage(r);
        }
        self.refresh();
        gone
    }

    /// A window went: nothing refers to it any more.
    fn forget(&mut self, id: u32) {
        if self.active == Some(id) {
            self.activate_top();
        }
        if self.drag.is_some_and(|d| d.0 == id) {
            self.drag = None;
        }
        self.input.retain(|e| e.window != id);
        self.taskbar_dirty = true;
    }

    /// Input waiting for `owner`'s windows, oldest first, at most
    /// `out.len()`; how many.
    pub fn take_input(&mut self, owner: u64, out: &mut [InputEvent]) -> usize {
        let mut n = 0;
        self.input.retain(|e| {
            if e.owner == owner && n < out.len() {
                out[n] = *e;
                n += 1;
                false
            } else {
                true
            }
        });
        n
    }

    pub fn has_input(&self, owner: u64) -> bool {
        self.input.iter().any(|e| e.owner == owner)
    }

    fn send(&mut self, window: u32, message: u32, wparam: u64, lparam: u64) {
        let Some(owner) = self.window(window).map(|w| w.owner) else { return };
        // A move replaces the window's previous unread one.
        if message == WM_MOUSEMOVE
            && let Some(last) = self.input.last_mut()
            && last.window == window
            && last.message == WM_MOUSEMOVE
        {
            last.wparam = wparam;
            last.lparam = lparam;
            return;
        }
        if self.input.len() >= MAX_INPUT {
            self.input.remove(0);
        }
        let _ = self.input.try_push(InputEvent { owner, window, message, wparam, lparam });
        if !self.woken.contains(&owner) {
            let _ = self.woken.try_push(owner);
        }
    }

    /// `owner`'s event to signal when input arrives for it (one per
    /// process; a new one replaces the old).
    pub fn listen(&mut self, owner: u64, event: ObjectRef) -> Result<(), NtStatus> {
        self.listeners.retain(|(o, _)| *o != owner);
        self.listeners.try_push((owner, event)).map_err(|_| NtStatus::NO_MEMORY)?;
        if self.has_input(owner) && !self.woken.contains(&owner) {
            let _ = self.woken.try_push(owner);
        }
        Ok(())
    }

    /// The events to signal for processes with new input (signalled by the
    /// caller after letting go of the desktop).
    fn take_wakes(&mut self) -> Vec<ObjectRef> {
        let mut out = Vec::new();
        for owner in core::mem::take(&mut self.woken) {
            if let Some((_, e)) = self.listeners.iter().find(|(o, _)| *o == owner) {
                let _ = out.try_push(e.clone());
            }
        }
        out
    }

    /// A key went down or up: virtual key `vk`, `scan` code, `extended`
    /// (E0-prefixed). To the active window, as WM_KEYDOWN/WM_KEYUP (with
    /// Alt held: WM_SYSKEYDOWN/WM_SYSKEYUP), lParam as Windows packs it.
    pub fn key(&mut self, vk: u8, scan: u8, extended: bool, down: bool) {
        if vk == VK_MENU {
            self.alt = down;
        }
        let Some(id) = self.active else { return };
        let mut lparam = 1u64 | u64::from(scan) << 16 | u64::from(extended) << 24;
        if self.alt {
            lparam |= 1 << 29;
        }
        if !down {
            lparam |= 3 << 30;
        }
        let message = match (self.alt || vk == VK_MENU, down) {
            (false, true) => WM_KEYDOWN,
            (false, false) => WM_KEYUP,
            (true, true) => WM_SYSKEYDOWN,
            (true, false) => WM_SYSKEYUP,
        };
        self.send(id, message, u64::from(vk), lparam);
    }

    // ---- Window management ---------------------------------------------

    fn raise(&mut self, id: u32) {
        if let Some(i) = self.index(id)
            && i + 1 != self.windows.len()
        {
            let w = self.windows.remove(i);
            let bounds = w.bounds();
            let _ = self.windows.try_push(w);
            self.add_damage(bounds);
        }
    }

    fn activate(&mut self, id: Option<u32>) {
        if self.active != id {
            self.active = id;
            self.taskbar_dirty = true;
        }
    }

    /// The topmost shown window becomes active (or none).
    fn activate_top(&mut self) {
        let top = self.windows.iter().rev().find(|w| w.shown()).map(|w| w.id);
        self.activate(top);
    }

    fn minimize(&mut self, id: u32) {
        let Some(i) = self.index(id) else { return };
        self.windows[i].minimized = true;
        let bounds = self.windows[i].bounds();
        self.add_damage(bounds);
        if self.active == Some(id) {
            self.activate_top();
        }
        self.taskbar_dirty = true;
    }

    fn restore(&mut self, id: u32) {
        let Some(i) = self.index(id) else { return };
        self.windows[i].minimized = false;
        let bounds = self.windows[i].bounds();
        self.add_damage(bounds);
        self.raise(id);
        self.activate(Some(id));
        self.taskbar_dirty = true;
    }

    /// Moves a window to (x, y), its caption kept on the screen.
    fn move_to(&mut self, id: u32, x: i64, y: i64) {
        let (sw, sh) = self.screen;
        let Some(i) = self.index(id) else { return };
        let w = &self.windows[i];
        let (ww, wh) = (i64::from(w.w), i64::from(w.h));
        let _ = wh;
        let x = x.clamp(-ww + 80, (sw - 80).max(0));
        let y = y.clamp(0, (sh - i64::from(TASKBAR) - i64::from(CAPTION)).max(0));
        if (x, y) == (i64::from(w.x), i64::from(w.y)) {
            return;
        }
        let old = w.bounds();
        let w = &mut self.windows[i];
        w.x = x as i32;
        w.y = y as i32;
        let new = w.bounds();
        self.add_damage(old);
        self.add_damage(new);
    }

    // ---- The shell's layout --------------------------------------------

    fn taskbar_rect(&self) -> Rect {
        let h = i64::from(TASKBAR);
        Rect { x: 0, y: self.screen.1 - h, w: self.screen.0, h }
    }

    /// Where a click opens the start menu: the button and the screen's
    /// corner around it (a flick to the corner hits it).
    fn start_rect(&self) -> Rect {
        let h = i64::from(TASKBAR);
        Rect { x: 0, y: self.screen.1 - h, w: START.x + START.w + 2, h }
    }

    /// The taskbar's window buttons (windows the programs show, in the
    /// order they were made) and their rectangles.
    fn tasks(&self) -> Vec<(u32, Rect)> {
        let mut ids: Vec<u32> = Vec::new();
        for w in self.windows.iter().filter(|w| w.visible) {
            let _ = ids.try_push(w.id);
        }
        ids.sort_unstable();
        let room = self.screen.0 - TASKS_X - CLOCK_W - 8;
        let n = ids.len() as i64;
        let w = if n == 0 { TASK_W } else { (room / n - 4).clamp(48, TASK_W) };
        let top = self.screen.1 - i64::from(TASKBAR) + 4;
        let mut out = Vec::new();
        for (i, id) in ids.into_iter().enumerate() {
            let _ = out.try_push((id, Rect { x: TASKS_X + i as i64 * (w + 4), y: top, w, h: 40 }));
        }
        out
    }

    fn menu_rect(&self) -> Rect {
        let (w, h) = (i64::from(MENU_W), i64::from(MENU_H));
        Rect { x: 8, y: self.screen.1 - i64::from(TASKBAR) - 8 - h, w, h }
    }

    fn menu_bounds(&self) -> Rect {
        let r = self.menu_rect();
        Rect {
            x: r.x - SHADOW_REACH,
            y: r.y - SHADOW_REACH,
            w: r.w + 2 * SHADOW_REACH,
            h: r.h + 2 * SHADOW_REACH + SHADOW_DROP,
        }
    }

    fn menu_item_rect(&self, i: usize) -> Rect {
        let m = self.menu_rect();
        Rect {
            x: m.x + 12,
            y: m.y + MENU_ITEMS_Y + i as i64 * MENU_ITEM_H,
            w: m.w - 24,
            h: MENU_ITEM_H - 4,
        }
    }

    fn cursor_rect(&self) -> Rect {
        Rect { x: self.pointer.0, y: self.pointer.1, w: 14, h: CURSOR.len() as i64 + 2 }
    }

    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    /// The programs the start menu lists (indices into PROGRAMS).
    pub fn set_programs(&mut self, programs: &[usize]) {
        if self.programs.as_slice() != programs {
            self.programs.clear();
            let _ = self.programs.try_extend_from_slice(programs);
            self.menu_dirty = true;
        }
    }

    fn hit(&self, x: i64, y: i64) -> Hit {
        if self.screen.0 == 0 {
            return Hit::Nothing;
        }
        if self.menu.is_some() && self.menu_rect().contains(x, y) {
            for i in 0..self.programs.len() {
                if self.menu_item_rect(i).contains(x, y) {
                    return Hit::MenuItem(i);
                }
            }
            return Hit::Menu;
        }
        if self.taskbar_rect().contains(x, y) {
            if self.start_rect().contains(x, y) {
                return Hit::Start;
            }
            for (id, r) in self.tasks() {
                if r.contains(x, y) {
                    return Hit::Task(id);
                }
            }
            return Hit::Taskbar;
        }
        for w in self.windows.iter().rev().filter(|w| w.shown()) {
            let (lx, ly) = (x - i64::from(w.x), y - i64::from(w.y));
            if !w.rect().contains(x, y) || w.coverage(lx, ly) < 128 {
                continue;
            }
            let ww = i64::from(w.w);
            if ly < i64::from(BORDER + CAPTION) {
                let close_x = ww - CAPTION_BUTTON - 1;
                if ly >= 1 && lx >= close_x && lx < ww - 1 {
                    return Hit::Button(w.id, CaptionButton::Close);
                }
                if ly >= 1 && lx >= close_x - CAPTION_BUTTON && lx < close_x {
                    return Hit::Button(w.id, CaptionButton::Minimize);
                }
                return Hit::Caption(w.id);
            }
            let (cx, cy) = (lx - i64::from(BORDER), ly - i64::from(BORDER + CAPTION));
            let (cw, ch) = w.client_size();
            if cx >= 0 && cy >= 0 && cx < i64::from(cw) && cy < i64::from(ch) {
                return Hit::Client(w.id);
            }
            return Hit::Frame(w.id);
        }
        Hit::Nothing
    }

    // ---- The mouse -------------------------------------------------------

    /// The pointer moved by (dx, dy) (screen directions) with `buttons`
    /// held; the areas to redraw.
    pub fn pointer(&mut self, dx: i64, dy: i64, buttons: u8) -> Vec<Rect> {
        if self.screen.0 == 0 {
            return Vec::new();
        }
        let old_cursor = self.cursor_rect();
        self.pointer.0 = (self.pointer.0 + dx).clamp(0, self.screen.0 - 1);
        self.pointer.1 = (self.pointer.1 + dy).clamp(0, self.screen.1 - 1);
        let (x, y) = self.pointer;
        if old_cursor != self.cursor_rect() {
            self.add_damage(old_cursor);
            let c = self.cursor_rect();
            self.add_damage(c);
        }
        if let Some((id, gx, gy)) = self.drag {
            self.move_to(id, x - gx, y - gy);
        }
        let hit = self.hit(x, y);
        self.set_hover(hit);
        let pressed = buttons & !self.buttons;
        let released = self.buttons & !buttons;
        self.buttons = buttons;
        let keys = u64::from(buttons & 1) | (u64::from(buttons & 2));
        // Moves go to the window under the pointer, or to the one a button
        // went down in while it is held (capture).
        let target = match (self.pressed, hit) {
            (Hit::Client(id), _) if buttons & 1 != 0 => Some(id),
            (_, Hit::Client(id)) => Some(id),
            _ => None,
        };
        if (dx, dy) != (0, 0)
            && self.drag.is_none()
            && let Some(id) = target
        {
            let lp = self.client_lparam(id, x, y);
            self.send(id, WM_MOUSEMOVE, keys, lp);
        }
        if pressed & 1 != 0 {
            self.left_down(hit, x, y, keys);
        }
        if released & 1 != 0 {
            self.left_up(hit, x, y, keys);
        }
        if pressed & 2 != 0
            && let Hit::Client(id) = hit
        {
            self.raise(id);
            self.activate(Some(id));
            let lp = self.client_lparam(id, x, y);
            self.send(id, WM_RBUTTONDOWN, keys, lp);
        }
        if released & 2 != 0
            && let Hit::Client(id) = hit
        {
            let lp = self.client_lparam(id, x, y);
            self.send(id, WM_RBUTTONUP, keys, lp);
        }
        self.refresh();
        core::mem::take(&mut self.damage)
    }

    fn client_lparam(&self, id: u32, x: i64, y: i64) -> u64 {
        let (ox, oy) = self.window(id).map_or((0, 0), |w| w.client_origin());
        let (cx, cy) = ((x - ox) as i16 as u16, (y - oy) as i16 as u16);
        u64::from(cy) << 16 | u64::from(cx)
    }

    fn set_hover(&mut self, hit: Hit) {
        if hit == self.hover {
            return;
        }
        let old = self.hover;
        self.hover = hit;
        for h in [old, hit] {
            match h {
                Hit::Start | Hit::Task(_) | Hit::Taskbar => self.taskbar_dirty = true,
                Hit::MenuItem(_) | Hit::Menu => self.menu_dirty = true,
                _ => {}
            }
        }
    }

    fn left_down(&mut self, hit: Hit, x: i64, y: i64, keys: u64) {
        self.pressed = hit;
        // A click outside the open menu closes it (and does nothing else).
        if self.menu.is_some() && !matches!(hit, Hit::Menu | Hit::MenuItem(_) | Hit::Start) {
            self.close_menu();
            self.pressed = Hit::Nothing;
            return;
        }
        match hit {
            Hit::Start => {
                if self.menu.is_some() {
                    self.close_menu();
                } else {
                    self.menu_dirty = true;
                    self.menu = Canvas::new(1, 1, 0); // rendered by refresh
                    let b = self.menu_bounds();
                    self.add_damage(b);
                }
                self.taskbar_dirty = true;
            }
            Hit::Task(id) => {
                let minimized = self.window(id).is_some_and(|w| w.minimized);
                let top = self.windows.iter().rev().find(|w| w.shown()).map(|w| w.id);
                if minimized {
                    self.restore(id);
                } else if self.active == Some(id) && top == Some(id) {
                    self.minimize(id);
                } else {
                    self.raise(id);
                    self.activate(Some(id));
                }
            }
            Hit::Caption(id) => {
                self.raise(id);
                self.activate(Some(id));
                if let Some(w) = self.window(id) {
                    self.drag = Some((id, x - i64::from(w.x), y - i64::from(w.y)));
                }
            }
            Hit::Button(id, _) | Hit::Frame(id) => {
                self.raise(id);
                self.activate(Some(id));
            }
            Hit::Client(id) => {
                self.raise(id);
                self.activate(Some(id));
                let lp = self.client_lparam(id, x, y);
                self.send(id, WM_LBUTTONDOWN, keys, lp);
            }
            Hit::Menu | Hit::MenuItem(_) => self.menu_dirty = true,
            Hit::Nothing | Hit::Taskbar => {}
        }
    }

    fn left_up(&mut self, hit: Hit, x: i64, y: i64, keys: u64) {
        let pressed = core::mem::replace(&mut self.pressed, Hit::Nothing);
        // A drag that ends tells the program where its client area went
        // (WM_MOVE: the client area's screen position).
        if let Some((id, _, _)) = self.drag.take()
            && let Some(w) = self.window(id)
        {
            let (cx, cy) = w.client_origin();
            let lp = u64::from(cy as i16 as u16) << 16 | u64::from(cx as i16 as u16);
            self.send(id, WM_MOVE, 0, lp);
        }
        match (pressed, hit) {
            (Hit::Button(a, CaptionButton::Close), Hit::Button(b, CaptionButton::Close))
                if a == b =>
            {
                self.send(a, WM_CLOSE, 0, 0);
            }
            (Hit::Button(a, CaptionButton::Minimize), Hit::Button(b, CaptionButton::Minimize))
                if a == b =>
            {
                self.minimize(a);
            }
            (Hit::MenuItem(a), Hit::MenuItem(b)) if a == b => {
                self.launch = self.programs.get(a).copied();
                self.close_menu();
            }
            (Hit::Client(id), _) => {
                let lp = self.client_lparam(id, x, y);
                self.send(id, WM_LBUTTONUP, keys, lp);
            }
            (Hit::Menu | Hit::MenuItem(_), _) => self.menu_dirty = true,
            _ => {}
        }
    }

    fn close_menu(&mut self) {
        if self.menu.take().is_some() {
            let b = self.menu_bounds();
            self.add_damage(b);
            self.taskbar_dirty = true;
        }
    }

    /// The program the start menu asked to start, once.
    pub fn take_launch(&mut self) -> Option<&'static Program> {
        self.launch.take().and_then(|i| PROGRAMS.get(i))
    }

    /// The clock shows `minute` (minutes since 1601, UTC).
    pub fn set_minute(&mut self, minute: u64) {
        if minute != self.minute {
            self.minute = minute;
            self.taskbar_dirty = true;
            self.refresh();
        }
    }

    // ---- Drawing -------------------------------------------------------

    /// Brings the captions, the taskbar and the menu up to date with the
    /// state, collecting damage.
    fn refresh(&mut self) {
        if self.screen.0 == 0 {
            // Windows alone (host tests): captions only.
            let active = self.active;
            for w in self.windows.iter_mut() {
                w.render_caption(active == Some(w.id), None);
            }
            return;
        }
        let active = self.active;
        let hover = self.hover;
        let mut changed = Vec::new();
        for w in self.windows.iter_mut() {
            let button = match hover {
                Hit::Button(id, b) if id == w.id => Some(b),
                _ => None,
            };
            let before = w.caption_state;
            w.render_caption(active == Some(w.id), button);
            if before != w.caption_state && w.shown() {
                let _ = changed.try_push(w.caption_rect());
            }
        }
        for r in changed {
            self.add_damage(r);
        }
        if self.taskbar_dirty || self.taskbar.is_none() {
            self.taskbar = self.render_taskbar();
            self.taskbar_dirty = false;
            let r = self.taskbar_rect();
            self.add_damage(r);
        }
        if self.menu.is_some() && self.menu_dirty {
            self.menu = self.render_menu();
            self.menu_dirty = false;
            let r = self.menu_rect();
            self.add_damage(r);
        }
    }

    fn render_taskbar(&self) -> Option<Canvas> {
        let (w, h) = (self.screen.0, i64::from(TASKBAR));
        let mut c = Canvas::new(w, h, 0)?;
        for y in 0..h {
            let (row, a) = if y == 0 {
                (TASKBAR_LINE, 255)
            } else {
                (lerp(TASKBAR_TOP, TASKBAR_BOTTOM, y, h - 1), TASKBAR_ALPHA)
            };
            for x in 0..w {
                c.put(x, y, row, a);
            }
        }
        let top = self.screen.1 - h;
        // The start button.
        let s = START;
        let glow = if self.menu.is_some() {
            48
        } else if self.hover == Hit::Start {
            28
        } else {
            0
        };
        if glow > 0 {
            c.round_rect(s.x, s.y, s.w, s.h, 10, 0x00ff_ffff, 0x00ff_ffff, glow);
        }
        c.image(s.x + (s.w - 40) / 2, s.y, CAMEL40, 40);
        // The windows.
        for (id, r) in self.tasks() {
            let Some(win) = self.window(id) else { continue };
            let (x, y) = (r.x, r.y - top);
            let active = self.active == Some(id) && win.shown();
            let alpha = if active {
                44
            } else if self.hover == Hit::Task(id) {
                26
            } else if win.minimized {
                6
            } else {
                12
            };
            c.round_rect(x, y, r.w, r.h, 8, 0x00ff_ffff, 0x00ff_ffff, alpha);
            // A little window as the icon.
            c.round_rect(x + 12, y + 12, 20, 16, 3, 0x00_8a_b4_ff, 0x00_4d_7f_e0, 255);
            c.round_rect(x + 14, y + 17, 16, 9, 1, 0x00ff_ffff, 0x00_e8_ee_f8, 235);
            let title = char::decode_utf16(win.title.iter().copied()).map(|c| c.unwrap_or('?'));
            let color = if active {
                0x00ff_ffff
            } else if win.minimized {
                0x00_8b_93_a1
            } else {
                0x00_d1_d5_db
            };
            c.text(x + 40, y + 12, title, color, Font::Regular, x + r.w - 8);
            if active {
                c.round_rect(x + r.w / 2 - 12, y + 35, 24, 3, 1, ACCENT, ACCENT, 255);
            } else if !win.minimized {
                c.round_rect(x + r.w / 2 - 4, y + 35, 8, 3, 1, 0x00_9c_a3_af, 0x00_9c_a3_af, 255);
            }
        }
        // The clock (UTC).
        let (hh, mm, date) = clock(self.minute);
        let time = [b'0' + hh / 10, b'0' + hh % 10, b':', b'0' + mm / 10, b'0' + mm % 10];
        let cx = w - CLOCK_W / 2 - 4;
        let tw = 5 * Font::Bold.advance();
        c.text(cx - tw / 2, 6, time.iter().map(|&b| b as char), 0x00_f3_f4_f6, Font::Bold, w);
        let dw = 10 * Font::Regular.advance();
        c.text(cx - dw / 2, 25, date.iter().map(|&b| b as char), 0x00_a7_b0_bf, Font::Regular, w);
        Some(c)
    }

    fn render_menu(&self) -> Option<Canvas> {
        let (w, h) = (i64::from(MENU_W), i64::from(MENU_H));
        let mut c = Canvas::new(w, h, 0)?;
        c.round_rect(0, 0, w, h, 14, 0x00_24_2d_42, 0x00_15_1b_29, 246);
        c.round_outline(0, 0, w, h, 14, 0x00_40_4d_6a, 255);
        // A warm glow behind the camel.
        c.round_rect(18, 18, 72, 72, 18, 0x00_f5_a5_24, 0x00_c2_5e_1a, 46);
        c.image(22, 22, CAMEL64, 64);
        c.text(104, 30, "gala-os".chars(), 0x00ff_ffff, Font::Large, w - 16);
        let version = env!("CARGO_PKG_VERSION");
        let x = c.text(104, 60, "version ".chars(), 0x00_9a_a4_b5, Font::Regular, w - 16);
        c.text(x, 60, version.chars(), 0x00_9a_a4_b5, Font::Regular, w - 16);
        for x in 20..w - 20 {
            c.put(x, 104, 0x00_36_43_5e, 255);
        }
        c.text(22, 118, "Programs".chars(), 0x00_c9_d1_de, Font::Bold, w - 16);
        let m = self.menu_rect();
        for (i, &p) in self.programs.iter().enumerate() {
            let Some(program) = PROGRAMS.get(p) else { continue };
            let r = self.menu_item_rect(i).offset(-m.x, -m.y);
            if r.y + r.h > h - 64 {
                break;
            }
            let glow = match (self.hover, self.pressed) {
                (Hit::MenuItem(a), Hit::MenuItem(b)) if a == i && b == i => 40,
                (Hit::MenuItem(a), _) if a == i => 24,
                _ => 0,
            };
            if glow > 0 {
                c.round_rect(r.x, r.y, r.w, r.h, 10, 0x00ff_ffff, 0x00ff_ffff, glow);
            }
            let (ix, iy) = (r.x + 10, r.y + (r.h - 28) / 2);
            c.round_rect(ix, iy, 28, 28, 8, program.colors.0, program.colors.1, 255);
            let letter = program.name.chars().next().unwrap_or('?');
            c.text(
                ix + (28 - Font::Bold.advance()) / 2,
                iy + 6,
                core::iter::once(letter),
                0x00ff_ffff,
                Font::Bold,
                ix + 28,
            );
            c.text(
                ix + 40,
                r.y + (r.h - 16) / 2,
                program.name.chars(),
                0x00_ee_f1_f6,
                Font::Regular,
                r.x + r.w - 8,
            );
        }
        if self.programs.is_empty() {
            c.text(
                22,
                MENU_ITEMS_Y + 10,
                "No programs installed".chars(),
                0x00_8b_93_a1,
                Font::Regular,
                w - 16,
            );
        }
        // The foot: who and what is running.
        for x in 20..w - 20 {
            c.put(x, h - 56, 0x00_36_43_5e, 255);
        }
        c.image(22, h - 38, CAMEL20, 20);
        let open = self.windows.iter().filter(|w| w.visible).count();
        let mut line = [0u8; 24];
        let n =
            format_count(&mut line, open, if open == 1 { " window open" } else { " windows open" });
        c.text(
            52,
            h - 36,
            line[..n].iter().map(|&b| b as char),
            0x00_9a_a4_b5,
            Font::Regular,
            w - 16,
        );
        Some(c)
    }

    /// The colour of screen point (x, y), all layers composed.
    pub fn pixel(&self, x: i64, y: i64) -> u32 {
        let mut c = self.wallpaper.as_ref().map_or(DESKTOP_COLOR, |w| w.pixel(x, y));
        for w in self.windows.iter().filter(|w| w.shown()) {
            if !w.bounds().contains(x, y) {
                continue;
            }
            let (lx, ly) = (x - i64::from(w.x), y - i64::from(w.y));
            let cov = w.coverage(lx, ly);
            let active = self.active == Some(w.id);
            if cov < 255 {
                let r = w.rect();
                let rect = (r.x + 3, r.y + SHADOW_DROP, r.x + r.w - 3, r.y + r.h + SHADOW_DROP - 2);
                let strength = if active { SHADOW_ACTIVE } else { SHADOW_INACTIVE };
                c = blend(c, 0, shadow_alpha(x, y, rect, SHADOW_REACH, strength));
            }
            if cov > 0 {
                c = blend(c, w.body(lx, ly, active), cov);
            }
        }
        if self.screen.0 == 0 {
            return c;
        }
        let t = self.taskbar_rect();
        if t.contains(x, y)
            && let Some(bar) = &self.taskbar
        {
            c = over(c, bar.get(x - t.x, y - t.y));
        }
        if let Some(menu) = &self.menu
            && self.menu_bounds().contains(x, y)
        {
            let m = self.menu_rect();
            let layer = menu.get(x - m.x, y - m.y);
            if layer >> 24 < 255 {
                let rect = (m.x + 4, m.y + SHADOW_DROP, m.x + m.w - 4, m.y + m.h + SHADOW_DROP - 2);
                c = blend(c, 0, shadow_alpha(x, y, rect, SHADOW_REACH, SHADOW_ACTIVE));
            }
            c = over(c, layer);
        }
        // The pointer, with a faint shadow.
        let (px, py) = self.pointer;
        let row = |yy: i64, xx: i64| -> u8 {
            CURSOR.get(yy as usize).and_then(|r| r.get(xx as usize)).copied().unwrap_or(b' ')
        };
        if x >= px && y >= py && x < px + 14 && y < py + 21 {
            match row(y - py, x - px) {
                b'#' => return 0x00_10_10_10,
                b'.' => return 0x00ff_ffff,
                _ => {}
            }
            if row(y - py - 1, x - px - 1) != b' ' || row(y - py - 2, x - px - 1) != b' ' {
                c = blend(c, 0, 70);
            }
        }
        c
    }
}

/// "<n><suffix>" into `out`; its length.
fn format_count(out: &mut [u8], n: usize, suffix: &str) -> usize {
    let mut digits = [0u8; 20];
    let mut len = 0;
    let mut v = n;
    loop {
        digits[len] = b'0' + (v % 10) as u8;
        len += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    let mut i = 0;
    for &d in digits[..len].iter().rev() {
        if i < out.len() {
            out[i] = d;
            i += 1;
        }
    }
    for b in suffix.bytes() {
        if i < out.len() {
            out[i] = b;
            i += 1;
        }
    }
    i
}

/// The clock: hours, minutes and "YYYY-MM-DD" of `minute` (minutes since
/// 1601-01-01 UTC). The date: the days-to-civil algorithm from Howard
/// Hinnant's "chrono-Compatible Low-Level Date Algorithms" (public domain).
fn clock(minute: u64) -> (u8, u8, [u8; 10]) {
    let hh = (minute / 60 % 24) as u8;
    let mm = (minute % 60) as u8;
    // Days since 1970-01-01 (1601 to 1970: 134774 days).
    let z = (minute / (60 * 24)) as i64 - 134_774 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let mut date = *b"0000-00-00";
    let y = y.clamp(0, 9999) as u32;
    for (i, div) in [1000, 100, 10, 1].iter().enumerate() {
        date[i] = b'0' + (y / div % 10) as u8;
    }
    date[5] = b'0' + (m / 10) as u8;
    date[6] = b'0' + (m % 10) as u8;
    date[8] = b'0' + (d / 10) as u8;
    date[9] = b'0' + (d % 10) as u8;
    (hh, mm, date)
}

static DESKTOP: IrqMutex<Desktop> = IrqMutex::new(Desktop::new());

/// Redraws screen area `r` from the desktop.
fn redraw(desktop: &Desktop, r: Rect) {
    let Some((sw, sh)) = platform::screen_size() else { return };
    let Some(r) = r.clip(sw.into(), sh.into()) else { return };
    let mut row = Vec::new();
    if row.try_reserve_exact(r.w as usize).is_err() {
        return;
    }
    for y in r.y..r.y + r.h {
        row.clear();
        row.extend((r.x..r.x + r.w).map(|x| desktop.pixel(x, y)));
        platform::screen_write_row(r.x as u32, y as u32, &row);
    }
}

fn redraw_damage(d: &mut Desktop) {
    let damage = core::mem::take(&mut d.damage);
    for r in damage {
        redraw(d, r);
    }
}

/// The desktop on the screen, painted once.
fn ensure_painted(d: &mut Desktop) -> Result<(), NtStatus> {
    if !d.painted
        && let Some((w, h)) = platform::screen_size()
    {
        d.set_screen(w, h)?;
        d.set_minute(platform::system_time() / 600_000_000);
        d.painted = true;
        d.damage.clear();
        redraw(d, Rect { x: 0, y: 0, w: w.into(), h: h.into() });
    }
    Ok(())
}

/// Runs `f` on the desktop and redraws what it changed; STATUS_NOT_SUPPORTED
/// without a screen.
pub fn with<R>(
    f: impl FnOnce(&mut Desktop) -> Result<(R, Option<Rect>), NtStatus>,
) -> Result<R, NtStatus> {
    if platform::screen_size().is_none() {
        return Err(NtStatus::NOT_SUPPORTED);
    }
    let mut d = DESKTOP.lock();
    ensure_painted(&mut d)?;
    let (r, area) = f(&mut d)?;
    if let Some(area) = area {
        d.add_damage(area);
    }
    redraw_damage(&mut d);
    Ok(r)
}

/// A process ended: its windows go.
pub fn process_ended(owner: u64) {
    if platform::screen_size().is_none() {
        return;
    }
    let mut d = DESKTOP.lock();
    if !d.painted {
        return;
    }
    d.destroy_owned(owner);
    redraw_damage(&mut d);
}

/// Mouse input: the pointer moved by (dx, dy) with `buttons` held (bit 0
/// left, 1 right, 2 middle). Nothing before the desktop is on the screen.
/// Returns the program the start menu asked to start, if any.
pub fn pointer_event(dx: i64, dy: i64, buttons: u8) -> Option<&'static Program> {
    // Which programs exist (outside the desktop's lock: the file system).
    let mut available: [usize; 8] = [0; 8];
    let mut n = 0;
    if buttons & 1 != 0 {
        let mut seen: [&str; 8] = [""; 8];
        for (i, p) in PROGRAMS.iter().enumerate() {
            if n < available.len()
                && !seen[..n].contains(&p.name)
                && super::process::program_exists(p.path)
            {
                available[n] = i;
                seen[n] = p.name;
                n += 1;
            }
        }
    }
    let mut d = DESKTOP.lock();
    if !d.painted {
        return None;
    }
    if buttons & 1 != 0 && !d.menu_open() {
        d.set_programs(&available[..n]);
    }
    let damage = d.pointer(dx, dy, buttons);
    for r in damage {
        redraw(&d, r);
    }
    let launch = d.take_launch();
    let wakes = d.take_wakes();
    drop(d);
    signal(wakes);
    launch
}

/// Signals input events (outside the desktop's lock).
fn signal(events: Vec<ObjectRef>) {
    for e in events {
        super::wait::set_signal(&e, 1);
    }
}

/// Keyboard input: virtual key `vk` (scan code `scan`, E0-prefixed if
/// `extended`) went down or up.
pub fn key_event(vk: u8, scan: u8, extended: bool, down: bool) {
    let mut d = DESKTOP.lock();
    if !d.painted {
        return;
    }
    d.key(vk, scan, extended, down);
    let wakes = d.take_wakes();
    drop(d);
    signal(wakes);
}

/// GalaWindowListen: `owner`'s input event.
pub fn listen(owner: u64, event: ObjectRef) -> Result<(), NtStatus> {
    let mut d = DESKTOP.lock();
    d.listen(owner, event)?;
    let wakes = d.take_wakes();
    drop(d);
    signal(wakes);
    Ok(())
}

/// The clock: called now and then (the input thread).
pub fn tick() {
    let mut d = DESKTOP.lock();
    if !d.painted {
        return;
    }
    d.set_minute(platform::system_time() / 600_000_000);
    redraw_damage(&mut d);
}

/// Takes input waiting for `owner`'s windows into `out`; how many.
pub fn take_input(owner: u64, out: &mut [InputEvent]) -> usize {
    DESKTOP.lock().take_input(owner, out)
}

pub fn has_input(owner: u64) -> bool {
    DESKTOP.lock().has_input(owner)
}

#[cfg(test)]
mod tests;
