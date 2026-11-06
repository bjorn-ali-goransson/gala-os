//! `gala install` over winget manifests (ROADMAP PK stages 1 and 2):
//! a zip package is installed from a local winget-pkgs snapshot
//! (`C:\pk_source`, user/pk_install_test/source, made by
//! make_source.py), its command runs from PATH, it is listed, upgraded
//! and uninstalled; failures roll back. `gala` is gala's own command, so
//! there is no Windows run to compare with; what it leaves on the machine
//! is what winget documents and leaves for a portable package:
//!
//! - files under `%LOCALAPPDATA%\Microsoft\WinGet\Packages\<id>_<source>`
//!   (`GalaTest.PkHello_Microsoft.Winget.Source_8wekyb3d8bbwe`);
//! - the command in `%LOCALAPPDATA%\Microsoft\WinGet\Links` (winget: a
//!   symbolic link; gala: `pkhello.exe`, a shim, with `pkhello.shim`),
//!   that directory on the user's PATH (`HKCU\Environment\Path`);
//! - the ARP entry `HKCU\Software\Microsoft\Windows\CurrentVersion\
//!   Uninstall\<id>_<source>` with DisplayName, DisplayVersion,
//!   Publisher, InstallLocation, WinGetPackageIdentifier.
//!
//! The command is run as a new shell would after the install: PATH taken
//! again from `HKCU\Environment` (winget tells running programs with
//! WM_SETTINGCHANGE; a console started afterwards has it), then
//! CreateProcessW with the bare name `pkhello`, which searches PATH.
//!
//! Checks: search (by tag and name, a real winget-pkgs manifest found);
//! install 1.0.0 (files, shim, PATH, ARP, the install log); the command
//! runs (hello.exe's line) with its arguments; `gala list`; installing
//! again is "already installed"; a package wanting the same command fails
//! after writing its files and is rolled back (exit 4, nothing left); a
//! wrong SHA-256 (exit 5); an MSIX (exit 3, a later stage); a download
//! from GitHub fails (exit 4: this image has no winhttp.dll, nor network);
//! `gala upgrade` lists and installs 1.1.0 (the
//! old files gone); uninstall removes files, command and record; the
//! command is gone from PATH.
//!
//! With the argument `nsis` (scenario pk_install_nsis) it checks an NSIS
//! installer instead (`nsis_test`); the zip run also checks an Inno Setup
//! stand-in that registers nothing (exit 4) and an MSIX (exit 3). With
//! `https` (scenario pk_install_https) it checks downloads over winhttp
//! and Schannel (`https_test`).
//!
//! Prints `pk_install_test: ok` or exits with the number of the failing
//! check (the install log is printed on failure).

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
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn CreatePipe(
        read: *mut Handle,
        write: *mut Handle,
        sa: *const SecurityAttributes,
        size: u32,
    ) -> Bool;
    fn SetHandleInformation(h: Handle, mask: u32, flags: u32) -> Bool;
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
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn Sleep(ms: u32);
    fn LoadLibraryW(name: *const u16) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn GetCommandLineW() -> *const u16;
    fn SetEnvironmentVariableW(name: *const u16, value: *const u16) -> Bool;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegGetValueW(
        key: isize,
        subkey: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: *mut u8,
        len: *mut u32,
    ) -> u32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
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

const STARTF_USESTDHANDLES: u32 = 0x100;
const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const RRF_RT_REG_SZ: u32 = 0x2;
const RRF_RT_REG_EXPAND_SZ_NOEXPAND: u32 = 0x1000_0004;
const INVALID_ATTRIBUTES: u32 = u32::MAX;
const ERROR_FILE_NOT_FOUND: u32 = 2;

const KEY: &str = "GalaTest.PkHello_Microsoft.Winget.Source_8wekyb3d8bbwe";
const HELLO: &[u8] = b"Hello from user mode!";

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

fn hex(v: u32) -> [u8; 10] {
    let mut s = *b"0x00000000";
    for i in 0..8 {
        let d = ((v >> (28 - 4 * i)) & 0xf) as u8;
        s[2 + i] = if d < 10 { b'0' + d } else { b'a' + d - 10 };
    }
    s
}

/// A NUL-terminated UTF-16 string built from ASCII parts.
struct W {
    b: [u16; 1024],
    n: usize,
}

impl W {
    fn new(text: &str) -> W {
        let mut w = W { b: [0; 1024], n: 0 };
        w.add(text.as_bytes());
        w
    }

    fn add(&mut self, text: &[u8]) -> &mut W {
        for &c in text {
            if self.n < self.b.len() - 1 {
                self.b[self.n] = u16::from(c);
                self.n += 1;
            }
        }
        self
    }

    fn add_w(&mut self, text: &[u16]) -> &mut W {
        for &c in text {
            if self.n < self.b.len() - 1 {
                self.b[self.n] = c;
                self.n += 1;
            }
        }
        self
    }

    fn ptr(&mut self) -> *mut u16 {
        self.b[self.n] = 0;
        self.b.as_mut_ptr()
    }

    fn text(&self) -> &[u16] {
        &self.b[..self.n]
    }
}

struct Output {
    b: [u8; 16384],
    n: usize,
}

impl Output {
    fn text(&self) -> &[u8] {
        &self.b[..self.n]
    }

    fn has(&self, what: &[u8]) -> bool {
        contains(self.text(), what)
    }
}

fn contains(hay: &[u8], what: &[u8]) -> bool {
    what.is_empty() || hay.windows(what.len()).any(|w| w == what)
}

