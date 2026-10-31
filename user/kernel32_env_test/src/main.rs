//! kernel32's environment, time zone and directory queries, as the Go
//! `os` and `time` packages use them, with the results real Windows gives:
//! GetEnvironmentVariableW (length rules, ERROR_ENVVAR_NOT_FOUND),
//! SetEnvironmentVariableW (set, replace, delete; visible in
//! GetEnvironmentStringsW), GetTimeZoneInformation, GetCurrentDirectoryW
//! (the PEB's current directory), GetTempPathW/GetTempPath2W (a directory
//! path ending in a backslash), the user's profile variables (USERPROFILE,
//! APPDATA, LOCALAPPDATA, ProgramData: existing directories),
//! GetSystemDirectoryW / GetWindowsDirectoryW, AddDllDirectory /
//! RemoveDllDirectory / SetDefaultDllDirectories, ExpandEnvironmentStringsW/A
//! (unset variables and a lone % stay as they are). Prints
//! `kernel32_env_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn GetLastError() -> u32;
    fn SetLastError(code: u32);
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn SetEnvironmentVariableW(name: *const u16, value: *const u16) -> Bool;
    fn ExpandEnvironmentStringsW(src: *const u16, dst: *mut u16, size: u32) -> u32;
    fn ExpandEnvironmentStringsA(src: *const u8, dst: *mut u8, size: u32) -> u32;
    fn GetEnvironmentStringsW() -> *mut u16;
    fn FreeEnvironmentStringsW(block: *mut u16) -> Bool;
    fn GetTimeZoneInformation(info: *mut u8) -> u32;
    fn GetCurrentDirectoryW(size: u32, buf: *mut u16) -> u32;
    fn GetTempPathW(size: u32, buf: *mut u16) -> u32;
    fn GetTempPath2W(size: u32, buf: *mut u16) -> u32;
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn GetWindowsDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn AddDllDirectory(path: *const u16) -> *mut c_void;
    fn RemoveDllDirectory(cookie: *mut c_void) -> Bool;
    fn SetDefaultDllDirectories(flags: u32) -> Bool;
    fn ExitProcess(code: u32) -> !;
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

const ERROR_ENVVAR_NOT_FOUND: u32 = 203;

macro_rules! w {
    ($s:literal) => {{
        const S: &str = concat!($s, "\0");
        const N: usize = S.len();
        const W: [u16; N] = {
            let b = S.as_bytes();
            let mut w = [0u16; N];
            let mut i = 0;
            while i < N {
                w[i] = b[i] as u16;
                i += 1;
            }
            w
        };
        W.as_ptr()
    }};
}

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
        out(b"kernel32_env_test: check ");
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

fn ascii_eq(w: &[u16], s: &[u8]) -> bool {
    w.len() == s.len() && w.iter().zip(s).all(|(&a, &b)| a == u16::from(b))
}

