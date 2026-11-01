//! Security structures (SIDs, ACLs, security descriptors) built and read
//! as documented, the accounts gala knows, and the process token.
//!
//! gala has one user, "gala" (machine GALA), and enforces no security:
//! SIDs, ACLs and descriptors are real data (programs build and read
//! them), but files keep no descriptors (SetFileSecurity succeeds and is
//! recorded). Tokens are the kernel's (`token.rs`; docs/SECURITY.md).

use core::ffi::c_void;

use super::{ERROR_INSUFFICIENT_BUFFER, fail, missing};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LocalAlloc(flags: u32, bytes: usize) -> *mut u8;
    fn LoadLibraryA(name: *const u8) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> usize;
}

const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_SID: u32 = 1337;
const ERROR_NONE_MAPPED: u32 = 1332;
const ERROR_ALLOTTED_SPACE_EXCEEDED: u32 = 1344;
const ERROR_INVALID_ACL: u32 = 1336;
const ERROR_NO_SUCH_PRIVILEGE: u32 = 1313;
const ERROR_INVALID_SECURITY_DESCR: u32 = 1338;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;

const SID_MAX_SUB_AUTHORITIES: usize = 15;

// ---- SIDs ----
//
// SID: Revision (1), SubAuthorityCount, IdentifierAuthority (6 bytes, big
// endian), SubAuthority[count] (DWORDs).

/// The SID at `p` as (authority, sub-authorities), if valid.
fn read_sid(p: *const u8) -> Option<(u64, [u32; SID_MAX_SUB_AUTHORITIES], usize)> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller's SID; its length comes from its header.
    unsafe {
        let count = usize::from(*p.add(1));
        if *p != 1 || count > SID_MAX_SUB_AUTHORITIES {
            return None;
        }
        let mut authority = 0u64;
        for i in 0..6 {
            authority = authority << 8 | u64::from(*p.add(2 + i));
        }
        let mut subs = [0u32; SID_MAX_SUB_AUTHORITIES];
        for (i, s) in subs.iter_mut().enumerate().take(count) {
            *s = p.add(8 + 4 * i).cast::<u32>().read_unaligned();
        }
        Some((authority, subs, count))
    }
}

/// Writes a SID at `p` (8 + 4 * subs.len() bytes).
fn write_sid(p: *mut u8, authority: [u8; 6], subs: &[u32]) {
    // SAFETY: the caller's buffer of the SID's length.
    unsafe {
        *p = 1;
        *p.add(1) = subs.len() as u8;
        core::ptr::copy_nonoverlapping(authority.as_ptr(), p.add(2), 6);
        for (i, s) in subs.iter().enumerate() {
            p.add(8 + 4 * i).cast::<u32>().write_unaligned(*s);
        }
    }
}

fn sid_len(count: usize) -> usize {
    8 + 4 * count
}

#[unsafe(no_mangle)]
pub extern "system" fn IsValidSid(sid: *const u8) -> i32 {
    i32::from(read_sid(sid).is_some())
}

#[unsafe(no_mangle)]
pub extern "system" fn GetLengthSid(sid: *const u8) -> u32 {
    read_sid(sid).map_or(0, |(_, _, n)| sid_len(n) as u32)
}

/// AllocateAndInitializeSid: a SID in local memory (freed by FreeSid).
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn AllocateAndInitializeSid(
    authority: *const [u8; 6],
    count: u8,
    s0: u32,
    s1: u32,
    s2: u32,
    s3: u32,
    s4: u32,
    s5: u32,
    s6: u32,
    s7: u32,
    sid: *mut *mut u8,
) -> i32 {
    if authority.is_null() || sid.is_null() || count == 0 || count > 8 {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let subs = [s0, s1, s2, s3, s4, s5, s6, s7];
    // SAFETY: plain call.
    let p = unsafe { LocalAlloc(0, sid_len(count.into())) };
    if p.is_null() {
        return fail(8, 0); // ERROR_NOT_ENOUGH_MEMORY
    }
    // SAFETY: the caller's authority and output.
    unsafe {
        write_sid(p, *authority, &subs[..count.into()]);
        *sid = p;
    }
    1
}

/// InitializeSid: the revision, count and authority of a SID in the
/// caller's buffer; the sub-authorities are left for GetSidSubAuthority.
#[unsafe(no_mangle)]
pub extern "system" fn InitializeSid(sid: *mut u8, authority: *const [u8; 6], count: u8) -> i32 {
    if sid.is_null() || authority.is_null() || usize::from(count) > SID_MAX_SUB_AUTHORITIES {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's buffer of at least the header (8 bytes).
    unsafe {
        *sid = 1;
        *sid.add(1) = count;
        core::ptr::copy_nonoverlapping((*authority).as_ptr(), sid.add(2), 6);
    }
    1
}

/// FreeSid: frees a SID from AllocateAndInitializeSid; returns NULL.
#[unsafe(no_mangle)]
pub extern "system" fn FreeSid(sid: *mut u8) -> *mut u8 {
    super::local_free(sid.cast());
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "system" fn CopySid(length: u32, dst: *mut u8, src: *const u8) -> i32 {
    let Some((_, _, n)) = read_sid(src) else { return fail(ERROR_INVALID_SID, 0) };
    if (length as usize) < sid_len(n) || dst.is_null() {
        return fail(ERROR_INSUFFICIENT_BUFFER, 0);
    }
    // SAFETY: both of at least the SID's length.
    unsafe { core::ptr::copy_nonoverlapping(src, dst, sid_len(n)) };
    1
}

/// Formats `v` in decimal into `out` at `*n`.
fn push_decimal(out: &mut [u16], n: &mut usize, mut v: u64) -> Option<()> {
    let mut digits = [0u8; 20];
    let mut d = digits.len();
    loop {
        d -= 1;
        digits[d] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    for &c in &digits[d..] {
        *out.get_mut(*n)? = c.into();
        *n += 1;
    }
    Some(())
}

/// ConvertSidToStringSidW: "S-1-<authority>-<sub>-..." in local memory
/// (authorities of 2^32 and more in hexadecimal, as documented).
#[unsafe(no_mangle)]
pub extern "system" fn ConvertSidToStringSidW(sid: *const u8, out: *mut *mut u16) -> i32 {
    let Some((authority, subs, count)) = read_sid(sid) else {
        return fail(ERROR_INVALID_SID, 0);
    };
    let mut text = [0u16; 200];
    let mut n = 0;
    for c in "S-1-".encode_utf16() {
        text[n] = c;
        n += 1;
    }
    if authority >> 32 != 0 {
        for c in "0x".encode_utf16() {
            text[n] = c;
            n += 1;
        }
        for shift in (0..12).rev() {
            let d = ((authority >> (4 * shift)) & 0xf) as u8;
            text[n] = u16::from(if d < 10 { b'0' + d } else { b'A' + d - 10 });
            n += 1;
        }
    } else if push_decimal(&mut text, &mut n, authority).is_none() {
        return fail(ERROR_INVALID_SID, 0);
    }
    for &s in &subs[..count] {
        text[n] = u16::from(b'-');
        n += 1;
        if push_decimal(&mut text, &mut n, s.into()).is_none() {
            return fail(ERROR_INVALID_SID, 0);
        }
    }
    // SAFETY: plain call.
    let p = unsafe { LocalAlloc(0, 2 * (n + 1)) }.cast::<u16>();
    if p.is_null() {
        return fail(8, 0);
    }
    // SAFETY: a buffer of n + 1 units; the caller's output.
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), p, n);
        *p.add(n) = 0;
        *out = p;
    }
    1
}

// ---- Accounts ----

const SID_TYPE_USER: u32 = 1;
const SID_TYPE_WELL_KNOWN_GROUP: u32 = 5;
const SID_TYPE_ALIAS: u32 = 4;

/// gala's user's SID: a machine SID (S-1-5-21-x-y-z) and RID 1001, the
/// first local account's.
pub(crate) const USER_SUBS: [u32; 5] = [21, 1_000_000_001, 1_000_000_002, 1_000_000_003, 1001];
const NT: [u8; 6] = [0, 0, 0, 0, 0, 5];

/// Accounts: (authority, sub-authorities, name, domain, SID_NAME_USE).
const ACCOUNTS: &[(u64, &[u32], &str, &str, u32)] = &[
    (5, &USER_SUBS, "gala", "GALA", SID_TYPE_USER),
    (1, &[0], "Everyone", "", SID_TYPE_WELL_KNOWN_GROUP),
    (3, &[0], "CREATOR OWNER", "", SID_TYPE_WELL_KNOWN_GROUP),
    (5, &[11], "Authenticated Users", "NT AUTHORITY", SID_TYPE_WELL_KNOWN_GROUP),
    // LocalSystem is a user account (Win32-OpenSSH, which accepts only
    // SidTypeUser accounts, runs as SYSTEM on Windows; system_test).
    (5, &[18], "SYSTEM", "NT AUTHORITY", SID_TYPE_USER),
    (5, &[32, 544], "Administrators", "BUILTIN", SID_TYPE_ALIAS),
    (5, &[32, 545], "Users", "BUILTIN", SID_TYPE_ALIAS),
];

// ---- Service SIDs ----
//
// "NT SERVICE\<name>" is S-1-5-80 followed by the SHA-1 of the service's
// name, upper-cased, in UTF-16 (five little-endian DWORDs), as documented
// for service SIDs; S-1-5-80-0 is "NT SERVICE\ALL SERVICES". Names resolve
// for services the system has: the keys under
// HKLM\SYSTEM\CurrentControlSet\Services (where the service control
// manager keeps them). SHA-1 comes from bcrypt, loaded on first use.

const NT_SERVICE: &str = "NT SERVICE";

/// The five sub-authorities after 80 of service `name`'s SID.
fn service_hash(name: &[u16]) -> Option<[u32; 5]> {
    type HashFn =
        unsafe extern "system" fn(usize, *const u8, u32, *const u8, u32, *mut u8, u32) -> u32;
    let mut bytes = [0u8; 512];
    if name.len() * 2 > bytes.len() {
        return None;
    }
    for (i, &c) in name.iter().enumerate() {
        let up = if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c };
        bytes[2 * i..2 * i + 2].copy_from_slice(&up.to_le_bytes());
    }
    let mut digest = [0u8; 20];
    // SAFETY: loading bcrypt and calling BCryptHash (documented signature)
    // with the SHA-1 pseudo-handle and our buffers.
    let ok = unsafe {
        let m = LoadLibraryA(c"bcrypt.dll".as_ptr().cast());
        let f = if m == 0 { 0 } else { GetProcAddress(m, c"BCryptHash".as_ptr().cast()) };
        f != 0
            && core::mem::transmute::<usize, HashFn>(f)(
                0x31,
                core::ptr::null(),
                0,
                bytes.as_ptr(),
                (2 * name.len()) as u32,
                digest.as_mut_ptr(),
                20,
            ) == 0
    };
    if !ok {
        return None;
    }
    let mut subs = [0u32; 5];
    for (i, s) in subs.iter_mut().enumerate() {
        *s = u32::from_le_bytes([
            digest[4 * i],
            digest[4 * i + 1],
            digest[4 * i + 2],
            digest[4 * i + 3],
        ]);
    }
    Some(subs)
}

