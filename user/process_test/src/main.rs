//! CreateProcessW, with the results real Windows gives: the parent starts
//! itself as a child (`process_test child <handle>`) with an inheritable
//! event; the child prints through the inherited console, signals the
//! event through the inherited handle value and exits with 0x1234. The
//! parent waits on the process handle and reads the exit code
//! (STILL_ACTIVE while it runs), sees the event set, starts a child
//! suspended (it does not run until ResumeThread) and checks that a
//! missing program fails with ERROR_FILE_NOT_FOUND. OpenProcess by id
//! opens an exited child (while a handle keeps it), a suspended one and
//! the caller; id 0 or an unused id fail with ERROR_INVALID_PARAMETER.
//! A Toolhelp snapshot lists the suspended child (parent, threads, name)
//! and its thread. VirtualAllocEx/VirtualQueryEx/VirtualFreeEx reach the
//! suspended child's address space (PROCESS_VM_OPERATION needed).
//! Then redirected
//! standard handles (STARTF_USESTDHANDLES): a child (`process_test
//! pipes`) reads its stdin from one pipe until end of file and writes its
//! stdout and stderr into another, which the parent reads until
//! ERROR_BROKEN_PIPE (the CreatePipe docs' pattern: the parent's ends are
//! not inheritable, and it closes the child's ends after CreateProcess).
//! A child started with a 20000-character command line sees all of it.
//! Last, job objects: limits, accounting, and a one-process limit that
//! stops this process from starting another (ERROR_NOT_ENOUGH_QUOTA).
//! Prints `process_test: ok` or exits with the number of the failing check.

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

#[repr(C)]
struct StartupInfo {
    cb: u32,
    rest: [u8; 100],
}

#[repr(C)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    process_id: u32,
    thread_id: u32,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn VirtualAllocEx(
        process: Handle,
        at: *mut c_void,
        size: usize,
        kind: u32,
        protect: u32,
    ) -> *mut c_void;
    fn VirtualFreeEx(process: Handle, at: *mut c_void, size: usize, kind: u32) -> Bool;
    fn VirtualQueryEx(process: Handle, at: *mut c_void, info: *mut u8, length: usize) -> usize;
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn GetCommandLineW() -> *const u16;
    fn GetModuleFileNameW(module: Handle, buf: *mut u16, size: u32) -> u32;
    fn GetLastError() -> u32;
    fn CreateEventW(
        attributes: *const SecurityAttributes,
        manual: Bool,
        initial: Bool,
        name: *const u16,
    ) -> Handle;
    fn SetEvent(h: Handle) -> Bool;
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
    fn WaitForSingleObject(h: Handle, ms: u32) -> u32;
    fn CreateJobObjectW(security: *const c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(job: Handle, class: u32, info: *const u8, len: u32) -> Bool;
    fn QueryInformationJobObject(
        job: Handle,
        class: u32,
        info: *mut u8,
        len: u32,
        ret: *mut u32,
    ) -> Bool;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> Bool;
    fn IsProcessInJob(process: Handle, job: Handle, result: *mut Bool) -> Bool;
    fn GetExitCodeProcess(h: Handle, code: *mut u32) -> Bool;
    fn OpenProcess(access: u32, inherit: Bool, id: u32) -> Handle;
    fn GetCurrentProcessId() -> u32;
    fn GetCurrentProcess() -> Handle;
    fn CreateToolhelp32Snapshot(flags: u32, process: u32) -> Handle;
    fn Process32FirstW(snapshot: Handle, entry: *mut u64) -> Bool;
    fn Process32NextW(snapshot: Handle, entry: *mut u64) -> Bool;
    fn Thread32First(snapshot: Handle, entry: *mut u32) -> Bool;
    fn Thread32Next(snapshot: Handle, entry: *mut u32) -> Bool;
    fn ResumeThread(h: Handle) -> u32;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> Bool;
    fn CreatePipe(
        read: *mut Handle,
        write: *mut Handle,
        attributes: *const SecurityAttributes,
        size: u32,
    ) -> Bool;
    fn SetHandleInformation(h: Handle, mask: u32, flags: u32) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn ExitProcess(code: u32) -> !;
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
    // The stack probe for frames over a page (the long command line).
    fn __chkstk();
}

