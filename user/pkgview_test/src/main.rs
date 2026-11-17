//! A process's view of a Click-to-Run package's VFS tree (ROADMAP OF),
//! with the results of Windows.
//!
//! An Office Click-to-Run install keeps the files a program is to find
//! elsewhere on the machine under `<root>\root\VFS\<token>`, where the
//! token names the place: `ProgramFilesCommonX64` is
//! `C:\Program Files\Common Files`, `Windows` is `C:\Windows`, and so on
//! (App-V's tokens, which the package manifest writes every such path
//! with). Measured on Windows 11 with Office 16 installed:
//!
//! - `C:\Program Files\Common Files\Microsoft Shared\Office16` is on no
//!   machine-wide listing of `Common Files` and no process finds it,
//!   while a running WINWORD.EXE reports six of its own DLLs
//!   (`mso.dll`, `mso20win32client.dll`, ...) loaded from exactly that
//!   directory, and the files themselves are under the install's
//!   `root\VFS\ProgramFilesCommonX64\Microsoft Shared\Office16`. So the
//!   view belongs to the process, not to the machine.
//! - `HKLM\SOFTWARE\Microsoft\AppVISV` holds one value per package, its
//!   name the package's root (`c:\program files\microsoft office`) and
//!   its data the package's id. Office's own `AppVIsvSubsystems64.dll`
//!   reads it at start-up.
//! - None of the App-V services or drivers runs, and Word's modules from
//!   that directory are mapped at addresses outside the loader's usual
//!   range: on Windows the view is Office's own loader naming what it
//!   mapped, not a file system the kernel keeps. gala makes the same
//!   files answer at the same paths in the same processes, in the kernel
//!   (ARCHITECTURE "A package's VFS view"), which is what a program sees
//!   of it.
//!
//! What this test builds and checks, since no Office install is needed
//! for either: a package of its own at `C:\pkgview` with two tokens'
//! trees, registered the way Windows registers one, and the same
//! executable run from inside it.
//!
//! - A process whose image is outside the package finds none of it: the
//!   token paths are missing, as `Common Files\...\Office16` is for
//!   every process on Windows but Word's own.
//! - A process whose image is under the package root finds the
//!   package's files at the token's real path, in both trees, and a
//!   directory the package alone has can be listed there.
//! - A file the machine itself has keeps its own contents: the package
//!   answers where the machine has nothing. Windows never shows which
//!   way round this is -- Office asks for the files it ships by their
//!   real `root\Office16` path, and its VFS copies of the Visual C++
//!   runtimes sit beside identical local ones -- so this is gala's rule,
//!   and ARCHITECTURE says so.
//! - A name neither has is still missing, with the error of a missing
//!   file.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::{null, null_mut};

use ntrt as _;

type Handle = *mut c_void;
type Bool = i32;

const NAME: &str = "pkgview_test";

/// The package this test builds, as Office's install root is for Office.
const PKG: &str = "C:\\pkgview";
/// Its id, as the data of its `AppVISV` value (Office's is a GUID).
const PKG_ID: &str = "{pkgview-test}";

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const SHARE_ALL: u32 = 7;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const INVALID_HANDLE: isize = -1;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const HKEY_LOCAL_MACHINE: Handle = 0x8000_0002u32 as usize as Handle;
const KEY_WRITE: u32 = 0x2_0006;
const REG_SZ: u32 = 1;
const WAIT_OBJECT_0: u32 = 0;
/// How long the child may take. It copies nothing and opens six files;
/// a second is already far more than it needs.
const CHILD_WAIT_MS: u32 = 20_000;

/// STARTUPINFOW: only `cb` is set, the rest zero (no STARTF_* flags),
/// so the child takes the console and handles it inherits.
#[repr(C)]
struct StartupInfo {
    cb: u32,
    padding: u32,
    rest: [u64; 12],
}

#[repr(C)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    process_id: u32,
    thread_id: u32,
}

