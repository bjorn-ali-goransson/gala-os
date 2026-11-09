//! gala's rstrtmgr.dll: the Restart Manager (restartmanager.h), which
//! installers ask which processes and services use the files they are
//! about to replace, and to shut those down and restart them after.
//! Click-to-Run's repoman.dll asks it before staging an update
//! (RmStartSession, RmRegisterResources, RmGetList, RmEndSession);
//! OfficeClickToRun.exe also shuts down and restarts (RmAddFilter,
//! RmShutdown, RmRestart). Answers follow Microsoft's documentation of
//! each function (`rstrtmgr_test`; ARCHITECTURE "The Restart Manager").
//!
//! - Sessions live in the calling process: a handle is a small number,
//!   the session key 32 hexadecimal digits (CCH_RM_SESSION_KEY), at most
//!   64 sessions (ERROR_MAX_SESSIONS_REACHED). RmJoinSession joins a
//!   session of this process (a secondary handle: no filters); the key of
//!   another process's session is ERROR_SESSION_CREDENTIAL_CONFLICT,
//!   recorded (`rstrtmgr.dll!RmJoinSession(another process's session)`):
//!   Windows shares sessions between processes, gala does not yet.
//! - RmGetList finds the processes using a registered file through the
//!   kernel's FileProcessIdsUsingFileInformation (a handle open on it, a
//!   view of it mapped, a loaded DLL, the executable), the registered
//!   processes still running with the same start time, and the
//!   registered services running. A service's process is RmService
//!   (always restartable), a console program RmConsole, any other
//!   RmUnknownApp: gala does not look for its windows (recorded,
//!   `rstrtmgr.dll!RmGetList(application without window lookup)`), nor
//!   can it read another process's restart registration (not
//!   restartable). A registered directory is ERROR_ACCESS_DENIED. The
//!   caller itself in the list is RmRebootReasonDetectedSelf.
//! - RmShutdown stops listed services (ControlService, waiting the 20 s
//!   Windows gives a service before forcing) and, with RmForceShutdown,
//!   terminates listed applications; without it an application is not
//!   asked to close (no WM_QUERYENDSESSION or console close event from
//!   here: RmStatusErrorOnStop, ERROR_FAIL_SHUTDOWN, recorded
//!   `rstrtmgr.dll!RmShutdown(asking an application to close)`).
//!   RmRestart restarts the services it stopped; an empty list shuts
//!   down and restarts nothing and succeeds.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::ptr::{null, null_mut};

use gala_heap as _;

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

// The x64 build's memory routines come from gala's ntdll (as other
// DLLs on `alloc` take them).
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly (ntrt's
/// x86 runtime defines it on x86).
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

type Handle = usize;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
    fn NtQueryInformationFile(
        file: Handle,
        iosb: *mut [usize; 2],
        buffer: *mut u8,
        length: u32,
        class: u32,
    ) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn AcquireSRWLockExclusive(lock: *mut usize);
    fn ReleaseSRWLockExclusive(lock: *mut usize);
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: usize,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn GetProcessTimes(
        h: Handle,
        created: *mut [u32; 2],
        exited: *mut [u32; 2],
        kernel: *mut [u32; 2],
        user: *mut [u32; 2],
    ) -> i32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> i32;
    fn QueryFullProcessImageNameW(h: Handle, flags: u32, name: *mut u16, size: *mut u32) -> i32;
    fn ProcessIdToSessionId(pid: u32, session: *mut u32) -> i32;
    fn TerminateProcess(h: Handle, code: u32) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetCurrentProcessId() -> u32;
    fn Sleep(ms: u32);
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "advapi32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "advapi32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn OpenSCManagerW(machine: *const u16, database: *const u16, access: u32) -> Handle;
    fn OpenServiceW(scm: Handle, name: *const u16, access: u32) -> Handle;
    fn CloseServiceHandle(h: Handle) -> i32;
    fn ControlService(h: Handle, control: u32, status: *mut u32) -> i32;
    fn StartServiceW(h: Handle, argc: u32, argv: *const *const u16) -> i32;
    fn QueryServiceStatusEx(h: Handle, level: u32, buf: *mut u8, size: u32, need: *mut u32) -> i32;
    #[allow(clippy::too_many_arguments)]
    fn EnumServicesStatusExW(
        scm: Handle,
        level: u32,
        kinds: u32,
        states: u32,
        out: *mut u8,
        size: u32,
        needed: *mut u32,
        returned: *mut u32,
        resume: *mut u32,
        group: *const u16,
    ) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "bcryptprimitives", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "bcryptprimitives", kind = "raw-dylib"))]
unsafe extern "system" {
    fn ProcessPrng(data: *mut u8, length: usize) -> i32;
}

const ERROR_SUCCESS: u32 = 0;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_OUTOFMEMORY: u32 = 14;
const ERROR_BAD_ARGUMENTS: u32 = 160;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_FAIL_NOACTION_REBOOT: u32 = 350;
const ERROR_FAIL_SHUTDOWN: u32 = 351;
const ERROR_FAIL_RESTART: u32 = 352;
const ERROR_MAX_SESSIONS_REACHED: u32 = 353;
const ERROR_REQUEST_OUT_OF_SEQUENCE: u32 = 776;
const ERROR_SESSION_CREDENTIAL_CONFLICT: u32 = 1219;

/// CCH_RM_SESSION_KEY: a GUID's 16 bytes as hexadecimal digits.
const CCH_RM_SESSION_KEY: usize = 32;
/// "The system supports a maximum of 64 sessions" (per user on Windows;
/// per process here).
const MAX_SESSIONS: usize = 64;
/// Handles: each session's own and those RmJoinSession gives.
const MAX_HANDLES: usize = 2 * MAX_SESSIONS;
const CCH_RM_MAX_APP_NAME: usize = 255;
const CCH_RM_MAX_SVC_NAME: usize = 63;

