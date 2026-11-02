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
