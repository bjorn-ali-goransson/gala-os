//! Copy-on-write under threads on two processors at once, as on real
//! Windows: two threads pinned to processors 0 and 1 write, in step, their
//! own word of every page of a fresh PAGE_WRITECOPY view of a file, so
//! that they often take the first write fault on the same page at the
//! same time. Afterwards every word written must be there (no write lost
//! to the other thread's copy) and the file, read back, must be unchanged
//! (no write reached the shared page). Repeated for many views.
//!
//! A write fault on a copy-on-write page made the page private in the VAD
//! before mapping the copy; a second thread faulting in between found
//! "read/write, mapped" and made the shared page writable (gala before
//! 2026-11-02: 4 of 8 boots failed check 12).
//!
//! Needs NumberOfProcessors >= 2 (else only one thread writes).
//! Prints `smpcow_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use ntrt::*;

const THREAD_AFFINITY_MASK: u32 = 4;
const PAGE_WRITECOPY: u32 = 0x08;
const FILE_DISPOSITION_INFORMATION: u32 = 13;
const SYNC: u32 = FILE_SYNCHRONOUS_IO_NONALERT | FILE_NON_DIRECTORY_FILE;
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const RW: u32 = GENERIC_READ | GENERIC_WRITE | DELETE;
const PAGE: usize = 0x1000;
const PAGES: usize = 128;
const ROUNDS: u32 = 40;
/// Writes per thread per page: a write lost while the other thread's
/// fault is resolved shows in the last one too.
const WRITES: u64 = 4;

/// Check `id` (also the exit status when it fails).
fn check(id: u32, cond: bool, got: u64) -> Result<(), u32> {
    if cond {
        return Ok(());
    }
    print("smpcow_test: check ");
    write_file(stdout(), &hex32(id));
    print(" got ");
    write_file(stdout(), &hex32(got as u32));
    print("\r\n");
    Err(id)
}

fn status(id: u32, s: NtStatus) -> Result<(), u32> {
    check(id, s == STATUS_SUCCESS, s.into())
}

fn pattern(i: usize) -> u8 {
    (i % 251) as u8
}

/// The word thread `t` leaves in page `page` in round `round`.
fn marker(t: usize, round: u32, page: usize) -> u64 {
    0xc0de_0000_0000_0000 | (t as u64) << 40 | u64::from(round) << 16 | page as u64
}

fn pin(cpu: usize) -> NtStatus {
    set_thread(CURRENT_THREAD, THREAD_AFFINITY_MASK, &(1u64 << cpu).to_le_bytes())
}

/// The current view's base, the round the writers are to do (0: none
/// yet), the page each writer is at (to keep them in step) and how many
/// rounds each has finished.
static VIEW: AtomicUsize = AtomicUsize::new(0);
static ROUND: AtomicU32 = AtomicU32::new(0);
static AT: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
static DONE: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
static WRITERS: AtomicUsize = AtomicUsize::new(2);

/// Writer `t`: pinned to processor `t`, writes its word of every page of
/// each round's view, never more than one page ahead of the other one.
extern "C" fn writer(t: usize) -> ! {
    if pin(t) != STATUS_SUCCESS {
        exit_thread(1);
    }
    let other = 1 - t;
    let two = WRITERS.load(Ordering::Relaxed) == 2;
    for round in 1..=ROUNDS {
        while ROUND.load(Ordering::Acquire) < round {
            core::hint::spin_loop();
        }
        let view = VIEW.load(Ordering::Acquire);
        for page in 0..PAGES {
            while two
                && DONE[other].load(Ordering::Acquire) < round
                && AT[other].load(Ordering::Acquire) + 1 < page
            {
                core::hint::spin_loop();
            }
            AT[t].store(page, Ordering::Release);
            let word = (view + page * PAGE + 8 * t) as *mut u64;
            for _ in 0..WRITES {
                // SAFETY: inside the copy-on-write view, which stays mapped
                // until both writers have finished the round.
                unsafe { word.write_volatile(marker(t, round, page)) };
            }
        }
        AT[t].store(0, Ordering::Release);
        DONE[t].store(round, Ordering::Release);
    }
    exit_thread(STATUS_SUCCESS)
}