#[unsafe(no_mangle)]
pub static _fltused: i32 = 0;

#[unsafe(no_mangle)]
pub extern "C" fn __CxxFrameHandler3() -> ! {
    // SAFETY: ends the process.
    unsafe { ExitProcess(0xdeac) }
}

const INFINITE: u32 = 0xffff_ffff;
const WAIT_TIMEOUT: u32 = 258;
const STILL_ACTIVE: u32 = 259;
const CREATE_SUSPENDED: u32 = 4;
const SYNCHRONIZE: u32 = 0x0010_0000;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_NO_MORE_FILES: u32 = 18;
const TH32CS_SNAPPROCESS: u32 = 2;
const TH32CS_SNAPTHREAD: u32 = 4;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const CHILD_EXIT: u32 = 0x1234;
const PIPES_CHILD_EXIT: u32 = 0x77;
const STARTF_USESTDHANDLES: u32 = 0x100;
const ERROR_BROKEN_PIPE: u32 = 109;
/// What the pipes child writes for the parent's `ping`.
const PIPES_OUTPUT: &[u8] = b"child read 4: ping\r\nchild stderr\r\n";

fn out(text: &[u8]) {
    let mut written = 0;
    // SAFETY: valid buffer and output pointer.
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

fn hex(v: u32) -> [u8; 8] {
    let mut s = [0u8; 8];
    for (i, c) in s.iter_mut().enumerate() {
        let d = (v >> (28 - 4 * i)) & 0xf;
        *c = if d < 10 { b'0' + d as u8 } else { b'a' + d as u8 - 10 };
    }
    s
}

struct Checks(u32);

impl Checks {
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"process_test: check ");
        out(&hex(self.0));
        out(b" got ");
        out(&hex(got as u32));
        out(b"\r\n");
        Err(self.0)
    }

    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }
}

/// The command line's words after the program name, as UTF-16 slices.
fn arguments<'a>(line: &'a [u16], out: &mut [&'a [u16]; 4]) -> usize {
    // Skip the program name: quoted (it may contain spaces) or one word.
    let rest = if line.first() == Some(&u16::from(b'"')) {
        let close =
            line[1..].iter().position(|&c| c == u16::from(b'"')).map_or(line.len(), |i| i + 2);
        &line[close.min(line.len())..]
    } else {
        let end = line.iter().position(|&c| c == u16::from(b' ')).unwrap_or(line.len());
        &line[end..]
    };
    let mut n = 0;
    for w in rest.split(|&c| c == u16::from(b' ')).filter(|w| !w.is_empty()) {
        if n < out.len() {
            out[n] = w;
            n += 1;
        }
    }
    n
}

fn parse_hex(w: &[u16]) -> Option<usize> {
    let mut v = 0usize;
    for &c in w {
        let d = (c as u8 as char).to_digit(16)?;
        v = v * 16 + d as usize;
    }
    Some(v)
}

/// UTF-16 of `text` into `buf` at `at`; returns the new end.
const ERROR_BAD_LENGTH: u32 = 24;

/// The long command line's length, in characters.
const LONG_LINE: usize = 20_000;

fn put(buf: &mut [u16], at: usize, text: &[u8]) -> usize {
    for (i, &b) in text.iter().enumerate() {
        buf[at + i] = u16::from(b);
    }
    at + text.len()
}

fn child(handle: usize) -> ! {
    out(b"process_test: child says hello through the inherited console\r\n");
    // SAFETY: the inherited event handle (by value, as Windows passes it).
    let ok = unsafe { SetEvent(handle as Handle) } != 0;
    // SAFETY: ends the process.
    unsafe { ExitProcess(if ok { CHILD_EXIT } else { 0xbad }) }
}

