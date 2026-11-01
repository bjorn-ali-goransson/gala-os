//! advapi32's security structures and the process token, as the JVM uses
//! them, with the results real Windows gives (documented layouts and
//! contracts) for a standard user:
//!
//! - SIDs: AllocateAndInitializeSid (BUILTIN\Administrators, S-1-5-32-544),
//!   GetLengthSid, IsValidSid, CopySid (ERROR_INSUFFICIENT_BUFFER when
//!   short), EqualSid, ConvertSidToStringSidW, FreeSid;
//!   LookupAccountSidW (names, domains, SID_NAME_USE, size queries).
//! - ACLs: InitializeAcl, AddAccessAllowedAce (ERROR_ALLOTTED_SPACE_EXCEEDED
//!   when full), AddAce, GetAce, GetAclInformation (revision, sizes).
//! - Security descriptors: InitializeSecurityDescriptor,
//!   SetSecurityDescriptorDacl, GetSecurityDescriptorDacl,
//!   IsValidSecurityDescriptor; SetFileSecurityA.
//! - GetUserNameW; the process token: OpenProcessToken, GetTokenInformation
//!   (TokenUser names this user), LookupPrivilegeValueA (well-known LUIDs,
//!   ERROR_NO_SUCH_PRIVILEGE), AdjustTokenPrivileges of a privilege a
//!   standard user does not hold (TRUE with ERROR_NOT_ALL_ASSIGNED).
//! - CryptAcquireContextA with CRYPT_VERIFYCONTEXT.
//! - What the .NET runtime uses: RegGetValueW (a missing key,
//!   ERROR_INVALID_PARAMETER without a type restriction or with data but
//!   no size), GetSidSubAuthority(Count), RevertToSelf without
//!   impersonation, SetKernelObjectSecurity with a NULL DACL, ETW
//!   providers (EventRegister, EventEnabled, EventWrite(Transfer),
//!   EventUnregister) with no session listening; the thread's activity id
//!   (EventActivityIdControl); software restriction policies (none: a
//!   script is fully trusted; SaferIdentifyLevel and friends);
//!   CreateWellKnownSid; secur32's GetUserNameExW.
//!
//! Prints `advapi32_test: ok` or exits with the number of the failing
//! check.

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
    fn LocalFree(p: *mut c_void) -> *mut c_void;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn AllocateAndInitializeSid(
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
    ) -> Bool;
    fn FreeSid(sid: *mut u8) -> *mut u8;
    fn GetLengthSid(sid: *const u8) -> u32;
    fn IsValidSid(sid: *const u8) -> Bool;
    fn CopySid(len: u32, dst: *mut u8, src: *const u8) -> Bool;
    fn EqualSid(a: *const u8, b: *const u8) -> Bool;
    fn ConvertSidToStringSidW(sid: *const u8, out: *mut *mut u16) -> Bool;
    fn LookupAccountSidW(
        system: *const u16,
        sid: *const u8,
        name: *mut u16,
        name_len: *mut u32,
        domain: *mut u16,
        domain_len: *mut u32,
        kind: *mut u32,
    ) -> Bool;
    fn InitializeAcl(acl: *mut u8, len: u32, revision: u32) -> Bool;
    fn AddAccessAllowedAce(acl: *mut u8, revision: u32, mask: u32, sid: *const u8) -> Bool;
    fn AddAce(acl: *mut u8, revision: u32, index: u32, list: *const u8, len: u32) -> Bool;
    fn GetAce(acl: *const u8, index: u32, ace: *mut *mut u8) -> Bool;
    fn GetAclInformation(acl: *const u8, info: *mut u32, len: u32, class: u32) -> Bool;
    fn InitializeSecurityDescriptor(sd: *mut u8, revision: u32) -> Bool;
    fn SetSecurityDescriptorDacl(sd: *mut u8, present: Bool, acl: *mut u8, defaulted: Bool)
    -> Bool;
    fn GetSecurityDescriptorDacl(
        sd: *const u8,
        present: *mut Bool,
        acl: *mut *mut u8,
        defaulted: *mut Bool,
    ) -> Bool;
    fn IsValidSecurityDescriptor(sd: *const u8) -> Bool;
    fn SetFileSecurityA(path: *const u8, info: u32, sd: *const u8) -> Bool;
    fn GetUserNameW(buf: *mut u16, size: *mut u32) -> Bool;
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> Bool;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        info: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> Bool;
    fn LookupPrivilegeValueA(system: *const u8, name: *const u8, luid: *mut [u32; 2]) -> Bool;
    fn AdjustTokenPrivileges(
        token: Handle,
        disable: Bool,
        new: *const TokenPrivileges,
        len: u32,
        previous: *mut TokenPrivileges,
        ret: *mut u32,
    ) -> Bool;
    fn CryptAcquireContextA(
        provider: *mut usize,
        container: *const u8,
        name: *const u8,
        kind: u32,
        flags: u32,
    ) -> Bool;
    fn CryptReleaseContext(provider: usize, flags: u32) -> Bool;
    fn ConvertStringSidToSidW(text: *const u16, sid: *mut *mut u8) -> Bool;
    fn LookupAccountNameW(
        system: *const u16,
        name: *const u16,
        sid: *mut u8,
        sid_len: *mut u32,
        domain: *mut u16,
        domain_len: *mut u32,
        kind: *mut u32,
    ) -> Bool;
    fn MapGenericMask(mask: *mut u32, mapping: *const [u32; 4]);
    fn GetFileSecurityW(
        path: *const u16,
        info: u32,
        sd: *mut u8,
        len: u32,
        needed: *mut u32,
    ) -> Bool;
    fn GetSecurityDescriptorOwner(sd: *const u8, owner: *mut *mut u8, defaulted: *mut Bool)
    -> Bool;
    fn SetSecurityDescriptorOwner(sd: *mut u8, owner: *mut u8, defaulted: Bool) -> Bool;
    fn DuplicateTokenEx(
        token: Handle,
        access: u32,
        attrs: *mut c_void,
        level: u32,
        kind: u32,
        new: *mut Handle,
    ) -> Bool;
    fn SetThreadToken(thread: *mut Handle, token: Handle) -> Bool;
    fn RegGetValueW(
        key: usize,
        subkey: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> i32;
    fn GetSidSubAuthorityCount(sid: *const u8) -> *mut u8;
    fn GetSidSubAuthority(sid: *const u8, index: u32) -> *mut u32;
    fn RevertToSelf() -> Bool;
    fn ImpersonateSelf(level: u32) -> Bool;
    fn OpenThreadToken(thread: Handle, access: u32, as_self: Bool, token: *mut Handle) -> Bool;
    fn SetKernelObjectSecurity(handle: Handle, info: u32, sd: *const u8) -> Bool;
    fn EventRegister(
        provider: *const [u8; 16],
        callback: *const c_void,
        context: *mut c_void,
        handle: *mut u64,
    ) -> u32;
    fn EventWrite(handle: u64, descriptor: *const [u8; 16], count: u32, data: *const c_void)
    -> u32;
    fn EventWriteTransfer(
        handle: u64,
        descriptor: *const [u8; 16],
        activity: *const [u8; 16],
        related: *const [u8; 16],
        count: u32,
        data: *const c_void,
    ) -> u32;
    fn EventEnabled(handle: u64, descriptor: *const [u8; 16]) -> u8;
    fn EventUnregister(handle: u64) -> u32;
    fn EventActivityIdControl(code: u32, id: *mut [u8; 16]) -> u32;
    fn SaferIdentifyLevel(
        count: u32,
        props: *mut u8,
        level: *mut Handle,
        reserved: *mut c_void,
    ) -> Bool;
    fn SaferGetLevelInformation(
        level: Handle,
        class: u32,
        buf: *mut u32,
        len: u32,
        needed: *mut u32,
    ) -> Bool;
    fn SaferComputeTokenFromLevel(
        level: Handle,
        token: Handle,
        out: *mut Handle,
        flags: u32,
        reserved: *mut c_void,
    ) -> Bool;
    fn SaferCloseLevel(level: Handle) -> Bool;
    fn CreateWellKnownSid(kind: u32, domain: *const u8, sid: *mut u8, size: *mut u32) -> Bool;
}

