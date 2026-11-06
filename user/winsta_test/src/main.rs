//! Window stations and desktops (ROADMAP RS step B3a), with the results
//! real Windows gives a program started in the console's session (and,
//! with the argument `service`, one run as SYSTEM in session 0, as
//! services are: `psexec -s` on Windows, `as=system` on gala). Expected
//! values come from Microsoft's documentation ("Process Connection to a
//! Window Station", GetUserObjectInformation, USEROBJECTFLAGS) and Wine's
//! conformance tests (dlls/user32/tests/winstation.c), which are checked
//! on Windows.
//!
//! - GetProcessWindowStation: a handle, the same each time, to
//!   "WinSta0" (UOI_NAME), type "WindowStation" (UOI_TYPE), visible
//!   (USEROBJECTFLAGS.dwFlags has WSF_VISIBLE), not inheritable; granted
//!   STANDARD_RIGHTS_REQUIRED | WINSTA_ALL_ACCESS. As a service:
//!   "Service-0x0-3e7$" (the SYSTEM logon session's LUID), not visible.
//! - It cannot be closed: CloseWindowStation fails, CloseHandle fails
//!   with ERROR_INVALID_HANDLE; a duplicate of it closes either way.
//! - GetUserObjectInformationW sizes: the bytes with the NUL, and
//!   ERROR_INSUFFICIENT_BUFFER when short (also for UOI_FLAGS' 12
//!   bytes). The A form reports the UTF-16 size when short, the ANSI
//!   bytes on success.
//! - OpenWindowStationW/A: a new handle to WinSta0 (any case); an
//!   unknown name ERROR_FILE_NOT_FOUND, "" too, a name with a backslash
//!   ERROR_PATH_NOT_FOUND. CloseDesktop of a window station handle and
//!   CloseWindowStation of a desktop handle fail with
//!   ERROR_INVALID_HANDLE.
//! - EnumWindowStationsW lists WinSta0 and returns the callback's value.
//! - GetThreadDesktop: "Default", type "Desktop", the same handle each
//!   time and for every thread of the process (a second thread's too);
//!   granted STANDARD_RIGHTS_REQUIRED | DESKTOP_ALL_ACCESS; CloseDesktop
//!   fails with ERROR_BUSY, CloseHandle with ERROR_INVALID_HANDLE.
//! - OpenDesktopW("Default"): a new handle; "" is ERROR_INVALID_HANDLE,
//!   "foo\bar" ERROR_BAD_PATHNAME, an unknown name fails.
//! - OpenInputDesktop: a new handle each call, "Default", UOI_IO TRUE.
//! - SetThreadDesktop of another handle to the same desktop: the thread's
//!   desktop handle is then that one (CloseDesktop of it ERROR_BUSY);
//!   set back, it closes.
//! - SetProcessWindowStation: GetProcessWindowStation returns the handle
//!   set (which then cannot be closed); set back, it closes.
//! - Child processes (`child`) started with STARTUPINFO.lpDesktop
//!   "winsta0\default" and with none (inherited) are on WinSta0\Default.
//!
//! Prints `winsta_test: ok` or exits with the number of the failing
//! check (a child: 100 plus its check's).

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetCurrentProcess() -> Handle;
    fn GetCurrentThreadId() -> u32;
    fn DuplicateHandle(
        source_process: Handle,
        source: Handle,
        target_process: Handle,
        target: *mut Handle,
        access: u32,
        inherit: Bool,
        options: u32,
    ) -> Bool;
    fn CreateThread(
        sa: *mut c_void,
        stack: usize,
        start: unsafe extern "system" fn(*mut c_void) -> u32,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeThread(h: Handle, code: *mut u32) -> Bool;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn GetCommandLineW() -> *const u16;
    fn GetModuleFileNameW(module: Handle, buf: *mut u16, size: u32) -> u32;
    fn CreateProcessW(
        application: *const u16,
        command_line: *mut u16,
        process_attributes: *mut c_void,
        thread_attributes: *mut c_void,
        inherit: Bool,
        flags: u32,
        environment: *mut c_void,
        directory: *const u16,
        startup: *const c_void,
        info: *mut usize,
    ) -> Bool;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQueryObject(h: Handle, class: u32, buf: *mut u8, len: u32, ret: *mut u32) -> u32;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessWindowStation() -> Handle;
    fn SetProcessWindowStation(station: Handle) -> Bool;
    fn OpenWindowStationW(name: *const u16, inherit: Bool, access: u32) -> Handle;
    fn OpenWindowStationA(name: *const u8, inherit: Bool, access: u32) -> Handle;
    fn CloseWindowStation(station: Handle) -> Bool;
    fn EnumWindowStationsW(
        callback: unsafe extern "system" fn(*const u16, isize) -> Bool,
        param: isize,
    ) -> Bool;
    fn GetThreadDesktop(tid: u32) -> Handle;
    fn SetThreadDesktop(desktop: Handle) -> Bool;
    fn OpenDesktopW(name: *const u16, flags: u32, inherit: Bool, access: u32) -> Handle;
    fn OpenInputDesktop(flags: u32, inherit: Bool, access: u32) -> Handle;
    fn CloseDesktop(desktop: Handle) -> Bool;
    fn GetUserObjectInformationW(
        h: Handle,
        index: i32,
        info: *mut c_void,
        len: u32,
        needed: *mut u32,
    ) -> Bool;
    fn GetUserObjectInformationA(
        h: Handle,
        index: i32,
        info: *mut c_void,
        len: u32,
        needed: *mut u32,
    ) -> Bool;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn __chkstk();
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const UOI_FLAGS: i32 = 1;
const UOI_NAME: i32 = 2;
const UOI_TYPE: i32 = 3;
const UOI_IO: i32 = 6;
const WSF_VISIBLE: u32 = 1;
const WINSTA_ALL_ACCESS: u32 = 0x37f;
const WINSTA_ENUMERATE: u32 = 0x100;
const DESKTOP_ALL_ACCESS: u32 = 0x1ff;
const STANDARD_RIGHTS_REQUIRED: u32 = 0xf_0000;
const DUPLICATE_SAME_ACCESS: u32 = 2;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_BAD_PATHNAME: u32 = 161;
const ERROR_BUSY: u32 = 170;
const OBJECT_BASIC_INFORMATION: u32 = 0;

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
}

