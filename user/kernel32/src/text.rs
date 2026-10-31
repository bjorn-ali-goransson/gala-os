//! Code pages, system time conversion and the remaining functions OpenSSL
//! imports. gala's ANSI code page is UTF-8 (65001): every `A` function
//! takes UTF-8, so MultiByteToWideChar/WideCharToMultiByte convert UTF-8
//! for every code page they are given.

use super::*;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQuerySystemTime(time: *mut i64) -> u32;
}

const CP_UTF8: u32 = 65001;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_NO_UNICODE_TRANSLATION: u32 = 1113;

#[unsafe(no_mangle)]
pub extern "system" fn GetACP() -> u32 {
    CP_UTF8
}

#[unsafe(no_mangle)]
pub extern "system" fn GetOEMCP() -> u32 {
    CP_UTF8
}

/// GetCPInfo: every code page is UTF-8 here (up to 4 bytes a character,
/// default character '?', no DBCS lead bytes).
#[unsafe(no_mangle)]
pub extern "system" fn GetCPInfo(_code_page: u32, info: *mut u8) -> Bool {
    if info.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's CPINFO (20 bytes: MaxCharSize, DefaultChar[2],
    // LeadByte[12]).
    unsafe {
        core::ptr::write_bytes(info, 0, 20);
        info.cast::<u32>().write_unaligned(4);
        *info.add(4) = b'?';
    }
    TRUE
}

/// GetCPInfoExW: CPINFOEXW (MaxCharSize, DefaultChar[2], LeadByte[12],
/// UnicodeDefaultChar, CodePage, CodePageName[260]) of a code page gala
/// serves (CP_ACP, CP_OEMCP and CP_THREAD_ACP are the ANSI code page).
#[unsafe(no_mangle)]
pub extern "system" fn GetCPInfoExW(code_page: u32, _flags: u32, info: *mut u8) -> Bool {
    let cp = if matches!(code_page, 0 | 1 | 3) { GetACP() } else { code_page };
    let name: &str = match cp {
        65001 => "65001 (UTF-8)",
        1252 => "1252  (ANSI - Latin I)",
        437 => "437   (OEM - United States)",
        20127 => "20127 (US-ASCII)",
        _ => return fail_with(ERROR_INVALID_PARAMETER, FALSE),
    };
    if info.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's CPINFOEXW (24 + 520 bytes).
    unsafe {
        core::ptr::write_bytes(info, 0, 24 + 520);
        info.cast::<u32>().write_unaligned(if cp == 65001 { 4 } else { 1 });
        *info.add(4) = b'?';
        (info.add(18) as *mut u16).write_unaligned(0x3f);
        (info.add(20) as *mut u32).write_unaligned(cp);
        for (i, b) in name.bytes().enumerate() {
            (info.add(24 + 2 * i) as *mut u16).write_unaligned(b.into());
        }
    }
    TRUE
}

/// IsValidCodePage: UTF-8 (and the ASCII-compatible code pages programs
/// ask about are served as UTF-8 too).
#[unsafe(no_mangle)]
pub extern "system" fn IsValidCodePage(code_page: u32) -> Bool {
    if matches!(code_page, 65001 | 20127 | 1252 | 437) { TRUE } else { FALSE }
}

/// No double-byte code pages here.
#[unsafe(no_mangle)]
pub extern "system" fn IsDBCSLeadByteEx(_code_page: u32, _byte: u8) -> Bool {
    FALSE
}

