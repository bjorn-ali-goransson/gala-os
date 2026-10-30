//! Files: the volume device `\\Device\\HarddiskVolume1` (also `\\??\\C:`) backed
//! by the in-memory file system, file objects and their I/O semantics
//! (dispositions, positions, share access, delete on close), following the
//! documented behaviour of NtCreateFile / NtReadFile / NtWriteFile.

use alloc::vec::Vec;

use super::namespace;
use super::object::{KObject, ObjectBody, ObjectRef};
use super::status::NtStatus;
use crate::fs::ramfs::{self, Access, FsError, NodeId, RamFs, Share};
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;
use crate::time;

static FS: IrqMutex<Option<RamFs>> = IrqMutex::new(None);

/// Runs `f` on the file system (initialized by `init`).
pub fn with_fs<R>(f: impl FnOnce(&mut RamFs) -> R) -> R {
    f(FS.lock().as_mut().expect("file system initialized"))
}

pub const FILE_READ_DATA: u32 = 0x0001;
pub const FILE_WRITE_DATA: u32 = 0x0002;
pub const FILE_APPEND_DATA: u32 = 0x0004;
pub const FILE_READ_ATTRIBUTES: u32 = 0x0080;
pub const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
pub const DELETE: u32 = 0x0001_0000;

pub const FILE_SHARE_READ: u32 = 1;
pub const FILE_SHARE_WRITE: u32 = 2;
pub const FILE_SHARE_DELETE: u32 = 4;

// CreateDisposition and IoStatus.Information values.
pub const FILE_SUPERSEDE: u32 = 0;
pub const FILE_OPEN: u32 = 1;
pub const FILE_CREATE: u32 = 2;
pub const FILE_OPEN_IF: u32 = 3;
pub const FILE_OVERWRITE: u32 = 4;
pub const FILE_OVERWRITE_IF: u32 = 5;
pub const FILE_SUPERSEDED: u64 = 0;
pub const FILE_OPENED: u64 = 1;
pub const FILE_CREATED: u64 = 2;
pub const FILE_OVERWRITTEN: u64 = 3;

// CreateOptions.
pub const FILE_DIRECTORY_FILE: u32 = 0x0001;
pub const FILE_SYNCHRONOUS_IO_ALERT: u32 = 0x0010;
pub const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x0020;
pub const FILE_NON_DIRECTORY_FILE: u32 = 0x0040;
pub const FILE_DELETE_ON_CLOSE: u32 = 0x1000;

/// Directories created at boot.
const DIRECTORIES: &[&[&str]] = &[&["Windows", "System32"], &["Users", "Public"], &["gala"]];

fn wide(s: &str) -> Result<Vec<u16>, NtStatus> {
    let mut v = Vec::new();
    v.try_reserve(s.len()).map_err(|_| NtStatus::NO_MEMORY)?;
    for b in s.bytes() {
        let _ = v.try_push(u16::from(b));
    }
    Ok(v)
}

