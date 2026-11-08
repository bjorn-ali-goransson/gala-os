//! Threads in a process: creation from a CONTEXT, per-thread TEBs,
//! suspension, exit codes, waiting on thread handles, termination of
//! another thread, and concurrent updates. Results are expected to match
//! real Windows. Prints `thread_test: ok` or exits with the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicUsize, Ordering};

use ntrt::*;

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
        print("thread_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        check_line(core::panic::Location::caller().line());
        print("\r\n");
        Err(self.0)
    }
}

static RAN: AtomicUsize = AtomicUsize::new(0);
static SEEN_TEB: AtomicUsize = AtomicUsize::new(0);
static SEEN_TID: AtomicUsize = AtomicUsize::new(0);
static SEEN_ARG: AtomicUsize = AtomicUsize::new(0);
static COUNTER: AtomicUsize = AtomicUsize::new(0);
static BLOCKER: AtomicUsize = AtomicUsize::new(0);

extern "C" fn simple(arg: usize) -> ! {
    SEEN_ARG.store(arg, Ordering::SeqCst);
    SEEN_TEB.store(teb_field(0x30), Ordering::SeqCst);
    SEEN_TID.store(teb_field(0x48), Ordering::SeqCst);
    RAN.fetch_add(1, Ordering::SeqCst);
    exit_thread(0x1234)
}

extern "C" fn adder(n: usize) -> ! {
    for _ in 0..1000 {
        COUNTER.fetch_add(n, Ordering::SeqCst);
        delay(0); // yield
    }
    exit_thread(n as u32)
}

extern "C" fn blocked(_: usize) -> ! {
    // Waits forever on an event nobody sets.
    wait(Handle(BLOCKER.load(Ordering::SeqCst)), None);
    exit_thread(0xbad)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let infinite = None;

    // Process information while running.
    let (st, exit, peb_addr, pid) = process_basic_info(CURRENT_PROCESS);
    c.eq(st.into(), STATUS_SUCCESS.into())?;
    c.eq(exit.into(), STATUS_PENDING.into())?;
    c.eq(peb_addr as u64, peb() as u64)?;
    c.eq(pid as u64, teb_field(0x40) as u64)?;

    // A thread runs with its own TEB and argument, and reports an exit code.
    let (st, h, tid) = create_thread(simple, 0x5eed, false);
    c.eq(st.into(), STATUS_SUCCESS.into())?;
    c.ok(tid != 0 && tid != teb_field(0x48), tid as u64)?;
    c.eq(wait(h, infinite).into(), STATUS_SUCCESS.into())?;
    c.eq(RAN.load(Ordering::SeqCst) as u64, 1)?;
    c.eq(SEEN_ARG.load(Ordering::SeqCst) as u64, 0x5eed)?;
    c.eq(SEEN_TID.load(Ordering::SeqCst) as u64, tid as u64)?;
    let teb = SEEN_TEB.load(Ordering::SeqCst);
    c.ok(teb != 0 && teb != teb_field(0x30), teb as u64)?;
    let (st, exit, info_teb, info_tid) = thread_basic_info(h);
    c.eq(st.into(), STATUS_SUCCESS.into())?;
    c.eq(exit.into(), 0x1234)?;
    // The TEB is freed when the thread ends: NULL (Windows 11 25H2).
    c.eq(info_teb as u64, 0)?;
    c.eq(info_tid as u64, tid as u64)?;
    close(h);

    // Created suspended: does not run until resumed.
    let (st, h, _) = create_thread(simple, 2, true);
    c.eq(st.into(), STATUS_SUCCESS.into())?;
    delay(relative_ms(20));
    c.eq(RAN.load(Ordering::SeqCst) as u64, 1)?;
    let (st, exit, info_teb, _) = thread_basic_info(h);
    c.eq(st.into(), STATUS_SUCCESS.into())?;
    c.eq(exit.into(), STATUS_PENDING.into())?;
    c.ok(info_teb != 0 && info_teb != teb_field(0x30), info_teb as u64)?; // its own TEB
    c.eq(wait(h, Some(0)).into(), STATUS_TIMEOUT.into())?;
    let (st, previous) = resume_thread(h);
    c.eq(st.into(), STATUS_SUCCESS.into())?;
    c.eq(previous.into(), 1)?;
    c.eq(wait(h, infinite).into(), STATUS_SUCCESS.into())?;
    c.eq(RAN.load(Ordering::SeqCst) as u64, 2)?;
    close(h);

    // Several threads update shared memory concurrently.
    let mut handles = [Handle(0); 4];
    for (i, slot) in handles.iter_mut().enumerate() {
        let (st, h, _) = create_thread(adder, i + 1, false);
        c.eq(st.into(), STATUS_SUCCESS.into())?;
        *slot = h;
    }
    c.eq(wait_multiple(&handles, WAIT_ALL, infinite).into(), STATUS_SUCCESS.into())?;
    c.eq(COUNTER.load(Ordering::SeqCst) as u64, 1000 * (1 + 2 + 3 + 4))?;
    for (i, h) in handles.iter().enumerate() {
        c.eq(thread_basic_info(*h).1.into(), (i + 1) as u64)?;
        close(*h);
    }

    // Terminating a thread that is blocked in a wait.
    let (_, ev) = create_event(EVENT_ALL_ACCESS, NOTIFICATION_EVENT, false);
    BLOCKER.store(ev.0, Ordering::SeqCst);
    let (st, h, _) = create_thread(blocked, 0, false);
    c.eq(st.into(), STATUS_SUCCESS.into())?;
    delay(relative_ms(10));
    c.eq(wait(h, Some(0)).into(), STATUS_TIMEOUT.into())?;
    c.eq(terminate_thread(h, 0x55).into(), STATUS_SUCCESS.into())?;
    c.eq(wait(h, infinite).into(), STATUS_SUCCESS.into())?;
    c.eq(thread_basic_info(h).1.into(), 0x55)?;
    close(h);
    close(ev);
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("thread_test: ok\r\n");
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
