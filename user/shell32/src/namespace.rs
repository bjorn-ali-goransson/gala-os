//! The shell namespace (ROADMAP EX step 1; ARCHITECTURE "The file
//! explorer"): IShellFolder, IShellItem, their enumerators and the item
//! ID list functions over gala's file systems, the drives below "This
//! PC" and each WSL distribution below "Linux" (`\\wsl.localhost\<distro>`,
//! the kernel's one file system: nt::wslfs). The model (lists, parsing
//! and display names, order) is `user/shellns`; this module asks the
//! file systems (FindFirstFileW, GetFileAttributesW, the volume's label)
//! and the registry (the user's WSL registrations, HKCU\...\Lxss) and
//! makes the COM objects.
//!
//! - Lists come from the process heap, as CoTaskMemAlloc's and ILFree's.
//! - A parse checks every level exists (FindFirstFileW, which also gives
//!   each name's case as stored); a missing one is
//!   HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND).
//! - An item's properties (SHGetPropertyStoreFromParsingName) are a
//!   read-only IPropertyStore over those same readings: the name, the
//!   kind, the size, the attributes and the times. A property gala does
//!   not keep is VT_EMPTY and a row in the ledger.
//! - Not there (each recorded, E_NOTIMPL): IShellFolder's BindToStorage,
//!   CreateViewObject and GetUIObjectOf (context menus, data objects,
//!   icons: gala's explorer.exe does without), the handlers of
//!   BindToHandler other than the folder and its items, IShellItem2.
//! - The desktop lists This PC and Linux (when a distribution is
//!   registered), not the files of the user's Desktop directory.

use alloc::vec::Vec;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use shellns::{CLSID_LINUX, CLSID_MY_COMPUTER, Guid, Item};

use super::fileinfo::{extension, type_name};
use super::{E_FAIL, E_INVALIDARG, E_OUTOFMEMORY, S_OK, reached, unsupported};
use crate::link::{VT_EMPTY, VT_FILETIME, VT_LPWSTR, VT_UI4, VT_UI8, VTable, write_variant};

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn SetLastError(e: u32);
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn FindFirstFileW(pattern: *const u16, data: *mut FindData) -> usize;
    fn FindNextFileW(h: usize, data: *mut FindData) -> i32;
    fn FindClose(h: usize) -> i32;
    fn GetLogicalDrives() -> u32;
    fn MoveFileW(from: *const u16, to: *const u16) -> i32;
    fn GetLastError() -> u32;
    fn GetVolumeInformationW(
        root: *const u16,
        label: *mut u16,
        label_size: u32,
        serial: *mut u32,
        max_component: *mut u32,
        flags: *mut u32,
        fs_name: *mut u16,
        fs_name_size: u32,
    ) -> i32;
}

type HResult = i32;
const S_FALSE: HResult = 1;
const E_NOTIMPL: HResult = 0x8000_4001u32 as i32;
const E_NOINTERFACE: HResult = 0x8000_4002u32 as i32;
const E_POINTER: HResult = 0x8000_4003u32 as i32;
const MK_E_NOOBJECT: HResult = 0x8004_01e5u32 as i32;
const S_NO_MORE: HResult = S_FALSE;

/// HRESULT_FROM_WIN32.
const fn from_win32(e: u32) -> HResult {
    (0x8007_0000 | (e & 0xffff)) as i32
}
const E_FILE_NOT_FOUND: HResult = from_win32(2);
const E_PATH_NOT_FOUND: HResult = from_win32(3);

const OLE: [u8; 8] = [0xc0, 0, 0, 0, 0, 0, 0, 0x46];
const IID_IUNKNOWN: Guid = shellns::guid(0, 0, 0, OLE);
const IID_IPERSIST: Guid = shellns::guid(0x10c, 0, 0, OLE);
const IID_ISHELLFOLDER: Guid = shellns::guid(0x0002_14e6, 0, 0, OLE);
const IID_IENUMIDLIST: Guid = shellns::guid(0x0002_14f2, 0, 0, OLE);
const IID_IPERSISTFOLDER: Guid = shellns::guid(0x0002_14ea, 0, 0, OLE);
const IID_IPERSISTFOLDER2: Guid =
    shellns::guid(0x1ac3_d9f0, 0x175c, 0x11d1, [0x95, 0xbe, 0x00, 0x60, 0x97, 0x97, 0xea, 0x4f]);
const IID_ISHELLITEM: Guid =
    shellns::guid(0x4382_6d1e, 0xe718, 0x42ee, [0xbc, 0x55, 0xa1, 0xe2, 0x61, 0xc3, 0x7b, 0xfe]);
const IID_IENUMSHELLITEMS: Guid =
    shellns::guid(0x7062_9033, 0xe363, 0x4a28, [0xa5, 0x67, 0x0d, 0xb7, 0x80, 0x06, 0xe6, 0xd7]);
const IID_IPERSISTIDLIST: Guid =
    shellns::guid(0x1079_acfc, 0x29bd, 0x11d3, [0x8e, 0x0d, 0x00, 0xc0, 0x4f, 0x68, 0x37, 0xd5]);
const BHID_SFOBJECT: Guid =
    shellns::guid(0x3981_e224, 0xf559, 0x11d3, [0x8e, 0x3a, 0x00, 0xc0, 0x4f, 0x68, 0x37, 0xd5]);
const BHID_ENUMITEMS: Guid =
    shellns::guid(0x94f6_0519, 0x2850, 0x4924, [0xaa, 0x5a, 0xd1, 0x5e, 0x84, 0x86, 0x80, 0x39]);
/// CLSID_ShellDesktop and CLSID_ShellFSFolder (IPersist::GetClassID).
const CLSID_SHELL_DESKTOP: Guid = shellns::guid(0x0002_1400, 0, 0, OLE);
const CLSID_SHELL_FS_FOLDER: Guid =
    shellns::guid(0xf336_4ba0, 0x65b9, 0x11ce, [0xa9, 0xba, 0x00, 0xaa, 0x00, 0x4a, 0xe8, 0x37]);
/// FOLDERID_Desktop and FOLDERID_ComputerFolder (lists of their own).
const FOLDERID_DESKTOP: Guid =
    shellns::guid(0xb4bf_cc3a, 0xdb2c, 0x424c, [0xb0, 0x29, 0x7f, 0xe9, 0x9a, 0x87, 0xc6, 0x41]);
const FOLDERID_COMPUTER: Guid =
    shellns::guid(0x0ac0_837c, 0xbbf8, 0x452a, [0x85, 0x0d, 0x79, 0xd0, 0x8e, 0x66, 0x7c, 0xa7]);

// SFGAO_* bits.
const SFGAO_CANCOPY: u32 = 0x1;
const SFGAO_CANMOVE: u32 = 0x2;
const SFGAO_CANLINK: u32 = 0x4;
const SFGAO_STORAGE: u32 = 0x8;
const SFGAO_CANRENAME: u32 = 0x10;
const SFGAO_CANDELETE: u32 = 0x20;
const SFGAO_HASPROPSHEET: u32 = 0x40;
const SFGAO_DROPTARGET: u32 = 0x100;
const SFGAO_LINK: u32 = 0x1_0000;
const SFGAO_READONLY: u32 = 0x4_0000;
const SFGAO_HIDDEN: u32 = 0x8_0000;
const SFGAO_STREAM: u32 = 0x40_0000;
const SFGAO_STORAGEANCESTOR: u32 = 0x80_0000;
const SFGAO_FILESYSANCESTOR: u32 = 0x1000_0000;
const SFGAO_FOLDER: u32 = 0x2000_0000;
const SFGAO_FILESYSTEM: u32 = 0x4000_0000;
const SFGAO_HASSUBFOLDER: u32 = 0x8000_0000;

// SHCONTF_* and SHGDN_* flags.
const SHCONTF_FOLDERS: u32 = 0x20;
const SHCONTF_NONFOLDERS: u32 = 0x40;
const SHCONTF_INCLUDEHIDDEN: u32 = 0x80;
const SHGDN_INFOLDER: u32 = 0x1;
const SHGDN_FORADDRESSBAR: u32 = 0x4000;
const SHGDN_FORPARSING: u32 = 0x8000;

// SIGDN values.
const SIGDN_NORMALDISPLAY: u32 = 0;
const SIGDN_PARENTRELATIVEPARSING: u32 = 0x8001_8001;
const SIGDN_DESKTOPABSOLUTEPARSING: u32 = 0x8002_8000;
const SIGDN_PARENTRELATIVEEDITING: u32 = 0x8003_1001;
const SIGDN_DESKTOPABSOLUTEEDITING: u32 = 0x8004_c000;
const SIGDN_FILESYSPATH: u32 = 0x8005_8000;
const SIGDN_PARENTRELATIVEFORADDRESSBAR: u32 = 0x8007_c001;
const SIGDN_PARENTRELATIVE: u32 = 0x8008_0001;
const SIGDN_PARENTRELATIVEFORUI: u32 = 0x8009_4001;

const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const INVALID: u32 = u32::MAX;

use super::shellops::FindData;

fn zeroed_find() -> FindData {
    FindData {
        attributes: 0,
        times: [0; 6],
        size_high: 0,
        size_low: 0,
        reserved: [0; 2],
        name: [0; 260],
        short_name: [0; 14],
    }
}

fn units(z: &[u16]) -> &[u16] {
    &z[..z.iter().position(|&c| c == 0).unwrap_or(z.len())]
}

/// `s` with a NUL.
fn z(s: &[u16]) -> Vec<u16> {
    let mut v = Vec::new();
    if v.try_reserve_exact(s.len() + 1).is_ok() {
        v.extend_from_slice(s);
        v.push(0);
    }
    v
}

/// A NUL-terminated string of the caller's.
fn wstr<'a>(p: *const u16) -> &'a [u16] {
    if p.is_null() {
        return &[];
    }
    // SAFETY: the caller's NUL-terminated string.
    unsafe {
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    }
}

fn put<T>(out: *mut T, v: T) {
    if !out.is_null() {
        // SAFETY: the caller's output (null checked).
        unsafe { out.write_unaligned(v) };
    }
}

fn iid(p: *const Guid) -> Guid {
    // SAFETY: the caller's GUID (null checked).
    if p.is_null() { [0; 16] } else { unsafe { p.read_unaligned() } }
}

fn data1(g: &Guid) -> u64 {
    u64::from(u32::from_le_bytes([g[0], g[1], g[2], g[3]]))
}

// ---- Lists --------------------------------------------------------------------

/// The bytes of the caller's list, its terminator included.
pub fn list_bytes<'a>(p: *const u8) -> &'a [u8] {
    if p.is_null() {
        return &[];
    }
    let mut at = 0usize;
    loop {
        // SAFETY: the caller's list: items of `cb` bytes up to a zero cb.
        let cb = usize::from(unsafe { p.add(at).cast::<u16>().read_unaligned() });
        if cb == 0 {
            // SAFETY: `at + 2` bytes walked above.
            return unsafe { core::slice::from_raw_parts(p, at + 2) };
        }
        at += cb;
    }
}