fn hex(v: u32) -> [u8; 8] {
    let mut s = [0u8; 8];
    for (i, c) in s.iter_mut().enumerate() {
        let d = (v >> (28 - 4 * i)) & 0xf;
        *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
    }
    s
}

struct Checks(u32);

impl Checks {
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        // One write per line (the kernel's log shares the port).
        let mut line = [0u8; 48];
        let head = b"winsta_test: check ";
        line[..head.len()].copy_from_slice(head);
        let mut n = head.len();
        line[n..n + 8].copy_from_slice(&hex(self.0));
        n += 8;
        line[n..n + 5].copy_from_slice(b" got ");
        n += 5;
        line[n..n + 8].copy_from_slice(&hex(got as u32));
        n += 8;
        line[n..n + 2].copy_from_slice(b"\r\n");
        out(&line[..n + 2]);
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// The call failed (returned 0) with last error `error`.
    fn fails(&mut self, result: u64, error: u32) -> Result<(), u32> {
        // SAFETY: a plain call.
        let e = unsafe { GetLastError() };
        self.ok(result == 0, result)?;
        self.eq(e.into(), error.into())
    }
}

/// A NUL-terminated UTF-16 copy of an ASCII string.
fn w(s: &str) -> [u16; 64] {
    let mut b = [0u16; 64];
    for (d, c) in b.iter_mut().zip(s.bytes()) {
        *d = c.into();
    }
    b
}

/// Whether the NUL-terminated UTF-16 string at `p` is `want`.
fn is(p: *const u16, want: &str) -> bool {
    // SAFETY: a NUL-terminated string the system wrote.
    unsafe {
        want.bytes().enumerate().all(|(i, b)| *p.add(i) == u16::from(b)) && *p.add(want.len()) == 0
    }
}

fn last_error() -> u64 {
    // SAFETY: a plain call.
    unsafe { GetLastError() }.into()
}

