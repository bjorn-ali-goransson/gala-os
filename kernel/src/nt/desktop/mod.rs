//! The desktop of shasha, gala's graphical shell: top-level windows of
//! Win32 programs, composited on the screen, with the shell's taskbar and
//! start menu and the mouse.
//!
//! The kernel keeps each window's frame (rounded corners, a shaded caption
//! with the title and minimize/close buttons, drawn here) and a copy of its
//! client area, which the program's user32/gdi32 draw in their own memory
//! and present (`present`). Every pixel is composed in layers: the
//! wallpaper, the windows bottom to top (each with a soft shadow), the
//! taskbar (translucent), the start menu (with its shadow) and the pointer.
//! Changes collect damage rectangles which `with`/`pointer_event`/`tick`
//! compose and hand to the desktop's presenter (`present`: the screen, a
//! frame in memory, or nothing; the windows and their pixels are kept
//! either way). The window state itself (windows, Z order, focus,
//! capture, client pixels, input for programs) is `nt::winstate`'s, which
//! a bare desktop is alone; this module is the compositor on top.
//!
//! The mouse: a click raises and activates a window, its caption drags
//! it, the caption buttons minimize it or ask it to close (WM_CLOSE to the
//! program), the taskbar's buttons restore, activate or minimize windows,
//! the start button opens the start menu, whose items start programs.
//! Clicks and moves in a client area become input for its program.
//!
//! What the shell does to windows is a named [`Action`] (`act`): the
//! taskbar, the caption buttons and the Snap keys call it, and so does
//! GalaDesktopAction (`nt::action`, the `gala` command): everything shasha
//! does with the mouse can be done from a terminal.
//! Design: docs/ARCHITECTURE.md, "Windows on the screen", "Server and
//! desktop workloads" and "Presenters (B3a)".

mod paint;
mod present;

use super::object::ObjectRef;
use super::platform;
use super::status::NtStatus;
/// The window state's names, as the desktop's (the system calls, the
/// named actions use them).
pub use super::winstate::{
    Action, BORDER, CAPTION, GALA_WM_ACTIVE, GALA_WM_MINIMIZED, GALA_WM_PLACE,
    GALA_WM_POINTER_LEFT, ICON, InputEvent, LISTED_ACTIVE, LISTED_MAXIMIZED, LISTED_MINIMIZED,
    LISTED_SNAPPED, LISTED_TITLE, Listed, Listener, Rect, SC_CLOSE, SC_MAXIMIZE, SC_RESTORE,
    SMALL_ICON, STYLE_FRAMELESS, STYLE_POPUP, SnapTo, TASKBAR, WHEEL_DELTA, WM_CLOSE, WM_KEYDOWN,
    WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_MOVE, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_SYSKEYUP, scale_icon, scale_icon_to,
};
use super::winstate::{CAPTION_BUTTON, Shadow, State, VK_MENU, Window};
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;
use alloc::vec::Vec;
use paint::{Canvas, Font, Picture, Wallpaper, blend, over, round_coverage2, shadow_alpha};
pub use present::{DEFAULT_SIZE, Frame, Kind, MAX_SIDE as MAX_FRAME_SIDE, Presenter};

/// Corner radii of a window (Windows 11's 8 pixels): above and below.
const RADIUS: i64 = 8;
const RADIUS_BOTTOM: i64 = 8;
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

/// The taskbar (TASKBAR rows at the bottom of the screen): translucent
/// light; the start button and the windows' buttons centred, the clock at
/// the right.
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

/// GalaWindowResize's edges (a program's own border: WM_NCHITTEST's
/// HTLEFT..HTBOTTOMRIGHT).
pub const RESIZE_LEFT: u32 = 0x1;
pub const RESIZE_TOP: u32 = 0x2;
pub const RESIZE_RIGHT: u32 = 0x4;
pub const RESIZE_BOTTOM: u32 = 0x8;
/// A resize by a program's own border (`begin_resize`).
#[derive(Clone, Copy)]
struct Resize {
    id: u32,
    /// RESIZE_* bits.
    edges: u32,
    /// The pointer where it began.
    start: (i64, i64),
    /// The rectangle then and now (x, y, w, h).
    from: [i64; 4],
    to: [i64; 4],
}

/// The smallest size a resize gives a window.
const RESIZE_MIN: (i64, i64) = (120, 60);

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

/// The compositor's own per-window data (`Window::chrome`): its caption
/// bar's pixels and what they were drawn for (the window's `look`, active,
/// the button under the pointer, maximized).
#[derive(Default)]
pub struct Chrome {
    caption: Option<Canvas>,
    caption_state: Option<(u32, bool, Option<CaptionButton>, bool)>,
}

impl Window<Chrome> {
    /// How much of window point (lx, ly) its rounded shape covers.
    fn coverage(&self, lx: i64, ly: i64) -> u8 {
        let square = self.square() || self.popup;
        let (top, bottom) = if square { (0, 0) } else { (RADIUS, RADIUS_BOTTOM) };
        round_coverage2(lx, ly, self.w.into(), self.h.into(), top, bottom)
    }

    /// The colour of window point (lx, ly) inside its shape.
    fn body(&self, lx: i64, ly: i64, active: bool) -> u32 {
        if self.popup || self.frameless {
            let (cw, _) = self.client_size();
            return self
                .client
                .get((ly * i64::from(cw) + lx) as usize)
                .map_or(0x00ff_ffff, |p| p & 0x00ff_ffff);
        }
        let (w, h) = (i64::from(self.w), i64::from(self.h));
        let b = i64::from(BORDER);
        let (rt, rb) = if self.square() { (0, 0) } else { (RADIUS - 1, RADIUS_BOTTOM - 1) };
        let inner = round_coverage2(lx - b, ly - b, w - 2 * b, h - 2 * b, rt, rb);
        let outline = if active { OUTLINE_ACTIVE } else { OUTLINE_INACTIVE };
        if inner == 0 {
            return outline;
        }
        let c = if ly < b + i64::from(CAPTION) {
            self.chrome
                .caption
                .as_ref()
                .map_or(CAPTION_INACTIVE, |c| c.get(lx, ly - b) & 0x00ff_ffff)
        } else {
            let (cw, _) = self.client_size();
            let (cx, cy) = (lx - b, ly - b - i64::from(CAPTION));
            // A program's fourth byte (e.g. an alpha it copied in) is not
            // the window's: client pixels are opaque.
            self.client
                .get((cy * i64::from(cw) + cx) as usize)
                .map_or(0x00ff_ffff, |p| p & 0x00ff_ffff)
        };
        blend(outline, c, inner)
    }

    /// Draws the caption bar for its state: the background, the title at
    /// the left, minimize, maximize (or restore) and close at the right
    /// with thin glyphs.
    fn render_caption(&mut self, active: bool, hover: Option<CaptionButton>) {
        let state = (self.look, active, hover, self.maximized);
        if self.chrome.caption_state == Some(state) && self.chrome.caption.is_some() {
            return;
        }
        let (w, h) = (i64::from(self.w), i64::from(CAPTION));
        let bg = if active { CAPTION_ACTIVE } else { CAPTION_INACTIVE };
        let Some(mut c) = Canvas::new(w, h, 0xff00_0000 | bg) else {
            // No caption bar (the plain caption colour shows) rather
            // than one of another size.
            self.chrome.caption = None;
            self.chrome.caption_state = None;
            return;
        };
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
        // The small icon, then the title (12 pixels), at the left.
        let mut text_x = 12;
        if self.small_icon.len() == SMALL_ICON * SMALL_ICON {
            let top = (h - SMALL_ICON as i64) / 2;
            for (k, &p) in self.small_icon.iter().enumerate() {
                let (px, py) = ((k % SMALL_ICON) as i64, (k / SMALL_ICON) as i64);
                c.put(12 + px, top + py, p, p >> 24);
            }
            text_x += SMALL_ICON as i64 + 8;
        }
        let title = char::decode_utf16(self.title.iter().copied()).map(|c| c.unwrap_or('?'));
        c.text(text_x, (h - Font::Ui12.height()) / 2, title, glyph, Font::Ui12, min_x - 8);
        self.chrome.caption = Some(c);
        self.chrome.caption_state = Some(state);
    }
}