/// `process_test pipes`: reads stdin until end of file, reports it on
/// stdout, writes a line to stderr.
fn pipes_child() -> ! {
    let mut data = [0u8; 64];
    let mut total = 0usize;
    // SAFETY: documented kernel32 calls with valid buffers.
    unsafe {
        let input = GetStdHandle(-10i32 as u32);
        loop {
            let mut n = 0;
            let room = data.len() - total;
            if room == 0
                || ReadFile(
                    input,
                    data.as_mut_ptr().add(total),
                    room as u32,
                    &mut n,
                    core::ptr::null_mut(),
                ) == 0
            {
                break;
            }
            total += n as usize;
        }
        let mut line = [0u8; 96];
        let head = b"child read ";
        line[..head.len()].copy_from_slice(head);
        let mut at = head.len();
        line[at] = b'0' + (total % 10) as u8;
        at += 1;
        line[at..at + 2].copy_from_slice(b": ");
        at += 2;
        line[at..at + total].copy_from_slice(&data[..total]);
        at += total;
        line[at..at + 2].copy_from_slice(b"\r\n");
        at += 2;
        let mut written = 0;
        WriteFile(
            GetStdHandle(-11i32 as u32),
            line.as_ptr(),
            at as u32,
            &mut written,
            core::ptr::null_mut(),
        );
        let err = b"child stderr\r\n";
        WriteFile(
            GetStdHandle(-12i32 as u32),
            err.as_ptr(),
            err.len() as u32,
            &mut written,
            core::ptr::null_mut(),
        );
        ExitProcess(PIPES_CHILD_EXIT)
    }
}

/// The redirected-stdio part (see the module comment).
fn pipes(c: &mut Checks, path: &[u16; 260], n: usize) -> Result<(), u32> {
    // SAFETY: documented kernel32 calls with valid arguments.
    unsafe {
        let inherit =
            SecurityAttributes { length: 24, descriptor: core::ptr::null_mut(), inherit: 1 };
        let (mut in_r, mut in_w) = (core::ptr::null_mut(), core::ptr::null_mut());
        let (mut out_r, mut out_w) = (core::ptr::null_mut(), core::ptr::null_mut());
        c.ok(CreatePipe(&mut in_r, &mut in_w, &inherit, 0) != 0, GetLastError().into())?;
        c.ok(CreatePipe(&mut out_r, &mut out_w, &inherit, 0) != 0, GetLastError().into())?;
        // The parent's ends stay here.
        c.ok(SetHandleInformation(in_w, 1, 0) != 0, GetLastError().into())?;
        c.ok(SetHandleInformation(out_r, 1, 0) != 0, GetLastError().into())?;

        let mut line = [0u16; 400];
        let mut at = put(&mut line, 0, b"\"");
        line[at..at + n].copy_from_slice(&path[..n]);
        at += n;
        at = put(&mut line, at, b"\" pipes");
        line[at] = 0;
        // STARTUPINFOW: dwFlags at 60, hStdInput/Output/Error at 80/88/96.
        let mut startup = StartupInfo { cb: 104, rest: [0; 100] };
        startup.rest[56..60].copy_from_slice(&STARTF_USESTDHANDLES.to_le_bytes());
        startup.rest[76..84].copy_from_slice(&(in_r as u64).to_le_bytes());
        startup.rest[84..92].copy_from_slice(&(out_w as u64).to_le_bytes());
        startup.rest[92..100].copy_from_slice(&(out_w as u64).to_le_bytes());
        let mut info = ProcessInformation {
            process: core::ptr::null_mut(),
            thread: core::ptr::null_mut(),
            process_id: 0,
            thread_id: 0,
        };
        let ok = CreateProcessW(
            path.as_ptr(),
            line.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            1,
            0,
            core::ptr::null(),
            core::ptr::null(),
            &startup,
            &mut info,
        );
        c.ok(ok != 0, GetLastError().into())?;
        // The child's ends are the child's now.
        CloseHandle(in_r);
        CloseHandle(out_w);

        let mut written = 0;
        c.ok(WriteFile(in_w, b"ping".as_ptr(), 4, &mut written, core::ptr::null_mut()) != 0, 0)?;
        c.eq(written.into(), 4)?;
        CloseHandle(in_w); // end of the child's input

        let mut output = [0u8; 128];
        let mut total = 0usize;
        loop {
            let mut got = 0;
            let room = (output.len() - total) as u32;
            if room == 0
                || ReadFile(
                    out_r,
                    output.as_mut_ptr().add(total),
                    room,
                    &mut got,
                    core::ptr::null_mut(),
                ) == 0
            {
                break;
            }
            total += got as usize;
        }
        c.eq(GetLastError().into(), ERROR_BROKEN_PIPE.into())?;
        c.ok(&output[..total] == PIPES_OUTPUT, total as u64)?;
        CloseHandle(out_r);

        c.eq(WaitForSingleObject(info.process, 10_000).into(), 0)?;
        let mut code = 0;
        c.ok(GetExitCodeProcess(info.process, &mut code) != 0, GetLastError().into())?;
        c.eq(code.into(), PIPES_CHILD_EXIT.into())?;
        CloseHandle(info.thread);
        CloseHandle(info.process);
    }
    Ok(())
}

