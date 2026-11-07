//! The desktop without a compositor: a server kernel, built without the
//! `shasha` feature (the graphical shell; ROADMAP SD, ARCHITECTURE
//! "Server and desktop workloads"). Same interface as `nt::desktop`.
//!
//! Each session's desktop keeps its window state (`nt::winstate`: windows,
//! Z order, focus, capture, each window's last client pixels, input for
//! programs) as on a machine whose desktop nobody shows: GUI programs
//! make, show, paint and close their windows, GalaSession's query reports
//! them, and a remote presenter (ROADMAP RS B3b) will show them. Nothing
//! composes them here: the only presenter is none (a frame, injected input
//! and the Test and Console presenters are refused), and the shell's
//! window actions, its window and program lists and the wallpaper are
//! STATUS_NOT_SUPPORTED (no shell). The compositor, wallpapers, fonts and
//! the start menu are not in the binary.

use super::object::ObjectRef;
use super::status::NtStatus;
use super::winstate::{Shadow, State};
use crate::sync::IrqMutex;
use alloc::vec::Vec;

pub use super::winstate::{
    Action, DEFAULT_SIZE, InputEvent, LISTED_ACTIVE, LISTED_TITLE, Listed, Rect, SnapTo, TASKBAR,
};

/// The largest Test frame side (nt::desktop's).
pub const MAX_FRAME_SIDE: u32 = 4096;

/// A start menu entry (there is no start menu).
pub struct Program {
    pub name: &'static str,
    pub path: &'static str,
}

/// A desktop with nothing to compose it: the window state alone, as the
/// system calls use it (`with`).
pub struct Desktop(State<()>);

impl core::ops::Deref for Desktop {
    type Target = State<()>;
    fn deref(&self) -> &State<()> {
        &self.0
    }
}

impl core::ops::DerefMut for Desktop {
    fn deref_mut(&mut self) -> &mut State<()> {
        &mut self.0
    }
}

impl Desktop {
    /// GalaWindowDrag: no pointer moves on a desktop nobody shows, so no
    /// button is down (STATUS_INVALID_DEVICE_STATE, as with the button up
    /// on a screen); STATUS_INVALID_HANDLE for another's window.
    pub fn begin_drag(&mut self, id: u32, owner: u64) -> Result<(), NtStatus> {
        self.0.owned(id, owner).ok_or(NtStatus::INVALID_HANDLE)?;
        Err(NtStatus::INVALID_DEVICE_STATE)
    }

    /// GalaWindowResize: as `begin_drag`.
    pub fn begin_resize(&mut self, id: u32, owner: u64, _edges: u32) -> Result<(), NtStatus> {
        self.begin_drag(id, owner)
    }
}

/// One session's desktop (nt::session::Desk holds it): its window state
/// behind its own lock.
pub struct Shell {
    session: u32,
    d: IrqMutex<Desktop>,
}

/// No compositor: a window's damage is its own rectangle (and is dropped:
/// nothing composes it).
const NO_SHADOW: Shadow = Shadow { reach: 0, drop: 0 };

/// Why nothing shows a desktop here (the log, the presenter event, the
/// state dump; the desktop kernel's bare desktops give theirs).
const BARE: &str = "no compositor in this kernel";

impl Shell {
    pub const fn new(session: u32) -> Shell {
        Shell { session, d: IrqMutex::new(Desktop(State::new(NO_SHADOW))) }
    }

    /// (windows, live, what shows it, why nothing can): the state
    /// dump's figures.
    pub fn counts(&self) -> (usize, bool, Kind, Option<&'static str>) {
        let d = self.d.lock();
        (d.windows.len(), d.live, Kind::None, Some(BARE))
    }

    /// No desktop delivers input here.
    pub fn recent_input(
        &self,
    ) -> ([Option<crate::nt::winstate::InputNote>; crate::nt::winstate::RECENT_INPUT], u64, usize)
    {
        ([None; crate::nt::winstate::RECENT_INPUT], 0, 0)
    }
}

/// The kinds of presenter (nt::desktop's; only none here).
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

pub fn choose_wallpaper(_: Option<&str>) {}

pub fn set_server_profile() {}

/// `presenter=`: accepted (and without effect: nothing composes here).
pub fn set_local_presenter(name: &str) -> bool {
    matches!(name, "test" | "none")
}

/// Always the server profile: no console windows, no shell.
pub fn server_profile() -> bool {
    true
}

