//! The Known Folder Manager (shell32.dll's CLSID_KnownFolderManager,
//! IKnownFolderManager and IKnownFolder), as App-V's ConfigurePackage
//! uses it while the Click-to-Run service installs Office (ROADMAP OF),
//! and as Windows answers it. Expected values are those of Wine's
//! conformance tests (dlls/shell32/tests/shellpath.c, test_knownFolders
//! and check_known_folder, run against Windows) and of real Windows on
//! CI's runner (this program's run there):
//! - CoCreateInstance(CLSID_KnownFolderManager) gives
//!   IKnownFolderManager, which answers QueryInterface for IUnknown and
//!   not for IMarshal (E_NOINTERFACE); so do its folders;
//! - FolderIdFromCsidl(CSIDL_WINDOWS) is FOLDERID_Windows and
//!   FolderIdToCsidl(FOLDERID_Windows) CSIDL_WINDOWS;
//! - the Windows folder: category KF_CATEGORY_FIXED, its id, GetPath the
//!   Windows directory, no redirection capabilities, SetPath refused
//!   (E_INVALIDARG), its definition named "Windows" with attributes 0;
//!   releasing a folder or the manager gives 0 (each is its own object);
//! - GetFolderByName ignores case; an unknown name is
//!   HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND) and NULL;
//! - GetFolderIds lists every folder (in CoTaskMemAlloc'd memory); each
//!   one's GetFolder gives a folder with that id and its definition's
//!   category; for the folders of `FOLDERS` (Wine's table, the Windows 8
//!   and later alternatives where it has two): the CSIDL, name,
//!   category, parent, relative path, parsing name, attributes and
//!   flags (a subset of those listed), GetPath equal to
//!   SHGetKnownFolderPath and to SHGetFolderPathW of the CSIDL, and a
//!   folder with a relative path under its parent (the parent's path,
//!   a backslash, the relative path);
//! - the folders the environment names (the profile, AppData, Local
//!   AppData, ProgramData, Program Files, Public) and the Windows and
//!   system directories are where the environment and kernel32 say;
//! - FindFolderFromPath (the documentation's FFFP_MODE): an exact path
//!   gives its folder, case aside; a path below one gives the nearest
//!   folder above it with FFFP_NEARESTPARENTMATCH and fails with
//!   FFFP_EXACTMATCH;
//! - an unknown id or CSIDL fails (the codes Windows gives are `info`
//!   lines);
//! - SHCreateDirectory, imported by its ordinal 165 as App-V's
//!   AppVCatalog.dll imports it, and SHCreateDirectoryExW
//!   (`create_directory`).
//!
//! The same source builds `wow64_knownfolder_test`, the 32-bit (x86)
//! program (FOLDERID_ProgramFilesX64 and FOLDERID_ProgramFilesCommonX64
//! are 64-bit only). Prints `knownfolder_test: ok`
//! (`wow64_knownfolder_test: ok`), or `<name>: check N got X (line L)`
//! and exits with N.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::ptr::null_mut;

use ntrt as _;

#[cfg(target_arch = "x86_64")]
const NAME: &str = "knownfolder_test";
#[cfg(target_arch = "x86")]
const NAME: &str = "wow64_knownfolder_test";

type Handle = usize;
type HResult = i32;
type Guid = [u8; 16];
type Obj = *mut c_void;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn GetWindowsDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn RemoveDirectoryW(path: *const u16) -> i32;
    fn DeleteFileW(path: *const u16) -> i32;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: usize,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CloseHandle(h: Handle) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ole32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ole32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> HResult;
    fn CoCreateInstance(
        clsid: *const Guid,
        outer: Obj,
        context: u32,
        iid: *const Guid,
        out: *mut Obj,
    ) -> HResult;
    fn CoTaskMemFree(p: *mut c_void);
    fn CoUninitialize();
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "shell32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "shell32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn SHGetKnownFolderPath(
        id: *const Guid,
        flags: u32,
        token: Handle,
        path: *mut *mut u16,
    ) -> HResult;
    fn SHGetFolderPathW(
        window: Handle,
        csidl: i32,
        token: Handle,
        flags: u32,
        path: *mut u16,
    ) -> HResult;
    fn SHCreateDirectoryExW(window: Handle, path: *const u16, security: usize) -> i32;
    // By ordinal, as App-V's AppVCatalog.dll imports it (shell32 has no
    // name for it): SHCreateDirectory.
    #[link_ordinal(165)]
    fn SHCreateDirectory(window: Handle, path: *const u16) -> i32;
}

