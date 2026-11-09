//! The service control manager's calls over `nt::scm`, for advapi32's
//! service functions (ARCHITECTURE "The service control manager"). The
//! SCM's own errors come back as `scm::win32(error)` (FACILITY_NTWIN32);
//! a handle that is not the SCM's is STATUS_INVALID_HANDLE
//! (ERROR_INVALID_HANDLE, as Windows answers).

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use super::super::object::{Handle, HandleEntry, KObject, ObjectBody, ObjectRef};
use super::super::registry;
use super::super::scm::{
    self, ERROR_INVALID_NAME, ERROR_INVALID_PARAMETER, ERROR_INVALID_SERVICE_CONTROL,
    ERROR_NOT_SUPPORTED, ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_DISABLED,
    ERROR_SERVICE_MARKED_FOR_DELETE, ERROR_SERVICE_NOT_ACTIVE, SC_MANAGER_CONNECT,
    SC_MANAGER_CREATE_SERVICE, SC_MANAGER_ENUMERATE_SERVICE, SERVICE_CHANGE_CONFIG,
    SERVICE_DISABLED, SERVICE_INTERROGATE, SERVICE_PAUSE_CONTINUE, SERVICE_QUERY_CONFIG,
    SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START, SERVICE_STOP, SERVICE_STOPPED,
    SERVICE_USER_DEFINED_CONTROL, ScManager, ServiceObject, win32,
};
use super::super::scm_config;
use super::super::status::NtStatus;
use super::super::winsta::Plain;
use super::Call;
use crate::mm::heap::TryVec;

/// Log lines written so far (the first `LOG_LINES` are; every event is
/// in the `service` stream).
static LOGGED: AtomicU32 = AtomicU32::new(0);
const LOG_LINES: u32 = 64;

fn nomem<T>(_: T) -> NtStatus {
    NtStatus::NO_MEMORY
}

/// Logs one SCM line and writes its event: `op` on `name` by the caller,
/// with the access granted or the failing status.
fn note(call: &Call<'_>, op: &str, name: &[u16], result: Result<u32, NtStatus>) {
    let (pid, process) = (call.process.id, call.process.name);
    let n = LOGGED.fetch_add(1, Ordering::Relaxed);
    if n < LOG_LINES {
        match result {
            Ok(access) => {
                info!("scm: pid {pid} ({process}) {op} \"{}\": access {access:#x}", Plain(name))
            }
            Err(e) => info!(
                "scm: pid {pid} ({process}) {op} \"{}\": {:#010x}{}",
                Plain(name),
                e.0,
                if e.0 & 0xffff_0000 == 0xc007_0000 { " (a Win32 error)" } else { "" }
            ),
        }
    } else if n == LOG_LINES {
        info!("scm: further calls are in the event stream only (events=service)");
    }
    crate::events::emit(
        crate::events::Category::Service,
        "service",
        format_args!(
            r#""op":"{op}","pid":{pid},"name":{},"access":{},"status":{}"#,
            crate::events::Utf16(name),
            result.unwrap_or(0),
            result.err().map_or(0, |e| e.0)
        ),
    );
}

/// The SCM connection behind handle `h` (STATUS_INVALID_HANDLE for
/// anything else).
fn manager(call: &Call<'_>, h: u64) -> Result<HandleEntry, NtStatus> {
    let entry = call.handle(Handle(h))?;
    match entry.object.body {
        ObjectBody::ScManager(_) => Ok(entry),
        _ => Err(NtStatus::INVALID_HANDLE),
    }
}

/// The service behind handle `h`: its entry and name.
fn service(call: &Call<'_>, h: u64) -> Result<(HandleEntry, Vec<u16>), NtStatus> {
    let entry = call.handle(Handle(h))?;
    let name = match &entry.object.body {
        ObjectBody::Service(s) => {
            let mut n = Vec::new();
            n.try_extend_from_slice(&s.name).map_err(nomem)?;
            n
        }
        _ => return Err(NtStatus::INVALID_HANDLE),
    };
    Ok((entry, name))
}

