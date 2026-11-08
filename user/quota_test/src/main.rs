//! Pool and handle quotas (ROADMAP R6), with the results real Windows
//! gives: a process that leaks handles fails at its handle limit
//! (0xff0000 on Windows, measured: 2^24 - 2^16, where "Kernel Objects" on
//! Microsoft Learn documents 2^24) with ERROR_NO_SYSTEM_RESOURCES
//! (measured too); GetProcessHandleCount and
//! NtQueryInformationProcess(ProcessHandleCount) count every handle; the
//! handle entries are charged to its paged pool quota
//! (PROCESS_MEMORY_COUNTERS.QuotaPagedPoolUsage); a second process created
//! before the leak is unaffected (it creates objects while the first is
//! at its limit); closing the leaked handles lets the first create
//! objects again.
//!
//! `quota_test <handles> <nonpaged bytes>` (gala's scenario, which boots
//! with `quota=handles:<handles>,nonpaged:<bytes>`): the handle limit is
//! exactly `<handles>`, ProcessQuotaLimits reports the nonpaged limit, and
//! a second leak, of new event objects, fails with ERROR_NOT_ENOUGH_QUOTA
//! (STATUS_QUOTA_EXCEEDED) with QuotaNonPagedPoolUsage at most the limit;
//! closing them gives the objects' charge back. (Windows' default pool
//! quotas are not reachable by a test, and a test cannot lower them:
//! without arguments that part is skipped and the handle limit is
//! checked to be Windows' 0xff0000.)
//!
//! In that mode a completion port's queued packets are charged too:
//! PostQueuedCompletionStatus without taking fails with
//! ERROR_NOT_ENOUGH_QUOTA once the nonpaged pool quota is reached (a
//! packet is nonpaged pool on Windows), and taking them gives it back.
//!
//! Sockets (every mode): SO_RCVBUF and SO_SNDBUF set to 1 MiB read back
//! as 1 MiB; connected sockets add to QuotaNonPagedPoolUsage (their
//! endpoints and buffers); the children connect sockets of their own
//! while this process holds its. `quota_test sockets <nonpaged bytes>`
//! (gala's `quota_sockets` scenario, booted with `quota=nonpaged:<bytes>`):
//! connections with 1 MiB buffers each way are made until one is refused
//! with WSAENOBUFS (the buffers over the process's quota; gala charges a
//! socket's buffers whole when it is made), with QuotaNonPagedPoolUsage at
//! most the limit; a child makes its own meanwhile; closing them gives
//! the charge back and a connection can be made again. Then
//! NtSetInformationProcess(ProcessQuotaLimits) is checked: changing a
//! pool limit needs SeIncreaseQuotaPrivilege, which a standard user lacks
//! (STATUS_PRIVILEGE_NOT_HELD); the working set's sizes are kept and
//! read back.
//!
//! Prints `quota_test: ok` or exits with the number of the failing check;
//! `quota_test: handle limit at N`, `quota_test: pool quota at N
//! objects`, `quota_test: port quota at N packets` and `quota_test:
//! socket quota at N connections` report what was reached.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

const MEM_COMMIT: u32 = 0x1000;
const MEM_RESERVE: u32 = 0x2000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_READWRITE: u32 = 0x04;
const DUPLICATE_SAME_ACCESS: u32 = 2;
const CREATE_SUSPENDED: u32 = 4;
const ERROR_NO_SYSTEM_RESOURCES: u32 = 1450;
const ERROR_NOT_ENOUGH_QUOTA: u32 = 1816;
/// The handle limit real Windows (25H2, build 26200) stops a process at:
/// 0xff0000 = 2^24 - 2^16, a little under the documented 2^24.
const WINDOWS_HANDLE_LIMIT: u64 = 0xff_0000;
/// Duplicates the leak stops at whatever happens (a little over 2^24).
const MAX_LEAK: usize = (1 << 24) + 4096;
/// Objects the children create.
const CHILD_OBJECTS: usize = 64;
const CHILD_OK: u32 = 0x51;
/// Socket constants (winsock2.h).
const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const SOL_SOCKET: i32 = 0xffff;
const SO_SNDBUF: i32 = 0x1001;
const SO_RCVBUF: i32 = 0x1002;
const INVALID_SOCKET: Socket = !0;
const WSAENOBUFS: i32 = 10055;
/// The buffer size the sockets ask for, each way.
const BIG_BUFFER: i32 = 1 << 20;
/// Connections the socket parts make at most (Windows: a few).
const MAX_PAIRS: usize = 64;
const WINDOWS_PAIRS: usize = 4;
const STATUS_PRIVILEGE_NOT_HELD: i32 = 0xc000_0061_u32 as i32;
/// ProcessQuotaLimits.
const PROCESS_QUOTA_LIMITS: u32 = 1;

