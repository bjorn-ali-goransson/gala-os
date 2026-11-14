//! System V shared memory for the Linux personality: shmget(2),
//! shmat(2), shmdt(2) and shmctl(2) (ROADMAP LD step 9: GIMP hands image
//! tiles to its plug-ins in a segment; X's MIT-SHM uses them too).
//! user/lx/shm_test is the spec, recorded on Linux.
//!
//! A segment is a pagefile-backed section of its size rounded up to
//! pages (as MAP_SHARED anonymous memory's, `lx::mm`), held by the
//! registry; an attachment is a view of that same section object
//! (`mm::attach`), so every attachment in every process shares its
//! pages, a fork's child's too (its views keep their sections). The
//! attach count (`shm_nattch`) is the number of views of the section in
//! the running processes, which is what Linux counts (its VMAs: a fork
//! adds the child's, an exit or execve takes them away). IPC_RMID marks a
//! segment removed (SHM_DEST in its mode, its key IPC_PRIVATE: no longer
//! found by key) and it goes, its pages freed, once nothing maps it
//! (`collect`, run after a detach, an exit and an execve); until then it
//! can still be attached by id, as GIMP's plug-ins do with the segment
//! GIMP removes at once.
//!
//! Ids are a slot below SHMMNI plus 32768 times a sequence number, as
//! Linux makes them, so a removed segment's id is not soon reused. Access
//! follows Linux's ipcperms: the mode's owner bits for the owner or the
//! creator, the group bits for their group, the others' otherwise; uid 0
//! may do anything. IPC_SET changes the owner and the permissions,
//! IPC_STAT reports struct shmid64_ds.
//!
//! Limits: shmdt unmaps the view starting at the address (one that
//! mprotect or a partial munmap split leaves its other pieces mapped);
//! SHM_REMAP, SHM_LOCK, SHM_UNLOCK, IPC_INFO, SHM_INFO and SHM_STAT are
//! not supported (EINVAL, ledgered `linux option missing`); `shm_lpid`
//! and `shm_dtime` change at shmat and shmdt only, not when an exit
//! detaches.
//!
//! Observability: an `lx shm` event (`events=ipc`) per call (pid, op
//! `get`, `attach`, `detach` or `ctl`, the segment's id, `arg`: the key,
//! the address or the command, the size, the errno) and per segment
//! destroyed (`destroyed`, after the event of the call that let it go);
//! the state dump lists the segments (`lx: shm segment ...`).

use alloc::vec::Vec;

use super::dispatch::{Call, Result};
use super::errno::{EACCES, EEXIST, EFAULT, EINVAL, ENOENT, ENOMEM, ENOSPC, EPERM};
use super::identity::Ids;
use crate::mm::heap::{KArc, TryVec};
use crate::nt::ledger;
use crate::nt::object::{KObject, ObjectBody, ObjectRef};
use crate::nt::section::Section;
use crate::nt::vad;
use crate::sync::IrqMutex;

pub const IPC_PRIVATE: i32 = 0;
pub const IPC_CREAT: u64 = 0o1000;
pub const IPC_EXCL: u64 = 0o2000;
pub const IPC_RMID: u64 = 0;
pub const IPC_SET: u64 = 1;
pub const IPC_STAT: u64 = 2;
pub const SHM_RDONLY: u64 = 0o10000;
pub const SHM_RND: u64 = 0o20000;
pub const SHM_REMAP: u64 = 0o40000;
pub const SHM_EXEC: u64 = 0o100000;
/// In a segment's mode: removed, going at its last detach.
pub const SHM_DEST: u32 = 0o1000;
/// Segments at most (Linux's default SHMMNI).
pub const SHMMNI: usize = 4096;
/// The largest segment (Linux's default SHMMAX).
pub const SHMMAX: u64 = u64::MAX - (1 << 24);
/// An id's slot is below this; the rest is the sequence number.
const SEQ_MULTIPLIER: u32 = 32768;
/// struct shmid64_ds on x86-64.
pub const SHMID_DS_SIZE: usize = 112;
const PAGE: u64 = 4096;

