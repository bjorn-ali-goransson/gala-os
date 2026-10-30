//! Named objects in the object manager namespace, with results expected to
//! match real Windows. Prints `object_test: ok` or exits with the number of
//! the failing check.

#![no_std]
#![no_main]

use ntrt::*;

const STATUS_OBJECT_NAME_EXISTS: NtStatus = 0x4000_0035;
const STATUS_INVALID_PARAMETER: NtStatus = 0xc000_000d;
const STATUS_OBJECT_NAME_NOT_FOUND: NtStatus = 0xc000_0034;
const STATUS_OBJECT_NAME_COLLISION: NtStatus = 0xc000_0035;
const STATUS_OBJECT_PATH_NOT_FOUND: NtStatus = 0xc000_003a;
const STATUS_OBJECT_PATH_SYNTAX_BAD: NtStatus = 0xc000_003b;
const DIRECTORY_ALL_ACCESS: u32 = 0xf_000f;
const OBJECT_BASIC_INFORMATION: u32 = 0;
const OBJECT_NAME_INFORMATION: u32 = 1;

struct Checks(u32);

impl Checks {
    fn eq(&mut self, got: u32, want: u32) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    fn ok(&mut self, cond: bool, got: u32) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("object_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got));
        print("\r\n");
        Err(self.0)
    }
}

fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Reads the name returned by NtQueryObject(ObjectNameInformation).
fn name_of(h: Handle, out: &mut [u16; 64]) -> (NtStatus, usize) {
    let mut buf = [0u8; 256];
    let (st, _) = query_object(h, OBJECT_NAME_INFORMATION, &mut buf);
    if st != STATUS_SUCCESS {
        return (st, 0);
    }
    // OBJECT_NAME_INFORMATION { UNICODE_STRING Name; } + string data.
    let len = usize::from(u16::from_le_bytes([buf[0], buf[1]])) / 2;
    let ptr = usize::from_le_bytes(buf[8..16].try_into().unwrap()) as *const u16;
    for (i, slot) in out.iter_mut().enumerate().take(len.min(64)) {
        // SAFETY: the kernel points Buffer into `buf`, after the header.
        *slot = unsafe { ptr.add(i).read_unaligned() };
    }
    (st, len)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let a_name = wide!("\\BaseNamedObjects\\gala_object_test_A");
    let a = UnicodeString::new(&a_name);
    let attrs = ObjectAttributes::new(&a, Handle(0), OBJ_CASE_INSENSITIVE);
    let openif = ObjectAttributes::new(&a, Handle(0), OBJ_CASE_INSENSITIVE | OBJ_OPENIF);

    // Not there yet.
    c.eq(open_event(EVENT_ALL_ACCESS, &attrs).0, STATUS_OBJECT_NAME_NOT_FOUND)?;

    // Create; then create-or-open finds the same object.
    let (st, h1) = create_event_with(EVENT_ALL_ACCESS, Some(&attrs), NOTIFICATION_EVENT, false);
    c.eq(st, STATUS_SUCCESS)?;
    let (st, h2) = create_event_with(EVENT_ALL_ACCESS, Some(&openif), NOTIFICATION_EVENT, false);
    c.eq(st, STATUS_OBJECT_NAME_EXISTS)?;
    c.ok(h2 != h1 && h2.0 != 0, h2.0 as u32)?;
    set_event(h2);
    c.eq(wait(h1, Some(0)), STATUS_SUCCESS)?;
    // Without OBJ_OPENIF the name collides.
    c.eq(
        create_event_with(EVENT_ALL_ACCESS, Some(&attrs), NOTIFICATION_EVENT, false).0,
        STATUS_OBJECT_NAME_COLLISION,
    )?;

    // Case-insensitive open.
    let upper_name = wide!("\\BASENAMEDOBJECTS\\GALA_OBJECT_TEST_a");
    let upper = UnicodeString::new(&upper_name);
    let (st, h3) = open_event(
        EVENT_ALL_ACCESS,
        &ObjectAttributes::new(&upper, Handle(0), OBJ_CASE_INSENSITIVE),
    );
    c.eq(st, STATUS_SUCCESS)?;
    c.eq(wait(h3, Some(0)), STATUS_SUCCESS)?;

    // The object knows its full name.
    let mut name = [0u16; 64];
    let (st, len) = name_of(h3, &mut name);
    c.eq(st, STATUS_SUCCESS)?;
    c.ok(name[..len] == a_name[..], len as u32)?;

    // Basic information: granted access and handle count.
    let mut basic = [0u8; 56];
    let (st, _) = query_object(h1, OBJECT_BASIC_INFORMATION, &mut basic);
    c.eq(st, STATUS_SUCCESS)?;
    c.eq(u32_at(&basic, 4), EVENT_ALL_ACCESS)?; // GrantedAccess
    c.eq(u32_at(&basic, 8), 3)?; // HandleCount

    // Path errors.
    let bad_dir = wide!("\\BaseNamedObjects\\gala_no_such_dir\\x");
    let bad_dir = UnicodeString::new(&bad_dir);
    c.eq(
        open_event(EVENT_ALL_ACCESS, &ObjectAttributes::new(&bad_dir, Handle(0), 0)).0,
        STATUS_OBJECT_PATH_NOT_FOUND,
    )?;
    let relative = wide!("gala_object_test_A");
    let relative = UnicodeString::new(&relative);
    c.eq(
        open_event(EVENT_ALL_ACCESS, &ObjectAttributes::new(&relative, Handle(0), 0)).0,
        STATUS_OBJECT_PATH_SYNTAX_BAD,
    )?;
    let mut bad_len = ObjectAttributes::new(&a, Handle(0), 0);
    bad_len.length = 12;
    c.eq(open_event(EVENT_ALL_ACCESS, &bad_len).0, STATUS_INVALID_PARAMETER)?;

    // Names relative to a directory handle.
    let dir_name = wide!("\\BaseNamedObjects");
    let dir_name = UnicodeString::new(&dir_name);
    let (st, dir) = open_directory(
        DIRECTORY_ALL_ACCESS,
        &ObjectAttributes::new(&dir_name, Handle(0), OBJ_CASE_INSENSITIVE),
    );
    c.eq(st, STATUS_SUCCESS)?;
    let b_rel = wide!("gala_object_test_B");
    let b_rel = UnicodeString::new(&b_rel);
    let (st, hb) = create_event_with(
        EVENT_ALL_ACCESS,
        Some(&ObjectAttributes::new(&b_rel, dir, 0)),
        SYNCHRONIZATION_EVENT,
        true,
    );
    c.eq(st, STATUS_SUCCESS)?;
    let b_full = wide!("\\BaseNamedObjects\\gala_object_test_B");
    let b_full_us = UnicodeString::new(&b_full);
    let (st, hb2) = open_event(EVENT_ALL_ACCESS, &ObjectAttributes::new(&b_full_us, Handle(0), 0));
    c.eq(st, STATUS_SUCCESS)?;
    c.eq(wait(hb2, Some(0)), STATUS_SUCCESS)?;
    c.eq(wait(hb, Some(0)), STATUS_TIMEOUT)?; // same auto-reset event
    close(hb);
    close(hb2);
    close(dir);

    // An unnamed object has an empty name.
    let (_, anon) = create_event(EVENT_ALL_ACCESS, NOTIFICATION_EVENT, false);
    let (st, len) = name_of(anon, &mut name);
    c.eq(st, STATUS_SUCCESS)?;
    c.eq(len as u32, 0)?;
    close(anon);

    // The name goes away with the last handle.
    close(h1);
    close(h2);
    let (st, h4) = open_event(EVENT_ALL_ACCESS, &attrs); // h3 still open
    c.eq(st, STATUS_SUCCESS)?;
    close(h4);
    close(h3);
    c.eq(open_event(EVENT_ALL_ACCESS, &attrs).0, STATUS_OBJECT_NAME_NOT_FOUND)?;
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("object_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
