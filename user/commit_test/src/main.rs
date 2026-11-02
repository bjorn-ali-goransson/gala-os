//! The commit charge (ROADMAP R6), with the results real Windows gives:
//! committing private memory is charged to the process (its PrivateUsage
//! grows by what was committed, and only once for pages already
//! committed; reserving costs nothing); a commit larger than the system's
//! commit limit fails with ERROR_COMMITMENT_LIMIT; a job's per-process
//! memory limit (JOB_OBJECT_LIMIT_PROCESS_MEMORY) and its job-wide limit
//! (JOB_OBJECT_LIMIT_JOB_MEMORY) make a commit over them fail the same
//! way, decommitting gives the charge back, and the job reports the peak
//! commit of its processes and of the job. Prints `commit_test: ok` or
//! exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;

type Bool = i32;
type Handle = *mut c_void;

const MEM_COMMIT: u32 = 0x1000;
const MEM_RESERVE: u32 = 0x2000;
const MEM_DECOMMIT: u32 = 0x4000;
const MEM_RELEASE: u32 = 0x8000;
const PAGE_READWRITE: u32 = 0x04;
const ERROR_COMMITMENT_LIMIT: u32 = 1455;
const MIB: usize = 1 << 20;

/// JobObjectExtendedLimitInformation and its fields (x64).
const EXTENDED_LIMIT: u32 = 9;
const EXTENDED_SIZE: usize = 144;
const LIMIT_FLAGS: usize = 16;
const PROCESS_MEMORY_LIMIT: usize = 112;
const JOB_MEMORY_LIMIT: usize = 120;
const PEAK_PROCESS_MEMORY: usize = 128;
const PEAK_JOB_MEMORY: usize = 136;
const JOB_OBJECT_LIMIT_PROCESS_MEMORY: u32 = 0x100;
const JOB_OBJECT_LIMIT_JOB_MEMORY: u32 = 0x200;

#[repr(C)]
#[derive(Default)]
struct MemoryStatusEx {
    length: u32,
    load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page_file: u64,
    avail_page_file: u64,
    total_virtual: u64,
    avail_virtual: u64,
    avail_extended_virtual: u64,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn VirtualAlloc(at: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(at: *mut c_void, size: usize, kind: u32) -> Bool;
    fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> Bool;
    fn K32GetProcessMemoryInfo(process: Handle, counters: *mut u64, size: u32) -> Bool;
    fn GetStdHandle(which: u32) -> Handle;
    fn WriteFile(h: Handle, buf: *const u8, len: u32, written: *mut u32, ov: *mut c_void) -> Bool;
    fn GetLastError() -> u32;
    fn SetLastError(code: u32);
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
    fn GetCurrentProcess() -> Handle;
    fn ExitProcess(code: u32) -> !;
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

fn hex(mut v: u64) {
    let mut buf = [0u8; 18];
    buf[0] = b'0';
    buf[1] = b'x';
    for i in (2..18).rev() {
        buf[i] = b"0123456789abcdef"[(v & 15) as usize];
        v >>= 4;
    }
    out(&buf);
}

struct Checks(u32);

impl Checks {
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        out(b"commit_test: check ");
        hex(self.0.into());
        out(b" failed, got ");
        hex(got);
        out(b"\r\n");
        Err(self.0)
    }

    /// A commit that must fail with ERROR_COMMITMENT_LIMIT.
    fn refused(&mut self, at: *mut c_void, size: usize, kind: u32) -> Result<(), u32> {
        // SAFETY: a request for memory; on success it is given back.
        let p = unsafe {
            SetLastError(0);
            VirtualAlloc(at, size, kind, PAGE_READWRITE)
        };
        if !p.is_null() {
            // SAFETY: what was just allocated.
            unsafe {
                if at.is_null() {
                    VirtualFree(p, 0, MEM_RELEASE);
                } else {
                    VirtualFree(p, size, MEM_DECOMMIT);
                }
            }
            return self.ok(false, p as u64);
        }
        // SAFETY: plain call.
        let error = unsafe { GetLastError() };
        self.ok(error == ERROR_COMMITMENT_LIMIT, error.into())
    }
}

/// PROCESS_MEMORY_COUNTERS_EX's PrivateUsage.
fn private_usage() -> u64 {
    let mut counters = [0u64; 10];
    // SAFETY: a PROCESS_MEMORY_COUNTERS_EX (80 bytes).
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), counters.as_mut_ptr(), 80) };
    counters[9]
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(v)
}