type Socket = usize;

#[repr(C)]
#[derive(Clone, Copy)]
struct SockaddrIn {
    family: u16,
    port: u16,
    addr: [u8; 4],
    zero: [u8; 8],
}

#[repr(C)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    process_id: u32,
    thread_id: u32,
}

/// A child is ended and its handles closed whichever way the test leaves
/// (a failed check returns early): a child left suspended would outlive
/// the test, holding what it inherited (run-all.ps1's output pipe).
impl Drop for ProcessInformation {
    fn drop(&mut self) {
        // SAFETY: the handles CreateProcessW gave (or null, which fails).
        unsafe {
            if !self.process.is_null() {
                TerminateProcess(self.process, 1);
                CloseHandle(self.process);
            }
            if !self.thread.is_null() {
                CloseHandle(self.thread);
            }
        }
    }
}

#[repr(C)]
struct StartupInfo {
    cb: u32,
    rest: [u8; 100],
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn VirtualAlloc(at: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(at: *mut c_void, size: usize, kind: u32) -> Bool;
    fn K32GetProcessMemoryInfo(process: Handle, counters: *mut u64, size: u32) -> Bool;
    fn GetProcessHandleCount(process: Handle, count: *mut u32) -> Bool;
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn GetLastError() -> u32;
    fn SetLastError(code: u32);
    fn GetCurrentProcess() -> Handle;
    fn GetCommandLineW() -> *const u16;
    fn GetModuleFileNameW(module: Handle, buf: *mut u16, size: u32) -> u32;
    fn CreateEventW(
        attributes: *const c_void,
        manual: Bool,
        initial: Bool,
        name: *const u16,
    ) -> Handle;
    fn DuplicateHandle(
        source_process: Handle,
        source: Handle,
        target_process: Handle,
        target: *mut Handle,
        access: u32,
        inherit: Bool,
        options: u32,
    ) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn CreateProcessW(
        application: *const u16,
        command_line: *mut u16,
        process_attributes: *const c_void,
        thread_attributes: *const c_void,
        inherit: Bool,
        flags: u32,
        environment: *const c_void,
        directory: *const u16,
        startup: *const StartupInfo,
        info: *mut ProcessInformation,
    ) -> Bool;
    fn ResumeThread(h: Handle) -> u32;
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn TerminateProcess(h: Handle, code: u32) -> Bool;
    fn ExitProcess(code: u32) -> !;
    fn CreateIoCompletionPort(file: Handle, port: Handle, key: usize, threads: u32) -> Handle;
    fn PostQueuedCompletionStatus(port: Handle, bytes: u32, key: usize, ov: usize) -> Bool;
    fn GetQueuedCompletionStatus(
        port: Handle,
        bytes: *mut u32,
        key: *mut usize,
        ov: *mut usize,
        ms: u32,
    ) -> Bool;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        process: Handle,
        class: u32,
        buffer: *mut c_void,
        length: u32,
        ret: *mut u32,
    ) -> i32;
    fn NtSetInformationProcess(
        process: Handle,
        class: u32,
        buffer: *const c_void,
        length: u32,
    ) -> i32;
}

