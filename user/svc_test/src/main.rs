//! The service control manager's client functions (advapi32: ROADMAP OF,
//! the Office Deployment Tool's setup.exe delay-loads OpenSCManagerW),
//! run elevated (an administrator: SYSTEM on gala, CI's elevated runner
//! on Windows). The expected values are those of Wine's conformance tests
//! (dlls/advapi32/tests/service.c, run against Windows), Microsoft's
//! documentation ("Service Security and Access Rights", CreateServiceW,
//! DeleteService, ControlService) and real Windows on CI's runner (this
//! program's run there):
//! - OpenSCManagerW: any access (SC_MANAGER_CONNECT is implied), NULL or
//!   "ServicesActive" databases on the local machine ("" too);
//!   "ServicesFailed" is ERROR_DATABASE_DOES_NOT_EXIST, another name
//!   ERROR_INVALID_NAME;
//! - NULL handles are ERROR_INVALID_HANDLE everywhere, except that
//!   QueryServiceStatusEx and EnumServicesStatusExW check the level
//!   first (ERROR_INVALID_LEVEL) and QueryServiceStatusEx the buffer size
//!   second (ERROR_INSUFFICIENT_BUFFER, 36 needed); NULL names and paths
//!   are ERROR_INVALID_ADDRESS;
//! - a service that does not exist is ERROR_SERVICE_DOES_NOT_EXIST;
//! - AFD (Winsock's ancillary function driver, which every Windows and
//!   gala's kernel provide) is a running kernel driver without a process;
//!   opened with STANDARD_RIGHTS_READ its status is ERROR_ACCESS_DENIED,
//!   with SERVICE_QUERY_STATUS it is not; its configuration needs
//!   SERVICE_QUERY_CONFIG;
//! - EnumServicesStatusExW needs SC_MANAGER_ENUMERATE_SERVICE (after the
//!   type and state are checked), an unknown group is
//!   ERROR_SERVICE_DOES_NOT_EXIST, a short buffer ERROR_MORE_DATA with the
//!   size; the drivers include AFD among the active ones only;
//! - CreateServiceW: SC_MANAGER_CREATE_SERVICE is checked after the NULL
//!   pointers and before the names (ERROR_INVALID_NAME for an empty name,
//!   ERROR_INVALID_PARAMETER for an empty path, mixed types, a start
//!   type that is not one, boot start for a program); a second creation
//!   is ERROR_SERVICE_EXISTS; the new service's configuration is what was
//!   given (LocalSystem, no group, no dependencies), its status stopped,
//!   never started (exit code ERROR_SERVICE_NEVER_STARTED); starting a
//!   disabled one is ERROR_SERVICE_DISABLED, stopping a stopped one
//!   ERROR_SERVICE_NOT_ACTIVE; DeleteService needs DELETE, a second one
//!   is ERROR_SERVICE_MARKED_FOR_DELETE, as is creating it again while a
//!   handle is open; it can still be opened then;
//! - a standard user (`galatest`, impersonated) may connect and enumerate
//!   but not create services or open the manager for all access, and may
//!   query AFD's status but not stop it.
//!
//! The same source builds `wow64_svc_test`, the 32-bit (x86) program.
//! Prints `svc_test: ok` (`wow64_svc_test: ok`), or `<name>: check N got
//! X (line L)` and exits with N. Without the `galatest` account the
//! standard user's part says it is skipped.

#![no_std]
#![no_main]

use core::ptr::{null, null_mut};

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "svc_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_svc_test";

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
        buffer: *mut u8,
        size: u32,
        needed: *mut u32,
    ) -> i32;
    fn QueryServiceConfigW(h: Handle, config: *mut u8, size: u32, needed: *mut u32) -> i32;
    fn EnumServicesStatusExW(
        scm: Handle,
        level: u32,
        kind: u32,
        state: u32,
        buffer: *mut u8,
        size: u32,
        needed: *mut u32,
        returned: *mut u32,
        resume: *mut u32,
        group: *const u16,
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
    fn LogonUserW(
        user: *const u16,
        domain: *const u16,
        password: *const u16,
        kind: u32,
        provider: u32,
        token: *mut Handle,
    ) -> i32;
    fn ImpersonateLoggedOnUser(token: Handle) -> i32;
    fn RevertToSelf() -> i32;
}

#[repr(C)]
#[derive(Default)]
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

/// QUERY_SERVICE_CONFIGW.
#[repr(C)]
struct Config {
    kind: u32,
    start: u32,
    error_control: u32,
    path: *const u16,
    group: *const u16,
    tag: u32,
    dependencies: *const u16,
    start_name: *const u16,
    display: *const u16,
}

