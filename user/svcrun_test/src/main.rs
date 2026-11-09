//! Services that run (ROADMAP OF: the Click-to-Run client starts its
//! ClickToRunSvc service): this program creates services whose
//! executable is itself, run with `service <mode> <name>`, and starts and
//! controls them, run elevated (an administrator: SYSTEM on gala, CI's
//! elevated runner on Windows). The expected values are those of Wine's
//! conformance tests run on Windows (dlls/advapi32/tests/service.c
//! test_start_stop and its service; programs/services/tests/service.c
//! test_service, test_no_stop, test_kill_service_process) and Microsoft's
//! documentation (StartService, StartServiceCtrlDispatcher, ControlService,
//! SetServiceStatus, SERVICE_TABLE_ENTRY):
//! - a program that is not a service: StartServiceCtrlDispatcher fails
//!   with ERROR_FAILED_SERVICE_CONTROLLER_CONNECT;
//! - a service whose process exits without connecting: StartService
//!   fails with ERROR_SERVICE_REQUEST_TIMEOUT, twice alike, the service
//!   stopped without a process; stopping it is ERROR_SERVICE_NOT_ACTIVE;
//!   one whose executable does not exist fails, twice alike, stopped;
//! - a service that runs: StartService with two arguments succeeds (its
//!   ServiceMain gets the service's name and both); a second start is
//!   ERROR_SERVICE_ALREADY_RUNNING; it reports RUNNING (accepting stop and
//!   shutdown; its wait hint reads 0) with a process id; a user control
//!   (128) reaches its handler and succeeds though the handler answers
//!   0xdeadbeef; interrogation succeeds; a pause it does not accept is
//!   ERROR_INVALID_SERVICE_CONTROL; a stop reaches the handler, the
//!   service stops (exit code 0, no process) and its process ends;
//! - inside the service (its result comes back as its service-specific
//!   exit code, ERROR_SERVICE_SPECIFIC_ERROR): it runs as SYSTEM (S-1-5-18)
//!   in session 0, in the system directory, RegisterServiceCtrlHandlerEx
//!   takes any name (own process), a second StartServiceCtrlDispatcher is
//!   ERROR_SERVICE_ALREADY_RUNNING;
//! - a service process that is killed: the service is stopped with
//!   ERROR_PROCESS_ABORTED, nothing accepted, no process;
//! - a service whose ServiceMain returns after reporting RUNNING (ANSI
//!   dispatcher and handler) still stops through its handler, and its
//!   dispatcher then returns.
//!
//! The same source builds `wow64_svcrun_test`, the 32-bit (x86) program.
//! Prints `<name>: <section>` before each section, then `svcrun_test: ok`,
//! or `<name>: check N got X (line L)` and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::{null, null_mut};

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "svcrun_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_svcrun_test";

