//! Kernel threads and a round-robin, preemptive scheduler (single CPU for
//! now).
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

use crate::arch::context::{Context, KernelStack, StackError};
use crate::arch::cpu;
use crate::arch::paging::{self, SharedUserSpace};
use crate::mm::heap::{AllocError, TryVec, try_box};
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
    Sleeping { until_tick: u64 },
    Dead,
}

struct Thread {
    name: &'static str,
    state: State,
    context: Context,
    /// `None` for the boot thread (Limine's stack) and after reaping.
    stack: Option<KernelStack>,
    entry: Option<(fn(usize), usize)>,
    /// Threads blocked in `join` on this one.
    joiners: Vec<ThreadId>,
    /// Set for threads that run user code.
    user: Option<UserThread>,
}

/// User-mode side of a thread.
struct UserThread {
    /// Keeps the address space alive while the thread exists; `root` is
    /// its PML4, read without taking the space's lock.
    space: SharedUserSpace,
    root: u64,
    /// GS base in user mode (the TEB).
    gs_base: u64,
}

struct Scheduler {
    /// Indexed by `ThreadId`; entries are never removed, so ids are unique.
    /// Boxed so that each `Context` keeps its address when the vector grows.
    #[allow(clippy::vec_box)]
    threads: Vec<Box<Thread>>,
    /// Invariant: capacity >= number of threads, so pushing never allocates.
    ready: VecDeque<ThreadId>,
    current: ThreadId,
    idle: Option<ThreadId>,
    quantum_left: u32,
    need_resched: bool,
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

impl Scheduler {
    fn thread(&mut self, id: ThreadId) -> &mut Thread {
        &mut self.threads[id.0]
    }

    fn make_ready(&mut self, id: ThreadId) {
        self.thread(id).state = State::Ready;
        if Some(id) != self.idle {
            debug_assert!(self.ready.len() < self.ready.capacity());
            self.ready.push_back(id);
        }
    }

