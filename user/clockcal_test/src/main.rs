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
//! emulator, never run ahead).
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

/// KUSER_SHARED_DATA.InterruptTime (a KSYSTEM_TIME) in milliseconds.
fn interrupt_ms() -> i64 {
    loop {
        let high1 = u32_at(0x0c);
        let low = u32_at(0x08);
        let high2 = u32_at(0x10);
        if high1 == high2 {
            return ((u64::from(high1) << 32 | u64::from(low)) / 10_000) as i64;
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

fn mark(n: u64) -> (i64, i64, i64) {
    let t = (now_ms(), interrupt_ms(), tick_ms());
    print("clockcal_test: mark ");
    print_dec(n);
    print(" uptime_ms=");
    print_dec(t.0.max(0) as u64);
    print("\r\n");
    t
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
    let (q0, i0, t0) = mark(1);
    delay(-SLEEP_MS * 10_000);
    let (q1, i1, t1) = mark(2);
    let (q, i, t) = (q1 - q0, i1 - i0, t1 - t0);
    // Not early (a millisecond's rounding allowed).
    if q < SLEEP_MS - 1 {
        return fail(1, "slept short on the performance counter:", q);
    }
    // The interrupt time is the performance counter's clock.
    if (i - q).abs() > 20 {
        return fail(2, "interrupt time moved", i);
    }
    // Ticks may lag (late interrupts under emulation), never lead.
    if t > q + 20 || t < q * 8 / 10 {
        return fail(3, "tick count moved", t);
    }
    print("clockcal_test: slept ");
    print_dec(q as u64);
    print(" ms\r\n");
    print("clockcal_test: ok\r\n");
    0
}

entry!(main);
