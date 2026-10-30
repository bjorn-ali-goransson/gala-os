//! Symbolic links and directory listing in the object namespace, with
//! results expected to match real Windows (services with build-dependent
//! numbers are called through ntdll's exports): `\??\C:` is a link to the
//! volume device; links can be created in `\BaseNamedObjects` and are
//! followed in the middle and at the end of a path; the session's
//! BaseNamedObjects has a `Global` link; NtQueryDirectoryObject lists
//! (name, type) entries. Prints `symlink_test: ok` or exits with the
//! number of the failing check.

#![no_std]
#![no_main]

use ntrt::*;

const STATUS_BUFFER_TOO_SMALL: NtStatus = 0xc000_0023;
const STATUS_NO_MORE_ENTRIES: NtStatus = 0x8000_001a;
const SYMBOLIC_LINK_QUERY: u32 = 0x0001;
const SYMBOLIC_LINK_ALL_ACCESS: u32 = 0xf_0001;
const DIRECTORY_QUERY: u32 = 0x0001;
const PEB_SESSION_ID: usize = 0x2c0;

type CreateLink =
    extern "system" fn(*mut Handle, u32, *const ObjectAttributes, *const UnicodeString) -> NtStatus;
type OpenLink = extern "system" fn(*mut Handle, u32, *const ObjectAttributes) -> NtStatus;
type QueryLink = extern "system" fn(Handle, *mut UnicodeString, *mut u32) -> NtStatus;
type QueryDirectory =
    extern "system" fn(Handle, *mut u8, u32, u8, u8, *mut u32, *mut u32) -> NtStatus;

struct Checks(u32);

impl Checks {
    fn st(&mut self, got: NtStatus, want: NtStatus) -> Result<(), u32> {
        self.ok(got == want, got.into())
    }

    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("symlink_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn starts_with(s: &[u16], prefix: &[u16]) -> bool {
    s.len() >= prefix.len() && s[..prefix.len()] == *prefix
}

/// A UNICODE_STRING at `p`, as a slice.
fn ustr(p: *const u8) -> &'static [u16] {
    // SAFETY: a UNICODE_STRING written by the system.
    unsafe {
        let len = usize::from(*(p as *const u16)) / 2;
        let buf = *(p.add(8) as *const *const u16);
        if buf.is_null() { &[] } else { core::slice::from_raw_parts(buf, len) }
    }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = STATUS_SUCCESS;
    let Some(ntdll) = find_ntdll() else { return c.ok(false, 0) };
    let get = |name: &[u8]| find_export(ntdll, name);
    let (Some(create_link), Some(open_link), Some(query_link), Some(query_directory)) = (
        get(b"NtCreateSymbolicLinkObject"),
        get(b"NtOpenSymbolicLinkObject"),
        get(b"NtQuerySymbolicLinkObject"),
        get(b"NtQueryDirectoryObject"),
    ) else {
        return c.ok(false, 1);
    };
    // SAFETY: the documented signatures of these ntdll exports.
    let (create_link, open_link, query_link, query_directory) = unsafe {
        (
            core::mem::transmute::<usize, CreateLink>(create_link),
            core::mem::transmute::<usize, OpenLink>(open_link),
            core::mem::transmute::<usize, QueryLink>(query_link),
            core::mem::transmute::<usize, QueryDirectory>(query_directory),
        )
    };

    // \??\C: is a link to the volume device.
    let c_drive = wide!("\\??\\C:");
    let c_drive = UnicodeString::new(&c_drive);
    let c_attrs = ObjectAttributes::new(&c_drive, Handle(0), OBJ_CASE_INSENSITIVE);
    let mut link = Handle(0);
    c.st(open_link(&mut link, SYMBOLIC_LINK_QUERY, &c_attrs), ok)?;
    let mut buf = [0u16; 256];
    let mut target = UnicodeString { length: 0, maximum_length: 512, buffer: buf.as_mut_ptr() };
    let mut returned = 0u32;
    c.st(query_link(link, &mut target, &mut returned), ok)?;
    let len = usize::from(target.length) / 2;
    c.ok(starts_with(&buf[..len], &wide!("\\Device\\HarddiskVolume")), len as u64)?;
    c.ok(returned >= u32::from(target.length), returned.into())?;
    let mut tiny = [0u16; 2];
    let mut short = UnicodeString { length: 0, maximum_length: 4, buffer: tiny.as_mut_ptr() };
    let mut needed = 0u32;
    c.st(query_link(link, &mut short, &mut needed), STATUS_BUFFER_TOO_SMALL)?;
    c.ok(needed >= u32::from(target.length), needed.into())?;
    close(link);

