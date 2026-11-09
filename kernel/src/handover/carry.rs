//! Carrying programs across a kernel handover (ROADMAP R9 milestones (b)
//! and (c), ARCHITECTURE "Programs across a handover"): the running kernel
//! brings every thread of every program to a safe point, writes the
//! programs down as data (`programs`' formats), and the next kernel
//! rebuilds the objects, the address spaces (the user pages stay in their
//! frames, image pages are laid out again from their files) and the
//! threads (ARCHITECTURE invariants 1 and 2).
//!
//! Safe points: a thread returning to user mode waits on the way
//! (`thread::hold_for_handover`; one in user mode gets there at its next
//! timer tick, one in a system call when the call returns) with its full
//! user context (every register, the x87/SSE state) kept as data; a thread
//! blocked in a call that has done nothing yet (a wait, a synchronous pipe
//! read, `syscalls::restart_as`) stays blocked and makes the call again in
//! the next kernel with its absolute deadline; a suspended thread, or one
//! created suspended, is carried as it is.
//!
//! What cannot be carried yet makes the handover refuse, before the point
//! of no return, with the reason (`Refusal`): threads blocked in other
//! system calls (or that never get to a safe point), terminating threads,
//! impersonation; console windows, jobs; registry keys, object
//! directories, the AFD
//! device and the other object kinds not listed in `programs`;
//! asynchronous pipe I/O in flight, writable views of files; files a
//! program wrote (without `persist`) or that complete to a port.
//!
//! Sockets (milestone (e)): once the threads are stopped the network is
//! stopped too (`net::tcp::stop`), and every AFD endpoint is written down
//! with its kernel socket and pending requests (`handover::sockets`).

use alloc::vec::Vec;
use core::fmt;

use super::programs::{
    self, ApcRec, EventRec, ExitedThreadRec, FileRec, FileSectionRec, HandleRec, ImageSectionRec,
    MutantRec, ObjectHead, PacketRec, PageKind, PageRec, PipeEndRec, PipeRec, ProcessRec,
    SectionRec, SemaphoreRec, ThreadRec, TimerRec, TokenRec, ViewRec, thread_state,
};
use super::record::{self, Range, Reader, Record, Wide, Writer, tag};
use super::sockets::{self, RequestRec, SocketRec};
use crate::mm::PAGE_SIZE;
use crate::mm::frame::Frame;
use crate::mm::heap::{AllocError, KArc, TryVec};
use crate::net::tcp;
use crate::nt::object::{
    CompletionPacket, CompletionQueue, Event, EventType, HandleEntry, IoCompletion, KObject,
    Mutant, MutantState, ObjectBody, ObjectRef, Semaphore, Timer, TimerState,
};
use crate::nt::platform::{self, UserRegs, XState};
use crate::nt::process::{self, Process};
use crate::nt::section::ImageLayout;
use crate::nt::status::NtStatus;
use crate::nt::thread::{Resume, UserApc};
use crate::nt::wait::Dispatcher;
use crate::nt::{afd, file, keyed_event, kuser, namespace, pipe, section, syscalls, thread, vad};
use crate::sched::{self, UserWait};
use crate::sync::IrqMutex;

/// How long the programs may take to reach safe points.
const QUIET_MS: u64 = 3000;

/// STATUS_SUCCESS, as a finished system call returns it (rax).
const SUCCESS: u64 = 0;

/// Why programs cannot be carried (the handover is not done).
#[derive(Clone, Copy, Debug)]
pub enum Refusal {
    /// A thread that does not stop where it can be carried.
    Thread {
        pid: u64,
        tid: u64,
        why: &'static str,
        call: Option<&'static str>,
    },
    /// A process holds something not carried yet.
    Process {
        pid: u64,
        what: &'static str,
    },
    /// Kernel state the programs depend on is not carried yet.
    Kernel(&'static str),
    NoMemory,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Refusal::Thread { pid, tid, why, call: Some(call) } => {
                write!(f, "thread {tid} of process {pid} {why} ({call})")
            }
            Refusal::Thread { pid, tid, why, call: None } => {
                write!(f, "thread {tid} of process {pid} {why}")
            }
            Refusal::Process { pid, what } => write!(f, "process {pid} has {what}"),
            Refusal::Kernel(what) => f.write_str(what),
            Refusal::NoMemory => f.write_str("out of memory"),
        }
    }
}

impl From<AllocError> for Refusal {
    fn from(_: AllocError) -> Self {
        Refusal::NoMemory
    }
}

/// A live thread, as carried.
struct ThreadState {
    object: ObjectRef,
    snap: thread::Snapshot,
    resume: Resume,
    apcs: Vec<UserApc>,
}

struct ProcessState {
    process: KArc<Process>,
    pages: Vec<PageRec>,
    views: Vec<ViewRec>,
    handles: Vec<HandleRec>,
}

/// The programs, collected with their threads stopped (`collect`).
pub struct Programs {
    processes: Vec<ProcessState>,
    threads: Vec<ThreadState>,
    /// Object `id` is `objects[id - 1]`.
    objects: Vec<ObjectRef>,
    /// Timers' states as they were when the threads stopped (the timer
    /// worker goes on until the jump: a timer firing after this fires
    /// again, at once, in the next kernel).
    timers: Vec<(u32, TimerState)>,
    /// The pipes the pipe ends reach (PipeRec::index is the place + 1).
    pipes: Vec<KArc<pipe::Pipe>>,
    /// Frames of owned pages and section pages, sorted.
    frames: Vec<u64>,
    /// Milestone (e): the TCP stack (stopped) and the AFD endpoints, by
    /// object id; None and empty when no program holds a socket.
    net: Option<tcp::CarriedStack>,
    sockets: Vec<SocketRec>,
}

impl Programs {
    pub fn process_count(&self) -> usize {
        self.processes.len()
    }

    pub fn thread_count(&self) -> usize {
        self.threads.len()
    }

