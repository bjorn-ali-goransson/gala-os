//! The file explorer (ROADMAP EX): `\\wsl.localhost` paths for every
//! Windows program, shell32's namespace over gala's file systems, and
//! gala's `explorer.exe` browsing from C: into a WSL distribution.
//!
//! Expectations are Windows 11's with WSL installed (the paths' answers
//! are WSL's documented behaviour; the namespace's are Microsoft's
//! documentation and Wine's shell32 conformance tests, `shlfolder.c` and
//! `shellpath.c`, which pass on Windows: THIRD_PARTY.md). Where Windows'
//! answer is not known yet (the Linux node's names, FileStatLxInformation
//! over the 9P redirector) gala's answer is checked on gala and printed as
//! an `info` line on Windows. Without the `bb-rootfs.tar` beside this
//! program, or with a `bb` distribution already registered, the WSL parts
//! are skipped (`SKIP` lines), as wsl_test does.
//!
//! Part A, the plumbing (one file system, no 9P):
//! - the distribution `bb` is imported with `wsl --import`;
//! - `\\wsl.localhost\bb\etc\passwd` and `\\wsl$\bb\...` read the file
//!   the distribution has; the server name's case does not matter, the
//!   tree's does (Linux names: `ETC` is not `etc`);
//! - a Linux symbolic link is followed inside the distribution
//!   (`/bin/sh` -> `busybox`, an ELF file);
//! - FindFirstFileW lists the distribution's root;
//! - a file written from Windows through `\\wsl.localhost` is the file
//!   `cat` reads inside the distribution, and one written there is read
//!   here;
//! - an unknown distribution is ERROR_BAD_NET_NAME;
//! - GetFullPathNameW keeps the share as the root (`..` stops there) and
//!   a UNC current directory works for relative names;
//! - FileStatLxInformation shows the Linux mode, uid and gid.
//!
//! Part B, shell32's namespace: SHGetDesktopFolder and ParseDisplayName,
//! GetDisplayNameOf, SHBindToParent, EnumObjects, the item ID list
//! functions (ILClone, ILCombine, ILFindLastID, ILRemoveLastID,
//! ILIsEqual, SHGetPathFromIDListW), SHGetFolderLocation(CSIDL_DESKTOP)
//! (the empty list), SHGetKnownFolderIDList and SHGetKnownFolderItem,
//! SHCreateItemFromParsingName and IShellItem (GetDisplayName in its
//! forms, GetAttributes, GetParent, BindToHandler for the folder and its
//! items), "This PC" (`::{20D04FE0-...}`) with C:, its display name from
//! the volume's label, and the Linux node listing `bb`.
//!
//! Then (`explorer_test: ready`) the screen check (scenario `explorer`)
//! drives gala's explorer.exe; this program follows what it does: the
//! file copied from C: into the distribution (`explorer_test: copied`),
//! the copy opened with its association (this program's `open` mode
//! writes what it got: `explorer_test: opened`), and Explorer closed.
//!
//! Prints `explorer_test: ok`, or exits with the number of the failing
//! check.

#![no_std]
#![no_main]

extern crate alloc;

use gala_heap as _;

use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::fmt::Write;

type Bool = i32;
type Handle = *mut c_void;
type HResult = i32;
type Guid = [u8; 16];

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: Bool,
}

#[repr(C)]
struct FindData {
    attributes: u32,
    times: [u32; 6],
    size_high: u32,
    size_low: u32,
    reserved: [u32; 2],
    name: [u16; 260],
    alt: [u16; 14],
    extra: [u32; 2],
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetModuleFileNameW(module: Handle, name: *mut u16, size: u32) -> u32;
    fn GetModuleHandleA(name: *const u8) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn GetCommandLineW() -> *const u16;
    fn CreateDirectoryW(name: *const u16, sa: *mut c_void) -> Bool;
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
    fn GetTickCount() -> u32;
    fn GetLastError() -> u32;
    fn GetFileAttributesW(name: *const u16) -> u32;
    fn DeleteFileW(name: *const u16) -> Bool;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn FindFirstFileW(pattern: *const u16, data: *mut FindData) -> Handle;
    fn FindNextFileW(h: Handle, data: *mut FindData) -> Bool;
    fn FindClose(h: Handle) -> Bool;
    fn GetFullPathNameW(name: *const u16, size: u32, buf: *mut u16, part: *mut *mut u16) -> u32;
    fn SetCurrentDirectoryW(name: *const u16) -> Bool;
    fn GetCurrentDirectoryW(size: u32, buf: *mut u16) -> u32;
    fn GetWindowsDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn GetVolumeInformationW(
        root: *const u16,
        label: *mut u16,
        label_size: u32,
        serial: *mut u32,
        max_component: *mut u32,
        flags: *mut u32,
        fs_name: *mut u16,
        fs_name_size: u32,
    ) -> Bool;
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

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQueryInformationFile(
        h: Handle,
        iosb: *mut [u64; 2],
        buf: *mut u8,
        len: u32,
        class: u32,
    ) -> u32;
}

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    #[allow(clippy::too_many_arguments)]
    fn RegCreateKeyExW(
        key: usize,
        sub: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        access: u32,
        sa: *mut c_void,
        out: *mut usize,
        disposition: *mut u32,
    ) -> u32;
    fn RegSetValueExW(
        key: usize,
        name: *const u16,
        reserved: u32,
        kind: u32,
        data: *const u8,
        size: u32,
    ) -> u32;
    fn RegCloseKey(key: usize) -> u32;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> Handle;
}

#[link(name = "ole32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> HResult;
    fn CoTaskMemFree(p: *mut c_void);
}

