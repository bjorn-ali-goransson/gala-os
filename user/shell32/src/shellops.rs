//! What wxWidgets imports from shell32 beyond folders: running programs
//! (ShellExecuteW/ShellExecuteExW), dropped files (DragQueryFileW and
//! friends on an HDROP), file operations (SHFileOperationW), and the
//! notifications gala has no receiver for (SHChangeNotify,
//! SHGetIconOverlayIndexW; the notification area is `tray.rs`). As documented; see
//! user/shell32_test.
//!
//! ShellExecute runs programs (.exe and .com, found as CreateProcess and
//! the search path find them) for the verbs "open" (or none) and
//! "runas", and documents with their association's command for "open"
//! (`association`: the registry's classes through shlwapi); URLs, other
//! verbs and extensions no class claims are SE_ERR_NOASSOC (recorded). "runas" (elevation, as UAC
//! does it; docs/SECURITY.md "Elevation") starts the program with the
//! caller's own token when that token is already elevated, which is what
//! Windows does for an elevated administrator; a caller that is not
//! elevated would get UAC's prompt, which gala does not have: the request
//! is declined (recorded), as when the user cancels the prompt. user32
//! and advapi32 are reached at run time (DragAcceptFiles, the token
//! query), so loading shell32 loads neither.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

#[cfg_attr(
    target_arch = "x86",
    link(name = "kernel32", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(not(target_arch = "x86"), link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn SearchPathW(
        path: *const u16,
        file: *const u16,
        ext: *const u16,
        size: u32,
        buf: *mut u16,
        part: *mut *mut u16,
    ) -> u32;
    fn CreateProcessW(
        app: *const u16,
        cmdline: *mut u16,
        process_attrs: *const c_void,
        thread_attrs: *const c_void,
        inherit: i32,
        flags: u32,
        env: *const c_void,
        dir: *const u16,
        startup: *const StartupInfo,
        info: *mut ProcessInformation,
    ) -> i32;
    fn CloseHandle(h: usize) -> i32;
    fn SetLastError(e: u32);
    fn GetLastError() -> u32;
    fn GlobalLock(h: *mut c_void) -> *mut u8;
    fn GlobalUnlock(h: *mut c_void) -> i32;
    fn GlobalFree(h: *mut c_void) -> *mut c_void;
    fn MultiByteToWideChar(cp: u32, flags: u32, s: *const u8, n: i32, w: *mut u16, m: i32) -> i32;
    fn LoadLibraryW(name: *const u16) -> usize;
    fn GetProcAddress(module: usize, name: *const u8) -> *const c_void;
    fn FindFirstFileW(pattern: *const u16, data: *mut FindData) -> usize;
    fn FindNextFileW(h: usize, data: *mut FindData) -> i32;
    fn FindClose(h: usize) -> i32;
    fn DeleteFileW(path: *const u16) -> i32;
    fn RemoveDirectoryW(path: *const u16) -> i32;
    fn CreateDirectoryW(path: *const u16, attrs: *const c_void) -> i32;
    fn CopyFileW(from: *const u16, to: *const u16, fail_if_exists: i32) -> i32;
    fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
}

fn record(what: &[u8], detail: u64) {
    crate::unsupported(what, detail);
}

fn wlen(p: *const u16) -> usize {
    let mut n = 0;
    // SAFETY: the caller's NUL-terminated string.
    while unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    n
}

fn chars<'a>(p: *const u16) -> &'a [u16] {
    if p.is_null() {
        return &[];
    }
    // SAFETY: the caller's string of `wlen(p)` characters.
    unsafe { core::slice::from_raw_parts(p, wlen(p)) }
}

/// A NUL-terminated path in a fixed buffer.
struct Path {
    buf: [u16; 520],
    len: usize,
}

impl Path {
    fn new() -> Self {
        Path { buf: [0; 520], len: 0 }
    }
    fn from(s: &[u16]) -> Option<Self> {
        let mut p = Path::new();
        p.push(s)?;
        Some(p)
    }
    fn push(&mut self, s: &[u16]) -> Option<()> {
        if self.len + s.len() + 1 > self.buf.len() {
            return None;
        }
        self.buf[self.len..self.len + s.len()].copy_from_slice(s);
        self.len += s.len();
        self.buf[self.len] = 0;
        Some(())
    }
    /// `name` as a component after a backslash.
    fn join(&self, name: &[u16]) -> Option<Path> {
        let mut p = Path::from(self.text())?;
        if p.len > 0 && p.buf[p.len - 1] != u16::from(b'\\') {
            p.push(&[u16::from(b'\\')])?;
        }
        p.push(name)?;
        Some(p)
    }
    fn text(&self) -> &[u16] {
        &self.buf[..self.len]
    }
    fn ptr(&self) -> *const u16 {
        self.buf.as_ptr()
    }
}

// ---- Running programs ----

#[repr(C)]
struct StartupInfo {
    cb: u32,
    reserved: *const u16,
    desktop: *const u16,
    title: *const u16,
    x: u32,
    y: u32,
    x_size: u32,
    y_size: u32,
    x_chars: u32,
    y_chars: u32,
    fill: u32,
    flags: u32,
    show: u16,
    reserved2: u16,
    reserved3: *const u8,
    stdin: usize,
    stdout: usize,
    stderr: usize,
}

#[repr(C)]
struct ProcessInformation {
    process: usize,
    thread: usize,
    pid: u32,
    tid: u32,
}

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_NO_ASSOCIATION: u32 = 1155;
const ERROR_CANCELLED: u32 = 1223;
const SE_ERR_FNF: usize = 2;
const SE_ERR_ACCESSDENIED: usize = 5;
const SE_ERR_NOASSOC: usize = 31;
const STARTF_USESHOWWINDOW: u32 = 1;

fn ascii_eq(a: &[u16], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(&x, &y)| x < 128 && (x as u8).eq_ignore_ascii_case(&y))
}