#[link(name = "secur32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetUserNameExW(format: u32, buf: *mut u16, size: *mut u32) -> u8;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
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

/// TOKEN_PRIVILEGES with one LUID_AND_ATTRIBUTES.
#[repr(C)]
struct TokenPrivileges {
    count: u32,
    luid: [u32; 2],
    attributes: u32,
}

const NT_AUTHORITY: [u8; 6] = [0, 0, 0, 0, 0, 5];
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_ALLOTTED_SPACE_EXCEEDED: u32 = 1344;
const ERROR_NO_SUCH_PRIVILEGE: u32 = 1313;
const ERROR_NOT_ALL_ASSIGNED: u32 = 1300;
const ACL_REVISION: u32 = 2;
const SID_TYPE_USER: u32 = 1;
const SID_TYPE_ALIAS: u32 = 4;
const TOKEN_QUERY_ADJUST: u32 = 0x0008 | 0x0020;
const SE_PRIVILEGE_ENABLED: u32 = 2;

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

struct Checks(u32);

impl Checks {
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"advapi32_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// The NUL-terminated UTF-16 string at `p` is `want`.
    fn text(&mut self, p: *const u16, want: &str) -> Result<(), u32> {
        // SAFETY: a NUL-terminated string the system wrote.
        let same = unsafe {
            want.bytes().enumerate().all(|(i, b)| *p.add(i) == u16::from(b))
                && *p.add(want.len()) == 0
        };
        // SAFETY: as above.
        self.ok(same, unsafe { *p }.into())
    }
}

