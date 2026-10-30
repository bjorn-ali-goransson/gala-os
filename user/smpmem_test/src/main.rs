//! Memory changes under threads running on other processors, as on real
//! Windows: when one thread frees (NtFreeVirtualMemory), write-protects
//! (NtProtectVirtualMemory) or unmaps (NtUnmapViewOfSection) memory that a
//! thread on another processor is using in a tight loop without system
//! calls, that thread must fault promptly (a vectored handler, through
//! ntdll, turns the fault into a thread exit) and never see other data.
//! Also: interlocked increments from two threads add up exactly, and four
//! pairs of threads ping-pong 1000 events each at the same time.
//!
//! Needs NumberOfProcessors >= 2 (else only the counting checks run).
//! Prints `smpmem_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ntrt::*;

const THREAD_AFFINITY_MASK: u32 = 4;
const STATUS_ACCESS_VIOLATION: u32 = 0xc000_0005;
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
/// Exit status of a thread whose access faulted.
const FAULTED: NtStatus = 0x0fa1_7ed0;
const PATTERN: u64 = 0x5a5a_1234_8765_a5a5;
const CTX_RSP: usize = 0x98;
const CTX_RIP: usize = 0xf8;

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
        print("smpmem_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn pin(cpu: u32) -> NtStatus {
    set_thread(CURRENT_THREAD, THREAD_AFFINITY_MASK, &(1u64 << cpu).to_le_bytes())
}

#[repr(C)]
struct ExceptionPointers {
    record: *const u8,
    context: *mut u8,
}

extern "C" fn fault_exit() -> ! {
    exit_thread(FAULTED)
}

