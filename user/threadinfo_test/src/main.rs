//! Thread information kernel32 relies on, with results expected to match
//! real Windows: NtYieldExecution; NtQueryInformationThread start address,
//! "am I the last thread", times and termination; NtSetInformationThread
//! hide-from-debugger and priorities; private, zeroed TLS slots in the TEB.
//! (Assumes no other threads in the process, as for an exe without
//! imports.) Prints `threadinfo_test: ok` or exits with the number of the
//! failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU64, Ordering};

use ntrt::*;

unsafe extern "C" {
    static __ImageBase: u8;
}

const STATUS_NO_YIELD_PERFORMED: NtStatus = 0x4000_0024;
const STATUS_INFO_LENGTH_MISMATCH: NtStatus = 0xc000_0004;
const STATUS_INVALID_PARAMETER: NtStatus = 0xc000_000d;
const THREAD_TIMES: u32 = 1;
const THREAD_PRIORITY: u32 = 2;
const THREAD_BASE_PRIORITY: u32 = 3;
const THREAD_WIN32_START_ADDRESS: u32 = 9;
const THREAD_AM_I_LAST_THREAD: u32 = 12;
const THREAD_HIDE_FROM_DEBUGGER: u32 = 17;
const THREAD_IS_TERMINATED: u32 = 20;
const TEB_TLS_SLOTS: usize = 0x1480;

struct Checks(u32);

impl Checks {
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("threadinfo_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn u32_at(b: &[u8], off: usize) -> u64 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap()).into()
}

fn u64_at(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}

fn query_u32(h: Handle, class: u32) -> (NtStatus, u64) {
    let mut b = [0xffu8; 4];
    let (s, _) = query_thread(h, class, &mut b);
    (s, u32_at(&b, 0))
}

/// The worker's TLS slot 5 as it saw it, plus 1 (0 = not yet run).
static WORKER_SLOT: AtomicU64 = AtomicU64::new(0);
/// Set by main to let the worker finish.
static RELEASE: AtomicU64 = AtomicU64::new(0);

fn tls_slot(i: usize) -> u64 {
    teb_field(TEB_TLS_SLOTS + 8 * i) as u64
}

extern "C" fn worker(_: usize) -> ! {
    let all_zero = (0..64).all(|i| tls_slot(i) == 0);
    WORKER_SLOT.store(if all_zero { tls_slot(5) + 1 } else { 0xbad }, Ordering::SeqCst);
    while RELEASE.load(Ordering::SeqCst) == 0 {
        yield_execution();
    }
    exit_thread(0x1234)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = u64::from(STATUS_SUCCESS);

    let y = yield_execution();
    c.ok(y == STATUS_SUCCESS || y == STATUS_NO_YIELD_PERFORMED, y.into())?;

    // The main thread starts at the image entry point.
    let image = &raw const __ImageBase as usize;
    // SAFETY: reading our own PE header.
    let entry_rva = unsafe {
        let pe = image + (*((image + 0x3c) as *const u32)) as usize;
        *((pe + 24 + 16) as *const u32)
    };
    let mut b = [0u8; 8];
    let (s, ret) = query_thread(CURRENT_THREAD, THREAD_WIN32_START_ADDRESS, &mut b);
    c.eq(s.into(), ok)?;
    c.eq(ret.into(), 8)?;
    c.eq(u64_at(&b, 0), (image + entry_rva as usize) as u64)?;

    let (s, last) = query_u32(CURRENT_THREAD, THREAD_AM_I_LAST_THREAD);
    c.eq(s.into(), ok)?;
    c.eq(last, 1)?;
    c.eq(query_u32(CURRENT_THREAD, THREAD_IS_TERMINATED).1, 0)?;

    // Times: created before now, not exited.
    let mut times = [0u8; 32];
    let (s, ret) = query_thread(CURRENT_THREAD, THREAD_TIMES, &mut times);
    let (_, now) = system_time();
    c.eq(s.into(), ok)?;
    c.eq(ret.into(), 32)?;
    c.ok(u64_at(&times, 0) > 0 && u64_at(&times, 0) <= now as u64, u64_at(&times, 0))?;
    c.eq(u64_at(&times, 8), 0)?;
    c.eq(
        query_thread(CURRENT_THREAD, THREAD_TIMES, &mut times[..16]).0.into(),
        STATUS_INFO_LENGTH_MISMATCH.into(),
    )?;

    // HideFromDebugger takes no data.
    c.eq(set_thread(CURRENT_THREAD, THREAD_HIDE_FROM_DEBUGGER, &[]).into(), ok)?;
    c.eq(
        set_thread(CURRENT_THREAD, THREAD_HIDE_FROM_DEBUGGER, &[0; 4]).into(),
        STATUS_INFO_LENGTH_MISMATCH.into(),
    )?;

    // A second thread: TLS private and zeroed, start address = its entry
    // (Rip, Rcx and Rax all hold it, so any convention gives the same).
    // SAFETY: TLS slot 5 of our own TEB.
    unsafe {
        core::ptr::write_volatile((teb_field(0x30) + TEB_TLS_SLOTS + 8 * 5) as *mut u64, 0x77)
    };
    let entry = worker as *const () as usize;
    let mut ctx = Context::for_thread(worker, entry, 0);
    ctx.gpr[0] = entry as u64;
    let (s, t, _) = create_thread_with_context(&mut ctx);
    c.eq(s.into(), ok)?;
    while WORKER_SLOT.load(Ordering::SeqCst) == 0 {
        yield_execution();
    }
    c.eq(WORKER_SLOT.load(Ordering::SeqCst), 1)?; // all slots zero
    c.eq(tls_slot(5), 0x77)?;
    c.eq(query_u32(CURRENT_THREAD, THREAD_AM_I_LAST_THREAD).1, 0)?;
    let (s, _) = query_thread(t, THREAD_WIN32_START_ADDRESS, &mut b);
    c.eq(s.into(), ok)?;
    c.eq(u64_at(&b, 0), entry as u64)?;

    // Priorities: BasePriority = process base (8) + increment.
    c.eq(set_thread(t, THREAD_BASE_PRIORITY, &1i32.to_le_bytes()).into(), ok)?;
    let mut basic = [0u8; 48];
    c.eq(query_thread(t, 0, &mut basic).0.into(), ok)?;
    c.eq(u32_at(&basic, 44), 9)?; // BasePriority
    c.eq(set_thread(t, THREAD_PRIORITY, &12i32.to_le_bytes()).into(), ok)?;
    c.eq(query_thread(t, 0, &mut basic).0.into(), ok)?;
    c.eq(u32_at(&basic, 40), 12)?; // Priority
    c.eq(
        set_thread(t, THREAD_PRIORITY, &12u64.to_le_bytes()).into(),
        STATUS_INFO_LENGTH_MISMATCH.into(),
    )?;
    c.eq(
        set_thread(t, THREAD_PRIORITY, &32i32.to_le_bytes()).into(),
        STATUS_INVALID_PARAMETER.into(),
    )?;

    // After exit: terminated, ExitTime set.
    RELEASE.store(1, Ordering::SeqCst);
    c.eq(wait(t, None).into(), ok)?;
    c.eq(query_u32(t, THREAD_IS_TERMINATED).1, 1)?;
    c.eq(query_thread(t, THREAD_TIMES, &mut times).0.into(), ok)?;
    c.ok(u64_at(&times, 8) >= u64_at(&times, 0) && u64_at(&times, 8) != 0, u64_at(&times, 8))?;
    close(t);
    c.eq(query_u32(CURRENT_THREAD, THREAD_AM_I_LAST_THREAD).1, 1)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("threadinfo_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