type Handle = usize;

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
    fn SetLastError(e: u32);
    fn CloseHandle(h: Handle) -> i32;
    fn GetCommandLineW() -> *const u16;
    fn GetModuleFileNameW(module: Handle, buf: *mut u16, size: u32) -> u32;
    fn CreateEventW(
        attributes: *const c_void,
        manual: i32,
        initial: i32,
        name: *const u16,
    ) -> Handle;
    fn SetEvent(h: Handle) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn TerminateProcess(h: Handle, code: u32) -> i32;
    fn GetCurrentProcess() -> Handle;
    fn GetCurrentDirectoryW(size: u32, buf: *mut u16) -> u32;
    fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
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
    fn QueryServiceStatus(h: Handle, status: *mut ServiceStatus) -> i32;
    fn QueryServiceStatusEx(
        h: Handle,
        level: u32,
        buffer: *mut StatusProcess,
        size: u32,
        needed: *mut u32,
    ) -> i32;
    fn CreateServiceW(
        scm: Handle,
        name: *const u16,
        display: *const u16,
        access: u32,
        kind: u32,
        start: u32,
        error_control: u32,
        path: *const u16,
        group: *const u16,
        tag: *mut u32,
        dependencies: *const u16,
        start_name: *const u16,
        password: *const u16,
    ) -> Handle;
    fn DeleteService(h: Handle) -> i32;
    fn StartServiceW(h: Handle, argc: u32, argv: *const *const u16) -> i32;
    fn ControlService(h: Handle, control: u32, status: *mut ServiceStatus) -> i32;
    fn StartServiceCtrlDispatcherW(table: *const TableW) -> i32;
    fn StartServiceCtrlDispatcherA(table: *const TableA) -> i32;
    fn RegisterServiceCtrlHandlerExW(
        name: *const u16,
        handler: extern "system" fn(u32, u32, *mut c_void, *mut c_void) -> u32,
        context: *mut c_void,
    ) -> Handle;
    fn RegisterServiceCtrlHandlerExA(
        name: *const u8,
        handler: extern "system" fn(u32, u32, *mut c_void, *mut c_void) -> u32,
        context: *mut c_void,
    ) -> Handle;
    fn SetServiceStatus(h: Handle, status: *const ServiceStatus) -> i32;
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        info: *mut u8,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct ServiceStatus {
    kind: u32,
    state: u32,
    controls: u32,
    exit_code: u32,
    specific_exit_code: u32,
    check_point: u32,
    wait_hint: u32,
}

/// SERVICE_STATUS_PROCESS.
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct StatusProcess {
    kind: u32,
    state: u32,
    controls: u32,
    exit_code: u32,
    specific_exit_code: u32,
    check_point: u32,
    wait_hint: u32,
    pid: u32,
    flags: u32,
}

/// SERVICE_TABLE_ENTRYW / A.
#[repr(C)]
struct TableW {
    name: *const u16,
    main: Option<extern "system" fn(u32, *mut *mut u16)>,
}

#[repr(C)]
struct TableA {
    name: *const u8,
    main: Option<extern "system" fn(u32, *mut *mut u8)>,
}

const ERROR_INVALID_SERVICE_CONTROL: u32 = 1052;
const ERROR_SERVICE_REQUEST_TIMEOUT: u32 = 1053;
const ERROR_SERVICE_ALREADY_RUNNING: u32 = 1056;
const ERROR_SERVICE_NOT_ACTIVE: u32 = 1062;
const ERROR_FAILED_SERVICE_CONTROLLER_CONNECT: u32 = 1063;
const ERROR_SERVICE_SPECIFIC_ERROR: u32 = 1066;
const ERROR_PROCESS_ABORTED: u32 = 1067;
const ERROR_CALL_NOT_IMPLEMENTED: u32 = 120;

const SC_MANAGER_ALL_ACCESS: u32 = 0x000f_003f;
const SERVICE_ALL_ACCESS: u32 = 0x000f_01ff;
const SERVICE_WIN32_OWN_PROCESS: u32 = 0x10;
const SERVICE_DEMAND_START: u32 = 3;
const SERVICE_ERROR_IGNORE: u32 = 0;
const SERVICE_STOPPED: u32 = 1;
const SERVICE_STOP_PENDING: u32 = 3;
const SERVICE_RUNNING: u32 = 4;
const SERVICE_ACCEPT_STOP: u32 = 1;
const SERVICE_ACCEPT_SHUTDOWN: u32 = 4;
const SERVICE_CONTROL_STOP: u32 = 1;
const SERVICE_CONTROL_PAUSE: u32 = 2;
const SERVICE_CONTROL_INTERROGATE: u32 = 4;
const SERVICE_CONTROL_SHUTDOWN: u32 = 5;
const SC_STATUS_PROCESS_INFO: u32 = 0;
const SYNCHRONIZE: u32 = 0x0010_0000;
const PROCESS_TERMINATE: u32 = 0x0001;
const TOKEN_QUERY: u32 = 8;

struct Line {
    buf: [u8; 200],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 200], n: 0 }
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

