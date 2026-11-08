//! Kernel pool and handle quotas, NT's half (ROADMAP R6, ARCHITECTURE
//! "Pool and handle quotas"; the block itself, the `quota=` limits and
//! the peaks are core: `crate::quota`). What a process makes the kernel
//! hold for it is charged to the process, so the program using too much
//! is the one that fails (STATUS_QUOTA_EXCEEDED, or
//! STATUS_INSUFFICIENT_RESOURCES at the handle limit) instead of every
//! program starving for kernel memory.
//!
//! gala has one kernel heap; the split follows Windows: waitable objects
//! (events, mutants, semaphores, timers, completion ports, threads,
//! processes, files, pipes, sockets), pipe buffers, sockets' buffers and
//! completion packets are "nonpaged", everything else (handle entries,
//! sections and their page tables, keys, tokens, names, security
//! descriptors) "paged".
//!
//! What is charged (`object_cost`, `HANDLE_ENTRY`):
//! - each handle entry, to the process whose table holds it, released by
//!   its close;
//! - each object, to the first process that gets a handle to it (the
//!   creator, for every object a service creates), released when the
//!   object is deleted (`Charge`'s drop), even if that process has ended
//!   meanwhile; permanent objects (the namespace's directories, devices)
//!   are the system's and never charged;
//! - an object's own big buffers: a pipe instance's inbound and outbound
//!   quotas (the most its byte queues hold), a section's page table, a
//!   token's groups, privileges and default DACL;
//! - what grows after an object's creation (`grow`): a token's default
//!   DACL beyond what was charged; a completion port's queued packets
//!   (`CompletionQueue`, to the port's creator); a socket's buffers
//!   (`net::tcp::Owner`, to the endpoint's creator).
//!
//! Limits are fixed when a process is created (`new_block`): the `quota=`
//! boot option, else the defaults; NtSetInformationProcess
//! (ProcessQuotaLimits) changes them.

use super::object::{KObject, ObjectBody};
use super::status::NtStatus;
use crate::mm::heap::{AllocError, KArc};
pub use crate::quota::{Block, DEFAULT_HANDLES, Exceeded, Pool, configure};

/// STATUS_QUOTA_EXCEEDED (ERROR_NOT_ENOUGH_QUOTA, 1816).
pub const QUOTA_EXCEEDED: NtStatus = NtStatus::QUOTA_EXCEEDED;

/// What one handle entry costs (paged pool): the table's slot.
pub const HANDLE_ENTRY: u64 = core::mem::size_of::<Option<super::object::HandleEntry>>() as u64;

/// A pool quota's refusal as NT reports it (the handle limit's is
/// STATUS_INSUFFICIENT_RESOURCES, mapped where it is checked).
pub fn status(_: Exceeded) -> NtStatus {
    QUOTA_EXCEEDED
}

/// The machine's memory the default limits are a share of.
fn memory() -> u64 {
    super::platform::frame_stats().1 * 4096
}

/// The limits a new process gets now: (paged, nonpaged, handles).
pub fn limits() -> (u64, u64, u64) {
    crate::quota::limits(memory())
}

/// A quota block with the limits new processes get now (`limits`).
pub fn new_block(pid: u64, name: &'static str) -> Result<KArc<Block>, AllocError> {
    let (paged, nonpaged, handles) = limits();
    Block::with_limits(pid, name, paged, nonpaged, handles)
}

/// The largest peaks so far, for the end-of-run line.
pub fn peaks() -> crate::quota::Peaks {
    crate::quota::peaks(memory())
}

/// An object's charge: given back to its block when the object goes.
pub struct Charge {
    block: KArc<Block>,
    paged: u64,
    nonpaged: u64,
}

impl Drop for Charge {
    fn drop(&mut self) {
        self.block.release(Pool::Paged, self.paged);
        self.block.release(Pool::NonPaged, self.nonpaged);
    }
}

