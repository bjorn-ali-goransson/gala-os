//! Sessions and per-session named objects (ROADMAP RS step B1,
//! ARCHITECTURE "Plan: sessions and remote shasha"), with the results
//! Windows documents: "Kernel object namespaces" (`Global\`, `Local\`,
//! session 0 for services), ProcessIdToSessionId,
//! WTSGetActiveConsoleSessionId, GetTokenInformation(TokenSessionId),
//! SetTokenInformation(TokenSessionId) (SeTcbPrivilege) and
//! CreateProcessAsUserW (the new process runs in its token's session),
//! and Windows Internals' object directory layout (`\Sessions\<n>\
//! BaseNamedObjects` per session, session 0's named objects in
//! `\BaseNamedObjects`, `\Sessions\BNOLINKS\<n>` links).
//!
//! gala starts it as SYSTEM (boot option `as=system`); on Windows run it
//! as SYSTEM in session 0 (`psexec -s`, not `-i`) with a console session
//! 1 logged on. Needs the second local account `galatest` (see
//! access_test).
//!
//! - The SYSTEM process is in session 0: PEB.SessionId,
//!   NtQueryInformationProcess(ProcessSessionInformation),
//!   ProcessIdToSessionId and TokenSessionId agree; the console session
//!   (WTSGetActiveConsoleSessionId) is 1.
//! - The namespace: `\Sessions\1\BaseNamedObjects` is a directory;
//!   `\Sessions\0` exists but has no BaseNamedObjects (session 0's are
//!   `\BaseNamedObjects`); `\Sessions\BNOLINKS\0` and `\1` are links to
//!   them.
//! - In session 0, `Local\` and `Global\` are both `\BaseNamedObjects`.
//! - A logon token (LogonUserW of `galatest`, made in session 0) moved to
//!   session 1 (SetTokenInformation) starts the child (this program,
//!   `child`) in session 1, where the four answers agree on 1; its
//!   `Local\` object is in `\Sessions\1\BaseNamedObjects` and not found
//!   from session 0 by that name; the parent's `Global\` object is found
//!   from session 1 only with the prefix.
//! - ProcessIdToSessionId of another process: the child's (1) from the
//!   parent; an id that is no process: ERROR_INVALID_PARAMETER; a handle
//!   without PROCESS_QUERY_LIMITED_INFORMATION: STATUS_ACCESS_DENIED.
//!
//! Prints `session_test: ok` or exits with the number of the failing check
//! (the child's failures are 0x100 + its check, seen as its exit code).

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: Bool,
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *const u16,
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: Handle,
    name: *const UnicodeString,
    attributes: u32,
    descriptor: *const c_void,
    qos: *const c_void,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetLastError() -> u32;
    fn SetLastError(e: u32);
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetCurrentProcess() -> Handle;
    fn GetCurrentProcessId() -> u32;
    fn GetCommandLineW() -> *const u16;
    fn GetModuleFileNameW(module: Handle, name: *mut u16, size: u32) -> u32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn WaitForMultipleObjects(n: u32, handles: *const Handle, all: Bool, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn CreateEventW(
        sa: *const SecurityAttributes,
        manual: Bool,
        initial: Bool,
        name: *const u16,
    ) -> Handle;
    fn OpenEventW(access: u32, inherit: Bool, name: *const u16) -> Handle;
    fn SetEvent(h: Handle) -> Bool;
    fn OpenProcess(access: u32, inherit: Bool, id: u32) -> Handle;
    fn ProcessIdToSessionId(pid: u32, session: *mut u32) -> Bool;
    fn WTSGetActiveConsoleSessionId() -> u32;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LogonUserW(
        user: *const u16,
        domain: *const u16,
        password: *const u16,
        kind: u32,
        provider: u32,
        token: *mut Handle,
    ) -> Bool;
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> Bool;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        out: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> Bool;
    fn SetTokenInformation(token: Handle, class: u32, info: *const u8, len: u32) -> Bool;
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        text: *const u16,
        revision: u32,
        sd: *mut *mut c_void,
        size: *mut u32,
    ) -> Bool;
    #[allow(clippy::too_many_arguments)]
    fn CreateProcessAsUserW(
        token: Handle,
        application: *const u16,
        command_line: *mut u16,
        process_attributes: *mut c_void,
        thread_attributes: *mut c_void,
        inherit: Bool,
        flags: u32,
        environment: *mut c_void,
        directory: *const u16,
        startup: *const u8,
        info: *mut [usize; 3],
    ) -> Bool;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        process: Handle,
        class: u32,
        info: *mut u8,
        length: u32,
        ret: *mut u32,
    ) -> u32;
    fn NtOpenDirectoryObject(h: *mut Handle, access: u32, oa: *const ObjectAttributes) -> u32;
    fn NtOpenSymbolicLinkObject(h: *mut Handle, access: u32, oa: *const ObjectAttributes) -> u32;
    fn NtQuerySymbolicLinkObject(h: Handle, target: *mut UnicodeString, ret: *mut u32) -> u32;
    fn NtOpenEvent(h: *mut Handle, access: u32, oa: *const ObjectAttributes) -> u32;
    fn NtClose(h: Handle) -> u32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn __chkstk();
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const STATUS_SUCCESS: u32 = 0;
const STATUS_ACCESS_DENIED: u32 = 0xc000_0022;
const STATUS_OBJECT_NAME_NOT_FOUND: u32 = 0xc000_0034;
const PROCESS_SESSION_INFORMATION: u32 = 24;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const TOKEN_SESSION_ID: u32 = 12;
const TOKEN_QUERY: u32 = 0x8;
const DIRECTORY_QUERY: u32 = 0x1;
const SYMBOLIC_LINK_QUERY: u32 = 0x1;
const SYNCHRONIZE: u32 = 0x10_0000;
const OBJ_CASE_INSENSITIVE: u32 = 0x40;
const LOGON32_LOGON_INTERACTIVE: u32 = 2;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_INVALID_PARAMETER: u32 = 87;
/// PEB.SessionId (x64).
const PEB_SESSION_ID: usize = 0x2c0;

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

