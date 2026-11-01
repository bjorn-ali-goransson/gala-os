//! Per-user processes and impersonation across processes (docs/SECURITY.md,
//! phase S4), with the results real Windows gives a standard user, from
//! the documented contracts (CreateProcessAsUserW, ImpersonateNamedPipeClient,
//! GetUserProfileDirectoryW). Needs the second local account `galatest`
//! (password `Gala-Test-1`; see access_test).
//!
//! - CreateProcessAsUserW of another user's token (from LogonUserW) needs
//!   SeAssignPrimaryTokenPrivilege, which a standard user does not hold:
//!   ERROR_PRIVILEGE_NOT_HELD. Of a restricted version of the caller's own
//!   token it works: the child (this program, `child`) runs with that
//!   token (TokenUser the caller's, IsTokenRestricted) and reports it in
//!   its exit code.
//! - An impersonation token is not a primary one: ERROR_BAD_TOKEN_TYPE.
//! - A named pipe's server impersonating its client
//!   (ImpersonateNamedPipeClient, after reading from it) gets the
//!   client's token: a client connecting while it impersonates `galatest`
//!   is seen as `galatest`, at impersonation level.
//! - GetUserProfileDirectoryW of the caller's token is %USERPROFILE%;
//!   size queries (ERROR_INSUFFICIENT_BUFFER and the size with the NUL).
//!
//! Prints `logon_test: ok` or exits with the number of the failing check.

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

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetLastError() -> u32;
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetCurrentProcess() -> Handle;
    fn GetCommandLineW() -> *const u16;
    fn GetModuleFileNameW(module: Handle, name: *mut u16, size: u32) -> u32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn CreateNamedPipeW(
        name: *const u16,
        mode: u32,
        pipe_mode: u32,
        instances: u32,
        out_size: u32,
        in_size: u32,
        timeout: u32,
        sa: *const SecurityAttributes,
    ) -> Handle;
    fn ConnectNamedPipe(h: Handle, ov: *mut c_void) -> Bool;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *const SecurityAttributes,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CreateThread(
        attrs: *mut c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
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
    fn ImpersonateLoggedOnUser(token: Handle) -> Bool;
    fn ImpersonateNamedPipeClient(pipe: Handle) -> Bool;
    fn RevertToSelf() -> Bool;
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> Bool;
    fn OpenThreadToken(thread: Handle, access: u32, as_self: Bool, token: *mut Handle) -> Bool;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        out: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> Bool;
    fn DuplicateTokenEx(
        token: Handle,
        access: u32,
        attributes: *mut c_void,
        level: u32,
        kind: u32,
        new: *mut Handle,
    ) -> Bool;
    #[allow(clippy::too_many_arguments)]
    fn CreateRestrictedToken(
        token: Handle,
        flags: u32,
        disable_count: u32,
        disable: *const [usize; 2],
        delete_count: u32,
        delete: *const [u32; 3],
        restrict_count: u32,
        restrict: *const [usize; 2],
        new: *mut Handle,
    ) -> Bool;
    fn IsTokenRestricted(token: Handle) -> Bool;
    fn EqualSid(a: *const u8, b: *const u8) -> Bool;
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
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        text: *const u16,
        revision: u32,
        sd: *mut *mut c_void,
        size: *mut u32,
    ) -> Bool;
}

#[link(name = "userenv", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetUserProfileDirectoryW(token: Handle, dir: *mut u16, size: *mut u32) -> Bool;
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

const TOKEN_ALL_ACCESS: u32 = 0x000f_01ff;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_PRIVILEGE_NOT_HELD: u32 = 1314;
const ERROR_BAD_TOKEN_TYPE: u32 = 1349;
/// The child's exit code when it runs restricted as the caller's user.
const CHILD_RESTRICTED: u32 = 0x50;

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

struct Checks(core::cell::Cell<u32>);

impl Checks {
    fn ok(&self, cond: bool, got: u64) -> Result<(), u32> {
        self.0.set(self.0.get() + 1);
        if cond {
            return Ok(());
        }
        out(b"logon_test: check ");
        out(&hex(self.0.get()));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0.get())
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

#[repr(C, align(8))]
struct Buf([u8; 512]);

/// TokenUser's SID of `token` (in `b`).
fn user_of(token: Handle, b: &mut Buf) -> *const u8 {
    let mut n = 0;
    // SAFETY: our buffer; TOKEN_USER starts with the SID pointer.
    unsafe {
        if GetTokenInformation(token, 1, b.0.as_mut_ptr(), 512, &mut n) == 0 {
            return core::ptr::null();
        }
        b.0.as_ptr().cast::<*const u8>().read()
    }
}

/// The child: exits with CHILD_RESTRICTED if its process token is
/// restricted (it was started with a restricted token), else 1.
fn child() -> u32 {
    let mut token = core::ptr::null_mut();
    // SAFETY: plain calls.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), 8, &mut token) == 0 {
            return 2;
        }
        if IsTokenRestricted(token) != 0 { CHILD_RESTRICTED } else { 1 }
    }
}

