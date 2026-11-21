//! GalaDesktopAction: the shell's named actions behind one system call,
//! for the `gala` command (user/gala) as for shasha's own mouse and keys
//! (ROADMAP SD "CLI parity", ARCHITECTURE "Server and desktop
//! workloads"). Starting a program works on every kernel; the window
//! actions are shasha's shell's (`nt::desktop::Action`, `has_shell`) and
//! fail with STATUS_NOT_SUPPORTED where a desktop is bare (no shell runs
//! on it: `profile=server`, no screen and no `presenter=`).
//!
//! The request (little-endian, version 1):
//!
//! | bytes  | field |
//! |--------|-------|
//! | 0..4   | version (1) |
//! | 4..8   | action (ACTION_*) |
//! | 8..16  | the window's HWND (the registry's) |
//! | 16..32 | four i32 arguments: Snap's side (0 left, 1 right, 2 up, 3 down); Move's x, y, width, height; LinuxProgress's state and percent; InstallProgress's state, percent and gala's exit code |
//! | 32..36 | the text's length in UTF-16 units (at most MAX_TEXT) |
//! | 36..40 | zero |
//! | 40..   | the text (UTF-16): Start's program and arguments, Wallpaper's name, LinuxProgress's distribution, InstallProgress's package id |
//!
//! The output: Start writes the new process id (u64). Windows, Programs,
//! Catalog and LinuxList write a header (u32 version 1, u32 how many
//! there are), then as many entries as fit (WINDOW_ENTRY / PROGRAM_ENTRY
//! / CATALOG_ENTRY / LINUX_ENTRY bytes each). Actions 12 and 13 were the kernel taskbar's
//! notification area (its icons and a click on one): they are gone with
//! it, as shasha's icons are UI Automation elements
//! (`TrayIcon<hwnd>_<id>`), and stay unused.
//!
//! A catalog tile that changes (an install asked for, moving on, ended
//! or gone quiet) posts gala's `WM_GALA_CATALOG` (0x03A0, in Windows'
//! unused system range) to the shell's window, which reads Catalog
//! again (`desktop::tell_catalog`); a Linux section's tile that does
//! (the same, or the tiles listed changing) posts it with wparam 1,
//! for LinuxList (`desktop::tell_linux`).

use super::desktop::{self, Action, SnapTo};
use super::status::NtStatus;
use crate::mm::heap::TryVec;
use alloc::vec::Vec;

pub const REQUEST_VERSION: u32 = 1;
pub const REQUEST_HEADER: usize = 40;
/// The longest text a request carries (UTF-16 units).
pub const MAX_TEXT: usize = 520;

pub const ACTION_START: u32 = 1;
pub const ACTION_WINDOWS: u32 = 2;
pub const ACTION_ACTIVATE: u32 = 3;
pub const ACTION_MINIMIZE: u32 = 4;
pub const ACTION_MAXIMIZE: u32 = 5;
pub const ACTION_RESTORE: u32 = 6;
pub const ACTION_CLOSE: u32 = 7;
pub const ACTION_SNAP: u32 = 8;
pub const ACTION_MOVE: u32 = 9;
pub const ACTION_WALLPAPER: u32 = 10;
/// WM_SETTINGCHANGE and its wParam for the wallpaper.
const WM_SETTINGCHANGE: u32 = 0x001a;
const SPI_SETDESKWALLPAPER: u64 = 0x14;
pub const ACTION_PROGRAMS: u32 = 11;
/// An install's progress for the start menu's Linux section (`gala
/// linux-install`): the text the distribution's name, args[0] the state
/// (0 downloading, 1 importing, 2 done, 3 failed), args[1] the percent
/// downloaded.
pub const ACTION_LINUX_PROGRESS: u32 = 14;
/// Where the install of a catalog package is, for its start menu tile
/// (`gala install <id>`, which a tile runs without a window): the text
/// the winget id, args[0] the state (0 resolving, 1 downloading, 2
/// installing, 3 done, 4 failed), args[1] the percent downloaded,
/// args[2] the exit code gala fails with (galapkg's `Kind`).
pub const ACTION_INSTALL_PROGRESS: u32 = 15;
/// A user-mode shell's start menu (ROADMAP SH step 1, part 4): install a
/// catalog package as the caller, without a window (`gala install
/// <id>`, which reports through ACTION_INSTALL_PROGRESS): the text the
/// winget id (letters, digits, `.`, `-`, `_`, `+`), args[0] the launch
/// flags.
pub const ACTION_INSTALL: u32 = 16;
/// A user-mode shell's Linux section, as the caller: the text the
/// distribution's name, args[0] the launch flags, args[1] what to start
/// (LINUX_*), args[2] the install's suggestion (`SUGGESTIONS` index) or
/// the application (its index among the distribution's applications).
pub const ACTION_LINUX: u32 = 17;
/// A user-mode shell's start menu (ROADMAP SH step 1, part 11): the
/// catalog's tiles, CATALOG_ENTRY bytes each, in the start menu's order
/// (`desktop::catalog`): STATUS_NOT_SUPPORTED where no shell runs, as
/// Programs.
pub const ACTION_CATALOG: u32 = 18;
/// A user-mode shell's start menu (ROADMAP SH step 1, part 11): the
/// Linux section's entries, LINUX_ENTRY bytes each (`desktop::
/// linux_section`, `linux_entries`): each distribution's tile in the
/// section's order, an installed one's followed by its terminal and its
/// applications. STATUS_NOT_SUPPORTED where no shell runs, as Programs.
pub const ACTION_LINUX_LIST: u32 = 19;

/// The launch verbs' flags (Start, Install, Linux: args[0]): start a
/// console program in a console window of its own (conhost.exe, as
/// Explorer's CreateProcess without a console to inherit gives it; the
/// Linux verb's terminal always has one); give the caller a handle to
/// the process (PROCESS_ALL_ACCESS) after its id, as ShellExecuteEx's
/// SEE_MASK_NOCLOSEPROCESS does.
pub const LAUNCH_CONSOLE_WINDOW: i32 = 1;
pub const LAUNCH_HANDLE: i32 = 2;
/// ACTION_LINUX's operations: install a suggested distribution (`gala
/// linux-install`), start its desktop session, a terminal (`wsl -d`), or
/// one of its applications (`wsl -d <name> --exec ...`).
pub const LINUX_INSTALL: i32 = 0;
pub const LINUX_DESKTOP: i32 = 1;
pub const LINUX_TERMINAL: i32 = 2;
pub const LINUX_APP: i32 = 3;

/// Whether `action` starts a program (as the caller: GalaDesktopAction
/// gives it a copy of the caller's primary token).
pub fn is_launch(action: u32) -> bool {
    matches!(action, ACTION_START | ACTION_INSTALL | ACTION_LINUX)
}

/// A program a launch started.
pub struct Started {
    pub pid: u64,
    pub process: crate::mm::heap::KArc<super::process::Process>,
}

/// The list outputs' version and header size.
pub const LIST_VERSION: u32 = 1;
pub const LIST_HEADER: usize = 8;
/// A window: HWND (u64), process id (u32), screen window id (u32), flags
/// (u32: 1 minimized, 2 maximized, 4 active, 8 snapped), the outer
/// rectangle x, y, width, height (4 x i32), the title's length (u32) and
/// the title (128 UTF-16 units).
pub const WINDOW_ENTRY: usize = 40 + 2 * desktop::LISTED_TITLE;
/// A start menu program: the name's and the path's lengths (2 x u32), the
/// name (32 UTF-16 units) and the path (128).
pub const PROGRAM_ENTRY: usize = 8 + 2 * PROGRAM_NAME + 2 * PROGRAM_PATH;
const PROGRAM_NAME: usize = 32;
const PROGRAM_PATH: usize = 128;
/// A catalog tile: the name's and the package id's lengths (2 x u32), the
/// state (u32, CATALOG_*), how much of its bar is filled (u32 percent,
/// `pkg::Progress::percent`; 0 without an install), gala's exit code
/// (i32, for a failed install, else 0), the name (32 UTF-16 units) and
/// the package id (64).
pub const CATALOG_ENTRY: usize = 20 + 2 * PROGRAM_NAME + 2 * CATALOG_ID;
const CATALOG_ID: usize = 64;
/// A catalog tile's states: its program is not on this machine and no
/// install is under way; it is; its install is resolving, downloading,
/// installing; it failed (with gala's exit code); its `gala install`
/// ended without a word (`pkg::Progress::Lost`).
pub const CATALOG_NOT_INSTALLED: u32 = 0;
pub const CATALOG_INSTALLED: u32 = 1;
pub const CATALOG_RESOLVING: u32 = 2;
pub const CATALOG_DOWNLOADING: u32 = 3;
pub const CATALOG_INSTALLING: u32 = 4;
pub const CATALOG_FAILED: u32 = 5;
pub const CATALOG_LOST: u32 = 6;

