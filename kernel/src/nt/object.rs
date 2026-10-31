//! Object manager core: reference-counted kernel objects and per-process
//! handle tables with granted access masks.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use super::status::NtStatus;
use super::wait::Dispatcher;
use crate::mm::heap::{AllocError, KArc, TryVec};
use crate::sync::IrqMutex;

// Standard and generic access rights (winnt.h).
pub const DELETE: u32 = 0x0001_0000;
pub const READ_CONTROL: u32 = 0x0002_0000;
pub const SYNCHRONIZE: u32 = 0x0010_0000;
pub const STANDARD_RIGHTS_REQUIRED: u32 = 0x000f_0000;
pub const MAXIMUM_ALLOWED: u32 = 0x0200_0000;
pub const GENERIC_READ: u32 = 0x8000_0000;
pub const GENERIC_WRITE: u32 = 0x4000_0000;
pub const GENERIC_EXECUTE: u32 = 0x2000_0000;
pub const GENERIC_ALL: u32 = 0x1000_0000;

/// How generic rights map to specific rights for one object type.
#[derive(Clone, Copy, Debug)]
pub struct GenericMapping {
    pub read: u32,
    pub write: u32,
    pub execute: u32,
    pub all: u32,
}

impl GenericMapping {
    /// Turns a requested access mask into the specific rights granted.
    pub fn map(&self, desired: u32) -> u32 {
        let mut access = desired
            & !(GENERIC_READ | GENERIC_WRITE | GENERIC_EXECUTE | GENERIC_ALL | MAXIMUM_ALLOWED);
        if desired & GENERIC_READ != 0 {
            access |= self.read;
        }
        if desired & GENERIC_WRITE != 0 {
            access |= self.write;
        }
        if desired & GENERIC_EXECUTE != 0 {
            access |= self.execute;
        }
        if desired & (GENERIC_ALL | MAXIMUM_ALLOWED) != 0 {
            access |= self.all;
        }
        access & self.all
    }
}

pub const EVENT_QUERY_STATE: u32 = 0x0001;
pub const EVENT_MODIFY_STATE: u32 = 0x0002;
pub const EVENT_ALL_ACCESS: u32 = STANDARD_RIGHTS_REQUIRED | SYNCHRONIZE | 0x3;

pub const EVENT_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL | EVENT_QUERY_STATE,
    write: READ_CONTROL | EVENT_MODIFY_STATE,
    execute: READ_CONTROL | SYNCHRONIZE,
    all: EVENT_ALL_ACCESS,
};

const FILE_ALL_ACCESS: u32 = STANDARD_RIGHTS_REQUIRED | SYNCHRONIZE | 0x1ff;

/// FILE_GENERIC_READ / WRITE / EXECUTE / FILE_ALL_ACCESS.
pub const FILE_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL | SYNCHRONIZE | 0x89,
    write: READ_CONTROL | SYNCHRONIZE | 0x116,
    execute: READ_CONTROL | SYNCHRONIZE | 0xa0,
    all: FILE_ALL_ACCESS,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventType {
    /// Stays signaled until cleared (manual reset).
    Notification,
    /// Releases one waiter, then resets (auto reset).
    Synchronization,
}

pub struct Event {
    pub kind: EventType,
    pub dispatcher: Dispatcher,
}

pub const MUTANT_QUERY_STATE: u32 = 0x0001;
pub const MUTANT_ALL_ACCESS: u32 = STANDARD_RIGHTS_REQUIRED | SYNCHRONIZE | 0x1;
pub const MUTANT_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL | MUTANT_QUERY_STATE,
    write: READ_CONTROL,
    execute: READ_CONTROL | SYNCHRONIZE,
    all: MUTANT_ALL_ACCESS,
};
pub const SEMAPHORE_QUERY_STATE: u32 = 0x0001;
pub const SEMAPHORE_MODIFY_STATE: u32 = 0x0002;
pub const SEMAPHORE_ALL_ACCESS: u32 = STANDARD_RIGHTS_REQUIRED | SYNCHRONIZE | 0x3;
pub const SEMAPHORE_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL | SEMAPHORE_QUERY_STATE,
    write: READ_CONTROL | SEMAPHORE_MODIFY_STATE,
    execute: READ_CONTROL | SYNCHRONIZE,
    all: SEMAPHORE_ALL_ACCESS,
};

