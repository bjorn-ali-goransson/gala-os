//! The LSA policy functions sshd uses for its virtual accounts
//! (docs/SECURITY.md, phase S4), with their documented contracts:
//!
//! - LsaManageSidNameMapping adds and removes SID-to-name mappings (a
//!   domain, then accounts under it whose SIDs extend the domain's); it
//!   needs SeTcbPrivilege. Errors come back as an
//!   LSA_SID_NAME_MAPPING_OPERATION_GENERIC_OUTPUT (LsaFreeMemory).
//!   Mappings are kept in the registry under
//!   `HKLM\SAM\Gala\Mappings\<domain>[\<account>]` (value `Sid`), where
//!   LookupAccountSidW finds them and the kernel's virtual logon reads
//!   them.
//! - LsaOpenPolicy gives a policy handle; access beyond looking up and
//!   viewing needs write access to `HKLM\SAM\Gala` (Administrators and
//!   SYSTEM), else STATUS_ACCESS_DENIED.
//! - LsaAddAccountRights / LsaRemoveAccountRights /
//!   LsaEnumerateAccountRights keep an account's rights (privileges and
//!   logon rights by name) as values of `HKLM\SAM\Gala\Rights\<SID>`;
//!   the kernel checks SeServiceLogonRight for virtual logons.
//! - LsaClose, LsaFreeMemory.

use core::ffi::c_void;

use super::registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegDeleteValueW, RegEnumKeyExW, RegEnumValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LocalAlloc(flags: u32, bytes: usize) -> *mut u8;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
}

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

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *const u16,
}

impl UnicodeString {
    fn chars(&self) -> &[u16] {
        if self.buffer.is_null() || self.length == 0 {
            return &[];
        }
        // SAFETY: the caller's string of `length` bytes.
        unsafe { core::slice::from_raw_parts(self.buffer, usize::from(self.length / 2)) }
    }
}

const STATUS_SUCCESS: u32 = 0;
const STATUS_INVALID_PARAMETER: u32 = 0xc000_000d;
const STATUS_NO_MEMORY: u32 = 0xc000_0017;
const STATUS_ACCESS_DENIED: u32 = 0xc000_0022;
const STATUS_OBJECT_NAME_NOT_FOUND: u32 = 0xc000_0034;
const STATUS_INVALID_HANDLE: u32 = 0xc000_0008;
const STATUS_NO_SUCH_PRIVILEGE: u32 = 0xc000_0060;
const STATUS_PRIVILEGE_NOT_HELD: u32 = 0xc000_0061;
const STATUS_INVALID_SID: u32 = 0xc000_0078;

const HKLM: usize = 0x8000_0002;
const KEY_READ: u32 = 0x2_0019;
const KEY_ALL_ACCESS: u32 = 0xf_003f;
const KEY_WRITE: u32 = 0x2_0006;
const REG_BINARY: u32 = 3;
const REG_DWORD: u32 = 4;
const ERROR_FILE_NOT_FOUND: i32 = 2;

/// LSA_SID_NAME_MAPPING_OPERATION_TYPE and _ERROR.
const OP_ADD: u32 = 0;
const OP_REMOVE: u32 = 1;
const NAME_COLLISION: u32 = 2;
const SID_COLLISION: u32 = 3;
const DOMAIN_NOT_FOUND: u32 = 4;
const DOMAIN_SID_PREFIX_MISMATCH: u32 = 5;
const MAPPING_NOT_FOUND: u32 = 6;

const MAPPINGS: &str = "SAM\\Gala\\Mappings";
const RIGHTS: &str = "SAM\\Gala\\Rights";

/// A key path: an ASCII prefix, then `\part` for each part, NUL-terminated.
struct Path {
    chars: [u16; 512],
    len: usize,
}

impl Path {
    fn new(prefix: &str) -> Path {
        let mut p = Path { chars: [0; 512], len: 0 };
        for c in prefix.encode_utf16() {
            p.chars[p.len] = c;
            p.len += 1;
        }
        p
    }

