//! The start menu's "Linux distributions" section (ROADMAP LD step 7).
//! Driven by xtask's screen check (scenario `linux_menu`, a headless
//! desktop): this program checks the starting state, says
//! `linux_menu_test: ready`, then follows what the clicks do through
//! what they leave where Windows keeps it: the user's choices in their
//! hive (HKCU\Software\gala\StartMenu, which the registry keeps across
//! reboots) and the installed distribution in WSL's registry (Lxss) and
//! its directory. gala's own shell, so there is no Windows run to compare
//! with; on Windows it says so and passes.
//!
//! - Before: the start menu lists its programs, no choices saved, no "Ubuntu" or "Alpine" registered.
//! - Alpine's suggestion dismissed: LinuxDismissed is "Alpine" (a
//!   REG_MULTI_SZ), LinuxHidden 0.
//! - Ubuntu installed from its tile (fetched from the host, imported by
//!   `wsl --import`): registered (State 1, Version 2) with BasePath
//!   %LOCALAPPDATA%\Linux\Ubuntu, whose rootfs has the desktop session;
//!   the downloaded tarball is gone.
//! - The section hidden ("Hide"): LinuxHidden 1, LinuxCollapsed 0 (it
//!   was collapsed and expanded again), Alpine still dismissed.
//!
//! Prints `linux_menu_test: ok` or exits with the number of the failing
//! check.

#![no_std]
#![no_main]

extern crate alloc;

use gala_heap as _;

use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;
type Hkey = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetModuleHandleA(name: *const u8) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn GetFileAttributesW(name: *const u16) -> u32;
    fn Sleep(ms: u32);
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegOpenKeyExW(key: Hkey, sub: *const u16, options: u32, access: u32, out: *mut Hkey) -> u32;
    fn RegQueryValueExW(
        key: Hkey,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        len: *mut u32,
    ) -> u32;
    fn RegEnumKeyExW(
        key: Hkey,
        index: u32,
        name: *mut u16,
        len: *mut u32,
        reserved: *mut u32,
        class: *mut u16,
        class_len: *mut u32,
        time: *mut u64,
    ) -> u32;
    fn RegCloseKey(key: Hkey) -> u32;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaDesktopAction(
        request: *const u8,
        len: u32,
        out: *mut u8,
        out_len: u32,
        returned: *mut u32,
    ) -> u32;
}

// The compiler's own calls (copies, stack probes) bind to these.
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn __chkstk();
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

/// core's unwind tables name it; nothing unwinds (panics end the process).
#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const HKEY_CURRENT_USER: Hkey = 0x8000_0001;
const KEY_READ: u32 = 0x20019;
const LXSS: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Lxss";
const CHOICES: &str = "Software\\gala\\StartMenu";
const INVALID_ATTRIBUTES: u32 = u32::MAX;
/// How long each stage may take (an install under TCG takes minutes).
const WAIT_SECS: u32 = 1800;

fn out(text: &str) {
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

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    char::decode_utf16(units).map(|c| c.unwrap_or('\u{fffd}')).collect()
}

fn fail(check: u32, what: &str) -> ! {
    out("linux_menu_test: check ");
    let mut n = [0u8; 10];
    let mut i = n.len();
    let mut v = check;
    loop {
        i -= 1;
        n[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    out(core::str::from_utf8(&n[i..]).unwrap_or("?"));
    out(" failed: ");
    out(what);
    out("\r\n");
    // SAFETY: ends the process.
    unsafe { ExitProcess(check) }
}

struct Key(Hkey);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: a key this program opened.
        unsafe { RegCloseKey(self.0) };
    }
}

impl Key {
    fn open(parent: Hkey, path: &str) -> Option<Key> {
        let p = wide(path);
        let mut k = 0;
        // SAFETY: valid strings and out pointer.
        (unsafe { RegOpenKeyExW(parent, p.as_ptr(), 0, KEY_READ, &mut k) } == 0).then_some(Key(k))
    }

    fn value(&self, name: &str) -> Option<(u32, Vec<u8>)> {
        let n = wide(name);
        let mut buf = [0u8; 2048];
        let (mut kind, mut len) = (0u32, buf.len() as u32);
        // SAFETY: valid buffers of the stated sizes.
        let r = unsafe {
            RegQueryValueExW(
                self.0,
                n.as_ptr(),
                core::ptr::null_mut(),
                &mut kind,
                buf.as_mut_ptr(),
                &mut len,
            )
        };
        (r == 0).then(|| (kind, buf[..len as usize].to_vec()))
    }

    fn string(&self, name: &str) -> Option<String> {
        let (kind, data) = self.value(name)?;
        (kind == 1).then(|| String::from(utf16le(&data).trim_end_matches('\0')))
    }

    fn dword(&self, name: &str) -> Option<u32> {
        let (kind, data) = self.value(name)?;
        (kind == 4 && data.len() == 4)
            .then(|| u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
    }

    fn subkeys(&self) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0.. {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            let n = || core::ptr::null_mut();
            // SAFETY: valid buffers of the stated sizes.
            if unsafe {
                RegEnumKeyExW(
                    self.0,
                    i,
                    name.as_mut_ptr(),
                    &mut len,
                    n(),
                    core::ptr::null_mut(),
                    n(),
                    core::ptr::null_mut(),
                )
            } != 0
            {
                break;
            }
            out.push(String::from_utf16_lossy(&name[..len as usize]));
        }
        out
    }
}