fn set_limits(job: Handle, flags: u32, process: u64, whole: u64) -> Bool {
    let mut info = [0u8; EXTENDED_SIZE];
    info[LIMIT_FLAGS..LIMIT_FLAGS + 4].copy_from_slice(&flags.to_le_bytes());
    info[PROCESS_MEMORY_LIMIT..PROCESS_MEMORY_LIMIT + 8].copy_from_slice(&process.to_le_bytes());
    info[JOB_MEMORY_LIMIT..JOB_MEMORY_LIMIT + 8].copy_from_slice(&whole.to_le_bytes());
    // SAFETY: a JOBOBJECT_EXTENDED_LIMIT_INFORMATION.
    unsafe { SetInformationJobObject(job, EXTENDED_LIMIT, info.as_ptr(), EXTENDED_SIZE as u32) }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: Win32 calls on memory this function owns.
    unsafe {
        // The system's commit limit.
        let mut status = MemoryStatusEx { length: 64, ..Default::default() };
        c.ok(GlobalMemoryStatusEx(&mut status) != 0, GetLastError().into())?;
        c.ok(
            status.total_page_file >= status.avail_page_file && status.avail_page_file > 0,
            status.avail_page_file,
        )?;
        // More than the page file can ever grow to (at most 3 times
        // physical memory) plus what is free: refused, while reserving
        // it is fine.
        let huge = (status.avail_page_file + 4 * status.total_phys) as usize + 1024 * MIB;
        c.refused(core::ptr::null_mut(), huge, MEM_COMMIT | MEM_RESERVE)?;
        let p = VirtualAlloc(core::ptr::null_mut(), huge, MEM_RESERVE, PAGE_READWRITE);
        c.ok(!p.is_null(), GetLastError().into())?;
        c.ok(VirtualFree(p, 0, MEM_RELEASE) != 0, GetLastError().into())?;

        // Committing is charged once; reserving is free.
        let before = private_usage();
        c.ok(before > 0, before)?;
        let region = VirtualAlloc(core::ptr::null_mut(), 64 * MIB, MEM_RESERVE, PAGE_READWRITE);
        c.ok(!region.is_null(), GetLastError().into())?;
        let reserved = private_usage();
        c.ok(reserved < before + MIB as u64, reserved)?;
        c.ok(!VirtualAlloc(region, 16 * MIB, MEM_COMMIT, PAGE_READWRITE).is_null(), 0)?;
        let committed = private_usage();
        c.ok(committed >= before + 16 * MIB as u64, committed)?;
        c.ok(committed < before + 17 * MIB as u64, committed)?;
        // Again: already committed, no new charge.
        c.ok(!VirtualAlloc(region, 16 * MIB, MEM_COMMIT, PAGE_READWRITE).is_null(), 0)?;
        let again = private_usage();
        c.ok(again < before + 17 * MIB as u64, again)?;
        c.ok(VirtualFree(region, 16 * MIB, MEM_DECOMMIT) != 0, GetLastError().into())?;
        let back = private_usage();
        c.ok(back < before + MIB as u64, back)?;

        // A per-process limit 24 MiB over the current use.
        let job = CreateJobObjectW(core::ptr::null(), core::ptr::null());
        c.ok(!job.is_null(), GetLastError().into())?;
        let base = private_usage();
        let limit = base + 24 * MIB as u64;
        c.ok(
            set_limits(job, JOB_OBJECT_LIMIT_PROCESS_MEMORY, limit, 0) != 0,
            GetLastError().into(),
        )?;
        c.ok(AssignProcessToJobObject(job, GetCurrentProcess()) != 0, GetLastError().into())?;
        c.refused(region, 32 * MIB, MEM_COMMIT)?;
        c.refused(core::ptr::null_mut(), 32 * MIB, MEM_COMMIT | MEM_RESERVE)?;
        c.ok(!VirtualAlloc(region, 16 * MIB, MEM_COMMIT, PAGE_READWRITE).is_null(), 0)?;
        // Touch it: the pages are there.
        memset(region as *mut u8, 0x5a, 16 * MIB);
        c.ok(!VirtualAlloc(region, 16 * MIB, MEM_COMMIT, PAGE_READWRITE).is_null(), 0)?;
        let next = (region as usize + 16 * MIB) as *mut c_void;
        c.refused(next, 16 * MIB, MEM_COMMIT)?;
        // Decommitting gives the charge back.
        c.ok(VirtualFree(region, 16 * MIB, MEM_DECOMMIT) != 0, GetLastError().into())?;
        c.ok(!VirtualAlloc(next, 20 * MIB, MEM_COMMIT, PAGE_READWRITE).is_null(), 0)?;
        let mut info = [0u8; EXTENDED_SIZE];
        let mut ret = 0;
        c.ok(
            QueryInformationJobObject(
                job,
                EXTENDED_LIMIT,
                info.as_mut_ptr(),
                EXTENDED_SIZE as u32,
                &mut ret,
            ) != 0,
            GetLastError().into(),
        )?;
        c.ok(u64_at(&info, PROCESS_MEMORY_LIMIT) == limit, u64_at(&info, PROCESS_MEMORY_LIMIT))?;
        let peak = u64_at(&info, PEAK_PROCESS_MEMORY);
        c.ok(peak >= base + 20 * MIB as u64 && peak <= limit, peak)?;
        let peak_job = u64_at(&info, PEAK_JOB_MEMORY);
        c.ok(peak_job >= base + 20 * MIB as u64, peak_job)?;
        c.ok(VirtualFree(next, 20 * MIB, MEM_DECOMMIT) != 0, GetLastError().into())?;

        // A limit on the whole job.
        let now = private_usage();
        let whole = now + 8 * MIB as u64;
        c.ok(set_limits(job, JOB_OBJECT_LIMIT_JOB_MEMORY, 0, whole) != 0, GetLastError().into())?;
        c.refused(region, 16 * MIB, MEM_COMMIT)?;
        c.ok(!VirtualAlloc(region, 4 * MIB, MEM_COMMIT, PAGE_READWRITE).is_null(), 0)?;

        // No limits: the commit goes through.
        c.ok(set_limits(job, 0, 0, 0) != 0, GetLastError().into())?;
        c.ok(!VirtualAlloc(region, 48 * MIB, MEM_COMMIT, PAGE_READWRITE).is_null(), 0)?;
        memset(region as *mut u8, 0xa5, 48 * MIB);
        c.ok(VirtualFree(region, 0, MEM_RELEASE) != 0, GetLastError().into())?;
        let end = private_usage();
        c.ok(end < before + MIB as u64, end)?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let code = match run() {
        Ok(()) => {
            out(b"commit_test: ok\r\n");
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
