//! The machine's SMBIOS tables: what the firmware says about the
//! hardware it booted ("System Management BIOS (SMBIOS) Reference
//! Specification", DMTF DSP0134), which the bootloader points the kernel
//! at (Limine's SMBIOS request: the 32-bit `_SM_` and the 64-bit `_SM3_`
//! entry point).
//!
//! Windows hands the same bytes to programs through
//! GetSystemFirmwareTable's `RSMB` provider (ARCHITECTURE "Firmware
//! tables"), which Office's shared code asks for the machine it runs on.
//!
//! Parsing works on byte slices (host-tested). The kernel reads the entry
//! point through the HHDM while the bootloader's memory is still there
//! (`boot::collect`) and copies the table itself once the heap is up
//! (`platform::init_firmware`).

/// What an SMBIOS entry point says: the version the firmware reports, and
/// where the structure table is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryPoint {
    pub major: u8,
    pub minor: u8,
    /// The document revision the 64-bit entry point carries; the 32-bit
    /// one has none, and 0 is what Windows reports for it.
    pub docrev: u8,
    /// The structure table's physical address.
    pub table_phys: u64,
    /// The table's length: what the 32-bit entry point declares, or the
    /// maximum the 64-bit one gives, which [`table_end`] narrows to the
    /// end-of-table structure.
    pub table_len: u32,
    /// Whether this is the 64-bit entry point (`_SM3_`).
    pub bits_64: bool,
}

/// The 32-bit entry point's length, and the 64-bit one's: the shortest
/// either may be.
pub const ENTRY_32_LEN: usize = 0x1f;
pub const ENTRY_64_LEN: usize = 0x18;

/// The last structure of the table (SMBIOS type 127).
const END_OF_TABLE: u8 = 127;