/// Turns an access violation into `fault_exit` on the faulting thread.
extern "system" fn handler(pointers: *mut ExceptionPointers) -> i32 {
    // SAFETY: the dispatcher passes valid EXCEPTION_POINTERS; the offsets
    // are those of the public x64 EXCEPTION_RECORD and CONTEXT.
    unsafe {
        let (record, context) = ((*pointers).record, (*pointers).context);
        if (record as *const u32).read_unaligned() != STATUS_ACCESS_VIOLATION {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let rsp = (context.add(CTX_RSP) as *const u64).read_unaligned();
        // As if `fault_exit` had been called: rsp % 16 == 8 at its entry.
        (context.add(CTX_RSP) as *mut u64).write_unaligned((rsp & !0xf) - 0x108);
        (context.add(CTX_RIP) as *mut u64).write_unaligned(fault_exit as *const () as u64);
    }
    EXCEPTION_CONTINUE_EXECUTION
}

/// Accesses made by the current victim thread, and whether it ever saw a
/// value other than PATTERN.
static ACCESSES: AtomicU64 = AtomicU64::new(0);
static WRONG: AtomicBool = AtomicBool::new(false);

/// Reads `addr` on processor 1 until that faults.
extern "C" fn reader(addr: usize) -> ! {
    if pin(1) != STATUS_SUCCESS {
        exit_thread(1);
    }
    loop {
        // SAFETY: valid until the main thread takes it away; then this
        // faults and the handler ends the thread.
        let v = unsafe { (addr as *const u64).read_volatile() };
        if v != PATTERN {
            WRONG.store(true, Ordering::Relaxed);
        }
        ACCESSES.fetch_add(1, Ordering::Relaxed);
    }
}

/// Writes `addr` on processor 1 until that faults.
extern "C" fn writer(addr: usize) -> ! {
    if pin(1) != STATUS_SUCCESS {
        exit_thread(1);
    }
    loop {
        // SAFETY: as in `reader`.
        unsafe { (addr as *mut u64).write_volatile(PATTERN) };
        ACCESSES.fetch_add(1, Ordering::Relaxed);
    }
}

/// Runs `victim(addr)` on processor 1, lets it get going, calls `take`
/// (which removes its access) and checks that it faults within 5 s
/// without seeing other data.
fn victim_faults(
    c: &mut Checks,
    victim: extern "C" fn(usize) -> !,
    addr: usize,
    take: impl FnOnce() -> NtStatus,
) -> Result<(), u32> {
    ACCESSES.store(0, Ordering::Relaxed);
    WRONG.store(false, Ordering::Relaxed);
    // Let earlier threads be cleaned up first, so that nothing else
    // happens to flush the victim's processor while it runs.
    delay(-2_000_000); // 200 ms
    let (s, h, _) = create_thread(victim, addr, false);
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    let (start, freq) = performance_counter();
    while ACCESSES.load(Ordering::Relaxed) < 10_000 && performance_counter().0 - start < 5 * freq {
        yield_execution();
    }
    c.ok(ACCESSES.load(Ordering::Relaxed) >= 10_000, 0)?;
    c.eq(take().into(), STATUS_SUCCESS.into())?;
    let exited = wait(h, Some(-50_000_000));
    if exited != STATUS_SUCCESS {
        terminate_thread(h, 0);
    }
    c.eq(exited.into(), STATUS_SUCCESS.into())?;
    let (s, code, _, _) = thread_basic_info(h);
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.eq(code.into(), FAULTED.into())?;
    c.ok(!WRONG.load(Ordering::Relaxed), 0)?;
    close(h);
    Ok(())
}

fn committed_page(c: &mut Checks) -> Result<usize, u32> {
    let (s, base, _) = allocate(0, 0x1000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    // SAFETY: freshly committed read-write page.
    unsafe { (base as *mut u64).write_volatile(PATTERN) };
    Ok(base)
}

static COUNTER: AtomicU64 = AtomicU64::new(0);
const INCREMENTS: u64 = 100_000;

extern "C" fn incrementer(_: usize) -> ! {
    for _ in 0..INCREMENTS {
        COUNTER.fetch_add(1, Ordering::SeqCst); // lock xadd, InterlockedIncrement
    }
    exit_thread(STATUS_SUCCESS)
}

const PAIRS: usize = 4;
const ROUNDS: u64 = 1000;
static PINGS: [AtomicU64; PAIRS] = [const { AtomicU64::new(0) }; PAIRS];
static PONGS: [AtomicU64; PAIRS] = [const { AtomicU64::new(0) }; PAIRS];
static ANSWERED: AtomicU64 = AtomicU64::new(0);

extern "C" fn ponger(i: usize) -> ! {
    let (ping, pong) = (
        Handle(PINGS[i].load(Ordering::Relaxed) as usize),
        Handle(PONGS[i].load(Ordering::Relaxed) as usize),
    );
    for _ in 0..ROUNDS {
        if wait(ping, Some(-100_000_000)) != STATUS_SUCCESS {
            exit_thread(1);
        }
        ANSWERED.fetch_add(1, Ordering::Relaxed);
        set_event(pong);
    }
    exit_thread(STATUS_SUCCESS)
}

extern "C" fn pinger(i: usize) -> ! {
    let (ping, pong) = (
        Handle(PINGS[i].load(Ordering::Relaxed) as usize),
        Handle(PONGS[i].load(Ordering::Relaxed) as usize),
    );
    for _ in 0..ROUNDS {
        set_event(ping);
        if wait(pong, Some(-100_000_000)) != STATUS_SUCCESS {
            exit_thread(1);
        }
    }
    exit_thread(STATUS_SUCCESS)
}

type AddHandler =
    extern "system" fn(u32, extern "system" fn(*mut ExceptionPointers) -> i32) -> usize;

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = u64::from(STATUS_SUCCESS);
    let mut b = [0u8; 64];
    c.eq(query_system(0, &mut b).0.into(), ok)?;
    let cpus = u32::from(b[56]);

    // Interlocked increments.
    let mut threads = [Handle(0); 2];
    for t in &mut threads {
        let (s, h, _) = create_thread(incrementer, 0, false);
        c.eq(s.into(), ok)?;
        *t = h;
    }
    for &h in &threads {
        c.eq(wait(h, Some(-100_000_000)).into(), ok)?;
        close(h);
    }
    c.eq(COUNTER.load(Ordering::SeqCst), 2 * INCREMENTS)?;

    // Four event ping-pongs at once.
    for i in 0..PAIRS {
        let (s, ping) = create_event(EVENT_ALL_ACCESS, SYNCHRONIZATION_EVENT, false);
        c.eq(s.into(), ok)?;
        let (s, pong) = create_event(EVENT_ALL_ACCESS, SYNCHRONIZATION_EVENT, false);
        c.eq(s.into(), ok)?;
        PINGS[i].store(ping.0 as u64, Ordering::Relaxed);
        PONGS[i].store(pong.0 as u64, Ordering::Relaxed);
    }
    let mut storm = [Handle(0); 2 * PAIRS];
    for i in 0..PAIRS {
        let (s, h, _) = create_thread(ponger, i, false);
        c.eq(s.into(), ok)?;
        storm[2 * i] = h;
        let (s, h, _) = create_thread(pinger, i, false);
        c.eq(s.into(), ok)?;
        storm[2 * i + 1] = h;
    }
    for &h in &storm {
        c.eq(wait(h, Some(-200_000_000)).into(), ok)?;
        let (_, code, _, _) = thread_basic_info(h);
        c.eq(code.into(), ok)?;
        close(h);
    }
    c.eq(ANSWERED.load(Ordering::Relaxed), PAIRS as u64 * ROUNDS)?;

    if cpus < 2 {
        print("smpmem_test: one processor, cross-processor checks skipped\r\n");
        return Ok(());
    }
    let Some(add) = find_ntdll().and_then(|n| find_export(n, b"RtlAddVectoredExceptionHandler"))
    else {
        return c.ok(false, 0);
    };
    // SAFETY: ntdll's RtlAddVectoredExceptionHandler.
    let add = unsafe { core::mem::transmute::<usize, AddHandler>(add) };
    c.ok(add(1, handler) != 0, 0)?;
    c.eq(pin(0).into(), ok)?;

    // Freed under a reader.
    let page = committed_page(&mut c)?;
    victim_faults(&mut c, reader, page, || free(page, 0, MEM_RELEASE).0)?;
    // Decommitted under a reader.
    let page = committed_page(&mut c)?;
    victim_faults(&mut c, reader, page, || free(page, 0x1000, MEM_DECOMMIT).0)?;
    // Write-protected under a writer.
    let page = committed_page(&mut c)?;
    victim_faults(&mut c, writer, page, || protect(page, 0x1000, PAGE_READONLY).0)?;
    // A section view unmapped under a reader.
    let (s, section) = create_section(None, Some(0x1000), PAGE_READWRITE);
    c.eq(s.into(), ok)?;
    let (s, view, _) = map_view(section, 0, 0, PAGE_READWRITE);
    c.eq(s.into(), ok)?;
    // SAFETY: the view is mapped read-write.
    unsafe { (view as *mut u64).write_volatile(PATTERN) };
    victim_faults(&mut c, reader, view, || unmap_view(view))?;
    close(section);
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("smpmem_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
