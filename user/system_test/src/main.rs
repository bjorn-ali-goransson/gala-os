//! A program running as SYSTEM, the account services run as
//! (docs/SECURITY.md, phase S4), with the results Windows gives (the
//! documented LocalSystem account: "LocalSystem Account", "Well-known
//! SIDs", "Privilege Constants"). gala starts it as SYSTEM with the boot
//! option `as=system`; on Windows run it as SYSTEM (`psexec -s`). Needs
//! the second local account `galatest` (see access_test).
//!
//! - The token: user S-1-5-18, owner BUILTIN\Administrators (a group it
//!   holds with SE_GROUP_OWNER), the System integrity level, the
//!   SeTcbPrivilege enabled, SeAssignPrimaryTokenPrivilege and
//!   SeBackupPrivilege held but disabled.
//! - A file it creates is owned by Administrators; a DACL that gives
//!   only BUILTIN\Users access denies it reading, with or without backup
//!   semantics while SeBackupPrivilege is disabled; enabled
//!   (AdjustTokenPrivileges, last error 0), FILE_FLAG_BACKUP_SEMANTICS
//!   opens it for reading.
//! - CreateProcessAsUserW of `galatest`'s token (LogonUserW) works: the
//!   child (this program, `child`) runs as `galatest` (it exits with its
//!   user's RID).
//!
//! Prints `system_test: ok` or exits with the number of the failing check.

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
    fn SetLastError(e: u32);
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetCurrentProcess() -> Handle;
    fn GetCommandLineW() -> *const u16;
    fn GetModuleFileNameW(module: Handle, name: *mut u16, size: u32) -> u32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn DeleteFileW(path: *const u16) -> Bool;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *const SecurityAttributes,
        disposition: u32,
        flags: u32,
        template: Handle,
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
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> Bool;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        out: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> Bool;
    fn AdjustTokenPrivileges(
        token: Handle,
        disable_all: Bool,
        new: *const u32,
        length: u32,
        previous: *mut u8,
        ret: *mut u32,
    ) -> Bool;
    fn EqualSid(a: *const u8, b: *const u8) -> Bool;
    fn GetFileSecurityW(
        path: *const u16,
        info: u32,
        sd: *mut u8,
        length: u32,
        needed: *mut u32,
    ) -> Bool;
    fn GetSecurityDescriptorOwner(
        sd: *const u8,
        owner: *mut *const u8,
        defaulted: *mut Bool,
    ) -> Bool;
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

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const ERROR_ACCESS_DENIED: u32 = 5;
const SE_PRIVILEGE_ENABLED: u32 = 2;
const SE_GROUP_OWNER: u32 = 8;
const SE_ASSIGN_PRIMARY_TOKEN: u32 = 3;
const SE_TCB: u32 = 7;
const SE_BACKUP: u32 = 17;

const SYSTEM: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0];
const ADMINISTRATORS: [u8; 16] = [1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 0x20, 2, 0, 0];
const SYSTEM_LABEL: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 16, 0, 0x40, 0, 0];

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
        out(b"system_test: check ");
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
struct Buf([u8; 1024]);

fn query(token: Handle, class: u32, b: &mut Buf) -> bool {
    let mut n = 0;
    // SAFETY: our buffer.
    unsafe { GetTokenInformation(token, class, b.0.as_mut_ptr(), 1024, &mut n) != 0 }
}

/// The first SID pointer of a TOKEN_USER/TOKEN_OWNER/TOKEN_MANDATORY_LABEL.
fn first_sid(b: &Buf) -> *const u8 {
    // SAFETY: the structure starts with the SID pointer.
    unsafe { b.0.as_ptr().cast::<*const u8>().read() }
}

/// The attributes of privilege `luid` (None: not held).
fn privilege(token: Handle, luid: u32) -> Option<u32> {
    let mut b = Buf([0; 1024]);
    if !query(token, 3, &mut b) {
        return None;
    }
    let rd = |at: usize| u32::from_le_bytes([b.0[at], b.0[at + 1], b.0[at + 2], b.0[at + 3]]);
    (0..rd(0) as usize).map(|i| 4 + 12 * i).find(|&e| rd(e) == luid).map(|e| rd(e + 8))
}

/// The child: exits with its user's RID (the last sub-authority).
fn child() -> u32 {
    let mut token = core::ptr::null_mut();
    let mut b = Buf([0; 1024]);
    // SAFETY: plain calls; a SID inside `b`.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), 8, &mut token) == 0 || !query(token, 1, &mut b) {
            return 1;
        }
        let sid = first_sid(&b);
        let count = usize::from(*sid.add(1));
        sid.add(8 + 4 * (count - 1)).cast::<u32>().read_unaligned()
    }
}

fn is_child() -> bool {
    // SAFETY: the process's command line, NUL-terminated.
    unsafe {
        let line = GetCommandLineW();
        let mut n = 0;
        while *line.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(line, n).ends_with(&w::<5>("child"))
    }
}

