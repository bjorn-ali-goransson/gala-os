//! gala's shell32.dll: the folder functions programs use to find the
//! user's profile folders (the JVM's user.home). The answers come from the
//! same place as the environment's: USERPROFILE, APPDATA, LOCALAPPDATA,
//! ProgramData, ProgramFiles, and the Windows and system directories.
//! Also SHGetFileInfoW with gala's system image lists (`fileinfo.rs`),
//! running programs, dropped files and file operations (`shellops.rs`),
//! and shell links (`link.rs`: CLSID_ShellLink, `.lnk` files,
//! SHAddToRecentDocs).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod browser;
mod fileinfo;
mod icons;
mod link;
mod shellops;
mod unimplemented;

// The 32-bit build (SysWOW64) takes the memory routines, 64-bit
// division helpers and unwind personality from ntrt's x86 runtime
// (ARCHITECTURE "Plan: WOW64").
#[cfg(target_arch = "x86")]
use ntrt as _;

#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
    // MSVC-target unwind tables reference it (gala's ntdll provides it).
    fn __CxxFrameHandler3();
    // The stack probe for frames over a page (ntdll exports it).
    fn __chkstk();
}

/// MSVC-target codegen references this marker symbol directly, so it cannot
/// come from an import. (It becomes a harmless extra data export.)
#[cfg(target_arch = "x86_64")]
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn GetWindowsDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn LocalAlloc(flags: u32, bytes: usize) -> *mut u16;
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn GetModuleFileNameW(module: *mut core::ffi::c_void, buf: *mut u16, size: u32) -> u32;
    fn CreateDirectoryW(path: *const u16, attrs: *const core::ffi::c_void) -> i32;
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

/// Reports `what` to the ledger as unsupported, with `detail`.
pub(crate) fn unsupported(what: &[u8], detail: u64) {
    // SAFETY: a valid buffer; 2 is GalaRecordMissing's "unsupported".
    unsafe { GalaRecordMissing(what.as_ptr(), what.len(), 2, detail) };
}

const S_OK: i32 = 0;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
const E_FAIL: i32 = 0x8000_4005u32 as i32;
/// HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND): an unknown folder id.
const E_FILE_NOT_FOUND: i32 = 0x8007_0002u32 as i32;
const E_OUTOFMEMORY: i32 = 0x8007_000eu32 as i32;
const MAX_PATH: usize = 260;

/// Where a folder is: an environment variable (with an optional
/// subfolder), or the Windows or system directory.
#[derive(Clone, Copy)]
enum Folder {
    Env(&'static str, &'static str),
    Windows,
    System,
}

const START_MENU: &str = "\\Microsoft\\Windows\\Start Menu";
const PROGRAMS: &str = "\\Microsoft\\Windows\\Start Menu\\Programs";
const STARTUP: &str = "\\Microsoft\\Windows\\Start Menu\\Programs\\Startup";

