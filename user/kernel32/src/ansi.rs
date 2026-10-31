//! ANSI (`A`) variants of file functions, on top of the `W` ones. gala's
//! ANSI code page is UTF-8 (see text.rs), so strings convert losslessly.

use super::sync::ansi_to_wide;
use super::*;

const MAX: usize = 1024;

/// `s` (UTF-8, NUL-terminated) as UTF-16 with a NUL in `buf`; null stays
/// null.
fn wide(s: *const u8, buf: &mut [u16; MAX]) -> Result<*const u16, u32> {
    if s.is_null() {
        return Ok(core::ptr::null());
    }
    let n = ansi_to_wide(s, &mut buf[..MAX - 1]).ok_or(ERROR_FILENAME_EXCED_RANGE)?;
    buf[n] = 0;
    Ok(buf.as_ptr())
}

/// UTF-16 `w` (no NUL) to UTF-8 at `out` (`size` bytes, NUL added); the
/// length without the NUL, or None if it does not fit.
fn to_utf8(w: &[u16], out: *mut u8, size: usize) -> Option<usize> {
    if w.is_empty() {
        if size == 0 {
            return None;
        }
        // SAFETY: the caller's buffer of at least one byte.
        unsafe { *out = 0 };
        return Some(0);
    }
    let n = super::text::WideCharToMultiByte(
        65001,
        0,
        w.as_ptr(),
        w.len() as i32,
        out,
        size.saturating_sub(1) as i32,
        core::ptr::null(),
        core::ptr::null_mut(),
    );
    if n <= 0 {
        return None;
    }
    // SAFETY: n < size bytes were written.
    unsafe { *out.add(n as usize) = 0 };
    Some(n as usize)
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateFileA(
    path: *const u8,
    access: u32,
    share: u32,
    security: *mut c_void,
    disposition: u32,
    flags: u32,
    template: Handle,
) -> Handle {
    let mut buf = [0u16; MAX];
    match wide(path, &mut buf) {
        Ok(p) => CreateFileW(p, access, share, security, disposition, flags, template),
        Err(e) => fail_with(e, INVALID_HANDLE_VALUE),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn GetFileAttributesA(path: *const u8) -> u32 {
    let mut buf = [0u16; MAX];
    match wide(path, &mut buf) {
        Ok(p) => GetFileAttributesW(p),
        Err(e) => fail_with(e, u32::MAX),
    }
}

/// GetFileAttributesExA: WIN32_FILE_ATTRIBUTE_DATA has no strings.
#[unsafe(no_mangle)]
pub extern "system" fn GetFileAttributesExA(path: *const u8, level: u32, info: *mut u32) -> Bool {
    let mut buf = [0u16; MAX];
    match wide(path, &mut buf) {
        Ok(p) => GetFileAttributesExW(p, level, info),
        Err(e) => fail_with(e, FALSE),
    }
}

/// GetCompressedFileSizeA: files are not compressed, so their size.
#[unsafe(no_mangle)]
pub extern "system" fn GetCompressedFileSizeA(path: *const u8, high: *mut u32) -> u32 {
    let mut info = [0u32; 9];
    if GetFileAttributesExA(path, 0, info.as_mut_ptr()) == FALSE {
        return u32::MAX; // INVALID_FILE_SIZE, error set
    }
    // WIN32_FILE_ATTRIBUTE_DATA: nFileSizeHigh at 28, nFileSizeLow at 32.
    if !high.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *high = info[7] };
    }
    SetLastError(0);
    info[8]
}

#[unsafe(no_mangle)]
pub extern "system" fn GetFullPathNameA(
    path: *const u8,
    size: u32,
    out: *mut u8,
    file_part: *mut *mut u8,
) -> u32 {
    let mut buf = [0u16; MAX];
    let p = match wide(path, &mut buf) {
        Ok(p) => p,
        Err(e) => return fail_with(e, 0),
    };
    let mut full = [0u16; MAX];
    let n = GetFullPathNameW(p, MAX as u32, full.as_mut_ptr(), core::ptr::null_mut()) as usize;
    if n == 0 || n >= MAX {
        return 0;
    }
    let mut utf8 = [0u8; 4 * MAX];
    let Some(len) = to_utf8(&full[..n], utf8.as_mut_ptr(), utf8.len()) else { return 0 };
    if out.is_null() || (size as usize) <= len {
        return len as u32 + 1; // the size needed, with the NUL
    }
    // SAFETY: the caller's buffer of `size` > len bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(utf8.as_ptr(), out, len);
        *out.add(len) = 0;
        if !file_part.is_null() {
            let last = utf8[..len].iter().rposition(|&c| c == b'\\').map_or(0, |i| i + 1);
            *file_part = if last < len { out.add(last) } else { core::ptr::null_mut() };
        }
    }
    len as u32
}

