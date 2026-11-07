//! A desktop's window state, apart from composition (ROADMAP RS step B3a,
//! ARCHITECTURE "Presenters (B3a)", Target design item 1): its top-level
//! windows bottom to top (rectangles, shown, minimized, maximized, docked,
//! popups), the active window, the mouse capture, each window's title,
//! icon and last client pixels, the input queued for programs and the
//! events that wake them. Every kernel builds it: shasha's compositor
//! (`nt::desktop`) draws frames, the taskbar, the start menu and the
//! wallpaper on top of it, and the server kernel (`desktop_off.rs`, no
//! compositor) keeps the same state with nothing to compose it, ready for
//! a remote presenter (B3b).
//!
//! What is here is what a program can ask about or change through its
//! window calls, and what the shell's named actions (`Action`) do to it;
//! how windows look (captions, shadows, the pointer) is the compositor's.
//! `Window::chrome` is the compositor's per-window data (`()` without
//! one), and `Window::look` counts changes to what a frame shows (title,
//! icon, size, maximized), so a compositor knows when to draw it again.

use super::object::ObjectRef;
use super::status::NtStatus;
use crate::mm::heap::TryVec;
use alloc::vec::Vec;

/// Frame metrics (what user32's GetSystemMetrics reports): Windows 11's
/// (a 1-pixel border, a 32-pixel caption, 46 x 32 caption buttons).
pub const BORDER: u32 = 1;
pub const CAPTION: u32 = 32;
/// The caption buttons' width.
pub const CAPTION_BUTTON: i64 = 46;
/// The taskbar's rows at the bottom of a desktop: not in its work area
/// (maximized windows and Snap stop above it).
pub const TASKBAR: u32 = 48;

/// The size of a desktop no screen sizes: a session's desktop made while
/// nothing shows it, `presenter=test`'s frame. QEMU's default UEFI mode,
/// and what user32 assumes without screen information.
pub const DEFAULT_SIZE: (u32, u32) = (1280, 800);

/// The largest window gala accepts (each pixel is kept twice).
pub const MAX_SIDE: u32 = 4096;
/// Input waiting for programs (oldest dropped beyond this).
const MAX_INPUT: usize = 512;

/// Window messages the desktop sends to programs.
pub const WM_MOVE: u32 = 0x0003;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_SYSCOMMAND: u32 = 0x0112;
pub const SC_CLOSE: u64 = 0xf060;
pub const SC_MAXIMIZE: u64 = 0xf030;
pub const SC_RESTORE: u64 = 0xf120;
/// gala's request to a window's program to move it (Snap): wParam the
/// left and top, lParam the width and height (low and high 32 bits).
/// Above 0xFFFF: no Windows message has this number.
pub const GALA_WM_PLACE: u32 = 0x0001_0001;
/// gala's notice to a window's program that the desktop minimized
/// (wParam 1) or restored (0) its window itself (user32 keeps IsIconic).
pub const GALA_WM_MINIMIZED: u32 = 0x0001_0002;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_MOUSEWHEEL: u32 = 0x020a;
/// One wheel notch (WHEEL_DELTA).
pub const WHEEL_DELTA: i64 = 120;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const WM_SYSKEYUP: u32 = 0x0105;
pub const VK_MENU: u8 = 0x12;

/// GalaWindowCreate's style: a frameless popup.
pub const STYLE_POPUP: u32 = 0x1;
/// GalaWindowCreate's style: an application window that draws its own
/// frame (WM_NCCALCSIZE took gala's away): all client area, otherwise as
/// a framed window (active, on the taskbar, in the usual order).
pub const STYLE_FRAMELESS: u32 = 0x2;

/// Whether an outer size suits a window (a framed one needs room for its
/// caption and buttons).
pub fn size_fits(w: u32, h: u32, popup: bool) -> bool {
    let (min_w, min_h) = if popup {
        (1, 1)
    } else {
        (2 * BORDER + 2 * CAPTION_BUTTON as u32 + 40, 2 * BORDER + CAPTION + 1)
    };
    w >= min_w && h >= min_h && w <= MAX_SIDE && h <= MAX_SIDE
}

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

/// Messages `recent_input` notes: mouse buttons and keys.
fn is_noted(message: u32) -> bool {
    matches!(message, 0x0100..=0x0105 | 0x0201..=0x020d)
}

/// Where Snap puts a window (Win+arrow keys, `gala snap`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapTo {
    /// The left or right half of the work area.
    Left,
    Right,
    /// Maximized.
    Up,
    /// Back from maximized or snapped, else minimized.
    Down,
}

