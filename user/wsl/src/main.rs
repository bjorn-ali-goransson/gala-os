//! `wsl.exe`: gala's Windows Subsystem for Linux front end (ROADMAP CT
//! step 2, ARCHITECTURE "The WSL facade"). Distributions are registered
//! per user under `HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss`
//! as Windows keeps them (a `{guid}` key each with DistributionName,
//! BasePath, State, Version, Flags, DefaultUid, RunOOBE; the
//! DefaultDistribution value); each one's root file system is its own
//! tree, `<BasePath>\rootfs`, made by the kernel from the imported
//! tarball (GalaLxNode); its programs run on the Linux personality
//! rooted there (GalaLxSpawn), with this program's standard handles as
//! their fds 0-2 (so `wsl -d <distro>` in a console window is a terminal
//! for the distribution).
//!
//! ```text
//! wsl --import <name> <dir> <rootfs.tar[.gz]>
//! wsl -l|--list [-v|--verbose] [-q|--quiet] [--running]
//! wsl [-d|--distribution <name>] [-e|--exec <program> [args] | [--] <command line>]
//! wsl -t|--terminate <name>      wsl --shutdown
//! wsl --unregister <name>        wsl -s|--set-default <name>
//! wsl -v|--version
//! ```
//!
//! Output as WSL's: UTF-16 (UTF-8 with WSL_UTF8=1) with CR LF, the
//! console's own on a console; exit code -1 when wsl.exe itself fails,
//! else the Linux program's exit status (128 + the signal that killed
//! it). Options gala does not have yet print why and go to the ledger
//! (`wsl.exe!<option>`).

#![no_std]
#![no_main]

extern crate alloc;

use gala_heap as _;

use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;

use wsl::{Row, args, tar};

type Bool = i32;
type Handle = *mut c_void;
type Hkey = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetCommandLineW() -> *const u16;
    fn GetLastError() -> u32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn GetFileSizeEx(h: Handle, size: *mut i64) -> Bool;
    fn CreateDirectoryW(name: *const u16, sa: *mut c_void) -> Bool;
    fn RemoveDirectoryW(name: *const u16) -> Bool;
    fn GetFullPathNameW(name: *const u16, len: u32, buf: *mut u16, file: *mut *mut u16) -> u32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn GetConsoleMode(h: Handle, mode: *mut u32) -> Bool;
    fn SetConsoleMode(h: Handle, mode: u32) -> Bool;
    fn WriteConsoleW(
        h: Handle,
        buf: *const u16,
        len: u32,
        written: *mut u32,
        r: *mut c_void,
    ) -> Bool;
    fn SetConsoleCtrlHandler(handler: Option<extern "system" fn(u32) -> Bool>, add: Bool) -> Bool;
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegCreateKeyExW(
        key: Hkey,
        sub: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        access: u32,
        sa: *mut c_void,
        result: *mut Hkey,
        disposition: *mut u32,
    ) -> u32;
    fn RegOpenKeyExW(key: Hkey, sub: *const u16, options: u32, access: u32, out: *mut Hkey) -> u32;
    fn RegSetValueExW(
        key: Hkey,
        name: *const u16,
        reserved: u32,
        kind: u32,
        data: *const u8,
        len: u32,
    ) -> u32;
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
    fn RegDeleteTreeW(key: Hkey, sub: *const u16) -> u32;
    fn RegDeleteValueW(key: Hkey, name: *const u16) -> u32;
    fn RegCloseKey(key: Hkey) -> u32;
    fn SystemFunction036(buffer: *mut u8, length: u32) -> u8;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaLxNode(root: Handle, record: *const u8, length: usize) -> u32;
    fn GalaLxSpawn(record: *const u8, length: usize, process: *mut Handle) -> u32;
    fn GalaLxDistro(op: u32, root: Handle, name: *const u8, length: usize, count: *mut u64) -> u32;
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn strlen(s: *const u8) -> usize;
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

const HKEY_CURRENT_USER: Hkey = 0x8000_0001;
const KEY_ALL_ACCESS: u32 = 0xf003f;
const REG_SZ: u32 = 1;
const REG_DWORD: u32 = 4;
const LXSS: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Lxss";

const GENERIC_READ: u32 = 0x8000_0000;
const FILE_SHARE_ALL: u32 = 7;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
/// FILE_LIST_DIRECTORY | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY |
/// FILE_TRAVERSE | SYNCHRONIZE: what GalaLxNode, GalaLxSpawn and
/// GalaLxDistro need of a tree's root.
const DIR_ACCESS: u32 = 0x1 | 0x2 | 0x4 | 0x20 | 0x0010_0000;
const INFINITE: u32 = u32::MAX;

/// WSL's states (its service's LxssDistributionState).
const STATE_INSTALLED: u32 = 1;
const STATE_INSTALLING: u32 = 3;
/// LXSS_DISTRO_FLAGS_DEFAULT (interop, the NT path, drive mounting) |
/// LXSS_DISTRO_FLAGS_VM_MODE: a WSL 2 distribution.
const FLAGS_WSL2: u32 = 0xf;
/// LXSS_DISTRO_VERSION_CURRENT.
const DISTRO_VERSION: u32 = 2;