#[link(name = "ws2_32", kind = "raw-dylib")]
unsafe extern "system" {
    fn WSAStartup(version: u16, data: *mut u8) -> i32;
    fn WSAGetLastError() -> i32;
    fn socket(af: i32, kind: i32, protocol: i32) -> Socket;
    fn bind(s: Socket, addr: *const SockaddrIn, len: i32) -> i32;
    fn listen(s: Socket, backlog: i32) -> i32;
    fn accept(s: Socket, addr: *mut SockaddrIn, len: *mut i32) -> Socket;
    fn connect(s: Socket, addr: *const SockaddrIn, len: i32) -> i32;
    fn getsockname(s: Socket, addr: *mut SockaddrIn, len: *mut i32) -> i32;
    fn setsockopt(s: Socket, level: i32, name: i32, value: *const i32, len: i32) -> i32;
    fn getsockopt(s: Socket, level: i32, name: i32, value: *mut i32, len: *mut i32) -> i32;
    fn closesocket(s: Socket) -> i32;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

// What core's code links against (as the other socket tests have it).
#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

fn out(s: &[u8]) {
    let mut written = 0;
    // SAFETY: writes `s` to the standard output.
    unsafe {
        WriteFile(
            GetStdHandle(-11i32 as u32),
            s.as_ptr(),
            s.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        );
    }
}

/// `v` in decimal into `buf`; the digits.
fn decimal(mut v: u64, buf: &mut [u8; 20]) -> &[u8] {
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            return &buf[i..];
        }
    }
}

/// One output line `prefix N suffix` (one WriteFile: the kernel's log
/// shares the serial port).
fn line(prefix: &[u8], n: u64, suffix: &[u8]) {
    let mut text = [0u8; 160];
    let mut digits = [0u8; 20];
    let n = decimal(n, &mut digits);
    let mut at = 0;
    for part in [prefix, n, suffix, b"\r\n"] {
        let len = part.len().min(text.len() - at);
        text[at..at + len].copy_from_slice(&part[..len]);
        at += len;
    }
    out(&text[..at]);
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.ok_with(cond, got, &[])
    }

    /// As `ok`, with named values that explain the failure appended to
    /// the same line (`, base 34 n 16711641`).
    #[track_caller]
    fn ok_with(&mut self, cond: bool, got: u64, extra: &[(&[u8], u64)]) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        // One line: run-all.ps1 keeps the last line with the program's
        // name, so the check, its value and its source line go together.
        let (at, at_len) = line_text(core::panic::Location::caller().line());
        let mut text = [0u8; 256];
        let mut digits = [0u8; 20];
        let mut n = 0;
        let check = decimal(self.0.into(), &mut digits);
        let mut more = [0u8; 20];
        let value = decimal(got, &mut more);
        let mut put = |part: &[u8]| {
            let len = part.len().min(text.len() - 2 - n);
            text[n..n + len].copy_from_slice(&part[..len]);
            n += len;
        };
        for part in [&b"quota_test: check "[..], check, b" failed, got ", value, &at[..at_len]] {
            put(part);
        }
        for (i, (name, v)) in extra.iter().enumerate() {
            put(if i == 0 { b", " } else { b" " });
            put(name);
            put(b" ");
            let mut d = [0u8; 20];
            put(decimal(*v, &mut d));
        }
        text[n..n + 2].copy_from_slice(b"\r\n");
        out(&text[..n + 2]);
        Err(self.0)
    }
}

/// PROCESS_MEMORY_COUNTERS: (QuotaPagedPoolUsage, QuotaNonPagedPoolUsage).
fn pool_usage() -> (u64, u64) {
    let mut counters = [0u64; 9];
    // SAFETY: a PROCESS_MEMORY_COUNTERS (72 bytes).
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), counters.as_mut_ptr(), 72) };
    (counters[4], counters[6])
}

/// GetProcessHandleCount, and ProcessHandleCount (class 20) when both
/// agree; u64::MAX otherwise.
fn handle_count() -> u64 {
    let mut k32 = 0u32;
    let mut nt = 0u32;
    // SAFETY: plain calls with 4-byte outputs.
    let (a, b) = unsafe {
        (
            GetProcessHandleCount(GetCurrentProcess(), &mut k32),
            NtQueryInformationProcess(
                GetCurrentProcess(),
                20,
                (&raw mut nt).cast(),
                4,
                core::ptr::null_mut(),
            ),
        )
    };
    if a != 0 && b == 0 && k32 == nt { k32.into() } else { u64::MAX }
}

/// What the command line asks for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// No arguments (real Windows): the handle limit, sockets.
    Windows,
    /// `<handles> <nonpaged>` (gala): the configured limits, objects,
    /// completion packets.
    Limits(u64, u64),
    /// `sockets <nonpaged>` (gala): sockets' buffers over the quota.
    Sockets(u64),
    /// `child` (with `sockets`: `sockchild`): a child process.
    Child { sockets: bool },
}