/// The shell's named actions on a top-level window: what the taskbar,
/// the caption buttons and the Snap keys do, and what GalaDesktopAction
/// asks for (ARCHITECTURE "Server and desktop workloads": CLI parity).
/// The program moves its own window (WM_SYSCOMMAND, GALA_WM_PLACE), as
/// with the mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// On top and active (a minimized one restored): a taskbar click.
    Activate,
    Minimize,
    /// SC_MAXIMIZE (a minimized one restored first).
    Maximize,
    /// Back from minimized, maximized (SC_RESTORE) or snapped.
    Restore,
    /// SC_CLOSE (the program decides).
    Close,
    Snap(SnapTo),
    /// To this outer rectangle.
    Place {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    },
}

/// A window as the taskbar lists it (GalaDesktopAction's window list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listed {
    pub id: u32,
    pub owner: u64,
    /// LISTED_MINIMIZED, LISTED_MAXIMIZED, LISTED_ACTIVE, LISTED_SNAPPED.
    pub flags: u32,
    /// The outer rectangle: x, y, width, height.
    pub rect: [i32; 4],
    pub title: [u16; LISTED_TITLE],
    pub title_len: usize,
}

pub const LISTED_TITLE: usize = 128;
pub const LISTED_MINIMIZED: u32 = 1;
pub const LISTED_MAXIMIZED: u32 = 2;
pub const LISTED_ACTIVE: u32 = 4;
/// Against the left or right edge, the work area's height (Snap).
pub const LISTED_SNAPPED: u32 = 8;

/// The size the taskbar draws a window's icon at.
pub const ICON: usize = 24;
/// The caption's icon size (Windows' small icon, SM_CXSMICON).
pub const SMALL_ICON: usize = 16;

/// An icon of `size` x `size` 0xAARRGGBB pixels scaled to ICON x ICON:
/// each output pixel averages the source area it covers (box filter,
/// weighted by alpha so transparent pixels don't darken the edges).
pub fn scale_icon(src: &[u32], size: usize) -> Option<Vec<u32>> {
    scale_icon_to(src, size, ICON)
}

/// `scale_icon` to `to` x `to` pixels.
pub fn scale_icon_to(src: &[u32], size: usize, to: usize) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    out.try_reserve_exact(to * to).ok()?;
    if size == 0 || src.len() < size * size {
        return None;
    }
    for oy in 0..to {
        for ox in 0..to {
            // The source span [x0, x1) in units of 1/to of a source pixel.
            let (x0, x1) = (ox * size, (ox + 1) * size);
            let (y0, y1) = (oy * size, (oy + 1) * size);
            let (mut a, mut r, mut g, mut b, mut area) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for sy in y0 / to..y1.div_ceil(to) {
                let hy = ((sy + 1) * to).min(y1) - (sy * to).max(y0);
                for sx in x0 / to..x1.div_ceil(to) {
                    let wx = ((sx + 1) * to).min(x1) - (sx * to).max(x0);
                    let wgt = (wx * hy) as u64;
                    let p = src[sy * size + sx];
                    let pa = u64::from(p >> 24);
                    a += pa * wgt;
                    r += u64::from((p >> 16) & 0xff) * pa * wgt;
                    g += u64::from((p >> 8) & 0xff) * pa * wgt;
                    b += u64::from(p & 0xff) * pa * wgt;
                    area += wgt;
                }
            }
            let pixel = if a == 0 {
                0
            } else {
                let alpha = (a + area / 2) / area;
                let c = |v: u64| ((v + a / 2) / a) as u32;
                ((alpha as u32) << 24) | (c(r) << 16) | (c(g) << 8) | c(b)
            };
            out.try_push(pixel).ok()?;
        }
    }
    Some(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

impl Rect {
    pub fn contains(&self, x: i64, y: i64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }

    /// Clipped to the screen `(0, 0, sw, sh)`.
    pub fn clip(self, sw: i64, sh: i64) -> Option<Rect> {
        let (x0, y0) = (self.x.max(0), self.y.max(0));
        let (x1, y1) = ((self.x + self.w).min(sw), (self.y + self.h).min(sh));
        (x1 > x0 && y1 > y0).then_some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }

    pub fn offset(self, dx: i64, dy: i64) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, ..self }
    }
}

/// How far a compositor draws outside a window (its shadow): the reach
/// on every side and the drop below. A window's damage covers it; (0, 0)
/// without a compositor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shadow {
    pub reach: i64,
    pub drop: i64,
}