/// The items of the caller's list (None: not gala's).
pub fn items_of(p: *const u8) -> Option<Vec<Item>> {
    shellns::decode_list(list_bytes(p))
}

/// `bytes` copied to the process heap (a list for the caller to free).
fn heap_copy(bytes: &[u8]) -> *mut u8 {
    // SAFETY: plain call.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 0, bytes.len().max(2)) };
    if !p.is_null() {
        // SAFETY: a fresh block of at least that size.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len()) };
    }
    p
}

/// A new list of `items` (NULL when out of memory).
pub fn new_list(items: &[Item]) -> *mut u8 {
    heap_copy(&shellns::encode_list(items))
}

/// A task-allocated copy of `s` (CoTaskMemAlloc's heap) with its NUL.
fn task_string(s: &[u16]) -> *mut u16 {
    // SAFETY: plain call.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 0, 2 * (s.len() + 1)) }.cast::<u16>();
    if !p.is_null() {
        // SAFETY: a fresh block of s.len() + 1 units.
        unsafe {
            core::ptr::copy_nonoverlapping(s.as_ptr(), p, s.len());
            *p.add(s.len()) = 0;
        }
    }
    p
}

// ---- What the file systems and the registry say -------------------------------

/// The user's Desktop directory (the desktop's own path).
fn desktop_dir() -> Vec<u16> {
    let mut path = [0u16; super::MAX_PATH];
    // CSIDL_DESKTOPDIRECTORY.
    if super::SHGetFolderPathW(0, 0x10, 0, 0, path.as_mut_ptr()) != S_OK {
        return Vec::new();
    }
    units(&path).to_vec()
}

/// The volume label of drive `letter` (empty without one).
fn drive_label(letter: u8) -> Vec<u16> {
    let root = [u16::from(letter), u16::from(b':'), u16::from(b'\\'), 0];
    let mut label = [0u16; 64];
    let null = core::ptr::null_mut();
    // SAFETY: a NUL-terminated root and our buffer.
    let ok = unsafe {
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
    if ok == 0 { Vec::new() } else { units(&label).to_vec() }
}

/// The drive letters (GetLogicalDrives).
fn drives() -> Vec<u8> {
    // SAFETY: plain call.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8).filter(|i| mask & (1 << i) != 0).map(|i| b'A' + i).collect()
}

/// The WSL distributions registered for this user (Lxss keys in State 1,
/// installed), by their DistributionName.
pub fn distros() -> Vec<Vec<u16>> {
    type Open = extern "system" fn(usize, *const u16, u32, u32, *mut usize) -> u32;
    type Enum = extern "system" fn(
        usize,
        u32,
        *mut u16,
        *mut u32,
        *mut u32,
        *mut u16,
        *mut u32,
        *mut u64,
    ) -> u32;
    type Query =
        extern "system" fn(usize, *const u16, *mut u32, *mut u32, *mut u8, *mut u32) -> u32;
    let mut out = Vec::new();
    let (Some(open), Some(enumerate), Some(query), Some(close)) = (
        import!("advapi32.dll", "RegOpenKeyExW", Open),
        import!("advapi32.dll", "RegEnumKeyExW", Enum),
        import!("advapi32.dll", "RegQueryValueExW", Query),
        import!("advapi32.dll", "RegCloseKey", extern "system" fn(usize) -> u32),
    ) else {
        return out;
    };
    const HKEY_CURRENT_USER: usize = 0x8000_0001;
    const KEY_READ: u32 = 0x2_0019;
    let path: Vec<u16> =
        "Software\\Microsoft\\Windows\\CurrentVersion\\Lxss\0".encode_utf16().collect();
    let mut lxss = 0usize;
    if open(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ, &mut lxss) != 0 {
        return out;
    }
    let null = core::ptr::null_mut();
    let state_name: Vec<u16> = "State\0".encode_utf16().collect();
    let name_name: Vec<u16> = "DistributionName\0".encode_utf16().collect();
    for i in 0.. {
        let mut sub = [0u16; 64];
        let mut n = 64u32;
        if enumerate(
            lxss,
            i,
            sub.as_mut_ptr(),
            &mut n,
            null,
            core::ptr::null_mut(),
            null,
            core::ptr::null_mut(),
        ) != 0
        {
            break;
        }
        let mut key = 0usize;
        if open(lxss, sub.as_ptr(), 0, KEY_READ, &mut key) != 0 {
            continue;
        }
        let (mut state, mut size) = (0u32, 4u32);
        let has_state =
            query(key, state_name.as_ptr(), null, null, (&raw mut state).cast(), &mut size) == 0;
        let mut name = [0u16; 64];
        let mut size = 128u32;
        let has_name =
            query(key, name_name.as_ptr(), null, null, name.as_mut_ptr().cast(), &mut size) == 0;
        close(key);
        if has_state && state == 1 && has_name {
            out.push(units(&name).to_vec());
        }
    }
    close(lxss);
    out
}

/// The find data of the file at `path` (FindFirstFileW: its name as the
/// file system stores it, its attributes), or the error.
fn find_one(path: &[u16]) -> Result<FindData, u32> {
    let p = z(path);
    let mut fd = zeroed_find();
    // SAFETY: a NUL-terminated name and our buffer.
    let h = unsafe { FindFirstFileW(p.as_ptr(), &mut fd) };
    if h == usize::MAX {
        // SAFETY: plain call.
        return Err(unsafe { GetLastError() });
    }
    // SAFETY: our handle.
    unsafe { FindClose(h) };
    Ok(fd)
}

/// The items `name` (a desktop-absolute parsing name) names, each level
/// checked: names as stored, the last one's attributes.
pub fn resolve(name: &[u16]) -> Result<Vec<Item>, HResult> {
    let mut items = shellns::parse(name).ok_or(E_INVALIDARG)?;
    let count = items.len();
    for i in 0..count {
        match &items[i] {
            Item::Root(CLSID_LINUX) if distros().is_empty() => return Err(E_FILE_NOT_FOUND),
            Item::Root(_) => {}
            Item::Drive(letter) => {
                let root = [u16::from(*letter), u16::from(b':'), u16::from(b'\\'), 0];
                // SAFETY: a NUL-terminated path.
                if unsafe { GetFileAttributesW(root.as_ptr()) } == INVALID {
                    return Err(E_PATH_NOT_FOUND);
                }
            }
            Item::Distro(d) => {
                let known = distros();
                let Some(stored) = known.iter().find(|k| eq_ignore_ascii_case(k, d)) else {
                    return Err(E_FILE_NOT_FOUND);
                };
                items[i] = Item::Distro(stored.clone());
            }
            Item::Fs { .. } => {
                let path = shellns::fs_path(&items[..=i]).ok_or(E_INVALIDARG)?;
                let fd = find_one(&path)
                    .map_err(|e| if i + 1 == count { from_win32(e) } else { E_PATH_NOT_FOUND })?;
                if i + 1 < count && fd.attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
                    return Err(E_PATH_NOT_FOUND);
                }
                items[i] = Item::Fs { attributes: fd.attributes, name: units(&fd.name).to_vec() };
            }
        }
    }
    Ok(items)
}

fn eq_ignore_ascii_case(a: &[u16], b: &[u16]) -> bool {
    let fold = |c: u16| if (65..=90).contains(&c) { c + 32 } else { c };
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| fold(x) == fold(y))
}

/// The children of folder `items` that `flags` (SHCONTF_*) asks for, in
/// Explorer's order.
pub fn children(items: &[Item], flags: u32) -> Vec<Item> {
    let mut out = Vec::new();
    let folders = flags & SHCONTF_FOLDERS != 0;
    match items {
        [] => {
            if folders {
                out.push(Item::Root(CLSID_MY_COMPUTER));
                if !distros().is_empty() {
                    out.push(Item::Root(CLSID_LINUX));
                }
            }
        }
        [Item::Root(CLSID_MY_COMPUTER)] => {
            if folders {
                out.extend(drives().into_iter().map(Item::Drive));
            }
        }
        [Item::Root(CLSID_LINUX)] => {
            if folders {
                out.extend(distros().into_iter().map(Item::Distro));
            }
        }
        _ => {
            let Some(mut pattern) = shellns::fs_path(items) else { return out };
            if pattern.last() != Some(&u16::from(b'\\')) {
                pattern.push(u16::from(b'\\'));
            }
            pattern.push(u16::from(b'*'));
            let p = z(&pattern);
            let mut fd = zeroed_find();
            // SAFETY: a NUL-terminated pattern and our buffer.
            let h = unsafe { FindFirstFileW(p.as_ptr(), &mut fd) };
            if h == usize::MAX {
                return out;
            }
            loop {
                let name = units(&fd.name);
                let dots = name == [46] || name == [46, 46];
                let dir = fd.attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
                let hidden = fd.attributes & FILE_ATTRIBUTE_HIDDEN != 0;
                let wanted = if dir { folders } else { flags & SHCONTF_NONFOLDERS != 0 };
                if !dots && wanted && (!hidden || flags & SHCONTF_INCLUDEHIDDEN != 0) {
                    out.push(Item::Fs { attributes: fd.attributes, name: name.to_vec() });
                }
                // SAFETY: our handle and buffer.
                if unsafe { FindNextFileW(h, &mut fd) } == 0 {
                    break;
                }
            }
            // SAFETY: our handle.
            unsafe { FindClose(h) };
        }
    }
    out.sort_by(shellns::compare_items);
    out
}

/// SFGAO_* of the last of `items` (the desktop's for none).
pub fn attributes_of(items: &[Item]) -> u32 {
    const FOLDER: u32 = SFGAO_FOLDER | SFGAO_HASSUBFOLDER | SFGAO_FILESYSANCESTOR;
    const FS_FOLDER: u32 = FOLDER
        | SFGAO_FILESYSTEM
        | SFGAO_STORAGE
        | SFGAO_STORAGEANCESTOR
        | SFGAO_DROPTARGET
        | SFGAO_HASPROPSHEET;
    const MOVABLE: u32 =
        SFGAO_CANCOPY | SFGAO_CANMOVE | SFGAO_CANLINK | SFGAO_CANRENAME | SFGAO_CANDELETE;
    match items.last() {
        None => FOLDER | SFGAO_HASPROPSHEET,
        Some(Item::Root(_)) => FOLDER | SFGAO_HASPROPSHEET,
        Some(Item::Drive(_)) => FS_FOLDER | SFGAO_CANLINK,
        Some(Item::Distro(_)) => FS_FOLDER,
        Some(Item::Fs { attributes, name }) => {
            let mut a = if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                FS_FOLDER | MOVABLE
            } else {
                SFGAO_FILESYSTEM | SFGAO_STREAM | SFGAO_HASPROPSHEET | MOVABLE
            };
            if attributes & FILE_ATTRIBUTE_HIDDEN != 0 {
                a |= SFGAO_HIDDEN;
            }
            if attributes & FILE_ATTRIBUTE_READONLY != 0 {
                a |= SFGAO_READONLY;
            }
            if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
                || name.ends_with(&[46, 108, 110, 107])
            {
                // A link (.lnk, a symbolic link).
                a |= SFGAO_LINK;
            }
            a
        }
    }
}

