//! TLB shootdowns when the thread that changed the mappings moves to
//! another processor, as on real Windows: a "changer" thread (no
//! affinity, so the scheduler moves it between processors) decommits and
//! recommits a range of pages (their frames going to another range in
//! between) and writes a new round number into each; a
//! "reader" thread pinned to processor 1 reads every page after each
//! round (so its processor's TLB holds the pages' translations) and must
//! see the new number. Two more unpinned threads keep both processors
//! busy, so the changer is preempted and moved often.
//!
//! The kernel invalidated the changed pages on the processor that changed
//! them, released the address space's lock and then sent the shootdown to
//! the other processors that had the space loaded, leaving out the one it
//! ran on at that moment. A changer preempted right after the release (an
//! interrupt held back during the change is taken there) and moved to the
//! reader's processor left that processor's old translations in place:
//! the reader (and the changer) went on using the decommitted pages'
//! frames, already freed and reused (stale data, lost writes: the Java
//! heap corruption, WORKFLOW "Known intermittent").
//!
//! Needs NumberOfProcessors >= 2. Prints `shootdown_test: ok <rounds>` or
//! exits with the number of the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ntrt::*;

const THREAD_AFFINITY_MASK: u32 = 4;
const PAGE: usize = 0x1000;
/// Pages changed per round (a longer hold of the address space's lock:
/// a timer interrupt arriving meanwhile is taken right at its release).
const PAGES: usize = 64;
const ROUNDS: u64 = 4000;
const TIME_LIMIT_MS: i64 = 20_000;

static BASE: AtomicU64 = AtomicU64::new(0);
/// The round the changer has finished writing, and the one the reader has
/// checked.
static DONE: AtomicU64 = AtomicU64::new(0);
static CHECKED: AtomicU64 = AtomicU64::new(0);
static STOP: AtomicBool = AtomicBool::new(false);
/// The first wrong value the reader saw: (round, page, value).
static WRONG: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
static FAILED: AtomicU64 = AtomicU64::new(0);

fn pin(cpu: u32) -> NtStatus {
    set_thread(CURRENT_THREAD, THREAD_AFFINITY_MASK, &(1u64 << cpu).to_le_bytes())
}

fn word(round: u64, page: usize) -> u64 {
    0x7eb0_0000_0000_0000 | round << 16 | page as u64
}

extern "C" fn reader(_: usize) -> ! {
    if pin(1) != STATUS_SUCCESS {
        FAILED.store(0x20, Ordering::SeqCst);
        STOP.store(true, Ordering::SeqCst);
        exit_thread(1);
    }
    let base = BASE.load(Ordering::SeqCst) as usize;
    let mut round = 1;
    while !STOP.load(Ordering::SeqCst) {
        if DONE.load(Ordering::SeqCst) < round {
            core::hint::spin_loop();
            continue;
        }
        for p in 0..PAGES {
            // SAFETY: committed by the changer for this round; it does not
            // decommit it before CHECKED says the round was read.
            let v = unsafe { ((base + p * PAGE) as *const u64).read_volatile() };
            if v != word(round, p) && FAILED.load(Ordering::SeqCst) == 0 {
                WRONG[0].store(round, Ordering::SeqCst);
                WRONG[1].store(p as u64, Ordering::SeqCst);
                WRONG[2].store(v, Ordering::SeqCst);
                FAILED.store(0x10, Ordering::SeqCst);
                STOP.store(true, Ordering::SeqCst);
            }
        }
        CHECKED.store(round, Ordering::SeqCst);
        round += 1;
    }
    exit_thread(STATUS_SUCCESS)
}

extern "C" fn spinner(_: usize) -> ! {
    let mut x = 0u64;
    while !STOP.load(Ordering::Relaxed) {
        x = core::hint::black_box(x.wrapping_add(1));
    }
    exit_thread(STATUS_SUCCESS)
}

fn run() -> Result<u64, u32> {
    let mut b = [0u8; 64];
    if query_system(0, &mut b).0 != STATUS_SUCCESS {
        return Err(1);
    }
    if b[56] < 2 {
        print("shootdown_test: one processor, skipped\r\n");
        return Ok(0);
    }
    let (s, base, _) = allocate(0, PAGES * PAGE, MEM_RESERVE, PAGE_READWRITE);
    if s != STATUS_SUCCESS {
        return Err(2);
    }
    BASE.store(base as u64, Ordering::SeqCst);
    // The decommitted pages' frames go here before the pages are
    // committed again (so they get other frames: an old translation then
    // shows this region's data).
    let (s, sink, _) = allocate(0, PAGES * PAGE, MEM_RESERVE, PAGE_READWRITE);
    if s != STATUS_SUCCESS {
        return Err(2);
    }
    let mut threads = [Handle(0); 3];
    let entries: [extern "C" fn(usize) -> !; 3] = [reader, spinner, spinner];
    for (t, entry) in threads.iter_mut().zip(entries) {
        let (s, h, _) = create_thread(entry, 0, false);
        if s != STATUS_SUCCESS {
            return Err(3);
        }
        *t = h;
    }
    let start = now_ms();
    let mut round = 0;
    while round < ROUNDS && now_ms() - start < TIME_LIMIT_MS && !STOP.load(Ordering::SeqCst) {
        round += 1;
        if round > 1 && free(base, PAGES * PAGE, MEM_DECOMMIT).0 != STATUS_SUCCESS {
            FAILED.store(4, Ordering::SeqCst);
            break;
        }
        if allocate(sink, PAGES * PAGE, MEM_COMMIT, PAGE_READWRITE).0 != STATUS_SUCCESS {
            FAILED.store(5, Ordering::SeqCst);
            break;
        }
        for p in 0..PAGES {
            // SAFETY: committed just above.
            unsafe { ((sink + p * PAGE) as *mut u64).write_volatile(0xdead_0000 | p as u64) };
        }
        if allocate(base, PAGES * PAGE, MEM_COMMIT, PAGE_READWRITE).0 != STATUS_SUCCESS {
            FAILED.store(5, Ordering::SeqCst);
            break;
        }
        for p in 0..PAGES {
            // SAFETY: committed just above.
            unsafe { ((base + p * PAGE) as *mut u64).write_volatile(word(round, p)) };
        }
        DONE.store(round, Ordering::SeqCst);
        let waited = now_ms();
        while CHECKED.load(Ordering::SeqCst) < round && !STOP.load(Ordering::SeqCst) {
            if now_ms() - waited > 10_000 {
                FAILED.store(6, Ordering::SeqCst);
                break;
            }
            yield_execution();
        }
        free(sink, PAGES * PAGE, MEM_DECOMMIT);
    }
    STOP.store(true, Ordering::SeqCst);
    for &t in &threads {
        if wait(t, Some(-100_000_000)) != STATUS_SUCCESS {
            terminate_thread(t, 1);
        }
        close(t);
    }
    match FAILED.load(Ordering::SeqCst) {
        0 => Ok(round),
        id => {
            print("shootdown_test: check ");
            write_file(stdout(), &hex32(id as u32));
            for w in &WRONG {
                let v = w.load(Ordering::SeqCst);
                print(" ");
                write_file(stdout(), &hex32((v >> 32) as u32));
                write_file(stdout(), &hex32(v as u32));
            }
            print("\r\n");
            Err(id as u32)
        }
    }
}

fn main() -> u32 {
    match run() {
        Ok(rounds) => {
            print("shootdown_test: ok ");
            write_file(stdout(), &hex32(rounds as u32));
            print("\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