#[link(name = "shell32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SHGetDesktopFolder(out: *mut *mut c_void) -> HResult;
    fn SHParseDisplayName(
        name: *const u16,
        bc: *mut c_void,
        out: *mut *mut u8,
        sfgao_in: u32,
        sfgao_out: *mut u32,
    ) -> HResult;
    fn SHGetPathFromIDListW(list: *const u8, path: *mut u16) -> Bool;
    fn SHGetFolderLocation(
        hwnd: Handle,
        csidl: i32,
        token: Handle,
        reserved: u32,
        out: *mut *mut u8,
    ) -> HResult;
    fn SHGetKnownFolderIDList(
        id: *const Guid,
        flags: u32,
        token: Handle,
        out: *mut *mut u8,
    ) -> HResult;
    fn SHGetKnownFolderItem(
        id: *const Guid,
        flags: u32,
        token: Handle,
        riid: *const Guid,
        out: *mut *mut c_void,
    ) -> HResult;
    fn SHCreateItemFromParsingName(
        name: *const u16,
        bc: *mut c_void,
        riid: *const Guid,
        out: *mut *mut c_void,
    ) -> HResult;
    fn SHBindToParent(
        list: *const u8,
        riid: *const Guid,
        out: *mut *mut c_void,
        last: *mut *const u8,
    ) -> HResult;
    fn ILClone(list: *const u8) -> *mut u8;
    fn ILCombine(a: *const u8, b: *const u8) -> *mut u8;
    fn ILFindLastID(list: *const u8) -> *mut u8;
    fn ILRemoveLastID(list: *mut u8) -> Bool;
    fn ILIsEqual(a: *const u8, b: *const u8) -> Bool;
    fn ILGetSize(list: *const u8) -> u32;
    fn ILFree(list: *mut u8);
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

const NAME: &str = "explorer_test";
const STARTF_USESTDHANDLES: u32 = 0x100;
const HANDLE_FLAG_INHERIT: u32 = 1;
const INVALID: u32 = u32::MAX;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
const CREATE_ALWAYS: u32 = 2;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_BAD_NET_NAME: u32 = 67;
const HKEY_CURRENT_USER: usize = 0x8000_0001;
const S_OK: HResult = 0;

const OLE: [u8; 8] = [0xc0, 0, 0, 0, 0, 0, 0, 0x46];
const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}
const IID_ISHELLFOLDER: Guid = guid(0x0002_14e6, 0, 0, OLE);
const IID_ISHELLITEM: Guid =
    guid(0x4382_6d1e, 0xe718, 0x42ee, [0xbc, 0x55, 0xa1, 0xe2, 0x61, 0xc3, 0x7b, 0xfe]);
const IID_IENUMSHELLITEMS: Guid =
    guid(0x7062_9033, 0xe363, 0x4a28, [0xa5, 0x67, 0x0d, 0xb7, 0x80, 0x06, 0xe6, 0xd7]);
const BHID_ENUMITEMS: Guid =
    guid(0x94f6_0519, 0x2850, 0x4924, [0xaa, 0x5a, 0xd1, 0x5e, 0x84, 0x86, 0x80, 0x39]);
const FOLDERID_WINDOWS: Guid =
    guid(0xf38b_f404, 0x1d43, 0x42f2, [0x93, 0x05, 0x67, 0xde, 0x0b, 0x28, 0xfc, 0x23]);

const SIGDN_NORMALDISPLAY: u32 = 0;
const SIGDN_PARENTRELATIVEPARSING: u32 = 0x8001_8001;
const SIGDN_DESKTOPABSOLUTEPARSING: u32 = 0x8002_8000;
const SIGDN_FILESYSPATH: u32 = 0x8005_8000;
const SHGDN_NORMAL: u32 = 0;
const SHGDN_INFOLDER: u32 = 1;
const SHGDN_FORPARSING: u32 = 0x8000;
const SFGAO_FOLDER: u32 = 0x2000_0000;
const SFGAO_FILESYSTEM: u32 = 0x4000_0000;
const SHCONTF_FOLDERS: u32 = 0x20;
const SHCONTF_NONFOLDERS: u32 = 0x40;
const CSIDL_DESKTOP: i32 = 0;

// ---- Output ----------------------------------------------------------------

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

fn line(text: &str) {
    let mut s = String::from(NAME);
    s.push_str(": ");
    s.push_str(text);
    s.push_str("\r\n");
    print(&s);
}

struct Checks {
    n: u32,
    gala: bool,
}

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, what: &str) -> Result<(), u32> {
        self.n += 1;
        if cond {
            return Ok(());
        }
        let mut s = String::new();
        let _ = write!(
            s,
            "check {} failed (line {}): {what}",
            self.n,
            core::panic::Location::caller().line()
        );
        line(&s);
        Err(self.n)
    }

    #[track_caller]
    fn eq(&mut self, got: &str, want: &str, what: &str) -> Result<(), u32> {
        let mut s = String::from(what);
        let _ = write!(s, ": got {got:?}, want {want:?}");
        self.ok(got == want, &s)
    }

    /// gala's answer where Windows' is not recorded yet: checked on gala,
    /// an `info` line on Windows.
    #[track_caller]
    fn gala_answer(&mut self, cond: bool, what: &str) -> Result<(), u32> {
        if self.gala {
            return self.ok(cond, what);
        }
        let mut s = String::from("info: ");
        s.push_str(what);
        s.push_str(if cond { ": as on gala" } else { ": differs from gala" });
        line(&s);
        Ok(())
    }
}

// ---- Text, files and processes ---------------------------------------------

fn wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

fn text(w: &[u16]) -> String {
    let n = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..n])
}

