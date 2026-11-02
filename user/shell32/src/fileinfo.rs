//! SHGetFileInfoW and the system image lists: two comctl32 image lists
//! (small and large icons, made on first use) of gala's own file icons
//! (`assets/*.svg`, baked by `cargo xtask bake-icons` to 32-bit pixels
//! with straight alpha): a document, a closed and an open folder, a
//! program and a drive. Without SHGFI_USEFILEATTRIBUTES the file's
//! attributes are read (GetFileAttributesW). Type names are "File
//! folder", "Application" for .exe, "<EXT> File" otherwise (no registry
//! of file types yet).

use core::ffi::c_void;
use core::sync::atomic::{AtomicUsize, Ordering};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> *const c_void;
}

const SHGFI_ICON: u32 = 0x100;
const SHGFI_DISPLAYNAME: u32 = 0x200;
const SHGFI_TYPENAME: u32 = 0x400;
const SHGFI_SYSICONINDEX: u32 = 0x4000;
const SHGFI_SMALLICON: u32 = 0x1;
const SHGFI_OPENICON: u32 = 0x2;
const SHGFI_USEFILEATTRIBUTES: u32 = 0x10;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const INVALID_FILE_ATTRIBUTES: u32 = 0xffff_ffff;

/// The images, in their order in the system image lists.
const DOCUMENT: i32 = 0;
const FOLDER: i32 = 1;
const FOLDER_OPEN: i32 = 2;
const PROGRAM: i32 = 3;
const DRIVE: i32 = 4;
static ICONS: [[&[u8]; 2]; 5] = [
    [include_bytes!("../assets/document-16.bgra"), include_bytes!("../assets/document-32.bgra")],
    [include_bytes!("../assets/folder-16.bgra"), include_bytes!("../assets/folder-32.bgra")],
    [
        include_bytes!("../assets/folder-open-16.bgra"),
        include_bytes!("../assets/folder-open-32.bgra"),
    ],
    [include_bytes!("../assets/program-16.bgra"), include_bytes!("../assets/program-32.bgra")],
    [include_bytes!("../assets/drive-16.bgra"), include_bytes!("../assets/drive-32.bgra")],
];

/// SHFILEINFOW.
#[repr(C)]
pub struct ShFileInfoW {
    icon: usize,
    index: i32,
    attributes: u32,
    display_name: [u16; 260],
    type_name: [u16; 80],
}

/// `dll`'s function `name` (both NUL-terminated), resolved once per call
/// site (shell32 does not import the GUI DLLs: programs without windows
/// use it too).
fn resolve(cache: &AtomicUsize, dll: &[u8], name: &[u8]) -> usize {
    let f = cache.load(Ordering::Acquire);
    if f != 0 {
        return f;
    }
    let mut wide = [0u16; 16];
    for (w, &b) in wide.iter_mut().zip(dll) {
        *w = u16::from(b);
    }
    // SAFETY: NUL-terminated names.
    let f = unsafe {
        let m = LoadLibraryW(wide.as_ptr());
        if m == 0 { 0 } else { GetProcAddress(m, name.as_ptr()) as usize }
    };
    cache.store(f, Ordering::Release);
    f
}

macro_rules! import {
    ($dll:literal, $name:literal, $ty:ty) => {{
        static CACHE: AtomicUsize = AtomicUsize::new(0);
        let f = resolve(&CACHE, concat!($dll, "\0").as_bytes(), concat!($name, "\0").as_bytes());
        // SAFETY: that DLL's function of that name has this signature.
        (f != 0).then(|| unsafe { core::mem::transmute::<usize, $ty>(f) })
    }};
}

/// The system image list for small (16 px) or large (32 px) icons, made
/// on first use; 0 if comctl32 or gdi32 is missing.
fn system_list(small: bool) -> usize {
    static LISTS: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];
    let slot = &LISTS[usize::from(!small)];
    let h = slot.load(Ordering::Acquire);
    if h != 0 {
        return h;
    }
    type Create = extern "system" fn(i32, i32, u32, i32, i32) -> usize;
    type CreateDib = extern "system" fn(usize, *const u32, u32, *mut *mut u8, usize, u32) -> usize;
    let (Some(create), Some(add), Some(destroy), Some(dib), Some(delete)) = (
        import!("comctl32.dll", "ImageList_Create", Create),
        import!("comctl32.dll", "ImageList_Add", extern "system" fn(usize, usize, usize) -> i32),
        import!("comctl32.dll", "ImageList_Destroy", extern "system" fn(usize) -> i32),
        import!("gdi32.dll", "CreateDIBSection", CreateDib),
        import!("gdi32.dll", "DeleteObject", extern "system" fn(usize) -> i32),
    ) else {
        return 0;
    };
    let size: i32 = if small { 16 } else { 32 };
    // ILC_COLOR32 | ILC_MASK.
    let list = create(size, size, 0x20 | 1, ICONS.len() as i32, 4);
    if list == 0 {
        return 0;
    }
    for icon in &ICONS {
        let pixels = icon[usize::from(!small)];
        // BITMAPINFOHEADER: 40 bytes, size x -size (top-down), 1 plane,
        // 32 bits, BI_RGB.
        let header: [u32; 10] = [40, size as u32, (-size) as u32, 1 | 32 << 16, 0, 0, 0, 0, 0, 0];
        let mut bits: *mut u8 = core::ptr::null_mut();
        let b = dib(0, header.as_ptr(), 0, &mut bits, 0, 0);
        if b == 0 || bits.is_null() {
            destroy(list);
            return 0;
        }
        // SAFETY: the DIB holds size * size * 4 bytes, as the icon does.
        unsafe { core::ptr::copy_nonoverlapping(pixels.as_ptr(), bits, pixels.len()) };
        add(list, b, 0);
        delete(b);
    }
    match slot.compare_exchange(0, list, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => list,
        Err(other) => {
            destroy(list);
            other
        }
    }
}

