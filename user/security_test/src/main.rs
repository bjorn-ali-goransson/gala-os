//! Security descriptors kept on objects (docs/SECURITY.md, phase S2),
//! with the results real Windows gives a standard user, from the
//! documented contracts ("Security Descriptors", "ACE Inheritance Rules",
//! GetFileSecurity, SetFileSecurity, GetSecurityInfo, SetSecurityInfo,
//! RegGetKeySecurity, RegSetKeySecurity, Get/SetKernelObjectSecurity):
//!
//! - A directory created with a protected DACL (Everyone FILE_ALL_ACCESS
//!   inherited by files and directories, SYSTEM FILE_GENERIC_READ not
//!   inheritable) keeps it; its owner is the creator (TokenUser).
//! - A file created in it inherits the Everyone ACE (INHERITED_ACE, no
//!   inheritance flags); a subdirectory inherits it with OI|CI kept; the
//!   SYSTEM ACE is not inherited; neither child's DACL is protected.
//! - A file created with its own protected DACL keeps exactly that.
//! - SetFileSecurityW replaces the DACL (read back); a size query reports
//!   ERROR_INSUFFICIENT_BUFFER and the size.
//! - SetSecurityInfo with PROTECTED_DACL_SECURITY_INFORMATION protects
//!   the DACL it sets; GetSecurityInfo reads owner and DACL of a handle.
//! - Registry keys: a key created with a DACL keeps it, a subkey inherits
//!   its container-inherit ACE (CI kept, INHERITED_ACE), RegSetKeySecurity
//!   replaces it (reading and DELETE, which deleting the key needs);
//!   RegGetKeySecurity size queries.
//! - Kernel objects: a named event created with a DACL keeps it
//!   (GetKernelObjectSecurity); an unnamed one without gets the token's
//!   default DACL (its first trustee is the user); a handle without
//!   READ_CONTROL cannot read it and one without WRITE_DAC cannot change
//!   it (ERROR_ACCESS_DENIED); SetKernelObjectSecurity changes it; the
//!   owner cannot be set to a group the user may not own
//!   (ERROR_INVALID_OWNER).
//!
//! Prints `security_test: ok` or exits with the number of the failing
//! check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: Bool,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetLastError() -> u32;
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetCurrentProcess() -> Handle;
    fn LocalFree(p: *mut c_void) -> *mut c_void;
    fn CreateDirectoryW(path: *const u16, sa: *const SecurityAttributes) -> Bool;
    fn RemoveDirectoryW(path: *const u16) -> Bool;
    fn DeleteFileW(path: *const u16) -> Bool;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *const SecurityAttributes,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CreateEventW(
        sa: *const SecurityAttributes,
        manual: Bool,
        initial: Bool,
        name: *const u16,
    ) -> Handle;
    fn OpenEventW(access: u32, inherit: Bool, name: *const u16) -> Handle;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
        text: *const u16,
        revision: u32,
        sd: *mut *mut c_void,
        size: *mut u32,
    ) -> Bool;
    fn GetFileSecurityW(
        path: *const u16,
        info: u32,
        sd: *mut u8,
        length: u32,
        needed: *mut u32,
    ) -> Bool;
    fn SetFileSecurityW(path: *const u16, info: u32, sd: *const c_void) -> Bool;
    fn GetSecurityDescriptorDacl(
        sd: *const u8,
        present: *mut Bool,
        dacl: *mut *const u8,
        defaulted: *mut Bool,
    ) -> Bool;
    fn GetSecurityDescriptorOwner(
        sd: *const u8,
        owner: *mut *const u8,
        defaulted: *mut Bool,
    ) -> Bool;
    fn GetSecurityDescriptorControl(sd: *const u8, control: *mut u16, revision: *mut u32) -> Bool;
    fn GetSecurityInfo(
        handle: Handle,
        kind: u32,
        info: u32,
        owner: *mut *const u8,
        group: *mut *const u8,
        dacl: *mut *const u8,
        sacl: *mut *const u8,
        sd: *mut *mut c_void,
    ) -> u32;
    fn SetSecurityInfo(
        handle: Handle,
        kind: u32,
        info: u32,
        owner: *const u8,
        group: *const u8,
        dacl: *const u8,
        sacl: *const u8,
    ) -> u32;
    fn GetKernelObjectSecurity(
        h: Handle,
        info: u32,
        sd: *mut u8,
        length: u32,
        needed: *mut u32,
    ) -> Bool;
    fn SetKernelObjectSecurity(h: Handle, info: u32, sd: *const c_void) -> Bool;
    fn RegCreateKeyExW(
        key: usize,
        sub: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        access: u32,
        sa: *const SecurityAttributes,
        out: *mut usize,
        disposition: *mut u32,
    ) -> i32;
    fn RegGetKeySecurity(key: usize, info: u32, sd: *mut u8, size: *mut u32) -> i32;
    fn RegSetKeySecurity(key: usize, info: u32, sd: *const c_void) -> i32;
    fn RegCloseKey(key: usize) -> i32;
    fn RegDeleteKeyW(key: usize, sub: *const u16) -> i32;
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> Bool;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        out: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> Bool;
    fn EqualSid(a: *const u8, b: *const u8) -> Bool;
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

