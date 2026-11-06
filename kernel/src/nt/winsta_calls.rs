//! gala's window station and desktop services over `nt::winsta`, for
//! gala's user32 (ARCHITECTURE "Window stations and desktops"): opening
//! by name, the process's and threads' connection handles, setting them,
//! closing, GetUserObjectInformation and the name lists.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use super::super::object::{Handle, HandleEntry, MAXIMUM_ALLOWED, ObjectBody, ObjectRef};
use super::super::status::NtStatus;
use super::super::thread;
use super::super::winsta::{
    self, DEFAULT_DESKTOP, Plain, UOI_FLAGS, UOI_IO, UOI_NAME, UOI_TYPE, WINSTA_ENUMDESKTOPS,
    WSF_VISIBLE, desktop_of, station_of,
};
use super::{Call, HANDLE_ATTRIBUTES};
use crate::mm::heap::TryVec;

/// Opens named failures logged so far (the first 32 are).
static NOT_FOUND: AtomicU32 = AtomicU32::new(0);

/// The longest name a caller may pass (characters).
const MAX_NAME: u64 = 256;

impl Call<'_> {
    /// The caller's window station and startup desktop, chosen now if
    /// this is its first window station or desktop call (logged, with
    /// where the choice came from, and an event).
    fn connection(&self) -> Result<(ObjectRef, ObjectRef), NtStatus> {
        let ui = &self.process.ui;
        if let Some(r) = ui.resolved() {
            return r;
        }
        let (origin, spec, inherited) = ui.request_of()?;
        let token = self.process.token.lock().clone();
        let chosen = winsta::choose(self.process.session, &token, &spec, inherited);
        let (r, first) = ui.set_resolved(chosen);
        if first {
            let (pid, name, session) = (self.process.id, self.process.name, self.process.session);
            match &r {
                Ok((s, d)) => {
                    let (s, d) = (station_of(s), desktop_of(d));
                    let (s, d) = (s.map_or(&[][..], |s| &s.name), d.map_or(&[][..], |d| &d.name));
                    info!(
                        "winsta: pid {pid} ({name}, session {session}) is on {}\\{} ({origin})",
                        Plain(s),
                        Plain(d)
                    );
                    crate::events::emit(
                        crate::events::Category::Window,
                        "winsta",
                        format_args!(
                            r#""op":"connect","pid":{pid},"session":{session},"station":{},"desktop":{},"from":"{}""#,
                            crate::events::Utf16(s),
                            crate::events::Utf16(d),
                            origin.tag()
                        ),
                    );
                }
                Err(e) => {
                    warn!(
                        "winsta: pid {pid} ({name}, session {session}) has no window station: \
                         \"{}\" ({origin}) gives {:#010x} (Windows fails user32's start-up here)",
                        Plain(&spec),
                        e.0
                    );
                    crate::events::emit(
                        crate::events::Category::Window,
                        "winsta",
                        format_args!(
                            r#""op":"connect failed","pid":{pid},"session":{session},"spec":{},"from":"{}","status":{}"#,
                            crate::events::Utf16(&spec),
                            origin.tag(),
                            e.0
                        ),
                    );
                }
            }
        }
        r
    }

    /// A new handle to `object` granting all access (the handles the
    /// connection opens itself, not inheritable).
    fn connection_handle(&self, object: ObjectRef) -> Result<u64, NtStatus> {
        let access = object.mapping().map(MAXIMUM_ALLOWED);
        let entry = HandleEntry { object, access, attributes: 0 };
        self.process.handles.lock().insert(entry).map(|h| h.0).map_err(|_| NtStatus::NO_MEMORY)
    }

    /// The process's window station handle, opened on first use.
    fn station_handle(&self) -> Result<u64, NtStatus> {
        let (station, _) = self.connection()?;
        match self.process.ui.station_handle() {
            0 => {
                let opened = self.connection_handle(station)?;
                let kept = self.process.ui.set_station_handle(opened);
                if kept != opened {
                    let entry = self.process.handles.lock().close(Handle(opened));
                    drop(entry);
                }
                Ok(kept)
            }
            h => Ok(h),
        }
    }

    /// The desktop handle of thread `tid` (the startup desktop's, opened
    /// on first use, unless the thread set its own).
    fn desktop_handle(&self, tid: u64) -> Result<u64, NtStatus> {
        let (_, desktop) = self.connection()?;
        match self.process.ui.thread_desktop(tid) {
            0 => {
                let opened = self.connection_handle(desktop)?;
                let kept = self.process.ui.set_desktop_handle(opened);
                if kept != opened {
                    let entry = self.process.handles.lock().close(Handle(opened));
                    drop(entry);
                }
                Ok(kept)
            }
            h => Ok(h),
        }
    }
}