/// UOI_NAME or UOI_TYPE of `h` is `want` (W form, with the size the
/// call reports both when short and when it fits).
fn check_text(c: &mut Checks, h: Handle, index: i32, want: &str) -> Result<(), u32> {
    let size = 2 * (want.len() as u32 + 1);
    let mut needed = 0;
    let mut buf = [0u16; 64];
    // SAFETY: plain calls with our buffers.
    unsafe {
        SetLastError(0);
        let r = GetUserObjectInformationW(h, index, core::ptr::null_mut(), 0, &mut needed);
        c.fails(r as u64, ERROR_INSUFFICIENT_BUFFER)?;
        c.eq(needed.into(), size.into())?;
        // One byte short.
        let r = GetUserObjectInformationW(h, index, buf.as_mut_ptr().cast(), size - 1, &mut needed);
        c.fails(r as u64, ERROR_INSUFFICIENT_BUFFER)?;
        c.eq(needed.into(), size.into())?;
        needed = 0;
        let r = GetUserObjectInformationW(h, index, buf.as_mut_ptr().cast(), 128, &mut needed);
        c.ok(r != 0, last_error())?;
        c.eq(needed.into(), size.into())?;
        c.ok(is(buf.as_ptr(), want), buf[0].into())
    }
}

/// USEROBJECTFLAGS of `h`: (fInherit, dwFlags).
fn flags_of(c: &mut Checks, h: Handle) -> Result<(u32, u32), u32> {
    let mut flags = [0u32; 3];
    let mut needed = 0;
    // SAFETY: plain calls with our buffer.
    unsafe {
        let r = GetUserObjectInformationW(h, UOI_FLAGS, flags.as_mut_ptr().cast(), 8, &mut needed);
        c.fails(r as u64, ERROR_INSUFFICIENT_BUFFER)?;
        c.eq(needed.into(), 12)?;
        let r = GetUserObjectInformationW(h, UOI_FLAGS, flags.as_mut_ptr().cast(), 12, &mut needed);
        c.ok(r != 0, last_error())?;
        c.eq(needed.into(), 12)?;
    }
    Ok((flags[0], flags[2]))
}

/// The access a handle grants (OBJECT_BASIC_INFORMATION.GrantedAccess).
fn granted(h: Handle) -> u64 {
    let mut info = [0u32; 14];
    // SAFETY: a 56-byte buffer.
    let status = unsafe {
        NtQueryObject(
            h,
            OBJECT_BASIC_INFORMATION,
            info.as_mut_ptr().cast(),
            56,
            core::ptr::null_mut(),
        )
    };
    if status != 0 { u64::from(status) << 32 } else { info[1].into() }
}

static mut SEEN_WINSTA0: bool = false;

unsafe extern "system" fn station_callback(name: *const u16, param: isize) -> Bool {
    if is(name, "WinSta0") {
        // SAFETY: the main thread's flag; the callback runs on it.
        unsafe { SEEN_WINSTA0 = true };
    }
    param as Bool
}

/// The second thread: its desktop handle is the process's (the main
/// thread's) one, and it cannot be closed here either.
unsafe extern "system" fn second_thread(param: *mut c_void) -> u32 {
    // SAFETY: plain calls.
    unsafe {
        let d = GetThreadDesktop(GetCurrentThreadId());
        if d != param {
            return 1;
        }
        SetLastError(0);
        if CloseDesktop(d) != 0 || GetLastError() != ERROR_BUSY {
            return 2;
        }
    }
    0
}

/// The station and the thread's desktop a child (or the service run)
/// is on: their names.
fn check_connection(c: &mut Checks, station: &str, visible: bool) -> Result<(), u32> {
    // SAFETY: plain calls.
    let (s, d) = unsafe { (GetProcessWindowStation(), GetThreadDesktop(GetCurrentThreadId())) };
    c.ok(!s.is_null(), last_error())?;
    c.ok(!d.is_null(), last_error())?;
    check_text(c, s, UOI_NAME, station)?;
    check_text(c, s, UOI_TYPE, "WindowStation")?;
    check_text(c, d, UOI_NAME, "Default")?;
    let (inherit, flags) = flags_of(c, s)?;
    c.eq(inherit.into(), 0)?;
    c.eq((flags & WSF_VISIBLE).into(), u64::from(visible))
}

