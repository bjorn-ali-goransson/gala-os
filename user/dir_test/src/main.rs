//! Directory listing, volume information, flush, rename, delete on close and
//! relative opens (RootDirectory = directory handle) under
//! `\??\C:\Users\Public\gala_dir_test`, with results expected to match real
//! Windows (NTFS). Prints `dir_test: ok` or exits with the number of the
//! failing check.

#![no_std]
#![no_main]

use ntrt::*;

const STATUS_NO_MORE_FILES: NtStatus = 0x8000_0006;
const STATUS_INFO_LENGTH_MISMATCH: NtStatus = 0xc000_0004;
const STATUS_NO_SUCH_FILE: NtStatus = 0xc000_000f;
const STATUS_INVALID_PARAMETER: NtStatus = 0xc000_000d;
const STATUS_ACCESS_DENIED: NtStatus = 0xc000_0022;
const STATUS_OBJECT_NAME_NOT_FOUND: NtStatus = 0xc000_0034;
const STATUS_OBJECT_NAME_COLLISION: NtStatus = 0xc000_0035;
const STATUS_DIRECTORY_NOT_EMPTY: NtStatus = 0xc000_0101;

const FILE_DIRECTORY_INFORMATION: u32 = 1;
const FILE_BOTH_DIRECTORY_INFORMATION: u32 = 3;
const FILE_NAMES_INFORMATION: u32 = 12;
const FILE_RENAME_INFORMATION: u32 = 10;
const FILE_DISPOSITION_INFORMATION: u32 = 13;
const FILE_STANDARD_INFORMATION: u32 = 5;
const FILE_FS_VOLUME_INFORMATION: u32 = 1;
const FILE_FS_SIZE_INFORMATION: u32 = 3;
const FILE_FS_DEVICE_INFORMATION: u32 = 4;
const FILE_FS_ATTRIBUTE_INFORMATION: u32 = 5;
const FILE_DEVICE_DISK: u32 = 7;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x20;

const SYNC: u32 = FILE_SYNCHRONOUS_IO_NONALERT;
const FILE: u32 = SYNC | FILE_NON_DIRECTORY_FILE;
const DIR: u32 = SYNC | FILE_DIRECTORY_FILE;
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
        print("dir_test: check ");
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

fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}

/// Whether the UTF-16LE bytes `name` spell the ASCII string `want`.
fn is(name: &[u8], want: &str) -> bool {
    name.len() == want.len() * 2
        && name.chunks(2).zip(want.bytes()).all(|(c, w)| c[0] == w && c[1] == 0)
}

/// Layout of a directory information class: (FileNameLength, FileName).
fn name_fields(class: u32) -> (usize, usize) {
    match class {
        FILE_NAMES_INFORMATION => (8, 12),
        FILE_BOTH_DIRECTORY_INFORMATION => (60, 94),
        _ => (60, 64),
    }
}

/// The entries of a directory information buffer as (offset, name).
fn entries(buf: &[u8], class: u32) -> impl Iterator<Item = (usize, &[u8])> {
    let (len_at, name_at) = name_fields(class);
    let mut next = Some(0usize);
    core::iter::from_fn(move || {
        let off = next?;
        let step = u32_at(buf, off) as usize;
        next = if step == 0 { None } else { Some(off + step) };
        let len = u32_at(buf, off + len_at) as usize;
        Some((off, &buf[off + name_at..off + name_at + len]))
    })
}

/// Queries one entry and returns whether its name is `want`.
fn next_name(h: Handle, buf: &mut [u8], mask: Option<&UnicodeString>, want: &str) -> bool {
    let (s, _) = query_directory(h, buf, FILE_NAMES_INFORMATION, true, mask, false);
    s == STATUS_SUCCESS
        && entries(buf, FILE_NAMES_INFORMATION).next().is_some_and(|e| is(e.1, want))
}

fn delete(root: Handle, name: &[u16], options: u32) {
    let name = UnicodeString::new(name);
    let attrs = ObjectAttributes::new(&name, root, OBJ_CASE_INSENSITIVE);
    let (s, h, _) = open_file(&attrs, DELETE, SHARE_ALL, options);
    if s == STATUS_SUCCESS {
        set_file(h, FILE_DISPOSITION_INFORMATION, &[1]);
        close(h);
    }
}

/// Removes what an earlier failed run left behind (errors ignored).
fn clean(dir: Handle) {
    delete(dir, &wide!("sub\\e.txt"), FILE);
    let names: [&[u16]; 5] =
        [&wide!("A.dat"), &wide!("b.txt"), &wide!("c.txt"), &wide!("d.txt"), &wide!("tmp.txt")];
    for name in names {
        delete(dir, name, FILE);
    }
    delete(dir, &wide!("sub"), DIR);
}

