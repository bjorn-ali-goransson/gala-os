//! ELF64 executables for the Linux personality: the header and program
//! headers parsed and validated, and the load plan (where each PT_LOAD
//! segment goes, from which file bytes, with which protection, and the
//! bss) as data. Mapping it is the caller's (the exec path, later).
//!
//! Sources (CLEAN_ROOM.md "Linux"): the ELF specification (System V gABI)
//! and its x86-64 supplement (the psABI), and real Linux's behaviour as
//! user/lx's programs observe it (auxv_test: AT_PHDR, AT_ENTRY and the
//! load bias of a static-pie).
//!
//! Invariants of an [`ElfImage`] (checked by [`ElfImage::parse`], which
//! never panics on any input):
//! - ELFCLASS64, little-endian, EV_CURRENT, EM_X86_64, ET_EXEC or ET_DYN;
//!   the program headers lie inside the file and are at most 64 KiB.
//! - At least one PT_LOAD; each one's file bytes lie inside the file,
//!   `filesz <= memsz`, its alignment is 0, 1 or a power of two of at most
//!   1 GiB, its address and file offset agree modulo the page size (so it
//!   can be mapped from file pages), it ends below [`USER_END`] without
//!   overflow, and the PT_LOAD entries are sorted by address and do not
//!   overlap (two may share the page at their boundary: the mapper gives
//!   that page both protections).
//! - The entry point lies inside a PT_LOAD.
//! - At most one PT_INTERP (a NUL-terminated path of at most 4096 bytes,
//!   no NUL inside) and at most one PT_TLS (inside the file, `filesz <=
//!   memsz`, power-of-two alignment).
//!
//! Addresses in an [`ElfImage`] are the file's own (unbiased); a
//! [`LoadPlan`] has them biased (an ET_DYN image is placed at a base of
//! the loader's choice: [`ElfImage::load_bias`]).

use alloc::vec::Vec;
use core::fmt;
use core::ops::Range;

use crate::mm::heap::TryVec;

/// The page size of the Linux x86_64 ABI (AT_PAGESZ: 4096, auxv_test).
pub const PAGE_SIZE: u64 = 4096;

/// One past the highest user address with 4-level paging: observed on
/// Linux 6.18, ARCH_SET_FS accepts 0x7fff_ffff_e000 and refuses
/// 0x7fff_ffff_f000 (arch_prctl_test).
pub const USER_END: u64 = 0x7fff_ffff_f000;

/// Where Linux 6.18 places an ET_DYN program that has an interpreter when
/// address randomization is off (observed with `setarch -R`: AT_PHDR
/// 0x5555_5555_4040). A static-pie (no interpreter) it places like an
/// mmap of its size instead, just below the mmap base (0x7fff_f7ff_6000
/// for a small one). gala's loader uses this one base for both until it
/// randomizes: a program finds itself through AT_PHDR and AT_ENTRY, not
/// by its address.
pub const DEFAULT_DYN_BASE: u64 = 0x5555_5555_4000;

const ELF_HEADER_SIZE: usize = 64;
const PHDR_SIZE: usize = 56;
/// The most program-header bytes accepted (Linux accepts up to 64 KiB of
/// them; real executables have about 10 headers).
const MAX_PHDRS_SIZE: usize = 64 * 1024;
/// The longest interpreter path accepted (PATH_MAX, with its NUL).
const MAX_INTERP: usize = 4096;
/// The largest PT_LOAD alignment accepted (ET_DYN bases are aligned to
/// it; real ones are 4 KiB to 2 MiB).
const MAX_ALIGN: u64 = 1 << 30;

pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;
pub const EM_X86_64: u16 = 62;
pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;
pub const PT_INTERP: u32 = 3;
pub const PT_PHDR: u32 = 6;
pub const PT_TLS: u32 = 7;
pub const PT_GNU_STACK: u32 = 0x6474_e551;
pub const PT_GNU_RELRO: u32 = 0x6474_e552;
pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

/// What a malformed or unsupported image is; each carries the values that
/// explain it, for the exec path's log line and event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElfError {
    TooShort {
        len: usize,
    },
    BadMagic,
    NotElf64 {
        class: u8,
    },
    NotLittleEndian {
        data: u8,
    },
    BadVersion {
        version: u32,
    },
    NotX86_64 {
        machine: u16,
    },
    NotExecutable {
        kind: u16,
    },
    BadHeaderSize {
        ehsize: u16,
    },
    BadPhentsize {
        phentsize: u16,
    },
    TooManyPhdrs {
        phnum: u16,
    },
    PhdrsOutsideFile {
        phoff: u64,
        len: usize,
    },
    NoLoadSegment,
    SegmentOutsideFile {
        index: usize,
        offset: u64,
        filesz: u64,
    },
    FileSizeAboveMemSize {
        index: usize,
        filesz: u64,
        memsz: u64,
    },
    BadAlign {
        index: usize,
        align: u64,
    },
    Misaligned {
        index: usize,
        vaddr: u64,
        offset: u64,
    },
    AboveUserSpace {
        index: usize,
        vaddr: u64,
        memsz: u64,
    },
    NotSorted {
        index: usize,
    },
    Overlap {
        index: usize,
        prev_end: u64,
        vaddr: u64,
    },
    EntryOutside {
        entry: u64,
    },
    BadInterp {
        reason: &'static str,
    },
    TwoInterps,
    BadTls {
        reason: &'static str,
    },
    TwoTls,
    /// An ET_DYN base not aligned to the segments' alignment, or one that
    /// puts the image above user space.
    BadBase {
        base: u64,
        align: u64,
    },
    OutOfMemory,
}