fn section(what: &str) {
    Line::new().s(NAME.as_bytes()).s(b": ").s(what.as_bytes()).s(b"\r\n").print();
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
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// A call that returned `succeeded` failed with last error `want`.
    #[track_caller]
    fn fails(&mut self, succeeded: bool, want: u32) -> Result<(), u32> {
        let error = last_error();
        self.ok(!succeeded && error == want, if succeeded { u64::MAX } else { u64::from(error) })
    }

    /// A call that should have succeeded (the error otherwise).
    #[track_caller]
    fn succeeds(&mut self, succeeded: bool) -> Result<(), u32> {
        let error = last_error();
        self.ok(succeeded, u64::from(error))
    }
}

fn last_error() -> u32 {
    // SAFETY: plain call.
    unsafe { GetLastError() }
}

/// A NUL-terminated UTF-16 string of up to 300 units.
struct W {
    buf: [u16; 300],
    n: usize,
}

impl W {
    fn new() -> W {
        W { buf: [0; 300], n: 0 }
    }

    fn s(mut self, text: &str) -> W {
        for b in text.bytes() {
            self = self.u(&[u16::from(b)]);
        }
        self
    }

    fn u(mut self, units: &[u16]) -> W {
        for &c in units {
            if self.n + 1 < self.buf.len() {
                self.buf[self.n] = c;
                self.n += 1;
            }
        }
        self.buf[self.n] = 0;
        self
    }

    fn p(&self) -> *const u16 {
        self.buf.as_ptr()
    }
}

/// The NUL-terminated UTF-16 string at `p` (empty for NULL).
fn units<'a>(p: *const u16) -> &'a [u16] {
    if p.is_null() {
        return &[];
    }
    let mut n = 0;
    // SAFETY: a NUL-terminated string.
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    }
}

/// Whether `s` equals ASCII `text`, ignoring case.
fn same(s: &[u16], text: &str) -> bool {
    s.len() == text.len()
        && s.iter().zip(text.bytes()).all(|(&a, b)| a < 0x80 && (a as u8).eq_ignore_ascii_case(&b))
}

fn same_units(a: &[u16], b: &[u16]) -> bool {
    let up = |c: u16| if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c };
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| up(x) == up(y))
}

/// The words of this process's command line after the program's path.
fn arguments(out: &mut [&'static [u16]; 4]) -> usize {
    // SAFETY: the process's command line, alive as long as the process.
    let line: &'static [u16] = units(unsafe { GetCommandLineW() });
    let mut rest = line;
    // The program's path, quoted or not.
    if rest.first() == Some(&u16::from(b'"')) {
        let end =
            rest[1..].iter().position(|&c| c == u16::from(b'"')).map_or(rest.len(), |e| e + 2);
        rest = &rest[end.min(rest.len())..];
    } else {
        let end = rest.iter().position(|&c| c == u16::from(b' ')).unwrap_or(rest.len());
        rest = &rest[end..];
    }
    let mut n = 0;
    for word in rest.split(|&c| c == u16::from(b' ')).filter(|w| !w.is_empty()) {
        if n < out.len() {
            out[n] = word;
            n += 1;
        }
    }
    n
}

// ---- The service side ----

/// The service's name (from its command line), its mode and events.
static mut SERVICE_NAME: [u16; 64] = [0; 64];
static mut STATUS_HANDLE: Handle = 0;
static mut STOP_EVENT: Handle = 0;
/// The first service-side check that failed (0: none).
static mut FAILED: u32 = 0;

fn service_name() -> &'static [u16] {
    // SAFETY: written once before the dispatcher starts.
    units(unsafe { (*core::ptr::addr_of!(SERVICE_NAME)).as_ptr() })
}

/// The named event `<service>_<what>` (Global\ namespace), created or
/// opened.
fn event(service: &[u16], what: &str) -> Handle {
    let name = W::new().s("Global\\").u(service).s("_").s(what);
    // SAFETY: our name; an auto-reset event.
    unsafe { CreateEventW(null(), 0, 0, name.p()) }
}