/// The words after the program's name on the command line, read as a
/// mode.
fn arguments() -> Mode {
    let mut words = [[0u8; 16]; 2];
    let mut lens = [0usize; 2];
    let mut count = 0;
    // SAFETY: the process's command line, NUL-terminated.
    unsafe {
        let p = GetCommandLineW();
        let mut i = 0;
        let mut quoted = false;
        // Skip the program's name.
        loop {
            let c = *p.add(i);
            if c == 0 || (c == u16::from(b' ') && !quoted) {
                break;
            }
            if c == u16::from(b'"') {
                quoted = !quoted;
            }
            i += 1;
        }
        let mut in_word = false;
        loop {
            let c = *p.add(i);
            if c == 0 {
                break;
            }
            if c == u16::from(b' ') {
                if in_word {
                    count += 1;
                    in_word = false;
                }
            } else if count < 2 {
                in_word = true;
                if lens[count] < 16 {
                    words[count][lens[count]] = c as u8;
                    lens[count] += 1;
                }
            }
            i += 1;
        }
        if in_word {
            count += 1;
        }
    }
    let word = |n: usize| &words[n][..lens[n]];
    let number = |n: usize| {
        word(n)
            .iter()
            .try_fold(0u64, |v, &d| d.is_ascii_digit().then(|| v * 10 + u64::from(d - b'0')))
    };
    match (count, word(0)) {
        (0, _) => Mode::Windows,
        (_, b"child") => Mode::Child { sockets: false },
        (_, b"sockchild") => Mode::Child { sockets: true },
        (2, b"sockets") => Mode::Sockets(number(1).unwrap_or(0)),
        (2, _) => Mode::Limits(number(0).unwrap_or(0), number(1).unwrap_or(0)),
        _ => Mode::Windows,
    }
}

/// A listening socket on 127.0.0.1 (an ephemeral port) whose connections
/// get `buffer`-byte buffers (0: the default), and its address.
fn listener(c: &mut Checks, buffer: i32) -> Result<(Socket, SockaddrIn), u32> {
    let mut addr =
        SockaddrIn { family: AF_INET as u16, port: 0, addr: [127, 0, 0, 1], zero: [0; 8] };
    // SAFETY: winsock calls with valid buffers.
    unsafe {
        let l = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.ok(l != INVALID_SOCKET, WSAGetLastError() as u64)?;
        if buffer != 0 {
            buffers(c, l, buffer)?;
        }
        c.ok(bind(l, &addr, 16) == 0, WSAGetLastError() as u64)?;
        c.ok(listen(l, 64) == 0, WSAGetLastError() as u64)?;
        let mut len = 16;
        c.ok(getsockname(l, &mut addr, &mut len) == 0, WSAGetLastError() as u64)?;
        Ok((l, addr))
    }
}

/// Sets SO_RCVBUF and SO_SNDBUF of `s` to `size`; checks they read back.
fn buffers(c: &mut Checks, s: Socket, size: i32) -> Result<(), u32> {
    for name in [SO_RCVBUF, SO_SNDBUF] {
        let mut got = 0i32;
        let mut len = 4;
        // SAFETY: 4-byte option values.
        unsafe {
            c.ok(setsockopt(s, SOL_SOCKET, name, &size, 4) == 0, WSAGetLastError() as u64)?;
            c.ok(getsockopt(s, SOL_SOCKET, name, &mut got, &mut len) == 0, 0)?;
        }
        c.ok(got == size && len == 4, got as u64)?;
    }
    Ok(())
}

/// One connection to `to` with `size`-byte buffers: (client, accepted),
/// or the connect's error (WSAGetLastError).
fn pair(
    c: &mut Checks,
    l: Socket,
    to: &SockaddrIn,
    size: i32,
) -> Result<Result<(Socket, Socket), i32>, u32> {
    // SAFETY: winsock calls with valid buffers.
    unsafe {
        let s = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        c.ok(s != INVALID_SOCKET, WSAGetLastError() as u64)?;
        buffers(c, s, size)?;
        if connect(s, to, 16) != 0 {
            let e = WSAGetLastError();
            closesocket(s);
            return Ok(Err(e));
        }
        let a = accept(l, core::ptr::null_mut(), core::ptr::null_mut());
        c.ok(a != INVALID_SOCKET, WSAGetLastError() as u64)?;
        Ok(Ok((s, a)))
    }
}