const S_OK: HResult = 0;
const E_INVALIDARG: HResult = 0x8007_0057u32 as i32;
const E_NOINTERFACE: HResult = 0x8000_4002u32 as i32;
/// HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND).
const E_FILE_NOT_FOUND: HResult = 0x8007_0002u32 as i32;
const COINIT_APARTMENTTHREADED: u32 = 2;
const CLSCTX_INPROC_SERVER: u32 = 1;
const CSIDL_WINDOWS: i32 = 0x24;
/// CSIDL_FLAG_DONT_VERIFY: the path whether or not the folder exists.
const CSIDL_FLAG_DONT_VERIFY: i32 = 0x4000;
/// KF_FLAG_DONT_VERIFY: the same for known folders.
const KF_FLAG_DONT_VERIFY: u32 = 0x4000;
const KF_CATEGORY_VIRTUAL: u32 = 1;
const KF_CATEGORY_FIXED: u32 = 2;
const FFFP_EXACTMATCH: u32 = 0;
const FFFP_NEARESTPARENTMATCH: u32 = 1;
const MAX_PATH: usize = 260;

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Guid {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

const GUID_NULL: Guid = [0; 16];
const CLSID_KNOWNFOLDERMANAGER: Guid =
    guid(0x4df0_c730, 0xdf9d, 0x4ae3, [0x91, 0x53, 0xaa, 0x6b, 0x82, 0xe9, 0x79, 0x5a]);
const IID_IKNOWNFOLDERMANAGER: Guid =
    guid(0x8be2_d872, 0x86aa, 0x4d47, [0xb7, 0x76, 0x32, 0xcc, 0xa4, 0x0c, 0x70, 0x18]);
const IID_IKNOWNFOLDER: Guid =
    guid(0x3aa7_af7e, 0x9b36, 0x420c, [0xa8, 0xe3, 0xf7, 0x7d, 0x46, 0x74, 0xa4, 0x88]);
const IID_IUNKNOWN: Guid = guid(0, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IMARSHAL: Guid = guid(0x0000_0003, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
const FOLDERID_WINDOWS: Guid =
    guid(0xf38b_f404, 0x1d43, 0x42f2, [0x93, 0x05, 0x67, 0xde, 0x0b, 0x28, 0xfc, 0x23]);
const FOLDERID_SYSTEM: Guid =
    guid(0x1ac1_4e77, 0x02e7, 0x4e5d, [0xb7, 0x44, 0x2e, 0xb1, 0xae, 0x51, 0x98, 0xb7]);
const FOLDERID_PROFILE: Guid =
    guid(0x5e6c_858f, 0x0e22, 0x4760, [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73]);
const FOLDERID_ROAMINGAPPDATA: Guid =
    guid(0x3eb6_85db, 0x65f9, 0x4cf6, [0xa0, 0x3a, 0xe3, 0xef, 0x65, 0x72, 0x9f, 0x3d]);
const FOLDERID_LOCALAPPDATA: Guid =
    guid(0xf1b3_2785, 0x6fba, 0x4fcf, [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91]);
const FOLDERID_PROGRAMDATA: Guid =
    guid(0x62ab_5d82, 0xfdc1, 0x4dc3, [0xa9, 0xdd, 0x07, 0x0d, 0x1d, 0x49, 0x5d, 0x97]);
const FOLDERID_PROGRAMFILES: Guid =
    guid(0x905e_63b6, 0xc1bf, 0x494e, [0xb2, 0x9c, 0x65, 0xb7, 0x32, 0xd3, 0xd2, 0x1a]);
const FOLDERID_PROGRAMFILESX86: Guid =
    guid(0x7c5a_40ef, 0xa0fb, 0x4bfc, [0x87, 0x4a, 0xc0, 0xf2, 0xe0, 0xb9, 0xfa, 0x8e]);
const FOLDERID_PUBLIC: Guid =
    guid(0xdfdf_76a2, 0xc82a, 0x4d63, [0x90, 0x6a, 0x56, 0x44, 0xac, 0x45, 0x73, 0x85]);
const FOLDERID_DOWNLOADS: Guid =
    guid(0x374d_e290, 0x123f, 0x4565, [0x91, 0x64, 0x39, 0xc4, 0x92, 0x5e, 0x46, 0x7b]);
/// No folder has this id.
const FOLDERID_NONE: Guid =
    guid(0x6a1f_0e3c, 0x5b2d, 0x4c11, [0x9e, 0x47, 0x0d, 0x2a, 0x61, 0x7b, 0x88, 0x14]);

// IUnknown
const QUERY_INTERFACE: usize = 0;
const RELEASE: usize = 2;
// IKnownFolderManager
const FOLDER_ID_FROM_CSIDL: usize = 3;
const FOLDER_ID_TO_CSIDL: usize = 4;
const GET_FOLDER_IDS: usize = 5;
const GET_FOLDER: usize = 6;
const GET_FOLDER_BY_NAME: usize = 7;
const FIND_FOLDER_FROM_PATH: usize = 10;
// IKnownFolder
const GET_ID: usize = 3;
const GET_CATEGORY: usize = 4;
const GET_PATH: usize = 6;
const SET_PATH: usize = 7;
const GET_REDIRECTION_CAPABILITIES: usize = 10;
const GET_FOLDER_DEFINITION: usize = 11;

/// KNOWNFOLDER_DEFINITION (shobjidl.h); its strings are CoTaskMemAlloc'd.
#[repr(C)]
struct Definition {
    category: u32,
    name: *mut u16,
    description: *mut u16,
    parent: Guid,
    relative: *mut u16,
    parsing: *mut u16,
    tooltip: *mut u16,
    localized: *mut u16,
    icon: *mut u16,
    security: *mut u16,
    attributes: u32,
    flags: u32,
    folder_type: Guid,
}

impl Definition {
    fn new() -> Self {
        // SAFETY: integers, GUIDs and pointers, all valid as zeros.
        unsafe { core::mem::zeroed() }
    }

    /// FreeKnownFolderDefinitionFields.
    fn free(&mut self) {
        for p in [
            self.name,
            self.description,
            self.relative,
            self.parsing,
            self.tooltip,
            self.localized,
            self.icon,
            self.security,
        ] {
            // SAFETY: CoTaskMemAlloc'd strings (or NULL).
            unsafe { CoTaskMemFree(p.cast()) };
        }
        *self = Definition::new();
    }
}

/// Calls slot `slot` of `obj`'s vtable as `$ty`.
macro_rules! call {
    ($obj:expr, $slot:expr, fn($($t:ty),*) $(-> $r:ty)?, $($arg:expr),*) => {{
        let obj: Obj = $obj;
        // SAFETY: a live COM object whose vtable has this slot with this
        // signature (shobjidl.h).
        unsafe {
            let vtable = *(obj as *const *const usize);
            let f: extern "system" fn(Obj $(, $t)*) $(-> $r)? =
                core::mem::transmute(*vtable.add($slot));
            f(obj $(, $arg)*)
        }
    }};
}

fn release(obj: Obj) -> u32 {
    call!(obj, RELEASE, fn() -> u32,)
}

fn query(obj: Obj, iid: &Guid) -> (HResult, Obj) {
    let mut out = null_mut();
    let hr = call!(obj, QUERY_INTERFACE, fn(*const Guid, *mut Obj) -> HResult, iid, &mut out);
    (hr, out)
}

fn get_folder(manager: Obj, id: &Guid) -> (HResult, Obj) {
    let mut out: Obj = core::ptr::dangling_mut();
    let hr = call!(manager, GET_FOLDER, fn(*const Guid, *mut Obj) -> HResult, id, &mut out);
    (hr, out)
}

fn folder_id(folder: Obj) -> (HResult, Guid) {
    let mut id = [0xcc; 16];
    let hr = call!(folder, GET_ID, fn(*mut Guid) -> HResult, &mut id);
    (hr, id)
}

/// A folder's path (CoTaskMemAlloc'd; NULL when it fails).
fn folder_path(folder: Obj, flags: u32) -> (HResult, *mut u16) {
    let mut p = null_mut();
    let hr = call!(folder, GET_PATH, fn(u32, *mut *mut u16) -> HResult, flags, &mut p);
    (hr, p)
}

/// `s` as a NUL-terminated wide string (ASCII).
fn wide(s: &str) -> [u16; MAX_PATH] {
    let mut w = [0u16; MAX_PATH];
    for (d, c) in w.iter_mut().zip(s.encode_utf16()) {
        *d = c;
    }
    w
}

fn wlen(s: *const u16) -> usize {
    let mut n = 0;
    // SAFETY: a NUL-terminated string, read up to its NUL.
    while unsafe { *s.add(n) } != 0 {
        n += 1;
    }
    n
}

fn upper(c: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
}

/// Whether two NUL-terminated wide strings are equal, ASCII case aside
/// (NULL equals nothing).
fn same_ci(a: *const u16, b: *const u16) -> bool {
    if a.is_null() || b.is_null() {
        return false;
    }
    let (n, m) = (wlen(a), wlen(b));
    // SAFETY: both strings hold n (m) units.
    n == m && (0..n).all(|i| unsafe { upper(*a.add(i)) == upper(*b.add(i)) })
}

/// Whether the NUL-terminated wide string `s` is `want` exactly.
fn is(s: *const u16, want: &str) -> bool {
    if s.is_null() {
        return false;
    }
    let n = wlen(s);
    n == want.len()
        // SAFETY: n units.
        && want.bytes().enumerate().all(|(i, b)| unsafe { *s.add(i) } == u16::from(b))
}

/// Whether `s` is one of `alternatives` (NULL when there are none).
fn is_one_of(s: *const u16, alternatives: &[&str]) -> bool {
    if alternatives.is_empty() { s.is_null() } else { alternatives.iter().any(|a| is(s, a)) }
}

/// `path` is `parent`, a backslash and `relative`, case aside.
fn under(path: *const u16, parent: *const u16, relative: *const u16) -> bool {
    if path.is_null() || parent.is_null() || relative.is_null() {
        return false;
    }
    let mut joined = [0u16; 2 * MAX_PATH];
    let (n, r) = (wlen(parent), wlen(relative));
    if n + 1 + r >= joined.len() {
        return false;
    }
    // SAFETY: n and r units of the two strings, into a buffer that holds both.
    unsafe {
        core::ptr::copy_nonoverlapping(parent, joined.as_mut_ptr(), n);
        joined[n] = u16::from(b'\\');
        core::ptr::copy_nonoverlapping(relative, joined.as_mut_ptr().add(n + 1), r);
    }
    same_ci(path, joined.as_ptr())
}

fn free(p: *mut u16) {
    // SAFETY: CoTaskMemAlloc'd (or NULL).
    unsafe { CoTaskMemFree(p.cast()) };
}

struct Line {
    buf: [u8; 300],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 300], n: 0 }
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

    fn w(mut self, s: *const u16) -> Self {
        if s.is_null() {
            return self.s(b"(null)");
        }
        for i in 0..wlen(s) {
            // SAFETY: inside the string.
            let u = unsafe { *s.add(i) };
            self = self.s(&[if (0x20..0x7f).contains(&u) { u as u8 } else { b'?' }]);
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

    fn hex(self, v: u64) -> Self {
        let mut d = [0u8; 18];
        d[0] = b'0';
        d[1] = b'x';
        for i in 0..16 {
            d[2 + i] = b"0123456789abcdef"[((v >> (60 - 4 * i)) & 0xf) as usize];
        }
        self.s(&d)
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: our buffer; the standard output handle.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            )
        };
    }
}

fn info() -> Line {
    Line::new().s(NAME.as_bytes()).s(b": info ")
}

/// The check counter, and the first check that `later` found wrong (0
/// when none): such a check records its mismatch and goes on, so one run
/// on Windows reports every value that differs.
struct Checks(u32, u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got)
            .s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn hr(&mut self, got: HResult, want: HResult) -> Result<(), u32> {
        self.ok(got == want, got as u32 as u64)
    }

    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// An error code whose mismatch does not stop the test.
    #[track_caller]
    fn code(&mut self, got: HResult, want: HResult) {
        self.later(got == want, got as u32 as u64, Some(want as u32 as u64), "");
    }

    /// A condition about folder `what` whose failure does not stop the
    /// test: it is printed, and the test fails at its end with the first
    /// such check's number.
    #[track_caller]
    fn note(&mut self, cond: bool, got: u64, what: &str) {
        self.later(cond, got, None, what);
    }

    #[track_caller]
    fn later(&mut self, cond: bool, got: u64, want: Option<u64>, what: &str) {
        self.0 += 1;
        if cond {
            return;
        }
        let mut line = Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(u64::from(self.0))
            .s(b" got ")
            .hex(got);
        if let Some(w) = want {
            line = line.s(b" want ").hex(w);
        }
        if !what.is_empty() {
            line = line.s(b" for ").s(what.as_bytes());
        }
        line.s(b" (line ")
            .dec(u64::from(core::panic::Location::caller().line()))
            .s(b", continuing)\r\n")
            .print();
        if self.1 == 0 {
            self.1 = self.0;
        }
    }
}

