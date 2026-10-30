//! PE32+ image parsing, written from Microsoft's public "PE Format"
//! specification. Only parses and validates; mapping is done by the loader.

use core::fmt;

const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
const PE32_PLUS_MAGIC: u16 = 0x20b;
const IMAGE_FILE_EXECUTABLE_IMAGE: u16 = 0x0002;
const IMAGE_FILE_DLL: u16 = 0x2000;

pub const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
pub const IMAGE_SCN_MEM_READ: u32 = 0x4000_0000;
pub const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;

pub const DIRECTORY_IMPORT: usize = 1;
pub const DIRECTORY_BASERELOC: usize = 5;

const IMAGE_REL_BASED_ABSOLUTE: u16 = 0;
const IMAGE_REL_BASED_DIR64: u16 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeError {
    Truncated,
    BadDosSignature,
    BadPeSignature,
    UnsupportedMachine(u16),
    NotPe32Plus,
    NotExecutable,
    BadAlignment,
    SectionOutOfBounds,
    BadRelocation,
}

impl fmt::Display for PeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

fn u16_at(d: &[u8], off: usize) -> Result<u16, PeError> {
    let b = d.get(off..off + 2).ok_or(PeError::Truncated)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(d: &[u8], off: usize) -> Result<u32, PeError> {
    let b = d.get(off..off + 4).ok_or(PeError::Truncated)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(d: &[u8], off: usize) -> Result<u64, PeError> {
    Ok(u64::from(u32_at(d, off)?) | u64::from(u32_at(d, off + 4)?) << 32)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: [u8; 8],
    pub virtual_address: u32,
    pub virtual_size: u32,
    pub raw_offset: u32,
    pub raw_size: u32,
    pub characteristics: u32,
}

impl Section {
    pub fn name(&self) -> &str {
        let len = self.name.iter().position(|&b| b == 0).unwrap_or(8);
        core::str::from_utf8(&self.name[..len]).unwrap_or("?")
    }

    /// Bytes of the section in memory (virtual size, or raw size if 0).
    pub fn memory_size(&self) -> u32 {
        if self.virtual_size == 0 { self.raw_size } else { self.virtual_size }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DataDirectory {
    pub rva: u32,
    pub size: u32,
}

/// A validated PE32+ executable image.
#[derive(Debug)]
pub struct PeImage<'a> {
    data: &'a [u8],
    pub image_base: u64,
    pub size_of_image: u32,
    pub size_of_headers: u32,
    pub entry_rva: u32,
    pub section_alignment: u32,
    pub subsystem: u16,
    pub stack_reserve: u64,
    pub stack_commit: u64,
    directories: [DataDirectory; 16],
    section_table: usize,
    section_count: usize,
}

impl<'a> PeImage<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, PeError> {
        if data.get(0..2) != Some(b"MZ") {
            return Err(PeError::BadDosSignature);
        }
        let pe = u32_at(data, 0x3c)? as usize;
        if data.get(pe..pe + 4) != Some(b"PE\0\0") {
            return Err(PeError::BadPeSignature);
        }
        let coff = pe + 4;
        let machine = u16_at(data, coff)?;
        if machine != IMAGE_FILE_MACHINE_AMD64 {
            return Err(PeError::UnsupportedMachine(machine));
        }
        let section_count = usize::from(u16_at(data, coff + 2)?);
        let optional_size = usize::from(u16_at(data, coff + 16)?);
        let characteristics = u16_at(data, coff + 18)?;
        if characteristics & IMAGE_FILE_EXECUTABLE_IMAGE == 0
            || characteristics & IMAGE_FILE_DLL != 0
        {
            return Err(PeError::NotExecutable);
        }

        let opt = coff + 20;
        if u16_at(data, opt)? != PE32_PLUS_MAGIC {
            return Err(PeError::NotPe32Plus);
        }
        let entry_rva = u32_at(data, opt + 16)?;
        let image_base = u64_at(data, opt + 24)?;
        let section_alignment = u32_at(data, opt + 32)?;
        let file_alignment = u32_at(data, opt + 36)?;
        let size_of_image = u32_at(data, opt + 56)?;
        let size_of_headers = u32_at(data, opt + 60)?;
        let subsystem = u16_at(data, opt + 68)?;
        let stack_reserve = u64_at(data, opt + 72)?;
        let stack_commit = u64_at(data, opt + 80)?;
        let dir_count = (u32_at(data, opt + 108)? as usize).min(16);
        let mut directories = [DataDirectory::default(); 16];
        for (i, dir) in directories.iter_mut().enumerate().take(dir_count) {
            let off = opt + 112 + 8 * i;
            *dir = DataDirectory { rva: u32_at(data, off)?, size: u32_at(data, off + 4)? };
        }

        // We map pages directly, so sections must be page-aligned.
        if !section_alignment.is_power_of_two()
            || section_alignment < 4096
            || !file_alignment.is_power_of_two()
            || !image_base.is_multiple_of(u64::from(section_alignment))
        {
            return Err(PeError::BadAlignment);
        }
        if size_of_headers as usize > data.len() || size_of_headers > size_of_image {
            return Err(PeError::Truncated);
        }

        let image = PeImage {
            data,
            image_base,
            size_of_image,
            size_of_headers,
            entry_rva,
            section_alignment,
            subsystem,
            stack_reserve,
            stack_commit,
            directories,
            section_table: opt + optional_size,
            section_count,
        };
        let mut prev_end = size_of_headers;
        for i in 0..section_count {
            let s = image.section(i)?;
            let end = s
                .virtual_address
                .checked_add(s.memory_size())
                .ok_or(PeError::SectionOutOfBounds)?;
            let raw_end =
                s.raw_offset.checked_add(s.raw_size).ok_or(PeError::SectionOutOfBounds)?;
            if !s.virtual_address.is_multiple_of(section_alignment)
                || s.virtual_address < prev_end
                || end > size_of_image
                || raw_end as usize > data.len()
            {
                return Err(PeError::SectionOutOfBounds);
            }
            prev_end = end;
        }
        if entry_rva >= size_of_image {
            return Err(PeError::SectionOutOfBounds);
        }
        Ok(image)
    }

    fn section(&self, i: usize) -> Result<Section, PeError> {
        let off = self.section_table + 40 * i;
        let raw = self.data.get(off..off + 40).ok_or(PeError::Truncated)?;
        let mut name = [0; 8];
        name.copy_from_slice(&raw[..8]);
        Ok(Section {
            name,
            virtual_size: u32_at(raw, 8)?,
            virtual_address: u32_at(raw, 12)?,
            raw_size: u32_at(raw, 16)?,
            raw_offset: u32_at(raw, 20)?,
            characteristics: u32_at(raw, 36)?,
        })
    }

    /// Sections in table order (validated by `parse`).
    pub fn sections(&self) -> impl Iterator<Item = Section> + '_ {
        (0..self.section_count).filter_map(|i| self.section(i).ok())
    }

    pub fn directory(&self, index: usize) -> DataDirectory {
        self.directories.get(index).copied().unwrap_or_default()
    }

    pub fn headers(&self) -> &'a [u8] {
        &self.data[..self.size_of_headers as usize]
    }

    /// File bytes of `section` (at most `raw_size`, clipped to its memory size).
    pub fn section_data(&self, section: &Section) -> &'a [u8] {
        let len = section.raw_size.min(section.memory_size()) as usize;
        let start = section.raw_offset as usize;
        &self.data[start..start + len]
    }

    pub fn has_imports(&self) -> bool {
        self.directory(DIRECTORY_IMPORT).size != 0
    }
}

