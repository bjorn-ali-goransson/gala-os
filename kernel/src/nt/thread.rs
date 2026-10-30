//! NT threads: user threads of a process, as waitable kernel objects.
//!
//! Termination of another thread is cooperative: the target is flagged and
//! woken, and exits at its next safe point (return to user mode from a
//! system call or interrupt, or after a wait wakes up). It never exits while
//! holding kernel locks.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use super::layout::{self, teb};
use super::object::{KObject, ObjectBody, ObjectRef};
use super::process::{self, Process};
use super::status::NtStatus;
use super::vad;
use super::wait::{self, Dispatcher};
use crate::arch::syscall::{self, UserRegs};
use crate::mm::PAGE_SIZE;
use crate::mm::heap::{KArc, TryVec};
use crate::mm::paging::PageFlags;
use crate::sched::{self, ThreadId};
use crate::sync::IrqMutex;

pub struct NtThread {
    /// ClientId.UniqueThread.
    pub id: u64,
    pub teb: u64,
    pub process: KArc<Process>,
    /// Scheduler thread, once spawned.
    sched_id: IrqMutex<Option<ThreadId>>,
    /// Signaled when the thread has exited.
    pub dispatcher: Dispatcher,
    exit_status: IrqMutex<Option<NtStatus>>,
    suspend_count: IrqMutex<u32>,
    /// Set when another thread asked this one to terminate.
    terminate_request: IrqMutex<Option<NtStatus>>,
    start: UserRegs,
}

impl NtThread {
    /// ExitStatus as reported by NtQueryInformationThread.
    pub fn exit_status(&self) -> NtStatus {
        self.exit_status.lock().unwrap_or(NtStatus::PENDING)
    }

    pub fn has_exited(&self) -> bool {
        self.exit_status.lock().is_some()
    }
}

/// Every live NT thread. Lookup by scheduler id or ClientId.
static THREADS: IrqMutex<Vec<ObjectRef>> = IrqMutex::new(Vec::new());
/// NT thread and process ids share one sequence (multiples of 4).
static NEXT_ID: AtomicU64 = AtomicU64::new(4);

pub fn next_id() -> u64 {
    NEXT_ID.fetch_add(4, Ordering::Relaxed)
}

fn as_thread(object: &ObjectRef) -> &NtThread {
    match &object.body {
        ObjectBody::Thread(t) => t,
        _ => unreachable!("THREADS holds thread objects only"),
    }
}

/// The NT thread object of the running thread (None for kernel threads).
pub fn current() -> Option<ObjectRef> {
    let me = sched::current();
    THREADS.lock().iter().find(|o| *as_thread(o).sched_id.lock() == Some(me)).cloned()
}

/// The current thread's TEB and its stack reservation `[bottom, base)`
/// (DeallocationStack, StackBase), if it is a thread of `process`.
pub fn current_stack(process: &Process) -> Option<(u64, u64, u64)> {
    let object = current()?;
    let t = as_thread(&object);
    if !core::ptr::eq(&*t.process, process) {
        return None;
    }
    let mut space = process.space.lock();
    let mut word = |off: u64| {
        let mut b = [0u8; 8];
        space.read(t.teb + off, &mut b).ok().map(|()| u64::from_le_bytes(b))
    };
    let base = word(teb::STACK_BASE)?;
    let bottom = word(teb::DEALLOCATION_STACK)?;
    (bottom != 0 && bottom < base).then_some((t.teb, bottom, base))
}

fn by_id(id: u64) -> Option<ObjectRef> {
    THREADS.lock().iter().find(|o| as_thread(o).id == id).cloned()
}