/// What object `o` costs: (paged, nonpaged) bytes. Reads only fields
/// fixed at its creation, its name and its security descriptor.
pub fn object_cost(o: &KObject) -> (u64, u64) {
    let header = core::mem::size_of::<KObject>() as u64;
    let name = o.name.lock().as_ref().map_or(0, |n| {
        (n.name.len() * 2 + core::mem::size_of::<super::object::ObjectName>()) as u64
    });
    let security = o.security.lock().as_ref().map_or(0, |s| s.len() as u64);
    let mut paged = name + security;
    let mut nonpaged = 0;
    match &o.body {
        ObjectBody::Pipe(end) => nonpaged += end.buffer_bytes() as u64,
        ObjectBody::Section(s) => paged += s.table_bytes() as u64,
        ObjectBody::Thread(_) => nonpaged += core::mem::size_of::<super::thread::NtThread>() as u64,
        ObjectBody::Token(t) => {
            paged += t.fixed + t.dynamic.load(core::sync::atomic::Ordering::Acquire);
        }
        _ => {}
    }
    if o.dispatcher().is_some() {
        nonpaged += header;
    } else {
        paged += header;
    }
    (paged, nonpaged)
}

/// Charges object `o` to `block` unless it is charged already (to the
/// process that first held a handle to it) or permanent. Fails with
/// STATUS_QUOTA_EXCEEDED, charging nothing, if `check` and that would
/// exceed a limit.
pub fn claim(o: &KObject, block: &KArc<Block>, check: bool) -> Result<(), NtStatus> {
    if o.permanent {
        return Ok(());
    }
    let mut slot = o.quota.lock();
    if slot.is_some() {
        return Ok(());
    }
    let (paged, nonpaged) = object_cost(o);
    if check {
        block.try_charge(Pool::Paged, paged).map_err(status)?;
        if let Err(e) = block.try_charge(Pool::NonPaged, nonpaged) {
            block.release(Pool::Paged, paged);
            return Err(status(e));
        }
    } else {
        block.charge(Pool::Paged, paged);
        block.charge(Pool::NonPaged, nonpaged);
    }
    *slot = Some(Charge { block: block.clone(), paged, nonpaged });
    Ok(())
}

/// Charges `bytes` more of `pool` to whoever object `o` is charged to:
/// memory it holds that grew after its creation (a token's default DACL).
/// Fails with STATUS_QUOTA_EXCEEDED, charging nothing, over the limit.
/// Nothing is charged for an object no process holds yet (its charge,
/// when it gets one, is `object_cost` then) or a permanent one.
pub fn grow(o: &KObject, pool: Pool, bytes: u64) -> Result<(), NtStatus> {
    let mut slot = o.quota.lock();
    let Some(c) = slot.as_mut() else { return Ok(()) };
    c.block.try_charge(pool, bytes).map_err(status)?;
    match pool {
        Pool::Paged => c.paged += bytes,
        Pool::NonPaged => c.nonpaged += bytes,
    }
    Ok(())
}

/// Gives back `bytes` of `pool` of object `o`'s charge (memory it held
/// that it no longer does).
pub fn shrink(o: &KObject, pool: Pool, bytes: u64) {
    let mut slot = o.quota.lock();
    let Some(c) = slot.as_mut() else { return };
    let field = match pool {
        Pool::Paged => &mut c.paged,
        Pool::NonPaged => &mut c.nonpaged,
    };
    let bytes = bytes.min(*field);
    *field -= bytes;
    c.block.release(pool, bytes);
}

/// The quota block object `o` is charged to, if any.
pub fn owner(o: &KObject) -> Option<KArc<Block>> {
    o.quota.lock().as_ref().map(|c| c.block.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects_stay_small() {
        // Every object is as large as the largest ObjectBody variant: a
        // large one is boxed (a thread is about 10 KiB), or every event
        // and file would cost that much.
        assert!(core::mem::size_of::<KObject>() <= 1024, "{}", core::mem::size_of::<KObject>());
    }
}