    // A named event, and links to its directory and to the event itself.
    let event_name = wide!("\\BaseNamedObjects\\gala_symlink_test_event");
    let event_name = UnicodeString::new(&event_name);
    let event_attrs = ObjectAttributes::new(&event_name, Handle(0), OBJ_CASE_INSENSITIVE);
    let (s, event) =
        create_event_with(EVENT_ALL_ACCESS, Some(&event_attrs), NOTIFICATION_EVENT, false);
    c.st(s, ok)?;
    let dir_link = wide!("\\BaseNamedObjects\\gala_symlink_test_link");
    let dir_link = UnicodeString::new(&dir_link);
    let dir_target = wide!("\\BaseNamedObjects");
    let dir_target = UnicodeString::new(&dir_target);
    let mut l1 = Handle(0);
    let l1_attrs = ObjectAttributes::new(&dir_link, Handle(0), OBJ_CASE_INSENSITIVE);
    c.st(create_link(&mut l1, SYMBOLIC_LINK_ALL_ACCESS, &l1_attrs, &dir_target), ok)?;
    let ev_link = wide!("\\BaseNamedObjects\\gala_symlink_test_evlink");
    let ev_link = UnicodeString::new(&ev_link);
    let mut l2 = Handle(0);
    let l2_attrs = ObjectAttributes::new(&ev_link, Handle(0), OBJ_CASE_INSENSITIVE);
    c.st(create_link(&mut l2, SYMBOLIC_LINK_ALL_ACCESS, &l2_attrs, &event_name), ok)?;

    // Links are followed in the middle and at the end of a path.
    let via_dir = wide!("\\BaseNamedObjects\\gala_symlink_test_link\\gala_symlink_test_event");
    let via_dir = UnicodeString::new(&via_dir);
    let (s, h) = open_event(
        EVENT_ALL_ACCESS,
        &ObjectAttributes::new(&via_dir, Handle(0), OBJ_CASE_INSENSITIVE),
    );
    c.st(s, ok)?;
    close(h);
    let (s, h) = open_event(EVENT_ALL_ACCESS, &l2_attrs);
    c.st(s, ok)?;
    close(h);

    // The session's Global link reaches \BaseNamedObjects.
    // SAFETY: SessionId in the PEB.
    let session = unsafe { *(peb().add(PEB_SESSION_ID) as *const u32) };
    let mut path = [0u16; 96];
    let mut n = 0;
    for part in ["\\Sessions\\", "", "\\BaseNamedObjects\\Global\\gala_symlink_test_event"] {
        if part.is_empty() {
            let mut digits = [0u16; 10];
            let mut v = session;
            let mut k = 0;
            loop {
                digits[k] = u16::from(b'0') + (v % 10) as u16;
                k += 1;
                v /= 10;
                if v == 0 {
                    break;
                }
            }
            for d in digits[..k].iter().rev() {
                path[n] = *d;
                n += 1;
            }
        }
        for b in part.bytes() {
            path[n] = u16::from(b);
            n += 1;
        }
    }
    let global = UnicodeString::new(&path[..n]);
    let (s, h) = open_event(
        EVENT_ALL_ACCESS,
        &ObjectAttributes::new(&global, Handle(0), OBJ_CASE_INSENSITIVE),
    );
    c.st(s, ok)?;
    close(h);

    // NtQueryDirectoryObject: one (name, type) entry at a time.
    let bno = wide!("\\BaseNamedObjects");
    let bno = UnicodeString::new(&bno);
    let (s, dir) = open_directory(
        DIRECTORY_QUERY,
        &ObjectAttributes::new(&bno, Handle(0), OBJ_CASE_INSENSITIVE),
    );
    c.st(s, ok)?;
    let (mut context, mut found_event, mut found_link) = (0u32, false, false);
    let mut entry = [0u8; 1024];
    let mut status = ok;
    for i in 0..100_000 {
        let mut ret = 0;
        status = query_directory(
            dir,
            entry.as_mut_ptr(),
            entry.len() as u32,
            1,
            u8::from(i == 0),
            &mut context,
            &mut ret,
        );
        if status != ok {
            break;
        }
        let (name, kind) = (ustr(entry.as_ptr()), ustr(entry[16..].as_ptr()));
        found_event |= name == wide!("gala_symlink_test_event") && kind == wide!("Event");
        found_link |= name == wide!("gala_symlink_test_link") && kind == wide!("SymbolicLink");
    }
    c.st(status, STATUS_NO_MORE_ENTRIES)?;
    c.ok(found_event, 0)?;
    c.ok(found_link, 1)?;
    c.ok(context > 2, context.into())?;
    close(dir);

    close(l2);
    close(l1);
    close(event);
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("symlink_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