// RM_APP_TYPE.
const RM_UNKNOWN_APP: u32 = 0;
const RM_SERVICE: u32 = 3;
const RM_CONSOLE: u32 = 5;
// RM_APP_STATUS.
const RM_STATUS_RUNNING: u32 = 1;
const RM_STATUS_STOPPED: u32 = 2;
const RM_STATUS_RESTARTED: u32 = 8;
const RM_STATUS_ERROR_ON_STOP: u32 = 0x10;
const RM_STATUS_ERROR_ON_RESTART: u32 = 0x20;
const RM_STATUS_SHUTDOWN_MASKED: u32 = 0x40;
const RM_STATUS_RESTART_MASKED: u32 = 0x80;
// RM_REBOOT_REASON.
const RM_REBOOT_REASON_DETECTED_SELF: u32 = 0x10;
// RM_SHUTDOWN_TYPE.
const RM_FORCE_SHUTDOWN: u32 = 0x1;
const RM_SHUTDOWN_ONLY_REGISTERED: u32 = 0x10;
// RM_FILTER_ACTION and RM_FILTER_TRIGGER.
const RM_NO_RESTART: u32 = 1;
const RM_NO_SHUTDOWN: u32 = 2;
const RM_FILTER_TRIGGER_FILE: u32 = 1;
const RM_FILTER_TRIGGER_PROCESS: u32 = 2;
const RM_FILTER_TRIGGER_SERVICE: u32 = 3;

/// FileProcessIdsUsingFileInformation (ntifs.h).
const FILE_PROCESS_IDS_USING_FILE_INFORMATION: u32 = 47;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xc000_0004_u32 as i32;
const INVALID_HANDLE_VALUE: Handle = usize::MAX;
const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_READ_ATTRIBUTES: u32 = 0x80;
const GENERIC_READ: u32 = 0x8000_0000;
const FILE_SHARE_ALL: u32 = 7;
const OPEN_EXISTING: u32 = 3;
const PROCESS_TERMINATE: u32 = 0x0001;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const SYNCHRONIZE: u32 = 0x0010_0000;
const STILL_ACTIVE: u32 = 259;
const INFINITE: u32 = u32::MAX;
const SC_MANAGER_CONNECT: u32 = 0x1;
const SC_MANAGER_ENUMERATE_SERVICE: u32 = 0x4;
const SERVICE_QUERY_STATUS: u32 = 0x4;
const SERVICE_START: u32 = 0x10;
const SERVICE_STOP: u32 = 0x20;
const SERVICE_CONTROL_STOP: u32 = 1;
const SERVICE_STOPPED: u32 = 1;
const SERVICE_WIN32: u32 = 0x30;
const SERVICE_ACTIVE: u32 = 1;
/// How long a service has to stop before RmForceShutdown ends it: "A
/// service that does not respond to a shutdown request is forced to shut
/// down after 20 seconds" (RmShutdown).
const SERVICE_STOP_WAIT_MS: u32 = 20_000;
const SERVICE_POLL_MS: u32 = 50;
/// The exit code of an application RmForceShutdown terminates.
const TERMINATED_EXIT_CODE: u32 = 1;

/// RM_UNIQUE_PROCESS: a process id and its start time.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct UniqueProcess {
    pid: u32,
    start: [u32; 2],
}

/// RM_PROCESS_INFO (668 bytes on both widths).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ProcessInfo {
    process: UniqueProcess,
    app_name: [u16; CCH_RM_MAX_APP_NAME + 1],
    service: [u16; CCH_RM_MAX_SVC_NAME + 1],
    app_type: u32,
    status: u32,
    ts_session: u32,
    restartable: i32,
}

const _: () = assert!(core::mem::size_of::<ProcessInfo>() == 668);

/// RM_FILTER_INFO's fixed part: FilterAction, FilterTrigger,
/// cbNextOffset, then the union of a string pointer and an
/// RM_UNIQUE_PROCESS, pointer-aligned (32 bytes on x64, 24 on x86).
#[repr(C)]
struct FilterInfo {
    action: u32,
    trigger: u32,
    next: u32,
    target: FilterTarget,
}

#[repr(C)]
union FilterTarget {
    name: *const u16,
    process: UniqueProcess,
}

/// RM_WRITE_STATUS_CALLBACK: the percentage done.
type StatusCallback = Option<extern "C" fn(u32)>;

/// What a filter names.
enum Target {
    File(Vec<u16>),
    Process(UniqueProcess),
    Service(Vec<u16>),
}

struct Filter {
    target: Target,
    action: u32,
}

/// One listed process (or service), with what filters and shutdown need.
struct Entry {
    info: ProcessInfo,
    /// The executable's full path (a module filter's match).
    image: Vec<u16>,
}

struct Session {
    key: [u16; CCH_RM_SESSION_KEY + 1],
    files: Vec<Vec<u16>>,
    apps: Vec<UniqueProcess>,
    services: Vec<Vec<u16>>,
    filters: Vec<Filter>,
    /// After RmShutdown: the list it worked on, with each one's status.
    shut: Option<Vec<Entry>>,
    /// RmRestart has run since that RmShutdown.
    restarted: bool,
}

#[derive(Clone, Copy)]
struct HandleSlot {
    session: usize,
    /// RmStartSession's handle (a primary installer); RmJoinSession's are
    /// secondary.
    primary: bool,
}

struct State {
    sessions: [Option<Session>; MAX_SESSIONS],
    handles: [Option<HandleSlot>; MAX_HANDLES],
}

struct Global {
    lock: UnsafeCell<usize>,
    state: UnsafeCell<State>,
}

// SAFETY: `state` is only reached under the SRW lock (`with_state`).
unsafe impl Sync for Global {}

static GLOBAL: Global = Global {
    lock: UnsafeCell::new(0),
    state: UnsafeCell::new(State {
        sessions: [const { None }; MAX_SESSIONS],
        handles: [None; MAX_HANDLES],
    }),
};

/// Runs `f` on the process's sessions, under their lock (never held
/// across a call out: status callbacks, the service control manager).
fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    // SAFETY: the SRW lock serializes every access to the state.
    unsafe {
        AcquireSRWLockExclusive(GLOBAL.lock.get());
        let r = f(&mut *GLOBAL.state.get());
        ReleaseSRWLockExclusive(GLOBAL.lock.get());
        r
    }
}

/// The handle's slot and session, or ERROR_INVALID_HANDLE.
fn session_of(state: &mut State, handle: u32) -> Result<(HandleSlot, &mut Session), u32> {
    let slot = state.handles.get(handle as usize).copied().flatten().ok_or(ERROR_INVALID_HANDLE)?;
    let session = state.sessions[slot.session].as_mut().ok_or(ERROR_INVALID_HANDLE)?;
    Ok((slot, session))
}

