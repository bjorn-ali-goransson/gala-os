//! Services that run (ARCHITECTURE "The service control manager"): the
//! SCM starts a Win32 service's ImagePath as a process of its own under
//! its account's token (LocalSystem: SYSTEM's, session 0), waits for the
//! process's control dispatcher (advapi32's StartServiceCtrlDispatcher)
//! to connect, hands it the start request and the controls, and keeps the
//! status the service reports (SetServiceStatus). Each service process
//! has one request slot: the SCM's side (StartService, ControlService)
//! takes the slot (`free`), puts its request and waits for the answer;
//! the dispatcher's thread waits for a request (`pending`), runs it
//! (ServiceMain's thread, the control handler) and answers (`answered`).
//! The state is kept from a start to the next, so a stopped service
//! still reports how it ended (ERROR_PROCESS_ABORTED for a process that
//! ended while the service ran).

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use super::object::{Event, EventType, KObject, ObjectBody, ObjectRef};
use super::process::Process;
use super::registry;
use super::scm::{self, Config, win32};
use super::status::NtStatus;
use super::wait::{self, Dispatcher, Timeout};
use super::winsta::Plain;
use crate::mm::heap::{KArc, TryVec};
use crate::sync::IrqMutex;

// Service states (winsvc.h).
pub const SERVICE_STOPPED: u32 = 1;
pub const SERVICE_START_PENDING: u32 = 2;
pub const SERVICE_STOP_PENDING: u32 = 3;
pub const SERVICE_RUNNING: u32 = 4;
pub const SERVICE_CONTINUE_PENDING: u32 = 5;
pub const SERVICE_PAUSE_PENDING: u32 = 6;
pub const SERVICE_PAUSED: u32 = 7;

// Controls and what a service accepts for them.
pub const SERVICE_CONTROL_STOP: u32 = 1;
pub const SERVICE_CONTROL_INTERROGATE: u32 = 4;

// The Win32 errors of starting and controlling.
pub const ERROR_INVALID_DATA: u32 = 13;
pub const ERROR_SERVICE_REQUEST_TIMEOUT: u32 = 1053;
pub const ERROR_SERVICE_CANNOT_ACCEPT_CTRL: u32 = 1061;
pub const ERROR_FAILED_SERVICE_CONTROLLER_CONNECT: u32 = 1063;
pub const ERROR_PROCESS_ABORTED: u32 = 1067;
pub const ERROR_SERVICE_LOGON_FAILED: u32 = 1069;
pub const ERROR_INVALID_HANDLE: u32 = 6;

/// The requests a dispatcher gets (GalaServiceDispatch's record).
pub const REQUEST_START: u32 = 1;
pub const REQUEST_CONTROL: u32 = 2;
/// Every service of the process has stopped: StartServiceCtrlDispatcher
/// returns.
pub const REQUEST_RETURN: u32 = 3;

/// How long the SCM waits for a service's dispatcher to connect when the
/// registry does not say (`ServicesPipeTimeout`, milliseconds: 30 s, as
/// documented for StartService and the service's handshake).
const PIPE_TIMEOUT_MS: u32 = 30_000;
/// How long a control handler (and the start of ServiceMain's thread) may
/// take: 30 s, as documented for HandlerEx.
const CONTROL_TIMEOUT_MS: u32 = 30_000;

/// The longest start arguments (characters, NULs included).
pub const MAX_ARGS: usize = 16 * 1024;

/// A service process's request slot's signals (new for every start).
#[derive(Clone)]
struct Signals {
    /// Set when the dispatcher connects (notification).
    connected: ObjectRef,
    /// A request (or the return) waits for the dispatcher (auto reset).
    pending: ObjectRef,
    /// The dispatcher answered (auto reset).
    answered: ObjectRef,
    /// The slot is free (auto reset, set at first).
    free: ObjectRef,
}

fn event(kind: EventType, signaled: bool) -> Result<ObjectRef, NtStatus> {
    let d = Dispatcher::new(i32::from(signaled));
    KObject::new(ObjectBody::Event(Event { kind, dispatcher: d })).map_err(|_| NtStatus::NO_MEMORY)
}

