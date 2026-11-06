//! Trustees and explicit access entries (aclapi.h: BuildTrusteeWithSidW,
//! BuildTrusteeWithNameW, BuildExplicitAccessWithNameW), descriptors built
//! from them (BuildSecurityDescriptorW), GetSecurityDescriptorSacl and
//! mandatory label ACEs (AddMandatoryAce): what Chromium's chrome_elf and
//! sandbox code call (ROADMAP BR).
//!
//! TRUSTEE_W and EXPLICIT_ACCESS_W are `layout::Trustee` and
//! `layout::ExplicitAccess` (their x64 and x86 layouts).

use super::layout::{ExplicitAccess, Part, SecurityDescriptor, Trustee, part};
use super::security::{
    GetSecurityDescriptorDacl, InitializeSecurityDescriptor, SetEntriesInAclW,
    SetSecurityDescriptorDacl, SetSecurityDescriptorGroup, SetSecurityDescriptorOwner,
};
use super::{fail, missing_detail};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn LocalAlloc(flags: u32, bytes: usize) -> *mut u8;
    fn LocalFree(p: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn RtlAbsoluteToSelfRelativeSD(abs: *const u8, rel: *mut u8, len: *mut u32) -> u32;
}

const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INVALID_SECURITY_DESCR: u32 = 1338;
const ERROR_NONE_MAPPED: u32 = 1332;
const TRUSTEE_IS_SID: u32 = 0;
const TRUSTEE_IS_NAME: u32 = 1;
const SE_SACL_PRESENT: u16 = 0x10;
const SE_SACL_DEFAULTED: u16 = 0x20;

/// A TRUSTEE_W (its TrusteeType TRUSTEE_IS_UNKNOWN).
fn trustee(form: u32, name: *const u16) -> Trustee {
    let null = core::ptr::null_mut();
    Trustee { multiple: null, operation: 0, form, kind: 0, name: name.cast_mut() }
}

/// Fills a TRUSTEE_W.
fn build_trustee(t: *mut Trustee, form: u32, name: *const u16) {
    if t.is_null() {
        return;
    }
    // SAFETY: the caller's TRUSTEE_W.
    unsafe { t.write_unaligned(trustee(form, name)) };
}

/// BuildTrusteeWithSidW(pTrustee, pSid): a TRUSTEE_IS_SID trustee.
#[unsafe(no_mangle)]
pub extern "system" fn BuildTrusteeWithSidW(t: *mut Trustee, sid: *const u8) {
    build_trustee(t, TRUSTEE_IS_SID, sid.cast());
}

/// BuildTrusteeWithNameW(pTrustee, pName): a TRUSTEE_IS_NAME trustee.
#[unsafe(no_mangle)]
pub extern "system" fn BuildTrusteeWithNameW(t: *mut Trustee, name: *const u16) {
    build_trustee(t, TRUSTEE_IS_NAME, name);
}

/// BuildExplicitAccessWithNameW(pExplicitAccess, pTrusteeName,
/// AccessPermissions, AccessMode, Ineritance).
#[unsafe(no_mangle)]
pub extern "system" fn BuildExplicitAccessWithNameW(
    ea: *mut ExplicitAccess,
    name: *const u16,
    permissions: u32,
    mode: u32,
    inheritance: u32,
) {
    if ea.is_null() {
        return;
    }
    let trustee = trustee(TRUSTEE_IS_NAME, name);
    // SAFETY: the caller's EXPLICIT_ACCESS_W.
    unsafe { ea.write_unaligned(ExplicitAccess { permissions, mode, inheritance, trustee }) };
}

/// The SID of a trustee (a SID, or a name LookupAccountNameW knows).
fn trustee_sid(t: *const Trustee, buf: &mut [u8; 68]) -> Option<*const u8> {
    // SAFETY: the caller's TRUSTEE_W.
    let Trustee { form, name, .. } = unsafe { t.read_unaligned() };
    let name = name.cast_const();
    match form {
        TRUSTEE_IS_SID => Some(name.cast()),
        TRUSTEE_IS_NAME => super::security::trustee_name_sid(name, buf).then_some(buf.as_ptr()),
        _ => None,
    }
}