/// A Linux section's entry: its kind (u32, LINUX_KIND_*), state (u32,
/// LINUX_STATE_*), how much of its bar is filled (u32 percent,
/// `linux::Progress::percent`; 0 without an install), the index the
/// Linux action takes with it (u32: a distribution's suggestion, the
/// `SUGGESTIONS` index for LINUX_INSTALL, LINUX_NOT_SUGGESTED for one
/// gala does not suggest; an application's index among its
/// distribution's for LINUX_APP; 0 for a terminal), the name's and the
/// label's lengths (2 x u32), the distribution's name (32 UTF-16 units)
/// and the label (48: the application's name, `Terminal` for a
/// terminal, the distribution's name again for a distribution).
pub const LINUX_ENTRY: usize = 24 + 2 * LINUX_NAME + 2 * LINUX_LABEL;
const LINUX_NAME: usize = desktop::linux::NAME;
const LINUX_LABEL: usize = desktop::linux::APP_NAME;
/// A Linux section entry's kinds: a distribution's tile (LINUX_INSTALL
/// when it is not installed, LINUX_DESKTOP when it is), an installed
/// distribution's terminal (LINUX_TERMINAL), one of its applications
/// (LINUX_APP).
pub const LINUX_KIND_DISTRIBUTION: u32 = 0;
pub const LINUX_KIND_TERMINAL: u32 = 1;
pub const LINUX_KIND_APP: u32 = 2;
/// A distribution's states: not registered and no install under way;
/// registered (a terminal's and an application's state too); its
/// `gala linux-install` downloading, importing; failed; gone without a
/// word (`linux::Progress::Lost`).
pub const LINUX_STATE_NOT_INSTALLED: u32 = 0;
pub const LINUX_STATE_INSTALLED: u32 = 1;
pub const LINUX_STATE_DOWNLOADING: u32 = 2;
pub const LINUX_STATE_IMPORTING: u32 = 3;
pub const LINUX_STATE_FAILED: u32 = 4;
pub const LINUX_STATE_LOST: u32 = 5;
/// A distribution entry's index when gala does not suggest it.
pub const LINUX_NOT_SUGGESTED: u32 = u32::MAX;

/// A decoded request.
#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub action: u32,
    pub hwnd: u64,
    pub args: [i32; 4],
    pub text: Vec<u16>,
}

