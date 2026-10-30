//! Physical memory map as handed over by the bootloader, plus validation of
//! the guarantees the Limine protocol makes about it.

use core::fmt;

use super::PAGE_SIZE;

/// What a physical address range is used for. Mirrors the Limine memory map
/// types, which are our only source of this information.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionKind {
    Usable,
    Reserved,
    AcpiReclaimable,
    AcpiNvs,
    BadMemory,
    BootloaderReclaimable,
    ExecutableAndModules,
    Framebuffer,
    ReservedMapped,
    /// A type this kernel does not know about. Treated as reserved.
    Unknown(u64),
}

impl RegionKind {
    /// Every known kind, in Limine type-number order.
    pub const KNOWN: [RegionKind; 9] = [
        RegionKind::Usable,
        RegionKind::Reserved,
        RegionKind::AcpiReclaimable,
        RegionKind::AcpiNvs,
        RegionKind::BadMemory,
        RegionKind::BootloaderReclaimable,
        RegionKind::ExecutableAndModules,
        RegionKind::Framebuffer,
        RegionKind::ReservedMapped,
    ];

    /// Converts a Limine `LIMINE_MEMMAP_*` value.
    pub fn from_limine(raw: u64) -> Self {
        usize::try_from(raw)
            .ok()
            .and_then(|i| Self::KNOWN.get(i).copied())
            .unwrap_or(RegionKind::Unknown(raw))
    }

    fn known_index(self) -> Option<usize> {
        Self::KNOWN.iter().position(|&k| k == self)
    }

    pub fn name(self) -> &'static str {
        match self {
            RegionKind::Usable => "usable",
            RegionKind::Reserved => "reserved",
            RegionKind::AcpiReclaimable => "ACPI reclaimable",
            RegionKind::AcpiNvs => "ACPI NVS",
            RegionKind::BadMemory => "bad memory",
            RegionKind::BootloaderReclaimable => "bootloader reclaimable",
            RegionKind::ExecutableAndModules => "executable and modules",
            RegionKind::Framebuffer => "framebuffer",
            RegionKind::ReservedMapped => "reserved (mapped)",
            RegionKind::Unknown(_) => "unknown",
        }
    }

    /// Kinds that the protocol guarantees are page-aligned and never overlap
    /// any other region, i.e. RAM we may eventually hand to the allocator.
    fn is_exclusive_ram(self) -> bool {
        matches!(self, RegionKind::Usable | RegionKind::BootloaderReclaimable)
    }
}

impl fmt::Display for RegionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegionKind::Unknown(raw) => write!(f, "unknown ({raw})"),
            other => f.write_str(other.name()),
        }
    }
}

/// A physical address range `[base, base + len)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryRegion {
    pub base: u64,
    pub len: u64,
    pub kind: RegionKind,
}

impl MemoryRegion {
    /// Exclusive end address. Saturates; [`MemorySummary::from_regions`]
    /// rejects regions whose end would overflow.
    pub fn end(&self) -> u64 {
        self.base.saturating_add(self.len)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryMapError {
    /// Region `index` ends past the top of the 64-bit address space.
    Overflow { index: usize },
    /// Region `index` starts below the region before it.
    Unsorted { index: usize },
    /// Usable or bootloader-reclaimable region `index` is not page-aligned.
    Misaligned { index: usize },
    /// Region `index` overlaps a region that must not overlap anything.
    Overlap { index: usize },
}

impl fmt::Display for MemoryMapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            MemoryMapError::Overflow { index } => {
                write!(f, "entry {index} wraps the address space")
            }
            MemoryMapError::Unsorted { index } => {
                write!(f, "entry {index} is not sorted by base address")
            }
            MemoryMapError::Misaligned { index } => {
                write!(f, "entry {index} is RAM but not page-aligned")
            }
            MemoryMapError::Overlap { index } => {
                write!(f, "entry {index} overlaps exclusive RAM")
            }
        }
    }
}

/// Totals over a validated memory map.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemorySummary {
    pub region_count: usize,
    pub usable_bytes: u64,
    /// Exclusive end of the highest usable region, or 0 if there is none.
    pub highest_usable_end: u64,
    pub unknown_bytes: u64,
    bytes_by_kind: [u64; RegionKind::KNOWN.len()],
}

impl MemorySummary {
    /// Validates `regions` against the Limine memory map guarantees (sorted by
    /// base; usable and bootloader-reclaimable entries page-aligned and not
    /// overlapping anything) and totals them.
    pub fn from_regions(
        regions: impl IntoIterator<Item = MemoryRegion>,
    ) -> Result<Self, MemoryMapError> {
        let mut summary = MemorySummary::default();
        let mut prev_base = 0;
        // Highest end seen so far, over all regions / over exclusive RAM only.
        let mut max_end = 0;
        let mut max_exclusive_end = 0;

        for (index, region) in regions.into_iter().enumerate() {
            let end = region
                .base
                .checked_add(region.len)
                .ok_or(MemoryMapError::Overflow { index })?;
            if region.base < prev_base {
                return Err(MemoryMapError::Unsorted { index });
            }
            prev_base = region.base;

            if region.len != 0 {
                if region.kind.is_exclusive_ram() {
                    if region.base % PAGE_SIZE != 0 || region.len % PAGE_SIZE != 0 {
                        return Err(MemoryMapError::Misaligned { index });
                    }
                    if region.base < max_end {
                        return Err(MemoryMapError::Overlap { index });
                    }
                    max_exclusive_end = end;
                } else if region.base < max_exclusive_end {
                    return Err(MemoryMapError::Overlap { index });
                }
                max_end = max_end.max(end);
            }

            summary.region_count += 1;
            match region.kind.known_index() {
                Some(i) => summary.bytes_by_kind[i] += region.len,
                None => summary.unknown_bytes += region.len,
            }
            if region.kind == RegionKind::Usable && region.len != 0 {
                summary.usable_bytes += region.len;
                summary.highest_usable_end = end;
            }
        }
        Ok(summary)
    }

