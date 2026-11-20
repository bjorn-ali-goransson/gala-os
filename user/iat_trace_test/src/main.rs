//! `imports=<program>[!<dll>]`: the first call through each of a
//! program's import slots, recorded by the kernel
//! (docs/OBSERVABILITY.md, "Import tracing"). This checks the mechanism
//! from the program's side, where it is visible: an armed slot holds an
//! address in a reserved page instead of the export's, the first call
//! through it faults, and the kernel answers the fault by putting the
//! real address in the slot and continuing the call. So the same call
//! answers what it always did, the slot afterwards is the export's
//! address, and every later call goes straight there.
//!
//! Data imports are left alone, since the value a program reads from
//! such a slot *is* what it then uses: `GALA_VALUE`'s slot holds the
//! export's address throughout. A slot of a function can be read as
//! data too -- a hook installer reads the import's value and then the
//! bytes it points at -- and that read faults at the trap address. The
//! kernel answers it as the state the program would have had with
//! nothing armed: the export's address back in the slot and in the
//! register the stale value sits in, the read taken again. So reading
//! an import's first byte gives the export's first byte, armed or not.
//!
//! `imports_repeat=<n>` keeps a slot armed instead, so that every call
//! through it faults and is recorded and not only the first. From the
//! program's side that is the slot still holding the trap address after
//! a call has gone through it, calls that answer what they always did
//! while it does, and the export's address back in the slot once the
//! budget for those records is spent. The count checked is the budget
//! `iat_repeat` gives (three records, so four calls through the slot:
//! the fourth finds the budget spent and repairs it), this side having
//! no way to read a budget the kernel was handed.
//!
//! Prints `iat_trace_test: repeating` (with `imports_repeat=`),
//! `: trapped` (on gala, with `imports=` alone) or `: direct` (on
//! Windows, or with no option), then `iat_trace_test: ok` -- or exits
//! with the number of the failing check.

#![no_std]
#![no_main]

use ntrt::*;

#[link(name = "testdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn gala_add(a: u64, b: u64) -> u64;
    fn gala_get_value() -> u64;
    static GALA_VALUE: u64;
}

unsafe extern "C" {
    static __ImageBase: u8;
}

const INITIAL_VALUE: u64 = 0x1234_5678_9abc;

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
        print("iat_trace_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        check_line(core::panic::Location::caller().line());
        print("\r\n");
        Err(self.0)
    }
}

fn u32_at(addr: usize) -> u32 {
    // SAFETY: callers read inside our mapped image.
    unsafe { (addr as *const u32).read_unaligned() }
}

fn u64_at(addr: usize) -> u64 {
    // SAFETY: as above.
    unsafe { (addr as *const u64).read_unaligned() }
}

/// An import slot's current value, read afresh each time: the kernel
/// writes a slot behind the program's back, so a cached read would not
/// see it.
fn slot(addr: usize) -> usize {
    // SAFETY: the address of one of our own IAT entries.
    unsafe { core::ptr::read_volatile(addr as *const usize) }
}

/// The first byte of the code at `addr`: what a hook installer reads of
/// a function it is about to patch, and the only reason this test
/// dereferences an import's value.
fn byte_at(addr: usize) -> u8 {
    // SAFETY: the address of an exported function -- or, where the slot
    // is armed, of the trap the kernel wrote in its place, which the
    // kernel repairs to that same export.
    unsafe { core::ptr::read_volatile(addr as *const u8) }
}

/// A NUL-terminated string in our image.
fn c_str(addr: usize) -> &'static [u8] {
    let mut n = 0;
    // SAFETY: import names are NUL-terminated strings in the image.
    unsafe {
        while *((addr + n) as *const u8) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(addr as *const u8, n)
    }
}