/// ENUM_SERVICE_STATUS_PROCESSW.
#[repr(C)]
struct EnumEntry {
    name: *const u16,
    display: *const u16,
    status: StatusProcess,
}

const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_INVALID_NAME: u32 = 123;
const ERROR_INVALID_LEVEL: u32 = 124;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_INVALID_ADDRESS: u32 = 487;
const ERROR_SERVICE_DISABLED: u32 = 1058;
const ERROR_SERVICE_DOES_NOT_EXIST: u32 = 1060;
const ERROR_SERVICE_NOT_ACTIVE: u32 = 1062;
const ERROR_DATABASE_DOES_NOT_EXIST: u32 = 1065;
const ERROR_SERVICE_MARKED_FOR_DELETE: u32 = 1072;
const ERROR_SERVICE_EXISTS: u32 = 1073;
const ERROR_SERVICE_NEVER_STARTED: u32 = 1077;
const ERROR_LOGON_FAILURE: u32 = 1326;

const DELETE: u32 = 0x0001_0000;
const STANDARD_RIGHTS_READ: u32 = 0x0002_0000;
const GENERIC_READ: u32 = 0x8000_0000;
const SC_MANAGER_CONNECT: u32 = 0x0001;
const SC_MANAGER_CREATE_SERVICE: u32 = 0x0002;
const SC_MANAGER_ENUMERATE_SERVICE: u32 = 0x0004;
const SC_MANAGER_QUERY_LOCK_STATUS: u32 = 0x0010;
const SC_MANAGER_ALL_ACCESS: u32 = 0x000f_003f;
const SERVICE_QUERY_CONFIG: u32 = 0x0001;
const SERVICE_QUERY_STATUS: u32 = 0x0004;
const SERVICE_START: u32 = 0x0010;
const SERVICE_STOP: u32 = 0x0020;

const SERVICE_KERNEL_DRIVER: u32 = 0x01;
const SERVICE_DRIVER: u32 = 0x0b;
const SERVICE_WIN32_OWN_PROCESS: u32 = 0x10;
const SERVICE_WIN32_SHARE_PROCESS: u32 = 0x20;
const SERVICE_WIN32: u32 = 0x30;
const SERVICE_BOOT_START: u32 = 0;
const SERVICE_AUTO_START: u32 = 2;
const SERVICE_DISABLED: u32 = 4;
const SERVICE_ERROR_IGNORE: u32 = 0;
const SERVICE_ACTIVE: u32 = 1;
const SERVICE_INACTIVE: u32 = 2;
const SERVICE_STATE_ALL: u32 = 3;
const SERVICE_STOPPED: u32 = 1;
const SERVICE_RUNNING: u32 = 4;
const SERVICE_CONTROL_STOP: u32 = 1;
const SC_STATUS_PROCESS_INFO: u32 = 0;
const SC_ENUM_PROCESS_INFO: u32 = 0;

struct Line {
    buf: [u8; 160],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 160], n: 0 }
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

    /// A handle-returning call that should have failed with `want`.
    #[track_caller]
    fn no_handle(&mut self, h: Handle, want: u32) -> Result<(), u32> {
        let error = last_error();
        if h != 0 {
            // SAFETY: the handle just returned.
            unsafe { CloseServiceHandle(h) };
        }
        self.ok(h == 0 && error == want, if h != 0 { u64::MAX } else { u64::from(error) })
    }

    /// A handle-returning call that should have succeeded.
    #[track_caller]
    fn handle(&mut self, h: Handle) -> Result<Handle, u32> {
        let error = last_error();
        self.ok(h != 0, u64::from(error))?;
        Ok(h)
    }
}

fn last_error() -> u32 {
    // SAFETY: plain call.
    unsafe { GetLastError() }
}

fn clear_error() {
    // SAFETY: plain call.
    unsafe { SetLastError(0xdead_beef) };
}

/// `text` as a NUL-terminated UTF-16 string (ASCII).
fn wide<const N: usize>(text: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (slot, b) in out.iter_mut().zip(text.bytes()) {
        *slot = u16::from(b);
    }
    out
}

/// The NUL-terminated UTF-16 string at `p` (empty for NULL).
fn units<'a>(p: *const u16) -> &'a [u16] {
    if p.is_null() {
        return &[];
    }
    let mut n = 0;
    // SAFETY: a NUL-terminated string the system wrote.
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

/// The buffer's size: every driver of a Windows machine fits.
const BUF_SIZE: usize = 1 << 18;

/// An 8-byte aligned buffer for the variable-sized answers.
#[repr(C, align(8))]
struct Buf([u8; BUF_SIZE]);

static mut BUF: Buf = Buf([0; BUF_SIZE]);

fn buffer() -> &'static mut [u8; BUF_SIZE] {
    // SAFETY: one thread; each use is done before the next begins.
    unsafe { &mut (*core::ptr::addr_of_mut!(BUF)).0 }
}