/// The name of `items` that `sigdn` asks for (None: it has none of that
/// kind, as a virtual folder's file-system path).
pub fn name_of(items: &[Item], sigdn: u32) -> Option<Vec<u16>> {
    let last = items.last();
    let display = || match last {
        None => "Desktop".encode_utf16().collect(),
        Some(item) => {
            let label = if let Item::Drive(l) = item { drive_label(*l) } else { Vec::new() };
            shellns::display_name(item, &label)
        }
    };
    let absolute = || {
        if items.is_empty() { desktop_dir() } else { shellns::parsing_name(items) }
    };
    match sigdn {
        SIGDN_NORMALDISPLAY
        | SIGDN_PARENTRELATIVEEDITING
        | SIGDN_PARENTRELATIVEFORADDRESSBAR
        | SIGDN_PARENTRELATIVE
        | SIGDN_PARENTRELATIVEFORUI => Some(display()),
        SIGDN_PARENTRELATIVEPARSING => {
            Some(last.map_or_else(desktop_dir, shellns::relative_parsing_name))
        }
        SIGDN_DESKTOPABSOLUTEPARSING => Some(absolute()),
        SIGDN_DESKTOPABSOLUTEEDITING => Some(shellns::fs_path(items).unwrap_or_else(display)),
        SIGDN_FILESYSPATH => {
            if items.is_empty() {
                Some(desktop_dir())
            } else {
                shellns::fs_path(items)
            }
        }
        _ => {
            unsupported(b"shell32.dll!IShellItem::GetDisplayName(sigdn)", u64::from(sigdn));
            None
        }
    }
}

/// IShellFolder::GetDisplayNameOf's flags as the SIGDN they ask for.
fn sigdn_of(flags: u32) -> u32 {
    let parsing = flags & SHGDN_FORPARSING != 0;
    let in_folder = flags & SHGDN_INFOLDER != 0;
    let address = flags & SHGDN_FORADDRESSBAR != 0;
    match (parsing, in_folder, address) {
        (true, true, _) => SIGDN_PARENTRELATIVEPARSING,
        (true, false, true) => SIGDN_DESKTOPABSOLUTEEDITING,
        (true, false, false) => SIGDN_DESKTOPABSOLUTEPARSING,
        (false, ..) => SIGDN_NORMALDISPLAY,
    }
}

// ---- Shared COM plumbing ------------------------------------------------------

/// The parts every object here has: its interface pointers (`N` vtables,
/// the first the identity), a reference count.
#[repr(C)]
struct Head<const N: usize> {
    vtables: [*const (); N],
    refs: AtomicU32,
}

/// `T` on the process heap with one reference (it starts with its
/// interface pointers).
fn boxed<T>(value: T) -> *mut T {
    // SAFETY: plain call.
    let p = unsafe { HeapAlloc(GetProcessHeap(), 0, core::mem::size_of::<T>()) }.cast::<T>();
    if !p.is_null() {
        // SAFETY: a fresh block of T's size (the process heap aligns to 16).
        unsafe { p.write(value) };
    }
    p
}

fn free<T>(p: *mut T) {
    // SAFETY: an object `boxed` made, no longer referenced.
    unsafe {
        core::ptr::drop_in_place(p);
        HeapFree(GetProcessHeap(), 0, p.cast());
    }
}

/// The object an interface pointer belongs to (its vtable `slot`
/// pointers into the object).
fn this<'a, T>(p: *mut c_void, slot: usize) -> &'a mut T {
    // SAFETY: COM calls a method with the interface pointer we gave out,
    // `slot` pointers into a live T.
    unsafe { &mut *(p.cast::<*const ()>().sub(slot).cast::<T>()) }
}

/// Hands out interface `slot` of object `base` (one more reference).
fn give(base: *mut c_void, refs: &AtomicU32, slot: usize, out: *mut *mut c_void) -> HResult {
    refs.fetch_add(1, Ordering::Relaxed);
    // SAFETY: an interface slot at the object's start.
    put(out, unsafe { base.cast::<*const ()>().add(slot) }.cast());
    S_OK
}

/// QueryInterface of an object whose interfaces are `table` (IID, slot).
fn query(
    base: *mut c_void,
    refs: &AtomicU32,
    table: &[(Guid, usize)],
    riid: *const Guid,
    out: *mut *mut c_void,
    what: &[u8],
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    let want = iid(riid);
    if want == IID_IUNKNOWN {
        return give(base, refs, 0, out);
    }
    match table.iter().find(|(g, _)| *g == want) {
        Some(&(_, slot)) => give(base, refs, slot, out),
        None => {
            put(out, core::ptr::null_mut());
            unsupported(what, data1(&want));
            E_NOINTERFACE
        }
    }
}

// ---- IShellFolder (and IPersistFolder2) -------------------------------------------

#[repr(C)]
struct Folder {
    head: Head<2>,
    /// The folder's absolute items.
    items: Vec<Item>,
}

const FOLDER_IIDS: &[(Guid, usize)] =
    &[(IID_ISHELLFOLDER, 0), (IID_IPERSIST, 1), (IID_IPERSISTFOLDER, 1), (IID_IPERSISTFOLDER2, 1)];

fn folder<'a>(p: *mut c_void, slot: usize) -> &'a mut Folder {
    this::<Folder>(p, slot)
}

fn new_folder(items: Vec<Item>) -> *mut Folder {
    boxed(Folder {
        head: Head {
            vtables: [
                (&FOLDER_VTABLE as *const VTable<13>).cast(),
                (&PERSIST_FOLDER_VTABLE as *const VTable<6>).cast(),
            ],
            refs: AtomicU32::new(1),
        },
        items,
    })
}

/// A new folder object for `items`, as interface `riid`.
fn folder_as(items: Vec<Item>, riid: *const Guid, out: *mut *mut c_void) -> HResult {
    let f = new_folder(items);
    if f.is_null() {
        put(out, core::ptr::null_mut());
        return E_OUTOFMEMORY;
    }
    // SAFETY: the new object.
    let me = unsafe { &mut *f };
    let r = query(
        f.cast(),
        &me.head.refs,
        FOLDER_IIDS,
        riid,
        out,
        b"shell32.dll!ShellFolder(QueryInterface)",
    );
    folder_release(f.cast());
    r
}

extern "system" fn folder_qi(p: *mut c_void, riid: *const Guid, out: *mut *mut c_void) -> HResult {
    let me = folder(p, 0);
    let base = (me as *mut Folder).cast();
    query(base, &me.head.refs, FOLDER_IIDS, riid, out, b"shell32.dll!ShellFolder(QueryInterface)")
}
extern "system" fn folder_add_ref(p: *mut c_void) -> u32 {
    folder(p, 0).head.refs.fetch_add(1, Ordering::Relaxed) + 1
}
extern "system" fn folder_release(p: *mut c_void) -> u32 {
    let me = folder(p, 0);
    let left = me.head.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        free(me as *mut Folder);
    }
    left
}

/// The items of a list relative to folder `me`, appended to its own.
fn below(me: &Folder, relative: *const u8) -> Option<Vec<Item>> {
    let mut all = me.items.clone();
    all.extend(items_of(relative)?);
    Some(all)
}

/// IShellFolder::ParseDisplayName(hwnd, bc, name, *eaten, *list, *attributes).
extern "system" fn folder_parse(
    p: *mut c_void,
    _hwnd: usize,
    _bc: *mut c_void,
    name: *const u16,
    eaten: *mut u32,
    out: *mut *mut u8,
    attributes: *mut u32,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if name.is_null() {
        return E_INVALIDARG;
    }
    let me = folder(p, 0);
    let name = wstr(name);
    // Relative to this folder: its parsing name, then the name.
    let full: Vec<u16> = if me.items.is_empty() {
        name.to_vec()
    } else {
        let mut f = shellns::parsing_name(&me.items);
        if f.last() != Some(&u16::from(b'\\')) {
            f.push(u16::from(b'\\'));
        }
        f.extend_from_slice(name);
        f
    };
    let items = match resolve(&full) {
        Ok(i) => i,
        Err(e) => return e,
    };
    if items.len() < me.items.len() || items[..me.items.len()] != me.items[..] {
        return E_INVALIDARG;
    }
    let rest = &items[me.items.len()..];
    put(eaten, name.len() as u32);
    if !attributes.is_null() {
        // SAFETY: the caller's in/out mask.
        let mask = unsafe { attributes.read_unaligned() };
        put(attributes, attributes_of(&items) & mask);
    }
    let list = new_list(rest);
    put(out, list);
    if list.is_null() { E_OUTOFMEMORY } else { S_OK }
}

/// IShellFolder::EnumObjects(hwnd, flags, *IEnumIDList).
extern "system" fn folder_enum(
    p: *mut c_void,
    _hwnd: usize,
    flags: u32,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let me = folder(p, 0);
    let mut lists = Vec::new();
    for child in children(&me.items, flags) {
        lists.push(shellns::encode_list(core::slice::from_ref(&child)));
    }
    new_enum_lists(lists, out)
}

/// IShellFolder::BindToObject(list, bc, riid, *out).
extern "system" fn folder_bind(
    p: *mut c_void,
    list: *const u8,
    _bc: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    put(out, core::ptr::null_mut());
    let me = folder(p, 0);
    let Some(items) = below(me, list) else { return E_INVALIDARG };
    if items.len() == me.items.len() || !items.last().is_some_and(Item::is_folder) {
        return E_INVALIDARG;
    }
    folder_as(items, riid, out)
}

extern "system" fn folder_bind_storage(
    _p: *mut c_void,
    _list: *const u8,
    _bc: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    put(out, core::ptr::null_mut());
    unsupported(b"shell32.dll!IShellFolder::BindToStorage", data1(&iid(riid)));
    E_NOTIMPL
}

/// IShellFolder::CompareIDs(lparam, a, b): the order (the low 16 bits:
/// -1, 0, 1 as an HRESULT's code), level by level.
extern "system" fn folder_compare(
    _p: *mut c_void,
    _lparam: isize,
    a: *const u8,
    b: *const u8,
) -> HResult {
    let (Some(a), Some(b)) = (items_of(a), items_of(b)) else { return E_INVALIDARG };
    let mut order = core::cmp::Ordering::Equal;
    for (x, y) in a.iter().zip(&b) {
        order = shellns::compare_items(x, y);
        if order != core::cmp::Ordering::Equal {
            break;
        }
    }
    if order == core::cmp::Ordering::Equal {
        order = a.len().cmp(&b.len());
    }
    match order {
        core::cmp::Ordering::Less => 0xffff,
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Greater => 1,
    }
}