/// Records a gap in the ledger as `rstrtmgr.dll!<what>`, kind "other".
fn record(what: &[u8], detail: u64) {
    let mut text = [0u8; 96];
    let mut n = 0;
    for part in [&b"rstrtmgr.dll!"[..], what] {
        let take = part.len().min(text.len() - n);
        text[n..n + take].copy_from_slice(&part[..take]);
        n += take;
    }
    // SAFETY: a valid buffer; 5 is the ledger's "other".
    unsafe { GalaRecordMissing(text.as_ptr(), n, 5, detail) };
}

/// The caller's NUL-terminated string (without its NUL), or None for NULL.
fn wide_arg<'a>(p: *const u16) -> Option<&'a [u16]> {
    if p.is_null() {
        return None;
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        Some(core::slice::from_raw_parts(p, n))
    }
}

/// `s` with a NUL, in fallible memory.
fn owned(s: &[u16]) -> Option<Vec<u16>> {
    let mut v = Vec::new();
    v.try_reserve_exact(s.len() + 1).ok()?;
    v.extend_from_slice(s);
    v.push(0);
    Some(v)
}

/// Without its NUL.
fn text(v: &[u16]) -> &[u16] {
    v.strip_suffix(&[0]).unwrap_or(v)
}

fn push<T>(v: &mut Vec<T>, item: T) -> Result<(), u32> {
    v.try_reserve(1).map_err(|_| ERROR_OUTOFMEMORY)?;
    v.push(item);
    Ok(())
}

/// Case-insensitive equality (ASCII folding; paths and service names).
fn same(a: &[u16], b: &[u16]) -> bool {
    let fold = |c: u16| if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c };
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| fold(x) == fold(y))
}

/// Copies `s` into the fixed field `out`, truncated, NUL-terminated.
fn fill(out: &mut [u16], s: &[u16]) {
    let n = s.len().min(out.len() - 1);
    out[..n].copy_from_slice(&s[..n]);
    out[n] = 0;
}

// ---------------------------------------------------------------------
// Sessions.

/// RmStartSession(*pSessionHandle, dwSessionFlags (reserved),
/// strSessionKey[CCH_RM_SESSION_KEY + 1]): a new session, its handle and
/// key. ERROR_BAD_ARGUMENTS for a NULL pointer, ERROR_MAX_SESSIONS_REACHED
/// past 64 sessions.
#[unsafe(no_mangle)]
pub extern "system" fn RmStartSession(handle: *mut u32, _flags: u32, key: *mut u16) -> u32 {
    if handle.is_null() || key.is_null() {
        return ERROR_BAD_ARGUMENTS;
    }
    let mut new_key = [0u16; CCH_RM_SESSION_KEY + 1];
    let mut random = [0u8; CCH_RM_SESSION_KEY / 2];
    // SAFETY: our buffer.
    unsafe { ProcessPrng(random.as_mut_ptr(), random.len()) };
    for (i, b) in random.iter().enumerate() {
        new_key[2 * i] = u16::from(b"0123456789abcdef"[usize::from(b >> 4)]);
        new_key[2 * i + 1] = u16::from(b"0123456789abcdef"[usize::from(b & 0xf)]);
    }
    let r = with_state(|state| {
        let s =
            state.sessions.iter().position(Option::is_none).ok_or(ERROR_MAX_SESSIONS_REACHED)?;
        let h = state.handles.iter().position(Option::is_none).ok_or(ERROR_MAX_SESSIONS_REACHED)?;
        state.sessions[s] = Some(Session {
            key: new_key,
            files: Vec::new(),
            apps: Vec::new(),
            services: Vec::new(),
            filters: Vec::new(),
            shut: None,
            restarted: false,
        });
        state.handles[h] = Some(HandleSlot { session: s, primary: true });
        Ok(h as u32)
    });
    match r {
        Ok(h) => {
            // SAFETY: the caller's handle and key buffer (CCH_RM_SESSION_KEY
            // + 1 characters, as documented).
            unsafe {
                *handle = h;
                core::ptr::copy_nonoverlapping(new_key.as_ptr(), key, new_key.len());
            }
            ERROR_SUCCESS
        }
        Err(e) => e,
    }
}

/// RmJoinSession(*pSessionHandle, strSessionKey): a secondary handle on
/// the session with that key. Sessions are this process's (module
/// comment): another key is ERROR_SESSION_CREDENTIAL_CONFLICT ("the
/// session key cannot be validated"), recorded when it is a well-formed
/// key (another process's session).
#[unsafe(no_mangle)]
pub extern "system" fn RmJoinSession(handle: *mut u32, key: *const u16) -> u32 {
    let Some(key) = wide_arg(key) else { return ERROR_BAD_ARGUMENTS };
    if handle.is_null() {
        return ERROR_BAD_ARGUMENTS;
    }
    let r = with_state(|state| {
        let s = state
            .sessions
            .iter()
            .position(|s| s.as_ref().is_some_and(|s| same(text(&s.key), key)))
            .ok_or(ERROR_SESSION_CREDENTIAL_CONFLICT)?;
        let h = state.handles.iter().position(Option::is_none).ok_or(ERROR_MAX_SESSIONS_REACHED)?;
        state.handles[h] = Some(HandleSlot { session: s, primary: false });
        Ok(h as u32)
    });
    match r {
        Ok(h) => {
            // SAFETY: the caller's handle.
            unsafe { *handle = h };
            ERROR_SUCCESS
        }
        Err(e) => {
            let hex = key.iter().all(|&c| u8::try_from(c).is_ok_and(|b| b.is_ascii_hexdigit()));
            if e == ERROR_SESSION_CREDENTIAL_CONFLICT && key.len() == CCH_RM_SESSION_KEY && hex {
                record(b"RmJoinSession(another process's session)", 0);
            }
            e
        }
    }
}

