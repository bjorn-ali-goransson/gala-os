//! Linux memory calls on the core address space: mmap(2) of anonymous
//! memory and of files, munmap(2), mprotect(2), mremap(2), madvise(2)
//! and brk(2), page-granular, with Linux's errors (man-pages;
//! user/lx/mmap_test, brk_test and dyn_test record what Linux 6.18 does).
//!
//! The pages live in the process's VAD (`nt::vad`, its page-granular
//! Linux methods). Anonymous memory is committed private memory, so the
//! core's fault path (`nt::vm::resolve_fault`) gives it zeroed frames on
//! first touch, and `nt::vm::sync_range` keeps the page tables in step
//! (frames of removed pages are freed; changed protections take effect).
//! Linux overcommits: the pages are counted in the commit charge but
//! never refused by it.
//!
//! A file mapping is a view (`nt::section::View`, in the process's
//! `views`) of a section on the file (`Section::new_linux_file`, which
//! keeps the open file description), its pages MEM_MAPPED in the VAD:
//! the fault path maps the file's own pages (`ramfs` keeps a mapped
//! file's contents in whole pages), shared by every process mapping the
//! file. MAP_PRIVATE pages are copy-on-write (PAGE_WRITECOPY until the
//! first write makes a private copy: `vm::copy_on_write`); MAP_SHARED
//! ones on a file open for writing write the file. Linux cuts and moves
//! mappings page by page, so the views are cut with them: a munmap,
//! MAP_FIXED or mprotect over part of a view leaves views for the parts
//! that stay, each with its offset in the file (`cut`).
//!
//! MAP_SHARED anonymous memory (and a MAP_SHARED mapping of /dev/zero)
//! is a view of a pagefile-backed section of its own (`Section::new`,
//! zeroed pages made at the first touch), so a fork's child, whose views
//! hold the same sections, shares it, pages first touched after the
//! fork included.
//!
//! Lock order as everywhere: the views, the VAD, then the space (inside
//! `sync_range`). No user memory is touched with these locks held, and a
//! view (whose section may close a file when it goes) is dropped after
//! them.

use alloc::vec::Vec;

use super::dispatch::Call;
use super::errno::{EACCES, EBADF, EEXIST, EFAULT, EINVAL, ENODEV, ENOMEM, EOVERFLOW, EPERM};
use super::state::Object;
use crate::fs::ramfs::{SEAL_FUTURE_WRITE, SEAL_WRITE};
use crate::mm::heap::TryVec;
use crate::nt::object::{KObject, ObjectBody};
use crate::nt::process::Process;
use crate::nt::section::{Section, View};
use crate::nt::status::NtStatus;
use crate::nt::vad::{self, Vad};
use crate::nt::{charge, ledger, vm};

pub const PAGE: u64 = 4096;
/// The end of the user address space for Linux's checks (TASK_SIZE on
/// x86_64 with 4-level paging; elf::USER_END).
pub const TASK_SIZE: u64 = super::elf::USER_END;
/// The top of the main thread's stack (its last byte is below), and its
/// size (RLIMIT_STACK's usual 8 MiB, committed, given frames on touch).
pub const STACK_END: u64 = 0x7fff_fffe_0000;
pub const STACK_SIZE: u64 = 8 << 20;
/// mmap without a usable hint places mappings top-down from here (below
/// the stack, as Linux's mmap base is).
pub const MMAP_TOP: u64 = STACK_END - STACK_SIZE - (16 << 20);

pub const PROT_READ: u64 = 1;
pub const PROT_WRITE: u64 = 2;
pub const PROT_EXEC: u64 = 4;

pub const MAP_SHARED: u64 = 0x01;
pub const MAP_PRIVATE: u64 = 0x02;
pub const MAP_SHARED_VALIDATE: u64 = 0x03;
pub const MAP_TYPE: u64 = 0x0f;
pub const MAP_FIXED: u64 = 0x10;
pub const MAP_ANONYMOUS: u64 = 0x20;
pub const MAP_FIXED_NOREPLACE: u64 = 0x10_0000;

/// mremap's flags.
pub const MREMAP_MAYMOVE: u64 = 1;
pub const MREMAP_FIXED: u64 = 2;
pub const MREMAP_DONTUNMAP: u64 = 4;

/// madvise's advice (the ones taken).
pub const MADV_NORMAL: u64 = 0;
pub const MADV_RANDOM: u64 = 1;
pub const MADV_SEQUENTIAL: u64 = 2;
pub const MADV_WILLNEED: u64 = 3;
pub const MADV_DONTNEED: u64 = 4;
pub const MADV_FREE: u64 = 8;
pub const MADV_MERGEABLE: u64 = 12;
pub const MADV_UNMERGEABLE: u64 = 13;
pub const MADV_HUGEPAGE: u64 = 14;
pub const MADV_NOHUGEPAGE: u64 = 15;
pub const MADV_DONTDUMP: u64 = 16;
pub const MADV_DODUMP: u64 = 17;

