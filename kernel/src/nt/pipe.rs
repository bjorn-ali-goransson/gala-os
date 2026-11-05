//! Named pipes (`\Device\NamedPipe`, also `\??\PIPE`): instances created
//! with NtCreateNamedPipeFile (server ends) and opened by clients with
//! NtCreateFile/NtOpenFile. kernel32's CreatePipe is a uniquely named
//! one-instance pipe; CreateNamedPipeW creates the instances of a server.
//!
//! The instances of a name share its MaximumInstances (set by the first);
//! FILE_CREATE (FILE_FLAG_FIRST_PIPE_INSTANCE) fails with
//! STATUS_ACCESS_DENIED once one exists, and an instance beyond the
//! maximum with STATUS_INSTANCE_NOT_AVAILABLE. A new instance listens: a
//! client open takes the oldest listening instance of its name
//! (STATUS_OBJECT_NAME_NOT_FOUND without any, STATUS_PIPE_NOT_AVAILABLE
//! if none listens). FSCTL_PIPE_LISTEN (ConnectNamedPipe) waits for a
//! client (STATUS_PIPE_CONNECTED if one is there, STATUS_PIPE_CLOSING if
//! it already left); FSCTL_PIPE_DISCONNECT ends the connection: the
//! queued bytes go, the client's end is left disconnected
//! (STATUS_PIPE_DISCONNECTED) and the instance can listen again.
//!
//! A connection holds a byte queue per direction (bounded by the
//! creator's inbound and outbound quotas) and, for a message-type pipe,
//! the length of each message in it. Reads return what is there (at
//! least one byte, waiting if there is none); in message read mode at
//! most one message, and STATUS_BUFFER_OVERFLOW if it does not fit (the
//! rest stays for the next read). Writes complete fully, waiting while
//! the pipe is full (a message waits until it fits whole, or the queue is
//! empty). When the other end is closed a reader drains what is left and
//! gets STATUS_PIPE_BROKEN; a writer fails with STATUS_PIPE_CLOSING.
//!
//! An end opened without FILE_SYNCHRONOUS_IO_* is asynchronous: a listen
//! or read that would wait is left pending (STATUS_PENDING) and completes
//! later through its `notify::Request` (status block, event, completion
//! port, APC) when a client connects or data arrives; CancelIoEx and
//! closing the end cancel it. Writes on such an end still wait (a
//! documented simplification: they complete once the bytes are queued).
//!
//! **Invariant: a write completes the reads already waiting for it**, on
//! synchronous ends too: a synchronous read that has to wait is a pending
//! request without a `Request` (its thread waits for its result), so the
//! write that satisfies it hands it the bytes at once, as Windows
//! completes a waiting read's IRP. DisconnectNamedPipe then discards only
//! bytes nobody was reading (Crashpad's server writes its reply and
//! disconnects at once). FSCTL_PIPE_TRANSCEIVE (TransactNamedPipe) puts
//! its read in place before its write, so the answer cannot be lost
//! either. An asynchronous end's write that does not fit pends with the
//! rest of its bytes (`Op::Write`), moved on as readers make room, instead
//! of blocking its thread. A waiting thread looks for its result before each wait, never
//! after only: the result can come while the lock is not held (the reply
//! to a transceive's write), and its wake-up is then already spent.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::notify::{self, Request};
use super::object::ObjectRef;
use super::platform::{self, ThreadId};
use super::process::Process;
use super::status::NtStatus;
use super::wait::Dispatcher;
use crate::mm::heap::{KArc, TryVec};
use crate::sync::IrqMutex;

/// Quota used when the creator asks for none (CreatePipe's nSize 0).
const DEFAULT_QUOTA: usize = 4096;

/// Numbers pipes for the event stream.
static NEXT_PIPE_ID: AtomicU64 = AtomicU64::new(1);

