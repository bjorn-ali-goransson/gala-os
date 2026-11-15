//! Requests the system does not support must fail cleanly, as on real
//! Windows: an unknown system service number gives
//! STATUS_INVALID_SYSTEM_SERVICE, an unknown information class
//! STATUS_INVALID_INFO_CLASS. On gala each one is also counted in the
//! missing-features ledger (the boot scenario checks the `ledger:` lines).
//! The ledger's table is finite, so the test also fills it: a record it
//! has no room for must still be counted under its kind (a `?full` line),
//! because a dump that only said how many it dropped would hide which gap
//! was lost. Filling it is asked of gala only (`ntrt::on_gala`), since it
//! takes gala's own service number: on Windows such a number reaches
//! win32k's table and faults the process instead of being refused
//! (WORKFLOW "Test programs"). The questions above it are Windows' own
//! and are asked of both. Prints `ledger_test: ok` or exits with the
//! number of the failing check.

#![no_std]
#![no_main]

use ntrt::*;

const STATUS_INVALID_SYSTEM_SERVICE: NtStatus = 0xc000_001c;
const STATUS_INVALID_INFO_CLASS: NtStatus = 0xc000_0003;
/// Far above any service number Windows has used.
const UNKNOWN_SERVICE: u32 = 0x0fff;
/// GalaRecordMissing(Name, NameLength, What, Detail), gala's own service
/// for a gap found in user mode (kernel/src/nt/syscalls.rs).
const GALA_RECORD_MISSING: u32 = 0x7f00;
/// What a report is about: 5 is "other" (`ledger::user_what`).
const WHAT_OTHER: u64 = 5;
/// More records than the ledger's table holds entries (512), each its own
/// entry (one name, a detail of its own), so the table fills up and the
/// rest are dropped under their kind.
const FILL_RECORDS: u64 = 600;
/// The name they are recorded under; printable ASCII, as the ledger asks.
const FILL_NAME: &[u8] = b"ledger_test.exe!fill";
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

/// Four arguments, in the x64 system-call convention (rcx moved to r10
/// by the caller, as ntdll does, then rdx, r8, r9).
fn raw_syscall4(number: u32, a: u64, b: u64, c: u64, d: u64) -> NtStatus {
    let status: u64;
    // SAFETY: GalaRecordMissing reads `a` for `b` bytes and returns a
    // status in rax; the pointer is a slice this program owns.
    unsafe {
        core::arch::asm!(
            "mov r10, rcx",
            "syscall",
            inlateout("rax") u64::from(number) => status,
            in("rcx") a,
            in("rdx") b,
            in("r8") c,
            in("r9") d,
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
    // Fill the table: the dump says how many `other` records it had no
    // room for, which is the line the scenario checks for. gala only: the
    // number is gala's own (see this program's own doc comment).
    if !on_gala() {
        return 0;
    }
    for detail in 0..FILL_RECORDS {
        let status = raw_syscall4(
            GALA_RECORD_MISSING,
            FILL_NAME.as_ptr() as u64,
            FILL_NAME.len() as u64,
            WHAT_OTHER,
            detail,
        );
        if status != 0 {
            return 4;
        }
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