/// A segment, its memory an `M` (the section object; nothing in host
/// tests).
#[derive(Clone, Debug)]
pub struct Segment<M> {
    pub id: i32,
    pub key: i32,
    pub size: u64,
    /// The permissions (low 9 bits) and SHM_DEST.
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub cuid: u32,
    pub cgid: u32,
    pub cpid: u32,
    pub lpid: u32,
    pub atime: i64,
    pub dtime: i64,
    pub ctime: i64,
    /// shmat calls between finding it and mapping it (`collect` keeps it).
    pub pending: u32,
    pub mem: M,
}

/// What shmget finds for a key.
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    Existing(i32),
    Create,
}

/// The segments.
pub struct Registry<M> {
    segments: Vec<Segment<M>>,
    seq: u32,
}

/// Whether `ids` may access `s` for `want` (rwx bits, as in a mode's
/// "others" field): Linux's ipcperms.
fn permits<M>(s: &Segment<M>, ids: &Ids, want: u32) -> bool {
    if ids.uid == 0 {
        return true;
    }
    let granted = if ids.uid == s.uid || ids.uid == s.cuid {
        s.mode >> 6
    } else if ids.groups().iter().any(|&g| g == s.gid || g == s.cgid) {
        s.mode >> 3
    } else {
        s.mode
    };
    want & !granted & 0o7 == 0
}

/// The access shmget's flags ask for (their permission bits, folded).
fn requested(flags: u64) -> u32 {
    let f = (flags & 0o777) as u32;
    (f >> 6) | (f >> 3) | f
}

impl<M> Default for Registry<M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M> Registry<M> {
    pub const fn new() -> Self {
        Registry { segments: Vec::new(), seq: 0 }
    }

    pub fn find(&self, id: i32) -> Option<&Segment<M>> {
        self.segments.iter().find(|s| s.id == id)
    }

    pub fn find_mut(&mut self, id: i32) -> Option<&mut Segment<M>> {
        self.segments.iter_mut().find(|s| s.id == id)
    }

    pub fn segments(&self) -> &[Segment<M>] {
        &self.segments
    }

    /// shmget's look-up for `key`: the existing segment's id or that one
    /// is to be made, or the errno.
    pub fn lookup(
        &self,
        key: i32,
        size: u64,
        flags: u64,
        ids: &Ids,
    ) -> core::result::Result<Lookup, u16> {
        let existing = (key != IPC_PRIVATE)
            .then(|| self.segments.iter().find(|s| s.key == key && s.mode & SHM_DEST == 0))
            .flatten();
        match existing {
            None if key != IPC_PRIVATE && flags & IPC_CREAT == 0 => Err(ENOENT),
            None => {
                if size == 0 || size > SHMMAX {
                    return Err(EINVAL);
                }
                if self.segments.len() >= SHMMNI {
                    return Err(ENOSPC);
                }
                Ok(Lookup::Create)
            }
            Some(_) if flags & (IPC_CREAT | IPC_EXCL) == IPC_CREAT | IPC_EXCL => Err(EEXIST),
            Some(s) if size > s.size => Err(EINVAL),
            Some(s) if !permits(s, ids, requested(flags)) => Err(EACCES),
            Some(s) => Ok(Lookup::Existing(s.id)),
        }
    }

    /// Adds a segment of `size` bytes for `key` (shmget's IPC_CREAT, after
    /// `lookup` said so and its memory was made): its id.
    #[allow(clippy::too_many_arguments)]
    pub fn insert(
        &mut self,
        key: i32,
        size: u64,
        flags: u64,
        ids: &Ids,
        pid: u32,
        now: i64,
        mem: M,
    ) -> core::result::Result<i32, u16> {
        let slot = (0..SEQ_MULTIPLIER)
            .find(|slot| !self.segments.iter().any(|s| s.id as u32 % SEQ_MULTIPLIER == *slot))
            .ok_or(ENOSPC)?;
        self.seq = (self.seq + 1) % 65536;
        let id = (self.seq * SEQ_MULTIPLIER + slot) as i32;
        self.segments.try_reserve(1).map_err(|_| ENOMEM)?;
        let s = Segment {
            id,
            key,
            size,
            mode: (flags & 0o777) as u32,
            uid: ids.uid,
            gid: ids.gid,
            cuid: ids.uid,
            cgid: ids.gid,
            cpid: pid,
            lpid: 0,
            atime: 0,
            dtime: 0,
            ctime: now,
            pending: 0,
            mem,
        };
        // Room reserved above.
        let _ = self.segments.try_push(s);
        Ok(id)
    }

