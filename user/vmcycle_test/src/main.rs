//! Commit, decommit, recommit, protect and MEM_RESET as a garbage
//! collector uses them (HotSpot grows and shrinks its generations and card
//! table with VirtualAlloc(MEM_COMMIT) and VirtualFree(MEM_DECOMMIT)), as
//! on real Windows:
//!
//! - pages committed again after a decommit read zero, across 2 MiB
//!   boundaries, and the pages around them keep their contents;
//! - NtQueryVirtualMemory reports the runs of committed, reserved and
//!   re-protected pages with their exact boundaries;
//! - committing a range that is partly committed keeps the committed
//!   part's contents and zeroes the rest;
//! - MEM_RESET and MEM_RESET_UNDO succeed on committed memory (gala keeps
//!   the contents, which Windows allows: MEM_RESET_UNDO then succeeds);
//! - under two processors: one thread pinned to processor 1 keeps
//!   counters in pages of one reservation (checking each one it reads
//!   back) while the main thread on processor 0 commits, checks, writes,
//!   re-protects and decommits the 64 KiB chunks between them, many times.
//!   No counter may lose an update and every recommitted page reads zero.
//!
//! Prints `vmcycle_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use ntrt::*;

const THREAD_AFFINITY_MASK: u32 = 4;
const MEM_RESET: u32 = 0x8_0000;
const MEM_RESET_UNDO: u32 = 0x100_0000;
const PAGE: usize = 0x1000;
const CHUNK: usize = 0x1_0000;
/// Chunks in the concurrent phase's reservation (4 MiB: two 2 MiB
/// boundaries at most).
const CHUNKS: usize = 64;
const ROUNDS: usize = 12;
const MIB: usize = 0x10_0000;

/// Check `id` (also the exit status when it fails).
fn check(id: u32, cond: bool, got: u64) -> Result<(), u32> {
    if cond {
        return Ok(());
    }
    print("vmcycle_test: check ");
    write_file(stdout(), &hex32(id));
    print(" got ");
    write_file(stdout(), &hex32((got >> 32) as u32));
    write_file(stdout(), &hex32(got as u32));
    print("\r\n");
    Err(id)
}

fn status(id: u32, s: NtStatus) -> Result<(), u32> {
    check(id, s == STATUS_SUCCESS, s.into())
}

fn word(addr: usize) -> u64 {
    // SAFETY: callers pass committed, readable addresses.
    unsafe { (addr as *const u64).read_volatile() }
}

fn set_word(addr: usize, value: u64) {
    // SAFETY: callers pass committed, writable addresses.
    unsafe { (addr as *mut u64).write_volatile(value) }
}

/// Every word of the pages `[base, base + size)` is zero.
fn all_zero(base: usize, size: usize) -> Option<usize> {
    (base..base + size).step_by(8).find(|&a| word(a) != 0)
}

/// The marker left in each page by the single-thread phase.
fn marker(addr: usize) -> u64 {
    0x5eed_0000_0000_0000 | addr as u64
}

fn fill(base: usize, size: usize) {
    for page in (base..base + size).step_by(PAGE) {
        set_word(page, marker(page));
        set_word(page + PAGE - 8, marker(page));
    }
}

/// The first page of `[base, base + size)` whose markers are not there.
fn filled(base: usize, size: usize) -> Option<usize> {
    (base..base + size)
        .step_by(PAGE)
        .find(|&p| word(p) != marker(p) || word(p + PAGE - 8) != marker(p))
}

/// The region at `addr` is `[base, base + size)` with `state` and `protect`.
fn region(
    id: u32,
    addr: usize,
    base: usize,
    size: usize,
    state: u32,
    protect: u32,
) -> Result<(), u32> {
    let (s, mbi) = query(addr);
    status(id, s)?;
    check(id, mbi.base_address == base, mbi.base_address as u64)?;
    check(id, mbi.region_size == size, mbi.region_size as u64)?;
    check(id, mbi.state == state, mbi.state.into())?;
    check(id, mbi.protect == protect, mbi.protect.into())?;
    check(id, mbi.kind == MEM_PRIVATE, mbi.kind.into())
}

