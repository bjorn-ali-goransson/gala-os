//! Sessions (ROADMAP RS step B1, ARCHITECTURE "Plan: sessions and remote
//! shasha"): a session is an id and its named-object directory. Session 0
//! holds services and SYSTEM (sshd and its logons too, as on Windows);
//! session 1 is the local console's; 2 and up are for remote logons
//! (step B4).
//!
//! A process's session is its primary token's TokenSessionId when it is
//! created (nt::process), as Windows documents for CreateProcessAsUser.
//! Each session's BaseNamedObjects directory has the links Windows has:
//! `Global` (to `\BaseNamedObjects`), `Local` (to the session's own
//! directory) and `Session` (to `\Sessions\BNOLINKS`); session 0's
//! directory is `\BaseNamedObjects` itself, so there `Local\` and
//! `Global\` are the same. kernel32 names objects under the caller's
//! session directory (from PEB.SessionId), so `Local\x` resolves to the
//! session's and `Global\x` to the global one through these links.
//!
//! ```text
//! \BaseNamedObjects                 session 0's (and the global) objects
//!     Global -> \BaseNamedObjects   Local -> \BaseNamedObjects
//!     Session -> \Sessions\BNOLINKS
//! \Sessions\0                       (no BaseNamedObjects: they are global)
//! \Sessions\<n>\BaseNamedObjects    session n's objects
//!     Global -> \BaseNamedObjects   Local -> \Sessions\<n>\BaseNamedObjects
//!     Session -> \Sessions\BNOLINKS
//! \Sessions\BNOLINKS\<n>            -> session n's BaseNamedObjects
//! ```
//!
//! Sessions 0 and 1 exist from boot; a process created with a token of
//! another session makes that session's directories on first use (until
//! the session manager of step B4 creates sessions and refuses unknown
//! ones). Sessions are never removed yet.
//!
//! Step B2: each session has its window state ([`Desk`], what its
//! desktops share): the window registry, the
//! WinEvent hooks and shasha's desktop, each behind its own lock, reached
//! through the caller's process (`Process::desk`). Nothing about windows
//! crosses sessions. The window stations' directories exist as on
//! Windows (`\Windows\WindowStations` for session 0,
//! `\Sessions\<n>\Windows\WindowStations` for the others), each with
//! the session's window station objects (nt::winsta: WinSta0, and in
//! session 0 SYSTEM's `Service-0x0-3e7$`), whose desktops all share the
//! session's `Desk`.

use alloc::vec::Vec;

use super::object::ObjectRef;
use super::status::NtStatus;
use crate::mm::heap::{KArc, TryVec};
use crate::sync::IrqMutex;

/// Services and SYSTEM.
pub const SERVICES: u32 = 0;
/// The local console's session at boot (the boot user's): what the
/// console shows until a console switch (`active_console`).
pub const CONSOLE: u32 = 1;
/// No session is attached to the console (WTSGetActiveConsoleSessionId's
/// 0xFFFFFFFF: while a console switch is under way).
pub const NO_CONSOLE: u32 = u32::MAX;
/// How many sessions may exist at once (each holds directories; a token's
/// session id is any u32 that a SeTcbPrivilege holder sets).
pub const MAX_SESSIONS: usize = 64;

/// A session's record.
pub struct Session {
    pub id: u32,
    /// Its BaseNamedObjects directory.
    pub bno: ObjectRef,
    /// Its desktop.
    pub desk: KArc<Desk>,
}

/// A session's window state (what Windows keeps per desktop; shared by
/// every desktop object of the session, nt::winsta, until desktops with
/// windows of their own are wanted): what the session's GUI programs
/// share, and nothing another session sees.
pub struct Desk {
    pub session: u32,
    /// Every window of the session's processes (FindWindow, EnumWindows,
    /// messages between them, UIPI's filters).
    pub windows: super::window::Windows,
    /// SetWinEventHook's hooks and their waiting events.
    pub hooks: super::winevent::Hooks,
    /// The desktop's window state (nt::winstate: its windows and their
    /// pixels) and, in the desktop kernel, shasha's compositor and what
    /// shows it (ROADMAP RS B3a's presenters); the server kernel's has no
    /// compositor (desktop_off.rs).
    pub shell: super::desktop::Shell,
}

impl Desk {
    pub const fn new(session: u32) -> Desk {
        Desk {
            session,
            windows: super::window::Windows::new(),
            hooks: super::winevent::Hooks::new(),
            shell: super::desktop::Shell::new(session),
        }
    }
}

