//! CreateProcessW/A on gala's process creation service (GalaCreateProcess,
//! docs/ARCHITECTURE.md "Processes started by programs"), and
//! GetModuleFileNameW.

use super::sync::ansi_to_wide;
use super::*;

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GalaCreateProcess(parameters: *mut u8) -> u32;
}

const CREATE_SUSPENDED: u32 = 0x4;
const DETACHED_PROCESS: u32 = 0x8;
const CREATE_NEW_CONSOLE: u32 = 0x10;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
const CREATE_UNICODE_ENVIRONMENT: u32 = 0x400;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// CREATE_DEFAULT_ERROR_MODE: the child starts with the default error
/// mode instead of the caller's (gala's processes always do: the error
/// mode is not inherited yet). NSIS uninstallers pass it.
const CREATE_DEFAULT_ERROR_MODE: u32 = 0x0400_0000;
/// Flags that change nothing on gala (one console, no windows).
const IGNORED_FLAGS: u32 = DETACHED_PROCESS
    | CREATE_NEW_CONSOLE
    | CREATE_NEW_PROCESS_GROUP
    | CREATE_NO_WINDOW
    | CREATE_DEFAULT_ERROR_MODE;
const STARTF_USESTDHANDLES: u32 = 0x100;

/// STARTUPINFOW (and STARTUPINFOA, the same layout) followed by
/// STARTUPINFOEXW's lpAttributeList.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct StartupInfoEx {
    pub cb: u32,
    reserved: usize,
    desktop: usize,
    title: usize,
    position_and_size: [u32; 7],
    pub flags: u32,
    show_window: u16,
    reserved2_len: u16,
    reserved2: usize,
    /// hStdInput, hStdOutput, hStdError.
    pub std: [usize; 3],
    pub attribute_list: usize,
}

/// sizeof(STARTUPINFOW): 104 on x64, 68 on x86.
const STARTUPINFO_SIZE: usize = core::mem::offset_of!(StartupInfoEx, attribute_list);

/// The most handles a 32-bit caller's PROC_THREAD_ATTRIBUTE_HANDLE_LIST
/// may name (widened for GalaCreateProcess on the stack).
const MAX_HANDLE_LIST: usize = 64;

/// STARTUPINFO.dwFlags.
///
/// # Safety
/// `startup` is the caller's STARTUPINFO.
unsafe fn startup_flags(startup: *const u8) -> u32 {
    // SAFETY: as the caller promises.
    unsafe { (*startup.cast::<StartupInfoEx>()).flags }
}
const EXTENDED_STARTUPINFO_PRESENT: u32 = 0x0008_0000;
const PROC_THREAD_ATTRIBUTE_HANDLE_LIST: usize = 0x2_0002;
const PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY: usize = 0x2_0007;
/// An HPCON (CreatePseudoConsole): the child is attached to it.
const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x2_0016;
/// GROUP_AFFINITY (16 bytes), for threads.
const PROC_THREAD_ATTRIBUTE_GROUP_AFFINITY: usize = 0x3_0003;
/// PROCESSOR_NUMBER (4 bytes), for threads.
const PROC_THREAD_ATTRIBUTE_IDEAL_PROCESSOR: usize = 0x3_0005;
/// Attributes a list holds at most (enough for every documented one).
const MAX_ATTRIBUTES: u32 = 16;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_FILE_NOT_FOUND: u32 = 2;

/// The program's path from the application name or, without one, the
/// first token of the command line (quoted or up to a space), with `.exe`
/// appended if it has no extension.
fn program_token(
    application: *const u16,
    command_line: *const u16,
    out: &mut PathBuf,
) -> Result<(), u32> {
    if !application.is_null() {
        return out.push(wide_slice(application));
    }
    if command_line.is_null() {
        return Err(ERROR_INVALID_PARAMETER);
    }
    let line = wide_slice(command_line);
    let token = if line.first() == Some(&u16::from(b'"')) {
        let end =
            line[1..].iter().position(|&c| c == u16::from(b'"')).map_or(line.len(), |i| i + 1);
        &line[1..end]
    } else {
        let end = line
            .iter()
            .position(|&c| c == u16::from(b' ') || c == u16::from(b'\t'))
            .unwrap_or(line.len());
        &line[..end]
    };
    out.push(token)?;
    let name_start = token.iter().rposition(|&c| is_sep(c)).map_or(0, |i| i + 1);
    if !token[name_start..].contains(&u16::from(b'.')) {
        out.push(&[u16::from(b'.'), u16::from(b'e'), u16::from(b'x'), u16::from(b'e')])?;
    }
    Ok(())
}

/// Whether a file exists at `path` (NUL-terminated).
fn exists(path: &PathBuf) -> bool {
    let mut z = PathBuf::new();
    if z.push(path.as_slice()).is_err() || z.push(&[0]).is_err() {
        return false;
    }
    GetFileAttributesW(z.chars.as_ptr()) != u32::MAX
}

/// Where Windows looks for a program named without a directory: the
/// directory of the executable, the current directory, System32, Windows,
/// then the directories of the PATH variable in order (CreateProcess's
/// documented search; `gala install`'s Links directory is found so).
fn search(name: &PathBuf) -> Result<PathBuf, u32> {
    let mut exe = [0u16; MAX_PATH_CHARS];
    let n =
        GetModuleFileNameW(core::ptr::null_mut(), exe.as_mut_ptr(), MAX_PATH_CHARS as u32) as usize;
    let exe_dir = exe[..n].iter().rposition(|&c| is_sep(c)).map_or(0, |i| i + 1);
    let mut cwd = [0u16; MAX_PATH_CHARS];
    let c = GetCurrentDirectoryW(MAX_PATH_CHARS as u32, cwd.as_mut_ptr()) as usize;
    let system32: [u16; 20] = wide20("C:\\Windows\\System32\\");
    let windows: [u16; 20] = wide20("C:\\Windows\\");
    let dirs: [&[u16]; 4] =
        [&exe[..exe_dir], &cwd[..c.min(MAX_PATH_CHARS)], &system32[..20], &windows[..11]];
    for dir in dirs {
        let mut candidate = PathBuf::new();
        candidate.push(dir)?;
        if !dir.last().is_some_and(|&ch| is_sep(ch)) {
            candidate.push(&[BACKSLASH])?;
        }
        candidate.push(name.as_slice())?;
        if exists(&candidate) {
            return Ok(candidate);
        }
    }
    let mut path = [0u16; 4096];
    let n = GetEnvironmentVariableW(wide20("PATH").as_ptr(), path.as_mut_ptr(), 4096) as usize;
    if n == 0 || n >= path.len() {
        return Err(ERROR_FILE_NOT_FOUND);
    }
    for dir in path[..n].split(|&c| c == u16::from(b';')) {
        // Quotes around an entry are allowed (as cmd.exe writes them).
        let dir: &[u16] = match dir {
            [q, inner @ .., e] if *q == u16::from(b'"') && *e == u16::from(b'"') => inner,
            _ => dir,
        };
        if dir.is_empty() {
            continue;
        }
        let mut candidate = PathBuf::new();
        if candidate.push(dir).is_err()
            || (!dir.last().is_some_and(|&ch| is_sep(ch)) && candidate.push(&[BACKSLASH]).is_err())
            || candidate.push(name.as_slice()).is_err()
        {
            continue;
        }
        if exists(&candidate) {
            return Ok(candidate);
        }
    }
    Err(ERROR_FILE_NOT_FOUND)
}