/// A mutant (mutex): owned by one thread at a time, recursively.
pub struct Mutant {
    pub dispatcher: Dispatcher,
    pub state: crate::sync::IrqMutex<MutantState>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MutantState {
    /// NT thread id of the owner.
    pub owner: Option<u64>,
    /// Recursive acquisitions by the owner.
    pub count: u32,
    /// Set when the owner exited without releasing it; reported (and
    /// cleared) by the next acquisition.
    pub abandoned: bool,
}

impl MutantState {
    /// MUTANT_BASIC_INFORMATION.CurrentCount: 1 when free, then 0, -1, ...
    pub fn current_count(&self) -> i32 {
        1 - self.count as i32
    }
}

/// A semaphore: its dispatcher's signal state is the current count.
pub struct Semaphore {
    pub dispatcher: Dispatcher,
    pub maximum: i32,
}

/// The new count after releasing `release` units of a semaphore at
/// `current` of `maximum`.
pub fn semaphore_release(current: i32, maximum: i32, release: i32) -> Result<i32, NtStatus> {
    if release <= 0 {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    match current.checked_add(release) {
        Some(n) if n <= maximum => Ok(n),
        _ => Err(NtStatus::SEMAPHORE_LIMIT_EXCEEDED),
    }
}

/// Whether (initial, maximum) are valid semaphore counts.
pub fn semaphore_counts_valid(initial: i32, maximum: i32) -> bool {
    maximum > 0 && initial >= 0 && initial <= maximum
}

pub const IO_COMPLETION_QUERY_STATE: u32 = 0x0001;
pub const IO_COMPLETION_MODIFY_STATE: u32 = 0x0002;
pub const IO_COMPLETION_ALL_ACCESS: u32 = STANDARD_RIGHTS_REQUIRED | SYNCHRONIZE | 0x3;
pub const IO_COMPLETION_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL | IO_COMPLETION_QUERY_STATE,
    write: READ_CONTROL | IO_COMPLETION_MODIFY_STATE,
    execute: READ_CONTROL | SYNCHRONIZE,
    all: IO_COMPLETION_ALL_ACCESS,
};

/// A completion packet: what NtRemoveIoCompletion returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletionPacket {
    pub key: u64,
    pub context: u64,
    pub status: u32,
    pub information: u64,
}

/// The FIFO of an I/O completion port.
#[derive(Default)]
pub struct CompletionQueue {
    packets: Vec<CompletionPacket>,
}

impl CompletionQueue {
    pub fn push(&mut self, packet: CompletionPacket) -> Result<(), AllocError> {
        self.packets.try_push(packet)
    }

    pub fn pop(&mut self) -> Option<CompletionPacket> {
        (!self.packets.is_empty()).then(|| self.packets.remove(0))
    }

    pub fn len(&self) -> usize {
        self.packets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }
}

/// An I/O completion port: waitable while packets are queued.
pub struct IoCompletion {
    pub dispatcher: Dispatcher,
    pub queue: IrqMutex<CompletionQueue>,
}

pub const TIMER_QUERY_STATE: u32 = 0x0001;
pub const TIMER_MODIFY_STATE: u32 = 0x0002;
pub const TIMER_ALL_ACCESS: u32 = STANDARD_RIGHTS_REQUIRED | SYNCHRONIZE | 0x3;
pub const TIMER_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL | TIMER_QUERY_STATE,
    write: READ_CONTROL | TIMER_MODIFY_STATE,
    execute: READ_CONTROL | SYNCHRONIZE,
    all: TIMER_ALL_ACCESS,
};

/// A waitable timer (`nt::timer` runs it).
pub struct Timer {
    pub dispatcher: Dispatcher,
    /// SynchronizationTimer: a satisfied wait resets it.
    pub synchronization: bool,
    pub state: IrqMutex<TimerState>,
}

