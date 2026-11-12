//! What the Click-to-Run service's tasks after registry staging call:
//!
//! - wtsapi32's WTSEnumerateSessionsW and A (its UNINSTALLCENTENNIAL
//!   task delay-loads them; Wine's dlls/wtsapi32/tests/wtsapi.c
//!   test_WTSEnumerateSessions, run on Windows): at least two sessions,
//!   the same in both forms; the one named "Console" is active
//!   (WTSActive) and is the active console session
//!   (WTSGetActiveConsoleSessionId), "Services" is session 0,
//!   disconnected; a Version other than 1 is ERROR_INVALID_PARAMETER (the
//!   documentation); WTSFreeMemory frees the lists.
//! - ntdll's RtlValidRelativeSecurityDescriptor (the WDK's
//!   documentation): a self-relative descriptor MakeSelfRelativeSD made
//!   with an owner and a DACL is valid for those parts, not when a group
//!   is required; too short a length, a header that is not
//!   self-relative and an owner offset past the length are not.

use core::ptr::{null, null_mut};

use super::Checks;

#[cfg_attr(
    target_arch = "x86",
    link(name = "wtsapi32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "wtsapi32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn WTSEnumerateSessionsW(
        server: usize,
        reserved: u32,
        version: u32,
        info: *mut *mut SessionInfo,
        count: *mut u32,
    ) -> i32;
    fn WTSEnumerateSessionsA(
        server: usize,
        reserved: u32,
        version: u32,
        info: *mut *mut SessionInfo,
        count: *mut u32,
    ) -> i32;
    fn WTSFreeMemory(p: *mut SessionInfo);
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn WTSGetActiveConsoleSessionId() -> u32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn RtlValidRelativeSecurityDescriptor(sd: *const u8, length: u32, required: u32) -> u8;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "advapi32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "advapi32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn InitializeSecurityDescriptor(sd: *mut u8, revision: u32) -> i32;
    fn SetSecurityDescriptorOwner(sd: *mut u8, owner: *mut u8, defaulted: i32) -> i32;
    fn SetSecurityDescriptorDacl(sd: *mut u8, present: i32, acl: *mut u8, defaulted: i32) -> i32;
    fn InitializeAcl(acl: *mut u8, size: u32, revision: u32) -> i32;
    fn MakeSelfRelativeSD(abs: *const u8, rel: *mut u8, len: *mut u32) -> i32;
}

/// WTS_SESSION_INFOW/A.
#[repr(C)]
struct SessionInfo {
    id: u32,
    name: usize,
    state: u32,
}

const WTS_ACTIVE: u32 = 0;
const WTS_DISCONNECTED: u32 = 4;

/// Whether the NUL-terminated name at `p` (units of `unit` bytes) is `s`.
fn name_is(p: usize, unit: usize, s: &[u8]) -> bool {
    // SAFETY: a NUL-terminated name in the list's block.
    unsafe {
        for (i, &b) in s.iter().chain(&[0]).enumerate() {
            let c = if unit == 2 {
                *(p as *const u16).add(i)
            } else {
                u16::from(*(p as *const u8).add(i))
            };
            if c != u16::from(b) {
                return false;
            }
        }
    }
    true
}

fn sessions(c: &mut Checks) -> Result<(), u32> {
    let (mut w, mut a) = (null_mut(), null_mut());
    let (mut n, mut n2) = (0u32, 0u32);
    // SAFETY: our outputs; the lists are read within their counts.
    unsafe {
        c.eq(WTSEnumerateSessionsW(0, 0, 1, &mut w, &mut n) as u64, 1)?;
        c.ok(n >= 2, n.into())?;
        c.eq(WTSEnumerateSessionsA(0, 0, 1, &mut a, &mut n2) as u64, 1)?;
        c.eq(n2.into(), n.into())?;
        let (mut console, mut services) = (false, false);
        for i in 0..n as usize {
            let (sw, sa) = (&*w.add(i), &*a.add(i));
            c.eq(sa.id.into(), sw.id.into())?;
            if name_is(sw.name, 2, b"Console") {
                console = true;
                c.ok(sw.state == WTS_ACTIVE && name_is(sa.name, 1, b"Console"), sw.state.into())?;
                c.eq(sw.id.into(), WTSGetActiveConsoleSessionId().into())?;
            } else if name_is(sw.name, 2, b"Services") {
                services = true;
                c.ok(sw.state == WTS_DISCONNECTED && sw.id == 0, sw.state.into())?;
                c.ok(name_is(sa.name, 1, b"Services"), 0)?;
            }
        }
        c.ok(console && services, u64::from(console))?;
        WTSFreeMemory(w);
        WTSFreeMemory(a);
        let ok = WTSEnumerateSessionsW(0, 0, 2, &mut w, &mut n) != 0;
        c.fails(ok, 87)?;
    }
    Ok(())
}

fn relative_descriptor(c: &mut Checks) -> Result<(), u32> {
    let mut abs = [0usize; 6];
    let p = abs.as_mut_ptr().cast::<u8>();
    let mut acl = [0u32; 4];
    let mut owner = [1u8, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0]; // S-1-1-0
    let mut rel = [0u32; 32];
    let r = rel.as_mut_ptr().cast::<u8>();
    // SAFETY: our descriptors and buffers.
    unsafe {
        InitializeSecurityDescriptor(p, 1);
        InitializeAcl(acl.as_mut_ptr().cast(), 16, 2);
        SetSecurityDescriptorOwner(p, owner.as_mut_ptr(), 0);
        SetSecurityDescriptorDacl(p, 1, acl.as_mut_ptr().cast(), 0);
        let mut len = 128u32;
        c.eq(MakeSelfRelativeSD(p, r, &mut len) as u64, 1)?;
        c.eq(len.into(), 128)?;
        // The header, the SID and the ACL (AclSize 16, no ACEs).
        let size = 20 + 12 + 16;
        c.eq(RtlValidRelativeSecurityDescriptor(r, size, 0).into(), 1)?;
        c.eq(RtlValidRelativeSecurityDescriptor(r, size, 1 | 4).into(), 1)?;
        c.eq(RtlValidRelativeSecurityDescriptor(r, size, 2).into(), 0)?;
        c.eq(RtlValidRelativeSecurityDescriptor(r, size - 1, 0).into(), 0)?;
        c.eq(RtlValidRelativeSecurityDescriptor(r, 19, 0).into(), 0)?;
        c.eq(RtlValidRelativeSecurityDescriptor(p, 20, 0).into(), 0)?;
        c.eq(RtlValidRelativeSecurityDescriptor(null(), 20, 0).into(), 0)?;
        // An owner offset past the length.
        *r.add(4).cast::<u32>() = 200;
        c.eq(RtlValidRelativeSecurityDescriptor(r, size, 0).into(), 0)?;
    }
    Ok(())
}

pub fn c2r_service(c: &mut Checks) -> Result<(), u32> {
    sessions(c)?;
    relative_descriptor(c)
}
