//! Keyed events (Windows Internals, "Keyed events"): one object on which
//! threads wait for, or release, a key (a pointer-sized value with its low
//! bit clear). A release wakes exactly one thread of the same process
//! waiting on that key; with none waiting, the releasing thread waits
//! until one comes, and the other way round. Locks built on them (the
//! JDK's wepoll, SRW locks' fallback) need no event per lock.
//!
//! Each blocked thread waits on an event object of its own, queued here
//! with its key; the arrival that pairs with it removes the entry and
//! signals that event. A thread whose wait ends otherwise (timeout, APC)
//! removes its entry, unless a partner already took it: then it was
//! paired and succeeds.

use alloc::vec::Vec;

use super::object::{
    Event, EventType, GenericMapping, KObject, ObjectBody, ObjectRef, READ_CONTROL,
    STANDARD_RIGHTS_REQUIRED,
};
use super::status::NtStatus;
use super::wait::{self, Dispatcher, Timeout};
use crate::mm::heap::{KArc, TryVec};
use crate::sync::IrqMutex;

pub const KEYEDEVENT_WAIT: u32 = 0x1;
pub const KEYEDEVENT_WAKE: u32 = 0x2;
pub const KEYEDEVENT_ALL_ACCESS: u32 = STANDARD_RIGHTS_REQUIRED | 0x3;

pub const KEYED_EVENT_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL | KEYEDEVENT_WAIT,
    write: READ_CONTROL | KEYEDEVENT_WAKE,
    execute: READ_CONTROL,
    all: KEYEDEVENT_ALL_ACCESS,
};

struct Pending {
    key: u64,
    release: bool,
    process: u64,
    wake: ObjectRef,
}

#[derive(Default)]
pub struct KeyedEvent {
    pending: IrqMutex<Vec<Pending>>,
}

/// The system's keyed event (`\KernelObjects\CritSecOutOfMemoryEvent`),
/// which a null handle names.
static SYSTEM: IrqMutex<Option<ObjectRef>> = IrqMutex::new(None);

pub fn system() -> Result<ObjectRef, NtStatus> {
    let mut slot = SYSTEM.lock();
    if let Some(o) = slot.as_ref() {
        return Ok(o.clone());
    }
    let o = KObject::with_permanence(ObjectBody::KeyedEvent(KeyedEvent::default()), true)
        .map_err(|_| NtStatus::NO_MEMORY)?;
    *slot = Some(o.clone());
    Ok(o)
}

/// NtWaitForKeyedEvent (`release` false) or NtReleaseKeyedEvent (true) on
/// `object` for `key`, by a thread of process `process`.
pub fn meet(
    object: &ObjectRef,
    key: u64,
    release: bool,
    process: u64,
    timeout: Timeout,
    alertable: bool,
) -> NtStatus {
    if key & 1 != 0 {
        return NtStatus::INVALID_PARAMETER_1;
    }
    let ObjectBody::KeyedEvent(k) = &object.body else { return NtStatus::OBJECT_TYPE_MISMATCH };
    let wake = {
        let mut pending = k.pending.lock();
        let partner = pending
            .iter()
            .position(|p| p.key == key && p.process == process && p.release != release);
        if let Some(i) = partner {
            let p = pending.remove(i);
            drop(pending);
            wait::set_signal(&p.wake, 1);
            return NtStatus::SUCCESS;
        }
        if timeout == Timeout::Poll {
            return NtStatus::TIMEOUT;
        }
        let event = Event { kind: EventType::Notification, dispatcher: Dispatcher::new(0) };
        let Ok(wake) = KObject::new(ObjectBody::Event(event)) else {
            return NtStatus::NO_MEMORY;
        };
        let entry = Pending { key, release, process, wake: wake.clone() };
        if pending.try_push(entry).is_err() {
            return NtStatus::NO_MEMORY;
        }
        wake
    };
    let objects = [wake];
    let status = wait::wait_for_alertable(&objects, false, timeout, alertable);
    if status == NtStatus::SUCCESS {
        return status;
    }
    // Ended otherwise: withdraw, unless a partner took the entry already.
    let mut pending = k.pending.lock();
    match pending.iter().position(|p| KArc::ptr_eq(&p.wake, &objects[0])) {
        Some(i) => {
            pending.remove(i);
            status
        }
        None => NtStatus::SUCCESS,
    }
}
