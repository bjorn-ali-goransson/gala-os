//! 32-bit threads under WOW64 (ARCHITECTURE "Plan: WOW64", ROADMAP WOW
//! stage 2), with the results 64-bit Windows of the pinned build gives a
//! 32-bit program:
//!
//! - NtCreateThread from 32-bit code with an x86 CONTEXT (CONTEXT_FULL,
//!   Windows' selectors) and an x86 INITIAL_TEB: the thread runs 32-bit
//!   code (CS 0x23) on the given stack, with its own TEB32 (Self, its
//!   ClientId as returned in the x86 CLIENT_ID, StackBase the given one);
//!   it ends with NtTerminateThread, whose status ThreadBasicInformation
//!   (x86, 28 bytes) reports with its TEB32.
//! - A thread created suspended: NtGetContextThread gives the x86 CONTEXT
//!   it was created with (SegCs 0x23, Eip, Esp, Ebx).
//! - A running thread spinning in 32-bit code, suspended (previous count
//!   0): its context is in the spin loop (SegCs 0x23), it stops running;
//!   NtSetContextThread moves it to another function, which ends it with
//!   0x77 once resumed (previous count 1).
//! - A thread blocked in a system call, suspended: its context is the
//!   32-bit caller's (in this image, on its stack), and it finishes its
//!   wait once resumed.
//! - The calling thread's own context: SegCs 0x23, on its stack.
//! - Sixteen threads one after another (each runs, sees its argument and
//!   a TEB32 of its own, and exits).
//!
//! Prints `wow64_thread_test: ok` and exits with 0, or exits with the
//! number of the failing check (`(line 0x...) wow64_thread_test: check N
//! failed`: its source line in hex).

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicUsize, Ordering};

use ntrt::*;

unsafe extern "C" {
    /// The linker-defined start of this image.
    static __ImageBase: u8;
}

const STATUS_PENDING: u32 = 0x103;
const CURRENT_THREAD: usize = usize::MAX - 1;
const THREAD_ALL_ACCESS: usize = 0x1f_ffff;
/// CONTEXT_i386 | CONTEXT_CONTROL | CONTEXT_INTEGER | CONTEXT_SEGMENTS.
const CONTEXT_FULL: u32 = 0x1_0007;
const EXIT_A: u32 = 0x1234_0042;

/// x86 CONTEXT offsets (winnt.h).
mod ctx {
    pub const SIZE: usize = 0x2cc;
    pub const FLAGS: usize = 0x00;
    pub const SEG_GS: usize = 0x8c;
    pub const SEG_FS: usize = 0x90;
    pub const SEG_ES: usize = 0x94;
    pub const SEG_DS: usize = 0x98;
    pub const EBX: usize = 0xa4;
    pub const EIP: usize = 0xb8;
    pub const SEG_CS: usize = 0xbc;
    pub const EFLAGS: usize = 0xc0;
    pub const ESP: usize = 0xc4;
    pub const SEG_SS: usize = 0xc8;
}

struct Checks(u32);

impl Checks {
    #[track_caller]
    fn eq(&mut self, got: u64, want: u64) -> Result<(), u32> {
        self.ok(got == want, got)
    }

    /// A failure names its check, the source line and what was got.
    #[track_caller]
    fn ok(&mut self, cond: bool, got: u64) -> Result<(), u32> {
        self.0 += 1;
        if cond {
            return Ok(());
        }
        let mut at = *b"(line 0x00000000) ";
        at[8..16].copy_from_slice(&hex32(core::panic::Location::caller().line()));
        if let Ok(text) = core::str::from_utf8(&at) {
            print(text);
        }
        let mut line = *b"wow64_thread_test: check 00000000 failed, got 00000000 00000000\n";
        line[25..33].copy_from_slice(&hex32(self.0));
        line[46..54].copy_from_slice(&hex32((got >> 32) as u32));
        line[55..63].copy_from_slice(&hex32(got as u32));
        if let Ok(text) = core::str::from_utf8(&line) {
            print(text);
        }
        Err(self.0)
    }
}

fn sys(n: u32, args: &[usize]) -> u32 {
    // SAFETY: the callers pass what each service expects.
    unsafe { syscall(n, args) }
}

fn mptr<T>(v: &mut T) -> usize {
    v as *mut T as usize
}

fn ptr<T>(v: &T) -> usize {
    v as *const T as usize
}

fn get(c: &[u8; ctx::SIZE], at: usize) -> u32 {
    u32::from_le_bytes([c[at], c[at + 1], c[at + 2], c[at + 3]])
}

