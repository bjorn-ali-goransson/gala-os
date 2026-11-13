//! The program the whole-machine snapshot scenario saves and resumes
//! (ROADMAP SN step 1; scenario `snapshot_machine`, WORKFLOW "Machine
//! snapshots"). It works in phases, each announced by a line:
//!
//! 1. writes a 128 KiB file on C: (`C:\Users\Public\snapshot_test.dat`,
//!    pattern A), flushes it, reads it back, then prints `snapshot_test:
//!    phase 1 done`: the line xtask saves the machine at;
//! 2. waits 30 times 100 ms (NtDelayExecution) and prints the longest of
//!    those waits as the performance counter measured it (`snapshot_test:
//!    longest wait <n> ms`): a wait the machine was saved or restored in
//!    shows whether the guest's clock counted the pause;
//! 3. reads the file back (pattern A: what the snapshot's memory and disk
//!    hold), writes its second half with pattern B, flushes, reads it all
//!    back, prints the guest's system time (`snapshot_test: guest time
//!    <seconds since 1970>`, compared with the host's by xtask) and
//!    `snapshot_test: ok`.
//!
//! A run resumed from the snapshot starts inside phase 2, so its log has
//! phase 3's lines and not phase 1's. On Windows the program runs
//! straight through (no snapshot) and prints the same lines. Exits with 0,
//! or the number of the failing check (`check N got X (line L)`).

#![no_std]
#![no_main]

use ntrt::*;

const SYNC: u32 = FILE_SYNCHRONOUS_IO_NONALERT | FILE_NON_DIRECTORY_FILE;
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const RW: u32 = GENERIC_READ | GENERIC_WRITE | DELETE;
const FILE_DISPOSITION_INFORMATION: u32 = 13;
const SIZE: usize = 128 * 1024;
/// 1601-01-01 to 1970-01-01 in 100 ns units.
const UNIX_EPOCH: i64 = 116_444_736_000_000_000;

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
        print("snapshot_test: check ");
        print_num(u64::from(self.0));
        print(" got ");
        print_num(got);
        print(" (line ");
        print_num(u64::from(core::panic::Location::caller().line()));
        print(")\r\n");
        Err(self.0)
    }
}

fn print_num(mut n: u64) {
    let mut b = [0u8; 20];
    let mut i = b.len();
    loop {
        i -= 1;
        b[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    print(core::str::from_utf8(&b[i..]).unwrap_or("?"));
}

fn pattern_a(i: usize) -> u8 {
    (i % 251) as u8
}

fn pattern_b(i: usize) -> u8 {
    (i % 241) as u8 ^ 0x5a
}

/// Reads the whole file and counts the bytes that differ from `want`.
fn mismatches(h: Handle, want: impl Fn(usize) -> u8) -> (NtStatus, u64) {
    let mut buf = [0u8; 4096];
    let mut bad = 0;
    for chunk in 0..SIZE / buf.len() {
        let off = chunk * buf.len();
        let (s, n) = read_file(h, &mut buf, Some(off as i64));
        if s != STATUS_SUCCESS || n != buf.len() {
            return (s, u64::MAX);
        }
        bad += buf.iter().enumerate().filter(|&(i, &b)| b != want(off + i)).count() as u64;
    }
    (STATUS_SUCCESS, bad)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = u64::from(STATUS_SUCCESS);
    let path = wide!("\\??\\C:\\Users\\Public\\snapshot_test.dat");
    let path = UnicodeString::new(&path);
    let attrs = ObjectAttributes::new(&path, Handle(0), OBJ_CASE_INSENSITIVE);

    // Phase 1: the file, on the disk before the snapshot.
    let (s, h, _) = create_file(&attrs, RW, SHARE_ALL, FILE_OVERWRITE_IF, SYNC);
    c.eq(s.into(), ok)?;
    let mut buf = [0u8; 4096];
    for chunk in 0..SIZE / buf.len() {
        let off = chunk * buf.len();
        for (i, b) in buf.iter_mut().enumerate() {
            *b = pattern_a(off + i);
        }
        c.eq(write_file_at(h, &buf, off as i64).0.into(), ok)?;
    }
    c.eq(flush_file(h).into(), ok)?;
    let (s, bad) = mismatches(h, pattern_a);
    c.eq(s.into(), ok)?;
    c.eq(bad, 0)?;
    print("snapshot_test: phase 1 done\r\n");

    // Phase 2: waits; the machine is saved (or resumed) in one of them.
    let (_, freq) = performance_counter();
    c.ok(freq > 0, freq as u64)?;
    let (mut shortest, mut longest) = (i64::MAX, 0);
    for _ in 0..30 {
        let (t0, _) = performance_counter();
        c.eq(delay(-1_000_000).into(), ok)?;
        let (t1, _) = performance_counter();
        shortest = shortest.min(t1 - t0);
        longest = longest.max(t1 - t0);
    }
    print("snapshot_test: shortest wait ");
    print_num((shortest * 10_000 / freq) as u64);
    print(" x0.1 ms, longest ");
    print_num((longest * 10_000 / freq) as u64);
    print(" x0.1 ms\r\n");

    // Phase 3: what phase 1 wrote, then a write after the resume.
    let (s, bad) = mismatches(h, pattern_a);
    c.eq(s.into(), ok)?;
    c.eq(bad, 0)?;
    for chunk in SIZE / 2 / buf.len()..SIZE / buf.len() {
        let off = chunk * buf.len();
        for (i, b) in buf.iter_mut().enumerate() {
            *b = pattern_b(off + i);
        }
        c.eq(write_file_at(h, &buf, off as i64).0.into(), ok)?;
    }
    c.eq(flush_file(h).into(), ok)?;
    let (s, bad) = mismatches(h, |i| if i < SIZE / 2 { pattern_a(i) } else { pattern_b(i) });
    c.eq(s.into(), ok)?;
    c.eq(bad, 0)?;
    c.eq(set_file(h, FILE_DISPOSITION_INFORMATION, &[1]).into(), ok)?;
    close(h);

    let (s, now) = system_time();
    c.eq(s.into(), ok)?;
    print("snapshot_test: guest time ");
    print_num(((now - UNIX_EPOCH) / 10_000_000) as u64);
    print("\r\n");
    print("snapshot_test: ok\r\n");
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => 0,
        Err(id) => id,
    }
}

entry!(main);