/// The sessions that exist (lock order: before the namespace's lock).
static SESSIONS: IrqMutex<Vec<Session>> = IrqMutex::new(Vec::new());

/// The session the console (the screen, the PS/2 keyboard and mouse)
/// shows: CONSOLE at boot, moved by a console switch
/// (`desktop::switch_console`), NO_CONSOLE while one is under way.
static ACTIVE_CONSOLE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(CONSOLE);

/// The session the console shows (KUSER_SHARED_DATA.ActiveConsoleId,
/// WTSGetActiveConsoleSessionId); NO_CONSOLE during a console switch.
pub fn active_console() -> u32 {
    ACTIVE_CONSOLE.load(core::sync::atomic::Ordering::Acquire)
}

/// The console shows session `id` from now on (NO_CONSOLE: none, during
/// a switch): the local input goes there and KUSER_SHARED_DATA says so.
pub fn set_active_console(id: u32) {
    ACTIVE_CONSOLE.store(id, core::sync::atomic::Ordering::Release);
    super::kuser::set_active_console(id);
}

/// Appends `id` in decimal to `out`.
fn push_number(out: &mut Vec<u16>, id: u32) -> Result<(), NtStatus> {
    let mut digits = [0u16; 10];
    let (mut n, mut v) = (0, id);
    loop {
        digits[n] = u16::from(b'0') + (v % 10) as u16;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    for d in digits[..n].iter().rev() {
        out.try_push(*d).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(())
}

fn push_str(out: &mut Vec<u16>, s: &str) -> Result<(), NtStatus> {
    out.try_reserve(s.len()).map_err(|_| NtStatus::NO_MEMORY)?;
    for b in s.bytes() {
        out.try_push(u16::from(b)).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(())
}

/// `\Sessions\<id>` followed by `tail`.
fn session_path(id: u32, tail: &str) -> Result<Vec<u16>, NtStatus> {
    let mut p = Vec::new();
    push_str(&mut p, "\\Sessions\\")?;
    push_number(&mut p, id)?;
    push_str(&mut p, tail)?;
    Ok(p)
}

/// The NT path of session `id`'s BaseNamedObjects directory:
/// `\BaseNamedObjects` for session 0, `\Sessions\<id>\BaseNamedObjects`
/// for the others.
pub fn bno_path(id: u32) -> Result<Vec<u16>, NtStatus> {
    if id == SERVICES {
        let mut p = Vec::new();
        push_str(&mut p, "\\BaseNamedObjects")?;
        Ok(p)
    } else {
        session_path(id, "\\BaseNamedObjects")
    }
}

/// `\Sessions\BNOLINKS\<id>`.
fn bnolink_path(id: u32) -> Result<Vec<u16>, NtStatus> {
    let mut p = Vec::new();
    push_str(&mut p, "\\Sessions\\BNOLINKS\\")?;
    push_number(&mut p, id)?;
    Ok(p)
}

/// Creates the permanent directory `path`, or finds it.
fn directory(path: &[u16]) -> Result<ObjectRef, NtStatus> {
    let dir = super::object::KObject::with_permanence(
        super::object::ObjectBody::Directory(super::object::Directory::default()),
        true,
    )
    .map_err(|_| NtStatus::NO_MEMORY)?;
    let (dir, _) = super::namespace::create(None, path, dir, true, true)?;
    if dir.as_directory().is_none() {
        return Err(NtStatus::OBJECT_TYPE_MISMATCH);
    }
    Ok(dir)
}

/// Creates the permanent symbolic link `path` -> `target` (an existing
/// name is left as it is).
fn link(path: &[u16], target: &[u16]) -> Result<(), NtStatus> {
    let l = super::namespace::new_symbolic_link(target)?;
    super::namespace::create(None, path, l, true, true).map(drop)
}

/// `dir`'s path followed by `\name`.
fn child(dir: &[u16], name: &str) -> Result<Vec<u16>, NtStatus> {
    let mut p = Vec::new();
    p.try_extend_from_slice(dir).map_err(|_| NtStatus::NO_MEMORY)?;
    push_str(&mut p, "\\")?;
    push_str(&mut p, name)?;
    Ok(p)
}

/// The NT path of session `id`'s window stations' directory:
/// `\Windows\WindowStations` for session 0,
/// `\Sessions\<id>\Windows\WindowStations` for the others.
pub fn window_stations_path(id: u32) -> Result<Vec<u16>, NtStatus> {
    let mut p = Vec::new();
    if id == SERVICES {
        push_str(&mut p, "\\Windows\\WindowStations")?;
        Ok(p)
    } else {
        session_path(id, "\\Windows\\WindowStations")
    }
}

/// Makes session `id`'s directories and links (see the module comment).
fn make(id: u32) -> Result<Session, NtStatus> {
    let bno_path = bno_path(id)?;
    directory(&session_path(id, "")?)?;
    let stations = window_stations_path(id)?;
    // Its parent (`...\Windows`) first.
    let parent = stations.len() - "\\WindowStations".len();
    directory(&stations[..parent])?;
    directory(&stations)?;
    super::winsta::make_session(id)?;
    let bno = directory(&bno_path)?;
    let mut global = Vec::new();
    push_str(&mut global, "\\BaseNamedObjects")?;
    let mut bnolinks = Vec::new();
    push_str(&mut bnolinks, "\\Sessions\\BNOLINKS")?;
    link(&child(&bno_path, "Global")?, &global)?;
    link(&child(&bno_path, "Local")?, &bno_path)?;
    link(&child(&bno_path, "Session")?, &bnolinks)?;
    link(&bnolink_path(id)?, &bno_path)?;
    let desk = KArc::try_new(Desk::new(id)).map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(Session { id, bno, desk })
}

/// The kernel log (host unit tests have no console: nothing).
macro_rules! log {
    ($level:ident, $($arg:tt)*) => {
        #[cfg(not(test))]
        $level!($($arg)*);
    };
}

/// Makes session `id` if it does not exist yet (its directories, its
/// BNOLINKS entry and its desktop); its desktop.
/// STATUS_INSUFFICIENT_RESOURCES past MAX_SESSIONS.
pub fn ensure(id: u32) -> Result<KArc<Desk>, NtStatus> {
    let mut sessions = SESSIONS.lock();
    if let Some(s) = sessions.iter().find(|s| s.id == id) {
        return Ok(s.desk.clone());
    }
    if sessions.len() >= MAX_SESSIONS {
        log!(warn, "session: cannot make session {id}: {MAX_SESSIONS} sessions exist");
        return Err(NtStatus::INSUFFICIENT_RESOURCES);
    }
    let session = match make(id) {
        Ok(s) => s,
        Err(e) => {
            log!(warn, "session: cannot make session {id}: {e:?}");
            return Err(e);
        }
    };
    let desk = session.desk.clone();
    sessions.try_push(session).map_err(|_| NtStatus::NO_MEMORY)?;
    drop(sessions);
    let path = bno_path(id)?;
    log!(info, "session: {id} made, named objects in {}", Plain(&path));
    crate::events::emit(
        crate::events::Category::Process,
        "session",
        format_args!(r#""id":{id},"bno":{}"#, crate::events::Utf16(&path)),
    );
    Ok(desk)
}

/// The console's session for the log: `session 2`, or `no session`
/// (during a switch).
pub struct ConsoleShows(pub u32);

impl core::fmt::Display for ConsoleShows {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            NO_CONSOLE => f.write_str("no session"),
            id => write!(f, "session {id}"),
        }
    }
}

/// A UTF-16 path as text (for the log).
#[cfg_attr(test, allow(dead_code))]
struct Plain<'a>(&'a [u16]);

impl core::fmt::Display for Plain<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for c in char::decode_utf16(self.0.iter().copied()) {
            core::fmt::Write::write_char(f, c.unwrap_or(char::REPLACEMENT_CHARACTER))?;
        }
        Ok(())
    }
}

