//! Restricted tokens and integrity levels (docs/SECURITY.md, phase S5) as
//! PostgreSQL uses them (pg_ctl's and initdb's CreateRestrictedProcess,
//! pgwin32_is_admin), with the results Windows documents
//! (CreateRestrictedToken, CheckTokenMembership, SetTokenInformation,
//! AccessCheck's rules for deny-only groups and mandatory labels). Runs as
//! SYSTEM (gala: `as=system`; Windows: `psexec -s`), which is in
//! BUILTIN\Administrators.
//!
//! - CheckTokenMembership(NULL, Administrators): TRUE for SYSTEM. A
//!   primary token passed to it: ERROR_NO_IMPERSONATION_TOKEN.
//! - CreateRestrictedToken with Administrators and Power Users to disable
//!   and every privilege but SeChangeNotify and SeCreateGlobal deleted
//!   (pg_ctl): Administrators stays, deny-only (SE_GROUP_USE_FOR_DENY_ONLY,
//!   not enabled); the token holds only those two privileges;
//!   CheckTokenMembership of it (an impersonation copy) is FALSE.
//! - AddUserToTokenDacl: TokenDefaultDacl read, an ACE for the user added,
//!   SetTokenInformation(TokenDefaultDacl) kept (one ACE more).
//! - A file only Administrators may open (D:P(A;;FA;;;BA)): SYSTEM opens it,
//!   the restricted token (impersonated) is denied; a deny ACE for
//!   Administrators (D:(D;;FA;;;BA)(A;;FA;;;WD)) still denies a deny-only
//!   Administrators.
//! - A child started with CreateProcessAsUserW and the restricted token is
//!   not an administrator (its CheckTokenMembership is FALSE); also with
//!   CreateProcessAsUserA.
//! - Integrity: SetTokenInformation(TokenIntegrityLevel, Low) on a
//!   duplicate; GetTokenInformation reads Low back; impersonating it, a
//!   new file in C:\gala (no label: Medium) is denied (no write up) and
//!   an existing one opens for reading.
//!
//! Prints `restricted_test: ok` or exits with the number of the failing
//! check.

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
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetCurrentProcess() -> Handle;
    fn GetCommandLineW() -> *const u16;
    fn GetModuleFileNameW(module: Handle, name: *mut u16, size: u32) -> u32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *const SecurityAttributes,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn DeleteFileW(name: *const u16) -> Bool;
    fn LocalFree(p: *mut c_void) -> *mut c_void;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> Bool;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        out: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> Bool;
    fn SetTokenInformation(token: Handle, class: u32, info: *const u8, len: u32) -> Bool;
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
    fn CheckTokenMembership(token: Handle, sid: *const u8, member: *mut Bool) -> Bool;
    fn ImpersonateLoggedOnUser(token: Handle) -> Bool;
    fn RevertToSelf() -> Bool;
    fn EqualSid(a: *const u8, b: *const u8) -> Bool;
    fn InitializeAcl(acl: *mut u8, length: u32, revision: u32) -> Bool;
    fn AddAccessAllowedAce(acl: *mut u8, revision: u32, mask: u32, sid: *const u8) -> Bool;
    fn GetAce(acl: *const u8, index: u32, ace: *mut *const u8) -> Bool;
    fn AddAce(acl: *mut u8, revision: u32, start: u32, list: *const u8, length: u32) -> Bool;
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
    #[allow(clippy::too_many_arguments)]
    fn CreateProcessAsUserA(
        token: Handle,
        application: *const u8,
        command_line: *mut u8,
        process_attributes: *mut c_void,
        thread_attributes: *mut c_void,
        inherit: Bool,
        flags: u32,
        environment: *mut c_void,
        directory: *const u8,
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
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_NO_IMPERSONATION_TOKEN: u32 = 1309;
const SE_GROUP_ENABLED: u32 = 4;
const SE_GROUP_USE_FOR_DENY_ONLY: u32 = 0x10;
const SE_GROUP_INTEGRITY: u32 = 0x20;
/// TOKEN_INFORMATION_CLASS values.
const TOKEN_GROUPS: u32 = 2;
const TOKEN_PRIVILEGES: u32 = 3;
const TOKEN_DEFAULT_DACL: u32 = 6;
const TOKEN_INTEGRITY_LEVEL: u32 = 25;
/// SeChangeNotifyPrivilege and SeCreateGlobalPrivilege.
const SE_CHANGE_NOTIFY: u32 = 23;
const SE_CREATE_GLOBAL: u32 = 30;
/// The child's exit codes.
const CHILD_NOT_ADMIN: u32 = 0x51;
const CHILD_ADMIN: u32 = 0x52;

/// S-1-5-32-544 (Administrators), S-1-5-32-547 (Power Users), S-1-16-4096
/// (Low), S-1-1-0 (Everyone).
const ADMINS: [u8; 16] = [1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 0x20, 2, 0, 0];
const POWER_USERS: [u8; 16] = [1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 0x23, 2, 0, 0];
const LOW: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 16, 0, 0x10, 0, 0];

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
        out(b"restricted_test: check ");
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

