//! The clock against the host's: the guest's time must pass at the
//! host's rate even when the host is busy (docs/ARCHITECTURE.md
//! "Interrupts and time", calibration). The program prints its uptime
//! (NtQueryPerformanceCounter) at two marks five seconds apart; xtask
//! notes when each mark appears on the serial console and compares the
//! guest's interval with the host's (scenario `clockcal_test`, booted with
//! `calstall` under an artificial host load).
//!
//! In the guest it checks what Windows guarantees: a 5 s
//! NtDelayExecution does not end early on the performance counter, and
//! KUSER_SHARED_DATA's interrupt time and tick count follow the
//! performance counter (the tick count may lag a little behind under an
//! emulator, never run ahead). The page is a snapshot written at clock
//! interrupts, so each mark brackets a fresh one between two counter
//! reads (see `mark`) instead of assuming the interrupts come on time.
//!
//! Prints `clockcal_test: ok` or exits with the number of the failing
//! check.

#![no_std]
#![no_main]

use ntrt::*;

const KUSER: usize = 0x7ffe_0000;

fn u32_at(off: usize) -> u32 {
    // SAFETY: KUSER_SHARED_DATA is always mapped readable in user mode.
    unsafe { ((KUSER + off) as *const u32).read_volatile() }
}

fn u64_at(off: usize) -> u64 {
    // SAFETY: as above.
    unsafe { ((KUSER + off) as *const u64).read_volatile() }
}

/// KUSER_SHARED_DATA.InterruptTime (a KSYSTEM_TIME) in 100 ns units.
fn interrupt_raw() -> u64 {
    loop {
        let high1 = u32_at(0x0c);
        let low = u32_at(0x08);
        let high2 = u32_at(0x10);
        if high1 == high2 {
            return u64::from(high1) << 32 | u64::from(low);
        }
    }
}

/// GetTickCount64: TickCount * TickCountMultiplier >> 24.
fn tick_ms() -> i64 {
    ((u128::from(u64_at(0x320)) * u128::from(u32_at(0x04))) >> 24) as i64
}

fn print_dec(mut v: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    write_file(stdout(), &buf[i..]);
}

/// A snapshot of KUSER_SHARED_DATA's clocks and when it was taken on
/// the performance counter: between `lo` and `hi` (ms).
struct Mark {
    lo: i64,
    hi: i64,
    interrupt: i64,
    tick: i64,
}

/// The page is a snapshot that the kernel writes at each clock
/// interrupt, as Windows does. Under an emulator on a busy host an
/// interrupt can come tens of milliseconds late, so the page read at
/// any moment can lag the counter by that much (check 2 once failed
/// with the interrupt time 26 ms behind, 2026-11-04). So a mark waits
/// for the page to change, reading the counter before and after: the
/// new snapshot was taken between those two reads, however late.
fn mark(n: u64) -> Option<Mark> {
    let start = now_ms();
    // The counter read before the last read of the page that still
    // showed the old snapshot: the new one was written after it.
    let mut before = start;
    let first = interrupt_raw();
    let (interrupt, tick) = loop {
        let now = now_ms();
        let raw = interrupt_raw();
        if raw != first {
            break ((raw / 10_000) as i64, tick_ms());
        }
        before = now;
        // A generous give-up: the page did not change for 5 s.
        if now - start > SLEEP_MS {
            return None;
        }
    };
    let hi = now_ms();
    print("clockcal_test: mark ");
    print_dec(n);
    print(" uptime_ms=");
    print_dec(if n == 1 { hi } else { before }.max(0) as u64);
    print("\r\n");
    Some(Mark { lo: before, hi, interrupt, tick })
}

fn fail(check: u32, what: &str, got: i64) -> u32 {
    print("clockcal_test: check ");
    print_dec(u64::from(check));
    print(" failed: ");
    print(what);
    print(" ");
    print_dec(got.max(0) as u64);
    print(" ms\r\n");
    check
}

const SLEEP_MS: i64 = 5000;

fn main() -> u32 {
    let Some(m0) = mark(1) else { return fail(4, "the page did not change in", SLEEP_MS) };
    delay(-SLEEP_MS * 10_000);
    let Some(m1) = mark(2) else { return fail(4, "the page did not change in", SLEEP_MS) };
    // The sleep lies between the first mark's end and the second's start.
    let q = m1.lo - m0.hi;
    // Not early (a millisecond's rounding allowed).
    if q < SLEEP_MS - 1 {
        return fail(1, "slept short on the performance counter:", q);
    }
    // The interrupt time is the performance counter's clock: the two
    // snapshots lay between the marks' brackets (a millisecond's
    // rounding at each end allowed).
    let (shortest, longest) = (m1.lo - m0.hi, m1.hi - m0.lo);
    let i = m1.interrupt - m0.interrupt;
    if i < shortest - 2 || i > longest + 2 {
        return fail(2, "interrupt time moved", i);
    }
    // Ticks may lag (late interrupts under emulation), never lead.
    let t = m1.tick - m0.tick;
    if t > longest + 20 || t < shortest * 8 / 10 {
        return fail(3, "tick count moved", t);
    }
    print("clockcal_test: slept ");
    print_dec(q as u64);
    print(" ms\r\n");
    print("clockcal_test: ok\r\n");
    0
}

entry!(main);