impl Request {
    /// Decodes a request: STATUS_INVALID_PARAMETER for another version, a
    /// short buffer or a text too long.
    pub fn parse(b: &[u8]) -> Result<Request, NtStatus> {
        let u32_at = |at: usize| -> Result<u32, NtStatus> {
            let bytes = b.get(at..at + 4).ok_or(NtStatus::INVALID_PARAMETER)?;
            Ok(u32::from_le_bytes(bytes.try_into().map_err(|_| NtStatus::INVALID_PARAMETER)?))
        };
        if b.len() < REQUEST_HEADER || u32_at(0)? != REQUEST_VERSION {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let len = u32_at(32)? as usize;
        if len > MAX_TEXT || b.len() < REQUEST_HEADER + 2 * len {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let mut text = Vec::new();
        text.try_reserve_exact(len).map_err(|_| NtStatus::NO_MEMORY)?;
        let units = &b[REQUEST_HEADER..REQUEST_HEADER + 2 * len];
        text.extend(units.chunks_exact(2).map(|u| u16::from_le_bytes([u[0], u[1]])));
        let mut hwnd = [0u8; 8];
        hwnd.copy_from_slice(&b[8..16]);
        Ok(Request {
            action: u32_at(4)?,
            hwnd: u64::from_le_bytes(hwnd),
            args: [u32_at(16)? as i32, u32_at(20)? as i32, u32_at(24)? as i32, u32_at(28)? as i32],
            text,
        })
    }
}

/// The window action a request asks for (None: not a window action).
pub fn window_action(r: &Request) -> Result<Option<Action>, NtStatus> {
    Ok(Some(match r.action {
        ACTION_ACTIVATE => Action::Activate,
        ACTION_MINIMIZE => Action::Minimize,
        ACTION_MAXIMIZE => Action::Maximize,
        ACTION_RESTORE => Action::Restore,
        ACTION_CLOSE => Action::Close,
        ACTION_SNAP => Action::Snap(match r.args[0] {
            0 => SnapTo::Left,
            1 => SnapTo::Right,
            2 => SnapTo::Up,
            3 => SnapTo::Down,
            _ => return Err(NtStatus::INVALID_PARAMETER),
        }),
        ACTION_MOVE => {
            let [x, y, w, h] = r.args;
            if w <= 0 || h <= 0 {
                return Err(NtStatus::INVALID_PARAMETER);
            }
            Action::Place { x, y, w: w as u32, h: h as u32 }
        }
        _ => return Ok(None),
    }))
}

/// Who asks (GalaDesktopAction's caller): its process and integrity
/// level for UIPI, the primary token Start gives the program (a copy of
/// the caller's) and its session's desktop: window actions and lists
/// never leave it (ROADMAP RS step B2; another session's HWND is no
/// window, STATUS_INVALID_HANDLE).
pub struct Caller {
    pub pid: u32,
    pub level: u32,
    pub token: Option<super::object::ObjectRef>,
    pub desk: crate::mm::heap::KArc<super::session::Desk>,
}

/// What a request gives back: the bytes it writes to the caller and, for
/// a launch with LAUNCH_HANDLE, the process to give it a handle to
/// (written after the bytes).
pub struct Output {
    pub bytes: Vec<u8>,
    pub process: Option<crate::mm::heap::KArc<super::process::Process>>,
}

/// Runs request `r` for `caller`; what it writes to the caller. A window
/// action needs the caller to pass UIPI against the window's process (an
/// equal or higher integrity level: STATUS_ACCESS_DENIED otherwise);
/// `level_of` gives a process's level (None: it has ended).
pub fn run(
    r: &Request,
    caller: &Caller,
    level_of: impl Fn(u32) -> Option<u32>,
) -> Result<Output, NtStatus> {
    if is_launch(r.action) {
        return run_launch(r, caller);
    }
    run_other(r, caller, level_of).map(|bytes| Output { bytes, process: None })
}

/// A launch verb: the process id, and the process when the caller asked
/// for a handle. Logged with the caller, with a `window` event `launch`.
fn run_launch(r: &Request, caller: &Caller) -> Result<Output, NtStatus> {
    let flags = r.args[0];
    if flags & !(LAUNCH_CONSOLE_WINDOW | LAUNCH_HANDLE) != 0 {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    let mut buf = [0u8; MAX_TEXT];
    let text = ascii(&r.text, &mut buf);
    let token = caller.token.clone();
    let (op, started) = match r.action {
        ACTION_START => {
            let window = flags & LAUNCH_CONSOLE_WINDOW != 0;
            ("start", start(&r.text, window, token, &caller.desk.shell))
        }
        ACTION_INSTALL => {
            let id = text.filter(|id| package_id(id)).ok_or(NtStatus::INVALID_PARAMETER)?;
            ("install", install_for_shell(id, token, &caller.desk))
        }
        _ => {
            let r = linux_request(text, r.args[1], r.args[2])?;
            ("linux", linux_for_shell(r, token, &caller.desk))
        }
    };
    let text = text.unwrap_or("");
    let (pid, status) = match &started {
        Ok(s) => (s.pid, 0),
        Err(e) => (0, e.0),
    };
    crate::info!(
        "desktop: pid {} asks for {op} {text:?} (flags {flags:#x}): {}",
        caller.pid,
        if status == 0 { "started" } else { "not started" }
    );
    crate::events::emit(
        crate::events::Category::Window,
        "launch",
        format_args!(
            r#""op":"{op}","by":{},"text":{},"flags":{flags},"pid":{pid},"status":{status}"#,
            caller.pid,
            crate::events::Str(text)
        ),
    );
    let started = started?;
    let mut bytes = Vec::new();
    bytes.try_extend_from_slice(&started.pid.to_le_bytes()).map_err(|_| NtStatus::NO_MEMORY)?;
    let process = (flags & LAUNCH_HANDLE != 0).then_some(started.process);
    Ok(Output { bytes, process })
}

/// Whether `id` is a package id `gala install` takes as one argument
/// (winget's `Publisher.Package[.Variant]`: letters, digits, `.`, `-`,
/// `_`, `+`).
fn package_id(id: &str) -> bool {
    (1..=MAX_ID).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_+".contains(&b))
}

/// ACTION_LINUX's request: distribution `name`, operation `op`, `index`
/// its suggestion or application. STATUS_INVALID_PARAMETER for a name
/// that is not a distribution's, an operation or a suggestion out of
/// range; STATUS_OBJECT_NAME_NOT_FOUND for an application of a
/// distribution that is not registered or has no such application.
fn linux_request(
    name: Option<&str>,
    op: i32,
    index: i32,
) -> Result<desktop::linux::Request, NtStatus> {
    use desktop::linux::{Name, Request, SUGGESTIONS, valid_name};
    let name = name
        .filter(|n| valid_name(n.as_bytes()))
        .and_then(|n| Name::new(n.as_bytes()))
        .ok_or(NtStatus::INVALID_PARAMETER)?;
    let index = usize::try_from(index).map_err(|_| NtStatus::INVALID_PARAMETER)?;
    Ok(match op {
        LINUX_INSTALL if index < SUGGESTIONS.len() => Request::Install(name, index),
        LINUX_DESKTOP => Request::Desktop(name),
        LINUX_TERMINAL => Request::Terminal(name),
        LINUX_APP => Request::App(
            name,
            desktop::linux_app(name.as_bytes(), index).ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?,
        ),
        _ => return Err(NtStatus::INVALID_PARAMETER),
    })
}

/// The requests that are not launches.
fn run_other(
    r: &Request,
    caller: &Caller,
    level_of: impl Fn(u32) -> Option<u32>,
) -> Result<Vec<u8>, NtStatus> {
    let mut out = Vec::new();
    if let Some(action) = window_action(r)? {
        // Without shasha nothing is on the screen to act on (checked
        // before the window, so a server always says so).
        let shell = &caller.desk.shell;
        if !desktop::has_shell(shell) {
            return Err(NtStatus::NOT_SUPPORTED);
        }
        let info = caller.desk.windows.query(r.hwnd).ok_or(NtStatus::INVALID_HANDLE)?;
        if info.desktop == 0 {
            return Err(NtStatus::INVALID_HANDLE);
        }
        let level = level_of(info.pid).ok_or(NtStatus::INVALID_HANDLE)?;
        let from = super::window::Sender { pid: caller.pid, level: caller.level };
        if !super::window::may_act_on(from, info.pid, level) {
            return Err(NtStatus::ACCESS_DENIED);
        }
        // Each named action on a window leaves a line, so a shell's
        // taskbar click that did nothing says what it asked and why.
        let (pid, hwnd) = (caller.pid, r.hwnd);
        match desktop::window_action(shell, u64::from(info.pid), info.desktop, action) {
            Ok(()) => crate::info!(
                "desktop: pid {pid} asks for {action:?} of window {hwnd:#x} (pid {})",
                info.pid
            ),
            Err(e) => {
                crate::warn!(
                    "desktop: pid {pid} asked for {action:?} of window {hwnd:#x} (refused: {e:?})"
                );
                return Err(e);
            }
        }
        return Ok(out);
    }
    match r.action {
        ACTION_WINDOWS => {
            let listed = desktop::list_windows(&caller.desk.shell)?;
            list_header(&mut out, listed.len())?;
            for w in &listed {
                let hwnd = caller.desk.windows.hwnd_of_desktop(w.owner as u32, w.id).unwrap_or(0);
                window_entry(&mut out, hwnd, w)?;
            }
        }
        ACTION_PROGRAMS => {
            if !desktop::has_shell(&caller.desk.shell) {
                return Err(NtStatus::NOT_SUPPORTED);
            }
            let programs = desktop::start_menu();
            list_header(&mut out, programs.len())?;
            for (name, path) in &programs {
                program_entry(&mut out, name, path)?;
            }
        }
        ACTION_CATALOG => {
            if !desktop::has_shell(&caller.desk.shell) {
                return Err(NtStatus::NOT_SUPPORTED);
            }
            let tiles = desktop::catalog(&caller.desk.shell);
            list_header(&mut out, tiles.len())?;
            for t in &tiles {
                let (state, percent, exit) = catalog_state(t.installed, t.install);
                let id = t.program.package.unwrap_or("");
                catalog_entry(&mut out, t.program.name, id, state, percent, exit)?;
            }
        }
        ACTION_LINUX_LIST => {
            if !desktop::has_shell(&caller.desk.shell) {
                return Err(NtStatus::NOT_SUPPORTED);
            }
            let tiles = desktop::linux_section(&caller.desk.shell);
            linux_entries(&mut out, &tiles)?;
        }
        ACTION_WALLPAPER => {
            let mut name = [0u8; 260];
            let name = ascii(&r.text, &mut name).ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
            desktop::set_wallpaper(&caller.desk.shell, name)?;
            // The shell hears of it as Windows tells every top-level
            // window (WM_SETTINGCHANGE, SPI_SETDESKWALLPAPER), and shows
            // the new value.
            desktop::tell_shell_window(
                &caller.desk.windows,
                WM_SETTINGCHANGE,
                SPI_SETDESKWALLPAPER,
                "the wallpaper",
                true,
            );
        }
        ACTION_LINUX_PROGRESS => {
            let mut name = [0u8; desktop::linux::NAME];
            let name = ascii(&r.text, &mut name).ok_or(NtStatus::INVALID_PARAMETER)?;
            let [state, percent, ..] = r.args;
            linux_progress_noted(name, state, percent);
            desktop::linux_progress(&caller.desk.shell, name.as_bytes(), state, percent)?;
        }
        ACTION_INSTALL_PROGRESS => {
            let mut id = [0u8; MAX_ID];
            let id = ascii(&r.text, &mut id).ok_or(NtStatus::INVALID_PARAMETER)?;
            let [state, percent, exit, _] = r.args;
            install_progress_noted(id, state, percent, exit);
            desktop::install_progress(&caller.desk.shell, id.as_bytes(), state, percent, exit)?;
        }
        _ => return Err(NtStatus::INVALID_PARAMETER),
    }
    Ok(out)
}

fn push(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), NtStatus> {
    out.try_extend_from_slice(bytes).map_err(|_| NtStatus::NO_MEMORY)
}

fn list_header(out: &mut Vec<u8>, count: usize) -> Result<(), NtStatus> {
    push(out, &LIST_VERSION.to_le_bytes())?;
    push(out, &(count as u32).to_le_bytes())
}

fn window_entry(out: &mut Vec<u8>, hwnd: u64, w: &desktop::Listed) -> Result<(), NtStatus> {
    push(out, &hwnd.to_le_bytes())?;
    push(out, &(w.owner as u32).to_le_bytes())?;
    push(out, &w.id.to_le_bytes())?;
    push(out, &w.flags.to_le_bytes())?;
    for v in w.rect {
        push(out, &v.to_le_bytes())?;
    }
    push(out, &(w.title_len as u32).to_le_bytes())?;
    for c in w.title {
        push(out, &c.to_le_bytes())?;
    }
    Ok(())
}

fn program_entry(out: &mut Vec<u8>, name: &str, path: &str) -> Result<(), NtStatus> {
    let (n, p) = (name.len().min(PROGRAM_NAME), path.len().min(PROGRAM_PATH));
    push(out, &(n as u32).to_le_bytes())?;
    push(out, &(p as u32).to_le_bytes())?;
    for (text, room) in [(name, PROGRAM_NAME), (path, PROGRAM_PATH)] {
        let mut units = [0u16; PROGRAM_PATH];
        for (u, b) in units.iter_mut().zip(text.bytes().take(room)) {
            *u = u16::from(b);
        }
        for u in &units[..room] {
            push(out, &u.to_le_bytes())?;
        }
    }
    Ok(())
}

/// A catalog tile's state, bar and exit code (CATALOG_ENTRY's): an
/// installed program is installed whatever its install last said.
pub fn catalog_state(installed: bool, install: Option<desktop::pkg::Progress>) -> (u32, u32, i32) {
    use desktop::pkg::Progress;
    let Some(p) = install.filter(|_| !installed) else {
        return (if installed { CATALOG_INSTALLED } else { CATALOG_NOT_INSTALLED }, 0, 0);
    };
    let state = match p {
        Progress::Resolving => CATALOG_RESOLVING,
        Progress::Downloading(_) => CATALOG_DOWNLOADING,
        Progress::Installing => CATALOG_INSTALLING,
        Progress::Failed(_) => CATALOG_FAILED,
        Progress::Lost(_) => CATALOG_LOST,
    };
    let exit = if let Progress::Failed(e) = p { e } else { 0 };
    (state, u32::from(p.percent()), exit)
}

fn catalog_entry(
    out: &mut Vec<u8>,
    name: &str,
    id: &str,
    state: u32,
    percent: u32,
    exit: i32,
) -> Result<(), NtStatus> {
    let (n, i) = (name.len().min(PROGRAM_NAME), id.len().min(CATALOG_ID));
    for v in [n as u32, i as u32, state, percent] {
        push(out, &v.to_le_bytes())?;
    }
    push(out, &exit.to_le_bytes())?;
    for (text, room) in [(name, PROGRAM_NAME), (id, CATALOG_ID)] {
        let mut units = [0u16; CATALOG_ID];
        for (u, b) in units.iter_mut().zip(text.bytes().take(room)) {
            *u = u16::from(b);
        }
        for u in &units[..room] {
            push(out, &u.to_le_bytes())?;
        }
    }
    Ok(())
}

/// A Linux section distribution's state and bar (LINUX_ENTRY's): a
/// registered distribution is installed whatever its install last said.
pub fn linux_state(installed: bool, install: Option<desktop::linux::Progress>) -> (u32, u32) {
    use desktop::linux::Progress;
    let Some(p) = install.filter(|_| !installed) else {
        return (if installed { LINUX_STATE_INSTALLED } else { LINUX_STATE_NOT_INSTALLED }, 0);
    };
    let state = match p {
        Progress::Downloading(_) => LINUX_STATE_DOWNLOADING,
        Progress::Importing => LINUX_STATE_IMPORTING,
        Progress::Failed => LINUX_STATE_FAILED,
        Progress::Lost(_) => LINUX_STATE_LOST,
    };
    (state, u32::from(p.percent()))
}

/// LinuxList's output for `tiles`: the header, then each tile's entry,
/// an installed distribution's followed by its terminal's and its
/// applications'.
fn linux_entries(out: &mut Vec<u8>, tiles: &[desktop::LinuxTile]) -> Result<(), NtStatus> {
    let count: usize = tiles
        .iter()
        .map(|t| if t.distro.installed { 2 + t.distro.apps().count() } else { 1 })
        .sum();
    list_header(out, count)?;
    for t in tiles {
        let d = &t.distro;
        let name = d.name.as_str();
        let (state, percent) = linux_state(d.installed, t.install);
        let index = d.suggestion.map_or(LINUX_NOT_SUGGESTED, |s| s as u32);
        linux_entry(out, LINUX_KIND_DISTRIBUTION, state, percent, index, name, name)?;
        if !d.installed {
            continue;
        }
        linux_entry(out, LINUX_KIND_TERMINAL, state, 0, 0, name, "Terminal")?;
        for (k, app) in d.apps().enumerate() {
            linux_entry(out, LINUX_KIND_APP, state, 0, k as u32, name, app.name.as_str())?;
        }
    }
    Ok(())
}

fn linux_entry(
    out: &mut Vec<u8>,
    kind: u32,
    state: u32,
    percent: u32,
    index: u32,
    name: &str,
    label: &str,
) -> Result<(), NtStatus> {
    let mut texts = [([0u16; LINUX_LABEL], 0usize); 2];
    for ((units, n), (text, room)) in
        texts.iter_mut().zip([(name, LINUX_NAME), (label, LINUX_LABEL)])
    {
        // Whole characters only: one cut at the room's end keeps none of
        // its surrogates.
        for c in text.chars() {
            let mut pair = [0u16; 2];
            let c = c.encode_utf16(&mut pair);
            if *n + c.len() > room {
                break;
            }
            units[*n..*n + c.len()].copy_from_slice(c);
            *n += c.len();
        }
    }
    for v in [kind, state, percent, index, texts[0].1 as u32, texts[1].1 as u32] {
        push(out, &v.to_le_bytes())?;
    }
    for ((units, _), room) in texts.iter().zip([LINUX_NAME, LINUX_LABEL]) {
        for u in &units[..room] {
            push(out, &u.to_le_bytes())?;
        }
    }
    Ok(())
}

/// `text` as ASCII in `buf` (None: not ASCII, or too long).
fn ascii<'a>(text: &[u16], buf: &'a mut [u8]) -> Option<&'a str> {
    if text.len() > buf.len() {
        return None;
    }
    for (b, &c) in buf.iter_mut().zip(text) {
        *b = u8::try_from(c).ok().filter(u8::is_ascii)?;
    }
    core::str::from_utf8(&buf[..text.len()]).ok()
}

/// Splits Start's text into the program and its arguments: the program
/// runs to the first space, or is quoted.
pub fn split_program(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    if let Some(rest) = text.strip_prefix('"') {
        return match rest.find('"') {
            Some(end) => (&rest[..end], rest[end + 1..].trim_start()),
            None => (rest, ""),
        };
    }
    match text.find(' ') {
        Some(at) => (&text[..at], text[at + 1..].trim_start()),
        None => (text, ""),
    }
}

/// Where Start looks for a program named without a path, in order.
const SEARCH: [&str; 3] = ["C:\\gala\\", "C:\\", "C:\\Windows\\System32\\"];

/// Starts the program `text` names (a path on C:, a start menu name, or a
/// file name found in SEARCH, `.exe` added if missing), with the rest of
/// `text` as its arguments, under primary token `token` (see
/// `start_path`), a console program in a console window of its own when
/// `window` says so (`start_in_window`). STATUS_OBJECT_NAME_NOT_FOUND
/// when there is no such program.
pub fn start(
    text: &[u16],
    window: bool,
    token: Option<super::object::ObjectRef>,
    shell: &desktop::Shell,
) -> Result<Started, NtStatus> {
    let mut buf = [0u8; MAX_TEXT];
    let text = ascii(text, &mut buf).ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
    let (program, args) = split_program(text);
    if program.is_empty() {
        return Err(NtStatus::OBJECT_NAME_NOT_FOUND);
    }
    let path = resolve(program, shell)?;
    if window { start_in_window(path, args, token) } else { start_path(path, args, token) }
}

/// Starts the program at `path` (shasha's start menu: `token` None, the
/// boot user; `gala start` and the launch verbs: a copy of the caller's
/// primary token), logged.
pub fn start_path(
    path: &'static str,
    args: &str,
    token: Option<super::object::ObjectRef>,
) -> Result<Started, NtStatus> {
    match super::process::launch(path, args, token, None) {
        Ok(process) => {
            let pid = process.id;
            crate::info!("desktop: started {path} (pid {pid})");
            Ok(Started { pid, process })
        }
        Err(e) => {
            crate::warn!("desktop: cannot start {path}: {e:?}");
            Err(e)
        }
    }
}

/// Where the agent panel is installed (ARCHITECTURE "The agent panel").
pub const AGENT: &str = "C:\\Windows\\System32\\agent.exe";

/// Starts the agent panel with the session (the boot user's), when it is
/// installed; a desktop without one says so.
pub fn start_agent() {
    match super::process::open_program(AGENT) {
        Ok(_) => {
            let _ = start_path(AGENT, "", None);
        }
        Err(e) => crate::info!("desktop: no agent panel: {AGENT} not opened ({e:?})"),
    }
}

/// Where the user-mode shell is installed (ARCHITECTURE "The user-mode
/// shell"): an image with a screen installs it there, as Windows ships
/// Explorer; the boot's C:\gala copy is a test program's module and is
/// not started (without System32's DLLs it does not load).
pub const SHASHA: &str = "C:\\Windows\\System32\\shasha.exe";

/// Starts the user-mode shell with the session (the default; `shell=`
/// with another name leaves it out), when it is installed; else the
/// screen shows the desktop with no shell, and says why
/// (`desktop::no_shell`).
pub fn start_shell() {
    // A shell the previous kernel handed over goes on (not a second one,
    // which the shell window would refuse).
    if let Some(p) = crate::handover::carry::resumed("shasha.exe") {
        crate::info!(
            "desktop: the user-mode shell goes on from the previous kernel (pid {})",
            p.id
        );
        return;
    }
    match super::process::open_program(SHASHA) {
        Ok(_) => {
            let _ = start_path(SHASHA, "", None);
        }
        Err(e) => {
            crate::info!("desktop: no user-mode shell: {SHASHA} not opened ({e:?})");
            desktop::no_shell("shasha.exe is not installed");
        }
    }
}

/// Where gala's own programs are that the shell's actions run: gala.exe for
/// `gala install`, conhost.exe for console windows (System32 as kernel32's
/// CreatePseudoConsole looks there, else the boot modules' C:\gala).
const GALA_EXE: &[&str] = &["C:\\gala\\gala.exe"];
const CONHOST_EXE: &[&str] = &["%SystemRoot%\\System32\\conhost.exe", "C:\\gala\\conhost.exe"];

/// The first of `paths` that exists (expanded; interned).
fn first_existing(paths: &'static [&'static str]) -> Option<&'static str> {
    let p = desktop::Program { name: "", paths, package: None };
    let mut buf = [0; 260];
    let (_, path) = p.find(&mut |path| super::process::program_exists(path), &mut buf)?;
    super::process::intern(path).ok()
}

