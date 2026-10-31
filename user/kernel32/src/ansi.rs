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

/// MoveFileExA: MoveFileExW.
#[unsafe(no_mangle)]
pub extern "system" fn MoveFileExA(from: *const u8, to: *const u8, flags: u32) -> Bool {
    let (mut a, mut b) = ([0u16; MAX], [0u16; MAX]);
    match (wide(from, &mut a), wide(to, &mut b)) {
        (Ok(f), Ok(t)) => MoveFileExW(f, t, flags),
        (Err(e), _) | (_, Err(e)) => fail_with(e, FALSE),
    }
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

#[unsafe(no_mangle)]
pub extern "system" fn SearchPathA(
    path: *const u8,
    file: *const u8,
    extension: *const u8,
    size: u32,
    out: *mut u8,
    file_part: *mut *mut u8,
) -> u32 {
    let (mut p, mut f) = ([0u16; MAX], [0u16; MAX_PATH]);
    let mut e = [0u16; 32];
    let p = match wide(path, &mut p) {
        Ok(p) => p,
        Err(e) => return fail_with(e, 0),
    };
    let convert = |s: *const u8, buf: &mut [u16]| -> Result<*const u16, u32> {
        if s.is_null() {
            return Ok(core::ptr::null());
        }
        super::sync::ansi_to_wide(s, buf).ok_or(ERROR_FILENAME_EXCED_RANGE)?;
        Ok(buf.as_ptr())
    };
    match (convert(file, &mut f), convert(extension, &mut e)) {
        (Ok(f), Ok(e)) => search_path_to_ansi(p, f, e, size, out, file_part),
        (Err(e), _) | (_, Err(e)) => fail_with(e, 0),
    }
}

const MAX_PATH: usize = 260;

/// SearchPathW's result as UTF-8 (its own frame: SearchPathA's stays
/// under 4 KiB).
#[inline(never)]
fn search_path_to_ansi(
    p: *const u16,
    f: *const u16,
    e: *const u16,
    size: u32,
    out: *mut u8,
    file_part: *mut *mut u8,
) -> u32 {
    let mut full = [0u16; 520];
    let n = super::files::SearchPathW(p, f, e, 520, full.as_mut_ptr(), core::ptr::null_mut());
    if n == 0 || n >= 520 {
        return 0;
    }
    let mut utf8 = [0u8; 4 * 520];
    let Some(len) = to_utf8(&full[..n as usize], utf8.as_mut_ptr(), utf8.len()) else { return 0 };
    if out.is_null() || (size as usize) <= len {
        return len as u32 + 1;
    }
    // SAFETY: the caller's buffer of `size` > len bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(utf8.as_ptr(), out, len);
        *out.add(len) = 0;
        if !file_part.is_null() {
            let last = utf8[..len].iter().rposition(|&c| c == b'\\').map_or(0, |i| i + 1);
            *file_part = out.add(last);
        }
    }
    len as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateDirectoryA(path: *const u8, security: *mut c_void) -> Bool {
    let mut buf = [0u16; MAX];
    match wide(path, &mut buf) {
        Ok(p) => CreateDirectoryW(p, security),
        Err(e) => fail_with(e, FALSE),
    }
}

/// An ANSI result from a W function that returns a length (`f` fills a
/// UTF-16 buffer): the length without the NUL, or the size needed (with
/// it) if `size` is too small.
fn ansi_result(size: u32, buf: *mut u8, f: impl FnOnce(u32, *mut u16) -> u32) -> u32 {
    let mut w = [0u16; MAX];
    let n = f(MAX as u32, w.as_mut_ptr());
    if n == 0 || n as usize >= MAX {
        return 0;
    }
    let mut tmp = [0u8; MAX * 3];
    let Some(len) = to_utf8(&w[..n as usize], tmp.as_mut_ptr(), tmp.len()) else { return 0 };
    if (size as usize) <= len || buf.is_null() {
        return len as u32 + 1;
    }
    // SAFETY: the caller's buffer of `size` > len bytes.
    unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf, len + 1) };
    len as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn GetTempPathA(size: u32, buf: *mut u8) -> u32 {
    ansi_result(size, buf, |n, w| GetTempPathW(n, w))
}

/// GetModuleHandleExA: the W form (FROM_ADDRESS takes an address, not a
/// name).
#[unsafe(no_mangle)]
pub extern "system" fn GetModuleHandleExA(
    flags: u32,
    name: *const u8,
    module: *mut Handle,
) -> Bool {
    const FROM_ADDRESS: u32 = 4;
    if flags & FROM_ADDRESS != 0 || name.is_null() {
        return super::text::GetModuleHandleExW(flags, name.cast(), module);
    }
    let mut buf = [0u16; MAX];
    match wide(name, &mut buf) {
        Ok(p) => super::text::GetModuleHandleExW(flags, p, module),
        Err(e) => fail_with(e, FALSE),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn OpenFileMappingA(access: u32, inherit: Bool, name: *const u8) -> Handle {
    let mut buf = [0u16; MAX];
    match wide(name, &mut buf) {
        Ok(p) => super::mapping::OpenFileMappingW(access, inherit, p),
        Err(e) => fail_with(e, core::ptr::null_mut()),
    }
}

/// GetEnvironmentStrings (the ANSI block): the environment converted to
/// UTF-8 in a heap block, each string NUL-terminated and the block ending
/// with an empty one; freed by FreeEnvironmentStringsA.
#[unsafe(no_mangle)]
pub extern "system" fn GetEnvironmentStrings() -> *mut u8 {
    let env = GetEnvironmentStringsW();
    // The length of the W block, up to and including its final NUL.
    let mut n = 0;
    // SAFETY: the block ends with an empty string (two NULs).
    unsafe {
        while *env.add(n) != 0 || *env.add(n + 1) != 0 {
            n += 1;
        }
    }
    n += 2;
    // SAFETY: the block has `n` units.
    let units = unsafe { core::slice::from_raw_parts(env, n) };
    let size = super::text::WideCharToMultiByte(
        65001,
        0,
        units.as_ptr(),
        n as i32,
        core::ptr::null_mut(),
        0,
        core::ptr::null(),
        core::ptr::null_mut(),
    );
    let block =
        super::heap::HeapAlloc(super::heap::GetProcessHeap(), 0, size.max(2) as usize) as *mut u8;
    if block.is_null() {
        FreeEnvironmentStringsW(env);
        return fail_with(8, core::ptr::null_mut()); // ERROR_NOT_ENOUGH_MEMORY
    }
    super::text::WideCharToMultiByte(
        65001,
        0,
        units.as_ptr(),
        n as i32,
        block,
        size,
        core::ptr::null(),
        core::ptr::null_mut(),
    );
    FreeEnvironmentStringsW(env);
    block
}

#[unsafe(no_mangle)]
pub extern "system" fn FreeEnvironmentStringsA(block: *mut u8) -> Bool {
    super::heap::HeapFree(super::heap::GetProcessHeap(), 0, block.cast())
}

/// MoveFileExW: MOVEFILE_REPLACE_EXISTING replaces the target; the other
/// flags (copy allowed, write through) change nothing here; a null target
/// deletes the source.
#[unsafe(no_mangle)]
pub extern "system" fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> Bool {
    const MOVEFILE_REPLACE_EXISTING: u32 = 1;
    if to.is_null() {
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
