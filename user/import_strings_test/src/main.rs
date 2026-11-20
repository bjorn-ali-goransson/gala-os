//! `imports_strings=<n>`: the arguments of a recorded call that read
//! as text, quoted beside the registers in the event and said in the
//! log (docs/OBSERVABILITY.md, "Import tracing"). A register is a
//! number; what a call means is mostly in what its numbers point at,
//! and a trace of calls to `WideCharToMultiByte` or `LoadLibraryW`
//! that cannot say which string is a trace of the shape of a program's
//! work and not of the work.
//!
//! This program is the four cases an argument can be, and a fifth
//! that is not a case of an argument at all: a buffer the callee fills,
//! which holds its text only once the call is over, so a trace reads it
//! at the answer or not at all.
//!
//! The four are handed to one import in one call each: a narrow string, a wide string, a number
//! too small to be a pointer, and a pointer at bytes that are no text.
//! A second call hands it a pointer into no mapping at all, where the
//! read must fail and leave the register as it found it, and a third
//! hands it four plain numbers, where nothing is quoted at all.
//!
//! What this side can check is that none of that changed the program:
//! each call answers the sum of what it was handed, armed or not,
//! quoted or not. The quotes themselves are the kernel's lines and the
//! event stream's, which `import_strings` matches against. That
//! division is the same one `iat_trace_test` is written along: a
//! program cannot read the stream it is being recorded into.
//!
//! Prints `import_strings_test: ok`, or exits with the number of the
//! failing check.

#![no_std]
#![no_main]

use ntrt::*;

#[link(name = "testdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn gala_four(a: u64, b: u64, c: u64, d: u64) -> u64;
    fn gala_fill(out: *mut u16, n: u64) -> u64;
}

/// A narrow string, as every `*A` call and every C string is handed
/// one: bytes ending in a NUL.
static NARROW: &[u8; 13] = b"ErrorMessage\0";

/// A second one, handed at a later argument, so that a reader can tell
/// which argument a quote belongs to and not merely that the call
/// carried one.
static NARROW2: &[u8; 14] = b"second string\0";

/// A wide string, as every `*W` call is handed one: UTF-16 ending in a
/// NUL, which on this side is what tells it from the narrow one.
static WIDE: [u16; 17] = wide(b"Document1 - Word");

/// Bytes that are no text: a pointer at these is a pointer at
/// something, and a quote of it would be a reading invented out of an
/// array of small numbers.
static BLOB: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];

/// An address in no mapping of this program: the read behind a quote
/// has to fail there and leave the argument as it found it, since a
/// trace must not be able to fault a program it is only watching.
const UNMAPPED: u64 = 0x7ff0_dead_0000;

/// A number too small to be a pointer, which is most of what the
/// registers of a call hold.
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
        print("import_strings_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print(" want ");
        write_file(stdout(), &hex32(want as u32));
        check_line(core::panic::Location::caller().line());
        print("\r\n");
        Err(self.0)
    }
}

/// Calls the import with four arguments and checks it answered their
/// sum: the one thing this side can say about a call the kernel may
/// have trapped, armed and quoted on the way through.
fn four(c: &mut Checks, a: u64, b: u64, x: u64, d: u64) -> Result<(), u32> {
    // SAFETY: the DLL's own export, which only adds what it is handed.
    // The arguments may be addresses of ours or numbers; this callee
    // reads neither, and the kernel's own read of them is bounded and
    // allowed to fail (UNMAPPED).
    let got = unsafe { gala_four(a, b, x, d) };
    c.eq(got, a.wrapping_add(b).wrapping_add(x).wrapping_add(d))
}

fn run() -> Result<(), u32> {
    let c = &mut Checks(0);
    let narrow = NARROW.as_ptr() as u64;
    let narrow2 = NARROW2.as_ptr() as u64;
    let wide = WIDE.as_ptr() as u64;
    let blob = BLOB.as_ptr() as u64;

    // The four cases, in the order a reader of the event sees them:
    // text a byte a character, text two, a number, a pointer at bytes
    // that are no text.
    four(c, narrow, wide, SMALL, blob)?;
    // A pointer into nothing, a number, a pointer at no text, and text
    // at the last argument: the read that fails is at the first
    // argument and the quote at the last, so one cannot have swallowed
    // the other.
    four(c, UNMAPPED, SMALL, blob, narrow2)?;
    // Four numbers, which is a call with no text to quote at all.
    four(c, SMALL, SMALL, SMALL, SMALL)?;

    // A buffer of this frame's: empty where the call is recorded and
    // holding the callee's text where its answer is. So the call
    // carries no text and its return carries it twice, at the answer
    // and at the argument the buffer was handed at -- the same register
    // read at two times, which is the whole of what a filled buffer is.
    let mut buf = [0u16; 32];
    let at = buf.as_mut_ptr() as u64;
    // SAFETY: the DLL's own export, handed this buffer and the number
    // of characters it has.
    let got = unsafe { gala_fill(buf.as_mut_ptr(), buf.len() as u64) };
    print("import_strings_test: filled\r\n");
    c.eq(got, at)?;
    c.eq(u64::from(buf[0]), u64::from(b'f'))?;
    c.eq(u64::from(buf[19]), u64::from(b'n'))?;
    c.eq(u64::from(buf[20]), 0)?;

    // And the strings are as they were: the quoting reads them and
    // nothing else, so a program cannot tell from its own memory that
    // it was read.
    c.eq(u64::from(NARROW[0]), u64::from(b'E'))?;
    c.eq(u64::from(WIDE[16]), 0)?;
    c.eq(u64::from(BLOB[15]), 16)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("import_strings_test: ok\r\n");
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
