//! The functions msvcp140.dll imports besides the thread pool
//! (threadpool.rs): CreateEventExW, symbolic and hard links, file copies.

use super::*;

/// CreateEventExW: CREATE_EVENT_MANUAL_RESET (1), CREATE_EVENT_INITIAL_SET
/// (2); the access mask is not narrowed.
#[unsafe(no_mangle)]
pub extern "system" fn CreateEventExW(
    attributes: *mut c_void,
    name: *const u16,
    flags: u32,
    _access: u32,
) -> Handle {
    super::sync::CreateEventW(
        attributes,
        Bool::from(flags & 1 != 0),
        Bool::from(flags & 2 != 0),
        name,
    )
}

/// CreateSymbolicLinkW: the link created (FILE_CREATE, the link itself:
/// FILE_OPEN_REPARSE_POINT; a directory with SYMBOLIC_LINK_FLAG_DIRECTORY)
/// and given an IO_REPARSE_TAG_SYMLINK reparse point, as Windows does: a
/// relative target is kept as given (SYMLINK_FLAG_RELATIVE), another made
/// an NT path (`\??\C:\...`), the print name the target as given.
/// SeCreateSymbolicLinkPrivilege is enabled for the call if the caller
/// holds it (an administrator, SYSTEM); without it ERROR_PRIVILEGE_NOT_HELD
/// (SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE needs developer mode,
/// which is off). A failed link leaves no file behind.
#[unsafe(no_mangle)]
pub extern "system" fn CreateSymbolicLinkW(link: *const u16, target: *const u16, flags: u32) -> u8 {
    const SYMBOLIC_LINK_FLAG_DIRECTORY: u32 = 1;
    const SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE: u32 = 2;
    const FSCTL_SET_REPARSE_POINT: u32 = 0x0009_00a4;
    const IO_REPARSE_TAG_SYMLINK: u32 = 0xa000_000c;
    const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
    const FILE_CREATE: u32 = 2;
    #[cfg_attr(
        target_arch = "x86",
        link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
    )]
    #[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
    unsafe extern "system" {
        fn NtFsControlFile(
            h: Handle,
            event: usize,
            apc: usize,
            context: usize,
            iosb: *mut [usize; 2],
            code: u32,
            input: *const u8,
            input_length: u32,
            output: *mut u8,
            output_length: u32,
        ) -> u32;
    }
    if link.is_null() || target.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, 0);
    }
    let known = SYMBOLIC_LINK_FLAG_DIRECTORY | SYMBOLIC_LINK_FLAG_ALLOW_UNPRIVILEGED_CREATE;
    if flags & !known != 0 {
        unsupported(b"kernel32.dll!CreateSymbolicLinkW(flags)", u64::from(flags));
    }
    let given = wide_slice(target);
    // RtlPathTypeRelative: neither rooted (`\`, `/`) nor with a drive.
    let relative = !given.first().is_some_and(|&c| is_sep(c))
        && !(given.len() >= 2 && given[1] == u16::from(b':'));
    let absolute;
    let substitute: &[u16] = if relative {
        given
    } else {
        absolute = match nt_path(target) {
            Ok(p) => p,
            Err(e) => return fail_with(e, 0),
        };
        absolute.as_slice()
    };
    // REPARSE_DATA_BUFFER: the header, SymbolicLinkReparseBuffer's four
    // name fields and Flags, then the substitute and print names (each
    // NUL-terminated).
    let (sub_bytes, print_bytes) = (substitute.len() * 2, given.len() * 2);
    let size = 20 + sub_bytes + 2 + print_bytes + 2;
    if size > 16 * 1024 {
        return fail_with(ERROR_FILENAME_EXCED_RANGE, 0);
    }
    let mut buf = [0u8; 16 * 1024];
    let put16 = |b: &mut [u8], at: usize, v: usize| {
        b[at..at + 2].copy_from_slice(&(v as u16).to_le_bytes())
    };
    buf[0..4].copy_from_slice(&IO_REPARSE_TAG_SYMLINK.to_le_bytes());
    put16(&mut buf, 4, size - 8);
    put16(&mut buf, 8, 0);
    put16(&mut buf, 10, sub_bytes);
    put16(&mut buf, 12, sub_bytes + 2);
    put16(&mut buf, 14, print_bytes);
    buf[16..20].copy_from_slice(&u32::from(relative).to_le_bytes());
    for (i, &c) in substitute.iter().enumerate() {
        put16(&mut buf, 20 + 2 * i, usize::from(c));
    }
    for (i, &c) in given.iter().enumerate() {
        put16(&mut buf, 20 + sub_bytes + 2 + 2 * i, usize::from(c));
    }
    let Some(privilege) = SymlinkPrivilege::acquire() else {
        return fail_with(ERROR_PRIVILEGE_NOT_HELD, 0);
    };
    let kind = if flags & SYMBOLIC_LINK_FLAG_DIRECTORY != 0 {
        FILE_DIRECTORY_FILE
    } else {
        FILE_NON_DIRECTORY_FILE
    };
    let options = kind | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT;
    let access = FILE_WRITE_ATTRIBUTES | DELETE | SYNCHRONIZE;
    let (h, _) = match open_path(link, access, 0, FILE_CREATE, options, 0) {
        Ok(o) => o,
        Err(e) => {
            drop(privilege);
            return fail_with(e, 0);
        }
    };
    let mut iosb = [0usize; 2];
    // SAFETY: our handle and buffer.
    let status = unsafe {
        NtFsControlFile(
            h,
            0,
            0,
            0,
            &mut iosb,
            FSCTL_SET_REPARSE_POINT,
            buf.as_ptr(),
            size as u32,
            core::ptr::null_mut(),
            0,
        )
    };
    drop(privilege);
    if status != STATUS_SUCCESS {
        // The file just made goes again.
        let _ = set_file(h, 13, &[1]);
        CloseHandle(h);
        return fail(status, 0);
    }
    CloseHandle(h);
    1
}