/// The NT page protection for Linux's PROT_* bits (on x86_64 write and
/// exec imply read).
pub fn protect_of(prot: u64) -> u32 {
    let (w, x) = (prot & PROT_WRITE != 0, prot & PROT_EXEC != 0);
    match (prot & 7 == 0, w, x) {
        (true, ..) => vad::PAGE_NOACCESS,
        (_, true, true) => vad::PAGE_EXECUTE_READWRITE,
        (_, false, true) => vad::PAGE_EXECUTE_READ,
        (_, true, false) => vad::PAGE_READWRITE,
        (_, false, false) => vad::PAGE_READONLY,
    }
}

/// The NT page protection for PROT_* bits on pages of `section` (a file
/// mapping): with PROT_WRITE, copy-on-write unless the section writes
/// through (MAP_SHARED).
fn view_protect(prot: u64, section: &Section) -> u32 {
    let protect = protect_of(prot);
    if section.writes_through() {
        return protect;
    }
    match protect {
        vad::PAGE_READWRITE => vad::PAGE_WRITECOPY,
        vad::PAGE_EXECUTE_READWRITE => vad::PAGE_EXECUTE_WRITECOPY,
        p => p,
    }
}

fn round_up(len: u64) -> Option<u64> {
    len.checked_next_multiple_of(PAGE)
}

/// After a change of `[start, end)` in the VAD: the charge follows and
/// the page tables match.
pub fn sync(process: &Process, vad: &Vad, start: u64, end: u64) {
    charge::sync(process, vad.private_commit());
    vm::sync_range(process, vad, start, end - start);
}

/// Cuts the views overlapping `[start, end)` out of `views`: a view
/// partly inside leaves views for its parts outside (each at its offset
/// in the section), the ones taken out go to `removed` (dropped by the
/// caller after its locks). All or nothing on out of memory.
fn cut(views: &mut Vec<View>, start: u64, end: u64, removed: &mut Vec<View>) -> Result<(), u16> {
    let overlaps = |v: &View| v.base < end && start < v.base + v.size;
    let hit = views.iter().filter(|v| overlaps(v)).count();
    if hit == 0 {
        return Ok(());
    }
    // Room first: at most two pieces per view cut, and the removed ones.
    views.try_reserve(2 * hit).map_err(|_| ENOMEM)?;
    removed.try_reserve(hit).map_err(|_| ENOMEM)?;
    let mut i = 0;
    while i < views.len() {
        if !overlaps(&views[i]) {
            i += 1;
            continue;
        }
        let v = views.swap_remove(i);
        let v_end = v.base + v.size;
        if v.base < start {
            let _ = views.try_push(View::new(v.base, start - v.base, v.offset, v.section.clone()));
        }
        if end < v_end {
            let _ = views.try_push(View::new(
                end,
                v_end - end,
                v.offset + (end - v.base),
                v.section.clone(),
            ));
        }
        let _ = removed.try_push(v);
        // The pieces were pushed at the end, past `i`: they do not
        // overlap, so the loop passes them by.
    }
    Ok(())
}

/// Removes `[start, end)` from the VAD and the views (with the locks
/// held: views, then the VAD), the page tables after.
fn unmap_locked(
    process: &Process,
    views: &mut Vec<View>,
    vad: &mut Vad,
    start: u64,
    end: u64,
    removed: &mut Vec<View>,
) -> Result<(), u16> {
    cut(views, start, end, removed)?;
    vad.unmap_range(start, end).map_err(|_| ENOMEM)?;
    sync(process, vad, start, end);
    Ok(())
}

/// What an mmap maps.
enum Source {
    /// Private anonymous memory.
    Anonymous,
    /// A section's pages: a file's (a new section for each mmap; the
    /// frames are the file's, shared), or MAP_SHARED anonymous memory's
    /// (its own pagefile-backed section), and the view's offset in it.
    File(Section, u64),
}

