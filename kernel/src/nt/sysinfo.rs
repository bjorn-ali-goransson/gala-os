//! Encoders for NtQuerySystemInformation and NtQueryInformationProcess
//! results (public x64 layouts: winternl.h / mingw-w64), kept pure so they
//! can be host-tested.

use alloc::vec::Vec;

pub const SYSTEM_BASIC_INFORMATION: u32 = 0;
pub const SYSTEM_PERFORMANCE_INFORMATION: u32 = 2;
pub const SYSTEM_TIME_OF_DAY_INFORMATION: u32 = 3;
pub const SYSTEM_PROCESS_INFORMATION: u32 = 5;

/// Lowest and highest user-mode addresses reported (64 KiB null region,
/// top 64 KiB below 2^47 reserved), as on x64 Windows.
pub const MINIMUM_USER_ADDRESS: u64 = 0x1_0000;
pub const MAXIMUM_USER_ADDRESS: u64 = 0x7fff_fffe_ffff;

/// SYSTEM_BASIC_INFORMATION (64 bytes).
pub fn basic_information(physical_pages: u64, processors: u8, timer_resolution: u32) -> [u8; 64] {
    let mut b = [0u8; 64];
    b[4..8].copy_from_slice(&timer_resolution.to_le_bytes()); // 100 ns units
    b[8..12].copy_from_slice(&4096u32.to_le_bytes()); // PageSize
    let pages = u32::try_from(physical_pages).unwrap_or(u32::MAX);
    b[12..16].copy_from_slice(&pages.to_le_bytes()); // NumberOfPhysicalPages
    b[16..20].copy_from_slice(&1u32.to_le_bytes()); // LowestPhysicalPageNumber
    b[20..24].copy_from_slice(&pages.to_le_bytes()); // HighestPhysicalPageNumber
    b[24..28].copy_from_slice(&0x1_0000u32.to_le_bytes()); // AllocationGranularity
    b[32..40].copy_from_slice(&MINIMUM_USER_ADDRESS.to_le_bytes());
    b[40..48].copy_from_slice(&MAXIMUM_USER_ADDRESS.to_le_bytes());
    let mask = if processors >= 64 { u64::MAX } else { (1u64 << processors) - 1 };
    b[48..56].copy_from_slice(&mask.to_le_bytes()); // ActiveProcessorsAffinityMask
    b[56] = processors;
    b
}

/// SYSTEM_PERFORMANCE_INFORMATION's size on x64 (TODO: check against the
/// pinned build).
pub const PERFORMANCE_INFORMATION_SIZE: usize = 0x158;

/// SYSTEM_PERFORMANCE_INFORMATION: only the memory counts are filled in
/// (AvailablePages at 0x2c, CommittedPages at 0x30, CommitLimit at 0x34,
/// PeakCommitment at 0x38, after the idle time and the I/O counters); the
/// rest is zero.
pub fn performance_information(available: u64, total: u64) -> [u8; PERFORMANCE_INFORMATION_SIZE] {
    let mut b = [0u8; PERFORMANCE_INFORMATION_SIZE];
    let pages = |v: u64| u32::try_from(v).unwrap_or(u32::MAX).to_le_bytes();
    let committed = total.saturating_sub(available);
    b[0x2c..0x30].copy_from_slice(&pages(available));
    b[0x30..0x34].copy_from_slice(&pages(committed));
    b[0x34..0x38].copy_from_slice(&pages(total));
    b[0x38..0x3c].copy_from_slice(&pages(committed));
    b
}

/// SYSTEM_TIMEOFDAY_INFORMATION (48 bytes): boot and current NT times; time
/// zone fields are UTC (bias 0).
pub fn time_of_day(boot: u64, now: u64) -> [u8; 48] {
    let mut b = [0u8; 48];
    b[0..8].copy_from_slice(&boot.to_le_bytes());
    b[8..16].copy_from_slice(&now.to_le_bytes());
    b
}

