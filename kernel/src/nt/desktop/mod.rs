//! The desktop: the top-level windows of Win32 programs (and of Linux
//! clients, through the Wayland server), composited on the screen with
//! the mouse pointer. The shell is a program: shasha (user/shasha) draws
//! the taskbar, the start menu and the wallpaper in windows of its own
//! (ARCHITECTURE "The user-mode shell"); the kernel draws none of them.
//!
//! The kernel keeps each window's frame (rounded corners, a shaded caption
//! with the title and minimize/close buttons, drawn here) and a copy of its
//! client area, which the program's user32/gdi32 draw in their own memory
//! and present (`present`). Every pixel is composed in layers: the
//! desktop's colour (DESKTOP_COLOR: all a session without a shell shows
//! behind its windows; shasha's desktop window covers it), the windows
//! bottom to top (each with a soft shadow) and the pointer. Changes
//! collect damage rectangles which `with`/`pointer_event`/the display
//! watch compose and hand to the desktop's presenter (`present`: the
//! screen, a frame in memory, or nothing; the windows and their pixels
//! are kept either way). The window state itself (windows, Z order,
//! focus, capture, client pixels, input for programs) is
//! `nt::winstate`'s, which a bare desktop is alone; this module is the
//! compositor on top.
//!
//! The mouse: a click raises and activates a window, its caption drags
//! it, the caption buttons minimize it or ask it to close (WM_CLOSE to the
//! program). Clicks and moves in a client area become input for its
//! program. The keys the system keeps for itself: Ctrl+Shift+Esc (Task
//! Manager) and Win+arrows (Snap); the Windows key alone is a hot key
//! (shasha's start menu registers it).
//!
//! What the shell does to windows is a named [`Action`] (`act`): the
//! caption buttons and the Snap keys call it, and so does
//! GalaDesktopAction (`nt::action`: shasha's taskbar, the `gala`
//! command). The desktop also keeps what shasha's start menu shows of the
//! installs it asked for (the catalog's packages, the Linux
//! distributions): where each is, watched until its installer's last word,
//! with WM_GALA_CATALOG to the shell's window when one changes.
//! Design: docs/ARCHITECTURE.md, "Windows on the screen", "Server and
//! desktop workloads" and "Presenters (B3a)".

pub mod appbar;
pub mod linux;
mod paint;
pub mod pkg;
mod present;

use super::object::ObjectRef;
use super::platform;
use super::status::NtStatus;
/// The window state's names, as the desktop's (the system calls, the
/// named actions use them).
pub use super::winstate::{
    Action, BORDER, CAPTION, GALA_WM_ACTIVE, GALA_WM_MINIMIZED, GALA_WM_MOUSEACTIVATE,
    GALA_WM_PLACE, GALA_WM_POINTER_LEFT, HTCAPTION, InputEvent, LISTED_ACTIVE, LISTED_MAXIMIZED,
    LISTED_MINIMIZED, LISTED_SNAPPED, LISTED_TITLE, Listed, Listener, Rect, SC_CLOSE, SC_MAXIMIZE,
    SC_RESTORE, SMALL_ICON, STYLE_FRAMELESS, STYLE_POPUP, SnapTo, TASKBAR, WHEEL_DELTA, WM_CLOSE,
    WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_MOVE,
    WM_NCRBUTTONDOWN, WM_NCRBUTTONUP, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSCOMMAND, WM_SYSKEYDOWN,
    WM_SYSKEYUP, scale_icon,
};
use super::winstate::{CAPTION_BUTTON, Shadow, State, VK_MENU, Window};
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;
use alloc::vec::Vec;
use paint::{Canvas, Font, blend, round_coverage2, shadow_alpha};
pub use present::{DEFAULT_SIZE, Frame, Kind, MAX_SIDE as MAX_FRAME_SIDE, Presenter};

/// Corner radii of a window (Windows 11's 8 pixels): above and below.
const RADIUS: i64 = 8;
const RADIUS_BOTTOM: i64 = 8;
/// Soft shadows (Windows 11's are wide and light): reach, drop, darkness.
const SHADOW_REACH: i64 = 26;
const SHADOW_DROP: i64 = 6;
const SHADOW_ACTIVE: i64 = 80;
const SHADOW_INACTIVE: i64 = 45;
/// What shows behind the windows: the whole screen of a session no shell
/// draws on (shasha's desktop window covers it with the wallpaper).
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

/// A start menu entry: one of gala's own programs, or a catalog entry
/// (`package`) installed on a click (ROADMAP SM).
pub struct Program {
    pub name: &'static str,
    /// Where its program is, in order: Win32 paths whose `%NAME%`s are
    /// the boot user's environment variables
    /// (`process::expand_environment`). The first that exists is started.
    pub paths: &'static [&'static str],
    /// The winget package that installs it (`gala install <id>`); None
    /// for gala's own programs, listed only when present.
    pub package: Option<&'static str>,
}

/// The start menu's programs, in the order it shows them: gala's own
/// default programs first, then the catalog (installed or not). The
/// catalog's paths are where each package's installer puts the program,
/// as its winget-pkgs manifest says (Scope, InstallLocation,
/// DefaultInstallLocation) or, when the manifest names none, the
/// installer's documented default for that scope: the user scope's under
/// %LOCALAPPDATA% first (what `gala install` picks for the boot user, who
/// is no administrator), then the machine scope's.
pub const PROGRAMS: &[Program] = &[
    // gala's cmd.exe (user/cmd): System32 as on Windows, C:\gala in
    // images that ship it as a boot module.
    Program {
        name: "Command Prompt",
        paths: &["%SystemRoot%\\System32\\cmd.exe", "C:\\gala\\cmd.exe"],
        package: None,
    },
    // gala's test window (gui_test), when the image has it.
    Program {
        name: "Hello window",
        paths: &["C:\\gui_test.exe", "C:\\gala\\gui_test.exe"],
        package: None,
    },
    // gala's wsl.exe (user/wsl) as the "Linux" tile (gala's UI says
    // Linux, never WSL: ARCHITECTURE "The WSL facade"): the default
    // distribution's login shell in a console window (ROADMAP CT).
    Program { name: "Linux", paths: &["%SystemRoot%\\System32\\wsl.exe"], package: None },
    // gala's taskmgr.exe (user/taskmgr: ROADMAP TM), where Windows has
    // its Task Manager; also Ctrl+Shift+Esc (`TASK_MANAGER`).
    Program { name: TASK_MANAGER, paths: &["%SystemRoot%\\System32\\Taskmgr.exe"], package: None },
    // gala's explorer.exe (user/explorer: ROADMAP EX), where Windows has
    // it (Win+E is shasha's hot key).
    Program { name: "File Explorer", paths: &["%SystemRoot%\\explorer.exe"], package: None },
    // The Office Deployment Tool's setup.exe (machine scope): Click-to-Run
    // puts Word under root\Office16 (ROADMAP OF).
    Program {
        name: "Microsoft Office",
        paths: &["%ProgramFiles%\\Microsoft Office\\root\\Office16\\WINWORD.EXE"],
        package: Some("Microsoft.Office"),
    },
    // FileZilla's NSIS installer's default directory, and the portable
    // placement of the `filezilla` scenarios. winget-pkgs has no
    // TimKosse.FileZilla.Client at present: `gala install` says so.
    Program {
        name: "FileZilla",
        paths: &[
            "%ProgramFiles%\\FileZilla FTP Client\\filezilla.exe",
            "C:\\filezilla\\filezilla.exe",
        ],
        package: Some("TimKosse.FileZilla.Client"),
    },
    // The .NET SDK's bundle (burn, machine scope).
    Program {
        name: ".NET",
        paths: &["%ProgramFiles%\\dotnet\\dotnet.exe"],
        package: Some("Microsoft.DotNet.SDK.9"),
    },
    // python.org's installer (burn): DefaultJustForMeTargetDir for the
    // user scope, DefaultAllUsersTargetDir for the machine's.
    Program {
        name: "Python",
        paths: &[
            "%LOCALAPPDATA%\\Programs\\Python\\Python313\\python.exe",
            "%ProgramFiles%\\Python313\\python.exe",
        ],
        package: Some("Python.Python.3.13"),
    },
    // Chromium's mini_installer: per user under %LOCALAPPDATA%, with
    // --system-level (the machine scope) under Program Files.
    Program {
        name: "Chromium",
        paths: &[
            "%LOCALAPPDATA%\\Chromium\\Application\\chrome.exe",
            "%ProgramFiles%\\Chromium\\Application\\chrome.exe",
        ],
        package: Some("Hibbiki.Chromium"),
    },
    // Git for Windows' Inno Setup installer: {autopf}\Git, which is
    // %LOCALAPPDATA%\Programs for a user install.
    Program {
        name: "Git",
        paths: &[
            "%LOCALAPPDATA%\\Programs\\Git\\git-bash.exe",
            "%ProgramFiles%\\Git\\git-bash.exe",
        ],
        package: Some("Git.Git"),
    },
    // 7-Zip's installer (machine scope; the manifest's
    // DefaultInstallLocation %ProgramFiles%\7-Zip).
    Program {
        name: "7-Zip",
        paths: &["%ProgramFiles%\\7-Zip\\7zFM.exe"],
        package: Some("7zip.7zip"),
    },
];

/// The Task Manager's entry in PROGRAMS (Ctrl+Shift+Esc starts it).
pub const TASK_MANAGER: &str = "Task Manager";

/// Its index in PROGRAMS.
fn task_manager() -> Option<usize> {
    PROGRAMS.iter().position(|p| p.name == TASK_MANAGER)
}

/// How many entries the program list holds (`list_programs`): more
/// than PROGRAMS has.
pub const MAX_TILES: usize = 24;

/// Whose install an installer is running (`Desktop::installers`): a
/// catalog package's tile, or a Linux distribution's.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Installing {
    Package(usize),
    Linux(linux::Name),
}

/// How many installs are watched at once. The catalog has fewer
/// programs than this and the Linux section fewer tiles, so every
/// install a user can start is watched; one past it is not (logged),
/// and its tile waits for the next listing as it did before the watch.
const MAX_INSTALLERS: usize = 32;

/// Why a tile's install has no word left to come (`install_quiet`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Quiet {
    /// Its installer never started: no gala.exe, no memory for the
    /// command line, or the program is not on this machine and has no
    /// package to install.
    NeverStarted,
    /// Its installer is gone without a last word: killed outright, or
    /// it faulted before it could report.
    Gone,
}

/// An install that went quiet, as the log and the event tell it
/// (`Desktop::take_quiet`): whose it was, how far the bar got, and why
/// no word is coming. Whose, rather than the name: a distribution's
/// name is in its `Name`, not a static string.
struct Quieted {
    who: Installing,
    at: u8,
    why: Quiet,
}

impl Installing {
    /// The name on the tile, for what is told of it.
    fn name(&self) -> &str {
        match self {
            Installing::Package(p) => PROGRAMS.get(*p).map(|e| e.name).unwrap_or("A program"),
            Installing::Linux(n) => n.as_str(),
        }
    }
}

impl Quiet {
    /// What the log and the ledger say.
    fn as_str(self) -> &'static str {
        match self {
            Quiet::NeverStarted => "its installer never started",
            Quiet::Gone => "its installer is gone with no word",
        }
    }

    /// What the notification says under the program's name.
    fn told(self) -> &'static str {
        match self {
            Quiet::NeverStarted => "The install never started. A click tries again.",
            Quiet::Gone => {
                "The install went quiet: its installer is gone with no word. A click tries again."
            }
        }
    }
}

/// A start menu tile: a program (an index into its table) and whether it
/// is installed (one of its paths exists).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub program: usize,
    pub installed: bool,
}

