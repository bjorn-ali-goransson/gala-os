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

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

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

enum Op {
    Listen,
    Read { max: usize, message: bool },
}

struct Pending {
    end: usize,
    generation: u64,
    op: Op,
    request: Request,
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
    /// Signaled when an I/O on it completes (waiting on the handle).
    pub dispatcher: KArc<Dispatcher>,
}

/// Completions collected under the pipe's lock, reported after it.
type Done = Vec<(Request, NtStatus, Vec<u8>)>;

fn report(done: Done) {
    for (r, status, data) in done {
        notify::complete(r, status, &data);
    }
}

impl Drop for PipeEnd {
    fn drop(&mut self) {
        if self.end == 0 {
            // No client can connect to a closed instance any more.
            INSTANCES.lock().retain(|i| !KArc::ptr_eq(&i.pipe, &self.pipe));
        }
        let mut done = Vec::new();
        {
            let mut s = self.pipe.state.lock();
            if self.end == 0 {
                s.server_open = false;
            } else if self.generation == s.generation && s.conn == Conn::Connected {
                s.conn = Conn::Closing;
            }
            // Requests of this end are cancelled; the other side's are
            // looked at again (broken pipe).
            let end = self.end;
            let generation = self.generation;
            let mut i = 0;
            while i < s.pending.len() {
                let p = &s.pending[i];
                if p.end == end && (end == 0 || p.generation == generation) {
                    let p = s.pending.remove(i);
                    let _ = done.try_push((p.request, notify::CANCELLED, Vec::new()));
                } else {
                    i += 1;
                }
            }
            try_complete(&mut s, &mut done);
            wake_all(&mut s);
        }
        report(done);
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
    let pipe = KArc::try_new(Pipe {
        state: IrqMutex::new(State {
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
    PipeEnd::new(pipe, 0, 0, o.synchronous, o.message_reads)
}

/// Opens a client end of pipe `name` (its oldest listening instance).
pub fn open_client(name: &[u16], synchronous: bool) -> Result<PipeEnd, NtStatus> {
    let key = upper(name)?;
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
        let mut done = Vec::new();
        try_complete(&mut s, &mut done);
        wake_all(&mut s);
        drop(s);
        let pipe = i.pipe.clone();
        drop(instances);
        report(done);
        // A client starts in byte read mode.
        return PipeEnd::new(pipe, 1, generation, synchronous, false);
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
    }
}

/// Completes the pending requests that can finish now, oldest first.
fn try_complete(s: &mut State, done: &mut Done) {
    let mut i = 0;
    while i < s.pending.len() {
        let (end, generation) = (s.pending[i].end, s.pending[i].generation);
        // Take the op out to borrow the state mutably.
        let op = core::mem::replace(&mut s.pending[i].op, Op::Listen);
        match attempt(s, end, generation, &op) {
            None => {
                s.pending[i].op = op;
                i += 1;
            }
            Some(result) => {
                let p = s.pending.remove(i);
                let (status, data) = match result {
                    Ok((data, status)) => (status, data),
                    Err(e) => (e, Vec::new()),
                };
                if done.try_push((p.request, status, data)).is_err() {
                    // Out of memory: the request is lost rather than the
                    // kernel; its waiter would wait forever, so this is
                    // as far as fallibility goes here.
                }
            }
        }
    }
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

/// How a read or listen of an asynchronous end ends: done now, or pending.
pub enum Outcome {
    Done(NtStatus, Vec<u8>),
    Pending,
}

impl PipeEnd {
    fn new(
        pipe: KArc<Pipe>,
        end: usize,
        generation: u64,
        synchronous: bool,
        message_reads: bool,
    ) -> Result<PipeEnd, NtStatus> {
        let dispatcher = KArc::try_new(Dispatcher::new(1)).map_err(|_| NtStatus::NO_MEMORY)?;
        Ok(PipeEnd {
            pipe,
            end,
            generation,
            synchronous,
            message_reads: AtomicBool::new(message_reads),
            completion: IrqMutex::new(None),
            dispatcher,
        })
    }

    fn peer(&self) -> usize {
        1 - self.end
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
    fn run(&self, op: Op, mut request: Option<Request>) -> Result<Outcome, NtStatus> {
        loop {
            let mut s = self.pipe.state.lock();
            if let Some(result) = attempt(&mut s, self.end, self.generation, &op) {
                return Ok(match result {
                    Ok((data, status)) => Outcome::Done(status, data),
                    Err(e) => Outcome::Done(e, Vec::new()),
                });
            }
            if !self.synchronous
                && let Some(request) = request.take()
            {
                let p = Pending { end: self.end, generation: self.generation, op, request };
                s.pending.try_push(p).map_err(|_| NtStatus::INSUFFICIENT_RESOURCES)?;
                return Ok(Outcome::Pending);
            }
            self.wait(s)?;
        }
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
        let mut done = Vec::new();
        {
            let mut s = self.pipe.state.lock();
            if s.conn == Conn::Disconnected {
                return Err(PIPE_DISCONNECTED);
            }
            s.conn = Conn::Disconnected;
            s.generation += 1;
            for e in 0..2 {
                s.inbox[e].clear();
                s.messages[e].clear();
            }
            try_complete(&mut s, &mut done);
            wake_all(&mut s);
        }
        report(done);
        Ok(())
    }

    /// Writes all of `data`, waiting while the pipe is full.
    pub fn write(&self, mut data: &[u8]) -> Result<(), NtStatus> {
        let peer = self.peer();
        let mut first = true;
        loop {
            let mut done = Vec::new();
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
                data = &data[n..];
                first = false;
                try_complete(&mut s, &mut done);
                wake_all(&mut s); // readers waiting for data
            }
            if data.is_empty() {
                drop(s);
                report(done);
                return Ok(());
            }
            if !done.is_empty() {
                drop(s);
                report(done);
                continue;
            }
            self.wait(s)?;
        }
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
        let mut done = Vec::new();
        {
            let mut s = self.pipe.state.lock();
            let mut i = 0;
            while i < s.pending.len() {
                let p = &s.pending[i];
                if p.end == self.end
                    && p.generation == self.generation
                    && KArc::ptr_eq(&p.request.process, process)
                    && iosb.is_none_or(|b| b == p.request.iosb)
                {
                    let p = s.pending.remove(i);
                    let _ = done.try_push((p.request, notify::CANCELLED, Vec::new()));
                } else {
                    i += 1;
                }
            }
        }
        let n = done.len();
        report(done);
        n
    }
}