/// A UNICODE_STRING header followed by the string and a terminator, as
/// returned in place to a buffer at user address `buffer`. Returns the
/// bytes; their length is the required ReturnLength.
pub fn unicode_string_in_place(name: &[u16], buffer: u64) -> Option<Vec<u8>> {
    let bytes = name.len() * 2;
    let length = u16::try_from(bytes + 2).ok()?;
    let mut out = Vec::new();
    out.try_reserve_exact(16 + bytes + 2).ok()?;
    out.resize(16 + bytes + 2, 0u8);
    out[0..2].copy_from_slice(&((bytes) as u16).to_le_bytes());
    out[2..4].copy_from_slice(&length.to_le_bytes());
    out[8..16].copy_from_slice(&(buffer + 16).to_le_bytes());
    for (i, c) in name.iter().enumerate() {
        out[16 + 2 * i..18 + 2 * i].copy_from_slice(&c.to_le_bytes());
    }
    Some(out)
}

/// A process for SystemProcessInformation.
pub struct ProcessRecord<'a> {
    pub id: u64,
    pub parent_id: u64,
    pub create_time: u64,
    pub handle_count: u32,
    /// The image file name (empty for the idle process).
    pub name: &'a str,
}

/// One of its threads.
pub struct ThreadRecord {
    pub id: u64,
    pub start: u64,
    pub create_time: u64,
}

/// SYSTEM_PROCESS_INFORMATION's size on x64; its SYSTEM_THREAD_INFORMATION
/// entries follow it, then the image name.
pub const PROCESS_ENTRY: usize = 0x100;
pub const THREAD_ENTRY: usize = 0x50;
/// KPRIORITY of a normal process / thread.
const NORMAL_PRIORITY: u32 = 8;
/// KTHREAD_STATE Waiting / KWAIT_REASON Executive: what a snapshot of a
/// thread that is not running shows.
const WAITING: u32 = 5;