    /// Threads stopped in user mode (interrupted or on their way back).
    pub fn user_mode_count(&self) -> usize {
        self.threads.iter().filter(|t| matches!(t.resume, Resume::User { .. })).count()
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    /// Sockets carried: (endpoints, TCP connections, listeners, UDP
    /// sockets).
    pub fn socket_counts(&self) -> (usize, usize, usize, usize) {
        let mut conns = self.net.as_ref().map_or(0, |n| n.lingering.len());
        let (mut listeners, mut udp) = (0, 0);
        for s in &self.sockets {
            match s.entry.as_ref().map(|e| &e.kind) {
                Some(tcp::CarriedKind::Conn { .. }) => conns += 1,
                Some(tcp::CarriedKind::Listener { ready, opening, .. }) => {
                    listeners += 1;
                    conns += ready.len() + opening.len();
                }
                Some(tcp::CarriedKind::Udp { .. }) => udp += 1,
                _ => {}
            }
        }
        (self.sockets.len(), conns, listeners, udp)
    }

    /// The extended state the threads carry (XCR0 bits): what this kernel
    /// enables (x87, SSE, AVX, AVX-512 where the CPU has it); the next
    /// kernel's note must say it restores all of it.
    pub fn xstate(&self) -> u64 {
        if self.threads.is_empty() { 0 } else { platform::xstate_enabled() }
    }
}

/// Lets the programs run again (a refused or failed handover).
pub fn release() {
    tcp::restart();
    sched::freeze_users(false);
    thread::hold_for_handover(false);
}

/// Stops the programs and collects them: Ok(None) if there are none;
/// Err with the reason (the programs run on) if they cannot be carried.
/// On Ok(Some) the threads stay stopped: the handover goes on, or
/// `release` lets them run.
pub fn collect() -> Result<Option<Programs>, Refusal> {
    let processes = process::running()?;
    if processes.is_empty() {
        return Ok(None);
    }
    if crate::clockctl::armed() {
        return Err(Refusal::Kernel("clock control is on (its state is not carried)"));
    }
    let collected = wait_quiet().and_then(|threads| collect_stopped(&processes, threads));
    if collected.is_err() {
        release();
    }
    collected.map(Some)
}

/// When `wait_quiet` last began to stop the programs (raw monotonic ns;
/// 0: never): from then on a thread whose wait ends does not get back to
/// user mode (`hold_for_handover`), so a sleep due then is late from
/// there. The record's log line says how long before it that was.
static STOPPED_AT_NS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
/// The same in the tick count (the record's STOPPED section).
static STOPPED_AT_TICKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The tick count when the programs were stopped for this handover.
pub fn stopped_ticks() -> Option<u64> {
    match STOPPED_AT_NS.load(core::sync::atomic::Ordering::Relaxed) {
        0 => None,
        _ => Some(STOPPED_AT_TICKS.load(core::sync::atomic::Ordering::Relaxed)),
    }
}

/// Milliseconds since the programs were stopped for a handover (None if
/// they were not).
pub fn stopped_ms() -> Option<u64> {
    match STOPPED_AT_NS.load(core::sync::atomic::Ordering::Relaxed) {
        0 => None,
        at => Some(crate::time::raw_monotonic_ns().saturating_sub(at) / 1_000_000),
    }
}

/// Brings every thread of a program to a safe point and stops it there
/// (they stay stopped on success), returning them as they were then.
/// Threads returning to user mode wait on the way (`hold_for_handover`);
/// the rest must be blocked in a call that can be made again. The
/// scheduler is only frozen once all are there (a thread frozen on its
/// way to its safe point would never reach it); a thread woken in between
/// is caught by the check after freezing. Once frozen, a thread can still
/// be woken (a timer, an event): it is queued but does not run, so its
/// wait has consumed nothing.
fn wait_quiet() -> Result<Vec<ThreadState>, Refusal> {
    STOPPED_AT_TICKS.store(crate::time::raw_ticks(), core::sync::atomic::Ordering::Relaxed);
    STOPPED_AT_NS
        .store(crate::time::raw_monotonic_ns().max(1), core::sync::atomic::Ordering::Relaxed);
    thread::hold_for_handover(true);
    let until = crate::time::SpinDeadline::after_ns(QUIET_MS * 1_000_000);
    loop {
        let mut check = check_threads().map(drop);
        if check.is_ok() {
            sched::freeze_users(true);
            let stopped = if sched::users_quiet() { check_threads() } else { Err(None) };
            match stopped {
                Ok(threads) => return Ok(threads),
                Err(why) => check = Err(why),
            }
            sched::freeze_users(false);
        }
        match check {
            Err(why) if until.passed() => {
                return Err(why.unwrap_or(Refusal::Thread {
                    pid: 0,
                    tid: 0,
                    why: "still runs on a CPU",
                    call: None,
                }));
            }
            _ => sched::sleep_ms(2),
        }
    }
}

/// A carried thread's x87/SSE/AVX state: the FXSAVE part, and the XSAVE
/// area past it (THREADS version 3's blob) when it was saved under the
/// XCR0 this kernel enables. Other extended state cannot be restored here
/// (the running kernel checked the note, `version::agree_xstate`, but this
/// kernel chose its XCR0 on this CPU): the programs are refused.
fn carried_xstate(t: &ThreadRec<'_>) -> Result<XState, NtStatus> {
    let mut state = XState::from_legacy(&t.fx);
    if t.xstate == programs::XSTATE_LEGACY && t.extended.is_empty() {
        return Ok(state);
    }
    let enabled = platform::xstate_enabled();
    let len = platform::xstate::size_of(enabled) - 512;
    if t.xstate != enabled || t.extended.len() < len {
        warn!(
            "handover: thread {} carried under XCR0 {:#x} ({} bytes), this kernel enables {:#x}",
            t.tid,
            t.xstate,
            t.extended.len(),
            enabled
        );
        return Err(NtStatus::NOT_SUPPORTED);
    }
    let legacy_bv = state.bv();
    state.bytes_mut()[512..512 + len].copy_from_slice(&t.extended[..len]);
    state.set_bv(state.bv() | legacy_bv);
    Ok(state)
}

/// Every live thread as carried, or why one cannot be (yet).
fn check_threads() -> Result<Vec<ThreadState>, Option<Refusal>> {
    let mut out = Vec::new();
    for object in thread::all().map_err(|_| Some(Refusal::NoMemory))? {
        let Some(snap) = thread::snapshot(&object) else { continue };
        let refuse =
            |why, call| Some(Refusal::Thread { pid: snap.process.id, tid: snap.id, why, call });
        let Some(sched_id) = snap.sched else { return Err(refuse("has not started", None)) };
        if snap.terminating {
            return Err(refuse("is terminating", None));
        }
        let (state, fx) = sched::user_wait(sched_id);
        let resume = if let Some(h) = &snap.held {
            // On its way back to user mode: every register.
            Resume::User { regs: h.regs, fx: h.fx.clone() }
        } else if let (Some(p), true) = (&snap.parked, snap.suspended > 0) {
            // Suspended on its way back to user mode.
            Resume::User { regs: p.regs, fx: p.fx.clone() }
        } else if let Some((number, regs)) = snap.in_syscall {
            let call = Some(syscalls::service_name(number));
            if !syscalls::restart_as(&snap.process, number, &regs) {
                return Err(refuse("is in a system call that cannot be made again", call));
            }
            let (UserWait::Waiting(deadline), Some(fx)) = (state, fx) else {
                return Err(refuse("is not blocked yet", call));
            };
            Resume::Syscall { number, regs, fx, deadline }
        } else if !snap.entered && snap.suspended > 0 && matches!(state, UserWait::Waiting(_)) {
            Resume::Start { regs: snap.start }
        } else {
            return Err(refuse("runs user code", None));
        };
        let apcs = thread::queued_apcs(&object).map_err(|_| Some(Refusal::NoMemory))?;
        let object = object.clone();
        out.try_push(ThreadState { object, snap, resume, apcs })
            .map_err(|_| Some(Refusal::NoMemory))?;
    }
    keyed_event_waits(&mut out)?;
    Ok(out)
}

/// Threads blocked in a keyed event call whose partner has already come
/// (paired, woken, not yet run) have finished their call: they are
/// carried as returning STATUS_SUCCESS, not as making it again. A thread
/// still unpaired makes its call again.
fn keyed_event_waits(threads: &mut [ThreadState]) -> Result<(), Option<Refusal>> {
    for i in 0..threads.len() {
        let Resume::Syscall { number, regs, ref fx, .. } = threads[i].resume else { continue };
        let Some(release) = syscalls::keyed_event_kind(number) else { continue };
        let process = threads[i].snap.process.clone();
        let (handle, key) = (call_arg(&regs, 0), call_arg(&regs, 1));
        let object = if handle == 0 {
            keyed_event::system().map_err(|_| Some(Refusal::NoMemory))?
        } else {
            let entry = process.handles.lock().get(crate::nt::object::Handle(handle));
            match entry {
                Some(e) => e.object,
                None => continue, // the call fails again the same way
            }
        };
        // The same call by other threads of the process.
        let same = threads
            .iter()
            .filter(|t| {
                KArc::ptr_eq(&t.snap.process, &process)
                    && matches!(t.resume, Resume::Syscall { number: n, regs: r, .. }
                        if n == number && call_arg(&r, 0) == handle && call_arg(&r, 1) == key)
            })
            .count();
        let unpaired = keyed_event::unpaired(&object, key, release, process.id);
        if unpaired == same {
            continue;
        }
        if unpaired != 0 {
            // Which of them was paired cannot be told.
            return Err(Some(Refusal::Thread {
                pid: process.id,
                tid: threads[i].snap.id,
                why: "was paired in a keyed event with others still waiting",
                call: Some(syscalls::service_name(number)),
            }));
        }
        // Finished: returns STATUS_SUCCESS as the call's return to user
        // mode would (x86-64's SYSRET: rcx = rip, r11 = rflags).
        let done = platform::SyscallFrame::from_regs(&regs, SUCCESS).return_regs();
        let fx = fx.clone();
        threads[i].resume = Resume::User { regs: done, fx };
    }
    Ok(())
}

fn collect_stopped(
    processes: &[KArc<Process>],
    threads: Vec<ThreadState>,
) -> Result<Programs, Refusal> {
    let mut p = Programs {
        processes: Vec::new(),
        threads: Vec::new(),
        objects: Vec::new(),
        timers: Vec::new(),
        pipes: Vec::new(),
        frames: Vec::new(),
        net: None,
        sockets: Vec::new(),
    };
    // The network stops before anything is written down: from here no
    // frame is taken and nothing acknowledged that the record would not
    // hold (peers send again what was not acknowledged).
    if !tcp::stop(NET_STOP_MS) {
        return Err(Refusal::Kernel("the network did not stop"));
    }
    let stopped_ns = crate::time::raw_monotonic_ns();
    // Live threads' objects first (handles to them name these ids).
    for t in &threads {
        if !processes.iter().any(|q| KArc::ptr_eq(q, &t.snap.process)) {
            return Err(Refusal::Process { pid: t.snap.process.id, what: "ended" });
        }
        id_of(&mut p.objects, &t.object)?;
    }
    for t in &threads {
        if let Some(token) = impersonation_of(&t.object) {
            id_of(&mut p.objects, &token)?;
        }
    }
    p.threads = threads;
    for process in processes {
        let state = collect_process(&mut p, process)?;
        p.processes.try_push(state)?;
    }
    // Every object reached: what it needs (objects it names get ids too,
    // so the list grows while it is walked), and its frames.
    let mut i = 0;
    while let Some(object) = p.objects.get(i).cloned() {
        i += 1;
        for needed in needs(&object)? {
            id_of(&mut p.objects, &needed)?;
        }
        let mut frames = Vec::new();
        match check_object(&p, &object, &mut frames) {
            Ok(()) => p.frames.try_extend_from_slice(&frames)?,
            Err(Some(what)) => return Err(Refusal::Process { pid: owner_of(&p, &object), what }),
            Err(None) => return Err(Refusal::NoMemory),
        }
        if let ObjectBody::Timer(t) = &object.body {
            let state = t.state.lock().clone();
            p.timers.try_push((i as u32, state))?;
        }
        if let ObjectBody::Pipe(end) = &object.body
            && !p.pipes.iter().any(|q| KArc::ptr_eq(q, end.pipe()))
        {
            p.pipes.try_push(end.pipe().clone())?;
        }
    }
    p.frames.sort_unstable();
    p.frames.dedup();
    collect_sockets(&mut p, stopped_ns)?;
    Ok(p)
}

/// How long the net thread may take to stop.
const NET_STOP_MS: u64 = 1000;

/// The programs' sockets as data (milestone (e)): every AFD endpoint
/// collected, its kernel socket and pending requests, and the stack's own
/// state if there is any socket.
fn collect_sockets(p: &mut Programs, stopped_ns: u64) -> Result<(), Refusal> {
    for i in 0..p.objects.len() {
        let ObjectBody::Socket(endpoint) = &p.objects[i].body else { continue };
        let owner = owner_of(p, &p.objects[i]);
        let refuse = |what| Refusal::Process { pid: owner, what };
        // The record writes IPv4 addresses (ROADMAP N3: IPv6 sockets
        // across a handover are a step of their own).
        if matches!(endpoint.state.transport, afd::Transport::Tcp6 | afd::Transport::Udp6) {
            return Err(refuse("an IPv6 socket"));
        }
        let c = afd::carry(&endpoint.state).map_err(|_| Refusal::NoMemory)?;
        let entry = match c.socket {
            Some(id) => Some(tcp::carry_entry(id).map_err(|e| match e {
                tcp::NetError::NoMemory => Refusal::NoMemory,
                _ => refuse("a socket whose connection cannot be written down"),
            })?),
            None => None,
        };
        let mut requests = Vec::new();
        for q in &c.requests {
            if !p.processes.iter().any(|s| KArc::ptr_eq(&s.process, &q.process)) {
                return Err(refuse("a socket request of a process that is not carried"));
            }
            let mut endpoints = Vec::new();
            for (handle, state, events) in &q.endpoints {
                let id = p
                    .objects
                    .iter()
                    .position(|o| matches!(&o.body, ObjectBody::Socket(e) if KArc::ptr_eq(&e.state, state)))
                    .ok_or(refuse("a socket request naming a socket that is not carried"))?;
                endpoints.try_push((*handle, id as u32 + 1, *events))?;
            }
            let mut buffers = Vec::new();
            buffers.try_extend_from_slice(&q.buffers)?;
            let (apc_thread, apc) = match &q.apc {
                Some((thread, a)) => (p.id(thread), [a.routine, a.args[0], a.args[1], a.args[2]]),
                None => (0, [0; 4]),
            };
            requests.try_push(RequestRec {
                kind: q.kind,
                pid: q.process.id,
                iosb: q.iosb,
                event: p.id_opt(&q.event),
                context: q.context,
                apc_thread,
                apc,
                flag: q.flag,
                done: q.done,
                output: q.output,
                output_len: q.output_len,
                receive: q.receive,
                local: q.local,
                buffers,
                endpoints,
                address: q.address,
                address_len: q.address_len,
                to: q.to,
            })?;
        }
        let (port, key) = c.completion.as_ref().map_or((0, 0), |(o, k)| (p.id(o), *k));
        let (select_event, select_mask) = c.select.as_ref().map_or((0, 0), |(o, m)| (p.id(o), *m));
        let rec = SocketRec {
            id: i as u32 + 1,
            transport: c.transport.number(),
            non_blocking: c.non_blocking,
            modes: c.modes,
            port,
            key,
            select_event,
            select_mask,
            recorded: c.recorded,
            signal: c.signal,
            entry,
            requests,
            send_side: c.send_side,
        };
        p.sockets.try_push(rec)?;
    }
    if !p.sockets.is_empty() {
        let stack = tcp::carry_stack(stopped_ns).map_err(|_| Refusal::NoMemory)?;
        if stack.lingering.iter().any(|c| c.state.tuple.is_some_and(|(l, _)| l.ip.is_v6())) {
            return Err(Refusal::Kernel("a closing IPv6 connection"));
        }
        p.net = Some(stack);
    }
    Ok(())
}

/// The objects `object` names that must be carried with it: a data
/// section's file, the thread a timer's APC goes to.
fn needs(object: &ObjectRef) -> Result<Vec<ObjectRef>, AllocError> {
    let mut out = Vec::new();
    match &object.body {
        ObjectBody::Section(s) => {
            if let Some((f, _)) = s.mapped_file() {
                out.try_push(f.clone())?;
            }
        }
        ObjectBody::Timer(t) => {
            let apc = t.state.lock().apc.as_ref().map(|a| a.0.clone());
            if let Some(thread) = apc {
                out.try_push(thread)?;
            }
        }
        // Its completion port, selected event, requests' events and
        // completion routines' threads.
        ObjectBody::Socket(e) => {
            for o in afd::referenced(&e.state).map_err(|_| AllocError)? {
                out.try_push(o)?;
            }
        }
        _ => {}
    }
    Ok(out)
}

/// The impersonation token thread `object` acts with, if any.
fn impersonation_of(object: &ObjectRef) -> Option<ObjectRef> {
    match &object.body {
        ObjectBody::Thread(t) => t.impersonation.lock().clone(),
        _ => None,
    }
}

/// The id of `object` (assigned on first sight).
fn id_of(objects: &mut Vec<ObjectRef>, object: &ObjectRef) -> Result<u32, AllocError> {
    if let Some(i) = objects.iter().position(|o| KArc::ptr_eq(o, object)) {
        return Ok(i as u32 + 1);
    }
    objects.try_push(object.clone())?;
    Ok(objects.len() as u32)
}

/// The node of the file an image section was made from.
fn image_file(s: &section::Section) -> Option<crate::fs::ramfs::NodeId> {
    let name = s.file_name.as_ref()?;
    let device: Vec<u16> = {
        let mut v = Vec::new();
        for c in file::VOLUME_DEVICE.encode_utf16() {
            v.try_push(c).ok()?;
        }
        v
    };
    let rest = name.strip_prefix(device.as_slice())?;
    let parts = file::components(rest, false).ok()?;
    file::with_fs(|fs| fs.resolve(crate::fs::ramfs::ROOT, &parts)).ok()
}

fn collect_process(p: &mut Programs, process: &KArc<Process>) -> Result<ProcessState, Refusal> {
    let pid = process.id;
    let refuse = |what| Refusal::Process { pid, what };
    // Only NT processes are carried (the PROCESSES record has no
    // personality yet; ROADMAP LX).
    if process.personality != crate::personality::Personality::Nt {
        return Err(refuse("a personality other than NT"));
    }
    // A WOW64 process's 32-bit side (its address limit, TEB32s, the
    // segment its threads need) is not in the record yet (ROADMAP WOW).
    if process.wow64.is_some() {
        return Err(refuse("32-bit code (WOW64)"));
    }
    if process.console.is_some() {
        return Err(refuse("a console window"));
    }
    if process.job.lock().is_some() {
        return Err(refuse("a job"));
    }
    // The executable is laid out again from its file.
    let exe = process::open_program(process.path).map_err(|_| refuse("no executable file"))?;
    if !matches!(&exe.body, ObjectBody::File(f) if file::survives_handover(f.node)) {
        return Err(refuse("an executable that the next kernel would not find"));
    }
    drop(exe);
    // Section views (their sections are checked with the other objects).
    let mut views = Vec::new();
    let mut view_sections = Vec::new();
    for v in process.views.lock().iter() {
        view_sections.try_push(v.section.clone())?;
        views.try_push((v.base, v.size, v.offset))?;
    }
    let mut view_recs = Vec::new();
    for ((base, size, offset), s) in views.iter().zip(&view_sections) {
        let section = id_of(&mut p.objects, s)?;
        view_recs.try_push(ViewRec { base: *base, size: *size, offset: *offset, section })?;
    }
    // The image layout of the view at `va`, and the view's base.
    let image_view = |va: u64| -> Option<(&KArc<ImageLayout>, u64)> {
        let i = views.iter().position(|&(b, s, _)| (b..b + s).contains(&va))?;
        match &view_sections[i].body {
            ObjectBody::Section(s) => s.image.as_ref().map(|l| (l, views[i].0)),
            _ => None,
        }
    };
    // The pages, as the page tables have them.
    let mut raw: Vec<(u64, Frame, crate::mm::paging::PageFlags)> = Vec::new();
    let mut full = false;
    process.space.lock().for_each_page(|va, frame, flags| {
        if raw.try_push((va, frame, flags)).is_err() {
            full = true;
        }
    });
    if full {
        return Err(Refusal::NoMemory);
    }
    let (image_base, image_size) = process.image;
    let layout = process.layout().ok_or(refuse("no PE image layout"))?;
    let mut pages = Vec::new();
    pages.try_reserve_exact(raw.len()).map_err(|_| Refusal::NoMemory)?;
    for &(va, frame, flags) in &raw {
        let page = if !flags.borrowed {
            p.frames.try_push(frame.number())?;
            PageRec { va, value: frame.number(), kind: PageKind::Owned, flags }
        } else if va == kuser::ADDRESS {
            PageRec { va, value: 0, kind: PageKind::SharedData, flags }
        } else if (image_base..image_base + image_size).contains(&va)
            && layout.shared_frame(((va - image_base) / PAGE_SIZE) as usize) == Some(frame)
        {
            PageRec { va, value: (va - image_base) / PAGE_SIZE, kind: PageKind::Image, flags }
        } else if let Some((l, base)) = image_view(va) {
            let index = ((va - base) / PAGE_SIZE) as usize;
            if l.view_frame(index, base, false) != Some(frame) {
                return Err(refuse("a DLL page the handover cannot carry"));
            }
            PageRec { va, value: index as u64, kind: PageKind::ImageView, flags }
        } else if section::frame_for(process, va).is_some_and(|(_, f)| f == frame) {
            PageRec { va, value: 0, kind: PageKind::Section, flags }
        } else {
            return Err(refuse("a mapped page the handover cannot carry"));
        };
        let _ = pages.try_push(page);
    }
    // The primary token.
    let token = process.token.lock().clone();
    id_of(&mut p.objects, &token)?;
    // Handles.
    let mut entries: Vec<(u64, HandleEntry)> = Vec::new();
    for (h, e) in process.handles.lock().iter() {
        entries.try_push((h.0, e.clone()))?;
    }
    let mut handles = Vec::new();
    for (handle, e) in &entries {
        let object = id_of(&mut p.objects, &e.object)?;
        handles.try_push(HandleRec {
            handle: *handle,
            object,
            access: e.access,
            attributes: e.attributes,
        })?;
    }
    Ok(ProcessState { process: process.clone(), pages, views: view_recs, handles })
}

/// The process whose handle reaches `object` (for a refusal's message).
fn owner_of(p: &Programs, object: &ObjectRef) -> u64 {
    let id = p.id(object);
    p.processes
        .iter()
        .find(|s| s.handles.iter().any(|h| h.object == id))
        .map_or(0, |s| s.process.id)
}

/// Whether section protection `protect` lets a view write the file.
fn writes_file(protect: u32) -> bool {
    matches!(protect, vad::PAGE_READWRITE | vad::PAGE_EXECUTE_READWRITE)
}

/// Checks that `object` can be carried (Err(Some(what)): not; Err(None):
/// out of memory); adds the frames it holds to `frames`.
fn check_object(
    p: &Programs,
    object: &ObjectRef,
    frames: &mut Vec<u64>,
) -> Result<(), Option<&'static str>> {
    let refuse = Some;
    // A name is carried if its directory is one every kernel makes.
    if let Some(n) = object.name.lock().as_ref()
        && !n.parent.permanent
    {
        return Err(refuse("a named object in a directory of its own"));
    }
    match &object.body {
        ObjectBody::Event(_) | ObjectBody::Semaphore(_) | ObjectBody::Mutant(_) => {}
        ObjectBody::ConsoleInput | ObjectBody::ConsoleOutput => {}
        ObjectBody::IoCompletion(_) => {}
        ObjectBody::KeyedEvent(_) => {
            if keyed_event::system().is_ok_and(|s| KArc::ptr_eq(&s, object)) {
                return Err(refuse("a handle to the system's keyed event"));
            }
        }
        ObjectBody::Timer(t) => {
            let apc = t.state.lock().apc.as_ref().map(|a| a.0.clone());
            if let Some(thread) = apc
                && !p.threads.iter().any(|t| KArc::ptr_eq(&t.object, &thread))
            {
                return Err(refuse("a timer whose APC goes to a thread not carried"));
            }
        }
        ObjectBody::Section(s) => {
            if s.image.is_some() {
                if !image_file(s).is_some_and(file::survives_handover) {
                    return Err(refuse("an image the next kernel would not find"));
                }
            } else if let Some((_, node)) = s.mapped_file() {
                if writes_file(s.protect) {
                    return Err(refuse("a writable view of a file"));
                }
                if !file::survives_handover(node) {
                    return Err(refuse("a view of a file the next kernel would not find"));
                }
            } else {
                for (_, frame) in s.allocated_pages().map_err(|_| None)? {
                    frames.try_push(frame.number()).map_err(|_| None)?;
                }
            }
        }
        ObjectBody::File(f) => {
            if f.completion.lock().is_some() {
                return Err(refuse("a file bound to a completion port"));
            }
            if f.deletes_on_close() {
                return Err(refuse("a file to delete on close"));
            }
            if !file::survives_handover(f.node) {
                return Err(refuse("a file the next kernel would not find (written since boot)"));
            }
        }
        ObjectBody::Pipe(end) => {
            if end.completion.lock().is_some() {
                return Err(refuse("a pipe bound to a completion port"));
            }
            // Asynchronous I/O in flight is checked when the pipe is
            // written down (`pipe::carry`); checked here too, before the
            // point of no return.
            pipe::carry(end.pipe())?;
        }
        ObjectBody::Thread(_) => {
            let snap = thread::snapshot(object).ok_or(None)?;
            let live = p.threads.iter().any(|t| KArc::ptr_eq(&t.object, object));
            let process_carried =
                p.processes.iter().any(|s| KArc::ptr_eq(&s.process, &snap.process));
            if !live && (snap.exit_status.is_none() || !process_carried) {
                return Err(refuse("a handle to a thread of another program"));
            }
        }
        ObjectBody::Process(target) => {
            if !p.processes.iter().any(|s| KArc::ptr_eq(&s.process, target)) {
                return Err(refuse("a handle to a process that is not carried"));
            }
        }
        // Sockets (milestone (e)): written down by `collect_sockets`.
        ObjectBody::Socket(_) => {}
        ObjectBody::AfdDevice => return Err(refuse("a handle to the AFD device")),
        ObjectBody::NamedPipeDevice => return Err(refuse("a handle to the pipe device")),
        ObjectBody::Key(_) => return Err(refuse("an open registry key")),
        ObjectBody::Job(_) => return Err(refuse("a job object")),
        ObjectBody::Token(_) => {}
        // Window stations and desktops are carried by nobody yet (ROADMAP
        // RS B5): a process that asked user32 for one holds a handle.
        ObjectBody::WindowStation(_) | ObjectBody::Desktop(_) => {
            return Err(refuse("a window station or desktop handle"));
        }
        // The service control manager's handles are not carried yet.
        ObjectBody::ScManager(_) | ObjectBody::Service(_) => {
            return Err(refuse("a service control manager handle"));
        }
        ObjectBody::Directory(_) | ObjectBody::SymbolicLink(_) => {
            return Err(refuse("an object directory or link handle"));
        }
        ObjectBody::Volume
        | ObjectBody::VolumeFile(_)
        | ObjectBody::DiskDevice
        | ObjectBody::Disk(_)
        | ObjectBody::NullDevice
        | ObjectBody::Null(_) => return Err(refuse("a device handle")),
    }
    Ok(())
}

