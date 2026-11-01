//! gala's cmd.exe: the command processor's `/c` (and `/k`), as programs
//! use it (the C runtime's `system` and `_popen` run `cmd.exe /c
//! <command>`), from the behaviour `cmd /?` documents and Windows shows:
//!
//! - Switches before `/c`: `/s`, `/q`, `/d`, `/a`, `/u`, `/e:..`, `/f:..`,
//!   `/v:..` are accepted (and change nothing here).
//! - The quote rules of `/c`: the rest of the line keeps its quotes if
//!   there is no `/s`, exactly two quotes, no special character (`&<>()@^|`)
//!   between them, whitespace between them, and they enclose an
//!   executable's name; else a leading quote and the last quote are
//!   removed.
//! - One command: `%VAR%` expanded, `^` escaping a character, redirections
//!   `<`, `>`, `>>`, `n>`, `n>>`, `n>&m` (stdin 0, stdout 1, stderr 2).
//! - Internal commands: `echo` (`echo.` an empty line), `exit [/b] [n]`,
//!   `rem`; others are run as programs: found with the extensions .COM and
//!   .EXE in the current directory and then the PATH, started with the
//!   command line as written (without the redirections), waited for; its
//!   exit code is cmd's. Not found: exit code 9009 and "'name' is not
//!   recognized as an internal or external command," / "operable program
//!   or batch file." on stderr.
//! - Not yet: `&`, `&&`, `||`, `|`, `( )`, batch files, `if`/`for`/`set`,
//!   the interactive prompt; each is reported to the missing-features
//!   ledger and ends cmd with exit code 1.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: Bool,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetCommandLineW() -> *const u16;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn GetFileAttributesW(path: *const u16) -> u32;
    fn GetCurrentDirectoryW(size: u32, buf: *mut u16) -> u32;
    fn SetFilePointerEx(h: Handle, distance: i64, new: *mut i64, method: u32) -> Bool;
    fn GetCurrentProcess() -> Handle;
    fn DuplicateHandle(
        source_process: Handle,
        source: Handle,
        target_process: Handle,
        target: *mut Handle,
        access: u32,
        inherit: Bool,
        options: u32,
    ) -> Bool;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: *const SecurityAttributes,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    #[allow(clippy::too_many_arguments)]
    fn CreateProcessW(
        application: *const u16,
        command_line: *mut u16,
        process_attributes: *mut c_void,
        thread_attributes: *mut c_void,
        inherit: Bool,
        flags: u32,
        environment: *mut c_void,
        directory: *const u16,
        startup: *const u8,
        info: *mut [usize; 3],
    ) -> Bool;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn GalaRecordMissing(name: *const u8, length: usize, what: u32, detail: u64) -> u32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn __chkstk();
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
const OPEN_ALWAYS: u32 = 4;
const CREATE_ALWAYS: u32 = 2;
const STARTF_USESTDHANDLES: u32 = 0x100;
const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const DUPLICATE_SAME_ACCESS: u32 = 2;
/// The exit code of a command cmd cannot find.
const NOT_FOUND: u32 = 9009;
/// The longest command line handled (as Windows' cmd: 8191 characters).
const MAX: usize = 8192;

/// A UTF-16 buffer with its length.
struct Text {
    b: [u16; MAX],
    n: usize,
}

impl Text {
    fn new() -> Text {
        Text { b: [0; MAX], n: 0 }
    }

    fn push(&mut self, c: u16) -> bool {
        if self.n + 1 >= MAX {
            return false;
        }
        self.b[self.n] = c;
        self.n += 1;
        true
    }

    fn push_all(&mut self, s: &[u16]) -> bool {
        s.iter().all(|&c| self.push(c))
    }

    fn as_slice(&self) -> &[u16] {
        &self.b[..self.n]
    }

    /// The text, NUL-terminated.
    fn ptr(&mut self) -> *mut u16 {
        self.b[self.n] = 0;
        self.b.as_mut_ptr()
    }
}

