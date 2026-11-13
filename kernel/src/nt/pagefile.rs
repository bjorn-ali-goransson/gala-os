//! The page file (ARCHITECTURE "The page file"): private memory of NT
//! processes leaves memory for `\pagefile.sys` on C:'s data area when
//! memory is short, and comes back at its next touch.
//!
//! - **The file**: made at mount (`create`, `persist::mount`, before the
//!   file system records changes), its clusters taken outside every file
//!   and never recorded (`RamFs::claim_clusters`: free again at the next
//!   mount, as Windows' page file holds nothing across a boot); a node
//!   `\pagefile.sys` (hidden, system) of its size, held open by the system
//!   without sharing, so a program's open of its data is a sharing
//!   violation, as on Windows. It grows (`grow`) when a commit would pass
//!   the commit limit, up to its maximum.
//! - **The commit limit** is memory plus the page file (`charge::limit`).
//! - **Paging out** (`page_out`, from `persist::evict` once the file
//!   cache's pages and image pages could not bring free memory back to
//!   the mark): a clock over NT processes' page tables. A present page of
//!   the process's own (not borrowed from a section, not shared by a fork,
//!   not a TEB) accessed since the hand last passed loses its accessed
//!   bit; one that was not loses its dirty bit and is a candidate. A batch
//!   of candidates gets slots, is copied (after the flush that made the
//!   dirty bits visible), written to the disk with no lock held, and then,
//!   under the process's views, VAD and space locks, each page whose leaf
//!   still maps the same frame and was not written since (its dirty bit;
//!   the kernel's own writes set it, `UserSpace::copy_in`) becomes a
//!   *token* (a not-present leaf naming its slot) and its frame is freed.
//!   A page written meanwhile stays, its slot goes back.
//! - **Paging in** (`page_in`, from `vm::resolve_fault` when the leaf holds
//!   a token): the slot is read with no lock held, then, under the locks,
//!   mapped with the VAD's protection if the leaf still holds that token
//!   (a slot's generation tells a later use apart), and the slot is given
//!   back. A read that fails raises STATUS_IN_PAGE_ERROR in the program.
//! - **Failures**: a write that fails leaves the batch in memory (the
//!   slots back; logged, a `page out failed` event); no page file (no disk,
//!   no data area, no room): the commit limit is memory, as before.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::platform::{self, PAGE_SIZE};
use super::process::Process;
use super::status::NtStatus;
use super::vad::PageState;
use super::vm::{self, Fault};
use crate::fs::ramfs::{FsError, NodeId, ROOT, RamFs};
use crate::mm::heap::TryVec;
use crate::mm::paging::Age;
use crate::mm::swap;
use crate::sync::IrqMutex;

use alloc::vec::Vec;

const PAGE: usize = PAGE_SIZE as usize;
/// Pages written per batch (one disk request when their slots are
/// consecutive): the reserved staging buffer's size.
const BATCH: usize = 16;
/// The most pages one `page_out` call frees (a fault waits for it).
const PER_CALL: usize = 1024;
/// The most leaves one `page_out` call looks at.
const SCAN_BUDGET: usize = 16384;
/// A page file smaller than this is not made (pages).
const MIN_PAGES: u32 = 4096;
/// The least a growth adds (pages): 64 MiB.
const GROW_STEP: u32 = 16384;
/// The most TEB pages kept per process by a page out's look.
const MAX_TEBS: usize = 512;

/// Where the page file is.
struct State {
    /// (first slot, first cluster, count): the slots' clusters, in slot
    /// order.
    runs: Vec<(u32, u32, u32)>,
    /// Its size and its maximum, in pages.
    pages: u32,
    max: u32,
    node: NodeId,
}

static STATE: IrqMutex<Option<State>> = IrqMutex::new(None);
/// Its size in pages (0: no page file), read lock-free by the commit
/// limit.
static SIZE: AtomicU64 = AtomicU64::new(0);
/// The node `\pagefile.sys` + 1 (0: none).
static NODE: AtomicU64 = AtomicU64::new(0);