fn wsa_start(c: &mut Checks) -> Result<(), u32> {
    let mut data = [0u8; 512];
    // SAFETY: a WSADATA-sized buffer.
    c.ok(unsafe { WSAStartup(0x202, data.as_mut_ptr()) } == 0, 0)
}

/// The child's sockets: a listener and one connection with big buffers.
fn child_sockets() -> Result<(), u32> {
    let mut c = Checks(0);
    wsa_start(&mut c)?;
    let (l, addr) = listener(&mut c, BIG_BUFFER)?;
    let (s, a) = pair(&mut c, l, &addr, BIG_BUFFER)?.map_err(|e| e as u32)?;
    // SAFETY: the sockets made above.
    unsafe {
        closesocket(s);
        closesocket(a);
        closesocket(l);
    }
    Ok(())
}

/// `quota_test child`: creates objects and handles (and with `sockets`
/// connects sockets with big buffers); exits CHILD_OK if it could.
fn child(sockets: bool) -> ! {
    let mut handles = [core::ptr::null_mut(); CHILD_OBJECTS];
    let mut ok = !sockets || child_sockets().is_ok();
    // SAFETY: plain calls.
    unsafe {
        for h in handles.iter_mut() {
            *h = CreateEventW(core::ptr::null(), 1, 0, core::ptr::null());
            ok &= !h.is_null();
        }
        for &h in handles.iter() {
            let mut dup = core::ptr::null_mut();
            let me = GetCurrentProcess();
            ok &= DuplicateHandle(me, h, me, &mut dup, 0, 0, DUPLICATE_SAME_ACCESS) != 0;
            CloseHandle(dup);
            CloseHandle(h);
        }
        ExitProcess(if ok { CHILD_OK } else { 0xbad })
    }
}

/// Starts `quota_test child` (with `sockets`: `sockchild`) suspended.
fn start_child(c: &mut Checks, sockets: bool) -> Result<ProcessInformation, u32> {
    let mut path = [0u16; 260];
    let mut line = [0u16; 300];
    let mut info = ProcessInformation {
        process: core::ptr::null_mut(),
        thread: core::ptr::null_mut(),
        process_id: 0,
        thread_id: 0,
    };
    // SAFETY: documented calls with valid buffers.
    unsafe {
        let n = GetModuleFileNameW(core::ptr::null_mut(), path.as_mut_ptr(), 260) as usize;
        c.ok(n > 0 && n < 260, n as u64)?;
        line[0] = u16::from(b'"');
        line[1..1 + n].copy_from_slice(&path[..n]);
        let arg: &[u8] = if sockets { b"\" sockchild" } else { b"\" child" };
        for (i, &b) in arg.iter().enumerate() {
            line[1 + n + i] = u16::from(b);
        }
        let startup = StartupInfo { cb: 104, rest: [0; 100] };
        let created = CreateProcessW(
            path.as_ptr(),
            line.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            0,
            CREATE_SUSPENDED,
            core::ptr::null(),
            core::ptr::null(),
            &startup,
            &mut info,
        );
        c.ok(created != 0, GetLastError().into())?;
    }
    Ok(info)
}

/// Lets `child` run; checks it could create its objects.
fn child_runs(c: &mut Checks, child: &ProcessInformation) -> Result<(), u32> {
    let mut code = 0;
    // SAFETY: the child's handles, held since its creation.
    unsafe {
        c.ok(ResumeThread(child.thread) == 1, GetLastError().into())?;
        c.ok(WaitForSingleObject(child.process, 30_000) == 0, GetLastError().into())?;
        c.ok(GetExitCodeProcess(child.process, &mut code) != 0, GetLastError().into())?;
    }
    c.ok(code == CHILD_OK, code.into())
}

