//! gala's version.dll: version resources. GetFileVersionInfo* read an
//! image's VS_VERSION_INFO resource (RT_VERSION, from the resource
//! directory, through the section table) into the caller's buffer, and
//! VerQueryValue* walk its tree of blocks (wLength, wValueLength, wType,
//! a key, the value and child blocks, each 4-byte aligned), as
//! documented for VS_VERSIONINFO.
//!
//! The size GetFileVersionInfoSize reports is twice the resource plus a
//! little: the second half holds the ANSI copies VerQueryValueA makes of
//! string values (the block's own strings are UTF-16).

#![no_std]
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

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
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: usize,
    ) -> isize;
    fn ReadFile(h: isize, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> i32;
    fn SetFilePointerEx(h: isize, distance: i64, new: *mut i64, method: u32) -> i32;
    fn CloseHandle(h: isize) -> i32;
    fn SetLastError(e: u32);
    fn MultiByteToWideChar(cp: u32, flags: u32, s: *const u8, n: i32, w: *mut u16, m: i32) -> i32;
    fn WideCharToMultiByte(
        cp: u32,
        flags: u32,
        w: *const u16,
        n: i32,
        s: *mut u8,
        m: i32,
        default: *const u8,
        used: *mut i32,
    ) -> i32;
}

const ERROR_RESOURCE_DATA_NOT_FOUND: u32 = 1812;
const ERROR_RESOURCE_TYPE_NOT_FOUND: u32 = 1813;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const RT_VERSION: u32 = 16;

fn fail<T>(error: u32, value: T) -> T {
    // SAFETY: plain call.
    unsafe { SetLastError(error) };
    value
}

/// An open file read at offsets.
struct File(isize);

impl File {
    fn open(path: *const u16) -> Option<File> {
        // GENERIC_READ, share read/write/delete, OPEN_EXISTING.
        // SAFETY: the caller's NUL-terminated path.
        let h = unsafe { CreateFileW(path, 0x8000_0000, 7, core::ptr::null_mut(), 3, 0, 0) };
        (h != -1).then_some(File(h))
    }

    fn read(&self, offset: u64, buf: &mut [u8]) -> bool {
        let mut got = 0;
        // SAFETY: our handle and buffer.
        unsafe {
            SetFilePointerEx(self.0, offset as i64, core::ptr::null_mut(), 0) != 0
                && ReadFile(
                    self.0,
                    buf.as_mut_ptr(),
                    buf.len() as u32,
                    &mut got,
                    core::ptr::null_mut(),
                ) != 0
                && got as usize == buf.len()
        }
    }

    fn u32_at(&self, offset: u64) -> Option<u32> {
        let mut b = [0u8; 4];
        self.read(offset, &mut b).then(|| u32::from_le_bytes(b))
    }

    fn u16_at(&self, offset: u64) -> Option<u16> {
        let mut b = [0u8; 2];
        self.read(offset, &mut b).then(|| u16::from_le_bytes(b))
    }
}

impl Drop for File {
    fn drop(&mut self) {
        // SAFETY: our handle.
        unsafe { CloseHandle(self.0) };
    }
}

/// The file offset and size of the image's version resource; Err with the
/// error code if it has none.
fn find_version(file: &File) -> Result<(u64, u32), u32> {
    let bad = ERROR_RESOURCE_DATA_NOT_FOUND;
    if file.u16_at(0) != Some(0x5a4d) {
        return Err(bad);
    }
    let nt = u64::from(file.u32_at(0x3c).ok_or(bad)?);
    let sections = u64::from(file.u16_at(nt + 6).ok_or(bad)?);
    let optional_size = u64::from(file.u16_at(nt + 20).ok_or(bad)?);
    let optional = nt + 24;
    let magic = file.u16_at(optional).ok_or(bad)?;
    let directories = optional + if magic == 0x20b { 112 } else { 96 };
    let resource_rva = file.u32_at(directories + 2 * 8).ok_or(bad)?;
    if resource_rva == 0 {
        return Err(bad);
    }
    // RVA -> file offset through the section table.
    let table = optional + optional_size;
    let to_offset = |rva: u32| -> Option<u64> {
        (0..sections).find_map(|i| {
            let s = table + 40 * i;
            let size = file.u32_at(s + 8)?.max(file.u32_at(s + 16)?);
            let va = file.u32_at(s + 12)?;
            let raw = file.u32_at(s + 20)?;
            (rva >= va && rva < va + size).then(|| u64::from(raw) + u64::from(rva - va))
        })
    };
    let root = to_offset(resource_rva).ok_or(bad)?;
    // A directory's entries: the named ones, then those with ids.
    let find = |dir: u64, id: Option<u32>| -> Option<u32> {
        let named = u64::from(file.u16_at(dir + 12)?);
        let ids = u64::from(file.u16_at(dir + 14)?);
        (0..named + ids).find_map(|i| {
            let e = dir + 16 + 8 * i;
            let name = file.u32_at(e)?;
            let target = file.u32_at(e + 4)?;
            match id {
                Some(id) if name != id => None,
                _ => Some(target),
            }
        })
    };
    let ty = find(root, Some(RT_VERSION)).ok_or(ERROR_RESOURCE_TYPE_NOT_FOUND)?;
    // Name level, then language level (the first of each), then the data.
    let mut entry = ty;
    for _ in 0..2 {
        if entry & 0x8000_0000 == 0 {
            return Err(bad);
        }
        entry = find(root + u64::from(entry & 0x7fff_ffff), None).ok_or(bad)?;
    }
    let data = root + u64::from(entry);
    let rva = file.u32_at(data).ok_or(bad)?;
    let size = file.u32_at(data + 4).ok_or(bad)?;
    Ok((to_offset(rva).ok_or(bad)?, size))
}

