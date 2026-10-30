//! Scheduling on several processors, as on real Windows:
//! - two threads exchange 1000 messages through shared memory without any
//!   system call; with one processor that would need ~1000 preemptions
//!   (seconds), with two it is quick, and the processor numbers (RDTSCP)
//!   seen by the two sides differ;
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
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("smpsched_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
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
    exit_thread(STATUS_SUCCESS)
}

/// Plays ROUNDS exchanges; returns (completed rounds, whether the two
/// sides were ever seen on different processors at the same exchange).
fn ping_pong() -> (u32, bool) {
    let give_up = deadline(10);
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

    // Two threads running at the same time.
    GIVE_UP.store(deadline(10) as u64, Ordering::Relaxed);
    let (s, h, _) = create_thread(partner, 0, false);
    c.eq(s.into(), ok)?;
    let start = performance_counter().0;
    let (rounds, different) = ping_pong();
    let (end, freq) = performance_counter();
    c.eq(rounds.into(), ROUNDS.into())?;
    c.ok(different, 0)?;
    // Far below the ~1000 quanta one processor would need.
    c.ok(end - start < 2 * freq, (end - start) as u64)?;
    c.eq(wait(h, Some(-50_000_000)).into(), ok)?;
    close(h);

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
