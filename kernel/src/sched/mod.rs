//! Kernel threads and a round-robin, preemptive scheduler for all CPUs.
//!
//! One run queue, shared by every CPU under `SCHED` (an `IrqMutex`, so a
//! spinlock with interrupts off). Each CPU has its current and idle thread
//! and quantum in `Scheduler::cpus`, indexed by `percpu::cpu_index()`,
//! which is only read while `SCHED` is held (interrupts are off then, so
//! the thread cannot move to another CPU meanwhile).
//!
//! A thread switched away from keeps `on_cpu` set until the CPU has saved
//! its context and runs the next thread (`finish_switch`); no other CPU
//! picks it up before that, and its stack is not freed before that.
//!
//! Locking rule: every lock that can be taken while a thread may be
//! preempted must be an `IrqMutex`. The timer interrupt can then only
//! preempt code that holds no locks.

#[allow(unsafe_code)]
mod switch;

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::fmt;
use core::panic::Location;

use crate::arch::context::{Context, KernelStack, StackError};
use crate::arch::paging::{self, SharedUserSpace};
use crate::arch::syscall::UserRegs;
use crate::arch::xstate::XState;
use crate::arch::{cpu, percpu, smp};
use crate::mm::heap::{AllocError, TryVec, try_box};
use crate::personality::Personality;
use crate::sync::IrqMutex;
use crate::time;

/// Timer ticks a thread may run before it is preempted.
const QUANTUM_TICKS: u32 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThreadId(usize);

impl ThreadId {
    pub fn as_usize(self) -> usize {
        self.0
    }

    /// The id whose `as_usize` is `raw` (for ids kept in atomics).
    pub fn from_raw(raw: usize) -> Self {
        Self(raw)
    }
}

impl fmt::Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Running,
    Ready,
    Blocked,
    Sleeping {
        until_tick: u64,
    },
    /// Waiting for `wake` or, if set, until the tick deadline.
    Waiting {
        until_tick: Option<u64>,
    },
    Dead,
}

struct Thread {
    name: &'static str,
    state: State,
    /// Its saved registers and x87/SSE/AVX area; `None` once a dead
    /// thread is reaped (the area is a heap block per thread, and dead
    /// threads' records stay in the table).
    context: Option<Context>,
    /// `None` for the boot thread and the APs' idle threads (stacks that
    /// are never freed), and after reaping.
    stack: Option<KernelStack>,
    entry: Option<(fn(usize), usize)>,
    /// Threads blocked in `join` on this one.
    joiners: Vec<ThreadId>,
    /// Set for threads that run user code.
    user: Option<UserThread>,
    /// Why the last `wait` ended.
    wake_reason: WakeReason,
    /// Where the last `prepare_wait` was called from (printed beside
    /// `Waiting` in the state dump: which wait a stuck thread is in).
    wait_site: Option<&'static Location<'static>>,
    /// Some CPU is running on this thread's stack (see the module docs).
    on_cpu: bool,
    /// CPUs this thread may run on.
    affinity: u64,
    /// A CPU's idle thread: never queued, run when nothing else is.
    is_idle: bool,
    /// Registers the current system call returns with (NtContinue).
    pending_continue: Option<UserRegs>,
    /// Times a CPU switched to this thread (SYSTEM_THREAD_INFORMATION's
    /// ContextSwitches; wraps).
    switches: u32,
}

/// Why a `prepare_wait`/`wait` ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeReason {
    Woken,
    TimedOut,
}

/// User-mode side of a thread.
struct UserThread {
    /// Keeps the address space alive while the thread exists; `root` is
    /// its PML4, read without taking the space's lock.
    space: SharedUserSpace,
    root: u64,
    /// GS base in user mode (the TEB; a Linux thread's arch_prctl value).
    gs_base: u64,
    /// FS base in user mode: 0 for NT threads; a Linux thread's
    /// arch_prctl(ARCH_SET_FS) value (its TLS); a WOW64 thread's 32-bit
    /// TEB.
    fs_base: u64,
    /// A WOW64 thread (`set_teb32`): its FS is the TEB32 segment, loaded
    /// with DS and ES at every switch to it (they are per CPU, and 32-bit
    /// code needs them).
    teb32: bool,
    /// Its process's personality: which dispatcher its system calls go to
    /// (`crate::personality`; fixed for the thread's life).
    personality: Personality,
    /// Its client id (an NT thread id, a Linux tid), for the event stream.
    client: u64,
}