impl Signals {
    fn new() -> Result<Signals, NtStatus> {
        Ok(Signals {
            connected: event(EventType::Notification, false)?,
            pending: event(EventType::Synchronization, false)?,
            answered: event(EventType::Synchronization, false)?,
            free: event(EventType::Synchronization, true)?,
        })
    }
}

/// A service that has been started since boot.
struct Run {
    name: Vec<u16>,
    /// SERVICE_STATUS_PROCESS (its type field is the configuration's).
    status: [u32; 9],
    /// The service's process while it is the service's.
    process: Option<KArc<Process>>,
    connected: bool,
    /// The dispatcher is to return (REQUEST_RETURN).
    returning: bool,
    request: Option<Vec<u8>>,
    reply: Option<u32>,
    signals: Signals,
}

static RUNS: IrqMutex<Vec<Run>> = IrqMutex::named("scm.runs", Vec::new());

/// Log lines written so far by the running side (the first `LOG_LINES`).
static LOGGED: AtomicU32 = AtomicU32::new(0);
const LOG_LINES: u32 = 96;

fn log_allowed() -> bool {
    let n = LOGGED.fetch_add(1, Ordering::Relaxed);
    if n == LOG_LINES {
        info!("scm: further service state changes are in the event stream only (events=service)");
    }
    n < LOG_LINES
}

/// A `service` event: `op` on `name`, with the status it leaves.
fn emit(op: &str, name: &[u16], status: &[u32; 9]) {
    crate::events::emit(
        crate::events::Category::Service,
        "service",
        format_args!(
            r#""op":"{op}","name":{},"state":{},"exit":{},"specific":{},"service_pid":{}"#,
            crate::events::Utf16(name),
            status[1],
            status[3],
            status[4],
            status[7]
        ),
    );
}

fn same(a: &[u16], b: &[u16]) -> bool {
    let up = |c: u16| if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c };
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| up(x) == up(y))
}

fn nomem<T>(_: T) -> NtStatus {
    NtStatus::NO_MEMORY
}

/// SERVICE_STATUS_PROCESS of service `name` of type `kind`: what it last
/// reported if it has run; else the built-in drivers run (no process, no
/// controls accepted) and every other service is stopped and was never
/// started.
pub fn status(name: &[u16], kind: u32) -> [u32; 9] {
    if let Some(r) = RUNS.lock().iter().find(|r| same(&r.name, name)) {
        let mut s = r.status;
        s[0] = kind;
        return s;
    }
    if scm::built_in(name) {
        [kind, SERVICE_RUNNING, 0, 0, 0, 0, 0, 0, 0]
    } else {
        [kind, SERVICE_STOPPED, 0, scm::ERROR_SERVICE_NEVER_STARTED, 0, 0, 0, 0, 0]
    }
}

/// Whether service `name` is not stopped (its deletion waits for that).
pub fn active(name: &[u16]) -> bool {
    RUNS.lock().iter().any(|r| same(&r.name, name) && r.status[1] != SERVICE_STOPPED)
}

/// Whether process `pid` is a service's process (a kernel handover does
/// not carry the SCM's record of it).
pub fn runs_service(pid: u64) -> bool {
    RUNS.lock().iter().any(|r| r.process.as_ref().is_some_and(|p| p.id == pid))
}

/// Forgets service `name`'s run (its key was deleted).
pub fn forget(name: &[u16]) {
    let gone = {
        let mut runs = RUNS.lock();
        runs.iter().position(|r| same(&r.name, name)).map(|at| runs.swap_remove(at))
    };
    drop(gone);
}