/// Installs catalog package `id` (a `package_id`) under primary token
/// `token` (None: the boot user's): `gala install <id>` with no window.
/// Its output is in the log (a process without a console writes there)
/// and its progress comes back through ACTION_INSTALL_PROGRESS.
fn start_install(id: &str, token: Option<super::object::ObjectRef>) -> Result<Started, NtStatus> {
    let Some(gala) = first_existing(GALA_EXE) else {
        crate::warn!("desktop: cannot install {id}: no gala.exe at {}", GALA_EXE[0]);
        return Err(NtStatus::OBJECT_NAME_NOT_FOUND);
    };
    let mut buf = [0u8; 160];
    let args = join(&mut buf, &[b"install ", id.as_bytes()]).ok_or(NtStatus::BUFFER_TOO_SMALL)?;
    start_path(gala, args, token)
}

/// GalaDesktopAction's Install of package `id` for the caller's desktop
/// `desk`: a catalog entry's tile is resolving before `gala install`
/// starts (`desktop::begin_install`), and the installer's pid is watched
/// so that one ending without a word leaves the tile quiet instead of
/// waiting for ever (`installer_registered`). A package the catalog does
/// not list just installs.
fn install_for_shell(
    id: &str,
    token: Option<super::object::ObjectRef>,
    desk: &super::session::Desk,
) -> Result<Started, NtStatus> {
    let program = desktop::program_with_package(id.as_bytes())
        .and_then(|k| desktop::PROGRAMS.get(k).map(|p| (k, p)))
        .filter(|_| desktop::has_shell(&desk.shell));
    let Some((k, program)) = program else {
        return start_install(id, token);
    };
    desktop::begin_install(&desk.shell, k);
    let started = start_install(id, token);
    installer_registered(desk.session, program, started.as_ref().ok());
    started
}