/// Runs `file` (a program) with `params` in `dir`: the process handle,
/// or the SE_ERR_* code and the Win32 error.
fn execute(
    verb: *const u16,
    file: *const u16,
    params: *const u16,
    dir: *const u16,
    show: i32,
) -> Result<usize, (usize, u32)> {
    if file.is_null() {
        return Err((SE_ERR_FNF, ERROR_FILE_NOT_FOUND));
    }
    let verb = chars(verb);
    if ascii_eq(verb, b"runas") {
        // Elevation: an elevated caller's token is already the full one,
        // so the program starts with it (CreateProcess below, no prompt);
        // else UAC would ask (consent or credentials), and gala has no
        // prompt: declined, as a cancelled prompt answers.
        if !caller_elevated() {
            record(b"shell32.dll!ShellExecute(runas: elevation prompt)", 0);
            return Err((SE_ERR_ACCESSDENIED, ERROR_CANCELLED));
        }
    } else if !verb.is_empty() && !ascii_eq(verb, b"open") {
        record(b"shell32.dll!ShellExecute(verb)", verb.len() as u64);
        return Err((SE_ERR_NOASSOC, ERROR_NO_ASSOCIATION));
    }
    // The program: as given, else along the search path (.exe added).
    let mut found = [0u16; 520];
    let exe = [u16::from(b'.'), u16::from(b'e'), u16::from(b'x'), u16::from(b'e'), 0];
    // SAFETY: the caller's string; our buffer of its size.
    let n = unsafe {
        SearchPathW(
            core::ptr::null(),
            file,
            exe.as_ptr(),
            520,
            found.as_mut_ptr(),
            core::ptr::null_mut(),
        )
    };
    let path = if n != 0 && (n as usize) < found.len() {
        &found[..n as usize]
    } else {
        // SAFETY: the caller's string.
        if unsafe { GetFileAttributesW(file) } == u32::MAX {
            return Err((SE_ERR_FNF, ERROR_FILE_NOT_FOUND));
        }
        chars(file)
    };
    let extension = path.rsplit(|&c| c == u16::from(b'.')).next().unwrap_or(&[]);
    if !(ascii_eq(extension, b"exe") || ascii_eq(extension, b"com")) {
        // A document: its association's command.
        let mut line = [0u16; 2048];
        let mut program = [0u16; 520];
        association(verb, path, chars(params), &mut program, &mut line)?;
        return start(&program, &mut line, dir, show);
    }
    // "path" params
    let mut line = [0u16; 2048];
    let mut len = 0;
    let mut put = |s: &[u16]| -> bool {
        if len + s.len() + 1 > line.len() {
            return false;
        }
        line[len..len + s.len()].copy_from_slice(s);
        len += s.len();
        true
    };
    let quote = [u16::from(b'"')];
    let params = chars(params);
    let fits = put(&quote)
        && put(path)
        && put(&quote)
        && (params.is_empty() || (put(&[u16::from(b' ')]) && put(params)));
    if !fits {
        return Err((SE_ERR_FNF, 206)); // ERROR_FILENAME_EXCED_RANGE
    }
    let mut program = [0u16; 520];
    program[..path.len()].copy_from_slice(path);
    start(&program, &mut line, dir, show)
}

/// Starts `program` (NUL-terminated) with command line `line` (NUL-
/// terminated, writable) in `dir` (none: ours): the process handle.
fn start(
    program: &[u16],
    line: &mut [u16],
    dir: *const u16,
    show: i32,
) -> Result<usize, (usize, u32)> {
    let startup = StartupInfo {
        cb: core::mem::size_of::<StartupInfo>() as u32,
        reserved: core::ptr::null(),
        desktop: core::ptr::null(),
        title: core::ptr::null(),
        x: 0,
        y: 0,
        x_size: 0,
        y_size: 0,
        x_chars: 0,
        y_chars: 0,
        fill: 0,
        flags: STARTF_USESHOWWINDOW,
        show: show as u16,
        reserved2: 0,
        reserved3: core::ptr::null(),
        stdin: 0,
        stdout: 0,
        stderr: 0,
    };
    let mut info = ProcessInformation { process: 0, thread: 0, pid: 0, tid: 0 };
    let dir = if chars(dir).is_empty() { core::ptr::null() } else { dir };
    // SAFETY: our NUL-terminated program path and command line (writable,
    // as CreateProcessW wants), the caller's directory, our structures.
    let ok = unsafe {
        CreateProcessW(
            program.as_ptr(),
            line.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            0,
            0,
            core::ptr::null(),
            dir,
            &startup,
            &mut info,
        )
    };
    if ok == 0 {
        // SAFETY: plain call.
        let e = unsafe { GetLastError() };
        return Err((
            if e == ERROR_FILE_NOT_FOUND || e == 3 { SE_ERR_FNF } else { SE_ERR_NOASSOC },
            e,
        ));
    }
    // SAFETY: the new thread's handle, ours to close.
    unsafe { CloseHandle(info.thread) };
    Ok(info.process)
}