/// Up to 20 ASCII characters as UTF-16 (zero-padded).
fn wide20(s: &str) -> [u16; 20] {
    let mut out = [0u16; 20];
    for (o, b) in out.iter_mut().zip(s.bytes()) {
        *o = u16::from(b);
    }
    out
}

/// The length in characters of an environment block, through its final
/// empty string.
fn block_len(block: *const u16) -> usize {
    let mut n = 0;
    // SAFETY: an environment block ends with two NULs.
    unsafe {
        loop {
            if *block.add(n) == 0 && (n == 0 || *block.add(n - 1) == 0) {
                return n + 1;
            }
            n += 1;
        }
    }
}

/// An ANSI (UTF-8 here) environment block as UTF-16, in memory that
/// FreeEnvironmentStringsW frees; None if out of memory.
fn ansi_block_to_wide(block: *const u8) -> Option<*mut u16> {
    let mut n = 0;
    // SAFETY: an environment block ends with two NULs (or is one NUL
    // pair when empty).
    unsafe {
        while !(*block.add(n) == 0 && (n == 0 || *block.add(n - 1) == 0)) {
            n += 1;
        }
    }
    let n = n + 1; // bytes, with the final NUL
    // UTF-8 never needs more UTF-16 units than bytes, plus one NUL pair
    // for an empty block.
    let units = n + 1;
    let w = VirtualAlloc(core::ptr::null_mut(), units * 2, 0x3000, 4).cast::<u16>();
    if w.is_null() {
        return None;
    }
    let got = super::text::MultiByteToWideChar(65001, 0, block, n as i32, w, units as i32);
    if got <= 0 {
        FreeEnvironmentStringsW(w);
        return None;
    }
    // SAFETY: inside the zeroed allocation of `units` units: an empty
    // block ("\0") gets its second NUL.
    unsafe { *w.add(got as usize) = 0 };
    Some(w)
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateProcessW(
    application: *const u16,
    command_line: *mut u16,
    process_attributes: *mut c_void,
    thread_attributes: *mut c_void,
    inherit: Bool,
    flags: u32,
    environment: *mut c_void,
    directory: *const u16,
    startup: *const u8,
    info: *mut usize,
) -> Bool {
    create_process(
        0,
        application,
        command_line,
        process_attributes,
        thread_attributes,
        inherit,
        flags,
        environment,
        directory,
        startup,
        info,
    )
}

/// CreateProcessAsUserW (kernel32 exports it, as Windows' does;
/// advapi32's forwards here): CreateProcessW with `token` as the new
/// process's primary token. The kernel checks it (a primary token the
/// caller may assign: ERROR_BAD_TOKEN_TYPE, ERROR_PRIVILEGE_NOT_HELD).
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn CreateProcessAsUserW(
    token: Handle,
    application: *const u16,
    command_line: *mut u16,
    process_attributes: *mut c_void,
    thread_attributes: *mut c_void,
    inherit: Bool,
    flags: u32,
    environment: *mut c_void,
    directory: *const u16,
    startup: *const u8,
    info: *mut usize,
) -> Bool {
    if token.is_null() {
        return fail_with(ERROR_INVALID_HANDLE, FALSE);
    }
    // SeAssignPrimaryTokenPrivilege enabled for the call (a service holds
    // it disabled), then restored.
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
    let mut own = 0usize;
    let mut previous = [0u32; 4];
    let mut ret = 0;
    // SAFETY: plain calls with our buffers (TOKEN_ADJUST_PRIVILEGES |
    // TOKEN_QUERY; SeAssignPrimaryTokenPrivilege is LUID 3).
    let adjusted = unsafe {
        NtOpenProcessToken(-1, 0x28, &mut own) == 0
            && NtAdjustPrivilegesToken(
                own,
                0,
                [1, 3, 0, 2].as_ptr(),
                16,
                previous.as_mut_ptr(),
                &mut ret,
            ) == 0
    };
    let r = create_process(
        token as usize,
        application,
        command_line,
        process_attributes,
        thread_attributes,
        inherit,
        flags,
        environment,
        directory,
        startup,
        info,
    );
    if own != 0 {
        // SAFETY: our token handle; the previous state (empty when the
        // privilege was already enabled).
        unsafe {
            if adjusted && previous[0] != 0 {
                NtAdjustPrivilegesToken(
                    own,
                    0,
                    previous.as_ptr(),
                    0,
                    core::ptr::null_mut(),
                    &mut ret,
                );
            }
            NtClose(own as Handle);
        }
    }
    r
}

/// CreateProcessW and CreateProcessAsUserW (`token` 0: the caller's).
#[allow(clippy::too_many_arguments)]
fn create_process(
    token: usize,
    application: *const u16,
    command_line: *mut u16,
    process_attributes: *mut c_void,
    thread_attributes: *mut c_void,
    inherit: Bool,
    flags: u32,
    environment: *mut c_void,
    directory: *const u16,
    startup: *const u8,
    info: *mut usize,
) -> Bool {
    if !process_attributes.is_null() || !thread_attributes.is_null() {
        unsupported(b"kernel32.dll!CreateProcessW(security attributes)", 0);
    }
    let unknown = flags
        & !(CREATE_SUSPENDED
            | CREATE_UNICODE_ENVIRONMENT
            | EXTENDED_STARTUPINFO_PRESENT
            | IGNORED_FLAGS);
    if unknown != 0 {
        unsupported(b"kernel32.dll!CreateProcessW(flags)", unknown.into());
    }
    // The image: as given (with a directory), or searched for.
    let mut program_name = PathBuf::new();
    if let Err(e) = program_token(application, command_line, &mut program_name) {
        return fail_with(e, FALSE);
    }
    let found = if program_name.as_slice().iter().any(|&c| is_sep(c) || c == u16::from(b':')) {
        Ok(program_name)
    } else {
        search(&program_name)
    };
    let program = match found {
        Ok(p) => p,
        Err(e) => return fail_with(e, FALSE),
    };
    let mut program_z = PathBuf::new();
    if program_z.push(program.as_slice()).is_err() || program_z.push(&[0]).is_err() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let image = match nt_path(program_z.chars.as_ptr()) {
        Ok(p) => p,
        Err(e) => return fail_with(e, FALSE),
    };
    // The command line (the application name if there is none).
    let line: &[u16] =
        if command_line.is_null() { wide_slice(application) } else { wide_slice(command_line) };
    // The current directory, with a trailing backslash.
    let mut cwd = PathBuf::new();
    let pushed = if directory.is_null() {
        let mut buf = [0u16; MAX_PATH_CHARS];
        let n = GetCurrentDirectoryW(MAX_PATH_CHARS as u32, buf.as_mut_ptr()) as usize;
        cwd.push(&buf[..n.min(MAX_PATH_CHARS)])
    } else {
        full_path(wide_slice(directory)).and_then(|p| cwd.push(p.as_slice()))
    };
    if let Err(e) = pushed {
        return fail_with(e, FALSE);
    }
    if !cwd.as_slice().last().is_some_and(|&c| is_sep(c)) && cwd.push(&[BACKSLASH]).is_err() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // The environment: the given UTF-16 block, or ours.
    let (env, own_env) = if environment.is_null() {
        (GetEnvironmentStringsW(), true)
    } else if flags & CREATE_UNICODE_ENVIRONMENT != 0 {
        (environment.cast::<u16>(), false)
    } else {
        match ansi_block_to_wide(environment.cast::<u8>()) {
            Some(w) => (w, true),
            None => return fail_with(ERROR_NOT_ENOUGH_MEMORY, FALSE),
        }
    };
    // Standard handles: from STARTUPINFO if it says so, else ours.
    // SAFETY: the caller's STARTUPINFOW.
    let std = unsafe {
        if !startup.is_null() && startup_flags(startup) & STARTF_USESTDHANDLES != 0 {
            (*startup.cast::<StartupInfoEx>()).std
        } else {
            [
                GetStdHandle(-10i32 as u32) as usize,
                GetStdHandle(-11i32 as u32) as usize,
                GetStdHandle(-12i32 as u32) as usize,
            ]
        }
    };
    // STARTUPINFOEXW: lpAttributeList after the STARTUPINFOW.
    let list = if flags & EXTENDED_STARTUPINFO_PRESENT != 0 && !startup.is_null() {
        // SAFETY: the caller's STARTUPINFOEXW.
        unsafe { (*startup.cast::<StartupInfoEx>()).attribute_list as *const AttributeList }
    } else {
        core::ptr::null()
    };
    let handle_list = attribute(list, PROC_THREAD_ATTRIBUTE_HANDLE_LIST);
    // The pseudo console's session (the HPCON's first field).
    let pseudo_console = attribute(list, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE)
        // SAFETY: an HPCON from CreatePseudoConsole (a PseudoConsole).
        .map(|(value, _)| unsafe { *(value as *const u32) });
    // SAFETY: the caller's STARTUPINFOW.
    let own_std =
        !startup.is_null() && unsafe { startup_flags(startup) } & STARTF_USESTDHANDLES != 0;
    let mut p = [0u8; 184];
    let mut put = |at: usize, v: u64| p[at..at + 8].copy_from_slice(&v.to_le_bytes());
    put(0, image.chars.as_ptr() as u64);
    put(16, line.as_ptr() as u64);
    put(32, cwd.chars.as_ptr() as u64);
    put(48, env as u64);
    // Handles sign-extended (a 32-bit caller's INVALID_HANDLE_VALUE).
    put(64, std[0] as isize as u64);
    put(72, std[1] as isize as u64);
    put(80, std[2] as isize as u64);
    let lens = [(8, image.len), (24, line.len()), (40, cwd.len), (56, block_len(env))];
    for (at, len) in lens {
        p[at..at + 4].copy_from_slice(&(len as u32).to_le_bytes());
    }
    let mut bits = 0u32;
    if inherit != 0 {
        bits |= 1;
    }
    if flags & CREATE_SUSPENDED != 0 {
        bits |= 2;
    }
    // STARTUPINFO cbReserved2 / lpReserved2: the C runtime's inherited
    // descriptors, for the child's RuntimeData.
    // SAFETY: the caller's STARTUPINFO.
    let (reserved_len, reserved) = if startup.is_null() {
        (0, 0)
    } else {
        let s = unsafe { &*startup.cast::<StartupInfoEx>() };
        (s.reserved2_len, s.reserved2 as u64)
    };
    if reserved_len != 0 && reserved != 0 {
        bits |= 8;
        p[136..144].copy_from_slice(&reserved.to_le_bytes());
        p[144..148].copy_from_slice(&u32::from(reserved_len).to_le_bytes());
    }
    // The block's handle list holds 64-bit handles: a 32-bit caller's are
    // widened (sign-extended) into `wide`.
    let mut wide = [0u64; MAX_HANDLE_LIST];
    if let Some((handles, size)) = handle_list {
        let count = size / size_of::<usize>();
        let handles = if size_of::<usize>() == 8 {
            handles as u64
        } else if count <= MAX_HANDLE_LIST {
            // SAFETY: the attribute's array of `count` handles.
            let list = unsafe { core::slice::from_raw_parts(handles as *const usize, count) };
            for (w, &h) in wide.iter_mut().zip(list) {
                *w = h as isize as u64;
            }
            wide.as_ptr() as u64
        } else {
            unsupported(b"kernel32.dll!CreateProcess(handle list)", count as u64);
            return fail_with(ERROR_NOT_SUPPORTED, FALSE);
        };
        bits |= 4;
        p[120..128].copy_from_slice(&handles.to_le_bytes());
        p[128..132].copy_from_slice(&(count as u32).to_le_bytes());
    }
    if token != 0 {
        bits |= 16;
        p[152..160].copy_from_slice(&(token as isize as u64).to_le_bytes());
    }
    // Flag 32: attached to a pseudo console; 64: with its own standard
    // handles (the documented default without STARTF_USESTDHANDLES).
    if let Some(session) = pseudo_console {
        bits |= 32;
        if !own_std {
            bits |= 64;
        }
        p[160..164].copy_from_slice(&session.to_le_bytes());
    } else if flags & DETACHED_PROCESS != 0 {
        // Flag 128: no console at all.
        bits |= 128;
    }
    // Flag 256: STARTUPINFO.lpDesktop, the window station and desktop the
    // child connects to (else it inherits the caller's).
    // SAFETY: the caller's STARTUPINFOW.
    let desktop = if startup.is_null() {
        core::ptr::null()
    } else {
        unsafe { (*startup.cast::<StartupInfoEx>()).desktop as *const u16 }
    };
    if !desktop.is_null() {
        bits |= 256;
        let name = wide_slice(desktop);
        p[168..176].copy_from_slice(&(name.as_ptr() as u64).to_le_bytes());
        p[176..180].copy_from_slice(&(name.len() as u32).to_le_bytes());
    }
    p[12..16].copy_from_slice(&bits.to_le_bytes());
    // SAFETY: the 184-byte parameter block; its pointers live through
    // the call.
    let status = unsafe { GalaCreateProcess(p.as_mut_ptr()) };
    if own_env {
        FreeEnvironmentStringsW(env);
    }
    if status != STATUS_SUCCESS {
        return fail(status, FALSE);
    }
    // PROCESS_INFORMATION {hProcess, hThread, dwProcessId, dwThreadId}.
    let read = |at: usize| u64::from_le_bytes(p[at..at + 8].try_into().unwrap_or([0; 8]));
    if !info.is_null() {
        // SAFETY: the caller's PROCESS_INFORMATION.
        unsafe {
            *info = read(88) as usize;
            *info.add(1) = read(96) as usize;
            let ids = info.add(2).cast::<u32>();
            *ids = read(104) as u32;
            *ids.add(1) = read(112) as u32;
        }
    }
    TRUE
}

/// CreateProcessA: the strings converted to UTF-16 (STARTUPINFOA has the
/// same layout where it matters: dwFlags and the handles).
#[unsafe(no_mangle)]
pub extern "system" fn CreateProcessA(
    application: *const u8,
    command_line: *mut u8,
    process_attributes: *mut c_void,
    thread_attributes: *mut c_void,
    inherit: Bool,
    flags: u32,
    environment: *mut c_void,
    directory: *const u8,
    startup: *const u8,
    info: *mut usize,
) -> Bool {
    create_process_a(
        None,
        application,
        command_line,
        process_attributes,
        thread_attributes,
        inherit,
        flags,
        environment,
        directory,
        startup,
        info,
    )
}

/// CreateProcessAsUserA: CreateProcessAsUserW with the strings converted.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn CreateProcessAsUserA(
    token: Handle,
    application: *const u8,
    command_line: *mut u8,
    process_attributes: *mut c_void,
    thread_attributes: *mut c_void,
    inherit: Bool,
    flags: u32,
    environment: *mut c_void,
    directory: *const u8,
    startup: *const u8,
    info: *mut usize,
) -> Bool {
    create_process_a(
        Some(token),
        application,
        command_line,
        process_attributes,
        thread_attributes,
        inherit,
        flags,
        environment,
        directory,
        startup,
        info,
    )
}

