//! Carrying programs across a kernel handover (ROADMAP R9 milestone (b),
//! ARCHITECTURE "Kernel handover"): the running kernel stops its programs
//! at a point where every thread is blocked in a wait it can make again,
//! writes them down as data (`programs`' formats), and the next kernel
//! rebuilds the objects, the address spaces (the user pages stay in their
//! frames) and the threads, which make their blocked system call again
//! with the same arguments and the same absolute deadline (ARCHITECTURE
//! invariants 1 and 2).
//!
//! What cannot be carried yet makes the handover refuse, before the point
//! of no return, with the reason (`Refusal`): threads that run, or are in
//! another system call; suspended or terminating threads, queued APCs,
//! impersonation; consoles windows, jobs, DLL and file views; sockets,
//! pipes, registry keys, tokens, keyed events and the other object kinds
//! not listed in `programs`; timers with an APC; files a program wrote
//! (without `persist`) or that complete to a port.

use alloc::vec::Vec;
use core::fmt;

use super::programs::{
    self, EventRec, ExitedThreadRec, FileRec, HandleRec, MutantRec, ObjectHead, PacketRec,
    PageKind, PageRec, ProcessRec, SectionRec, SemaphoreRec, ThreadRec, TimerRec, ViewRec,
};
use super::record::{self, Range, Reader, Record, Wide, Writer, tag};
use crate::mm::PAGE_SIZE;
use crate::mm::frame::Frame;
use crate::mm::heap::{AllocError, KArc, TryVec};
use crate::nt::object::{
    CompletionPacket, CompletionQueue, Event, EventType, HandleEntry, IoCompletion, KObject,
    Mutant, MutantState, ObjectBody, ObjectRef, Semaphore, Timer, TimerState,
};
use crate::nt::platform::{FxState, UserRegs};
use crate::nt::process::{self, Process};
use crate::nt::status::NtStatus;
use crate::nt::wait::Dispatcher;
use crate::nt::{file, kuser, namespace, section, syscalls, thread, vad};
use crate::sched::{self, UserWait};
use crate::sync::IrqMutex;

/// How long the programs may take to be all blocked in waits.
const QUIET_MS: u64 = 3000;

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
    number: u32,
    regs: UserRegs,
    fx: FxState,
    deadline: Option<u64>,
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
    /// Frames of owned pages and section pages, sorted.
    frames: Vec<u64>,
}

impl Programs {
    pub fn process_count(&self) -> usize {
        self.processes.len()
    }