    /// IPC_RMID: removed (no longer found by key), going once nothing
    /// maps it.
    pub fn remove(&mut self, id: i32, ids: &Ids, now: i64) -> core::result::Result<(), u16> {
        let s = self.find_mut(id).ok_or(EINVAL)?;
        if ids.uid != 0 && ids.uid != s.uid && ids.uid != s.cuid {
            return Err(EPERM);
        }
        s.mode |= SHM_DEST;
        s.key = IPC_PRIVATE;
        s.ctime = now;
        Ok(())
    }

    /// IPC_SET: owner and permissions (from struct shmid64_ds's perm).
    pub fn set(
        &mut self,
        id: i32,
        ids: &Ids,
        uid: u32,
        gid: u32,
        mode: u32,
        now: i64,
    ) -> core::result::Result<(), u16> {
        let s = self.find_mut(id).ok_or(EINVAL)?;
        if ids.uid != 0 && ids.uid != s.uid && ids.uid != s.cuid {
            return Err(EPERM);
        }
        s.uid = uid;
        s.gid = gid;
        s.mode = (s.mode & !0o777) | (mode & 0o777);
        s.ctime = now;
        Ok(())
    }

    /// Takes out the removed segments `attached` says nothing maps.
    pub fn take_unused(&mut self, mut attached: impl FnMut(&M) -> u64) -> Vec<Segment<M>> {
        let mut gone = Vec::new();
        let mut i = 0;
        while i < self.segments.len() {
            let s = &self.segments[i];
            if s.mode & SHM_DEST != 0 && s.pending == 0 && attached(&s.mem) == 0 {
                let s = self.segments.remove(i);
                // A segment that cannot be noted still goes.
                let _ = gone.try_push(s);
            } else {
                i += 1;
            }
        }
        gone
    }
}

/// `s` as struct shmid64_ds, with `nattch` attachments.
pub fn layout<M>(s: &Segment<M>, nattch: u64) -> [u8; SHMID_DS_SIZE] {
    let mut b = [0u8; SHMID_DS_SIZE];
    b[0..4].copy_from_slice(&s.key.to_le_bytes());
    b[4..8].copy_from_slice(&s.uid.to_le_bytes());
    b[8..12].copy_from_slice(&s.gid.to_le_bytes());
    b[12..16].copy_from_slice(&s.cuid.to_le_bytes());
    b[16..20].copy_from_slice(&s.cgid.to_le_bytes());
    b[20..24].copy_from_slice(&s.mode.to_le_bytes());
    // The sequence number (24..26), as Linux's id carries it.
    b[24..26].copy_from_slice(&((s.id as u32 / SEQ_MULTIPLIER) as u16).to_le_bytes());
    b[48..56].copy_from_slice(&s.size.to_le_bytes());
    b[56..64].copy_from_slice(&s.atime.to_le_bytes());
    b[64..72].copy_from_slice(&s.dtime.to_le_bytes());
    b[72..80].copy_from_slice(&s.ctime.to_le_bytes());
    b[80..84].copy_from_slice(&s.cpid.to_le_bytes());
    b[84..88].copy_from_slice(&s.lpid.to_le_bytes());
    b[88..96].copy_from_slice(&nattch.to_le_bytes());
    b
}

// ---- The system calls ------------------------------------------------

static SEGMENTS: IrqMutex<Registry<ObjectRef>> = IrqMutex::new(Registry::new());

fn now() -> i64 {
    (super::time::realtime_ns() / 1_000_000_000) as i64
}