/// The ANSI process creation functions: the strings as UTF-16, then
/// CreateProcessW or (with a token) CreateProcessAsUserW. STARTUPINFOA has
/// STARTUPINFOW's layout where it matters (dwFlags and the handles).
#[allow(clippy::too_many_arguments)]
fn create_process_a(
    token: Option<Handle>,
    application: *const u8,
    command_line: *mut u8,
    process_attributes: *mut c_void,
    thread_attributes: *mut c_void,
    inherit: Bool,
    flags: u32,
    environment: *mut c_void,
    directory: *const u8,
    startup: *const u8,
    info: *mut usize,
) -> Bool {
    let (mut app, mut line, mut dir) = ([0u16; 512], [0u16; 2048], [0u16; 512]);
    let convert = |s: *const u8, buf: &mut [u16]| -> Result<*const u16, u32> {
        if s.is_null() {
            return Ok(core::ptr::null());
        }
        ansi_to_wide(s, buf).ok_or(ERROR_INVALID_PARAMETER)?;
        Ok(buf.as_ptr())
    };
    let app_w = match convert(application, &mut app) {
        Ok(p) => p,
        Err(e) => return fail_with(e, FALSE),
    };
    let line_w = match convert(command_line, &mut line) {
        Ok(p) => p,
        Err(e) => return fail_with(e, FALSE),
    };
    let dir_w = match convert(directory, &mut dir) {
        Ok(p) => p,
        Err(e) => return fail_with(e, FALSE),
    };
    // STARTUPINFOA's lpDesktop as UTF-16, in a copy of the structure
    // (STARTUPINFOEXA's, with its attribute list).
    let mut desktop = [0u16; 260];
    // SAFETY: an all-zero STARTUPINFOEX is a valid bit pattern.
    let mut startup_w: StartupInfoEx = unsafe { core::mem::zeroed() };
    let startup = if startup.is_null() {
        startup
    } else {
        let size = if flags & EXTENDED_STARTUPINFO_PRESENT != 0 {
            size_of::<StartupInfoEx>()
        } else {
            STARTUPINFO_SIZE
        };
        // SAFETY: the caller's STARTUPINFO(EX)A of `size` bytes; our copy.
        unsafe {
            core::ptr::copy_nonoverlapping(startup, (&raw mut startup_w).cast::<u8>(), size);
        }
        let name = startup_w.desktop as *const u8;
        match convert(name, &mut desktop) {
            Ok(p) => startup_w.desktop = p as usize,
            Err(e) => return fail_with(e, FALSE),
        }
        (&raw const startup_w).cast::<u8>()
    };
    match token {
        Some(token) => CreateProcessAsUserW(
            token,
            app_w,
            line_w.cast_mut(),
            process_attributes,
            thread_attributes,
            inherit,
            flags,
            environment,
            dir_w,
            startup,
            info,
        ),
        None => CreateProcessW(
            app_w,
            line_w.cast_mut(),
            process_attributes,
            thread_attributes,
            inherit,
            flags,
            environment,
            dir_w,
            startup,
            info,
        ),
    }
}

