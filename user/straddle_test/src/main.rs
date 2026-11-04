//! Code patched where an instruction straddles a page boundary, as a JIT
//! patches it: HotSpot rewrites the 8-byte object address of a
//! `mov r64, imm64` in compiled code (a garbage collection moved the
//! object), and nothing keeps such an instruction from starting a few
//! bytes before the end of a page, its immediate on the next page. On
//! real Windows the next call runs the new instruction.
//!
//! Each case writes `mov rax, imm64; ret` into PAGE_EXECUTE_READWRITE
//! memory, calls it, then rewrites the immediate and calls it again, many
//! times: every call must return the value written last.
//! - case 1: the instruction starts 2 bytes before a page boundary (REX
//!   and opcode on one page, the whole immediate and the `ret` on the
//!   next);
//! - case 2: the immediate itself straddles the boundary, and only its
//!   high half (on the second page) changes;
//! - case 3: the same function inside one page (the control).
//!
//! Why (ROADMAP Priorities, "the Java corruption", 2026-11-03): QEMU's
//! TCG 8.1.0 to 8.2.5 records a translated block that spans two pages
//! only under its first page (accel/tcg/tb-maint.c `tb_record`
//! computes the second page's index from the first page's address), so a
//! write to the second page does not discard the translation, and the
//! old immediate runs on: the JVM then uses a pre-GC object address.
//!
//! Prints `straddle_test: ok` or exits with the number of the failing
//! check (details on stdout).

#![no_std]
#![no_main]

use ntrt::*;

const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const PAGE: usize = 0x1000;
const ROUNDS: u64 = 200;

fn check(id: u32, cond: bool, round: u64, got: u64) -> Result<(), u32> {
    if cond {
        return Ok(());
    }
    print("straddle_test: check ");
    write_file(stdout(), &hex32(id));
    print(" round ");
    write_file(stdout(), &hex32(round as u32));
    print(" got ");
    write_file(stdout(), &hex32((got >> 32) as u32));
    write_file(stdout(), &hex32(got as u32));
    print("\r\n");
    Err(id)
}

/// Writes `mov rax, imm; ret` at `at`.
fn write_function(at: usize, imm: u64) {
    let mut bytes = [0x48u8, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0xc3];
    bytes[2..10].copy_from_slice(&imm.to_le_bytes());
    for (k, b) in bytes.iter().enumerate() {
        // SAFETY: inside the committed RWX pages.
        unsafe { ((at + k) as *mut u8).write_volatile(*b) };
    }
}

/// Rewrites the immediate of the function at `at` with one 8-byte store
/// (two 4-byte stores, low half first, when it is not 8-byte aligned),
/// as a JIT patches an embedded constant.
fn patch(at: usize, imm: u64) {
    let p = at + 2;
    if p.is_multiple_of(8) {
        // SAFETY: the aligned immediate inside the RWX pages.
        unsafe { (p as *mut u64).write_volatile(imm) };
    } else {
        // SAFETY: as above; 4-byte aligned halves (the callers place them so).
        unsafe {
            (p as *mut u32).write_volatile(imm as u32);
            ((p + 4) as *mut u32).write_volatile((imm >> 32) as u32);
        }
    }
}

fn call(at: usize) -> u64 {
    // SAFETY: `at` holds `mov rax, imm64; ret`.
    let f: extern "C" fn() -> u64 = unsafe { core::mem::transmute(at) };
    f()
}

/// The value of round `round` in case `case` (distinct in both halves).
fn value(case: u64, round: u64) -> u64 {
    (case << 56) | (round << 32) | (0x5000_0000 + round * 0x101)
}

/// Writes the function at `at`, then rewrites and calls it `ROUNDS`
/// times; each call must return the value written last (check `id`).
fn run_case(id: u32, case: u64, at: usize) -> Result<(), u32> {
    write_function(at, value(case, 0));
    check(id, call(at) == value(case, 0), 0, call(at))?;
    for round in 1..=ROUNDS {
        patch(at, value(case, round));
        let got = call(at);
        check(id, got == value(case, round), round, got)?;
        // Again: the translation made by the call above must not outlive
        // the next rewrite either.
        let again = call(at);
        check(id, again == got, round, again)?;
    }
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("straddle_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

fn run() -> Result<(), u32> {
    let (s, base, _) = allocate(0, 4 * PAGE, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE);
    check(1, s == STATUS_SUCCESS, 0, u64::from(s))?;
    // Case 1: REX and opcode on page 0, the immediate (8-byte aligned)
    // and `ret` on page 1.
    run_case(2, 1, base + PAGE - 2)?;
    // Case 2: the immediate straddles the next boundary: low half on
    // page 1, high half on page 2 (both 4-byte aligned).
    run_case(3, 2, base + 2 * PAGE - 6)?;
    // Case 3: inside page 3 (control).
    run_case(4, 3, base + 3 * PAGE + 0x100 - 2)?;
    Ok(())
}
entry!(main);