#[repr(C)]
struct FindData {
    attributes: u32,
    creation: [u32; 2],
    access: [u32; 2],
    write: [u32; 2],
    size_high: u32,
    size_low: u32,
    reserved0: u32,
    reserved1: u32,
    name: [u16; 260],
    alternate: [u16; 14],
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn GetLastError() -> u32;
    fn GetCommandLineA() -> *const u8;
    fn GetModuleFileNameW(module: Handle, buf: *mut u16, len: u32) -> u32;
    fn CreateDirectoryW(path: *const u16, security: *mut c_void) -> Bool;
    fn CopyFileW(from: *const u16, to: *const u16, fail_if_exists: Bool) -> Bool;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn FindFirstFileW(name: *const u16, data: *mut FindData) -> Handle;
    fn FindNextFileW(h: Handle, data: *mut FindData) -> Bool;
    fn FindClose(h: Handle) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn CreateProcessW(
        application: *const u16,
        command_line: *mut u16,
        process_security: *mut c_void,
        thread_security: *mut c_void,
        inherit: Bool,
        flags: u32,
        environment: *mut c_void,
        directory: *const u16,
        startup: *const StartupInfo,
        information: *mut ProcessInformation,
    ) -> Bool;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegCreateKeyExW(
        key: Handle,
        name: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        access: u32,
        security: *mut c_void,
        out: *mut Handle,
        disposition: *mut u32,
    ) -> u32;
    fn RegSetValueExW(
        key: Handle,
        name: *const u16,
        reserved: u32,
        kind: u32,
        data: *const u8,
        len: u32,
    ) -> u32;
    fn RegCloseKey(key: Handle) -> u32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn __chkstk();
}

struct Line {
    buf: [u8; 240],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 240], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(self, v: u64) -> Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        let mut v = v;
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.s(&d[i..])
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: a valid buffer and output pointer.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                null_mut(),
            );
        }
    }
}

/// A path's UTF-16 form, NUL-terminated (every path here is ASCII).
struct Wide {
    buf: [u16; 300],
}

impl Wide {
    fn of(parts: &[&str]) -> Self {
        let mut w = Wide { buf: [0; 300] };
        let mut n = 0;
        for part in parts {
            for b in part.bytes() {
                if n + 1 < w.buf.len() {
                    w.buf[n] = u16::from(b);
                    n += 1;
                }
            }
        }
        w.buf[n] = 0;
        w
    }

    fn ptr(&self) -> *const u16 {
        self.buf.as_ptr()
    }
}

fn fail(what: &[u8], detail: u64) -> u32 {
    Line::new().s(NAME.as_bytes()).s(b": ").s(what).s(b" (").dec(detail).s(b")\r\n").print();
    1
}

/// Creates `path` and every directory above it below `C:\`.
fn make_directories(path: &str) {
    let bytes = path.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\\' && i > 3 {
            let w = Wide::of(&[&path[..i]]);
            // SAFETY: a NUL-terminated path.
            unsafe { CreateDirectoryW(w.ptr(), null_mut()) };
        }
    }
    let w = Wide::of(&[path]);
    // SAFETY: a NUL-terminated path.
    unsafe { CreateDirectoryW(w.ptr(), null_mut()) };
}

fn write_file(path: &str, data: &[u8]) -> Result<(), u32> {
    let w = Wide::of(&[path]);
    // SAFETY: a NUL-terminated path and a valid buffer.
    let written = unsafe {
        let h = CreateFileW(
            w.ptr(),
            GENERIC_WRITE,
            SHARE_ALL,
            null_mut(),
            CREATE_ALWAYS,
            0,
            null_mut(),
        );
        if h as isize == INVALID_HANDLE {
            return Err(GetLastError());
        }
        let mut written = 0;
        let ok = WriteFile(h, data.as_ptr(), data.len() as u32, &mut written, null_mut());
        let e = GetLastError();
        CloseHandle(h);
        if ok == 0 {
            return Err(e);
        }
        written as usize
    };
    if written != data.len() { Err(0) } else { Ok(()) }
}

/// What opening `path` gives: the file's first bytes, or the Win32 error.
fn read_file(path: &str, buf: &mut [u8]) -> Result<usize, u32> {
    let w = Wide::of(&[path]);
    // SAFETY: a NUL-terminated path and a valid buffer.
    unsafe {
        let h =
            CreateFileW(w.ptr(), GENERIC_READ, SHARE_ALL, null_mut(), OPEN_EXISTING, 0, null_mut());
        if h as isize == INVALID_HANDLE {
            return Err(GetLastError());
        }
        let mut read = 0;
        let ok = ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut read, null_mut());
        let e = GetLastError();
        CloseHandle(h);
        if ok == 0 { Err(e) } else { Ok(read as usize) }
    }
}

/// `path` holds `want`.
fn holds(path: &str, want: &[u8], what: &[u8]) -> Result<(), u32> {
    let mut buf = [0u8; 64];
    match read_file(path, &mut buf) {
        Ok(n) if &buf[..n] == want => Ok(()),
        Ok(n) => {
            Line::new()
                .s(NAME.as_bytes())
                .s(b": ")
                .s(what)
                .s(b": holds ")
                .s(&buf[..n.min(32)])
                .s(b", not ")
                .s(want)
                .s(b"\r\n")
                .print();
            Err(1)
        }
        Err(e) => Err(fail(what, u64::from(e))),
    }
}

