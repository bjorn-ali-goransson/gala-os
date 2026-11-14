//! The Restart Manager (rstrtmgr.dll) as installers use it: Click-to-Run's
//! repoman.dll and OfficeClickToRun.exe (ROADMAP OF). Expected values are
//! those Microsoft's documentation of each function states (Wine has no
//! conformance tests for it); what the documentation leaves open is
//! printed as `rstrtmgr_test: info ...` lines for a run on Windows to
//! show, not checked:
//! - RmStartSession: ERROR_BAD_ARGUMENTS for a NULL handle or key
//!   pointer; a key of CCH_RM_SESSION_KEY (32) hexadecimal digits and a
//!   NUL; two sessions differ in handle and key; at most 64 sessions
//!   (ERROR_MAX_SESSIONS_REACHED; other sessions of the user may take
//!   some on Windows);
//! - RmEndSession: ERROR_INVALID_HANDLE for a session ended or never
//!   started; the other functions too;
//! - RmRegisterResources: an array NULL with a count above 0 is
//!   ERROR_BAD_ARGUMENTS;
//! - RmGetList: ERROR_BAD_ARGUMENTS for a NULL count or reasons pointer;
//!   files nobody uses (and one that does not exist) list nothing,
//!   RmRebootReasonNone; a registered directory is ERROR_ACCESS_DENIED;
//!   a file this process holds open lists this process (ERROR_MORE_DATA
//!   for no room, with the count needed), its start time, RmStatusRunning
//!   and its Terminal Services session; a file a child holds open lists
//!   the child (QueryFullProcessImageNameW of the child: this program's
//!   path), a console program (RmConsole), not restartable (never
//!   registered for restart), as does the child's RM_UNIQUE_PROCESS
//!   registered, but not with another start time;
//! - RmShutdown of an empty list succeeds; with RmForceShutdown it ends
//!   the child, whose status is then RmStatusStopped; RmRestart before
//!   RmShutdown is ERROR_REQUEST_OUT_OF_SEQUENCE, after it succeeds;
//! - RmAddFilter: ERROR_BAD_ARGUMENTS unless exactly one of the module,
//!   process and service is given, or for RmInvalidFilterAction;
//!   RmGetFilterList gives the filters (ERROR_MORE_DATA without a buffer,
//!   the size needed), RmRemoveFilter removes one, then
//!   ERROR_FILE_NOT_FOUND.
//!
//! The same source builds `wow64_rstrtmgr_test`, the 32-bit (x86)
//! program (32-bit installers: Inno Setup's). Prints `rstrtmgr_test: ok`
//! (`wow64_rstrtmgr_test: ok`), or `<name>: check N got X (line L)` and
//! exits with N. Started as `rstrtmgr_test hold <path>` it is the
//! child: it opens the file, says so and waits to be ended.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::{null, null_mut};
use core::sync::atomic::{AtomicU32, Ordering};

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "rstrtmgr_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_rstrtmgr_test";

type Handle = usize;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct UniqueProcess {
    pid: u32,
    start: [u32; 2],
}

/// RM_PROCESS_INFO.
#[repr(C)]
#[derive(Clone, Copy)]
struct ProcessInfo {
    process: UniqueProcess,
    app_name: [u16; 256],
    service: [u16; 64],
    app_type: u32,
    status: u32,
    ts_session: u32,
    restartable: i32,
}

const ZERO_INFO: ProcessInfo = ProcessInfo {
    process: UniqueProcess { pid: 0, start: [0; 2] },
    app_name: [0; 256],
    service: [0; 64],
    app_type: 0,
    status: 0,
    ts_session: 0,
    restartable: 0,
};

#[repr(C)]
struct StartupInfo {
    cb: u32,
    rest: [u8; 100],
}