/// Session `id`'s BaseNamedObjects directory, if the session exists.
pub fn bno(id: u32) -> Option<ObjectRef> {
    SESSIONS.lock().iter().find(|s| s.id == id).map(|s| s.bno.clone())
}

/// Session `id`'s desktop, if the session exists.
pub fn desk(id: u32) -> Option<KArc<Desk>> {
    SESSIONS.lock().iter().find(|s| s.id == id).map(|s| s.desk.clone())
}

/// The session other than `except` whose window registry holds `hwnd`,
/// and the window's process (an HWND asked for across sessions: the log
/// says where it is). Takes each registry's lock in turn, none held.
pub fn window_elsewhere(hwnd: u64, except: u32) -> Option<(u32, u32)> {
    let mut desks: [Option<KArc<Desk>>; MAX_SESSIONS] = [const { None }; MAX_SESSIONS];
    for (slot, s) in desks.iter_mut().zip(SESSIONS.lock().iter()) {
        *slot = Some(s.desk.clone());
    }
    desks
        .iter()
        .flatten()
        .filter(|d| d.session != except)
        .find_map(|d| d.windows.holds(hwnd).map(|pid| (d.session, pid)))
}

/// How many times a process asked for another session's window.
static CROSSED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Process `pid` of session `session` asked (`what`: post, send, query)
/// for window `hwnd`, which is not in its session: if another session
/// holds it, the log says so (the first 32 times) and the event stream
/// too (`"ev":"window","op":"other session"`), so an
/// ERROR_INVALID_WINDOW_HANDLE across sessions explains itself.
pub fn note_window_elsewhere(hwnd: u64, pid: u64, session: u32, what: &str) {
    let Some((there, owner)) = window_elsewhere(hwnd, session) else { return };
    let n = CROSSED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if n < 32 {
        log!(
            info,
            "window: pid {pid} (session {session}) {what} window {hwnd:#x} of pid {owner} in session {there}: not visible across sessions"
        );
    }
    crate::events::emit(
        crate::events::Category::Window,
        "window",
        format_args!(
            r#""op":"other session","what":{},"hwnd":{hwnd},"pid":{pid},"session":{session},"owner":{owner},"owner_session":{there}"#,
            crate::events::Str(what)
        ),
    );
}