fn signal(what: &str) {
    let h = event(service_name(), what);
    // SAFETY: our event handle.
    unsafe {
        SetEvent(h);
        CloseHandle(h);
    }
}

fn report(state: u32, controls: u32, exit: u32, specific: u32, wait_hint: u32) -> bool {
    let st = ServiceStatus {
        kind: SERVICE_WIN32_OWN_PROCESS,
        state,
        controls,
        exit_code: exit,
        specific_exit_code: specific,
        check_point: 0,
        wait_hint,
    };
    // SAFETY: our status and the handle RegisterServiceCtrlHandlerEx gave.
    unsafe { SetServiceStatus(STATUS_HANDLE, &st) != 0 }
}

fn note_failure(n: u32) {
    // SAFETY: the service's threads; a plain word.
    unsafe {
        if FAILED == 0 {
            FAILED = n;
        }
    }
}

extern "system" fn handler(
    control: u32,
    _event: u32,
    _data: *mut c_void,
    _ctx: *mut c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            report(SERVICE_STOP_PENDING, 0, 0, 0, 0);
            // SAFETY: the event ServiceMain made.
            unsafe { SetEvent(STOP_EVENT) };
            0
        }
        128 => {
            signal("custom");
            0xdead_beef
        }
        SERVICE_CONTROL_INTERROGATE => 0,
        _ => ERROR_CALL_NOT_IMPLEMENTED,
    }
}

/// The checks a running service makes about itself (the first failing
/// one's number, from 101 on).
fn check_self(argc: u32, argv: *mut *mut u16, args: bool) -> u32 {
    // SAFETY: the arguments the dispatcher passed.
    let arg = |i: u32| units(unsafe { *argv.add(i as usize) });
    let want = if args { 3 } else { 1 };
    if argc != want {
        return 101;
    }
    if !same_units(arg(0), service_name()) {
        return 102;
    }
    if args && !(same(arg(1), "param1") && same(arg(2), "param2")) {
        return 103;
    }
    // SYSTEM's token, session 0.
    let mut token = 0;
    let mut buf = [0u8; 128];
    let mut got = 0u32;
    // SAFETY: plain calls with our buffers.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return 104;
        }
        if GetTokenInformation(token, 1, buf.as_mut_ptr(), 128, &mut got) == 0 {
            return 105;
        }
        let sid = *(buf.as_ptr() as *const *const u8);
        let system = [1u8, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0];
        if core::slice::from_raw_parts(sid, 12) != system {
            return 106;
        }
        let mut session = 0xffff_ffffu32;
        if GetTokenInformation(token, 12, (&mut session as *mut u32).cast(), 4, &mut got) == 0
            || session != 0
        {
            return 107;
        }
        CloseHandle(token);
    }
    // In the system directory.
    let (mut cwd, mut sys) = ([0u16; 260], [0u16; 260]);
    // SAFETY: our buffers.
    let (c, s) = unsafe {
        (GetCurrentDirectoryW(260, cwd.as_mut_ptr()), GetSystemDirectoryW(sys.as_mut_ptr(), 260))
    };
    let cwd = &cwd[..c as usize];
    let cwd = cwd.strip_suffix(&[u16::from(b'\\')]).unwrap_or(cwd);
    if s == 0 || !same_units(cwd, &sys[..s as usize]) {
        return 108;
    }
    // A second dispatcher in the process.
    let empty = [0u16];
    let table = [
        TableW { name: empty.as_ptr(), main: Some(serve_main) },
        TableW { name: null(), main: None },
    ];
    // SAFETY: a well-formed table (the call fails before using it).
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } != 0
        || last_error() != ERROR_SERVICE_ALREADY_RUNNING
    {
        return 109;
    }
    0
}