/// The test's own service.
const SERVICE: &str = "gala_svc_test";
const DISPLAY: &str = "gala svc_test service";
const PATH: &str = "C:\\gala_svc_test\\none.exe";

/// What a NULL handle and NULL arguments give.
fn null_handles(c: &mut Checks) -> Result<(), u32> {
    let deadbeef = wide::<16>("deadbeef_gala");
    let mut st = ServiceStatus::default();
    let mut sp = StatusProcess::default();
    let mut needed = 0u32;
    let mut returned = 0u32;
    // SAFETY: plain calls with our buffers throughout.
    unsafe {
        clear_error();
        c.fails(CloseServiceHandle(0) != 0, ERROR_INVALID_HANDLE)?;
        clear_error();
        c.no_handle(OpenServiceW(0, null(), 0), ERROR_INVALID_HANDLE)?;
        clear_error();
        c.no_handle(OpenServiceW(0, deadbeef.as_ptr(), GENERIC_READ), ERROR_INVALID_HANDLE)?;
        clear_error();
        c.fails(QueryServiceStatus(0, null_mut()) != 0, ERROR_INVALID_HANDLE)?;
        clear_error();
        c.fails(QueryServiceStatus(0, &mut st) != 0, ERROR_INVALID_HANDLE)?;
        // QueryServiceStatusEx: the level first, then the size, then the
        // handle.
        clear_error();
        c.fails(QueryServiceStatusEx(0, 1, null_mut(), 0, null_mut()) != 0, ERROR_INVALID_LEVEL)?;
        clear_error();
        let r = QueryServiceStatusEx(0, SC_STATUS_PROCESS_INFO, null_mut(), 0, &mut needed);
        c.fails(r != 0, ERROR_INSUFFICIENT_BUFFER)?;
        c.eq(needed.into(), 36)?;
        clear_error();
        let r = QueryServiceStatusEx(0, 0, (&raw mut sp).cast(), 36, &mut needed);
        c.fails(r != 0, ERROR_INVALID_HANDLE)?;
        clear_error();
        c.fails(QueryServiceConfigW(0, null_mut(), 0, &mut needed) != 0, ERROR_INVALID_HANDLE)?;
        clear_error();
        c.fails(DeleteService(0) != 0, ERROR_INVALID_HANDLE)?;
        clear_error();
        c.fails(StartServiceW(0, 0, null()) != 0, ERROR_INVALID_HANDLE)?;
        clear_error();
        c.fails(ControlService(0, SERVICE_CONTROL_STOP, &mut st) != 0, ERROR_INVALID_HANDLE)?;
        clear_error();
        let h = CreateServiceW(
            0,
            null(),
            null(),
            0,
            0,
            0,
            0,
            null(),
            null(),
            null_mut(),
            null(),
            null(),
            null(),
        );
        c.no_handle(h, ERROR_INVALID_HANDLE)?;
        // EnumServicesStatusExW: the level first, then the handle.
        clear_error();
        let r = EnumServicesStatusExW(
            0,
            1,
            0,
            0,
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            null_mut(),
            null(),
        );
        c.fails(r != 0, ERROR_INVALID_LEVEL)?;
        clear_error();
        let r = EnumServicesStatusExW(
            0,
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            null_mut(),
            0,
            &mut needed,
            &mut returned,
            null_mut(),
            null(),
        );
        c.fails(r != 0, ERROR_INVALID_HANDLE)?;
    }
    Ok(())
}

/// OpenSCManagerW's machines, databases and access.
fn open_manager(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain calls with our strings.
    unsafe {
        clear_error();
        let h = c.handle(OpenSCManagerW(null(), null(), 0))?;
        c.ok(CloseServiceHandle(h) != 0, last_error().into())?;
        clear_error();
        let h = OpenSCManagerW(null(), wide::<16>("DoesNotExist").as_ptr(), SC_MANAGER_CONNECT);
        c.no_handle(h, ERROR_INVALID_NAME)?;
        clear_error();
        let h = OpenSCManagerW(null(), wide::<16>("ServicesFailed").as_ptr(), SC_MANAGER_CONNECT);
        c.no_handle(h, ERROR_DATABASE_DOES_NOT_EXIST)?;
        clear_error();
        let active = wide::<16>("ServicesActive");
        let h =
            c.handle(OpenSCManagerW(wide::<1>("").as_ptr(), active.as_ptr(), SC_MANAGER_CONNECT))?;
        c.ok(CloseServiceHandle(h) != 0, last_error().into())?;
        let h = c.handle(OpenSCManagerW(null(), active.as_ptr(), SC_MANAGER_ALL_ACCESS))?;
        c.ok(CloseServiceHandle(h) != 0, last_error().into())?;
    }
    Ok(())
}