/// A known folder as Wine's table (shellpath.c's known_folders) has it:
/// the alternatives it lists for its parent, relative path and parsing
/// name (Windows 8 and later have the second); an empty list is GUID_NULL
/// or NULL.
struct Kf {
    id: Guid,
    what: &'static str,
    /// -1 for a folder without a CSIDL.
    csidl: i32,
    name: &'static str,
    category: u32,
    parents: &'static [Guid],
    relative: &'static [&'static str],
    parsing: &'static [&'static str],
    attributes: u32,
    /// The KFDF_ flags the definition may have.
    flags: u32,
}

mod folders;
use folders::FOLDERS;

/// The Wine test's checks of the Windows folder and of names.
fn windows_folder(c: &mut Checks, manager: Obj) -> Result<(), u32> {
    let mut id = [0u8; 16];
    let hr =
        call!(manager, FOLDER_ID_FROM_CSIDL, fn(i32, *mut Guid) -> HResult, CSIDL_WINDOWS, &mut id);
    c.hr(hr, S_OK)?;
    c.ok(id == FOLDERID_WINDOWS, u64::from(id[0]))?;
    let mut csidl = -1;
    let hr = call!(
        manager,
        FOLDER_ID_TO_CSIDL,
        fn(*const Guid, *mut i32) -> HResult,
        &FOLDERID_WINDOWS,
        &mut csidl
    );
    c.hr(hr, S_OK)?;
    c.eq(csidl as u64, CSIDL_WINDOWS as u64)?;

    let (hr, folder) = get_folder(manager, &FOLDERID_WINDOWS);
    c.hr(hr, S_OK)?;
    c.hr(query(folder, &IID_IMARSHAL).0, E_NOINTERFACE)?;
    let (hr, unknown) = query(folder, &IID_IKNOWNFOLDER);
    c.hr(hr, S_OK)?;
    c.ok(unknown == folder, 0)?;
    release(unknown);
    let mut category = 0;
    c.hr(call!(folder, GET_CATEGORY, fn(*mut u32) -> HResult, &mut category), S_OK)?;
    c.eq(category.into(), KF_CATEGORY_FIXED.into())?;
    let (hr, id) = folder_id(folder);
    c.hr(hr, S_OK)?;
    c.ok(id == FOLDERID_WINDOWS, u64::from(id[0]))?;
    let mut windows = [0u16; MAX_PATH];
    // SAFETY: a MAX_PATH buffer.
    unsafe { GetWindowsDirectoryW(windows.as_mut_ptr(), MAX_PATH as u32) };
    let (hr, path) = folder_path(folder, 0);
    c.hr(hr, S_OK)?;
    let same = same_ci(path, windows.as_ptr());
    free(path);
    c.ok(same, 0)?;
    let mut capabilities = 1u32;
    let hr =
        call!(folder, GET_REDIRECTION_CAPABILITIES, fn(*mut u32) -> HResult, &mut capabilities);
    c.hr(hr, S_OK)?;
    c.note(capabilities == 0, capabilities.into(), "Windows");
    let hr = call!(folder, SET_PATH, fn(u32, *const u16) -> HResult, 0, windows.as_ptr());
    c.code(hr, E_INVALIDARG);
    let mut def = Definition::new();
    let hr = call!(folder, GET_FOLDER_DEFINITION, fn(*mut Definition) -> HResult, &mut def);
    c.hr(hr, S_OK)?;
    let (cat, named, attributes) = (def.category, is(def.name, "Windows"), def.attributes);
    def.free();
    c.eq(cat.into(), KF_CATEGORY_FIXED.into())?;
    c.ok(named, 0)?;
    c.eq(attributes.into(), 0)?;
    c.eq(release(folder).into(), 0)?;

    for name in ["Windows", "windows"] {
        let name = wide(name);
        let mut folder = null_mut();
        let hr = call!(
            manager,
            GET_FOLDER_BY_NAME,
            fn(*const u16, *mut Obj) -> HResult,
            name.as_ptr(),
            &mut folder
        );
        c.hr(hr, S_OK)?;
        let (hr, id) = folder_id(folder);
        c.hr(hr, S_OK)?;
        c.ok(id == FOLDERID_WINDOWS, u64::from(id[0]))?;
        c.eq(release(folder).into(), 0)?;
    }
    let name = wide("NoSuch");
    let mut folder: Obj = core::ptr::dangling_mut();
    let hr = call!(
        manager,
        GET_FOLDER_BY_NAME,
        fn(*const u16, *mut Obj) -> HResult,
        name.as_ptr(),
        &mut folder
    );
    c.hr(hr, E_FILE_NOT_FOUND)?;
    c.ok(folder.is_null(), folder as u64)?;
    // The names App-V's package tokens use ([{AppData}], [{Common
    // AppData}], [{ProgramFilesX86}], ...).
    for (name, want) in [
        ("AppData", FOLDERID_ROAMINGAPPDATA),
        ("Local AppData", FOLDERID_LOCALAPPDATA),
        ("Common AppData", FOLDERID_PROGRAMDATA),
        ("ProgramFilesX86", FOLDERID_PROGRAMFILESX86),
        ("System", FOLDERID_SYSTEM),
        ("Profile", FOLDERID_PROFILE),
    ] {
        let wname = wide(name);
        let mut folder = null_mut();
        let hr = call!(
            manager,
            GET_FOLDER_BY_NAME,
            fn(*const u16, *mut Obj) -> HResult,
            wname.as_ptr(),
            &mut folder
        );
        c.hr(hr, S_OK)?;
        let (hr, id) = folder_id(folder);
        c.hr(hr, S_OK)?;
        c.ok(id == want, u64::from(id[0]))?;
        c.eq(release(folder).into(), 0)?;
    }
    Ok(())
}