/// RmEndSession(dwSessionHandle): a primary handle ends its session (and
/// the handles joined to it), a secondary one leaves it.
/// ERROR_INVALID_HANDLE for a handle that is not open.
#[unsafe(no_mangle)]
pub extern "system" fn RmEndSession(handle: u32) -> u32 {
    with_state(|state| {
        let slot = match session_of(state, handle) {
            Ok((slot, _)) => slot,
            Err(e) => return e,
        };
        state.handles[handle as usize] = None;
        if slot.primary {
            state.sessions[slot.session] = None;
            for h in state.handles.iter_mut() {
                if h.is_some_and(|h| h.session == slot.session) {
                    *h = None;
                }
            }
        }
        ERROR_SUCCESS
    })
}

/// RmRegisterResources(dwSessionHandle, nFiles, rgsFileNames[],
/// nApplications, rgApplications[], nServices, rgsServiceNames[]): adds
/// them to the session (each once). An array may be NULL only when its
/// count is 0, and no name may be NULL (ERROR_BAD_ARGUMENTS).
#[unsafe(no_mangle)]
pub extern "system" fn RmRegisterResources(
    handle: u32,
    file_count: u32,
    files: *const *const u16,
    app_count: u32,
    apps: *const UniqueProcess,
    service_count: u32,
    services: *const *const u16,
) -> u32 {
    if (file_count > 0 && files.is_null())
        || (app_count > 0 && apps.is_null())
        || (service_count > 0 && services.is_null())
    {
        return ERROR_BAD_ARGUMENTS;
    }
    let names = |list: *const *const u16, count: u32| -> Result<Vec<Vec<u16>>, u32> {
        let mut out = Vec::new();
        out.try_reserve(count as usize).map_err(|_| ERROR_OUTOFMEMORY)?;
        for i in 0..count as usize {
            // SAFETY: the caller's array of `count` strings.
            let name = wide_arg(unsafe { *list.add(i) }).ok_or(ERROR_BAD_ARGUMENTS)?;
            out.push(owned(name).ok_or(ERROR_OUTOFMEMORY)?);
        }
        Ok(out)
    };
    let (new_files, new_services) = match (names(files, file_count), names(services, service_count))
    {
        (Ok(f), Ok(s)) => (f, s),
        (Err(e), _) | (_, Err(e)) => return e,
    };
    with_state(|state| {
        let session = match session_of(state, handle) {
            Ok((_, s)) => s,
            Err(e) => return e,
        };
        for f in new_files {
            if !session.files.iter().any(|g| same(g, &f))
                && let Err(e) = push(&mut session.files, f)
            {
                return e;
            }
        }
        for s in new_services {
            if !session.services.iter().any(|g| same(g, &s))
                && let Err(e) = push(&mut session.services, s)
            {
                return e;
            }
        }
        for i in 0..app_count as usize {
            // SAFETY: the caller's array of `app_count` RM_UNIQUE_PROCESS.
            let app = unsafe { *apps.add(i) };
            if !session.apps.contains(&app)
                && let Err(e) = push(&mut session.apps, app)
            {
                return e;
            }
        }
        ERROR_SUCCESS
    })
}

// ---------------------------------------------------------------------
// What uses the registered resources.

/// A running service: its process, short and display names.
struct Service {
    pid: u32,
    name: Vec<u16>,
    display: Vec<u16>,
}

/// The running Win32 services (none when the service control manager
/// cannot be asked).
fn running_services() -> Vec<Service> {
    let mut out = Vec::new();
    // SAFETY: plain calls on our buffers; ENUM_SERVICE_STATUS_PROCESSW
    // entries are read within the buffer the call filled.
    unsafe {
        let scm = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE);
        if scm == 0 {
            return out;
        }
        let (mut needed, mut count, mut resume) = (0u32, 0u32, 0u32);
        EnumServicesStatusExW(
            scm,
            0,
            SERVICE_WIN32,
            SERVICE_ACTIVE,
            null_mut(),
            0,
            &mut needed,
            &mut count,
            &mut resume,
            null(),
        );
        let mut buf: Vec<usize> = Vec::new();
        let words = (needed as usize).div_ceil(core::mem::size_of::<usize>());
        if words > 0 && buf.try_reserve_exact(words).is_ok() {
            buf.resize(words, 0);
            resume = 0;
            if EnumServicesStatusExW(
                scm,
                0,
                SERVICE_WIN32,
                SERVICE_ACTIVE,
                buf.as_mut_ptr().cast(),
                (words * core::mem::size_of::<usize>()) as u32,
                &mut needed,
                &mut count,
                &mut resume,
                null(),
            ) != 0
            {
                // ENUM_SERVICE_STATUS_PROCESSW: two string pointers, then
                // SERVICE_STATUS_PROCESS (nine DWORDs, dwProcessId the
                // eighth), pointer-aligned.
                let ptr = core::mem::size_of::<usize>();
                let size = (2 * ptr + 36).next_multiple_of(ptr);
                let base = buf.as_ptr().cast::<u8>();
                for i in 0..count as usize {
                    let e = base.add(i * size);
                    let name = wide_arg(e.cast::<*const u16>().read_unaligned());
                    let display = wide_arg(e.add(ptr).cast::<*const u16>().read_unaligned());
                    let pid = e.add(2 * ptr + 28).cast::<u32>().read_unaligned();
                    if let (Some(name), Some(display)) = (name, display)
                        && pid != 0
                        && let (Some(name), Some(display)) = (owned(name), owned(display))
                        && out.try_reserve(1).is_ok()
                    {
                        out.push(Service { pid, name, display });
                    }
                }
            }
        }
        CloseServiceHandle(scm);
    }
    out
}

