//! A 32-bit (x86) program on advapi32 (ROADMAP WOW stage 3): the registry,
//! tokens, SIDs, security descriptors and the CryptoAPI's random numbers
//! through the 32-bit advapi32, with the results of 64-bit Windows' WOW64.
//! The structures are checked in their x86 layouts (4-byte pointers).
//!
//! - The registry: RegCreateKeyExW under HKEY_CURRENT_USER
//!   (REG_CREATED_NEW_KEY), RegSetValueExW and RegQueryValueExW of REG_SZ
//!   and REG_DWORD (the size query, ERROR_MORE_DATA for a short buffer),
//!   RegCreateKeyExW of a subkey, RegEnumKeyExW (its name, then
//!   ERROR_NO_MORE_ITEMS), RegDeleteKeyW; HKEY_LOCAL_MACHINE (a 32-bit
//!   HKEY, 0x80000002) opens SOFTWARE.
//! - The registry's 32-bit view ("Registry Redirector" on Microsoft
//!   Learn): a key a 32-bit program creates under HKLM\SOFTWARE is
//!   HKLM\SOFTWARE\WOW6432Node's in the 64-bit view (KEY_WOW64_64KEY) and
//!   not in the 64-bit SOFTWARE; KEY_WOW64_64KEY creates in the 64-bit
//!   view, which the 32-bit view does not see; KEY_WOW64_32KEY is the
//!   32-bit view; WOW6432Node itself opens from the 32-bit view (the same
//!   key); HKCU\Software is shared; RegDeleteKeyExW deletes in the view
//!   its flag names.
//! - Tokens: OpenProcessToken; GetTokenInformation's TokenUser (the size
//!   query: ERROR_INSUFFICIENT_BUFFER; the SID pointer at 0 into the
//!   buffer, the attributes at 4), TokenGroups (the count, then 8-byte
//!   SID_AND_ATTRIBUTES whose SIDs are in the buffer; Everyone enabled);
//!   CheckTokenMembership of Everyone; DuplicateTokenEx with an x86
//!   SECURITY_ATTRIBUTES (bInheritHandle at 8: an inheritable handle) and
//!   TokenType of the copy.
//! - SIDs: ConvertSidToStringSidW of the user (S-1-5-...),
//!   LookupAccountSidW of the user (SidTypeUser, GetUserNameW's name) and
//!   of S-1-5-32-544 (BUILTIN\Administrators, SidTypeAlias),
//!   ConvertStringSidToSidW.
//! - Security descriptors: an absolute one is 20 bytes (Owner, Group,
//!   Sacl, Dacl at 4, 8, 12, 16): InitializeSecurityDescriptor writes no
//!   further, SetSecurityDescriptorOwner/Dacl store there and the Get
//!   functions read there; a self-relative one from
//!   ConvertStringSecurityDescriptorToSecurityDescriptorW (owner
//!   BUILTIN\Administrators, a DACL whose ACE is Everyone's;
//!   GetSecurityDescriptorLength its length). SetEntriesInAclW with x86
//!   EXPLICIT_ACCESS_W (32 bytes, the trustee at 12, its name at 28);
//!   BuildExplicitAccessWithNameW writes 32 bytes.
//! - CryptAcquireContextW (CRYPT_VERIFYCONTEXT), CryptGenRandom,
//!   SystemFunction036 (bcryptprimitives' ProcessPrng).
//!
//! Prints `wow64_adv_test: ok` and exits with 0, or exits with the number
//! of the failing check (`(line 0x...) wow64_adv_test: check N failed`
//! first).

#![no_std]
#![no_main]

use core::ptr::{null, null_mut};

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn GetLastError() -> u32;
    fn GetCurrentProcess() -> Handle;
    fn GetHandleInformation(h: Handle, flags: *mut u32) -> i32;
    fn LocalFree(p: *mut u8) -> *mut u8;
    fn ExitProcess(code: u32) -> !;
}

