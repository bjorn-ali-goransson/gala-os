//! Fault-safe access to a 32-bit caller's memory, and scratch buffers.
//!
//! Every read and write of memory a 32-bit caller names goes through
//! `gala_wow64_copy` (lib.rs), whose one faulting instruction the
//! exception dispatcher (`GalaWow64Exception`) recognizes: a fault there
//! resumes at its failure exit instead of ending the process, so an
//! invalid pointer fails the call with STATUS_ACCESS_VIOLATION, as
//! Windows' thunks do (their `__try`/`__except`). Demand paging, guard
//! pages and copy-on-write are the kernel's as for any access.

use crate::{NtStatus, STATUS_ACCESS_VIOLATION, syscall};

unsafe extern "C" {
    fn gala_wow64_copy(dst: u64, src: u64, len: u64) -> u64;
}

pub type Res<T> = Result<T, NtStatus>;

/// Copies `len` bytes from `src` to `dst`, either of which may be a
/// caller's address; STATUS_ACCESS_VIOLATION if either faults.
pub fn copy(dst: u64, src: u64, len: u64) -> Res<()> {
    if len == 0 {
        return Ok(());
    }
    // SAFETY: the copy's faults are caught (GalaWow64Exception); wow64.dll
    // passes its own buffers only with their lengths.
    match unsafe { gala_wow64_copy(dst, src, len) } {
        0 => Ok(()),
        _ => Err(STATUS_ACCESS_VIOLATION),
    }
}

/// Reads the caller's bytes at `addr` into `buf`.
pub fn read(addr: u64, buf: &mut [u8]) -> Res<()> {
    copy(buf.as_mut_ptr() as u64, addr, buf.len() as u64)
}

/// Writes `data` to the caller's `addr`.
pub fn write(addr: u64, data: &[u8]) -> Res<()> {
    copy(addr, data.as_ptr() as u64, data.len() as u64)
}

pub fn get32(addr: u64) -> Res<u32> {
    let mut b = [0u8; 4];
    read(addr, &mut b)?;
    Ok(u32::from_le_bytes(b))
}

pub fn get16(addr: u64) -> Res<u16> {
    let mut b = [0u8; 2];
    read(addr, &mut b)?;
    Ok(u16::from_le_bytes(b))
}

pub fn put32(addr: u64, v: u32) -> Res<()> {
    write(addr, &v.to_le_bytes())
}

pub fn put16(addr: u64, v: u16) -> Res<()> {
    write(addr, &v.to_le_bytes())
}

/// Checks that the caller's `len` bytes at `addr` are writable (reads
/// them and writes them back), before a service whose result could not
/// be handed back otherwise (a handle it created).
pub fn probe_write(addr: u64, len: usize) -> Res<()> {
    let mut b = [0u8; 16];
    let b = &mut b[..len.min(16)];
    read(addr, b)?;
    write(addr, b)
}

/// Little-endian fields of a local buffer.
pub fn u32_at(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub fn u64_at(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 8)
        .map_or(0, |s| u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

pub fn set32(b: &mut [u8], at: usize, v: u32) {
    if let Some(s) = b.get_mut(at..at + 4) {
        s.copy_from_slice(&v.to_le_bytes());
    }
}

const NT_ALLOCATE_VIRTUAL_MEMORY: u32 = 0x18;
const NT_FREE_VIRTUAL_MEMORY: u32 = 0x1e;
const MEM_COMMIT_RESERVE: u64 = 0x3000;
const MEM_RELEASE: u64 = 0x8000;
const PAGE_READWRITE: u64 = 4;

/// A scratch buffer of committed memory (for results larger than the
/// 64-bit stack should hold), freed when dropped. wow64.dll has no heap.
pub struct Scratch {
    base: u64,
    len: usize,
}

impl Scratch {
    pub fn new(len: usize) -> Res<Scratch> {
        let (mut base, mut size) = (0u64, len.max(1) as u64);
        let args = [
            u64::MAX,
            &raw mut base as u64,
            0,
            &raw mut size as u64,
            MEM_COMMIT_RESERVE,
            PAGE_READWRITE,
        ];
        match syscall(NT_ALLOCATE_VIRTUAL_MEMORY, &args) {
            0 => Ok(Scratch { base, len }),
            e => Err(e),
        }
    }

    pub fn addr(&self) -> u64 {
        self.base
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: `len` bytes committed read-write at `base`, owned by this
        // value until it is dropped.
        unsafe { core::slice::from_raw_parts_mut(self.base as *mut u8, self.len) }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let (mut base, mut size) = (self.base, 0u64);
        syscall(
            NT_FREE_VIRTUAL_MEMORY,
            &[u64::MAX, &raw mut base as u64, &raw mut size as u64, MEM_RELEASE],
        );
    }
}
