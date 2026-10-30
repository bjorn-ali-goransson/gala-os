//! I/O completion APCs (what ReadFileEx/WriteFileEx use) on an
//! asynchronous file under `\??\C:\Users\Public`, with results expected to
//! match real Windows: the ApcRoutine is queued to the calling thread when
//! the I/O completes, runs only at the next alertable wait or delay as
//! ApcRoutine(ApcContext, IoStatusBlock, 0) with the IO_STATUS_BLOCK filled
//! in, in completion order; an event given as well is still signaled; a
//! synchronous handle rejects an ApcRoutine. Prints `ioapc_test: ok` or
//! exits with the number of the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use ntrt::*;

const STATUS_INVALID_PARAMETER: NtStatus = 0xc000_000d;
const FILE_DISPOSITION_INFORMATION: u32 = 13;
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const SECOND: i64 = -10_000_000;
const DATA: &[u8] = b"completion routines";

/// Calls as (context, IOSB address, reserved, IOSB status, IOSB count).
static LOG: [[AtomicU64; 5]; 8] = [const { [const { AtomicU64::new(0) }; 5] }; 8];
static CALLS: AtomicUsize = AtomicUsize::new(0);

extern "system" fn done(context: usize, iosb: *mut IoStatusBlock, reserved: u32) {
    let i = CALLS.fetch_add(1, Ordering::SeqCst);
    // SAFETY: the system passes the IO_STATUS_BLOCK of the completed I/O.
    let (status, count) = unsafe { ((*iosb).status as u64, (*iosb).information as u64) };
    if let Some(slot) = LOG.get(i) {
        let values = [context as u64, iosb as u64, reserved.into(), status, count];
        for (v, x) in slot.iter().zip(values) {
            v.store(x, Ordering::SeqCst);
        }
    }
}

fn logged(i: usize) -> [u64; 5] {
    [0, 1, 2, 3, 4].map(|k| LOG[i][k].load(Ordering::SeqCst))
}

fn calls() -> u64 {
    CALLS.load(Ordering::SeqCst) as u64
}

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
        print("ioapc_test: check ");
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

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = u64::from(STATUS_SUCCESS);
    let path = wide!("\\??\\C:\\Users\\Public\\gala_ioapc_test.dat");
    let path = UnicodeString::new(&path);
    let attrs = ObjectAttributes::new(&path, Handle(0), OBJ_CASE_INSENSITIVE);
    let (s, old, _) = open_file(&attrs, DELETE, SHARE_ALL, FILE_NON_DIRECTORY_FILE);
    if s == STATUS_SUCCESS {
        set_file(old, FILE_DISPOSITION_INFORMATION, &[1]);
        close(old);
    }
    let rw = GENERIC_READ | GENERIC_WRITE | DELETE;
    let (s, file, _) = create_file(&attrs, rw, SHARE_ALL, FILE_CREATE, FILE_NON_DIRECTORY_FILE);
    c.eq(s.into(), ok)?;

    // A write's routine runs at the next alertable delay, with its IOSB.
    let mut w = IoStatusBlock { status: 0xdead, information: 0xdead };
    let s = write_file_apc(file, DATA, Some(0), done, 0x1111, &mut w);
    c.ok(started(s), s.into())?;
    c.eq(calls(), 0)?;
    c.eq(delay_alertable(SECOND).into(), STATUS_USER_APC.into())?;
    c.eq(calls(), 1)?;
    let iosb_addr = &raw const w as u64;
    c.ok(logged(0) == [0x1111, iosb_addr, 0, 0, DATA.len() as u64], logged(0)[0])?;

    // Two reads complete in order; an alertable wait runs both.
    let (s, event) = create_event(EVENT_ALL_ACCESS, NOTIFICATION_EVENT, false);
    c.eq(s.into(), ok)?;
    let (mut a, mut b) = ([0u8; 4], [0u8; 4]);
    let mut ra = IoStatusBlock { status: 0xdead, information: 0xdead };
    let mut rb = IoStatusBlock { status: 0xdead, information: 0xdead };
    c.ok(started(read_file_apc(file, &mut a, Some(0), Handle(0), done, 1, &mut ra)), 0)?;
    c.ok(started(read_file_apc(file, &mut b, Some(4), Handle(0), done, 2, &mut rb)), 1)?;
    c.eq(wait_alertable(event, Some(SECOND)).into(), STATUS_USER_APC.into())?;
    c.eq(calls(), 3)?;
    c.eq(logged(1)[0], 1)?;
    c.eq(logged(2)[0], 2)?;
    c.ok(&a == b"comp" && &b == b"leti", 0)?;

    // With an event too, the event is signaled and the routine still queued.
    let mut r = IoStatusBlock { status: 0xdead, information: 0xdead };
    let mut buf = [0u8; 8];
    c.ok(started(read_file_apc(file, &mut buf, Some(0), event, done, 3, &mut r)), 2)?;
    c.eq(wait(event, Some(SECOND)).into(), ok)?;
    c.eq(calls(), 3)?;
    c.eq(delay_alertable(SECOND).into(), STATUS_USER_APC.into())?;
    c.eq(calls(), 4)?;
    c.eq(logged(3)[0], 3)?;
    close(event);

    // A synchronous handle does not take an ApcRoutine.
    let sync = FILE_SYNCHRONOUS_IO_NONALERT | FILE_NON_DIRECTORY_FILE;
    let (s, h, _) = open_file(&attrs, GENERIC_READ, SHARE_ALL, sync);
    c.eq(s.into(), ok)?;
    let mut r = IoStatusBlock { status: 0xdead, information: 0xdead };
    c.eq(
        read_file_apc(h, &mut buf, Some(0), Handle(0), done, 4, &mut r).into(),
        STATUS_INVALID_PARAMETER.into(),
    )?;
    close(h);
    c.eq(delay_alertable(SECOND / 100).into(), ok)?; // nothing queued
    c.eq(calls(), 4)?;

    c.eq(set_file(file, FILE_DISPOSITION_INFORMATION, &[1]).into(), ok)?;
    close(file);
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("ioapc_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