/// Runs a command line (the program found as CreateProcessW finds it
/// when `exe` is None): its exit code (or 0xfffe0000 | the error) and
/// output.
fn run(exe: Option<&str>, line: &str) -> (u32, Output) {
    let mut o = Output { b: [0; 16384], n: 0 };
    let null = core::ptr::null_mut();
    let sa = SecurityAttributes { length: 24, descriptor: null, inherit: 1 };
    let (mut r, mut w) = (null, null);
    let mut cmd = W::new(line);
    let mut exe_w = W::new(exe.unwrap_or(""));
    // SAFETY: plain calls with our buffers and handles.
    unsafe {
        if CreatePipe(&mut r, &mut w, &sa, 0) == 0 {
            return (0xffff_0000 | GetLastError(), o);
        }
        SetHandleInformation(r, 1, 0);
        let mut startup = [0u8; 104];
        startup[0] = 104;
        startup[60..64].copy_from_slice(&STARTF_USESTDHANDLES.to_le_bytes());
        startup[80..88].copy_from_slice(&(GetStdHandle(-10i32 as u32) as u64).to_le_bytes());
        startup[88..96].copy_from_slice(&(w as u64).to_le_bytes());
        startup[96..104].copy_from_slice(&(w as u64).to_le_bytes());
        let mut info = [0usize; 3];
        let app = if exe.is_some() { exe_w.ptr() as *const u16 } else { core::ptr::null() };
        let ok = CreateProcessW(
            app,
            cmd.ptr(),
            null,
            null,
            1,
            0,
            null,
            core::ptr::null(),
            startup.as_ptr(),
            &mut info,
        );
        CloseHandle(w);
        if ok == 0 {
            CloseHandle(r);
            return (0xfffe_0000 | GetLastError(), o);
        }
        let mut got = 0;
        while o.n < o.b.len()
            && ReadFile(r, o.b.as_mut_ptr().add(o.n), (o.b.len() - o.n) as u32, &mut got, null) != 0
            && got > 0
        {
            o.n += got as usize;
        }
        CloseHandle(r);
        WaitForSingleObject(info[0] as Handle, 60_000);
        let mut code = 0xdead_0000;
        GetExitCodeProcess(info[0] as Handle, &mut code);
        CloseHandle(info[0] as Handle);
        CloseHandle(info[1] as Handle);
        (code, o)
    }
}

/// `%LOCALAPPDATA%` followed by `rest`.
fn local(rest: &str) -> W {
    let mut buf = [0u16; 512];
    let name = W::new("LOCALAPPDATA");
    // SAFETY: our buffers.
    let n = unsafe { GetEnvironmentVariableW(name.b.as_ptr(), buf.as_mut_ptr(), 512) } as usize;
    let mut w = W::new("");
    w.add_w(&buf[..n.min(512)]).add(rest.as_bytes());
    w
}

fn exists(mut path: W) -> bool {
    // SAFETY: a NUL-terminated path.
    unsafe { GetFileAttributesW(path.ptr()) != INVALID_ATTRIBUTES }
}

/// A REG_SZ (or REG_EXPAND_SZ, unexpanded) value of HKCU as ASCII
/// (None if absent).
fn hkcu(subkey: &str, value: &str, expand: bool) -> Option<([u8; 2048], usize)> {
    let mut key = W::new(subkey);
    let mut name = W::new(value);
    let mut data = [0u16; 2048];
    let mut len = 4096u32;
    let mut kind = 0;
    let flags = if expand { RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ_NOEXPAND } else { RRF_RT_REG_SZ };
    // SAFETY: our buffers with their lengths.
    let e = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.ptr(),
            name.ptr(),
            flags,
            &mut kind,
            data.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if e != 0 {
        return None;
    }
    let mut s = [0u8; 2048];
    let n = (len as usize / 2).saturating_sub(1).min(2048);
    for i in 0..n {
        s[i] = if data[i] < 0x80 { data[i] as u8 } else { b'?' };
    }
    Some((s, n))
}

fn arp(value: &str) -> Option<([u8; 2048], usize)> {
    let mut k = [0u8; 200];
    let base = b"Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\";
    k[..base.len()].copy_from_slice(base);
    k[base.len()..base.len() + KEY.len()].copy_from_slice(KEY.as_bytes());
    hkcu(core::str::from_utf8(&k[..base.len() + KEY.len()]).unwrap_or(""), value, false)
}

/// PATH as a new logon would make it: the process's own plus the user's
/// `HKCU\Environment\Path` (winget broadcasts WM_SETTINGCHANGE; programs
/// started afterwards see the new value).
fn refresh_path() {
    let mut buf = [0u16; 2048];
    let name = W::new("Path");
    // SAFETY: our buffers.
    let n = unsafe { GetEnvironmentVariableW(name.b.as_ptr(), buf.as_mut_ptr(), 2048) } as usize;
    let mut path = W::new("");
    path.add_w(&buf[..n.min(1024)]);
    if let Some((user, len)) = hkcu("Environment", "Path", true) {
        // %LOCALAPPDATA% may be in it (REG_EXPAND_SZ): expanded here.
        path.add(b";");
        let mut i = 0;
        let var = b"%LOCALAPPDATA%";
        while i < len {
            if user[i..len].starts_with(var) {
                let l = local("");
                path.add_w(l.text());
                i += var.len();
            } else {
                path.add(&user[i..i + 1]);
                i += 1;
            }
        }
    }
    let mut name = W::new("Path");
    // SAFETY: NUL-terminated strings.
    unsafe { SetEnvironmentVariableW(name.ptr(), path.ptr()) };
}