/// The command line of document `path`'s association for `verb` ("open"
/// when none) into `line`, its program into `program` (both NUL-
/// terminated): shlwapi's AssocQueryStringW (the user's classes, then
/// HKCR) for ASSOCSTR_COMMAND, with `%1`/`%L` the document's path and
/// `%*` the parameters. An extension no class claims has no program
/// (Windows' Unknown class runs OpenWith.exe, which gala has not):
/// SE_ERR_NOASSOC, recorded. When the program is `wsl.exe` and the
/// document is in that distribution's tree (`\\wsl.localhost\<d>\...`),
/// `%1` is the document's Linux path (`/a/b`): the association runs a
/// Linux program on it (ROADMAP EX step 2).
fn association(
    verb: &[u16],
    path: &[u16],
    params: &[u16],
    program: &mut [u16; 520],
    line: &mut [u16; 2048],
) -> Result<(), (usize, u32)> {
    type Query = extern "system" fn(u32, u32, *const u16, *const u16, *mut u16, *mut u32) -> i32;
    // "shlwapi.dll".
    const SHLWAPI: [u16; 12] = [115, 104, 108, 119, 97, 112, 105, 46, 100, 108, 108, 0];
    const ASSOCSTR_COMMAND: u32 = 1;
    const ASSOCSTR_EXECUTABLE: u32 = 2;
    let no_assoc = || {
        record(b"shell32.dll!ShellExecute(no association)", 0);
        Err((SE_ERR_NOASSOC, ERROR_NO_ASSOCIATION))
    };
    // SAFETY: a NUL-terminated name; the documented signature.
    let query: Query = unsafe {
        let m = LoadLibraryW(SHLWAPI.as_ptr());
        let f = if m == 0 {
            core::ptr::null()
        } else {
            GetProcAddress(m, c"AssocQueryStringW".as_ptr().cast())
        };
        if f.is_null() {
            return no_assoc();
        }
        core::mem::transmute::<*const c_void, Query>(f)
    };
    // The extension with its dot, NUL-terminated.
    let dot = path.iter().rposition(|&c| c == u16::from(b'.')).unwrap_or(path.len());
    let mut ext = [0u16; 64];
    let e = &path[dot..];
    if e.is_empty() || e.len() >= ext.len() {
        return no_assoc();
    }
    ext[..e.len()].copy_from_slice(e);
    let mut v = [0u16; 32];
    let verb = if verb.is_empty() { &[111, 112, 101, 110][..] } else { verb };
    if verb.len() >= v.len() {
        return no_assoc();
    }
    v[..verb.len()].copy_from_slice(verb);
    let mut n = 520u32;
    if query(0, ASSOCSTR_EXECUTABLE, ext.as_ptr(), v.as_ptr(), program.as_mut_ptr(), &mut n) != 0 {
        return no_assoc();
    }
    let mut command = [0u16; 1024];
    let mut n = 1024u32;
    if query(0, ASSOCSTR_COMMAND, ext.as_ptr(), v.as_ptr(), command.as_mut_ptr(), &mut n) != 0 {
        return no_assoc();
    }
    let command = &command[..command.iter().position(|&c| c == 0).unwrap_or(0)];
    let exe = &program[..program.iter().position(|&c| c == 0).unwrap_or(0)];
    let exe_name = exe.rsplit(|&c| c == u16::from(b'\\')).next().unwrap_or(&[]);
    // A document in a distribution, for wsl.exe: its Linux path.
    let mut linux = [0u16; 520];
    let mut document = path;
    if ascii_eq(exe_name, b"wsl.exe") {
        let prefix: [u16; 16] =
            [92, 92, 119, 115, 108, 46, 108, 111, 99, 97, 108, 104, 111, 115, 116, 92];
        if path.len() > prefix.len()
            && path[..prefix.len()].iter().zip(&prefix).all(|(&a, &b)| a | 0x20 == b | 0x20)
        {
            let rest = &path[prefix.len()..];
            let at = rest.iter().position(|&c| c == u16::from(b'\\')).unwrap_or(rest.len());
            let tail = &rest[at..];
            for (o, &c) in linux.iter_mut().zip(tail) {
                *o = if c == u16::from(b'\\') { u16::from(b'/') } else { c };
            }
            document = &linux[..tail.len().min(linux.len())];
        }
    }
    let mut len = 0;
    let mut i = 0;
    while i < command.len() {
        let c = command[i];
        let next = command.get(i + 1).copied();
        let insert: Option<&[u16]> = match (c, next) {
            (37, Some(49)) | (37, Some(76)) | (37, Some(108)) => Some(document), // %1 %L %l
            (37, Some(42)) => Some(params),                                      // %*
            _ => None,
        };
        let piece: &[u16] = match insert {
            Some(p) => {
                i += 2;
                p
            }
            None => {
                i += 1;
                core::slice::from_ref(&command[i - 1])
            }
        };
        if len + piece.len() + 1 > line.len() {
            return Err((SE_ERR_FNF, 206)); // ERROR_FILENAME_EXCED_RANGE
        }
        line[len..len + piece.len()].copy_from_slice(piece);
        len += piece.len();
    }
    line[len] = 0;
    Ok(())
}

