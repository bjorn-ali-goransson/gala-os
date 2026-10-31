//! The POSIX-style layer of msvcrt: files and directories by name, the
//! current directory, the environment (`_putenv`), temporary names, and
//! `_spawnve` (child processes; busybox's shell runs pipelines with it).
//! Paths are UTF-8 (the ANSI code page here).

use crate::heap::malloc;
use core::ffi::c_void;
use core::ptr::null_mut;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn CreateDirectoryW(path: *const u16, attributes: *mut c_void) -> i32;
    fn RemoveDirectoryW(path: *const u16) -> i32;
    fn DeleteFileW(path: *const u16) -> i32;
    fn MoveFileW(from: *const u16, to: *const u16) -> i32;
    fn GetCurrentDirectoryW(size: u32, buf: *mut u16) -> u32;
    fn GetFullPathNameW(path: *const u16, size: u32, buf: *mut u16, part: *mut *mut u16) -> u32;
    fn GetCurrentProcessId() -> u32;
    fn GetCurrentProcess() -> *mut c_void;
    fn SetEnvironmentVariableW(name: *const u16, value: *const u16) -> i32;
    fn CreateProcessA(
        application: *const u8,
        command_line: *mut u8,
        process_attributes: *mut c_void,
        thread_attributes: *mut c_void,
        inherit: i32,
        flags: u32,
        environment: *mut c_void,
        directory: *const u8,
        startup: *mut u8,
        info: *mut usize,
    ) -> i32;
    fn WaitForSingleObject(h: *mut c_void, ms: u32) -> u32;
    fn GetExitCodeProcess(h: *mut c_void, code: *mut u32) -> i32;
    fn CloseHandle(h: *mut c_void) -> i32;
    fn GetStdHandle(which: u32) -> *mut c_void;
    fn ReadFile(h: *mut c_void, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> i32;
    fn GetProcessTimes(
        process: *mut c_void,
        creation: *mut u64,
        exit: *mut u64,
        kernel: *mut u64,
        user: *mut u64,
    ) -> i32;
    fn GetSystemTimeAsFileTime(time: *mut u64);
    fn GetLastError() -> u32;
    fn SearchPathA(
        path: *const u8,
        file: *const u8,
        extension: *const u8,
        size: u32,
        buf: *mut u8,
        file_part: *mut *mut u8,
    ) -> u32;
    fn SetCurrentDirectoryW(path: *const u16) -> i32;
    fn WriteFile(
        h: *mut c_void,
        buf: *const u8,
        len: u32,
        written: *mut u32,
        ov: *mut c_void,
    ) -> i32;
}

/// Characters of a path buffer (on the stack: several must fit in 4 KiB,
/// as there is no __chkstk here).
const PATH: usize = 520;

const ENOENT: i32 = 2;
const EACCES: i32 = 13;
const EEXIST: i32 = 17;
const EINVAL: i32 = 22;
const ENOMEM: i32 = 12;

fn set_errno(e: i32) {
    // SAFETY: _errno() points to the process's errno.
    unsafe { *crate::stdio::_errno() = e };
}

/// errno for the last Win32 error.
fn errno_now() -> i32 {
    // SAFETY: plain call.
    match unsafe { GetLastError() } {
        2 | 3 => ENOENT,
        5 | 32 | 145 => EACCES,
        80 | 183 => EEXIST,
        8 | 14 => ENOMEM,
        _ => EINVAL,
    }
}

/// A NUL-terminated UTF-8 path as NUL-terminated UTF-16.
fn wide(path: *const u8, out: &mut [u16; PATH]) -> Option<*const u16> {
    if path.is_null() {
        return None;
    }
    // SAFETY: the caller's NUL-terminated string.
    let text = core::str::from_utf8(unsafe { crate::c_str(path) }).ok()?;
    let mut n = 0;
    for c in text.encode_utf16() {
        *out.get_mut(n)? = c;
        n += 1;
    }
    *out.get_mut(n)? = 0;
    Some(out.as_ptr())
}

/// UTF-16 `w` to UTF-8 at `out` (`size` bytes, NUL added), or None.
fn narrow(w: &[u16], out: *mut u8, size: usize) -> Option<usize> {
    let mut at = 0;
    for c in char::decode_utf16(w.iter().copied()) {
        let mut b = [0u8; 4];
        let e = c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut b);
        if at + e.len() + 1 > size {
            return None;
        }
        // SAFETY: within the caller's buffer of `size` bytes.
        unsafe { core::ptr::copy_nonoverlapping(e.as_ptr(), out.add(at), e.len()) };
        at += e.len();
    }
    // SAFETY: at < size.
    unsafe { *out.add(at) = 0 };
    Some(at)
}

