//! Copy-on-write, with results expected to match real Windows: a
//! PAGE_WRITECOPY view of a mapped file reads the file; a write gives the
//! view a private copy of that page (reported as PAGE_READWRITE) that later
//! file writes no longer reach, while other views, untouched pages and the
//! file itself keep sharing; image views of testdll keep identical code
//! and their writable data pages become private (PAGE_READWRITE) on write.
//! Prints `cow_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use ntrt::*;

static TEST_DLL: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../target/x86_64-pc-windows-msvc/release/testdll.dll"
));

const PAGE_WRITECOPY: u32 = 0x08;
const PAGE_EXECUTE_READ: u32 = 0x20;
const FILE_DISPOSITION_INFORMATION: u32 = 13;
const SYNC: u32 = FILE_SYNCHRONOUS_IO_NONALERT | FILE_NON_DIRECTORY_FILE;
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const RW: u32 = GENERIC_READ | GENERIC_WRITE | DELETE;

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
        print("cow_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn pattern(i: usize) -> u8 {
    (i % 239) as u8
}

fn at(addr: usize) -> u8 {
    // SAFETY: inside a mapped view.
    unsafe { core::ptr::read_volatile(addr as *const u8) }
}

fn put(addr: usize, v: u8) {
    // SAFETY: inside a writable (or copy-on-write) view.
    unsafe { core::ptr::write_volatile(addr as *mut u8, v) }
}

fn file_byte(h: Handle, off: i64) -> u8 {
    let mut b = [0u8; 1];
    read_file(h, &mut b, Some(off));
    b[0]
}

fn protection(addr: usize) -> u64 {
    query(addr).1.protect.into()
}

/// A fresh file at `attrs` holding `data`, opened read/write.
fn new_file(attrs: &ObjectAttributes, data: &[u8]) -> (NtStatus, Handle) {
    let (s, old, _) = open_file(attrs, DELETE, SHARE_ALL, FILE_NON_DIRECTORY_FILE);
    if s == STATUS_SUCCESS {
        set_file(old, FILE_DISPOSITION_INFORMATION, &[1]);
        close(old);
    }
    let (s, h, _) = create_file(attrs, RW | GENERIC_EXECUTE, SHARE_ALL, FILE_CREATE, SYNC);
    if s != STATUS_SUCCESS {
        return (s, h);
    }
    (write_file_info(h, data).0, h)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = u64::from(STATUS_SUCCESS);

    // A 3-page file, one section, a copy-on-write view and a shared view.
    let path = wide!("\\??\\C:\\Users\\Public\\gala_cow_test.dat");
    let path = UnicodeString::new(&path);
    let attrs = ObjectAttributes::new(&path, Handle(0), OBJ_CASE_INSENSITIVE);
    let mut data = [0u8; 0x3000];
    for (i, b) in data.iter_mut().enumerate() {
        *b = pattern(i);
    }
    let (s, file) = new_file(&attrs, &data);
    c.eq(s.into(), ok)?;
    let (s, section) = create_file_section(file, None, PAGE_READWRITE);
    c.eq(s.into(), ok)?;
    let (s, cow, _) = map_view(section, 0, 0, PAGE_WRITECOPY);
    c.eq(s.into(), ok)?;
    let (s, shared, _) = map_view(section, 0, 0, PAGE_READWRITE);
    c.eq(s.into(), ok)?;
    c.eq(at(cow + 5).into(), pattern(5).into())?;
    c.eq(protection(cow), PAGE_WRITECOPY.into())?;

    // A write copies the page for this view only.
    put(cow + 5, b'C');
    c.eq(at(cow + 5).into(), u64::from(b'C'))?;
    c.eq(at(shared + 5).into(), pattern(5).into())?;
    c.eq(file_byte(file, 5).into(), pattern(5).into())?;
    c.eq(protection(cow), PAGE_READWRITE.into())?;
    c.eq(protection(cow + 0x1000), PAGE_WRITECOPY.into())?;
    c.eq(query(cow).1.region_size as u64, 0x1000)?;

    // File writes reach untouched pages, not the private copy.
    c.eq(write_file_at(file, b"F", 6).0.into(), ok)?;
    c.eq(at(shared + 6).into(), u64::from(b'F'))?;
    c.eq(at(cow + 6).into(), pattern(6).into())?;
    c.eq(write_file_at(file, b"G", 0x1006).0.into(), ok)?;
    c.eq(at(cow + 0x1006).into(), u64::from(b'G'))?;
    put(shared + 0x2000, b'S');
    c.eq(at(cow + 0x2000).into(), u64::from(b'S'))?;

    for v in [cow, shared] {
        c.eq(unmap_view(v).into(), ok)?;
    }
    close(section);
    c.eq(set_file(file, FILE_DISPOSITION_INFORMATION, &[1]).into(), ok)?;
    close(file);

    // Image views: shared code, private data after a write.
    let dll_path = wide!("\\??\\C:\\Users\\Public\\gala_cow_test.dll");
    let dll_path = UnicodeString::new(&dll_path);
    let dll_attrs = ObjectAttributes::new(&dll_path, Handle(0), OBJ_CASE_INSENSITIVE);
    let (s, dll) = new_file(&dll_attrs, TEST_DLL);
    c.eq(s.into(), ok)?;
    let (s, image) = create_image_section(dll);
    c.eq(s.into(), ok)?;
    let (s, a, _) = map_view(image, 0, 0, PAGE_READWRITE);
    c.ok(s == STATUS_SUCCESS || s == 0x4000_0003, s.into())?;
    let (s, b, _) = map_view(image, 0, 0, PAGE_READWRITE);
    c.ok(s == STATUS_SUCCESS || s == 0x4000_0003, s.into())?;
    let (Some(add_a), Some(add_b)) = (find_export(a, b"gala_add"), find_export(b, b"gala_add"))
    else {
        return c.ok(false, 0);
    };
    c.eq(protection(add_a), PAGE_EXECUTE_READ.into())?;
    // SAFETY: both point at the same function's code in two views.
    let same = unsafe {
        core::slice::from_raw_parts(add_a as *const u8, 16)
            == core::slice::from_raw_parts(add_b as *const u8, 16)
    };
    c.ok(same, 0)?;
    let (Some(value_a), Some(value_b)) =
        (find_export(a, b"GALA_VALUE"), find_export(b, b"GALA_VALUE"))
    else {
        return c.ok(false, 1);
    };
    c.eq(protection(value_a), PAGE_WRITECOPY.into())?;
    put(value_a, 0x42);
    c.eq(at(value_a).into(), 0x42)?;
    c.ok(at(value_b) != 0x42, at(value_b).into())?;
    c.eq(protection(value_a), PAGE_READWRITE.into())?;
    c.eq(protection(value_b), PAGE_WRITECOPY.into())?;
    for v in [a, b] {
        c.eq(unmap_view(v).into(), ok)?;
    }
    close(image);
    c.eq(set_file(dll, FILE_DISPOSITION_INFORMATION, &[1]).into(), ok)?;
    close(dll);
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("cow_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
