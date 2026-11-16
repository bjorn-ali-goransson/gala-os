//! Patching the running kernel (stub: no hot swap is offered here, since
//! fw_cfg is not mapped; see the x86-64 module).

pub const WINDOW: u64 = 0;
pub const WINDOW_SIZE: u64 = 0;

pub fn current_ip() -> u64 {
    0
}

pub fn serialize() {}

pub struct Area {
    pub base: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AreaError {
    TooLarge,
    AlreadyMapped,
    NoMemory,
    Map,
}

pub fn map_area(_blob: &[u8], _data_off: usize) -> Result<Area, AreaError> {
    Err(AreaError::Map)
}

impl Area {
    pub fn pages(&self) -> u64 {
        0
    }

    pub fn unmap(&mut self) {}
}

pub fn text_alias(_va: u64) -> Option<u64> {
    None
}

pub fn write_text(_aliases: &[u64], _bytes: &[u8]) {}

pub fn read_kernel(_va: u64, _buf: &mut [u8]) -> bool {
    false
}

/// The top of the stack the boot thread runs on (the bootloader's), as
/// x86-64's: `note_boot_stack`.
static BOOT_STACK_TOP: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Records the boot thread's stack top: called first thing by `kmain`,
/// whose stack pointer then lies within a page of it (Limine's stack ends
/// at a page boundary).
pub fn note_boot_stack() {
    const PAGE: u64 = 4096;
    let top = (current_sp() + PAGE) & !(PAGE - 1);
    BOOT_STACK_TOP.store(top, core::sync::atomic::Ordering::Release);
}

/// The boot thread's stack top (`note_boot_stack`; 0 before).
pub fn boot_stack_top() -> u64 {
    BOOT_STACK_TOP.load(core::sync::atomic::Ordering::Acquire)
}

/// This CPU's stack pointer.
#[inline(always)]
pub fn current_sp() -> u64 {
    let sp: u64;
    // SAFETY: reads a register.
    unsafe {
        core::arch::asm!("mov {}, sp", out(reg) sp, options(nomem, nostack, preserves_flags))
    };
    sp
}
