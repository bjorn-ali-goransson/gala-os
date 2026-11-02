//! Kernel handover with idle programs (ROADMAP R9 milestone (b),
//! docs/ARCHITECTURE.md "Kernel handover"): a program whose threads are
//! all blocked in waits goes on in the new kernel as if nothing happened,
//! but for the time it took.
//!
//! The program writes patterns into private memory and into a view of a
//! pagefile-backed section, opens handles (events, a timer, its own
//! executable file at a position, its own thread and process), starts
//! three threads that block (on the timer, on an event the timer's thread
//! sets, in an alertable wait on another event), notes its ids and the
//! time, asks for a handover (gala's test-only GalaRequestHandover) and
//! sleeps. The kernel hands over once every thread is blocked. After the
//! sleep it checks:
//! - the sleep ended at its deadline: it took the time asked, not that
//!   plus the handover (the call is made again with its absolute
//!   deadline, ARCHITECTURE invariant 2), and the clocks went forward;
//! - the memory patterns are intact (the user pages stayed in their
//!   frames), in the main thread and in a thread woken in the new kernel;
//! - the handles work: the threads woke and exited with their codes
//!   (NtQueryInformationThread's exit status, as GetExitCodeThread), the
//!   events can be set and waited on, the file reads on from where it was;
//! - the process and thread ids, as the kernel reports them for handles
//!   and as the TEB holds them, are the same.
//!
//! With the argument `twice`, it asks for a second handover after the
//! first and checks again: a process that came across goes on across the
//! next one too, with handles to the threads that have exited by then.
//!
//! With the argument `refuse`, the program holds what a handover cannot
//! carry yet (a handle to the NUL device): the kernel refuses the
//! handover, says why, and the program goes on undisturbed in the same
//! kernel (`ok (refused)`).
//!
//! GalaRequestHandover is gala's own (number 0x7f5a), as are the timer
//! services' numbers here: on Windows the request fails and the test
//! reports itself skipped (exit 0).
//!
//! Prints `handover_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use ntrt::*;

entry!(main);

/// How long the main thread sleeps across the handover (ms).
const SLEEP_MS: i64 = 4000;
/// When the timer fires (ms after it is set): during the sleep, after the
/// handover (which takes well under a second).
const TIMER_MS: i64 = 2500;
/// How much later than asked the sleep may end, in ticks of the tick count
/// (milliseconds) that waits' deadlines are kept in: a tick or two, a busy
/// machine. A sleep started again after the handover would end later by
/// the handover's time (a few hundred ms).
const SLACK_TICKS: u64 = 100;

const PAGE: usize = 4096;
const FILE_READ_DATA: u32 = 1;
const PRIVATE_PAGES: usize = 3;
const SECTION_PAGES: usize = 2;

static PRIVATE: AtomicUsize = AtomicUsize::new(0);
static GO: AtomicUsize = AtomicUsize::new(0);
static ALERT: AtomicUsize = AtomicUsize::new(0);
static TIMER: AtomicUsize = AtomicUsize::new(0);
/// Each worker's thread id as its TEB had it before and after its wait.
static IDS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

fn private_byte(i: usize) -> u8 {
    (i.wrapping_mul(7) ^ (i >> 9)).wrapping_add(3) as u8
}

fn section_byte(i: usize) -> u8 {
    (i.wrapping_mul(13) as u8) ^ 0xa5
}

fn fill(base: usize, len: usize, f: fn(usize) -> u8) {
    for i in 0..len {
        // SAFETY: committed read-write memory of this process.
        unsafe { ((base + i) as *mut u8).write_volatile(f(i)) };
    }
}

fn intact(base: usize, len: usize, f: fn(usize) -> u8) -> bool {
    // SAFETY: as in `fill`.
    (0..len).all(|i| unsafe { ((base + i) as *const u8).read_volatile() } == f(i))
}

