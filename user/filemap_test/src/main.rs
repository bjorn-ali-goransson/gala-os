//! Memory-mapped files (data sections on a file handle) under
//! `\??\C:\Users\Public`, with results expected to match real Windows:
//! the section takes the file's size or extends the file; views show the
//! file and stay coherent with NtReadFile/NtWriteFile; views and sections
//! on one file share pages; access follows the file handle; the file
//! cannot be truncated below a mapped size; data persists after unmapping.
//! Prints `filemap_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use ntrt::*;

const STATUS_ACCESS_DENIED: NtStatus = 0xc000_0022;
const STATUS_USER_MAPPED_FILE: NtStatus = 0xc000_0243;
const FILE_DISPOSITION_INFORMATION: u32 = 13;
const FILE_STANDARD_INFORMATION: u32 = 5;
const FILE_END_OF_FILE_INFORMATION: u32 = 20;
const MEM_MAPPED: u32 = 0x4_0000;
const SIZE: usize = 5000;
const SYNC: u32 = FILE_SYNCHRONOUS_IO_NONALERT | FILE_NON_DIRECTORY_FILE;
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

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
        print("filemap_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn pattern(i: usize) -> u8 {
    (i % 251) as u8
}

fn at(base: usize, off: usize) -> u8 {
    // SAFETY: inside a mapped view.
    unsafe { core::ptr::read_volatile((base + off) as *const u8) }
}

fn put(base: usize, off: usize, v: u8) {
    // SAFETY: inside a writable mapped view.
    unsafe { core::ptr::write_volatile((base + off) as *mut u8, v) }
}

fn file_byte(h: Handle, off: i64) -> u8 {
    let mut b = [0u8; 1];
    read_file(h, &mut b, Some(off));
    b[0]
}

fn end_of_file(h: Handle) -> u64 {
    let mut b = [0u8; 24];
    query_file(h, FILE_STANDARD_INFORMATION, &mut b);
    u64::from_le_bytes(b[8..16].try_into().unwrap())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = u64::from(STATUS_SUCCESS);
    let path = wide!("\\??\\C:\\Users\\Public\\gala_filemap_test.dat");
    let path = UnicodeString::new(&path);
    let attrs = ObjectAttributes::new(&path, Handle(0), OBJ_CASE_INSENSITIVE);

    // A fresh 5000-byte file.
    let (s, old, _) = open_file(&attrs, DELETE, SHARE_ALL, FILE_NON_DIRECTORY_FILE);
    if s == STATUS_SUCCESS {
        set_file(old, FILE_DISPOSITION_INFORMATION, &[1]);
        close(old);
    }
    let rw = GENERIC_READ | GENERIC_WRITE | DELETE;
    let (s, file, _) = create_file(&attrs, rw, SHARE_ALL, FILE_CREATE, SYNC);
    c.eq(s.into(), ok)?;
    let mut data = [0u8; SIZE];
    for (i, b) in data.iter_mut().enumerate() {
        *b = pattern(i);
    }
    c.eq(write_file_info(file, &data).0.into(), ok)?;

    // The section takes the file's size; a view shows the contents.
    let (s, section) = create_file_section(file, None, PAGE_READWRITE);
    c.eq(s.into(), ok)?;
    let (s, _, max) = query_section(section);
    c.eq(s.into(), ok)?;
    c.eq(max, SIZE as u64)?;
    let (s, a, size) = map_view(section, 0, 0, PAGE_READWRITE);
    c.eq(s.into(), ok)?;
    c.eq(size as u64, 0x2000)?;
    c.ok((0..SIZE).all(|i| at(a, i) == pattern(i)), 0)?;
    c.ok((SIZE..0x2000).all(|i| at(a, i) == 0), 0)?;

    // Coherent with file I/O, both ways.
    put(a, 10, b'X');
    c.eq(file_byte(file, 10).into(), u64::from(b'X'))?;
    c.eq(write_file_at(file, b"Y", 20).0.into(), ok)?;
    c.eq(at(a, 20).into(), u64::from(b'Y'))?;

    // Views and sections of one file share pages.
    let (s, b, _) = map_view(section, 0, 0, PAGE_READWRITE);
    c.eq(s.into(), ok)?;
    c.ok(b != a, b as u64)?;
    c.eq(at(b, 10).into(), u64::from(b'X'))?;
    put(b, 30, b'Z');
    c.eq(at(a, 30).into(), u64::from(b'Z'))?;
    let (s, section2) = create_file_section(file, None, PAGE_READONLY);
    c.eq(s.into(), ok)?;
    let (s, v2, _) = map_view(section2, 0, 0, PAGE_READONLY);
    c.eq(s.into(), ok)?;
    c.eq(at(v2, 30).into(), u64::from(b'Z'))?;

    // A mapped view: MEM_MAPPED, named after the file.
    let (s, mbi) = query(a);
    c.eq(s.into(), ok)?;
    c.eq(mbi.kind.into(), MEM_MAPPED.into())?;
    let mut name = [0u8; 600];
    let mapped = mapped_file_name(a, &mut name);
    c.ok(mapped.is_ok_and(|n| ends_with_ci(n, "\\Users\\Public\\gala_filemap_test.dat")), 0)?;

    // No truncation below the mapped size.
    c.eq(
        set_file(file, FILE_END_OF_FILE_INFORMATION, &100u64.to_le_bytes()).into(),
        STATUS_USER_MAPPED_FILE.into(),
    )?;
    c.eq(end_of_file(file), SIZE as u64)?;

    // Section access follows the file handle.
    let (s, ro, _) = open_file(&attrs, GENERIC_READ, SHARE_ALL, SYNC);
    c.eq(s.into(), ok)?;
    let (s, h) = create_file_section(ro, None, PAGE_READONLY);
    c.eq(s.into(), ok)?;
    close(h);
    c.eq(create_file_section(ro, None, PAGE_READWRITE).0.into(), STATUS_ACCESS_DENIED.into())?;
    close(ro);

    // A larger section on a writable handle extends the file.
    let (s, big) = create_file_section(file, Some(0x3000), PAGE_READWRITE);
    c.eq(s.into(), ok)?;
    c.eq(end_of_file(file), 0x3000)?;
    let (s, v3, size) = map_view(big, 0, 0, PAGE_READWRITE);
    c.eq(s.into(), ok)?;
    c.eq(size as u64, 0x3000)?;
    c.eq(at(v3, 30).into(), u64::from(b'Z'))?;
    put(v3, 0x2ff0, b'E');
    c.eq(file_byte(file, 0x2ff0).into(), u64::from(b'E'))?;

    // Unmapped and closed: the data stays in the file.
    for v in [a, b, v2, v3] {
        c.eq(unmap_view(v).into(), ok)?;
    }
    for h in [section, section2, big] {
        close(h);
    }
    c.eq(file_byte(file, 10).into(), u64::from(b'X'))?;
    c.eq(file_byte(file, 20).into(), u64::from(b'Y'))?;
    c.eq(file_byte(file, 30).into(), u64::from(b'Z'))?;
    c.eq(set_file(file, FILE_DISPOSITION_INFORMATION, &[1]).into(), ok)?;
    close(file);
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("filemap_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