/// The state dump's sessions part: per session its window registry's,
/// hooks' and shasha's figures and its top-level windows (at most 16).
#[cfg_attr(test, allow(unused_variables))]
pub fn dump() {
    let mut desks: [Option<KArc<Desk>>; MAX_SESSIONS] = [const { None }; MAX_SESSIONS];
    for (slot, s) in desks.iter_mut().zip(SESSIONS.lock().iter()) {
        *slot = Some(s.desk.clone());
    }
    log!(
        info,
        "statedump: sessions (the console shows {}; windows asked for across sessions: {}):",
        ConsoleShows(active_console()),
        CROSSED.load(core::sync::atomic::Ordering::Relaxed)
    );
    for d in desks.iter().flatten() {
        let c = d.windows.counts();
        let (hooks, events) = d.hooks.counts();
        let (on_desktop, live, presenter, bare) = d.shell.counts();
        log!(
            info,
            "statedump: session {}: {} windows ({} top-level), {} queues, {} posted, {} sent, {} filters, {} hooks ({} events waiting), desktop: {} ({} windows, presenter {}{}{})",
            d.session,
            c.windows,
            c.top_level,
            c.queues,
            c.posted,
            c.sent,
            c.filters,
            hooks,
            events,
            if live { "live" } else { "not live" },
            on_desktop,
            presenter.name(),
            // Nothing can show it: why (the server kernel, the server
            // profile, no screen and no `presenter=`).
            if bare.is_some() { "; never shown: " } else { "" },
            bare.unwrap_or(""),
        );
        dump_stations(d.session);
        let (recent, dropped, waiting) = d.shell.recent_input();
        log!(
            info,
            "statedump: session {} input queue: {} waiting, {} dropped (full)",
            d.session,
            waiting,
            dropped
        );
        let now = crate::nt::platform::monotonic_ns();
        for b in recent.iter().flatten() {
            log!(
                info,
                "statedump: session {} {} to window {}{} ({} ms ago), {}",
                d.session,
                InputWhat(b),
                b.window,
                if b.captured { " (its capture)" } else { "" },
                now.saturating_sub(b.t) / 1_000_000,
                InputFate(b),
            );
        }
        let mut top = [super::window::Info::default(); 16];
        let n = d.windows.top_level(&mut top);
        for w in &top[..n] {
            log!(
                info,
                "statedump: session {} window {:#x} pid {} tid {} class {} text {} rect {:?} flags {:#x}",
                d.session,
                w.hwnd,
                w.pid,
                w.tid,
                Plain(&w.class[..w.class_len as usize]),
                Plain(&w.text[..w.text_len as usize]),
                w.rect,
                w.flags
            );
        }
    }
}

/// The state dump's line about session `id`'s window stations.
#[cfg_attr(test, allow(unused_variables))]
fn dump_stations(id: u32) {
    let Ok(names) = super::winsta::station_names(id) else { return };
    let mut line = [0u8; 160];
    let mut n = 0;
    for name in &names {
        for c in char::decode_utf16(name.iter().copied()).chain(core::iter::once(Ok(' '))) {
            let b = match c {
                Ok(c) if c.is_ascii() => c as u8,
                _ => b'?',
            };
            if n < line.len() {
                line[n] = b;
                n += 1;
            }
        }
    }
    log!(
        info,
        "statedump: session {id} window stations: {}",
        core::str::from_utf8(&line[..n]).unwrap_or("?").trim_end()
    );
}

