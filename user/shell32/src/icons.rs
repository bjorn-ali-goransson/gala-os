//! Icons from files (ExtractIconW, ExtractIconExW, SHDefExtractIconW), as
//! documented: an image's RT_GROUP_ICON resources in the order of its
//! resource directory (named ones, then IDs), or an .ico file's images.
//! The file is read, not loaded (the PE format's resource directory);
//! the icon is made by user32's CreateIconFromResourceEx, found at run
//! time. See user/shell32_test.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> usize;
    fn ReadFile(h: usize, buf: *mut u8, n: u32, read: *mut u32, overlapped: *mut c_void) -> i32;
    fn GetFileSizeEx(h: usize, size: *mut i64) -> i32;
    fn CloseHandle(h: usize) -> i32;
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
    fn LoadLibraryW(name: *const u16) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> *const c_void;
}

/// A whole file in heap memory.
struct FileData {
    p: *mut u8,
    len: usize,
}

impl FileData {
    fn bytes(&self) -> &[u8] {
        // SAFETY: our block of `len` bytes.
        unsafe { core::slice::from_raw_parts(self.p, self.len) }
    }
}

impl Drop for FileData {
    fn drop(&mut self) {
        // SAFETY: our block.
        unsafe { HeapFree(GetProcessHeap(), 0, self.p) };
    }
}

/// The file's bytes (up to 256 MiB), or None if it cannot be read.
fn read(path: *const u16) -> Option<FileData> {
    // SAFETY: the caller's NUL-terminated path; our handle and buffer.
    unsafe {
        let h = CreateFileW(path, 0x8000_0000, 7, core::ptr::null(), 3, 0, 0);
        if h == usize::MAX {
            return None;
        }
        let mut size = 0i64;
        let ok = GetFileSizeEx(h, &mut size) != 0 && (0..=256 << 20).contains(&size);
        let len = size as usize;
        let p = if ok { HeapAlloc(GetProcessHeap(), 0, len.max(1)) } else { core::ptr::null_mut() };
        let mut got = 0u32;
        let read_ok =
            !p.is_null() && ReadFile(h, p, len as u32, &mut got, core::ptr::null_mut()) != 0;
        CloseHandle(h);
        if p.is_null() {
            return None;
        }
        let data = FileData { p, len: got as usize };
        read_ok.then_some(data)
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// A PE file's resource section: its bytes from the directory's start
/// and the RVA that start has.
struct Resources<'a> {
    file: &'a [u8],
    /// File offset and RVA of the resource directory.
    base: usize,
    rva: u32,
    /// Sections: (RVA, size, file offset).
    sections: [(u32, u32, u32); 32],
    count: usize,
}

impl<'a> Resources<'a> {
    fn of(file: &'a [u8]) -> Option<Self> {
        if file.get(..2) != Some(b"MZ") {
            return None;
        }
        let nt = u32_at(file, 0x3c)? as usize;
        if file.get(nt..nt + 4) != Some(b"PE\0\0") {
            return None;
        }
        let fh = nt + 4;
        let nsections = u16_at(file, fh + 2)? as usize;
        let opt_size = u16_at(file, fh + 16)? as usize;
        let opt = fh + 20;
        let dirs = match u16_at(file, opt)? {
            0x10b => opt + 96,
            0x20b => opt + 112,
            _ => return None,
        };
        let rva = u32_at(file, dirs + 16)?;
        let mut r = Resources { file, base: 0, rva, sections: [(0, 0, 0); 32], count: 0 };
        let table = opt + opt_size;
        for i in 0..nsections.min(32) {
            let s = table + i * 40;
            r.sections[i] = (
                u32_at(file, s + 12)?,
                u32_at(file, s + 8)?.max(u32_at(file, s + 16)?),
                u32_at(file, s + 20)?,
            );
            r.count += 1;
        }
        if rva == 0 {
            return Some(r); // no resources
        }
        r.base = r.offset(rva)?;
        Some(r)
    }

    /// The file offset of `rva`.
    fn offset(&self, rva: u32) -> Option<usize> {
        self.sections[..self.count]
            .iter()
            .find(|&&(start, size, _)| rva >= start && rva < start + size)
            .map(|&(start, _, raw)| (raw + (rva - start)) as usize)
    }

    /// The entries of the directory at `dir` (an offset from the
    /// resource directory's start): (name or ID, data offset, is a
    /// directory, named).
    fn entries(&self, dir: usize) -> impl Iterator<Item = (u32, usize, bool, bool)> + '_ {
        let at = self.base + dir;
        let named = u16_at(self.file, at + 12).unwrap_or(0) as usize;
        let ids = u16_at(self.file, at + 14).unwrap_or(0) as usize;
        (0..named + ids).filter_map(move |i| {
            let e = at + 16 + i * 8;
            let name = u32_at(self.file, e)?;
            let data = u32_at(self.file, e + 4)?;
            Some((name, (data & 0x7fff_ffff) as usize, data & 0x8000_0000 != 0, i < named))
        })
    }

    /// The type `kind`'s subdirectory.
    fn kind(&self, kind: u32) -> Option<usize> {
        if self.rva == 0 {
            return None;
        }
        self.entries(0).find(|&(n, _, d, named)| d && !named && n == kind).map(|(_, o, _, _)| o)
    }

    /// The first language's data of a name entry's subdirectory.
    fn data(&self, sub: usize) -> Option<&'a [u8]> {
        let (_, leaf, is_dir, _) = self.entries(sub).next()?;
        if is_dir {
            return None;
        }
        let at = self.base + leaf;
        let (rva, size) = (u32_at(self.file, at)?, u32_at(self.file, at + 4)? as usize);
        let off = self.offset(rva)?;
        self.file.get(off..off + size)
    }

    /// The RT_GROUP_ICON groups: their name entries' subdirectories and
    /// IDs (0 for named ones), in directory order.
    fn groups(&self) -> impl Iterator<Item = (usize, u32)> + '_ {
        let dir = self.kind(14);
        dir.into_iter().flat_map(move |d| {
            self.entries(d).filter(|e| e.2).map(|(n, o, _, named)| (o, if named { 0 } else { n }))
        })
    }

    fn icon(&self, id: u32) -> Option<&'a [u8]> {
        let dir = self.kind(3)?;
        let (_, sub, _, _) = self.entries(dir).find(|&(n, _, d, named)| d && !named && n == id)?;
        self.data(sub)
    }
}