/// AFD: a running kernel driver on Windows and gala.
fn afd(c: &mut Checks) -> Result<(), u32> {
    let afd = wide::<4>("AFD");
    let mut st = ServiceStatus::default();
    let mut sp = StatusProcess::default();
    let mut needed = 0u32;
    // SAFETY: plain calls with our buffers throughout.
    unsafe {
        let scm = c.handle(OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT))?;
        clear_error();
        c.no_handle(OpenServiceW(scm, null(), GENERIC_READ), ERROR_INVALID_ADDRESS)?;
        clear_error();
        let deadbeef = wide::<16>("deadbeef_gala");
        c.no_handle(
            OpenServiceW(scm, deadbeef.as_ptr(), GENERIC_READ),
            ERROR_SERVICE_DOES_NOT_EXIST,
        )?;
        // Not the display name either.
        clear_error();
        let display = wide::<48>("Ancillary Function Driver for Winsock");
        c.no_handle(
            OpenServiceW(scm, display.as_ptr(), GENERIC_READ),
            ERROR_SERVICE_DOES_NOT_EXIST,
        )?;

        // Without SERVICE_QUERY_STATUS.
        let h = c.handle(OpenServiceW(scm, afd.as_ptr(), STANDARD_RIGHTS_READ))?;
        clear_error();
        c.fails(QueryServiceStatus(h, null_mut()) != 0, ERROR_INVALID_ADDRESS)?;
        clear_error();
        c.fails(QueryServiceStatus(h, &mut st) != 0, ERROR_ACCESS_DENIED)?;
        clear_error();
        let r = QueryServiceStatusEx(h, SC_STATUS_PROCESS_INFO, null_mut(), 0, &mut needed);
        c.fails(r != 0, ERROR_INSUFFICIENT_BUFFER)?;
        c.eq(needed.into(), 36)?;
        clear_error();
        let r =
            QueryServiceStatusEx(h, SC_STATUS_PROCESS_INFO, (&raw mut sp).cast(), 36, &mut needed);
        c.fails(r != 0, ERROR_ACCESS_DENIED)?;
        c.ok(CloseServiceHandle(h) != 0, last_error().into())?;

        // With it: running, a kernel driver, no process.
        let h = c.handle(OpenServiceW(scm, afd.as_ptr(), SERVICE_QUERY_STATUS))?;
        let r =
            QueryServiceStatusEx(h, SC_STATUS_PROCESS_INFO, (&raw mut sp).cast(), 36, &mut needed);
        c.ok(r != 0, last_error().into())?;
        c.eq(sp.kind.into(), SERVICE_KERNEL_DRIVER.into())?;
        c.eq(sp.state.into(), SERVICE_RUNNING.into())?;
        c.eq(sp.pid.into(), 0)?;
        c.eq(sp.flags.into(), 0)?;
        c.ok(QueryServiceStatus(h, &mut st) != 0, last_error().into())?;
        c.eq(st.kind.into(), SERVICE_KERNEL_DRIVER.into())?;
        c.eq(st.state.into(), SERVICE_RUNNING.into())?;
        // Its configuration needs SERVICE_QUERY_CONFIG.
        let b = buffer();
        clear_error();
        let r = QueryServiceConfigW(h, b.as_mut_ptr(), 8192, &mut needed);
        c.fails(r != 0, ERROR_ACCESS_DENIED)?;
        c.ok(CloseServiceHandle(h) != 0, last_error().into())?;

        let h = c.handle(OpenServiceW(scm, afd.as_ptr(), SERVICE_QUERY_CONFIG))?;
        clear_error();
        let r = QueryServiceConfigW(h, null_mut(), 0, &mut needed);
        c.fails(r != 0, ERROR_INSUFFICIENT_BUFFER)?;
        c.ok(needed as usize > size_of::<Config>(), needed.into())?;
        c.ok(needed <= 8192, needed.into())?;
        let r = QueryServiceConfigW(h, b.as_mut_ptr(), needed, &mut needed);
        c.ok(r != 0, last_error().into())?;
        let config = &*b.as_ptr().cast::<Config>();
        c.eq(config.kind.into(), SERVICE_KERNEL_DRIVER.into())?;
        c.ok(!units(config.path).is_empty(), 0)?;
        c.ok(!units(config.display).is_empty(), 0)?;
        c.ok(!config.group.is_null() && !config.dependencies.is_null(), 0)?;
        c.ok(!config.start_name.is_null(), 0)?;
        c.ok(CloseServiceHandle(h) != 0, last_error().into())?;
        c.ok(CloseServiceHandle(scm) != 0, last_error().into())?;
    }
    Ok(())
}