/// Tells the desktop which `gala install` `program`'s tile waits for a
/// word from (None: it never started). A process can end before it is
/// watched, and its end found no install to quiet then: one that has
/// ended by now is told as gone (`desktop::process_ended` again), so
/// the tile does not wait for a word that will not come.
fn installer_registered(
    session: u32,
    program: &'static desktop::Program,
    started: Option<&Started>,
) {
    desktop::install_started(session, program, started.map(|s| s.pid));
    if let Some(s) = started
        && s.process.exit_status().is_some()
    {
        crate::info!(
            "desktop: start menu: {}'s installer (pid {}) ended before it was watched",
            program.name,
            s.pid
        );
        if let Some(desk) = super::session::desk(session) {
            desktop::process_ended(&desk.shell, s.pid);
        }
    }
}

/// A system key on a desktop asked for `program` (Ctrl+Shift+Esc: the
/// Task Manager), from the console's keys or injected input: started as
/// the boot user, whose session is the console's at boot, in a console
/// window when it is a console program (`start_in_window`). Logged, with
/// a `window` event `system key`. Another session's key starts nothing
/// yet: a session has no user of its own until B4's session manager
/// (ROADMAP RS), so a program started for it would land in session 1.
pub fn start_for_system_key(session: u32, program: &'static desktop::Program) {
    if session != super::session::CONSOLE {
        crate::warn!(
            "desktop: session {session}'s system key asked for {}: not started (a session other than {} has no user to start it as yet)",
            program.name,
            super::session::CONSOLE
        );
        super::ledger::record("system key", "session without a user", session.into());
        return;
    }
    let mut buf = [0u8; 260];
    let path = program
        .find(&mut |path| super::process::program_exists(path), &mut buf)
        .and_then(|(_, p)| super::process::intern(p).ok());
    let (target, started) = match path {
        Some(path) => {
            crate::info!("desktop: system key starts {}: {path}", program.name);
            (path, start_in_window(path, "", None))
        }
        None => {
            crate::warn!(
                "desktop: system key: {} is not on this machine (none of {:?} exists)",
                program.name,
                program.paths
            );
            ("", Err(NtStatus::OBJECT_NAME_NOT_FOUND))
        }
    };
    let (pid, status) = match &started {
        Ok(s) => (s.pid, 0),
        Err(e) => (0, e.0),
    };
    crate::events::emit(
        crate::events::Category::Window,
        "system key",
        format_args!(
            r#""session":{session},"name":{},"target":{},"pid":{pid},"status":{status}"#,
            crate::events::Str(program.name),
            crate::events::Str(target)
        ),
    );
}

/// wsl.exe, where Windows has it.
const WSL_EXE: &[&str] = &["%SystemRoot%\\System32\\wsl.exe"];
/// LD step 6's desktop session, in the distribution (rootful Xwayland on
/// display :1 as one window, twm and an xterm: user/lx/xdesktop).
const DESKTOP_SESSION: &str = "/lx/xdesktop/session.sh";

/// The `linux_mirror=` boot option: suggestions are fetched from
/// `<mirror>/<file name>` (the scenario's host).
static MIRROR: crate::sync::IrqMutex<Option<&'static str>> = crate::sync::IrqMutex::new(None);

/// `linux_mirror=<base URL>` (percent-decoded by the caller, interned).
pub fn set_linux_mirror(url: &'static str) {
    *MIRROR.lock() = Some(url);
}

/// LinuxProgress logged when its state changes (and each tenth of a
/// download), with a `window` event.
fn linux_progress_noted(name: &str, state: i32, percent: i32) {
    use core::sync::atomic::{AtomicI32, Ordering};
    static LAST: AtomicI32 = AtomicI32::new(-1);
    let key = state * 1000 + if state == 0 { percent / 10 } else { 0 };
    if LAST.swap(key, Ordering::Relaxed) == key {
        return;
    }
    let what = match state {
        0 => "downloading",
        1 => "importing",
        2 => "installed",
        3 => "failed",
        _ => "unknown state",
    };
    crate::info!("desktop: start menu: Linux distribution {name}: {what} ({percent}%)");
    crate::events::emit(
        crate::events::Category::Window,
        "start menu",
        format_args!(
            r#""op":"linux progress","name":{},"state":"{what}","percent":{percent}"#,
            crate::events::Str(name)
        ),
    );
}

/// The longest package id a progress report carries (winget's ids are
/// `Publisher.Package[.Variant]`; the catalog's longest is far shorter).
const MAX_ID: usize = 128;

