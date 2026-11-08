//! Window stations and desktops (ROADMAP RS step B3a; ARCHITECTURE "Window
//! stations and desktops"): the kernel objects behind user32's
//! GetProcessWindowStation, OpenWindowStation, GetThreadDesktop,
//! OpenDesktop, OpenInputDesktop and GetUserObjectInformation.
//!
//! Every session has the interactive window station `WinSta0` with its
//! desktop `Default`, named in the session's window stations directory
//! (`\Sessions\<n>\Windows\WindowStations\WinSta0`, session 0's under
//! `\Windows\WindowStations`). A process in a non-interactive logon
//! session (SYSTEM's services, a network logon) gets a station named after
//! its logon session, `Service-0x<high>-<low>$` (SYSTEM's is
//! `Service-0x0-3e7$`, made with session 0), made on first use with its
//! own `Default` desktop, as Microsoft documents ("Process Connection to
//! a Window Station"). Desktops are not in the object namespace: a
//! station holds them, and they are opened by name relative to the
//! caller's station (as Windows' window station parses them).
//!
//! A process's connection ([`Connection`]): what it asked for when it was
//! created (STARTUPINFO.lpDesktop, else its parent's station and desktop,
//! else an inherited window station handle, else its logon session's
//! default), resolved on its first window station or desktop call
//! (Windows connects at the first USER32 call), and the handles user32
//! hands out for it: the process's station handle and each thread's
//! desktop handle, which cannot be closed (NtClose fails with
//! STATUS_HANDLE_NOT_CLOSABLE, CloseDesktop with STATUS_DEVICE_BUSY) while
//! they are in use.
//!
//! All desktops of a session share its window state (`session::Desk`);
//! several desktops with windows of their own (CreateDesktop,
//! SwitchDesktop) are later work.

use alloc::vec::Vec;

use super::object::{
    GenericMapping, KObject, ObjectBody, ObjectRef, READ_CONTROL, STANDARD_RIGHTS_REQUIRED,
};
use super::status::NtStatus;
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;

/// The kernel log (host unit tests have no console: nothing).
macro_rules! log {
    ($level:ident, $($arg:tt)*) => {
        #[cfg(not(test))]
        $level!($($arg)*);
    };
}

// Window station access rights (winuser.h).
pub const WINSTA_ENUMDESKTOPS: u32 = 0x0001;
pub const WINSTA_READATTRIBUTES: u32 = 0x0002;
pub const WINSTA_ACCESSCLIPBOARD: u32 = 0x0004;
pub const WINSTA_CREATEDESKTOP: u32 = 0x0008;
pub const WINSTA_WRITEATTRIBUTES: u32 = 0x0010;
pub const WINSTA_ACCESSGLOBALATOMS: u32 = 0x0020;
pub const WINSTA_EXITWINDOWS: u32 = 0x0040;
pub const WINSTA_ENUMERATE: u32 = 0x0100;
pub const WINSTA_READSCREEN: u32 = 0x0200;
pub const WINSTA_ALL_ACCESS: u32 = 0x037f;

/// "Window Station Security and Access Rights".
pub const WINSTA_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL
        | WINSTA_ENUMDESKTOPS
        | WINSTA_ENUMERATE
        | WINSTA_READATTRIBUTES
        | WINSTA_READSCREEN,
    write: READ_CONTROL | WINSTA_ACCESSCLIPBOARD | WINSTA_CREATEDESKTOP | WINSTA_WRITEATTRIBUTES,
    execute: READ_CONTROL | WINSTA_ACCESSGLOBALATOMS | WINSTA_EXITWINDOWS,
    all: STANDARD_RIGHTS_REQUIRED | WINSTA_ALL_ACCESS,
};

