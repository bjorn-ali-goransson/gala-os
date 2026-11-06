//! The desktop left out: a server kernel, built without the `shasha`
//! feature (the graphical shell; ROADMAP SD, ARCHITECTURE "Server and
//! desktop workloads"). Same interface as `nt::desktop`, nothing behind
//! it: every window call and GalaDesktopAction's window actions are
//! STATUS_NOT_SUPPORTED, as on a machine without a screen, so user32
//! keeps its windows in the registry and draws nothing. The compositor,
//! wallpapers, fonts and the start menu are not in the binary.

use super::object::ObjectRef;
use super::status::NtStatus;
use alloc::vec::Vec;

pub const TASKBAR: u32 = 48;
/// The largest Test frame side (nt::desktop's).
pub const MAX_FRAME_SIDE: u32 = 4096;

/// A start menu entry (there is no start menu).
pub struct Program {
    pub name: &'static str,
    pub path: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapTo {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Activate,
    Minimize,
    Maximize,
    Restore,
    Close,
    Snap(SnapTo),
    Place { x: i32, y: i32, w: u32, h: u32 },
}

pub const LISTED_TITLE: usize = 128;
pub const LISTED_ACTIVE: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listed {
    pub id: u32,
    pub owner: u64,
    pub flags: u32,
    pub rect: [i32; 4],
    pub title: [u16; LISTED_TITLE],
    pub title_len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputEvent {
    pub owner: u64,
    pub window: u32,
    pub message: u32,
    pub wparam: u64,
    pub lparam: u64,
}

/// No desktop can exist: `with` never calls its closure.
pub enum Desktop {}

/// A session's desktop as shasha would keep it: nothing here (the window
/// registry, nt::window, still serves the session's windows).
pub struct Shell;

impl Shell {
    pub const fn new(_session: u32) -> Shell {
        Shell
    }

    /// (windows, live, what shows it): none, never, nothing.
    pub fn counts(&self) -> (usize, bool, Kind) {
        (0, false, Kind::None)
    }
}

/// The kinds of presenter (nt::desktop's; none can attach here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    None = 0,
    Console = 1,
    Test = 2,
}

impl Kind {
    pub fn from_number(n: u64) -> Option<Kind> {
        match n {
            0 => Some(Kind::None),
            1 => Some(Kind::Console),
            2 => Some(Kind::Test),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::None => "none",
            Kind::Console => "console",
            Kind::Test => "test",
        }
    }
}

#[allow(clippy::too_many_arguments)]
impl Desktop {
    pub fn create(
        &mut self,
        _: u64,
        _: i32,
        _: i32,
        _: u32,
        _: u32,
        _: &[u16],
        _: u32,
    ) -> Result<u32, NtStatus> {
        match *self {}
    }
    pub fn show(&mut self, _: u32, _: u64, _: bool) -> Result<Rect, NtStatus> {
        match *self {}
    }
    pub fn show_minimized(&mut self, _: u32, _: u64) -> Result<Rect, NtStatus> {
        match *self {}
    }
    pub fn begin_drag(&mut self, _: u32, _: u64) -> Result<(), NtStatus> {
        match *self {}
    }
    pub fn begin_resize(&mut self, _: u32, _: u64, _: u32) -> Result<(), NtStatus> {
        match *self {}
    }
    pub fn present(&mut self, _: u32, _: Rect, _: &[u32], _: usize) -> Result<Rect, NtStatus> {
        match *self {}
    }
    pub fn place(
        &mut self,
        _: u32,
        _: u64,
        _: i32,
        _: i32,
        _: u32,
        _: u32,
    ) -> Result<Rect, NtStatus> {
        match *self {}
    }
    pub fn set_title(&mut self, _: u32, _: u64, _: &[u16]) -> Result<Rect, NtStatus> {
        match *self {}
    }
    pub fn set_icon(&mut self, _: u32, _: u64, _: &[u32], _: usize) -> Result<(), NtStatus> {
        match *self {}
    }
    pub fn capture(&mut self, _: u32, _: u64) -> Result<(), NtStatus> {
        match *self {}
    }
    pub fn destroy(&mut self, _: u32, _: u64) -> Result<Rect, NtStatus> {
        match *self {}
    }
}

pub fn choose_wallpaper(_: Option<&str>) {}

pub fn set_server_profile() {}

/// `presenter=`: accepted (and without effect: there are no desktops).
pub fn set_local_presenter(name: &str) -> bool {
    matches!(name, "test" | "none")
}

/// GalaSession's presenters: no desktop to show.
pub fn attach(_: &Shell, _: Kind, _: Option<(u32, u32)>) -> Result<Kind, NtStatus> {
    Err(NtStatus::NOT_SUPPORTED)
}

pub fn query(_: &Shell) -> (Kind, bool, (u32, u32), usize) {
    (Kind::None, false, (0, 0), 0)
}

pub fn read_frame(_: &Shell, _: Rect, _: &mut [u32]) -> Result<((u32, u32), u64), NtStatus> {
    Err(NtStatus::NOT_SUPPORTED)
}

/// Always the server profile: there is no desktop to bring up.
pub fn server_profile() -> bool {
    true
}

pub fn screen(_: &Shell) -> Option<(u32, u32)> {
    None
}

pub fn with<R>(
    _: &Shell,
    _: impl FnOnce(&mut Desktop) -> Result<(R, Option<Rect>), NtStatus>,
) -> Result<R, NtStatus> {
    Err(NtStatus::NOT_SUPPORTED)
}

pub fn process_ended(_: &Shell, _: u64) {}

pub fn pointer_event(_: i64, _: i64, _: u8) -> Option<(u32, &'static Program)> {
    None
}

/// Injected input (GalaSession's SESSION_INPUT), as the desktop kernel's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Injected {
    Pointer { x: i64, y: i64, buttons: u8 },
    Key { vk: u8, scan: u8, extended: bool, down: bool },
    Wheel(i64),
}

/// No desktop to bring input to.
pub fn inject(_: &Shell, _: Injected) -> Result<Option<&'static Program>, NtStatus> {
    Err(NtStatus::NOT_SUPPORTED)
}

pub fn wheel_event(_: i64) {}

pub fn key_event(_: u8, _: u8, _: bool, _: bool) {}

pub fn listen(_: &Shell, _: u64, _: ObjectRef) -> Result<(), NtStatus> {
    Err(NtStatus::NOT_SUPPORTED)
}

pub fn tick() {}

pub fn take_input(_: &Shell, _: u64, _: &mut [InputEvent]) -> usize {
    0
}

pub fn start_menu() -> Vec<&'static Program> {
    Vec::new()
}

pub fn window_action(_: &Shell, _: u64, _: u32, _: Action) -> Result<(), NtStatus> {
    Err(NtStatus::NOT_SUPPORTED)
}

pub fn list_windows(_: &Shell) -> Result<Vec<Listed>, NtStatus> {
    Err(NtStatus::NOT_SUPPORTED)
}

pub fn set_wallpaper(_: &Shell, _: &str) -> Result<(), NtStatus> {
    Err(NtStatus::NOT_SUPPORTED)
}