/// MoveFileExA: MOVEFILE_REPLACE_EXISTING replaces the target; the other
/// flags (copy allowed, write through) change nothing here.
#[unsafe(no_mangle)]
pub extern "system" fn MoveFileExA(from: *const u8, to: *const u8, flags: u32) -> Bool {
    const MOVEFILE_REPLACE_EXISTING: u32 = 1;
    let (mut a, mut b) = ([0u16; MAX], [0u16; MAX]);
    let (from, to) = match (wide(from, &mut a), wide(to, &mut b)) {
        (Ok(f), Ok(t)) => (f, t),
        (Err(e), _) | (_, Err(e)) => return fail_with(e, FALSE),
    };
    if to.is_null() {
        // A null target deletes the source.
        return DeleteFileW(from);
    }
    if super::mapping::MoveFileW(from, to) != FALSE {
        return TRUE;
    }
    if flags & MOVEFILE_REPLACE_EXISTING != 0
        && GetLastError() == ERROR_ALREADY_EXISTS
        && DeleteFileW(to) != FALSE
    {
        return super::mapping::MoveFileW(from, to);
    }
    FALSE
}

#[unsafe(no_mangle)]
pub extern "system" fn LoadLibraryExA(name: *const u8, file: Handle, flags: u32) -> Handle {
    let mut buf = [0u16; MAX];
    match wide(name, &mut buf) {
        Ok(p) => LoadLibraryExW(p, file, flags),
        Err(e) => fail_with(e, core::ptr::null_mut()),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn GetDiskFreeSpaceExA(
    directory: *const u8,
    available: *mut u64,
    total: *mut u64,
    free: *mut u64,
) -> Bool {
    let mut buf = [0u16; MAX];
    match wide(directory, &mut buf) {
        Ok(p) => super::mapping::GetDiskFreeSpaceExW(p, available, total, free),
        Err(e) => fail_with(e, FALSE),
    }
}

/// WIN32_FIND_DATAW (592 bytes) to WIN32_FIND_DATAA (320): the first 44
/// bytes are the same; the names are converted.
fn find_data_to_ansi(w: &[u16; 296], a: *mut u8) {
    let name_len = w[22..282].iter().position(|&c| c == 0).unwrap_or(260);
    let alt_len = w[282..296].iter().position(|&c| c == 0).unwrap_or(14);
    // SAFETY: the caller's WIN32_FIND_DATAA (320 bytes).
    unsafe {
        core::ptr::write_bytes(a, 0, 320);
        core::ptr::copy_nonoverlapping(w.as_ptr().cast::<u8>(), a, 44);
    }
    // SAFETY: as above: cFileName at 44 (260 bytes), cAlternateFileName
    // at 304 (14 bytes).
    let (name, alt) = unsafe { (a.add(44), a.add(304)) };
    let _ = to_utf8(&w[22..22 + name_len], name, 260);
    let _ = to_utf8(&w[282..282 + alt_len], alt, 14);
}

#[unsafe(no_mangle)]
pub extern "system" fn FindFirstFileA(pattern: *const u8, data: *mut u8) -> Handle {
    let mut buf = [0u16; MAX];
    let p = match wide(pattern, &mut buf) {
        Ok(p) => p,
        Err(e) => return fail_with(e, INVALID_HANDLE_VALUE),
    };
    let mut w = [0u16; 296];
    let find = FindFirstFileW(p, w.as_mut_ptr());
    if find != INVALID_HANDLE_VALUE {
        find_data_to_ansi(&w, data);
    }
    find
}

#[unsafe(no_mangle)]
pub extern "system" fn FindNextFileA(find: Handle, data: *mut u8) -> Bool {
    let mut w = [0u16; 296];
    if FindNextFileW(find, w.as_mut_ptr()) == FALSE {
        return FALSE;
    }
    find_data_to_ansi(&w, data);
    TRUE
}
