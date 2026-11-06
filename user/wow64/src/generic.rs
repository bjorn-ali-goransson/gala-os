//! Services whose arguments convert one by one: a kind per argument
//! (`K`) says how a 32-bit value becomes the 64-bit one and what is
//! written back. Structures with pointer-sized fields that are arguments
//! themselves (OBJECT_ATTRIBUTES, UNICODE_STRING, CLIENT_ID, a security
//! descriptor) convert here; information classes in `info`.

use crate::mem::{self, Res};
use crate::{NtStatus, STATUS_ACCESS_VIOLATION, is_error, syscall};

/// How one 32-bit argument converts.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum K {
    /// A value or a pointer to data laid out alike in x86 and x64 (ULONG,
    /// ACCESS_MASK, BOOLEAN, SIZE_T values, LARGE_INTEGER*, ULONG*, an
    /// IO_STATUS_BLOCK*, which the kernel writes in the x86 layout for a
    /// WOW64 process): zero-extended.
    U,
    /// A HANDLE: sign-extended (pseudo-handles are negative).
    H,
    /// HANDLE* out (optional): probed first, the 64-bit handle written
    /// back.
    Oh,
    /// POBJECT_ATTRIBUTES (optional).
    Oa,
    /// PUNICODE_STRING in (optional).
    Us,
    /// PVOID*, PSIZE_T, PULONG_PTR in and out.
    Sz,
    /// PVOID*, PSIZE_T, PULONG_PTR out only (optional).
    Osz,
    /// PCLIENT_ID in (optional).
    Cid,
    /// PCLIENT_ID out (optional).
    Ocid,
    /// PSECURITY_DESCRIPTOR in (optional; absolute or self-relative).
    Sd,
    /// A PIO_APC_ROUTINE (or a timer's): a 32-bit routine cannot run as
    /// the kernel's APC until APCs reach 32-bit code (ROADMAP WOW stage
    /// 3); a call that names one is refused.
    Apc,
}

/// UNICODE_STRING (x64).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Us64 {
    pub length: u16,
    pub maximum_length: u16,
    pub pad: u32,
    pub buffer: u64,
}

/// OBJECT_ATTRIBUTES (x64).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Oa64 {
    length: u32,
    root: u64,
    name: u64,
    attributes: u32,
    sd: u64,
    qos: u64,
}

/// SECURITY_DESCRIPTOR, absolute (x64).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Sd64 {
    revision: u8,
    sbz1: u8,
    control: u16,
    owner: u64,
    group: u64,
    sacl: u64,
    dacl: u64,
}

/// Where one argument's 64-bit form lives during the call.
#[derive(Default)]
pub struct Slot {
    word: [u64; 2],
    us: Us64,
    oa: Oa64,
    sd: Sd64,
}

/// SE_SELF_RELATIVE: offsets instead of pointers (alike in both layouts).
const SE_SELF_RELATIVE: u16 = 0x8000;

/// The x86 UNICODE_STRING at `at` as the x64 one.
pub fn us_in(at: u64) -> Res<Us64> {
    let mut b = [0u8; 8];
    mem::read(at, &mut b)?;
    Ok(Us64 {
        length: u16::from_le_bytes([b[0], b[1]]),
        maximum_length: u16::from_le_bytes([b[2], b[3]]),
        pad: 0,
        buffer: u64::from(mem::u32_at(&b, 4)),
    })
}

/// The security descriptor at `at` for the kernel: a self-relative one
/// as it is, an absolute one converted into `sd`.
fn sd_in(at: u64, sd: &mut Sd64) -> Res<u64> {
    if at == 0 {
        return Ok(0);
    }
    let mut b = [0u8; 20];
    mem::read(at, &mut b[..4])?;
    let control = u16::from_le_bytes([b[2], b[3]]);
    if control & SE_SELF_RELATIVE != 0 {
        return Ok(at);
    }
    mem::read(at, &mut b)?;
    *sd = Sd64 {
        revision: b[0],
        sbz1: b[1],
        control,
        owner: mem::u32_at(&b, 4).into(),
        group: mem::u32_at(&b, 8).into(),
        sacl: mem::u32_at(&b, 12).into(),
        dacl: mem::u32_at(&b, 16).into(),
    };
    Ok(&raw const *sd as u64)
}