/// A top-level window; `X` is the compositor's own data for it.
pub struct Window<X> {
    pub id: u32,
    pub owner: u64,
    pub x: i32,
    pub y: i32,
    /// The outer size (frame included).
    pub w: u32,
    pub h: u32,
    /// Shown by its program.
    pub visible: bool,
    /// Minimized by the user (on the taskbar only).
    pub minimized: bool,
    /// It fills the work area (square corners, no shadow; the maximize
    /// button shows "restore").
    pub maximized: bool,
    /// It fills the work area's height against its left or right edge
    /// (docked, as Snap puts it): square corners and no shadow too.
    pub docked: bool,
    /// The client area's pixels (client_w * client_h), white at first:
    /// its program's last ones.
    pub client: Vec<u32>,
    /// The title (UTF-16).
    pub title: Vec<u16>,
    /// A frameless popup (STYLE_POPUP: a menu, a drop-down list, a
    /// tooltip): all client area, above the framed windows, not on the
    /// taskbar, never active.
    pub popup: bool,
    /// An application window without gala's frame (STYLE_FRAMELESS).
    pub frameless: bool,
    /// Its icon (GalaWindowSetIcon), ICON x ICON 0xAARRGGBB pixels
    /// (straight alpha); empty: none (the taskbar shows a lettered tile).
    pub icon: Vec<u32>,
    /// The same at SMALL_ICON for the caption.
    pub small_icon: Vec<u32>,
    /// Its rectangle before Snap put it on a half of the screen (Win+Down
    /// gives it back).
    pub unsnapped: Option<(i32, i32, u32, u32)>,
    /// Counts changes to what its frame shows (title, icon, size,
    /// maximized): a compositor draws the frame again when it moved on.
    pub look: u32,
    /// The compositor's data (its caption's pixels).
    pub chrome: X,
}

impl<X> Window<X> {
    /// The frame's left/right/bottom width and its top (caption) height.
    pub fn frame(&self) -> (u32, u32) {
        if self.popup || self.frameless { (0, 0) } else { (BORDER, BORDER + CAPTION) }
    }

    pub fn client_size(&self) -> (u32, u32) {
        let (b, top) = self.frame();
        (self.w - 2 * b, self.h - top - b)
    }

    pub fn shown(&self) -> bool {
        self.visible && !self.minimized
    }

    /// The screen rectangle it covers.
    pub fn rect(&self) -> Rect {
        Rect { x: i64::from(self.x), y: i64::from(self.y), w: self.w.into(), h: self.h.into() }
    }

    /// Square corners and no shadow: maximized or docked.
    pub fn square(&self) -> bool {
        self.maximized || self.docked
    }

    /// With its shadow.
    pub fn bounds(&self, shadow: Shadow) -> Rect {
        let r = self.rect();
        if self.square() || self.popup {
            return r;
        }
        Rect {
            x: r.x - shadow.reach,
            y: r.y - shadow.reach,
            w: r.w + 2 * shadow.reach,
            h: r.h + 2 * shadow.reach + shadow.drop,
        }
    }

    pub fn client_origin(&self) -> (i64, i64) {
        let (b, top) = self.frame();
        (i64::from(self.x) + i64::from(b), i64::from(self.y) + i64::from(top))
    }

    pub fn caption_rect(&self) -> Rect {
        Rect {
            x: i64::from(self.x),
            y: i64::from(self.y),
            w: self.w.into(),
            h: i64::from(BORDER + CAPTION),
        }
    }

    /// What its frame shows changed.
    fn restyled(&mut self) {
        self.look = self.look.wrapping_add(1);
    }
}

/// A desktop's window state (one per session: nt::desktop's `Shell`, the
/// server kernel's too). Bottom to top; damage collects the screen areas
/// changes touched, for whatever composes the desktop (dropped where
/// nothing does).
pub struct State<X> {
    /// Bottom to top.
    pub windows: Vec<Window<X>>,
    next_id: u32,
    /// Its size is known (laid out): it keeps its windows and their
    /// pixels from then on, whether or not anything shows it.
    pub live: bool,
    /// The desktop's size (0 x 0 until known).
    pub screen: (i64, i64),
    pub active: Option<u32>,
    /// The window with the mouse capture (GalaWindowCapture): it gets all
    /// mouse input, wherever the pointer is.
    pub captured: Option<u32>,
    pub damage: Vec<Rect>,
    /// What the taskbar lists changed (a window shown, hidden, minimized,
    /// activated, renamed, given an icon, gone).
    pub listing_changed: bool,
    shadow: Shadow,
    /// Input queued for programs, oldest first.
    pub input: Vec<InputEvent>,
    /// Each process's event to signal when input arrives for it
    /// (GalaWindowListen), and the processes with new input.
    listeners: Vec<(u64, ObjectRef)>,
    woken: Vec<u64>,
    /// The last mouse buttons and keys delivered (`note_button`,
    /// `key`), newest at `input_at - 1`, each with when its program took
    /// it: the state dump shows them, so input a program did not act on
    /// shows where it went and whether the program had it, without a
    /// trace switched on (one that changes the timing it is about).
    recent_input: [InputNote; RECENT_INPUT],
    input_at: usize,
    /// Input dropped because the queue was full (MAX_INPUT).
    pub input_dropped: u64,
}

/// How many deliveries `recent_input` keeps.
pub const RECENT_INPUT: usize = 16;