/// One CPU's scheduling state.
#[derive(Clone, Copy)]
struct Cpu {
    /// Whether this CPU schedules (its idle thread exists).
    online: bool,
    current: ThreadId,
    idle: Option<ThreadId>,
    quantum_left: u32,
    need_resched: bool,
    /// The thread this CPU just switched away from, whose `on_cpu` is
    /// cleared by `finish_switch` on this CPU.
    prev: Option<ThreadId>,
    /// The idle thread is reaping: not preempted until done (see
    /// `idle_loop`).
    reaping: bool,
}

const OFFLINE: Cpu = Cpu {
    online: false,
    current: ThreadId(0),
    idle: None,
    quantum_left: QUANTUM_TICKS,
    need_resched: false,
    prev: None,
    reaping: false,
};

struct Scheduler {
    /// Indexed by `ThreadId`; entries are never removed, so ids are unique.
    /// Boxed so that each `Context` keeps its address when the vector grows.
    #[allow(clippy::vec_box)]
    threads: Vec<Box<Thread>>,
    /// Invariant: capacity >= number of threads, so pushing never allocates.
    ready: VecDeque<ThreadId>,
    cpus: [Cpu; smp::MAX_CPUS],
    /// A kernel handover is collecting the programs' state: threads that
    /// run user code are not picked (they stay where they are, blocked or
    /// queued), and a running one leaves its CPU at the next tick.
    users_frozen: bool,
}

static SCHED: IrqMutex<Option<Scheduler>> = IrqMutex::new(None);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnError {
    NotInitialized,
    OutOfMemory,
    Stack(StackError),
}

impl From<AllocError> for SpawnError {
    fn from(_: AllocError) -> Self {
        SpawnError::OutOfMemory
    }
}

fn with<R>(f: impl FnOnce(&mut Scheduler) -> R) -> R {
    f(SCHED.lock().as_mut().expect("scheduler not initialized"))
}

/// This CPU's index; stable while `SCHED` is held (interrupts are off).
fn me() -> usize {
    percpu::cpu_index() as usize
}

/// Every CPU.
const ALL_CPUS: u64 = u64::MAX;

impl Scheduler {
    fn thread(&mut self, id: ThreadId) -> &mut Thread {
        &mut self.threads[id.0]
    }

    /// The thread running on this CPU.
    fn current(&self) -> ThreadId {
        self.cpus[me()].current
    }

    fn make_ready(&mut self, id: ThreadId) {
        let t = self.thread(id);
        t.state = State::Ready;
        if t.is_idle {
            return;
        }
        let affinity = t.affinity;
        debug_assert!(self.ready.len() < self.ready.capacity());
        self.ready.push_back(id);
        // Wake one idle CPU that may run it (an idle CPU sits in `hlt`).
        let here = me();
        if let Some(cpu) = (0..smp::MAX_CPUS).find(|&c| {
            let cpu = &self.cpus[c];
            c != here && cpu.online && Some(cpu.current) == cpu.idle && affinity & (1 << c) != 0
        }) {
            smp::send_reschedule(cpu as u32);
        }
    }

    /// Whether a queued thread may run on CPU `cpu` now: allowed there and
    /// not still on another CPU's stack (the current thread is fine).
    fn runnable_here(&self, id: ThreadId, cpu: usize) -> bool {
        let t = &self.threads[id.0];
        t.affinity & (1 << cpu) != 0
            && (!t.on_cpu || id == self.cpus[cpu].current)
            && !(self.users_frozen && t.user.is_some())
    }

    fn has_work(&self, cpu: usize) -> bool {
        self.ready.iter().any(|&id| self.runnable_here(id, cpu))
    }

    /// Takes the first queued thread that may run on `cpu`.
    fn pick(&mut self, cpu: usize) -> Option<ThreadId> {
        let i = self.ready.iter().position(|&id| self.runnable_here(id, cpu))?;
        self.ready.remove(i)
    }

    fn add(&mut self, thread: Thread) -> Result<ThreadId, SpawnError> {
        self.add_boxed(try_box(thread)?).map_err(|_| SpawnError::OutOfMemory)
    }

    /// Adds a thread; on failure (no memory) hands it back, so that the
    /// caller can drop it outside the lock.
    #[allow(clippy::result_large_err)]
    fn add_boxed(&mut self, thread: Box<Thread>) -> Result<ThreadId, Box<Thread>> {
        let grow = self.threads.len() + 1 - self.ready.len();
        if self.ready.try_reserve(grow).is_err() || self.threads.try_reserve(1).is_err() {
            return Err(thread);
        }
        let id = ThreadId(self.threads.len());
        self.threads.try_push(thread).map_err(|_| AllocError).expect("capacity was reserved");
        Ok(id)
    }
}