fn sids(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: advapi32 calls with buffers of ours.
    unsafe {
        let mut sid = core::ptr::null_mut();
        c.ok(
            AllocateAndInitializeSid(&NT_AUTHORITY, 2, 32, 544, 0, 0, 0, 0, 0, 0, &mut sid) != 0,
            GetLastError().into(),
        )?;
        c.eq(GetLengthSid(sid).into(), 16)?;
        c.ok(IsValidSid(sid) != 0, 0)?;
        let mut copy = [0u8; 16];
        SetLastError(0);
        c.eq(CopySid(8, copy.as_mut_ptr(), sid) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_INSUFFICIENT_BUFFER.into())?;
        c.ok(CopySid(16, copy.as_mut_ptr(), sid) != 0, 0)?;
        c.ok(EqualSid(copy.as_ptr(), sid) != 0, 0)?;
        let mut text = core::ptr::null_mut();
        c.ok(ConvertSidToStringSidW(sid, &mut text) != 0, GetLastError().into())?;
        c.text(text, "S-1-5-32-544")?;
        LocalFree(text.cast());

        // Names: a size query first.
        let (mut name, mut domain) = ([0u16; 32], [0u16; 32]);
        let (mut n, mut d, mut kind) = (0u32, 0u32, 0u32);
        c.eq(
            LookupAccountSidW(
                core::ptr::null(),
                sid,
                core::ptr::null_mut(),
                &mut n,
                core::ptr::null_mut(),
                &mut d,
                &mut kind,
            ) as u64,
            0,
        )?;
        c.eq(GetLastError().into(), ERROR_INSUFFICIENT_BUFFER.into())?;
        c.eq(n.into(), 15)?; // "Administrators" and its NUL
        c.eq(d.into(), 8)?; // "BUILTIN"
        c.ok(
            LookupAccountSidW(
                core::ptr::null(),
                sid,
                name.as_mut_ptr(),
                &mut n,
                domain.as_mut_ptr(),
                &mut d,
                &mut kind,
            ) != 0,
            GetLastError().into(),
        )?;
        c.text(name.as_ptr(), "Administrators")?;
        c.text(domain.as_ptr(), "BUILTIN")?;
        c.eq(n.into(), 14)?; // without the NUL now
        c.eq(kind.into(), SID_TYPE_ALIAS.into())?;
        c.eq(FreeSid(sid) as u64, 0)?;
    }
    Ok(())
}

