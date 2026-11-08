//! Scheduling on several processors, as on real Windows:
//! - two threads exchange 1000 messages through shared memory without any
//!   system call; with one processor every exchange needs both threads
//!   switched in (at least 2000 context switches, seconds of preemptions),
//!   with two they run at once: the threads' ContextSwitches
//!   (SystemProcessInformation) grow by far less than that, and the
//!   processor numbers (RDTSCP) seen by the two sides differ (in one of up
//!   to five attempts). Counted switches, not elapsed time: a loaded host
//!   (a virtual machine's processors not scheduled for a while) makes the
//!   exchange slow without the guest switching threads;
//! - ThreadAffinityMask (NtSetInformationThread class 4) pins the thread:
//!   RDTSCP and ntdll's NtGetCurrentProcessorNumber then report that
//!   processor; a mask of no or unknown processors is STATUS_INVALID_PARAMETER,
//!   and ThreadBasicInformation reports the mask;
//! - waits and wakes across processors: 1000 event round trips between two
//!   threads pinned to different processors.
//!
//! The multi-processor checks need NumberOfProcessors >= 2 (else skipped).
//! Prints `smpsched_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use ntrt::*;

const THREAD_AFFINITY_MASK: u32 = 4;
const STATUS_INVALID_PARAMETER: NtStatus = 0xc000_000d;
const ROUNDS: u32 = 1000;

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("smpsched_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        check_line(core::panic::Location::caller().line());
        print("\r\n");
        Err(self.0)
    }
}

fn processor() -> u32 {
    let aux: u32;
    // SAFETY: rdtscp only reads the TSC and IA32_TSC_AUX.
    unsafe {
        core::arch::asm!("rdtscp", out("eax") _, out("edx") _, out("ecx") aux, options(nomem, nostack))
    };
    aux & 0xfff
}

fn pin(h: Handle, mask: u64) -> NtStatus {
    set_thread(h, THREAD_AFFINITY_MASK, &mask.to_le_bytes())
}

/// Seconds of performance counter from now.
fn deadline(seconds: i64) -> i64 {
    let (now, freq) = performance_counter();
    now + seconds * freq
}

fn past(deadline: i64) -> bool {
    performance_counter().0 > deadline
}

// Shared-memory ping-pong: the main thread writes odd values, the partner
// answers with the next even one.
static BALL: AtomicU32 = AtomicU32::new(0);
static PARTNER_CPU: AtomicU32 = AtomicU32::new(u32::MAX);
static GIVE_UP: AtomicU64 = AtomicU64::new(0);
/// Set by the main thread once it has counted the partner's switches: the
/// partner stays (and stays listed) until then.
static DONE: AtomicU32 = AtomicU32::new(0);
/// How long an exchange may take at most (a bound against a hang only:
/// a loaded host can make it slow).
const GIVE_UP_SECONDS: i64 = 120;

extern "C" fn partner(_: usize) -> ! {
    let give_up = GIVE_UP.load(Ordering::Relaxed) as i64;
    let mut expect = 1;
    let mut spins = 0u32;
    while expect < 2 * ROUNDS {
        if BALL.load(Ordering::Acquire) == expect {
            PARTNER_CPU.store(processor(), Ordering::Relaxed);
            BALL.store(expect + 1, Ordering::Release);
            expect += 2;
        } else {
            spins = spins.wrapping_add(1);
            // A rare clock read bounds the test without yielding often.
            if spins.is_multiple_of(1 << 16) && past(give_up) {
                break;
            }
            core::hint::spin_loop();
        }
    }
    while DONE.load(Ordering::Acquire) == 0 && !past(give_up) {
        core::hint::spin_loop();
    }
    exit_thread(STATUS_SUCCESS)
}

const SYSTEM_PROCESS_INFORMATION: u32 = 5;
const STATUS_INFO_LENGTH_MISMATCH: NtStatus = 0xc000_0004;
/// SYSTEM_PROCESS_INFORMATION's size on x64 (its threads follow it), and
/// SYSTEM_THREAD_INFORMATION's.
const PROCESS_ENTRY: usize = 0x100;
const THREAD_ENTRY: usize = 0x50;