const OWNER: u32 = 1;
const DACL: u32 = 4;
const PROTECTED_DACL: u32 = 0x8000_0000;
const SE_DACL_PRESENT: u16 = 0x0004;
const SE_DACL_PROTECTED: u16 = 0x1000;
const SE_FILE_OBJECT: u32 = 1;
const SE_KERNEL_OBJECT: u32 = 6;

const READ_CONTROL: u32 = 0x0002_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const EVENT_MODIFY_STATE: u32 = 0x0002;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const KEY_ALL_ACCESS: u32 = 0x000f_003f;
const HKEY_CURRENT_USER: usize = 0x8000_0001;

const FILE_ALL_ACCESS: u32 = 0x001f_01ff;
const FILE_GENERIC_READ: u32 = 0x0012_0089;
const KEY_READ: u32 = 0x0002_0019;

const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_INVALID_OWNER: u32 = 1307;

/// S-1-1-0 (Everyone), S-1-5-18 (SYSTEM).
const EVERYONE: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];
const SYSTEM: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0];

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
        out(b"security_test: check ");
        out(&hex(self.0.get()));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0.get())
    }

    fn eq(&self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

fn last_error() -> u64 {
    // SAFETY: a plain call.
    u64::from(unsafe { GetLastError() })
}

/// A NUL-terminated UTF-16 string.
struct W([u16; 96]);

fn w(s: &str) -> W {
    let mut b = [0u16; 96];
    for (slot, c) in b.iter_mut().zip(s.encode_utf16()) {
        *slot = c;
    }
    W(b)
}

/// A descriptor from SDDL (LocalAlloc'd, never freed: the test is short).
fn sddl(c: &Checks, text: &str) -> Result<*mut c_void, u32> {
    let mut sd = core::ptr::null_mut();
    // SAFETY: our string and output.
    let r = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text_ptr(&w(text)),
            1,
            &mut sd,
            core::ptr::null_mut(),
        )
    };
    c.ok(r != 0, last_error())?;
    Ok(sd)
}

fn text_ptr(s: &W) -> *const u16 {
    s.0.as_ptr()
}

#[repr(C, align(8))]
struct Buf([u8; 1024]);