// Desktop access rights (winuser.h).
pub const DESKTOP_READOBJECTS: u32 = 0x0001;
pub const DESKTOP_CREATEWINDOW: u32 = 0x0002;
pub const DESKTOP_CREATEMENU: u32 = 0x0004;
pub const DESKTOP_HOOKCONTROL: u32 = 0x0008;
pub const DESKTOP_JOURNALRECORD: u32 = 0x0010;
pub const DESKTOP_JOURNALPLAYBACK: u32 = 0x0020;
pub const DESKTOP_ENUMERATE: u32 = 0x0040;
pub const DESKTOP_WRITEOBJECTS: u32 = 0x0080;
pub const DESKTOP_SWITCHDESKTOP: u32 = 0x0100;
pub const DESKTOP_ALL_ACCESS: u32 = 0x01ff;

/// "Desktop Security and Access Rights".
pub const DESKTOP_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL | DESKTOP_ENUMERATE | DESKTOP_READOBJECTS,
    write: READ_CONTROL
        | DESKTOP_CREATEMENU
        | DESKTOP_CREATEWINDOW
        | DESKTOP_HOOKCONTROL
        | DESKTOP_JOURNALPLAYBACK
        | DESKTOP_JOURNALRECORD
        | DESKTOP_WRITEOBJECTS,
    execute: READ_CONTROL | DESKTOP_SWITCHDESKTOP,
    all: STANDARD_RIGHTS_REQUIRED | DESKTOP_ALL_ACCESS,
};

/// The interactive window station of every session.
pub const INTERACTIVE: &str = "WinSta0";
/// The desktop every window station has.
pub const DEFAULT_DESKTOP: &str = "Default";

// GetUserObjectInformation's indexes.
pub const UOI_FLAGS: u32 = 1;
pub const UOI_NAME: u32 = 2;
pub const UOI_TYPE: u32 = 3;
pub const UOI_USER_SID: u32 = 4;
pub const UOI_HEAPSIZE: u32 = 5;
pub const UOI_IO: u32 = 6;
/// USEROBJECTFLAGS.dwFlags of a window station with visible display
/// surfaces.
pub const WSF_VISIBLE: u32 = 1;

/// A window station: a session's, holding its desktops.
pub struct WindowStation {
    pub name: Vec<u16>,
    pub session: u32,
    /// WinSta0 (it can receive input), not a logon session's station.
    pub interactive: bool,
    desktops: IrqMutex<Vec<ObjectRef>>,
}

impl WindowStation {
    /// USEROBJECTFLAGS' WSF_VISIBLE: the interactive station of a user
    /// session. Session 0's WinSta0 is never displayed (Windows' session
    /// 0 isolation) and gala does not mark it.
    pub fn visible(&self) -> bool {
        self.interactive && self.session != super::session::SERVICES
    }

    /// Its desktop named `name` (case-insensitive).
    pub fn desktop(&self, name: &[u16]) -> Option<ObjectRef> {
        self.desktops
            .lock()
            .iter()
            .find(|d| desktop_of(d).is_some_and(|d| same(&d.name, name)))
            .cloned()
    }

    /// Its desktops' names, in the order they were made.
    pub fn desktop_names(&self) -> Result<Vec<Vec<u16>>, NtStatus> {
        let desktops = self.desktops.lock();
        let mut out = Vec::new();
        for d in desktops.iter().filter_map(|d| desktop_of(d)) {
            out.try_push(owned(&d.name)?).map_err(|_| NtStatus::NO_MEMORY)?;
        }
        Ok(out)
    }
}

/// A desktop of a window station.
pub struct Desktop {
    pub name: Vec<u16>,
    /// Its window station's object.
    pub station: ObjectRef,
}

impl Desktop {
    /// UOI_IO: the desktop that receives the user's input (each
    /// interactive station's Default; SwitchDesktop does not exist yet).
    pub fn receives_input(&self) -> bool {
        station_of(&self.station).is_some_and(|s| s.interactive)
            && same(&self.name, &w(DEFAULT_DESKTOP))
    }
}

pub fn station_of(o: &ObjectRef) -> Option<&WindowStation> {
    match &o.body {
        ObjectBody::WindowStation(s) => Some(s),
        _ => None,
    }
}