/// The sum of the ContextSwitches of this process's threads `tids`
/// (SystemProcessInformation), or None if the query failed.
fn context_switches(buf: &mut [u8], tids: &[usize]) -> Option<u64> {
    let (st, _) = query_system(SYSTEM_PROCESS_INFORMATION, buf);
    if st != STATUS_SUCCESS {
        return None;
    }
    let pid = teb_field(0x40);
    let read = |at: usize, n: usize| -> Option<u64> {
        let b = buf.get(at..at + n)?;
        let mut v = [0u8; 8];
        v[..n].copy_from_slice(b);
        Some(u64::from_le_bytes(v))
    };
    let mut at = 0;
    loop {
        let threads = read(at + 4, 4)? as usize;
        if read(at + 0x50, 8)? as usize == pid {
            let mut sum = 0;
            for i in 0..threads {
                let t = at + PROCESS_ENTRY + THREAD_ENTRY * i;
                if tids.contains(&(read(t + 0x30, 8)? as usize)) {
                    sum += read(t + 0x40, 4)?;
                }
            }
            return Some(sum);
        }
        match read(at, 4)? {
            0 => return None,
            next => at += next as usize,
        }
    }
}

/// Plays ROUNDS exchanges; returns (completed rounds, whether the two
/// sides were ever seen on different processors at the same exchange).
fn ping_pong() -> (u32, bool) {
    let give_up = deadline(GIVE_UP_SECONDS);
    let mut different = false;
    let mut spins = 0u32;
    for round in 0..ROUNDS {
        BALL.store(2 * round + 1, Ordering::Release);
        while BALL.load(Ordering::Acquire) != 2 * round + 2 {
            spins = spins.wrapping_add(1);
            if spins.is_multiple_of(1 << 16) && past(give_up) {
                return (round, different);
            }
            core::hint::spin_loop();
        }
        different |= PARTNER_CPU.load(Ordering::Relaxed) != processor();
    }
    (ROUNDS, different)
}

// Event ping-pong between pinned threads.
static EVENTS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
static ANSWERED: AtomicU32 = AtomicU32::new(0);

extern "C" fn event_partner(cpu: usize) -> ! {
    if pin(CURRENT_THREAD, 1 << cpu) != STATUS_SUCCESS {
        exit_thread(1);
    }
    let (ping, pong) = (
        Handle(EVENTS[0].load(Ordering::Relaxed) as usize),
        Handle(EVENTS[1].load(Ordering::Relaxed) as usize),
    );
    for _ in 0..ROUNDS {
        if wait(ping, Some(-50_000_000)) != STATUS_SUCCESS || processor() != cpu as u32 {
            exit_thread(2);
        }
        ANSWERED.fetch_add(1, Ordering::Relaxed);
        set_event(pong);
    }
    exit_thread(STATUS_SUCCESS)
}