/// The ACEs of an ACL: (type, flags, mask, SID pointer).
fn aces(acl: *const u8) -> ([(u8, u8, u32, *const u8); 8], usize) {
    let mut list = [(0u8, 0u8, 0u32, core::ptr::null::<u8>()); 8];
    if acl.is_null() {
        return (list, 0);
    }
    // SAFETY: an ACL the system returned.
    unsafe {
        let count = usize::from(acl.add(4).cast::<u16>().read_unaligned()).min(8);
        let mut at = acl.add(8);
        for slot in list.iter_mut().take(count) {
            let size = usize::from(at.add(2).cast::<u16>().read_unaligned());
            *slot = (*at, *at.add(1), at.add(4).cast::<u32>().read_unaligned(), at.add(8));
            at = at.add(size);
        }
        (list, count)
    }
}

/// The DACL and control of a self-relative descriptor.
fn dacl_of(c: &Checks, sd: *const u8) -> Result<(*const u8, u16), u32> {
    let (mut present, mut dacl, mut defaulted) = (0, core::ptr::null(), 0);
    let (mut control, mut revision) = (0u16, 0u32);
    // SAFETY: a descriptor the system returned; our outputs.
    unsafe {
        c.ok(GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) != 0, 0)?;
        c.ok(GetSecurityDescriptorControl(sd, &mut control, &mut revision) != 0, 0)?;
    }
    c.ok(present != 0, 0)?;
    Ok((dacl, control))
}

/// Checks an ACL is exactly `want` (flags, mask, SID).
fn expect_aces(c: &Checks, acl: *const u8, want: &[(u8, u32, &[u8])]) -> Result<(), u32> {
    let (list, n) = aces(acl);
    c.eq(n as u64, want.len() as u64)?;
    for (got, &(flags, mask, sid)) in list.iter().zip(want) {
        c.eq(got.0.into(), 0)?; // ACCESS_ALLOWED_ACE_TYPE
        c.eq(got.1.into(), flags.into())?;
        c.eq(got.2.into(), mask.into())?;
        // SAFETY: SIDs inside the returned ACL and our constant.
        c.ok(unsafe { EqualSid(got.3, sid.as_ptr()) } != 0, 0)?;
    }
    Ok(())
}

fn file_dacl(c: &Checks, path: &W) -> Result<(Buf, *const u8, u16), u32> {
    let mut b = Buf([0; 1024]);
    let mut needed = 0;
    // SAFETY: our buffer.
    let r = unsafe { GetFileSecurityW(text_ptr(path), DACL, b.0.as_mut_ptr(), 1024, &mut needed) };
    c.ok(r != 0, last_error())?;
    let (dacl, control) = dacl_of(c, b.0.as_ptr())?;
    Ok((b, dacl, control))
}