#[link(name = "advapi32", kind = "raw-dylib", import_name_type = "undecorated")]
unsafe extern "system" {
    fn RegCreateKeyExW(
        key: usize,
        sub: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        access: u32,
        security: usize,
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
    fn RegSetValueExW(
        key: usize,
        name: *const u16,
        reserved: u32,
        kind: u32,
        data: *const u8,
        size: u32,
    ) -> i32;
    fn RegQueryValueExW(
        key: usize,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        size: *mut u32,
    ) -> i32;
    fn RegEnumKeyExW(
        key: usize,
        index: u32,
        name: *mut u16,
        len: *mut u32,
        reserved: *mut u32,
        class: *mut u16,
        class_len: *mut u32,
        time: *mut u64,
    ) -> i32;
    fn RegDeleteKeyW(key: usize, sub: *const u16) -> i32;
    fn RegDeleteKeyExW(key: usize, sub: *const u16, access: u32, reserved: u32) -> i32;
    fn RegCloseKey(key: usize) -> i32;
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        out: *mut u8,
        length: u32,
        ret: *mut u32,
    ) -> i32;
    fn DuplicateTokenEx(
        token: Handle,
        access: u32,
        attributes: *const SecurityAttributes,
        level: u32,
        kind: u32,
        new: *mut Handle,
    ) -> i32;
    fn CheckTokenMembership(token: Handle, sid: *const u8, member: *mut i32) -> i32;
    fn IsValidSid(sid: *const u8) -> i32;
    fn GetLengthSid(sid: *const u8) -> u32;
    fn EqualSid(a: *const u8, b: *const u8) -> i32;
    fn ConvertSidToStringSidW(sid: *const u8, out: *mut *mut u16) -> i32;
    fn ConvertStringSidToSidW(text: *const u16, sid: *mut *mut u8) -> i32;
    fn LookupAccountSidW(
        system: *const u16,
        sid: *const u8,
        name: *mut u16,
        name_len: *mut u32,
        domain: *mut u16,
        domain_len: *mut u32,
        kind: *mut u32,
    ) -> i32;
    fn GetUserNameW(buffer: *mut u16, size: *mut u32) -> i32;
    fn InitializeSecurityDescriptor(sd: *mut u8, revision: u32) -> i32;
    fn InitializeAcl(acl: *mut u8, length: u32, revision: u32) -> i32;
    fn AddAccessAllowedAce(acl: *mut u8, revision: u32, mask: u32, sid: *const u8) -> i32;
    fn GetAce(acl: *const u8, index: u32, ace: *mut *mut u8) -> i32;
    fn SetSecurityDescriptorDacl(sd: *mut u8, present: i32, dacl: *mut u8, defaulted: i32) -> i32;
    fn GetSecurityDescriptorDacl(
        sd: *const u8,
        present: *mut i32,
        dacl: *mut *mut u8,
        defaulted: *mut i32,
    ) -> i32;
    fn SetSecurityDescriptorOwner(sd: *mut u8, owner: *mut u8, defaulted: i32) -> i32;
    fn GetSecurityDescriptorOwner(sd: *const u8, owner: *mut *mut u8, defaulted: *mut i32) -> i32;
    fn SetSecurityDescriptorGroup(sd: *mut u8, group: *mut u8, defaulted: i32) -> i32;
    fn GetSecurityDescriptorGroup(sd: *const u8, group: *mut *mut u8, defaulted: *mut i32) -> i32;
    fn GetSecurityDescriptorLength(sd: *const u8) -> u32;
    fn IsValidSecurityDescriptor(sd: *const u8) -> i32;
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        text: *const u16,
        revision: u32,
        out: *mut *mut u8,
        size: *mut u32,
    ) -> i32;
    fn SetEntriesInAclW(
        count: u32,
        entries: *const ExplicitAccess,
        old: *const u8,
        new: *mut *mut u8,
    ) -> u32;
    fn BuildExplicitAccessWithNameW(
        ea: *mut ExplicitAccess,
        name: *const u16,
        permissions: u32,
        mode: u32,
        inheritance: u32,
    );
    fn CryptAcquireContextW(
        provider: *mut usize,
        container: *const u16,
        name: *const u16,
        kind: u32,
        flags: u32,
    ) -> i32;
    fn CryptGenRandom(provider: usize, length: u32, buffer: *mut u8) -> i32;
    fn CryptReleaseContext(provider: usize, flags: u32) -> i32;
    fn SystemFunction036(buffer: *mut u8, length: u32) -> u8;
}

// The memory routines the compiler calls: ntrt's x86 runtime (nothing
// else of it is used).
use ntrt as _;

/// SECURITY_ATTRIBUTES (x86: 12 bytes).
#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut u8,
    inherit: i32,
}

/// TRUSTEE_W (x86: 20 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
struct Trustee {
    multiple: *mut Trustee,
    operation: u32,
    form: u32,
    kind: u32,
    name: *const u16,
}