/// GetModuleFileNameW: the executable's path (other modules: reported).
#[unsafe(no_mangle)]
pub extern "system" fn GetModuleFileNameW(module: Handle, buf: *mut u16, size: u32) -> u32 {
    let (path, len) = match module_path(module) {
        Ok(p) => p,
        Err(e) => return fail_with(e, 0),
    };
    let path = &path[..len];
    if size == 0 {
        return fail_with(122, 0);
    }
    let n = path.len().min(size as usize - 1);
    // SAFETY: the caller's buffer of `size` characters.
    unsafe {
        core::ptr::copy_nonoverlapping(path.as_ptr(), buf, n);
        *buf.add(n) = 0;
    }
    if n < path.len() {
        // Truncated, as documented: the buffer size and this error.
        return fail_with(122, size);
    }
    n as u32
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn LdrGetDllFullName(module: Handle, name: *mut UnicodeString) -> u32;
    fn LdrSetDllDirectory(dir: *const UnicodeString) -> u32;
    fn LdrGetDllDirectory(dir: *mut UnicodeString) -> u32;
}

/// SetDllDirectoryW: a directory searched for DLLs after the executable's
/// (null or empty: the default search again).
#[unsafe(no_mangle)]
pub extern "system" fn SetDllDirectoryW(dir: *const u16) -> Bool {
    let mut n = 0;
    if !dir.is_null() {
        // SAFETY: the caller's NUL-terminated string, read up to its NUL.
        while unsafe { *dir.add(n) } != 0 {
            n += 1;
        }
    }
    if n >= 32767 {
        return fail_with(206, 0); // ERROR_FILENAME_EXCED_RANGE
    }
    let us = UnicodeString { length: (2 * n) as u16, maximum_length: (2 * n) as u16, buffer: dir };
    // SAFETY: a UNICODE_STRING over the caller's string.
    match unsafe { LdrSetDllDirectory(&us) } {
        STATUS_SUCCESS => 1,
        _ => fail_with(206, 0),
    }
}

