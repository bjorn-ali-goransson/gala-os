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

/// LockFileEx: granted at once. gala has no byte-range locks yet (they
/// only matter between processes sharing a file); recorded.
#[unsafe(no_mangle)]
pub extern "system" fn LockFileEx(
    _h: Handle,
    flags: u32,
    _reserved: u32,
    _low: u32,
    _high: u32,
    _overlapped: *mut c_void,
) -> Bool {
    unsupported(b"kernel32.dll!LockFileEx", flags.into());
    TRUE
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

/// UnlockFile: nothing is locked (see LockFileEx).
#[unsafe(no_mangle)]
pub extern "system" fn UnlockFile(
    _h: Handle,
    _low: u32,
    _high: u32,
    _nlow: u32,
    _nhigh: u32,
) -> Bool {
    TRUE
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
