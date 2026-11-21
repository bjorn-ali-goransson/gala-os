//! The agent panel's `run` tool (agent.exe; ROADMAP LY "Change this
//! program", step 4; ARCHITECTURE "The agent panel", "Tools"), as a user
//! sees it: File Explorer open on a folder of three files, "compress all
//! the files" asked in the panel, the model's call of `run` shown as a
//! step, Run clicked, the archive made in that folder, the model's final
//! answer listed.
//!
//! The model is reached through `cargo xtask agent-relay` (QEMU's
//! guestfwd to 10.0.2.101:80), which replays the calls recorded in
//! `xtask/agent-recordings/`: the same on every run.
//!
//! - once the panel is docked, this test makes `%TEMP%\agent_run` with
//!   `one.txt`, `two.txt` and `three.txt` (known contents) and starts
//!   File Explorer on it (`C:\Windows\explorer.exe <folder>`);
//! - Explorer, active, is the panel's context ("ContextProgram" File
//!   Explorer) and reports the folder it shows: the chip's
//!   "ContextFolder" is that folder;
//! - the prompt ("Prompt") is announced for the scenario's `click_on`,
//!   which types `compress all the files`; Send ("SendButton") is
//!   announced and clicked;
//! - the model's call is a step ("Request0Step0"): its command
//!   ("...Command", not empty) and its folder ("...Folder", `In
//!   <folder>`), with Run ("...Run", a Button named Run) and Cancel
//!   ("...Cancel", named Cancel); nothing has run (the folder still has
//!   its three files only); Run is announced and clicked;
//! - the step's result ("...Result") says exit code 0, and the reply card
//!   ("Reply0") the model's final answer (not one of the loop's states,
//!   nor "No answer: ...");
//! - the folder has one more file: the archive, whose first bytes are a
//!   gzip's (1f 8b), a bzip2's (`BZh`) or a tar's (`ustar` at 257), and
//!   which `busybox tar -t` lists with the three files and `busybox tar
//!   -xO` gives back `two.txt`'s contents from;
//! - then the panel's Hide is announced and the test waits for the
//!   scenario's click, so the run ends after the last screen step.
//!
//! Prints `agent_run_test: files made in <folder>`, `agent_run_test:
//! typed`, `agent_run_test: command: <it>`, `agent_run_test: step shown`,
//! `agent_run_test: answered: <it>`, `agent_run_test: archive <name>, N
//! bytes`, `agent_run_test: listed: <tar -t's output>`, `agent_run_test:
//! archive checked`, `agent_run_test: ok`, or `agent_run_test: check N
//! got X (line L)` and exits with N.

#![no_std]
#![no_main]

use ntrt as _;
use uia_client::{BUTTON, EDIT, Element, TEXT, Uia};

const NAME: &str = "agent_run_test";
/// What the scenario types (AGENT_RUN_SCREEN).
const PROMPT: &str = "compress all the files";
/// The folder under %TEMP%, and its files with their contents.
const FOLDER: &str = "agent_run";
const FILES: [(&str, &[u8]); 3] = [
    ("one.txt", b"the first file\r\n"),
    ("two.txt", b"the second file, a little longer\r\n"),
    ("three.txt", b"the third\r\n"),
];
/// The texts the reply card has while the loop runs (agent's `reply`).
const UNFINISHED: [&str; 3] =
    ["Asking the model...", "Run the command, or cancel it.", "Running the command..."];

type Handle = usize;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, overlapped: usize) -> i32;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, overlapped: usize) -> i32;
    fn ExitProcess(code: u32) -> !;
    fn Sleep(ms: u32);
    fn GetTickCount64() -> u64;
    fn GetTempPathW(size: u32, buf: *mut u16) -> u32;
    fn CreateDirectoryW(path: *const u16, sa: usize) -> i32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        sa: usize,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CloseHandle(h: Handle) -> i32;
    fn FindFirstFileW(name: *const u16, data: *mut FindData) -> Handle;
    fn FindNextFileW(h: Handle, data: *mut FindData) -> i32;
    fn FindClose(h: Handle) -> i32;
    fn SearchPathW(
        path: *const u16,
        file: *const u16,
        ext: *const u16,
        size: u32,
        buf: *mut u16,
        part: usize,
    ) -> u32;
    fn CreatePipe(read: *mut Handle, write: *mut Handle, sa: *const Security, size: u32) -> i32;
    fn SetHandleInformation(h: Handle, mask: u32, flags: u32) -> i32;
    fn CreateProcessW(
        app: *const u16,
        cmd: *mut u16,
        pa: usize,
        ta: usize,
        inherit: i32,
        flags: u32,
        env: usize,
        dir: *const u16,
        si: *const StartupInfo,
        pi: *mut ProcessInfo,
    ) -> i32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> i32;
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> Handle;
    fn SystemParametersInfoW(action: u32, param: u32, out: *mut [i32; 4], ini: u32) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
}

