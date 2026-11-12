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

/// Runs `f` on the file system (initialized by `init`); then delivers the
/// directory change notifications its changes caused (nt::notify), with
/// the file system released.
pub fn with_fs<R>(f: impl FnOnce(&mut RamFs) -> R) -> R {
    let (r, events) = {
        let mut guard = FS.lock();
        let fs = guard.as_mut().expect("file system initialized");
        let r = f(fs);
        if fs.recording && fs.has_ops() {
            super::persist::note_pending();
        }
        (r, super::notify::collect(fs))
    };
    super::notify::deliver(events);
    r
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
pub const FILE_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

/// The directories every boot starts with: the system's, the user's
/// profile (`gala`, as advapi32's GetUserName names the user) with its
/// AppData folders, the shared ones.
const DIRECTORIES: &[&[&str]] = &[
    &["Windows", "System32"],
    // The 32-bit system DLLs' directory (WOW64; GetSystemWow64DirectoryW).
    &["Windows", "SysWOW64"],
    &["Users", "Public"],
    &["Users", "gala", "AppData", "Roaming"],
    &["Users", "gala", "AppData", "Local", "Temp"],
    // SYSTEM's profile and the system's temporary directory, a
    // LocalSystem service's (nt::process::SERVICE_ENVIRONMENT).
    &["Windows", "System32", "config", "systemprofile", "AppData", "Roaming"],
    &["Windows", "System32", "config", "systemprofile", "AppData", "Local"],
    &["Windows", "Temp"],
    &["ProgramData"],
    &["Program Files", "Common Files"],
    // The 32-bit programs' (WOW64; ProgramFiles in a 32-bit process).
    &["Program Files (x86)", "Common Files"],
    &["gala"],
];

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
    // Opening the volume itself: its attributes for everyone, the rest
    // for administrators (as Windows' volumes).
    const FILE_READ_ATTRIBUTES_SYNCHRONIZE_READ_CONTROL: u32 = 0x0012_0080;
    *volume.security.lock() =
        Some(super::disk::device_descriptor(FILE_READ_ATTRIBUTES_SYNCHRONIZE_READ_CONTROL)?);
    namespace::create(None, &wide("\\Device\\HarddiskVolume1")?, volume.clone(), false, false)?;
    // C: is a symbolic link to the volume, as on Windows.
    let c_drive = namespace::new_symbolic_link(&wide(VOLUME_DEVICE)?)?;
    namespace::create(None, &wide("\\??\\C:")?, c_drive, false, false)?;
    // And its volume GUID name, as the mount manager makes it (kernel32's
    // GetVolumeNameForVolumeMountPoint returns the same GUID).
    let guid = namespace::new_symbolic_link(&wide(VOLUME_DEVICE)?)?;
    namespace::create(None, &wide(VOLUME_GUID_LINK)?, guid, false, false)?;
    drop(volume);
    // The NUL device, and the DOS name for it.
    let null =
        KObject::with_permanence(ObjectBody::NullDevice, true).map_err(|_| NtStatus::NO_MEMORY)?;
    namespace::create(None, &wide("\\Device\\Null")?, null, false, false)?;
    let link = namespace::new_symbolic_link(&wide("\\Device\\Null")?)?;
    namespace::create(None, &wide("\\??\\NUL")?, link, false, false)?;
    Ok(())
}

/// The device path of the volume.
pub const VOLUME_DEVICE: &str = "\\Device\\HarddiskVolume1";
/// Its volume GUID name (user/kernel32/src/volume.rs has the same GUID).
pub const VOLUME_GUID_LINK: &str = "\\??\\Volume{9a1a0c00-0000-4000-8000-00000000c0de}";

/// When the boot had made C: (system time; 0: not yet): what is older is
/// made again by the next kernel's boot (`survives_handover`).
static BOOT_DONE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The boot has made C: (called before programs start).
pub fn mark_boot_done() {
    BOOT_DONE.store(platform::system_time(), core::sync::atomic::Ordering::Relaxed);
}

/// Whether file or directory `node` will be in the next kernel's C: (a
/// kernel handover reopens it there): kept on the disk (`persist`), or
/// made by the boot and, for a file, not written since.
pub fn survives_handover(node: NodeId) -> bool {
    let mark = BOOT_DONE.load(core::sync::atomic::Ordering::Relaxed);
    with_fs(|fs| {
        let recording = fs.recording;
        fs.node(node)
            .is_some_and(|n| recording || (n.created <= mark && (n.is_dir() || n.written <= mark)))
    })
}