/// A thread record for code that is already running on this CPU.
fn adopted(name: &'static str, is_idle: bool, affinity: u64) -> Result<Thread, SpawnError> {
    Ok(Thread {
        name,
        state: State::Running,
        context: Some(Context::current().map_err(|_| SpawnError::OutOfMemory)?),
        stack: None,
        entry: None,
        joiners: Vec::new(),
        user: None,
        wake_reason: WakeReason::Woken,
        wait_site: None,
        on_cpu: true,
        affinity,
        is_idle,
        pending_continue: None,
        switches: 0,
    })
}

/// Adopts the running boot thread as thread #0 and starts the boot CPU's
/// idle thread.
pub fn init() -> Result<(), SpawnError> {
    let mut sched = Scheduler {
        threads: Vec::new(),
        ready: VecDeque::new(),
        cpus: [OFFLINE; smp::MAX_CPUS],
        users_frozen: false,
    };
    let boot = sched.add(adopted("boot", false, ALL_CPUS)?)?;
    sched.cpus[0] = Cpu { online: true, current: boot, ..OFFLINE };
    *SCHED.lock() = Some(sched);
    let idle = spawn("idle", idle_loop, 0)?;
    with(|s| {
        // `spawn` queued it; the idle thread is only run when nothing else is.
        s.ready.retain(|&t| t != idle);
        let t = s.thread(idle);
        t.is_idle = true;
        t.affinity = 1;
        s.cpus[0].idle = Some(idle);
    });
    Ok(())
}

/// Entered by each AP once its CPU is set up (see `arch::smp`): adopts the
/// running code as the CPU's idle thread and starts scheduling there.
pub fn run_ap(index: u32) -> ! {
    let cpu = index as usize;
    // The record (and its save area) is allocated outside the lock.
    let record = adopted("idle", true, 1 << cpu);
    let added = with(|s| {
        let id = s.add(record?)?;
        s.cpus[cpu] = Cpu { online: true, current: id, idle: Some(id), ..OFFLINE };
        Ok::<_, SpawnError>(())
    });
    if let Err(e) = added {
        crate::warn!("sched: CPU {index} stays idle: {e:?}");
        loop {
            cpu::enable_interrupts_and_halt();
        }
    }
    time::start_tick_here();
    idle_loop(0);
    unreachable!("the idle loop does not return");
}

/// Creates a thread running `entry(arg)`. It starts on the next switch.
pub fn spawn(name: &'static str, entry: fn(usize), arg: usize) -> Result<ThreadId, SpawnError> {
    spawn_inner(name, entry, arg, None)
}

/// Creates a thread that runs `entry(arg)` in kernel mode inside the user
/// address space `space`, with user GS base `gs_base`; its system calls
/// go to `personality`'s dispatcher. `entry` normally ends by entering
/// user mode.
pub fn spawn_user(
    name: &'static str,
    entry: fn(usize),
    arg: usize,
    space: SharedUserSpace,
    gs_base: u64,
    personality: Personality,
    client: u64,
) -> Result<ThreadId, SpawnError> {
    let root = space.lock().root();
    spawn_inner(
        name,
        entry,
        arg,
        Some(UserThread { space, root, gs_base, fs_base: 0, teb32: false, personality, client }),
    )
}

fn spawn_inner(
    name: &'static str,
    entry: fn(usize),
    arg: usize,
    user: Option<UserThread>,
) -> Result<ThreadId, SpawnError> {
    // Ended threads give back their kernel stacks first: the idle threads
    // reap only when a CPU has nothing to do, and on a machine that never
    // idles (KVM, a program making and ending threads back to back) the
    // dead ones' stacks would otherwise use up the stack window's slots
    // (memload_test: CreateThread failed at its ~1040th thread). Each
    // spawn reaps up to a batch, more than the one thread it adds.
    reap();
    // Allocate outside the scheduler lock.
    let stack = KernelStack::new().map_err(SpawnError::Stack)?;
    let context = Context::new_thread(&stack, thread_start).map_err(|_| SpawnError::OutOfMemory)?;
    let thread = Thread {
        name,
        state: State::Ready,
        context: Some(context),
        stack: Some(stack),
        entry: Some((entry, arg)),
        joiners: Vec::new(),
        user,
        wake_reason: WakeReason::Woken,
        wait_site: None,
        on_cpu: false,
        affinity: ALL_CPUS,
        is_idle: false,
        pending_continue: None,
        switches: 0,
    };
    let thread = try_box(thread)?;
    let added = {
        let mut guard = SCHED.lock();
        match guard.as_mut() {
            None => Err((SpawnError::NotInitialized, Some(thread))),
            Some(sched) => match sched.add_boxed(thread) {
                Ok(id) => {
                    sched.make_ready(id);
                    Ok(id)
                }
                Err(thread) => Err((SpawnError::OutOfMemory, Some(thread))),
            },
        }
    };
    // On failure the thread (and its stack) is dropped here, outside the
    // lock: freeing a stack flushes other CPUs' TLBs.
    added.map_err(|(e, _thread)| e)
}