pub fn desktop_of(o: &ObjectRef) -> Option<&Desktop> {
    match &o.body {
        ObjectBody::Desktop(d) => Some(d),
        _ => None,
    }
}

/// An ASCII name as UTF-16 (fixed names only: no allocation).
fn w(s: &str) -> [u16; 16] {
    let mut out = [0u16; 16];
    for (d, b) in out.iter_mut().zip(s.bytes()) {
        *d = u16::from(b);
    }
    out
}

/// Names compare case-insensitively (ASCII), the trailing NULs of `w`'s
/// arrays ignored.
fn same(a: &[u16], b: &[u16]) -> bool {
    let trim = |s: &[u16]| s.iter().position(|&c| c == 0).map_or(s.len(), |n| n);
    let (a, b) = (&a[..trim(a)], &b[..trim(b)]);
    let fold = |c: u16| if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c };
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| fold(x) == fold(y))
}

fn owned(s: &[u16]) -> Result<Vec<u16>, NtStatus> {
    let mut v = Vec::new();
    v.try_extend_from_slice(s).map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(v)
}

fn push_str(out: &mut Vec<u16>, s: &str) -> Result<(), NtStatus> {
    for b in s.bytes() {
        out.try_push(u16::from(b)).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(())
}

/// `n` in lower-case hexadecimal, no leading zeros.
fn push_hex(out: &mut Vec<u16>, n: u32) -> Result<(), NtStatus> {
    let mut started = false;
    for i in (0..8).rev() {
        let d = (n >> (4 * i)) & 0xf;
        if d != 0 || started || i == 0 {
            started = true;
            let c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
            out.try_push(u16::from(c)).map_err(|_| NtStatus::NO_MEMORY)?;
        }
    }
    Ok(())
}

/// The window station name of a non-interactive logon session:
/// `Service-0x<high>-<low>$` (its LUID's halves in hexadecimal).
pub fn logon_station_name(luid: u64) -> Result<Vec<u16>, NtStatus> {
    let mut name = Vec::new();
    push_str(&mut name, "Service-0x")?;
    push_hex(&mut name, (luid >> 32) as u32)?;
    push_str(&mut name, "-")?;
    push_hex(&mut name, luid as u32)?;
    push_str(&mut name, "$")?;
    Ok(name)
}

/// An lpDesktop-style name split at its backslash: (`station`,
/// `desktop`) for `station\desktop`, (None, `desktop`) for a desktop
/// alone, (None, None) for an empty name.
pub fn split_spec(spec: &[u16]) -> (Option<&[u16]>, Option<&[u16]>) {
    let spec = &spec[..spec.iter().position(|&c| c == 0).unwrap_or(spec.len())];
    if spec.is_empty() {
        return (None, None);
    }
    match spec.iter().position(|&c| c == u16::from(b'\\')) {
        Some(i) => (Some(&spec[..i]), Some(&spec[i + 1..]).filter(|d| !d.is_empty())),
        None => (None, Some(spec)),
    }
}

/// A new window station object with its desktop Default (permanent:
/// they live as long as their session).
fn new_station(name: &[u16], session: u32, interactive: bool) -> Result<ObjectRef, NtStatus> {
    let nomem = |_| NtStatus::NO_MEMORY;
    let station = KObject::with_permanence(
        ObjectBody::WindowStation(WindowStation {
            name: owned(name)?,
            session,
            interactive,
            desktops: IrqMutex::new(Vec::new()),
        }),
        true,
    )
    .map_err(nomem)?;
    let mut desktop_name = Vec::new();
    push_str(&mut desktop_name, DEFAULT_DESKTOP)?;
    let desktop = KObject::with_permanence(
        ObjectBody::Desktop(Desktop { name: desktop_name, station: station.clone() }),
        true,
    )
    .map_err(nomem)?;
    if let Some(s) = station_of(&station) {
        s.desktops.lock().try_push(desktop).map_err(nomem)?;
    }
    Ok(station)
}

/// Session `session`'s window stations directory followed by `\name`.
fn station_path(session: u32, name: &[u16]) -> Result<Vec<u16>, NtStatus> {
    let mut path = super::session::window_stations_path(session)?;
    push_str(&mut path, "\\")?;
    path.try_extend_from_slice(name).map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(path)
}

/// Makes (or finds) window station `name` in `session`'s directory, with
/// its desktop Default; whether it was made.
fn make_station(
    session: u32,
    name: &[u16],
    interactive: bool,
) -> Result<(ObjectRef, bool), NtStatus> {
    let station = new_station(name, session, interactive)?;
    let path = station_path(session, name)?;
    let (found, existed) = super::namespace::create(None, &path, station, true, true)?;
    if station_of(&found).is_none() {
        return Err(NtStatus::OBJECT_TYPE_MISMATCH);
    }
    Ok((found, !existed))
}

/// A session's window stations, made with it (`session::ensure`): WinSta0
/// and, in session 0, SYSTEM's `Service-0x0-3e7$` (the services').
pub fn make_session(session: u32) -> Result<(), NtStatus> {
    make_station(session, &w(INTERACTIVE)[..INTERACTIVE.len()], true)?;
    if session == super::session::SERVICES {
        let name = logon_station_name(super::token::SYSTEM_LOGON)?;
        make_station(session, &name, false)?;
    }
    Ok(())
}

/// Window station `name` of `session` (OpenWindowStation):
/// STATUS_OBJECT_NAME_NOT_FOUND for an unknown or empty name,
/// STATUS_OBJECT_PATH_NOT_FOUND for a name with a backslash (as Windows
/// answers).
pub fn station_by_name(session: u32, name: &[u16]) -> Result<ObjectRef, NtStatus> {
    if name.is_empty() {
        return Err(NtStatus::OBJECT_NAME_NOT_FOUND);
    }
    if name.contains(&u16::from(b'\\')) {
        return Err(NtStatus::OBJECT_PATH_NOT_FOUND);
    }
    let path = station_path(session, name)?;
    let found = super::namespace::lookup(None, &path, true)?;
    if station_of(&found).is_none() {
        return Err(NtStatus::OBJECT_NAME_NOT_FOUND);
    }
    Ok(found)
}

/// The window station OpenWindowStation(`name`) opens for a process with
/// `token` in `session`: the empty name is the process's logon session's
/// own station, `Service-0x<high>-<low>$`, if it exists (it is not made:
/// an interactive logon whose processes are all on WinSta0 has none, and
/// the name is then STATUS_OBJECT_NAME_NOT_FOUND, as on a Windows Server
/// 2025 CI runner; a Windows 11 desktop's logon had one); a name starting
/// with a backslash is STATUS_OBJECT_PATH_SYNTAX_BAD; otherwise
/// [`station_by_name`].
pub fn station_to_open(
    session: u32,
    token: &ObjectRef,
    name: &[u16],
) -> Result<ObjectRef, NtStatus> {
    if name.is_empty() {
        let own = logon_station_name(logon_of(token).1)?;
        return station_by_name(session, &own);
    }
    if name[0] == u16::from(b'\\') {
        return Err(NtStatus::OBJECT_PATH_SYNTAX_BAD);
    }
    station_by_name(session, name)
}

/// The window station of logon session `luid` in `session`, made with
/// its desktop Default if it does not exist yet ("Process Connection to
/// a Window Station": a non-interactive logon's station).
pub fn logon_station(session: u32, luid: u64) -> Result<ObjectRef, NtStatus> {
    let name = logon_station_name(luid)?;
    let (station, made) = make_station(session, &name, false)?;
    if made {
        log!(info, "winsta: made window station {} in session {session}", Plain(&name));
        crate::events::emit(
            crate::events::Category::Window,
            "winsta",
            format_args!(
                r#""op":"made","session":{session},"station":{}"#,
                crate::events::Utf16(&name)
            ),
        );
    }
    Ok(station)
}

/// The names of `session`'s window stations (EnumWindowStations).
pub fn station_names(session: u32) -> Result<Vec<Vec<u16>>, NtStatus> {
    let dir =
        super::namespace::lookup(None, &super::session::window_stations_path(session)?, true)?;
    let mut out = Vec::new();
    for (name, kind) in super::namespace::directory_entries(&dir, 0)? {
        if kind == "WindowStation" {
            out.try_push(name).map_err(|_| NtStatus::NO_MEMORY)?;
        }
    }
    Ok(out)
}

/// A UTF-16 name as text (for the log).
#[cfg_attr(test, allow(dead_code))]
pub struct Plain<'a>(pub &'a [u16]);