/// The entries of an EnumServicesStatusExW answer.
fn entries(b: &[u8], returned: u32) -> &[EnumEntry] {
    // SAFETY: the system wrote `returned` entries at the buffer's start
    // (8-byte aligned).
    unsafe { core::slice::from_raw_parts(b.as_ptr().cast::<EnumEntry>(), returned as usize) }
}

/// Enumerates `kind`/`state` into the buffer: how many were returned.
fn enumerate(c: &mut Checks, scm: Handle, kind: u32, state: u32) -> Result<u32, u32> {
    let (mut needed, mut returned, mut resume) = (0u32, 0u32, 0u32);
    // SAFETY: our buffer and counters.
    unsafe {
        clear_error();
        let r = EnumServicesStatusExW(
            scm,
            SC_ENUM_PROCESS_INFO,
            kind,
            state,
            null_mut(),
            0,
            &mut needed,
            &mut returned,
            &mut resume,
            null(),
        );
        c.eq(returned.into(), 0)?;
        if r != 0 {
            // Nothing of that type in that state (gala has no stopped
            // drivers).
            c.eq(needed.into(), 0)?;
            return Ok(0);
        }
        c.eq(last_error().into(), ERROR_MORE_DATA.into())?;
        c.ok(needed > 0 && needed as usize <= BUF_SIZE, needed.into())?;
        let b = buffer();
        let r = EnumServicesStatusExW(
            scm,
            SC_ENUM_PROCESS_INFO,
            kind,
            state,
            b.as_mut_ptr(),
            BUF_SIZE as u32,
            &mut needed,
            &mut returned,
            &mut resume,
            null(),
        );
        c.ok(r != 0, last_error().into())?;
        c.eq(resume.into(), 0)?;
    }
    Ok(returned)
}

/// Whether the buffer's `returned` entries include AFD.
fn has_afd(returned: u32) -> Option<StatusProcess> {
    entries(buffer(), returned).iter().find(|e| same(units(e.name), "AFD")).map(|e| e.status)
}