/// GetFolderIds, and each listed folder's id and category.
fn listed(c: &mut Checks, manager: Obj, found: &mut [bool]) -> Result<(), u32> {
    let mut ids: *mut Guid = null_mut();
    let mut count = 0u32;
    let hr = call!(
        manager,
        GET_FOLDER_IDS,
        fn(*mut *mut Guid, *mut u32) -> HResult,
        &mut ids,
        &mut count
    );
    c.hr(hr, S_OK)?;
    c.ok(!ids.is_null() && count as usize >= FOLDERS.len() / 2, count.into())?;
    let mut virtuals = 0;
    for i in 0..count as usize {
        // SAFETY: GetFolderIds' `count` ids.
        let id = unsafe { *ids.add(i) };
        if let Some(k) = FOLDERS.iter().position(|k| k.id == id) {
            found[k] = true;
        }
        let (hr, folder) = get_folder(manager, &id);
        c.hr(hr, S_OK)?;
        let (hr, got) = folder_id(folder);
        c.hr(hr, S_OK)?;
        c.ok(got == id, i as u64)?;
        let mut category = 0;
        c.hr(call!(folder, GET_CATEGORY, fn(*mut u32) -> HResult, &mut category), S_OK)?;
        let mut def = Definition::new();
        let hr = call!(folder, GET_FOLDER_DEFINITION, fn(*mut Definition) -> HResult, &mut def);
        c.hr(hr, S_OK)?;
        let def_category = def.category;
        def.free();
        c.eq(def_category.into(), category.into())?;
        if category == KF_CATEGORY_VIRTUAL {
            virtuals += 1;
        }
        c.eq(release(folder).into(), 0)?;
    }
    free(ids.cast());
    info()
        .s(b"GetFolderIds: ")
        .dec(count.into())
        .s(b" folders, ")
        .dec(virtuals)
        .s(b" virtual\r\n")
        .print();
    Ok(())
}