/// The ids of the processes using `path` (the kernel's
/// FileProcessIdsUsingFileInformation); none for a file that does not
/// exist or cannot be opened. A directory is ERROR_ACCESS_DENIED.
fn processes_using(path: &[u16], out: &mut Vec<u32>) -> Result<(), u32> {
    // SAFETY: a NUL-terminated path.
    let attributes = unsafe { GetFileAttributesW(path.as_ptr()) };
    if attributes == INVALID_FILE_ATTRIBUTES {
        return Ok(());
    }
    if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        return Err(ERROR_ACCESS_DENIED);
    }
    // SAFETY: a NUL-terminated path.
    let file = unsafe {
        CreateFileW(path.as_ptr(), FILE_READ_ATTRIBUTES, FILE_SHARE_ALL, 0, OPEN_EXISTING, 0, 0)
    };
    if file == INVALID_HANDLE_VALUE {
        return Ok(());
    }
    let mut buf: Vec<usize> = Vec::new();
    let mut words: usize = 16;
    let r = loop {
        if buf.try_reserve_exact(words.saturating_sub(buf.len())).is_err() {
            break Err(ERROR_OUTOFMEMORY);
        }
        buf.resize(words, 0);
        let mut iosb = [0usize; 2];
        // SAFETY: our handle and buffer of `words` ULONG_PTRs.
        let status = unsafe {
            NtQueryInformationFile(
                file,
                &mut iosb,
                buf.as_mut_ptr().cast(),
                (words * core::mem::size_of::<usize>()) as u32,
                FILE_PROCESS_IDS_USING_FILE_INFORMATION,
            )
        };
        if status == STATUS_INFO_LENGTH_MISMATCH {
            words *= 2;
            continue;
        }
        if status < 0 {
            record(b"RmGetList(FileProcessIdsUsingFileInformation)", status as u32 as u64);
            break Ok(());
        }
        // NumberOfProcessIdsInList (a ULONG), then the ids.
        let count = (buf[0] as u32) as usize;
        break buf.iter().skip(1).take(count).try_for_each(|&id| push(out, id as u32));
    };
    // SAFETY: our handle.
    unsafe { CloseHandle(file) };
    r
}

/// Whether the process at `image` (a Win32 path) is a console program:
/// its PE optional header's Subsystem is IMAGE_SUBSYSTEM_WINDOWS_CUI.
fn is_console(image: &[u16]) -> bool {
    // SAFETY: a NUL-terminated path; our buffer.
    unsafe {
        let f = CreateFileW(image.as_ptr(), GENERIC_READ, FILE_SHARE_ALL, 0, OPEN_EXISTING, 0, 0);
        if f == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut head = [0u8; 1024];
        let mut read = 0u32;
        let ok = ReadFile(f, head.as_mut_ptr(), head.len() as u32, &mut read, 0) != 0;
        CloseHandle(f);
        if !ok || read < 0x40 {
            return false;
        }
        let pe = u32::from_le_bytes([head[0x3c], head[0x3d], head[0x3e], head[0x3f]]) as usize;
        // "PE\0\0", the 20-byte file header, then Subsystem at offset 68
        // of the optional header (PE32 and PE32+ alike).
        let at = pe + 4 + 20 + 68;
        at + 2 <= read as usize && u16::from_le_bytes([head[at], head[at + 1]]) == 3
    }
}

/// The entry for process `pid` (as `service`, if it is one), or None if
/// it is not running any more.
fn describe(pid: u32, service: Option<&Service>) -> Result<Option<Entry>, u32> {
    // SAFETY: plain calls on our buffers and handle.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h == 0 {
            return Ok(None);
        }
        let (mut created, mut times) = ([0u32; 2], [[0u32; 2]; 3]);
        let mut code = 0u32;
        let running = GetExitCodeProcess(h, &mut code) != 0 && code == STILL_ACTIVE;
        let timed =
            GetProcessTimes(h, &mut created, &mut times[0], &mut times[1], &mut times[2]) != 0;
        let mut image = [0u16; 520];
        let mut size = image.len() as u32;
        let named = QueryFullProcessImageNameW(h, 0, image.as_mut_ptr(), &mut size) != 0;
        CloseHandle(h);
        if !running || !timed {
            return Ok(None);
        }
        let image = if named { &image[..size as usize] } else { &[][..] };
        let mut info = ProcessInfo {
            process: UniqueProcess { pid, start: created },
            app_name: [0; CCH_RM_MAX_APP_NAME + 1],
            service: [0; CCH_RM_MAX_SVC_NAME + 1],
            app_type: RM_UNKNOWN_APP,
            status: RM_STATUS_RUNNING,
            ts_session: 0,
            restartable: 0,
        };
        let mut ts = 0u32;
        if ProcessIdToSessionId(pid, &mut ts) != 0 {
            info.ts_session = ts;
        }
        let image_z = owned(image).ok_or(ERROR_OUTOFMEMORY)?;
        match service {
            Some(s) => {
                fill(&mut info.app_name, text(&s.display));
                fill(&mut info.service, text(&s.name));
                info.app_type = RM_SERVICE;
                info.restartable = 1;
            }
            None => {
                let base = image.iter().rposition(|&c| c == u16::from(b'\\')).map_or(0, |i| i + 1);
                fill(&mut info.app_name, &image[base..]);
                if is_console(&image_z) {
                    info.app_type = RM_CONSOLE;
                } else {
                    record(b"RmGetList(application without window lookup)", u64::from(pid));
                }
            }
        }
        Ok(Some(Entry { info, image: image_z }))
    }
}

/// A copy of what a session registered.
struct Registered {
    files: Vec<Vec<u16>>,
    apps: Vec<UniqueProcess>,
    services: Vec<Vec<u16>>,
}

fn copy_list(list: &[Vec<u16>]) -> Result<Vec<Vec<u16>>, u32> {
    let mut out = Vec::new();
    out.try_reserve_exact(list.len()).map_err(|_| ERROR_OUTOFMEMORY)?;
    for s in list {
        out.push(owned(text(s)).ok_or(ERROR_OUTOFMEMORY)?);
    }
    Ok(out)
}

fn registered(session: &Session) -> Result<Registered, u32> {
    let mut apps = Vec::new();
    apps.try_reserve_exact(session.apps.len()).map_err(|_| ERROR_OUTOFMEMORY)?;
    apps.extend_from_slice(&session.apps);
    Ok(Registered {
        files: copy_list(&session.files)?,
        apps,
        services: copy_list(&session.services)?,
    })
}