extern "system" fn folder_view(
    _p: *mut c_void,
    _hwnd: usize,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    put(out, core::ptr::null_mut());
    unsupported(b"shell32.dll!IShellFolder::CreateViewObject", data1(&iid(riid)));
    E_NOTIMPL
}

/// IShellFolder::GetAttributesOf(count, lists, *mask): what all of them
/// have (the folder's own for none).
extern "system" fn folder_attributes(
    p: *mut c_void,
    count: u32,
    lists: *const *const u8,
    mask: *mut u32,
) -> HResult {
    if mask.is_null() {
        return E_INVALIDARG;
    }
    let me = folder(p, 0);
    let mut all = u32::MAX;
    if count == 0 || lists.is_null() {
        all = attributes_of(&me.items);
    }
    for i in 0..count as usize {
        if lists.is_null() {
            break;
        }
        // SAFETY: the caller's array of `count` lists.
        let list = unsafe { *lists.add(i) };
        let Some(items) = below(me, list) else { return E_INVALIDARG };
        all &= attributes_of(&items);
    }
    // SAFETY: the caller's in/out mask.
    let want = unsafe { mask.read_unaligned() };
    put(mask, all & want);
    S_OK
}

extern "system" fn folder_ui(
    _p: *mut c_void,
    _hwnd: usize,
    _count: u32,
    _lists: *const *const u8,
    riid: *const Guid,
    _reserved: *mut u32,
    out: *mut *mut c_void,
) -> HResult {
    put(out, core::ptr::null_mut());
    unsupported(b"shell32.dll!IShellFolder::GetUIObjectOf", data1(&iid(riid)));
    E_NOTIMPL
}

/// STRRET: uType, then the union (pOleStr, uOffset, cStr[260]) at the
/// pointer's alignment (8 in a 64-bit process, 4 in a 32-bit one).
#[repr(C)]
pub struct StrRet {
    kind: u32,
    text: *mut u16,
    rest: [u8; 256],
}

/// IShellFolder::GetDisplayNameOf(list, flags, *STRRET): STRRET_WSTR.
extern "system" fn folder_name(
    p: *mut c_void,
    list: *const u8,
    flags: u32,
    out: *mut StrRet,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let me = folder(p, 0);
    let Some(items) = below(me, list) else { return E_INVALIDARG };
    let Some(name) = name_of(&items, sigdn_of(flags)) else { return E_INVALIDARG };
    let s = task_string(&name);
    if s.is_null() {
        return E_OUTOFMEMORY;
    }
    put(out, StrRet { kind: 0, text: s, rest: [0; 256] });
    S_OK
}

/// IShellFolder::SetNameOf(hwnd, list, name, flags, *new list): a file
/// or folder renamed (MoveFileW).
extern "system" fn folder_set_name(
    p: *mut c_void,
    _hwnd: usize,
    list: *const u8,
    name: *const u16,
    _flags: u32,
    out: *mut *mut u8,
) -> HResult {
    put(out, core::ptr::null_mut());
    let me = folder(p, 0);
    let Some(items) = below(me, list) else { return E_INVALIDARG };
    let (Some(from), Some(Item::Fs { attributes, .. })) = (shellns::fs_path(&items), items.last())
    else {
        return E_INVALIDARG;
    };
    let new_name = wstr(name);
    let mut renamed = items.clone();
    let attributes = *attributes;
    if let Some(last) = renamed.last_mut() {
        *last = Item::Fs { attributes, name: new_name.to_vec() };
    }
    let Some(to) = shellns::fs_path(&renamed) else { return E_INVALIDARG };
    let (f, t) = (z(&from), z(&to));
    // SAFETY: NUL-terminated paths.
    if unsafe { MoveFileW(f.as_ptr(), t.as_ptr()) } == 0 {
        // SAFETY: plain call.
        return from_win32(unsafe { GetLastError() });
    }
    if !out.is_null() {
        put(out, new_list(&renamed[me.items.len()..]));
    }
    S_OK
}

static FOLDER_VTABLE: VTable<13> = VTable([
    folder_qi as *const (),
    folder_add_ref as *const (),
    folder_release as *const (),
    folder_parse as *const (),
    folder_enum as *const (),
    folder_bind as *const (),
    folder_bind_storage as *const (),
    folder_compare as *const (),
    folder_view as *const (),
    folder_attributes as *const (),
    folder_ui as *const (),
    folder_name as *const (),
    folder_set_name as *const (),
]);

extern "system" fn pf_qi(p: *mut c_void, riid: *const Guid, out: *mut *mut c_void) -> HResult {
    folder_qi((folder(p, 1) as *mut Folder).cast(), riid, out)
}
extern "system" fn pf_add_ref(p: *mut c_void) -> u32 {
    folder(p, 1).head.refs.fetch_add(1, Ordering::Relaxed) + 1
}
extern "system" fn pf_release(p: *mut c_void) -> u32 {
    folder_release((folder(p, 1) as *mut Folder).cast())
}
/// IPersist::GetClassID: the desktop's, a root's own, else the file
/// system folder's.
extern "system" fn pf_class(p: *mut c_void, out: *mut Guid) -> HResult {
    let me = folder(p, 1);
    let class = match me.items.as_slice() {
        [] => CLSID_SHELL_DESKTOP,
        [Item::Root(g)] => *g,
        _ => CLSID_SHELL_FS_FOLDER,
    };
    put(out, class);
    S_OK
}
/// IPersistFolder::Initialize(list): the folder becomes that one.
extern "system" fn pf_initialize(p: *mut c_void, list: *const u8) -> HResult {
    let me = folder(p, 1);
    match items_of(list) {
        Some(items) => {
            me.items = items;
            S_OK
        }
        None => E_INVALIDARG,
    }
}
/// IPersistFolder2::GetCurFolder(*list): a copy of its absolute list.
extern "system" fn pf_current(p: *mut c_void, out: *mut *mut u8) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let list = new_list(&folder(p, 1).items);
    put(out, list);
    if list.is_null() { E_OUTOFMEMORY } else { S_OK }
}

static PERSIST_FOLDER_VTABLE: VTable<6> = VTable([
    pf_qi as *const (),
    pf_add_ref as *const (),
    pf_release as *const (),
    pf_class as *const (),
    pf_initialize as *const (),
    pf_current as *const (),
]);

// ---- Enumerators: IEnumIDList and IEnumShellItems -----------------------------------

#[repr(C)]
struct Enum {
    head: Head<1>,
    /// IEnumIDList: each child's one-item list; IEnumShellItems: each
    /// child's absolute list.
    lists: Vec<Vec<u8>>,
    at: usize,
    shell_items: bool,
}

fn new_enum(lists: Vec<Vec<u8>>, shell_items: bool, out: *mut *mut c_void) -> HResult {
    let vtable: *const () = if shell_items {
        (&ENUM_ITEMS_VTABLE as *const VTable<7>).cast()
    } else {
        (&ENUM_LIST_VTABLE as *const VTable<7>).cast()
    };
    let e = boxed(Enum {
        head: Head { vtables: [vtable], refs: AtomicU32::new(1) },
        lists,
        at: 0,
        shell_items,
    });
    put(out, e.cast());
    if e.is_null() { E_OUTOFMEMORY } else { S_OK }
}

fn new_enum_lists(lists: Vec<Vec<u8>>, out: *mut *mut c_void) -> HResult {
    new_enum(lists, false, out)
}

extern "system" fn enum_qi(p: *mut c_void, riid: *const Guid, out: *mut *mut c_void) -> HResult {
    let me = this::<Enum>(p, 0);
    let table: &[(Guid, usize)] =
        if me.shell_items { &[(IID_IENUMSHELLITEMS, 0)] } else { &[(IID_IENUMIDLIST, 0)] };
    let base = (me as *mut Enum).cast();
    query(base, &me.head.refs, table, riid, out, b"shell32.dll!EnumIDList(QueryInterface)")
}
extern "system" fn enum_add_ref(p: *mut c_void) -> u32 {
    this::<Enum>(p, 0).head.refs.fetch_add(1, Ordering::Relaxed) + 1
}
extern "system" fn enum_release(p: *mut c_void) -> u32 {
    let me = this::<Enum>(p, 0);
    let left = me.head.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        free(me as *mut Enum);
    }
    left
}
/// Next(count, out[], *fetched): lists (IEnumIDList) or new items
/// (IEnumShellItems); S_FALSE when fewer than asked were left.
extern "system" fn enum_next(
    p: *mut c_void,
    count: u32,
    out: *mut *mut c_void,
    fetched: *mut u32,
) -> HResult {
    if out.is_null() || (count > 1 && fetched.is_null()) {
        return E_INVALIDARG;
    }
    let me = this::<Enum>(p, 0);
    let mut n = 0u32;
    while n < count && me.at < me.lists.len() {
        let list = &me.lists[me.at];
        let made: *mut c_void = if me.shell_items {
            let Some(items) = shellns::decode_list(list) else { break };
            new_item(items).cast()
        } else {
            heap_copy(list).cast()
        };
        if made.is_null() {
            break;
        }
        // SAFETY: the caller's array of `count` pointers.
        unsafe { *out.add(n as usize) = made };
        n += 1;
        me.at += 1;
    }
    put(fetched, n);
    if n == count { S_OK } else { S_NO_MORE }
}
extern "system" fn enum_skip(p: *mut c_void, count: u32) -> HResult {
    let me = this::<Enum>(p, 0);
    let left = me.lists.len() - me.at;
    me.at += (count as usize).min(left);
    if count as usize <= left { S_OK } else { S_FALSE }
}
extern "system" fn enum_reset(p: *mut c_void) -> HResult {
    this::<Enum>(p, 0).at = 0;
    S_OK
}
extern "system" fn enum_clone(p: *mut c_void, out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let me = this::<Enum>(p, 0);
    let r = new_enum(me.lists.clone(), me.shell_items, out);
    if r == S_OK {
        // SAFETY: the new enumerator.
        this::<Enum>(unsafe { *out }, 0).at = me.at;
    }
    r
}

static ENUM_LIST_VTABLE: VTable<7> = VTable([
    enum_qi as *const (),
    enum_add_ref as *const (),
    enum_release as *const (),
    enum_next as *const (),
    enum_skip as *const (),
    enum_reset as *const (),
    enum_clone as *const (),
]);
static ENUM_ITEMS_VTABLE: VTable<7> = VTable([
    enum_qi as *const (),
    enum_add_ref as *const (),
    enum_release as *const (),
    enum_next as *const (),
    enum_skip as *const (),
    enum_reset as *const (),
    enum_clone as *const (),
]);

// ---- IShellItem (and IPersistIDList) --------------------------------------------------

