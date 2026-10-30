//! An executable with imports: functions and data from testdll.dll, bound
//! by the user-mode loader (ntdll) before our entry point runs — on Windows
//! when testdll.dll sits next to the exe, on gala through its test ntdll.
//! Checks the imported calls and data, that the IAT entries point into a
//! MEM_IMAGE mapping of a file named testdll.dll, and that they are that
//! DLL's exports. Prints `import_test: ok` or exits with the number of the
//! failing check.

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
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("import_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
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

/// Our imports from `dll`: (name, bound address) for each by-name import.
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
                    out[n] = (c_str(image + entry as usize + 2), u64_at(iat + 8 * i) as usize);
                    n += 1;
                }
                i += 1;
            }
        }
        desc += 20;
    }
    n
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    // SAFETY: imported from testdll, bound by the loader.
    unsafe {
        c.eq(gala_add(2, 3), 5)?;
        c.eq(gala_get_value(), INITIAL_VALUE)?;
        c.eq(core::ptr::read_volatile(&raw const GALA_VALUE), INITIAL_VALUE)?;
    }
    let mut imports = [(&[][..], 0usize); 8];
    let n = imports_from(b"testdll.dll", &mut imports);
    c.eq(n as u64, 3)?;
    let (s, mbi) = query(imports[0].1);
    c.eq(s.into(), STATUS_SUCCESS.into())?;
    c.eq(mbi.kind.into(), MEM_IMAGE.into())?;
    let dll = mbi.allocation_base;
    let mut buf = [0u8; 600];
    c.ok(mapped_file_name(dll, &mut buf).is_ok_and(|name| ends_with_ci(name, "\\testdll.dll")), 0)?;
    c.ok(export_dll_name(dll).is_some_and(|name| name.eq_ignore_ascii_case(b"testdll.dll")), 0)?;
    for &(name, bound) in &imports[..n] {
        c.ok(find_export(dll, name) == Some(bound), bound as u64)?;
    }
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("import_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
