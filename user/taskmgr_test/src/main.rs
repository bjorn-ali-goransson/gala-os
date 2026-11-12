//! The task manager (ROADMAP TM steps 1-2): the kernel's one list of
//! every process of both personalities, and gala's `taskmgr.exe` ending
//! one of each from it. gala's own (GalaTasks, `nt::tasks`): on Windows,
//! whose ntdll has no GalaTasks, it says so and passes.
//!
//! What runs: a Windows program with a window (this program's `busy`
//! mode: "Busy window", and a thread that keeps one processor busy) and
//! a WSL distribution `bb` (busybox, imported from `bb-rootfs.tar` beside
//! this program) running `sh -c "sleep 600 & trap '' TERM; cat; echo
//! cat ended"`: a shell that ignores SIGTERM, its `sleep` and its `cat`
//! (a command after `cat`, or busybox's sh would exec it in its own
//! place).
//!
//! Step 1, the query (GalaTasks op 0), checked here:
//! - every process is listed once, this one as a Windows process with
//!   its image, command line, user `gala`, threads, handles, start time;
//! - the busy program has its window (the flag, the HWND, the title)
//!   and its CPU time grows by about the second it spins;
//! - sh, sleep and cat are Linux processes of distribution `bb`, with
//!   their `comm`, argv as the command line, parents (sleep's and cat's
//!   is sh), user `gala` (uid 1000), and their open fds;
//! - NtQuerySystemInformation(SystemProcessInformation) lists the
//!   Windows processes and none of the Linux ones (Windows' contents);
//! - the load (op 1): one pair per processor, the memory, a processor
//!   busy over the second the busy thread spins;
//! - a signal for a Windows process is STATUS_INVALID_PARAMETER (those
//!   end through TerminateProcess), for no process STATUS_INVALID_CID;
//! - `gala ps` lists the Linux processes with their distribution.
//!
//! Then it prints `taskmgr_test: ready` and the screen check (scenario
//! `taskmgr`) opens Task Manager with Ctrl+Shift+Esc and ends, from it:
//! the busy window (TerminateProcess: exit code 1, as Windows' Task
//! Manager's End task), `sleep` (SIGTERM ends it), `sh` (SIGTERM is
//! ignored: SIGKILL after the grace time) and the distribution's row
//! (its whole tree: `cat`, and the `wsl.exe` that ran it returns). This
//! program follows each through the query and says so
//! (`taskmgr_test: busy window ended`, `sleep ended`, `sh ended`,
//! `distribution ended`), then that Task Manager, one instance, is
//! closed (Alt+F4: `task manager closed`).
//!
//! Prints `taskmgr_test: ok`, or exits with the number of the failing
//! check.

#![no_std]
#![no_main]

extern crate alloc;

use gala_heap as _;

use alloc::string::String;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::fmt::Write;

use taskmgr::{FLAG_LINUX, FLAG_WINDOW, Proc};

type Bool = i32;
type Handle = *mut c_void;
type Hwnd = *mut c_void;

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
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CloseHandle(h: Handle) -> Bool;
    fn GetModuleFileNameW(module: Handle, name: *mut u16, size: u32) -> u32;
    fn GetModuleHandleA(name: *const u8) -> Handle;
    fn GetProcAddress(module: Handle, name: *const u8) -> *mut c_void;
    fn GetEnvironmentVariableW(name: *const u16, buf: *mut u16, size: u32) -> u32;
    fn GetCommandLineW() -> *const u16;
    fn GetCurrentProcessId() -> u32;
    fn CreateDirectoryW(name: *const u16, sa: *mut c_void) -> Bool;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn CreatePipe(
        read: *mut Handle,
        write: *mut Handle,
        sa: *const SecurityAttributes,
        size: u32,
    ) -> Bool;
    fn SetHandleInformation(h: Handle, mask: u32, flags: u32) -> Bool;
    fn Sleep(ms: u32);
    fn GetTickCount() -> u32;
    fn CreateThread(
        sa: *mut c_void,
        stack: usize,
        start: extern "system" fn(*mut c_void) -> u32,
        param: *mut c_void,
        flags: u32,
        id: *mut u32,
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
    fn NtQuerySystemInformation(class: u32, buf: *mut u8, len: u32, ret: *mut u32) -> u32;
}

