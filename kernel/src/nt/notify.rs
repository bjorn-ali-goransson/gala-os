//! Directory change notification (NtNotifyChangeDirectoryFile; kernel32's
//! ReadDirectoryChangesW).
//!
//! A watch belongs to one open of a directory (keyed by its file object).
//! The ramfs records changes in a journal while any watch exists;
//! `with_fs` hands the journal to `collect` before releasing the file
//! system (so names can be made relative to each watched directory), and
//! `deliver` then completes requests outside it. Changes that arrive while
//! no request waits are kept, as FILE_NOTIFY_INFORMATION records, up to
//! the size of the last request's buffer; beyond it the next request
//! completes with STATUS_NOTIFY_ENUM_DIR (rescan). A request completes
//! the way AFD's do: IO_STATUS_BLOCK, event, the file object's signal, a
//! packet to its completion port, its completion routine. CancelIoEx ends
//! it with STATUS_CANCELLED; the open's close with STATUS_NOTIFY_CLEANUP.

use alloc::vec::Vec;

use super::object::{CompletionPacket, ObjectBody, ObjectRef};
use super::process::Process;
use super::status::NtStatus;
use super::thread::{self, UserApc};
use super::wait::{self, Dispatcher};
use crate::fs::ramfs::{self, Change, NodeId, RamFs};
use crate::mm::heap::{KArc, TryVec};
use crate::sync::IrqMutex;

/// STATUS_NOTIFY_CLEANUP / STATUS_NOTIFY_ENUM_DIR (success codes).
pub const NOTIFY_CLEANUP: NtStatus = NtStatus(0x0000_010b);
pub const NOTIFY_ENUM_DIR: NtStatus = NtStatus(0x0000_010c);
pub const CANCELLED: NtStatus = NtStatus(0xc000_0120);

// FILE_NOTIFY_CHANGE_* filter bits.
const CHANGE_FILE_NAME: u32 = 0x1;
const CHANGE_DIR_NAME: u32 = 0x2;
const CHANGE_SIZE: u32 = 0x8;
const CHANGE_LAST_WRITE: u32 = 0x10;

/// What a request's completion will post, reserved when it starts: room
/// for the packet in its completion port's queue and for its completion
/// routine in its thread's APC queue (Windows allocates both with the
/// request, the IRP), so a completion never fails for lack of memory.
/// Slots not used are given back when this is dropped.
pub struct Reserved {
    packet: IrqMutex<Option<Slot>>,
    apc: IrqMutex<Option<Slot>>,
    /// The request as one of its thread's outstanding ones
    /// (ThreadIsIoPending), for as long as this lives; None for one that
    /// is not the thread's (thread-agnostic: its file has a completion
    /// port; registry watches; carried across a kernel handover). Taken
    /// when the request completes, before anything the caller can see.
    issuer: IrqMutex<Option<thread::IoPending>>,
}

/// One reserved slot in a port's queue or a thread's APC queue.
struct Slot {
    target: ObjectRef,
    apc: bool,
    /// Filled: nothing to give back.
    used: bool,
}

impl Drop for Slot {
    fn drop(&mut self) {
        if self.used {
            return;
        }
        if self.apc {
            thread::release_apc(&self.target);
        } else if let ObjectBody::IoCompletion(c) = &self.target.body {
            c.queue.lock().release_slot();
        }
    }
}

impl Reserved {
    /// Nothing reserved.
    pub const fn none() -> Reserved {
        Reserved {
            packet: IrqMutex::new(None),
            apc: IrqMutex::new(None),
            issuer: IrqMutex::new(None),
        }
    }

    /// Reserves the packet a completion will post to `port` and the APC it
    /// will queue to `apc_thread`. STATUS_INSUFFICIENT_RESOURCES without
    /// memory, as an IRP that cannot be allocated.
    pub fn new(port: Option<&ObjectRef>, apc_thread: Option<&ObjectRef>) -> Result<Self, NtStatus> {
        let r = Reserved::none();
        if let Some(port) = port
            && let ObjectBody::IoCompletion(c) = &port.body
        {
            c.queue.lock().reserve_slot().map_err(|_| NtStatus::INSUFFICIENT_RESOURCES)?;
            *r.packet.lock() = Some(Slot { target: port.clone(), apc: false, used: false });
        }
        if let Some(t) = apc_thread {
            thread::reserve_apc(t)?;
            *r.apc.lock() = Some(Slot { target: t.clone(), apc: true, used: false });
        }
        Ok(r)
    }

    /// Counts the request as one of `issuer`'s outstanding ones while it
    /// is (`thread::IoPending`).
    pub fn issued_by(self, issuer: Option<thread::IoPending>) -> Self {
        *self.issuer.lock() = issuer;
        self
    }

