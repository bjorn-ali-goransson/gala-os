//! More file functions: sizes, locks, device control, change
//! notification, named pipe servers, LocalFree.

use super::*;

/// GetFileSize(hFile, *lpFileSizeHigh): the low half, the high half
/// through the pointer.
#[unsafe(no_mangle)]
pub extern "system" fn GetFileSize(h: Handle, high: *mut u32) -> u32 {
    let mut size = 0i64;
    if GetFileSizeEx(h, &mut size) == FALSE {
        return u32::MAX; // INVALID_FILE_SIZE, error set
    }
    if !high.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *high = (size >> 32) as u32 };
    }
    SetLastError(0);
    size as u32
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtLockFile(
        h: Handle,
        event: Handle,
        apc: usize,
        apc_context: usize,
        iosb: *mut usize,
        offset: *const u64,
        length: *const u64,
        key: u32,
        fail_immediately: u8,
        exclusive: u8,
    ) -> u32;
    fn NtUnlockFile(
        h: Handle,
        iosb: *mut usize,
        offset: *const u64,
        length: *const u64,
        key: u32,
    ) -> u32;
}

const LOCKFILE_FAIL_IMMEDIATELY: u32 = 1;
const LOCKFILE_EXCLUSIVE_LOCK: u32 = 2;

/// The byte offset in an OVERLAPPED (Offset at 16, OffsetHigh at 20).
fn overlapped_offset(overlapped: *const c_void) -> u64 {
    // SAFETY: the caller's OVERLAPPED.
    unsafe {
        let p = overlapped.cast::<u32>();
        u64::from(*p.add(4)) | u64::from(*p.add(5)) << 32
    }
}

/// LockFileEx: a byte-range lock at the OVERLAPPED's offset, shared or
/// exclusive, waiting unless LOCKFILE_FAIL_IMMEDIATELY (then
/// ERROR_LOCK_VIOLATION on a conflict). The OVERLAPPED receives the status.
#[unsafe(no_mangle)]
pub extern "system" fn LockFileEx(
    h: Handle,
    flags: u32,
    _reserved: u32,
    low: u32,
    high: u32,
    overlapped: *mut c_void,
) -> Bool {
    if overlapped.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let (offset, length) = (overlapped_offset(overlapped), u64::from(low) | u64::from(high) << 32);
    // SAFETY: hEvent at 24 in the caller's OVERLAPPED, which is also the
    // IO_STATUS_BLOCK.
    let status = unsafe {
        let event = *overlapped.cast::<u8>().add(24).cast::<Handle>();
        NtLockFile(
            h,
            event,
            0,
            0,
            overlapped.cast(),
            &offset,
            &length,
            0,
            u8::from(flags & LOCKFILE_FAIL_IMMEDIATELY != 0),
            u8::from(flags & LOCKFILE_EXCLUSIVE_LOCK != 0),
        )
    };
    if status == STATUS_SUCCESS { TRUE } else { fail(status, FALSE) }
}

/// LockFile: an exclusive lock, failing at once on a conflict.
#[unsafe(no_mangle)]
pub extern "system" fn LockFile(
    h: Handle,
    offset_low: u32,
    offset_high: u32,
    low: u32,
    high: u32,
) -> Bool {
    let offset = u64::from(offset_low) | u64::from(offset_high) << 32;
    let length = u64::from(low) | u64::from(high) << 32;
    let mut iosb = [0usize; 2];
    // SAFETY: valid pointers.
    let status = unsafe {
        NtLockFile(h, core::ptr::null_mut(), 0, 0, iosb.as_mut_ptr(), &offset, &length, 0, 1, 1)
    };
    if status == STATUS_SUCCESS { TRUE } else { fail(status, FALSE) }
}

fn unlock(h: Handle, offset: u64, length: u64) -> Bool {
    let mut iosb = [0usize; 2];
    // SAFETY: valid pointers.
    let status = unsafe { NtUnlockFile(h, iosb.as_mut_ptr(), &offset, &length, 0) };
    if status == STATUS_SUCCESS { TRUE } else { fail(status, FALSE) }
}