fn env(name: &str) -> Option<String> {
    let mut buf = [0u16; 512];
    let n = wide(name);
    // SAFETY: a valid buffer of its stated size.
    let len = unsafe { GetEnvironmentVariableW(n.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    (len > 0 && (len as usize) < buf.len()).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

/// This program's path.
fn me() -> String {
    let mut b = [0u16; 260];
    // SAFETY: our buffer.
    let n = unsafe { GetModuleFileNameW(core::ptr::null_mut(), b.as_mut_ptr(), 260) } as usize;
    String::from_utf16_lossy(&b[..n])
}

/// This program's directory, with its trailing backslash.
fn here() -> String {
    let path = me();
    match path.rfind('\\') {
        Some(i) => String::from(&path[..=i]),
        None => String::new(),
    }
}

fn attributes(path: &str) -> (u32, u32) {
    let p = wide(path);
    // SAFETY: a NUL-terminated path; plain calls.
    unsafe {
        let a = GetFileAttributesW(p.as_ptr());
        (a, if a == INVALID { GetLastError() } else { 0 })
    }
}

/// The whole file at `path`, or the error.
fn read_file(path: &str) -> Result<Vec<u8>, u32> {
    let p = wide(path);
    // SAFETY: a NUL-terminated path.
    let h = unsafe {
        CreateFileW(
            p.as_ptr(),
            GENERIC_READ,
            7,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        )
    };
    if h as isize == -1 {
        // SAFETY: plain call.
        return Err(unsafe { GetLastError() });
    }
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let mut got = 0;
        // SAFETY: a valid buffer and out pointer.
        let ok = unsafe { ReadFile(h, buf.as_mut_ptr(), 4096, &mut got, core::ptr::null_mut()) };
        if ok == 0 || got == 0 {
            break;
        }
        out.extend_from_slice(&buf[..got as usize]);
    }
    // SAFETY: our handle.
    unsafe { CloseHandle(h) };
    Ok(out)
}

fn write_file(path: &str, data: &[u8]) -> Result<(), u32> {
    let p = wide(path);
    // SAFETY: a NUL-terminated path.
    let h = unsafe {
        CreateFileW(
            p.as_ptr(),
            GENERIC_WRITE,
            0,
            core::ptr::null_mut(),
            CREATE_ALWAYS,
            0,
            core::ptr::null_mut(),
        )
    };
    if h as isize == -1 {
        // SAFETY: plain call.
        return Err(unsafe { GetLastError() });
    }
    let mut n = 0;
    // SAFETY: our buffer and handle.
    let ok =
        unsafe { WriteFile(h, data.as_ptr(), data.len() as u32, &mut n, core::ptr::null_mut()) };
    // SAFETY: our handle.
    unsafe { CloseHandle(h) };
    if ok == 0 { Err(1) } else { Ok(()) }
}

/// The names FindFirstFileW lists for `pattern`.
fn list(pattern: &str) -> Vec<String> {
    let p = wide(pattern);
    // SAFETY: zeroed plain data.
    let mut fd: FindData = unsafe { core::mem::zeroed() };
    // SAFETY: a NUL-terminated pattern and our buffer.
    let h = unsafe { FindFirstFileW(p.as_ptr(), &mut fd) };
    let mut out = Vec::new();
    if h as isize == -1 {
        return out;
    }
    loop {
        out.push(text(&fd.name));
        // SAFETY: our handle and buffer.
        if unsafe { FindNextFileW(h, &mut fd) } == 0 {
            break;
        }
    }
    // SAFETY: our handle.
    unsafe { FindClose(h) };
    out
}

fn full_path(name: &str) -> String {
    let p = wide(name);
    let mut b = [0u16; 520];
    // SAFETY: a NUL-terminated name and our buffer.
    let n = unsafe { GetFullPathNameW(p.as_ptr(), 520, b.as_mut_ptr(), core::ptr::null_mut()) };
    String::from_utf16_lossy(&b[..(n as usize).min(520)])
}

/// A pipe; (read, write), neither inherited.
fn pipe() -> (Handle, Handle) {
    let sa = SecurityAttributes {
        length: core::mem::size_of::<SecurityAttributes>() as u32,
        descriptor: core::ptr::null_mut(),
        inherit: 0,
    };
    let (mut r, mut w) = (core::ptr::null_mut(), core::ptr::null_mut());
    // SAFETY: valid out pointers.
    unsafe { CreatePipe(&mut r, &mut w, &sa, 65536) };
    (r, w)
}

/// Starts `exe` with `args`; with `stdio` its standard handles (made
/// inheritable for it). The process handle.
fn start(exe: &str, args: &str, stdio: Option<[Handle; 3]>) -> Option<Handle> {
    let mut cmd = String::from("\"");
    cmd.push_str(exe);
    cmd.push_str("\" ");
    cmd.push_str(args);
    let mut cmd = wide(&cmd);
    let app = wide(exe);
    let mut startup = [0u8; 104];
    startup[0] = 104;
    if let Some(handles) = stdio {
        startup[60..64].copy_from_slice(&STARTF_USESTDHANDLES.to_le_bytes());
        for (i, h) in handles.iter().enumerate() {
            // SAFETY: a handle this program holds.
            unsafe { SetHandleInformation(*h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
            startup[80 + 8 * i..88 + 8 * i].copy_from_slice(&(*h as u64).to_le_bytes());
        }
    }
    let mut info = [0usize; 3];
    let null = core::ptr::null_mut();
    // SAFETY: valid strings, STARTUPINFOW and PROCESS_INFORMATION.
    let ok = unsafe {
        CreateProcessW(
            app.as_ptr(),
            cmd.as_mut_ptr(),
            null,
            null,
            i32::from(stdio.is_some()),
            0,
            null,
            core::ptr::null(),
            startup.as_ptr(),
            &mut info,
        )
    };
    if ok == 0 {
        return None;
    }
    // SAFETY: the thread handle is not needed.
    unsafe { CloseHandle(info[1] as Handle) };
    Some(info[0] as Handle)
}

/// Runs `exe args` to its end: (exit code, standard output).
fn run(exe: &str, args: &str) -> (u32, Vec<u8>) {
    let (out_r, out_w) = pipe();
    // SAFETY: plain calls.
    let stdin = unsafe { GetStdHandle(-10i32 as u32) };
    let Some(h) = start(exe, args, Some([stdin, out_w, out_w])) else {
        return (0xdead_0000, Vec::new());
    };
    // SAFETY: our write end (the child has its own).
    unsafe { CloseHandle(out_w) };
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let mut got = 0;
        // SAFETY: a valid buffer and out pointer.
        let ok =
            unsafe { ReadFile(out_r, buf.as_mut_ptr(), 4096, &mut got, core::ptr::null_mut()) };
        if ok == 0 || got == 0 {
            break;
        }
        out.extend_from_slice(&buf[..got as usize]);
    }
    let mut code = 0xdead_0001;
    // SAFETY: handles this program holds.
    unsafe {
        if WaitForSingleObject(h, 120_000) == 0 {
            GetExitCodeProcess(h, &mut code);
        }
        CloseHandle(h);
        CloseHandle(out_r);
    }
    (code, out)
}

fn is_gala() -> bool {
    // SAFETY: plain calls with NUL-terminated names.
    unsafe {
        !GetProcAddress(
            GetModuleHandleA(c"ntdll.dll".as_ptr().cast()),
            c"GalaTasks".as_ptr().cast(),
        )
        .is_null()
    }
}

// ---- COM -----------------------------------------------------------------------

/// Method `slot` of interface `this`.
fn method<F: Copy>(this: *mut c_void, slot: usize) -> F {
    // SAFETY: a COM interface pointer: its first word is the vtable; the
    // caller names the method's signature.
    unsafe {
        let vtable = *(this as *const *const usize);
        core::mem::transmute_copy::<usize, F>(&*vtable.add(slot))
    }
}

fn release(this: *mut c_void) {
    if !this.is_null() {
        let f: extern "system" fn(*mut c_void) -> u32 = method(this, 2);
        f(this);
    }
}

/// A task-allocated string, freed.
fn take_string(p: *mut u16) -> String {
    if p.is_null() {
        return String::from("(null)");
    }
    // SAFETY: a NUL-terminated string the callee allocated.
    let s = unsafe {
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(core::slice::from_raw_parts(p, n))
    };
    // SAFETY: the callee's allocation.
    unsafe { CoTaskMemFree(p.cast()) };
    s
}

/// IShellItem::GetDisplayName.
fn item_name(item: *mut c_void, sigdn: u32) -> String {
    let f: extern "system" fn(*mut c_void, u32, *mut *mut u16) -> HResult = method(item, 5);
    let mut p = core::ptr::null_mut();
    if f(item, sigdn, &mut p) != S_OK {
        return String::from("(failed)");
    }
    take_string(p)
}

/// IShellItem::GetAttributes of `mask`.
fn item_attributes(item: *mut c_void, mask: u32) -> u32 {
    let f: extern "system" fn(*mut c_void, u32, *mut u32) -> HResult = method(item, 6);
    let mut a = 0;
    f(item, mask, &mut a);
    a
}

fn item_parent(item: *mut c_void) -> *mut c_void {
    let f: extern "system" fn(*mut c_void, *mut *mut c_void) -> HResult = method(item, 4);
    let mut p = core::ptr::null_mut();
    f(item, &mut p);
    p
}

fn item_from(name: &str) -> *mut c_void {
    let n = wide(name);
    let mut p = core::ptr::null_mut();
    // SAFETY: a NUL-terminated name and an out pointer.
    let hr = unsafe {
        SHCreateItemFromParsingName(n.as_ptr(), core::ptr::null_mut(), &IID_ISHELLITEM, &mut p)
    };
    if hr != S_OK { core::ptr::null_mut() } else { p }
}

/// The FILESYSPATH (else desktop-absolute) names of an item's children,
/// through BindToHandler(BHID_EnumItems).
fn children(item: *mut c_void) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bind: extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const Guid,
        *const Guid,
        *mut *mut c_void,
    ) -> HResult = method(item, 3);
    let mut e = core::ptr::null_mut();
    if bind(item, core::ptr::null_mut(), &BHID_ENUMITEMS, &IID_IENUMSHELLITEMS, &mut e) != S_OK {
        return out;
    }
    let next: extern "system" fn(*mut c_void, u32, *mut *mut c_void, *mut u32) -> HResult =
        method(e, 3);
    loop {
        let mut child = core::ptr::null_mut();
        let mut got = 0;
        if next(e, 1, &mut child, &mut got) != S_OK || got != 1 {
            break;
        }
        out.push((
            item_name(child, SIGDN_DESKTOPABSOLUTEPARSING),
            item_name(child, SIGDN_NORMALDISPLAY),
        ));
        release(child);
    }
    release(e);
    out
}

/// STRRET's text (STRRET_WSTR, STRRET_OFFSET into `list`, STRRET_CSTR).
fn strret(s: &[u8; 272], list: *const u8) -> String {
    match u32::from_le_bytes([s[0], s[1], s[2], s[3]]) {
        0 => take_string(usize::from_le_bytes(s[8..16].try_into().unwrap_or([0; 8])) as *mut u16),
        1 => {
            let off = u32::from_le_bytes([s[8], s[9], s[10], s[11]]) as usize;
            // SAFETY: an ANSI string inside the caller's list.
            unsafe {
                let p = list.add(off);
                let mut n = 0;
                while *p.add(n) != 0 {
                    n += 1;
                }
                String::from_utf8_lossy(core::slice::from_raw_parts(p, n)).into_owned()
            }
        }
        _ => {
            let n = s[8..].iter().position(|&b| b == 0).unwrap_or(0);
            String::from_utf8_lossy(&s[8..8 + n]).into_owned()
        }
    }
}

/// IShellFolder::GetDisplayNameOf.
fn folder_name(folder: *mut c_void, child: *const u8, flags: u32) -> String {
    let f: extern "system" fn(*mut c_void, *const u8, u32, *mut [u8; 272]) -> HResult =
        method(folder, 11);
    let mut s = [0u8; 272];
    if f(folder, child, flags, &mut s) != S_OK {
        return String::from("(failed)");
    }
    strret(&s, child)
}

fn parse(name: &str) -> *mut u8 {
    let n = wide(name);
    let mut list = core::ptr::null_mut();
    // SAFETY: a NUL-terminated name and an out pointer.
    let hr = unsafe {
        SHParseDisplayName(n.as_ptr(), core::ptr::null_mut(), &mut list, 0, core::ptr::null_mut())
    };
    if hr != S_OK { core::ptr::null_mut() } else { list }
}

fn path_of(list: *const u8) -> String {
    let mut b = [0u16; 260];
    // SAFETY: a list and a MAX_PATH buffer.
    if unsafe { SHGetPathFromIDListW(list, b.as_mut_ptr()) } == 0 {
        return String::from("(failed)");
    }
    text(&b)
}

// ---- The tests ------------------------------------------------------------------

/// Part A: `\\wsl.localhost` and `\\wsl$` for Windows programs.
fn plumbing(c: &mut Checks, wsl: &str) -> Result<(), u32> {
    let (a, _) = attributes("\\\\wsl.localhost\\bb\\etc");
    c.ok(
        a != INVALID && a & FILE_ATTRIBUTE_DIRECTORY != 0,
        "\\\\wsl.localhost\\bb\\etc a directory",
    )?;
    let passwd = read_file("\\\\wsl.localhost\\bb\\etc\\passwd");
    c.ok(passwd.as_ref().is_ok_and(|d| d.starts_with(b"root:x:0:0:")), "read /etc/passwd")?;
    let again = read_file("\\\\wsl$\\bb\\etc\\passwd");
    c.ok(again == passwd, "\\\\wsl$ reads the same file")?;
    let upper = read_file("\\\\WSL.LOCALHOST\\bb\\etc\\passwd");
    c.ok(upper == passwd, "the server's name in any case")?;
    let (a, e) = attributes("\\\\wsl.localhost\\bb\\ETC\\passwd");
    c.ok(a == INVALID && e == ERROR_PATH_NOT_FOUND, "Linux names keep their case")?;
    let sh = read_file("\\\\wsl.localhost\\bb\\bin\\sh");
    c.ok(sh.is_ok_and(|d| d.starts_with(b"\x7fELF")), "a Linux symbolic link followed")?;
    let root = list("\\\\wsl.localhost\\bb\\*");
    c.ok(root.iter().any(|n| n == "etc") && root.iter().any(|n| n == "tmp"), "the root listed")?;
    // Both ways through one file system.
    c.ok(
        write_file("\\\\wsl.localhost\\bb\\tmp\\from-windows.txt", b"hello from windows").is_ok(),
        "write",
    )?;
    let (code, out) = run(wsl, "-d bb --exec /bin/cat /tmp/from-windows.txt");
    c.ok(code == 0 && out == b"hello from windows", "cat reads what Windows wrote")?;
    let (code, _) = run(wsl, "-d bb -- echo from linux > /tmp/from-linux.txt");
    c.ok(code == 0, "a file written in the distribution")?;
    let got = read_file("\\\\wsl.localhost\\bb\\tmp\\from-linux.txt");
    c.ok(got.is_ok_and(|d| d == b"from linux\n"), "read here")?;
    let (a, e) = attributes("\\\\wsl.localhost\\nosuch\\etc");
    c.ok(a == INVALID && e == ERROR_BAD_NET_NAME, "an unknown distribution: ERROR_BAD_NET_NAME")?;
    // Paths: the share is the root.
    c.eq(
        &full_path("\\\\wsl.localhost\\bb\\etc\\..\\tmp"),
        "\\\\wsl.localhost\\bb\\tmp",
        "GetFullPathNameW",
    )?;
    c.eq(
        &full_path("//wsl.localhost/bb/etc/passwd"),
        "\\\\wsl.localhost\\bb\\etc\\passwd",
        "slashes",
    )?;
    let mut saved = [0u16; 260];
    // SAFETY: our buffer.
    unsafe { GetCurrentDirectoryW(260, saved.as_mut_ptr()) };
    let cd = wide("\\\\wsl.localhost\\bb\\etc");
    // SAFETY: a NUL-terminated path.
    c.ok(unsafe { SetCurrentDirectoryW(cd.as_ptr()) } != 0, "a UNC current directory")?;
    c.eq(&full_path("passwd"), "\\\\wsl.localhost\\bb\\etc\\passwd", "relative to it")?;
    c.ok(read_file("passwd") == passwd, "opened relative to it")?;
    // SAFETY: the saved directory, NUL-terminated.
    unsafe { SetCurrentDirectoryW(saved.as_ptr()) };
    // FileStatLxInformation (70): the Linux mode, uid and gid.
    let p = wide("\\\\wsl.localhost\\bb\\etc\\passwd");
    // SAFETY: a NUL-terminated path (FILE_READ_ATTRIBUTES).
    let h = unsafe {
        CreateFileW(
            p.as_ptr(),
            0x80,
            7,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        )
    };
    let mut info = [0u8; 96];
    let mut iosb = [0u64; 2];
    // SAFETY: our handle and buffer.
    let status = unsafe { NtQueryInformationFile(h, &mut iosb, info.as_mut_ptr(), 96, 70) };
    // SAFETY: our handle.
    unsafe { CloseHandle(h) };
    let at = |o: usize| u32::from_le_bytes([info[o], info[o + 1], info[o + 2], info[o + 3]]);
    let (flags, mode) = (at(72), at(84));
    let mut s = String::new();
    let _ = write!(
        s,
        "FileStatLxInformation: status {status:#x} flags {flags:#x} mode {mode:o} uid {}",
        at(76)
    );
    line(&s);
    c.gala_answer(
        status == 0 && flags & 7 == 7 && mode == 0o100644,
        "Linux mode 100644 in FileStatLxInformation",
    )?;
    Ok(())
}

/// Part B: shell32's namespace.
fn namespace(c: &mut Checks, wsl_ready: bool) -> Result<(), u32> {
    let mut windir = [0u16; 260];
    // SAFETY: our buffer.
    unsafe { GetWindowsDirectoryW(windir.as_mut_ptr(), 260) };
    let windir = text(&windir);
    let mut system32 = windir.clone();
    system32.push_str("\\System32");
    // The desktop folder and a parse below it.
    let mut desktop = core::ptr::null_mut();
    // SAFETY: an out pointer.
    c.ok(
        unsafe { SHGetDesktopFolder(&mut desktop) } == S_OK && !desktop.is_null(),
        "SHGetDesktopFolder",
    )?;
    let parse_fn: extern "system" fn(
        *mut c_void,
        Handle,
        *mut c_void,
        *const u16,
        *mut u32,
        *mut *mut u8,
        *mut u32,
    ) -> HResult = method(desktop, 3);
    let name = wide(&windir);
    let mut list = core::ptr::null_mut();
    let hr = parse_fn(
        desktop,
        core::ptr::null_mut(),
        core::ptr::null_mut(),
        name.as_ptr(),
        core::ptr::null_mut(),
        &mut list,
        core::ptr::null_mut(),
    );
    c.ok(hr == S_OK && !list.is_null(), "IShellFolder::ParseDisplayName")?;
    c.eq(&folder_name(desktop, list, SHGDN_FORPARSING), &windir, "GetDisplayNameOf(FORPARSING)")?;
    c.eq(&path_of(list), &windir, "SHGetPathFromIDListW")?;
    // SAFETY: our list.
    unsafe { ILFree(list) };
    // The item ID list functions.
    let sys = parse(&system32);
    c.ok(!sys.is_null(), "SHParseDisplayName")?;
    let win = parse(&windir);
    // SAFETY: lists shell32 made.
    unsafe {
        let copy = ILClone(sys);
        c.ok(ILIsEqual(copy, sys) != 0, "ILClone")?;
        c.ok(ILRemoveLastID(copy) != 0 && ILIsEqual(copy, win) != 0, "ILRemoveLastID")?;
        let last = ILFindLastID(sys);
        let joined = ILCombine(copy, last);
        c.ok(ILIsEqual(joined, sys) != 0, "ILCombine(parent, ILFindLastID)")?;
        c.ok(ILGetSize(joined) == ILGetSize(sys), "ILGetSize")?;
        ILFree(joined);
        ILFree(copy);
        // SHBindToParent: the folder and the last item.
        let mut parent = core::ptr::null_mut();
        let mut child = core::ptr::null();
        c.ok(
            SHBindToParent(sys, &IID_ISHELLFOLDER, &mut parent, &mut child) == S_OK,
            "SHBindToParent",
        )?;
        c.ok(core::ptr::eq(child, last), "SHBindToParent's last item")?;
        c.eq(
            &folder_name(parent, child, SHGDN_INFOLDER | SHGDN_FORPARSING),
            "System32",
            "INFOLDER|FORPARSING",
        )?;
        c.eq(&folder_name(parent, child, SHGDN_FORPARSING), &system32, "FORPARSING")?;
        // EnumObjects of C:\Windows finds System32 among its folders.
        let enum_fn: extern "system" fn(*mut c_void, Handle, u32, *mut *mut c_void) -> HResult =
            method(parent, 4);
        let mut e = core::ptr::null_mut();
        c.ok(
            enum_fn(parent, core::ptr::null_mut(), SHCONTF_FOLDERS | SHCONTF_NONFOLDERS, &mut e)
                == S_OK,
            "EnumObjects",
        )?;
        let next: extern "system" fn(*mut c_void, u32, *mut *mut u8, *mut u32) -> HResult =
            method(e, 3);
        let mut found = false;
        let mut n = 0;
        loop {
            let mut item = core::ptr::null_mut();
            let mut got = 0;
            if next(e, 1, &mut item, &mut got) != S_OK || got != 1 {
                break;
            }
            n += 1;
            found |= folder_name(parent, item, SHGDN_NORMAL | SHGDN_INFOLDER)
                .eq_ignore_ascii_case("System32");
            ILFree(item);
        }
        release(e);
        c.ok(found && n > 1, "EnumObjects lists System32")?;
        release(parent);
        ILFree(sys);
        ILFree(win);
        // The desktop's list is empty.
        let mut d = core::ptr::null_mut();
        c.ok(
            SHGetFolderLocation(
                core::ptr::null_mut(),
                CSIDL_DESKTOP,
                core::ptr::null_mut(),
                0,
                &mut d,
            ) == S_OK,
            "SHGetFolderLocation(CSIDL_DESKTOP)",
        )?;
        c.ok(ILGetSize(d) == 2, "the desktop: the empty list")?;
        ILFree(d);
        // Known folders.
        let mut k = core::ptr::null_mut();
        c.ok(
            SHGetKnownFolderIDList(&FOLDERID_WINDOWS, 0, core::ptr::null_mut(), &mut k) == S_OK,
            "SHGetKnownFolderIDList",
        )?;
        c.eq(&path_of(k), &windir, "FOLDERID_Windows' list")?;
        ILFree(k);
        let mut item = core::ptr::null_mut();
        c.ok(
            SHGetKnownFolderItem(
                &FOLDERID_WINDOWS,
                0,
                core::ptr::null_mut(),
                &IID_ISHELLITEM,
                &mut item,
            ) == S_OK,
            "SHGetKnownFolderItem",
        )?;
        c.eq(&item_name(item, SIGDN_FILESYSPATH), &windir, "its FILESYSPATH")?;
        release(item);
    }
    release(desktop);
    // IShellItem.
    let item = item_from(&system32);
    c.ok(!item.is_null(), "SHCreateItemFromParsingName")?;
    c.eq(&item_name(item, SIGDN_FILESYSPATH), &system32, "SIGDN_FILESYSPATH")?;
    c.eq(&item_name(item, SIGDN_NORMALDISPLAY), "System32", "SIGDN_NORMALDISPLAY")?;
    c.eq(&item_name(item, SIGDN_PARENTRELATIVEPARSING), "System32", "SIGDN_PARENTRELATIVEPARSING")?;
    let a = item_attributes(item, SFGAO_FOLDER | SFGAO_FILESYSTEM);
    c.ok(a == SFGAO_FOLDER | SFGAO_FILESYSTEM, "GetAttributes: a file-system folder")?;
    let parent = item_parent(item);
    c.eq(&item_name(parent, SIGDN_FILESYSPATH), &windir, "GetParent")?;
    release(parent);
    let kids = children(item);
    c.ok(
        kids.iter().any(|(p, _)| p.to_ascii_lowercase().ends_with("\\system32\\kernel32.dll")),
        "BHID_EnumItems lists kernel32.dll",
    )?;
    release(item);
    // This PC and its drives.
    let pc = item_from("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}");
    c.ok(!pc.is_null(), "This PC parsed")?;
    c.eq(
        &item_name(pc, SIGDN_DESKTOPABSOLUTEPARSING),
        "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
        "This PC's parsing name",
    )?;
    c.eq(&item_name(pc, SIGDN_NORMALDISPLAY), "This PC", "This PC's name")?;
    let mut label = [0u16; 64];
    let root = wide("C:\\");
    let null = core::ptr::null_mut();
    // SAFETY: our buffers.
    unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            label.as_mut_ptr(),
            64,
            null,
            null,
            null,
            core::ptr::null_mut(),
            0,
        )
    };
    let label = text(&label);
    let mut want = if label.is_empty() { String::from("Local Disk") } else { label };
    want.push_str(" (C:)");
    let drives = children(pc);
    let c_drive = drives.iter().find(|(p, _)| p == "C:\\");
    c.ok(c_drive.is_some(), "This PC lists C:\\")?;
    c.eq(&c_drive.map_or(String::new(), |(_, d)| d.clone()), &want, "C:'s name")?;
    release(pc);
    let drive = item_from("C:\\");
    c.eq(&item_name(drive, SIGDN_PARENTRELATIVEPARSING), "C:\\", "C:\\ within This PC")?;
    let up = item_parent(drive);
    c.eq(&item_name(up, SIGDN_NORMALDISPLAY), "This PC", "C:\\'s parent")?;
    release(up);
    release(drive);
    if !wsl_ready {
        line("SKIP: the Linux node (no bb distribution)");
        return Ok(());
    }
    // A distribution's tree.
    let etc = item_from("\\\\wsl.localhost\\bb\\etc");
    c.ok(!etc.is_null(), "\\\\wsl.localhost\\bb\\etc parsed")?;
    c.eq(&item_name(etc, SIGDN_FILESYSPATH), "\\\\wsl.localhost\\bb\\etc", "its FILESYSPATH")?;
    c.eq(&item_name(etc, SIGDN_NORMALDISPLAY), "etc", "its name")?;
    let a = item_attributes(etc, SFGAO_FOLDER | SFGAO_FILESYSTEM);
    c.ok(a == SFGAO_FOLDER | SFGAO_FILESYSTEM, "a file-system folder")?;
    let kids = children(etc);
    c.ok(kids.iter().any(|(p, _)| p == "\\\\wsl.localhost\\bb\\etc\\passwd"), "its files listed")?;
    let bb = item_parent(etc);
    let bb_name = item_name(bb, SIGDN_NORMALDISPLAY);
    let mut s = String::from("the distribution's name: ");
    s.push_str(&bb_name);
    line(&s);
    c.gala_answer(bb_name == "bb", "a distribution shown by its name")?;
    let linux = item_parent(bb);
    let linux_name = if linux.is_null() {
        String::from("(none)")
    } else {
        item_name(linux, SIGDN_NORMALDISPLAY)
    };
    c.gala_answer(linux_name == "Linux", "the distributions' parent: Linux")?;
    release(linux);
    release(bb);
    release(etc);
    let linux = item_from("::{B2B4A4D1-2754-4140-A2EB-9A76D9D7CDC6}");
    let distros = if linux.is_null() { Vec::new() } else { children(linux) };
    c.gala_answer(
        distros.iter().any(|(p, d)| p == "\\\\wsl.localhost\\bb" && d == "bb"),
        "Linux lists bb",
    )?;
    release(linux);
    Ok(())
}