/// Makes `shell`'s desktop live if it is not: DEFAULT_SIZE, shown by
/// none (logged once per session, with the presenter event).
fn ensure_live(shell: &Shell, d: &mut Desktop) {
    if d.live {
        return;
    }
    let (w, h) = DEFAULT_SIZE;
    d.screen = (i64::from(w), i64::from(h));
    d.live = true;
    crate::info!(
        "desktop: session {} keeps its windows ({w}x{h}), shown by none ({BARE})",
        shell.session
    );
    crate::events::emit(
        crate::events::Category::Window,
        "presenter",
        format_args!(
            r#""op":"live","session":{},"presenter":"none","w":{w},"h":{h},"bare":"{BARE}""#,
            shell.session
        ),
    );
}

/// The size of `shell`'s desktop (made live if it was not).
pub fn screen(shell: &Shell) -> Option<(u32, u32)> {
    let mut d = shell.d.lock();
    ensure_live(shell, &mut d);
    Some((d.screen.0 as u32, d.screen.1 as u32))
}

/// Whether shasha's shell (taskbar, start menu, named window actions)
/// runs on `shell`'s desktop: never in this kernel.
pub fn has_shell(_: &Shell) -> bool {
    false
}

/// Runs `f` on the desktop's window state (made live if it was not); the
/// damage it leaves is dropped (nothing composes it).
pub fn with<R>(
    shell: &Shell,
    f: impl FnOnce(&mut Desktop) -> Result<(R, Option<Rect>), NtStatus>,
) -> Result<R, NtStatus> {
    let mut d = shell.d.lock();
    ensure_live(shell, &mut d);
    let result = f(&mut d);
    d.damage.clear();
    Ok(result?.0)
}

/// GalaSession's presenters: none is what shows it already (the desktop
/// goes live); Test and Console need a compositor (STATUS_NOT_SUPPORTED).
pub fn attach(shell: &Shell, kind: Kind, _: Option<(u32, u32)>) -> Result<Kind, NtStatus> {
    if kind != Kind::None {
        crate::warn!(
            "desktop: session {} cannot be shown by {}: {BARE}",
            shell.session,
            kind.name()
        );
        return Err(NtStatus::NOT_SUPPORTED);
    }
    let mut d = shell.d.lock();
    ensure_live(shell, &mut d);
    Ok(Kind::None)
}

/// What shows the desktop (none), whether it is live, its size and its
/// windows; nothing is made live.
pub fn query(shell: &Shell) -> (Kind, bool, (u32, u32), usize) {
    let d = shell.d.lock();
    (Kind::None, d.live, (d.screen.0 as u32, d.screen.1 as u32), d.windows.len())
}

/// No Test frame shows a desktop here.
pub fn read_frame(_: &Shell, _: Rect, _: &mut [u32]) -> Result<((u32, u32), u64), NtStatus> {
    Err(NtStatus::INVALID_DEVICE_STATE)
}

/// A process ended: its windows go.
pub fn process_ended(shell: &Shell, owner: u64) {
    let mut d = shell.d.lock();
    if !d.live {
        return;
    }
    d.destroy_owned(owner);
    d.damage.clear();
}

/// No local keyboard or mouse reaches a desktop here.
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

/// Only a desktop a Test frame shows takes injected input: none here.
pub fn inject(_: &Shell, _: Injected) -> Result<Option<&'static Program>, NtStatus> {
    Err(NtStatus::INVALID_DEVICE_STATE)
}

pub fn wheel_event(_: i64) {}

pub fn key_event(_: u8, _: u8, _: bool, _: bool) {}

/// GalaWindowListen: `owner`'s input event.
pub fn listen(shell: &Shell, owner: u64, event: ObjectRef) -> Result<(), NtStatus> {
    let mut d = shell.d.lock();
    d.listen(owner, event)?;
    let wakes = d.take_wakes();
    drop(d);
    for e in wakes {
        super::wait::set_signal(&e, 1);
    }
    Ok(())
}

pub fn tick() {}

/// Takes input waiting for `owner`'s windows into `out`; how many.
pub fn take_input(shell: &Shell, owner: u64, out: &mut [InputEvent]) -> usize {
    shell.d.lock().take_input(owner, out)
}

pub fn start_menu() -> Vec<&'static Program> {
    Vec::new()
}

/// No shell: no named window actions.
pub fn window_action(_: &Shell, _: u64, _: u32, _: Action) -> Result<(), NtStatus> {
    Err(NtStatus::NOT_SUPPORTED)
}

/// No taskbar to list.
pub fn list_windows(_: &Shell) -> Result<Vec<Listed>, NtStatus> {
    Err(NtStatus::NOT_SUPPORTED)
}

pub fn set_wallpaper(_: &Shell, _: &str) -> Result<(), NtStatus> {
    Err(NtStatus::NOT_SUPPORTED)
}