/// UnlockFile: releases exactly a range LockFile(Ex) took on this handle
/// (ERROR_NOT_LOCKED otherwise).
#[unsafe(no_mangle)]
pub extern "system" fn UnlockFile(
    h: Handle,
    offset_low: u32,
    offset_high: u32,
    low: u32,
    high: u32,
) -> Bool {
    unlock(
        h,
        u64::from(offset_low) | u64::from(offset_high) << 32,
        u64::from(low) | u64::from(high) << 32,
    )
}

/// UnlockFileEx: UnlockFile with the offset in an OVERLAPPED.
#[unsafe(no_mangle)]
pub extern "system" fn UnlockFileEx(
    h: Handle,
    _reserved: u32,
    low: u32,
    high: u32,
    overlapped: *mut c_void,
) -> Bool {
    if overlapped.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    unlock(h, overlapped_offset(overlapped), u64::from(low) | u64::from(high) << 32)
}

/// LocalFree: gala's kernel32 has no LocalAlloc; memory programs free
/// with it came from VirtualAlloc. Returns NULL on success.
#[unsafe(no_mangle)]
pub extern "system" fn LocalFree(memory: *mut c_void) -> *mut c_void {
    if !memory.is_null() {
        VirtualFree(memory, 0, 0x8000); // MEM_RELEASE
    }
    core::ptr::null_mut()
}

/// DeviceIoControl: no device control codes yet (recorded by code).
#[unsafe(no_mangle)]
pub extern "system" fn DeviceIoControl(
    _h: Handle,
    code: u32,
    _input: *const c_void,
    _input_size: u32,
    _output: *mut c_void,
    _output_size: u32,
    _returned: *mut u32,
    _overlapped: *mut c_void,
) -> Bool {
    unsupported(b"kernel32.dll!DeviceIoControl", code.into());
    fail_with(ERROR_NOT_SUPPORTED, FALSE)
}

/// ReadDirectoryChangesW: no change notification yet.
#[unsafe(no_mangle)]
pub extern "system" fn ReadDirectoryChangesW(
    _h: Handle,
    _buffer: *mut c_void,
    _length: u32,
    _subtree: Bool,
    _filter: u32,
    _returned: *mut u32,
    _overlapped: *mut c_void,
    _routine: usize,
) -> Bool {
    unsupported(b"kernel32.dll!ReadDirectoryChangesW", 0);
    fail_with(ERROR_NOT_SUPPORTED, FALSE)
}

/// CreateNamedPipeA: named pipe servers are not there yet (anonymous
/// pipes are, see pipe.rs).
#[unsafe(no_mangle)]
pub extern "system" fn CreateNamedPipeA(
    _name: *const u8,
    _open_mode: u32,
    _pipe_mode: u32,
    _instances: u32,
    _out_size: u32,
    _in_size: u32,
    _timeout: u32,
    _attributes: *mut c_void,
) -> Handle {
    unsupported(b"kernel32.dll!CreateNamedPipeA", 0);
    fail_with(ERROR_NOT_SUPPORTED, INVALID_HANDLE_VALUE)
}

/// SetFileAttributesA: changing attributes is not there yet; recorded.
#[unsafe(no_mangle)]
pub extern "system" fn SetFileAttributesA(_path: *const u8, attributes: u32) -> Bool {
    unsupported(b"kernel32.dll!SetFileAttributesA", attributes.into());
    fail_with(ERROR_NOT_SUPPORTED, FALSE)
}

/// SetFilePointer(hFile, lDistanceToMove, *lpDistanceToMoveHigh, method):
/// the low half of the new position (INVALID_SET_FILE_POINTER and an
/// error on failure); the high half through the pointer.
#[unsafe(no_mangle)]
pub extern "system" fn SetFilePointer(h: Handle, low: i32, high: *mut i32, method: u32) -> u32 {
    let distance = if high.is_null() {
        i64::from(low)
    } else {
        // SAFETY: the caller's high half.
        (i64::from(unsafe { *high }) << 32) | i64::from(low as u32)
    };
    let mut new = 0i64;
    if SetFilePointerEx(h, distance, &mut new, method) == FALSE {
        return u32::MAX;
    }
    if !high.is_null() {
        // SAFETY: as above.
        unsafe { *high = (new >> 32) as i32 };
    }
    SetLastError(0);
    new as u32
}