    /// The request is complete: no longer its thread's outstanding I/O.
    /// Called before its status block, event or packet tells anyone, so a
    /// thread woken by them never sees it still pending.
    pub fn retire(&self) {
        let issuer = self.issuer.lock().take();
        drop(issuer);
    }

    /// Posts `packet` to `port` (and signals it): into the reserved slot
    /// if there is one for that port, else an ordinary post (the port was
    /// associated after the request started), which may fail; a lost
    /// packet is logged.
    pub fn post(&self, port: &ObjectRef, packet: CompletionPacket) {
        let ObjectBody::IoCompletion(c) = &port.body else { return };
        let slot = self.packet.lock().take();
        let pushed = match slot {
            Some(mut slot) if KArc::ptr_eq(&slot.target, port) => {
                slot.used = true;
                c.queue.lock().push_reserved(packet)
            }
            other => {
                drop(other);
                c.queue.lock().push(packet)
            }
        };
        if pushed.is_ok() {
            wait::set_signal(port, 1);
        } else {
            crate::warn!(
                "io: a completion packet (context {:#x}) was lost: no memory",
                packet.context
            );
        }
    }

    /// Queues `apc` to `thread`: into the reserved slot if there is one for
    /// that thread, else an ordinary queueing; a lost one is logged.
    pub fn queue_apc(&self, thread_object: &ObjectRef, apc: UserApc) {
        let slot = self.apc.lock().take();
        let queued = match slot {
            Some(mut slot) if KArc::ptr_eq(&slot.target, thread_object) => {
                slot.used = true;
                thread::queue_reserved_apc(thread_object, apc)
            }
            other => {
                drop(other);
                thread::queue_user_apc(thread_object, apc)
            }
        };
        if queued == Err(NtStatus::NO_MEMORY) {
            crate::warn!("io: a completion routine ({:#x}) was lost: no memory", apc.routine);
        }
    }
}

/// Where and how a request completes. Clones (a copy that pends while
/// the call tries to finish it at once) share one reservation.
#[derive(Clone)]
pub struct Request {
    pub process: KArc<Process>,
    pub iosb: u64,
    pub buffer: u64,
    pub length: u32,
    pub event: Option<ObjectRef>,
    /// The completion port and key of the file (FileCompletionInformation)
    /// and the request's ApcContext (the OVERLAPPED): a packet is posted
    /// when both are there.
    pub port: Option<(ObjectRef, u64)>,
    pub apc_context: u64,
    /// The completion routine, queued to the requesting thread.
    pub apc: Option<(ObjectRef, UserApc)>,
    /// The file object's signal (waits on the file handle).
    pub file_signal: KArc<Dispatcher>,
    /// Signaled when it completes (a synchronous handle's caller waits).
    pub done: Option<ObjectRef>,
    /// Its packet and APC (`Request::reserve`); None: not reserved (the
    /// completion's posts may then fail).
    pub reserved: Option<KArc<Reserved>>,
}

impl Request {
    /// Reserves what its completion will post (`Reserved`): a packet when
    /// the file has a port and the request an ApcContext, the completion
    /// routine's APC.
    pub fn reserve(mut self) -> Result<Self, NtStatus> {
        let port = self.port.as_ref().filter(|_| self.apc_context != 0).map(|(p, _)| p);
        // A request on a file with a completion port is the file's, not
        // its thread's (thread-agnostic I/O).
        let issuer = if self.port.is_none() { thread::IoPending::current() } else { None };
        let reserved = Reserved::new(port, self.apc.as_ref().map(|(t, _)| t))?.issued_by(issuer);
        self.reserved =
            Some(KArc::try_new(reserved).map_err(|_| NtStatus::INSUFFICIENT_RESOURCES)?);
        Ok(self)
    }
}

struct State {
    filter: u32,
    tree: bool,
    /// FILE_NOTIFY_INFORMATION records not yet delivered.
    buffered: Vec<u8>,
    /// Offset of the last record in `buffered` (its NextEntryOffset is set
    /// when another follows).
    last: Option<usize>,
    capacity: usize,
    overflow: bool,
    pending: Vec<Request>,
}

pub struct Watch {
    key: usize,
    dir: NodeId,
    state: IrqMutex<State>,
}

static WATCHES: IrqMutex<Vec<KArc<Watch>>> = IrqMutex::new(Vec::new());

/// The FILE_NOTIFY_CHANGE_* bits a change matches.
fn change_bits(c: &Change) -> u32 {
    match c.action {
        ramfs::ACTION_MODIFIED => CHANGE_LAST_WRITE | CHANGE_SIZE,
        _ if c.directory => CHANGE_DIR_NAME,
        _ => CHANGE_FILE_NAME,
    }
}

