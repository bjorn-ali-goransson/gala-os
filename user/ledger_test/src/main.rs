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
//! (WORKFLOW "Test programs"). One more gap is reported after the table
//! is full, under a name of its own: whether its row is in the dump is
//! what says a slot was freed, which is what the exclusion filter is
//! read by (`ledger_except`). One report before them carries a name
//! that is not text, which the ledger names apart from a name it had no
//! room for. The questions above them are Windows' own
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
/// More records than the ledger's tables hold entries (512 fixed and
/// 1536 the heap gives), each its own entry (one name, a detail of its
/// own), so both fill up and the rest are dropped under their kind.
const FILL_RECORDS: u64 = 2100;
/// The name they are recorded under; printable ASCII, as the ledger asks.
const FILL_NAME: &[u8] = b"ledger_test.exe!fill";
/// The name of the one gap reported after the table is full. The table
/// has no room for it, so its row is in the dump only when the records
/// above were kept out of the table (`ledger_except=ledger_test.exe!fill`,
/// scenario `ledger_except`), which is how a run says the filter freed
/// the slots rather than only counting what it hid.
const AFTER_NAME: &[u8] = b"ledger_test.exe!after";
/// A name that is not plain text, as a report whose length was never
/// meant carries: the ledger cannot name its row and says so under
/// `?text`, which is not the `?user` of a name table with no room. It
/// is reported before the table fills, because a row the table dropped
/// would say neither.
const UNNAMEABLE: &[u8] = b"ledger_test.exe!\0\0bad";
/// Its detail, so that its row is told from the rest at a glance.
const UNNAMEABLE_DETAIL: u64 = 0xbad;
const UNKNOWN_PROCESS_CLASS: u32 = 0x7f00;

/// System service `N` with no arguments, made directly: x64's `syscall`
/// (the number in eax), ARM64's `svc #N` (Windows ARM64's ntdll puts the
/// number in the immediate).
///
/// A call returns with every volatile register the kernel's to change,
/// as it does through ntdll's stubs, which is what `clobber_abi` and
/// `=> _` say here. A call that kept that to itself would leave the
/// caller holding an argument in a register the kernel had written to:
/// these calls are made in a loop, and the second of them reported a
/// name as long as whatever rdx held by then (WORKFLOW "Test programs").
#[cfg(target_arch = "x86_64")]
fn raw_syscall<const N: u32>() -> NtStatus {
    let status: u64;
    // SAFETY: a system call with no arguments; the kernel only reads the
    // number and returns a status in rax.
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") u64::from(N) => status,
            clobber_abi("win64"),
            options(nostack)
        )
    };
    status as NtStatus
}

#[cfg(target_arch = "aarch64")]
fn raw_syscall<const N: u32>() -> NtStatus {
    let status: u64;
    // SAFETY: a system call with no arguments; the status comes in x0.
    unsafe {
        core::arch::asm!(
            "svc #{n}",
            n = const N,
            lateout("x0") status,
            clobber_abi("C"),
            options(nostack)
        )
    };
    status as NtStatus
}

/// Four arguments, in the system-call convention: x64's (r10, as ntdll's
/// stubs leave the first, then rdx, r8, r9), ARM64's (x0-x3).
#[cfg(target_arch = "x86_64")]
fn raw_syscall4<const N: u32>(a: u64, b: u64, c: u64, d: u64) -> NtStatus {
    let status: u64;
    // SAFETY: GalaRecordMissing reads `a` for `b` bytes and returns a
    // status in rax; the pointer is a slice this program owns.
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") u64::from(N) => status,
            inlateout("r10") a => _,
            inlateout("rdx") b => _,
            inlateout("r8") c => _,
            inlateout("r9") d => _,
            clobber_abi("win64"),
            options(nostack)
        )
    };
    status as NtStatus
}

#[cfg(target_arch = "aarch64")]
fn raw_syscall4<const N: u32>(a: u64, b: u64, c: u64, d: u64) -> NtStatus {
    let status: u64;
    // SAFETY: as x64's; the status comes in x0.
    unsafe {
        core::arch::asm!(
            "svc #{n}",
            n = const N,
            inlateout("x0") a => status,
            inlateout("x1") b => _,
            inlateout("x2") c => _,
            inlateout("x3") d => _,
            clobber_abi("C"),
            options(nostack)
        )
    };
    status as NtStatus
}

fn run() -> u32 {
    if raw_syscall::<UNKNOWN_SERVICE>() != STATUS_INVALID_SYSTEM_SERVICE {
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
    // A name the ledger cannot carry, while the table still has room
    // for its row.
    let status = raw_syscall4::<GALA_RECORD_MISSING>(
        UNNAMEABLE.as_ptr() as u64,
        UNNAMEABLE.len() as u64,
        WHAT_OTHER,
        UNNAMEABLE_DETAIL,
    );
    if status != 0 {
        return 6;
    }
    for detail in 0..FILL_RECORDS {
        let status = raw_syscall4::<GALA_RECORD_MISSING>(
            FILL_NAME.as_ptr() as u64,
            FILL_NAME.len() as u64,
            WHAT_OTHER,
            detail,
        );
        if status != 0 {
            return 4;
        }
    }
    // The gap after them, which the full table drops and a filtered run
    // records.
    let status = raw_syscall4::<GALA_RECORD_MISSING>(
        AFTER_NAME.as_ptr() as u64,
        AFTER_NAME.len() as u64,
        WHAT_OTHER,
        0,
    );
    if status != 0 {
        return 5;
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