/// Runs a Win32 call on the UTF-16 form of `path`: 0, or -1 with errno.
fn by_path(path: *const u8, f: impl FnOnce(*const u16) -> bool) -> i32 {
    let mut buf = [0u16; PATH];
    let Some(p) = wide(path, &mut buf) else {
        set_errno(ENOENT);
        return -1;
    };
    if f(p) {
        0
    } else {
        set_errno(errno_now());
        -1
    }
}

/// _access(path, mode): 0 if it exists (and, for mode 2, is writable).
#[unsafe(no_mangle)]
pub extern "C" fn _access(path: *const u8, mode: i32) -> i32 {
    let mut buf = [0u16; PATH];
    let Some(p) = wide(path, &mut buf) else {
        set_errno(ENOENT);
        return -1;
    };
    // SAFETY: a NUL-terminated path.
    let attributes = unsafe { GetFileAttributesW(p) };
    if attributes == u32::MAX {
        set_errno(errno_now());
        return -1;
    }
    if mode & 2 != 0 && attributes & 1 != 0 {
        set_errno(EACCES); // FILE_ATTRIBUTE_READONLY
        return -1;
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn _mkdir(path: *const u8) -> i32 {
    // SAFETY: a NUL-terminated path.
    by_path(path, |p| unsafe { CreateDirectoryW(p, null_mut()) } != 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn _rmdir(path: *const u8) -> i32 {
    // SAFETY: a NUL-terminated path.
    by_path(path, |p| unsafe { RemoveDirectoryW(p) } != 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn _unlink(path: *const u8) -> i32 {
    // SAFETY: a NUL-terminated path.
    by_path(path, |p| unsafe { DeleteFileW(p) } != 0)
}

/// remove(path): a file, as _unlink.
#[unsafe(no_mangle)]
pub extern "C" fn remove(path: *const u8) -> i32 {
    _unlink(path)
}

/// rename(from, to): fails if `to` exists (as msvcrt's).
#[unsafe(no_mangle)]
pub extern "C" fn rename(from: *const u8, to: *const u8) -> i32 {
    let mut b = [0u16; PATH];
    let Some(t) = wide(to, &mut b) else {
        set_errno(ENOENT);
        return -1;
    };
    // SAFETY: NUL-terminated paths.
    by_path(from, |f| unsafe { MoveFileW(f, t) } != 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn _getpid() -> i32 {
    // SAFETY: plain call.
    unsafe { GetCurrentProcessId() as i32 }
}

static mut UMASK: i32 = 0;

/// _umask(mode): sets the mask; returns the previous one.
#[unsafe(no_mangle)]
pub extern "C" fn _umask(mode: i32) -> i32 {
    // SAFETY: a plain integer.
    unsafe { core::mem::replace(&mut *(&raw mut UMASK), mode) }
}

/// _getcwd(buf, size): with a null buffer, a malloc'ed one (at least
/// `size` bytes).
#[unsafe(no_mangle)]
pub extern "C" fn _getcwd(buf: *mut u8, size: i32) -> *mut u8 {
    let mut w = [0u16; PATH];
    // SAFETY: a buffer of PATH characters.
    let n = unsafe { GetCurrentDirectoryW(PATH as u32, w.as_mut_ptr()) } as usize;
    if n == 0 || n >= PATH {
        set_errno(errno_now());
        return null_mut();
    }
    let (out, cap) = if buf.is_null() {
        let cap = (size.max(0) as usize).max(4 * n + 1);
        (malloc(cap), cap)
    } else {
        (buf, size.max(0) as usize)
    };
    if out.is_null() {
        set_errno(ENOMEM);
        return null_mut();
    }
    if narrow(&w[..n], out, cap).is_none() {
        set_errno(34); // ERANGE
        return null_mut();
    }
    out
}

/// _fullpath(abs, rel, size): GetFullPathName; a malloc'ed buffer for null.
#[unsafe(no_mangle)]
pub extern "C" fn _fullpath(abs: *mut u8, rel: *const u8, size: usize) -> *mut u8 {
    let mut b = [0u16; PATH];
    let Some(p) = wide(rel, &mut b) else {
        set_errno(EINVAL);
        return null_mut();
    };
    let mut w = [0u16; PATH];
    // SAFETY: a NUL-terminated path, a PATH-character buffer.
    let n = unsafe { GetFullPathNameW(p, PATH as u32, w.as_mut_ptr(), null_mut()) } as usize;
    if n == 0 || n >= PATH {
        set_errno(EINVAL);
        return null_mut();
    }
    let (out, cap) = if abs.is_null() { (malloc(4 * n + 1), 4 * n + 1) } else { (abs, size) };
    if out.is_null() || narrow(&w[..n], out, cap).is_none() {
        set_errno(34); // ERANGE
        return null_mut();
    }
    out
}

/// _mktemp(template): replaces the trailing "XXXXXX" with a letter and the
/// process id (as msvcrt) so that no such file exists.
#[unsafe(no_mangle)]
pub extern "C" fn _mktemp(template: *mut u8) -> *mut u8 {
    // SAFETY: the caller's NUL-terminated, writable template.
    let t = unsafe { core::slice::from_raw_parts_mut(template, crate::strlen(template)) };
    if t.len() < 6 || t[t.len() - 6..].iter().any(|&c| c != b'X') {
        set_errno(EINVAL);
        return null_mut();
    }
    let start = t.len() - 6;
    let mut pid = _getpid() as u32;
    for i in (start + 1..t.len()).rev() {
        t[i] = b'0' + (pid % 10) as u8;
        pid /= 10;
    }
    for letter in b'a'..=b'z' {
        t[start] = letter;
        if _access(template, 0) != 0 {
            return template;
        }
    }
    set_errno(EEXIST);
    null_mut()
}

/// _putenv("NAME=value"; "NAME=" removes it): the CRT's `_environ` (what
/// getenv reads) and the process environment (what children inherit).
#[unsafe(no_mangle)]
pub extern "C" fn _putenv(entry: *const u8) -> i32 {
    // SAFETY: the caller's NUL-terminated string.
    let text = unsafe { crate::c_str(entry) };
    let Some(eq) = text.iter().position(|&c| c == b'=').filter(|&i| i > 0) else {
        set_errno(EINVAL);
        return -1;
    };
    let (name, value) = (&text[..eq], &text[eq + 1..]);
    // The process environment.
    let (mut wn, mut wv) = ([0u16; PATH], [0u16; PATH]);
    let mut n = 0;
    for c in core::str::from_utf8(name).unwrap_or("").encode_utf16() {
        if n < PATH - 1 {
            wn[n] = c;
            n += 1;
        }
    }
    let mut v = 0;
    for c in core::str::from_utf8(value).unwrap_or("").encode_utf16() {
        if v < PATH - 1 {
            wv[v] = c;
            v += 1;
        }
    }
    // SAFETY: NUL-terminated strings (or null to remove).
    unsafe {
        SetEnvironmentVariableW(
            wn.as_ptr(),
            if value.is_empty() { null_mut() } else { wv.as_ptr() },
        )
    };
    // The CRT's array: replace, remove or append (the strings are copies).
    let copy = malloc(text.len() + 1);
    if copy.is_null() {
        set_errno(ENOMEM);
        return -1;
    }
    // SAFETY: a fresh buffer; the environment array is the CRT's own,
    // NULL-terminated, and reallocated here.
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), copy, text.len());
        *copy.add(text.len()) = 0;
        let env = crate::_environ;
        let mut count = 0;
        let mut found = None;
        while !env.is_null() && !(*env.add(count)).is_null() {
            let e = crate::c_str(*env.add(count));
            if e.len() > eq && e[eq] == b'=' && e[..eq].eq_ignore_ascii_case(name) {
                found = Some(count);
            }
            count += 1;
        }
        match (found, value.is_empty()) {
            (Some(i), false) => *env.add(i) = copy,
            (Some(i), true) => {
                for k in i..count {
                    *env.add(k) = *env.add(k + 1);
                }
            }
            (None, true) => {}
            (None, false) => {
                let grown = malloc((count + 2) * 8).cast::<*mut u8>();
                if grown.is_null() {
                    set_errno(ENOMEM);
                    return -1;
                }
                for k in 0..count {
                    *grown.add(k) = *env.add(k);
                }
                *grown.add(count) = copy;
                *grown.add(count + 1) = null_mut();
                crate::_environ = grown;
            }
        }
    }
    0
}

/// _getch: one byte from standard input (no echo; gala's console has no
/// line editing to bypass).
#[unsafe(no_mangle)]
pub extern "C" fn _getch() -> i32 {
    let mut b = 0u8;
    let mut got = 0;
    // SAFETY: a one-byte buffer.
    let ok = unsafe { ReadFile(GetStdHandle(-10i32 as u32), &mut b, 1, &mut got, null_mut()) };
    if ok == 0 || got == 0 { -1 } else { i32::from(b) }
}

/// clock(): milliseconds since the process started (CLOCKS_PER_SEC 1000).
#[unsafe(no_mangle)]
pub extern "C" fn clock() -> i32 {
    let (mut created, mut x, mut now) = (0u64, 0u64, 0u64);
    // SAFETY: valid output pointers.
    unsafe {
        if GetProcessTimes(GetCurrentProcess(), &mut created, &mut x, &mut x, &mut x) == 0 {
            return -1;
        }
        GetSystemTimeAsFileTime(&mut now);
    }
    (now.saturating_sub(created) / 10_000) as i32
}

const P_WAIT: i32 = 0;
const P_NOWAIT: i32 = 1;
const P_OVERLAY: i32 = 2;
const P_NOWAITO: i32 = 3;

/// _spawnve(mode, cmdname, argv, envp): msvcrt's semantics. The command
/// line is the arguments joined by spaces (msvcrt does not quote them:
/// callers do), `envp` (or null: ours) the environment; the child gets
/// our standard handles and inheritable handles. P_WAIT returns the exit
/// code, P_NOWAIT/P_NOWAITO the process handle, P_OVERLAY waits and ends
/// this process with the child's exit code.
#[unsafe(no_mangle)]
pub extern "C" fn _spawnve(
    mode: i32,
    cmdname: *const u8,
    argv: *const *const u8,
    envp: *const *const u8,
) -> isize {
    if cmdname.is_null() || argv.is_null() {
        set_errno(EINVAL);
        return -1;
    }
    // SAFETY: the caller's NULL-terminated arrays of NUL-terminated strings.
    let (line, env) = unsafe {
        let mut len = 1;
        let mut i = 0;
        while !(*argv.add(i)).is_null() {
            len += crate::strlen(*argv.add(i)) + 1;
            i += 1;
        }
        let line = malloc(len);
        if line.is_null() {
            set_errno(ENOMEM);
            return -1;
        }
        let mut at = 0;
        for k in 0..i {
            let s = crate::c_str(*argv.add(k));
            if k > 0 {
                *line.add(at) = b' ';
                at += 1;
            }
            core::ptr::copy_nonoverlapping(s.as_ptr(), line.add(at), s.len());
            at += s.len();
        }
        *line.add(at) = 0;
        // NAME=value\0...\0\0 (ANSI: UTF-8 here).
        let env = if envp.is_null() {
            null_mut()
        } else {
            let mut total = 1;
            let mut n = 0;
            while !(*envp.add(n)).is_null() {
                total += crate::strlen(*envp.add(n)) + 1;
                n += 1;
            }
            let block = malloc(total + 1);
            if block.is_null() {
                set_errno(ENOMEM);
                return -1;
            }
            let mut at = 0;
            for k in 0..n {
                let s = crate::c_str(*envp.add(k));
                core::ptr::copy_nonoverlapping(s.as_ptr(), block.add(at), s.len());
                at += s.len();
                *block.add(at) = 0;
                at += 1;
            }
            *block.add(at) = 0;
            *block.add(at + 1) = 0;
            block
        };
        (line, env)
    };
    // STARTUPINFOA with our standard handles (STARTF_USESTDHANDLES) and our
    // other descriptors (cbReserved2/lpReserved2, see stdio.rs).
    let mut startup = [0u8; 104];
    startup[0..4].copy_from_slice(&104u32.to_le_bytes());
    let mut block = [0u8; 4 + 9 * crate::stdio::MAX_FDS];
    let block_len = crate::stdio::inheritance_block(&mut block);
    if block_len != 0 {
        startup[66..68].copy_from_slice(&(block_len as u16).to_le_bytes());
        startup[72..80].copy_from_slice(&(block.as_ptr() as u64).to_le_bytes());
    }
    startup[60..64].copy_from_slice(&0x100u32.to_le_bytes());
    for (i, at) in [80usize, 88, 96].into_iter().enumerate() {
        // SAFETY: plain call.
        let h = unsafe { GetStdHandle((-10 - i as i32) as u32) } as u64;
        startup[at..at + 8].copy_from_slice(&h.to_le_bytes());
    }
    let mut info = [0usize; 3];
    // SAFETY: valid strings, blocks and structures.
    let ok = unsafe {
        CreateProcessA(
            cmdname,
            line,
            null_mut(),
            null_mut(),
            1,
            0,
            env.cast(),
            core::ptr::null(),
            startup.as_mut_ptr(),
            info.as_mut_ptr(),
        )
    };
    crate::heap::free(line);
    if !env.is_null() {
        crate::heap::free(env);
    }
    if ok == 0 {
        set_errno(errno_now());
        return -1;
    }
    let (process, thread) = (info[0] as *mut c_void, info[1] as *mut c_void);
    // SAFETY: our process and thread handles.
    unsafe {
        CloseHandle(thread);
        match mode {
            P_NOWAIT | P_NOWAITO => process as isize,
            P_WAIT | P_OVERLAY => {
                WaitForSingleObject(process, u32::MAX);
                let mut code = 0u32;
                GetExitCodeProcess(process, &mut code);
                CloseHandle(process);
                if mode == P_OVERLAY {
                    crate::exit(code as i32);
                }
                code as isize
            }
            _ => {
                CloseHandle(process);
                set_errno(EINVAL);
                -1
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _chdir(path: *const u8) -> i32 {
    // SAFETY: a NUL-terminated path.
    by_path(path, |p| unsafe { SetCurrentDirectoryW(p) } != 0)
}

/// _chmod(path, mode): permissions are not modelled beyond read-only,
/// which cannot be changed yet; succeeds for an existing file.
#[unsafe(no_mangle)]
pub extern "C" fn _chmod(path: *const u8, _mode: i32) -> i32 {
    _access(path, 0)
}

/// _getche: _getch, echoing the byte to standard output.
#[unsafe(no_mangle)]
pub extern "C" fn _getche() -> i32 {
    let c = _getch();
    if c >= 0 {
        let b = c as u8;
        let mut written = 0;
        // SAFETY: a one-byte buffer.
        unsafe { WriteFile(GetStdHandle(-11i32 as u32), &b, 1, &mut written, null_mut()) };
    }
    c
}

/// _spawnvp(mode, cmdname, argv): _spawnve with our environment, the
/// program searched for (with ".exe" if it has no extension) in the
/// current directory and PATH, as SearchPath does.
#[unsafe(no_mangle)]
pub extern "C" fn _spawnvp(mode: i32, cmdname: *const u8, argv: *const *const u8) -> isize {
    let mut found = [0u8; PATH];
    // SAFETY: the caller's NUL-terminated name; our buffer.
    let n = unsafe {
        SearchPathA(
            core::ptr::null(),
            cmdname,
            c".exe".as_ptr().cast(),
            PATH as u32,
            found.as_mut_ptr(),
            null_mut(),
        )
    } as usize;
    let program = if n > 0 && n < PATH { found.as_ptr() } else { cmdname };
    _spawnve(mode, program, argv, core::ptr::null())
}

/// _cwait(status, process, action): waits for a process from
/// _spawn*(P_NOWAIT), stores its exit code and closes its handle; the
/// handle, or -1.
#[unsafe(no_mangle)]
pub extern "C" fn _cwait(status: *mut i32, process: isize, _action: i32) -> isize {
    let h = process as *mut c_void;
    let mut code = 0u32;
    // SAFETY: the caller's process handle and status pointer.
    unsafe {
        if WaitForSingleObject(h, u32::MAX) != 0 || GetExitCodeProcess(h, &mut code) == 0 {
            set_errno(ECHILD);
            return -1;
        }
        CloseHandle(h);
        if !status.is_null() {
            *status = code as i32;
        }
    }
    process
}

const ECHILD: i32 = 10;

/// system(command): `%COMSPEC% /c command`. gala has no command
/// interpreter unless COMSPEC names one: then system(NULL) is 0 and a
/// command fails with ENOENT.
#[unsafe(no_mangle)]
pub extern "C" fn system(command: *const u8) -> i32 {
    let comspec = crate::getenv(c"COMSPEC".as_ptr().cast());
    if command.is_null() {
        return i32::from(!comspec.is_null());
    }
    if comspec.is_null() {
        set_errno(ENOENT);
        return -1;
    }
    let argv = [comspec.cast_const(), c"/c".as_ptr().cast(), command, core::ptr::null()];
    _spawnve(P_WAIT, comspec, argv.as_ptr(), core::ptr::null()) as i32
}