/// One page out at a time; one page in reads at a time (its buffer).
static OUT_TURN: AtomicBool = AtomicBool::new(false);
static IN_TURN: AtomicBool = AtomicBool::new(false);
static GROW_TURN: AtomicBool = AtomicBool::new(false);
/// A kernel handover is carrying the programs: nothing pages out.
static HELD: AtomicBool = AtomicBool::new(false);
/// The buffers, reserved when the page file is made (a page out runs
/// when memory is short and must not need any).
static STAGING: IrqMutex<Option<Vec<u8>>> = IrqMutex::new(None);
static IN_BUFFER: IrqMutex<Option<Vec<u8>>> = IrqMutex::new(None);
/// The clock's hand: (process id, address).
static HAND: IrqMutex<(u64, u64)> = IrqMutex::new((0, 0));

/// Counters (the `pagefile:` lines).
static OUT_PAGES: AtomicU64 = AtomicU64::new(0);
static OUT_WRITES: AtomicU64 = AtomicU64::new(0);
static OUT_KEPT: AtomicU64 = AtomicU64::new(0);
static IN_PAGES: AtomicU64 = AtomicU64::new(0);
static OUT_FAILED: AtomicU64 = AtomicU64::new(0);
static IN_FAILED: AtomicU64 = AtomicU64::new(0);
static GROWN: AtomicU64 = AtomicU64::new(0);
/// The page file was full (logged once until slots come free).
static FULL: AtomicBool = AtomicBool::new(false);

/// Its NT name, as SystemPageFileInformation reports it.
pub const NT_NAME: [u16; 19] = {
    let s = b"\\??\\C:\\pagefile.sys";
    let mut out = [0u16; 19];
    let mut i = 0;
    while i < 19 {
        out[i] = s[i] as u16;
        i += 1;
    }
    out
};

/// Its size in pages (0: none): what the commit limit adds to memory.
pub fn pages() -> u64 {
    SIZE.load(Ordering::Relaxed)
}

/// Whether `node` is `\pagefile.sys`.
pub fn is_page_file(node: NodeId) -> bool {
    NODE.load(Ordering::Relaxed) == node as u64 + 1
}

/// Its size: memory, at most a quarter of the free data area; its
/// maximum three times memory (at least 4 GiB, as Windows' system-managed
/// page file), at most half the free data area. (initial, max) in pages.
pub fn sizes(memory_pages: u64, free_clusters: u32) -> (u32, u32) {
    let free = u64::from(free_clusters);
    let initial = memory_pages.min(free / 4) / 256 * 256;
    let max = (3 * memory_pages).max(1 << 20).min(free / 2).max(initial);
    (initial as u32, max as u32)
}

/// What `create` did (logged by `report`, outside the file system's lock:
/// the console's lock is never taken inside it).
pub enum Created {
    /// Too little room: the free data area, in clusters.
    None(u32),
    /// Made: its size and maximum (pages), its runs of clusters.
    Made(u32, u32, usize),
    Failed(FsError),
}

/// Makes the page file at mount (`persist::mount`, with the data area
/// attached and nothing recorded yet); none when the data area is too
/// small.
pub fn create(fs: &mut RamFs) -> Option<Created> {
    let (_, free) = fs.area_size()?;
    let (_, memory) = platform::frame_stats();
    let (initial, max) = sizes(memory, free);
    if initial < MIN_PAGES {
        return Some(Created::None(free));
    }
    Some(match make(fs, initial, max) {
        Ok(runs) => Created::Made(initial, max, runs),
        Err(e) => Created::Failed(e),
    })
}

