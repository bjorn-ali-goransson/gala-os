//! GalaDesktopAction: the shell's named actions behind one system call,
//! for the `gala` command (user/gala) as for shasha's own mouse and keys
//! (ROADMAP SD "CLI parity", ARCHITECTURE "Server and desktop
//! workloads"). Starting a program works on every kernel; the window
//! actions are shasha's (`nt::desktop::Action`) and fail with
//! STATUS_NOT_SUPPORTED on the server kernel and with `profile=server`.
//!
//! The request (little-endian, version 1):
//!
//! | bytes  | field |
//! |--------|-------|
//! | 0..4   | version (1) |
//! | 4..8   | action (ACTION_*) |
//! | 8..16  | the window's HWND (the registry's) |
//! | 16..32 | four i32 arguments: Snap's side (0 left, 1 right, 2 up, 3 down); Move's x, y, width, height |
//! | 32..36 | the text's length in UTF-16 units (at most MAX_TEXT) |
//! | 36..40 | zero |
//! | 40..   | the text (UTF-16): Start's program and arguments, Wallpaper's name |
//!
//! The output: Start writes the new process id (u64). Windows and
//! Programs write a header (u32 version 1, u32 how many there are), then
//! as many entries as fit (WINDOW_ENTRY / PROGRAM_ENTRY bytes each).

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
pub const ACTION_PROGRAMS: u32 = 11;

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

/// Runs request `r` for `caller`; what it writes to the caller. A window
/// action needs the caller to pass UIPI against the window's process (an
/// equal or higher integrity level: STATUS_ACCESS_DENIED otherwise);
/// `level_of` gives a process's level (None: it has ended).
pub fn run(
    r: &Request,
    caller: &Caller,
    level_of: impl Fn(u32) -> Option<u32>,
) -> Result<Vec<u8>, NtStatus> {
    let mut out = Vec::new();
    if let Some(action) = window_action(r)? {
        // Without shasha nothing is on the screen to act on (checked
        // before the window, so a server always says so).
        let shell = &caller.desk.shell;
        if desktop::screen(shell).is_none() {
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
        desktop::window_action(shell, u64::from(info.pid), info.desktop, action)?;
        return Ok(out);
    }
    match r.action {
        ACTION_START => {
            let pid = start(&r.text, caller.token.clone(), &caller.desk.shell)?;
            out.try_extend_from_slice(&pid.to_le_bytes()).map_err(|_| NtStatus::NO_MEMORY)?;
        }
        ACTION_WINDOWS => {
            let listed = desktop::list_windows(&caller.desk.shell)?;
            list_header(&mut out, listed.len())?;
            for w in &listed {
                let hwnd = caller.desk.windows.hwnd_of_desktop(w.owner as u32, w.id).unwrap_or(0);
                window_entry(&mut out, hwnd, w)?;
            }
        }
        ACTION_PROGRAMS => {
            if desktop::screen(&caller.desk.shell).is_none() {
                return Err(NtStatus::NOT_SUPPORTED);
            }
            let programs = desktop::start_menu();
            list_header(&mut out, programs.len())?;
            for p in &programs {
                program_entry(&mut out, p.name, p.path)?;
            }
        }
        ACTION_WALLPAPER => {
            let mut name = [0u8; 32];
            let name = ascii(&r.text, &mut name).ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
            desktop::set_wallpaper(&caller.desk.shell, name)?;
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
/// `start_path`); its process id. STATUS_OBJECT_NAME_NOT_FOUND when there
/// is no such program.
pub fn start(
    text: &[u16],
    token: Option<super::object::ObjectRef>,
    shell: &desktop::Shell,
) -> Result<u64, NtStatus> {
    let mut buf = [0u8; MAX_TEXT];
    let text = ascii(text, &mut buf).ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
    let (program, args) = split_program(text);
    if program.is_empty() {
        return Err(NtStatus::OBJECT_NAME_NOT_FOUND);
    }
    let path = resolve(program, shell)?;
    start_path(path, args, token)
}

/// Starts the program at `path` (shasha's start menu: `token` None, the
/// boot user; `gala start`: a copy of the caller's primary token); its
/// process id, logged.
pub fn start_path(
    path: &'static str,
    args: &str,
    token: Option<super::object::ObjectRef>,
) -> Result<u64, NtStatus> {
    match super::process::launch(path, args, token) {
        Ok(pid) => {
            crate::info!("desktop: started {path} (pid {pid})");
            Ok(pid)
        }
        Err(e) => {
            crate::warn!("desktop: cannot start {path}: {e:?}");
            Err(e)
        }
    }
}

/// A desktop's start menu asked for `program` (the mouse or keys on the
/// console, or injected input): started as the boot user, whose session
/// is the console's at boot. Another session's start menu starts nothing
/// yet: a session has no user of its own until B4's session manager
/// (ROADMAP RS), so a program started for it would land in session 1.
pub fn start_from_menu(session: u32, program: &desktop::Program) {
    if session != super::session::CONSOLE {
        crate::warn!(
            "desktop: session {session}'s start menu asked for {}: not started (a session other than {} has no user to start it as yet)",
            program.path,
            super::session::CONSOLE
        );
        super::ledger::record("start menu", "session without a user", session.into());
        return;
    }
    // The same path as GalaDesktopAction's Start (logged there).
    let _ = start_path(program.path, "", None);
}

/// The path of the program `name` names (see `start`; the start menu's
/// names when `shell`'s desktop is on the screen).
fn resolve(name: &str, shell: &desktop::Shell) -> Result<&'static str, NtStatus> {
    let mut path = [0u8; MAX_TEXT + 32];
    let has_drive = name.len() > 2 && name.as_bytes()[1] == b':';
    if !has_drive {
        // A start menu name.
        if desktop::screen(shell).is_some()
            && let Some(p) =
                desktop::start_menu().iter().find(|p| p.name.eq_ignore_ascii_case(name))
        {
            return Ok(p.path);
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
}