fn acls(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: advapi32 calls with buffers of ours.
    unsafe {
        let mut sid = core::ptr::null_mut();
        c.ok(
            AllocateAndInitializeSid(&NT_AUTHORITY, 2, 32, 544, 0, 0, 0, 0, 0, 0, &mut sid) != 0,
            0,
        )?;
        let mut acl = [0u32; 32]; // 128 bytes, aligned
        let a = acl.as_mut_ptr().cast::<u8>();
        c.ok(InitializeAcl(a, 128, ACL_REVISION) != 0, GetLastError().into())?;
        let mut info = [0u32; 3];
        c.ok(GetAclInformation(a, info.as_mut_ptr(), 12, 2) != 0, 0)?; // AclSizeInformation
        c.ok(info == [0, 8, 120], u64::from(info[1]))?;
        c.ok(AddAccessAllowedAce(a, ACL_REVISION, 0x1f_01ff, sid) != 0, GetLastError().into())?;
        c.ok(GetAclInformation(a, info.as_mut_ptr(), 12, 2) != 0, 0)?;
        c.ok(info == [1, 32, 96], u64::from(info[1]))?;
        let mut ace = core::ptr::null_mut();
        c.ok(GetAce(a, 0, &mut ace) != 0, 0)?;
        c.eq((*ace).into(), 0)?; // ACCESS_ALLOWED_ACE_TYPE
        c.eq(u16::from_le_bytes([*ace.add(2), *ace.add(3)]).into(), 24)?; // AceSize
        c.eq(
            u32::from_le_bytes([*ace.add(4), *ace.add(5), *ace.add(6), *ace.add(7)]).into(),
            0x1f_01ff,
        )?;
        c.ok(EqualSid(ace.add(8), sid) != 0, 0)?;
        // AddAce appends a copy at MAXDWORD.
        let mut copy = [0u8; 24];
        core::ptr::copy_nonoverlapping(ace, copy.as_mut_ptr(), 24);
        c.ok(AddAce(a, ACL_REVISION, u32::MAX, copy.as_ptr(), 24) != 0, GetLastError().into())?;
        c.ok(GetAclInformation(a, info.as_mut_ptr(), 12, 2) != 0, 0)?;
        c.ok(info == [2, 56, 72], u64::from(info[1]))?;
        c.ok(GetAclInformation(a, info.as_mut_ptr(), 4, 1) != 0, 0)?; // AclRevisionInformation
        c.eq(info[0].into(), 2)?;
        // A full ACL.
        let mut small = [0u32; 4];
        let s = small.as_mut_ptr().cast::<u8>();
        c.ok(InitializeAcl(s, 16, ACL_REVISION) != 0, 0)?;
        c.eq(AddAccessAllowedAce(s, ACL_REVISION, 1, sid) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_ALLOTTED_SPACE_EXCEEDED.into())?;

        // A security descriptor with that DACL.
        let mut sd = [0u64; 5];
        let p = sd.as_mut_ptr().cast::<u8>();
        c.ok(InitializeSecurityDescriptor(p, 1) != 0, 0)?;
        c.ok(IsValidSecurityDescriptor(p) != 0, 0)?;
        c.ok(SetSecurityDescriptorDacl(p, 1, a, 0) != 0, GetLastError().into())?;
        let (mut present, mut dacl, mut defaulted) = (0, core::ptr::null_mut(), 1);
        c.ok(GetSecurityDescriptorDacl(p, &mut present, &mut dacl, &mut defaulted) != 0, 0)?;
        c.ok(present == 1 && dacl == a && defaulted == 0, present as u64)?;
        let mut name = [0u16; 32];
        for (d, b) in name.iter_mut().zip(b"C:\\gala-advapi32-test.txt") {
            *d = u16::from(*b);
        }
        let h = CreateFileW(
            name.as_ptr(),
            0xc000_0000,
            0,
            core::ptr::null_mut(),
            2,
            0,
            core::ptr::null_mut(),
        );
        c.ok(h as isize != -1, GetLastError().into())?;
        CloseHandle(h);
        c.ok(
            SetFileSecurityA(c"C:\\gala-advapi32-test.txt".as_ptr().cast(), 4, p) != 0,
            GetLastError().into(),
        )?;
        FreeSid(sid);
    }
    Ok(())
}

fn token(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: advapi32 calls with buffers of ours.
    unsafe {
        let mut name = [0u16; 16];
        let mut size = 16;
        c.ok(GetUserNameW(name.as_mut_ptr(), &mut size) != 0, GetLastError().into())?;
        c.text(name.as_ptr(), "gala")?;
        c.eq(size.into(), 5)?;

        let mut token = core::ptr::null_mut();
        c.ok(
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY_ADJUST, &mut token) != 0,
            GetLastError().into(),
        )?;
        // TokenUser: TOKEN_USER (SID_AND_ATTRIBUTES), then the SID.
        let mut user = [0u64; 16];
        let mut ret = 0;
        c.ok(
            GetTokenInformation(token, 1, user.as_mut_ptr().cast(), 128, &mut ret) != 0,
            GetLastError().into(),
        )?;
        let sid = user[0] as *const u8;
        c.ok(IsValidSid(sid) != 0, 0)?;
        let (mut account, mut domain) = ([0u16; 32], [0u16; 32]);
        let (mut n, mut d, mut kind) = (32u32, 32u32, 0u32);
        c.ok(
            LookupAccountSidW(
                core::ptr::null(),
                sid,
                account.as_mut_ptr(),
                &mut n,
                domain.as_mut_ptr(),
                &mut d,
                &mut kind,
            ) != 0,
            GetLastError().into(),
        )?;
        c.text(account.as_ptr(), "gala")?;
        c.eq(kind.into(), SID_TYPE_USER.into())?;

        let mut luid = [0u32; 2];
        c.ok(
            LookupPrivilegeValueA(
                core::ptr::null(),
                c"SeLockMemoryPrivilege".as_ptr().cast(),
                &mut luid,
            ) != 0,
            0,
        )?;
        c.ok(luid == [4, 0], luid[0].into())?;
        c.ok(
            LookupPrivilegeValueA(
                core::ptr::null(),
                c"SeDebugPrivilege".as_ptr().cast(),
                &mut luid,
            ) != 0,
            0,
        )?;
        c.ok(luid == [20, 0], luid[0].into())?;
        c.eq(
            LookupPrivilegeValueA(
                core::ptr::null(),
                c"SeNoSuchPrivilege".as_ptr().cast(),
                &mut luid,
            ) as u64,
            0,
        )?;
        c.eq(GetLastError().into(), ERROR_NO_SUCH_PRIVILEGE.into())?;
        let tp = TokenPrivileges { count: 1, luid: [4, 0], attributes: SE_PRIVILEGE_ENABLED };
        SetLastError(0);
        c.ok(
            AdjustTokenPrivileges(token, 0, &tp, 0, core::ptr::null_mut(), core::ptr::null_mut())
                != 0,
            GetLastError().into(),
        )?;
        c.eq(GetLastError().into(), ERROR_NOT_ALL_ASSIGNED.into())?;
        c.ok(CloseHandle(token) != 0, GetLastError().into())?;

        let mut provider = 0;
        c.ok(
            CryptAcquireContextA(
                &mut provider,
                core::ptr::null(),
                core::ptr::null(),
                1,
                0xf000_0000,
            ) != 0,
            GetLastError().into(),
        )?;
        c.ok(CryptReleaseContext(provider, 0) != 0, 0)?;
    }
    Ok(())
}