/// Calls `f` with each registered service's name (stopping when it
/// returns true); whether it did.
fn any_service(mut f: impl FnMut(&[u16]) -> bool) -> bool {
    const HKLM: usize = 0x8000_0002;
    let path: [u16; 35] = {
        let mut p = [0u16; 35];
        for (d, c) in p.iter_mut().zip("SYSTEM\\CurrentControlSet\\Services".encode_utf16()) {
            *d = c;
        }
        p
    };
    let mut key = 0usize;
    if super::registry::RegOpenKeyExW(HKLM, path.as_ptr(), 0, 0x2_0019, &mut key) != 0 {
        return false;
    }
    let mut found = false;
    let mut index = 0;
    loop {
        let mut name = [0u16; 256];
        let mut len = 256u32;
        let r = super::registry::RegEnumKeyExW(
            key,
            index,
            name.as_mut_ptr(),
            &mut len,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        if r != 0 {
            break;
        }
        if f(&name[..len as usize]) {
            found = true;
            break;
        }
        index += 1;
    }
    super::registry::RegCloseKey(key);
    found
}

fn same_upper(a: &[u16], b: &[u16]) -> bool {
    let up = |c: u16| if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c };
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| up(x) == up(y))
}

fn service_registered(name: &[u16]) -> bool {
    any_service(|s| same_upper(s, name))
}

/// The registered service whose SID has sub-authorities `subs` (after
/// 80): its name into `out`, and the length.
fn service_named_by(subs: &[u32], out: &mut [u16; 256]) -> Option<usize> {
    let mut n = None;
    any_service(|s| {
        if service_hash(s).is_some_and(|h| h[..] == *subs) {
            out[..s.len()].copy_from_slice(s);
            n = Some(s.len());
            return true;
        }
        false
    });
    n
}

/// Copies `text` with a NUL into `buf` of `*size` units: on success
/// `*size` is the length without the NUL.
fn put_name(text: &str, buf: *mut u16) {
    for (i, c) in text.encode_utf16().enumerate() {
        // SAFETY: the caller checked the size.
        unsafe { *buf.add(i) = c };
    }
    // SAFETY: as above.
    unsafe { *buf.add(text.len()) = 0 };
}

/// LookupAccountSidW: the name and domain of a SID gala knows; a size
/// query (or short buffers) fails with ERROR_INSUFFICIENT_BUFFER and the
/// sizes needed with the NUL; ERROR_NONE_MAPPED for other SIDs.
#[unsafe(no_mangle)]
pub extern "system" fn LookupAccountSidW(
    _system: *const u16,
    sid: *const u8,
    name: *mut u16,
    name_len: *mut u32,
    domain: *mut u16,
    domain_len: *mut u32,
    kind: *mut u32,
) -> i32 {
    let Some((authority, subs, count)) = read_sid(sid) else {
        return fail(ERROR_INVALID_SID, 0);
    };
    // The account's name (UTF-16), domain and SID_NAME_USE.
    let mut account = [0u16; 256];
    let (account_len, dom, use_) = if let Some(&(_, _, a, dom, use_)) =
        ACCOUNTS.iter().find(|a| a.0 == authority && a.1 == &subs[..count])
    {
        for (d, c) in account.iter_mut().zip(a.encode_utf16()) {
            *d = c;
        }
        (a.len(), dom, use_)
    } else if authority == 5
        && count == 5
        && subs[..4] == USER_SUBS[..4]
        && sam_name(subs[4], &mut account).is_some()
    {
        let n = account.iter().position(|&c| c == 0).unwrap_or(account.len());
        (n, "GALA", SID_TYPE_USER)
    } else if authority == 5 && count == 2 && subs[..2] == [80, 0] {
        for (d, c) in account.iter_mut().zip("ALL SERVICES".encode_utf16()) {
            *d = c;
        }
        (12, NT_SERVICE, SID_TYPE_WELL_KNOWN_GROUP)
    } else if authority == 5 && count == 6 && subs[0] == 80 {
        match service_named_by(&subs[1..6], &mut account) {
            Some(n) => (n, NT_SERVICE, SID_TYPE_WELL_KNOWN_GROUP),
            None => return fail(ERROR_NONE_MAPPED, 0),
        }
    } else {
        return fail(ERROR_NONE_MAPPED, 0);
    };
    let account = &account[..account_len];
    if name_len.is_null() || domain_len.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's sizes and buffers of those sizes.
    unsafe {
        let (need_name, need_domain) = (account.len() as u32 + 1, dom.len() as u32 + 1);
        if name.is_null() || domain.is_null() || *name_len < need_name || *domain_len < need_domain
        {
            *name_len = need_name;
            *domain_len = need_domain;
            return fail(ERROR_INSUFFICIENT_BUFFER, 0);
        }
        core::ptr::copy_nonoverlapping(account.as_ptr(), name, account.len());
        *name.add(account.len()) = 0;
        put_name(dom, domain);
        *name_len = need_name - 1;
        *domain_len = need_domain - 1;
        if !kind.is_null() {
            *kind = use_;
        }
    }
    1
}

/// The name of the caller's user: its thread's impersonation token's,
/// else its process token's (LookupAccountSidW), into `name`; its length
/// and the domain's into `domain`.
pub(crate) fn current_user(
    name: &mut [u16; 256],
    domain: &mut [u16; 64],
) -> Option<(usize, usize)> {
    #[link(name = "ntdll", kind = "raw-dylib")]
    unsafe extern "system" {
        fn NtOpenThreadToken(thread: usize, access: u32, as_self: u8, token: *mut usize) -> u32;
        fn NtOpenProcessToken(process: usize, access: u32, token: *mut usize) -> u32;
        fn NtQueryInformationToken(
            token: usize,
            class: u32,
            buf: *mut u8,
            len: u32,
            ret: *mut u32,
        ) -> u32;
        fn NtClose(h: usize) -> u32;
    }
    let mut token = 0;
    let mut b = [0u64; 32];
    let mut n = 0;
    // SAFETY: plain calls with our buffers (TOKEN_QUERY; TokenUser).
    let sid = unsafe {
        if NtOpenThreadToken(usize::MAX - 1, 8, 1, &mut token) != 0
            && NtOpenProcessToken(usize::MAX, 8, &mut token) != 0
        {
            return None;
        }
        let status = NtQueryInformationToken(token, 1, b.as_mut_ptr().cast(), 256, &mut n);
        NtClose(token);
        if status != 0 {
            return None;
        }
        b[0] as *const u8
    };
    let (mut name_len, mut domain_len, mut kind) = (256u32, 64u32, 0u32);
    let r = LookupAccountSidW(
        core::ptr::null(),
        sid,
        name.as_mut_ptr(),
        &mut name_len,
        domain.as_mut_ptr(),
        &mut domain_len,
        &mut kind,
    );
    (r != 0).then_some((name_len as usize, domain_len as usize))
}

/// GetUserNameW: the caller's user's name (an impersonating thread's:
/// the client's), as documented; the size includes the NUL.
#[unsafe(no_mangle)]
pub extern "system" fn GetUserNameW(buffer: *mut u16, size: *mut u32) -> i32 {
    if size.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let (mut name, mut domain) = ([0u16; 256], [0u16; 64]);
    let Some((len, _)) = current_user(&mut name, &mut domain) else { return 0 };
    let needed = len as u32 + 1;
    // SAFETY: the caller's size and buffer.
    unsafe {
        if buffer.is_null() || *size < needed {
            *size = needed;
            return fail(ERROR_INSUFFICIENT_BUFFER, 0);
        }
        core::ptr::copy_nonoverlapping(name.as_ptr(), buffer, len);
        *buffer.add(len) = 0;
        *size = needed;
    }
    1
}

// ---- Local accounts (the kernel's account database) ----
//
// The accounts beyond `gala` are the subkeys of HKLM\SAM\Gala\Users
// (nt::accounts): their names, and their RID in the value `Rid`. SIDs are
// the machine's (USER_SUBS without the RID) with that RID.

const SAM_USERS: &str = "SAM\\Gala\\Users";

