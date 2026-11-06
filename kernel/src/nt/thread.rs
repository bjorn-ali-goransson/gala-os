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
    /// Queued user APCs; room is kept for the completion routines of I/O
    /// requests in progress (`notify::Reserved`).
    apcs: IrqMutex<crate::mm::heap::ReservedVec<UserApc>>,
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
    /// Its number in its process, in creation order (0: the first): the
    /// same on every platform and every run of a program, unlike thread
    /// ids (record/replay's streams).
    pub ordinal: u32,
    /// A thread a kernel handover carried over, or a Linux fork's child:
    /// how it goes on when it starts (instead of `start`).
    resume: IrqMutex<Option<Resume>>,
    /// The user FS and GS bases it starts with (a Linux fork's child
    /// keeps its parent's); None: 0, or the TEB's for an NT thread.
    start_bases: Option<(u64, u64)>,
    /// While a kernel handover holds it on its way back to user mode
    /// (`hold_for_handover`): its full user state.
    held: IrqMutex<Option<Parked>>,
    /// Set once it has entered user mode (a thread created suspended has
    /// not, until resumed).
    entered: core::sync::atomic::AtomicBool,
    /// The absolute deadline of the reissued call (`take_restart_deadline`):
    /// Some(None) = none (infinite).
    restart_deadline: IrqMutex<Option<Option<u64>>>,
}

/// How a thread a kernel handover carried goes on (ARCHITECTURE
/// invariants 1 and 2).
#[derive(Clone)]
pub enum Resume {
    /// Blocked in a system call: made again with the user registers at
    /// the `syscall` (its arguments), the user's x87/SSE state, and the
    /// deadline (timer tick) it had, kept absolute.
    Syscall { number: u32, regs: UserRegs, fx: platform::XState, deadline: Option<u64> },
    /// Stopped in user mode (interrupted, or on its way back from a system
    /// call): goes on with exactly these registers and x87/SSE state.
    User { regs: UserRegs, fx: platform::XState },
    /// Created suspended and never run: starts with `regs` once resumed.
    Start { regs: UserRegs, mxcsr: Option<u32> },
}

/// Set while a kernel handover stops the programs: every thread that
/// returns to user mode waits on the way (`hold_for_handover`).
static HOLD: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The user state of a thread parked by suspension.
#[derive(Clone)]
pub struct Parked {
    pub regs: UserRegs,
    pub fx: platform::XState,
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

