//! Security structures (SIDs, ACLs, security descriptors) built and read
//! as documented, the accounts gala knows, and the process token.
//!
//! gala has one user, "gala" (machine GALA), and enforces no security:
//! SIDs, ACLs and descriptors are real data (programs build and read
//! them), but files keep no descriptors (SetFileSecurity succeeds and is
//! recorded). The process token is a stand-in: OpenProcessToken returns a
//! duplicate of the process handle (so CloseHandle works) and the token
//! functions describe the one user, a standard (not elevated) user with
//! the privileges such a user holds.

use core::ffi::c_void;

use super::{ERROR_INSUFFICIENT_BUFFER, fail, missing};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LocalAlloc(flags: u32, bytes: usize) -> *mut u8;
    fn GetCurrentProcess() -> usize;
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn DuplicateHandle(
        source_process: usize,
        source: usize,
        target_process: usize,
        target: *mut usize,
        access: u32,
        inherit: i32,
        options: u32,
    ) -> i32;
}

const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_SID: u32 = 1337;
const ERROR_NONE_MAPPED: u32 = 1332;
const ERROR_ALLOTTED_SPACE_EXCEEDED: u32 = 1344;
const ERROR_INVALID_ACL: u32 = 1336;
const ERROR_NO_SUCH_PRIVILEGE: u32 = 1313;
const ERROR_NOT_ALL_ASSIGNED: u32 = 1300;
const ERROR_INVALID_SECURITY_DESCR: u32 = 1338;
const ERROR_INVALID_HANDLE: u32 = 6;

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
    (5, &[18], "SYSTEM", "NT AUTHORITY", SID_TYPE_WELL_KNOWN_GROUP),
    (5, &[32, 544], "Administrators", "BUILTIN", SID_TYPE_ALIAS),
    (5, &[32, 545], "Users", "BUILTIN", SID_TYPE_ALIAS),
];

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
    let Some(&(_, _, account, dom, use_)) =
        ACCOUNTS.iter().find(|a| a.0 == authority && a.1 == &subs[..count])
    else {
        return fail(ERROR_NONE_MAPPED, 0);
    };
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
        put_name(account, name);
        put_name(dom, domain);
        *name_len = need_name - 1;
        *domain_len = need_domain - 1;
        if !kind.is_null() {
            *kind = use_;
        }
    }
    1
}