/// CSIDL values and their folders (where Windows 11 has them).
const CSIDLS: &[(i32, Folder)] = &[
    (0x02, Folder::Env("APPDATA", PROGRAMS)), // CSIDL_PROGRAMS
    (0x05, Folder::Env("USERPROFILE", "\\Documents")), // CSIDL_PERSONAL
    (0x06, Folder::Env("USERPROFILE", "\\Favorites")), // CSIDL_FAVORITES
    (0x07, Folder::Env("APPDATA", STARTUP)),  // CSIDL_STARTUP
    (0x08, Folder::Env("APPDATA", "\\Microsoft\\Windows\\Recent")), // CSIDL_RECENT
    (0x09, Folder::Env("APPDATA", "\\Microsoft\\Windows\\SendTo")), // CSIDL_SENDTO
    (0x0b, Folder::Env("APPDATA", START_MENU)), // CSIDL_STARTMENU
    (0x10, Folder::Env("USERPROFILE", "\\Desktop")), // CSIDL_DESKTOPDIRECTORY
    (0x14, Folder::Env("windir", "\\Fonts")), // CSIDL_FONTS
    (0x15, Folder::Env("APPDATA", "\\Microsoft\\Windows\\Templates")), // CSIDL_TEMPLATES
    (0x16, Folder::Env("ProgramData", START_MENU)), // CSIDL_COMMON_STARTMENU
    (0x17, Folder::Env("ProgramData", PROGRAMS)), // CSIDL_COMMON_PROGRAMS
    (0x18, Folder::Env("ProgramData", STARTUP)), // CSIDL_COMMON_STARTUP
    (0x19, Folder::Env("PUBLIC", "\\Desktop")), // CSIDL_COMMON_DESKTOPDIRECTORY
    (0x1a, Folder::Env("APPDATA", "")),       // CSIDL_APPDATA
    (0x1c, Folder::Env("LOCALAPPDATA", "")),  // CSIDL_LOCAL_APPDATA
    (0x23, Folder::Env("ProgramData", "")),   // CSIDL_COMMON_APPDATA
    (0x24, Folder::Windows),                  // CSIDL_WINDOWS
    (0x25, Folder::System),                   // CSIDL_SYSTEM
    (0x26, Folder::Env("ProgramFiles", "")),  // CSIDL_PROGRAM_FILES
    (0x28, Folder::Env("USERPROFILE", "")),   // CSIDL_PROFILE
    (0x29, Folder::Env("windir", "\\SysWOW64")), // CSIDL_SYSTEMX86 (64-bit Windows)
    (0x2a, Folder::Env("ProgramFiles(x86)", "")), // CSIDL_PROGRAM_FILESX86
    (0x2b, Folder::Env("CommonProgramFiles", "")), // CSIDL_PROGRAM_FILES_COMMON
    (0x2c, Folder::Env("CommonProgramFiles(x86)", "")), // CSIDL_PROGRAM_FILES_COMMONX86
    (0x2d, Folder::Env("ProgramData", "\\Microsoft\\Windows\\Templates")), // CSIDL_COMMON_TEMPLATES
    (0x2e, Folder::Env("PUBLIC", "\\Documents")), // CSIDL_COMMON_DOCUMENTS
];