/// GetDllDirectoryW: SetDllDirectoryW's directory (characters copied, no
/// NUL counted; 0 and an empty string if none); a buffer too small gets
/// nothing and the return value is the size needed, NUL included.
#[unsafe(no_mangle)]
pub extern "system" fn GetDllDirectoryW(size: u32, buffer: *mut u16) -> u32 {
    let mut local = [0u16; 261];
    let mut us = UnicodeString {
        length: 0,
        maximum_length: (2 * local.len()) as u16,
        // ntdll writes the directory here.
        buffer: local.as_mut_ptr().cast_const(),
    };
    // SAFETY: a UNICODE_STRING over our buffer.
    if unsafe { LdrGetDllDirectory(&mut us) } != STATUS_SUCCESS {
        return 0;
    }
    let n = usize::from(us.length) / 2;
    if n + 1 > size as usize || buffer.is_null() {
        return (n + 1) as u32;
    }
    // SAFETY: the caller's buffer of `size` characters (n + 1 fit).
    unsafe {
        core::ptr::copy_nonoverlapping(local.as_ptr(), buffer, n);
        buffer.add(n).write(0);
    }
    n as u32
}

/// The file a module was loaded from: the executable's ImagePathName for
/// null or the executable, else the loader's record (LdrGetDllFullName);
/// ERROR_MOD_NOT_FOUND for an unknown handle.
pub(crate) fn module_path(module: Handle) -> Result<([u16; 520], usize), u32> {
    let mut out = [0u16; 520];
    let image = mapping::image_base();
    if module.is_null() || module == image {
        // RTL_USER_PROCESS_PARAMETERS.ImagePathName (a UNICODE_STRING).
        // SAFETY: the PEB's process parameters are always set.
        let path = unsafe {
            let params = process_parameters() + PARAMS_IMAGE_PATH;
            let len = usize::from(*(params as *const u16)) / 2;
            core::slice::from_raw_parts(*((params + US_BUFFER) as *const *const u16), len)
        };
        let n = path.len().min(out.len());
        out[..n].copy_from_slice(&path[..n]);
        return Ok((out, n));
    }
    // ntdll writes the path here.
    let mut us = UnicodeString {
        length: 0,
        maximum_length: (2 * out.len()) as u16,
        buffer: out.as_mut_ptr().cast_const(),
    };
    // SAFETY: a UNICODE_STRING over our buffer.
    let status = unsafe { LdrGetDllFullName(module, &mut us) };
    if status != STATUS_SUCCESS {
        return Err(126); // ERROR_MOD_NOT_FOUND
    }
    Ok((out, usize::from(us.length) / 2))
}

/// gala's PROC_THREAD_ATTRIBUTE_LIST (opaque to programs, which only get
/// its size): a count and the attributes as (attribute, value, size).
#[repr(C)]
pub struct AttributeList {
    capacity: u32,
    count: u32,
    entries: [(usize, usize, usize); MAX_ATTRIBUTES as usize],
}

/// The (value, size) of `attribute` in `list`, if it is there.
fn attribute(list: *const AttributeList, attribute: usize) -> Option<(usize, usize)> {
    if list.is_null() {
        return None;
    }
    // SAFETY: a list initialised by InitializeProcThreadAttributeList.
    let list = unsafe { &*list };
    list.entries[..list.count.min(list.capacity) as usize]
        .iter()
        .find(|e| e.0 == attribute)
        .map(|e| (e.1, e.2))
}

/// InitializeProcThreadAttributeList(lpAttributeList, dwAttributeCount,
/// dwFlags, *lpSize): with a null list, the size it needs
/// (ERROR_INSUFFICIENT_BUFFER).
#[unsafe(no_mangle)]
pub extern "system" fn InitializeProcThreadAttributeList(
    list: *mut AttributeList,
    count: u32,
    flags: u32,
    size: *mut usize,
) -> Bool {
    if flags != 0 || count > MAX_ATTRIBUTES || size.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let needed = core::mem::size_of::<AttributeList>();
    // SAFETY: the caller's size and list (of *size bytes).
    unsafe {
        if list.is_null() || *size < needed {
            *size = needed;
            return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
        }
        list.write(AttributeList { capacity: count, count: 0, entries: [(0, 0, 0); 16] });
    }
    TRUE
}

