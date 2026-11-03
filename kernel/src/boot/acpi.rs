//! The firmware's processor list: the ACPI MADT's local APIC entries
//! (ACPI specification, "Multiple APIC Description Table"), so the kernel
//! knows how many CPUs the machine has even when the bootloader started
//! fewer (ARCHITECTURE "Processors": an AP that does not answer Limine in
//! time is left out of its MP response).
//!
//! It also finds the clock references the TSC is calibrated against (ARCHITECTURE
//! "Interrupts and time"): the HPET table's base address and the FADT's
//! ACPI PM timer port.
//!
//! Parsing works on byte slices (host-tested); `read_madt` maps the tables
//! through the HHDM, where Limine (base revision >= 4) guarantees them.

/// At most this many processors are listed (as `arch::smp::MAX_CPUS`).
pub const MAX_FIRMWARE_CPUS: usize = 64;

/// The local APIC ids of the processors the MADT lists as enabled.
#[derive(Clone, Copy, Debug)]
pub struct FirmwareCpus {
    ids: [u32; MAX_FIRMWARE_CPUS],
    count: usize,
}

impl FirmwareCpus {
    pub const fn empty() -> Self {
        Self { ids: [0; MAX_FIRMWARE_CPUS], count: 0 }
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn ids(&self) -> &[u32] {
        &self.ids[..self.count]
    }

    fn push(&mut self, id: u32) {
        if self.count < MAX_FIRMWARE_CPUS && !self.ids().contains(&id) {
            self.ids[self.count] = id;
            self.count += 1;
        }
    }
}

/// Why the MADT could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcpiError {
    NoRsdp,
    BadRsdp,
    BadTable(&'static str),
    NoMadt,
}

const SDT_HEADER_LEN: usize = 36;
const MADT_ENTRIES: usize = SDT_HEADER_LEN + 8;
/// MADT local APIC flags: bit 0 enabled (bit 1, online capable, is a CPU
/// that may be hot-added later: not one that runs now).
const LAPIC_ENABLED: u32 = 1;

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn checksum_ok(b: &[u8]) -> bool {
    b.iter().fold(0u8, |a, &x| a.wrapping_add(x)) == 0
}

/// The RSDP's root table: (physical address, is XSDT).
pub fn parse_rsdp(rsdp: &[u8]) -> Result<(u64, bool), AcpiError> {
    if rsdp.get(..8) != Some(b"RSD PTR ") || !rsdp.get(..20).is_some_and(checksum_ok) {
        return Err(AcpiError::BadRsdp);
    }
    let revision = rsdp[15];
    if revision >= 2
        && let Some(xsdt) = u64_at(rsdp, 24).filter(|&a| a != 0)
    {
        return Ok((xsdt, true));
    }
    let rsdt = u32_at(rsdp, 16).ok_or(AcpiError::BadRsdp)?;
    Ok((u64::from(rsdt), false))
}

/// A table's length from its header (`None` if shorter than a header).
pub fn table_len(header: &[u8]) -> Option<usize> {
    let len = u32_at(header, 4)? as usize;
    (len >= SDT_HEADER_LEN).then_some(len)
}

/// The physical addresses of the tables a root table (RSDT or XSDT) lists.
pub fn root_entries(root: &[u8], xsdt: bool) -> Result<impl Iterator<Item = u64> + '_, AcpiError> {
    let want: &[u8; 4] = if xsdt { b"XSDT" } else { b"RSDT" };
    if root.get(..4) != Some(want) || !checksum_ok(root) {
        return Err(AcpiError::BadTable(if xsdt { "XSDT" } else { "RSDT" }));
    }
    let width = if xsdt { 8 } else { 4 };
    let body = root.get(SDT_HEADER_LEN..).unwrap_or(&[]);
    Ok(body.chunks_exact(width).map(move |c| {
        if xsdt { u64_at(c, 0).unwrap_or(0) } else { u64::from(u32_at(c, 0).unwrap_or(0)) }
    }))
}