/// A reservation of `size` whose base is `align`-aligned (reserved large,
/// released, reserved again at the aligned address).
fn reserve_aligned(id: u32, size: usize, align: usize) -> Result<usize, u32> {
    let (s, big, _) = allocate(0, size + align, MEM_RESERVE, PAGE_READWRITE);
    status(id, s)?;
    status(id, free(big, 0, MEM_RELEASE).0)?;
    // Just below a 2 MiB boundary: the reservation crosses it.
    let base = big.next_multiple_of(align) + align / 2;
    let (s, got, _) = allocate(base, size, MEM_RESERVE, PAGE_READWRITE);
    status(id, s)?;
    check(id, got == base, got as u64)?;
    Ok(base)
}

fn single() -> Result<(), u32> {
    // 8 MiB reserved from 1 MiB below a 2 MiB boundary.
    let size = 8 * MIB;
    let base = reserve_aligned(1, size, 2 * MIB)?;
    let boundary = base + MIB;
    let (s, _, got) = allocate(base, 4 * MIB, MEM_COMMIT, PAGE_READWRITE);
    status(2, s)?;
    check(2, got == 4 * MIB, got as u64)?;
    check(3, all_zero(base, 4 * MIB).is_none(), 0)?;
    fill(base, 4 * MIB);
    region(4, base, base, 4 * MIB, MEM_COMMIT, PAGE_READWRITE)?;
    region(5, base + 4 * MIB, base + 4 * MIB, 4 * MIB, MEM_RESERVE, 0)?;

    // Decommit 1 MiB around the 2 MiB boundary.
    let (dstart, dsize) = (boundary - MIB / 2, MIB);
    let (s, b, n) = free(dstart, dsize, MEM_DECOMMIT);
    status(6, s)?;
    check(6, b == dstart && n == dsize, n as u64)?;
    region(7, base, base, dstart - base, MEM_COMMIT, PAGE_READWRITE)?;
    region(8, dstart + PAGE, dstart + PAGE, dsize - PAGE, MEM_RESERVE, 0)?;
    region(
        9,
        dstart + dsize,
        dstart + dsize,
        base + 4 * MIB - (dstart + dsize),
        MEM_COMMIT,
        PAGE_READWRITE,
    )?;
    check(10, filled(base, dstart - base).is_none(), 0)?;
    check(10, filled(dstart + dsize, base + 4 * MIB - (dstart + dsize)).is_none(), 0)?;
    // Committed again: zero.
    let (s, _, _) = allocate(dstart, dsize, MEM_COMMIT, PAGE_READWRITE);
    status(11, s)?;
    let bad = all_zero(dstart, dsize);
    check(12, bad.is_none(), bad.unwrap_or(0) as u64)?;
    region(13, base, base, 4 * MIB, MEM_COMMIT, PAGE_READWRITE)?;
    fill(dstart, dsize);

    // Decommit page by page across the boundary, recommit in one call.
    for p in (boundary - 4 * PAGE..boundary + 4 * PAGE).step_by(PAGE) {
        status(14, free(p, PAGE, MEM_DECOMMIT).0)?;
    }
    region(15, boundary - 4 * PAGE, boundary - 4 * PAGE, 8 * PAGE, MEM_RESERVE, 0)?;
    status(16, allocate(boundary - 8 * PAGE, 16 * PAGE, MEM_COMMIT, PAGE_READWRITE).0)?;
    check(17, filled(boundary - 8 * PAGE, 4 * PAGE).is_none(), 0)?;
    check(17, filled(boundary + 4 * PAGE, 4 * PAGE).is_none(), 0)?;
    let bad = all_zero(boundary - 4 * PAGE, 8 * PAGE);
    check(18, bad.is_none(), bad.unwrap_or(0) as u64)?;
    fill(boundary - 4 * PAGE, 8 * PAGE);

    // Protection of a sub-range across the boundary.
    let (pstart, psize) = (boundary - 3 * PAGE, 6 * PAGE);
    let (s, old) = protect(pstart, psize, PAGE_READONLY);
    status(19, s)?;
    check(19, old == PAGE_READWRITE, old.into())?;
    region(20, pstart - PAGE, pstart - PAGE, PAGE, MEM_COMMIT, PAGE_READWRITE)?;
    region(20, base, base, pstart - base, MEM_COMMIT, PAGE_READWRITE)?;
    region(21, pstart, pstart, psize, MEM_COMMIT, PAGE_READONLY)?;
    region(
        22,
        pstart + psize,
        pstart + psize,
        base + 4 * MIB - pstart - psize,
        MEM_COMMIT,
        PAGE_READWRITE,
    )?;
    check(23, filled(base, 4 * MIB).is_none(), 0)?;
    let (s, old) = protect(pstart, psize, PAGE_NOACCESS);
    status(24, s)?;
    check(24, old == PAGE_READONLY, old.into())?;
    let (s, old) = protect(pstart, psize, PAGE_READWRITE);
    status(25, s)?;
    check(25, old == PAGE_NOACCESS, old.into())?;
    check(26, filled(base, 4 * MIB).is_none(), 0)?;
    region(27, base, base, 4 * MIB, MEM_COMMIT, PAGE_READWRITE)?;
    // Decommit a re-protected range: committed again it is read/write and zero.
    status(28, protect(pstart, psize, PAGE_READONLY).0)?;
    status(28, free(pstart, psize, MEM_DECOMMIT).0)?;
    status(28, allocate(pstart, psize, MEM_COMMIT, PAGE_READWRITE).0)?;
    region(29, base, base, 4 * MIB, MEM_COMMIT, PAGE_READWRITE)?;
    check(30, all_zero(pstart, psize).is_none(), 0)?;
    fill(pstart, psize);

    // Commit a range half committed: the committed half keeps its data.
    let (s, _, _) = allocate(base + 3 * MIB, 2 * MIB, MEM_COMMIT, PAGE_READWRITE);
    status(31, s)?;
    check(32, filled(base, 4 * MIB).is_none(), 0)?;
    check(33, all_zero(base + 4 * MIB, MIB).is_none(), 0)?;
    region(34, base, base, 5 * MIB, MEM_COMMIT, PAGE_READWRITE)?;

    // MEM_RESET then MEM_RESET_UNDO: both succeed; gala keeps the data,
    // so the undo reports it intact (and then it must be).
    let (s, b, n) = allocate(base + MIB, MIB, MEM_RESET, PAGE_NOACCESS);
    status(35, s)?;
    check(35, b == base + MIB && n == MIB, n as u64)?;
    region(36, base, base, 5 * MIB, MEM_COMMIT, PAGE_READWRITE)?;
    let (s, _, _) = allocate(base + MIB, MIB, MEM_RESET_UNDO, PAGE_READWRITE);
    status(37, s)?;
    check(38, filled(base + MIB, MIB).is_none(), 0)?;

    // Decommit to the end of the reservation (size 0), recommit: zero.
    status(39, free(base + 2 * MIB, 0, MEM_DECOMMIT).0)?;
    region(40, base + 2 * MIB, base + 2 * MIB, 6 * MIB, MEM_RESERVE, 0)?;
    status(41, allocate(base + 2 * MIB, 6 * MIB, MEM_COMMIT, PAGE_READWRITE).0)?;
    let bad = all_zero(base + 2 * MIB, 6 * MIB);
    check(42, bad.is_none(), bad.unwrap_or(0) as u64)?;
    check(43, filled(base, 2 * MIB).is_none(), 0)?;
    status(44, free(base, 0, MEM_RELEASE).0)?;
    region_free(45, base)
}