/// Opens file or directory `path` (from the volume root) again as a
/// kernel handover's record has it: same rights, share access and mode,
/// at `position`.
pub fn reopen(
    path: &[u16],
    granted: u32,
    share: u32,
    options: u32,
    position: u64,
) -> Result<ObjectRef, NtStatus> {
    let mut name = Vec::new();
    name.try_extend_from_slice(path).map_err(|_| NtStatus::NO_MEMORY)?;
    let path = FilePath { start: ramfs::ROOT, name };
    let created = create(&path, granted, share, FILE_OPEN, options)?;
    if let ObjectBody::File(f) = &created.object.body {
        *f.position.lock() = position;
    }
    Ok(created.object)
}

/// Adds a boot module as a read-only file `C:\\<directory>\\<name>`; the
/// directory (`/`-separated, created if needed) defaults to `gala`.
pub fn add_boot_file(
    directory: &str,
    name: &str,
    data: &'static [u8],
    writable: bool,
) -> Result<(), NtStatus> {
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
        fs.add_static_file(dir, &name, data, writable, now).map_err(fs_status)?;
        Ok(())
    })
}

/// Makes the directory `directory` (`a/b`, on C:) and its parents, for a
/// boot archive's (possibly empty) directories.
pub fn add_boot_directory(directory: &str) -> Result<(), NtStatus> {
    let mut parts: Vec<Vec<u16>> = Vec::new();
    for p in directory.split(['/', '\\']).filter(|p| !p.is_empty()) {
        parts.try_push(wide(p)?).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    let mut refs: Vec<&[u16]> = Vec::new();
    for p in &parts {
        refs.try_push(p).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    with_fs(|fs| fs.mkdir_all(&refs, platform::system_time()).map(|_| ()).map_err(fs_status))
}

/// The NT path (`\\Device\\HarddiskVolume1\\...`) of `node`.
pub fn nt_path(node: NodeId) -> Result<Vec<u16>, NtStatus> {
    let path = with_fs(|fs| fs.path_of(node)).map_err(fs_status)?;
    let mut out = wide(VOLUME_DEVICE)?;
    out.try_extend_from_slice(&path).map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(out)
}

/// Whether two NT paths name the same file (the volume's names are case
/// insensitive; ASCII case folding, as the volume's own lookups).
pub fn same_path(a: &[u16], b: &[u16]) -> bool {
    let fold = |c: u16| if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c };
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| fold(x) == fold(y))
}

/// FileProcessIdsUsingFileInformation: the ids of the running processes
/// that use file `node`, in process order: a handle open on it, a view of
/// a section on it (a mapped data file or a loaded DLL) or it as their
/// executable. `query` (process id, handle) is the handle the question is
/// asked through, which does not count by itself.
pub fn processes_using(node: NodeId, query: (u64, u64)) -> Result<Vec<u64>, NtStatus> {
    let name = nt_path(node)?;
    let mut out = Vec::new();
    for p in super::process::running().map_err(|_| NtStatus::NO_MEMORY)? {
        let by_handle = p.handles.lock().iter().any(|(h, e)| {
            matches!(&e.object.body, ObjectBody::File(f) if f.node == node) && (p.id, h.0) != query
        });
        let uses = by_handle
            || p.views
                .lock()
                .iter()
                .any(|v| v.section().file_name.as_deref().is_some_and(|n| same_path(n, &name)))
            || p.image_nt_path().is_ok_and(|n| same_path(&n, &name));
        if uses {
            out.try_push(p.id).map_err(|_| NtStatus::NO_MEMORY)?;
        }
    }
    Ok(out)
}

/// The path of `node` from the volume root (`\\dir\\name`), as
/// FileNameInformation reports it.
pub fn volume_path(node: NodeId) -> Result<Vec<u16>, NtStatus> {
    with_fs(|fs| fs.path_of(node)).map_err(fs_status)
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
    let n = read(node, 0, &mut data).map_err(fs_status)?;
    data.truncate(n);
    Ok(data)
}