/// A named object's name (empty: unnamed).
fn name_of(object: &ObjectRef) -> Result<Vec<u16>, AllocError> {
    if object.name.lock().is_none() {
        return Ok(Vec::new());
    }
    namespace::full_name(object)
}

/// The user registers in a record's order (the architecture's: x86-64's
/// 16 general-purpose registers, rip, rflags).
fn regs_of(r: &UserRegs) -> programs::Regs {
    r.to_words()
}

fn user_regs(r: &programs::Regs) -> UserRegs {
    UserRegs::from_words(r)
}

/// Argument `i` of the system call a carried thread is blocked in (its
/// registers `regs`); one in a register (the keyed event calls' handle
/// and key are).
fn call_arg(regs: &UserRegs, i: usize) -> u64 {
    match crate::nt::abi::saved_arg(regs, i) {
        crate::nt::abi::ArgSlot::Value(v) => v,
        crate::nt::abi::ArgSlot::Stack(_) => 0,
    }
}

impl Programs {
    /// The id of an object already collected.
    fn id(&self, object: &ObjectRef) -> u32 {
        self.objects.iter().position(|o| KArc::ptr_eq(o, object)).map_or(0, |i| i as u32 + 1)
    }

    fn id_opt(&self, object: &Option<ObjectRef>) -> u32 {
        object.as_ref().map_or(0, |o| self.id(o))
    }

