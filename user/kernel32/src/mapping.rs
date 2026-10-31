//! File mappings (sections) and the remaining functions Nginx imports:
//! ANSI variants, module and version queries, process exit codes, file
//! times and sizes, disk space.

use super::sync::ansi_to_wide;
use super::*;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtCreateSection(
        handle: *mut Handle,
        access: u32,
        attributes: *const ObjectAttributes,
        size: *const i64,
        protect: u32,
        allocation: u32,
        file: Handle,
    ) -> u32;
    fn NtMapViewOfSection(
        section: Handle,
        process: isize,
        base: *mut *mut c_void,
        zero_bits: usize,
        commit: usize,
        offset: *const i64,
        size: *mut usize,
        inherit: u32,
        allocation: u32,
        protect: u32,
    ) -> u32;
    fn NtUnmapViewOfSection(process: isize, base: *mut c_void) -> u32;
    fn NtProtectVirtualMemory(
        process: isize,
        base: *mut *mut c_void,
        size: *mut usize,
        protect: u32,
        old: *mut u32,
    ) -> u32;
    fn NtQuerySystemTime(time: *mut i64) -> u32;
    fn LdrGetDllHandle(
        path: *const u16,
        flags: *const u32,
        name: *const UnicodeString,
        base: *mut Handle,
    ) -> u32;
}

const SECTION_ALL_ACCESS: u32 = 0xf_001f;
const SEC_COMMIT: u32 = 0x800_0000;
const VIEW_SHARE: u32 = 1;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

/// CreateFileMappingW: a section backed by the file, or by the page file
/// for INVALID_HANDLE_VALUE; optionally named.
#[unsafe(no_mangle)]
pub extern "system" fn CreateFileMappingW(
    file: Handle,
    attributes: *mut c_void,
    protect: u32,
    size_high: u32,
    size_low: u32,
    name: *const u16,
) -> Handle {
    if !attributes.is_null() {
        unsupported(b"kernel32.dll!CreateFileMappingW(security attributes)", 0);
    }
    let size = (i64::from(size_high) << 32) | i64::from(size_low);
    let size_ptr = if size == 0 { core::ptr::null() } else { &size as *const i64 };
    let file = if file == INVALID_HANDLE_VALUE { core::ptr::null_mut() } else { file };
    let mut h = core::ptr::null_mut();
    let mut path = [0u16; 300];
    let named = named_path(name, &mut path);
    let Some(len) = named else { return fail_with(ERROR_INVALID_PARAMETER, core::ptr::null_mut()) };
    let us = UnicodeString {
        length: (len * 2) as u16,
        maximum_length: (len * 2) as u16,
        buffer: path.as_ptr(),
    };
    let oa = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root_directory: 0,
        object_name: &us,
        attributes: 0x80, // OBJ_OPENIF
        security_descriptor: 0,
        security_quality_of_service: 0,
    };
    let oa_ptr = if name.is_null() { core::ptr::null() } else { &oa as *const ObjectAttributes };
    // SAFETY: valid pointers for the call.
    let status = unsafe {
        NtCreateSection(&mut h, SECTION_ALL_ACCESS, oa_ptr, size_ptr, protect, SEC_COMMIT, file)
    };
    match status {
        STATUS_SUCCESS => {
            SetLastError(0);
            h
        }
        STATUS_OBJECT_NAME_EXISTS => fail_with(ERROR_ALREADY_EXISTS, h),
        _ => fail(status, core::ptr::null_mut()),
    }
}

