//! The kernel handover record (ROADMAP R9, ARCHITECTURE "Kernel handover"):
//! what a running kernel tells the kernel it hands the machine to, in
//! place of the bootloader's answers. Plain bytes, little-endian,
//! versioned (ARCHITECTURE invariant 3), checked by a CRC.
//!
//! ```text
//! header (64 bytes)
//!   0  magic "GALAHAND"
//!   8  format version (u32)      12 header length (u32, 64)
//!  16  total length (u64)        24 CRC-32 of the record, this field as 0 (u32)
//!  28  flags (u32, 0)            32 generation: handovers done (u64)
//!  40  section count (u32)       44 reserved (20 bytes of 0)
//! sections, each 8-aligned
//!   0  tag (u32)   4 version (u16)   6 flags (u16)   8 length (u32)   12 0 (u32)
//!  16  data (length bytes, padded with 0 to 8)
//! ```
//!
//! How it grows: a new kind of state is a new section (tag); an existing
//! section only ever gains fields at its end, with its version raised. A
//! reader takes the fields the section's length holds and uses 0 for the
//! rest (a record from an older kernel), and ignores bytes past the
//! fields it knows (a newer one). A section a reader must understand to
//! carry on safely (processes and their memory, later) sets
//! [`REQUIRED`]: a reader that does not know its tag refuses the record;
//! other unknown sections are skipped. The header's format version
//! changes only if this framing changes; readers accept [`MIN_VERSION`]
//! to [`VERSION`].

use alloc::vec::Vec;

use crate::fs::log::crc32;
use crate::mm::heap::{AllocError, TryVec};

pub const MAGIC: [u8; 8] = *b"GALAHAND";
/// The framing's version (header and section headers).
pub const VERSION: u32 = 1;
/// The oldest framing this kernel reads.
pub const MIN_VERSION: u32 = 1;
pub const HEADER_LEN: usize = 64;
const SECTION_HEADER_LEN: usize = 16;
/// Section flag: a reader that does not know the tag must refuse the record.
pub const REQUIRED: u16 = 1;
/// Records larger than this are refused (a damaged length).
pub const MAX_LEN: u64 = 64 << 20;

/// Section tags (never reused).
pub mod tag {
    /// The physical memory map (`MemoryRegion`s, kinds as Limine numbers).
    pub const MEMORY_MAP: u32 = 1;
    /// HHDM offset, the new kernel image, firmware type, boot time.
    pub const PLATFORM: u32 = 2;
    pub const FRAMEBUFFER: u32 = 3;
    /// The kernel command line (UTF-8).
    pub const CMDLINE: u32 = 4;
    /// Boot modules (files the bootloader loaded), still in memory.
    pub const MODULES: u32 = 5;
    /// The clock, so time goes on without a jump (invariant 5).
    pub const CLOCK: u32 = 6;
    /// The processors, parked where the new kernel starts them.
    pub const CPUS: u32 = 7;
    /// The mounted disk volume's log position.
    pub const VOLUME: u32 = 8;
    /// Physical ranges in use and what for.
    pub const KEPT: u32 = 9;
    /// The kernel executable file (ELF) to hand over to next time.
    pub const KERNEL_FILE: u32 = 10;
    // Programs (milestone (b); `handover::programs` has their formats).
    // All REQUIRED: a kernel that does not know one would lose programs.
    /// Physical ranges holding user pages and section pages (`Range`s,
    /// `what` 0): the next kernel's frame allocator keeps them.
    pub const USER_FRAMES: u32 = 11;
    /// Processes (and the NT id counter).
    pub const PROCESSES: u32 = 12;
    /// Address spaces: VAD, section views, page mappings.
    pub const ADDRESS_SPACES: u32 = 13;
    /// Threads as data: "in system call N with these registers, until T".
    pub const THREADS: u32 = 14;
    /// Objects by kind (one tag per kind, so a kernel that does not know
    /// a kind refuses the record before it starts).
    pub const EVENTS: u32 = 15;
    pub const SEMAPHORES: u32 = 16;
    pub const MUTANTS: u32 = 17;
    pub const TIMERS: u32 = 18;
    pub const SECTIONS: u32 = 19;
    pub const FILES: u32 = 20;
    pub const IO_COMPLETIONS: u32 = 21;
    /// Thread objects of threads that have exited (handles to them).
    pub const EXITED_THREADS: u32 = 22;
    /// Process objects (each an object of its own, naming a process).
    pub const PROCESS_OBJECTS: u32 = 23;
    /// Console input and output objects (no state).
    pub const CONSOLES: u32 = 24;
    /// Handle tables.
    pub const HANDLES: u32 = 25;
}