    /// SYSTEM_THREAD_INFORMATION's ContextSwitches: times a processor
    /// switched to it (kept from before a handover: 0 then).
    pub fn context_switches(&self) -> u32 {
        let tid = *self.sched_id.lock();
        tid.map_or(0, platform::context_switches)
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

/// Each NT thread's object by its scheduler id (the index): `current`
/// finds the caller's at once. Scanning every thread of every process on
/// each system call took 36% of a Chromium run (its hundreds of threads;
/// `bench`'s null_syscall_200_threads).
static BY_SCHED: IrqMutex<Vec<Option<ObjectRef>>> = IrqMutex::new(Vec::new());

/// Scheduler ids beyond this are not indexed (`current` scans for them).
const BY_SCHED_MAX: usize = 1 << 16;

/// Records `object` as the NT thread of scheduler thread `id`. Only the
/// thread itself does (in `thread_entry`): its creator binding it after
/// the spawn raced the thread's own exit, leaving a stale reference that
/// kept its process alive. Out of memory it is not indexed and `current`
/// finds it by a scan.
fn bind_sched(id: ThreadId, object: &ObjectRef) {
    let i = id.as_usize();
    if i >= BY_SCHED_MAX {
        return;
    }
    let mut table = BY_SCHED.lock();
    if table.len() <= i {
        let more = i + 1 - table.len();
        if table.try_reserve(more).is_err() {
            return;
        }
        table.resize(i + 1, None);
    }
    table[i] = Some(object.clone());
}

/// Forgets `object` as the NT thread of scheduler thread `id`.
fn unbind_sched(id: ThreadId, object: &ObjectRef) {
    let mut table = BY_SCHED.lock();
    if let Some(slot) = table.get_mut(id.as_usize())
        && slot.as_ref().is_some_and(|o| KArc::ptr_eq(o, object))
    {
        *slot = None;
    }
}

/// The NT thread object of the running thread (None for kernel threads).
pub fn current() -> Option<ObjectRef> {
    let me = platform::current();
    // An entry is checked: a scheduler id is reused once its thread is
    // gone, and an exit that missed `unbind_sched` must not name another
    // thread.
    let indexed = BY_SCHED.lock().get(me.as_usize()).cloned().flatten();
    if let Some(o) = indexed
        && *as_thread(&o).sched_id.lock() == Some(me)
    {
        return Some(o);
    }
    THREADS.lock().iter().find(|o| *as_thread(o).sched_id.lock() == Some(me)).cloned()
}

/// The running thread's number in its process (`NtThread::ordinal`; 0 for
/// kernel threads).
pub fn current_ordinal() -> u32 {
    current().map_or(0, |t| as_thread(&t).ordinal)
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

/// The object of thread `id` (NtOpenThread).
pub fn object_by_id(id: u64) -> Option<ObjectRef> {
    by_id(id)
}

/// Whether thread `id` exists and has not exited.
pub fn is_running(id: u64) -> bool {
    by_id(id).is_some_and(|t| !as_thread(&t).has_exited())
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
    create_inner(process, regs, win32_start, mxcsr, stack, suspended, None)
}

/// Creates a thread of the Linux personality that goes on in user mode
/// with exactly `regs`, the x87/SSE state `fx` and the FS and GS bases
/// `bases`: a fork's child's (its parent's state at the fork,
/// `lx::fork`) or a clone'd thread (`lx::threads`). `suspended`: it
/// waits for `resume` (its Linux state is set up first).
pub fn create_forked(
    process: &KArc<Process>,
    regs: UserRegs,
    fx: platform::XState,
    bases: (u64, u64),
    suspended: bool,
) -> Result<ObjectRef, NtStatus> {
    let resume = Some((Resume::User { regs, fx }, bases));
    create_inner(process, regs, regs.rip, None, (0, 0, 0), suspended, resume)
}

/// A thread object's id (its NT ClientId thread; a Linux thread's core
/// id).
pub fn id_of(object: &ObjectRef) -> u64 {
    as_thread(object).id
}

fn create_inner(
    process: &KArc<Process>,
    regs: UserRegs,
    win32_start: u64,
    mxcsr: Option<u32>,
    stack: (u64, u64, u64),
    suspended: bool,
    resume: Option<(Resume, (u64, u64))>,
) -> Result<ObjectRef, NtStatus> {
    if !regs.is_valid() {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    let id = next_id();
    // Only NT threads have a TEB (and start with it as their GS base); a
    // Linux thread starts with GS and FS bases 0 (arch_prctl_test).
    let teb_addr = if process.personality == super::platform::Personality::Nt {
        allocate_teb(process, id, stack)?
    } else {
        0
    };
    let thread = NtThread {
        id,
        teb: teb_addr,
        process: process.clone(),
        sched_id: IrqMutex::named("thread.sched_id", None),
        affinity: core::sync::atomic::AtomicU64::new(platform::online_mask()),
        dispatcher: Dispatcher::new(0),
        exit_status: IrqMutex::new(None),
        suspend_count: IrqMutex::named("thread.suspend_count", u32::from(suspended)),
        terminate_request: IrqMutex::new(None),
        start: regs,
        start_mxcsr: mxcsr,
        win32_start,
        create_time: platform::system_time(),
        exit_time: IrqMutex::named("thread.exit_time", 0),
        priority: IrqMutex::new((PROCESS_BASE_PRIORITY, PROCESS_BASE_PRIORITY)),
        apcs: IrqMutex::new(crate::mm::heap::ReservedVec::new()),
        alertable: core::sync::atomic::AtomicBool::new(false),
        owned_mutants: IrqMutex::new(Vec::new()),
        parked: IrqMutex::new(None),
        in_syscall: IrqMutex::new(None),
        impersonation: IrqMutex::new(None),
        ordinal: process.threads_created.fetch_add(1, core::sync::atomic::Ordering::Relaxed),
        start_bases: resume.as_ref().map(|r| r.1),
        resume: IrqMutex::new(resume.map(|r| r.0)),
        restart_deadline: IrqMutex::new(None),
        held: IrqMutex::new(None),
        entered: core::sync::atomic::AtomicBool::new(false),
    };
    let thread = crate::mm::heap::try_box(thread).map_err(|_| NtStatus::NO_MEMORY)?;
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
        process.personality,
        id,
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
        // Mapped under the VAD lock (ARCHITECTURE "Virtual memory").
        let flags = PageFlags { user: true, global: false, ..PageFlags::KERNEL_RW };
        let mapped = process.space.lock().map_zeroed(addr, teb::PAGES, flags);
        mapped.map_err(|_| NtStatus::NO_MEMORY)?;
        addr
    };
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
    bind_sched(platform::current(), &object);
    t.apply_affinity();
    let resume = t.resume.lock().take();
    if let Some((fs, gs)) = t.start_bases {
        platform::set_user_bases(Some(fs), Some(gs));
    }
    let (mut regs, mut mxcsr) = (t.start, t.start_mxcsr);
    match resume {
        Some(Resume::Syscall { number, regs, fx, deadline }) => {
            // Carried over by a kernel handover: the blocked call again,
            // with its deadline (the kernel never touches the x87/SSE
            // registers, so the user's state set here is what user mode
            // gets back).
            *t.restart_deadline.lock() = Some(deadline);
            t.entered.store(true, Ordering::SeqCst);
            drop(object);
            platform::xrstor(&fx);
            let g = &regs.gpr;
            let mut frame = platform::SyscallFrame {
                r15: g[15],
                r14: g[14],
                r13: g[13],
                r12: g[12],
                rbp: g[5],
                rbx: g[3],
                rsi: g[6],
                rdi: g[7],
                r9: g[9],
                r8: g[8],
                rdx: g[2],
                r10: g[10],
                rax: u64::from(number),
                rip: regs.rip,
                rflags: regs.rflags,
                rsp: g[4],
            };
            platform::reissue_syscall(&mut frame);
        }
        Some(Resume::User { regs, fx }) => {
            // Stopped in user mode by a handover: parked as a suspended
            // thread is (NtGetContextThread sees it while it stays
            // suspended), then on with every register.
            *t.parked.lock() = Some(Parked { regs, fx, changed: false });
        }
        Some(Resume::Start { regs: start, mxcsr: start_mxcsr }) => {
            (regs, mxcsr) = (start, start_mxcsr);
        }
        None => {}
    }
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
    let parked = t.parked.lock().take();
    t.entered.store(true, Ordering::SeqCst);
    drop(object);
    if let Some(status) = pending {
        exit_current(status);
    }
    if let Some(p) = parked {
        platform::xrstor(&p.fx);
        platform::enter_user(&p.regs);
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
    queue_apc(object, apc, false)
}

/// Keeps room in `object`'s APC queue for the completion routine of an
/// I/O request in progress (`notify::Reserved`).
pub fn reserve_apc(object: &ObjectRef) -> Result<(), NtStatus> {
    let ObjectBody::Thread(t) = &object.body else { return Err(NtStatus::OBJECT_TYPE_MISMATCH) };
    t.apcs.lock().reserve_slot().map_err(|_| NtStatus::INSUFFICIENT_RESOURCES)
}

/// Gives back a slot `reserve_apc` kept.
pub fn release_apc(object: &ObjectRef) {
    if let ObjectBody::Thread(t) = &object.body {
        t.apcs.lock().release_slot();
    }
}

/// Queues an I/O request's completion routine into the slot reserved for
/// it (no allocation); the slot is used up either way.
pub fn queue_reserved_apc(object: &ObjectRef, apc: UserApc) -> Result<(), NtStatus> {
    queue_apc(object, apc, true)
}

fn queue_apc(object: &ObjectRef, apc: UserApc, reserved: bool) -> Result<(), NtStatus> {
    let ObjectBody::Thread(t) = &object.body else { return Err(NtStatus::OBJECT_TYPE_MISMATCH) };
    wait::with_dispatcher_lock(|| {
        // Under the lock the thread's exit marks it under (`exit_current`).
        if t.has_exited() {
            if reserved {
                t.apcs.lock().release_slot();
            }
            return Err(NtStatus::UNSUCCESSFUL);
        }
        let mut apcs = t.apcs.lock();
        let pushed = if reserved { apcs.push_reserved(apc) } else { apcs.try_push(apc) };
        drop(apcs);
        pushed.map_err(|_| NtStatus::NO_MEMORY)?;
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
        // Checked and freed in one hold of the dispatcher lock.
        wait::change_mutant(&object, |state| {
            if state.owner == Some(t.id) {
                *state = super::object::MutantState { owner: None, count: 0, abandoned: true };
            }
        });
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
    if HOLD.load(Ordering::SeqCst) {
        hold(t, regs);
        let terminate = *t.terminate_request.lock();
        if let Some(status) = terminate {
            drop(object);
            exit_current(status);
        }
    }
    if *t.suspend_count.lock() == 0 {
        return false;
    }
    // Suspended: wait here, with the user state where other threads can
    // read and change it, until resumed (or terminated).
    *t.parked.lock() = Some(Parked { regs: *regs, fx: platform::xsave(), changed: false });
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
            platform::xrstor(&p.fx);
            true
        }
        _ => false,
    }
}

/// Waits on the way back to user mode while a kernel handover stops the
/// programs, with its user state where the handover reads it
/// (`snapshot`). Nothing is held here: the system call or interrupt is
/// over, so the thread is at a safe point.
fn hold(t: &NtThread, regs: &UserRegs) {
    *t.held.lock() = Some(Parked { regs: *regs, fx: platform::xsave(), changed: false });
    loop {
        if !HOLD.load(Ordering::SeqCst) || t.terminate_request.lock().is_some() {
            break;
        }
        platform::prepare_wait(None);
        if !HOLD.load(Ordering::SeqCst) || t.terminate_request.lock().is_some() {
            platform::wake(platform::current()); // see `wake_if_terminating`
        }
        platform::wait();
    }
    *t.held.lock() = None;
}

/// A kernel handover stops (true) or lets go (false) the programs: with
/// `on`, every thread waits at its next return to user mode (one in user
/// mode gets there at its next timer tick, one in a system call when the
/// call returns); with false the held threads go on.
pub fn hold_for_handover(on: bool) {
    HOLD.store(on, Ordering::SeqCst);
    // Under the list's lock: nothing to allocate, so none is left held.
    for object in THREADS.lock().iter() {
        let t = as_thread(object);
        let sched = *t.sched_id.lock();
        let Some(id) = sched else { continue };
        if on {
            platform::interrupt(id);
        } else if t.held.lock().is_some() {
            platform::wake(id);
        }
    }
}

/// NtSuspendThread: raises the suspend count (the thread stops on its way
/// back to user mode). Returns the previous count.
pub fn suspend(object: &ObjectRef) -> Result<u32, NtStatus> {
    let ObjectBody::Thread(t) = &object.body else { return Err(NtStatus::OBJECT_TYPE_MISMATCH) };
    // Checked under the suspend count, which the thread's exit marks it
    // under (`exit_current`): no suspension is counted once it began.
    let mut count = t.suspend_count.lock();
    if t.has_exited() || t.terminate_request.lock().is_some() {
        return Err(NtStatus::THREAD_IS_TERMINATING);
    }
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

/// The deadline a reissued system call keeps (a kernel handover): taken by
/// the call's timeout (`Call::timeout`) instead of its argument, once.
/// Some(None): no deadline.
pub fn take_restart_deadline() -> Option<Option<u64>> {
    let object = current()?;
    as_thread(&object).restart_deadline.lock().take()
}

/// What a kernel handover needs to know about a thread.
#[derive(Clone)]
pub struct Snapshot {
    pub id: u64,
    pub teb: u64,
    pub process: KArc<Process>,
    pub sched: Option<ThreadId>,
    /// The system call it is in (number, user registers at the `syscall`).
    pub in_syscall: Option<(u32, UserRegs)>,
    pub suspended: u32,
    pub terminating: bool,
    pub apcs: usize,
    pub impersonating: bool,
    pub win32_start: u64,
    pub create_time: u64,
    pub exit_time: u64,
    pub priority: (i32, i32),
    pub affinity: u64,
    pub ordinal: u32,
    pub exit_status: Option<NtStatus>,
    /// Held on its way back to user mode by a handover: its user state.
    pub held: Option<Parked>,
    /// Parked by suspension on its way back to user mode.
    pub parked: Option<Parked>,
    /// Whether it has entered user mode (not: created suspended).
    pub entered: bool,
    /// What it starts with if it has not entered user mode yet.
    pub start: UserRegs,
    pub start_mxcsr: Option<u32>,
}

/// `object`'s state, if it is a thread.
pub fn snapshot(object: &ObjectRef) -> Option<Snapshot> {
    let ObjectBody::Thread(t) = &object.body else { return None };
    // One lock at a time (see `dump`).
    let sched = *t.sched_id.lock();
    let in_syscall = *t.in_syscall.lock();
    let suspended = *t.suspend_count.lock();
    let terminating = t.terminate_request.lock().is_some();
    let apcs = t.apcs.lock().len();
    let impersonating = t.impersonation.lock().is_some();
    let exit_time = *t.exit_time.lock();
    let priority = *t.priority.lock();
    let exit_status = *t.exit_status.lock();
    let held = t.held.lock().clone();
    let parked = t.parked.lock().clone();
    Some(Snapshot {
        id: t.id,
        teb: t.teb,
        process: t.process.clone(),
        sched,
        in_syscall,
        suspended,
        terminating,
        apcs,
        impersonating,
        win32_start: t.win32_start,
        create_time: t.create_time,
        exit_time,
        priority,
        affinity: t.affinity(),
        ordinal: t.ordinal,
        exit_status,
        held,
        parked,
        entered: t.entered.load(Ordering::SeqCst),
        start: t.start,
        start_mxcsr: t.start_mxcsr,
    })
}

/// The user APCs queued to thread `object`, oldest first.
pub fn queued_apcs(object: &ObjectRef) -> Result<Vec<UserApc>, crate::mm::heap::AllocError> {
    let ObjectBody::Thread(t) = &object.body else { return Ok(Vec::new()) };
    let apcs = t.apcs.lock();
    let mut out = Vec::new();
    out.try_extend_from_slice(&apcs)?;
    Ok(out)
}

/// Every live NT thread object.
pub fn all() -> Result<Vec<ObjectRef>, crate::mm::heap::AllocError> {
    let threads = THREADS.lock();
    let mut out = Vec::new();
    out.try_reserve(threads.len()).map_err(|_| crate::mm::heap::AllocError)?;
    for t in threads.iter() {
        out.try_push(t.clone())?;
    }
    Ok(out)
}

/// The next NT id `next_id` would hand out.
pub fn peek_next_id() -> u64 {
    NEXT_ID.load(Ordering::Relaxed)
}

/// Makes ids continue from at least `next` (ids carried by a handover are
/// never handed out again).
pub fn continue_ids_from(next: u64) {
    NEXT_ID.fetch_max(next, Ordering::Relaxed);
}

/// A thread a kernel handover carried over (`handover::programs`).
pub struct Adopted {
    pub id: u64,
    pub teb: u64,
    pub process: KArc<Process>,
    pub win32_start: u64,
    pub create_time: u64,
    pub priority: (i32, i32),
    pub affinity: u64,
    pub ordinal: u32,
    /// A live thread: the call it was blocked in. None: it has exited,
    /// with (status, exit time) (a handle still names it).
    pub resume: Option<Resume>,
    pub exited: Option<(NtStatus, u64)>,
    /// Its suspend count and queued user APCs (oldest first).
    pub suspend: u32,
    pub apcs: Vec<UserApc>,
}

/// Rebuilds a thread object from a kernel handover's record. A live one
/// joins its process and the thread list; `start_adopted` runs it.
pub fn adopt(a: Adopted) -> Result<ObjectRef, NtStatus> {
    let live = a.exited.is_none();
    let online = platform::online_mask();
    let affinity = if a.affinity & online != 0 { a.affinity & online } else { online };
    let thread = NtThread {
        id: a.id,
        teb: a.teb,
        process: a.process.clone(),
        sched_id: IrqMutex::named("thread.sched_id", None),
        affinity: core::sync::atomic::AtomicU64::new(affinity),
        dispatcher: Dispatcher::new(i32::from(!live)),
        exit_status: IrqMutex::new(a.exited.map(|e| e.0)),
        suspend_count: IrqMutex::named("thread.suspend_count", a.suspend),
        terminate_request: IrqMutex::new(None),
        start: UserRegs::default(),
        start_mxcsr: None,
        win32_start: a.win32_start,
        create_time: a.create_time,
        exit_time: IrqMutex::named("thread.exit_time", a.exited.map_or(0, |e| e.1)),
        priority: IrqMutex::new(a.priority),
        apcs: IrqMutex::new(crate::mm::heap::ReservedVec::from_vec(a.apcs)),
        alertable: core::sync::atomic::AtomicBool::new(false),
        owned_mutants: IrqMutex::new(Vec::new()),
        parked: IrqMutex::new(None),
        in_syscall: IrqMutex::new(None),
        impersonation: IrqMutex::new(None),
        ordinal: a.ordinal,
        resume: IrqMutex::new(a.resume),
        start_bases: None,
        restart_deadline: IrqMutex::new(None),
        held: IrqMutex::new(None),
        entered: core::sync::atomic::AtomicBool::new(false),
    };
    let thread = crate::mm::heap::try_box(thread).map_err(|_| NtStatus::NO_MEMORY)?;
    let object = KObject::new(ObjectBody::Thread(thread)).map_err(|_| NtStatus::NO_MEMORY)?;
    if live {
        THREADS.lock().try_push(object.clone()).map_err(|_| NtStatus::NO_MEMORY)?;
        if let Err(status) = a.process.add_thread(object.clone()) {
            THREADS.lock().retain(|o| !KArc::ptr_eq(o, &object));
            return Err(status);
        }
    }
    continue_ids_from(a.id + 4);
    Ok(object)
}

/// Starts a thread `adopt` rebuilt: it makes its blocked call again.
pub fn start_adopted(object: &ObjectRef) -> Result<(), NtStatus> {
    let ObjectBody::Thread(t) = &object.body else { return Err(NtStatus::OBJECT_TYPE_MISMATCH) };
    let p = &t.process;
    let tid = platform::spawn_user(
        p.name,
        thread_entry,
        t.id as usize,
        p.space.clone(),
        t.teb,
        p.personality,
        t.id,
    )
    .map_err(|_| NtStatus::NO_MEMORY)?;
    *t.sched_id.lock() = Some(tid);
    Ok(())
}

/// Records that thread `owner` owns `mutant` (a mutant carried over by a
/// kernel handover; abandoned if the thread exits).
pub fn note_owner(owner: &ObjectRef, mutant: &ObjectRef) -> Result<(), NtStatus> {
    let ObjectBody::Thread(t) = &owner.body else { return Err(NtStatus::OBJECT_TYPE_MISMATCH) };
    t.owned_mutants.lock().try_push(mutant.clone()).map_err(|_| NtStatus::NO_MEMORY)
}

/// Records (or clears) the current thread's user registers while it is in
/// a system call (the dispatcher calls this around every service).
pub fn set_in_syscall(object: &ObjectRef, regs: Option<(u32, UserRegs)>) {
    let t = as_thread(object);
    // A system call that ends is progress for the hang watchdog (a thread
    // that calls the kernel in a loop is mostly in kernel code).
    if regs.is_none()
        && let Some(id) = *t.sched_id.lock()
    {
        crate::flight::progress(id.as_usize());
    }
    *t.in_syscall.lock() = regs;
}

/// The user state of a suspended thread: parked on its way to user mode,
/// or, while it is inside a system call (blocked in a wait, say), its
/// registers at the `syscall` with the default floating-point state.
/// Waits for one of the two, as Windows does (up to 30 s: a thread just
/// created or held back by a loaded host may take long to get there; a
/// one-second limit failed context_test's GetThreadContext under load
/// with STATUS_UNSUCCESSFUL).
pub fn parked_state(object: &ObjectRef) -> Result<Parked, NtStatus> {
    let ObjectBody::Thread(t) = &object.body else { return Err(NtStatus::OBJECT_TYPE_MISMATCH) };
    let give_up = platform::monotonic_ns().saturating_add(30_000_000_000);
    while platform::monotonic_ns() < give_up {
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
    let Ok(threads) = crate::mm::heap::try_to_vec(&THREADS.lock()) else {
        info!("thread: no memory to list the threads");
        return;
    };
    for object in threads.iter() {
        let t = as_thread(object);
        let in_syscall = *t.in_syscall.lock();
        // One lock at a time (format arguments' guards would live to the
        // end of the statement: sched_id held while taking suspend_count,
        // the inverse of `resume`'s order).
        let sched = *t.sched_id.lock();
        let suspended = *t.suspend_count.lock();
        info!(
            "thread: pid {} tid {} (sched {:?}) suspend {} exited {} in a system call {} \
             session {} personality {} ({})",
            t.process.id,
            t.id,
            sched,
            suspended,
            t.has_exited(),
            in_syscall.is_some(),
            t.process.session,
            t.process.personality,
            t.process.name
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
            if let ObjectBody::IoCompletion(c) = &e.object.body {
                // A port is ready while it holds packets (its dispatcher
                // flag is set by a post and never cleared: not a sign).
                match c.queue.try_lock() {
                    Some(q) => info!(
                        "thread:   on handle {:#x}: IoCompletion with {} packets queued",
                        handle.0,
                        q.len()
                    ),
                    None => info!("thread:   on handle {:#x}: IoCompletion (queue busy)", handle.0),
                }
            } else {
                let signal = e.object.dispatcher().map_or(-1, |d| d.signal_state());
                info!(
                    "thread:   on handle {:#x}: {} signal {signal}",
                    handle.0,
                    e.object.type_name()
                );
            }
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
fn default_fx() -> platform::XState {
    platform::XState::initial()
}

/// Replaces the parked user state of a suspended thread (applied when it
/// resumes).
pub fn set_parked_state(
    object: &ObjectRef,
    regs: UserRegs,
    fx: Option<platform::XState>,
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
        // The thread counts as exited from here, decided under the locks
        // that queuing an APC (the dispatcher lock) and suspending (the
        // suspend count) check it under: a call sees it before its exit
        // began or after, never between a check and its effect (an APC
        // accepted that never runs, a suspension never seen). User APCs
        // still queued are discarded, as on Windows.
        wait::with_dispatcher_lock(|| {
            let _count = t.suspend_count.lock();
            t.exit_status.lock().get_or_insert(status);
            t.apcs.lock().clear();
        });
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
        // Its windows are destroyed with it, as on Windows (user32 frees
        // its own records at DLL_THREAD_DETACH; this covers threads that
        // never got there), and messages sent to it fail.
        t.process.desk.windows.thread_ended(t.process.id as u32, t.id as u32);
        t.process.desk.hooks.thread_ended(t.process.id as u32, t.id as u32);
        // A desktop handle it was set to (SetThreadDesktop) may be closed.
        t.process.ui.thread_ended(t.id);
        THREADS.lock().retain(|o| !KArc::ptr_eq(o, &object));
        if let Some(id) = *t.sched_id.lock() {
            unbind_sched(id, &object);
        }
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