/// The calling process's token is elevated (TokenElevation, through
/// advapi32 loaded at run time); false when that cannot be asked.
fn caller_elevated() -> bool {
    type OpenProcessToken = unsafe extern "system" fn(usize, u32, *mut usize) -> i32;
    type GetTokenInformation =
        unsafe extern "system" fn(usize, u32, *mut c_void, u32, *mut u32) -> i32;
    const TOKEN_QUERY: u32 = 8;
    const TOKEN_ELEVATION: u32 = 20;
    // "advapi32.dll".
    const ADVAPI32: [u16; 13] = [97, 100, 118, 97, 112, 105, 51, 50, 46, 100, 108, 108, 0];
    // SAFETY: NUL-terminated names; the functions' documented signatures;
    // a token handle of ours, closed; a 4-byte TOKEN_ELEVATION.
    unsafe {
        let module = LoadLibraryW(ADVAPI32.as_ptr());
        if module == 0 {
            return false;
        }
        let open = GetProcAddress(module, c"OpenProcessToken".as_ptr().cast());
        let query = GetProcAddress(module, c"GetTokenInformation".as_ptr().cast());
        if open.is_null() || query.is_null() {
            return false;
        }
        let open: OpenProcessToken = core::mem::transmute(open);
        let query: GetTokenInformation = core::mem::transmute(query);
        let mut token = 0usize;
        // GetCurrentProcess's pseudo handle.
        if open(usize::MAX, TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let (mut elevation, mut n) = (0u32, 0u32);
        let ok = query(token, TOKEN_ELEVATION, (&raw mut elevation).cast(), 4, &mut n);
        CloseHandle(token);
        ok != 0 && elevation != 0
    }
}

/// ShellExecuteW: runs a program; a value over 32 on success, else
/// SE_ERR_FNF (2) for a file that is not there, SE_ERR_NOASSOC (31)
/// for a document or URL (gala has no associations; recorded), and
/// SE_ERR_ACCESSDENIED (5) with ERROR_CANCELLED for "runas" by a caller
/// that is not elevated (UAC's prompt, which gala lacks, declined).
#[unsafe(no_mangle)]
pub extern "system" fn ShellExecuteW(
    _window: usize,
    verb: *const u16,
    file: *const u16,
    params: *const u16,
    dir: *const u16,
    show: i32,
) -> usize {
    match execute(verb, file, params, dir, show) {
        Ok(process) => {
            // SAFETY: the new process's handle, ours to close.
            unsafe { CloseHandle(process) };
            33
        }
        Err((code, error)) => {
            // SAFETY: plain call.
            unsafe { SetLastError(error) };
            code
        }
    }
}

/// SHELLEXECUTEINFOW (x64 layout).
#[repr(C)]
pub struct ShellExecuteInfo {
    size: u32,
    mask: u32,
    window: usize,
    verb: *const u16,
    file: *const u16,
    params: *const u16,
    dir: *const u16,
    show: i32,
    instance: usize,
    id_list: *const c_void,
    class: *const u16,
    class_key: usize,
    hot_key: u32,
    icon: usize,
    process: usize,
}

const SEE_MASK_CLASSNAME: u32 = 0x1;
const SEE_MASK_CLASSKEY: u32 = 0x3;
const SEE_MASK_IDLIST: u32 = 0x4;
const SEE_MASK_NOCLOSEPROCESS: u32 = 0x40;

/// ShellExecuteExW: as ShellExecuteW; TRUE, with the process handle in
/// hProcess when SEE_MASK_NOCLOSEPROCESS asks for it, else FALSE with the
/// last error and hInstApp set. Item ID lists and classes are not
/// supported (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn ShellExecuteExW(info: *mut ShellExecuteInfo) -> i32 {
    if info.is_null() {
        // SAFETY: plain call.
        unsafe { SetLastError(87) };
        return 0;
    }
    // SAFETY: the caller's structure.
    let info = unsafe { &mut *info };
    if info.size as usize != core::mem::size_of::<ShellExecuteInfo>() {
        // SAFETY: plain call.
        unsafe { SetLastError(87) };
        return 0;
    }
    if info.mask & (SEE_MASK_CLASSNAME | SEE_MASK_CLASSKEY | SEE_MASK_IDLIST) != 0 {
        record(b"shell32.dll!ShellExecuteExW(mask)", info.mask.into());
    }
    info.process = 0;
    match execute(info.verb, info.file, info.params, info.dir, info.show) {
        Ok(process) => {
            info.instance = 33;
            if info.mask & SEE_MASK_NOCLOSEPROCESS != 0 {
                info.process = process;
            } else {
                // SAFETY: the new process's handle, ours to close.
                unsafe { CloseHandle(process) };
            }
            1
        }
        Err((code, error)) => {
            info.instance = code;
            // SAFETY: plain call.
            unsafe { SetLastError(error) };
            0
        }
    }
}

// ---- Dropped files ----

/// DROPFILES: where the names start, the drop point, whether it was in
/// the non-client area, wide names.
#[repr(C)]
struct DropFiles {
    files: u32,
    x: i32,
    y: i32,
    non_client: i32,
    wide: i32,
}

/// The `index`-th name of a locked HDROP, as UTF-16 into `out` (its length;
/// None past the last name).
fn drop_name(drop: *const u8, index: u32, out: &mut [u16; 520]) -> Option<usize> {
    // SAFETY: the caller's HDROP memory, which starts with a DROPFILES.
    let header = unsafe { &*(drop as *const DropFiles) };
    // SAFETY: the names follow at `files`, double-NUL-terminated.
    let mut p = unsafe { drop.add(header.files as usize) };
    let mut i = 0;
    loop {
        if header.wide != 0 {
            let w = p as *const u16;
            let n = wlen(w);
            if n == 0 {
                return None;
            }
            if i == index {
                let n = n.min(out.len());
                // SAFETY: `n` characters of the name.
                out[..n].copy_from_slice(unsafe { core::slice::from_raw_parts(w, n) });
                return Some(n);
            }
            // SAFETY: past this name and its NUL.
            p = unsafe { p.add((n + 1) * 2) };
        } else {
            let mut n = 0;
            // SAFETY: an ANSI name, NUL-terminated.
            while unsafe { *p.add(n) } != 0 {
                n += 1;
            }
            if n == 0 {
                return None;
            }
            if i == index {
                // SAFETY: `n` bytes into our buffer of 520 characters.
                let m = unsafe { MultiByteToWideChar(0, 0, p, n as i32, out.as_mut_ptr(), 520) };
                return Some(m.max(0) as usize);
            }
            // SAFETY: past this name and its NUL.
            p = unsafe { p.add(n + 1) };
        }
        i += 1;
    }
}