/// ServiceMain of modes `serve` (two arguments) and `kill` (none): checks
/// itself, reports RUNNING (a wait hint the SCM drops), signals
/// `<name>_running`, waits for the stop and reports STOPPED with the
/// first failed check as its service-specific exit code.
fn serve(argc: u32, argv: *mut *mut u16, args: bool) {
    // SAFETY: plain calls; the handle is the service's own.
    unsafe {
        STOP_EVENT = CreateEventW(null(), 1, 0, null());
        // Any name in an own-process service ("" here).
        STATUS_HANDLE = RegisterServiceCtrlHandlerExW([0u16].as_ptr(), handler, null_mut());
    }
    // SAFETY: as above.
    if unsafe { STATUS_HANDLE } == 0 {
        signal("running");
        return;
    }
    note_failure(check_self(argc, argv, args));
    if !report(SERVICE_RUNNING, SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN, 0, 0, 10_000) {
        note_failure(110);
    }
    signal("running");
    // SAFETY: our event.
    unsafe { WaitForSingleObject(STOP_EVENT, 0xffff_ffff) };
    // SAFETY: the service's threads are done with it.
    let failed = unsafe { FAILED };
    if failed == 0 {
        report(SERVICE_STOPPED, 0, 0, 0, 0);
    } else {
        report(SERVICE_STOPPED, 0, ERROR_SERVICE_SPECIFIC_ERROR, failed, 0);
    }
}

extern "system" fn serve_main(argc: u32, argv: *mut *mut u16) {
    serve(argc, argv, true);
}

extern "system" fn kill_main(argc: u32, argv: *mut *mut u16) {
    serve(argc, argv, false);
}

extern "system" fn nostop_handler(
    control: u32,
    _event: u32,
    _data: *mut c_void,
    _ctx: *mut c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            report(SERVICE_STOPPED, 0, 0, 0, 0);
            0
        }
        _ => 0,
    }
}

/// Mode `nostop` (ANSI): reports RUNNING and returns; its handler stops
/// the service.
extern "system" fn nostop_main(_argc: u32, _argv: *mut *mut u8) {
    // SAFETY: plain call.
    unsafe {
        STATUS_HANDLE =
            RegisterServiceCtrlHandlerExA(c"".as_ptr().cast(), nostop_handler, null_mut())
    };
    report(SERVICE_RUNNING, SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN, 0, 0, 0);
    signal("running");
}

/// This process as service `name` in `mode`: the dispatcher's result.
fn run_service(mode: &[u16], name: &[u16]) -> u32 {
    // SAFETY: before any other thread.
    unsafe {
        let slot = &mut *core::ptr::addr_of_mut!(SERVICE_NAME);
        for (d, &c) in slot.iter_mut().zip(name.iter().take(63)) {
            *d = c;
        }
    }
    let ok = if same(mode, "exit") {
        return 0;
    } else if same(mode, "nostop") {
        let table = [
            TableA { name: c"".as_ptr().cast(), main: Some(nostop_main) },
            TableA { name: null(), main: None },
        ];
        // SAFETY: our table.
        unsafe { StartServiceCtrlDispatcherA(table.as_ptr()) }
    } else {
        let main: extern "system" fn(u32, *mut *mut u16) =
            if same(mode, "kill") { kill_main } else { serve_main };
        let empty = [0u16];
        let table = [
            TableW { name: empty.as_ptr(), main: Some(main) },
            TableW { name: null(), main: None },
        ];
        // SAFETY: our table.
        unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) }
    };
    if ok != 0 { 0 } else { last_error() }
}

// ---- The client side ----

/// The path of this program.
fn own_path() -> W {
    let mut w = W::new();
    // SAFETY: our buffer.
    let n = unsafe { GetModuleFileNameW(0, w.buf.as_mut_ptr(), 260) } as usize;
    w.n = n.min(260);
    w.buf[w.n] = 0;
    w
}

/// The service name `<program>` or `<program>_<suffix>`.
fn service(suffix: &str) -> W {
    let w = W::new().s(NAME);
    if suffix.is_empty() { w } else { w.s("_").s(suffix) }
}