/// A mouse button or key delivered: when (monotonic ns), the message,
/// the screen point (a key: x its virtual key), the window, whether a
/// capture took it; when the program took it from the queue (0: not
/// yet), whether the full queue dropped it first.
#[derive(Clone, Copy, Default, Debug)]
pub struct InputNote {
    pub t: u64,
    pub message: u32,
    pub x: i64,
    pub y: i64,
    pub window: u32,
    pub captured: bool,
    pub taken: u64,
    pub dropped: bool,
}

impl<X: Default> State<X> {
    pub const fn new(shadow: Shadow) -> Self {
        State {
            windows: Vec::new(),
            next_id: 1,
            live: false,
            screen: (0, 0),
            active: None,
            captured: None,
            damage: Vec::new(),
            listing_changed: true,
            shadow,
            input: Vec::new(),
            listeners: Vec::new(),
            woken: Vec::new(),
            recent_input: [InputNote {
                t: 0,
                message: 0,
                x: 0,
                y: 0,
                window: 0,
                captured: false,
                taken: 0,
                dropped: false,
            }; RECENT_INPUT],
            input_at: 0,
            input_dropped: 0,
        }
    }

    /// The last mouse buttons and keys delivered, oldest first.
    pub fn recent_input(&self) -> impl Iterator<Item = &InputNote> {
        let n = self.input_at.min(RECENT_INPUT);
        (self.input_at - n..self.input_at).map(|i| &self.recent_input[i % RECENT_INPUT])
    }

    /// Records a delivery in `recent_input`.
    fn note_input(&mut self, note: InputNote) {
        self.recent_input[self.input_at % RECENT_INPUT] = note;
        self.input_at += 1;
    }

    /// The oldest note of `window`'s `message` neither taken nor dropped
    /// (buttons and keys reach a window in order), marked as taken now
    /// or as dropped.
    fn settle_note(&mut self, window: u32, message: u32, dropped: bool) {
        let n = self.input_at.min(RECENT_INPUT);
        for i in self.input_at - n..self.input_at {
            let note = &mut self.recent_input[i % RECENT_INPUT];
            if note.window == window && note.message == message && note.taken == 0 && !note.dropped
            {
                if dropped {
                    note.dropped = true;
                } else {
                    note.taken = crate::nt::platform::monotonic_ns().max(1);
                }
                return;
            }
        }
    }

    pub fn shadow(&self) -> Shadow {
        self.shadow
    }

    pub fn add_damage(&mut self, r: Rect) {
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

    fn find(&mut self, id: u32, owner: u64) -> Result<&mut Window<X>, NtStatus> {
        self.windows
            .iter_mut()
            .find(|w| w.id == id && w.owner == owner)
            .ok_or(NtStatus::INVALID_HANDLE)
    }

    /// `owner`'s window `id`, if it has it.
    pub fn owned(&self, id: u32, owner: u64) -> Option<&Window<X>> {
        self.windows.iter().find(|w| w.id == id && w.owner == owner)
    }

    pub fn window(&self, id: u32) -> Option<&Window<X>> {
        self.windows.iter().find(|w| w.id == id)
    }

    pub fn index(&self, id: u32) -> Option<usize> {
        self.windows.iter().position(|w| w.id == id)
    }

    // ---- What programs do ----------------------------------------------

    /// A new hidden window of outer size w x h at (x, y) titled `title`;
    /// `style` STYLE_POPUP: a frameless popup.
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
        let popup = style & STYLE_POPUP != 0;
        let frameless = style & STYLE_FRAMELESS != 0;
        if style & !(STYLE_POPUP | STYLE_FRAMELESS) != 0
            || (popup && frameless)
            || !size_fits(w, h, popup || frameless)
        {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let (b, top) = if popup || frameless { (0, 0) } else { (BORDER, BORDER + CAPTION) };
        let mut client = Vec::new();
        let n = ((w - 2 * b) * (h - top - b)) as usize;
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
            docked: false,
            client,
            title: name,
            icon: Vec::new(),
            small_icon: Vec::new(),
            unsnapped: None,
            popup,
            frameless,
            look: 0,
            chrome: X::default(),
        };
        self.windows.try_reserve(1).map_err(|_| NtStatus::NO_MEMORY)?;
        self.windows.try_push(window).map_err(|_| NtStatus::NO_MEMORY)?;
        self.next_id += 1;
        Ok(id)
    }

