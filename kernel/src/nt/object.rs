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

/// An object directory: named children (see `nt::namespace`).
#[derive(Default)]
pub struct Directory {
    pub entries: IrqMutex<Vec<DirEntry>>,
}

pub struct DirEntry {
    pub name: Vec<u16>,
    pub object: ObjectRef,
}

pub enum ObjectBody {
    /// Console output: writes go to the kernel console.
    ConsoleOutput,
    ConsoleInput,
    Event(Event),
    Directory(Directory),
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
            ObjectBody::Directory(_) => "Directory",
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
            ObjectBody::ConsoleInput | ObjectBody::ConsoleOutput | ObjectBody::Directory(_) => None,
        }
    }

    pub fn mapping(&self) -> GenericMapping {
        match &self.body {
            ObjectBody::Event(_) => EVENT_MAPPING,
            ObjectBody::Directory(_) => DIRECTORY_MAPPING,
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
