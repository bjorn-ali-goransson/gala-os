//! More file functions: sizes, locks, device control, change
//! notification, named pipe servers.

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
    fn NtNotifyChangeDirectoryFile(
        h: Handle,
        event: usize,
        apc: usize,
        apc_context: usize,
        iosb: *mut [usize; 2],
        buffer: *mut u8,
        length: u32,
        filter: u32,
        tree: u8,
    ) -> u32;
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

/// ReadDirectoryChangesW(hDirectory, lpBuffer, nBufferLength,
/// bWatchSubtree, dwNotifyFilter, lpBytesReturned, lpOverlapped,
/// lpCompletionRoutine), over NtNotifyChangeDirectoryFile. Without an
/// OVERLAPPED it waits for a change; with one it returns at once (TRUE),
/// the OVERLAPPED being the status block, its hEvent the event and,
/// unless its low bit is set, the completion packet's context; a
/// completion routine runs as an APC. A buffer overflow
/// (STATUS_NOTIFY_ENUM_DIR) succeeds with 0 bytes: the caller rescans.
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn ReadDirectoryChangesW(
    h: Handle,
    buffer: *mut c_void,
    length: u32,
    subtree: Bool,
    filter: u32,
    returned: *mut u32,
    overlapped: *mut c_void,
    routine: usize,
) -> Bool {
    const STATUS_NOTIFY_ENUM_DIR: u32 = 0x10c;
    let tree = u8::from(subtree != 0);
    if overlapped.is_null() {
        let mut iosb = [0usize; 2];
        // SAFETY: the caller's buffer of `length` bytes; our status block.
        let mut status = unsafe {
            NtNotifyChangeDirectoryFile(h, 0, 0, 0, &mut iosb, buffer.cast(), length, filter, tree)
        };
        if status == STATUS_PENDING {
            // SAFETY: waits for the request on an asynchronous handle.
            unsafe { NtWaitForSingleObject(h, 0, core::ptr::null()) };
            status = iosb[0] as u32;
        }
        if status != STATUS_SUCCESS && status != STATUS_NOTIFY_ENUM_DIR {
            return fail(status, FALSE);
        }
        if !returned.is_null() {
            // SAFETY: the caller's output.
            unsafe { *returned = iosb[1] as u32 };
        }
        return TRUE;
    }
    let ov = overlapped.cast::<usize>();
    // SAFETY: the caller's OVERLAPPED, valid until the request completes.
    let event = unsafe {
        *ov = STATUS_PENDING as usize;
        *ov.add(1) = 0;
        *ov.add(3)
    };
    let (apc, context) = if routine != 0 {
        (io_completion_trampoline as *const () as usize, routine)
    } else {
        (0, if event & 1 != 0 { 0 } else { ov as usize })
    };
    // SAFETY: as above; the caller's buffer of `length` bytes.
    let status = unsafe {
        NtNotifyChangeDirectoryFile(
            h,
            event & !1,
            apc,
            context,
            ov.cast(),
            buffer.cast(),
            length,
            filter,
            tree,
        )
    };
    match status {
        STATUS_SUCCESS | STATUS_PENDING | STATUS_NOTIFY_ENUM_DIR => TRUE,
        _ => fail(status, FALSE),
    }
}