    /// Total bytes of regions of a known `kind` (0 for [`RegionKind::Unknown`]).
    pub fn bytes_of(&self, kind: RegionKind) -> u64 {
        kind.known_index().map_or(0, |i| self.bytes_by_kind[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(base: u64, len: u64, kind: RegionKind) -> MemoryRegion {
        MemoryRegion { base, len, kind }
    }

    use RegionKind::*;

    #[test]
    fn kind_from_limine_numbers() {
        assert_eq!(RegionKind::from_limine(0), Usable);
        assert_eq!(RegionKind::from_limine(5), BootloaderReclaimable);
        assert_eq!(RegionKind::from_limine(8), ReservedMapped);
        assert_eq!(RegionKind::from_limine(9), Unknown(9));
        assert_eq!(RegionKind::from_limine(u64::MAX), Unknown(u64::MAX));
    }

    #[test]
    fn summarizes_typical_map() {
        let map = [
            r(0x1000, 0x9e000, Usable),
            r(0x9f000, 0x1000, Reserved),
            r(0x100000, 0x700000, Usable),
            r(0x800000, 0x8000, AcpiNvs),
            r(0x808000, 0x10000, BootloaderReclaimable),
            r(0x900000, 0x100000, ExecutableAndModules),
            r(0xfd000000, 0x300000, Framebuffer),
            r(0xffe00000, 0x200000, Unknown(42)),
        ];
        let s = MemorySummary::from_regions(map).unwrap();
        assert_eq!(s.region_count, 8);
        assert_eq!(s.usable_bytes, 0x9e000 + 0x700000);
        assert_eq!(s.highest_usable_end, 0x800000);
        assert_eq!(s.bytes_of(AcpiNvs), 0x8000);
        assert_eq!(s.bytes_of(BootloaderReclaimable), 0x10000);
        assert_eq!(s.bytes_of(Unknown(42)), 0);
        assert_eq!(s.unknown_bytes, 0x200000);
    }

    #[test]
    fn empty_map_has_no_usable_memory() {
        let s = MemorySummary::from_regions([]).unwrap();
        assert_eq!(s.usable_bytes, 0);
        assert_eq!(s.highest_usable_end, 0);
    }

    #[test]
    fn rejects_unsorted() {
        let map = [r(0x2000, 0x1000, Usable), r(0x1000, 0x1000, Usable)];
        assert_eq!(
            MemorySummary::from_regions(map),
            Err(MemoryMapError::Unsorted { index: 1 })
        );
    }

    #[test]
    fn rejects_misaligned_ram_but_not_reserved() {
        let map = [r(0x1000, 0x800, Usable)];
        assert_eq!(
            MemorySummary::from_regions(map),
            Err(MemoryMapError::Misaligned { index: 0 })
        );
        let map = [r(0x1234, 0x800, Reserved)];
        assert!(MemorySummary::from_regions(map).is_ok());
    }

    #[test]
    fn reserved_regions_may_overlap_each_other() {
        let map = [r(0x1000, 0x3000, Reserved), r(0x2000, 0x3000, AcpiNvs)];
        assert!(MemorySummary::from_regions(map).is_ok());
    }

    #[test]
    fn rejects_overlap_with_exclusive_ram() {
        // Reserved region starting inside usable RAM.
        let map = [r(0x1000, 0x2000, Usable), r(0x2000, 0x10, Reserved)];
        assert_eq!(
            MemorySummary::from_regions(map),
            Err(MemoryMapError::Overlap { index: 1 })
        );
        // Usable RAM starting inside an earlier, long reserved region.
        let map = [
            r(0x0, 0x10000, Reserved),
            r(0x1000, 0x1000, AcpiNvs),
            r(0x4000, 0x1000, Usable),
        ];
        assert_eq!(
            MemorySummary::from_regions(map),
            Err(MemoryMapError::Overlap { index: 2 })
        );
    }

    #[test]
    fn adjacent_regions_do_not_overlap() {
        let map = [
            r(0x1000, 0x1000, Reserved),
            r(0x2000, 0x1000, Usable),
            r(0x3000, 1, Reserved),
        ];
        assert!(MemorySummary::from_regions(map).is_ok());
    }

    #[test]
    fn rejects_overflow() {
        let map = [r(u64::MAX - 0xfff, 0x2000, Reserved)];
        assert_eq!(
            MemorySummary::from_regions(map),
            Err(MemoryMapError::Overflow { index: 0 })
        );
    }
}