/// SetSystemTime: programs may not set the clock here.
#[unsafe(no_mangle)]
pub extern "system" fn SetSystemTime(_time: *const u16) -> Bool {
    fail_with(1314, FALSE) // ERROR_PRIVILEGE_NOT_HELD
}

/// SleepEx(ms, alertable): WAIT_IO_COMPLETION (0xc0) if an APC ran.
#[unsafe(no_mangle)]
pub extern "system" fn SleepEx(ms: u32, alertable: Bool) -> u32 {
    let t: i64 = if ms == u32::MAX { i64::MIN } else { -(i64::from(ms) * 10_000) };
    // SAFETY: a valid interval.
    let status = unsafe { NtDelayExecution(u8::from(alertable != 0), &t) };
    if status == 0xc0 { 0xc0 } else { 0 } // STATUS_USER_APC
}

/// GetFinalPathNameByHandleW(hFile, lpszFilePath, cchFilePath, dwFlags):
/// the file's path from FileNameInformation (from the volume root) after
/// the volume: VOLUME_NAME_DOS (0) `\\?\C:`, VOLUME_NAME_NT (2) the
/// volume device, VOLUME_NAME_NONE (4) nothing. Returns the length, or
/// the size needed (with the NUL) if the buffer is too small.
#[unsafe(no_mangle)]
pub extern "system" fn GetFinalPathNameByHandleW(
    h: Handle,
    out: *mut u16,
    size: u32,
    flags: u32,
) -> u32 {
    // FILE_NAME_INFORMATION: FileNameLength, then the name.
    let mut info = [0u16; 2 + 1024];
    let mut iosb = [0usize; 2];
    // SAFETY: a buffer of the given size.
    let status = unsafe {
        NtQueryInformationFile(h, &mut iosb, info.as_mut_ptr().cast(), (info.len() * 2) as u32, 9)
    };
    if status != STATUS_SUCCESS {
        return fail(status, 0);
    }
    let name_len = ((u32::from(info[0]) | (u32::from(info[1]) << 16)) / 2) as usize;
    let name = &info[2..2 + name_len.min(1024)];
    let volume: &[u8] = match flags & 7 {
        0 => b"\\\\?\\C:",
        2 => b"\\Device\\HarddiskVolume1",
        4 => b"",
        _ => return fail_with(ERROR_INVALID_PARAMETER, 0),
    };
    let total = volume.len() + name.len();
    if out.is_null() || (size as usize) <= total {
        return total as u32 + 1;
    }
    // SAFETY: the caller's buffer of `size` > total characters.
    unsafe {
        for (i, &b) in volume.iter().enumerate() {
            *out.add(i) = u16::from(b);
        }
        core::ptr::copy_nonoverlapping(name.as_ptr(), out.add(volume.len()), name.len());
        *out.add(total) = 0;
    }
    total as u32
}

/// GetFinalPathNameByHandleA: the W function, converted to UTF-8.
#[unsafe(no_mangle)]
pub extern "system" fn GetFinalPathNameByHandleA(
    h: Handle,
    out: *mut u8,
    size: u32,
    flags: u32,
) -> u32 {
    let mut w = [0u16; 1100];
    let n = GetFinalPathNameByHandleW(h, w.as_mut_ptr(), w.len() as u32, flags) as usize;
    if n == 0 || n >= w.len() {
        return 0;
    }
    let needed = super::text::WideCharToMultiByte(
        65001,
        0,
        w.as_ptr(),
        n as i32,
        core::ptr::null_mut(),
        0,
        core::ptr::null(),
        core::ptr::null_mut(),
    );
    if needed <= 0 {
        return 0;
    }
    if out.is_null() || size as usize <= needed as usize {
        return needed as u32 + 1;
    }
    super::text::WideCharToMultiByte(
        65001,
        0,
        w.as_ptr(),
        n as i32,
        out,
        needed,
        core::ptr::null(),
        core::ptr::null_mut(),
    );
    // SAFETY: needed < size bytes were written.
    unsafe { *out.add(needed as usize) = 0 };
    needed as u32
}