/// EXPLICIT_ACCESS_W (x86: 32 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
struct ExplicitAccess {
    permissions: u32,
    mode: u32,
    inheritance: u32,
    trustee: Trustee,
}

const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
const HKEY_CURRENT_USER: usize = 0x8000_0001;
const HKEY_LOCAL_MACHINE: usize = 0x8000_0002;
const KEY_READ: u32 = 0x2_0019;
const KEY_ALL_ACCESS: u32 = 0xf_003f;
const KEY_WOW64_64KEY: u32 = 0x0100;
const KEY_WOW64_32KEY: u32 = 0x0200;
const REG_SZ: u32 = 1;
const REG_DWORD: u32 = 4;
const REG_CREATED_NEW_KEY: u32 = 1;
const REG_OPENED_EXISTING_KEY: u32 = 2;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_NO_MORE_ITEMS: u32 = 259;
const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_DUPLICATE: u32 = 0x0002;
const TOKEN_USER: u32 = 1;
const TOKEN_GROUPS: u32 = 2;
const TOKEN_TYPE: u32 = 8;
const SE_GROUP_ENABLED: u32 = 0x4;
const SID_TYPE_USER: u32 = 1;
const SID_TYPE_ALIAS: u32 = 4;
const GENERIC_ALL: u32 = 0x1000_0000;
const GENERIC_READ: u32 = 0x8000_0000;
const SE_SELF_RELATIVE: u16 = 0x8000;
const GRANT_ACCESS: u32 = 1;
const TRUSTEE_IS_SID: u32 = 0;
const TRUSTEE_IS_NAME: u32 = 1;
const CRYPT_VERIFYCONTEXT: u32 = 0xf000_0000;
const PROV_RSA_FULL: u32 = 1;
/// S-1-1-0 (Everyone).
const EVERYONE: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];

/// A NUL-terminated UTF-16 copy of ASCII `s`.
fn wide(s: &str) -> [u16; 64] {
    let mut out = [0u16; 64];
    for (d, b) in out.iter_mut().zip(s.bytes()) {
        *d = u16::from(b);
    }
    out
}

/// Whether the NUL-terminated `got` is ASCII `want`.
fn same(got: &[u16], want: &str) -> bool {
    got.len() > want.len()
        && got[want.len()] == 0
        && got.iter().zip(want.bytes()).all(|(&a, b)| a == u16::from(b))
}

/// Whether the NUL-terminated string at `p` starts with ASCII `want`.
fn starts(p: *const u16, want: &str) -> bool {
    // SAFETY: a NUL-terminated string; reading stops at the first mismatch.
    want.bytes().enumerate().all(|(i, b)| unsafe { *p.add(i) } == u16::from(b))
}

fn print(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid arguments.
    unsafe {
        WriteFile(
            GetStdHandle(STD_OUTPUT_HANDLE),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            0,
        )
    };
}

fn hex32(v: u32) -> [u8; 8] {
    let mut out = [0u8; 8];
    for (i, d) in out.iter_mut().enumerate() {
        *d = b"0123456789abcdef"[((v >> (28 - 4 * i)) & 0xf) as usize];
    }
    out
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn eq(&mut self, got: u32, want: u32) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// A failure names its check, the source line and what was got.
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u32) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        let mut at = *b"(line 0x00000000) ";
        at[8..16].copy_from_slice(&hex32(core::panic::Location::caller().line()));
        print(&at);
        let mut line = *b"wow64_adv_test: check 00000000 failed, got 00000000\r\n";
        line[22..30].copy_from_slice(&hex32(self.0));
        line[43..51].copy_from_slice(&hex32(got));
        print(&line);
        Err(self.0)
    }
}

/// Whether `p` lies in `buf[..len]`.
fn inside(p: *const u8, buf: &[u8], len: u32) -> bool {
    let (p, start) = (p as usize, buf.as_ptr() as usize);
    p >= start && p < start + len as usize
}