#[repr(C)]
struct Security {
    size: u32,
    descriptor: usize,
    inherit: i32,
}

/// STARTUPINFOW.
#[repr(C)]
struct StartupInfo {
    cb: u32,
    reserved: usize,
    desktop: usize,
    title: usize,
    pos: [u32; 7],
    flags: u32,
    show: u16,
    reserved2_len: u16,
    reserved2: usize,
    input: Handle,
    output: Handle,
    error: Handle,
}

#[repr(C)]
struct ProcessInfo {
    process: Handle,
    thread: Handle,
    pid: u32,
    tid: u32,
}

/// WIN32_FIND_DATAW.
#[repr(C)]
struct FindData {
    attributes: u32,
    times: [u32; 6],
    size_high: u32,
    size_low: u32,
    reserved: [u32; 2],
    name: [u16; 260],
    alternate: [u16; 14],
}

const SPI_GETWORKAREA: u32 = 0x30;
const SM_CXSCREEN: i32 = 0;
/// The panel's docked width (agent's WIDTH).
const WIDTH: i32 = 320;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const FILE_SHARE_READ: u32 = 1;
const INVALID: Handle = usize::MAX;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const STARTF_USESTDHANDLES: u32 = 0x100;
const HANDLE_FLAG_INHERIT: u32 = 1;
const INFINITE: u32 = u32::MAX;
/// How long a step may take: the scenario's clicks come once this test
/// has printed what the step waits for, the answers once the relay has
/// replayed them, the archive once busybox has made it.
const STEP_MS: u64 = 60_000;

// ---- Output ---------------------------------------------------------------

struct Line {
    buf: [u8; 600],
    n: usize,
}

impl Line {
    fn new() -> Self {
        Self { buf: [0; 600], n: 0 }
    }

    fn s(mut self, s: &[u8]) -> Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn dec(self, v: i64) -> Self {
        let mut d = [0u8; 21];
        let mut i = d.len();
        let neg = v < 0;
        let mut v = v.unsigned_abs();
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        if neg {
            i -= 1;
            d[i] = b'-';
        }
        self.s(&d[i..])
    }

    /// `s` with its line ends shown as `\n`, so the line stays one.
    fn flat(mut self, s: &[u8]) -> Self {
        for &b in s {
            self = match b {
                b'\r' => self,
                b'\n' => self.s(b"\\n"),
                b => self.s(&[b]),
            };
        }
        self
    }

    fn print(self) {
        let mut written = 0;
        // SAFETY: our buffer; the standard output handle.
        unsafe {
            WriteFile(
                GetStdHandle(-11i32 as u32),
                self.buf.as_ptr(),
                self.n as u32,
                &mut written,
                0,
            )
        };
    }
}

