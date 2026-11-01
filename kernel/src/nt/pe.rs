//! PE32+ image parsing, written from Microsoft's public "PE Format"
//! specification. Only parses and validates; mapping is done by the loader.
//! Image sections also take 32-bit (PE32, i386) images, as 64-bit Windows
//! maps them (WOW64, IL-only .NET assemblies); processes run only PE32+.

use core::fmt;

const IMAGE_FILE_MACHINE_AMD64: u16 = 0x8664;
const IMAGE_FILE_MACHINE_I386: u16 = 0x14c;
const PE32_MAGIC: u16 = 0x10b;
const PE32_PLUS_MAGIC: u16 = 0x20b;
const IMAGE_FILE_EXECUTABLE_IMAGE: u16 = 0x0002;
const IMAGE_FILE_DLL: u16 = 0x2000;
const IMAGE_FILE_RELOCS_STRIPPED: u16 = 0x0001;

pub const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
pub const IMAGE_SCN_MEM_READ: u32 = 0x4000_0000;
pub const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;

pub const DIRECTORY_IMPORT: usize = 1;
pub const DIRECTORY_BASERELOC: usize = 5;

const IMAGE_REL_BASED_ABSOLUTE: u16 = 0;
const IMAGE_REL_BASED_HIGHLOW: u16 = 3;
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
    /// (MajorSubsystemVersion, MinorSubsystemVersion)
    pub subsystem_version: (u16, u16),
    pub stack_reserve: u64,
    pub stack_commit: u64,
    directories: [DataDirectory; 16],
    section_table: usize,
    section_count: usize,
    pub is_dll: bool,
    /// Whether the image can be loaded away from `image_base`.
    pub relocatable: bool,
    /// A 32-bit (PE32) image: it can be mapped, not run.
    pub pe32: bool,
}

impl<'a> PeImage<'a> {
    /// Parses an executable (.exe) image.
    pub fn parse(data: &'a [u8]) -> Result<Self, PeError> {
        let image = Self::parse_image(data)?;
        if image.pe32 {
            return Err(PeError::NotPe32Plus);
        }
        if image.is_dll {
            return Err(PeError::NotExecutable);
        }
        Ok(image)
    }