/// KNOWNFOLDERIDs (GUIDs as in memory: Data1-3 little endian) and their
/// folders.
const KNOWN: &[([u8; 16], Folder)] = &[
    // FOLDERID_Profile {5E6C858F-0E22-4760-9AFE-EA3317B67173}
    (
        guid(0x5e6c858f, 0x0e22, 0x4760, [0x9a, 0xfe, 0xea, 0x33, 0x17, 0xb6, 0x71, 0x73]),
        Folder::Env("USERPROFILE", ""),
    ),
    // FOLDERID_RoamingAppData {3EB685DB-65F9-4CF6-A03A-E3EF65729F3D}
    (
        guid(0x3eb685db, 0x65f9, 0x4cf6, [0xa0, 0x3a, 0xe3, 0xef, 0x65, 0x72, 0x9f, 0x3d]),
        Folder::Env("APPDATA", ""),
    ),
    // FOLDERID_LocalAppData {F1B32785-6FBA-4FCF-9D55-7B8E7F157091}
    (
        guid(0xf1b32785, 0x6fba, 0x4fcf, [0x9d, 0x55, 0x7b, 0x8e, 0x7f, 0x15, 0x70, 0x91]),
        Folder::Env("LOCALAPPDATA", ""),
    ),
    // FOLDERID_Documents {FDD39AD0-238F-46AF-ADB4-6C85480369C7}
    (
        guid(0xfdd39ad0, 0x238f, 0x46af, [0xad, 0xb4, 0x6c, 0x85, 0x48, 0x03, 0x69, 0xc7]),
        Folder::Env("USERPROFILE", "\\Documents"),
    ),
    // FOLDERID_ProgramData {62AB5D82-FDC1-4DC3-A9DD-070D1D495D97}
    (
        guid(0x62ab5d82, 0xfdc1, 0x4dc3, [0xa9, 0xdd, 0x07, 0x0d, 0x1d, 0x49, 0x5d, 0x97]),
        Folder::Env("ProgramData", ""),
    ),
    // FOLDERID_Windows {F38BF404-1D43-42F2-9305-67DE0B28FC23}
    (
        guid(0xf38bf404, 0x1d43, 0x42f2, [0x93, 0x05, 0x67, 0xde, 0x0b, 0x28, 0xfc, 0x23]),
        Folder::Windows,
    ),
    // FOLDERID_System {1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}
    (
        guid(0x1ac14e77, 0x02e7, 0x4e5d, [0xb7, 0x44, 0x2e, 0xb1, 0xae, 0x51, 0x98, 0xb7]),
        Folder::System,
    ),
    // FOLDERID_UserProgramFiles {5CD7AEE2-2219-4A67-B85D-6C9CE15660CB}
    (
        guid(0x5cd7aee2, 0x2219, 0x4a67, [0xb8, 0x5d, 0x6c, 0x9c, 0xe1, 0x56, 0x60, 0xcb]),
        Folder::Env("LOCALAPPDATA", "\\Programs"),
    ),
    // FOLDERID_UserProgramFilesCommon {BCBD3057-CA5C-4622-B42D-BC56DB0AE516}
    (
        guid(0xbcbd3057, 0xca5c, 0x4622, [0xb4, 0x2d, 0xbc, 0x56, 0xdb, 0x0a, 0xe5, 0x16]),
        Folder::Env("LOCALAPPDATA", "\\Programs\\Common"),
    ),
    // FOLDERID_Programs {A77F5D77-2E2B-44C3-A6A2-ABA601054A51}
    (
        guid(0xa77f5d77, 0x2e2b, 0x44c3, [0xa6, 0xa2, 0xab, 0xa6, 0x01, 0x05, 0x4a, 0x51]),
        Folder::Env("APPDATA", PROGRAMS),
    ),
    // FOLDERID_StartMenu {625B53C3-AB48-4EC1-BA1F-A1EF4146FC19}
    (
        guid(0x625b53c3, 0xab48, 0x4ec1, [0xba, 0x1f, 0xa1, 0xef, 0x41, 0x46, 0xfc, 0x19]),
        Folder::Env("APPDATA", START_MENU),
    ),
    // FOLDERID_Desktop {B4BFCC3A-DB2C-424C-B029-7FE99A87C641}
    (
        guid(0xb4bfcc3a, 0xdb2c, 0x424c, [0xb0, 0x29, 0x7f, 0xe9, 0x9a, 0x87, 0xc6, 0x41]),
        Folder::Env("USERPROFILE", "\\Desktop"),
    ),
    // FOLDERID_SavedGames {4C5C32FF-BB9D-43B0-B5B4-2D72E54EAAA4}
    (
        guid(0x4c5c32ff, 0xbb9d, 0x43b0, [0xb5, 0xb4, 0x2d, 0x72, 0xe5, 0x4e, 0xaa, 0xa4]),
        Folder::Env("USERPROFILE", "\\Saved Games"),
    ),
];

/// Creates the directory `path` (its first `n` units) and the ones above
/// it that are missing, as CSIDL_FLAG_CREATE and KF_FLAG_CREATE ask.
/// Whether it exists now.
fn create_folder(path: &mut [u16; MAX_PATH], n: usize) -> bool {
    const INVALID: u32 = u32::MAX;
    // SAFETY: NUL-terminated at n.
    if unsafe { GetFileAttributesW(path.as_ptr()) } != INVALID {
        return true;
    }
    // Each component after the drive ("C:\"), shortest first.
    for i in 3..=n {
        if i == n || path[i] == u16::from(b'\\') {
            let saved = path[i];
            path[i] = 0;
            // SAFETY: NUL-terminated at i; an existing one fails harmlessly.
            unsafe { CreateDirectoryW(path.as_ptr(), core::ptr::null()) };
            path[i] = saved;
        }
    }
    // SAFETY: NUL-terminated at n.
    unsafe { GetFileAttributesW(path.as_ptr()) != INVALID }
}

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> [u8; 16] {
    let a = d1.to_le_bytes();
    let b = d2.to_le_bytes();
    let c = d3.to_le_bytes();
    [
        a[0], a[1], a[2], a[3], b[0], b[1], c[0], c[1], d4[0], d4[1], d4[2], d4[3], d4[4], d4[5],
        d4[6], d4[7],
    ]
}