/// The calling thread's id.
fn current_tid() -> Result<u64, NtStatus> {
    let me = thread::current().ok_or(NtStatus::INVALID_HANDLE)?;
    match &me.body {
        ObjectBody::Thread(t) => Ok(t.id),
        _ => Err(NtStatus::INVALID_HANDLE),
    }
}

/// An open of `what` "`name`" found nothing: logged (the first 32
/// times), so a program's failing open explains itself.
fn note_not_found(call: &Call<'_>, what: &str, name: &[u16], status: NtStatus) {
    if NOT_FOUND.fetch_add(1, Ordering::Relaxed) < 32 {
        info!(
            "winsta: pid {} ({}, session {}) asked for {what} \"{}\": {:#010x}",
            call.process.id,
            call.process.name,
            call.process.session,
            Plain(name),
            status.0
        );
    }
}

/// GalaUserObjectOpen(Kind, *Name, NameLength, DesiredAccess, Inherit,
/// *Handle): Kind 0 opens window station Name of the caller's session
/// (OpenWindowStationW), 1 desktop Name of the caller's window station
/// (OpenDesktopW: "" is STATUS_OBJECT_TYPE_MISMATCH, a backslash
/// STATUS_OBJECT_PATH_SYNTAX_BAD, as Windows reports them), 2 the input
/// desktop (OpenInputDesktop: STATUS_INVALID_DEVICE_REQUEST from a window
/// station that cannot receive input). A new handle each time.
pub(super) fn gala_user_object_open(call: &mut Call<'_>) -> NtStatus {
    let (kind, ptr, len) = (arg!(call, 0) as u32, arg!(call, 1), arg!(call, 2) as u32 as u64);
    let (access, inherit, out) = (arg!(call, 3) as u32, arg!(call, 4) as u32, arg!(call, 5));
    if len > MAX_NAME {
        return NtStatus::INVALID_PARAMETER;
    }
    let name = tri!(call.read_wide(ptr, len));
    let object = match kind {
        0 => match winsta::station_by_name(call.process.session, &name) {
            Ok(s) => s,
            Err(e) => {
                note_not_found(call, "window station", &name, e);
                return e;
            }
        },
        1 | 2 => {
            let (station, _) = tri!(call.connection());
            let Some(st) = station_of(&station) else { return NtStatus::OBJECT_TYPE_MISMATCH };
            let wanted: Vec<u16> = if kind == 2 {
                if !st.interactive {
                    note_not_found(
                        call,
                        "the input desktop of",
                        &st.name,
                        NtStatus::INVALID_DEVICE_REQUEST,
                    );
                    return NtStatus::INVALID_DEVICE_REQUEST;
                }
                let mut d = Vec::new();
                for b in DEFAULT_DESKTOP.bytes() {
                    tri!(d.try_push(u16::from(b)).map_err(|_| NtStatus::NO_MEMORY));
                }
                d
            } else if name.is_empty() {
                return NtStatus::OBJECT_TYPE_MISMATCH;
            } else if name.contains(&u16::from(b'\\')) {
                return NtStatus::OBJECT_PATH_SYNTAX_BAD;
            } else {
                name
            };
            match st.desktop(&wanted) {
                Some(d) => d,
                None => {
                    note_not_found(call, "desktop", &wanted, NtStatus::OBJECT_NAME_NOT_FOUND);
                    return NtStatus::OBJECT_NAME_NOT_FOUND;
                }
            }
        }
        _ => return NtStatus::INVALID_PARAMETER,
    };
    let attributes = if inherit != 0 { HANDLE_ATTRIBUTES } else { 0 };
    call.return_handle(out, object, access, attributes)
}

