//! I/O completion ports, with results expected to match real Windows (the
//! services are called through ntdll's exports: their numbers depend on the
//! build): a port bound to an asynchronous file receives one packet per
//! completed read or write (key, ApcContext, status, count) in FIFO order;
//! NtSetIoCompletion posts directly; NtRemoveIoCompletion times out when
//! empty and wakes a thread blocked in it; closing the port's last handle
//! ends such a wait with STATUS_ABANDONED_WAIT_0 (GetQueuedCompletionStatus'
//! ERROR_ABANDONED_WAIT_0); a bad output pointer fails with
//! STATUS_ACCESS_VIOLATION and leaves the packet queued;
//! NtQueryIoCompletion reports the
//! depth; an I/O that fails at once posts nothing. Prints `iocp_test: ok`
//! or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use ntrt::*;

const STATUS_TIMEOUT: NtStatus = 0x0000_0102;
const STATUS_ABANDONED_WAIT_0: NtStatus = 0x0000_0080;
const STATUS_ACCESS_VIOLATION: NtStatus = 0xc000_0005u32 as NtStatus;
const STATUS_END_OF_FILE: NtStatus = 0xc000_0011;
const IO_COMPLETION_ALL_ACCESS: u32 = 0x1f_0003;
const FILE_COMPLETION_INFORMATION: u32 = 30;
const FILE_DISPOSITION_INFORMATION: u32 = 13;
const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const SECOND: i64 = -10_000_000;
const DATA: &[u8] = b"completion port";

type Create = extern "system" fn(*mut Handle, u32, *const ObjectAttributes, u32) -> NtStatus;
type Set = extern "system" fn(Handle, usize, usize, NtStatus, usize) -> NtStatus;
type Remove =
    extern "system" fn(Handle, *mut usize, *mut usize, *mut IoStatusBlock, *const i64) -> NtStatus;
type Query = extern "system" fn(Handle, u32, *mut u8, u32, *mut u32) -> NtStatus;

struct Api {
    create: Create,
    set: Set,
    remove: Remove,
    query: Query,
}

/// One dequeued packet: (status of the call, key, context, IOSB).
fn remove(
    api: &Api,
    port: Handle,
    timeout: Option<i64>,
) -> (NtStatus, usize, usize, IoStatusBlock) {
    let (mut key, mut context) = (0xdead, 0xdead);
    let mut iosb = IoStatusBlock { status: 0xdead, information: 0xdead };
    let t = timeout.unwrap_or(0);
    let p = if timeout.is_some() { &t as *const i64 } else { core::ptr::null() };
    let s = (api.remove)(port, &mut key, &mut context, &mut iosb, p);
    (s, key, context, iosb)
}

fn depth(api: &Api, port: Handle) -> u64 {
    let mut b = [0xffu8; 4];
    let mut ret = 0;
    (api.query)(port, 0, b.as_mut_ptr(), 4, &mut ret);
    u32::from_le_bytes(b).into()
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    #[track_caller]
    fn st(&mut self, got: NtStatus, want: NtStatus) -> Result<(), u32> {
        self.eq(got.into(), want.into())
    }

    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("iocp_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        check_line(core::panic::Location::caller().line());
        print("\r\n");
        Err(self.0)
    }
}

fn started(s: NtStatus) -> bool {
    s == STATUS_SUCCESS || s == STATUS_PENDING
}

static PORT: AtomicUsize = AtomicUsize::new(0);
static REMOVE_FN: AtomicUsize = AtomicUsize::new(0);
static WORKER_KEY: AtomicU64 = AtomicU64::new(0);
/// NtRemoveIoCompletion's status in `waiter`.
static WORKER_STATUS: AtomicU64 = AtomicU64::new(0);

