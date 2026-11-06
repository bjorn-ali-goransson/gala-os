//! Pool and handle quotas (ROADMAP R6), with the results real Windows
//! gives: a process that leaks handles fails at its handle limit (2^24 on
//! Windows, "Kernel Objects" on Microsoft Learn) with
//! ERROR_NO_SYSTEM_RESOURCES (STATUS_INSUFFICIENT_RESOURCES: the status is
//! assumed, Learn documents only the limit); GetProcessHandleCount and
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
//! checked to be about 2^24.)
//!
//! Prints `quota_test: ok` or exits with the number of the failing check;
//! `quota_test: handle limit at N` and `quota_test: pool quota at N
//! objects` report what was reached.

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
const WINDOWS_HANDLE_LIMIT: u64 = 1 << 24;
/// Duplicates the leak stops at whatever happens (a little over 2^24).
const MAX_LEAK: usize = (1 << 24) + 4096;
/// Objects the children create.
const CHILD_OBJECTS: usize = 64;
const CHILD_OK: u32 = 0x51;

#[repr(C)]
struct ProcessInformation {
    process: Handle,
    thread: Handle,
    process_id: u32,
    thread_id: u32,
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
    fn ExitProcess(code: u32) -> !;
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
}

#[allow(dead_code)]
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8;
    fn memset(dst: *mut u8, value: i32, n: usize) -> *mut u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
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
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        line(b"quota_test: check ", self.0.into(), b" failed");
        line(b"quota_test: got ", got, b"");
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

/// The numbers after the program's name on the command line (at most 2).
fn arguments() -> ([u64; 2], usize) {
    let mut numbers = [0u64; 2];
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
        let mut current: Option<u64> = None;
        loop {
            let c = *p.add(i);
            match c {
                0x30..=0x39 => {
                    current = Some(current.unwrap_or(0) * 10 + u64::from(c - 0x30));
                }
                _ => {
                    if let Some(n) = current.take()
                        && count < 2
                    {
                        numbers[count] = n;
                        count += 1;
                    }
                    if c == u16::from(b'c') {
                        // `child`
                        return ([0, 0], 3);
                    }
                }
            }
            if c == 0 {
                break;
            }
            i += 1;
        }
    }
    (numbers, count)
}

/// `quota_test child`: creates objects and handles; exits CHILD_OK if it
/// could.
fn child() -> ! {
    let mut handles = [core::ptr::null_mut(); CHILD_OBJECTS];
    let mut ok = true;
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

/// Starts `quota_test child` suspended.
fn start_child(c: &mut Checks) -> Result<ProcessInformation, u32> {
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
        for (i, &b) in b"\" child".iter().enumerate() {
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
    let (args, count) = arguments();
    if count == 3 {
        child();
    }
    let gala = count == 2;
    let (handle_limit, nonpaged_limit) = (args[0], args[1]);
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
        let first = start_child(&mut c)?;
        let second = start_child(&mut c)?;

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
        c.ok(at_limit == base + 2 * 2 + 1 + n as u64, at_limit)?;
        if gala {
            c.ok(at_limit == handle_limit, at_limit)?;
        } else {
            c.ok(at_limit <= WINDOWS_HANDLE_LIMIT, at_limit)?;
            c.ok(at_limit > WINDOWS_HANDLE_LIMIT - (1 << 16), at_limit)?;
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
        c.ok(handle_count() == base + 2 * 2 + 1, handle_count())?;
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
        } else {
            child_runs(&mut c, &second)?;
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
