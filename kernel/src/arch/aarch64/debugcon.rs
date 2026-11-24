//! The debug console the event stream goes to (`crate::events`): a host
//! file written through Arm semihosting (SYS_OPEN, SYS_WRITE; QEMU with
//! `-semihosting-config enable=on`, which xtask passes on aarch64, and
//! the run's exit already uses: `qemu`). x86-64's is QEMU's port 0xe9;
//! QEMU's `virt` machine has no such device. The `events_host` flag on
//! the kernel command line (which xtask gives an aarch64 test image)
//! says a semihosting host takes the stream; the host names the file,
//! per run, as its semihosting command line (SYS_GET_CMDLINE; xtask's
//! `-semihosting-config ...,arg=<path>`), as x86-64's host names its
//! device's file. An image does not carry the path: its copies (an
//! `XTASK_REPEAT` boot's) would all write the first one's file. Without
//! the flag nothing is written, so a machine with no semihosting host
//! never traps on it.

use core::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use crate::sync::IrqMutex;

pub static DEBUGCON: IrqMutex<DebugCon> = IrqMutex::new(DebugCon);

/// The file's name (NUL-terminated) and its length, from the host.
static PATH: IrqMutex<([u8; 512], usize)> = IrqMutex::new(([0; 512], 0));
/// The semihosting handle: NONE (no file named), OPEN_FAILED, or one.
static HANDLE: AtomicI64 = AtomicI64::new(NONE);
const NONE: i64 = -1;
const OPEN_FAILED: i64 = -2;
const UNOPENED: i64 = -3;

/// Semihosting operations (Arm's semihosting specification) and SYS_OPEN's
/// mode 5 ("wb": create or truncate, write, binary).
const SYS_OPEN: u64 = 0x01;
const SYS_WRITE: u64 = 0x05;
const SYS_GET_CMDLINE: u64 = 0x15;
const MODE_WRITE_BINARY: u64 = 5;

/// Asks the host for the event file's name when the command line says a
/// semihosting host takes the stream (`events_host`), and says on the
/// serial log which file that is (or why there is none): the host checks
/// the stream it reads is the one the kernel wrote.
pub fn configure(cmdline: &crate::cmdline::Cmdline<'_>) {
    if !cmdline.flag("events_host") {
        return;
    }
    let mut p = PATH.lock();
    // SYS_GET_CMDLINE's block: the buffer and its size in, the length
    // written out (without the NUL it adds); the room for the NUL is kept.
    let mut block = [p.0.as_mut_ptr() as u64, (p.0.len() - 1) as u64];
    let failed = semihost_mut(SYS_GET_CMDLINE, &mut block) != 0;
    let len = block[1] as usize;
    if failed || len == 0 || len >= p.0.len() {
        crate::kprintln!(
            "events: no file: the semihosting host named none (SYS_GET_CMDLINE {}, {len} bytes)",
            if failed { "failed" } else { "answered" }
        );
        return;
    }
    p.0[len] = 0;
    p.1 = len;
    match core::str::from_utf8(&p.0[..len]) {
        Ok(path) => crate::kprintln!("events: file {path} (named by the semihosting host)"),
        Err(_) => crate::kprintln!("events: file of {len} bytes, not UTF-8"),
    }
    HANDLE.store(UNOPENED, Ordering::Release);
}

/// A semihosting call whose parameter block the host writes.
fn semihost_mut(op: u64, block: &mut [u64]) -> u64 {
    let result: u64;
    // SAFETY: QEMU's semihosting reads and writes the block (and what it
    // points at, a buffer the caller owns); only called once the command
    // line said a semihosting host is there.
    unsafe {
        core::arch::asm!(
            "hlt #0xf000",
            inout("x0") op => result,
            in("x1") block.as_mut_ptr(),
            options(nostack, preserves_flags)
        )
    };
    result
}

/// A semihosting call: `op` with its parameter block.
fn semihost(op: u64, block: &[u64]) -> u64 {
    let result: u64;
    // SAFETY: QEMU's semihosting reads the block (and what it points at);
    // only called once the host named a file, so a semihosting host is there.
    unsafe {
        core::arch::asm!(
            "hlt #0xf000",
            inout("x0") op => result,
            in("x1") block.as_ptr(),
            options(nostack, readonly, preserves_flags)
        )
    };
    result
}

fn handle() -> Option<u64> {
    match HANDLE.load(Ordering::Acquire) {
        UNOPENED => {
            let p = PATH.lock();
            let h =
                semihost(SYS_OPEN, &[p.0.as_ptr() as u64, MODE_WRITE_BINARY, p.1 as u64]) as i64;
            HANDLE.store(if h < 0 { OPEN_FAILED } else { h }, Ordering::Release);
            (h >= 0).then_some(h as u64)
        }
        h if h >= 0 => Some(h as u64),
        _ => None,
    }
}

/// How many writes the file has taken, for the summary (x86-64 counts
/// the same thing: its port takes one write a line, this one takes one a
/// piece of a line, a semihosting call being a trap and not an exit).
static WRITES: AtomicU64 = AtomicU64::new(0);

/// The number of writes the device has taken.
pub fn writes() -> u64 {
    WRITES.load(Ordering::Relaxed)
}

/// Nothing is held back, so there is nothing to flush (x86-64 holds a
/// line).
pub fn flush() {}

pub struct DebugCon;

impl core::fmt::Write for DebugCon {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        if let Some(h) = handle() {
            semihost(SYS_WRITE, &[h, s.as_ptr() as u64, s.len() as u64]);
            WRITES.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }
}