fn registry(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout.
    unsafe {
        let path = wide("Software\\gala_wow64_adv_test");
        let (mut key, mut disposition) = (0, 0);
        c.eq(
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                null(),
                0,
                KEY_ALL_ACCESS,
                0,
                &mut key,
                &mut disposition,
            ) as u32,
            0,
        )?;
        c.eq(disposition, REG_CREATED_NEW_KEY)?;
        let text = wide("gala");
        let name = wide("name");
        c.eq(RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, text.as_ptr().cast(), 10) as u32, 0)?;
        let number = wide("number");
        let value = 0x1234_5678u32.to_le_bytes();
        c.eq(RegSetValueExW(key, number.as_ptr(), 0, REG_DWORD, value.as_ptr(), 4) as u32, 0)?;
        let (mut kind, mut size) = (0, 0);
        c.eq(
            RegQueryValueExW(key, name.as_ptr(), null_mut(), &mut kind, null_mut(), &mut size)
                as u32,
            0,
        )?;
        c.eq(kind, REG_SZ)?;
        c.eq(size, 10)?;
        let mut data = [0u16; 8];
        size = 4;
        let r = RegQueryValueExW(
            key,
            name.as_ptr(),
            null_mut(),
            &mut kind,
            data.as_mut_ptr().cast(),
            &mut size,
        );
        c.eq(r as u32, ERROR_MORE_DATA)?;
        c.eq(size, 10)?;
        size = 16;
        let r = RegQueryValueExW(
            key,
            name.as_ptr(),
            null_mut(),
            &mut kind,
            data.as_mut_ptr().cast(),
            &mut size,
        );
        c.eq(r as u32, 0)?;
        c.eq(size, 10)?;
        c.ok(same(&data, "gala"), u32::from(data[0]))?;
        let mut dword = [0u8; 4];
        size = 4;
        let r = RegQueryValueExW(
            key,
            number.as_ptr(),
            null_mut(),
            &mut kind,
            dword.as_mut_ptr(),
            &mut size,
        );
        c.eq(r as u32, 0)?;
        c.eq(kind, REG_DWORD)?;
        c.eq(u32::from_le_bytes(dword), 0x1234_5678)?;

        // A subkey, enumerated, then deleted.
        let child = wide("child");
        let mut sub = 0;
        c.eq(
            RegCreateKeyExW(
                key,
                child.as_ptr(),
                0,
                null(),
                0,
                KEY_ALL_ACCESS,
                0,
                &mut sub,
                &mut disposition,
            ) as u32,
            0,
        )?;
        c.eq(disposition, REG_CREATED_NEW_KEY)?;
        c.eq(RegCloseKey(sub) as u32, 0)?;
        let mut found = [0u16; 32];
        let mut len = 32;
        let r = RegEnumKeyExW(
            key,
            0,
            found.as_mut_ptr(),
            &mut len,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
        );
        c.eq(r as u32, 0)?;
        c.eq(len, 5)?;
        c.ok(same(&found, "child"), u32::from(found[0]))?;
        len = 32;
        let r = RegEnumKeyExW(
            key,
            1,
            found.as_mut_ptr(),
            &mut len,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
        );
        c.eq(r as u32, ERROR_NO_MORE_ITEMS)?;
        c.eq(RegDeleteKeyW(key, child.as_ptr()) as u32, 0)?;
        len = 32;
        let r = RegEnumKeyExW(
            key,
            0,
            found.as_mut_ptr(),
            &mut len,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
        );
        c.eq(r as u32, ERROR_NO_MORE_ITEMS)?;
        c.eq(RegCloseKey(key) as u32, 0)?;
        c.eq(RegDeleteKeyW(HKEY_CURRENT_USER, path.as_ptr()) as u32, 0)?;
        let r = RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ, &mut key);
        c.eq(r as u32, ERROR_FILE_NOT_FOUND)?;

        // HKEY_LOCAL_MACHINE as a 32-bit HKEY.
        let software = wide("SOFTWARE");
        c.eq(
            RegOpenKeyExW(HKEY_LOCAL_MACHINE, software.as_ptr(), 0, KEY_READ, &mut key) as u32,
            0,
        )?;
        c.eq(RegCloseKey(key) as u32, 0)?;
    }
    Ok(())
}