/// The RID of account `name` (not `gala`, which ACCOUNTS has).
fn sam_rid(name: &[u16]) -> Option<u32> {
    if name.is_empty() || name.len() > 200 || name.contains(&u16::from(b'\\')) {
        return None;
    }
    let mut path = [0u16; 256];
    let mut n = 0;
    for c in SAM_USERS
        .encode_utf16()
        .chain(core::iter::once(u16::from(b'\\')))
        .chain(name.iter().copied())
    {
        path[n] = c;
        n += 1;
    }
    let value = [b'R', b'i', b'd', 0].map(u16::from);
    let mut rid = 0u32;
    let mut size = 4u32;
    // RRF_RT_REG_DWORD (0x10).
    let r = {
        super::registry::RegGetValueW(
            0x8000_0002,
            path.as_ptr(),
            value.as_ptr(),
            0x10,
            core::ptr::null_mut(),
            (&raw mut rid).cast(),
            &mut size,
        )
    };
    (r == 0).then_some(rid)
}

/// The name of the account with RID `rid` into `out` (NUL-terminated).
fn sam_name(rid: u32, out: &mut [u16; 256]) -> Option<()> {
    let mut users = [0u16; 32];
    for (slot, c) in users.iter_mut().zip(SAM_USERS.encode_utf16()) {
        *slot = c;
    }
    let mut key = 0;
    // KEY_READ.
    if super::registry::RegOpenKeyExW(0x8000_0002, users.as_ptr(), 0, 0x2_0019, &mut key) != 0 {
        return None;
    }
    let mut found = None;
    for index in 0..1024 {
        let mut name = [0u16; 256];
        let mut len = 256u32;
        let r = {
            super::registry::RegEnumKeyExW(
                key,
                index,
                name.as_mut_ptr(),
                &mut len,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            )
        };
        if r != 0 {
            break;
        }
        if sam_rid(&name[..len as usize]) == Some(rid) {
            *out = name;
            found = Some(());
            break;
        }
    }
    super::registry::RegCloseKey(key);
    found
}

// ---- ACLs ----
//
// ACL: AclRevision, Sbz1, AclSize (WORD), AceCount (WORD), Sbz2; the ACEs
// follow, each starting with ACE_HEADER (AceType, AceFlags, AceSize).

fn acl_word(acl: *const u8, off: usize) -> u16 {
    // SAFETY: inside the caller's ACL header.
    unsafe { acl.add(off).cast::<u16>().read_unaligned() }
}

fn set_acl_word(acl: *mut u8, off: usize, v: u16) {
    // SAFETY: as above.
    unsafe { acl.add(off).cast::<u16>().write_unaligned(v) }
}

/// The bytes the ACL's header and ACEs use.
fn acl_used(acl: *const u8) -> Option<usize> {
    let mut used = 8;
    for _ in 0..acl_word(acl, 4) {
        // SAFETY: an ACE header inside the ACL.
        let size = usize::from(unsafe { acl.add(used + 2).cast::<u16>().read_unaligned() });
        if size < 4 || used + size > usize::from(acl_word(acl, 2)) {
            return None;
        }
        used += size;
    }
    Some(used)
}