/// The x86 OBJECT_ATTRIBUTES at `at` converted into `slot` (its name and
/// absolute security descriptor too); the 64-bit pointer.
fn oa_in(at: u64, slot: &mut Slot) -> Res<u64> {
    if at == 0 {
        return Ok(0);
    }
    let mut b = [0u8; 24];
    mem::read(at, &mut b)?;
    let length = mem::u32_at(&b, 0);
    let name = u64::from(mem::u32_at(&b, 8));
    if name != 0 {
        slot.us = us_in(name)?;
    }
    let sd = sd_in(mem::u32_at(&b, 16).into(), &mut slot.sd)?;
    slot.oa = Oa64 {
        // The kernel checks the length: the x64 size for the x86 one.
        length: if length == 24 { 48 } else { length },
        root: mem::u32_at(&b, 4) as i32 as i64 as u64,
        name: if name != 0 { &raw const slot.us as u64 } else { 0 },
        attributes: mem::u32_at(&b, 12),
        sd,
        qos: mem::u32_at(&b, 20).into(),
    };
    Ok(&raw const slot.oa as u64)
}

/// Converts `kinds` of the 32-bit `args` into `out` (64-bit), using
/// `slots` for what they point to.
pub fn convert_in(kinds: &[K], args: &[u32], out: &mut [u64], slots: &mut [Slot]) -> Res<()> {
    for (i, (&k, &a)) in kinds.iter().zip(args).enumerate() {
        let p = u64::from(a);
        let slot = &mut slots[i];
        out[i] = match k {
            K::U => p,
            K::H => a as i32 as i64 as u64,
            K::Apc if a != 0 => return Err(crate::STATUS_NOT_SUPPORTED),
            K::Apc => 0,
            K::Oh | K::Osz | K::Ocid if p == 0 => 0,
            K::Oh => {
                mem::probe_write(p, 4)?;
                &raw mut slot.word[0] as u64
            }
            K::Oa => oa_in(p, slot)?,
            K::Us if p == 0 => 0,
            K::Us => {
                slot.us = us_in(p)?;
                &raw const slot.us as u64
            }
            K::Sz => {
                slot.word[0] = mem::get32(p)?.into();
                &raw mut slot.word[0] as u64
            }
            K::Osz => {
                mem::probe_write(p, 4)?;
                &raw mut slot.word[0] as u64
            }
            K::Cid if p == 0 => 0,
            K::Cid => {
                slot.word = [mem::get32(p)?.into(), mem::get32(p + 4)?.into()];
                &raw const slot.word as u64
            }
            K::Ocid => {
                mem::probe_write(p, 8)?;
                &raw mut slot.word as u64
            }
            K::Sd => sd_in(p, &mut slot.sd)?,
        };
    }
    Ok(())
}