#[repr(C)]
struct ShellItem {
    head: Head<2>,
    items: Vec<Item>,
}

const ITEM_IIDS: &[(Guid, usize)] =
    &[(IID_ISHELLITEM, 0), (IID_IPERSIST, 1), (IID_IPERSISTIDLIST, 1)];

fn new_item(items: Vec<Item>) -> *mut ShellItem {
    boxed(ShellItem {
        head: Head {
            vtables: [
                (&ITEM_VTABLE as *const VTable<8>).cast(),
                (&PERSIST_IDLIST_VTABLE as *const VTable<6>).cast(),
            ],
            refs: AtomicU32::new(1),
        },
        items,
    })
}

/// A new item for `items`, as interface `riid`.
pub fn item_as(items: Vec<Item>, riid: *const Guid, out: *mut *mut c_void) -> HResult {
    let i = new_item(items);
    if i.is_null() {
        put(out, core::ptr::null_mut());
        return E_OUTOFMEMORY;
    }
    // SAFETY: the new object.
    let me = unsafe { &mut *i };
    let r = query(
        i.cast(),
        &me.head.refs,
        ITEM_IIDS,
        riid,
        out,
        b"shell32.dll!ShellItem(QueryInterface)",
    );
    item_release(i.cast());
    r
}

extern "system" fn item_qi(p: *mut c_void, riid: *const Guid, out: *mut *mut c_void) -> HResult {
    let me = this::<ShellItem>(p, 0);
    let base = (me as *mut ShellItem).cast();
    query(base, &me.head.refs, ITEM_IIDS, riid, out, b"shell32.dll!ShellItem(QueryInterface)")
}
extern "system" fn item_add_ref(p: *mut c_void) -> u32 {
    this::<ShellItem>(p, 0).head.refs.fetch_add(1, Ordering::Relaxed) + 1
}
extern "system" fn item_release(p: *mut c_void) -> u32 {
    let me = this::<ShellItem>(p, 0);
    let left = me.head.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        free(me as *mut ShellItem);
    }
    left
}

/// IShellItem::BindToHandler(bc, handler, riid, *out): BHID_SFObject (the
/// folder) and BHID_EnumItems (its children, hidden ones included).
extern "system" fn item_bind(
    p: *mut c_void,
    _bc: *mut c_void,
    handler: *const Guid,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    put(out, core::ptr::null_mut());
    let me = this::<ShellItem>(p, 0);
    let h = iid(handler);
    if !me.items.last().is_none_or(Item::is_folder) {
        return E_INVALIDARG;
    }
    if h == BHID_SFOBJECT {
        return folder_as(me.items.clone(), riid, out);
    }
    if h == BHID_ENUMITEMS {
        if iid(riid) != IID_IENUMSHELLITEMS {
            return E_NOINTERFACE;
        }
        let flags = SHCONTF_FOLDERS | SHCONTF_NONFOLDERS | SHCONTF_INCLUDEHIDDEN;
        let mut lists = Vec::new();
        for child in children(&me.items, flags) {
            let mut all = me.items.clone();
            all.push(child);
            lists.push(shellns::encode_list(&all));
        }
        return new_enum(lists, true, out);
    }
    unsupported(b"shell32.dll!IShellItem::BindToHandler", data1(&h));
    E_NOTIMPL
}

/// IShellItem::GetParent: MK_E_NOOBJECT for the desktop.
extern "system" fn item_parent(p: *mut c_void, out: *mut *mut c_void) -> HResult {
    put(out, core::ptr::null_mut());
    let me = this::<ShellItem>(p, 0);
    if me.items.is_empty() {
        return MK_E_NOOBJECT;
    }
    let parent = me.items[..me.items.len() - 1].to_vec();
    item_as(parent, &IID_ISHELLITEM, out)
}

extern "system" fn item_name(p: *mut c_void, sigdn: u32, out: *mut *mut u16) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    let me = this::<ShellItem>(p, 0);
    let Some(name) = name_of(&me.items, sigdn) else { return E_INVALIDARG };
    let s = task_string(&name);
    put(out, s);
    if s.is_null() { E_OUTOFMEMORY } else { S_OK }
}

/// IShellItem::GetAttributes(mask, *attributes): S_OK when every bit
/// asked for is set, else S_FALSE.
extern "system" fn item_attributes(p: *mut c_void, mask: u32, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let a = attributes_of(&this::<ShellItem>(p, 0).items) & mask;
    put(out, a);
    if a == mask { S_OK } else { S_FALSE }
}

/// IShellItem::Compare(other, hint, *order): S_OK when they are the same
/// item, else S_FALSE with the order.
extern "system" fn item_compare(
    p: *mut c_void,
    other: *mut c_void,
    _hint: u32,
    order: *mut i32,
) -> HResult {
    if other.is_null() || order.is_null() {
        return E_INVALIDARG;
    }
    let me = this::<ShellItem>(p, 0);
    let mut list = core::ptr::null_mut();
    if id_list_of(other, &mut list) != S_OK {
        return E_INVALIDARG;
    }
    let theirs = items_of(list);
    super::browser::ILFree(list);
    let Some(theirs) = theirs else { return E_INVALIDARG };
    let mut o = core::cmp::Ordering::Equal;
    for (x, y) in me.items.iter().zip(&theirs) {
        o = shellns::compare_items(x, y);
        if o != core::cmp::Ordering::Equal {
            break;
        }
    }
    if o == core::cmp::Ordering::Equal {
        o = me.items.len().cmp(&theirs.len());
    }
    put(order, o as i32);
    if o == core::cmp::Ordering::Equal { S_OK } else { S_FALSE }
}

static ITEM_VTABLE: VTable<8> = VTable([
    item_qi as *const (),
    item_add_ref as *const (),
    item_release as *const (),
    item_bind as *const (),
    item_parent as *const (),
    item_name as *const (),
    item_attributes as *const (),
    item_compare as *const (),
]);

extern "system" fn pi_qi(p: *mut c_void, riid: *const Guid, out: *mut *mut c_void) -> HResult {
    let me = this::<ShellItem>(p, 1);
    item_qi((me as *mut ShellItem).cast(), riid, out)
}
extern "system" fn pi_add_ref(p: *mut c_void) -> u32 {
    this::<ShellItem>(p, 1).head.refs.fetch_add(1, Ordering::Relaxed) + 1
}
extern "system" fn pi_release(p: *mut c_void) -> u32 {
    item_release((this::<ShellItem>(p, 1) as *mut ShellItem).cast())
}
/// IPersist::GetClassID: CLSID_ShellItem {9AC9FBE1-E0A2-4AD6-B4EE-E212013EA917}.
extern "system" fn pi_class(_p: *mut c_void, out: *mut Guid) -> HResult {
    put(
        out,
        shellns::guid(
            0x9ac9_fbe1,
            0xe0a2,
            0x4ad6,
            [0xb4, 0xee, 0xe2, 0x12, 0x01, 0x3e, 0xa9, 0x17],
        ),
    );
    S_OK
}
extern "system" fn pi_set(p: *mut c_void, list: *const u8) -> HResult {
    match items_of(list) {
        Some(items) => {
            this::<ShellItem>(p, 1).items = items;
            S_OK
        }
        None => E_INVALIDARG,
    }
}
extern "system" fn pi_get(p: *mut c_void, out: *mut *mut u8) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let list = new_list(&this::<ShellItem>(p, 1).items);
    put(out, list);
    if list.is_null() { E_OUTOFMEMORY } else { S_OK }
}

static PERSIST_IDLIST_VTABLE: VTable<6> = VTable([
    pi_qi as *const (),
    pi_add_ref as *const (),
    pi_release as *const (),
    pi_class as *const (),
    pi_set as *const (),
    pi_get as *const (),
]);

/// The absolute list of an object of ours or anyone's (IPersistIDList,
/// else IPersistFolder2).
fn id_list_of(object: *mut c_void, out: *mut *mut u8) -> HResult {
    type Qi = extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> HResult;
    type Get = extern "system" fn(*mut c_void, *mut *mut u8) -> HResult;
    type Release = extern "system" fn(*mut c_void) -> u32;
    // SAFETY: a COM object: its first word is its vtable.
    let vt = |o: *mut c_void| unsafe { *(o as *const *const usize) };
    // SAFETY: IUnknown's first slot.
    let qi: Qi = unsafe { core::mem::transmute::<usize, Qi>(*vt(object)) };
    for (want, slot) in [(IID_IPERSISTIDLIST, 5), (IID_IPERSISTFOLDER2, 5)] {
        let mut i = core::ptr::null_mut();
        if qi(object, &want, &mut i) == S_OK && !i.is_null() {
            // SAFETY: that interface's GetIDList / GetCurFolder slot and
            // Release.
            let (get, release) = unsafe {
                (
                    core::mem::transmute::<usize, Get>(*vt(i).add(slot)),
                    core::mem::transmute::<usize, Release>(*vt(i).add(2)),
                )
            };
            let r = get(i, out);
            release(i);
            return r;
        }
    }
    E_NOINTERFACE
}

// ---- The exported functions --------------------------------------------------------

/// SHGetDesktopFolder: the desktop's IShellFolder.
#[unsafe(no_mangle)]
pub extern "system" fn SHGetDesktopFolder(out: *mut *mut c_void) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    folder_as(Vec::new(), &IID_ISHELLFOLDER, out)
}

/// SHParseDisplayName(name, bc, *list, attributes asked, *attributes).
#[unsafe(no_mangle)]
pub extern "system" fn SHParseDisplayName(
    name: *const u16,
    _bc: *mut c_void,
    out: *mut *mut u8,
    want: u32,
    attributes: *mut u32,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    put(attributes, 0);
    if name.is_null() {
        return E_INVALIDARG;
    }
    let items = match resolve(wstr(name)) {
        Ok(i) => i,
        Err(e) => return e,
    };
    put(attributes, attributes_of(&items) & want);
    let list = new_list(&items);
    put(out, list);
    if list.is_null() { E_OUTOFMEMORY } else { S_OK }
}

/// SHCreateItemFromParsingName(name, bc, riid, *out).
#[unsafe(no_mangle)]
pub extern "system" fn SHCreateItemFromParsingName(
    name: *const u16,
    _bc: *mut c_void,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    if name.is_null() {
        return E_INVALIDARG;
    }
    match resolve(wstr(name)) {
        Ok(items) => item_as(items, riid, out),
        Err(e) => e,
    }
}

/// SHCreateItemFromIDList(list, riid, *out).
#[unsafe(no_mangle)]
pub extern "system" fn SHCreateItemFromIDList(
    list: *const u8,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    put(out, core::ptr::null_mut());
    match items_of(list) {
        Some(items) if !list.is_null() => item_as(items, riid, out),
        _ => E_INVALIDARG,
    }
}