/// KUSER_SHARED_DATA.TickCount (ms; GetTickCount64): the clock of the
/// waits' deadlines.
fn tick_count() -> u64 {
    loop {
        // SAFETY: KUSER_SHARED_DATA is always mapped readable in user mode.
        let (high1, low, high2) = unsafe {
            (
                (0x7ffe_0324 as *const u32).read_volatile(),
                (0x7ffe_0320 as *const u32).read_volatile(),
                (0x7ffe_0328 as *const u32).read_volatile(),
            )
        };
        if high1 == high2 {
            // SAFETY: as above.
            let multiplier = unsafe { (0x7ffe_0004 as *const u32).read_volatile() };
            let ticks = u64::from(high1) << 32 | u64::from(low);
            return ((u128::from(ticks) * u128::from(multiplier)) >> 24) as u64;
        }
    }
}

/// ClientId.UniqueThread from the TEB.
fn my_tid() -> u64 {
    teb_field(0x48) as u64
}

/// Waits for the timer, then sets GO (exit code 0x11).
extern "C" fn timer_thread(_: usize) -> ! {
    IDS[0].store(my_tid(), Ordering::SeqCst);
    let st = wait(Handle(TIMER.load(Ordering::SeqCst)), Some(relative_ms(60_000)));
    IDS[1].store(my_tid(), Ordering::SeqCst);
    if st != STATUS_SUCCESS {
        exit_thread(0x100 | st & 0xff);
    }
    set_event(Handle(GO.load(Ordering::SeqCst)));
    exit_thread(0x11)
}

/// Waits for GO, then checks the private memory (exit code 0x22).
extern "C" fn go_thread(_: usize) -> ! {
    IDS[2].store(my_tid(), Ordering::SeqCst);
    let st = wait(Handle(GO.load(Ordering::SeqCst)), Some(relative_ms(60_000)));
    IDS[3].store(my_tid(), Ordering::SeqCst);
    if st != STATUS_SUCCESS {
        exit_thread(0x200 | st & 0xff);
    }
    if !intact(PRIVATE.load(Ordering::SeqCst), PRIVATE_PAGES * PAGE, private_byte) {
        exit_thread(0x2ff);
    }
    exit_thread(0x22)
}

/// An alertable wait on ALERT, which the main thread sets after the
/// handover (exit code 0x33).
extern "C" fn alert_thread(_: usize) -> ! {
    IDS[4].store(my_tid(), Ordering::SeqCst);
    let st = wait_alertable(Handle(ALERT.load(Ordering::SeqCst)), Some(relative_ms(60_000)));
    IDS[5].store(my_tid(), Ordering::SeqCst);
    if st != STATUS_SUCCESS {
        exit_thread(0x300 | st & 0xff);
    }
    exit_thread(0x33)
}

fn say(text: &str) {
    print(text);
}

