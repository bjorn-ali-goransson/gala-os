//! The debug console the event stream goes to (`crate::events`): a host
//! file written through Arm semihosting (SYS_OPEN, SYS_WRITE; QEMU with
//! `-semihosting-config enable=on`, which xtask passes on aarch64, and
//! the run's exit already uses: `qemu`). x86-64's is QEMU's port 0xe9;
//! QEMU's `virt` machine has no such device. The file is the one the
//! kernel command line names (`events_file=<host path>`, which xtask
//! gives an aarch64 image), opened at the first write; without that
//! argument nothing is written, so a machine with no semihosting host
//! never traps on it.

use core::sync::atomic::{AtomicI64, Ordering};

use crate::sync::IrqMutex;

pub static DEBUGCON: IrqMutex<DebugCon> = IrqMutex::new(DebugCon);

/// The file's name (NUL-terminated) and its length, from the command line.
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
const MODE_WRITE_BINARY: u64 = 5;

/// Takes the event file's name from the command line (`events_file=`).
pub fn configure(cmdline: &crate::cmdline::Cmdline<'_>) {
    let Some(path) = cmdline.value("events_file") else { return };
    let mut p = PATH.lock();
    if path.is_empty() || path.len() >= p.0.len() {
        return;
    }
    p.0[..path.len()].copy_from_slice(path.as_bytes());
    p.0[path.len()] = 0;
    p.1 = path.len();
    HANDLE.store(UNOPENED, Ordering::Release);
}

/// A semihosting call: `op` with its parameter block.
fn semihost(op: u64, block: &[u64]) -> u64 {
    let result: u64;
    // SAFETY: QEMU's semihosting reads the block (and what it points at);
    // only called once a file was named, so a semihosting host is there.
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

pub struct DebugCon;

impl core::fmt::Write for DebugCon {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        if let Some(h) = handle() {
            semihost(SYS_WRITE, &[h, s.as_ptr() as u64, s.len() as u64]);
        }
        Ok(())
    }
}
