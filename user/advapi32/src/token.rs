//! Access tokens: the documented token functions as thin wrappers over the
//! kernel's token services (docs/SECURITY.md, phase S1), as advapi32 is on
//! Windows: NtOpenProcessToken, NtOpenThreadToken,
//! NtQueryInformationToken, NtAdjustPrivilegesToken, NtDuplicateToken,
//! NtFilterToken, NtPrivilegeCheck and NtSetInformationThread
//! (ThreadImpersonationToken).

use core::ffi::c_void;

use super::fail;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtOpenProcessToken(process: usize, access: u32, token: *mut usize) -> u32;
    fn NtOpenThreadToken(thread: usize, access: u32, as_self: u8, token: *mut usize) -> u32;
    fn NtQueryInformationToken(
        token: usize,
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> u32;
    fn NtAdjustPrivilegesToken(
        token: usize,
        disable_all: u8,
        new: *const u8,
        length: u32,
        previous: *mut u8,
        ret: *mut u32,
    ) -> u32;
    fn NtDuplicateToken(
        token: usize,
        access: u32,
        attributes: *const ObjectAttributes,
        effective_only: u8,
        kind: u32,
        new: *mut usize,
    ) -> u32;
    fn NtFilterToken(
        token: usize,
        flags: u32,
        disable: *const u8,
        delete: *const u8,
        restrict: *const u8,
        new: *mut usize,
    ) -> u32;
    fn NtPrivilegeCheck(token: usize, set: *mut u8, result: *mut u8) -> u32;
    fn NtSetInformationThread(thread: usize, class: u32, buf: *const u8, len: u32) -> u32;
    fn NtClose(h: usize) -> u32;
    fn RtlNtStatusToDosError(status: u32) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LocalAlloc(flags: u32, bytes: usize) -> *mut u8;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
    fn SetLastError(code: u32);
}

const CURRENT_PROCESS: usize = usize::MAX; // -1
const CURRENT_THREAD: usize = usize::MAX - 1; // -2

const TOKEN_DUPLICATE: u32 = 0x0002;
const TOKEN_IMPERSONATE: u32 = 0x0004;
const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_IMPERSONATION: u32 = 2;
const THREAD_IMPERSONATION_TOKEN: u32 = 5;

const TOKEN_USER: u32 = 1;
const TOKEN_GROUPS: u32 = 2;
const TOKEN_TYPE: u32 = 8;
const TOKEN_RESTRICTED_SIDS: u32 = 11;

const STATUS_NOT_ALL_ASSIGNED: u32 = 0x0000_0106;
const STATUS_BUFFER_TOO_SMALL: u32 = 0xc000_0023;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_NO_IMPERSONATION_TOKEN: u32 = 1309;

const SE_GROUP_ENABLED: u32 = 0x4;
const SE_GROUP_USE_FOR_DENY_ONLY: u32 = 0x10;

/// OBJECT_ATTRIBUTES (x64).
#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: usize,
    name: *const c_void,
    attributes: u32,
    descriptor: *const c_void,
    quality: *const Quality,
}

/// SECURITY_QUALITY_OF_SERVICE.
#[repr(C)]
struct Quality {
    length: u32,
    level: u32,
    tracking: u8,
    effective_only: u8,
}

/// TRUE, or FALSE with the status's Win32 error.
fn result(status: u32) -> i32 {
    if status == 0 {
        1
    } else {
        // SAFETY: a plain call.
        fail(unsafe { RtlNtStatusToDosError(status) }, 0)
    }
}

/// OpenProcessToken(ProcessHandle, DesiredAccess, *TokenHandle)
#[unsafe(no_mangle)]
pub extern "system" fn OpenProcessToken(process: usize, access: u32, token: *mut usize) -> i32 {
    if token.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's output.
    result(unsafe { NtOpenProcessToken(process, access, token) })
}

