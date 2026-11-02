//! Just enough ELF (the System V ABI's ELF-64 format) to load a kernel
//! image for a handover: a little-endian x86-64 executable whose loadable
//! segments sit in one span of the kernel's higher-half window.

use crate::mm::PAGE_SIZE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElfError {
    NotElf64,
    NotX86_64Executable,
    Truncated,
    NoSegments,
    /// A segment outside `[base, base + max_span)` or a file range outside
    /// the file.
    BadSegment,
}

/// A loadable segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub vaddr: u64,
    pub mem_len: u64,
    pub file_offset: u64,
    pub file_len: u64,
}

/// A checked kernel image.
#[derive(Clone, Copy, Debug)]
pub struct KernelImage<'a> {
    file: &'a [u8],
    pub entry: u64,
    /// The page-aligned virtual span every segment lies in.
    pub base: u64,
    pub span: u64,
    phoff: usize,
    phentsize: usize,
    phnum: usize,
}

const PT_LOAD: u32 = 1;
const PT_NOTE: u32 = 4;

/// Walks a note segment's entries (namesz, descsz, type, name, desc, each
/// padded to `align`) for the one named `name` of type `note_type`.
fn find_note<'a>(
    mut notes: &'a [u8],
    align: usize,
    name: &[u8],
    note_type: u32,
) -> Option<&'a [u8]> {
    while notes.len() >= 12 {
        let namesz = u32_at(notes, 0)? as usize;
        let descsz = u32_at(notes, 4)? as usize;
        let ty = u32_at(notes, 8)?;
        let name_at = 12;
        let desc_at = name_at + namesz.next_multiple_of(align);
        let end = desc_at.checked_add(descsz.next_multiple_of(align))?;
        let this_name = notes.get(name_at..name_at + namesz)?;
        let desc = notes.get(desc_at..desc_at + descsz)?;
        if this_name == name && ty == note_type {
            return Some(desc);
        }
        notes = notes.get(end..)?;
    }
    None
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// Checks `file` as a kernel linked at `base` taking at most `max_span`
/// bytes of virtual space.
pub fn parse(file: &[u8], base: u64, max_span: u64) -> Result<KernelImage<'_>, ElfError> {
    if file.get(0..4) != Some(b"\x7fELF") || file.get(4) != Some(&2) || file.get(5) != Some(&1) {
        return Err(ElfError::NotElf64);
    }
    // ET_EXEC, EM_X86_64.
    if u16_at(file, 16) != Some(2) || u16_at(file, 18) != Some(62) {
        return Err(ElfError::NotX86_64Executable);
    }
    let entry = u64_at(file, 24).ok_or(ElfError::Truncated)?;
    let phoff = u64_at(file, 32).ok_or(ElfError::Truncated)? as usize;
    let phentsize = usize::from(u16_at(file, 54).ok_or(ElfError::Truncated)?);
    let phnum = usize::from(u16_at(file, 56).ok_or(ElfError::Truncated)?);
    if phentsize < 56 || phoff.checked_add(phentsize * phnum).is_none_or(|e| e > file.len()) {
        return Err(ElfError::Truncated);
    }
    let mut image = KernelImage { file, entry, base, span: 0, phoff, phentsize, phnum };
    let mut end = base;
    let mut any = false;
    for seg in image.segments() {
        let seg_end = seg.vaddr.checked_add(seg.mem_len).ok_or(ElfError::BadSegment)?;
        let file_end = seg.file_offset.checked_add(seg.file_len).ok_or(ElfError::BadSegment)?;
        if seg.vaddr < base
            || seg_end - base > max_span
            || seg.file_len > seg.mem_len
            || file_end > file.len() as u64
        {
            return Err(ElfError::BadSegment);
        }
        end = end.max(seg_end);
        any = true;
    }
    if !any {
        return Err(ElfError::NoSegments);
    }
    image.span = (end - base).next_multiple_of(PAGE_SIZE);
    if entry < base || entry >= base + image.span {
        return Err(ElfError::BadSegment);
    }
    Ok(image)
}

