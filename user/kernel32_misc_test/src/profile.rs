//! Initialization files (Microsoft's GetPrivateProfileString and
//! WritePrivateProfileString pages; the details as Wine's
//! `dlls/kernel32/tests/profile.c` checks them on Windows). Inno Setup
//! writes its Internet shortcuts (`.url` files) with them:
//!
//! - WritePrivateProfileStringW to a file that does not exist creates it
//!   as ANSI text "[App]\r\nkey=value\r\n"; another key of the section
//!   goes after the section's last key, another section at the end.
//! - GetPrivateProfileStringW: a value by section and key, matched
//!   without regard to case, blanks around keys and values ignored, one
//!   pair of quotes around a value removed; a missing one gives the
//!   default without its trailing blanks (NULL: empty); the count of
//!   characters copied. Too small a buffer: the value cut to size - 1
//!   characters (the count size - 1). A NULL key lists the section's keys,
//!   a NULL section the sections, each NUL-terminated with a NUL after
//!   the last (the count without that final NUL); cut short, size - 2
//!   with two NULs at the end.
//! - WritePrivateProfileStringW with a NULL value deletes the key, with a
//!   NULL key the whole section.
//! - A file name without a path is in the Windows directory
//!   (GetProfileStringW/WriteProfileStringW: win.ini there).

use super::{Checks, Handle};