/// OpenThreadToken(ThreadHandle, DesiredAccess, OpenAsSelf, *TokenHandle):
/// ERROR_NO_TOKEN when the thread does not impersonate.
#[unsafe(no_mangle)]
pub extern "system" fn OpenThreadToken(
    thread: usize,
    access: u32,
    as_self: i32,
    token: *mut usize,
) -> i32 {
    if token.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's output.
    result(unsafe { NtOpenThreadToken(thread, access, u8::from(as_self != 0), token) })
}

/// GetTokenInformation: ERROR_INSUFFICIENT_BUFFER with the size when the
/// buffer is short.
#[unsafe(no_mangle)]
pub extern "system" fn GetTokenInformation(
    token: usize,
    class: u32,
    out: *mut u8,
    length: u32,
    ret: *mut u32,
) -> i32 {
    // SAFETY: the caller's buffer of `length` and output.
    result(unsafe { NtQueryInformationToken(token, class, out, length, ret) })
}

/// AdjustTokenPrivileges: TRUE with last error ERROR_NOT_ALL_ASSIGNED when
/// some privilege is not held, TRUE with 0 when all were adjusted.
#[unsafe(no_mangle)]
pub extern "system" fn AdjustTokenPrivileges(
    token: usize,
    disable_all: i32,
    new: *const u8,
    length: u32,
    previous: *mut u8,
    ret: *mut u32,
) -> i32 {
    // SAFETY: the caller's TOKEN_PRIVILEGES buffers.
    let status = unsafe {
        NtAdjustPrivilegesToken(token, u8::from(disable_all != 0), new, length, previous, ret)
    };
    match status {
        0 | STATUS_NOT_ALL_ASSIGNED => {
            // SAFETY: a plain call.
            unsafe { SetLastError(if status == 0 { 0 } else { 1300 }) };
            1
        }
        _ => result(status),
    }
}

/// DuplicateTokenEx(ExistingToken, DesiredAccess, TokenAttributes,
/// ImpersonationLevel, TokenType, *NewToken): the level goes in the
/// attributes' security quality of service, the inheritance from
/// SECURITY_ATTRIBUTES. TODO(S2): its security descriptor.
#[unsafe(no_mangle)]
pub extern "system" fn DuplicateTokenEx(
    token: usize,
    access: u32,
    attributes: *mut c_void,
    level: u32,
    kind: u32,
    new: *mut usize,
) -> i32 {
    if new.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let quality = Quality { length: 12, level, tracking: 0, effective_only: 0 };
    // SECURITY_ATTRIBUTES: nLength, lpSecurityDescriptor, bInheritHandle.
    let inherit = !attributes.is_null()
        // SAFETY: the caller's SECURITY_ATTRIBUTES.
        && unsafe { attributes.cast::<u8>().add(16).cast::<i32>().read_unaligned() } != 0;
    let oa = ObjectAttributes {
        length: 48,
        root: 0,
        name: core::ptr::null(),
        attributes: if inherit { 2 } else { 0 }, // OBJ_INHERIT
        descriptor: core::ptr::null(),
        quality: &quality,
    };
    // SAFETY: our attributes; the caller's output.
    result(unsafe { NtDuplicateToken(token, access, &oa, 0, kind, new) })
}

/// DuplicateToken: an impersonation token at `level` (TOKEN_IMPERSONATE |
/// TOKEN_QUERY), as documented.
#[unsafe(no_mangle)]
pub extern "system" fn DuplicateToken(token: usize, level: u32, new: *mut usize) -> i32 {
    DuplicateTokenEx(
        token,
        TOKEN_IMPERSONATE | TOKEN_QUERY,
        core::ptr::null_mut(),
        level,
        TOKEN_IMPERSONATION,
        new,
    )
}

fn set_thread_token(thread: usize, token: usize) -> i32 {
    // SAFETY: our handle value, 8 bytes.
    result(unsafe {
        NtSetInformationThread(thread, THREAD_IMPERSONATION_TOKEN, (&raw const token).cast(), 8)
    })
}