/// The service's process's executable and command line from ImagePath
/// `image` (unexpanded): `%SystemRoot%` and the like expanded, a
/// `\SystemRoot\` prefix made `C:\Windows\`; a quoted path is the text
/// in quotes, an unquoted one the shortest prefix ending before a space
/// that names a file (as CreateProcess resolves an unquoted command
/// line), else the text up to the first space. The command line is the
/// expanded ImagePath.
fn image_path(image: &[u16]) -> Result<(Vec<u8>, Vec<u8>), NtStatus> {
    let mut text: Vec<u8> = Vec::new();
    for c in char::decode_utf16(image.iter().copied()) {
        let mut b = [0u8; 4];
        let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
        text.try_extend_from_slice(c.encode_utf8(&mut b).as_bytes()).map_err(nomem)?;
    }
    let text = core::str::from_utf8(&text).map_err(|_| NtStatus::INVALID_PARAMETER)?;
    let mut buf = Vec::new();
    buf.try_reserve_exact(2 * text.len() + 260).map_err(nomem)?;
    buf.resize(2 * text.len() + 260, 0u8);
    let expanded = super::process::expand_environment(text, &mut buf).unwrap_or(text);
    let mut line: Vec<u8> = Vec::new();
    match expanded.get(..12) {
        Some(p) if p.eq_ignore_ascii_case("\\SystemRoot\\") => {
            line.try_extend_from_slice(b"C:\\Windows\\").map_err(nomem)?;
            line.try_extend_from_slice(&expanded.as_bytes()[12..]).map_err(nomem)?;
        }
        _ => line.try_extend_from_slice(expanded.as_bytes()).map_err(nomem)?,
    }
    let line_text = core::str::from_utf8(&line).map_err(|_| NtStatus::INVALID_PARAMETER)?;
    let path: &str = if let Some(rest) = line_text.strip_prefix('"') {
        rest.split('"').next().unwrap_or(rest)
    } else {
        let mut found = None;
        for (at, _) in line_text.match_indices(' ') {
            let candidate = &line_text[..at];
            if super::process::open_program(candidate).is_ok() {
                found = Some(candidate);
                break;
            }
        }
        found.unwrap_or_else(|| line_text.split(' ').next().unwrap_or(line_text))
    };
    let mut p = Vec::new();
    p.try_extend_from_slice(path.as_bytes()).map_err(nomem)?;
    Ok((p, line))
}

/// The SCM's wait for the dispatcher (ServicesPipeTimeout in
/// `HKLM\SYSTEM\CurrentControlSet\Control`, else 30 s).
fn pipe_timeout_ms() -> u32 {
    registry::with(|r| {
        let id =
            r.lookup(registry::ROOT, &scm::utf16("MACHINE\\SYSTEM\\CurrentControlSet\\Control")?)?;
        let v = r.value(id, &scm::utf16("ServicesPipeTimeout")?)?;
        if v.kind == 4 && v.data.len() >= 4 {
            Ok(u32::from_le_bytes([v.data[0], v.data[1], v.data[2], v.data[3]]))
        } else {
            Err(NtStatus::OBJECT_NAME_NOT_FOUND)
        }
    })
    .ok()
    .filter(|&ms| ms > 0)
    .unwrap_or(PIPE_TIMEOUT_MS)
}

fn timeout(ms: u32) -> Timeout {
    Timeout::from_nt(Some(-(i64::from(ms) * 10_000)))
}

fn process_object(p: &KArc<Process>) -> Result<ObjectRef, NtStatus> {
    KObject::new(ObjectBody::Process(p.clone())).map_err(|_| NtStatus::NO_MEMORY)
}

/// Waits for any of `objects` for `ms` milliseconds: its index, or None.
fn wait_any(objects: &[ObjectRef], ms: u32) -> Option<usize> {
    let status = wait::wait_for(objects, false, timeout(ms));
    ((status.0 as usize) < objects.len()).then_some(status.0 as usize)
}

/// The run of `name` with `f` applied, under the lock.
fn with_run<R>(name: &[u16], f: impl FnOnce(&mut Run) -> R) -> Option<R> {
    RUNS.lock().iter_mut().find(|r| same(&r.name, name)).map(f)
}

/// Sets the service stopped (no process, nothing accepted, the given
/// exit codes) and asks its dispatcher to return; logs and emits `op`.
fn stop(r: &mut Run, exit: u32, specific: u32, op: &str) {
    r.status[1] = SERVICE_STOPPED;
    r.status[2] = 0;
    r.status[3] = exit;
    r.status[4] = specific;
    r.status[5] = 0;
    r.status[6] = 0;
    r.status[7] = 0;
    r.returning = true;
    wait::set_signal(&r.signals.pending, 1);
    emit(op, &r.name, &r.status);
}