extern "C" fn thread_start() -> ! {
    finish_switch();
    let (entry, arg) = with(|s| {
        let id = s.current();
        s.thread(id).entry.take().expect("new thread has an entry point")
    });
    cpu::enable_interrupts();
    entry(arg);
    exit()
}

/// Ends the current thread.
pub fn exit() -> ! {
    with(|s| {
        let id = s.current();
        let joiners = core::mem::take(&mut s.thread(id).joiners);
        for j in joiners {
            s.make_ready(j);
        }
        s.thread(id).state = State::Dead;
    });
    schedule();
    unreachable!("dead thread was scheduled again");
}

pub fn current() -> ThreadId {
    with(|s| s.current())
}

/// The current thread's user (FS, GS) bases; (0, 0) for a kernel thread.
pub fn user_bases() -> (u64, u64) {
    with(|s| {
        let id = s.current();
        s.thread(id).user.as_ref().map_or((0, 0), |u| (u.fs_base, u.gs_base))
    })
}

/// Makes the current user thread one that runs 32-bit code (WOW64), with
/// its 32-bit TEB at `teb32` (below 4 GiB) as FS from this return to user
/// mode on (ARCHITECTURE "Plan: WOW64"). False for a kernel thread or an
/// address above 4 GiB.
pub fn set_teb32(teb32: u64) -> bool {
    let Ok(base) = u32::try_from(teb32) else { return false };
    with(|s| {
        let id = s.current();
        let Some(u) = s.thread(id).user.as_mut() else { return false };
        u.fs_base = teb32;
        u.teb32 = true;
        percpu::load_teb32(base);
        true
    })
}

/// Sets the current user thread's FS and/or GS base, for this return to
/// user mode and every later switch to it (Linux's arch_prctl). The
/// caller has checked the values are canonical user addresses. False for
/// a kernel thread.
pub fn set_user_bases(fs: Option<u64>, gs: Option<u64>) -> bool {
    // Under the scheduler's lock (interrupts off): no switch between the
    // thread's record and this CPU's registers.
    with(|s| {
        let id = s.current();
        let Some(u) = s.thread(id).user.as_mut() else { return false };
        if let Some(fs) = fs {
            u.fs_base = fs;
            percpu::set_user_fs_base(fs);
        }
        if let Some(gs) = gs {
            u.gs_base = gs;
            percpu::set_user_gs_base(gs);
        }
        true
    })
}

/// This CPU's current thread and whether it is its idle thread; None
/// before the scheduler exists (early timer ticks).
/// Per CPU: whether it runs its idle thread (set at each switch), for
/// readers that may not take the scheduler's lock (the profiler's choice
/// of CPUs to sample, from another CPU's tick).
static RUNNING_IDLE: [core::sync::atomic::AtomicBool; crate::arch::smp::MAX_CPUS] =
    [const { core::sync::atomic::AtomicBool::new(false) }; crate::arch::smp::MAX_CPUS];

/// Whether CPU `cpu` runs its idle thread (lock-free; as of its last
/// switch).
pub fn cpu_idle(cpu: usize) -> bool {
    RUNNING_IDLE.get(cpu).is_some_and(|f| f.load(core::sync::atomic::Ordering::Relaxed))
}

pub fn current_if_running() -> Option<(ThreadId, bool)> {
    let guard = SCHED.lock();
    let s = guard.as_ref()?;
    let id = s.cpus[me()].current;
    Some((id, s.threads.get(id.as_usize()).is_some_and(|t| t.is_idle)))
}

pub fn current_name() -> &'static str {
    with(|s| {
        let id = s.current();
        s.thread(id).name
    })
}

/// Makes the current thread's system call return with `regs`.
pub fn set_pending_continue(regs: UserRegs) {
    with(|s| {
        let id = s.current();
        s.thread(id).pending_continue = Some(regs);
    });
}

/// Takes the registers set by `set_pending_continue`, if any.
pub fn take_pending_continue() -> Option<UserRegs> {
    with(|s| {
        let id = s.current();
        s.thread(id).pending_continue.take()
    })
}

