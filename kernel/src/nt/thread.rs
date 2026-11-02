//! NT threads: user threads of a process, as waitable kernel objects.
//!
//! Termination of another thread is cooperative: the target is flagged and
//! woken, and exits at its next safe point (return to user mode from a
//! system call or interrupt, or after a wait wakes up). It never exits while
//! holding kernel locks.

use super::platform;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use super::layout::{self, teb};
use super::object::{KObject, ObjectBody, ObjectRef};
use super::platform::PAGE_SIZE;
use super::platform::PageFlags;
use super::platform::ThreadId;
use super::platform::UserRegs;
use super::process::{self, Process};
use super::status::NtStatus;
use super::vad;
use super::wait::{self, Dispatcher};
use crate::mm::heap::{KArc, TryVec};
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
    /// MXCSR to start with, from the creating CONTEXT.
    start_mxcsr: Option<u32>,
    /// ThreadQuerySetWin32StartAddress.
    pub win32_start: u64,
    /// NT times (ThreadTimes); exit time 0 while running.
    pub create_time: u64,
    pub exit_time: IrqMutex<u64>,
    /// (Priority, BasePriority) as reported; stored, not yet used by the
    /// scheduler.
    pub priority: IrqMutex<(i32, i32)>,
    /// ThreadAffinityMask: the CPUs it may run on.
    affinity: core::sync::atomic::AtomicU64,
    /// Queued user APCs (FIFO), run at alertable waits.
    apcs: IrqMutex<Vec<UserApc>>,
    /// Set while the thread is in an alertable wait (an APC wakes it).
    alertable: core::sync::atomic::AtomicBool,
    /// Mutants it owns (abandoned if it exits).
    owned_mutants: IrqMutex<Vec<ObjectRef>>,
    /// While suspended on its way to user mode: the registers it will
    /// resume with (NtGet/SetContextThread from other threads).
    parked: IrqMutex<Option<Parked>>,
    /// While it is inside a system call: its user registers at the
    /// `syscall` (the context Windows reports for a thread suspended while
    /// blocked in one).
    in_syscall: IrqMutex<Option<(u32, UserRegs)>>,
    /// The impersonation token it acts with (nt::token), if any.
    pub impersonation: IrqMutex<Option<ObjectRef>>,
}

/// The user state of a thread parked by suspension.
#[derive(Clone)]
pub struct Parked {
    pub regs: UserRegs,
    pub fx: platform::FxState,
    /// Set by NtSetContextThread: resume with `regs`/`fx`.
    pub changed: bool,
}

/// The most a thread can be suspended (MAXIMUM_SUSPEND_COUNT).
const MAXIMUM_SUSPEND_COUNT: u32 = 127;

/// A user-mode APC: `routine(args[0], args[1], args[2])`.
#[derive(Clone, Copy, Debug)]
pub struct UserApc {
    pub routine: u64,
    pub args: [u64; 3],
}

/// The base priority of every process (NORMAL_PRIORITY_CLASS).
pub const PROCESS_BASE_PRIORITY: i32 = 8;

/// The base priority for a ThreadBasePriority increment over the process's
/// base: -2..=2, or saturation to idle (1) / time critical (15) with -15/15.
pub fn base_priority(increment: i32) -> Option<i32> {
    match increment {
        -2..=2 => Some(PROCESS_BASE_PRIORITY + increment),
        15 => Some(15),
        -15 => Some(1),
        _ => None,
    }
}