/// SHCreateShellItem(parent list, parent folder, child, *IShellItem): the
/// child of the parent (by list, else the folder's own).
#[unsafe(no_mangle)]
pub extern "system" fn SHCreateShellItem(
    parent: *const u8,
    folder: *mut c_void,
    child: *const u8,
    out: *mut *mut c_void,
) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    let mut items = if !parent.is_null() {
        match items_of(parent) {
            Some(i) => i,
            None => return E_INVALIDARG,
        }
    } else if !folder.is_null() {
        let mut list = core::ptr::null_mut();
        if id_list_of(folder, &mut list) != S_OK {
            return E_INVALIDARG;
        }
        let i = items_of(list);
        super::browser::ILFree(list);
        match i {
            Some(i) => i,
            None => return E_INVALIDARG,
        }
    } else {
        Vec::new()
    };
    match items_of(child) {
        Some(c) => items.extend(c),
        None => return E_INVALIDARG,
    }
    item_as(items, &IID_ISHELLITEM, out)
}

/// SHCreateItemWithParent(parent list, parent folder, child, riid, *out).
#[unsafe(no_mangle)]
pub extern "system" fn SHCreateItemWithParent(
    parent: *const u8,
    folder: *mut c_void,
    child: *const u8,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    let mut item = core::ptr::null_mut();
    let r = SHCreateShellItem(parent, folder, child, &mut item);
    if r != S_OK {
        put(out, core::ptr::null_mut());
        return r;
    }
    let r = item_qi(item, riid, out);
    item_release(item);
    r
}

/// SHGetIDListFromObject(object, *list).
#[unsafe(no_mangle)]
pub extern "system" fn SHGetIDListFromObject(object: *mut c_void, out: *mut *mut u8) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if object.is_null() {
        return E_NOINTERFACE;
    }
    id_list_of(object, out)
}

/// SHGetNameFromIDList(list, sigdn, *name).
#[unsafe(no_mangle)]
pub extern "system" fn SHGetNameFromIDList(
    list: *const u8,
    sigdn: u32,
    out: *mut *mut u16,
) -> HResult {
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    let Some(items) = items_of(list) else { return E_INVALIDARG };
    let Some(name) = name_of(&items, sigdn) else { return E_INVALIDARG };
    let s = task_string(&name);
    put(out, s);
    if s.is_null() { E_OUTOFMEMORY } else { S_OK }
}

/// SHBindToParent(list, riid, *folder, *last): the parent folder and the
/// list's last item (a pointer into `list`).
#[unsafe(no_mangle)]
pub extern "system" fn SHBindToParent(
    list: *const u8,
    riid: *const Guid,
    out: *mut *mut c_void,
    last: *mut *const u8,
) -> HResult {
    put(out, core::ptr::null_mut());
    put(last, core::ptr::null());
    if list.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    let Some(items) = items_of(list) else { return E_INVALIDARG };
    let parent = items[..items.len().saturating_sub(1)].to_vec();
    let r = folder_as(parent, riid, out);
    if r == S_OK {
        put(last, ILFindLastID(list).cast_const());
    }
    r
}

/// The list of a known folder: the desktop's is empty, This PC's its
/// root, the rest their paths' (SHGetKnownFolderPath).
fn known_items(id: &Guid, flags: u32) -> Result<Vec<Item>, HResult> {
    if *id == FOLDERID_DESKTOP {
        return Ok(Vec::new());
    }
    if *id == FOLDERID_COMPUTER {
        return Ok(alloc::vec![Item::Root(CLSID_MY_COMPUTER)]);
    }
    let mut path = core::ptr::null_mut();
    let r = super::SHGetKnownFolderPath(id, flags, 0, &mut path);
    if r != S_OK {
        return Err(r);
    }
    let items = resolve(wstr(path));
    import!("kernel32.dll", "LocalFree", extern "system" fn(*mut u16) -> usize).map(|f| f(path));
    items
}

/// SHGetKnownFolderIDList(id, flags, token, *list).
#[unsafe(no_mangle)]
pub extern "system" fn SHGetKnownFolderIDList(
    id: *const Guid,
    flags: u32,
    _token: usize,
    out: *mut *mut u8,
) -> HResult {
    if id.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    match known_items(&iid(id), flags) {
        Ok(items) => {
            let list = new_list(&items);
            put(out, list);
            if list.is_null() { E_OUTOFMEMORY } else { S_OK }
        }
        Err(e) => e,
    }
}

/// SHGetKnownFolderItem(id, flags, token, riid, *out).
#[unsafe(no_mangle)]
pub extern "system" fn SHGetKnownFolderItem(
    id: *const Guid,
    flags: u32,
    _token: usize,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    if id.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    match known_items(&iid(id), flags) {
        Ok(items) => item_as(items, riid, out),
        Err(e) => e,
    }
}

/// SHGetFolderLocation(hwnd, csidl, token, reserved, *list): as
/// SHGetSpecialFolderLocation.
#[unsafe(no_mangle)]
pub extern "system" fn SHGetFolderLocation(
    hwnd: usize,
    csidl: i32,
    _token: usize,
    _reserved: u32,
    out: *mut *mut u8,
) -> HResult {
    SHGetSpecialFolderLocation(hwnd, csidl, out)
}

/// SHGetSpecialFolderLocation(hwnd, csidl, *list): the desktop's list is
/// empty, This PC (CSIDL_DRIVES) its root, the others their paths'.
#[unsafe(no_mangle)]
pub extern "system" fn SHGetSpecialFolderLocation(
    _hwnd: usize,
    csidl: i32,
    out: *mut *mut u8,
) -> HResult {
    const CSIDL_DESKTOP: i32 = 0;
    const CSIDL_DRIVES: i32 = 0x11;
    if out.is_null() {
        return E_INVALIDARG;
    }
    put(out, core::ptr::null_mut());
    let items = match csidl & 0xff {
        CSIDL_DESKTOP => Vec::new(),
        CSIDL_DRIVES => alloc::vec![Item::Root(CLSID_MY_COMPUTER)],
        c => {
            let mut path = [0u16; super::MAX_PATH];
            if super::SHGetFolderPathW(0, c, 0, 0, path.as_mut_ptr()) != S_OK {
                return E_INVALIDARG;
            }
            // A folder Windows makes with every profile and gala's lacks
            // has its list all the same, as SHGetFolderPathW gives its path
            // (ARCHITECTURE "Other DLLs and programs").
            match resolve(units(&path)) {
                Ok(i) => i,
                Err(_) => match shellns::parse(units(&path)) {
                    Some(i) => i,
                    None => return E_INVALIDARG,
                },
            }
        }
    };
    let list = new_list(&items);
    put(out, list);
    if list.is_null() { E_OUTOFMEMORY } else { S_OK }
}

/// SHGetPathFromIDListW(list, path[MAX_PATH]): the file-system path (the
/// desktop's directory for the empty list); FALSE for a virtual folder
/// or a list gala did not make.
#[unsafe(no_mangle)]
pub extern "system" fn SHGetPathFromIDListW(list: *const u8, path: *mut u16) -> i32 {
    if list.is_null() || path.is_null() {
        return 0;
    }
    put(path, 0);
    let Some(items) = items_of(list) else { return 0 };
    let Some(name) = name_of(&items, SIGDN_FILESYSPATH) else { return 0 };
    if name.is_empty() || name.len() >= super::MAX_PATH {
        return 0;
    }
    // SAFETY: the caller's MAX_PATH buffer.
    unsafe {
        core::ptr::copy_nonoverlapping(name.as_ptr(), path, name.len());
        *path.add(name.len()) = 0;
    }
    1
}

/// SHGetPathFromIDListEx(list, path, cch, flags): the same path into a
/// buffer the caller measures. Word's recent documents delay-load it,
/// and a Word that does not find it dies there.
///
/// As Windows 11 answers it, measured and kept by `shell32_test`: the
/// path and TRUE, with the last error set to 0, for a list that has
/// one; the flags make no difference to a file-system path
/// (GPFIDL_DEFAULT, GPFIDL_ALTNAME, GPFIDL_UNCPRINTER and bits nothing
/// names all answer alike, so gala reads none of them). A buffer one
/// character short of the path and its NUL is FALSE and holds the path
/// cut to `cch - 1` characters -- what fits is written either way, so
/// the caller that ignores the answer reads a path that is not the one
/// it asked for -- and a `cch` of 1 is FALSE and the empty string. A
/// `cch` of 0 writes nothing at all and leaves the last error alone,
/// and so does a NULL list, which is the one way to tell "no room"
/// from "no path": a list with no path (a virtual folder, This PC) is
/// FALSE with the empty string and the last error 0.
#[unsafe(no_mangle)]
pub extern "system" fn SHGetPathFromIDListEx(
    list: *const u8,
    path: *mut u16,
    cch: u32,
    _flags: u32,
) -> i32 {
    // Nowhere to write: the buffer and the last error both stand.
    // (Windows faults on a NULL buffer with room asked for; gala says
    // no instead, which no caller can tell from a fault it survived.)
    if cch == 0 || path.is_null() {
        return 0;
    }
    put(path, 0);
    if list.is_null() {
        return 0;
    }
    let name = items_of(list).and_then(|items| name_of(&items, SIGDN_FILESYSPATH));
    // SAFETY: plain call.
    unsafe { SetLastError(0) };
    let Some(name) = name else { return 0 };
    if name.is_empty() {
        return 0;
    }
    // What fits, whether or not all of it does.
    let room = cch as usize - 1;
    let n = name.len().min(room);
    // SAFETY: the caller's buffer of `cch` units, and `n` < `cch`.
    unsafe {
        core::ptr::copy_nonoverlapping(name.as_ptr(), path, n);
        *path.add(n) = 0;
    }
    i32::from(n == name.len())
}

// ---- IPropertyStore of an item (SHGetPropertyStoreFromParsingName) ------------

/// A PROPERTYKEY: a property set's GUID and the property's number in it.
type PropKey = (Guid, u32);

const IID_IPROPERTYSTORE: Guid =
    shellns::guid(0x886d_8eeb, 0x8cf2, 0x4446, [0x8d, 0x02, 0xcd, 0xba, 0x1d, 0xbd, 0xcf, 0x99]);

/// The property sets the store answers from.
const PSET_STORAGE: Guid =
    shellns::guid(0xb725_f130, 0x47ef, 0x101a, [0xa5, 0xf1, 0x02, 0x60, 0x8c, 0x9e, 0xeb, 0xac]);
const PSET_SHELL_DETAILS: Guid =
    shellns::guid(0x2863_6aa6, 0x953d, 0x11d2, [0xb5, 0xd6, 0x00, 0xc0, 0x4f, 0xd9, 0x18, 0xd0]);
const PSET_FILE_NAME: Guid =
    shellns::guid(0x41cf_5ae0, 0xf75a, 0x4806, [0xbd, 0x87, 0x59, 0xc7, 0xd9, 0x24, 0x8e, 0xb9]);