/// Wine's check_known_folder for each folder of FOLDERS.
fn definitions(c: &mut Checks, manager: Obj, found: &[bool]) -> Result<(), u32> {
    for (k, kf) in FOLDERS.iter().enumerate() {
        if !found[k] {
            info().s(b"not listed: ").s(kf.what.as_bytes()).s(b"\r\n").print();
            c.note(false, 0, kf.what);
            continue;
        }
        if kf.csidl >= 0 {
            let mut csidl = -1;
            let hr = call!(
                manager,
                FOLDER_ID_TO_CSIDL,
                fn(*const Guid, *mut i32) -> HResult,
                &kf.id,
                &mut csidl
            );
            c.note(hr == S_OK && csidl == kf.csidl, csidl as u64, kf.what);
        }
        let (hr, folder) = get_folder(manager, &kf.id);
        c.hr(hr, S_OK)?;
        let mut def = Definition::new();
        let hr = call!(folder, GET_FOLDER_DEFINITION, fn(*mut Definition) -> HResult, &mut def);
        c.hr(hr, S_OK)?;
        if !is(def.name, kf.name) {
            info().s(kf.what.as_bytes()).s(b" name ").w(def.name).s(b"\r\n").print();
        }
        c.note(is(def.name, kf.name), 0, kf.what);
        c.note(def.category == kf.category, def.category.into(), kf.what);
        let parent_ok = if kf.parents.is_empty() {
            def.parent == GUID_NULL
        } else {
            kf.parents.contains(&def.parent)
        };
        c.note(parent_ok, u64::from(def.parent[0]), kf.what);
        if !is_one_of(def.relative, kf.relative) {
            info().s(kf.what.as_bytes()).s(b" relative ").w(def.relative).s(b"\r\n").print();
        }
        c.note(is_one_of(def.relative, kf.relative), 0, kf.what);
        if !is_one_of(def.parsing, kf.parsing) {
            info().s(kf.what.as_bytes()).s(b" parsing ").w(def.parsing).s(b"\r\n").print();
        }
        c.note(is_one_of(def.parsing, kf.parsing), 0, kf.what);
        c.note(def.attributes == kf.attributes, def.attributes.into(), kf.what);
        c.note(def.flags & !kf.flags == 0, def.flags.into(), kf.what);

        // GetPath, SHGetKnownFolderPath and SHGetFolderPathW agree.
        let (hr, path) = folder_path(folder, KF_FLAG_DONT_VERIFY);
        let mut known = null_mut();
        // SAFETY: a GUID and an output.
        let known_hr = unsafe { SHGetKnownFolderPath(&kf.id, KF_FLAG_DONT_VERIFY, 0, &mut known) };
        c.note(hr == known_hr, hr as u32 as u64, kf.what);
        if hr == S_OK {
            c.note(same_ci(path, known), 0, kf.what);
            if kf.csidl >= 0 {
                let mut buf = [0u16; MAX_PATH];
                // SAFETY: a MAX_PATH buffer.
                let hr = unsafe {
                    SHGetFolderPathW(0, kf.csidl | CSIDL_FLAG_DONT_VERIFY, 0, 0, buf.as_mut_ptr())
                };
                if hr != S_OK || !same_ci(path, buf.as_ptr()) {
                    info()
                        .s(kf.what.as_bytes())
                        .s(b" SHGetFolderPathW ")
                        .hex(hr as u32 as u64)
                        .s(b" ")
                        .w(buf.as_ptr())
                        .s(b"\r\n")
                        .print();
                }
                c.note(hr == S_OK && same_ci(path, buf.as_ptr()), hr as u32 as u64, kf.what);
            }
        } else {
            info().s(kf.what.as_bytes()).s(b" GetPath ").hex(hr as u32 as u64).s(b"\r\n").print();
        }
        // A relative path is below the parent's.
        if hr == S_OK && !def.relative.is_null() {
            let (phr, parent) = get_folder(manager, &def.parent);
            c.note(phr == S_OK, phr as u32 as u64, kf.what);
            if phr == S_OK {
                let (phr, parent_path) = folder_path(parent, KF_FLAG_DONT_VERIFY);
                c.note(phr == S_OK && under(path, parent_path, def.relative), 0, kf.what);
                if !under(path, parent_path, def.relative) {
                    info()
                        .s(kf.what.as_bytes())
                        .s(b" path ")
                        .w(path)
                        .s(b" parent ")
                        .w(parent_path)
                        .s(b"\r\n")
                        .print();
                }
                free(parent_path);
                release(parent);
            }
        }
        free(path);
        free(known);
        def.free();
        c.eq(release(folder).into(), 0)?;
    }
    Ok(())
}