/// SetThreadToken(*Thread, Token): the thread (null: the caller)
/// impersonates with `token` (an impersonation token: else
/// ERROR_BAD_TOKEN_TYPE), or reverts with a null token.
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadToken(thread: *mut usize, token: usize) -> i32 {
    // SAFETY: the caller's thread handle.
    let thread = if thread.is_null() { CURRENT_THREAD } else { unsafe { *thread } };
    set_thread_token(thread, token)
}

/// RevertToSelf: the calling thread stops impersonating.
#[unsafe(no_mangle)]
pub extern "system" fn RevertToSelf() -> i32 {
    set_thread_token(CURRENT_THREAD, 0)
}

/// ImpersonateSelf: the calling thread impersonates a copy of its process
/// token at `level`.
#[unsafe(no_mangle)]
pub extern "system" fn ImpersonateSelf(level: u32) -> i32 {
    let mut process = 0;
    if OpenProcessToken(CURRENT_PROCESS, TOKEN_DUPLICATE, &mut process) == 0 {
        return 0;
    }
    let mut copy = 0;
    let access = TOKEN_IMPERSONATE | TOKEN_QUERY;
    let r = DuplicateTokenEx(process, access, core::ptr::null_mut(), level, 2, &mut copy);
    // SAFETY: our handles.
    unsafe { NtClose(process) };
    if r == 0 {
        return 0;
    }
    let r = set_thread_token(CURRENT_THREAD, copy);
    // SAFETY: our handle (the thread keeps its own reference).
    unsafe { NtClose(copy) };
    r
}

/// PrivilegeCheck(ClientToken, RequiredPrivileges, *pfResult): needs an
/// impersonation token (ERROR_NO_IMPERSONATION_TOKEN).
#[unsafe(no_mangle)]
pub extern "system" fn PrivilegeCheck(token: usize, set: *mut u8, out: *mut i32) -> i32 {
    if set.is_null() || out.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let mut held = 0u8;
    // SAFETY: the caller's PRIVILEGE_SET; our result.
    let r = result(unsafe { NtPrivilegeCheck(token, set, &mut held) });
    if r != 0 {
        // SAFETY: the caller's output.
        unsafe { *out = i32::from(held) };
    }
    r
}

/// A LocalAlloc'd copy of `count` entries of `size` bytes at `entries`,
/// after a count (TOKEN_GROUPS: padded to 8; TOKEN_PRIVILEGES: 4); null
/// for none.
fn counted(count: u32, entries: *const u8, size: usize, head: usize) -> Result<*mut u8, ()> {
    if count == 0 || entries.is_null() {
        return Ok(core::ptr::null_mut());
    }
    let bytes = head + size * count as usize;
    // SAFETY: plain call; LMEM_ZEROINIT (0x40).
    let p = unsafe { LocalAlloc(0x40, bytes) };
    if p.is_null() {
        return Err(());
    }
    // SAFETY: our buffer of `bytes`; the caller's array of `count` entries.
    unsafe {
        p.cast::<u32>().write(count);
        core::ptr::copy_nonoverlapping(entries, p.add(head), size * count as usize);
    }
    Ok(p)
}

/// CreateRestrictedToken(ExistingToken, Flags, DisableSidCount,
/// SidsToDisable, DeletePrivilegeCount, PrivilegesToDelete,
/// RestrictedSidCount, SidsToRestrict, *NewToken): NtFilterToken with
/// those lists.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn CreateRestrictedToken(
    token: usize,
    flags: u32,
    disable_count: u32,
    disable: *const u8,
    delete_count: u32,
    delete: *const u8,
    restrict_count: u32,
    restrict: *const u8,
    new: *mut usize,
) -> i32 {
    if new.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let lists = (
        counted(disable_count, disable, 16, 8),
        counted(delete_count, delete, 12, 4),
        counted(restrict_count, restrict, 16, 8),
    );
    let free = |p: Result<*mut u8, ()>| {
        if let Ok(p) = p {
            // SAFETY: from LocalAlloc (or null).
            unsafe { LocalFree(p.cast()) };
        }
    };
    let r = match lists {
        (Ok(a), Ok(b), Ok(c)) => {
            // SAFETY: our lists; the caller's output.
            result(unsafe { NtFilterToken(token, flags, a, b, c, new) })
        }
        _ => fail(ERROR_NOT_ENOUGH_MEMORY, 0),
    };
    free(lists.0);
    free(lists.1);
    free(lists.2);
    r
}