/// What a kept physical range is (KEPT section).
pub mod purpose {
    /// Handover scaffolding: trampoline, parked CPUs, page tables, the new
    /// kernel's boot stack. Free once the kernel that got it hands over.
    pub const SCAFFOLD: u32 = 1;
    /// The record itself (same lifetime).
    pub const RECORD: u32 = 2;
    /// The running kernel's image (same lifetime).
    pub const KERNEL_IMAGE: u32 = 3;
    /// A page that must survive every handover (the test's marker).
    pub const MARKER: u32 = 4;
    /// Room for the next kernel's frame bitmap, taken from free frames so
    /// that it cannot land on user pages (USER_FRAMES). Free once that
    /// kernel hands over.
    pub const BITMAP: u32 = 5;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordError {
    TooShort,
    BadMagic,
    Version(u32),
    Length,
    Crc { stored: u32, computed: u32 },
    Section { index: u32 },
    UnknownRequired { tag: u32 },
}

impl core::fmt::Display for RecordError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RecordError::TooShort => f.write_str("shorter than its header"),
            RecordError::BadMagic => f.write_str("no GALAHAND magic"),
            RecordError::Version(v) => {
                write!(f, "format version {v} (this kernel reads {MIN_VERSION} to {VERSION})")
            }
            RecordError::Length => f.write_str("bad length"),
            RecordError::Crc { stored, computed } => {
                write!(f, "CRC {stored:#010x} does not match the contents ({computed:#010x})")
            }
            RecordError::Section { index } => write!(f, "section {index} is damaged"),
            RecordError::UnknownRequired { tag } => {
                write!(f, "section {tag} is required and this kernel does not know it")
            }
        }
    }
}

/// The tags this kernel knows (unknown ones are skipped unless REQUIRED).
pub const KNOWN: [u32; 25] = [
    tag::MEMORY_MAP,
    tag::PLATFORM,
    tag::FRAMEBUFFER,
    tag::CMDLINE,
    tag::MODULES,
    tag::CLOCK,
    tag::CPUS,
    tag::VOLUME,
    tag::KEPT,
    tag::KERNEL_FILE,
    tag::USER_FRAMES,
    tag::PROCESSES,
    tag::ADDRESS_SPACES,
    tag::THREADS,
    tag::EVENTS,
    tag::SEMAPHORES,
    tag::MUTANTS,
    tag::TIMERS,
    tag::SECTIONS,
    tag::FILES,
    tag::IO_COMPLETIONS,
    tag::EXITED_THREADS,
    tag::PROCESS_OBJECTS,
    tag::CONSOLES,
    tag::HANDLES,
];

fn get<const N: usize>(bytes: &[u8], at: usize) -> Option<[u8; N]> {
    bytes.get(at..at.checked_add(N)?)?.try_into().ok()
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    get(bytes, at).map(u32::from_le_bytes)
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    get(bytes, at).map(u64::from_le_bytes)
}

/// Builds a record.
pub struct Writer {
    buf: Vec<u8>,
    sections: u32,
    /// Start of the open section's header.
    open: Option<usize>,
}

impl Writer {
    pub fn new(generation: u64) -> Result<Self, AllocError> {
        let mut w = Writer { buf: Vec::new(), sections: 0, open: None };
        w.buf.try_extend_from_slice(&[0; HEADER_LEN])?;
        w.buf[0..8].copy_from_slice(&MAGIC);
        w.buf[8..12].copy_from_slice(&VERSION.to_le_bytes());
        w.buf[12..16].copy_from_slice(&(HEADER_LEN as u32).to_le_bytes());
        w.buf[32..40].copy_from_slice(&generation.to_le_bytes());
        Ok(w)
    }

    /// Starts a section; its fields follow (`u32`, `u64`, `bytes`).
    pub fn section(&mut self, tag: u32, version: u16, flags: u16) -> Result<(), AllocError> {
        self.close()?;
        self.open = Some(self.buf.len());
        let mut header = [0u8; SECTION_HEADER_LEN];
        header[0..4].copy_from_slice(&tag.to_le_bytes());
        header[4..6].copy_from_slice(&version.to_le_bytes());
        header[6..8].copy_from_slice(&flags.to_le_bytes());
        self.buf.try_extend_from_slice(&header)?;
        self.sections += 1;
        Ok(())
    }

    /// Ends the open section: its length, then padding to 8.
    fn close(&mut self) -> Result<(), AllocError> {
        let Some(start) = self.open.take() else { return Ok(()) };
        let len = (self.buf.len() - start - SECTION_HEADER_LEN) as u32;
        self.buf[start + 8..start + 12].copy_from_slice(&len.to_le_bytes());
        let pad = (8 - self.buf.len() % 8) % 8;
        self.buf.try_extend_from_slice(&[0; 8][..pad])
    }

    pub fn u32(&mut self, v: u32) -> Result<(), AllocError> {
        self.buf.try_extend_from_slice(&v.to_le_bytes())
    }

    pub fn u64(&mut self, v: u64) -> Result<(), AllocError> {
        self.buf.try_extend_from_slice(&v.to_le_bytes())
    }

    pub fn bytes(&mut self, v: &[u8]) -> Result<(), AllocError> {
        self.buf.try_extend_from_slice(v)
    }

    /// Starts an entry of a list section: a u32 length (filled in by
    /// `end_entry`), then its fields. Entries let a list's items gain
    /// fields at their end, as sections do (`Reader::entry`).
    pub fn begin_entry(&mut self) -> Result<usize, AllocError> {
        let at = self.buf.len();
        self.u32(0)?;
        Ok(at)
    }