/// wsl.exe's own failure.
const EXIT_FAILED: u32 = u32::MAX;

const MSG_SUCCESS: &str = "The operation completed successfully.";
const MSG_NOT_FOUND: &str = "There is no distribution with the supplied name.";
const MSG_NO_DISTROS: &str = "Windows Subsystem for Linux has no installed distributions.
You can resolve this by installing a distribution with the instructions below:

Use 'wsl.exe --list --online' to list available distributions
and 'wsl.exe --install <Distro>' to install.";
const MSG_EXISTS: &str =
    "A distribution with the supplied name already exists. Use --name to choose a different name.";

// ---- Text -------------------------------------------------------------

fn wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

fn wide_of(s: &[u16]) -> Vec<u16> {
    let mut v = s.to_vec();
    v.push(0);
    v
}

fn utf8(s: &[u16]) -> String {
    char::decode_utf16(s.iter().copied()).map(|c| c.unwrap_or('\u{fffd}')).collect()
}

fn std_handle(which: u32) -> Handle {
    // SAFETY: a plain call.
    unsafe { GetStdHandle(which) }
}

fn is_console(h: Handle) -> bool {
    let mut mode = 0;
    // SAFETY: a valid out pointer.
    !h.is_null() && unsafe { GetConsoleMode(h, &mut mode) } != 0
}

fn env(name: &str) -> Option<String> {
    let mut buf = [0u16; 512];
    let n = wide(name);
    // SAFETY: a valid buffer of its stated size.
    let len = unsafe { GetEnvironmentVariableW(n.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    (len > 0 && (len as usize) < buf.len()).then(|| utf8(&buf[..len as usize]))
}

/// Writes text as WSL's CRT does in its _O_U16TEXT mode: on a console
/// the console's characters, else UTF-16 bytes (UTF-8 with WSL_UTF8=1),
/// `\n` as CR LF.
fn print_to(which: u32, text: &str) {
    let h = std_handle(which);
    let mut units: Vec<u16> = Vec::new();
    for c in text.chars() {
        if c == '\n' {
            units.push(u16::from(b'\r'));
        }
        let mut b = [0u16; 2];
        units.extend_from_slice(c.encode_utf16(&mut b));
    }
    let mut written = 0;
    if is_console(h) {
        // SAFETY: a valid buffer and out pointer.
        unsafe {
            WriteConsoleW(
                h,
                units.as_ptr(),
                units.len() as u32,
                &mut written,
                core::ptr::null_mut(),
            )
        };
        return;
    }
    let bytes: Vec<u8> = if env("WSL_UTF8").as_deref() == Some("1") {
        utf8(&units).into_bytes()
    } else {
        units.iter().flat_map(|u| u.to_le_bytes()).collect()
    };
    // SAFETY: a valid buffer and out pointer.
    unsafe {
        WriteFile(h, bytes.as_ptr(), bytes.len() as u32, &mut written, core::ptr::null_mut())
    };
}

/// A message line on stdout (WSL prints its messages there).
fn say(text: &str) {
    print_to(-11i32 as u32, text);
    print_to(-11i32 as u32, "\n");
}

/// A failure as WSL reports one: the message and its error code line.
fn fail(message: &str, code: &str) -> u32 {
    say(message);
    if !code.is_empty() {
        let mut line = String::from("Error code: ");
        line.push_str(code);
        say(&line);
    }
    EXIT_FAILED
}

/// An option gala's wsl.exe does not have yet: said and ledgered.
fn unsupported(option: &str) -> u32 {
    let mut name = String::from("wsl.exe!");
    name.push_str(option);
    // SAFETY: a valid buffer; "unsupported" (2).
    unsafe { GalaRecordMissing(name.as_ptr(), name.len(), 2, 0) };
    let mut line = String::from("wsl: ");
    line.push_str(option);
    line.push_str(" is not supported by gala's wsl.exe yet.");
    print_to(-12i32 as u32, &line);
    print_to(-12i32 as u32, "\n");
    EXIT_FAILED
}

/// A Win32 error as WSL reports one it did not expect: the system's
/// message and `Wsl/<ERROR_NAME>`.
fn system_error(code: u32) -> u32 {
    let (message, name) = match code {
        2 => ("The system cannot find the file specified.", "Wsl/ERROR_FILE_NOT_FOUND"),
        3 => ("The system cannot find the path specified.", "Wsl/ERROR_PATH_NOT_FOUND"),
        5 => ("Access is denied.", "Wsl/E_ACCESSDENIED"),
        _ => ("The operation failed.", "Wsl/E_FAIL"),
    };
    fail(message, name)
}

fn invalid_option(option: &[u16]) -> u32 {
    let mut line = String::from("Invalid command line argument: ");
    line.push_str(&utf8(option));
    line.push_str("\nPlease use 'wsl.exe --help' to get a list of supported arguments.");
    say(&line);
    EXIT_FAILED
}

fn missing_parameter(option: &str) -> u32 {
    let mut line = String::from("Command line argument ");
    line.push_str(option);
    line.push_str(
        " requires a value.\nPlease use 'wsl.exe --help' to get a list of supported arguments.",
    );
    say(&line);
    EXIT_FAILED
}

// ---- The registry -----------------------------------------------------

/// An open registry key, closed on drop.
struct Key(Hkey);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: a key this program opened.
        unsafe { RegCloseKey(self.0) };
    }
}