/// A token information class in LocalAlloc'd memory (free with LocalFree).
fn query_alloc(token: usize, class: u32) -> Result<*mut u8, u32> {
    let mut size = 0;
    // SAFETY: a size query.
    let status =
        unsafe { NtQueryInformationToken(token, class, core::ptr::null_mut(), 0, &mut size) };
    if status != STATUS_BUFFER_TOO_SMALL && status != 0 {
        return Err(status);
    }
    // SAFETY: plain call.
    let p = unsafe { LocalAlloc(0x40, size.max(8) as usize) };
    if p.is_null() {
        return Err(0xc000_0017);
    }
    // SAFETY: our buffer of `size`.
    let status = unsafe { NtQueryInformationToken(token, class, p, size.max(8), &mut size) };
    if status != 0 {
        // SAFETY: ours.
        unsafe { LocalFree(p.cast()) };
        return Err(status);
    }
    Ok(p)
}

/// IsTokenRestricted: whether the token has restricting SIDs.
#[unsafe(no_mangle)]
pub extern "system" fn IsTokenRestricted(token: usize) -> i32 {
    let mut groups = [0u64; 1];
    let mut size = 0;
    // SAFETY: our 8-byte buffer: room for an empty TOKEN_GROUPS only.
    let status = unsafe {
        NtQueryInformationToken(
            token,
            TOKEN_RESTRICTED_SIDS,
            groups.as_mut_ptr().cast(),
            8,
            &mut size,
        )
    };
    match status {
        0 => i32::from(groups[0] as u32 != 0),
        STATUS_BUFFER_TOO_SMALL => 1,
        _ => result(status),
    }
}

/// Whether `sid` is in the TOKEN_GROUPS at `p`, with attributes passing
/// `ok`.
fn in_groups(p: *const u8, sid: *const u8, ok: impl Fn(u32) -> bool) -> bool {
    // SAFETY: a TOKEN_GROUPS from the kernel: count, then SID_AND_ATTRIBUTES.
    unsafe {
        let count = p.cast::<u32>().read() as usize;
        (0..count).any(|i| {
            let e = p.add(8 + 16 * i);
            let s = e.cast::<*const u8>().read();
            ok(e.add(8).cast::<u32>().read()) && super::EqualSid(s, sid) != 0
        })
    }
}