/// The enabled processors of a MADT (types 0, local APIC, and 9, local
/// x2APIC).
pub fn parse_madt(madt: &[u8]) -> Result<FirmwareCpus, AcpiError> {
    if madt.get(..4) != Some(b"APIC") || !checksum_ok(madt) {
        return Err(AcpiError::BadTable("MADT"));
    }
    let mut cpus = FirmwareCpus::empty();
    let mut at = MADT_ENTRIES;
    while let (Some(&kind), Some(&len)) = (madt.get(at), madt.get(at + 1)) {
        let len = usize::from(len);
        if len < 2 {
            break;
        }
        let Some(entry) = madt.get(at..at + len) else { break };
        match kind {
            // Processor UID (1), APIC id (1), flags (4).
            0 if len >= 8 => {
                if u32_at(entry, 4).is_some_and(|f| f & LAPIC_ENABLED != 0) {
                    cpus.push(u32::from(entry[3]));
                }
            }
            // Reserved (2), x2APIC id (4), flags (4), UID (4).
            9 if len >= 16 => {
                if let (Some(id), Some(flags)) = (u32_at(entry, 4), u32_at(entry, 8))
                    && flags & LAPIC_ENABLED != 0
                {
                    cpus.push(id);
                }
            }
            _ => {}
        }
        at += len;
    }
    Ok(cpus)
}

/// The ACPI PM timer (FADT PM_TMR_BLK / X_PM_TMR_BLK): an I/O port
/// counting at 3.579545 MHz, 24 bits wide (32 with TMR_VAL_EXT).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PmTimer {
    pub port: u16,
    pub bits32: bool,
}

/// The firmware's wall-clock references for calibration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClockTables {
    /// The HPET's physical MMIO base (HPET table, memory space).
    pub hpet: Option<u64>,
    pub pm_timer: Option<PmTimer>,
}

/// The HPET table's base address (a Generic Address Structure at offset
/// 40: space id 0 = memory, the address at 44).
pub fn parse_hpet(t: &[u8]) -> Option<u64> {
    if t.get(..4) != Some(b"HPET") || !checksum_ok(t) || *t.get(40)? != 0 {
        return None;
    }
    u64_at(t, 44).filter(|&a| a != 0)
}

/// The FADT's PM timer: X_PM_TMR_BLK (GAS at 208, space id 1 = I/O) when
/// the table is long enough and it is set, else PM_TMR_BLK (76) with
/// PM_TMR_LEN (91) 4; TMR_VAL_EXT is flag bit 8 (flags at 112).
pub fn parse_fadt(t: &[u8]) -> Option<PmTimer> {
    const TMR_VAL_EXT: u32 = 1 << 8;
    if t.get(..4) != Some(b"FACP") || !checksum_ok(t) {
        return None;
    }
    let bits32 = u32_at(t, 112).is_some_and(|f| f & TMR_VAL_EXT != 0);
    let extended = (t.get(208) == Some(&1)).then(|| u64_at(t, 212)).flatten().filter(|&a| a != 0);
    let legacy = (t.get(91) == Some(&4)).then(|| u32_at(t, 76)).flatten().map(u64::from);
    let port = extended.or(legacy).filter(|&p| p != 0)?;
    Some(PmTimer { port: u16::try_from(port).ok()?, bits32 })
}

/// Reads `len` bytes at physical `phys` (HHDM-mapped).
///
/// # Safety
/// The range must be mapped in the HHDM and stay unchanged while used.
unsafe fn phys(hhdm_offset: u64, phys: u64, len: usize) -> &'static [u8] {
    // SAFETY: forwarded to the caller.
    unsafe { super::phys_bytes(hhdm_offset, phys, len as u64) }
}

/// The MADT's processors, from the RSDP at virtual `rsdp` (HHDM).
pub fn read_madt(hhdm_offset: u64, rsdp_virt: u64) -> Result<FirmwareCpus, AcpiError> {
    parse_madt(read_table(hhdm_offset, rsdp_virt, b"APIC")?.ok_or(AcpiError::NoMadt)?)
}