/// BuildSecurityDescriptorW(pOwner, pGroup, cCountOfAccessEntries,
/// pListOfAccessEntries, cCountOfAuditEntries, pListOfAuditEntries, pOldSD,
/// pSizeNewSD, pNewSD): a self-relative descriptor (LocalAlloc'd) with the
/// owner and group trustees (or the old descriptor's) and a DACL of the
/// old one's ACEs with the entries applied (SetEntriesInAclW). Audit
/// entries (a SACL) are not supported (reported; ERROR_INVALID_PARAMETER).
/// Returns the error itself.
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn BuildSecurityDescriptorW(
    owner: *const Trustee,
    group: *const Trustee,
    access_count: u32,
    access: *const ExplicitAccess,
    audit_count: u32,
    _audit: *const ExplicitAccess,
    old: *const u8,
    size: *mut u32,
    new: *mut *mut u8,
) -> u32 {
    if size.is_null() || new.is_null() {
        return ERROR_INVALID_PARAMETER;
    }
    if audit_count != 0 {
        missing_detail(b"advapi32.dll!BuildSecurityDescriptorW(audit entries)", audit_count.into());
        return ERROR_INVALID_PARAMETER;
    }
    let mut sd = SecurityDescriptor::new();
    let abs = sd.ptr();
    InitializeSecurityDescriptor(abs, 1);
    let (mut owner_buf, mut group_buf) = ([0u8; 68], [0u8; 68]);
    let mut old_dacl: *const u8 = core::ptr::null();
    if !old.is_null() {
        let mut present = 0;
        let mut dacl = core::ptr::null_mut();
        let mut defaulted = 0;
        if GetSecurityDescriptorDacl(old, &mut present, &mut dacl, &mut defaulted) == 0 {
            return ERROR_INVALID_SECURITY_DESCR;
        }
        if present != 0 {
            old_dacl = dacl;
        }
        let (mut o, mut g) = (core::ptr::null_mut(), core::ptr::null_mut());
        let mut d = 0;
        super::security::GetSecurityDescriptorOwner(old, &mut o, &mut d);
        super::security::GetSecurityDescriptorGroup(old, &mut g, &mut d);
        SetSecurityDescriptorOwner(abs, o, 0);
        SetSecurityDescriptorGroup(abs, g, 0);
    }
    if !owner.is_null() {
        let Some(sid) = trustee_sid(owner, &mut owner_buf) else { return ERROR_NONE_MAPPED };
        SetSecurityDescriptorOwner(abs, sid.cast_mut(), 0);
    }
    if !group.is_null() {
        let Some(sid) = trustee_sid(group, &mut group_buf) else { return ERROR_NONE_MAPPED };
        SetSecurityDescriptorGroup(abs, sid.cast_mut(), 0);
    }
    let mut dacl = core::ptr::null_mut();
    if access_count != 0 || !old_dacl.is_null() {
        let e = SetEntriesInAclW(access_count, access, old_dacl, &mut dacl);
        if e != 0 {
            return e;
        }
        SetSecurityDescriptorDacl(abs, 1, dacl, 0);
    }
    let mut len = 0u32;
    // SAFETY: an absolute descriptor; asks for the size.
    unsafe { RtlAbsoluteToSelfRelativeSD(abs, core::ptr::null_mut(), &mut len) };
    // SAFETY: a plain allocation.
    let rel = unsafe { LocalAlloc(0x40, len as usize) };
    if rel.is_null() {
        // SAFETY: SetEntriesInAclW's ACL (or null).
        unsafe { LocalFree(dacl.cast()) };
        return 8; // ERROR_NOT_ENOUGH_MEMORY
    }
    // SAFETY: a buffer of `len` bytes.
    let status = unsafe { RtlAbsoluteToSelfRelativeSD(abs, rel, &mut len) };
    // SAFETY: SetEntriesInAclW's ACL (copied into `rel`), or null.
    unsafe { LocalFree(dacl.cast()) };
    if status != 0 {
        // SAFETY: the allocation above.
        unsafe { LocalFree(rel.cast()) };
        return ERROR_INVALID_SECURITY_DESCR;
    }
    // SAFETY: the caller's outputs.
    unsafe {
        *size = len;
        *new = rel;
    }
    0
}

/// GetSecurityDescriptorSacl: absolute or self-relative descriptors.
#[unsafe(no_mangle)]
pub extern "system" fn GetSecurityDescriptorSacl(
    sd: *const u8,
    present: *mut i32,
    sacl: *mut *mut u8,
    defaulted: *mut i32,
) -> i32 {
    if super::security::IsValidSecurityDescriptor(sd) == 0 || present.is_null() {
        return fail(ERROR_INVALID_SECURITY_DESCR, 0);
    }
    // SAFETY: the caller's descriptor and outputs.
    unsafe {
        let c = sd.add(2).cast::<u16>().read_unaligned();
        *present = i32::from(c & SE_SACL_PRESENT != 0);
        if c & SE_SACL_PRESENT != 0 {
            if !sacl.is_null() {
                *sacl = part(sd, Part::Sacl);
            }
            if !defaulted.is_null() {
                *defaulted = i32::from(c & SE_SACL_DEFAULTED != 0);
            }
        }
    }
    1
}

/// AddMandatoryAce(pAcl, dwAceRevision, AceFlags, MandatoryPolicy,
/// pLabelSid): a SYSTEM_MANDATORY_LABEL_ACE (type 0x11; the policy bits
/// NO_WRITE_UP, NO_READ_UP, NO_EXECUTE_UP are its mask) at the end of a
/// SACL.
#[unsafe(no_mangle)]
pub extern "system" fn AddMandatoryAce(
    acl: *mut u8,
    _revision: u32,
    flags: u32,
    policy: u32,
    sid: *const u8,
) -> i32 {
    if policy & !7 != 0 {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    super::security::add_simple_ace(acl, 0x11, flags, policy, sid)
}