fn region_free(id: u32, addr: usize) -> Result<(), u32> {
    let (s, mbi) = query(addr);
    status(id, s)?;
    check(id, mbi.state == MEM_FREE, mbi.state.into())
}

fn pin(cpu: usize) -> NtStatus {
    set_thread(CURRENT_THREAD, THREAD_AFFINITY_MASK, &(1u64 << cpu).to_le_bytes())
}

static BASE: AtomicUsize = AtomicUsize::new(0);
static STOP: AtomicBool = AtomicBool::new(false);
/// A counter that read back wrong: (address << 16) | 1, 0 if none.
static LOST: AtomicU64 = AtomicU64::new(0);
static PASSES: AtomicU64 = AtomicU64::new(0);

/// Counter words per page the writer keeps (spread over the page).
const WORDS: usize = 4;

/// The writer: in every even chunk, each page holds counters only it
/// writes; each pass adds one to each after checking it still holds what
/// it wrote last.
extern "C" fn writer(_: usize) -> ! {
    if pin(1) != STATUS_SUCCESS {
        exit_thread(1);
    }
    let base = BASE.load(Ordering::Acquire);
    let mut pass = 0u64;
    while !STOP.load(Ordering::Acquire) {
        for chunk in (0..CHUNKS).step_by(2) {
            for page in (base + chunk * CHUNK..base + (chunk + 1) * CHUNK).step_by(PAGE) {
                for w in 0..WORDS {
                    let a = page + w * (PAGE / WORDS) + 8 * (w + 1);
                    let v = word(a);
                    if v != pass && LOST.load(Ordering::Relaxed) == 0 {
                        LOST.store((a as u64) << 16 | 1, Ordering::Relaxed);
                    }
                    set_word(a, pass + 1);
                }
            }
        }
        pass += 1;
        PASSES.store(pass, Ordering::Release);
    }
    exit_thread(STATUS_SUCCESS)
}