/// An `ipc` event about pipe `id` (`fields` after `"pipe":id`).
fn event(name: &str, id: u64, fields: core::fmt::Arguments) {
    crate::events::emit(
        crate::events::Category::Ipc,
        name,
        format_args!(r#""pipe":{id},{fields}"#),
    );
}

/// FILE_PIPE_PEEK_BUFFER.NamedPipeState values.
pub const FILE_PIPE_DISCONNECTED_STATE: u32 = 1;
pub const FILE_PIPE_LISTENING_STATE: u32 = 2;
pub const FILE_PIPE_CONNECTED_STATE: u32 = 3;
pub const FILE_PIPE_CLOSING_STATE: u32 = 4;

const PIPE_DISCONNECTED: NtStatus = NtStatus::PIPE_DISCONNECTED;
const PIPE_LISTENING: NtStatus = NtStatus::PIPE_LISTENING;
const PIPE_CONNECTED: NtStatus = NtStatus::PIPE_CONNECTED;

pub struct Pipe {
    state: IrqMutex<State>,
    /// The connected client's security context (an impersonation token,
    /// captured when it connected): what FSCTL_PIPE_IMPERSONATE gives the
    /// server.
    client_token: IrqMutex<Option<ObjectRef>>,
    /// The name it was created with (empty for an anonymous pipe), as
    /// FileNameInformation reports it.
    name: Vec<u16>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Conn {
    /// Waiting for a client.
    Listening,
    Connected,
    /// The client closed its end; DisconnectNamedPipe is next.
    Closing,
    /// After DisconnectNamedPipe, until the next listen.
    Disconnected,
}

struct State {
    /// The pipe's number in the event stream (`ipc` events).
    id: u64,
    /// Bytes ever put into each inbox and taken from it: the offsets of
    /// `pipe write` and `pipe read` events, so a read is matched to the
    /// writes it took (the data for end e are written by the other end).
    put: [u64; 2],
    taken: [u64; 2],
    /// Bytes waiting to be read by end 0 (server) and end 1 (client).
    inbox: [VecDeque<u8>; 2],
    /// The lengths of the messages in each inbox (message-type pipes).
    messages: [VecDeque<usize>; 2],
    /// Writes are messages (PIPE_TYPE_MESSAGE).
    message_type: bool,
    /// The most each inbox holds before writers wait.
    quota: [usize; 2],
    conn: Conn,
    server_open: bool,
    /// Numbers the connections: a client end belongs to one.
    generation: u64,
    /// Threads waiting for data, space, a client or a close.
    waiters: Vec<ThreadId>,
    /// Requests of asynchronous ends waiting for the same.
    pending: Vec<Pending>,
}

#[derive(Clone, Copy)]
enum Op {
    Listen,
    Read {
        max: usize,
        message: bool,
    },
    /// An asynchronous write of `total` bytes that did not fit: its
    /// remaining bytes wait in `Pending::out` and go into the pipe as
    /// readers make room (oldest first), as Windows pends an overlapped
    /// write to a full pipe instead of blocking its thread.
    Write {
        total: usize,
    },
}

struct Pending {
    end: usize,
    generation: u64,
    op: Op,
    /// None: a synchronous call's, its thread waiting for `result`
    /// (found by `token`), not reported by `report`.
    request: Option<Request>,
    token: u64,
    /// A pending write's bytes not yet in the pipe.
    out: VecDeque<u8>,
    /// Set when it has finished: reported (and removed) by `report`
    /// after the pipe's lock is released. Finishing a request needs no
    /// allocation, so none is ever lost (a list of finished requests that
    /// could not grow once dropped one, and its waiter waited for ever).
    result: Option<(NtStatus, Vec<u8>)>,
}

/// One end of a pipe: the body of its file object.
pub struct PipeEnd {
    pipe: KArc<Pipe>,
    /// 0: the server end (NtCreateNamedPipeFile), 1: the client end.
    end: usize,
    /// The connection a client end belongs to.
    generation: u64,
    /// Opened for synchronous I/O (FILE_SYNCHRONOUS_IO_*): requests wait
    /// in the call instead of pending.
    pub synchronous: bool,
    /// FILE_PIPE_MESSAGE_MODE reads (FilePipeInformation).
    message_reads: AtomicBool,
    /// The completion port and key (FileCompletionInformation).
    pub completion: IrqMutex<Option<(ObjectRef, u64)>>,
    /// FILE_SKIP_* notification modes (FileIoCompletionNotificationInformation).
    pub modes: core::sync::atomic::AtomicU32,
    /// Signaled when an I/O on it completes (waiting on the handle).
    pub dispatcher: KArc<Dispatcher>,
}

/// Reports the finished requests (oldest first), each outside the
/// pipe's lock (completing one writes user memory and signals objects).
fn report(pipe: &Pipe) {
    loop {
        let finished = {
            let mut s = pipe.state.lock();
            match s.pending.iter().position(|p| p.result.is_some() && p.request.is_some()) {
                Some(i) => s.pending.remove(i),
                None => return,
            }
        };
        match (finished.result, finished.request, finished.op) {
            // A write reports its bytes (none copied back to the caller).
            (Some((status, _)), Some(request), Op::Write { total }) => {
                notify::complete_count(request, status, total as u64)
            }
            (Some((status, data)), Some(request), _) => notify::complete(request, status, &data),
            _ => {}
        }
    }
}

/// Marks pending request `i` cancelled (reported by `report`).
fn cancel_pending(s: &mut State, i: usize) {
    s.pending[i].result = Some((notify::CANCELLED, Vec::new()));
}

impl Drop for PipeEnd {
    fn drop(&mut self) {
        if self.end == 0 {
            // No client can connect to a closed instance any more.
            INSTANCES.lock().retain(|i| !KArc::ptr_eq(&i.pipe, &self.pipe));
        }
        {
            let mut s = self.pipe.state.lock();
            event("pipe close", s.id, format_args!(r#""end":{}"#, self.end));
            if self.end == 0 {
                s.server_open = false;
            } else if self.generation == s.generation && s.conn == Conn::Connected {
                s.conn = Conn::Closing;
            }
            // Requests of this end are cancelled; the other side's are
            // looked at again (broken pipe).
            let end = self.end;
            let generation = self.generation;
            for i in 0..s.pending.len() {
                let p = &s.pending[i];
                if p.result.is_none() && p.end == end && (end == 0 || p.generation == generation) {
                    cancel_pending(&mut s, i);
                }
            }
            try_complete(&mut s);
            wake_all(&mut s);
        }
        report(&self.pipe);
    }
}

fn wake_all(s: &mut State) {
    for t in s.waiters.drain(..) {
        platform::wake(t);
    }
}

/// Creates `\Device\NamedPipe` and its DOS name `\??\PIPE`.
pub fn init() -> Result<(), NtStatus> {
    use super::object::{KObject, ObjectBody};
    let device = KObject::with_permanence(ObjectBody::NamedPipeDevice, true)
        .map_err(|_| NtStatus::NO_MEMORY)?;
    let name = wide("\\Device\\NamedPipe")?;
    super::namespace::create(None, &name, device, true, false).map(drop)?;
    let link = super::namespace::new_symbolic_link(&name)?;
    super::namespace::create(None, &wide("\\??\\PIPE")?, link, false, false).map(drop)
}

fn wide(text: &str) -> Result<Vec<u16>, NtStatus> {
    let mut v = Vec::new();
    for c in text.encode_utf16() {
        v.try_push(c).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(v)
}

/// A server instance, under its (upper-cased) name.
struct Instance {
    name: Vec<u16>,
    /// MaximumInstances of the name (u32::MAX: unlimited).
    max: u32,
    pipe: KArc<Pipe>,
}

/// Every server instance, oldest first.
static INSTANCES: IrqMutex<Vec<Instance>> = IrqMutex::new(Vec::new());

fn upper(name: &[u16]) -> Result<Vec<u16>, NtStatus> {
    let mut v = Vec::new();
    v.try_reserve_exact(name.len()).map_err(|_| NtStatus::NO_MEMORY)?;
    v.extend(
        name.iter()
            .map(|&c| if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }),
    );
    Ok(v)
}

/// What NtCreateNamedPipeFile asks for.
pub struct ServerOptions {
    /// `inbound` and `outbound` quotas (bytes towards the server and the
    /// client).
    pub inbound: u32,
    pub outbound: u32,
    /// MaximumInstances (u32::MAX: unlimited).
    pub max_instances: u32,
    /// FILE_PIPE_MESSAGE_TYPE / FILE_PIPE_MESSAGE_MODE.
    pub message_type: bool,
    pub message_reads: bool,
    /// CreateDisposition FILE_CREATE: the first instance only.
    pub first: bool,
    pub synchronous: bool,
}

/// Creates an instance of pipe `name` and returns its server end.
pub fn create_server(name: &[u16], o: &ServerOptions) -> Result<PipeEnd, NtStatus> {
    if o.max_instances == 0 || (o.message_reads && !o.message_type) {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    let key = upper(name)?;
    // Allocated before the instance is listed: a failure after it left an
    // instance without a server end.
    let dispatcher = new_dispatcher()?;
    let mut instances = INSTANCES.lock();
    let mut count = 0u32;
    let mut max = o.max_instances;
    for i in instances.iter().filter(|i| i.name == key) {
        count += 1;
        max = i.max;
    }
    if count > 0 && o.first {
        return Err(NtStatus::ACCESS_DENIED);
    }
    if count >= max {
        return Err(NtStatus::INSTANCE_NOT_AVAILABLE);
    }
    let quota = |q: u32| if q == 0 { DEFAULT_QUOTA } else { (q as usize).max(DEFAULT_QUOTA) };
    let mut given = Vec::new();
    given.try_reserve_exact(name.len()).map_err(|_| NtStatus::NO_MEMORY)?;
    given.try_extend_from_slice(name).map_err(|_| NtStatus::NO_MEMORY)?;
    let id = NEXT_PIPE_ID.fetch_add(1, Ordering::Relaxed);
    event(
        "pipe create",
        id,
        format_args!(
            r#""name":{},"message":{},"instance":{}"#,
            crate::events::Utf16(name),
            o.message_type,
            count + 1
        ),
    );
    let pipe = KArc::try_new(Pipe {
        client_token: IrqMutex::new(None),
        name: given,
        state: IrqMutex::new(State {
            id,
            put: [0; 2],
            taken: [0; 2],
            inbox: [VecDeque::new(), VecDeque::new()],
            messages: [VecDeque::new(), VecDeque::new()],
            message_type: o.message_type,
            quota: [quota(o.inbound), quota(o.outbound)],
            conn: Conn::Listening,
            server_open: true,
            generation: 0,
            waiters: Vec::new(),
            pending: Vec::new(),
        }),
    })
    .map_err(|_| NtStatus::NO_MEMORY)?;
    instances
        .try_push(Instance { name: key, max, pipe: pipe.clone() })
        .map_err(|_| NtStatus::NO_MEMORY)?;
    drop(instances);
    Ok(PipeEnd::new(pipe, 0, 0, o.synchronous, o.message_reads, dispatcher))
}

/// The signal of a new end's file object.
fn new_dispatcher() -> Result<KArc<Dispatcher>, NtStatus> {
    KArc::try_new(Dispatcher::new(1)).map_err(|_| NtStatus::NO_MEMORY)
}

/// Opens a client end of pipe `name` (its oldest listening instance).
pub fn open_client(
    name: &[u16],
    synchronous: bool,
    client_token: Option<ObjectRef>,
) -> Result<PipeEnd, NtStatus> {
    let key = upper(name)?;
    // Allocated before connecting: a failure after it left the server
    // connected to a client end that did not exist.
    let dispatcher = new_dispatcher()?;
    let instances = INSTANCES.lock();
    let mut found = false;
    for i in instances.iter().filter(|i| i.name == key) {
        found = true;
        let mut s = i.pipe.state.lock();
        if s.conn != Conn::Listening || !s.server_open {
            continue;
        }
        s.conn = Conn::Connected;
        let generation = s.generation;
        event("pipe connect", s.id, format_args!(r#""generation":{generation}"#));
        try_complete(&mut s);
        wake_all(&mut s);
        drop(s);
        let pipe = i.pipe.clone();
        drop(instances);
        report(&pipe);
        *pipe.client_token.lock() = client_token;
        // A client starts in byte read mode.
        return Ok(PipeEnd::new(pipe, 1, generation, synchronous, false, dispatcher));
    }
    Err(if found { NtStatus::PIPE_NOT_AVAILABLE } else { NtStatus::OBJECT_NAME_NOT_FOUND })
}

/// What a request (of end `end`, connection `generation`) gets now, or
/// None if it has to wait.
fn attempt(
    s: &mut State,
    end: usize,
    generation: u64,
    op: &Op,
) -> Option<Result<(Vec<u8>, NtStatus), NtStatus>> {
    let peer = 1 - end;
    match *op {
        Op::Listen => match s.conn {
            Conn::Listening if s.server_open => None,
            Conn::Listening => Some(Err(NtStatus::PIPE_BROKEN)),
            Conn::Connected => Some(Ok((Vec::new(), NtStatus::SUCCESS))),
            Conn::Closing => Some(Err(NtStatus::PIPE_BROKEN)),
            Conn::Disconnected => Some(Err(PIPE_DISCONNECTED)),
        },
        Op::Read { max, message } => {
            if let Err(e) = s.check_connected(end, generation) {
                return Some(Err(e));
            }
            if s.inbox[end].is_empty() && max > 0 {
                let peer_open = if peer == 0 { s.server_open } else { s.conn == Conn::Connected };
                return if peer_open { None } else { Some(Err(NtStatus::PIPE_BROKEN)) };
            }
            Some(s.take(end, max, message))
        }
        // Writes move in `complete_pass`, never here.
        Op::Write { .. } => None,
    }
}

/// Finishes the pending requests that can finish now, oldest first;
/// returns how many (`report` reports them).
fn try_complete(s: &mut State) -> usize {
    let mut finished = 0;
    // Writes free reads and reads make room for writes: again until a
    // pass changes nothing.
    loop {
        let (done, moved) = complete_pass(s);
        finished += done;
        if done == 0 && !moved {
            return finished;
        }
    }
}

/// One pass of `try_complete`: (requests finished, whether any pending
/// write's bytes went into the pipe).
fn complete_pass(s: &mut State) -> (usize, bool) {
    let mut finished = 0;
    let mut moved = false;
    for i in 0..s.pending.len() {
        if s.pending[i].result.is_some() {
            continue;
        }
        let (end, generation) = (s.pending[i].end, s.pending[i].generation);
        if let Op::Write { .. } = s.pending[i].op {
            // Only the oldest unfinished write of an end moves: bytes keep
            // their order.
            if s.pending[..i]
                .iter()
                .any(|p| p.end == end && p.result.is_none() && matches!(p.op, Op::Write { .. }))
            {
                continue;
            }
            let peer = 1 - end;
            let peer_open = if peer == 0 { s.server_open } else { s.conn == Conn::Connected };
            if let Err(e) = s.check_connected(end, generation) {
                s.pending[i].result = Some((e, Vec::new()));
                finished += 1;
                continue;
            }
            if !peer_open {
                s.pending[i].result = Some((NtStatus::PIPE_CLOSING, Vec::new()));
                finished += 1;
                continue;
            }
            let out = core::mem::take(&mut s.pending[i].out);
            let (left, n) = put(s, end, out);
            moved |= n > 0;
            s.pending[i].out = left;
            if s.pending[i].out.is_empty() {
                s.pending[i].result = Some((NtStatus::SUCCESS, Vec::new()));
                finished += 1;
            }
            continue;
        }
        // Take the op out to borrow the state mutably.
        let op = core::mem::replace(&mut s.pending[i].op, Op::Listen);
        match attempt(s, end, generation, &op) {
            None => s.pending[i].op = op,
            Some(result) => {
                s.pending[i].result = Some(match result {
                    Ok((data, status)) => (status, data),
                    Err(e) => (e, Vec::new()),
                });
                finished += 1;
            }
        }
    }
    (finished, moved)
}

/// Puts what fits of `data` (written by end `writer`) into the other end's
/// inbox: a message whole (when it fits, or alone), bytes as far as the
/// quota allows. Returns what is left and how many bytes went in.
fn put(s: &mut State, writer: usize, mut data: VecDeque<u8>) -> (VecDeque<u8>, usize) {
    let peer = 1 - writer;
    let space = s.quota[peer].saturating_sub(s.inbox[peer].len());
    let n = if s.message_type {
        if space >= data.len() || s.inbox[peer].is_empty() { data.len() } else { 0 }
    } else {
        space.min(data.len())
    };
    if n == 0 {
        return (data, 0);
    }
    if s.inbox[peer].try_reserve(n).is_err()
        || (s.message_type && s.messages[peer].try_reserve(1).is_err())
    {
        // Out of memory: the bytes wait for the next pass.
        return (data, 0);
    }
    if s.message_type {
        s.messages[peer].push_back(n);
    }
    s.inbox[peer].extend(data.drain(..n));
    event(
        "pipe write",
        s.id,
        format_args!(
            r#""end":{writer},"to":{peer},"off":{},"bytes":{n},"queued":{},"pended":true"#,
            s.put[peer],
            s.inbox[peer].len()
        ),
    );
    s.put[peer] += n as u64;
    wake_all(s); // readers waiting for data
    (data, n)
}

impl State {
    /// Whether end `end` of connection `generation` may read or write.
    fn check_connected(&self, end: usize, generation: u64) -> Result<(), NtStatus> {
        if end == 1 {
            if generation != self.generation {
                return Err(PIPE_DISCONNECTED);
            }
            return Ok(());
        }
        match self.conn {
            Conn::Listening => Err(PIPE_LISTENING),
            Conn::Disconnected => Err(PIPE_DISCONNECTED),
            Conn::Connected | Conn::Closing => Ok(()),
        }
    }

    /// Takes up to `max` bytes for end `end`: in `message` read mode of a
    /// message-type pipe at most one message (STATUS_BUFFER_OVERFLOW if
    /// it does not fit); otherwise across messages.
    fn take(
        &mut self,
        end: usize,
        max: usize,
        message: bool,
    ) -> Result<(Vec<u8>, NtStatus), NtStatus> {
        let available = self.inbox[end].len();
        let (n, status) = match self.messages[end].front().copied() {
            Some(len) if message && self.message_type => {
                if len > max {
                    (max, NtStatus::BUFFER_OVERFLOW)
                } else {
                    (len, NtStatus::SUCCESS)
                }
            }
            _ => (max.min(available), NtStatus::SUCCESS),
        };
        let mut out = Vec::new();
        out.try_reserve_exact(n).map_err(|_| NtStatus::NO_MEMORY)?;
        out.extend(self.inbox[end].drain(..n));
        // The thread may be the writer completing a waiting read: `by`
        // says who, `end` whose read it is.
        event(
            "pipe read",
            self.id,
            format_args!(
                r#""end":{end},"off":{},"bytes":{n},"left":{}"#,
                self.taken[end],
                self.inbox[end].len()
            ),
        );
        self.taken[end] += n as u64;
        // Consume the message lengths the bytes covered.
        let mut left = n;
        while left > 0 {
            let Some(front) = self.messages[end].front_mut() else { break };
            if *front > left {
                *front -= left;
                left = 0;
            } else {
                left -= *front;
                self.messages[end].pop_front();
            }
        }
        if message && self.message_type && status == NtStatus::SUCCESS {
            // A zero-length message is consumed by a read too.
            if n == 0 && self.messages[end].front() == Some(&0) {
                self.messages[end].pop_front();
            }
        }
        wake_all(self); // writers waiting for space
        Ok((out, status))
    }
}

/// Numbers synchronous requests (0: none).
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

fn done(result: Result<(Vec<u8>, NtStatus), NtStatus>) -> Outcome {
    match result {
        Ok((data, status)) => Outcome::Done(status, data),
        Err(e) => Outcome::Done(e, Vec::new()),
    }
}

/// How a read or listen of an asynchronous end ends: done now, or pending.
pub enum Outcome {
    Done(NtStatus, Vec<u8>),
    Pending,
}

impl PipeEnd {
    /// The pipe's name (empty for an anonymous pipe).
    pub fn name(&self) -> &[u16] {
        &self.pipe.name
    }
    fn new(
        pipe: KArc<Pipe>,
        end: usize,
        generation: u64,
        synchronous: bool,
        message_reads: bool,
        dispatcher: KArc<Dispatcher>,
    ) -> PipeEnd {
        PipeEnd {
            pipe,
            end,
            generation,
            synchronous,
            message_reads: AtomicBool::new(message_reads),
            completion: IrqMutex::new(None),
            modes: core::sync::atomic::AtomicU32::new(0),
            dispatcher,
        }
    }

    fn peer(&self) -> usize {
        1 - self.end
    }

    /// The connected client's token (see Pipe::client_token).
    pub fn client_token(&self) -> Option<ObjectRef> {
        self.pipe.client_token.lock().clone()
    }

    pub fn is_server(&self) -> bool {
        self.end == 0
    }

    /// Sleeps until the other side changes something (or termination).
    fn wait(&self, mut s: crate::sync::IrqGuard<'_, State>) -> Result<(), NtStatus> {
        let me = platform::current();
        if !s.waiters.contains(&me) {
            s.waiters.try_push(me).map_err(|_| NtStatus::NO_MEMORY)?;
        }
        platform::prepare_wait(None);
        super::thread::wake_if_terminating();
        drop(s);
        platform::wait();
        if super::thread::is_terminating() {
            return Err(NtStatus::THREAD_IS_TERMINATING);
        }
        Ok(())
    }

    /// Runs `op` now, waits for it (a synchronous end) or leaves `request`
    /// pending (an asynchronous one).
    fn run(&self, op: Op, request: Option<Request>) -> Result<Outcome, NtStatus> {
        let mut s = self.pipe.state.lock();
        if let Some(result) = attempt(&mut s, self.end, self.generation, &op) {
            // The room a read made lets pending writes go on.
            let moved = try_complete(&mut s);
            drop(s);
            if moved > 0 {
                report(&self.pipe);
            }
            return Ok(done(result));
        }
        let token = self.queue(&mut s, op, request)?;
        if token == 0 {
            return Ok(Outcome::Pending);
        }
        self.await_result(s, op, token)
    }

    /// Queues `op` as a pending request: `request`'s on an asynchronous
    /// end (returns 0), else a synchronous one whose token it returns.
    fn queue(&self, s: &mut State, op: Op, request: Option<Request>) -> Result<u64, NtStatus> {
        let asynchronous = !self.synchronous && request.is_some();
        let token = if asynchronous { 0 } else { NEXT_TOKEN.fetch_add(1, Ordering::Relaxed) };
        let p = Pending {
            end: self.end,
            generation: self.generation,
            op,
            request: if asynchronous { request } else { None },
            token,
            out: VecDeque::new(),
            result: None,
        };
        s.pending.try_push(p).map_err(|_| NtStatus::INSUFFICIENT_RESOURCES)?;
        Ok(token)
    }

    /// Waits for synchronous request `token` to be finished (by a write,
    /// a connect, a disconnect or a close) and takes its result.
    fn await_result<'a>(
        &'a self,
        mut s: crate::sync::IrqGuard<'a, State>,
        op: Op,
        token: u64,
    ) -> Result<Outcome, NtStatus> {
        // The result is looked for before each wait: it may have come
        // while the lock was not held (TransactNamedPipe's read, completed
        // by the reply to its write before this end took the lock again),
        // and that wake-up has passed.
        loop {
            match s.pending.iter().position(|p| p.token == token && p.request.is_none()) {
                Some(i) if s.pending[i].result.is_some() => {
                    let p = s.pending.remove(i);
                    let (status, data) = p.result.unwrap_or((NtStatus::UNSUCCESSFUL, Vec::new()));
                    return Ok(Outcome::Done(status, data));
                }
                Some(_) => {}
                // Not there (a kernel handover does not carry it): ask again.
                None => {
                    if let Some(result) = attempt(&mut s, self.end, self.generation, &op) {
                        let moved = try_complete(&mut s);
                        drop(s);
                        if moved > 0 {
                            report(&self.pipe);
                        }
                        return Ok(done(result));
                    }
                    let p = Pending {
                        end: self.end,
                        generation: self.generation,
                        op,
                        request: None,
                        token,
                        out: VecDeque::new(),
                        result: None,
                    };
                    s.pending.try_push(p).map_err(|_| NtStatus::INSUFFICIENT_RESOURCES)?;
                }
            }
            if let Err(e) = self.wait(s) {
                let mut s = self.pipe.state.lock();
                s.pending.retain(|p| p.token != token || p.request.is_some());
                return Err(e);
            }
            s = self.pipe.state.lock();
        }
    }

    /// FSCTL_PIPE_TRANSCEIVE (TransactNamedPipe): writes `data` as one
    /// message and reads one, the read in place before the write so the
    /// answer cannot be missed. A message-type pipe in message read mode
    /// only (STATUS_INVALID_PIPE_STATE); unread data in the pipe:
    /// STATUS_PIPE_BUSY.
    pub fn transceive(
        &self,
        data: &[u8],
        max: usize,
        request: Option<Request>,
    ) -> Result<Outcome, NtStatus> {
        let op = Op::Read { max, message: true };
        let token = {
            let mut s = self.pipe.state.lock();
            if !s.message_type || !self.message_reads.load(Ordering::Acquire) {
                return Err(NtStatus::INVALID_PIPE_STATE);
            }
            s.check_connected(self.end, self.generation)?;
            if !s.inbox[self.end].is_empty() {
                return Err(NtStatus::PIPE_BUSY);
            }
            self.queue(&mut s, op, request)?
        };
        if let Err(e) = self.write(data) {
            // The read goes with the write that failed.
            let mut s = self.pipe.state.lock();
            if token != 0 {
                s.pending.retain(|p| p.token != token || p.request.is_some());
            } else if let Some(i) = s
                .pending
                .iter()
                .rposition(|p| p.request.is_some() && p.end == self.end && p.result.is_none())
            {
                cancel_pending(&mut s, i);
                drop(s);
                report(&self.pipe);
            }
            return Err(e);
        }
        if token == 0 {
            return Ok(Outcome::Pending);
        }
        let s = self.pipe.state.lock();
        self.await_result(s, op, token)
    }

    /// Reads up to `max` bytes (see the module comment).
    pub fn read(&self, max: usize, request: Option<Request>) -> Result<Outcome, NtStatus> {
        let message = self.message_reads.load(Ordering::Acquire);
        self.run(Op::Read { max, message }, request)
    }

    /// FSCTL_PIPE_LISTEN on a server end.
    pub fn listen(&self, request: Option<Request>) -> Result<Outcome, NtStatus> {
        if self.end != 0 {
            return Err(NtStatus::ILLEGAL_FUNCTION);
        }
        {
            let mut s = self.pipe.state.lock();
            match s.conn {
                Conn::Connected => return Err(PIPE_CONNECTED),
                Conn::Closing => return Err(NtStatus::PIPE_CLOSING),
                Conn::Disconnected => s.conn = Conn::Listening,
                Conn::Listening => {}
            }
        }
        self.run(Op::Listen, request)
    }

    /// FSCTL_PIPE_DISCONNECT on a server end: the client's end is left
    /// disconnected, queued bytes and its requests go.
    pub fn disconnect(&self) -> Result<(), NtStatus> {
        if self.end != 0 {
            return Err(NtStatus::ILLEGAL_FUNCTION);
        }
        {
            let mut s = self.pipe.state.lock();
            if s.conn == Conn::Disconnected {
                return Err(PIPE_DISCONNECTED);
            }
            s.conn = Conn::Disconnected;
            let dropped = s.inbox[0].len() + s.inbox[1].len();
            event("pipe disconnect", s.id, format_args!(r#""unread":{dropped}"#));
            s.generation += 1;
            for e in 0..2 {
                s.inbox[e].clear();
                s.messages[e].clear();
            }
            try_complete(&mut s);
            wake_all(&mut s);
        }
        report(&self.pipe);
        Ok(())
    }

    /// FlushFileBuffers on a pipe end: waits until the other end has read
    /// everything this end wrote (STATUS_PIPE_BROKEN once it is closed).
    pub fn flush(&self) -> Result<(), NtStatus> {
        let peer = self.peer();
        loop {
            let s = self.pipe.state.lock();
            s.check_connected(self.end, self.generation)?;
            let writing = s.pending.iter().any(|p| {
                p.end == self.end && p.result.is_none() && matches!(p.op, Op::Write { .. })
            });
            if s.inbox[peer].is_empty() && !writing {
                return Ok(());
            }
            let peer_open = if peer == 0 { s.server_open } else { s.conn == Conn::Connected };
            if !peer_open {
                return Err(NtStatus::PIPE_BROKEN);
            }
            self.wait(s)?;
        }
    }

    /// Writes all of `data`, waiting while the pipe is full.
    pub fn write(&self, mut data: &[u8]) -> Result<(), NtStatus> {
        let peer = self.peer();
        let mut first = true;
        loop {
            let mut finished = 0;
            let mut s = self.pipe.state.lock();
            s.check_connected(self.end, self.generation)?;
            let peer_open = if peer == 0 { s.server_open } else { s.conn == Conn::Connected };
            if !peer_open {
                return Err(NtStatus::PIPE_CLOSING);
            }
            let space = s.quota[peer].saturating_sub(s.inbox[peer].len());
            let n = if s.message_type {
                // A message goes in whole (when it fits, or alone).
                if space >= data.len() || s.inbox[peer].is_empty() { data.len() } else { 0 }
            } else {
                space.min(data.len())
            };
            if n > 0 || (s.message_type && first && data.is_empty()) {
                s.inbox[peer].try_reserve(n).map_err(|_| NtStatus::NO_MEMORY)?;
                if s.message_type {
                    s.messages[peer].try_reserve(1).map_err(|_| NtStatus::NO_MEMORY)?;
                    s.messages[peer].push_back(n);
                }
                s.inbox[peer].extend(&data[..n]);
                event(
                    "pipe write",
                    s.id,
                    format_args!(
                        r#""end":{},"to":{peer},"off":{},"bytes":{n},"queued":{}"#,
                        self.end,
                        s.put[peer],
                        s.inbox[peer].len()
                    ),
                );
                s.put[peer] += n as u64;
                data = &data[n..];
                first = false;
                finished = try_complete(&mut s);
                wake_all(&mut s); // readers waiting for data
            }
            if data.is_empty() {
                drop(s);
                report(&self.pipe);
                return Ok(());
            }
            if finished > 0 {
                drop(s);
                report(&self.pipe);
                continue;
            }
            self.wait(s)?;
        }
    }

    /// An asynchronous end's write: done now (the bytes are in the pipe),
    /// or pending with `request` (Ok(None), STATUS_PENDING) until readers
    /// make room for the rest, behind this end's earlier pending writes.
    pub fn write_async(&self, data: &[u8], request: Request) -> Result<Option<usize>, NtStatus> {
        let total = data.len();
        let mut out = VecDeque::new();
        out.try_reserve_exact(total).map_err(|_| NtStatus::NO_MEMORY)?;
        out.extend(data);
        let mut s = self.pipe.state.lock();
        s.check_connected(self.end, self.generation)?;
        let peer = self.peer();
        let peer_open = if peer == 0 { s.server_open } else { s.conn == Conn::Connected };
        if !peer_open {
            return Err(NtStatus::PIPE_CLOSING);
        }
        let behind = s
            .pending
            .iter()
            .any(|p| p.end == self.end && p.result.is_none() && matches!(p.op, Op::Write { .. }));
        if !behind {
            let (left, _) = put(&mut s, self.end, out);
            out = left;
        }
        if out.is_empty() && !(s.message_type && total == 0 && behind) {
            if total == 0 && s.message_type {
                // A zero-length message.
                s.messages[peer].try_reserve(1).map_err(|_| NtStatus::NO_MEMORY)?;
                s.messages[peer].push_back(0);
            }
            let finished = try_complete(&mut s);
            drop(s);
            if finished > 0 {
                report(&self.pipe);
            }
            return Ok(Some(total));
        }
        let p = Pending {
            end: self.end,
            generation: self.generation,
            op: Op::Write { total },
            request: Some(request),
            token: 0,
            out,
            result: None,
        };
        s.pending.try_push(p).map_err(|_| NtStatus::INSUFFICIENT_RESOURCES)?;
        let finished = try_complete(&mut s);
        drop(s);
        if finished > 0 {
            report(&self.pipe);
        }
        Ok(None)
    }

    /// FSCTL_PIPE_PEEK: (NamedPipeState, bytes available, messages, the
    /// length of the first message, a copy of up to `max` bytes).
    pub fn peek(&self, max: usize) -> Result<(u32, usize, usize, usize, Vec<u8>), NtStatus> {
        let s = self.pipe.state.lock();
        s.check_connected(self.end, self.generation)?;
        let inbox = &s.inbox[self.end];
        let mut copy = Vec::new();
        let message = s.messages[self.end].front().copied().unwrap_or(0);
        let limit = if s.message_type && message > 0 { message } else { inbox.len() };
        let n = max.min(limit);
        copy.try_reserve_exact(n).map_err(|_| NtStatus::NO_MEMORY)?;
        copy.extend(inbox.iter().take(n));
        let peer_open = if self.end == 0 { s.conn == Conn::Connected } else { s.server_open };
        let state = if peer_open { FILE_PIPE_CONNECTED_STATE } else { FILE_PIPE_CLOSING_STATE };
        Ok((state, inbox.len(), s.messages[self.end].len(), message, copy))
    }

    /// FilePipeInformation: the read mode (FILE_PIPE_MESSAGE_MODE needs a
    /// message-type pipe).
    pub fn set_read_mode(&self, message: bool) -> Result<(), NtStatus> {
        if message && !self.pipe.state.lock().message_type {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        self.message_reads.store(message, Ordering::Release);
        Ok(())
    }

    /// The NamedPipeState of this end (FilePipeLocalInformation).
    pub fn state(&self) -> u32 {
        let s = self.pipe.state.lock();
        if self.end == 1 {
            return if s.generation != self.generation {
                FILE_PIPE_DISCONNECTED_STATE
            } else if s.server_open {
                FILE_PIPE_CONNECTED_STATE
            } else {
                FILE_PIPE_CLOSING_STATE
            };
        }
        match s.conn {
            Conn::Listening => FILE_PIPE_LISTENING_STATE,
            Conn::Connected => FILE_PIPE_CONNECTED_STATE,
            Conn::Closing => FILE_PIPE_CLOSING_STATE,
            Conn::Disconnected => FILE_PIPE_DISCONNECTED_STATE,
        }
    }

    /// Cancels this end's pending requests of `process` whose status
    /// block is `iosb` (any if None); returns how many.
    pub fn cancel(&self, process: &KArc<Process>, iosb: Option<u64>) -> usize {
        let mut n = 0;
        {
            let mut s = self.pipe.state.lock();
            for i in 0..s.pending.len() {
                let p = &s.pending[i];
                let Some(request) = &p.request else { continue };
                if p.result.is_none()
                    && p.end == self.end
                    && p.generation == self.generation
                    && KArc::ptr_eq(&request.process, process)
                    && iosb.is_none_or(|b| b == request.iosb)
                {
                    cancel_pending(&mut s, i);
                    n += 1;
                }
            }
        }
        report(&self.pipe);
        n
    }

    /// The pipe (shared by its ends).
    pub fn pipe(&self) -> &KArc<Pipe> {
        &self.pipe
    }

    /// What a kernel handover carries of this end.
    pub fn carried(&self) -> CarriedEnd {
        CarriedEnd {
            end: self.end as u32,
            generation: self.generation,
            synchronous: self.synchronous,
            message_reads: self.message_reads.load(Ordering::Acquire),
            modes: self.modes.load(Ordering::Acquire),
        }
    }
}

/// A pipe as a kernel handover carries it (`handover::programs::PipeRec`):
/// its connection and both directions' queued bytes. The waiting threads
/// are not part of it (they make their calls again), nor is the client's
/// security context (FSCTL_PIPE_IMPERSONATE on a carried pipe fails).
pub struct Carried {
    /// The instance's name if it is listed (clients can open it).
    pub name: Vec<u16>,
    pub max_instances: u32,
    pub message_type: bool,
    pub quota: [u64; 2],
    /// 0 listening, 1 connected, 2 closing, 3 disconnected.
    pub conn: u32,
    pub server_open: bool,
    pub generation: u64,
    pub inbox: [Vec<u8>; 2],
    pub messages: [Vec<u64>; 2],
}

/// What a kernel handover carries of a pipe end.
#[derive(Clone, Copy, Debug)]
pub struct CarriedEnd {
    pub end: u32,
    pub generation: u64,
    pub synchronous: bool,
    pub message_reads: bool,
    pub modes: u32,
}

/// `pipe` as a handover carries it; Err(Some(why)) if it cannot be (an
/// asynchronous request is pending on it), Err(None) out of memory.
pub fn carry(pipe: &KArc<Pipe>) -> Result<Carried, Option<&'static str>> {
    let mut name = Vec::new();
    let mut max_instances = 1;
    for i in INSTANCES.lock().iter().filter(|i| KArc::ptr_eq(&i.pipe, pipe)) {
        name.try_extend_from_slice(&i.name).map_err(|_| None)?;
        max_instances = i.max;
    }
    let s = pipe.state.lock();
    if s.pending.iter().any(|p| p.request.is_some()) {
        return Err(Some("a pipe with asynchronous I/O in flight"));
    }
    // A synchronous read is carried by its thread making its call again,
    // but one already finished holds bytes taken from the queue.
    if s.pending.iter().any(|p| p.result.is_some()) {
        return Err(Some("a pipe with a read finishing"));
    }
    let mut inbox = [Vec::new(), Vec::new()];
    let mut messages = [Vec::new(), Vec::new()];
    for end in 0..2 {
        inbox[end].try_reserve_exact(s.inbox[end].len()).map_err(|_| None)?;
        inbox[end].extend(s.inbox[end].iter().copied());
        messages[end].try_reserve_exact(s.messages[end].len()).map_err(|_| None)?;
        messages[end].extend(s.messages[end].iter().map(|&m| m as u64));
    }
    let conn = match s.conn {
        Conn::Listening => 0,
        Conn::Connected => 1,
        Conn::Closing => 2,
        Conn::Disconnected => 3,
    };
    Ok(Carried {
        name,
        max_instances,
        message_type: s.message_type,
        quota: [s.quota[0] as u64, s.quota[1] as u64],
        conn,
        server_open: s.server_open,
        generation: s.generation,
        inbox,
        messages,
    })
}

/// A pipe rebuilt by a kernel handover (listed again under its name if it
/// was).
pub fn adopt(c: Carried) -> Result<KArc<Pipe>, NtStatus> {
    let conn = match c.conn {
        0 => Conn::Listening,
        1 => Conn::Connected,
        2 => Conn::Closing,
        3 => Conn::Disconnected,
        _ => return Err(NtStatus::INVALID_PARAMETER),
    };
    let mut inbox = [VecDeque::new(), VecDeque::new()];
    let mut messages = [VecDeque::new(), VecDeque::new()];
    for end in 0..2 {
        inbox[end].try_reserve_exact(c.inbox[end].len()).map_err(|_| NtStatus::NO_MEMORY)?;
        inbox[end].extend(c.inbox[end].iter().copied());
        messages[end].try_reserve_exact(c.messages[end].len()).map_err(|_| NtStatus::NO_MEMORY)?;
        for &m in &c.messages[end] {
            messages[end].push_back(usize::try_from(m).map_err(|_| NtStatus::INVALID_PARAMETER)?);
        }
    }
    let quota = |q: u64| usize::try_from(q).map_err(|_| NtStatus::INVALID_PARAMETER);
    let mut given = Vec::new();
    given.try_reserve_exact(c.name.len()).map_err(|_| NtStatus::NO_MEMORY)?;
    given.try_extend_from_slice(&c.name).map_err(|_| NtStatus::NO_MEMORY)?;
    let pipe = KArc::try_new(Pipe {
        client_token: IrqMutex::new(None),
        name: given,
        state: IrqMutex::new(State {
            id: NEXT_PIPE_ID.fetch_add(1, Ordering::Relaxed),
            put: [0; 2],
            taken: [0; 2],
            inbox,
            messages,
            message_type: c.message_type,
            quota: [quota(c.quota[0])?, quota(c.quota[1])?],
            conn,
            server_open: c.server_open,
            generation: c.generation,
            waiters: Vec::new(),
            pending: Vec::new(),
        }),
    })
    .map_err(|_| NtStatus::NO_MEMORY)?;
    if !c.name.is_empty() {
        INSTANCES
            .lock()
            .try_push(Instance { name: c.name, max: c.max_instances, pipe: pipe.clone() })
            .map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(pipe)
}

/// An end of a pipe `adopt` rebuilt.
pub fn adopt_end(pipe: KArc<Pipe>, c: CarriedEnd) -> Result<PipeEnd, NtStatus> {
    if c.end > 1 {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    let end = PipeEnd::new(
        pipe,
        c.end as usize,
        c.generation,
        c.synchronous,
        c.message_reads,
        new_dispatcher()?,
    );
    end.modes.store(c.modes, Ordering::Release);
    Ok(end)
}
