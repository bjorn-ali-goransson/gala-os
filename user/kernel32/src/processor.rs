//! Processor groups, affinity and topology (what CoreCLR's GC and thread
//! pool size themselves by), as documented: gala has one group (0) of up
//! to 64 processors, one NUMA node and one package; each processor is
//! reported as its own core (gala does not track SMT siblings), and the
//! caches come from CPUID leaf 4 (deterministic cache parameters).

use super::{
    Bool, FALSE, Handle, NtQueryInformationThread, NtSetInformationThread, STATUS_SUCCESS, TRUE,
    basic_information, fail, fail_with, u64_at,
};

const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

/// The processors (active processor mask).
fn processor_mask() -> u64 {
    u64_at(&basic_information(), 48)
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GroupAffinity {
    mask: usize,
    group: u16,
    reserved: [u16; 3],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ProcessorNumber {
    group: u16,
    number: u8,
    reserved: u8,
}

#[unsafe(no_mangle)]
pub extern "system" fn GetActiveProcessorGroupCount() -> u16 {
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn GetMaximumProcessorGroupCount() -> u16 {
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn GetCurrentProcessorNumberEx(n: *mut ProcessorNumber) {
    let number = super::process::GetCurrentProcessorNumber() as u8;
    // SAFETY: the caller's output.
    unsafe { n.write(ProcessorNumber { group: 0, number, reserved: 0 }) };
}

/// GetProcessGroupAffinity: the groups the process has threads in (0).
#[unsafe(no_mangle)]
pub extern "system" fn GetProcessGroupAffinity(
    _process: Handle,
    count: *mut u16,
    groups: *mut u16,
) -> Bool {
    if count.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's count and array of that many entries.
    unsafe {
        if *count < 1 || groups.is_null() {
            *count = 1;
            return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
        }
        *count = 1;
        *groups = 0;
    }
    TRUE
}

/// The thread's affinity mask (THREAD_BASIC_INFORMATION.AffinityMask).
fn thread_affinity(thread: Handle) -> Result<u64, u32> {
    let mut basic = [0u8; 48];
    let mut ret = 0;
    // SAFETY: a 48-byte buffer.
    let status = unsafe { NtQueryInformationThread(thread, 0, basic.as_mut_ptr(), 48, &mut ret) };
    if status != STATUS_SUCCESS {
        return Err(status);
    }
    Ok(u64_at(&basic, 32))
}

/// ThreadAffinityMask (4); the kernel checks it against the process's.
fn set_thread_affinity(thread: Handle, mask: u64) -> u32 {
    let v = mask.to_le_bytes();
    // SAFETY: an 8-byte buffer.
    unsafe { NtSetInformationThread(thread, 4, v.as_ptr(), 8) }
}

#[unsafe(no_mangle)]
pub extern "system" fn GetThreadGroupAffinity(
    thread: Handle,
    affinity: *mut GroupAffinity,
) -> Bool {
    match thread_affinity(thread) {
        Ok(mask) => {
            // SAFETY: the caller's output.
            unsafe {
                affinity.write(GroupAffinity { mask: mask as usize, group: 0, reserved: [0; 3] })
            };
            TRUE
        }
        Err(status) => fail(status, FALSE),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn SetThreadGroupAffinity(
    thread: Handle,
    affinity: *const GroupAffinity,
    previous: *mut GroupAffinity,
) -> Bool {
    // SAFETY: the caller's input.
    let new = unsafe { *affinity };
    if new.group != 0 || new.reserved != [0; 3] {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let old = match thread_affinity(thread) {
        Ok(mask) => mask,
        Err(status) => return fail(status, FALSE),
    };
    let status = set_thread_affinity(thread, new.mask as u64);
    if status != STATUS_SUCCESS {
        return fail(status, FALSE);
    }
    if !previous.is_null() {
        // SAFETY: the caller's output.
        unsafe { previous.write(GroupAffinity { mask: old as usize, group: 0, reserved: [0; 3] }) };
    }
    TRUE
}

/// SetThreadAffinityMask: the previous mask, 0 on failure.
#[unsafe(no_mangle)]
pub extern "system" fn SetThreadAffinityMask(thread: Handle, mask: usize) -> usize {
    let old = match thread_affinity(thread) {
        Ok(mask) => mask,
        Err(status) => return fail(status, 0),
    };
    let status = set_thread_affinity(thread, mask as u64);
    if status != STATUS_SUCCESS {
        return fail(status, 0);
    }
    old as usize
}

/// Get/SetThreadIdealProcessorEx: a scheduling hint gala's scheduler does
/// not take; the ideal processor reads as processor 0.
#[unsafe(no_mangle)]
pub extern "system" fn GetThreadIdealProcessorEx(_thread: Handle, n: *mut ProcessorNumber) -> Bool {
    if n.is_null() {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    // SAFETY: the caller's output.
    unsafe { n.write(ProcessorNumber { group: 0, number: 0, reserved: 0 }) };
    TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn SetThreadIdealProcessorEx(
    _thread: Handle,
    n: *const ProcessorNumber,
    previous: *mut ProcessorNumber,
) -> Bool {
    // SAFETY: the caller's input.
    let n = unsafe { *n };
    if n.group != 0 || processor_mask() >> n.number == 0 {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    if !previous.is_null() {
        // SAFETY: the caller's output.
        unsafe { previous.write(ProcessorNumber { group: 0, number: 0, reserved: 0 }) };
    }
    TRUE
}

/// GetNumaProcessorNodeEx: every processor is on node 0.
#[unsafe(no_mangle)]
pub extern "system" fn GetNumaProcessorNodeEx(n: *const ProcessorNumber, node: *mut u16) -> Bool {
    // SAFETY: the caller's input and output.
    unsafe {
        let n = *n;
        if n.group != 0 || n.number >= 64 || processor_mask() & (1 << n.number) == 0 {
            *node = 0xffff;
            return fail_with(ERROR_INVALID_PARAMETER, FALSE);
        }
        *node = 0;
    }
    TRUE
}

/// A cache from CPUID leaf 4: level, ways, line size, size, type
/// (PROCESSOR_CACHE_TYPE: 0 unified, 1 instruction, 2 data) and whether
/// all processors share it.
#[derive(Clone, Copy)]
struct Cache {
    level: u8,
    ways: u8,
    line: u16,
    size: u32,
    kind: u32,
    shared: bool,
}

/// The caches CPUID leaf 4 describes (up to 8).
fn caches(out: &mut [Cache; 8], processors: u32) -> usize {
    let max = core::arch::x86_64::__cpuid(0).eax;
    if max < 4 {
        return 0;
    }
    let mut n = 0;
    for sub in 0..8 {
        let r = core::arch::x86_64::__cpuid_count(4, sub); // leaf 4 exists (checked)
        let kind = match r.eax & 0x1f {
            0 => break,
            1 => 2, // data
            2 => 1, // instruction
            _ => 0, // unified
        };
        let ways = (r.ebx >> 22) + 1;
        let partitions = ((r.ebx >> 12) & 0x3ff) + 1;
        let line = (r.ebx & 0xfff) + 1;
        let sets = r.ecx + 1;
        let sharing = ((r.eax >> 14) & 0xfff) + 1;
        out[n] = Cache {
            level: ((r.eax >> 5) & 7) as u8,
            ways: ways.min(255) as u8,
            line: line as u16,
            size: ways * partitions * line * sets,
            kind,
            shared: sharing >= processors && processors > 1,
        };
        n += 1;
    }
    n
}

/// Writes `bytes` at `at` in `out` if it fits; the offset after them.
fn put(out: &mut [u8], at: usize, bytes: &[u8]) -> usize {
    if let Some(dst) = out.get_mut(at..at + bytes.len()) {
        dst.copy_from_slice(bytes);
    }
    at + bytes.len()
}

/// GetLogicalProcessorInformation: SYSTEM_LOGICAL_PROCESSOR_INFORMATION
/// entries (32 bytes: ProcessorMask, Relationship, a 16-byte union): a
/// core per processor, the NUMA node, the caches and the package.
#[unsafe(no_mangle)]
pub extern "system" fn GetLogicalProcessorInformation(buf: *mut u8, len: *mut u32) -> Bool {
    let all = processor_mask();
    let count = all.count_ones();
    let mut list = [Cache { level: 0, ways: 0, line: 0, size: 0, kind: 0, shared: false }; 8];
    let ncaches = caches(&mut list, count);
    let mut entries = [[0u8; 32]; 64 + 2 + 8 * 64];
    let mut n = 0;
    let mut entry = |mask: u64, relation: u32, data: &[u8]| {
        let e = &mut entries[n];
        e[..8].copy_from_slice(&mask.to_le_bytes());
        e[8..12].copy_from_slice(&relation.to_le_bytes());
        e[16..16 + data.len()].copy_from_slice(data);
        n += 1;
    };
    for cpu in (0..64).filter(|c| all & (1 << c) != 0) {
        entry(1 << cpu, 0, &[0]); // RelationProcessorCore, Flags 0
    }
    entry(all, 1, &[0]); // RelationNumaNode, node 0
    for c in &list[..ncaches] {
        // CACHE_DESCRIPTOR: Level, Associativity, LineSize, Size, Type.
        let mut d = [0u8; 12];
        d[0] = c.level;
        d[1] = c.ways;
        d[2..4].copy_from_slice(&c.line.to_le_bytes());
        d[4..8].copy_from_slice(&c.size.to_le_bytes());
        d[8..12].copy_from_slice(&c.kind.to_le_bytes());
        if c.shared {
            entry(all, 2, &d);
        } else {
            for cpu in (0..64).filter(|x| all & (1 << x) != 0) {
                entry(1 << cpu, 2, &d);
            }
        }
    }
    entry(all, 3, &[0]); // RelationProcessorPackage
    let need = (n * 32) as u32;
    // SAFETY: the caller's length and buffer of that many bytes.
    unsafe {
        if len.is_null() {
            return fail_with(ERROR_INVALID_PARAMETER, FALSE);
        }
        if buf.is_null() || *len < need {
            *len = need;
            return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
        }
        *len = need;
        let out = core::slice::from_raw_parts_mut(buf, need as usize);
        for (i, e) in entries[..n].iter().enumerate() {
            out[32 * i..32 * i + 32].copy_from_slice(e);
        }
    }
    TRUE
}

/// One SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX entry of `relation` with
/// the relation-specific `head` and a GROUP_AFFINITY of `mask` at the end
/// (or, for groups, `head` alone).
fn put_ex(out: &mut [u8], at: usize, relation: u32, head: &[u8], mask: Option<u64>) -> usize {
    let size = 8 + head.len() + if mask.is_some() { 16 } else { 0 };
    let mut at2 = put(out, at, &relation.to_le_bytes());
    at2 = put(out, at2, &(size as u32).to_le_bytes());
    at2 = put(out, at2, head);
    if let Some(mask) = mask {
        at2 = put(out, at2, &mask.to_le_bytes());
        at2 = put(out, at2, &[0; 8]); // Group 0, Reserved
    }
    at2
}

/// GetLogicalProcessorInformationEx: the same topology as variable-sized
/// entries; RelationAll (0xffff) lists cores, NUMA nodes, caches,
/// packages and groups.
#[unsafe(no_mangle)]
pub extern "system" fn GetLogicalProcessorInformationEx(
    relation: u32,
    buf: *mut u8,
    len: *mut u32,
) -> Bool {
    if len.is_null() || !(relation <= 7 || relation == 0xffff) {
        return fail_with(ERROR_INVALID_PARAMETER, FALSE);
    }
    let all = processor_mask();
    let count = all.count_ones();
    let mut list = [Cache { level: 0, ways: 0, line: 0, size: 0, kind: 0, shared: false }; 8];
    let ncaches = caches(&mut list, count);
    // SAFETY: the caller's length.
    let have = unsafe { *len } as usize;
    let mut scratch = [0u8; 0];
    let out: &mut [u8] = if buf.is_null() {
        &mut scratch
    } else {
        // SAFETY: the caller's buffer of `have` bytes.
        unsafe { core::slice::from_raw_parts_mut(buf, have) }
    };
    let wants = |r: u32| relation == 0xffff || relation == r;
    let mut at = 0;
    // PROCESSOR_RELATIONSHIP: Flags, EfficiencyClass, Reserved[20],
    // GroupCount, then GROUP_AFFINITY.
    let mut processor = [0u8; 24];
    processor[22] = 1;
    if wants(0) {
        for cpu in (0..64).filter(|c| all & (1 << c) != 0) {
            at = put_ex(out, at, 0, &processor, Some(1 << cpu));
        }
    }
    if wants(1) || relation == 6 {
        // NUMA_NODE_RELATIONSHIP: NodeNumber, Reserved[18], GroupCount.
        let mut node = [0u8; 24];
        node[22] = 1;
        at = put_ex(out, at, if relation == 6 { 6 } else { 1 }, &node, Some(all));
    }
    if wants(2) {
        for c in &list[..ncaches] {
            // CACHE_RELATIONSHIP: Level, Associativity, LineSize, CacheSize,
            // Type, Reserved[18], GroupCount.
            let mut d = [0u8; 32];
            d[0] = c.level;
            d[1] = c.ways;
            d[2..4].copy_from_slice(&c.line.to_le_bytes());
            d[4..8].copy_from_slice(&c.size.to_le_bytes());
            d[8..12].copy_from_slice(&c.kind.to_le_bytes());
            d[30] = 1;
            if c.shared {
                at = put_ex(out, at, 2, &d, Some(all));
            } else {
                for cpu in (0..64).filter(|x| all & (1 << x) != 0) {
                    at = put_ex(out, at, 2, &d, Some(1 << cpu));
                }
            }
        }
    }
    if wants(3) {
        at = put_ex(out, at, 3, &processor, Some(all));
    }
    if wants(4) {
        // GROUP_RELATIONSHIP: MaximumGroupCount, ActiveGroupCount,
        // Reserved[20], then PROCESSOR_GROUP_INFO (MaximumProcessorCount,
        // ActiveProcessorCount, Reserved[38], ActiveProcessorMask).
        let mut g = [0u8; 72];
        g[0] = 1;
        g[2] = 1;
        g[24] = count as u8;
        g[25] = count as u8;
        g[64..72].copy_from_slice(&all.to_le_bytes());
        at = put_ex(out, at, 4, &g, None);
    }
    // SAFETY: the caller's length.
    unsafe { *len = at as u32 };
    if buf.is_null() || have < at {
        return fail_with(ERROR_INSUFFICIENT_BUFFER, FALSE);
    }
    TRUE
}