/// DragQueryFileW: with index 0xFFFFFFFF the number of files; with a NULL
/// buffer the name's length; else the name copied (truncated to
/// `size` - 1 characters) and its copied length. 0 past the last file.
#[unsafe(no_mangle)]
pub extern "system" fn DragQueryFileW(
    drop: *mut c_void,
    index: u32,
    buf: *mut u16,
    size: u32,
) -> u32 {
    if drop.is_null() {
        return 0;
    }
    // SAFETY: the caller's HDROP.
    let p = unsafe { GlobalLock(drop) };
    if p.is_null() {
        return 0;
    }
    let mut name = [0u16; 520];
    let result = if index == u32::MAX {
        let mut count = 0;
        while drop_name(p, count, &mut name).is_some() {
            count += 1;
        }
        count
    } else {
        match drop_name(p, index, &mut name) {
            None => 0,
            Some(n) if buf.is_null() => n as u32,
            Some(_) if size == 0 => 0,
            Some(n) => {
                let copied = n.min(size as usize - 1);
                // SAFETY: the caller's buffer of `size` characters.
                unsafe {
                    core::ptr::copy_nonoverlapping(name.as_ptr(), buf, copied);
                    buf.add(copied).write(0);
                }
                copied as u32
            }
        }
    };
    // SAFETY: as locked above.
    unsafe { GlobalUnlock(drop) };
    result
}

/// DragQueryPoint: the drop point (client coordinates); TRUE if it was in
/// the client area.
#[unsafe(no_mangle)]
pub extern "system" fn DragQueryPoint(drop: *mut c_void, point: *mut [i32; 2]) -> i32 {
    if drop.is_null() || point.is_null() {
        return 0;
    }
    // SAFETY: the caller's HDROP.
    let p = unsafe { GlobalLock(drop) };
    if p.is_null() {
        return 0;
    }
    // SAFETY: it starts with a DROPFILES; the caller's POINT.
    let in_client = unsafe {
        let header = &*(p as *const DropFiles);
        point.write([header.x, header.y]);
        header.non_client == 0
    };
    // SAFETY: as locked above.
    unsafe { GlobalUnlock(drop) };
    i32::from(in_client)
}

/// DragFinish: frees the HDROP.
#[unsafe(no_mangle)]
pub extern "system" fn DragFinish(drop: *mut c_void) {
    if !drop.is_null() {
        // SAFETY: the caller's HDROP, a global memory block.
        unsafe { GlobalFree(drop) };
    }
}

/// The user32 function `name` (NUL-terminated), loading user32.
fn user32(name: &[u8]) -> usize {
    let dll: [u16; 11] = [
        b'u' as u16,
        b's' as u16,
        b'e' as u16,
        b'r' as u16,
        b'3' as u16,
        b'2' as u16,
        b'.' as u16,
        b'd' as u16,
        b'l' as u16,
        b'l' as u16,
        0,
    ];
    // SAFETY: NUL-terminated names.
    unsafe {
        let module = LoadLibraryW(dll.as_ptr());
        if module == 0 { 0 } else { GetProcAddress(module, name.as_ptr()) as usize }
    }
}

const GWL_EXSTYLE: i32 = -20;
const WS_EX_ACCEPTFILES: isize = 0x10;

/// DragAcceptFiles: sets or clears the window's WS_EX_ACCEPTFILES (which
/// makes it a drop target for WM_DROPFILES).
#[unsafe(no_mangle)]
pub extern "system" fn DragAcceptFiles(window: usize, accept: i32) {
    let (get, set) = (user32(b"GetWindowLongPtrW\0"), user32(b"SetWindowLongPtrW\0"));
    if get == 0 || set == 0 {
        record(b"shell32.dll!DragAcceptFiles(no user32)", 0);
        return;
    }
    // SAFETY: user32's documented signatures.
    let (get, set): (
        extern "system" fn(usize, i32) -> isize,
        extern "system" fn(usize, i32, isize) -> isize,
    ) = unsafe {
        (
            core::mem::transmute::<usize, extern "system" fn(usize, i32) -> isize>(get),
            core::mem::transmute::<usize, extern "system" fn(usize, i32, isize) -> isize>(set),
        )
    };
    let style = get(window, GWL_EXSTYLE);
    let new = if accept != 0 { style | WS_EX_ACCEPTFILES } else { style & !WS_EX_ACCEPTFILES };
    if new != style {
        set(window, GWL_EXSTYLE, new);
    }
}

// ---- Creating folders ----

const ERROR_FILE_EXISTS: u32 = 80;
const ERROR_BAD_PATHNAME: u32 = 161;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_FILENAME_EXCED_RANGE: u32 = 206;
const MAX_PATH: usize = 260;

/// A path shlwapi's PathIsRelativeW calls relative: not `\...` (rooted
/// or UNC) and not `X:...`.
fn is_relative(p: &[u16]) -> bool {
    !(p.first() == Some(&u16::from(b'\\')) || p.get(1) == Some(&u16::from(b':')))
}

/// How many characters of `p` are its root, which is never created: `X:\`,
/// `\\server\share\`, or `\`.
fn root_len(p: &[u16]) -> usize {
    let sep = u16::from(b'\\');
    if p.starts_with(&[sep, sep]) {
        // Past the server and the share.
        let mut seps = 0;
        for (i, &c) in p.iter().enumerate().skip(2) {
            if c == sep {
                seps += 1;
                if seps == 2 {
                    return i + 1;
                }
            }
        }
        return p.len();
    }
    if p.get(1) == Some(&u16::from(b':')) {
        return if p.get(2) == Some(&sep) { 3 } else { 2 };
    }
    usize::from(p.first() == Some(&sep))
}