fn delete_if_there(scm: Handle, name: &W) {
    // SAFETY: plain calls.
    unsafe {
        let h = OpenServiceW(scm, name.p(), SERVICE_ALL_ACCESS);
        if h != 0 {
            let mut st = ServiceStatus::default();
            ControlService(h, SERVICE_CONTROL_STOP, &mut st);
            DeleteService(h);
            CloseServiceHandle(h);
        }
    }
}

/// Creates the demand-start service `name` running `path`.
fn create(c: &mut Checks, scm: Handle, name: &W, path: &W) -> Result<Handle, u32> {
    delete_if_there(scm, name);
    // SAFETY: our strings.
    let h = unsafe {
        CreateServiceW(
            scm,
            name.p(),
            name.p(),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_DEMAND_START,
            SERVICE_ERROR_IGNORE,
            path.p(),
            null(),
            null_mut(),
            null(),
            null(),
            null(),
        )
    };
    c.succeeds(h != 0)?;
    Ok(h)
}

/// `"<own path>" service <mode> <name>`.
fn command(mode: &str, name: &W) -> W {
    let own = own_path();
    W::new().s("\"").u(&own.buf[..own.n]).s("\" service ").s(mode).s(" ").u(&name.buf[..name.n])
}

fn query(c: &mut Checks, h: Handle) -> Result<StatusProcess, u32> {
    let mut sp = StatusProcess::default();
    let mut needed = 0;
    // SAFETY: our buffer.
    let ok = unsafe { QueryServiceStatusEx(h, SC_STATUS_PROCESS_INFO, &mut sp, 36, &mut needed) };
    c.succeeds(ok != 0)?;
    Ok(sp)
}

/// The service's status once it is STOPPED: Windows' SCM learns of a
/// process's end on its own time, so it is asked again (every 50 ms, for
/// at most 30 s) until then.
fn query_stopped(c: &mut Checks, h: Handle) -> Result<StatusProcess, u32> {
    for _ in 0..600 {
        let sp = query(c, h)?;
        if sp.state == SERVICE_STOPPED {
            return Ok(sp);
        }
        // SAFETY: plain call.
        unsafe { Sleep(50) };
    }
    query(c, h)
}

fn wait(c: &mut Checks, h: Handle) -> Result<(), u32> {
    // SAFETY: our handle.
    let r = unsafe { WaitForSingleObject(h, 30_000) };
    c.eq(u64::from(r), 0)
}

/// A service started and stopped by its process's end before it connects,
/// and one whose executable is missing (Wine's try_start_stop).
fn failed_starts(c: &mut Checks, scm: Handle) -> Result<(), u32> {
    section("a process that exits without connecting");
    let name = service("exit");
    let h = create(c, scm, &name, &command("exit", &name))?;
    for _ in 0..2 {
        // SAFETY: our handle.
        c.fails(unsafe { StartServiceW(h, 0, null()) } != 0, ERROR_SERVICE_REQUEST_TIMEOUT)?;
        let sp = query(c, h)?;
        c.eq(u64::from(sp.state), u64::from(SERVICE_STOPPED))?;
        c.eq(u64::from(sp.pid), 0)?;
    }
    let mut st = ServiceStatus { state: 0xdead_beef, ..Default::default() };
    // SAFETY: our handle and status.
    c.fails(
        unsafe { ControlService(h, SERVICE_CONTROL_STOP, &mut st) } != 0,
        ERROR_SERVICE_NOT_ACTIVE,
    )?;
    c.eq(u64::from(st.state), u64::from(SERVICE_STOPPED))?;
    // SAFETY: our handle.
    unsafe {
        DeleteService(h);
        CloseServiceHandle(h);
    }

    section("a missing executable");
    let name = service("none");
    let h = create(c, scm, &name, &W::new().s("C:\\gala_svcrun_none\\none.exe"))?;
    // SAFETY: our handle.
    c.ok(unsafe { StartServiceW(h, 0, null()) } == 0, 1)?;
    let first = last_error();
    let sp = query(c, h)?;
    c.eq(u64::from(sp.state), u64::from(SERVICE_STOPPED))?;
    c.eq(u64::from(sp.pid), 0)?;
    // SAFETY: our handle.
    c.fails(unsafe { StartServiceW(h, 0, null()) } != 0, first)?;
    // SAFETY: our handle.
    unsafe {
        DeleteService(h);
        CloseServiceHandle(h);
    }
    Ok(())
}