    fn add(&mut self, thread: Thread) -> Result<ThreadId, SpawnError> {
        self.ready
            .try_reserve(self.threads.len() + 1 - self.ready.len())
            .map_err(|_| AllocError)?;
        let id = ThreadId(self.threads.len());
        self.threads.try_push(try_box(thread)?)?;
        Ok(id)
    }
}

/// Adopts the running boot thread as thread #0 and starts the idle thread.
pub fn init() -> Result<(), SpawnError> {
    let boot = Thread {
        name: "boot",
        state: State::Running,
        context: Context::current(),
        stack: None,
        entry: None,
        joiners: Vec::new(),
        user: None,
    };
    let mut sched = Scheduler {
        threads: Vec::new(),
        ready: VecDeque::new(),
        current: ThreadId(0),
        idle: None,
        quantum_left: QUANTUM_TICKS,
        need_resched: false,
    };
    sched.add(boot)?;
    *SCHED.lock() = Some(sched);
    let idle = spawn("idle", idle_loop, 0)?;
    with(|s| {
        // `spawn` queued it; the idle thread is only run when nothing else is.
        s.ready.retain(|&t| t != idle);
        s.idle = Some(idle);
    });
    Ok(())
}

/// Creates a thread running `entry(arg)`. It starts on the next switch.
pub fn spawn(name: &'static str, entry: fn(usize), arg: usize) -> Result<ThreadId, SpawnError> {
    spawn_inner(name, entry, arg, None)
}

/// Creates a thread that runs `entry(arg)` in kernel mode inside the user
/// address space `space`, with user GS base `gs_base`. `entry` normally
/// ends by entering user mode.
pub fn spawn_user(
    name: &'static str,
    entry: fn(usize),
    arg: usize,
    space: SharedUserSpace,
    gs_base: u64,
) -> Result<ThreadId, SpawnError> {
    let root = space.lock().root();
    spawn_inner(name, entry, arg, Some(UserThread { space, root, gs_base }))
}

fn spawn_inner(
    name: &'static str,
    entry: fn(usize),
    arg: usize,
    user: Option<UserThread>,
) -> Result<ThreadId, SpawnError> {
    // Allocate outside the scheduler lock.
    let stack = KernelStack::new().map_err(SpawnError::Stack)?;
    let context = Context::new_thread(&stack, thread_start);
    let thread = Thread {
        name,
        state: State::Ready,
        context,
        stack: Some(stack),
        entry: Some((entry, arg)),
        joiners: Vec::new(),
        user,
    };
    let mut guard = SCHED.lock();
    let sched = guard.as_mut().ok_or(SpawnError::NotInitialized)?;
    let id = sched.add(thread)?;
    sched.make_ready(id);
    Ok(id)
}

extern "C" fn thread_start() -> ! {
    let (entry, arg) = with(|s| {
        let id = s.current;
        s.thread(id).entry.take().expect("new thread has an entry point")
    });
    cpu::enable_interrupts();
    entry(arg);
    exit()
}

/// Ends the current thread.
pub fn exit() -> ! {
    with(|s| {
        let id = s.current;
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
    with(|s| s.current)
}

pub fn current_name() -> &'static str {
    with(|s| {
        let id = s.current;
        s.thread(id).name
    })
}

/// Picks the next thread and switches to it. The current thread is
/// re-queued if it is still running.
fn schedule() {
    let restore = cpu::disable_interrupts();
    let switch_pair = {
        let mut guard = SCHED.lock();
        let s = guard.as_mut().expect("scheduler not initialized");
        let cur = s.current;
        if s.thread(cur).state == State::Running {
            s.make_ready(cur);
        }
        let next = s.ready.pop_front().or(s.idle).unwrap_or(cur);
        s.thread(next).state = State::Running;
        s.current = next;
        s.quantum_left = QUANTUM_TICKS;
        s.need_resched = false;
        (next != cur).then(|| {
            let from: *const Context = &s.thread(cur).context;
            let n = s.thread(next);
            let target = switch::Target {
                context: &n.context,
                kernel_stack_top: n.stack.as_ref().map(KernelStack::top),
                root: n.user.as_ref().map_or_else(paging::kernel_root, |u| u.root),
                user_gs_base: n.user.as_ref().map_or(0, |u| u.gs_base),
            };
            (from, target)
        })
    };
    // The lock is released before switching; with interrupts disabled on a
    // single CPU nothing can touch the threads in between.
    // TODO(smp): hand the lock over across the switch instead.
    if let Some((from, target)) = switch_pair {
        switch::switch(from, target);
    }
    if restore {
        cpu::enable_interrupts();
    }
}

/// Gives up the CPU to another ready thread, if any.
pub fn yield_now() {
    schedule();
}

/// Blocks the current thread for at least `ms` milliseconds.
pub fn sleep_ms(ms: u64) {
    let ticks = ms.saturating_mul(time::TICK_HZ).div_ceil(1000).max(1);
    with(|s| {
        let id = s.current;
        s.thread(id).state = State::Sleeping { until_tick: time::ticks() + ticks };
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
            let me = s.current;
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

/// Frees the stacks of dead threads (never the current one).
pub fn reap() -> usize {
    // Take the stacks out under the lock, drop (unmap) them outside it.
    let mut stacks: [Option<KernelStack>; 8] = Default::default();
    let mut spaces: [Option<SharedUserSpace>; 8] = Default::default();
    let mut n = 0;
    with(|s| {
        let cur = s.current;
        for (i, t) in s.threads.iter_mut().enumerate() {
            if n == stacks.len() {
                break;
            }
            if t.state == State::Dead && i != cur.0 && t.stack.is_some() {
                stacks[n] = t.stack.take();
                n += 1;
                // Not active any more (a switch away loaded another root),
                // so the address space may be freed with its last thread.
                spaces[n - 1] = t.user.take().map(|u| u.space);
            }
        }
    });
    drop(stacks);
    drop(spaces);
    n
}

/// Called from the timer interrupt (after EOI). Wakes sleepers and preempts
/// the current thread when its quantum is used up.
pub(crate) fn on_tick(now: u64) {
    let preempt = {
        let mut guard = SCHED.lock();
        let Some(s) = guard.as_mut() else { return };
        for i in 0..s.threads.len() {
            if let State::Sleeping { until_tick } = s.threads[i].state
                && until_tick <= now
            {
                s.make_ready(ThreadId(i));
            }
        }
        s.quantum_left = s.quantum_left.saturating_sub(1);
        let idle_with_work = Some(s.current) == s.idle && !s.ready.is_empty();
        if s.quantum_left == 0 || idle_with_work {
            s.need_resched = true;
        }
        s.need_resched && !s.ready.is_empty()
    };
    if preempt {
        schedule();
    }
}

fn idle_loop(_: usize) {
    loop {
        reap();
        let has_work = {
            // Check and halt with interrupts off, so a wakeup between the
            // check and `hlt` cannot be missed (`sti; hlt` is atomic).
            cpu::disable_interrupts();
            with(|s| !s.ready.is_empty())
        };
        if has_work {
            cpu::enable_interrupts();
            schedule();
        } else {
            cpu::enable_interrupts_and_halt();
        }
    }
}

/// `(id, name, state)` for every thread, for diagnostics.
pub fn for_each_thread(mut f: impl FnMut(ThreadId, &'static str, &'static str)) {
    with(|s| {
        for (i, t) in s.threads.iter().enumerate() {
            let state = match t.state {
                State::Running => "running",
                State::Ready => "ready",
                State::Blocked => "blocked",
                State::Sleeping { .. } => "sleeping",
                State::Dead => "dead",
            };
            f(ThreadId(i), t.name, state);
        }
    });
}
