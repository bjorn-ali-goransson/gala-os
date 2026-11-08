//! netapi32's NetWkstaGetInfo, with the results real Windows gives on a
//! workstation that is not in a domain: level 100 (WKSTA_INFO_100) holds
//! PLATFORM_ID_NT (500), the computer name (GetComputerNameW's), the
//! workgroup and the Windows version (10.0); level 102 adds the logged-on
//! users; the buffer is freed with NetApiBufferFree; an unknown level is
//! ERROR_INVALID_LEVEL.
//!
//! NetUserGetInfo (Chromium's password manager reads the logged-on user's
//! password age with it): for the user GetUserNameW names, level 0
//! (USER_INFO_0) is the name; level 1 (USER_INFO_1) adds a null password,
//! the privilege level (USER_PRIV_USER or USER_PRIV_ADMIN) and flags with
//! UF_SCRIPT and UF_NORMAL_ACCOUNT; a user that does not exist is
//! NERR_UserNotFound (2221), an unknown level ERROR_INVALID_LEVEL.
//!
//! Prints `netapi32_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Handle = *mut c_void;

/// WKSTA_INFO_102 (its first fields are WKSTA_INFO_100's).
#[repr(C)]
struct WkstaInfo102 {
    platform_id: u32,
    computername: *const u16,
    langroup: *const u16,
    ver_major: u32,
    ver_minor: u32,
    lanroot: *const u16,
    logged_on_users: u32,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetComputerNameW(buf: *mut u16, size: *mut u32) -> i32;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetUserNameW(buf: *mut u16, size: *mut u32) -> i32;
}

/// USER_INFO_1 (lmaccess.h).
#[repr(C)]
struct UserInfo1 {
    name: *const u16,
    password: *const u16,
    password_age: u32,
    privilege: u32,
    home_dir: *const u16,
    comment: *const u16,
    flags: u32,
    script_path: *const u16,
}

#[link(name = "netapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn NetWkstaGetInfo(server: *const u16, level: u32, out: *mut *mut u8) -> u32;
    fn NetApiBufferFree(buffer: *mut u8) -> u32;
    fn NetUserGetInfo(server: *const u16, user: *const u16, level: u32, out: *mut *mut u8) -> u32;
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

const NERR_SUCCESS: u64 = 0;
const ERROR_INVALID_LEVEL: u64 = 124;

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
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"netapi32_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        check_line(core::panic::Location::caller().line());
        out(b"\r\n");
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

trait EqWide {
    fn eq_ignore_ascii_case_wide(&self, other: &[u16]) -> bool;
}

impl EqWide for [u16] {
    fn eq_ignore_ascii_case_wide(&self, other: &[u16]) -> bool {
        let fold = |c: u16| if (65..=90).contains(&c) { c + 32 } else { c };
        self.len() == other.len() && self.iter().zip(other).all(|(&a, &b)| fold(a) == fold(b))
    }
}

/// The NUL-terminated string at `p`.
fn wide(p: *const u16) -> &'static [u16] {
    let mut n = 0;
    // SAFETY: callers pass NUL-terminated strings.
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: netapi32 calls with buffers of ours; the buffers it returns
    // hold the documented structures.
    unsafe {
        let mut name = [0u16; 32];
        let mut size = 32;
        c.ok(GetComputerNameW(name.as_mut_ptr(), &mut size) != 0, 0)?;
        let mut buf = core::ptr::null_mut();
        c.eq(NetWkstaGetInfo(core::ptr::null(), 100, &mut buf).into(), NERR_SUCCESS)?;
        let info = &*(buf as *const WkstaInfo102);
        c.eq(info.platform_id.into(), 500)?; // PLATFORM_ID_NT
        c.ok(wide(info.computername) == &name[..size as usize], 0)?;
        c.ok(wide(info.langroup).iter().copied().eq(b"WORKGROUP".map(u16::from)), 0)?;
        c.eq(info.ver_major.into(), 10)?;
        c.eq(info.ver_minor.into(), 0)?;
        c.eq(NetApiBufferFree(buf).into(), NERR_SUCCESS)?;

        c.eq(NetWkstaGetInfo(core::ptr::null(), 102, &mut buf).into(), NERR_SUCCESS)?;
        let info = &*(buf as *const WkstaInfo102);
        c.ok(wide(info.computername) == &name[..size as usize], 0)?;
        c.ok(info.logged_on_users >= 1, info.logged_on_users.into())?;
        c.eq(NetApiBufferFree(buf).into(), NERR_SUCCESS)?;

        c.eq(NetWkstaGetInfo(core::ptr::null(), 999, &mut buf).into(), ERROR_INVALID_LEVEL)?;

        // NetUserGetInfo for the logged-on user.
        let mut user = [0u16; 64];
        let mut size = 64;
        c.ok(GetUserNameW(user.as_mut_ptr(), &mut size) != 0, 0)?;
        let user_name = &user[..size as usize - 1];
        c.eq(NetUserGetInfo(core::ptr::null(), user.as_ptr(), 0, &mut buf).into(), NERR_SUCCESS)?;
        c.ok(wide(*(buf as *const *const u16)).eq_ignore_ascii_case_wide(user_name), 0)?;
        c.eq(NetApiBufferFree(buf).into(), NERR_SUCCESS)?;
        c.eq(NetUserGetInfo(core::ptr::null(), user.as_ptr(), 1, &mut buf).into(), NERR_SUCCESS)?;
        let info = &*(buf as *const UserInfo1);
        c.ok(wide(info.name).eq_ignore_ascii_case_wide(user_name), 0)?;
        c.ok(info.password.is_null(), info.password as u64)?;
        c.ok(matches!(info.privilege, 1 | 2), info.privilege.into())?; // USER_PRIV_USER/ADMIN
        c.eq((info.flags & 0x201).into(), 0x201)?; // UF_SCRIPT | UF_NORMAL_ACCOUNT
        c.eq(NetApiBufferFree(buf).into(), NERR_SUCCESS)?;
        let nobody = b"no-such-user-gala\0".map(u16::from);
        c.eq(NetUserGetInfo(core::ptr::null(), nobody.as_ptr(), 1, &mut buf).into(), 2221)?;
        c.eq(
            NetUserGetInfo(core::ptr::null(), user.as_ptr(), 7777, &mut buf).into(),
            ERROR_INVALID_LEVEL,
        )?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"netapi32_test: ok\r\n");
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

/// ` (line L)`: the source line of the failing check, from
/// `#[track_caller]` on Checks' methods (so a report names the line
/// without anyone counting checks).
fn check_line(line: u32) {
    let mut b = *b" (line 0000000000)";
    let mut digits = [0u8; 10];
    let (mut n, mut len) = (line, 0);
    loop {
        digits[len] = b'0' + (n % 10) as u8;
        len += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for i in 0..len {
        b[7 + i] = digits[len - 1 - i];
    }
    b[7 + len] = b')';
    out(&b[..8 + len]);
}