    /// Writes the programs' sections (twice per handover: once to size
    /// the record; the programs are stopped, so both are the same).
    pub fn write(&self, w: &mut Writer) -> Result<(), AllocError> {
        programs::section(w, tag::PROCESSES)?;
        let header = w.begin_entry()?;
        w.u64(thread::peek_next_id())?;
        w.end_entry(header)?;
        for s in &self.processes {
            let q = &s.process;
            ProcessRec {
                id: q.id,
                parent_id: q.parent_id,
                create_time: q.create_time,
                image_base: q.image.0,
                image_size: q.image.1,
                exception_dispatcher: q.exception_dispatcher(),
                apc_dispatcher: q.apc_dispatcher.load(core::sync::atomic::Ordering::Relaxed),
                threads_created: q.threads_created.load(core::sync::atomic::Ordering::Relaxed),
                priority_class: u32::from(
                    q.priority_class.load(core::sync::atomic::Ordering::Relaxed),
                ),
                name: q.name,
                path: q.path,
                image_digest: q.layout().map_or(0, |l| l.digest()),
                token: self.id(&q.token.lock().clone()),
                session: Some(q.session),
                handovers: handovers_of(q.id).saturating_add(1),
            }
            .write(w)?;
        }
        programs::section(w, tag::ADDRESS_SPACES)?;
        for s in &self.processes {
            let e = w.begin_entry()?;
            w.u64(s.process.id)?;
            s.process.vad.lock().write_to(w)?;
            w.table(s.views.len() as u32, ViewRec::STRIDE)?;
            for v in &s.views {
                v.write(w)?;
            }
            w.table(s.pages.len() as u32, PageRec::STRIDE)?;
            for page in &s.pages {
                page.write(w)?;
            }
            w.end_entry(e)?;
        }
        // Each thread's whole extended state: the FXSAVE part in `fx`, the
        // XSAVE area past it (header and components, standard format) in
        // the blob, under this kernel's XCR0. With any blob filled, THREADS
        // needs a version-3 reader (`programs::threads_must_read`).
        let enabled = platform::xstate_enabled();
        let size = platform::xstate::size_of(enabled);
        let extended = enabled & !programs::XSTATE_LEGACY != 0 && !self.threads.is_empty();
        w.section_needing(
            tag::THREADS,
            programs::version_of(tag::THREADS),
            record::REQUIRED,
            programs::threads_must_read(extended),
        )?;
        for t in &self.threads {
            let (state, number, regs, xs, deadline, mxcsr) = match &t.resume {
                Resume::Syscall { number, regs, fx, deadline } => {
                    (thread_state::SYSCALL, *number, *regs, Some(fx), *deadline, None)
                }
                Resume::User { regs, fx } => (thread_state::USER, 0, *regs, Some(fx), None, None),
                // A START thread's x87/SSE state is fresh: no MXCSR.
                Resume::Start { regs } => (thread_state::START, 0, *regs, None, None, None),
            };
            let fx = xs.map_or([0; 512], |s| *s.legacy());
            let (xstate, blob): (u64, &[u8]) = match xs {
                Some(s) if extended => (enabled, &s.bytes()[512..size]),
                _ => (programs::XSTATE_LEGACY, &[]),
            };
            let mut apcs: Vec<ApcRec> = Vec::new();
            for a in &t.apcs {
                apcs.try_push([a.routine, a.args[0], a.args[1], a.args[2]])?;
            }
            ThreadRec {
                tid: t.snap.id,
                pid: t.snap.process.id,
                teb: t.snap.teb,
                win32_start: t.snap.win32_start,
                create_time: t.snap.create_time,
                affinity: t.snap.affinity,
                object: self.id(&t.object),
                ordinal: t.snap.ordinal,
                priority: t.snap.priority.0,
                base_priority: t.snap.priority.1,
                number,
                regs: regs_of(&regs),
                deadline,
                fx,
                state,
                suspend: t.snap.suspended,
                mxcsr,
                impersonation: impersonation_of(&t.object).map_or(0, |o| self.id(&o)),
                xstate,
                extended: blob,
            }
            .write(w, &apcs)?;
        }
        self.write_objects(w)?;
        programs::section(w, tag::HANDLES)?;
        for s in &self.processes {
            let e = w.begin_entry()?;
            w.u64(s.process.id)?;
            w.table(s.handles.len() as u32, HandleRec::STRIDE)?;
            for h in &s.handles {
                h.write(w)?;
            }
            w.end_entry(e)?;
        }
        record::write_ranges_flagged(
            w,
            tag::USER_FRAMES,
            record::REQUIRED,
            frame_ranges(&self.frames),
        )
    }