#[unsafe(no_mangle)]
pub extern "system" fn InitializeAcl(acl: *mut u8, length: u32, revision: u32) -> i32 {
    if acl.is_null() || !(2..=4).contains(&revision) || !(8..=0xffff).contains(&length) {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let length = (length & !3) as u16;
    // SAFETY: the caller's buffer of `length` bytes.
    unsafe { core::ptr::write_bytes(acl, 0, 8) };
    // SAFETY: as above.
    unsafe { *acl = revision as u8 };
    set_acl_word(acl, 2, length);
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn IsValidAcl(acl: *const u8) -> i32 {
    // SAFETY: the caller's ACL.
    i32::from(!acl.is_null() && (2..=4).contains(&unsafe { *acl }) && acl_used(acl).is_some())
}

/// Inserts `count` ACEs (`bytes` long, at `list`) before ACE `index`
/// (or at the end).
fn insert_aces(acl: *mut u8, index: u32, list: *const u8, bytes: usize, count: u16) -> i32 {
    let Some(used) = acl_used(acl) else { return fail(ERROR_INVALID_ACL, 0) };
    if used + bytes > usize::from(acl_word(acl, 2)) {
        return fail(ERROR_ALLOTTED_SPACE_EXCEEDED, 0);
    }
    // Where ACE `index` starts.
    let mut at = 8;
    for _ in 0..index.min(u32::from(acl_word(acl, 4))) {
        // SAFETY: an ACE header inside the ACL.
        at += usize::from(unsafe { acl.add(at + 2).cast::<u16>().read_unaligned() });
    }
    // SAFETY: inside the ACL's AclSize (checked above).
    unsafe {
        core::ptr::copy(acl.add(at), acl.add(at + bytes), used - at);
        core::ptr::copy_nonoverlapping(list, acl.add(at), bytes);
    }
    set_acl_word(acl, 4, acl_word(acl, 4) + count);
    1
}

/// AddAccessAllowedAce (and the Ex form with flags): ACCESS_ALLOWED_ACE
/// (header, mask, SID) at the end.
#[unsafe(no_mangle)]
pub extern "system" fn AddAccessAllowedAce(
    acl: *mut u8,
    revision: u32,
    mask: u32,
    sid: *const u8,
) -> i32 {
    AddAccessAllowedAceEx(acl, revision, 0, mask, sid)
}

fn add_simple_ace(acl: *mut u8, kind: u8, flags: u32, mask: u32, sid: *const u8) -> i32 {
    let Some((_, _, n)) = read_sid(sid) else { return fail(ERROR_INVALID_SID, 0) };
    let size = 8 + sid_len(n);
    let mut ace = [0u8; 8 + 8 + 4 * SID_MAX_SUB_AUTHORITIES];
    ace[0] = kind;
    ace[1] = flags as u8;
    ace[2..4].copy_from_slice(&(size as u16).to_le_bytes());
    ace[4..8].copy_from_slice(&mask.to_le_bytes());
    // SAFETY: a valid SID of sid_len(n) bytes.
    unsafe { core::ptr::copy_nonoverlapping(sid, ace.as_mut_ptr().add(8), sid_len(n)) };
    insert_aces(acl, u32::MAX, ace.as_ptr(), size, 1)
}

#[unsafe(no_mangle)]
pub extern "system" fn AddAccessAllowedAceEx(
    acl: *mut u8,
    _revision: u32,
    flags: u32,
    mask: u32,
    sid: *const u8,
) -> i32 {
    add_simple_ace(acl, 0, flags, mask, sid) // ACCESS_ALLOWED_ACE_TYPE
}

#[unsafe(no_mangle)]
pub extern "system" fn AddAccessDeniedAce(
    acl: *mut u8,
    _revision: u32,
    mask: u32,
    sid: *const u8,
) -> i32 {
    add_simple_ace(acl, 1, 0, mask, sid) // ACCESS_DENIED_ACE_TYPE
}

#[unsafe(no_mangle)]
pub extern "system" fn AddAccessDeniedAceEx(
    acl: *mut u8,
    _revision: u32,
    flags: u32,
    mask: u32,
    sid: *const u8,
) -> i32 {
    add_simple_ace(acl, 1, flags, mask, sid)
}

/// AddAce: inserts a list of ACEs (`length` bytes) before `index`
/// (MAXDWORD: at the end).
#[unsafe(no_mangle)]
pub extern "system" fn AddAce(
    acl: *mut u8,
    _revision: u32,
    index: u32,
    list: *const u8,
    length: u32,
) -> i32 {
    if list.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    // Count the ACEs in the list.
    let (mut at, mut count) = (0usize, 0u16);
    while at + 4 <= length as usize {
        // SAFETY: an ACE header inside the caller's list.
        let size = usize::from(unsafe { list.add(at + 2).cast::<u16>().read_unaligned() });
        if size < 4 {
            return fail(ERROR_INVALID_PARAMETER, 0);
        }
        at += size;
        count += 1;
    }
    if at != length as usize {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    insert_aces(acl, index, list, length as usize, count)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetAce(acl: *const u8, index: u32, ace: *mut *mut u8) -> i32 {
    if acl.is_null() || index >= u32::from(acl_word(acl, 4)) {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let mut at = 8;
    for _ in 0..index {
        // SAFETY: an ACE header inside the ACL.
        at += usize::from(unsafe { acl.add(at + 2).cast::<u16>().read_unaligned() });
    }
    // SAFETY: the caller's output.
    unsafe { *ace = acl.add(at).cast_mut() };
    1
}

/// GetAclInformation: AclRevisionInformation (1) or AclSizeInformation
/// (2: AceCount, AclBytesInUse, AclBytesFree).
#[unsafe(no_mangle)]
pub extern "system" fn GetAclInformation(
    acl: *const u8,
    info: *mut u32,
    length: u32,
    class: u32,
) -> i32 {
    let Some(used) = (!acl.is_null()).then(|| acl_used(acl)).flatten() else {
        return fail(ERROR_INVALID_ACL, 0);
    };
    // SAFETY: the caller's buffer of `length` bytes.
    unsafe {
        match class {
            1 if length >= 4 => *info = u32::from(*acl),
            2 if length >= 12 => {
                *info = acl_word(acl, 4).into();
                *info.add(1) = used as u32;
                *info.add(2) = u32::from(acl_word(acl, 2)) - used as u32;
            }
            1 | 2 => return fail(ERROR_INSUFFICIENT_BUFFER, 0),
            _ => return fail(ERROR_INVALID_PARAMETER, 0),
        }
    }
    1
}

// ---- Security descriptors (absolute, x64) ----
//
// SECURITY_DESCRIPTOR: Revision, Sbz1, Control (WORD), Owner, Group,
// Sacl, Dacl (pointers at 8, 16, 24, 32).

const SE_DACL_PRESENT: u16 = 0x4;
const SE_DACL_DEFAULTED: u16 = 0x8;
const SE_SELF_RELATIVE: u16 = 0x8000;

#[unsafe(no_mangle)]
pub extern "system" fn InitializeSecurityDescriptor(sd: *mut u8, revision: u32) -> i32 {
    if sd.is_null() || revision != 1 {
        return fail(1305, 0); // ERROR_UNKNOWN_REVISION
    }
    // SAFETY: the caller's 40-byte descriptor.
    unsafe {
        core::ptr::write_bytes(sd, 0, 40);
        *sd = 1;
    }
    1
}

fn control(sd: *const u8) -> u16 {
    // SAFETY: the caller's descriptor.
    unsafe { sd.add(2).cast::<u16>().read_unaligned() }
}

#[unsafe(no_mangle)]
pub extern "system" fn IsValidSecurityDescriptor(sd: *const u8) -> i32 {
    // SAFETY: the caller's descriptor.
    i32::from(!sd.is_null() && unsafe { *sd } == 1)
}

#[unsafe(no_mangle)]
pub extern "system" fn SetSecurityDescriptorDacl(
    sd: *mut u8,
    present: i32,
    dacl: *mut u8,
    defaulted: i32,
) -> i32 {
    if IsValidSecurityDescriptor(sd) == 0 || control(sd) & SE_SELF_RELATIVE != 0 {
        return fail(ERROR_INVALID_SECURITY_DESCR, 0);
    }
    let mut c = control(sd) & !(SE_DACL_PRESENT | SE_DACL_DEFAULTED);
    let mut p = core::ptr::null_mut();
    if present != 0 {
        c |= SE_DACL_PRESENT;
        p = dacl;
        if defaulted != 0 {
            c |= SE_DACL_DEFAULTED;
        }
    }
    // SAFETY: the caller's absolute descriptor.
    unsafe {
        sd.add(2).cast::<u16>().write_unaligned(c);
        sd.add(32).cast::<*mut u8>().write_unaligned(p);
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn GetSecurityDescriptorDacl(
    sd: *const u8,
    present: *mut i32,
    dacl: *mut *mut u8,
    defaulted: *mut i32,
) -> i32 {
    if IsValidSecurityDescriptor(sd) == 0 {
        return fail(ERROR_INVALID_SECURITY_DESCR, 0);
    }
    let c = control(sd);
    // SAFETY: the caller's descriptor (absolute: a pointer at 32;
    // self-relative: an offset at 16) and outputs.
    unsafe {
        *present = i32::from(c & SE_DACL_PRESENT != 0);
        if c & SE_DACL_PRESENT != 0 {
            *dacl = if c & SE_SELF_RELATIVE != 0 {
                let off = sd.add(16).cast::<u32>().read_unaligned() as usize;
                if off == 0 { core::ptr::null_mut() } else { sd.add(off).cast_mut() }
            } else {
                sd.add(32).cast::<*mut u8>().read_unaligned()
            };
            *defaulted = i32::from(c & SE_DACL_DEFAULTED != 0);
        }
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn SetSecurityDescriptorOwner(
    sd: *mut u8,
    owner: *mut u8,
    defaulted: i32,
) -> i32 {
    if IsValidSecurityDescriptor(sd) == 0 {
        return fail(ERROR_INVALID_SECURITY_DESCR, 0);
    }
    // SAFETY: the caller's absolute descriptor; SE_OWNER_DEFAULTED is 1.
    unsafe {
        let c = (control(sd) & !1) | u16::from(defaulted != 0);
        sd.add(2).cast::<u16>().write_unaligned(c);
        sd.add(8).cast::<*mut u8>().write_unaligned(owner);
    }
    1
}

/// Well-known privilege names and their LUIDs (SE_*_PRIVILEGE).
const PRIVILEGES: &[(&str, u32)] = &[
    ("SeCreateTokenPrivilege", 2),
    ("SeAssignPrimaryTokenPrivilege", 3),
    ("SeLockMemoryPrivilege", 4),
    ("SeIncreaseQuotaPrivilege", 5),
    ("SeMachineAccountPrivilege", 6),
    ("SeTcbPrivilege", 7),
    ("SeSecurityPrivilege", 8),
    ("SeTakeOwnershipPrivilege", 9),
    ("SeLoadDriverPrivilege", 10),
    ("SeSystemProfilePrivilege", 11),
    ("SeSystemtimePrivilege", 12),
    ("SeProfileSingleProcessPrivilege", 13),
    ("SeIncreaseBasePriorityPrivilege", 14),
    ("SeCreatePagefilePrivilege", 15),
    ("SeCreatePermanentPrivilege", 16),
    ("SeBackupPrivilege", 17),
    ("SeRestorePrivilege", 18),
    ("SeShutdownPrivilege", 19),
    ("SeDebugPrivilege", 20),
    ("SeAuditPrivilege", 21),
    ("SeSystemEnvironmentPrivilege", 22),
    ("SeChangeNotifyPrivilege", 23),
    ("SeRemoteShutdownPrivilege", 24),
    ("SeUndockPrivilege", 25),
    ("SeSyncAgentPrivilege", 26),
    ("SeEnableDelegationPrivilege", 27),
    ("SeManageVolumePrivilege", 28),
    ("SeImpersonatePrivilege", 29),
    ("SeCreateGlobalPrivilege", 30),
    ("SeTrustedCredManAccessPrivilege", 31),
    ("SeRelabelPrivilege", 32),
    ("SeIncreaseWorkingSetPrivilege", 33),
    ("SeTimeZonePrivilege", 34),
    ("SeCreateSymbolicLinkPrivilege", 35),
    ("SeDelegateSessionUserImpersonatePrivilege", 36),
];

fn privilege_luid(name: impl Iterator<Item = u16> + Clone) -> Option<u32> {
    PRIVILEGES.iter().find_map(|&(n, luid)| {
        let mut a = name.clone();
        let same = n
            .bytes()
            .all(|b| a.next().map(|c| (c as u8).eq_ignore_ascii_case(&b) && c < 128) == Some(true))
            && a.next().is_none();
        same.then_some(luid)
    })
}

fn write_luid(luid: *mut u32, v: Option<u32>) -> i32 {
    let Some(v) = v else { return fail(ERROR_NO_SUCH_PRIVILEGE, 0) };
    // SAFETY: the caller's LUID.
    unsafe {
        *luid = v;
        *luid.add(1) = 0;
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn LookupPrivilegeValueA(
    _system: *const u8,
    name: *const u8,
    luid: *mut u32,
) -> i32 {
    if name.is_null() || luid.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's NUL-terminated name.
    let bytes = unsafe {
        core::slice::from_raw_parts(name, (0..).take_while(|&i| *name.add(i) != 0).count())
    };
    write_luid(luid, privilege_luid(bytes.iter().map(|&b| u16::from(b))))
}

#[unsafe(no_mangle)]
pub extern "system" fn LookupPrivilegeValueW(
    _system: *const u16,
    name: *const u16,
    luid: *mut u32,
) -> i32 {
    if name.is_null() || luid.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's NUL-terminated name.
    let units = unsafe {
        core::slice::from_raw_parts(name, (0..).take_while(|&i| *name.add(i) != 0).count())
    };
    write_luid(luid, privilege_luid(units.iter().copied()))
}

// ---- CryptoAPI and registry forms the JDK imports ----

/// CryptAcquireContextA: CryptAcquireContextW (a verify context only).
#[unsafe(no_mangle)]
pub extern "system" fn CryptAcquireContextA(
    provider: *mut usize,
    container: *const u8,
    _name: *const u8,
    kind: u32,
    flags: u32,
) -> i32 {
    let container: *const u16 =
        if container.is_null() { core::ptr::null() } else { [0u16; 1].as_ptr() };
    super::CryptAcquireContextW(provider, container, core::ptr::null(), kind, flags)
}

// ---- What the JDK's net and nio libraries import ----

/// ConvertStringSidToSidW: "S-1-<authority>-<sub>..." (decimal, or a
/// 0x-hexadecimal authority) and the SDDL abbreviations for the
/// well-known accounts gala knows (BA, BU, WD, SY, AU, CO); the SID in
/// local memory (LocalFree).
#[unsafe(no_mangle)]
pub extern "system" fn ConvertStringSidToSidW(text: *const u16, sid: *mut *mut u8) -> i32 {
    if text.is_null() || sid.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let mut chars = [0u8; 200];
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    unsafe {
        while *text.add(n) != 0 {
            if n == chars.len() {
                return fail(ERROR_INVALID_SID, 0);
            }
            chars[n] = u8::try_from(*text.add(n)).unwrap_or(0);
            n += 1;
        }
    }
    let s = core::str::from_utf8(&chars[..n]).unwrap_or("");
    let (authority, subs): (u64, &[u32]) = match s {
        "BA" => (5, &[32, 544]),
        "BU" => (5, &[32, 545]),
        "WD" => (1, &[0]),
        "SY" => (5, &[18]),
        "AU" => (5, &[11]),
        "CO" => (3, &[0]),
        _ => (u64::MAX, &[]),
    };
    let mut parsed = [0u32; SID_MAX_SUB_AUTHORITIES];
    let (authority, count) = if authority != u64::MAX {
        parsed[..subs.len()].copy_from_slice(subs);
        (authority, subs.len())
    } else {
        let mut parts = s.split('-');
        if parts.next() != Some("S") || parts.next() != Some("1") {
            return fail(ERROR_INVALID_SID, 0);
        }
        let authority = match parts.next() {
            Some(a) if a.starts_with("0x") || a.starts_with("0X") => {
                u64::from_str_radix(&a[2..], 16).ok()
            }
            Some(a) => a.parse::<u64>().ok(),
            None => None,
        };
        let Some(authority) = authority.filter(|a| *a < 1 << 48) else {
            return fail(ERROR_INVALID_SID, 0);
        };
        let mut count = 0;
        for part in parts {
            let Some(v) = part.parse::<u32>().ok().filter(|_| count < SID_MAX_SUB_AUTHORITIES)
            else {
                return fail(ERROR_INVALID_SID, 0);
            };
            parsed[count] = v;
            count += 1;
        }
        if count == 0 {
            return fail(ERROR_INVALID_SID, 0);
        }
        (authority, count)
    };
    // SAFETY: plain call.
    let p = unsafe { LocalAlloc(0, sid_len(count)) };
    if p.is_null() {
        return fail(8, 0);
    }
    let a = authority.to_be_bytes();
    write_sid(p, [a[2], a[3], a[4], a[5], a[6], a[7]], &parsed[..count]);
    // SAFETY: the caller's output.
    unsafe { *sid = p };
    1
}

/// LookupAccountNameW: the SID, domain and SID_NAME_USE of an account
/// gala knows ("gala", "Administrators", "Everyone"...; "DOMAIN\\name"
/// too); the sizes needed (the domain's with its NUL) and
/// ERROR_INSUFFICIENT_BUFFER for a size query; ERROR_NONE_MAPPED for
/// other names.
#[unsafe(no_mangle)]
pub extern "system" fn LookupAccountNameW(
    _system: *const u16,
    name: *const u16,
    sid: *mut u8,
    sid_len_io: *mut u32,
    domain: *mut u16,
    domain_len: *mut u32,
    kind: *mut u32,
) -> i32 {
    if name.is_null() || sid_len_io.is_null() || domain_len.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let mut chars = [0u16; 128];
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated name.
    unsafe {
        while *name.add(n) != 0 && n < chars.len() {
            chars[n] = *name.add(n);
            n += 1;
        }
    }
    let full = &chars[..n];
    // "DOMAIN\name": the name part (the domain must match the account's).
    let (want_domain, account) = match full.iter().position(|&c| c == u16::from(b'\\')) {
        Some(i) => (Some(&full[..i]), &full[i + 1..]),
        None => (None, full),
    };
    let same = |a: &[u16], b: &str| {
        a.len() == b.len()
            && a.iter()
                .zip(b.bytes())
                .all(|(&x, y)| u8::try_from(x).is_ok_and(|x| x.eq_ignore_ascii_case(&y)))
    };
    let mut service = [0u32; 6];
    let (authority, subs, dom, use_): (u64, &[u32], &str, u32) =
        if let Some(&(authority, subs, _, dom, use_)) =
            ACCOUNTS.iter().find(|a| same(account, a.2) && want_domain.is_none_or(|d| same(d, a.3)))
        {
            (authority, subs, dom, use_)
        } else if want_domain.is_none_or(|d| same(d, "GALA") || same(d, "."))
            && let Some(rid) = sam_rid(account)
        {
            service[..4].copy_from_slice(&USER_SUBS[..4]);
            service[4] = rid;
            (5, &service[..5], "GALA", SID_TYPE_USER)
        } else if want_domain.is_some_and(|d| same(d, NT_SERVICE)) && same(account, "ALL SERVICES")
        {
            (5, &[80, 0], NT_SERVICE, SID_TYPE_WELL_KNOWN_GROUP)
        } else if want_domain.is_some_and(|d| same(d, NT_SERVICE)) && service_registered(account) {
            let Some(hash) = service_hash(account) else { return fail(ERROR_NONE_MAPPED, 0) };
            service[0] = 80;
            service[1..].copy_from_slice(&hash);
            (5, &service[..], NT_SERVICE, SID_TYPE_WELL_KNOWN_GROUP)
        } else {
            return fail(ERROR_NONE_MAPPED, 0);
        };
    let need_sid = sid_len(subs.len()) as u32;
    let need_domain = dom.len() as u32 + 1;
    // SAFETY: the caller's sizes and buffers.
    unsafe {
        if sid.is_null() || domain.is_null() || *sid_len_io < need_sid || *domain_len < need_domain
        {
            *sid_len_io = need_sid;
            *domain_len = need_domain;
            return fail(ERROR_INSUFFICIENT_BUFFER, 0);
        }
        let a = authority.to_be_bytes();
        write_sid(sid, [a[2], a[3], a[4], a[5], a[6], a[7]], subs);
        put_name(dom, domain);
        *sid_len_io = need_sid;
        *domain_len = need_domain - 1;
        if !kind.is_null() {
            *kind = use_;
        }
    }
    1
}

/// MapGenericMask: GENERIC_READ/WRITE/EXECUTE/ALL become the mapping's
/// rights (GENERIC_MAPPING: read, write, execute, all) and are cleared.
#[unsafe(no_mangle)]
pub extern "system" fn MapGenericMask(mask: *mut u32, mapping: *const u32) {
    if mask.is_null() || mapping.is_null() {
        return;
    }
    // SAFETY: the caller's mask and GENERIC_MAPPING.
    unsafe {
        let mut m = *mask;
        for (i, bit) in
            [0x8000_0000u32, 0x4000_0000, 0x2000_0000, 0x1000_0000].into_iter().enumerate()
        {
            if m & bit != 0 {
                m = (m & !bit) | *mapping.add(i);
            }
        }
        *mask = m;
    }
}

/// GetSecurityDescriptorOwner: absolute (a pointer at 8) or self-relative
/// (an offset at 4) descriptors; SE_OWNER_DEFAULTED is control bit 1.
#[unsafe(no_mangle)]
pub extern "system" fn GetSecurityDescriptorOwner(
    sd: *const u8,
    owner: *mut *mut u8,
    defaulted: *mut i32,
) -> i32 {
    if IsValidSecurityDescriptor(sd) == 0 || owner.is_null() {
        return fail(ERROR_INVALID_SECURITY_DESCR, 0);
    }
    let c = control(sd);
    // SAFETY: the caller's descriptor and outputs.
    unsafe {
        *owner = if c & SE_SELF_RELATIVE != 0 {
            let off = sd.add(4).cast::<u32>().read_unaligned() as usize;
            if off == 0 { core::ptr::null_mut() } else { sd.add(off).cast_mut() }
        } else {
            sd.add(8).cast::<*mut u8>().read_unaligned()
        };
        if !defaulted.is_null() {
            *defaulted = i32::from(c & 1 != 0);
        }
    }
    1
}

/// SetEntriesInAclW(cCountOfExplicitEntries, pListOfExplicitEntries,
/// OldAcl, *NewAcl): a new LocalAlloc'd ACL: the old one's ACEs with
/// each EXPLICIT_ACCESS_W applied - GRANT_ACCESS (1) adds an allowed ACE,
/// SET_ACCESS (2) replaces the trustee's ACEs with one, DENY_ACCESS (3)
/// adds a denied ACE, REVOKE_ACCESS (4) removes the trustee's ACEs - and
/// denied ACEs kept before allowed ones (canonical order). Trustees are
/// SIDs (TRUSTEE_IS_SID) or names LookupAccountNameW knows
/// (TRUSTEE_IS_NAME, "CURRENT_USER" included). Returns the error itself.
#[unsafe(no_mangle)]
pub extern "system" fn SetEntriesInAclW(
    count: u32,
    entries: *const u8,
    old: *const u8,
    new: *mut *mut u8,
) -> u32 {
    const ENTRY: usize = 48;
    if new.is_null() || (count > 0 && entries.is_null()) {
        return ERROR_INVALID_PARAMETER;
    }
    if !old.is_null() && IsValidAcl(old) == 0 {
        return ERROR_INVALID_ACL;
    }
    // The ACEs, as (bytes), denied ones first.
    let mut aces: [[u8; 8 + 8 + 4 * SID_MAX_SUB_AUTHORITIES]; 64] =
        [[0; 8 + 8 + 4 * SID_MAX_SUB_AUTHORITIES]; 64];
    let mut n = 0usize;
    let size_of = |ace: &[u8]| usize::from(u16::from_le_bytes([ace[2], ace[3]]));
    if !old.is_null() {
        for i in 0..u32::from(acl_word(old, 4)) {
            let mut ace = core::ptr::null_mut();
            if GetAce(old, i, &mut ace) == 0 || n == aces.len() {
                return ERROR_INVALID_ACL;
            }
            // SAFETY: an ACE inside the old ACL.
            let size = usize::from(unsafe { ace.add(2).cast::<u16>().read_unaligned() });
            if size > aces[0].len() {
                return ERROR_INVALID_ACL;
            }
            // SAFETY: as above, `size` bytes.
            unsafe { core::ptr::copy_nonoverlapping(ace, aces[n].as_mut_ptr(), size) };
            n += 1;
        }
    }
    for e in 0..count as usize {
        // SAFETY: the caller's array of `count` EXPLICIT_ACCESS_W.
        let (mask, mode, inherit, form, name) = unsafe {
            let p = entries.add(e * ENTRY);
            (
                p.cast::<u32>().read_unaligned(),
                p.add(4).cast::<u32>().read_unaligned(),
                p.add(8).cast::<u32>().read_unaligned(),
                p.add(16 + 12).cast::<u32>().read_unaligned(),
                p.add(16 + 24).cast::<*const u8>().read_unaligned(),
            )
        };
        let mut sid_buf = [0u8; 8 + 4 * SID_MAX_SUB_AUTHORITIES];
        let sid: *const u8 = match form {
            0 => name, // TRUSTEE_IS_SID
            1 => {
                // TRUSTEE_IS_NAME
                if !trustee_name_sid(name.cast(), &mut sid_buf) {
                    return ERROR_NONE_MAPPED;
                }
                sid_buf.as_ptr()
            }
            _ => return ERROR_INVALID_PARAMETER,
        };
        let Some((_, _, subs)) = read_sid(sid) else { return ERROR_INVALID_SID };
        let slen = sid_len(subs);
        // SAFETY: a valid SID of `slen` bytes.
        let sid_bytes = unsafe { core::slice::from_raw_parts(sid, slen) };
        let same = |ace: &[u8]| {
            let size = size_of(ace);
            size == 8 + slen && ace[0] <= 1 && &ace[8..size] == sid_bytes
        };
        // SET_ACCESS and REVOKE_ACCESS drop the trustee's ACEs.
        if mode == 2 || mode == 4 {
            let mut k = 0;
            for i in 0..n {
                if !same(&aces[i]) {
                    aces[k] = aces[i];
                    k += 1;
                }
            }
            n = k;
        }
        let kind = match mode {
            1 | 2 => 0u8, // ACCESS_ALLOWED_ACE_TYPE
            3 => 1u8,     // ACCESS_DENIED_ACE_TYPE
            4 => continue,
            _ => return ERROR_INVALID_PARAMETER,
        };
        if n == aces.len() {
            return ERROR_NOT_ENOUGH_MEMORY;
        }
        let mut ace = [0u8; 8 + 8 + 4 * SID_MAX_SUB_AUTHORITIES];
        ace[0] = kind;
        // The inheritance flags (OBJECT_INHERIT_ACE...) are AceFlags.
        ace[1] = (inherit & 0x1f) as u8;
        ace[2..4].copy_from_slice(&((8 + slen) as u16).to_le_bytes());
        ace[4..8].copy_from_slice(&mask.to_le_bytes());
        ace[8..8 + slen].copy_from_slice(sid_bytes);
        if kind == 1 {
            // Before the allowed ACEs.
            let at = (0..n).find(|&i| aces[i][0] != 1).unwrap_or(n);
            for i in (at..n).rev() {
                aces[i + 1] = aces[i];
            }
            aces[at] = ace;
        } else {
            aces[n] = ace;
        }
        n += 1;
    }
    let bytes: usize = 8 + aces[..n].iter().map(|a| size_of(a)).sum::<usize>();
    // SAFETY: a plain allocation.
    let acl = unsafe { LocalAlloc(0, bytes) };
    if acl.is_null() {
        return ERROR_NOT_ENOUGH_MEMORY;
    }
    InitializeAcl(acl, bytes as u32, 2);
    for a in &aces[..n] {
        if insert_aces(acl, u32::MAX, a.as_ptr(), size_of(a), 1) == 0 {
            return ERROR_INVALID_ACL;
        }
    }
    // SAFETY: the caller's output.
    unsafe { *new = acl };
    0
}

/// The SID of trustee name `name` ("CURRENT_USER" or an account
/// LookupAccountNameW knows) into `out`.
fn trustee_name_sid(name: *const u16, out: &mut [u8; 8 + 4 * SID_MAX_SUB_AUTHORITIES]) -> bool {
    if name.is_null() {
        return false;
    }
    let current = "CURRENT_USER";
    // SAFETY: the caller's NUL-terminated name.
    let is_current = unsafe {
        current.bytes().enumerate().all(|(i, b)| *name.add(i) == u16::from(b))
            && *name.add(current.len()) == 0
    };
    if is_current {
        write_sid(out.as_mut_ptr(), NT, &USER_SUBS);
        return true;
    }
    let mut domain = [0u16; 64];
    let (mut sid_len_v, mut domain_len, mut kind) = (out.len() as u32, domain.len() as u32, 0);
    LookupAccountNameW(
        core::ptr::null(),
        name,
        out.as_mut_ptr(),
        &mut sid_len_v,
        domain.as_mut_ptr(),
        &mut domain_len,
        &mut kind,
    ) != 0
}

/// GetSidIdentifierAuthority: the SID_IDENTIFIER_AUTHORITY inside the SID
/// (no checks, as documented).
#[unsafe(no_mangle)]
pub extern "system" fn GetSidIdentifierAuthority(sid: *mut u8) -> *mut [u8; 6] {
    sid.wrapping_add(2).cast()
}

/// GetSidSubAuthorityCount: the count byte inside the SID (no checks, as
/// documented; the caller passes a valid SID).
#[unsafe(no_mangle)]
pub extern "system" fn GetSidSubAuthorityCount(sid: *mut u8) -> *mut u8 {
    sid.wrapping_add(1)
}

/// GetSidSubAuthority: the subauthority's place inside the SID.
#[unsafe(no_mangle)]
pub extern "system" fn GetSidSubAuthority(sid: *mut u8, index: u32) -> *mut u32 {
    sid.wrapping_add(8 + 4 * index as usize).cast()
}

// ---- Software restriction policies (SAFER) ----
//
// gala has no policies, as a Windows machine without software restriction
// policies or AppLocker rules: every program and script is fully trusted.
// A level handle is the level id itself (only the fully trusted one is
// ever handed out).

const SAFER_LEVELID_FULLYTRUSTED: usize = 0x4_0000;

/// SaferIdentifyLevel: the level for the code described - fully trusted.
#[unsafe(no_mangle)]
pub extern "system" fn SaferIdentifyLevel(
    _count: u32,
    _properties: *mut u8,
    level: *mut usize,
    _reserved: *mut c_void,
) -> i32 {
    if level.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's output.
    unsafe { *level = SAFER_LEVELID_FULLYTRUSTED };
    1
}

/// SaferCreateLevel: the levels exist (handles name them).
#[unsafe(no_mangle)]
pub extern "system" fn SaferCreateLevel(
    _scope: u32,
    level_id: u32,
    _open_flags: u32,
    level: *mut usize,
    _reserved: *mut c_void,
) -> i32 {
    if level.is_null() || level_id == 0 {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's output.
    unsafe { *level = level_id as usize };
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn SaferCloseLevel(level: usize) -> i32 {
    if level == 0 {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    1
}

/// SaferGetLevelInformation: SaferObjectLevelId (1) only (a DWORD).
#[unsafe(no_mangle)]
pub extern "system" fn SaferGetLevelInformation(
    level: usize,
    class: u32,
    buffer: *mut u32,
    size: u32,
    needed: *mut u32,
) -> i32 {
    if level == 0 {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    if class != 1 {
        missing(b"advapi32.dll!SaferGetLevelInformation(class)");
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's outputs.
    unsafe {
        if !needed.is_null() {
            *needed = 4;
        }
        if buffer.is_null() || size < 4 {
            return fail(ERROR_INSUFFICIENT_BUFFER, 0);
        }
        *buffer = level as u32;
    }
    1
}

/// SaferComputeTokenFromLevel: a fully trusted level restricts nothing:
/// with SAFER_TOKEN_NULL_IF_EQUAL (1) no token, else a primary copy of
/// the token given (null: the caller's process token).
#[unsafe(no_mangle)]
pub extern "system" fn SaferComputeTokenFromLevel(
    level: usize,
    token: usize,
    out: *mut usize,
    flags: u32,
    _reserved: *mut c_void,
) -> i32 {
    if level == 0 || out.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    if flags & 1 != 0 {
        // SAFETY: the caller's output.
        unsafe { *out = 0 };
        return 1;
    }
    super::token::primary_copy(token, out)
}

/// CreateWellKnownSid: the documented well-known SIDs that need no domain
/// (WELL_KNOWN_SID_TYPE values); others are recorded.
/// IsWellKnownSid: whether `sid` is the well-known SID of `kind` (as
/// CreateWellKnownSid makes it; domain-relative kinds never match here).
#[unsafe(no_mangle)]
pub extern "system" fn IsWellKnownSid(sid: *const u8, kind: u32) -> i32 {
    let Some((_, _, n)) = read_sid(sid) else { return 0 };
    let mut known = [0u8; 8 + 4 * SID_MAX_SUB_AUTHORITIES];
    let mut size = known.len() as u32;
    if CreateWellKnownSid(kind, core::ptr::null(), known.as_mut_ptr(), &mut size) == 0 {
        return 0;
    }
    let len = sid_len(n);
    // SAFETY: a valid SID of `len` bytes.
    let a = unsafe { core::slice::from_raw_parts(sid, len) };
    i32::from(size as usize == len && a == &known[..len])
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateWellKnownSid(
    kind: u32,
    _domain: *const u8,
    sid: *mut u8,
    size: *mut u32,
) -> i32 {
    const NT: [u8; 6] = [0, 0, 0, 0, 0, 5];
    let auth = |a: u8| [0, 0, 0, 0, 0, a];
    let (authority, subs): ([u8; 6], &[u32]) = match kind {
        0 => (auth(0), &[0]),        // WinNullSid S-1-0-0
        1 => (auth(1), &[0]),        // WinWorldSid S-1-1-0
        2 => (auth(2), &[0]),        // WinLocalSid S-1-2-0
        3 => (auth(3), &[0]),        // WinCreatorOwnerSid S-1-3-0
        4 => (auth(3), &[1]),        // WinCreatorGroupSid S-1-3-1
        7 => (NT, &[]),              // WinNtAuthoritySid S-1-5
        8 => (NT, &[1]),             // WinDialupSid
        9 => (NT, &[2]),             // WinNetworkSid
        10 => (NT, &[3]),            // WinBatchSid
        11 => (NT, &[4]),            // WinInteractiveSid
        12 => (NT, &[6]),            // WinServiceSid
        13 => (NT, &[7]),            // WinAnonymousSid
        17 => (NT, &[11]),           // WinAuthenticatedUserSid
        18 => (NT, &[12]),           // WinRestrictedCodeSid
        19 => (NT, &[13]),           // WinTerminalServerSid
        20 => (NT, &[14]),           // WinRemoteLogonIdSid
        22 => (NT, &[18]),           // WinLocalSystemSid
        23 => (NT, &[19]),           // WinLocalServiceSid
        24 => (NT, &[20]),           // WinNetworkServiceSid
        25 => (NT, &[32]),           // WinBuiltinDomainSid
        26 => (NT, &[32, 544]),      // WinBuiltinAdministratorsSid
        27 => (NT, &[32, 545]),      // WinBuiltinUsersSid
        28 => (NT, &[32, 546]),      // WinBuiltinGuestsSid
        29 => (NT, &[32, 547]),      // WinBuiltinPowerUsersSid
        30 => (NT, &[32, 548]),      // WinBuiltinAccountOperatorsSid
        31 => (NT, &[32, 549]),      // WinBuiltinSystemOperatorsSid
        32 => (NT, &[32, 550]),      // WinBuiltinPrintOperatorsSid
        33 => (NT, &[32, 551]),      // WinBuiltinBackupOperatorsSid
        34 => (NT, &[32, 552]),      // WinBuiltinReplicatorSid
        35 => (NT, &[32, 554]),      // WinBuiltinPreWindows2000CompatibleAccessSid
        36 => (NT, &[32, 555]),      // WinBuiltinRemoteDesktopUsersSid
        37 => (NT, &[32, 556]),      // WinBuiltinNetworkConfigurationOperatorsSid
        51 => (NT, &[64, 10]),       // WinNTLMAuthenticationSid
        52 => (NT, &[64, 21]),       // WinDigestAuthenticationSid
        53 => (NT, &[64, 14]),       // WinSChannelAuthenticationSid
        54 => (NT, &[15]),           // WinThisOrganizationSid
        55 => (NT, &[1000]),         // WinOtherOrganizationSid
        56 => (NT, &[32, 557]),      // WinBuiltinIncomingForestTrustBuildersSid
        57 => (NT, &[32, 558]),      // WinBuiltinPerfMonitoringUsersSid
        58 => (NT, &[32, 559]),      // WinBuiltinPerfLoggingUsersSid
        59 => (NT, &[32, 560]),      // WinBuiltinAuthorizationAccessSid
        60 => (NT, &[32, 561]),      // WinBuiltinTerminalServerLicenseServersSid
        61 => (NT, &[32, 562]),      // WinBuiltinDCOMUsersSid
        62 => (NT, &[32, 568]),      // WinBuiltinIUsersSid
        63 => (NT, &[17]),           // WinIUserSid
        64 => (NT, &[32, 569]),      // WinBuiltinCryptoOperatorsSid
        65 => (auth(16), &[0]),      // WinUntrustedLabelSid
        66 => (auth(16), &[0x1000]), // WinLowLabelSid
        67 => (auth(16), &[0x2000]), // WinMediumLabelSid
        68 => (auth(16), &[0x3000]), // WinHighLabelSid
        69 => (auth(16), &[0x4000]), // WinSystemLabelSid
        70 => (NT, &[33]),           // WinWriteRestrictedCodeSid
        71 => (auth(3), &[4]),       // WinCreatorOwnerRightsSid
        76 => (NT, &[32, 573]),      // WinBuiltinEventLogReadersGroup
        78 => (NT, &[32, 574]),      // WinBuiltinCertSvcDComAccessGroup
        79 => (auth(16), &[0x2100]), // WinMediumPlusLabelSid
        _ => {
            super::missing_detail(b"advapi32.dll!CreateWellKnownSid(type)", kind.into());
            return fail(ERROR_INVALID_PARAMETER, 0);
        }
    };
    if size.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let need = sid_len(subs.len()) as u32;
    // SAFETY: the caller's size and buffer of that many bytes.
    unsafe {
        let have = *size;
        *size = need;
        if sid.is_null() || have < need {
            return fail(ERROR_INSUFFICIENT_BUFFER, 0);
        }
    }
    write_sid(sid, authority, subs);
    1
}

// ---- SDDL ----
//
// ConvertStringSecurityDescriptorToSecurityDescriptorW/A, from the
// documented "Security Descriptor String Format": O:, G:, D: and S:
// parts; ACL flags P (protected), AI (auto-inherited), AR (inheritance
// required) and NO_ACCESS_CONTROL (a NULL DACL); simple ACEs
// (A, D, AU, AL, ML) with their flags, rights (abbreviations or numbers)
// and trustee (an abbreviation or an S-1-... string). Object ACEs (OA,
// OD...) and conditional ones are not supported (recorded). The result is
// a self-relative descriptor from LocalAlloc.

const SE_SACL_PRESENT: u16 = 0x10;

/// SID abbreviations: (text, authority, sub-authorities).
const SDDL_SIDS: &[(&str, u8, &[u32])] = &[
    ("AN", 5, &[7]),
    ("AO", 5, &[32, 548]),
    ("AU", 5, &[11]),
    ("BA", 5, &[32, 544]),
    ("BG", 5, &[32, 546]),
    ("BO", 5, &[32, 551]),
    ("BU", 5, &[32, 545]),
    ("CG", 3, &[1]),
    ("CO", 3, &[0]),
    ("ED", 5, &[9]),
    ("IU", 5, &[4]),
    ("LS", 5, &[19]),
    ("NO", 5, &[32, 556]),
    ("NS", 5, &[20]),
    ("NU", 5, &[2]),
    ("OW", 3, &[4]),
    ("PO", 5, &[32, 550]),
    ("PS", 5, &[10]),
    ("PU", 5, &[32, 547]),
    ("RC", 5, &[12]),
    ("RD", 5, &[32, 555]),
    ("RE", 5, &[32, 552]),
    ("RU", 5, &[32, 554]),
    ("SO", 5, &[32, 549]),
    ("SU", 5, &[6]),
    ("SY", 5, &[18]),
    ("WD", 1, &[0]),
    ("LW", 16, &[4096]),
    ("ME", 16, &[8192]),
    ("HI", 16, &[12288]),
    ("SI", 16, &[16384]),
    ("AC", 15, &[2, 1]),
];

/// Access right abbreviations.
const SDDL_RIGHTS: &[(&str, u32)] = &[
    ("GA", 0x1000_0000),
    ("GR", 0x8000_0000),
    ("GW", 0x4000_0000),
    ("GX", 0x2000_0000),
    ("RC", 0x2_0000),
    ("SD", 0x1_0000),
    ("WD", 0x4_0000),
    ("WO", 0x8_0000),
    ("RP", 0x10),
    ("WP", 0x20),
    ("CC", 0x1),
    ("DC", 0x2),
    ("LC", 0x4),
    ("SW", 0x8),
    ("LO", 0x80),
    ("DT", 0x40),
    ("CR", 0x100),
    ("FA", 0x1f_01ff),
    ("FR", 0x12_0089),
    ("FW", 0x12_0116),
    ("FX", 0x12_00a0),
    ("KA", 0xf_003f),
    ("KR", 0x2_0019),
    ("KW", 0x2_0006),
    ("KX", 0x2_0019),
    ("NR", 0x1),
    ("NW", 0x2),
    ("NX", 0x4),
];

/// A SID in a fixed buffer.
#[derive(Clone, Copy)]
struct SidBuf {
    bytes: [u8; 8 + 4 * SID_MAX_SUB_AUTHORITIES],
    len: usize,
}

impl SidBuf {
    fn new(authority: u64, subs: &[u32]) -> SidBuf {
        let mut b =
            SidBuf { bytes: [0; 8 + 4 * SID_MAX_SUB_AUTHORITIES], len: sid_len(subs.len()) };
        let a = authority.to_be_bytes();
        write_sid(b.bytes.as_mut_ptr(), [a[2], a[3], a[4], a[5], a[6], a[7]], subs);
        b
    }
    fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// Parses a trustee: an abbreviation (two letters) or S-1-...; the rest.
fn sddl_sid(t: &[u8]) -> Option<(SidBuf, &[u8])> {
    if t.len() >= 2 && t[0] == b'S' && t.get(1) == Some(&b'-') {
        let end = t
            .iter()
            .position(|&c| !(c.is_ascii_digit() || c == b'-' || c == b'S'))
            .unwrap_or(t.len());
        let text = &t[..end];
        let mut parts = text.split(|&c| c == b'-');
        if parts.next() != Some(b"S") || parts.next() != Some(b"1") {
            return None;
        }
        let num = |p: &[u8]| -> Option<u64> {
            if p.is_empty() {
                return None;
            }
            p.iter().try_fold(0u64, |a, &c| a.checked_mul(10)?.checked_add(u64::from(c - b'0')))
        };
        let authority = num(parts.next()?)?;
        let mut subs = [0u32; SID_MAX_SUB_AUTHORITIES];
        let mut n = 0;
        for p in parts {
            if n == SID_MAX_SUB_AUTHORITIES {
                return None;
            }
            subs[n] = u32::try_from(num(p)?).ok()?;
            n += 1;
        }
        return Some((SidBuf::new(authority, &subs[..n]), &t[end..]));
    }
    let code = t.get(..2)?;
    if code == b"LA" || code == b"LG" {
        // The machine's administrator and guest (RIDs 500, 501).
        let mut subs = USER_SUBS;
        subs[4] = if code == b"LA" { 500 } else { 501 };
        return Some((SidBuf::new(5, &subs), &t[2..]));
    }
    let &(_, auth, subs) = SDDL_SIDS.iter().find(|(name, _, _)| name.as_bytes() == code)?;
    Some((SidBuf::new(u64::from(auth), subs), &t[2..]))
}

/// The access mask of an ACE's rights field.
fn sddl_rights(t: &[u8]) -> Option<u32> {
    if t.starts_with(b"0x") || t.starts_with(b"0X") {
        return u32::from_str_radix(core::str::from_utf8(&t[2..]).ok()?, 16).ok();
    }
    if !t.is_empty() && t.iter().all(u8::is_ascii_digit) {
        return core::str::from_utf8(t).ok()?.parse().ok();
    }
    let mut mask = 0;
    for pair in t.chunks(2) {
        mask |= SDDL_RIGHTS.iter().find(|(n, _)| n.as_bytes() == pair)?.1;
    }
    Some(mask)
}

/// An ACL being built: its ACEs, back to back.
struct AclBuf {
    bytes: [u8; 4096],
    len: usize,
    count: u16,
    null: bool,
}

/// Parses ACL flags and ACEs (after "D:" or "S:"); returns the control
/// bits (protected, auto-inherited, inheritance required) and the rest.
fn sddl_acl<'a>(mut t: &'a [u8], acl: &mut AclBuf, sacl: bool) -> Option<(u16, &'a [u8])> {
    let mut control = 0u16;
    loop {
        if t.starts_with(b"NO_ACCESS_CONTROL") {
            acl.null = true;
            t = &t[17..];
        } else if t.starts_with(b"AI") {
            control |= if sacl { 0x800 } else { 0x400 };
            t = &t[2..];
        } else if t.starts_with(b"AR") {
            control |= if sacl { 0x200 } else { 0x100 };
            t = &t[2..];
        } else if t.first() == Some(&b'P') {
            control |= if sacl { 0x2000 } else { 0x1000 };
            t = &t[1..];
        } else {
            break;
        }
    }
    while t.first() == Some(&b'(') {
        let end = t.iter().position(|&c| c == b')')?;
        let body = &t[1..end];
        t = &t[end + 1..];
        let mut f = body.split(|&c| c == b';');
        let kind = match f.next()? {
            b"A" => 0u8,
            b"D" => 1,
            b"AU" => 2,
            b"AL" => 3,
            b"ML" => 0x11,
            other => {
                let _ = other;
                missing(b"advapi32.dll!SDDL ACE type");
                return None;
            }
        };
        let flags_text = f.next()?;
        let mut flags = 0u8;
        for pair in flags_text.chunks(2) {
            flags |= match pair {
                b"OI" => 0x01,
                b"CI" => 0x02,
                b"NP" => 0x04,
                b"IO" => 0x08,
                b"ID" => 0x10,
                b"SA" => 0x40,
                b"FA" => 0x80,
                _ => return None,
            };
        }
        let mask = sddl_rights(f.next()?)?;
        let (object, inherit) = (f.next()?, f.next()?);
        if !object.is_empty() || !inherit.is_empty() {
            missing(b"advapi32.dll!SDDL object ACE");
            return None;
        }
        let (sid, rest) = sddl_sid(f.next()?)?;
        if !rest.is_empty() || f.next().is_some() {
            return None;
        }
        let size = 8 + sid.len;
        if acl.len + size > acl.bytes.len() {
            return None;
        }
        let a = &mut acl.bytes[acl.len..acl.len + size];
        a[0] = kind;
        a[1] = flags;
        a[2..4].copy_from_slice(&(size as u16).to_le_bytes());
        a[4..8].copy_from_slice(&mask.to_le_bytes());
        a[8..].copy_from_slice(sid.bytes());
        acl.len += size;
        acl.count += 1;
    }
    Some((control, t))
}

#[unsafe(no_mangle)]
pub extern "system" fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
    text: *const u16,
    revision: u32,
    out: *mut *mut u8,
    size: *mut u32,
) -> i32 {
    if text.is_null() || out.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    if revision != 1 {
        return fail(1305, 0); // ERROR_UNKNOWN_REVISION
    }
    // ASCII copy without blanks (SDDL is ASCII).
    let mut chars = [0u8; 4096];
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    unsafe {
        let mut i = 0;
        loop {
            let c = *text.add(i);
            if c == 0 {
                break;
            }
            if c > 0x7f || n == chars.len() {
                return fail(ERROR_INVALID_PARAMETER, 0);
            }
            if c != u16::from(b' ') {
                chars[n] = c as u8;
                n += 1;
            }
            i += 1;
        }
    }
    sddl_to_descriptor(&chars[..n], out, size)
}

#[unsafe(no_mangle)]
pub extern "system" fn ConvertStringSecurityDescriptorToSecurityDescriptorA(
    text: *const u8,
    revision: u32,
    out: *mut *mut u8,
    size: *mut u32,
) -> i32 {
    if text.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let mut wide = [0u16; 4096];
    // SAFETY: the caller's NUL-terminated string.
    unsafe {
        let mut i = 0;
        while *text.add(i) != 0 {
            if i == wide.len() - 1 {
                return fail(ERROR_INVALID_PARAMETER, 0);
            }
            wide[i] = u16::from(*text.add(i));
            i += 1;
        }
    }
    ConvertStringSecurityDescriptorToSecurityDescriptorW(wide.as_ptr(), revision, out, size)
}

fn sddl_to_descriptor(mut t: &[u8], out: *mut *mut u8, size: *mut u32) -> i32 {
    let (mut owner, mut group) = (None, None);
    let mut dacl = AclBuf { bytes: [0; 4096], len: 0, count: 0, null: false };
    let mut sacl = AclBuf { bytes: [0; 4096], len: 0, count: 0, null: false };
    let (mut has_dacl, mut has_sacl) = (false, false);
    let mut control: u16 = SE_SELF_RELATIVE;
    while !t.is_empty() {
        let tag = t.get(..2);
        t = &t[2.min(t.len())..];
        let parsed = match tag {
            Some(b"O:") => sddl_sid(t).map(|(s, r)| {
                owner = Some(s);
                r
            }),
            Some(b"G:") => sddl_sid(t).map(|(s, r)| {
                group = Some(s);
                r
            }),
            Some(b"D:") => sddl_acl(t, &mut dacl, false).map(|(c, r)| {
                has_dacl = true;
                control |= c;
                r
            }),
            Some(b"S:") => sddl_acl(t, &mut sacl, true).map(|(c, r)| {
                has_sacl = true;
                control |= c;
                r
            }),
            _ => None,
        };
        match parsed {
            Some(rest) => t = rest,
            None => return fail(ERROR_INVALID_PARAMETER, 0),
        }
    }
    // Layout: header, owner, group, SACL, DACL.
    let mut total = 20;
    let acl_size = |a: &AclBuf| if a.null { 0 } else { 8 + a.len };
    total += owner.map_or(0, |s| s.len) + group.map_or(0, |s| s.len);
    if has_sacl {
        total += acl_size(&sacl);
    }
    if has_dacl {
        total += acl_size(&dacl);
    }
    // SAFETY: a plain allocation (zeroed: LMEM_ZEROINIT).
    let p = unsafe { LocalAlloc(0x40, total) };
    if p.is_null() {
        return fail(ERROR_NOT_ENOUGH_MEMORY, 0);
    }
    // SAFETY: `p` has `total` zeroed bytes laid out as computed.
    unsafe {
        let mut at = 20usize;
        let put = |bytes: &[u8], field: usize, at: &mut usize| {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), p.add(*at), bytes.len());
            p.add(field).cast::<u32>().write_unaligned(*at as u32);
            *at += bytes.len();
        };
        if let Some(s) = owner {
            put(s.bytes(), 4, &mut at);
        }
        if let Some(s) = group {
            put(s.bytes(), 8, &mut at);
        }
        let write_acl = |a: &AclBuf, field: usize, at: &mut usize| {
            if a.null {
                return;
            }
            let base = p.add(*at);
            *base = 2; // ACL_REVISION
            base.add(2).cast::<u16>().write_unaligned((8 + a.len) as u16);
            base.add(4).cast::<u16>().write_unaligned(a.count);
            core::ptr::copy_nonoverlapping(a.bytes.as_ptr(), base.add(8), a.len);
            p.add(field).cast::<u32>().write_unaligned(*at as u32);
            *at += 8 + a.len;
        };
        if has_sacl {
            control |= SE_SACL_PRESENT;
            write_acl(&sacl, 12, &mut at);
        }
        if has_dacl {
            control |= SE_DACL_PRESENT;
            write_acl(&dacl, 16, &mut at);
        }
        *p = 1;
        p.add(2).cast::<u16>().write_unaligned(control);
        *out = p;
        if !size.is_null() {
            *size = total as u32;
        }
    }
    1
}

/// GetSecurityDescriptorGroup: as GetSecurityDescriptorOwner (group
/// pointer at 16 absolute, offset at 8 self-relative; SE_GROUP_DEFAULTED).
#[unsafe(no_mangle)]
pub extern "system" fn GetSecurityDescriptorGroup(
    sd: *const u8,
    group: *mut *mut u8,
    defaulted: *mut i32,
) -> i32 {
    if IsValidSecurityDescriptor(sd) == 0 || group.is_null() {
        return fail(ERROR_INVALID_SECURITY_DESCR, 0);
    }
    let c = control(sd);
    // SAFETY: the caller's descriptor and outputs.
    unsafe {
        *group = if c & SE_SELF_RELATIVE != 0 {
            let off = sd.add(8).cast::<u32>().read_unaligned() as usize;
            if off == 0 { core::ptr::null_mut() } else { sd.add(off).cast_mut() }
        } else {
            sd.add(16).cast::<*mut u8>().read_unaligned()
        };
        if !defaulted.is_null() {
            *defaulted = i32::from(c & 2 != 0);
        }
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn GetSecurityDescriptorControl(
    sd: *const u8,
    out: *mut u16,
    revision: *mut u32,
) -> i32 {
    if IsValidSecurityDescriptor(sd) == 0 || out.is_null() || revision.is_null() {
        return fail(ERROR_INVALID_SECURITY_DESCR, 0);
    }
    // SAFETY: the caller's descriptor and outputs.
    unsafe {
        *out = control(sd);
        *revision = u32::from(*sd);
    }
    1
}

/// GetSecurityDescriptorLength: a self-relative descriptor's bytes up to
/// the end of its last part; an absolute one's header (40).
#[unsafe(no_mangle)]
pub extern "system" fn GetSecurityDescriptorLength(sd: *const u8) -> u32 {
    if IsValidSecurityDescriptor(sd) == 0 {
        return 0;
    }
    if control(sd) & SE_SELF_RELATIVE == 0 {
        return 40;
    }
    let mut end = 20usize;
    // SAFETY: the offsets inside the caller's self-relative descriptor.
    unsafe {
        for field in [4usize, 8] {
            let off = sd.add(field).cast::<u32>().read_unaligned() as usize;
            if off != 0 {
                if let Some((_, _, n)) = read_sid(sd.add(off)) {
                    end = end.max(off + sid_len(n));
                }
            }
        }
        for field in [12usize, 16] {
            let off = sd.add(field).cast::<u32>().read_unaligned() as usize;
            if off != 0 {
                let size = usize::from(sd.add(off + 2).cast::<u16>().read_unaligned());
                end = end.max(off + size);
            }
        }
    }
    end as u32
}