/// UpdateProcThreadAttribute(lpAttributeList, dwFlags, Attribute, lpValue,
/// cbSize, lpPreviousValue, lpReturnSize): the value is kept by pointer,
/// as on Windows (it must live until CreateProcess).
#[unsafe(no_mangle)]
pub extern "system" fn UpdateProcThreadAttribute(
    list: *mut AttributeList,
    flags: u32,
    attribute: usize,
    value: usize,
    size: usize,
    _previous: *mut c_void,
    _return_size: *mut usize,
) -> Bool {
    if list.is_null() || flags != 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    if attribute == PROC_THREAD_ATTRIBUTE_HANDLE_LIST {
        if size == 0 || !size.is_multiple_of(8) {
            return fail_with(ERROR_INVALID_PARAMETER, FALSE);
        }
    } else if attribute == PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE {
        // The value is the HPCON itself.
        if size != 8 || value == 0 {
            return fail_with(ERROR_INVALID_PARAMETER, FALSE);
        }
    } else if attribute == PROC_THREAD_ATTRIBUTE_MITIGATION_POLICY {
        // A DWORD64, or two (the second word: the newer policies). Kept
        // in the list; the child is not given them (mitigation policies
        // are kept, not enforced, on gala: ROADMAP BR stage 3), recorded.
        if size != 8 && size != 16 {
            return fail_with(24, FALSE); // ERROR_BAD_LENGTH
        }
        unsupported(b"kernel32.dll!UpdateProcThreadAttribute(mitigation policy, not applied)", 0);
    } else if attribute == PROC_THREAD_ATTRIBUTE_IDEAL_PROCESSOR
        || attribute == PROC_THREAD_ATTRIBUTE_GROUP_AFFINITY
    {
        let want = if attribute == PROC_THREAD_ATTRIBUTE_IDEAL_PROCESSOR { 4 } else { 16 };
        if size != want {
            return fail_with(24, FALSE); // ERROR_BAD_LENGTH
        }
    } else {
        // Recorded so the ledger shows which attributes programs use.
        unsupported(b"kernel32.dll!UpdateProcThreadAttribute(attribute)", attribute as u64);
        return fail_with(ERROR_NOT_SUPPORTED, FALSE);
    }
    // SAFETY: a list initialised by InitializeProcThreadAttributeList.
    let list = unsafe { &mut *list };
    if list.count >= list.capacity {
        return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
    }
    list.entries[list.count as usize] = (attribute, value, size);
    list.count += 1;
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn DeleteProcThreadAttributeList(_list: *mut AttributeList) {}

/// CreateRemoteThreadEx: a thread in this process (as CreateThread), with
/// the attribute list's ideal processor (a hint) and group affinity
/// applied before it runs. Threads in other processes are not supported
/// (recorded).
#[allow(clippy::too_many_arguments)]
#[unsafe(no_mangle)]
pub extern "system" fn CreateRemoteThreadEx(
    process: Handle,
    security: *mut c_void,
    stack_size: usize,
    start: extern "system" fn(*mut c_void) -> u32,
    param: *mut c_void,
    flags: u32,
    list: *const AttributeList,
    thread_id: *mut u32,
) -> Handle {
    const CREATE_SUSPENDED: u32 = 0x4;
    if !process.is_null()
        && process != super::GetCurrentProcess()
        && GetProcessId(process) != super::GetCurrentProcessId()
    {
        unsupported(b"kernel32.dll!CreateRemoteThreadEx(another process)", 0);
        return fail_with(ERROR_NOT_SUPPORTED, core::ptr::null_mut());
    }
    if !list.is_null() {
        // SAFETY: a list initialised by InitializeProcThreadAttributeList.
        let l = unsafe { &*list };
        let known = [PROC_THREAD_ATTRIBUTE_IDEAL_PROCESSOR, PROC_THREAD_ATTRIBUTE_GROUP_AFFINITY];
        if let Some(&(other, ..)) =
            l.entries[..l.count.min(l.capacity) as usize].iter().find(|e| !known.contains(&e.0))
        {
            unsupported(b"kernel32.dll!CreateRemoteThreadEx(attribute)", other as u64);
            return fail_with(ERROR_NOT_SUPPORTED, core::ptr::null_mut());
        }
    }
    // Suspended until the attributes are applied.
    let thread = super::CreateThread(
        security,
        stack_size,
        start,
        param,
        flags | CREATE_SUSPENDED,
        thread_id,
    );
    if thread.is_null() {
        return thread;
    }
    if let Some((value, _)) = attribute(list, PROC_THREAD_ATTRIBUTE_GROUP_AFFINITY) {
        let affinity = value as *const super::processor::GroupAffinity;
        if super::processor::SetThreadGroupAffinity(thread, affinity, core::ptr::null_mut())
            == FALSE
        {
            // SAFETY: the thread just created (suspended).
            unsafe { super::NtTerminateThread(thread as isize, 0) };
            CloseHandle(thread);
            return core::ptr::null_mut();
        }
    }
    if let Some((value, _)) = attribute(list, PROC_THREAD_ATTRIBUTE_IDEAL_PROCESSOR) {
        let ideal = value as *const super::processor::ProcessorNumber;
        if super::processor::SetThreadIdealProcessorEx(thread, ideal, core::ptr::null_mut())
            == FALSE
        {
            // SAFETY: the thread just created (suspended).
            unsafe { super::NtTerminateThread(thread as isize, 0) };
            CloseHandle(thread);
            return core::ptr::null_mut();
        }
    }
    if flags & CREATE_SUSPENDED == 0 {
        super::ResumeThread(thread);
    }
    thread
}

/// GetProcessTimes(hProcess, *lpCreationTime, *lpExitTime, *lpKernelTime,
/// *lpUserTime): ProcessTimes (KERNEL_USER_TIMES).
#[unsafe(no_mangle)]
pub extern "system" fn GetProcessTimes(
    process: Handle,
    creation: *mut u64,
    exit: *mut u64,
    kernel: *mut u64,
    user: *mut u64,
) -> Bool {
    let mut t = [0u64; 4];
    // SAFETY: a 32-byte KERNEL_USER_TIMES buffer.
    let status = unsafe {
        NtQueryInformationProcess(
            process as isize,
            4,
            t.as_mut_ptr().cast(),
            32,
            core::ptr::null_mut(),
        )
    };
    if status != STATUS_SUCCESS {
        return fail(status, FALSE);
    }
    for (out, v) in [creation, exit, kernel, user].into_iter().zip(t) {
        if !out.is_null() {
            // SAFETY: the caller's FILETIME (8 bytes, maybe unaligned).
            unsafe { out.write_unaligned(v) };
        }
    }
    TRUE
}

/// GetProcessId: PROCESS_BASIC_INFORMATION.UniqueProcessId.
#[unsafe(no_mangle)]
pub extern "system" fn GetProcessId(process: Handle) -> u32 {
    match process_basic(process) {
        Ok(b) => b.pid as u32,
        Err(status) => fail(status, 0),
    }
}

/// ProcessIdToSessionId(pid, *session): the caller's own from its PEB;
/// another process's through OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)
/// and ProcessSessionInformation (an id that is no process:
/// ERROR_INVALID_PARAMETER; a process the caller may not query:
/// ERROR_ACCESS_DENIED).
#[unsafe(no_mangle)]
pub extern "system" fn ProcessIdToSessionId(pid: u32, session: *mut u32) -> Bool {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const PROCESS_SESSION_INFORMATION: u32 = 24;
    if session.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let id = if pid == super::GetCurrentProcessId() {
        super::sync::session_id()
    } else {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
        if process.is_null() {
            return FALSE; // OpenProcess set the error
        }
        let mut id = 0u32;
        // SAFETY: a 4-byte buffer; our handle.
        let status = unsafe {
            let status = NtQueryInformationProcess(
                process as isize,
                PROCESS_SESSION_INFORMATION,
                (&raw mut id).cast(),
                4,
                core::ptr::null_mut(),
            );
            NtClose(process);
            status
        };
        if status != STATUS_SUCCESS {
            return fail(status, FALSE);
        }
        id
    };
    // SAFETY: the caller's output.
    unsafe { session.write_unaligned(id) };
    TRUE
}

/// WTSGetActiveConsoleSessionId: the session the console shows
/// (KUSER_SHARED_DATA.ActiveConsoleId at 0x7ffe02d8; 1 under gala).
#[unsafe(no_mangle)]
pub extern "system" fn WTSGetActiveConsoleSessionId() -> u32 {
    // SAFETY: KUSER_SHARED_DATA is mapped read-only in every process.
    unsafe { core::ptr::read_volatile(0x7ffe_02d8 as *const u32) }
}

/// Whether `process` is a 32-bit (WOW64) process: ProcessWow64Information
/// (its PEB32, 0 for a 64-bit process); the status on failure.
pub(crate) fn is_wow64(process: Handle) -> Result<bool, u32> {
    let mut peb32 = 0usize;
    // SAFETY: a ULONG_PTR buffer.
    let status = unsafe {
        NtQueryInformationProcess(
            process as isize,
            26,
            (&raw mut peb32).cast(),
            size_of::<usize>() as u32,
            core::ptr::null_mut(),
        )
    };
    if status != STATUS_SUCCESS {
        return Err(status);
    }
    Ok(peb32 != 0)
}

/// IsWow64Process: whether `process` is a 32-bit program running through
/// WOW64 (ARCHITECTURE "Plan: WOW64").
#[unsafe(no_mangle)]
pub extern "system" fn IsWow64Process(process: Handle, wow64: *mut Bool) -> Bool {
    let answer = match is_wow64(process) {
        Ok(w) => w,
        Err(status) => return fail(status, FALSE),
    };
    if !wow64.is_null() {
        // SAFETY: the caller's output pointer.
        unsafe { *wow64 = Bool::from(answer) };
    }
    TRUE
}

/// OpenProcess(access, inherit, id): NtOpenProcess with the id as
/// CLIENT_ID.UniqueProcess; an id that is no process fails with
/// ERROR_INVALID_PARAMETER.
#[unsafe(no_mangle)]
pub extern "system" fn OpenProcess(access: u32, inherit: Bool, id: u32) -> Handle {
    let attributes = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root_directory: 0,
        object_name: core::ptr::null(),
        attributes: if inherit != 0 { 2 } else { 0 }, // OBJ_INHERIT
        security_descriptor: 0,
        security_quality_of_service: 0,
    };
    let client_id = [id as usize, 0];
    let mut h = 0usize;
    // SAFETY: valid output, attributes and CLIENT_ID.
    let status = unsafe { NtOpenProcess(&mut h, access, &attributes, &client_id) };
    if status != STATUS_SUCCESS {
        return fail(status, core::ptr::null_mut());
    }
    h as Handle
}