/// "handover_test: <what> <n> ms\r\n" in one write.
fn say_ms(what: &str, ms: i64) {
    let mut line = [0u8; 96];
    let mut n = 0;
    for &b in b"handover_test: ".iter().chain(what.as_bytes()).chain(b" ") {
        line[n] = b;
        n += 1;
    }
    let mut digits = [0u8; 20];
    let mut d = 0;
    let mut v = ms.unsigned_abs();
    loop {
        digits[d] = b'0' + (v % 10) as u8;
        d += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    if ms < 0 {
        line[n] = b'-';
        n += 1;
    }
    for i in (0..d).rev() {
        line[n] = digits[i];
        n += 1;
    }
    for &b in b" ms\r\n" {
        line[n] = b;
        n += 1;
    }
    write_file(stdout(), &line[..n]);
}

/// Whether the command line's last word is `word`.
fn last_arg_is(word: &str) -> bool {
    // SAFETY: PEB+0x20 is ProcessParameters; its CommandLine is a
    // UNICODE_STRING at +0x70.
    let cmd = unsafe {
        let pp = *(peb().add(0x20) as *const *const u8);
        let len = usize::from(*(pp.add(0x70) as *const u16)) / 2;
        core::slice::from_raw_parts(*(pp.add(0x78) as *const *const u16), len)
    };
    let word = word.as_bytes();
    cmd.len() > word.len()
        && cmd[cmd.len() - word.len() - 1] == u16::from(b' ')
        && cmd[cmd.len() - word.len()..].iter().zip(word).all(|(&a, &b)| a == u16::from(b))
}

/// `refuse`: a handover asked for while holding a device handle is
/// refused; the program goes on.
fn refused() -> Result<&'static str, u32> {
    let path = wide!("\\??\\NUL");
    let name = UnicodeString::new(&path);
    let attrs = ObjectAttributes::new(&name, Handle(0), OBJ_CASE_INSENSITIVE);
    let options = FILE_SYNCHRONOUS_IO_NONALERT | FILE_NON_DIRECTORY_FILE;
    let (st, null, _) =
        open_file(&attrs, GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, options);
    check(st == STATUS_SUCCESS, 101)?;
    let mut buf = [0u8; 16];
    let pid = teb_field(0x40);
    let before = tick_count();
    match request_handover() {
        STATUS_SUCCESS => {}
        STATUS_ACCESS_DENIED => return Ok("ok (no handover allowed)"),
        STATUS_INVALID_SYSTEM_SERVICE => return Ok("skipped (no GalaRequestHandover: not gala)"),
        _ => return Err(102),
    }
    check(delay(relative_ms(1000)) == STATUS_SUCCESS, 103)?;
    check(tick_count() >= before + 1000, 104)?;
    check(write_file(null, &buf) == STATUS_SUCCESS, 105)?;
    buf[0] = 1;
    check(teb_field(0x40) == pid && buf[0] == 1, 106)?;
    check(close(null) == STATUS_SUCCESS, 107)?;
    Ok("ok (refused)")
}

fn main() -> u32 {
    let result = if last_arg_is("refuse") { refused() } else { run().map(|()| "ok") };
    match result {
        Ok(what) => {
            let mut line = [0u8; 80];
            let mut n = 0;
            for &b in b"handover_test: ".iter().chain(what.as_bytes()).chain(b"\r\n") {
                line[n] = b;
                n += 1;
            }
            write_file(stdout(), &line[..n]);
            0
        }
        Err(n) => {
            let mut line = *b"handover_test: failed check 00000000\r\n";
            line[28..36].copy_from_slice(&hex32(n));
            write_file(stdout(), &line);
            n
        }
    }
}

fn check(ok: bool, n: u32) -> Result<(), u32> {
    if ok { Ok(()) } else { Err(n) }
}

