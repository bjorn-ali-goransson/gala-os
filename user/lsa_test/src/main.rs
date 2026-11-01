//! LSA logons as Win32-OpenSSH's sshd makes them (docs/SECURITY.md,
//! phase S4), with the results Windows documents (LsaConnectUntrusted,
//! LsaRegisterLogonProcess, LsaLookupAuthenticationPackage, LsaLogonUser,
//! MSV1_0_S4U_LOGON, MSV1_0_INTERACTIVE_LOGON, LsaFreeReturnBuffer,
//! LsaDeregisterLogonProcess, LsaNtStatusToWinError). Runs as SYSTEM (gala:
//! the boot option `as=system`; Windows: `psexec -s`) and needs the
//! second local account `galatest` (see access_test).
//!
//! - An untrusted connection finds the MSV1_0 package
//!   (MICROSOFT_AUTHENTICATION_PACKAGE_V1_0); an unknown package is
//!   STATUS_NO_SUCH_PACKAGE.
//! - A registered logon process (SeTcbPrivilege) logs `galatest` on with
//!   S4U (no password, a network logon: the token's user is `galatest`,
//!   it has the NETWORK group, a new logon session) and with its password
//!   (MsV1_0InteractiveLogon: the INTERACTIVE group); a wrong password
//!   is STATUS_LOGON_FAILURE, which LsaNtStatusToWinError makes
//!   ERROR_LOGON_FAILURE. The profile buffer is freed with
//!   LsaFreeReturnBuffer.
//!
//! Prints `lsa_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Handle = *mut c_void;

#[repr(C)]
struct LsaString {
    length: u16,
    maximum: u16,
    buffer: *const u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UnicodeString {
    length: u16,
    maximum: u16,
    buffer: *const u16,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> i32;
}

#[link(name = "secur32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LsaConnectUntrusted(handle: *mut Handle) -> u32;
    fn LsaRegisterLogonProcess(name: *const LsaString, handle: *mut Handle, mode: *mut u32) -> u32;
    fn LsaLookupAuthenticationPackage(
        handle: Handle,
        name: *const LsaString,
        package: *mut u32,
    ) -> u32;
    #[allow(clippy::too_many_arguments)]
    fn LsaLogonUser(
        handle: Handle,
        origin: *const LsaString,
        kind: u32,
        package: u32,
        info: *const u8,
        info_length: u32,
        groups: *const u8,
        source: *const [u8; 16],
        profile: *mut *mut c_void,
        profile_length: *mut u32,
        logon_id: *mut u64,
        token: *mut Handle,
        quotas: *mut [u64; 6],
        substatus: *mut u32,
    ) -> u32;
    fn LsaFreeReturnBuffer(buffer: *mut c_void) -> u32;
    fn LsaDeregisterLogonProcess(handle: Handle) -> u32;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LsaNtStatusToWinError(status: u32) -> u32;
    fn GetTokenInformation(token: Handle, class: u32, out: *mut u8, len: u32, ret: *mut u32)
    -> i32;
    fn EqualSid(a: *const u8, b: *const u8) -> i32;
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

const STATUS_NO_SUCH_PACKAGE: u32 = 0xc000_00fe;
const STATUS_LOGON_FAILURE: u32 = 0xc000_006d;
const ERROR_LOGON_FAILURE: u32 = 1326;
const NETWORK: u32 = 3;
const INTERACTIVE: u32 = 2;
const MSV1_0_INTERACTIVE_LOGON: u32 = 2;
const MSV1_0_S4U_LOGON: u32 = 12;

/// S-1-5-2 (NETWORK) and S-1-5-4 (INTERACTIVE).
const NETWORK_SID: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 5, 2, 0, 0, 0];
const INTERACTIVE_SID: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 5, 4, 0, 0, 0];

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
        out(b"lsa_test: check ");
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

