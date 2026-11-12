//! `wsl.exe` as Windows' WSL answers it (ROADMAP CT step 2), driven from
//! outside as Docker Desktop and Podman drive it: each command line run
//! with its standard output and error on pipes, its exit code and output
//! checked. Spec: Microsoft's WSL documentation and the messages and
//! formats of its published source (THIRD_PARTY.md, CLEAN_ROOM "WSL").
//!
//! The distributions are imported from `bb-rootfs.tar.gz` and
//! `bb-rootfs.tar` beside this program (xtask makes them: busybox.net's
//! static busybox as /bin/busybox, its applets as symbolic links, an
//! /etc/passwd with root's shell /bin/sh, /root and /tmp), as `bb` and
//! `bb2` under `wsl_test.d\` here:
//!
//! - `--version`: exit 0, "WSL version: " first;
//! - `--import <name> <dir> <tar[.gz]>`: exit 0, "The operation
//!   completed successfully."; the registration under HKCU\...\Lxss
//!   (a `{guid}` key: DistributionName, BasePath, State 1, Version 2,
//!   DefaultUid 0; the first one becomes DefaultDistribution); a second
//!   import of a name: exit -1 and "A distribution with the supplied name
//!   already exists...";
//! - `-d bb -- echo hi`: "hi\n" (the Linux program's own bytes), exit 0;
//!   `--exec` with a PATH lookup; the Linux exit status as the exit code;
//!   WSL_DISTRO_NAME set;
//! - one file system per distribution: a file written in one is absent
//!   in the other;
//! - `-l -v`: UTF-16 with CR LF, WSL's table (`"%s %-<w>s%-16s%s\n"`),
//!   `*` on the default; `--set-default` moves it;
//! - `--terminate` ends a running `sleep` (its wsl.exe returns), state
//!   Stopped; `--shutdown`: exit 0, no output;
//! - an unknown option and an unknown distribution: exit -1 with WSL's
//!   messages;
//! - `--unregister`: exit 0, the registration and the install directory
//!   gone, the default moved to the one left.
//!
//! Uncertain (printed as `info` lines for the record on Windows): the
//! version lines' values, the "Error code:" lines, BasePath's exact form,
//! Flags, the state right after a command ends, the exit code of a
//! `wsl.exe` whose distribution was terminated, `ls /` (WSL 2 adds its
//! mount points). A machine that already has a `bb` or `bb2` is left
//! alone (SKIP).
//!
//! Prints `wsl_test: ok`, or exits with the number of the failing check.
//!
//! `wsl_test import` only imports `bb` (scenario `wsl_terminal`, which
//! then opens its terminal) and prints `wsl_test: imported bb`.

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

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: Bool,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetModuleFileNameW(module: Handle, name: *mut u16, size: u32) -> u32;
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn GetCommandLineW() -> *const u16;
    fn CreateDirectoryW(name: *const u16, sa: *mut c_void) -> Bool;
    fn RemoveDirectoryW(name: *const u16) -> Bool;
    fn GetFileAttributesW(name: *const u16) -> u32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn CreatePipe(
        read: *mut Handle,
        write: *mut Handle,
        sa: *const SecurityAttributes,
        size: u32,
    ) -> Bool;
    fn SetHandleInformation(h: Handle, mask: u32, flags: u32) -> Bool;
    fn Sleep(ms: u32);
    fn CreateThread(
        sa: *mut c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    #[allow(clippy::too_many_arguments)]
    fn CreateProcessW(
        application: *const u16,
        command_line: *mut u16,
        process_attributes: *mut c_void,
        thread_attributes: *mut c_void,
        inherit: Bool,
        flags: u32,
        environment: *mut c_void,
        directory: *const u16,
        startup: *const u8,
        info: *mut [usize; 3],
    ) -> Bool;
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

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const NAME: &str = "wsl_test";
const HKEY_CURRENT_USER: Hkey = 0x8000_0001;
const KEY_READ: u32 = 0x20019;
const LXSS: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Lxss";
const STARTF_USESTDHANDLES: u32 = 0x100;
const HANDLE_FLAG_INHERIT: u32 = 1;
const INVALID_ATTRIBUTES: u32 = u32::MAX;
const WSL_FAILED: u32 = u32::MAX;

const SUCCESS: &str = "The operation completed successfully.";
const NOT_FOUND: &str = "There is no distribution with the supplied name.";
const EXISTS: &str =
    "A distribution with the supplied name already exists. Use --name to choose a different name.";
const NO_DISTROS: &str = "Windows Subsystem for Linux has no installed distributions.";

// ---- Output --------------------------------------------------------------

fn print(text: &str) {
    let mut written = 0;
    // SAFETY: a valid buffer and out pointer.
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

fn line(parts: &[&str]) {
    let mut s = String::from(NAME);
    s.push_str(": ");
    for p in parts {
        s.push_str(p);
    }
    s.push_str("\r\n");
    print(&s);
}

/// An `info` line: what this machine answers, for the record.
fn info(what: &str, value: &str) {
    let mut v = String::new();
    for c in value.chars() {
        match c {
            '\r' => v.push_str("\\r"),
            '\n' => v.push_str("\\n"),
            c => v.push(c),
        }
    }
    line(&["info ", what, ": ", &v]);
}

fn hex(v: u32) -> String {
    let mut s = String::from("0x");
    for i in (0..8).rev() {
        let d = ((v >> (4 * i)) & 0xf) as u8;
        s.push(char::from(if d < 10 { b'0' + d } else { b'a' + d - 10 }));
    }
    s
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, what: &str) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        let mut n = String::new();
        let mut v = self.0;
        let mut digits = [0u8; 10];
        let mut i = digits.len();
        loop {
            i -= 1;
            digits[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        n.push_str(core::str::from_utf8(&digits[i..]).unwrap_or("?"));
        let mut l = [0u8; 10];
        let mut v = core::panic::Location::caller().line();
        let mut j = l.len();
        loop {
            j -= 1;
            l[j] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        line(&[
            "check ",
            &n,
            " failed (line ",
            core::str::from_utf8(&l[j..]).unwrap_or("?"),
            "): ",
            what,
        ]);
        Err(self.0)
    }
}

// ---- Text ----------------------------------------------------------------

fn wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

fn utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    char::decode_utf16(units).map(|c| c.unwrap_or('\u{fffd}')).collect()
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn env(name: &str) -> Option<String> {
    let mut buf = [0u16; 512];
    let n = wide(name);
    // SAFETY: a valid buffer of its stated size.
    let len = unsafe { GetEnvironmentVariableW(n.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    (len > 0 && (len as usize) < buf.len()).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

fn exists(path: &str) -> bool {
    let p = wide(path);
    // SAFETY: a valid path.
    unsafe { GetFileAttributesW(p.as_ptr()) != INVALID_ATTRIBUTES }
}

/// Makes directory `path` (there already is fine).
fn mkdir(path: &str) {
    let p = wide(path);
    // SAFETY: a valid path.
    unsafe { CreateDirectoryW(p.as_ptr(), core::ptr::null_mut()) };
}

/// This program's directory, with its trailing backslash.
fn here() -> String {
    let mut b = [0u16; 260];
    // SAFETY: our buffer.
    let n = unsafe { GetModuleFileNameW(core::ptr::null_mut(), b.as_mut_ptr(), 260) } as usize;
    let path = String::from_utf16_lossy(&b[..n]);
    match path.rfind('\\') {
        Some(i) => String::from(&path[..=i]),
        None => String::new(),
    }
}

// ---- Running wsl.exe -------------------------------------------------------

/// What a run gave.
struct Ran {
    code: u32,
    out: Vec<u8>,
    err: Vec<u8>,
}

/// A pipe's read end drained on its own thread (so a full pipe never
/// holds the child).
struct Drain {
    handle: Handle,
    data: Vec<u8>,
}

extern "system" fn drain(p: *mut c_void) -> u32 {
    // SAFETY: a Drain that outlives this thread (joined before it is read).
    let d = unsafe { &mut *p.cast::<Drain>() };
    let mut buf = [0u8; 4096];
    loop {
        let mut got = 0;
        // SAFETY: a valid buffer and out pointer.
        let ok = unsafe {
            ReadFile(d.handle, buf.as_mut_ptr(), buf.len() as u32, &mut got, core::ptr::null_mut())
        };
        if ok == 0 || got == 0 {
            return 0;
        }
        d.data.extend_from_slice(&buf[..got as usize]);
    }
}

/// A pipe whose write end the child inherits; (read, write).
fn pipe() -> (Handle, Handle) {
    let sa = SecurityAttributes {
        length: core::mem::size_of::<SecurityAttributes>() as u32,
        descriptor: core::ptr::null_mut(),
        inherit: 1,
    };
    let (mut r, mut w) = (core::ptr::null_mut(), core::ptr::null_mut());
    // SAFETY: valid out pointers; the read end is not inherited.
    unsafe {
        CreatePipe(&mut r, &mut w, &sa, 65536);
        SetHandleInformation(r, HANDLE_FLAG_INHERIT, 0);
    }
    (r, w)
}

struct Wsl {
    exe: String,
}

impl Wsl {
    /// Starts `wsl.exe <args>` with its output on pipes; (process,
    /// drains, their threads).
    fn start(&self, args: &str) -> Option<(Handle, [*mut Drain; 2], [Handle; 2])> {
        let mut cmd = String::from("\"");
        cmd.push_str(&self.exe);
        cmd.push_str("\" ");
        cmd.push_str(args);
        let mut cmd = wide(&cmd);
        let app = wide(&self.exe);
        let (out_r, out_w) = pipe();
        let (err_r, err_w) = pipe();
        let mut startup = [0u8; 104];
        startup[0] = 104;
        startup[60..64].copy_from_slice(&STARTF_USESTDHANDLES.to_le_bytes());
        // SAFETY: plain calls; STARTUPINFOW's handles at 80, 88, 96.
        unsafe {
            startup[80..88].copy_from_slice(&(GetStdHandle(-10i32 as u32) as u64).to_le_bytes());
        }
        startup[88..96].copy_from_slice(&(out_w as u64).to_le_bytes());
        startup[96..104].copy_from_slice(&(err_w as u64).to_le_bytes());
        let mut info = [0usize; 3];
        let null = core::ptr::null_mut();
        // SAFETY: valid strings, STARTUPINFOW and PROCESS_INFORMATION.
        let ok = unsafe {
            CreateProcessW(
                app.as_ptr(),
                cmd.as_mut_ptr(),
                null,
                null,
                1,
                0,
                null,
                core::ptr::null(),
                startup.as_ptr(),
                &mut info,
            )
        };
        // SAFETY: our write ends (the child has its own).
        unsafe {
            CloseHandle(out_w);
            CloseHandle(err_w);
        }
        if ok == 0 {
            return None;
        }
        // SAFETY: the thread handle is not needed.
        unsafe { CloseHandle(info[1] as Handle) };
        let mut drains = [core::ptr::null_mut(); 2];
        let mut threads = [core::ptr::null_mut(); 2];
        for (i, h) in [out_r, err_r].into_iter().enumerate() {
            let d = alloc::boxed::Box::into_raw(alloc::boxed::Box::new(Drain {
                handle: h,
                data: Vec::new(),
            }));
            drains[i] = d;
            // SAFETY: the Drain lives until `finish` joins the thread.
            threads[i] =
                unsafe { CreateThread(null, 0, drain, d.cast(), 0, core::ptr::null_mut()) };
        }
        Some((info[0] as Handle, drains, threads))
    }

    /// Waits for a started run (`ms` at most) and collects it.
    fn finish(started: (Handle, [*mut Drain; 2], [Handle; 2]), ms: u32) -> Ran {
        let (process, drains, threads) = started;
        let mut code = 0;
        // SAFETY: handles this program holds; the drains are joined
        // before they are taken back.
        unsafe {
            if WaitForSingleObject(process, ms) != 0 {
                code = 0xdead_0001;
            } else {
                GetExitCodeProcess(process, &mut code);
            }
            CloseHandle(process);
            let mut out = [Vec::new(), Vec::new()];
            for i in 0..2 {
                WaitForSingleObject(threads[i], 10_000);
                CloseHandle(threads[i]);
                let d = alloc::boxed::Box::from_raw(drains[i]);
                CloseHandle(d.handle);
                out[i] = d.data;
            }
            let [o, e] = out;
            Ran { code, out: o, err: e }
        }
    }

    fn run(&self, args: &str) -> Ran {
        match self.start(args) {
            Some(s) => Self::finish(s, 60_000),
            None => Ran { code: 0xdead_0000, out: Vec::new(), err: Vec::new() },
        }
    }
}

// ---- The registry ----------------------------------------------------------

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
        if kind != 1 {
            return None;
        }
        let s = utf16le(&data);
        Some(String::from(s.trim_end_matches('\0')))
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

/// The registration of distribution `name`: (its key's name, the key).
fn registration(name: &str) -> Option<(String, Key)> {
    let lxss = Key::open(HKEY_CURRENT_USER, LXSS)?;
    for guid in lxss.subkeys() {
        if let Some(k) = Key::open(lxss.0, &guid)
            && k.string("DistributionName").as_deref() == Some(name)
        {
            return Some((guid, k));
        }
    }
    None
}

fn default_guid() -> Option<String> {
    Key::open(HKEY_CURRENT_USER, LXSS)?.string("DefaultDistribution")
}

// ---- `-l -v` -----------------------------------------------------------------

/// The rows of a `-l -v` table: (default, name, state, version).
fn rows(table: &str) -> Vec<(bool, String, String, String)> {
    table
        .split("\r\n")
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let default = l.starts_with('*');
            let mut f = l[1..].split_whitespace();
            let name = String::from(f.next().unwrap_or(""));
            let state = String::from(f.next().unwrap_or(""));
            let version = String::from(f.next().unwrap_or(""));
            (default, name, state, version)
        })
        .collect()
}

/// The table WSL would print for these rows (`"%s %-<w>s%-16s%s\n"`).
fn table(rows: &[(bool, String, String, String)]) -> String {
    let width = rows.iter().map(|r| r.1.chars().count()).max().unwrap_or(0).max(4) + 4;
    let mut out = String::new();
    let mut put = |mark: &str, a: &str, b: &str, c: &str| {
        out.push_str(mark);
        out.push(' ');
        out.push_str(a);
        for _ in a.chars().count()..width {
            out.push(' ');
        }
        out.push_str(b);
        for _ in b.chars().count()..16 {
            out.push(' ');
        }
        out.push_str(c);
        out.push_str("\r\n");
    };
    put(" ", "NAME", "STATE", "VERSION");
    for r in rows {
        put(if r.0 { "*" } else { " " }, &r.1, &r.2, &r.3);
    }
    out
}

fn row<'a>(
    rows: &'a [(bool, String, String, String)],
    name: &str,
) -> Option<&'a (bool, String, String, String)> {
    rows.iter().find(|r| r.1 == name)
}

// ---- The test ------------------------------------------------------------------

fn test() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok_line = [SUCCESS, "\r\n"].concat();
    let root = env("SystemRoot").unwrap_or(String::from("C:\\Windows"));
    let mut exe = root.clone();
    exe.push_str("\\System32\\wsl.exe");
    if !exists(&exe) {
        line(&["SKIP no ", &exe]);
        return Ok(());
    }
    let dir = here();
    let mut gz = dir.clone();
    gz.push_str("bb-rootfs.tar.gz");
    let mut plain = dir.clone();
    plain.push_str("bb-rootfs.tar");
    if !exists(&gz) || !exists(&plain) {
        line(&["SKIP no bb-rootfs.tar(.gz) beside the program"]);
        return Ok(());
    }
    if registration("bb").is_some() || registration("bb2").is_some() {
        line(&["SKIP distributions bb or bb2 exist here; not touched"]);
        return Ok(());
    }
    let wsl = Wsl { exe };
    let others = Key::open(HKEY_CURRENT_USER, LXSS).map_or(0, |k| k.subkeys().len());
    let had_default = default_guid().is_some();
    let mut work = dir.clone();
    work.push_str("wsl_test.d");
    mkdir(&work);
    let (mut bb_dir, mut bb2_dir) = (work.clone(), work.clone());
    bb_dir.push_str("\\bb");
    bb2_dir.push_str("\\bb2");

    // --version.
    let r = wsl.run("--version");
    let text = utf16le(&r.out);
    info("--version", &text);
    c.ok(r.code == 0, "--version exits 0")?;
    c.ok(text.starts_with("WSL version: "), "--version starts with WSL version:")?;

    // --import, from a .tar.gz and from a .tar.
    let mut args = String::from("--import bb \"");
    args.push_str(&bb_dir);
    args.push_str("\" \"");
    args.push_str(&gz);
    args.push('"');
    let r = wsl.run(&args);
    let text = utf16le(&r.out);
    info("--import stdout", &text);
    info("--import stderr", &utf16le(&r.err));
    c.ok(r.code == 0, "--import exits 0")?;
    c.ok(text.ends_with(&ok_line[..]), "--import succeeds")?;
    let Some((bb_guid, k)) = registration("bb") else { return c.ok(false, "bb registered") };
    c.ok(
        bb_guid.len() == 38 && bb_guid.starts_with('{') && bb_guid.ends_with('}'),
        "a {guid} key",
    )?;
    info("bb key", &bb_guid);
    let base = k.string("BasePath").unwrap_or_default();
    info("BasePath", &base);
    c.ok(base.ends_with("wsl_test.d\\bb"), "BasePath is the install directory")?;
    c.ok(k.dword("State") == Some(1), "State 1 (installed)")?;
    c.ok(k.dword("Version") == Some(2), "Version 2")?;
    c.ok(k.dword("DefaultUid") == Some(0), "DefaultUid 0 (root)")?;
    info("Flags", &hex(k.dword("Flags").unwrap_or(0)));
    drop(k);
    if !had_default {
        c.ok(default_guid().as_deref() == Some(&bb_guid[..]), "the first import is the default")?;
    }
    let mut args = String::from("--import bb2 \"");
    args.push_str(&bb2_dir);
    args.push_str("\" \"");
    args.push_str(&plain);
    args.push('"');
    let r = wsl.run(&args);
    c.ok(r.code == 0, "--import of a plain tar exits 0")?;
    let Some((bb2_guid, _)) = registration("bb2") else { return c.ok(false, "bb2 registered") };
    let mut args = String::from("--import bb \"");
    args.push_str(&work);
    args.push_str("\\again\" \"");
    args.push_str(&gz);
    args.push('"');
    let r = wsl.run(&args);
    let text = utf16le(&r.out);
    info("--import of a taken name", &text);
    c.ok(r.code == WSL_FAILED, "--import of a taken name exits -1")?;
    c.ok(text.starts_with(EXISTS), "--import of a taken name says so")?;
    let mut args = String::from("--import bbx \"");
    args.push_str(&work);
    args.push_str("\\no\\such\" \"");
    args.push_str(&gz);
    args.push('"');
    let r = wsl.run(&args);
    let text = utf16le(&r.out);
    info("--import below a missing directory", &text);
    c.ok(r.code == WSL_FAILED, "--import below a missing directory exits -1")?;
    c.ok(text.starts_with("The system cannot find the path specified.\r\n"), "and says so")?;
    c.ok(registration("bbx").is_none(), "a failed import leaves no registration")?;

    // Running programs.
    let r = wsl.run("-d bb -- echo hi");
    c.ok(r.code == 0, "-d bb -- echo hi exits 0")?;
    c.ok(r.out == b"hi\n", "echo hi prints hi")?;
    c.ok(r.err.is_empty(), "nothing on stderr")?;
    let r = wsl.run("-d bb --exec /bin/sh -c \"exit 3\"");
    c.ok(r.code == 3, "the Linux exit status is the exit code")?;
    let r = wsl.run("-d bb -e echo exec");
    c.ok(r.code == 0 && r.out == b"exec\n", "--exec finds echo on PATH")?;
    let r = wsl.run("-d bb -- echo $WSL_DISTRO_NAME");
    c.ok(r.out == b"bb\n", "WSL_DISTRO_NAME")?;
    let r = wsl.run("-d bb -- id -u");
    c.ok(r.code == 0, "id -u exits 0")?;
    info("id -u", &lossy(&r.out));
    let r = wsl.run("-d bb -- pwd");
    info("pwd", &lossy(&r.out));

    // One file system each.
    let r = wsl.run("-d bb -- echo one > /tmp/f; cat /tmp/f");
    c.ok(r.code == 0 && r.out == b"one\n", "a file written in bb reads back")?;
    let r = wsl.run("-d bb2 -- cat /tmp/f");
    c.ok(r.code == 1 && r.out.is_empty(), "the file is not in bb2")?;
    info("bb2 cat stderr", &lossy(&r.err));
    let r = wsl.run("-d bb2 -- echo two > /root/g");
    c.ok(r.code == 0, "a file written in bb2")?;
    let r = wsl.run("-d bb -- ls /root");
    c.ok(r.code == 0 && r.out.is_empty(), "is not in bb")?;
    let r = wsl.run("-d bb -- ls /");
    info("ls /", &lossy(&r.out));
    let listing = lossy(&r.out);
    c.ok(
        listing.lines().any(|l| l == "bin") && listing.lines().any(|l| l == "etc"),
        "/ is the rootfs",
    )?;
    c.ok(!listing.lines().any(|l| l == "Windows" || l == "Users"), "and not C:")?;

    // -l -v.
    let r = wsl.run("-l -v");
    let text = utf16le(&r.out);
    info("-l -v", &text);
    c.ok(r.code == 0, "-l -v exits 0")?;
    let rs = rows(&text);
    c.ok(text == table(&rs), "-l -v is WSL's table")?;
    let (Some(a), Some(b)) = (row(&rs, "bb"), row(&rs, "bb2")) else {
        return c.ok(false, "bb and bb2 listed");
    };
    c.ok(a.3 == "2" && b.3 == "2", "version 2")?;
    c.ok(["Running", "Stopped"].contains(&&a.2[..]), "a state")?;
    info("bb's state after its commands", &a.2);
    if !had_default {
        c.ok(a.0 && !b.0, "bb is the default")?;
        c.ok(rs[0].1 == "bb", "the default first")?;
    }
    let r = wsl.run("--list --verbose");
    c.ok(r.code == 0 && utf16le(&r.out) == text, "--list --verbose is -l -v")?;
    let r = wsl.run("--set-default bb2");
    c.ok(r.code == 0 && utf16le(&r.out) == ok_line, "--set-default")?;
    c.ok(default_guid().as_deref() == Some(&bb2_guid[..]), "DefaultDistribution is bb2's")?;
    let rs = rows(&utf16le(&wsl.run("-l -v").out));
    c.ok(row(&rs, "bb2").is_some_and(|r| r.0) && rs[0].1 == "bb2", "* on bb2, listed first")?;
    let r = wsl.run("-- echo $WSL_DISTRO_NAME");
    c.ok(r.out == b"bb2\n", "the default runs without -d")?;

    // --terminate.
    let Some(sleeper) = wsl.start("-d bb -- sleep 30") else { return c.ok(false, "sleep started") };
    let mut running = false;
    for _ in 0..100 {
        let rs = rows(&utf16le(&wsl.run("-l -v").out));
        if row(&rs, "bb").is_some_and(|r| r.2 == "Running") {
            running = true;
            break;
        }
        // SAFETY: a plain call.
        unsafe { Sleep(100) };
    }
    c.ok(running, "bb is Running while sleep runs")?;
    let r = wsl.run("--terminate bb");
    c.ok(r.code == 0 && utf16le(&r.out) == ok_line, "--terminate")?;
    let ended = Wsl::finish(sleeper, 10_000);
    c.ok(ended.code != 0xdead_0001, "the sleeping wsl.exe returns")?;
    info("exit code of a terminated wsl.exe", &hex(ended.code));
    let rs = rows(&utf16le(&wsl.run("-l -v").out));
    c.ok(row(&rs, "bb").is_some_and(|r| r.2 == "Stopped"), "bb Stopped")?;
    let r = wsl.run("--shutdown");
    c.ok(r.code == 0 && r.out.is_empty(), "--shutdown exits 0 silently")?;

    // Errors.
    let r = wsl.run("--bogus");
    let text = utf16le(&r.out);
    info("--bogus", &text);
    c.ok(r.code == WSL_FAILED, "an unknown option exits -1")?;
    c.ok(text.starts_with("Invalid command line argument: --bogus\r\n"), "and says so")?;
    let r = wsl.run("-d nope -- true");
    let text = utf16le(&r.out);
    info("-d nope", &text);
    c.ok(r.code == WSL_FAILED, "an unknown distribution exits -1")?;
    c.ok(text.starts_with(NOT_FOUND), "and says so")?;

    // --unregister.
    let r = wsl.run("--unregister bb2");
    let text = utf16le(&r.out);
    info("--unregister", &text);
    c.ok(r.code == 0 && text.ends_with(&ok_line[..]), "--unregister")?;
    c.ok(registration("bb2").is_none(), "its registration is gone")?;
    c.ok(!exists(&bb2_dir), "and its install directory")?;
    if others == 0 {
        c.ok(default_guid().as_deref() == Some(&bb_guid[..]), "the default moved to bb")?;
    }
    let r = wsl.run("-d bb2 -- true");
    c.ok(r.code == WSL_FAILED, "bb2 is gone")?;
    let r = wsl.run("--unregister bb");
    c.ok(r.code == 0, "--unregister bb")?;
    c.ok(registration("bb").is_none() && !exists(&bb_dir), "bb is gone")?;
    if others == 0 {
        let r = wsl.run("-l -v");
        let text = utf16le(&r.out);
        c.ok(r.code == WSL_FAILED && text.starts_with(NO_DISTROS), "no distributions left")?;
    }
    let w = wide(&work);
    // SAFETY: a valid path.
    unsafe { RemoveDirectoryW(w.as_ptr()) };
    line(&["ok"]);
    Ok(())
}

/// `wsl_test import`: bb from the .tar.gz, for the terminal scenario.
fn import_only() -> Result<(), u32> {
    let mut c = Checks(0);
    let root = env("SystemRoot").unwrap_or(String::from("C:\\Windows"));
    let mut exe = root.clone();
    exe.push_str("\\System32\\wsl.exe");
    let dir = here();
    let mut work = dir.clone();
    work.push_str("wsl_test.d");
    mkdir(&work);
    let mut args = String::from("--import bb \"");
    args.push_str(&dir);
    args.push_str("wsl_test.d\\bb\" \"");
    args.push_str(&dir);
    args.push_str("bb-rootfs.tar.gz\"");
    let r = Wsl { exe }.run(&args);
    c.ok(r.code == 0, "--import bb")?;
    line(&["imported bb"]);
    Ok(())
}

/// Whether the command line asks for `import`.
fn import_mode() -> bool {
    // SAFETY: the process's NUL-terminated command line.
    let cmd = unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(core::slice::from_raw_parts(p, n))
    };
    cmd.split_whitespace().skip(1).any(|w| w == "import")
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match if import_mode() { import_only() } else { test() } {
        Ok(()) => 0,
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    line(&["panic"]);
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