/// A session's desktop as shasha shows it: the window state
/// (`nt::winstate`: windows, Z order, focus, capture, client pixels,
/// input for programs) and, on top of it, the compositor's own: the
/// presenter, the wallpaper, the taskbar, the start menu, the pointer and
/// what the mouse and keys are doing.
pub struct Desktop {
    /// The window state (laid out at `s.screen` once live; the desktop
    /// keeps its windows and their pixels from then on, whether or not a
    /// presenter shows it).
    s: State<Chrome>,
    /// What shows it (ROADMAP RS B3a): composed rows go there.
    presenter: Presenter,
    /// Why nothing can ever show it, on a machine where nothing shows
    /// desktops (`bare`: the server profile, no screen and no
    /// `presenter=`): then it is the window state alone, as the server
    /// kernel's (no wallpaper, captions, taskbar or input), shown by none.
    bare: Option<&'static str>,
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
    /// A resize by a program's own border (sent as GALA_WM_PLACE when the
    /// button goes up).
    resize: Option<Resize>,
    /// The minute the clock shows.
    minute: u64,
    /// A program the start menu asked to start (index into PROGRAMS).
    launch: Option<usize>,
    /// What was typed in the start menu (ASCII, filters the programs).
    search: [u8; 32],
    search_len: usize,
    /// The programs started from the menu, most recent first (PROGRAMS
    /// indices; usize::MAX: none).
    recent: [usize; 4],
    /// Alt is held (WM_SYSKEYDOWN).
    alt: bool,
    /// The Windows key is held, and whether a key went with it (then its
    /// release doesn't open the start menu).
    win_key: bool,
    win_used: bool,
}

impl Default for Desktop {
    fn default() -> Self {
        Self::new()
    }
}

/// What the compositor draws outside a window: its soft shadow.
const SHADOW: Shadow = Shadow { reach: SHADOW_REACH, drop: SHADOW_DROP };

impl Desktop {
    pub const fn new() -> Self {
        Desktop {
            s: State::new(SHADOW),
            presenter: Presenter::None,
            bare: None,
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
            resize: None,
            minute: 0,
            launch: None,
            search: [0; 32],
            search_len: 0,
            recent: [usize::MAX; 4],
            alt: false,
            win_key: false,
            win_used: false,
        }
    }

    /// The screen is w x h: lays out the wallpaper and the taskbar and
    /// puts the pointer in the middle.
    pub fn set_screen(&mut self, w: u32, h: u32) -> Result<(), NtStatus> {
        let (w, h) = (i64::from(w), i64::from(h));
        let wallpaper = Wallpaper::new(w, h, PICTURE.lock().as_ref());
        self.wallpaper = Some(wallpaper.ok_or(NtStatus::NO_MEMORY)?);
        self.s.screen = (w, h);
        self.pointer = (w / 2, h / 2);
        self.taskbar_dirty = true;
        self.refresh();
        Ok(())
    }

    /// Shows the desktop by `presenter` from now on: the desktop takes
    /// its size (keeping its windows' places) and is drawn whole there.
    /// Returns the presenter it replaced.
    pub fn set_presenter(&mut self, presenter: Presenter) -> Result<Presenter, NtStatus> {
        if let Some((w, h)) = presenter.size()
            && (i64::from(w), i64::from(h)) != self.s.screen
        {
            self.set_screen(w, h)?;
        }
        let old = core::mem::replace(&mut self.presenter, presenter);
        self.s.damage.clear();
        let (w, h) = self.s.screen;
        redraw(self, Rect { x: 0, y: 0, w, h });
        Ok(old)
    }

    /// The console leaves this desktop (a console switch): buttons still
    /// held go up where the pointer is (a drag or resize ends, the window
    /// under it gets its WM_LBUTTONUP), the Alt and Windows keys count as
    /// released, an open start menu closes. The areas to redraw.
    pub fn leave_console(&mut self) -> Vec<Rect> {
        let mut damage = if self.buttons != 0 { self.pointer(0, 0, 0) } else { Vec::new() };
        (self.alt, self.win_key, self.win_used) = (false, false, false);
        if self.menu.is_some() {
            self.close_menu();
            self.refresh();
            for r in core::mem::take(&mut self.s.damage) {
                let _ = damage.try_push(r);
            }
        }
        damage
    }

    /// Moves the pointer to (x, y) (clamped to the desktop) with
    /// `buttons` held: injected input (GalaSession's SESSION_INPUT), in
    /// the same terms as the mouse's moves. The areas to redraw.
    pub fn pointer_at(&mut self, x: i64, y: i64, buttons: u8) -> Vec<Rect> {
        let (dx, dy) = (x - self.pointer.0, y - self.pointer.1);
        self.pointer(dx, dy, buttons)
    }

    // ---- What programs do (the window state's, then drawn) -------------