    /// A UTF-16 string field: its length in code units (u32), the units,
    /// padded to 4 bytes.
    pub fn wide(&mut self, s: &[u16]) -> Result<(), AllocError> {
        self.u32(s.len() as u32)?;
        for &c in s {
            self.bytes(&c.to_le_bytes())?;
        }
        let pad = (4 - (s.len() * 2) % 4) % 4;
        self.bytes(&[0; 4][..pad])
    }

    /// Starts a table: `count` records of `stride` bytes each follow
    /// (`Reader::table`). Records only gain fields at their end, the
    /// stride telling a reader where the next one starts.
    pub fn table(&mut self, count: u32, stride: u32) -> Result<(), AllocError> {
        self.u32(count)?;
        self.u32(stride)
    }

    /// Ends the entry `begin_entry` started at `at`, padded to 4.
    pub fn end_entry(&mut self, at: usize) -> Result<(), AllocError> {
        let pad = (4 - self.buf.len() % 4) % 4;
        self.bytes(&[0; 4][..pad])?;
        let len = (self.buf.len() - at - 4) as u32;
        self.buf[at..at + 4].copy_from_slice(&len.to_le_bytes());
        Ok(())
    }

    /// A string field: length (u32), bytes, padded to 4.
    pub fn str(&mut self, s: &str) -> Result<(), AllocError> {
        self.u32(s.len() as u32)?;
        self.bytes(s.as_bytes())?;
        let pad = (4 - s.len() % 4) % 4;
        self.bytes(&[0; 4][..pad])
    }

    /// The finished record: lengths, section count and CRC filled in.
    pub fn finish(mut self) -> Result<Vec<u8>, AllocError> {
        self.close()?;
        let total = self.buf.len() as u64;
        self.buf[16..24].copy_from_slice(&total.to_le_bytes());
        self.buf[40..44].copy_from_slice(&self.sections.to_le_bytes());
        let crc = crc32(0, &self.buf);
        self.buf[24..28].copy_from_slice(&crc.to_le_bytes());
        Ok(self.buf)
    }
}

/// The length a record claims in its header (to know how much to map
/// before parsing), if the header looks like one.
pub fn claimed_len(header: &[u8]) -> Option<u64> {
    if header.get(0..8)? != MAGIC {
        return None;
    }
    u64_at(header, 16).filter(|&n| n >= HEADER_LEN as u64 && n <= MAX_LEN)
}

/// A checked record.
#[derive(Clone, Copy)]
pub struct Record<'a> {
    pub version: u32,
    pub generation: u64,
    bytes: &'a [u8],
}

/// One section of a record.
#[derive(Clone, Copy, Debug)]
pub struct Section<'a> {
    pub tag: u32,
    pub version: u16,
    pub flags: u16,
    pub data: &'a [u8],
}

/// Checks a record: magic, version, lengths, CRC, the section framing,
/// and that every REQUIRED section is one this kernel knows.
pub fn parse(bytes: &[u8]) -> Result<Record<'_>, RecordError> {
    if bytes.len() < HEADER_LEN {
        return Err(RecordError::TooShort);
    }
    if bytes[0..8] != MAGIC {
        return Err(RecordError::BadMagic);
    }
    let version = u32_at(bytes, 8).ok_or(RecordError::TooShort)?;
    if !(MIN_VERSION..=VERSION).contains(&version) {
        return Err(RecordError::Version(version));
    }
    let header_len = u32_at(bytes, 12).ok_or(RecordError::TooShort)? as usize;
    let total = u64_at(bytes, 16).ok_or(RecordError::TooShort)?;
    if header_len < HEADER_LEN || total > bytes.len() as u64 || (total as usize) < header_len {
        return Err(RecordError::Length);
    }
    let bytes = &bytes[..total as usize];
    let stored = u32_at(bytes, 24).ok_or(RecordError::TooShort)?;
    let computed = {
        let crc = crc32(0, &bytes[..24]);
        let crc = crc32(crc, &[0; 4]);
        crc32(crc, &bytes[28..])
    };
    if stored != computed {
        return Err(RecordError::Crc { stored, computed });
    }
    let record = Record {
        version,
        generation: u64_at(bytes, 32).ok_or(RecordError::TooShort)?,
        bytes: &bytes[header_len..],
    };
    let count = u32_at(bytes, 40).ok_or(RecordError::TooShort)?;
    let mut seen = 0;
    for section in record.sections() {
        let section = section?;
        if section.flags & REQUIRED != 0 && !KNOWN.contains(&section.tag) {
            return Err(RecordError::UnknownRequired { tag: section.tag });
        }
        seen += 1;
    }
    if seen != count {
        return Err(RecordError::Section { index: seen });
    }
    Ok(record)
}