/// Runs `f` on the file system like `with_fs`, first reading from the
/// disk each page it finds on the disk only (a disk-backed C:, D4:
/// `FsError::NotResident`), then running it again. The read happens with
/// the file system released; a page that changed meanwhile is read again.
pub fn with_pages<R>(mut f: impl FnMut(&mut RamFs) -> Result<R, FsError>) -> Result<R, FsError> {
    loop {
        let (r, wanted) = with_fs(|fs| {
            let r = f(fs);
            let wanted =
                if matches!(r, Err(FsError::NotResident)) { fs.take_page_in() } else { None };
            (r, wanted)
        });
        match (r, wanted) {
            (Err(FsError::NotResident), Some(req)) => page_in(&req)?,
            (r, _) => return r,
        }
    }
}

/// Reads page `req` from its cluster into memory (see `with_pages`).
fn page_in(req: &ramfs::PageIn) -> Result<(), FsError> {
    super::persist::relieve();
    let mut page =
        crate::mm::heap::try_box(ramfs::Page([0; ramfs::PAGE])).map_err(|_| FsError::NoMemory)?;
    if super::persist::read_cluster(req.cluster, &mut page.0).is_err() {
        warn!(
            "file: cannot read cluster {} (page {} of node {})",
            req.cluster, req.index, req.node
        );
        return Err(FsError::NotResident);
    }
    super::persist::note_page_in();
    with_fs(|fs| fs.fill(req, page));
    Ok(())
}

/// Reads file `node` from `offset` into `buf`: all of it, short only at
/// the end of the file (pages on the disk only are read in).
pub fn read(node: NodeId, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
    let mut done = 0;
    while done < buf.len() {
        let at = offset + done as u64;
        let n = with_pages(|fs| fs.read(node, at, &mut buf[done..]))?;
        if n == 0 {
            break;
        }
        done += n;
    }
    Ok(done)
}

/// Writes `data` to file `node` at `offset` (pages it changes in part are
/// read in first).
pub fn write(node: NodeId, offset: u64, data: &[u8], now: u64) -> Result<usize, FsError> {
    with_pages(|fs| fs.write(node, offset, data, now))
}

/// Sets file `node`'s size (a partial last page is read in first).
pub fn set_size(node: NodeId, size: u64, now: u64) -> Result<(), FsError> {
    with_pages(|fs| fs.set_size(node, size, now))
}