/// A new object with descriptor `sd`, the access check of `desired`
/// against it (the caller's effective token), and its handle at `out`.
fn open(
    call: &Call<'_>,
    body: ObjectBody,
    sd: Vec<u8>,
    desired: u32,
    out: u64,
) -> Result<u32, NtStatus> {
    let object: ObjectRef = KObject::new(body).map_err(nomem)?;
    *object.security.lock() = Some(sd);
    let granted = super::security_calls::check_object(call, &object, desired)?;
    match call.return_handle(out, object, granted, 0) {
        NtStatus::SUCCESS => Ok(granted),
        e => Err(e),
    }
}

/// GalaScmOpen(DesiredAccess, *Handle): OpenSCManager of the local
/// active database; SC_MANAGER_CONNECT is always asked for, as
/// documented. The access check is against the SCM's descriptor.
pub(super) fn gala_scm_open(call: &mut Call<'_>) -> NtStatus {
    let (desired, out) = (arg!(call, 0) as u32, arg!(call, 1));
    let sd = tri!(scm::manager_descriptor());
    let result =
        open(call, ObjectBody::ScManager(ScManager), sd, desired | SC_MANAGER_CONNECT, out);
    let label: [u16; 14] = core::array::from_fn(|i| u16::from(b"ServicesActive"[i]));
    note(call, "connected to", &label, result);
    result.err().unwrap_or(NtStatus::SUCCESS)
}

/// GalaServiceOpen(Scm, *Name, NameLength, DesiredAccess, *Handle):
/// OpenService by the service's key name (not its display name):
/// ERROR_INVALID_NAME for an empty name, one over 256 characters or one
/// with a slash; ERROR_SERVICE_DOES_NOT_EXIST; then the access check
/// against the service's descriptor.
pub(super) fn gala_service_open(call: &mut Call<'_>) -> NtStatus {
    let (h, ptr, len) = (arg!(call, 0), arg!(call, 1), arg!(call, 2) as u32 as u64);
    let (desired, out) = (arg!(call, 3) as u32, arg!(call, 4));
    tri!(manager(call, h));
    if len == 0 || len > scm::MAX_NAME as u64 {
        return win32(ERROR_INVALID_NAME);
    }
    let name = tri!(call.read_wide(ptr, len));
    if !scm::valid_name(&name) {
        note(call, "opened", &name, Err(win32(ERROR_INVALID_NAME)));
        return win32(ERROR_INVALID_NAME);
    }
    let found = registry::with(|r| {
        let id = scm::find(r, &name)?;
        Ok((scm::key_name(r, id)?, scm::service_descriptor(r, id)?))
    });
    let result = found.and_then(|(key, sd)| {
        let object = ServiceObject::new(&key)?;
        open(call, ObjectBody::Service(object), sd, desired, out)
    });
    note(call, "opened", &name, result);
    result.err().unwrap_or(NtStatus::SUCCESS)
}