/// `nomigrate` (kernel command line, for telling migration bugs from
/// others): a thread that runs user code stays on the CPU it first ran on.
pub static NO_MIGRATE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// `checkuser` (kernel command line): every return to user mode checks
/// that this CPU is set up for the thread returning (`check_user_return`).
pub static CHECK_USER_RETURN: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// On a return to user mode (interrupts off, after the user-return hook):
/// panics unless this CPU is set up for the current thread, as the
/// switch that last put it here must have left it: its kernel stack for
/// the next entry, its address space loaded, its TEB as the user GS base,
/// and no other CPU running it.
pub fn check_user_return() {
    with(|s| {
        let here = me();
        let id = s.cpus[here].current;
        let others = (0..smp::MAX_CPUS)
            .filter(|&c| c != here && s.cpus[c].online && s.cpus[c].current == id)
            .fold(0u64, |m, c| m | 1 << c);
        let t = s.thread(id);
        let stack_top = t.stack.as_ref().map_or(0, KernelStack::top);
        let (root, gs) = t.user.as_ref().map_or((0, 0), |u| (u.root, u.gs_base));
        let personality = t.user.as_ref().map_or(Personality::Nt, |u| u.personality);
        let (cr3, kstack, ugs) =
            (paging::active_root(), percpu::kernel_stack(), percpu::user_gs_base());
        let cpu_personality = percpu::personality();
        if others != 0
            || !t.on_cpu
            || t.state != State::Running
            || kstack != stack_top
            || cr3 != root
            || ugs != gs
            || cpu_personality != personality
        {
            panic!(
                "sched: thread {id} ({}) returns to user mode on CPU {here} with others {others:#x}, \
                 on_cpu {}, state {:?}, kernel stack {kstack:#x} (its own {stack_top:#x}), \
                 cr3 {cr3:#x} (its {root:#x}), user GS {ugs:#x} (its {gs:#x}), \
                 personality {cpu_personality} (its {personality})",
                t.name, t.on_cpu, t.state
            );
        }
    });
}

/// How many times a CPU switched to thread `id` (0 for an unknown id).
pub fn context_switches(id: ThreadId) -> u32 {
    with(|s| s.threads.get(id.0).map_or(0, |t| t.switches))
}

/// The CPU the caller runs on (it may move right after).
pub fn current_cpu() -> u32 {
    percpu::cpu_index()
}

/// Sets the CPUs thread `id` may run on (`mask` must contain an online
/// CPU). A thread running on a CPU outside the mask moves at its next
/// reschedule: right away for the caller itself, else within a tick.
pub fn set_affinity(id: ThreadId, mask: u64) {
    let move_self = with(|s| {
        let here = me();
        let Some(t) = s.threads.get_mut(id.0) else { return false };
        t.affinity = mask;
        let running = t.state == State::Running;
        for c in 0..smp::MAX_CPUS {
            if running && s.cpus[c].online && s.cpus[c].current == id && mask & (1 << c) == 0 {
                if c == here {
                    return true;
                }
                s.cpus[c].need_resched = true;
            }
        }
        false
    });
    if move_self {
        schedule();
    }
}

/// Clears `on_cpu` of the thread this CPU just switched away from. Runs
/// on the new thread right after every switch (and first thing in new
/// threads).
fn finish_switch() {
    with(|s| {
        let cpu = me();
        if let Some(prev) = s.cpus[cpu].prev.take() {
            s.thread(prev).on_cpu = false;
        }
    });
}

/// Picks the next thread and switches to it. The current thread is
/// re-queued if it is still running.
fn schedule() {
    let restore = cpu::disable_interrupts();
    let switch_pair = {
        let mut guard = SCHED.lock();
        let s = guard.as_mut().expect("scheduler not initialized");
        let here = me();
        let cur = s.cpus[here].current;
        if s.thread(cur).state == State::Running {
            s.make_ready(cur);
        } else {
            // It blocks (waits, sleeps, ends): the watchdog counts afresh
            // when it runs again.
            crate::flight::blocked(cur.as_usize());
        }
        let next = s.pick(here).or(s.cpus[here].idle).unwrap_or(cur);
        s.thread(next).state = State::Running;
        if NO_MIGRATE.load(core::sync::atomic::Ordering::Relaxed) {
            let n = s.thread(next);
            if n.user.is_some() && n.affinity == ALL_CPUS {
                n.affinity = 1 << here;
            }
        }
        let cpu = &mut s.cpus[here];
        cpu.current = next;
        cpu.quantum_left = QUANTUM_TICKS;
        cpu.need_resched = false;
        // Every thread that runs, or may run, has its context.
        let has_contexts = s.thread(cur).context.is_some() && s.thread(next).context.is_some();
        (next != cur && has_contexts).then_some(()).and_then(|()| {
            s.cpus[here].prev = Some(cur);
            let from: *const Context =
                s.thread(cur).context.as_ref().map_or(core::ptr::null(), |c| c as *const Context);
            let n = s.thread(next);
            n.on_cpu = true;
            if let Some(flag) = RUNNING_IDLE.get(here) {
                flag.store(n.is_idle, core::sync::atomic::Ordering::Relaxed);
            }
            n.switches = n.switches.wrapping_add(1);
            let target = switch::Target {
                context: n.context.as_ref()?,
                kernel_stack_top: n.stack.as_ref().map(KernelStack::top),
                root: n.user.as_ref().map_or_else(paging::kernel_root, |u| u.root),
                user_gs_base: n.user.as_ref().map_or(0, |u| u.gs_base),
                user_fs_base: n.user.as_ref().map_or(0, |u| u.fs_base),
                teb32: n.user.as_ref().is_some_and(|u| u.teb32),
                personality: n.user.as_ref().map_or(Personality::Nt, |u| u.personality),
                client: n.user.as_ref().map_or(0, |u| u.client),
            };
            Some((from, target))
        })
    };
    // The lock is released before switching. `cur` keeps `on_cpu` until
    // `finish_switch`, so no other CPU runs it or frees its stack meanwhile.
    if let Some((from, target)) = switch_pair {
        switch::switch(from, target);
        finish_switch();
    }
    if restore {
        cpu::enable_interrupts();
    }
}