#[repr(C)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    pid: u32,
    tid: u32,
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetLastError() -> u32;
    fn GetCommandLineW() -> *const u16;
    fn GetModuleFileNameW(module: usize, name: *mut u16, size: u32) -> u32;
    fn GetTempPathW(size: u32, buffer: *mut u16) -> u32;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: usize,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CreateDirectoryW(path: *const u16, security: usize) -> i32;
    fn RemoveDirectoryW(path: *const u16) -> i32;
    fn DeleteFileW(path: *const u16) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn GetCurrentProcess() -> Handle;
    fn GetCurrentProcessId() -> u32;
    fn GetProcessTimes(
        h: Handle,
        created: *mut [u32; 2],
        exited: *mut [u32; 2],
        kernel: *mut [u32; 2],
        user: *mut [u32; 2],
    ) -> i32;
    fn ProcessIdToSessionId(pid: u32, session: *mut u32) -> i32;
    fn CreateProcessW(
        application: *const u16,
        command_line: *mut u16,
        process_attributes: *const c_void,
        thread_attributes: *const c_void,
        inherit: i32,
        flags: u32,
        environment: *const c_void,
        directory: *const u16,
        startup: *const StartupInfo,
        info: *mut ProcessInformation,
    ) -> i32;
    fn CreateEventW(security: usize, manual: i32, initial: i32, name: *const u16) -> Handle;
    fn SetEvent(h: Handle) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn WaitForMultipleObjects(count: u32, handles: *const Handle, all: i32, ms: u32) -> u32;
    fn TerminateProcess(h: Handle, code: u32) -> i32;
    fn Sleep(ms: u32);
    fn QueryFullProcessImageNameW(h: Handle, flags: u32, name: *mut u16, size: *mut u32) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "rstrtmgr", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "rstrtmgr", kind = "raw-dylib"))]
unsafe extern "system" {
    fn RmStartSession(handle: *mut u32, flags: u32, key: *mut u16) -> u32;
    fn RmJoinSession(handle: *mut u32, key: *const u16) -> u32;
    fn RmEndSession(handle: u32) -> u32;
    fn RmRegisterResources(
        handle: u32,
        file_count: u32,
        files: *const *const u16,
        app_count: u32,
        apps: *const UniqueProcess,
        service_count: u32,
        services: *const *const u16,
    ) -> u32;
    fn RmGetList(
        handle: u32,
        needed: *mut u32,
        count: *mut u32,
        apps: *mut ProcessInfo,
        reasons: *mut u32,
    ) -> u32;
    fn RmShutdown(handle: u32, flags: u32, status: Option<extern "C" fn(u32)>) -> u32;
    fn RmRestart(handle: u32, flags: u32, status: Option<extern "C" fn(u32)>) -> u32;
    fn RmAddFilter(
        handle: u32,
        module: *const u16,
        process: *const UniqueProcess,
        service: *const u16,
        action: u32,
    ) -> u32;
    fn RmRemoveFilter(
        handle: u32,
        module: *const u16,
        process: *const UniqueProcess,
        service: *const u16,
    ) -> u32;
    fn RmGetFilterList(handle: u32, buf: *mut u8, size: u32, needed: *mut u32) -> u32;
}

const ERROR_SUCCESS: u32 = 0;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_BAD_ARGUMENTS: u32 = 160;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_MAX_SESSIONS_REACHED: u32 = 353;
const ERROR_REQUEST_OUT_OF_SEQUENCE: u32 = 776;
const ERROR_SESSION_CREDENTIAL_CONFLICT: u32 = 1219;

const CCH_RM_SESSION_KEY: usize = 32;
const RM_CONSOLE: u32 = 5;
const RM_STATUS_RUNNING: u32 = 1;
const RM_STATUS_STOPPED: u32 = 2;
const RM_FORCE_SHUTDOWN: u32 = 1;
const RM_NO_RESTART: u32 = 1;
const RM_NO_SHUTDOWN: u32 = 2;
const RM_FILTER_TRIGGER_FILE: u32 = 1;
const RM_FILTER_TRIGGER_SERVICE: u32 = 3;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_ALL: u32 = 7;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const INVALID_HANDLE_VALUE: Handle = usize::MAX;
const INFINITE: u32 = u32::MAX;
const WAIT_OBJECT_0: u32 = 0;
/// A handle no session has.
const NOT_A_SESSION: u32 = 0x7fff_fff0;

struct Line {
    buf: [u8; 600],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 600], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn w(mut self, s: &[u16]) -> Self {
        for &c in s.iter().take_while(|&&c| c != 0) {
            let b = if c < 0x80 { c as u8 } else { b'?' };
            self = self.s(&[b]);
        }
        self
    }

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: our buffer; the standard output handle.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            )
        };
    }
}

fn info() -> Line {
    Line::new().s(NAME.as_bytes()).s(b": info ")
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u32, want: u32) -> Result<(), u32> {
        self.ok(got == want, got.into())
    }
}

