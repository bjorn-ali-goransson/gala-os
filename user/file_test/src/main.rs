//! File I/O through NtCreateFile & co. on `\??\C:\Users\Public` (writable for
//! normal users on Windows), with results expected to match real Windows.
//! Prints `file_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use ntrt::*;

const STATUS_END_OF_FILE: NtStatus = 0xc000_0011;
const STATUS_ACCESS_DENIED: NtStatus = 0xc000_0022;
const STATUS_OBJECT_NAME_NOT_FOUND: NtStatus = 0xc000_0034;
const STATUS_OBJECT_NAME_COLLISION: NtStatus = 0xc000_0035;
const STATUS_OBJECT_PATH_NOT_FOUND: NtStatus = 0xc000_003a;
const STATUS_SHARING_VIOLATION: NtStatus = 0xc000_0043;
const STATUS_FILE_IS_A_DIRECTORY: NtStatus = 0xc000_00ba;
const STATUS_NOT_A_DIRECTORY: NtStatus = 0xc000_0103;
const FILE_CREATED: u64 = 2;
const FILE_OPENED: u64 = 1;
const FILE_OVERWRITTEN: u64 = 3;
const FILE_DISPOSITION_INFORMATION: u32 = 13;
const FILE_POSITION_INFORMATION: u32 = 14;
const FILE_STANDARD_INFORMATION: u32 = 5;
const FILE_END_OF_FILE_INFORMATION: u32 = 20;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x20;
const SYNC: u32 = FILE_SYNCHRONOUS_IO_NONALERT | FILE_NON_DIRECTORY_FILE;
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
        print("file_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn st(s: NtStatus) -> u64 {
    s.into()
}

fn u64_at(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}

fn position(h: Handle) -> u64 {
    let mut b = [0u8; 8];
    query_file(h, FILE_POSITION_INFORMATION, &mut b);
    u64_at(&b, 0)
}