type GetProcessorNumber = extern "system" fn() -> u32;

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = u64::from(STATUS_SUCCESS);

    let mut b = [0u8; 64];
    c.eq(query_system(0, &mut b).0.into(), ok)?;
    let cpus = u32::from(b[56]);
    let all = u64::from_le_bytes(b[48..56].try_into().unwrap());
    let Some(get_number) =
        find_ntdll().and_then(|n| find_export(n, b"NtGetCurrentProcessorNumber"))
    else {
        return c.ok(false, 0);
    };
    // SAFETY: ntdll's NtGetCurrentProcessorNumber takes no arguments.
    let get_number = unsafe { core::mem::transmute::<usize, GetProcessorNumber>(get_number) };

    // Pinning to each processor in turn.
    for cpu in 0..cpus.min(64) {
        c.eq(pin(CURRENT_THREAD, 1 << cpu).into(), ok)?;
        let mut t = [0u8; 48];
        c.eq(query_thread(CURRENT_THREAD, 0, &mut t).0.into(), ok)?;
        c.eq(u64::from_le_bytes(t[32..40].try_into().unwrap()), 1 << cpu)?;
        for _ in 0..16 {
            c.eq(processor().into(), cpu.into())?;
            c.eq(get_number().into(), cpu.into())?;
            c.0 -= 2; // the same two checks each time
            yield_execution();
        }
        c.0 += 2;
    }
    c.eq(pin(CURRENT_THREAD, 0).into(), STATUS_INVALID_PARAMETER.into())?;
    if cpus < 64 {
        c.eq(pin(CURRENT_THREAD, 1 << cpus).into(), STATUS_INVALID_PARAMETER.into())?;
    }
    c.eq(pin(CURRENT_THREAD, all).into(), ok)?;

    if cpus < 2 {
        print("smpsched_test: one processor, concurrency checks skipped\r\n");
        return Ok(());
    }

    // Two threads running at the same time. Windows does not promise that
    // two ready threads run at once at any given moment (other work on
    // the machine can put them on one processor for a while): the
    // exchange is tried up to five times and must once be seen on two
    // processors with few context switches. Each attempt must complete.
    // Room for SystemProcessInformation on a real machine (hundreds of
    // processes; 256 KiB overflowed on Windows with
    // STATUS_INFO_LENGTH_MISMATCH).
    let (st, list, size) = allocate(0, 1 << 23, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    c.eq(st.into(), ok)?;
    // SAFETY: just committed, read-write, ours alone.
    let list = unsafe { core::slice::from_raw_parts_mut(list as *mut u8, size) };
    let me = teb_field(0x48);
    let mut attempts = Checks(c.0);
    let mut concurrent = (false, 0);
    for _ in 0..5 {
        attempts.0 = c.0;
        BALL.store(0, Ordering::Relaxed);
        PARTNER_CPU.store(u32::MAX, Ordering::Relaxed);
        DONE.store(0, Ordering::Relaxed);
        GIVE_UP.store(deadline(GIVE_UP_SECONDS) as u64, Ordering::Relaxed);
        let (s, h, tid) = create_thread(partner, 0, false);
        attempts.eq(s.into(), ok)?;
        let before = context_switches(list, &[me, tid]);
        let (rounds, different) = ping_pong();
        let after = context_switches(list, &[me, tid]);
        DONE.store(1, Ordering::Release);
        attempts.eq(rounds.into(), ROUNDS.into())?;
        attempts.eq(wait(h, Some(-50_000_000)).into(), ok)?;
        close(h);
        let (Some(before), Some(after)) = (before, after) else {
            return attempts.ok(false, u64::from(STATUS_INFO_LENGTH_MISMATCH));
        };
        // Half the 2 * ROUNDS one processor would need at least. Not
        // near 0: each time other work (the system's own threads) takes
        // a processor, the spinning thread is switched in again after it,
        // and that grows with the exchange's elapsed time: about 300 when
        // the host ran the VM's two processors on one of its own.
        let switches = after.saturating_sub(before);
        concurrent = (different && switches < u64::from(ROUNDS), switches);
        if concurrent.0 {
            break;
        }
    }
    c.0 = attempts.0;
    c.ok(concurrent.0, concurrent.1)?;
    // What it took (one line, one write): for a timing question later.
    let mut line = *b"smpsched_test: exchange context switches 00000000\r\n";
    line[41..49].copy_from_slice(&hex32(concurrent.1 as u32));
    write_file(stdout(), &line);

    // Event round trips between threads pinned to processors 0 and 1.
    c.eq(pin(CURRENT_THREAD, 1).into(), ok)?;
    for slot in &EVENTS {
        let (s, e) = create_event(EVENT_ALL_ACCESS, SYNCHRONIZATION_EVENT, false);
        c.eq(s.into(), ok)?;
        slot.store(e.0 as u64, Ordering::Relaxed);
    }
    let (ping, pong) = (
        Handle(EVENTS[0].load(Ordering::Relaxed) as usize),
        Handle(EVENTS[1].load(Ordering::Relaxed) as usize),
    );
    let (s, h, _) = create_thread(event_partner, 1, false);
    c.eq(s.into(), ok)?;
    let mut answered = 0;
    for _ in 0..ROUNDS {
        set_event(ping);
        if wait(pong, Some(-50_000_000)) != STATUS_SUCCESS || processor() != 0 {
            break;
        }
        answered += 1;
    }
    c.eq(answered, ROUNDS.into())?;
    c.eq(ANSWERED.load(Ordering::Relaxed).into(), ROUNDS.into())?;
    c.eq(wait(h, Some(-50_000_000)).into(), ok)?;
    let (s, code, _, _) = thread_basic_info(h);
    c.eq(s.into(), ok)?;
    c.eq(code.into(), ok)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("smpsched_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);

/// ` (line L)`: the source line of the failing check, from
/// `#[track_caller]` on Checks' methods (so a report names the line
/// without anyone counting checks).
fn check_line(line: u32) {
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
    print(core::str::from_utf8(&b[..8 + len]).unwrap_or(""));
}