    /// Shows or hides a window (shown: on top and active).
    pub fn show(&mut self, id: u32, owner: u64, visible: bool) -> Result<Rect, NtStatus> {
        let shadow = self.shadow;
        let win = self.find(id, owner)?;
        let was = win.visible;
        let was_minimized = win.minimized;
        win.visible = visible;
        win.minimized = false;
        let bounds = win.bounds(shadow);
        if visible && !was {
            // The title, ASCII, for the log (screen checks wait for it; not
            // in host unit tests, which have no console).
            #[cfg(not(test))]
            {
                let mut text = [0u8; 64];
                let n = win.title.len().min(text.len());
                for (t, &c) in text.iter_mut().zip(&win.title[..n]) {
                    *t = if (0x20..0x7f).contains(&c) { c as u8 } else { b'?' };
                }
                crate::info!(
                    "desktop: shown window '{}'",
                    core::str::from_utf8(&text[..n]).unwrap_or("?")
                );
            }
            let popup = self.window(id).is_some_and(|w| w.popup);
            self.raise(id);
            if !popup {
                self.active = Some(id);
            }
        }
        if visible && was && was_minimized {
            // Restored (SC_RESTORE of a minimized window): on top and
            // active again.
            self.raise(id);
            self.activate(Some(id));
        }
        if !visible && self.active == Some(id) {
            self.activate_top();
        }
        self.listing_changed = true;
        self.add_damage(bounds);
        Ok(bounds)
    }

    /// Shows a window minimized (GalaWindowShow 2: the program's
    /// SC_MINIMIZE or ShowWindow SW_MINIMIZE): on the taskbar only.
    pub fn show_minimized(&mut self, id: u32, owner: u64) -> Result<Rect, NtStatus> {
        let shadow = self.shadow;
        let win = self.find(id, owner)?;
        win.visible = true;
        let bounds = win.bounds(shadow);
        self.set_minimized(id);
        Ok(bounds)
    }

    /// Copies a w x h rectangle at (x, y) of the client area from `pixels`
    /// (rows of `stride` pixels); the screen area to redraw.
    pub fn present(
        &mut self,
        id: u32,
        r: Rect,
        pixels: &[u32],
        stride: usize,
    ) -> Result<Rect, NtStatus> {
        let win = self.windows.iter_mut().find(|w| w.id == id).ok_or(NtStatus::INVALID_HANDLE)?;
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
        let shadow = self.shadow;
        let win = self.find(id, owner)?;
        let popup = win.popup || win.frameless;
        if !size_fits(w, h, popup) {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let old = win.bounds(shadow);
        if (w, h) != (win.w, win.h) {
            let (ow, oh) = win.client_size();
            let (b, top) = win.frame();
            let (nw, nh) = (w - 2 * b, h - top - b);
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
        }
        win.x = x;
        win.y = y;
        let work = (self.screen.0, self.screen.1 - i64::from(TASKBAR));
        let win = self.find(id, owner)?;
        win.maximized = (x, y) == (0, 0) && (i64::from(w), i64::from(h)) == work && work.0 > 0;
        win.docked = !win.maximized
            && work.0 > 0
            && y == 0
            && i64::from(h) == work.1
            && (x == 0 || i64::from(x) + i64::from(w) == work.0);
        win.restyled();
        let new = win.bounds(shadow);
        self.add_damage(old);
        self.add_damage(new);
        Ok(new)
    }

    /// A window's new title (the caption and the taskbar show it).
    pub fn set_title(&mut self, id: u32, owner: u64, title: &[u16]) -> Result<Rect, NtStatus> {
        let mut name = Vec::new();
        name.try_extend_from_slice(title).map_err(|_| NtStatus::NO_MEMORY)?;
        let win = self.find(id, owner)?;
        win.title = name;
        win.restyled();
        let r = win.caption_rect();
        self.listing_changed = true;
        self.add_damage(r);
        Ok(r)
    }

    /// `owner`'s window `id`'s icon: `size` x `size` 0xAARRGGBB pixels
    /// (none when empty), shown on its taskbar button.
    pub fn set_icon(
        &mut self,
        id: u32,
        owner: u64,
        pixels: &[u32],
        size: usize,
    ) -> Result<(), NtStatus> {
        let (icon, small) = if pixels.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            (
                scale_icon(pixels, size).ok_or(NtStatus::NO_MEMORY)?,
                scale_icon_to(pixels, size, SMALL_ICON).ok_or(NtStatus::NO_MEMORY)?,
            )
        };
        let win = self.find(id, owner)?;
        win.icon = icon;
        win.small_icon = small;
        win.restyled();
        let r = win.caption_rect();
        self.add_damage(r);
        self.listing_changed = true;
        Ok(())
    }

    /// Gives `owner`'s window `id` the mouse capture (0: releases the
    /// capture if one of `owner`'s windows has it).
    pub fn capture(&mut self, id: u32, owner: u64) -> Result<(), NtStatus> {
        if id == 0 {
            if self.captured.and_then(|c| self.window(c)).is_some_and(|w| w.owner == owner) {
                self.captured = None;
            }
            return Ok(());
        }
        self.find(id, owner)?;
        self.captured = Some(id);
        Ok(())
    }