impl Key {
    /// The Lxss key (made if `create`).
    fn lxss(create: bool) -> Option<Key> {
        let path = wide(LXSS);
        let mut k = 0;
        // SAFETY: valid strings and out pointers.
        let r = unsafe {
            if create {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    path.as_ptr(),
                    0,
                    core::ptr::null(),
                    0,
                    KEY_ALL_ACCESS,
                    core::ptr::null_mut(),
                    &mut k,
                    core::ptr::null_mut(),
                )
            } else {
                RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_ALL_ACCESS, &mut k)
            }
        };
        (r == 0).then_some(Key(k))
    }

    fn sub(&self, name: &str, create: bool) -> Option<Key> {
        let n = wide(name);
        let mut k = 0;
        // SAFETY: valid strings and out pointers.
        let r = unsafe {
            if create {
                RegCreateKeyExW(
                    self.0,
                    n.as_ptr(),
                    0,
                    core::ptr::null(),
                    0,
                    KEY_ALL_ACCESS,
                    core::ptr::null_mut(),
                    &mut k,
                    core::ptr::null_mut(),
                )
            } else {
                RegOpenKeyExW(self.0, n.as_ptr(), 0, KEY_ALL_ACCESS, &mut k)
            }
        };
        (r == 0).then_some(Key(k))
    }

    fn set_dword(&self, name: &str, v: u32) {
        let n = wide(name);
        // SAFETY: a valid name and four bytes of data.
        unsafe { RegSetValueExW(self.0, n.as_ptr(), 0, REG_DWORD, v.to_le_bytes().as_ptr(), 4) };
    }

    fn set_string(&self, name: &str, v: &str) {
        let n = wide(name);
        let data = wide(v);
        // SAFETY: a valid name and data with its NUL.
        unsafe {
            RegSetValueExW(
                self.0,
                n.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr().cast(),
                2 * data.len() as u32,
            )
        };
    }

    fn dword(&self, name: &str) -> Option<u32> {
        let n = wide(name);
        let (mut kind, mut v, mut len) = (0u32, [0u8; 4], 4u32);
        // SAFETY: valid buffers of the stated sizes.
        let r = unsafe {
            RegQueryValueExW(
                self.0,
                n.as_ptr(),
                core::ptr::null_mut(),
                &mut kind,
                v.as_mut_ptr(),
                &mut len,
            )
        };
        (r == 0 && kind == REG_DWORD).then(|| u32::from_le_bytes(v))
    }

    fn string(&self, name: &str) -> Option<String> {
        let n = wide(name);
        let mut buf = [0u16; 1024];
        let (mut kind, mut len) = (0u32, 2 * buf.len() as u32);
        // SAFETY: valid buffers of the stated sizes.
        let r = unsafe {
            RegQueryValueExW(
                self.0,
                n.as_ptr(),
                core::ptr::null_mut(),
                &mut kind,
                buf.as_mut_ptr().cast(),
                &mut len,
            )
        };
        if r != 0 || kind != REG_SZ {
            return None;
        }
        let units = &buf[..(len as usize / 2).min(buf.len())];
        let end = units.iter().position(|&c| c == 0).unwrap_or(units.len());
        Some(utf8(&units[..end]))
    }

    /// The names of its subkeys, sorted as the registry enumerates them.
    fn subkeys(&self) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0.. {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            // SAFETY: valid buffers of the stated sizes.
            let r = unsafe {
                RegEnumKeyExW(
                    self.0,
                    i,
                    name.as_mut_ptr(),
                    &mut len,
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                    core::ptr::null_mut(),
                )
            };
            if r != 0 {
                break;
            }
            out.push(utf8(&name[..len as usize]));
        }
        out.sort_by_key(|n| n.to_ascii_lowercase());
        out
    }

    fn delete_tree(&self, name: &str) {
        let n = wide(name);
        // SAFETY: a valid name.
        unsafe { RegDeleteTreeW(self.0, n.as_ptr()) };
    }

    fn delete_value(&self, name: &str) {
        let n = wide(name);
        // SAFETY: a valid name.
        unsafe { RegDeleteValueW(self.0, n.as_ptr()) };
    }
}

/// A registered distribution.
struct Distro {
    guid: String,
    name: String,
    base: String,
    uid: u32,
    flags: u32,
}

impl Distro {
    fn version(&self) -> u32 {
        if self.flags & 8 != 0 { 2 } else { 1 }
    }

    /// `<BasePath>\rootfs`.
    fn rootfs(&self) -> String {
        let mut p = self.base.clone();
        if !p.ends_with('\\') {
            p.push('\\');
        }
        p.push_str("rootfs");
        p
    }
}