impl<'a> Record<'a> {
    /// The sections in order (an error for a damaged one ends the walk).
    pub fn sections(&self) -> impl Iterator<Item = Result<Section<'a>, RecordError>> + 'a {
        let bytes = self.bytes;
        let mut at = 0usize;
        let mut index = 0u32;
        core::iter::from_fn(move || {
            if at >= bytes.len() {
                return None;
            }
            let bad = RecordError::Section { index };
            let parsed = (|| {
                let tag = u32_at(bytes, at)?;
                let version = get::<2>(bytes, at + 4).map(u16::from_le_bytes)?;
                let flags = get::<2>(bytes, at + 6).map(u16::from_le_bytes)?;
                let len = u32_at(bytes, at + 8)? as usize;
                let start = at + SECTION_HEADER_LEN;
                let data = bytes.get(start..start.checked_add(len)?)?;
                Some((Section { tag, version, flags, data }, (start + len).next_multiple_of(8)))
            })();
            index += 1;
            match parsed {
                Some((section, next)) => {
                    at = next;
                    Some(Ok(section))
                }
                None => {
                    at = bytes.len();
                    Some(Err(bad))
                }
            }
        })
    }

    /// The first section with `tag`, as a field reader.
    pub fn section(&self, tag: u32) -> Option<Reader<'a>> {
        self.sections().filter_map(Result::ok).find(|s| s.tag == tag).map(|s| Reader {
            data: s.data,
            at: 0,
            version: s.version,
        })
    }

    /// Bytes of the whole record (header included are not: sections only).
    pub fn sections_len(&self) -> usize {
        self.bytes.len()
    }
}

/// Reads a section's fields in order; a field past the section's end
/// reads as 0 (written by an older kernel that did not have it yet).
#[derive(Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    at: usize,
    pub version: u16,
}

impl<'a> Reader<'a> {
    pub fn u8(&mut self) -> u8 {
        let v = self.data.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        v
    }

    pub fn u16(&mut self) -> u16 {
        let v = get::<2>(self.data, self.at).map_or(0, u16::from_le_bytes);
        self.at += 2;
        v
    }

    pub fn u32(&mut self) -> u32 {
        let v = u32_at(self.data, self.at).unwrap_or(0);
        self.at += 4;
        v
    }

    pub fn u64(&mut self) -> u64 {
        let v = u64_at(self.data, self.at).unwrap_or(0);
        self.at += 8;
        v
    }

    /// A string field (see `Writer::str`); empty if damaged or missing.
    pub fn str(&mut self) -> &'a str {
        let len = self.u32() as usize;
        let s = self.data.get(self.at..self.at.saturating_add(len)).unwrap_or(&[]);
        self.at = self.at.saturating_add(len.next_multiple_of(4));
        core::str::from_utf8(s).unwrap_or("")
    }

    /// A UTF-16 string field (`Writer::wide`), as its little-endian bytes;
    /// empty if damaged or missing.
    pub fn wide(&mut self) -> Wide<'a> {
        let units = self.u32() as usize;
        let len = units.saturating_mul(2);
        let s = self.data.get(self.at..self.at.saturating_add(len)).unwrap_or(&[]);
        self.at = self.at.saturating_add(len.next_multiple_of(4));
        Wide(s)
    }

    /// A table (`Writer::table`): its records, each read by a reader of
    /// its own. Records past the section's end are not there.
    pub fn table(&mut self) -> Table<'a> {
        let (count, stride) = (self.u32() as usize, self.u32() as usize);
        let len = count.saturating_mul(stride);
        let data = self.data.get(self.at..).unwrap_or(&[]);
        let data = &data[..len.min(data.len())];
        self.at = self.at.saturating_add(len);
        Table { data, stride, version: self.version }
    }

    /// Fixed-size bytes (zeros past the end).
    pub fn array<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0u8; N];
        if let Some(b) = self.data.get(self.at..) {
            let n = b.len().min(N);
            out[..n].copy_from_slice(&b[..n]);
        }
        self.at = self.at.saturating_add(N);
        out
    }

    /// The next entry (`Writer::begin_entry`) as a reader of its own
    /// fields; None at the end. A length past the section's end is damage
    /// (the CRC makes that unlikely): the walk ends there.
    pub fn entry(&mut self) -> Option<Reader<'a>> {
        if self.at_end() {
            return None;
        }
        let len = self.u32() as usize;
        let data = self.data.get(self.at..self.at.checked_add(len)?)?;
        self.at += len;
        Some(Reader { data, at: 0, version: self.version })
    }

    /// The rest of the section.
    pub fn rest(&self) -> &'a [u8] {
        self.data.get(self.at..).unwrap_or(&[])
    }

    /// Whether every field has been read.
    pub fn at_end(&self) -> bool {
        self.at >= self.data.len()
    }
}

/// A UTF-16 string in a record (little-endian bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wide<'a>(pub &'a [u8]);

impl Wide<'_> {
    pub fn units(&self) -> impl Iterator<Item = u16> + '_ {
        self.0.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]))
    }

    pub fn is_empty(&self) -> bool {
        self.0.len() < 2
    }

    /// The units in a vector.
    pub fn to_vec(&self) -> Result<Vec<u16>, AllocError> {
        let mut v = Vec::new();
        v.try_reserve_exact(self.0.len() / 2).map_err(|_| AllocError)?;
        v.extend(self.units());
        Ok(v)
    }
}