const ERROR_PRIVILEGE_NOT_HELD: u32 = 1314;
const ERROR_FILENAME_EXCED_RANGE: u32 = 206;

/// SeCreateSymbolicLinkPrivilege enabled in the process's token while
/// this lives (RtlAcquirePrivilege's part in CreateSymbolicLinkW); the
/// previous state put back when it goes.
struct SymlinkPrivilege {
    token: usize,
    previous: [u32; 4],
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn NtOpenProcessToken(process: isize, access: u32, token: *mut usize) -> u32;
    fn NtAdjustPrivilegesToken(
        token: usize,
        disable_all: u8,
        new: *const u32,
        length: u32,
        previous: *mut u32,
        ret: *mut u32,
    ) -> u32;
}

impl SymlinkPrivilege {
    /// None if the token does not hold the privilege.
    fn acquire() -> Option<SymlinkPrivilege> {
        let mut p = SymlinkPrivilege { token: 0, previous: [0; 4] };
        let mut ret = 0;
        // SAFETY: plain calls with our buffers (TOKEN_ADJUST_PRIVILEGES |
        // TOKEN_QUERY; SeCreateSymbolicLinkPrivilege is LUID 35, enabled).
        let status = unsafe {
            if NtOpenProcessToken(-1, 0x28, &mut p.token) != STATUS_SUCCESS {
                return None;
            }
            NtAdjustPrivilegesToken(
                p.token,
                0,
                [1, 35, 0, 2].as_ptr(),
                16,
                p.previous.as_mut_ptr(),
                &mut ret,
            )
        };
        // STATUS_NOT_ALL_ASSIGNED (0x106): not held.
        if status != STATUS_SUCCESS {
            p.previous = [0; 4];
            return None;
        }
        Some(p)
    }
}

impl Drop for SymlinkPrivilege {
    fn drop(&mut self) {
        // SAFETY: our token handle; the previous state (empty when the
        // privilege was already enabled).
        unsafe {
            if self.previous[0] != 0 {
                let mut ret = 0;
                NtAdjustPrivilegesToken(
                    self.token,
                    0,
                    self.previous.as_ptr(),
                    16,
                    core::ptr::null_mut(),
                    &mut ret,
                );
            }
        }
        if self.token != 0 {
            CloseHandle(self.token as Handle);
        }
    }
}

