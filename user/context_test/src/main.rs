//! Suspending threads and their register state (what debuggers and Go's
//! goroutine preemption use), with results expected to match real Windows
//! (NtSuspendThread/NtGetContextThread/NtSetContextThread through ntdll's
//! exports: their numbers depend on the build). Prints `context_test: ok`
//! or exits with the number of the failing check.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

use ntrt::*;

const STATUS_THREAD_IS_TERMINATING: NtStatus = 0xc000_004b;
const MARK: u64 = 0x5eed_0000_c0de_0002;
const SECOND: i64 = -10_000_000;
const CTX_FLAGS: usize = 0x30;
const CTX_RCX: usize = 0x80;
const CTX_RSP: usize = 0x98;
const CTX_R12: usize = 0xd8;
const CTX_RIP: usize = 0xf8;
const CONTEXT_SIZE: usize = 0x4d0;
const TEB_STACK_BASE: usize = 0x08;
const TEB_STACK_LIMIT: usize = 0x10;

// A loop that only increments a counter, with r12 = the marker. Its bounds
// are known, so a captured Rip can be checked against them.
global_asm!(
    ".globl gala_spin",
    "gala_spin:",
    "mov r12, rdx",
    "2:",
    "lock inc qword ptr [rcx]",
    "jmp 2b",
    ".globl gala_spin_end",
    "gala_spin_end:",
);

unsafe extern "C" {
    fn gala_spin(counter: *const AtomicU64, marker: u64) -> !;
    static gala_spin_end: u8;
    static __ImageBase: u8;
}

static COUNTER: AtomicU64 = AtomicU64::new(0);
static REDIRECTED: AtomicU64 = AtomicU64::new(0);
static WAITER_DONE: AtomicU64 = AtomicU64::new(0);
static WAITER_EVENT: AtomicU64 = AtomicU64::new(0);

extern "C" fn spinner(_: usize) -> ! {
    // SAFETY: the loop only touches COUNTER.
    unsafe { gala_spin(&COUNTER, MARK) }
}

/// Where the spinner is sent by NtSetContextThread (rcx = its argument).
extern "C" fn redirected(arg: usize) -> ! {
    REDIRECTED.store(arg as u64, Ordering::SeqCst);
    exit_thread(0)
}

extern "C" fn waiter(_: usize) -> ! {
    let s = wait(Handle(WAITER_EVENT.load(Ordering::SeqCst) as usize), None);
    WAITER_DONE.store(1 + u64::from(s), Ordering::SeqCst);
    exit_thread(0)
}

type Suspend = extern "system" fn(Handle, *mut u32) -> NtStatus;
type GetContext = extern "system" fn(Handle, *mut u8) -> NtStatus;
type SetContext = extern "system" fn(Handle, *const u8) -> NtStatus;

#[repr(C, align(16))]
struct Ctx([u8; CONTEXT_SIZE]);

impl Ctx {
    fn full() -> Self {
        let mut c = Ctx([0; CONTEXT_SIZE]);
        c.0[CTX_FLAGS..CTX_FLAGS + 4].copy_from_slice(&CONTEXT_FULL.to_le_bytes());
        c
    }

    fn get(&self, off: usize) -> u64 {
        u64::from_le_bytes(self.0[off..off + 8].try_into().unwrap())
    }

    fn set(&mut self, off: usize, v: u64) {
        self.0[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }
}

struct Checks(u32);

impl Checks {
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    fn st(&mut self, got: NtStatus, want: NtStatus) -> Result<(), u32> {
        self.eq(got.into(), want.into())
    }

    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        print("context_test: check ");
        write_file(stdout(), &hex32(self.0));
        print(" got ");
        write_file(stdout(), &hex32(got as u32));
        print("\r\n");
        Err(self.0)
    }
}

fn counter() -> u64 {
    COUNTER.load(Ordering::SeqCst)
}

/// (StackLimit, StackBase) of the thread with TEB `teb`.
fn stack_of(teb: usize) -> (u64, u64) {
    // SAFETY: TEBs of this process's threads are readable.
    unsafe { (*((teb + TEB_STACK_LIMIT) as *const u64), *((teb + TEB_STACK_BASE) as *const u64)) }
}