/// Fixed-size records (`Writer::table`).
#[derive(Clone, Copy)]
pub struct Table<'a> {
    data: &'a [u8],
    stride: usize,
    version: u16,
}

impl<'a> Table<'a> {
    /// The whole records present.
    pub fn len(&self) -> usize {
        self.data.len().checked_div(self.stride).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = Reader<'a>> + 'a {
        let (data, version) = (self.data, self.version);
        data.chunks_exact(self.stride.max(1)).take(self.len()).map(move |data| Reader {
            data,
            at: 0,
            version,
        })
    }
}

/// The CLOCK section (version 2; version 1 had no `timer_hz`): enough
/// for the next kernel's clock to go on from this one's (ARCHITECTURE
/// invariant 5: one clock).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clock {
    /// TSC ticks per second, as calibrated at the first boot.
    pub tsc_hz: u64,
    /// The TSC value at the clock's zero (the first boot's clock start).
    pub tsc_at_zero: u64,
    /// Timer ticks counted so far.
    pub ticks: u64,
    /// The clock (ns) when the record was written.
    pub monotonic_ns: u64,
    /// NT system time at the clock's zero.
    pub boot_system_time: u64,
    /// The local APIC timer's rate as calibrated at the first boot (0:
    /// unknown, version 1): the next kernel ticks at the same rate, so the
    /// tick count (the waits' deadlines) and the TSC agree across kernels.
    pub timer_hz: u64,
}

impl Clock {
    pub const VERSION: u16 = 2;

    pub fn write(&self, w: &mut Writer) -> Result<(), AllocError> {
        w.section(tag::CLOCK, Self::VERSION, 0)?;
        for v in [self.tsc_hz, self.tsc_at_zero, self.ticks, self.monotonic_ns] {
            w.u64(v)?;
        }
        w.u64(self.boot_system_time)?;
        w.u64(self.timer_hz)
    }

    pub fn read(r: &mut Reader<'_>) -> Self {
        Clock {
            tsc_hz: r.u64(),
            tsc_at_zero: r.u64(),
            ticks: r.u64(),
            monotonic_ns: r.u64(),
            boot_system_time: r.u64(),
            timer_hz: r.u64(),
        }
    }
}

/// The PLATFORM section (version 1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Platform {
    pub hhdm_offset: u64,
    /// Where the new kernel's image is (physical) and is linked (virtual).
    pub kernel_phys: u64,
    pub kernel_virt: u64,
    pub kernel_len: u64,
    /// Limine's firmware type number + 1 (0: unknown).
    pub firmware: u64,
    /// UNIX time read at the first boot + 1 (0: none).
    pub boot_unix_time: u64,
}

impl Platform {
    pub const VERSION: u16 = 1;

    pub fn write(&self, w: &mut Writer) -> Result<(), AllocError> {
        w.section(tag::PLATFORM, Self::VERSION, 0)?;
        for v in [
            self.hhdm_offset,
            self.kernel_phys,
            self.kernel_virt,
            self.kernel_len,
            self.firmware,
            self.boot_unix_time,
        ] {
            w.u64(v)?;
        }
        Ok(())
    }

    pub fn read(r: &mut Reader<'_>) -> Self {
        Platform {
            hhdm_offset: r.u64(),
            kernel_phys: r.u64(),
            kernel_virt: r.u64(),
            kernel_len: r.u64(),
            firmware: r.u64(),
            boot_unix_time: r.u64(),
        }
    }
}

/// The FRAMEBUFFER section (version 1): Limine's framebuffer fields, with
/// the address physical.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Framebuffer {
    pub phys: u64,
    pub width: u64,
    pub height: u64,
    pub pitch: u64,
    pub bpp: u16,
    pub memory_model: u8,
    /// (size, shift) of red, green, blue.
    pub masks: [u8; 6],
}

impl Framebuffer {
    pub const VERSION: u16 = 1;

    pub fn write(&self, w: &mut Writer) -> Result<(), AllocError> {
        w.section(tag::FRAMEBUFFER, Self::VERSION, 0)?;
        for v in [self.phys, self.width, self.height, self.pitch] {
            w.u64(v)?;
        }
        let m = self.masks;
        w.bytes(&self.bpp.to_le_bytes())?;
        w.bytes(&[self.memory_model, m[0], m[1], m[2], m[3], m[4], m[5]])?;
        w.bytes(&[0])
    }

    pub fn read(r: &mut Reader<'_>) -> Self {
        let (phys, width, height, pitch) = (r.u64(), r.u64(), r.u64(), r.u64());
        let (bpp, memory_model) = (r.u16(), r.u8());
        let masks = [r.u8(), r.u8(), r.u8(), r.u8(), r.u8(), r.u8()];
        Framebuffer { phys, width, height, pitch, bpp, memory_model, masks }
    }
}

/// The CPUS section (version 1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cpus {
    pub bsp_lapic_id: u32,
    /// Entries in the parked CPUs' array (the boot CPU's included).
    pub count: u32,
    /// Physical address of the array: Limine's `limine_mp_info` layout
    /// (32 bytes each), each AP spinning on its `goto_address`.
    pub parked_phys: u64,
}