impl Program {
    /// The first of its paths that exists (`exists` gets each expanded:
    /// `process::program_exists`), as its index and the expanded path in
    /// `buf`.
    pub fn find<'b>(
        &self,
        exists: &mut impl FnMut(&str) -> bool,
        buf: &'b mut [u8; 260],
    ) -> Option<(usize, &'b str)> {
        let mut found = None;
        for (i, raw) in self.paths.iter().enumerate() {
            let mut b = [0u8; 260];
            if let Some(path) = super::process::expand_environment(raw, &mut b)
                && exists(path)
            {
                found = Some((i, path.len()));
                buf[..path.len()].copy_from_slice(path.as_bytes());
                break;
            }
        }
        let (i, n) = found?;
        Some((i, core::str::from_utf8(&buf[..n]).ok()?))
    }
}

/// The tiles for `programs`, one per name (the first entry of a name
/// wins): gala's own programs whose file exists, every catalog entry,
/// installed or not; how many. At most MAX_TILES. Reads the file system
/// through `exists`: not under the desktop's lock.
pub fn list_programs(
    programs: &[Program],
    exists: &mut impl FnMut(&str) -> bool,
) -> ([Entry; MAX_TILES], usize) {
    let mut out = [Entry { program: 0, installed: false }; MAX_TILES];
    let mut n = 0;
    for (i, p) in programs.iter().enumerate() {
        if n == MAX_TILES || out[..n].iter().any(|e| programs[e.program].name == p.name) {
            continue;
        }
        let installed = p.find(exists, &mut [0; 260]).is_some();
        if installed || p.package.is_some() {
            out[n] = Entry { program: i, installed };
            n += 1;
        }
    }
    (out, n)
}

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