/// Numbered checks; the child's numbers start at 0x100 and it does not
/// print (its exit code says which failed).
struct Checks {
    n: core::cell::Cell<u32>,
    quiet: bool,
}

impl Checks {
    fn new(first: u32, quiet: bool) -> Self {
        Checks { n: core::cell::Cell::new(first), quiet }
    }

    fn ok(&self, cond: bool, got: u64) -> Result<(), u32> {
        self.n.set(self.n.get() + 1);
        if cond {
            return Ok(());
        }
        if !self.quiet {
            // One line, one WriteFile (WORKFLOW).
            let mut line = *b"session_test: check 00000000 got 00000000\r\n";
            line[20..28].copy_from_slice(&hex(self.n.get()));
            line[33..41].copy_from_slice(&hex(got as u32));
            out(&line);
        }
        Err(self.n.get())
    }

    fn eq(&self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

fn last_error() -> u64 {
    // SAFETY: a plain call.
    u64::from(unsafe { GetLastError() })
}

fn w<const N: usize>(s: &str) -> [u16; N] {
    let mut b = [0u16; N];
    for (slot, c) in b.iter_mut().zip(s.encode_utf16()) {
        *slot = c;
    }
    b
}

/// The length of a NUL-terminated wide string.
fn wlen(s: &[u16]) -> usize {
    s.iter().position(|&c| c == 0).unwrap_or(s.len())
}

fn peb_session() -> u32 {
    let peb: usize;
    // SAFETY: gs:[0x60] is the PEB pointer in the TEB.
    unsafe {
        core::arch::asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly));
        *((peb + PEB_SESSION_ID) as *const u32)
    }
}

/// NtQueryInformationProcess(ProcessSessionInformation): (status, id,
/// returned length).
fn process_session(process: Handle) -> (u32, u32, u32) {
    let (mut id, mut ret) = (0xdead_u32, 0u32);
    // SAFETY: a 4-byte buffer.
    let status = unsafe {
        NtQueryInformationProcess(
            process,
            PROCESS_SESSION_INFORMATION,
            (&raw mut id).cast(),
            4,
            &mut ret,
        )
    };
    (status, id, ret)
}

