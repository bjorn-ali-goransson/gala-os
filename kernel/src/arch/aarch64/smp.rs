//! The other CPUs (stub: PSCI CPU_ON, GIC SGIs): one CPU, the boot CPU.

use super::context::StackError;
use super::unported;
use crate::boot::{BootInfo, KernelAddress};
use crate::mm::heap::AllocError;

pub const MAX_CPUS: usize = 64;

/// What became of the bootloader's AP tables (x86-64's late-AP shelter;
/// none here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shelter {
    Added,
    NoTrampoline,
    AlreadyMapped,
    Unsupported,
    UnknownTables,
}

pub fn shelter_state() -> Option<Shelter> {
    None
}

pub fn shelter_bootloader_aps(
    _hhdm_offset: u64,
    _kernel: KernelAddress,
    _reclaimable: impl Iterator<Item = (u64, u64)>,
) {
}

/// (TLB shootdowns, IPIs sent for them).
pub fn flush_stats() -> (u64, u64) {
    (0, 0)
}

pub fn flush_tlb_everywhere() {
    unported("smp::flush_tlb_everywhere")
}

pub fn set_tlb_paranoid(_on: bool) {}

pub fn tlb_paranoid() -> bool {
    false
}

pub fn moved_flushes() -> u64 {
    0
}

pub fn flush_user_space(_root: u64, _changed_on: Option<u32>) {
    unported("smp::flush_user_space")
}

pub fn online_count() -> u32 {
    1
}

pub fn online_mask() -> u64 {
    1
}

#[derive(Debug)]
pub enum SmpError {
    OutOfMemory,
    Stack(StackError),
}

impl From<AllocError> for SmpError {
    fn from(_: AllocError) -> Self {
        SmpError::OutOfMemory
    }
}

impl From<StackError> for SmpError {
    fn from(e: StackError) -> Self {
        SmpError::Stack(e)
    }
}

/// (CPU index, hardware id: MPIDR's affinity) of every online CPU.
pub fn online_cpus() -> impl Iterator<Item = (u32, u32)> {
    core::iter::once((0, 0))
}

pub fn send_reschedule(_index: u32) {}

pub fn send_nmi(_hw_id: u32) {
    unported("smp::send_nmi")
}

pub fn init_bsp() {
    unported("smp::init_bsp")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SmpTest {
    None,
    Hide,
    Late,
}

#[derive(Debug, Default)]
pub struct SmpReport {
    pub online: u32,
    pub reported: usize,
    pub firmware: Option<usize>,
    pub started_here: u32,
    pub failed: u32,
    pub first_failed: Option<u32>,
    pub late_replay: Option<&'static str>,
    pub slow: bool,
}

pub fn start_aps(
    _info: &BootInfo,
    _test: SmpTest,
    _main: fn(u32) -> !,
) -> Result<SmpReport, SmpError> {
    unported("smp::start_aps")
}