/// A NUL-terminated UTF-16 path built from parts.
struct Path {
    buf: [u16; 400],
    n: usize,
}

impl Path {
    fn temp() -> Path {
        let mut p = Path { buf: [0; 400], n: 0 };
        // SAFETY: our buffer of 260 characters.
        p.n = unsafe { GetTempPathW(260, p.buf.as_mut_ptr()) } as usize;
        p
    }

    fn push(mut self, s: &str) -> Path {
        for b in s.bytes() {
            self.buf[self.n] = b.into();
            self.n += 1;
        }
        self.buf[self.n] = 0;
        self
    }

    fn ptr(&self) -> *const u16 {
        self.buf.as_ptr()
    }

    fn text(&self) -> &[u16] {
        &self.buf[..self.n]
    }
}

fn wide<const N: usize>(text: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (slot, b) in out.iter_mut().zip(text.bytes()) {
        *slot = u16::from(b);
    }
    out
}

/// Text up to a NUL.
fn until_nul(s: &[u16]) -> &[u16] {
    &s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())]
}

/// Creates (or truncates) `path` and closes it.
fn make_file(path: &Path) -> bool {
    // SAFETY: a NUL-terminated path.
    let h =
        unsafe { CreateFileW(path.ptr(), GENERIC_WRITE, FILE_SHARE_ALL, 0, CREATE_ALWAYS, 0, 0) };
    if h == INVALID_HANDLE_VALUE {
        return false;
    }
    // SAFETY: our handle.
    unsafe { CloseHandle(h) };
    true
}

/// This process's start time.
fn own_start() -> [u32; 2] {
    let (mut c, mut t) = ([0u32; 2], [[0u32; 2]; 3]);
    // SAFETY: our buffers.
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut t[0], &mut t[1], &mut t[2]) };
    c
}

fn start_session(c: &mut Checks) -> Result<u32, u32> {
    let mut h = 0u32;
    let mut key = [0u16; CCH_RM_SESSION_KEY + 1];
    // SAFETY: our handle and key buffer.
    c.eq(unsafe { RmStartSession(&mut h, 0, key.as_mut_ptr()) }, ERROR_SUCCESS)?;
    Ok(h)
}

/// RmGetList into `out` (room for its length); the result, the count
/// filled and the reboot reasons.
fn get_list(h: u32, out: &mut [ProcessInfo]) -> (u32, u32, u32, u32) {
    let (mut needed, mut count, mut reasons) = (0u32, out.len() as u32, 0u32);
    let apps = if out.is_empty() { null_mut() } else { out.as_mut_ptr() };
    // SAFETY: our counters and array.
    let r = unsafe { RmGetList(h, &mut needed, &mut count, apps, &mut reasons) };
    (r, needed, count, reasons)
}

fn print_entry(what: &str, e: &ProcessInfo, reasons: u32) {
    info()
        .s(what.as_bytes())
        .s(b": type ")
        .dec(e.app_type.into())
        .s(b" status ")
        .hex(e.status.into())
        .s(b" restartable ")
        .dec(e.restartable as u64)
        .s(b" reasons ")
        .hex(reasons.into())
        .s(b" name \"")
        .w(&e.app_name)
        .s(b"\" service \"")
        .w(&e.service)
        .s(b"\"\r\n")
        .print();
}