/// Imports `bb` (None: skipped).
fn import(c: &mut Checks, wsl: &str) -> Result<bool, u32> {
    let tar = {
        let mut t = here();
        t.push_str("bb-rootfs.tar");
        t
    };
    if attributes(&tar).0 == INVALID {
        line("SKIP: no bb-rootfs.tar beside this program");
        return Ok(false);
    }
    if attributes("\\\\wsl.localhost\\bb\\etc").0 != INVALID {
        line("SKIP: a bb distribution is already registered");
        return Ok(false);
    }
    let dir = wide("C:\\explorer.d");
    // SAFETY: a valid path.
    unsafe { CreateDirectoryW(dir.as_ptr(), core::ptr::null_mut()) };
    let mut args = String::from("--import bb C:\\explorer.d\\bb ");
    args.push_str(&tar);
    let (code, _) = run(wsl, &args);
    c.ok(code == 0, "wsl --import bb")?;
    Ok(true)
}

/// Waits up to `ms` for `f` to hold, checking every 250 ms.
fn wait(ms: u32, mut f: impl FnMut() -> bool) -> bool {
    // SAFETY: plain calls.
    let start = unsafe { GetTickCount() };
    loop {
        if f() {
            return true;
        }
        // SAFETY: plain calls.
        if unsafe { GetTickCount() }.wrapping_sub(start) > ms {
            return false;
        }
        // SAFETY: plain call.
        unsafe { Sleep(250) };
    }
}