/// GalaUserObjectConnection(What, ThreadId, *Handle): What 0 is the
/// process's window station handle (GetProcessWindowStation), 1 the
/// desktop handle of thread ThreadId of the caller's process (0: the
/// calling thread; GetThreadDesktop). The same handle each time; it
/// cannot be closed while in use.
pub(super) fn gala_user_object_connection(call: &mut Call<'_>) -> NtStatus {
    let (what, tid, out) = (arg!(call, 0) as u32, arg!(call, 1) as u32 as u64, arg!(call, 2));
    let handle = match what {
        0 => tri!(call.station_handle()),
        1 => {
            let tid = if tid == 0 { tri!(current_tid()) } else { tid };
            let threads = tri!(call.process.threads().map_err(|_| NtStatus::NO_MEMORY));
            let ours =
                threads.iter().any(|t| matches!(&t.body, ObjectBody::Thread(t) if t.id == tid));
            if !ours {
                return NtStatus::INVALID_PARAMETER;
            }
            tri!(call.desktop_handle(tid))
        }
        _ => return NtStatus::INVALID_PARAMETER,
    };
    tri!(call.write_u64(out, handle));
    NtStatus::SUCCESS
}

/// GalaUserObjectSet(What, Handle): What 0 makes Handle (a window
/// station) the process's (SetProcessWindowStation), 1 makes Handle (a
/// desktop) the calling thread's (SetThreadDesktop). Every desktop of a
/// session shares its windows, so a thread with windows may change too.
pub(super) fn gala_user_object_set(call: &mut Call<'_>) -> NtStatus {
    let (what, h) = (arg!(call, 0) as u32, arg!(call, 1));
    let entry = tri!(call.handle(Handle(h)));
    tri!(call.connection());
    match what {
        0 => {
            if station_of(&entry.object).is_none() {
                return NtStatus::OBJECT_TYPE_MISMATCH;
            }
            call.process.ui.set_station(h, entry.object.clone());
        }
        1 => {
            if desktop_of(&entry.object).is_none() {
                return NtStatus::OBJECT_TYPE_MISMATCH;
            }
            let tid = tri!(current_tid());
            // The startup desktop's handle exists first, so that setting
            // it back is told apart from setting another handle.
            tri!(call.desktop_handle(0));
            tri!(call.process.ui.set_thread_desktop(tid, h, entry.object.clone()));
        }
        _ => return NtStatus::INVALID_PARAMETER,
    }
    NtStatus::SUCCESS
}

/// GalaUserObjectClose(Kind, Handle): CloseWindowStation (Kind 0) or
/// CloseDesktop (1): the handle must be of that kind
/// (STATUS_OBJECT_TYPE_MISMATCH) and not in use by the process's
/// connection (STATUS_DEVICE_BUSY).
pub(super) fn gala_user_object_close(call: &mut Call<'_>) -> NtStatus {
    let (kind, h) = (arg!(call, 0) as u32, arg!(call, 1));
    let entry = tri!(call.handle(Handle(h)));
    let (right_kind, busy) = match kind {
        0 => (station_of(&entry.object).is_some(), call.process.ui.holds(h)),
        1 => (desktop_of(&entry.object).is_some(), call.process.ui.desktop_in_use(h)),
        _ => return NtStatus::INVALID_PARAMETER,
    };
    if !right_kind {
        return NtStatus::OBJECT_TYPE_MISMATCH;
    }
    if busy {
        return NtStatus::DEVICE_BUSY;
    }
    drop(entry);
    let closed = call.process.handles.lock().close(Handle(h));
    drop(closed);
    NtStatus::SUCCESS
}