/// Opening `path` fails with the error of a missing file.
fn missing(path: &str, what: &[u8]) -> Result<(), u32> {
    let mut buf = [0u8; 8];
    match read_file(path, &mut buf) {
        Err(ERROR_FILE_NOT_FOUND) | Err(ERROR_PATH_NOT_FOUND) => Ok(()),
        Err(e) => Err(fail(what, u64::from(e))),
        Ok(_) => Err(fail(what, 0)),
    }
}

/// The names `mask` lists, joined by spaces.
fn listing(mask: &str, out: &mut [u8]) -> Result<usize, u32> {
    let w = Wide::of(&[mask]);
    let mut data = FindData {
        attributes: 0,
        creation: [0; 2],
        access: [0; 2],
        write: [0; 2],
        size_high: 0,
        size_low: 0,
        reserved0: 0,
        reserved1: 0,
        name: [0; 260],
        alternate: [0; 14],
    };
    let mut n = 0;
    // SAFETY: a NUL-terminated mask and a valid FindData.
    unsafe {
        let h = FindFirstFileW(w.ptr(), &mut data);
        if h as isize == INVALID_HANDLE {
            return Err(GetLastError());
        }
        loop {
            if n > 0 && n < out.len() {
                out[n] = b' ';
                n += 1;
            }
            for &c in &data.name {
                if c == 0 {
                    break;
                }
                if n < out.len() {
                    out[n] = c as u8;
                    n += 1;
                }
            }
            if FindNextFileW(h, &mut data) == 0 {
                break;
            }
        }
        FindClose(h);
    }
    Ok(n)
}

/// The token paths this test uses: a tree the machine has nothing of,
/// one under a second token, and one the machine has a file of.
const IN_PACKAGE: &str = "C:\\Program Files\\Common Files\\PkgView\\in-package.txt";
const BOTH: &str = "C:\\Program Files\\Common Files\\PkgView\\both.txt";
const IN_WINDOWS: &str = "C:\\Windows\\PkgView\\in-windows.txt";
const NEITHER: &str = "C:\\Program Files\\Common Files\\PkgView\\nowhere.txt";

/// The same files where the package keeps them.
const PKG_IN_PACKAGE: &str =
    "C:\\pkgview\\root\\VFS\\ProgramFilesCommonX64\\PkgView\\in-package.txt";
const PKG_BOTH: &str = "C:\\pkgview\\root\\VFS\\ProgramFilesCommonX64\\PkgView\\both.txt";
const PKG_IN_WINDOWS: &str = "C:\\pkgview\\root\\VFS\\Windows\\PkgView\\in-windows.txt";
/// Where the copy of this program runs from: inside the package.
const CHILD: &str = "C:\\pkgview\\root\\app\\pkgview_inside.exe";

/// The checks of a process whose image is under the package root.
fn child_checks() -> Result<(), u32> {
    holds(IN_PACKAGE, b"package", b"in the package, through the token path")?;
    holds(IN_WINDOWS, b"package", b"in the package, through the Windows token")?;
    // The machine's own file keeps its contents (gala's rule).
    holds(BOTH, b"local", b"the machine's own file")?;
    missing(NEITHER, b"a name in neither")?;
    // A directory the package alone has lists the package's files
    // (a `*` mask names `.` and `..` first, as Windows' own does).
    let mut out = [0u8; 128];
    let n =
        listing("C:\\Windows\\PkgView\\*", &mut out).map_err(|e| fail(b"listing", u64::from(e)))?;
    if &out[..n] != b". .. in-windows.txt" {
        Line::new()
            .s(NAME.as_bytes())
            .s(b": the package's directory lists ")
            .s(&out[..n])
            .s(b"\r\n")
            .print();
        return Err(1);
    }
    // A directory the machine has is its own: the view answers for a
    // path that is not there, not for what a directory holds.
    let n = listing("C:\\Program Files\\Common Files\\PkgView\\*", &mut out)
        .map_err(|e| fail(b"listing the machine's directory", u64::from(e)))?;
    if &out[..n] != b". .. both.txt" {
        Line::new()
            .s(NAME.as_bytes())
            .s(b": the machine's directory lists ")
            .s(&out[..n])
            .s(b"\r\n")
            .print();
        return Err(1);
    }
    Line::new().s(NAME.as_bytes()).s(b": in the package: ok\r\n").print();
    Ok(())
}