/// The ids of the sessions that exist, in the order they were made.
pub fn ids() -> Result<Vec<u32>, NtStatus> {
    let sessions = SESSIONS.lock();
    let mut out = Vec::new();
    out.try_reserve(sessions.len()).map_err(|_| NtStatus::NO_MEMORY)?;
    for s in sessions.iter() {
        out.try_push(s.id).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(out)
}

/// The sessions every boot has: 0 (services) and 1 (the console). Called
/// once the namespace's fixed directories exist.
pub fn init() -> Result<(), NtStatus> {
    SESSIONS.lock().clear();
    ensure(SERVICES)?;
    ensure(CONSOLE).map(drop)
}

/// A noted input as the state dump names it: a button with its screen
/// point, a key with its virtual key.
#[cfg_attr(test, allow(dead_code))] // test builds' log! drops its arguments
struct InputWhat<'a>(&'a crate::nt::winstate::InputNote);

impl core::fmt::Display for InputWhat<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let b = self.0;
        if (0x100..=0x105).contains(&b.message) {
            write!(f, "key {:#x} vk {:#x}", b.message, b.x)
        } else {
            write!(f, "button {:#x} at ({}, {})", b.message, b.x, b.y)
        }
    }
}

/// What became of a noted input: taken by its program (after how long),
/// dropped by the full queue, or still waiting.
#[cfg_attr(test, allow(dead_code))]
struct InputFate<'a>(&'a crate::nt::winstate::InputNote);