#[derive(Clone, Default)]
pub struct TimerState {
    /// Tick at which it expires next, if armed.
    pub due: Option<u64>,
    /// The expiry as asked for (NT system time); `due` is it rounded up to
    /// a tick. QueryTimer's RemainingTime comes from this one.
    pub due_time: u64,
    /// Ticks between expiries (0 = once).
    pub period: u64,
    /// (thread, routine, context) of the APC to queue on expiry.
    pub apc: Option<(ObjectRef, u64, u64)>,
}

pub const SYMBOLIC_LINK_QUERY: u32 = 0x0001;
pub const SYMBOLIC_LINK_ALL_ACCESS: u32 = STANDARD_RIGHTS_REQUIRED | 0x1;
pub const SYMBOLIC_LINK_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL | SYMBOLIC_LINK_QUERY,
    write: READ_CONTROL,
    execute: READ_CONTROL | SYMBOLIC_LINK_QUERY,
    all: SYMBOLIC_LINK_ALL_ACCESS,
};

/// A symbolic link in the object namespace: lookups that reach it continue
/// at `target` (an absolute path).
pub struct SymbolicLink {
    pub target: Vec<u16>,
}

pub const DIRECTORY_QUERY: u32 = 0x0001;
pub const DIRECTORY_TRAVERSE: u32 = 0x0002;
pub const DIRECTORY_CREATE_OBJECT: u32 = 0x0004;
pub const DIRECTORY_CREATE_SUBDIRECTORY: u32 = 0x0008;
pub const DIRECTORY_ALL_ACCESS: u32 = STANDARD_RIGHTS_REQUIRED | 0xf;

pub const DIRECTORY_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL | DIRECTORY_QUERY | DIRECTORY_TRAVERSE,
    write: READ_CONTROL | DIRECTORY_CREATE_OBJECT | DIRECTORY_CREATE_SUBDIRECTORY,
    execute: READ_CONTROL | DIRECTORY_QUERY | DIRECTORY_TRAVERSE,
    all: DIRECTORY_ALL_ACCESS,
};

pub const THREAD_TERMINATE: u32 = 0x0001;
pub const THREAD_SUSPEND_RESUME: u32 = 0x0002;
pub const THREAD_QUERY_INFORMATION: u32 = 0x0040;
pub const THREAD_QUERY_LIMITED_INFORMATION: u32 = 0x0800;
pub const THREAD_ALL_ACCESS: u32 = STANDARD_RIGHTS_REQUIRED | SYNCHRONIZE | 0xffff;

pub const THREAD_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL | THREAD_QUERY_INFORMATION | 0x0008,
    write: READ_CONTROL | THREAD_TERMINATE | THREAD_SUSPEND_RESUME | 0x0034,
    execute: READ_CONTROL | SYNCHRONIZE | THREAD_QUERY_LIMITED_INFORMATION | 0x1000,
    all: THREAD_ALL_ACCESS,
};

pub const SECTION_QUERY: u32 = 0x0001;
pub const SECTION_MAP_WRITE: u32 = 0x0002;
pub const SECTION_MAP_READ: u32 = 0x0004;
pub const SECTION_MAP_EXECUTE: u32 = 0x0008;
pub const SECTION_ALL_ACCESS: u32 = STANDARD_RIGHTS_REQUIRED | 0x1f;

pub const SECTION_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL | SECTION_QUERY | SECTION_MAP_READ,
    write: READ_CONTROL | SECTION_MAP_WRITE,
    execute: READ_CONTROL | SECTION_MAP_EXECUTE,
    all: SECTION_ALL_ACCESS,
};

pub const PROCESS_TERMINATE: u32 = 0x0001;
pub const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
pub const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
pub const PROCESS_ALL_ACCESS: u32 = STANDARD_RIGHTS_REQUIRED | SYNCHRONIZE | 0xffff;

pub const PROCESS_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL | PROCESS_QUERY_INFORMATION | 0x0010,
    write: READ_CONTROL | 0x0bea,
    execute: READ_CONTROL | SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
    all: PROCESS_ALL_ACCESS,
};

/// An object directory: named children (see `nt::namespace`).
#[derive(Default)]
pub struct Directory {
    pub entries: IrqMutex<Vec<DirEntry>>,
}