/// Prints the install log (on failure: what gala did and undid).
fn dump_log() {
    let mut p = local("\\gala\\logs\\install.log");
    // SAFETY: plain calls with our buffers.
    unsafe {
        let h = CreateFileW(
            p.ptr(),
            0x8000_0000,
            7,
            core::ptr::null_mut(),
            3,
            0,
            core::ptr::null_mut(),
        );
        if h as isize == -1 {
            out(b"pk_install_test: no install log\r\n");
            return;
        }
        out(b"pk_install_test: the install log:\r\n");
        let mut b = [0u8; 4096];
        let mut got = 0;
        while ReadFile(h, b.as_mut_ptr(), b.len() as u32, &mut got, core::ptr::null_mut()) != 0
            && got > 0
        {
            out(&b[..got as usize]);
        }
        CloseHandle(h);
    }
}

struct Checks(u32);

impl Checks {
    fn ok(&mut self, cond: bool, got: u32, what: &str) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"pk_install_test: check ");
        out(&hex(self.0));
        out(b" (");
        out(what.as_bytes());
        out(b") got ");
        out(&hex(got));
        out(b"\r\n");
        Err(self.0)
    }

    /// `gala <args>` exits with `code` and its output has every `has`.
    fn gala(&mut self, args: &str, code: u32, has: &[&[u8]]) -> Result<Output, u32> {
        let mut line = [0u8; 512];
        let n = args.len().min(500);
        line[..5].copy_from_slice(b"gala ");
        line[5..5 + n].copy_from_slice(&args.as_bytes()[..n]);
        let line = core::str::from_utf8(&line[..5 + n]).unwrap_or("gala");
        let (got, o) = run(Some("C:\\gala\\gala.exe"), line);
        let good = got == code && has.iter().all(|h| o.has(h));
        if !good {
            out(b"pk_install_test: ");
            out(line.as_bytes());
            out(b" -> ");
            out(&hex(got));
            out(b":\r\n");
            out(o.text());
        }
        self.ok(good, got, args)?;
        Ok(o)
    }

    /// `pkhello <args>` found on PATH: hello.exe's line, exit 0.
    fn pkhello_runs(&mut self) -> Result<(), u32> {
        self.command_runs("pkhello --some \"args here\"")
    }

    /// A command line whose program is an installed command found on
    /// PATH (all are hello.exe): its line, exit 0.
    fn command_runs(&mut self, line: &str) -> Result<(), u32> {
        let (got, o) = run(None, line);
        if got != 0 || !o.has(HELLO) {
            out(b"pk_install_test: ");
            out(line.as_bytes());
            out(b" -> ");
            out(&hex(got));
            out(b":\r\n");
            out(o.text());
        }
        self.ok(got == 0 && o.has(HELLO), got, line)
    }
}