fn is_space(c: u16) -> bool {
    c == u16::from(b' ') || c == u16::from(b'\t')
}

fn lower(c: u16) -> u16 {
    if (u16::from(b'A')..=u16::from(b'Z')).contains(&c) { c + 32 } else { c }
}

fn same_ascii(a: &[u16], b: &str) -> bool {
    a.len() == b.len() && a.iter().zip(b.bytes()).all(|(&x, y)| lower(x) == u16::from(y))
}

/// Writes UTF-16 text to `h` in the ANSI code page (UTF-8 on gala).
fn write_text(h: Handle, s: &[u16]) {
    let mut out = [0u8; 4 * 256];
    for chunk in s.chunks(256) {
        let mut n = 0;
        for c in char::decode_utf16(chunk.iter().copied()) {
            let mut b = [0u8; 4];
            let e = c.unwrap_or(char::REPLACEMENT_CHARACTER).encode_utf8(&mut b);
            out[n..n + e.len()].copy_from_slice(e.as_bytes());
            n += e.len();
        }
        let mut written = 0;
        // SAFETY: our buffer.
        unsafe { WriteFile(h, out.as_ptr(), n as u32, &mut written, core::ptr::null_mut()) };
    }
}

fn write_ascii(h: Handle, s: &[u8]) {
    let mut written = 0;
    // SAFETY: our buffer.
    unsafe { WriteFile(h, s.as_ptr(), s.len() as u32, &mut written, core::ptr::null_mut()) };
}

/// Records a feature cmd lacks and ends with exit code 1.
fn unsupported(what: &[u8]) -> ! {
    // SAFETY: a valid buffer; then the process ends.
    unsafe {
        GalaRecordMissing(what.as_ptr(), what.len(), 2, 0);
        ExitProcess(1)
    }
}

/// This process's command line.
fn command_line() -> &'static [u16] {
    // SAFETY: the process's NUL-terminated command line, kept for its life.
    unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    }
}

/// The command line after cmd's own name and switches: (the command,
/// /s given, /c or /k given).
fn after_switches(line: &[u16]) -> (&[u16], bool, bool) {
    let mut i = 0;
    // The program's name: quoted, or up to whitespace.
    if line.first() == Some(&u16::from(b'"')) {
        i = 1;
        while i < line.len() && line[i] != u16::from(b'"') {
            i += 1;
        }
        i = (i + 1).min(line.len());
    } else {
        while i < line.len() && !is_space(line[i]) {
            i += 1;
        }
    }
    let mut s_switch = false;
    loop {
        while i < line.len() && is_space(line[i]) {
            i += 1;
        }
        if i + 1 >= line.len() || line[i] != u16::from(b'/') {
            return (&line[i..], s_switch, false);
        }
        let switch = lower(line[i + 1]);
        if switch == u16::from(b'c') || switch == u16::from(b'k') {
            if switch == u16::from(b'k') {
                unsupported(b"cmd.exe!/k (interactive)");
            }
            let rest = &line[i + 2..];
            let start = rest.iter().position(|&c| !is_space(c)).unwrap_or(rest.len());
            return (&rest[start..], s_switch, true);
        }
        if switch == u16::from(b's') {
            s_switch = true;
        }
        // /q /d /a /u /e:x /f:x /v:x: skip the switch.
        i += 2;
        while i < line.len() && !is_space(line[i]) && line[i] != u16::from(b'/') {
            i += 1;
        }
    }
}