/// Whether the environment block contains exactly `entry` (NAME=value).
fn block_has(entry: &[u8]) -> bool {
    // SAFETY: a block of NUL-terminated strings ending with an empty one.
    unsafe {
        let env = GetEnvironmentStringsW();
        let (mut i, mut found) = (0, false);
        loop {
            let start = i;
            while *env.add(i) != 0 {
                i += 1;
            }
            if i == start {
                break;
            }
            found |= ascii_eq(core::slice::from_raw_parts(env.add(start), i - start), entry);
            i += 1;
        }
        FreeEnvironmentStringsW(env);
        found
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let mut buf = [0u16; 300];
    // SAFETY: documented kernel32 calls with valid arguments.
    unsafe {
        // A variable that is not set.
        SetLastError(0);
        c.eq(GetEnvironmentVariableW(w!("GALA_NOT_SET"), buf.as_mut_ptr(), 300).into(), 0)?;
        c.eq(GetLastError().into(), ERROR_ENVVAR_NOT_FOUND.into())?;

        // Set, read (case-insensitive name), lengths.
        c.ok(SetEnvironmentVariableW(w!("GALA_VAR"), w!("qala")) != 0, 0)?;
        c.eq(GetEnvironmentVariableW(w!("gala_var"), buf.as_mut_ptr(), 300).into(), 4)?;
        c.ok(ascii_eq(&buf[..4], b"qala") && buf[4] == 0, 0)?;
        // Too small a buffer: the size needed, including the NUL.
        c.eq(GetEnvironmentVariableW(w!("GALA_VAR"), buf.as_mut_ptr(), 2).into(), 5)?;
        c.ok(block_has(b"GALA_VAR=qala"), 0)?;

        // Replace, then delete.
        c.ok(SetEnvironmentVariableW(w!("GALA_VAR"), w!("fortress")) != 0, 0)?;
        c.eq(GetEnvironmentVariableW(w!("GALA_VAR"), buf.as_mut_ptr(), 300).into(), 8)?;
        c.ok(block_has(b"GALA_VAR=fortress") && !block_has(b"GALA_VAR=qala"), 0)?;
        c.ok(SetEnvironmentVariableW(w!("GALA_VAR"), core::ptr::null()) != 0, 0)?;
        SetLastError(0);
        c.eq(GetEnvironmentVariableW(w!("GALA_VAR"), buf.as_mut_ptr(), 300).into(), 0)?;
        c.eq(GetLastError().into(), ERROR_ENVVAR_NOT_FOUND.into())?;
        c.ok(!block_has(b"GALA_VAR=fortress"), 0)?;
        // Variables from the start still there.
        c.ok(GetEnvironmentVariableW(w!("SystemRoot"), buf.as_mut_ptr(), 300) > 0, 0)?;

        // Expansion: set variables replaced (names ignore case), others
        // and a lone % left as they are; the count includes the NUL.
        c.ok(SetEnvironmentVariableW(w!("GALA_VAR"), w!("qala")) != 0, 0)?;
        let src = w!("a%gala_var%b %GALA_NOT_SET% 100%");
        c.eq(ExpandEnvironmentStringsW(src, buf.as_mut_ptr(), 300).into(), 27)?;
        c.ok(ascii_eq(&buf[..26], b"aqalab %GALA_NOT_SET% 100%") && buf[26] == 0, 0)?;
        // Too small: the size needed.
        c.eq(ExpandEnvironmentStringsW(src, buf.as_mut_ptr(), 3).into(), 27)?;
        let mut narrow = [0u8; 64];
        c.eq(
            ExpandEnvironmentStringsA(c"x%GALA_VAR%".as_ptr().cast(), narrow.as_mut_ptr(), 64)
                .into(),
            6,
        )?;
        c.ok(&narrow[..6] == b"xqala\0", narrow[0].into())?;
        SetEnvironmentVariableW(w!("GALA_VAR"), core::ptr::null());

        // Time zone: a valid answer (TIME_ZONE_ID_UNKNOWN/STANDARD/DAYLIGHT).
        let mut tzi = [0u8; 172];
        let id = GetTimeZoneInformation(tzi.as_mut_ptr());
        c.ok(id <= 2, id.into())?;

        // Current directory: the PEB's, with its trailing backslash.
        let n = GetCurrentDirectoryW(300, buf.as_mut_ptr()) as usize;
        let peb: usize;
        core::arch::asm!("mov {}, gs:[0x60]", out(reg) peb, options(nostack, readonly));
        let params = *((peb + 0x20) as *const usize);
        let dos_len = usize::from(*((params + 0x38) as *const u16)) / 2;
        let dos = core::slice::from_raw_parts(*((params + 0x40) as *const *const u16), dos_len);
        // GetCurrentDirectoryW drops the trailing backslash except for a root.
        c.ok(
            n > 2 && buf[1] == u16::from(b':') && dos.starts_with(&buf[..n]) && buf[n] == 0,
            n as u64,
        )?;
        c.ok(n == 3 || buf[n - 1] != u16::from(b'\\'), n as u64)?;
        // Too small: the size needed, including the NUL.
        c.eq(GetCurrentDirectoryW(2, buf.as_mut_ptr()).into(), n as u64 + 1)?;

        // The temporary directory: a path ending in a backslash.
        let n = GetTempPathW(300, buf.as_mut_ptr()) as usize;
        c.ok(
            n > 3 && buf[1] == u16::from(b':') && buf[n - 1] == u16::from(b'\\') && buf[n] == 0,
            n as u64,
        )?;
        let mut buf2 = [0u16; 300];
        let n2 = GetTempPath2W(300, buf2.as_mut_ptr()) as usize;
        c.ok(n2 > 3 && buf2[n2 - 1] == u16::from(b'\\'), n2 as u64)?;

        // The user's profile: variables naming existing directories.
        for name in [w!("USERPROFILE"), w!("APPDATA"), w!("LOCALAPPDATA"), w!("ProgramData")] {
            let n = GetEnvironmentVariableW(name, buf.as_mut_ptr(), 300) as usize;
            c.ok(n > 3 && buf[1] == u16::from(b':'), n as u64)?;
            let attributes = GetFileAttributesW(buf.as_ptr());
            c.ok(attributes != u32::MAX && attributes & 0x10 != 0, attributes.into())?;
        }

        // The system directories: Windows\System32 under the Windows one.
        let n = GetWindowsDirectoryW(buf.as_mut_ptr(), 300) as usize;
        let s = GetSystemDirectoryW(buf2.as_mut_ptr(), 300) as usize;
        let tail: [u16; 9] = (*b"\\system32").map(u16::from);
        let lower = |c: u16| if (65..=90).contains(&c) { c + 32 } else { c };
        c.ok(n > 2 && s == n + 9 && buf2[..n] == buf[..n], s as u64)?;
        c.ok(buf2[n..s].iter().zip(&tail).all(|(&a, &b)| lower(a) == b) && buf2[s] == 0, 0)?;
        c.eq(GetSystemDirectoryW(buf2.as_mut_ptr(), 3).into(), s as u64 + 1)?; // too small
        c.ok(GetFileAttributesW(buf2.as_ptr()) & 0x10 != 0, 0)?;

        // DLL directories: added, removed; the default search set.
        let cookie = AddDllDirectory(buf.as_ptr());
        c.ok(!cookie.is_null(), GetLastError().into())?;
        c.ok(RemoveDllDirectory(cookie) != 0, GetLastError().into())?;
        c.ok(SetDefaultDllDirectories(0x1000) != 0, GetLastError().into())?; // DEFAULT_DIRS
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"kernel32_env_test: ok\r\n");
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