/// Starts this program as `<exe> child` with STARTUPINFO.lpDesktop
/// `desktop` (none: inherited) and returns its exit code.
fn run_child(c: &mut Checks, desktop: Option<&str>) -> Result<u32, u32> {
    let mut exe = [0u16; 260];
    let mut line = [0u16; 300];
    // SAFETY: plain calls with our buffers.
    unsafe {
        let n = GetModuleFileNameW(core::ptr::null_mut(), exe.as_mut_ptr(), 260) as usize;
        c.ok(n > 0 && n < 260, n as u64)?;
        line[0] = u16::from(b'"');
        line[1..=n].copy_from_slice(&exe[..n]);
        for (i, b) in b"\" child".iter().enumerate() {
            line[n + 1 + i] = u16::from(*b);
        }
        let name = w(desktop.unwrap_or(""));
        let mut startup = [0usize; 13];
        startup[0] = 104;
        if desktop.is_some() {
            startup[2] = name.as_ptr() as usize;
        }
        let mut info = [0usize; 3];
        let null = core::ptr::null_mut();
        let r = CreateProcessW(
            exe.as_ptr(),
            line.as_mut_ptr(),
            null,
            null,
            0,
            0,
            null,
            core::ptr::null(),
            startup.as_ptr().cast(),
            info.as_mut_ptr(),
        );
        c.ok(r != 0, last_error())?;
        let process = info[0] as Handle;
        c.eq(WaitForSingleObject(process, 30_000).into(), 0)?;
        let mut code = 0;
        GetExitCodeProcess(process, &mut code);
        CloseHandle(process);
        CloseHandle(info[1] as Handle);
        Ok(code)
    }
}

/// The command line's word after the program (`child`, `service`), if
/// any.
fn mode() -> u8 {
    // SAFETY: the process's command line, NUL-terminated.
    let line = unsafe {
        let p = GetCommandLineW();
        core::slice::from_raw_parts(p, (0..).take_while(|&i| *p.add(i) != 0).count())
    };
    let ends = |word: &str| {
        let n = word.len();
        line.len() >= n && line[line.len() - n..].iter().copied().eq(word.encode_utf16())
    };
    if ends(" child") {
        b'c'
    } else if ends(" service") {
        b's'
    } else {
        b'i'
    }
}