/// `/c`'s quote rule (cmd /?): keep the quotes when the command has
/// exactly two, no special characters between them, whitespace between
/// them, and they enclose an executable's name; else strip a leading
/// quote and the last quote.
fn apply_quote_rule(command: &[u16], s_switch: bool, out: &mut Text) {
    let quotes: usize = command.iter().filter(|&&c| c == u16::from(b'"')).count();
    let first_is_quote = command.first() == Some(&u16::from(b'"'));
    let keep = !s_switch && quotes == 2 && first_is_quote && {
        let close = command[1..].iter().position(|&c| c == u16::from(b'"')).map_or(0, |p| p + 1);
        let inner = &command[1..close];
        let special = inner.iter().any(|&c| b"&<>()@^|".iter().any(|&s| u16::from(s) == c));
        let has_space = inner.iter().any(|&c| is_space(c));
        !special && has_space && resolve(inner).is_some()
    };
    if keep || !first_is_quote {
        out.push_all(command);
        return;
    }
    let last = command.iter().rposition(|&c| c == u16::from(b'"')).unwrap_or(0);
    out.push_all(&command[1..last.max(1)]);
    if last > 0 {
        out.push_all(&command[last + 1..]);
    }
}

/// `%NAME%` replaced by the variable's value (left as written if it has
/// none).
fn expand(command: &[u16], out: &mut Text) {
    let mut i = 0;
    while i < command.len() {
        let c = command[i];
        if c == u16::from(b'%')
            && let Some(len) = command[i + 1..].iter().position(|&x| x == u16::from(b'%'))
            && len > 0
        {
            let mut name = [0u16; 256];
            if len < 255 {
                name[..len].copy_from_slice(&command[i + 1..i + 1 + len]);
                let mut value = [0u16; 2048];
                // SAFETY: our buffers.
                let n = unsafe { GetEnvironmentVariableW(name.as_ptr(), value.as_mut_ptr(), 2048) }
                    as usize;
                if n > 0 && n < 2048 {
                    out.push_all(&value[..n]);
                    i += len + 2;
                    continue;
                }
            }
        }
        out.push(c);
        i += 1;
    }
}

/// A redirection: the standard handle (0, 1, 2) and where it goes.
#[derive(Clone, Copy)]
enum Target {
    /// A file: (start, end) of its name in the command, appending.
    File(usize, usize, bool),
    /// Another standard handle (`2>&1`).
    Handle(usize),
}

/// One command, parsed: its text without the redirections, and them.
struct Command {
    text: Text,
    redirects: [(usize, Option<Target>); 3],
}

/// Splits the redirections off `command` (outside quotes; `^` escapes);
/// the operators cmd has but gala does not yet end it.
fn parse(command: &[u16], names: &mut Text) -> Command {
    let mut c = Command { text: Text::new(), redirects: [(0, None), (1, None), (2, None)] };
    let mut i = 0;
    let mut quoted = false;
    while i < command.len() {
        let ch = command[i];
        if ch == u16::from(b'"') {
            quoted = !quoted;
            c.text.push(ch);
            i += 1;
            continue;
        }
        if quoted {
            c.text.push(ch);
            i += 1;
            continue;
        }
        if ch == u16::from(b'^') && i + 1 < command.len() {
            c.text.push(command[i + 1]);
            i += 2;
            continue;
        }
        if ch == u16::from(b'|') {
            unsupported(b"cmd.exe!pipelines (|)");
        }
        if ch == u16::from(b'&') {
            unsupported(b"cmd.exe!command lists (&, &&, ||)");
        }
        // A redirection: [n]< [n]> [n]>> [n]>&m.
        let digit = ch >= u16::from(b'0') && ch <= u16::from(b'2');
        let op_at = if digit
            && i + 1 < command.len()
            && (command[i + 1] == u16::from(b'>') || command[i + 1] == u16::from(b'<'))
        {
            // A handle number only right before the operator, after a
            // space (or at the start).
            if i == 0 || is_space(command[i - 1]) { Some(i + 1) } else { None }
        } else if ch == u16::from(b'>') || ch == u16::from(b'<') {
            Some(i)
        } else {
            None
        };
        let Some(op) = op_at else {
            c.text.push(ch);
            i += 1;
            continue;
        };
        let input = command[op] == u16::from(b'<');
        let handle = if op > i {
            usize::from(command[i] - u16::from(b'0'))
        } else if input {
            0
        } else {
            1
        };
        let mut j = op + 1;
        let append = !input && j < command.len() && command[j] == u16::from(b'>');
        if append {
            j += 1;
        }
        if j < command.len() && command[j] == u16::from(b'&') {
            let to =
                command.get(j + 1).map_or(1, |&d| usize::from(d.wrapping_sub(u16::from(b'0'))));
            if to > 2 {
                unsupported(b"cmd.exe!redirection to a handle above 2");
            }
            c.redirects[handle] = (handle, Some(Target::Handle(to)));
            i = j + 2;
            continue;
        }
        while j < command.len() && is_space(command[j]) {
            j += 1;
        }
        // The file name: quoted, or up to whitespace or another operator.
        let start = names.n;
        if j < command.len() && command[j] == u16::from(b'"') {
            j += 1;
            while j < command.len() && command[j] != u16::from(b'"') {
                names.push(command[j]);
                j += 1;
            }
            j += 1;
        } else {
            while j < command.len()
                && !is_space(command[j])
                && command[j] != u16::from(b'>')
                && command[j] != u16::from(b'<')
                && command[j] != u16::from(b'&')
                && command[j] != u16::from(b'|')
            {
                names.push(command[j]);
                j += 1;
            }
        }
        let end = names.n;
        names.push(0);
        c.redirects[handle] = (handle, Some(Target::File(start, end, append)));
        i = j;
    }
    // Trailing whitespace (before a removed redirection) goes.
    while c.text.n > 0 && is_space(c.text.b[c.text.n - 1]) {
        c.text.n -= 1;
    }
    c
}