/// CreateDirectoryW of the first `n` characters of `buf` (NUL-terminated
/// there for the call): 0 or its error.
fn create_dir(buf: &mut [u16; MAX_PATH + 1], n: usize, security: *const c_void) -> u32 {
    let kept = buf[n];
    buf[n] = 0;
    // SAFETY: a NUL-terminated path; the caller's SECURITY_ATTRIBUTES or NULL.
    let ok = unsafe { CreateDirectoryW(buf.as_ptr(), security) } != 0;
    buf[n] = kept;
    // SAFETY: plain call.
    if ok { 0 } else { unsafe { GetLastError() } }
}

/// SHCreateDirectoryExW(hwnd, pszPath, psa): creates the folder, and
/// every missing folder above it (those with no security attributes, the
/// last with `psa`), as Microsoft's SHCreateDirectoryEx page documents:
/// ERROR_SUCCESS; ERROR_BAD_PATHNAME for a relative path (nothing made);
/// ERROR_ALREADY_EXISTS for an existing folder; ERROR_FILENAME_EXCED_RANGE
/// for a path of MAX_PATH characters or more; else CreateDirectoryW's
/// error for the folder itself. With a window, Windows reports a failure
/// in a message box and then answers ERROR_CANCELLED; gala shows none
/// and answers the error itself (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn SHCreateDirectoryExW(
    window: usize,
    path: *const u16,
    security: *const c_void,
) -> i32 {
    let p = chars(path);
    let error = if is_relative(p) {
        ERROR_BAD_PATHNAME
    } else if p.len() >= MAX_PATH {
        ERROR_FILENAME_EXCED_RANGE
    } else {
        let mut buf = [0u16; MAX_PATH + 1];
        buf[..p.len()].copy_from_slice(p);
        let first = create_dir(&mut buf, p.len(), security);
        if matches!(
            first,
            0 | ERROR_FILE_EXISTS | ERROR_ALREADY_EXISTS | ERROR_FILENAME_EXCED_RANGE
        ) {
            first
        } else {
            // A folder above is missing: make each one below the root
            // (an existing one is fine), then the folder itself.
            let sep = u16::from(b'\\');
            let end = p.len() - usize::from(p.last() == Some(&sep));
            for i in root_len(p)..end {
                if p[i] == sep && i > 0 && p[i - 1] != sep {
                    create_dir(&mut buf, i, core::ptr::null());
                }
            }
            create_dir(&mut buf, p.len(), security)
        }
    };
    if error != 0 {
        if window != 0 && error != ERROR_ALREADY_EXISTS && error != ERROR_CANCELLED {
            record(b"shell32.dll!SHCreateDirectoryExW(message box for a failure)", error.into());
        }
        // SAFETY: plain call.
        unsafe { SetLastError(error) };
    }
    error as i32
}

/// SHCreateDirectory(hwnd, pszPath), shell32's ordinal 165 (build.rs; App-V
/// imports it by that number): SHCreateDirectoryExW without security
/// attributes, as Microsoft's SHCreateDirectory page documents it.
#[unsafe(no_mangle)]
pub extern "system" fn SHCreateDirectory(window: usize, path: *const u16) -> i32 {
    SHCreateDirectoryExW(window, path, core::ptr::null())
}

// ---- Notifications nothing receives in gala ----

/// SHChangeNotify: tells the shell about a change; gala has no shell
/// views or change listeners, so there is nobody to tell.
#[unsafe(no_mangle)]
pub extern "system" fn SHChangeNotify(
    _event: i32,
    _flags: u32,
    _item1: *const c_void,
    _item2: *const c_void,
) {
}

/// The icon paths SHGetIconOverlayIndexW has handed indexes for (a hash
/// of each, case folded; 0 = free), the index being FIRST_OVERLAY plus
/// the slot.
static OVERLAYS: [core::sync::atomic::AtomicU64; 32] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 32];
const FIRST_OVERLAY: i32 = 5;

/// SHGetIconOverlayIndexW(pszIconPath, iIconIndex): an overlay index for
/// the path, whether or not the file exists: the same for the same path,
/// the next free one for a new path (Windows 11 25H2 measured 5 for a
/// missing path three times, then 6 for another; -1 is documented for
/// "not found"). -1 once gala's table is full (recorded).
#[unsafe(no_mangle)]
pub extern "system" fn SHGetIconOverlayIndexW(path: *const u16, index: i32) -> i32 {
    use core::sync::atomic::Ordering;
    if path.is_null() {
        return -1;
    }
    // FNV-1a over the path (ASCII case folded) and the icon index; never 0.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    loop {
        // SAFETY: the caller's NUL-terminated string.
        let c = unsafe { *path.add(i) };
        if c == 0 {
            break;
        }
        let c = if (0x41..=0x5a).contains(&c) { c | 0x20 } else { c };
        h = (h ^ u64::from(c)).wrapping_mul(0x100_0000_01b3);
        i += 1;
    }
    h = (h ^ index as u32 as u64).wrapping_mul(0x100_0000_01b3) | 1;
    for (slot, entry) in OVERLAYS.iter().enumerate() {
        match entry.compare_exchange(0, h, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return FIRST_OVERLAY + slot as i32,
            Err(seen) if seen == h => return FIRST_OVERLAY + slot as i32,
            Err(_) => {}
        }
    }
    record(b"shell32.dll!SHGetIconOverlayIndexW(table full)", OVERLAYS.len() as u64);
    -1
}

