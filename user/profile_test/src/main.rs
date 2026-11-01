//! User profiles as a service loads them for a logged-on user
//! (docs/SECURITY.md, phase S4; sshd-session does this), with the results
//! Windows documents (LoadUserProfileW, PROFILEINFOW, UnloadUserProfile,
//! GetUserProfileDirectoryW). Runs as SYSTEM (gala: `as=system`; Windows:
//! `psexec -s`) and needs the second local account `galatest` (see
//! access_test).
//!
//! - LoadUserProfileW of `galatest`'s token: TRUE; PROFILEINFOW.hProfile is
//!   an HKEY of the user's hive (HKEY_USERS\<SID>: a key created under it
//!   is found there).
//! - The profile directory exists (GetUserProfileDirectoryW: under
//!   C:\Users, named after the user) and its DACL gives the user
//!   FILE_ALL_ACCESS.
//! - UnloadUserProfile: TRUE.
//!
//! Prints `profile_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetLastError() -> u32;
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn LocalFree(p: *mut c_void) -> *mut c_void;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LogonUserW(
        user: *const u16,
        domain: *const u16,
        password: *const u16,
        kind: u32,
        provider: u32,
        token: *mut Handle,
    ) -> Bool;
    fn GetTokenInformation(
        token: Handle,
        class: u32,
        out: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> Bool;
    fn EqualSid(a: *const u8, b: *const u8) -> Bool;
    fn ConvertSidToStringSidW(sid: *const u8, text: *mut *mut u16) -> Bool;
    fn GetFileSecurityW(
        path: *const u16,
        info: u32,
        sd: *mut u8,
        length: u32,
        needed: *mut u32,
    ) -> Bool;
    fn GetSecurityDescriptorDacl(
        sd: *const u8,
        present: *mut Bool,
        dacl: *mut *const u8,
        defaulted: *mut Bool,
    ) -> Bool;
    fn RegCreateKeyExW(
        key: usize,
        sub: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        access: u32,
        sa: *const c_void,
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
    fn RegCloseKey(key: usize) -> i32;
    fn RegDeleteKeyW(key: usize, sub: *const u16) -> i32;
}

#[link(name = "userenv", kind = "raw-dylib")]
unsafe extern "system" {
    fn LoadUserProfileW(token: Handle, info: *mut ProfileInfo) -> Bool;
    fn UnloadUserProfile(token: Handle, profile: usize) -> Bool;
    fn GetUserProfileDirectoryW(token: Handle, dir: *mut u16, size: *mut u32) -> Bool;
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

/// PROFILEINFOW.
#[repr(C)]
struct ProfileInfo {
    size: u32,
    flags: u32,
    user_name: *const u16,
    profile_path: *const u16,
    default_path: *const u16,
    server_name: *const u16,
    policy_path: *const u16,
    profile: usize,
}

const HKEY_USERS: usize = 0x8000_0003;
const KEY_ALL_ACCESS: u32 = 0x000f_003f;
const KEY_READ: u32 = 0x0002_0019;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ALL_ACCESS: u32 = 0x001f_01ff;
const PI_NOUI: u32 = 1;

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
        out(b"profile_test: check ");
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

fn w<const N: usize>(s: &str) -> [u16; N] {
    let mut b = [0u16; N];
    for (slot, c) in b.iter_mut().zip(s.encode_utf16()) {
        *slot = c;
    }
    b
}

#[repr(C, align(8))]
struct Buf([u8; 1024]);

fn run() -> Result<(), u32> {
    let c = Checks(core::cell::Cell::new(0));
    let null = core::ptr::null_mut();
    // SAFETY: plain calls with our buffers and handles throughout.
    unsafe {
        let mut token = null;
        let (user, dot, password) = (w::<16>("galatest"), w::<4>("."), w::<16>("Gala-Test-1"));
        c.ok(
            LogonUserW(user.as_ptr(), dot.as_ptr(), password.as_ptr(), 2, 0, &mut token) != 0,
            last_error(),
        )?;
        let mut tu = Buf([0; 1024]);
        let mut n = 0;
        c.ok(GetTokenInformation(token, 1, tu.0.as_mut_ptr(), 1024, &mut n) != 0, 0)?;
        let sid = tu.0.as_ptr().cast::<*const u8>().read();
        let mut sid_text = null.cast::<u16>();
        c.ok(ConvertSidToStringSidW(sid, &mut sid_text) != 0, 0)?;

        // LoadUserProfileW.
        let mut info = ProfileInfo {
            size: core::mem::size_of::<ProfileInfo>() as u32,
            flags: PI_NOUI,
            user_name: user.as_ptr(),
            profile_path: core::ptr::null(),
            default_path: core::ptr::null(),
            server_name: core::ptr::null(),
            policy_path: core::ptr::null(),
            profile: 0,
        };
        c.ok(LoadUserProfileW(token, &mut info) != 0, last_error())?;
        c.ok(info.profile != 0, 0)?;

        // hProfile is the user's hive: HKEY_USERS\<SID>.
        let name = w::<32>("Software\\gala-profile-test");
        let (mut k, mut disposition) = (0usize, 0u32);
        let r = RegCreateKeyExW(
            info.profile,
            name.as_ptr(),
            0,
            core::ptr::null(),
            0,
            KEY_ALL_ACCESS,
            core::ptr::null(),
            &mut k,
            &mut disposition,
        );
        c.eq(r as u64, 0)?;
        RegCloseKey(k);
        let mut path = [0u16; 128];
        let mut at = 0;
        while *sid_text.add(at) != 0 {
            path[at] = *sid_text.add(at);
            at += 1;
        }
        for (i, ch) in "\\Software\\gala-profile-test".encode_utf16().enumerate() {
            path[at + i] = ch;
        }
        let mut found = 0usize;
        c.eq(RegOpenKeyExW(HKEY_USERS, path.as_ptr(), 0, KEY_READ, &mut found) as u64, 0)?;
        RegCloseKey(found);
        c.eq(RegDeleteKeyW(info.profile, name.as_ptr()) as u64, 0)?;

        // The profile directory.
        let mut dir = [0u16; 260];
        let mut size = 260u32;
        c.ok(GetUserProfileDirectoryW(token, dir.as_mut_ptr(), &mut size) != 0, last_error())?;
        let prefix = w::<9>("C:\\Users\\");
        let named = w::<8>("galatest");
        let starts = (0..9).all(|i| (dir[i] | 0x20) == (prefix[i] | 0x20) || dir[i] == prefix[i]);
        c.ok(starts && (0..8).all(|i| (dir[9 + i] | 0x20) == named[i]), 0)?;
        let attributes = GetFileAttributesW(dir.as_ptr());
        c.ok(
            attributes != u32::MAX && attributes & FILE_ATTRIBUTE_DIRECTORY != 0,
            attributes.into(),
        )?;
        let mut sd = Buf([0; 1024]);
        c.ok(
            GetFileSecurityW(dir.as_ptr(), 4, sd.0.as_mut_ptr(), 1024, &mut n) != 0,
            last_error(),
        )?;
        let (mut present, mut dacl, mut defaulted) = (0, core::ptr::null(), 0);
        c.ok(
            GetSecurityDescriptorDacl(sd.0.as_ptr(), &mut present, &mut dacl, &mut defaulted) != 0,
            0,
        )?;
        c.ok(present != 0 && !dacl.is_null(), 0)?;
        let count = usize::from(dacl.add(4).cast::<u16>().read_unaligned());
        let mut ace = dacl.add(8);
        let mut user_full = false;
        for _ in 0..count {
            let size = usize::from(ace.add(2).cast::<u16>().read_unaligned());
            let mask = ace.add(4).cast::<u32>().read_unaligned();
            if *ace == 0
                && mask & FILE_ALL_ACCESS == FILE_ALL_ACCESS
                && EqualSid(ace.add(8), sid) != 0
            {
                user_full = true;
            }
            ace = ace.add(size);
        }
        c.ok(user_full, 0)?;

        c.ok(UnloadUserProfile(token, info.profile) != 0, last_error())?;
        LocalFree(sid_text.cast());
        CloseHandle(token);
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"profile_test: ok\r\n");
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