/// One change for one watch, ready to deliver.
pub struct Event {
    watch: KArc<Watch>,
    action: u32,
    name: Vec<u16>,
}

/// What the ramfs journal holds for the watches (called with the file
/// system held, by `file::with_fs`).
pub fn collect(fs: &mut RamFs) -> (Vec<Event>, Vec<KArc<Watch>>) {
    let mut events = Vec::new();
    let mut lost = Vec::new();
    if !fs.journal {
        return (events, lost);
    }
    let (changes, changes_lost) = fs.take_changes();
    let watches = WATCHES.lock();
    if changes_lost {
        for w in watches.iter() {
            let _ = lost.try_push(w.clone());
        }
    }
    for c in &changes {
        for w in watches.iter() {
            let (filter, tree) = {
                let s = w.state.lock();
                (s.filter, s.tree)
            };
            if filter & change_bits(c) == 0 {
                continue;
            }
            let Some(name) = fs.relative_name(w.dir, c.parent, &c.name, tree) else { continue };
            let event = Event { watch: w.clone(), action: c.action, name };
            if events.try_push(event).is_err() {
                let _ = lost.try_push(w.clone());
            }
        }
    }
    (events, lost)
}

/// Delivers what `collect` found (with the file system released).
pub fn deliver((events, lost): (Vec<Event>, Vec<KArc<Watch>>)) {
    for w in &lost {
        let mut s = w.state.lock();
        s.overflow = true;
        s.buffered.clear();
        s.last = None;
    }
    for e in &events {
        let mut s = e.watch.state.lock();
        append(&mut s, e.action, &e.name);
    }
    let mut touched: Vec<KArc<Watch>> = Vec::new();
    for w in events.iter().map(|e| &e.watch).chain(lost.iter()) {
        if !touched.iter().any(|t| KArc::ptr_eq(t, w)) {
            let _ = touched.try_push(w.clone());
        }
    }
    for w in touched {
        let ready = take_ready(&w);
        if let Some((request, status, data)) = ready {
            complete(request, status, &data);
        }
    }
}

/// Adds a FILE_NOTIFY_INFORMATION record (NextEntryOffset, Action,
/// FileNameLength, FileName; records 4-byte aligned).
fn append(s: &mut State, action: u32, name: &[u16]) {
    if s.overflow {
        return;
    }
    let start = s.buffered.len().next_multiple_of(4);
    let size = 12 + 2 * name.len();
    if start + size > s.capacity.max(1) {
        s.overflow = true;
        s.buffered.clear();
        s.last = None;
        return;
    }
    if s.buffered.try_reserve(start + size - s.buffered.len()).is_err() {
        s.overflow = true;
        s.buffered.clear();
        s.last = None;
        return;
    }
    s.buffered.resize(start, 0);
    if let Some(prev) = s.last {
        let next = (start - prev) as u32;
        s.buffered[prev..prev + 4].copy_from_slice(&next.to_le_bytes());
    }
    let _ = s.buffered.try_extend_from_slice(&0u32.to_le_bytes());
    let _ = s.buffered.try_extend_from_slice(&action.to_le_bytes());
    let _ = s.buffered.try_extend_from_slice(&((2 * name.len()) as u32).to_le_bytes());
    for c in name {
        let _ = s.buffered.try_extend_from_slice(&c.to_le_bytes());
    }
    s.last = Some(start);
}

/// The oldest waiting request and what it gets, if there is something.
fn take_ready(w: &Watch) -> Option<(Request, NtStatus, Vec<u8>)> {
    let mut s = w.state.lock();
    if s.pending.is_empty() || (!s.overflow && s.buffered.is_empty()) {
        return None;
    }
    let request = s.pending.remove(0);
    Some(drain(&mut s, request))
}

/// What request `r` completes with: the records, or STATUS_NOTIFY_ENUM_DIR
/// (no data) after an overflow or if they do not fit.
fn drain(s: &mut State, r: Request) -> (Request, NtStatus, Vec<u8>) {
    let data = core::mem::take(&mut s.buffered);
    s.last = None;
    let overflow = core::mem::take(&mut s.overflow);
    if overflow || data.len() > r.length as usize {
        (r, NOTIFY_ENUM_DIR, Vec::new())
    } else {
        (r, NtStatus::SUCCESS, data)
    }
}

/// Reports a finished request.
pub fn complete(r: Request, status: NtStatus, data: &[u8]) {
    let mut status = status;
    if !data.is_empty() && r.process.write(r.buffer, data).is_err() {
        status = NtStatus::ACCESS_VIOLATION;
    }
    complete_count(r, status, data.len() as u64);
}

