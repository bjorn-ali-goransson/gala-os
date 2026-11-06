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

    /// Never shown: there is no compositor.
    pub fn shown(&self) -> bool {
        false
    }

    /// (windows on the screen, drawn yet): none.
    pub fn counts(&self) -> (usize, bool) {
        (0, false)
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

pub fn pointer_event(_: i64, _: i64, _: u8) -> Option<&'static Program> {
    None
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