/// Appends `s` and a NUL as UTF-16 bytes.
fn push_text(out: &mut Vec<u8>, s: impl Iterator<Item = u16>) -> Result<(), NtStatus> {
    for c in s.chain(core::iter::once(0)) {
        out.try_extend_from_slice(&c.to_le_bytes()).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(())
}

/// Copies `bytes` to the caller's buffer, their size to *Needed (if
/// given); STATUS_BUFFER_TOO_SMALL if Length is short.
fn give(call: &Call<'_>, bytes: &[u8], buffer: u64, length: u64, needed: u64) -> NtStatus {
    if needed != 0 {
        tri!(call.write_u32(needed, bytes.len() as u32));
    }
    if buffer == 0 || length < bytes.len() as u64 {
        return NtStatus::BUFFER_TOO_SMALL;
    }
    if call.process.write(buffer, bytes).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    NtStatus::SUCCESS
}

/// GalaUserObjectInformation(Handle, Index, *Buffer, Length, *Needed):
/// GetUserObjectInformationW of a window station or desktop handle:
/// UOI_FLAGS (USEROBJECTFLAGS: the handle's inheritance, WSF_VISIBLE for
/// a visible station), UOI_NAME, UOI_TYPE ("WindowStation", "Desktop";
/// UTF-16 with the NUL), UOI_IO (a BOOL: the desktop receives input).
/// *Needed gets the size, also when Length is short.
pub(super) fn gala_user_object_information(call: &mut Call<'_>) -> NtStatus {
    let (h, index, buffer) = (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2));
    let (length, needed) = (arg!(call, 3) as u32 as u64, arg!(call, 4));
    let entry = tri!(call.handle(Handle(h)));
    let (station, desktop) = (station_of(&entry.object), desktop_of(&entry.object));
    if station.is_none() && desktop.is_none() {
        return NtStatus::OBJECT_TYPE_MISMATCH;
    }
    let mut bytes = Vec::new();
    match index {
        UOI_FLAGS => {
            let inherit = u32::from(entry.attributes & HANDLE_ATTRIBUTES != 0);
            let flags = if station.is_some_and(|s| s.visible()) { WSF_VISIBLE } else { 0 };
            for v in [inherit, 0, flags] {
                tri!(
                    bytes.try_extend_from_slice(&v.to_le_bytes()).map_err(|_| NtStatus::NO_MEMORY)
                );
            }
        }
        UOI_NAME => {
            let name = station.map(|s| &s.name).or(desktop.map(|d| &d.name));
            tri!(push_text(&mut bytes, name.map_or(&[][..], |n| n).iter().copied()));
        }
        UOI_TYPE => {
            tri!(push_text(&mut bytes, entry.object.type_name().bytes().map(u16::from)));
        }
        UOI_IO => {
            let io = u32::from(desktop.is_some_and(|d| d.receives_input()));
            tri!(bytes.try_extend_from_slice(&io.to_le_bytes()).map_err(|_| NtStatus::NO_MEMORY));
        }
        _ => {
            // UOI_USER_SID, UOI_HEAPSIZE, UOI_TIMERPROC_EXCEPTION_SUPPRESSION.
            super::super::ledger::record("GalaUserObjectInformation", "index", u64::from(index));
            return NtStatus::INVALID_PARAMETER;
        }
    }
    give(call, &bytes, buffer, length, needed)
}

/// GalaUserObjectEnum(Handle, *Buffer, Length, *Needed): the names of the
/// caller's session's window stations (Handle 0; EnumWindowStationsW) or
/// of a window station's desktops (WINSTA_ENUMDESKTOPS; EnumDesktopsW),
/// each UTF-16 with its NUL, then one more NUL.
pub(super) fn gala_user_object_enum(call: &mut Call<'_>) -> NtStatus {
    let (h, buffer, length, needed) =
        (arg!(call, 0), arg!(call, 1), arg!(call, 2) as u32 as u64, arg!(call, 3));
    let names = if h == 0 {
        tri!(winsta::station_names(call.process.session))
    } else {
        let entry = tri!(call.handle(Handle(h)));
        let Some(station) = station_of(&entry.object) else {
            return NtStatus::OBJECT_TYPE_MISMATCH;
        };
        tri!(entry.require(WINSTA_ENUMDESKTOPS));
        tri!(station.desktop_names())
    };
    let mut bytes = Vec::new();
    for name in &names {
        tri!(push_text(&mut bytes, name.iter().copied()));
    }
    tri!(bytes.try_extend_from_slice(&[0, 0]).map_err(|_| NtStatus::NO_MEMORY));
    give(call, &bytes, buffer, length, needed)
}