/// Every byte of a checksummed area adds up to nought.
fn checksum_ok(b: &[u8]) -> bool {
    b.iter().fold(0u8, |a, &x| a.wrapping_add(x)) == 0
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

/// The 32-bit entry point (`_SM_`, SMBIOS 2.1 and later): the version at 6
/// and 7, then an intermediate structure from 0x10 (`_DMI_`) with its own
/// checksum, the table's length at 0x16 and its 32-bit address at 0x18.
/// Both checksums must hold, or these are not the firmware's bytes.
pub fn parse_32(b: &[u8]) -> Option<EntryPoint> {
    if !b.starts_with(b"_SM_") {
        return None;
    }
    let len = usize::from(*b.get(5)?);
    if len < ENTRY_32_LEN || !checksum_ok(b.get(..len)?) {
        return None;
    }
    if b.get(0x10..0x15)? != b"_DMI_" || !checksum_ok(b.get(0x10..ENTRY_32_LEN)?) {
        return None;
    }
    Some(EntryPoint {
        major: *b.get(6)?,
        minor: *b.get(7)?,
        docrev: 0,
        table_phys: u64::from(u32_at(b, 0x18)?),
        table_len: u32::from(u16_at(b, 0x16)?),
        bits_64: false,
    })
}

/// The 64-bit entry point (`_SM3_`, SMBIOS 3.0 and later): the version at
/// 7 to 9 (major, minor, document revision), the table's maximum size at
/// 0x0c and its 64-bit address at 0x10.
pub fn parse_64(b: &[u8]) -> Option<EntryPoint> {
    if !b.starts_with(b"_SM3_") {
        return None;
    }
    let len = usize::from(*b.get(6)?);
    if len < ENTRY_64_LEN || !checksum_ok(b.get(..len)?) {
        return None;
    }
    Some(EntryPoint {
        major: *b.get(7)?,
        minor: *b.get(8)?,
        docrev: *b.get(9)?,
        table_phys: u64_at(b, 0x10)?,
        table_len: u32_at(b, 0x0c)?,
        bits_64: true,
    })
}

/// Where the structure table really ends. The structures run one after
/// another: a four-byte header (type, length, handle), the formatted area
/// the length covers, then the strings, which end at a pair of NULs; the
/// end-of-table structure (type 127) is the last one. Returns the number
/// of bytes up to and including it, or None if the bytes do not read as
/// structures — the 64-bit entry point gives only a maximum size, so this
/// walk is what makes the table's length exact.
pub fn table_end(b: &[u8]) -> Option<usize> {
    let mut at = 0usize;
    loop {
        let kind = *b.get(at)?;
        let len = usize::from(*b.get(at + 1)?);
        if len < 4 {
            return None;
        }
        let mut strings = at.checked_add(len)?;
        while *b.get(strings)? != 0 || *b.get(strings + 1)? != 0 {
            strings += 1;
        }
        at = strings + 2;
        if kind == END_OF_TABLE {
            return Some(at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix_checksum(b: &mut [u8], at: usize, range: core::ops::Range<usize>) {
        b[at] = 0;
        let sum = b[range].iter().fold(0u8, |a, &x| a.wrapping_add(x));
        b[at] = 0u8.wrapping_sub(sum);
    }

    fn entry_32(table: u32, len: u16) -> Vec<u8> {
        let mut b = vec![0u8; ENTRY_32_LEN];
        b[..4].copy_from_slice(b"_SM_");
        b[5] = ENTRY_32_LEN as u8;
        b[6] = 2;
        b[7] = 8;
        b[0x10..0x15].copy_from_slice(b"_DMI_");
        b[0x16..0x18].copy_from_slice(&len.to_le_bytes());
        b[0x18..0x1c].copy_from_slice(&table.to_le_bytes());
        fix_checksum(&mut b, 0x15, 0x10..ENTRY_32_LEN);
        fix_checksum(&mut b, 4, 0..ENTRY_32_LEN);
        b
    }

    fn entry_64(table: u64, max: u32) -> Vec<u8> {
        let mut b = vec![0u8; ENTRY_64_LEN];
        b[..5].copy_from_slice(b"_SM3_");
        b[6] = ENTRY_64_LEN as u8;
        b[7] = 3;
        b[8] = 4;
        b[0x0a] = 1;
        b[0x0c..0x10].copy_from_slice(&max.to_le_bytes());
        b[0x10..0x18].copy_from_slice(&table.to_le_bytes());
        fix_checksum(&mut b, 5, 0..ENTRY_64_LEN);
        b
    }

    /// A structure: a type, a four-byte header, its formatted area and the
    /// strings after it (none is a pair of NULs of its own).
    fn structure(kind: u8, extra: usize, strings: &[&str]) -> Vec<u8> {
        let mut s = vec![0u8; 4 + extra];
        s[0] = kind;
        s[1] = (4 + extra) as u8;
        for text in strings {
            s.extend_from_slice(text.as_bytes());
            s.push(0);
        }
        if strings.is_empty() {
            s.push(0);
        }
        s.push(0);
        s
    }

    #[test]
    fn the_64_bit_entry_point_gives_the_version_and_the_table() {
        let e = parse_64(&entry_64(0x7fff_0000, 0x400)).expect("entry point");
        assert_eq!(
            e,
            EntryPoint {
                major: 3,
                minor: 4,
                docrev: 0,
                table_phys: 0x7fff_0000,
                table_len: 0x400,
                bits_64: true,
            }
        );
    }

    #[test]
    fn the_32_bit_entry_point_gives_the_length_it_declares() {
        let e = parse_32(&entry_32(0xf_0000, 0x1a0)).expect("entry point");
        assert_eq!(
            e,
            EntryPoint {
                major: 2,
                minor: 8,
                docrev: 0,
                table_phys: 0xf_0000,
                table_len: 0x1a0,
                bits_64: false,
            }
        );
    }

    #[test]
    fn an_entry_point_whose_checksum_is_wrong_is_not_the_firmwares() {
        let mut b = entry_64(0x7fff_0000, 0x400);
        b[5] = b[5].wrapping_add(1);
        assert_eq!(parse_64(&b), None);
        let mut b = entry_32(0xf_0000, 0x1a0);
        b[4] = b[4].wrapping_add(1);
        assert_eq!(parse_32(&b), None);
        // The intermediate structure has a checksum of its own.
        let mut b = entry_32(0xf_0000, 0x1a0);
        b[0x15] = b[0x15].wrapping_add(1);
        fix_checksum(&mut b, 4, 0..ENTRY_32_LEN);
        assert_eq!(parse_32(&b), None);
    }

    #[test]
    fn an_entry_point_of_another_kind_is_refused() {
        assert_eq!(parse_64(&entry_32(0xf_0000, 0x1a0)), None);
        assert_eq!(parse_32(&entry_64(0x7fff_0000, 0x400)), None);
        assert_eq!(parse_32(&[]), None);
        assert_eq!(parse_64(&[]), None);
    }

    #[test]
    fn the_table_ends_after_the_end_of_table_structure() {
        let mut table = structure(1, 0x1a, &["QEMU", "Standard PC"]);
        let first = table.len();
        table.extend_from_slice(&structure(4, 0x2a, &[]));
        let second = table.len();
        table.extend_from_slice(&structure(END_OF_TABLE, 0, &[]));
        let end = table.len();
        // Whatever the entry point's maximum leaves after it is not table.
        table.extend_from_slice(&[0xff; 64]);
        assert_eq!(table_end(&table), Some(end));
        assert_eq!(table_end(&table[first..]), Some(end - first));
        assert_eq!(table_end(&table[second..]), Some(end - second));
    }

    #[test]
    fn a_table_that_never_ends_is_refused() {
        // No end-of-table structure.
        let table = structure(1, 0x1a, &["QEMU"]);
        assert_eq!(table_end(&table), None);
        // A header that cannot be one (a length shorter than the header).
        assert_eq!(table_end(&[1, 2, 0, 0, 0, 0]), None);
        // Strings that are never terminated.
        let mut table = structure(END_OF_TABLE, 0, &["x"]);
        table.pop();
        table.pop();
        assert_eq!(table_end(&table), None);
        assert_eq!(table_end(&[]), None);
    }
}