    /// Parses any image (executable or DLL), as for SEC_IMAGE sections.
    pub fn parse_image(data: &'a [u8]) -> Result<Self, PeError> {
        if data.get(0..2) != Some(b"MZ") {
            return Err(PeError::BadDosSignature);
        }
        let pe = u32_at(data, 0x3c)? as usize;
        if data.get(pe..pe + 4) != Some(b"PE\0\0") {
            return Err(PeError::BadPeSignature);
        }
        let coff = pe + 4;
        let machine = u16_at(data, coff)?;
        if machine != IMAGE_FILE_MACHINE_AMD64 && machine != IMAGE_FILE_MACHINE_I386 {
            return Err(PeError::UnsupportedMachine(machine));
        }
        let section_count = usize::from(u16_at(data, coff + 2)?);
        let optional_size = usize::from(u16_at(data, coff + 16)?);
        let characteristics = u16_at(data, coff + 18)?;
        if characteristics & IMAGE_FILE_EXECUTABLE_IMAGE == 0 {
            return Err(PeError::NotExecutable);
        }
        let is_dll = characteristics & IMAGE_FILE_DLL != 0;
        let relocs_stripped = characteristics & IMAGE_FILE_RELOCS_STRIPPED != 0;

        let opt = coff + 20;
        // PE32 has a 4-byte ImageBase and stack/heap sizes and BaseOfData,
        // so its data directories start at +96 rather than +112.
        let pe32 = match (u16_at(data, opt)?, machine) {
            (PE32_PLUS_MAGIC, IMAGE_FILE_MACHINE_AMD64) => false,
            (PE32_MAGIC, IMAGE_FILE_MACHINE_I386) => true,
            _ => return Err(PeError::NotPe32Plus),
        };
        let entry_rva = u32_at(data, opt + 16)?;
        let image_base =
            if pe32 { u64::from(u32_at(data, opt + 28)?) } else { u64_at(data, opt + 24)? };
        let section_alignment = u32_at(data, opt + 32)?;
        let file_alignment = u32_at(data, opt + 36)?;
        let size_of_image = u32_at(data, opt + 56)?;
        let size_of_headers = u32_at(data, opt + 60)?;
        let subsystem = u16_at(data, opt + 68)?;
        let subsystem_version = (u16_at(data, opt + 48)?, u16_at(data, opt + 50)?);
        let (stack_reserve, stack_commit, dirs_at) = if pe32 {
            (u64::from(u32_at(data, opt + 72)?), u64::from(u32_at(data, opt + 76)?), opt + 96)
        } else {
            (u64_at(data, opt + 72)?, u64_at(data, opt + 80)?, opt + 112)
        };
        let dir_count = (u32_at(data, dirs_at - 4)? as usize).min(16);
        let mut directories = [DataDirectory::default(); 16];
        for (i, dir) in directories.iter_mut().enumerate().take(dir_count) {
            let off = dirs_at + 8 * i;
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

        let mut image = PeImage {
            data,
            image_base,
            size_of_image,
            size_of_headers,
            entry_rva,
            section_alignment,
            subsystem,
            subsystem_version,
            stack_reserve,
            stack_commit,
            directories,
            section_table: opt + optional_size,
            section_count,
            is_dll,
            relocatable: false,
            pe32,
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
        // Only IMAGE_FILE_RELOCS_STRIPPED pins an image to its base: one
        // without a relocation directory simply has nothing to fix up (PE
        // format docs; lld omits an empty .reloc).
        image.relocatable = !relocs_stripped;
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

impl PeImage<'_> {
    /// Copies headers and sections to their RVAs in `out` (`SizeOfImage`
    /// bytes, the rest is zeroed).
    pub fn lay_out(&self, out: &mut [u8]) {
        out.fill(0);
        let headers = self.headers();
        out[..headers.len()].copy_from_slice(headers);
        for s in self.sections() {
            let data = self.section_data(&s);
            let start = s.virtual_address as usize;
            out[start..start + data.len()].copy_from_slice(data);
        }
    }
}

/// The RVA of export `name` in a laid-out image (`image` = SizeOfImage
/// bytes as mapped). Forwarders are returned as their RVA too.
pub fn find_export(image: &[u8], name: &[u8]) -> Option<u32> {
    let u32_at = |off: usize| -> Option<u32> {
        Some(u32::from_le_bytes(image.get(off..off.checked_add(4)?)?.try_into().ok()?))
    };
    let pe = u32_at(0x3c)? as usize;
    let dir = u32_at(pe + 24 + 112)? as usize; // DataDirectory[0]
    if dir == 0 {
        return None;
    }
    let count = u32_at(dir + 24)? as usize;
    let functions = u32_at(dir + 28)? as usize;
    let names = u32_at(dir + 32)? as usize;
    let ordinals = u32_at(dir + 36)? as usize;
    for i in 0..count {
        let s = u32_at(names + 4 * i)? as usize;
        let text = image.get(s..)?;
        let end = text.iter().position(|&b| b == 0)?;
        if &text[..end] == name {
            let o = ordinals + 2 * i;
            let ordinal = u16::from_le_bytes(image.get(o..o + 2)?.try_into().ok()?);
            return u32_at(functions + 4 * usize::from(ordinal));
        }
    }
    None
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
                // PE32 images: 32-bit addresses (the delta's low half).
                IMAGE_REL_BASED_HIGHLOW => {
                    let at = page + offset;
                    let slot = image.get_mut(at..at + 4).ok_or(PeError::BadRelocation)?;
                    let value =
                        u32::from_le_bytes(slot.try_into().map_err(|_| PeError::BadRelocation)?);
                    slot.copy_from_slice(&value.wrapping_add(delta as u32).to_le_bytes());
                }
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

/// Page `page_rva` (4 KiB) of a laid-out `image` relocated by `delta`,
/// into `out`: the page's bytes with every relocation that touches it
/// applied, computed from the unrelocated image (an entry that crosses
/// into the next page is written in both pages, each its own bytes).
/// The same result as `apply_relocations` on the whole image, a page at a
/// time, so a large image is never copied whole.
pub fn relocate_page(
    image: &[u8],
    reloc: DataDirectory,
    delta: u64,
    page_rva: usize,
    out: &mut [u8; 4096],
) -> Result<(), PeError> {
    let page_end = page_rva + out.len();
    let have = image.len().min(page_end).saturating_sub(page_rva);
    out[..have].copy_from_slice(&image[page_rva..page_rva + have]);
    out[have..].fill(0);
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
        // Only this page's block and the one before (entries crossing in).
        if page == page_rva || page + 4096 == page_rva {
            for entry in (block + 8..block + size).step_by(2) {
                let e = u16_at(image, entry).map_err(|_| PeError::BadRelocation)?;
                let (kind, offset) = (e >> 12, usize::from(e & 0xfff));
                let at = page + offset;
                let bytes: [u8; 8] = match kind {
                    IMAGE_REL_BASED_ABSOLUTE => continue,
                    IMAGE_REL_BASED_HIGHLOW => {
                        let v = u32_at(image, at).map_err(|_| PeError::BadRelocation)?;
                        let mut b = [0u8; 8];
                        b[..4].copy_from_slice(&v.wrapping_add(delta as u32).to_le_bytes());
                        b
                    }
                    IMAGE_REL_BASED_DIR64 => {
                        let slot = image.get(at..at + 8).ok_or(PeError::BadRelocation)?;
                        let v = u64::from_le_bytes(
                            slot.try_into().map_err(|_| PeError::BadRelocation)?,
                        );
                        v.wrapping_add(delta).to_le_bytes()
                    }
                    _ => return Err(PeError::BadRelocation),
                };
                let width = if kind == IMAGE_REL_BASED_DIR64 { 8 } else { 4 };
                for (k, &b) in bytes[..width].iter().enumerate() {
                    let rva = at + k;
                    if (page_rva..page_end).contains(&rva) {
                        out[rva - page_rva] = b;
                    }
                }
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

    #[test]
    fn relocating_a_page_at_a_time() {
        // Three pages of data, a relocation table after them: DIR64 entries
        // in page 0 and 2, one crossing from page 0 into page 1 (offset
        // 0xffc), a HIGHLOW in page 1, padding (ABSOLUTE).
        let mut image = vec![0u8; 0x4000];
        for (i, b) in image.iter_mut().enumerate().take(0x3000) {
            *b = (i * 7 + i / 256) as u8;
        }
        let mut table = Vec::new();
        let mut block = |page: u32, entries: &[u16]| {
            let size = 8 + 2 * entries.len() as u32;
            table.extend_from_slice(&page.to_le_bytes());
            table.extend_from_slice(&size.to_le_bytes());
            for e in entries {
                table.extend_from_slice(&e.to_le_bytes());
            }
        };
        block(0x0000, &[0xa010, 0xaffc]);
        block(0x1000, &[0x3020, 0x0000]);
        block(0x2000, &[0xa7f8, 0xaff8]);
        image[0x3000..0x3000 + table.len()].copy_from_slice(&table);
        let reloc = DataDirectory { rva: 0x3000, size: table.len() as u32 };
        let delta = 0x1234_5678_9abc_u64;
        let mut whole = image.clone();
        apply_relocations(&mut whole, reloc, delta).unwrap();
        for page in 0..4 {
            let mut out = [0u8; 4096];
            relocate_page(&image, reloc, delta, page * 0x1000, &mut out).unwrap();
            assert_eq!(&out[..], &whole[page * 0x1000..(page + 1) * 0x1000], "page {page}");
        }
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
    fn dll_parses_as_image_and_lays_out() {
        let mut d = build(
            &[(".text", 0x1000, 3, &[0x90, 0x90, 0xc3], TEXT), (".data", 0x2000, 8, &[7; 4], DATA)],
            0x1000,
        );
        d[0x44 + 19] = 0x20; // IMAGE_FILE_DLL
        let pe = PeImage::parse_image(&d).unwrap();
        assert!(pe.is_dll);
        assert!(pe.relocatable, "no fixups, but not IMAGE_FILE_RELOCS_STRIPPED");
        let mut out = vec![0xffu8; pe.size_of_image as usize];
        pe.lay_out(&mut out);
        assert_eq!(&out[0..2], b"MZ");
        assert_eq!(&out[0x1000..0x1003], &[0x90, 0x90, 0xc3]);
        assert_eq!(out[0x1003], 0, "rest of the page is zero");
        assert_eq!(&out[0x2000..0x2008], &[7, 7, 7, 7, 0, 0, 0, 0]);
    }

    #[test]
    fn finds_exports_by_name() {
        // An export directory at RVA 0x1000 with two names (sorted) and
        // ordinals pointing at swapped functions.
        let mut e = vec![0u8; 0x100];
        let put =
            |e: &mut Vec<u8>, off: usize, v: u32| e[off..off + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut e, 20, 2); // NumberOfFunctions
        put(&mut e, 24, 2); // NumberOfNames
        put(&mut e, 28, 0x1040); // AddressOfFunctions
        put(&mut e, 32, 0x1050); // AddressOfNames
        put(&mut e, 36, 0x1060); // AddressOfNameOrdinals
        put(&mut e, 0x40, 0x2000);
        put(&mut e, 0x44, 0x2010);
        put(&mut e, 0x50, 0x1070);
        put(&mut e, 0x54, 0x1080);
        e[0x60..0x64].copy_from_slice(&[1, 0, 0, 0]); // ordinals 1, 0
        e[0x70..0x74].copy_from_slice(b"Alfa");
        e[0x80..0x84].copy_from_slice(b"Beta");
        let mut d = build(&[(".edata", 0x1000, 0x100, &e, IMAGE_SCN_MEM_READ)], 0x1000);
        let dir0 = 0x44 + 20 + 112;
        d[dir0..dir0 + 4].copy_from_slice(&0x1000u32.to_le_bytes());
        d[dir0 + 4..dir0 + 8].copy_from_slice(&0x100u32.to_le_bytes());
        let pe = PeImage::parse_image(&d).unwrap();
        let mut image = vec![0u8; pe.size_of_image as usize];
        pe.lay_out(&mut image);
        assert_eq!(find_export(&image, b"Alfa"), Some(0x2010));
        assert_eq!(find_export(&image, b"Beta"), Some(0x2000));
        assert_eq!(find_export(&image, b"Gamma"), None);
        assert_eq!(find_export(&image, b"Alf"), None);
    }

    #[test]
    fn dir64_and_highlow_relocations() {
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
        // HIGHLOW (PE32): the delta's low 32 bits.
        image[0x1020..0x1024].copy_from_slice(&0x0040_1000u32.to_le_bytes());
        image[0x1808..0x180a].copy_from_slice(&0x3020u16.to_le_bytes());
        apply_relocations(&mut image, dir, 0x1_0010_0000).unwrap();
        assert_eq!(u32_at(&image, 0x1020).unwrap(), 0x0050_1000);
        // Unknown relocation type.
        image[0x1808..0x180a].copy_from_slice(&0x5010u16.to_le_bytes());
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