impl core::fmt::Display for Plain<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for c in char::decode_utf16(self.0.iter().copied()) {
            core::fmt::Write::write_char(f, c.unwrap_or(char::REPLACEMENT_CHARACTER))?;
        }
        Ok(())
    }
}

/// How a process's window station was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Its logon session's default (a process the kernel started, or one
    /// whose creator named nothing and has no station itself).
    Logon,
    /// STARTUPINFO.lpDesktop.
    StartupInfo,
    /// Its parent's (pid), as CreateProcess passes it on.
    Parent(u64),
    /// A window station handle it inherited.
    Handle,
    /// SetProcessWindowStation.
    Set,
}

impl core::fmt::Display for Origin {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Origin::Logon => f.write_str("its logon session's"),
            Origin::StartupInfo => f.write_str("STARTUPINFO.lpDesktop"),
            Origin::Parent(pid) => write!(f, "its parent's, pid {pid}"),
            Origin::Handle => f.write_str("an inherited window station handle"),
            Origin::Set => f.write_str("SetProcessWindowStation"),
        }
    }
}

impl Origin {
    /// Its name in the event stream.
    pub fn tag(&self) -> &'static str {
        match self {
            Origin::Logon => "logon",
            Origin::StartupInfo => "startupinfo",
            Origin::Parent(_) => "parent",
            Origin::Handle => "handle",
            Origin::Set => "set",
        }
    }
}

