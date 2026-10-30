//! Guard pages and stack growth, with results expected to match real
//! Windows: a PAGE_GUARD page raises STATUS_GUARD_PAGE_VIOLATION once (to a
//! vectored handler registered through ntdll) and then behaves normally;
//! the main thread's stack grows past its initial commit without any
//! exception (TEB StackLimit moves down); runaway recursion on a thread
//! with a Windows-style stack raises STATUS_STACK_OVERFLOW. Prints
//! `guard_test: ok` or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::hint::black_box;
use core::sync::atomic::{AtomicU64, Ordering};

use ntrt::*;

const STATUS_GUARD_PAGE_VIOLATION: u32 = 0x8000_0001;
const STATUS_STACK_OVERFLOW: u32 = 0xc000_00fd;
const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
const TEB_STACK_LIMIT: usize = 0x10;
const TEB_DEALLOCATION_STACK: usize = 0x1478;
const CTX_RSP: usize = 0x98;
const CTX_RIP: usize = 0xf8;

#[repr(C)]
struct ExceptionPointers {
    record: *const u8,
    context: *mut u8,
}

/// Last exception seen: code, information[0], information[1]; and the
/// number of handler calls.
static SEEN: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn rd(p: *const u8, off: usize) -> u64 {
    // SAFETY: offsets inside the record/context the system passes.
    unsafe { (p.add(off) as *const u64).read_unaligned() }
}

/// Where the overflowing thread is sent: it ends with the status.
extern "C" fn overflow_exit() -> ! {
    exit_thread(STATUS_STACK_OVERFLOW)
}

extern "system" fn handler(pointers: *mut ExceptionPointers) -> i32 {
    // SAFETY: the dispatcher passes valid EXCEPTION_POINTERS.
    let (record, context) = unsafe { ((*pointers).record, (*pointers).context) };
    let code = rd(record, 0) as u32;
    SEEN[0].store(code.into(), Ordering::SeqCst);
    SEEN[1].store(rd(record, 0x20), Ordering::SeqCst);
    SEEN[2].store(rd(record, 0x28), Ordering::SeqCst);
    SEEN[3].fetch_add(1, Ordering::SeqCst);
    if code == STATUS_STACK_OVERFLOW {
        // Leave the exhausted stack frames behind: continue in
        // `overflow_exit`, on a call-aligned rsp.
        let rsp = (rd(context, CTX_RSP) & !0xf) - 8;
        // SAFETY: rewriting the context that will be resumed.
        unsafe {
            (context.add(CTX_RIP) as *mut u64).write_unaligned(overflow_exit as *const () as u64);
            (context.add(CTX_RSP) as *mut u64).write_unaligned(rsp);
        }
    }
    // A guard page fault is simply retried: the guard is gone now.
    EXCEPTION_CONTINUE_EXECUTION
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
        print("guard_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn seen(i: usize) -> u64 {
    SEEN[i].load(Ordering::SeqCst)
}

/// Uses about `depth` * 2 KiB of stack, touching every frame.
#[inline(never)]
fn deep(depth: u32) -> u64 {
    let mut buf = [0u8; 2048];
    black_box(&mut buf);
    buf[0] = depth as u8;
    let below = if depth == 0 { 0 } else { deep(depth - 1) };
    black_box(&buf)[2047] as u64 + below
}

/// Never stops recursing (the stack overflows).
#[inline(never)]
#[allow(unconditional_recursion)]
fn runaway(depth: u64) -> u64 {
    let mut buf = [0u8; 1024];
    black_box(&mut buf);
    buf[0] = depth as u8;
    runaway(depth + 1) + u64::from(black_box(&buf)[1])
}

extern "C" fn overflow_thread(_: usize) -> ! {
    black_box(runaway(0));
    exit_thread(0)
}

type AddHandler =
    extern "system" fn(u32, extern "system" fn(*mut ExceptionPointers) -> i32) -> *mut c_void;

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = u64::from(STATUS_SUCCESS);
    let Some(ntdll) = find_ntdll() else { return c.ok(false, 0) };
    let Some(add) = find_export(ntdll, b"RtlAddVectoredExceptionHandler") else {
        return c.ok(false, 1);
    };
    // SAFETY: the documented signature of the export.
    let add = unsafe { core::mem::transmute::<usize, AddHandler>(add) };
    c.ok(!add(1, handler).is_null(), 0)?;

    // Two guard pages: each raises once, then works.
    let (s, page, _) = allocate(0, 0x2000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE | PAGE_GUARD);
    c.eq(s.into(), ok)?;
    c.eq(query(page).1.protect.into(), (PAGE_READWRITE | PAGE_GUARD).into())?;
    // SAFETY: committed memory; the first access faults and is retried.
    let v = unsafe { core::ptr::read_volatile((page + 0x10) as *const u8) };
    c.eq(v.into(), 0)?;
    c.eq(seen(3), 1)?;
    c.eq(seen(0), STATUS_GUARD_PAGE_VIOLATION.into())?;
    c.eq(seen(1), 0)?; // read
    c.eq(seen(2), (page + 0x10) as u64)?;
    c.eq(query(page).1.protect.into(), PAGE_READWRITE.into())?;
    c.eq(query(page + 0x1000).1.protect.into(), (PAGE_READWRITE | PAGE_GUARD).into())?;
    // SAFETY: as above, for a write to the second page.
    unsafe { core::ptr::write_volatile((page + 0x1008) as *mut u8, 7) };
    c.eq(seen(3), 2)?;
    c.eq(seen(1), 1)?; // write
    c.eq(seen(2), (page + 0x1008) as u64)?;
    // SAFETY: both guards are gone; no more exceptions.
    unsafe {
        core::ptr::write_volatile((page + 0x10) as *mut u8, 1);
        c.eq(core::ptr::read_volatile((page + 0x1008) as *const u8).into(), 7)?;
    }
    c.eq(seen(3), 2)?;

    // The main thread's stack grows silently.
    let limit = teb_field(TEB_STACK_LIMIT);
    let bottom = teb_field(TEB_DEALLOCATION_STACK);
    c.ok(bottom != 0 && bottom < limit, bottom as u64)?;
    black_box(deep(64)); // about 128 KiB
    let grown = teb_field(TEB_STACK_LIMIT);
    c.ok(grown + 96 * 1024 <= limit, ((limit - grown) / 1024) as u64)?;
    c.eq(seen(3), 2)?;

    // Runaway recursion: STATUS_STACK_OVERFLOW on a 256 KiB stack.
    let (s, t, _) = create_thread_with_stack(overflow_thread, 0, 256 * 1024, 0x2000);
    c.eq(s.into(), ok)?;
    c.eq(wait(t, None).into(), ok)?;
    c.eq(thread_basic_info(t).1.into(), STATUS_STACK_OVERFLOW.into())?;
    c.eq(seen(0), STATUS_STACK_OVERFLOW.into())?;
    c.eq(seen(3), 3)?;
    close(t);
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("guard_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