/// Our imports from `dll`: (name, the address of its IAT slot) for each
/// by-name import. The slot's address rather than its value, because
/// what this test is about is the value changing.
fn imports_from(dll: &[u8], out: &mut [(&'static [u8], usize); 8]) -> usize {
    let image = &raw const __ImageBase as usize;
    let pe = image + u32_at(image + 0x3c) as usize;
    let dir = u32_at(pe + 24 + 112 + 8) as usize; // DataDirectory[1]
    let mut n = 0;
    let mut desc = image + dir;
    while u32_at(desc + 12) != 0 {
        if c_str(image + u32_at(desc + 12) as usize).eq_ignore_ascii_case(dll) {
            let (lookup, iat) = (image + u32_at(desc) as usize, image + u32_at(desc + 16) as usize);
            let mut i = 0;
            while u64_at(lookup + 8 * i) != 0 && n < out.len() {
                let entry = u64_at(lookup + 8 * i);
                if entry >> 63 == 0 {
                    out[n] = (c_str(image + entry as usize + 2), iat + 8 * i);
                    n += 1;
                }
                i += 1;
            }
        }
        desc += 20;
    }
    n
}

/// The address of the slot of the import named, among those found.
fn named(imports: &[(&'static [u8], usize); 8], n: usize, name: &[u8]) -> usize {
    imports[..n].iter().find(|&&(had, _)| had == name).map_or(0, |&(_, at)| at)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let mut imports = [(&[][..], 0usize); 8];
    let n = imports_from(b"testdll.dll", &mut imports);
    c.eq(n as u64, 3)?;
    let (add, get) = (named(&imports, n, b"gala_add"), named(&imports, n, b"gala_get_value"));
    let value = named(&imports, n, b"GALA_VALUE");
    c.ok(add != 0 && get != 0 && value != 0, 0)?;

    // testdll's base, from the data import: a data slot is never armed,
    // so it holds an address in the DLL's image whatever the option says.
    let (s, mbi) = query(slot(value));
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.eq(mbi.kind.into(), MEM_IMAGE.into())?;
    let dll = mbi.allocation_base;
    c.ok(export_dll_name(dll).is_some_and(|name| name.eq_ignore_ascii_case(b"testdll.dll")), 0)?;
    c.ok(find_export(dll, b"GALA_VALUE") == Some(slot(value)), slot(value) as u64)?;

    // Before the first call: either both code slots hold their export's
    // address (nothing is armed -- Windows, or no option) or neither
    // does (both are armed). One of each would mean the arming skipped a
    // slot, which is the bug this check is here for.
    let direct = [
        find_export(dll, b"gala_add").unwrap_or(0),
        find_export(dll, b"gala_get_value").unwrap_or(0),
    ];
    c.ok(direct[0] != 0 && direct[1] != 0, 0)?;
    let before = [slot(add), slot(get)];
    let trapped = before[0] != direct[0];
    c.eq((before[1] != direct[1]) as u64, trapped as u64)?;
    if trapped {
        // An armed slot points at a page that is reserved and not
        // committed: running it faults, which is what gives the kernel
        // the call.
        let (s, mbi) = query(before[0]);
        c.eq(s.into(), STATUS_SUCCESS.into())?;
        c.eq(mbi.state.into(), MEM_RESERVE.into())?;
        c.ok(mbi.allocation_base != dll, mbi.allocation_base as u64)?;
    }

    // A slot read as data, and dereferenced: the byte an import's value
    // points at is the export's first byte, armed or not. On gala with
    // the option this is a fault at a trap address that is not an
    // instruction fetch, which the kernel repairs; on Windows the slot
    // holds the export's address and nothing faults.
    c.eq(byte_at(slot(get)).into(), byte_at(direct[1]).into())?;
    // The read put the slot back, so `gala_get_value` below is a direct
    // call and the kernel records a read of this slot in place of a
    // first call through it.
    c.ok(slot(get) == direct[1], slot(get) as u64)?;

    // The calls themselves answer what they always did, armed or not.
    // SAFETY: imported from testdll, bound by the loader.
    unsafe {
        c.eq(gala_add(2, 3), 5)?;
        c.eq(gala_get_value(), INITIAL_VALUE)?;
        c.eq(core::ptr::read_volatile(&raw const GALA_VALUE), INITIAL_VALUE)?;
    }

    // With `imports_repeat=` the kernel leaves the slot armed, so the
    // call above faulted and the slot holds the trap address still:
    // what is recorded is every call through it and not only the first.
    let repeating = slot(add) != direct[0];
    if repeating {
        c.ok(trapped, 0)?;
        // Each of those calls answers what it always did, and the slot
        // is the export's again once the records asked for are spent.
        // The bound is what makes a budget that outlives the loop a
        // failing check below rather than a loop that does not end.
        let mut calls = 0;
        while slot(add) != direct[0] && calls < 16 {
            calls += 1;
            // SAFETY: as above, through a slot that is armed again.
            unsafe { c.eq(gala_add(calls, 1), calls + 1)? };
        }
        // Three records (`iat_repeat`'s budget) and a fourth call that
        // finds it spent and repairs the slot -- the first call above
        // having taken none of it. An exact count is this side's whole
        // claim about the sequence: the records themselves are events,
        // which go to the debug console and not the serial log, and
        // what the log carries is the kernel's count of them.
        c.ok(calls == 4, calls)?;
    }

    // And afterwards each slot holds its export's address, so every
    // later call goes straight there: the trace costs the first call
    // through a slot and nothing after it, or the calls the repeat
    // budget above paid for.
    c.ok(slot(add) == direct[0], slot(add) as u64)?;
    c.ok(slot(get) == direct[1], slot(get) as u64)?;
    // SAFETY: as above, now through a slot the kernel has written.
    unsafe {
        c.eq(gala_add(4, 5), 9)?;
    }
    c.ok(slot(value) == find_export(dll, b"GALA_VALUE").unwrap_or(0), slot(value) as u64)?;

    print(match (repeating, trapped) {
        (true, _) => "iat_trace_test: repeating\r\n",
        (false, true) => "iat_trace_test: trapped\r\n",
        (false, false) => "iat_trace_test: direct\r\n",
    });
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("iat_trace_test: ok\r\n");
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