/// EnumServicesStatusExW's checks and the drivers.
fn enumeration(c: &mut Checks) -> Result<(), u32> {
    let (mut needed, mut returned) = (0xdead_beefu32, 0xdead_beefu32);
    // SAFETY: plain calls with our buffers throughout.
    unsafe {
        let scm = c.handle(OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT))?;
        clear_error();
        let r = EnumServicesStatusExW(
            scm,
            0,
            0,
            0,
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            null_mut(),
            null(),
        );
        c.fails(r != 0, ERROR_INVALID_ADDRESS)?;
        clear_error();
        let r = EnumServicesStatusExW(
            scm,
            0,
            0,
            0,
            null_mut(),
            0,
            &mut needed,
            &mut returned,
            null_mut(),
            null(),
        );
        c.fails(r != 0, ERROR_INVALID_PARAMETER)?;
        c.eq(u64::from(needed) << 32 | u64::from(returned), 0)?;
        (needed, returned) = (0xdead_beef, 0xdead_beef);
        clear_error();
        let r = EnumServicesStatusExW(
            scm,
            0,
            SERVICE_WIN32,
            0,
            null_mut(),
            0,
            &mut needed,
            &mut returned,
            null_mut(),
            null(),
        );
        c.fails(r != 0, ERROR_INVALID_PARAMETER)?;
        // Without SC_MANAGER_ENUMERATE_SERVICE.
        (needed, returned) = (0xdead_beef, 0xdead_beef);
        clear_error();
        let r = EnumServicesStatusExW(
            scm,
            0,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            null_mut(),
            0,
            &mut needed,
            &mut returned,
            null_mut(),
            null(),
        );
        c.fails(r != 0, ERROR_ACCESS_DENIED)?;
        c.eq(u64::from(needed) << 32 | u64::from(returned), 0)?;
        c.ok(CloseServiceHandle(scm) != 0, last_error().into())?;

        let scm = c.handle(OpenSCManagerW(null(), null(), SC_MANAGER_ENUMERATE_SERVICE))?;
        (needed, returned) = (0xdead_beef, 0xdead_beef);
        clear_error();
        let group = wide::<16>("deadbeef_group");
        let r = EnumServicesStatusExW(
            scm,
            0,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            null_mut(),
            0,
            &mut needed,
            &mut returned,
            null_mut(),
            group.as_ptr(),
        );
        c.fails(r != 0, ERROR_SERVICE_DOES_NOT_EXIST)?;
        c.eq(u64::from(needed) << 32 | u64::from(returned), 0)?;

        // The drivers: AFD running, without a process; every entry named,
        // every type a driver's.
        let n = enumerate(c, scm, SERVICE_DRIVER, SERVICE_STATE_ALL)?;
        c.ok(n > 0, 0)?;
        for e in entries(buffer(), n) {
            c.ok(!units(e.name).is_empty(), 0)?;
            c.ok(!e.display.is_null(), 0)?;
            c.ok(e.status.kind & SERVICE_DRIVER != 0, e.status.kind.into())?;
        }
        let Some(status) = has_afd(n) else { return c.ok(false, n.into()) };
        c.eq(status.kind.into(), SERVICE_KERNEL_DRIVER.into())?;
        c.eq(status.state.into(), SERVICE_RUNNING.into())?;
        c.eq(status.pid.into(), 0)?;
        let n = enumerate(c, scm, SERVICE_DRIVER, SERVICE_ACTIVE)?;
        c.ok(has_afd(n).is_some(), n.into())?;
        for e in entries(buffer(), n) {
            c.ok(e.status.state != SERVICE_STOPPED, e.status.state.into())?;
        }
        let n = enumerate(c, scm, SERVICE_DRIVER, SERVICE_INACTIVE)?;
        c.ok(has_afd(n).is_none(), n.into())?;
        for e in entries(buffer(), n) {
            c.eq(e.status.state.into(), SERVICE_STOPPED.into())?;
        }
        c.ok(CloseServiceHandle(scm) != 0, last_error().into())?;
    }
    Ok(())
}

/// CreateServiceW with the test's name, path and display name and the
/// given access, type and start type.
fn create(
    scm: Handle,
    name: *const u16,
    path: *const u16,
    access: u32,
    kind: u32,
    start: u32,
) -> Handle {
    let display = wide::<32>(DISPLAY);
    // SAFETY: our strings.
    unsafe {
        CreateServiceW(
            scm,
            name,
            display.as_ptr(),
            access,
            kind,
            start,
            SERVICE_ERROR_IGNORE,
            path,
            null(),
            null_mut(),
            null(),
            null(),
            null(),
        )
    }
}