/// Gives up the CPU to another ready thread, if any.
pub fn yield_now() {
    // A voluntary yield is a polling wait, not a spin: progress for the
    // watchdog (preemption is not).
    crate::flight::progress(current().as_usize());
    schedule();
}

/// Blocks the current thread for at least `ms` milliseconds of real time
/// (raw ticks: clock control does not stretch the kernel's own sleeps).
pub fn sleep_ms(ms: u64) {
    let ticks = ms.saturating_mul(time::TICK_HZ).div_ceil(1000).max(1);
    with(|s| {
        let id = s.current();
        s.thread(id).state = State::Sleeping { until_tick: time::raw_ticks() + ticks };
    });
    schedule();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinError {
    NoSuchThread,
    JoinSelf,
    OutOfMemory,
}

/// Waits until thread `id` has exited.
pub fn join(id: ThreadId) -> Result<(), JoinError> {
    loop {
        let wait = with(|s| {
            let me = s.current();
            if id == me {
                return Err(JoinError::JoinSelf);
            }
            let target = s.threads.get_mut(id.0).ok_or(JoinError::NoSuchThread)?;
            if target.state == State::Dead {
                return Ok(false);
            }
            target.joiners.try_push(me).map_err(|_| JoinError::OutOfMemory)?;
            s.thread(me).state = State::Blocked;
            Ok(true)
        })?;
        if !wait {
            return Ok(());
        }
        schedule();
    }
}

/// Address-space references `reap` calls hold while they free (taken from
/// dead threads, not dropped yet): diagnostics for a leak check.
static REAPING: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// How many address-space references reaps in progress hold.
pub fn held_by_reaps() -> usize {
    REAPING.load(core::sync::atomic::Ordering::Relaxed)
}

/// `(threads, dead ones, dead ones whose stack is not freed yet)`: for
/// low-memory reports (a dead thread's stack goes at the next `reap`).
pub fn thread_counts() -> (usize, usize, usize) {
    with(|s| {
        let dead = s.threads.iter().filter(|t| t.state == State::Dead);
        let (mut n, mut stacks) = (0, 0);
        for t in dead {
            n += 1;
            if t.stack.is_some() {
                stacks += 1;
            }
        }
        (s.threads.len(), n, stacks)
    })
}

/// Frees the stacks of dead threads that no CPU runs on any more.
pub fn reap() -> usize {
    // Take the stacks out under the lock, drop (unmap) them outside it.
    let mut stacks: [Option<KernelStack>; 8] = Default::default();
    let mut spaces: [Option<SharedUserSpace>; 8] = Default::default();
    let mut contexts: [Option<Context>; 8] = Default::default();
    let mut n = 0;
    with(|s| {
        for t in s.threads.iter_mut() {
            if n == stacks.len() {
                break;
            }
            if t.state == State::Dead && !t.on_cpu && t.stack.is_some() {
                stacks[n] = t.stack.take();
                // Never switched to again: its save area goes (outside the
                // lock, with the stack).
                contexts[n] = t.context.take();
                n += 1;
                // Not active any more (a switch away loaded another root),
                // so the address space may be freed with its last thread.
                spaces[n - 1] = t.user.take().map(|u| u.space);
            }
        }
    });
    let held = spaces.iter().flatten().count();
    REAPING.fetch_add(held, core::sync::atomic::Ordering::Relaxed);
    drop(stacks);
    drop(spaces);
    REAPING.fetch_sub(held, core::sync::atomic::Ordering::Relaxed);
    n
}

/// First half of a wait: marks the current thread as waiting (until `wake`
/// or the tick `deadline`). The caller then releases the locks that protect
/// its wait condition and calls [`wait`]. A `wake` in between is not lost:
/// the thread is simply already runnable when it calls `wait`.
#[track_caller]
pub fn prepare_wait(deadline: Option<u64>) {
    let site = Location::caller();
    with(|s| {
        let id = s.current();
        let t = s.thread(id);
        t.wake_reason = WakeReason::Woken;
        t.wait_site = Some(site);
        t.state = State::Waiting { until_tick: deadline };
        if deadline.is_some_and(|d| d <= time::ticks()) {
            t.wake_reason = WakeReason::TimedOut;
            s.make_ready(id);
        }
    });
}

/// Second half of a wait: blocks until woken or timed out.
pub fn wait() -> WakeReason {
    schedule();
    with(|s| {
        let id = s.current();
        s.thread(id).wake_reason
    })
}

/// Makes a waiting thread runnable. No effect on threads in other states.
pub fn wake(id: ThreadId) {
    with(|s| {
        if let Some(t) = s.threads.get(id.0)
            && matches!(t.state, State::Waiting { .. })
        {
            s.make_ready(id);
        }
    });
}

/// Called from the boot CPU's timer interrupt (after EOI): wakes sleepers
/// (by `raw`, the real tick count) and timed-out waiters (by `now`, the
/// programs' clock: virtual under clock control), then does this CPU's
/// preemption.
pub(crate) fn on_tick(now: u64, raw: u64) {
    {
        let mut guard = SCHED.lock();
        let Some(s) = guard.as_mut() else { return };
        for i in 0..s.threads.len() {
            match s.threads[i].state {
                State::Sleeping { until_tick } if until_tick <= raw => s.make_ready(ThreadId(i)),
                State::Waiting { until_tick: Some(t) } if t <= now => {
                    s.threads[i].wake_reason = WakeReason::TimedOut;
                    s.make_ready(ThreadId(i));
                }
                _ => {}
            }
        }
    }
    on_tick_here();
}

/// Called from every CPU's timer interrupt: preempts the current thread
/// when its quantum is used up or it was asked to move.
pub(crate) fn on_tick_here() {
    let preempt = {
        let mut guard = SCHED.lock();
        let Some(s) = guard.as_mut() else { return };
        let here = me();
        if !s.cpus[here].online {
            return;
        }
        let has_work = s.has_work(here);
        let cpu = &mut s.cpus[here];
        cpu.quantum_left = cpu.quantum_left.saturating_sub(1);
        let idle_with_work = Some(cpu.current) == cpu.idle && has_work;
        if cpu.quantum_left == 0 || idle_with_work {
            cpu.need_resched = true;
        }
        if cpu.reaping && idle_with_work {
            // It switches as soon as the reap is done (`idle_loop`).
            return;
        }
        // A thread that must leave this CPU goes even with nothing queued
        // (a user thread too while users are frozen).
        let current = &s.threads[cpu.current.0];
        let must_move =
            current.affinity & (1 << here) == 0 || (s.users_frozen && current.user.is_some());
        if must_move {
            cpu.need_resched = true;
        }
        cpu.need_resched && (has_work || must_move)
    };
    if preempt {
        schedule();
    }
}

fn idle_loop(_: usize) {
    loop {
        // Not preempted while reaping: an idle thread switched away from
        // runs again only when its CPU has nothing else to do, and the
        // dead threads' stacks and address spaces it has taken would stay
        // allocated until then (on a busy CPU, for seconds: an exited
        // process's memory looked leaked, "address space still referenced
        // 8 times after exit": reap's batch of 8).
        with(|s| s.cpus[me()].reaping = true);
        reap();
        with(|s| s.cpus[me()].reaping = false);
        let has_work = {
            // Check and halt with interrupts off, so a wakeup between the
            // check and `hlt` cannot be missed (`sti; hlt` is atomic, and a
            // reschedule IPI sent meanwhile stays pending until then).
            cpu::disable_interrupts();
            with(|s| s.has_work(me()))
        };
        if has_work {
            cpu::enable_interrupts();
            schedule();
        } else {
            cpu::enable_interrupts_and_halt();
        }
    }
}

/// Stops (or lets again) the scheduler from running threads that run user
/// code (a kernel handover: `users_quiet` then tells when none is on a
/// CPU any more). Kernel threads go on.
pub fn freeze_users(frozen: bool) {
    with(|s| {
        s.users_frozen = frozen;
        if !frozen {
            // Idle CPUs look at the queue again.
            for c in 0..smp::MAX_CPUS {
                if c != me() && s.cpus[c].online && Some(s.cpus[c].current) == s.cpus[c].idle {
                    smp::send_reschedule(c as u32);
                }
            }
        }
    });
}

/// Whether no thread that runs user code is on a CPU (after
/// `freeze_users`).
pub fn users_quiet() -> bool {
    with(|s| !s.threads.iter().any(|t| t.user.is_some() && t.on_cpu))
}

/// How a thread that runs user code stands, for a kernel handover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserWait {
    /// Blocked in a wait (`prepare_wait`/`wait`) until the tick, if any.
    Waiting(Option<u64>),
    /// Anything else: running, queued, sleeping in the kernel, ...
    Busy,
    Dead,
}