fn sessions(c: &mut Checks) -> Result<(), u32> {
    let mut key = [0u16; CCH_RM_SESSION_KEY + 1];
    let mut h = 0u32;
    // SAFETY: NULL pointers, then ours.
    unsafe {
        c.eq(RmStartSession(null_mut(), 0, key.as_mut_ptr()), ERROR_BAD_ARGUMENTS)?;
        c.eq(RmStartSession(&mut h, 0, null_mut()), ERROR_BAD_ARGUMENTS)?;
    }
    let mut key2 = [0xffffu16; CCH_RM_SESSION_KEY + 1];
    let mut h2 = 0u32;
    // SAFETY: our handles and key buffers.
    unsafe {
        c.eq(RmStartSession(&mut h, 0, key.as_mut_ptr()), ERROR_SUCCESS)?;
        c.eq(RmStartSession(&mut h2, 0, key2.as_mut_ptr()), ERROR_SUCCESS)?;
    }
    info()
        .s(b"session handles ")
        .dec(h.into())
        .s(b", ")
        .dec(h2.into())
        .s(b" key ")
        .w(&key)
        .s(b"\r\n")
        .print();
    for k in [&key, &key2] {
        c.ok(k[CCH_RM_SESSION_KEY] == 0, 0)?;
        let hex = k[..CCH_RM_SESSION_KEY]
            .iter()
            .all(|&ch| u8::try_from(ch).is_ok_and(|b| b.is_ascii_hexdigit()));
        c.ok(hex, u64::from(k[0]))?;
    }
    c.ok(h != h2 && key != key2, h2.into())?;
    // SAFETY: plain calls.
    unsafe {
        c.eq(RmEndSession(h2), ERROR_SUCCESS)?;
        c.eq(RmEndSession(h2), ERROR_INVALID_HANDLE)?;
        c.eq(RmEndSession(NOT_A_SESSION), ERROR_INVALID_HANDLE)?;
        c.eq(
            RmRegisterResources(NOT_A_SESSION, 0, null(), 0, null(), 0, null()),
            ERROR_INVALID_HANDLE,
        )?;
        c.eq(RmRegisterResources(h, 1, null(), 0, null(), 0, null()), ERROR_BAD_ARGUMENTS)?;
        c.eq(RmRegisterResources(h, 0, null(), 1, null(), 0, null()), ERROR_BAD_ARGUMENTS)?;
        c.eq(RmShutdown(NOT_A_SESSION, 0, None), ERROR_INVALID_HANDLE)?;
        c.eq(RmRestart(NOT_A_SESSION, 0, None), ERROR_INVALID_HANDLE)?;
    }
    // Joining: an unknown key, and this session's key from its own
    // process (the documentation speaks of secondary installers in or out
    // of the primary's process).
    let unknown = wide::<33>("0123456789abcdef0123456789abcdef");
    let mut joined = 0u32;
    // SAFETY: our handle and keys.
    let (r_unknown, r_own) = unsafe {
        (RmJoinSession(&mut joined, unknown.as_ptr()), RmJoinSession(&mut joined, key.as_ptr()))
    };
    info()
        .s(b"join unknown key ")
        .dec(r_unknown.into())
        .s(b", own key ")
        .dec(r_own.into())
        .s(b"\r\n")
        .print();
    c.ok(r_unknown != ERROR_SUCCESS, r_unknown.into())?;
    if r_own == ERROR_SUCCESS {
        let module = wide::<16>("C:\\x\\a.exe");
        // SAFETY: a joined (secondary) handle: filters are the primary's.
        let r = unsafe { RmAddFilter(joined, module.as_ptr(), null(), null(), RM_NO_SHUTDOWN) };
        c.eq(r, ERROR_SESSION_CREDENTIAL_CONFLICT)?;
        // SAFETY: plain call.
        c.eq(unsafe { RmEndSession(joined) }, ERROR_SUCCESS)?;
    }
    // SAFETY: plain call.
    c.eq(unsafe { RmEndSession(h) }, ERROR_SUCCESS)?;
    Ok(())
}

static LAST_STATUS: AtomicU32 = AtomicU32::new(u32::MAX);

extern "C" fn status(percent: u32) {
    LAST_STATUS.store(percent, Ordering::Relaxed);
}