/// Whether the caller (thread token, else process token) is in `sid`.
fn member(token: Handle, sid: &[u8]) -> Option<bool> {
    let mut is = 0;
    // SAFETY: a plain call with our output.
    (unsafe { CheckTokenMembership(token, sid.as_ptr(), &mut is) } != 0).then_some(is != 0)
}

/// The attributes of group `sid` in the token's TokenGroups.
fn group_attributes(token: Handle, sid: &[u8]) -> Option<u32> {
    let mut b = Buf([0; 1024]);
    let mut n = 0;
    // SAFETY: our buffer; TOKEN_GROUPS from the system.
    unsafe {
        if GetTokenInformation(token, TOKEN_GROUPS, b.0.as_mut_ptr(), 1024, &mut n) == 0 {
            return None;
        }
        let count = b.0.as_ptr().cast::<u32>().read() as usize;
        (0..count).find_map(|i| {
            let e = b.0.as_ptr().add(8 + 16 * i);
            (EqualSid(e.cast::<*const u8>().read(), sid.as_ptr()) != 0)
                .then(|| e.add(8).cast::<u32>().read())
        })
    }
}

/// Opens `path` with `access` (existing file), closing it again.
fn open(path: &[u16], access: u32) -> Result<(), u32> {
    // SAFETY: a NUL-terminated path.
    let h = unsafe {
        CreateFileW(
            path.as_ptr(),
            access,
            7,
            core::ptr::null(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        )
    };
    if h as isize == -1 {
        // SAFETY: a plain call.
        return Err(unsafe { GetLastError() });
    }
    // SAFETY: ours.
    unsafe { CloseHandle(h) };
    Ok(())
}

/// Creates `path` with the descriptor of SDDL `sddl`.
fn create_with(path: &[u16], sddl: &[u16]) -> bool {
    let mut sd = core::ptr::null_mut();
    // SAFETY: our strings; the descriptor is freed after use.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut sd,
            core::ptr::null_mut(),
        ) == 0
        {
            return false;
        }
        let sa = SecurityAttributes { length: 24, descriptor: sd, inherit: 0 };
        let h = CreateFileW(
            path.as_ptr(),
            GENERIC_WRITE,
            0,
            &sa,
            CREATE_ALWAYS,
            0,
            core::ptr::null_mut(),
        );
        LocalFree(sd);
        if h as isize == -1 {
            return false;
        }
        CloseHandle(h);
    }
    true
}