impl fmt::Display for ElfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::TooShort { len } => write!(f, "{len} bytes: shorter than an ELF64 header"),
            Self::BadMagic => write!(f, "not an ELF file (no \\x7fELF)"),
            Self::NotElf64 { class } => write!(f, "EI_CLASS {class}: not ELFCLASS64"),
            Self::NotLittleEndian { data } => write!(f, "EI_DATA {data}: not little-endian"),
            Self::BadVersion { version } => write!(f, "ELF version {version}"),
            Self::NotX86_64 { machine } => write!(f, "e_machine {machine}: not x86-64 (62)"),
            Self::NotExecutable { kind } => write!(f, "e_type {kind}: not ET_EXEC or ET_DYN"),
            Self::BadHeaderSize { ehsize } => write!(f, "e_ehsize {ehsize}"),
            Self::BadPhentsize { phentsize } => write!(f, "e_phentsize {phentsize}, not 56"),
            Self::TooManyPhdrs { phnum } => write!(f, "{phnum} program headers: too many"),
            Self::PhdrsOutsideFile { phoff, len } => {
                write!(f, "program headers at {phoff:#x} outside the {len}-byte file")
            }
            Self::NoLoadSegment => write!(f, "no PT_LOAD segment"),
            Self::SegmentOutsideFile { index, offset, filesz } => {
                write!(f, "segment {index}: file bytes {offset:#x}+{filesz:#x} outside the file")
            }
            Self::FileSizeAboveMemSize { index, filesz, memsz } => {
                write!(f, "segment {index}: p_filesz {filesz:#x} above p_memsz {memsz:#x}")
            }
            Self::BadAlign { index, align } => write!(f, "segment {index}: p_align {align:#x}"),
            Self::Misaligned { index, vaddr, offset } => write!(
                f,
                "segment {index}: address {vaddr:#x} and offset {offset:#x} differ modulo the page size"
            ),
            Self::AboveUserSpace { index, vaddr, memsz } => {
                write!(f, "segment {index}: {vaddr:#x}+{memsz:#x} ends above user space")
            }
            Self::NotSorted { index } => write!(f, "PT_LOAD {index} below the one before it"),
            Self::Overlap { index, prev_end, vaddr } => write!(
                f,
                "PT_LOAD {index} at {vaddr:#x} overlaps the one before it (ending at {prev_end:#x})"
            ),
            Self::EntryOutside { entry } => {
                write!(f, "entry point {entry:#x} outside every PT_LOAD")
            }
            Self::BadInterp { reason } => write!(f, "PT_INTERP: {reason}"),
            Self::TwoInterps => write!(f, "two PT_INTERP headers"),
            Self::BadTls { reason } => write!(f, "PT_TLS: {reason}"),
            Self::TwoTls => write!(f, "two PT_TLS headers"),
            Self::BadBase { base, align } => {
                write!(f, "load base {base:#x} (alignment {align:#x}) does not fit")
            }
            Self::OutOfMemory => write!(f, "out of memory"),
        }
    }
}

/// A segment's protection (from p_flags).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Prot {
    pub read: bool,
    pub write: bool,
    pub exec: bool,
}

impl Prot {
    pub const fn from_flags(flags: u32) -> Self {
        Self { read: flags & PF_R != 0, write: flags & PF_W != 0, exec: flags & PF_X != 0 }
    }
}

impl fmt::Display for Prot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let c = |on, ch| if on { ch } else { '-' };
        write!(f, "{}{}{}", c(self.read, 'r'), c(self.write, 'w'), c(self.exec, 'x'))
    }
}

/// One PT_LOAD segment: `memsz` bytes at `vaddr`, the first `filesz` of
/// them from the file at `offset`, the rest zero (the bss).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub vaddr: u64,
    pub memsz: u64,
    pub offset: u64,
    pub filesz: u64,
    pub prot: Prot,
    /// p_align (0 and 1: none).
    pub align: u64,
}

impl Segment {
    /// The first page it touches.
    pub const fn page_start(&self) -> u64 {
        page_down(self.vaddr)
    }

    /// One past the last page it touches.
    pub const fn page_end(&self) -> u64 {
        page_up(self.vaddr + self.memsz)
    }

    /// The pages mapped from the file, as (address, length, file offset),
    /// all page-aligned; `None` for a segment with no file bytes.
    pub const fn file_pages(&self) -> Option<(u64, u64, u64)> {
        if self.filesz == 0 {
            return None;
        }
        let start = self.page_start();
        Some((start, page_up(self.vaddr + self.filesz) - start, page_down(self.offset)))
    }

    /// The bss: the bytes past the file data, which read as zero.
    pub const fn bss(&self) -> Range<u64> {
        self.vaddr + self.filesz..self.vaddr + self.memsz
    }

    /// The part of the bss inside the last file page: a mapping of that
    /// page from the file shows the file's next bytes there, so they are
    /// cleared after mapping.
    pub const fn zero_tail(&self) -> Range<u64> {
        let from = self.vaddr + self.filesz;
        if self.filesz == 0 || self.memsz == self.filesz {
            return from..from;
        }
        let to = page_up(from);
        let end = self.vaddr + self.memsz;
        from..if to < end { to } else { end }
    }

    /// Whole zero pages after the file pages (anonymous memory).
    pub const fn anon_pages(&self) -> Range<u64> {
        let from =
            if self.filesz == 0 { self.page_start() } else { page_up(self.vaddr + self.filesz) };
        let end = self.page_end();
        if from < end { from..end } else { end..end }
    }

    const fn biased(&self, bias: u64) -> Self {
        Self { vaddr: self.vaddr + bias, ..*self }
    }
}

/// The PT_TLS template: the initial image of each thread's TLS block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tls {
    pub vaddr: u64,
    pub offset: u64,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