    pub fn destroy(&mut self, id: u32, owner: u64) -> Result<Rect, NtStatus> {
        let i = self.windows.iter().position(|w| w.id == id && w.owner == owner);
        let i = i.ok_or(NtStatus::INVALID_HANDLE)?;
        let bounds = self.windows.remove(i).bounds(self.shadow);
        self.forget(id);
        self.add_damage(bounds);
        Ok(bounds)
    }

    /// Removes `owner`'s windows (and adds their areas to the damage).
    /// A process's end: allocates nothing, so nothing is left behind
    /// (a capture or activation of a window that is gone).
    pub fn destroy_owned(&mut self, owner: u64) {
        let mut i = 0;
        while i < self.windows.len() {
            if self.windows[i].owner != owner {
                i += 1;
                continue;
            }
            let w = self.windows.remove(i);
            self.forget(w.id);
            self.add_damage(w.bounds(self.shadow));
        }
        self.input.retain(|e| e.owner != owner);
        self.listeners.retain(|(o, _)| *o != owner);
        self.woken.retain(|o| *o != owner);
    }

    /// A window went: nothing refers to it any more.
    fn forget(&mut self, id: u32) {
        if self.captured == Some(id) {
            self.captured = None;
        }
        if self.active == Some(id) {
            self.activate_top();
        }
        self.input.retain(|e| e.window != id);
        self.listing_changed = true;
    }