/// The registered distributions (installed ones; `all`: every one) in
/// the registry's order, and the default's GUID.
fn distros(all: bool) -> (Vec<Distro>, Option<String>) {
    let Some(lxss) = Key::lxss(false) else { return (Vec::new(), None) };
    let mut out = Vec::new();
    for guid in lxss.subkeys() {
        let Some(k) = lxss.sub(&guid, false) else { continue };
        let (Some(name), Some(base)) = (k.string("DistributionName"), k.string("BasePath")) else {
            continue;
        };
        if !all && k.dword("State") != Some(STATE_INSTALLED) {
            continue;
        }
        let uid = k.dword("DefaultUid").unwrap_or(0);
        let flags = k.dword("Flags").unwrap_or(7);
        out.push(Distro { guid, name, base, uid, flags });
    }
    let default = lxss.string("DefaultDistribution");
    (out, default)
}

/// The distribution named `name` (any case; `all`: in any state).
fn find(name: &[u16], all: bool) -> Option<Distro> {
    let name = utf8(name);
    distros(all).0.into_iter().find(|d| d.name.eq_ignore_ascii_case(&name))
}

/// The default distribution.
fn default_distro() -> Option<Distro> {
    let (list, default) = distros(false);
    let default = default?;
    list.into_iter().find(|d| d.guid.eq_ignore_ascii_case(&default))
}

// ---- Files ------------------------------------------------------------

/// An open handle, closed on drop.
struct Owned(Handle);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: a handle this program opened.
        unsafe { CloseHandle(self.0) };
    }
}

fn invalid(h: Handle) -> bool {
    h.is_null() || h as isize == -1
}

/// A directory opened for the GalaLx* calls.
fn open_dir(path: &str) -> Option<Owned> {
    let p = wide(path);
    // SAFETY: a valid path.
    let h = unsafe {
        CreateFileW(
            p.as_ptr(),
            DIR_ACCESS,
            FILE_SHARE_ALL,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            core::ptr::null_mut(),
        )
    };
    (!invalid(h)).then_some(Owned(h))
}

/// A whole file's bytes, or the error code.
fn read_file(path: &[u16]) -> Result<Vec<u8>, u32> {
    let p = wide_of(path);
    // SAFETY: a valid path.
    let h = unsafe {
        CreateFileW(
            p.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_ALL,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        )
    };
    if invalid(h) {
        // SAFETY: a plain call.
        return Err(unsafe { GetLastError() });
    }
    let h = Owned(h);
    let mut size = 0i64;
    // SAFETY: a valid out pointer.
    unsafe { GetFileSizeEx(h.0, &mut size) };
    let mut data = Vec::new();
    if data.try_reserve_exact(size.max(0) as usize).is_err() {
        return Err(8); // ERROR_NOT_ENOUGH_MEMORY
    }
    let mut chunk = [0u8; 65536];
    loop {
        let mut got = 0u32;
        // SAFETY: a valid buffer and out pointer.
        let ok = unsafe {
            ReadFile(h.0, chunk.as_mut_ptr(), chunk.len() as u32, &mut got, core::ptr::null_mut())
        };
        if ok == 0 || got == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..got as usize]);
    }
    Ok(data)
}

/// The absolute form of `path`.
fn full_path(path: &[u16]) -> String {
    let p = wide_of(path);
    let mut buf = [0u16; 1024];
    // SAFETY: valid buffers.
    let n = unsafe {
        GetFullPathNameW(p.as_ptr(), buf.len() as u32, buf.as_mut_ptr(), core::ptr::null_mut())
    };
    if n == 0 || n as usize >= buf.len() { utf8(path) } else { utf8(&buf[..n as usize]) }
}

// ---- The kernel's calls -----------------------------------------------

const KIND_DIRECTORY: u32 = 1;
const KIND_FILE: u32 = 2;
const KIND_SYMLINK: u32 = 3;
const KIND_HARD_LINK: u32 = 4;
const KIND_SPECIAL: u32 = 5;
const STATUS_NOT_SUPPORTED: u32 = 0xc000_00bb;
const STATUS_OBJECT_PATH_NOT_FOUND: u32 = 0xc000_003a;