/// MultiByteToWideChar(CodePage, dwFlags, lpMultiByteStr, cbMultiByte,
/// lpWideCharStr, cchWideChar): UTF-8 to UTF-16; -1 includes the
/// terminator; a zero output size asks for the length.
#[unsafe(no_mangle)]
pub extern "system" fn MultiByteToWideChar(
    _code_page: u32,
    flags: u32,
    src: *const u8,
    src_len: i32,
    dst: *mut u16,
    dst_len: i32,
) -> i32 {
    if src.is_null() || src_len == 0 {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    // SAFETY: the caller's string (NUL-terminated for -1).
    let bytes = unsafe {
        let n = if src_len < 0 { strlen(src) + 1 } else { src_len as usize };
        core::slice::from_raw_parts(src, n)
    };
    let text = match core::str::from_utf8(bytes) {
        Ok(t) => t,
        // MB_ERR_INVALID_CHARS: refuse; otherwise U+FFFD (lossy).
        Err(_) if flags & 8 != 0 => return fail_with(ERROR_NO_UNICODE_TRANSLATION, 0),
        Err(e) => core::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap_or(""),
    };
    let mut n = 0usize;
    for c in text.encode_utf16() {
        if dst_len > 0 {
            if n >= dst_len as usize {
                return fail_with(ERROR_INSUFFICIENT_BUFFER, 0);
            }
            // SAFETY: n < dst_len characters of the caller's buffer.
            unsafe { *dst.add(n) = c };
        }
        n += 1;
    }
    n as i32
}

/// WideCharToMultiByte: UTF-16 to UTF-8 (the default-character arguments
/// do not apply to UTF-8).
#[unsafe(no_mangle)]
pub extern "system" fn WideCharToMultiByte(
    _code_page: u32,
    _flags: u32,
    src: *const u16,
    src_len: i32,
    dst: *mut u8,
    dst_len: i32,
    _default_char: *const u8,
    used_default: *mut Bool,
) -> i32 {
    if src.is_null() || src_len == 0 {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let n = if src_len < 0 { wide_len(src) + 1 } else { src_len as usize };
    // SAFETY: the caller's string of `n` characters.
    let wide = unsafe { core::slice::from_raw_parts(src, n) };
    if !used_default.is_null() {
        // SAFETY: the caller's flag.
        unsafe { *used_default = FALSE };
    }
    let mut at = 0usize;
    for c in char::decode_utf16(wide.iter().copied()) {
        let mut b = [0u8; 4];
        let e = c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut b);
        if dst_len > 0 {
            if at + e.len() > dst_len as usize {
                return fail_with(ERROR_INSUFFICIENT_BUFFER, 0);
            }
            // SAFETY: within the caller's buffer of `dst_len` bytes.
            unsafe { core::ptr::copy_nonoverlapping(e.as_ptr(), dst.add(at), e.len()) };
        }
        at += e.len();
    }
    at as i32
}

/// Days since 1970-01-01 of a civil date.
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// FILETIME (100 ns since 1601) of 1970-01-01.
const UNIX_EPOCH_FILETIME: i64 = 116_444_736_000_000_000;

/// GetSystemTime: SYSTEMTIME {year, month, day of week, day, hour,
/// minute, second, milliseconds} (UTC).
#[unsafe(no_mangle)]
pub extern "system" fn GetSystemTime(st: *mut u16) {
    let mut t = 0i64;
    // SAFETY: a valid output pointer.
    unsafe { NtQuerySystemTime(&mut t) };
    write_system_time(t, st);
}

/// FileTimeToSystemTime(*lpFileTime, *lpSystemTime): the UTC fields.
#[unsafe(no_mangle)]
pub extern "system" fn FileTimeToSystemTime(ft: *const u64, st: *mut u16) -> Bool {
    if ft.is_null() || st.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's FILETIME (maybe unaligned).
    let t = unsafe { ft.read_unaligned() };
    if t >= 0x8000_0000_0000_0000 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    write_system_time(t as i64, st);
    TRUE
}

/// SYSTEMTIME fields of `t` (100 ns since 1601) into the caller's `st`.
fn write_system_time(t: i64, st: *mut u16) {
    let since_epoch = t - UNIX_EPOCH_FILETIME;
    let days = since_epoch.div_euclid(864_000_000_000);
    let rest = since_epoch.rem_euclid(864_000_000_000);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let fields = [
        year,
        month,
        (days + 4).rem_euclid(7),
        day,
        rest / 36_000_000_000,
        rest / 600_000_000 % 60,
        rest / 10_000_000 % 60,
        rest / 10_000 % 1000,
    ];
    for (i, v) in fields.iter().enumerate() {
        // SAFETY: the caller's SYSTEMTIME (eight WORDs).
        unsafe { *st.add(i) = *v as u16 };
    }
}

/// SystemTimeToFileTime: SYSTEMTIME (the day of week is ignored) to
/// 100 ns since 1601.
#[unsafe(no_mangle)]
pub extern "system" fn SystemTimeToFileTime(st: *const u16, ft: *mut u64) -> Bool {
    // SAFETY: the caller's SYSTEMTIME.
    let f = |i: usize| i64::from(unsafe { *st.add(i) });
    let (year, month, day) = (f(0), f(1), f(3));
    if !(1601..=30827).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let secs = days_from_civil(year, month, day) * 86_400 + f(4) * 3600 + f(5) * 60 + f(6);
    let t = secs * 10_000_000 + f(7) * 10_000 + UNIX_EPOCH_FILETIME;
    // SAFETY: the caller's FILETIME.
    unsafe { core::ptr::write_unaligned(ft, t as u64) };
    TRUE
}

/// GetVersion: 6.2 build 9200 (like GetVersionExA, as Windows answers
/// programs without a compatibility manifest).
#[unsafe(no_mangle)]
pub extern "system" fn GetVersion() -> u32 {
    let (major, minor, build) = super::mapping::reported_version();
    (build << 16) | (minor << 8) | major
}

/// GetModuleHandleExW: by name, or (GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS)
/// the image containing an address; pinning and reference counts do not
/// apply (gala does not unload images that are in use anyway).
#[unsafe(no_mangle)]
pub extern "system" fn GetModuleHandleExW(
    flags: u32,
    name: *const u16,
    module: *mut Handle,
) -> Bool {
    const FROM_ADDRESS: u32 = 4;
    if module.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let found = if flags & FROM_ADDRESS != 0 {
        // MEMORY_BASIC_INFORMATION.AllocationBase of the address.
        let mut info = [0usize; 6];
        // SAFETY: a 48-byte buffer.
        let status = unsafe {
            NtQueryVirtualMemory(
                CURRENT_PROCESS,
                name as usize,
                0,
                info.as_mut_ptr().cast(),
                48,
                core::ptr::null_mut(),
            )
        };
        if status != STATUS_SUCCESS {
            return fail(status, FALSE);
        }
        info[1] as Handle
    } else {
        super::mapping::GetModuleHandleW(name)
    };
    // SAFETY: the caller's output pointer.
    unsafe { *module = found };
    if found.is_null() {
        return FALSE;
    }
    // A reference the caller drops with FreeLibrary, unless
    // GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT (2); FLAG_PIN (1) pins.
    if flags & UNCHANGED_REFCOUNT == 0 {
        // SAFETY: a plain call.
        let status = unsafe { LdrAddRefDll(flags & PIN, found) };
        if status != STATUS_SUCCESS {
            return fail(status, FALSE);
        }
    }
    TRUE
}

const PIN: u32 = 1;
const UNCHANGED_REFCOUNT: u32 = 2;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn LdrAddRefDll(flags: u32, module: Handle) -> u32;
}