#[cfg_attr(
    target_arch = "x86",
    link(name = "ntdll", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "ntdll", kind = "raw-dylib"))]
unsafe extern "system" {
    fn NtReadVirtualMemory(
        p: Handle,
        base: usize,
        buf: *mut u8,
        size: usize,
        done: *mut usize,
    ) -> u32;
    fn NtWriteVirtualMemory(
        p: Handle,
        base: usize,
        buf: *const u8,
        size: usize,
        done: *mut usize,
    ) -> u32;
}

/// ReadProcessMemory: copies from another process's memory (the handle
/// needs PROCESS_VM_READ); *read (optional) gets the bytes copied, also
/// when it fails part of the way (ERROR_PARTIAL_COPY).
#[unsafe(no_mangle)]
pub extern "system" fn ReadProcessMemory(
    process: Handle,
    address: *const c_void,
    buffer: *mut c_void,
    size: usize,
    read: *mut usize,
) -> Bool {
    let mut done = 0usize;
    // SAFETY: the caller's buffer of `size` bytes; the kernel checks both.
    let status =
        unsafe { NtReadVirtualMemory(process, address as usize, buffer.cast(), size, &mut done) };
    if !read.is_null() {
        // SAFETY: the caller's output.
        unsafe { *read = done };
    }
    if status != 0 { fail(status, FALSE) } else { TRUE }
}

/// WriteProcessMemory: copies into another process's memory (the handle
/// needs PROCESS_VM_WRITE).
#[unsafe(no_mangle)]
pub extern "system" fn WriteProcessMemory(
    process: Handle,
    address: *mut c_void,
    buffer: *const c_void,
    size: usize,
    written: *mut usize,
) -> Bool {
    let mut done = 0usize;
    // SAFETY: the caller's buffer of `size` bytes; the kernel checks both.
    let status =
        unsafe { NtWriteVirtualMemory(process, address as usize, buffer.cast(), size, &mut done) };
    if !written.is_null() {
        // SAFETY: the caller's output.
        unsafe { *written = done };
    }
    if status != 0 { fail(status, FALSE) } else { TRUE }
}

#[unsafe(no_mangle)]
pub extern "system" fn CreateRemoteThread(
    _process: Handle,
    _attributes: *mut c_void,
    _stack: usize,
    _start: usize,
    _parameter: *mut c_void,
    _flags: u32,
    _id: *mut u32,
) -> Handle {
    unsupported(b"kernel32.dll!CreateRemoteThread", 0);
    fail_with(ERROR_NOT_SUPPORTED, core::ptr::null_mut())
}

/// GetStartupInfoW: from the process parameters: the standard handles
/// (STARTF_USESTDHANDLES) and cbReserved2/lpReserved2 (RuntimeData, the
/// C runtime's inherited descriptors).
#[unsafe(no_mangle)]
pub extern "system" fn GetStartupInfoW(info: *mut u8) {
    let params = process_parameters();
    // SAFETY: our parameters: the standard handles, RuntimeData (a
    // UNICODE_STRING's Length and Buffer).
    let (std, len, data) = unsafe {
        (
            *((params + PARAMS_STANDARD_INPUT) as *const [usize; 3]),
            *((params + PARAMS_RUNTIME_DATA) as *const u16),
            *((params + PARAMS_RUNTIME_DATA + US_BUFFER) as *const usize),
        )
    };
    // SAFETY: an all-zero STARTUPINFO is a valid bit pattern.
    let mut s: StartupInfoEx = unsafe { core::mem::zeroed() };
    s.cb = STARTUPINFO_SIZE as u32;
    s.flags = STARTF_USESTDHANDLES;
    s.std = std;
    if len != 0 && data != 0 {
        s.reserved2_len = len;
        s.reserved2 = data;
    }
    // SAFETY: the caller's STARTUPINFOW (without the EX part).
    unsafe { core::ptr::copy_nonoverlapping((&raw const s).cast::<u8>(), info, STARTUPINFO_SIZE) };
}

