//! Named pipes (`\Device\NamedPipe`), as far as anonymous pipes need them:
//! kernel32's CreatePipe creates a uniquely named instance with
//! NtCreateNamedPipeFile (the server end, which it reads) and opens it
//! (the client end, which it writes). Byte mode, one instance per name.
//!
//! A pipe holds a byte queue per direction (bounded by the creator's
//! inbound and outbound quotas) and whether each end is open. An end
//! closes when its object goes away (its last handle, inherited and
//! duplicated ones included), which wakes the other side: a reader then
//! drains what is left and gets STATUS_PIPE_BROKEN, a writer fails with
//! STATUS_PIPE_CLOSING. Reads return what is there (at least one byte,
//! waiting if there is none); writes complete fully, waiting while the
//! pipe is full, as Windows' anonymous pipes do.

use alloc::collections::VecDeque;
use alloc::vec::Vec;

use super::platform::{self, ThreadId};
use super::status::NtStatus;
use super::wait::Dispatcher;
use crate::mm::heap::{KArc, TryVec};
use crate::sync::IrqMutex;

/// Quota used when the creator asks for none (CreatePipe's nSize 0).
const DEFAULT_QUOTA: usize = 4096;

/// FILE_PIPE_PEEK_BUFFER.NamedPipeState values.
pub const FILE_PIPE_CONNECTED_STATE: u32 = 3;
pub const FILE_PIPE_CLOSING_STATE: u32 = 4;

pub struct Pipe {
    state: IrqMutex<State>,
}

struct State {
    /// Bytes waiting to be read by end 0 (server) and end 1 (client).
    inbox: [VecDeque<u8>; 2],
    /// The most each inbox holds before writers wait.
    quota: [usize; 2],
    open: [bool; 2],
    /// Threads waiting for data, space or a close.
    waiters: Vec<ThreadId>,
}

/// One end of a pipe: the body of its file object.
pub struct PipeEnd {
    pipe: KArc<Pipe>,
    /// 0: the server end (NtCreateNamedPipeFile), 1: the client end.
    end: usize,
    /// Signaled when an I/O on it completes (waiting on the handle).
    pub dispatcher: Dispatcher,
}

impl Drop for PipeEnd {
    fn drop(&mut self) {
        if self.end == 0 {
            // No client can connect to a closed instance any more.
            LISTENING.lock().retain(|(_, p)| !KArc::ptr_eq(p, &self.pipe));
        }
        let mut s = self.pipe.state.lock();
        s.open[self.end] = false;
        wake_all(&mut s);
    }
}

fn wake_all(s: &mut State) {
    for t in s.waiters.drain(..) {
        platform::wake(t);
    }
}

/// Creates `\Device\NamedPipe`.
pub fn init() -> Result<(), NtStatus> {
    use super::object::{KObject, ObjectBody};
    let device = KObject::with_permanence(ObjectBody::NamedPipeDevice, true)
        .map_err(|_| NtStatus::NO_MEMORY)?;
    let mut name = Vec::new();
    for c in "\\Device\\NamedPipe".encode_utf16() {
        name.try_push(c).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    super::namespace::create(None, &name, device, true, false).map(drop)
}

/// Instances waiting for their client, by name (upper-cased).
static LISTENING: IrqMutex<Vec<(Vec<u16>, KArc<Pipe>)>> = IrqMutex::new(Vec::new());

fn upper(name: &[u16]) -> Result<Vec<u16>, NtStatus> {
    let mut v = Vec::new();
    v.try_reserve_exact(name.len()).map_err(|_| NtStatus::NO_MEMORY)?;
    v.extend(
        name.iter()
            .map(|&c| if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }),
    );
    Ok(v)
}