fn test() -> Result<(), u32> {
    let mut c = Checks(0);
    let src = "--source C:\\pk_source";

    // Search: by tag, by name; a real winget-pkgs manifest (ripgrep).
    c.gala("search hello --source C:\\pk_source", 0, &[b"GalaTest.PkHello", b"1.1.0"])?;
    c.gala("search grep --source C:\\pk_source", 0, &[b"BurntSushi.ripgrep.MSVC", b"15.2.0"])?;
    c.gala("search nothing-like-this --source C:\\pk_source", 2, &[b"No package"])?;
    // Nothing is installed: the command is not there.
    let (got, _) = run(None, "pkhello");
    c.ok(got == 0xfffe_0000 | ERROR_FILE_NOT_FOUND, got, "pkhello before the install")?;

    // Install 1.0.0.
    let mut a = [0u8; 200];
    let args = join(&mut a, &["install GalaTest.PkHello --version 1.0.0 ", src]);
    c.gala(args, 0, &[b"Successfully installed", b"pkhello"])?;
    let dir =
        "\\Microsoft\\WinGet\\Packages\\GalaTest.PkHello_Microsoft.Winget.Source_8wekyb3d8bbwe";
    let mut p = [0u8; 300];
    c.ok(
        exists(local(join(&mut p, &[dir, "\\pkhello-1.0.0\\pkhello.exe"]))),
        0,
        "the program unpacked",
    )?;
    c.ok(
        exists(local(join(&mut p, &[dir, "\\pkhello-1.0.0\\doc\\LICENSE.txt"]))),
        0,
        "the zip's subdirectory",
    )?;
    c.ok(exists(local("\\Microsoft\\WinGet\\Links\\pkhello.exe")), 0, "the command's shim")?;
    c.ok(exists(local("\\Microsoft\\WinGet\\Links\\pkhello.shim")), 0, "the shim's target file")?;
    let path = hkcu("Environment", "Path", true);
    c.ok(
        path.is_some_and(|(s, n)| contains(&s[..n], b"Microsoft\\WinGet\\Links")),
        0,
        "Links on the user's PATH",
    )?;
    let v = arp("DisplayVersion");
    c.ok(v.is_some_and(|(s, n)| &s[..n] == b"1.0.0"), 0, "ARP DisplayVersion")?;
    let v = arp("DisplayName");
    c.ok(v.is_some_and(|(s, n)| &s[..n] == b"PkHello"), 0, "ARP DisplayName")?;
    let v = arp("WinGetPackageIdentifier");
    c.ok(v.is_some_and(|(s, n)| &s[..n] == b"GalaTest.PkHello"), 0, "ARP WinGetPackageIdentifier")?;
    let v = arp("InstallLocation");
    c.ok(v.is_some_and(|(s, n)| contains(&s[..n], KEY.as_bytes())), 0, "ARP InstallLocation")?;
    c.ok(exists(local("\\gala\\logs\\install.log")), 0, "the install log")?;

    // The command, from PATH, as a new shell has it.
    refresh_path();
    c.pkhello_runs()?;

    // Listed; installing again changes nothing.
    c.gala("list", 0, &[b"GalaTest.PkHello", b"1.0.0", b"PkHello"])?;
    let args = join(&mut a, &["install GalaTest.PkHello --version 1.0.0 ", src]);
    c.gala(args, 0, &[b"already installed"])?;

    // A package wanting the same command: its files are written, then
    // the install fails and is rolled back.
    let args = join(&mut a, &["install GalaTest.Conflict ", src]);
    c.gala(args, 4, &[b"pkhello exists already", b"rolled back"])?;
    c.ok(
        !exists(local("\\Microsoft\\WinGet\\Packages\\GalaTest.Conflict_Microsoft.Winget.Source_8wekyb3d8bbwe")),
        0,
        "the conflicting package's files removed",
    )?;
    c.gala("list", 0, &[b"GalaTest.PkHello"])?;
    c.pkhello_runs()?;

    // A download that is not the manifest's; types of later stages.
    let args = join(&mut a, &["install GalaTest.BadHash ", src]);
    c.gala(args, 5, &[b"SHA-256"])?;
    c.ok(!exists(local("\\Microsoft\\WinGet\\Links\\badhash.exe")), 0, "nothing of BadHash")?;
    // An Inno Setup stand-in that registers nothing: run (winget's Inno
    // switches: its default mode shows progress, /SILENT; --silent shows
    // nothing, /VERYSILENT), then refused for want of an uninstall entry.
    let args = join(&mut a, &["install GalaTest.InnoSetup ", src]);
    c.gala(args, 4, &[b"registered no uninstall entry"])?;
    c.ok(log_has(b"/SP- /SILENT /SUPPRESSMSGBOXES /NORESTART"), 0, "Inno's default switches")?;
    let args = join(&mut a, &["install GalaTest.InnoSetup --silent ", src]);
    c.gala(args, 4, &[b"registered no uninstall entry"])?;
    c.ok(log_has(b"/SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART"), 0, "Inno's --silent switches")?;
    let args = join(&mut a, &["install GalaTest.Msix ", src]);
    c.gala(args, 3, &[b"msix"])?;
    let args = join(&mut a, &["install jqlang.jq ", src]);
    c.gala(args, 4, &[b"downloading https://github.com/jqlang/jq/"])?;
    c.gala("install No.Such.Package --source C:\\pk_source", 2, &[b"No.Such.Package"])?;

    // Upgrade: listed, then 1.1.0 in place of 1.0.0.
    let args = join(&mut a, &["upgrade ", src]);
    c.gala(args, 0, &[b"GalaTest.PkHello", b"1.0.0", b"1.1.0"])?;
    c.gala("upgrade GalaTest.PkHello", 0, &[b"Successfully upgraded", b"1.1.0"])?;
    // winget's selection, logged: 1.1.0 lists x86 first; x64 is taken.
    c.ok(
        log_has(b"selected installer=2 type=zip nested=portable arch=x64"),
        0,
        "the selected installer logged",
    )?;
    c.ok(log_has(b"runner-up installer=1 arch=x86"), 0, "the runner-up logged")?;
    c.ok(exists(local(join(&mut p, &[dir, "\\pkhello-1.1.0\\README.txt"]))), 0, "1.1.0's files")?;
    c.ok(!exists(local(join(&mut p, &[dir, "\\pkhello-1.0.0"]))), 0, "1.0.0's files gone")?;
    let v = arp("DisplayVersion");
    c.ok(v.is_some_and(|(s, n)| &s[..n] == b"1.1.0"), 0, "ARP DisplayVersion after the upgrade")?;
    c.pkhello_runs()?;
    c.gala("upgrade GalaTest.PkHello", 0, &[b"No newer version"])?;

    // Uninstall: files, command and record gone.
    c.gala("uninstall GalaTest.PkHello", 0, &[b"Successfully uninstalled"])?;
    c.ok(!exists(local(dir)), 0, "the package directory removed")?;
    c.ok(!exists(local("\\Microsoft\\WinGet\\Links\\pkhello.exe")), 0, "the command removed")?;
    c.ok(!exists(local("\\Microsoft\\WinGet\\Links\\pkhello.shim")), 0, "the shim file removed")?;
    c.ok(arp("DisplayName").is_none(), 0, "the ARP entry removed")?;
    let (got, _) = run(None, "pkhello");
    c.ok(got == 0xfffe_0000 | ERROR_FILE_NOT_FOUND, got, "pkhello after the uninstall")?;
    let o = c.gala("list", 0, &[])?;
    c.ok(!o.has(b"GalaTest.PkHello"), 0, "not listed after the uninstall")?;
    c.gala("uninstall GalaTest.PkHello", 2, &[])?;
    dependencies_and_scope(&mut c)
}