pub struct DirEntry {
    pub name: Vec<u16>,
    pub object: ObjectRef,
}

// Objects already live in their own `KArc` allocation, so variant size
// differences only cost a little memory per object.
#[allow(clippy::large_enum_variant)]
pub enum ObjectBody {
    /// Console output: writes go to the kernel console.
    ConsoleOutput,
    ConsoleInput,
    Event(Event),
    Mutant(Mutant),
    Semaphore(Semaphore),
    SymbolicLink(SymbolicLink),
    IoCompletion(IoCompletion),
    Timer(Timer),
    Directory(Directory),
    Thread(super::thread::NtThread),
    Section(super::section::Section),
    /// The file system volume (`\Device\HarddiskVolume1`, `\??\C:`).
    Volume,
    File(super::file::FileObject),
    Process(KArc<super::process::Process>),
    /// `\Device\Afd`: opening a name below it creates a socket endpoint.
    AfdDevice,
    /// An AFD socket endpoint (a SOCKET is a handle to one).
    Socket(super::afd::Endpoint),
}

/// Where a named object lives in the namespace.
pub struct ObjectName {
    pub parent: ObjectRef,
    pub name: Vec<u16>,
}

pub struct KObject {
    pub body: ObjectBody,
    /// Set while the object is linked into a directory.
    pub name: IrqMutex<Option<ObjectName>>,
    /// Open handles; a non-permanent named object leaves the namespace
    /// when this drops to zero.
    handle_count: AtomicUsize,
    /// Permanent objects stay named without handles (system directories).
    pub permanent: bool,
}

pub type ObjectRef = KArc<KObject>;

impl KObject {
    pub fn new(body: ObjectBody) -> Result<ObjectRef, AllocError> {
        Self::with_permanence(body, false)
    }

    pub fn with_permanence(body: ObjectBody, permanent: bool) -> Result<ObjectRef, AllocError> {
        KArc::try_new(KObject {
            body,
            name: IrqMutex::new(None),
            handle_count: AtomicUsize::new(0),
            permanent,
        })
    }

    pub fn handle_count(&self) -> usize {
        self.handle_count.load(Ordering::Acquire)
    }

    pub fn type_name(&self) -> &'static str {
        match self.body {
            ObjectBody::ConsoleInput | ObjectBody::ConsoleOutput => "File",
            ObjectBody::Event(_) => "Event",
            ObjectBody::Mutant(_) => "Mutant",
            ObjectBody::Semaphore(_) => "Semaphore",
            ObjectBody::SymbolicLink(_) => "SymbolicLink",
            ObjectBody::IoCompletion(_) => "IoCompletion",
            ObjectBody::Timer(_) => "Timer",
            ObjectBody::Directory(_) => "Directory",
            ObjectBody::Thread(_) => "Thread",
            ObjectBody::Section(_) => "Section",
            ObjectBody::Volume => "Device",
            ObjectBody::File(_) => "File",
            ObjectBody::Process(_) => "Process",
            ObjectBody::AfdDevice => "Device",
            ObjectBody::Socket(_) => "File",
        }
    }

    pub fn as_symbolic_link(&self) -> Option<&[u16]> {
        match &self.body {
            ObjectBody::SymbolicLink(l) => Some(&l.target),
            _ => None,
        }
    }

    pub fn as_directory(&self) -> Option<&Directory> {
        match &self.body {
            ObjectBody::Directory(d) => Some(d),
            _ => None,
        }
    }

    /// Signal state shared with waiters, for waitable objects.
    pub fn dispatcher(&self) -> Option<&Dispatcher> {
        match &self.body {
            ObjectBody::Event(e) => Some(&e.dispatcher),
            ObjectBody::Mutant(m) => Some(&m.dispatcher),
            ObjectBody::Semaphore(s) => Some(&s.dispatcher),
            ObjectBody::IoCompletion(c) => Some(&c.dispatcher),
            ObjectBody::Timer(t) => Some(&t.dispatcher),
            ObjectBody::Thread(t) => Some(&t.dispatcher),
            ObjectBody::Process(p) => Some(&p.dispatcher),
            ObjectBody::File(f) => Some(&f.dispatcher),
            ObjectBody::Socket(e) => Some(&e.state.dispatcher),
            ObjectBody::ConsoleInput
            | ObjectBody::ConsoleOutput
            | ObjectBody::Directory(_)
            | ObjectBody::SymbolicLink(_)
            | ObjectBody::Section(_)
            | ObjectBody::Volume
            | ObjectBody::AfdDevice => None,
        }
    }

    pub fn mapping(&self) -> GenericMapping {
        match &self.body {
            ObjectBody::Event(_) => EVENT_MAPPING,
            ObjectBody::Mutant(_) => MUTANT_MAPPING,
            ObjectBody::Semaphore(_) => SEMAPHORE_MAPPING,
            ObjectBody::SymbolicLink(_) => SYMBOLIC_LINK_MAPPING,
            ObjectBody::IoCompletion(_) => IO_COMPLETION_MAPPING,
            ObjectBody::Timer(_) => TIMER_MAPPING,
            ObjectBody::Directory(_) => DIRECTORY_MAPPING,
            ObjectBody::Thread(_) => THREAD_MAPPING,
            ObjectBody::Section(_) => SECTION_MAPPING,
            ObjectBody::Volume
            | ObjectBody::File(_)
            | ObjectBody::AfdDevice
            | ObjectBody::Socket(_) => FILE_MAPPING,
            ObjectBody::Process(_) => PROCESS_MAPPING,
            ObjectBody::ConsoleInput | ObjectBody::ConsoleOutput => GenericMapping {
                read: READ_CONTROL | SYNCHRONIZE | 0x89,
                write: READ_CONTROL | SYNCHRONIZE | 0x116,
                execute: READ_CONTROL | SYNCHRONIZE | 0xa0,
                all: FILE_ALL_ACCESS,
            },
        }
    }
}