/// The UTF-16 string at (offset, length) in `record` (characters).
fn field(record: &[u8], at: usize) -> Result<Vec<u16>, NtStatus> {
    let word = |i: usize| -> Result<usize, NtStatus> {
        let b = record.get(i..i + 4).ok_or(NtStatus::INVALID_PARAMETER)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    let (offset, len) = (word(at)?, word(at + 4)?);
    let bytes = record.get(offset..offset + 2 * len).ok_or(NtStatus::INVALID_PARAMETER)?;
    let mut out = Vec::new();
    for pair in bytes.chunks_exact(2) {
        out.try_push(u16::from_le_bytes([pair[0], pair[1]])).map_err(nomem)?;
    }
    Ok(out)
}

/// The largest CreateService record (its strings included).
const MAX_RECORD: u64 = 64 * 1024;

/// GalaServiceCreate(Scm, *Record, RecordLength, DesiredAccess, *Handle):
/// CreateService. Record: Type, Start, ErrorControl, a reserved word,
/// then (offset, length in characters) pairs for the name, display name,
/// path, load order group, dependencies (NUL-separated) and account, the
/// strings after them. SC_MANAGER_CREATE_SERVICE is checked first, then
/// `scm::validate`'s rules, then existence; the new service's handle is
/// checked against the default descriptor.
pub(super) fn gala_service_create(call: &mut Call<'_>) -> NtStatus {
    let (h, ptr, len) = (arg!(call, 0), arg!(call, 1), arg!(call, 2) as u32 as u64);
    let (desired, out) = (arg!(call, 3) as u32, arg!(call, 4));
    let entry = tri!(manager(call, h));
    if !(64..=MAX_RECORD).contains(&len) {
        return NtStatus::INVALID_PARAMETER;
    }
    let mut record = Vec::new();
    tri!(record.try_reserve_exact(len as usize).map_err(nomem));
    record.resize(len as usize, 0u8);
    if call.process.read(ptr, &mut record).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let word =
        |i: usize| u32::from_le_bytes([record[i], record[i + 1], record[i + 2], record[i + 3]]);
    let (kind, start, error_control) = (word(0), word(4), word(8));
    let name = tri!(field(&record, 16));
    if entry.require(SC_MANAGER_CREATE_SERVICE).is_err() {
        note(call, "created", &name, Err(NtStatus::ACCESS_DENIED));
        return NtStatus::ACCESS_DENIED;
    }
    let (display, path, group) =
        (tri!(field(&record, 24)), tri!(field(&record, 32)), tri!(field(&record, 40)));
    let (dependencies, start_name) = (tri!(field(&record, 48)), tri!(field(&record, 56)));
    let new = scm::NewService {
        name: &name,
        display: &display,
        path: &path,
        group: &group,
        dependencies: &dependencies,
        start_name: &start_name,
        kind,
        start,
        error_control,
    };
    if let Err(e) = scm::validate(&new) {
        if e == ERROR_INVALID_PARAMETER && kind & 0x40 != 0 {
            super::super::ledger::record("GalaServiceCreate", "service type", u64::from(kind));
        }
        note(call, "created", &name, Err(win32(e)));
        return win32(e);
    }
    let now = super::super::platform::system_time();
    let result = registry::with(|r| scm::create(r, &new, now)).and_then(|key| {
        let sd = scm::default_service_descriptor()?;
        let object = ServiceObject::new(&key)?;
        open(call, ObjectBody::Service(object), sd, desired, out)
    });
    if result.is_ok() {
        info!(
            "scm: service \"{}\" created: type {kind:#x}, start {start}, path \"{}\", account \"{}\"",
            Plain(&name),
            Plain(&path),
            Plain(&start_name)
        );
    }
    note(call, "created", &name, result);
    result.err().unwrap_or(NtStatus::SUCCESS)
}

/// Copies `bytes` to the caller's buffer, their size to *Needed;
/// STATUS_BUFFER_TOO_SMALL if Length is short.
fn give(call: &Call<'_>, bytes: &[u8], buffer: u64, length: u64, needed: u64) -> NtStatus {
    if needed != 0 {
        tri!(call.write_u32(needed, bytes.len() as u32));
    }
    if length < bytes.len() as u64 {
        return NtStatus::BUFFER_TOO_SMALL;
    }
    if !bytes.is_empty() && call.process.write(buffer, bytes).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    NtStatus::SUCCESS
}

fn push_u32(out: &mut Vec<u8>, v: u32) -> Result<(), NtStatus> {
    out.try_extend_from_slice(&v.to_le_bytes()).map_err(nomem)
}

fn push_text(out: &mut Vec<u8>, s: &[u16]) -> Result<(), NtStatus> {
    for &c in s {
        out.try_extend_from_slice(&c.to_le_bytes()).map_err(nomem)?;
    }
    Ok(())
}

/// GalaServiceQuery(Service, Class, *Buffer, Length, *Needed): Class 0 is
/// SERVICE_STATUS_PROCESS (36 bytes; SERVICE_QUERY_STATUS), 1 the
/// configuration (SERVICE_QUERY_CONFIG): Type, Start, ErrorControl, Tag,
/// then (offset, length in characters) pairs for the path, load order
/// group, dependencies (NUL-separated), account and display name, the
/// strings after them. The access is checked before the size.
pub(super) fn gala_service_query(call: &mut Call<'_>) -> NtStatus {
    let (h, class, buffer) = (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2));
    let (length, needed) = (arg!(call, 3) as u32 as u64, arg!(call, 4));
    let (entry, name) = tri!(service(call, h));
    let mut out = Vec::new();
    match class {
        0 => {
            tri!(entry.require(SERVICE_QUERY_STATUS));
            let kind = tri!(registry::with(|r| Ok(scm::config(r, scm::find(r, &name)?)?.kind)));
            for v in scm::status(&name, kind) {
                tri!(push_u32(&mut out, v));
            }
        }
        1 => {
            tri!(entry.require(SERVICE_QUERY_CONFIG));
            let c = tri!(registry::with(|r| scm::config(r, scm::find(r, &name)?)));
            for v in [c.kind, c.start, c.error_control, c.tag] {
                tri!(push_u32(&mut out, v));
            }
            let texts = [&c.path, &c.group, &c.dependencies, &c.start_name, &c.display];
            let mut at = 16 + 8 * texts.len();
            for t in texts {
                tri!(push_u32(&mut out, at as u32));
                tri!(push_u32(&mut out, t.len() as u32));
                at += 2 * t.len();
            }
            for t in texts {
                tri!(push_text(&mut out, t));
            }
        }
        0x100.. => {
            // QueryServiceConfig2: the level's record (scm_config).
            tri!(entry.require(SERVICE_QUERY_CONFIG));
            out = tri!(registry::with(|r| scm_config::get(r, scm::find(r, &name)?, class - 0x100)));
        }
        _ => {
            super::super::ledger::record("GalaServiceQuery", "class", u64::from(class));
            return NtStatus::INVALID_INFO_CLASS;
        }
    }
    give(call, &out, buffer, length, needed)
}

