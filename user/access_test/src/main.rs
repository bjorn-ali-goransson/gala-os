//! Access checks at open (docs/SECURITY.md, phase S3), with the results
//! real Windows gives, from the documented contracts ("How DACLs Control
//! Access to an Object", AccessCheck, LogonUserW, "Privilege Constants",
//! "File Security and Access Rights"). It runs as a standard user and
//! needs a second local account, `galatest` with password `Gala-Test-1`
//! (on Windows an administrator creates it once: `net user galatest
//! Gala-Test-1 /add`; gala's `access_test` scenario seeds it).
//!
//! - LogonUserW: the account's primary token (another user than the
//!   caller's); a wrong password or unknown user is ERROR_LOGON_FAILURE.
//!   LookupAccountNameW and LookupAccountSidW know the account (a
//!   SidTypeUser); impersonating it, GetUserNameW names it.
//! - As the caller, a directory whose protected DACL gives the caller
//!   FILE_ALL_ACCESS and `galatest` FILE_GENERIC_READ (both inherited), a
//!   file in it, a file whose DACL denies `galatest` writing before
//!   allowing Everyone everything (the write rights without the
//!   SYNCHRONIZE and READ_CONTROL FILE_GENERIC_WRITE includes), a file
//!   only `galatest` may use (the caller only SYNCHRONIZE, which
//!   CreateFileW always asks for), a
//!   registry key (the caller's hive, by its HKEY_USERS path) reading for
//!   `galatest`, and a named event `galatest` may only signal.
//! - Impersonating `galatest` (ImpersonateLoggedOnUser): reading is
//!   allowed, writing is ERROR_ACCESS_DENIED; MAXIMUM_ALLOWED opens with
//!   exactly FILE_GENERIC_READ; creating a file in the directory is
//!   denied (no FILE_ADD_FILE); the deny ACE wins; the key opens for
//!   KEY_READ, not KEY_SET_VALUE, and no subkey can be created; the event
//!   opens for EVENT_MODIFY_STATE, not EVENT_ALL_ACCESS; an event it
//!   creates with an empty DACL gives its owner READ_CONTROL and WRITE_DAC
//!   only (the owner's implicit rights), and the caller nothing.
//! - Back as the caller: writing works; the file only `galatest` may use
//!   opens for READ_CONTROL | WRITE_DAC (the caller owns it) but not for
//!   reading, also with FILE_FLAG_BACKUP_SEMANTICS (a standard user holds
//!   no SeBackupPrivilege: AdjustTokenPrivileges says ERROR_NOT_ALL_ASSIGNED).
//! - A restricted token (restricting SID Everyone) cannot read the file
//!   its user may read but Everyone may not, and can read the one
//!   Everyone may.
//! - Impersonating at identification level, opening a file is
//!   ERROR_BAD_IMPERSONATION_LEVEL.
//! - Deleting the file only `galatest` may use works through the
//!   directory's FILE_DELETE_CHILD.
//!
//! Prints `access_test: ok` or exits with the number of the failing
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
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetCurrentProcess() -> Handle;
    fn CreateDirectoryW(path: *const u16, sa: *const SecurityAttributes) -> Bool;
    fn RemoveDirectoryW(path: *const u16) -> Bool;
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
    fn CreateEventW(
        sa: *const SecurityAttributes,
        manual: Bool,
        initial: Bool,
        name: *const u16,
    ) -> Handle;
    fn OpenEventW(access: u32, inherit: Bool, name: *const u16) -> Handle;
    fn SetEvent(h: Handle) -> Bool;
    fn LocalFree(p: *mut c_void) -> *mut c_void;
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
    fn ImpersonateSelf(level: u32) -> Bool;
    fn RevertToSelf() -> Bool;
    fn SetThreadToken(thread: *mut Handle, token: Handle) -> Bool;
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
    fn EqualSid(a: *const u8, b: *const u8) -> Bool;
    fn GetUserNameW(buffer: *mut u16, size: *mut u32) -> Bool;
    fn LookupAccountNameW(
        system: *const u16,
        name: *const u16,
        sid: *mut u8,
        sid_size: *mut u32,
        domain: *mut u16,
        domain_size: *mut u32,
        kind: *mut u32,
    ) -> Bool;
    fn LookupAccountSidW(
        system: *const u16,
        sid: *const u8,
        name: *mut u16,
        name_size: *mut u32,
        domain: *mut u16,
        domain_size: *mut u32,
        kind: *mut u32,
    ) -> Bool;
    fn ConvertSidToStringSidW(sid: *const u8, text: *mut *mut u16) -> Bool;
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        text: *const u16,
        revision: u32,
        sd: *mut *mut c_void,
        size: *mut u32,
    ) -> Bool;
    fn RegCreateKeyExW(
        key: usize,
        sub: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        access: u32,
        sa: *const SecurityAttributes,
        out: *mut usize,
        disposition: *mut u32,
    ) -> i32;
    fn RegOpenKeyExW(
        key: usize,
        sub: *const u16,
        options: u32,
        access: u32,
        out: *mut usize,
    ) -> i32;
    fn RegCloseKey(key: usize) -> i32;
    fn RegDeleteKeyW(key: usize, sub: *const u16) -> i32;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQueryObject(h: Handle, class: u32, buf: *mut u8, len: u32, ret: *mut u32) -> u32;
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
const MAXIMUM_ALLOWED: u32 = 0x0200_0000;
const READ_CONTROL: u32 = 0x0002_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const FILE_GENERIC_READ: u32 = 0x0012_0089;
const EVENT_MODIFY_STATE: u32 = 0x0002;
const EVENT_ALL_ACCESS: u32 = 0x001f_0003;
const KEY_READ: u32 = 0x0002_0019;
const KEY_SET_VALUE: u32 = 0x0002;
const KEY_ALL_ACCESS: u32 = 0x000f_003f;
const HKEY_USERS: usize = 0x8000_0003;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_NOT_ALL_ASSIGNED: u32 = 1300;
const ERROR_LOGON_FAILURE: u32 = 1326;
const ERROR_BAD_IMPERSONATION_LEVEL: u32 = 1346;

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
        out(b"access_test: check ");
        out(&hex(self.0.get()));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0.get())
    }

    fn eq(&self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// A call that must fail with `error` (`failed`: it did).
    fn fails(&self, failed: bool, error: u32) -> Result<(), u32> {
        self.ok(failed, 0)?;
        self.eq(last_error(), error.into())
    }
}

