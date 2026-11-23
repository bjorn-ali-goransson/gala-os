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

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
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

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
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
const ERROR_INVALID_FUNCTION: u32 = 1;
const ERROR_INVALID_DATA: u32 = 13;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_NOT_SUPPORTED: u32 = 50;
const UOI_FLAGS: i32 = 1;
const UOI_NAME: i32 = 2;
const UOI_TYPE: i32 = 3;
const UOI_TIMERPROC_EXCEPTION_SUPPRESSION: i32 = 7;
/// The bytes of a USEROBJECTFLAGS (fInherit, fReserved, dwFlags).
const USEROBJECTFLAGS: usize = 12;
/// GetCurrentProcess()'s pseudo handle, which SetUserObjectInformation
/// takes instead of a user object for the TIMERPROC suppression.
const CURRENT_PROCESS: usize = usize::MAX;
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
/// UOI_TYPE, UOI_IO (a desktop's alone); *lpnLengthNeeded the bytes
/// (with the NUL), also when the buffer is short
/// (ERROR_INSUFFICIENT_BUFFER). The current process's pseudo handle has
/// nothing to read: the one thing it sets (the TIMERPROC exception
/// suppression) is write-only, and Windows answers
/// ERROR_INVALID_FUNCTION for it.
#[unsafe(no_mangle)]
pub extern "system" fn GetUserObjectInformationW(
    object: usize,
    index: i32,
    info: *mut u8,
    length: u32,
    needed: *mut u32,
) -> i32 {
    if object == CURRENT_PROCESS {
        set_last_error(ERROR_INVALID_FUNCTION);
        return 0;
    }
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

/// SetUserObjectInformationW(hObj, nIndex, pvInfo, nLength): two routes,
/// told apart by the handle (measured on 26100).
///
/// A window station or desktop handle takes UOI_FLAGS and nothing else
/// (ERROR_INVALID_PARAMETER); pvInfo must hold a whole USEROBJECTFLAGS
/// (ERROR_INVALID_DATA for a shorter buffer; a larger one is taken).
/// What a station is, gala decides and not a write through one of its
/// handles -- WSF_VISIBLE follows the interactive station of a user
/// (the kernel's nt::winsta) -- so a write of what
/// GetUserObjectInformationW already answers succeeds as on Windows,
/// and one that would change something succeeds with a row saying it
/// did not.
///
/// The current process's pseudo handle is the other route: it takes
/// UOI_TIMERPROC_EXCEPTION_SUPPRESSION and nothing else
/// (ERROR_INVALID_FUNCTION), a BOOL of exactly four bytes, and a real
/// handle to the same process is not it (ERROR_INVALID_PARAMETER).
/// gala calls a TIMERPROC with no exception handler around it, which is
/// what FALSE asks for and what Microsoft recommends; TRUE is recorded
/// and cannot be given. Office's shared runtime delay-loads this
/// function, so its absence ends the process.
///
/// A NULL pvInfo faults the caller on Windows; gala answers
/// ERROR_INVALID_PARAMETER instead (gala's choice).
#[unsafe(no_mangle)]
pub extern "system" fn SetUserObjectInformationW(
    object: usize,
    index: i32,
    info: *const u8,
    length: u32,
) -> i32 {
    if object == CURRENT_PROCESS {
        if index != UOI_TIMERPROC_EXCEPTION_SUPPRESSION {
            set_last_error(ERROR_INVALID_FUNCTION);
            return 0;
        }
        if length as usize != size_of::<u32>() || info.is_null() {
            set_last_error(ERROR_INVALID_PARAMETER);
            return 0;
        }
        // SAFETY: the caller's BOOL, the four bytes nLength covers.
        let suppress = unsafe { info.cast::<u32>().read_unaligned() };
        let what = b"user32.dll!SetUserObjectInformationW(UOI_TIMERPROC_EXCEPTION_SUPPRESSION)";
        if suppress != 0 {
            crate::window::unsupported(what, u64::from(suppress));
        } else {
            crate::window::reached(what, 0);
        }
        return 1;
    }
    if index != UOI_FLAGS {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    if (length as usize) < USEROBJECTFLAGS {
        set_last_error(ERROR_INVALID_DATA);
        return 0;
    }
    if info.is_null() {
        set_last_error(ERROR_INVALID_PARAMETER);
        return 0;
    }
    // The handle must be a window station or desktop, and what it holds
    // now is what gala can keep: the kernel's own answer says both.
    let mut held = [0u8; USEROBJECTFLAGS];
    let mut needed = 0;
    // SAFETY: our buffer and output.
    let status = unsafe {
        GalaUserObjectInformation(
            object,
            UOI_FLAGS as u32,
            held.as_mut_ptr(),
            USEROBJECTFLAGS as u32,
            &mut needed,
        )
    };
    if status != 0 {
        return done(status, 0);
    }
    // SAFETY: the caller's buffer, the bytes nLength covers.
    let asked = unsafe { core::slice::from_raw_parts(info, USEROBJECTFLAGS) };
    if asked != &held[..] {
        let flags = u32::from_le_bytes([asked[8], asked[9], asked[10], asked[11]]);
        crate::window::unsupported(
            b"user32.dll!SetUserObjectInformationW(UOI_FLAGS change)",
            u64::from(flags),
        );
    }
    1
}

/// SetUserObjectInformationA: the same call, since no index it takes
/// holds text.
#[unsafe(no_mangle)]
pub extern "system" fn SetUserObjectInformationA(
    object: usize,
    index: i32,
    info: *const u8,
    length: u32,
) -> i32 {
    SetUserObjectInformationW(object, index, info, length)
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