extern "C" fn waiter(_: usize) -> ! {
    // SAFETY: stored from the resolved NtRemoveIoCompletion export.
    let remove = unsafe { core::mem::transmute::<usize, Remove>(REMOVE_FN.load(Ordering::SeqCst)) };
    let (mut key, mut context) = (0, 0);
    let mut iosb = IoStatusBlock { status: 0, information: 0 };
    let s = remove(
        Handle(PORT.load(Ordering::SeqCst)),
        &mut key,
        &mut context,
        &mut iosb,
        core::ptr::null(),
    );
    WORKER_STATUS.store(u64::from(s as u32), Ordering::SeqCst);
    WORKER_KEY.store(if s == STATUS_SUCCESS { key as u64 } else { 0xbad }, Ordering::SeqCst);
    exit_thread(0)
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = STATUS_SUCCESS;
    let Some(ntdll) = find_ntdll() else { return c.ok(false, 0) };
    let get = |name: &[u8]| find_export(ntdll, name);
    let (Some(create), Some(set), Some(remove_fn), Some(query)) = (
        get(b"NtCreateIoCompletion"),
        get(b"NtSetIoCompletion"),
        get(b"NtRemoveIoCompletion"),
        get(b"NtQueryIoCompletion"),
    ) else {
        return c.ok(false, 1);
    };
    // SAFETY: the documented signatures of these ntdll exports.
    let api = unsafe {
        Api {
            create: core::mem::transmute::<usize, Create>(create),
            set: core::mem::transmute::<usize, Set>(set),
            remove: core::mem::transmute::<usize, Remove>(remove_fn),
            query: core::mem::transmute::<usize, Query>(query),
        }
    };
    let mut port = Handle(0);
    c.st((api.create)(&mut port, IO_COMPLETION_ALL_ACCESS, core::ptr::null(), 0), ok)?;
    c.eq(depth(&api, port), 0)?;

    // An asynchronous file bound to the port with key 0x77.
    let path = wide!("\\??\\C:\\Users\\Public\\gala_iocp_test.dat");
    let path = UnicodeString::new(&path);
    let attrs = ObjectAttributes::new(&path, Handle(0), OBJ_CASE_INSENSITIVE);
    let (s, old, _) = open_file(&attrs, DELETE, SHARE_ALL, FILE_NON_DIRECTORY_FILE);
    if s == STATUS_SUCCESS {
        set_file(old, FILE_DISPOSITION_INFORMATION, &[1]);
        close(old);
    }
    let rw = GENERIC_READ | GENERIC_WRITE | DELETE;
    let (s, file, _) = create_file(&attrs, rw, SHARE_ALL, FILE_CREATE, FILE_NON_DIRECTORY_FILE);
    c.st(s, ok)?;
    let mut binding = [0u8; 16];
    binding[0..8].copy_from_slice(&(port.0 as u64).to_le_bytes());
    binding[8..16].copy_from_slice(&0x77u64.to_le_bytes());
    c.st(set_file(file, FILE_COMPLETION_INFORMATION, &binding), ok)?;

    // A write posts (key, context, status, count).
    let mut w = IoStatusBlock { status: 0xdead, information: 0xdead };
    c.ok(started(write_file_context(file, DATA, 0, 0x1000, &mut w)), 0)?;
    let (s, key, context, iosb) = remove(&api, port, Some(SECOND));
    c.st(s, ok)?;
    c.ok(key == 0x77 && context == 0x1000, key as u64)?;
    c.eq(iosb.status as u64, 0)?;
    c.eq(iosb.information as u64, DATA.len() as u64)?;

    // Two reads come out in order.
    let (mut a, mut b) = ([0u8; 4], [0u8; 4]);
    let mut ra = IoStatusBlock { status: 0, information: 0 };
    let mut rb = IoStatusBlock { status: 0, information: 0 };
    c.ok(started(read_file_context(file, &mut a, 0, 1, &mut ra)), 1)?;
    c.ok(started(read_file_context(file, &mut b, 4, 2, &mut rb)), 2)?;
    let (s1, _, c1, _) = remove(&api, port, Some(SECOND));
    let (s2, _, c2, i2) = remove(&api, port, Some(SECOND));
    c.ok(s1 == ok && s2 == ok, s1.into())?;
    c.ok(c1 == 1 && c2 == 2, c1 as u64)?;
    c.eq(i2.information as u64, 4)?;
    c.ok(&a == b"comp" && &b == b"leti", 0)?;

    // Posting directly, and the queue depth.
    c.st((api.set)(port, 0x55, 0x66, ok, 9), ok)?;
    c.st((api.set)(port, 0x56, 0x67, ok, 10), ok)?;
    c.eq(depth(&api, port), 2)?;
    let (s, key, context, iosb) = remove(&api, port, Some(0));
    c.st(s, ok)?;
    c.ok(key == 0x55 && context == 0x66 && iosb.information == 9, key as u64)?;
    let _ = remove(&api, port, Some(0));
    c.eq(depth(&api, port), 0)?;
    c.st(remove(&api, port, Some(SECOND / 100)).0, STATUS_TIMEOUT)?;

    // A read that fails at once (end of file) posts nothing.
    let mut buf = [0u8; 8];
    let mut r = IoStatusBlock { status: 0, information: 0 };
    let s = read_file_context(file, &mut buf, 1000, 3, &mut r);
    if s == STATUS_PENDING {
        let (s, _, context, iosb) = remove(&api, port, Some(SECOND));
        c.ok(
            s == ok && context == 3 && iosb.status as u32 == STATUS_END_OF_FILE,
            iosb.status as u64,
        )?;
    } else {
        c.st(s, STATUS_END_OF_FILE)?;
        c.st(remove(&api, port, Some(SECOND / 100)).0, STATUS_TIMEOUT)?;
    }

    // A post wakes a thread blocked in NtRemoveIoCompletion.
    PORT.store(port.0, Ordering::SeqCst);
    REMOVE_FN.store(remove_fn, Ordering::SeqCst);
    let (s, t, _) = create_thread(waiter, 0, false);
    c.st(s, ok)?;
    delay(SECOND / 50);
    c.eq(WORKER_KEY.load(Ordering::SeqCst), 0)?;
    c.st((api.set)(port, 0x99, 0, ok, 0), ok)?;
    c.st(wait(t, Some(5 * SECOND)), ok)?;
    c.eq(WORKER_KEY.load(Ordering::SeqCst), 0x99)?;
    close(t);

    // A bad output pointer fails the call and leaves the packet queued
    // (Windows probes the outputs before dequeuing).
    c.st((api.set)(port, 0x77, 0, ok, 0), ok)?;
    let mut ctx = 0usize;
    let mut iosb2 = IoStatusBlock { status: 0, information: 0 };
    let bad = core::ptr::without_provenance_mut::<usize>(8); // the first page: never mapped
    c.st(
        (api.remove)(port, bad, &mut ctx, &mut iosb2, core::ptr::null()),
        STATUS_ACCESS_VIOLATION,
    )?;
    c.eq(depth(&api, port), 1)?;
    let (s, key, _, _) = remove(&api, port, Some(0));
    c.st(s, ok)?;
    c.eq(key as u64, 0x77)?;

    // Closing a port's last handle ends a wait in NtRemoveIoCompletion
    // (GetQueuedCompletionStatus: ERROR_ABANDONED_WAIT_0).
    let mut port2 = Handle(0);
    c.st((api.create)(&mut port2, IO_COMPLETION_ALL_ACCESS, core::ptr::null(), 0), ok)?;
    PORT.store(port2.0, Ordering::SeqCst);
    let (s, t2, _) = create_thread(waiter, 0, false);
    c.st(s, ok)?;
    delay(SECOND / 50);
    close(port2);
    c.st(wait(t2, Some(5 * SECOND)), ok)?;
    c.eq(WORKER_STATUS.load(Ordering::SeqCst), u64::from(STATUS_ABANDONED_WAIT_0))?;
    close(t2);

    c.st(set_file(file, FILE_DISPOSITION_INFORMATION, &[1]), ok)?;
    close(file);
    close(port);
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("iocp_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);

/// ` (line L)`: the source line of the failing check, from
/// `#[track_caller]` on Checks' methods (so a report names the line
/// without anyone counting checks).
fn check_line(line: u32) {
    let mut b = *b" (line 0000000000)";
    let mut digits = [0u8; 10];
    let (mut n, mut len) = (line, 0);
    loop {
        digits[len] = b'0' + (n % 10) as u8;
        len += 1;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    for i in 0..len {
        b[7 + i] = digits[len - 1 - i];
    }
    b[7 + len] = b')';
    print(core::str::from_utf8(&b[..8 + len]).unwrap_or(""));
}