/// Logs what `create` did.
pub fn report(created: Created) {
    match created {
        Created::None(free) => crate::info!(
            "pagefile: none (the data area has {} MiB free; the commit limit is memory)",
            u64::from(free) * 4 / 1024
        ),
        Created::Made(initial, max, runs) => crate::info!(
            "pagefile: created \\pagefile.sys of {} MiB (up to {} MiB) in {runs} runs of \
             clusters; commit limit {} MiB",
            u64::from(initial) * 4 / 1024,
            u64::from(max) * 4 / 1024,
            super::charge::limit() * 4 / 1024
        ),
        Created::Failed(e) => {
            crate::warn!("pagefile: cannot make \\pagefile.sys ({e:?}); the commit limit is memory")
        }
    }
}

fn make(fs: &mut RamFs, pages: u32, max: u32) -> Result<usize, FsError> {
    let mut staging = Vec::new();
    staging.try_reserve_exact(BATCH * PAGE).map_err(|_| FsError::NoMemory)?;
    staging.resize(BATCH * PAGE, 0);
    let mut in_buffer = Vec::new();
    in_buffer.try_reserve_exact(PAGE).map_err(|_| FsError::NoMemory)?;
    in_buffer.resize(PAGE, 0);
    let slots = swap::Slots::new(pages).ok_or(FsError::NoMemory)?;
    let mut clusters = Vec::new();
    fs.claim_clusters(pages, &mut clusters)?;
    let mut runs = Vec::new();
    let undo = |fs: &mut RamFs, clusters: &[(u32, u32)]| fs.unclaim_clusters(clusters);
    let mut slot = 0;
    for &(c, n) in &clusters {
        if runs.try_push((slot, c, n)).is_err() {
            undo(fs, &clusters);
            return Err(FsError::NoMemory);
        }
        slot += n;
    }
    let name: Vec<u16> = match "pagefile.sys".encode_utf16().try_collect_vec() {
        Some(v) => v,
        None => {
            undo(fs, &clusters);
            return Err(FsError::NoMemory);
        }
    };
    let now = platform::system_time();
    let node = match fs.system_file(ROOT, &name, u64::from(pages) * PAGE_SIZE, now) {
        Ok(id) => id,
        Err(e) => {
            undo(fs, &clusters);
            return Err(e);
        }
    };
    let count = runs.len();
    *STATE.lock() = Some(State { runs, pages, max, node });
    *STAGING.lock() = Some(staging);
    *IN_BUFFER.lock() = Some(in_buffer);
    swap::install(slots);
    NODE.store(node as u64 + 1, Ordering::Relaxed);
    SIZE.store(u64::from(pages), Ordering::Relaxed);
    Ok(count)
}

/// Collects UTF-16 units fallibly.
trait TryCollect {
    fn try_collect_vec(self) -> Option<Vec<u16>>;
}

impl<I: Iterator<Item = u16>> TryCollect for I {
    fn try_collect_vec(self) -> Option<Vec<u16>> {
        let mut v = Vec::new();
        for u in self {
            v.try_push(u).ok()?;
        }
        Some(v)
    }
}

/// Grows the page file by at least `want` pages (64 MiB at least, up to
/// its maximum) when a commit would pass the limit (`vm::allocate`); with
/// no lock held. Whether it grew. Logged (`pagefile: grown`, `pagefile:
/// at its maximum`).
pub fn grow(want: u64) -> bool {
    let Some((pages, max)) = STATE.lock().as_ref().map(|s| (s.pages, s.max)) else {
        return false;
    };
    if GROW_TURN.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        // Another commit is growing it: theirs may be enough.
        while GROW_TURN.load(Ordering::Acquire) {
            platform::yield_now();
        }
        return SIZE.load(Ordering::Relaxed) > u64::from(pages);
    }
    let grown = grow_turn(want, pages, max);
    GROW_TURN.store(false, Ordering::Release);
    grown
}

