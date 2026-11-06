//! Window stations and desktops: kernel objects (the kernel's
//! nt::winsta), reached through gala's GalaUserObject* services
//! (ARCHITECTURE "Window stations and desktops"). The process's window
//! station and each thread's desktop are chosen by the kernel (from
//! STARTUPINFO.lpDesktop, the parent, an inherited handle or the logon
//! session) at the first of these calls; their handles cannot be closed
//! while in use. OpenSSL tells a service from GetProcessWindowStation's
//! name or USEROBJECTFLAGS.dwFlags' WSF_VISIBLE; Chromium checks that the
//! input desktop is "Default".

use super::set_last_error;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaUserObjectOpen(
        kind: u32,
        name: *const u16,
        length: u32,
        access: u32,
        inherit: u32,
        handle: *mut usize,
    ) -> u32;
    fn GalaUserObjectConnection(what: u32, tid: u32, handle: *mut usize) -> u32;
    fn GalaUserObjectSet(what: u32, handle: usize) -> u32;
    fn GalaUserObjectClose(kind: u32, handle: usize) -> u32;
    fn GalaUserObjectInformation(
        handle: usize,
        index: u32,
        buffer: *mut u8,
        length: u32,
        needed: *mut u32,
    ) -> u32;
    fn GalaUserObjectEnum(handle: usize, buffer: *mut u8, length: u32, needed: *mut u32) -> u32;
    fn RtlNtStatusToDosError(status: u32) -> u32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
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
    fn GetProcessHeap() -> usize;
    fn HeapAlloc(heap: usize, flags: u32, size: usize) -> *mut u8;
    fn HeapFree(heap: usize, flags: u32, p: *mut u8) -> i32;
}

const STATUS_BUFFER_TOO_SMALL: u32 = 0xc000_0023;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_NOT_SUPPORTED: u32 = 50;
const UOI_NAME: i32 = 2;
const UOI_TYPE: i32 = 3;
/// The longest window station or desktop name handled (characters).
const MAX_NAME: usize = 256;

/// `value` if `status` is success, else 0 with the last error set from it.
fn done<T: Default>(status: u32, value: T) -> T {
    if status == 0 {
        return value;
    }
    // SAFETY: a plain call.
    set_last_error(unsafe { RtlNtStatusToDosError(status) });
    T::default()
}

/// The length of a NUL-terminated UTF-16 string (null: 0), at most
/// MAX_NAME + 1 (the kernel refuses longer names).
fn wide_len(s: *const u16) -> u32 {
    if s.is_null() {
        return 0;
    }
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    while n <= MAX_NAME && unsafe { *s.add(n) } != 0 {
        n += 1;
    }
    n as u32
}

/// An ANSI name as UTF-16 (NUL-terminated), or None if too long.
fn widen(s: *const u8, out: &mut [u16; MAX_NAME + 1]) -> Option<()> {
    if s.is_null() {
        out[0] = 0;
        return Some(());
    }
    // SAFETY: the caller's NUL-terminated string; our buffer.
    let n = unsafe { MultiByteToWideChar(0, 0, s, -1, out.as_mut_ptr(), out.len() as i32) };
    (n > 0).then_some(())
}

/// Opens window station (0) or desktop (1) `name`, or the input desktop
/// (2).
fn open(kind: u32, name: *const u16, inherit: i32, access: u32) -> usize {
    let mut handle = 0;
    // SAFETY: the caller's name of `wide_len` characters; our output.
    let status = unsafe {
        GalaUserObjectOpen(kind, name, wide_len(name), access, u32::from(inherit != 0), &mut handle)
    };
    done(status, handle)
}

/// GetProcessWindowStation: the process's window station (the same
/// handle each time; CloseWindowStation and CloseHandle refuse it).
#[unsafe(no_mangle)]
pub extern "system" fn GetProcessWindowStation() -> usize {
    let mut handle = 0;
    // SAFETY: our output.
    let status = unsafe { GalaUserObjectConnection(0, 0, &mut handle) };
    done(status, handle)
}

/// SetProcessWindowStation(hWinSta).
#[unsafe(no_mangle)]
pub extern "system" fn SetProcessWindowStation(station: usize) -> i32 {
    // SAFETY: a plain call.
    done(unsafe { GalaUserObjectSet(0, station) }, 1)
}

/// OpenWindowStationW(lpszWinSta, fInherit, dwDesiredAccess): a window
/// station of the caller's session by name.
#[unsafe(no_mangle)]
pub extern "system" fn OpenWindowStationW(name: *const u16, inherit: i32, access: u32) -> usize {
    open(0, name, inherit, access)
}

