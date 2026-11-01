//! The LSA logon functions (docs/SECURITY.md, phase S4) as Win32-OpenSSH's
//! sshd uses them, with their documented contracts: LsaConnectUntrusted,
//! LsaRegisterLogonProcess (needs SeTcbPrivilege), LsaLookupAuthenticationPackage
//! (MSV1_0, Negotiate, Kerberos), LsaLogonUser with MSV1_0's S4U logon
//! (no password; the kernel requires SeTcbPrivilege) and interactive logon
//! (with the password), LsaFreeReturnBuffer, LsaDeregisterLogonProcess.
//! The logons are gala's GalaLogonUser service (nt::accounts); there is no
//! LSA process. An LSA handle is a tag (untrusted or registered).

use core::ffi::c_void;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaLogonUser(
        user: *const UnicodeString,
        domain: *const UnicodeString,
        password: *const UnicodeString,
        kind: u32,
        token: *mut usize,
    ) -> u32;
    fn NtOpenProcessToken(process: isize, access: u32, token: *mut usize) -> u32;
    fn NtQueryInformationToken(
        token: usize,
        class: u32,
        buf: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> u32;
    fn NtClose(h: usize) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LocalAlloc(flags: u32, bytes: usize) -> *mut u8;
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *const u16,
}

/// LSA_STRING.
#[repr(C)]
pub struct LsaString {
    length: u16,
    maximum: u16,
    buffer: *const u8,
}

const STATUS_SUCCESS: u32 = 0;
const STATUS_INVALID_PARAMETER: u32 = 0xc000_000d;
const STATUS_NO_MEMORY: u32 = 0xc000_0017;
const STATUS_INVALID_HANDLE: u32 = 0xc000_0008;
const STATUS_PRIVILEGE_NOT_HELD: u32 = 0xc000_0061;
const STATUS_NO_SUCH_PACKAGE: u32 = 0xc000_00fe;

/// The handles: an untrusted connection, a registered logon process.
const UNTRUSTED: usize = 0x4c53_0001;
const REGISTERED: usize = 0x4c53_0002;

/// The packages, in the order of their ids.
const PACKAGES: [&[u8]; 3] = [b"Negotiate", b"MICROSOFT_AUTHENTICATION_PACKAGE_V1_0", b"Kerberos"];

const MSV1_0_INTERACTIVE_LOGON: u32 = 2;
const MSV1_0_S4U_LOGON: u32 = 12;
/// GalaLogonUser's flag for an S4U logon.
const GALA_LOGON_S4U: u32 = 0x8000_0000;
/// MSV1_0_INTERACTIVE_PROFILE's size (x64) and MessageType.
const PROFILE_SIZE: usize = 0x98;
const MSV1_0_INTERACTIVE_PROFILE: u32 = 2;

fn valid(h: usize) -> bool {
    h == UNTRUSTED || h == REGISTERED
}

/// LsaConnectUntrusted(*LsaHandle)
#[unsafe(no_mangle)]
pub extern "system" fn LsaConnectUntrusted(handle: *mut usize) -> u32 {
    if handle.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: the caller's output.
    unsafe { *handle = UNTRUSTED };
    STATUS_SUCCESS
}

/// Whether the process token has SeTcbPrivilege enabled.
fn has_tcb() -> bool {
    let mut token = 0;
    let mut b = [0u32; 128];
    let mut n = 0;
    // SAFETY: plain calls with our buffer (TOKEN_QUERY; TokenPrivileges).
    unsafe {
        if NtOpenProcessToken(-1, 8, &mut token) != 0 {
            return false;
        }
        let status = NtQueryInformationToken(token, 3, b.as_mut_ptr().cast(), 512, &mut n);
        NtClose(token);
        if status != 0 {
            return false;
        }
    }
    // TOKEN_PRIVILEGES: count, then (LUID low, high, attributes).
    (0..b[0] as usize).any(|i| b[1 + 3 * i] == 7 && b[3 + 3 * i] & 2 != 0)
}

/// LsaRegisterLogonProcess(LogonProcessName, *LsaHandle, *SecurityMode):
/// a trusted connection, for callers with SeTcbPrivilege
/// (STATUS_PRIVILEGE_NOT_HELD).
#[unsafe(no_mangle)]
pub extern "system" fn LsaRegisterLogonProcess(
    name: *const LsaString,
    handle: *mut usize,
    mode: *mut u32,
) -> u32 {
    if name.is_null() || handle.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    if !has_tcb() {
        return STATUS_PRIVILEGE_NOT_HELD;
    }
    // SAFETY: the caller's outputs.
    unsafe {
        *handle = REGISTERED;
        if !mode.is_null() {
            *mode = 0;
        }
    }
    STATUS_SUCCESS
}

/// LsaDeregisterLogonProcess(LsaHandle)
#[unsafe(no_mangle)]
pub extern "system" fn LsaDeregisterLogonProcess(handle: usize) -> u32 {
    if valid(handle) { STATUS_SUCCESS } else { STATUS_INVALID_HANDLE }
}