fn is_child() -> bool {
    // SAFETY: the process's command line.
    let line = unsafe { GetCommandLineW() };
    let mut n = 0;
    // SAFETY: a NUL-terminated string.
    while unsafe { *line.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: `n` units.
    let s = unsafe { core::slice::from_raw_parts(line, n) };
    s.ends_with(&w::<5>("child"))
}

static mut CLIENT_TOKEN: usize = 0;

/// The pipe client: impersonates `galatest`, connects and writes a byte.
extern "system" fn client(_: *mut c_void) -> u32 {
    // SAFETY: our pipe name; the token handle set before the thread starts.
    unsafe {
        if ImpersonateLoggedOnUser(CLIENT_TOKEN as Handle) == 0 {
            return 1;
        }
        let name = w::<64>("\\\\.\\pipe\\gala-logon-test");
        let h = CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            core::ptr::null(),
            3,
            0,
            core::ptr::null_mut(),
        );
        if h as isize == -1 {
            return 2;
        }
        let mut n = 0;
        WriteFile(h, b"x".as_ptr(), 1, &mut n, core::ptr::null_mut());
        let mut b = [0u8; 1];
        ReadFile(h, b.as_mut_ptr(), 1, &mut n, core::ptr::null_mut());
        CloseHandle(h);
        RevertToSelf();
    }
    0
}