/// GalaServiceConfigure(Service, Level, *Record, Length):
/// ChangeServiceConfig2's level `Level` as `scm_config`'s record;
/// SERVICE_CHANGE_CONFIG, and SERVICE_START for a restart action.
pub(super) fn gala_service_configure(call: &mut Call<'_>) -> NtStatus {
    let (h, level, ptr) = (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2));
    let len = arg!(call, 3) as u32 as u64;
    let (entry, name) = tri!(service(call, h));
    if len > MAX_RECORD {
        return NtStatus::INVALID_PARAMETER;
    }
    let mut record = Vec::new();
    tri!(record.try_reserve_exact(len as usize).map_err(nomem));
    record.resize(len as usize, 0u8);
    if call.process.read(ptr, &mut record).is_err() {
        return NtStatus::ACCESS_VIOLATION;
    }
    let result = (|| {
        entry.require(SERVICE_CHANGE_CONFIG)?;
        if scm_config::needs_start(level, &record)? {
            entry.require(SERVICE_START)?;
        }
        let now = super::super::platform::system_time();
        registry::with(|r| scm_config::set(r, scm::find(r, &name)?, level, &record, now))?;
        Ok(level)
    })();
    note(call, "changed configuration level of", &name, result);
    result.err().unwrap_or(NtStatus::SUCCESS)
}

/// GalaServiceEnum(Scm, Type, State, *Buffer, Length, *Needed):
/// EnumServicesStatusEx's list (ERROR_INVALID_PARAMETER for a type with
/// no service bit or a state not 1-3, then SC_MANAGER_ENUMERATE_SERVICE):
/// per service, its record's size, SERVICE_STATUS_PROCESS, the lengths
/// (characters) of its name, display name and group, then the three.
pub(super) fn gala_service_enum(call: &mut Call<'_>) -> NtStatus {
    let (h, kinds, states) = (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2) as u32);
    let (buffer, length, needed) = (arg!(call, 3), arg!(call, 4) as u32 as u64, arg!(call, 5));
    let entry = tri!(manager(call, h));
    if kinds & (scm::SERVICE_DRIVER | scm::SERVICE_WIN32) == 0 || !(1..=3).contains(&states) {
        return win32(ERROR_INVALID_PARAMETER);
    }
    tri!(entry.require(SC_MANAGER_ENUMERATE_SERVICE));
    let listed = tri!(registry::with(|r| scm::list(r, kinds, states)));
    let mut out = Vec::new();
    for s in &listed {
        let size = 4 + 36 + 12 + 2 * (s.name.len() + s.display.len() + s.group.len());
        tri!(push_u32(&mut out, size as u32));
        for v in s.status {
            tri!(push_u32(&mut out, v));
        }
        for t in [&s.name, &s.display, &s.group] {
            tri!(push_u32(&mut out, t.len() as u32));
        }
        for t in [&s.name, &s.display, &s.group] {
            tri!(push_text(&mut out, t));
        }
    }
    give(call, &out, buffer, length, needed)
}

/// What ControlService's code `code` needs (None: not a code a program
/// may send, ERROR_INVALID_PARAMETER).
fn control_right(code: u32) -> Option<u32> {
    match code {
        1 => Some(SERVICE_STOP),
        2 | 3 | 6..=10 => Some(SERVICE_PAUSE_CONTINUE),
        4 => Some(SERVICE_INTERROGATE),
        128..=255 => Some(SERVICE_USER_DEFINED_CONTROL),
        _ => None,
    }
}

