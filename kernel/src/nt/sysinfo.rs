//! Encoders for NtQuerySystemInformation and NtQueryInformationProcess
//! results (public x64 layouts: winternl.h / mingw-w64), kept pure so they
//! can be host-tested.

use alloc::vec::Vec;

pub const SYSTEM_BASIC_INFORMATION: u32 = 0;
pub const SYSTEM_TIME_OF_DAY_INFORMATION: u32 = 3;

/// Lowest and highest user-mode addresses reported (64 KiB null region,
/// top 64 KiB below 2^47 reserved), as on x64 Windows.
pub const MINIMUM_USER_ADDRESS: u64 = 0x1_0000;
pub const MAXIMUM_USER_ADDRESS: u64 = 0x7fff_fffe_ffff;

/// SYSTEM_BASIC_INFORMATION (64 bytes).
pub fn basic_information(physical_pages: u64, processors: u8, timer_resolution: u32) -> [u8; 64] {
    let mut b = [0u8; 64];
    b[4..8].copy_from_slice(&timer_resolution.to_le_bytes()); // 100 ns units
    b[8..12].copy_from_slice(&4096u32.to_le_bytes()); // PageSize
    let pages = u32::try_from(physical_pages).unwrap_or(u32::MAX);
    b[12..16].copy_from_slice(&pages.to_le_bytes()); // NumberOfPhysicalPages
    b[16..20].copy_from_slice(&1u32.to_le_bytes()); // LowestPhysicalPageNumber
    b[20..24].copy_from_slice(&pages.to_le_bytes()); // HighestPhysicalPageNumber
    b[24..28].copy_from_slice(&0x1_0000u32.to_le_bytes()); // AllocationGranularity
    b[32..40].copy_from_slice(&MINIMUM_USER_ADDRESS.to_le_bytes());
    b[40..48].copy_from_slice(&MAXIMUM_USER_ADDRESS.to_le_bytes());
    let mask = if processors >= 64 { u64::MAX } else { (1u64 << processors) - 1 };
    b[48..56].copy_from_slice(&mask.to_le_bytes()); // ActiveProcessorsAffinityMask
    b[56] = processors;
    b
}

/// SYSTEM_TIMEOFDAY_INFORMATION (48 bytes): boot and current NT times; time
/// zone fields are UTC (bias 0).
pub fn time_of_day(boot: u64, now: u64) -> [u8; 48] {
    let mut b = [0u8; 48];
    b[0..8].copy_from_slice(&boot.to_le_bytes());
    b[8..16].copy_from_slice(&now.to_le_bytes());
    b
}

/// A UNICODE_STRING header followed by the string and a terminator, as
/// returned in place to a buffer at user address `buffer`. Returns the
/// bytes; their length is the required ReturnLength.
pub fn unicode_string_in_place(name: &[u16], buffer: u64) -> Option<Vec<u8>> {
    let bytes = name.len() * 2;
    let length = u16::try_from(bytes + 2).ok()?;
    let mut out = Vec::new();
    out.try_reserve_exact(16 + bytes + 2).ok()?;
    out.resize(16 + bytes + 2, 0u8);
    out[0..2].copy_from_slice(&((bytes) as u16).to_le_bytes());
    out[2..4].copy_from_slice(&length.to_le_bytes());
    out[8..16].copy_from_slice(&(buffer + 16).to_le_bytes());
    for (i, c) in name.iter().enumerate() {
        out[16 + 2 * i..18 + 2 * i].copy_from_slice(&c.to_le_bytes());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_information_layout() {
        let b = basic_information(0x2_0000, 1, 10_000);
        assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()), 4096);
        assert_eq!(u32::from_le_bytes(b[12..16].try_into().unwrap()), 0x2_0000);
        assert_eq!(u32::from_le_bytes(b[24..28].try_into().unwrap()), 0x1_0000);
        assert_eq!(u64::from_le_bytes(b[40..48].try_into().unwrap()), 0x7fff_fffe_ffff);
        assert_eq!(u64::from_le_bytes(b[48..56].try_into().unwrap()), 1);
        assert_eq!(b[56], 1);
        assert_eq!(u64::from_le_bytes(basic_information(1, 4, 0)[48..56].try_into().unwrap()), 0xf);
    }

    #[test]
    fn unicode_string_layout() {
        let name: Vec<u16> = "\\A".encode_utf16().collect();
        let b = unicode_string_in_place(&name, 0x1000).unwrap();
        assert_eq!(b.len(), 16 + 4 + 2);
        assert_eq!(u16::from_le_bytes([b[0], b[1]]), 4);
        assert_eq!(u16::from_le_bytes([b[2], b[3]]), 6);
        assert_eq!(u64::from_le_bytes(b[8..16].try_into().unwrap()), 0x1010);
        assert_eq!(&b[16..22], &[b'\\', 0, b'A', 0, 0, 0]);
    }
}