/// The command's first word (the program), unquoted.
fn first_word(text: &[u16]) -> (&[u16], usize) {
    let start = text.iter().position(|&c| !is_space(c)).unwrap_or(text.len());
    let t = &text[start..];
    if t.first() == Some(&u16::from(b'"')) {
        let end = t[1..].iter().position(|&c| c == u16::from(b'"')).map_or(t.len(), |p| p + 1);
        return (&t[1..end], start + (end + 1).min(t.len()));
    }
    let end = t.iter().position(|&c| is_space(c) || c == u16::from(b'/')).unwrap_or(t.len());
    (&t[..end], start + end)
}

/// Whether `path` names a file (not a directory).
fn is_file(path: &mut Text) -> bool {
    // SAFETY: a NUL-terminated path.
    let a = unsafe { GetFileAttributesW(path.ptr()) };
    a != INVALID_FILE_ATTRIBUTES && a & FILE_ATTRIBUTE_DIRECTORY == 0
}

/// `dir\name` (or `name`) with each of the extensions tried: the path
/// found.
fn try_with_extensions(dir: &[u16], name: &[u16]) -> Option<Text> {
    let has_ext = name.iter().rposition(|&c| c == u16::from(b'.')).is_some_and(|dot| {
        !name[dot..].iter().any(|&c| c == u16::from(b'\\') || c == u16::from(b'/'))
    });
    for ext in ["", ".com", ".exe"] {
        if has_ext != ext.is_empty() {
            continue;
        }
        let mut p = Text::new();
        if !dir.is_empty() {
            p.push_all(dir);
            if dir.last() != Some(&u16::from(b'\\')) && dir.last() != Some(&u16::from(b'/')) {
                p.push(u16::from(b'\\'));
            }
        }
        p.push_all(name);
        for c in ext.encode_utf16() {
            p.push(c);
        }
        if is_file(&mut p) {
            return Some(p);
        }
    }
    None
}