/// (EndOfFile, Directory)
fn standard(h: Handle) -> (u64, u8, u32) {
    let mut b = [0u8; 24];
    query_file(h, FILE_STANDARD_INFORMATION, &mut b);
    (u64_at(&b, 8), b[21], u32::from_le_bytes(b[16..20].try_into().unwrap()))
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = st(STATUS_SUCCESS);
    let path = wide!("\\??\\C:\\Users\\Public\\gala_file_test.txt");
    let path = UnicodeString::new(&path);
    let file = ObjectAttributes::new(&path, Handle(0), OBJ_CASE_INSENSITIVE);

    // Clean up after an earlier failed run (ignore errors).
    let (s, old, _) = open_file(&file, DELETE, FILE_SHARE_DELETE, FILE_NON_DIRECTORY_FILE);
    if s == STATUS_SUCCESS {
        set_file(old, FILE_DISPOSITION_INFORMATION, &[1]);
        close(old);
    }

    // Names that do not exist.
    c.eq(st(open_file(&file, GENERIC_READ, 0, SYNC).0), st(STATUS_OBJECT_NAME_NOT_FOUND))?;
    let bad = wide!("\\??\\C:\\gala_no_such_dir\\x.txt");
    let bad = UnicodeString::new(&bad);
    let bad = ObjectAttributes::new(&bad, Handle(0), OBJ_CASE_INSENSITIVE);
    c.eq(st(open_file(&bad, GENERIC_READ, 0, SYNC).0), st(STATUS_OBJECT_PATH_NOT_FOUND))?;

    // Create, write, read back.
    let (s, h, info) = create_file(&file, RW, 0, FILE_CREATE, SYNC);
    c.eq(st(s), ok)?;
    c.eq(info as u64, FILE_CREATED)?;
    let (s, n) = write_file_info(h, b"hello file\n");
    c.eq(st(s), ok)?;
    c.eq(n as u64, 11)?;
    c.eq(position(h), 11)?;
    c.eq(standard(h).0, 11)?;
    c.eq(u64::from(standard(h).1), 0)?;
    c.eq(u64::from(standard(h).2), 1)?; // NumberOfLinks
    c.eq(st(set_file(h, FILE_POSITION_INFORMATION, &0u64.to_le_bytes())), ok)?;
    let mut buf = [0u8; 64];
    let (s, n) = read_file(h, &mut buf, None);
    c.eq(st(s), ok)?;
    c.eq(n as u64, 11)?;
    c.ok(&buf[..11] == b"hello file\n", 0)?;
    c.eq(st(read_file(h, &mut buf, None).0), st(STATUS_END_OF_FILE))?;
    let (s, n) = read_file(h, &mut buf, Some(6));
    c.eq(st(s), ok)?;
    c.eq(n as u64, 5)?;
    c.ok(&buf[..5] == b"file\n", 0)?;
    c.eq(position(h), 11)?;
    // Append at end of file.
    let (s, n) = write_file_at(h, b"more", -1);
    c.eq(st(s), ok)?;
    c.eq(n as u64, 4)?;
    c.eq(standard(h).0, 15)?;

    // Collisions and sharing.
    c.eq(st(create_file(&file, RW, 0, FILE_CREATE, SYNC).0), st(STATUS_OBJECT_NAME_COLLISION))?;
    c.eq(
        st(open_file(&file, GENERIC_READ, FILE_SHARE_READ, SYNC).0),
        st(STATUS_SHARING_VIOLATION),
    )?;
    close(h);

    // Read-only open; writing is denied.
    let (s, r, info) = open_file(&file, GENERIC_READ, FILE_SHARE_READ, SYNC);
    c.eq(st(s), ok)?;
    c.eq(info as u64, FILE_OPENED)?;
    c.eq(st(write_file_info(r, b"x").0), st(STATUS_ACCESS_DENIED))?;
    let (s, n) = read_file(r, &mut buf, None);
    c.eq(st(s), ok)?;
    c.eq(n as u64, 15)?;
    c.ok(&buf[..15] == b"hello file\nmore", 0)?;
    close(r);

    let (s, attributes) = query_attributes(&file);
    c.eq(st(s), ok)?;
    c.eq(attributes.into(), FILE_ATTRIBUTE_ARCHIVE.into())?;

    // Overwrite, extend, delete on close.
    let (s, h, info) = create_file(&file, RW, 0, FILE_OVERWRITE_IF, SYNC);
    c.eq(st(s), ok)?;
    c.eq(info as u64, FILE_OVERWRITTEN)?;
    c.eq(standard(h).0, 0)?;
    c.eq(st(set_file(h, FILE_END_OF_FILE_INFORMATION, &100u64.to_le_bytes())), ok)?;
    c.eq(standard(h).0, 100)?;
    let (s, n) = read_file(h, &mut buf, Some(90));
    c.eq(st(s), ok)?;
    c.eq(n as u64, 10)?;
    c.ok(buf[..10].iter().all(|&b| b == 0), 0)?;
    c.eq(st(set_file(h, FILE_DISPOSITION_INFORMATION, &[1])), ok)?;
    close(h);
    c.eq(st(open_file(&file, GENERIC_READ, 0, SYNC).0), st(STATUS_OBJECT_NAME_NOT_FOUND))?;

    // Directories.
    let dir = wide!("\\??\\C:\\Users\\Public");
    let dir = UnicodeString::new(&dir);
    let dir = ObjectAttributes::new(&dir, Handle(0), OBJ_CASE_INSENSITIVE);
    let (s, d, _) =
        open_file(&dir, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, FILE_DIRECTORY_FILE);
    c.eq(st(s), ok)?;
    close(d);
    let (s, attributes) = query_attributes(&dir);
    c.eq(st(s), ok)?;
    c.ok(attributes & FILE_ATTRIBUTE_DIRECTORY != 0, attributes.into())?;
    c.eq(
        st(open_file(&dir, GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, SYNC).0),
        st(STATUS_FILE_IS_A_DIRECTORY),
    )?;
    let (_, h, _) = create_file(&file, RW, 0, FILE_CREATE, SYNC);
    close(h);
    c.eq(st(open_file(&file, GENERIC_READ, 0, FILE_DIRECTORY_FILE).0), st(STATUS_NOT_A_DIRECTORY))?;
    let (_, h, _) = open_file(&file, DELETE, 0, FILE_NON_DIRECTORY_FILE);
    set_file(h, FILE_DISPOSITION_INFORMATION, &[1]);
    close(h);
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("file_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