/// winget's documented behaviour for dependencies ("install command":
/// PackageDependencies are installed first; uninstall leaves them) and
/// scope (`--scope` is a requirement; an upgrade keeps the installed
/// scope).
fn dependencies_and_scope(c: &mut Checks) -> Result<(), u32> {
    let src = "--source C:\\pk_source";
    let mut a = [0u8; 200];
    // PkDep needs PkLib 1.0.0: PkLib is installed first.
    let args = join(&mut a, &["install GalaTest.PkDep ", src]);
    c.gala(args, 0, &[b"GalaTest.PkLib 1.0.0", b"Successfully installed"])?;
    c.ok(
        log_has(b"plan install GalaTest.PkLib 1.0.0 installer 1 (for GalaTest.PkDep)"),
        0,
        "the dependency planned",
    )?;
    c.ok(log_has(b"installed id=GalaTest.PkLib version=1.0.0"), 0, "the dependency installed")?;
    c.ok(exists(local("\\Microsoft\\WinGet\\Links\\pklib.exe")), 0, "the dependency's command")?;
    refresh_path();
    c.command_runs("pklib")?;
    c.command_runs("pkdep")?;
    c.gala("list", 0, &[b"GalaTest.PkLib", b"GalaTest.PkDep"])?;
    // Uninstalling the package leaves its dependency.
    c.gala("uninstall GalaTest.PkDep", 0, &[b"Successfully uninstalled"])?;
    c.gala("list", 0, &[b"GalaTest.PkLib"])?;
    c.gala("uninstall GalaTest.PkLib", 0, &[b"Successfully uninstalled"])?;
    // A dependency the source does not have: nothing is installed.
    let args = join(&mut a, &["install GalaTest.PkDepMissing ", src]);
    c.gala(args, 2, &[b"needs GalaTest.NoSuchLib"])?;
    c.ok(
        !exists(local("\\Microsoft\\WinGet\\Links\\pkdepmissing.exe")),
        0,
        "nothing of PkDepMissing",
    )?;

    // Scope: --scope machine (Program Files and HKLM for a portable
    // package) is not built: refused, nothing changed.
    let args = join(&mut a, &["install GalaTest.PkScoped --scope machine ", src]);
    c.gala(args, 3, &[b"machine scope"])?;
    let args = join(&mut a, &["install GalaTest.PkScoped --scope user --version 1.0.0 ", src]);
    c.gala(args, 0, &[b"Successfully installed"])?;
    c.ok(
        log_has(b"scope=user locale=- url=https://packages.gala.invalid/pkscoped-1.0.0-user.zip"),
        0,
        "the user installer",
    )?;
    // The upgrade keeps the user scope, though 1.1.0 lists the machine
    // installer first.
    c.gala("upgrade GalaTest.PkScoped", 0, &[b"Successfully upgraded", b"1.1.0"])?;
    c.ok(log_has(b"installed scope=user type=portable"), 0, "the installed scope read")?;
    c.ok(log_has(b"url=https://packages.gala.invalid/pkscoped-1.1.0-user.zip"), 0, "scope kept")?;
    refresh_path();
    c.command_runs("pkscoped")?;
    c.gala("uninstall GalaTest.PkScoped", 0, &[b"Successfully uninstalled"])?;
    Ok(())
}

/// ASCII parts joined into `buf`.
fn join<'a>(buf: &'a mut [u8], parts: &[&str]) -> &'a str {
    let mut n = 0;
    for p in parts {
        let l = p.len().min(buf.len() - n);
        buf[n..n + l].copy_from_slice(&p.as_bytes()[..l]);
        n += l;
    }
    core::str::from_utf8(&buf[..n]).unwrap_or("")
}

// ---- An NSIS installer (scenario pk_install_nsis) ----------------------

const NSIS_ARP: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\GalaTest.PkNsis";

/// A whole file (up to 16 KiB; None if it cannot be opened).
fn read_file(mut path: W) -> Option<Output> {
    let mut o = Output { b: [0; 16384], n: 0 };
    // SAFETY: plain calls with our buffers.
    unsafe {
        let h = CreateFileW(
            path.ptr(),
            0x8000_0000,
            7,
            core::ptr::null_mut(),
            3,
            0,
            core::ptr::null_mut(),
        );
        if h as isize == -1 {
            return None;
        }
        let mut got = 0;
        while o.n < o.b.len()
            && ReadFile(
                h,
                o.b.as_mut_ptr().add(o.n),
                (o.b.len() - o.n) as u32,
                &mut got,
                core::ptr::null_mut(),
            ) != 0
            && got > 0
        {
            o.n += got as usize;
        }
        CloseHandle(h);
    }
    Some(o)
}

/// Prints a file the test reads (what NSIS wrote), or that it is missing.
fn show_file(name: &str, o: &Option<Output>) {
    out(b"pk_install_test: ");
    out(name.as_bytes());
    match o {
        Some(o) => {
            out(b": ");
            out(o.text());
            out(b"\r\n");
        }
        None => out(b": cannot be opened\r\n"),
    }
}

/// The install log has `what` (anywhere: read in chunks that overlap
/// by `what`'s length, the log being longer than one buffer).
fn log_has(what: &[u8]) -> bool {
    let mut path = local("\\gala\\logs\\install.log");
    let keep = what.len().saturating_sub(1).min(1024);
    let mut b = [0u8; 8192];
    let mut have = 0usize;
    // SAFETY: plain calls with our buffers; the handle is closed.
    unsafe {
        let h = CreateFileW(
            path.ptr(),
            0x8000_0000,
            7,
            core::ptr::null_mut(),
            3,
            0,
            core::ptr::null_mut(),
        );
        if h as isize == -1 {
            return false;
        }
        let mut found = false;
        loop {
            let mut got = 0;
            let ok = ReadFile(
                h,
                b.as_mut_ptr().add(have),
                (b.len() - have) as u32,
                &mut got,
                core::ptr::null_mut(),
            );
            if ok == 0 || got == 0 {
                break;
            }
            have += got as usize;
            if contains(&b[..have], what) {
                found = true;
                break;
            }
            let tail = have.min(keep);
            b.copy_within(have - tail..have, 0);
            have = tail;
        }
        CloseHandle(h);
        found
    }
}

fn nsis_value(value: &str) -> Option<([u8; 2048], usize)> {
    hkcu(NSIS_ARP, value, false)
}

fn nsis_value_is(value: &str, want: &[u8]) -> bool {
    nsis_value(value).is_some_and(|(s, n)| &s[..n] == want)
}