/// GetTokenInformation(TokenSessionId): (id, returned length), or None.
fn token_session(token: Handle) -> Option<(u32, u32)> {
    let (mut id, mut ret) = (0xdead_u32, 0u32);
    // SAFETY: a 4-byte buffer.
    let ok = unsafe {
        GetTokenInformation(token, TOKEN_SESSION_ID, (&raw mut id).cast(), 4, &mut ret) != 0
    };
    ok.then_some((id, ret))
}

fn own_token_session() -> Option<(u32, u32)> {
    let mut token = core::ptr::null_mut();
    // SAFETY: plain calls.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let r = token_session(token);
        CloseHandle(token);
        r
    }
}

/// The four views of the caller's session agree on `want`.
fn check_own_session(c: &Checks, want: u32) -> Result<(), u32> {
    c.eq(peb_session().into(), want.into())?;
    // SAFETY: the current-process pseudo handle.
    let (status, id, ret) = process_session(unsafe { GetCurrentProcess() });
    c.eq(status.into(), STATUS_SUCCESS.into())?;
    c.eq(id.into(), want.into())?;
    c.eq(ret.into(), 4)?;
    let mut id = 0xdead;
    // SAFETY: plain calls; our output.
    c.ok(unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut id) } != 0, last_error())?;
    c.eq(id.into(), want.into())?;
    let token = own_token_session();
    c.eq(token.map_or(0xdead, |(id, _)| id).into(), want.into())?;
    c.eq(token.map_or(0, |(_, n)| n).into(), 4)?;
    // SAFETY: a plain call.
    c.eq(unsafe { WTSGetActiveConsoleSessionId() }.into(), 1)
}

fn us(s: &[u16]) -> UnicodeString {
    let n = (wlen(s) * 2) as u16;
    UnicodeString { length: n, maximum: n, buffer: s.as_ptr() }
}

fn attributes(name: &UnicodeString) -> ObjectAttributes {
    ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root: core::ptr::null_mut(),
        name,
        attributes: OBJ_CASE_INSENSITIVE,
        descriptor: core::ptr::null(),
        qos: core::ptr::null(),
    }
}

type Open = unsafe extern "system" fn(*mut Handle, u32, *const ObjectAttributes) -> u32;

/// Opens the NT path `path` with `open`; the status (the handle closed).
fn nt_open(open: Open, access: u32, path: &str) -> u32 {
    let name = w::<96>(path);
    let name = us(&name);
    let mut h = core::ptr::null_mut();
    // SAFETY: valid attributes and output.
    let status = unsafe { open(&mut h, access, &attributes(&name)) };
    if status == STATUS_SUCCESS {
        // SAFETY: our handle.
        unsafe { NtClose(h) };
    }
    status
}

/// Whether the symbolic link `path`'s target is `want`.
fn link_target_is(path: &str, want: &str) -> Result<bool, u32> {
    let name = w::<96>(path);
    let name = us(&name);
    let mut h = core::ptr::null_mut();
    // SAFETY: valid attributes and output.
    let status =
        unsafe { NtOpenSymbolicLinkObject(&mut h, SYMBOLIC_LINK_QUERY, &attributes(&name)) };
    if status != STATUS_SUCCESS {
        return Err(status);
    }
    let mut buf = [0u16; 96];
    let mut target = UnicodeString { length: 0, maximum: 192, buffer: buf.as_mut_ptr() };
    let mut ret = 0;
    // SAFETY: our handle and buffer.
    let status = unsafe { NtQuerySymbolicLinkObject(h, &mut target, &mut ret) };
    // SAFETY: our handle.
    unsafe { NtClose(h) };
    if status != STATUS_SUCCESS {
        return Err(status);
    }
    let got = &buf[..usize::from(target.length / 2)];
    Ok(got.iter().copied().eq(want.encode_utf16()))
}

