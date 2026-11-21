//! `imports=` records the module a call came from, and not only the
//! import it was a call to (docs/OBSERVABILITY.md, "Import tracing").
//! One import is a slot in every image that imports it, so a record
//! that names the import alone cannot say who called: in a trace of a
//! whole program the same name is reached from several of its DLLs.
//!
//! This program imports `gala_add` from testdll.dll, and `gala2_add3`
//! and `gala2_get_value` from testdll2.dll -- which imports `gala_add`
//! and `gala_get_value` from testdll.dll in turn. So arming the
//! program's imports from testdll.dll arms three slots in two images,
//! two of them for one name: `testdll.dll!gala_add` is a slot of this
//! program's and a slot of testdll2's, and the two calls through them
//! are the same call under every field an `import` event carried before
//! the importing module was one of them.
//!
//! What is visible from this side is the slots. Every one of those
//! three is armed before the first call through it -- an address in a
//! reserved region, not the export's -- and holds the export's address
//! after it, which is the claim that the trace reached a DLL's own
//! imports and not only the program's. The imports from testdll2.dll
//! are not armed at all, the option naming testdll.dll as the
//! supplier, so this program's slot for `gala2_add3` is testdll2's
//! export throughout and is what finds that image.
//!
//! Which image a slot is in is a question of its own, and `imports_in=`
//! is how it is asked: `args=inner` expects a run that armed only the
//! slots in testdll2.dll. Then the same import of the same name from
//! the same DLL is armed in testdll2's image and left alone in this
//! program's, which no filter on the import's name could tell apart --
//! and the call through the unarmed slot answers as it always did, so
//! the narrowing costs the program nothing.
//!
//! The records themselves are `import` events, which the debug console
//! carries and the serial log does not; what the log carries is the
//! kernel's count of the slots it armed and of the images they are in.
//!
//! Prints `iat_from_test: trapped` (on gala, with `imports=`; under
//! `args=inner`, with the slots in testdll2.dll armed) or
//! `: direct` (on Windows, or with no option), then
//! `iat_from_test: ok` -- or exits with the number of the failing
//! check.

#![no_std]
#![no_main]

use ntrt::*;

#[link(name = "testdll", kind = "raw-dylib")]
unsafe extern "C" {
    fn gala_add(a: u64, b: u64) -> u64;
}

#[link(name = "testdll2", kind = "raw-dylib")]
unsafe extern "C" {
    fn gala2_add3(a: u64, b: u64, c: u64) -> u64;
    fn gala2_get_value() -> u64;
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
        print("iat_from_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        check_line(core::panic::Location::caller().line());
        print("\r\n");
        Err(self.0)
    }
}

fn u32_at(addr: usize) -> u32 {
    // SAFETY: callers read inside a mapped image.
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
    // SAFETY: the address of an IAT entry of a mapped image.
    unsafe { core::ptr::read_volatile(addr as *const usize) }
}

/// A NUL-terminated string in a mapped image.
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