/// The HPET and PM timer, from the RSDP at virtual `rsdp` (HHDM); none
/// when the tables cannot be read.
pub fn read_clock_tables(hhdm_offset: u64, rsdp_virt: u64) -> ClockTables {
    let table = |sig| read_table(hhdm_offset, rsdp_virt, sig).ok().flatten();
    ClockTables {
        hpet: table(b"HPET").and_then(parse_hpet),
        pm_timer: table(b"FACP").and_then(parse_fadt),
    }
}

/// The first table with signature `sig` the root table lists.
fn read_table(
    hhdm_offset: u64,
    rsdp_virt: u64,
    sig: &[u8; 4],
) -> Result<Option<&'static [u8]>, AcpiError> {
    if rsdp_virt == 0 {
        return Err(AcpiError::NoRsdp);
    }
    let rsdp_phys = rsdp_virt.wrapping_sub(hhdm_offset);
    // SAFETY (all `phys` calls): Limine (base revision >= 4) maps the RSDP,
    // the root table and every table it lists in the HHDM, in memory it
    // reports as ACPI or reserved-mapped, which nothing writes. Each table
    // is read with its header's length after the header itself.
    let rsdp = unsafe { phys(hhdm_offset, rsdp_phys, 36) };
    let (root_phys, xsdt) = parse_rsdp(rsdp)?;
    let header = unsafe { phys(hhdm_offset, root_phys, SDT_HEADER_LEN) };
    let len = table_len(header).ok_or(AcpiError::BadTable("root"))?;
    let root = unsafe { phys(hhdm_offset, root_phys, len) };
    for table in root_entries(root, xsdt)? {
        let header = unsafe { phys(hhdm_offset, table, SDT_HEADER_LEN) };
        if header.get(..4) != Some(sig) {
            continue;
        }
        let len = table_len(header).ok_or(AcpiError::BadTable("table length"))?;
        return Ok(Some(unsafe { phys(hhdm_offset, table, len) }));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix_checksum(b: &mut [u8], at: usize, range: core::ops::Range<usize>) {
        b[at] = 0;
        let sum = b[range].iter().fold(0u8, |a, &x| a.wrapping_add(x));
        b[at] = 0u8.wrapping_sub(sum);
    }

    fn table(sig: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut t = vec![0u8; SDT_HEADER_LEN];
        t[..4].copy_from_slice(sig);
        t.extend_from_slice(body);
        let len = t.len() as u32;
        t[4..8].copy_from_slice(&len.to_le_bytes());
        let n = t.len();
        fix_checksum(&mut t, 9, 0..n);
        t
    }

    #[test]
    fn madt_lists_enabled_processors() {
        let mut body = vec![0u8; 8]; // local APIC address, flags
        body.extend_from_slice(&[0, 8, 0, 0, 1, 0, 0, 0]); // UID 0, id 0, enabled
        body.extend_from_slice(&[0, 8, 1, 1, 1, 0, 0, 0]); // id 1, enabled
        body.extend_from_slice(&[0, 8, 2, 2, 2, 0, 0, 0]); // id 2, online capable only
        body.extend_from_slice(&[1, 12, 0, 0, 0, 0, 0xc0, 0xfe, 0, 0, 0, 0]); // I/O APIC
        let mut x2 = vec![9, 16, 0, 0];
        x2.extend_from_slice(&300u32.to_le_bytes());
        x2.extend_from_slice(&1u32.to_le_bytes());
        x2.extend_from_slice(&3u32.to_le_bytes());
        body.extend_from_slice(&x2);
        body.extend_from_slice(&[0, 8, 3, 1, 1, 0, 0, 0]); // id 1 again: once
        let madt = table(b"APIC", &body);
        let cpus = parse_madt(&madt).unwrap();
        assert_eq!(cpus.ids(), &[0, 1, 300]);
        let mut bad = madt.clone();
        bad[40] ^= 1;
        assert_eq!(parse_madt(&bad).unwrap_err(), AcpiError::BadTable("MADT"));
    }

    #[test]
    fn madt_stops_at_a_truncated_entry() {
        let mut body = vec![0u8; 8];
        body.extend_from_slice(&[0, 8, 0, 0, 1, 0, 0, 0]);
        body.extend_from_slice(&[0, 0]); // zero length: stop
        let madt = table(b"APIC", &body);
        assert_eq!(parse_madt(&madt).unwrap().ids(), &[0]);
    }

    #[test]
    fn clock_tables() {
        // HPET: event timer block id (4), base address GAS (12), ...
        let mut body = vec![0u8; 20];
        body[4] = 0; // memory space
        body[8..16].copy_from_slice(&0xfed0_0000u64.to_le_bytes());
        let hpet = table(b"HPET", &body);
        assert_eq!(parse_hpet(&hpet), Some(0xfed0_0000));
        let mut io = body.clone();
        io[4] = 1;
        assert_eq!(parse_hpet(&table(b"HPET", &io)), None);

        // FADT (ACPI 1 length 116): PM_TMR_BLK 0x608, length 4, 24 bits.
        let mut fadt = vec![0u8; 116 - SDT_HEADER_LEN];
        fadt[76 - 36..80 - 36].copy_from_slice(&0x608u32.to_le_bytes());
        fadt[91 - 36] = 4;
        let t = table(b"FACP", &fadt);
        assert_eq!(parse_fadt(&t), Some(PmTimer { port: 0x608, bits32: false }));
        // TMR_VAL_EXT and an X_PM_TMR_BLK in I/O space.
        let mut long = vec![0u8; 244 - SDT_HEADER_LEN];
        long[76 - 36..80 - 36].copy_from_slice(&0x608u32.to_le_bytes());
        long[91 - 36] = 4;
        long[112 - 36..116 - 36].copy_from_slice(&(1u32 << 8).to_le_bytes());
        long[208 - 36] = 1;
        long[212 - 36..220 - 36].copy_from_slice(&0x408u64.to_le_bytes());
        assert_eq!(parse_fadt(&table(b"FACP", &long)), Some(PmTimer { port: 0x408, bits32: true }));
        // No timer.
        assert_eq!(parse_fadt(&table(b"FACP", &[0u8; 80])), None);
    }

    #[test]
    fn rsdp_and_root_tables() {
        let mut rsdp = vec![0u8; 36];
        rsdp[..8].copy_from_slice(b"RSD PTR ");
        rsdp[16..20].copy_from_slice(&0x1234u32.to_le_bytes());
        fix_checksum(&mut rsdp, 8, 0..20);
        assert_eq!(parse_rsdp(&rsdp), Ok((0x1234, false)));
        rsdp[15] = 2;
        rsdp[24..32].copy_from_slice(&0x5678u64.to_le_bytes());
        fix_checksum(&mut rsdp, 8, 0..20);
        assert_eq!(parse_rsdp(&rsdp), Ok((0x5678, true)));
        rsdp[0] = b'X';
        assert_eq!(parse_rsdp(&rsdp), Err(AcpiError::BadRsdp));

        let mut body = Vec::new();
        body.extend_from_slice(&0x1000u64.to_le_bytes());
        body.extend_from_slice(&0x2000u64.to_le_bytes());
        let xsdt = table(b"XSDT", &body);
        assert_eq!(table_len(&xsdt), Some(xsdt.len()));
        assert_eq!(root_entries(&xsdt, true).unwrap().collect::<Vec<_>>(), [0x1000, 0x2000]);
        let rsdt = table(b"RSDT", &[0, 0x30, 0, 0]);
        assert_eq!(root_entries(&rsdt, false).unwrap().collect::<Vec<_>>(), [0x3000]);
        assert!(root_entries(&rsdt, true).is_err());
    }
}