/// InstallProgress logged when its state changes (and each tenth of a
/// download), with a `window` event. The failure says gala's exit code
/// and what it stands for; which step failed is in gala's own line
/// above it, since a tile's install has no window to show it in.
fn install_progress_noted(id: &str, state: i32, percent: i32, exit: i32) {
    use core::sync::atomic::{AtomicI32, Ordering};
    static LAST: AtomicI32 = AtomicI32::new(-1);
    let key = state * 1000 + if state == 1 { percent / 10 } else { 0 };
    if LAST.swap(key, Ordering::Relaxed) == key {
        return;
    }
    let what = match state {
        0 => "resolving",
        1 => "downloading",
        2 => "installing",
        3 => "installed",
        4 => "failed",
        _ => "unknown state",
    };
    if state == 4 {
        // What the code stands for when it stands for something; gala's
        // own line above says which step failed either way.
        match desktop::pkg::install_failure(exit) {
            Some(why) => {
                crate::info!("desktop: start menu: install {id}: failed (gala exit {exit}: {why})");
            }
            None => crate::info!("desktop: start menu: install {id}: failed (gala exit {exit})"),
        }
    } else {
        crate::info!("desktop: start menu: install {id}: {what} ({percent}%)");
    }
    crate::events::emit(
        crate::events::Category::Window,
        "start menu",
        format_args!(
            r#""op":"install progress","name":{},"state":"{what}","percent":{percent},"exit":{exit}"#,
            crate::events::Str(id)
        ),
    );
}

/// GalaDesktopAction's Linux for the caller's desktop `desk`: an install
/// of a distribution makes its tile downloading before `gala
/// linux-install` starts (`desktop::begin_linux_install`), and the
/// installer's pid is watched so that one ending without a word leaves
/// the tile quiet instead of waiting for ever
/// (`linux_installer_registered`). Without a shell it just starts.
fn linux_for_shell(
    r: desktop::linux::Request,
    token: Option<super::object::ObjectRef>,
    desk: &super::session::Desk,
) -> Result<Started, NtStatus> {
    let desktop::linux::Request::Install(name, _) = r else {
        return launch_linux(r, token);
    };
    if !desktop::has_shell(&desk.shell) {
        return launch_linux(r, token);
    }
    desktop::begin_linux_install(&desk.shell, name);
    let started = launch_linux(r, token);
    linux_installer_registered(desk.session, name, started.as_ref().ok());
    started
}

/// Tells the desktop which `gala linux-install` distribution `name`'s
/// tile waits for a word from (None: it never started), as
/// `installer_registered` does for a package's: one that has ended
/// before it was watched is told as gone (`desktop::process_ended`
/// again).
fn linux_installer_registered(session: u32, name: desktop::linux::Name, started: Option<&Started>) {
    desktop::linux_install_started(session, name, started.map(|s| s.pid));
    if let Some(s) = started
        && s.process.exit_status().is_some()
    {
        crate::info!(
            "desktop: start menu: {}'s installer (pid {}) ended before it was watched",
            name.as_str(),
            s.pid
        );
        if let Some(desk) = super::session::desk(session) {
            desktop::process_ended(&desk.shell, s.pid);
        }
    }
}

/// Starts what a Linux section asks for under primary token `token`
/// (None: the boot user's): an install (`gala linux-install <name>
/// <url>`, no window: its progress is on the tile), a distribution's
/// desktop (wsl.exe running LD step 6's session, no window of its own),
/// a terminal (`wsl -d <name>` in a console window) or an application
/// (`wsl -d <name> --exec <its Exec>`). Logged; STATUS_INVALID_PARAMETER
/// for a name that is not a distribution's, STATUS_OBJECT_NAME_NOT_FOUND
/// for a desktop the distribution has no session for or a missing
/// program.
pub fn launch_linux(
    r: desktop::linux::Request,
    token: Option<super::object::ObjectRef>,
) -> Result<Started, NtStatus> {
    use desktop::linux::{Request, SUGGESTIONS, exec_words, source_url, valid_name};
    let name = match r {
        Request::Install(n, _)
        | Request::Desktop(n)
        | Request::Terminal(n)
        | Request::App(n, _) => n,
    };
    let name_text = name.as_str();
    if !valid_name(name.as_bytes()) {
        crate::warn!("desktop: start menu: {name_text:?} is not a distribution's name");
        return Err(NtStatus::INVALID_PARAMETER);
    }
    let mut args = [0u8; 512];
    let (op, program, args, window): (&str, Option<&'static str>, Option<&str>, bool) = match r {
        Request::Install(_, s) => {
            let mirror = *MIRROR.lock();
            let mut url = [0u8; 300];
            let url = SUGGESTIONS.get(s).and_then(|s| source_url(s, mirror, &mut url));
            let args = url.and_then(|u| {
                join(&mut args, &[b"linux-install ", name.as_bytes(), b" ", u.as_bytes()])
            });
            ("linux install", first_existing(GALA_EXE), args, false)
        }
        Request::Desktop(_) => {
            // The session is gala's script in the distribution (the
            // stand-in rootfs carries it); without it nothing starts.
            let has = desktop::distro_root_of(name.as_bytes()).is_some_and(|root| {
                crate::lx::file::resolve_in(root, DESKTOP_SESSION.as_bytes(), true)
                    .is_ok_and(|f| f.node.is_some())
            });
            if !has {
                crate::warn!(
                    "desktop: start menu: {name_text} has no desktop session ({DESKTOP_SESSION} is not in it): not started"
                );
                super::ledger::record("start menu", "linux desktop session missing", 0);
                return Err(NtStatus::OBJECT_NAME_NOT_FOUND);
            }
            let args = join(
                &mut args,
                &[b"-d ", name.as_bytes(), b" --exec /bin/sh ", DESKTOP_SESSION.as_bytes()],
            );
            ("linux desktop", first_existing(WSL_EXE), args, false)
        }
        Request::Terminal(_) => (
            "linux terminal",
            first_existing(WSL_EXE),
            join(&mut args, &[b"-d ", name.as_bytes()]),
            true,
        ),
        Request::App(_, app) => {
            // --exec's words, each quoted (CommandLineToArgvW's rules; a
            // word with a quote of its own is left out).
            let mut n = 0;
            let mut ok = true;
            let mut put = |part: &[u8]| {
                ok &= args.get_mut(n..n + part.len()).map(|s| s.copy_from_slice(part)).is_some();
                n += part.len();
            };
            put(b"-d ");
            put(name.as_bytes());
            put(b" --exec");
            for word in exec_words(app.exec.as_bytes()).iter().filter(|w| !w.contains(&b'"')) {
                put(b" \"");
                put(word);
                put(b"\"");
            }
            let a = if ok { core::str::from_utf8(&args[..n]).ok() } else { None };
            ("linux app", first_existing(WSL_EXE), a, false)
        }
    };
    match (program, args) {
        (Some(program), Some(args)) => {
            crate::info!("desktop: start menu: {op} {name_text}: {program} {args}");
            if window {
                start_in_window(program, args, token)
            } else {
                start_path(program, args, token)
            }
        }
        (None, _) => {
            crate::warn!(
                "desktop: start menu: {op} {name_text}: no {}",
                if matches!(r, Request::Install(..)) { GALA_EXE[0] } else { WSL_EXE[0] }
            );
            Err(NtStatus::OBJECT_NAME_NOT_FOUND)
        }
        (Some(_), None) => {
            crate::warn!("desktop: start menu: {op} {name_text}: its command line does not fit");
            Err(NtStatus::BUFFER_TOO_SMALL)
        }
    }
}

/// Starts the program at `path` with `args` under primary token `token`
/// (`start_path`) and, when it is a console program, in a new console
/// window: a console
/// session hosted by conhost.exe, started first with the session's id and
/// the program's file name, as `console=` does at boot. On Windows a
/// console program started from Explorer gets its own console the same
/// way (CreateProcess without a console to inherit).
fn start_in_window(
    path: &'static str,
    args: &str,
    token: Option<super::object::ObjectRef>,
) -> Result<Started, NtStatus> {
    if !super::process::is_console_program(path)? {
        return start_path(path, args, token);
    }
    let Some(host) = first_existing(CONHOST_EXE) else {
        crate::warn!("desktop: no conhost.exe for {path}'s console window ({CONHOST_EXE:?})");
        return Err(NtStatus::OBJECT_NAME_NOT_FOUND);
    };
    let session = super::conhost::create()?;
    let file = path.rsplit('\\').next().unwrap_or(path);
    // conhost.exe's arguments: the session's id and the program's name.
    let mut digits = [0u8; 10];
    let mut d = digits.len();
    let mut id = session.id;
    loop {
        d -= 1;
        digits[d] = b'0' + (id % 10) as u8;
        id /= 10;
        if id == 0 {
            break;
        }
    }
    let mut buf = [0u8; 128];
    let host_args =
        join(&mut buf, &[&digits[d..], b" ", file.as_bytes()]).ok_or(NtStatus::BUFFER_TOO_SMALL)?;
    // The console window's host runs as the program does: a Low
    // program's conhost is Low too, so the program's console is no way
    // up to a higher level (conhost.exe under the caller's token, as
    // Windows starts it for a CreateProcess with CREATE_NEW_CONSOLE).
    start_path(host, host_args, token.clone())?;
    match super::process::launch(path, args, token, Some(session)) {
        Ok(process) => {
            let pid = process.id;
            crate::info!("desktop: started {path} (pid {pid}) in a console window ({host})");
            Ok(Started { pid, process })
        }
        Err(e) => {
            crate::warn!("desktop: cannot start {path}: {e:?}");
            Err(e)
        }
    }
}