const PSET_FOLDER_PATH: Guid =
    shellns::guid(0xe3e0_584c, 0xb788, 0x4a5a, [0xbb, 0x20, 0x7f, 0x5a, 0x44, 0xc9, 0xac, 0xdd]);
const PSET_SUMMARY: Guid =
    shellns::guid(0xf29f_85e0, 0x4ff9, 0x1068, [0xab, 0x91, 0x08, 0x00, 0x2b, 0x27, 0xb3, 0xd9]);

/// The properties the store answers.
const PK_ITEM_TYPE_TEXT: PropKey = (PSET_STORAGE, 4);
const PK_ITEM_NAME_DISPLAY: PropKey = (PSET_STORAGE, 10);
const PK_SIZE: PropKey = (PSET_STORAGE, 12);
const PK_FILE_ATTRIBUTES: PropKey = (PSET_STORAGE, 13);
const PK_DATE_MODIFIED: PropKey = (PSET_STORAGE, 14);
const PK_DATE_CREATED: PropKey = (PSET_STORAGE, 15);
const PK_DATE_ACCESSED: PropKey = (PSET_STORAGE, 16);
const PK_ITEM_TYPE: PropKey = (PSET_SHELL_DETAILS, 11);
const PK_FILE_NAME: PropKey = (PSET_FILE_NAME, 100);
const PK_ITEM_FOLDER_PATH_DISPLAY: PropKey = (PSET_FOLDER_PATH, 6);

/// The properties Windows answers VT_EMPTY for as well, which a program
/// asking after them has not been refused anything by: they are not
/// recorded. System.ParsingPath is one of them (the store of a file as
/// much as of a folder has no value for it) and System.Title is another
/// (it comes from a handler for the file's format, which is what
/// GPS_HANDLERPROPERTIESONLY asks for).
const EMPTY_KEYS: &[PropKey] = &[(PSET_STORAGE, 30), (PSET_SUMMARY, 2)];

/// GETPROPERTYSTOREFLAGS.
const GPS_HANDLERPROPERTIESONLY: u32 = 0x1;
const GPS_READWRITE: u32 = 0x2;
const GPS_TEMPORARY: u32 = 0x4;
const GPS_EXTRINSICPROPERTIESONLY: u32 = 0x400;
const GPS_VOLATILEPROPERTIESONLY: u32 = 0x1000;
const GPS_MASK_VALID: u32 = 0x1fff;

const STG_E_ACCESSDENIED: HResult = 0x8003_0005u32 as i32;

/// What a property the store answers holds, as the variant GetValue
/// writes for it.
enum PropValue {
    /// VT_LPWSTR, a string from the task allocator.
    Str(Vec<u16>),
    /// VT_UI4, FILE_ATTRIBUTE_*.
    Attributes(u32),
    /// VT_UI8, a file's size in bytes.
    Size(u64),
    /// VT_FILETIME, a time as the file system keeps it.
    Time(u64),
}

/// The read-only property store of one item.
#[repr(C)]
struct PropStore {
    head: Head<1>,
    /// The properties it has, in the order GetAt gives them.
    props: Vec<(PropKey, PropValue)>,
}

const STORE_IIDS: &[(Guid, usize)] = &[(IID_IPROPERTYSTORE, 0)];

/// One of FindFirstFileW's times (`at`: 0 created, 2 accessed, 4
/// written), as the 64-bit FILETIME a VT_FILETIME holds.
fn file_time(times: &[u32; 6], at: usize) -> u64 {
    u64::from(times[at]) | u64::from(times[at + 1]) << 32
}

/// The properties of `items`: what the shell answers for a file, a
/// folder, a drive or a virtual folder. A file's size and times are the
/// file system's (FindFirstFileW, the same reading SHGetFileInfoW and
/// the enumerators take), so a program that compares them with its own
/// find data sees the same numbers.
fn props_of(items: &[Item]) -> Vec<(PropKey, PropValue)> {
    let mut props: Vec<(PropKey, PropValue)> = Vec::new();
    if let Some(name) = name_of(items, SIGDN_NORMALDISPLAY) {
        props.push((PK_ITEM_NAME_DISPLAY, PropValue::Str(name)));
    }
    match items.last() {
        // The desktop and the roots below it: a folder with no file of
        // its own, which only its name and its kind are answered for.
        None | Some(Item::Root(_)) => {
            props.push((
                PK_ITEM_TYPE_TEXT,
                PropValue::Str("System Folder".encode_utf16().collect()),
            ));
        }
        Some(Item::Drive(_)) => {
            props.push((PK_ITEM_TYPE_TEXT, PropValue::Str(type_name(true, 0, None))));
            props.push((PK_FILE_NAME, PropValue::Str(shellns::parsing_name(items))));
        }
        Some(Item::Distro(name)) => {
            props.push((
                PK_ITEM_TYPE_TEXT,
                PropValue::Str(type_name(false, FILE_ATTRIBUTE_DIRECTORY, None)),
            ));
            props.push((PK_FILE_NAME, PropValue::Str(name.clone())));
        }
        Some(Item::Fs { attributes, name }) => {
            let directory = attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
            let ext = extension(name);
            let kind = if directory {
                Some("Directory".encode_utf16().collect::<Vec<u16>>())
            } else {
                ext.filter(|e| !e.is_empty())
                    .map(|e| core::iter::once(u16::from(b'.')).chain(e.iter().copied()).collect())
            };
            if let Some(kind) = kind {
                props.push((PK_ITEM_TYPE, PropValue::Str(kind)));
            }
            props.push((PK_ITEM_TYPE_TEXT, PropValue::Str(type_name(false, *attributes, ext))));
            props.push((PK_FILE_NAME, PropValue::Str(name.clone())));
            if let Some(parent) = shellns::fs_path(&items[..items.len() - 1]) {
                props.push((PK_ITEM_FOLDER_PATH_DISPLAY, PropValue::Str(parent)));
            }
            props.push((PK_FILE_ATTRIBUTES, PropValue::Attributes(*attributes)));
            if let Some(fd) = shellns::fs_path(items).and_then(|p| find_one(&p).ok()) {
                if !directory {
                    let size = u64::from(fd.size_high) << 32 | u64::from(fd.size_low);
                    props.push((PK_SIZE, PropValue::Size(size)));
                }
                props.push((PK_DATE_MODIFIED, PropValue::Time(file_time(&fd.times, 4))));
                props.push((PK_DATE_CREATED, PropValue::Time(file_time(&fd.times, 0))));
                props.push((PK_DATE_ACCESSED, PropValue::Time(file_time(&fd.times, 2))));
            }
        }
    }
    props
}

/// A new store of `items`, as interface `riid`.
fn store_as(items: &[Item], riid: *const Guid, out: *mut *mut c_void) -> HResult {
    let s = boxed(PropStore {
        head: Head {
            vtables: [(&STORE_VTABLE as *const VTable<8>).cast()],
            refs: AtomicU32::new(1),
        },
        props: props_of(items),
    });
    if s.is_null() {
        return E_OUTOFMEMORY;
    }
    // SAFETY: the new object.
    let me = unsafe { &mut *s };
    let r = query(
        s.cast(),
        &me.head.refs,
        STORE_IIDS,
        riid,
        out,
        b"shell32.dll!IPropertyStore(QueryInterface)",
    );
    store_release(s.cast());
    r
}

extern "system" fn store_qi(p: *mut c_void, riid: *const Guid, out: *mut *mut c_void) -> HResult {
    let me = this::<PropStore>(p, 0);
    let base = (me as *mut PropStore).cast();
    query(base, &me.head.refs, STORE_IIDS, riid, out, b"shell32.dll!IPropertyStore(QueryInterface)")
}
extern "system" fn store_add_ref(p: *mut c_void) -> u32 {
    this::<PropStore>(p, 0).head.refs.fetch_add(1, Ordering::Relaxed) + 1
}
extern "system" fn store_release(p: *mut c_void) -> u32 {
    let me = this::<PropStore>(p, 0);
    let left = me.head.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if left == 0 {
        free(me as *mut PropStore);
    }
    left
}

/// IPropertyStore::GetCount(*count): how many properties the item has
/// (on Windows the shell's whole list for its kind, here the ones gala
/// answers).
extern "system" fn store_get_count(p: *mut c_void, out: *mut u32) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    put(out, this::<PropStore>(p, 0).props.len() as u32);
    S_OK
}

/// IPropertyStore::GetAt(index, *key): the key at `index` (E_FAIL past
/// the last one).
extern "system" fn store_get_at(p: *mut c_void, index: u32, out: *mut u8) -> HResult {
    if out.is_null() {
        return E_POINTER;
    }
    match this::<PropStore>(p, 0).props.get(index as usize) {
        Some(((set, pid), _)) => {
            // SAFETY: the caller's PROPERTYKEY (16 bytes of FMTID, then
            // the property's number, neither of them aligned here).
            unsafe {
                core::ptr::copy_nonoverlapping(set.as_ptr(), out, 16);
                out.add(16).cast::<u32>().write_unaligned(*pid);
            }
            S_OK
        }
        None => E_FAIL,
    }
}

/// The key the caller passed (a PROPERTYKEY: 20 bytes, unaligned).
fn read_key(key: *const u8) -> PropKey {
    let mut set = [0u8; 16];
    // SAFETY: the caller's PROPERTYKEY.
    let pid = unsafe {
        core::ptr::copy_nonoverlapping(key, set.as_mut_ptr(), 16);
        key.add(16).cast::<u32>().read_unaligned()
    };
    (set, pid)
}

/// IPropertyStore::GetValue(key, *value): the property's value, or S_OK
/// with VT_EMPTY for one the item has not got -- which is what Windows
/// answers for a key nothing names, and what gala answers for a
/// property it does not keep. A key of the second kind is recorded
/// (once per key, the set's first word and the property's number as the
/// detail), so that a program which wanted a property gala has not got
/// says so in the ledger instead of quietly reading nothing.
extern "system" fn store_get_value(p: *mut c_void, key: *const u8, pv: *mut u8) -> HResult {
    if key.is_null() || pv.is_null() {
        return E_POINTER;
    }
    write_variant(pv, VT_EMPTY, 0);
    let want = read_key(key);
    let me = this::<PropStore>(p, 0);
    let Some((_, value)) = me.props.iter().find(|(k, _)| *k == want) else {
        if !EMPTY_KEYS.contains(&want) {
            unsupported(
                b"shell32.dll!IPropertyStore::GetValue(property)",
                data1(&want.0) << 32 | u64::from(want.1),
            );
        }
        return S_OK;
    };
    match value {
        PropValue::Str(s) => {
            let text = task_string(s);
            if text.is_null() {
                return E_OUTOFMEMORY;
            }
            write_variant(pv, VT_LPWSTR, text as u64);
        }
        PropValue::Attributes(a) => write_variant(pv, VT_UI4, u64::from(*a)),
        PropValue::Size(n) => write_variant(pv, VT_UI8, *n),
        PropValue::Time(t) => write_variant(pv, VT_FILETIME, *t),
    }
    S_OK
}