    /// Appends `\part`; None if it is too long or names a subkey.
    fn push(mut self, part: &[u16]) -> Option<Path> {
        if part.is_empty() || part.contains(&u16::from(b'\\')) || self.len + part.len() + 2 > 512 {
            return None;
        }
        self.chars[self.len] = u16::from(b'\\');
        self.chars[self.len + 1..self.len + 1 + part.len()].copy_from_slice(part);
        self.len += part.len() + 1;
        self.chars[self.len] = 0;
        Some(self)
    }

    fn ptr(&self) -> *const u16 {
        self.chars.as_ptr()
    }
}

const SID_VALUE: [u16; 4] = [b'S' as u16, b'i' as u16, b'd' as u16, 0];

/// The SID stored at key `path` (its `Sid` value), into `out`.
fn stored_sid(path: &Path, out: &mut [u8; 68]) -> Option<usize> {
    let mut key = 0;
    if RegOpenKeyExW(HKLM, path.ptr(), 0, KEY_READ, &mut key) != 0 {
        return None;
    }
    let mut size = 68u32;
    let r = RegQueryValueExW(
        key,
        SID_VALUE.as_ptr(),
        core::ptr::null_mut(),
        core::ptr::null_mut(),
        out.as_mut_ptr(),
        &mut size,
    );
    RegCloseKey(key);
    (r == 0).then_some(size as usize)
}

/// The bytes of a valid SID at `p`.
fn sid_bytes<'a>(p: *const u8) -> Option<&'a [u8]> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller's SID: revision, count, then its sub-authorities.
    unsafe {
        if *p != 1 || *p.add(1) > 15 {
            return None;
        }
        Some(core::slice::from_raw_parts(p, 8 + 4 * usize::from(*p.add(1))))
    }
}

/// Whether `sid` extends `domain` (same authority, its sub-authorities
/// first, at least one more).
fn extends(domain: &[u8], sid: &[u8]) -> bool {
    domain.len() >= 8
        && sid.len() > domain.len()
        && sid[2..8] == domain[2..8]
        && sid[8..domain.len()] == domain[8..]
}