    /// A new hidden window (`State::create`).
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &mut self,
        owner: u64,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        title: &[u16],
        style: u32,
    ) -> Result<u32, NtStatus> {
        self.s.create(owner, x, y, w, h, title, style)
    }

    /// Shows or hides a window (shown: on top and active).
    pub fn show(&mut self, id: u32, owner: u64, visible: bool) -> Result<Rect, NtStatus> {
        let r = self.s.show(id, owner, visible)?;
        self.refresh();
        Ok(r)
    }

    /// Shows a window minimized: on the taskbar only.
    pub fn show_minimized(&mut self, id: u32, owner: u64) -> Result<Rect, NtStatus> {
        let r = self.s.show_minimized(id, owner)?;
        self.refresh();
        Ok(r)
    }

    /// Copies client pixels in (`State::present`); the screen area to
    /// redraw.
    pub fn present(
        &mut self,
        id: u32,
        r: Rect,
        pixels: &[u32],
        stride: usize,
    ) -> Result<Rect, NtStatus> {
        self.s.present(id, r, pixels, stride)
    }

    /// Moves and resizes a window (outer rectangle). The area to redraw.
    pub fn place(
        &mut self,
        id: u32,
        owner: u64,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    ) -> Result<Rect, NtStatus> {
        let r = self.s.place(id, owner, x, y, w, h)?;
        self.refresh();
        Ok(r)
    }

    /// `owner`'s window `id`'s outer rectangle (x, y, width, height).
    pub fn outer(&self, id: u32, owner: u64) -> Option<(i32, i32, u32, u32)> {
        self.s.owned(id, owner).map(|w| (w.x, w.y, w.w, w.h))
    }

    /// A window's new title (the caption and the taskbar show it).
    pub fn set_title(&mut self, id: u32, owner: u64, title: &[u16]) -> Result<Rect, NtStatus> {
        let r = self.s.set_title(id, owner, title)?;
        self.refresh();
        Ok(r)
    }

    /// A window's icon, shown on its taskbar button and caption.
    pub fn set_icon(
        &mut self,
        id: u32,
        owner: u64,
        pixels: &[u32],
        size: usize,
    ) -> Result<(), NtStatus> {
        self.s.set_icon(id, owner, pixels, size)?;
        self.refresh();
        Ok(())
    }

    /// Gives `owner`'s window `id` the mouse capture (0: releases the
    /// capture if one of `owner`'s windows has it).
    pub fn capture(&mut self, id: u32, owner: u64) -> Result<(), NtStatus> {
        self.s.capture(id, owner)?;
        if id != 0 {
            // No caption drag or press carries on underneath.
            self.drag = None;
            self.pressed = Hit::Nothing;
        }
        Ok(())
    }

    pub fn destroy(&mut self, id: u32, owner: u64) -> Result<Rect, NtStatus> {
        let r = self.s.destroy(id, owner)?;
        self.forget_gone();
        self.refresh();
        Ok(r)
    }

    /// Removes `owner`'s windows (a process's end).
    pub fn destroy_owned(&mut self, owner: u64) {
        self.s.destroy_owned(owner);
        self.forget_gone();
        self.refresh();
    }

    /// A drag of a window that is gone ends.
    fn forget_gone(&mut self) {
        if self.drag.is_some_and(|d| self.s.window(d.0).is_none()) {
            self.drag = None;
        }
    }

    /// Input waiting for `owner`'s windows, oldest first, at most
    /// `out.len()`; how many.
    pub fn take_input(&mut self, owner: u64, out: &mut [InputEvent]) -> usize {
        self.s.take_input(owner, out)
    }

    pub fn has_input(&self, owner: u64) -> bool {
        self.s.has_input(owner)
    }

    /// Who to tell when input arrives for `owner`.
    pub fn listen(&mut self, owner: u64, listener: Listener) -> Result<(), NtStatus> {
        self.s.listen(owner, listener)
    }

    /// The windows the taskbar shows (in its order).
    pub fn list(&self) -> Vec<Listed> {
        self.s.list()
    }

    /// Does `action` to window `id` (`State::act`: the one path for the
    /// taskbar, the caption buttons, the Snap keys and
    /// GalaDesktopAction), then draws what changed.
    pub fn act(&mut self, id: u32, action: Action) -> Result<(), NtStatus> {
        self.s.act(id, action)?;
        self.refresh();
        Ok(())
    }

    /// A key went down or up: virtual key `vk`, `scan` code, `extended`
    /// (E0-prefixed). The shell's keys (the Windows key, Snap, the start
    /// menu's search) are the shell's; the others go to the active window
    /// (`State::key_to_active`).
    pub fn key(&mut self, vk: u8, scan: u8, extended: bool, down: bool) {
        if vk == VK_MENU {
            self.alt = down;
        }
        // The Windows key, pressed and released alone, opens and closes
        // the start menu; with the arrows it snaps the active window.
        if vk == 0x5b || vk == 0x5c {
            if down {
                (self.win_key, self.win_used) = (true, false);
            } else if core::mem::take(&mut self.win_key) && !self.win_used {
                if self.menu.is_some() {
                    self.close_menu();
                } else {
                    self.open_menu();
                }
                self.refresh();
            }
            return;
        }
        if self.win_key && (0x25..=0x28).contains(&vk) {
            self.win_used = true;
            let to = match vk {
                0x25 => SnapTo::Left,
                0x26 => SnapTo::Up,
                0x27 => SnapTo::Right,
                _ => SnapTo::Down,
            };
            if down && let Some(id) = self.s.active {
                let _ = self.act(id, Action::Snap(to));
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
        self.s.key_to_active(vk, scan, extended, down, self.alt);
    }

    /// The wheel turned `notches` (positive away from the user): a
    /// WM_MOUSEWHEEL to the window under the pointer (Windows' "scroll
    /// inactive windows when hovering"), else to the capture or the active
    /// one: wParam's high word the distance (WHEEL_DELTA a notch), lParam
    /// the client position (user32 makes it the screen position).
    pub fn wheel(&mut self, notches: i64) {
        let (x, y) = self.pointer;
        let id = match (self.s.captured, self.hit(x, y)) {
            (Some(id), _) => id,
            (None, Hit::Client(id)) => id,
            _ => match self.s.active {
                Some(id) => id,
                None => return,
            },
        };
        let delta = (notches * WHEEL_DELTA).clamp(-0x8000, 0x7fff) as i16 as u16;
        let lp = self.s.client_lparam(id, x, y);
        self.s.send(id, WM_MOUSEWHEEL, u64::from(delta) << 16, lp);
    }

    fn take_wakes(&mut self) -> Vec<Listener> {
        self.s.take_wakes()
    }

    // ---- The shell's layout --------------------------------------------

    fn taskbar_rect(&self) -> Rect {
        let h = i64::from(TASKBAR);
        Rect { x: 0, y: self.s.screen.1 - h, w: self.s.screen.0, h }
    }

    /// The centred group of taskbar buttons: the start button and one per
    /// window the programs show; its left edge.
    fn group_x(&self, windows: usize) -> i64 {
        let n = 1 + windows as i64;
        (self.s.screen.0 - (n * TASK_BUTTON + (n - 1) * TASK_GAP)) / 2
    }

    /// The start button (the first of the centred group).
    fn start_rect(&self) -> Rect {
        let top = self.s.screen.1 - i64::from(TASKBAR) + 4;
        Rect { x: self.group_x(self.s.visible_windows().len()), y: top, w: TASK_BUTTON, h: 40 }
    }

    /// The taskbar's window buttons (in the order the windows were made)
    /// and their rectangles, after the start button.
    fn tasks(&self) -> Vec<(u32, Rect)> {
        let ids = self.s.visible_windows();
        let x0 = self.group_x(ids.len()) + TASK_BUTTON + TASK_GAP;
        let top = self.s.screen.1 - i64::from(TASKBAR) + 4;
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
        let w = i64::from(MENU_W).min(self.s.screen.0 - 24);
        let h = i64::from(MENU_H).min(self.s.screen.1 - i64::from(TASKBAR) - 24);
        Rect {
            x: (self.s.screen.0 - w) / 2,
            y: self.s.screen.1 - i64::from(TASKBAR) - 12 - h,
            w,
            h,
        }
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
        if self.s.screen.0 == 0 {
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
        for w in self.s.windows.iter().rev().filter(|w| w.shown()) {
            let (lx, ly) = (x - i64::from(w.x), y - i64::from(w.y));
            if !w.rect().contains(x, y) || w.coverage(lx, ly) < 128 {
                continue;
            }
            if w.popup || w.frameless {
                return Hit::Client(w.id);
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
        if self.s.screen.0 == 0 {
            return Vec::new();
        }
        let old_cursor = self.cursor_rect();
        self.pointer.0 = (self.pointer.0 + dx).clamp(0, self.s.screen.0 - 1);
        self.pointer.1 = (self.pointer.1 + dy).clamp(0, self.s.screen.1 - 1);
        let (x, y) = self.pointer;
        if old_cursor != self.cursor_rect() {
            self.s.add_damage(old_cursor);
            let c = self.cursor_rect();
            self.s.add_damage(c);
        }
        // The capture gets every mouse message, in its client coordinates.
        if let Some(id) = self.s.captured
            && self.s.window(id).is_some()
        {
            let keys = u64::from(buttons & 1) | u64::from(buttons & 2);
            let lp = self.s.client_lparam(id, x, y);
            let pressed = buttons & !self.buttons;
            let released = self.buttons & !buttons;
            self.buttons = buttons;
            self.s.pointer_over(Some(id));
            if (dx, dy) != (0, 0) {
                self.s.send(id, WM_MOUSEMOVE, keys, lp);
            }
            for (bit, down, up) in
                [(1, WM_LBUTTONDOWN, WM_LBUTTONUP), (2, WM_RBUTTONDOWN, WM_RBUTTONUP)]
            {
                if pressed & bit != 0 {
                    self.s.note_button(id, down, x, y, true);
                    self.s.send(id, down, keys, lp);
                }
                if released & bit != 0 {
                    self.s.note_button(id, up, x, y, true);
                    self.s.send(id, up, keys, lp);
                }
            }
            self.refresh();
            return core::mem::take(&mut self.s.damage);
        }
        if let Some((id, gx, gy)) = self.drag {
            self.s.move_to(id, x - gx, y - gy);
        }
        self.resize_to(x, y);
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
        // A window the pointer left is told so if served (the Wayland
        // server's pointer leave); a drag or resize leaves it too.
        let over = if self.drag.is_some() || self.resize.is_some() { None } else { target };
        self.s.pointer_over(over);
        if (dx, dy) != (0, 0)
            && self.drag.is_none()
            && self.resize.is_none()
            && let Some(id) = target
        {
            let lp = self.s.client_lparam(id, x, y);
            self.s.send(id, WM_MOUSEMOVE, keys, lp);
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
            self.s.raise(id);
            if !self.s.window(id).is_some_and(|w| w.popup) {
                self.s.activate(Some(id));
            }
            let lp = self.s.client_lparam(id, x, y);
            self.s.note_button(id, WM_RBUTTONDOWN, x, y, false);
            self.s.send(id, WM_RBUTTONDOWN, keys, lp);
        }
        if released & 2 != 0
            && let Hit::Client(id) = hit
        {
            let lp = self.s.client_lparam(id, x, y);
            self.s.note_button(id, WM_RBUTTONUP, x, y, false);
            self.s.send(id, WM_RBUTTONUP, keys, lp);
        }
        self.refresh();
        core::mem::take(&mut self.s.damage)
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
                // A click minimizes the active window on top, else
                // activates (restores) it.
                let minimized = self.s.window(id).is_some_and(|w| w.minimized);
                let top = self.s.windows.iter().rev().find(|w| w.shown()).map(|w| w.id);
                let action = if !minimized && self.s.active == Some(id) && top == Some(id) {
                    Action::Minimize
                } else {
                    Action::Activate
                };
                let _ = self.act(id, action);
            }
            Hit::Caption(id) => {
                self.s.raise(id);
                self.s.activate(Some(id));
                if let Some(w) = self.s.window(id) {
                    self.drag = Some((id, x - i64::from(w.x), y - i64::from(w.y)));
                }
            }
            Hit::Button(id, _) | Hit::Frame(id) => {
                self.s.raise(id);
                self.s.activate(Some(id));
            }
            Hit::Client(id) => {
                self.s.raise(id);
                if !self.s.window(id).is_some_and(|w| w.popup) {
                    self.s.activate(Some(id));
                }
                let lp = self.s.client_lparam(id, x, y);
                self.s.note_button(id, WM_LBUTTONDOWN, x, y, false);
                self.s.send(id, WM_LBUTTONDOWN, keys, lp);
            }
            Hit::Menu | Hit::MenuItem(_) => self.menu_dirty = true,
            Hit::Nothing | Hit::Taskbar => {}
        }
    }

    /// `owner`'s window `id` asks to be dragged by the pointer while the
    /// left button stays down (a program that draws its own caption:
    /// WM_NCHITTEST answered HTCAPTION). STATUS_INVALID_DEVICE_STATE when
    /// the button is up already.
    pub fn begin_drag(&mut self, id: u32, owner: u64) -> Result<(), NtStatus> {
        let (x, y) = self.pointer;
        let w = self.s.owned(id, owner).ok_or(NtStatus::INVALID_HANDLE)?;
        let (wx, wy) = (i64::from(w.x), i64::from(w.y));
        if self.buttons & 1 == 0 {
            return Err(NtStatus::INVALID_DEVICE_STATE);
        }
        self.s.raise(id);
        self.s.activate(Some(id));
        // As Windows' move loop: the drag takes the mouse capture.
        self.release_capture_for_loop(id);
        self.drag = Some((id, x - wx, y - wy));
        // (Host unit tests have no console to log to.)
        #[cfg(not(test))]
        crate::info!("desktop: dragging window {id} for its program (its own caption)");
        Ok(())
    }

    /// `owner`'s window `id` asks to be resized by the pointer from its
    /// `edges` (RESIZE_*) while the left button stays down (its
    /// WM_NCHITTEST answered a border); the program gets the rectangle
    /// (GALA_WM_PLACE) when the button goes up.
    pub fn begin_resize(&mut self, id: u32, owner: u64, edges: u32) -> Result<(), NtStatus> {
        let start = self.pointer;
        let w = self.s.owned(id, owner).ok_or(NtStatus::INVALID_HANDLE)?;
        let rect = [i64::from(w.x), i64::from(w.y), i64::from(w.w), i64::from(w.h)];
        if self.buttons & 1 == 0 {
            return Err(NtStatus::INVALID_DEVICE_STATE);
        }
        if edges == 0 || edges & !0xf != 0 {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        self.s.raise(id);
        self.s.activate(Some(id));
        self.release_capture_for_loop(id);
        self.resize = Some(Resize { id, edges, start, from: rect, to: rect });
        #[cfg(not(test))]
        crate::info!("desktop: resizing window {id} for its program (edges {edges:#x})");
        Ok(())
    }

    /// A drag or resize of `id` that starts takes the mouse capture from
    /// whichever window held it (otherwise every move would go to the
    /// capture and the window would not follow the pointer).
    fn release_capture_for_loop(&mut self, id: u32) {
        if let Some(c) = self.s.captured.take() {
            #[cfg(not(test))]
            crate::info!("desktop: window {c} loses the mouse capture to the move of {id}");
            #[cfg(test)]
            let _ = (c, id);
        }
    }

    /// A resize follows the pointer at (x, y).
    fn resize_to(&mut self, x: i64, y: i64) {
        let Some(Resize { id, edges, start: (sx, sy), from: [rx, ry, rw, rh], .. }) = self.resize
        else {
            return;
        };
        let (dx, dy) = (x - sx, y - sy);
        let (mut l, mut t, mut r, mut b) = (rx, ry, rx + rw, ry + rh);
        if edges & RESIZE_LEFT != 0 {
            l = (l + dx).min(r - RESIZE_MIN.0);
        }
        if edges & RESIZE_RIGHT != 0 {
            r = (r + dx).max(l + RESIZE_MIN.0);
        }
        if edges & RESIZE_TOP != 0 {
            t = (t + dy).min(b - RESIZE_MIN.1);
        }
        if edges & RESIZE_BOTTOM != 0 {
            b = (b + dy).max(t + RESIZE_MIN.1);
        }
        self.resize = Some(Resize {
            id,
            edges,
            start: (sx, sy),
            from: [rx, ry, rw, rh],
            to: [l, t, r - l, b - t],
        });
    }

    fn left_up(&mut self, hit: Hit, x: i64, y: i64, keys: u64) {
        // A resize that ends: the program places its window there.
        if let Some(Resize { id, to: [l, t, w, h], .. }) = self.resize.take() {
            self.s.send_place(id, (l as i32, t as i32, w as u32, h as u32));
        }
        let pressed = core::mem::replace(&mut self.pressed, Hit::Nothing);
        // A drag that ends tells the program where its client area went
        // (WM_MOVE: the client area's screen position).
        if let Some((id, _, _)) = self.drag.take()
            && let Some(w) = self.s.window(id)
        {
            let (cx, cy) = w.client_origin();
            let lp = u64::from(cy as i16 as u16) << 16 | u64::from(cx as i16 as u16);
            self.s.send(id, WM_MOVE, 0, lp);
        }
        match (pressed, hit) {
            // As Windows: the caption buttons are WM_SYSCOMMANDs to the
            // program (DefWindowProc closes, maximizes or restores).
            (Hit::Button(a, CaptionButton::Close), Hit::Button(b, CaptionButton::Close))
                if a == b =>
            {
                let _ = self.act(a, Action::Close);
            }
            (Hit::Button(a, CaptionButton::Maximize), Hit::Button(b, CaptionButton::Maximize))
                if a == b =>
            {
                let maximized = self.s.window(a).is_some_and(|w| w.maximized);
                let _ = self.act(a, if maximized { Action::Restore } else { Action::Maximize });
            }
            (Hit::Button(a, CaptionButton::Minimize), Hit::Button(b, CaptionButton::Minimize))
                if a == b =>
            {
                let _ = self.act(a, Action::Minimize);
            }
            (Hit::MenuItem(a), Hit::MenuItem(b)) if a == b => {
                self.launch = self.listed().get(a).copied();
                self.close_menu();
            }
            (Hit::Client(id), _) => {
                let lp = self.s.client_lparam(id, x, y);
                self.s.note_button(id, WM_LBUTTONUP, x, y, false);
                self.s.send(id, WM_LBUTTONUP, keys, lp);
            }
            (Hit::Menu | Hit::MenuItem(_), _) => self.menu_dirty = true,
            _ => {}
        }
    }

    fn close_menu(&mut self) {
        self.search_len = 0;
        if self.menu.take().is_some() {
            let b = self.menu_bounds();
            self.s.add_damage(b);
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
        self.s.add_damage(b);
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
        if self.bare.is_some() {
            // Nothing will ever draw it.
            return;
        }
        if self.s.screen.0 == 0 {
            // Windows alone (host tests): captions only.
            let active = self.s.active;
            for w in self.s.windows.iter_mut().filter(|w| !w.popup) {
                w.render_caption(active == Some(w.id), None);
            }
            return;
        }
        let active = self.s.active;
        let hover = self.hover;
        let mut changed = Vec::new();
        for w in self.s.windows.iter_mut() {
            let button = match hover {
                Hit::Button(id, b) if id == w.id => Some(b),
                _ => None,
            };
            if w.popup {
                continue;
            }
            let before = w.chrome.caption_state;
            w.render_caption(active == Some(w.id), button);
            if before != w.chrome.caption_state && w.shown() {
                let _ = changed.try_push(w.caption_rect());
            }
        }
        for r in changed {
            self.s.add_damage(r);
        }
        // The taskbar's own changes (hover, the menu, the clock) and the
        // window state's (what it lists).
        if self.taskbar_dirty || self.s.listing_changed || self.taskbar.is_none() {
            self.taskbar = self.render_taskbar();
            self.taskbar_dirty = false;
            self.s.listing_changed = false;
            let r = self.taskbar_rect();
            self.s.add_damage(r);
        }
        if self.menu.is_some() && self.menu_dirty {
            self.menu = self.render_menu();
            self.menu_dirty = false;
            let r = self.menu_rect();
            self.s.add_damage(r);
        }
    }

    /// The taskbar: translucent light with a line on top; the start
    /// button (the camel) and the windows' buttons centred, each an icon
    /// with Windows' indicator under it (the active window: a long accent
    /// pill on a lit button; the others: a short grey one); the clock and
    /// date at the right.
    fn render_taskbar(&self) -> Option<Canvas> {
        let (w, h) = (self.s.screen.0, i64::from(TASKBAR));
        let mut c = Canvas::new(w, h, 0)?;
        for y in 0..h {
            let (row, a) =
                if y == 0 { (TASKBAR_LINE, 255) } else { (TASKBAR_COLOR, TASKBAR_ALPHA) };
            for x in 0..w {
                c.put(x, y, row, a);
            }
        }
        let top = self.s.screen.1 - h;
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
            let Some(win) = self.s.window(id) else { continue };
            let (x, y) = (r.x, r.y - top);
            let active = self.s.active == Some(id) && win.shown();
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
            // The window's icon, or a tile with its initial.
            if win.icon.len() == ICON * ICON {
                let (ix, iy) = (x + 10, y + 8);
                for (k, &p) in win.icon.iter().enumerate() {
                    let (px, py) = ((k % ICON) as i64, (k / ICON) as i64);
                    c.put(ix + px, iy + py, p, p >> 24);
                }
                if active {
                    c.round_rect(x + r.w / 2 - 8, y + 36, 16, 3, 1, ACCENT, ACCENT, 255);
                } else if !win.minimized {
                    c.round_rect(
                        x + r.w / 2 - 3,
                        y + 36,
                        6,
                        3,
                        1,
                        0x00_8a_8a_8a,
                        0x00_8a_8a_8a,
                        255,
                    );
                }
                continue;
            }
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
        for w in self.s.windows.iter().filter(|w| w.shown()) {
            if !w.bounds(SHADOW).contains(x, y) {
                continue;
            }
            let (lx, ly) = (x - i64::from(w.x), y - i64::from(w.y));
            let cov = w.coverage(lx, ly);
            let active = self.s.active == Some(w.id);
            if cov < 255 && !w.square() {
                let r = w.rect();
                let rect = (r.x + 3, r.y + SHADOW_DROP, r.x + r.w - 3, r.y + r.h + SHADOW_DROP - 2);
                let strength = if active { SHADOW_ACTIVE } else { SHADOW_INACTIVE };
                c = blend(c, 0, shadow_alpha(x, y, rect, SHADOW_REACH, strength));
            }
            if cov > 0 {
                c = blend(c, w.body(lx, ly, active), cov);
            }
        }
        if self.s.screen.0 == 0 {
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

/// One session's desktop as shasha keeps it (ROADMAP RS steps B2, B3a):
/// its windows, Z order, focus, capture, pixels and input queue, and the
/// presenter that shows it, behind its own lock (nt::session::Desk holds
/// it). Every session's desktop keeps its windows whether or not anything
/// shows it; the console session's is shown by the screen (or by what
/// `presenter=` names) and gets the keyboard and mouse.
pub struct Shell {
    session: u32,
    d: IrqMutex<Desktop>,
}

impl Shell {
    pub const fn new(session: u32) -> Shell {
        Shell { session, d: IrqMutex::new(Desktop::new()) }
    }

    /// (windows, live, what shows it, why nothing can): the state
    /// dump's figures.
    pub fn counts(&self) -> (usize, bool, Kind, Option<&'static str>) {
        let d = self.d.lock();
        (d.s.windows.len(), d.s.live, d.presenter.kind(), d.bare)
    }

    /// The last mouse buttons and keys the desktop delivered, oldest
    /// first, and how many inputs the full queue dropped (the state
    /// dump's lines).
    pub fn recent_input(
        &self,
    ) -> ([Option<crate::nt::winstate::InputNote>; crate::nt::winstate::RECENT_INPUT], u64, usize)
    {
        let d = self.d.lock();
        let mut out = [None; crate::nt::winstate::RECENT_INPUT];
        for (slot, b) in out.iter_mut().zip(d.s.recent_input()) {
            *slot = Some(*b);
        }
        (out, d.s.input_dropped, d.s.input.len())
    }
}

/// The desktop the console shows (the input thread's), if any: none
/// while a console switch is under way.
fn console() -> Option<crate::mm::heap::KArc<super::session::Desk>> {
    super::session::desk(super::session::active_console())
}

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

/// Composes area `r` of the desktop and hands it to its presenter
/// (nothing is composed when nothing shows it).
fn redraw(d: &mut Desktop, r: Rect) {
    if d.presenter.kind() == Kind::None {
        return;
    }
    let Some(r) = r.clip(d.s.screen.0, d.s.screen.1) else { return };
    let mut row = Vec::new();
    if row.try_reserve_exact(r.w as usize).is_err() {
        return;
    }
    // Out of the desktop while it composes from it (a move, no copy).
    let mut presenter = core::mem::take(&mut d.presenter);
    for y in r.y..r.y + r.h {
        row.clear();
        row.extend((r.x..r.x + r.w).map(|x| d.pixel(x, y)));
        presenter.write_row(r.x as u32, y as u32, &row);
    }
    // A GPU shows the rows now (TRANSFER_TO_HOST_2D + RESOURCE_FLUSH).
    presenter.flush(r.x as u32, r.y as u32, r.w as u32, r.h as u32);
    d.presenter = presenter;
}

fn redraw_damage(d: &mut Desktop) {
    let damage = core::mem::take(&mut d.s.damage);
    for r in damage {
        redraw(d, r);
    }
}

/// `profile=server` (ROADMAP SD, ARCHITECTURE "Server and desktop
/// workloads"): nothing ever draws a desktop, even with a screen; each
/// session's desktop keeps its windows, shown by none (`bare`).
static SERVER: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Boots in the server profile (before any program runs).
pub fn set_server_profile() {
    SERVER.store(true, core::sync::atomic::Ordering::Relaxed);
}

pub fn server_profile() -> bool {
    SERVER.load(core::sync::atomic::Ordering::Relaxed)
}

/// The boot option `presenter=<test|none>` (before any program runs):
/// a headless desktop. False for another value.
pub fn set_local_presenter(name: &str) -> bool {
    present::set_local(name)
}

/// Why nothing can show this machine's desktops, if so: the server
/// profile (it never draws), or no screen and no `presenter=` (BIOS boots,
/// gala-um). Then every session's desktop is bare: the window state alone,
/// shown by none:
/// GUI programs work, GalaSession's query reports their windows, and
/// frames, injected input, the Test and Console presenters and the
/// shell's actions are refused. Fixed before any program runs.
fn bare() -> Option<&'static str> {
    if server_profile() {
        return Some("server profile");
    }
    if present::local().is_some() || platform::screen_size().is_some() {
        return None;
    }
    Some("no screen and no presenter=")
}

/// What shows the console session's desktop when it goes live: the
/// screen, or what `presenter=` names.
fn local_presenter() -> Result<Presenter, NtStatus> {
    Ok(match present::local() {
        None | Some(Kind::Console) => Presenter::Console,
        Some(Kind::None) => Presenter::None,
        Some(Kind::Test) => {
            let (w, h) = DEFAULT_SIZE;
            Presenter::Test(Frame::new(w, h).ok_or(NtStatus::NO_MEMORY)?)
        }
    })
}

/// Logs a Test frame's place (xtask's screen checks read it there with
/// QEMU's `memsave`).
fn note_frame(session: u32, presenter: &Presenter) {
    if let Presenter::Test(f) = presenter {
        let (w, h) = f.size();
        crate::info!(
            "presenter: test frame for session {session}: {w}x{h} at {:#x} ({} bytes, 0x00RRGGBB rows)",
            f.address(),
            4 * w as usize * h as usize
        );
    }
}

/// Makes `shell`'s desktop live if it is not: laid out at its presenter's
/// size (DEFAULT_SIZE without one), drawn whole there. The console
/// session's gets the local presenter and the keyboard and mouse;
/// another session's none. Where nothing shows desktops (`bare`) it is
/// the window state alone at DEFAULT_SIZE, shown by none, as the server
/// kernel's: nothing is laid out or drawn, no input is started.
fn ensure_live(shell: &Shell, d: &mut Desktop) -> Result<(), NtStatus> {
    if d.s.live {
        return Ok(());
    }
    if let Some(why) = bare() {
        let (w, h) = DEFAULT_SIZE;
        d.s.screen = (i64::from(w), i64::from(h));
        d.s.live = true;
        d.bare = Some(why);
        crate::info!(
            "desktop: session {} keeps its windows ({w}x{h}), shown by none ({why})",
            shell.session
        );
        crate::events::emit(
            crate::events::Category::Window,
            "presenter",
            format_args!(
                r#""op":"live","session":{},"presenter":"none","w":{w},"h":{h},"bare":"{why}""#,
                shell.session
            ),
        );
        return Ok(());
    }
    let console = shell.session == super::session::active_console();
    let presenter = if console { local_presenter()? } else { Presenter::None };
    let (w, h) = presenter.size().unwrap_or(DEFAULT_SIZE);
    d.set_screen(w, h)?;
    d.set_minute(platform::system_time() / 600_000_000);
    d.s.live = true;
    let kind = presenter.kind();
    note_frame(shell.session, &presenter);
    d.set_presenter(presenter)?;
    if console {
        crate::info!(
            "desktop: session {} is live ({w}x{h}), shown by {}",
            shell.session,
            kind.name()
        );
        platform::local_input();
    } else {
        crate::info!(
            "desktop: session {} is live ({w}x{h}), shown by {} (the console shows {})",
            shell.session,
            kind.name(),
            super::session::ConsoleShows(super::session::active_console())
        );
    }
    crate::events::emit(
        crate::events::Category::Window,
        "presenter",
        format_args!(
            r#""op":"live","session":{},"presenter":"{}","w":{w},"h":{h}"#,
            shell.session,
            kind.name()
        ),
    );
    Ok(())
}

/// The size of `shell`'s desktop (made live if it was not; DEFAULT_SIZE
/// where nothing shows desktops); None only without the memory to lay it
/// out.
pub fn screen(shell: &Shell) -> Option<(u32, u32)> {
    let mut d = shell.d.lock();
    ensure_live(shell, &mut d).ok()?;
    Some((d.s.screen.0 as u32, d.s.screen.1 as u32))
}

/// Whether shasha's shell (taskbar, start menu, the named window
/// actions) runs on `shell`'s desktop (made live if it was not): not on a
/// bare one (the server profile, no screen and no `presenter=`).
pub fn has_shell(shell: &Shell) -> bool {
    let mut d = shell.d.lock();
    ensure_live(shell, &mut d).is_ok() && d.bare.is_none()
}

/// Runs `f` on the desktop and redraws what it changed. Works the same
/// whatever shows the desktop, or nothing, bare or not.
pub fn with<R>(
    shell: &Shell,
    f: impl FnOnce(&mut Desktop) -> Result<(R, Option<Rect>), NtStatus>,
) -> Result<R, NtStatus> {
    let mut d = shell.d.lock();
    ensure_live(shell, &mut d)?;
    let result = f(&mut d);
    if let Ok((_, Some(area))) = &result {
        d.s.add_damage(*area);
    }
    redraw_damage(&mut d);
    // A window shown, hidden or gone moves the activation: its served
    // owners are told (GALA_WM_ACTIVE).
    let wakes = d.take_wakes();
    drop(d);
    signal(wakes);
    result.map(|(r, _)| r)
}

/// Attaches a presenter of `kind` to `shell`'s desktop (GalaSession):
/// it goes live if it was not, a Test frame is `size` (else the
/// desktop's size; the desktop takes the presenter's size, keeping its
/// windows' places) and the desktop is drawn whole there. The old
/// presenter goes (a Test frame is freed). Returns the kind it replaced.
/// Console on another session than the one the console shows is a
/// console switch (`switch_console`). Where nothing shows desktops
/// (`bare`) only none is accepted (what shows it already; the desktop
/// goes live); STATUS_NOT_SUPPORTED for the
/// others there, for Console without a screen or on session 0;
/// STATUS_NO_MEMORY for a frame.
pub fn attach(shell: &Shell, kind: Kind, size: Option<(u32, u32)>) -> Result<Kind, NtStatus> {
    if let Some(why) = bare() {
        if kind != Kind::None {
            crate::warn!(
                "desktop: session {} cannot be shown by {}: {why}",
                shell.session,
                kind.name()
            );
            return Err(NtStatus::NOT_SUPPORTED);
        }
        let mut d = shell.d.lock();
        ensure_live(shell, &mut d)?;
        return Ok(Kind::None);
    }
    if kind == Kind::Console {
        if platform::screen_size().is_none() {
            return Err(NtStatus::NOT_SUPPORTED);
        }
        if shell.session != super::session::active_console() {
            return switch_console(shell);
        }
    }
    let current = {
        let mut d = shell.d.lock();
        ensure_live(shell, &mut d)?;
        (d.s.screen.0 as u32, d.s.screen.1 as u32)
    };
    // A frame (megabytes) is made outside the desktop's lock.
    let presenter = match kind {
        Kind::None => Presenter::None,
        Kind::Console => Presenter::Console,
        Kind::Test => {
            let (w, h) = size.unwrap_or(current);
            Presenter::Test(Frame::new(w, h).ok_or(NtStatus::NO_MEMORY)?)
        }
    };
    note_frame(shell.session, &presenter);
    let mut d = shell.d.lock();
    let old = d.set_presenter(presenter)?;
    let (w, h) = d.s.screen;
    drop(d);
    let was = old.kind();
    // The old frame is freed outside the lock.
    drop(old);
    crate::info!(
        "desktop: session {} is now shown by {} ({w}x{h}; was {})",
        shell.session,
        kind.name(),
        was.name()
    );
    crate::events::emit(
        crate::events::Category::Window,
        "presenter",
        format_args!(
            r#""op":"attach","session":{},"presenter":"{}","was":"{}","w":{w},"h":{h}"#,
            shell.session,
            kind.name(),
            was.name()
        ),
    );
    Ok(was)
}

/// Serializes console switches (lock order: before any desktop's lock).
static SWITCH: IrqMutex<()> = IrqMutex::new(());

/// The console switch (fast user switching; `gala attach`, GalaSession's
/// SESSION_PRESENT with Console): the screen, the keyboard and the mouse
/// go to `to`'s desktop, which goes live if it was not and is drawn whole
/// on the screen; the desktop the console showed is shown by nothing
/// from then on and keeps its windows. ActiveConsoleId
/// (WTSGetActiveConsoleSessionId) follows: 0xFFFFFFFF while the switch is
/// under way, then `to`'s id. Returns what showed `to` before.
/// STATUS_NOT_SUPPORTED for session 0 (never on the console: session 0
/// isolation) and where nothing shows desktops (`bare`); on a failure (no
/// memory to lay `to` out at the screen's size) the console stays where
/// it was.
pub fn switch_console(to: &Shell) -> Result<Kind, NtStatus> {
    use super::session;
    if to.session == session::SERVICES || bare().is_some() {
        return Err(NtStatus::NOT_SUPPORTED);
    }
    let _switching = SWITCH.lock();
    let from_id = session::active_console();
    // `to` goes live first, shown by nothing (not the console's yet): a
    // failure here leaves the console as it was.
    {
        let mut d = to.d.lock();
        ensure_live(to, &mut d)?;
    }
    let from = session::desk(from_id);
    // No session is on the console while it moves: input is dropped.
    session::set_active_console(session::NO_CONSOLE);
    let left = from.as_ref().map(|f| move_console(&f.shell, None));
    let result = move_console(to, Some(Presenter::Console));
    let was = match result {
        Ok(was) => was,
        Err(e) => {
            // Back where it was.
            if let (Some(f), Some(Ok(Kind::Console))) = (from.as_ref(), left) {
                let _ = move_console(&f.shell, Some(Presenter::Console));
            }
            session::set_active_console(from_id);
            crate::warn!(
                "desktop: the console could not move from {} to session {}: {:#010x}; it stays",
                session::ConsoleShows(from_id),
                to.session,
                e.0
            );
            return Err(e);
        }
    };
    session::set_active_console(to.session);
    let (w, h) = query(to).2;
    let kept = from.as_ref().map_or(0, |f| f.shell.counts().0);
    crate::info!(
        "desktop: the console now shows session {} ({w}x{h}; was {}, which keeps its {kept} windows shown by none)",
        to.session,
        session::ConsoleShows(from_id),
    );
    crate::events::emit(
        crate::events::Category::Window,
        "presenter",
        format_args!(
            r#""op":"console","session":{},"from":{},"was":"{}","kept":{kept},"w":{w},"h":{h}"#,
            to.session,
            if from_id == session::NO_CONSOLE { -1 } else { i64::from(from_id) },
            was.name()
        ),
    );
    Ok(was)
}

/// One half of a console switch, under `shell`'s lock alone: `to` Some,
/// the desktop is shown by it from now on (the console arrives); None,
/// the console leaves (held buttons and keys are let go, then nothing
/// shows it: a desktop shown by something else keeps that). Returns what
/// showed it before.
fn move_console(shell: &Shell, to: Option<Presenter>) -> Result<Kind, NtStatus> {
    let mut d = shell.d.lock();
    let was = d.presenter.kind();
    let old = match to {
        Some(p) => d.set_presenter(p)?,
        None => {
            if !d.s.live || was != Kind::Console {
                return Ok(was);
            }
            for r in d.leave_console() {
                redraw(&mut d, r);
            }
            d.set_presenter(Presenter::None)?
        }
    };
    let wakes = d.take_wakes();
    drop(d);
    // A Test frame it replaced is freed outside the lock.
    drop(old);
    signal(wakes);
    Ok(was)
}

/// What a session's desktop is (GalaSession's query): what shows it,
/// whether it is live, its size (0 x 0 before) and its windows; nothing
/// is made live.
pub fn query(shell: &Shell) -> (Kind, bool, (u32, u32), usize) {
    let d = shell.d.lock();
    (d.presenter.kind(), d.s.live, (d.s.screen.0 as u32, d.s.screen.1 as u32), d.s.windows.len())
}

/// Copies area `r` of the Test frame that shows `shell`'s desktop into
/// `out` (row after row; GalaSession): the frame's size and flush count.
/// STATUS_INVALID_DEVICE_STATE when no Test presenter shows it,
/// STATUS_INVALID_PARAMETER when `r` is not inside the frame.
pub fn read_frame(shell: &Shell, r: Rect, out: &mut [u32]) -> Result<((u32, u32), u64), NtStatus> {
    let d = shell.d.lock();
    let Presenter::Test(f) = &d.presenter else { return Err(NtStatus::INVALID_DEVICE_STATE) };
    if !f.read(r, out) {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    Ok((f.size(), f.flushes()))
}

/// A process ended: its windows go (shown or not).
pub fn process_ended(shell: &Shell, owner: u64) {
    let mut d = shell.d.lock();
    if !d.s.live {
        return;
    }
    d.destroy_owned(owner);
    redraw_damage(&mut d);
    let wakes = d.take_wakes();
    drop(d);
    signal(wakes);
}

/// Mouse input: the pointer moved by (dx, dy) with `buttons` held (bit 0
/// left, 1 right, 2 middle), to the desktop the console shows. Nothing
/// before that desktop is live, nor during a console switch.
/// Returns the session and the program its start menu asked to start, if
/// any.
pub fn pointer_event(dx: i64, dy: i64, buttons: u8) -> Option<(u32, &'static Program)> {
    let desk = console()?;
    pointer_on(&desk.shell, buttons, |d| d.pointer(dx, dy, buttons))
        .map(|p| (desk.shell.session, p))
}

/// Mouse input to `shell`'s desktop: `go` moves the pointer (relative,
/// the mouse's; absolute, injected) with `buttons` held. The program its
/// start menu asked to start, if any.
fn pointer_on(
    shell: &Shell,
    buttons: u8,
    go: impl FnOnce(&mut Desktop) -> Vec<Rect>,
) -> Option<&'static Program> {
    // Which programs exist (outside the desktop's lock: the file system).
    let (available, n) = if buttons & 1 != 0 { available_programs() } else { ([0; 8], 0) };
    let mut d = shell.d.lock();
    // No pointer moves on a bare desktop.
    if !d.s.live || d.bare.is_some() {
        return None;
    }
    if buttons & 1 != 0 && !d.menu_open() {
        d.set_programs(&available[..n]);
    }
    let damage = go(&mut d);
    for r in damage {
        redraw(&mut d, r);
    }
    let launch = d.take_launch();
    let wakes = d.take_wakes();
    drop(d);
    signal(wakes);
    launch
}

/// Input a Test presenter brings to its desktop (GalaSession's
/// SESSION_INPUT: screen checks without the PS/2 devices).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Injected {
    /// The pointer to (x, y) with these buttons held (bit 0 left, 1 right).
    Pointer { x: i64, y: i64, buttons: u8 },
    /// A key down or up (virtual key, scan code, E0-prefixed).
    Key { vk: u8, scan: u8, extended: bool, down: bool },
    /// The wheel turned (positive away from the user).
    Wheel(i64),
}

/// Brings `input` to `shell`'s desktop as the keyboard and mouse would,
/// when a Test presenter shows it: STATUS_INVALID_DEVICE_STATE otherwise
/// (the console's desktop gets the PS/2 devices, one shown by nothing
/// gets nothing). Returns the program its start menu asked to start.
pub fn inject(shell: &Shell, input: Injected) -> Result<Option<&'static Program>, NtStatus> {
    {
        let d = shell.d.lock();
        if !d.s.live || d.presenter.kind() != Kind::Test {
            return Err(NtStatus::INVALID_DEVICE_STATE);
        }
    }
    Ok(match input {
        Injected::Pointer { x, y, buttons } => {
            pointer_on(shell, buttons, |d| d.pointer_at(x, y, buttons))
        }
        Injected::Key { vk, scan, extended, down } => {
            key_on(shell, vk, scan, extended, down);
            None
        }
        Injected::Wheel(notches) => {
            wheel_on(shell, notches);
            None
        }
    })
}

/// The start menu's programs that exist (indices into PROGRAMS, one per
/// name) and how many. Reads the file system: not under the desktop's
/// lock.
fn available_programs() -> ([usize; 8], usize) {
    let mut available: [usize; 8] = [0; 8];
    let mut n = 0;
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
    (available, n)
}

/// The start menu's programs that exist (GalaDesktopAction's program
/// list and `gala start <name>`).
pub fn start_menu() -> Vec<&'static Program> {
    let (available, n) = available_programs();
    let mut out = Vec::new();
    for &i in &available[..n] {
        if out.try_push(&PROGRAMS[i]).is_err() {
            break;
        }
    }
    out
}

/// GalaDesktopAction on window `id` of process `owner` (the desktop's
/// ids; the caller found it from the window's HWND):
/// STATUS_NOT_SUPPORTED without shasha's shell (`has_shell`: a bare
/// desktop), STATUS_INVALID_HANDLE when there is no such window.
pub fn window_action(shell: &Shell, owner: u64, id: u32, action: Action) -> Result<(), NtStatus> {
    if !has_shell(shell) {
        return Err(NtStatus::NOT_SUPPORTED);
    }
    let mut d = shell.d.lock();
    if d.s.window(id).is_none_or(|w| w.owner != owner) {
        return Err(NtStatus::INVALID_HANDLE);
    }
    let result = d.act(id, action);
    redraw_damage(&mut d);
    let wakes = d.take_wakes();
    drop(d);
    signal(wakes);
    result
}

/// The windows the taskbar shows (GalaDesktopAction): none before the
/// desktop is drawn; STATUS_NOT_SUPPORTED without shasha's shell.
pub fn list_windows(shell: &Shell) -> Result<Vec<Listed>, NtStatus> {
    if !has_shell(shell) {
        return Err(NtStatus::NOT_SUPPORTED);
    }
    Ok(shell.d.lock().list())
}

/// Shows wallpaper `name` (one of WALLPAPERS, or "drawn") from now on
/// (GalaDesktopAction): STATUS_OBJECT_NAME_NOT_FOUND for another name or
/// a missing file, STATUS_NOT_SUPPORTED without shasha's shell.
pub fn set_wallpaper(shell: &Shell, name: &str) -> Result<(), NtStatus> {
    if !has_shell(shell) {
        return Err(NtStatus::NOT_SUPPORTED);
    }
    let picture = if name.eq_ignore_ascii_case("drawn") {
        None
    } else {
        let (_, path) = WALLPAPERS
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
        let data = super::file::lookup(path)
            .and_then(super::file::read_all)
            .map_err(|_| NtStatus::OBJECT_NAME_NOT_FOUND)?;
        Some(paint::decode_qoi(&data).ok_or(NtStatus::INVALID_IMAGE_FORMAT)?)
    };
    // The new layout is made outside the desktop's lock (it scales the
    // picture to the screen), then swapped in.
    let screen = shell.d.lock().s.screen;
    let layout = if screen.0 > 0 {
        Some(Wallpaper::new(screen.0, screen.1, picture.as_ref()).ok_or(NtStatus::NO_MEMORY)?)
    } else {
        None
    };
    *PICTURE.lock() = picture;
    let mut d = shell.d.lock();
    if let Some(layout) = layout
        && d.s.live
        && d.s.screen == screen
    {
        d.wallpaper = Some(layout);
        d.s.add_damage(Rect { x: 0, y: 0, w: screen.0, h: screen.1 });
        redraw_damage(&mut d);
    }
    drop(d);
    crate::info!("desktop: wallpaper {name}");
    Ok(())
}

/// Tells the listeners of processes with new input (outside the
/// desktop's lock): an event is signalled, a server's thread woken.
fn signal(listeners: Vec<Listener>) {
    for l in listeners {
        match l {
            Listener::Event(e) => {
                super::wait::set_signal(&e, 1);
            }
            Listener::Server(thread) => platform::wake(thread),
        }
    }
}

/// Keyboard input: virtual key `vk` (scan code `scan`, E0-prefixed if
/// `extended`) went down or up.
/// The mouse wheel (from the input thread).
pub fn wheel_event(notches: i64) {
    if let Some(desk) = console() {
        wheel_on(&desk.shell, notches);
    }
}

fn wheel_on(shell: &Shell, notches: i64) {
    let mut d = shell.d.lock();
    if !d.s.live || d.bare.is_some() {
        return;
    }
    d.wheel(notches);
    let wakes = d.take_wakes();
    drop(d);
    signal(wakes);
}

pub fn key_event(vk: u8, scan: u8, extended: bool, down: bool) {
    if let Some(desk) = console() {
        key_on(&desk.shell, vk, scan, extended, down);
    }
}

fn key_on(shell: &Shell, vk: u8, scan: u8, extended: bool, down: bool) {
    let mut d = shell.d.lock();
    if !d.s.live || d.bare.is_some() {
        return;
    }
    d.key(vk, scan, extended, down);
    redraw_damage(&mut d);
    let wakes = d.take_wakes();
    drop(d);
    signal(wakes);
}

/// GalaWindowListen: `owner`'s input event.
pub fn listen(shell: &Shell, owner: u64, event: ObjectRef) -> Result<(), NtStatus> {
    listen_with(shell, owner, Listener::Event(event))
}

/// `owner`'s windows are served by kernel thread `thread` (the Wayland
/// server, for a Linux client's windows): it is woken for their input,
/// which then includes GALA_WM_ACTIVE and GALA_WM_POINTER_LEFT.
pub fn listen_server(
    shell: &Shell,
    owner: u64,
    thread: platform::ThreadId,
) -> Result<(), NtStatus> {
    listen_with(shell, owner, Listener::Server(thread))
}

/// `thread` no longer serves `owner`'s windows.
pub fn unlisten_server(shell: &Shell, owner: u64, thread: platform::ThreadId) {
    shell.d.lock().s.unlisten_server(owner, thread);
}

fn listen_with(shell: &Shell, owner: u64, listener: Listener) -> Result<(), NtStatus> {
    let mut d = shell.d.lock();
    d.listen(owner, listener)?;
    let wakes = d.take_wakes();
    drop(d);
    signal(wakes);
    Ok(())
}

/// The clock: called now and then (the input thread), for every live
/// desktop. Also notices a display change (virtio-gpu: the host's window
/// resized), which the desktop the screen shows follows.
pub fn tick() {
    let changed = platform::screen_changed();
    let minute = platform::system_time() / 600_000_000;
    let Ok(ids) = super::session::ids() else { return };
    for id in ids {
        let Some(desk) = super::session::desk(id) else { continue };
        let mut d = desk.shell.d.lock();
        if !d.s.live || d.bare.is_some() {
            continue;
        }
        if let Some((w, h)) = changed
            && d.presenter.kind() == Kind::Console
        {
            resize(&mut d, w, h);
        }
        d.set_minute(minute);
        redraw_damage(&mut d);
    }
}

/// The screen is now w x h: the shell is laid out again and everything
/// redrawn. Windows keep their places (a later step moves maximized and
/// docked ones and sends WM_DISPLAYCHANGE: ARCHITECTURE "Plan: graphics
/// and GPU").
fn resize(d: &mut Desktop, w: u32, h: u32) {
    if d.s.screen == (i64::from(w), i64::from(h)) {
        return;
    }
    crate::info!("desktop: the screen is now {w}x{h}");
    if d.set_screen(w, h).is_err() {
        crate::warn!("desktop: no memory to lay out a {w}x{h} screen");
        return;
    }
    d.s.damage.clear();
    redraw(d, Rect { x: 0, y: 0, w: w.into(), h: h.into() });
}

/// Takes input waiting for `owner`'s windows into `out`; how many.
pub fn take_input(shell: &Shell, owner: u64, out: &mut [InputEvent]) -> usize {
    shell.d.lock().take_input(owner, out)
}

pub fn has_input(shell: &Shell, owner: u64) -> bool {
    shell.d.lock().has_input(owner)
}

#[cfg(test)]
mod tests;