fn last_error() -> u64 {
    // SAFETY: a plain call.
    u64::from(unsafe { GetLastError() })
}

/// A NUL-terminated UTF-16 string built from parts.
struct W {
    b: [u16; 512],
    n: usize,
}

impl W {
    fn new() -> W {
        W { b: [0; 512], n: 0 }
    }

    fn s(mut self, s: &str) -> W {
        for c in s.encode_utf16() {
            self.b[self.n] = c;
            self.n += 1;
        }
        self
    }

    /// Appends a NUL-terminated UTF-16 string.
    fn w(mut self, p: *const u16) -> W {
        // SAFETY: a NUL-terminated string from the system.
        unsafe {
            let mut i = 0;
            while *p.add(i) != 0 {
                self.b[self.n] = *p.add(i);
                self.n += 1;
                i += 1;
            }
        }
        self
    }

    fn p(&self) -> *const u16 {
        self.b.as_ptr()
    }
}

fn w(s: &str) -> W {
    W::new().s(s)
}

fn sd(c: &Checks, text: &W) -> Result<SecurityAttributes, u32> {
    let mut d = core::ptr::null_mut();
    // SAFETY: our string and output.
    let r = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.p(),
            1,
            &mut d,
            core::ptr::null_mut(),
        )
    };
    c.ok(r != 0, last_error())?;
    Ok(SecurityAttributes { length: 24, descriptor: d, inherit: 0 })
}

#[repr(C, align(8))]
struct Buf([u8; 512]);

/// TokenUser's SID of `token` (in `b`).
fn user_of(c: &Checks, token: Handle, b: &mut Buf) -> Result<*const u8, u32> {
    let mut n = 0;
    // SAFETY: our buffer.
    c.ok(
        unsafe { GetTokenInformation(token, 1, b.0.as_mut_ptr(), 512, &mut n) } != 0,
        last_error(),
    )?;
    // SAFETY: TOKEN_USER starts with the SID pointer.
    Ok(unsafe { b.0.as_ptr().cast::<*const u8>().read() })
}