fn put(c: &mut [u8; ctx::SIZE], at: usize, v: u32) {
    c[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// What thread A saw of itself.
static A_CS: AtomicUsize = AtomicUsize::new(0);
static A_TEB: AtomicUsize = AtomicUsize::new(0);
static A_TID: AtomicUsize = AtomicUsize::new(0);
static A_ESP: AtomicUsize = AtomicUsize::new(0);
static A_STACK_BASE: AtomicUsize = AtomicUsize::new(0);
static A_PARAM: AtomicUsize = AtomicUsize::new(0);
/// Thread B's spin count.
static SPINS: AtomicUsize = AtomicUsize::new(0);

fn terminate_thread(status: u32) -> ! {
    sys(nr::NT_TERMINATE_THREAD, &[CURRENT_THREAD, status as usize]);
    loop {
        core::hint::spin_loop();
    }
}

extern "C" fn thread_a(param: usize) -> ! {
    A_CS.store(code_segment().into(), Ordering::SeqCst);
    A_TEB.store(teb_field(teb::SELF), Ordering::SeqCst);
    A_TID.store(teb_field(teb::CLIENT_ID_THREAD), Ordering::SeqCst);
    A_ESP.store(stack_pointer(), Ordering::SeqCst);
    A_STACK_BASE.store(teb_field(teb::STACK_BASE), Ordering::SeqCst);
    A_PARAM.store(param, Ordering::SeqCst);
    terminate_thread(EXIT_A)
}

extern "C" fn spin(_param: usize) -> ! {
    loop {
        SPINS.fetch_add(1, Ordering::SeqCst);
    }
}

extern "C" fn exit_77(_param: usize) -> ! {
    terminate_thread(0x77)
}

extern "C" fn blocker(event: usize) -> ! {
    let status = sys(nr::NT_WAIT_FOR_SINGLE_OBJECT, &[event, 0, 0]);
    terminate_thread(status)
}

/// A new thread's stack and start.
struct Started {
    status: u32,
    handle: Handle,
    /// CLIENT_ID (x86): process, thread.
    cid: [usize; 2],
    /// Its stack: [bottom, top); `esp` as created.
    bottom: usize,
    top: usize,
    esp: usize,
}

const STACK: usize = 0x1_0000;

fn create(entry: usize, param: usize, suspended: bool) -> Started {
    let (status, bottom, _) = allocate(0, STACK, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    let mut s = Started {
        status,
        handle: Handle(0),
        cid: [0; 2],
        bottom,
        top: bottom + STACK,
        esp: bottom + STACK - 16,
    };
    if status != STATUS_SUCCESS {
        return s;
    }
    // SAFETY: the stack is committed: a null return address, the argument.
    unsafe {
        core::ptr::write_volatile(s.esp as *mut usize, 0);
        core::ptr::write_volatile((s.esp + 4) as *mut usize, param);
    }
    let mut c = [0u8; ctx::SIZE];
    put(&mut c, ctx::FLAGS, CONTEXT_FULL);
    for (at, v) in [
        (ctx::SEG_GS, 0x2b),
        (ctx::SEG_FS, 0x53),
        (ctx::SEG_ES, 0x2b),
        (ctx::SEG_DS, 0x2b),
        (ctx::SEG_CS, 0x23),
        (ctx::SEG_SS, 0x2b),
        (ctx::EBX, 0x0b0b_0b0b),
        (ctx::EIP, entry as u32),
        (ctx::ESP, s.esp as u32),
        (ctx::EFLAGS, 0x202),
    ] {
        put(&mut c, at, v);
    }
    // INITIAL_TEB (x86): OldStackBase, OldStackLimit, StackBase,
    // StackLimit, StackAllocationBase.
    let initial = [0, 0, s.top, bottom, bottom];
    let args = [
        mptr(&mut s.handle),
        THREAD_ALL_ACCESS,
        0,
        CURRENT_PROCESS.0,
        mptr(&mut s.cid),
        ptr(&c),
        ptr(&initial),
        usize::from(suspended),
    ];
    s.status = sys(nr::NT_CREATE_THREAD, &args);
    s
}

fn wait(h: Handle) -> u32 {
    let ten = -100_000_000i64;
    sys(nr::NT_WAIT_FOR_SINGLE_OBJECT, &[h.0, 0, ptr(&ten)])
}

fn delay_ms(ms: i64) {
    let t = -10_000 * ms;
    sys(nr::NT_DELAY_EXECUTION, &[0, ptr(&t)]);
}

/// THREAD_BASIC_INFORMATION (x86): ExitStatus, TebBaseAddress, ClientId.
fn basic(h: Handle) -> (u32, [usize; 7], u32) {
    let mut info = [0usize; 7];
    let mut returned = 0u32;
    let args = [h.0, 0, mptr(&mut info), 28, mptr(&mut returned)];
    let status = sys(nr::NT_QUERY_INFORMATION_THREAD, &args);
    (status, info, returned)
}

fn context(h: Handle) -> (u32, [u8; ctx::SIZE]) {
    let mut c = [0u8; ctx::SIZE];
    put(&mut c, ctx::FLAGS, CONTEXT_FULL);
    let status = sys(nr::NT_GET_CONTEXT_THREAD, &[h.0, mptr(&mut c)]);
    (status, c)
}

fn run(c: &mut Checks) -> Result<(), u32> {
    let image = &raw const __ImageBase as usize;
    // SAFETY: the headers of this image are mapped: e_lfanew, SizeOfImage.
    let image_end = unsafe {
        let nt = image + core::ptr::read_unaligned((image + 0x3c) as *const u32) as usize;
        image + core::ptr::read_unaligned((nt + 0x50) as *const u32) as usize
    };
    let in_image = |eip: u32| (image..image_end).contains(&(eip as usize));
    let main_teb = teb_field(teb::SELF);

    // A thread that runs and exits.
    let a = create(thread_a as *const () as usize, 0x5157, false);
    c.eq(a.status.into(), 0)?;
    c.eq(a.cid[0] as u64, teb_field(teb::CLIENT_ID_PROCESS) as u64)?;
    c.ok(a.cid[1] != 0 && a.cid[1] != teb_field(teb::CLIENT_ID_THREAD), a.cid[1] as u64)?;
    c.eq(wait(a.handle).into(), 0)?;
    c.eq(A_CS.load(Ordering::SeqCst) as u64, 0x23)?;
    let teb_a = A_TEB.load(Ordering::SeqCst);
    c.ok(teb_a != 0 && teb_a != main_teb, teb_a as u64)?;
    c.eq(A_TID.load(Ordering::SeqCst) as u64, a.cid[1] as u64)?;
    c.eq(A_STACK_BASE.load(Ordering::SeqCst) as u64, a.top as u64)?;
    let esp = A_ESP.load(Ordering::SeqCst);
    c.ok(a.bottom < esp && esp < a.top, esp as u64)?;
    c.eq(A_PARAM.load(Ordering::SeqCst) as u64, 0x5157)?;
    let (status, info, returned) = basic(a.handle);
    c.eq(status.into(), 0)?;
    c.eq(returned.into(), 28)?;
    c.eq(info[0] as u64, EXIT_A.into())?;
    c.eq(info[1] as u64, teb_a as u64)?;
    c.eq(info[3] as u64, a.cid[1] as u64)?;
    c.eq(close(a.handle).into(), 0)?;

    // A thread created suspended: its initial context.
    let b = create(spin as *const () as usize, 0, true);
    c.eq(b.status.into(), 0)?;
    let (status, ctx_b) = context(b.handle);
    c.eq(status.into(), 0)?;
    c.eq(get(&ctx_b, ctx::SEG_CS).into(), 0x23)?;
    c.eq(get(&ctx_b, ctx::EIP).into(), spin as *const () as usize as u64)?;
    c.eq(get(&ctx_b, ctx::ESP).into(), b.esp as u64)?;
    c.eq(get(&ctx_b, ctx::EBX).into(), 0x0b0b_0b0b)?;
    let (status, info, _) = basic(b.handle);
    c.eq(status.into(), 0)?;
    c.eq(info[0] as u64, STATUS_PENDING.into())?;

    // Running in 32-bit code, then suspended there.
    let mut previous = 0xffu32;
    c.eq(sys(nr::NT_RESUME_THREAD, &[b.handle.0, mptr(&mut previous)]).into(), 0)?;
    c.eq(previous.into(), 1)?;
    let mut tries = 0;
    while SPINS.load(Ordering::SeqCst) == 0 && tries < 500 {
        delay_ms(1);
        tries += 1;
    }
    c.ok(SPINS.load(Ordering::SeqCst) != 0, tries)?;
    c.eq(sys(nr::NT_SUSPEND_THREAD, &[b.handle.0, mptr(&mut previous)]).into(), 0)?;
    c.eq(previous.into(), 0)?;
    let (status, mut ctx_b) = context(b.handle);
    c.eq(status.into(), 0)?;
    c.eq(get(&ctx_b, ctx::SEG_CS).into(), 0x23)?;
    let eip = get(&ctx_b, ctx::EIP) as usize;
    let spin_at = spin as *const () as usize;
    c.ok(spin_at <= eip && eip < spin_at + 0x100, eip as u64)?;
    let before = SPINS.load(Ordering::SeqCst);
    delay_ms(20);
    c.eq(SPINS.load(Ordering::SeqCst) as u64, before as u64)?;
    // Moved elsewhere: it ends there once resumed.
    put(&mut ctx_b, ctx::EIP, exit_77 as *const () as usize as u32);
    put(&mut ctx_b, ctx::ESP, b.esp as u32);
    c.eq(sys(nr::NT_SET_CONTEXT_THREAD, &[b.handle.0, ptr(&ctx_b)]).into(), 0)?;
    c.eq(sys(nr::NT_RESUME_THREAD, &[b.handle.0, mptr(&mut previous)]).into(), 0)?;
    c.eq(previous.into(), 1)?;
    c.eq(wait(b.handle).into(), 0)?;
    let (_, info, _) = basic(b.handle);
    c.eq(info[0] as u64, 0x77)?;
    c.eq(close(b.handle).into(), 0)?;

    // Blocked in a system call, then suspended.
    let mut event = Handle(0);
    let args = [mptr(&mut event), 0x1f_0003, 0, 0, 0];
    c.eq(sys(nr::NT_CREATE_EVENT, &args).into(), 0)?;
    let t = create(blocker as *const () as usize, event.0, false);
    c.eq(t.status.into(), 0)?;
    delay_ms(50);
    c.eq(sys(nr::NT_SUSPEND_THREAD, &[t.handle.0, mptr(&mut previous)]).into(), 0)?;
    let (status, ctx_t) = context(t.handle);
    c.eq(status.into(), 0)?;
    c.eq(get(&ctx_t, ctx::SEG_CS).into(), 0x23)?;
    c.ok(in_image(get(&ctx_t, ctx::EIP)), get(&ctx_t, ctx::EIP).into())?;
    let esp = get(&ctx_t, ctx::ESP) as usize;
    c.ok(t.bottom < esp && esp < t.top, esp as u64)?;
    c.eq(sys(nr::NT_SET_EVENT, &[event.0, 0]).into(), 0)?;
    c.eq(sys(nr::NT_RESUME_THREAD, &[t.handle.0, mptr(&mut previous)]).into(), 0)?;
    c.eq(wait(t.handle).into(), 0)?;
    let (_, info, _) = basic(t.handle);
    c.eq(info[0] as u64, 0)?;
    c.eq(close(t.handle).into(), 0)?;
    c.eq(close(event).into(), 0)?;

    // This thread's own context.
    let (status, own) = context(Handle(CURRENT_THREAD));
    c.eq(status.into(), 0)?;
    c.eq(get(&own, ctx::SEG_CS).into(), 0x23)?;
    c.ok(in_image(get(&own, ctx::EIP)), get(&own, ctx::EIP).into())?;
    let esp = get(&own, ctx::ESP) as usize;
    let (limit, base) = (teb_field(teb::STACK_LIMIT), teb_field(teb::STACK_BASE));
    c.ok(limit < esp && esp < base, esp as u64)?;

    // Threads one after another, each with its own TEB32.
    for i in 0..16 {
        let t = create(thread_a as *const () as usize, i, false);
        c.eq(t.status.into(), 0)?;
        c.eq(wait(t.handle).into(), 0)?;
        let teb = A_TEB.load(Ordering::SeqCst);
        c.ok(teb != 0 && teb != main_teb && A_PARAM.load(Ordering::SeqCst) == i, teb as u64)?;
        let (_, info, _) = basic(t.handle);
        c.eq(info[0] as u64, EXIT_A.into())?;
        c.eq(close(t.handle).into(), 0)?;
        let (status, _, _) = free(t.bottom, 0, MEM_RELEASE);
        c.eq(status.into(), 0)?;
    }
    Ok(())
}

fn main() -> u32 {
    let mut c = Checks(0);
    match run(&mut c) {
        Ok(()) => {
            print("wow64_thread_test: ok\n");
            0
        }
        Err(n) => n,
    }
}

entry!(main);