/// CheckTokenMembership(TokenHandle, SidToCheck, *IsMember): whether the
/// SID is the user or an enabled group (not deny-only) of an impersonation
/// token, and among the restricting SIDs of a restricted one. A null
/// token is the caller's: its thread token or else a copy of its process
/// token.
#[unsafe(no_mangle)]
pub extern "system" fn CheckTokenMembership(token: usize, sid: *const u8, member: *mut i32) -> i32 {
    if sid.is_null() || member.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let mut own = 0;
    let token = if token != 0 {
        let mut kind = 0u32;
        let mut n = 0;
        // SAFETY: our 4-byte output.
        let status = unsafe {
            NtQueryInformationToken(token, TOKEN_TYPE, (&raw mut kind).cast(), 4, &mut n)
        };
        if status != 0 {
            return result(status);
        }
        if kind != TOKEN_IMPERSONATION {
            return fail(ERROR_NO_IMPERSONATION_TOKEN, 0);
        }
        token
    } else {
        // SAFETY: our output.
        if unsafe { NtOpenThreadToken(CURRENT_THREAD, TOKEN_QUERY, 1, &mut own) } != 0 {
            let mut process = 0;
            if OpenProcessToken(CURRENT_PROCESS, TOKEN_DUPLICATE, &mut process) == 0 {
                return 0;
            }
            let r = DuplicateToken(process, 1, &mut own);
            // SAFETY: ours.
            unsafe { NtClose(process) };
            if r == 0 {
                return 0;
            }
        }
        own
    };
    let parts = (
        query_alloc(token, TOKEN_USER),
        query_alloc(token, TOKEN_GROUPS),
        query_alloc(token, TOKEN_RESTRICTED_SIDS),
    );
    let r = match parts {
        (Ok(user), Ok(groups), Ok(restricted)) => {
            let usable = |a: u32| a & SE_GROUP_USE_FOR_DENY_ONLY == 0;
            let enabled = |a: u32| a & SE_GROUP_ENABLED != 0 && a & SE_GROUP_USE_FOR_DENY_ONLY == 0;
            // SAFETY: TOKEN_USER: a SID_AND_ATTRIBUTES.
            let (user_sid, user_attrs) =
                unsafe { (user.cast::<*const u8>().read(), user.add(8).cast::<u32>().read()) };
            let mut yes = (usable(user_attrs) && super::EqualSid(user_sid, sid) != 0)
                || in_groups(groups, sid, enabled);
            // SAFETY: TOKEN_GROUPS' count.
            if yes && unsafe { restricted.cast::<u32>().read() } != 0 {
                yes = in_groups(restricted, sid, |_| true);
            }
            // SAFETY: the caller's output.
            unsafe { *member = i32::from(yes) };
            1
        }
        _ => {
            let status =
                [&parts.0, &parts.1, &parts.2].iter().find_map(|p| p.err()).unwrap_or(0xc000_0017);
            result(status)
        }
    };
    for p in [parts.0, parts.1, parts.2].into_iter().flatten() {
        // SAFETY: from query_alloc.
        unsafe { LocalFree(p.cast()) };
    }
    if own != 0 {
        // SAFETY: ours.
        unsafe { NtClose(own) };
    }
    r
}

/// SaferComputeTokenFromLevel's copy of a token (the caller's process
/// token when null): a primary duplicate.
pub fn primary_copy(token: usize, out: *mut usize) -> i32 {
    let mut process = 0;
    let source = if token != 0 {
        token
    } else {
        if OpenProcessToken(CURRENT_PROCESS, TOKEN_DUPLICATE, &mut process) == 0 {
            return 0;
        }
        process
    };
    let r = DuplicateTokenEx(source, 0x000f_01ff, core::ptr::null_mut(), 0, 1, out);
    if process != 0 {
        // SAFETY: ours.
        unsafe { NtClose(process) };
    }
    r
}

/// ImpersonateNamedPipeClient: FSCTL_PIPE_IMPERSONATE (the file system
/// makes the calling thread impersonate the pipe's client).
#[unsafe(no_mangle)]
pub extern "system" fn ImpersonateNamedPipeClient(pipe: usize) -> i32 {
    #[link(name = "ntdll", kind = "raw-dylib")]
    unsafe extern "system" {
        fn NtFsControlFile(
            h: usize,
            event: usize,
            apc: usize,
            context: usize,
            iosb: *mut [usize; 2],
            code: u32,
            input: *const u8,
            input_length: u32,
            output: *mut u8,
            output_length: u32,
        ) -> u32;
    }
    let mut iosb = [0usize; 2];
    // SAFETY: plain call with our status block.
    result(unsafe {
        NtFsControlFile(
            pipe,
            0,
            0,
            0,
            &mut iosb,
            0x0011_001c,
            core::ptr::null(),
            0,
            core::ptr::null_mut(),
            0,
        )
    })
}
