//! Asynchronous file I/O (a handle opened without FILE_SYNCHRONOUS_IO_*)
//! under `\??\C:\Users\Public`, with results expected to match real
//! Windows, which may complete such I/O at once or later (both accepted):
//! offsets are required, completion goes to the IO_STATUS_BLOCK and signals
//! the event (or the file object), the file position does not move, and
//! FileModeInformation tells synchronous from asynchronous handles.
//! Prints `async_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use ntrt::*;

const STATUS_END_OF_FILE: NtStatus = 0xc000_0011;
const STATUS_INVALID_PARAMETER: NtStatus = 0xc000_000d;
const FILE_DISPOSITION_INFORMATION: u32 = 13;
const FILE_POSITION_INFORMATION: u32 = 14;
const FILE_MODE_INFORMATION: u32 = 16;
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const DATA: &[u8] = b"asynchronous gala";

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
        print("async_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn started(s: NtStatus) -> bool {
    s == STATUS_SUCCESS || s == STATUS_PENDING
}

fn mode(h: Handle) -> (NtStatus, u64) {
    let mut b = [0xffu8; 4];
    let s = query_file(h, FILE_MODE_INFORMATION, &mut b);
    (s, u32::from_le_bytes(b).into())
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = u64::from(STATUS_SUCCESS);
    let path = wide!("\\??\\C:\\Users\\Public\\gala_async_test.dat");
    let path = UnicodeString::new(&path);
    let attrs = ObjectAttributes::new(&path, Handle(0), OBJ_CASE_INSENSITIVE);
    let (s, old, _) = open_file(&attrs, DELETE, SHARE_ALL, FILE_NON_DIRECTORY_FILE);
    if s == STATUS_SUCCESS {
        set_file(old, FILE_DISPOSITION_INFORMATION, &[1]);
        close(old);
    }

    // An asynchronous handle (no FILE_SYNCHRONOUS_IO_* option).
    let rw = GENERIC_READ | GENERIC_WRITE | DELETE;
    let (s, file, _) = create_file(&attrs, rw, SHARE_ALL, FILE_CREATE, FILE_NON_DIRECTORY_FILE);
    c.eq(s.into(), ok)?;
    let (s, m) = mode(file);
    c.eq(s.into(), ok)?;
    c.eq(m, 0)?;

    // Offsets are required.
    let mut iosb = IoStatusBlock { status: 0, information: 0 };
    c.eq(
        write_file_async(file, DATA, None, Handle(0), &mut iosb).into(),
        STATUS_INVALID_PARAMETER.into(),
    )?;
    let mut buf = [0u8; 64];
    c.eq(
        read_file_async(file, &mut buf, None, Handle(0), &mut iosb).into(),
        STATUS_INVALID_PARAMETER.into(),
    )?;

    // Write with an event.
    let (s, event) = create_event(EVENT_ALL_ACCESS, NOTIFICATION_EVENT, false);
    c.eq(s.into(), ok)?;
    let mut iosb = IoStatusBlock { status: 0xdead, information: 0xdead };
    let s = write_file_async(file, DATA, Some(0), event, &mut iosb);
    c.ok(started(s), s.into())?;
    c.eq(wait(event, None).into(), ok)?;
    c.eq(iosb.status as u64, ok)?;
    c.eq(iosb.information as u64, DATA.len() as u64)?;

    // Read with an event (it is reset when the I/O starts).
    let mut iosb = IoStatusBlock { status: 0xdead, information: 0xdead };
    let s = read_file_async(file, &mut buf, Some(0), event, &mut iosb);
    c.ok(started(s), s.into())?;
    c.eq(wait(event, None).into(), ok)?;
    c.eq(iosb.status as u64, ok)?;
    c.eq(iosb.information as u64, DATA.len() as u64)?;
    c.ok(&buf[..DATA.len()] == DATA, 0)?;

    // Without an event the file object itself is signaled.
    let mut small = [0u8; 4];
    let mut iosb = IoStatusBlock { status: 0xdead, information: 0xdead };
    let s = read_file_async(file, &mut small, Some(6), Handle(0), &mut iosb);
    c.ok(started(s), s.into())?;
    c.eq(wait(file, None).into(), ok)?;
    c.eq(iosb.status as u64, ok)?;
    c.eq(iosb.information as u64, 4)?;
    c.ok(&small == b"rono", 0)?; // DATA[6..10]

    // End of file, at once or through the IO_STATUS_BLOCK.
    let mut iosb = IoStatusBlock { status: 0xdead, information: 0xdead };
    let s = read_file_async(file, &mut buf, Some(DATA.len() as i64), event, &mut iosb);
    if s == STATUS_PENDING {
        c.eq(wait(event, None).into(), ok)?;
        c.eq(iosb.status as u64, STATUS_END_OF_FILE.into())?;
    } else {
        c.eq(s.into(), STATUS_END_OF_FILE.into())?;
    }

    // Asynchronous I/O does not move the file position.
    let mut pos = [0xffu8; 8];
    c.eq(query_file(file, FILE_POSITION_INFORMATION, &mut pos).into(), ok)?;
    c.eq(u64::from_le_bytes(pos), 0)?;

    // A synchronous handle says so.
    let sync = FILE_SYNCHRONOUS_IO_NONALERT | FILE_NON_DIRECTORY_FILE;
    let (s, h, _) = open_file(&attrs, GENERIC_READ, SHARE_ALL, sync);
    c.eq(s.into(), ok)?;
    c.eq(mode(h).1, FILE_SYNCHRONOUS_IO_NONALERT.into())?;
    close(h);

    close(event);
    c.eq(set_file(file, FILE_DISPOSITION_INFORMATION, &[1]).into(), ok)?;
    close(file);
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("async_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