fn run() -> Result<(), u32> {
    let (handle_limit, nonpaged_limit) = match arguments() {
        Mode::Child { sockets } => child(sockets),
        Mode::Sockets(limit) => return sockets_over_quota(limit),
        Mode::Limits(handles, nonpaged) => (handles, nonpaged),
        Mode::Windows => (0, 0),
    };
    let gala = handle_limit != 0;
    let mut c = Checks(0);
    // SAFETY: Win32 calls on memory and handles this function owns.
    unsafe {
        let me = GetCurrentProcess();
        let base = handle_count();
        c.ok(base > 0 && base != u64::MAX, base)?;
        let (paged_before, nonpaged_before) = pool_usage();
        c.ok(paged_before > 0 && nonpaged_before > 0, paged_before)?;
        // Two processes, started before the leaks (starting one needs
        // handles), let run while this one is at its limits.
        // (Under gala's small configured quota a child's sockets would
        // not fit: only real Windows' children connect sockets here.)
        let first = start_child(&mut c, !gala)?;
        let second = start_child(&mut c, !gala)?;

        // Handles: duplicates of one event until refused.
        let event = CreateEventW(core::ptr::null(), 1, 0, core::ptr::null());
        c.ok(!event.is_null(), GetLastError().into())?;
        // Room for what can leak: a little over the limits.
        let cap = if gala {
            (handle_limit as usize).max(nonpaged_limit as usize / 64) + 64
        } else {
            MAX_LEAK
        };
        let room = cap * 8;
        let leaked =
            VirtualAlloc(core::ptr::null_mut(), room, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE)
                as *mut Handle;
        c.ok(!leaked.is_null(), GetLastError().into())?;
        // The loop is judged against the count just before it: whatever
        // else the process holds by then (on Windows the loader and
        // CreateProcess may keep handles of their own) is not the loop's.
        let before_loop = handle_count();
        let mut n = 0;
        let error = loop {
            let mut dup = core::ptr::null_mut();
            SetLastError(0);
            if DuplicateHandle(me, event, me, &mut dup, 0, 0, DUPLICATE_SAME_ACCESS) == 0 {
                break GetLastError();
            }
            *leaked.add(n) = dup;
            n += 1;
            if n == cap {
                break 0;
            }
        };
        c.ok(error == ERROR_NO_SYSTEM_RESOURCES, error.into())?;
        let at_limit = handle_count();
        line(b"quota_test: handle limit at ", at_limit, b" handles");
        let explain: [(&[u8], u64); 3] =
            [(b"base", base), (b"before_loop", before_loop), (b"n", n as u64)];
        c.ok_with(at_limit == before_loop + n as u64, at_limit, &explain)?;
        // gala: the two children (a process and a thread handle each) and
        // the event are all that was opened since `base`. On Windows other
        // handles may come and go meanwhile, so it is gala's claim only.
        if gala {
            c.ok_with(before_loop == base + 2 * 2 + 1, before_loop, &explain)?;
        }
        if gala {
            c.ok(at_limit == handle_limit, at_limit)?;
        } else {
            c.ok(at_limit == WINDOWS_HANDLE_LIMIT, at_limit)?;
        }
        // The entries are charged to the paged pool quota.
        let (paged_full, _) = pool_usage();
        c.ok(paged_full > paged_before, paged_full)?;
        // Again: still refused.
        let mut dup = core::ptr::null_mut();
        c.ok(DuplicateHandle(me, event, me, &mut dup, 0, 0, DUPLICATE_SAME_ACCESS) == 0, 0)?;
        c.ok(GetLastError() == ERROR_NO_SYSTEM_RESOURCES, GetLastError().into())?;
        // The other process does not care.
        child_runs(&mut c, &first)?;
        // Closing gives the handles back.
        for i in 0..n {
            c.ok(CloseHandle(*leaked.add(i)) != 0, i as u64)?;
        }
        c.ok(handle_count() == before_loop, handle_count())?;
        c.ok(DuplicateHandle(me, event, me, &mut dup, 0, 0, DUPLICATE_SAME_ACCESS) != 0, 0)?;
        c.ok(CloseHandle(dup) != 0, 0)?;

        if gala {
            // ProcessQuotaLimits (QUOTA_LIMITS): the nonpaged pool limit.
            let mut limits = [0u64; 6];
            let mut ret = 0;
            let status = NtQueryInformationProcess(me, 1, limits.as_mut_ptr().cast(), 48, &mut ret);
            c.ok(status == 0 && ret == 48, status as u32 as u64)?;
            c.ok(limits[1] == nonpaged_limit, limits[1])?;
            c.ok(limits[0] > 0, limits[0])?;

            // Objects: new events until the nonpaged pool quota refuses one.
            let (_, nonpaged_start) = pool_usage();
            let mut objects = 0;
            let error = loop {
                SetLastError(0);
                let e = CreateEventW(core::ptr::null(), 1, 0, core::ptr::null());
                if e.is_null() {
                    break GetLastError();
                }
                *leaked.add(objects) = e;
                objects += 1;
                if objects == cap {
                    break 0;
                }
            };
            c.ok(error == ERROR_NOT_ENOUGH_QUOTA, error.into())?;
            line(b"quota_test: pool quota at ", objects as u64, b" objects");
            let (_, nonpaged_full) = pool_usage();
            c.ok(nonpaged_full > nonpaged_start && nonpaged_full <= nonpaged_limit, nonpaged_full)?;
            // Only the pool quota was in the way: duplicates still work.
            c.ok(DuplicateHandle(me, event, me, &mut dup, 0, 0, DUPLICATE_SAME_ACCESS) != 0, 0)?;
            c.ok(CloseHandle(dup) != 0, 0)?;
            child_runs(&mut c, &second)?;
            for i in 0..objects {
                c.ok(CloseHandle(*leaked.add(i)) != 0, i as u64)?;
            }
            // The objects' charge is given back.
            let (_, nonpaged_after) = pool_usage();
            c.ok(nonpaged_after <= nonpaged_start, nonpaged_after)?;
            let e = CreateEventW(core::ptr::null(), 1, 0, core::ptr::null());
            c.ok(!e.is_null(), GetLastError().into())?;
            CloseHandle(e);

            // A completion port's packets: posted, never taken, until the
            // nonpaged pool quota refuses one.
            let port = CreateIoCompletionPort(!0usize as Handle, core::ptr::null_mut(), 0, 1);
            c.ok(!port.is_null(), GetLastError().into())?;
            let (_, nonpaged_start) = pool_usage();
            let mut posted = 0u64;
            let error = loop {
                SetLastError(0);
                if PostQueuedCompletionStatus(port, 7, posted as usize, 0) == 0 {
                    break GetLastError();
                }
                posted += 1;
                if posted == cap as u64 {
                    break 0;
                }
            };
            c.ok(error == ERROR_NOT_ENOUGH_QUOTA, error.into())?;
            line(b"quota_test: port quota at ", posted, b" packets");
            let (_, nonpaged_full) = pool_usage();
            c.ok(nonpaged_full > nonpaged_start && nonpaged_full <= nonpaged_limit, nonpaged_full)?;
            // Taken, in order, they give the charge back.
            for i in 0..posted {
                let (mut bytes, mut key, mut ov) = (0u32, 0usize, 0usize);
                c.ok(GetQueuedCompletionStatus(port, &mut bytes, &mut key, &mut ov, 0) != 0, i)?;
                c.ok(bytes == 7 && key == i as usize, key as u64)?;
            }
            let (_, nonpaged_after) = pool_usage();
            c.ok(nonpaged_after <= nonpaged_start, nonpaged_after)?;
            c.ok(PostQueuedCompletionStatus(port, 0, 0, 0) != 0, GetLastError().into())?;
            CloseHandle(port);
        } else {
            child_runs(&mut c, &second)?;
            // Sockets with big buffers: kept and charged.
            wsa_start(&mut c)?;
            let (_, nonpaged_start) = pool_usage();
            let (l, addr) = listener(&mut c, BIG_BUFFER)?;
            let mut open = [(0, 0); WINDOWS_PAIRS];
            for p in open.iter_mut() {
                *p = pair(&mut c, l, &addr, BIG_BUFFER)?.map_err(|e| e as u32)?;
            }
            let (_, nonpaged_full) = pool_usage();
            c.ok(nonpaged_full > nonpaged_start, nonpaged_full)?;
            for &(s, a) in open.iter() {
                closesocket(s);
                closesocket(a);
            }
            closesocket(l);
        }
        c.ok(VirtualFree(leaked.cast(), 0, MEM_RELEASE) != 0, GetLastError().into())?;
        CloseHandle(event);
        for child in [&first, &second] {
            CloseHandle(child.thread);
            CloseHandle(child.process);
        }
    }
    Ok(())
}