type Bool = i32;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetPrivateProfileStringW(
        app: *const u16,
        key: *const u16,
        default: *const u16,
        out: *mut u16,
        size: u32,
        file: *const u16,
    ) -> u32;
    fn WritePrivateProfileStringW(
        app: *const u16,
        key: *const u16,
        value: *const u16,
        file: *const u16,
    ) -> Bool;
    fn GetProfileStringW(
        app: *const u16,
        key: *const u16,
        default: *const u16,
        out: *mut u16,
        size: u32,
    ) -> u32;
    fn WriteProfileStringW(app: *const u16, key: *const u16, value: *const u16) -> Bool;
    fn CreateFileW(
        path: *const u16,
        access: u32,
        share: u32,
        security: *mut core::ffi::c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn ReadFile(
        h: Handle,
        buf: *mut u8,
        n: u32,
        read: *mut u32,
        ov: *mut core::ffi::c_void,
    ) -> Bool;
    fn WriteFile(
        h: Handle,
        buf: *const u8,
        n: u32,
        written: *mut u32,
        ov: *mut core::ffi::c_void,
    ) -> Bool;
    fn CloseHandle(h: Handle) -> Bool;
    fn DeleteFileW(path: *const u16) -> Bool;
    fn GetTempPathW(n: u32, buf: *mut u16) -> u32;
    fn GetWindowsDirectoryW(buf: *mut u16, n: u32) -> u32;
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;

/// A NUL-terminated UTF-16 copy of an ASCII string.
fn w(s: &str) -> [u16; 80] {
    let mut b = [0u16; 80];
    for (d, ch) in b.iter_mut().zip(s.bytes()) {
        *d = ch.into();
    }
    b
}

/// `dir` (NUL-terminated) followed by `name`.
fn path(dir: &[u16], name: &str) -> [u16; 300] {
    let mut p = [0u16; 300];
    let n = dir.iter().position(|&c| c == 0).unwrap_or(dir.len());
    p[..n].copy_from_slice(&dir[..n]);
    let mut at = n;
    if at > 0 && p[at - 1] != u16::from(b'\\') {
        p[at] = u16::from(b'\\');
        at += 1;
    }
    for (d, ch) in p[at..].iter_mut().zip(name.bytes()) {
        *d = ch.into();
    }
    p
}

fn file_bytes(p: &[u16; 300], buf: &mut [u8; 256]) -> usize {
    // SAFETY: documented calls on a NUL-terminated path and our buffer.
    unsafe {
        let h = CreateFileW(
            p.as_ptr(),
            GENERIC_READ,
            3,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        );
        if h as isize == -1 {
            return 0;
        }
        let mut n = 0;
        ReadFile(h, buf.as_mut_ptr(), 256, &mut n, core::ptr::null_mut());
        CloseHandle(h);
        n as usize
    }
}

fn put_file(p: &[u16; 300], bytes: &[u8]) {
    // SAFETY: documented calls on a NUL-terminated path and our bytes.
    unsafe {
        let h = CreateFileW(
            p.as_ptr(),
            GENERIC_WRITE,
            0,
            core::ptr::null_mut(),
            CREATE_ALWAYS,
            0,
            core::ptr::null_mut(),
        );
        let mut n = 0;
        WriteFile(h, bytes.as_ptr(), bytes.len() as u32, &mut n, core::ptr::null_mut());
        CloseHandle(h);
    }
}

fn same(got: &[u16], want: &str) -> bool {
    got.len() >= want.len() && got.iter().zip(want.bytes()).all(|(&g, b)| g == u16::from(b))
}

pub fn profiles(c: &mut Checks) -> Result<(), u32> {
    let mut temp = [0u16; 260];
    // SAFETY: our buffer.
    unsafe { GetTempPathW(260, temp.as_mut_ptr()) };
    let file = path(&temp, "gala_profile_test.ini");
    let f = file.as_ptr();
    let null = core::ptr::null();
    let mut out = [0u16; 64];
    let o = out.as_mut_ptr();
    // SAFETY: documented calls with NUL-terminated strings and our buffers
    // of the sizes given.
    unsafe {
        DeleteFileW(f);
        c.ok(
            WritePrivateProfileStringW(
                w("App").as_ptr(),
                w("key").as_ptr(),
                w("string").as_ptr(),
                f,
            ) != 0,
            0,
        )?;
        let mut bytes = [0u8; 256];
        let n = file_bytes(&file, &mut bytes);
        c.ok(&bytes[..n] == b"[App]\r\nkey=string\r\n", n as u64)?;
        c.ok(
            WritePrivateProfileStringW(
                w("App").as_ptr(),
                w("other").as_ptr(),
                w("two").as_ptr(),
                f,
            ) != 0,
            0,
        )?;
        c.ok(
            WritePrivateProfileStringW(w("Next").as_ptr(), w("k").as_ptr(), w("v").as_ptr(), f)
                != 0,
            0,
        )?;
        let n = file_bytes(&file, &mut bytes);
        let text = &bytes[..n];
        let has = |s: &[u8]| text.windows(s.len()).any(|x| x == s);
        c.ok(text.starts_with(b"[App]\r\nkey=string\r\nother=two\r\n"), n as u64)?;
        c.ok(has(b"[Next]\r\nk=v\r\n") && text.ends_with(b"k=v\r\n"), n as u64)?;

        // Reading back: case, blanks and quotes.
        put_file(
            &file,
            b"[Section1]\r\n  Key1 =  value one  \r\nkey2=\"quoted\"\r\n\r\n[ section2 ]\r\nk=x\r\n",
        );
        c.eq(
            u64::from(GetPrivateProfileStringW(
                w("section1").as_ptr(),
                w("KEY1").as_ptr(),
                w("d").as_ptr(),
                o,
                64,
                f,
            )),
            9,
        )?;
        c.ok(same(&out, "value one\0"), 0)?;
        c.eq(
            u64::from(GetPrivateProfileStringW(
                w("Section1").as_ptr(),
                w("key2").as_ptr(),
                null,
                o,
                64,
                f,
            )),
            6,
        )?;
        c.ok(same(&out, "quoted\0"), 0)?;
        c.eq(
            u64::from(GetPrivateProfileStringW(
                w("section2").as_ptr(),
                w("k").as_ptr(),
                null,
                o,
                64,
                f,
            )),
            1,
        )?;
        // Missing: the default without its trailing blanks; NULL: "".
        c.eq(
            u64::from(GetPrivateProfileStringW(
                w("Section1").as_ptr(),
                w("none").as_ptr(),
                w("def  ").as_ptr(),
                o,
                64,
                f,
            )),
            3,
        )?;
        c.ok(same(&out, "def\0"), 0)?;
        c.eq(
            u64::from(GetPrivateProfileStringW(
                w("Nope").as_ptr(),
                w("none").as_ptr(),
                null,
                o,
                64,
                f,
            )),
            0,
        )?;
        c.eq(u64::from(out[0]), 0)?;
        // Too small a buffer.
        c.eq(
            u64::from(GetPrivateProfileStringW(
                w("Section1").as_ptr(),
                w("Key1").as_ptr(),
                null,
                o,
                4,
                f,
            )),
            3,
        )?;
        c.ok(same(&out, "val\0"), 0)?;
        // Lists: keys of a section, sections.
        c.eq(
            u64::from(GetPrivateProfileStringW(w("Section1").as_ptr(), null, null, o, 64, f)),
            10,
        )?;
        c.ok(same(&out, "Key1\0key2\0\0"), 0)?;
        c.eq(u64::from(GetPrivateProfileStringW(null, null, null, o, 64, f)), 18)?;
        c.ok(same(&out, "Section1\0section2\0\0"), 0)?;
        c.eq(u64::from(GetPrivateProfileStringW(null, null, null, o, 12, f)), 10)?;
        c.ok(same(&out, "Section1\0s\0\0"), 0)?;

        // Deleting a key, then a section.
        c.ok(
            WritePrivateProfileStringW(w("Section1").as_ptr(), w("key1").as_ptr(), null, f) != 0,
            0,
        )?;
        c.eq(
            u64::from(GetPrivateProfileStringW(
                w("Section1").as_ptr(),
                w("Key1").as_ptr(),
                w("gone").as_ptr(),
                o,
                64,
                f,
            )),
            4,
        )?;
        c.eq(
            u64::from(GetPrivateProfileStringW(
                w("Section1").as_ptr(),
                w("key2").as_ptr(),
                null,
                o,
                64,
                f,
            )),
            6,
        )?;
        c.ok(WritePrivateProfileStringW(w("Section1").as_ptr(), null, null, f) != 0, 0)?;
        c.eq(u64::from(GetPrivateProfileStringW(null, null, null, o, 64, f)), 9)?;
        c.ok(same(&out, "section2\0\0"), 0)?;
        // Replacing a value keeps the key where it was.
        c.ok(
            WritePrivateProfileStringW(w("section2").as_ptr(), w("K").as_ptr(), w("y").as_ptr(), f)
                != 0,
            0,
        )?;
        c.eq(
            u64::from(GetPrivateProfileStringW(
                w("section2").as_ptr(),
                w("k").as_ptr(),
                null,
                o,
                64,
                f,
            )),
            1,
        )?;
        c.eq(u64::from(out[0]), u64::from(b'y'))?;
        DeleteFileW(f);

        // A bare name is in the Windows directory; win.ini is there too.
        let mut windir = [0u16; 260];
        GetWindowsDirectoryW(windir.as_mut_ptr(), 260);
        let bare = w("gala_profile_test.ini");
        c.ok(
            WritePrivateProfileStringW(
                w("A").as_ptr(),
                w("b").as_ptr(),
                w("c").as_ptr(),
                bare.as_ptr(),
            ) != 0,
            0,
        )?;
        let full = path(&windir, "gala_profile_test.ini");
        c.ok(file_bytes(&full, &mut bytes) > 0, 0)?;
        DeleteFileW(full.as_ptr());
        c.ok(
            WriteProfileStringW(w("GalaTest").as_ptr(), w("x").as_ptr(), w("1").as_ptr()) != 0,
            0,
        )?;
        c.eq(
            u64::from(GetProfileStringW(w("GalaTest").as_ptr(), w("x").as_ptr(), null, o, 64)),
            1,
        )?;
        c.ok(WriteProfileStringW(w("GalaTest").as_ptr(), null, null) != 0, 0)?;
        c.eq(
            u64::from(GetProfileStringW(w("GalaTest").as_ptr(), w("x").as_ptr(), null, o, 64)),
            0,
        )?;
    }
    Ok(())
}
