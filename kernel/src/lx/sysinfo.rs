//! What a Linux program learns about the machine (ROADMAP LD step 9:
//! GIMP's start): sysinfo(2) and sched_getaffinity(2). glibc's
//! `sysconf(_SC_PHYS_PAGES)` is sysinfo's totalram (GIMP sizes its undo
//! memory and tile cache from it); GLib's `g_get_num_processors` counts
//! the affinity mask (GEGL sizes its thread pool from it). Sources: the
//! man-pages; sysinfo_test's transcript, recorded on Linux.
//!
//! The figures are gala's own: RAM is the frame allocator's managed
//! frames and its free ones (the figure NT's NumberOfPhysicalPages and
//! the commit limit are), in bytes (`mem_unit` 1, as Linux on 64 bits);
//! no swap (gala has no pagefile: ARCHITECTURE "Memory"), no shared or
//! buffer memory counted apart, no high memory; `procs` the threads of
//! the running processes (Linux's nr_threads); the uptime the monotonic
//! clock's, rounded up to a second as Linux rounds it. The load averages
//! are 0: gala keeps none. The affinity mask is the thread's scheduler
//! mask (NT's ThreadAffinityMask, every online CPU unless set), at most
//! 64 CPUs: Linux's cpumask_size, one long.
//!
//! Observability: refusals are the errnos the man-pages list.

use super::dispatch::{Call, Result};
use super::errno::{EFAULT, EINVAL, ESRCH};
use crate::nt::object::ObjectBody;
use crate::nt::platform;

/// struct sysinfo on x86-64: 112 bytes.
pub const SYSINFO_SIZE: usize = 112;

/// The machine's figures sysinfo reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Figures {
    pub uptime_ns: u64,
    pub total_frames: u64,
    pub free_frames: u64,
    pub threads: u64,
}

/// `f` laid out as struct sysinfo (zeroed holes, as Linux clears it).
pub fn layout(f: &Figures) -> [u8; SYSINFO_SIZE] {
    let mut b = [0u8; SYSINFO_SIZE];
    // Whole seconds, a started one counted (Linux: tv_sec + !!tv_nsec).
    let uptime = f.uptime_ns.div_ceil(1_000_000_000);
    b[0..8].copy_from_slice(&uptime.to_le_bytes());
    // loads[3] at 8..32: 0.
    b[32..40].copy_from_slice(&(f.total_frames * 4096).to_le_bytes()); // totalram
    b[40..48].copy_from_slice(&(f.free_frames * 4096).to_le_bytes()); // freeram
    // sharedram, bufferram, totalswap, freeswap at 48..80: 0.
    let procs = u16::try_from(f.threads).unwrap_or(u16::MAX);
    b[80..82].copy_from_slice(&procs.to_le_bytes());
    // pad, its hole, totalhigh, freehigh at 82..104: 0.
    b[104..108].copy_from_slice(&1u32.to_le_bytes()); // mem_unit
    b
}

/// sysinfo(info).
pub fn sysinfo(call: &Call<'_>, info: u64) -> Result {
    let (free_frames, total_frames) = platform::frame_stats();
    let threads = crate::nt::process::running()
        .map(|list| list.iter().map(|p| p.thread_count() as u64).sum())
        .unwrap_or(1);
    let f = Figures { uptime_ns: platform::monotonic_ns(), total_frames, free_frames, threads };
    call.process.write(info, &layout(&f)).map_err(|_| EFAULT)?;
    Ok(0)
}

/// The size sched_getaffinity copies (Linux's cpumask_size: whole longs
/// for nr_cpu_ids), or EINVAL for a buffer of `len` bytes Linux refuses:
/// too small for the CPUs, or not whole longs.
pub fn affinity_size(len: u64, cpus: u32) -> core::result::Result<u64, u16> {
    if len.saturating_mul(8) < u64::from(cpus) || !len.is_multiple_of(8) {
        return Err(EINVAL);
    }
    Ok(8)
}

/// sched_getaffinity(pid, len, mask): the mask of thread `pid` (a Linux
/// tid; 0 the caller).
pub fn sched_getaffinity(call: &Call<'_>, pid: u64, len: u64, mask: u64) -> Result {
    let size = affinity_size(len, platform::online_count())?;
    let pid = pid as u32 as i32;
    let core = if pid == 0 {
        call.tid
    } else {
        let (_, core) = super::state::find_thread(pid as u64).ok_or(ESRCH)?;
        core
    };
    let thread = crate::nt::thread::object_by_id(core).ok_or(ESRCH)?;
    let ObjectBody::Thread(t) = &thread.body else { return Err(ESRCH) };
    let bits = t.affinity() & platform::online_mask();
    let bytes = bits.to_le_bytes();
    let n = usize::try_from(size).unwrap_or(8).min(bytes.len());
    call.process.write(mask, &bytes[..n]).map_err(|_| EFAULT)?;
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sysinfo_counts_bytes_with_a_unit_of_one() {
        let f = Figures { uptime_ns: 1_000_000_001, total_frames: 3, free_frames: 2, threads: 7 };
        let b = layout(&f);
        assert_eq!(u64::from_le_bytes(b[0..8].try_into().unwrap()), 2); // rounded up
        assert_eq!(u64::from_le_bytes(b[32..40].try_into().unwrap()), 3 * 4096);
        assert_eq!(u64::from_le_bytes(b[40..48].try_into().unwrap()), 2 * 4096);
        assert_eq!(u16::from_le_bytes(b[80..82].try_into().unwrap()), 7);
        assert_eq!(u32::from_le_bytes(b[104..108].try_into().unwrap()), 1);
        assert!(b[82..104].iter().chain(&b[108..]).all(|&x| x == 0));
        assert_eq!(layout(&Figures { uptime_ns: 2_000_000_000, ..f })[0], 2);
    }

    #[test]
    fn the_mask_size_is_whole_longs_for_the_cpus() {
        assert_eq!(affinity_size(128, 2), Ok(8));
        assert_eq!(affinity_size(8, 64), Ok(8));
        assert_eq!(affinity_size(4, 2), Err(EINVAL));
        assert_eq!(affinity_size(12, 2), Err(EINVAL));
        assert_eq!(affinity_size(0, 1), Err(EINVAL));
    }
}
