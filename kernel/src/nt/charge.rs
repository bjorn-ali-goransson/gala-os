//! The commit charge (ROADMAP R6, ARCHITECTURE "Commit charge"): committed
//! private memory is charged to its process, to the process's job and to
//! the system, and a commit that would go over a limit fails with
//! STATUS_COMMITMENT_LIMIT instead of the machine running out of frames
//! later (when a fault on committed memory finds none, every program
//! suffers, not the one using too much).
//!
//! The VAD counts each process's committed MEM_PRIVATE pages exactly
//! (`Vad::private_commit`); `Process::commit` is what was charged for them.
//! `try_charge` checks and charges a new commit before the VAD takes it;
//! `sync` brings the charge back in line with the VAD after any change
//! (a failed commit, a decommit, a release, commits the kernel makes
//! itself for stacks and TEBs, which are charged without a limit); `ended`
//! gives a finished process's charge back.
//!
//! Limits: the system's commit limit is the machine's memory (no page
//! file), a job's per-process limit (JOB_OBJECT_LIMIT_PROCESS_MEMORY) and
//! its job-wide one (JOB_OBJECT_LIMIT_JOB_MEMORY). Lock order: the VAD,
//! then the process's job pointer, then the job's state.

use core::sync::atomic::{AtomicU64, Ordering};

use super::object::ObjectBody;
use super::platform;
use super::process::Process;
use super::status::NtStatus;

const PAGE: u64 = 4096;

/// Pages charged in the whole system, and the most ever.
static COMMITTED: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);

/// The system's commit limit, in pages.
pub fn limit() -> u64 {
    platform::frame_stats().1
}

/// Pages charged now, and the most ever.
pub fn committed() -> (u64, u64) {
    (COMMITTED.load(Ordering::Relaxed), PEAK.load(Ordering::Relaxed))
}

/// Charges `pages` more to `process`, its job and the system, or fails
/// with STATUS_COMMITMENT_LIMIT if that would exceed a limit. The caller
/// holds the process's VAD lock (one change of a process's charge at a
/// time).
pub fn try_charge(process: &Process, pages: u64) -> Result<(), NtStatus> {
    if pages == 0 || process.commit_ended.load(Ordering::Relaxed) {
        return Ok(());
    }
    let own = process.commit.load(Ordering::Relaxed) + pages;
    // The system first: one atomic step, so concurrent commits in
    // different processes cannot both slip under the limit.
    let limit = limit();
    COMMITTED
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
            (c + pages <= limit).then_some(c + pages)
        })
        .map_err(|_| NtStatus::COMMITMENT_LIMIT)?;
    if let Err(e) = charge_job(process, pages, own, true) {
        COMMITTED.fetch_sub(pages, Ordering::Relaxed);
        return Err(e);
    }
    PEAK.fetch_max(COMMITTED.load(Ordering::Relaxed), Ordering::Relaxed);
    process.commit.store(own, Ordering::Relaxed);
    Ok(())
}

/// Adds `pages` to the job's charge (checking its limits if `check`);
/// `own` is the process's charge with them.
fn charge_job(process: &Process, pages: u64, own: u64, check: bool) -> Result<(), NtStatus> {
    let job = process.job.lock().clone();
    let Some(job) = job else { return Ok(()) };
    let ObjectBody::Job(j) = &job.body else { return Ok(()) };
    let mut s = j.state.lock();
    if check {
        let over = |limit: Option<u64>, pages: u64| limit.is_some_and(|l| pages * PAGE > l);
        if over(s.process_memory_limit(), own) || over(s.job_memory_limit(), s.committed + pages) {
            return Err(NtStatus::COMMITMENT_LIMIT);
        }
    }
    s.committed += pages;
    s.peak_job = s.peak_job.max(s.committed);
    s.peak_process = s.peak_process.max(own);
    Ok(())
}

/// Takes `pages` off the job's charge.
fn uncharge_job(process: &Process, pages: u64) {
    let job = process.job.lock().clone();
    let Some(job) = job else { return };
    if let ObjectBody::Job(j) = &job.body {
        let mut s = j.state.lock();
        s.committed = s.committed.saturating_sub(pages);
    }
}

/// Brings `process`'s charge to `pages` (its VAD's private commit) with no
/// limit checked. The caller holds the VAD lock.
pub fn sync(process: &Process, pages: u64) {
    if process.commit_ended.load(Ordering::Relaxed) {
        return;
    }
    let charged = process.commit.load(Ordering::Relaxed);
    if pages > charged {
        let more = pages - charged;
        PEAK.fetch_max(COMMITTED.fetch_add(more, Ordering::Relaxed) + more, Ordering::Relaxed);
        let _ = charge_job(process, more, pages, false);
    } else if pages < charged {
        let less = charged - pages;
        COMMITTED.fetch_sub(less, Ordering::Relaxed);
        uncharge_job(process, less);
    }
    process.commit.store(pages, Ordering::Relaxed);
}

/// A process joins a job: its charge is now the job's too.
pub fn joined(process: &Process) {
    let _vad = process.vad.lock();
    let pages = process.commit.load(Ordering::Relaxed);
    let _ = charge_job(process, pages, pages, false);
}

/// A process has ended: its charge goes back (its memory is freed when the
/// last reference to it goes, soon after).
pub fn ended(process: &Process) {
    let _vad = process.vad.lock();
    process.commit_ended.store(true, Ordering::Relaxed);
    let pages = process.commit.swap(0, Ordering::Relaxed);
    COMMITTED.fetch_sub(pages, Ordering::Relaxed);
    uncharge_job(process, pages);
}