/// GetFileVersionInfoSizeW: the buffer size GetFileVersionInfoW needs (0
/// and ERROR_RESOURCE_TYPE_NOT_FOUND/DATA_NOT_FOUND without a version
/// resource).
#[unsafe(no_mangle)]
pub extern "system" fn GetFileVersionInfoSizeW(path: *const u16, handle: *mut u32) -> u32 {
    if !handle.is_null() {
        // SAFETY: the caller's output.
        unsafe { *handle = 0 };
    }
    let Some(file) = File::open(path) else { return 0 };
    match find_version(&file) {
        Ok((_, size)) => 2 * size + 4,
        Err(e) => fail(e, 0),
    }
}

/// GetFileVersionInfoW: the resource, then zeros (room for ANSI copies).
#[unsafe(no_mangle)]
pub extern "system" fn GetFileVersionInfoW(
    path: *const u16,
    _handle: u32,
    length: u32,
    data: *mut u8,
) -> i32 {
    if data.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let Some(file) = File::open(path) else { return 0 };
    let (offset, size) = match find_version(&file) {
        Ok(v) => v,
        Err(e) => return fail(e, 0),
    };
    let n = size.min(length) as usize;
    // SAFETY: the caller's buffer of `length` bytes.
    let buf = unsafe { core::slice::from_raw_parts_mut(data, length as usize) };
    buf.fill(0);
    if !file.read(offset, &mut buf[..n]) {
        return fail(ERROR_RESOURCE_DATA_NOT_FOUND, 0);
    }
    1
}