/// Starts service `name` (configuration `c`, StartService's arguments
/// `args`, each NUL-terminated): its process, the dispatcher's
/// connection, the start request. Returns once the dispatcher has
/// started ServiceMain's thread (as documented for StartService); the
/// service is then START_PENDING until it reports. Its Win32 error
/// otherwise: ERROR_SERVICE_ALREADY_RUNNING; a driver or an account
/// other than LocalSystem (recorded); the executable's (not found, not
/// an image); ERROR_SERVICE_REQUEST_TIMEOUT when the dispatcher does not
/// connect within ServicesPipeTimeout (the process ended or never called
/// StartServiceCtrlDispatcher: it is ended) or does not answer.
pub fn start(name: &[u16], c: &Config, args: &[u16]) -> Result<u64, NtStatus> {
    if c.kind & scm::SERVICE_WIN32 == 0 {
        warn!("scm: starting driver \"{}\" (\"{}\") is not supported", Plain(name), Plain(&c.path));
        super::ledger::record("GalaServiceControl", "start a driver", u64::from(c.kind));
        return Err(win32(scm::ERROR_NOT_SUPPORTED));
    }
    if !scm::local_system(&c.start_name) {
        warn!(
            "scm: service \"{}\" runs as \"{}\": only LocalSystem services start",
            Plain(name),
            Plain(&c.start_name)
        );
        super::ledger::record("GalaServiceControl", "start as an account", 0);
        return Err(win32(ERROR_SERVICE_LOGON_FAILED));
    }
    if c.kind & scm::SERVICE_WIN32_SHARE_PROCESS != 0 {
        // Each service gets a process of its own (svchost's shared
        // processes are not there).
        super::ledger::record("GalaServiceControl", "share process: run alone", u64::from(c.kind));
    }
    let (path, line) = image_path(&c.path)?;
    let path = core::str::from_utf8(&path).unwrap_or("");
    let line = core::str::from_utf8(&line).unwrap_or("");
    let token = super::token::object(super::token::TokenData::system()?)?;
    let signals = Signals::new()?;
    // Claimed first (START_PENDING), so that two starts cannot both run
    // it; given back as it was if the process cannot be made.
    let before = {
        let mut runs = RUNS.lock();
        let claim = Run {
            name: scm::copy(name)?,
            status: [c.kind, SERVICE_START_PENDING, 0, 0, 0, 0, 0, 0, 0],
            process: None,
            connected: false,
            returning: false,
            request: None,
            reply: None,
            signals: signals.clone(),
        };
        match runs.iter_mut().find(|r| same(&r.name, name)) {
            Some(r) if r.status[1] != SERVICE_STOPPED => {
                return Err(win32(scm::ERROR_SERVICE_ALREADY_RUNNING));
            }
            Some(r) => Some(core::mem::replace(r, claim)),
            None => {
                runs.try_push(claim).map_err(nomem)?;
                None
            }
        }
    };
    let launched = super::process::launch_service(path, line, "C:\\Windows\\System32\\", token)
        .and_then(|(p, t)| Ok((process_object(&p)?, p, t)));
    let (process_ref, process, thread) = match launched {
        Ok(l) => l,
        Err(e) => {
            warn!("scm: service \"{}\": cannot start \"{path}\": {:#010x}", Plain(name), e.0);
            let mut runs = RUNS.lock();
            if let Some(at) = runs.iter().position(|r| same(&r.name, name)) {
                match before {
                    Some(b) => runs[at] = b,
                    None => drop(runs.swap_remove(at)),
                }
            }
            return Err(e);
        }
    };
    let pid = process.id;
    with_run(name, |r| {
        r.status[7] = pid as u32;
        r.process = Some(process.clone());
    });
    if log_allowed() {
        info!("scm: service \"{}\" starting: pid {pid}, {line}", Plain(name));
    }
    emit("starting", name, &status(name, c.kind));
    super::thread::resume(&thread);
    // The handshake: the dispatcher connects, or the process ends, or the
    // time-out passes (the process is then ended, as the SCM gives up).
    let pipe_ms = pipe_timeout_ms();
    let connected = wait_any(&[signals.connected.clone(), process_ref.clone()], pipe_ms) == Some(0);
    if !connected {
        let alive = process.exit_status().is_none();
        warn!(
            "scm: service \"{}\" (pid {pid}) {} StartServiceCtrlDispatcher",
            Plain(name),
            if alive {
                "did not call, within ServicesPipeTimeout,"
            } else {
                "ended without calling"
            }
        );
        with_run(name, |r| {
            if r.status[1] != SERVICE_STOPPED {
                stop(r, ERROR_SERVICE_REQUEST_TIMEOUT, 0, "start timed out");
            }
            r.process = None;
        });
        if alive {
            super::process::terminate(&process, NtStatus(ERROR_SERVICE_REQUEST_TIMEOUT), false);
        }
        return Err(win32(ERROR_SERVICE_REQUEST_TIMEOUT));
    }
    // The start request: the service's type and name, its arguments.
    let mut request = Vec::new();
    let count = args.iter().filter(|&&a| a == 0).count();
    for v in [REQUEST_START, 0, 0, c.kind, name.len() as u32, count as u32, args.len() as u32] {
        request.try_extend_from_slice(&v.to_le_bytes()).map_err(nomem)?;
    }
    for &u in name.iter().chain(args) {
        request.try_extend_from_slice(&u.to_le_bytes()).map_err(nomem)?;
    }
    match exchange(name, request)? {
        0 => {
            if log_allowed() {
                info!("scm: service \"{}\" (pid {pid}): ServiceMain's thread started", Plain(name));
            }
            Ok(pid)
        }
        e => {
            warn!(
                "scm: service \"{}\" (pid {pid}): its dispatcher refused the start: {e}",
                Plain(name)
            );
            with_run(name, |r| {
                if r.status[1] != SERVICE_STOPPED {
                    stop(r, e, 0, "start refused");
                }
            });
            Err(win32(e))
        }
    }
}