/// Applies base relocations to a loaded image (`image` is the whole image in
/// memory, `SizeOfImage` bytes) that was placed `delta` bytes away from its
/// preferred base.
pub fn apply_relocations(
    image: &mut [u8],
    reloc: DataDirectory,
    delta: u64,
) -> Result<(), PeError> {
    if delta == 0 || reloc.size == 0 {
        return Ok(());
    }
    let start = reloc.rva as usize;
    let end = start.checked_add(reloc.size as usize).ok_or(PeError::BadRelocation)?;
    let mut block = start;
    while block + 8 <= end {
        let page = u32_at(image, block).map_err(|_| PeError::BadRelocation)? as usize;
        let size = u32_at(image, block + 4).map_err(|_| PeError::BadRelocation)? as usize;
        if size < 8 || block + size > end {
            return Err(PeError::BadRelocation);
        }
        for entry in (block + 8..block + size).step_by(2) {
            let e = u16_at(image, entry).map_err(|_| PeError::BadRelocation)?;
            let (kind, offset) = (e >> 12, usize::from(e & 0xfff));
            match kind {
                IMAGE_REL_BASED_ABSOLUTE => {}
                IMAGE_REL_BASED_DIR64 => {
                    let at = page + offset;
                    let slot = image.get_mut(at..at + 8).ok_or(PeError::BadRelocation)?;
                    let value =
                        u64::from_le_bytes(slot.try_into().map_err(|_| PeError::BadRelocation)?);
                    slot.copy_from_slice(&value.wrapping_add(delta).to_le_bytes());
                }
                _ => return Err(PeError::BadRelocation),
            }
        }
        block += size;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal PE32+ image: headers in the first 0x200 bytes,
    /// sections described by (name, rva, vsize, raw bytes, characteristics).
    fn build(sections: &[(&str, u32, u32, &[u8], u32)], entry: u32) -> Vec<u8> {
        let mut d = vec![0u8; 0x200];
        d[0..2].copy_from_slice(b"MZ");
        d[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        d[0x40..0x44].copy_from_slice(b"PE\0\0");
        let coff = 0x44;
        d[coff..coff + 2].copy_from_slice(&0x8664u16.to_le_bytes());
        d[coff + 2..coff + 4].copy_from_slice(&(sections.len() as u16).to_le_bytes());
        d[coff + 16..coff + 18].copy_from_slice(&240u16.to_le_bytes());
        d[coff + 18..coff + 20].copy_from_slice(&0x22u16.to_le_bytes());
        let opt = coff + 20;
        let put32 =
            |d: &mut Vec<u8>, off: usize, v: u32| d[off..off + 4].copy_from_slice(&v.to_le_bytes());
        d[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        put32(&mut d, opt + 16, entry);
        d[opt + 24..opt + 32].copy_from_slice(&0x1_4000_0000u64.to_le_bytes());
        put32(&mut d, opt + 32, 0x1000);
        put32(&mut d, opt + 36, 0x200);
        let image_size =
            sections.iter().map(|s| (s.1 + s.2).next_multiple_of(0x1000)).max().unwrap_or(0x1000);
        put32(&mut d, opt + 56, image_size);
        put32(&mut d, opt + 60, 0x200);
        d[opt + 68..opt + 70].copy_from_slice(&3u16.to_le_bytes());
        put32(&mut d, opt + 108, 16);
        let table = opt + 240;
        for (i, (name, rva, vsize, raw, ch)) in sections.iter().enumerate() {
            let s = table + 40 * i;
            let raw_off = d.len() as u32;
            d[s..s + name.len()].copy_from_slice(name.as_bytes());
            put32(&mut d, s + 8, *vsize);
            put32(&mut d, s + 12, *rva);
            put32(&mut d, s + 16, raw.len() as u32);
            put32(&mut d, s + 20, raw_off);
            put32(&mut d, s + 36, *ch);
            d.extend_from_slice(raw);
        }
        d
    }

    const TEXT: u32 = IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_READ;
    const DATA: u32 = IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE;

    #[test]
    fn parses_minimal_image() {
        let d = build(
            &[
                (".text", 0x1000, 3, &[0x90, 0x90, 0xc3], TEXT),
                (".data", 0x2000, 0x10, &[1; 4], DATA),
            ],
            0x1000,
        );
        let pe = PeImage::parse(&d).unwrap();
        assert_eq!(pe.image_base, 0x1_4000_0000);
        assert_eq!(pe.entry_rva, 0x1000);
        assert_eq!(pe.size_of_image, 0x3000);
        assert_eq!(pe.subsystem, 3);
        let s: Vec<_> = pe.sections().collect();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].name(), ".text");
        assert_eq!(pe.section_data(&s[0]), &[0x90, 0x90, 0xc3]);
        assert_eq!(pe.section_data(&s[1]), &[1; 4]);
        assert!(!pe.has_imports());
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(PeImage::parse(b"").unwrap_err(), PeError::BadDosSignature);
        assert_eq!(PeImage::parse(b"MZ").unwrap_err(), PeError::Truncated);
        let mut d = build(&[(".text", 0x1000, 1, &[0xc3], TEXT)], 0x1000);
        d[0x44] = 0x4c; // i386
        assert!(matches!(PeImage::parse(&d), Err(PeError::UnsupportedMachine(_))));
    }

    #[test]
    fn rejects_bad_sections_and_entry() {
        // Overlapping the headers.
        let d = build(&[(".text", 0, 1, &[0xc3], TEXT)], 0);
        assert_eq!(PeImage::parse(&d).unwrap_err(), PeError::SectionOutOfBounds);
        // Unaligned section.
        let d = build(&[(".text", 0x1800, 1, &[0xc3], TEXT)], 0x1800);
        assert_eq!(PeImage::parse(&d).unwrap_err(), PeError::SectionOutOfBounds);
        // Entry outside the image.
        let d = build(&[(".text", 0x1000, 1, &[0xc3], TEXT)], 0x9000);
        assert_eq!(PeImage::parse(&d).unwrap_err(), PeError::SectionOutOfBounds);
        // Raw data beyond the file.
        let mut d = build(&[(".text", 0x1000, 1, &[0xc3], TEXT)], 0x1000);
        let len = d.len();
        d.truncate(len - 1);
        assert_eq!(PeImage::parse(&d).unwrap_err(), PeError::SectionOutOfBounds);
    }

    #[test]
    fn dll_is_not_an_executable() {
        let mut d = build(&[(".text", 0x1000, 1, &[0xc3], TEXT)], 0x1000);
        d[0x44 + 18] = 0x22;
        d[0x44 + 19] = 0x20; // IMAGE_FILE_DLL
        assert_eq!(PeImage::parse(&d).unwrap_err(), PeError::NotExecutable);
    }

    #[test]
    fn dir64_relocations() {
        let mut image = vec![0u8; 0x2000];
        image[0x1010..0x1018].copy_from_slice(&0x1_4000_1234u64.to_le_bytes());
        // One block for page 0x1000: DIR64 at +0x10, then ABSOLUTE padding.
        let block: [u8; 12] = {
            let mut b = [0u8; 12];
            b[0..4].copy_from_slice(&0x1000u32.to_le_bytes());
            b[4..8].copy_from_slice(&12u32.to_le_bytes());
            b[8..10].copy_from_slice(&(0xa000u16 | 0x010).to_le_bytes());
            b
        };
        image[0x1800..0x180c].copy_from_slice(&block);
        let dir = DataDirectory { rva: 0x1800, size: 12 };
        apply_relocations(&mut image, dir, 0x1000_0000).unwrap();
        assert_eq!(u64_at(&image, 0x1010).unwrap(), 0x1_5000_1234);
        // Unknown relocation type.
        image[0x1808..0x180a].copy_from_slice(&0x3010u16.to_le_bytes());
        assert_eq!(apply_relocations(&mut image, dir, 1), Err(PeError::BadRelocation));
    }

    #[test]
    fn hello_exe_shape() {
        // Shape of user/hello as linked by lld-link (checked by the boot test
        // with the real binary): sections .text/.rdata/.pdata, no imports.
        let d = build(
            &[
                (".text", 0x1000, 0xc3, &[0xc3; 0x200], TEXT),
                (".rdata", 0x2000, 0x68, &[0; 0x200], IMAGE_SCN_MEM_READ),
            ],
            0x10a0,
        );
        let pe = PeImage::parse(&d).unwrap();
        assert_eq!(pe.section_data(&pe.sections().next().unwrap()).len(), 0xc3);
    }
}