fn run() -> Result<(), u32> {
    let c = Checks(core::cell::Cell::new(0));
    let null = core::ptr::null_mut();
    // SAFETY: plain calls with our buffers and handles throughout.
    unsafe {
        // The user's SID.
        let mut token = null;
        c.ok(OpenProcessToken(GetCurrentProcess(), 8, &mut token) != 0, 0)?;
        let mut user = Buf([0; 1024]);
        let mut n = 0;
        c.ok(GetTokenInformation(token, 1, user.0.as_mut_ptr(), 1024, &mut n) != 0, 0)?;
        let user_sid = user.0.as_ptr().cast::<*const u8>().read();

        // A directory with a protected DACL.
        let dir = w("C:\\security_test");
        let sa = SecurityAttributes {
            length: 24,
            descriptor: sddl(&c, "D:P(A;OICI;FA;;;WD)(A;;FR;;;SY)")?,
            inherit: 0,
        };
        c.ok(CreateDirectoryW(text_ptr(&dir), &sa) != 0, last_error())?;
        let (_b, dacl, control) = file_dacl(&c, &dir)?;
        c.ok(control & SE_DACL_PROTECTED != 0, control.into())?;
        expect_aces(&c, dacl, &[(3, FILE_ALL_ACCESS, &EVERYONE), (0, FILE_GENERIC_READ, &SYSTEM)])?;
        // Its owner is the creator.
        let mut owner_sd = Buf([0; 1024]);
        let r = GetFileSecurityW(text_ptr(&dir), OWNER, owner_sd.0.as_mut_ptr(), 1024, &mut n);
        c.ok(r != 0, last_error())?;
        let (mut owner, mut defaulted) = (core::ptr::null(), 0);
        c.ok(GetSecurityDescriptorOwner(owner_sd.0.as_ptr(), &mut owner, &mut defaulted) != 0, 0)?;
        c.ok(!owner.is_null() && EqualSid(owner, user_sid) != 0, 0)?;
        // A size query.
        let mut needed = 0;
        let r = GetFileSecurityW(text_ptr(&dir), DACL, null.cast(), 0, &mut needed);
        c.ok(r == 0, 0)?;
        c.eq(last_error(), ERROR_INSUFFICIENT_BUFFER.into())?;
        c.ok(needed > 20, needed.into())?;

        // A file inherits the Everyone ACE; a subdirectory with OI|CI.
        let file = w("C:\\security_test\\a.txt");
        let h = CreateFileW(text_ptr(&file), GENERIC_WRITE, 0, core::ptr::null(), 1, 0x80, null);
        c.ok(h as isize != -1, last_error())?;
        CloseHandle(h);
        let (_b, dacl, control) = file_dacl(&c, &file)?;
        c.eq((control & SE_DACL_PROTECTED).into(), 0)?;
        expect_aces(&c, dacl, &[(0x10, FILE_ALL_ACCESS, &EVERYONE)])?;
        let sub = w("C:\\security_test\\sub");
        c.ok(CreateDirectoryW(text_ptr(&sub), core::ptr::null()) != 0, last_error())?;
        let (_b, dacl, _) = file_dacl(&c, &sub)?;
        expect_aces(&c, dacl, &[(0x13, FILE_ALL_ACCESS, &EVERYONE)])?;

        // A file with its own protected DACL keeps exactly that.
        let own = w("C:\\security_test\\b.txt");
        let sa =
            SecurityAttributes { length: 24, descriptor: sddl(&c, "D:P(A;;FR;;;SY)")?, inherit: 0 };
        let h = CreateFileW(text_ptr(&own), GENERIC_WRITE, 0, &sa, 1, 0x80, null);
        c.ok(h as isize != -1, last_error())?;
        CloseHandle(h);
        let (_b, dacl, control) = file_dacl(&c, &own)?;
        c.ok(control & SE_DACL_PROTECTED != 0, control.into())?;
        expect_aces(&c, dacl, &[(0, FILE_GENERIC_READ, &SYSTEM)])?;

        // SetFileSecurityW replaces the DACL.
        let r = SetFileSecurityW(text_ptr(&file), DACL, sddl(&c, "D:P(A;;FR;;;WD)")?);
        c.ok(r != 0, last_error())?;
        let (_b, dacl, control) = file_dacl(&c, &file)?;
        c.ok(control & SE_DACL_PROTECTED != 0, control.into())?;
        expect_aces(&c, dacl, &[(0, FILE_GENERIC_READ, &EVERYONE)])?;

        // SetSecurityInfo with PROTECTED_DACL_SECURITY_INFORMATION on a
        // handle; GetSecurityInfo reads it back with the owner.
        let h = CreateFileW(
            text_ptr(&own),
            READ_CONTROL | WRITE_DAC,
            7,
            core::ptr::null(),
            3,
            // Overlapped: no SYNCHRONIZE, which only SYSTEM's ACE could
            // grant (the owner has READ_CONTROL and WRITE_DAC).
            0x4000_0080,
            null,
        );
        c.ok(h as isize != -1, last_error())?;
        let unprotected = sddl(&c, "D:(A;;FA;;;SY)")?;
        let (mut present, mut new_dacl, mut d) = (0, core::ptr::null(), 0);
        c.ok(
            GetSecurityDescriptorDacl(unprotected.cast(), &mut present, &mut new_dacl, &mut d) != 0,
            0,
        )?;
        let r = SetSecurityInfo(
            h,
            SE_FILE_OBJECT,
            DACL | PROTECTED_DACL,
            null.cast(),
            null.cast(),
            new_dacl,
            null.cast(),
        );
        c.eq(r.into(), 0)?;
        let (mut o, mut acl, mut sd) = (core::ptr::null(), core::ptr::null(), null);
        let r = GetSecurityInfo(
            h,
            SE_FILE_OBJECT,
            OWNER | DACL,
            &mut o,
            null.cast(),
            &mut acl,
            null.cast(),
            &mut sd,
        );
        c.eq(r.into(), 0)?;
        c.ok(!o.is_null() && EqualSid(o, user_sid) != 0, 0)?;
        expect_aces(&c, acl, &[(0, FILE_ALL_ACCESS, &SYSTEM)])?;
        let (_, control) = dacl_of(&c, sd.cast())?;
        c.ok(control & SE_DACL_PROTECTED != 0, control.into())?;
        LocalFree(sd);
        CloseHandle(h);

        // Registry keys.
        let key_name = w("Software\\gala-security-test");
        let sa = SecurityAttributes {
            length: 24,
            descriptor: sddl(&c, "D:P(A;CI;KA;;;WD)")?,
            inherit: 0,
        };
        let (mut key, mut disposition) = (0usize, 0u32);
        let r = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            text_ptr(&key_name),
            0,
            core::ptr::null(),
            0,
            KEY_ALL_ACCESS,
            &sa,
            &mut key,
            &mut disposition,
        );
        c.eq(r as u64, 0)?;
        let mut kb = Buf([0; 1024]);
        let mut size = 8u32;
        c.eq(
            RegGetKeySecurity(key, DACL, kb.0.as_mut_ptr(), &mut size) as u64,
            ERROR_INSUFFICIENT_BUFFER.into(),
        )?;
        c.ok(size > 20, size.into())?;
        size = 1024;
        c.eq(RegGetKeySecurity(key, DACL, kb.0.as_mut_ptr(), &mut size) as u64, 0)?;
        let (dacl, _) = dacl_of(&c, kb.0.as_ptr())?;
        expect_aces(&c, dacl, &[(2, KEY_ALL_ACCESS, &EVERYONE)])?;
        let sub_name = w("child");
        let mut subkey = 0usize;
        let r = RegCreateKeyExW(
            key,
            text_ptr(&sub_name),
            0,
            core::ptr::null(),
            0,
            KEY_ALL_ACCESS,
            core::ptr::null(),
            &mut subkey,
            &mut disposition,
        );
        c.eq(r as u64, 0)?;
        size = 1024;
        c.eq(RegGetKeySecurity(subkey, DACL, kb.0.as_mut_ptr(), &mut size) as u64, 0)?;
        let (dacl, _) = dacl_of(&c, kb.0.as_ptr())?;
        expect_aces(&c, dacl, &[(0x12, KEY_ALL_ACCESS, &EVERYONE)])?;
        c.eq(RegSetKeySecurity(subkey, DACL, sddl(&c, "D:P(A;;KRSD;;;WD)")?) as u64, 0)?;
        size = 1024;
        c.eq(RegGetKeySecurity(subkey, DACL, kb.0.as_mut_ptr(), &mut size) as u64, 0)?;
        let (dacl, _) = dacl_of(&c, kb.0.as_ptr())?;
        expect_aces(&c, dacl, &[(0, KEY_READ | 0x0001_0000, &EVERYONE)])?;
        RegCloseKey(subkey);
        c.eq(RegDeleteKeyW(key, text_ptr(&sub_name)) as u64, 0)?;
        RegCloseKey(key);
        c.eq(RegDeleteKeyW(HKEY_CURRENT_USER, text_ptr(&key_name)) as u64, 0)?;

        // Kernel objects: a named event with a DACL.
        let name = w("gala-security-test");
        let sa = SecurityAttributes {
            length: 24,
            descriptor: sddl(&c, "D:P(A;;0x1f0003;;;WD)")?,
            inherit: 0,
        };
        let event = CreateEventW(&sa, 1, 0, text_ptr(&name));
        c.ok(!event.is_null(), last_error())?;
        let mut eb = Buf([0; 1024]);
        c.ok(
            GetKernelObjectSecurity(event, DACL, eb.0.as_mut_ptr(), 1024, &mut n) != 0,
            last_error(),
        )?;
        let (dacl, _) = dacl_of(&c, eb.0.as_ptr())?;
        expect_aces(&c, dacl, &[(0, 0x001f_0003, &EVERYONE)])?;
        // Without READ_CONTROL / WRITE_DAC on the handle.
        let weak = OpenEventW(EVENT_MODIFY_STATE, 0, text_ptr(&name));
        c.ok(!weak.is_null(), last_error())?;
        c.ok(GetKernelObjectSecurity(weak, DACL, eb.0.as_mut_ptr(), 1024, &mut n) == 0, 0)?;
        c.eq(last_error(), ERROR_ACCESS_DENIED.into())?;
        let replacement = sddl(&c, "D:P(A;;0x1f0003;;;SY)")?;
        c.ok(SetKernelObjectSecurity(weak, DACL, replacement) == 0, 0)?;
        c.eq(last_error(), ERROR_ACCESS_DENIED.into())?;
        CloseHandle(weak);
        // SetKernelObjectSecurity changes it.
        c.ok(SetKernelObjectSecurity(event, DACL, replacement) != 0, last_error())?;
        c.ok(GetKernelObjectSecurity(event, DACL, eb.0.as_mut_ptr(), 1024, &mut n) != 0, 0)?;
        let (dacl, _) = dacl_of(&c, eb.0.as_ptr())?;
        expect_aces(&c, dacl, &[(0, 0x001f_0003, &SYSTEM)])?;
        // An owner the user may not set.
        c.ok(SetKernelObjectSecurity(event, OWNER, sddl(&c, "O:WD")?) == 0, 0)?;
        c.eq(last_error(), ERROR_INVALID_OWNER.into())?;
        // GetSecurityInfo of a kernel object: the owner is the user.
        let (mut o, mut sd) = (core::ptr::null(), null);
        let r = GetSecurityInfo(
            event,
            SE_KERNEL_OBJECT,
            OWNER,
            &mut o,
            null.cast(),
            null.cast(),
            null.cast(),
            &mut sd,
        );
        c.eq(r.into(), 0)?;
        c.ok(!o.is_null() && EqualSid(o, user_sid) != 0, 0)?;
        LocalFree(sd);
        CloseHandle(event);
        // An unnamed event without a descriptor: the token's default DACL.
        let plain = CreateEventW(core::ptr::null(), 1, 0, core::ptr::null());
        c.ok(
            GetKernelObjectSecurity(plain, DACL, eb.0.as_mut_ptr(), 1024, &mut n) != 0,
            last_error(),
        )?;
        let (dacl, _) = dacl_of(&c, eb.0.as_ptr())?;
        let (list, count) = aces(dacl);
        c.ok(count >= 2, count as u64)?;
        c.ok(EqualSid(list[0].3, user_sid) != 0, 0)?;
        CloseHandle(plain);

        // Clean up.
        c.ok(DeleteFileW(text_ptr(&file)) != 0, last_error())?;
        c.ok(DeleteFileW(text_ptr(&own)) != 0, last_error())?;
        c.ok(RemoveDirectoryW(text_ptr(&sub)) != 0, last_error())?;
        c.ok(RemoveDirectoryW(text_ptr(&dir)) != 0, last_error())?;
        let _ = (GENERIC_READ, SE_DACL_PRESENT);
        CloseHandle(token);
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"security_test: ok\r\n");
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