/// The path's characters before its NUL (at most 32767).
fn path_of(path: *const u16) -> &'static [u16] {
    if path.is_null() {
        return &[];
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    while n < 32767 && unsafe { *path.add(n) } != 0 {
        n += 1;
    }
    // SAFETY: `n` characters were read.
    unsafe { core::slice::from_raw_parts(path, n) }
}

/// The name part: after the last `\` or `/`.
fn name_part(path: &[u16]) -> &[u16] {
    let start = path.iter().rposition(|&c| c == u16::from(b'\\') || c == u16::from(b'/'));
    &path[start.map_or(0, |i| i + 1)..]
}

fn copy(out: &mut [u16], s: impl Iterator<Item = u16>) {
    let mut n = 0;
    for c in s.take(out.len() - 1) {
        out[n] = c;
        n += 1;
    }
    out[n] = 0;
}

/// SHGetFileInfoW(path, attributes, info, size, flags): see the module
/// comment. With SHGFI_SYSICONINDEX, the system image list (nonzero);
/// otherwise nonzero on success.
#[unsafe(no_mangle)]
pub extern "system" fn SHGetFileInfoW(
    path: *const u16,
    attributes: u32,
    info: *mut ShFileInfoW,
    size: u32,
    flags: u32,
) -> usize {
    let path = path_of(path);
    let attributes = if flags & SHGFI_USEFILEATTRIBUTES != 0 {
        attributes
    } else {
        let mut z = [0u16; 261];
        if path.len() >= z.len() {
            return 0;
        }
        z[..path.len()].copy_from_slice(path);
        // SAFETY: a NUL-terminated path.
        let a = unsafe { super::GetFileAttributesW(z.as_ptr()) };
        if a == INVALID_FILE_ATTRIBUTES {
            return 0;
        }
        a
    };
    let name = name_part(path);
    let ext = name.iter().rposition(|&c| c == u16::from(b'.')).map(|i| &name[i + 1..]);
    let lower = |c: u16| if (0x41..=0x5a).contains(&c) { c + 32 } else { c };
    let is_exe =
        ext.is_some_and(|e| e.len() == 3 && e.iter().map(|&c| lower(c)).eq("exe".encode_utf16()));
    let is_drive = path.len() <= 3 && path.get(1) == Some(&u16::from(b':'));
    let index = if is_drive {
        DRIVE
    } else if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        if flags & SHGFI_OPENICON != 0 { FOLDER_OPEN } else { FOLDER }
    } else if is_exe {
        PROGRAM
    } else {
        DOCUMENT
    };
    let small = flags & SHGFI_SMALLICON != 0;
    let wants_list = flags & (SHGFI_SYSICONINDEX | SHGFI_ICON) != 0;
    let list = if wants_list { system_list(small) } else { 0 };
    if wants_list && list == 0 {
        return 0;
    }
    if !info.is_null() && size as usize >= core::mem::size_of::<ShFileInfoW>() {
        // SAFETY: the caller's SHFILEINFOW.
        let fi = unsafe { &mut *info };
        if flags & SHGFI_SYSICONINDEX != 0 {
            fi.index = index;
        }
        if flags & SHGFI_ICON != 0 {
            fi.index = index;
            fi.icon = match import!(
                "comctl32.dll",
                "ImageList_GetIcon",
                extern "system" fn(usize, i32, u32) -> usize
            ) {
                Some(get) => get(list, index, 0),
                None => 0,
            };
        }
        if flags & SHGFI_DISPLAYNAME != 0 {
            copy(&mut fi.display_name, name.iter().copied());
        }
        if flags & SHGFI_TYPENAME != 0 {
            let upper = |c: u16| if (0x61..=0x7a).contains(&c) { c - 32 } else { c };
            if is_drive {
                copy(&mut fi.type_name, "Local Disk".encode_utf16());
            } else if attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                copy(&mut fi.type_name, "File folder".encode_utf16());
            } else if is_exe {
                copy(&mut fi.type_name, "Application".encode_utf16());
            } else if let Some(e) = ext.filter(|e| !e.is_empty()) {
                copy(&mut fi.type_name, e.iter().map(|&c| upper(c)).chain(" File".encode_utf16()));
            } else {
                copy(&mut fi.type_name, "File".encode_utf16());
            }
        }
    }
    if flags & SHGFI_SYSICONINDEX != 0 { list } else { 1 }
}