/// The folder's path in `out` (NUL-terminated); its length.
fn resolve(folder: Folder, out: &mut [u16; MAX_PATH]) -> Option<usize> {
    // SAFETY: buffers of MAX_PATH units.
    let n = unsafe {
        match folder {
            Folder::Windows => GetWindowsDirectoryW(out.as_mut_ptr(), MAX_PATH as u32),
            Folder::System => GetSystemDirectoryW(out.as_mut_ptr(), MAX_PATH as u32),
            Folder::Env(var, sub) => {
                let mut name = [0u16; 32];
                for (d, c) in name.iter_mut().zip(var.encode_utf16()) {
                    *d = c;
                }
                let n = GetEnvironmentVariableW(name.as_ptr(), out.as_mut_ptr(), MAX_PATH as u32)
                    as usize;
                if n == 0 || n + sub.len() >= MAX_PATH {
                    return None;
                }
                for (i, c) in sub.encode_utf16().enumerate() {
                    out[n + i] = c;
                }
                out[n + sub.len()] = 0;
                (n + sub.len()) as u32
            }
        }
    } as usize;
    (n > 0 && n < MAX_PATH).then_some(n)
}

/// SHGetFolderPathW(hwnd, csidl, token, flags, path[MAX_PATH]): S_OK or
/// E_INVALIDARG for a folder gala does not have. With CSIDL_FLAG_CREATE
/// the folder is created if it is missing (E_FAIL if that fails); without
/// it the path is given even when the folder is missing (gala's profile
/// does not hold every standard folder Windows makes with a profile).
#[unsafe(no_mangle)]
pub extern "system" fn SHGetFolderPathW(
    _window: usize,
    csidl: i32,
    _token: usize,
    _flags: u32,
    path: *mut u16,
) -> i32 {
    let Some(&(_, folder)) = CSIDLS.iter().find(|(c, _)| *c == csidl & 0xff) else {
        return E_INVALIDARG;
    };
    let mut buf = [0u16; MAX_PATH];
    let Some(n) = resolve(folder, &mut buf) else { return E_INVALIDARG };
    if path.is_null() {
        return E_INVALIDARG;
    }
    const CSIDL_FLAG_CREATE: i32 = 0x8000;
    if csidl & CSIDL_FLAG_CREATE != 0 && !create_folder(&mut buf, n) {
        return E_FAIL;
    }
    // SAFETY: the caller's MAX_PATH buffer.
    unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), path, n + 1) };
    S_OK
}

/// SHGetFolderPathA: SHGetFolderPathW's path in the ANSI code page
/// (UTF-8 on gala), into the caller's MAX_PATH buffer.
#[unsafe(no_mangle)]
pub extern "system" fn SHGetFolderPathA(
    window: usize,
    csidl: i32,
    token: usize,
    flags: u32,
    path: *mut u8,
) -> i32 {
    if path.is_null() {
        return E_INVALIDARG;
    }
    let mut wide = [0u16; MAX_PATH];
    let r = SHGetFolderPathW(window, csidl, token, flags, wide.as_mut_ptr());
    if r != S_OK {
        return r;
    }
    let n = wide.iter().position(|&c| c == 0).unwrap_or(MAX_PATH);
    let mut at = 0;
    for c in char::decode_utf16(wide[..n].iter().copied()) {
        let mut b = [0u8; 4];
        let bytes = c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut b).as_bytes();
        if at + bytes.len() >= MAX_PATH {
            return E_INVALIDARG;
        }
        // SAFETY: inside the caller's MAX_PATH buffer.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), path.add(at), bytes.len()) };
        at += bytes.len();
    }
    // SAFETY: as above.
    unsafe { *path.add(at) = 0 };
    S_OK
}

