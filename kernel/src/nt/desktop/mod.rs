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
use paint::{Canvas, Font, Picture, Wallpaper, blend, over, round_coverage2, shadow_alpha};

/// Frame metrics (what user32's GetSystemMetrics reports): Windows 11's
/// (a 1-pixel border, a 32-pixel caption, 46 x 32 caption buttons, 8-pixel
/// rounded corners).
pub const BORDER: u32 = 1;
pub const CAPTION: u32 = 32;
/// Corner radii of a window: above and below.
const RADIUS: i64 = 8;
const RADIUS_BOTTOM: i64 = 8;
/// The caption buttons' width.
const CAPTION_BUTTON: i64 = 46;
/// Soft shadows (Windows 11's are wide and light): reach, drop, darkness.
const SHADOW_REACH: i64 = 26;
const SHADOW_DROP: i64 = 6;
const SHADOW_ACTIVE: i64 = 80;
const SHADOW_INACTIVE: i64 = 45;
/// Without a known screen (host tests of the windows alone).
pub const DESKTOP_COLOR: u32 = 0x00_3a_6e_a5;

/// The light theme, after Windows 11's: the caption takes the window's
/// (Mica-like) background, the title is near-black on the active window
/// and grey on the others; hovered buttons darken a little, close turns
/// red.
const CAPTION_ACTIVE: u32 = 0x00_ee_f2_f7;
const CAPTION_INACTIVE: u32 = 0x00_f7_f8_fa;
const OUTLINE_ACTIVE: u32 = 0x00_9c_a3_ad;
const OUTLINE_INACTIVE: u32 = 0x00_cd_d1_d6;
const TITLE_ACTIVE: u32 = 0x00_1b_1b_1b;
const TITLE_INACTIVE: u32 = 0x00_8a_8a_8a;
const BUTTON_HOVER: u32 = 0x00_df_e3_e9;
const CLOSE_HOVER: u32 = 0x00_c4_2b_1c;
/// Windows' default accent (blue): the active app's indicator.
const ACCENT: u32 = 0x00_00_5f_b8;
const TEXT: u32 = 0x00_1b_1b_1b;
const TEXT_DIM: u32 = 0x00_5d_5d_5d;

/// The taskbar: the bottom rows of the screen, translucent light; the
/// start button and the windows' buttons centred, the clock at the right.
pub const TASKBAR: u32 = 48;
const TASKBAR_COLOR: u32 = 0x00_ee_f1_f6;
const TASKBAR_ALPHA: u32 = 236;
const TASKBAR_LINE: u32 = 0x00_d7_dc_e3;
/// A taskbar button (the start button too) and the gap between them.
const TASK_BUTTON: i64 = 44;
const TASK_GAP: i64 = 4;

/// The start menu: centred above the taskbar.
pub const MENU_W: u32 = 600;
pub const MENU_H: u32 = 640;
/// The pinned programs' grid.
const GRID_X: i64 = 32;
const GRID_Y: i64 = 116;
const TILE_W: i64 = 89;
const TILE_H: i64 = 84;
const GRID_COLUMNS: usize = 6;
const MENU_COLOR: u32 = 0x00_f2_f4_f8;
const MENU_BORDER: u32 = 0x00_d2_d6_dd;