    fn write_objects(&self, w: &mut Writer) -> Result<(), AllocError> {
        // The pipes first (their ends name them).
        programs::section(w, tag::PIPES)?;
        for (i, pipe) in self.pipes.iter().enumerate() {
            let c = pipe::carry(pipe).map_err(|_| AllocError)?;
            PipeRec {
                index: i as u32 + 1,
                name: Wide(&[]),
                max_instances: c.max_instances,
                message_type: c.message_type,
                quota: c.quota,
                conn: c.conn,
                server_open: c.server_open,
                generation: c.generation,
                inbox: [&c.inbox[0], &c.inbox[1]],
            }
            .write(w, &c.name, [&c.messages[0], &c.messages[1]])?;
        }
        // One pass per kind, in the order of their tags.
        let kinds = [
            tag::EVENTS,
            tag::SEMAPHORES,
            tag::MUTANTS,
            tag::TIMERS,
            tag::SECTIONS,
            tag::FILES,
            tag::IO_COMPLETIONS,
            tag::EXITED_THREADS,
            tag::PROCESS_OBJECTS,
            tag::CONSOLES,
            tag::IMAGE_SECTIONS,
            tag::FILE_SECTIONS,
            tag::PIPE_ENDS,
            tag::KEYED_EVENTS,
            tag::TOKENS,
        ];
        for kind in kinds {
            programs::section(w, kind)?;
            for (i, object) in self.objects.iter().enumerate() {
                let id = i as u32 + 1;
                self.write_object(w, kind, id, object)?;
            }
        }
        programs::section(w, tag::SECURITY)?;
        for (i, object) in self.objects.iter().enumerate() {
            let sd = object.security.lock();
            if let Some(sd) = sd.as_ref() {
                programs::write_security(w, i as u32 + 1, sd)?;
            }
        }
        // Milestone (e): only when a program holds a socket.
        if let Some(net) = &self.net {
            sockets::write_net(w, net)?;
            sockets::sockets_section(w, &self.sockets)?;
            for s in &self.sockets {
                s.write(w)?;
            }
        }
        Ok(())
    }

    fn write_object(
        &self,
        w: &mut Writer,
        kind: u32,
        id: u32,
        object: &ObjectRef,
    ) -> Result<(), AllocError> {
        let head = ObjectHead { id, name: Wide(&[]) };
        match (&object.body, kind) {
            (ObjectBody::Event(e), tag::EVENTS) => {
                let kind = u32::from(e.kind == EventType::Synchronization);
                let signal = e.dispatcher.signal_state();
                EventRec { head, kind, signal }.write(w, &name_of(object)?)?;
            }
            (ObjectBody::Semaphore(s), tag::SEMAPHORES) => {
                let count = s.dispatcher.signal_state();
                SemaphoreRec { head, count, maximum: s.maximum }.write(w, &name_of(object)?)?;
            }
            (ObjectBody::Mutant(m), tag::MUTANTS) => {
                let st = *m.state.lock();
                MutantRec {
                    head,
                    owner: st.owner.unwrap_or(0),
                    count: st.count,
                    abandoned: st.abandoned,
                }
                .write(w, &name_of(object)?)?;
            }
            (ObjectBody::Timer(t), tag::TIMERS) => {
                let st = match self.timers.iter().find(|(i, _)| *i == id) {
                    Some((_, st)) => st.clone(),
                    None => t.state.lock().clone(),
                };
                let apc = st
                    .apc
                    .as_ref()
                    .map(|(thread, routine, context)| (self.id(thread), *routine, *context));
                TimerRec {
                    head,
                    synchronization: t.synchronization,
                    signal: t.dispatcher.signal_state(),
                    due: st.due,
                    due_time: st.due_time,
                    period: st.period,
                    apc,
                }
                .write(w, &name_of(object)?)?;
            }
            (ObjectBody::Section(s), tag::SECTIONS) if s.is_anonymous() => {
                let pages = s.allocated_pages()?;
                let rec =
                    SectionRec { head, size: s.size, protect: s.protect, attributes: s.attributes };
                let frames = pages.iter().map(|&(i, f)| (i as u64, f.number()));
                rec.write(w, &name_of(object)?, pages.len() as u32, frames)?;
            }
            (ObjectBody::Section(s), tag::IMAGE_SECTIONS) => {
                let Some(layout) = &s.image else { return Ok(()) };
                let node = image_file(s).ok_or(AllocError)?;
                let path = file::volume_path(node).map_err(|_| AllocError)?;
                ImageSectionRec {
                    head,
                    digest: layout.digest(),
                    size: layout.size(),
                    dynamic: layout.dynamic().unwrap_or(0),
                    path: Wide(&[]),
                }
                .write(w, &name_of(object)?, &path)?;
            }
            (ObjectBody::Section(s), tag::FILE_SECTIONS) => {
                let Some((f, _)) = s.mapped_file() else { return Ok(()) };
                FileSectionRec { head, size: s.size, protect: s.protect, file: self.id(f) }
                    .write(w, &name_of(object)?)?;
            }
            (ObjectBody::File(f), tag::FILES) => {
                let path = file::volume_path(f.node).map_err(|_| AllocError)?;
                let shape = if f.directory {
                    file::FILE_DIRECTORY_FILE
                } else {
                    file::FILE_NON_DIRECTORY_FILE
                };
                FileRec {
                    id,
                    granted: f.granted,
                    share: f.share_mask(),
                    options: f.mode | shape,
                    position: *f.position.lock(),
                    path: Wide(&[]),
                }
                .write(w, &path)?;
            }
            (ObjectBody::IoCompletion(c), tag::IO_COMPLETIONS) => {
                let e = w.begin_entry()?;
                let name = name_of(object)?;
                w.u32(id)?;
                w.wide(&name)?;
                let queue = c.queue.lock();
                w.table(queue.len() as u32, PacketRec::STRIDE)?;
                for p in queue.packets() {
                    PacketRec {
                        key: p.key,
                        context: p.context,
                        information: p.information,
                        status: p.status,
                    }
                    .write(w)?;
                }
                drop(queue);
                w.end_entry(e)?;
            }
            (ObjectBody::Thread(_), tag::EXITED_THREADS) => {
                if self.threads.iter().any(|t| KArc::ptr_eq(&t.object, object)) {
                    return Ok(());
                }
                let Some(s) = thread::snapshot(object) else { return Ok(()) };
                ExitedThreadRec {
                    id,
                    exit_status: s.exit_status.map_or(0, |x| x.0),
                    tid: s.id,
                    pid: s.process.id,
                    teb: s.teb,
                    win32_start: s.win32_start,
                    create_time: s.create_time,
                    exit_time: s.exit_time,
                }
                .write(w)?;
            }
            (ObjectBody::Process(q), tag::PROCESS_OBJECTS) => {
                programs::write_process_object(w, id, q.id)?;
            }
            (ObjectBody::ConsoleInput, tag::CONSOLES) => programs::write_console(w, id, true)?,
            (ObjectBody::ConsoleOutput, tag::CONSOLES) => programs::write_console(w, id, false)?,
            (ObjectBody::Pipe(end), tag::PIPE_ENDS) => {
                let c = end.carried();
                let pipe = self.pipes.iter().position(|q| KArc::ptr_eq(q, end.pipe()));
                PipeEndRec {
                    id,
                    pipe: pipe.map_or(0, |i| i as u32 + 1),
                    end: c.end,
                    synchronous: c.synchronous,
                    message_reads: c.message_reads,
                    modes: c.modes,
                    generation: c.generation,
                }
                .write(w)?;
            }
            (ObjectBody::KeyedEvent(_), tag::KEYED_EVENTS) => {
                programs::write_keyed_event(w, id, &name_of(object)?)?;
            }
            (ObjectBody::Token(t), tag::TOKENS) => write_token(w, id, t)?,
            _ => {}
        }
        Ok(())
    }
}

/// A TOKENS entry for token `t`.
fn write_token(w: &mut Writer, id: u32, t: &crate::nt::token::Token) -> Result<(), AllocError> {
    use crate::nt::token::TokenType;
    let d = t.data.lock().try_clone().map_err(|_| AllocError)?;
    let (kind, level) = match d.kind {
        TokenType::Primary => (0, 0),
        TokenType::Impersonation(level) => (1, level),
    };
    let mut groups = Vec::new();
    for g in &d.groups {
        groups.try_push((g.sid.to_padded(), g.attributes))?;
    }
    let mut restricted = Vec::new();
    for g in &d.restricted {
        restricted.try_push((g.sid.to_padded(), g.attributes))?;
    }
    let mut privileges = Vec::new();
    for p in &d.privileges {
        privileges.try_push((p.luid, p.attributes))?;
    }
    TokenRec {
        id,
        kind,
        level,
        token_id: t.id,
        origin: t.origin,
        user: (d.user.sid.to_padded(), d.user.attributes),
        primary_group: d.primary_group.to_padded(),
        owner: d.owner.to_padded(),
        default_dacl: &d.default_dacl,
        authentication_id: d.authentication_id,
        modified_id: d.modified_id,
        session: d.session,
        elevation_type: d.elevation_type,
        source_name: d.source.0,
        source_id: d.source.1,
        sandbox_inert: d.sandbox_inert,
    }
    .write(w, &groups, &privileges, &restricted)
}

/// A token object from a TOKENS entry.
fn read_token(e: &mut Reader<'_>) -> Result<(u32, ObjectRef), NtStatus> {
    use crate::nt::token::{Group, Privilege, Sid, Token, TokenData, TokenType};
    let (t, tables) = TokenRec::read(e);
    let sid = |b: &[u8]| Sid::parse(b).ok_or(NtStatus::INVALID_SID);
    let groups_of = |table: &record::Table<'_>| -> Result<Vec<Group>, NtStatus> {
        let mut v = Vec::new();
        for mut g in table.iter() {
            let (s, attributes) = programs::read_group(&mut g);
            v.try_push(Group { sid: sid(&s)?, attributes }).map_err(no_memory)?;
        }
        Ok(v)
    };
    let mut privileges = Vec::new();
    for mut p in tables[2].iter() {
        let (luid, attributes) = programs::read_privilege(&mut p);
        privileges.try_push(Privilege { luid, attributes }).map_err(no_memory)?;
    }
    let mut default_dacl = Vec::new();
    default_dacl.try_extend_from_slice(t.default_dacl).map_err(no_memory)?;
    let data = TokenData {
        kind: match t.kind {
            0 => TokenType::Primary,
            1 => TokenType::Impersonation(t.level),
            _ => return Err(NtStatus::INVALID_PARAMETER),
        },
        user: Group { sid: sid(&t.user.0)?, attributes: t.user.1 },
        groups: groups_of(&tables[0])?,
        privileges,
        restricted: groups_of(&tables[1])?,
        primary_group: sid(&t.primary_group)?,
        owner: sid(&t.owner)?,
        default_dacl,
        authentication_id: t.authentication_id,
        modified_id: t.modified_id,
        session: t.session,
        elevation_type: t.elevation_type,
        source: (t.source_name, t.source_id),
        sandbox_inert: t.sandbox_inert,
    };
    let token = Token::adopt(t.token_id, t.origin, data);
    Ok((t.id, new_object(ObjectBody::Token(token))?))
}

