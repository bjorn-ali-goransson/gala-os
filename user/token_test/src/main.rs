//! Access tokens as kernel objects (docs/SECURITY.md, phase S1), with the
//! results real Windows gives for a standard (not elevated) user, from
//! the documented contracts of the token functions ("Access Tokens",
//! GetTokenInformation, AdjustTokenPrivileges, DuplicateTokenEx,
//! SetThreadToken, OpenThreadToken, PrivilegeCheck,
//! CreateRestrictedToken):
//!
//! - The process token: a "Token" object (NtQueryObject); every
//!   OpenProcessToken opens the same token (TokenStatistics' TokenId); a
//!   primary token (TokenType); sizes queried (ERROR_INSUFFICIENT_BUFFER
//!   and the size); TokenImpersonationLevel is not a primary token's.
//! - Access: a handle without TOKEN_QUERY cannot query, one without
//!   TOKEN_ADJUST_PRIVILEGES cannot adjust, one without TOKEN_DUPLICATE
//!   cannot duplicate (ERROR_ACCESS_DENIED); a handle that is not a token
//!   is ERROR_INVALID_HANDLE.
//! - AdjustTokenPrivileges changes the token object (seen through another
//!   handle; ModifiedId changes, TokenId does not): enabling the held but
//!   disabled SeShutdownPrivilege (last error 0, PreviousState reports it
//!   disabled), SeDebugPrivilege which a standard user does not hold
//!   (TRUE with ERROR_NOT_ALL_ASSIGNED), restoring PreviousState, and a
//!   PreviousState buffer too small (fails, nothing changes).
//! - DuplicateTokenEx: an impersonation token (TokenType 2, its level,
//!   a new TokenId, the same logon session and user) whose privileges
//!   change independently; a primary duplicate.
//! - Thread tokens: OpenThreadToken without impersonation is
//!   ERROR_NO_TOKEN; SetThreadToken of a primary token is
//!   ERROR_BAD_TOKEN_TYPE; of an impersonation token the thread then has
//!   that token (the same object); RevertToSelf and SetThreadToken(NULL)
//!   end it; ImpersonateSelf impersonates at the level asked; another
//!   thread has no token.
//! - PrivilegeCheck needs an impersonation token
//!   (ERROR_NO_IMPERSONATION_TOKEN); enabled privileges pass and are
//!   marked SE_PRIVILEGE_USED_FOR_ACCESS; all-necessary and any-of sets.
//! - CreateRestrictedToken: DISABLE_MAX_PRIVILEGE keeps only
//!   SeChangeNotifyPrivilege; disabled SIDs become deny-only, deleted
//!   privileges go, restricting SIDs are listed (TokenRestrictedSids) and
//!   make the token restricted (IsTokenRestricted); CheckTokenMembership
//!   of a deny-only group is FALSE.
//!
//! Prints `token_test: ok` or exits with the number of the failing check.

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
    fn GetCurrentThread() -> Handle;
    fn CreateEventW(attrs: *mut c_void, manual: Bool, initial: Bool, name: *const u16) -> Handle;
    fn CreateThread(
        attrs: *mut c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn ResumeThread(h: Handle) -> u32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> Bool;
    fn OpenThreadToken(thread: Handle, access: u32, as_self: Bool, token: *mut Handle) -> Bool;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        out: *mut u8,
        length: u32,
        ret: *mut u32,
    ) -> Bool;
    fn AdjustTokenPrivileges(
        token: Handle,
        disable_all: Bool,
        new: *const u8,
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
    fn SetThreadToken(thread: *mut Handle, token: Handle) -> Bool;
    fn ImpersonateSelf(level: u32) -> Bool;
    fn RevertToSelf() -> Bool;
    fn PrivilegeCheck(token: Handle, set: *mut u8, result: *mut Bool) -> Bool;
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
    fn CheckTokenMembership(token: Handle, sid: *const u8, member: *mut Bool) -> Bool;
    fn EqualSid(a: *const u8, b: *const u8) -> Bool;
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

const TOKEN_ASSIGN_PRIMARY: u32 = 0x0001;
const TOKEN_DUPLICATE: u32 = 0x0002;
const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_ADJUST_PRIVILEGES: u32 = 0x0020;
const TOKEN_ALL_ACCESS: u32 = 0x000f_01ff;

const TOKEN_USER: u32 = 1;
const TOKEN_GROUPS: u32 = 2;
const TOKEN_PRIVILEGES: u32 = 3;
const TOKEN_TYPE: u32 = 8;
const TOKEN_IMPERSONATION_LEVEL: u32 = 9;
const TOKEN_STATISTICS: u32 = 10;
const TOKEN_RESTRICTED_SIDS: u32 = 11;

const SECURITY_IDENTIFICATION: u32 = 1;
const SECURITY_IMPERSONATION: u32 = 2;
const TOKEN_PRIMARY: u32 = 1;
const TOKEN_IMPERSONATION: u32 = 2;

const SE_PRIVILEGE_ENABLED: u32 = 2;
const SE_PRIVILEGE_USED_FOR_ACCESS: u32 = 0x8000_0000;
const SE_GROUP_ENABLED: u32 = 4;
const SE_GROUP_USE_FOR_DENY_ONLY: u32 = 0x10;
const PRIVILEGE_SET_ALL_NECESSARY: u32 = 1;
const DISABLE_MAX_PRIVILEGE: u32 = 1;

const SE_SHUTDOWN: u32 = 19;
const SE_DEBUG: u32 = 20;
const SE_CHANGE_NOTIFY: u32 = 23;

const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_NO_TOKEN: u32 = 1008;
const ERROR_NOT_ALL_ASSIGNED: u32 = 1300;
const ERROR_NO_IMPERSONATION_TOKEN: u32 = 1309;
const ERROR_BAD_TOKEN_TYPE: u32 = 1349;

/// S-1-1-0 (Everyone) and S-1-5-32-544 (BUILTIN\Administrators).
const EVERYONE: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];
const ADMINISTRATORS: [u8; 16] = [1, 2, 0, 0, 0, 0, 0, 5, 32, 0, 0, 0, 0x20, 2, 0, 0];

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
        out(b"token_test: check ");
        out(&hex(self.0.get()));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0.get())
    }

    fn eq(&self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// A call that must fail with `error`.
    fn fails(&self, result: Bool, error: u32) -> Result<(), u32> {
        self.ok(result == 0, 1)?;
        // SAFETY: a plain call.
        self.eq(u64::from(unsafe { GetLastError() }), u64::from(error))
    }
}