/// A process's window station and desktops (see the module comment).
pub struct Connection(IrqMutex<State>);

struct State {
    origin: Origin,
    /// What it asked for: `station\desktop`, `desktop`, or empty.
    spec: Vec<u16>,
    /// An inherited window station handle's station.
    inherited: Option<ObjectRef>,
    /// Its window station and startup desktop, once resolved (or why
    /// they could not be).
    resolved: Option<Result<(ObjectRef, ObjectRef), NtStatus>>,
    /// The process's window station handle (0: not opened yet).
    station_handle: u64,
    /// The startup desktop's handle, every thread's unless set.
    desktop_handle: u64,
    /// (thread id, desktop handle, desktop) set with SetThreadDesktop.
    threads: Vec<(u64, u64, ObjectRef)>,
}

impl Default for Connection {
    fn default() -> Self {
        Self::new()
    }
}

impl Connection {
    pub fn new() -> Connection {
        Connection(IrqMutex::named(
            "process.winsta",
            State {
                origin: Origin::Logon,
                spec: Vec::new(),
                inherited: None,
                resolved: None,
                station_handle: 0,
                desktop_handle: 0,
                threads: Vec::new(),
            },
        ))
    }

    /// What its creator chose (before the process runs): a name, a
    /// parent's names, or an inherited station.
    pub fn request(&self, origin: Origin, spec: Vec<u16>, inherited: Option<ObjectRef>) {
        let mut s = self.0.lock();
        s.origin = origin;
        s.spec = spec;
        s.inherited = inherited;
    }

    /// Whether `handle` is one the process's connection uses (the
    /// station's, the startup desktop's or a thread's): NtClose refuses
    /// it.
    pub fn holds(&self, handle: u64) -> bool {
        let s = self.0.lock();
        handle != 0
            && (s.station_handle == handle
                || s.desktop_handle == handle
                || s.threads.iter().any(|t| t.1 == handle))
    }