impl<'a> KernelImage<'a> {
    /// The PT_LOAD segments.
    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        (0..self.phnum).filter_map(move |i| {
            let at = self.phoff + i * self.phentsize;
            if u32_at(self.file, at)? != PT_LOAD {
                return None;
            }
            Some(Segment {
                file_offset: u64_at(self.file, at + 8)?,
                vaddr: u64_at(self.file, at + 16)?,
                file_len: u64_at(self.file, at + 32)?,
                mem_len: u64_at(self.file, at + 40)?,
            })
        })
    }

    /// The description of the first note named `name` (without padding,
    /// e.g. `b"GALA\0"`) of type `note_type` in a PT_NOTE segment, if any
    /// (`handover::version`: what the kernel reads of a handover record).
    pub fn note(&self, name: &[u8], note_type: u32) -> Option<&'a [u8]> {
        (0..self.phnum).find_map(|i| {
            let at = self.phoff + i * self.phentsize;
            if u32_at(self.file, at)? != PT_NOTE {
                return None;
            }
            let offset = usize::try_from(u64_at(self.file, at + 8)?).ok()?;
            let len = usize::try_from(u64_at(self.file, at + 32)?).ok()?;
            let align = if u64_at(self.file, at + 48)? == 8 { 8 } else { 4 };
            let notes = self.file.get(offset..offset.checked_add(len)?)?;
            find_note(notes, align, name, note_type)
        })
    }

    /// The bytes of the file `seg` loads (checked by `parse`).
    pub fn file_bytes(&self, seg: &Segment) -> &'a [u8] {
        let start = seg.file_offset as usize;
        self.file.get(start..start + seg.file_len as usize).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: u64 = 0xffff_ffff_8000_0000;

    /// A minimal ELF: header, two program headers, contents.
    fn elf(segments: &[(u64, u64, &[u8])], entry: u64) -> Vec<u8> {
        let mut f = vec![0u8; 64 + 56 * segments.len()];
        f[0..4].copy_from_slice(b"\x7fELF");
        f[4] = 2;
        f[5] = 1;
        f[6] = 1;
        f[16..18].copy_from_slice(&2u16.to_le_bytes());
        f[18..20].copy_from_slice(&62u16.to_le_bytes());
        f[24..32].copy_from_slice(&entry.to_le_bytes());
        f[32..40].copy_from_slice(&64u64.to_le_bytes());
        f[54..56].copy_from_slice(&56u16.to_le_bytes());
        f[56..58].copy_from_slice(&(segments.len() as u16).to_le_bytes());
        for (i, (vaddr, mem, data)) in segments.iter().enumerate() {
            let off = f.len() as u64;
            f.extend_from_slice(data);
            let ph = 64 + 56 * i;
            f[ph..ph + 4].copy_from_slice(&PT_LOAD.to_le_bytes());
            f[ph + 8..ph + 16].copy_from_slice(&off.to_le_bytes());
            f[ph + 16..ph + 24].copy_from_slice(&vaddr.to_le_bytes());
            f[ph + 32..ph + 40].copy_from_slice(&(data.len() as u64).to_le_bytes());
            f[ph + 40..ph + 48].copy_from_slice(&mem.to_le_bytes());
        }
        f
    }

    #[test]
    fn parses_a_kernel() {
        let f = elf(&[(BASE, 0x10, b"rodata"), (BASE + 0x1000, 0x3000, b"code")], BASE + 0x1000);
        let img = parse(&f, BASE, 1 << 30).unwrap();
        assert_eq!(img.entry, BASE + 0x1000);
        assert_eq!(img.span, 0x4000);
        let segs: Vec<_> = img.segments().collect();
        assert_eq!(segs.len(), 2);
        assert_eq!(img.file_bytes(&segs[1]), b"code");
        assert_eq!(segs[1].mem_len, 0x3000);
    }

    #[test]
    fn finds_notes() {
        // Two notes: another vendor's, then GALA's (namesz 5, padded to 8).
        let mut notes = Vec::new();
        for (name, ty, desc) in [(&b"GNU\0"[..], 3u32, &b"abcd"[..]), (b"GALA\0", 7, b"12345678")] {
            notes.extend_from_slice(&(name.len() as u32).to_le_bytes());
            notes.extend_from_slice(&(desc.len() as u32).to_le_bytes());
            notes.extend_from_slice(&ty.to_le_bytes());
            notes.extend_from_slice(name);
            notes.resize(notes.len().next_multiple_of(4), 0);
            notes.extend_from_slice(desc);
        }
        let mut f = elf(&[(BASE, 0x10, b"x")], BASE);
        // A third program header: PT_NOTE over the notes.
        let off = f.len();
        f.extend_from_slice(&notes);
        let mut ph = [0u8; 56];
        ph[0..4].copy_from_slice(&PT_NOTE.to_le_bytes());
        ph[8..16].copy_from_slice(&(off as u64 + 56).to_le_bytes());
        ph[32..40].copy_from_slice(&(notes.len() as u64).to_le_bytes());
        ph[48..56].copy_from_slice(&4u64.to_le_bytes());
        // Insert it after the first header (contents move by 56).
        f.splice(120..120, ph);
        f[56..58].copy_from_slice(&2u16.to_le_bytes());
        let seg_off = u64::from_le_bytes(f[72..80].try_into().unwrap()) + 56;
        f[72..80].copy_from_slice(&seg_off.to_le_bytes());
        let img = parse(&f, BASE, 1 << 30).unwrap();
        assert_eq!(img.note(b"GALA\0", 7), Some(&b"12345678"[..]));
        assert_eq!(img.note(b"GNU\0", 3), Some(&b"abcd"[..]));
        assert_eq!(img.note(b"GALA\0", 8), None);
        // No PT_NOTE: none.
        let plain = elf(&[(BASE, 0x10, b"x")], BASE);
        assert_eq!(parse(&plain, BASE, 1 << 30).unwrap().note(b"GALA\0", 7), None);
    }

    #[test]
    fn refuses_bad_images() {
        assert_eq!(parse(b"MZ", BASE, 1 << 30).err(), Some(ElfError::NotElf64));
        // Below the base, or too large.
        let f = elf(&[(BASE - 0x1000, 0x10, b"x")], BASE - 0x1000);
        assert_eq!(parse(&f, BASE, 1 << 30).err(), Some(ElfError::BadSegment));
        let f = elf(&[(BASE, 0x200_0000, b"x")], BASE);
        assert_eq!(parse(&f, BASE, 0x100_0000).err(), Some(ElfError::BadSegment));
        // Entry outside the image.
        let f = elf(&[(BASE, 0x10, b"x")], BASE + 0x5000);
        assert_eq!(parse(&f, BASE, 1 << 30).err(), Some(ElfError::BadSegment));
        // Truncated program headers.
        let f = elf(&[(BASE, 0x10, b"x")], BASE);
        assert_eq!(parse(&f[..80], BASE, 1 << 30).err(), Some(ElfError::Truncated));
        let f = elf(&[], BASE);
        assert_eq!(parse(&f, BASE, 1 << 30).err(), Some(ElfError::NoSegments));
    }
}