/// OpenEventW(SYNCHRONIZE, name): Ok(()) if it opened, else the last
/// error.
fn open_event(name: &str) -> Result<(), u32> {
    let name = w::<64>(name);
    // SAFETY: a NUL-terminated name.
    let h = unsafe { OpenEventW(SYNCHRONIZE, 0, name.as_ptr()) };
    if h.is_null() {
        return Err(last_error() as u32);
    }
    // SAFETY: our handle.
    unsafe { CloseHandle(h) };
    Ok(())
}

const LOCAL_NAME: &str = "Local\\gala_session_test_local";
const S0_NAME: &str = "Local\\gala_session_test_s0";
const GLOBAL_NAME: &str = "Global\\gala_session_test_global";

/// The child, in session 1: (ready, done, parent pid) from its command
/// line. Exits with 0 or 0x100 + the failing check.
fn child(ready: Handle, done: Handle) -> u32 {
    let c = Checks::new(0x100, true);
    let run = || -> Result<(), u32> {
        check_own_session(&c, 1)?;
        // Its Local\ object, also found without the prefix (the session's
        // own namespace).
        let local = w::<64>(LOCAL_NAME);
        // SAFETY: plain calls; NUL-terminated names.
        unsafe {
            SetLastError(0xdead);
            let h = CreateEventW(core::ptr::null(), 1, 0, local.as_ptr());
            c.ok(!h.is_null(), last_error())?;
            c.eq(last_error(), 0)?;
        }
        c.ok(open_event("gala_session_test_local").is_ok(), 0)?;
        // The parent's Global\ object: found with the prefix only.
        let r = open_event(GLOBAL_NAME);
        c.ok(r.is_ok(), r.err().unwrap_or(0).into())?;
        c.eq(
            open_event("gala_session_test_global").err().unwrap_or(0).into(),
            ERROR_FILE_NOT_FOUND.into(),
        )?;
        // The parent's Local\ object is session 0's: not here.
        c.eq(open_event(S0_NAME).err().unwrap_or(0).into(), ERROR_FILE_NOT_FOUND.into())?;
        // SAFETY: inherited handles.
        unsafe {
            c.ok(SetEvent(ready) != 0, last_error())?;
            c.eq(WaitForSingleObject(done, 30_000).into(), 0)?;
        }
        Ok(())
    };
    // The event handle stays open until the process ends.
    run().err().unwrap_or(0)
}

/// The command line's words after the program, if the first is `child`:
/// (ready, done) handle values.
fn child_args() -> Option<(Handle, Handle)> {
    // SAFETY: the process's command line, NUL-terminated.
    let line = unsafe {
        let p = GetCommandLineW();
        core::slice::from_raw_parts(p, (0..).take_while(|&i| *p.add(i) != 0).count())
    };
    // After the quoted program name.
    let rest = line.iter().skip(1).position(|&c| c == u16::from(b'"')).map(|i| &line[i + 2..])?;
    let mut words = rest.split(|&c| c == u16::from(b' ')).filter(|w| !w.is_empty());
    if !words.next()?.iter().copied().eq("child".encode_utf16()) {
        return None;
    }
    let mut number = || -> Option<usize> {
        words.next()?.iter().try_fold(0usize, |v, &d| {
            let d = d.checked_sub(u16::from(b'0')).filter(|&d| d < 10)?;
            Some(v * 10 + usize::from(d))
        })
    };
    Some((number()? as Handle, number()? as Handle))
}