/// `id`'s state and, if it is off every CPU, its saved x87/SSE/AVX state.
pub fn user_wait(id: ThreadId) -> (UserWait, Option<XState>) {
    with(|s| {
        let Some(t) = s.threads.get(id.0) else { return (UserWait::Dead, None) };
        let state = match t.state {
            State::Waiting { until_tick } if !t.on_cpu => UserWait::Waiting(until_tick),
            State::Dead => UserWait::Dead,
            _ => UserWait::Busy,
        };
        let fx = t.context.as_ref().filter(|_| !t.on_cpu).map(switch::saved_xstate);
        (state, fx)
    })
}

/// Logs every thread's scheduling state (for hangs; see `nt::thread::dump`).
pub fn dump() {
    let now = time::ticks();
    with(|s| {
        for (i, t) in s.threads.iter().enumerate() {
            info!(
                "sched: thread {i} ({}) {:?}{} affinity {:#x} (now tick {now})",
                t.name,
                t.state,
                WaitSite(&t.state, t.wait_site),
                t.affinity
            );
        }
    });
}

/// For a state dump that must not wait (an NMI): each online CPU's
/// current thread `(cpu, thread id, name, state)` if the scheduler's lock
/// is free; false if it is held.
pub fn try_cpu_threads(mut f: impl FnMut(usize, usize, &'static str, &'static str)) -> bool {
    let Some(guard) = SCHED.try_lock_untracked() else { return false };
    if let Some(s) = guard.as_ref() {
        for (cpu, c) in s.cpus.iter().enumerate().filter(|(_, c)| c.online) {
            let id = c.current.0;
            if let Some(t) = s.threads.get(id) {
                let idle = c.idle == Some(c.current);
                f(cpu, id, t.name, if idle { "idle" } else { state_name(&t.state) });
            }
        }
    }
    true
}

