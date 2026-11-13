//! QEMU's firmware configuration device (fw_cfg), read through its I/O
//! ports: the selector at 0x510 (16-bit, little-endian), the data at
//! 0x511 (a byte at a time, `rep insb` for many). xtask hands a resumed
//! machine files through it (`-fw_cfg name=opt/...,file=...`): the hot
//! swap's plan (ROADMAP SN step 1d, `crate::hotswap`).
//!
//! Reads only port I/O and atomics: safe in an NMI. One reader at a time
//! (`crate::hotswap` keeps it so); a read from another context would move the
//! device's position under it.
//!
//! The device is probed only on a hypervisor (CPUID's hypervisor bit),
//! where its signature is checked ("QEMU" at selector 0) before anything
//! else is read: on bare metal the ports are left alone.

use core::arch::asm;
use core::sync::atomic::{AtomicU8, Ordering};

use super::cpu;
use super::port::Port;

const SELECTOR: u16 = 0x510;
const DATA: u16 = 0x511;
/// FW_CFG_SIGNATURE, FW_CFG_FILE_DIR (QEMU's docs/specs/fw_cfg.rst).
const SIGNATURE: u16 = 0x0000;
const FILE_DIR: u16 = 0x0019;

/// 0: not probed; 1: present; 2: absent.
static PRESENT: AtomicU8 = AtomicU8::new(0);

fn select(key: u16) {
    // SAFETY: fw_cfg's selector port, probed (`present`) or about to be on
    // a hypervisor; selecting moves only the device's read position.
    unsafe { Port::<u16>::new(SELECTOR) }.write(key);
}

fn read_into(buf: &mut [u8]) {
    if buf.is_empty() {
        return;
    }
    // SAFETY: `rep insb` writes `buf.len()` bytes from fw_cfg's data port
    // into `buf`, which is valid for that many bytes; DF is clear (the
    // ABI's state at any Rust call).
    unsafe {
        asm!(
            "rep insb",
            inout("rdi") buf.as_mut_ptr() => _,
            inout("rcx") buf.len() => _,
            in("dx") DATA,
            options(nostack, preserves_flags)
        );
    }
}

/// Whether QEMU's fw_cfg is there (probed once).
pub fn present() -> bool {
    match PRESENT.load(Ordering::Acquire) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    const HYPERVISOR: u32 = 1 << 31;
    let found = cpu::cpuid(1, 0).ecx & HYPERVISOR != 0 && {
        select(SIGNATURE);
        let mut sig = [0u8; 4];
        read_into(&mut sig);
        &sig == b"QEMU"
    };
    PRESENT.store(if found { 1 } else { 2 }, Ordering::Release);
    found
}

/// The selector key and size of fw_cfg file `name` (`opt/gala/hotswap`).
pub fn find(name: &[u8]) -> Option<(u16, u32)> {
    if !present() {
        return None;
    }
    select(FILE_DIR);
    let mut count = [0u8; 4];
    read_into(&mut count);
    let count = u32::from_be_bytes(count).min(4096);
    let mut entry = [0u8; 64];
    for _ in 0..count {
        read_into(&mut entry);
        let size = u32::from_be_bytes([entry[0], entry[1], entry[2], entry[3]]);
        let key = u16::from_be_bytes([entry[4], entry[5]]);
        let file = &entry[8..];
        let len = file.iter().position(|&b| b == 0).unwrap_or(file.len());
        if &file[..len] == name {
            return Some((key, size));
        }
    }
    None
}

/// Reads the start of item `key` into `buf` (as much as `buf` holds).
pub fn read(key: u16, buf: &mut [u8]) {
    if present() {
        select(key);
        read_into(buf);
    }
}