/// A NUL-terminated UTF-16 copy of an ASCII string.
fn w(s: &str) -> [u16; 64] {
    let mut b = [0u16; 64];
    for (d, ch) in b.iter_mut().zip(s.bytes()) {
        *d = ch.into();
    }
    b
}

/// What the JDK's net and nio libraries import.
fn more(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: advapi32 calls with buffers of ours.
    unsafe {
        let mut sid = core::ptr::null_mut();
        c.ok(
            ConvertStringSidToSidW(w("S-1-5-32-544").as_ptr(), &mut sid) != 0,
            GetLastError().into(),
        )?;
        let mut admins = core::ptr::null_mut();
        AllocateAndInitializeSid(&NT_AUTHORITY, 2, 32, 544, 0, 0, 0, 0, 0, 0, &mut admins);
        c.ok(EqualSid(sid, admins) != 0, 0)?;
        LocalFree(sid.cast());
        SetLastError(0);
        c.eq(ConvertStringSidToSidW(w("X-1-5").as_ptr(), &mut sid) as u64, 0)?;
        c.eq(GetLastError().into(), 1337)?; // ERROR_INVALID_SID

        // LookupAccountNameW: a size query, then the user's SID.
        let (mut buf, mut domain) = ([0u8; 68], [0u16; 32]);
        let (mut n, mut d, mut kind) = (0u32, 0u32, 0u32);
        c.eq(
            LookupAccountNameW(
                core::ptr::null(),
                w("gala").as_ptr(),
                core::ptr::null_mut(),
                &mut n,
                core::ptr::null_mut(),
                &mut d,
                &mut kind,
            ) as u64,
            0,
        )?;
        c.eq(GetLastError().into(), ERROR_INSUFFICIENT_BUFFER.into())?;
        c.eq(n.into(), 28)?; // S-1-5-21-x-y-z-1001
        c.eq(d.into(), 5)?; // "GALA" and its NUL
        c.ok(
            LookupAccountNameW(
                core::ptr::null(),
                w("gala").as_ptr(),
                buf.as_mut_ptr(),
                &mut n,
                domain.as_mut_ptr(),
                &mut d,
                &mut kind,
            ) != 0,
            GetLastError().into(),
        )?;
        c.eq(kind.into(), SID_TYPE_USER.into())?;
        c.text(domain.as_ptr(), "GALA")?;
        let mut text = core::ptr::null_mut();
        ConvertSidToStringSidW(buf.as_ptr(), &mut text);
        c.ok(*text.add(6) == u16::from(b'2') && *text.add(7) == u16::from(b'1'), 0)?; // S-1-5-21-
        LocalFree(text.cast());
        let (mut n, mut d) = (68u32, 32u32);
        c.ok(
            LookupAccountNameW(
                core::ptr::null(),
                w("Administrators").as_ptr(),
                buf.as_mut_ptr(),
                &mut n,
                domain.as_mut_ptr(),
                &mut d,
                &mut kind,
            ) != 0,
            0,
        )?;
        c.ok(EqualSid(buf.as_ptr(), admins) != 0, 0)?;
        c.eq(kind.into(), SID_TYPE_ALIAS.into())?;
        let (mut n, mut d) = (68u32, 32u32);
        c.eq(
            LookupAccountNameW(
                core::ptr::null(),
                w("nobody-here").as_ptr(),
                buf.as_mut_ptr(),
                &mut n,
                domain.as_mut_ptr(),
                &mut d,
                &mut kind,
            ) as u64,
            0,
        )?;
        c.eq(GetLastError().into(), 1332)?; // ERROR_NONE_MAPPED

        // MapGenericMask: GENERIC_READ becomes the mapping's read rights.
        let mut mask = 0x8000_0000u32;
        MapGenericMask(&mut mask, &[1, 2, 4, 8]);
        c.eq(mask.into(), 1)?;

        // A file's owner.
        let path = w("C:\\gala-advapi32-test.txt");
        let mut needed = 0u32;
        c.eq(GetFileSecurityW(path.as_ptr(), 1, core::ptr::null_mut(), 0, &mut needed) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_INSUFFICIENT_BUFFER.into())?;
        let mut sd = [0u64; 32];
        c.ok(needed > 0 && needed as usize <= 256, needed.into())?;
        c.ok(
            GetFileSecurityW(path.as_ptr(), 1, sd.as_mut_ptr().cast(), 256, &mut needed) != 0,
            GetLastError().into(),
        )?;
        let (mut owner, mut defaulted) = (core::ptr::null_mut(), 1);
        c.ok(GetSecurityDescriptorOwner(sd.as_ptr().cast(), &mut owner, &mut defaulted) != 0, 0)?;
        c.ok(IsValidSid(owner) != 0, 0)?;
        // An absolute descriptor's owner.
        let mut abs = [0u64; 5];
        InitializeSecurityDescriptor(abs.as_mut_ptr().cast(), 1);
        c.ok(SetSecurityDescriptorOwner(abs.as_mut_ptr().cast(), admins, 0) != 0, 0)?;
        c.ok(GetSecurityDescriptorOwner(abs.as_ptr().cast(), &mut owner, &mut defaulted) != 0, 0)?;
        c.ok(owner == admins && defaulted == 0, 0)?;
        FreeSid(admins);

        // An impersonation token from the process token.
        let mut token = core::ptr::null_mut();
        c.ok(OpenProcessToken(GetCurrentProcess(), 0x2 | 0x8, &mut token) != 0, 0)?; // DUPLICATE | QUERY
        let mut copy = core::ptr::null_mut();
        c.ok(
            DuplicateTokenEx(token, 0x0200_0000, core::ptr::null_mut(), 2, 2, &mut copy) != 0,
            GetLastError().into(),
        )?;
        c.ok(SetThreadToken(core::ptr::null_mut(), copy) != 0, GetLastError().into())?;
        c.ok(SetThreadToken(core::ptr::null_mut(), core::ptr::null_mut()) != 0, 0)?; // revert
        CloseHandle(copy);
        CloseHandle(token);
    }
    Ok(())
}