/// `%TEMP%` followed by `rest`.
fn temp(rest: &str) -> W {
    let mut buf = [0u16; 512];
    let name = W::new("TEMP");
    // SAFETY: our buffers.
    let n = unsafe { GetEnvironmentVariableW(name.b.as_ptr(), buf.as_mut_ptr(), 512) } as usize;
    let mut w = W::new("");
    w.add_w(&buf[..n.min(512)]).add(rest.as_bytes());
    w
}

/// `gala install` of a real NSIS installer (nsis/pknsis.nsi, x64), run
/// as winget runs it (/S, /D=), found through its uninstall entry,
/// upgraded by running 1.1.0's installer over it, uninstalled through its
/// UninstallString with /S. What the installer leaves is what NSIS
/// documents for this script: its files in $INSTDIR (default
/// `%LOCALAPPDATA%\Programs\PkNsis`), uninstall.exe, the HKCU entry it
/// writes.
fn nsis_test() -> Result<(), u32> {
    let mut c = Checks(0);
    let src = "--source C:\\pk_source";
    let mut a = [0u8; 300];
    c.gala("search nsis --source C:\\pk_source", 0, &[b"GalaTest.PkNsis", b"1.1.0"])?;
    // Install 1.0.0: the installer runs with /S.
    let args = join(&mut a, &["install GalaTest.PkNsis --version 1.0.0 ", src]);
    c.gala(args, 0, &[b"Successfully installed", b"Programs\\PkNsis"])?;
    c.ok(log_has(b"cmd=\""), 0, "the install log has the command line")?;
    c.ok(log_has(b"pknsis-1.0.0.exe\" /S"), 0, "run with /S")?;
    c.ok(log_has(b"exit code=0 (0x0)"), 0, "the installer's exit code logged")?;
    c.ok(
        log_has(b"arp-entry key=HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\GalaTest.PkNsis found-by="),
        0,
        "the ARP entry found",
    )?;
    let dir = "\\Programs\\PkNsis";
    let mut p = [0u8; 300];
    c.ok(exists(local(join(&mut p, &[dir, "\\pknsis.exe"]))), 0, "NSIS wrote the program")?;
    c.ok(exists(local(join(&mut p, &[dir, "\\uninstall.exe"]))), 0, "NSIS wrote its uninstaller")?;
    let readme = read_file(local(join(&mut p, &[dir, "\\README.txt"])));
    show_file("README.txt", &readme);
    c.ok(readme.is_some_and(|o| o.has(b"PkNsis 1.0.0")), 0, "NSIS's FileWrite")?;
    c.ok(
        !exists(temp("\\gala\\GalaTest.PkNsis_Microsoft.Winget.Source_8wekyb3d8bbwe")),
        0,
        "the staged installer removed",
    )?;
    c.ok(nsis_value_is("DisplayName", b"PkNsis"), 0, "NSIS's DisplayName")?;
    c.ok(nsis_value_is("DisplayVersion", b"1.0.0"), 0, "NSIS's DisplayVersion")?;
    c.ok(
        nsis_value("InstallLocation").is_some_and(|(s, n)| contains(&s[..n], b"Programs\\PkNsis")),
        0,
        "NSIS's InstallLocation",
    )?;
    c.ok(nsis_value_is("WinGetPackageIdentifier", b"GalaTest.PkNsis"), 0, "gala's tag")?;
    c.ok(nsis_value_is("WinGetInstallerType", b"nullsoft"), 0, "gala's type tag")?;
    // The program runs.
    let mut exe = local(join(&mut p, &[dir, "\\pknsis.exe"]));
    let mut line = [0u8; 300];
    let mut n = 0;
    for &u in exe.text() {
        if n < line.len() {
            line[n] = u as u8;
            n += 1;
        }
    }
    let _ = exe.ptr();
    let exe_s = core::str::from_utf8(&line[..n]).unwrap_or("");
    let (got, o) = run(Some(exe_s), "pknsis");
    c.ok(got == 0 && o.has(HELLO), got, "the installed program runs")?;
    // Listed; again is "already installed".
    c.gala("list", 0, &[b"GalaTest.PkNsis", b"1.0.0", b"PkNsis"])?;
    let args = join(&mut a, &["install GalaTest.PkNsis --version 1.0.0 ", src]);
    c.gala(args, 0, &[b"already installed"])?;
    // Upgrade: 1.1.0's installer over 1.0.0.
    c.gala("upgrade GalaTest.PkNsis", 0, &[b"Successfully upgraded", b"1.1.0"])?;
    c.ok(nsis_value_is("DisplayVersion", b"1.1.0"), 0, "DisplayVersion after the upgrade")?;
    let readme = read_file(local(join(&mut p, &[dir, "\\README.txt"])));
    show_file("README.txt", &readme);
    c.ok(readme.is_some_and(|o| o.has(b"PkNsis 1.1.0")), 0, "1.1.0's README")?;
    // Uninstall: UninstallString /S; NSIS's uninstaller runs from %TEMP%
    // and gala waits for the entry to go.
    c.gala("uninstall GalaTest.PkNsis", 0, &[b"Successfully uninstalled"])?;
    c.ok(log_has(b"uninstall.exe\" /S"), 0, "the uninstaller run with /S")?;
    c.ok(log_has(b"arp-entry-gone key=HKCU"), 0, "the entry's removal logged")?;
    c.ok(nsis_value("DisplayName").is_none(), 0, "the ARP entry removed")?;
    c.ok(!exists(local(join(&mut p, &[dir, "\\pknsis.exe"]))), 0, "the program removed")?;
    c.ok(!exists(local(join(&mut p, &[dir, "\\uninstall.exe"]))), 0, "the uninstaller removed")?;
    let o = c.gala("list", 0, &[])?;
    c.ok(!o.has(b"GalaTest.PkNsis"), 0, "not listed after the uninstall")?;
    // --location: /D=<dir>.
    let args = join(&mut a, &["install GalaTest.PkNsis --location C:\\PkNsisHere ", src]);
    c.gala(args, 0, &[b"Successfully installed", b"C:\\PkNsisHere"])?;
    c.ok(log_has(b"/S /D=C:\\PkNsisHere"), 0, "/D= last")?;
    c.ok(exists(W::new("C:\\PkNsisHere\\pknsis.exe")), 0, "installed where asked")?;
    c.gala("uninstall GalaTest.PkNsis", 0, &[b"Successfully uninstalled"])?;
    c.ok(!exists(W::new("C:\\PkNsisHere\\pknsis.exe")), 0, "removed from there")?;
    c.ok(nsis_value("DisplayName").is_none(), 0, "the ARP entry removed again")?;
    // Installed without gala (its installer run directly): winget list
    // and upgrade match the uninstall entry to the source's package by
    // its name and publisher.
    let (got, _) = run(Some("C:\\pk_source\\installers\\pknsis-1.0.0.exe"), "pknsis-1.0.0.exe /S");
    c.ok(got == 0, got, "PkNsis 1.0.0's installer run directly")?;
    c.ok(nsis_value("WinGetPackageIdentifier").is_none(), 0, "no gala tags")?;
    c.gala("list", 0, &[b"ARP\\User\\X64\\GalaTest.PkNsis"])?;
    c.gala(
        "list PkNsis --source C:\\pk_source",
        0,
        &[b"GalaTest.PkNsis", b"1.0.0", b"1.1.0", b"winget"],
    )?;
    c.gala(
        "upgrade --source C:\\pk_source",
        0,
        &[b"GalaTest.PkNsis", b"1.1.0", b"1 upgrades available"],
    )?;
    c.gala(
        "upgrade GalaTest.PkNsis --source C:\\pk_source",
        0,
        &[b"Successfully upgraded", b"1.1.0"],
    )?;
    c.ok(log_has(b"matched key=GalaTest.PkNsis by=NameAndPublisher"), 0, "matched by name")?;
    c.ok(nsis_value_is("DisplayVersion", b"1.1.0"), 0, "the matched program upgraded")?;
    c.ok(
        nsis_value_is("WinGetPackageIdentifier", b"GalaTest.PkNsis"),
        0,
        "tagged after the upgrade",
    )?;
    c.gala("uninstall GalaTest.PkNsis", 0, &[b"Successfully uninstalled"])?;
    c.ok(nsis_value("DisplayName").is_none(), 0, "the ARP entry removed at the end")?;
    Ok(())
}