/// Where the folders the environment and kernel32 name are.
fn roots(c: &mut Checks, manager: Obj) -> Result<(), u32> {
    let mut want = [0u16; MAX_PATH];
    for (id, var) in [
        (FOLDERID_PROFILE, "USERPROFILE"),
        (FOLDERID_ROAMINGAPPDATA, "APPDATA"),
        (FOLDERID_LOCALAPPDATA, "LOCALAPPDATA"),
        (FOLDERID_PROGRAMDATA, "ProgramData"),
        (FOLDERID_PROGRAMFILES, "ProgramFiles"),
        (FOLDERID_PROGRAMFILESX86, "ProgramFiles(x86)"),
        (FOLDERID_PUBLIC, "PUBLIC"),
        (FOLDERID_WINDOWS, ""),
        (FOLDERID_SYSTEM, ""),
    ] {
        let n = if id == FOLDERID_WINDOWS {
            // SAFETY: a MAX_PATH buffer.
            unsafe { GetWindowsDirectoryW(want.as_mut_ptr(), MAX_PATH as u32) }
        } else if id == FOLDERID_SYSTEM {
            // SAFETY: a MAX_PATH buffer.
            unsafe { GetSystemDirectoryW(want.as_mut_ptr(), MAX_PATH as u32) }
        } else {
            let name = wide(var);
            // SAFETY: a name and a MAX_PATH buffer.
            unsafe { GetEnvironmentVariableW(name.as_ptr(), want.as_mut_ptr(), MAX_PATH as u32) }
        };
        c.ok(n > 0, u64::from(id[0]))?;
        let (hr, folder) = get_folder(manager, &id);
        c.hr(hr, S_OK)?;
        let (hr, path) = folder_path(folder, KF_FLAG_DONT_VERIFY);
        c.hr(hr, S_OK)?;
        let same = same_ci(path, want.as_ptr());
        if !same {
            info().w(path).s(b" is not ").w(want.as_ptr()).s(b"\r\n").print();
        }
        free(path);
        c.ok(same, u64::from(id[0]))?;
        c.eq(release(folder).into(), 0)?;
    }
    Ok(())
}

/// FindFolderFromPath's id for `path` in `mode` (GUID_NULL when it fails).
fn find(manager: Obj, path: &[u16], mode: u32) -> (HResult, Guid) {
    let mut folder: Obj = core::ptr::dangling_mut();
    let hr = call!(
        manager,
        FIND_FOLDER_FROM_PATH,
        fn(*const u16, u32, *mut Obj) -> HResult,
        path.as_ptr(),
        mode,
        &mut folder
    );
    if hr != S_OK {
        return (hr, if folder.is_null() { GUID_NULL } else { [0xff; 16] });
    }
    let (_, id) = folder_id(folder);
    release(folder);
    (hr, id)
}