/// Appends `p`'s entry (to be copied to user address `buffer`) to `out`
/// and links the previous entry to it (NextEntryOffset; the last is 0).
pub fn push_process(
    out: &mut Vec<u8>,
    p: &ProcessRecord<'_>,
    threads: &[ThreadRecord],
    buffer: u64,
) -> Result<(), crate::mm::heap::AllocError> {
    let start = out.len();
    let name_at = PROCESS_ENTRY + THREAD_ENTRY * threads.len();
    let name_bytes = 2 * p.name.len();
    let size = (name_at + if p.name.is_empty() { 0 } else { name_bytes + 2 }).next_multiple_of(8);
    out.try_reserve(size).map_err(|_| crate::mm::heap::AllocError)?;
    out.resize(start + size, 0);
    let e = &mut out[start..];
    let put = |e: &mut [u8], at: usize, v: &[u8]| e[at..at + v.len()].copy_from_slice(v);
    put(e, 0x04, &(threads.len() as u32).to_le_bytes()); // NumberOfThreads
    put(e, 0x20, &p.create_time.to_le_bytes()); // CreateTime
    if !p.name.is_empty() {
        // ImageName: UNICODE_STRING {Length, MaximumLength, Buffer}.
        put(e, 0x38, &(name_bytes as u16).to_le_bytes());
        put(e, 0x3a, &(name_bytes as u16 + 2).to_le_bytes());
        put(e, 0x40, &(buffer + (start + name_at) as u64).to_le_bytes());
        for (i, c) in p.name.bytes().enumerate() {
            put(e, name_at + 2 * i, &u16::from(c).to_le_bytes());
        }
    }
    let priority = if p.id == 0 { 0 } else { NORMAL_PRIORITY };
    put(e, 0x48, &priority.to_le_bytes()); // BasePriority
    put(e, 0x50, &p.id.to_le_bytes()); // UniqueProcessId
    put(e, 0x58, &p.parent_id.to_le_bytes()); // InheritedFromUniqueProcessId
    put(e, 0x60, &p.handle_count.to_le_bytes()); // HandleCount
    for (i, t) in threads.iter().enumerate() {
        let at = PROCESS_ENTRY + THREAD_ENTRY * i;
        put(e, at + 0x10, &t.create_time.to_le_bytes()); // CreateTime
        put(e, at + 0x20, &t.start.to_le_bytes()); // StartAddress
        put(e, at + 0x28, &p.id.to_le_bytes()); // ClientId.UniqueProcess
        put(e, at + 0x30, &t.id.to_le_bytes()); // ClientId.UniqueThread
        put(e, at + 0x38, &NORMAL_PRIORITY.to_le_bytes()); // Priority
        put(e, at + 0x3c, &NORMAL_PRIORITY.to_le_bytes()); // BasePriority
        put(e, at + 0x44, &WAITING.to_le_bytes()); // ThreadState
    }
    // Link the previous entry to this one.
    let mut prev = 0;
    while prev < start {
        let next = u32::from_le_bytes(out[prev..prev + 4].try_into().unwrap_or([0; 4])) as usize;
        if next == 0 {
            out[prev..prev + 4].copy_from_slice(&((start - prev) as u32).to_le_bytes());
            break;
        }
        prev += next;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn performance_information_has_the_memory_counts() {
        let b = super::performance_information(100, 400);
        let at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        assert_eq!((at(0x2c), at(0x30), at(0x34)), (100, 300, 400));
        assert_eq!(b.len(), 0x158);
    }

    use super::*;

    #[test]
    fn basic_information_layout() {
        let b = basic_information(0x2_0000, 1, 10_000);
        assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()), 4096);
        assert_eq!(u32::from_le_bytes(b[12..16].try_into().unwrap()), 0x2_0000);
        assert_eq!(u32::from_le_bytes(b[24..28].try_into().unwrap()), 0x1_0000);
        assert_eq!(u64::from_le_bytes(b[40..48].try_into().unwrap()), 0x7fff_fffe_ffff);
        assert_eq!(u64::from_le_bytes(b[48..56].try_into().unwrap()), 1);
        assert_eq!(b[56], 1);
        assert_eq!(u64::from_le_bytes(basic_information(1, 4, 0)[48..56].try_into().unwrap()), 0xf);
    }

    #[test]
    fn unicode_string_layout() {
        let name: Vec<u16> = "\\A".encode_utf16().collect();
        let b = unicode_string_in_place(&name, 0x1000).unwrap();
        assert_eq!(b.len(), 16 + 4 + 2);
        assert_eq!(u16::from_le_bytes([b[0], b[1]]), 4);
        assert_eq!(u16::from_le_bytes([b[2], b[3]]), 6);
        assert_eq!(u64::from_le_bytes(b[8..16].try_into().unwrap()), 0x1010);
        assert_eq!(&b[16..22], &[b'\\', 0, b'A', 0, 0, 0]);
    }

    #[test]
    fn process_information_layout() {
        let base = 0x10_0000;
        let mut out = Vec::new();
        let idle = ProcessRecord { id: 0, parent_id: 0, create_time: 0, handle_count: 0, name: "" };
        push_process(&mut out, &idle, &[], base).unwrap();
        let p =
            ProcessRecord { id: 8, parent_id: 4, create_time: 77, handle_count: 3, name: "a.exe" };
        let threads = [ThreadRecord { id: 12, start: 0x1400, create_time: 78 }];
        push_process(&mut out, &p, &threads, base).unwrap();
        let u32_at = |at: usize| u32::from_le_bytes(out[at..at + 4].try_into().unwrap());
        let u64_at = |at: usize| u64::from_le_bytes(out[at..at + 8].try_into().unwrap());
        // The idle entry links to the next; the last has no next.
        assert_eq!(u32_at(0), PROCESS_ENTRY as u32);
        let e = PROCESS_ENTRY;
        assert_eq!(u32_at(e), 0);
        assert_eq!(u32_at(e + 4), 1);
        assert_eq!(u64_at(e + 0x20), 77);
        assert_eq!(u64_at(e + 0x50), 8);
        assert_eq!(u64_at(e + 0x58), 4);
        assert_eq!(u32_at(e + 0x60), 3);
        // The thread entry, then the name the UNICODE_STRING points at.
        let t = e + PROCESS_ENTRY;
        assert_eq!(u64_at(t + 0x28), 8);
        assert_eq!(u64_at(t + 0x30), 12);
        assert_eq!(u64_at(t + 0x20), 0x1400);
        assert_eq!(out[e + 0x38], 10);
        let name = (u64_at(e + 0x40) - base) as usize;
        assert_eq!(name, t + THREAD_ENTRY);
        let text: Vec<u8> = (0..5).map(|i| out[name + 2 * i]).collect();
        assert_eq!(text, b"a.exe");
        assert_eq!(out.len() % 8, 0);
    }
}