/// Builds the package and registers it as Windows registers one.
fn build_package() -> Result<(), u32> {
    make_directories("C:\\pkgview\\root\\VFS\\ProgramFilesCommonX64\\PkgView");
    make_directories("C:\\pkgview\\root\\VFS\\Windows\\PkgView");
    make_directories("C:\\pkgview\\root\\app");
    make_directories("C:\\Program Files\\Common Files\\PkgView");
    write_file(PKG_IN_PACKAGE, b"package")
        .map_err(|e| fail(b"writing the package's file", u64::from(e)))?;
    write_file(PKG_BOTH, b"package")
        .map_err(|e| fail(b"writing the package's copy", u64::from(e)))?;
    write_file(PKG_IN_WINDOWS, b"package")
        .map_err(|e| fail(b"writing the package's Windows file", u64::from(e)))?;
    write_file(BOTH, b"local").map_err(|e| fail(b"writing the machine's file", u64::from(e)))?;
    // The value Windows keeps per package: its name the root, its data
    // the package's id.
    let key_name = Wide::of(&["SOFTWARE\\Microsoft\\AppVISV"]);
    let value_name = Wide::of(&[PKG]);
    let value = Wide::of(&[PKG_ID]);
    let bytes = (PKG_ID.len() + 1) * 2;
    // SAFETY: NUL-terminated names and a valid out pointer.
    let e = unsafe {
        let mut key: Handle = null_mut();
        let mut disposition = 0;
        let e = RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            key_name.ptr(),
            0,
            null(),
            0,
            KEY_WRITE,
            null_mut(),
            &mut key,
            &mut disposition,
        );
        if e != 0 {
            return Err(fail(b"creating the AppVISV key", u64::from(e)));
        }
        let e = RegSetValueExW(
            key,
            value_name.ptr(),
            0,
            REG_SZ,
            value.ptr() as *const u8,
            bytes as u32,
        );
        RegCloseKey(key);
        e
    };
    if e != 0 {
        return Err(fail(b"registering the package", u64::from(e)));
    }
    Ok(())
}

/// Copies this program into the package and runs it there.
fn run_in_package() -> Result<(), u32> {
    let mut me = [0u16; 300];
    // SAFETY: a valid buffer.
    let n = unsafe { GetModuleFileNameW(null_mut(), me.as_mut_ptr(), me.len() as u32) };
    if n == 0 || n as usize >= me.len() {
        return Err(fail(b"GetModuleFileNameW", u64::from(n)));
    }
    let to = Wide::of(&[CHILD]);
    // SAFETY: NUL-terminated paths.
    let copied = unsafe { CopyFileW(me.as_ptr(), to.ptr(), 0) };
    if copied == 0 {
        // SAFETY: no pointers.
        let e = unsafe { GetLastError() };
        return Err(fail(b"copying the program into the package", u64::from(e)));
    }
    let mut command: [u16; 300] = Wide::of(&["\"", CHILD, "\" child"]).buf;
    let startup =
        StartupInfo { cb: core::mem::size_of::<StartupInfo>() as u32, padding: 0, rest: [0; 12] };
    let mut information =
        ProcessInformation { process: null_mut(), thread: null_mut(), process_id: 0, thread_id: 0 };
    // SAFETY: a NUL-terminated command line, a valid STARTUPINFO and
    // PROCESS_INFORMATION.
    let status = unsafe {
        let started = CreateProcessW(
            null(),
            command.as_mut_ptr(),
            null_mut(),
            null_mut(),
            1,
            0,
            null_mut(),
            null(),
            &startup,
            &mut information,
        );
        if started == 0 {
            let e = GetLastError();
            return Err(fail(b"starting the program in the package", u64::from(e)));
        }
        if WaitForSingleObject(information.process, CHILD_WAIT_MS) != WAIT_OBJECT_0 {
            return Err(fail(b"the program in the package did not end", 0));
        }
        let mut code = 0;
        GetExitCodeProcess(information.process, &mut code);
        CloseHandle(information.thread);
        CloseHandle(information.process);
        code
    };
    if status != 0 {
        return Err(fail(b"the program in the package failed", u64::from(status)));
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    build_package()?;
    // Outside the package nothing of it is there, as nothing of Office's
    // own view is there for any process but Word.
    missing(IN_PACKAGE, b"outside the package, through the token path")?;
    missing(IN_WINDOWS, b"outside the package, through the Windows token")?;
    holds(BOTH, b"local", b"outside the package, the machine's own file")?;
    run_in_package()?;
    Ok(())
}

/// Whether this process is the copy running inside the package.
fn in_package() -> bool {
    // SAFETY: the process's command line, NUL-terminated.
    let line = unsafe {
        let p = GetCommandLineA();
        let mut n = 0;
        while p.add(n).read_volatile() != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    };
    line.windows(6).any(|x| x == b" child")
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = if in_package() {
        match child_checks() {
            Ok(()) => 0,
            Err(n) => n,
        }
    } else {
        match run() {
            Ok(()) => {
                Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
                0
            }
            Err(n) => n,
        }
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