/// GetUserNameW: "gala"; the size includes the NUL.
#[unsafe(no_mangle)]
pub extern "system" fn GetUserNameW(buffer: *mut u16, size: *mut u32) -> i32 {
    if size.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let needed = super::USER_NAME.len() as u32 + 1;
    // SAFETY: the caller's size and buffer.
    unsafe {
        if buffer.is_null() || *size < needed {
            *size = needed;
            return fail(ERROR_INSUFFICIENT_BUFFER, 0);
        }
        for (i, &b) in super::USER_NAME.iter().enumerate() {
            *buffer.add(i) = b.into();
        }
        *buffer.add(super::USER_NAME.len()) = 0;
        *size = needed;
    }
    1
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

/// SetFileSecurityA/W: gala's files keep no security descriptors; the
/// call succeeds (recorded) and nothing is enforced.
#[unsafe(no_mangle)]
pub extern "system" fn SetFileSecurityA(path: *const u8, _info: u32, sd: *const u8) -> i32 {
    if path.is_null() || IsValidSecurityDescriptor(sd) == 0 {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    missing(b"advapi32.dll!SetFileSecurityA(not kept)");
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn SetFileSecurityW(path: *const u16, _info: u32, sd: *const u8) -> i32 {
    if path.is_null() || IsValidSecurityDescriptor(sd) == 0 {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    missing(b"advapi32.dll!SetFileSecurityW(not kept)");
    1
}

// ---- The process token ----

/// OpenProcessToken: the token handle is a duplicate of the process
/// handle (see the module comment).
#[unsafe(no_mangle)]
pub extern "system" fn OpenProcessToken(process: usize, _access: u32, token: *mut usize) -> i32 {
    if token.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: plain calls; DUPLICATE_SAME_ACCESS (2).
    unsafe {
        let me = GetCurrentProcess();
        if DuplicateHandle(me, process, me, token, 0, 0, 2) == 0 {
            return 0;
        }
    }
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn OpenThreadToken(
    _thread: usize,
    _access: u32,
    _as_self: i32,
    _token: *mut usize,
) -> i32 {
    fail(1008, 0) // ERROR_NO_TOKEN: threads do not impersonate
}

/// Privileges a standard user holds (LUIDs): SeShutdown (19),
/// SeChangeNotify (23, enabled), SeUndock (25), SeIncreaseWorkingSet (33),
/// SeTimeZone (34).
const HELD: [u32; 5] = [19, 23, 25, 33, 34];

/// Writes `bytes` to the caller's buffer of `length`; ERROR_INSUFFICIENT_BUFFER
/// if it is short. The size is returned in `ret` either way.
fn token_info(out: *mut u8, length: u32, ret: *mut u32, bytes: &[u8]) -> i32 {
    if !ret.is_null() {
        // SAFETY: the caller's output.
        unsafe { *ret = bytes.len() as u32 };
    }
    if out.is_null() || (length as usize) < bytes.len() {
        return fail(ERROR_INSUFFICIENT_BUFFER, 0);
    }
    // SAFETY: the caller's buffer of `length` >= bytes.len().
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
    1
}

/// GetTokenInformation: TokenUser (1), TokenOwner (4), TokenPrimaryGroup
/// (5): the user's SID after the structure; TokenPrivileges (3): the
/// held privileges; TokenElevationType (18): default (1); TokenElevation
/// (20): not elevated. Other classes are reported.
#[unsafe(no_mangle)]
pub extern "system" fn GetTokenInformation(
    token: usize,
    class: u32,
    out: *mut u8,
    length: u32,
    ret: *mut u32,
) -> i32 {
    if token == 0 {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    let mut b = [0u8; 128];
    let n = match class {
        1 | 4 | 5 => {
            // A pointer (and for TOKEN_USER the attributes), then the SID.
            let head = if class == 1 { 16 } else { 8 };
            write_sid(b[head..].as_mut_ptr(), NT, &USER_SUBS);
            let sid = (out as usize).wrapping_add(head) as u64;
            b[..8].copy_from_slice(&sid.to_le_bytes());
            head + sid_len(USER_SUBS.len())
        }
        3 => {
            // TOKEN_PRIVILEGES: count, then LUID_AND_ATTRIBUTES.
            b[..4].copy_from_slice(&(HELD.len() as u32).to_le_bytes());
            for (i, &luid) in HELD.iter().enumerate() {
                let at = 4 + 12 * i;
                b[at..at + 4].copy_from_slice(&luid.to_le_bytes());
                let enabled: u32 = if luid == 23 { 3 } else { 0 }; // ENABLED_BY_DEFAULT | ENABLED
                b[at + 8..at + 12].copy_from_slice(&enabled.to_le_bytes());
            }
            4 + 12 * HELD.len()
        }
        18 => {
            b[..4].copy_from_slice(&1u32.to_le_bytes());
            4
        }
        20 => 4,
        _ => {
            missing(b"advapi32.dll!GetTokenInformation(class)");
            return fail(ERROR_INVALID_PARAMETER, 0);
        }
    };
    token_info(out, length, ret, &b[..n])
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

/// AdjustTokenPrivileges: succeeds; ERROR_NOT_ALL_ASSIGNED (as the last
/// error) when a privilege the user does not hold is asked for, else 0.
/// The previous state is reported as no privileges changed.
#[unsafe(no_mangle)]
pub extern "system" fn AdjustTokenPrivileges(
    token: usize,
    disable_all: i32,
    new: *const u32,
    length: u32,
    previous: *mut u32,
    ret: *mut u32,
) -> i32 {
    if token == 0 {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    if !previous.is_null() {
        if length < 4 {
            return fail(ERROR_INSUFFICIENT_BUFFER, 0);
        }
        // SAFETY: the caller's TOKEN_PRIVILEGES of `length` bytes.
        unsafe { *previous = 0 };
    }
    if !ret.is_null() {
        // SAFETY: the caller's output.
        unsafe { *ret = 4 };
    }
    let mut all = true;
    if disable_all == 0 && !new.is_null() {
        // SAFETY: the caller's TOKEN_PRIVILEGES: count, then 12-byte
        // LUID_AND_ATTRIBUTES.
        unsafe {
            for i in 0..*new as usize {
                let luid = *new.add(1 + 3 * i);
                all &= HELD.contains(&luid);
            }
        }
    }
    fail(if all { 0 } else { ERROR_NOT_ALL_ASSIGNED }, 1)
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
    let Some(&(authority, subs, _, dom, use_)) =
        ACCOUNTS.iter().find(|a| same(account, a.2) && want_domain.is_none_or(|d| same(d, a.3)))
    else {
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

/// GetFileSecurityW: gala's files keep no security descriptors; the one
/// returned (self-relative) is what a file created by the user has: the
/// user as owner and primary group, and a DACL granting the user,
/// Administrators and SYSTEM full access (FILE_ALL_ACCESS). Sizes as
/// documented: ERROR_INSUFFICIENT_BUFFER with the size needed.
#[unsafe(no_mangle)]
pub extern "system" fn GetFileSecurityW(
    path: *const u16,
    info: u32,
    sd: *mut u8,
    length: u32,
    needed: *mut u32,
) -> i32 {
    // SAFETY: a plain call.
    if path.is_null() || unsafe { GetFileAttributesW(path) } == u32::MAX {
        return 0; // the file's own error (ERROR_FILE_NOT_FOUND...)
    }
    // Build it: header (20), owner, group, DACL.
    let mut b = [0u8; 256];
    let mut at = 20usize;
    let mut control_bits = SE_SELF_RELATIVE;
    let user = |p: *mut u8| write_sid(p, NT, &USER_SUBS);
    let user_len = sid_len(USER_SUBS.len());
    if info & 1 != 0 {
        user(b[at..].as_mut_ptr());
        b[4..8].copy_from_slice(&(at as u32).to_le_bytes());
        at += user_len;
    }
    if info & 2 != 0 {
        user(b[at..].as_mut_ptr());
        b[8..12].copy_from_slice(&(at as u32).to_le_bytes());
        at += user_len;
    }
    if info & 4 != 0 {
        let acl = b[at..].as_mut_ptr();
        InitializeAcl(acl, 128, 2);
        let mut sid = [0u8; 28];
        for (auth, subs) in [(NT, &USER_SUBS[..]), (NT, &[32, 544][..]), (NT, &[18][..])] {
            write_sid(sid.as_mut_ptr(), auth, subs);
            add_simple_ace(acl, 0, 0, 0x1f_01ff, sid.as_ptr());
        }
        let used = acl_used(acl).unwrap_or(8);
        set_acl_word(acl, 2, used as u16);
        b[16..20].copy_from_slice(&(at as u32).to_le_bytes());
        at += used;
        control_bits |= SE_DACL_PRESENT;
    }
    b[0] = 1;
    b[2..4].copy_from_slice(&control_bits.to_le_bytes());
    if !needed.is_null() {
        // SAFETY: the caller's output.
        unsafe { *needed = at as u32 };
    }
    if sd.is_null() || (length as usize) < at {
        return fail(ERROR_INSUFFICIENT_BUFFER, 0);
    }
    // SAFETY: the caller's buffer of `length` >= at bytes.
    unsafe { core::ptr::copy_nonoverlapping(b.as_ptr(), sd, at) };
    1
}

/// DuplicateTokenEx: another handle on the stand-in token (any level and
/// type).
#[unsafe(no_mangle)]
pub extern "system" fn DuplicateTokenEx(
    token: usize,
    _access: u32,
    _attributes: *mut c_void,
    _level: u32,
    _kind: u32,
    new: *mut usize,
) -> i32 {
    if new.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: plain calls; DUPLICATE_SAME_ACCESS (2).
    unsafe {
        let me = GetCurrentProcess();
        if DuplicateHandle(me, token, me, new, 0, 0, 2) == 0 {
            return 0;
        }
    }
    1
}

/// SetThreadToken: impersonating the one user is being that user; a
/// null token reverts. (Threads keep no tokens here.)
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadToken(_thread: *mut usize, _token: usize) -> i32 {
    1
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

/// RevertToSelf: threads never impersonate here (see SetThreadToken).
#[unsafe(no_mangle)]
pub extern "system" fn RevertToSelf() -> i32 {
    1
}

/// SetKernelObjectSecurity: objects keep no security descriptors here;
/// a valid one is accepted (recorded) and nothing is enforced.
#[unsafe(no_mangle)]
pub extern "system" fn SetKernelObjectSecurity(handle: usize, _info: u32, sd: *const u8) -> i32 {
    if handle == 0 {
        return fail(ERROR_INVALID_HANDLE, 0);
    }
    if IsValidSecurityDescriptor(sd) == 0 {
        return fail(ERROR_INVALID_SECURITY_DESCR, 0);
    }
    missing(b"advapi32.dll!SetKernelObjectSecurity(not kept)");
    1
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
/// with SAFER_TOKEN_NULL_IF_EQUAL (1) no token, else a copy of the
/// caller's (the stand-in token: a process handle duplicate).
#[unsafe(no_mangle)]
pub extern "system" fn SaferComputeTokenFromLevel(
    level: usize,
    _token: usize,
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
    // SAFETY: plain call (the current process pseudo-handle).
    OpenProcessToken(unsafe { GetCurrentProcess() }, 0, out)
}

/// CreateWellKnownSid: the documented well-known SIDs that need no domain
/// (WELL_KNOWN_SID_TYPE values); others are recorded.
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
        66 => (auth(16), &[0x1000]), // WinLowLabelSid
        67 => (auth(16), &[0x2000]), // WinMediumLabelSid
        68 => (auth(16), &[0x3000]), // WinHighLabelSid
        69 => (auth(16), &[0x4000]), // WinSystemLabelSid
        _ => {
            missing(b"advapi32.dll!CreateWellKnownSid(type)");
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