/// The start button (where Windows has its logo) and the user's picture:
/// Gala's fortress (paint::FORTRESS24), in a blue like Windows' logo.
const LOGO_TOP: u32 = 0x00_2e_9b_f0;
const LOGO_BOTTOM: u32 = 0x00_00_67_c0;

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
pub const WM_SYSCOMMAND: u32 = 0x0112;
pub const SC_CLOSE: u64 = 0xf060;
pub const SC_MAXIMIZE: u64 = 0xf030;
pub const SC_RESTORE: u64 = 0xf120;
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
    Maximize,
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
    /// It fills the work area (square corners, no shadow; the maximize
    /// button shows "restore").
    maximized: bool,
    /// The caption bar's pixels (w x CAPTION) and the state they show.
    caption: Option<Canvas>,
    caption_state: Option<(bool, Option<CaptionButton>, bool)>,
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
        let (top, bottom) = if self.maximized { (0, 0) } else { (RADIUS, RADIUS_BOTTOM) };
        round_coverage2(lx, ly, self.w.into(), self.h.into(), top, bottom)
    }

    /// The colour of window point (lx, ly) inside its shape.
    fn body(&self, lx: i64, ly: i64, active: bool) -> u32 {
        let (w, h) = (i64::from(self.w), i64::from(self.h));
        let b = i64::from(BORDER);
        let (rt, rb) = if self.maximized { (0, 0) } else { (RADIUS - 1, RADIUS_BOTTOM - 1) };
        let inner = round_coverage2(lx - b, ly - b, w - 2 * b, h - 2 * b, rt, rb);
        let outline = if active { OUTLINE_ACTIVE } else { OUTLINE_INACTIVE };
        if inner == 0 {
            return outline;
        }
        let c = if ly < b + i64::from(CAPTION) {
            self.caption.as_ref().map_or(CAPTION_INACTIVE, |c| c.get(lx, ly - b) & 0x00ff_ffff)
        } else {
            let (cw, _) = self.client_size();
            let (cx, cy) = (lx - b, ly - b - i64::from(CAPTION));
            self.client.get((cy * i64::from(cw) + cx) as usize).copied().unwrap_or(0x00ff_ffff)
        };
        blend(outline, c, inner)
    }

    /// Draws the caption bar for its state: the background, the title at
    /// the left, minimize, maximize (or restore) and close at the right
    /// with thin glyphs.
    fn render_caption(&mut self, active: bool, hover: Option<CaptionButton>) {
        let state = (active, hover, self.maximized);
        if self.caption_state == Some(state) && self.caption.is_some() {
            return;
        }
        let (w, h) = (i64::from(self.w), i64::from(CAPTION));
        let bg = if active { CAPTION_ACTIVE } else { CAPTION_INACTIVE };
        let Some(mut c) = Canvas::new(w, h, 0xff00_0000 | bg) else { return };
        let close_x = w - CAPTION_BUTTON - 1;
        let max_x = close_x - CAPTION_BUTTON;
        let min_x = max_x - CAPTION_BUTTON;
        let glyph = if active { TITLE_ACTIVE } else { TITLE_INACTIVE };
        let fill = |c: &mut Canvas, x0: i64, color: u32| {
            for y in 0..h {
                for x in x0..x0 + CAPTION_BUTTON {
                    c.put(x, y, color, 255);
                }
            }
        };
        match hover {
            Some(CaptionButton::Close) => fill(&mut c, close_x, CLOSE_HOVER),
            Some(CaptionButton::Maximize) => fill(&mut c, max_x, BUTTON_HOVER),
            Some(CaptionButton::Minimize) => fill(&mut c, min_x, BUTTON_HOVER),
            None => {}
        }
        let cy = h / 2;
        // Close: an X of ten pixels, anti-aliased.
        let close_glyph = if hover == Some(CaptionButton::Close) { 0x00ff_ffff } else { glyph };
        let cx = close_x + CAPTION_BUTTON / 2;
        for t in -5..5 {
            c.put(cx + t, cy + t, close_glyph, 230);
            c.put(cx + t, cy - t - 1, close_glyph, 230);
            c.put(cx + t + 1, cy + t, close_glyph, 60);
            c.put(cx + t + 1, cy - t - 1, close_glyph, 60);
        }
        // Maximize: a ten-pixel square; restore: two overlapping ones.
        let mx = max_x + CAPTION_BUTTON / 2 - 5;
        if self.maximized {
            c.round_outline(mx + 2, cy - 5, 8, 8, 1, glyph, 255);
            for y in cy - 3..cy + 5 {
                for x in mx..mx + 8 {
                    c.put(x, y, bg, 255);
                }
            }
            c.round_outline(mx, cy - 3, 8, 8, 1, glyph, 255);
        } else {
            c.round_outline(mx, cy - 5, 10, 10, 1, glyph, 255);
        }
        // Minimize: a ten-pixel line.
        let nx = min_x + CAPTION_BUTTON / 2 - 5;
        for x in nx..nx + 10 {
            c.put(x, cy, glyph, 255);
        }
        // The title, 12 pixels, at the left.
        let title = char::decode_utf16(self.title.iter().copied()).map(|c| c.unwrap_or('?'));
        c.text(12, (h - Font::Ui12.height()) / 2, title, glyph, Font::Ui12, min_x - 8);
        self.caption = Some(c);
        self.caption_state = Some(state);
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
    /// What was typed in the start menu (ASCII, filters the programs).
    search: [u8; 32],
    search_len: usize,
    /// The programs started from the menu, most recent first (PROGRAMS
    /// indices; usize::MAX: none).
    recent: [usize; 4],
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
            search: [0; 32],
            search_len: 0,
            recent: [usize::MAX; 4],
            listeners: Vec::new(),
            woken: Vec::new(),
            alt: false,
        }
    }

    /// The screen is w x h: lays out the wallpaper and the taskbar and
    /// puts the pointer in the middle.
    pub fn set_screen(&mut self, w: u32, h: u32) -> Result<(), NtStatus> {
        let (w, h) = (i64::from(w), i64::from(h));
        let wallpaper = Wallpaper::new(w, h, PICTURE.lock().as_ref());
        self.wallpaper = Some(wallpaper.ok_or(NtStatus::NO_MEMORY)?);
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
            maximized: false,
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
        let work = (self.screen.0, self.screen.1 - i64::from(TASKBAR));
        let win = self.find(id, owner)?;
        win.maximized = (x, y) == (0, 0) && (i64::from(w), i64::from(h)) == work && work.0 > 0;
        win.caption_state = None;
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
        // The Windows key opens and closes the start menu.
        if vk == 0x5b || vk == 0x5c {
            if down {
                if self.menu.is_some() {
                    self.close_menu();
                } else {
                    self.open_menu();
                }
                self.refresh();
            }
            return;
        }
        // With the menu open, typing searches its programs: Enter starts
        // the first, Escape closes it, Backspace deletes.
        if self.menu.is_some() {
            if down {
                match vk {
                    0x1b => self.close_menu(),
                    0x0d => {
                        self.launch = self.listed().first().copied();
                        self.close_menu();
                    }
                    0x08 => self.search_len = self.search_len.saturating_sub(1),
                    b'A'..=b'Z' | b'0'..=b'9' | b' ' if self.search_len < self.search.len() => {
                        self.search[self.search_len] = vk.to_ascii_lowercase();
                        self.search_len += 1;
                    }
                    0xbd if self.search_len < self.search.len() => {
                        self.search[self.search_len] = b'-';
                        self.search_len += 1;
                    }
                    _ => {}
                }
                self.menu_dirty = true;
                self.refresh();
            }
            return;
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

    /// The centred group of taskbar buttons: the start button and one per
    /// window the programs show; its left edge.
    fn group_x(&self, windows: usize) -> i64 {
        let n = 1 + windows as i64;
        (self.screen.0 - (n * TASK_BUTTON + (n - 1) * TASK_GAP)) / 2
    }

    fn visible_windows(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = Vec::new();
        for w in self.windows.iter().filter(|w| w.visible) {
            let _ = ids.try_push(w.id);
        }
        ids.sort_unstable();
        ids
    }

    /// The start button (the first of the centred group).
    fn start_rect(&self) -> Rect {
        let top = self.screen.1 - i64::from(TASKBAR) + 4;
        Rect { x: self.group_x(self.visible_windows().len()), y: top, w: TASK_BUTTON, h: 40 }
    }

    /// The taskbar's window buttons (in the order the windows were made)
    /// and their rectangles, after the start button.
    fn tasks(&self) -> Vec<(u32, Rect)> {
        let ids = self.visible_windows();
        let x0 = self.group_x(ids.len()) + TASK_BUTTON + TASK_GAP;
        let top = self.screen.1 - i64::from(TASKBAR) + 4;
        let mut out = Vec::new();
        for (i, id) in ids.into_iter().enumerate() {
            let _ = out.try_push((
                id,
                Rect { x: x0 + i as i64 * (TASK_BUTTON + TASK_GAP), y: top, w: TASK_BUTTON, h: 40 },
            ));
        }
        out
    }

    fn menu_rect(&self) -> Rect {
        let w = i64::from(MENU_W).min(self.screen.0 - 24);
        let h = i64::from(MENU_H).min(self.screen.1 - i64::from(TASKBAR) - 24);
        Rect { x: (self.screen.0 - w) / 2, y: self.screen.1 - i64::from(TASKBAR) - 12 - h, w, h }
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

    /// A pinned program's tile in the grid.
    fn menu_item_rect(&self, i: usize) -> Rect {
        let m = self.menu_rect();
        let (col, row) = ((i % GRID_COLUMNS) as i64, (i / GRID_COLUMNS) as i64);
        Rect {
            x: m.x + GRID_X + col * TILE_W,
            y: m.y + GRID_Y + row * TILE_H,
            w: TILE_W - 4,
            h: TILE_H - 4,
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
            for (k, _) in self.listed().iter().enumerate() {
                if self.menu_item_rect(k).contains(x, y) {
                    return Hit::MenuItem(k);
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
                    return Hit::Button(w.id, CaptionButton::Maximize);
                }
                if ly >= 1 && lx >= close_x - 2 * CAPTION_BUTTON && lx < close_x - CAPTION_BUTTON {
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
                    self.open_menu();
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
            // As Windows: the caption buttons are WM_SYSCOMMANDs to the
            // program (DefWindowProc closes, maximizes or restores).
            (Hit::Button(a, CaptionButton::Close), Hit::Button(b, CaptionButton::Close))
                if a == b =>
            {
                self.send(a, WM_SYSCOMMAND, SC_CLOSE, 0);
            }
            (Hit::Button(a, CaptionButton::Maximize), Hit::Button(b, CaptionButton::Maximize))
                if a == b =>
            {
                let maximized = self.window(a).is_some_and(|w| w.maximized);
                self.send(a, WM_SYSCOMMAND, if maximized { SC_RESTORE } else { SC_MAXIMIZE }, 0);
            }
            (Hit::Button(a, CaptionButton::Minimize), Hit::Button(b, CaptionButton::Minimize))
                if a == b =>
            {
                self.minimize(a);
            }
            (Hit::MenuItem(a), Hit::MenuItem(b)) if a == b => {
                self.launch = self.listed().get(a).copied();
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
        self.search_len = 0;
        if self.menu.take().is_some() {
            let b = self.menu_bounds();
            self.add_damage(b);
            self.taskbar_dirty = true;
        }
    }

    /// The program the start menu asked to start, once (it becomes the
    /// most recent).
    pub fn take_launch(&mut self) -> Option<&'static Program> {
        let i = self.launch.take()?;
        if let Some(at) = self.recent.iter().position(|&r| r == i) {
            self.recent.copy_within(0..at, 1);
        } else {
            self.recent.copy_within(0..3, 1);
        }
        self.recent[0] = i;
        PROGRAMS.get(i)
    }

    fn open_menu(&mut self) {
        self.search_len = 0;
        self.menu_dirty = true;
        self.menu = Canvas::new(1, 1, 0); // rendered by refresh
        let b = self.menu_bounds();
        self.add_damage(b);
        self.taskbar_dirty = true;
    }

    /// The programs the menu shows: the available ones matching what was
    /// typed (case-insensitively).
    fn listed(&self) -> Vec<usize> {
        let q = &self.search[..self.search_len];
        let mut out = Vec::new();
        for &p in &self.programs {
            let Some(prog) = PROGRAMS.get(p) else { continue };
            let name = prog.name.as_bytes();
            let matches = q.is_empty()
                || name
                    .windows(q.len())
                    .any(|w| w.iter().zip(q).all(|(a, b)| a.eq_ignore_ascii_case(b)));
            if matches {
                let _ = out.try_push(p);
            }
        }
        out
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

    /// The taskbar: translucent light with a line on top; the start
    /// button (the camel) and the windows' buttons centred, each an icon
    /// with Windows' indicator under it (the active window: a long accent
    /// pill on a lit button; the others: a short grey one); the clock and
    /// date at the right.
    fn render_taskbar(&self) -> Option<Canvas> {
        let (w, h) = (self.screen.0, i64::from(TASKBAR));
        let mut c = Canvas::new(w, h, 0)?;
        for y in 0..h {
            let (row, a) =
                if y == 0 { (TASKBAR_LINE, 255) } else { (TASKBAR_COLOR, TASKBAR_ALPHA) };
            for x in 0..w {
                c.put(x, y, row, a);
            }
        }
        let top = self.screen.1 - h;
        let s = self.start_rect();
        let glow = if self.menu.is_some() {
            170
        } else if self.hover == Hit::Start {
            130
        } else {
            0
        };
        if glow > 0 {
            c.round_rect(s.x, s.y - top, s.w, s.h, 4, 0x00ff_ffff, 0x00ff_ffff, glow);
        }
        c.mask(
            s.x + (s.w - 24) / 2,
            s.y - top + 8,
            paint::FORTRESS24,
            24,
            LOGO_TOP,
            LOGO_BOTTOM,
            255,
        );
        for (id, r) in self.tasks() {
            let Some(win) = self.window(id) else { continue };
            let (x, y) = (r.x, r.y - top);
            let active = self.active == Some(id) && win.shown();
            let alpha = if active {
                190
            } else if self.hover == Hit::Task(id) {
                130
            } else {
                0
            };
            if alpha > 0 {
                c.round_rect(x, y, r.w, r.h, 4, 0x00ff_ffff, 0x00ff_ffff, alpha);
            }
            // The window's icon: a tile with its initial (programs' own
            // icons come later).
            let initial = char::decode_utf16(win.title.iter().copied())
                .next()
                .and_then(|c| c.ok())
                .unwrap_or('?');
            let (c0, c1) = tile_colors(&win.title);
            c.round_rect(x + 10, y + 8, 24, 24, 5, c0, c1, 255);
            let iw = Font::Ui12Semibold.measure(core::iter::once(initial));
            c.text(
                x + 22 - iw / 2,
                y + 12,
                core::iter::once(initial),
                0x00ff_ffff,
                Font::Ui12Semibold,
                x + 34,
            );
            if active {
                c.round_rect(x + r.w / 2 - 8, y + 36, 16, 3, 1, ACCENT, ACCENT, 255);
            } else if !win.minimized {
                c.round_rect(x + r.w / 2 - 3, y + 36, 6, 3, 1, 0x00_8a_8a_8a, 0x00_8a_8a_8a, 255);
            }
        }
        // The clock: the time over the date, right-aligned.
        let (time, tn, date, dn) = clock(self.minute);
        let tw = Font::Ui12.measure(time[..tn].iter().map(|&b| b as char));
        let dw = Font::Ui12.measure(date[..dn].iter().map(|&b| b as char));
        let right = w - 14;
        c.text(right - tw, 7, time[..tn].iter().map(|&b| b as char), TEXT, Font::Ui12, w);
        c.text(right - dw, 25, date[..dn].iter().map(|&b| b as char), TEXT, Font::Ui12, w);
        Some(c)
    }

    /// The start menu, after Windows 11's: a search box at the top, the
    /// pinned programs in a grid, the recently started ones, the user at
    /// the bottom.
    fn render_menu(&self) -> Option<Canvas> {
        let m = self.menu_rect();
        let (w, h) = (m.w, m.h);
        let mut c = Canvas::new(w, h, 0)?;
        c.round_rect(0, 0, w, h, 8, MENU_COLOR, MENU_COLOR, 255);
        // The foot: a little darker, under a line.
        let foot = h - 64;
        for y in foot..h - 1 {
            for x in 1..w - 1 {
                if crate::nt::desktop::paint::round_coverage(x, y, w, h, 8) == 255 {
                    c.put(x, y, 0x00_e7_ea_ef, 255);
                }
            }
        }
        for x in 0..w {
            c.put(x, foot, MENU_BORDER, 255);
        }
        c.round_outline(0, 0, w, h, 8, MENU_BORDER, 255);
        // The search box.
        c.round_rect(32, 24, w - 64, 36, 18, 0x00ff_ffff, 0x00ff_ffff, 255);
        c.round_outline(32, 24, w - 64, 36, 18, 0x00_d9_dd_e3, 255);
        // A magnifier: a ring and a handle.
        for a in 0..64 {
            let (dx, dy) = (ring(a, 6), ring(a + 16, 6));
            c.put(54 + dx, 40 + dy, TEXT_DIM, 200);
        }
        for t in 0..5 {
            c.put(59 + t, 45 + t, TEXT_DIM, 230);
        }
        if self.search_len == 0 {
            c.text(72, 34, "Type to search programs".chars(), TEXT_DIM, Font::Ui12, w - 48);
        } else {
            let end = c.text(
                72,
                34,
                self.search[..self.search_len].iter().map(|&b| b as char),
                TEXT,
                Font::Ui12,
                w - 48,
            );
            for y in 33..49 {
                c.put(end + 1, y, TEXT, 255);
            }
        }
        // Pinned programs.
        c.text(52, 84, "Pinned".chars(), TEXT, Font::Ui14Semibold, w);
        let listed = self.listed();
        for (k, &p) in listed.iter().enumerate() {
            let Some(program) = PROGRAMS.get(p) else { continue };
            let r = self.menu_item_rect(k).offset(-m.x, -m.y);
            if r.y + r.h > foot - 120 {
                break;
            }
            let lit = match (self.hover, self.pressed) {
                (Hit::MenuItem(a), Hit::MenuItem(b)) if a == k && b == k => 210,
                (Hit::MenuItem(a), _) if a == k => 170,
                _ => 0,
            };
            if lit > 0 {
                c.round_rect(r.x, r.y, r.w, r.h, 4, 0x00ff_ffff, 0x00ff_ffff, lit);
            }
            let (ix, iy) = (r.x + (r.w - 32) / 2, r.y + 10);
            c.round_rect(ix, iy, 32, 32, 6, program.colors.0, program.colors.1, 255);
            let letter = program.name.chars().next().unwrap_or('?');
            let lw = Font::Ui14Semibold.measure(core::iter::once(letter));
            c.text(
                ix + 16 - lw / 2,
                iy + 7,
                core::iter::once(letter),
                0x00ff_ffff,
                Font::Ui14Semibold,
                ix + 32,
            );
            let nw = Font::Ui12.measure(program.name.chars()).min(r.w - 8);
            c.text(
                r.x + (r.w - nw) / 2,
                r.y + 50,
                program.name.chars(),
                TEXT,
                Font::Ui12,
                r.x + r.w - 4,
            );
        }
        if listed.is_empty() {
            let text =
                if self.search_len == 0 { "No programs installed" } else { "No programs match" };
            c.text(52, GRID_Y + 10, text.chars(), TEXT_DIM, Font::Ui12, w - 32);
        }
        // Recently started programs.
        let rec_y = foot - 112;
        c.text(52, rec_y, "Recommended".chars(), TEXT, Font::Ui14Semibold, w);
        let mut y = rec_y + 32;
        let mut any = false;
        for &p in self.recent.iter().filter(|&&p| p != usize::MAX).take(2) {
            let Some(program) = PROGRAMS.get(p) else { continue };
            any = true;
            c.round_rect(52, y, 32, 32, 6, program.colors.0, program.colors.1, 255);
            c.text(96, y, program.name.chars(), TEXT, Font::Ui12, w - 40);
            c.text(96, y + 16, "Recently started".chars(), TEXT_DIM, Font::Ui12, w - 40);
            y += 40;
        }
        if !any {
            c.text(
                52,
                y,
                "Programs you start will show up here.".chars(),
                TEXT_DIM,
                Font::Ui12,
                w - 40,
            );
        }
        // The user.
        c.round_rect(52, foot + 16, 32, 32, 16, LOGO_TOP, LOGO_BOTTOM, 255);
        c.mask(56, foot + 20, paint::FORTRESS24, 24, 0x00ff_ffff, 0x00ff_ffff, 255);
        c.text(96, foot + 24, "gala".chars(), TEXT, Font::Ui12, w - 40);
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
            if cov < 255 && !w.maximized {
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

/// A point on a circle of radius r at a 64th-of-a-turn step: its x (its y
/// is `ring(step + 16, r)`).
fn ring(step: i64, r: i64) -> i64 {
    // cos by a quarter-wave table of 17 points (x1024).
    const COS: [i64; 17] =
        [1024, 1019, 1004, 980, 946, 903, 851, 792, 724, 650, 569, 483, 392, 297, 200, 100, 0];
    let s = step.rem_euclid(64);
    let v = match s {
        0..=16 => COS[s as usize],
        17..=32 => -COS[(32 - s) as usize],
        33..=48 => -COS[(s - 32) as usize],
        _ => COS[(64 - s) as usize],
    };
    (v * r + 512).div_euclid(1024)
}

/// A program's tile colours from its title (a stable hue).
fn tile_colors(title: &[u16]) -> (u32, u32) {
    const PALETTE: [(u32, u32); 6] = [
        (0x00_4c_9a_ff, 0x00_1f_6f_e0),
        (0x00_2e_b8_8a, 0x00_17_8a_63),
        (0x00_f0_8a_3c, 0x00_d0_63_1d),
        (0x00_a8_6a_f0, 0x00_7c_3f_d6),
        (0x00_e8_4c_6a, 0x00_c0_2a_4a),
        (0x00_3c_b4_d8, 0x00_1c_8a_b4),
    ];
    let h = title.iter().fold(7u32, |a, &c| a.wrapping_mul(31).wrapping_add(u32::from(c)));
    PALETTE[(h % PALETTE.len() as u32) as usize]
}

/// The clock as Windows shows it in the US: "10:52 PM" and "11/1/2026"
/// (and their lengths) for `minute` (minutes since 1601-01-01 UTC). The
/// date: the days-to-civil algorithm from Howard Hinnant's
/// "chrono-Compatible Low-Level Date Algorithms" (public domain).
fn clock(minute: u64) -> ([u8; 8], usize, [u8; 10], usize) {
    let hh = (minute / 60 % 24) as u8;
    let mm = (minute % 60) as u8;
    let z = (minute / (60 * 24)) as i64 - 134_774 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = (yoe + era * 400 + i64::from(m <= 2)).clamp(0, 9999) as u32;
    let mut time = [0u8; 8];
    let mut tn = 0;
    let h12 = match hh % 12 {
        0 => 12,
        h => h,
    };
    let push = |buf: &mut [u8], n: &mut usize, b: u8| {
        buf[*n] = b;
        *n += 1;
    };
    if h12 >= 10 {
        push(&mut time, &mut tn, b'1');
    }
    push(&mut time, &mut tn, b'0' + h12 % 10);
    for b in [b':', b'0' + mm / 10, b'0' + mm % 10, b' ', if hh < 12 { b'A' } else { b'P' }, b'M'] {
        push(&mut time, &mut tn, b);
    }
    let mut date = [0u8; 10];
    let mut dn = 0;
    for v in [m as u32, d as u32] {
        if v >= 10 {
            push(&mut date, &mut dn, b'0' + (v / 10) as u8);
        }
        push(&mut date, &mut dn, b'0' + (v % 10) as u8);
        push(&mut date, &mut dn, b'/');
    }
    for div in [1000, 100, 10, 1] {
        push(&mut date, &mut dn, b'0' + (y / div % 10) as u8);
    }
    (time, tn, date, dn)
}

static DESKTOP: IrqMutex<Desktop> = IrqMutex::new(Desktop::new());

/// The wallpaper picture chosen at boot (`choose_wallpaper`).
static PICTURE: IrqMutex<Option<Picture>> = IrqMutex::new(None);

/// The wallpapers gala ships and their files (THIRD_PARTY.md).
const WALLPAPERS: [(&str, &str); 4] = [
    ("dunes", "\\Windows\\Web\\Wallpaper\\dunes.qoi"),
    ("ribbons", "\\Windows\\Web\\Wallpaper\\ribbons.qoi"),
    ("najdi", "\\Windows\\Web\\Wallpaper\\najdi.qoi"),
    ("night", "\\Windows\\Web\\Wallpaper\\night.qoi"),
];

/// Chooses the desktop's wallpaper at boot: `choice` (the `wallpaper=`
/// boot option: one of WALLPAPERS, or "drawn" for the drawn light and
/// ribbons), else one of the shipped pictures present at random. Without
/// any, the drawn one.
pub fn choose_wallpaper(choice: Option<&str>) {
    if choice == Some("drawn") {
        return;
    }
    let mut present = [("", ""); WALLPAPERS.len()];
    let mut count = 0;
    for (name, path) in WALLPAPERS {
        if choice.is_none_or(|c| c == name) && super::file::lookup(path).is_ok() {
            present[count] = (name, path);
            count += 1;
        }
    }
    if count == 0 {
        return;
    }
    // Random enough: the time's low bits (100 ns units).
    let (name, path) = present[(platform::system_time() / 10 % count as u64) as usize];
    let picture = super::file::lookup(path)
        .and_then(super::file::read_all)
        .ok()
        .and_then(|data| paint::decode_qoi(&data));
    match picture {
        Some(p) => {
            crate::info!("desktop: wallpaper {name} ({} x {})", p.w, p.h);
            *PICTURE.lock() = Some(p);
        }
        None => crate::warn!("desktop: cannot load wallpaper {name}"),
    }
}

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
