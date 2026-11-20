//! `imports_answers=<n>`: what a call through an import slot answered,
//! recorded by the kernel (docs/OBSERVABILITY.md, "Import tracing").
//! This checks the mechanism from the program's side, where almost the
//! whole claim is that nothing changed: a call whose return the kernel
//! trapped comes back to the same place with the same answer, calls
//! nested inside one another each get their own, and a second thread's
//! pending return is its own.
//!
//! What the kernel does is put a second address of the slot's own --
//! eight bytes past the call trap -- where the call left its return
//! address: on the stack on x86-64, in the link register on ARM64. The
//! callee returns there, which faults, and the kernel arrives with the
//! answer still in the register the ABI returns it in; it records the
//! answer and sends the return on to where the call really came from.
//!
//! So the one thing a program *can* see is a callee that reads its own
//! return address, which is what a stack walk or an unwind does:
//! `gala_return_address` gives back the trap address while the call's
//! return is trapped, and the caller's own code once the budget for
//! answers is spent. That is the limit the option is read with,
//! asserted here rather than only described.
//!
//! Prints `iat_ret_test: answering` (with `imports_answers=`) or
//! `: direct` (on Windows, or with no option), then `iat_ret_test: ok`
//! -- or exits with the number of the failing check.

#![no_std]
#![no_main]

use ntrt::*;

#[link(name = "testdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn gala_add(a: u64, b: u64) -> u64;
    fn gala_get_value() -> u64;
    fn gala_call_back(f: extern "C" fn(u64) -> u64, x: u64) -> u64;
    fn gala_return_address() -> u64;
}

unsafe extern "C" {
    static __ImageBase: u8;
}

const INITIAL_VALUE: u64 = 0x1234_5678_9abc;

/// A call through an armed slot made from inside another: the inner
/// call's return is pending under the outer one's, and each must be
/// answered to its own caller.
extern "C" fn add_one(x: u64) -> u64 {
    // SAFETY: imported from testdll, bound by the loader.
    unsafe { gala_add(x, 1) }
}

/// Two levels deep through the slot the call it is inside went through,
/// so that one slot has two returns pending at once on one thread.
extern "C" fn call_back_again(x: u64) -> u64 {
    // SAFETY: as above.
    unsafe { gala_call_back(add_one, x) }
}

/// The same calls on another thread, so that two threads have returns
/// pending at the same time: a pending return belongs to the thread
/// that made the call, and nothing may send one thread's return to
/// where another thread's call came from. Its exit code is the number
/// of its failing check, or zero.
extern "C" fn worker(_arg: usize) -> ! {
    exit_thread(match work() {
        Ok(()) => 0,
        Err(id) => id,
    })
}

fn work() -> Result<(), u32> {
    // Its own numbering, the line a report names being what tells a
    // worker's check from the main thread's.
    let mut c = Checks(0);
    // SAFETY: imported from testdll, bound by the loader; the callback
    // is this program's own function.
    unsafe {
        c.eq(gala_call_back(add_one, 20), 22)?;
        c.eq(gala_add(5, 6), 11)?;
        c.eq(gala_get_value(), INITIAL_VALUE)?;
    }
    Ok(())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);

    // What a callee sees of its own return address. While the call's
    // return is trapped it is the kernel's second trap address, which
    // sits in the region ntdll reserved and never committed -- the same
    // region the call traps are in; with nothing armed it is the
    // caller's own code, in this program's image.
    // SAFETY: imported from testdll, bound by the loader.
    let first = unsafe { gala_return_address() };
    let (s, mbi) = query(first as usize);
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    let image = &raw const __ImageBase as usize;
    let answering = mbi.state == MEM_RESERVE;
    if answering {
        c.ok(mbi.allocation_base != image, mbi.allocation_base as u64)?;
    } else {
        c.eq(mbi.kind.into(), MEM_IMAGE.into())?;
        c.eq(mbi.allocation_base as u64, image as u64)?;
    }

    // SAFETY: as above, and the callbacks are this program's own.
    unsafe {
        // One call inside another, each through an armed slot: two
        // returns pending at once, the inner answered first.
        // `gala_call_back` adds one to what the callback answered, so
        // this is (7 + 1) + 1.
        c.eq(gala_call_back(add_one, 7), 9)?;
        // And two levels, through the same slot both times:
        // ((1 + 1) + 1) + 1.
        c.eq(gala_call_back(call_back_again, 1), 4)?;
        // Plain calls, whose answers are the whole point: a small one,
        // and one wide enough that a truncated register would show.
        c.eq(gala_add(2, 3), 5)?;
        c.eq(gala_get_value(), INITIAL_VALUE)?;
    }

    // The other thread, which runs while this one waits for it.
    let (s, h, tid) = create_thread(worker, 0, false);
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.ok(tid != 0, tid as u64)?;
    c.eq(wait(h, None).into(), STATUS_SUCCESS.into())?;
    let (s, exit, _, _) = thread_basic_info(h);
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.eq(exit.into(), 0)?;
    close(h);

    // Twelve calls have gone through armed slots by now, so the five
    // answers `iat_answers` asks for are spent and the kernel leaves
    // this call's return alone: what the callee sees is this program's
    // code again, as it is with nothing armed at all. This side cannot
    // read a budget the kernel was handed, so the number the scenario
    // gives is what makes the check exact.
    // SAFETY: as above.
    let last = unsafe { gala_return_address() };
    let (s, mbi) = query(last as usize);
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.eq(mbi.kind.into(), MEM_IMAGE.into())?;
    c.eq(mbi.allocation_base as u64, image as u64)?;

    print(if answering { "iat_ret_test: answering\r\n" } else { "iat_ret_test: direct\r\n" });
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("iat_ret_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);

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
        print("iat_ret_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        check_line(core::panic::Location::caller().line());
        print("\r\n");
        Err(self.0)
    }
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