fn run() -> Result<(), u32> {
    let c = Checks(core::cell::Cell::new(0));
    let null = core::ptr::null_mut();
    // SAFETY: plain calls with our buffers and handles throughout.
    unsafe {
        let mut token = null;
        c.ok(OpenProcessToken(GetCurrentProcess(), 0x8 | 0x20, &mut token) != 0, last_error())?;
        let mut b = Buf([0; 1024]);
        c.ok(query(token, 1, &mut b), last_error())?;
        c.ok(EqualSid(first_sid(&b), SYSTEM.as_ptr()) != 0, 0)?;
        c.ok(query(token, 4, &mut b), 0)?; // TokenOwner
        c.ok(EqualSid(first_sid(&b), ADMINISTRATORS.as_ptr()) != 0, 0)?;
        c.ok(query(token, 25, &mut b), 0)?; // TokenIntegrityLevel
        c.ok(EqualSid(first_sid(&b), SYSTEM_LABEL.as_ptr()) != 0, 0)?;
        // Administrators: an owner group.
        c.ok(query(token, 2, &mut b), 0)?;
        let count = u32::from_le_bytes([b.0[0], b.0[1], b.0[2], b.0[3]]) as usize;
        let mut admin = 0;
        for i in 0..count {
            let e = b.0.as_ptr().add(8 + 16 * i);
            if EqualSid(e.cast::<*const u8>().read(), ADMINISTRATORS.as_ptr()) != 0 {
                admin = e.add(8).cast::<u32>().read();
            }
        }
        c.ok(admin & SE_GROUP_OWNER != 0 && admin & 4 != 0, admin.into())?;
        // Privileges.
        let tcb = privilege(token, SE_TCB).unwrap_or(0);
        c.ok(tcb & SE_PRIVILEGE_ENABLED != 0, tcb.into())?;
        c.eq(privilege(token, SE_ASSIGN_PRIMARY_TOKEN).map_or(9, u64::from), 0)?;
        c.eq(privilege(token, SE_BACKUP).map_or(9, u64::from), 0)?;

        // A file only BUILTIN\Users may use, created as SYSTEM.
        let path = w::<64>("C:\\system_test.txt");
        let mut sd = null;
        let text = w::<32>("D:P(A;;FA;;;BU)");
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
        let h = CreateFileW(path.as_ptr(), GENERIC_WRITE, 0, &sa, 1, 0x80, null);
        c.ok(h as isize != -1, last_error())?;
        CloseHandle(h);
        let mut n = 0;
        c.ok(
            GetFileSecurityW(path.as_ptr(), 1, b.0.as_mut_ptr(), 1024, &mut n) != 0,
            last_error(),
        )?;
        let (mut owner, mut defaulted) = (core::ptr::null(), 0);
        c.ok(GetSecurityDescriptorOwner(b.0.as_ptr(), &mut owner, &mut defaulted) != 0, 0)?;
        c.ok(!owner.is_null() && EqualSid(owner, ADMINISTRATORS.as_ptr()) != 0, 0)?;
        let open = |flags: u32| {
            CreateFileW(path.as_ptr(), GENERIC_READ, 7, core::ptr::null(), 3, flags, null)
        };
        c.ok(open(0) as isize == -1, 0)?;
        c.eq(last_error(), ERROR_ACCESS_DENIED.into())?;
        c.ok(open(FILE_FLAG_BACKUP_SEMANTICS) as isize == -1, 0)?;
        c.eq(last_error(), ERROR_ACCESS_DENIED.into())?;
        let enable = [1u32, SE_BACKUP, 0, SE_PRIVILEGE_ENABLED];
        SetLastError(0xdead);
        c.ok(
            AdjustTokenPrivileges(token, 0, enable.as_ptr(), 0, null.cast(), null.cast()) != 0,
            last_error(),
        )?;
        c.eq(last_error(), 0)?;
        let h = open(FILE_FLAG_BACKUP_SEMANTICS);
        c.ok(h as isize != -1, last_error())?;
        CloseHandle(h);
        c.ok(open(0) as isize == -1, 0)?;
        c.ok(DeleteFileW(path.as_ptr()) != 0, last_error())?;

        // CreateProcessAsUserW of another user's token.
        let mut test = null;
        let (user, dot, password) = (w::<16>("galatest"), w::<4>("."), w::<16>("Gala-Test-1"));
        c.ok(
            LogonUserW(user.as_ptr(), dot.as_ptr(), password.as_ptr(), 2, 0, &mut test) != 0,
            last_error(),
        )?;
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
        c.ok(r != 0, last_error())?;
        c.eq(u64::from(WaitForSingleObject(info[0] as Handle, 30_000)), 0)?;
        let mut code = 0;
        c.ok(GetExitCodeProcess(info[0] as Handle, &mut code) != 0, 0)?;
        c.ok(query(test, 1, &mut b), 0)?;
        let sid = first_sid(&b);
        let rid = sid.add(8 + 4 * (usize::from(*sid.add(1)) - 1)).cast::<u32>().read_unaligned();
        c.eq(code.into(), rid.into())?;
        CloseHandle(info[0] as Handle);
        CloseHandle(info[1] as Handle);
        CloseHandle(test);
        CloseHandle(token);
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
            out(b"system_test: ok\r\n");
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