/// An `lx shm` event: the call's segment (`id`, -1 if none), `arg`
/// (shmget's key, shmat's and shmdt's address, shmctl's command), the
/// size (shmget's) and the errno (0: done).
fn noted(call: &Call<'_>, op: &str, id: i64, arg: u64, size: u64, r: Result) -> Result {
    crate::events::emit(
        crate::events::Category::Ipc,
        "lx shm",
        format_args!(
            r#""pid":{},"op":"{op}","id":{id},"arg":{arg},"size":{size},"errno":{}"#,
            call.process.id,
            r.err().unwrap_or(0)
        ),
    );
    r
}

/// The views of the section `mem` in the running processes.
fn attachments(processes: &[KArc<crate::nt::process::Process>], mem: &ObjectRef) -> u64 {
    processes.iter().map(|p| super::mm::count_views(p, mem)).sum()
}

/// Destroys the removed segments nothing maps any more (their pages go
/// with their sections' last references).
pub fn collect() {
    if !SEGMENTS.lock().segments().iter().any(|s| s.mode & SHM_DEST != 0) {
        return;
    }
    let Ok(processes) = crate::nt::process::running() else { return };
    let gone = SEGMENTS.lock().take_unused(|mem| attachments(&processes, mem));
    for s in gone {
        crate::events::emit(
            crate::events::Category::Ipc,
            "lx shm",
            format_args!(r#""op":"destroyed","id":{},"size":{}"#, s.id, s.size),
        );
    }
}

/// shmget(key, size, flags).
pub fn shmget(call: &Call<'_>, key: u64, size: u64, flags: u64) -> Result {
    let key = key as u32 as i32;
    let ids = super::identity::of_process(&call.process);
    let r = (|| {
        if let Lookup::Existing(id) = SEGMENTS.lock().lookup(key, size, flags, &ids)? {
            return Ok(id as u64);
        }
        let len = size.checked_next_multiple_of(PAGE).ok_or(EINVAL)?;
        let section =
            Section::new(len, vad::PAGE_EXECUTE_READWRITE, crate::nt::section::SEC_COMMIT)
                .map_err(|_| ENOMEM)?;
        let mem = KObject::new(ObjectBody::Section(section)).map_err(|_| ENOMEM)?;
        let mut segments = SEGMENTS.lock();
        // Another process may have made the key's segment meanwhile.
        match segments.lookup(key, size, flags, &ids)? {
            Lookup::Existing(id) => Ok(id as u64),
            Lookup::Create => {
                let pid = call.process.id as u32;
                segments.insert(key, size, flags, &ids, pid, now(), mem).map(|id| id as u64)
            }
        }
    })();
    noted(call, "get", r.map_or(-1, |id| id as i64), key as u32 as u64, size, r)
}

/// shmat(id, addr, flags).
pub fn shmat(call: &Call<'_>, id: u64, addr: u64, flags: u64) -> Result {
    let id = id as u32 as i32;
    let r = (|| {
        let at = match addr {
            0 => None,
            a if flags & SHM_RND != 0 => Some(a / PAGE * PAGE),
            a if !a.is_multiple_of(PAGE) => return Err(EINVAL),
            a => Some(a),
        };
        if flags & SHM_REMAP != 0 {
            ledger::record("shmat", "linux option missing", SHM_REMAP);
            return Err(EINVAL);
        }
        let mut prot = super::mm::PROT_READ;
        let mut want = 0o4;
        if flags & SHM_RDONLY == 0 {
            prot |= super::mm::PROT_WRITE;
            want |= 0o2;
        }
        if flags & SHM_EXEC != 0 {
            prot |= super::mm::PROT_EXEC;
            want |= 0o1;
        }
        let ids = super::identity::of_process(&call.process);
        let (mem, size) = {
            let mut segments = SEGMENTS.lock();
            let s = segments.find_mut(id).ok_or(EINVAL)?;
            if !permits(s, &ids, want) {
                return Err(EACCES);
            }
            s.pending += 1;
            (s.mem.clone(), s.size)
        };
        let mapped = super::mm::attach(&call.process, at, size, prot, mem);
        let mut segments = SEGMENTS.lock();
        if let Some(s) = segments.find_mut(id) {
            s.pending -= 1;
            if mapped.is_ok() {
                s.atime = now();
                s.lpid = call.process.id as u32;
            }
        }
        mapped
    })();
    noted(call, "attach", id as i64, addr, 0, r)
}

/// shmdt(addr).
pub fn shmdt(call: &Call<'_>, addr: u64) -> Result {
    let mut detached = -1;
    let r = (|| {
        if !addr.is_multiple_of(PAGE) {
            return Err(EINVAL);
        }
        let (mem, size, offset) = super::mm::view_at(&call.process, addr).ok_or(EINVAL)?;
        let id = {
            let segments = SEGMENTS.lock();
            let s = segments.segments().iter().find(|s| KArc::ptr_eq(&s.mem, &mem));
            match s {
                Some(s) if offset == 0 => s.id,
                _ => return Err(EINVAL),
            }
        };
        drop(mem);
        detached = i64::from(id);
        super::mm::munmap(&call.process, addr, size)?;
        if let Some(s) = SEGMENTS.lock().find_mut(id) {
            s.dtime = now();
            s.lpid = call.process.id as u32;
        }
        Ok(0)
    })();
    let r = noted(call, "detach", detached, addr, 0, r);
    collect();
    r
}

/// shmctl(id, cmd, buf).
pub fn shmctl(call: &Call<'_>, id: u64, cmd: u64, buf: u64) -> Result {
    let id = id as u32 as i32;
    let ids = super::identity::of_process(&call.process);
    let r = match cmd {
        IPC_STAT => (|| {
            let processes = crate::nt::process::running().map_err(|_| ENOMEM)?;
            let b = {
                let segments = SEGMENTS.lock();
                let s = segments.find(id).ok_or(EINVAL)?;
                if !permits(s, &ids, 0o4) {
                    return Err(EACCES);
                }
                layout(s, attachments(&processes, &s.mem))
            };
            call.process.write(buf, &b).map_err(|_| EFAULT)?;
            Ok(0)
        })(),
        IPC_SET => (|| {
            let mut b = [0u8; 24];
            call.process.read(buf, &mut b).map_err(|_| EFAULT)?;
            let word = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
            SEGMENTS.lock().set(id, &ids, word(4), word(8), word(20), now())?;
            Ok(0)
        })(),
        IPC_RMID => SEGMENTS.lock().remove(id, &ids, now()).map(|()| 0),
        _ => {
            ledger::record("shmctl", "linux option missing", cmd);
            Err(EINVAL)
        }
    };
    let r = noted(call, "ctl", id as i64, cmd, 0, r);
    if cmd == IPC_RMID {
        collect();
    }
    r
}

/// The state dump's segments (`lx: shm segment ...`), if the registry
/// is free (a dump from an NMI must not wait).
pub fn dump() {
    let Some(segments) = SEGMENTS.try_lock() else {
        crate::info!("lx: shm segments: the registry is busy");
        return;
    };
    for s in segments.segments() {
        crate::info!(
            "lx: shm segment {} key {:#x} {} bytes mode {:o} owner uid {} creator pid {}{}{}",
            s.id,
            s.key,
            s.size,
            s.mode & 0o777,
            s.uid,
            s.cpid,
            if s.mode & SHM_DEST != 0 { ", removed" } else { "" },
            if s.pending != 0 { ", being attached" } else { "" }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(uid: u32) -> Ids {
        super::super::identity::test_ids(uid, uid)
    }

    #[test]
    fn a_private_segment_is_made_and_found_by_id() {
        let mut r: Registry<()> = Registry::new();
        let me = user(1000);
        assert_eq!(r.lookup(IPC_PRIVATE, 100, IPC_CREAT | 0o600, &me), Ok(Lookup::Create));
        let id = r.insert(IPC_PRIVATE, 100, IPC_CREAT | 0o600, &me, 7, 5, ()).unwrap();
        let s = r.find(id).unwrap();
        assert_eq!((s.size, s.mode, s.cpid, s.ctime, s.key), (100, 0o600, 7, 5, 0));
        // Another private one is another segment.
        let other = r.insert(IPC_PRIVATE, 100, IPC_CREAT | 0o600, &me, 7, 5, ()).unwrap();
        assert_ne!(other, id);
        assert_eq!(r.lookup(IPC_PRIVATE, 0, IPC_CREAT, &me), Err(EINVAL));
    }

    #[test]
    fn a_key_is_found_again_and_its_errors_are_linuxs() {
        let mut r: Registry<()> = Registry::new();
        let me = user(1000);
        assert_eq!(r.lookup(42, 8192, 0o640, &me), Err(ENOENT));
        let id = r.insert(42, 8192, IPC_CREAT | 0o640, &me, 7, 5, ()).unwrap();
        assert_eq!(r.lookup(42, 8192, IPC_CREAT | IPC_EXCL | 0o640, &me), Err(EEXIST));
        assert_eq!(r.lookup(42, 100, 0, &me), Ok(Lookup::Existing(id)));
        assert_eq!(r.lookup(42, 8193, 0, &me), Err(EINVAL));
        // Another user: the "others" bits (none) refuse reading.
        assert_eq!(r.lookup(42, 0, 0o400, &user(1001)), Err(EACCES));
        assert_eq!(r.lookup(42, 0, 0o400, &user(0)), Ok(Lookup::Existing(id)));
        assert_eq!(r.remove(id, &user(1001), 6), Err(EPERM));
        r.remove(id, &me, 6).unwrap();
        // Removed: no longer found by key, still by id.
        assert_eq!(r.lookup(42, 0, 0, &me), Err(ENOENT));
        let s = r.find(id).unwrap();
        assert_eq!((s.key, s.mode & SHM_DEST, s.ctime), (IPC_PRIVATE, SHM_DEST, 6));
    }

    #[test]
    fn a_removed_segment_goes_when_nothing_maps_it() {
        let mut r: Registry<u64> = Registry::new();
        let me = user(1000);
        let a = r.insert(IPC_PRIVATE, 4096, 0o600, &me, 7, 5, 2).unwrap();
        let b = r.insert(IPC_PRIVATE, 4096, 0o600, &me, 7, 5, 0).unwrap();
        // Not removed: kept even unmapped.
        assert!(r.take_unused(|&n| n).is_empty());
        r.remove(a, &me, 6).unwrap();
        r.remove(b, &me, 6).unwrap();
        // a is mapped twice: it stays; b goes.
        let gone: Vec<i32> = r.take_unused(|&n| n).iter().map(|s| s.id).collect();
        assert_eq!(gone, [b]);
        // Being attached: kept.
        r.find_mut(a).unwrap().pending = 1;
        assert!(r.take_unused(|_| 0).is_empty());
    }

    #[test]
    fn ids_carry_a_sequence_so_a_slot_is_not_the_same_id_again() {
        let mut r: Registry<u64> = Registry::new();
        let me = user(1000);
        let a = r.insert(IPC_PRIVATE, 4096, 0o600, &me, 7, 5, 0).unwrap();
        r.remove(a, &me, 6).unwrap();
        r.take_unused(|_| 0);
        let b = r.insert(IPC_PRIVATE, 4096, 0o600, &me, 7, 5, 0).unwrap();
        assert_eq!(a as u32 % SEQ_MULTIPLIER, b as u32 % SEQ_MULTIPLIER);
        assert_ne!(a, b);
    }

    #[test]
    fn ipc_stat_lays_out_shmid64_ds() {
        let mut r: Registry<()> = Registry::new();
        let id = r.insert(9, 10000, 0o640, &user(1000), 7, 5, ()).unwrap();
        let b = layout(r.find(id).unwrap(), 3);
        assert_eq!(i32::from_le_bytes(b[0..4].try_into().unwrap()), 9);
        assert_eq!(u32::from_le_bytes(b[20..24].try_into().unwrap()), 0o640);
        assert_eq!(u64::from_le_bytes(b[48..56].try_into().unwrap()), 10000);
        assert_eq!(i64::from_le_bytes(b[72..80].try_into().unwrap()), 5);
        assert_eq!(u32::from_le_bytes(b[80..84].try_into().unwrap()), 7);
        assert_eq!(u64::from_le_bytes(b[88..96].try_into().unwrap()), 3);
    }
}