/// The child: is it in Administrators (as pgwin32_is_admin asks)?
fn child() -> u32 {
    match member(core::ptr::null_mut(), &ADMINS) {
        Some(false) => CHILD_NOT_ADMIN,
        Some(true) => CHILD_ADMIN,
        None => 3,
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

fn run() -> Result<(), u32> {
    let c = Checks(core::cell::Cell::new(0));
    let null = core::ptr::null_mut();
    // SAFETY: plain calls with our buffers and handles throughout.
    unsafe {
        let mut mine = null;
        c.ok(OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut mine) != 0, 0)?;
        c.ok(member(null, &ADMINS) == Some(true), last_error())?;
        // A primary token is not an impersonation token.
        let mut is = 0;
        c.eq(CheckTokenMembership(mine, ADMINS.as_ptr(), &mut is) as u64, 0)?;
        c.eq(last_error(), ERROR_NO_IMPERSONATION_TOKEN.into())?;

        // pg_ctl's restricted token: Administrators and Power Users
        // deny-only, all privileges but two deleted.
        let mut b = Buf([0; 1024]);
        let mut n = 0;
        c.ok(GetTokenInformation(mine, TOKEN_PRIVILEGES, b.0.as_mut_ptr(), 1024, &mut n) != 0, 0)?;
        let count = b.0.as_ptr().cast::<u32>().read() as usize;
        let mut delete = [[0u32; 3]; 40];
        let mut deleting = 0;
        for i in 0..count.min(40) {
            let p = b.0.as_ptr().add(4 + 12 * i).cast::<u32>();
            let luid = p.read();
            if luid != SE_CHANGE_NOTIFY && luid != SE_CREATE_GLOBAL {
                delete[deleting] = [luid, p.add(1).read(), 0];
                deleting += 1;
            }
        }
        let disable = [[ADMINS.as_ptr() as usize, 0], [POWER_USERS.as_ptr() as usize, 0]];
        let mut restricted = null;
        let ok = CreateRestrictedToken(
            mine,
            0,
            2,
            disable.as_ptr(),
            deleting as u32,
            delete.as_ptr(),
            0,
            core::ptr::null(),
            &mut restricted,
        );
        c.ok(ok != 0, last_error())?;
        let admins = group_attributes(restricted, &ADMINS).unwrap_or(0);
        c.ok(
            admins & SE_GROUP_USE_FOR_DENY_ONLY != 0 && admins & SE_GROUP_ENABLED == 0,
            admins.into(),
        )?;
        c.ok(
            GetTokenInformation(restricted, TOKEN_PRIVILEGES, b.0.as_mut_ptr(), 1024, &mut n) != 0,
            0,
        )?;
        let count = b.0.as_ptr().cast::<u32>().read() as usize;
        let kept = (0..count).all(|i| {
            let luid = b.0.as_ptr().add(4 + 12 * i).cast::<u32>().read();
            luid == SE_CHANGE_NOTIFY || luid == SE_CREATE_GLOBAL
        });
        c.ok(count >= 1 && kept, count as u64)?;
        let mut imp = null;
        c.ok(DuplicateTokenEx(restricted, TOKEN_ALL_ACCESS, null, 2, 2, &mut imp) != 0, 0)?;
        c.ok(member(imp, &ADMINS) == Some(false), last_error())?;

        // AddUserToTokenDacl: the default DACL with an ACE for the user.
        c.ok(
            GetTokenInformation(restricted, TOKEN_DEFAULT_DACL, b.0.as_mut_ptr(), 1024, &mut n)
                != 0,
            0,
        )?;
        let old = b.0.as_ptr().cast::<*const u8>().read();
        c.ok(!old.is_null(), 0)?;
        let old_count = u32::from(old.add(4).cast::<u16>().read_unaligned());
        let mut user = Buf([0; 1024]);
        c.ok(GetTokenInformation(restricted, 1, user.0.as_mut_ptr(), 1024, &mut n) != 0, 0)?;
        let user_sid = user.0.as_ptr().cast::<*const u8>().read();
        let mut acl = Buf([0; 1024]);
        c.ok(InitializeAcl(acl.0.as_mut_ptr(), 512, 2) != 0, last_error())?;
        for i in 0..old_count {
            let mut ace = core::ptr::null();
            c.ok(GetAce(old, i, &mut ace) != 0, last_error())?;
            let size = u32::from(ace.add(2).cast::<u16>().read_unaligned());
            c.ok(AddAce(acl.0.as_mut_ptr(), 2, u32::MAX, ace, size) != 0, last_error())?;
        }
        c.ok(AddAccessAllowedAce(acl.0.as_mut_ptr(), 2, 0x1000_0000, user_sid) != 0, last_error())?;
        let set = [acl.0.as_ptr() as usize];
        c.ok(
            SetTokenInformation(restricted, TOKEN_DEFAULT_DACL, set.as_ptr().cast(), 8) != 0,
            last_error(),
        )?;
        c.ok(
            GetTokenInformation(restricted, TOKEN_DEFAULT_DACL, b.0.as_mut_ptr(), 1024, &mut n)
                != 0,
            0,
        )?;
        let new = b.0.as_ptr().cast::<*const u8>().read();
        c.eq(u64::from(new.add(4).cast::<u16>().read_unaligned()), u64::from(old_count + 1))?;

        // A deny-only Administrators: not granted by an allow ACE, still
        // denied by a deny ACE.
        let only_admins = w::<64>("C:\\gala\\restricted_admins.txt");
        let denied = w::<64>("C:\\gala\\restricted_denied.txt");
        c.ok(create_with(&only_admins, &w::<32>("D:P(A;;FA;;;BA)")), last_error())?;
        c.ok(create_with(&denied, &w::<48>("D:(D;;FA;;;BA)(A;;FA;;;WD)")), last_error())?;
        c.ok(open(&only_admins, GENERIC_READ).is_ok(), 0)?;
        c.ok(ImpersonateLoggedOnUser(restricted) != 0, last_error())?;
        c.eq(
            open(&only_admins, GENERIC_READ).err().unwrap_or(0).into(),
            ERROR_ACCESS_DENIED.into(),
        )?;
        c.eq(open(&denied, GENERIC_READ).err().unwrap_or(0).into(), ERROR_ACCESS_DENIED.into())?;
        c.ok(RevertToSelf() != 0, 0)?;

        // The child of the restricted token is no administrator.
        let mut exe = [0u16; 260];
        let len = GetModuleFileNameW(null, exe.as_mut_ptr(), 260) as usize;
        c.ok(len > 0, 0)?;
        let mut line = [0u16; 300];
        line[0] = u16::from(b'"');
        line[1..=len].copy_from_slice(&exe[..len]);
        for (i, ch) in "\" child".encode_utf16().enumerate() {
            line[len + 1 + i] = ch;
        }
        let mut startup = [0u8; 104];
        startup[0] = 104;
        let mut info = [0usize; 3];
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
        c.eq(code.into(), CHILD_NOT_ADMIN.into())?;
        CloseHandle(info[0] as Handle);
        CloseHandle(info[1] as Handle);
        // The same through CreateProcessAsUserA (initdb's way).
        let mut narrow = [0u8; 300];
        for (i, &ch) in line.iter().take_while(|&&ch| ch != 0).enumerate() {
            narrow[i] = ch as u8;
        }
        let r = CreateProcessAsUserA(
            restricted,
            core::ptr::null(),
            narrow.as_mut_ptr(),
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
        c.ok(GetExitCodeProcess(info[0] as Handle, &mut code) != 0, 0)?;
        c.eq(code.into(), CHILD_NOT_ADMIN.into())?;
        CloseHandle(info[0] as Handle);
        CloseHandle(info[1] as Handle);

        // Integrity: a Low duplicate can read but not create in C:\gala.
        let mut low = null;
        c.ok(DuplicateTokenEx(mine, TOKEN_ALL_ACCESS, null, 2, 2, &mut low) != 0, 0)?;
        let label = [LOW.as_ptr() as usize, SE_GROUP_INTEGRITY as usize];
        c.ok(
            SetTokenInformation(low, TOKEN_INTEGRITY_LEVEL, label.as_ptr().cast(), 16 + 12) != 0,
            last_error(),
        )?;
        c.ok(
            GetTokenInformation(low, TOKEN_INTEGRITY_LEVEL, b.0.as_mut_ptr(), 1024, &mut n) != 0,
            0,
        )?;
        let level = b.0.as_ptr().cast::<*const u8>().read();
        c.ok(EqualSid(level, LOW.as_ptr()) != 0, 0)?;
        c.ok(ImpersonateLoggedOnUser(low) != 0, last_error())?;
        let lowfile = w::<64>("C:\\gala\\restricted_low.txt");
        let h = CreateFileW(
            lowfile.as_ptr(),
            GENERIC_WRITE,
            0,
            core::ptr::null(),
            CREATE_ALWAYS,
            0,
            null,
        );
        let error = last_error();
        c.ok(h as isize == -1 && error == ERROR_ACCESS_DENIED.into(), error)?;
        let read = open(&only_admins, GENERIC_READ);
        c.ok(read.is_ok(), read.err().unwrap_or(0).into())?;
        c.ok(RevertToSelf() != 0, 0)?;

        DeleteFileW(only_admins.as_ptr());
        DeleteFileW(denied.as_ptr());
        for h in [mine, restricted, imp, low] {
            CloseHandle(h);
        }
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
            out(b"restricted_test: ok\r\n");
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