fn rd32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn rd64(b: &[u8], at: usize) -> u64 {
    u64::from(rd32(b, at)) | u64::from(rd32(b, at + 4)) << 32
}

/// A token information class into an aligned buffer.
#[repr(C, align(8))]
struct Info([u8; 1024]);

fn query(token: Handle, class: u32, info: &mut Info) -> Bool {
    let mut n = 0;
    // SAFETY: our buffer of its size.
    unsafe { GetTokenInformation(token, class, info.0.as_mut_ptr(), 1024, &mut n) }
}

fn dword(c: &Checks, token: Handle, class: u32) -> Result<u32, u32> {
    let mut info = Info([0; 1024]);
    c.ok(query(token, class, &mut info) != 0, class.into())?;
    Ok(rd32(&info.0, 0))
}

/// TokenStatistics: (TokenId, AuthenticationId, TokenType,
/// ImpersonationLevel, ModifiedId).
fn statistics(c: &Checks, token: Handle) -> Result<(u64, u64, u32, u32, u64), u32> {
    let mut info = Info([0; 1024]);
    c.ok(query(token, TOKEN_STATISTICS, &mut info) != 0, 0)?;
    let b = &info.0;
    Ok((rd64(b, 0), rd64(b, 8), rd32(b, 24), rd32(b, 28), rd64(b, 48)))
}