#[repr(C)]
struct WndClassW {
    style: u32,
    proc_: extern "system" fn(Hwnd, u32, usize, isize) -> isize,
    class_extra: i32,
    window_extra: i32,
    instance: Handle,
    icon: Handle,
    cursor: Handle,
    background: Handle,
    menu_name: *const u16,
    class_name: *const u16,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: [i32; 2],
    private: u32,
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegisterClassW(class: *const WndClassW) -> u16;
    #[allow(clippy::too_many_arguments)]
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Hwnd,
        menu: Handle,
        instance: Handle,
        param: *mut c_void,
    ) -> Hwnd;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostQuitMessage(code: i32);
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn __chkstk();
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    fn strlen(s: *const u8) -> usize;
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const NAME: &str = "taskmgr_test";
const STARTF_USESTDHANDLES: u32 = 0x100;
const HANDLE_FLAG_INHERIT: u32 = 1;
const SYSTEM_PROCESS_INFORMATION: u32 = 5;
const STATUS_INVALID_PARAMETER: u32 = 0xc000_000d;
const STATUS_INVALID_CID: u32 = 0xc000_000b;
const STATUS_BUFFER_TOO_SMALL: u32 = 0xc000_0023;
/// How long the screen check may take to end each of them.
const END_WAIT_MS: u32 = 300_000;

type GalaTasksFn = unsafe extern "system" fn(u32, usize, *mut u8, u32, *mut u32) -> u32;

// ---- Output ----------------------------------------------------------------

fn print(text: &str) {
    let mut written = 0;
    // SAFETY: a valid buffer and out pointer.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            text.as_ptr(),
            text.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        )
    };
}

fn line(text: &str) {
    let mut s = String::from(NAME);
    s.push_str(": ");
    s.push_str(text);
    s.push_str("\r\n");
    print(&s);
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, what: &str) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        let mut s = String::new();
        let _ = write!(
            s,
            "check {} failed (line {}): {what}",
            self.0,
            core::panic::Location::caller().line()
        );
        line(&s);
        Err(self.0)
    }
}

// ---- Text and processes ----------------------------------------------------

fn wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