/// HKCU\Software\Classes\`key`'s default value `value`.
fn set_class(key: &str, value: &str) -> bool {
    let mut path = String::from("Software\\Classes\\");
    path.push_str(key);
    let p = wide(&path);
    let mut h = 0usize;
    let mut disposition = 0;
    // SAFETY: valid strings and out pointers; KEY_ALL_ACCESS.
    let r = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            p.as_ptr(),
            0,
            core::ptr::null(),
            0,
            0xf003f,
            core::ptr::null_mut(),
            &mut h,
            &mut disposition,
        )
    };
    if r != 0 {
        return false;
    }
    let v = wide(value);
    // SAFETY: our key; REG_SZ with its NUL.
    let r = unsafe {
        RegSetValueExW(h, core::ptr::null(), 0, 1, v.as_ptr().cast(), (2 * v.len()) as u32)
    };
    // SAFETY: our key.
    unsafe { RegCloseKey(h) };
    r == 0
}

const NOTES: &str = "C:\\explorer.d\\files\\notes.galatxt";
const COPY: &str = "\\\\wsl.localhost\\bb\\tmp\\notes.galatxt";
const OPENED: &str = "C:\\explorer.d\\opened.txt";
const NOTES_TEXT: &[u8] = b"notes from C:";

/// Part C, after `ready`: what the screen check does with explorer.exe,
/// seen from here.
fn screen(c: &mut Checks, wsl: &str) -> Result<(), u32> {
    // A file on C: and its association: this program's `open` mode.
    let dir = wide("C:\\explorer.d\\files");
    // SAFETY: a valid path.
    unsafe { CreateDirectoryW(dir.as_ptr(), core::ptr::null_mut()) };
    c.ok(write_file(NOTES, NOTES_TEXT).is_ok(), "the file to copy")?;
    let mut command = String::from("\"");
    command.push_str(&me());
    command.push_str("\" open \"%1\"");
    c.ok(set_class(".galatxt", "GalaTestNotes"), "the extension's class")?;
    c.ok(set_class("GalaTestNotes\\shell\\open\\command", &command), "its open command")?;
    // The desktop on the screen before any window (its start menu's
    // programs, `gala programs`'s action), so that Win+E reaches it.
    // SAFETY: plain calls with NUL-terminated names.
    let action = unsafe {
        GetProcAddress(
            GetModuleHandleA(c"ntdll.dll".as_ptr().cast()),
            c"GalaDesktopAction".as_ptr().cast(),
        )
    };
    if !action.is_null() {
        type Action = unsafe extern "system" fn(*const u8, u32, *mut u8, u32, *mut u32) -> u32;
        // SAFETY: GalaDesktopAction's signature.
        let action = unsafe { core::mem::transmute::<*mut c_void, Action>(action) };
        let mut request = [0u8; 40];
        request[0] = 1; // version
        request[4] = 11; // ACTION_PROGRAMS
        let mut listing = alloc::vec![0u8; 4096];
        let mut returned = 0;
        // SAFETY: our buffers with their lengths.
        let status =
            unsafe { action(request.as_ptr(), 40, listing.as_mut_ptr(), 4096, &mut returned) };
        c.ok(status == 0, "the desktop brought up (the start menu's programs)")?;
    }
    line("ready");
    // Explorer copies the file from C: into the distribution.
    let copied = wait(600_000, || read_file(COPY).is_ok_and(|d| d == NOTES_TEXT));
    c.ok(copied, "the copy in the distribution")?;
    line("copied");
    // ... and opens the copy with its association.
    let opened = wait(600_000, || read_file(OPENED).is_ok());
    let got = read_file(OPENED).unwrap_or_default();
    let mut want = Vec::from(COPY.as_bytes());
    want.push(b'\n');
    want.extend_from_slice(NOTES_TEXT);
    let mut s = String::from("opened ");
    s.push_str(&String::from_utf8_lossy(&got));
    line(&s.replace('\n', " | "));
    c.ok(opened && got == want, "the association opened the copy")?;
    // A file a Linux process makes appears in Explorer without a refresh
    // (its log says so: `explorer: changed ...`).
    let (code, _) = run(wsl, "-d bb -- echo live > /tmp/live.txt");
    c.ok(code == 0, "a file made in the distribution")?;
    line("linux wrote live.txt");
    // Explorer is closed last (Alt+F4).
    let class = wide("CabinetWClass");
    // SAFETY: a NUL-terminated class name.
    let closed =
        wait(600_000, || unsafe { FindWindowW(class.as_ptr(), core::ptr::null()) }.is_null());
    c.ok(closed, "Explorer closed")?;
    line("explorer closed");
    Ok(())
}