/// The session path of an object name in `path`; Some(0) for no name.
fn named_path(name: *const u16, path: &mut [u16; 300]) -> Option<usize> {
    if name.is_null() {
        return Some(0);
    }
    let mut n = 0;
    for c in
        "\\Sessions\\1\\BaseNamedObjects\\".encode_utf16().chain(wide_slice(name).iter().copied())
    {
        *path.get_mut(n)? = c;
        n += 1;
    }
    Some(n)
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateFileMappingA(
    file: Handle,
    attributes: *mut c_void,
    protect: u32,
    size_high: u32,
    size_low: u32,
    name: *const u8,
) -> Handle {
    let mut buf = [0u16; 260];
    let wide = if name.is_null() {
        core::ptr::null()
    } else if ansi_to_wide(name, &mut buf).is_some() {
        buf.as_ptr()
    } else {
        return fail_with(ERROR_INVALID_PARAMETER, core::ptr::null_mut());
    };
    CreateFileMappingW(file, attributes, protect, size_high, size_low, wide)
}

/// MapViewOfFileEx: FILE_MAP_* access to page protection; at `base` if
/// given.
#[unsafe(no_mangle)]
pub extern "system" fn MapViewOfFileEx(
    section: Handle,
    access: u32,
    offset_high: u32,
    offset_low: u32,
    bytes: usize,
    base: *mut c_void,
) -> *mut c_void {
    let protect = if access & 1 != 0 && access & 2 == 0 {
        0x08 // FILE_MAP_COPY: PAGE_WRITECOPY
    } else if access & 2 != 0 {
        0x04 // FILE_MAP_WRITE (and ALL_ACCESS): PAGE_READWRITE
    } else {
        0x02 // FILE_MAP_READ: PAGE_READONLY
    };
    let protect = if access & 0x20 != 0 { protect << 4 } else { protect }; // FILE_MAP_EXECUTE
    let offset = (i64::from(offset_high) << 32) | i64::from(offset_low);
    let (mut at, mut size) = (base, bytes);
    // SAFETY: valid in/out pointers.
    let status = unsafe {
        NtMapViewOfSection(
            section,
            CURRENT_PROCESS,
            &mut at,
            0,
            0,
            &offset,
            &mut size,
            VIEW_SHARE,
            0,
            protect,
        )
    };
    if status != STATUS_SUCCESS && status != 0x4000_0003 {
        // (STATUS_IMAGE_NOT_AT_BASE is a success.)
        return fail(status, core::ptr::null_mut());
    }
    at
}

#[unsafe(no_mangle)]
pub extern "system" fn MapViewOfFile(
    section: Handle,
    access: u32,
    offset_high: u32,
    offset_low: u32,
    bytes: usize,
) -> *mut c_void {
    MapViewOfFileEx(section, access, offset_high, offset_low, bytes, core::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "system" fn UnmapViewOfFile(base: *mut c_void) -> Bool {
    // SAFETY: the kernel validates the address.
    let status = unsafe { NtUnmapViewOfSection(CURRENT_PROCESS, base) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

#[unsafe(no_mangle)]
pub extern "system" fn VirtualProtect(
    address: *mut c_void,
    size: usize,
    protect: u32,
    old: *mut u32,
) -> Bool {
    let (mut base, mut len) = (address, size);
    // SAFETY: valid in/out pointers; `old` is the caller's.
    let status =
        unsafe { NtProtectVirtualMemory(CURRENT_PROCESS, &mut base, &mut len, protect, old) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

/// GetSystemTimeAsFileTime: 100 ns intervals since 1601 (UTC).
#[unsafe(no_mangle)]
pub extern "system" fn GetSystemTimeAsFileTime(time: *mut u64) {
    let mut t = 0i64;
    // SAFETY: valid output pointers.
    unsafe {
        NtQuerySystemTime(&mut t);
        core::ptr::write_unaligned(time, t as u64);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn TerminateProcess(process: Handle, code: u32) -> Bool {
    // SAFETY: the kernel validates the handle.
    let status = unsafe { NtTerminateProcess(process as isize, code) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

/// GetExitCodeProcess: the exit status, or STILL_ACTIVE (259).
#[unsafe(no_mangle)]
pub extern "system" fn GetExitCodeProcess(process: Handle, code: *mut u32) -> Bool {
    // PROCESS_BASIC_INFORMATION: ExitStatus first.
    let mut info = [0u8; 48];
    // SAFETY: a 48-byte buffer.
    let status = unsafe {
        NtQueryInformationProcess(process as isize, 0, info.as_mut_ptr(), 48, core::ptr::null_mut())
    };
    if status != STATUS_SUCCESS {
        return fail(status, FALSE);
    }
    // SAFETY: the caller's output pointer.
    unsafe { *code = u32::from_le_bytes([info[0], info[1], info[2], info[3]]) };
    TRUE
}

/// GetCommandLineA: the command line in the ANSI code page (UTF-8 on
/// gala), converted once.
#[unsafe(no_mangle)]
pub extern "system" fn GetCommandLineA() -> *const u8 {
    static LINE: core::sync::atomic::AtomicPtr<u8> =
        core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());
    let have = LINE.load(core::sync::atomic::Ordering::Acquire);
    if !have.is_null() {
        return have;
    }
    let wide = wide_slice(GetCommandLineW());
    let size = wide.len() * 3 + 1;
    let buf = VirtualAlloc(core::ptr::null_mut(), size, 0x3000, 4).cast::<u8>();
    if buf.is_null() {
        return c"".as_ptr().cast();
    }
    let n = narrow_into(wide, buf, size);
    // SAFETY: n < size.
    unsafe { *buf.add(n) = 0 };
    match LINE.compare_exchange(
        core::ptr::null_mut(),
        buf,
        core::sync::atomic::Ordering::AcqRel,
        core::sync::atomic::Ordering::Acquire,
    ) {
        Ok(_) => buf,
        Err(other) => {
            VirtualFree(buf.cast(), 0, 0x8000);
            other
        }
    }
}

/// Writes `wide` as UTF-8 into `out` (at most `size - 1` bytes); returns
/// the byte count.
fn narrow_into(wide: &[u16], out: *mut u8, size: usize) -> usize {
    let mut n = 0;
    for c in char::decode_utf16(wide.iter().copied()) {
        let mut b = [0u8; 4];
        let s = c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut b);
        if n + s.len() + 1 > size {
            break;
        }
        // SAFETY: within `size` bytes of the caller's buffer.
        unsafe { core::ptr::copy_nonoverlapping(s.as_ptr(), out.add(n), s.len()) };
        n += s.len();
    }
    n
}

/// GetEnvironmentVariableA: as GetEnvironmentVariableW, in UTF-8.
#[unsafe(no_mangle)]
pub extern "system" fn GetEnvironmentVariableA(name: *const u8, buf: *mut u8, size: u32) -> u32 {
    let mut wname = [0u16; 260];
    if ansi_to_wide(name, &mut wname).is_none() {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let mut value = [0u16; 4096];
    let n = GetEnvironmentVariableW(wname.as_ptr(), value.as_mut_ptr(), value.len() as u32);
    if n == 0 || n as usize >= value.len() {
        return 0; // not found (last error set), or too long for us
    }
    let mut bytes = [0u8; 3 * 4096];
    let len = narrow_into(&value[..n as usize], bytes.as_mut_ptr(), bytes.len());
    if len + 1 > size as usize {
        return (len + 1) as u32;
    }
    // SAFETY: the caller's buffer of `size` bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len);
        *buf.add(len) = 0;
    }
    len as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn SetEnvironmentVariableA(name: *const u8, value: *const u8) -> Bool {
    let (mut wname, mut wvalue) = ([0u16; 260], [0u16; 4096]);
    if ansi_to_wide(name, &mut wname).is_none() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    if value.is_null() {
        return SetEnvironmentVariableW(wname.as_ptr(), core::ptr::null());
    }
    if ansi_to_wide(value, &mut wvalue).is_none() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    SetEnvironmentVariableW(wname.as_ptr(), wvalue.as_ptr())
}

/// The PEB's ImageBaseAddress.
fn image_base() -> Handle {
    // SAFETY: gs:[0x60] is the PEB; ImageBaseAddress is at +0x10.
    unsafe { *((teb_u64(0x60) as usize + 0x10) as *const Handle) }
}

#[unsafe(no_mangle)]
pub extern "system" fn GetModuleHandleW(name: *const u16) -> Handle {
    if name.is_null() {
        return image_base();
    }
    let len = wide_len(name);
    let us =
        UnicodeString { length: (len * 2) as u16, maximum_length: (len * 2) as u16, buffer: name };
    let mut base = core::ptr::null_mut();
    // SAFETY: valid UNICODE_STRING and output pointer.
    let status = unsafe { LdrGetDllHandle(core::ptr::null(), core::ptr::null(), &us, &mut base) };
    if status != STATUS_SUCCESS { fail(status, core::ptr::null_mut()) } else { base }
}

#[unsafe(no_mangle)]
pub extern "system" fn GetModuleHandleA(name: *const u8) -> Handle {
    if name.is_null() {
        return image_base();
    }
    let mut wide = [0u16; 260];
    if ansi_to_wide(name, &mut wide).is_none() {
        return fail_with(ERROR_INVALID_PARAMETER, core::ptr::null_mut());
    }
    GetModuleHandleW(wide.as_ptr())
}

/// GetModuleFileNameA: the executable's path (other modules: reported).
#[unsafe(no_mangle)]
pub extern "system" fn GetModuleFileNameA(module: Handle, buf: *mut u8, size: u32) -> u32 {
    if !module.is_null() && module != image_base() {
        unsupported(b"kernel32.dll!GetModuleFileNameA(DLL)", 0);
        return fail_with(ERROR_NOT_SUPPORTED, 0);
    }
    if size == 0 {
        return fail_with(ERROR_INSUFFICIENT_BUFFER, 0);
    }
    // RTL_USER_PROCESS_PARAMETERS.ImagePathName (UNICODE_STRING at +0x60).
    // SAFETY: the PEB's process parameters are always set.
    let path = unsafe {
        let params = *((teb_u64(0x60) as usize + 0x20) as *const usize);
        let len = usize::from(*((params + 0x60) as *const u16)) / 2;
        let buffer = *((params + 0x68) as *const *const u16);
        core::slice::from_raw_parts(buffer, len)
    };
    let n = narrow_into(path, buf, size as usize);
    // SAFETY: n < size.
    unsafe { *buf.add(n) = 0 };
    if n + 1 >= size as usize && path.len() > n {
        // Truncated, as documented: the buffer size and this error.
        return fail_with(ERROR_INSUFFICIENT_BUFFER, size);
    }
    n as u32
}

/// GetVersionExA: like Windows for a program without a compatibility
/// manifest (Nginx has none): version 6.2, build 9200.
#[unsafe(no_mangle)]
pub extern "system" fn GetVersionExA(info: *mut u8) -> Bool {
    // SAFETY: the caller's OSVERSIONINFOA (148 bytes) or OSVERSIONINFOEXA
    // (156 bytes), its size in the first field.
    unsafe {
        let size = *(info as *const u32);
        if size != 148 && size != 156 {
            return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
        }
        core::ptr::write_bytes(info.add(4), 0, size as usize - 4);
        let fields = [6u32, 2, 9200, 2]; // major, minor, build, VER_PLATFORM_WIN32_NT
        for (i, v) in fields.iter().enumerate() {
            core::ptr::write_unaligned(info.add(4 + 4 * i).cast::<u32>(), *v);
        }
        if size == 156 {
            *info.add(154) = 1; // wProductType: VER_NT_WORKSTATION
        }
    }
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn FreeConsole() -> Bool {
    TRUE
}

/// SetStdHandle: replaces a standard handle in the process parameters.
#[unsafe(no_mangle)]
pub extern "system" fn SetStdHandle(which: u32, h: Handle) -> Bool {
    let offset = match which as i32 {
        -10 => 0x20,
        -11 => 0x28,
        -12 => 0x30,
        _ => return fail_with(6, FALSE),
    };
    // SAFETY: the PEB's process parameters are always set.
    unsafe {
        let params = *((teb_u64(0x60) as usize + 0x20) as *const usize);
        *((params + offset) as *mut Handle) = h;
    }
    TRUE
}

/// SetUnhandledExceptionFilter: remembered and returned; gala ends a
/// process with an unhandled exception without calling it (yet).
#[unsafe(no_mangle)]
pub extern "system" fn SetUnhandledExceptionFilter(filter: usize) -> usize {
    static FILTER: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    FILTER.swap(filter, core::sync::atomic::Ordering::AcqRel)
}

/// The volume is in memory: report a fixed geometry (512-byte sectors, 4 KiB
/// clusters) and 1 GiB free of 2 GiB.
const CLUSTERS_FREE: u32 = 1 << 18;
const CLUSTERS_TOTAL: u32 = 1 << 19;

#[unsafe(no_mangle)]
pub extern "system" fn GetDiskFreeSpaceW(
    _root: *const u16,
    sectors_per_cluster: *mut u32,
    bytes_per_sector: *mut u32,
    free_clusters: *mut u32,
    total_clusters: *mut u32,
) -> Bool {
    // SAFETY: the caller's optional output pointers.
    unsafe {
        for (p, v) in [
            (sectors_per_cluster, 8),
            (bytes_per_sector, 512),
            (free_clusters, CLUSTERS_FREE),
            (total_clusters, CLUSTERS_TOTAL),
        ] {
            if !p.is_null() {
                *p = v;
            }
        }
    }
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn GetDiskFreeSpaceExW(
    _directory: *const u16,
    available: *mut u64,
    total: *mut u64,
    free: *mut u64,
) -> Bool {
    let (f, t) = (u64::from(CLUSTERS_FREE) * 4096, u64::from(CLUSTERS_TOTAL) * 4096);
    // SAFETY: the caller's optional output pointers.
    unsafe {
        for (p, v) in [(available, f), (total, t), (free, f)] {
            if !p.is_null() {
                core::ptr::write_unaligned(p, v);
            }
        }
    }
    TRUE
}

/// GetLongPathNameW: gala has no short (8.3) names, so the path itself.
#[unsafe(no_mangle)]
pub extern "system" fn GetLongPathNameW(short: *const u16, long: *mut u16, size: u32) -> u32 {
    let path = wide_slice(short);
    if path.len() + 1 > size as usize {
        return (path.len() + 1) as u32;
    }
    // SAFETY: the caller's buffer of `size` characters.
    unsafe {
        core::ptr::copy(path.as_ptr(), long, path.len());
        *long.add(path.len()) = 0;
    }
    path.len() as u32
}

/// MoveFileW: renames a file or directory (not across volumes; an existing
/// target is an error).
#[unsafe(no_mangle)]
pub extern "system" fn MoveFileW(from: *const u16, to: *const u16) -> Bool {
    let target = match nt_path(to) {
        Ok(p) => p,
        Err(e) => return fail_with(e, FALSE),
    };
    let h = match open_path(
        from,
        0x0001_0000 | 0x0010_0000,
        SHARE_ALL,
        1,
        FILE_OPEN_FOR_BACKUP_INTENT | FILE_SYNCHRONOUS_IO_NONALERT,
        0,
    ) {
        Ok((h, _)) => h,
        Err(e) => return fail_with(e, FALSE),
    };
    // FILE_RENAME_INFORMATION: ReplaceIfExists, RootDirectory,
    // FileNameLength, FileName.
    let mut info = [0u8; 20 + 2 * MAX_PATH_CHARS];
    let name = target.as_slice();
    info[16..20].copy_from_slice(&((name.len() * 2) as u32).to_le_bytes());
    for (i, c) in name.iter().enumerate() {
        info[20 + 2 * i..22 + 2 * i].copy_from_slice(&c.to_le_bytes());
    }
    let mut iosb = [0usize; 2];
    let len = 20 + 2 * name.len();
    // SAFETY: a buffer of `len` bytes.
    let status = unsafe { NtSetInformationFile(h, &mut iosb, info.as_ptr(), len as u32, 10) };
    CloseHandle(h);
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

/// SetEndOfFile: the file ends at the current position.
#[unsafe(no_mangle)]
pub extern "system" fn SetEndOfFile(file: Handle) -> Bool {
    let mut position = 0i64;
    if SetFilePointerEx(file, 0, &mut position, 1) == FALSE {
        return FALSE;
    }
    let mut iosb = [0usize; 2];
    // SAFETY: an 8-byte FILE_END_OF_FILE_INFORMATION.
    let status =
        unsafe { NtSetInformationFile(file, &mut iosb, (&raw const position).cast(), 8, 20) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

/// SetFileTime: not yet (reported).
#[unsafe(no_mangle)]
pub extern "system" fn SetFileTime(
    _file: Handle,
    _created: *const u64,
    _accessed: *const u64,
    _written: *const u64,
) -> Bool {
    unsupported(b"kernel32.dll!SetFileTime", 0);
    fail_with(ERROR_NOT_SUPPORTED, FALSE)
}