/// IPropertyStore::SetValue(key, value) and Commit(): the item's own
/// store is read-only, as on Windows.
extern "system" fn store_set_value(_p: *mut c_void, _key: *const u8, _pv: *const u8) -> HResult {
    STG_E_ACCESSDENIED
}
extern "system" fn store_commit(_p: *mut c_void) -> HResult {
    STG_E_ACCESSDENIED
}

static STORE_VTABLE: VTable<8> = VTable([
    store_qi as *const (),
    store_add_ref as *const (),
    store_release as *const (),
    store_get_count as *const (),
    store_get_at as *const (),
    store_get_value as *const (),
    store_set_value as *const (),
    store_commit as *const (),
]);

/// SHGetPropertyStoreFromParsingName(name, bind context, flags, riid,
/// *out): the read-only IPropertyStore of the item a desktop-absolute
/// parsing name names -- a file, a folder, a drive (with or without its
/// trailing separator) or the desktop itself for the empty name -- and
/// the bind context makes no difference. The store answers
/// System.ItemNameDisplay, System.FileName,
/// System.ItemFolderPathDisplay, System.ItemType (the extension with
/// its dot, "Directory" for a folder), System.ItemTypeText, System.Size
/// (a file's only), System.FileAttributes and the three times, of which
/// System.DateModified is FindFirstFileW's ftLastWriteTime to the tick.
/// GetCount and GetAt walk those, E_FAIL past the last; a property the
/// item has not got is S_OK with VT_EMPTY; SetValue and Commit are
/// STG_E_ACCESSDENIED. A name that is not there is
/// HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND) and a name that is no
/// parsing name is E_INVALIDARG, with *out NULL either way.
/// GPS_HANDLERPROPERTIESONLY and GPS_READWRITE want a property
/// handler's store, which is the file format's business and nothing gala
/// has (E_FAIL for a file, E_NOINTERFACE for a folder, as on Windows),
/// and GPS_EXTRINSICPROPERTIESONLY wants the shell's extrinsic one
/// (E_FAIL for both).
///
/// Limits, each recorded rather than worked around (ARCHITECTURE "The
/// file explorer", ROADMAP EX): GPS_TEMPORARY and
/// GPS_VOLATILEPROPERTIESONLY (and so GPS_MASK_VALID, which holds the
/// first) ask for an empty store of their own, writable in the one case
/// and not in the other, which gala has not got and answers E_NOTIMPL
/// for; System.ItemTypeText is `fileinfo::type_name`, not the registry's
/// description of the extension; a relative name is no parsing name
/// here, where Windows reads one against the desktop folder, and a DOS
/// device name is E_INVALIDARG rather than
/// HRESULT_FROM_WIN32(ERROR_BAD_PATHNAME); the call does not want an
/// initialized apartment, where Windows answers CO_E_NOTINITIALIZED
/// without one; and a riid of IPropertyStoreCapabilities, which Windows
/// answers, is E_NOINTERFACE and a row of its own. The first call is
/// recorded with its flags, so that a program which stops after it
/// leaves behind what it asked for.
#[unsafe(no_mangle)]
pub extern "system" fn SHGetPropertyStoreFromParsingName(
    name: *const u16,
    _bc: *mut c_void,
    flags: u32,
    riid: *const Guid,
    out: *mut *mut c_void,
) -> HResult {
    // (Windows faults on a NULL out parameter; gala says no instead,
    // which no caller can tell from a fault it survived.)
    if out.is_null() {
        return E_POINTER;
    }
    put(out, core::ptr::null_mut());
    if name.is_null() {
        return E_INVALIDARG;
    }
    reached(b"shell32.dll!SHGetPropertyStoreFromParsingName(flags)", u64::from(flags));
    if flags & !GPS_MASK_VALID != 0 {
        return E_INVALIDARG;
    }
    let items = match resolve(wstr(name)) {
        Ok(items) => items,
        Err(e) => return e,
    };
    if flags & (GPS_HANDLERPROPERTIESONLY | GPS_READWRITE) != 0 {
        let folder = items.last().is_none_or(Item::is_folder);
        return if folder { E_NOINTERFACE } else { E_FAIL };
    }
    if flags & GPS_EXTRINSICPROPERTIESONLY != 0 {
        return E_FAIL;
    }
    if flags & (GPS_TEMPORARY | GPS_VOLATILEPROPERTIESONLY) != 0 {
        unsupported(
            b"shell32.dll!SHGetPropertyStoreFromParsingName(store of its own)",
            u64::from(flags),
        );
        return E_NOTIMPL;
    }
    store_as(&items, riid, out)
}

/// ILCreateFromPathW(path): the list of an existing path (NULL otherwise).
pub fn list_from_path(path: &[u16]) -> *mut u8 {
    match resolve(path) {
        Ok(items) if !path.is_empty() => new_list(&items),
        _ => core::ptr::null_mut(),
    }
}

/// The list of `path` without looking at the file system (its last item
/// a file); NULL for a path outside the namespace.
pub fn simple_list(path: &[u16]) -> *mut u8 {
    let Some(mut items) = shellns::parse(path) else { return core::ptr::null_mut() };
    if let Some(Item::Fs { attributes, .. }) = items.last_mut() {
        *attributes = 0;
    }
    new_list(&items)
}

/// SHSimpleIDListFromPath(path): the list of a path, whether or not it
/// exists (its last item a file).
#[unsafe(no_mangle)]
pub extern "system" fn SHSimpleIDListFromPath(path: *const u16) -> *mut u8 {
    simple_list(wstr(path))
}

// ---- Item ID list functions (on the bytes: any list) ----------------------------------

/// ILClone(list): a copy.
#[unsafe(no_mangle)]
pub extern "system" fn ILClone(list: *const u8) -> *mut u8 {
    if list.is_null() {
        return core::ptr::null_mut();
    }
    heap_copy(list_bytes(list))
}

/// ILCloneFirst(list): a list of its first item.
#[unsafe(no_mangle)]
pub extern "system" fn ILCloneFirst(list: *const u8) -> *mut u8 {
    let b = list_bytes(list);
    if b.len() < 2 {
        return core::ptr::null_mut();
    }
    let cb = usize::from(u16::from_le_bytes([b[0], b[1]]));
    let mut v = Vec::new();
    v.extend_from_slice(&b[..cb]);
    v.extend_from_slice(&[0, 0]);
    heap_copy(&v)
}

/// ILCombine(a, b): `a`'s items then `b`'s.
#[unsafe(no_mangle)]
pub extern "system" fn ILCombine(a: *const u8, b: *const u8) -> *mut u8 {
    if a.is_null() && b.is_null() {
        return core::ptr::null_mut();
    }
    let (x, y) = (list_bytes(a), list_bytes(b));
    let mut v = Vec::new();
    v.extend_from_slice(&x[..x.len().saturating_sub(2)]);
    v.extend_from_slice(&y[..y.len().saturating_sub(2)]);
    v.extend_from_slice(&[0, 0]);
    heap_copy(&v)
}

/// ILFindLastID(list): its last item (the terminator for the empty list).
#[unsafe(no_mangle)]
pub extern "system" fn ILFindLastID(list: *const u8) -> *mut u8 {
    if list.is_null() {
        return core::ptr::null_mut();
    }
    let b = list_bytes(list);
    let (mut at, mut last) = (0usize, 0usize);
    while at + 2 <= b.len() {
        let cb = usize::from(u16::from_le_bytes([b[at], b[at + 1]]));
        if cb == 0 {
            break;
        }
        last = at;
        at += cb;
    }
    list.wrapping_add(last).cast_mut()
}

/// ILRemoveLastID(list): its last item cut off in place (FALSE for the
/// empty list).
#[unsafe(no_mangle)]
pub extern "system" fn ILRemoveLastID(list: *mut u8) -> i32 {
    if list.is_null() || list_bytes(list).len() <= 2 {
        return 0;
    }
    let last = ILFindLastID(list);
    // SAFETY: the last item's cb, inside the caller's list.
    unsafe { last.cast::<u16>().write_unaligned(0) };
    1
}

/// ILIsEqual(a, b): the same items.
#[unsafe(no_mangle)]
pub extern "system" fn ILIsEqual(a: *const u8, b: *const u8) -> i32 {
    if a.is_null() || b.is_null() {
        return 0;
    }
    i32::from(list_bytes(a) == list_bytes(b))
}

/// ILGetNext(item): the next item (NULL after the last).
#[unsafe(no_mangle)]
pub extern "system" fn ILGetNext(list: *const u8) -> *mut u8 {
    if list.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: the caller's item's cb.
    let cb = usize::from(unsafe { list.cast::<u16>().read_unaligned() });
    if cb == 0 {
        return core::ptr::null_mut();
    }
    list.wrapping_add(cb).cast_mut()
}

/// ILIsParent(parent, child, immediate): whether `child` is below
/// `parent` (directly, if `immediate`).
#[unsafe(no_mangle)]
pub extern "system" fn ILIsParent(parent: *const u8, child: *const u8, immediate: i32) -> i32 {
    let rest = ILFindChild(parent, child);
    if rest.is_null() {
        return 0;
    }
    let rest = list_bytes(rest);
    if rest.len() <= 2 {
        return 0;
    }
    let first = usize::from(u16::from_le_bytes([rest[0], rest[1]]));
    i32::from(immediate == 0 || rest.len() == first + 2)
}

/// ILFindChild(parent, child): the part of `child` below `parent` (NULL
/// when it is not below it).
#[unsafe(no_mangle)]
pub extern "system" fn ILFindChild(parent: *const u8, child: *const u8) -> *mut u8 {
    if parent.is_null() || child.is_null() {
        return core::ptr::null_mut();
    }
    let (p, c) = (list_bytes(parent), list_bytes(child));
    let head = &p[..p.len() - 2];
    if c.len() < head.len() + 2 || &c[..head.len()] != head {
        return core::ptr::null_mut();
    }
    child.wrapping_add(head.len()).cast_mut()
}

/// ILAppendID(list, item, at end): a new list with the item added (the
/// old one freed).
#[unsafe(no_mangle)]
pub extern "system" fn ILAppendID(list: *mut u8, item: *const u8, end: i32) -> *mut u8 {
    let mut one = Vec::new();
    if !item.is_null() {
        // SAFETY: the caller's item's cb.
        let cb = usize::from(unsafe { item.cast::<u16>().read_unaligned() });
        // SAFETY: its `cb` bytes.
        one.extend_from_slice(unsafe { core::slice::from_raw_parts(item, cb) });
    }
    one.extend_from_slice(&[0, 0]);
    let joined =
        if end != 0 { ILCombine(list, one.as_ptr()) } else { ILCombine(one.as_ptr(), list) };
    super::browser::ILFree(list);
    joined
}