fn find_from_path(c: &mut Checks, manager: Obj) -> Result<(), u32> {
    let mut windows = [0u16; MAX_PATH];
    let mut system = [0u16; MAX_PATH];
    // SAFETY: MAX_PATH buffers.
    let n = unsafe { GetSystemDirectoryW(system.as_mut_ptr(), MAX_PATH as u32) } as usize;
    // SAFETY: as above.
    let w = unsafe { GetWindowsDirectoryW(windows.as_mut_ptr(), MAX_PATH as u32) } as usize;
    let (hr, id) = find(manager, &windows, FFFP_EXACTMATCH);
    c.hr(hr, S_OK)?;
    c.ok(id == FOLDERID_WINDOWS, u64::from(id[0]))?;
    let (hr, id) = find(manager, &system, FFFP_EXACTMATCH);
    c.hr(hr, S_OK)?;
    c.ok(id == FOLDERID_SYSTEM, u64::from(id[0]))?;
    // Case aside.
    let mut lower = windows;
    for u in lower[..w].iter_mut() {
        if (u16::from(b'A')..=u16::from(b'Z')).contains(u) {
            *u += 32;
        }
    }
    let (hr, id) = find(manager, &lower, FFFP_EXACTMATCH);
    c.hr(hr, S_OK)?;
    c.ok(id == FOLDERID_WINDOWS, u64::from(id[0]))?;
    // Below the system directory: its folder only as the nearest parent.
    let mut below = system;
    for (i, u) in "\\drivers\\etc".encode_utf16().enumerate() {
        below[n + i] = u;
    }
    let (hr, id) = find(manager, &below, FFFP_NEARESTPARENTMATCH);
    c.hr(hr, S_OK)?;
    c.ok(id == FOLDERID_SYSTEM, u64::from(id[0]))?;
    let (hr, id) = find(manager, &below, FFFP_EXACTMATCH);
    info().s(b"FindFolderFromPath(below, exact) ").hex(hr as u32 as u64).s(b"\r\n").print();
    c.ok(hr < 0, hr as u32 as u64)?;
    c.note(id == GUID_NULL, u64::from(id[0]), "FindFolderFromPath");
    // A path under no folder.
    let nowhere = wide("Q:\\gala-knownfolder-test");
    let (hr, id) = find(manager, &nowhere, FFFP_NEARESTPARENTMATCH);
    info().s(b"FindFolderFromPath(nowhere) ").hex(hr as u32 as u64).s(b"\r\n").print();
    c.ok(hr < 0, hr as u32 as u64)?;
    c.note(id == GUID_NULL, u64::from(id[0]), "FindFolderFromPath");
    Ok(())
}

/// Unknown ids and CSIDLs; a folder without a CSIDL.
fn unknown(c: &mut Checks, manager: Obj) -> Result<(), u32> {
    let (hr, folder) = get_folder(manager, &FOLDERID_NONE);
    info().s(b"GetFolder(unknown) ").hex(hr as u32 as u64).s(b"\r\n").print();
    c.ok(hr < 0, hr as u32 as u64)?;
    c.note(folder.is_null(), folder as u64, "GetFolder(unknown)");
    let mut csidl = 7;
    let hr = call!(
        manager,
        FOLDER_ID_TO_CSIDL,
        fn(*const Guid, *mut i32) -> HResult,
        &FOLDERID_NONE,
        &mut csidl
    );
    info().s(b"FolderIdToCsidl(unknown) ").hex(hr as u32 as u64).s(b"\r\n").print();
    c.ok(hr < 0, hr as u32 as u64)?;
    let hr = call!(
        manager,
        FOLDER_ID_TO_CSIDL,
        fn(*const Guid, *mut i32) -> HResult,
        &FOLDERID_DOWNLOADS,
        &mut csidl
    );
    info().s(b"FolderIdToCsidl(Downloads) ").hex(hr as u32 as u64).s(b"\r\n").print();
    c.ok(hr < 0, hr as u32 as u64)?;
    let mut id = [0u8; 16];
    let hr = call!(manager, FOLDER_ID_FROM_CSIDL, fn(i32, *mut Guid) -> HResult, 0x7f, &mut id);
    info().s(b"FolderIdFromCsidl(0x7f) ").hex(hr as u32 as u64).s(b"\r\n").print();
    c.ok(hr < 0, hr as u32 as u64)?;
    let mut path = core::ptr::dangling_mut::<u16>();
    // SAFETY: a GUID and an output.
    let hr = unsafe { SHGetKnownFolderPath(&FOLDERID_NONE, 0, 0, &mut path) };
    info().s(b"SHGetKnownFolderPath(unknown) ").hex(hr as u32 as u64).s(b"\r\n").print();
    c.ok(hr < 0, hr as u32 as u64)?;
    c.ok(path.is_null(), path as u64)?;
    Ok(())
}

const ERROR_SUCCESS: u64 = 0;
const ERROR_BAD_PATHNAME: u64 = 161;
const ERROR_ALREADY_EXISTS: u64 = 183;
const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

/// `dir` followed by `tail` (ASCII), NUL-terminated.
fn join(dir: &[u16], tail: &str) -> [u16; MAX_PATH] {
    let mut w = [0u16; MAX_PATH];
    let n = wlen(dir.as_ptr());
    w[..n].copy_from_slice(&dir[..n]);
    for (d, c) in w[n..MAX_PATH - 1].iter_mut().zip(tail.encode_utf16()) {
        *d = c;
    }
    w
}

fn is_dir(path: &[u16]) -> bool {
    // SAFETY: a NUL-terminated path.
    let a = unsafe { GetFileAttributesW(path.as_ptr()) };
    a != INVALID_FILE_ATTRIBUTES && a & FILE_ATTRIBUTE_DIRECTORY != 0
}

fn exists(path: &[u16]) -> bool {
    // SAFETY: a NUL-terminated path.
    unsafe { GetFileAttributesW(path.as_ptr()) != INVALID_FILE_ATTRIBUTES }
}

