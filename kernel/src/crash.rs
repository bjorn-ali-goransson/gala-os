//! The crash record kept across reboots (docs/OBSERVABILITY.md §5,
//! ROADMAP R4): when the kernel panics, the panic message, its backtrace
//! and the flight recorder's last events go to the crash area of the disk
//! (the last sectors of a version 2 volume, `fs::volume::CRASH_SECTORS`);
//! the next boot that mounts the volume prints the record and clears it.
//! It stays on the machine (privacy by design: nothing is sent anywhere).
//!
//! The record: magic `GCRASH\0\x01` (its last byte the format's version),
//! the text's length (u32), CRC-32 of the text, 0 (u32), then the text
//! (UTF-8), the rest of the area zero.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU64, Ordering};

pub const MAGIC: &[u8; 8] = b"GCRASH\0\x01";
pub const HEADER: usize = 20;
/// The record's size on the disk (the crash area: 16 sectors).
pub const SIZE: usize = 16 * 512;

/// Writes `text` as a record into `out` (SIZE bytes; the text cut to fit).
pub fn encode(text: &[u8], out: &mut [u8; SIZE]) {
    out.fill(0);
    let n = text.len().min(SIZE - HEADER);
    out[HEADER..HEADER + n].copy_from_slice(&text[..n]);
    seal(out, n);
}

/// Writes the header for the `n` bytes of text already after it, and
/// zeroes the rest.
pub fn seal(out: &mut [u8; SIZE], n: usize) {
    let n = n.min(SIZE - HEADER);
    out[HEADER + n..].fill(0);
    out[0..8].copy_from_slice(MAGIC);
    out[8..12].copy_from_slice(&(n as u32).to_le_bytes());
    let crc = crate::fs::log::crc32(0, &out[HEADER..HEADER + n]);
    out[12..16].copy_from_slice(&crc.to_le_bytes());
    out[16..20].fill(0);
}

/// The record's text, if `area` holds a valid one.
pub fn decode(area: &[u8]) -> Option<&[u8]> {
    if area.len() < HEADER || &area[0..8] != MAGIC {
        return None;
    }
    let n = u32::from_le_bytes(area[8..12].try_into().ok()?) as usize;
    let text = area.get(HEADER..HEADER + n)?;
    let crc = u32::from_le_bytes(area[12..16].try_into().ok()?);
    (crate::fs::log::crc32(0, text) == crc).then_some(text)
}

/// The crash area's first sector (0: none, no version 2 volume mounted).
#[cfg_attr(any(test, feature = "hosted"), allow(dead_code))]
static AREA: AtomicU64 = AtomicU64::new(0);

/// The record being built in a panic (no allocation there).
#[cfg_attr(any(test, feature = "hosted"), allow(dead_code))]
static BUFFER: spin::Mutex<([u8; SIZE], usize)> = spin::Mutex::new(([0; SIZE], 0));

/// Text into BUFFER's text part (cut when full).
#[cfg_attr(any(test, feature = "hosted"), allow(dead_code))]
struct Text<'a>(&'a mut ([u8; SIZE], usize));

impl Write for Text<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let (buf, len) = &mut *self.0;
        let room = buf.len() - *len;
        let n = s.len().min(room);
        buf[*len..*len + n].copy_from_slice(&s.as_bytes()[..n]);
        *len += n;
        Ok(())
    }
}

/// The volume's crash area starts at sector `first`; prints and clears a
/// record the previous boot left there. Whether there was one.
#[cfg(not(any(test, feature = "hosted")))]
pub fn mounted(first: u64) -> bool {
    AREA.store(first, Ordering::Release);
    let Some(mut guard) = BUFFER.try_lock() else { return false };
    let (buf, _) = &mut *guard;
    if crate::disk::read(first, buf).is_err() {
        return false;
    }
    let Some(text) = decode(buf) else { return false };
    crate::info!("crash: a record from the previous boot (kept on this machine only):");
    for line in text.split(|&b| b == b'\n').filter(|l| !l.is_empty()) {
        crate::info!("crash: {}", core::str::from_utf8(line).unwrap_or("(not UTF-8)"));
    }
    buf.fill(0);
    let cleared = crate::disk::write(first, buf).and_then(|()| crate::disk::flush());
    if cleared.is_err() {
        crate::warn!("crash: the record could not be cleared");
    }
    true
}

/// From the panic handler: the record goes to the crash area, if there is
/// one and the disk is free (it never waits: the panic may hold anything).
#[cfg(not(any(test, feature = "hosted")))]
pub fn save(info: &core::panic::PanicInfo, backtrace: &dyn fmt::Display) {
    let first = AREA.load(Ordering::Acquire);
    if first == 0 {
        return;
    }
    let Some(mut guard) = BUFFER.try_lock() else { return };
    // The text goes straight after the header's room.
    guard.1 = HEADER;
    {
        let mut t = Text(&mut guard);
        let _ = writeln!(t, "KERNEL PANIC: {info}");
        let _ = writeln!(t, "{backtrace}");
        crate::flight::dump(u64::MAX, 48, |line| {
            let _ = writeln!(t, "{line}");
        });
    }
    let (buf, len) = &mut *guard;
    let n = *len - HEADER;
    seal(buf, n);
    if crate::disk::write_in_panic(first, buf).is_ok() {
        crate::console::emergency_println(format_args!(
            "crash: the record is on the disk (sector {first}); the next boot prints it"
        ));
    }
}

/// gala-um and host tests keep no crash record (no virtio disk).
#[cfg(any(test, feature = "hosted"))]
pub fn mounted(_first: u64) -> bool {
    let _ = Ordering::Relaxed;
    false
}

#[cfg(any(test, feature = "hosted"))]
pub fn save(_info: &core::panic::PanicInfo, _backtrace: &dyn fmt::Display) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_and_damage_is_noticed() {
        let mut area = [0u8; SIZE];
        encode(b"KERNEL PANIC: test\nbacktrace: 0x1\n", &mut area);
        assert_eq!(decode(&area), Some(&b"KERNEL PANIC: test\nbacktrace: 0x1\n"[..]));
        area[HEADER] ^= 1;
        assert_eq!(decode(&area), None);
        assert_eq!(decode(&[0u8; SIZE]), None);
        // Too long: cut to the area.
        let long = alloc::vec![b'x'; SIZE * 2];
        encode(&long, &mut area);
        assert_eq!(decode(&area).map(<[u8]>::len), Some(SIZE - HEADER));
    }
}