/// Creating, querying and deleting the test's own service.
fn own_service(c: &mut Checks) -> Result<(), u32> {
    let (name, path) = (wide::<16>(SERVICE), wide::<32>(PATH));
    let (n, p, empty) = (name.as_ptr(), path.as_ptr(), wide::<1>(""));
    let mut st = ServiceStatus::default();
    let mut sp = StatusProcess::default();
    let mut needed = 0u32;
    const OWN: u32 = SERVICE_WIN32_OWN_PROCESS;
    // SAFETY: plain calls with our buffers throughout.
    unsafe {
        // With SC_MANAGER_CONNECT only: the NULL pointers, then the access.
        let scm = c.handle(OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT))?;
        clear_error();
        c.no_handle(create(scm, null(), null(), 0, 0, 0), ERROR_INVALID_ADDRESS)?;
        clear_error();
        c.no_handle(create(scm, n, null(), 0, 0, 0), ERROR_INVALID_ADDRESS)?;
        clear_error();
        c.no_handle(create(scm, null(), p, 0, 0, 0), ERROR_INVALID_ADDRESS)?;
        clear_error();
        c.no_handle(create(scm, n, p, 0, 0, 0), ERROR_ACCESS_DENIED)?;
        clear_error();
        c.no_handle(create(scm, empty.as_ptr(), p, 0, 0, 0), ERROR_ACCESS_DENIED)?;
        c.ok(CloseServiceHandle(scm) != 0, last_error().into())?;

        // A leftover from an earlier run that ended early.
        let all = c.handle(OpenSCManagerW(null(), null(), SC_MANAGER_ALL_ACCESS))?;
        let old = OpenServiceW(all, n, DELETE);
        if old != 0 {
            DeleteService(old);
            CloseServiceHandle(old);
        }
        c.ok(CloseServiceHandle(all) != 0, last_error().into())?;

        let scm = c.handle(OpenSCManagerW(null(), null(), SC_MANAGER_CREATE_SERVICE))?;
        clear_error();
        c.no_handle(create(scm, empty.as_ptr(), p, 0, 0, 0), ERROR_INVALID_NAME)?;
        clear_error();
        c.no_handle(create(scm, n, empty.as_ptr(), 0, 0, 0), ERROR_INVALID_PARAMETER)?;
        clear_error();
        c.no_handle(
            create(scm, n, empty.as_ptr(), 0, OWN, SERVICE_DISABLED),
            ERROR_INVALID_PARAMETER,
        )?;
        clear_error();
        let mixed = OWN | SERVICE_WIN32_SHARE_PROCESS;
        c.no_handle(create(scm, n, p, 0, mixed, SERVICE_DISABLED), ERROR_INVALID_PARAMETER)?;
        clear_error();
        let start = SERVICE_AUTO_START | SERVICE_DISABLED;
        c.no_handle(create(scm, n, p, 0, OWN, start), ERROR_INVALID_PARAMETER)?;
        clear_error();
        c.no_handle(create(scm, n, p, 0, OWN, SERVICE_BOOT_START), ERROR_INVALID_PARAMETER)?;

        // Created disabled, with no access asked for.
        clear_error();
        let h = c.handle(create(scm, n, p, 0, OWN, SERVICE_DISABLED))?;
        clear_error();
        c.no_handle(create(scm, n, p, 0, OWN, SERVICE_DISABLED), ERROR_SERVICE_EXISTS)?;
        clear_error();
        c.fails(DeleteService(h) != 0, ERROR_ACCESS_DENIED)?;
        c.ok(CloseServiceHandle(h) != 0, last_error().into())?;

        let access =
            SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS | SERVICE_START | SERVICE_STOP | DELETE;
        let h = c.handle(OpenServiceW(scm, n, access))?;
        // Its configuration: as given, LocalSystem, no group, no
        // dependencies.
        let b = buffer();
        clear_error();
        c.fails(
            QueryServiceConfigW(h, null_mut(), 0, &mut needed) != 0,
            ERROR_INSUFFICIENT_BUFFER,
        )?;
        c.ok(needed as usize > size_of::<Config>() && needed <= 8192, needed.into())?;
        let r = QueryServiceConfigW(h, b.as_mut_ptr(), needed, &mut needed);
        c.ok(r != 0, last_error().into())?;
        let config = &*b.as_ptr().cast::<Config>();
        c.eq(config.kind.into(), OWN.into())?;
        c.eq(config.start.into(), SERVICE_DISABLED.into())?;
        c.eq(config.error_control.into(), SERVICE_ERROR_IGNORE.into())?;
        c.ok(same(units(config.path), PATH), 0)?;
        c.ok(!config.group.is_null() && units(config.group).is_empty(), 0)?;
        c.eq(config.tag.into(), 0)?;
        c.ok(!config.dependencies.is_null() && *config.dependencies == 0, 0)?;
        c.ok(same(units(config.start_name), "LocalSystem"), 0)?;
        c.ok(same(units(config.display), DISPLAY), 0)?;
        // A buffer one byte short.
        clear_error();
        let r = QueryServiceConfigW(h, b.as_mut_ptr(), needed - 1, &mut needed);
        c.fails(r != 0, ERROR_INSUFFICIENT_BUFFER)?;

        // Its status: stopped, never started.
        let r =
            QueryServiceStatusEx(h, SC_STATUS_PROCESS_INFO, (&raw mut sp).cast(), 36, &mut needed);
        c.ok(r != 0, last_error().into())?;
        c.eq(sp.kind.into(), OWN.into())?;
        c.eq(sp.state.into(), SERVICE_STOPPED.into())?;
        c.eq(sp.controls.into(), 0)?;
        c.eq(sp.exit_code.into(), ERROR_SERVICE_NEVER_STARTED.into())?;
        c.eq(sp.pid.into(), 0)?;
        clear_error();
        c.fails(StartServiceW(h, 0, null()) != 0, ERROR_SERVICE_DISABLED)?;
        clear_error();
        c.fails(ControlService(h, SERVICE_CONTROL_STOP, &mut st) != 0, ERROR_SERVICE_NOT_ACTIVE)?;
        c.eq(st.state.into(), SERVICE_STOPPED.into())?;

        // Listed among the inactive programs' services.
        let e = c.handle(OpenSCManagerW(null(), null(), SC_MANAGER_ENUMERATE_SERVICE))?;
        let count = enumerate(c, e, SERVICE_WIN32, SERVICE_INACTIVE)?;
        let ours = entries(buffer(), count).iter().find(|x| same(units(x.name), SERVICE));
        let Some(ours) = ours else { return c.ok(false, count.into()) };
        c.ok(same(units(ours.display), DISPLAY), 0)?;
        c.eq(ours.status.kind.into(), OWN.into())?;
        c.eq(ours.status.state.into(), SERVICE_STOPPED.into())?;
        c.ok(CloseServiceHandle(e) != 0, last_error().into())?;

        // Deleted: marked while a handle is open; it still opens.
        c.ok(DeleteService(h) != 0, last_error().into())?;
        clear_error();
        c.fails(DeleteService(h) != 0, ERROR_SERVICE_MARKED_FOR_DELETE)?;
        let again = c.handle(OpenServiceW(scm, n, GENERIC_READ))?;
        c.ok(CloseServiceHandle(again) != 0, last_error().into())?;
        clear_error();
        c.no_handle(create(scm, n, p, 0, OWN, SERVICE_DISABLED), ERROR_SERVICE_MARKED_FOR_DELETE)?;
        c.ok(CloseServiceHandle(h) != 0, last_error().into())?;
        c.ok(CloseServiceHandle(scm) != 0, last_error().into())?;
    }
    Ok(())
}