/// Files nobody uses, a directory, and a file this process holds open.
fn files(c: &mut Checks) -> Result<(), u32> {
    let unused = Path::temp().push("rstrtmgr_test_unused.txt");
    let missing = Path::temp().push("rstrtmgr_test_missing.txt");
    let dir = Path::temp().push("rstrtmgr_test_dir");
    c.ok(make_file(&unused), 0)?;
    // SAFETY: NUL-terminated paths.
    unsafe {
        DeleteFileW(missing.ptr());
        CreateDirectoryW(dir.ptr(), 0);
    }
    let h = start_session(c)?;
    let names = [unused.ptr(), missing.ptr()];
    let svc = wide::<24>("gala_no_such_service");
    let services = [svc.as_ptr()];
    // SAFETY: arrays of NUL-terminated names.
    let r = unsafe { RmRegisterResources(h, 2, names.as_ptr(), 0, null(), 1, services.as_ptr()) };
    c.eq(r, ERROR_SUCCESS)?;
    let (r, needed, count, reasons) = get_list(h, &mut []);
    c.eq(r, ERROR_SUCCESS)?;
    c.eq(needed, 0)?;
    c.eq(count, 0)?;
    c.eq(reasons, 0)?;
    // NULL counters or reasons.
    let (mut n1, mut n2, mut rr) = (0u32, 0u32, 0u32);
    // SAFETY: NULL pointers where the function needs them.
    unsafe {
        c.eq(RmGetList(h, null_mut(), &mut n2, null_mut(), &mut rr), ERROR_BAD_ARGUMENTS)?;
        c.eq(RmGetList(h, &mut n1, null_mut(), null_mut(), &mut rr), ERROR_BAD_ARGUMENTS)?;
        c.eq(RmGetList(h, &mut n1, &mut n2, null_mut(), null_mut()), ERROR_BAD_ARGUMENTS)?;
        c.eq(
            RmGetList(NOT_A_SESSION, &mut n1, &mut n2, null_mut(), &mut rr),
            ERROR_INVALID_HANDLE,
        )?;
    }
    // RmRestart before RmShutdown; an empty list shuts down.
    // SAFETY: plain calls.
    unsafe {
        c.eq(RmRestart(h, 0, None), ERROR_REQUEST_OUT_OF_SEQUENCE)?;
        c.eq(RmShutdown(h, RM_FORCE_SHUTDOWN, Some(status)), ERROR_SUCCESS)?;
    }
    info()
        .s(b"empty shutdown's last status ")
        .dec(LAST_STATUS.load(Ordering::Relaxed).into())
        .s(b"\r\n")
        .print();
    // SAFETY: plain calls.
    unsafe {
        c.eq(RmRestart(h, 0, None), ERROR_SUCCESS)?;
        c.eq(RmEndSession(h), ERROR_SUCCESS)?;
    }

    // A directory.
    let h = start_session(c)?;
    let names = [dir.ptr()];
    // SAFETY: an array of one NUL-terminated name.
    c.eq(
        unsafe { RmRegisterResources(h, 1, names.as_ptr(), 0, null(), 0, null()) },
        ERROR_SUCCESS,
    )?;
    c.eq(get_list(h, &mut []).0, ERROR_ACCESS_DENIED)?;
    // SAFETY: plain call.
    c.eq(unsafe { RmEndSession(h) }, ERROR_SUCCESS)?;

    // A file this process holds open.
    // SAFETY: a NUL-terminated path.
    let held =
        unsafe { CreateFileW(unused.ptr(), GENERIC_READ, FILE_SHARE_ALL, 0, OPEN_EXISTING, 0, 0) };
    c.ok(held != INVALID_HANDLE_VALUE, 0)?;
    let h = start_session(c)?;
    let names = [unused.ptr()];
    // SAFETY: an array of one NUL-terminated name.
    c.eq(
        unsafe { RmRegisterResources(h, 1, names.as_ptr(), 0, null(), 0, null()) },
        ERROR_SUCCESS,
    )?;
    let (r, needed, _, _) = get_list(h, &mut []);
    c.eq(r, ERROR_MORE_DATA)?;
    c.ok(needed >= 1, needed.into())?;
    let mut list = [ZERO_INFO; 8];
    let (r, _, count, reasons) = get_list(h, &mut list);
    c.eq(r, ERROR_SUCCESS)?;
    // SAFETY: plain call.
    let me = unsafe { GetCurrentProcessId() };
    let mine = list[..count as usize].iter().find(|e| e.process.pid == me);
    c.ok(mine.is_some(), count.into())?;
    let mine = mine.copied().unwrap_or(ZERO_INFO);
    print_entry("own process", &mine, reasons);
    c.ok(mine.process.start == own_start(), mine.process.start[0].into())?;
    c.eq(mine.status, RM_STATUS_RUNNING)?;
    let mut ts = u32::MAX;
    // SAFETY: our session id.
    unsafe { ProcessIdToSessionId(me, &mut ts) };
    c.eq(mine.ts_session, ts)?;
    // SAFETY: plain calls.
    unsafe {
        c.eq(RmEndSession(h), ERROR_SUCCESS)?;
        CloseHandle(held);
        RemoveDirectoryW(dir.ptr());
    }
    Ok(())
}

