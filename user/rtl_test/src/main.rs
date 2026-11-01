//! ntdll's run-time library and a few calls the MSYS2 runtime (Git
//! Bash's msys-2.0.dll) imports, with the results Windows documents:
//!
//! - strings: RtlInitUnicodeString/RtlInitAnsiString (lengths without the
//!   NUL; a null source is empty), RtlCompareUnicodeString and
//!   RtlEqualUnicodeString (with and without case), RtlPrefixUnicodeString,
//!   RtlCopyUnicodeString (truncating, NUL-terminating if there is room),
//!   RtlAppendUnicodeToString and RtlAppendUnicodeStringToString
//!   (STATUS_BUFFER_TOO_SMALL when it does not fit), RtlUpcaseUnicodeChar,
//!   RtlUpcaseUnicodeString and RtlDowncaseUnicodeString (allocating),
//!   RtlUnicodeStringToAnsiString (allocating), RtlFreeUnicodeString;
//! - SIDs: RtlInitializeSid, RtlSubAuthoritySid, RtlSubAuthorityCountSid,
//!   RtlIdentifierAuthoritySid, RtlLengthSid, RtlCopySid, RtlEqualSid,
//!   RtlEqualPrefixSid, RtlConvertSidToUnicodeString ("S-1-5-32-544");
//! - ACLs and descriptors: RtlCreateAcl, RtlAddAccessAllowedAce(Ex),
//!   RtlAddAccessDeniedAceEx, RtlGetAce, RtlFirstFreeAce,
//!   RtlCreateSecurityDescriptor, RtlSet/Get Dacl/Owner/Group,
//!   RtlGet/SetControlSecurityDescriptor, RtlAbsoluteToSelfRelativeSD
//!   (STATUS_BUFFER_TOO_SMALL with the size, then SE_SELF_RELATIVE);
//! - a critical section initialized by kernel32 entered and left through
//!   ntdll (RtlEnter/LeaveCriticalSection) and kernel32, recursively;
//! - NtQueryEvent (type and state), NtCreateDirectoryObject (a second
//!   create: STATUS_OBJECT_NAME_COLLISION) and NtOpenDirectoryObject,
//!   NtQueryTimerResolution, RtlSetCurrentDirectory_U;
//! - kernel32: VirtualProtectEx on this process, TerminateThread (its
//!   exit code), QueryUnbiasedInterruptTime.
//!
//! Prints `rtl_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Handle = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *mut u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AnsiString {
    length: u16,
    maximum: u16,
    buffer: *mut u8,
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: Handle,
    name: *const UnicodeString,
    attributes: u32,
    descriptor: *const c_void,
    qos: *const c_void,
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn RtlInitUnicodeString(dst: *mut UnicodeString, src: *const u16);
    fn RtlInitAnsiString(dst: *mut AnsiString, src: *const u8);
    fn RtlCompareUnicodeString(a: *const UnicodeString, b: *const UnicodeString, ci: u8) -> i32;
    fn RtlEqualUnicodeString(a: *const UnicodeString, b: *const UnicodeString, ci: u8) -> u8;
    fn RtlPrefixUnicodeString(p: *const UnicodeString, s: *const UnicodeString, ci: u8) -> u8;
    fn RtlCopyUnicodeString(dst: *mut UnicodeString, src: *const UnicodeString);
    fn RtlAppendUnicodeToString(dst: *mut UnicodeString, src: *const u16) -> u32;
    fn RtlAppendUnicodeStringToString(dst: *mut UnicodeString, src: *const UnicodeString) -> u32;
    fn RtlUpcaseUnicodeChar(c: u16) -> u16;
    fn RtlUpcaseUnicodeString(dst: *mut UnicodeString, src: *const UnicodeString, alloc: u8)
    -> u32;
    fn RtlDowncaseUnicodeString(
        dst: *mut UnicodeString,
        src: *const UnicodeString,
        alloc: u8,
    ) -> u32;
    fn RtlUnicodeStringToAnsiString(
        dst: *mut AnsiString,
        src: *const UnicodeString,
        alloc: u8,
    ) -> u32;
    fn RtlFreeUnicodeString(s: *mut UnicodeString);
    fn RtlFreeAnsiString(s: *mut AnsiString);

    fn RtlInitializeSid(sid: *mut u8, authority: *const [u8; 6], count: u8) -> u32;
    fn RtlSubAuthoritySid(sid: *mut u8, index: u32) -> *mut u32;
    fn RtlSubAuthorityCountSid(sid: *mut u8) -> *mut u8;
    fn RtlIdentifierAuthoritySid(sid: *mut u8) -> *mut [u8; 6];
    fn RtlLengthSid(sid: *const u8) -> u32;
    fn RtlCopySid(len: u32, dst: *mut u8, src: *const u8) -> u32;
    fn RtlEqualSid(a: *const u8, b: *const u8) -> u8;
    fn RtlEqualPrefixSid(a: *const u8, b: *const u8) -> u8;
    fn RtlConvertSidToUnicodeString(dst: *mut UnicodeString, sid: *const u8, alloc: u8) -> u32;

    fn RtlCreateAcl(acl: *mut u8, len: u32, revision: u32) -> u32;
    fn RtlAddAccessAllowedAce(acl: *mut u8, revision: u32, mask: u32, sid: *const u8) -> u32;
    fn RtlAddAccessAllowedAceEx(
        acl: *mut u8,
        revision: u32,
        flags: u32,
        mask: u32,
        sid: *const u8,
    ) -> u32;
    fn RtlAddAccessDeniedAceEx(
        acl: *mut u8,
        revision: u32,
        flags: u32,
        mask: u32,
        sid: *const u8,
    ) -> u32;
    fn RtlGetAce(acl: *const u8, index: u32, ace: *mut *mut u8) -> u32;
    fn RtlFirstFreeAce(acl: *const u8, ace: *mut *mut u8) -> u8;
    fn RtlCreateSecurityDescriptor(sd: *mut u8, revision: u32) -> u32;
    fn RtlSetDaclSecurityDescriptor(sd: *mut u8, present: u8, acl: *mut u8, defaulted: u8) -> u32;
    fn RtlGetDaclSecurityDescriptor(
        sd: *const u8,
        present: *mut u8,
        acl: *mut *mut u8,
        defaulted: *mut u8,
    ) -> u32;
    fn RtlSetOwnerSecurityDescriptor(sd: *mut u8, owner: *mut u8, defaulted: u8) -> u32;
    fn RtlGetOwnerSecurityDescriptor(sd: *const u8, owner: *mut *mut u8, defaulted: *mut u8)
    -> u32;
    fn RtlSetGroupSecurityDescriptor(sd: *mut u8, group: *mut u8, defaulted: u8) -> u32;
    fn RtlGetGroupSecurityDescriptor(sd: *const u8, group: *mut *mut u8, defaulted: *mut u8)
    -> u32;
    fn RtlGetControlSecurityDescriptor(sd: *const u8, control: *mut u16, revision: *mut u32)
    -> u32;
    fn RtlSetControlSecurityDescriptor(sd: *mut u8, mask: u16, bits: u16) -> u32;
    fn RtlAbsoluteToSelfRelativeSD(abs: *const u8, rel: *mut u8, len: *mut u32) -> u32;

    fn RtlEnterCriticalSection(cs: *mut [usize; 5]) -> u32;
    fn RtlLeaveCriticalSection(cs: *mut [usize; 5]) -> u32;

    fn NtQueryEvent(h: Handle, class: u32, buf: *mut u32, len: u32, ret: *mut u32) -> u32;
    fn NtCreateDirectoryObject(h: *mut Handle, access: u32, oa: *const ObjectAttributes) -> u32;
    fn NtOpenDirectoryObject(h: *mut Handle, access: u32, oa: *const ObjectAttributes) -> u32;
    fn NtQueryTimerResolution(max: *mut u32, min: *mut u32, current: *mut u32) -> u32;
    fn RtlSetCurrentDirectory_U(path: *const UnicodeString) -> u32;
    fn NtClose(h: Handle) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn InitializeCriticalSection(cs: *mut [usize; 5]);
    fn EnterCriticalSection(cs: *mut [usize; 5]);
    fn LeaveCriticalSection(cs: *mut [usize; 5]);
    fn GetCurrentThreadId() -> u32;
    fn GetCurrentProcess() -> Handle;
    fn CreateEventW(sa: *const c_void, manual: i32, initial: i32, name: *const u16) -> Handle;
    fn SetEvent(h: Handle) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn VirtualAlloc(at: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut u8;
    fn VirtualProtectEx(p: Handle, at: *mut c_void, size: usize, new: u32, old: *mut u32) -> i32;
    fn CreateThread(
        sa: *const c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    fn TerminateThread(h: Handle, code: u32) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeThread(h: Handle, code: *mut u32) -> i32;
    fn QueryUnbiasedInterruptTime(t: *mut u64) -> i32;
    fn GetCurrentDirectoryW(len: u32, buf: *mut u16) -> u32;
    fn SetCurrentDirectoryW(path: *const u16) -> i32;
}

// The compiler's memcpy/memset/memcmp, from ntdll.
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

// What the compiler's output expects from a C runtime.
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const STATUS_SUCCESS: u64 = 0;
const STATUS_BUFFER_TOO_SMALL: u64 = 0xc000_0023;
const STATUS_OBJECT_NAME_COLLISION: u64 = 0xc000_0035;

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
        out(b"rtl_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

/// A NUL-terminated UTF-16 copy of an ASCII string.
fn w<const N: usize>(s: &str) -> [u16; N] {
    let mut out = [0u16; N];
    for (d, b) in out.iter_mut().zip(s.bytes()) {
        *d = u16::from(b);
    }
    out
}

/// The characters of a UNICODE_STRING.
fn chars(s: &UnicodeString) -> &[u16] {
    // SAFETY: Length bytes at Buffer.
    unsafe { core::slice::from_raw_parts(s.buffer, usize::from(s.length) / 2) }
}

fn same(s: &UnicodeString, text: &str) -> bool {
    chars(s).len() == text.len()
        && chars(s).iter().zip(text.bytes()).all(|(&a, b)| a == u16::from(b))
}

fn us(text: &[u16]) -> UnicodeString {
    let mut s = UnicodeString { length: 0, maximum: 0, buffer: core::ptr::null_mut() };
    // SAFETY: a NUL-terminated string that outlives the use.
    unsafe { RtlInitUnicodeString(&mut s, text.as_ptr()) };
    s
}

fn strings(c: &mut Checks) -> Result<(), u32> {
    let (abc, upper, ab) = (w::<4>("abc"), w::<4>("ABC"), w::<3>("ab"));
    let (a, b, p) = (us(&abc), us(&upper), us(&ab));
    c.eq(a.length.into(), 6)?;
    c.eq(a.maximum.into(), 8)?;
    c.ok(a.buffer.cast_const() == abc.as_ptr(), 0)?;
    // SAFETY: valid strings and buffers throughout.
    unsafe {
        let mut empty = UnicodeString { length: 1, maximum: 1, buffer: core::ptr::null_mut() };
        RtlInitUnicodeString(&mut empty, core::ptr::null());
        c.ok(empty.length == 0 && empty.maximum == 0 && empty.buffer.is_null(), 0)?;
        let mut an = AnsiString { length: 0, maximum: 0, buffer: core::ptr::null_mut() };
        RtlInitAnsiString(&mut an, c"hello".as_ptr().cast());
        c.ok(an.length == 5 && an.maximum == 6, an.length.into())?;

        c.ok(RtlCompareUnicodeString(&a, &b, 0) > 0, 0)?; // 'a' > 'A'
        c.eq(RtlCompareUnicodeString(&a, &b, 1) as u64, 0)?;
        c.ok(RtlCompareUnicodeString(&p, &a, 0) < 0, 0)?; // a prefix sorts first
        c.eq(RtlEqualUnicodeString(&a, &b, 0).into(), 0)?;
        c.eq(RtlEqualUnicodeString(&a, &b, 1).into(), 1)?;
        c.eq(RtlPrefixUnicodeString(&p, &a, 0).into(), 1)?;
        c.eq(RtlPrefixUnicodeString(&a, &p, 0).into(), 0)?;
        c.eq(RtlUpcaseUnicodeChar(u16::from(b'q')).into(), u64::from(b'Q'))?;

        // Copy into a 4-character buffer: truncated, then NUL-terminated
        // when there is room.
        let mut buf = [0xffffu16; 4];
        let mut dst = UnicodeString { length: 0, maximum: 8, buffer: buf.as_mut_ptr() };
        let long = w::<6>("abcde");
        RtlCopyUnicodeString(&mut dst, &us(&long));
        c.eq(dst.length.into(), 8)?;
        let mut buf2 = [0xffffu16; 8];
        let mut dst2 = UnicodeString { length: 0, maximum: 16, buffer: buf2.as_mut_ptr() };
        RtlCopyUnicodeString(&mut dst2, &a);
        c.ok(same(&dst2, "abc") && buf2[3] == 0, buf2[3].into())?;
        c.eq(RtlAppendUnicodeToString(&mut dst2, w::<3>("de").as_ptr()).into(), STATUS_SUCCESS)?;
        c.ok(same(&dst2, "abcde"), dst2.length.into())?;
        c.eq(RtlAppendUnicodeStringToString(&mut dst2, &p).into(), STATUS_SUCCESS)?;
        c.ok(same(&dst2, "abcdeab"), dst2.length.into())?;
        c.eq(RtlAppendUnicodeStringToString(&mut dst2, &p).into(), STATUS_BUFFER_TOO_SMALL)?;
        c.ok(same(&dst2, "abcdeab"), dst2.length.into())?;

        let mut up = UnicodeString { length: 0, maximum: 0, buffer: core::ptr::null_mut() };
        c.eq(RtlUpcaseUnicodeString(&mut up, &a, 1).into(), STATUS_SUCCESS)?;
        c.ok(same(&up, "ABC"), up.length.into())?;
        RtlFreeUnicodeString(&mut up);
        c.ok(up.buffer.is_null(), 0)?;
        let mut down = UnicodeString { length: 0, maximum: 0, buffer: core::ptr::null_mut() };
        c.eq(RtlDowncaseUnicodeString(&mut down, &b, 1).into(), STATUS_SUCCESS)?;
        c.ok(same(&down, "abc"), down.length.into())?;
        RtlFreeUnicodeString(&mut down);
        let mut ansi = AnsiString { length: 0, maximum: 0, buffer: core::ptr::null_mut() };
        c.eq(RtlUnicodeStringToAnsiString(&mut ansi, &a, 1).into(), STATUS_SUCCESS)?;
        c.ok(ansi.length == 3 && core::slice::from_raw_parts(ansi.buffer, 4) == b"abc\0", 0)?;
        RtlFreeAnsiString(&mut ansi);
    }
    Ok(())
}

/// S-1-5-32-544 (BUILTIN\Administrators) in `sid`.
fn administrators(c: &mut Checks, sid: &mut [u8; 68]) -> Result<(), u32> {
    let nt = [0u8, 0, 0, 0, 0, 5];
    // SAFETY: a buffer large enough for two sub-authorities.
    unsafe {
        c.eq(RtlInitializeSid(sid.as_mut_ptr(), &nt, 2).into(), STATUS_SUCCESS)?;
        *RtlSubAuthoritySid(sid.as_mut_ptr(), 0) = 32;
        *RtlSubAuthoritySid(sid.as_mut_ptr(), 1) = 544;
    }
    Ok(())
}

fn sids(c: &mut Checks) -> Result<(), u32> {
    let (mut admins, mut copy, mut users) = ([0u8; 68], [0u8; 68], [0u8; 68]);
    administrators(c, &mut admins)?;
    administrators(c, &mut users)?;
    // SAFETY: valid SIDs and buffers.
    unsafe {
        *RtlSubAuthoritySid(users.as_mut_ptr(), 1) = 545;
        c.eq(RtlLengthSid(admins.as_ptr()).into(), 16)?;
        c.eq((*RtlSubAuthorityCountSid(admins.as_mut_ptr())).into(), 2)?;
        c.eq((*RtlIdentifierAuthoritySid(admins.as_mut_ptr()))[5].into(), 5)?;
        c.eq(RtlCopySid(8, copy.as_mut_ptr(), admins.as_ptr()).into(), STATUS_BUFFER_TOO_SMALL)?;
        c.eq(RtlCopySid(68, copy.as_mut_ptr(), admins.as_ptr()).into(), STATUS_SUCCESS)?;
        c.eq(RtlEqualSid(copy.as_ptr(), admins.as_ptr()).into(), 1)?;
        c.eq(RtlEqualSid(users.as_ptr(), admins.as_ptr()).into(), 0)?;
        // The same but for the last sub-authority.
        c.eq(RtlEqualPrefixSid(users.as_ptr(), admins.as_ptr()).into(), 1)?;
        let mut text = UnicodeString { length: 0, maximum: 0, buffer: core::ptr::null_mut() };
        c.eq(RtlConvertSidToUnicodeString(&mut text, admins.as_ptr(), 1).into(), STATUS_SUCCESS)?;
        c.ok(same(&text, "S-1-5-32-544"), text.length.into())?;
        RtlFreeUnicodeString(&mut text);
    }
    Ok(())
}

fn security(c: &mut Checks) -> Result<(), u32> {
    let mut admins = [0u8; 68];
    administrators(c, &mut admins)?;
    let (mut acl, mut sd, mut rel) = ([0u64; 16], [0u64; 8], [0u64; 32]);
    let a = acl.as_mut_ptr().cast::<u8>();
    let s = sd.as_mut_ptr().cast::<u8>();
    // SAFETY: buffers of the sizes given; ACE pointers point into `acl`.
    unsafe {
        c.eq(RtlCreateAcl(a, 128, 2).into(), STATUS_SUCCESS)?;
        c.eq(RtlAddAccessAllowedAce(a, 2, 0x1f01ff, admins.as_ptr()).into(), STATUS_SUCCESS)?;
        c.eq(RtlAddAccessDeniedAceEx(a, 2, 0x3, 0x10000, admins.as_ptr()).into(), STATUS_SUCCESS)?;
        c.eq(RtlAddAccessAllowedAceEx(a, 2, 0x2, 0x1, admins.as_ptr()).into(), STATUS_SUCCESS)?;
        c.eq((*a.add(4).cast::<u16>()).into(), 3)?; // AceCount
        let mut ace = core::ptr::null_mut();
        c.eq(RtlGetAce(a, 1, &mut ace).into(), STATUS_SUCCESS)?;
        // ACE header: type 1 (denied), flags 3, size 24; then the mask.
        c.ok(*ace == 1 && *ace.add(1) == 3 && *ace.add(2) == 24, (*ace).into())?;
        c.eq((*ace.add(4).cast::<u32>()).into(), 0x10000)?;
        let mut free = core::ptr::null_mut();
        c.eq(RtlFirstFreeAce(a, &mut free).into(), 1)?;
        c.eq(free as u64 - a as u64, 8 + 3 * 24)?;

        c.eq(RtlCreateSecurityDescriptor(s, 1).into(), STATUS_SUCCESS)?;
        c.eq(RtlSetDaclSecurityDescriptor(s, 1, a, 0).into(), STATUS_SUCCESS)?;
        c.eq(RtlSetOwnerSecurityDescriptor(s, admins.as_mut_ptr(), 0).into(), STATUS_SUCCESS)?;
        c.eq(RtlSetGroupSecurityDescriptor(s, admins.as_mut_ptr(), 1).into(), STATUS_SUCCESS)?;
        let (mut present, mut defaulted, mut got) = (0u8, 9u8, core::ptr::null_mut());
        c.eq(RtlGetDaclSecurityDescriptor(s, &mut present, &mut got, &mut defaulted).into(), 0)?;
        c.ok(present == 1 && defaulted == 0 && got == a, present.into())?;
        c.eq(RtlGetOwnerSecurityDescriptor(s, &mut got, &mut defaulted).into(), 0)?;
        c.ok(got == admins.as_mut_ptr() && defaulted == 0, 0)?;
        c.eq(RtlGetGroupSecurityDescriptor(s, &mut got, &mut defaulted).into(), 0)?;
        c.ok(got == admins.as_mut_ptr() && defaulted == 1, defaulted.into())?;
        c.eq(RtlSetControlSecurityDescriptor(s, 0x1000, 0x1000).into(), STATUS_SUCCESS)?;
        let (mut control, mut revision) = (0u16, 0u32);
        c.eq(RtlGetControlSecurityDescriptor(s, &mut control, &mut revision).into(), 0)?;
        // SE_DACL_PRESENT, SE_GROUP_DEFAULTED, SE_DACL_PROTECTED.
        c.ok(control == 0x1006 && revision == 1, control.into())?;
        let mut len = 0u32;
        let r = rel.as_mut_ptr().cast::<u8>();
        c.eq(RtlAbsoluteToSelfRelativeSD(s, r, &mut len).into(), STATUS_BUFFER_TOO_SMALL)?;
        // Header 20, two 16-byte SIDs, the ACL: all of its AclSize (128),
        // not just the bytes its ACEs use.
        c.eq(len.into(), 20 + 16 + 16 + 128)?;
        c.eq(RtlAbsoluteToSelfRelativeSD(s, r, &mut len).into(), STATUS_SUCCESS)?;
        c.eq((*r.add(2).cast::<u16>()).into(), 0x9006)?; // + SE_SELF_RELATIVE
        let mut acl2 = core::ptr::null_mut();
        c.eq(RtlGetDaclSecurityDescriptor(r, &mut present, &mut acl2, &mut defaulted).into(), 0)?;
        c.eq((*acl2.add(4).cast::<u16>()).into(), 3)?;
    }
    Ok(())
}

static mut SECTION: [usize; 5] = [0; 5];

fn critical_sections(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: our own section, used by this thread only.
    unsafe {
        let cs = core::ptr::addr_of_mut!(SECTION);
        InitializeCriticalSection(cs);
        RtlEnterCriticalSection(cs);
        EnterCriticalSection(cs);
        // RTL_CRITICAL_SECTION: LockCount, RecursionCount, OwningThread.
        let words = cs.cast::<u32>();
        c.eq((*words.add(3)).into(), 2)?; // RecursionCount
        c.eq((*cs)[2] as u64, GetCurrentThreadId().into())?; // OwningThread
        LeaveCriticalSection(cs);
        c.eq(RtlLeaveCriticalSection(cs).into(), STATUS_SUCCESS)?;
        c.eq((*cs)[2] as u64, 0)?;
    }
    Ok(())
}

extern "system" fn forever(_: *mut c_void) -> u32 {
    loop {
        core::hint::spin_loop();
    }
}

fn calls(c: &mut Checks) -> Result<(), u32> {
    // SAFETY: valid handles, names and buffers throughout.
    unsafe {
        let event = CreateEventW(core::ptr::null(), 1, 0, core::ptr::null());
        c.ok(SetEvent(event) != 0, 0)?;
        let mut info = [9u32; 2];
        let mut ret = 0;
        c.eq(NtQueryEvent(event, 0, info.as_mut_ptr(), 8, &mut ret).into(), STATUS_SUCCESS)?;
        // EVENT_BASIC_INFORMATION: NotificationEvent (0), signaled.
        c.ok(info == [0, 1] && ret == 8, info[0].into())?;
        CloseHandle(event);

        let name = w::<40>("\\BaseNamedObjects\\gala_rtl_test");
        let us_name = us(&name);
        let oa = ObjectAttributes {
            length: 48,
            root: core::ptr::null_mut(),
            name: &us_name,
            attributes: 0x40, // OBJ_CASE_INSENSITIVE
            descriptor: core::ptr::null(),
            qos: core::ptr::null(),
        };
        let (mut dir, mut again, mut opened) =
            (core::ptr::null_mut(), core::ptr::null_mut(), core::ptr::null_mut());
        c.eq(NtCreateDirectoryObject(&mut dir, 0xf000f, &oa).into(), STATUS_SUCCESS)?;
        c.eq(
            NtCreateDirectoryObject(&mut again, 0xf000f, &oa).into(),
            STATUS_OBJECT_NAME_COLLISION,
        )?;
        c.eq(NtOpenDirectoryObject(&mut opened, 0x3, &oa).into(), STATUS_SUCCESS)?;
        NtClose(opened);
        NtClose(dir);

        let (mut max, mut min, mut current) = (0u32, 0u32, 0u32);
        c.eq(NtQueryTimerResolution(&mut max, &mut min, &mut current).into(), STATUS_SUCCESS)?;
        c.ok(min > 0 && min <= current && current <= max, current.into())?;

        // RtlSetCurrentDirectory_U, seen by GetCurrentDirectoryW.
        let mut before = [0u16; 260];
        let n = GetCurrentDirectoryW(260, before.as_mut_ptr());
        let target = w::<16>("C:\\Windows");
        c.eq(RtlSetCurrentDirectory_U(&us(&target)).into(), STATUS_SUCCESS)?;
        let mut now = [0u16; 260];
        let m = GetCurrentDirectoryW(260, now.as_mut_ptr()) as usize;
        c.ok(now[..m] == target[..10], m as u64)?;
        before[n as usize] = 0;
        c.ok(SetCurrentDirectoryW(before.as_ptr()) != 0, 0)?;

        // kernel32.
        let page = VirtualAlloc(core::ptr::null_mut(), 4096, 0x3000, 0x04);
        let mut old = 0;
        c.ok(VirtualProtectEx(GetCurrentProcess(), page.cast(), 4096, 0x02, &mut old) != 0, 0)?;
        c.eq(old.into(), 0x04)?;
        let t = CreateThread(
            core::ptr::null(),
            0,
            forever,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
        );
        c.ok(!t.is_null(), 0)?;
        c.ok(TerminateThread(t, 7) != 0, 0)?;
        c.eq(WaitForSingleObject(t, 10_000).into(), 0)?;
        let mut code = 0;
        c.ok(GetExitCodeThread(t, &mut code) != 0 && code == 7, code.into())?;
        CloseHandle(t);
        let mut time = 0u64;
        c.ok(QueryUnbiasedInterruptTime(&mut time) != 0 && time > 0, 0)?;
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    strings(&mut c)?;
    sids(&mut c)?;
    security(&mut c)?;
    critical_sections(&mut c)?;
    calls(&mut c)
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"rtl_test: ok\r\n");
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