// ---- Downloads over HTTPS (scenario pk_install_https) -----------------

/// Whether crypt32's "ROOT" system store has a certificate whose subject
/// has `name` (crypt32 loaded here: the other modes' images lack it).
fn root_has(name: &str) -> bool {
    type OpenSystemStore = unsafe extern "system" fn(usize, *const u16) -> Handle;
    type Find = unsafe extern "system" fn(Handle, u32, u32, u32, *const u16, Handle) -> Handle;
    type FreeContext = unsafe extern "system" fn(Handle) -> Bool;
    type CloseStore = unsafe extern "system" fn(Handle, u32) -> Bool;
    const X509_ASN_ENCODING: u32 = 1;
    const CERT_FIND_SUBJECT_STR_W: u32 = 0x0008_0007;
    let mut dll = W::new("crypt32.dll");
    let mut store_name = W::new("ROOT");
    let mut subject = W::new(name);
    // SAFETY: crypt32's exports with their documented signatures; our
    // NUL-terminated strings; the context and store are released.
    unsafe {
        let m = LoadLibraryW(dll.ptr());
        let f = |n: &[u8]| GetProcAddress(m, n.as_ptr());
        let procs = [
            f(b"CertOpenSystemStoreW\0"),
            f(b"CertFindCertificateInStore\0"),
            f(b"CertFreeCertificateContext\0"),
            f(b"CertCloseStore\0"),
        ];
        if m.is_null() || procs.iter().any(|p| p.is_null()) {
            return false;
        }
        let open = core::mem::transmute::<*mut c_void, OpenSystemStore>(procs[0]);
        let find = core::mem::transmute::<*mut c_void, Find>(procs[1]);
        let free = core::mem::transmute::<*mut c_void, FreeContext>(procs[2]);
        let close = core::mem::transmute::<*mut c_void, CloseStore>(procs[3]);
        let store = open(0, store_name.ptr());
        if store.is_null() {
            return false;
        }
        let cert = find(
            store,
            X509_ASN_ENCODING,
            0,
            CERT_FIND_SUBJECT_STR_W,
            subject.ptr(),
            core::ptr::null_mut(),
        );
        let found = !cert.is_null();
        if found {
            free(cert);
        }
        close(store, 0);
        found
    }
}