    // ---- Input for programs ----------------------------------------------

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
        for e in &out[..n] {
            if is_noted(e.message) {
                self.settle_note(e.window, e.message, false);
            }
        }
        n
    }

    pub fn has_input(&self, owner: u64) -> bool {
        self.input.iter().any(|e| e.owner == owner)
    }

    /// Queues a message for window `window`'s program (a move replaces the
    /// window's unread one; the oldest goes beyond MAX_INPUT).
    pub fn send(&mut self, window: u32, message: u32, wparam: u64, lparam: u64) {
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
            let lost = self.input.remove(0);
            self.input_dropped += 1;
            if is_noted(lost.message) {
                self.settle_note(lost.window, lost.message, true);
            }
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
    pub fn take_wakes(&mut self) -> Vec<ObjectRef> {
        let mut out = Vec::new();
        for owner in core::mem::take(&mut self.woken) {
            if let Some((_, e)) = self.listeners.iter().find(|(o, _)| *o == owner) {
                let _ = out.try_push(e.clone());
            }
        }
        out
    }

    /// A key went down or up: to the active window as WM_KEYDOWN/WM_KEYUP
    /// (with Alt held, `alt`: WM_SYSKEYDOWN/WM_SYSKEYUP), lParam as Windows
    /// packs it (repeat 1, scan code, extended bit, context and transition
    /// bits). Nothing without an active window.
    pub fn key_to_active(&mut self, vk: u8, scan: u8, extended: bool, down: bool, alt: bool) {
        let Some(id) = self.active else { return };
        let mut lparam = 1u64 | u64::from(scan) << 16 | u64::from(extended) << 24;
        if alt {
            lparam |= 1 << 29;
        }
        if !down {
            lparam |= 3 << 30;
        }
        let message = match (alt || vk == VK_MENU, down) {
            (false, true) => WM_KEYDOWN,
            (false, false) => WM_KEYUP,
            (true, true) => WM_SYSKEYDOWN,
            (true, false) => WM_SYSKEYUP,
        };
        // Where a key went (`events=window`): a program that does not
        // react to one shows whether it had it.
        let owner = self.window(id).map_or(0, |w| w.owner);
        let t = crate::nt::platform::monotonic_ns();
        self.note_input(InputNote {
            t,
            message,
            x: i64::from(vk),
            y: 0,
            window: id,
            captured: false,
            taken: 0,
            dropped: false,
        });
        crate::events::emit(
            crate::events::Category::Window,
            "key",
            format_args!(
                r#""vk":{vk},"down":{down},"message":{message},"window":{id},"owner":{owner}"#
            ),
        );
        self.send(id, message, u64::from(vk), lparam);
    }

    /// Where a mouse button went (`events=window`, as `key` for keys): the
    /// window, the message, the screen point, the owner, and whether a
    /// capture took it. A click a program does not act on shows whether
    /// and where it had it.
    pub fn note_button(&mut self, id: u32, message: u32, x: i64, y: i64, captured: bool) {
        let t = crate::nt::platform::monotonic_ns();
        self.note_input(InputNote {
            t,
            message,
            x,
            y,
            window: id,
            captured,
            taken: 0,
            dropped: false,
        });
        let owner = self.window(id).map_or(0, |w| w.owner);
        crate::events::emit(
            crate::events::Category::Window,
            "button",
            format_args!(
                r#""message":{message},"x":{x},"y":{y},"window":{id},"owner":{owner},"captured":{captured}"#
            ),
        );
    }

    /// Screen point (x, y) in window `id`'s client coordinates, as a
    /// mouse message's lParam.
    pub fn client_lparam(&self, id: u32, x: i64, y: i64) -> u64 {
        let (ox, oy) = self.window(id).map_or((0, 0), |w| w.client_origin());
        let (cx, cy) = ((x - ox) as i16 as u16, (y - oy) as i16 as u16);
        u64::from(cy) << 16 | u64::from(cx)
    }

    // ---- Window management ---------------------------------------------

    /// Does `action` to window `id` (a framed window its program shows):
    /// the one path for the taskbar, the caption buttons, the Snap keys
    /// and GalaDesktopAction. STATUS_INVALID_HANDLE for another window
    /// (none, a popup, a hidden one), STATUS_INVALID_PARAMETER for a size
    /// no window may have.
    pub fn act(&mut self, id: u32, action: Action) -> Result<(), NtStatus> {
        let i = self.index(id).ok_or(NtStatus::INVALID_HANDLE)?;
        let w = &self.windows[i];
        if w.popup || !w.visible {
            return Err(NtStatus::INVALID_HANDLE);
        }
        let (minimized, maximized) = (w.minimized, w.maximized);
        match action {
            Action::Activate if minimized => self.restore(id),
            Action::Activate => {
                self.raise(id);
                self.activate(Some(id));
            }
            Action::Minimize => {
                if !minimized {
                    self.minimize(id);
                }
            }
            Action::Maximize => {
                if minimized {
                    self.restore(id);
                }
                if !maximized {
                    self.send(id, WM_SYSCOMMAND, SC_MAXIMIZE, 0);
                }
                self.raise(id);
                self.activate(Some(id));
            }
            Action::Restore if minimized => self.restore(id),
            Action::Restore if maximized => self.send(id, WM_SYSCOMMAND, SC_RESTORE, 0),
            Action::Restore => {
                if let Some(r) = self.windows[i].unsnapped.take() {
                    self.send_place(id, r);
                }
            }
            Action::Close => self.send(id, WM_SYSCOMMAND, SC_CLOSE, 0),
            Action::Snap(to) => self.snap(id, to)?,
            Action::Place { x, y, w, h } => {
                if !size_fits(w, h, false) {
                    return Err(NtStatus::INVALID_PARAMETER);
                }
                if minimized {
                    self.restore(id);
                }
                // restore() raises it: its index may have changed.
                let i = self.index(id).ok_or(NtStatus::INVALID_HANDLE)?;
                self.windows[i].unsnapped = None;
                self.send_place(id, (x, y, w, h));
            }
        }
        Ok(())
    }

    /// GALA_WM_PLACE: the program moves its window to (x, y, w, h).
    pub fn send_place(&mut self, id: u32, (x, y, w, h): (i32, i32, u32, u32)) {
        self.send(
            id,
            GALA_WM_PLACE,
            // MAKELONG pairs, as WM_MOVE and WM_SIZE: they fit a 32-bit
            // program's WPARAM/LPARAM too (WOW64).
            u64::from(x as i16 as u16) | u64::from(y as i16 as u16) << 16,
            u64::from(w.min(0xffff) as u16) | u64::from(h.min(0xffff) as u16) << 16,
        );
    }

    /// Snap, as Windows 11 does it with the Windows key: Left and Right
    /// put the window on that half of the work area, Up maximizes it,
    /// Down restores it (a maximized or snapped one) or minimizes it.
    /// The program moves its window (GALA_WM_PLACE, WM_SYSCOMMAND).
    fn snap(&mut self, id: u32, to: SnapTo) -> Result<(), NtStatus> {
        let (sw, sh) = self.screen;
        let i = self.index(id).ok_or(NtStatus::INVALID_HANDLE)?;
        if sw <= 0 {
            return Err(NtStatus::NOT_SUPPORTED);
        }
        let w = &self.windows[i];
        let (minimized, maximized) = (w.minimized, w.maximized);
        if minimized {
            // Down leaves a minimized window alone; the others bring it
            // back first.
            if to == SnapTo::Down {
                return Ok(());
            }
            self.restore(id);
        }
        // restore() raises it: its index may have changed.
        let i = self.index(id).ok_or(NtStatus::INVALID_HANDLE)?;
        let work_h = (sh - i64::from(TASKBAR)).max(1) as u32;
        match to {
            SnapTo::Up if !maximized => self.send(id, WM_SYSCOMMAND, SC_MAXIMIZE, 0),
            SnapTo::Up => {}
            SnapTo::Down if maximized => self.send(id, WM_SYSCOMMAND, SC_RESTORE, 0),
            SnapTo::Down => match self.windows[i].unsnapped.take() {
                Some(r) => self.send_place(id, r),
                None => self.minimize(id),
            },
            SnapTo::Left | SnapTo::Right => {
                let half = (sw / 2) as u32;
                let (x, width) =
                    if to == SnapTo::Left { (0, half) } else { (half as i32, sw as u32 - half) };
                let w = &self.windows[i];
                if w.unsnapped.is_none() && !maximized {
                    self.windows[i].unsnapped = Some((w.x, w.y, w.w, w.h));
                }
                self.send_place(id, (x, 0, width, work_h));
            }
        }
        if to != SnapTo::Down {
            self.raise(id);
            self.activate(Some(id));
        }
        Ok(())
    }

    /// The framed windows their programs show (shown or minimized), in
    /// the order they were made: the taskbar's buttons.
    pub fn visible_windows(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = Vec::new();
        for w in self.windows.iter().filter(|w| w.visible && !w.popup) {
            let _ = ids.try_push(w.id);
        }
        ids.sort_unstable();
        ids
    }

    /// The windows the taskbar shows (in its order).
    pub fn list(&self) -> Vec<Listed> {
        let mut out = Vec::new();
        let work = (self.screen.0, self.screen.1 - i64::from(TASKBAR));
        for id in self.visible_windows() {
            let Some(w) = self.window(id) else { continue };
            let mut flags = 0;
            if w.minimized {
                flags |= LISTED_MINIMIZED;
            }
            if w.maximized {
                flags |= LISTED_MAXIMIZED;
            }
            if self.active == Some(id) {
                flags |= LISTED_ACTIVE;
            }
            if w.docked && work.0 > 0 {
                flags |= LISTED_SNAPPED;
            }
            let mut title = [0u16; LISTED_TITLE];
            let n = w.title.len().min(LISTED_TITLE);
            title[..n].copy_from_slice(&w.title[..n]);
            let entry = Listed {
                id,
                owner: w.owner,
                flags,
                rect: [w.x, w.y, w.w as i32, w.h as i32],
                title,
                title_len: n,
            };
            if out.try_push(entry).is_err() {
                break;
            }
        }
        out
    }

    /// To the top of its layer: framed windows stay below the popups.
    pub fn raise(&mut self, id: u32) {
        let Some(i) = self.index(id) else { return };
        let w = self.windows.remove(i);
        let bounds = w.bounds(self.shadow);
        let at = if w.popup {
            self.windows.len()
        } else {
            self.windows.iter().position(|o| o.popup).unwrap_or(self.windows.len())
        };
        // Back in (the capacity it left), then into place.
        let _ = self.windows.try_push(w);
        self.windows[at..].rotate_right(1);
        if at != i {
            self.add_damage(bounds);
        }
    }

    pub fn activate(&mut self, id: Option<u32>) {
        if self.active != id {
            self.active = id;
            self.listing_changed = true;
        }
    }

    /// The topmost shown window becomes active (or none).
    pub fn activate_top(&mut self) {
        let top = self.windows.iter().rev().find(|w| w.shown() && !w.popup).map(|w| w.id);
        self.activate(top);
    }

    /// The user minimizes a window: its program is told
    /// (GALA_WM_MINIMIZED), so IsIconic says so.
    fn minimize(&mut self, id: u32) {
        self.set_minimized(id);
        self.send(id, GALA_WM_MINIMIZED, 1, 0);
    }

    fn set_minimized(&mut self, id: u32) {
        let Some(i) = self.index(id) else { return };
        self.windows[i].minimized = true;
        let bounds = self.windows[i].bounds(self.shadow);
        self.add_damage(bounds);
        if self.active == Some(id) {
            self.activate_top();
        }
        self.listing_changed = true;
    }

    /// The user restores a minimized window (its taskbar button).
    fn restore(&mut self, id: u32) {
        let Some(i) = self.index(id) else { return };
        self.send(id, GALA_WM_MINIMIZED, 0, 0);
        self.windows[i].minimized = false;
        let bounds = self.windows[i].bounds(self.shadow);
        self.add_damage(bounds);
        self.raise(id);
        self.activate(Some(id));
        self.listing_changed = true;
    }

    /// Moves a window to (x, y), its caption kept on the screen.
    pub fn move_to(&mut self, id: u32, x: i64, y: i64) {
        let (sw, sh) = self.screen;
        let shadow = self.shadow;
        let Some(i) = self.index(id) else { return };
        let w = &self.windows[i];
        let ww = i64::from(w.w);
        let x = x.clamp(-ww + 80, (sw - 80).max(0));
        let y = y.clamp(0, (sh - i64::from(TASKBAR) - i64::from(CAPTION)).max(0));
        if (x, y) == (i64::from(w.x), i64::from(w.y)) {
            return;
        }
        let old = w.bounds(shadow);
        let w = &mut self.windows[i];
        w.x = x as i32;
        w.y = y as i32;
        let new = w.bounds(shadow);
        self.add_damage(old);
        self.add_damage(new);
    }
}

#[cfg(test)]
mod tests;