/// Which image of a group (or .ico) to take for `size` pixels: the one
/// of that size with the most bits per pixel, else the nearest size.
fn best(entries: impl Iterator<Item = (u32, u16, usize)>, size: u32) -> Option<usize> {
    let mut pick: Option<(u32, u16, usize)> = None;
    for (w, bpp, i) in entries {
        let w = if w == 0 { 256 } else { w };
        let better = match pick {
            None => true,
            Some((pw, pb, _)) => {
                let (d, pd) = (w.abs_diff(size), pw.abs_diff(size));
                d < pd || (d == pd && bpp > pb)
            }
        };
        if better {
            pick = Some((w, bpp, i));
        }
    }
    pick.map(|p| p.2)
}

type CreateIcon = extern "system" fn(*const u8, u32, i32, u32, i32, i32, u32) -> usize;

fn user32_fn(name: &core::ffi::CStr) -> usize {
    let dll: [u16; 11] = [
        b'u' as u16,
        b's' as u16,
        b'e' as u16,
        b'r' as u16,
        b'3' as u16,
        b'2' as u16,
        b'.' as u16,
        b'd' as u16,
        b'l' as u16,
        b'l' as u16,
        0,
    ];
    // SAFETY: NUL-terminated names.
    unsafe {
        let m = LoadLibraryW(dll.as_ptr());
        if m == 0 { 0 } else { GetProcAddress(m, name.as_ptr().cast()) as usize }
    }
}

/// An icon from an RT_ICON-format image (BITMAPINFOHEADER and bits).
fn make(bits: &[u8], size: u32) -> usize {
    let f = user32_fn(c"CreateIconFromResourceEx");
    if f == 0 {
        crate::unsupported(b"shell32.dll!ExtractIcon(no user32)", 0);
        return 0;
    }
    // SAFETY: user32's documented signature.
    let create: CreateIcon = unsafe { core::mem::transmute::<usize, CreateIcon>(f) };
    create(bits.as_ptr(), bits.len() as u32, 1, 0x0003_0000, size as i32, size as i32, 0)
}

/// The standard icon size (SM_CXICON), 32 without user32.
fn icon_size() -> u32 {
    let f = user32_fn(c"GetSystemMetrics");
    if f == 0 {
        return 32;
    }
    // SAFETY: user32's documented signature.
    let metrics: extern "system" fn(i32) -> i32 =
        unsafe { core::mem::transmute::<usize, extern "system" fn(i32) -> i32>(f) };
    match metrics(11) {
        n if n > 0 => n as u32,
        _ => 32,
    }
}

/// What a file holds for icons (on the stack: the size difference is fine).
#[allow(clippy::large_enum_variant)]
enum Source<'a> {
    Image(Resources<'a>),
    Ico(&'a [u8]),
    Neither,
}

fn source(file: &[u8]) -> Source<'_> {
    if let Some(r) = Resources::of(file) {
        return Source::Image(r);
    }
    if file.len() >= 6
        && u16_at(file, 0) == Some(0)
        && u16_at(file, 2) == Some(1)
        && u16_at(file, 4).is_some_and(|n| n > 0)
    {
        return Source::Ico(file);
    }
    Source::Neither
}

/// The number of icons in a source.
fn count(s: &Source) -> u32 {
    match s {
        Source::Image(r) => r.groups().count() as u32,
        Source::Ico(_) => 1,
        Source::Neither => 0,
    }
}