fn lsa(s: &'static [u8]) -> LsaString {
    LsaString { length: s.len() as u16, maximum: s.len() as u16, buffer: s.as_ptr() }
}

fn w<const N: usize>(s: &str) -> [u16; N] {
    let mut b = [0u16; N];
    for (slot, c) in b.iter_mut().zip(s.encode_utf16()) {
        *slot = c;
    }
    b
}

fn unicode(b: &[u16]) -> UnicodeString {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    UnicodeString { length: (2 * n) as u16, maximum: (2 * n) as u16, buffer: b.as_ptr() }
}

#[repr(C, align(8))]
struct Buf([u8; 1024]);

/// Whether `sid` is among the token's groups.
fn has_group(token: Handle, sid: &[u8]) -> bool {
    let mut b = Buf([0; 1024]);
    let mut n = 0;
    // SAFETY: our buffer; TOKEN_GROUPS from the system.
    unsafe {
        if GetTokenInformation(token, 2, b.0.as_mut_ptr(), 1024, &mut n) == 0 {
            return false;
        }
        let count = b.0.as_ptr().cast::<u32>().read() as usize;
        (0..count).any(|i| {
            let e = b.0.as_ptr().add(8 + 16 * i);
            EqualSid(e.cast::<*const u8>().read(), sid.as_ptr()) != 0
        })
    }
}

/// The last sub-authority (RID) of the token's user.
fn user_rid(token: Handle) -> u32 {
    let mut b = Buf([0; 1024]);
    let mut n = 0;
    // SAFETY: our buffer; TOKEN_USER's SID.
    unsafe {
        if GetTokenInformation(token, 1, b.0.as_mut_ptr(), 1024, &mut n) == 0 {
            return 0;
        }
        let sid = b.0.as_ptr().cast::<*const u8>().read();
        sid.add(8 + 4 * (usize::from(*sid.add(1)) - 1)).cast::<u32>().read_unaligned()
    }
}

/// One LsaLogonUser: (status, token, logon id).
fn logon(lsa_handle: Handle, kind: u32, package: u32, info: &[u8]) -> (u32, Handle, u64) {
    let origin = lsa(b"lsa_test");
    let mut source = [0u8; 16];
    source[..8].copy_from_slice(b"lsatest\0");
    let (mut profile, mut profile_length, mut id, mut token) =
        (core::ptr::null_mut(), 0, 0u64, core::ptr::null_mut());
    let (mut quotas, mut substatus) = ([0u64; 6], 0u32);
    // SAFETY: our buffers.
    let status = unsafe {
        LsaLogonUser(
            lsa_handle,
            &origin,
            kind,
            package,
            info.as_ptr(),
            info.len() as u32,
            core::ptr::null(),
            &source,
            &mut profile,
            &mut profile_length,
            &mut id,
            &mut token,
            &mut quotas,
            &mut substatus,
        )
    };
    if !profile.is_null() {
        // SAFETY: the buffer LsaLogonUser allocated.
        unsafe { LsaFreeReturnBuffer(profile) };
    }
    (status, token, id)
}

/// MSV1_0_S4U_LOGON for `user` (strings after the 40-byte structure).
fn s4u(user: &[u16; 16], domain: &[u16; 8], b: &mut [u8; 256]) -> usize {
    let at = b.as_ptr() as usize;
    let put = |b: &mut [u8; 256], off: usize, s: &[u16], pos: usize| -> usize {
        let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
        for (i, &c) in s[..n].iter().enumerate() {
            b[pos + 2 * i..pos + 2 * i + 2].copy_from_slice(&c.to_le_bytes());
        }
        b[off..off + 2].copy_from_slice(&((2 * n) as u16).to_le_bytes());
        b[off + 2..off + 4].copy_from_slice(&((2 * n) as u16).to_le_bytes());
        b[off + 8..off + 16].copy_from_slice(&((at + pos) as u64).to_le_bytes());
        pos + 2 * n
    };
    b[..4].copy_from_slice(&MSV1_0_S4U_LOGON.to_le_bytes());
    let end = put(b, 8, user, 40);
    put(b, 24, domain, end)
}