/// `gala install` of a package whose installer is not in the snapshot's
/// mirror: downloaded over HTTPS from schannel_test's Nginx in the guest,
/// whose release URL answers 302 with the file's URL (as GitHub's
/// release downloads do), through gala's winhttp (redirect followed) and
/// Schannel (the test CA trusted by the machine root store, beside the
/// public CA bundle gala ships), the SHA-256 checked, installed, run and
/// uninstalled; an untrusted certificate (Nginx's 8444) fails the install
/// with the reason (exit 4); github.com is not reachable from the VM (no
/// network card), which fails as a name not resolved.
fn https_test() -> Result<(), u32> {
    let mut c = Checks(0);
    let src = "--source C:\\pk_source";
    let mut a = [0u8; 300];
    // The machine root store: the public CA bundle (GitHub's roots among
    // them) and the test CA, as crypt32 reads it.
    c.ok(root_has("ISRG Root X1"), 0, "a public root (ISRG Root X1) in ROOT")?;
    c.ok(root_has("USERTrust ECC Certification Authority"), 0, "github.com's root in ROOT")?;
    c.ok(root_has("gala schannel test CA"), 0, "the test CA in ROOT")?;
    // Installed from the download; retried while Nginx starts (gala
    // cannot connect yet: error 12029).
    let args = join(&mut a, &["gala install GalaTest.PkHttps ", src]);
    let mut tries = 0;
    let (got, o) = loop {
        let (got, o) = run(Some("C:\\gala\\gala.exe"), args);
        if got == 4 && o.has(b"12029") && tries < 100 {
            tries += 1;
            // SAFETY: a plain call.
            unsafe { Sleep(200) };
            continue;
        }
        break (got, o);
    };
    let good = got == 0 && o.has(b"Successfully installed");
    if !good {
        out(b"pk_install_test: ");
        out(args.as_bytes());
        out(b" -> ");
        out(&hex(got));
        out(b":\r\n");
        out(o.text());
    }
    c.ok(good, got, "install GalaTest.PkHttps (downloaded)")?;
    c.ok(
        log_has(b"download url=https://localhost:8443/releases/download/v1.0.0/pkhttps-1.0.0.zip"),
        0,
        "the release URL asked for",
    )?;
    c.ok(
        log_has(b"fetch status=200 url=https://localhost:8443/objects/pkhttps-1.0.0.zip redirected-from="),
        0,
        "the redirect followed",
    )?;
    c.ok(log_has(b"sha256 expected="), 0, "the download's SHA-256 checked")?;
    c.ok(exists(local("\\Microsoft\\WinGet\\Links\\pkhttps.exe")), 0, "the command's shim")?;
    refresh_path();
    let (got, o) = run(None, "pkhttps");
    c.ok(got == 0 && o.has(HELLO), got, "pkhttps runs from PATH")?;
    c.gala("uninstall GalaTest.PkHttps", 0, &[b"Successfully uninstalled"])?;
    c.ok(!exists(local("\\Microsoft\\WinGet\\Links\\pkhttps.exe")), 0, "the command removed")?;
    // An untrusted certificate: the download fails, nothing is installed.
    let args = join(&mut a, &["install GalaTest.PkUntrusted ", src]);
    c.gala(args, 4, &[b"not trusted", b"12175"])?;
    c.ok(log_has(b"fetch error step=WinHttpSendRequest code=12175"), 0, "the failing call logged")?;
    c.ok(!exists(local("\\Microsoft\\WinGet\\Links\\pkuntrusted.exe")), 0, "nothing installed")?;
    // A real manifest's URL: github.com does not resolve in the VM.
    let args = join(&mut a, &["install jqlang.jq ", src]);
    c.gala(args, 4, &[b"downloading https://github.com/jqlang/jq/", b"12007"])?;

    // Manifests over HTTPS: a winget-pkgs tree Nginx serves as files,
    // each directory listed as JSON.
    let web = "--source https://localhost:8443/winget-pkgs";
    let args = join(&mut a, &["install GalaTest.PkHttps ", web]);
    c.gala(args, 0, &[b"Successfully installed"])?;
    c.ok(
        log_has(
            b"source list url=https://localhost:8443/winget-pkgs/manifests/g/GalaTest/PkHttps/",
        ),
        0,
        "the package's directory listed over HTTPS",
    )?;
    c.ok(
        log_has(b"source read url=https://localhost:8443/winget-pkgs/manifests/g/GalaTest/PkHttps/1.0.0/GalaTest.PkHttps.yaml"),
        0,
        "the manifest read over HTTPS",
    )?;
    let args = join(&mut a, &["search pkhttps ", web]);
    c.gala(args, 0, &[b"GalaTest.PkHttps", b"1.0.0"])?;
    // The ARP entry keeps the URL: upgrade reads it again.
    c.gala("upgrade GalaTest.PkHttps", 0, &[b"No newer version"])?;
    c.gala("uninstall GalaTest.PkHttps", 0, &[b"Successfully uninstalled"])?;
    // The default source, winget's community repository on GitHub,
    // listed through api.github.com: not reachable from the VM.
    c.gala(
        "install jqlang.jq",
        4,
        &[b"api.github.com/repos/microsoft/winget-pkgs/contents/manifests/j/jqlang/jq", b"12007"],
    )?;
    // Searching it needs winget's source index (not built: exit 3).
    c.gala("search jq", 3, &[b"source index"])?;
    Ok(())
}

/// Whether the command line has the argument `word` (after a space).
fn has_arg(word: &str) -> bool {
    // SAFETY: the process's NUL-terminated command line.
    unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        let line = core::slice::from_raw_parts(p, n);
        let w = word.as_bytes();
        line.windows(w.len() + 1)
            .any(|x| x[0] == 32 && x[1..].iter().zip(w).all(|(&c, &b)| c == u16::from(b)))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let result = if has_arg("nsis") {
        nsis_test()
    } else if has_arg("https") {
        https_test()
    } else {
        test()
    };
    let code = match result {
        Ok(()) => {
            out(b"pk_install_test: ok\r\n");
            0
        }
        Err(id) => {
            dump_log();
            id
        }
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