/// Sorted frame numbers as physical ranges.
fn frame_ranges(frames: &[u64]) -> impl Iterator<Item = Range> + '_ {
    let mut i = 0;
    core::iter::from_fn(move || {
        let first = *frames.get(i)?;
        let mut n = 1;
        while frames.get(i + n as usize) == Some(&(first + n)) {
            n += 1;
        }
        i += n as usize;
        Some(Range { base: first * PAGE_SIZE, len: n * PAGE_SIZE, what: 0 })
    })
}

// The receiving kernel.

/// The processes a handover brought in (`run=` waits for them instead of
/// starting the program again).
static RESUMED: IrqMutex<Vec<KArc<Process>>> = IrqMutex::new(Vec::new());

/// (pid, handovers come through) of the processes a handover brought, for
/// PROCESSES version 4 (`handover_next`; the default build keeps none).
static HANDOVERS: IrqMutex<Vec<(u64, u32)>> = IrqMutex::named("handover.counts", Vec::new());

fn note_handovers(pid: u64, count: u32) -> Result<(), AllocError> {
    HANDOVERS.lock().try_push((pid, count))
}

/// The handovers process `pid` has come through (0: started here).
fn handovers_of(pid: u64) -> u32 {
    HANDOVERS.lock().iter().find(|e| e.0 == pid).map_or(0, |e| e.1)
}

/// Takes the carried process named `name` (an executable's file name), if
/// any (the list then forgets it, so its memory goes when it ends).
pub fn resumed(name: &str) -> Option<KArc<Process>> {
    let mut list = RESUMED.lock();
    let i = list.iter().position(|p| p.name.eq_ignore_ascii_case(name))?;
    Some(list.swap_remove(i))
}

/// Rebuilds the programs a handover record carries and lets their threads
/// go on. After `nt::init` and C:'s files.
pub fn restore(rec: &Record<'_>) {
    if rec.section(tag::PROCESSES).is_none() {
        return;
    }
    match restore_all(rec) {
        Ok((processes, threads)) => {
            // How long the programs were stopped (the clock at the record).
            let clock = rec.section(tag::CLOCK).map(|mut c| record::Clock::read(&mut c));
            let at = clock.as_ref().map_or(0, record::Clock::monotonic_ns);
            let now = crate::time::raw_monotonic_ns();
            // In the tick count that waits' deadlines and GetTickCount
            // use, from when the programs stopped (just before the record;
            // the record's own time if the writer did not say): what a
            // sleep spanning the handover can be late by.
            let stopped = rec.section(tag::STOPPED).map(|mut s| s.u64());
            let ticks = crate::time::raw_ticks().saturating_sub(
                stopped.unwrap_or_else(|| clock.as_ref().map_or(0, record::Clock::ticks)),
            );
            let pause_ms = ticks * 1000 / crate::time::TICK_HZ;
            crate::nt::syscalls::set_handover_pause(super::generation(), pause_ms);
            crate::info!(
                "handover: {processes} process(es) and {threads} thread(s) resumed, {} ms after \
                 the record (stopped {pause_ms} ms of the tick count)",
                now.saturating_sub(at) / 1_000_000
            );
        }
        Err(e) => crate::error!("handover: cannot resume the programs: {e:?}"),
    }
}

/// Objects by id while rebuilding.
struct Objects(Vec<Option<ObjectRef>>);