/// The by-name imports `image` takes from `dll`: (name, the address of
/// its IAT slot) for each. `image` is any mapped image, this program's
/// or a DLL's, which is what makes a DLL's own slots readable from
/// here.
fn imports_from(image: usize, dll: &[u8], out: &mut [(&'static [u8], usize); 8]) -> usize {
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

/// Whether `at` is in a region that is reserved and never committed,
/// which is what an armed slot holds the address of.
fn reserved(c: &mut Checks, at: usize) -> Result<bool, u32> {
    let (s, mbi) = query(at);
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    Ok(mbi.state == MEM_RESERVE)
}

/// Whether the command line's last word is `word`.
fn last_arg_is(word: &str) -> bool {
    // SAFETY: PEB+0x20 is ProcessParameters; its CommandLine is a
    // UNICODE_STRING at +0x70.
    let cmd = unsafe {
        let pp = *(peb().add(0x20) as *const *const u8);
        let len = usize::from(*(pp.add(0x70) as *const u16)) / 2;
        core::slice::from_raw_parts(*(pp.add(0x78) as *const *const u16), len)
    };
    let cmd = match cmd.iter().rposition(|&ch| ch != u16::from(b' ')) {
        Some(end) => &cmd[..=end],
        None => return false,
    };
    let word = word.as_bytes();
    cmd.len() > word.len()
        && cmd[cmd.len() - word.len() - 1] == u16::from(b' ')
        && cmd[cmd.len() - word.len()..].iter().zip(word).all(|(&a, &b)| a == u16::from(b))
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let image = &raw const __ImageBase as usize;
    // `args=inner`: the run armed the slots in testdll2.dll alone
    // (`imports_in=`), so this program's own slot for the same import
    // is the export's address throughout.
    let inner_only = last_arg_is("inner");

    // Our own imports: one from testdll.dll, which the trace arms, and
    // two from testdll2.dll, which it does not.
    let mut ours = [(&[][..], 0usize); 8];
    let n = imports_from(image, b"testdll.dll", &mut ours);
    c.eq(n as u64, 1)?;
    let add = named(&ours, n, b"gala_add");
    c.ok(add != 0, 0)?;
    let mut theirs = [(&[][..], 0usize); 8];
    let n2 = imports_from(image, b"testdll2.dll", &mut theirs);
    c.eq(n2 as u64, 2)?;
    let add3 = named(&theirs, n2, b"gala2_add3");
    c.ok(add3 != 0, 0)?;

    // testdll2's image, found through the slot that is never armed
    // here: the option names testdll.dll as the supplier, so an import
    // of testdll2's own exports holds that export's address throughout.
    let (s, mbi) = query(slot(add3));
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.eq(mbi.kind.into(), MEM_IMAGE.into())?;
    let dll2 = mbi.allocation_base;
    c.ok(export_dll_name(dll2).is_some_and(|name| name.eq_ignore_ascii_case(b"testdll2.dll")), 0)?;

    // And testdll2's own imports from testdll.dll, which are slots of
    // its image and not of ours.
    let mut inner = [(&[][..], 0usize); 8];
    let n3 = imports_from(dll2, b"testdll.dll", &mut inner);
    c.eq(n3 as u64, 2)?;
    let (inner_add, inner_get) =
        (named(&inner, n3, b"gala_add"), named(&inner, n3, b"gala_get_value"));
    c.ok(inner_add != 0 && inner_get != 0, 0)?;

    // Before the first call through them, either every slot of an
    // import from testdll.dll is armed or none is -- in both images at
    // once, which is the whole point: a trace of this program's
    // imports is a trace of its DLLs' imports too.
    let trapped = reserved(&mut c, slot(add))?;
    let (armed_add, armed_get) =
        (reserved(&mut c, slot(inner_add))?, reserved(&mut c, slot(inner_get))?);
    let traced = if inner_only {
        // Narrowed to testdll2's image: our own slot for
        // `testdll.dll!gala_add` is never armed, though it is an import
        // of the same name from the same DLL as one of testdll2's two.
        // On Windows nothing is armed at all, which is the same claim
        // about this slot and why the check is not conditional.
        c.eq(trapped as u64, 0)?;
        c.eq(armed_get as u64, armed_add as u64)?;
        armed_add
    } else {
        c.eq(armed_add as u64, trapped as u64)?;
        c.eq(armed_get as u64, trapped as u64)?;
        trapped
    };

    // The calls answer what they always did, armed or not: ours
    // straight into testdll, and testdll2's two from inside it.
    // SAFETY: imported from testdll and testdll2, bound by the loader.
    unsafe {
        c.eq(gala_add(2, 3), 5)?;
        c.eq(gala2_add3(1, 2, 3), 6)?;
        c.eq(gala2_get_value(), INITIAL_VALUE)?;
    }

    // And afterwards every one of those slots holds its export's
    // address, in both images: testdll's, which is the image our own
    // repaired slot now points into.
    let (s, mbi) = query(slot(add));
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.eq(mbi.kind.into(), MEM_IMAGE.into())?;
    let dll = mbi.allocation_base;
    c.ok(export_dll_name(dll).is_some_and(|name| name.eq_ignore_ascii_case(b"testdll.dll")), 0)?;
    let exports = [
        find_export(dll, b"gala_add").unwrap_or(0),
        find_export(dll, b"gala_get_value").unwrap_or(0),
    ];
    c.ok(exports[0] != 0 && exports[1] != 0, 0)?;
    c.eq(slot(add) as u64, exports[0] as u64)?;
    c.eq(slot(inner_add) as u64, exports[0] as u64)?;
    c.eq(slot(inner_get) as u64, exports[1] as u64)?;

    print(if traced { "iat_from_test: trapped\r\n" } else { "iat_from_test: direct\r\n" });
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("iat_from_test: ok\r\n");
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