/// The registry's 32-bit view: HKLM\SOFTWARE is WOW6432Node's for a
/// 32-bit program; KEY_WOW64_64KEY and KEY_WOW64_32KEY choose.
fn registry_view(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout.
    unsafe {
        let (mut key, mut other, mut disposition) = (0, 0, 0);
        let path = wide("SOFTWARE\\gala_wow64_view");
        let node = wide("SOFTWARE\\WOW6432Node\\gala_wow64_view");
        // Created by a 32-bit program: in WOW6432Node.
        let r = RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            0,
            null(),
            0,
            KEY_ALL_ACCESS,
            0,
            &mut key,
            &mut disposition,
        );
        c.eq(r as u32, 0)?;
        c.eq(disposition, REG_CREATED_NEW_KEY)?;
        let name = wide("bits");
        let value = 32u32.to_le_bytes();
        c.eq(RegSetValueExW(key, name.as_ptr(), 0, REG_DWORD, value.as_ptr(), 4) as u32, 0)?;
        c.eq(RegCloseKey(key) as u32, 0)?;
        // The 64-bit view: under WOW6432Node, not under SOFTWARE.
        let r = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            node.as_ptr(),
            0,
            KEY_READ | KEY_WOW64_64KEY,
            &mut key,
        );
        c.eq(r as u32, 0)?;
        let (mut kind, mut size, mut data) = (0, 4, [0u8; 4]);
        let r = RegQueryValueExW(
            key,
            name.as_ptr(),
            null_mut(),
            &mut kind,
            data.as_mut_ptr(),
            &mut size,
        );
        c.eq(r as u32, 0)?;
        c.eq(u32::from_le_bytes(data), 32)?;
        c.eq(RegCloseKey(key) as u32, 0)?;
        let r = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            0,
            KEY_READ | KEY_WOW64_64KEY,
            &mut key,
        );
        c.eq(r as u32, ERROR_FILE_NOT_FOUND)?;
        // KEY_WOW64_32KEY is this program's view anyway.
        let r = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            0,
            KEY_READ | KEY_WOW64_32KEY,
            &mut key,
        );
        c.eq(r as u32, 0)?;
        c.eq(RegCloseKey(key) as u32, 0)?;
        // WOW6432Node named by a 32-bit program: the same key.
        let r = RegOpenKeyExW(HKEY_LOCAL_MACHINE, node.as_ptr(), 0, KEY_READ, &mut key);
        c.eq(r as u32, 0)?;
        c.eq(RegCloseKey(key) as u32, 0)?;
        // KEY_WOW64_64KEY: a key in the 64-bit SOFTWARE.
        let software = wide("SOFTWARE");
        let mut sw64 = 0;
        let r = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            software.as_ptr(),
            0,
            KEY_ALL_ACCESS | KEY_WOW64_64KEY,
            &mut sw64,
        );
        c.eq(r as u32, 0)?;
        let leaf = wide("gala_wow64_view64");
        let all64 = KEY_ALL_ACCESS | KEY_WOW64_64KEY;
        let r = RegCreateKeyExW(
            sw64,
            leaf.as_ptr(),
            0,
            null(),
            0,
            all64,
            0,
            &mut other,
            &mut disposition,
        );
        c.eq(r as u32, 0)?;
        c.eq(disposition, REG_CREATED_NEW_KEY)?;
        c.eq(RegCloseKey(other) as u32, 0)?;
        // ... which the 32-bit view does not see.
        let path64 = wide("SOFTWARE\\gala_wow64_view64");
        let r = RegOpenKeyExW(HKEY_LOCAL_MACHINE, path64.as_ptr(), 0, KEY_READ, &mut key);
        c.eq(r as u32, ERROR_FILE_NOT_FOUND)?;
        let r = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            path64.as_ptr(),
            0,
            KEY_READ | KEY_WOW64_64KEY,
            &mut key,
        );
        c.eq(r as u32, 0)?;
        c.eq(RegCloseKey(key) as u32, 0)?;
        // Opened again in the 32-bit view, as existing.
        let r = RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            0,
            null(),
            0,
            KEY_ALL_ACCESS,
            0,
            &mut key,
            &mut disposition,
        );
        c.eq(r as u32, 0)?;
        c.eq(disposition, REG_OPENED_EXISTING_KEY)?;
        c.eq(RegCloseKey(key) as u32, 0)?;
        // HKCU\Software is shared: no WOW6432Node there.
        let user = wide("Software\\gala_wow64_shared");
        let r = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            user.as_ptr(),
            0,
            null(),
            0,
            KEY_ALL_ACCESS,
            0,
            &mut key,
            &mut disposition,
        );
        c.eq(r as u32, 0)?;
        c.eq(RegCloseKey(key) as u32, 0)?;
        let r = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            user.as_ptr(),
            0,
            KEY_READ | KEY_WOW64_64KEY,
            &mut key,
        );
        c.eq(r as u32, 0)?;
        c.eq(RegCloseKey(key) as u32, 0)?;
        c.eq(RegDeleteKeyW(HKEY_CURRENT_USER, user.as_ptr()) as u32, 0)?;
        // Deleted from each view.
        c.eq(RegDeleteKeyExW(sw64, leaf.as_ptr(), KEY_WOW64_64KEY, 0) as u32, 0)?;
        c.eq(RegCloseKey(sw64) as u32, 0)?;
        c.eq(RegDeleteKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), KEY_WOW64_32KEY, 0) as u32, 0)?;
        let r = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            node.as_ptr(),
            0,
            KEY_READ | KEY_WOW64_64KEY,
            &mut key,
        );
        c.eq(r as u32, ERROR_FILE_NOT_FOUND)?;
    }
    Ok(())
}