/// Appends `v` in decimal to `line` at `*n`.
fn put_number(line: &mut [u16], n: &mut usize, v: usize) {
    let mut digits = [0u16; 20];
    let (mut k, mut v) = (0, v);
    loop {
        digits[k] = u16::from(b'0') + (v % 10) as u16;
        k += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    for d in digits[..k].iter().rev() {
        line[*n] = *d;
        *n += 1;
    }
}

fn run() -> Result<(), u32> {
    let c = Checks::new(0, false);
    let null = core::ptr::null_mut();
    // SYSTEM: session 0.
    check_own_session(&c, 0)?;

    // The namespace.
    c.eq(
        nt_open(NtOpenDirectoryObject, DIRECTORY_QUERY, "\\Sessions\\1\\BaseNamedObjects").into(),
        0,
    )?;
    c.eq(nt_open(NtOpenDirectoryObject, DIRECTORY_QUERY, "\\Sessions\\0").into(), 0)?;
    c.eq(
        nt_open(NtOpenDirectoryObject, DIRECTORY_QUERY, "\\Sessions\\0\\BaseNamedObjects").into(),
        STATUS_OBJECT_NAME_NOT_FOUND.into(),
    )?;
    let r = link_target_is("\\Sessions\\BNOLINKS\\0", "\\BaseNamedObjects");
    c.ok(r == Ok(true), r.err().unwrap_or(0).into())?;
    let r = link_target_is("\\Sessions\\BNOLINKS\\1", "\\Sessions\\1\\BaseNamedObjects");
    c.ok(r == Ok(true), r.err().unwrap_or(0).into())?;
    c.eq(nt_open(NtOpenDirectoryObject, DIRECTORY_QUERY, "\\Sessions\\BNOLINKS\\1").into(), 0)?;

    // SAFETY: plain calls with our buffers and handles throughout.
    unsafe {
        // Session 0: Local\ and Global\ are both \BaseNamedObjects.
        let s0 = w::<64>(S0_NAME);
        let s0_event = CreateEventW(core::ptr::null(), 1, 0, s0.as_ptr());
        c.ok(!s0_event.is_null(), last_error())?;
        c.eq(
            nt_open(NtOpenEvent, SYNCHRONIZE, "\\BaseNamedObjects\\gala_session_test_s0").into(),
            0,
        )?;
        c.ok(open_event("Global\\gala_session_test_s0").is_ok(), 0)?;
        c.ok(open_event("gala_session_test_s0").is_ok(), 0)?;

        // A Global\ object everyone may open (for the child).
        let mut sd = null;
        let text = w::<32>("D:(A;;GA;;;WD)");
        c.ok(
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                1,
                &mut sd,
                null.cast(),
            ) != 0,
            last_error(),
        )?;
        let open_sa = SecurityAttributes { length: 24, descriptor: sd, inherit: 0 };
        let global = w::<64>(GLOBAL_NAME);
        let global_event = CreateEventW(&open_sa, 1, 0, global.as_ptr());
        c.ok(!global_event.is_null(), last_error())?;
        c.eq(
            nt_open(NtOpenEvent, SYNCHRONIZE, "\\BaseNamedObjects\\gala_session_test_global")
                .into(),
            0,
        )?;

        // ProcessIdToSessionId of an id that is no process.
        let mut id = 0xdead;
        SetLastError(0);
        c.ok(ProcessIdToSessionId(0x7fff_fff0, &mut id) == 0, 0)?;
        c.eq(last_error(), ERROR_INVALID_PARAMETER.into())?;

        // galatest's logon token: made in session 0, moved to session 1
        // (SeTcbPrivilege, which SYSTEM has enabled).
        let mut test = null;
        let (user, dot, password) = (w::<16>("galatest"), w::<4>("."), w::<16>("Gala-Test-1"));
        c.ok(
            LogonUserW(
                user.as_ptr(),
                dot.as_ptr(),
                password.as_ptr(),
                LOGON32_LOGON_INTERACTIVE,
                0,
                &mut test,
            ) != 0,
            last_error(),
        )?;
        c.eq(token_session(test).map_or(0xdead, |(id, _)| id).into(), 0)?;
        let one = 1u32;
        c.ok(
            SetTokenInformation(test, TOKEN_SESSION_ID, (&raw const one).cast(), 4) != 0,
            last_error(),
        )?;
        c.eq(token_session(test).map_or(0xdead, |(id, _)| id).into(), 1)?;

        // The child, with two inherited events.
        let inherit_sa =
            SecurityAttributes { length: 24, descriptor: core::ptr::null_mut(), inherit: 1 };
        let ready = CreateEventW(&inherit_sa, 1, 0, core::ptr::null());
        let done = CreateEventW(&inherit_sa, 1, 0, core::ptr::null());
        c.ok(!ready.is_null() && !done.is_null(), last_error())?;
        let mut exe = [0u16; 260];
        let n = GetModuleFileNameW(null, exe.as_mut_ptr(), 260) as usize;
        c.ok(n > 0 && n < 200, 0)?;
        let mut line = [0u16; 300];
        line[0] = u16::from(b'"');
        line[1..=n].copy_from_slice(&exe[..n]);
        let mut at = n + 1;
        for ch in "\" child ".encode_utf16() {
            line[at] = ch;
            at += 1;
        }
        put_number(&mut line, &mut at, ready as usize);
        line[at] = u16::from(b' ');
        at += 1;
        put_number(&mut line, &mut at, done as usize);
        let mut startup = [0u8; 104];
        startup[0] = 104;
        let mut info = [0usize; 3];
        let r = CreateProcessAsUserW(
            test,
            exe.as_ptr(),
            line.as_mut_ptr(),
            null,
            null,
            1,
            CREATE_NO_WINDOW,
            null,
            core::ptr::null(),
            startup.as_ptr(),
            &mut info,
        );
        c.ok(r != 0, last_error())?;
        let (process, pid) = (info[0] as Handle, info[2] as u32);
        let either = [ready, process];
        let woke = WaitForMultipleObjects(2, either.as_ptr(), 0, 30_000);
        let mut code = 0;
        GetExitCodeProcess(process, &mut code);
        // 1: the child ended first (its exit code says why).
        c.ok(woke == 0, if woke == 1 { code.into() } else { woke.into() })?;

        // The child's Local\ object: not found by that name in session 0,
        // found by its session's path and through BNOLINKS.
        c.eq(open_event(LOCAL_NAME).err().unwrap_or(0).into(), ERROR_FILE_NOT_FOUND.into())?;
        c.eq(
            open_event("Global\\gala_session_test_local").err().unwrap_or(0).into(),
            ERROR_FILE_NOT_FOUND.into(),
        )?;
        c.eq(
            nt_open(
                NtOpenEvent,
                SYNCHRONIZE,
                "\\Sessions\\1\\BaseNamedObjects\\gala_session_test_local",
            )
            .into(),
            0,
        )?;
        c.eq(
            nt_open(NtOpenEvent, SYNCHRONIZE, "\\Sessions\\BNOLINKS\\1\\gala_session_test_local")
                .into(),
            0,
        )?;
        c.eq(
            nt_open(NtOpenEvent, SYNCHRONIZE, "\\BaseNamedObjects\\gala_session_test_local").into(),
            STATUS_OBJECT_NAME_NOT_FOUND.into(),
        )?;

        // The child's session, asked from here.
        let mut id = 0xdead;
        c.ok(ProcessIdToSessionId(pid, &mut id) != 0, last_error())?;
        c.eq(id.into(), 1)?;
        let (status, id, _) = process_session(process);
        c.eq(status.into(), 0)?;
        c.eq(id.into(), 1)?;
        let limited = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        c.ok(!limited.is_null(), last_error())?;
        c.eq(process_session(limited).1.into(), 1)?;
        CloseHandle(limited);
        let sync_only = OpenProcess(SYNCHRONIZE, 0, pid);
        c.ok(!sync_only.is_null(), last_error())?;
        c.eq(process_session(sync_only).0.into(), STATUS_ACCESS_DENIED.into())?;
        CloseHandle(sync_only);

        SetEvent(done);
        c.eq(WaitForSingleObject(process, 30_000).into(), 0)?;
        c.ok(GetExitCodeProcess(process, &mut code) != 0, last_error())?;
        c.eq(code.into(), 0)?;
        CloseHandle(info[0] as Handle);
        CloseHandle(info[1] as Handle);
        CloseHandle(ready);
        CloseHandle(done);
        CloseHandle(test);
        CloseHandle(global_event);
        CloseHandle(s0_event);
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    if let Some((ready, done)) = child_args() {
        // SAFETY: ends the process.
        unsafe { ExitProcess(child(ready, done)) }
    }
    let code = match run() {
        Ok(()) => {
            out(b"session_test: ok\r\n");
            0
        }
        Err(id) => id,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