/// LsaLookupAuthenticationPackage(LsaHandle, PackageName, *AuthenticationPackage)
#[unsafe(no_mangle)]
pub extern "system" fn LsaLookupAuthenticationPackage(
    handle: usize,
    name: *const LsaString,
    package: *mut u32,
) -> u32 {
    if !valid(handle) {
        return STATUS_INVALID_HANDLE;
    }
    if name.is_null() || package.is_null() {
        return STATUS_INVALID_PARAMETER;
    }
    // SAFETY: the caller's LSA_STRING.
    let text = unsafe { core::slice::from_raw_parts((*name).buffer, usize::from((*name).length)) };
    match PACKAGES.iter().position(|p| p.eq_ignore_ascii_case(text)) {
        Some(i) => {
            // SAFETY: the caller's output.
            unsafe { *package = i as u32 };
            STATUS_SUCCESS
        }
        None => STATUS_NO_SUCH_PACKAGE,
    }
}

/// LsaFreeReturnBuffer(Buffer): a buffer the LSA functions returned.
#[unsafe(no_mangle)]
pub extern "system" fn LsaFreeReturnBuffer(buffer: *mut c_void) -> u32 {
    // SAFETY: from LocalAlloc (or null).
    unsafe { LocalFree(buffer) };
    STATUS_SUCCESS
}

/// A UNICODE_STRING of an authentication information block at `info`
/// (`length` bytes) at offset `at`: its buffer an address, or an offset
/// into the block (both are in use).
fn string_at(info: *const u8, length: usize, at: usize) -> Option<UnicodeString> {
    if at + 16 > length {
        return None;
    }
    // SAFETY: inside the caller's block of `length` bytes.
    let mut s = unsafe { info.add(at).cast::<UnicodeString>().read_unaligned() };
    let offset = s.buffer as usize;
    if !s.buffer.is_null() && offset < length {
        s.buffer = info.wrapping_add(offset).cast();
    }
    Some(s)
}

/// LsaLogonUser(LsaHandle, OriginName, LogonType, AuthenticationPackage,
/// AuthenticationInformation, AuthenticationInformationLength, LocalGroups,
/// SourceContext, *ProfileBuffer, *ProfileBufferLength, *LogonId, *Token,
/// *Quotas, *SubStatus): MSV1_0 (or Negotiate) S4U and interactive logons.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn LsaLogonUser(
    handle: usize,
    _origin: *const LsaString,
    kind: u32,
    package: u32,
    info: *const u8,
    info_length: u32,
    groups: *const u8,
    _source: *const u8,
    profile: *mut *mut u8,
    profile_length: *mut u32,
    logon_id: *mut u64,
    token: *mut usize,
    quotas: *mut [u64; 6],
    substatus: *mut u32,
) -> u32 {
    if !valid(handle) {
        return STATUS_INVALID_HANDLE;
    }
    if package > 1 {
        super::missing(b"secur32.dll!LsaLogonUser(package)");
        return STATUS_NO_SUCH_PACKAGE;
    }
    if info.is_null() || token.is_null() || (info_length as usize) < 8 {
        return STATUS_INVALID_PARAMETER;
    }
    if !groups.is_null() {
        super::missing(b"secur32.dll!LsaLogonUser(local groups)");
    }
    let length = info_length as usize;
    // SAFETY: the caller's block, at least 8 bytes.
    let message = unsafe { info.cast::<u32>().read_unaligned() };
    let empty = UnicodeString { length: 0, maximum: 0, buffer: core::ptr::null() };
    let (user, domain, password, flags) = match message {
        MSV1_0_S4U_LOGON => {
            let (Some(u), Some(d)) = (string_at(info, length, 8), string_at(info, length, 24))
            else {
                return STATUS_INVALID_PARAMETER;
            };
            (u, d, empty, GALA_LOGON_S4U)
        }
        MSV1_0_INTERACTIVE_LOGON => {
            let parts = (
                string_at(info, length, 8),
                string_at(info, length, 24),
                string_at(info, length, 40),
            );
            let (Some(d), Some(u), Some(p)) = parts else { return STATUS_INVALID_PARAMETER };
            (u, d, p, 0)
        }
        _ => {
            super::missing(b"secur32.dll!LsaLogonUser(logon message)");
            return STATUS_INVALID_PARAMETER;
        }
    };
    let mut h = 0;
    // SAFETY: our strings; our output.
    let status = unsafe { GalaLogonUser(&user, &domain, &password, kind | flags, &mut h) };
    if !substatus.is_null() {
        // SAFETY: the caller's output.
        unsafe { *substatus = 0 };
    }
    if status != 0 {
        return status;
    }
    // The logon session: TokenStatistics.AuthenticationId.
    let mut stats = [0u64; 7];
    let mut n = 0;
    // SAFETY: our 56-byte buffer.
    unsafe { NtQueryInformationToken(h, 10, stats.as_mut_ptr().cast(), 56, &mut n) };
    // SAFETY: a plain allocation (LMEM_ZEROINIT).
    let block = unsafe { LocalAlloc(0x40, PROFILE_SIZE) };
    if block.is_null() {
        // SAFETY: ours.
        unsafe { NtClose(h) };
        return STATUS_NO_MEMORY;
    }
    // SAFETY: the caller's outputs; our block.
    unsafe {
        block.cast::<u32>().write(MSV1_0_INTERACTIVE_PROFILE);
        *token = h;
        if !logon_id.is_null() {
            *logon_id = stats[1];
        }
        if !profile.is_null() {
            *profile = block;
        } else {
            LocalFree(block.cast());
        }
        if !profile_length.is_null() {
            *profile_length = PROFILE_SIZE as u32;
        }
        if !quotas.is_null() {
            *quotas = [0; 6];
        }
    }
    STATUS_SUCCESS
}
