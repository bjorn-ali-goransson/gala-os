//! The disk as Windows names it (D1): `\Device\Harddisk0\DR0`, with the
//! links `\Device\Harddisk0\Partition0` and `\??\PhysicalDrive0`, over
//! the platform's disk (`platform::disk_*`). Opening it (administrators and SYSTEM only, as on
//! Windows) gives raw sector access: reads and writes of whole sectors at
//! sector offsets, the disk IOCTLs below, FlushFileBuffers.
//! Design: docs/ARCHITECTURE.md, "Disk".

use super::namespace;
use super::object::{KObject, ObjectBody};
use super::platform::{self, DISK_SECTOR as SECTOR, DiskError};
use super::security::{self, Descriptor, SE_DACL_PRESENT};
use super::status::NtStatus;
use super::token::{LOCAL_SYSTEM, Sid};
use super::wait::Dispatcher;
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;
use alloc::vec::Vec;

pub const DEVICE: &str = "\\Device\\Harddisk0\\DR0";

pub const IOCTL_DISK_GET_DRIVE_GEOMETRY: u32 = 0x0007_0000;
pub const IOCTL_DISK_GET_DRIVE_GEOMETRY_EX: u32 = 0x0007_00a0;
pub const IOCTL_DISK_GET_LENGTH_INFO: u32 = 0x0007_405c;
/// MEDIA_TYPE FixedMedia.
const FIXED_MEDIA: u32 = 12;
/// The geometry Windows reports for disks (LBA translation).
const HEADS: u64 = 255;
const SECTORS_PER_TRACK: u64 = 63;

/// An open of the disk.
pub struct DiskFile {
    /// The file pointer of a synchronous handle.
    pub position: IrqMutex<u64>,
    pub synchronous: bool,
    /// Signaled: transfers complete in the call.
    pub dispatcher: Dispatcher,
}

impl DiskFile {
    pub fn new(synchronous: bool) -> Self {
        Self { position: IrqMutex::new(0), synchronous, dispatcher: Dispatcher::new(1) }
    }
}