/// Toolhelp snapshots: the suspended child `info` is listed with us as
/// its parent, one thread and its file name, and we are listed; its
/// thread belongs to it; the lists end with ERROR_NO_MORE_FILES.
fn toolhelp(c: &mut Checks, info: &ProcessInformation) -> Result<(), u32> {
    // SAFETY: documented kernel32 calls with valid arguments.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS | TH32CS_SNAPTHREAD, 0);
        c.ok(snapshot as isize != -1, GetLastError().into())?;
        // PROCESSENTRY32W: dwSize, cntUsage, th32ProcessID, (pad),
        // th32DefaultHeapID, th32ModuleID, cntThreads (28),
        // th32ParentProcessID (32), pcPriClassBase, dwFlags, szExeFile (44).
        let mut entry = [0u64; 71];
        entry[0] = 568;
        let field = |e: &[u64; 71], at: usize| {
            let b = e.as_ptr().cast::<u8>().add(at);
            u32::from_le_bytes([*b, *b.add(1), *b.add(2), *b.add(3)])
        };
        // The child's (parent, threads, named), and whether we were named.
        let (mut child, mut self_named, mut count) = (None, false, 0u64);
        let mut ok = Process32FirstW(snapshot, entry.as_mut_ptr());
        while ok != 0 {
            count += 1;
            let name =
                core::slice::from_raw_parts(entry.as_ptr().cast::<u8>().add(44).cast::<u16>(), 260);
            let len = name.iter().position(|&ch| ch == 0).unwrap_or(260);
            let named =
                name[..len].iter().copied().eq(b"process_test.exe".iter().map(|&b| u16::from(b)));
            if field(&entry, 8) == info.process_id {
                child = Some((field(&entry, 32), field(&entry, 28), named));
            }
            if field(&entry, 8) == GetCurrentProcessId() {
                self_named = named;
            }
            ok = Process32NextW(snapshot, entry.as_mut_ptr());
        }
        c.eq(GetLastError().into(), ERROR_NO_MORE_FILES.into())?;
        c.ok(child == Some((GetCurrentProcessId(), 1, true)), count)?;
        c.ok(self_named, count)?;
        // THREADENTRY32: dwSize, cntUsage, th32ThreadID, th32OwnerProcessID, ...
        let mut thread = [0u32; 7];
        thread[0] = 28;
        let mut owner = None;
        let mut ok = Thread32First(snapshot, thread.as_mut_ptr());
        while ok != 0 {
            if thread[2] == info.thread_id {
                owner = Some(thread[3]);
            }
            ok = Thread32Next(snapshot, thread.as_mut_ptr());
        }
        c.eq(GetLastError().into(), ERROR_NO_MORE_FILES.into())?;
        c.ok(owner == Some(info.process_id), owner.unwrap_or(0).into())?;
        c.ok(CloseHandle(snapshot) != 0, GetLastError().into())?;
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: documented kernel32 calls with valid arguments.
    unsafe {
        let mut path = [0u16; 260];
        let n = GetModuleFileNameW(core::ptr::null_mut(), path.as_mut_ptr(), 260) as usize;
        c.ok(n > 0 && n < 260, n as u64)?;

        // An inheritable event; its value goes on the child's command line.
        let inherit =
            SecurityAttributes { length: 24, descriptor: core::ptr::null_mut(), inherit: 1 };
        let event = CreateEventW(&inherit, 1, 0, core::ptr::null());
        c.ok(!event.is_null(), GetLastError().into())?;
        let mut line = [0u16; 400];
        let mut at = put(&mut line, 0, b"\"");
        line[at..at + n].copy_from_slice(&path[..n]);
        at += n;
        at = put(&mut line, at, b"\" child ");
        at = put(&mut line, at, &hex(event as usize as u32));
        line[at] = 0;

        let startup = StartupInfo { cb: 104, rest: [0; 100] };
        let mut info = ProcessInformation {
            process: core::ptr::null_mut(),
            thread: core::ptr::null_mut(),
            process_id: 0,
            thread_id: 0,
        };
        let ok = CreateProcessW(
            path.as_ptr(),
            line.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            1,
            0,
            core::ptr::null(),
            core::ptr::null(),
            &startup,
            &mut info,
        );
        c.ok(ok != 0, GetLastError().into())?;
        c.ok(!info.process.is_null() && !info.thread.is_null(), 0)?;
        c.ok(info.process_id != 0 && info.thread_id != 0, info.process_id.into())?;
        c.eq(WaitForSingleObject(info.process, 10_000).into(), 0)?;
        let mut code = 0;
        c.ok(GetExitCodeProcess(info.process, &mut code) != 0, GetLastError().into())?;
        c.eq(code.into(), CHILD_EXIT.into())?;
        c.eq(WaitForSingleObject(event, 0).into(), 0)?; // the child set it
        // OpenProcess by id: the exited child (its object lives on while
        // we hold a handle), ourselves, and an id that is no process.
        let opened =
            OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, info.process_id);
        c.ok(!opened.is_null(), GetLastError().into())?;
        c.eq(WaitForSingleObject(opened, 0).into(), 0)?;
        code = 0;
        c.ok(GetExitCodeProcess(opened, &mut code) != 0, GetLastError().into())?;
        c.eq(code.into(), CHILD_EXIT.into())?;
        CloseHandle(opened);
        let me = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, GetCurrentProcessId());
        c.ok(!me.is_null(), GetLastError().into())?;
        c.ok(GetExitCodeProcess(me, &mut code) != 0, GetLastError().into())?;
        c.eq(code.into(), STILL_ACTIVE.into())?;
        CloseHandle(me);
        c.ok(OpenProcess(SYNCHRONIZE, 0, 0).is_null(), 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_PARAMETER.into())?;
        c.ok(OpenProcess(SYNCHRONIZE, 0, 0x7fff_fff0).is_null(), 0)?;
        c.eq(GetLastError().into(), ERROR_INVALID_PARAMETER.into())?;
        CloseHandle(info.thread);
        CloseHandle(info.process);

        // Suspended: nothing runs until ResumeThread.
        let ok = CreateProcessW(
            path.as_ptr(),
            line.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            1,
            CREATE_SUSPENDED,
            core::ptr::null(),
            core::ptr::null(),
            &startup,
            &mut info,
        );
        c.ok(ok != 0, GetLastError().into())?;
        c.eq(WaitForSingleObject(info.process, 200).into(), WAIT_TIMEOUT.into())?;
        c.ok(GetExitCodeProcess(info.process, &mut code) != 0, GetLastError().into())?;
        c.eq(code.into(), STILL_ACTIVE.into())?;
        // Opened by id while it waits to run.
        let opened = OpenProcess(SYNCHRONIZE, 0, info.process_id);
        c.ok(!opened.is_null(), GetLastError().into())?;
        c.eq(WaitForSingleObject(opened, 0).into(), WAIT_TIMEOUT.into())?;
        toolhelp(&mut c, &info)?;
        // Its address space through the process handle (PostgreSQL
        // reserves its shared memory in a suspended child so): a region
        // reserved there is there and not here; released, it is free.
        const WANT: usize = 0x2a0_0000_0000;
        let at = VirtualAllocEx(info.process, WANT as *mut c_void, 0x10_0000, 0x2000, 4);
        c.eq(at as u64, WANT as u64)?;
        let mut mbi = [0u64; 6];
        c.eq(VirtualQueryEx(info.process, at, mbi.as_mut_ptr().cast(), 48) as u64, 48)?;
        c.eq(mbi[4] & 0xffff_ffff, 0x2000)?; // State: MEM_RESERVE
        c.ok(mbi[3] >= 0x10_0000, mbi[3])?; // RegionSize
        c.eq(VirtualQueryEx(GetCurrentProcess(), at, mbi.as_mut_ptr().cast(), 48) as u64, 48)?;
        c.eq(mbi[4] & 0xffff_ffff, 0x1_0000)?; // MEM_FREE here
        c.ok(VirtualFreeEx(info.process, at, 0, 0x8000) != 0, GetLastError().into())?;
        c.eq(VirtualQueryEx(info.process, at, mbi.as_mut_ptr().cast(), 48) as u64, 48)?;
        c.eq(mbi[4] & 0xffff_ffff, 0x1_0000)?;
        // Without PROCESS_VM_OPERATION: ERROR_ACCESS_DENIED.
        let limited = OpenProcess(SYNCHRONIZE, 0, info.process_id);
        let denied = VirtualAllocEx(limited, core::ptr::null_mut(), 0x1_0000, 0x2000, 4);
        c.ok(denied.is_null(), denied as u64)?;
        c.eq(GetLastError().into(), 5)?;
        CloseHandle(limited);
        c.eq(ResumeThread(info.thread).into(), 1)?;
        c.eq(WaitForSingleObject(opened, INFINITE).into(), 0)?;
        CloseHandle(opened);
        c.eq(WaitForSingleObject(info.process, INFINITE).into(), 0)?;
        c.ok(GetExitCodeProcess(info.process, &mut code) != 0 && code == CHILD_EXIT, code.into())?;
        CloseHandle(info.thread);
        CloseHandle(info.process);

        // A program that does not exist (in the same directory).
        let mut missing = path;
        let slash = missing[..n].iter().rposition(|&ch| ch == u16::from(b'\\')).unwrap_or(0);
        let end = put(&mut missing, slash + 1, b"no_such_program.exe");
        missing[end] = 0;
        let ok = CreateProcessW(
            missing.as_ptr(),
            core::ptr::null_mut(),
            core::ptr::null(),
            core::ptr::null(),
            0,
            0,
            core::ptr::null(),
            core::ptr::null(),
            &startup,
            &mut info,
        );
        c.eq(ok as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_FILE_NOT_FOUND.into())?;
        CloseHandle(event);

        // A long command line (Windows allows 32767 characters): the child
        // (`process_test long xxx...`) exits with its length.
        let mut long = [0u16; LONG_LINE + 1];
        let mut at = put(&mut long, 0, b"process_test long ");
        while at < LONG_LINE {
            long[at] = u16::from(b'x');
            at += 1;
        }
        long[LONG_LINE] = 0;
        let ok = CreateProcessW(
            path.as_ptr(),
            long.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            0,
            0,
            core::ptr::null(),
            core::ptr::null(),
            &startup,
            &mut info,
        );
        c.ok(ok != 0, GetLastError().into())?;
        c.eq(WaitForSingleObject(info.process, INFINITE).into(), 0)?;
        c.ok(GetExitCodeProcess(info.process, &mut code) != 0, 0)?;
        c.eq(code.into(), LONG_LINE as u64)?;
        CloseHandle(info.thread);
        CloseHandle(info.process);
        pipes(&mut c, &path, n)?;
        jobs(&mut c, &path)?;
    }
    Ok(())
}