/// The icon `index` (>= 0: the n-th group; < 0: the group with resource
/// ID -index) of `size` pixels, or 0.
fn extract(s: &Source, index: i32, size: u32) -> usize {
    match s {
        Source::Neither => 0,
        Source::Ico(f) => {
            if index != 0 {
                return 0;
            }
            let n = u16_at(f, 4).unwrap_or(0) as usize;
            let entries = (0..n).filter_map(|i| {
                let e = 6 + i * 16;
                Some((u32::from(*f.get(e)?), u16_at(f, e + 6)?, i))
            });
            let Some(i) = best(entries, size) else { return 0 };
            let e = 6 + i * 16;
            let (len, off) =
                (u32_at(f, e + 8).unwrap_or(0) as usize, u32_at(f, e + 12).unwrap_or(0) as usize);
            f.get(off..off + len).map_or(0, |bits| make(bits, size))
        }
        Source::Image(r) => {
            let group = if index >= 0 {
                r.groups().nth(index as usize)
            } else {
                let id = index.unsigned_abs();
                r.groups().find(|&(_, gid)| gid == id)
            };
            let Some((sub, _)) = group else { return 0 };
            let Some(dir) = r.data(sub) else { return 0 };
            // GRPICONDIR: reserved, type, count; 14-byte entries
            // (width, height, colours, reserved, planes, bits, bytes, ID).
            let n = u16_at(dir, 4).unwrap_or(0) as usize;
            let entries = (0..n).filter_map(|i| {
                let e = 6 + i * 14;
                Some((u32::from(*dir.get(e)?), u16_at(dir, e + 6)?, i))
            });
            let Some(i) = best(entries, size) else { return 0 };
            let id = u16_at(dir, 6 + i * 14 + 12).unwrap_or(0);
            r.icon(id.into()).map_or(0, |bits| make(bits, size))
        }
    }
}

/// ExtractIconW: with index -1 (0xFFFFFFFF) the number of icons in the
/// file; else the icon (SM_CXICON pixels) at that index (a negative one:
/// the resource ID); NULL if there is none or the file cannot be read,
/// 1 for a file that is neither an image nor an icon file.
#[unsafe(no_mangle)]
pub extern "system" fn ExtractIconW(_instance: usize, file: *const u16, index: u32) -> usize {
    if file.is_null() {
        return 0;
    }
    let Some(data) = read(file) else { return 0 };
    let s = source(data.bytes());
    if matches!(s, Source::Neither) {
        return 1;
    }
    if index == u32::MAX {
        return count(&s) as usize;
    }
    extract(&s, index as i32, icon_size())
}

/// ExtractIconExW: with index -1 and no arrays, the number of icons;
/// else up to `n` icons from `index` on, large (SM_CXICON) and small
/// (16 pixels) into the arrays given; the number extracted.
#[unsafe(no_mangle)]
pub extern "system" fn ExtractIconExW(
    file: *const u16,
    index: i32,
    large: *mut usize,
    small: *mut usize,
    n: u32,
) -> u32 {
    if file.is_null() {
        return 0;
    }
    let Some(data) = read(file) else { return 0 };
    let s = source(data.bytes());
    if index == -1 && large.is_null() && small.is_null() {
        return count(&s);
    }
    let mut done = 0;
    let size = icon_size();
    for i in 0..n.max(1) {
        let at = if index < 0 { index } else { index + i as i32 };
        let l = if large.is_null() { 0 } else { extract(&s, at, size) };
        let sm = if small.is_null() { 0 } else { extract(&s, at, 16) };
        if l == 0 && sm == 0 {
            break;
        }
        // SAFETY: the caller's arrays of `n` handles.
        unsafe {
            if !large.is_null() {
                *large.add(i as usize) = l;
            }
            if !small.is_null() {
                *small.add(i as usize) = sm;
            }
        }
        done += 1;
        if index < 0 {
            break;
        }
    }
    done
}

const S_OK: i32 = 0;
const S_FALSE: i32 = 1;
const E_FAIL: i32 = 0x8000_4005u32 as i32;

/// SHDefExtractIconW: the icon `index` of `file`, large (LOWORD of
/// `sizes`) and small (HIWORD) into the pointers given; S_OK, S_FALSE
/// when the file has no such icon, E_FAIL when it cannot be read.
#[unsafe(no_mangle)]
pub extern "system" fn SHDefExtractIconW(
    file: *const u16,
    index: i32,
    _flags: u32,
    large: *mut usize,
    small: *mut usize,
    sizes: u32,
) -> i32 {
    if file.is_null() {
        return E_FAIL;
    }
    let Some(data) = read(file) else { return E_FAIL };
    let s = source(data.bytes());
    let (big, little) = (sizes & 0xffff, sizes >> 16);
    let big = if big == 0 { icon_size() } else { big };
    let little = if little == 0 { 16 } else { little };
    let mut found = false;
    for (out, size) in [(large, big), (small, little)] {
        if out.is_null() {
            continue;
        }
        let h = extract(&s, index, size);
        // SAFETY: the caller's output pointer.
        unsafe { *out = h };
        found |= h != 0;
    }
    if found { S_OK } else { S_FALSE }
}
