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

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    sids(&mut c)?;
    acls(&mut c)?;
    token(&mut c)
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
