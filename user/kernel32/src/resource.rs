//! Resources of loaded images (FindResource*, LoadResource, LockResource,
//! SizeofResource), read from the image's resource directory as the PE
//! format documents it: three levels of IMAGE_RESOURCE_DIRECTORY (type,
//! name, language), whose entries hold an ID or an offset (high bit set)
//! of a counted UTF-16 name, and an offset of the next directory (high bit
//! set) or of an IMAGE_RESOURCE_DATA_ENTRY (RVA, size, code page).
//!
//! An HRSRC is the address of the data entry; the HGLOBAL LoadResource
//! returns is the data's address (LockResource returns it unchanged).

use super::{Bool, Handle, fail_with};

const ERROR_RESOURCE_DATA_NOT_FOUND: u32 = 1812;
const ERROR_RESOURCE_TYPE_NOT_FOUND: u32 = 1813;
const ERROR_RESOURCE_NAME_NOT_FOUND: u32 = 1814;
const ERROR_RESOURCE_LANG_NOT_FOUND: u32 = 1815;

fn rd32(a: usize) -> u32 {
    // SAFETY: inside a mapped image (the caller checked the directory).
    unsafe { (a as *const u32).read_unaligned() }
}

fn rd16(a: usize) -> u16 {
    // SAFETY: as above.
    unsafe { (a as *const u16).read_unaligned() }
}

/// The module's base (NULL: the executable).
fn module_base(module: Handle) -> usize {
    if module.is_null() { super::mapping::image_base() as usize } else { module as usize & !3 }
}

/// The address of the image's resource directory, if it has one.
fn resource_root(base: usize) -> Option<usize> {
    if rd16(base) != 0x5a4d {
        return None;
    }
    let nt = base + rd32(base + 0x3c) as usize;
    let directories = nt + 24 + if rd16(nt + 24) == 0x20b { 112 } else { 96 };
    let rva = rd32(directories + 2 * 8) as usize; // IMAGE_DIRECTORY_ENTRY_RESOURCE
    (rva != 0).then_some(base + rva)
}

/// A resource type, name or language to look up.
#[derive(Clone, Copy)]
pub(crate) enum Key {
    Id(u32),
    /// A NUL-terminated UTF-16 name (compared case-insensitively).
    Name(*const u16),
    Any,
}

impl Key {
    /// From a LPCWSTR: MAKEINTRESOURCE ids, "#123", or a name.
    pub(crate) fn from_wide(p: *const u16) -> Key {
        if (p as usize) >> 16 == 0 {
            return Key::Id(p as usize as u32);
        }
        // SAFETY: the caller's NUL-terminated string.
        unsafe {
            if *p == u16::from(b'#') {
                let mut n: u32 = 0;
                let mut i = 1;
                while (u16::from(b'0')..=u16::from(b'9')).contains(&*p.add(i)) {
                    n = n.wrapping_mul(10).wrapping_add(u32::from(*p.add(i) - u16::from(b'0')));
                    i += 1;
                }
                return Key::Id(n);
            }
        }
        Key::Name(p)
    }

    fn matches(&self, root: usize, entry: usize) -> bool {
        let id = rd32(entry);
        match *self {
            Key::Any => true,
            Key::Id(want) => id & 0x8000_0000 == 0 && id == want,
            Key::Name(p) => {
                if id & 0x8000_0000 == 0 {
                    return false;
                }
                let s = root + (id & 0x7fff_ffff) as usize;
                let len = usize::from(rd16(s));
                let upper = |c: u16| {
                    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
                };
                // SAFETY: the caller's NUL-terminated string, read up to
                // its terminator or `len` + 1 characters.
                unsafe {
                    (0..len).all(|i| upper(*p.add(i)) == upper(rd16(s + 2 + 2 * i)))
                        && *p.add(len) == 0
                }
            }
        }
    }
}

/// The entry of directory `dir` matching `key`: its offset field.
fn find(root: usize, dir: usize, key: Key) -> Option<u32> {
    let n = usize::from(rd16(dir + 12)) + usize::from(rd16(dir + 14));
    (0..n).map(|i| dir + 16 + 8 * i).find(|&e| key.matches(root, e)).map(|e| rd32(e + 4))
}