const ERROR_FILE_NOT_FOUND_: u32 = 2;

/// Characters of SearchPath's candidate buffer (on the stack: frames
/// stay under 4 KiB, there is no __chkstk).
const SEARCH_CHARS: usize = 520;

/// Appends `parts` to `out` at `*n`; false if they do not fit (with a NUL).
fn append(out: &mut [u16; SEARCH_CHARS], n: &mut usize, parts: &[&[u16]]) -> bool {
    for part in parts {
        if *n + part.len() >= SEARCH_CHARS {
            return false;
        }
        out[*n..*n + part.len()].copy_from_slice(part);
        *n += part.len();
    }
    out[*n] = 0;
    true
}

/// Whether `dir` + `\\` + `name` + `ext` names an existing file; if so its
/// full path goes to the caller's buffer as SearchPathW returns it.
#[inline(never)] // its own frame: SearchPathW's stays under 4 KiB
fn try_candidate(
    dir: &[u16],
    name: &[u16],
    ext: &[u16],
    size: u32,
    buf: *mut u16,
    file_part: *mut *mut u16,
) -> Option<u32> {
    let mut c = [0u16; SEARCH_CHARS];
    let mut n = 0;
    let sep: &[u16] =
        if dir.is_empty() || dir.last().is_some_and(|&ch| is_sep(ch)) { &[] } else { &[BACKSLASH] };
    if !append(&mut c, &mut n, &[dir, sep, name, ext]) {
        return None;
    }
    let attributes = GetFileAttributesW(c.as_ptr());
    if attributes == u32::MAX || attributes & 0x10 != 0 {
        return None; // missing, or a directory
    }
    Some(GetFullPathNameW(c.as_ptr(), size, buf, file_part))
}

/// SearchPathW(path, file, extension, size, buf, file_part): the full path
/// of `file` (with `extension` if it has none) in the `;`-separated `path`,
/// or by default in the program's directory, System32, the Windows
/// directory, the current directory and PATH. The length without the
/// NUL, the size needed if `buf` is too small, or 0 (ERROR_FILE_NOT_FOUND_).
#[unsafe(no_mangle)]
pub extern "system" fn SearchPathW(
    path: *const u16,
    file: *const u16,
    extension: *const u16,
    size: u32,
    buf: *mut u16,
    file_part: *mut *mut u16,
) -> u32 {
    if file.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let name = wide_slice(file);
    let base = name.iter().rposition(|&c| is_sep(c) || c == u16::from(b':')).map_or(0, |i| i + 1);
    let ext: &[u16] = if extension.is_null() || name[base..].contains(&u16::from(b'.')) {
        &[]
    } else {
        wide_slice(extension)
    };
    // A name with a directory is only looked up there.
    if base > 0 {
        return try_candidate(&[], name, ext, size, buf, file_part)
            .unwrap_or_else(|| fail_with(ERROR_FILE_NOT_FOUND_, 0));
    }
    let mut list = [0u16; 1536];
    let dirs: &[u16] = if !path.is_null() {
        wide_slice(path)
    } else {
        // The default order: the program's directory, the system
        // directories, the current directory, then PATH.
        let mut n =
            super::process::GetModuleFileNameW(core::ptr::null_mut(), list.as_mut_ptr(), 520)
                as usize;
        n = list[..n.min(520)].iter().rposition(|&c| is_sep(c)).unwrap_or(0);
        for s in [
            &ascii16::<19>(b"C:\\Windows\\System32")[..],
            &ascii16::<10>(b"C:\\Windows"),
            &[u16::from(b'.')],
        ] {
            list[n] = u16::from(b';');
            list[n + 1..n + 1 + s.len()].copy_from_slice(s);
            n += 1 + s.len();
        }
        list[n] = u16::from(b';');
        n += 1;
        let name_path = ascii16::<5>(b"PATH\0");
        let room = (list.len() - n) as u32;
        let len = GetEnvironmentVariableW(name_path.as_ptr(), list[n..].as_mut_ptr(), room);
        if len < room {
            n += len as usize;
        }
        &list[..n]
    };
    for dir in dirs.split(|&c| c == u16::from(b';')) {
        if let Some(r) = try_candidate(dir, name, ext, size, buf, file_part) {
            return r;
        }
    }
    fail_with(ERROR_FILE_NOT_FOUND_, 0)
}