/// A standard user (`galatest`): connect and enumerate, query, no more.
/// Err(0) when the account is not there.
fn standard_user(c: &mut Checks) -> Result<(), u32> {
    let (user, dot, password) = (wide::<9>("galatest"), wide::<2>("."), wide::<12>("Gala-Test-1"));
    let mut token = 0;
    // SAFETY: plain calls with our buffers throughout.
    unsafe {
        let r = LogonUserW(user.as_ptr(), dot.as_ptr(), password.as_ptr(), 2, 0, &mut token);
        if r == 0 {
            let error = last_error();
            c.eq(error.into(), ERROR_LOGON_FAILURE.into())?;
            return Err(0);
        }
        c.ok(ImpersonateLoggedOnUser(token) != 0, last_error().into())?;
        let result = standard_user_checks(c);
        let reverted = RevertToSelf() != 0;
        CloseHandle(token);
        result?;
        c.ok(reverted, last_error().into())?;
    }
    Ok(())
}

fn standard_user_checks(c: &mut Checks) -> Result<(), u32> {
    let afd = wide::<4>("AFD");
    let mut sp = StatusProcess::default();
    let mut needed = 0u32;
    // SAFETY: plain calls with our buffers throughout.
    unsafe {
        clear_error();
        c.no_handle(OpenSCManagerW(null(), null(), SC_MANAGER_ALL_ACCESS), ERROR_ACCESS_DENIED)?;
        clear_error();
        c.no_handle(
            OpenSCManagerW(null(), null(), SC_MANAGER_CREATE_SERVICE),
            ERROR_ACCESS_DENIED,
        )?;
        let access =
            SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE | SC_MANAGER_QUERY_LOCK_STATUS;
        let scm = c.handle(OpenSCManagerW(null(), null(), access))?;
        let n = enumerate(c, scm, SERVICE_DRIVER, SERVICE_STATE_ALL)?;
        c.ok(has_afd(n).is_some(), n.into())?;
        clear_error();
        c.no_handle(OpenServiceW(scm, afd.as_ptr(), SERVICE_STOP), ERROR_ACCESS_DENIED)?;
        let h = c.handle(OpenServiceW(scm, afd.as_ptr(), SERVICE_QUERY_STATUS))?;
        let r =
            QueryServiceStatusEx(h, SC_STATUS_PROCESS_INFO, (&raw mut sp).cast(), 36, &mut needed);
        c.ok(r != 0, last_error().into())?;
        c.eq(sp.state.into(), SERVICE_RUNNING.into())?;
        c.ok(CloseServiceHandle(h) != 0, last_error().into())?;
        c.ok(CloseServiceHandle(scm) != 0, last_error().into())?;
    }
    Ok(())
}

fn run() -> Result<bool, u32> {
    let mut c = Checks(0);
    null_handles(&mut c)?;
    open_manager(&mut c)?;
    afd(&mut c)?;
    enumeration(&mut c)?;
    own_service(&mut c)?;
    match standard_user(&mut c) {
        Ok(()) => Ok(true),
        Err(0) => Ok(false),
        Err(n) => Err(n),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(true) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
            0
        }
        Ok(false) => {
            Line::new()
                .s(NAME.as_bytes())
                .s(b": ok (the standard user's part skipped: no galatest account)\r\n")
                .print();
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