fn grow_turn(want: u64, pages: u32, max: u32) -> bool {
    let room = max.saturating_sub(pages);
    let add = u32::try_from(want).unwrap_or(u32::MAX).max(GROW_STEP).min(room);
    if add == 0 || u64::from(add) < want {
        if !FULL.swap(true, Ordering::Relaxed) {
            warn!(
                "pagefile: at its maximum of {} MiB; commit {} of {} MiB, {} MiB more refused",
                u64::from(max) * 4 / 1024,
                super::charge::committed().0 * 4 / 1024,
                super::charge::limit() * 4 / 1024,
                want * 4 / 1024
            );
        }
        return false;
    }
    let mut clusters = Vec::new();
    let claimed = super::file::with_fs(|fs| fs.claim_clusters(add, &mut clusters));
    if let Err(e) = claimed {
        if !FULL.swap(true, Ordering::Relaxed) {
            warn!("pagefile: cannot grow by {} MiB ({e:?})", u64::from(add) * 4 / 1024);
        }
        return false;
    }
    let mut state = STATE.lock();
    let Some(s) = state.as_mut() else { return false };
    let reserved = s.runs.try_reserve(clusters.len()).is_ok();
    if !reserved || swap::with(|slots| slots.grow(pages + add)) != Some(true) {
        drop(state);
        super::file::with_fs(|fs| fs.unclaim_clusters(&clusters));
        warn!("pagefile: no memory to grow by {} MiB", u64::from(add) * 4 / 1024);
        return false;
    }
    let mut slot = s.pages;
    for &(c, n) in &clusters {
        let _ = s.runs.try_push((slot, c, n));
        slot += n;
    }
    s.pages += add;
    let (node, total) = (s.node, s.pages);
    drop(state);
    SIZE.store(u64::from(total), Ordering::Relaxed);
    super::file::with_fs(|fs| fs.set_system_file_size(node, u64::from(total) * PAGE_SIZE));
    GROWN.fetch_add(1, Ordering::Relaxed);
    info!(
        "pagefile: grown to {} MiB ({} MiB more); commit limit {} MiB",
        u64::from(total) * 4 / 1024,
        u64::from(add) * 4 / 1024,
        super::charge::limit() * 4 / 1024
    );
    true
}

/// The first sector of `slot`'s cluster.
fn sector_of(slot: u32) -> Option<u64> {
    let state = STATE.lock();
    let s = state.as_ref()?;
    let &(first, cluster, _) = s.runs.iter().find(|&&(f, _, n)| (f..f + n).contains(&slot))?;
    super::persist::cluster_sector(cluster + (slot - first))
}

/// A kernel handover is carrying the programs (`true`) or let them go:
/// meanwhile nothing pages out.
pub fn hold(held: bool) {
    HELD.store(held, Ordering::Release);
}