fn run() -> Result<(), u32> {
    let c = Checks(core::cell::Cell::new(0));
    let null = core::ptr::null_mut();
    // SAFETY: plain calls with our buffers and handles throughout.
    unsafe {
        let msv = lsa(b"MICROSOFT_AUTHENTICATION_PACKAGE_V1_0");
        let mut untrusted = null;
        c.eq(LsaConnectUntrusted(&mut untrusted).into(), 0)?;
        let mut package = u32::MAX;
        c.eq(LsaLookupAuthenticationPackage(untrusted, &msv, &mut package).into(), 0)?;
        c.ok(package != u32::MAX, package.into())?;
        let mut other = 0;
        let nope = lsa(b"NO_SUCH_PACKAGE");
        c.eq(
            LsaLookupAuthenticationPackage(untrusted, &nope, &mut other).into(),
            STATUS_NO_SUCH_PACKAGE.into(),
        )?;
        c.eq(LsaDeregisterLogonProcess(untrusted).into(), 0)?;

        // A registered logon process (SeTcbPrivilege).
        let name = lsa(b"lsa_test");
        let (mut lsa_handle, mut mode) = (null, 0u32);
        c.eq(LsaRegisterLogonProcess(&name, &mut lsa_handle, &mut mode).into(), 0)?;
        let mut msv_package = 0;
        c.eq(LsaLookupAuthenticationPackage(lsa_handle, &msv, &mut msv_package).into(), 0)?;
        c.eq(msv_package.into(), package.into())?;

        // S4U: no password.
        let (user, domain) = (w::<16>("galatest"), w::<8>("."));
        let mut info = [0u8; 256];
        let n = s4u(&user, &domain, &mut info);
        let (status, token, id) = logon(lsa_handle, NETWORK, msv_package, &info[..n]);
        c.eq(status.into(), 0)?;
        c.ok(!token.is_null() && id != 0, id)?;
        let rid = user_rid(token);
        c.ok(rid >= 1000, rid.into())?;
        c.ok(has_group(token, &NETWORK_SID), 0)?;
        c.ok(!has_group(token, &INTERACTIVE_SID), 0)?;
        CloseHandle(token);

        // MsV1_0InteractiveLogon: with the password.
        let password = w::<16>("Gala-Test-1");
        let wrong = w::<16>("wrong");
        let interactive = |password: &[u16; 16]| {
            let mut b = [0u8; 56];
            b[..4].copy_from_slice(&MSV1_0_INTERACTIVE_LOGON.to_le_bytes());
            let parts = [unicode(&domain), unicode(&user), unicode(password)];
            for (i, s) in parts.iter().enumerate() {
                let off = 8 + 16 * i;
                b[off..off + 2].copy_from_slice(&s.length.to_le_bytes());
                b[off + 2..off + 4].copy_from_slice(&s.maximum.to_le_bytes());
                b[off + 8..off + 16].copy_from_slice(&(s.buffer as u64).to_le_bytes());
            }
            b
        };
        let (status, token, _) =
            logon(lsa_handle, INTERACTIVE, msv_package, &interactive(&password));
        c.eq(status.into(), 0)?;
        c.eq(user_rid(token).into(), rid.into())?;
        c.ok(has_group(token, &INTERACTIVE_SID), 0)?;
        CloseHandle(token);
        let (status, _, _) = logon(lsa_handle, INTERACTIVE, msv_package, &interactive(&wrong));
        c.eq(status.into(), STATUS_LOGON_FAILURE.into())?;
        c.eq(LsaNtStatusToWinError(STATUS_LOGON_FAILURE).into(), ERROR_LOGON_FAILURE.into())?;
        c.eq(LsaDeregisterLogonProcess(lsa_handle).into(), 0)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"lsa_test: ok\r\n");
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