/// The image's kind (e_type).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// ET_EXEC: loaded at its own addresses.
    Exec,
    /// ET_DYN: a static-pie, or a dynamic executable or ld.so itself;
    /// loaded at a base the loader chooses.
    Dyn,
}

/// A validated ELF64 x86-64 executable's headers (see the module's
/// invariants). Holds no reference to the file: ranges index into it.
#[derive(Debug, PartialEq, Eq)]
pub struct ElfImage {
    pub kind: Kind,
    /// e_entry (unbiased).
    pub entry: u64,
    pub phoff: u64,
    pub phentsize: u16,
    pub phnum: u16,
    /// The PT_LOAD segments, sorted by address.
    pub segments: Vec<Segment>,
    /// PT_INTERP's path in the file (without its NUL): a dynamic program,
    /// whose interpreter (ld.so) is loaded too and gets control first.
    pub interp: Option<Range<usize>>,
    /// PT_GNU_STACK's flags; `None` when absent (by the ELF convention
    /// gcc and ld follow, the program then wants an executable stack).
    pub gnu_stack: Option<Prot>,
    pub tls: Option<Tls>,
    /// PT_GNU_RELRO's range (unbiased): read-only after relocation.
    pub relro: Option<Range<u64>>,
    /// Whether the image has PT_DYNAMIC (a static-pie has one: it
    /// relocates itself).
    pub dynamic: bool,
    /// The program headers' address in memory (unbiased): PT_PHDR's, or
    /// that of the PT_LOAD whose file bytes hold them; `None` if neither.
    pub phdr_vaddr: Option<u64>,
}

/// Where everything goes for one bias: what the exec path maps, and the
/// values the initial stack's auxv gives the program.
#[derive(Debug)]
pub struct LoadPlan {
    pub bias: u64,
    /// The PT_LOAD segments at their final addresses.
    pub segments: Vec<Segment>,
    /// AT_ENTRY: where the program starts (its interpreter's entry, if it
    /// has one, is the interpreter's own plan's).
    pub entry: u64,
    /// AT_PHDR (0 when the headers are not in memory).
    pub phdr: u64,
    pub phent: u64,
    pub phnum: u64,
    /// The image's page span.
    pub start: u64,
    pub end: u64,
    /// The program break's initial value before any randomization: the
    /// page after the highest segment.
    pub brk: u64,
    pub tls: Option<Tls>,
}

impl fmt::Display for LoadPlan {
    /// One line per segment, for the exec path's log and the state dump.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bias {:#x} entry {:#x} brk {:#x}", self.bias, self.entry, self.brk)?;
        for s in &self.segments {
            write!(
                f,
                "\n  {:#x}..{:#x} {} file {:#x}+{:#x} bss {:#x}",
                s.page_start(),
                s.page_end(),
                s.prot,
                s.offset,
                s.filesz,
                s.memsz - s.filesz
            )?;
        }
        Ok(())
    }
}

const fn page_down(v: u64) -> u64 {
    v & !(PAGE_SIZE - 1)
}