struct Turn(&'static AtomicBool);

impl Turn {
    fn try_take(flag: &'static AtomicBool) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Turn(flag))
    }

    fn take(flag: &'static AtomicBool) -> Self {
        loop {
            if let Some(t) = Self::try_take(flag) {
                return t;
            }
            platform::yield_now();
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// A page chosen to go: where, its frame, its slot's token.
#[derive(Clone, Copy)]
struct Candidate {
    virt: u64,
    frame: crate::mm::frame::Frame,
    token: u64,
}

/// Lets up to `want` pages of NT processes' private memory go to the page
/// file (see the module comment); with no lock held. The pages freed.
pub fn page_out(want: usize) -> usize {
    if SIZE.load(Ordering::Relaxed) == 0 || HELD.load(Ordering::Acquire) || want == 0 {
        return 0;
    }
    let Some(_turn) = Turn::try_take(&OUT_TURN) else { return 0 };
    let Some(mut staging) = STAGING.lock().take() else { return 0 };
    let want = want.min(PER_CALL);
    let mut freed = 0;
    let mut budget = SCAN_BUDGET;
    if let Ok(list) = super::process::running() {
        let (hand_pid, hand_virt) = *HAND.lock();
        let start = list.iter().position(|p| p.id == hand_pid).unwrap_or(0);
        let n = list.len();
        // Round the list once from the hand (its process again at the end
        // from its start: the hand's part before its address).
        for k in 0..=n {
            if freed >= want || budget == 0 || n == 0 {
                break;
            }
            let p = &list[(start + k) % n];
            if p.personality != crate::personality::Personality::Nt {
                continue;
            }
            let from = if k == 0 && p.id == hand_pid { hand_virt } else { 0 };
            match page_out_process(p, from, want - freed, &mut budget, &mut staging) {
                Ok((n, at)) => {
                    freed += n;
                    *HAND.lock() = (p.id, at);
                }
                Err(n) => {
                    freed += n;
                    break;
                }
            }
        }
    }
    *STAGING.lock() = Some(staging);
    freed
}

/// Pages out of `p` from `from`: Ok((pages freed, where the hand stops))
/// or Err(pages freed) when the round must stop (the page file full, a
/// failed write).
fn page_out_process(
    p: &Process,
    from: u64,
    want: usize,
    budget: &mut usize,
    staging: &mut [u8],
) -> Result<(usize, u64), usize> {
    use crate::mm::paging::USER_END;
    let mut tebs = Vec::new();
    let _ = tebs.try_reserve_exact(MAX_TEBS);
    super::thread::teb_pages(p, &mut tebs);
    let peb = super::layout::PEB_ADDRESS;
    let mut virt = from;
    let mut freed = 0;
    let mut batch =
        [Candidate { virt: 0, frame: crate::mm::frame::Frame::from_number(0), token: 0 }; BATCH];
    while freed < want && *budget > 0 && virt < USER_END {
        // The clock: ages pages until a batch of old ones is found.
        let mut n = 0;
        {
            let mut space = p.space.lock();
            let mut next = space.next_mapped(virt, USER_END);
            while let Some(at) = next {
                if *budget == 0 || n == BATCH {
                    break;
                }
                *budget -= 1;
                virt = at + PAGE_SIZE;
                next = space.next_mapped(virt, USER_END);
                if at == peb || tebs.contains(&at) {
                    continue;
                }
                if let Age::Old(frame) = space.age_page(at) {
                    batch[n] = Candidate { virt: at, frame, token: 0 };
                    n += 1;
                }
            }
            if next.is_none() && n < BATCH && *budget > 0 {
                virt = USER_END;
            }
        }
        // (The space's lock released: every CPU flushed the cleared bits.)
        if n == 0 {
            continue;
        }
        // Slots: consecutive where possible.
        let mut have = 0;
        while have < n {
            match swap::alloc() {
                Some(t) => {
                    batch[have].token = t;
                    have += 1;
                }
                None => break,
            }
        }
        if have < n {
            for c in &batch[..have] {
                swap::release(c.token);
            }
            if !FULL.swap(true, Ordering::Relaxed) {
                let (count, used, _) = swap::stats();
                warn!(
                    "pagefile: full ({used} of {count} slots in use): private pages stay in memory"
                );
            }
            return Err(freed);
        }
        // The copies, after the flush.
        let mut copied = [false; BATCH];
        {
            let mut space = p.space.lock();
            for (i, c) in batch[..n].iter().enumerate() {
                copied[i] =
                    space.copy_page(c.virt, c.frame, &mut staging[i * PAGE..(i + 1) * PAGE]);
            }
        }
        // To the disk, with no lock held.
        if let Err(at) = write_batch(&batch[..n], &copied, staging) {
            for c in &batch[..n] {
                swap::release(c.token);
            }
            OUT_FAILED.fetch_add(n as u64, Ordering::Relaxed);
            warn!(
                "pagefile: the disk failed a write of {n} pages at slot {at} (pid {} {}): they \
                 stay in memory, their slots free again",
                p.id, p.name
            );
            crate::events::emit(
                crate::events::Category::Fault,
                "page out failed",
                format_args!(r#""pid":{},"pages":{n},"slot":{at}"#, p.id),
            );
            return Err(freed);
        }
        // Out: each page not written since its copy.
        let mut out = 0;
        {
            let _views = p.views.lock();
            let _vad = p.vad.lock();
            let mut space = p.space.lock();
            let room = space.reserve_frees(n);
            for (i, c) in batch[..n].iter().enumerate() {
                if room && copied[i] && space.page_out(c.virt, c.frame, c.token) {
                    out += 1;
                } else {
                    swap::release(c.token);
                }
            }
        }
        OUT_WRITES.fetch_add(1, Ordering::Relaxed);
        OUT_PAGES.fetch_add(out as u64, Ordering::Relaxed);
        OUT_KEPT.fetch_add((n - out) as u64, Ordering::Relaxed);
        crate::events::emit(
            crate::events::Category::Fault,
            "page out",
            format_args!(
                r#""pid":{},"pages":{out},"kept":{},"slot":{}"#,
                p.id,
                n - out,
                swap::slot_of(batch[0].token)
            ),
        );
        freed += out;
    }
    Ok((freed, virt))
}

/// Writes the copied pages of a batch to their slots, consecutive slots
/// in one request. Err(slot) where the disk failed.
fn write_batch(batch: &[Candidate], copied: &[bool], staging: &[u8]) -> Result<(), u32> {
    let mut i = 0;
    while i < batch.len() {
        if !copied[i] {
            i += 1;
            continue;
        }
        let first = swap::slot_of(batch[i].token);
        let sector = sector_of(first).ok_or(first)?;
        let mut n = 1;
        while i + n < batch.len()
            && copied[i + n]
            && swap::slot_of(batch[i + n].token) == first + n as u32
            && sector_of(first + n as u32) == Some(sector + n as u64 * 8)
        {
            n += 1;
        }
        if crate::failpoint::hit(crate::failpoint::Point::PageOut)
            || platform::disk_write(sector, &staging[i * PAGE..(i + n) * PAGE]).is_err()
        {
            return Err(first);
        }
        i += n;
    }
    Ok(())
}

/// The fault at `addr` found its page in the page file: reads it back and
/// maps it (see the module comment); with no lock held. Resolved: retry
/// the fault (the page is mapped, or changed meanwhile); OutOfMemory: no
/// frame now; Raise(STATUS_IN_PAGE_ERROR): the disk failed the read.
pub fn page_in(process: &Process, addr: u64) -> Fault {
    let page = addr / PAGE_SIZE * PAGE_SIZE;
    let Some(token) = process.space.lock().swapped(page) else { return Fault::Resolved };
    let _turn = Turn::take(&IN_TURN);
    let Some(mut buf) = IN_BUFFER.lock().take() else { return Fault::OutOfMemory };
    let result = page_in_with(process, page, token, &mut buf);
    *IN_BUFFER.lock() = Some(buf);
    result
}

fn page_in_with(process: &Process, page: u64, token: u64, buf: &mut [u8]) -> Fault {
    let slot = swap::slot_of(token);
    let read = sector_of(slot).is_some_and(|sector| platform::disk_read(sector, buf).is_ok());
    if !read {
        let n = IN_FAILED.fetch_add(1, Ordering::Relaxed);
        if n < 8 {
            warn!(
                "pagefile: the disk failed a read of slot {slot} for pid {} ({}) at {page:#x}: \
                 STATUS_IN_PAGE_ERROR",
                process.id, process.name
            );
        }
        crate::events::emit(
            crate::events::Category::Fault,
            "page in failed",
            format_args!(r#""pid":{},"address":{page},"slot":{slot}"#, process.id),
        );
        return Fault::Raise(NtStatus::IN_PAGE_ERROR);
    }
    let _views = process.views.lock();
    let vad = process.vad.lock();
    let Some(PageState { committed: true, protect }) = vad.page(page) else {
        // Decommitted meanwhile (its slot went with it): the fault decides.
        return Fault::Resolved;
    };
    let mut space = process.space.lock();
    match space.page_in(page, token, buf, vm::flags_for(protect)) {
        Ok(true) => {
            swap::release(token);
            if swap::with(|s| s.used() < s.count()) == Some(true) {
                FULL.store(false, Ordering::Relaxed);
            }
            IN_PAGES.fetch_add(1, Ordering::Relaxed);
            crate::events::emit(
                crate::events::Category::Fault,
                "page in",
                format_args!(r#""pid":{},"address":{page},"slot":{slot}"#, process.id),
            );
            Fault::Resolved
        }
        Ok(false) => Fault::Resolved,
        Err(_) => Fault::OutOfMemory,
    }
}

/// Brings every page of `process` back from the page file (a kernel
/// handover carries pages in memory only). Err if one cannot come back.
pub fn bring_back(process: &Process) -> Result<(), NtStatus> {
    use crate::mm::paging::USER_END;
    if SIZE.load(Ordering::Relaxed) == 0 {
        return Ok(());
    }
    let mut virt = 0;
    loop {
        let found = {
            let mut space = process.space.lock();
            let mut next = space.next_mapped(virt, USER_END);
            loop {
                match next {
                    Some(at) if space.swapped(at).is_none() => {
                        next = space.next_mapped(at + PAGE_SIZE, USER_END);
                    }
                    other => break other,
                }
            }
        };
        let Some(at) = found else { return Ok(()) };
        let mut tries = 0;
        while process.space.lock().swapped(at).is_some() {
            match page_in(process, at) {
                Fault::Raise(s) => return Err(s),
                Fault::OutOfMemory if tries < 100 => {
                    super::section::trim(256);
                    platform::sleep_ms(1);
                    tries += 1;
                }
                Fault::OutOfMemory => return Err(NtStatus::NO_MEMORY),
                _ => {}
            }
        }
        virt = at + PAGE_SIZE;
    }
}

/// The `pagefile:` line (end of a run, a state dump, the low-memory
/// warning).
pub struct Summary;

impl core::fmt::Display for Summary {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let size = SIZE.load(Ordering::Relaxed);
        if size == 0 {
            return f.write_str("none (the commit limit is memory)");
        }
        let max = STATE.try_lock().and_then(|s| s.as_ref().map(|s| s.max)).unwrap_or(0);
        let (_, used, peak) = swap::stats();
        let (out, inn) = (OUT_PAGES.load(Ordering::Relaxed), IN_PAGES.load(Ordering::Relaxed));
        write!(
            f,
            "\\pagefile.sys {} MiB (at most {} MiB), {} MiB in use (most {} MiB); paged out {out} \
             pages ({} MiB) in {} writes, {} kept (written meanwhile); paged in {inn} pages ({} \
             MiB); {} pages failed to write, {} reads failed; grown {} times; {} stray tokens",
            size * 4 / 1024,
            u64::from(max) * 4 / 1024,
            u64::from(used) * 4 / 1024,
            u64::from(peak) * 4 / 1024,
            out * 4 / 1024,
            OUT_WRITES.load(Ordering::Relaxed),
            OUT_KEPT.load(Ordering::Relaxed),
            inn * 4 / 1024,
            OUT_FAILED.load(Ordering::Relaxed),
            IN_FAILED.load(Ordering::Relaxed),
            GROWN.load(Ordering::Relaxed),
            swap::STRAY.load(Ordering::Relaxed),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_file_is_memory_sized_and_leaves_room_on_the_disk() {
        // 512 MiB of memory, 15 GiB free: 512 MiB, up to 4 GiB.
        assert_eq!(sizes(131_072, 15 << 18), (131_072, 1 << 20));
        // A 1 GiB disk: a quarter of it at first, half at most.
        let free = 240 << 8;
        let (initial, max) = sizes(131_072, free);
        assert_eq!(initial, free / 4 / 256 * 256);
        assert_eq!(max, free / 2);
        // A small disk: under MIN_PAGES, none is made.
        assert!(sizes(131_072, 7 << 8).0 < MIN_PAGES);
    }
}