/// Hands `request` to service `name`'s dispatcher and waits for its
/// answer (the slot first, then the answer, each at most the control
/// time-out: ERROR_SERVICE_REQUEST_TIMEOUT); ERROR_SERVICE_NOT_ACTIVE
/// when its process has ended.
fn exchange(name: &[u16], request: Vec<u8>) -> Result<u32, NtStatus> {
    let not_active = win32(scm::ERROR_SERVICE_NOT_ACTIVE);
    let (signals, process) =
        with_run(name, |r| (r.signals.clone(), r.process.clone())).ok_or(not_active)?;
    let process = process_object(&process.ok_or(not_active)?)?;
    match wait_any(&[signals.free.clone(), process.clone()], CONTROL_TIMEOUT_MS) {
        Some(0) => {}
        Some(_) => return Err(not_active),
        None => return Err(win32(ERROR_SERVICE_REQUEST_TIMEOUT)),
    }
    let mut request = Some(request);
    with_run(name, |r| {
        r.request = request.take();
        r.reply = None;
    });
    wait::set_signal(&signals.pending, 1);
    let answered = wait_any(&[signals.answered.clone(), process], CONTROL_TIMEOUT_MS);
    let reply = with_run(name, |r| {
        r.request = None;
        r.reply.take()
    })
    .flatten();
    wait::set_signal(&signals.free, 1);
    match (answered, reply) {
        (_, Some(v)) => Ok(v),
        (Some(_), None) => Err(not_active),
        (None, None) => {
            warn!("scm: service \"{}\" did not answer within 30 s", Plain(name));
            Err(win32(ERROR_SERVICE_REQUEST_TIMEOUT))
        }
    }
}

/// What control `code` needs the service to accept (SERVICE_ACCEPT_*);
/// 0: any service takes it (interrogate, user-defined controls).
fn accept_bit(code: u32) -> u32 {
    match code {
        SERVICE_CONTROL_STOP => 0x1,
        2 | 3 => 0x2,
        5 => 0x4,
        6 => 0x8,
        7..=10 => 0x10,
        _ => 0,
    }
}