/// SHCreateDirectory (shell32's ordinal 165, imported by it) and
/// SHCreateDirectoryExW, as Microsoft's SHCreateDirectory page documents
/// them and Wine's shlfileop.c test_sh_create_dir checks them on Windows:
/// a fully qualified path is created with every missing folder above it
/// (ERROR_SUCCESS); an existing directory is ERROR_ALREADY_EXISTS; a
/// relative path is ERROR_BAD_PATHNAME and creates nothing. Under
/// %TEMP%\gala_shcd_<program>, removed before and after. The answers
/// for an existing file's name and for a path under a file (the page
/// lists ERROR_FILE_EXISTS and ERROR_ALREADY_EXISTS) are `info` lines.
fn create_directory(c: &mut Checks) -> Result<(), u32> {
    let mut temp = [0u16; MAX_PATH];
    let name = wide("TEMP");
    // SAFETY: valid buffers.
    let n = unsafe { GetEnvironmentVariableW(name.as_ptr(), temp.as_mut_ptr(), MAX_PATH as u32) };
    c.ok(n > 0 && (n as usize) < MAX_PATH - 64, n.into())?;
    let mut top = join(&temp, "\\gala_shcd_");
    top = join(&top, NAME);
    let a = join(&top, "\\a");
    let ab = join(&top, "\\a\\b");
    let cd = join(&top, "\\c\\d");
    let c_ = join(&top, "\\c");
    let file = join(&top, "\\file");
    let under_file = join(&top, "\\file\\x");
    let clean = || {
        // SAFETY: NUL-terminated paths; failures (absent) are fine.
        unsafe {
            for d in [&ab, &a, &cd, &c_, &under_file] {
                RemoveDirectoryW(d.as_ptr());
            }
            DeleteFileW(file.as_ptr());
            RemoveDirectoryW(top.as_ptr());
        }
    };
    clean();
    c.ok(!exists(&top), 0)?;

    // Nested: the top folder and `a` are created on the way to `b`.
    // SAFETY: a NUL-terminated path.
    c.eq(unsafe { SHCreateDirectory(0, ab.as_ptr()) } as u32 as u64, ERROR_SUCCESS)?;
    c.ok(is_dir(&top), 0)?;
    c.ok(is_dir(&a), 0)?;
    c.ok(is_dir(&ab), 0)?;
    // Again: it exists.
    // SAFETY: as above.
    c.eq(unsafe { SHCreateDirectory(0, ab.as_ptr()) } as u32 as u64, ERROR_ALREADY_EXISTS)?;
    // SAFETY: as above.
    c.eq(unsafe { SHCreateDirectory(0, top.as_ptr()) } as u32 as u64, ERROR_ALREADY_EXISTS)?;
    // Relative paths: refused, nothing created.
    for rel in ["gala_shcd_rel", "gala_shcd_rel\\sub", ".\\gala_shcd_rel"] {
        let w = wide(rel);
        // SAFETY: a NUL-terminated path.
        c.eq(unsafe { SHCreateDirectory(0, w.as_ptr()) } as u32 as u64, ERROR_BAD_PATHNAME)?;
        // SAFETY: as above.
        c.eq(unsafe { SHCreateDirectoryExW(0, w.as_ptr(), 0) } as u32 as u64, ERROR_BAD_PATHNAME)?;
    }
    c.ok(!exists(&wide("gala_shcd_rel")), 0)?;
    // SHCreateDirectoryExW with no security attributes: the same.
    // SAFETY: a NUL-terminated path.
    c.eq(unsafe { SHCreateDirectoryExW(0, cd.as_ptr(), 0) } as u32 as u64, ERROR_SUCCESS)?;
    c.ok(is_dir(&c_), 0)?;
    c.ok(is_dir(&cd), 0)?;
    // SAFETY: as above.
    c.eq(unsafe { SHCreateDirectoryExW(0, cd.as_ptr(), 0) } as u32 as u64, ERROR_ALREADY_EXISTS)?;
    // A file's name, and a path under a file.
    // SAFETY: a NUL-terminated path; CREATE_NEW, GENERIC_WRITE.
    let h = unsafe { CreateFileW(file.as_ptr(), 0x4000_0000, 0, 0, 1, 0x80, 0) };
    c.ok(h != usize::MAX, 0)?;
    // SAFETY: the handle just opened.
    unsafe { CloseHandle(h) };
    // SAFETY: a NUL-terminated path.
    let r = unsafe { SHCreateDirectory(0, file.as_ptr()) } as u32;
    info().s(b"SHCreateDirectory(an existing file) ").dec(r.into()).s(b"\r\n").print();
    c.ok(r != 0 && !is_dir(&file), r.into())?;
    // SAFETY: a NUL-terminated path.
    let r = unsafe { SHCreateDirectory(0, under_file.as_ptr()) } as u32;
    info().s(b"SHCreateDirectory(under a file) ").dec(r.into()).s(b"\r\n").print();
    c.ok(r != 0 && !exists(&under_file), r.into())?;
    clean();
    c.ok(!exists(&top), 0)?;
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0, 0);
    create_directory(&mut c)?;
    // SAFETY: plain call.
    c.hr(unsafe { CoInitializeEx(null_mut(), COINIT_APARTMENTTHREADED) }, S_OK)?;
    let mut manager: Obj = null_mut();
    // SAFETY: valid GUIDs and output.
    let hr = unsafe {
        CoCreateInstance(
            &CLSID_KNOWNFOLDERMANAGER,
            null_mut(),
            CLSCTX_INPROC_SERVER,
            &IID_IKNOWNFOLDERMANAGER,
            &mut manager,
        )
    };
    c.hr(hr, S_OK)?;
    c.ok(!manager.is_null(), 0)?;
    c.hr(query(manager, &IID_IMARSHAL).0, E_NOINTERFACE)?;
    let (hr, unknown_) = query(manager, &IID_IUNKNOWN);
    c.hr(hr, S_OK)?;
    release(unknown_);
    windows_folder(&mut c, manager)?;
    let mut found = [false; FOLDERS.len()];
    listed(&mut c, manager, &mut found)?;
    definitions(&mut c, manager, &found)?;
    roots(&mut c, manager)?;
    find_from_path(&mut c, manager)?;
    unknown(&mut c, manager)?;
    c.eq(release(manager).into(), 0)?;
    // SAFETY: balances CoInitializeEx.
    unsafe { CoUninitialize() };
    if c.1 != 0 {
        return Err(c.1);
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            Line::new().s(NAME.as_bytes()).s(b": ok\r\n").print();
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