/// The attributes of privilege `luid` in the token (None: not held).
fn privilege(c: &Checks, token: Handle, luid: u32) -> Result<Option<u32>, u32> {
    let mut info = Info([0; 1024]);
    c.ok(query(token, TOKEN_PRIVILEGES, &mut info) != 0, 0)?;
    let b = &info.0;
    Ok((0..rd32(b, 0) as usize)
        .map(|i| 4 + 12 * i)
        .find(|&e| rd32(b, e) == luid && rd32(b, e + 4) == 0)
        .map(|e| rd32(b, e + 8)))
}

fn privilege_count(c: &Checks, token: Handle) -> Result<u32, u32> {
    dword(c, token, TOKEN_PRIVILEGES)
}

/// TOKEN_PRIVILEGES with one privilege.
fn one_privilege(luid: u32, attrs: u32) -> [u32; 4] {
    [1, luid, 0, attrs]
}

fn adjust(token: Handle, new: &[u32; 4], previous: Option<&mut [u32; 8]>) -> Bool {
    let mut ret = 0;
    let (p, len) = match previous {
        Some(p) => (p.as_mut_ptr().cast::<u8>(), 32),
        None => (core::ptr::null_mut(), 0),
    };
    // SAFETY: our TOKEN_PRIVILEGES buffers.
    unsafe { AdjustTokenPrivileges(token, 0, new.as_ptr().cast(), len, p, &mut ret) }
}

fn last_error() -> u64 {
    // SAFETY: a plain call.
    u64::from(unsafe { GetLastError() })
}

/// The attributes of group `sid` in TokenGroups (None: not there).
fn group(c: &Checks, token: Handle, class: u32, sid: &[u8]) -> Result<Option<u32>, u32> {
    let mut info = Info([0; 1024]);
    c.ok(query(token, class, &mut info) != 0, class.into())?;
    let b = &info.0;
    for i in 0..rd32(b, 0) as usize {
        let e = 8 + 16 * i;
        let p = rd64(b, e) as *const u8;
        // SAFETY: SIDs inside `info`, as GetTokenInformation laid them out.
        if unsafe { EqualSid(p, sid.as_ptr()) } != 0 {
            return Ok(Some(rd32(b, e + 8)));
        }
    }
    Ok(None)
}

extern "system" fn idle(_: *mut c_void) -> u32 {
    0
}

/// PrivilegeCheck of `luids` with `control`: (call result, result,
/// attributes of the first privilege after the call).
fn check_privileges(token: Handle, control: u32, luids: &[u32]) -> (Bool, Bool, u32) {
    let mut set = [0u32; 2 + 3 * 4];
    set[0] = luids.len() as u32;
    set[1] = control;
    for (i, &l) in luids.iter().enumerate() {
        set[2 + 3 * i] = l;
    }
    let mut result = 0;
    // SAFETY: our PRIVILEGE_SET.
    let r = unsafe { PrivilegeCheck(token, set.as_mut_ptr().cast(), &mut result) };
    (r, result, set[4])
}