    /// Whether `handle` is a thread's desktop handle (CloseDesktop:
    /// STATUS_DEVICE_BUSY).
    pub fn desktop_in_use(&self, handle: u64) -> bool {
        let s = self.0.lock();
        handle != 0 && (s.desktop_handle == handle || s.threads.iter().any(|t| t.1 == handle))
    }

    /// The station and startup desktop, if resolved.
    pub fn resolved(&self) -> Option<Result<(ObjectRef, ObjectRef), NtStatus>> {
        self.0.lock().resolved.clone()
    }

    /// Records the resolution (the first one wins: whether it is this
    /// one).
    pub fn set_resolved(
        &self,
        r: Result<(ObjectRef, ObjectRef), NtStatus>,
    ) -> (Result<(ObjectRef, ObjectRef), NtStatus>, bool) {
        let mut s = self.0.lock();
        let first = s.resolved.is_none();
        (s.resolved.get_or_insert(r).clone(), first)
    }

    /// (origin, spec copy, inherited station) for resolving.
    pub fn request_of(&self) -> Result<(Origin, Vec<u16>, Option<ObjectRef>), NtStatus> {
        let s = self.0.lock();
        Ok((s.origin, owned(&s.spec)?, s.inherited.clone()))
    }

    pub fn station_handle(&self) -> u64 {
        self.0.lock().station_handle
    }

    /// Records the station handle opened for it (unless another thread
    /// did meanwhile: then that one, and `opened` is to be closed).
    pub fn set_station_handle(&self, opened: u64) -> u64 {
        let mut s = self.0.lock();
        if s.station_handle == 0 {
            s.station_handle = opened;
        }
        s.station_handle
    }

    /// SetProcessWindowStation: the station and its handle.
    pub fn set_station(&self, handle: u64, station: ObjectRef) {
        let mut s = self.0.lock();
        s.station_handle = handle;
        s.origin = Origin::Set;
        if let Some(Ok((st, _))) = s.resolved.as_mut() {
            *st = station;
        }
    }

    /// The desktop handle of thread `tid` (0: the startup desktop's, not
    /// opened yet).
    pub fn thread_desktop(&self, tid: u64) -> u64 {
        let s = self.0.lock();
        s.threads.iter().find(|t| t.0 == tid).map_or(s.desktop_handle, |t| t.1)
    }

    /// Records the startup desktop handle (as `set_station_handle`).
    pub fn set_desktop_handle(&self, opened: u64) -> u64 {
        let mut s = self.0.lock();
        if s.desktop_handle == 0 {
            s.desktop_handle = opened;
        }
        s.desktop_handle
    }

    /// SetThreadDesktop: thread `tid`'s desktop handle is `handle`
    /// (the startup desktop's: no entry).
    pub fn set_thread_desktop(
        &self,
        tid: u64,
        handle: u64,
        desktop: ObjectRef,
    ) -> Result<(), NtStatus> {
        let mut s = self.0.lock();
        s.threads.retain(|t| t.0 != tid);
        if handle != s.desktop_handle {
            s.threads.try_push((tid, handle, desktop)).map_err(|_| NtStatus::NO_MEMORY)?;
        }
        Ok(())
    }

    /// Thread `tid` ended: its desktop handle is free to close.
    pub fn thread_ended(&self, tid: u64) {
        self.0.lock().threads.retain(|t| t.0 != tid);
    }

    /// The origin, for the log.
    pub fn origin(&self) -> Origin {
        self.0.lock().origin
    }
}

/// Whether token object `token` is an interactive logon's (it holds the
/// INTERACTIVE group, S-1-5-4), and its logon session's LUID.
fn logon_of(token: &ObjectRef) -> (bool, u64) {
    match &token.body {
        ObjectBody::Token(t) => {
            let d = t.data.lock();
            let interactive = d.groups.iter().any(|g| g.sid == super::token::Sid::new(5, &[4]));
            (interactive, d.authentication_id)
        }
        _ => (true, 0),
    }
}