/// A handle value as seen by user mode (multiples of 4, never 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle(pub u64);

impl Handle {
    /// `NtCurrentProcess()`.
    pub const CURRENT_PROCESS: Handle = Handle(u64::MAX);
    /// `NtCurrentThread()`.
    pub const CURRENT_THREAD: Handle = Handle(u64::MAX - 1);
}

#[derive(Clone)]
pub struct HandleEntry {
    pub object: ObjectRef,
    /// Specific rights granted to this handle.
    pub access: u32,
    pub attributes: u32,
}

impl HandleEntry {
    /// Checks that the handle grants all of `required`.
    pub fn require(&self, required: u32) -> Result<&ObjectRef, NtStatus> {
        if self.access & required == required {
            Ok(&self.object)
        } else {
            Err(NtStatus::ACCESS_DENIED)
        }
    }
}

#[derive(Default)]
pub struct HandleTable {
    entries: Vec<Option<HandleEntry>>,
}

fn slot(handle: Handle) -> Option<usize> {
    // The low two bits of a handle are ignored by NT (tag bits).
    usize::try_from((handle.0 >> 2).checked_sub(1)?).ok()
}

impl HandleTable {
    pub fn insert(&mut self, entry: HandleEntry) -> Result<Handle, AllocError> {
        let object = entry.object.clone();
        let index = match self.entries.iter().position(Option::is_none) {
            Some(i) => {
                self.entries[i] = Some(entry);
                i
            }
            None => {
                self.entries.try_push(Some(entry))?;
                self.entries.len() - 1
            }
        };
        object.handle_count.fetch_add(1, Ordering::AcqRel);
        Ok(Handle((index as u64 + 1) * 4))
    }

    /// Inserts `object` with its full rights (used for kernel-created handles).
    pub fn insert_object(&mut self, object: ObjectRef) -> Result<Handle, AllocError> {
        let access = object.mapping().all;
        self.insert(HandleEntry { object, access, attributes: 0 })
    }

    pub fn get(&self, handle: Handle) -> Option<HandleEntry> {
        self.entries.get(slot(handle)?)?.clone()
    }