/// The processes and services using what `r` names, each once: those
/// using a registered file, the registered applications still running
/// with the start time given, the registered services running.
fn affected(r: &Registered) -> Result<Vec<Entry>, u32> {
    let services = running_services();
    // (pid, the service it is, the start time it must have).
    let mut found: Vec<(u32, Option<usize>, Option<[u32; 2]>)> = Vec::new();
    let mut pids = Vec::new();
    for f in &r.files {
        processes_using(f, &mut pids)?;
    }
    for pid in pids {
        push(&mut found, (pid, services.iter().position(|s| s.pid == pid), None))?;
    }
    for app in &r.apps {
        let service = services.iter().position(|s| s.pid == app.pid);
        push(&mut found, (app.pid, service, Some(app.start)))?;
    }
    for name in &r.services {
        if let Some(i) = services.iter().position(|s| same(text(&s.name), text(name))) {
            push(&mut found, (services[i].pid, Some(i), None))?;
        }
    }
    let mut out: Vec<Entry> = Vec::new();
    for (pid, service, start) in found {
        let service = service.map(|i| &services[i]);
        let duplicate = out.iter().any(|e| {
            e.info.process.pid == pid
                && service.is_none_or(|s| same(text(&s.name), text_of(&e.info.service)))
        });
        if duplicate {
            continue;
        }
        let Some(entry) = describe(pid, service)? else { continue };
        if start.is_none_or(|t| t == entry.info.process.start) {
            push(&mut out, entry)?;
        }
    }
    Ok(out)
}

/// RmRebootReasonDetectedSelf when the caller is among `list`.
fn reboot_reasons(list: &[Entry]) -> u32 {
    // SAFETY: a plain call.
    let me = unsafe { GetCurrentProcessId() };
    if list.iter().any(|e| e.info.process.pid == me) { RM_REBOOT_REASON_DETECTED_SELF } else { 0 }
}

/// The session's list: what its RmShutdown worked on, with the statuses
/// since; else what uses its resources now.
fn current_list(handle: u32) -> Result<Vec<Entry>, u32> {
    let r = with_state(|state| {
        let (_, session) = session_of(state, handle)?;
        if let Some(shut) = &session.shut {
            let mut copy = Vec::new();
            copy.try_reserve_exact(shut.len()).map_err(|_| ERROR_OUTOFMEMORY)?;
            for e in shut {
                copy.push(Entry {
                    info: e.info,
                    image: owned(text(&e.image)).ok_or(ERROR_OUTOFMEMORY)?,
                });
            }
            return Ok(Err(copy));
        }
        registered(session).map(Ok)
    })?;
    match r {
        Ok(registered) => affected(&registered),
        Err(shut) => Ok(shut),
    }
}

/// RmGetList(dwSessionHandle, *pnProcInfoNeeded, *pnProcInfo,
/// rgAffectedApps[], *lpdwRebootReasons): the processes and services
/// using the session's resources. ERROR_MORE_DATA when more than
/// *pnProcInfo (with *pnProcInfoNeeded the count); ERROR_BAD_ARGUMENTS for
/// a NULL count or reasons pointer, or no array for a count above 0.
#[unsafe(no_mangle)]
pub extern "system" fn RmGetList(
    handle: u32,
    needed: *mut u32,
    count: *mut u32,
    apps: *mut ProcessInfo,
    reasons: *mut u32,
) -> u32 {
    if needed.is_null() || count.is_null() || reasons.is_null() {
        return ERROR_BAD_ARGUMENTS;
    }
    // SAFETY: the caller's count.
    let room = unsafe { *count } as usize;
    if room > 0 && apps.is_null() {
        return ERROR_BAD_ARGUMENTS;
    }
    let list = match current_list(handle) {
        Ok(l) => l,
        Err(e) => return e,
    };
    // SAFETY: the caller's counters and array of `room` entries.
    unsafe {
        *needed = list.len() as u32;
        *reasons = reboot_reasons(&list);
        if list.len() > room {
            *count = 0;
            return ERROR_MORE_DATA;
        }
        for (i, e) in list.iter().enumerate() {
            apps.add(i).write(e.info);
        }
        *count = list.len() as u32;
    }
    ERROR_SUCCESS
}

// ---------------------------------------------------------------------
// Filters.

/// The one target named by exactly one of the three, or None.
fn target_of(
    module: *const u16,
    process: *const UniqueProcess,
    service: *const u16,
) -> Option<Result<Target, u32>> {
    let given = [!module.is_null(), !process.is_null(), !service.is_null()];
    if given.iter().filter(|&&g| g).count() != 1 {
        return None;
    }
    Some(if let Some(m) = wide_arg(module) {
        owned(m).map(Target::File).ok_or(ERROR_OUTOFMEMORY)
    } else if let Some(s) = wide_arg(service) {
        owned(s).map(Target::Service).ok_or(ERROR_OUTOFMEMORY)
    } else {
        // SAFETY: the caller's RM_UNIQUE_PROCESS.
        Ok(Target::Process(unsafe { *process }))
    })
}

fn same_target(a: &Target, b: &Target) -> bool {
    match (a, b) {
        (Target::File(x), Target::File(y)) | (Target::Service(x), Target::Service(y)) => {
            same(text(x), text(y))
        }
        (Target::Process(x), Target::Process(y)) => x == y,
        _ => false,
    }
}

/// RmAddFilter(dwSessionHandle, strModuleName, pProcess,
/// strServiceShortName, FilterAction): keeps RmNoRestart or RmNoShutdown
/// for exactly one application (by path or RM_UNIQUE_PROCESS) or service
/// (ERROR_BAD_ARGUMENTS otherwise), replacing an earlier filter on it.
/// Only a primary installer filters (ERROR_SESSION_CREDENTIAL_CONFLICT).
#[unsafe(no_mangle)]
pub extern "system" fn RmAddFilter(
    handle: u32,
    module: *const u16,
    process: *const UniqueProcess,
    service: *const u16,
    action: u32,
) -> u32 {
    let target = match target_of(module, process, service) {
        Some(Ok(t)) if action == RM_NO_RESTART || action == RM_NO_SHUTDOWN => t,
        Some(Err(e)) => return e,
        _ => {
            return with_state(|state| match session_of(state, handle) {
                Ok(_) => ERROR_BAD_ARGUMENTS,
                Err(e) => e,
            });
        }
    };
    with_state(|state| {
        let session = match session_of(state, handle) {
            Ok((slot, _)) if !slot.primary => return ERROR_SESSION_CREDENTIAL_CONFLICT,
            Ok((_, s)) => s,
            Err(e) => return e,
        };
        if let Some(f) = session.filters.iter_mut().find(|f| same_target(&f.target, &target)) {
            f.action = action;
            return ERROR_SUCCESS;
        }
        match push(&mut session.filters, Filter { target, action }) {
            Ok(()) => ERROR_SUCCESS,
            Err(e) => e,
        }
    })
}