/// Chooses a process's window station and startup desktop ("Process
/// Connection to a Window Station", "Thread Connection to a Desktop"): an
/// inherited station handle's station, else the station `spec` names,
/// else WinSta0 for an interactive logon or the logon session's station
/// (made if need be); the desktop `spec` names, else Default. Names are
/// looked up in `session`.
pub fn choose(
    session: u32,
    token: &ObjectRef,
    spec: &[u16],
    inherited: Option<ObjectRef>,
) -> Result<(ObjectRef, ObjectRef), NtStatus> {
    let (station_name, desktop_name) = split_spec(spec);
    let station = match (inherited, station_name) {
        (Some(s), _) => s,
        (None, Some(name)) => station_by_name(session, name)?,
        (None, None) => match logon_of(token) {
            (true, _) => station_by_name(session, &w(INTERACTIVE)[..INTERACTIVE.len()])?,
            (false, luid) => logon_station(session, luid)?,
        },
    };
    let default = w(DEFAULT_DESKTOP);
    let desktop_name = desktop_name.unwrap_or(&default[..DEFAULT_DESKTOP.len()]);
    let desktop = station_of(&station)
        .and_then(|s| s.desktop(desktop_name))
        .ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
    Ok((station, desktop))
}

/// `station\desktop`, the names of a resolved connection (a child's
/// inherited lpDesktop).
pub fn spec_of(station: &ObjectRef, desktop: &ObjectRef) -> Result<Vec<u16>, NtStatus> {
    let mut spec = Vec::new();
    if let Some(s) = station_of(station) {
        spec.try_extend_from_slice(&s.name).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    push_str(&mut spec, "\\")?;
    if let Some(d) = desktop_of(desktop) {
        spec.try_extend_from_slice(&d.name).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mm::heap::KArc;

    fn wv(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn logon_station_names() {
        assert_eq!(logon_station_name(0x3e7).unwrap(), wv("Service-0x0-3e7$"));
        assert_eq!(logon_station_name(0x1_0002_a3b4).unwrap(), wv("Service-0x1-2a3b4$"));
    }

    #[test]
    fn specs() {
        let spec = wv("winsta0\\default");
        assert_eq!(split_spec(&spec), (Some(&spec[..7]), Some(&spec[8..])));
        let spec = wv("Default");
        assert_eq!(split_spec(&spec), (None, Some(&spec[..])));
        assert_eq!(split_spec(&[]), (None, None));
        let spec = wv("WinSta0\\");
        assert_eq!(split_spec(&spec), (Some(&spec[..7]), None));
    }

    #[test]
    fn sessions_have_their_stations() {
        crate::nt::namespace::test_init();
        crate::nt::session::ensure(5).unwrap();
        // WinSta0 with Default in every session, case-insensitively.
        let s5 = station_by_name(5, &wv("winsta0")).unwrap();
        let s1 = station_by_name(1, &wv("WinSta0")).unwrap();
        assert!(!KArc::ptr_eq(&s5, &s1));
        let st = station_of(&s5).unwrap();
        assert_eq!(st.name, wv("WinSta0"));
        assert!(st.interactive && st.visible() && st.session == 5);
        let d = st.desktop(&wv("DEFAULT")).unwrap();
        assert!(desktop_of(&d).unwrap().receives_input());
        assert!(KArc::ptr_eq(&desktop_of(&d).unwrap().station, &s5));
        assert_eq!(st.desktop_names().unwrap(), [wv("Default")]);
        // Session 0: WinSta0 (not visible) and SYSTEM's services' station.
        let s0 = station_of(&station_by_name(0, &wv("WinSta0")).unwrap()).unwrap().visible();
        assert!(!s0);
        let svc = station_by_name(0, &wv("Service-0x0-3e7$")).unwrap();
        let svc = station_of(&svc).unwrap();
        assert!(!svc.interactive && !svc.visible());
        let names = station_names(0).unwrap();
        assert!(names.contains(&wv("WinSta0")) && names.contains(&wv("Service-0x0-3e7$")));
        // Errors as Windows reports them.
        assert_eq!(station_by_name(1, &wv("nope")).err(), Some(NtStatus::OBJECT_NAME_NOT_FOUND));
        assert_eq!(station_by_name(1, &[]).err(), Some(NtStatus::OBJECT_NAME_NOT_FOUND));
        assert_eq!(station_by_name(1, &wv("a\\b")).err(), Some(NtStatus::OBJECT_PATH_NOT_FOUND));
        // A logon session's station, made once.
        let a = logon_station(5, 0x1234).unwrap();
        let b = logon_station(5, 0x1234).unwrap();
        assert!(KArc::ptr_eq(&a, &b));
        assert!(station_by_name(5, &wv("Service-0x0-1234$")).is_ok());
        assert!(station_of(&a).unwrap().desktop(&wv("Default")).is_some());
        assert!(
            !desktop_of(&station_of(&a).unwrap().desktop(&wv("Default")).unwrap())
                .unwrap()
                .receives_input()
        );
    }

    #[test]
    fn choosing() {
        crate::nt::namespace::test_init();
        let user = crate::nt::token::for_new_process(None).unwrap();
        // The boot user: an interactive logon, WinSta0\Default.
        let (s, d) = choose(1, &user, &[], None).unwrap();
        assert_eq!(station_of(&s).unwrap().name, wv("WinSta0"));
        assert_eq!(desktop_of(&d).unwrap().name, wv("Default"));
        assert_eq!(spec_of(&s, &d).unwrap(), wv("WinSta0\\Default"));
        // OpenWindowStation(""): the logon session's own station, not
        // WinSta0, and not visible, once it exists (never made by it).
        assert_eq!(station_to_open(1, &user, &[]).err(), Some(NtStatus::OBJECT_NAME_NOT_FOUND));
        logon_station(1, logon_of(&user).1).unwrap();
        let own = station_to_open(1, &user, &[]).unwrap();
        let own = station_of(&own).unwrap();
        assert_eq!(own.name, logon_station_name(logon_of(&user).1).unwrap());
        assert!(!own.interactive && !own.visible());
        assert_eq!(
            station_to_open(1, &user, &wv("\\")).err(),
            Some(NtStatus::OBJECT_PATH_SYNTAX_BAD)
        );
        assert_eq!(
            station_to_open(1, &user, &wv(".")).err(),
            Some(NtStatus::OBJECT_NAME_NOT_FOUND)
        );
        // Named.
        let (s2, _) = choose(1, &user, &wv("winsta0\\default"), None).unwrap();
        assert!(KArc::ptr_eq(&s, &s2));
        assert_eq!(
            choose(1, &user, &wv("nosuch\\Default"), None).err(),
            Some(NtStatus::OBJECT_NAME_NOT_FOUND)
        );
        assert_eq!(
            choose(1, &user, &wv("WinSta0\\nosuch"), None).err(),
            Some(NtStatus::OBJECT_NAME_NOT_FOUND)
        );
        // An inherited station wins over the name.
        let svc = station_by_name(0, &wv("Service-0x0-3e7$")).unwrap();
        let (s3, _) = choose(1, &user, &wv("WinSta0\\Default"), Some(svc.clone())).unwrap();
        assert!(KArc::ptr_eq(&s3, &svc));
        // The connection's handles.
        let c = Connection::new();
        assert!(!c.holds(0) && !c.holds(8));
        assert_eq!(c.set_station_handle(8), 8);
        assert_eq!(c.set_station_handle(12), 8);
        assert_eq!(c.set_desktop_handle(16), 16);
        assert!(c.holds(8) && c.holds(16) && c.desktop_in_use(16));
        c.set_thread_desktop(7, 20, d.clone()).unwrap();
        assert_eq!(c.thread_desktop(7), 20);
        assert_eq!(c.thread_desktop(9), 16);
        assert!(c.desktop_in_use(20));
        c.set_thread_desktop(7, 16, d).unwrap();
        assert!(!c.desktop_in_use(20) && c.thread_desktop(7) == 16);
        c.thread_ended(7);
    }
}