/// A fresh file at `attrs` holding `data`, opened read/write.
fn new_file(attrs: &ObjectAttributes, data: &[u8]) -> (NtStatus, Handle) {
    let (s, old, _) = open_file(attrs, DELETE, SHARE_ALL, FILE_NON_DIRECTORY_FILE);
    if s == STATUS_SUCCESS {
        set_file(old, FILE_DISPOSITION_INFORMATION, &[1]);
        close(old);
    }
    let (s, h, _) = create_file(attrs, RW, SHARE_ALL, FILE_CREATE, SYNC);
    if s != STATUS_SUCCESS {
        return (s, h);
    }
    (write_file_info(h, data).0, h)
}

static mut DATA: [u8; PAGES * PAGE] = [0; PAGES * PAGE];

fn run() -> Result<(), u32> {
    let mut b = [0u8; 64];
    status(1, query_system(0, &mut b).0)?;
    let cpus = usize::from(b[56]);
    let writers = cpus.clamp(1, 2);
    WRITERS.store(writers, Ordering::Relaxed);

    // SAFETY: only this thread touches DATA.
    let data = unsafe { &mut *core::ptr::addr_of_mut!(DATA) };
    for (i, b) in data.iter_mut().enumerate() {
        *b = pattern(i);
    }
    let path = wide!("\\??\\C:\\Users\\Public\\gala_smpcow_test.dat");
    let path = UnicodeString::new(&path);
    let attrs = ObjectAttributes::new(&path, Handle(0), OBJ_CASE_INSENSITIVE);
    let (s, file) = new_file(&attrs, data);
    status(2, s)?;
    let (s, section) = create_file_section(file, None, PAGE_READWRITE);
    status(3, s)?;

    let mut threads = [Handle(0); 2];
    for (t, h) in threads.iter_mut().enumerate().take(writers) {
        let (s, thread, _) = create_thread(writer, t, false);
        status(4, s)?;
        *h = thread;
    }
    for round in 1..=ROUNDS {
        let (s, view, _) = map_view(section, 0, 0, PAGE_WRITECOPY);
        status(5, s)?;
        VIEW.store(view, Ordering::Release);
        ROUND.store(round, Ordering::Release);
        let (start, freq) = performance_counter();
        while DONE[..writers].iter().any(|d| d.load(Ordering::Acquire) < round)
            && performance_counter().0 - start < 30 * freq
        {
            yield_execution();
        }
        check(6, DONE[..writers].iter().all(|d| d.load(Ordering::Acquire) >= round), round.into())?;
        // Every write is in the view, the rest of each page is the file's.
        for page in 0..PAGES {
            let base = view + page * PAGE;
            for t in 0..writers {
                // SAFETY: inside the mapped view.
                let got = unsafe { ((base + 8 * t) as *const u64).read_volatile() };
                // A lost write: the page number and the thread.
                check(7, got == marker(t, round, page), (page << 4 | t) as u64)?;
            }
            // SAFETY: inside the mapped view.
            let tail = unsafe { ((base + PAGE - 1) as *const u8).read_volatile() };
            check(8, tail == pattern(page * PAGE + PAGE - 1), tail.into())?;
        }
        status(9, unmap_view(view))?;
        // The file never saw the writes.
        data.fill(0);
        let (s, n) = read_file(file, data, Some(0));
        status(10, s)?;
        check(11, n == PAGES * PAGE, n as u64)?;
        // A write that reached the file's page: its offset.
        let bad = data.iter().enumerate().position(|(i, &b)| b != pattern(i));
        check(12, bad.is_none(), bad.unwrap_or(0) as u64)?;
    }
    for &h in &threads[..writers] {
        status(13, wait(h, Some(-100_000_000)))?;
        let (_, code, _, _) = thread_basic_info(h);
        status(14, code)?;
        close(h);
    }
    close(section);
    status(15, set_file(file, FILE_DISPOSITION_INFORMATION, &[1]))?;
    close(file);
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("smpcow_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