/// Creates instance `name` and returns its server end. `inbound` and
/// `outbound` are the quotas (bytes towards the server and the client).
pub fn create_server(name: &[u16], inbound: u32, outbound: u32) -> Result<PipeEnd, NtStatus> {
    let key = upper(name)?;
    let mut listening = LISTENING.lock();
    if listening.iter().any(|(n, _)| *n == key) {
        // One instance per name (MaximumInstances 1, as CreatePipe asks).
        return Err(NtStatus::INSTANCE_NOT_AVAILABLE);
    }
    let quota = |q: u32| if q == 0 { DEFAULT_QUOTA } else { (q as usize).max(DEFAULT_QUOTA) };
    let pipe = KArc::try_new(Pipe {
        state: IrqMutex::new(State {
            inbox: [VecDeque::new(), VecDeque::new()],
            quota: [quota(inbound), quota(outbound)],
            open: [true, false],
            waiters: Vec::new(),
        }),
    })
    .map_err(|_| NtStatus::NO_MEMORY)?;
    listening.try_push((key, pipe.clone())).map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(PipeEnd { pipe, end: 0, dispatcher: Dispatcher::new(1) })
}

/// Opens the client end of instance `name`.
pub fn open_client(name: &[u16]) -> Result<PipeEnd, NtStatus> {
    let key = upper(name)?;
    let mut listening = LISTENING.lock();
    let i = listening.iter().position(|(n, _)| *n == key).ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
    let (_, pipe) = listening.swap_remove(i);
    let mut s = pipe.state.lock();
    if !s.open[0] {
        return Err(NtStatus::PIPE_NOT_AVAILABLE);
    }
    s.open[1] = true;
    drop(s);
    Ok(PipeEnd { pipe, end: 1, dispatcher: Dispatcher::new(1) })
}

impl PipeEnd {
    fn peer(&self) -> usize {
        1 - self.end
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

    /// Reads up to `max` bytes (at least one, waiting for it); end of data
    /// once the other end is closed is STATUS_PIPE_BROKEN.
    pub fn read(&self, max: usize) -> Result<Vec<u8>, NtStatus> {
        loop {
            let mut s = self.pipe.state.lock();
            let inbox = &mut s.inbox[self.end];
            if !inbox.is_empty() || max == 0 {
                let n = max.min(inbox.len());
                let mut out = Vec::new();
                out.try_reserve_exact(n).map_err(|_| NtStatus::NO_MEMORY)?;
                out.extend(inbox.drain(..n));
                wake_all(&mut s); // writers waiting for space
                return Ok(out);
            }
            if !s.open[self.peer()] {
                return Err(NtStatus::PIPE_BROKEN);
            }
            self.wait(s)?;
        }
    }

    /// Writes all of `data`, waiting while the pipe is full.
    pub fn write(&self, mut data: &[u8]) -> Result<(), NtStatus> {
        let peer = self.peer();
        loop {
            let mut s = self.pipe.state.lock();
            if !s.open[peer] {
                return Err(NtStatus::PIPE_CLOSING);
            }
            let space = s.quota[peer].saturating_sub(s.inbox[peer].len());
            let n = space.min(data.len());
            if n > 0 {
                s.inbox[peer].try_reserve(n).map_err(|_| NtStatus::NO_MEMORY)?;
                s.inbox[peer].extend(&data[..n]);
                data = &data[n..];
                wake_all(&mut s); // readers waiting for data
            }
            if data.is_empty() {
                return Ok(());
            }
            self.wait(s)?;
        }
    }

    /// FSCTL_PIPE_PEEK: (NamedPipeState, bytes available, a copy of up to
    /// `max` of them).
    pub fn peek(&self, max: usize) -> Result<(u32, usize, Vec<u8>), NtStatus> {
        let s = self.pipe.state.lock();
        let inbox = &s.inbox[self.end];
        let mut copy = Vec::new();
        let n = max.min(inbox.len());
        copy.try_reserve_exact(n).map_err(|_| NtStatus::NO_MEMORY)?;
        copy.extend(inbox.iter().take(n));
        let state =
            if s.open[self.peer()] { FILE_PIPE_CONNECTED_STATE } else { FILE_PIPE_CLOSING_STATE };
        Ok((state, inbox.len(), copy))
    }
}
