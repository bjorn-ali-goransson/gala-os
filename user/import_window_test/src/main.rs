//! A trace of a window and not of a whole run: `imports=` with
//! `events_from=` (docs/OBSERVABILITY.md, "Import tracing").
//!
//! The budget of `imports_repeat=` bounds what the trace costs, and
//! what it costs is the record and not the fault: 2 ms against 5 us,
//! measured (`import_cost_test`, OBSERVABILITY "What the stream
//! costs"). So a fault whose record is not written -- the stream not
//! started yet -- takes nothing of the budget and leaves the slot
//! armed, which is what lets a whole budget be spent on a window late
//! in a run. Word's own give-up is at 33.8 s of a startup that calls
//! into MSO from its first second, and the question is what it called
//! in the 931 ms before the box, not what it called at the start.
//!
//! So this program is two batches of calls through one import with a
//! wait between them: a stream that starts in the wait records the
//! second batch whole and nothing of the first. Its own check is that
//! the calls answered the same either way, armed or not, recorded or
//! not; the counting is the kernel's own lines, which the scenario
//! expects -- one first call (the first of the second batch, the
//! first batch having written none), the rest of that batch from the
//! budget, and two quotes for each.
//!
//! Prints `import_window_test: ok`, or exits with the number of the
//! failing check.

#![no_std]
#![no_main]

use ntrt::*;

#[link(name = "testdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn gala_four(a: u64, b: u64, c: u64, d: u64) -> u64;
}

/// Calls in each batch. Enough that a budget of exactly one batch is
/// unambiguous (the first call recorded free, the other 999 from the
/// budget), few enough that the recorded batch is seconds and not
/// minutes at what an event costs.
const CALLS: u64 = 1_000;

/// The wait between the batches, in the 100 ns units NtDelayExecution
/// takes, negative for a relative time: 7 s, with the scenario's
/// `events_from=4` inside it: the first batch is over within a second
/// of boot and the second begins after the seven, so the window has
/// margin on either side of it.
const WAIT: i64 = -70_000_000;

/// A narrow string, as a `*A` call is handed one, and a wide one, as a
/// `*W` call is: two quotes a call.
static NARROW: &[u8; 13] = b"ErrorMessage\0";
static WIDE: [u16; 17] = wide(b"Document1 - Word");

/// Bytes that are no text, so one argument of the four costs the
/// quoting a read it then throws away.
static BLOB: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];

/// A number too small to be a pointer, which the quoting answers
/// without a read at all.
const SMALL: u64 = 7;

/// ASCII as UTF-16, for a static the compiler can build: the length is
/// the string's and one for the terminator.
const fn wide<const N: usize>(s: &[u8]) -> [u16; N] {
    let mut out = [0u16; N];
    let mut i = 0;
    while i < s.len() {
        out[i] = s[i] as u16;
        i += 1;
    }
    out
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.0 += 1;
        if got == want {
            return Ok(());
        }
        print("import_window_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        wide_hex(got);
        print(" want ");
        wide_hex(want);
        check_line(core::panic::Location::caller().line());
        print("\r\n");
        Err(self.0)
    }
}

/// One batch of calls, answering the sum of what they were handed.
fn batch(narrow: u64, wide: u64, blob: u64) -> u64 {
    let mut sum = 0u64;
    for _ in 0..CALLS {
        // SAFETY: the DLL's own export, which only adds what it is
        // handed and reads none of the pointers among them. The
        // kernel's own read of them is bounded and may fail.
        sum = sum.wrapping_add(unsafe { gala_four(narrow, wide, SMALL, blob) });
    }
    sum
}

fn run() -> Result<(), u32> {
    let c = &mut Checks(0);
    let (narrow, wide, blob) = (NARROW.as_ptr() as u64, WIDE.as_ptr() as u64, BLOB.as_ptr() as u64);
    let one = narrow.wrapping_add(wide).wrapping_add(SMALL).wrapping_add(blob);

    // Before the window: these calls fault where the slot is armed,
    // and nothing is written, so they take none of the budget.
    c.eq(batch(narrow, wide, blob), one.wrapping_mul(CALLS))?;

    // The window opens somewhere in here.
    print("import_window_test: waiting\r\n");
    c.eq(delay(WAIT).into(), 0)?;
    print("import_window_test: waited\r\n");

    // Inside it: the same calls, every one of them recorded.
    c.eq(batch(narrow, wide, blob), one.wrapping_mul(CALLS))?;

    // And the strings are as they were: the quoting reads them and
    // nothing else, twice CALLS times over.
    c.eq(u64::from(NARROW[0]), u64::from(b'E'))?;
    c.eq(u64::from(WIDE[16]), 0)?;
    c.eq(u64::from(BLOB[15]), 16)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("import_window_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);

/// A 64-bit value as two halves, ntrt's own hex being 32 bits wide
/// (the sum of a thousand addresses is wider than one).
fn wide_hex(value: u64) {
    write_file(stdout(), &hex32((value >> 32) as u32));
    write_file(stdout(), &hex32(value as u32));
}

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
    write_file(stdout(), &b[..8 + len]);
}