/// The address of mapped file `node`'s page `index` (read in first if it
/// is on the disk only).
pub fn page_address(node: NodeId, index: usize) -> Option<usize> {
    with_pages(|fs| fs.page_address(node, index)).ok()
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
        // A Linux symbolic link or socket: a reparse point NT does not
        // follow.
        FsError::Symlink | FsError::Socket => NtStatus::IO_REPARSE_TAG_NOT_HANDLED,
        FsError::Sealed => NtStatus::ACCESS_DENIED,
        FsError::WriteMapped => NtStatus::USER_MAPPED_FILE,
        FsError::NotSealable => NtStatus::INVALID_PARAMETER,
        // NT never reaches a volume's tree (ramfs's doc).
        FsError::CrossVolume => NtStatus::NOT_SAME_DEVICE,
        FsError::DiskFull => NtStatus::DISK_FULL,
        // A page that could not be read from the disk (`with_pages`).
        FsError::NotResident => NtStatus::IO_DEVICE_ERROR,
        // FSCTL_SET_REPARSE_POINT, FSCTL_DELETE_REPARSE_POINT (NTFS's
        // answers).
        FsError::ReparseHasData => NtStatus::IO_REPARSE_DATA_INVALID,
        FsError::ReparseTagMismatch => NtStatus::IO_REPARSE_TAG_MISMATCH,
        FsError::NotReparsePoint => NtStatus::NOT_A_REPARSE_POINT,
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
    /// Signaled when an I/O on it completes (waiting on a file handle);
    /// shared with requests that complete later (nt::notify).
    pub dispatcher: crate::mm::heap::KArc<super::wait::Dispatcher>,
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

impl FileObject {
    /// The share access it was opened with (FILE_SHARE_*).
    pub fn share_mask(&self) -> u32 {
        (u32::from(self.share.read) * FILE_SHARE_READ)
            | (u32::from(self.share.write) * FILE_SHARE_WRITE)
            | (u32::from(self.share.delete) * FILE_SHARE_DELETE)
    }

    /// FILE_DELETE_ON_CLOSE was given.
    pub fn deletes_on_close(&self) -> bool {
        self.delete_on_close
    }

    /// The owner of its byte-range locks (`locks`): its address.
    pub fn lock_owner(&self) -> usize {
        self as *const Self as usize
    }
}

impl Drop for FileObject {
    fn drop(&mut self) {
        super::locks::release_owner(self.lock_owner());
        // Its directory watch ends (waiting requests: NOTIFY_CLEANUP).
        if self.directory && super::notify::watched(self.lock_owner()) {
            super::notify::cleanup(self.lock_owner());
        }
        let (node, access, share) = (self.node, self.access, self.share);
        let events = FS.lock().as_mut().map(|fs| {
            if self.delete_on_close {
                // Fails for a non-empty directory, which then stays.
                let _ = fs.set_delete_pending(node, true);
            }
            fs.close(node, access, share);
            super::notify::collect(fs)
        });
        if let Some(events) = events {
            super::notify::deliver(events);
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

/// The node `path` names, if it exists, and its parent directory (None
/// for both when a directory on the way is missing: opening reports it).
pub fn lookup_path(path: &FilePath) -> Result<(Option<NodeId>, Option<NodeId>), NtStatus> {
    let start = path.start;
    let parts = components(&path.name, start != ramfs::ROOT)?;
    with_fs(|fs| {
        let Some(last) = parts.last() else {
            let parent = fs.node(start).map(ramfs::Node::parent).filter(|&p| p != start);
            return Ok((Some(start), parent));
        };
        let Ok(parent) = fs.parent_of(start, &parts) else { return Ok((None, None)) };
        Ok((fs.find(parent, last).ok().flatten(), Some(parent)))
    })
}

/// How many reparse points one open may follow (as Windows: 63); more is
/// STATUS_REPARSE_POINT_NOT_RESOLVED, a loop included.
const MAX_REPARSE: usize = 63;

/// SYMLINK_FLAG_RELATIVE: a symbolic link's target is relative to its
/// directory.
const SYMLINK_FLAG_RELATIVE: u32 = 1;

/// Follows the NT symbolic links and junctions on `path` (and at its end
/// if `follow_final`), as the I/O manager reparses an open that meets one
/// (ARCHITECTURE "Files": reparse points): the path the open then names.
/// A reparse point of another tag on the way, or at the end when followed,
/// is STATUS_IO_REPARSE_TAG_NOT_HANDLED (no filter handles it here).
pub fn resolve_links(path: FilePath, follow_final: bool) -> Result<FilePath, NtStatus> {
    let mut path = path;
    for _ in 0..MAX_REPARSE {
        match follow_once(&path, follow_final)? {
            Some(next) => path = next,
            None => return Ok(path),
        }
    }
    Err(NtStatus::REPARSE_POINT_NOT_RESOLVED)
}

/// The path after the first link on `path`, or None if it meets none
/// (or a component is missing: the open reports that).
fn follow_once(path: &FilePath, follow_final: bool) -> Result<Option<FilePath>, NtStatus> {
    let parts = components(&path.name, path.start != ramfs::ROOT)?;
    let found = with_fs(|fs| -> Result<Option<(Vec<u16>, ramfs::Reparse, usize)>, NtStatus> {
        let mut dir = path.start;
        for (i, part) in parts.iter().enumerate() {
            let last = i + 1 == parts.len();
            let Ok(Some(node)) = fs.find(dir, part) else { return Ok(None) };
            let n = fs.node(node).ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
            if let Some(r) = n.reparse() {
                if last && !follow_final {
                    return Ok(None);
                }
                if r.tag != ramfs::TAG_SYMLINK && r.tag != ramfs::TAG_MOUNT_POINT {
                    return Err(NtStatus::IO_REPARSE_TAG_NOT_HANDLED);
                }
                let base = fs.path_of(n.parent()).map_err(fs_status)?;
                let copy = ramfs::Reparse {
                    tag: r.tag,
                    data: crate::mm::heap::try_to_vec(&r.data).map_err(|_| NtStatus::NO_MEMORY)?,
                };
                return Ok(Some((base, copy, i + 1)));
            }
            if !n.is_dir() {
                return Ok(None);
            }
            dir = node;
        }
        Ok(None)
    })?;
    let Some((base, reparse, used)) = found else { return Ok(None) };
    let (target, relative) = link_target(&reparse)?;
    // What is left of the path goes after the target.
    let mut rest: Vec<u16> = Vec::new();
    for part in &parts[used..] {
        rest.try_push(u16::from(b'\\')).map_err(|_| NtStatus::NO_MEMORY)?;
        rest.try_extend_from_slice(part).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    if relative {
        // From the link's directory, `.` and `..` taken out.
        let mut name = base;
        for part in target.split(|&c| c == u16::from(b'\\')) {
            match part {
                [] => {}
                [0x2e] => {}
                [0x2e, 0x2e] => {
                    let cut = name.iter().rposition(|&c| c == u16::from(b'\\')).unwrap_or(0);
                    name.truncate(cut);
                }
                _ => {
                    name.try_push(u16::from(b'\\')).map_err(|_| NtStatus::NO_MEMORY)?;
                    name.try_extend_from_slice(part).map_err(|_| NtStatus::NO_MEMORY)?;
                }
            }
        }
        name.try_extend_from_slice(&rest).map_err(|_| NtStatus::NO_MEMORY)?;
        return Ok(Some(FilePath { start: ramfs::ROOT, name }));
    }
    // An NT path (`\??\C:\...`): through the object namespace to the volume.
    let mut full = target;
    while full.len() > 1 && full.last() == Some(&u16::from(b'\\')) {
        full.pop();
    }
    full.try_extend_from_slice(&rest).map_err(|_| NtStatus::NO_MEMORY)?;
    if full.first() != Some(&u16::from(b'\\')) {
        return Err(NtStatus::OBJECT_NAME_INVALID);
    }
    let (device, name) = namespace::lookup_parse(None, &full, true)?;
    if !matches!(device.body, ObjectBody::Volume) {
        // A link to a device other than the volume (a pipe, ...).
        super::ledger::record("reparse point", "target not on the volume", 0);
        return Err(NtStatus::OBJECT_PATH_NOT_FOUND);
    }
    Ok(Some(FilePath { start: ramfs::ROOT, name }))
}

/// A symbolic link's or junction's substitute name and whether it is
/// relative to the link's directory (REPARSE_DATA_BUFFER after its
/// header: SubstituteNameOffset, SubstituteNameLength, PrintNameOffset,
/// PrintNameLength, a symbolic link's Flags, then the names).
pub fn link_target(r: &ramfs::Reparse) -> Result<(Vec<u16>, bool), NtStatus> {
    let d = &r.data;
    let u16_at = |at: usize| d.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let (names, relative) = if r.tag == ramfs::TAG_SYMLINK {
        let flags = d.get(8..12).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        (12, flags.ok_or(NtStatus::IO_REPARSE_DATA_INVALID)? & SYMLINK_FLAG_RELATIVE != 0)
    } else {
        (8, false)
    };
    let (off, len) = (
        u16_at(0).ok_or(NtStatus::IO_REPARSE_DATA_INVALID)?,
        u16_at(2).ok_or(NtStatus::IO_REPARSE_DATA_INVALID)?,
    );
    let bytes = d.get(names + off..names + off + len).ok_or(NtStatus::IO_REPARSE_DATA_INVALID)?;
    let mut name = Vec::new();
    name.try_reserve_exact(bytes.len() / 2).map_err(|_| NtStatus::NO_MEMORY)?;
    name.extend(bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])));
    Ok((name, relative))
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
                // A Linux symbolic link or socket is a reparse point
                // nothing here follows (ARCHITECTURE "Files"): opened only
                // as itself. An NT link's was followed before
                // (`resolve_links`), unless it is the one to create.
                let reparse = n.is_symlink() || n.is_socket() || n.reparse().is_some();
                if reparse && options & FILE_OPEN_REPARSE_POINT == 0 {
                    return Err(NtStatus::IO_REPARSE_TAG_NOT_HANDLED);
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
        position: IrqMutex::named("file.position", 0),
        delete_on_close,
        scan: IrqMutex::new(None),
        mode: mode_of(options),
        dispatcher: crate::mm::heap::KArc::try_new(super::wait::Dispatcher::new(0)).map_err(
            |_| {
                with_fs(|fs| fs.close(node, access, share));
                NtStatus::NO_MEMORY
            },
        )?,
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
        b[8..16].copy_from_slice(&n.accessed.to_le_bytes()); // LastAccessTime
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
        b[16..20].copy_from_slice(&n.nlink().to_le_bytes()); // NumberOfLinks (hard links)
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
        put(out, 16, &n.accessed.to_le_bytes()); // LastAccessTime
        put(out, 24, &n.written.to_le_bytes());
        put(out, 32, &n.written.to_le_bytes());
        put(out, 40, &n.size().to_le_bytes());
        put(out, 48, &n.size().next_multiple_of(4096).to_le_bytes());
        put(out, 56, &n.attributes.to_le_bytes());
        put(out, 60, &(name_bytes as u32).to_le_bytes());
        // EaSize: a reparse point's tag instead (what FindFirstFile puts
        // in dwReserved0).
        if class != FILE_DIRECTORY_INFORMATION && n.attributes & ramfs::ATTRIBUTE_REPARSE_POINT != 0
        {
            put(out, 64, &n.reparse_tag().to_le_bytes());
        }
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
        // Every name: first names and hard links.
        let ids = fs.entries(dir).map_err(fs_status)?;
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
            .chain(ids.iter().map(|&(id, name)| (None, name, id)).filter(|(_, name, _)| {
                scan.after.as_ref().is_none_or(|a| ramfs::order(name, a).is_gt())
            }));
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
pub const FILE_FS_SECTOR_SIZE_INFORMATION: u32 = 11;

const FILE_DEVICE_DISK: u32 = 7;
const FILE_DEVICE_IS_MOUNTED: u32 = 0x20;
const FILE_CASE_PRESERVED_NAMES: u32 = 0x2;
const FILE_UNICODE_ON_DISK: u32 = 0x4;
const VOLUME_SERIAL: u32 = 0x6a1a_0001;
const SECTORS_PER_CLUSTER: u32 = 8;
const BYTES_PER_SECTOR: u32 = 512;
/// What FileFsSectorSizeInformation reports as the physical sector (a
/// 512e disk: 512-byte logical, 4 KiB physical sectors).
const PHYSICAL_BYTES_PER_SECTOR: u32 = 4096;
/// The file system name reported to programs (they check for it).
const FS_NAME: &[u8] = b"NTFS";

/// Volume information of `class` as (bytes, minimum buffer length). Sizes
/// are in 4 KiB clusters: the data area's on a disk-backed C: (D4), else
/// free and total RAM (the volume lives in memory).
pub fn volume_information(class: u32) -> Result<(Vec<u8>, usize), NtStatus> {
    let (free, total) = match with_fs(|fs| fs.area_size()) {
        Some((count, free)) => (u64::from(free), u64::from(count)),
        None => platform::frame_stats(),
    };
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
        FILE_FS_SECTOR_SIZE_INFORMATION => {
            // LogicalBytesPerSector, PhysicalBytesPerSectorForAtomicity,
            // ...ForPerformance, FileSystemEffectivePhysicalBytesPerSector
            // ForAtomicity, Flags (aligned device and partition), and the
            // two alignment offsets (0).
            grow(&mut b, 28)?;
            put(&mut b, 0, &BYTES_PER_SECTOR.to_le_bytes());
            for at in [4, 8, 12] {
                put(&mut b, at, &PHYSICAL_BYTES_PER_SECTOR.to_le_bytes());
            }
            put(&mut b, 16, &3u32.to_le_bytes());
            28
        }
        _ => {
            super::ledger::record("NtQueryVolumeInformationFile", "info class", class.into());
            return Err(NtStatus::INVALID_INFO_CLASS);
        }
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

    #[test]
    fn same_path_folds_ascii_case_only() {
        let w = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert!(same_path(
            &w("\\Device\\X\\Program Files\\a.DLL"),
            &w("\\device\\x\\program files\\A.dll")
        ));
        assert!(!same_path(&w("\\a\\b.dll"), &w("\\a\\b.dl")));
        assert!(!same_path(&w("\\a\\b"), &w("\\a\\c")));
        assert!(!same_path(&w("[x"), &w("{x")));
    }
}