/// The file mapping `fd` and `offset` ask for: EBADF for no fd (or an
/// O_PATH one), ENODEV for what is not a regular file, EACCES for an fd
/// not open for reading or, with MAP_SHARED and PROT_WRITE, for writing
/// (mmap(2)). /dev/zero maps anonymous memory.
fn file_source(
    call: &Call<'_>,
    fd: i64,
    offset: u64,
    flags: u64,
    prot: u64,
) -> Result<Source, u16> {
    let f = u64::try_from(fd).ok().and_then(|fd| call.lx.fd(fd)).ok_or(EBADF)?;
    let file = match &f.object {
        Object::File(file) if file.path_only => return Err(EBADF),
        Object::File(file) if !file.dir => file.clone(),
        Object::Device(d) if d.kind == super::file::DeviceKind::Zero => {
            return Ok(Source::Anonymous);
        }
        _ => {
            ledger::record("mmap", "linux option missing", fd as u64);
            return Err(ENODEV);
        }
    };
    let shared = flags & MAP_TYPE != MAP_PRIVATE;
    if !file.readable
        || shared
            && prot & PROT_WRITE != 0
            && (!file.writable || file.append.load(core::sync::atomic::Ordering::Relaxed))
    {
        return Err(EACCES);
    }
    let node = file.node;
    let (size, seals) =
        crate::nt::file::with_fs(|fs| fs.node(node).map_or((0, None), |n| (n.size(), n.seals())));
    // A file sealed against writing (`lx::memfd`) maps shared only
    // read-only, and its section cannot be made writable (mprotect:
    // EACCES).
    let sealed = seals.unwrap_or(0) & (SEAL_WRITE | SEAL_FUTURE_WRITE) != 0;
    if sealed && shared && prot & PROT_WRITE != 0 {
        return Err(EPERM);
    }
    let writable = file.writable && !sealed;
    let section = match Section::new_linux_file(file.clone(), node, size, shared, writable) {
        Ok(s) => s,
        Err(NtStatus::NO_MEMORY) => return Err(ENOMEM),
        // Sealed between the look above and the mapping.
        Err(NtStatus::ACCESS_DENIED) => return Err(EPERM),
        Err(_) => return Err(ENODEV),
    };
    Ok(Source::File(section, offset))
}

/// mmap(2): anonymous memory, or a file's pages from `offset` (see the
/// module's doc). Returns the address or an errno. MAP_DENYWRITE,
/// MAP_STACK, MAP_NORESERVE and the other hints are taken and ignored,
/// as Linux ignores most of them.
pub fn mmap(
    call: &Call<'_>,
    addr: u64,
    len: u64,
    prot: u64,
    flags: u64,
    fd: i64,
    offset: u64,
) -> Result<u64, u16> {
    let process = &call.process;
    if len == 0 || !offset.is_multiple_of(PAGE) {
        return Err(EINVAL);
    }
    match flags & MAP_TYPE {
        MAP_SHARED | MAP_PRIVATE | MAP_SHARED_VALIDATE => {}
        _ => return Err(EINVAL),
    }
    if prot & !(PROT_READ | PROT_WRITE | PROT_EXEC) != 0 {
        return Err(EINVAL);
    }
    let len = round_up(len).filter(|&l| l <= TASK_SIZE).ok_or(ENOMEM)?;
    let fixed = flags & (MAP_FIXED | MAP_FIXED_NOREPLACE) != 0;
    if fixed && !addr.is_multiple_of(PAGE) {
        return Err(EINVAL);
    }
    let mut source = if flags & MAP_ANONYMOUS != 0 {
        Source::Anonymous
    } else {
        if offset.checked_add(len).is_none() {
            return Err(EOVERFLOW);
        }
        file_source(call, fd, offset, flags, prot)?
    };
    if matches!(source, Source::Anonymous) && flags & MAP_TYPE != MAP_PRIVATE {
        // MAP_SHARED anonymous memory (or /dev/zero's): one memory for
        // every process mapping it (a fork's child), pages first touched
        // after the fork included: a pagefile-backed section of its own.
        let section =
            Section::new(len, vad::PAGE_EXECUTE_READWRITE, crate::nt::section::SEC_COMMIT)
                .map_err(|_| ENOMEM)?;
        source = Source::File(section, 0);
    }
    // The section's object before the locks (its making allocates).
    let (protect, kind, view) = match source {
        Source::Anonymous => (protect_of(prot), vad::MEM_PRIVATE, None),
        Source::File(section, offset) => {
            let protect = view_protect(prot, &section);
            let object = KObject::new(ObjectBody::Section(section)).map_err(|_| ENOMEM)?;
            (protect, vad::MEM_MAPPED, Some((object, offset)))
        }
    };
    let mut removed = Vec::new();
    let mapped = {
        let mut views = process.views.lock();
        views.try_reserve(1).map_err(|_| ENOMEM)?;
        let mut vad = process.vad.lock();
        let start = if fixed {
            let end = addr.checked_add(len).filter(|&e| e <= vad::USER_MAX).ok_or(ENOMEM)?;
            if addr < vad::USER_MIN {
                return Err(ENOMEM);
            }
            if !vad.is_range_free(addr, end) {
                if flags & MAP_FIXED_NOREPLACE != 0 {
                    return Err(EEXIST);
                }
                // MAP_FIXED replaces what was there.
                unmap_locked(process, &mut views, &mut vad, addr, end, &mut removed)?;
            }
            addr
        } else {
            let hint = addr / PAGE * PAGE;
            let usable = hint >= vad::USER_MIN
                && hint
                    .checked_add(len)
                    .is_some_and(|e| e <= vad::USER_MAX && vad.is_range_free(hint, e));
            if usable { hint } else { vad.find_free_below(len, MMAP_TOP).ok_or(ENOMEM)? }
        };
        let done = vad
            .map_exact(start, start + len, protect, kind)
            .map_err(|e| if e == NtStatus::NO_MEMORY { ENOMEM } else { EINVAL });
        if done.is_ok() {
            if let Some((section, offset)) = view {
                // Room reserved above.
                let _ = views.try_push(View::new(start, len, offset, section));
            }
            sync(process, &vad, start, start + len);
        }
        done.map(|()| start)
    };
    drop(removed);
    mapped
}