fn wide(s: &str) -> Result<Vec<u16>, NtStatus> {
    let mut v = Vec::new();
    for c in s.encode_utf16() {
        v.try_push(c).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(v)
}

/// What the disk device grants: everything to Administrators and SYSTEM,
/// nothing to anyone else.
fn admin_only() -> Result<Vec<u8>, NtStatus> {
    const ADMINISTRATORS: Sid = Sid::new(5, &[32, 544]);
    const FILE_ALL_ACCESS: u32 = 0x001f_01ff;
    let dacl = security::allow_acl(&[
        (FILE_ALL_ACCESS, ADMINISTRATORS, 0),
        (FILE_ALL_ACCESS, LOCAL_SYSTEM, 0),
    ])?;
    Descriptor {
        control: SE_DACL_PRESENT,
        owner: Some(ADMINISTRATORS),
        group: Some(LOCAL_SYSTEM),
        dacl: Some(dacl),
        sacl: None,
    }
    .to_bytes(u32::MAX)
}

/// Names the disk, if the machine has one.
pub fn init() -> Result<(), NtStatus> {
    if platform::disk_sectors() == 0 {
        return Ok(());
    }
    namespace::create_directory(&wide("\\Device\\Harddisk0")?)?;
    let device =
        KObject::with_permanence(ObjectBody::DiskDevice, true).map_err(|_| NtStatus::NO_MEMORY)?;
    *device.security.lock() = Some(admin_only()?);
    namespace::create(None, &wide(DEVICE)?, device, false, false)?;
    for name in ["\\Device\\Harddisk0\\Partition0", "\\??\\PhysicalDrive0"] {
        let link = namespace::new_symbolic_link(&wide(DEVICE)?)?;
        namespace::create(None, &wide(name)?, link, false, false)?;
    }
    Ok(())
}

pub fn status(e: DiskError) -> NtStatus {
    match e {
        // Windows' code for a transfer beyond the disk is unverified.
        DiskError::OutOfRange | DiskError::Unaligned => NtStatus::INVALID_PARAMETER,
        DiskError::Device => NtStatus::IO_DEVICE_ERROR,
        DiskError::ReadOnly => NtStatus::MEDIA_WRITE_PROTECTED,
    }
}

/// The first sector of a transfer at byte `offset` (STATUS_INVALID_PARAMETER
/// unless it is a sector boundary).
pub fn sector_of(offset: u64) -> Result<u64, NtStatus> {
    if !offset.is_multiple_of(SECTOR as u64) {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    Ok(offset / SECTOR as u64)
}

/// DISK_GEOMETRY { Cylinders, MediaType, TracksPerCylinder,
/// SectorsPerTrack, BytesPerSector } of a disk of `sectors` sectors.
fn geometry(sectors: u64) -> [u8; 24] {
    let mut g = [0u8; 24];
    g[0..8].copy_from_slice(&(sectors / (HEADS * SECTORS_PER_TRACK)).to_le_bytes());
    g[8..12].copy_from_slice(&FIXED_MEDIA.to_le_bytes());
    g[12..16].copy_from_slice(&(HEADS as u32).to_le_bytes());
    g[16..20].copy_from_slice(&(SECTORS_PER_TRACK as u32).to_le_bytes());
    g[20..24].copy_from_slice(&(SECTOR as u32).to_le_bytes());
    g
}

/// The output of disk IOCTL `code` for a disk of `sectors` sectors, if it
/// is one gala knows (STATUS_INVALID_DEVICE_REQUEST otherwise); the caller
/// fails with STATUS_BUFFER_TOO_SMALL if the output does not fit.
/// GEOMETRY_EX gives DISK_GEOMETRY and DiskSize, without the partition
/// and detection information Windows appends.
pub fn control(code: u32, sectors: u64) -> Result<Vec<u8>, NtStatus> {
    let mut out = Vec::new();
    let size = sectors * SECTOR as u64;
    match code {
        IOCTL_DISK_GET_LENGTH_INFO => out.try_extend_from_slice(&size.to_le_bytes()),
        IOCTL_DISK_GET_DRIVE_GEOMETRY => out.try_extend_from_slice(&geometry(sectors)),
        IOCTL_DISK_GET_DRIVE_GEOMETRY_EX => out
            .try_extend_from_slice(&geometry(sectors))
            .and_then(|()| out.try_extend_from_slice(&size.to_le_bytes())),
        _ => return Err(NtStatus::INVALID_DEVICE_REQUEST),
    }
    .map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(out)
}

/// Whether IOCTL `code` needs read access (its RequiredAccess bits).
pub fn needs_read(code: u32) -> bool {
    code >> 14 & 1 != 0
}

/// FileStandardInformation of the disk: allocation size and end of file
/// are its size.
pub fn size() -> u64 {
    platform::disk_sectors() * SECTOR as u64
}

/// FILE_DEVICE_DISK, for FileFsDeviceInformation.
pub const FILE_DEVICE_DISK: u32 = 7;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_ioctl_outputs() {
        let sectors = 32768; // 16 MiB
        let len = control(IOCTL_DISK_GET_LENGTH_INFO, sectors).unwrap();
        assert_eq!(len, (16u64 << 20).to_le_bytes());
        let g = control(IOCTL_DISK_GET_DRIVE_GEOMETRY, sectors).unwrap();
        // 2 whole cylinders of 255 * 63 sectors, fixed media, 512 bytes.
        assert_eq!(&g[0..8], &2u64.to_le_bytes());
        assert_eq!(&g[8..12], &12u32.to_le_bytes());
        assert_eq!(&g[12..16], &255u32.to_le_bytes());
        assert_eq!(&g[16..20], &63u32.to_le_bytes());
        assert_eq!(&g[20..24], &512u32.to_le_bytes());
        let ex = control(IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, sectors).unwrap();
        assert_eq!(&ex[..24], &g[..]);
        assert_eq!(&ex[24..], &(16u64 << 20).to_le_bytes());
        assert_eq!(control(0x0007_0004, sectors), Err(NtStatus::INVALID_DEVICE_REQUEST));
        assert!(needs_read(IOCTL_DISK_GET_LENGTH_INFO));
        assert!(!needs_read(IOCTL_DISK_GET_DRIVE_GEOMETRY));
    }

    #[test]
    fn transfers_start_on_sectors() {
        assert_eq!(sector_of(1024), Ok(2));
        assert_eq!(sector_of(7), Err(NtStatus::INVALID_PARAMETER));
    }
}