impl core::fmt::Display for InputFate<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let b = self.0;
        if b.dropped {
            f.write_str("dropped (queue full)")
        } else if b.taken == 0 {
            f.write_str("not taken by its program")
        } else {
            write!(f, "taken after {} ms", b.taken.saturating_sub(b.t) / 1_000_000)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mm::heap::KArc;
    use crate::nt::namespace::{create, lookup, lookup_link};
    use crate::nt::object::{KObject, ObjectBody};

    fn setup() {
        crate::nt::namespace::test_init();
    }

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn event() -> ObjectRef {
        KObject::new(ObjectBody::ConsoleOutput).unwrap()
    }

    fn target(path: &str) -> Vec<u16> {
        let l = lookup_link(None, &w(path), true).unwrap();
        l.as_symbolic_link().unwrap().to_vec()
    }

    #[test]
    fn paths() {
        assert_eq!(bno_path(0).unwrap(), w("\\BaseNamedObjects"));
        assert_eq!(bno_path(1).unwrap(), w("\\Sessions\\1\\BaseNamedObjects"));
        assert_eq!(bno_path(4_000_000_000).unwrap(), w("\\Sessions\\4000000000\\BaseNamedObjects"));
        assert_eq!(bnolink_path(12).unwrap(), w("\\Sessions\\BNOLINKS\\12"));
    }

    #[test]
    fn boot_sessions_and_links() {
        setup();
        assert!(bno(0).is_some() && bno(1).is_some());
        let global = lookup(None, &w("\\BaseNamedObjects"), true).unwrap();
        assert!(KArc::ptr_eq(&bno(0).unwrap(), &global));
        // Session 0 has no BaseNamedObjects under \Sessions\0.
        assert!(lookup(None, &w("\\Sessions\\0"), true).unwrap().as_directory().is_some());
        assert_eq!(
            lookup(None, &w("\\Sessions\\0\\BaseNamedObjects"), true).err(),
            Some(NtStatus::OBJECT_NAME_NOT_FOUND)
        );
        assert_eq!(target("\\Sessions\\BNOLINKS\\0"), w("\\BaseNamedObjects"));
        assert_eq!(target("\\Sessions\\BNOLINKS\\1"), w("\\Sessions\\1\\BaseNamedObjects"));
        assert_eq!(target("\\BaseNamedObjects\\Local"), w("\\BaseNamedObjects"));
        assert_eq!(target("\\BaseNamedObjects\\Session"), w("\\Sessions\\BNOLINKS"));
        assert_eq!(
            target("\\Sessions\\1\\BaseNamedObjects\\Local"),
            w("\\Sessions\\1\\BaseNamedObjects")
        );
    }

    /// What kernel32 asks for: `<session BNO>\Local\x` and `\Global\x`.
    #[test]
    fn local_and_global_resolve_per_session() {
        setup();
        let local1 = event();
        create(
            None,
            &w("\\Sessions\\1\\BaseNamedObjects\\Local\\st_local"),
            local1.clone(),
            true,
            false,
        )
        .unwrap();
        // In session 1's directory, not the global one.
        let found = lookup(None, &w("\\Sessions\\1\\BaseNamedObjects\\st_local"), true).unwrap();
        assert!(KArc::ptr_eq(&found, &local1));
        assert_eq!(
            lookup(None, &w("\\BaseNamedObjects\\Local\\st_local"), true).err(),
            Some(NtStatus::OBJECT_NAME_NOT_FOUND)
        );
        // Through BNOLINKS.
        let via = lookup(None, &w("\\Sessions\\BNOLINKS\\1\\st_local"), true).unwrap();
        assert!(KArc::ptr_eq(&via, &local1));
        // Global\ from session 1 is session 0's Local\.
        let global = event();
        create(
            None,
            &w("\\Sessions\\1\\BaseNamedObjects\\Global\\st_global"),
            global.clone(),
            true,
            false,
        )
        .unwrap();
        let from0 = lookup(None, &w("\\BaseNamedObjects\\Local\\st_global"), true).unwrap();
        assert!(KArc::ptr_eq(&from0, &global));
        assert_eq!(
            lookup(None, &w("\\Sessions\\1\\BaseNamedObjects\\st_global"), true).err(),
            Some(NtStatus::OBJECT_NAME_NOT_FOUND)
        );
    }

    #[test]
    fn sessions_made_on_demand() {
        setup();
        assert!(bno(7).is_none());
        ensure(7).unwrap();
        ensure(7).unwrap();
        let dir = bno(7).unwrap();
        let found = lookup(None, &w("\\Sessions\\7\\BaseNamedObjects"), true).unwrap();
        assert!(KArc::ptr_eq(&dir, &found));
        assert_eq!(target("\\Sessions\\BNOLINKS\\7"), w("\\Sessions\\7\\BaseNamedObjects"));
        assert_eq!(ids().unwrap().iter().filter(|&&i| i == 7).count(), 1);
        // Its desktop, the same each time; its window stations' directory.
        let desk = ensure(7).unwrap();
        assert!(KArc::ptr_eq(&desk, &super::desk(7).unwrap()));
        assert_eq!(desk.session, 7);
        assert!(
            lookup(None, &w("\\Sessions\\7\\Windows\\WindowStations"), true)
                .unwrap()
                .as_directory()
                .is_some()
        );
    }

    /// Step B2: a window registered in one session's registry is no
    /// window in another's; the HWNDs differ (one counter).
    #[test]
    fn windows_stay_in_their_session() {
        setup();
        let (a, b) = (ensure(11).unwrap(), ensure(12).unwrap());
        let info = crate::nt::window::Info::default();
        let ha = a.windows.register(1101, info).unwrap();
        let hb = b.windows.register(1201, info).unwrap();
        assert_ne!(ha, hb);
        assert!(a.windows.query(ha).is_some() && a.windows.query(hb).is_none());
        assert!(b.windows.query(hb).is_some() && b.windows.query(ha).is_none());
        let mut top = [0u64; 8];
        let n = a.windows.children(0, &mut top);
        assert!(top[..n.min(8)].contains(&ha) && !top[..n.min(8)].contains(&hb));
        let from = crate::nt::window::Sender { pid: 1101, level: 0x2000 };
        assert!(matches!(
            a.windows.post(from, 0x2000, hb, 0x400, 0, 0),
            Err(NtStatus::INVALID_HANDLE)
        ));
        b.windows.has_queue(1201, 5).unwrap();
        assert!(matches!(
            a.windows.post_thread(from, 0x2000, 5, 0x400, 0, 0),
            Err(NtStatus::INVALID_CID)
        ));
        // Where an HWND asked for across sessions is.
        assert_eq!(window_elsewhere(hb, 11), Some((12, 1201)));
        assert_eq!(window_elsewhere(hb, 12), None);
        // The window stations' directories, session 0's at \Windows.
        assert!(lookup(None, &w("\\Windows\\WindowStations"), true).is_ok());
        assert_eq!(window_stations_path(3).unwrap(), w("\\Sessions\\3\\Windows\\WindowStations"));
        a.windows.process_ended(1101);
        b.windows.process_ended(1201);
    }
}