    pub fn thread_count(&self) -> usize {
        self.threads.len()
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn object_count(&self) -> usize {
        self.objects.len()
    }
}

/// Lets the programs run again (a refused or failed handover).
pub fn release() {
    sched::freeze_users(false);
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
    let threads = wait_quiet()?;
    let collected = collect_stopped(&processes, threads);
    if collected.is_err() {
        release();
    }
    collected.map(Some)
}

/// Waits until every thread of a program is blocked in a wait that can be
/// made again, then stops them there (they stay stopped on success) and
/// returns them as they were then. Threads are not stopped while they get
/// there: a thread stopped in user mode on its way to its wait would never
/// reach it. Once stopped, a thread can still be woken (a timer, an
/// event): it is queued but does not run, so its wait has consumed
/// nothing and making the call again is still right.
fn wait_quiet() -> Result<Vec<ThreadState>, Refusal> {
    let until = crate::time::raw_monotonic_ns() + QUIET_MS * 1_000_000;
    loop {
        // All blocked now? Then stop them, and check again (one may have
        // been woken in between and be on its way back to user mode).
        let mut check = check_threads().map(drop);
        if check.is_ok() {
            sched::freeze_users(true);
            let stopped = if sched::users_quiet() { check_threads() } else { Err(None) };
            match stopped {
                Ok(threads) => return Ok(threads),
                Err(why) => check = Err(why),
            }
            release();
        }
        match check {
            Err(why) if crate::time::raw_monotonic_ns() >= until => {
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

/// Every live thread as carried, or why one cannot be.
fn check_threads() -> Result<Vec<ThreadState>, Option<Refusal>> {
    let mut out = Vec::new();
    for object in thread::all().map_err(|_| Some(Refusal::NoMemory))? {
        let Some(snap) = thread::snapshot(&object) else { continue };
        let refuse =
            |why, call| Some(Refusal::Thread { pid: snap.process.id, tid: snap.id, why, call });
        let Some(sched_id) = snap.sched else { return Err(refuse("has not started", None)) };
        if snap.suspended > 0 {
            return Err(refuse("is suspended", None));
        }
        if snap.terminating {
            return Err(refuse("is terminating", None));
        }
        if snap.apcs > 0 {
            return Err(refuse("has user APCs queued", None));
        }
        if snap.impersonating {
            return Err(refuse("impersonates", None));
        }
        let (state, fx) = sched::user_wait(sched_id);
        let Some((number, regs)) = snap.in_syscall else {
            return Err(refuse("runs user code", None));
        };
        let call = Some(syscalls::service_name(number));
        if !syscalls::restartable(number) {
            return Err(refuse("is in a system call that cannot be made again", call));
        }
        let (UserWait::Waiting(deadline), Some(fx)) = (state, fx) else {
            return Err(refuse("is not blocked yet", call));
        };
        let object = object.clone();
        out.try_push(ThreadState { object, snap, number, regs, fx, deadline })
            .map_err(|_| Some(Refusal::NoMemory))?;
    }
    Ok(out)
}

fn collect_stopped(
    processes: &[KArc<Process>],
    threads: Vec<ThreadState>,
) -> Result<Programs, Refusal> {
    let mut p = Programs {
        processes: Vec::new(),
        threads: Vec::new(),
        objects: Vec::new(),
        frames: Vec::new(),
    };
    // Live threads' objects first (handles to them name these ids).
    for t in &threads {
        if !processes.iter().any(|q| KArc::ptr_eq(q, &t.snap.process)) {
            return Err(Refusal::Process { pid: t.snap.process.id, what: "ended" });
        }
        id_of(&mut p.objects, &t.object)?;
    }
    p.threads = threads;
    for process in processes {
        let state = collect_process(&mut p, process)?;
        p.processes.try_push(state)?;
    }
    // Every object reached: what it needs, and its frames.
    for object in &p.objects {
        let mut frames = Vec::new();
        match check_object(&p, object, &mut frames) {
            Ok(()) => p.frames.try_extend_from_slice(&frames)?,
            Err(Some(what)) => return Err(Refusal::Process { pid: owner_of(&p, object), what }),
            Err(None) => return Err(Refusal::NoMemory),
        }
    }
    p.frames.sort_unstable();
    p.frames.dedup();
    Ok(p)
}

/// The id of `object` (assigned on first sight).
fn id_of(objects: &mut Vec<ObjectRef>, object: &ObjectRef) -> Result<u32, AllocError> {
    if let Some(i) = objects.iter().position(|o| KArc::ptr_eq(o, object)) {
        return Ok(i as u32 + 1);
    }
    objects.try_push(object.clone())?;
    Ok(objects.len() as u32)
}

fn collect_process(p: &mut Programs, process: &KArc<Process>) -> Result<ProcessState, Refusal> {
    let pid = process.id;
    let refuse = |what| Refusal::Process { pid, what };
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
    // Section views (pagefile-backed only).
    let mut views = Vec::new();
    let mut view_sections = Vec::new();
    for v in process.views.lock().iter() {
        match &v.section.body {
            ObjectBody::Section(s) if s.is_anonymous() => {}
            ObjectBody::Section(s) if s.image.is_some() => {
                return Err(refuse("a DLL or image view"));
            }
            _ => return Err(refuse("a view of a file")),
        }
        view_sections.try_push(v.section.clone())?;
        views.try_push((v.base, v.size, v.offset))?;
    }
    let mut view_recs = Vec::new();
    for ((base, size, offset), s) in views.iter().zip(&view_sections) {
        let section = id_of(&mut p.objects, s)?;
        view_recs.try_push(ViewRec { base: *base, size: *size, offset: *offset, section })?;
    }
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
    let layout = process.layout();
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
        } else if section::frame_for(process, va).is_some_and(|(_, f)| f == frame) {
            PageRec { va, value: 0, kind: PageKind::Section, flags }
        } else {
            return Err(refuse("a mapped page the handover cannot carry"));
        };
        let _ = pages.try_push(page);
    }
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
        ObjectBody::Timer(t) => {
            let apc = t.state.lock().apc.is_some();
            if apc {
                return Err(refuse("a timer with an APC"));
            }
        }
        ObjectBody::Section(s) => {
            if !s.is_anonymous() {
                return Err(refuse("a file or image section"));
            }
            for (_, frame) in s.allocated_pages().map_err(|_| None)? {
                frames.try_push(frame.number()).map_err(|_| None)?;
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
        ObjectBody::Socket(_) | ObjectBody::AfdDevice => return Err(refuse("an open socket")),
        ObjectBody::Pipe(_) | ObjectBody::NamedPipeDevice => return Err(refuse("a pipe")),
        ObjectBody::Key(_) => return Err(refuse("an open registry key")),
        ObjectBody::Job(_) => return Err(refuse("a job object")),
        ObjectBody::KeyedEvent(_) => return Err(refuse("a keyed event")),
        ObjectBody::Token(_) => return Err(refuse("a token handle")),
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

impl Programs {
    /// The id of an object already collected.
    fn id(&self, object: &ObjectRef) -> u32 {
        self.objects.iter().position(|o| KArc::ptr_eq(o, object)).map_or(0, |i| i as u32 + 1)
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
        programs::section(w, tag::THREADS)?;
        for t in &self.threads {
            let g = &t.regs.gpr;
            let mut regs = [0u64; 18];
            regs[..16].copy_from_slice(g);
            regs[16] = t.regs.rip;
            regs[17] = t.regs.rflags;
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
                number: t.number,
                regs,
                deadline: t.deadline,
                fx: t.fx.0,
            }
            .write(w)?;
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
        ];
        for kind in kinds {
            programs::section(w, kind)?;
            for (i, object) in self.objects.iter().enumerate() {
                let id = i as u32 + 1;
                let head = ObjectHead { id, name: Wide(&[]) };
                match (&object.body, kind) {
                    (ObjectBody::Event(e), tag::EVENTS) => {
                        let kind = u32::from(e.kind == EventType::Synchronization);
                        let signal = e.dispatcher.signal_state();
                        EventRec { head, kind, signal }.write(w, &name_of(object)?)?;
                    }
                    (ObjectBody::Semaphore(s), tag::SEMAPHORES) => {
                        let count = s.dispatcher.signal_state();
                        SemaphoreRec { head, count, maximum: s.maximum }
                            .write(w, &name_of(object)?)?;
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
                        let st = t.state.lock().clone();
                        TimerRec {
                            head,
                            synchronization: t.synchronization,
                            signal: t.dispatcher.signal_state(),
                            due: st.due,
                            due_time: st.due_time,
                            period: st.period,
                        }
                        .write(w, &name_of(object)?)?;
                    }
                    (ObjectBody::Section(s), tag::SECTIONS) => {
                        let pages = s.allocated_pages()?;
                        let rec = SectionRec {
                            head,
                            size: s.size,
                            protect: s.protect,
                            attributes: s.attributes,
                        };
                        let frames = pages.iter().map(|&(i, f)| (i as u64, f.number()));
                        rec.write(w, &name_of(object)?, pages.len() as u32, frames)?;
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
                            continue;
                        }
                        let Some(s) = thread::snapshot(object) else { continue };
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
                    (ObjectBody::ConsoleInput, tag::CONSOLES) => {
                        programs::write_console(w, id, true)?
                    }
                    (ObjectBody::ConsoleOutput, tag::CONSOLES) => {
                        programs::write_console(w, id, false)?
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
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

/// Takes the carried process named `name` (an executable's file name), if
/// any (the list then forgets it, so its memory goes when it ends).
pub fn resumed(name: &str) -> Option<KArc<Process>> {
    let mut list = RESUMED.lock();
    let i = list.iter().position(|p| p.name.eq_ignore_ascii_case(name))?;
    Some(list.swap_remove(i))
}

/// Rebuilds the programs a handover record carries and lets their threads
/// make their blocked calls again. After `nt::init` and C:'s files.
pub fn restore(rec: &Record<'_>) {
    if rec.section(tag::PROCESSES).is_none() {
        return;
    }
    match restore_all(rec) {
        Ok((processes, threads)) => {
            // How long the programs were stopped (the clock at the record).
            let at =
                rec.section(tag::CLOCK).map_or(0, |mut c| record::Clock::read(&mut c).monotonic_ns);
            let now = crate::time::raw_monotonic_ns();
            crate::info!(
                "handover: {processes} process(es) and {threads} thread(s) resumed, {} ms after \
                 the record",
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
    each(rec, tag::TIMERS, |mut e| {
        let t = TimerRec::read(&mut e);
        let state = TimerState { due: t.due, due_time: t.due_time, period: t.period, apc: None };
        let o = new_object(ObjectBody::Timer(Timer {
            dispatcher: Dispatcher::new(t.signal),
            synchronization: t.synchronization,
            state: IrqMutex::new(state),
        }))?;
        name(&o, t.head.name)?;
        if t.due.is_some() {
            timers.try_push(o.clone()).map_err(no_memory)?;
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
    each(rec, tag::FILES, |mut e| {
        let f = FileRec::read(&mut e);
        let path = f.path.to_vec().map_err(no_memory)?;
        let o = file::reopen(&path, f.granted, f.share, f.options, f.position)?;
        objects.set(f.id, o)
    })?;
    each(rec, tag::IO_COMPLETIONS, |mut e| {
        let head_id = e.u32();
        let head_name = e.wide();
        let mut queue = CompletionQueue::default();
        for mut p in e.table().iter() {
            let p = PacketRec::read(&mut p);
            queue
                .push(CompletionPacket {
                    key: p.key,
                    context: p.context,
                    status: p.status,
                    information: p.information,
                })
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

    // Processes with their address spaces.
    let mut processes: Vec<KArc<Process>> = Vec::new();
    let mut first = true;
    each(rec, tag::PROCESSES, |mut e| {
        if core::mem::take(&mut first) {
            return Ok(()); // the header (the next NT id)
        }
        let p = ProcessRec::read(&mut e);
        let process = restore_process(rec, &p, &objects)?;
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
        let t = ThreadRec::read(&mut e);
        let process = find(t.pid).ok_or(NtStatus::INVALID_CID)?;
        let mut gpr = [0u64; 16];
        gpr.copy_from_slice(&t.regs[..16]);
        let regs = UserRegs { gpr, rip: t.regs[16], rflags: t.regs[17] };
        if !regs.is_valid() || !syscalls::restartable(t.number) {
            return Err(NtStatus::INVALID_PARAMETER);
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
            resume: Some(thread::Resume {
                number: t.number,
                regs,
                fx: FxState(t.fx),
                deadline: t.deadline,
            }),
            exited: None,
        })?;
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
        let mapped = match page.kind {
            PageKind::Owned => {
                space.map_adopted(page.va, Frame::from_number(page.value), page.flags).is_ok()
            }
            PageKind::Image => match layout.shared_frame(page.value as usize) {
                Some(frame) => space.map_borrowed(page.va, frame, page.flags).is_ok(),
                None => false,
            },
            PageKind::SharedData => kuser::map_page(&mut space).is_ok(),
            PageKind::Section => {
                let view = views.iter().find(|v| (v.base..v.base + v.size).contains(&page.va));
                let frame = view.and_then(|v| {
                    let index = (page.va - v.base + v.offset) / PAGE_SIZE;
                    match &v.section.body {
                        ObjectBody::Section(s) => s.frame(index as usize),
                        _ => None,
                    }
                });
                frame.is_some_and(|f| space.map_borrowed(page.va, f, page.flags).is_ok())
            }
        };
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
}