/// The status a stopped service reports: `exit`, nothing accepted, no
/// process; a service-side failure is shown as its check number.
fn expect_stopped(c: &mut Checks, sp: &StatusProcess, exit: u32) -> Result<(), u32> {
    if sp.exit_code == ERROR_SERVICE_SPECIFIC_ERROR {
        // The service's own check `specific` failed.
        c.eq(u64::from(sp.specific_exit_code), 0)?;
    }
    c.eq(u64::from(sp.kind), u64::from(SERVICE_WIN32_OWN_PROCESS))?;
    c.eq(u64::from(sp.state), u64::from(SERVICE_STOPPED))?;
    c.eq(u64::from(sp.exit_code), u64::from(exit))?;
    c.eq(u64::from(sp.specific_exit_code), 0)?;
    c.eq(u64::from(sp.controls), 0)?;
    c.eq(u64::from(sp.check_point), 0)?;
    c.eq(u64::from(sp.wait_hint), 0)?;
    c.eq(u64::from(sp.pid), 0)
}

/// Waits for the service's `running` event, then checks it runs: its
/// process's handle (SYNCHRONIZE and PROCESS_TERMINATE).
fn expect_running(c: &mut Checks, h: Handle, running: Handle) -> Result<Handle, u32> {
    wait(c, running)?;
    let sp = query(c, h)?;
    if sp.state == SERVICE_STOPPED {
        // It stopped at once: one of its own checks failed.
        c.eq(u64::from(sp.specific_exit_code), 0)?;
    }
    c.eq(u64::from(sp.kind), u64::from(SERVICE_WIN32_OWN_PROCESS))?;
    c.eq(u64::from(sp.state), u64::from(SERVICE_RUNNING))?;
    c.eq(u64::from(sp.controls), u64::from(SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN))?;
    c.eq(u64::from(sp.exit_code), 0)?;
    c.eq(u64::from(sp.specific_exit_code), 0)?;
    c.eq(u64::from(sp.check_point), 0)?;
    c.eq(u64::from(sp.wait_hint), 0)?;
    c.ok(sp.pid != 0, 0)?;
    c.eq(u64::from(sp.flags), 0)?;
    // SAFETY: plain call.
    let process = unsafe { OpenProcess(SYNCHRONIZE | PROCESS_TERMINATE, 0, sp.pid) };
    c.succeeds(process != 0)?;
    Ok(process)
}