/// ControlService(code) on service `name` (its right already checked):
/// ERROR_SERVICE_NOT_ACTIVE when stopped; ERROR_SERVICE_CANNOT_ACCEPT_CTRL
/// while it starts or stops (as documented); ERROR_INVALID_SERVICE_CONTROL
/// for a control it does not accept (interrogation and user-defined
/// controls always go); else the handler runs (its answer is logged, not
/// returned: Windows' ControlService succeeds for a user-defined control
/// whose handler answers 0xdeadbeef, Wine's tests). The status after it,
/// with the answer, either way.
pub fn control(name: &[u16], kind: u32, code: u32) -> (Result<(), NtStatus>, [u32; 9]) {
    let now = status(name, kind);
    let refused = match now[1] {
        SERVICE_STOPPED => Some(scm::ERROR_SERVICE_NOT_ACTIVE),
        SERVICE_START_PENDING | SERVICE_STOP_PENDING => Some(ERROR_SERVICE_CANNOT_ACCEPT_CTRL),
        _ if scm::built_in(name) => {
            (code != SERVICE_CONTROL_INTERROGATE).then_some(scm::ERROR_INVALID_SERVICE_CONTROL)
        }
        _ => {
            let bit = accept_bit(code);
            (bit != 0 && now[2] & bit == 0).then_some(scm::ERROR_INVALID_SERVICE_CONTROL)
        }
    };
    if let Some(e) = refused {
        return (Err(win32(e)), now);
    }
    if scm::built_in(name) {
        return (Ok(()), now);
    }
    let mut request = Vec::new();
    let mut put = || -> Result<(), NtStatus> {
        for v in [REQUEST_CONTROL, code, 0, kind, name.len() as u32, 0, 0] {
            request.try_extend_from_slice(&v.to_le_bytes()).map_err(nomem)?;
        }
        for &u in name {
            request.try_extend_from_slice(&u.to_le_bytes()).map_err(nomem)?;
        }
        Ok(())
    };
    if let Err(e) = put() {
        return (Err(e), now);
    }
    let result = exchange(name, request);
    if let Ok(answer) = result
        && answer != 0
        && log_allowed()
    {
        info!(
            "scm: service \"{}\": its handler answered control {code} with {answer:#x}",
            Plain(name)
        );
    }
    (result.map(drop), status(name, kind))
}

/// The run whose process is `pid`'s (the caller's): `f` on it under the
/// lock; ERROR_FAILED_SERVICE_CONTROLLER_CONNECT when the caller is no
/// service's process.
fn with_caller<R>(pid: u64, f: impl FnOnce(&mut Run) -> R) -> Result<R, u32> {
    let mut runs = RUNS.lock();
    let r = runs
        .iter_mut()
        .find(|r| r.process.as_ref().is_some_and(|p| p.id == pid))
        .ok_or(ERROR_FAILED_SERVICE_CONTROLLER_CONNECT)?;
    Ok(f(r))
}

/// StartServiceCtrlDispatcher's connection from process `pid`:
/// ERROR_FAILED_SERVICE_CONTROLLER_CONNECT when the SCM did not start it
/// as a service; ERROR_SERVICE_ALREADY_RUNNING on a second call.
pub fn connect(pid: u64) -> Result<(), u32> {
    let (name, signals) = with_caller(pid, |r| {
        if r.connected {
            return Err(scm::ERROR_SERVICE_ALREADY_RUNNING);
        }
        r.connected = true;
        Ok((scm::copy(&r.name).unwrap_or_default(), r.signals.clone()))
    })??;
    if log_allowed() {
        info!("scm: service \"{}\" (pid {pid}): its dispatcher connected", Plain(&name));
    }
    wait::set_signal(&signals.connected, 1);
    Ok(())
}

/// The dispatcher's next request (it waits for one): the record, or
/// REQUEST_RETURN alone once every service of the process has stopped.
pub fn next(pid: u64) -> Result<Vec<u8>, NtStatus> {
    loop {
        let taken = with_caller(pid, |r| {
            if r.returning {
                return (Some(None), r.signals.pending.clone());
            }
            (r.request.take().map(Some), r.signals.pending.clone())
        })
        .map_err(win32)?;
        match taken {
            (Some(Some(request)), _) => return Ok(request),
            (Some(None), _) => {
                let mut out = Vec::new();
                for v in [REQUEST_RETURN, 0, 0, 0, 0, 0, 0] {
                    out.try_extend_from_slice(&v.to_le_bytes()).map_err(nomem)?;
                }
                return Ok(out);
            }
            (None, pending) => {
                let status = wait::wait_for(&[pending], false, Timeout::Infinite);
                if status == NtStatus::THREAD_IS_TERMINATING {
                    return Err(status);
                }
            }
        }
    }
}