/// `parts` one after another in `buf` (None: they do not fit).
fn join<'b>(buf: &'b mut [u8], parts: &[&[u8]]) -> Option<&'b str> {
    let mut n = 0;
    for part in parts {
        buf.get_mut(n..n + part.len())?.copy_from_slice(part);
        n += part.len();
    }
    core::str::from_utf8(&buf[..n]).ok()
}

/// The path of the program `name` names (see `start`; the start menu's
/// names when `shell`'s desktop is on the screen).
fn resolve(name: &str, shell: &desktop::Shell) -> Result<&'static str, NtStatus> {
    let mut path = [0u8; MAX_TEXT + 32];
    let has_drive = name.len() > 2 && name.as_bytes()[1] == b':';
    if !has_drive {
        // A start menu name.
        if desktop::has_shell(shell)
            && let Some((_, path)) =
                desktop::start_menu().iter().find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            return Ok(path);
        }
    }
    let exe = if name.len() > 4 && name[name.len() - 4..].eq_ignore_ascii_case(".exe") {
        ""
    } else {
        ".exe"
    };
    let dirs: &[&str] = if has_drive { &[""] } else { &SEARCH };
    for dir in dirs {
        for ext in ["", exe] {
            let mut n = 0;
            for part in [*dir, name, ext] {
                for b in part.bytes() {
                    if n == path.len() {
                        return Err(NtStatus::OBJECT_NAME_NOT_FOUND);
                    }
                    path[n] = if b == b'/' { b'\\' } else { b };
                    n += 1;
                }
            }
            let candidate =
                core::str::from_utf8(&path[..n]).map_err(|_| NtStatus::OBJECT_NAME_NOT_FOUND)?;
            if super::process::program_exists(candidate) {
                return super::process::intern(candidate).map_err(|_| NtStatus::NO_MEMORY);
            }
        }
    }
    Err(NtStatus::OBJECT_NAME_NOT_FOUND)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn request(action: u32, hwnd: u64, args: [i32; 4], text: &str) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&REQUEST_VERSION.to_le_bytes());
        b.extend_from_slice(&action.to_le_bytes());
        b.extend_from_slice(&hwnd.to_le_bytes());
        for a in args {
            b.extend_from_slice(&a.to_le_bytes());
        }
        let units: Vec<u16> = text.encode_utf16().collect();
        b.extend_from_slice(&(units.len() as u32).to_le_bytes());
        b.extend_from_slice(&[0; 4]);
        for u in units {
            b.extend_from_slice(&u.to_le_bytes());
        }
        b
    }

    /// A catalog tile's state: an installed program is installed
    /// whatever its install last said; otherwise where its install is,
    /// its bar (`Progress::percent`) and, when it failed, gala's code.
    #[test]
    fn catalog_tiles_say_where_their_install_is() {
        use desktop::pkg::Progress;
        assert_eq!(catalog_state(false, None), (CATALOG_NOT_INSTALLED, 0, 0));
        assert_eq!(catalog_state(true, None), (CATALOG_INSTALLED, 0, 0));
        assert_eq!(catalog_state(true, Some(Progress::Failed(4))), (CATALOG_INSTALLED, 0, 0));
        assert_eq!(catalog_state(false, Some(Progress::Resolving)), (CATALOG_RESOLVING, 5, 0));
        assert_eq!(
            catalog_state(false, Some(Progress::Downloading(50))),
            (CATALOG_DOWNLOADING, 40, 0)
        );
        assert_eq!(catalog_state(false, Some(Progress::Installing)), (CATALOG_INSTALLING, 85, 0));
        assert_eq!(catalog_state(false, Some(Progress::Failed(4))), (CATALOG_FAILED, 100, 4));
        assert_eq!(catalog_state(false, Some(Progress::Lost(40))), (CATALOG_LOST, 40, 0));
    }

    /// A catalog entry's bytes, as shasha reads them.
    #[test]
    fn catalog_entries_are_laid_out_as_documented() {
        let mut out = Vec::new();
        catalog_entry(&mut out, "7-Zip", "7zip.7zip", CATALOG_FAILED, 100, 4).unwrap();
        assert_eq!(out.len(), CATALOG_ENTRY);
        assert_eq!(CATALOG_ENTRY, 212);
        let u32_at = |at: usize| u32::from_le_bytes(out[at..at + 4].try_into().unwrap());
        assert_eq!([u32_at(0), u32_at(4), u32_at(8), u32_at(12), u32_at(16)], [5, 9, 5, 100, 4]);
        let units = |at: usize, n: usize| -> String {
            let u: Vec<u16> =
                out[at..at + 2 * n].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            String::from_utf16(&u).unwrap()
        };
        assert_eq!(units(20, 5), "7-Zip");
        assert_eq!(units(20 + 64, 9), "7zip.7zip");
        // A name too long for its room is cut there, its length too.
        let mut out = Vec::new();
        let long = "n".repeat(40);
        catalog_entry(&mut out, &long, "a.b", CATALOG_NOT_INSTALLED, 0, -1).unwrap();
        assert_eq!(out.len(), CATALOG_ENTRY);
        assert_eq!(u32::from_le_bytes(out[0..4].try_into().unwrap()), 32);
        assert_eq!(i32::from_le_bytes(out[16..20].try_into().unwrap()), -1);
    }

    /// A Linux section distribution's state: a registered one is
    /// installed whatever its install last said; otherwise where its
    /// install is and its bar (`Progress::percent`).
    #[test]
    fn linux_tiles_say_where_their_install_is() {
        use desktop::linux::Progress;
        assert_eq!(linux_state(false, None), (LINUX_STATE_NOT_INSTALLED, 0));
        assert_eq!(linux_state(true, None), (LINUX_STATE_INSTALLED, 0));
        assert_eq!(linux_state(true, Some(Progress::Failed)), (LINUX_STATE_INSTALLED, 0));
        assert_eq!(
            linux_state(false, Some(Progress::Downloading(0))),
            (LINUX_STATE_DOWNLOADING, 0)
        );
        assert_eq!(
            linux_state(false, Some(Progress::Downloading(60))),
            (LINUX_STATE_DOWNLOADING, 48)
        );
        assert_eq!(linux_state(false, Some(Progress::Importing)), (LINUX_STATE_IMPORTING, 90));
        assert_eq!(linux_state(false, Some(Progress::Failed)), (LINUX_STATE_FAILED, 100));
        assert_eq!(linux_state(false, Some(Progress::Lost(48))), (LINUX_STATE_LOST, 48));
    }

    /// LinuxList's bytes, as shasha reads them: a suggestion not
    /// installed is one entry; an installed distribution is followed by
    /// its terminal and its applications, each with the index the Linux
    /// action takes.
    #[test]
    fn linux_entries_are_laid_out_as_documented() {
        use desktop::linux::{App, Distro, Name, Progress, Text};
        assert_eq!(LINUX_ENTRY, 184);
        let mut ubuntu = Distro::new(Name::cut("Ubuntu"), true);
        ubuntu.apps[0] = Some(App { name: Text::cut("XTerm"), exec: Text::cut("xterm") });
        let tiles = [
            desktop::LinuxTile { distro: ubuntu, install: None },
            desktop::LinuxTile {
                distro: Distro::new(Name::cut("Alpine"), false),
                install: Some(Progress::Downloading(50)),
            },
            desktop::LinuxTile { distro: Distro::new(Name::cut("Mine"), true), install: None },
        ];
        let mut out = Vec::new();
        linux_entries(&mut out, &tiles).unwrap();
        assert_eq!(out.len(), LIST_HEADER + 6 * LINUX_ENTRY);
        let u32_at = |at: usize| u32::from_le_bytes(out[at..at + 4].try_into().unwrap());
        assert_eq!([u32_at(0), u32_at(4)], [LIST_VERSION, 6]);
        let units = |at: usize, n: usize| -> String {
            let u: Vec<u16> =
                out[at..at + 2 * n].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            String::from_utf16(&u).unwrap()
        };
        let entry = |k: usize| {
            let at = LIST_HEADER + k * LINUX_ENTRY;
            let head: Vec<u32> = (0..6).map(|i| u32_at(at + 4 * i)).collect();
            let name = units(at + 24, head[4] as usize);
            let label = units(at + 24 + 2 * LINUX_NAME, head[5] as usize);
            (head[..4].to_vec(), name, label)
        };
        let e = |kind, state, percent, index, name: &str, label: &str| {
            (vec![kind, state, percent, index], String::from(name), String::from(label))
        };
        let (d, t, a) = (LINUX_KIND_DISTRIBUTION, LINUX_KIND_TERMINAL, LINUX_KIND_APP);
        let installed = LINUX_STATE_INSTALLED;
        assert_eq!(entry(0), e(d, installed, 0, 0, "Ubuntu", "Ubuntu"));
        assert_eq!(entry(1), e(t, installed, 0, 0, "Ubuntu", "Terminal"));
        assert_eq!(entry(2), e(a, installed, 0, 0, "Ubuntu", "XTerm"));
        assert_eq!(entry(3), e(d, LINUX_STATE_DOWNLOADING, 40, 1, "Alpine", "Alpine"));
        assert_eq!(entry(4), e(d, installed, 0, LINUX_NOT_SUGGESTED, "Mine", "Mine"));
        assert_eq!(entry(5), e(t, installed, 0, 0, "Mine", "Terminal"));
        // A label too long for its room is cut at a whole character.
        let mut out = Vec::new();
        let long = "\u{1f427}".repeat(30);
        linux_entry(&mut out, a, installed, 0, 0, "Ubuntu", &long).unwrap();
        assert_eq!(out.len(), LINUX_ENTRY);
        assert_eq!(u32::from_le_bytes(out[20..24].try_into().unwrap()), 48);
    }

    #[test]
    fn requests_decode_and_name_their_window_actions() {
        let b = request(ACTION_SNAP, 0x20010, [1, 0, 0, 0], "");
        let r = Request::parse(&b).unwrap();
        assert_eq!((r.action, r.hwnd, r.args[0]), (ACTION_SNAP, 0x20010, 1));
        assert_eq!(window_action(&r), Ok(Some(Action::Snap(SnapTo::Right))));
        let r = Request::parse(&request(ACTION_MOVE, 1, [10, 20, 300, 200], "")).unwrap();
        assert_eq!(window_action(&r), Ok(Some(Action::Place { x: 10, y: 20, w: 300, h: 200 })));
        // A size of 0 or less, a side that does not exist: refused.
        let r = Request::parse(&request(ACTION_MOVE, 1, [10, 20, 0, 200], "")).unwrap();
        assert_eq!(window_action(&r), Err(NtStatus::INVALID_PARAMETER));
        let r = Request::parse(&request(ACTION_SNAP, 1, [4, 0, 0, 0], "")).unwrap();
        assert_eq!(window_action(&r), Err(NtStatus::INVALID_PARAMETER));
        for (a, want) in [
            (ACTION_ACTIVATE, Action::Activate),
            (ACTION_MINIMIZE, Action::Minimize),
            (ACTION_MAXIMIZE, Action::Maximize),
            (ACTION_RESTORE, Action::Restore),
            (ACTION_CLOSE, Action::Close),
        ] {
            let r = Request::parse(&request(a, 1, [0; 4], "")).unwrap();
            assert_eq!(window_action(&r), Ok(Some(want)));
        }
        // Start and the lists are not window actions; the text comes along.
        let r = Request::parse(&request(ACTION_START, 0, [0; 4], "gui_test x")).unwrap();
        assert_eq!(window_action(&r), Ok(None));
        assert_eq!(String::from_utf16(&r.text).unwrap(), "gui_test x");
        // Another version, a short buffer, a text longer than it says.
        let mut b = request(ACTION_START, 0, [0; 4], "abc");
        b[0] = 2;
        assert_eq!(Request::parse(&b), Err(NtStatus::INVALID_PARAMETER));
        b[0] = 1;
        assert_eq!(Request::parse(&b[..REQUEST_HEADER + 4]), Err(NtStatus::INVALID_PARAMETER));
        assert_eq!(Request::parse(&b[..20]), Err(NtStatus::INVALID_PARAMETER));
    }

    #[test]
    fn start_splits_the_program_from_its_arguments() {
        assert_eq!(split_program("gui_test.exe"), ("gui_test.exe", ""));
        assert_eq!(split_program("  a.exe one two"), ("a.exe", "one two"));
        assert_eq!(split_program("\"C:\\My Apps\\a.exe\" -x"), ("C:\\My Apps\\a.exe", "-x"));
        assert_eq!(split_program("\"open"), ("open", ""));
    }

    #[test]
    fn windows_list_as_fixed_entries() {
        let mut title = [0u16; desktop::LISTED_TITLE];
        title[..2].copy_from_slice(&[u16::from(b'H'), u16::from(b'i')]);
        let w = desktop::Listed {
            id: 3,
            owner: 77,
            flags: desktop::LISTED_ACTIVE,
            rect: [0, 0, 640, 752],
            title,
            title_len: 2,
        };
        let mut out = Vec::new();
        list_header(&mut out, 1).unwrap();
        window_entry(&mut out, 0x20030, &w).unwrap();
        assert_eq!(out.len(), LIST_HEADER + WINDOW_ENTRY);
        assert_eq!(out[4..8], 1u32.to_le_bytes());
        let e = &out[LIST_HEADER..];
        assert_eq!(e[0..8], 0x20030u64.to_le_bytes());
        assert_eq!(e[8..12], 77u32.to_le_bytes());
        assert_eq!(e[16..20], 4u32.to_le_bytes());
        assert_eq!(e[28..32], 640i32.to_le_bytes());
        assert_eq!(e[40..44], [b'H', 0, b'i', 0]);
        let mut out = Vec::new();
        program_entry(&mut out, "Hello window", "C:\\gui_test.exe").unwrap();
        assert_eq!(out.len(), PROGRAM_ENTRY);
        assert_eq!(out[0..4], 12u32.to_le_bytes());
    }

    #[test]
    fn an_install_takes_one_package_id_and_nothing_more() {
        for id in ["7zip.7zip", "Microsoft.VisualStudioCode", "Python.Python.3.12", "a-b_c+d"] {
            assert!(package_id(id), "{id}");
        }
        let long = "a".repeat(MAX_ID + 1);
        for id in ["", "7zip 7zip", "\"7zip\"", "x;y", "--force", "a/b", long.as_str()] {
            // "--force" is letters and dashes: one word, which gala reads
            // as a (bad) id, never as more than one argument.
            assert_eq!(package_id(id), id == "--force", "{id}");
        }
    }

    #[test]
    fn a_linux_request_needs_a_distributions_name_and_a_known_operation() {
        use desktop::linux::{Request, SUGGESTIONS};
        let name = desktop::linux::Name::new(b"Debian").unwrap();
        assert_eq!(linux_request(Some("Debian"), LINUX_TERMINAL, 0), Ok(Request::Terminal(name)));
        assert_eq!(linux_request(Some("Debian"), LINUX_DESKTOP, 0), Ok(Request::Desktop(name)));
        assert_eq!(linux_request(Some("Debian"), LINUX_INSTALL, 0), Ok(Request::Install(name, 0)));
        let past = SUGGESTIONS.len() as i32;
        for (text, op, index) in [
            (Some("Debian"), LINUX_INSTALL, past),
            (Some("Debian"), LINUX_INSTALL, -1),
            (Some("Debian"), 9, 0),
            (Some("../etc"), LINUX_TERMINAL, 0),
            (Some(""), LINUX_TERMINAL, 0),
            (None, LINUX_TERMINAL, 0),
        ] {
            assert_eq!(
                linux_request(text, op, index),
                Err(NtStatus::INVALID_PARAMETER),
                "{text:?} {op} {index}"
            );
        }
    }
}