/// Creates the file system and the volume device.
pub fn init() -> Result<(), NtStatus> {
    let now = time::system_time();
    let mut fs = RamFs::new(now).map_err(|_| NtStatus::NO_MEMORY)?;
    for path in DIRECTORIES {
        let mut parts: Vec<Vec<u16>> = Vec::new();
        for p in *path {
            parts.try_push(wide(p)?).map_err(|_| NtStatus::NO_MEMORY)?;
        }
        let mut refs: Vec<&[u16]> = Vec::new();
        for p in &parts {
            refs.try_push(p).map_err(|_| NtStatus::NO_MEMORY)?;
        }
        fs.mkdir_all(&refs, now).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    *FS.lock() = Some(fs);
    let volume =
        KObject::with_permanence(ObjectBody::Volume, true).map_err(|_| NtStatus::NO_MEMORY)?;
    namespace::create(None, &wide("\\Device\\HarddiskVolume1")?, volume.clone(), false, false)?;
    namespace::create(None, &wide("\\??\\C:")?, volume, false, false)?;
    Ok(())
}

/// Adds a boot module as a read-only file `C:\\gala\\<name>`.
pub fn add_boot_file(name: &str, data: &'static [u8]) -> Result<(), NtStatus> {
    let gala = wide("gala")?;
    let name = wide(name)?;
    with_fs(|fs| {
        let dir = fs.resolve(&[&gala]).map_err(fs_status)?;
        fs.add_static_file(dir, &name, data, time::system_time()).map_err(fs_status)?;
        Ok(())
    })
}

pub fn fs_status(e: FsError) -> NtStatus {
    match e {
        FsError::NotFound => NtStatus::OBJECT_NAME_NOT_FOUND,
        FsError::PathNotFound => NtStatus::OBJECT_PATH_NOT_FOUND,
        FsError::Exists => NtStatus::OBJECT_NAME_COLLISION,
        FsError::NotADirectory => NtStatus::NOT_A_DIRECTORY,
        FsError::IsADirectory => NtStatus::FILE_IS_A_DIRECTORY,
        FsError::SharingViolation => NtStatus::SHARING_VIOLATION,
        FsError::DeletePending => NtStatus::DELETE_PENDING,
        FsError::DirectoryNotEmpty => NtStatus::DIRECTORY_NOT_EMPTY,
        FsError::NoMemory => NtStatus::NO_MEMORY,
    }
}

/// An open file or directory.
pub struct FileObject {
    pub node: NodeId,
    pub directory: bool,
    /// Granted specific rights.
    pub granted: u32,
    access: Access,
    share: Share,
    /// Opened for synchronous I/O: the file position is maintained.
    pub synchronous: bool,
    pub position: IrqMutex<u64>,
}

impl Drop for FileObject {
    fn drop(&mut self) {
        let (node, access, share) = (self.node, self.access, self.share);
        if let Some(fs) = FS.lock().as_mut() {
            fs.close(node, access, share);
        }
    }
}

/// Splits a `\\a\\b\\c` path (relative to a volume) into components.
fn components(rest: &[u16]) -> Result<Vec<&[u16]>, NtStatus> {
    let mut out = Vec::new();
    let Some(body) = rest.strip_prefix(&[u16::from(b'\\')]) else {
        return if rest.is_empty() { Ok(out) } else { Err(NtStatus::OBJECT_NAME_INVALID) };
    };
    for part in body.split(|&c| c == u16::from(b'\\')) {
        if part.is_empty() {
            // A trailing backslash names the directory itself.
            continue;
        }
        out.try_push(part).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(out)
}

/// The result of NtCreateFile.
pub struct Created {
    pub object: ObjectRef,
    pub information: u64,
}

/// Opens or creates `rest` on the volume `device` (NtCreateFile semantics).
pub fn create(
    device: &ObjectRef,
    rest: &[u16],
    granted: u32,
    share_mask: u32,
    disposition: u32,
    options: u32,
) -> Result<Created, NtStatus> {
    if !matches!(device.body, ObjectBody::Volume) {
        return Err(NtStatus::OBJECT_TYPE_MISMATCH);
    }
    if disposition > FILE_OVERWRITE_IF {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    let want_dir = options & FILE_DIRECTORY_FILE != 0;
    let want_file = options & FILE_NON_DIRECTORY_FILE != 0;
    if want_dir && want_file {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    let parts = components(rest)?;
    let access = Access {
        read: granted & FILE_READ_DATA != 0,
        write: granted & (FILE_WRITE_DATA | FILE_APPEND_DATA) != 0,
        delete: granted & DELETE != 0,
    };
    let share = Share {
        read: share_mask & FILE_SHARE_READ != 0,
        write: share_mask & FILE_SHARE_WRITE != 0,
        delete: share_mask & FILE_SHARE_DELETE != 0,
    };
    let now = time::system_time();
    let (node, information) = with_fs(|fs| -> Result<(NodeId, u64), NtStatus> {
        let existing = match parts.last() {
            None => Some(ramfs::ROOT),
            Some(last) => {
                let parent = fs.parent_of(&parts).map_err(fs_status)?;
                fs.find(parent, last).map_err(fs_status)?
            }
        };
        match existing {
            Some(node) => {
                let n = fs.node(node).ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
                if disposition == FILE_CREATE {
                    return Err(NtStatus::OBJECT_NAME_COLLISION);
                }
                if want_dir && !n.is_dir() {
                    return Err(NtStatus::NOT_A_DIRECTORY);
                }
                if want_file && n.is_dir() {
                    return Err(NtStatus::FILE_IS_A_DIRECTORY);
                }
                if access.write && n.attributes & ramfs::ATTRIBUTE_READONLY != 0 {
                    return Err(NtStatus::ACCESS_DENIED);
                }
                let truncate =
                    matches!(disposition, FILE_SUPERSEDE | FILE_OVERWRITE | FILE_OVERWRITE_IF);
                if truncate && n.is_dir() {
                    return Err(NtStatus::INVALID_PARAMETER);
                }
                fs.open(node, access, share).map_err(fs_status)?;
                let information = match disposition {
                    FILE_SUPERSEDE => FILE_SUPERSEDED,
                    FILE_OVERWRITE | FILE_OVERWRITE_IF => FILE_OVERWRITTEN,
                    _ => FILE_OPENED,
                };
                if truncate && let Err(e) = fs.set_size(node, 0, now) {
                    fs.close(node, access, share);
                    return Err(fs_status(e));
                }
                Ok((node, information))
            }
            None => {
                if matches!(disposition, FILE_OPEN | FILE_OVERWRITE) {
                    return Err(NtStatus::OBJECT_NAME_NOT_FOUND);
                }
                let last = parts.last().ok_or(NtStatus::OBJECT_NAME_INVALID)?;
                let parent = fs.parent_of(&parts).map_err(fs_status)?;
                let node = fs.create(parent, last, want_dir, now).map_err(fs_status)?;
                fs.open(node, access, share).map_err(fs_status)?;
                Ok((node, FILE_CREATED))
            }
        }
    })?;
    let directory = with_fs(|fs| fs.node(node).is_some_and(ramfs::Node::is_dir));
    let file = FileObject {
        node,
        directory,
        granted,
        access,
        share,
        synchronous: options & (FILE_SYNCHRONOUS_IO_ALERT | FILE_SYNCHRONOUS_IO_NONALERT) != 0,
        position: IrqMutex::new(0),
    };
    let delete_on_close = options & FILE_DELETE_ON_CLOSE != 0;
    let object = match KObject::new(ObjectBody::File(file)) {
        Ok(o) => o,
        // `file` was dropped inside, which closed the open.
        Err(_) => return Err(NtStatus::NO_MEMORY),
    };
    if delete_on_close {
        with_fs(|fs| fs.set_delete_pending(node, true)).map_err(fs_status)?;
    }
    Ok(Created { object, information })
}

/// FILE_BASIC_INFORMATION (40 bytes) for `node`.
pub fn basic_information(node: NodeId) -> Result<[u8; 40], NtStatus> {
    with_fs(|fs| {
        let n = fs.node(node).ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
        let mut b = [0u8; 40];
        b[0..8].copy_from_slice(&n.created.to_le_bytes()); // CreationTime
        b[8..16].copy_from_slice(&n.written.to_le_bytes()); // LastAccessTime
        b[16..24].copy_from_slice(&n.written.to_le_bytes()); // LastWriteTime
        b[24..32].copy_from_slice(&n.written.to_le_bytes()); // ChangeTime
        b[32..36].copy_from_slice(&n.attributes.to_le_bytes());
        Ok(b)
    })
}

/// FILE_STANDARD_INFORMATION (24 bytes) for `node`.
pub fn standard_information(node: NodeId) -> Result<[u8; 24], NtStatus> {
    with_fs(|fs| {
        let n = fs.node(node).ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
        let size = n.size();
        let mut b = [0u8; 24];
        b[0..8].copy_from_slice(&size.next_multiple_of(4096).to_le_bytes()); // AllocationSize
        b[8..16].copy_from_slice(&size.to_le_bytes()); // EndOfFile
        b[16..20].copy_from_slice(&1u32.to_le_bytes()); // NumberOfLinks
        b[20] = u8::from(fs.delete_pending(node));
        b[21] = u8::from(n.is_dir());
        Ok(b)
    })
}