fn concurrent() -> Result<(), u32> {
    let mut b = [0u8; 64];
    status(50, query_system(0, &mut b).0)?;
    if b[56] < 2 {
        print("vmcycle_test: one processor, concurrent phase skipped\r\n");
        return Ok(());
    }
    status(51, pin(0))?;
    let size = CHUNKS * CHUNK;
    let base = reserve_aligned(52, size, 2 * MIB)?;
    for chunk in (0..CHUNKS).step_by(2) {
        status(53, allocate(base + chunk * CHUNK, CHUNK, MEM_COMMIT, PAGE_READWRITE).0)?;
    }
    BASE.store(base, Ordering::Release);
    let (s, thread, _) = create_thread(writer, 0, false);
    status(54, s)?;
    while PASSES.load(Ordering::Acquire) == 0 {
        yield_execution();
    }
    let mut result = Ok(());
    'rounds: for round in 0..ROUNDS {
        for chunk in (1..CHUNKS).step_by(2) {
            let c = base + chunk * CHUNK;
            // Every other round: commit with the even neighbours (already
            // committed, in use by the writer) in one call.
            let r = if round % 2 == 0 {
                allocate(c, CHUNK, MEM_COMMIT, PAGE_READWRITE).0
            } else {
                let (s, _, _) = allocate(
                    c - CHUNK,
                    (3 * CHUNK).min(base + size - (c - CHUNK)),
                    MEM_COMMIT,
                    PAGE_READWRITE,
                );
                s
            };
            result = status(55, r);
            if result.is_err() {
                break 'rounds;
            }
            if let Some(bad) = all_zero(c, CHUNK) {
                result = check(56, false, (bad as u64) << 8 | round as u64);
                break 'rounds;
            }
            for page in (c..c + CHUNK).step_by(PAGE) {
                set_word(page + 8 * (round % 64), marker(page));
            }
            let half = c + CHUNK / 2;
            result = status(57, protect(half, PAGE * 2, PAGE_READONLY).0)
                .and_then(|()| status(58, protect(half, PAGE * 2, PAGE_READWRITE).0))
                .and_then(|()| {
                    let run = if chunk == CHUNKS - 1 { CHUNK } else { 2 * CHUNK };
                    region(59, c, c, run, MEM_COMMIT, PAGE_READWRITE)
                })
                .and_then(|()| {
                    let ok = (c..c + CHUNK)
                        .step_by(PAGE)
                        .all(|p| word(p + 8 * (round % 64)) == marker(p));
                    check(60, ok, round as u64)
                })
                .and_then(|()| status(61, free(c, CHUNK, MEM_DECOMMIT).0));
            if result.is_err() {
                break 'rounds;
            }
        }
        if LOST.load(Ordering::Relaxed) != 0 {
            break;
        }
    }
    let target = PASSES.load(Ordering::Acquire) + 2;
    while result.is_ok() && PASSES.load(Ordering::Acquire) < target {
        yield_execution();
    }
    STOP.store(true, Ordering::Release);
    status(62, wait(thread, Some(-100_000_000)))?;
    close(thread);
    result?;
    let lost = LOST.load(Ordering::Relaxed);
    check(63, lost == 0, lost)?;
    status(64, free(base, 0, MEM_RELEASE).0)
}

fn main() -> u32 {
    match single().and_then(|()| concurrent()) {
        Ok(()) => {
            print("vmcycle_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