#[unsafe(no_mangle)]
pub extern "system" fn OpenWindowStationA(name: *const u8, inherit: i32, access: u32) -> usize {
    let mut wide = [0u16; MAX_NAME + 1];
    if widen(name, &mut wide).is_none() {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    open(0, wide.as_ptr(), inherit, access)
}

/// CloseWindowStation(hWinSta): fails for a handle that is no window
/// station (ERROR_INVALID_HANDLE) and for the process's own (ERROR_BUSY).
#[unsafe(no_mangle)]
pub extern "system" fn CloseWindowStation(station: usize) -> i32 {
    // SAFETY: a plain call.
    done(unsafe { GalaUserObjectClose(0, station) }, 1)
}

/// GetThreadDesktop(dwThreadId): the desktop handle of a thread of the
/// calling process (the same for every thread on the startup desktop).
#[unsafe(no_mangle)]
pub extern "system" fn GetThreadDesktop(tid: u32) -> usize {
    if tid == 0 {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    let mut handle = 0;
    // SAFETY: our output.
    let status = unsafe { GalaUserObjectConnection(1, tid, &mut handle) };
    done(status, handle)
}

/// SetThreadDesktop(hDesktop): the calling thread's desktop handle.
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadDesktop(desktop: usize) -> i32 {
    // SAFETY: a plain call.
    done(unsafe { GalaUserObjectSet(1, desktop) }, 1)
}

/// OpenDesktopW(lpszDesktop, dwFlags, fInherit, dwDesiredAccess): a
/// desktop of the process's window station by name (dwFlags'
/// DF_ALLOWOTHERACCOUNTHOOK changes nothing: hooks do not cross accounts'
/// checks yet).
#[unsafe(no_mangle)]
pub extern "system" fn OpenDesktopW(
    name: *const u16,
    _flags: u32,
    inherit: i32,
    access: u32,
) -> usize {
    open(1, name, inherit, access)
}

#[unsafe(no_mangle)]
pub extern "system" fn OpenDesktopA(
    name: *const u8,
    flags: u32,
    inherit: i32,
    access: u32,
) -> usize {
    let mut wide = [0u16; MAX_NAME + 1];
    if widen(name, &mut wide).is_none() {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    OpenDesktopW(wide.as_ptr(), flags, inherit, access)
}

/// OpenInputDesktop(dwFlags, fInherit, dwDesiredAccess): a new handle to
/// the desktop that receives input (the process's window station's
/// Default, if that station can receive input).
#[unsafe(no_mangle)]
pub extern "system" fn OpenInputDesktop(_flags: u32, inherit: i32, access: u32) -> usize {
    open(2, core::ptr::null(), inherit, access)
}

/// CloseDesktop(hDesktop): fails for a handle that is no desktop
/// (ERROR_INVALID_HANDLE) and for one a thread uses (ERROR_BUSY).
#[unsafe(no_mangle)]
pub extern "system" fn CloseDesktop(desktop: usize) -> i32 {
    // SAFETY: a plain call.
    done(unsafe { GalaUserObjectClose(1, desktop) }, 1)
}

/// GetUserObjectInformationW(hObj, nIndex, pvInfo, nLength,
/// lpnLengthNeeded) of a window station or desktop: UOI_FLAGS, UOI_NAME,
/// UOI_TYPE, UOI_IO; *lpnLengthNeeded the bytes (with the NUL), also
/// when the buffer is short (ERROR_INSUFFICIENT_BUFFER).
#[unsafe(no_mangle)]
pub extern "system" fn GetUserObjectInformationW(
    object: usize,
    index: i32,
    info: *mut u8,
    length: u32,
    needed: *mut u32,
) -> i32 {
    if index < 0 {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    // SAFETY: the caller's buffer of `length` bytes and output.
    let status = unsafe { GalaUserObjectInformation(object, index as u32, info, length, needed) };
    done(status, 1)
}

/// GetUserObjectInformationA: UOI_NAME and UOI_TYPE as ANSI text; when
/// the buffer is short, *lpnLengthNeeded is the UTF-16 size (as Windows
/// reports it), on success the ANSI bytes with the NUL.
#[unsafe(no_mangle)]
pub extern "system" fn GetUserObjectInformationA(
    object: usize,
    index: i32,
    info: *mut u8,
    length: u32,
    needed: *mut u32,
) -> i32 {
    if index != UOI_NAME && index != UOI_TYPE {
        return GetUserObjectInformationW(object, index, info, length, needed);
    }
    let mut wide = [0u16; MAX_NAME + 1];
    let mut wide_needed = 0u32;
    // SAFETY: our buffer and output.
    let status = unsafe {
        GalaUserObjectInformation(
            object,
            index as u32,
            wide.as_mut_ptr().cast(),
            2 * wide.len() as u32,
            &mut wide_needed,
        )
    };
    if status != 0 {
        if !needed.is_null() {
            // SAFETY: the caller's output.
            unsafe { *needed = wide_needed };
        }
        return done(status, 0);
    }
    let chars = (wide_needed / 2) as i32;
    // SAFETY: our string of `chars` units (with its NUL).
    let bytes = unsafe {
        WideCharToMultiByte(
            0,
            0,
            wide.as_ptr(),
            chars,
            core::ptr::null_mut(),
            0,
            core::ptr::null(),
            core::ptr::null_mut(),
        )
    };
    if info.is_null() || (length as i32) < bytes {
        if !needed.is_null() {
            // SAFETY: the caller's output.
            unsafe { *needed = wide_needed };
        }
        return done(STATUS_BUFFER_TOO_SMALL, 0);
    }
    // SAFETY: the caller's buffer of `length` bytes.
    unsafe {
        WideCharToMultiByte(
            0,
            0,
            wide.as_ptr(),
            chars,
            info,
            length as i32,
            core::ptr::null(),
            core::ptr::null_mut(),
        );
        if !needed.is_null() {
            *needed = bytes as u32;
        }
    }
    1
}

/// Calls `callback` with each name of the list GalaUserObjectEnum gives
/// for `handle` (0: the session's window stations); its last answer, or
/// 0 when it stops the walk.
fn enumerate(
    handle: usize,
    callback: unsafe extern "system" fn(*const u16, isize) -> i32,
    param: isize,
) -> i32 {
    let mut small = [0u16; 1024];
    let mut needed = 0;
    // SAFETY: our buffer and output.
    let mut status =
        unsafe { GalaUserObjectEnum(handle, small.as_mut_ptr().cast(), 2048, &mut needed) };
    let mut list = small.as_mut_ptr();
    let mut heap = core::ptr::null_mut();
    if status == STATUS_BUFFER_TOO_SMALL {
        // SAFETY: a plain allocation of `needed` bytes, then the call.
        unsafe {
            heap = HeapAlloc(GetProcessHeap(), 0, needed as usize);
            if heap.is_null() {
                set_last_error(8); // ERROR_NOT_ENOUGH_MEMORY
                return 0;
            }
            status = GalaUserObjectEnum(handle, heap, needed, &mut needed);
        }
        list = heap.cast();
    }
    let mut result = 0;
    if status == 0 {
        let mut at = 0;
        result = 1;
        // SAFETY: the kernel's list: NUL-terminated names, then a NUL.
        unsafe {
            while *list.add(at) != 0 {
                result = callback(list.add(at), param);
                if result == 0 {
                    break;
                }
                while *list.add(at) != 0 {
                    at += 1;
                }
                at += 1;
            }
        }
    } else {
        done(status, 0);
    }
    if !heap.is_null() {
        // SAFETY: our allocation.
        unsafe { HeapFree(GetProcessHeap(), 0, heap) };
    }
    result
}

/// EnumWindowStationsW(lpEnumFunc, lParam): the caller's session's
/// window stations.
#[unsafe(no_mangle)]
pub extern "system" fn EnumWindowStationsW(
    callback: unsafe extern "system" fn(*const u16, isize) -> i32,
    param: isize,
) -> i32 {
    enumerate(0, callback, param)
}

/// EnumDesktopsW(hwinsta, lpEnumFunc, lParam): a window station's
/// desktops (WINSTA_ENUMDESKTOPS; a null station is the process's).
#[unsafe(no_mangle)]
pub extern "system" fn EnumDesktopsW(
    station: usize,
    callback: unsafe extern "system" fn(*const u16, isize) -> i32,
    param: isize,
) -> i32 {
    let station = if station == 0 { GetProcessWindowStation() } else { station };
    if station == 0 {
        return 0;
    }
    enumerate(station, callback, param)
}

// ---- Window stations and desktops made by programs (the sandbox's) --------

/// CreateWindowStationW / CreateDesktopW: not supported yet (BR stage 3,
/// the sandbox's alternate desktop; reported).
#[unsafe(no_mangle)]
pub extern "system" fn CreateWindowStationW(
    _name: *const u16,
    _flags: u32,
    _access: u32,
    _sa: *const u8,
) -> usize {
    super::window::unsupported(b"user32.dll!CreateWindowStationW", 0);
    set_last_error(ERROR_NOT_SUPPORTED);
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateDesktopW(
    _name: *const u16,
    _device: *const u16,
    _mode: *const u8,
    _flags: u32,
    _access: u32,
    _sa: *const u8,
) -> usize {
    super::window::unsupported(b"user32.dll!CreateDesktopW", 0);
    set_last_error(ERROR_NOT_SUPPORTED);
    0
}