fn running_service(c: &mut Checks, scm: Handle) -> Result<(), u32> {
    section("a service that runs");
    let name = service("");
    let (running, custom) =
        (event(&name.buf[..name.n], "running"), event(&name.buf[..name.n], "custom"));
    let h = create(c, scm, &name, &command("serve", &name))?;
    let (p1, p2) = (W::new().s("param1"), W::new().s("param2"));
    let argv = [p1.p(), p2.p()];
    // SAFETY: our handle and arguments.
    c.succeeds(unsafe { StartServiceW(h, 2, argv.as_ptr()) } != 0)?;
    // SAFETY: as above.
    c.fails(unsafe { StartServiceW(h, 0, null()) } != 0, ERROR_SERVICE_ALREADY_RUNNING)?;
    let process = expect_running(c, h, running)?;
    let mut st = ServiceStatus::default();
    // SAFETY: our handle and status throughout.
    unsafe {
        c.succeeds(QueryServiceStatus(h, &mut st) != 0)?;
        c.eq(u64::from(st.state), u64::from(SERVICE_RUNNING))?;
        c.eq(u64::from(st.wait_hint), 0)?;
        // A user control; its handler's answer (0xdeadbeef) is not the call's.
        c.succeeds(ControlService(h, 128, &mut st) != 0)?;
        wait(c, custom)?;
        c.succeeds(ControlService(h, SERVICE_CONTROL_INTERROGATE, &mut st) != 0)?;
        c.eq(u64::from(st.state), u64::from(SERVICE_RUNNING))?;
        c.fails(
            ControlService(h, SERVICE_CONTROL_PAUSE, &mut st) != 0,
            ERROR_INVALID_SERVICE_CONTROL,
        )?;
        c.succeeds(ControlService(h, SERVICE_CONTROL_STOP, &mut st) != 0)?;
        c.ok(st.state == SERVICE_STOP_PENDING || st.state == SERVICE_STOPPED, u64::from(st.state))?;
    }
    wait(c, process)?;
    let sp = query_stopped(c, h)?;
    expect_stopped(c, &sp, 0)?;
    // SAFETY: our handle and status.
    c.fails(
        unsafe { ControlService(h, SERVICE_CONTROL_STOP, &mut st) } != 0,
        ERROR_SERVICE_NOT_ACTIVE,
    )?;

    section("a service process that is killed");
    // SAFETY: our handles.
    unsafe {
        DeleteService(h);
        CloseServiceHandle(h);
        CloseHandle(process);
    }
    let name = service("kill");
    let running = event(&name.buf[..name.n], "running");
    let h = create(c, scm, &name, &command("kill", &name))?;
    // SAFETY: our handle.
    c.succeeds(unsafe { StartServiceW(h, 0, null()) } != 0)?;
    let process = expect_running(c, h, running)?;
    // SAFETY: the service's process.
    c.succeeds(unsafe { TerminateProcess(process, 0xdead_beef) } != 0)?;
    wait(c, process)?;
    let sp = query_stopped(c, h)?;
    expect_stopped(c, &sp, ERROR_PROCESS_ABORTED)?;
    // SAFETY: our handles.
    unsafe {
        DeleteService(h);
        CloseServiceHandle(h);
        CloseHandle(process);
    }

    section("a ServiceMain that returns");
    let name = service("nostop");
    let running = event(&name.buf[..name.n], "running");
    let h = create(c, scm, &name, &command("nostop", &name))?;
    // SAFETY: our handle.
    c.succeeds(unsafe { StartServiceW(h, 0, null()) } != 0)?;
    let process = expect_running(c, h, running)?;
    // SAFETY: our handle and status.
    c.succeeds(unsafe { ControlService(h, SERVICE_CONTROL_STOP, &mut st) } != 0)?;
    // Its dispatcher returns: the process ends.
    wait(c, process)?;
    let sp = query_stopped(c, h)?;
    expect_stopped(c, &sp, 0)?;
    // SAFETY: our handles.
    unsafe {
        DeleteService(h);
        CloseServiceHandle(h);
        CloseHandle(process);
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    section("not a service");
    let table = [
        TableW { name: [0u16].as_ptr(), main: Some(serve_main) },
        TableW { name: null(), main: None },
    ];
    // SAFETY: plain call with our table.
    unsafe { SetLastError(0xdead_beef) };
    // SAFETY: as above.
    c.fails(
        unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } != 0,
        ERROR_FAILED_SERVICE_CONTROLLER_CONNECT,
    )?;
    // SAFETY: plain call.
    let scm = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_ALL_ACCESS) };
    c.succeeds(scm != 0)?;
    failed_starts(&mut c, scm)?;
    running_service(&mut c, scm)?;
    // SAFETY: our handle.
    unsafe { CloseServiceHandle(scm) };
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let mut words: [&'static [u16]; 4] = [&[]; 4];
    let n = arguments(&mut words);
    if n >= 3 && same(words[0], "service") {
        let code = run_service(words[1], words[2]);
        // SAFETY: ends the process.
        unsafe { ExitProcess(code) }
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