/// SHGetSpecialFolderPathW(window, path, csidl, create): SHGetFolderPathW's
/// path (CSIDL_FLAG_CREATE when `create`); TRUE on success.
#[unsafe(no_mangle)]
pub extern "system" fn SHGetSpecialFolderPathW(
    window: usize,
    path: *mut u16,
    csidl: i32,
    create: i32,
) -> i32 {
    let flag = if create != 0 { 0x8000 } else { 0 };
    i32::from(SHGetFolderPathW(window, csidl | flag, 0, 0, path) == S_OK)
}

#[unsafe(no_mangle)]
pub extern "system" fn SHGetSpecialFolderPathA(
    window: usize,
    path: *mut u8,
    csidl: i32,
    create: i32,
) -> i32 {
    let flag = if create != 0 { 0x8000 } else { 0 };
    i32::from(SHGetFolderPathA(window, csidl | flag, 0, 0, path) == S_OK)
}

/// SHGetKnownFolderPath(id, flags, token, *path): the path in memory the
/// caller frees (CoTaskMemFree; here from the process heap, which gala's
/// CoTaskMemFree would free too); KF_FLAG_CREATE creates a missing folder
/// (E_FAIL if that fails).
#[unsafe(no_mangle)]
pub extern "system" fn SHGetKnownFolderPath(
    id: *const [u8; 16],
    flags: u32,
    _token: usize,
    path: *mut *mut u16,
) -> i32 {
    if id.is_null() || path.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the caller's GUID.
    let id = unsafe { *id };
    let Some(&(_, folder)) = KNOWN.iter().find(|(g, _)| *g == id) else {
        // SAFETY: the caller's output.
        unsafe { *path = core::ptr::null_mut() };
        return E_FILE_NOT_FOUND;
    };
    let mut buf = [0u16; MAX_PATH];
    let Some(n) = resolve(folder, &mut buf) else { return E_FILE_NOT_FOUND };
    const KF_FLAG_CREATE: u32 = 0x8000;
    if flags & KF_FLAG_CREATE != 0 && !create_folder(&mut buf, n) {
        // SAFETY: the caller's output.
        unsafe { *path = core::ptr::null_mut() };
        return E_FAIL;
    }
    // SAFETY: plain call.
    let p = unsafe { LocalAlloc(0, 2 * (n + 1)) };
    if p.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: a buffer of n + 1 units; the caller's output.
    unsafe {
        core::ptr::copy_nonoverlapping(buf.as_ptr(), p, n + 1);
        *path = p;
    }
    S_OK
}