fn run() -> Result<(), u32> {
    let c = Checks(core::cell::Cell::new(0));
    let null = core::ptr::null_mut();
    // SAFETY: plain calls with our handles and outputs throughout.
    unsafe {
        let me = GetCurrentProcess();
        let mut query_only = null;
        c.ok(OpenProcessToken(me, TOKEN_QUERY, &mut query_only) != 0, last_error())?;

        // A Token object (OBJECT_TYPE_INFORMATION starts with its name).
        let mut info = Info([0; 1024]);
        c.eq(u64::from(NtQueryObject(query_only, 2, info.0.as_mut_ptr(), 1024, null.cast())), 0)?;
        let length = usize::from(u16::from_le_bytes([info.0[0], info.0[1]]));
        let name = rd64(&info.0, 8) as *const u16;
        c.eq(length as u64, 10)?;
        let expect = [b'T', b'o', b'k', b'e', b'n'];
        c.ok((0..5).all(|i| *name.add(i) == u16::from(expect[i])), 0)?;

        // A primary token; the same object through every open.
        c.eq(dword(&c, query_only, TOKEN_TYPE)?.into(), TOKEN_PRIMARY.into())?;
        let (id, logon, kind, _, modified) = statistics(&c, query_only)?;
        c.ok(id != 0 && logon != 0, id)?;
        c.eq(kind.into(), TOKEN_PRIMARY.into())?;
        let mut adjuster = null;
        c.ok(OpenProcessToken(me, TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES, &mut adjuster) != 0, 0)?;
        c.eq(statistics(&c, adjuster)?.0, id)?;

        // Sizes: a null buffer reports the size.
        let mut needed = 0;
        c.fails(
            GetTokenInformation(query_only, TOKEN_USER, null.cast(), 0, &mut needed),
            ERROR_INSUFFICIENT_BUFFER,
        )?;
        c.ok(needed > 16, needed.into())?;
        c.ok(query(query_only, TOKEN_IMPERSONATION_LEVEL, &mut info) == 0, 0)?;

        // Access: the handle's granted access decides.
        let shutdown_on = one_privilege(SE_SHUTDOWN, SE_PRIVILEGE_ENABLED);
        c.fails(adjust(query_only, &shutdown_on, None), ERROR_ACCESS_DENIED)?;
        let mut adjust_only = null;
        c.ok(OpenProcessToken(me, TOKEN_ADJUST_PRIVILEGES, &mut adjust_only) != 0, 0)?;
        c.fails(query(adjust_only, TOKEN_TYPE, &mut info), ERROR_ACCESS_DENIED)?;
        let event = CreateEventW(null, 1, 0, core::ptr::null());
        c.fails(query(event, TOKEN_TYPE, &mut info), ERROR_INVALID_HANDLE)?;

        // Privileges: SeShutdown held, disabled; SeChangeNotify enabled.
        c.eq(privilege(&c, query_only, SE_SHUTDOWN)?.map_or(9, u64::from), 0)?;
        let notify = privilege(&c, query_only, SE_CHANGE_NOTIFY)?.unwrap_or(0);
        c.ok(notify & SE_PRIVILEGE_ENABLED != 0, notify.into())?;
        c.eq(privilege(&c, query_only, SE_DEBUG)?.map_or(9, u64::from), 9)?;

        // Enabling SeShutdown: success, last error 0, the previous state.
        let mut previous = [0xffff_ffffu32; 8];
        SetLastError(0xdead);
        c.ok(adjust(adjuster, &shutdown_on, Some(&mut previous)) != 0, last_error())?;
        c.eq(last_error(), 0)?;
        c.eq(previous[0].into(), 1)?;
        c.eq(previous[1].into(), SE_SHUTDOWN.into())?;
        c.eq(previous[3].into(), 0)?;
        // Seen through the other handle: the token object changed.
        let now = privilege(&c, query_only, SE_SHUTDOWN)?.unwrap_or(0);
        c.ok(now & SE_PRIVILEGE_ENABLED != 0, now.into())?;
        let (id2, _, _, _, modified2) = statistics(&c, query_only)?;
        c.eq(id2, id)?;
        c.ok(modified2 != modified, modified2)?;

        // A privilege not held: TRUE with ERROR_NOT_ALL_ASSIGNED.
        c.ok(adjust(adjuster, &one_privilege(SE_DEBUG, SE_PRIVILEGE_ENABLED), None) != 0, 0)?;
        c.eq(last_error(), ERROR_NOT_ALL_ASSIGNED.into())?;
        c.eq(privilege(&c, query_only, SE_DEBUG)?.map_or(9, u64::from), 9)?;

        // Restoring the previous state disables SeShutdown again.
        let restore = [previous[0], previous[1], previous[2], previous[3]];
        c.ok(adjust(adjuster, &restore, None) != 0, last_error())?;
        c.eq(privilege(&c, query_only, SE_SHUTDOWN)?.map_or(9, u64::from), 0)?;

        // A previous-state buffer too small: fails, nothing changes.
        let mut ret = 0;
        let mut small = [0u32; 1];
        c.fails(
            AdjustTokenPrivileges(
                adjuster,
                0,
                shutdown_on.as_ptr().cast(),
                4,
                small.as_mut_ptr().cast(),
                &mut ret,
            ),
            ERROR_INSUFFICIENT_BUFFER,
        )?;
        c.eq(privilege(&c, query_only, SE_SHUTDOWN)?.map_or(9, u64::from), 0)?;

        // DuplicateTokenEx: an impersonation token of its own.
        let mut source = null;
        c.ok(OpenProcessToken(me, TOKEN_QUERY | TOKEN_DUPLICATE, &mut source) != 0, 0)?;
        let mut imp = null;
        let r = DuplicateTokenEx(
            source,
            TOKEN_ALL_ACCESS,
            null,
            SECURITY_IMPERSONATION,
            TOKEN_IMPERSONATION,
            &mut imp,
        );
        c.ok(r != 0, last_error())?;
        c.eq(dword(&c, imp, TOKEN_TYPE)?.into(), TOKEN_IMPERSONATION.into())?;
        c.eq(dword(&c, imp, TOKEN_IMPERSONATION_LEVEL)?.into(), SECURITY_IMPERSONATION.into())?;
        let (imp_id, imp_logon, imp_kind, imp_level, _) = statistics(&c, imp)?;
        c.ok(imp_id != id, imp_id)?;
        c.eq(imp_logon, logon)?;
        c.eq(imp_kind.into(), TOKEN_IMPERSONATION.into())?;
        c.eq(imp_level.into(), SECURITY_IMPERSONATION.into())?;
        let mut user = Info([0; 1024]);
        c.ok(query(query_only, TOKEN_USER, &mut user) != 0, 0)?;
        c.ok(query(imp, TOKEN_USER, &mut info) != 0, 0)?;
        let (a, b) = (rd64(&user.0, 0) as *const u8, rd64(&info.0, 0) as *const u8);
        c.ok(EqualSid(a, b) != 0, 0)?;
        // Its privileges are its own.
        c.ok(adjust(imp, &shutdown_on, None) != 0, last_error())?;
        c.eq(last_error(), 0)?;
        let on = privilege(&c, imp, SE_SHUTDOWN)?.unwrap_or(0);
        c.ok(on & SE_PRIVILEGE_ENABLED != 0, on.into())?;
        c.eq(privilege(&c, query_only, SE_SHUTDOWN)?.map_or(9, u64::from), 0)?;
        // A primary duplicate.
        let mut primary = null;
        let r = DuplicateTokenEx(source, TOKEN_ALL_ACCESS, null, 0, TOKEN_PRIMARY, &mut primary);
        c.ok(r != 0, last_error())?;
        c.eq(dword(&c, primary, TOKEN_TYPE)?.into(), TOKEN_PRIMARY.into())?;
        c.ok(statistics(&c, primary)?.0 != id, 0)?;
        // Without TOKEN_DUPLICATE.
        let mut nothing = null;
        c.fails(
            DuplicateTokenEx(query_only, TOKEN_ALL_ACCESS, null, 2, 2, &mut nothing),
            ERROR_ACCESS_DENIED,
        )?;

        // Thread tokens.
        let thread = GetCurrentThread();
        let mut t = null;
        c.fails(OpenThreadToken(thread, TOKEN_QUERY, 1, &mut t), ERROR_NO_TOKEN)?;
        c.fails(SetThreadToken(null.cast(), primary), ERROR_BAD_TOKEN_TYPE)?;
        c.fails(OpenThreadToken(thread, TOKEN_QUERY, 1, &mut t), ERROR_NO_TOKEN)?;
        c.ok(SetThreadToken(null.cast(), imp) != 0, last_error())?;
        c.ok(OpenThreadToken(thread, TOKEN_QUERY, 1, &mut t) != 0, last_error())?;
        c.eq(statistics(&c, t)?.0, imp_id)?;
        let on = privilege(&c, t, SE_SHUTDOWN)?.unwrap_or(0);
        c.ok(on & SE_PRIVILEGE_ENABLED != 0, on.into())?;
        CloseHandle(t);
        // Another thread has no token.
        let other = CreateThread(null, 0, idle, null, 4, null.cast()); // CREATE_SUSPENDED
        c.ok(!other.is_null(), 0)?;
        c.fails(OpenThreadToken(other, TOKEN_QUERY, 1, &mut t), ERROR_NO_TOKEN)?;
        ResumeThread(other);
        c.eq(u64::from(WaitForSingleObject(other, 10_000)), 0)?;
        CloseHandle(other);
        c.ok(RevertToSelf() != 0, 0)?;
        c.fails(OpenThreadToken(thread, TOKEN_QUERY, 1, &mut t), ERROR_NO_TOKEN)?;
        // SetThreadToken(NULL) reverts too.
        c.ok(SetThreadToken(null.cast(), imp) != 0, 0)?;
        c.ok(SetThreadToken(null.cast(), null) != 0, 0)?;
        c.fails(OpenThreadToken(thread, TOKEN_QUERY, 1, &mut t), ERROR_NO_TOKEN)?;
        // ImpersonateSelf: a copy of the process token at that level.
        c.ok(ImpersonateSelf(SECURITY_IDENTIFICATION) != 0, last_error())?;
        c.ok(OpenThreadToken(thread, TOKEN_QUERY, 1, &mut t) != 0, last_error())?;
        let (self_id, self_logon, self_kind, self_level, _) = statistics(&c, t)?;
        c.ok(self_id != id, self_id)?;
        c.eq(self_logon, logon)?;
        c.eq(self_kind.into(), TOKEN_IMPERSONATION.into())?;
        c.eq(self_level.into(), SECURITY_IDENTIFICATION.into())?;
        CloseHandle(t);
        c.ok(RevertToSelf() != 0, 0)?;
        c.fails(OpenThreadToken(thread, TOKEN_QUERY, 1, &mut t), ERROR_NO_TOKEN)?;

        // PrivilegeCheck.
        let (r, _, _) = check_privileges(query_only, 0, &[SE_CHANGE_NOTIFY]);
        c.fails(r, ERROR_NO_IMPERSONATION_TOKEN)?;
        let (r, held, attrs) = check_privileges(imp, PRIVILEGE_SET_ALL_NECESSARY, &[SE_SHUTDOWN]);
        c.ok(r != 0, last_error())?;
        c.eq(held as u64, 1)?;
        c.ok(attrs & SE_PRIVILEGE_USED_FOR_ACCESS != 0, attrs.into())?;
        let (r, held, attrs) = check_privileges(imp, PRIVILEGE_SET_ALL_NECESSARY, &[SE_DEBUG]);
        c.ok(r != 0, 0)?;
        c.eq(held as u64, 0)?;
        c.eq(attrs.into(), 0)?;
        let all = PRIVILEGE_SET_ALL_NECESSARY;
        c.eq(check_privileges(imp, all, &[SE_CHANGE_NOTIFY, SE_SHUTDOWN]).1 as u64, 1)?;
        c.eq(check_privileges(imp, all, &[SE_DEBUG, SE_CHANGE_NOTIFY]).1 as u64, 0)?;
        c.eq(check_privileges(imp, 0, &[SE_DEBUG, SE_CHANGE_NOTIFY]).1 as u64, 1)?;

        // CreateRestrictedToken.
        let mut full = null;
        let access = TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY;
        c.ok(OpenProcessToken(me, access, &mut full) != 0, 0)?;
        let mut restricted = null;
        let r = CreateRestrictedToken(
            full,
            DISABLE_MAX_PRIVILEGE,
            0,
            core::ptr::null(),
            0,
            core::ptr::null(),
            0,
            core::ptr::null(),
            &mut restricted,
        );
        c.ok(r != 0, last_error())?;
        c.eq(dword(&c, restricted, TOKEN_TYPE)?.into(), TOKEN_PRIMARY.into())?;
        c.eq(privilege_count(&c, restricted)?.into(), 1)?;
        c.ok(privilege(&c, restricted, SE_CHANGE_NOTIFY)?.is_some(), 0)?;
        c.eq(IsTokenRestricted(restricted) as u64, 0)?;
        c.ok(statistics(&c, restricted)?.0 != id, 0)?;
        // Everyone deny-only, SeShutdown deleted, Everyone restricting.
        let everyone = [EVERYONE.as_ptr() as usize, 0];
        let delete = [SE_SHUTDOWN, 0, 0];
        let mut filtered = null;
        let r =
            CreateRestrictedToken(full, 0, 1, &everyone, 1, &delete, 1, &everyone, &mut filtered);
        c.ok(r != 0, last_error())?;
        let attrs = group(&c, filtered, TOKEN_GROUPS, &EVERYONE)?.unwrap_or(0);
        c.ok(attrs & SE_GROUP_USE_FOR_DENY_ONLY != 0, attrs.into())?;
        c.eq((attrs & SE_GROUP_ENABLED).into(), 0)?;
        c.eq(privilege(&c, filtered, SE_SHUTDOWN)?.map_or(9, u64::from), 9)?;
        c.ok(privilege(&c, filtered, SE_CHANGE_NOTIFY)?.is_some(), 0)?;
        c.eq(dword(&c, filtered, TOKEN_RESTRICTED_SIDS)?.into(), 1)?;
        c.ok(group(&c, filtered, TOKEN_RESTRICTED_SIDS, &EVERYONE)?.is_some(), 0)?;
        c.eq(IsTokenRestricted(filtered) as u64, 1)?;
        // The original is unchanged.
        let attrs = group(&c, query_only, TOKEN_GROUPS, &EVERYONE)?.unwrap_or(0);
        c.ok(attrs & SE_GROUP_ENABLED != 0, attrs.into())?;
        c.eq(dword(&c, query_only, TOKEN_RESTRICTED_SIDS)?.into(), 0)?;
        // Without TOKEN_DUPLICATE.
        c.fails(
            CreateRestrictedToken(
                query_only,
                DISABLE_MAX_PRIVILEGE,
                0,
                core::ptr::null(),
                0,
                core::ptr::null(),
                0,
                core::ptr::null(),
                &mut nothing,
            ),
            ERROR_ACCESS_DENIED,
        )?;

        // CheckTokenMembership (an impersonation token): Everyone yes,
        // Administrators no (a standard user), a deny-only group no.
        let mut member = 7;
        c.ok(CheckTokenMembership(imp, EVERYONE.as_ptr(), &mut member) != 0, last_error())?;
        c.eq(member as u64, 1)?;
        c.ok(CheckTokenMembership(imp, ADMINISTRATORS.as_ptr(), &mut member) != 0, 0)?;
        c.eq(member as u64, 0)?;
        let mut filtered_imp = null;
        let r = DuplicateTokenEx(filtered, TOKEN_ALL_ACCESS, null, 2, 2, &mut filtered_imp);
        c.ok(r != 0, last_error())?;
        c.ok(CheckTokenMembership(filtered_imp, EVERYONE.as_ptr(), &mut member) != 0, 0)?;
        c.eq(member as u64, 0)?;

        for h in [
            query_only,
            adjuster,
            adjust_only,
            event,
            source,
            imp,
            primary,
            full,
            restricted,
            filtered,
            filtered_imp,
        ] {
            c.ok(CloseHandle(h) != 0, 0)?;
        }
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"token_test: ok\r\n");
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