// ---- File operations ----

#[repr(C)]
pub struct FindData {
    pub(crate) attributes: u32,
    pub(crate) times: [u32; 6],
    pub(crate) size_high: u32,
    pub(crate) size_low: u32,
    pub(crate) reserved: [u32; 2],
    pub(crate) name: [u16; 260],
    pub(crate) short_name: [u16; 14],
}

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const INVALID: usize = usize::MAX;

/// SHFILEOPSTRUCTW (x64 layout).
#[repr(C)]
pub struct FileOp {
    window: usize,
    func: u32,
    from: *const u16,
    to: *const u16,
    flags: u16,
    aborted: i32,
    mappings: *mut c_void,
    title: *const u16,
}

const FO_MOVE: u32 = 1;
const FO_COPY: u32 = 2;
const FO_DELETE: u32 = 3;
const FO_RENAME: u32 = 4;
const FOF_MULTIDESTFILES: u16 = 0x1;
const FOF_NOCONFIRMATION: u16 = 0x10;
const FOF_ALLOWUNDO: u16 = 0x40;
const FOF_FILESONLY: u16 = 0x80;
const FOF_RENAMEONCOLLISION: u16 = 0x8;
const FOF_NORECURSION: u16 = 0x1000;

/// The names of a double-NUL-terminated list.
fn list<'a>(p: *const u16) -> impl Iterator<Item = &'a [u16]> {
    let mut at = p;
    core::iter::from_fn(move || {
        if at.is_null() {
            return None;
        }
        let s = chars(at);
        if s.is_empty() {
            return None;
        }
        at = at.wrapping_add(s.len() + 1);
        Some(s)
    })
}

fn attributes(p: &Path) -> u32 {
    // SAFETY: a NUL-terminated path.
    unsafe { GetFileAttributesW(p.ptr()) }
}

fn is_dir(p: &Path) -> bool {
    let a = attributes(p);
    a != u32::MAX && a & FILE_ATTRIBUTE_DIRECTORY != 0
}

fn last_error() -> u32 {
    // SAFETY: plain call.
    unsafe { GetLastError() }
}

/// Calls `f` with each entry (not `.` or `..`) of the directory pattern
/// `pattern` and its attributes; the first error `f` returns.
fn each(pattern: &Path, mut f: impl FnMut(&[u16], u32) -> Result<(), u32>) -> Result<(), u32> {
    // SAFETY: zeroed plain data.
    let mut data: FindData = unsafe { core::mem::zeroed() };
    // SAFETY: a NUL-terminated pattern and our structure.
    let h = unsafe { FindFirstFileW(pattern.ptr(), &mut data) };
    if h == INVALID {
        return Err(last_error());
    }
    let mut result = Ok(());
    loop {
        let n = data.name.iter().position(|&c| c == 0).unwrap_or(260);
        let name = &data.name[..n];
        let dots = name == [u16::from(b'.')] || name == [u16::from(b'.'), u16::from(b'.')];
        if !dots {
            result = f(name, data.attributes);
            if result.is_err() {
                break;
            }
        }
        // SAFETY: the search handle and our structure.
        if unsafe { FindNextFileW(h, &mut data) } == 0 {
            break;
        }
    }
    // SAFETY: the search handle.
    unsafe { FindClose(h) };
    result
}

fn star() -> [u16; 1] {
    [u16::from(b'*')]
}

/// Deletes a file, or a directory and everything in it.
fn delete(p: &Path) -> Result<(), u32> {
    if is_dir(p) {
        let pattern = p.join(&star()).ok_or(206u32)?;
        let r = each(&pattern, |name, _| delete(&p.join(name).ok_or(206u32)?));
        if let Err(e) = r
            && e != ERROR_FILE_NOT_FOUND
            && e != 18
        {
            return Err(e);
        }
        // SAFETY: a NUL-terminated path.
        if unsafe { RemoveDirectoryW(p.ptr()) } == 0 {
            return Err(last_error());
        }
    } else {
        // SAFETY: as above.
        if unsafe { DeleteFileW(p.ptr()) } == 0 {
            return Err(last_error());
        }
    }
    Ok(())
}

/// Copies a file, or a directory with everything in it, to `to`.
fn copy(from: &Path, to: &Path, recurse: bool) -> Result<(), u32> {
    if !is_dir(from) {
        // SAFETY: NUL-terminated paths; overwriting (no confirmation UI).
        if unsafe { CopyFileW(from.ptr(), to.ptr(), 0) } == 0 {
            return Err(last_error());
        }
        return Ok(());
    }
    // SAFETY: as above.
    if unsafe { CreateDirectoryW(to.ptr(), core::ptr::null()) } == 0 && !is_dir(to) {
        return Err(last_error());
    }
    let pattern = from.join(&star()).ok_or(206u32)?;
    let r = each(&pattern, |name, attrs| {
        if attrs & FILE_ATTRIBUTE_DIRECTORY != 0 && !recurse {
            return Ok(());
        }
        copy(&from.join(name).ok_or(206u32)?, &to.join(name).ok_or(206u32)?, recurse)
    });
    match r {
        Err(e) if e != ERROR_FILE_NOT_FOUND && e != 18 => Err(e),
        _ => Ok(()),
    }
}

/// The last component of `s`.
fn base(s: &[u16]) -> &[u16] {
    s.rsplit(|&c| c == u16::from(b'\\')).next().unwrap_or(s)
}