/// DllMain (the entry point our link flags name).
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup(_instance: usize, _reason: u32, _reserved: usize) -> i32 {
    1
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// Splits a command line as CommandLineToArgvW documents: the program
/// name (the first word; a quoted one up to the next quote) without
/// escapes; then words separated by spaces and tabs, where 2n backslashes
/// before a quote are n backslashes and the quote toggles quoting, 2n+1
/// are n and a literal quote, other backslashes are literal, and `""`
/// inside quotes is a literal quote. `emit` gets each argument's
/// characters, one at a time, and `None` at its end.
fn split_command_line(line: &[u16], mut emit: impl FnMut(Option<u16>)) {
    const QUOTE: u16 = b'"' as u16;
    const BACKSLASH: u16 = b'\\' as u16;
    let blank = |c: u16| c == u16::from(b' ') || c == u16::from(b'\t');
    let mut i = 0;
    // The program name.
    if line.first() == Some(&QUOTE) {
        i = 1;
        while i < line.len() && line[i] != QUOTE {
            emit(Some(line[i]));
            i += 1;
        }
        i += 1;
    } else {
        while i < line.len() && !blank(line[i]) {
            emit(Some(line[i]));
            i += 1;
        }
    }
    emit(None);
    loop {
        while i < line.len() && blank(line[i]) {
            i += 1;
        }
        if i >= line.len() {
            return;
        }
        let mut quoted = false;
        while i < line.len() && (quoted || !blank(line[i])) {
            let c = line[i];
            if c == BACKSLASH {
                let start = i;
                while i < line.len() && line[i] == BACKSLASH {
                    i += 1;
                }
                let count = i - start;
                if i < line.len() && line[i] == QUOTE {
                    for _ in 0..count / 2 {
                        emit(Some(BACKSLASH));
                    }
                    if count % 2 == 1 {
                        emit(Some(QUOTE));
                        i += 1;
                    }
                } else {
                    for _ in 0..count {
                        emit(Some(BACKSLASH));
                    }
                }
                continue;
            }
            if c == QUOTE {
                if quoted && line.get(i + 1) == Some(&QUOTE) {
                    emit(Some(QUOTE));
                    i += 2;
                    continue;
                }
                quoted = !quoted;
                i += 1;
                continue;
            }
            emit(Some(c));
            i += 1;
        }
        emit(None);
    }
}

/// CommandLineToArgvW(line, &count): the arguments in one LocalAlloc'd
/// block (the pointer array, NULL-terminated, then the strings), which the
/// caller frees with LocalFree. An empty line gives the program's path.
#[unsafe(no_mangle)]
pub extern "system" fn CommandLineToArgvW(line: *const u16, count: *mut i32) -> *mut *mut u16 {
    let mut path = [0u16; 520];
    // SAFETY: the caller's NUL-terminated line, or our own buffer.
    let text: &[u16] = unsafe {
        let mut n = 0;
        if !line.is_null() {
            while *line.add(n) != 0 {
                n += 1;
            }
        }
        if n == 0 {
            let n = GetModuleFileNameW(core::ptr::null_mut(), path.as_mut_ptr(), 520) as usize;
            // Quoted, so a path with spaces stays one argument.
            path.copy_within(0..n.min(517), 1);
            path[0] = u16::from(b'"');
            path[n.min(517) + 1] = u16::from(b'"');
            &path[..n.min(517) + 2]
        } else {
            core::slice::from_raw_parts(line, n)
        }
    };
    let (mut args, mut chars) = (0usize, 0usize);
    split_command_line(text, |c| match c {
        Some(_) => chars += 1,
        None => args += 1,
    });
    let table = (args + 1) * 8;
    // SAFETY: a fresh block of the size computed; filled in order.
    unsafe {
        let block = LocalAlloc(0, table + 2 * (chars + args)).cast::<u8>();
        if block.is_null() {
            return core::ptr::null_mut();
        }
        let argv = block.cast::<*mut u16>();
        let mut at = block.add(table).cast::<u16>();
        let mut index = 0;
        let mut start = at;
        split_command_line(text, |c| match c {
            Some(ch) => {
                *at = ch;
                at = at.add(1);
            }
            None => {
                *at = 0;
                at = at.add(1);
                *argv.add(index) = start;
                index += 1;
                start = at;
            }
        });
        *argv.add(args) = core::ptr::null_mut();
        if !count.is_null() {
            *count = args as i32;
        }
        argv
    }
}

/// The process's explicit AppUserModelID (the taskbar's grouping key), up
/// to 128 characters.
static mut APP_ID: [u16; 129] = [0; 129];

/// SetCurrentProcessExplicitAppUserModelID(id): kept for the process;
/// S_OK, or E_INVALIDARG for NULL or more than 128 characters.
#[unsafe(no_mangle)]
pub extern "system" fn SetCurrentProcessExplicitAppUserModelID(id: *const u16) -> i32 {
    const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
    if id.is_null() {
        return E_INVALIDARG;
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string, read up to 129 units.
    while n <= 128 && unsafe { *id.add(n) } != 0 {
        n += 1;
    }
    if n > 128 {
        return E_INVALIDARG;
    }
    // SAFETY: a process-wide setting written as a whole; the source is the
    // caller's string of `n` units.
    unsafe {
        let dst = &raw mut APP_ID;
        core::ptr::copy_nonoverlapping(id, (*dst).as_mut_ptr(), n);
        (*dst)[n] = 0;
    }
    0
}