fn run() -> Result<(), u32> {
    let mut c = Checks(0);
    let ok = STATUS_SUCCESS;
    let Some(ntdll) = find_ntdll() else { return c.ok(false, 0) };
    let get = |name: &[u8]| find_export(ntdll, name);
    let (Some(suspend), Some(get_context), Some(set_context)) =
        (get(b"NtSuspendThread"), get(b"NtGetContextThread"), get(b"NtSetContextThread"))
    else {
        return c.ok(false, 1);
    };
    // SAFETY: the documented signatures of these ntdll exports.
    let (suspend, get_context, set_context) = unsafe {
        (
            core::mem::transmute::<usize, Suspend>(suspend),
            core::mem::transmute::<usize, GetContext>(get_context),
            core::mem::transmute::<usize, SetContext>(set_context),
        )
    };

    // A spinning thread.
    let (s, t, _) = create_thread(spinner, 0, false);
    c.st(s, ok)?;
    while counter() == 0 {
        yield_execution();
    }

    // Suspend counts; the context shows where it stopped.
    let mut prev = 0xff;
    c.st(suspend(t, &mut prev), ok)?;
    c.eq(prev.into(), 0)?;
    c.st(suspend(t, &mut prev), ok)?;
    c.eq(prev.into(), 1)?;
    let mut ctx = Ctx::full();
    c.st(get_context(t, ctx.0.as_mut_ptr()), ok)?;
    let (start, end) = (gala_spin as *const () as u64, &raw const gala_spin_end as u64);
    let rip = ctx.get(CTX_RIP);
    c.ok((start..end).contains(&rip), rip)?;
    c.eq(ctx.get(CTX_R12), MARK)?;
    let teb = thread_basic_info(t).2;
    let (limit, base) = stack_of(teb);
    let rsp = ctx.get(CTX_RSP);
    c.ok(rsp < base && rsp + 0x1_0000 > limit, rsp)?;

    // It makes no progress while suspended, and continues once resumed.
    let frozen = counter();
    delay(SECOND / 50);
    c.eq(counter(), frozen)?;
    c.eq(resume_thread(t).1.into(), 2)?;
    delay(SECOND / 50);
    c.eq(counter(), frozen)?; // still suspended once
    c.eq(resume_thread(t).1.into(), 1)?;
    delay(SECOND / 50);
    c.ok(counter() > frozen, counter())?;

    // A new context takes effect when it resumes.
    c.st(suspend(t, &mut prev), ok)?;
    let mut ctx = Ctx::full();
    c.st(get_context(t, ctx.0.as_mut_ptr()), ok)?;
    ctx.set(CTX_RIP, redirected as *const () as u64);
    ctx.set(CTX_RCX, 0x1234);
    ctx.set(CTX_RSP, (ctx.get(CTX_RSP) & !0xf) - 8);
    c.st(set_context(t, ctx.0.as_ptr()), ok)?;
    c.eq(resume_thread(t).1.into(), 1)?;
    c.st(wait(t, Some(5 * SECOND)), ok)?;
    c.eq(REDIRECTED.load(Ordering::SeqCst), 0x1234)?;

    // Suspending an exited thread fails.
    c.st(suspend(t, &mut prev), STATUS_THREAD_IS_TERMINATING)?;
    close(t);

    // The current thread's own context.
    let mut me = Ctx::full();
    c.st(get_context(CURRENT_THREAD, me.0.as_mut_ptr()), ok)?;
    let (limit, base) = stack_of(teb_field(0x30));
    let rsp = me.get(CTX_RSP);
    c.ok(rsp < base && rsp + 0x10_0000 > limit, rsp)?;
    let image = &raw const __ImageBase as u64;
    let rip = me.get(CTX_RIP);
    c.ok(rip > image && rip < image + 0x10_0000 || rip > 0x1_8000_0000, rip)?; // ours or ntdll's

    // A thread suspended in a wait keeps the wait's result and only runs on.
    let (s, event) = create_event(EVENT_ALL_ACCESS, NOTIFICATION_EVENT, false);
    c.st(s, ok)?;
    WAITER_EVENT.store(event.0 as u64, Ordering::SeqCst);
    let (s, w, _) = create_thread(waiter, 0, false);
    c.st(s, ok)?;
    delay(SECOND / 50);
    c.st(suspend(w, &mut prev), ok)?;
    c.st(set_event(event).0, ok)?;
    delay(SECOND / 50);
    c.eq(WAITER_DONE.load(Ordering::SeqCst), 0)?;
    c.eq(resume_thread(w).1.into(), 1)?;
    c.st(wait(w, Some(5 * SECOND)), ok)?;
    c.eq(WAITER_DONE.load(Ordering::SeqCst), 1)?; // 1 + STATUS_SUCCESS
    close(w);
    close(event);
    Ok(())
}

fn main() -> u32 {
    match run() {
        Ok(()) => {
            print("context_test: ok\r\n");
            0
        }
        Err(id) => id,
    }
}

entry!(main);