/// A child that holds a file open: listed, by file and by its
/// RM_UNIQUE_PROCESS, then shut down.
fn child(c: &mut Checks) -> Result<(), u32> {
    let file = Path::temp().push("rstrtmgr_test_held.txt");
    c.ok(make_file(&file), 0)?;
    let mut exe = [0u16; 300];
    // SAFETY: our buffer.
    let n = unsafe { GetModuleFileNameW(0, exe.as_mut_ptr(), 260) } as usize;
    let mut cmd = [0u16; 800];
    let mut at = 0;
    for part in [&[u16::from(b'"')][..], &exe[..n], &wide::<7>("\" hold ")[..7], file.text()] {
        cmd[at..at + part.len()].copy_from_slice(part);
        at += part.len();
    }
    let ready_name = wide::<24>("rstrtmgr_test_ready");
    // SAFETY: a NUL-terminated name.
    let ready = unsafe { CreateEventW(0, 1, 0, ready_name.as_ptr()) };
    c.ok(ready != 0, 0)?;
    let si = StartupInfo { cb: core::mem::size_of::<StartupInfo>() as u32, rest: [0; 100] };
    let mut pi = ProcessInformation { process: 0, thread: 0, pid: 0, tid: 0 };
    // SAFETY: our command line, startup info and result.
    let ok = unsafe {
        CreateProcessW(null(), cmd.as_mut_ptr(), null(), null(), 0, 0, null(), null(), &si, &mut pi)
    };
    // SAFETY: plain call.
    c.ok(ok != 0, unsafe { GetLastError() }.into())?;
    let waits = [ready, pi.process];
    // SAFETY: our handles.
    let w = unsafe { WaitForMultipleObjects(2, waits.as_ptr(), 0, INFINITE) };
    let r = held_by_child(c, &file, &pi, &exe[..n]);
    // SAFETY: our handles (the child is ended if a check failed first).
    unsafe {
        TerminateProcess(pi.process, 1);
        CloseHandle(pi.process);
        CloseHandle(pi.thread);
        CloseHandle(ready);
        DeleteFileW(file.ptr());
    }
    c.eq(w, WAIT_OBJECT_0)?;
    r
}

fn held_by_child(
    c: &mut Checks,
    file: &Path,
    pi: &ProcessInformation,
    exe: &[u16],
) -> Result<(), u32> {
    // The child's image path, asked of its process (what RmGetList
    // describes it by): this program's.
    let mut image = [0u16; 300];
    let mut size = image.len() as u32;
    // SAFETY: our buffer and size.
    let named = unsafe { QueryFullProcessImageNameW(pi.process, 0, image.as_mut_ptr(), &mut size) };
    c.ok(named != 0 && image[..size as usize] == *exe, size.into())?;
    let (mut created, mut t) = ([0u32; 2], [[0u32; 2]; 3]);
    // SAFETY: our buffers.
    unsafe { GetProcessTimes(pi.process, &mut created, &mut t[0], &mut t[1], &mut t[2]) };
    let child = UniqueProcess { pid: pi.pid, start: created };
    // By its RM_UNIQUE_PROCESS, and not with another start time.
    let h = start_session(c)?;
    let other = UniqueProcess { pid: pi.pid, start: [created[0] ^ 1, created[1]] };
    // SAFETY: an array of one RM_UNIQUE_PROCESS.
    c.eq(unsafe { RmRegisterResources(h, 0, null(), 1, &other, 0, null()) }, ERROR_SUCCESS)?;
    c.eq(get_list(h, &mut []).1, 0)?;
    // SAFETY: as above.
    c.eq(unsafe { RmRegisterResources(h, 0, null(), 1, &child, 0, null()) }, ERROR_SUCCESS)?;
    let mut list = [ZERO_INFO; 8];
    let (r, _, count, _) = get_list(h, &mut list);
    c.eq(r, ERROR_SUCCESS)?;
    c.eq(count, 1)?;
    c.ok(list[0].process == child, list[0].process.pid.into())?;
    // SAFETY: plain call.
    c.eq(unsafe { RmEndSession(h) }, ERROR_SUCCESS)?;

    // By the file it holds.
    let h = start_session(c)?;
    let names = [file.ptr()];
    // SAFETY: an array of one NUL-terminated name.
    c.eq(
        unsafe { RmRegisterResources(h, 1, names.as_ptr(), 0, null(), 0, null()) },
        ERROR_SUCCESS,
    )?;
    let mut list = [ZERO_INFO; 8];
    let (r, _, count, reasons) = get_list(h, &mut list);
    c.eq(r, ERROR_SUCCESS)?;
    c.eq(count, 1)?;
    let e = list[0];
    print_entry("child", &e, reasons);
    c.ok(e.process == child, e.process.pid.into())?;
    c.eq(reasons, 0)?;
    c.eq(e.status, RM_STATUS_RUNNING)?;
    c.eq(e.app_type, RM_CONSOLE)?;
    c.eq(e.restartable as u32, 0)?;
    c.ok(until_nul(&e.service).is_empty(), 0)?;
    c.ok(!until_nul(&e.app_name).is_empty(), 0)?;
    // A filter on it, then a forced shutdown.
    // SAFETY: our RM_UNIQUE_PROCESS.
    c.eq(unsafe { RmAddFilter(h, null(), &child, null(), RM_NO_RESTART) }, ERROR_SUCCESS)?;
    LAST_STATUS.store(u32::MAX, Ordering::Relaxed);
    // SAFETY: plain call.
    c.eq(unsafe { RmShutdown(h, RM_FORCE_SHUTDOWN, Some(status)) }, ERROR_SUCCESS)?;
    // SAFETY: the child's handle.
    c.eq(unsafe { WaitForSingleObject(pi.process, INFINITE) }, WAIT_OBJECT_0)?;
    let (r, _, count, reasons) = get_list(h, &mut list);
    c.eq(r, ERROR_SUCCESS)?;
    c.eq(count, 1)?;
    print_entry("child after RmShutdown", &list[0], reasons);
    info()
        .s(b"shutdown's last status ")
        .dec(LAST_STATUS.load(Ordering::Relaxed).into())
        .s(b"\r\n")
        .print();
    c.ok(list[0].status & RM_STATUS_STOPPED != 0, list[0].status.into())?;
    // SAFETY: plain calls.
    unsafe {
        c.eq(RmRestart(h, 0, None), ERROR_SUCCESS)?;
        c.eq(RmEndSession(h), ERROR_SUCCESS)?;
    }
    Ok(())
}