impl Cpus {
    pub const VERSION: u16 = 1;

    pub fn write(&self, w: &mut Writer) -> Result<(), AllocError> {
        w.section(tag::CPUS, Self::VERSION, 0)?;
        w.u32(self.bsp_lapic_id)?;
        w.u32(self.count)?;
        w.u64(self.parked_phys)
    }

    pub fn read(r: &mut Reader<'_>) -> Self {
        Cpus { bsp_lapic_id: r.u32(), count: r.u32(), parked_phys: r.u64() }
    }
}

/// The VOLUME section (version 1): where the mounted volume's log was.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Volume {
    pub sectors: u64,
    pub generation: u64,
    pub area: u32,
    /// Bytes of log in the active area.
    pub tail: u64,
}

impl Volume {
    pub const VERSION: u16 = 1;

    pub fn write(&self, w: &mut Writer) -> Result<(), AllocError> {
        w.section(tag::VOLUME, Self::VERSION, 0)?;
        w.u64(self.sectors)?;
        w.u64(self.generation)?;
        w.u32(self.area)?;
        w.u32(0)?;
        w.u64(self.tail)
    }

    pub fn read(r: &mut Reader<'_>) -> Self {
        let (sectors, generation, area) = (r.u64(), r.u64(), r.u32());
        let _ = r.u32();
        Volume { sectors, generation, area, tail: r.u64() }
    }
}

/// A physical range (MEMORY_MAP entries, KEPT entries, KERNEL_FILE).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub base: u64,
    pub len: u64,
    /// MEMORY_MAP: the kind (Limine number); KEPT: the purpose.
    pub what: u64,
}

/// Writes a section of `Range`s (MEMORY_MAP, KEPT; version 1).
pub fn write_ranges(
    w: &mut Writer,
    tag: u32,
    ranges: impl Iterator<Item = Range>,
) -> Result<(), AllocError> {
    write_ranges_flagged(w, tag, 0, ranges)
}

/// `write_ranges` with section flags (USER_FRAMES is REQUIRED).
pub fn write_ranges_flagged(
    w: &mut Writer,
    tag: u32,
    flags: u16,
    ranges: impl Iterator<Item = Range>,
) -> Result<(), AllocError> {
    w.section(tag, 1, flags)?;
    for r in ranges {
        w.u64(r.base)?;
        w.u64(r.len)?;
        w.u64(r.what)?;
    }
    Ok(())
}

/// The `Range`s of a section written by `write_ranges`.
pub fn read_ranges<'a>(r: &Reader<'a>) -> impl Iterator<Item = Range> + 'a {
    r.rest().chunks_exact(24).map(|c| Range {
        base: u64_at(c, 0).unwrap_or(0),
        len: u64_at(c, 8).unwrap_or(0),
        what: u64_at(c, 16).unwrap_or(0),
    })
}

/// A boot module in a MODULES section (version 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModuleEntry<'a> {
    pub phys: u64,
    pub len: u64,
    pub path: &'a str,
    pub cmdline: &'a str,
}

pub fn write_modules<'m>(
    w: &mut Writer,
    modules: impl Iterator<Item = ModuleEntry<'m>>,
) -> Result<(), AllocError> {
    w.section(tag::MODULES, 1, 0)?;
    for m in modules {
        w.u64(m.phys)?;
        w.u64(m.len)?;
        w.str(m.path)?;
        w.str(m.cmdline)?;
    }
    Ok(())
}