fn open(path: &W, access: u32, flags: u32) -> Handle {
    // SAFETY: our path; FILE_SHARE_ALL, OPEN_EXISTING.
    unsafe { CreateFileW(path.p(), access, 7, core::ptr::null(), 3, flags, core::ptr::null_mut()) }
}

fn invalid(h: Handle) -> bool {
    h as isize == -1
}

/// The handle's granted access (OBJECT_BASIC_INFORMATION.GrantedAccess).
fn granted(h: Handle) -> u64 {
    let mut b = [0u32; 14];
    // SAFETY: our buffer of 56 bytes.
    unsafe { NtQueryObject(h, 0, b.as_mut_ptr().cast(), 56, core::ptr::null_mut()) };
    u64::from(b[1])
}

fn run() -> Result<(), u32> {
    let c = Checks(core::cell::Cell::new(0));
    let null = core::ptr::null_mut();
    // SAFETY: plain calls with our buffers and handles throughout.
    unsafe {
        // LogonUserW.
        let (user, dot) = (w("galatest"), w("."));
        let mut test = null;
        c.ok(
            LogonUserW(user.p(), dot.p(), w("Gala-Test-1").p(), 2, 0, &mut test) != 0,
            last_error(),
        )?;
        let mut nothing = null;
        c.fails(
            LogonUserW(user.p(), dot.p(), w("wrong").p(), 2, 0, &mut nothing) == 0,
            ERROR_LOGON_FAILURE,
        )?;
        c.fails(
            LogonUserW(w("nobody-here").p(), dot.p(), w("x").p(), 2, 0, &mut nothing) == 0,
            ERROR_LOGON_FAILURE,
        )?;
        let mut mine = null;
        c.ok(OpenProcessToken(GetCurrentProcess(), 0x2 | 0x8 | 0x20, &mut mine) != 0, 0)?;
        let (mut b1, mut b2) = (Buf([0; 512]), Buf([0; 512]));
        let (me_sid, test_sid) = (user_of(&c, mine, &mut b1)?, user_of(&c, test, &mut b2)?);
        c.ok(EqualSid(me_sid, test_sid) == 0, 0)?;
        // The account by name and by SID (a local user).
        let mut sid = [0u8; 68];
        let (mut sid_size, mut domain_size, mut kind) = (68u32, 64u32, 0u32);
        let mut domain = [0u16; 64];
        let r = LookupAccountNameW(
            core::ptr::null(),
            user.p(),
            sid.as_mut_ptr(),
            &mut sid_size,
            domain.as_mut_ptr(),
            &mut domain_size,
            &mut kind,
        );
        c.ok(r != 0, last_error())?;
        c.ok(EqualSid(sid.as_ptr(), test_sid) != 0, 0)?;
        c.eq(kind.into(), 1)?; // SidTypeUser
        let mut name = [0u16; 64];
        let mut name_size = 64u32;
        domain_size = 64;
        let r = LookupAccountSidW(
            core::ptr::null(),
            test_sid,
            name.as_mut_ptr(),
            &mut name_size,
            domain.as_mut_ptr(),
            &mut domain_size,
            &mut kind,
        );
        c.ok(r != 0, last_error())?;
        c.ok(name[..9] == user.b[..9], 0)?; // "galatest" and its NUL
        let (mut me_text, mut test_text) = (null.cast(), null.cast());
        c.ok(ConvertSidToStringSidW(me_sid, &mut me_text) != 0, 0)?;
        c.ok(ConvertSidToStringSidW(test_sid, &mut test_text) != 0, 0)?;

        // The objects, as the caller.
        let dir = w("C:\\access_test");
        let readable = w("C:\\access_test\\r.txt");
        let deny = w("C:\\access_test\\deny.txt");
        let private = w("C:\\access_test\\private.txt");
        let created = w("C:\\access_test\\new.txt");
        let dacl =
            W::new().s("D:P(A;OICI;FA;;;").w(me_text).s(")(A;OICI;FR;;;").w(test_text).s(")");
        let sa = sd(&c, &dacl)?;
        c.ok(CreateDirectoryW(dir.p(), &sa) != 0, last_error())?;
        let h = CreateFileW(readable.p(), GENERIC_WRITE, 0, core::ptr::null(), 1, 0x80, null);
        c.ok(!invalid(h), last_error())?;
        let mut n = 0;
        c.ok(WriteFile(h, b"gala".as_ptr(), 4, &mut n, null) != 0, 0)?;
        CloseHandle(h);
        let sa = sd(&c, &W::new().s("D:P(D;;0x116;;;").w(test_text).s(")(A;;FA;;;WD)"))?;
        let h = CreateFileW(deny.p(), GENERIC_WRITE, 0, &sa, 1, 0x80, null);
        c.ok(!invalid(h), last_error())?;
        CloseHandle(h);
        let sa = sd(
            &c,
            &W::new().s("D:P(A;;FA;;;").w(test_text).s(")(A;;0x100000;;;").w(me_text).s(")"),
        )?;
        let h = CreateFileW(private.p(), GENERIC_WRITE, 0, &sa, 1, 0x80, null);
        c.ok(!invalid(h), last_error())?;
        CloseHandle(h);
        let key_path = W::new().w(me_text).s("\\Software\\gala-access-test");
        let sa =
            sd(&c, &W::new().s("D:P(A;CI;KA;;;").w(me_text).s(")(A;CI;KR;;;").w(test_text).s(")"))?;
        let (mut key, mut disposition) = (0usize, 0u32);
        let r = RegCreateKeyExW(
            HKEY_USERS,
            key_path.p(),
            0,
            core::ptr::null(),
            0,
            KEY_ALL_ACCESS,
            &sa,
            &mut key,
            &mut disposition,
        );
        c.eq(r as u64, 0)?;
        RegCloseKey(key);
        let event_name = w("gala-access-test");
        let sa = sd(
            &c,
            &W::new().s("D:P(A;;0x1f0003;;;").w(me_text).s(")(A;;0x100002;;;").w(test_text).s(")"),
        )?;
        let event = CreateEventW(&sa, 1, 0, event_name.p());
        c.ok(!event.is_null(), last_error())?;

        // As galatest.
        c.ok(ImpersonateLoggedOnUser(test) != 0, last_error())?;
        let mut name = [0u16; 64];
        let mut size = 64u32;
        c.ok(GetUserNameW(name.as_mut_ptr(), &mut size) != 0, last_error())?;
        c.ok(name[..9] == user.b[..9], u64::from(name[0]))?;
        let h = open(&readable, GENERIC_READ, 0);
        c.ok(!invalid(h), last_error())?;
        let mut text = [0u8; 8];
        c.ok(ReadFile(h, text.as_mut_ptr(), 8, &mut n, null) != 0 && n == 4, n.into())?;
        CloseHandle(h);
        c.fails(invalid(open(&readable, GENERIC_WRITE, 0)), ERROR_ACCESS_DENIED)?;
        let h = open(&readable, MAXIMUM_ALLOWED, 0);
        c.ok(!invalid(h), last_error())?;
        c.eq(granted(h), FILE_GENERIC_READ.into())?;
        CloseHandle(h);
        let h = CreateFileW(created.p(), GENERIC_WRITE, 0, core::ptr::null(), 1, 0x80, null);
        c.fails(invalid(h), ERROR_ACCESS_DENIED)?;
        let h = open(&deny, GENERIC_READ, 0);
        c.ok(!invalid(h), last_error())?;
        CloseHandle(h);
        c.fails(invalid(open(&deny, GENERIC_WRITE, 0)), ERROR_ACCESS_DENIED)?;
        let mut k = 0usize;
        c.eq(RegOpenKeyExW(HKEY_USERS, key_path.p(), 0, KEY_READ, &mut k) as u64, 0)?;
        let child = w("child");
        let mut sub = 0usize;
        let r = RegCreateKeyExW(
            k,
            child.p(),
            0,
            core::ptr::null(),
            0,
            KEY_READ,
            core::ptr::null(),
            &mut sub,
            &mut disposition,
        );
        c.eq(r as u64, ERROR_ACCESS_DENIED.into())?;
        RegCloseKey(k);
        let r = RegOpenKeyExW(HKEY_USERS, key_path.p(), 0, KEY_SET_VALUE, &mut k);
        c.eq(r as u64, ERROR_ACCESS_DENIED.into())?;
        let signal = OpenEventW(EVENT_MODIFY_STATE, 0, event_name.p());
        c.ok(!signal.is_null(), last_error())?;
        c.ok(SetEvent(signal) != 0, last_error())?;
        CloseHandle(signal);
        c.fails(OpenEventW(EVENT_ALL_ACCESS, 0, event_name.p()).is_null(), ERROR_ACCESS_DENIED)?;
        // Its own event with an empty DACL: the owner's implicit rights.
        let own_name = w("gala-access-test-own");
        let sa = sd(&c, &w("D:P"))?;
        let own = CreateEventW(&sa, 1, 0, own_name.p());
        c.ok(!own.is_null(), last_error())?;
        let h = OpenEventW(READ_CONTROL | WRITE_DAC, 0, own_name.p());
        c.ok(!h.is_null(), last_error())?;
        CloseHandle(h);
        c.fails(OpenEventW(EVENT_MODIFY_STATE, 0, own_name.p()).is_null(), ERROR_ACCESS_DENIED)?;
        c.ok(RevertToSelf() != 0, 0)?;
        c.fails(OpenEventW(READ_CONTROL, 0, own_name.p()).is_null(), ERROR_ACCESS_DENIED)?;
        CloseHandle(own);

        // As the caller: writing works; the file only galatest may use.
        let h = open(&readable, GENERIC_WRITE, 0);
        c.ok(!invalid(h), last_error())?;
        CloseHandle(h);
        let h = open(&private, READ_CONTROL | WRITE_DAC, 0);
        c.ok(!invalid(h), last_error())?;
        CloseHandle(h);
        c.fails(invalid(open(&private, GENERIC_READ, 0)), ERROR_ACCESS_DENIED)?;
        let backup = [1u32, 17, 0, 2]; // SeBackupPrivilege, enabled
        c.ok(AdjustTokenPrivileges(mine, 0, backup.as_ptr(), 0, null.cast(), null.cast()) != 0, 0)?;
        c.eq(last_error(), ERROR_NOT_ALL_ASSIGNED.into())?;
        c.fails(
            invalid(open(&private, GENERIC_READ, FILE_FLAG_BACKUP_SEMANTICS)),
            ERROR_ACCESS_DENIED,
        )?;

        // A restricted token: its user may read r.txt, Everyone may not.
        let everyone: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];
        let restrict = [everyone.as_ptr() as usize, 0];
        let mut restricted = null;
        let r = CreateRestrictedToken(
            mine,
            0,
            0,
            core::ptr::null(),
            0,
            core::ptr::null(),
            1,
            &restrict,
            &mut restricted,
        );
        c.ok(r != 0, last_error())?;
        let mut imp = null;
        c.ok(DuplicateTokenEx(restricted, 0x000f_01ff, null, 2, 2, &mut imp) != 0, last_error())?;
        c.ok(SetThreadToken(null.cast(), imp) != 0, last_error())?;
        c.fails(invalid(open(&readable, GENERIC_READ, 0)), ERROR_ACCESS_DENIED)?;
        let h = open(&deny, GENERIC_READ, 0);
        c.ok(!invalid(h), last_error())?;
        CloseHandle(h);
        c.ok(RevertToSelf() != 0, 0)?;

        // Identification level: no opens.
        c.ok(ImpersonateSelf(1) != 0, last_error())?;
        c.fails(invalid(open(&readable, GENERIC_READ, 0)), ERROR_BAD_IMPERSONATION_LEVEL)?;
        c.ok(RevertToSelf() != 0, 0)?;

        // Clean up: private.txt through the directory's FILE_DELETE_CHILD.
        c.ok(DeleteFileW(private.p()) != 0, last_error())?;
        c.ok(DeleteFileW(deny.p()) != 0, last_error())?;
        c.ok(DeleteFileW(readable.p()) != 0, last_error())?;
        c.ok(RemoveDirectoryW(dir.p()) != 0, last_error())?;
        c.eq(RegDeleteKeyW(HKEY_USERS, key_path.p()) as u64, 0)?;
        CloseHandle(event);
        for h in [test, mine, restricted, imp] {
            CloseHandle(h);
        }
        LocalFree(me_text.cast());
        LocalFree(test_text.cast());
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"access_test: ok\r\n");
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
