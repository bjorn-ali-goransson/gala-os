//! Files: the volume device `\\Device\\HarddiskVolume1` (also `\\??\\C:`) backed
//! by the in-memory file system, file objects and their I/O semantics
//! (dispositions, positions, share access, delete on close), following the
//! documented behaviour of NtCreateFile / NtReadFile / NtWriteFile.

use super::platform;
use alloc::vec::Vec;

use super::namespace;
use super::object::{KObject, ObjectBody, ObjectRef};
use super::status::NtStatus;
use crate::fs::ramfs::{self, Access, FsError, NodeId, RamFs, Share};
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;

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
    let now = platform::system_time();
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
    // C: is a symbolic link to the volume, as on Windows.
    let c_drive = namespace::new_symbolic_link(&wide(VOLUME_DEVICE)?)?;
    namespace::create(None, &wide("\\??\\C:")?, c_drive, false, false)?;
    drop(volume);
    Ok(())
}

/// The device path of the volume.
pub const VOLUME_DEVICE: &str = "\\Device\\HarddiskVolume1";

/// Adds a boot module as a read-only file `C:\\<directory>\\<name>`; the
/// directory (`/`-separated, created if needed) defaults to `gala`.
pub fn add_boot_file(directory: &str, name: &str, data: &'static [u8]) -> Result<(), NtStatus> {
    let directory = if directory.is_empty() { "gala" } else { directory };
    let mut parts: Vec<Vec<u16>> = Vec::new();
    for p in directory.split(['/', '\\']).filter(|p| !p.is_empty()) {
        parts.try_push(wide(p)?).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    let mut refs: Vec<&[u16]> = Vec::new();
    for p in &parts {
        refs.try_push(p).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    let name = wide(name)?;
    with_fs(|fs| {
        let now = platform::system_time();
        let dir = fs.mkdir_all(&refs, now).map_err(fs_status)?;
        fs.add_static_file(dir, &name, data, now).map_err(fs_status)?;
        Ok(())
    })
}

/// The NT path (`\\Device\\HarddiskVolume1\\...`) of `node`.
pub fn nt_path(node: NodeId) -> Result<Vec<u16>, NtStatus> {
    let path = with_fs(|fs| fs.path_of(node)).map_err(fs_status)?;
    let mut out = wide(VOLUME_DEVICE)?;
    out.try_extend_from_slice(&path).map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(out)
}

/// The node at `path` (`\\`-separated, from the volume root).
pub fn lookup(path: &str) -> Result<NodeId, NtStatus> {
    let wide_path = wide(path)?;
    let parts = components(&wide_path, false)?;
    with_fs(|fs| fs.resolve(ramfs::ROOT, &parts)).map_err(fs_status)
}

/// The whole contents of file `node`.
pub fn read_all(node: NodeId) -> Result<Vec<u8>, NtStatus> {
    let size = with_fs(|fs| fs.node(node).map_or(0, ramfs::Node::size));
    let len = usize::try_from(size).map_err(|_| NtStatus::NO_MEMORY)?;
    let mut data = Vec::new();
    data.try_reserve_exact(len).map_err(|_| NtStatus::NO_MEMORY)?;
    data.resize(len, 0);
    let n = with_fs(|fs| fs.read(node, 0, &mut data)).map_err(fs_status)?;
    data.truncate(n);
    Ok(data)
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
        FsError::AccessDenied => NtStatus::ACCESS_DENIED,
        FsError::UserMapped => NtStatus::USER_MAPPED_FILE,
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
    /// FILE_DELETE_ON_CLOSE: delete pending once this open closes.
    delete_on_close: bool,
    /// Directory enumeration state (NtQueryDirectoryFile).
    pub scan: IrqMutex<Option<DirScan>>,
    /// FileModeInformation: the mode options it was opened with.
    pub mode: u32,
    /// Signaled when an I/O on it completes (waiting on a file handle).
    pub dispatcher: super::wait::Dispatcher,
    /// FileCompletionInformation: the completion port and key that its
    /// completed I/O is posted to.
    pub completion: IrqMutex<Option<(ObjectRef, u64)>>,
}

/// CreateOptions reported by FileModeInformation.
const MODE_OPTIONS: u32 = 0x0002 // FILE_WRITE_THROUGH
    | 0x0004 // FILE_SEQUENTIAL_ONLY
    | 0x0008 // FILE_NO_INTERMEDIATE_BUFFERING
    | FILE_SYNCHRONOUS_IO_ALERT
    | FILE_SYNCHRONOUS_IO_NONALERT
    | FILE_DELETE_ON_CLOSE;

/// FileModeInformation for a file opened with `options`.
pub fn mode_of(options: u32) -> u32 {
    options & MODE_OPTIONS
}

impl Drop for FileObject {
    fn drop(&mut self) {
        let (node, access, share) = (self.node, self.access, self.share);
        if let Some(fs) = FS.lock().as_mut() {
            if self.delete_on_close {
                // Fails for a non-empty directory, which then stays.
                let _ = fs.set_delete_pending(node, true);
            }
            fs.close(node, access, share);
        }
    }
}

/// A file name relative to directory `start` on the volume.
pub struct FilePath {
    pub start: NodeId,
    pub name: Vec<u16>,
}

/// Splits a `\\a\\b\\c` path (relative to the volume root) or an `a\\b`
/// path (relative to a directory) into components.
pub fn components(rest: &[u16], relative: bool) -> Result<Vec<&[u16]>, NtStatus> {
    let mut out = Vec::new();
    let body = match rest.strip_prefix(&[u16::from(b'\\')]) {
        Some(_) if relative => return Err(NtStatus::OBJECT_NAME_INVALID),
        Some(body) => body,
        None if relative || rest.is_empty() => rest,
        None => return Err(NtStatus::OBJECT_NAME_INVALID),
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

/// Opens or creates `path` (NtCreateFile semantics).
pub fn create(
    path: &FilePath,
    granted: u32,
    share_mask: u32,
    disposition: u32,
    options: u32,
) -> Result<Created, NtStatus> {
    if disposition > FILE_OVERWRITE_IF {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    let delete_on_close = options & FILE_DELETE_ON_CLOSE != 0;
    if delete_on_close && granted & DELETE == 0 {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    let want_dir = options & FILE_DIRECTORY_FILE != 0;
    let want_file = options & FILE_NON_DIRECTORY_FILE != 0;
    if want_dir && want_file {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    let start = path.start;
    let parts = components(&path.name, start != ramfs::ROOT)?;
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
    let now = platform::system_time();
    let (node, information) = with_fs(|fs| -> Result<(NodeId, u64), NtStatus> {
        let existing = match parts.last() {
            None => Some(start),
            Some(last) => {
                let parent = fs.parent_of(start, &parts).map_err(fs_status)?;
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
                let parent = fs.parent_of(start, &parts).map_err(fs_status)?;
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
        delete_on_close,
        scan: IrqMutex::new(None),
        mode: mode_of(options),
        dispatcher: super::wait::Dispatcher::new(0),
        completion: IrqMutex::new(None),
    };
    let object = match KObject::new(ObjectBody::File(file)) {
        Ok(o) => o,
        // `file` was dropped inside, which closed the open.
        Err(_) => return Err(NtStatus::NO_MEMORY),
    };
    Ok(Created { object, information })
}

/// FILE_BASIC_INFORMATION (40 bytes) for `node`.
/// FileInternalInformation's IndexNumber: the node, unique on the volume.
pub fn index_number(node: NodeId) -> u64 {
    node as u64
}

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

/// Where a directory enumeration stands.
pub struct DirScan {
    /// The search mask, fixed by the first query on the handle.
    pub mask: Vec<u16>,
    /// "." and ".." returned so far (0..=2).
    dots: u8,
    /// The last returned name; enumeration resumes after it.
    after: Option<Vec<u16>>,
}

impl DirScan {
    pub fn new(mask: Vec<u16>) -> Self {
        DirScan { mask, dots: 0, after: None }
    }

    pub fn at_start(&self) -> bool {
        self.dots == 0 && self.after.is_none()
    }

    pub fn restart(&mut self) {
        self.dots = 0;
        self.after = None;
    }
}

pub const FILE_DIRECTORY_INFORMATION: u32 = 1;
pub const FILE_FULL_DIRECTORY_INFORMATION: u32 = 2;
pub const FILE_BOTH_DIRECTORY_INFORMATION: u32 = 3;
pub const FILE_NAMES_INFORMATION: u32 = 12;
pub const FILE_ID_BOTH_DIRECTORY_INFORMATION: u32 = 37;
pub const FILE_ID_FULL_DIRECTORY_INFORMATION: u32 = 38;

/// (fixed size, FileName offset) of a directory information class.
pub fn directory_class_layout(class: u32) -> Option<(usize, usize)> {
    match class {
        FILE_DIRECTORY_INFORMATION => Some((64, 64)),
        FILE_FULL_DIRECTORY_INFORMATION => Some((68, 68)),
        FILE_BOTH_DIRECTORY_INFORMATION => Some((94, 94)),
        FILE_NAMES_INFORMATION => Some((12, 12)),
        FILE_ID_BOTH_DIRECTORY_INFORMATION => Some((104, 104)),
        FILE_ID_FULL_DIRECTORY_INFORMATION => Some((80, 80)),
        _ => None,
    }
}

/// The result of one NtQueryDirectoryFile call.
pub struct Listing {
    pub status: NtStatus,
    /// The filled buffer (IoStatus.Information = its length).
    pub data: Vec<u8>,
}

/// One directory entry to report.
struct Entry<'a> {
    node: &'a ramfs::Node,
    name: &'a [u16],
}

fn put(b: &mut [u8], off: usize, v: &[u8]) {
    b[off..off + v.len()].copy_from_slice(v);
}

/// Encodes `e` in `class` at the start of `out` (which holds at least the
/// fixed part); returns false if the name was truncated.
fn encode_entry(out: &mut [u8], class: u32, e: &Entry<'_>, name_at: usize) -> bool {
    let n = e.node;
    let name_bytes = e.name.len() * 2;
    if class == FILE_NAMES_INFORMATION {
        put(out, 8, &(name_bytes as u32).to_le_bytes());
    } else {
        put(out, 8, &n.created.to_le_bytes());
        put(out, 16, &n.written.to_le_bytes());
        put(out, 24, &n.written.to_le_bytes());
        put(out, 32, &n.written.to_le_bytes());
        put(out, 40, &n.size().to_le_bytes());
        put(out, 48, &n.size().next_multiple_of(4096).to_le_bytes());
        put(out, 56, &n.attributes.to_le_bytes());
        put(out, 60, &(name_bytes as u32).to_le_bytes());
    }
    let room = (out.len() - name_at) / 2;
    for (i, c) in e.name.iter().take(room).enumerate() {
        put(out, name_at + 2 * i, &c.to_le_bytes());
    }
    room >= e.name.len()
}

const DOT: [u16; 1] = [b'.' as u16];
const DOT_DOT: [u16; 2] = [b'.' as u16, b'.' as u16];

/// Fills up to `length` bytes of `class` entries of directory `dir` from
/// the position in `scan` (NtQueryDirectoryFile semantics).
pub fn list_directory(
    dir: NodeId,
    scan: &mut DirScan,
    class: u32,
    length: usize,
    single: bool,
) -> Result<Listing, NtStatus> {
    let (fixed, name_at) = directory_class_layout(class).ok_or(NtStatus::INVALID_INFO_CLASS)?;
    if length < fixed {
        return Err(NtStatus::INFO_LENGTH_MISMATCH);
    }
    let first = scan.at_start();
    with_fs(|fs| {
        let this = fs.node(dir).ok_or(NtStatus::FILE_DELETED)?;
        let ids = fs.list(dir).map_err(fs_status)?;
        let mut data: Vec<u8> = Vec::new();
        let mut last_start: Option<usize> = None;
        let mut status = NtStatus::SUCCESS;
        // Candidates in order: ".", ".." (not in the root), then children.
        let dots: &[(&[u16], NodeId)] =
            if dir == ramfs::ROOT { &[] } else { &[(&DOT, dir), (&DOT_DOT, this.parent())] };
        let candidates = dots
            .iter()
            .enumerate()
            .skip(usize::from(scan.dots))
            .map(|(i, &(name, id))| (Some(i), name, id))
            .chain(ids.iter().filter_map(|&id| fs.node(id).map(|n| (None, n.name(), id))).filter(
                |(_, name, _)| scan.after.as_ref().is_none_or(|a| ramfs::collate(name, a).is_gt()),
            ));
        let mut consumed_dots = scan.dots;
        let mut consumed_after: Option<&[u16]> = None;
        for (dot, name, id) in candidates {
            if !ramfs::wildcard_match(&scan.mask, name) {
                if let Some(i) = dot {
                    consumed_dots = i as u8 + 1;
                } else {
                    consumed_after = Some(name);
                }
                continue;
            }
            let Some(node) = fs.node(id) else { continue };
            let start = data.len().next_multiple_of(8);
            let need = fixed.max(name_at + name.len() * 2);
            if start + need > length {
                if last_start.is_none() {
                    // Not even the first entry fits: return what does.
                    data.try_reserve_exact(length).map_err(|_| NtStatus::NO_MEMORY)?;
                    data.resize(length, 0);
                    encode_entry(&mut data, class, &Entry { node, name }, name_at);
                    status = NtStatus::BUFFER_OVERFLOW;
                }
                break;
            }
            data.try_reserve(start + need - data.len()).map_err(|_| NtStatus::NO_MEMORY)?;
            data.resize(start + need, 0);
            encode_entry(&mut data[start..], class, &Entry { node, name }, name_at);
            if let Some(prev) = last_start {
                put(&mut data, prev, &((start - prev) as u32).to_le_bytes());
            }
            last_start = Some(start);
            if let Some(i) = dot {
                consumed_dots = i as u8 + 1;
            } else {
                consumed_after = Some(name);
            }
            if single {
                break;
            }
        }
        if dots.is_empty() || consumed_after.is_some() {
            consumed_dots = 2;
        }
        let after = match consumed_after {
            Some(name) => {
                let mut v = Vec::new();
                v.try_extend_from_slice(name).map_err(|_| NtStatus::NO_MEMORY)?;
                Some(v)
            }
            None => None,
        };
        scan.dots = consumed_dots;
        if after.is_some() {
            scan.after = after;
        }
        if last_start.is_none() && status == NtStatus::SUCCESS {
            status = if first { NtStatus::NO_SUCH_FILE } else { NtStatus::NO_MORE_FILES };
        }
        Ok(Listing { status, data })
    })
}

pub const FILE_FS_VOLUME_INFORMATION: u32 = 1;
pub const FILE_FS_SIZE_INFORMATION: u32 = 3;
pub const FILE_FS_DEVICE_INFORMATION: u32 = 4;
pub const FILE_FS_ATTRIBUTE_INFORMATION: u32 = 5;
pub const FILE_FS_FULL_SIZE_INFORMATION: u32 = 7;

const FILE_DEVICE_DISK: u32 = 7;
const FILE_DEVICE_IS_MOUNTED: u32 = 0x20;
const FILE_CASE_PRESERVED_NAMES: u32 = 0x2;
const FILE_UNICODE_ON_DISK: u32 = 0x4;
const VOLUME_SERIAL: u32 = 0x6a1a_0001;
const SECTORS_PER_CLUSTER: u32 = 8;
const BYTES_PER_SECTOR: u32 = 512;
/// The file system name reported to programs (they check for it).
const FS_NAME: &[u8] = b"NTFS";

/// Volume information of `class` as (bytes, minimum buffer length). Sizes
/// are in clusters of free and total RAM (the volume lives in memory).
pub fn volume_information(class: u32) -> Result<(Vec<u8>, usize), NtStatus> {
    let (free, total) = platform::frame_stats();
    let mut b = Vec::new();
    let grow = |b: &mut Vec<u8>, n: usize| -> Result<(), NtStatus> {
        b.try_reserve_exact(n).map_err(|_| NtStatus::NO_MEMORY)?;
        b.resize(n, 0);
        Ok(())
    };
    let min = match class {
        FILE_FS_VOLUME_INFORMATION => {
            // VolumeCreationTime, VolumeSerialNumber, VolumeLabelLength (0),
            // SupportsObjects, then the (empty) label.
            grow(&mut b, 18)?;
            let created = with_fs(|fs| fs.node(ramfs::ROOT).map_or(0, |n| n.created));
            put(&mut b, 0, &created.to_le_bytes());
            put(&mut b, 8, &VOLUME_SERIAL.to_le_bytes());
            24
        }
        FILE_FS_SIZE_INFORMATION => {
            grow(&mut b, 24)?;
            put(&mut b, 0, &total.to_le_bytes());
            put(&mut b, 8, &free.to_le_bytes());
            put(&mut b, 16, &SECTORS_PER_CLUSTER.to_le_bytes());
            put(&mut b, 20, &BYTES_PER_SECTOR.to_le_bytes());
            24
        }
        FILE_FS_FULL_SIZE_INFORMATION => {
            grow(&mut b, 32)?;
            put(&mut b, 0, &total.to_le_bytes());
            put(&mut b, 8, &free.to_le_bytes());
            put(&mut b, 16, &free.to_le_bytes());
            put(&mut b, 24, &SECTORS_PER_CLUSTER.to_le_bytes());
            put(&mut b, 28, &BYTES_PER_SECTOR.to_le_bytes());
            32
        }
        FILE_FS_DEVICE_INFORMATION => {
            grow(&mut b, 8)?;
            put(&mut b, 0, &FILE_DEVICE_DISK.to_le_bytes());
            put(&mut b, 4, &FILE_DEVICE_IS_MOUNTED.to_le_bytes());
            8
        }
        FILE_FS_ATTRIBUTE_INFORMATION => {
            grow(&mut b, 12 + 2 * FS_NAME.len())?;
            let attributes = FILE_CASE_PRESERVED_NAMES | FILE_UNICODE_ON_DISK;
            put(&mut b, 0, &attributes.to_le_bytes());
            put(&mut b, 4, &255u32.to_le_bytes()); // MaximumComponentNameLength
            put(&mut b, 8, &((2 * FS_NAME.len()) as u32).to_le_bytes());
            for (i, &c) in FS_NAME.iter().enumerate() {
                put(&mut b, 12 + 2 * i, &u16::from(c).to_le_bytes());
            }
            16
        }
        _ => return Err(NtStatus::INVALID_INFO_CLASS),
    };
    Ok((b, min))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_information_keeps_mode_options_only() {
        assert_eq!(
            mode_of(FILE_SYNCHRONOUS_IO_NONALERT | FILE_NON_DIRECTORY_FILE),
            FILE_SYNCHRONOUS_IO_NONALERT
        );
        assert_eq!(mode_of(FILE_NON_DIRECTORY_FILE | FILE_DIRECTORY_FILE), 0);
        assert_eq!(mode_of(FILE_DELETE_ON_CLOSE | 0x0002), FILE_DELETE_ON_CLOSE | 0x0002);
    }
}