fn put64(r: &mut [u8], at: usize, v: u64) {
    r[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

/// GalaLxNode: one node below `root`.
fn make_node(root: Handle, path: &[u8], kind: u32, mode: u32, mtime: u64, data: &[u8]) -> u32 {
    let mut r = [0u8; 56];
    r[0..4].copy_from_slice(&1u32.to_le_bytes());
    r[4..8].copy_from_slice(&kind.to_le_bytes());
    r[8..12].copy_from_slice(&mode.to_le_bytes());
    put64(&mut r, 16, mtime);
    put64(&mut r, 24, path.as_ptr() as u64);
    put64(&mut r, 32, path.len() as u64);
    put64(&mut r, 40, data.as_ptr() as u64);
    put64(&mut r, 48, data.len() as u64);
    // SAFETY: the record and the buffers it points to live across the call.
    unsafe { GalaLxNode(root, r.as_ptr(), r.len()) }
}

/// GalaLxDistro.
fn distro_call(op: u32, root: Handle, name: &str, count: u64) -> (u32, u64) {
    let mut n = count;
    // SAFETY: a valid name and out pointer.
    let s = unsafe { GalaLxDistro(op, root, name.as_ptr(), name.len(), &mut n) };
    (s, n)
}

/// How many processes the distribution runs (None: its tree is gone).
fn running(d: &Distro) -> Option<u64> {
    let root = open_dir(&d.rootfs())?;
    let (s, n) = distro_call(0, root.0, &d.name, 0);
    (s == 0).then_some(n)
}

// ---- Commands ----------------------------------------------------------

fn import(rest: &[u16]) -> u32 {
    let words = args::split(rest);
    let mut positional: Vec<&[u16]> = Vec::new();
    for w in &words {
        if args::is(w, "--version") || args::is(w, "--vhd") {
            return unsupported(&utf8(w));
        }
        positional.push(w);
    }
    let [name, dir, file] = positional[..] else {
        say("Invalid command line argument: --import");
        return EXIT_FAILED;
    };
    let name = utf8(name);
    if find(name.encode_utf16().collect::<Vec<_>>().as_slice(), true).is_some() {
        return fail(MSG_EXISTS, "Wsl/Service/RegisterDistro/ERROR_ALREADY_EXISTS");
    }
    let base = full_path(dir);
    let base_w = wide(&base);
    // The install location: made (one level, as WSL's CreateDirectoryW),
    // or there already.
    // SAFETY: a valid path.
    let created = unsafe { CreateDirectoryW(base_w.as_ptr(), core::ptr::null_mut()) } != 0;
    // SAFETY: a plain call.
    let error = if created { 0 } else { unsafe { GetLastError() } };
    if !created && error != 183 {
        return system_error(error);
    }
    let cleanup_dir = || {
        if created {
            // SAFETY: a valid path.
            unsafe { RemoveDirectoryW(base_w.as_ptr()) };
        }
    };
    let archive = match read_file(file) {
        Ok(d) => d,
        Err(e) => {
            cleanup_dir();
            return system_error(e);
        }
    };
    let archive = if wsl::is_gzip(&archive) {
        match wsl::gunzip(&archive) {
            Some(a) => a,
            None => {
                cleanup_dir();
                return fail(
                    "The archive is not a valid gzip file.",
                    "Wsl/Service/RegisterDistro/E_INVALIDARG",
                );
            }
        }
    } else {
        archive
    };
    // The registration, Installing until the tree is made.
    let Some(lxss) = Key::lxss(true) else {
        cleanup_dir();
        return fail(
            "The registry key could not be created.",
            "Wsl/Service/RegisterDistro/E_ACCESSDENIED",
        );
    };
    let mut random = [0u8; 16];
    // SAFETY: a valid buffer.
    unsafe { SystemFunction036(random.as_mut_ptr(), 16) };
    let guid = wsl::guid_string(random);
    let Some(k) = lxss.sub(&guid, true) else {
        cleanup_dir();
        return fail(
            "The registry key could not be created.",
            "Wsl/Service/RegisterDistro/E_ACCESSDENIED",
        );
    };
    k.set_dword("State", STATE_INSTALLING);
    k.set_string("DistributionName", &name);
    k.set_dword("Version", DISTRO_VERSION);
    k.set_string("BasePath", &base);
    k.set_dword("Flags", FLAGS_WSL2);
    k.set_dword("DefaultUid", 0);
    k.set_dword("RunOOBE", 0);
    drop(k);
    let undo = |why: &str| {
        lxss.delete_tree(&guid);
        cleanup_dir();
        fail(why, "Wsl/Service/RegisterDistro/WSL_E_IMPORT_FAILED")
    };
    // The tree: `rootfs` below the base directory, then each entry.
    let Some(base_h) = open_dir(&base) else {
        return undo("The install location cannot be opened.");
    };
    let s = make_node(base_h.0, b"rootfs", KIND_DIRECTORY, 0o755, 0, &[]);
    if s != 0 {
        return undo("The distribution's root directory cannot be made.");
    }
    let mut rootfs = base.clone();
    rootfs.push_str("\\rootfs");
    let Some(root) = open_dir(&rootfs) else {
        return undo("The distribution's root cannot be opened.");
    };
    let (mut made, mut skipped) = (0u64, 0u64);
    for entry in tar::Reader::new(&archive) {
        let e = match entry {
            Ok(e) => e,
            Err(_) => {
                let _ = distro_call(2, root.0, &name, 0);
                return undo("The archive is not a valid tar file.");
            }
        };
        let path = tar::clean(&e.path).unwrap_or(b"/");
        let (kind, data): (u32, &[u8]) = match e.kind {
            tar::Kind::Directory => (KIND_DIRECTORY, &[]),
            tar::Kind::File => (KIND_FILE, e.data),
            tar::Kind::Symlink => (KIND_SYMLINK, &e.link),
            tar::Kind::HardLink => (KIND_HARD_LINK, tar::clean(&e.link).unwrap_or(b"/")),
            tar::Kind::Special => (KIND_SPECIAL, &[]),
        };
        let s = make_node(root.0, path, kind, e.mode, tar::nt_time(e.mtime), data);
        if s == STATUS_NOT_SUPPORTED {
            skipped += 1;
            continue;
        }
        if s != 0 {
            let _ = distro_call(2, root.0, &name, 0);
            return undo("The distribution's files cannot be made.");
        }
        made += 1;
    }
    let _ = distro_call(4, root.0, &name, made);
    if skipped > 0 {
        let mut n = [0u8; 10];
        let mut line = String::from("wsl: ");
        line.push_str(wsl::decimal(skipped.min(u64::from(u32::MAX)) as u32, &mut n));
        line.push_str(" device or FIFO entries were not made (gala has no device nodes yet).\n");
        print_to(-12i32 as u32, &line);
    }
    if let Some(k) = lxss.sub(&guid, false) {
        k.set_dword("State", STATE_INSTALLED);
    }
    if lxss.string("DefaultDistribution").is_none() {
        lxss.set_string("DefaultDistribution", &guid);
    }
    say(MSG_SUCCESS);
    0
}

fn list(rest: &[u16]) -> u32 {
    let (mut verbose, mut quiet, mut only_running, mut all) = (false, false, false, false);
    for w in args::split(rest) {
        if args::is(&w, "-v") || args::is(&w, "--verbose") {
            verbose = true;
        } else if args::is(&w, "-q") || args::is(&w, "--quiet") {
            quiet = true;
        } else if args::is(&w, "--running") {
            only_running = true;
        } else if args::is(&w, "--all") {
            all = true;
        } else if args::is(&w, "-o") || args::is(&w, "--online") {
            return unsupported("--online");
        } else {
            return invalid_option(&w);
        }
    }
    let (mut list, default) = distros(all);
    // The default first, the rest as enumerated.
    if let Some(i) =
        default.as_ref().and_then(|g| list.iter().position(|d| d.guid.eq_ignore_ascii_case(g)))
    {
        let d = list.remove(i);
        list.insert(0, d);
    }
    let states: Vec<u64> = list.iter().map(|d| running(d).unwrap_or(0)).collect();
    if only_running {
        let mut i = 0;
        list.retain(|_| {
            i += 1;
            states[i - 1] > 0
        });
        if list.is_empty() && !quiet {
            say("There are no running distributions.");
            return EXIT_FAILED;
        }
    }
    let is_default = |d: &Distro| default.as_ref().is_some_and(|g| d.guid.eq_ignore_ascii_case(g));
    if verbose {
        if list.is_empty() {
            return fail(MSG_NO_DISTROS, "");
        }
        let rows: Vec<Row> = list
            .iter()
            .map(|d| Row {
                name: &d.name,
                state: if running(d).unwrap_or(0) > 0 { "Running" } else { "Stopped" },
                version: d.version(),
                default: is_default(d),
            })
            .collect();
        print_to(-11i32 as u32, &wsl::list_verbose(&rows));
        return 0;
    }
    if !quiet {
        if list.is_empty() {
            return fail(MSG_NO_DISTROS, "");
        }
        say("Windows Subsystem for Linux Distributions:");
    }
    for d in &list {
        let mut line = d.name.clone();
        if !quiet && is_default(d) {
            line.push_str(" (Default)");
        }
        say(&line);
    }
    0
}

fn terminate(name: &[u16]) -> u32 {
    let Some(d) = find(name, false) else {
        return fail(MSG_NOT_FOUND, "Wsl/Service/WSL_E_DISTRO_NOT_FOUND");
    };
    if let Some(root) = open_dir(&d.rootfs()) {
        let _ = distro_call(1, root.0, &d.name, 0);
    }
    say(MSG_SUCCESS);
    0
}

fn unregister(name: &[u16]) -> u32 {
    let Some(d) = find(name, true) else {
        return fail(MSG_NOT_FOUND, "Wsl/Service/WSL_E_DISTRO_NOT_FOUND");
    };
    let Some(lxss) = Key::lxss(false) else { return fail(MSG_NOT_FOUND, "") };
    if let Some(k) = lxss.sub(&d.guid, false) {
        k.set_dword("State", 4); // Uninstalling
    }
    if lxss.string("DefaultDistribution").is_some_and(|g| g.eq_ignore_ascii_case(&d.guid)) {
        lxss.delete_value("DefaultDistribution");
        let next = distros(false).0.into_iter().find(|x| x.guid != d.guid);
        if let Some(next) = next {
            lxss.set_string("DefaultDistribution", &next.guid);
        }
    }
    if let Some(root) = open_dir(&d.rootfs()) {
        let _ = distro_call(1, root.0, &d.name, 0);
        // The terminate waited for them to be gone.
        let s = distro_call(2, root.0, &d.name, 0).0;
        if s != 0 {
            print_to(-12i32 as u32, "wsl: the distribution's files could not all be removed.\n");
        }
    }
    lxss.delete_tree(&d.guid);
    // The install location goes if nothing else is in it (as WSL's).
    let base = wide(&d.base);
    // SAFETY: a valid path.
    unsafe { RemoveDirectoryW(base.as_ptr()) };
    say(MSG_SUCCESS);
    0
}

fn set_default(name: &[u16]) -> u32 {
    let Some(d) = find(name, false) else {
        return fail(MSG_NOT_FOUND, "Wsl/Service/WSL_E_DISTRO_NOT_FOUND");
    };
    if let Some(lxss) = Key::lxss(false) {
        lxss.set_string("DefaultDistribution", &d.guid);
    }
    say(MSG_SUCCESS);
    0
}

fn version() -> u32 {
    // The build from KUSER_SHARED_DATA's NtBuildNumber (+0x260).
    // SAFETY: the shared page is mapped in every process.
    let build = unsafe { core::ptr::read_volatile(0x7ffe_0260 as *const u32) };
    let mut n = [0u8; 10];
    let mut text = String::from(
        "WSL version: 2.0.0.0\nKernel version: 6.18.0-gala\nWSLg version: 0.0.0\n\
         MSRDC version: 0.0.0\nDirect3D version: 0.0.0\nDXCore version: 0.0.0\n\
         Windows version: 10.0.",
    );
    text.push_str(wsl::decimal(build, &mut n));
    text.push_str(".0");
    say(&text);
    0
}

/// Ctrl+C in the console belongs to the Linux side: wsl.exe stays.
extern "system" fn ignore_ctrl(_: u32) -> Bool {
    1
}

/// How the Linux program is given.
enum Command {
    /// The user's login shell.
    Login,
    /// `$SHELL -c <line>`.
    Shell(Vec<u16>),
    /// The program and its arguments.
    Exec(Vec<Vec<u16>>),
}

fn launch(distro: Option<Distro>, command: Command) -> u32 {
    let Some(d) = distro.or_else(default_distro) else {
        return fail(MSG_NO_DISTROS, "");
    };
    let rootfs = d.rootfs();
    let Some(root) = open_dir(&rootfs) else {
        return fail(
            "The distribution's files are missing.",
            "Wsl/Service/CreateInstance/ERROR_PATH_NOT_FOUND",
        );
    };
    // The default user's name, home and shell from the distribution's own
    // /etc/passwd.
    let mut passwd_path = rootfs.clone();
    passwd_path.push_str("\\etc\\passwd");
    let passwd = read_file(&passwd_path.encode_utf16().collect::<Vec<_>>()).unwrap_or_default();
    let (user, home, shell) = match wsl::passwd_entry(&passwd, d.uid) {
        Some((u, h, s)) => (u.to_vec(), h.to_vec(), s.to_vec()),
        None => (b"root".to_vec(), b"/".to_vec(), b"/bin/sh".to_vec()),
    };
    let shell = if shell.is_empty() { b"/bin/sh".to_vec() } else { shell };
    let base = shell.rsplit(|&c| c == b'/').next().unwrap_or(b"sh").to_vec();
    let (path, argv): (Vec<u8>, Vec<Vec<u8>>) = match command {
        Command::Login => {
            let mut a0 = b"-".to_vec();
            a0.extend_from_slice(&base);
            (shell.clone(), alloc::vec![a0])
        }
        Command::Shell(line) => {
            (shell.clone(), alloc::vec![shell.clone(), b"-c".to_vec(), utf8(&line).into_bytes()])
        }
        Command::Exec(words) => {
            let argv: Vec<Vec<u8>> = words.iter().map(|w| utf8(w).into_bytes()).collect();
            (argv[0].clone(), argv)
        }
    };
    let mut envp: Vec<u8> = Vec::new();
    let mut put = |k: &str, v: &[u8]| {
        envp.extend_from_slice(k.as_bytes());
        envp.push(b'=');
        envp.extend_from_slice(v);
        envp.push(0);
    };
    put("HOSTTYPE", b"x86_64");
    put("LANG", b"en_US.UTF-8");
    put(
        "PATH",
        b"/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/games:/usr/local/games",
    );
    put("TERM", b"xterm-256color");
    put("HOME", &home);
    put("USER", &user);
    put("LOGNAME", &user);
    put("SHELL", &shell);
    put("WSL_DISTRO_NAME", d.name.as_bytes());
    let mut argv_block: Vec<u8> = Vec::new();
    for a in &argv {
        argv_block.extend_from_slice(a);
        argv_block.push(0);
    }
    // WSL starts in the Windows directory as /mnt/<drive>/...; gala has no
    // /mnt yet (ROADMAP CT step 3): the user's home.
    // SAFETY: a valid buffer.
    unsafe { GalaRecordMissing(b"wsl.exe!/mnt cwd".as_ptr(), 16, 2, 0) };
    let cwd = home.clone();
    // The standard handles; a missing one is NUL.
    let nul = wide("NUL");
    let mut opened: Vec<Owned> = Vec::new();
    let mut stdio = [core::ptr::null_mut(); 3];
    for (i, which) in [-10i32, -11, -12].into_iter().enumerate() {
        let mut h = std_handle(which as u32);
        if invalid(h) {
            // SAFETY: a valid path.
            h = unsafe {
                CreateFileW(
                    nul.as_ptr(),
                    0xc000_0000,
                    FILE_SHARE_ALL,
                    core::ptr::null_mut(),
                    OPEN_EXISTING,
                    0,
                    core::ptr::null_mut(),
                )
            };
            opened.push(Owned(h));
        }
        stdio[i] = h;
    }
    let console_in = is_console(stdio[0]);
    let mut saved_mode = 0;
    if console_in {
        // SAFETY: valid out pointers; a handler that lives as long.
        unsafe {
            GetConsoleMode(stdio[0], &mut saved_mode);
            SetConsoleCtrlHandler(Some(ignore_ctrl), 1);
        }
    }
    let name = d.name.as_bytes();
    let mut r = [0u8; 120];
    r[0..4].copy_from_slice(&1u32.to_le_bytes());
    put64(&mut r, 8, root.0 as u64);
    for (i, h) in stdio.iter().enumerate() {
        put64(&mut r, 16 + 8 * i, *h as u64);
    }
    for (i, (p, n)) in [
        (path.as_ptr(), path.len()),
        (argv_block.as_ptr(), argv_block.len()),
        (envp.as_ptr(), envp.len()),
        (cwd.as_ptr(), cwd.len()),
        (name.as_ptr(), name.len()),
    ]
    .into_iter()
    .enumerate()
    {
        put64(&mut r, 40 + 16 * i, p as u64);
        put64(&mut r, 48 + 16 * i, n as u64);
    }
    let mut process: Handle = core::ptr::null_mut();
    // SAFETY: the record and every buffer it points to live across the call.
    let s = unsafe { GalaLxSpawn(r.as_ptr(), r.len(), &mut process) };
    if s != 0 {
        let mut line = String::from("execvpe(");
        line.push_str(&String::from_utf8_lossy(&path));
        line.push_str(") failed: ");
        line.push_str(if s == STATUS_OBJECT_PATH_NOT_FOUND {
            "No such file or directory"
        } else {
            "Exec format error"
        });
        line.push('\n');
        print_to(-12i32 as u32, &line);
        return 1;
    }
    let process = Owned(process);
    let mut code = 0u32;
    // SAFETY: a process handle this program holds; a valid out pointer.
    unsafe {
        WaitForSingleObject(process.0, INFINITE);
        GetExitCodeProcess(process.0, &mut code);
    }
    if console_in {
        // SAFETY: the mode this program found.
        unsafe { SetConsoleMode(stdio[0], saved_mode) };
    }
    drop(opened);
    // An exit status, or 128 + the signal that ended it (the kernel's
    // status for a killed Linux process: 0xe4c580<signal>).
    if code <= 0xff {
        code
    } else if code & !0xff == 0xe4c5_8000 {
        128 + (code & 0xff)
    } else {
        128 + 9
    }
}

fn run() -> u32 {
    // SAFETY: the process's NUL-terminated command line, kept for its life.
    let line: &[u16] = unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    };
    let mut rest = args::skip_program(line);
    let mut distro: Option<Distro> = None;
    loop {
        let (w, after) = args::next(rest);
        if w.is_empty() {
            break;
        }
        let one = |opt: &str| -> Result<(&[u16], &[u16]), u32> {
            let (v, after) = args::next(after);
            if v.is_empty() { Err(missing_parameter(opt)) } else { Ok((v, after)) }
        };
        if args::is(w, "-d") || args::is(w, "--distribution") {
            if distro.is_some() {
                say("The distribution name has already been set.");
                return EXIT_FAILED;
            }
            let (v, after) = match one("--distribution") {
                Ok(x) => x,
                Err(e) => return e,
            };
            match find(v, false) {
                Some(d) => distro = Some(d),
                None => return fail(MSG_NOT_FOUND, "Wsl/Service/WSL_E_DISTRO_NOT_FOUND"),
            }
            rest = after;
            continue;
        }
        if args::is(w, "--") {
            rest = after;
            break;
        }
        if args::is(w, "-e") || args::is(w, "--exec") {
            let words = args::split(after);
            if words.is_empty() {
                return missing_parameter("--exec");
            }
            return launch(distro, Command::Exec(words));
        }
        if args::is(w, "--import") {
            return import(after);
        }
        if args::is(w, "-l") || args::is(w, "--list") {
            return list(after);
        }
        if args::is(w, "-t") || args::is(w, "--terminate") {
            return match one("--terminate") {
                Ok((v, _)) => terminate(v),
                Err(e) => e,
            };
        }
        if args::is(w, "--unregister") {
            return match one("--unregister") {
                Ok((v, _)) => unregister(v),
                Err(e) => e,
            };
        }
        if args::is(w, "-s") || args::is(w, "--set-default") || args::is(w, "--setdefault") {
            return match one("--set-default") {
                Ok((v, _)) => set_default(v),
                Err(e) => e,
            };
        }
        if args::is(w, "--shutdown") {
            let (s, _) = distro_call(3, core::ptr::null_mut(), "", 0);
            return if s == 0 { 0 } else { EXIT_FAILED };
        }
        if args::is(w, "-v") || args::is(w, "--version") {
            return version();
        }
        for known in [
            "--install",
            "--update",
            "--status",
            "--export",
            "--import-in-place",
            "--mount",
            "--unmount",
            "--manage",
            "--set-version",
            "--set-default-version",
            "--uninstall",
            "--debug-shell",
            "--system",
            "--cd",
            "-u",
            "--user",
            "--shell-type",
            "--help",
        ] {
            if args::is(w, known) {
                return unsupported(known);
            }
        }
        if w.first() == Some(&u16::from(b'-')) {
            return invalid_option(w);
        }
        break;
    }
    if rest.is_empty() {
        launch(distro, Command::Login)
    } else {
        launch(distro, Command::Shell(rest.to_vec()))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = run();
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