fn say(what: &[u8]) {
    Line::new().s(NAME.as_bytes()).s(b": ").s(what).s(b"\r\n").print();
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: i64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        Line::new()
            .s(NAME.as_bytes())
            .s(b": check ")
            .dec(i64::from(self.0))
            .s(b" got ")
            .dec(got)
            .s(b" (line ")
            .dec(i64::from(core::panic::Location::caller().line()))
            .s(b")\r\n")
            .print();
        Err(self.0)
    }

    #[track_caller]
    fn eq(&mut self, got: i64, want: i64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

/// A NUL-terminated UTF-16 string of at most 511 units (ASCII here).
struct W {
    buf: [u16; 512],
    n: usize,
}

impl W {
    fn new(s: &str) -> W {
        W { buf: [0; 512], n: 0 }.add(s)
    }

    fn add(mut self, s: &str) -> W {
        for u in s.encode_utf16() {
            if self.n < self.buf.len() - 1 {
                self.buf[self.n] = u;
                self.n += 1;
            }
        }
        self.buf[self.n] = 0;
        self
    }

    fn ptr(&self) -> *const u16 {
        self.buf.as_ptr()
    }

    fn mut_ptr(&mut self) -> *mut u16 {
        self.buf.as_mut_ptr()
    }
}

/// A short UTF-8 text (an ASCII path, a file's name).
#[derive(Clone, Copy)]
struct S {
    buf: [u8; 260],
    n: usize,
}

impl S {
    fn new() -> S {
        S { buf: [0; 260], n: 0 }
    }

    fn from_wide(w: &[u16]) -> S {
        let mut s = S::new();
        for c in char::decode_utf16(w.iter().copied().take_while(|&u| u != 0)) {
            let mut b = [0u8; 4];
            s = s.add(c.unwrap_or('?').encode_utf8(&mut b));
        }
        s
    }

    fn add(mut self, t: &str) -> S {
        for &b in t.as_bytes() {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.n]).unwrap_or("?")
    }
}

/// Waits up to `ms` for `done`.
fn wait(ms: u64, done: impl Fn() -> bool) -> bool {
    // SAFETY: plain calls.
    unsafe {
        let end = GetTickCount64() + ms;
        loop {
            if done() {
                return true;
            }
            if GetTickCount64() >= end {
                return false;
            }
            Sleep(20);
        }
    }
}

/// `path`'s file made anew with `bytes`.
fn write_file(path: &W, bytes: &[u8]) -> bool {
    // SAFETY: a NUL-terminated path; our buffer; the handle closed.
    unsafe {
        let h = CreateFileW(path.ptr(), GENERIC_WRITE, 0, 0, CREATE_ALWAYS, 0, 0);
        if h == INVALID {
            return false;
        }
        let mut n = 0;
        let ok = WriteFile(h, bytes.as_ptr(), bytes.len() as u32, &mut n, 0) != 0;
        CloseHandle(h);
        ok && n as usize == bytes.len()
    }
}

/// The first bytes of `path`'s file (up to the buffer's size): how many.
fn read_head(path: &W, buf: &mut [u8]) -> Option<usize> {
    // SAFETY: a NUL-terminated path; our buffer; the handle closed.
    unsafe {
        let h = CreateFileW(path.ptr(), GENERIC_READ, FILE_SHARE_READ, 0, OPEN_EXISTING, 0, 0);
        if h == INVALID {
            return None;
        }
        let mut total = 0usize;
        while total < buf.len() {
            let mut n = 0u32;
            let rest = &mut buf[total..];
            if ReadFile(h, rest.as_mut_ptr(), rest.len() as u32, &mut n, 0) == 0 || n == 0 {
                break;
            }
            total += n as usize;
        }
        CloseHandle(h);
        Some(total)
    }
}

/// The folder's files (not its directories): their names and sizes.
fn files_in(folder: &str, out: &mut [(S, u64); 8]) -> usize {
    let pattern = W::new(folder).add("\\*");
    // SAFETY: an all-zero WIN32_FIND_DATAW is valid; a NUL-terminated
    // pattern; the search closed.
    unsafe {
        let mut d: FindData = core::mem::zeroed();
        let h = FindFirstFileW(pattern.ptr(), &mut d);
        if h == INVALID {
            return 0;
        }
        let mut n = 0;
        loop {
            if d.attributes & FILE_ATTRIBUTE_DIRECTORY == 0 && n < out.len() {
                let size = (u64::from(d.size_high) << 32) | u64::from(d.size_low);
                out[n] = (S::from_wide(&d.name), size);
                n += 1;
            }
            if FindNextFileW(h, &mut d) == 0 {
                break;
            }
        }
        FindClose(h);
        n
    }
}