/// GetStartupInfoA: the same fields (the string fields stay null).
#[unsafe(no_mangle)]
pub extern "system" fn GetStartupInfoA(info: *mut u8) {
    GetStartupInfoW(info);
}

// ---- Small thread and processor functions ----

/// The process's pointer-encoding secret, chosen at first use.
static POINTER_COOKIE: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

fn pointer_cookie() -> usize {
    use core::sync::atomic::Ordering;
    let c = POINTER_COOKIE.load(Ordering::Relaxed);
    if c != 0 {
        return c;
    }
    let fresh = rdtsc() as usize | 1;
    match POINTER_COOKIE.compare_exchange(0, fresh, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => fresh,
        Err(other) => other,
    }
}

/// EncodePointer: an opaque form of `p` (XOR with a per-process secret,
/// rotated by it), undone by DecodePointer.
#[unsafe(no_mangle)]
pub extern "system" fn EncodePointer(p: *mut c_void) -> *mut c_void {
    let c = pointer_cookie();
    ((p as usize ^ c).rotate_right(c as u32 % usize::BITS)) as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "system" fn DecodePointer(p: *mut c_void) -> *mut c_void {
    let c = pointer_cookie();
    ((p as usize).rotate_left(c as u32 % usize::BITS) ^ c) as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "system" fn EncodeSystemPointer(p: *mut c_void) -> *mut c_void {
    EncodePointer(p)
}

#[unsafe(no_mangle)]
pub extern "system" fn DecodeSystemPointer(p: *mut c_void) -> *mut c_void {
    DecodePointer(p)
}

/// IsDebuggerPresent: PEB.BeingDebugged.
#[unsafe(no_mangle)]
pub extern "system" fn IsDebuggerPresent() -> Bool {
    // SAFETY: the PEB's BeingDebugged byte.
    unsafe { Bool::from(*((peb() + PEB_BEING_DEBUGGED) as *const u8)) }
}

/// DebugBreak: a breakpoint exception.
#[unsafe(no_mangle)]
pub extern "system" fn DebugBreak() {
    // SAFETY: raises STATUS_BREAKPOINT in this thread.
    unsafe { core::arch::asm!("int3") };
}

/// IsProcessorFeaturePresent: KUSER_SHARED_DATA.ProcessorFeatures.
#[unsafe(no_mangle)]
pub extern "system" fn IsProcessorFeaturePresent(feature: u32) -> Bool {
    if feature >= 64 {
        return FALSE;
    }
    // SAFETY: KUSER_SHARED_DATA is mapped read-only at 0x7ffe0000;
    // ProcessorFeatures[64] is at +0x274.
    Bool::from(unsafe { *((0x7ffe_0274 + feature as usize) as *const u8) })
}

/// GetCurrentProcessorNumber: the processor number the kernel keeps in
/// IA32_TSC_AUX (read with rdtscp).
#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentProcessorNumber() -> u32 {
    let mut aux = 0u32;
    rdtscp(&mut aux);
    aux & 0xfff
}

/// FlushProcessWriteBuffers: a full fence (gala's processors are kept
/// coherent; TODO: an IPI to serialize the other processors, as Windows).
#[unsafe(no_mangle)]
pub extern "system" fn FlushProcessWriteBuffers() {
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

/// IsThreadAFiber: gala has no fibers.
#[unsafe(no_mangle)]
pub extern "system" fn IsThreadAFiber() -> Bool {
    FALSE
}

/// DisableThreadLibraryCalls: no DLL_THREAD_ATTACH/DETACH to the DLL's
/// DllMain (LdrDisableThreadCalloutsForDll).
#[unsafe(no_mangle)]
pub extern "system" fn DisableThreadLibraryCalls(module: Handle) -> Bool {
    // SAFETY: a plain call.
    let status = unsafe { super::LdrDisableThreadCalloutsForDll(module) };
    if status != STATUS_SUCCESS { fail(status, FALSE) } else { TRUE }
}

#[unsafe(no_mangle)]
pub extern "system" fn FreeLibraryAndExitThread(module: Handle, code: u32) -> ! {
    FreeLibrary(module);
    ExitThread(code)
}

/// GetThreadTimes: ThreadTimes (creation, exit, kernel, user).
#[unsafe(no_mangle)]
pub extern "system" fn GetThreadTimes(
    h: Handle,
    creation: *mut u64,
    exit: *mut u64,
    kernel: *mut u64,
    user: *mut u64,
) -> Bool {
    let mut times = [0u64; 4];
    let mut ret = 0;
    // SAFETY: a 32-byte KERNEL_USER_TIMES buffer.
    let status = unsafe { NtQueryInformationThread(h, 1, times.as_mut_ptr().cast(), 32, &mut ret) };
    if status != STATUS_SUCCESS {
        return fail(status, FALSE);
    }
    for (p, v) in [creation, exit, kernel, user].into_iter().zip(times) {
        if !p.is_null() {
            // SAFETY: the caller's FILETIME.
            unsafe { p.write_unaligned(v) };
        }
    }
    TRUE
}

/// GetThreadId: from ThreadBasicInformation's ClientId.
#[unsafe(no_mangle)]
pub extern "system" fn GetThreadId(h: Handle) -> u32 {
    match thread_basic(h) {
        Ok(b) => b.client_id[1] as u32,
        Err(status) => fail(status, 0),
    }
}

/// OpenThread: NtOpenThread on the thread's id (ERROR_INVALID_PARAMETER
/// for an id no thread has).
#[unsafe(no_mangle)]
pub extern "system" fn OpenThread(access: u32, inherit: Bool, id: u32) -> Handle {
    let attributes = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root_directory: 0,
        object_name: core::ptr::null(),
        attributes: if inherit != 0 { 2 } else { 0 }, // OBJ_INHERIT
        security_descriptor: 0,
        security_quality_of_service: 0,
    };
    let client_id = [0usize, id as usize];
    let mut h = 0usize;
    // SAFETY: valid output, attributes and CLIENT_ID.
    let status = unsafe { NtOpenThread(&mut h, access, &attributes, &client_id) };
    if status != STATUS_SUCCESS {
        return fail(status, core::ptr::null_mut());
    }
    h as Handle
}

/// SetProcessDEPPolicy: for 32-bit processes only; a 64-bit process (all
/// of gala's: DEP is always on for them) gets ERROR_NOT_SUPPORTED, as
/// documented.
#[unsafe(no_mangle)]
pub extern "system" fn SetProcessDEPPolicy(_flags: u32) -> Bool {
    fail_with(ERROR_NOT_SUPPORTED, FALSE)
}