/// Maps `len` bytes of the section `object` (from its start) shared
/// into `process` with `prot`: at `at` (page-aligned; EINVAL if anything
/// is mapped there, as shmat(2) without SHM_REMAP), else where there is
/// room below the mmap top. The address, or an errno. A System V
/// segment's attachment (`lx::shm`).
pub fn attach(
    process: &Process,
    at: Option<u64>,
    len: u64,
    prot: u64,
    object: crate::nt::object::ObjectRef,
) -> Result<u64, u16> {
    let len = round_up(len).filter(|&l| l > 0 && l <= TASK_SIZE).ok_or(EINVAL)?;
    let protect = match &object.body {
        ObjectBody::Section(section) => view_protect(prot, section),
        _ => return Err(EINVAL),
    };
    let mut views = process.views.lock();
    views.try_reserve(1).map_err(|_| ENOMEM)?;
    let mut vad = process.vad.lock();
    let start = match at {
        Some(addr) => {
            let end = addr.checked_add(len).filter(|&e| e <= vad::USER_MAX).ok_or(EINVAL)?;
            if addr < vad::USER_MIN || !vad.is_range_free(addr, end) {
                return Err(EINVAL);
            }
            addr
        }
        None => vad.find_free_below(len, MMAP_TOP).ok_or(ENOMEM)?,
    };
    vad.map_exact(start, start + len, protect, vad::MEM_MAPPED)
        .map_err(|e| if e == NtStatus::NO_MEMORY { ENOMEM } else { EINVAL })?;
    // Room reserved above.
    let _ = views.try_push(View::new(start, len, 0, object));
    sync(process, &vad, start, start + len);
    Ok(start)
}

/// How many views of `process` map the section `object`.
pub fn count_views(process: &Process, object: &crate::nt::object::ObjectRef) -> u64 {
    let views = process.views.lock();
    views.iter().filter(|v| crate::mm::heap::KArc::ptr_eq(&v.section, object)).count() as u64
}

/// The view of `process` that starts at `addr`: its section, size and
/// offset.
pub fn view_at(process: &Process, addr: u64) -> Option<(crate::nt::object::ObjectRef, u64, u64)> {
    let views = process.views.lock();
    views.iter().find(|v| v.base == addr).map(|v| (v.section.clone(), v.size, v.offset))
}

/// munmap(2).
pub fn munmap(process: &Process, addr: u64, len: u64) -> Result<u64, u16> {
    if !addr.is_multiple_of(PAGE) || len == 0 {
        return Err(EINVAL);
    }
    let end = round_up(len).and_then(|l| addr.checked_add(l)).ok_or(EINVAL)?;
    if end > TASK_SIZE {
        return Err(EINVAL);
    }
    let mut removed = Vec::new();
    {
        let mut views = process.views.lock();
        let mut vad = process.vad.lock();
        let end = end.min(vad::USER_MAX);
        if addr < end {
            unmap_locked(process, &mut views, &mut vad, addr, end, &mut removed)?;
        }
    }
    drop(removed);
    Ok(0)
}

/// Unmaps the whole user address space, file mappings too (execve).
pub fn clear(process: &Process) -> Result<(), NtStatus> {
    let removed = {
        let mut views = process.views.lock();
        let mut vad = process.vad.lock();
        vad.unmap_range(vad::USER_MIN, vad::USER_MAX)?;
        sync(process, &vad, vad::USER_MIN, vad::USER_MAX);
        core::mem::take(&mut *views)
    };
    drop(removed);
    Ok(())
}