/// `quota_test sockets <nonpaged limit>` (gala): connections with big
/// buffers until the nonpaged pool quota refuses one.
fn sockets_over_quota(limit: u64) -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: winsock and Win32 calls on sockets and handles this
    // function owns.
    unsafe {
        let me = GetCurrentProcess();
        // Started first (starting one needs quota), run while this one is
        // at its limit.
        let child = start_child(&mut c, true)?;
        wsa_start(&mut c)?;
        let (l, addr) = listener(&mut c, BIG_BUFFER)?;
        let (_, nonpaged_start) = pool_usage();
        let mut open = [(0, 0); MAX_PAIRS];
        let mut n = 0;
        let error = loop {
            match pair(&mut c, l, &addr, BIG_BUFFER)? {
                Ok(p) => open[n] = p,
                Err(e) => break e,
            }
            n += 1;
            if n == MAX_PAIRS {
                break 0;
            }
        };
        c.ok(error == WSAENOBUFS, error as u64)?;
        line(b"quota_test: socket quota at ", n as u64, b" connections");
        c.ok(n >= 1, 0)?;
        // Each connection's sockets hold 2 MiB each: at least the client
        // sockets' are charged (the accepted ones are counted once the
        // listener has them).
        let (_, nonpaged_full) = pool_usage();
        c.ok(nonpaged_full <= limit, nonpaged_full)?;
        c.ok(nonpaged_full >= nonpaged_start + n as u64 * (2 * BIG_BUFFER as u64), nonpaged_full)?;
        // Another process connects its own meanwhile.
        child_runs(&mut c, &child)?;
        // Closing gives the charge back: a connection can be made again.
        for &(s, a) in open[..n].iter() {
            c.ok(closesocket(s) == 0, WSAGetLastError() as u64)?;
            c.ok(closesocket(a) == 0, WSAGetLastError() as u64)?;
        }
        let (_, nonpaged_after) = pool_usage();
        c.ok(nonpaged_after <= nonpaged_start, nonpaged_after)?;
        let (s, a) = pair(&mut c, l, &addr, BIG_BUFFER)?.map_err(|e| e as u32)?;
        closesocket(s);
        closesocket(a);
        closesocket(l);

        // ProcessQuotaLimits: a pool limit needs SeIncreaseQuotaPrivilege
        // (a standard user's token lacks it); the working set's sizes are
        // kept.
        let mut limits = [0u64; 6];
        limits[1] = limit / 2;
        let status = NtSetInformationProcess(me, PROCESS_QUOTA_LIMITS, limits.as_ptr().cast(), 48);
        c.ok(status == STATUS_PRIVILEGE_NOT_HELD, status as u32 as u64)?;
        let status = NtSetInformationProcess(me, PROCESS_QUOTA_LIMITS, limits.as_ptr().cast(), 40);
        c.ok(status != 0, status as u32 as u64)?;
        let ws = [0, 0, 1 << 20, 4 << 20, 0, 0u64];
        let status = NtSetInformationProcess(me, PROCESS_QUOTA_LIMITS, ws.as_ptr().cast(), 48);
        c.ok(status == 0, status as u32 as u64)?;
        let mut got = [0u64; 6];
        let mut ret = 0;
        let status = NtQueryInformationProcess(
            me,
            PROCESS_QUOTA_LIMITS,
            got.as_mut_ptr().cast(),
            48,
            &mut ret,
        );
        c.ok(status == 0, status as u32 as u64)?;
        c.ok(got[1] == limit && got[2] == 1 << 20 && got[3] == 4 << 20, got[3])?;
        // A minimum over the maximum is refused.
        let bad = [0, 0, 4 << 20, 1 << 20, 0, 0u64];
        let status = NtSetInformationProcess(me, PROCESS_QUOTA_LIMITS, bad.as_ptr().cast(), 48);
        c.ok(status != 0, status as u32 as u64)?;
        CloseHandle(child.thread);
        CloseHandle(child.process);
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"quota_test: ok\r\n");
            0
        }
        Err(n) => n,
    };
    // SAFETY: ends the process.
    unsafe { ExitProcess(code) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdead) }
}

/// ` (line L)` for the failing check's source line (`#[track_caller]` on
/// Checks' methods), as bytes and their length.
fn line_text(line: u32) -> ([u8; 18], usize) {
    let mut b = *b" (line 0000000000)";
    let mut digits = [0u8; 10];
    let (mut n, mut len) = (line, 0);
    loop {
        digits[len] = b'0' + (n % 10) as u8;
        len += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for i in 0..len {
        b[7 + i] = digits[len - 1 - i];
    }
    b[7 + len] = b')';
    (b, 8 + len)
}