/// Calls `f(index, name)` for the subkeys of `path` until it returns
/// Some.
fn each_subkey<T>(path: &Path, mut f: impl FnMut(&[u16]) -> Option<T>) -> Option<T> {
    let mut key = 0;
    if RegOpenKeyExW(HKLM, path.ptr(), 0, KEY_READ, &mut key) != 0 {
        return None;
    }
    let mut found = None;
    for index in 0..4096 {
        let mut name = [0u16; 256];
        let mut len = 256u32;
        let r = RegEnumKeyExW(
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
        found = f(&name[..len as usize]);
        if found.is_some() {
            break;
        }
    }
    RegCloseKey(key);
    found
}

/// The mapping of `sid`: (domain, account; empty for the domain itself),
/// each NUL-terminated in its buffer, with their lengths.
pub(crate) fn mapped_name(
    sid: &[u8],
    account: &mut [u16; 256],
    domain: &mut [u16; 256],
) -> Option<(usize, usize)> {
    let root = Path::new(MAPPINGS);
    each_subkey(&root, |d| {
        let dpath = Path::new(MAPPINGS).push(d)?;
        let mut dsid = [0u8; 68];
        let n = stored_sid(&dpath, &mut dsid)?;
        let dsid = &dsid[..n];
        let account_len = if dsid == sid {
            Some(0)
        } else if extends(dsid, sid) {
            each_subkey(&dpath, |a| {
                let apath = Path::new(MAPPINGS).push(d)?.push(a)?;
                let mut asid = [0u8; 68];
                let m = stored_sid(&apath, &mut asid)?;
                (&asid[..m] == sid).then(|| {
                    account[..a.len()].copy_from_slice(a);
                    account[a.len()] = 0;
                    a.len()
                })
            })
        } else {
            None
        }?;
        domain[..d.len()].copy_from_slice(d);
        domain[d.len()] = 0;
        Some((account_len, d.len()))
    })
}

/// Whether the caller's token (the thread's, else the process's) has
/// SeTcbPrivilege enabled.
fn has_tcb() -> bool {
    let mut token = 0;
    let mut b = [0u32; 128];
    let mut n = 0;
    // SAFETY: plain calls with our buffer (TOKEN_QUERY; TokenPrivileges).
    unsafe {
        if NtOpenThreadToken(usize::MAX - 1, 8, 1, &mut token) != 0
            && NtOpenProcessToken(usize::MAX, 8, &mut token) != 0
        {
            return false;
        }
        let status = NtQueryInformationToken(token, 3, b.as_mut_ptr().cast(), 512, &mut n);
        NtClose(token);
        if status != 0 {
            return false;
        }
    }
    (0..(b[0] as usize).min(42)).any(|i| b[1 + 3 * i] == 7 && b[3 + 3 * i] & 2 != 0)
}

/// An operation's result: the status, and on failure an output with
/// the error code (LSA memory) if the caller asked for one.
fn op_result(output: *mut *mut u32, status: u32, error: u32) -> u32 {
    if status != STATUS_SUCCESS && !output.is_null() {
        // SAFETY: a plain allocation; the caller's output.
        unsafe {
            let p = LocalAlloc(0x40, 4).cast::<u32>();
            if !p.is_null() {
                p.write(error);
            }
            *output = p;
        }
    }
    status
}

/// LsaManageSidNameMapping(OpType, OpInput, *OpOutput)
#[unsafe(no_mangle)]
pub extern "system" fn LsaManageSidNameMapping(
    operation: u32,
    input: *const u8,
    output: *mut *mut u32,
) -> u32 {
    if !output.is_null() {
        // SAFETY: the caller's output.
        unsafe { *output = core::ptr::null_mut() };
    }
    if input.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    if !has_tcb() {
        return STATUS_PRIVILEGE_NOT_HELD;
    }
    // ADD_INPUT {DomainName, AccountName, Sid, Flags}; REMOVE_INPUT
    // {DomainName, AccountName}.
    // SAFETY: the caller's input structure.
    let (domain, account) = unsafe {
        (
            input.cast::<UnicodeString>().read_unaligned(),
            input.add(16).cast::<UnicodeString>().read_unaligned(),
        )
    };
    let (domain, account) = (domain.chars(), account.chars());
    let Some(dpath) = Path::new(MAPPINGS).push(domain) else {
        return STATUS_INVALID_PARAMETER;
    };
    let target = if account.is_empty() {
        Some(dpath)
    } else {
        Path::new(MAPPINGS).push(domain).and_then(|p| p.push(account))
    };
    let Some(target) = target else { return STATUS_INVALID_PARAMETER };
    match operation {
        OP_ADD => {
            // SAFETY: as above.
            let sid = unsafe { input.add(32).cast::<*const u8>().read_unaligned() };
            let Some(sid) = sid_bytes(sid) else { return STATUS_INVALID_SID };
            let mut existing = [0u8; 68];
            if stored_sid(&target, &mut existing).is_some() {
                return op_result(output, STATUS_INVALID_PARAMETER, NAME_COLLISION);
            }
            let (mut a, mut d) = ([0u16; 256], [0u16; 256]);
            if mapped_name(sid, &mut a, &mut d).is_some() {
                return op_result(output, STATUS_INVALID_PARAMETER, SID_COLLISION);
            }
            if !account.is_empty() {
                let mut dsid = [0u8; 68];
                let Some(n) =
                    Path::new(MAPPINGS).push(domain).and_then(|p| stored_sid(&p, &mut dsid))
                else {
                    return op_result(output, STATUS_INVALID_PARAMETER, DOMAIN_NOT_FOUND);
                };
                if !extends(&dsid[..n], sid) {
                    return op_result(output, STATUS_INVALID_PARAMETER, DOMAIN_SID_PREFIX_MISMATCH);
                }
            }
            let mut key = 0;
            let r = RegCreateKeyExW(
                HKLM,
                target.ptr(),
                0,
                core::ptr::null(),
                0,
                KEY_ALL_ACCESS,
                core::ptr::null_mut(),
                &mut key,
                core::ptr::null_mut(),
            );
            if r != 0 {
                return if r == 5 { STATUS_ACCESS_DENIED } else { STATUS_NO_MEMORY };
            }
            let r = RegSetValueExW(
                key,
                SID_VALUE.as_ptr(),
                0,
                REG_BINARY,
                sid.as_ptr(),
                sid.len() as u32,
            );
            RegCloseKey(key);
            if r != 0 { STATUS_NO_MEMORY } else { STATUS_SUCCESS }
        }
        OP_REMOVE => {
            let mut existing = [0u8; 68];
            if stored_sid(&target, &mut existing).is_none() {
                return op_result(output, STATUS_INVALID_PARAMETER, MAPPING_NOT_FOUND);
            }
            if RegDeleteKeyW(HKLM, target.ptr()) != 0 {
                // A domain with accounts still mapped under it.
                return op_result(output, STATUS_INVALID_PARAMETER, 1);
            }
            STATUS_SUCCESS
        }
        _ => {
            super::missing(b"advapi32.dll!LsaManageSidNameMapping(operation)");
            STATUS_INVALID_PARAMETER
        }
    }
}

/// LsaFreeMemory(Buffer): memory the LSA functions returned.
#[unsafe(no_mangle)]
pub extern "system" fn LsaFreeMemory(buffer: *mut c_void) -> u32 {
    // SAFETY: from LocalAlloc (or null).
    unsafe { LocalFree(buffer) };
    STATUS_SUCCESS
}

/// A policy handle: LocalAlloc'd, a tag and the access granted.
#[repr(C)]
struct Policy {
    tag: u32,
    access: u32,
}

const POLICY_TAG: u32 = 0x4c53_5031;
/// POLICY_VIEW_LOCAL_INFORMATION | POLICY_VIEW_AUDIT_INFORMATION |
/// POLICY_LOOKUP_NAMES | POLICY_NOTIFICATION | READ_CONTROL: what any
/// user may ask for.
const POLICY_READ_ONLY: u32 = 0x1 | 0x2 | 0x800 | 0x1000 | 0x2_0000;
const POLICY_CREATE_ACCOUNT: u32 = 0x10;

fn policy(h: *mut c_void) -> Option<u32> {
    if h.is_null() {
        return None;
    }
    // SAFETY: a handle from LsaOpenPolicy (checked by its tag).
    let p = unsafe { h.cast::<Policy>().read() };
    (p.tag == POLICY_TAG).then_some(p.access)
}

/// LsaOpenPolicy(SystemName, ObjectAttributes, DesiredAccess,
/// *PolicyHandle): this machine's policy.
#[unsafe(no_mangle)]
pub extern "system" fn LsaOpenPolicy(
    system: *const UnicodeString,
    attributes: *const c_void,
    access: u32,
    handle: *mut *mut c_void,
) -> u32 {
    if handle.is_null() || attributes.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: the caller's string, if any.
    if !system.is_null() && !unsafe { (*system).chars() }.is_empty() {
        super::missing(b"advapi32.dll!LsaOpenPolicy(remote system)");
        return 0xc000_0022;
    }
    // MAXIMUM_ALLOWED / GENERIC_ALL / GENERIC_WRITE count as writing.
    if access & !POLICY_READ_ONLY != 0 {
        let gala = Path::new("SAM\\Gala");
        let mut key = 0;
        if RegOpenKeyExW(HKLM, gala.chars.as_ptr(), 0, KEY_WRITE, &mut key) != 0 {
            return STATUS_ACCESS_DENIED;
        }
        RegCloseKey(key);
    }
    // SAFETY: a plain allocation; the caller's output.
    unsafe {
        let p = LocalAlloc(0x40, core::mem::size_of::<Policy>()).cast::<Policy>();
        if p.is_null() {
            return STATUS_NO_MEMORY;
        }
        p.write(Policy {
            tag: POLICY_TAG,
            access: if access & 0x1000_0000 != 0 { !0 } else { access },
        });
        *handle = p.cast();
    }
    STATUS_SUCCESS
}

/// LsaClose(ObjectHandle)
#[unsafe(no_mangle)]
pub extern "system" fn LsaClose(handle: *mut c_void) -> u32 {
    if policy(handle).is_none() {
        return STATUS_INVALID_HANDLE;
    }
    // SAFETY: ours (checked); the tag is cleared before it is freed.
    unsafe {
        handle.cast::<Policy>().write(Policy { tag: 0, access: 0 });
        LocalFree(handle);
    }
    STATUS_SUCCESS
}

/// The rights key path of `sid` (`SAM\Gala\Rights\S-1-...`).
fn rights_path(sid: *const u8) -> Option<Path> {
    sid_bytes(sid)?;
    let mut text = core::ptr::null_mut::<u16>();
    if super::security::ConvertSidToStringSidW(sid, &mut text) == 0 {
        return None;
    }
    let mut n = 0;
    // SAFETY: the NUL-terminated string just made.
    while unsafe { *text.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: as above.
    let path = Path::new(RIGHTS).push(unsafe { core::slice::from_raw_parts(text, n) });
    // SAFETY: from LocalAlloc.
    unsafe { LocalFree(text.cast()) };
    path
}

/// Whether `name` names a right: a privilege, or a logon right.
fn known_right(name: &[u16]) -> bool {
    const LOGON_RIGHTS: [&str; 10] = [
        "SeInteractiveLogonRight",
        "SeNetworkLogonRight",
        "SeBatchLogonRight",
        "SeServiceLogonRight",
        "SeRemoteInteractiveLogonRight",
        "SeDenyInteractiveLogonRight",
        "SeDenyNetworkLogonRight",
        "SeDenyBatchLogonRight",
        "SeDenyServiceLogonRight",
        "SeDenyRemoteInteractiveLogonRight",
    ];
    let same = |s: &str| s.len() == name.len() && s.encode_utf16().zip(name).all(|(a, &b)| a == b);
    if LOGON_RIGHTS.iter().any(|s| same(s)) {
        return true;
    }
    let mut z = [0u16; 64];
    if name.len() >= 64 {
        return false;
    }
    z[..name.len()].copy_from_slice(name);
    let mut luid = 0u64;
    super::security::LookupPrivilegeValueW(core::ptr::null(), z.as_ptr(), (&raw mut luid).cast())
        != 0
}

/// The rights array of `count` LSA_UNICODE_STRINGs at `rights`.
fn rights_of<'a>(rights: *const UnicodeString, count: u32) -> &'a [UnicodeString] {
    if rights.is_null() || count == 0 {
        return &[];
    }
    // SAFETY: the caller's array of `count` strings.
    unsafe { core::slice::from_raw_parts(rights, count as usize) }
}

/// A right's name, NUL-terminated.
fn z(name: &[u16]) -> Option<[u16; 64]> {
    if name.is_empty() || name.len() >= 64 {
        return None;
    }
    let mut b = [0u16; 64];
    b[..name.len()].copy_from_slice(name);
    Some(b)
}

/// LsaAddAccountRights(PolicyHandle, AccountSid, UserRights,
/// CountOfRights): needs POLICY_CREATE_ACCOUNT (the account is created
/// if it has no rights yet).
#[unsafe(no_mangle)]
pub extern "system" fn LsaAddAccountRights(
    handle: *mut c_void,
    sid: *const u8,
    rights: *const UnicodeString,
    count: u32,
) -> u32 {
    let Some(access) = policy(handle) else { return STATUS_INVALID_HANDLE };
    if access & POLICY_CREATE_ACCOUNT == 0 {
        return STATUS_ACCESS_DENIED;
    }
    let Some(path) = rights_path(sid) else { return STATUS_INVALID_SID };
    let rights = rights_of(rights, count);
    if rights.is_empty() {
        return STATUS_INVALID_PARAMETER;
    }
    if !rights.iter().all(|r| known_right(r.chars())) {
        return STATUS_NO_SUCH_PRIVILEGE;
    }
    let mut key = 0;
    let r = RegCreateKeyExW(
        HKLM,
        path.ptr(),
        0,
        core::ptr::null(),
        0,
        KEY_ALL_ACCESS,
        core::ptr::null_mut(),
        &mut key,
        core::ptr::null_mut(),
    );
    if r != 0 {
        return if r == 5 { STATUS_ACCESS_DENIED } else { STATUS_NO_MEMORY };
    }
    let one = 1u32.to_le_bytes();
    let mut status = STATUS_SUCCESS;
    for right in rights {
        let Some(name) = z(right.chars()) else { continue };
        if RegSetValueExW(key, name.as_ptr(), 0, REG_DWORD, one.as_ptr(), 4) != 0 {
            status = STATUS_NO_MEMORY;
        }
    }
    RegCloseKey(key);
    status
}

/// LsaRemoveAccountRights(PolicyHandle, AccountSid, AllRights,
/// UserRights, CountOfRights): an account left with no rights is
/// removed.
#[unsafe(no_mangle)]
pub extern "system" fn LsaRemoveAccountRights(
    handle: *mut c_void,
    sid: *const u8,
    all: u8,
    rights: *const UnicodeString,
    count: u32,
) -> u32 {
    if policy(handle).is_none() {
        return STATUS_INVALID_HANDLE;
    }
    let Some(path) = rights_path(sid) else { return STATUS_INVALID_SID };
    let mut key = 0;
    match RegOpenKeyExW(HKLM, path.ptr(), 0, KEY_ALL_ACCESS, &mut key) {
        0 => {}
        ERROR_FILE_NOT_FOUND => return STATUS_OBJECT_NAME_NOT_FOUND,
        _ => return STATUS_ACCESS_DENIED,
    }
    if all == 0 {
        for right in rights_of(rights, count) {
            if let Some(name) = z(right.chars()) {
                RegDeleteValueW(key, name.as_ptr());
            }
        }
    }
    let mut name = [0u16; 64];
    let mut len = 64u32;
    let left = all == 0
        && RegEnumValueW(
            key,
            0,
            name.as_mut_ptr(),
            &mut len,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        ) == 0;
    RegCloseKey(key);
    if !left {
        RegDeleteKeyW(HKLM, path.ptr());
    }
    STATUS_SUCCESS
}

/// LsaEnumerateAccountRights(PolicyHandle, AccountSid, *UserRights,
/// *CountOfRights): the rights in one LSA allocation (LsaFreeMemory);
/// an account without rights is STATUS_OBJECT_NAME_NOT_FOUND.
#[unsafe(no_mangle)]
pub extern "system" fn LsaEnumerateAccountRights(
    handle: *mut c_void,
    sid: *const u8,
    rights: *mut *mut UnicodeString,
    count: *mut u32,
) -> u32 {
    if policy(handle).is_none() {
        return STATUS_INVALID_HANDLE;
    }
    if rights.is_null() || count.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    let Some(path) = rights_path(sid) else { return STATUS_INVALID_SID };
    let mut key = 0;
    if RegOpenKeyExW(HKLM, path.ptr(), 0, KEY_READ, &mut key) != 0 {
        return STATUS_OBJECT_NAME_NOT_FOUND;
    }
    // Up to 64 names of up to 63 characters: the strings, then the text.
    let mut names = [[0u16; 64]; 64];
    let mut lens = [0usize; 64];
    let mut n = 0;
    while n < 64 {
        let mut len = 64u32;
        let r = RegEnumValueW(
            key,
            n as u32,
            names[n].as_mut_ptr(),
            &mut len,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        if r != 0 {
            break;
        }
        lens[n] = len as usize;
        n += 1;
    }
    RegCloseKey(key);
    if n == 0 {
        return STATUS_OBJECT_NAME_NOT_FOUND;
    }
    let text: usize = lens[..n].iter().map(|l| 2 * (l + 1)).sum();
    let head = 16 * n;
    // SAFETY: a plain allocation of head + text bytes, filled in order.
    unsafe {
        let block = LocalAlloc(0x40, head + text);
        if block.is_null() {
            return STATUS_NO_MEMORY;
        }
        let mut at = head;
        for i in 0..n {
            let chars = block.add(at).cast::<u16>();
            core::ptr::copy_nonoverlapping(names[i].as_ptr(), chars, lens[i]);
            block.add(16 * i).cast::<UnicodeString>().write(UnicodeString {
                length: (2 * lens[i]) as u16,
                maximum: (2 * lens[i] + 2) as u16,
                buffer: chars,
            });
            at += 2 * (lens[i] + 1);
        }
        *rights = block.cast();
        *count = n as u32;
    }
    STATUS_SUCCESS
}