/// What the .NET runtime (hostfxr, CoreCLR) uses.
fn clr(c: &mut Checks) -> Result<(), u32> {
    const HKEY_LOCAL_MACHINE: usize = 0x8000_0002;
    const RRF_RT_REG_SZ: u32 = 2;
    const ERROR_FILE_NOT_FOUND: u64 = 2;
    const ERROR_INVALID_PARAMETER: u64 = 87;
    // SAFETY: advapi32 calls with buffers of ours.
    unsafe {
        // RegGetValueW: a key that does not exist; no type restriction.
        let key = w("SOFTWARE\\gala-test\\no-such-key");
        let value = w("InstallLocation");
        let mut buf = [0u16; 64];
        let mut size = 128u32;
        let r = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            core::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        );
        c.eq(r as u64, ERROR_FILE_NOT_FOUND)?;
        let null = core::ptr::null_mut();
        let r = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            0,
            null,
            null.cast(),
            null,
        );
        c.eq(r as u64, ERROR_INVALID_PARAMETER)?;
        // Data without a size.
        let r = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            null,
            buf.as_mut_ptr().cast(),
            core::ptr::null_mut(),
        );
        c.eq(r as u64, ERROR_INVALID_PARAMETER)?;

        // S-1-5-32-544: two subauthorities, 32 and 544.
        let mut admins = core::ptr::null_mut();
        AllocateAndInitializeSid(&NT_AUTHORITY, 2, 32, 544, 0, 0, 0, 0, 0, 0, &mut admins);
        c.eq((*GetSidSubAuthorityCount(admins)).into(), 2)?;
        c.eq((*GetSidSubAuthority(admins, 0)).into(), 32)?;
        c.eq((*GetSidSubAuthority(admins, 1)).into(), 544)?;
        *GetSidSubAuthority(admins, 1) = 545; // writable: it is the SID's own
        let mut text = core::ptr::null_mut();
        ConvertSidToStringSidW(admins, &mut text);
        c.text(text, "S-1-5-32-545")?;
        LocalFree(text.cast());
        FreeSid(admins);

        // Not impersonating: no thread token (ERROR_NO_TOKEN); RevertToSelf
        // succeeds all the same.
        let mut thread_token = core::ptr::null_mut();
        c.eq(OpenThreadToken(GetCurrentThread(), 8, 1, &mut thread_token) as u64, 0)?;
        c.eq(GetLastError().into(), 1008)?;
        c.ok(RevertToSelf() != 0, GetLastError().into())?;
        // ImpersonateSelf (SecurityImpersonation): the thread has a token,
        // of the same user, until RevertToSelf.
        c.ok(ImpersonateSelf(2) != 0, GetLastError().into())?;
        c.ok(
            OpenThreadToken(GetCurrentThread(), 8, 1, &mut thread_token) != 0,
            GetLastError().into(),
        )?;
        let mut user = [0u64; 16];
        let mut ret = 0;
        c.ok(
            GetTokenInformation(thread_token, 1, user.as_mut_ptr().cast(), 128, &mut ret) != 0,
            0,
        )?;
        let mut text = core::ptr::null_mut();
        ConvertSidToStringSidW(user[0] as *const u8, &mut text);
        c.ok(*text == u16::from(b'S'), 0)?;
        LocalFree(text.cast());
        CloseHandle(thread_token);
        c.ok(RevertToSelf() != 0, GetLastError().into())?;
        c.eq(OpenThreadToken(GetCurrentThread(), 8, 1, &mut thread_token) as u64, 0)?;
        c.eq(GetLastError().into(), 1008)?;

        // A NULL DACL on this process (full access to everyone).
        let mut sd = [0u64; 5];
        InitializeSecurityDescriptor(sd.as_mut_ptr().cast(), 1);
        SetSecurityDescriptorDacl(sd.as_mut_ptr().cast(), 1, core::ptr::null_mut(), 0);
        c.ok(
            SetKernelObjectSecurity(GetCurrentProcess(), 4, sd.as_ptr().cast()) != 0,
            GetLastError().into(),
        )?; // DACL_SECURITY_INFORMATION

        // ETW: registering a provider works without a listening session;
        // events then go nowhere, successfully.
        let provider = [
            0x6e, 0x2c, 0x4d, 0x52, 0x9a, 0x3b, 0x4f, 0x41, 0x8a, 0x55, 0x1c, 0x6d, 0x0e, 0x7f,
            0x3a, 0x21,
        ];
        let mut handle = 0u64;
        c.eq(
            EventRegister(&provider, core::ptr::null(), core::ptr::null_mut(), &mut handle).into(),
            0,
        )?;
        c.ok(handle != 0, 0)?;
        let descriptor = [1, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]; // id 1, level 4
        c.eq(EventEnabled(handle, &descriptor).into(), 0)?;
        c.eq(EventWrite(handle, &descriptor, 0, core::ptr::null()).into(), 0)?;
        let activity = [7u8; 16];
        let r = EventWriteTransfer(
            handle,
            &descriptor,
            &activity,
            core::ptr::null(),
            0,
            core::ptr::null(),
        );
        c.eq(r.into(), 0)?;
        c.eq(EventUnregister(handle).into(), 0)?;

        // The thread's activity id: none at first; CREATE makes new ones
        // (not stored), SET stores, GET reads, GET_SET swaps.
        let mut id = [9u8; 16];
        c.eq(EventActivityIdControl(1, &mut id).into(), 0)?; // EVENT_ACTIVITY_CTRL_GET_ID
        c.ok(id == [0; 16], id[0].into())?;
        let (mut a, mut b) = ([0u8; 16], [0u8; 16]);
        c.eq(EventActivityIdControl(3, &mut a).into(), 0)?; // CREATE_ID
        c.eq(EventActivityIdControl(3, &mut b).into(), 0)?;
        c.ok(a != b && a != [0; 16], 0)?;
        c.eq(EventActivityIdControl(1, &mut id).into(), 0)?;
        c.ok(id == [0; 16], 0)?;
        let mut set = a;
        c.eq(EventActivityIdControl(2, &mut set).into(), 0)?; // SET_ID
        c.eq(EventActivityIdControl(1, &mut id).into(), 0)?;
        c.ok(id == a, 0)?;
        let mut swap = b;
        c.eq(EventActivityIdControl(4, &mut swap).into(), 0)?; // GET_SET_ID
        c.ok(swap == a, 0)?;
        c.eq(EventActivityIdControl(1, &mut id).into(), 0)?;
        c.ok(id == b, 0)?;
        let mut zero = [0u8; 16];
        EventActivityIdControl(2, &mut zero);
        c.eq(EventActivityIdControl(9, &mut id).into(), 87)?; // ERROR_INVALID_PARAMETER

        // Software restriction policies: none are configured, so a script
        // is fully trusted and runs with the caller's own token.
        let path = w("C:\\gala\\script.ps1");
        let mut props = [0u64; 17]; // SAFER_CODE_PROPERTIES (V1, 136 bytes)
        props[0] = 136 | (1 << 32); // cbSize, SAFER_CRITERIA_IMAGEPATH
        props[1] = path.as_ptr() as u64;
        let mut level = core::ptr::null_mut();
        let ok =
            SaferIdentifyLevel(1, props.as_mut_ptr().cast(), &mut level, core::ptr::null_mut());
        c.ok(ok != 0 && !level.is_null(), GetLastError().into())?;
        let (mut id, mut needed) = (0u32, 0u32);
        c.ok(SaferGetLevelInformation(level, 1, &mut id, 4, &mut needed) != 0, 0)?; // SaferObjectLevelId
        c.eq(id.into(), 0x4_0000)?; // SAFER_LEVELID_FULLYTRUSTED
        let mut token = 7 as Handle;
        let flags = 1; // SAFER_TOKEN_NULL_IF_EQUAL
        c.ok(
            SaferComputeTokenFromLevel(
                level,
                core::ptr::null_mut(),
                &mut token,
                flags,
                core::ptr::null_mut(),
            ) != 0,
            0,
        )?;
        c.ok(token.is_null(), token as u64)?;
        c.ok(SaferCloseLevel(level) != 0, 0)?;

        // Well-known SIDs: the size first, then the SID.
        let mut size = 0u32;
        c.eq(
            CreateWellKnownSid(26, core::ptr::null(), core::ptr::null_mut(), &mut size) as u64,
            0,
        )?;
        c.eq(GetLastError().into(), ERROR_INSUFFICIENT_BUFFER.into())?;
        c.eq(size.into(), 16)?; // two subauthorities
        // GetUserNameExW: NameSamCompatible is MACHINE\user; a local
        // account has no user principal name.
        let mut name = [0u16; 64];
        let mut size = 3u32;
        c.eq(GetUserNameExW(2, name.as_mut_ptr(), &mut size).into(), 0)?;
        c.eq(GetLastError().into(), 234)?; // ERROR_MORE_DATA
        c.eq(size.into(), 10)?; // "GALA\gala" and the NUL
        let mut size = 64u32;
        c.eq(GetUserNameExW(2, name.as_mut_ptr(), &mut size).into(), 1)?;
        c.eq(size.into(), 9)?;
        c.text(name.as_ptr(), "GALA\\gala")?;
        let mut size = 64u32;
        c.eq(GetUserNameExW(8, name.as_mut_ptr(), &mut size).into(), 0)?; // NameUserPrincipal
        c.eq(GetLastError().into(), 1332)?; // ERROR_NONE_MAPPED

        for (kind, text) in [
            (26u32, "S-1-5-32-544"),
            (1, "S-1-1-0"),
            (22, "S-1-5-18"),
            (17, "S-1-5-11"),
            (27, "S-1-5-32-545"),
            (33, "S-1-5-32-551"), // WinBuiltinBackupOperatorsSid
            (51, "S-1-5-64-10"),  // WinNTLMAuthenticationSid
            (57, "S-1-5-32-558"), // WinBuiltinPerfMonitoringUsersSid
            (62, "S-1-5-32-568"), // WinBuiltinIUsersSid
            (63, "S-1-5-17"),     // WinIUserSid
            (71, "S-1-3-4"),      // WinCreatorOwnerRightsSid
        ] {
            let mut sid = [0u8; 68];
            let mut size = 68u32;
            c.ok(
                CreateWellKnownSid(kind, core::ptr::null(), sid.as_mut_ptr(), &mut size) != 0,
                kind.into(),
            )?;
            let mut s = core::ptr::null_mut();
            ConvertSidToStringSidW(sid.as_ptr(), &mut s);
            c.text(s, text)?;
            LocalFree(s.cast());
        }
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    sids(&mut c)?;
    acls(&mut c)?;
    token(&mut c)?;
    more(&mut c)?;
    clr(&mut c)
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"advapi32_test: ok\r\n");
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