/// The data entry of resource (`kind`, `name`, `lang`) in the image at
/// `base`, or the error code. Language 0 (neutral) takes the first.
pub(crate) fn find_resource(base: usize, kind: Key, name: Key, lang: u16) -> Result<usize, u32> {
    let root = resource_root(base).ok_or(ERROR_RESOURCE_DATA_NOT_FOUND)?;
    let sub = |offset: u32| root + (offset & 0x7fff_ffff) as usize;
    let types = find(root, root, kind).ok_or(ERROR_RESOURCE_TYPE_NOT_FOUND)?;
    let names = find(root, sub(types), name).ok_or(ERROR_RESOURCE_NAME_NOT_FOUND)?;
    let langs = sub(names);
    let data = if lang == 0 {
        find(root, langs, Key::Any)
    } else {
        find(root, langs, Key::Id(lang.into())).or_else(|| find(root, langs, Key::Any))
    }
    .ok_or(ERROR_RESOURCE_LANG_NOT_FOUND)?;
    Ok(root + data as usize)
}

/// The bytes of the resource whose data entry is at `entry`.
pub(crate) fn resource_bytes(base: usize, entry: usize) -> &'static [u8] {
    // SAFETY: the data inside the mapped image.
    unsafe {
        core::slice::from_raw_parts(
            (base + rd32(entry) as usize) as *const u8,
            rd32(entry + 4) as usize,
        )
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn FindResourceExW(
    module: Handle,
    kind: *const u16,
    name: *const u16,
    lang: u16,
) -> Handle {
    match find_resource(module_base(module), Key::from_wide(kind), Key::from_wide(name), lang) {
        Ok(entry) => entry as Handle,
        Err(e) => fail_with(e, core::ptr::null_mut()),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn FindResourceW(module: Handle, name: *const u16, kind: *const u16) -> Handle {
    FindResourceExW(module, kind, name, 0)
}

/// An ANSI resource name or type as UTF-16 (ids pass through).
fn widen(p: *const u8, buf: &mut [u16; 256]) -> *const u16 {
    if (p as usize) >> 16 == 0 {
        return p.cast();
    }
    let mut i = 0;
    // SAFETY: the caller's NUL-terminated string (resource names are ASCII).
    unsafe {
        while i < 255 && *p.add(i) != 0 {
            buf[i] = u16::from(*p.add(i));
            i += 1;
        }
    }
    buf[i] = 0;
    buf.as_ptr()
}

#[unsafe(no_mangle)]
pub extern "system" fn FindResourceExA(
    module: Handle,
    kind: *const u8,
    name: *const u8,
    lang: u16,
) -> Handle {
    let (mut k, mut n) = ([0u16; 256], [0u16; 256]);
    FindResourceExW(module, widen(kind, &mut k), widen(name, &mut n), lang)
}

#[unsafe(no_mangle)]
pub extern "system" fn FindResourceA(module: Handle, name: *const u8, kind: *const u8) -> Handle {
    FindResourceExA(module, kind, name, 0)
}

#[unsafe(no_mangle)]
pub extern "system" fn LoadResource(module: Handle, res: Handle) -> Handle {
    if res.is_null() {
        return fail_with(6, core::ptr::null_mut()); // ERROR_INVALID_HANDLE
    }
    resource_bytes(module_base(module), res as usize).as_ptr() as Handle
}

#[unsafe(no_mangle)]
pub extern "system" fn LockResource(data: Handle) -> Handle {
    data
}

#[unsafe(no_mangle)]
pub extern "system" fn SizeofResource(module: Handle, res: Handle) -> u32 {
    if res.is_null() {
        return fail_with(6, 0);
    }
    let _ = module;
    rd32(res as usize + 4)
}

/// FreeResource: resources live as long as their image (obsolete call).
#[unsafe(no_mangle)]
pub extern "system" fn FreeResource(_data: Handle) -> Bool {
    0
}