/// GetFileVersionInfoSizeExW / GetFileVersionInfoExW: the flags choose
/// between localized (MUI) and neutral resources and whether to resolve
/// the path; gala has no MUI files, so both read the file's own resource.
#[unsafe(no_mangle)]
pub extern "system" fn GetFileVersionInfoSizeExW(
    _flags: u32,
    path: *const u16,
    handle: *mut u32,
) -> u32 {
    GetFileVersionInfoSizeW(path, handle)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetFileVersionInfoExW(
    _flags: u32,
    path: *const u16,
    handle: u32,
    length: u32,
    data: *mut u8,
) -> i32 {
    GetFileVersionInfoW(path, handle, length, data)
}

/// A path in UTF-16 from an ANSI (UTF-8) one; None if too long.
fn widen(s: *const u8, out: &mut [u16; 520]) -> Option<*const u16> {
    if s.is_null() {
        return Some(core::ptr::null());
    }
    // SAFETY: the caller's NUL-terminated string (-1: with its NUL).
    let n = unsafe { MultiByteToWideChar(65001, 0, s, -1, out.as_mut_ptr(), 520) };
    (n > 0).then_some(out.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "system" fn GetFileVersionInfoSizeA(path: *const u8, handle: *mut u32) -> u32 {
    let mut w = [0u16; 520];
    match widen(path, &mut w) {
        Some(p) => GetFileVersionInfoSizeW(p, handle),
        None => fail(ERROR_INVALID_PARAMETER, 0),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn GetFileVersionInfoA(
    path: *const u8,
    handle: u32,
    length: u32,
    data: *mut u8,
) -> i32 {
    let mut w = [0u16; 520];
    match widen(path, &mut w) {
        Some(p) => GetFileVersionInfoW(p, handle, length, data),
        None => fail(ERROR_INVALID_PARAMETER, 0),
    }
}

/// A block of the tree: (length, value length, type, key, value offset,
/// children offset), offsets from the block's start.
struct Block {
    length: usize,
    value_length: usize,
    text: bool,
    key_at: usize,
    value_at: usize,
    children_at: usize,
}

const fn align4(v: usize) -> usize {
    (v + 3) & !3
}

/// The block at `p`.
fn block(p: *const u8) -> Block {
    // SAFETY: a block inside the caller's tree.
    unsafe {
        let w = |i: usize| p.add(2 * i).cast::<u16>().read_unaligned();
        let (length, value_length, text) = (usize::from(w(0)), usize::from(w(1)), w(2) == 1);
        let mut k = 3;
        while w(k) != 0 {
            k += 1;
        }
        let value_at = align4(2 * (k + 1));
        let value_bytes = if text { 2 * value_length } else { value_length };
        Block {
            length,
            value_length,
            text,
            key_at: 6,
            value_at,
            children_at: align4(value_at + value_bytes),
        }
    }
}

/// Whether the block's key at `p` is `name` (ignoring ASCII case).
fn key_is(p: *const u8, b: &Block, name: &[u16]) -> bool {
    // SAFETY: the key is NUL-terminated inside the block.
    unsafe {
        let key = p.add(b.key_at).cast::<u16>();
        let lower = |c: u16| if (65..=90).contains(&c) { c + 32 } else { c };
        name.iter().enumerate().all(|(i, &c)| lower(key.add(i).read_unaligned()) == lower(c))
            && key.add(name.len()).read_unaligned() == 0
    }
}

/// Walks the path `sub` (components separated by backslashes) from the
/// root block; the final block.
fn walk(root: *const u8, sub: &[u16]) -> Option<*const u8> {
    let mut node = root;
    for name in sub.split(|&c| c == u16::from(b'\\')).filter(|s| !s.is_empty()) {
        let b = block(node);
        let mut at = b.children_at;
        let mut found = None;
        while at + 6 <= b.length {
            // SAFETY: a child inside the block.
            let child = unsafe { node.add(at) };
            let cb = block(child);
            if cb.length == 0 {
                break;
            }
            if key_is(child, &cb, name) {
                found = Some(child);
                break;
            }
            at = align4(at + cb.length);
        }
        node = found?;
    }
    Some(node)
}

fn units(s: *const u16) -> &'static [u16] {
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    unsafe {
        while *s.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(s, n)
    }
}

/// VerQueryValueW: the value of the block `sub` names ("\\" is the root's
/// VS_FIXEDFILEINFO): its address and length (bytes for binary values,
/// characters for text).
#[unsafe(no_mangle)]
pub extern "system" fn VerQueryValueW(
    data: *const c_void,
    sub: *const u16,
    out: *mut *mut c_void,
    length: *mut u32,
) -> i32 {
    if data.is_null() || sub.is_null() || out.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let root = data.cast::<u8>();
    let Some(node) = walk(root, units(sub)) else { return fail(ERROR_RESOURCE_TYPE_NOT_FOUND, 0) };
    let b = block(node);
    if b.value_length == 0 {
        return fail(ERROR_RESOURCE_TYPE_NOT_FOUND, 0);
    }
    // SAFETY: the value inside the block; the caller's outputs.
    unsafe {
        *out = node.add(b.value_at).cast_mut().cast();
        if !length.is_null() {
            *length = b.value_length as u32;
        }
    }
    1
}

/// VerQueryValueA: as the W form; a text value is copied as ANSI into the
/// second half of the buffer GetFileVersionInfoA filled (at the same
/// offset, halved, after the resource).
#[unsafe(no_mangle)]
pub extern "system" fn VerQueryValueA(
    data: *const c_void,
    sub: *const u8,
    out: *mut *mut c_void,
    length: *mut u32,
) -> i32 {
    let mut w = [0u16; 520];
    let Some(sub_w) = widen(sub, &mut w) else { return fail(ERROR_INVALID_PARAMETER, 0) };
    if sub_w.is_null() || data.is_null() || out.is_null() {
        return fail(ERROR_INVALID_PARAMETER, 0);
    }
    let root = data.cast::<u8>();
    let Some(node) = walk(root, units(sub_w)) else {
        return fail(ERROR_RESOURCE_TYPE_NOT_FOUND, 0);
    };
    let b = block(node);
    if b.value_length == 0 {
        return fail(ERROR_RESOURCE_TYPE_NOT_FOUND, 0);
    }
    // SAFETY: inside the caller's buffer from GetFileVersionInfoA, which is
    // twice the resource (block(root).length) and more.
    unsafe {
        let value = node.add(b.value_at);
        if !b.text {
            *out = value.cast_mut().cast();
            if !length.is_null() {
                *length = b.value_length as u32;
            }
            return 1;
        }
        let total = block(root).length;
        let offset = value.offset_from(root) as usize;
        let ansi = root.add(total + offset / 2).cast_mut();
        let n = WideCharToMultiByte(
            65001,
            0,
            value.cast(),
            b.value_length as i32,
            ansi,
            b.value_length as i32,
            core::ptr::null(),
            core::ptr::null_mut(),
        );
        if n <= 0 {
            return fail(ERROR_INSUFFICIENT_BUFFER, 0);
        }
        *out = ansi.cast();
        if !length.is_null() {
            *length = n as u32;
        }
    }
    1
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