fn filters(c: &mut Checks) -> Result<(), u32> {
    let h = start_session(c)?;
    let module = wide::<32>("C:\\gala\\no_such_app.exe");
    let service = wide::<16>("gala_no_svc");
    // SAFETY: NUL-terminated names; NULL where the call allows.
    unsafe {
        c.eq(
            RmAddFilter(NOT_A_SESSION, module.as_ptr(), null(), null(), RM_NO_SHUTDOWN),
            ERROR_INVALID_HANDLE,
        )?;
        c.eq(
            RmAddFilter(h, module.as_ptr(), null(), service.as_ptr(), RM_NO_SHUTDOWN),
            ERROR_BAD_ARGUMENTS,
        )?;
        c.eq(RmAddFilter(h, null(), null(), null(), RM_NO_SHUTDOWN), ERROR_BAD_ARGUMENTS)?;
        c.eq(RmAddFilter(h, module.as_ptr(), null(), null(), 0), ERROR_BAD_ARGUMENTS)?;
        c.eq(RmAddFilter(h, module.as_ptr(), null(), null(), RM_NO_SHUTDOWN), ERROR_SUCCESS)?;
        c.eq(RmAddFilter(h, null(), null(), service.as_ptr(), RM_NO_RESTART), ERROR_SUCCESS)?;
    }
    let mut needed = 0u32;
    // SAFETY: no buffer: the size needed.
    c.eq(unsafe { RmGetFilterList(h, null_mut(), 0, &mut needed) }, ERROR_MORE_DATA)?;
    c.ok(needed > 0 && needed <= 1024, needed.into())?;
    let mut buf = [0usize; 128];
    // SAFETY: our buffer of 1024 bytes.
    let r = unsafe { RmGetFilterList(h, buf.as_mut_ptr().cast(), needed, &mut needed) };
    c.eq(r, ERROR_SUCCESS)?;
    // Walk the RM_FILTER_INFO list: FilterAction, FilterTrigger,
    // cbNextOffset, then the union at the next pointer-aligned offset.
    let base = buf.as_ptr().cast::<u8>();
    let union_at = 12usize.next_multiple_of(core::mem::align_of::<usize>());
    let (mut at, mut seen) = (0usize, 0u32);
    loop {
        // SAFETY: inside our buffer, filled by RmGetFilterList.
        let (action, trigger, next, name) = unsafe {
            let word = |o: usize| base.add(at + o).cast::<u32>().read_unaligned();
            (
                word(0),
                word(4),
                word(8),
                base.add(at + union_at).cast::<*const u16>().read_unaligned(),
            )
        };
        let mut chars = 0;
        // SAFETY: the name the filter points to, NUL-terminated.
        while unsafe { *name.add(chars) } != 0 {
            chars += 1;
        }
        // SAFETY: as above.
        let name = unsafe { core::slice::from_raw_parts(name, chars) };
        match trigger {
            RM_FILTER_TRIGGER_FILE => {
                c.eq(action, RM_NO_SHUTDOWN)?;
                c.ok(name == until_nul(&module), chars as u64)?;
                seen |= 1;
            }
            RM_FILTER_TRIGGER_SERVICE => {
                c.eq(action, RM_NO_RESTART)?;
                c.ok(name == until_nul(&service), chars as u64)?;
                seen |= 2;
            }
            t => c.eq(t, 0)?,
        }
        if next == 0 {
            break;
        }
        at += next as usize;
    }
    c.eq(seen, 3)?;
    // SAFETY: NUL-terminated names.
    unsafe {
        c.eq(RmRemoveFilter(h, module.as_ptr(), null(), null()), ERROR_SUCCESS)?;
        c.eq(RmRemoveFilter(h, module.as_ptr(), null(), null()), ERROR_FILE_NOT_FOUND)?;
        c.eq(RmRemoveFilter(h, null(), null(), service.as_ptr()), ERROR_SUCCESS)?;
        c.eq(RmEndSession(h), ERROR_SUCCESS)?;
    }
    Ok(())
}