/// ` at <file:line>` for a waiting thread whose wait site is known.
struct WaitSite<'a>(&'a State, Option<&'static Location<'static>>);

impl fmt::Display for WaitSite<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.0, self.1) {
            (State::Waiting { .. }, Some(at)) => write!(f, " at {}:{}", at.file(), at.line()),
            _ => Ok(()),
        }
    }
}

fn state_name(state: &State) -> &'static str {
    match state {
        State::Running => "running",
        State::Ready => "ready",
        State::Blocked => "blocked",
        State::Sleeping { .. } => "sleeping",
        State::Waiting { .. } => "waiting",
        State::Dead => "dead",
    }
}

/// Logs every thread that still holds the user address space whose PML4
/// is `root` (state, whether a CPU is on its stack, which CPU runs it):
/// what keeps an exited process's address space alive. Returns how many.
pub fn dump_space_holders(root: u64) -> usize {
    let now = time::ticks();
    let mut n = 0;
    with(|s| {
        for (i, t) in s.threads.iter().enumerate() {
            if t.user.as_ref().is_some_and(|u| u.root == root) {
                n += 1;
                let running_on = s.cpus.iter().position(|c| c.online && c.current.0 == i);
                info!(
                    "sched: thread {i} ({}) {:?} on_cpu {} running on {running_on:?} \
                     affinity {:#x} stack {} (now tick {now})",
                    t.name,
                    t.state,
                    t.on_cpu,
                    t.affinity,
                    t.stack.is_some()
                );
            }
        }
    });
    n
}

/// `(id, name, state)` for every thread, for diagnostics.
pub fn for_each_thread(mut f: impl FnMut(ThreadId, &'static str, &'static str)) {
    with(|s| {
        for (i, t) in s.threads.iter().enumerate() {
            f(ThreadId(i), t.name, state_name(&t.state));
        }
    });
}