/// The program `name` names: a path as given, else searched in the
/// current directory and then each PATH directory.
fn resolve(name: &[u16]) -> Option<Text> {
    if name.is_empty() {
        return None;
    }
    let has_dir =
        name.iter().any(|&c| c == u16::from(b'\\') || c == u16::from(b'/') || c == u16::from(b':'));
    if has_dir {
        return try_with_extensions(&[], name);
    }
    let mut cwd = [0u16; 1024];
    // SAFETY: our buffer.
    let n = unsafe { GetCurrentDirectoryW(1024, cwd.as_mut_ptr()) } as usize;
    if n > 0
        && n < 1024
        && let Some(p) = try_with_extensions(&cwd[..n], name)
    {
        return Some(p);
    }
    let mut path = Text::new();
    let path_name = [u16::from(b'P'), u16::from(b'A'), u16::from(b'T'), u16::from(b'H'), 0];
    // SAFETY: our buffers.
    let n = unsafe { GetEnvironmentVariableW(path_name.as_ptr(), path.b.as_mut_ptr(), MAX as u32) }
        as usize;
    if n == 0 || n >= MAX {
        return None;
    }
    path.n = n;
    path.as_slice()
        .split(|&c| c == u16::from(b';'))
        .filter(|d| !d.is_empty())
        .find_map(|dir| try_with_extensions(dir, name))
}

/// Opens a redirection's file: for reading (stdin) or writing; an
/// inheritable handle.
fn open_redirect(name: &[u16], input: bool, append: bool) -> Option<Handle> {
    let mut p = Text::new();
    p.push_all(name);
    let sa = SecurityAttributes { length: 24, descriptor: core::ptr::null_mut(), inherit: 1 };
    let (access, disposition) = if input {
        (GENERIC_READ, OPEN_EXISTING)
    } else if append {
        (GENERIC_WRITE, OPEN_ALWAYS)
    } else {
        (GENERIC_WRITE, CREATE_ALWAYS)
    };
    // SAFETY: a NUL-terminated name; our attributes.
    let h = unsafe { CreateFileW(p.ptr(), access, 3, &sa, disposition, 0, core::ptr::null_mut()) };
    if h as isize == -1 {
        return None;
    }
    if append {
        // SAFETY: our handle.
        unsafe { SetFilePointerEx(h, 0, core::ptr::null_mut(), 2) };
    }
    Some(h)
}

/// The three standard handles the command gets, after its redirections
/// (inheritable copies where they change); None if a file cannot be
/// opened (reported on stderr).
fn standard_handles(c: &Command, names: &Text) -> Option<[Handle; 3]> {
    // SAFETY: plain calls.
    let mut std = unsafe {
        [GetStdHandle(-10i32 as u32), GetStdHandle(-11i32 as u32), GetStdHandle(-12i32 as u32)]
    };
    // Files first, then handle copies (2>&1 after > file).
    for &(handle, target) in &c.redirects {
        if let Some(Target::File(start, end, append)) = target {
            let name = &names.b[start..end];
            match open_redirect(name, handle == 0, append) {
                Some(h) => std[handle] = h,
                None => {
                    // SAFETY: a plain call.
                    let err = unsafe { GetStdHandle(-12i32 as u32) };
                    write_ascii(err, b"The system cannot find the file specified.\r\n");
                    return None;
                }
            }
        }
    }
    for &(handle, target) in &c.redirects {
        if let Some(Target::Handle(to)) = target {
            let mut copy = core::ptr::null_mut();
            // SAFETY: plain calls with our handles.
            unsafe {
                let me = GetCurrentProcess();
                if DuplicateHandle(me, std[to], me, &mut copy, 0, 1, DUPLICATE_SAME_ACCESS) != 0 {
                    std[handle] = copy;
                }
            }
        }
    }
    Some(std)
}