// ---- Item ID lists ----
//
// gala's item IDs: one SHITEMID per list (cb, then b"gala", a kind and,
// for a folder on disk, its path in UTF-16 without a NUL), then the
// terminating zero cb. The desktop, the namespace's root, is the empty
// list. Lists come from the process heap, as CoTaskMemAlloc's do.

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
}

const KIND_PATH: u16 = 1;
const KIND_DRIVES: u16 = 2;
const CSIDL_DESKTOP: i32 = 0;
const CSIDL_DRIVES: i32 = 0x11;
const CSIDL_DESKTOPDIRECTORY: i32 = 0x10;

/// A list of one item of `kind` with `path`, or the empty list.
fn new_list(item: Option<(u16, &[u16])>) -> *mut u8 {
    let body = item.map_or(0, |(_, p)| 2 + 4 + 2 + 2 * p.len());
    // SAFETY: plain call.
    let list = unsafe { HeapAlloc(GetProcessHeap(), 8, body + 2) }; // HEAP_ZERO_MEMORY
    if list.is_null() {
        return list;
    }
    if let Some((kind, path)) = item {
        // SAFETY: `body + 2` bytes, zeroed (so the terminator is there).
        unsafe {
            list.cast::<u16>().write_unaligned(body as u16);
            core::ptr::copy_nonoverlapping(b"gala".as_ptr(), list.add(2), 4);
            list.add(6).cast::<u16>().write_unaligned(kind);
            for (i, &c) in path.iter().enumerate() {
                list.add(8 + 2 * i).cast::<u16>().write_unaligned(c);
            }
        }
    }
    list
}

/// SHGetSpecialFolderLocation(hwnd, csidl, &list): the folder's item ID
/// list (free it with CoTaskMemFree): the desktop's is empty, My
/// Computer (CSIDL_DRIVES) is virtual, the others are their paths.
#[unsafe(no_mangle)]
pub extern "system" fn SHGetSpecialFolderLocation(
    _hwnd: usize,
    csidl: i32,
    out: *mut *mut u8,
) -> i32 {
    if out.is_null() {
        return super::E_INVALIDARG;
    }
    let list = match csidl & 0xff {
        CSIDL_DESKTOP => new_list(None),
        CSIDL_DRIVES => new_list(Some((KIND_DRIVES, &[]))),
        c => {
            let mut path = [0u16; super::MAX_PATH];
            if super::SHGetFolderPathW(0, c, 0, 0, path.as_mut_ptr()) != super::S_OK {
                // SAFETY: the caller's pointer.
                unsafe { *out = core::ptr::null_mut() };
                return super::E_INVALIDARG;
            }
            let n = path.iter().position(|&ch| ch == 0).unwrap_or(0);
            new_list(Some((KIND_PATH, &path[..n])))
        }
    };
    // SAFETY: the caller's pointer.
    unsafe { *out = list };
    if list.is_null() { super::E_OUTOFMEMORY } else { super::S_OK }
}

/// SHGetPathFromIDListW(list, path[MAX_PATH]): the folder's path (the
/// desktop's directory for the empty list); FALSE for a virtual folder or
/// a list gala did not make.
#[unsafe(no_mangle)]
pub extern "system" fn SHGetPathFromIDListW(list: *const u8, path: *mut u16) -> i32 {
    if list.is_null() || path.is_null() {
        return 0;
    }
    // SAFETY: the caller's item ID list (cb first).
    let cb = usize::from(unsafe { list.cast::<u16>().read_unaligned() });
    if cb == 0 {
        // SAFETY: the caller's MAX_PATH buffer.
        unsafe { *path = 0 };
        return i32::from(
            super::SHGetFolderPathW(0, CSIDL_DESKTOPDIRECTORY, 0, 0, path) == super::S_OK,
        );
    }
    // SAFETY: the item's `cb` bytes.
    let item = unsafe { core::slice::from_raw_parts(list, cb) };
    if cb < 8 || &item[2..6] != b"gala" || u16::from_le_bytes([item[6], item[7]]) != KIND_PATH {
        // SAFETY: the caller's buffer.
        unsafe { *path = 0 };
        return 0;
    }
    let n = ((cb - 8) / 2).min(super::MAX_PATH - 1);
    for i in 0..n {
        // SAFETY: inside the item and the caller's MAX_PATH buffer.
        unsafe { *path.add(i) = u16::from_le_bytes([item[8 + 2 * i], item[9 + 2 * i]]) };
    }
    // SAFETY: n < MAX_PATH.
    unsafe { *path.add(n) = 0 };
    1
}

/// SHGetMalloc: the task allocator (ole32's CoGetMalloc).
#[unsafe(no_mangle)]
pub extern "system" fn SHGetMalloc(out: *mut usize) -> i32 {
    match import!("ole32.dll", "CoGetMalloc", extern "system" fn(u32, *mut usize) -> i32) {
        Some(get) => get(1, out),
        None => super::E_OUTOFMEMORY,
    }
}