impl NtThread {
    pub fn affinity(&self) -> u64 {
        self.affinity.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Sets ThreadAffinityMask (validated by the caller); the thread moves
    /// to an allowed CPU at its next reschedule.
    pub fn set_affinity(&self, mask: u64) {
        self.affinity.store(mask, core::sync::atomic::Ordering::Relaxed);
        self.apply_affinity();
    }

    fn apply_affinity(&self) {
        let tid = *self.sched_id.lock();
        if let Some(tid) = tid {
            platform::set_affinity(tid, self.affinity());
        }
    }

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
    let me = platform::current();
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

/// The process of the thread with id `id`.
pub fn process_of(id: u64) -> Option<KArc<Process>> {
    by_id(id).map(|t| as_thread(&t).process.clone())
}

/// Creates a user thread in `process` that starts with `regs` once resumed
/// (if `suspended`). `stack` is `(base, limit, allocation base)` for the
/// TEB (StackBase, StackLimit, DeallocationStack).
/// `win32_start` is what ThreadQuerySetWin32StartAddress reports.
pub fn create(
    process: &KArc<Process>,
    regs: UserRegs,
    win32_start: u64,
    mxcsr: Option<u32>,
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
        affinity: core::sync::atomic::AtomicU64::new(platform::online_mask()),
        dispatcher: Dispatcher::new(0),
        exit_status: IrqMutex::new(None),
        suspend_count: IrqMutex::new(u32::from(suspended)),
        terminate_request: IrqMutex::new(None),
        start: regs,
        start_mxcsr: mxcsr,
        win32_start,
        create_time: platform::system_time(),
        exit_time: IrqMutex::new(0),
        priority: IrqMutex::new((PROCESS_BASE_PRIORITY, PROCESS_BASE_PRIORITY)),
        apcs: IrqMutex::new(Vec::new()),
        alertable: core::sync::atomic::AtomicBool::new(false),
        owned_mutants: IrqMutex::new(Vec::new()),
        parked: IrqMutex::new(None),
        in_syscall: IrqMutex::new(None),
        impersonation: IrqMutex::new(None),
    };
    let object = KObject::new(ObjectBody::Thread(thread)).map_err(|_| NtStatus::NO_MEMORY)?;
    THREADS.lock().try_push(object.clone()).map_err(|_| NtStatus::NO_MEMORY)?;
    if let Err(status) = process.add_thread(object.clone()) {
        THREADS.lock().retain(|o| !KArc::ptr_eq(o, &object));
        return Err(status);
    }
    match platform::spawn_user(
        process.name,
        thread_entry,
        id as usize,
        process.space.clone(),
        teb_addr,
    ) {
        Ok(tid) => {
            *as_thread(&object).sched_id.lock() = Some(tid);
            crate::events::emit(
                crate::events::Category::Thread,
                "thread create",
                format_args!(
                    r#""pid":{},"tid":{id},"start":{},"teb":{teb_addr}"#,
                    process.id, win32_start
                ),
            );
            Ok(object)
        }
        Err(_) => {
            THREADS.lock().retain(|o| !KArc::ptr_eq(o, &object));
            let _ = process.remove_thread(&object);
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
    let Some(object) = by_id(id as u64) else { platform::exit() };
    let t = as_thread(&object);
    *t.sched_id.lock() = Some(platform::current());
    t.apply_affinity();
    // Stay here while suspended; `resume` wakes us.
    loop {
        let count = t.suspend_count.lock();
        if *count == 0 || t.terminate_request.lock().is_some() {
            break;
        }
        platform::prepare_wait(None);
        if t.terminate_request.lock().is_some() {
            platform::wake(platform::current()); // see `wake_if_terminating`
        }
        drop(count);
        platform::wait();
    }
    let pending = *t.terminate_request.lock();
    let regs = t.start;
    let mxcsr = t.start_mxcsr;
    drop(object);
    if let Some(status) = pending {
        exit_current(status);
    }
    // The kernel never touches SSE state, so this is what user mode gets.
    if let Some(value) = mxcsr {
        platform::set_mxcsr(value);
    }
    platform::enter_user(&regs)
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
            platform::wake(tid);
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
        platform::wake(tid);
        platform::interrupt(tid);
    }
}

/// Queues a user APC to thread `object`, waking it if it is in an
/// alertable wait (NtQueueApcThread).
pub fn queue_user_apc(object: &ObjectRef, apc: UserApc) -> Result<(), NtStatus> {
    let ObjectBody::Thread(t) = &object.body else { return Err(NtStatus::OBJECT_TYPE_MISMATCH) };
    if t.has_exited() {
        return Err(NtStatus::UNSUCCESSFUL);
    }
    wait::with_dispatcher_lock(|| {
        t.apcs.lock().try_push(apc).map_err(|_| NtStatus::NO_MEMORY)?;
        if t.alertable.load(Ordering::SeqCst)
            && let Some(tid) = *t.sched_id.lock()
        {
            platform::wake(tid);
        }
        Ok(())
    })
}

/// The current thread's ClientId.UniqueThread.
pub fn current_id() -> Option<u64> {
    current().map(|o| as_thread(&o).id)
}

/// Records that the current thread now owns `mutant`.
pub fn note_mutant_owned(mutant: &ObjectRef) {
    if let Some(o) = current() {
        // Without memory for the record, abandonment is missed; the mutant
        // still works.
        let _ = as_thread(&o).owned_mutants.lock().try_push(mutant.clone());
    }
}

/// Records that the current thread released `mutant` completely.
pub fn note_mutant_released(mutant: &ObjectRef) {
    if let Some(o) = current() {
        as_thread(&o).owned_mutants.lock().retain(|m| !KArc::ptr_eq(m, mutant));
    }
}

/// Abandons the mutants a dying thread still owns: they become free and
/// the next acquirer is told (STATUS_ABANDONED_WAIT_n).
fn abandon_mutants(t: &NtThread) {
    let owned = core::mem::take(&mut *t.owned_mutants.lock());
    for object in owned {
        if let ObjectBody::Mutant(m) = &object.body {
            {
                let mut state = m.state.lock();
                if state.owner != Some(t.id) {
                    continue;
                }
                *state = super::object::MutantState { owner: None, count: 0, abandoned: true };
            }
            wait::set_signal(&object, 1);
        }
    }
}

/// Whether the current thread has a user APC queued.
pub fn has_user_apc() -> bool {
    current().is_some_and(|o| !as_thread(&o).apcs.lock().is_empty())
}

/// Takes the current thread's next user APC.
pub fn take_user_apc() -> Option<UserApc> {
    let object = current()?;
    let mut apcs = as_thread(&object).apcs.lock();
    (!apcs.is_empty()).then(|| apcs.remove(0))
}

/// Marks the current thread as in (or out of) an alertable wait.
pub fn set_alertable(on: bool) {
    if let Some(o) = current() {
        as_thread(&o).alertable.store(on, Ordering::SeqCst);
    }
}

/// Whether the current thread has been asked to terminate.
pub fn is_terminating() -> bool {
    current().is_some_and(|o| as_thread(&o).terminate_request.lock().is_some())
}

/// For waits that termination must end: call right after
/// `platform::prepare_wait`. `request_termination` sets the request, then
/// wakes the thread, which only works once it is marked waiting; checking
/// here, after `prepare_wait`, closes the gap: a request made before this
/// check is seen (the thread wakes itself, so `wait` returns at once), one
/// made after finds it waiting. (Without it a thread asked to terminate
/// just before it slept slept forever: Go's ExitProcess then never ended
/// the process.)
pub fn wake_if_terminating() {
    if is_terminating() {
        platform::wake(platform::current());
    }
}

/// Runs on every return to user mode: acts on a pending termination.
fn on_user_return(regs: &mut UserRegs) -> bool {
    let Some(object) = current() else { return false };
    let t = as_thread(&object);
    let terminate = *t.terminate_request.lock();
    if let Some(status) = terminate {
        drop(object);
        exit_current(status);
    }
    if *t.suspend_count.lock() == 0 {
        return false;
    }
    // Suspended: wait here, with the user state where other threads can
    // read and change it, until resumed (or terminated).
    *t.parked.lock() = Some(Parked { regs: *regs, fx: platform::fxsave(), changed: false });
    loop {
        let count = t.suspend_count.lock();
        if *count == 0 || t.terminate_request.lock().is_some() {
            break;
        }
        platform::prepare_wait(None);
        if t.terminate_request.lock().is_some() {
            platform::wake(platform::current()); // see `wake_if_terminating`
        }
        drop(count);
        platform::wait();
    }
    let parked = t.parked.lock().take();
    let terminate = *t.terminate_request.lock();
    if let Some(status) = terminate {
        drop(object);
        exit_current(status);
    }
    match parked {
        Some(p) if p.changed => {
            *regs = p.regs;
            platform::fxrstor(&p.fx);
            true
        }
        _ => false,
    }
}

/// NtSuspendThread: raises the suspend count (the thread stops on its way
/// back to user mode). Returns the previous count.
pub fn suspend(object: &ObjectRef) -> Result<u32, NtStatus> {
    let ObjectBody::Thread(t) = &object.body else { return Err(NtStatus::OBJECT_TYPE_MISMATCH) };
    if t.has_exited() || t.terminate_request.lock().is_some() {
        return Err(NtStatus::THREAD_IS_TERMINATING);
    }
    let mut count = t.suspend_count.lock();
    if *count >= MAXIMUM_SUSPEND_COUNT {
        return Err(NtStatus::SUSPEND_COUNT_EXCEEDED);
    }
    let previous = *count;
    *count += 1;
    drop(count);
    // A thread running user code stops at its next pass through the hook.
    if let Some(tid) = *t.sched_id.lock() {
        platform::interrupt(tid);
    }
    Ok(previous)
}

/// Records (or clears) the current thread's user registers while it is in
/// a system call (the dispatcher calls this around every service).
pub fn set_in_syscall(object: &ObjectRef, regs: Option<(u32, UserRegs)>) {
    *as_thread(object).in_syscall.lock() = regs;
}

/// The user state of a suspended thread: parked on its way to user mode,
/// or, while it is inside a system call (blocked in a wait, say), its
/// registers at the `syscall` with the default floating-point state.
/// Waits up to about a second for one of the two.
pub fn parked_state(object: &ObjectRef) -> Result<Parked, NtStatus> {
    let ObjectBody::Thread(t) = &object.body else { return Err(NtStatus::OBJECT_TYPE_MISMATCH) };
    for _ in 0..1000 {
        if let Some(p) = t.parked.lock().clone() {
            return Ok(p);
        }
        if *t.suspend_count.lock() == 0 || t.has_exited() {
            break;
        }
        if let Some((_, regs)) = *t.in_syscall.lock() {
            return Ok(Parked { regs, fx: default_fx(), changed: false });
        }
        platform::sleep_ms(1);
    }
    // TODO: the context of a thread that is not suspended (Windows
    // interrupts it with a kernel APC).
    Err(NtStatus::UNSUCCESSFUL)
}

/// Logs every NT thread: its process, suspend count, and the system call
/// it is in (its user rip, and for a wait the object, its signal state, a
/// timer's due tick and the timeout) or that it runs user code. For hangs
/// (the kernel's `run=` dumps this when a program takes over a minute).
pub fn dump() {
    let threads = THREADS.lock().clone();
    for object in threads.iter() {
        let t = as_thread(object);
        let in_syscall = *t.in_syscall.lock();
        info!(
            "thread: pid {} tid {} (sched {:?}) suspend {} exited {} in a system call {}",
            t.process.id,
            t.id,
            *t.sched_id.lock(),
            *t.suspend_count.lock(),
            t.has_exited(),
            in_syscall.is_some()
        );
        let Some((number, regs)) = in_syscall else {
            // Running user code: where a tick last caught it.
            if let Some(rip) = (*t.sched_id.lock()).and_then(platform::last_user_rip) {
                info!("thread:   in user mode at rip {rip:#x} (last tick)");
            }
            continue;
        };
        let name = super::syscalls::service_name(number);
        info!("thread:   {name} from rip {:#x}", regs.rip);
        // The first argument (r10), as a handle: what a wait is on.
        let handle = super::object::Handle(regs.gpr[10]);
        if let Some(e) = t.process.handles.lock().get(handle) {
            let signal = e.object.dispatcher().map_or(-1, |d| d.signal_state());
            info!("thread:   on handle {:#x}: {} signal {signal}", handle.0, e.object.type_name());
            if let ObjectBody::Timer(timer) = &e.object.body {
                info!(
                    "thread:   timer due tick {:?} (now {})",
                    timer.state.lock().due,
                    platform::ticks()
                );
            }
        }
        // NtWaitForSingleObject's timeout (r8: *LARGE_INTEGER).
        if name == "NtWaitForSingleObject" && regs.gpr[8] != 0 {
            info!("thread:   timeout {:?}", t.process.read_u64(regs.gpr[8]).ok().map(|v| v as i64));
        }
    }
}

/// The floating-point state after reset (FCW 0x37f, MXCSR 0x1f80).
fn default_fx() -> platform::FxState {
    let mut fx = platform::FxState([0; 512]);
    fx.0[0..2].copy_from_slice(&0x37fu16.to_le_bytes());
    fx.0[24..28].copy_from_slice(&0x1f80u32.to_le_bytes());
    fx
}

/// Replaces the parked user state of a suspended thread (applied when it
/// resumes).
pub fn set_parked_state(
    object: &ObjectRef,
    regs: UserRegs,
    fx: Option<platform::FxState>,
) -> Result<(), NtStatus> {
    let ObjectBody::Thread(t) = &object.body else { return Err(NtStatus::OBJECT_TYPE_MISMATCH) };
    parked_state(object)?;
    let mut parked = t.parked.lock();
    let p = parked.as_mut().ok_or(NtStatus::UNSUCCESSFUL)?;
    p.regs = regs;
    if let Some(fx) = fx {
        p.fx = fx;
    }
    p.changed = true;
    Ok(())
}

/// Ends the current thread with `status`. If it was the last thread of its
/// process, the process ends too.
/// Whether the thread's process is ending (all its threads go with it).
fn process_is_exiting(t: &NtThread) -> bool {
    t.process.exit_status().is_some()
}

pub fn exit_current(status: NtStatus) -> ! {
    // Everything referenced here must be dropped before `platform::exit`,
    // which never returns.
    if let Some(object) = current() {
        let t = as_thread(&object);
        t.exit_status.lock().get_or_insert(status);
        crate::events::emit(
            crate::events::Category::Thread,
            "thread exit",
            format_args!(r#""pid":{},"tid":{},"status":{}"#, t.process.id, t.id, status.0),
        );
        *t.exit_time.lock() = platform::system_time();
        if status != NtStatus::SUCCESS && !process_is_exiting(t) {
            // A thread dying alone with an error is worth a line (a hang
            // that follows is often its consequence).
            info!(
                "thread: pid {} tid {} exited with status {:#010x}",
                t.process.id, t.id, status.0
            );
        }
        // Leave the process before waking waiters: once the thread object
        // is signaled, the thread must no longer count as live (e.g. for
        // ThreadAmILastThread).
        abandon_mutants(t);
        THREADS.lock().retain(|o| !KArc::ptr_eq(o, &object));
        let process = t.process.clone();
        let last = process.remove_thread(&object);
        wait::set_signal(&object, 1);
        drop(object);
        // Exactly one of several threads exiting at once on different CPUs
        // is the last one (decided under the process's thread lock).
        if last {
            process::finish(&process, status);
        }
    }
    platform::exit()
}

/// Makes the current thread exit with `status` on its way back to user
/// mode (so the caller can unwind and drop its references first).
pub fn exit_current_on_return(status: NtStatus) {
    if let Some(object) = current() {
        as_thread(&object).terminate_request.lock().get_or_insert(status);
    }
}

pub fn init() {
    platform::set_user_return_hook(on_user_return);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_priority_increments() {
        assert_eq!(base_priority(0), Some(8));
        assert_eq!(base_priority(2), Some(10));
        assert_eq!(base_priority(-2), Some(6));
        assert_eq!(base_priority(15), Some(15));
        assert_eq!(base_priority(-15), Some(1));
        assert_eq!(base_priority(3), None);
    }
}