/// RmRemoveFilter(dwSessionHandle, strModuleName, pProcess,
/// strServiceShortName): removes the filter on that one target;
/// ERROR_FILE_NOT_FOUND when there is none.
#[unsafe(no_mangle)]
pub extern "system" fn RmRemoveFilter(
    handle: u32,
    module: *const u16,
    process: *const UniqueProcess,
    service: *const u16,
) -> u32 {
    let target = match target_of(module, process, service) {
        Some(Ok(t)) => Some(t),
        Some(Err(e)) => return e,
        None => None,
    };
    with_state(|state| {
        let session = match session_of(state, handle) {
            Ok((slot, _)) if !slot.primary => return ERROR_SESSION_CREDENTIAL_CONFLICT,
            Ok((_, s)) => s,
            Err(e) => return e,
        };
        let Some(target) = target else { return ERROR_BAD_ARGUMENTS };
        match session.filters.iter().position(|f| same_target(&f.target, &target)) {
            Some(i) => {
                session.filters.remove(i);
                ERROR_SUCCESS
            }
            None => ERROR_FILE_NOT_FOUND,
        }
    })
}

/// RmGetFilterList(dwSessionHandle, pbFilterBuf, cbFilterBuf,
/// *cbFilterBufNeeded): the filters as RM_FILTER_INFO structures one after
/// another (cbNextOffset to the next, 0 on the last), the names they point
/// to after them. ERROR_MORE_DATA when they do not fit or the size
/// pointer is NULL.
#[unsafe(no_mangle)]
pub extern "system" fn RmGetFilterList(
    handle: u32,
    buf: *mut u8,
    size: u32,
    needed: *mut u32,
) -> u32 {
    with_state(|state| {
        let session = match session_of(state, handle) {
            Ok((slot, _)) if !slot.primary => return ERROR_SESSION_CREDENTIAL_CONFLICT,
            Ok((_, s)) => s,
            Err(e) => return e,
        };
        let fixed = core::mem::size_of::<FilterInfo>();
        let names: usize = session
            .filters
            .iter()
            .map(|f| match &f.target {
                Target::File(s) | Target::Service(s) => 2 * s.len(),
                Target::Process(_) => 0,
            })
            .sum();
        let total = fixed * session.filters.len() + names;
        if needed.is_null() {
            return ERROR_MORE_DATA;
        }
        // SAFETY: the caller's size.
        unsafe { *needed = total as u32 };
        if total > 0 && (buf.is_null() || (size as usize) < total) {
            return ERROR_MORE_DATA;
        }
        let mut name_at = fixed * session.filters.len();
        let last = session.filters.len().saturating_sub(1);
        for (i, f) in session.filters.iter().enumerate() {
            let (trigger, target) = match &f.target {
                Target::File(s) | Target::Service(s) => {
                    // SAFETY: inside the caller's `total` bytes.
                    let dst = unsafe { buf.add(name_at) }.cast::<u16>();
                    // SAFETY: as above; `s` includes its NUL.
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            s.as_ptr().cast::<u8>(),
                            dst.cast(),
                            2 * s.len(),
                        )
                    };
                    name_at += 2 * s.len();
                    let trigger = if matches!(f.target, Target::File(_)) {
                        RM_FILTER_TRIGGER_FILE
                    } else {
                        RM_FILTER_TRIGGER_SERVICE
                    };
                    (trigger, FilterTarget { name: dst })
                }
                Target::Process(p) => (RM_FILTER_TRIGGER_PROCESS, FilterTarget { process: *p }),
            };
            let info = FilterInfo {
                action: f.action,
                trigger,
                next: if i == last { 0 } else { fixed as u32 },
                target,
            };
            // SAFETY: inside the caller's buffer; unaligned is allowed.
            unsafe { buf.add(i * fixed).cast::<FilterInfo>().write_unaligned(info) };
        }
        ERROR_SUCCESS
    })
}

/// The filter action that applies to `e` (0: none).
fn filter_for(filters: &[Filter], e: &Entry) -> u32 {
    filters
        .iter()
        .find(|f| match &f.target {
            Target::File(path) => same(text(path), text(&e.image)),
            Target::Process(p) => *p == e.info.process,
            Target::Service(name) => same(text(name), text_of(&e.info.service)),
        })
        .map_or(0, |f| f.action)
}

/// A fixed field's text, up to its NUL.
fn text_of(field: &[u16]) -> &[u16] {
    &field[..field.iter().position(|&c| c == 0).unwrap_or(field.len())]
}

// ---------------------------------------------------------------------
// Shutdown and restart.

/// Stops service `name` (running in `pid`); with `force`, a service that
/// has not stopped within SERVICE_STOP_WAIT_MS is ended with its process.
fn stop_service(name: &[u16], pid: u32, force: bool) -> bool {
    let Some(name) = owned(name) else { return false };
    // SAFETY: plain calls on our handles and buffers.
    unsafe {
        let scm = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT);
        if scm == 0 {
            return false;
        }
        let svc = OpenServiceW(scm, name.as_ptr(), SERVICE_STOP | SERVICE_QUERY_STATUS);
        let mut stopped = false;
        if svc != 0 {
            let mut status = [0u32; 7];
            ControlService(svc, SERVICE_CONTROL_STOP, status.as_mut_ptr());
            let mut waited = 0;
            loop {
                let mut ssp = [0u32; 9];
                let mut need = 0u32;
                let ok = QueryServiceStatusEx(svc, 0, ssp.as_mut_ptr().cast(), 36, &mut need) != 0;
                if !ok || ssp[1] == SERVICE_STOPPED {
                    stopped = ok;
                    break;
                }
                if waited >= SERVICE_STOP_WAIT_MS {
                    break;
                }
                Sleep(SERVICE_POLL_MS);
                waited += SERVICE_POLL_MS;
            }
            CloseServiceHandle(svc);
        }
        CloseServiceHandle(scm);
        if !stopped && force {
            stopped = terminate(pid);
        }
        stopped
    }
}

