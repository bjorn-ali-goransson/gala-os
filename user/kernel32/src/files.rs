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