impl Objects {
    fn set(&mut self, id: u32, object: ObjectRef) -> Result<(), NtStatus> {
        let i = id.checked_sub(1).ok_or(NtStatus::INVALID_PARAMETER)? as usize;
        if i >= 1 << 20 {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        while self.0.len() <= i {
            self.0.try_push(None).map_err(|_| NtStatus::NO_MEMORY)?;
        }
        if self.0[i].is_some() {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        self.0[i] = Some(object);
        Ok(())
    }

    fn get(&self, id: u32) -> Option<&ObjectRef> {
        self.0.get(id.checked_sub(1)? as usize)?.as_ref()
    }
}

fn no_memory(_: AllocError) -> NtStatus {
    NtStatus::NO_MEMORY
}

/// Links a carried object under its name.
fn name(object: &ObjectRef, name: Wide<'_>) -> Result<(), NtStatus> {
    if name.is_empty() {
        return Ok(());
    }
    namespace::create(None, &name.to_vec().map_err(no_memory)?, object.clone(), false, false)?;
    Ok(())
}

/// Runs `f` on each entry of section `tag`; an error names the section.
fn each(
    rec: &Record<'_>,
    tag: u32,
    mut f: impl FnMut(Reader<'_>) -> Result<(), NtStatus>,
) -> Result<(), Failed> {
    let Some(mut r) = rec.section(tag) else { return Ok(()) };
    let mut index = 0;
    while let Some(e) = r.entry() {
        f(e).map_err(|status| Failed { section: tag, index, status })?;
        index += 1;
    }
    Ok(())
}

/// Where rebuilding the programs failed: the section and entry.
#[derive(Debug)]
pub struct Failed {
    pub section: u32,
    pub index: u32,
    pub status: NtStatus,
}

impl From<NtStatus> for Failed {
    fn from(status: NtStatus) -> Self {
        Failed { section: 0, index: 0, status }
    }
}

fn new_object(body: ObjectBody) -> Result<ObjectRef, NtStatus> {
    KObject::new(body).map_err(no_memory)
}

/// The layout of the image file at `path` (from the volume root) in this
/// kernel, checked against the previous kernel's digest.
fn image_layout_at(path: &[u16], digest: u64) -> Result<KArc<ImageLayout>, NtStatus> {
    let parts = file::components(path, false)?;
    let node =
        file::with_fs(|fs| fs.resolve(crate::fs::ramfs::ROOT, &parts)).map_err(file::fs_status)?;
    let layout = section::image_layout(node)?;
    if digest != 0 && layout.digest() != digest {
        return Err(NtStatus::INVALID_IMAGE_HASH);
    }
    Ok(layout)
}

/// The system ntdll's section object, if `path` is its file and its layout
/// has `digest` (every process shares that one section).
fn system_ntdll(path: &[u16], digest: u64) -> Option<ObjectRef> {
    let ntdll = crate::nt::ntdll::get()?;
    let ObjectBody::Section(s) = &ntdll.section.body else { return None };
    let node = image_file(s)?;
    let same_file = file::volume_path(node).is_ok_and(|p| p == path);
    let same_bytes = s.image.as_ref().is_some_and(|l| l.digest() == digest);
    (same_file && same_bytes).then_some(ntdll.section)
}

fn restore_all(rec: &Record<'_>) -> Result<(usize, usize), Failed> {
    let mut objects = Objects(Vec::new());
    if let Some(mut header) = rec.section(tag::PROCESSES).and_then(|mut r| r.entry()) {
        thread::continue_ids_from(header.u64());
    }
    // Objects that need nothing else.
    each(rec, tag::EVENTS, |mut e| {
        let ev = EventRec::read(&mut e);
        let kind = if ev.kind == 1 { EventType::Synchronization } else { EventType::Notification };
        let o =
            new_object(ObjectBody::Event(Event { kind, dispatcher: Dispatcher::new(ev.signal) }))?;
        name(&o, ev.head.name)?;
        objects.set(ev.head.id, o)
    })?;
    each(rec, tag::SEMAPHORES, |mut e| {
        let s = SemaphoreRec::read(&mut e);
        let o = new_object(ObjectBody::Semaphore(Semaphore {
            dispatcher: Dispatcher::new(s.count),
            maximum: s.maximum,
        }))?;
        name(&o, s.head.name)?;
        objects.set(s.head.id, o)
    })?;
    let mut timers = Vec::new();
    let mut timer_apcs = Vec::new();
    each(rec, tag::TIMERS, |mut e| {
        let t = TimerRec::read(&mut e);
        let state = TimerState {
            due: t.due,
            due_time: t.due_time,
            period: t.period,
            // Only clock control moves the system time alone, and a
            // handover is refused while it is armed.
            absolute: false,
            apc: None,
        };
        let o = new_object(ObjectBody::Timer(Timer {
            dispatcher: Dispatcher::new(t.signal),
            synchronization: t.synchronization,
            state: IrqMutex::new(state),
        }))?;
        name(&o, t.head.name)?;
        if t.due.is_some() {
            timers.try_push(o.clone()).map_err(no_memory)?;
        }
        if let Some(apc) = t.apc {
            timer_apcs.try_push((o.clone(), apc)).map_err(no_memory)?;
        }
        objects.set(t.head.id, o)
    })?;
    each(rec, tag::SECTIONS, |mut e| {
        let (s, frames) = SectionRec::read(&mut e);
        let frames = frames.iter().map(|mut f| (f.u64() as usize, Frame::from_number(f.u64())));
        let section = section::Section::adopt(s.size, s.protect, s.attributes, frames)?;
        let o = new_object(ObjectBody::Section(section))?;
        name(&o, s.head.name)?;
        objects.set(s.head.id, o)
    })?;
    each(rec, tag::IMAGE_SECTIONS, |mut e| {
        let s = ImageSectionRec::read(&mut e);
        let path = s.path.to_vec().map_err(no_memory)?;
        let o = match system_ntdll(&path, s.digest) {
            Some(o) => o,
            None => {
                let layout = image_layout_at(&path, s.digest)?;
                let mut section = section::Section::with_layout(layout);
                let parts = file::components(&path, false)?;
                let node = file::with_fs(|fs| fs.resolve(crate::fs::ramfs::ROOT, &parts))
                    .map_err(file::fs_status)?;
                section.file_name = Some(file::nt_path(node)?);
                new_object(ObjectBody::Section(section))?
            }
        };
        let ObjectBody::Section(section) = &o.body else { return Err(NtStatus::INTERNAL_ERROR) };
        let layout = section.image.as_ref().ok_or(NtStatus::INTERNAL_ERROR)?;
        if layout.size() != s.size {
            return Err(NtStatus::INVALID_IMAGE_FORMAT);
        }
        if s.dynamic != 0 {
            layout.adopt_dynamic(s.dynamic)?;
        }
        name(&o, s.head.name)?;
        objects.set(s.head.id, o)
    })?;
    each(rec, tag::FILES, |mut e| {
        let f = FileRec::read(&mut e);
        let path = f.path.to_vec().map_err(no_memory)?;
        let o = file::reopen(&path, f.granted, f.share, f.options, f.position)?;
        objects.set(f.id, o)
    })?;
    each(rec, tag::FILE_SECTIONS, |mut e| {
        let s = FileSectionRec::read(&mut e);
        let f = objects.get(s.file).ok_or(NtStatus::INVALID_HANDLE)?.clone();
        let ObjectBody::File(file_body) = &f.body else {
            return Err(NtStatus::OBJECT_TYPE_MISMATCH);
        };
        let node = file_body.node;
        let section = section::Section::new_file(f.clone(), node, s.size, s.protect)?;
        let o = new_object(ObjectBody::Section(section))?;
        name(&o, s.head.name)?;
        objects.set(s.head.id, o)
    })?;
    let mut pipes: Vec<(u32, KArc<pipe::Pipe>)> = Vec::new();
    each(rec, tag::PIPES, |mut e| {
        let (p, messages) = PipeRec::read(&mut e);
        let copy = |b: &[u8]| -> Result<Vec<u8>, NtStatus> {
            let mut v = Vec::new();
            v.try_extend_from_slice(b).map_err(no_memory)?;
            Ok(v)
        };
        let lengths = |t: &record::Table<'_>| -> Result<Vec<u64>, NtStatus> {
            let mut v = Vec::new();
            for mut m in t.iter() {
                v.try_push(m.u64()).map_err(no_memory)?;
            }
            Ok(v)
        };
        let carried = pipe::Carried {
            name: p.name.to_vec().map_err(no_memory)?,
            max_instances: p.max_instances,
            message_type: p.message_type,
            quota: p.quota,
            conn: p.conn,
            server_open: p.server_open,
            generation: p.generation,
            inbox: [copy(p.inbox[0])?, copy(p.inbox[1])?],
            messages: [lengths(&messages[0])?, lengths(&messages[1])?],
        };
        pipes.try_push((p.index, pipe::adopt(carried)?)).map_err(no_memory)
    })?;
    each(rec, tag::PIPE_ENDS, |mut e| {
        let r = PipeEndRec::read(&mut e);
        let (_, p) = pipes.iter().find(|(i, _)| *i == r.pipe).ok_or(NtStatus::INVALID_HANDLE)?;
        let end = pipe::adopt_end(
            p.clone(),
            pipe::CarriedEnd {
                end: r.end,
                generation: r.generation,
                synchronous: r.synchronous,
                message_reads: r.message_reads,
                modes: r.modes,
            },
        )?;
        objects.set(r.id, new_object(ObjectBody::Pipe(end))?)
    })?;
    each(rec, tag::TOKENS, |mut e| {
        let (id, o) = read_token(&mut e)?;
        objects.set(id, o)
    })?;
    each(rec, tag::KEYED_EVENTS, |mut e| {
        let head = programs::read_keyed_event(&mut e);
        let o = new_object(ObjectBody::KeyedEvent(keyed_event::KeyedEvent::default()))?;
        name(&o, head.name)?;
        objects.set(head.id, o)
    })?;
    each(rec, tag::IO_COMPLETIONS, |mut e| {
        let head_id = e.u32();
        let head_name = e.wide();
        let mut queue = CompletionQueue::default();
        for mut p in e.table().iter() {
            let p = PacketRec::read(&mut p);
            queue
                .push(CompletionPacket::new(p.key, p.context, p.status, p.information))
                .map_err(no_memory)?;
        }
        let o = new_object(ObjectBody::IoCompletion(IoCompletion {
            dispatcher: Dispatcher::new(0),
            queue: IrqMutex::new(queue),
        }))?;
        name(&o, head_name)?;
        objects.set(head_id, o)
    })?;
    each(rec, tag::CONSOLES, |mut e| {
        let (id, input) = programs::read_console(&mut e);
        let body = if input { ObjectBody::ConsoleInput } else { ObjectBody::ConsoleOutput };
        objects.set(id, new_object(body)?)
    })?;
    // Sockets (milestone (e)): the stack's own state, then the endpoints
    // with their kernel sockets; their requests once the processes and
    // threads they name are there.
    let sockets = restore_sockets(rec, &mut objects)?;

    // Processes with their address spaces.
    let mut processes: Vec<KArc<Process>> = Vec::new();
    let mut first = true;
    each(rec, tag::PROCESSES, |mut e| {
        if core::mem::take(&mut first) {
            return Ok(()); // the header (the next NT id)
        }
        let p = ProcessRec::read(&mut e);
        let process = restore_process(rec, &p, &objects)?;
        if programs::version_of(tag::PROCESSES) >= 4 {
            // 0: an older writer did not count (this one is the first
            // counted).
            let count = p.handovers.max(1);
            note_handovers(process.id, count).map_err(no_memory)?;
            crate::info!(
                "handover: process {} has come through {count} handover(s){}",
                process.name,
                if p.handovers == 0 {
                    " (counted from here: an older PROCESSES version)"
                } else {
                    ""
                }
            );
        }
        processes.try_push(process).map_err(no_memory)
    })?;
    let find = |pid: u64| processes.iter().find(|p| p.id == pid).cloned();
    each(rec, tag::PROCESS_OBJECTS, |mut e| {
        let (id, pid) = programs::read_process_object(&mut e);
        let p = find(pid).ok_or(NtStatus::INVALID_CID)?;
        objects.set(id, new_object(ObjectBody::Process(p))?)
    })?;

    // Threads: objects first, started last.
    let mut live = Vec::new();
    each(rec, tag::THREADS, |mut e| {
        let (t, apc_table) = ThreadRec::read(&mut e);
        let fx = carried_xstate(&t)?;
        let process = find(t.pid).ok_or(NtStatus::INVALID_CID)?;
        let regs = user_regs(&t.regs);
        let resume = match t.state {
            thread_state::SYSCALL => {
                if !regs.is_valid() || !syscalls::restartable(t.number) {
                    return Err(NtStatus::INVALID_PARAMETER);
                }
                Resume::Syscall { number: t.number, regs, fx, deadline: t.deadline }
            }
            thread_state::USER => {
                if !regs.is_valid() {
                    return Err(NtStatus::INVALID_PARAMETER);
                }
                Resume::User { regs, fx }
            }
            thread_state::START => {
                if !regs.is_valid() {
                    return Err(NtStatus::INVALID_PARAMETER);
                }
                // An MXCSR from an older writer is ignored: a new thread
                // starts with a fresh x87/SSE state.
                Resume::Start { regs }
            }
            _ => return Err(NtStatus::INVALID_PARAMETER),
        };
        let mut apcs = Vec::new();
        for mut a in apc_table.iter() {
            let [routine, a0, a1, a2] = programs::read_apc(&mut a);
            apcs.try_push(UserApc { routine, args: [a0, a1, a2] }).map_err(no_memory)?;
        }
        let o = thread::adopt(thread::Adopted {
            id: t.tid,
            teb: t.teb,
            process,
            win32_start: t.win32_start,
            create_time: t.create_time,
            priority: (t.priority, t.base_priority),
            affinity: t.affinity,
            ordinal: t.ordinal,
            resume: Some(resume),
            exited: None,
            suspend: t.suspend,
            apcs,
        })?;
        if t.impersonation != 0 {
            let token = objects.get(t.impersonation).ok_or(NtStatus::INVALID_HANDLE)?.clone();
            if let ObjectBody::Thread(nt) = &o.body {
                *nt.impersonation.lock() = Some(token);
            }
        }
        live.try_push((t.tid, o.clone())).map_err(no_memory)?;
        objects.set(t.object, o)
    })?;
    each(rec, tag::EXITED_THREADS, |mut e| {
        let x = ExitedThreadRec::read(&mut e);
        let process = find(x.pid).ok_or(NtStatus::INVALID_CID)?;
        let o = thread::adopt(thread::Adopted {
            id: x.tid,
            teb: x.teb,
            process,
            win32_start: x.win32_start,
            create_time: x.create_time,
            priority: (thread::PROCESS_BASE_PRIORITY, thread::PROCESS_BASE_PRIORITY),
            affinity: 0,
            ordinal: 0,
            resume: None,
            exited: Some((NtStatus(x.exit_status), x.exit_time)),
            suspend: 0,
            apcs: Vec::new(),
        })?;
        objects.set(x.id, o)
    })?;
    each(rec, tag::MUTANTS, |mut e| {
        let m = MutantRec::read(&mut e);
        let owner = (m.owner != 0).then_some(m.owner);
        let state = MutantState { owner, count: m.count, abandoned: m.abandoned };
        let o = new_object(ObjectBody::Mutant(Mutant {
            dispatcher: Dispatcher::new(i32::from(owner.is_none())),
            state: IrqMutex::new(state),
        }))?;
        if let Some(tid) = owner {
            let (_, t) = live.iter().find(|(id, _)| *id == tid).ok_or(NtStatus::INVALID_CID)?;
            thread::note_owner(t, &o)?;
        }
        name(&o, m.head.name)?;
        objects.set(m.head.id, o)
    })?;
    // Timers' APCs go to threads rebuilt now.
    for (timer, (thread_id, routine, context)) in &timer_apcs {
        let thread = objects.get(*thread_id).ok_or(NtStatus::INVALID_HANDLE)?.clone();
        if let ObjectBody::Timer(t) = &timer.body {
            t.state.lock().apc = Some((thread, *routine, *context));
        }
    }

    each(rec, tag::SOCKETS, |mut e| {
        let s = SocketRec::read(&mut e).map_err(socket_damage)?;
        let object = objects.get(s.id).ok_or(NtStatus::INVALID_HANDLE)?.clone();
        let mut requests = Vec::new();
        for q in s.requests {
            let process = find(q.pid).ok_or(NtStatus::INVALID_CID)?;
            let get = |id: u32| match id {
                0 => Ok(None),
                id => objects.get(id).cloned().map(Some).ok_or(NtStatus::INVALID_HANDLE),
            };
            let apc = match get(q.apc_thread)? {
                Some(thread) => {
                    let [routine, a0, a1, a2] = q.apc;
                    Some((thread, UserApc { routine, args: [a0, a1, a2] }))
                }
                None => None,
            };
            let mut endpoints = Vec::new();
            for (handle, id, events) in q.endpoints {
                let o = get(id)?.ok_or(NtStatus::INVALID_HANDLE)?;
                let ObjectBody::Socket(ep) = &o.body else {
                    return Err(NtStatus::OBJECT_TYPE_MISMATCH);
                };
                endpoints.try_push((handle, ep.state.clone(), events)).map_err(no_memory)?;
            }
            requests
                .try_push(afd::CarriedRequest {
                    kind: q.kind,
                    process,
                    iosb: q.iosb,
                    event: get(q.event)?,
                    context: q.context,
                    apc,
                    flag: q.flag,
                    done: q.done,
                    output: q.output,
                    output_len: q.output_len,
                    receive: q.receive,
                    local: q.local,
                    buffers: q.buffers,
                    endpoints,
                    address: q.address,
                    address_len: q.address_len,
                    to: q.to,
                })
                .map_err(no_memory)?;
        }
        afd::adopt_requests(&object, requests)
    })?;
    if let Some((count, stopped_ns)) = sockets {
        let now = crate::time::raw_monotonic_ns();
        crate::info!(
            "handover: {count} socket(s) resumed; the network was stopped for {} ms",
            now.saturating_sub(stopped_ns) / 1_000_000
        );
    }

    // Handle tables.
    each(rec, tag::HANDLES, |mut e| {
        let pid = e.u64();
        let process = find(pid).ok_or(NtStatus::INVALID_CID)?;
        let mut table = process.handles.lock();
        for mut h in e.table().iter() {
            let h = HandleRec::read(&mut h);
            let object = objects.get(h.object).ok_or(NtStatus::INVALID_HANDLE)?.clone();
            let entry = HandleEntry { object, access: h.access, attributes: h.attributes };
            table
                .insert_at(crate::nt::object::Handle(h.handle), entry)
                .map_err(|_| NtStatus::INVALID_HANDLE)?;
        }
        Ok(())
    })?;
    each(rec, tag::SECURITY, |mut e| {
        let (id, sd) = programs::read_security(&mut e);
        let object = objects.get(id).ok_or(NtStatus::INVALID_HANDLE)?;
        let mut copy = Vec::new();
        copy.try_extend_from_slice(sd).map_err(no_memory)?;
        *object.security.lock() = Some(copy);
        Ok(())
    })?;
    for t in &timers {
        crate::nt::timer::adopt(t)?;
    }

    // Go.
    for (_, t) in &live {
        thread::start_adopted(t)?;
    }
    for p in &processes {
        crate::info!(
            "handover: resumed process {} (pid {}): {} thread(s)",
            p.name,
            p.id,
            p.thread_count()
        );
        RESUMED.lock().try_push(p.clone()).map_err(no_memory)?;
    }
    Ok((processes.len(), live.len()))
}

fn socket_damage(e: Option<AllocError>) -> NtStatus {
    if e.is_some() { NtStatus::NO_MEMORY } else { NtStatus::INVALID_PARAMETER }
}

/// NET and the first pass over SOCKETS: the stack's carried state (the
/// card's address, the port counter, closing connections) and every
/// endpoint with its kernel socket rebuilt. Returns how many endpoints
/// and when the old kernel stopped the network (None: no sockets).
fn restore_sockets(
    rec: &Record<'_>,
    objects: &mut Objects,
) -> Result<Option<(usize, u64)>, Failed> {
    let Some(mut net) = rec.section(tag::NET) else { return Ok(None) };
    let fail = |section, status| Failed { section, index: 0, status };
    let stack = sockets::read_net(&mut net).map_err(|_| fail(tag::NET, NtStatus::NO_MEMORY))?;
    tcp::adopt_stack(&stack).map_err(|e| fail(tag::NET, afd::status_of(e)))?;
    let mut count = 0;
    each(rec, tag::SOCKETS, |mut e| {
        let s = SocketRec::read(&mut e).map_err(socket_damage)?;
        let get = |id: u32| match id {
            0 => Ok(None),
            id => objects.get(id).cloned().map(Some).ok_or(NtStatus::INVALID_HANDLE),
        };
        let c = afd::CarriedEndpoint {
            transport: afd::Transport::from_number(s.transport)
                .ok_or(NtStatus::INVALID_PARAMETER)?,
            socket: None,
            completion: get(s.port)?.map(|o| (o, s.key)),
            modes: s.modes,
            non_blocking: s.non_blocking,
            select: get(s.select_event)?.map(|o| (o, s.select_mask)),
            recorded: s.recorded,
            signal: s.signal,
            send_side: s.send_side,
            requests: Vec::new(),
        };
        let object = afd::adopt(&c, s.entry.as_ref())?;
        count += 1;
        objects.set(s.id, object)
    })?;
    Ok(Some((count, stack.stopped_ns)))
}

/// A process and its address space, from PROCESSES and ADDRESS_SPACES.
fn restore_process(
    rec: &Record<'_>,
    p: &ProcessRec<'_>,
    objects: &Objects,
) -> Result<KArc<Process>, NtStatus> {
    let mut spaces = rec.section(tag::ADDRESS_SPACES).ok_or(NtStatus::INVALID_PARAMETER)?;
    let mut e = loop {
        let mut e = spaces.entry().ok_or(NtStatus::INVALID_PARAMETER)?;
        if e.u64() == p.id {
            break e;
        }
    };
    let name = process::intern(p.name).map_err(no_memory)?;
    let path = process::intern(p.path).map_err(no_memory)?;
    let exe = process::open_program(path)?;
    let ObjectBody::File(f) = &exe.body else { return Err(NtStatus::OBJECT_TYPE_MISMATCH) };
    let layout = section::image_layout(f.node)?;
    drop(exe);
    if layout.base != p.image_base || layout.size() != p.image_size {
        return Err(NtStatus::INVALID_IMAGE_FORMAT);
    }
    if p.image_digest != 0 && layout.digest() != p.image_digest {
        return Err(NtStatus::INVALID_IMAGE_HASH);
    }
    let vad = vad::Vad::read_from(&mut e)?;
    let mut views = Vec::new();
    for mut v in e.table().iter() {
        let v = ViewRec::read(&mut v);
        let section = objects.get(v.section).ok_or(NtStatus::INVALID_HANDLE)?.clone();
        views
            .try_push(section::View { base: v.base, size: v.size, offset: v.offset, section })
            .map_err(no_memory)?;
    }
    let mut space = crate::nt::platform::UserSpace::new().map_err(|_| NtStatus::NO_MEMORY)?;
    for mut page in e.table().iter() {
        let page = PageRec::read(&mut page).ok_or(NtStatus::INVALID_PARAMETER)?;
        let view = views.iter().find(|v| (v.base..v.base + v.size).contains(&page.va));
        let frame = match page.kind {
            PageKind::Owned => {
                let frame = Frame::from_number(page.value);
                if space.map_adopted(page.va, frame, page.flags).is_err() {
                    return Err(NtStatus::NO_MEMORY);
                }
                continue;
            }
            PageKind::SharedData => {
                kuser::map_page(&mut space).map_err(|_| NtStatus::NO_MEMORY)?;
                continue;
            }
            PageKind::Image => layout.shared_frame(page.value as usize),
            PageKind::Section => view.and_then(|v| {
                let index = (page.va - v.base + v.offset) / PAGE_SIZE;
                match &v.section.body {
                    ObjectBody::Section(s) => s.frame(index as usize),
                    _ => None,
                }
            }),
            PageKind::ImageView => view.and_then(|v| match &v.section.body {
                ObjectBody::Section(s) => {
                    s.image.as_ref()?.view_frame(page.value as usize, v.base, true)
                }
                _ => None,
            }),
        };
        let mapped = frame.is_some_and(|f| space.map_borrowed(page.va, f, page.flags).is_ok());
        if !mapped {
            return Err(NtStatus::NO_MEMORY);
        }
    }
    process::adopt(process::Adopted {
        id: p.id,
        parent_id: p.parent_id,
        name,
        path,
        space,
        vad,
        views,
        image: (p.image_base, p.image_size),
        layout,
        exception_dispatcher: p.exception_dispatcher,
        apc_dispatcher: p.apc_dispatcher,
        create_time: p.create_time,
        threads_created: p.threads_created,
        priority_class: p.priority_class as u8,
        token: match p.token {
            0 => None,
            id => Some(objects.get(id).ok_or(NtStatus::INVALID_HANDLE)?.clone()),
        },
        session: p.session,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_become_ranges() {
        let frames = [1u64, 2, 3, 7, 9, 10];
        let r: Vec<_> = frame_ranges(&frames).map(|r| (r.base, r.len)).collect();
        assert_eq!(r, [(0x1000, 0x3000), (0x7000, 0x1000), (0x9000, 0x2000)]);
        assert_eq!(frame_ranges(&[]).count(), 0);
    }

    #[test]
    fn registers_round_trip() {
        let words = core::array::from_fn(|i| i as u64 * 7 + 1);
        let r = UserRegs::from_words(&words);
        assert_eq!(regs_of(&r), words);
        assert_eq!(regs_of(&user_regs(&regs_of(&r))), words);
    }
}