/// The protection `prot` gives the pages of `[s, e)`, one piece of a
/// mapping: a file mapping's per its section (EACCES beyond what its
/// file was opened for: mmap(2)), anonymous memory's directly. A file
/// mapping's piece may span several views (of one or more sections).
fn piece_protect(views: &[View], vad: &Vad, s: u64, e: u64, prot: u64) -> Result<Option<u32>, u16> {
    if vad.query(s).map(|q| q.kind) != Ok(vad::MEM_MAPPED) {
        return Ok(Some(protect_of(prot)));
    }
    let mut protect = None;
    for v in views.iter().filter(|v| v.base < e && s < v.base + v.size) {
        let section = v.section();
        let p = view_protect(prot, section);
        section.allows(p).map_err(|_| EACCES)?;
        if protect.is_some_and(|q| q != p) {
            // Views of differently shared sections in one piece: set
            // view by view.
            return Ok(None);
        }
        protect = Some(p);
    }
    Ok(protect.or(Some(protect_of(prot))))
}

/// mprotect(2): every page of the range must be mapped (ENOMEM); a file
/// mapping's pages get copy-on-write for PROT_WRITE unless shared, and
/// PROT_WRITE on a shared mapping of a file not open for writing is
/// EACCES.
pub fn mprotect(process: &Process, addr: u64, len: u64, prot: u64) -> Result<u64, u16> {
    if !addr.is_multiple_of(PAGE) || prot & !(PROT_READ | PROT_WRITE | PROT_EXEC) != 0 {
        return Err(EINVAL);
    }
    let end = round_up(len).and_then(|l| addr.checked_add(l)).ok_or(ENOMEM)?;
    if end > TASK_SIZE {
        return Err(ENOMEM);
    }
    if addr == end {
        return Ok(0);
    }
    let views = process.views.lock();
    let mut vad = process.vad.lock();
    if !vad.all_committed(addr, end) {
        return Err(ENOMEM);
    }
    let pieces = vad.pieces(addr, end).map_err(|_| ENOMEM)?;
    // Checked whole before anything changes.
    let mut plan: Vec<(u64, u64, u32)> = Vec::new();
    for &(s, e) in &pieces {
        match piece_protect(&views, &vad, s, e, prot)? {
            Some(p) => plan.try_push((s, e, p)).map_err(|_| ENOMEM)?,
            None => {
                for v in views.iter().filter(|v| v.base < e && s < v.base + v.size) {
                    let (vs, ve) = (v.base.max(s), (v.base + v.size).min(e));
                    let p = view_protect(prot, v.section());
                    plan.try_push((vs, ve, p)).map_err(|_| ENOMEM)?;
                }
            }
        }
    }
    for (s, e, p) in plan {
        vad.protect(s, e - s, p).map_err(|_| ENOMEM)?;
    }
    sync(process, &vad, addr, end);
    Ok(0)
}

/// One run of pages of a mapping: where, its protection and kind.
#[derive(Clone, Copy)]
struct Run {
    start: u64,
    end: u64,
    protect: u32,
    kind: u32,
}

/// The runs of `[start, end)`, every page of which must be mapped
/// (EFAULT otherwise: mremap(2)).
fn runs(vad: &Vad, start: u64, end: u64) -> Result<Vec<Run>, u16> {
    let mut out = Vec::new();
    let mut at = start;
    while at < end {
        let q = vad.query(at).map_err(|_| EFAULT)?;
        if q.state != vad::MEM_COMMIT {
            return Err(EFAULT);
        }
        let run_end = (q.base + q.size).min(end);
        out.try_push(Run { start: at, end: run_end, protect: q.protect, kind: q.kind })
            .map_err(|_| ENOMEM)?;
        at = run_end;
    }
    Ok(out)
}