/// ASCII `s` as UTF-16.
const fn ascii16<const N: usize>(s: &[u8]) -> [u16; N] {
    let mut out = [0u16; N];
    let mut i = 0;
    while i < N {
        out[i] = s[i] as u16;
        i += 1;
    }
    out
}

/// RtlAddFunctionTable (kernel32 forwards it to ntdll): registers unwind
/// data for generated code (tcc -run). gala's exception dispatch only
/// reads images' .pdata: the table is accepted and not consulted (ledger).
#[unsafe(no_mangle)]
pub extern "system" fn RtlAddFunctionTable(_table: *const c_void, _count: u32, _base: u64) -> u8 {
    unsupported(b"kernel32.dll!RtlAddFunctionTable", 0);
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn RtlDeleteFunctionTable(_table: *const c_void) -> u8 {
    1
}

/// WaitForSingleObjectEx: WaitForSingleObject, alertable on request
/// (WAIT_IO_COMPLETION when an APC ran).
#[unsafe(no_mangle)]
pub extern "system" fn WaitForSingleObjectEx(h: Handle, ms: u32, alertable: Bool) -> u32 {
    let t = timeout(ms);
    let p = t.as_ref().map_or(core::ptr::null(), |t| t as *const i64);
    // SAFETY: valid or null timeout pointer.
    let status = unsafe { NtWaitForSingleObject(h, u8::from(alertable != 0), p) };
    match status {
        0 | 0x80 | 0xc0 | STATUS_TIMEOUT => status, // 0xc0: STATUS_USER_APC
        _ => fail(status, 0xffff_ffff),
    }
}

/// GetOverlappedResult(file, overlapped, &bytes, wait): the result of an
/// overlapped operation from OVERLAPPED.Internal (status) and
/// InternalHigh (bytes), waiting for it (on hEvent, or the file) if asked.
#[unsafe(no_mangle)]
pub extern "system" fn GetOverlappedResult(
    file: Handle,
    overlapped: *mut usize,
    bytes: *mut u32,
    wait: Bool,
) -> Bool {
    const ERROR_IO_INCOMPLETE: u32 = 996;
    // SAFETY: the caller's OVERLAPPED (Internal, InternalHigh, offsets,
    // hEvent at index 3).
    unsafe {
        let status = || core::ptr::read_volatile(overlapped) as u32;
        if status() == STATUS_PENDING {
            if wait == FALSE {
                return fail_with(ERROR_IO_INCOMPLETE, FALSE);
            }
            let event = *overlapped.add(3) as Handle;
            let on = if event.is_null() { file } else { event };
            while status() == STATUS_PENDING {
                if NtWaitForSingleObject(on, 0, core::ptr::null()) != 0 {
                    break;
                }
            }
        }
        if !bytes.is_null() {
            *bytes = *overlapped.add(1) as u32;
        }
        match status() {
            STATUS_SUCCESS => TRUE,
            s => fail(s, FALSE),
        }
    }
}

/// FlushViewOfFile: views of files are the file's own pages here, so
/// there is nothing to write back.
#[unsafe(no_mangle)]
pub extern "system" fn FlushViewOfFile(_base: *const c_void, _size: usize) -> Bool {
    TRUE
}

/// OutputDebugStringA/W: without a debugger attached, nothing happens.
#[unsafe(no_mangle)]
pub extern "system" fn OutputDebugStringA(_text: *const u8) {}

#[unsafe(no_mangle)]
pub extern "system" fn OutputDebugStringW(_text: *const u16) {}

/// AreFileApisANSI: the A functions use the ANSI code page (UTF-8 here).
#[unsafe(no_mangle)]
pub extern "system" fn AreFileApisANSI() -> Bool {
    TRUE
}