/// Creates a user thread in `process` that starts with `regs` once resumed
/// (if `suspended`). `stack` is `(base, limit, allocation base)` for the
/// TEB (StackBase, StackLimit, DeallocationStack).
pub fn create(
    process: &KArc<Process>,
    regs: UserRegs,
    stack: (u64, u64, u64),
    suspended: bool,
) -> Result<ObjectRef, NtStatus> {
    if !regs.is_valid() {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    let id = next_id();
    let teb_addr = allocate_teb(process, id, stack)?;
    let thread = NtThread {
        id,
        teb: teb_addr,
        process: process.clone(),
        sched_id: IrqMutex::new(None),
        dispatcher: Dispatcher::new(0),
        exit_status: IrqMutex::new(None),
        suspend_count: IrqMutex::new(u32::from(suspended)),
        terminate_request: IrqMutex::new(None),
        start: regs,
    };
    let object = KObject::new(ObjectBody::Thread(thread)).map_err(|_| NtStatus::NO_MEMORY)?;
    THREADS.lock().try_push(object.clone()).map_err(|_| NtStatus::NO_MEMORY)?;
    if process.add_thread(object.clone()).is_err() {
        THREADS.lock().retain(|o| !KArc::ptr_eq(o, &object));
        return Err(NtStatus::NO_MEMORY);
    }
    match sched::spawn_user(
        process.name,
        thread_entry,
        id as usize,
        process.space.clone(),
        teb_addr,
    ) {
        Ok(tid) => {
            *as_thread(&object).sched_id.lock() = Some(tid);
            Ok(object)
        }
        Err(_) => {
            THREADS.lock().retain(|o| !KArc::ptr_eq(o, &object));
            process.remove_thread(&object);
            Err(NtStatus::NO_MEMORY)
        }
    }
}

/// Reserves, maps and fills a TEB for thread `id`.
fn allocate_teb(
    process: &Process,
    id: u64,
    (stack_base, stack_limit, stack_bottom): (u64, u64, u64),
) -> Result<u64, NtStatus> {
    let size = teb::PAGES * PAGE_SIZE;
    let addr = {
        let mut vad = process.vad.lock();
        let mut addr = layout::FIRST_TEB_ADDRESS;
        loop {
            match vad.reserve(Some(addr), size, vad::PAGE_READWRITE, vad::MEM_PRIVATE, false) {
                Ok(_) => break,
                Err(NtStatus::CONFLICTING_ADDRESSES) => addr += vad::ALLOCATION_GRANULARITY,
                Err(e) => return Err(e),
            }
        }
        vad.commit(addr, size, vad::PAGE_READWRITE)?;
        addr
    };
    let flags = PageFlags { user: true, global: false, ..PageFlags::KERNEL_RW };
    process.space.lock().map_zeroed(addr, teb::PAGES, flags).map_err(|_| NtStatus::NO_MEMORY)?;
    let put = |off: u64, v: u64| process.write_u64(addr + off, v).map_err(|_| NtStatus::NO_MEMORY);
    put(teb::STACK_BASE, stack_base)?;
    put(teb::STACK_LIMIT, stack_limit)?;
    put(teb::DEALLOCATION_STACK, stack_bottom)?;
    put(teb::SELF, addr)?;
    put(teb::CLIENT_ID_PROCESS, process.id)?;
    put(teb::CLIENT_ID_THREAD, id)?;
    put(teb::PEB, layout::PEB_ADDRESS)?;
    Ok(addr)
}

fn thread_entry(id: usize) {
    let Some(object) = by_id(id as u64) else { sched::exit() };
    let t = as_thread(&object);
    *t.sched_id.lock() = Some(sched::current());
    // Stay here while suspended; `resume` wakes us.
    loop {
        let count = t.suspend_count.lock();
        if *count == 0 || t.terminate_request.lock().is_some() {
            break;
        }
        sched::prepare_wait(None);
        drop(count);
        sched::wait();
    }
    let pending = *t.terminate_request.lock();
    let regs = t.start;
    drop(object);
    if let Some(status) = pending {
        exit_current(status);
    }
    syscall::enter_user_regs(&regs)
}

/// NtResumeThread: returns the previous suspend count.
pub fn resume(object: &ObjectRef) -> u32 {
    let ObjectBody::Thread(t) = &object.body else { return 0 };
    let mut count = t.suspend_count.lock();
    let previous = *count;
    if *count > 0 {
        *count -= 1;
        if *count == 0
            && let Some(tid) = *t.sched_id.lock()
        {
            sched::wake(tid);
        }
    }
    previous
}

/// Asks thread `object` to terminate with `status` (another thread).
pub fn request_termination(object: &ObjectRef, status: NtStatus) {
    let ObjectBody::Thread(t) = &object.body else { return };
    if t.has_exited() {
        return;
    }
    t.terminate_request.lock().get_or_insert(status);
    if let Some(tid) = *t.sched_id.lock() {
        sched::wake(tid);
    }
}

/// Whether the current thread has been asked to terminate.
pub fn is_terminating() -> bool {
    current().is_some_and(|o| as_thread(&o).terminate_request.lock().is_some())
}

/// Runs on every return to user mode: acts on a pending termination.
fn on_user_return() {
    let pending = current().and_then(|o| *as_thread(&o).terminate_request.lock());
    if let Some(status) = pending {
        exit_current(status);
    }
}

/// Ends the current thread with `status`. If it was the last thread of its
/// process, the process ends too.
pub fn exit_current(status: NtStatus) -> ! {
    // Everything referenced here must be dropped before `sched::exit`,
    // which never returns.
    if let Some(object) = current() {
        let t = as_thread(&object);
        t.exit_status.lock().get_or_insert(status);
        wait::set_signal(&object, 1);
        THREADS.lock().retain(|o| !KArc::ptr_eq(o, &object));
        let process = t.process.clone();
        process.remove_thread(&object);
        drop(object);
        if process.thread_count() == 0 {
            process::finish(&process, status);
        }
    }
    sched::exit()
}

/// Makes the current thread exit with `status` on its way back to user
/// mode (so the caller can unwind and drop its references first).
pub fn exit_current_on_return(status: NtStatus) {
    if let Some(object) = current() {
        as_thread(&object).terminate_request.lock().get_or_insert(status);
    }
}

pub fn init() {
    syscall::set_user_return_hook(on_user_return);
}