/// The directory part of `s` (without its last component), for a
/// wildcard source's matches.
fn parent(s: &[u16]) -> &[u16] {
    match s.iter().rposition(|&c| c == u16::from(b'\\')) {
        Some(i) => &s[..i],
        None => &[],
    }
}

fn wild(s: &[u16]) -> bool {
    s.iter().any(|&c| c == u16::from(b'*') || c == u16::from(b'?'))
}

/// One operation on one (expanded) source `from` to `to`.
fn operate(func: u32, from: &Path, to: Option<&Path>, flags: u16) -> Result<(), u32> {
    match (func, to) {
        (FO_DELETE, _) => delete(from),
        (FO_COPY, Some(to)) => copy(from, to, flags & FOF_NORECURSION == 0),
        // SAFETY: NUL-terminated paths. MOVEFILE_COPY_ALLOWED (2):
        // across volumes too.
        (FO_MOVE, Some(to)) => match unsafe { MoveFileExW(from.ptr(), to.ptr(), 2) } {
            0 => Err(last_error()),
            _ => Ok(()),
        },
        (FO_RENAME, Some(to)) => {
            if attributes(to) != u32::MAX {
                return Err(183); // ERROR_ALREADY_EXISTS
            }
            // SAFETY: as above.
            match unsafe { MoveFileExW(from.ptr(), to.ptr(), 0) } {
                0 => Err(last_error()),
                _ => Ok(()),
            }
        }
        _ => Err(87),
    }
}

fn file_operation(op: &mut FileOp) -> Result<(), u32> {
    if !matches!(op.func, FO_MOVE | FO_COPY | FO_DELETE | FO_RENAME) || op.from.is_null() {
        return Err(87);
    }
    if op.flags & FOF_NOCONFIRMATION == 0 {
        // Windows would ask; gala has no dialogs and goes ahead.
        record(b"shell32.dll!SHFileOperationW(confirmation)", op.flags.into());
    }
    if op.flags & (FOF_ALLOWUNDO | FOF_RENAMEONCOLLISION) != 0 {
        // No recycle bin, no renaming on collision: deleted and
        // overwritten for good.
        record(b"shell32.dll!SHFileOperationW(flags)", op.flags.into());
    }
    let sources = list(op.from).count();
    let destinations: [Option<&[u16]>; 1] = [list(op.to).next()];
    let multi = op.flags & FOF_MULTIDESTFILES != 0;
    if op.func != FO_DELETE && destinations[0].is_none() {
        return Err(87);
    }
    let dest_dir = destinations[0].and_then(Path::from);
    // The destination is a directory to put things in when it is one, or
    // when there are several sources (then it is made).
    let into_dir = op.func != FO_RENAME
        && !multi
        && dest_dir.as_ref().is_some_and(|d| is_dir(d) || sources > 1);
    if into_dir && sources > 1 {
        let d = dest_dir.as_ref().ok_or(87u32)?;
        // SAFETY: a NUL-terminated path.
        if !is_dir(d) && unsafe { CreateDirectoryW(d.ptr(), core::ptr::null()) } == 0 {
            return Err(last_error());
        }
    }
    let mut targets = list(op.to);
    for source in list(op.from) {
        let target = if multi { targets.next() } else { destinations[0] };
        let target = target.and_then(Path::from);
        if op.func != FO_DELETE && target.is_none() {
            return Err(87);
        }
        let destination = |name: &[u16]| -> Option<Path> {
            let t = target.as_ref()?;
            if into_dir { t.join(name) } else { Path::from(t.text()) }
        };
        if wild(source) && op.func != FO_RENAME {
            let pattern = Path::from(source).ok_or(206u32)?;
            let dir = Path::from(parent(source)).ok_or(206u32)?;
            let mut matched = false;
            each(&pattern, |name, attrs| {
                if attrs & FILE_ATTRIBUTE_DIRECTORY != 0 && op.flags & FOF_FILESONLY != 0 {
                    return Ok(());
                }
                matched = true;
                let from = dir.join(name).ok_or(206u32)?;
                let to = match &target {
                    Some(t) => Some(t.join(name).ok_or(206u32)?),
                    None => None,
                };
                if let Some(t) = &target {
                    // Matches go into the destination directory.
                    // SAFETY: a NUL-terminated path.
                    if !is_dir(t) && unsafe { CreateDirectoryW(t.ptr(), core::ptr::null()) } == 0 {
                        return Err(last_error());
                    }
                }
                operate(op.func, &from, to.as_ref(), op.flags)
            })?;
            if !matched {
                return Err(ERROR_FILE_NOT_FOUND);
            }
            continue;
        }
        let from = Path::from(source).ok_or(206u32)?;
        if attributes(&from) == u32::MAX {
            return Err(ERROR_FILE_NOT_FOUND);
        }
        let to = destination(base(source));
        operate(op.func, &from, to.as_ref(), op.flags)?;
    }
    Ok(())
}

/// SHFileOperationW: copies, moves, renames or deletes the files and
/// directories in the double-NUL-terminated pFrom (wildcards allowed) to
/// pTo; directories with what is in them. 0 on success, else an error
/// code (Win32's; Microsoft documents that the old DE_* values must not
/// be relied on). gala has no dialogs and no recycle bin: confirmation
/// and FOF_ALLOWUNDO are recorded, and deletes are permanent.
#[unsafe(no_mangle)]
pub extern "system" fn SHFileOperationW(op: *mut FileOp) -> i32 {
    if op.is_null() {
        return 87;
    }
    // SAFETY: the caller's structure.
    let op = unsafe { &mut *op };
    op.aborted = 0;
    match file_operation(op) {
        Ok(()) => 0,
        Err(e) => e as i32,
    }
}
