//! What the import trace costs a call, and the repeat and quote
//! budgets at a scale a three-call program cannot reach
//! (docs/OBSERVABILITY.md, "Import tracing").
//!
//! Every call through an armed slot is a page fault, a slot written
//! twice, an event formatted and a read of what the arguments point
//! at. Each of those is cheap and all of them together are what makes
//! a traced run of a real program ten times its own length, and a
//! number nobody has measured is a number nobody can argue with. So
//! this program is one import called [`CALLS`] times and nothing else:
//! the same work in every scenario, so that what the trace adds is the
//! difference between their in-kernel seconds (WORKFLOW "What the
//! import trace costs").
//!
//! It is a test and not only a probe. Each call hands the import two
//! strings and two numbers, so the three budgets are counted at scale
//! and their arithmetic is in the expectations: one slot armed, the
//! first call recorded free, [`CALLS`] minus one later calls, two
//! quotes a call. The program's own check is that none of it changed
//! the call: the sum of [`CALLS`] answers is the sum of what they were
//! handed, armed or not, quoted or not.
//!
//! Prints `import_cost_test: ok`, or exits with the number of the
//! failing check.

#![no_std]
#![no_main]

use ntrt::*;

#[link(name = "testdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn gala_four(a: u64, b: u64, c: u64, d: u64) -> u64;
}

/// Calls through the one armed slot. Enough that the per-call cost is
/// what the scenario's seconds are made of rather than its boot, and
/// few enough that the slowest configuration of the trace (every call
/// recorded, both arguments quoted) stays a test and not a wait: its
/// seconds are this times what an event costs to leave the guest,
/// which is the host's figure and not gala's (3 ms on a laptop, about
/// twice that on a CI runner), so the count has to leave the scenario
/// far inside its time limit on the slowest host that runs it. 20 000
/// did not: 60 s on a laptop and 98-121 s on CI, against 120 s.
const CALLS: u64 = 5_000;

/// A narrow string, as a `*A` call is handed one, and a wide one, as a
/// `*W` call is: two quotes a call, which is what a call of Word's
/// carries where it carries any.
static NARROW: &[u8; 13] = b"ErrorMessage\0";
static WIDE: [u16; 17] = wide(b"Document1 - Word");

/// Bytes that are no text, so that one argument of the four costs the
/// quoting a read it then throws away, as most of a real call's do.
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
        print("import_cost_test: check ");
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

fn run() -> Result<(), u32> {
    let c = &mut Checks(0);
    let (narrow, wide, blob) = (NARROW.as_ptr() as u64, WIDE.as_ptr() as u64, BLOB.as_ptr() as u64);
    let one = narrow.wrapping_add(wide).wrapping_add(SMALL).wrapping_add(blob);

    // The loop the measurement is: one call through one slot, CALLS
    // times, with nothing else in it. The answers are summed rather
    // than checked one by one so that the loop is the calls and not
    // the checking, and so that a single wrong answer cannot cancel
    // out another's.
    let mut sum = 0u64;
    for _ in 0..CALLS {
        // SAFETY: the DLL's own export, which only adds what it is
        // handed and reads none of the pointers among them. The
        // kernel's own read of them is bounded and may fail.
        sum = sum.wrapping_add(unsafe { gala_four(narrow, wide, SMALL, blob) });
    }
    c.eq(sum, one.wrapping_mul(CALLS))?;

    // And the strings are as they were: the quoting reads them and
    // nothing else, CALLS times over.
    c.eq(u64::from(NARROW[0]), u64::from(b'E'))?;
    c.eq(u64::from(WIDE[16]), 0)?;
    c.eq(u64::from(BLOB[15]), 16)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("import_cost_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);

/// A 64-bit value as two halves, ntrt's own hex being 32 bits wide
/// (the sum of twenty thousand addresses is wider than one).
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
    print(core::str::from_utf8(&b[..8 + len]).unwrap_or(""));
}