/// A request `next` gave that did not fit the dispatcher's buffer, back
/// in the slot for its next call.
pub fn put_back(pid: u64, request: Vec<u8>) {
    if let Ok(pending) = with_caller(pid, |r| {
        r.request = Some(request);
        r.signals.pending.clone()
    }) {
        wait::set_signal(&pending, 1);
    }
}

/// The dispatcher's answer to the request it took.
pub fn reply(pid: u64, value: u32) -> Result<(), u32> {
    let answered = with_caller(pid, |r| {
        r.reply = Some(value);
        r.signals.answered.clone()
    })?;
    wait::set_signal(&answered, 1);
    Ok(())
}

/// SetServiceStatus from process `pid`: SERVICE_STATUS's seven fields
/// (the type is the configuration's, kept). ERROR_INVALID_HANDLE when the
/// caller runs no service; ERROR_INVALID_DATA for a state that is not
/// one. The check point and wait hint are kept for the pending states
/// only (Windows reports 0 for a running service that gave a wait hint:
/// Wine's tests); a stopped service has no process (its process id 0)
/// and its dispatcher returns. A stopped service marked for deletion
/// goes once its handles are closed.
pub fn set_status(pid: u64, reported: [u32; 7]) -> Result<(), u32> {
    let state = reported[1];
    if !(SERVICE_STOPPED..=SERVICE_PAUSED).contains(&state) {
        return Err(ERROR_INVALID_DATA);
    }
    let changed = with_caller(pid, |r| {
        let before = r.status[1];
        if state == SERVICE_STOPPED {
            stop(r, reported[3], reported[4], "stopped");
            r.status[2] = reported[2];
        } else {
            let pending = matches!(
                state,
                SERVICE_START_PENDING
                    | SERVICE_STOP_PENDING
                    | SERVICE_CONTINUE_PENDING
                    | SERVICE_PAUSE_PENDING
            );
            r.status[1] = state;
            r.status[2] = reported[2];
            r.status[3] = reported[3];
            r.status[4] = reported[4];
            r.status[5] = if pending { reported[5] } else { 0 };
            r.status[6] = if pending { reported[6] } else { 0 };
            if before != state {
                emit("status", &r.name, &r.status);
            }
        }
        (before != state).then(|| (scm::copy(&r.name).unwrap_or_default(), r.status))
    })
    .map_err(|_| ERROR_INVALID_HANDLE)?;
    if let Some((name, s)) = changed {
        if log_allowed() {
            info!(
                "scm: service \"{}\" (pid {pid}) reports state {} (accepts {:#x}, exit {}, specific {})",
                Plain(&name),
                s[1],
                s[2],
                s[3],
                s[4]
            );
        }
        if s[1] == SERVICE_STOPPED {
            scm::stopped(&name);
        }
    }
    Ok(())
}

/// Process `process` has ended: a service it ran that had not stopped is
/// stopped with ERROR_PROCESS_ABORTED (as Windows reports a killed
/// service process: Wine's tests).
pub fn process_ended(process: &Process) {
    let ended = {
        let mut runs = RUNS.lock();
        let Some(r) =
            runs.iter_mut().find(|r| r.process.as_ref().is_some_and(|p| p.id == process.id))
        else {
            return;
        };
        r.process = None;
        r.connected = false;
        if r.status[1] == SERVICE_STOPPED {
            None
        } else {
            stop(r, ERROR_PROCESS_ABORTED, 0, "process ended");
            scm::copy(&r.name).ok()
        }
    };
    if let Some(name) = ended {
        warn!(
            "scm: service \"{}\": its process {} (pid {}) ended while it ran (ERROR_PROCESS_ABORTED)",
            Plain(&name),
            process.name,
            process.id
        );
        scm::stopped(&name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_need_their_accept_bits() {
        assert_eq!(accept_bit(SERVICE_CONTROL_STOP), 1);
        assert_eq!(accept_bit(2), 2);
        assert_eq!(accept_bit(3), 2);
        assert_eq!(accept_bit(SERVICE_CONTROL_INTERROGATE), 0);
        assert_eq!(accept_bit(128), 0);
    }
}