fn env(name: &str) -> Option<String> {
    let mut buf = [0u16; 512];
    let n = wide(name);
    // SAFETY: a valid buffer of its stated size.
    let len = unsafe { GetEnvironmentVariableW(n.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    (len > 0 && (len as usize) < buf.len()).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

/// This program's path.
fn me() -> String {
    let mut b = [0u16; 260];
    // SAFETY: our buffer.
    let n = unsafe { GetModuleFileNameW(core::ptr::null_mut(), b.as_mut_ptr(), 260) } as usize;
    String::from_utf16_lossy(&b[..n])
}

/// This program's directory, with its trailing backslash.
fn here() -> String {
    let path = me();
    match path.rfind('\\') {
        Some(i) => String::from(&path[..=i]),
        None => String::new(),
    }
}

/// A pipe; (read, write), neither inherited.
fn pipe() -> (Handle, Handle) {
    let sa = SecurityAttributes {
        length: core::mem::size_of::<SecurityAttributes>() as u32,
        descriptor: core::ptr::null_mut(),
        inherit: 0,
    };
    let (mut r, mut w) = (core::ptr::null_mut(), core::ptr::null_mut());
    // SAFETY: valid out pointers.
    unsafe { CreatePipe(&mut r, &mut w, &sa, 65536) };
    (r, w)
}

/// Starts `exe` with `args`; with `stdio` its standard handles (made
/// inheritable for it). The process handle.
fn start(exe: &str, args: &str, stdio: Option<[Handle; 3]>) -> Option<Handle> {
    let mut cmd = String::from("\"");
    cmd.push_str(exe);
    cmd.push_str("\" ");
    cmd.push_str(args);
    let mut cmd = wide(&cmd);
    let app = wide(exe);
    let mut startup = [0u8; 104];
    startup[0] = 104;
    if let Some(handles) = stdio {
        startup[60..64].copy_from_slice(&STARTF_USESTDHANDLES.to_le_bytes());
        for (i, h) in handles.iter().enumerate() {
            // SAFETY: a handle this program holds.
            unsafe { SetHandleInformation(*h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
            startup[80 + 8 * i..88 + 8 * i].copy_from_slice(&(*h as u64).to_le_bytes());
        }
    }
    let mut info = [0usize; 3];
    let null = core::ptr::null_mut();
    // SAFETY: valid strings, STARTUPINFOW and PROCESS_INFORMATION.
    let ok = unsafe {
        CreateProcessW(
            app.as_ptr(),
            cmd.as_mut_ptr(),
            null,
            null,
            i32::from(stdio.is_some()),
            0,
            null,
            core::ptr::null(),
            startup.as_ptr(),
            &mut info,
        )
    };
    if ok == 0 {
        return None;
    }
    // SAFETY: the thread handle is not needed.
    unsafe { CloseHandle(info[1] as Handle) };
    Some(info[0] as Handle)
}

/// Runs `exe args` to its end: (exit code, standard output).
fn run(exe: &str, args: &str) -> (u32, Vec<u8>) {
    let (out_r, out_w) = pipe();
    // SAFETY: plain calls.
    let stdin = unsafe { GetStdHandle(-10i32 as u32) };
    let Some(h) = start(exe, args, Some([stdin, out_w, out_w])) else {
        return (0xdead_0000, Vec::new());
    };
    // SAFETY: our write end (the child has its own).
    unsafe { CloseHandle(out_w) };
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let mut got = 0;
        // SAFETY: a valid buffer and out pointer.
        let ok =
            unsafe { ReadFile(out_r, buf.as_mut_ptr(), 4096, &mut got, core::ptr::null_mut()) };
        if ok == 0 || got == 0 {
            break;
        }
        out.extend_from_slice(&buf[..got as usize]);
    }
    let mut code = 0xdead_0001;
    // SAFETY: handles this program holds.
    unsafe {
        if WaitForSingleObject(h, 60_000) == 0 {
            GetExitCodeProcess(h, &mut code);
        }
        CloseHandle(h);
        CloseHandle(out_r);
    }
    (code, out)
}

// ---- GalaTasks ---------------------------------------------------------------

struct Tasks(GalaTasksFn);

impl Tasks {
    /// GalaTasks if this ntdll has it (gala's).
    fn find() -> Option<Tasks> {
        // SAFETY: plain calls with NUL-terminated names.
        let f = unsafe {
            GetProcAddress(
                GetModuleHandleA(c"ntdll.dll".as_ptr().cast()),
                c"GalaTasks".as_ptr().cast(),
            )
        };
        // SAFETY: GalaTasks' signature (testntdll's stub).
        (!f.is_null())
            .then(|| Tasks(unsafe { core::mem::transmute::<*mut c_void, GalaTasksFn>(f) }))
    }

    /// Op `op`'s answer, grown until it fits.
    fn query(&self, op: u32) -> Result<Vec<u8>, u32> {
        let mut size = 16 * 1024;
        loop {
            let mut buf = alloc::vec![0u8; size];
            let mut needed = 0;
            // SAFETY: a buffer of its stated size and an out pointer.
            let s = unsafe { (self.0)(op, 0, buf.as_mut_ptr(), size as u32, &mut needed) };
            if s == STATUS_BUFFER_TOO_SMALL {
                size = needed as usize + 4096;
                continue;
            }
            if s != 0 {
                return Err(s);
            }
            buf.truncate(needed as usize);
            return Ok(buf);
        }
    }

    fn processes(&self) -> Vec<Proc> {
        self.query(0).ok().and_then(|b| taskmgr::parse_processes(&b)).map_or(Vec::new(), |r| r.2)
    }

    fn signal(&self, pid: u64, sig: u32) -> u32 {
        // SAFETY: no buffer.
        unsafe { (self.0)(2, pid as usize, core::ptr::null_mut(), sig, core::ptr::null_mut()) }
    }
}

/// The processes the query lists that `f` picks, waiting up to `ms` for
/// at least `n`.
fn wait_for(tasks: &Tasks, ms: u32, n: usize, f: impl Fn(&Proc) -> bool) -> Vec<Proc> {
    // SAFETY: plain calls.
    let start = unsafe { GetTickCount() };
    loop {
        let found: Vec<Proc> = tasks.processes().into_iter().filter(|p| f(p)).collect();
        // SAFETY: plain call.
        if found.len() >= n || unsafe { GetTickCount() }.wrapping_sub(start) > ms {
            return found;
        }
        // SAFETY: plain call.
        unsafe { Sleep(200) };
    }
}

/// Waits up to `ms` for process `pid` to leave the query's list.
fn wait_gone(tasks: &Tasks, pid: u64, ms: u32) -> bool {
    // SAFETY: plain calls.
    let start = unsafe { GetTickCount() };
    loop {
        if !tasks.processes().iter().any(|p| p.pid == pid) {
            return true;
        }
        // SAFETY: plain calls.
        if unsafe { GetTickCount() }.wrapping_sub(start) > ms {
            return false;
        }
        // SAFETY: plain call.
        unsafe { Sleep(250) };
    }
}

/// The process ids NtQuerySystemInformation lists.
fn nt_pids() -> Vec<u64> {
    let mut buf = alloc::vec![0u8; 256 * 1024];
    let mut ret = 0;
    // SAFETY: a buffer of its stated size.
    let s = unsafe {
        NtQuerySystemInformation(
            SYSTEM_PROCESS_INFORMATION,
            buf.as_mut_ptr(),
            buf.len() as u32,
            &mut ret,
        )
    };
    let mut out = Vec::new();
    if s != 0 {
        return out;
    }
    let mut at = 0usize;
    loop {
        let pid = u64::from_le_bytes(buf[at + 0x50..at + 0x58].try_into().unwrap_or([0; 8]));
        out.push(pid);
        let next = u32::from_le_bytes(buf[at..at + 4].try_into().unwrap_or([0; 4])) as usize;
        if next == 0 {
            break;
        }
        at += next;
    }
    out
}

// ---- The busy program --------------------------------------------------------

extern "system" fn busy_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    if msg == 2 {
        // WM_DESTROY.
        // SAFETY: plain call.
        unsafe { PostQuitMessage(0) };
        return 0;
    }
    // SAFETY: the default procedure.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

extern "system" fn spin(_: *mut c_void) -> u32 {
    let mut x: u64 = 1;
    loop {
        x = core::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
    }
}

/// `busy`: a window and a thread that keeps one processor busy, until
/// it is ended.
fn busy() -> u32 {
    let class = wide("TaskmgrTestBusy");
    let title = wide("Busy window");
    let wc = WndClassW {
        style: 0,
        proc_: busy_proc,
        class_extra: 0,
        window_extra: 0,
        instance: core::ptr::null_mut(),
        icon: core::ptr::null_mut(),
        cursor: core::ptr::null_mut(),
        background: 6usize as Handle, // COLOR_WINDOW + 1
        menu_name: core::ptr::null(),
        class_name: class.as_ptr(),
    };
    let null = core::ptr::null_mut();
    // SAFETY: valid class and strings; plain calls.
    unsafe {
        RegisterClassW(&wc);
        // WS_OVERLAPPEDWINDOW.
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            0x00cf_0000,
            860,
            520,
            360,
            200,
            null,
            null,
            null,
            null,
        );
        if hwnd.is_null() {
            return 2;
        }
        ShowWindow(hwnd, 5);
        CreateThread(null, 0, spin, null, 0, core::ptr::null_mut());
        let mut msg: Msg = core::mem::zeroed();
        while GetMessageW(&mut msg, core::ptr::null_mut(), 0, 0) > 0 {
            DispatchMessageW(&msg);
        }
    }
    0
}

// ---- The test ------------------------------------------------------------------

fn find(procs: &[Proc], f: impl Fn(&Proc) -> bool) -> Option<&Proc> {
    procs.iter().find(|p| f(p))
}

fn test(tasks: &Tasks) -> Result<(), u32> {
    let mut c = Checks(0);
    let me_pid = u64::from(unsafe { GetCurrentProcessId() });
    let root = env("SystemRoot").unwrap_or(String::from("C:\\Windows"));
    let mut wsl = root.clone();
    wsl.push_str("\\System32\\wsl.exe");
    // The distribution.
    let dir = wide("C:\\taskmgr.d");
    // SAFETY: a valid path.
    unsafe { CreateDirectoryW(dir.as_ptr(), core::ptr::null_mut()) };
    let mut args = String::from("--import bb C:\\taskmgr.d\\bb ");
    args.push_str(&here());
    args.push_str("bb-rootfs.tar");
    let (code, _) = run(&wsl, &args);
    c.ok(code == 0, "wsl --import bb")?;
    // Its processes: sh ignoring SIGTERM, a sleep and a cat (reading a
    // pipe this program keeps open).
    let (in_r, _in_w) = pipe();
    // SAFETY: plain calls.
    let (out, err) = unsafe { (GetStdHandle(-11i32 as u32), GetStdHandle(-12i32 as u32)) };
    let linux = start(
        &wsl,
        "-d bb --exec /bin/sh -c \"sleep 600 & trap '' TERM; cat; echo cat ended\"",
        Some([in_r, out, err]),
    );
    c.ok(linux.is_some(), "wsl -d bb started")?;
    let linux = linux.unwrap_or(core::ptr::null_mut());
    // The busy program.
    let busy = start(&me(), "busy", None);
    c.ok(busy.is_some(), "the busy program started")?;
    let busy = busy.unwrap_or(core::ptr::null_mut());

    let listed = wait_for(tasks, 60_000, 3, |p| p.distro == "bb");
    c.ok(listed.len() >= 3, "sh, sleep and cat listed in bb")?;
    let apps = wait_for(tasks, 60_000, 1, |p| p.title == "Busy window");
    c.ok(apps.len() == 1, "the busy window listed")?;
    let first = tasks.processes();
    // SAFETY: plain call.
    unsafe { Sleep(1000) };
    let procs = tasks.processes();
    let perf0 = tasks.query(1).ok().and_then(|b| taskmgr::parse_perf(&b));

    // Each process once.
    let mut pids: Vec<u64> = procs.iter().map(|p| p.pid).collect();
    pids.sort_unstable();
    let n = pids.len();
    pids.dedup();
    c.ok(n == pids.len(), "each process once")?;
    // This one.
    let mine = find(&procs, |p| p.pid == me_pid);
    c.ok(mine.is_some(), "this process listed")?;
    let mine = mine.ok_or(c.0)?;
    c.ok(mine.image == "taskmgr_test.exe" && mine.flags & FLAG_LINUX == 0, "this one's image")?;
    c.ok(mine.command_line.contains("taskmgr_test"), "this one's command line")?;
    c.ok(mine.user == "gala", "this one's user")?;
    c.ok(mine.threads >= 1 && mine.handles > 0, "this one's threads and handles")?;
    c.ok(mine.create_time > 0 && mine.working_set > 0, "this one's start and memory")?;
    // The busy program: an app, its CPU time grows.
    let b1 = find(&procs, |p| p.title == "Busy window").ok_or(c.0)?;
    let b0 = find(&first, |p| p.pid == b1.pid).ok_or(c.0)?;
    c.ok(b1.flags & FLAG_WINDOW != 0 && b1.hwnd != 0, "the busy program's window")?;
    c.ok(b1.parent == me_pid && b1.threads >= 2, "the busy program's parent and threads")?;
    c.ok(b1.cpu_ns > b0.cpu_ns + 300_000_000, "the busy program's CPU time grows")?;
    // The Linux ones.
    let sh = find(&procs, |p| p.distro == "bb" && p.image == "sh").ok_or(c.0)?;
    let sleep = find(&procs, |p| p.distro == "bb" && p.image == "sleep").ok_or(c.0)?;
    let cat = find(&procs, |p| p.distro == "bb" && p.image == "cat").ok_or(c.0)?;
    c.ok(sh.flags & FLAG_LINUX != 0 && sleep.flags & FLAG_LINUX != 0, "Linux processes")?;
    c.ok(sleep.command_line == "sleep 600", "sleep's command line")?;
    c.ok(cat.command_line == "cat", "cat's command line")?;
    c.ok(sh.command_line.starts_with("/bin/sh -c"), "sh's command line")?;
    c.ok(sleep.parent == sh.pid && cat.parent == sh.pid, "sleep's and cat's parent is sh")?;
    c.ok(sleep.user == "gala" && sleep.uid == 1000, "the Linux processes' user")?;
    c.ok(cat.handles >= 3, "cat's fds")?;
    let wsl_proc = find(&procs, |p| p.image == "wsl.exe" && p.parent == me_pid);
    c.ok(wsl_proc.is_some_and(|p| p.flags & FLAG_LINUX == 0), "wsl.exe a Windows process")?;
    // Windows' own list: no Linux process.
    let nt = nt_pids();
    c.ok(nt.contains(&me_pid) && nt.contains(&b1.pid), "NtQuerySystemInformation's Windows ones")?;
    c.ok(
        !nt.contains(&sh.pid) && !nt.contains(&sleep.pid) && !nt.contains(&cat.pid),
        "NtQuerySystemInformation without the Linux ones",
    )?;
    // The load.
    let perf0 = perf0.ok_or(c.0)?;
    c.ok(!perf0.cpus.is_empty(), "processors")?;
    c.ok(perf0.memory_total > perf0.memory_used && perf0.memory_used > 0, "memory")?;
    c.ok(perf0.processes as usize >= procs.len().saturating_sub(2), "the process count")?;
    // SAFETY: plain call.
    unsafe { Sleep(1000) };
    let perf1 = tasks.query(1).ok().and_then(|b| taskmgr::parse_perf(&b)).ok_or(c.0)?;
    let busiest = perf0
        .cpus
        .iter()
        .zip(&perf1.cpus)
        .map(|(a, b)| taskmgr::cpu_busy(*a, *b))
        .max()
        .unwrap_or(0);
    c.ok(busiest >= 500, "a processor busy")?;
    // Signals: not for Windows processes, nor for nobody.
    c.ok(tasks.signal(b1.pid, 15) == STATUS_INVALID_PARAMETER, "a signal for a Windows process")?;
    c.ok(tasks.signal(0x7fff_fff0, 15) == STATUS_INVALID_CID, "a signal for no process")?;
    // `gala ps`.
    let (code, out) = run("C:\\gala\\gala.exe", "ps");
    let text = String::from_utf8_lossy(&out);
    let has_sleep =
        text.lines().any(|l| l.contains("sleep") && l.contains("Linux") && l.contains("bb"));
    c.ok(code == 0 && has_sleep, "gala ps lists sleep in bb")?;

    let (sh_pid, sleep_pid, cat_pid) = (sh.pid, sleep.pid, cat.pid);
    line("ready");

    // What the screen check ends from Task Manager.
    let mut code = 0;
    // SAFETY: handles this program holds.
    unsafe {
        c.ok(WaitForSingleObject(busy, END_WAIT_MS) == 0, "the busy window ended")?;
        GetExitCodeProcess(busy, &mut code);
        CloseHandle(busy);
    }
    let mut s = String::new();
    let _ = write!(s, "busy window ended, exit code {code}");
    line(&s);
    c.ok(code == 1, "End task's exit code")?;
    c.ok(wait_gone(tasks, sleep_pid, END_WAIT_MS), "sleep ended")?;
    line("sleep ended");
    c.ok(wait_gone(tasks, sh_pid, END_WAIT_MS), "sh ended")?;
    line("sh ended");
    c.ok(tasks.processes().iter().any(|p| p.pid == cat_pid), "cat still there")?;
    c.ok(wait_gone(tasks, cat_pid, END_WAIT_MS), "cat ended")?;
    // SAFETY: a handle this program holds.
    c.ok(unsafe { WaitForSingleObject(linux, 60_000) } == 0, "wsl.exe returned")?;
    line("distribution ended");
    // The screen check closes Task Manager last (Alt+F4), after its
    // final screendump.
    let tm = wait_for(tasks, 60_000, 1, |p| p.image.eq_ignore_ascii_case("taskmgr.exe"));
    c.ok(tm.len() == 1, "Task Manager listed (one instance)")?;
    c.ok(wait_gone(tasks, tm[0].pid, END_WAIT_MS), "Task Manager closed")?;
    line("task manager closed");
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    // SAFETY: the process's command line.
    let cmd = unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(core::slice::from_raw_parts(p, n))
    };
    if cmd.trim_end().ends_with(" busy") {
        // SAFETY: ends the process.
        unsafe { ExitProcess(busy()) };
    }
    let Some(tasks) = Tasks::find() else {
        line("not gala (no GalaTasks in ntdll): skipped");
        line("ok");
        // SAFETY: ends the process.
        unsafe { ExitProcess(0) };
    };
    let code = match test(&tasks) {
        Ok(()) => {
            line("ok");
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    line("panic");
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}