    /// Closes every handle (process teardown). The entries are returned so
    /// the caller can drop them outside the handle table lock.
    pub fn close_all(&mut self) -> Vec<HandleEntry> {
        let mut out = Vec::new();
        let _ = out.try_reserve(self.entries.len());
        for i in 0..self.entries.len() {
            if let Some(entry) = self.close(Handle((i as u64 + 1) * 4)) {
                // Capacity was reserved; if that failed, drop in place.
                let _ = out.try_push(entry);
            }
        }
        out
    }

    pub fn close(&mut self, handle: Handle) -> Option<HandleEntry> {
        let entry = self.entries.get_mut(slot(handle)?)?.take()?;
        if entry.object.handle_count.fetch_sub(1, Ordering::AcqRel) == 1 && !entry.object.permanent
        {
            super::namespace::unlink(&entry.object);
        }
        Some(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_queue_is_fifo() {
        let mut q = CompletionQueue::default();
        let p = |key| CompletionPacket { key, context: key * 2, status: 0, information: 1 };
        q.push(p(1)).unwrap();
        q.push(p(2)).unwrap();
        assert_eq!(q.len(), 2);
        assert_eq!(q.pop(), Some(p(1)));
        assert_eq!(q.pop(), Some(p(2)));
        assert_eq!(q.pop(), None);
        assert!(q.is_empty());
    }

    #[test]
    fn semaphore_and_mutant_counts() {
        assert_eq!(semaphore_release(0, 3, 2), Ok(2));
        assert_eq!(semaphore_release(2, 3, 2), Err(NtStatus::SEMAPHORE_LIMIT_EXCEEDED));
        assert_eq!(semaphore_release(2, 3, 0), Err(NtStatus::INVALID_PARAMETER));
        assert_eq!(
            semaphore_release(i32::MAX, i32::MAX, 1),
            Err(NtStatus::SEMAPHORE_LIMIT_EXCEEDED)
        );
        assert!(semaphore_counts_valid(2, 3));
        assert!(!semaphore_counts_valid(4, 3));
        assert!(!semaphore_counts_valid(0, 0));
        let free = MutantState::default();
        assert_eq!(free.current_count(), 1);
        let twice = MutantState { owner: Some(8), count: 2, abandoned: false };
        assert_eq!(twice.current_count(), -1);
    }

    fn console() -> ObjectRef {
        KObject::new(ObjectBody::ConsoleOutput).unwrap()
    }

    #[test]
    fn handles_are_multiples_of_four_and_reused() {
        let mut t = HandleTable::default();
        let a = t.insert_object(console()).unwrap();
        let b = t.insert_object(console()).unwrap();
        assert_eq!((a.0, b.0), (4, 8));
        assert!(t.get(b).is_some());
        assert!(t.get(Handle(b.0 | 3)).is_some(), "tag bits ignored");
        assert!(t.get(Handle(0)).is_none());
        assert!(t.get(Handle(12)).is_none());
        assert!(t.get(Handle::CURRENT_PROCESS).is_none());
        assert!(t.close(a).is_some());
        assert!(t.get(a).is_none());
        assert_eq!(t.insert_object(console()).unwrap(), a);
    }

    #[test]
    fn closing_drops_the_reference() {
        let mut t = HandleTable::default();
        let obj = console();
        let h = t.insert_object(obj.clone()).unwrap();
        assert_eq!(KArc::strong_count(&obj), 2);
        drop(t.close(h));
        assert_eq!(KArc::strong_count(&obj), 1);
    }

    #[test]
    fn generic_mapping_and_access_checks() {
        let m = EVENT_MAPPING;
        assert_eq!(m.map(GENERIC_ALL), EVENT_ALL_ACCESS);
        assert_eq!(m.map(MAXIMUM_ALLOWED), EVENT_ALL_ACCESS);
        assert_eq!(m.map(GENERIC_READ), READ_CONTROL | EVENT_QUERY_STATE);
        assert_eq!(m.map(SYNCHRONIZE | 0x8000), SYNCHRONIZE, "undefined bits dropped");
        let e = HandleEntry { object: console(), access: SYNCHRONIZE, attributes: 0 };
        assert!(e.require(SYNCHRONIZE).is_ok());
        assert_eq!(e.require(EVENT_MODIFY_STATE).err(), Some(NtStatus::ACCESS_DENIED));
    }
}