// Fibers: not yet (OpenSSL's asynchronous jobs use them; Nginx does not
// start any). Each fails as documented and is reported.

#[unsafe(no_mangle)]
pub extern "system" fn ConvertThreadToFiberEx(_parameter: *mut c_void, _flags: u32) -> *mut c_void {
    unsupported(b"kernel32.dll!ConvertThreadToFiberEx", 0);
    fail_with(ERROR_NOT_SUPPORTED, core::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "system" fn ConvertFiberToThread() -> Bool {
    unsupported(b"kernel32.dll!ConvertFiberToThread", 0);
    fail_with(ERROR_NOT_SUPPORTED, FALSE)
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateFiberEx(
    _commit: usize,
    _reserve: usize,
    _flags: u32,
    _start: usize,
    _parameter: *mut c_void,
) -> *mut c_void {
    unsupported(b"kernel32.dll!CreateFiberEx", 0);
    fail_with(ERROR_NOT_SUPPORTED, core::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "system" fn DeleteFiber(_fiber: *mut c_void) {
    unsupported(b"kernel32.dll!DeleteFiber", 0);
}

#[unsafe(no_mangle)]
pub extern "system" fn SwitchToFiber(_fiber: *mut c_void) {
    unsupported(b"kernel32.dll!SwitchToFiber", 0);
}
