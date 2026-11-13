//! QEMU's fw_cfg (stub: on AArch64 it is a memory-mapped device the port
//! has not mapped yet). Nothing is found, so no hot swap is offered.

pub fn present() -> bool {
    false
}

pub fn find(_name: &[u8]) -> Option<(u16, u32)> {
    None
}

pub fn read(_key: u16, _buf: &mut [u8]) {}
