//! The command line: PEB->ProcessParameters->CommandLine split into
//! arguments by the CommandLineToArgvW rules must give the program path and
//! `alpha`, `beta gamma`. Run it on Windows as
//! `args_test.exe alpha "beta gamma"`; gala's boot scenario passes the same
//! arguments with `args=`. Prints `args_test: ok` or exits with the number
//! of the failing check.

#![no_std]
#![no_main]

use ntrt::*;

const MAX_ARGS: usize = 8;
const MAX_CHARS: usize = 512;

/// Arguments as UTF-16, stored back to back in `chars`.
struct Argv {
    chars: [u16; MAX_CHARS],
    len: usize,
    ends: [usize; MAX_ARGS],
    count: usize,
}

impl Argv {
    fn push(&mut self, c: u16) {
        if self.len < MAX_CHARS {
            self.chars[self.len] = c;
            self.len += 1;
        }
    }

    fn arg(&self, i: usize) -> &[u16] {
        let start = if i == 0 { 0 } else { self.ends[i - 1] };
        &self.chars[start..self.ends[i]]
    }
}

const SPACE: u16 = b' ' as u16;
const TAB: u16 = b'\t' as u16;
const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;

/// Splits a command line by the documented CommandLineToArgvW rules:
/// argv[0] runs to the next space, or is quoted (no escapes); then
/// whitespace separates arguments, `"` toggles quoting, `2n` backslashes
/// before a quote give `n` backslashes, `2n+1` give `n` and a literal
/// quote, other backslashes are literal, and `""` inside quotes is a quote.
fn split(cmd: &[u16]) -> Argv {
    let mut a = Argv { chars: [0; MAX_CHARS], len: 0, ends: [0; MAX_ARGS], count: 0 };
    let mut i = 0;
    // argv[0]
    if cmd.first() == Some(&QUOTE) {
        i = 1;
        while i < cmd.len() && cmd[i] != QUOTE {
            a.push(cmd[i]);
            i += 1;
        }
        i += 1;
    } else {
        while i < cmd.len() && cmd[i] != SPACE && cmd[i] != TAB {
            a.push(cmd[i]);
            i += 1;
        }
    }
    a.ends[0] = a.len;
    a.count = 1;
    loop {
        while i < cmd.len() && (cmd[i] == SPACE || cmd[i] == TAB) {
            i += 1;
        }
        if i >= cmd.len() || a.count == MAX_ARGS {
            return a;
        }
        let mut quoted = false;
        while i < cmd.len() && (quoted || (cmd[i] != SPACE && cmd[i] != TAB)) {
            if cmd[i] == BACKSLASH {
                let mut n = 0;
                while i < cmd.len() && cmd[i] == BACKSLASH {
                    n += 1;
                    i += 1;
                }
                if i < cmd.len() && cmd[i] == QUOTE {
                    for _ in 0..n / 2 {
                        a.push(BACKSLASH);
                    }
                    if n % 2 == 1 {
                        a.push(QUOTE);
                        i += 1;
                    }
                } else {
                    for _ in 0..n {
                        a.push(BACKSLASH);
                    }
                }
            } else if cmd[i] == QUOTE {
                if quoted && cmd.get(i + 1) == Some(&QUOTE) {
                    a.push(QUOTE);
                    i += 2;
                } else {
                    quoted = !quoted;
                    i += 1;
                }
            } else {
                a.push(cmd[i]);
                i += 1;
            }
        }
        a.ends[a.count] = a.len;
        a.count += 1;
    }
}

struct Checks(u32);

impl Checks {
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("args_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn is(s: &[u16], want: &str) -> bool {
    s.len() == want.len() && s.iter().zip(want.bytes()).all(|(&a, b)| a == u16::from(b))
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // Self-check of the splitting rules on a fixed line.
    let sample = wide!("\"C:\\a b\\x.exe\" one \"two three\" a\\\\\\\"b c\\\\\"d e\" \"q\"\"r\"");
    let s = split(&sample);
    c.ok(s.count == 6, s.count as u64)?;
    c.ok(is(s.arg(0), "C:\\a b\\x.exe"), 0)?;
    c.ok(is(s.arg(1), "one"), 1)?;
    c.ok(is(s.arg(2), "two three"), 2)?;
    c.ok(is(s.arg(3), "a\\\"b"), 3)?;
    c.ok(is(s.arg(4), "c\\d e"), 4)?;
    c.ok(is(s.arg(5), "q\"r"), 5)?;

    // Our own command line.
    let pp = peb_process_parameters();
    // SAFETY: CommandLine is a UNICODE_STRING at +0x70.
    let cmd = unsafe {
        let len = usize::from(*(pp.add(0x70) as *const u16)) / 2;
        core::slice::from_raw_parts(*(pp.add(0x78) as *const *const u16), len)
    };
    let argv = split(cmd);
    c.ok(argv.count == 3, argv.count as u64)?;
    c.ok(ends_with_ci(argv.arg(0), "args_test.exe"), argv.arg(0).len() as u64)?;
    c.ok(is(argv.arg(1), "alpha"), argv.arg(1).len() as u64)?;
    c.ok(is(argv.arg(2), "beta gamma"), argv.arg(2).len() as u64)?;
    Ok(())
}

/// PEB->ProcessParameters.
fn peb_process_parameters() -> *const u8 {
    // SAFETY: PEB+0x20 holds ProcessParameters on x64.
    unsafe { *(peb().add(0x20) as *const *const u8) }
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("args_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