/// Runs `line` (a whole command line) in `dir` with its standard output
/// and error one pipe: its exit code and the first bytes of its output.
fn run_captured(line: &str, dir: &str, out: &mut [u8]) -> Option<(u32, usize)> {
    // SAFETY: our structures and buffers, live through each call; the
    // handles closed.
    unsafe {
        let sa =
            Security { size: core::mem::size_of::<Security>() as u32, descriptor: 0, inherit: 1 };
        let (mut read, mut write) = (0, 0);
        if CreatePipe(&mut read, &mut write, &sa, 0) == 0 {
            return None;
        }
        SetHandleInformation(read, HANDLE_FLAG_INHERIT, 0);
        let si = StartupInfo {
            cb: core::mem::size_of::<StartupInfo>() as u32,
            reserved: 0,
            desktop: 0,
            title: 0,
            pos: [0; 7],
            flags: STARTF_USESTDHANDLES,
            show: 0,
            reserved2_len: 0,
            reserved2: 0,
            input: 0,
            output: write,
            error: write,
        };
        let mut pi = ProcessInfo { process: 0, thread: 0, pid: 0, tid: 0 };
        let mut cmd = W::new(line);
        let dir = W::new(dir);
        let made = CreateProcessW(
            core::ptr::null(),
            cmd.mut_ptr(),
            0,
            0,
            1,
            0,
            0,
            dir.ptr(),
            &si,
            &mut pi,
        );
        CloseHandle(write);
        if made == 0 {
            CloseHandle(read);
            return None;
        }
        let mut total = 0usize;
        let mut sink = [0u8; 512];
        loop {
            let mut n = 0u32;
            let buf: &mut [u8] = if total < out.len() { &mut out[total..] } else { &mut sink };
            if ReadFile(read, buf.as_mut_ptr(), buf.len() as u32, &mut n, 0) == 0 || n == 0 {
                break;
            }
            if total < out.len() {
                total += n as usize;
            }
        }
        CloseHandle(read);
        let mut code = 0u32;
        WaitForSingleObject(pi.process, INFINITE);
        GetExitCodeProcess(pi.process, &mut code);
        CloseHandle(pi.process);
        CloseHandle(pi.thread);
        Some((code, total))
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

// ---- The checks -----------------------------------------------------------

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: plain calls with our structures.
    unsafe {
        // The panel, started with the session, docked: the work area ends
        // at it (it watches the active window from before it docks).
        let (class, title) = (W::new("GalaUiWindow"), W::new("Agent"));
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let docked = wait(STEP_MS, || {
            let mut work = [0i32; 4];
            SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work, 0);
            FindWindowW(class.ptr(), title.ptr()) != 0 && work[2] == sw - WIDTH
        });
        c.ok(docked, 0)?;
        let panel = FindWindowW(class.ptr(), title.ptr());

        // The folder and its three files.
        let mut temp = [0u16; 260];
        let n = GetTempPathW(temp.len() as u32, temp.as_mut_ptr()) as usize;
        c.ok(n > 0 && n < temp.len(), n as i64)?;
        let folder = S::from_wide(&temp[..n]);
        let folder = S::new().add(folder.as_str().trim_end_matches('\\')).add("\\").add(FOLDER);
        let folder = folder.as_str();
        CreateDirectoryW(W::new(folder).ptr(), 0);
        for (name, bytes) in FILES {
            c.ok(write_file(&W::new(folder).add("\\").add(name), bytes), 0)?;
        }
        let mut found = [(S::new(), 0u64); 8];
        c.eq(files_in(folder, &mut found) as i64, FILES.len() as i64)?;
        Line::new()
            .s(NAME.as_bytes())
            .s(b": files made in ")
            .s(folder.as_bytes())
            .s(b"\r\n")
            .print();

        // File Explorer on it: the panel's context, its folder reported.
        let mut line = W::new("C:\\Windows\\explorer.exe \"").add(folder).add("\"");
        let si = StartupInfo {
            cb: core::mem::size_of::<StartupInfo>() as u32,
            reserved: 0,
            desktop: 0,
            title: 0,
            pos: [0; 7],
            flags: 0,
            show: 0,
            reserved2_len: 0,
            reserved2: 0,
            input: 0,
            output: 0,
            error: 0,
        };
        let mut pi = ProcessInfo { process: 0, thread: 0, pid: 0, tid: 0 };
        let started = CreateProcessW(
            core::ptr::null(),
            line.mut_ptr(),
            0,
            0,
            0,
            0,
            0,
            core::ptr::null(),
            &si,
            &mut pi,
        );
        c.ok(started != 0, 0)?;
        CloseHandle(pi.process);
        CloseHandle(pi.thread);

        let uia = Uia::new();
        c.ok(uia.is_ok(), uia.as_ref().err().copied().unwrap_or(0).into())?;
        let uia = uia.unwrap_or_else(|_| unreachable!());
        let find = |id: &str| -> Option<Element> { uia.find(&uia.window(panel)?, id) };
        let named = wait(STEP_MS, || {
            find("ContextProgram").is_some_and(|e| e.name() == "File Explorer")
                && find("ContextFolder").is_some_and(|e| e.name().as_str() == folder)
        });
        c.ok(named, 0)?;
        say(b"folder known");

        // The prompt: the scenario clicks it and types.
        let prompt = find("Prompt");
        c.ok(prompt.is_some(), 0)?;
        let prompt = prompt.unwrap_or_else(|| unreachable!());
        c.eq(prompt.control_type().into(), EDIT.into())?;
        uia_client::announce(NAME, "Prompt", &prompt);
        let typed = wait(STEP_MS, || find("Prompt").is_some_and(|e| e.value() == PROMPT));
        c.ok(typed, 0)?;
        let send = find("SendButton");
        c.ok(send.as_ref().is_some_and(|e| e.name() == "Send"), 0)?;
        let send = send.unwrap_or_else(|| unreachable!());
        c.eq(send.control_type().into(), BUTTON.into())?;
        uia_client::announce(NAME, "SendButton", &send);
        say(b"typed");

        // The model's call: a step with Run and Cancel; nothing has run.
        let shown = wait(STEP_MS, || find("Request0Step0Run").is_some());
        if !shown {
            // What the reply card says instead (the model's failure, or
            // an answer without a call).
            let reply = find("Reply0").map(|e| e.name());
            Line::new()
                .s(NAME.as_bytes())
                .s(b": no step; the reply says: ")
                .s(reply.as_ref().map_or(&b"(no reply card)"[..], |r| r.as_str().as_bytes()))
                .s(b"\r\n")
                .print();
        }
        c.ok(shown, 0)?;
        let command = find("Request0Step0Command");
        c.ok(command.as_ref().is_some_and(|e| !e.name().as_str().is_empty()), 0)?;
        let command = command.unwrap_or_else(|| unreachable!()).name();
        Line::new()
            .s(NAME.as_bytes())
            .s(b": command: ")
            .s(command.as_str().as_bytes())
            .s(b"\r\n")
            .print();
        let shown = find("Request0Step0Folder");
        let want = S::new().add("In ").add(folder);
        c.ok(shown.is_some_and(|e| e.name().as_str() == want.as_str()), 0)?;
        let cancel = find("Request0Step0Cancel");
        c.ok(cancel.is_some_and(|e| e.name() == "Cancel"), 0)?;
        let run = find("Request0Step0Run");
        c.ok(run.as_ref().is_some_and(|e| e.name() == "Run"), 0)?;
        let run = run.unwrap_or_else(|| unreachable!());
        c.eq(run.control_type().into(), BUTTON.into())?;
        c.ok(find("Reply0").is_some_and(|e| e.name() == UNFINISHED[1]), 0)?;
        c.eq(files_in(folder, &mut found) as i64, FILES.len() as i64)?;
        uia_client::announce(NAME, "Run", &run);
        say(b"step shown");

        // Run: the command's result, then the model's final answer.
        let answered = wait(STEP_MS, || {
            find("Reply0").is_some_and(|e| !UNFINISHED.contains(&e.name().as_str()))
        });
        c.ok(answered, 0)?;
        let reply = find("Reply0");
        c.ok(reply.is_some(), 0)?;
        let reply = reply.unwrap_or_else(|| unreachable!());
        c.eq(reply.control_type().into(), TEXT.into())?;
        let said = reply.name();
        let said = said.as_str();
        Line::new().s(NAME.as_bytes()).s(b": answered: ").flat(said.as_bytes()).s(b"\r\n").print();
        c.ok(!said.is_empty() && !said.starts_with("No answer"), 0)?;
        let result = find("Request0Step0Result");
        c.ok(result.is_some_and(|e| e.name().as_str().starts_with("Exit code 0")), 0)?;
        // One step: the model asked for nothing more.
        c.ok(find("Request0Step1").is_none(), 0)?;

        // The archive: the one new file in the folder.
        let n = files_in(folder, &mut found);
        c.eq(n as i64, FILES.len() as i64 + 1)?;
        let new = found[..n]
            .iter()
            .find(|(name, _)| FILES.iter().all(|(f, _)| !name.as_str().eq_ignore_ascii_case(f)));
        c.ok(new.is_some(), 0)?;
        let (archive, size) = *new.unwrap_or_else(|| unreachable!());
        Line::new()
            .s(NAME.as_bytes())
            .s(b": archive ")
            .s(archive.as_str().as_bytes())
            .s(b", ")
            .dec(size as i64)
            .s(b" bytes\r\n")
            .print();
        let mut head = [0u8; 512];
        let path = W::new(folder).add("\\").add(archive.as_str());
        let got = read_head(&path, &mut head);
        c.ok(got.is_some_and(|g| g >= 3), 0)?;
        let got = got.unwrap_or(0);
        let flag = if head[..2] == [0x1f, 0x8b] {
            "z"
        } else if head[..3] == *b"BZh" {
            "j"
        } else {
            c.ok(got >= 262 && head[257..262] == *b"ustar", head[0].into())?;
            ""
        };

        // busybox lists the three files in it, and gives one back.
        let mut bb = [0u16; 260];
        let bn = SearchPathW(
            core::ptr::null(),
            W::new("busybox.exe").ptr(),
            core::ptr::null(),
            bb.len() as u32,
            bb.as_mut_ptr(),
            0,
        ) as usize;
        c.ok(bn > 0 && bn < bb.len(), bn as i64)?;
        let busybox = S::from_wide(&bb[..bn]);
        let list = S::new()
            .add("\"")
            .add(busybox.as_str())
            .add("\" tar -t")
            .add(flag)
            .add("f \"")
            .add(archive.as_str())
            .add("\"");
        let mut out = [0u8; 1024];
        let listed = run_captured(list.as_str(), folder, &mut out);
        c.ok(listed.is_some(), 0)?;
        let (code, len) = listed.unwrap_or((0, 0));
        let out = &out[..len];
        Line::new().s(NAME.as_bytes()).s(b": listed: ").flat(out).s(b"\r\n").print();
        c.eq(code.into(), 0)?;
        for (name, _) in FILES {
            c.ok(contains(out, name.as_bytes()), 0)?;
        }
        let extract = S::new()
            .add("\"")
            .add(busybox.as_str())
            .add("\" tar -xO")
            .add(flag)
            .add("f \"")
            .add(archive.as_str())
            .add("\" ");
        // The member as tar -t named it (it may carry a ./ before it).
        let member = out
            .split(|&b| b == b'\n' || b == b'\r')
            .find(|l| l.ends_with(FILES[1].0.as_bytes()))
            .and_then(|l| core::str::from_utf8(l).ok())
            .unwrap_or(FILES[1].0);
        let extract = extract.add(member);
        let mut content = [0u8; 256];
        let given = run_captured(extract.as_str(), folder, &mut content);
        c.ok(given.is_some(), 0)?;
        let (code, len) = given.unwrap_or((0, 0));
        c.eq(code.into(), 0)?;
        c.ok(&content[..len] == FILES[1].1, len as i64)?;
        say(b"archive checked");

        // The panel's Hide is the last click: the screen scenario checks
        // the steps before it.
        let hide = find("HideButton");
        c.ok(hide.is_some(), 0)?;
        uia_client::announce(NAME, "HideButton", &hide.unwrap_or_else(|| unreachable!()));
        say(b"answer listed");
        c.ok(wait(STEP_MS, || find("DockTab").is_some()), 0)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    match run() {
        Ok(()) => {
            say(b"ok");
            // SAFETY: plain call.
            unsafe { ExitProcess(0) }
        }
        // SAFETY: plain call.
        Err(n) => unsafe { ExitProcess(n) },
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xc000_0409) }
}