fn interactive(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: plain calls with our buffers; handles we got.
    unsafe {
        // The process's window station.
        let w1 = GetProcessWindowStation();
        c.ok(!w1.is_null(), last_error())?;
        c.eq(GetProcessWindowStation() as u64, w1 as u64)?;
        check_text(c, w1, UOI_NAME, "WinSta0")?;
        check_text(c, w1, UOI_TYPE, "WindowStation")?;
        let (inherit, flags) = flags_of(c, w1)?;
        c.eq(inherit.into(), 0)?;
        c.eq((flags & WSF_VISIBLE).into(), WSF_VISIBLE.into())?;
        c.eq(granted(w1), (STANDARD_RIGHTS_REQUIRED | WINSTA_ALL_ACCESS).into())?;
        // The A form: the UTF-16 size when short, the ANSI bytes when not.
        let mut needed = 0;
        let mut name = [0u8; 32];
        let r = GetUserObjectInformationA(w1, UOI_NAME, core::ptr::null_mut(), 0, &mut needed);
        c.fails(r as u64, ERROR_INSUFFICIENT_BUFFER)?;
        c.eq(needed.into(), 16)?;
        let r = GetUserObjectInformationA(w1, UOI_NAME, name.as_mut_ptr().cast(), 32, &mut needed);
        c.ok(r != 0, last_error())?;
        c.eq(needed.into(), 8)?;
        c.ok(&name[..8] == b"WinSta0\0", name[0].into())?;
        // It cannot be closed; a duplicate can.
        c.eq(CloseWindowStation(w1) as u64, 0)?;
        SetLastError(0);
        c.fails(CloseHandle(w1) as u64, ERROR_INVALID_HANDLE)?;
        c.eq(GetProcessWindowStation() as u64, w1 as u64)?;
        let mut w2 = core::ptr::null_mut();
        let me = GetCurrentProcess();
        c.ok(DuplicateHandle(me, w1, me, &mut w2, 0, 0, DUPLICATE_SAME_ACCESS) != 0, 0)?;
        c.ok(CloseWindowStation(w2) != 0, last_error())?;
        c.ok(DuplicateHandle(me, w1, me, &mut w2, 0, 0, DUPLICATE_SAME_ACCESS) != 0, 0)?;
        c.ok(CloseHandle(w2) != 0, last_error())?;

        // Opening window stations by name.
        let w2 = OpenWindowStationW(w("winsta0").as_ptr(), 0, WINSTA_ALL_ACCESS);
        c.ok(!w2.is_null(), last_error())?;
        c.ok(w2 != w1, 0)?;
        check_text(c, w2, UOI_NAME, "WinSta0")?;
        let w3 = OpenWindowStationA(c"WinSta0".as_ptr().cast(), 0, WINSTA_ENUMERATE);
        c.ok(!w3.is_null(), last_error())?;
        c.ok(CloseWindowStation(w3) != 0, last_error())?;
        let r = OpenWindowStationW(w("dummy name").as_ptr(), 0, WINSTA_ALL_ACCESS);
        c.fails(r as u64, ERROR_FILE_NOT_FOUND)?;
        let r = OpenWindowStationW(w("").as_ptr(), 0, WINSTA_ALL_ACCESS);
        c.fails(r as u64, ERROR_FILE_NOT_FOUND)?;
        let r = OpenWindowStationW(w("foo\\bar").as_ptr(), 0, WINSTA_ALL_ACCESS);
        c.fails(r as u64, ERROR_PATH_NOT_FOUND)?;
        // Listing them.
        SetLastError(0);
        c.eq(EnumWindowStationsW(station_callback, 0x12345) as u64, 0x12345)?;
        c.ok(SEEN_WINSTA0, 0)?;

        // The thread's desktop.
        let d1 = GetThreadDesktop(GetCurrentThreadId());
        c.ok(!d1.is_null(), last_error())?;
        c.eq(GetThreadDesktop(GetCurrentThreadId()) as u64, d1 as u64)?;
        check_text(c, d1, UOI_NAME, "Default")?;
        check_text(c, d1, UOI_TYPE, "Desktop")?;
        c.eq(granted(d1), (STANDARD_RIGHTS_REQUIRED | DESKTOP_ALL_ACCESS).into())?;
        let (inherit, _) = flags_of(c, d1)?;
        c.eq(inherit.into(), 0)?;
        SetLastError(0);
        c.fails(CloseDesktop(d1) as u64, ERROR_BUSY)?;
        SetLastError(0);
        c.fails(CloseHandle(d1) as u64, ERROR_INVALID_HANDLE)?;
        c.eq(GetThreadDesktop(GetCurrentThreadId()) as u64, d1 as u64)?;
        // The wrong kind of handle.
        SetLastError(0);
        c.fails(CloseDesktop(w2) as u64, ERROR_INVALID_HANDLE)?;
        SetLastError(0);
        c.fails(CloseWindowStation(d1) as u64, ERROR_INVALID_HANDLE)?;
        // A second thread is on the same desktop, through the same handle.
        let mut tid = 0;
        let t = CreateThread(core::ptr::null_mut(), 0, second_thread, d1, 0, &mut tid);
        c.ok(!t.is_null(), last_error())?;
        c.eq(WaitForSingleObject(t, 30_000).into(), 0)?;
        let mut code = 99;
        GetExitCodeThread(t, &mut code);
        c.eq(code.into(), 0)?;
        CloseHandle(t);

        // Opening desktops by name (in the process's window station).
        let d2 = OpenDesktopW(w("Default").as_ptr(), 0, 0, DESKTOP_ALL_ACCESS);
        c.ok(!d2.is_null(), last_error())?;
        c.ok(d2 != d1, 0)?;
        check_text(c, d2, UOI_NAME, "Default")?;
        c.ok(CloseDesktop(d2) != 0, last_error())?;
        let r = OpenDesktopW(w("dummy name").as_ptr(), 0, 0, DESKTOP_ALL_ACCESS);
        c.eq(r as u64, 0)?;
        let r = OpenDesktopW(w("").as_ptr(), 0, 0, DESKTOP_ALL_ACCESS);
        c.fails(r as u64, ERROR_INVALID_HANDLE)?;
        let r = OpenDesktopW(w("foo\\bar").as_ptr(), 0, 0, DESKTOP_ALL_ACCESS);
        c.fails(r as u64, ERROR_BAD_PATHNAME)?;

        // The input desktop: a new handle each time.
        let i1 = OpenInputDesktop(0, 0, DESKTOP_ALL_ACCESS);
        c.ok(!i1.is_null(), last_error())?;
        let i2 = OpenInputDesktop(0, 0, DESKTOP_ALL_ACCESS);
        c.ok(!i2.is_null(), last_error())?;
        c.ok(i1 != i2 && i1 != d1, 0)?;
        check_text(c, i1, UOI_NAME, "Default")?;
        let mut io = 0u32;
        let r = GetUserObjectInformationW(i1, UOI_IO, (&raw mut io).cast(), 4, &mut needed);
        c.ok(r != 0, last_error())?;
        c.eq(io.into(), 1)?;
        c.ok(CloseDesktop(i2) != 0, last_error())?;

        // SetThreadDesktop with another handle to the same desktop.
        c.ok(SetThreadDesktop(d1) != 0, last_error())?;
        c.ok(SetThreadDesktop(i1) != 0, last_error())?;
        c.eq(GetThreadDesktop(GetCurrentThreadId()) as u64, i1 as u64)?;
        SetLastError(0);
        c.fails(CloseDesktop(i1) as u64, ERROR_BUSY)?;
        c.ok(SetThreadDesktop(d1) != 0, last_error())?;
        c.eq(GetThreadDesktop(GetCurrentThreadId()) as u64, d1 as u64)?;
        c.ok(CloseDesktop(i1) != 0, last_error())?;
        SetLastError(0);
        c.fails(SetThreadDesktop(w2) as u64, ERROR_INVALID_HANDLE)?;

        // SetProcessWindowStation.
        c.ok(SetProcessWindowStation(w2) != 0, last_error())?;
        c.eq(GetProcessWindowStation() as u64, w2 as u64)?;
        c.eq(CloseWindowStation(w2) as u64, 0)?;
        c.ok(SetProcessWindowStation(w1) != 0, last_error())?;
        c.eq(GetProcessWindowStation() as u64, w1 as u64)?;
        c.ok(CloseWindowStation(w2) != 0, last_error())?;
    }

    // Children: lpDesktop names WinSta0\Default (any case), or nothing
    // (inherited).
    let code = run_child(c, Some("winsta0\\default"))?;
    c.eq(code.into(), 0)?;
    let code = run_child(c, None)?;
    c.eq(code.into(), 0)
}

fn service(c: &mut Checks) -> Result<(), u32> {
    // A SYSTEM service's window station: named after the SYSTEM logon
    // session (0x3e7), not visible.
    check_connection(c, "Service-0x0-3e7$", false)?;
    // Session 0 has a WinSta0 too.
    // SAFETY: plain calls.
    unsafe {
        let w0 = OpenWindowStationW(w("WinSta0").as_ptr(), 0, WINSTA_ENUMERATE);
        c.ok(!w0.is_null(), last_error())?;
        c.ok(CloseWindowStation(w0) != 0, last_error())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let mut c = Checks(0);
    let code = match mode() {
        b'c' => match check_connection(&mut c, "WinSta0", true) {
            Ok(()) => 0,
            Err(id) => 100 + id,
        },
        m => {
            let r = if m == b's' { service(&mut c) } else { interactive(&mut c) };
            match r {
                Ok(()) => {
                    out(b"winsta_test: ok\r\n");
                    0
                }
                Err(id) => id,
            }
        }
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