/// An I/O completion APC (ApcContext = the caller's
/// LPOVERLAPPED_COMPLETION_ROUTINE, the IO_STATUS_BLOCK = its OVERLAPPED):
/// calls routine(error code, bytes, overlapped).
extern "system" fn io_completion_trampoline(routine: usize, iosb: *mut usize, _reserved: usize) {
    // SAFETY: the routine the caller gave and its OVERLAPPED.
    unsafe {
        let routine: extern "system" fn(u32, u32, *mut usize) = core::mem::transmute(routine);
        let status = *iosb as u32;
        let error = if status == STATUS_SUCCESS { 0 } else { RtlNtStatusToDosError(status) };
        routine(error, *iosb.add(1) as u32, iosb);
    }
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
        // VOLUME_NAME_GUID: the GUID path without its final backslash.
        1 => &super::volume::VOLUME_GUID_PATH[..super::volume::VOLUME_GUID_PATH.len() - 1],
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

/// The Windows directory, KUSER_SHARED_DATA.NtSystemRoot (`C:\\Windows`),
/// as bytes in `out`, with `\\system32` after it for the system directory
/// (as Windows spells it); the length.
fn system_root(out: &mut [u8; 280], system32: bool) -> usize {
    let mut n = 0;
    // SAFETY: KUSER_SHARED_DATA.NtSystemRoot, a NUL-terminated WCHAR[260].
    unsafe {
        while n < 260 {
            let c = core::ptr::read_volatile((0x7ffe_0030 + 2 * n) as *const u16);
            if c == 0 {
                break;
            }
            out[n] = if c < 0x80 { c as u8 } else { b'?' };
            n += 1;
        }
    }
    if system32 {
        for &b in b"\\system32" {
            out[n] = b;
            n += 1;
        }
    }
    n
}

/// GetWindowsDirectoryW / A, GetSystemWindowsDirectoryW, GetSystemDirectoryW
/// / A: the length without the NUL, or the size needed with it if `buf` is
/// too small.
fn put_directory<T: From<u8> + Copy>(system32: bool, buf: *mut T, size: u32) -> u32 {
    let mut path = [0u8; 280];
    let n = system_root(&mut path, system32);
    if buf.is_null() || (size as usize) <= n {
        return n as u32 + 1;
    }
    // SAFETY: the caller's buffer of `size` > n characters.
    unsafe {
        for (i, &c) in path[..n].iter().enumerate() {
            *buf.add(i) = T::from(c);
        }
        *buf.add(n) = T::from(0);
    }
    n as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn GetWindowsDirectoryW(buf: *mut u16, size: u32) -> u32 {
    put_directory(false, buf, size)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetWindowsDirectoryA(buf: *mut u8, size: u32) -> u32 {
    put_directory(false, buf, size)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetSystemWindowsDirectoryW(buf: *mut u16, size: u32) -> u32 {
    put_directory(false, buf, size)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32 {
    put_directory(true, buf, size)
}

#[unsafe(no_mangle)]
pub extern "system" fn GetSystemDirectoryA(buf: *mut u8, size: u32) -> u32 {
    put_directory(true, buf, size)
}

/// The AddDllDirectory directories (full paths, NUL-terminated), at most
/// DLL_DIRECTORIES; a cookie is its slot + 1.
const DLL_DIRECTORIES: usize = 8;
static mut DLL_DIRS: [[u16; 260]; DLL_DIRECTORIES] = [[0; 260]; DLL_DIRECTORIES];
static DLL_DIRS_LOCK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn with_dll_dirs<R>(f: impl FnOnce(&mut [[u16; 260]; DLL_DIRECTORIES]) -> R) -> R {
    use core::sync::atomic::Ordering;
    while DLL_DIRS_LOCK
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    let dirs = &raw mut DLL_DIRS;
    // SAFETY: the table, under its lock.
    let r = f(unsafe { &mut *dirs });
    DLL_DIRS_LOCK.store(false, Ordering::Release);
    r
}

/// AddDllDirectory(path): a full path searched by LoadLibraryEx with
/// LOAD_LIBRARY_SEARCH_USER_DIRS; a cookie for RemoveDllDirectory.
#[unsafe(no_mangle)]
pub extern "system" fn AddDllDirectory(path: *const u16) -> *mut c_void {
    if path.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, core::ptr::null_mut());
    }
    let p = wide_slice(path);
    // A full path: a drive and a separator.
    if p.len() < 3 || p.len() >= 259 || p[1] != u16::from(b':') || !is_sep(p[2]) {
        return fail_with(ERROR_INVALID_PARAMETER, core::ptr::null_mut());
    }
    let slot = with_dll_dirs(|dirs| {
        let i = dirs.iter().position(|d| d[0] == 0)?;
        dirs[i][..p.len()].copy_from_slice(p);
        dirs[i][p.len()] = 0;
        Some(i)
    });
    match slot {
        Some(i) => (i + 1) as *mut c_void,
        None => fail_with(ERROR_NOT_ENOUGH_MEMORY, core::ptr::null_mut()),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn RemoveDllDirectory(cookie: *mut c_void) -> Bool {
    let i = (cookie as usize).wrapping_sub(1);
    let removed = with_dll_dirs(|dirs| match dirs.get_mut(i) {
        Some(d) if d[0] != 0 => {
            d[0] = 0;
            true
        }
        _ => false,
    });
    if removed { TRUE } else { fail_with(ERROR_INVALID_PARAMETER, FALSE) }
}

/// LoadLibrary of a bare `name` from the AddDllDirectory directories.
pub(crate) fn load_from_dll_directories(name: *const u16) -> Option<Handle> {
    let n = wide_slice(name);
    if n.iter().any(|&c| is_sep(c) || c == u16::from(b':')) {
        return None;
    }
    for i in 0..DLL_DIRECTORIES {
        let mut full = [0u16; 520];
        let len = with_dll_dirs(|dirs| {
            let d = &dirs[i];
            let dl = d.iter().position(|&c| c == 0).unwrap_or(0);
            if dl == 0 {
                return 0;
            }
            full[..dl].copy_from_slice(&d[..dl]);
            let mut at = dl;
            if !is_sep(d[dl - 1]) {
                full[at] = BACKSLASH;
                at += 1;
            }
            if at + n.len() >= full.len() {
                return 0;
            }
            full[at..at + n.len()].copy_from_slice(n);
            at + n.len()
        });
        if len == 0 || GetFileAttributesW(full.as_ptr()) == u32::MAX {
            continue;
        }
        return Some(LoadLibraryW(full.as_ptr()));
    }
    None
}