/// Writes back the outputs of a call that did not fail.
pub fn convert_out(kinds: &[K], args: &[u32], slots: &[Slot], status: NtStatus) -> Res<()> {
    if is_error(status) {
        return Ok(());
    }
    for (i, (&k, &a)) in kinds.iter().zip(args).enumerate() {
        let p = u64::from(a);
        let w = slots[i].word;
        match k {
            K::Sz => mem::put32(p, w[0] as u32)?,
            K::Oh | K::Osz if p != 0 => mem::put32(p, w[0] as u32)?,
            K::Ocid if p != 0 => {
                mem::put32(p, w[0] as u32)?;
                mem::put32(p + 4, w[1] as u32)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// FILE_DEVICE_NETWORK: AFD's (sockets') IOCTLs, whose structures hold
/// pointers (buffer arrays, addresses).
const FILE_DEVICE_NETWORK: u32 = 0x12;

/// The service `number` with arguments of `kinds`.
pub fn call(number: u32, kinds: &[K], args: &[u32]) -> NtStatus {
    // NtDeviceIoControlFile: control codes pass as they are, except the
    // network's, which are not converted yet.
    if number == 0x0007 && args[5] >> 16 == FILE_DEVICE_NETWORK {
        crate::unsupported_service(number, b"AFD IOCTL");
        return crate::STATUS_NOT_SUPPORTED;
    }
    let mut slots: [Slot; 16] = Default::default();
    let mut out = [0u64; 16];
    if let Err(e) = convert_in(kinds, args, &mut out, &mut slots) {
        if e == crate::STATUS_NOT_SUPPORTED {
            crate::unsupported_service(number, b"APC routine");
        }
        return e;
    }
    let status = syscall(number, &out[..kinds.len()]);
    match convert_out(kinds, args, &slots, status) {
        Ok(()) => status,
        Err(_) => STATUS_ACCESS_VIOLATION,
    }
}

use K::*;

/// The services converted argument by argument: (number, the kind of each
/// argument). The x64 numbers, as `kernel/src/nt/syscalls.rs`.
pub static TABLE: &[(u32, &[K])] = &[
    (0x0004, &[H, U, U]),                            // NtWaitForSingleObject
    (0x0006, &[H, H, Apc, U, U, U, U, U, U]),        // NtReadFile
    (0x0007, &[H, H, Apc, U, U, U, U, U, U, U]),     // NtDeviceIoControlFile
    (0x0008, &[H, H, Apc, U, U, U, U, U, U]),        // NtWriteFile
    (0x0009, &[H, Osz, Osz, U, U]),                  // NtRemoveIoCompletion
    (0x000a, &[H, U, U]),                            // NtReleaseSemaphore
    (0x000e, &[H, U]),                               // NtSetEvent
    (0x000f, &[H]),                                  // NtClose
    (0x0011, &[H, U, U, U, U]),                      // NtQueryInformationFile
    (0x0012, &[Oh, U, Oa]),                          // NtOpenKey
    (0x0013, &[H, U, U, U, U, U]),                   // NtEnumerateValueKey
    (0x0016, &[H, U, U, U, U]),                      // NtQueryKey
    (0x0017, &[H, Us, U, U, U, U]),                  // NtQueryValueKey
    (0x0018, &[H, Sz, U, Sz, U, U]),                 // NtAllocateVirtualMemory
    (0x001d, &[Oh, U, Oa, U, Us, U, U]),             // NtCreateKey
    (0x001e, &[H, Sz, Sz, U]),                       // NtFreeVirtualMemory
    (0x0020, &[H, U]),                               // NtReleaseMutant
    (0x0026, &[Oh, U, Oa, Cid]),                     // NtOpenProcess
    (0x0028, &[H, H, Sz, U, U, U, Sz, U, U, U]),     // NtMapViewOfSection
    (0x002a, &[H, U]),                               // NtUnmapViewOfSection
    (0x002c, &[H, U]),                               // NtTerminateProcess
    (0x0031, &[U, U]),                               // NtQueryPerformanceCounter
    (0x0032, &[H, U, U, U, U, U]),                   // NtEnumerateKey
    (0x0033, &[Oh, U, Oa, U, U, U]),                 // NtOpenFile
    (0x0034, &[U, U]),                               // NtDelayExecution
    (0x0035, &[H, H, Apc, U, U, U, U, U, U, Us, U]), // NtQueryDirectoryFile
    (0x0037, &[Oh, U, Oa]),                          // NtOpenSection
    (0x003a, &[H, U, U, U, Osz]),                    // NtWriteVirtualMemory
    (0x003c, &[H, H, H, Oh, U, U, U]),               // NtDuplicateObject
    (0x003d, &[Oa, U]),                              // NtQueryAttributesFile
    (0x003e, &[H]),                                  // NtClearEvent
    (0x003f, &[H, U, U, U, Osz]),                    // NtReadVirtualMemory
    (0x0040, &[Oh, U, Oa]),                          // NtOpenEvent
    (0x0046, &[]),                                   // NtYieldExecution
    (0x0048, &[Oh, U, Oa, U, U]),                    // NtCreateEvent
    (0x0049, &[H, U, U, U, U]),                      // NtQueryVolumeInformationFile
    (0x004a, &[Oh, U, Oa, U, U, U, H]),              // NtCreateSection
    (0x004b, &[H, U]),                               // NtFlushBuffersFile
    (0x0050, &[H, Sz, Sz, U, U]),                    // NtProtectVirtualMemory
    (0x0052, &[H, U]),                               // NtResumeThread
    (0x0053, &[H, U]),                               // NtTerminateThread
    (0x0055, &[Oh, U, Oa, U, U, U, U, U, U, U, U]),  // NtCreateFile
    (0x0056, &[H, U, U, U, U]),                      // NtQueryEvent
    (0x0058, &[Oh, U, Oa]),                          // NtOpenDirectoryObject
    (0x005a, &[U]),                                  // NtQuerySystemTime
    (0x0060, &[H, Us, U, U, U, U]),                  // NtSetValueKey
    (0x00bd, &[H, U]),                               // NtCancelTimer
    (0x00c3, &[Oh, U, Oa, U]),                       // NtCreateMutant
    (0x00c9, &[Oh, U, Oa, U]),                       // NtCreateIoCompletion
    (0x00cf, &[Oh, U, Oa, U, U]),                    // NtCreateSemaphore
    (0x00d1, &[Oh, U, Oa, Us]),                      // NtCreateSymbolicLinkObject
    (0x00d9, &[Oh, U, Oa, U]),                       // NtCreateTimer
    (0x00f8, &[]),                                   // NtGetCurrentProcessorNumber
    (0x0123, &[H, H, Apc, U, U, U, U, U, U, U]),     // NtLockFile
    (0x012c, &[Oh, U, Oa]),                          // NtOpenMutant
    (0x012f, &[Oh, U, Oa]),                          // NtOpenSemaphore
    (0x0137, &[Oh, U, Oa]),                          // NtOpenSymbolicLinkObject
    (0x013d, &[Oh, U, Oa]),                          // NtOpenTimer
    (0x0146, &[H, U, U, U, U]),                      // NtQueryMutant
    (0x014f, &[H, U, U, U, U]),                      // NtQueryIoCompletion
    (0x015a, &[H, U, U, U, U]),                      // NtQuerySemaphore
    (0x015f, &[H, U, U, U, U]),                      // NtQueryTimer
    (0x019e, &[H, U, Apc, U, U, U, U]),              // NtSetTimer
    (0x01b0, &[H, U, U, U, U]),                      // NtSetIoCompletion
    (0x01bc, &[H, U]),                               // NtSuspendThread
    (0x01c5, &[H, U, U, U, U]),                      // NtUnlockFile
    (0x7f00, &[U, U, U, U]),                         // GalaRecordMissing
    (0x7f01, &[H, U, U]),                            // NtCancelIoFileEx
    // GalaCreateProcess: its block has fixed 8-byte fields whatever the
    // caller's bitness (ARCHITECTURE "Processes started by programs").
    (0x7f02, &[U]),
    (0x7f03, &[Oh, U, Oa, U, U, U, U, U, U, U, U, U, U, U]), // NtCreateNamedPipeFile
    (0x7f04, &[H, H, Apc, U, U, U, U, U, U, U]),             // NtFsControlFile
    (0x7f06, &[Oh, U, Oa, U]),                               // NtOpenKeyEx
    (0x7f07, &[H]),                                          // NtDeleteKey
    (0x7f08, &[H, Us]),                                      // NtDeleteValueKey
    (0x7f09, &[H]),                                          // NtFlushKey
    (0x7f0a, &[Oh, U, Oa]),                                  // NtCreateJobObject
    (0x7f0b, &[H, H]),                                       // NtAssignProcessToJobObject
    (0x7f0e, &[H, H]),                                       // NtIsProcessInJob
    (0x7f0f, &[Oh, U, Oa, U]),                               // NtCreateKeyedEvent
    (0x7f10, &[Oh, U, Oa]),                                  // NtOpenKeyedEvent
    (0x7f11, &[H, U, U, U]),                                 // NtWaitForKeyedEvent
    (0x7f12, &[H, U, U, U]),                                 // NtReleaseKeyedEvent
    (0x7f13, &[H, H, Apc, U, U, U, U, U, U]),                // NtNotifyChangeDirectoryFile
    // FILE_SEGMENT_ELEMENT is 8 bytes in both layouts (PVOID64).
    (0x7f14, &[H, H, Apc, U, U, U, U, U, U]), // NtReadFileScatter
    (0x7f15, &[H, H, Apc, U, U, U, U, U, U]), // NtWriteFileGather
    (0x7f16, &[H, U, Oh]),                    // NtOpenProcessToken
    (0x7f17, &[H, U, U, Oh]),                 // NtOpenProcessTokenEx
    (0x7f18, &[H, U, U, Oh]),                 // NtOpenThreadToken
    (0x7f19, &[H, U, U, U, Oh]),              // NtOpenThreadTokenEx
    // TOKEN_PRIVILEGES and PRIVILEGE_SET hold no pointers.
    (0x7f1b, &[H, U, U, U, U, U]),   // NtAdjustPrivilegesToken
    (0x7f1c, &[H, U, Oa, U, U, Oh]), // NtDuplicateToken
    (0x7f1e, &[H, U, U]),            // NtPrivilegeCheck
    // Self-relative descriptors hold offsets, alike in both layouts.
    (0x7f1f, &[H, U, U, U, U]),  // NtQuerySecurityObject
    (0x7f20, &[H, U, Sd]),       // NtSetSecurityObject
    (0x7f21, &[U, U, U, U, Oh]), // GalaLogonUser
    (0x7f22, &[U]),              // NtAllocateLocallyUniqueId
    (0x7f24, &[Oh, U, Oa]),      // NtCreateDirectoryObject
    (0x7f4d, &[U, U]),           // GalaRandom
    (0x7f90, &[H, H]),           // NtCompareObjects
    (0x7f91, &[H]),              // NtSuspendProcess
    (0x7f92, &[H]),              // NtResumeProcess
    (0x7f93, &[Oh, U, Oa, Cid]), // NtOpenThread
];

/// The kinds of service `number`'s arguments, if it converts generically.
pub fn kinds(number: u32) -> Option<&'static [K]> {
    TABLE.iter().find(|(n, _)| *n == number).map(|&(_, k)| k)
}