fn tokens(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout.
    unsafe {
        let mut token = 0;
        c.eq(
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY | TOKEN_DUPLICATE, &mut token) as u32,
            1,
        )?;

        // TokenUser: SID_AND_ATTRIBUTES (Sid at 0, Attributes at 4), the
        // SID after it.
        let mut need = 0;
        c.eq(GetTokenInformation(token, TOKEN_USER, null_mut(), 0, &mut need) as u32, 0)?;
        c.eq(GetLastError(), ERROR_INSUFFICIENT_BUFFER)?;
        c.ok((8 + 12..=256).contains(&need), need)?;
        let mut user = [0u32; 64];
        let buf = core::slice::from_raw_parts_mut(user.as_mut_ptr().cast::<u8>(), 256);
        let r = GetTokenInformation(token, TOKEN_USER, buf.as_mut_ptr(), need, &mut need);
        c.eq(r as u32, 1)?;
        let sid = user[0] as *const u8;
        c.ok(inside(sid, buf, need), sid as u32)?;
        c.eq(user[1], 0)?;
        c.eq(IsValidSid(sid) as u32, 1)?;
        c.ok(inside(sid.add(GetLengthSid(sid) as usize - 1), buf, need), GetLengthSid(sid))?;
        let mut text = null_mut();
        c.eq(ConvertSidToStringSidW(sid, &mut text) as u32, 1)?;
        c.ok(starts(text, "S-1-5-"), u32::from(*text))?;
        LocalFree(text.cast());
        let (mut name, mut domain) = ([0u16; 64], [0u16; 64]);
        let (mut name_len, mut domain_len, mut kind) = (64, 64, 0);
        let r = LookupAccountSidW(
            null(),
            sid,
            name.as_mut_ptr(),
            &mut name_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut kind,
        );
        c.eq(r as u32, 1)?;
        c.eq(kind, SID_TYPE_USER)?;
        let mut me = [0u16; 64];
        let mut me_len = 64;
        c.eq(GetUserNameW(me.as_mut_ptr(), &mut me_len) as u32, 1)?;
        c.eq(me_len, name_len + 1)?;
        c.ok(me[..me_len as usize] == name[..me_len as usize], u32::from(me[0]))?;

        // TokenGroups: GroupCount, then SID_AND_ATTRIBUTES at 4.
        c.eq(GetTokenInformation(token, TOKEN_GROUPS, null_mut(), 0, &mut need) as u32, 0)?;
        c.eq(GetLastError(), ERROR_INSUFFICIENT_BUFFER)?;
        let mut groups = [0u32; 512];
        let buf = core::slice::from_raw_parts_mut(groups.as_mut_ptr().cast::<u8>(), 2048);
        c.ok(need <= 2048, need)?;
        let r = GetTokenInformation(token, TOKEN_GROUPS, buf.as_mut_ptr(), need, &mut need);
        c.eq(r as u32, 1)?;
        let count = groups[0] as usize;
        c.ok(count > 0 && 4 + 8 * count <= need as usize, count as u32)?;
        let mut everyone = false;
        for i in 0..count {
            let sid = groups[1 + 2 * i] as *const u8;
            c.ok(inside(sid, buf, need), sid as u32)?;
            c.eq(IsValidSid(sid) as u32, 1)?;
            if EqualSid(sid, EVERYONE.as_ptr()) != 0 {
                everyone = true;
                c.ok(groups[2 + 2 * i] & SE_GROUP_ENABLED != 0, groups[2 + 2 * i])?;
            }
        }
        c.ok(everyone, count as u32)?;
        let mut member = 0;
        c.eq(CheckTokenMembership(0, EVERYONE.as_ptr(), &mut member) as u32, 1)?;
        c.eq(member as u32, 1)?;

        // DuplicateTokenEx with an inheritable handle (x86
        // SECURITY_ATTRIBUTES: bInheritHandle at 8).
        let sa = SecurityAttributes { length: 12, descriptor: null_mut(), inherit: 1 };
        let mut copy = 0;
        c.eq(DuplicateTokenEx(token, TOKEN_QUERY, &sa, 2, 2, &mut copy) as u32, 1)?;
        let mut flags = 0;
        c.eq(GetHandleInformation(copy, &mut flags) as u32, 1)?;
        c.eq(flags & 1, 1)?; // HANDLE_FLAG_INHERIT
        let mut kind = 0u32;
        let r = GetTokenInformation(copy, TOKEN_TYPE, (&raw mut kind).cast(), 4, &mut need);
        c.eq(r as u32, 1)?;
        c.eq(kind, 2)?; // TokenImpersonation
        c.eq(CloseHandle(copy) as u32, 1)?;
        c.eq(CloseHandle(token) as u32, 1)?;

        // A well-known SID by its string.
        let mut admins = null_mut();
        let text = wide("S-1-5-32-544");
        c.eq(ConvertStringSidToSidW(text.as_ptr(), &mut admins) as u32, 1)?;
        c.eq(GetLengthSid(admins), 16)?;
        let (mut name_len, mut domain_len) = (64, 64);
        let r = LookupAccountSidW(
            null(),
            admins,
            name.as_mut_ptr(),
            &mut name_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut kind,
        );
        c.eq(r as u32, 1)?;
        c.ok(same(&name, "Administrators"), u32::from(name[0]))?;
        c.ok(same(&domain, "BUILTIN"), u32::from(domain[0]))?;
        c.eq(kind, SID_TYPE_ALIAS)?;
        LocalFree(admins);
    }
    Ok(())
}