/// Reports a finished request that moved `count` bytes (already copied).
pub fn complete_count(r: Request, status: NtStatus, count: u64) {
    // Success and warnings (STATUS_BUFFER_OVERFLOW: part of a message)
    // report the bytes.
    let information = if status.0 < 0xc000_0000 { count } else { 0 };
    if let Some(reserved) = &r.reserved {
        reserved.retire();
    }
    if r.iosb != 0 {
        let _ = r.process.write_iosb(r.iosb, status, information);
    }
    if let Some(e) = &r.event {
        wait::set_signal(e, 1);
    }
    wait::signal_dispatcher(&r.file_signal, 1);
    let none = Reserved::none();
    let reserved = r.reserved.as_deref().unwrap_or(&none);
    if let (Some((port, key)), true) = (&r.port, r.apc_context != 0) {
        let packet = CompletionPacket::new(*key, r.apc_context, status.0, information);
        reserved.post(port, packet);
    }
    // The completion routine runs whatever the status (ReadFileEx's gets
    // ERROR_BROKEN_PIPE, ERROR_OPERATION_ABORTED...): a request that
    // failed in its call never gets here (the caller reports it).
    if let Some((thread, apc)) = &r.apc {
        reserved.queue_apc(thread, UserApc { routine: apc.routine, args: apc.args });
    }
    if let Some(done) = &r.done {
        wait::set_signal(done, 1);
    }
}

/// NtNotifyChangeDirectoryFile: watches directory `dir` through open
/// `key` with `filter` (and below it with `tree`). Completes `request` at
/// once with changes already kept, else leaves it waiting (STATUS_PENDING).
/// Returns the status and, if it completed, nothing more to do.
pub fn request(key: usize, dir: NodeId, filter: u32, tree: bool, r: Request) -> NtStatus {
    let watch = {
        let mut watches = WATCHES.lock();
        match watches.iter().find(|w| w.key == key) {
            Some(w) => w.clone(),
            None => {
                let state = State {
                    filter,
                    tree,
                    buffered: Vec::new(),
                    last: None,
                    capacity: r.length as usize,
                    overflow: false,
                    pending: Vec::new(),
                };
                let w = match KArc::try_new(Watch { key, dir, state: IrqMutex::new(state) }) {
                    Ok(w) => w,
                    Err(_) => return NtStatus::NO_MEMORY,
                };
                if watches.try_push(w.clone()).is_err() {
                    return NtStatus::NO_MEMORY;
                }
                w
            }
        }
    };
    // From now on the file system records changes.
    super::file::with_fs(|fs| fs.journal = true);
    let ready = {
        let mut s = watch.state.lock();
        s.filter = filter;
        s.tree = tree;
        s.capacity = r.length as usize;
        if s.overflow || !s.buffered.is_empty() {
            Some(drain(&mut s, r))
        } else {
            if s.pending.try_push(r).is_err() {
                return NtStatus::NO_MEMORY;
            }
            None
        }
    };
    match ready {
        Some((r, status, data)) => {
            complete(r, status, &data);
            status
        }
        None => NtStatus::PENDING,
    }
}

/// CancelIoEx on open `key`: its waiting requests of `process` (only the
/// one whose IO_STATUS_BLOCK is `iosb`, if given) end with
/// STATUS_CANCELLED. Returns how many.
pub fn cancel(key: usize, process: &Process, iosb: Option<u64>) -> usize {
    let watch = WATCHES.lock().iter().find(|w| w.key == key).cloned();
    let Some(watch) = watch else { return 0 };
    let mut cancelled = Vec::new();
    {
        let mut s = watch.state.lock();
        let mut i = 0;
        while i < s.pending.len() {
            let p = &s.pending[i];
            if p.process.id == process.id && iosb.is_none_or(|b| b == p.iosb) {
                let r = s.pending.remove(i);
                if cancelled.try_push(r).is_err() {
                    break;
                }
            } else {
                i += 1;
            }
        }
    }
    let n = cancelled.len();
    for r in cancelled {
        complete(r, CANCELLED, &[]);
    }
    n
}

/// The open `key` closed: its watch ends, waiting requests complete with
/// STATUS_NOTIFY_CLEANUP.
pub fn cleanup(key: usize) {
    let watch = {
        let mut watches = WATCHES.lock();
        let at = watches.iter().position(|w| w.key == key);
        at.map(|i| watches.remove(i))
    };
    let Some(watch) = watch else { return };
    let pending = core::mem::take(&mut watch.state.lock().pending);
    for r in pending {
        complete(r, NOTIFY_CLEANUP, &[]);
    }
}

/// Whether the open `key` has a watch (its close must call `cleanup`).
pub fn watched(key: usize) -> bool {
    WATCHES.lock().iter().any(|w| w.key == key)
}