/// mremap(2): shrinks a mapping in place (the tail unmapped), grows it in
/// place when the pages after it are free, else (MREMAP_MAYMOVE) moves
/// it whole to a free range (or to `new_addr` with MREMAP_FIXED, which
/// replaces what is there): its pages move with their frames, not copied
/// (`UserSpace::move_page`), its file views with them. The range must be
/// mapped (EFAULT). MREMAP_DONTUNMAP is not supported (EINVAL, ledgered),
/// nor is growing a range of a shared anonymous mapping by its old size
/// 0 (a duplicate of it: EINVAL).
pub fn mremap(
    process: &Process,
    old: u64,
    old_len: u64,
    new_len: u64,
    flags: u64,
    new_addr: u64,
) -> Result<u64, u16> {
    if flags & !(MREMAP_MAYMOVE | MREMAP_FIXED | MREMAP_DONTUNMAP) != 0
        || !old.is_multiple_of(PAGE)
        || flags & MREMAP_FIXED != 0 && flags & MREMAP_MAYMOVE == 0
    {
        return Err(EINVAL);
    }
    if flags & MREMAP_DONTUNMAP != 0 || old_len == 0 {
        ledger::record("mremap", "linux option missing", flags);
        return Err(EINVAL);
    }
    let old_len = round_up(old_len).ok_or(EINVAL)?;
    let new_len = round_up(new_len).filter(|&l| l > 0 && l <= TASK_SIZE).ok_or(EINVAL)?;
    let old_end = old.checked_add(old_len).filter(|&e| e <= TASK_SIZE).ok_or(EFAULT)?;
    let fixed = flags & MREMAP_FIXED != 0;
    if fixed {
        let new_end = new_addr.checked_add(new_len).ok_or(EINVAL)?;
        if !new_addr.is_multiple_of(PAGE)
            || new_end > vad::USER_MAX
            || new_addr < vad::USER_MIN
            || new_addr < old_end && old < new_end
        {
            return Err(EINVAL);
        }
    }
    let mut removed = Vec::new();
    let moved = {
        let mut views = process.views.lock();
        let mut vad = process.vad.lock();
        let runs = runs(&vad, old, old_end)?;
        if !fixed && new_len <= old_len {
            if new_len < old_len {
                unmap_locked(process, &mut views, &mut vad, old + new_len, old_end, &mut removed)?;
            }
            Ok(old)
        } else if !fixed && vad.is_range_free(old_end, old + new_len) {
            grow(process, &mut views, &mut vad, &runs, old_end, old + new_len).map(|()| old)
        } else if flags & MREMAP_MAYMOVE == 0 {
            Err(ENOMEM)
        } else {
            let to = if fixed {
                unmap_locked(
                    process,
                    &mut views,
                    &mut vad,
                    new_addr,
                    new_addr + new_len,
                    &mut removed,
                )?;
                new_addr
            } else {
                vad.find_free_below(new_len, MMAP_TOP).ok_or(ENOMEM)?
            };
            move_to(process, &mut views, &mut vad, &runs, old, to, &mut removed)?;
            let keep = old_len.min(new_len);
            if new_len < old_len {
                unmap_locked(process, &mut views, &mut vad, to + keep, to + old_len, &mut removed)?;
            } else if new_len > old_len {
                grow(process, &mut views, &mut vad, &runs, to + old_len, to + new_len)?;
            }
            Ok(to)
        }
    };
    drop(removed);
    moved
}

/// Grows the mapping whose runs are `runs` (now ending at `end`) into the free range up to `new_end`, as its last page is
/// (anonymous memory, or the next pages of its file's view).
fn grow(
    process: &Process,
    views: &mut Vec<View>,
    vad: &mut Vad,
    runs: &[Run],
    end: u64,
    new_end: u64,
) -> Result<(), u16> {
    let last = runs.last().ok_or(EFAULT)?;
    if last.kind == vad::MEM_MAPPED {
        // The view of the old range's last page, now at `end - PAGE`.
        let v = views.iter().find(|v| (v.base..v.base + v.size).contains(&(end - PAGE)));
        let Some(v) = v else { return Err(EFAULT) };
        let view = View::new(end, new_end - end, v.offset + (end - v.base), v.section.clone());
        views.try_reserve(1).map_err(|_| ENOMEM)?;
        vad.map_exact(end, new_end, last.protect, last.kind).map_err(|_| ENOMEM)?;
        let _ = views.try_push(view);
    } else {
        vad.map_exact(end, new_end, last.protect, last.kind).map_err(|_| ENOMEM)?;
    }
    sync(process, vad, end, new_end);
    Ok(())
}