/// Sessions up to the limit.
fn limit(c: &mut Checks) -> Result<(), u32> {
    let mut handles = [0u32; 70];
    let mut started = 0;
    let mut last = ERROR_SUCCESS;
    for slot in handles.iter_mut() {
        let mut key = [0u16; CCH_RM_SESSION_KEY + 1];
        // SAFETY: our handle and key.
        last = unsafe { RmStartSession(slot, 0, key.as_mut_ptr()) };
        if last != ERROR_SUCCESS {
            break;
        }
        started += 1;
    }
    info().s(b"sessions started before the limit: ").dec(started).s(b"\r\n").print();
    for &h in &handles[..started as usize] {
        // SAFETY: plain call.
        c.eq(unsafe { RmEndSession(h) }, ERROR_SUCCESS)?;
    }
    c.eq(last, ERROR_MAX_SESSIONS_REACHED)?;
    c.ok((1..=64).contains(&started), started)?;
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    sessions(&mut c)?;
    files(&mut c)?;
    child(&mut c)?;
    filters(&mut c)?;
    limit(&mut c)?;
    Ok(())
}

/// The child: opens the file named after "hold " and waits to be ended.
fn hold(path: *const u16) -> ! {
    // SAFETY: a NUL-terminated path from the command line.
    let h = unsafe { CreateFileW(path, GENERIC_READ, FILE_SHARE_ALL, 0, OPEN_EXISTING, 0, 0) };
    if h == INVALID_HANDLE_VALUE {
        // SAFETY: ends the process.
        unsafe { ExitProcess(2) }
    }
    let name = wide::<24>("rstrtmgr_test_ready");
    // SAFETY: the parent's event, by name.
    unsafe {
        let ready = CreateEventW(0, 1, 0, name.as_ptr());
        SetEvent(ready);
        loop {
            Sleep(INFINITE);
        }
    }
}

/// The path after " hold " on the command line, if this is the child.
fn hold_argument() -> Option<*const u16> {
    // SAFETY: the process's NUL-terminated command line.
    let cmd = unsafe { GetCommandLineW() };
    let pattern: [u16; 6] = wide(" hold ");
    let mut i = 0;
    // SAFETY: read up to the NUL.
    unsafe {
        while *cmd.add(i) != 0 {
            if (0..6).all(|k| *cmd.add(i + k) == pattern[k]) {
                return Some(cmd.add(i + 6));
            }
            i += 1;
        }
    }
    None
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    if let Some(path) = hold_argument() {
        hold(path);
    }
    let code = match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