/// Starts service `name` again.
fn start_service(name: &[u16]) -> bool {
    let Some(name) = owned(name) else { return false };
    // SAFETY: plain calls on our handles.
    unsafe {
        let scm = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT);
        if scm == 0 {
            return false;
        }
        let svc = OpenServiceW(scm, name.as_ptr(), SERVICE_START);
        let ok = svc != 0 && StartServiceW(svc, 0, null()) != 0;
        if svc != 0 {
            CloseServiceHandle(svc);
        }
        CloseServiceHandle(scm);
        ok
    }
}

/// Ends process `pid` and waits until it has.
fn terminate(pid: u32) -> bool {
    // SAFETY: plain calls on our handle.
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE, 0, pid);
        if h == 0 {
            return false;
        }
        let ok = TerminateProcess(h, TERMINATED_EXIT_CODE) != 0;
        if ok {
            WaitForSingleObject(h, INFINITE);
        }
        CloseHandle(h);
        ok
    }
}

/// RmShutdown(dwSessionHandle, lActionFlags, fnStatus): shuts down what
/// uses the session's resources (module comment). An empty list
/// succeeds. ERROR_FAIL_NOACTION_REBOOT, with nothing done, when the
/// caller itself is listed; with RmShutdownOnlyRegistered,
/// ERROR_FAIL_SHUTDOWN with nothing done when one is not restartable;
/// ERROR_FAIL_SHUTDOWN when one could not be shut down.
#[unsafe(no_mangle)]
pub extern "system" fn RmShutdown(handle: u32, flags: u32, status: StatusCallback) -> u32 {
    let filters_and_resources = with_state(|state| {
        let (_, session) = session_of(state, handle)?;
        let mut filters = Vec::new();
        filters.try_reserve_exact(session.filters.len()).map_err(|_| ERROR_OUTOFMEMORY)?;
        for f in &session.filters {
            let target = match &f.target {
                Target::File(s) => Target::File(owned(text(s)).ok_or(ERROR_OUTOFMEMORY)?),
                Target::Service(s) => Target::Service(owned(text(s)).ok_or(ERROR_OUTOFMEMORY)?),
                Target::Process(p) => Target::Process(*p),
            };
            filters.push(Filter { target, action: f.action });
        }
        Ok((filters, registered(session)?))
    });
    let (filters, registered) = match filters_and_resources {
        Ok(v) => v,
        Err(e) => return e,
    };
    let mut list = match affected(&registered) {
        Ok(l) => l,
        Err(e) => return e,
    };
    if reboot_reasons(&list) != 0 {
        return ERROR_FAIL_NOACTION_REBOOT;
    }
    for e in list.iter_mut() {
        match filter_for(&filters, e) {
            RM_NO_SHUTDOWN => e.info.status |= RM_STATUS_SHUTDOWN_MASKED,
            RM_NO_RESTART => e.info.status |= RM_STATUS_RESTART_MASKED,
            _ => {}
        }
    }
    let to_stop = |e: &Entry| e.info.status & RM_STATUS_SHUTDOWN_MASKED == 0;
    if flags & RM_SHUTDOWN_ONLY_REGISTERED != 0
        && list.iter().any(|e| to_stop(e) && e.info.restartable == 0)
    {
        return ERROR_FAIL_SHUTDOWN;
    }
    let force = flags & RM_FORCE_SHUTDOWN != 0;
    let mut failed = false;
    let total = list.len();
    for (i, e) in list.iter_mut().enumerate() {
        if to_stop(e) {
            let pid = e.info.process.pid;
            let stopped = if e.info.app_type == RM_SERVICE {
                stop_service(text_of(&e.info.service), pid, force)
            } else if force {
                record(b"RmShutdown(terminated without asking first)", u64::from(pid));
                terminate(pid)
            } else {
                record(b"RmShutdown(asking an application to close)", u64::from(pid));
                false
            };
            e.info.status &= !RM_STATUS_RUNNING;
            e.info.status |= if stopped { RM_STATUS_STOPPED } else { RM_STATUS_ERROR_ON_STOP };
            failed |= !stopped;
        }
        if let Some(cb) = status {
            cb(((i + 1) * 100 / total) as u32);
        }
    }
    if total == 0
        && let Some(cb) = status
    {
        cb(100);
    }
    let stored = with_state(|state| {
        let (_, session) = session_of(state, handle)?;
        session.shut = Some(list);
        session.restarted = false;
        Ok(())
    });
    match stored {
        Err(e) => e,
        Ok(()) if failed => ERROR_FAIL_SHUTDOWN,
        Ok(()) => ERROR_SUCCESS,
    }
}

/// RmRestart(dwSessionHandle, dwRestartFlags (reserved), fnStatus):
/// restarts the services RmShutdown stopped (not those filtered with
/// RmNoRestart); applications gala cannot restart (none is restartable)
/// stay stopped. ERROR_REQUEST_OUT_OF_SEQUENCE without an RmShutdown
/// first; ERROR_FAIL_RESTART when a service did not start.
#[unsafe(no_mangle)]
pub extern "system" fn RmRestart(handle: u32, _flags: u32, status: StatusCallback) -> u32 {
    let taken = with_state(|state| {
        let (_, session) = session_of(state, handle)?;
        if session.restarted {
            return Err(ERROR_REQUEST_OUT_OF_SEQUENCE);
        }
        session.shut.take().ok_or(ERROR_REQUEST_OUT_OF_SEQUENCE)
    });
    let mut list = match taken {
        Ok(l) => l,
        Err(e) => return e,
    };
    let mut failed = false;
    for e in list.iter_mut() {
        let restart = e.info.app_type == RM_SERVICE
            && e.info.status & RM_STATUS_STOPPED != 0
            && e.info.status & RM_STATUS_RESTART_MASKED == 0;
        if restart {
            if start_service(text_of(&e.info.service)) {
                e.info.status = (e.info.status & !RM_STATUS_STOPPED) | RM_STATUS_RESTARTED;
            } else {
                e.info.status |= RM_STATUS_ERROR_ON_RESTART;
                failed = true;
            }
        }
    }
    if let Some(cb) = status {
        cb(100);
    }
    let stored = with_state(|state| {
        let (_, session) = session_of(state, handle)?;
        session.shut = Some(list);
        session.restarted = true;
        Ok(())
    });
    match stored {
        Err(e) => e,
        Ok(()) if failed => ERROR_FAIL_RESTART,
        Ok(()) => ERROR_SUCCESS,
    }
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