/// `open <path>`: what the association runs: writes the path and the
/// file's contents to OPENED.
fn open_mode(cmd: &str) -> u32 {
    let path = cmd.rsplit(" open ").next().unwrap_or("").trim().trim_matches('"');
    let data = read_file(path).unwrap_or_default();
    let mut out = Vec::from(path.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(&data);
    let mut s = String::from("open mode: ");
    s.push_str(path);
    line(&s);
    if write_file(OPENED, &out).is_ok() { 0 } else { 1 }
}

fn test() -> Result<(), u32> {
    let mut c = Checks { n: 0, gala: is_gala() };
    // SAFETY: plain call (COINIT_APARTMENTTHREADED).
    unsafe { CoInitializeEx(core::ptr::null_mut(), 2) };
    let root = env("SystemRoot").unwrap_or(String::from("C:\\Windows"));
    let mut wsl = root.clone();
    wsl.push_str("\\System32\\wsl.exe");
    let wsl_ready = import(&mut c, &wsl)?;
    if wsl_ready {
        plumbing(&mut c, &wsl)?;
        line("plumbing ok");
    }
    namespace(&mut c, wsl_ready)?;
    line("namespace ok");
    if wsl_ready && c.gala {
        // gala's explorer.exe, driven by the screen check.
        screen(&mut c, &wsl)?;
    }
    let _ = DeleteFileW;
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    // SAFETY: the process's command line.
    let cmd = unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(core::slice::from_raw_parts(p, n))
    };
    if cmd.contains("\" open ") {
        // SAFETY: ends the process.
        unsafe { ExitProcess(open_mode(&cmd)) };
    }
    let code = match test() {
        Ok(()) => {
            line("ok");
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    line("panic");
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
