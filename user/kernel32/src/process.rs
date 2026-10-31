//! CreateProcessW/A on gala's process creation service (GalaCreateProcess,
//! docs/ARCHITECTURE.md "Processes started by programs"), and
//! GetModuleFileNameW.

use super::sync::ansi_to_wide;
use super::*;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaCreateProcess(parameters: *mut u8) -> u32;
}

const CREATE_SUSPENDED: u32 = 0x4;
const DETACHED_PROCESS: u32 = 0x8;
const CREATE_NEW_CONSOLE: u32 = 0x10;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
const CREATE_UNICODE_ENVIRONMENT: u32 = 0x400;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Flags that change nothing on gala (one console, no windows).
const IGNORED_FLAGS: u32 =
    DETACHED_PROCESS | CREATE_NEW_CONSOLE | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;
const STARTF_USESTDHANDLES: u32 = 0x100;
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
/// directory of the executable, the current directory, System32, Windows.
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
    if !process_attributes.is_null() || !thread_attributes.is_null() {
        unsupported(b"kernel32.dll!CreateProcessW(security attributes)", 0);
    }
    let unknown = flags & !(CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | IGNORED_FLAGS);
    if unknown != 0 {
        unsupported(b"kernel32.dll!CreateProcessW(flags)", unknown.into());
    }
    // The image: as given (with a directory), or searched for.
    let mut token = PathBuf::new();
    if let Err(e) = program_token(application, command_line, &mut token) {
        return fail_with(e, FALSE);
    }
    let found = if token.as_slice().iter().any(|&c| is_sep(c) || c == u16::from(b':')) {
        Ok(token)
    } else {
        search(&token)
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
        unsupported(b"kernel32.dll!CreateProcessW(ANSI environment)", 0);
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    };
    // Standard handles: from STARTUPINFO if it says so, else ours.
    // SAFETY: the caller's STARTUPINFOW (dwFlags at 60, handles at 80).
    let std = unsafe {
        if !startup.is_null() && *startup.add(60).cast::<u32>() & STARTF_USESTDHANDLES != 0 {
            [
                *startup.add(80).cast::<usize>(),
                *startup.add(88).cast::<usize>(),
                *startup.add(96).cast::<usize>(),
            ]
        } else {
            [
                GetStdHandle(-10i32 as u32) as usize,
                GetStdHandle(-11i32 as u32) as usize,
                GetStdHandle(-12i32 as u32) as usize,
            ]
        }
    };
    let mut p = [0u8; 120];
    let mut put = |at: usize, v: u64| p[at..at + 8].copy_from_slice(&v.to_le_bytes());
    put(0, image.chars.as_ptr() as u64);
    put(16, line.as_ptr() as u64);
    put(32, cwd.chars.as_ptr() as u64);
    put(48, env as u64);
    put(64, std[0] as u64);
    put(72, std[1] as u64);
    put(80, std[2] as u64);
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
    p[12..16].copy_from_slice(&bits.to_le_bytes());
    // SAFETY: the 120-byte parameter block; its pointers live through
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
    if !environment.is_null() && flags & CREATE_UNICODE_ENVIRONMENT == 0 {
        unsupported(b"kernel32.dll!CreateProcessA(ANSI environment)", 0);
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    CreateProcessW(
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
    )
}

/// GetModuleFileNameW: the executable's path (other modules: reported).
#[unsafe(no_mangle)]
pub extern "system" fn GetModuleFileNameW(module: Handle, buf: *mut u16, size: u32) -> u32 {
    // SAFETY: gs:[0x60] is the PEB; ImageBaseAddress at +0x10.
    let image = unsafe { *((teb_u64(0x60) as usize + 0x10) as *const Handle) };
    if !module.is_null() && module != image {
        unsupported(b"kernel32.dll!GetModuleFileNameW(DLL)", 0);
        return fail_with(ERROR_NOT_SUPPORTED, 0);
    }
    if size == 0 {
        return fail_with(122, 0);
    }
    // RTL_USER_PROCESS_PARAMETERS.ImagePathName (UNICODE_STRING at +0x60).
    // SAFETY: the PEB's process parameters are always set.
    let path = unsafe {
        let params = *((teb_u64(0x60) as usize + 0x20) as *const usize);
        let len = usize::from(*((params + 0x60) as *const u16)) / 2;
        core::slice::from_raw_parts(*((params + 0x68) as *const *const u16), len)
    };
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