/// Distribution `name`'s registration.
fn registration(name: &str) -> Option<Key> {
    let lxss = Key::open(HKEY_CURRENT_USER, LXSS)?;
    for guid in lxss.subkeys() {
        if let Some(k) = Key::open(lxss.0, &guid)
            && k.string("DistributionName").as_deref() == Some(name)
        {
            return Some(k);
        }
    }
    None
}

/// The saved choices: (collapsed, hidden, dismissed names joined by ',').
fn choices() -> Option<(u32, u32, String)> {
    let k = Key::open(HKEY_CURRENT_USER, CHOICES)?;
    let (kind, data) = k.value("LinuxDismissed")?;
    if kind != 7 {
        return None;
    }
    let names: Vec<String> =
        utf16le(&data).split('\0').filter(|s| !s.is_empty()).map(String::from).collect();
    Some((k.dword("LinuxCollapsed")?, k.dword("LinuxHidden")?, names.join(",")))
}

fn env(name: &str) -> String {
    let n = wide(name);
    let mut buf = [0u16; 512];
    // SAFETY: our buffers with their lengths.
    let len = unsafe { GetEnvironmentVariableW(n.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())])
}

fn exists(path: &str) -> bool {
    // SAFETY: a NUL-terminated path.
    unsafe { GetFileAttributesW(wide(path).as_ptr()) != INVALID_ATTRIBUTES }
}

/// Polls `done` every half second for at most WAIT_SECS seconds.
fn wait_for(check: u32, what: &str, done: impl Fn() -> bool) {
    for _ in 0..WAIT_SECS * 2 {
        if done() {
            return;
        }
        // SAFETY: plain call.
        unsafe { Sleep(500) };
    }
    fail(check, what)
}

/// Whether this is gala (its ntdll has gala's own exports).
fn on_gala() -> bool {
    // SAFETY: NUL-terminated names.
    unsafe {
        let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
        ntdll.is_null() || !GetProcAddress(ntdll, c"GalaDesktopAction".as_ptr().cast()).is_null()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    if !on_gala() {
        out("linux_menu_test: skipped (gala's start menu)\r\n");
        // SAFETY: ends the process.
        unsafe { ExitProcess(0) }
    }
    // 1, 2: a fresh user: nothing chosen, nothing installed.
    if Key::open(HKEY_CURRENT_USER, CHOICES).is_some() {
        fail(1, "choices saved before the menu was used");
    }
    if registration("Ubuntu").is_some() || registration("Alpine").is_some() {
        fail(2, "a suggested distribution is registered already");
    }
    // 11: the start menu lists its programs (`gala programs`'s action),
    // which also brings the desktop up on the screen before any window.
    let mut request = [0u8; 40];
    request[0] = 1; // version
    request[4] = 11; // ACTION_PROGRAMS
    let mut listing = [0u8; 4096];
    let mut returned = 0;
    // SAFETY: our buffers with their lengths.
    let status = unsafe {
        GalaDesktopAction(request.as_ptr(), 40, listing.as_mut_ptr(), 4096, &mut returned)
    };
    if status != 0 {
        fail(11, "the start menu's programs (GalaDesktopAction)");
    }
    out("linux_menu_test: ready\r\n");
    // 3: Alpine dismissed (after a collapse and an expand).
    wait_for(3, "Alpine's suggestion dismissed", || {
        choices().is_some_and(|(_, _, d)| d == "Alpine")
    });
    match choices() {
        Some((0, 0, _)) => {}
        _ => fail(4, "the section collapsed or hidden after the dismissal"),
    }
    out("linux_menu_test: Alpine dismissed\r\n");
    // 5-8: Ubuntu installed from its tile.
    wait_for(5, "Ubuntu registered", || {
        registration("Ubuntu").is_some_and(|k| k.dword("State") == Some(1))
    });
    let Some(k) = registration("Ubuntu") else { fail(5, "Ubuntu registered") };
    let base = k.string("BasePath").unwrap_or_default();
    let want = alloc::format!("{}\\Linux\\Ubuntu", env("LOCALAPPDATA"));
    if !base.eq_ignore_ascii_case(&want) || k.dword("Version") != Some(2) {
        fail(6, "Ubuntu's BasePath or Version");
    }
    if !exists(&alloc::format!("{base}\\rootfs\\lx\\xdesktop\\session.sh")) {
        fail(7, "Ubuntu's rootfs (its desktop session)");
    }
    // gala linux-install deletes it once wsl.exe has imported it.
    let tarball = alloc::format!("{}\\Linux\\Ubuntu.rootfs.tar.gz", env("LOCALAPPDATA"));
    wait_for(8, "the downloaded tarball removed", || !exists(&tarball));
    out("linux_menu_test: Ubuntu installed\r\n");
    // 9: the section hidden at the end; Alpine still dismissed.
    wait_for(9, "the section hidden", || choices().is_some_and(|(_, h, _)| h == 1));
    match choices() {
        Some((0, 1, d)) if d == "Alpine" => {}
        _ => fail(10, "the choices after hiding"),
    }
    out("linux_menu_test: section hidden\r\n");
    out("linux_menu_test: ok\r\n");
    // SAFETY: ends the process.
    unsafe { ExitProcess(0) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