/// Moves the mapping whose runs are `runs` from `old` to the free range
/// at `to`: its VAD runs, its views and its pages (frames and all).
fn move_to(
    process: &Process,
    views: &mut Vec<View>,
    vad: &mut Vad,
    runs: &[Run],
    old: u64,
    to: u64,
    removed: &mut Vec<View>,
) -> Result<(), u16> {
    let end = runs.last().map_or(old, |r| r.end);
    let shift = |a: u64| a - old + to;
    // The views of the range, cut at its ends, then moved with it.
    let mut inside = Vec::new();
    cut(views, old, end, &mut inside)?;
    let mut moved = Vec::new();
    moved.try_reserve(inside.len()).map_err(|_| ENOMEM)?;
    views.try_reserve(inside.len()).map_err(|_| ENOMEM)?;
    removed.try_reserve(inside.len()).map_err(|_| ENOMEM)?;
    for r in runs {
        if vad.map_exact(shift(r.start), shift(r.end), r.protect, r.kind).is_err() {
            let _ = vad.unmap_range(to, shift(r.start));
            return Err(ENOMEM);
        }
    }
    // The cut put the parts outside back in `views`; what was inside
    // (clipped to the range) moves.
    for v in inside {
        let (s, e) = (v.base.max(old), (v.base + v.size).min(end));
        if s < e {
            let _ = moved.try_push(View::new(
                shift(s),
                e - s,
                v.offset + (s - v.base),
                v.section.clone(),
            ));
        }
        let _ = removed.try_push(v);
    }
    for v in moved {
        let _ = views.try_push(v);
    }
    {
        let mut space = process.space.lock();
        let mut next = space.next_mapped(old, end);
        while let Some(page) = next {
            if space.move_page(page, shift(page)).is_err() {
                // No page table at the new place: the page stays and is
                // freed with the old range below; its contents are lost
                // only if this happens, which is logged.
                crate::warn!(
                    "lx: pid {} mremap: no memory to move the page at {page:#x}",
                    process.id
                );
                ledger::record("mremap", "out of memory", page);
            }
            next = space.next_mapped(page + PAGE, end);
        }
    }
    vad.unmap_range(old, end).map_err(|_| ENOMEM)?;
    sync(process, vad, old, end);
    sync(process, vad, to, shift(end));
    Ok(())
}

/// madvise(2): MADV_DONTNEED drops the pages of the range (anonymous
/// ones read as zero again, a private file mapping's show the file
/// again); the hints (NORMAL, RANDOM, SEQUENTIAL, WILLNEED, FREE: Linux
/// may keep the pages, MERGEABLE, UNMERGEABLE, HUGEPAGE, NOHUGEPAGE,
/// DONTDUMP, DODUMP) change nothing here. Other advice is EINVAL,
/// ledgered. ENOMEM if the range is not all mapped.
pub fn madvise(process: &Process, addr: u64, len: u64, advice: u64) -> Result<u64, u16> {
    if !addr.is_multiple_of(PAGE) {
        return Err(EINVAL);
    }
    let end = round_up(len).and_then(|l| addr.checked_add(l)).ok_or(EINVAL)?;
    match advice {
        MADV_DONTNEED => {}
        MADV_NORMAL | MADV_RANDOM | MADV_SEQUENTIAL | MADV_WILLNEED | MADV_FREE
        | MADV_MERGEABLE | MADV_UNMERGEABLE | MADV_HUGEPAGE | MADV_NOHUGEPAGE | MADV_DONTDUMP
        | MADV_DODUMP => {
            let vad = process.vad.lock();
            return if addr == end || vad.all_committed(addr, end) { Ok(0) } else { Err(ENOMEM) };
        }
        _ => {
            ledger::record("madvise", "linux option missing", advice);
            return Err(EINVAL);
        }
    }
    if addr == end {
        return Ok(0);
    }
    let views = process.views.lock();
    let mut vad = process.vad.lock();
    if !vad.all_committed(addr, end) {
        return Err(ENOMEM);
    }
    // A private file page copied on write goes back to copy-on-write.
    let pieces = vad.pieces(addr, end).map_err(|_| ENOMEM)?;
    for (s, e) in pieces {
        if vad.query(s).map(|q| q.kind) != Ok(vad::MEM_MAPPED) {
            continue;
        }
        let mut at = s;
        while at < e {
            let q = vad.query(at).map_err(|_| ENOMEM)?;
            let run_end = (q.base + q.size).min(e);
            let back = match q.protect & 0xff {
                vad::PAGE_READWRITE => Some(vad::PAGE_WRITECOPY),
                vad::PAGE_EXECUTE_READWRITE => Some(vad::PAGE_EXECUTE_WRITECOPY),
                _ => None,
            };
            if let Some(p) = back
                && !views
                    .iter()
                    .filter(|v| v.base < run_end && at < v.base + v.size)
                    .any(|v| v.section().writes_through())
            {
                vad.protect(at, run_end - at, (q.protect & !0xff) | p).map_err(|_| ENOMEM)?;
            }
            at = run_end;
        }
    }
    // The frames go (a section's stay with it); the next touch maps a
    // zero page or the file's page again.
    let mut space = process.space.lock();
    let mut next = space.next_mapped(addr, end);
    while let Some(page) = next {
        space.unmap_page(page);
        next = space.next_mapped(page + PAGE, end);
    }
    drop(space);
    charge::sync(process, vad.private_commit());
    Ok(0)
}