/// CreateHardLinkW(link, existing, security): `link` becomes another name
/// of file `existing` (FileLinkInformation; Wine's kernel32 test_hard_link
/// and ntdll test_file_link_information, `hardlink_test`). The security
/// attributes are reserved (documented: NULL).
#[unsafe(no_mangle)]
pub extern "system" fn CreateHardLinkW(
    link: *const u16,
    existing: *const u16,
    _security: *mut c_void,
) -> Bool {
    if link.is_null() || existing.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    super::mapping::hard_link(existing, link)
}

const ERROR_FILE_EXISTS: u32 = 80;
const COPY_FILE_FAIL_IF_EXISTS: u32 = 1;

/// CopyFileW: the contents and attributes; with `fail_if_exists`,
/// ERROR_FILE_EXISTS if the target exists.
#[unsafe(no_mangle)]
pub extern "system" fn CopyFileW(from: *const u16, to: *const u16, fail_if_exists: Bool) -> Bool {
    copy_file(from, to, fail_if_exists != FALSE)
}

/// CopyFileExW: CopyFileW (COPY_FILE_FAIL_IF_EXISTS; no progress
/// callbacks or cancellation).
#[unsafe(no_mangle)]
pub extern "system" fn CopyFileExW(
    from: *const u16,
    to: *const u16,
    _progress: *mut c_void,
    _data: *mut c_void,
    _cancel: *mut Bool,
    flags: u32,
) -> Bool {
    copy_file(from, to, flags & COPY_FILE_FAIL_IF_EXISTS != 0)
}

/// CopyFile2(pwszExistingFileName, pwszNewFileName, pExtendedParameters):
/// CopyFileExW with COPYFILE2_EXTENDED_PARAMETERS' flags
/// (COPY_FILE_FAIL_IF_EXISTS; no progress callback or cancellation), the
/// result as an HRESULT (HRESULT_FROM_WIN32 of the error).
#[unsafe(no_mangle)]
pub extern "system" fn CopyFile2(from: *const u16, to: *const u16, params: *const u8) -> i32 {
    // COPYFILE2_EXTENDED_PARAMETERS { dwSize, dwCopyFlags, ... }.
    let flags = if params.is_null() {
        0
    } else {
        // SAFETY: the caller's structure, at least its first two DWORDs.
        unsafe { params.add(4).cast::<u32>().read_unaligned() }
    };
    if copy_file(from, to, flags & COPY_FILE_FAIL_IF_EXISTS != 0) != FALSE {
        return 0;
    }
    ((GetLastError() & 0xffff) | 0x8007_0000) as i32
}

fn copy_file(from: *const u16, to: *const u16, fail_if_exists: bool) -> Bool {
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    let src =
        CreateFileW(from, GENERIC_READ, 1, core::ptr::null_mut(), 3, 0, core::ptr::null_mut());
    if src == INVALID_HANDLE_VALUE {
        return FALSE;
    }
    let attributes = GetFileAttributesW(from);
    let disposition = if fail_if_exists { 1 } else { 2 }; // CREATE_NEW / CREATE_ALWAYS
    let dst = CreateFileW(
        to,
        GENERIC_WRITE,
        0,
        core::ptr::null_mut(),
        disposition,
        0,
        core::ptr::null_mut(),
    );
    if dst == INVALID_HANDLE_VALUE {
        let e = GetLastError();
        CloseHandle(src);
        return fail_with(if e == ERROR_ALREADY_EXISTS { ERROR_FILE_EXISTS } else { e }, FALSE);
    }
    let mut buf = [0u8; 4096];
    let ok = loop {
        let (mut n, mut written) = (0u32, 0u32);
        if ReadFile(src, buf.as_mut_ptr(), buf.len() as u32, &mut n, core::ptr::null_mut()) == FALSE
        {
            break false;
        }
        if n == 0 {
            break true;
        }
        if WriteFile(dst, buf.as_ptr(), n, &mut written, core::ptr::null_mut()) == FALSE
            || written != n
        {
            break false;
        }
    };
    CloseHandle(src);
    CloseHandle(dst);
    if !ok {
        return FALSE;
    }
    if attributes != u32::MAX {
        super::sysmem::SetFileAttributesW(to, attributes);
    }
    TRUE
}