/// GalaServiceControl(Handle, Op, Code, *Status): Op 0 closes an SCM or
/// service handle (CloseServiceHandle); 1 marks a service for deletion
/// (DELETE; it goes with its last handle); 2 starts it (SERVICE_START:
/// ERROR_SERVICE_MARKED_FOR_DELETE, ERROR_SERVICE_DISABLED,
/// ERROR_SERVICE_ALREADY_RUNNING; starting a service process is not
/// supported yet: recorded, ERROR_NOT_SUPPORTED); 3 sends control Code
/// (ControlService): its right, then ERROR_SERVICE_NOT_ACTIVE for a
/// stopped service and ERROR_INVALID_SERVICE_CONTROL for anything but
/// INTERROGATE to a built-in driver (which accepts no controls), the
/// SERVICE_STATUS (28 bytes) at Status with those answers.
pub(super) fn gala_service_control(call: &mut Call<'_>) -> NtStatus {
    let (h, op, code, status_out) =
        (arg!(call, 0), arg!(call, 1) as u32, arg!(call, 2) as u32, arg!(call, 3));
    if op == 0 {
        let entry = tri!(call.handle(Handle(h)));
        if !matches!(entry.object.body, ObjectBody::ScManager(_) | ObjectBody::Service(_)) {
            return NtStatus::INVALID_HANDLE;
        }
        drop(entry);
        let closed = call.process.handles.lock().close(Handle(h));
        drop(closed);
        return NtStatus::SUCCESS;
    }
    let (entry, name) = tri!(service(call, h));
    let result = match op {
        1 => entry.require(scm::DELETE).map(drop).and_then(|()| scm::mark(&name)),
        2 => start(&entry, &name),
        3 => return control(call, &entry, &name, code, status_out),
        _ => return NtStatus::INVALID_PARAMETER,
    };
    drop(entry);
    note(
        call,
        if op == 1 { "marked for deletion" } else { "asked to start" },
        &name,
        result.map(|()| 0),
    );
    result.err().unwrap_or(NtStatus::SUCCESS)
}

fn start(entry: &HandleEntry, name: &[u16]) -> Result<(), NtStatus> {
    entry.require(SERVICE_START)?;
    if scm::marked(name) {
        return Err(win32(ERROR_SERVICE_MARKED_FOR_DELETE));
    }
    let c = registry::with(|r| scm::config(r, scm::find(r, name)?))?;
    if c.start == SERVICE_DISABLED {
        return Err(win32(ERROR_SERVICE_DISABLED));
    }
    if scm::status(name, c.kind)[1] == SERVICE_RUNNING {
        return Err(win32(ERROR_SERVICE_ALREADY_RUNNING));
    }
    warn!(
        "scm: starting service \"{}\" (\"{}\", type {:#x}) is not supported yet",
        Plain(name),
        Plain(&c.path),
        c.kind
    );
    super::super::ledger::record("GalaServiceControl", "start a service", u64::from(c.kind));
    Err(win32(ERROR_NOT_SUPPORTED))
}

fn control(call: &Call<'_>, entry: &HandleEntry, name: &[u16], code: u32, out: u64) -> NtStatus {
    let Some(right) = control_right(code) else { return win32(ERROR_INVALID_PARAMETER) };
    tri!(entry.require(right));
    let kind = tri!(registry::with(|r| Ok(scm::config(r, scm::find(r, name)?)?.kind)));
    let status = scm::status(name, kind);
    let answer = if status[1] == SERVICE_STOPPED {
        win32(ERROR_SERVICE_NOT_ACTIVE)
    } else if code == 4 {
        NtStatus::SUCCESS
    } else {
        win32(ERROR_INVALID_SERVICE_CONTROL)
    };
    if out != 0 {
        let mut bytes = Vec::new();
        for v in &status[..7] {
            tri!(push_u32(&mut bytes, *v));
        }
        if call.process.write(out, &bytes).is_err() {
            return NtStatus::ACCESS_VIOLATION;
        }
    }
    note(
        call,
        "sent a control to",
        name,
        if answer == NtStatus::SUCCESS { Ok(code) } else { Err(answer) },
    );
    answer
}