fn run() -> Result<(), u32> {
    // Private memory, partly written (the rest stays demand-zero).
    let (st, private, _) = allocate(0, 16 * PAGE, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    check(st == STATUS_SUCCESS, 1)?;
    fill(private, PRIVATE_PAGES * PAGE, private_byte);
    PRIVATE.store(private, Ordering::SeqCst);
    // A pagefile-backed section's view.
    let (st, section) = create_section(None, Some(16 * PAGE as i64), PAGE_READWRITE);
    check(st == STATUS_SUCCESS, 2)?;
    let (st, view, _) = map_view(section, 0, 0, PAGE_READWRITE);
    check(st == STATUS_SUCCESS, 3)?;
    fill(view, SECTION_PAGES * PAGE, section_byte);
    // Events and a timer.
    let (st, go) = create_event(EVENT_ALL_ACCESS, NOTIFICATION_EVENT, false);
    check(st == STATUS_SUCCESS, 4)?;
    let (st, alert) = create_event(EVENT_ALL_ACCESS, SYNCHRONIZATION_EVENT, false);
    check(st == STATUS_SUCCESS, 5)?;
    let (st, timer) = create_timer(0);
    check(st == STATUS_SUCCESS, 6)?;
    GO.store(go.0, Ordering::SeqCst);
    ALERT.store(alert.0, Ordering::SeqCst);
    TIMER.store(timer.0, Ordering::SeqCst);
    // Its own executable, read up to offset 2.
    let path = wide!("\\??\\C:\\gala\\handover_test.exe");
    let name = UnicodeString::new(&path);
    let attrs = ObjectAttributes::new(&name, Handle(0), OBJ_CASE_INSENSITIVE);
    let options = FILE_SYNCHRONOUS_IO_NONALERT | FILE_NON_DIRECTORY_FILE;
    let (st, file, _) = open_file(&attrs, FILE_READ_DATA, FILE_SHARE_READ, options);
    check(st == STATUS_SUCCESS, 7)?;
    let mut mz = [0u8; 2];
    check(read_file(file, &mut mz, None) == (STATUS_SUCCESS, 2) && mz == *b"MZ", 8)?;
    // Its own thread and process, as real handles.
    let (st, me_thread) = duplicate(CURRENT_THREAD);
    check(st == STATUS_SUCCESS, 9)?;
    let (st, me_process) = duplicate(CURRENT_PROCESS);
    check(st == STATUS_SUCCESS, 10)?;
    let (st, _, _, pid) = process_basic_info(me_process);
    check(st == STATUS_SUCCESS && pid as u64 == teb_field(0x40) as u64, 11)?;
    let tid = my_tid();
    check(thread_basic_info(me_thread).3 as u64 == tid, 12)?;

    // The workers block: on the timer, on GO, alertably on ALERT.
    check(set_timer(timer, relative_ms(TIMER_MS), 0) == STATUS_SUCCESS, 13)?;
    let (st, timer_waiter, timer_tid) = create_thread(timer_thread, 0, false);
    check(st == STATUS_SUCCESS, 14)?;
    let (st, go_waiter, go_tid) = create_thread(go_thread, 0, false);
    check(st == STATUS_SUCCESS, 15)?;
    let (st, alert_waiter, alert_tid) = create_thread(alert_thread, 0, false);
    check(st == STATUS_SUCCESS, 16)?;
    // Until each has started (and is about to block).
    for (i, id) in [(0, timer_tid), (2, go_tid), (4, alert_tid)] {
        while IDS[i].load(Ordering::SeqCst) == 0 {
            yield_execution();
        }
        check(IDS[i].load(Ordering::SeqCst) == id as u64, 17)?;
    }

    // The handover, while everyone sleeps.
    let (before_qpc, frequency) = performance_counter();
    let (_, before_system) = system_time();
    let before_ticks = tick_count();
    say("handover_test: ready, asking for a handover\r\n");
    match request_handover() {
        STATUS_SUCCESS => {}
        STATUS_INVALID_SYSTEM_SERVICE => {
            say("handover_test: skipped (no GalaRequestHandover: not gala)\r\n");
            exit(0);
        }
        _ => return Err(18),
    }
    check(delay(relative_ms(SLEEP_MS)) == STATUS_SUCCESS, 19)?;
    let after_ticks = tick_count();
    let (after_qpc, _) = performance_counter();
    let (_, after_system) = system_time();
    let slept = after_ticks.wrapping_sub(before_ticks) as i64;
    say_ms("slept", slept);
    say_ms("slept (performance counter)", (after_qpc - before_qpc) * 1000 / frequency.max(1));

    // Time: forward, and the sleep kept its deadline.
    check(after_qpc > before_qpc && after_system > before_system, 20)?;
    check(slept >= SLEEP_MS, 21)?;
    check(slept < SLEEP_MS + SLACK_TICKS as i64, 22)?;
    // Memory.
    check(intact(private, PRIVATE_PAGES * PAGE, private_byte), 23)?;
    check(intact(view, SECTION_PAGES * PAGE, section_byte), 24)?;
    // SAFETY: committed memory of this process, never written: zero.
    let untouched = unsafe { ((private + 10 * PAGE) as *const u64).read_volatile() };
    check(untouched == 0, 25)?;
    // The threads: the timer fired, GO was set, the private memory was
    // intact for a thread woken in the new kernel.
    check(wait(timer_waiter, Some(relative_ms(10_000))) == STATUS_SUCCESS, 26)?;
    check(wait(go_waiter, Some(relative_ms(10_000))) == STATUS_SUCCESS, 27)?;
    check(thread_basic_info(timer_waiter).1 == 0x11, 28)?;
    check(thread_basic_info(go_waiter).1 == 0x22, 29)?;
    // The alertable waiter is still waiting; set its event.
    check(wait(alert_waiter, Some(0)) == 0x102, 30)?; // STATUS_TIMEOUT
    check(set_event(alert).0 == STATUS_SUCCESS, 31)?;
    check(wait(alert_waiter, Some(relative_ms(10_000))) == STATUS_SUCCESS, 32)?;
    check(thread_basic_info(alert_waiter).1 == 0x33, 33)?;
    for (before, after) in [(0, 1), (2, 3), (4, 5)] {
        check(IDS[before].load(Ordering::SeqCst) == IDS[after].load(Ordering::SeqCst), 34)?;
    }
    check(thread_basic_info(alert_waiter).3 == alert_tid, 35)?;
    // The events work.
    check(query_event(go) == (STATUS_SUCCESS, NOTIFICATION_EVENT, 1), 36)?;
    check(clear_event(go) == STATUS_SUCCESS, 37)?;
    check(wait(go, Some(0)) == 0x102, 38)?;
    check(set_event(go).0 == STATUS_SUCCESS && wait(go, Some(0)) == STATUS_SUCCESS, 39)?;
    // The file reads on from offset 2.
    let mut next = [0u8; 2];
    check(read_file(file, &mut next, None) == (STATUS_SUCCESS, 2), 40)?;
    let mut at2 = [0u8; 2];
    check(read_file(file, &mut at2, Some(2)) == (STATUS_SUCCESS, 2) && next == at2, 41)?;
    check(next != *b"MZ", 42)?;
    // Ids.
    check(my_tid() == tid, 43)?;
    check(thread_basic_info(me_thread).3 as u64 == tid, 44)?;
    let (st, exit_status, _, pid_after) = process_basic_info(me_process);
    check(st == STATUS_SUCCESS && pid_after == pid && exit_status == 0x103, 45)?;
    // The section still maps (a second view sees the same pages).
    let (st, view2, _) = map_view(section, 0, 0, PAGE_READWRITE);
    check(st == STATUS_SUCCESS && intact(view2, SECTION_PAGES * PAGE, section_byte), 46)?;
    if last_arg_is("twice") {
        // Everyone but this thread has exited; their objects stay (open
        // handles), and the next kernel must have them too.
        check(request_handover() == STATUS_SUCCESS, 48)?;
        let before = tick_count();
        check(delay(relative_ms(1000)) == STATUS_SUCCESS, 49)?;
        let slept = tick_count().wrapping_sub(before) as i64;
        say_ms("slept again", slept);
        check((1000..1000 + SLACK_TICKS as i64).contains(&slept), 50)?;
        check(intact(private, PRIVATE_PAGES * PAGE, private_byte), 51)?;
        check(intact(view, SECTION_PAGES * PAGE, section_byte), 52)?;
        check(intact(view2, SECTION_PAGES * PAGE, section_byte), 53)?;
        check(thread_basic_info(timer_waiter).1 == 0x11, 54)?;
        check(thread_basic_info(go_waiter).1 == 0x22, 55)?;
        check(thread_basic_info(alert_waiter).3 == alert_tid, 56)?;
        check(wait(alert_waiter, Some(0)) == STATUS_SUCCESS, 57)?;
        let mut more = [0u8; 2];
        check(read_file(file, &mut more, None) == (STATUS_SUCCESS, 2), 58)?;
        let mut at4 = [0u8; 2];
        check(read_file(file, &mut at4, Some(4)) == (STATUS_SUCCESS, 2) && more == at4, 59)?;
        check(my_tid() == tid && process_basic_info(me_process).3 == pid, 60)?;
    }
    for h in [go, alert, timer, file, me_thread, me_process, section] {
        check(close(h) == STATUS_SUCCESS, 47)?;
    }
    Ok(())
}