/// Creates `name` in `dir` with `data` and closes it.
fn make(dir: Handle, name: &[u16], data: &[u8], options: u32) -> NtStatus {
    let name = UnicodeString::new(name);
    let attrs = ObjectAttributes::new(&name, dir, OBJ_CASE_INSENSITIVE);
    let (s, h, _) = create_file(&attrs, RW, 0, FILE_CREATE, options);
    if s == STATUS_SUCCESS {
        if !data.is_empty() {
            write_file_info(h, data);
        }
        close(h);
    }
    s
}

fn open_in(dir: Handle, name: &[u16], access: u32, options: u32) -> (NtStatus, Handle) {
    let name = UnicodeString::new(name);
    let attrs = ObjectAttributes::new(&name, dir, OBJ_CASE_INSENSITIVE);
    let (s, h, _) = open_file(&attrs, access, SHARE_ALL, options);
    (s, h)
}

/// FILE_RENAME_INFORMATION for `name` (no root directory).
fn rename(h: Handle, name: &[u16], replace: bool) -> NtStatus {
    let mut info = [0u8; 20 + 2 * 64];
    info[0] = u8::from(replace);
    info[16..20].copy_from_slice(&((name.len() * 2) as u32).to_le_bytes());
    for (i, c) in name.iter().enumerate() {
        info[20 + 2 * i..22 + 2 * i].copy_from_slice(&c.to_le_bytes());
    }
    set_file(h, FILE_RENAME_INFORMATION, &info[..20 + name.len() * 2])
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = st(STATUS_SUCCESS);
    let path = wide!("\\??\\C:\\Users\\Public\\gala_dir_test");
    let path = UnicodeString::new(&path);
    let dir_attrs = ObjectAttributes::new(&path, Handle(0), OBJ_CASE_INSENSITIVE);
    let mut buf = [0u8; 4096];

    let (s, dir, _) = create_file(&dir_attrs, GENERIC_READ | DELETE, SHARE_ALL, FILE_OPEN_IF, DIR);
    c.eq(st(s), ok)?;
    clean(dir);

    // Children created relative to the directory handle.
    c.eq(st(make(dir, &wide!("b.txt"), b"hello", FILE)), ok)?;
    c.eq(st(make(dir, &wide!("A.dat"), b"", FILE)), ok)?;
    c.eq(st(make(dir, &wide!("c.txt"), b"", FILE)), ok)?;
    c.eq(st(make(dir, &wide!("sub"), b"", DIR)), ok)?;

    // Full listing in one call, sorted, with "." and "..".
    let (s, info) = query_directory(dir, &mut buf, FILE_DIRECTORY_INFORMATION, false, None, true);
    c.eq(st(s), ok)?;
    c.ok(info > 64 && info <= buf.len(), info as u64)?;
    let want = [".", "..", "A.dat", "b.txt", "c.txt", "sub"];
    let mut n = 0;
    for (i, (off, name)) in entries(&buf, FILE_DIRECTORY_INFORMATION).enumerate() {
        c.ok(i < want.len() && is(name, want[i]), i as u64)?;
        c.eq(u64::from(off % 8 == 0), 1)?;
        let attributes = u32_at(&buf, off + 56);
        match want[i] {
            "b.txt" => {
                c.eq(u64_at(&buf, off + 40), 5)?; // EndOfFile
                c.eq(
                    u64::from(attributes & FILE_ATTRIBUTE_ARCHIVE),
                    FILE_ATTRIBUTE_ARCHIVE.into(),
                )?;
            }
            "sub" | "." | ".." => c.eq(
                u64::from(attributes & FILE_ATTRIBUTE_DIRECTORY),
                FILE_ATTRIBUTE_DIRECTORY.into(),
            )?,
            _ => {}
        }
        n += 1;
    }
    c.eq(n, want.len() as u64)?;
    c.eq(
        st(query_directory(dir, &mut buf, FILE_DIRECTORY_INFORMATION, false, None, false).0),
        st(STATUS_NO_MORE_FILES),
    )?;

    // Restart, one entry at a time, another class.
    let (s, _) = query_directory(dir, &mut buf, FILE_NAMES_INFORMATION, true, None, true);
    c.eq(st(s), ok)?;
    c.ok(entries(&buf, FILE_NAMES_INFORMATION).next().is_some_and(|e| is(e.1, ".")), 0)?;
    c.eq(u64::from(u32_at(&buf, 0)), 0)?; // single entry: NextEntryOffset 0
    c.ok(next_name(dir, &mut buf, None, ".."), 0)?;
    c.ok(next_name(dir, &mut buf, None, "A.dat"), 0)?;
    let (s, _) = query_directory(dir, &mut buf, FILE_BOTH_DIRECTORY_INFORMATION, true, None, false);
    c.eq(st(s), ok)?;
    c.ok(
        entries(&buf, FILE_BOTH_DIRECTORY_INFORMATION).next().is_some_and(|e| is(e.1, "b.txt")),
        0,
    )?;
    c.eq(
        st(query_directory(dir, &mut buf[..8], FILE_DIRECTORY_INFORMATION, false, None, true).0),
        st(STATUS_INFO_LENGTH_MISMATCH),
    )?;

    // Masks (fixed by the first query on a handle).
    let mask = wide!("*.txt");
    let mask = UnicodeString::new(&mask);
    let (s, h, _) = open_file(&dir_attrs, GENERIC_READ, SHARE_ALL, DIR);
    c.eq(st(s), ok)?;
    c.ok(next_name(h, &mut buf, Some(&mask), "b.txt"), 0)?;
    c.ok(next_name(h, &mut buf, None, "c.txt"), 0)?;
    c.eq(
        st(query_directory(h, &mut buf, FILE_NAMES_INFORMATION, true, None, false).0),
        st(STATUS_NO_MORE_FILES),
    )?;
    close(h);
    let none = wide!("*.none");
    let none = UnicodeString::new(&none);
    let (_, h, _) = open_file(&dir_attrs, GENERIC_READ, SHARE_ALL, DIR);
    c.eq(
        st(query_directory(h, &mut buf, FILE_NAMES_INFORMATION, false, Some(&none), false).0),
        st(STATUS_NO_SUCH_FILE),
    )?;
    close(h);
    let exact = wide!("B.TXT");
    let exact = UnicodeString::new(&exact);
    let (_, h, _) = open_file(&dir_attrs, GENERIC_READ, SHARE_ALL, DIR);
    c.ok(next_name(h, &mut buf, Some(&exact), "b.txt"), 0)?;
    c.eq(
        st(query_directory(h, &mut buf, FILE_NAMES_INFORMATION, true, None, false).0),
        st(STATUS_NO_MORE_FILES),
    )?;
    close(h);
    let single = wide!("?.dat");
    let single = UnicodeString::new(&single);
    let (_, h, _) = open_file(&dir_attrs, GENERIC_READ, SHARE_ALL, DIR);
    c.ok(next_name(h, &mut buf, Some(&single), "A.dat"), 0)?;
    close(h);

    // Relative open and read; listing a file is invalid.
    let (s, b) = open_in(dir, &wide!("b.txt"), GENERIC_READ, FILE);
    c.eq(st(s), ok)?;
    let mut data = [0u8; 16];
    let (s, n) = read_file(b, &mut data, None);
    c.eq(st(s), ok)?;
    c.ok(n == 5 && &data[..5] == b"hello", n as u64)?;
    c.eq(
        st(query_directory(b, &mut buf, FILE_DIRECTORY_INFORMATION, false, None, true).0),
        st(STATUS_INVALID_PARAMETER),
    )?;

    // Flush needs write access.
    c.eq(st(flush_file(b)), st(STATUS_ACCESS_DENIED))?;
    let (s, w) = open_in(dir, &wide!("b.txt"), GENERIC_WRITE, FILE);
    c.eq(st(s), ok)?;
    c.eq(st(flush_file(w)), ok)?;
    close(w);

    // Volume information.
    let mut v = [0u8; 128];
    let (s, info) = query_volume(b, FILE_FS_DEVICE_INFORMATION, &mut v);
    c.eq(st(s), ok)?;
    c.eq(info as u64, 8)?;
    c.eq(u32_at(&v, 0).into(), FILE_DEVICE_DISK.into())?;
    c.eq(
        st(query_volume(b, FILE_FS_DEVICE_INFORMATION, &mut v[..4]).0),
        st(STATUS_INFO_LENGTH_MISMATCH),
    )?;
    let (s, info) = query_volume(b, FILE_FS_ATTRIBUTE_INFORMATION, &mut v);
    c.eq(st(s), ok)?;
    c.eq(u32_at(&v, 4).into(), 255)?; // MaximumComponentNameLength
    c.ok(is(&v[12..12 + u32_at(&v, 8) as usize], "NTFS"), u32_at(&v, 8).into())?;
    c.eq(info as u64, 20)?;
    let (s, info) = query_volume(b, FILE_FS_SIZE_INFORMATION, &mut v);
    c.eq(st(s), ok)?;
    c.eq(info as u64, 24)?;
    let (total, free) = (u64_at(&v, 0), u64_at(&v, 8));
    c.ok(total > 0 && free <= total, free)?;
    c.eq(u64::from(u32_at(&v, 16)) * u64::from(u32_at(&v, 20)), 4096)?; // cluster size
    let (s, _) = query_volume(b, FILE_FS_VOLUME_INFORMATION, &mut v);
    c.eq(st(s), ok)?;
    c.eq(u64::from(u32_at(&v, 12) % 2), 0)?; // VolumeLabelLength in bytes
    close(b);

    // Rename within the directory, collisions, replace, move.
    let (s, h) = open_in(dir, &wide!("c.txt"), GENERIC_READ, FILE);
    c.eq(st(s), ok)?;
    c.eq(st(rename(h, &wide!("d.txt"), false)), st(STATUS_ACCESS_DENIED))?;
    close(h);
    let (_, h) = open_in(dir, &wide!("c.txt"), DELETE, FILE);
    c.eq(st(rename(h, &wide!("d.txt"), false)), ok)?;
    close(h);
    c.eq(
        st(open_in(dir, &wide!("c.txt"), GENERIC_READ, FILE).0),
        st(STATUS_OBJECT_NAME_NOT_FOUND),
    )?;
    let (s, h) = open_in(dir, &wide!("d.txt"), GENERIC_READ, FILE);
    c.eq(st(s), ok)?;
    close(h);
    let (_, h) = open_in(dir, &wide!("A.dat"), DELETE, FILE);
    c.eq(st(rename(h, &wide!("b.txt"), false)), st(STATUS_OBJECT_NAME_COLLISION))?;
    c.eq(st(rename(h, &wide!("b.txt"), true)), ok)?;
    close(h);
    let (s, h) = open_in(dir, &wide!("b.txt"), GENERIC_READ, FILE);
    c.eq(st(s), ok)?;
    let mut sinfo = [0u8; 24];
    query_file(h, FILE_STANDARD_INFORMATION, &mut sinfo);
    c.eq(u64_at(&sinfo, 8), 0)?; // the former A.dat, empty
    close(h);
    c.eq(
        st(open_in(dir, &wide!("A.dat"), GENERIC_READ, FILE).0),
        st(STATUS_OBJECT_NAME_NOT_FOUND),
    )?;
    let (_, h) = open_in(dir, &wide!("d.txt"), DELETE, FILE);
    c.eq(st(rename(h, &wide!("\\??\\C:\\Users\\Public\\gala_dir_test\\sub\\e.txt"), false)), ok)?;
    close(h);
    let (s, h) = open_in(dir, &wide!("sub\\e.txt"), GENERIC_READ, FILE);
    c.eq(st(s), ok)?;
    close(h);

    // FILE_DELETE_ON_CLOSE needs DELETE access; the file goes on close.
    let tmp = wide!("tmp.txt");
    let tmp = UnicodeString::new(&tmp);
    let tmp = ObjectAttributes::new(&tmp, dir, OBJ_CASE_INSENSITIVE);
    c.eq(
        st(create_file(&tmp, GENERIC_WRITE, 0, FILE_CREATE, FILE | FILE_DELETE_ON_CLOSE).0),
        st(STATUS_INVALID_PARAMETER),
    )?;
    let (s, h, _) = create_file(&tmp, RW, SHARE_ALL, FILE_CREATE, FILE | FILE_DELETE_ON_CLOSE);
    c.eq(st(s), ok)?;
    let (s, h2, _) = open_file(&tmp, GENERIC_READ, SHARE_ALL, FILE);
    c.eq(st(s), ok)?; // not delete-pending until the handle closes
    close(h);
    close(h2);
    c.eq(st(open_file(&tmp, GENERIC_READ, SHARE_ALL, FILE).0), st(STATUS_OBJECT_NAME_NOT_FOUND))?;

    // A non-empty directory cannot be deleted.
    c.eq(st(set_file(dir, FILE_DISPOSITION_INFORMATION, &[1])), st(STATUS_DIRECTORY_NOT_EMPTY))?;
    clean(dir);
    c.eq(st(set_file(dir, FILE_DISPOSITION_INFORMATION, &[1])), ok)?;
    close(dir);
    c.eq(
        st(open_file(&dir_attrs, GENERIC_READ, SHARE_ALL, DIR).0),
        st(STATUS_OBJECT_NAME_NOT_FOUND),
    )?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("dir_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