/// Job objects, last (the limit then binds this process): a new job has
/// no limits and no processes; with JOB_OBJECT_LIMIT_ACTIVE_PROCESS at 1
/// and this process assigned, CreateProcess fails with
/// ERROR_NOT_ENOUGH_QUOTA (the documented effect: the limit counts this
/// process, and children join their parent's job).
fn jobs(c: &mut Checks, path: &[u16; 260]) -> Result<(), u32> {
    const BASIC_ACCOUNTING: u32 = 1; // JobObjectBasicAccountingInformation
    const BASIC_LIMIT: u32 = 2; // JobObjectBasicLimitInformation
    const LIMIT_ACTIVE_PROCESS: u32 = 8;
    const ERROR_NOT_ENOUGH_QUOTA: u32 = 1816;
    // SAFETY: documented calls with buffers of ours.
    unsafe {
        let me = GetCurrentProcess();
        let job = CreateJobObjectW(core::ptr::null(), core::ptr::null());
        c.ok(!job.is_null(), GetLastError().into())?;
        let mut inside = 1;
        c.ok(IsProcessInJob(me, job, &mut inside) != 0 && inside == 0, inside as u64)?;
        // JOBOBJECT_BASIC_LIMIT_INFORMATION (64 bytes on x64): no limits.
        let mut limits = [0u8; 64];
        let mut ret = 0;
        c.ok(
            QueryInformationJobObject(job, BASIC_LIMIT, limits.as_mut_ptr(), 64, &mut ret) != 0,
            0,
        )?;
        c.eq(ret.into(), 64)?;
        c.ok(limits.iter().all(|&b| b == 0), 0)?;
        // A wrong size.
        c.eq(SetInformationJobObject(job, BASIC_LIMIT, limits.as_ptr(), 63) as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_BAD_LENGTH.into())?;
        // One active process at most: LimitFlags at 16, ActiveProcessLimit at 40.
        limits[16..20].copy_from_slice(&LIMIT_ACTIVE_PROCESS.to_le_bytes());
        limits[40..44].copy_from_slice(&1u32.to_le_bytes());
        c.ok(SetInformationJobObject(job, BASIC_LIMIT, limits.as_ptr(), 64) != 0, 0)?;
        c.ok(AssignProcessToJobObject(job, me) != 0, GetLastError().into())?;
        c.ok(IsProcessInJob(me, job, &mut inside) != 0 && inside != 0, 0)?;
        // JOBOBJECT_BASIC_ACCOUNTING_INFORMATION: ActiveProcesses at 40.
        let mut accounting = [0u8; 48];
        c.ok(
            QueryInformationJobObject(job, BASIC_ACCOUNTING, accounting.as_mut_ptr(), 48, &mut ret)
                != 0,
            0,
        )?;
        c.eq(
            u32::from_le_bytes([accounting[40], accounting[41], accounting[42], accounting[43]])
                .into(),
            1,
        )?;
        let mut line = [0u16; 300];
        let end = put(&mut line, 0, b"process_test child 0");
        line[end] = 0;
        let mut startup: StartupInfo = core::mem::zeroed();
        startup.cb = size_of::<StartupInfo>() as u32;
        let mut info: ProcessInformation = core::mem::zeroed();
        let ok = CreateProcessW(
            path.as_ptr(),
            line.as_mut_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            0,
            0,
            core::ptr::null(),
            core::ptr::null(),
            &startup,
            &mut info,
        );
        c.eq(ok as u64, 0)?;
        c.eq(GetLastError().into(), ERROR_NOT_ENOUGH_QUOTA.into())?;
        c.ok(CloseHandle(job) != 0, 0)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    // SAFETY: the command line, NUL-terminated, for the process's lifetime.
    let line = unsafe {
        let p = GetCommandLineW();
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(p, n)
    };
    let mut args: [&[u16]; 4] = [&[]; 4];
    let argc = arguments(line, &mut args);
    let child_word: [u16; 5] = b"child".map(u16::from);
    let pipes_word: [u16; 5] = b"pipes".map(u16::from);
    if argc == 1 && args[0] == pipes_word {
        pipes_child();
    }
    if argc == 2 && args[0] == b"long".map(u16::from) {
        // SAFETY: ends the process.
        unsafe { ExitProcess(line.len() as u32) }
    }
    if argc == 2 && args[0] == child_word {
        child(parse_hex(args[1]).unwrap_or(0));
    }
    let code = match run() {
        Ok(()) => {
            out(b"process_test: ok\r\n");
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