pub fn read_modules<'a>(r: &Reader<'a>) -> impl Iterator<Item = ModuleEntry<'a>> + 'a {
    let mut r = Reader { data: r.rest(), at: 0, version: r.version };
    core::iter::from_fn(move || {
        if r.at_end() {
            return None;
        }
        Some(ModuleEntry { phys: r.u64(), len: r.u64(), path: r.str(), cmdline: r.str() })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut w = Writer::new(3).unwrap();
        write_ranges(
            &mut w,
            tag::MEMORY_MAP,
            [
                Range { base: 0x1000, len: 0x9f000, what: 0 },
                Range { base: 0x100000, len: 0x7f00000, what: 0 },
            ]
            .into_iter(),
        )
        .unwrap();
        Platform {
            hhdm_offset: 0xffff_8000_0000_0000,
            kernel_phys: 0x200_0000,
            kernel_virt: 0xffff_ffff_8000_0000,
            kernel_len: 0x26_0000,
            firmware: 1,
            boot_unix_time: 1_790_000_001,
        }
        .write(&mut w)
        .unwrap();
        w.section(tag::CMDLINE, 1, 0).unwrap();
        w.bytes(b"qemu_exit handover=2").unwrap();
        write_modules(
            &mut w,
            [
                ModuleEntry { phys: 0x300_0000, len: 1234, path: "/boot/hello.exe", cmdline: "" },
                ModuleEntry { phys: 0x300_1000, len: 5, path: "/boot/a.tar", cmdline: "tar:x" },
            ]
            .into_iter(),
        )
        .unwrap();
        Clock {
            tsc_hz: 2_000_000_000,
            tsc_at_zero: 77,
            ticks: 1500,
            monotonic_ns: 1_500_000_123,
            boot_system_time: 133_000_000_000_000_000,
            timer_hz: 62_500_000,
        }
        .write(&mut w)
        .unwrap();
        Framebuffer {
            phys: 0xfd00_0000,
            width: 1280,
            height: 800,
            pitch: 5120,
            bpp: 32,
            memory_model: 1,
            masks: [8, 16, 8, 8, 8, 0],
        }
        .write(&mut w)
        .unwrap();
        Cpus { bsp_lapic_id: 0, count: 2, parked_phys: 0x400_0000 }.write(&mut w).unwrap();
        Volume { sectors: 65536, generation: 4, area: 1, tail: 9000 }.write(&mut w).unwrap();
        w.finish().unwrap()
    }

    #[test]
    fn round_trip() {
        let bytes = sample();
        assert_eq!(claimed_len(&bytes), Some(bytes.len() as u64));
        let rec = parse(&bytes).unwrap();
        assert_eq!(rec.version, VERSION);
        assert_eq!(rec.generation, 3);
        let map: Vec<_> = read_ranges(&rec.section(tag::MEMORY_MAP).unwrap()).collect();
        assert_eq!(map.len(), 2);
        assert_eq!(map[1], Range { base: 0x100000, len: 0x7f00000, what: 0 });
        let p = Platform::read(&mut rec.section(tag::PLATFORM).unwrap());
        assert_eq!(p.kernel_len, 0x26_0000);
        assert_eq!(p.boot_unix_time, 1_790_000_001);
        assert_eq!(rec.section(tag::CMDLINE).unwrap().rest(), b"qemu_exit handover=2");
        let modules: Vec<_> = read_modules(&rec.section(tag::MODULES).unwrap()).collect();
        assert_eq!(modules.len(), 2);
        assert_eq!(modules[0].path, "/boot/hello.exe");
        assert_eq!(modules[1].cmdline, "tar:x");
        assert_eq!(modules[1].len, 5);
        let c = Clock::read(&mut rec.section(tag::CLOCK).unwrap());
        assert_eq!(c.monotonic_ns, 1_500_000_123);
        assert_eq!(c.boot_system_time, 133_000_000_000_000_000);
        assert_eq!(c.timer_hz, 62_500_000);
        let f = Framebuffer::read(&mut rec.section(tag::FRAMEBUFFER).unwrap());
        assert_eq!((f.width, f.pitch, f.bpp, f.memory_model), (1280, 5120, 32, 1));
        assert_eq!(f.masks, [8, 16, 8, 8, 8, 0]);
        let cpus = Cpus::read(&mut rec.section(tag::CPUS).unwrap());
        assert_eq!(cpus, Cpus { bsp_lapic_id: 0, count: 2, parked_phys: 0x400_0000 });
        let v = Volume::read(&mut rec.section(tag::VOLUME).unwrap());
        assert_eq!(v, Volume { sectors: 65536, generation: 4, area: 1, tail: 9000 });
        assert!(rec.section(tag::KEPT).is_none());
    }

    #[test]
    fn tables_and_wide_strings() {
        let mut w = Writer::new(1).unwrap();
        w.section(tag::HANDLES, 1, REQUIRED).unwrap();
        w.wide(&[0x41, 0x42, 0x43]).unwrap();
        // A newer writer's 12-byte records; this reader knows 8 bytes.
        w.table(2, 12).unwrap();
        for v in [5u64, 6] {
            w.u64(v).unwrap();
            w.u32(0xffff).unwrap();
        }
        w.u32(77).unwrap();
        let bytes = w.finish().unwrap();
        let rec = parse(&bytes).unwrap();
        let mut r = rec.section(tag::HANDLES).unwrap();
        let name = r.wide();
        assert_eq!(name.to_vec().unwrap(), [0x41, 0x42, 0x43]);
        let t = r.table();
        assert_eq!(t.len(), 2);
        let values: Vec<u64> = t.iter().map(|mut e| e.u64()).collect();
        assert_eq!(values, [5, 6]);
        assert_eq!(r.u32(), 77);
        // A table claiming more records than there are bytes.
        let mut w = Writer::new(1).unwrap();
        w.section(tag::HANDLES, 1, REQUIRED).unwrap();
        w.table(1000, 8).unwrap();
        w.u64(1).unwrap();
        let bytes = w.finish().unwrap();
        let rec = parse(&bytes).unwrap();
        assert_eq!(rec.section(tag::HANDLES).unwrap().table().len(), 1);
    }

    #[test]
    fn entries_grow_at_their_end() {
        // A list section of entries: an older writer's short entry reads
        // with zeros, a newer writer's longer one skips what it adds.
        let mut w = Writer::new(1).unwrap();
        w.section(tag::PROCESSES, 1, REQUIRED).unwrap();
        let e = w.begin_entry().unwrap();
        w.u64(7).unwrap();
        w.end_entry(e).unwrap();
        let e = w.begin_entry().unwrap();
        w.u64(8).unwrap();
        w.u32(9).unwrap();
        w.str("abc").unwrap();
        w.u64(0xdead).unwrap();
        w.end_entry(e).unwrap();
        let bytes = w.finish().unwrap();
        let rec = parse(&bytes).unwrap();
        let mut r = rec.section(tag::PROCESSES).unwrap();
        let mut first = r.entry().unwrap();
        assert_eq!((first.u64(), first.u32(), first.str()), (7, 0, ""));
        let mut second = r.entry().unwrap();
        assert_eq!((second.u64(), second.u32(), second.str()), (8, 9, "abc"));
        assert!(r.entry().is_none());
        // A REQUIRED section this kernel knows is accepted.
        assert!(parse(&bytes).is_ok());
    }

    #[test]
    fn damage_is_detected() {
        let good = sample();
        // Any flipped byte (header or section) fails the CRC or the framing.
        for i in [0usize, 9, 20, 30, 33, HEADER_LEN + 3, good.len() / 2, good.len() - 1] {
            let mut bad = good.clone();
            bad[i] ^= 0x40;
            assert!(parse(&bad).is_err(), "byte {i} flipped was not noticed");
        }
        // Cut short.
        assert_eq!(parse(&good[..good.len() - 8]).err(), Some(RecordError::Length));
        assert_eq!(parse(&good[..10]).err(), Some(RecordError::TooShort));
        // A wrong CRC is reported as such.
        let mut bad = good.clone();
        bad[HEADER_LEN + 20] ^= 1;
        assert!(matches!(parse(&bad), Err(RecordError::Crc { .. })));
    }

    /// Re-seals a record after a test edited it.
    fn reseal(bytes: &mut [u8]) {
        bytes[24..28].copy_from_slice(&[0; 4]);
        let crc = crc32(0, bytes);
        bytes[24..28].copy_from_slice(&crc.to_le_bytes());
    }

    #[test]
    fn versions() {
        let mut bytes = sample();
        bytes[8..12].copy_from_slice(&(VERSION + 1).to_le_bytes());
        reseal(&mut bytes);
        assert_eq!(parse(&bytes).err(), Some(RecordError::Version(VERSION + 1)));
        bytes[8..12].copy_from_slice(&(MIN_VERSION - 1).to_le_bytes());
        reseal(&mut bytes);
        assert_eq!(parse(&bytes).err(), Some(RecordError::Version(MIN_VERSION - 1)));
        bytes[8..12].copy_from_slice(&MIN_VERSION.to_le_bytes());
        reseal(&mut bytes);
        assert!(parse(&bytes).is_ok());
    }

    #[test]
    fn older_section_reads_with_defaults() {
        // An older kernel's CLOCK section, before `boot_system_time` (and
        // a newer one with a field this kernel does not know).
        let mut w = Writer::new(1).unwrap();
        w.section(tag::CLOCK, 0, 0).unwrap();
        for v in [10u64, 20, 30, 40] {
            w.u64(v).unwrap();
        }
        w.section(tag::VOLUME, 2, 0).unwrap();
        for v in [1u64, 2, 3, 4, 0xdead] {
            w.u64(v).unwrap();
        }
        let bytes = w.finish().unwrap();
        let rec = parse(&bytes).unwrap();
        let mut r = rec.section(tag::CLOCK).unwrap();
        assert_eq!(r.version, 0);
        let c = Clock::read(&mut r);
        assert_eq!((c.tsc_hz, c.monotonic_ns, c.boot_system_time, c.timer_hz), (10, 40, 0, 0));
        let v = Volume::read(&mut rec.section(tag::VOLUME).unwrap());
        assert_eq!((v.sectors, v.generation, v.area, v.tail), (1, 2, 3, 4));
    }

    #[test]
    fn unknown_sections() {
        // Optional: skipped.
        let mut w = Writer::new(1).unwrap();
        w.section(999, 1, 0).unwrap();
        w.u64(5).unwrap();
        Cpus { bsp_lapic_id: 1, count: 4, parked_phys: 0x9000 }.write(&mut w).unwrap();
        let bytes = w.finish().unwrap();
        let rec = parse(&bytes).unwrap();
        assert_eq!(Cpus::read(&mut rec.section(tag::CPUS).unwrap()).count, 4);
        // Required: the record is refused.
        let mut w = Writer::new(1).unwrap();
        w.section(1000, 1, REQUIRED).unwrap();
        w.u32(5).unwrap();
        let bytes = w.finish().unwrap();
        assert_eq!(parse(&bytes).err(), Some(RecordError::UnknownRequired { tag: 1000 }));
    }

    #[test]
    fn sections_are_aligned() {
        let mut w = Writer::new(0).unwrap();
        w.section(tag::CMDLINE, 1, 0).unwrap();
        w.bytes(b"abc").unwrap();
        w.section(tag::CPUS, 1, 0).unwrap();
        w.u32(1).unwrap();
        let bytes = w.finish().unwrap();
        assert!(bytes.len().is_multiple_of(8));
        let rec = parse(&bytes).unwrap();
        assert_eq!(rec.section(tag::CMDLINE).unwrap().rest(), b"abc");
        assert_eq!(rec.section(tag::CPUS).unwrap().u32(), 1);
    }
}
