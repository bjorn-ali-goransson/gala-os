//! What wxWidgets imports from shell32 beyond folders: running programs
//! (ShellExecuteW/ShellExecuteExW), dropped files (DragQueryFileW and
//! friends on an HDROP), file operations (SHFileOperationW), and the
//! notifications gala has no receiver for (SHChangeNotify,
//! Shell_NotifyIconW, SHGetIconOverlayIndexW). As documented; see
//! user/shell32_test.
//!
//! gala has no file associations or shell verbs: ShellExecute runs
//! programs (.exe and .com, found as CreateProcess and the search path
//! find them) and answers SE_ERR_NOASSOC for documents and URLs
//! (recorded). user32 is reached at run time (DragAcceptFiles), so
//! loading shell32 does not load the window manager.

#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use core::ffi::c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
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
const SE_ERR_FNF: usize = 2;
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
    if !verb.is_empty() && !ascii_eq(verb, b"open") {
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
        // A document or a URL: no file associations in gala yet.
        record(b"shell32.dll!ShellExecute(association)", 0);
        return Err((SE_ERR_NOASSOC, ERROR_NO_ASSOCIATION));
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

/// ShellExecuteW: runs a program; a value over 32 on success, else
/// SE_ERR_FNF (2) for a file that is not there and SE_ERR_NOASSOC (31)
/// for a document or URL (gala has no associations; recorded).
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

/// Shell_NotifyIconW: gala's taskbar has no notification area yet, so
/// the icon is not added (FALSE, recorded with the message).
#[unsafe(no_mangle)]
pub extern "system" fn Shell_NotifyIconW(message: u32, _data: *const c_void) -> i32 {
    record(b"shell32.dll!Shell_NotifyIconW(no notification area)", message.into());
    0
}

/// SHGetIconOverlayIndexW: no overlay icons are registered (-1, as
/// documented for an overlay that is not found).
#[unsafe(no_mangle)]
pub extern "system" fn SHGetIconOverlayIndexW(_path: *const u16, _index: i32) -> i32 {
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