/// Rounds up; callers pass values below [`USER_END`], which cannot overflow.
const fn page_up(v: u64) -> u64 {
    (v + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    b.get(at..at + 2).map_or(0, |s| u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    b.get(at..at + 8).map_or(0, |s| {
        let mut w = [0u8; 8];
        w.copy_from_slice(s);
        u64::from_le_bytes(w)
    })
}

/// Whether `offset + size` lies inside a file of `len` bytes.
fn in_file(offset: u64, size: u64, len: usize) -> bool {
    offset.checked_add(size).is_some_and(|end| end <= len as u64)
}

/// One raw program header.
struct Phdr {
    kind: u32,
    flags: u32,
    offset: u64,
    vaddr: u64,
    filesz: u64,
    memsz: u64,
    align: u64,
}

impl ElfImage {
    /// Parses and validates `file`'s headers (the module's invariants).
    pub fn parse(file: &[u8]) -> Result<Self, ElfError> {
        if file.len() < ELF_HEADER_SIZE {
            return Err(ElfError::TooShort { len: file.len() });
        }
        if file[..4] != *b"\x7fELF" {
            return Err(ElfError::BadMagic);
        }
        if file[4] != 2 {
            return Err(ElfError::NotElf64 { class: file[4] });
        }
        if file[5] != 1 {
            return Err(ElfError::NotLittleEndian { data: file[5] });
        }
        let version = u32_at(file, 20);
        if file[6] != 1 || version != 1 {
            return Err(ElfError::BadVersion {
                version: if file[6] != 1 { file[6].into() } else { version },
            });
        }
        let kind = match u16_at(file, 16) {
            ET_EXEC => Kind::Exec,
            ET_DYN => Kind::Dyn,
            kind => return Err(ElfError::NotExecutable { kind }),
        };
        let machine = u16_at(file, 18);
        if machine != EM_X86_64 {
            return Err(ElfError::NotX86_64 { machine });
        }
        let entry = u64_at(file, 24);
        let phoff = u64_at(file, 32);
        let ehsize = u16_at(file, 52);
        if usize::from(ehsize) < ELF_HEADER_SIZE {
            return Err(ElfError::BadHeaderSize { ehsize });
        }
        let phentsize = u16_at(file, 54);
        let phnum = u16_at(file, 56);
        if usize::from(phentsize) != PHDR_SIZE {
            return Err(ElfError::BadPhentsize { phentsize });
        }
        let phdrs_size = usize::from(phnum) * PHDR_SIZE;
        if phnum == 0 || phdrs_size > MAX_PHDRS_SIZE {
            return Err(ElfError::TooManyPhdrs { phnum });
        }
        if !in_file(phoff, phdrs_size as u64, file.len()) {
            return Err(ElfError::PhdrsOutsideFile { phoff, len: file.len() });
        }

        let mut image = ElfImage {
            kind,
            entry,
            phoff,
            phentsize,
            phnum,
            segments: Vec::new(),
            interp: None,
            gnu_stack: None,
            tls: None,
            relro: None,
            dynamic: false,
            phdr_vaddr: None,
        };
        let mut pt_phdr = None;
        for index in 0..usize::from(phnum) {
            let at = phoff as usize + index * PHDR_SIZE;
            let ph = Phdr {
                kind: u32_at(file, at),
                flags: u32_at(file, at + 4),
                offset: u64_at(file, at + 8),
                vaddr: u64_at(file, at + 16),
                filesz: u64_at(file, at + 32),
                memsz: u64_at(file, at + 40),
                align: u64_at(file, at + 48),
            };
            match ph.kind {
                PT_LOAD => image.add_load(index, &ph, file.len())?,
                PT_INTERP => image.set_interp(&ph, file)?,
                PT_TLS => image.set_tls(&ph, file.len())?,
                PT_GNU_STACK => image.gnu_stack = Some(Prot::from_flags(ph.flags)),
                PT_GNU_RELRO => {
                    image.relro = ph.vaddr.checked_add(ph.memsz).map(|end| ph.vaddr..end);
                }
                PT_DYNAMIC => image.dynamic = true,
                PT_PHDR => pt_phdr = Some(ph.vaddr),
                _ => {}
            }
        }
        if image.segments.is_empty() {
            return Err(ElfError::NoLoadSegment);
        }
        if !image.segments.iter().any(|s| (s.vaddr..s.vaddr + s.memsz).contains(&entry)) {
            return Err(ElfError::EntryOutside { entry });
        }
        image.phdr_vaddr = pt_phdr.or_else(|| {
            image
                .segments
                .iter()
                .find(|s| s.offset <= phoff && phoff + phdrs_size as u64 <= s.offset + s.filesz)
                .map(|s| s.vaddr + (phoff - s.offset))
        });
        Ok(image)
    }

    fn add_load(&mut self, index: usize, ph: &Phdr, len: usize) -> Result<(), ElfError> {
        let (offset, filesz, memsz, vaddr) = (ph.offset, ph.filesz, ph.memsz, ph.vaddr);
        if !in_file(offset, filesz, len) {
            return Err(ElfError::SegmentOutsideFile { index, offset, filesz });
        }
        if filesz > memsz {
            return Err(ElfError::FileSizeAboveMemSize { index, filesz, memsz });
        }
        if ph.align > 1 && (!ph.align.is_power_of_two() || ph.align > MAX_ALIGN) {
            return Err(ElfError::BadAlign { index, align: ph.align });
        }
        if vaddr % PAGE_SIZE != offset % PAGE_SIZE {
            return Err(ElfError::Misaligned { index, vaddr, offset });
        }
        if !vaddr.checked_add(memsz).is_some_and(|end| end <= USER_END) {
            return Err(ElfError::AboveUserSpace { index, vaddr, memsz });
        }
        if let Some(prev) = self.segments.last() {
            if vaddr < prev.vaddr {
                return Err(ElfError::NotSorted { index });
            }
            let prev_end = prev.vaddr + prev.memsz;
            if vaddr < prev_end {
                return Err(ElfError::Overlap { index, prev_end, vaddr });
            }
        }
        let segment = Segment {
            vaddr,
            memsz,
            offset,
            filesz,
            prot: Prot::from_flags(ph.flags),
            align: ph.align,
        };
        self.segments.try_push(segment).map_err(|_| ElfError::OutOfMemory)
    }

    fn set_interp(&mut self, ph: &Phdr, file: &[u8]) -> Result<(), ElfError> {
        if self.interp.is_some() {
            return Err(ElfError::TwoInterps);
        }
        if ph.filesz < 2 || ph.filesz > MAX_INTERP as u64 {
            return Err(ElfError::BadInterp { reason: "empty or longer than PATH_MAX" });
        }
        if !in_file(ph.offset, ph.filesz, file.len()) {
            return Err(ElfError::BadInterp { reason: "outside the file" });
        }
        let start = ph.offset as usize;
        let end = start + ph.filesz as usize - 1;
        if file[end] != 0 {
            return Err(ElfError::BadInterp { reason: "not NUL-terminated" });
        }
        if file[start..end].contains(&0) {
            return Err(ElfError::BadInterp { reason: "NUL inside the path" });
        }
        self.interp = Some(start..end);
        Ok(())
    }

    fn set_tls(&mut self, ph: &Phdr, len: usize) -> Result<(), ElfError> {
        if self.tls.is_some() {
            return Err(ElfError::TwoTls);
        }
        if !in_file(ph.offset, ph.filesz, len) {
            return Err(ElfError::BadTls { reason: "template outside the file" });
        }
        if ph.filesz > ph.memsz {
            return Err(ElfError::BadTls { reason: "p_filesz above p_memsz" });
        }
        if ph.align > 1 && (!ph.align.is_power_of_two() || ph.align > MAX_ALIGN) {
            return Err(ElfError::BadTls { reason: "bad alignment" });
        }
        if ph.memsz > USER_END {
            return Err(ElfError::BadTls { reason: "too large" });
        }
        self.tls = Some(Tls {
            vaddr: ph.vaddr,
            offset: ph.offset,
            filesz: ph.filesz,
            memsz: ph.memsz,
            align: ph.align.max(1),
        });
        Ok(())
    }

    /// The interpreter's path (without its NUL), if the image has one.
    pub fn interp<'a>(&self, file: &'a [u8]) -> Option<&'a [u8]> {
        self.interp.clone().and_then(|r| file.get(r))
    }

    /// The image's page span (unbiased): the first segment's first page to
    /// one past the last segment's last page.
    pub fn span(&self) -> Range<u64> {
        let start = self.segments.first().map_or(0, Segment::page_start);
        let end = self.segments.last().map_or(0, Segment::page_end);
        start..end
    }

    /// The largest PT_LOAD alignment, at least a page.
    pub fn max_align(&self) -> u64 {
        self.segments.iter().map(|s| s.align).fold(PAGE_SIZE, u64::max)
    }

    /// The bias for an image whose lowest page is placed at `base`: 0 for
    /// ET_EXEC (`base` is ignored), `base - span.start` for ET_DYN, whose
    /// `base` must be aligned to [`Self::max_align`] and leave the whole
    /// image below [`USER_END`].
    pub fn load_bias(&self, base: u64) -> Result<u64, ElfError> {
        if self.kind == Kind::Exec {
            return Ok(0);
        }
        let span = self.span();
        let align = self.max_align();
        let size = span.end - span.start;
        if !base.is_multiple_of(align) || !base.checked_add(size).is_some_and(|end| end <= USER_END)
        {
            return Err(ElfError::BadBase { base, align });
        }
        Ok(base - span.start)
    }

    /// The load plan for `bias` (from [`Self::load_bias`]; 0 for ET_EXEC).
    pub fn plan(&self, bias: u64) -> Result<LoadPlan, ElfError> {
        let span = self.span();
        if self.kind == Kind::Exec && bias != 0
            || !span.end.checked_add(bias).is_some_and(|end| end <= USER_END)
        {
            return Err(ElfError::BadBase {
                base: span.start.wrapping_add(bias),
                align: self.max_align(),
            });
        }
        let mut segments = Vec::new();
        segments.try_reserve_exact(self.segments.len()).map_err(|_| ElfError::OutOfMemory)?;
        for s in &self.segments {
            segments.try_push(s.biased(bias)).map_err(|_| ElfError::OutOfMemory)?;
        }
        Ok(LoadPlan {
            bias,
            segments,
            entry: self.entry + bias,
            phdr: self.phdr_vaddr.map_or(0, |v| v.wrapping_add(bias)),
            phent: u64::from(self.phentsize),
            phnum: u64::from(self.phnum),
            start: span.start + bias,
            end: span.end + bias,
            brk: span.end + bias,
            tls: self.tls.map(|t| Tls { vaddr: t.vaddr.wrapping_add(bias), ..t }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO: &[u8] = include_bytes!("../../../user/lx/bin/hello_test");
    const HELLO_PIE: &[u8] = include_bytes!("../../../user/lx/bin/hello_test-pie");
    const TLS: &[u8] = include_bytes!("../../../user/lx/bin/arch_prctl_test");
    const TLS_PIE: &[u8] = include_bytes!("../../../user/lx/bin/arch_prctl_test-pie");
    const ALL: &[&[u8]] = &[
        HELLO,
        HELLO_PIE,
        TLS,
        TLS_PIE,
        include_bytes!("../../../user/lx/bin/auxv_test"),
        include_bytes!("../../../user/lx/bin/auxv_test-pie"),
        include_bytes!("../../../user/lx/bin/brk_test"),
        include_bytes!("../../../user/lx/bin/brk_test-pie"),
        include_bytes!("../../../user/lx/bin/mmap_test"),
        include_bytes!("../../../user/lx/bin/mmap_test-pie"),
        include_bytes!("../../../user/lx/bin/write_exit_test"),
        include_bytes!("../../../user/lx/bin/write_exit_test-pie"),
    ];

    /// The checked-in musl programs: static (ET_EXEC at 0x400000) and
    /// static-pie (ET_DYN at 0, with PT_DYNAMIC), no interpreter, a
    /// non-executable stack, text r-x and data rw- with a bss.
    #[test]
    fn parses_the_musl_test_programs() {
        for (i, &file) in ALL.iter().enumerate() {
            let image = ElfImage::parse(file).unwrap_or_else(|e| panic!("binary {i}: {e}"));
            let pie = i % 2 == 1;
            assert_eq!(image.kind, if pie { Kind::Dyn } else { Kind::Exec }, "{i}");
            assert_eq!(image.dynamic, pie, "{i}");
            assert_eq!(image.interp(file), None);
            assert_eq!(image.gnu_stack, Some(Prot { read: true, write: true, exec: false }));
            assert_eq!(image.span().start, if pie { 0 } else { 0x40_0000 });
            assert!(image.relro.is_some());
            let text = image.segments.iter().find(|s| s.prot.exec).unwrap();
            assert!(text.vaddr <= image.entry && image.entry < text.vaddr + text.memsz);
            assert!(!text.prot.write);
            let data = image.segments.last().unwrap();
            assert_eq!(data.prot, Prot { read: true, write: true, exec: false });
            assert!(data.memsz > data.filesz, "{i}: a bss");
            // The headers are in the first segment, right after the ELF header.
            assert_eq!(image.phdr_vaddr, Some(image.segments[0].vaddr + 64));
            assert_eq!(image.phoff, 64);
        }
    }

    #[test]
    fn hello_test_plan() {
        let image = ElfImage::parse(HELLO).unwrap();
        assert_eq!(image.segments.len(), 4);
        let prots: Vec<_> = image.segments.iter().map(|s| s.prot.to_string()).collect();
        assert_eq!(prots, ["r--", "r-x", "r--", "rw-"]);
        assert_eq!(image.load_bias(0x1234_0000), Ok(0), "ET_EXEC ignores the base");
        let plan = image.plan(0).unwrap();
        assert_eq!(plan.entry, image.entry);
        assert_eq!(plan.phdr, 0x40_0040);
        assert_eq!((plan.phent, plan.phnum), (56, u64::from(image.phnum)));
        assert_eq!(plan.start, 0x40_0000);
        assert_eq!(plan.brk, plan.end);
        assert_eq!(plan.end % PAGE_SIZE, 0);
        assert!(image.plan(0x1000).is_err(), "ET_EXEC cannot move");
        let data = plan.segments[3];
        let (addr, len, off) = data.file_pages().unwrap();
        assert_eq!(addr, page_down(data.vaddr));
        assert_eq!(off, page_down(data.offset));
        assert_eq!(addr + len, page_up(data.vaddr + data.filesz));
        assert_eq!(data.zero_tail().start, data.vaddr + data.filesz);
        assert!(data.zero_tail().end <= addr + len);
        let mut text = alloc::string::String::new();
        core::fmt::write(&mut text, format_args!("{plan}")).unwrap();
        assert!(text.contains("r-x file 0x1000"), "{text}");
    }

    #[test]
    fn static_pie_bias() {
        let image = ElfImage::parse(HELLO_PIE).unwrap();
        assert_eq!(image.max_align(), PAGE_SIZE);
        let bias = image.load_bias(DEFAULT_DYN_BASE).unwrap();
        assert_eq!(bias, DEFAULT_DYN_BASE);
        let plan = image.plan(bias).unwrap();
        assert_eq!(plan.entry, DEFAULT_DYN_BASE + image.entry);
        assert_eq!(plan.phdr, DEFAULT_DYN_BASE + 64);
        assert_eq!(plan.segments[1].vaddr, DEFAULT_DYN_BASE + image.segments[1].vaddr);
        assert_eq!(
            image.load_bias(DEFAULT_DYN_BASE + 8),
            Err(ElfError::BadBase { base: DEFAULT_DYN_BASE + 8, align: 4096 })
        );
        assert!(image.load_bias(USER_END - PAGE_SIZE).is_err(), "above user space");
        assert!(image.plan(u64::MAX - 0xfff).is_err());
    }

    #[test]
    fn tls_template() {
        for file in [TLS, TLS_PIE] {
            let image = ElfImage::parse(file).unwrap();
            let tls = image.tls.unwrap();
            // `__thread int tls_value = 1234; __thread int tls_zero;`
            assert_eq!((tls.filesz, tls.memsz, tls.align), (4, 8, 4));
            let at = tls.offset as usize;
            assert_eq!(&file[at..at + 4], &1234u32.to_le_bytes());
            let plan = image.plan(image.load_bias(DEFAULT_DYN_BASE).unwrap()).unwrap();
            assert_eq!(plan.tls.unwrap().vaddr, tls.vaddr + plan.bias);
        }
        assert_eq!(ElfImage::parse(HELLO).unwrap().tls, None);
    }

    // Synthetic images for what the real ones do not have.

    struct Ph {
        kind: u32,
        flags: u32,
        offset: u64,
        vaddr: u64,
        filesz: u64,
        memsz: u64,
        align: u64,
    }

    const fn load(vaddr: u64, offset: u64, filesz: u64, memsz: u64, flags: u32) -> Ph {
        Ph { kind: PT_LOAD, flags, offset, vaddr, filesz, memsz, align: 0x1000 }
    }

    /// An ELF header and `phdrs` at 64, padded to `size` bytes.
    fn elf(kind: u16, entry: u64, phdrs: &[Ph], size: usize) -> Vec<u8> {
        let mut f = vec![0u8; size.max(64 + 56 * phdrs.len())];
        f[..4].copy_from_slice(b"\x7fELF");
        f[4] = 2;
        f[5] = 1;
        f[6] = 1;
        f[16..18].copy_from_slice(&kind.to_le_bytes());
        f[18..20].copy_from_slice(&EM_X86_64.to_le_bytes());
        f[20..24].copy_from_slice(&1u32.to_le_bytes());
        f[24..32].copy_from_slice(&entry.to_le_bytes());
        f[32..40].copy_from_slice(&64u64.to_le_bytes());
        f[52..54].copy_from_slice(&64u16.to_le_bytes());
        f[54..56].copy_from_slice(&56u16.to_le_bytes());
        f[56..58].copy_from_slice(&(phdrs.len() as u16).to_le_bytes());
        for (i, p) in phdrs.iter().enumerate() {
            let at = 64 + i * 56;
            f[at..at + 4].copy_from_slice(&p.kind.to_le_bytes());
            f[at + 4..at + 8].copy_from_slice(&p.flags.to_le_bytes());
            f[at + 8..at + 16].copy_from_slice(&p.offset.to_le_bytes());
            f[at + 16..at + 24].copy_from_slice(&p.vaddr.to_le_bytes());
            f[at + 24..at + 32].copy_from_slice(&p.vaddr.to_le_bytes());
            f[at + 32..at + 40].copy_from_slice(&p.filesz.to_le_bytes());
            f[at + 40..at + 48].copy_from_slice(&p.memsz.to_le_bytes());
            f[at + 48..at + 56].copy_from_slice(&p.align.to_le_bytes());
        }
        f
    }

    fn one(phdrs: &[Ph]) -> Result<ElfImage, ElfError> {
        ElfImage::parse(&elf(ET_EXEC, 0x40_1000, phdrs, 0x3000))
    }

    #[test]
    fn interpreter_path() {
        let mut f = elf(
            ET_DYN,
            0x1000,
            &[
                Ph {
                    kind: PT_INTERP,
                    flags: PF_R,
                    offset: 0x200,
                    vaddr: 0x200,
                    filesz: 28,
                    memsz: 28,
                    align: 1,
                },
                load(0, 0, 0x2000, 0x2000, PF_R | PF_X),
            ],
            0x2000,
        );
        f[0x200..0x200 + 28].copy_from_slice(b"/lib/ld-musl-x86_64.so.1\0\0\0\0");
        assert_eq!(ElfImage::parse(&f), Err(ElfError::BadInterp { reason: "NUL inside the path" }));
        f[0x200..0x200 + 28].copy_from_slice(b"/lib64/ld-linux-x86-64.so.2\0");
        let image = ElfImage::parse(&f).unwrap();
        assert_eq!(image.interp(&f), Some(&b"/lib64/ld-linux-x86-64.so.2"[..]));
        f[0x200 + 27] = b'x';
        assert_eq!(ElfImage::parse(&f), Err(ElfError::BadInterp { reason: "not NUL-terminated" }));
    }

    #[test]
    fn rejects_bad_headers() {
        let good =
            elf(ET_EXEC, 0x40_1000, &[load(0x40_0000, 0, 0x2000, 0x2000, PF_R | PF_X)], 0x2000);
        assert!(ElfImage::parse(&good).is_ok());
        let with = |at: usize, bytes: &[u8]| {
            let mut f = good.clone();
            f[at..at + bytes.len()].copy_from_slice(bytes);
            ElfImage::parse(&f)
        };
        assert_eq!(ElfImage::parse(&good[..63]), Err(ElfError::TooShort { len: 63 }));
        assert_eq!(ElfImage::parse(&[]), Err(ElfError::TooShort { len: 0 }));
        assert_eq!(with(0, b"MZ"), Err(ElfError::BadMagic));
        assert_eq!(with(4, &[1]), Err(ElfError::NotElf64 { class: 1 }));
        assert_eq!(with(5, &[2]), Err(ElfError::NotLittleEndian { data: 2 }));
        assert_eq!(with(6, &[0]), Err(ElfError::BadVersion { version: 0 }));
        assert_eq!(with(16, &1u16.to_le_bytes()), Err(ElfError::NotExecutable { kind: 1 }));
        assert_eq!(with(16, &4u16.to_le_bytes()), Err(ElfError::NotExecutable { kind: 4 }));
        assert_eq!(with(18, &183u16.to_le_bytes()), Err(ElfError::NotX86_64 { machine: 183 }));
        assert_eq!(with(52, &32u16.to_le_bytes()), Err(ElfError::BadHeaderSize { ehsize: 32 }));
        assert_eq!(with(54, &32u16.to_le_bytes()), Err(ElfError::BadPhentsize { phentsize: 32 }));
        assert_eq!(with(56, &0u16.to_le_bytes()), Err(ElfError::TooManyPhdrs { phnum: 0 }));
        assert_eq!(
            with(56, &0xffffu16.to_le_bytes()),
            Err(ElfError::TooManyPhdrs { phnum: 0xffff })
        );
        assert_eq!(
            with(56, &200u16.to_le_bytes()),
            Err(ElfError::PhdrsOutsideFile { phoff: 64, len: 0x2000 })
        );
        assert!(matches!(
            with(32, &u64::MAX.to_le_bytes()),
            Err(ElfError::PhdrsOutsideFile { .. })
        ));
        assert_eq!(
            with(24, &0x50_0000u64.to_le_bytes()),
            Err(ElfError::EntryOutside { entry: 0x50_0000 })
        );
        // Truncated anywhere: an error, never a panic.
        for len in 0..good.len() {
            assert!(ElfImage::parse(&good[..len]).is_err() || len >= 0x2000);
        }
    }

    #[test]
    fn rejects_bad_segments() {
        const RX: u32 = PF_R | PF_X;
        assert_eq!(one(&[]), Err(ElfError::TooManyPhdrs { phnum: 0 }));
        let note = Ph { kind: 4, flags: 0, offset: 0, vaddr: 0, filesz: 0, memsz: 0, align: 4 };
        assert_eq!(one(&[note]), Err(ElfError::NoLoadSegment));
        assert_eq!(
            one(&[load(0x40_0000, 0, 0x3001, 0x4000, RX)]),
            Err(ElfError::SegmentOutsideFile { index: 0, offset: 0, filesz: 0x3001 })
        );
        assert!(matches!(
            one(&[load(0x40_0000, u64::MAX, 2, 0x4000, RX)]),
            Err(ElfError::SegmentOutsideFile { .. })
        ));
        assert_eq!(
            one(&[load(0x40_0000, 0, 0x2000, 0x1000, RX)]),
            Err(ElfError::FileSizeAboveMemSize { index: 0, filesz: 0x2000, memsz: 0x1000 })
        );
        let mut odd = load(0x40_0000, 0, 0x2000, 0x2000, RX);
        odd.align = 0x3000;
        assert_eq!(one(&[odd]), Err(ElfError::BadAlign { index: 0, align: 0x3000 }));
        let mut huge = load(0x40_0000, 0, 0x2000, 0x2000, RX);
        huge.align = 1 << 40;
        assert_eq!(one(&[huge]), Err(ElfError::BadAlign { index: 0, align: 1 << 40 }));
        assert_eq!(
            one(&[load(0x40_0010, 0, 0x2000, 0x2000, RX)]),
            Err(ElfError::Misaligned { index: 0, vaddr: 0x40_0010, offset: 0 })
        );
        assert_eq!(
            one(&[load(0x40_0000, 0, 0x2000, u64::MAX - 0x1000, RX)]),
            Err(ElfError::AboveUserSpace { index: 0, vaddr: 0x40_0000, memsz: u64::MAX - 0x1000 })
        );
        assert!(matches!(
            one(&[load(USER_END - 0x1000, 0, 0x2000, 0x2000, RX)]),
            Err(ElfError::AboveUserSpace { .. })
        ));
        assert_eq!(
            one(&[
                load(0x40_0000, 0, 0x1000, 0x1000, RX),
                load(0x30_0000, 0x1000, 0x1000, 0x1000, PF_R)
            ]),
            Err(ElfError::NotSorted { index: 1 })
        );
        assert_eq!(
            one(&[
                load(0x40_0000, 0, 0x1800, 0x1800, RX),
                load(0x40_1000, 0x1000, 0x1000, 0x1000, PF_R)
            ]),
            Err(ElfError::Overlap { index: 1, prev_end: 0x40_1800, vaddr: 0x40_1000 })
        );
        // Sharing a page at the boundary is allowed (older linkers).
        let shared = one(&[
            load(0x40_0000, 0, 0x1800, 0x1800, RX),
            load(0x40_1800, 0x1800, 0x100, 0x900, PF_R | PF_W),
        ])
        .unwrap();
        assert_eq!(shared.segments[0].page_end(), shared.segments[1].page_start() + PAGE_SIZE);
    }

    #[test]
    fn rejects_bad_tls_and_two_of_a_kind() {
        let base = || load(0x40_0000, 0, 0x2000, 0x2000, PF_R | PF_X);
        let tls = |offset, filesz, memsz, align| Ph {
            kind: PT_TLS,
            flags: PF_R,
            offset,
            vaddr: 0x40_0000,
            filesz,
            memsz,
            align,
        };
        assert!(one(&[base(), tls(0, 8, 16, 8)]).is_ok());
        assert_eq!(
            one(&[base(), tls(0, 16, 8, 8)]),
            Err(ElfError::BadTls { reason: "p_filesz above p_memsz" })
        );
        assert_eq!(
            one(&[base(), tls(0x2ff0, 0x20, 0x20, 8)]),
            Err(ElfError::BadTls { reason: "template outside the file" })
        );
        assert_eq!(
            one(&[base(), tls(0, 8, 16, 12)]),
            Err(ElfError::BadTls { reason: "bad alignment" })
        );
        assert_eq!(
            one(&[base(), tls(0, 8, u64::MAX, 8)]),
            Err(ElfError::BadTls { reason: "too large" })
        );
        assert_eq!(one(&[base(), tls(0, 8, 16, 8), tls(0, 8, 16, 8)]), Err(ElfError::TwoTls));
        let interp = || Ph {
            kind: PT_INTERP,
            flags: PF_R,
            offset: 0x100,
            vaddr: 0,
            filesz: 1,
            memsz: 1,
            align: 1,
        };
        assert_eq!(
            one(&[interp(), base()]),
            Err(ElfError::BadInterp { reason: "empty or longer than PATH_MAX" })
        );
        let stack = Ph {
            kind: PT_GNU_STACK,
            flags: PF_R | PF_W | PF_X,
            offset: 0,
            vaddr: 0,
            filesz: 0,
            memsz: 0,
            align: 16,
        };
        assert_eq!(
            one(&[base(), stack]).unwrap().gnu_stack,
            Some(Prot { read: true, write: true, exec: true })
        );
        assert_eq!(one(&[base()]).unwrap().gnu_stack, None);
    }

    #[test]
    fn bss_pieces() {
        // File bytes 0x40_1100..0x40_1300, memory to 0x40_4010.
        let s = Segment {
            vaddr: 0x40_1100,
            memsz: 0x2f10,
            offset: 0x1100,
            filesz: 0x200,
            prot: Prot::default(),
            align: 0x1000,
        };
        assert_eq!(s.file_pages(), Some((0x40_1000, 0x1000, 0x1000)));
        assert_eq!(s.bss(), 0x40_1300..0x40_4010);
        assert_eq!(s.zero_tail(), 0x40_1300..0x40_2000);
        assert_eq!(s.anon_pages(), 0x40_2000..0x40_5000);
        // All bss.
        let b = Segment { filesz: 0, ..s };
        assert_eq!(b.file_pages(), None);
        assert_eq!(b.zero_tail(), 0x40_1100..0x40_1100);
        assert_eq!(b.anon_pages(), 0x40_1000..0x40_5000);
        // No bss; the bss ends inside the last file page.
        let n = Segment { memsz: 0x200, ..s };
        assert_eq!(n.zero_tail(), 0x40_1300..0x40_1300);
        assert_eq!(n.anon_pages(), 0x40_2000..0x40_2000);
        let t = Segment { memsz: 0x300, ..s };
        assert_eq!(t.zero_tail(), 0x40_1300..0x40_1400);
        assert_eq!(t.anon_pages(), 0x40_2000..0x40_2000);
    }

    /// Mutations of a real binary: an error or a valid image, never a
    /// panic (a cheap fuzz over every header byte).
    #[test]
    fn mutated_headers_never_panic() {
        let image = ElfImage::parse(HELLO).unwrap();
        let end = 64 + usize::from(image.phnum) * 56;
        let mut f = HELLO.to_vec();
        for at in 0..end {
            for v in [0x00, 0xff, 0x7f, 0x80, 0x01] {
                let old = f[at];
                f[at] = v;
                if let Ok(i) = ElfImage::parse(&f) {
                    let _ = i.plan(i.load_bias(DEFAULT_DYN_BASE).unwrap_or(0));
                }
                f[at] = old;
            }
        }
        for len in (0..HELLO.len()).step_by(97) {
            let _ = ElfImage::parse(&HELLO[..len]);
        }
    }
}