/// Runs one command; its exit code.
fn execute(c: &mut Command, names: &Text) -> u32 {
    let (word, word_end) = first_word(c.text.as_slice());
    let mut word_buf = [0u16; 260];
    let wl = word.len().min(259);
    word_buf[..wl].copy_from_slice(&word[..wl]);
    let word = &word_buf[..wl];
    let rest_start = word_end.min(c.text.n);
    let Some(std) = standard_handles(c, names) else { return 1 };
    // Internal commands.
    let echo_dot = word.len() == 5 && same_ascii(&word[..4], "echo") && word[4] == u16::from(b'.');
    if same_ascii(word, "echo") || echo_dot {
        let rest = &c.text.as_slice()[rest_start..];
        // One separator after `echo` is dropped; the rest is printed.
        let rest = if !echo_dot && rest.first().is_some_and(|&ch| is_space(ch)) {
            &rest[1..]
        } else {
            rest
        };
        if rest.is_empty() && !echo_dot {
            write_ascii(std[1], b"ECHO is on.\r\n");
        } else {
            write_text(std[1], rest);
            write_ascii(std[1], b"\r\n");
        }
        return 0;
    }
    if same_ascii(word, "rem") {
        return 0;
    }
    if same_ascii(word, "exit") {
        let rest = &c.text.as_slice()[rest_start..];
        let mut code: u32 = 0;
        for token in rest.split(|&ch| is_space(ch)).filter(|t| !t.is_empty()) {
            if same_ascii(token, "/b") {
                continue;
            }
            for &d in token {
                if !(u16::from(b'0')..=u16::from(b'9')).contains(&d) {
                    break;
                }
                code = code.wrapping_mul(10).wrapping_add(u32::from(d - u16::from(b'0')));
            }
            break;
        }
        return code;
    }
    for builtin in [
        "set", "if", "for", "cd", "chdir", "goto", "call", "copy", "del", "dir", "mkdir", "md",
        "type",
    ] {
        if same_ascii(word, builtin) {
            unsupported(b"cmd.exe!internal command");
        }
    }
    // A program.
    let Some(mut program) = resolve(word) else {
        write_ascii(std[2], b"'");
        write_text(std[2], word);
        write_ascii(std[2], b"' is not recognized as an internal or external command,\r\noperable program or batch file.\r\n");
        return NOT_FOUND;
    };
    let ext_at = program.as_slice().iter().rposition(|&ch| ch == u16::from(b'.')).unwrap_or(0);
    if same_ascii(&program.as_slice()[ext_at..], ".bat")
        || same_ascii(&program.as_slice()[ext_at..], ".cmd")
    {
        unsupported(b"cmd.exe!batch files");
    }
    let mut startup = [0u8; 104];
    startup[0] = 104;
    startup[60..64].copy_from_slice(&STARTF_USESTDHANDLES.to_le_bytes());
    for (k, h) in std.iter().enumerate() {
        startup[80 + 8 * k..88 + 8 * k].copy_from_slice(&(*h as u64).to_le_bytes());
    }
    let mut info = [0usize; 3];
    let null = core::ptr::null_mut();
    // SAFETY: NUL-terminated strings; our structures.
    unsafe {
        if CreateProcessW(
            program.ptr(),
            c.text.ptr(),
            null,
            null,
            1,
            0,
            null,
            core::ptr::null(),
            startup.as_ptr(),
            &mut info,
        ) == 0
        {
            write_ascii(std[2], b"The system cannot execute the specified program.\r\n");
            return 1;
        }
        WaitForSingleObject(info[0] as Handle, u32::MAX);
        let mut code = 0;
        GetExitCodeProcess(info[0] as Handle, &mut code);
        CloseHandle(info[0] as Handle);
        CloseHandle(info[1] as Handle);
        code
    }
}

fn main() -> u32 {
    let (command, s_switch, has_c) = after_switches(command_line());
    if !has_c {
        unsupported(b"cmd.exe!interactive prompt");
    }
    let mut unquoted = Text::new();
    apply_quote_rule(command, s_switch, &mut unquoted);
    let mut expanded = Text::new();
    expand(unquoted.as_slice(), &mut expanded);
    let mut names = Text::new();
    let mut c = parse(expanded.as_slice(), &mut names);
    if c.text.as_slice().iter().all(|&ch| is_space(ch)) {
        return 0;
    }
    execute(&mut c, &names)
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = main();
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(255) }
}
