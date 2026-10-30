//! Requests the system does not support must fail cleanly, as on real
//! Windows: an unknown system service number gives
//! STATUS_INVALID_SYSTEM_SERVICE, an unknown information class
//! STATUS_INVALID_INFO_CLASS. On gala each one is also counted in the
//! missing-features ledger (the boot scenario checks the `ledger:` lines).
//! Prints `ledger_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use ntrt::*;

const STATUS_INVALID_SYSTEM_SERVICE: NtStatus = 0xc000_001c;
const STATUS_INVALID_INFO_CLASS: NtStatus = 0xc000_0003;
/// Far above any service number Windows has used.
const UNKNOWN_SERVICE: u32 = 0x0fff;
const UNKNOWN_PROCESS_CLASS: u32 = 0x7f00;

fn raw_syscall(number: u32) -> NtStatus {
    let status: u64;
    // SAFETY: a system call with no arguments; the kernel only reads the
    // number and returns a status in rax.
    unsafe {
        core::arch::asm!(
            "mov r10, rcx",
            "syscall",
            inlateout("rax") u64::from(number) => status,
            out("rcx") _,
            out("r10") _,
            out("r11") _,
            options(nostack)
        )
    };
    status as NtStatus
}

fn run() -> u32 {
    if raw_syscall(UNKNOWN_SERVICE) != STATUS_INVALID_SYSTEM_SERVICE {
        return 1;
    }
    let mut buf = [0u8; 64];
    let (status, _) = query_process(CURRENT_PROCESS, UNKNOWN_PROCESS_CLASS, &mut buf);
    if status != STATUS_INVALID_INFO_CLASS {
        return 2;
    }
    // Twice: the ledger counts hits.
    let (status, _) = query_process(CURRENT_PROCESS, UNKNOWN_PROCESS_CLASS, &mut buf);
    if status != STATUS_INVALID_INFO_CLASS {
        return 3;
    }
    0
}

fn main() -> u32 {
    let failed = run();
    if failed == 0 {
        print("ledger_test: ok\r\n");
    }
    failed
}

entry!(main);
