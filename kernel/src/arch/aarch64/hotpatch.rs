//! Patching the running kernel (stub: no hot swap is offered here, since
//! fw_cfg is not mapped; see the x86-64 module).

pub const WINDOW: u64 = 0;
pub const WINDOW_SIZE: u64 = 0;

pub fn current_sp() -> u64 {
    0
}

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

pub fn note_boot_stack() {}

pub fn boot_stack_top() -> u64 {
    0
}