fn descriptors(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout.
    unsafe {
        // An absolute descriptor: 20 bytes, then a canary.
        let mut sd = [0xa5u8; 20 + 20];
        c.eq(InitializeSecurityDescriptor(sd.as_mut_ptr(), 1) as u32, 1)?;
        c.ok(sd[20..].iter().all(|&b| b == 0xa5), u32::from(sd[20]))?;
        c.ok(sd[1..20].iter().all(|&b| b == 0), u32::from(sd[4]))?;
        c.eq(u32::from(sd[0]), 1)?;
        let mut acl = [0u32; 16];
        let acl = acl.as_mut_ptr().cast::<u8>();
        c.eq(InitializeAcl(acl, 64, 2) as u32, 1)?;
        c.eq(AddAccessAllowedAce(acl, 2, GENERIC_ALL, EVERYONE.as_ptr()) as u32, 1)?;
        c.eq(SetSecurityDescriptorDacl(sd.as_mut_ptr(), 1, acl, 0) as u32, 1)?;
        let field = |at: usize| u32::from_le_bytes([sd[at], sd[at + 1], sd[at + 2], sd[at + 3]]);
        c.eq(field(16), acl as u32)?;
        let mut owner_sid = EVERYONE;
        c.eq(SetSecurityDescriptorOwner(sd.as_mut_ptr(), owner_sid.as_mut_ptr(), 0) as u32, 1)?;
        let field = |at: usize| u32::from_le_bytes([sd[at], sd[at + 1], sd[at + 2], sd[at + 3]]);
        c.eq(field(4), owner_sid.as_ptr() as u32)?;
        let mut group_sid = EVERYONE;
        c.eq(SetSecurityDescriptorGroup(sd.as_mut_ptr(), group_sid.as_mut_ptr(), 0) as u32, 1)?;
        let field = |at: usize| u32::from_le_bytes([sd[at], sd[at + 1], sd[at + 2], sd[at + 3]]);
        c.eq(field(8), group_sid.as_ptr() as u32)?;
        c.ok(sd[20..].iter().all(|&b| b == 0xa5), u32::from(sd[20]))?;
        c.eq(IsValidSecurityDescriptor(sd.as_ptr()) as u32, 1)?;
        let (mut present, mut dacl, mut defaulted) = (0, null_mut(), 0);
        c.eq(
            GetSecurityDescriptorDacl(sd.as_ptr(), &mut present, &mut dacl, &mut defaulted) as u32,
            1,
        )?;
        c.eq(present as u32, 1)?;
        c.eq(dacl as u32, acl as u32)?;
        let mut owner = null_mut();
        c.eq(GetSecurityDescriptorOwner(sd.as_ptr(), &mut owner, &mut defaulted) as u32, 1)?;
        c.eq(owner as u32, owner_sid.as_ptr() as u32)?;
        let mut group = null_mut();
        c.eq(GetSecurityDescriptorGroup(sd.as_ptr(), &mut group, &mut defaulted) as u32, 1)?;
        c.eq(group as u32, group_sid.as_ptr() as u32)?;

        // A self-relative one from SDDL: offsets.
        let text = wide("O:BAG:BAD:(A;;GA;;;WD)");
        let (mut rel, mut len) = (null_mut(), 0);
        c.eq(
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                1,
                &mut rel,
                &mut len,
            ) as u32,
            1,
        )?;
        let control = u16::from_le_bytes([*rel.add(2), *rel.add(3)]);
        c.ok(control & SE_SELF_RELATIVE != 0, control.into())?;
        c.eq(GetSecurityDescriptorLength(rel), len)?;
        c.eq(GetSecurityDescriptorOwner(rel, &mut owner, &mut defaulted) as u32, 1)?;
        let mut admins = null_mut();
        let sid_text = wide("S-1-5-32-544");
        c.eq(ConvertStringSidToSidW(sid_text.as_ptr(), &mut admins) as u32, 1)?;
        c.eq(EqualSid(owner, admins) as u32, 1)?;
        c.eq(GetSecurityDescriptorDacl(rel, &mut present, &mut dacl, &mut defaulted) as u32, 1)?;
        c.eq(present as u32, 1)?;
        c.ok(dacl > rel && dacl < rel.add(len as usize), dacl as u32)?;
        let mut ace = null_mut();
        c.eq(GetAce(dacl, 0, &mut ace) as u32, 1)?;
        c.eq(EqualSid(ace.add(8), EVERYONE.as_ptr()) as u32, 1)?;
        LocalFree(admins);
        LocalFree(rel);

        // SetEntriesInAclW with x86 EXPLICIT_ACCESS_W.
        let mut everyone = EVERYONE;
        let entry = ExplicitAccess {
            permissions: GENERIC_READ,
            mode: GRANT_ACCESS,
            inheritance: 0,
            trustee: Trustee {
                multiple: null_mut(),
                operation: 0,
                form: TRUSTEE_IS_SID,
                kind: 0,
                name: everyone.as_mut_ptr().cast(),
            },
        };
        let mut new = null_mut();
        c.eq(SetEntriesInAclW(1, &entry, null(), &mut new), 0)?;
        c.eq(u32::from(u16::from_le_bytes([*new.add(4), *new.add(5)])), 1)?; // AceCount
        c.eq(GetAce(new, 0, &mut ace) as u32, 1)?;
        c.eq(
            u32::from_le_bytes([*ace.add(4), *ace.add(5), *ace.add(6), *ace.add(7)]),
            GENERIC_READ,
        )?;
        c.eq(EqualSid(ace.add(8), EVERYONE.as_ptr()) as u32, 1)?;
        LocalFree(new);
        // BuildExplicitAccessWithNameW fills 32 bytes.
        let mut built = [ExplicitAccess { permissions: 0xa5a5_a5a5, ..entry }; 2];
        let me = wide("CURRENT_USER");
        BuildExplicitAccessWithNameW(&mut built[0], me.as_ptr(), GENERIC_ALL, GRANT_ACCESS, 0);
        c.eq(built[0].permissions, GENERIC_ALL)?;
        c.eq(built[0].trustee.form, TRUSTEE_IS_NAME)?;
        c.eq(built[0].trustee.name as u32, me.as_ptr() as u32)?;
        c.eq(built[1].permissions, 0xa5a5_a5a5)?;
    }
    Ok(())
}

fn crypto(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid arguments throughout.
    unsafe {
        let mut provider = 0;
        let r =
            CryptAcquireContextW(&mut provider, null(), null(), PROV_RSA_FULL, CRYPT_VERIFYCONTEXT);
        c.eq(r as u32, 1)?;
        let mut bytes = [0u8; 32];
        c.eq(CryptGenRandom(provider, 32, bytes.as_mut_ptr()) as u32, 1)?;
        c.ok(bytes.iter().any(|&b| b != 0), 0)?;
        c.eq(CryptReleaseContext(provider, 0) as u32, 1)?;
        let mut more = [0u8; 16];
        c.eq(u32::from(SystemFunction036(more.as_mut_ptr(), 16)), 1)?;
        c.ok(more.iter().any(|&b| b != 0) && more != bytes[..16], 0)?;
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    registry(&mut c)?;
    tokens(&mut c)?;
    descriptors(&mut c)?;
    crypto(&mut c)?;
    registry_view(&mut c)?;
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            print(b"wow64_adv_test: ok\r\n");
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