/// A `window` event `pointer` for each change of the buttons the
/// desktop takes (`pressed` and `released`: bit 0 left, bit 1 right) at
/// (x, y), with what got it (`to`: the capturing window, else what was
/// under the pointer): a screen check that finds the screen behind tells
/// from these whether the guest saw its click, and where (QEMU's PS/2
/// mouse merges a press it has no room for with the release after it:
/// WORKFLOW "Screen checks").
fn note_buttons(pressed: u8, released: u8, x: i64, y: i64, to: core::fmt::Arguments<'_>) {
    if pressed | released == 0 {
        return;
    }
    crate::events::emit(
        crate::events::Category::Window,
        "pointer",
        format_args!(r#""pressed":{pressed},"released":{released},"x":{x},"y":{y},"to":"{to}""#),
    );
}

/// What is under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Nothing,
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

/// A session's desktop: the window state (`nt::winstate`: windows, Z
/// order, focus, capture, client pixels, input for programs) and, on top
/// of it, the compositor's own: the presenter, the pointer, what the
/// mouse and keys are doing, the app bars and the installs shasha's start
/// menu asked for.
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
    /// kernel's (no captions or input), shown by none.
    bare: Option<&'static str>,
    /// The mouse pointer's hot spot and the buttons held (bit 0 left,
    /// bit 1 right).
    pointer: (i64, i64),
    buttons: u8,
    /// Buttons whose press ended a popup grab: their release goes
    /// nowhere either.
    swallowed: u8,
    /// The popup the last press went to (its client area, no capture):
    /// its program may take the activation for that press once
    /// (`activate_for_press`, user32's WM_MOUSEACTIVATE).
    pressed_popup: Option<u32>,
    hover: Hit,
    /// Where the left button went down.
    pressed: Hit,
    /// A window being dragged by its caption: (id, grab offset).
    drag: Option<(u32, i64, i64)>,
    /// A resize by a program's own border (sent as GALA_WM_PLACE when the
    /// button goes up).
    resize: Option<Resize>,
    /// A program a system key asked to start (Ctrl+Shift+Esc: Task
    /// Manager; an index into PROGRAMS).
    launch: Option<usize>,
    /// The installs a tile asked for (`gala install <id>`, no window),
    /// each a PROGRAMS index and where its install is.
    installs: Vec<(usize, pkg::Progress)>,
    /// The installer each install is waiting for a word from, by pid
    /// (`install_started`), packages and distributions alike: a process
    /// that ends without a word (`installers_gone`) leaves its tile
    /// quiet instead of waiting for ever.
    installers: Vec<(Installing, u64)>,
    /// The installs that went quiet, for the log and the event the free
    /// functions write (`take_quiet`): a `Desktop` method must not log,
    /// since host unit tests build without the `hosted` feature, where
    /// the console writes to the serial port. Every caller drains it in
    /// the same breath, so it holds one call's worth.
    quieted: Vec<Quieted>,
    /// How a catalog tile's install changed since the shell's window was
    /// last told (`take_catalog_changed`): a package install began,
    /// moved on, ended or went quiet. The free functions that change
    /// one tell shasha's window after the lock (`tell_catalog`), as a
    /// method may not post or log.
    catalog_changed: CatalogChange,
    /// The same for the Linux section's tiles (`take_linux_changed`,
    /// `tell_linux`): an install began, moved on, ended or went quiet,
    /// or the tiles listed changed.
    linux_changed: CatalogChange,
    /// The "Linux distributions" section's tiles as last listed (to tell
    /// shasha's window when they change) and the installs under way.
    linux_tiles: Vec<linux::Distro>,
    progress: Vec<(linux::Name, linux::Progress)>,
    /// Alt is held (WM_SYSKEYDOWN); Ctrl and Shift (Ctrl+Shift+Esc).
    alt: bool,
    ctrl: bool,
    shift: bool,
    /// The Windows key is held, and whether a key went with it (then its
    /// release is no hot key).
    win_key: bool,
    win_used: bool,
    /// The key whose down a hot key took and that is still held: its
    /// repeats are the same hot key's (MOD_NOREPEAT ignores them).
    held_hot: Option<u8>,
    /// The app bars (SHAppBarMessage): the strips they take from the
    /// work area (`s.reserved`).
    appbars: appbar::AppBars,
    /// What `announce` told (the unit tests read it).
    #[cfg(test)]
    told: Vec<(Vec<u16>, Vec<u16>)>,
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
            pointer: (0, 0),
            buttons: 0,
            swallowed: 0,
            pressed_popup: None,
            hover: Hit::Nothing,
            pressed: Hit::Nothing,
            drag: None,
            resize: None,
            launch: None,
            linux_tiles: Vec::new(),
            installs: Vec::new(),
            installers: Vec::new(),
            quieted: Vec::new(),
            catalog_changed: CatalogChange::None,
            linux_changed: CatalogChange::None,
            progress: Vec::new(),
            alt: false,
            ctrl: false,
            shift: false,
            win_key: false,
            win_used: false,
            held_hot: None,
            appbars: appbar::AppBars::new(),
            #[cfg(test)]
            told: Vec::new(),
        }
    }

    /// The screen is w x h: reserves the taskbar's strip and puts the
    /// pointer in the middle.
    pub fn set_screen(&mut self, w: u32, h: u32) -> Result<(), NtStatus> {
        let (w, h) = (i64::from(w), i64::from(h));
        self.s.screen = (w, h);
        self.pointer = (w / 2, h / 2);
        self.place_taskbar();
        self.refresh();
        Ok(())
    }

    /// The taskbar's strip, TASKBAR rows at the bottom of the screen,
    /// reserved as an app bar (the work area is what the bars leave):
    /// the shell's bar's once it took it (ABM_GALA_TASKBAR), else window
    /// 0's, which nothing draws, so that maximized windows leave room for
    /// the taskbar a shell will put there.
    fn place_taskbar(&mut self) {
        let (w, h) = self.s.screen;
        let tb = i64::from(TASKBAR);
        let strip = Rect { x: 0, y: h - tb, w, h: tb };
        let placed = match self.appbars.taskbar_window() {
            // The shell's bar keeps being the taskbar, on the new strip.
            Some(shell) if shell != appbar::TASKBAR_WINDOW => {
                self.appbars.set(shell, appbar::ABE_BOTTOM, strip, (w, h))
            }
            _ => {
                let (owner, window) = (0, appbar::TASKBAR_WINDOW);
                self.appbars.place_taskbar(owner, window, appbar::ABE_BOTTOM, strip, (w, h))
            }
        };
        #[cfg(not(test))]
        match placed {
            Some(r) => crate::info!(
                "desktop: the taskbar is an app bar on the bottom edge at {},{} {}x{}",
                r.x,
                r.y,
                r.w,
                r.h
            ),
            // Without its strip the work area takes the whole screen and
            // maximized windows go under the taskbar.
            None => crate::warn!("desktop: the taskbar's strip is not reserved (no memory)"),
        }
        #[cfg(test)]
        let _ = placed;
        self.appbars.take_posts();
        self.work_changed();
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
    /// released. The areas to redraw.
    pub fn leave_console(&mut self) -> Vec<Rect> {
        let damage = if self.buttons != 0 { self.pointer(0, 0, 0) } else { Vec::new() };
        (self.alt, self.win_key, self.win_used) = (false, false, false);
        self.held_hot = None;
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

    /// Window `id`'s program name (`program_name`).
    pub fn set_program(&mut self, id: u32, owner: u64, name: &[u16]) -> Result<(), NtStatus> {
        self.s.set_program(id, owner, name)
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

    /// Gives `owner`'s window `id` the popup grab (0: ends `owner`'s):
    /// `State::set_popup_grab`.
    pub fn popup_grab(&mut self, id: u32, owner: u64) -> Result<(), NtStatus> {
        self.s.set_popup_grab(id, owner)
    }

    /// `owner`'s window `id`'s client area's screen position.
    pub fn client_origin(&self, id: u32, owner: u64) -> Option<(i64, i64)> {
        self.s.owned(id, owner).map(|w| w.client_origin())
    }

    /// `owner`'s window `id`'s outer rectangle (x, y, width, height).
    pub fn outer(&self, id: u32, owner: u64) -> Option<(i32, i32, u32, u32)> {
        self.s.owned(id, owner).map(|w| (w.x, w.y, w.w, w.h))
    }

    /// `owner`'s window `id`'s place and state, as the window registry
    /// records a served owner's window (ARCHITECTURE "Linux windows in
    /// the window registry").
    pub fn placement(&self, id: u32, owner: u64) -> Option<Placement> {
        self.s.owned(id, owner).map(|w| Placement {
            x: w.x,
            y: w.y,
            w: w.w,
            h: w.h,
            visible: w.visible,
            minimized: w.minimized,
            maximized: w.maximized,
        })
    }

    /// A window's new title (the caption and the taskbar show it).
    pub fn set_title(&mut self, id: u32, owner: u64, title: &[u16]) -> Result<Rect, NtStatus> {
        let r = self.s.set_title(id, owner, title)?;
        self.refresh();
        Ok(r)
    }

    /// A window's icon, shown on its caption.
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
        if self.appbars.drop_owned(owner) == appbar::Changed::Yes {
            #[cfg(not(test))]
            crate::info!("desktop: app bars of pid {owner} removed (its end)");
            self.taskbar_back();
            self.work_changed();
        }
        self.forget_gone();
        self.refresh();
    }

    // ---- App bars -----------------------------------------------------------

    /// SHAppBarMessage's `op` (appbar::ABM_*) for window `hwnd` of
    /// process `owner`: its result (TRUE or FALSE), the rectangle
    /// QUERYPOS, SETPOS and GETTASKBARPOS give back and the edge
    /// GETTASKBARPOS gives. The taskbar's bar (window 0) is the
    /// desktop's: a program's NEW, REMOVE, QUERYPOS or SETPOS of it is
    /// FALSE.
    pub fn appbar_op(
        &mut self,
        owner: u64,
        op: u32,
        hwnd: u64,
        callback: u32,
        edge: u32,
        rect: Rect,
    ) -> Result<(bool, u32, Rect), NtStatus> {
        let screen = self.s.screen;
        if op == appbar::ABM_GETTASKBARPOS {
            return Ok(match self.appbars.taskbar() {
                Some((e, r)) => (true, e, r),
                None => (false, edge, rect),
            });
        }
        if op == appbar::ABM_GALA_TASKBAR {
            // The syscall checked that `hwnd` is the session's shell
            // window and the caller's.
            return Ok(match self.appbars.hand_taskbar(owner, hwnd, callback) {
                Some((e, r)) => {
                    #[cfg(not(test))]
                    crate::info!(
                        "desktop: the taskbar's bar is window {hwnd:#x} of pid {owner} (the \
                         shell's) at {},{} {}x{}",
                        r.x,
                        r.y,
                        r.w,
                        r.h
                    );
                    self.s.add_damage(r);
                    self.refresh();
                    (true, e, r)
                }
                None => {
                    #[cfg(not(test))]
                    crate::info!(
                        "desktop: pid {owner}: the taskbar not handed to window {hwnd:#x} \
                         (none placed, or that window is another bar)"
                    );
                    (false, edge, rect)
                }
            });
        }
        if hwnd == appbar::TASKBAR_WINDOW && op <= appbar::ABM_SETPOS {
            #[cfg(not(test))]
            crate::info!("desktop: pid {owner}: app bar call {op} on the taskbar's bar refused");
            return Ok((false, edge, rect));
        }
        let (ok, rect) = self.own_appbar_op(owner, op, hwnd, callback, edge, rect, screen)?;
        Ok((ok, edge, rect))
    }

    /// `appbar_op` for a program's own bar.
    #[allow(clippy::too_many_arguments)]
    fn own_appbar_op(
        &mut self,
        owner: u64,
        op: u32,
        hwnd: u64,
        callback: u32,
        edge: u32,
        rect: Rect,
        screen: (i64, i64),
    ) -> Result<(bool, Rect), NtStatus> {
        match op {
            appbar::ABM_NEW => {
                let new = self.appbars.add(owner, hwnd, callback)?;
                if new {
                    crate::info!(
                        "desktop: app bar registered: window {hwnd:#x}, pid {owner} ({} bars)",
                        self.appbars.len()
                    );
                }
                Ok((new, rect))
            }
            appbar::ABM_REMOVE => {
                let (was, changed) = self.appbars.remove(hwnd);
                if was {
                    crate::info!(
                        "desktop: app bar removed: window {hwnd:#x}, pid {owner} ({} bars)",
                        self.appbars.len()
                    );
                }
                if changed == appbar::Changed::Yes {
                    self.taskbar_back();
                    self.work_changed();
                }
                Ok((true, rect))
            }
            appbar::ABM_QUERYPOS => match self.appbars.query(hwnd, edge, rect, screen) {
                Some(r) => Ok((true, r)),
                None => Ok((false, rect)),
            },
            appbar::ABM_SETPOS => match self.appbars.set(hwnd, edge, rect, screen) {
                Some(r) => {
                    crate::info!(
                        "desktop: app bar placed: window {hwnd:#x} on edge {edge} at {},{} {}x{}",
                        r.x,
                        r.y,
                        r.w,
                        r.h
                    );
                    self.work_changed();
                    Ok((true, r))
                }
                None => Ok((false, rect)),
            },
            _ => Err(NtStatus::INVALID_PARAMETER),
        }
    }

    /// The app bars (the taskbar's too) changed what they take: the work
    /// area follows and the maximized windows are fitted to it (as
    /// Windows' shell refits them).
    fn work_changed(&mut self) {
        let reserved = self.appbars.insets(self.s.screen);
        if reserved == self.s.reserved {
            return;
        }
        self.s.reserved = reserved;
        let work = self.s.work();
        #[cfg(not(test))]
        self.log_work(work);
        for id in self.s.visible_windows() {
            if self.s.window(id).is_some_and(|w| w.maximized && !w.minimized) {
                #[cfg(not(test))]
                crate::info!("desktop: maximized window {id} fitted to the work area");
                self.s.send_place(id, (work.x as i32, work.y as i32, work.w as u32, work.h as u32));
            }
        }
        self.refresh();
    }

    /// What the bars reserve and the work area left (the log's trail of
    /// a work area change).
    #[cfg(not(test))]
    fn log_work(&self, work: Rect) {
        let edges = ["left", "top", "right", "bottom"];
        for (i, v) in self.s.reserved.iter().enumerate() {
            if *v > 0 {
                crate::info!(
                    "desktop: work area changed: {} edge reserved by {} app bar(s), {v} px",
                    edges[i],
                    self.appbars.on(i as u32)
                );
            }
        }
        crate::info!(
            "desktop: work area [{},{},{},{}]",
            work.x,
            work.y,
            work.x + work.w,
            work.y + work.h
        );
    }

    fn take_appbar_posts(&mut self) -> Vec<appbar::Post> {
        self.appbars.take_posts()
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
    /// caption buttons, the Snap keys and GalaDesktopAction), then draws
    /// what changed.
    pub fn act(&mut self, id: u32, action: Action) -> Result<(), NtStatus> {
        self.s.act(id, action)?;
        self.refresh();
        Ok(())
    }

    /// The hot key (modifiers, key, repeat) key `vk` going `down` (or up)
    /// would be, if any (`window::Windows::hot_key` says whether a process
    /// holds it): a key other than a modifier going down with the
    /// modifiers held, or the Windows key going up with no other key
    /// since it went down ((MOD_WIN, VK_LWIN or VK_RWIN): gala's hot key
    /// for the Windows key alone, ARCHITECTURE "Hot keys").
    pub fn hot_candidate(&self, vk: u8, down: bool) -> Option<(u32, u32, bool)> {
        use crate::nt::window::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};
        if vk == 0x5b || vk == 0x5c {
            return (!down && self.win_key && !self.win_used).then_some((
                MOD_WIN,
                u32::from(vk),
                false,
            ));
        }
        if !down || matches!(vk, 0x10..=0x12 | 0xa0..=0xa5) {
            return None;
        }
        let mods = [
            (self.alt, MOD_ALT),
            (self.ctrl, MOD_CONTROL),
            (self.shift, MOD_SHIFT),
            (self.win_key, MOD_WIN),
        ]
        .iter()
        .filter(|(held, _)| *held)
        .fold(0, |m, (_, bit)| m | bit);
        Some((mods, u32::from(vk), self.held_hot == Some(vk)))
    }

    /// A hot key took key `vk`'s down (or the Windows key's release): the
    /// Windows key's release is then no hot key of its own, and the key's
    /// repeats are known as such until it goes up.
    pub fn hot_key_taken(&mut self, vk: u8, down: bool) {
        if self.win_key {
            self.win_used = true;
        }
        if down {
            self.held_hot = Some(vk);
        }
    }

    /// A key went down or up: virtual key `vk`, `scan` code, `extended`
    /// (E0-prefixed). The system's keys (Ctrl+Shift+Esc, the Windows key,
    /// Win+arrows) are the desktop's; the others go to the active window
    /// (`State::key_to_active`).
    pub fn key(&mut self, vk: u8, scan: u8, extended: bool, down: bool) {
        if !down && self.held_hot == Some(vk) {
            self.held_hot = None;
        }
        if vk == VK_MENU {
            self.alt = down;
        }
        if vk == 0x11 {
            self.ctrl = down;
        }
        if vk == 0x10 {
            self.shift = down;
        }
        // Ctrl+Shift+Esc: Task Manager, whatever has the focus (no program
        // sees the Esc), the system's whether a shell runs or not
        // (`window::system_key`).
        if vk == 0x1b && self.ctrl && self.shift {
            if down {
                self.launch = task_manager();
            }
            return;
        }
        // The Windows key is the system's: no program sees it. Alone, its
        // release is a hot key (`hot_candidate`: shasha's start menu);
        // with the arrows it snaps the active window.
        if vk == 0x5b || vk == 0x5c {
            if down {
                (self.win_key, self.win_used) = (true, false);
            } else {
                self.win_key = false;
            }
            return;
        }
        if self.win_key && down {
            self.win_used = true;
        }
        if self.win_key && (0x25..=0x28).contains(&vk) {
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

    // ---- The taskbar's strip -------------------------------------------

    /// The shell's taskbar is gone (its bar removed, its process ended):
    /// its strip is reserved for window 0 again (`place_taskbar`), so the
    /// work area keeps room for the next shell's taskbar. Nothing is drawn
    /// there: without a shell the screen is the desktop's colour and the
    /// windows.
    fn taskbar_back(&mut self) {
        if self.appbars.taskbar_window().is_some() || self.s.screen.0 == 0 || self.bare.is_some() {
            return;
        }
        #[cfg(not(test))]
        crate::info!(
            "desktop: the taskbar's bar is gone: no shell draws a taskbar; its strip stays reserved"
        );
        self.place_taskbar();
    }

    fn cursor_rect(&self) -> Rect {
        Rect { x: self.pointer.0, y: self.pointer.1, w: 14, h: CURSOR.len() as i64 + 2 }
    }

    /// The programs on this machine now (`list_programs` of PROGRAMS):
    /// an install whose program is there is over.
    pub fn set_programs(&mut self, programs: &[Entry]) {
        let was = self.installs.len();
        // Which of them the listing ends, before it ends them: a
        // `retain` closure must not allocate, since `Vec::retain` itself
        // is infallible either way (`cargo xtask alloc-audit`).
        let listed = |p: usize| programs.iter().any(|e| e.program == p && e.installed);
        let mut done: Vec<usize> = Vec::new();
        for (p, s) in self.installs.iter() {
            if listed(*p) && !matches!(s, pkg::Progress::Failed(_)) {
                let _ = done.try_push(*p);
            }
        }
        self.installs.retain(|(p, _)| !listed(*p));
        if self.installs.len() != was {
            self.catalog_change(CatalogChange::State);
        }
        // This listing is what tells an install that worked, since it
        // comes before the install's own last word (`install_progress`).
        for p in done {
            if let Some(name) = PROGRAMS.get(p).map(|p| p.name) {
                self.install_done(name);
            }
        }
        self.watch_only_waiting();
    }

    fn hit(&self, x: i64, y: i64) -> Hit {
        if self.s.screen.0 == 0 {
            return Hit::Nothing;
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
        // A press outside the windows of a popup grab's owner ends the
        // grab (the owner dismisses its popups) and goes nowhere else.
        let up = self.swallowed & !buttons;
        self.swallowed &= !up;
        self.buttons &= !up;
        let pressed = buttons & !self.buttons;
        if pressed != 0 {
            self.pressed_popup = None;
        }
        if pressed != 0 && self.s.popup_grab.is_some() {
            let target = match self.s.captured.filter(|&c| self.s.window(c).is_some()) {
                Some(c) => Some(c),
                None => match self.hit(x, y) {
                    Hit::Client(id) => Some(id),
                    _ => None,
                },
            };
            if self.s.outside_popup_grab(target) {
                self.s.end_popup_grab(x, y);
                note_buttons(pressed, 0, x, y, format_args!("the end of a popup grab"));
                self.buttons |= pressed;
                self.swallowed |= pressed;
            }
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
            note_buttons(pressed, released, x, y, format_args!("capture {id}"));
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
        note_buttons(pressed, released, x, y, format_args!("{hit:?}"));
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
            self.press_client(id, WM_RBUTTONDOWN);
            let lp = self.s.client_lparam(id, x, y);
            self.s.note_button(id, WM_RBUTTONDOWN, x, y, false);
            self.s.send(id, WM_RBUTTONDOWN, keys, lp);
        }
        // A right click on a caption: WM_NCRBUTTONDOWN/UP (HTCAPTION, the
        // screen point) for the program's DefWindowProc, which shows the
        // window menu there (WM_CONTEXTMENU). Not for a window a kernel
        // thread serves (a Wayland client's has no window menu).
        if (pressed | released) & 2 != 0
            && let Hit::Caption(id) = hit
            && self.s.window(id).is_some_and(|w| !self.s.served(w.owner))
        {
            let pt = ((y as i16 as u16 as u64) << 16) | x as i16 as u16 as u64;
            if pressed & 2 != 0 {
                self.s.raise(id);
                self.s.activate_by_press(id);
                self.s.send(id, WM_NCRBUTTONDOWN, HTCAPTION, pt);
            }
            if released & 2 != 0 {
                self.s.send(id, WM_NCRBUTTONUP, HTCAPTION, pt);
            }
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

    /// What the pointer is over now (a caption button lights up:
    /// `refresh`).
    fn set_hover(&mut self, hit: Hit) {
        self.hover = hit;
    }

    fn left_down(&mut self, hit: Hit, x: i64, y: i64, keys: u64) {
        self.pressed = hit;
        match hit {
            Hit::Caption(id) => {
                self.s.raise(id);
                self.s.activate_by_press(id);
                if let Some(w) = self.s.window(id) {
                    self.drag = Some((id, x - i64::from(w.x), y - i64::from(w.y)));
                }
            }
            Hit::Button(id, _) | Hit::Frame(id) => {
                self.s.raise(id);
                self.s.activate_by_press(id);
            }
            Hit::Client(id) => {
                self.s.raise(id);
                self.press_client(id, WM_LBUTTONDOWN);
                let lp = self.s.client_lparam(id, x, y);
                self.s.note_button(id, WM_LBUTTONDOWN, x, y, false);
                self.s.send(id, WM_LBUTTONDOWN, keys, lp);
            }
            Hit::Nothing => {}
        }
    }

    /// A press (`button`'s down message) in window `id`'s client area: a
    /// framed window becomes active; a popup that is not active leaves
    /// the activation where it is and its program is asked first
    /// (GALA_WM_MOUSEACTIVATE, then `activate_for_press`).
    fn press_client(&mut self, id: u32, button: u32) {
        let Some(w) = self.s.window(id) else { return };
        if !w.popup {
            self.s.activate_by_press(id);
        } else if self.s.active != Some(id) && !self.s.served(w.owner) {
            self.pressed_popup = Some(id);
            self.s.send(id, GALA_WM_MOUSEACTIVATE, u64::from(button), 0);
        }
    }

    /// `owner`'s popup `id` takes the activation for the press it was
    /// just given (user32: its WM_MOUSEACTIVATE answered MA_ACTIVATE, as
    /// Windows activates a clicked WS_POPUP window). Once per press, and
    /// only for the window pressed, so no program takes the keyboard
    /// without the user's click: STATUS_INVALID_DEVICE_STATE otherwise.
    pub fn activate_for_press(&mut self, id: u32, owner: u64) -> Result<(), NtStatus> {
        self.s.owned(id, owner).ok_or(NtStatus::INVALID_HANDLE)?;
        if self.pressed_popup != Some(id) {
            #[cfg(not(test))]
            crate::warn!(
                "desktop: window {id} asked for the activation without a press of its own (refused)"
            );
            return Err(NtStatus::INVALID_DEVICE_STATE);
        }
        self.pressed_popup = None;
        self.s.activate_as(Some(id), owner);
        #[cfg(not(test))]
        crate::info!("desktop: popup {id} takes the activation for its press");
        Ok(())
    }

    /// `owner`'s window `id` is the shell's desktop window
    /// (SetShellWindow): at the bottom, under every other window.
    pub fn set_bottom(&mut self, id: u32, owner: u64) -> Result<(), NtStatus> {
        self.s.set_bottom(id, owner)?;
        #[cfg(not(test))]
        crate::info!("desktop: window {id} is the shell's window, kept at the bottom");
        Ok(())
    }

    /// `owner`'s window `id` into the topmost band or out of it
    /// (GalaWindowSetTopmost).
    pub fn set_topmost(&mut self, id: u32, owner: u64, on: bool) -> Result<(), NtStatus> {
        self.s.set_topmost(id, owner, on)?;
        #[cfg(not(test))]
        crate::info!(
            "desktop: window {id} {} the topmost band",
            if on { "joins" } else { "leaves" }
        );
        Ok(())
    }

    /// `owner`'s window `id` becomes the foreground window
    /// (GalaWindowForeground: SetForegroundWindow), or is refused while
    /// another program's window is active; both logged.
    pub fn foreground(&mut self, id: u32, owner: u64) -> Result<(), NtStatus> {
        let (active, last) = (self.s.active, self.s.last_input);
        let r = self.s.foreground(id, owner);
        #[cfg(not(test))]
        match r {
            Ok(()) => crate::info!("desktop: window {id} takes the foreground"),
            Err(e) => crate::warn!(
                "desktop: window {id} asked for the foreground (refused: {e:?}; window {active:?} \
                 of another program is active, and the last input went to pid {last:?})"
            ),
        }
        #[cfg(test)]
        let _ = (active, last);
        r
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
            (Hit::Client(id), _) => {
                let lp = self.s.client_lparam(id, x, y);
                self.s.note_button(id, WM_LBUTTONUP, x, y, false);
                self.s.send(id, WM_LBUTTONUP, keys, lp);
            }
            _ => {}
        }
    }

    /// What the Linux section lists now (read outside the lock); installs
    /// whose distribution is registered now are over.
    pub fn set_linux(&mut self, listing: &linux::Listing) {
        let installed = &listing.installed;
        // As `set_programs` does, and for the same reason: nothing is
        // allocated inside the `retain` closure.
        let listed = |n: &linux::Name| installed.iter().any(|d| d.name == *n);
        let mut done: Vec<linux::Name> = Vec::new();
        for (n, p) in self.progress.iter() {
            if listed(n) && *p != linux::Progress::Failed {
                let _ = done.try_push(*n);
            }
        }
        self.progress.retain(|(n, _)| !listed(n));
        let tiles = linux::entries(listing);
        if tiles != self.linux_tiles {
            self.linux_tiles = tiles;
            self.linux_change(CatalogChange::State);
        }
        // As a package's listing does, this tells an install that
        // worked (before `linux_progress`'s own last word).
        if !done.is_empty() {
            self.linux_change(CatalogChange::State);
        }
        for n in done {
            self.install_done(n.as_str());
        }
        self.watch_only_waiting();
    }

    /// What an install that is over tells: `title` over `text`'s
    /// pieces, in the log only (the kernel draws no notification area;
    /// shasha's start menu tile shows where the install is, told by
    /// WM_GALA_CATALOG).
    fn announce(&mut self, title: &str, text: &[&str]) {
        #[cfg(not(test))]
        {
            struct Pieces<'a>(&'a [&'a str]);
            impl core::fmt::Display for Pieces<'_> {
                fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                    self.0.iter().try_for_each(|p| f.write_str(p))
                }
            }
            crate::info!("desktop: start menu: {title}: {}", Pieces(text));
        }
        #[cfg(test)]
        {
            fn utf16(s: &str) -> Vec<u16> {
                s.encode_utf16().collect()
            }
            let text: Vec<u16> = text.iter().flat_map(|s| s.encode_utf16()).collect();
            self.told.push((utf16(title), text));
        }
    }

    /// An install that is over is told: the tile it ran on is out of
    /// sight as soon as the menu is closed, and a notification is where
    /// Windows says the same.
    fn install_done(&mut self, name: &str) {
        self.announce(name, &["Installed. Its tile starts it now."]);
    }

    /// The same for one that failed, with what gala's exit code stands
    /// for when there is one (`pkg::install_failure`).
    fn announce_quiet(&mut self, name: &str, why: Quiet) {
        self.announce(name, &[why.told()]);
    }

    fn install_failed(&mut self, name: &str, why: Option<&str>) {
        match why {
            Some(why) => self.announce(name, &["The install failed: ", why, "."]),
            None => self.announce(name, &["The install failed."]),
        }
    }

    /// Where a tile's install is (`gala install <id>` through
    /// GalaDesktopAction): None when it is over (the program list that
    /// follows shows it installed). An install that ends, either way, is
    /// told above the notification area - once: a listing that already
    /// told it leaves nothing to tell, and a failure that repeats itself
    /// is not told again (a click that tries again and fails is).
    pub fn install_progress(&mut self, program: usize, progress: Option<pkg::Progress>) {
        let before = self.installs.iter().find(|(p, _)| *p == program).map(|&(_, s)| s);
        let failed = |s: Option<pkg::Progress>| matches!(s, Some(pkg::Progress::Failed(_)));
        self.installs.retain(|(p, _)| *p != program);
        if let Some(p) = progress {
            let _ = self.installs.try_push((program, p));
        }
        if let Some(name) = PROGRAMS.get(program).map(|p| p.name) {
            match progress {
                None if before.is_some() && !failed(before) => self.install_done(name),
                Some(pkg::Progress::Failed(exit)) if !failed(before) => {
                    self.install_failed(name, pkg::install_failure(exit));
                }
                _ => {}
            }
        }
        self.watch_only_waiting();
        let state = |s: Option<pkg::Progress>| s.as_ref().map(core::mem::discriminant);
        self.catalog_change(if state(before) == state(progress) {
            CatalogChange::Bar
        } else {
            CatalogChange::State
        });
        self.refresh();
    }

    /// A user-mode shell's tile asked for `program`'s install
    /// (GalaDesktopAction's Install, `nt::action::run_launch`): it is
    /// resolving from now on, so that the installer's pid is watched
    /// (`install_started`) and a progress report finds it under way. One
    /// already under way starts again from the beginning: the new
    /// `gala install` is the one whose word counts.
    pub fn begin_install(&mut self, program: usize) {
        self.installs.retain(|(p, _)| *p != program);
        let _ = self.installs.try_push((program, pkg::Progress::Resolving));
        self.catalog_change(CatalogChange::State);
    }

    /// Where `program`'s install is, if one is under way or ended
    /// without its package showing up (the catalog's tiles).
    pub fn install_of(&self, program: usize) -> Option<pkg::Progress> {
        self.installs.iter().find(|(p, _)| *p == program).map(|&(_, s)| s)
    }

    fn catalog_change(&mut self, c: CatalogChange) {
        self.catalog_changed = self.catalog_changed.max(c);
    }

    /// How the catalog's tiles changed since the last call
    /// (`catalog_changed`), and from now on not.
    pub fn take_catalog_changed(&mut self) -> CatalogChange {
        core::mem::replace(&mut self.catalog_changed, CatalogChange::None)
    }

    /// What became of the `gala install` a tile asked for, as
    /// `nt::action::install_for_shell` reports it: the pid to wait for a
    /// word from, or None when it never started. False when the pid is
    /// not watched (MAX_INSTALLERS, or no memory for it): the caller
    /// logs that, since a method may not.
    pub fn install_started(&mut self, program: usize, pid: Option<u64>) -> bool {
        self.installer_started(Installing::Package(program), pid)
    }

    /// The same for a distribution's `gala linux-install`.
    pub fn linux_install_started(&mut self, name: linux::Name, pid: Option<u64>) -> bool {
        self.installer_started(Installing::Linux(name), pid)
    }

    fn installer_started(&mut self, who: Installing, pid: Option<u64>) -> bool {
        self.installers.retain(|(w, _)| *w != who);
        let Some(pid) = pid else {
            self.install_quiet(who, Quiet::NeverStarted);
            return true;
        };
        self.installers.len() < MAX_INSTALLERS && self.installers.try_push((who, pid)).is_ok()
    }

    /// Those processes have ended (`process_ended`, for every process):
    /// an install waiting for a word from one of them will get none.
    pub fn installers_gone(&mut self, gone: &[u64]) {
        self.watch_only_waiting();
        // `install_quiet` takes the entry out, so this ends.
        while let Some(who) =
            self.installers.iter().find(|(_, pid)| gone.contains(pid)).map(|&(w, _)| w)
        {
            self.install_quiet(who, Quiet::Gone);
        }
    }

    /// A tile's install has no word left to come: its bar stays where
    /// it got to, in grey under a question mark, a click tries again,
    /// and it is told above the notification area as any end is. The
    /// tile of an install that already ended is left as it is.
    fn install_quiet(&mut self, who: Installing, why: Quiet) {
        self.installers.retain(|(w, _)| *w != who);
        let at = match who {
            Installing::Package(p) => {
                let at = self.installs.iter().find(|(q, _)| *q == p).map(|&(_, s)| s);
                let Some(at) = at.filter(|s| !s.ended()).map(pkg::Progress::percent) else {
                    return;
                };
                self.installs.retain(|(q, _)| *q != p);
                let _ = self.installs.try_push((p, pkg::Progress::Lost(at)));
                self.catalog_change(CatalogChange::State);
                at
            }
            Installing::Linux(n) => {
                let at = self.progress.iter().find(|(m, _)| *m == n).map(|&(_, p)| p);
                let Some(at) = at.filter(|p| !p.ended()).map(linux::Progress::percent) else {
                    return;
                };
                self.progress.retain(|(m, _)| *m != n);
                let _ = self.progress.try_push((n, linux::Progress::Lost(at)));
                self.linux_change(CatalogChange::State);
                at
            }
        };
        if self.quieted.len() < MAX_INSTALLERS {
            let _ = self.quieted.try_push(Quieted { who, at, why });
        }
        self.announce_quiet(who.name(), why);
        self.refresh();
    }

    /// The next install that went quiet and has not been told in the
    /// log yet (`Quieted`): the free functions drain this, since they
    /// may log and a method may not.
    fn take_quiet(&mut self) -> Option<Quieted> {
        (!self.quieted.is_empty()).then(|| self.quieted.remove(0))
    }

    /// The watch holds the installs that are still waiting for a word
    /// and no others: a pid kept past its install would turn a tile
    /// grey for a stranger's process ending.
    fn watch_only_waiting(&mut self) {
        let (installs, progress) = (&self.installs, &self.progress);
        self.installers.retain(|(w, _)| match w {
            Installing::Package(p) => installs.iter().any(|(q, s)| q == p && !s.ended()),
            Installing::Linux(n) => progress.iter().any(|(m, p)| m == n && !p.ended()),
        });
    }

    /// An install's progress (`gala linux-install` through
    /// GalaDesktopAction): None when it is over (the listing that follows
    /// shows the distribution installed).
    pub fn linux_progress(&mut self, name: linux::Name, progress: Option<linux::Progress>) {
        let before = self.progress.iter().find(|(n, _)| *n == name).map(|&(_, p)| p);
        self.progress.retain(|(n, _)| *n != name);
        if let Some(p) = progress {
            let _ = self.progress.try_push((name, p));
        }
        // Told as a package's install is, without a reason: `wsl
        // --import` says why in the log, not in a code.
        match progress {
            None if before.is_some_and(|p| p != linux::Progress::Failed) => {
                self.install_done(name.as_str());
            }
            Some(linux::Progress::Failed) if before != Some(linux::Progress::Failed) => {
                self.install_failed(name.as_str(), None);
            }
            _ => {}
        }
        self.watch_only_waiting();
        let state = |p: Option<linux::Progress>| p.as_ref().map(core::mem::discriminant);
        self.linux_change(if state(before) == state(progress) {
            CatalogChange::Bar
        } else {
            CatalogChange::State
        });
        self.refresh();
    }

    /// A user-mode shell's tile asked for distribution `name`'s install
    /// (GalaDesktopAction's Linux with LINUX_INSTALL): it is downloading
    /// from nothing from now on, so that the installer's pid is watched
    /// (`linux_install_started`) and its progress finds it under way.
    /// One already under way starts again: the new `gala linux-install`
    /// is the one whose word counts.
    pub fn begin_linux_install(&mut self, name: linux::Name) {
        self.progress.retain(|(n, _)| *n != name);
        let _ = self.progress.try_push((name, linux::Progress::Downloading(0)));
        self.linux_change(CatalogChange::State);
    }

    /// Where distribution `name`'s install is, if one is under way or
    /// ended without the distribution being registered (the Linux
    /// section's tiles).
    pub fn linux_install_of(&self, name: linux::Name) -> Option<linux::Progress> {
        self.progress.iter().find(|(n, _)| *n == name).map(|&(_, p)| p)
    }

    fn linux_change(&mut self, c: CatalogChange) {
        self.linux_changed = self.linux_changed.max(c);
    }

    /// How the Linux section's tiles changed since the last call
    /// (`linux_changed`), and from now on not.
    pub fn take_linux_changed(&mut self) -> CatalogChange {
        core::mem::replace(&mut self.linux_changed, CatalogChange::None)
    }

    /// The Linux section's tiles (host tests, the state dump).
    pub fn linux_tiles(&self) -> &[linux::Distro] {
        &self.linux_tiles
    }

    /// What a system key asked to start (Ctrl+Shift+Esc), once.
    pub fn take_launch(&mut self) -> Option<&'static Program> {
        PROGRAMS.get(self.launch.take()?)
    }

    // ---- Drawing -------------------------------------------------------

    /// Brings the captions up to date with the state, collecting damage.
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
    }

    /// The colour of screen point (x, y), all layers composed: the
    /// desktop's colour, the windows, the pointer.
    pub fn pixel(&self, x: i64, y: i64) -> u32 {
        let mut c = DESKTOP_COLOR;
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

/// The name of process `pid`'s program, as a window's (`set_program`):
/// a Linux process's executable now (after its last execve), else the
/// NT image's file name; its length (at most 64 UTF-16 units). Not
/// called under the desktop's lock (it reads the file system).
pub fn program_name(pid: u64) -> ([u16; 64], usize) {
    let mut out = [0u16; 64];
    let mut n = 0;
    let mut put = |bytes: &[u8]| {
        let name = bytes.rsplit(|&b| b == b'/' || b == b'\\').next().unwrap_or(bytes);
        for (o, &b) in out.iter_mut().zip(name) {
            *o = u16::from(b);
            n += 1;
        }
    };
    if let Some(lx) = crate::lx::state::get(pid) {
        let node = *lx.exe.lock();
        if let Ok(path) = crate::lx::file::path_of(lx.root, node) {
            put(&path);
        }
    } else if let Some(p) = crate::nt::process::by_id(pid) {
        put(p.name.as_bytes());
    }
    (out, n)
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

    /// The window holding the popup grab (the state dump).
    pub fn popup_grab(&self) -> Option<u32> {
        self.d.lock().s.popup_grab
    }
}

/// The desktop the console shows (the input thread's), if any: none
/// while a console switch is under way.
fn console() -> Option<crate::mm::heap::KArc<super::session::Desk>> {
    super::session::desk(super::session::active_console())
}

/// The wallpapers gala ships and their files (THIRD_PARTY.md).
const WALLPAPERS: [(&str, &str); 4] = [
    ("dunes", "\\Windows\\Web\\Wallpaper\\dunes.qoi"),
    ("ribbons", "\\Windows\\Web\\Wallpaper\\ribbons.qoi"),
    ("najdi", "\\Windows\\Web\\Wallpaper\\najdi.qoi"),
    ("night", "\\Windows\\Web\\Wallpaper\\night.qoi"),
];

/// Chooses the boot user's wallpaper at boot, which shasha shows (it
/// reads the "Wallpaper" value, `save_wallpaper`): `choice` (the
/// `wallpaper=` boot option: one of WALLPAPERS, or "drawn" for none: the
/// value is left as it is), else one of the shipped pictures present at
/// random. Without any, none. The kernel draws no wallpaper: it checks
/// that the file is a QOI picture (`qoi_size`) and logs its size.
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
    match qoi_size(path) {
        Ok((w, h)) => {
            crate::info!("desktop: wallpaper {name} ({w} x {h})");
            save_wallpaper(path);
        }
        Err(e) => crate::warn!("desktop: cannot use wallpaper {name} ({path}): {e:?}"),
    }
}

/// The size QOI picture `path`'s header gives (`qoi_header`):
/// STATUS_OBJECT_NAME_NOT_FOUND for no such file,
/// STATUS_INVALID_IMAGE_FORMAT for another kind of file. shasha decodes
/// the picture itself.
fn qoi_size(path: &str) -> Result<(u32, u32), NtStatus> {
    let node = super::file::lookup(path).map_err(|_| NtStatus::OBJECT_NAME_NOT_FOUND)?;
    let mut head = [0u8; QOI_HEADER];
    let n = super::file::read(node, 0, &mut head).map_err(super::file::fs_status)?;
    qoi_header(&head[..n]).ok_or(NtStatus::INVALID_IMAGE_FORMAT)
}

/// A QOI header's length: "qoif", width and height (big-endian u32),
/// channels, colour space.
const QOI_HEADER: usize = 14;

/// The width and height a QOI header gives; None for a short one,
/// another magic, a zero size or channels other than 3 or 4.
fn qoi_header(head: &[u8]) -> Option<(u32, u32)> {
    let head: &[u8; QOI_HEADER] = head.get(..QOI_HEADER)?.try_into().ok()?;
    let be = |at: usize| u32::from_be_bytes([head[at], head[at + 1], head[at + 2], head[at + 3]]);
    let (w, h) = (be(4), be(8));
    (&head[..4] == b"qoif" && w > 0 && h > 0 && matches!(head[12], 3 | 4)).then_some((w, h))
}

/// HKCU\Control Panel\Desktop, where Windows keeps the wallpaper's path
/// ("Wallpaper", REG_SZ; empty for none): SystemParametersInfo's
/// SPI_GETDESKWALLPAPER reads it, the shell (shasha) shows it.
const DESKTOP_KEY: &str = "Control Panel\\Desktop";

/// Saves the wallpaper's file (`path` on the system volume, from its
/// root; empty for none) as the boot user's "Wallpaper" value,
/// `C:` and the path; logged.
fn save_wallpaper(path: &str) {
    use super::registry::{self, ROOT};
    let now = platform::system_time();
    let saved = registry::with(|r| {
        let mut at = ROOT;
        for part in USER_HIVE.split('\\').chain(DESKTOP_KEY.split('\\')) {
            let (w, n) = wide_name::<64>(part);
            at = r.create(at, &w[..n], 0, &[], now)?.0;
        }
        let mut data = Vec::new();
        let drive = if path.is_empty() { "" } else { "C:" };
        for u in drive.encode_utf16().chain(path.encode_utf16()).chain([0]) {
            data.try_extend_from_slice(&u.to_le_bytes()).map_err(|_| NtStatus::NO_MEMORY)?;
        }
        let (w, n) = wide_name::<16>("Wallpaper");
        r.set_value(at, &w[..n], REG_SZ, &data, now)
    });
    match saved {
        Ok(()) => crate::info!("desktop: wallpaper value C:{path}"),
        Err(e) => crate::warn!("desktop: cannot save the wallpaper value C:{path}: {e:?}"),
    }
}

/// A wallpaper's file from SetDesktopWallpaper's text: one of WALLPAPERS
/// by name, or a `.qoi` file's path (`C:\...` or from the root).
fn wallpaper_path(name: &str) -> Option<&str> {
    if let Some((_, path)) = WALLPAPERS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
        return Some(path);
    }
    let path = name.strip_prefix("C:").or_else(|| name.strip_prefix("c:")).unwrap_or(name);
    (path.starts_with('\\')
        && path.len() > 4
        && path[path.len() - 4..].eq_ignore_ascii_case(".qoi"))
    .then_some(path)
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

/// Whether desktops are bare (see `bare`): nothing shows them, and
/// shasha's shell does not run on them.
pub fn is_bare() -> bool {
    bare().is_some()
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

/// Whether a shell can run on `shell`'s desktop (made live if it was
/// not): shasha's taskbar and start menu, the named window actions, the
/// app bars. Not on a bare one (the server profile, no screen and no
/// `presenter=`).
pub fn has_shell(shell: &Shell) -> bool {
    let mut d = shell.d.lock();
    ensure_live(shell, &mut d).is_ok() && d.bare.is_none()
}

/// The end of a boot that keeps running (no `qemu_exit`: a machine, not
/// a test): the console session's desktop goes live if no program has
/// made it so, so the screen shows the desktop (shasha's taskbar and
/// wallpaper once it runs, the desktop's colour before) answering the
/// keyboard and mouse, instead of the boot splash (or, on virtio-gpu,
/// nothing). A desktop goes live otherwise only when a
/// program first uses it; a boot whose programs are console programs
/// left the splash up. Where nothing shows desktops (`bare`) it stays the
/// window state alone, as before. Says what it did.
pub fn show_console_at_boot_end() {
    let id = super::session::active_console();
    let Some(desk) = super::session::desk(id) else {
        crate::warn!("desktop: the boot ends with no console session ({id}): no desktop shown");
        return;
    };
    let mut d = desk.shell.d.lock();
    if d.s.live {
        crate::info!("desktop: the boot ends with session {id}'s desktop already live");
        return;
    }
    if let Err(e) = ensure_live(&desk.shell, &mut d) {
        crate::warn!("desktop: the boot ends: session {id}'s desktop not shown ({e:?})");
    }
}

/// A window's outer rectangle and state (Desktop::placement).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub visible: bool,
    pub minimized: bool,
    pub maximized: bool,
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

/// A process ended: its windows go (shown or not), and a tile waiting
/// for a word from it will get none (`installers_gone`: the install's
/// `gala install` was killed, or faulted before it could report).
pub fn process_ended(shell: &Shell, owner: u64) {
    let mut d = shell.d.lock();
    if !d.s.live {
        return;
    }
    d.destroy_owned(owner);
    d.installers_gone(&[owner]);
    tell_quiet(&mut d);
    redraw_damage(&mut d);
    let wakes = d.take_wakes();
    let (catalog, linux) = (d.take_catalog_changed(), d.take_linux_changed());
    drop(d);
    signal(wakes);
    tell_catalog(shell.session, catalog);
    tell_linux(shell.session, linux);
}

/// Mouse input: the pointer moved by (dx, dy) with `buttons` held (bit 0
/// left, 1 right, 2 middle), to the desktop the console shows. Nothing
/// before that desktop is live, nor during a console switch.
pub fn pointer_event(dx: i64, dy: i64, buttons: u8) {
    if let Some(desk) = console() {
        pointer_on(&desk.shell, |d| d.pointer(dx, dy, buttons));
    }
}

/// Mouse input to `shell`'s desktop: `go` moves the pointer (relative,
/// the mouse's; absolute, injected) with the buttons it says held.
fn pointer_on(shell: &Shell, go: impl FnOnce(&mut Desktop) -> Vec<Rect>) {
    let mut d = shell.d.lock();
    // No pointer moves on a bare desktop.
    if !d.s.live || d.bare.is_some() {
        return;
    }
    let damage = go(&mut d);
    for r in damage {
        redraw(&mut d, r);
    }
    let wakes = d.take_wakes();
    drop(d);
    signal(wakes);
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
/// gets nothing). Returns the program a system key asked to start
/// (Ctrl+Shift+Esc).
pub fn inject(
    shell: &Shell,
    windows: &crate::nt::window::Windows,
    input: Injected,
) -> Result<Option<&'static Program>, NtStatus> {
    {
        let d = shell.d.lock();
        if !d.s.live || d.presenter.kind() != Kind::Test {
            return Err(NtStatus::INVALID_DEVICE_STATE);
        }
    }
    Ok(match input {
        Injected::Pointer { x, y, buttons } => {
            pointer_on(shell, |d| d.pointer_at(x, y, buttons));
            None
        }
        Injected::Key { vk, scan, extended, down } => {
            key_on(shell, windows, vk, scan, extended, down)
        }
        Injected::Wheel(notches) => {
            wheel_on(shell, notches);
            None
        }
    })
}

/// The programs shasha's start menu can list (`list_programs` of
/// PROGRAMS). Reads the file system: not under the desktop's lock.
fn available_programs() -> ([Entry; MAX_TILES], usize) {
    list_programs(PROGRAMS, &mut |path| super::process::program_exists(path))
}

/// The boot user's hive below `\REGISTRY` (the start menu's user:
/// `token::USER`, whose HKCU this is).
const USER_HIVE: &str = "USER\\S-1-5-21-1000000001-1000000002-1000000003-1001";
/// WSL's registrations (ARCHITECTURE "The WSL facade") and the start
/// menu's choices, in that hive.
const LXSS: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Lxss";
const START_MENU_KEY: &str = "Software\\gala\\StartMenu";
const REG_SZ: u32 = 1;
const REG_MULTI_SZ: u32 = 7;

/// `parts` joined by backslashes, as UTF-16 (a registry path).
fn reg_path(parts: &[&str]) -> Option<Vec<u16>> {
    let mut out = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            out.try_push(u16::from(b'\\')).ok()?;
        }
        for u in p.encode_utf16() {
            out.try_push(u).ok()?;
        }
    }
    Some(out)
}

fn wide_name<const N: usize>(name: &str) -> ([u16; N], usize) {
    let mut w = [0u16; N];
    let mut n = 0;
    for u in name.encode_utf16().take(N) {
        w[n] = u;
        n += 1;
    }
    (w, n)
}

/// A REG_SZ's text (UTF-16, up to its NUL) as ASCII in `out`; None for
/// anything else.
fn reg_text<const N: usize>(v: &super::registry::Value) -> Option<linux::Text<N>> {
    if v.kind != REG_SZ {
        return None;
    }
    let mut b = [0u8; N];
    let mut n = 0;
    for c in v.data.chunks_exact(2) {
        let u = u16::from_le_bytes([c[0], c[1]]);
        if u == 0 {
            break;
        }
        *b.get_mut(n)? = u8::try_from(u).ok().filter(u8::is_ascii)?;
        n += 1;
    }
    linux::Text::new(&b[..n])
}

/// Registered distribution `name`'s application `index` (as LinuxList
/// lists them after its terminal), or None: no such distribution or
/// application.
pub fn linux_app(name: &[u8], index: usize) -> Option<linux::App> {
    let listing = linux_listing();
    let distro = listing.installed.iter().find(|d| d.name.as_bytes() == name)?;
    distro.apps().nth(index).copied()
}

/// What the start menu's "Linux distributions" section lists: the
/// boot user's registered distributions (Lxss keys in State 1, each with
/// its applications) and their choices. Reads the registry and the
/// distributions' files: never under the desktop's lock.
fn linux_listing() -> linux::Listing {
    use super::registry::{self, ROOT};
    let mut listing = linux::Listing::default();
    // (name, base path) of each registration.
    let mut found: Vec<(linux::Name, linux::Text<260>)> = Vec::new();
    let _ = registry::with(|r| {
        let path = reg_path(&[USER_HIVE, START_MENU_KEY]).ok_or(NtStatus::NO_MEMORY)?;
        if let Ok(key) = r.lookup(ROOT, &path) {
            let (w, n) = wide_name::<32>("LinuxDismissed");
            if let Ok(v) = r.value(key, &w[..n])
                && v.kind == REG_MULTI_SZ
            {
                listing.prefs.dismissed = linux::Prefs::read_dismissed(&v.data);
            }
        }
        let path = reg_path(&[USER_HIVE, LXSS]).ok_or(NtStatus::NO_MEMORY)?;
        let lxss = r.lookup(ROOT, &path)?;
        let mut i = 0;
        while let Ok(sub) = r.subkey(lxss, i) {
            i += 1;
            let value = |name: &str| {
                let (w, n) = wide_name::<32>(name);
                r.value(sub, &w[..n]).ok()
            };
            let state = value("State").and_then(|v| v.data.first().copied());
            let name = value("DistributionName").and_then(reg_text::<{ linux::NAME }>);
            let base = value("BasePath").and_then(reg_text::<260>);
            if let (Some(1), Some(name), Some(base)) = (state, name, base)
                && found.try_push((name, base)).is_err()
            {
                break;
            }
        }
        Ok(())
    });
    for (name, base) in found {
        let mut d = linux::Distro::new(name, true);
        distro_apps(base.as_bytes(), &mut d);
        if listing.installed.try_push(d).is_err() {
            break;
        }
    }
    listing
}

/// Registered distribution `name`'s tree (its Lxss key's BasePath).
pub fn distro_root_of(name: &[u8]) -> Option<crate::fs::ramfs::NodeId> {
    use super::registry::{self, ROOT};
    let mut base = None;
    let _ = registry::with(|r| {
        let path = reg_path(&[USER_HIVE, LXSS]).ok_or(NtStatus::NO_MEMORY)?;
        let lxss = r.lookup(ROOT, &path)?;
        let mut i = 0;
        while let Ok(sub) = r.subkey(lxss, i) {
            i += 1;
            let value = |name: &str| {
                let (w, n) = wide_name::<32>(name);
                r.value(sub, &w[..n]).ok()
            };
            let this = value("DistributionName").and_then(reg_text::<{ linux::NAME }>);
            if this.is_some_and(|t| t.as_bytes() == name) {
                base = value("BasePath").and_then(reg_text::<260>);
                break;
            }
        }
        Ok(())
    });
    distro_root(base?.as_bytes())
}

/// The boot user's registered distributions (Lxss keys in State 1) by
/// name, each with its tree's node (the task manager's "Linux" groups:
/// a Linux process belongs to the distribution its root is). Reads the
/// registry and the file system: not under the desktop's lock.
pub fn distro_roots() -> Vec<(linux::Name, crate::fs::ramfs::NodeId)> {
    distro_roots_in(USER_HIVE)
}

/// The registered distributions (Lxss keys in State 1) of the user whose
/// hive is `hive` below `\REGISTRY` (`USER\<SID>`), by name, each with
/// its tree's node (`\\wsl.localhost\<distro>`: nt::wslfs).
pub fn distro_roots_in(hive: &str) -> Vec<(linux::Name, crate::fs::ramfs::NodeId)> {
    use super::registry::{self, ROOT};
    let mut found: Vec<(linux::Name, linux::Text<260>)> = Vec::new();
    let _ = registry::with(|r| {
        let path = reg_path(&[hive, LXSS]).ok_or(NtStatus::NO_MEMORY)?;
        let lxss = r.lookup(ROOT, &path)?;
        let mut i = 0;
        while let Ok(sub) = r.subkey(lxss, i) {
            i += 1;
            let value = |name: &str| {
                let (w, n) = wide_name::<32>(name);
                r.value(sub, &w[..n]).ok()
            };
            let state = value("State").and_then(|v| v.data.first().copied());
            let name = value("DistributionName").and_then(reg_text::<{ linux::NAME }>);
            let base = value("BasePath").and_then(reg_text::<260>);
            if let (Some(1), Some(name), Some(base)) = (state, name, base)
                && found.try_push((name, base)).is_err()
            {
                break;
            }
        }
        Ok(())
    });
    let mut out = Vec::new();
    for (name, base) in found {
        if let Some(root) = distro_root(base.as_bytes())
            && out.try_push((name, root)).is_err()
        {
            break;
        }
    }
    out
}

/// A distribution's tree (`<BasePath>\rootfs`, a C: path) as a node.
fn distro_root(base: &[u8]) -> Option<crate::fs::ramfs::NodeId> {
    let rest = match base {
        [d, b':', rest @ ..] if d.eq_ignore_ascii_case(&b'c') => rest,
        _ => return None,
    };
    let mut path = Vec::new();
    for &b in rest.iter().chain(b"\\rootfs") {
        path.try_push(if b == b'\\' { b'/' } else { b }).ok()?;
    }
    crate::lx::file::resolve_in(crate::fs::ramfs::ROOT, &path, true).and_then(|f| f.existing()).ok()
}

/// A distribution's applications: its `/usr/share/applications/*.desktop`
/// files (in name order, as the file system lists them), those that
/// describe one (`linux::parse_desktop`), at most MAX_APPS.
fn distro_apps(base: &[u8], d: &mut linux::Distro) {
    const DIR: &[u8] = b"/usr/share/applications";
    let Some(root) = distro_root(base) else { return };
    let Ok(dir) = crate::lx::file::resolve_in(root, DIR, true).and_then(|f| f.existing()) else {
        return;
    };
    let mut names: Vec<linux::Text<128>> = Vec::new();
    super::file::with_fs(|fs| {
        let Ok(entries) = fs.entries(dir) else { return };
        for (_, name) in entries {
            let mut b = [0u8; 128];
            let ascii = name.len() <= b.len()
                && name.iter().zip(b.iter_mut()).all(|(&u, o)| {
                    *o = u as u8;
                    u < 0x80
                });
            let text = &b[..name.len().min(128)];
            if ascii
                && text.ends_with(b".desktop")
                && let Some(t) = linux::Text::new(text)
                && names.try_push(t).is_err()
            {
                break;
            }
        }
    });
    let mut n = 0;
    for name in names.iter().take(64) {
        let mut path = [0u8; 160];
        let len = DIR.len() + 1 + name.as_bytes().len();
        let Some(slot) = path.get_mut(..len) else { continue };
        slot[..DIR.len()].copy_from_slice(DIR);
        slot[DIR.len()] = b'/';
        slot[DIR.len() + 1..].copy_from_slice(name.as_bytes());
        let Ok(node) =
            crate::lx::file::resolve_in(root, &path[..len], true).and_then(|f| f.existing())
        else {
            continue;
        };
        let mut text = [0u8; 8192];
        let read = super::file::with_fs(|fs| fs.read(node, 0, &mut text).unwrap_or(0));
        if let Some(app) = linux::parse_desktop(&text[..read]) {
            d.apps[n] = Some(app);
            n += 1;
            if n == linux::MAX_APPS {
                break;
            }
        }
    }
}

/// Where a tile's install is for `shell`'s start menu
/// (GalaDesktopAction's InstallProgress from `gala install <id>`):
/// `state` 0 resolving, 1 downloading (`percent`), 2 installing, 3 done
/// (the tile is installed now: its bar goes), 4 failed (`exit`: what
/// gala ended with).
/// STATUS_INVALID_PARAMETER when no tile's package is `id`, so that an
/// install of something the catalog does not list is told so and goes on.
pub fn install_progress(
    shell: &Shell,
    id: &[u8],
    state: i32,
    percent: i32,
    exit: i32,
) -> Result<(), NtStatus> {
    if !has_shell(shell) {
        return Err(NtStatus::NOT_SUPPORTED);
    }
    let program = program_with_package(id).ok_or(NtStatus::INVALID_PARAMETER)?;
    let progress = match state {
        3 => None,
        _ => Some(
            pkg::Progress::from_state(state, percent, exit).ok_or(NtStatus::INVALID_PARAMETER)?,
        ),
    };
    // The list again once it is installed (the disk is read outside the
    // lock), so that the tile is drawn installed and no longer dimmed.
    let listed = if progress.is_none() { Some(available_programs()) } else { None };
    let mut d = shell.d.lock();
    if let Some((tiles, n)) = &listed {
        d.set_programs(&tiles[..*n]);
    }
    d.install_progress(program, progress);
    redraw_damage(&mut d);
    let catalog = d.take_catalog_changed();
    drop(d);
    tell_catalog(shell.session, catalog);
    Ok(())
}

/// What became of the `gala install` a tile asked for: its pid, or
/// None when it never started (`nt::action::install_for_shell`). A tile
/// whose installer never started, or whose installer later ends without
/// a word (`process_ended`), goes quiet instead of waiting for ever.
pub fn install_started(session: u32, program: &'static Program, pid: Option<u64>) {
    let Some(desk) = super::session::desk(session) else { return };
    let Some(k) = PROGRAMS.iter().position(|p| core::ptr::eq(p, program)) else { return };
    let mut d = desk.shell.d.lock();
    let watched = d.install_started(k, pid);
    tell_quiet(&mut d);
    not_watched(watched, pid);
    redraw_damage(&mut d);
    let catalog = d.take_catalog_changed();
    drop(d);
    tell_catalog(session, catalog);
}

/// A user-mode shell's tile asked for catalog entry `program`'s install
/// (GalaDesktopAction's Install): its tile is resolving
/// (`Desktop::begin_install`) before the installer starts, and the
/// shell's window hears of it.
pub fn begin_install(shell: &Shell, program: usize) {
    let mut d = shell.d.lock();
    d.begin_install(program);
    redraw_damage(&mut d);
    let catalog = d.take_catalog_changed();
    drop(d);
    tell_catalog(shell.session, catalog);
}

/// A catalog tile of the user-mode shell's start menu: the entry, whether
/// its program is on this machine, and where its install is (None: none
/// under way or ended). The installed state comes first: a program that
/// is there is installed whatever an install of it last said.
#[derive(Clone, Copy)]
pub struct CatalogTile {
    pub program: &'static Program,
    pub installed: bool,
    pub install: Option<pkg::Progress>,
}

/// The catalog's tiles for `shell`'s start menu (GalaDesktopAction's
/// Catalog): every entry with a package, in PROGRAMS order, once per
/// name (`list_programs`). Reads the file system before the desktop's
/// lock.
pub fn catalog(shell: &Shell) -> Vec<CatalogTile> {
    let (tiles, n) = available_programs();
    let d = shell.d.lock();
    let mut out = Vec::new();
    for e in &tiles[..n] {
        let Some(program) = PROGRAMS.get(e.program).filter(|p| p.package.is_some()) else {
            continue;
        };
        let install = if e.installed { None } else { d.install_of(e.program) };
        if out.try_push(CatalogTile { program, installed: e.installed, install }).is_err() {
            break;
        }
    }
    out
}

/// gala's own message to the shell's window (shasha's Progman): a
/// catalog tile or a Linux section's tile changed, read that list again
/// (GalaDesktopAction's Catalog or LinuxList, as the wparam says:
/// CHANGED_*). In the system range Windows leaves unused (0x0390..0x03DF),
/// so that no program's own message is taken for it.
pub const WM_GALA_CATALOG: u32 = 0x03a0;
/// WM_GALA_CATALOG's wparam: which list to read again, the catalog's
/// tiles (GalaDesktopAction's Catalog) or the Linux section's
/// (LinuxList).
pub const CHANGED_CATALOG: u64 = 0;
pub const CHANGED_LINUX: u64 = 1;

/// How a catalog tile changed (`Desktop::take_catalog_changed`): not
/// at all, its bar alone (a download moving on), or its state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CatalogChange {
    None,
    Bar,
    State,
}

/// Tells session `session`'s shell window that the catalog changed
/// (`WM_GALA_CATALOG`), as `change` says it did: a state that changed
/// leaves a line; a bar that moved does not (a download moves it each
/// percent, and the install's own line says each tenth), unless the
/// post fails.
fn tell_catalog(session: u32, change: CatalogChange) {
    if change == CatalogChange::None {
        return;
    }
    let Some(desk) = super::session::desk(session) else { return };
    let logged = change == CatalogChange::State;
    tell_shell_window(&desk.windows, WM_GALA_CATALOG, CHANGED_CATALOG, "the catalog", logged);
}

/// Posts `message` (`wparam`) to the shell's window (shasha's Progman,
/// the window that called SetShellWindow) as the system, and wakes its
/// program; logged as telling it of `what` (when `logged`; a failure
/// always is). Nothing without one (no user-mode shell runs).
pub fn tell_shell_window(
    windows: &super::window::Windows,
    message: u32,
    wparam: u64,
    what: &str,
    logged: bool,
) {
    let shell = windows.shell_window();
    if shell == 0 {
        return;
    }
    match windows.post_from_system(shell, message, wparam, 0) {
        Ok(event) => {
            if let Some(e) = event {
                super::wait::set_signal(&e, 1);
            }
            if logged {
                crate::info!("desktop: the shell's window {shell:#x} told of {what}");
            }
        }
        Err(e) => {
            crate::warn!("desktop: the shell's window {shell:#x} not told of {what}: {e:?}");
        }
    }
}

/// What an install that went quiet leaves behind: a log line and a
/// `window` event with the program, how far its bar got and why, and a
/// ledger entry (a tile left waiting for a word that never came is a
/// gap worth counting). Written here and not in `install_quiet`,
/// because a `Desktop` method may not log: host unit tests build
/// without the `hosted` feature, where `console::print` writes to COM1
/// with `out`, which faults in user mode.
/// An installer the desktop has no room to watch (more installs at
/// once than MAX_INSTALLERS, or no memory for the pid): its tile keeps
/// its bar until the menu lists the programs again, as every tile did
/// before the watch, and the log says which install that is.
fn not_watched(watched: bool, pid: Option<u64>) {
    if let (false, Some(pid)) = (watched, pid) {
        crate::warn!(
            "desktop: start menu: the install in pid {pid} is not watched (at most \
             {MAX_INSTALLERS} are): its tile waits for the next listing"
        );
        super::ledger::record("start menu", "install not watched", 0);
    }
}

fn tell_quiet(d: &mut Desktop) {
    while let Some(q) = d.take_quiet() {
        let (name, at, why) = (q.who.name(), q.at, q.why);
        crate::warn!("desktop: start menu: {name}'s install went quiet at {at}%: {}", why.as_str());
        crate::events::emit(
            crate::events::Category::Window,
            "start menu",
            format_args!(
                r#""op":"install quiet","name":{},"percent":{at},"why":"{}""#,
                crate::events::Str(name),
                why.as_str()
            ),
        );
        super::ledger::record("start menu", "install gone quiet", 0);
    }
}

/// The same for a distribution's `gala linux-install`.
pub fn linux_install_started(session: u32, name: linux::Name, pid: Option<u64>) {
    let Some(desk) = super::session::desk(session) else { return };
    let mut d = desk.shell.d.lock();
    let watched = d.linux_install_started(name, pid);
    tell_quiet(&mut d);
    not_watched(watched, pid);
    redraw_damage(&mut d);
    let linux = d.take_linux_changed();
    drop(d);
    tell_linux(session, linux);
}

/// A user-mode shell's tile asked for distribution `name`'s install
/// (GalaDesktopAction's Linux, LINUX_INSTALL): its tile is downloading
/// (`Desktop::begin_linux_install`) before the installer starts, and the
/// shell's window hears of it.
pub fn begin_linux_install(shell: &Shell, name: linux::Name) {
    let mut d = shell.d.lock();
    d.begin_linux_install(name);
    redraw_damage(&mut d);
    let linux = d.take_linux_changed();
    drop(d);
    tell_linux(shell.session, linux);
}

/// A tile of the user-mode shell's Linux section: the distribution (its
/// applications an installed one's) and where its install is (None: none
/// under way or ended; an installed distribution has none).
#[derive(Clone, Copy)]
pub struct LinuxTile {
    pub distro: linux::Distro,
    pub install: Option<linux::Progress>,
}

/// The Linux section's tiles for `shell`'s start menu (GalaDesktopAction's
/// LinuxList): `linux::entries` (the suggestions the user's choices
/// dismissed left out), read from the registry and the distributions'
/// files before the desktop's lock, each with its install's progress.
/// Reads only: what the desktop keeps is changed by the installs' own
/// reports (`linux_progress`).
pub fn linux_section(shell: &Shell) -> Vec<LinuxTile> {
    let listing = linux_listing();
    let d = shell.d.lock();
    let mut out = Vec::new();
    for distro in linux::entries(&listing) {
        let install = if distro.installed { None } else { d.linux_install_of(distro.name) };
        if out.try_push(LinuxTile { distro, install }).is_err() {
            break;
        }
    }
    out
}

/// Tells session `session`'s shell window that the Linux section changed
/// (`WM_GALA_CATALOG` with wparam `CHANGED_LINUX`), as `tell_catalog`
/// tells of the catalog: a state leaves a line, a bar that moved does not.
fn tell_linux(session: u32, change: CatalogChange) {
    if change == CatalogChange::None {
        return;
    }
    let Some(desk) = super::session::desk(session) else { return };
    let logged = change == CatalogChange::State;
    tell_shell_window(&desk.windows, WM_GALA_CATALOG, CHANGED_LINUX, "the Linux section", logged);
}

/// The catalog entry whose package is `id`: winget's ids, matched without
/// case as `gala install` matches them.
pub fn program_with_package(id: &[u8]) -> Option<usize> {
    let id = core::str::from_utf8(id).ok()?;
    PROGRAMS.iter().position(|p| p.package.is_some_and(|w| w.eq_ignore_ascii_case(id)))
}

/// An install's progress for `shell`'s start menu (GalaDesktopAction's
/// LinuxProgress from `gala linux-install`): `state` 0 downloading
/// (`percent`), 1 importing, 2 done (the section lists again: the
/// distribution is registered), 3 failed.
pub fn linux_progress(
    shell: &Shell,
    name: &[u8],
    state: i32,
    percent: i32,
) -> Result<(), NtStatus> {
    if !has_shell(shell) {
        return Err(NtStatus::NOT_SUPPORTED);
    }
    let name = linux::Name::new(name).filter(|n| linux::valid_name(n.as_bytes()));
    let name = name.ok_or(NtStatus::INVALID_PARAMETER)?;
    let progress = match state {
        2 => None,
        _ => Some(linux::Progress::from_state(state, percent).ok_or(NtStatus::INVALID_PARAMETER)?),
    };
    let listing = if progress.is_none() { Some(linux_listing()) } else { None };
    let mut d = shell.d.lock();
    if let Some(listing) = &listing {
        d.set_linux(listing);
    }
    d.linux_progress(name, progress);
    redraw_damage(&mut d);
    let linux = d.take_linux_changed();
    drop(d);
    tell_linux(shell.session, linux);
    Ok(())
}

/// The start menu's installed programs, each with the path that starts it
/// (GalaDesktopAction's program list and `gala start <name>`).
pub fn start_menu() -> Vec<(&'static str, &'static str)> {
    let (tiles, n) = available_programs();
    let mut out = Vec::new();
    for e in tiles[..n].iter().filter(|e| e.installed) {
        let Some(program) = PROGRAMS.get(e.program) else { continue };
        let mut buf = [0; 260];
        let Some((_, path)) =
            program.find(&mut |path| super::process::program_exists(path), &mut buf)
        else {
            continue;
        };
        let Ok(path) = super::process::intern(path) else { break };
        if out.try_push((program.name, path)).is_err() {
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
/// The desktop's active window: its owner's pid and its id on the screen
/// (None: no window is active, or no screen). GetForegroundWindow's
/// answer, for every process.
pub fn active_window(shell: &Shell) -> Option<(u64, u32)> {
    if !has_shell(shell) {
        return None;
    }
    let d = shell.d.lock();
    let id = d.s.active?;
    d.s.window(id).map(|w| (w.owner, id))
}

pub fn list_windows(shell: &Shell) -> Result<Vec<Listed>, NtStatus> {
    if !has_shell(shell) {
        return Err(NtStatus::NOT_SUPPORTED);
    }
    Ok(shell.d.lock().list())
}

/// SHAppBarMessage's `op` for window `hwnd` of process `owner`
/// (GalaAppBar): its result, the rectangle it gives back and the edge;
/// STATUS_NOT_SUPPORTED without shasha's shell (no work area to take
/// from). The other bars' ABN_POSCHANGED are posted after.
pub fn appbar_op(
    shell: &Shell,
    owner: u64,
    op: u32,
    hwnd: u64,
    callback: u32,
    edge: u32,
    rect: Rect,
) -> Result<(bool, u32, Rect), NtStatus> {
    if !has_shell(shell) {
        return Err(NtStatus::NOT_SUPPORTED);
    }
    let mut d = shell.d.lock();
    let result = d.appbar_op(owner, op, hwnd, callback, edge, rect);
    redraw_damage(&mut d);
    let posts = d.take_appbar_posts();
    drop(d);
    if !posts.is_empty() {
        crate::info!("desktop: app bars told ABN_POSCHANGED: {} window(s)", posts.len());
    }
    let Some(desk) = super::session::desk(shell.session) else { return result };
    let mut gone: Option<u64> = None;
    for p in posts {
        match desk.windows.post_from_system(p.hwnd, p.message, p.wparam, p.lparam) {
            Ok(Some(e)) => {
                super::wait::set_signal(&e, 1);
            }
            Ok(None) => {}
            Err(NtStatus::INVALID_HANDLE) => gone = Some(p.hwnd),
            Err(e) => crate::warn!(
                "desktop: app bar: message {:#x} to window {:#x} not posted: {e:?}",
                p.message,
                p.hwnd
            ),
        }
    }
    // A bar whose window is gone gives its strip back.
    if let Some(hwnd) = gone {
        crate::info!("desktop: app bar of gone window {hwnd:#x} removed");
        let mut d = shell.d.lock();
        let _ = d.appbar_op(0, appbar::ABM_REMOVE, hwnd, 0, 0, Rect { x: 0, y: 0, w: 0, h: 0 });
        d.appbars.take_posts();
        redraw_damage(&mut d);
    }
    result
}

/// Where the pointer's hot spot is on the screen (GetCursorPos: the
/// system's pointer, whichever process last had a mouse message), None
/// before the screen's size is known.
pub fn pointer_pos(shell: &Shell) -> Option<(i64, i64)> {
    let mut d = shell.d.lock();
    ensure_live(shell, &mut d).ok()?;
    Some(d.pointer)
}

/// The work area (the screen without the taskbar and the app bars'
/// strips), None before the screen's size is known.
pub fn work_area(shell: &Shell) -> Option<Rect> {
    let mut d = shell.d.lock();
    ensure_live(shell, &mut d).ok()?;
    let w = d.s.work();
    (w.w > 0).then_some(w)
}

/// Makes `name` the wallpaper (one of WALLPAPERS, a `.qoi` file's path,
/// or "drawn" for none) and saves it as the "Wallpaper" value
/// (GalaDesktopAction; SPI_SETDESKWALLPAPER), which shasha reads and
/// draws when told (`nt::action`'s WM_SETTINGCHANGE): STATUS_OBJECT_NAME_NOT_FOUND
/// for another name or a missing file, STATUS_INVALID_IMAGE_FORMAT for a
/// file that is no QOI picture (`qoi_size`), STATUS_NOT_SUPPORTED where
/// no shell runs (a bare desktop).
pub fn set_wallpaper(shell: &Shell, name: &str) -> Result<(), NtStatus> {
    if !has_shell(shell) {
        return Err(NtStatus::NOT_SUPPORTED);
    }
    let path = if name.eq_ignore_ascii_case("drawn") {
        ""
    } else {
        wallpaper_path(name).ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?
    };
    if !path.is_empty() {
        qoi_size(path)?;
    }
    crate::info!("desktop: wallpaper {name}");
    save_wallpaper(path);
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

/// A key to the desktop the console shows. Returns the session and the
/// program a system key asked to start (Ctrl+Shift+Esc), if any.
pub fn key_event(vk: u8, scan: u8, extended: bool, down: bool) -> Option<(u32, &'static Program)> {
    let desk = console()?;
    key_on(&desk.shell, &desk.windows, vk, scan, extended, down).map(|p| (desk.shell.session, p))
}

fn key_on(
    shell: &Shell,
    windows: &crate::nt::window::Windows,
    vk: u8,
    scan: u8,
    extended: bool,
    down: bool,
) -> Option<&'static Program> {
    let mut d = shell.d.lock();
    if !d.s.live || d.bare.is_some() {
        return None;
    }
    // A hot key (RegisterHotKey) takes the key's down before the desktop
    // and the active window see it (the registry's lock is a leaf: taken
    // under the desktop's).
    let mut hot = None;
    if let Some((mods, key, repeat)) = d.hot_candidate(vk, down)
        && let Some((taken, posted)) = windows.hot_key(mods, key, repeat)
    {
        d.hot_key_taken(vk, down);
        // Its program received this input (SetForegroundWindow's rule).
        d.s.last_input = Some(u64::from(taken.pid));
        hot = Some((taken, posted, repeat));
    }
    if !(hot.is_some() && down) {
        d.key(vk, scan, extended, down);
    }
    redraw_damage(&mut d);
    let launch = d.take_launch();
    if launch.is_some_and(|p| p.name == TASK_MANAGER) {
        crate::info!("desktop: Ctrl+Shift+Esc: {TASK_MANAGER}");
    }
    let wakes = d.take_wakes();
    drop(d);
    match hot {
        Some((key, Ok(event), repeat)) => {
            if !repeat && cfg!(not(test)) {
                let combo = crate::nt::window::Combo(key.modifiers, key.vk);
                crate::info!("desktop: hot key {} ({combo}) to pid {}", key.id, key.pid);
            }
            if let Some(e) = event {
                crate::nt::wait::set_signal(&e, 1);
            }
        }
        Some((key, Err(e), _)) if cfg!(not(test)) => {
            let combo = crate::nt::window::Combo(key.modifiers, key.vk);
            crate::warn!("desktop: hot key {} ({combo}) to pid {} dropped: {e:?}", key.id, key.pid);
        }
        _ => {}
    }
    signal(wakes);
    launch
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

/// The display watch has been started (`start_display_watch`), and how
/// many times it has ticked (the `desktop: display watch:` line at the
/// end of a run).
static WATCH_STARTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static WATCH_TICKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// How long the display watch sleeps between ticks.
const WATCH_MS: u64 = 1000;

/// Starts the display watch (once; later calls do nothing): [`tick`] at
/// once, then every second, on a thread of its own.
///
/// Its own, because a display change is not input: on the input
/// thread's poll loop, a machine with no PS/2 mouse (the input thread
/// stops at one) would never notice the host's window being resized.
pub fn start_display_watch() {
    if WATCH_STARTED.swap(true, core::sync::atomic::Ordering::AcqRel) {
        return;
    }
    if platform::spawn("desktop display", watch_thread, 0).is_err() {
        crate::warn!("desktop: no display watch thread: a display change will not be noticed");
    }
}

fn watch_thread(_: usize) {
    loop {
        tick();
        WATCH_TICKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        platform::sleep_ms(WATCH_MS);
    }
}

/// The `desktop: display watch:` line at the end of a run; nothing when
/// no desktop went live (the watch starts with the first one).
pub fn log_display_stats() {
    let ticks = WATCH_TICKS.load(core::sync::atomic::Ordering::Relaxed);
    if ticks == 0 {
        return;
    }
    crate::info!(
        "desktop: display watch: {ticks} ticks, one every {WATCH_MS} ms on its own thread"
    );
}

/// Whether `no_shell` has logged.
static NO_SHELL_TOLD: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// No shell runs (or will) on the console's desktop, for reason `why`:
/// logged once, because what the screen then shows is the desktop's
/// colour (DESKTOP_COLOR) and the windows alone, which looks broken to
/// whoever does not know the kernel draws no shell of its own.
pub fn no_shell(why: &str) {
    if NO_SHELL_TOLD.swap(true, core::sync::atomic::Ordering::AcqRel) {
        return;
    }
    crate::info!(
        "desktop: no shell yet: {why}: the screen shows the desktop's colour and the windows alone (the kernel draws no taskbar or start menu)"
    );
}

/// The display watch's tick (`start_display_watch`): notices a display
/// change (virtio-gpu: the host's window resized), which the live
/// desktop the screen shows follows.
pub fn tick() {
    let Some(changed) = platform::screen_changed() else { return };
    let Ok(ids) = super::session::ids() else { return };
    for id in ids {
        let Some(desk) = super::session::desk(id) else { continue };
        let mut d = desk.shell.d.lock();
        if !d.s.live || d.bare.is_some() {
            continue;
        }
        if d.presenter.kind() == Kind::Console {
            resize(&mut d, changed.0, changed.1);
        }
    }
}

/// The screen is now w x h: the desktop's layout (the work area, the
/// taskbar's strip) is made again and everything redrawn. Windows keep their places (a later step moves maximized and
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

/// Whether `owner`'s windows are "Not responding" (input waiting
/// untaken for `winstate::HUNG_NS`: the task manager's Status).
pub fn not_responding(shell: &Shell, owner: u64) -> bool {
    let now = platform::monotonic_ns();
    shell.d.lock().s.not_responding(owner, now)
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
