//! gala's shell32.dll: the folder functions programs use to find the
//! user's profile folders (the JVM's user.home). The answers come from the
//! same place as the environment's: USERPROFILE, APPDATA, LOCALAPPDATA,
//! ProgramData, ProgramFiles, and the Windows and system directories.

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

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
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn GetWindowsDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
    fn LocalAlloc(flags: u32, bytes: usize) -> *mut u16;
}

const S_OK: i32 = 0;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
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

/// CSIDL values and their folders.
const CSIDLS: &[(i32, Folder)] = &[
    (0x05, Folder::Env("USERPROFILE", "\\Documents")), // CSIDL_PERSONAL
    (0x10, Folder::Env("USERPROFILE", "\\Desktop")),   // CSIDL_DESKTOPDIRECTORY
    (0x1a, Folder::Env("APPDATA", "")),                // CSIDL_APPDATA
    (0x1c, Folder::Env("LOCALAPPDATA", "")),           // CSIDL_LOCAL_APPDATA
    (0x23, Folder::Env("ProgramData", "")),            // CSIDL_COMMON_APPDATA
    (0x24, Folder::Windows),                           // CSIDL_WINDOWS
    (0x25, Folder::System),                            // CSIDL_SYSTEM
    (0x26, Folder::Env("ProgramFiles", "")),           // CSIDL_PROGRAM_FILES
    (0x28, Folder::Env("USERPROFILE", "")),            // CSIDL_PROFILE
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
];

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
                let mut name = [0u16; 16];
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
/// E_INVALIDARG for a folder gala does not have (the CSIDL_FLAG_* bits
/// are ignored: the folders exist).
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
    // SAFETY: the caller's MAX_PATH buffer.
    unsafe { core::ptr::copy_nonoverlapping(buf.as_ptr(), path, n + 1) };
    S_OK
}

/// SHGetKnownFolderPath(id, flags, token, *path): the path in memory the
/// caller frees (CoTaskMemFree; here from the process heap, which gala's
/// CoTaskMemFree would free too).
#[unsafe(no_mangle)]
pub extern "system" fn SHGetKnownFolderPath(
    id: *const [u8; 16],
    _flags: u32,
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