/// brk(2): returns the new break, or the current one when the request is
/// refused (below the start, past the address space, onto a mapping, or
/// out of memory); brk(0) asks for it.
pub fn brk(process: &Process, brk: &crate::sync::IrqMutex<super::state::Brk>, addr: u64) -> u64 {
    let mut b = brk.lock();
    let current = b.current;
    if addr < b.start || addr > MMAP_TOP {
        return current;
    }
    let (Some(old_end), Some(new_end)) = (round_up(current), round_up(addr)) else {
        return current;
    };
    let mut vad = process.vad.lock();
    if new_end > old_end {
        if !vad.is_range_free(old_end, new_end)
            || vad.map_exact(old_end, new_end, vad::PAGE_READWRITE, vad::MEM_PRIVATE).is_err()
        {
            return current;
        }
        sync(process, &vad, old_end, new_end);
    } else if new_end < old_end {
        if vad.unmap_range(new_end, old_end).is_err() {
            return current;
        }
        sync(process, &vad, new_end, old_end);
    }
    b.current = addr;
    addr
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_protections_become_nt_ones() {
        assert_eq!(protect_of(0), vad::PAGE_NOACCESS);
        assert_eq!(protect_of(PROT_READ), vad::PAGE_READONLY);
        assert_eq!(protect_of(PROT_WRITE), vad::PAGE_READWRITE);
        assert_eq!(protect_of(PROT_READ | PROT_WRITE), vad::PAGE_READWRITE);
        assert_eq!(protect_of(PROT_EXEC), vad::PAGE_EXECUTE_READ);
        assert_eq!(protect_of(PROT_READ | PROT_EXEC), vad::PAGE_EXECUTE_READ);
        assert_eq!(protect_of(7), vad::PAGE_EXECUTE_READWRITE);
    }

    fn view(base: u64, pages: u64, offset: u64) -> View {
        let section = Section::new(64 * PAGE, vad::PAGE_EXECUTE_WRITECOPY, 0).unwrap();
        let section = KObject::new(ObjectBody::Section(section)).unwrap();
        View::new(base, pages * PAGE, offset, section)
    }

    fn shape(views: &[View]) -> Vec<(u64, u64, u64)> {
        let mut v: Vec<_> = views.iter().map(|v| (v.base, v.size / PAGE, v.offset)).collect();
        v.sort();
        v
    }

    /// munmap, MAP_FIXED and mremap cut views page by page: the parts
    /// outside the range stay, each at its offset in the section.
    #[test]
    fn cutting_views_keeps_the_parts_outside() {
        const B: u64 = 0x10_0000;
        let mut views = alloc::vec![view(B, 4, 0), view(B + 8 * PAGE, 2, 3 * PAGE)];
        let mut removed = Vec::new();
        // The middle of the first: two pieces, the second at its offset.
        cut(&mut views, B + PAGE, B + 2 * PAGE, &mut removed).unwrap();
        assert_eq!(
            shape(&views),
            [(B, 1, 0), (B + 2 * PAGE, 2, 2 * PAGE), (B + 8 * PAGE, 2, 3 * PAGE)]
        );
        assert_eq!(removed.len(), 1);
        // Across two views: the tail of one and the head of the next.
        cut(&mut views, B + 3 * PAGE, B + 9 * PAGE, &mut removed).unwrap();
        assert_eq!(
            shape(&views),
            [(B, 1, 0), (B + 2 * PAGE, 1, 2 * PAGE), (B + 9 * PAGE, 1, 4 * PAGE)]
        );
        assert_eq!(removed.len(), 3);
        // A range touching none: nothing.
        cut(&mut views, B + 20 * PAGE, B + 30 * PAGE, &mut removed).unwrap();
        assert_eq!(views.len(), 3);
        // A whole view.
        cut(&mut views, B, B + PAGE, &mut removed).unwrap();
        assert_eq!(shape(&views), [(B + 2 * PAGE, 1, 2 * PAGE), (B + 9 * PAGE, 1, 4 * PAGE)]);
    }

    #[test]
    fn the_layout_leaves_room() {
        const { assert!(MMAP_TOP < STACK_END - STACK_SIZE) };
        const { assert!(STACK_END <= vad::USER_MAX && STACK_END.is_multiple_of(PAGE)) };
        // A static-pie's base and mmap_test's 8 GiB hint are below it.
        const { assert!(super::super::elf::DEFAULT_DYN_BASE < MMAP_TOP) };
    }
}