fn run() -> Result<(), u32> {
    let c = Checks(core::cell::Cell::new(0));
    let null = core::ptr::null_mut();
    // SAFETY: plain calls with our buffers and handles throughout.
    unsafe {
        let mut test = null;
        let (user, dot, password) = (w::<16>("galatest"), w::<4>("."), w::<16>("Gala-Test-1"));
        c.ok(
            LogonUserW(user.as_ptr(), dot.as_ptr(), password.as_ptr(), 2, 0, &mut test) != 0,
            last_error(),
        )?;
        let mut mine = null;
        c.ok(OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut mine) != 0, 0)?;
        let (mut b1, mut b2) = (Buf([0; 512]), Buf([0; 512]));
        let (me_sid, test_sid) = (user_of(mine, &mut b1), user_of(test, &mut b2));
        c.ok(!me_sid.is_null() && !test_sid.is_null(), 0)?;

        // This program, to start as a child.
        let mut exe = [0u16; 260];
        let n = GetModuleFileNameW(null, exe.as_mut_ptr(), 260) as usize;
        c.ok(n > 0, 0)?;
        let mut line = [0u16; 300];
        line[0] = u16::from(b'"');
        line[1..=n].copy_from_slice(&exe[..n]);
        for (i, ch) in "\" child".encode_utf16().enumerate() {
            line[n + 1 + i] = ch;
        }
        let mut startup = [0u8; 104];
        startup[0] = 104;

        // Another user's token: SeAssignPrimaryTokenPrivilege is needed.
        let mut info = [0usize; 3];
        let r = CreateProcessAsUserW(
            test,
            exe.as_ptr(),
            line.as_mut_ptr(),
            null,
            null,
            0,
            0,
            null,
            core::ptr::null(),
            startup.as_ptr(),
            &mut info,
        );
        c.ok(r == 0, 0)?;
        c.eq(last_error(), ERROR_PRIVILEGE_NOT_HELD.into())?;
        // An impersonation token is not a primary token.
        let mut imp = null;
        c.ok(DuplicateTokenEx(mine, TOKEN_ALL_ACCESS, null, 2, 2, &mut imp) != 0, 0)?;
        let r = CreateProcessAsUserW(
            imp,
            exe.as_ptr(),
            line.as_mut_ptr(),
            null,
            null,
            0,
            0,
            null,
            core::ptr::null(),
            startup.as_ptr(),
            &mut info,
        );
        c.ok(r == 0, 0)?;
        c.eq(last_error(), ERROR_BAD_TOKEN_TYPE.into())?;
        // A restricted version of the caller's own token.
        // Restricting SIDs a program can still run with: Everyone,
        // BUILTIN\\Users (system files grant it reading) and the user.
        let everyone: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];
        let users: [u8; 16] = [1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 0x21, 2, 0, 0];
        let restrict =
            [[everyone.as_ptr() as usize, 0], [users.as_ptr() as usize, 0], [me_sid as usize, 0]];
        let mut restricted = null;
        let r = CreateRestrictedToken(
            mine,
            0,
            0,
            core::ptr::null(),
            0,
            core::ptr::null(),
            3,
            restrict.as_ptr(),
            &mut restricted,
        );
        c.ok(r != 0, last_error())?;
        let r = CreateProcessAsUserW(
            restricted,
            exe.as_ptr(),
            line.as_mut_ptr(),
            null,
            null,
            0,
            0,
            null,
            core::ptr::null(),
            startup.as_ptr(),
            &mut info,
        );
        c.ok(r != 0, last_error())?;
        c.eq(u64::from(WaitForSingleObject(info[0] as Handle, 30_000)), 0)?;
        let mut code = 0;
        c.ok(GetExitCodeProcess(info[0] as Handle, &mut code) != 0, 0)?;
        c.eq(code.into(), CHILD_RESTRICTED.into())?;
        CloseHandle(info[0] as Handle);
        CloseHandle(info[1] as Handle);

        // A pipe's server impersonates its client: the client's token.
        let mut sd = null;
        let text = w::<32>("D:(A;;FA;;;WD)");
        c.ok(
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                1,
                &mut sd,
                null.cast(),
            ) != 0,
            0,
        )?;
        let sa = SecurityAttributes { length: 24, descriptor: sd, inherit: 0 };
        let name = w::<64>("\\\\.\\pipe\\gala-logon-test");
        let pipe = CreateNamedPipeW(name.as_ptr(), 3, 0, 1, 512, 512, 0, &sa); // PIPE_ACCESS_DUPLEX
        c.ok(pipe as isize != -1, last_error())?;
        CLIENT_TOKEN = test as usize;
        let thread = CreateThread(null, 0, client, null, 0, null.cast());
        c.ok(!thread.is_null(), 0)?;
        let connected = ConnectNamedPipe(pipe, null);
        c.ok(connected != 0 || GetLastError() == 535, last_error())?; // ERROR_PIPE_CONNECTED
        let mut byte = [0u8; 1];
        let mut got = 0;
        c.ok(ReadFile(pipe, byte.as_mut_ptr(), 1, &mut got, null) != 0 && got == 1, last_error())?;
        c.ok(ImpersonateNamedPipeClient(pipe) != 0, last_error())?;
        let mut client_token = null;
        c.ok(OpenThreadToken(-2isize as Handle, 8, 1, &mut client_token) != 0, last_error())?;
        let mut b3 = Buf([0; 512]);
        let client_sid = user_of(client_token, &mut b3);
        c.ok(!client_sid.is_null() && EqualSid(client_sid, test_sid) != 0, 0)?;
        let mut level = 0u32;
        let mut n = 0;
        c.ok(GetTokenInformation(client_token, 9, (&raw mut level).cast(), 4, &mut n) != 0, 0)?;
        c.eq(level.into(), 2)?; // SecurityImpersonation
        CloseHandle(client_token);
        c.ok(RevertToSelf() != 0, 0)?;
        c.ok(WriteFile(pipe, b"y".as_ptr(), 1, &mut got, null) != 0, last_error())?;
        c.eq(u64::from(WaitForSingleObject(thread, 30_000)), 0)?;
        CloseHandle(thread);
        CloseHandle(pipe);

        // The profile directory: %USERPROFILE%.
        let mut size = 0u32;
        c.ok(GetUserProfileDirectoryW(mine, null.cast(), &mut size) == 0, 0)?;
        c.eq(last_error(), ERROR_INSUFFICIENT_BUFFER.into())?;
        let mut dir = [0u16; 260];
        let mut env = [0u16; 260];
        let var = w::<16>("USERPROFILE");
        let len = GetEnvironmentVariableW(var.as_ptr(), env.as_mut_ptr(), 260);
        c.ok(len > 0, 0)?;
        c.eq(size.into(), u64::from(len) + 1)?;
        c.ok(GetUserProfileDirectoryW(mine, dir.as_mut_ptr(), &mut size) != 0, last_error())?;
        c.ok(dir[..len as usize] == env[..len as usize] && dir[len as usize] == 0, 0)?;

        for h in [test, mine, imp, restricted] {
            CloseHandle(h);
        }
        let _ = GENERIC_WRITE;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    if is_child() {
        // SAFETY: ends the process.
        unsafe { ExitProcess(child()) }
    }
    let code = match run() {
        Ok(()) => {
            out(b"logon_test: ok\r\n");
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
