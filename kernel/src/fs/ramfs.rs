//! An in-memory file system: a tree of directories, files and symbolic
//! links, with case-preserving names compared case-insensitively unless a
//! directory is case-sensitive, hard links, and NT-style share access.
//! Shared by both personalities (ARCHITECTURE "Files", LX seam 4).
//!
//! - **Names**: a directory compares names case-insensitively (NT's rule)
//!   unless its `case_sensitive` flag is set (Windows' per-directory
//!   case sensitivity, FileCaseSensitiveInformation): then exactly, for
//!   every caller, so two names differing only in case can live in it. A
//!   new directory inherits its parent's flag (as Windows' does); the Linux
//!   personality sets it on every directory it makes (`create_posix`).
//! - **Hard links**: a node's own `parent` and `name` are its first link
//!   (what `path_of` gives, what NT reports); further links are entries of
//!   `RamFs::links`. Removing the first link while others remain moves the
//!   node to one of them. Directories have exactly one link.
//! - **Symbolic links**: `Kind::Symlink` keeps the target as the bytes
//!   given (a Linux path); following them is the caller's (`lx::file`).
//!   NT sees one as a file with FILE_ATTRIBUTE_REPARSE_POINT.
//! - **Reparse points** (`set_reparse`: NT's FSCTL_SET_REPARSE_POINT):
//!   a file or an empty directory may carry one, a tag and its data as
//!   given, as NTFS keeps them; it shows as FILE_ATTRIBUTE_REPARSE_POINT.
//!   What a tag means (an NT symbolic link, a junction) is the NT layer's
//!   (`nt::file`); the Linux personality sees such a node as what it is
//!   (a file or a directory).
//! - **Unlink of an open node** (`unlink`, Linux's rule): the name goes at
//!   once and the node lives on, *detached*, until its last close; NT's
//!   delete (`set_delete_pending`) keeps the name until the last close.
//! - **Anonymous files** (`create_anonymous`: Linux's memfd_create): a
//!   file made detached, in no directory, freed at its last close. Its
//!   seals (`Node::seals`, Linux's F_SEAL_* bits; None for every other
//!   file, which cannot be sealed) are checked here, under the file
//!   system's lock, by `write`, `set_size`, `map` and `set_mode`, so no
//!   writer slips past a seal being added (`add_seals`).
//! - **Socket nodes** (`create_socket`: a bound AF_UNIX socket's name):
//!   no contents; what is bound to one is the socket layer's (`unix`),
//!   keyed by the node, which the bound socket keeps open so its id is
//!   not reused. NT sees a reparse point, as Windows shows an AF_UNIX
//!   socket's file. They are not kept across reboots (the change log has
//!   no record for them: what was bound to one is gone then anyway).
//!
//! - **Volumes** (`create_volume`: what the Linux personality mounts,
//!   its `/run`): a directory tree of its own beside the root's, its root
//!   its own parent, so no path from the root reaches it (NT never sees
//!   it). Nothing in it is recorded in the change log (gone at shutdown,
//!   as a tmpfs); `path_of` gives a node's path within its volume; a
//!   rename or hard link from one tree to another is refused
//!   (`FsError::CrossVolume`).
//!
//! Pure data structure (host-tested); the NT layer maps it to file objects.

use alloc::vec::Vec;

use alloc::boxed::Box;

use super::clusters::Clusters;
use super::log::{self, Op, Run};
use crate::mm::heap::{TryVec, try_box};
use core::cell::Cell;

pub type NodeId = usize;

/// The root directory.
pub const ROOT: NodeId = 0;

pub const ATTRIBUTE_READONLY: u32 = 0x01;
pub const ATTRIBUTE_DIRECTORY: u32 = 0x10;
pub const ATTRIBUTE_ARCHIVE: u32 = 0x20;
pub const ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsError {
    NotFound,
    PathNotFound,
    Exists,
    NotADirectory,
    IsADirectory,
    SharingViolation,
    DeletePending,
    DirectoryNotEmpty,
    /// E.g. replacing a directory or an open file by rename.
    AccessDenied,
    /// Truncating below the size a section maps.
    UserMapped,
    NoMemory,
    /// A symbolic link where a file's or directory's contents were wanted.
    Symlink,
    /// A socket where a file's contents were wanted.
    Socket,
    /// A seal forbids the change (`Node::seals`).
    Sealed,
    /// A seal against writing cannot be added while a section maps the
    /// file writable.
    WriteMapped,
    /// The file cannot be sealed (only anonymous files can).
    NotSealable,
    /// A rename or hard link from one volume's tree to another's.
    CrossVolume,
    /// A page the call needs lives on the disk only (a disk-backed C:):
    /// `take_page_in` says which; read it, `fill` it, call again
    /// (`nt::file::with_pages`).
    NotResident,
    /// The data area has no free cluster for the pages a write needs.
    DiskFull,
    /// A symbolic link's reparse point on a file with contents
    /// (`set_reparse`).
    ReparseHasData,
    /// The node has a reparse point of another tag (`set_reparse`,
    /// `delete_reparse`).
    ReparseTagMismatch,
    /// The node has no reparse point (`delete_reparse`).
    NotReparsePoint,
}

/// A reparse point's tag and data (`Node::reparse`): the bytes after the
/// REPARSE_DATA_BUFFER header (for a tag not Microsoft's, its GUID first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reparse {
    pub tag: u32,
    pub data: Vec<u8>,
}

/// IO_REPARSE_TAG_SYMLINK: an NT symbolic link (it must have no contents).
pub const TAG_SYMLINK: u32 = 0xa000_000c;
/// IO_REPARSE_TAG_MOUNT_POINT: a junction (a directory's).
pub const TAG_MOUNT_POINT: u32 = 0xa000_0003;
/// WSL's tags for a Linux symbolic link and an AF_UNIX socket's name.
pub const TAG_LX_SYMLINK: u32 = 0xa000_001d;
pub const TAG_AF_UNIX: u32 = 0x8000_0023;

/// The seals of an anonymous file, as Linux numbers them (fcntl(2)
/// F_ADD_SEALS): no more seals; no shrinking; no growing; no writing (nor
/// writable shared mappings); no change of the execute bits.
pub const SEAL_SEAL: u32 = 0x1;
pub const SEAL_SHRINK: u32 = 0x2;
pub const SEAL_GROW: u32 = 0x4;
pub const SEAL_WRITE: u32 = 0x8;
pub const SEAL_FUTURE_WRITE: u32 = 0x10;
pub const SEAL_EXEC: u32 = 0x20;
/// Every seal `add_seals` knows.
pub const SEALS: u32 =
    SEAL_SEAL | SEAL_SHRINK | SEAL_GROW | SEAL_WRITE | SEAL_FUTURE_WRITE | SEAL_EXEC;

/// Size of a file page.
pub const PAGE: usize = 4096;

/// One page of file contents: page-aligned and at a fixed address while it
/// exists, so the kernel can map it into address spaces (memory-mapped
/// files) and see the same bytes as reads and writes.
#[repr(C, align(4096))]
pub struct Page(pub [u8; PAGE]);

enum Data {
    /// Immutable contents owned elsewhere (boot modules).
    Static(&'static [u8]),
    /// `len` bytes in whole pages; bytes past `len` are kept zero. A page
    /// not made yet (`None`) holds `backing`'s bytes at its offset (zeros
    /// past `backing`): a boot module's file turns into pages lazily, a page
    /// at a time when it is written or mapped (copying a large file at once
    /// held the file system for many seconds). On a disk-backed C: (D4)
    /// `disk` says which pages live in a cluster of the data area: such a
    /// page not in memory is read from the disk (`FsError::NotResident`).
    Paged { pages: Vec<Option<Box<Page>>>, len: usize, backing: &'static [u8], disk: Vec<Slot> },
}

/// Where a page of a disk-backed file lives (`Data::Paged::disk`; a page
/// past the vector's end has the default: no cluster).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Slot {
    /// Its cluster plus one (0: none).
    cluster: u32,
    /// Changed in memory since the cluster was last written.
    dirty: bool,
    /// The cluster holds the page's data (it was written, or the change
    /// log says so): only then may the page leave memory.
    stored: bool,
    /// Written or read back since `evict`'s hand last passed it: it gets
    /// a second chance (the hand clears it) before it may leave.
    recent: bool,
}

impl Slot {
    fn cluster(&self) -> Option<u32> {
        self.cluster.checked_sub(1)
    }
}

/// The pages `[first, end)` that the bytes `bytes` of a file touch.
fn page_span(bytes: &core::ops::Range<u64>) -> (usize, usize) {
    let first = usize::try_from(bytes.start / PAGE as u64).unwrap_or(usize::MAX);
    let end = usize::try_from(bytes.end.div_ceil(PAGE as u64)).unwrap_or(usize::MAX);
    (first, end)
}

/// Page `i` made if it was not (from `backing`, else zero). The caller
/// has made sure it is not a page that lives on the disk only.
fn page_mut<'a>(
    pages: &'a mut [Option<Box<Page>>],
    backing: &[u8],
    i: usize,
) -> Result<&'a mut Page, FsError> {
    let slot = pages.get_mut(i).ok_or(FsError::NoMemory)?;
    if slot.is_none() {
        let mut page = try_box(Page([0; PAGE])).map_err(|_| FsError::NoMemory)?;
        let from = (i * PAGE).min(backing.len());
        let to = ((i + 1) * PAGE).min(backing.len());
        page.0[..to - from].copy_from_slice(&backing[from..to]);
        *slot = Some(page);
    }
    slot.as_deref_mut().ok_or(FsError::NoMemory)
}

impl Data {
    const EMPTY: Data = Data::Paged { pages: Vec::new(), len: 0, backing: &[], disk: Vec::new() };

    fn len(&self) -> usize {
        match self {
            Data::Static(s) => s.len(),
            Data::Paged { len, .. } => *len,
        }
    }

    fn slot(&self, i: usize) -> Slot {
        match self {
            Data::Paged { disk, .. } => disk.get(i).copied().unwrap_or_default(),
            Data::Static(_) => Slot::default(),
        }
    }

    /// The slot of page `i`, the vector grown to cover it.
    fn slot_mut(&mut self, i: usize) -> Result<&mut Slot, FsError> {
        let (pages, _, _, disk) = self.paged()?;
        if disk.len() <= i {
            let n = pages.len().max(i + 1);
            disk.try_reserve(n - disk.len()).map_err(|_| FsError::NoMemory)?;
            disk.resize(n, Slot::default());
        }
        Ok(&mut disk[i])
    }

    fn resident(&self, i: usize) -> bool {
        matches!(self, Data::Paged { pages, .. } if pages.get(i).is_some_and(Option::is_some))
    }

    /// The cluster to read page `i` from: it is not in memory and lives
    /// on the disk.
    fn missing(&self, i: usize) -> Option<u32> {
        let slot = self.slot(i);
        (!self.resident(i) && slot.stored).then_some(slot.cluster()?)
    }

    /// Reads from `start` until the end, the buffer's end or a page that
    /// is on the disk only (Err: its index, when nothing was read).
    fn read_at(&self, start: usize, buf: &mut [u8]) -> Result<usize, usize> {
        let len = self.len();
        if start >= len {
            return Ok(0);
        }
        let n = buf.len().min(len - start);
        match self {
            Data::Static(s) => buf[..n].copy_from_slice(&s[start..start + n]),
            Data::Paged { pages, backing, .. } => {
                let mut done = 0;
                while done < n {
                    let at = start + done;
                    let (page, off) = (at / PAGE, at % PAGE);
                    let k = (PAGE - off).min(n - done);
                    let out = &mut buf[done..done + k];
                    match pages.get(page).and_then(|p| p.as_deref()) {
                        Some(p) => out.copy_from_slice(&p.0[off..off + k]),
                        None if self.missing(page).is_some() => {
                            return if done == 0 { Err(page) } else { Ok(done) };
                        }
                        None => {
                            // Not made yet: the backing's bytes, zeros past it.
                            let from = at.min(backing.len());
                            let to = (at + k).min(backing.len());
                            out[..to - from].copy_from_slice(&backing[from..to]);
                            out[to - from..].fill(0);
                        }
                    }
                    done += k;
                }
            }
        }
        Ok(n)
    }

    /// The pages, after turning static data into (lazy) pages.
    #[allow(clippy::type_complexity)]
    fn paged(
        &mut self,
    ) -> Result<
        (&mut Vec<Option<Box<Page>>>, &mut usize, &mut &'static [u8], &mut Vec<Slot>),
        FsError,
    > {
        if let Data::Static(s) = *self {
            let mut pages = Vec::new();
            let n = s.len().div_ceil(PAGE);
            pages.try_reserve_exact(n).map_err(|_| FsError::NoMemory)?;
            pages.extend(core::iter::repeat_with(|| None).take(n));
            *self = Data::Paged { pages, len: s.len(), backing: s, disk: Vec::new() };
        }
        match self {
            Data::Paged { pages, len, backing, disk } => Ok((pages, len, backing, disk)),
            Data::Static(_) => unreachable!(),
        }
    }

    /// Makes sure the pages cover `size` bytes (new pages are zero, made
    /// when first used).
    fn cover(pages: &mut Vec<Option<Box<Page>>>, size: usize) -> Result<(), FsError> {
        let need = size.div_ceil(PAGE);
        if need > pages.len() {
            pages.try_reserve(need - pages.len()).map_err(|_| FsError::NoMemory)?;
            let more = need - pages.len();
            pages.extend(core::iter::repeat_with(|| None).take(more));
        }
        Ok(())
    }

    /// Writes `data` at `start`; a page on the disk only must be in memory
    /// first unless the write covers all of it (see `RamFs::write`).
    fn write_at(&mut self, start: usize, data: &[u8]) -> Result<(), FsError> {
        let end = start.checked_add(data.len()).ok_or(FsError::NoMemory)?;
        let (pages, len, backing, _) = self.paged()?;
        Self::cover(pages, end)?;
        let mut done = 0;
        while done < data.len() {
            let at = start + done;
            let (page, off) = (at / PAGE, at % PAGE);
            let k = (PAGE - off).min(data.len() - done);
            page_mut(pages, backing, page)?.0[off..off + k].copy_from_slice(&data[done..done + k]);
            done += k;
        }
        *len = (*len).max(end);
        Ok(())
    }

    /// Sets the length; the clusters of pages that go are added to
    /// `freed`. A partial last page on the disk only must be in memory
    /// first (see `RamFs::set_size`).
    fn set_len(&mut self, size: usize, freed: &mut Vec<u32>) -> Result<(), FsError> {
        let (pages, len, backing, disk) = self.paged()?;
        Self::cover(pages, size)?;
        if size < *len {
            // Keep bytes past the end zero (mapped views show them): pages
            // not made yet read from the backing, so it ends there too.
            *backing = &backing[..size.min(backing.len())];
            let keep = size.div_ceil(PAGE);
            pages.truncate(keep);
            if disk.len() > keep {
                for slot in disk.drain(keep..) {
                    if let Some(c) = slot.cluster() {
                        // Out of memory: the cluster stays used until the
                        // next mount (`Clusters::leaked` counts such).
                        let _ = freed.try_push(c);
                    }
                }
            }
            if !size.is_multiple_of(PAGE)
                && let Some(Some(last)) = pages.get_mut(keep - 1)
            {
                last.0[size % PAGE..].fill(0);
            }
        }
        *len = size;
        Ok(())
    }

    /// The address of page `index`, made if it was not (for mapping). The
    /// caller has made sure it is not on the disk only.
    fn page_address(&mut self, index: usize) -> Option<usize> {
        match self {
            Data::Paged { pages, backing, .. } => {
                page_mut(pages, backing, index).ok().map(|p| p as *const Page as usize)
            }
            Data::Static(_) => None,
        }
    }

    /// Every cluster the file uses.
    fn clusters(&self) -> impl Iterator<Item = u32> + '_ {
        let disk: &[Slot] = match self {
            Data::Paged { disk, .. } => disk,
            Data::Static(_) => &[],
        };
        disk.iter().filter_map(Slot::cluster)
    }
}

enum Kind {
    /// The nodes whose first link is in this directory (more names in it
    /// may be hard links: `RamFs::links`).
    Directory(Vec<NodeId>),
    File(Data),
    /// A symbolic link and its target, as given (bytes of a Linux path).
    Symlink(Vec<u8>),
    /// A socket's name (`create_socket`).
    Socket,
}

/// A hard link beyond a node's first: the entry `name` of directory
/// `parent` names `node` too.
struct Link {
    parent: NodeId,
    name: Vec<u16>,
    node: NodeId,
}

/// Requested data access of an open, for share checks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Access {
    pub read: bool,
    pub write: bool,
    pub delete: bool,
}

/// Which access other opens may have at the same time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Share {
    pub read: bool,
    pub write: bool,
    pub delete: bool,
}

#[derive(Default)]
struct Opens {
    /// Every open, including those without data access.
    handles: u32,
    /// Opens with data access (they take part in share checks).
    count: u32,
    readers: u32,
    writers: u32,
    deleters: u32,
    shared_read: u32,
    shared_write: u32,
    shared_delete: u32,
}

/// The times FileBasicInformation sets (NT times; None keeps one).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Times {
    pub created: Option<u64>,
    pub accessed: Option<u64>,
    pub written: Option<u64>,
}

impl Times {
    pub fn all(created: u64, accessed: u64, written: u64) -> Self {
        Self { created: Some(created), accessed: Some(accessed), written: Some(written) }
    }
}

pub struct Node {
    parent: NodeId,
    name: Vec<u16>,
    kind: Kind,
    pub attributes: u32,
    /// NT times (100 ns since 1601). The access time is its own: set with
    /// the others (FileBasicInformation, scp -p) and by every write, not by
    /// reads (as Windows with last-access updates off); the change time is
    /// the write time.
    pub created: u64,
    pub accessed: u64,
    pub written: u64,
    /// Changes with every change of the contents (and when a writable
    /// data section maps the file or ends), unique across all nodes: what
    /// caches of the contents key on.
    version: u64,
    opens: Opens,
    delete_pending: bool,
    /// Sections mapping the file, how many of them can write it, and the
    /// largest size they map.
    map_count: u32,
    /// The pages (first, end) each live view of its sections shows
    /// (`view_started`): those stay in memory while one may show them.
    views: Vec<(usize, usize)>,
    /// Live views not in `views` (no memory to list one): every page
    /// stays while there are any.
    views_unlisted: u32,
    write_maps: u32,
    mapped_size: u64,
    /// Its security descriptor (self-relative; nt::security), if one was
    /// assigned (files gala creates at boot have none).
    pub security: Option<Vec<u8>>,
    /// Its POSIX permission bits (`0o7777`), set when the Linux personality
    /// creates it or changes them (lx::file); None for a node made without
    /// them (boot files, NT programs' files), which Linux sees with a
    /// default (`lx::file`). Kept in the change log (Create, SetMode).
    pub mode: Option<u16>,
    /// A directory whose names compare exactly (see the module's doc).
    pub case_sensitive: bool,
    /// An anonymous file's seals (`SEAL_*`); None: it cannot be sealed.
    seals: Option<u32>,
    /// Its names beyond the first (entries of `RamFs::links`).
    extra_links: u32,
    /// Unlinked while open (`unlink`): in no directory, freed at its last
    /// close, never recorded again.
    detached: bool,
    /// The root of the tree it is in: ROOT, or a volume's (`create_volume`).
    volume: NodeId,
    /// Made by the boot (modules), not by a program or the change log.
    boot: bool,
    /// Created or changed since boot (or by the change log): part of a
    /// snapshot of the changes (`snapshot`).
    dirty: bool,
    /// Its reparse point (`set_reparse`), if it has one.
    reparse: Option<Reparse>,
    /// Unique across all nodes ever made (a page read for a node whose id
    /// was reused meanwhile is not taken: `fill`).
    serial: u64,
    /// Changes whenever a page of the file leaves memory or gives up its
    /// cluster (a page read from the disk meanwhile is not taken: `fill`).
    epoch: u64,
}

/// A new content version (see `Node::version`).
fn next_version() -> u64 {
    static NEXT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

impl Node {
    /// The contents' version, or None while a writable data section maps
    /// the file (writes through its views do not pass through here). A
    /// read-only one cannot change it: image sections of the file keep
    /// sharing their cached layout (Chromium maps chrome.dll as data while
    /// every process maps it as an image).
    pub fn version(&self) -> Option<u64> {
        (self.write_maps == 0).then_some(self.version)
    }

    pub fn is_dir(&self) -> bool {
        matches!(self.kind, Kind::Directory(_))
    }

    pub fn is_symlink(&self) -> bool {
        matches!(self.kind, Kind::Symlink(_))
    }

    /// A socket's name (`create_socket`).
    pub fn is_socket(&self) -> bool {
        matches!(self.kind, Kind::Socket)
    }

    /// An anonymous file's seals; None for a file that cannot be sealed.
    pub fn seals(&self) -> Option<u32> {
        self.seals
    }

    /// How many sections map it writable (a seal against writing waits
    /// for none).
    pub fn write_maps(&self) -> u32 {
        self.write_maps
    }

    /// A symbolic link's target.
    pub fn symlink_target(&self) -> Option<&[u8]> {
        match &self.kind {
            Kind::Symlink(t) => Some(t),
            _ => None,
        }
    }

    /// Its reparse point, if it has one (`RamFs::set_reparse`).
    pub fn reparse(&self) -> Option<&Reparse> {
        self.reparse.as_ref()
    }

    /// Its reparse tag as NT reports it (FileAttributeTagInformation, a
    /// directory entry's EaSize): its reparse point's; a Linux symbolic
    /// link's and socket's are WSL's (IO_REPARSE_TAG_LX_SYMLINK,
    /// IO_REPARSE_TAG_AF_UNIX); 0 for none.
    pub fn reparse_tag(&self) -> u32 {
        match (&self.reparse, &self.kind) {
            (Some(r), _) => r.tag,
            (None, Kind::Symlink(_)) => TAG_LX_SYMLINK,
            (None, Kind::Socket) => TAG_AF_UNIX,
            (None, _) => 0,
        }
    }

    /// How many names it has (0 once unlinked while open).
    pub fn nlink(&self) -> u32 {
        if self.detached { 0 } else { 1 + self.extra_links }
    }

    /// Unlinked while open (see `RamFs::unlink`).
    pub fn detached(&self) -> bool {
        self.detached
    }

    pub fn name(&self) -> &[u16] {
        &self.name
    }

    pub fn parent(&self) -> NodeId {
        self.parent
    }

    /// The root of the tree it is in: ROOT, or a volume's root.
    pub fn volume(&self) -> NodeId {
        self.volume
    }

    pub fn size(&self) -> u64 {
        match &self.kind {
            Kind::File(d) => d.len() as u64,
            Kind::Symlink(t) => t.len() as u64,
            Kind::Directory(_) | Kind::Socket => 0,
        }
    }
}

/// Upcases one UTF-16 unit (ASCII only).
// TODO: the full Unicode upcase table NTFS uses.
fn fold(c: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
}

/// Case-insensitive comparison.
fn same_name(a: &[u16], b: &[u16]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| fold(x) == fold(y))
}

/// Paths compare like names.
fn same_path(a: &[u16], b: &[u16]) -> bool {
    same_name(a, b)
}

/// Names compare in directory `dir`'s way: exactly in a case-sensitive one.
fn name_matches(case_sensitive: bool, a: &[u16], b: &[u16]) -> bool {
    if case_sensitive { a == b } else { same_name(a, b) }
}

/// The order entries are listed and resumed in: NTFS collation, then (for
/// names only a case-sensitive directory can hold together) the units.
pub fn order(a: &[u16], b: &[u16]) -> core::cmp::Ordering {
    collate(a, b).then_with(|| a.cmp(b))
}

/// A fallible copy.
fn copy<T: Copy>(s: &[T]) -> Option<Vec<T>> {
    let mut v = Vec::new();
    v.try_extend_from_slice(s).ok()?;
    Some(v)
}

/// Directory order: upcased names compared unit by unit (NTFS collation).
pub fn collate(a: &[u16], b: &[u16]) -> core::cmp::Ordering {
    a.iter().map(|&c| fold(c)).cmp(b.iter().map(|&c| fold(c)))
}

/// Case-insensitive wildcard match (FsRtlIsNameInExpression's rules): `*`
/// any run, `?` one unit, and the DOS forms kernel32 makes of Win32
/// patterns: `<` (DOS_STAR) any run up to the name's final period,
/// `>` (DOS_QM) one unit, or none at a period or the end of the name,
/// `"` (DOS_DOT) a period, or none at the end. Run as a set of mask
/// positions (an NFA, no backtracking); a mask over 319 units matches
/// nothing (names are at most 255).
pub fn wildcard_match(mask: &[u16], name: &[u16]) -> bool {
    const WORDS: usize = 5;
    type Set = [u64; WORDS];
    if mask.len() >= WORDS * 64 {
        return false;
    }
    let (star, dos_star, qm, dos_qm, dos_dot, dot) =
        (b'*' as u16, b'<' as u16, b'?' as u16, b'>' as u16, b'"' as u16, b'.' as u16);
    let has = |s: &Set, m: usize| s[m / 64] & (1 << (m % 64)) != 0;
    let add = |s: &mut Set, m: usize| s[m / 64] |= 1 << (m % 64);
    // Positions reachable without consuming name[n] (or at the end).
    let closure = |s: &mut Set, n: usize| {
        let at_end = n == name.len();
        for (m, &c) in mask.iter().enumerate() {
            if !has(s, m) {
                continue;
            }
            let skip = c == star
                || c == dos_star
                || (c == dos_qm && (at_end || name[n] == dot))
                || (c == dos_dot && at_end);
            if skip {
                add(s, m + 1);
            }
        }
    };
    let mut set: Set = [0; WORDS];
    add(&mut set, 0);
    closure(&mut set, 0);
    for n in 0..name.len() {
        let unit = name[n];
        // DOS_STAR stops at the final period (it may take earlier ones).
        let final_dot = unit == dot && !name[n + 1..].contains(&dot);
        let mut next: Set = [0; WORDS];
        for (m, &c) in mask.iter().enumerate() {
            if !has(&set, m) {
                continue;
            }
            if c == star || (c == dos_star && !final_dot) {
                add(&mut next, m);
            } else if c == qm
                || (c == dos_qm && unit != dot)
                || (c == dos_dot && unit == dot)
                || (c != dos_star && c != dos_qm && c != dos_dot && fold(c) == fold(unit))
            {
                add(&mut next, m + 1);
            }
        }
        if next.iter().all(|&w| w == 0) {
            return false;
        }
        set = next;
        closure(&mut set, n + 1);
    }
    has(&set, mask.len())
}

pub fn has_wildcards(mask: &[u16]) -> bool {
    mask.iter().any(|&c| b"*?<>\"".iter().any(|&w| c == u16::from(w)))
}

pub struct RamFs {
    nodes: Vec<Option<Node>>,
    /// Changes are recorded for the disk (fs::log) while this is on.
    pub recording: bool,
    /// The change log is being replayed: what it makes is not the boot's.
    replaying: bool,
    /// Changes recorded since the last `take_ops`.
    ops: Vec<Op>,
    /// A change could not be recorded: only a snapshot is right now.
    ops_lost: bool,
    /// Paths of boot nodes removed or renamed away: the boot recreates
    /// them, a snapshot deletes them again.
    tombstones: Vec<Vec<u16>>,
    /// Hard links beyond nodes' first (see the module's doc).
    links: Vec<Link>,
    /// Changes since the last `take_changes`, recorded while `journal` is
    /// on (directory change notification: nt::notify).
    changes: Vec<Change>,
    pub journal: bool,
    /// A change could not be recorded (no memory): watchers must rescan.
    changes_lost: bool,
    /// The volume's data area, once attached (a disk-backed C:, D4).
    area: Option<Area>,
    /// The page the last call that failed with NotResident needs.
    miss: Cell<Option<PageIn>>,
}

/// The data area's state: its clusters, the pages to write to them, the
/// pages that may leave memory (see "A disk-backed C:" in ARCHITECTURE).
struct Area {
    clusters: Clusters,
    /// Pages changed since written (node, page; entries whose slot is no
    /// longer dirty are skipped).
    dirty: Vec<(NodeId, usize)>,
    /// `dirty` could not grow: writebacks scan every file until a scan
    /// finds them all.
    dirty_lost: bool,
    /// Where `evict`'s hand is (node, page): it goes round the files'
    /// pages in memory, a second chance each (`Slot::recent`).
    hand: (NodeId, usize),
    /// Dirty pages left without a cluster (the area was full at a sync).
    pub unbacked: u64,
}

/// A page to read from the disk (`RamFs::take_page_in`): cluster
/// `cluster` of the data area, for page `index` of `node`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageIn {
    pub node: NodeId,
    pub index: usize,
    pub cluster: u32,
    serial: u64,
    epoch: u64,
}

/// Files' pages in memory on a disk-backed C: (`RamFs::cache_stats`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Pages in memory.
    pub resident: u64,
    /// Of those: changed since written (they leave only once written).
    pub dirty: u64,
    /// Clean but not on the disk (no cluster: a boot module's page read,
    /// a hole made): they never leave.
    pub unstored: u64,
    /// Of a file a view shows (they stay while one does).
    pub mapped: u64,
    /// Entries of the dirty list.
    pub queued_dirty: u64,
}

/// A dirty page to write to its cluster (`RamFs::writeback`; its copy is
/// in the caller's staging buffer, in the same order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Writeback {
    pub node: NodeId,
    pub index: usize,
    pub cluster: u32,
}

/// FILE_ACTION_* of a change.
pub const ACTION_ADDED: u32 = 1;
pub const ACTION_REMOVED: u32 = 2;
pub const ACTION_MODIFIED: u32 = 3;
pub const ACTION_RENAMED_OLD_NAME: u32 = 4;
pub const ACTION_RENAMED_NEW_NAME: u32 = 5;

/// One change to a directory's entries or to a file's contents.
pub struct Change {
    /// The directory the entry is (or was) in, and its name there.
    pub parent: NodeId,
    pub name: Vec<u16>,
    pub directory: bool,
    pub action: u32,
}

impl RamFs {
    pub fn new(now: u64) -> Result<Self, FsError> {
        let mut nodes = Vec::new();
        let root = Node {
            parent: ROOT,
            name: Vec::new(),
            kind: Kind::Directory(Vec::new()),
            attributes: ATTRIBUTE_DIRECTORY,
            created: now,
            accessed: now,
            written: now,
            version: next_version(),
            opens: Opens::default(),
            delete_pending: false,
            map_count: 0,
            views: Vec::new(),
            views_unlisted: 0,
            write_maps: 0,
            mapped_size: 0,
            security: None,
            mode: None,
            case_sensitive: false,
            seals: None,
            extra_links: 0,
            detached: false,
            volume: ROOT,
            boot: true,
            dirty: false,
            reparse: None,
            serial: next_version(),
            epoch: 0,
        };
        nodes.try_push(Some(root)).map_err(|_| FsError::NoMemory)?;
        Ok(Self {
            nodes,
            recording: false,
            replaying: false,
            ops: Vec::new(),
            ops_lost: false,
            tombstones: Vec::new(),
            links: Vec::new(),
            changes: Vec::new(),
            journal: false,
            changes_lost: false,
            area: None,
            miss: Cell::new(None),
        })
    }

    /// Records a change (if the journal is on); the same change twice in a
    /// row is recorded once (writes come in chunks).
    fn note(&mut self, parent: NodeId, name: &[u16], directory: bool, action: u32) {
        if !self.journal {
            return;
        }
        if let Some(last) = self.changes.last()
            && last.parent == parent
            && last.action == action
            && last.name == name
        {
            return;
        }
        let mut owned = Vec::new();
        let recorded = owned.try_extend_from_slice(name).is_ok()
            && self.changes.try_push(Change { parent, name: owned, directory, action }).is_ok();
        if !recorded {
            self.changes_lost = true;
        }
    }

    /// The changes recorded so far, and whether some were lost.
    pub fn take_changes(&mut self) -> (Vec<Change>, bool) {
        let lost = core::mem::take(&mut self.changes_lost);
        (core::mem::take(&mut self.changes), lost)
    }

    /// The name of entry `name` of directory `parent` relative to `dir`:
    /// `name` if `parent` is `dir`; with `tree`, `sub\\name` for entries
    /// below it; None otherwise.
    pub fn relative_name(
        &self,
        dir: NodeId,
        parent: NodeId,
        name: &[u16],
        tree: bool,
    ) -> Option<Vec<u16>> {
        let mut chain: Vec<NodeId> = Vec::new();
        let mut at = parent;
        while at != dir {
            if !tree || self.is_root(at) {
                return None;
            }
            chain.try_push(at).ok()?;
            at = self.node(at)?.parent;
        }
        let mut out = Vec::new();
        for &n in chain.iter().rev() {
            out.try_extend_from_slice(&self.node(n)?.name).ok()?;
            out.try_push(u16::from(b'\\')).ok()?;
        }
        out.try_extend_from_slice(name).ok()?;
        Some(out)
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id)?.as_ref()
    }

    /// Sets a node's security descriptor.
    pub fn set_security(&mut self, id: NodeId, sd: Vec<u8>) -> Result<(), FsError> {
        self.node_mut(id)?.security = Some(sd);
        self.record_node(id, |path, n| {
            Some(Op::SetSecurity { path, descriptor: copy(n.security.as_deref().unwrap_or(&[]))? })
        });
        Ok(())
    }

    fn node_mut(&mut self, id: NodeId) -> Result<&mut Node, FsError> {
        self.nodes.get_mut(id).and_then(Option::as_mut).ok_or(FsError::NotFound)
    }

    fn children(&self, dir: NodeId) -> Result<&[NodeId], FsError> {
        match &self.node(dir).ok_or(FsError::NotFound)?.kind {
            Kind::Directory(c) => Ok(c),
            Kind::File(_) | Kind::Symlink(_) | Kind::Socket => Err(FsError::NotADirectory),
        }
    }

    /// Whether directory `dir` compares names exactly.
    fn exact(&self, dir: NodeId) -> bool {
        self.node(dir).is_some_and(|n| n.case_sensitive)
    }

    /// Finds `name` in directory `dir` (a first name or a hard link).
    pub fn find(&self, dir: NodeId, name: &[u16]) -> Result<Option<NodeId>, FsError> {
        let children = self.children(dir)?;
        let exact = self.exact(dir);
        let first = children
            .iter()
            .copied()
            .find(|&c| self.node(c).is_some_and(|n| name_matches(exact, &n.name, name)));
        Ok(first.or_else(|| self.link_at(dir, name).map(|i| self.links[i].node)))
    }

    /// The index in `links` of hard link `name` in `dir`.
    fn link_at(&self, dir: NodeId, name: &[u16]) -> Option<usize> {
        let exact = self.exact(dir);
        self.links.iter().position(|l| l.parent == dir && name_matches(exact, &l.name, name))
    }

    /// The entries of directory `dir` (first names and hard links) as
    /// (node, name), in `order`.
    pub fn entries(&self, dir: NodeId) -> Result<Vec<(NodeId, &[u16])>, FsError> {
        let mut out = Vec::new();
        for &c in self.children(dir)? {
            if let Some(n) = self.node(c) {
                out.try_push((c, n.name.as_slice())).map_err(|_| FsError::NoMemory)?;
            }
        }
        for l in self.links.iter().filter(|l| l.parent == dir) {
            out.try_push((l.node, l.name.as_slice())).map_err(|_| FsError::NoMemory)?;
        }
        out.sort_unstable_by(|a, b| order(a.1, b.1));
        Ok(out)
    }

    /// Resolves all but the last of `components` (relative to directory
    /// `start`) as directories, returning that directory (for create/open of
    /// the last component).
    pub fn parent_of(&self, start: NodeId, components: &[&[u16]]) -> Result<NodeId, FsError> {
        let mut dir = start;
        for part in components.iter().take(components.len().saturating_sub(1)) {
            match self.find(dir, part) {
                Ok(Some(next)) if self.node(next).is_some_and(Node::is_dir) => dir = next,
                _ => return Err(FsError::PathNotFound),
            }
        }
        Ok(dir)
    }

    /// Resolves a path relative to directory `start`.
    pub fn resolve(&self, start: NodeId, components: &[&[u16]]) -> Result<NodeId, FsError> {
        let Some(last) = components.last() else { return Ok(start) };
        let parent = self.parent_of(start, components)?;
        self.find(parent, last)?.ok_or(FsError::NotFound)
    }

    /// Creates `name` in `parent` (NT's way: no POSIX mode; a directory
    /// inherits its parent's case sensitivity).
    pub fn create(
        &mut self,
        parent: NodeId,
        name: &[u16],
        directory: bool,
        now: u64,
    ) -> Result<NodeId, FsError> {
        let case_sensitive = directory && self.exact(parent);
        let kind = if directory { Kind::Directory(Vec::new()) } else { Kind::File(Data::EMPTY) };
        self.make(parent, name, kind, None, case_sensitive, now)
    }

    /// Creates `name` in `parent` the Linux way: with POSIX permission
    /// bits `mode`, a directory case-sensitive.
    pub fn create_posix(
        &mut self,
        parent: NodeId,
        name: &[u16],
        directory: bool,
        mode: u16,
        now: u64,
    ) -> Result<NodeId, FsError> {
        let kind = if directory { Kind::Directory(Vec::new()) } else { Kind::File(Data::EMPTY) };
        self.make(parent, name, kind, Some(mode & 0o7777), directory, now)
    }

    /// Creates symbolic link `name` in `parent` pointing at `target`.
    pub fn symlink(
        &mut self,
        parent: NodeId,
        name: &[u16],
        target: &[u8],
        now: u64,
    ) -> Result<NodeId, FsError> {
        let target = copy(target).ok_or(FsError::NoMemory)?;
        self.make(parent, name, Kind::Symlink(target), None, false, now)
    }

    /// Creates `name` in `parent` as a socket's name, with POSIX
    /// permission bits `mode` (not recorded in the change log: see the
    /// module's doc).
    pub fn create_socket(
        &mut self,
        parent: NodeId,
        name: &[u16],
        mode: u16,
        now: u64,
    ) -> Result<NodeId, FsError> {
        self.make(parent, name, Kind::Socket, Some(mode & 0o7777), false, now)
    }

    /// Creates an anonymous file (memfd_create): an empty file in no
    /// directory, named `name` only for logs and paths of its mappings,
    /// with POSIX bits `mode` and the seals `seals` (None: it cannot be
    /// sealed), opened once with `access` and `share`: it goes at its
    /// last close (`close`). Never recorded in the change log.
    pub fn create_anonymous(
        &mut self,
        name: &[u16],
        mode: u16,
        seals: Option<u32>,
        access: Access,
        share: Share,
        now: u64,
    ) -> Result<NodeId, FsError> {
        let mut owned_name = Vec::new();
        owned_name.try_extend_from_slice(name).map_err(|_| FsError::NoMemory)?;
        let mut node =
            self.fresh_node(ROOT, owned_name, Kind::File(Data::EMPTY), Some(mode), false, now);
        node.seals = seals;
        node.detached = true;
        node.boot = false;
        node.dirty = false;
        let id = self.slot(node)?;
        if let Err(e) = self.open(id, access, share) {
            self.nodes[id] = None;
            return Err(e);
        }
        Ok(id)
    }

    /// Creates a volume (see the module's doc): an empty, case-sensitive
    /// directory with POSIX bits `mode`, the root of a tree of its own.
    pub fn create_volume(&mut self, mode: u16, now: u64) -> Result<NodeId, FsError> {
        let mut node = self.fresh_node(
            ROOT,
            Vec::new(),
            Kind::Directory(Vec::new()),
            Some(mode & 0o7777),
            true,
            now,
        );
        node.boot = false;
        node.dirty = false;
        let id = self.slot(node)?;
        let node = self.node_mut(id)?;
        node.parent = id;
        node.volume = id;
        Ok(id)
    }

    /// Whether `id` is a tree's root: ROOT or a volume's (its own parent).
    fn is_root(&self, id: NodeId) -> bool {
        id == ROOT || self.node(id).is_some_and(|n| n.parent == id)
    }

    /// Whether changes to `id` go to the change log: it is in the root's
    /// tree, not a volume's.
    fn logged(&self, id: NodeId) -> bool {
        self.node(id).is_some_and(|n| n.volume == ROOT)
    }

    /// Refuses a move or link of `id` into directory `dir` of another tree.
    fn same_volume(&self, id: NodeId, dir: NodeId) -> Result<(), FsError> {
        let volume = |n: NodeId| self.node(n).map(|n| n.volume).ok_or(FsError::NotFound);
        if volume(id)? != volume(dir)? {
            return Err(FsError::CrossVolume);
        }
        Ok(())
    }

    /// A node of `kind` named `name` in `parent` (not yet in the table).
    fn fresh_node(
        &self,
        parent: NodeId,
        name: Vec<u16>,
        kind: Kind,
        mode: Option<u16>,
        case_sensitive: bool,
        now: u64,
    ) -> Node {
        let attributes = match kind {
            Kind::Directory(_) => ATTRIBUTE_DIRECTORY,
            Kind::File(_) => ATTRIBUTE_ARCHIVE,
            Kind::Symlink(_) | Kind::Socket => ATTRIBUTE_ARCHIVE | ATTRIBUTE_REPARSE_POINT,
        };
        Node {
            parent,
            name,
            kind,
            attributes,
            created: now,
            accessed: now,
            written: now,
            version: next_version(),
            opens: Opens::default(),
            delete_pending: false,
            map_count: 0,
            views: Vec::new(),
            views_unlisted: 0,
            write_maps: 0,
            mapped_size: 0,
            security: None,
            mode,
            case_sensitive,
            seals: None,
            extra_links: 0,
            detached: false,
            volume: self.node(parent).map_or(ROOT, |n| n.volume),
            boot: !self.recording && !self.replaying,
            dirty: self.recording || self.replaying,
            reparse: None,
            serial: next_version(),
            epoch: 0,
        }
    }

    /// Puts `node` in a free slot of the table; its id.
    fn slot(&mut self, node: Node) -> Result<NodeId, FsError> {
        Ok(match self.nodes.iter().position(Option::is_none) {
            Some(free) => {
                self.nodes[free] = Some(node);
                free
            }
            None => {
                self.nodes.try_push(Some(node)).map_err(|_| FsError::NoMemory)?;
                self.nodes.len() - 1
            }
        })
    }

    fn make(
        &mut self,
        parent: NodeId,
        name: &[u16],
        kind: Kind,
        mode: Option<u16>,
        case_sensitive: bool,
        now: u64,
    ) -> Result<NodeId, FsError> {
        if self.find(parent, name)?.is_some() {
            return Err(FsError::Exists);
        }
        let mut owned_name = Vec::new();
        owned_name.try_extend_from_slice(name).map_err(|_| FsError::NoMemory)?;
        let directory = matches!(kind, Kind::Directory(_));
        let node = self.fresh_node(parent, owned_name, kind, mode, case_sensitive, now);
        let id = self.slot(node)?;
        if let Kind::Directory(children) = &mut self.node_mut(parent)?.kind
            && children.try_push(id).is_err()
        {
            self.nodes[id] = None;
            return Err(FsError::NoMemory);
        }
        self.note(parent, name, directory, ACTION_ADDED);
        self.record_node(id, |path, n| match &n.kind {
            Kind::Symlink(target) => {
                Some(Op::Symlink { path, target: copy(target)?, created: n.created })
            }
            _ => Some(Op::Create {
                path,
                directory,
                attributes: n.attributes,
                created: n.created,
                written: n.written,
                mode: n.mode,
                case_sensitive: n.case_sensitive,
            }),
        });
        Ok(id)
    }

    /// Adds a file whose contents live elsewhere (copied into pages when
    /// first written): read-only (program files) unless `writable` (data
    /// a program changes, such as database files).
    pub fn add_static_file(
        &mut self,
        parent: NodeId,
        name: &[u16],
        data: &'static [u8],
        writable: bool,
        now: u64,
    ) -> Result<NodeId, FsError> {
        let id = self.create(parent, name, false, now)?;
        let node = self.node_mut(id)?;
        node.kind = Kind::File(Data::Static(data));
        node.attributes =
            if writable { ATTRIBUTE_ARCHIVE } else { ATTRIBUTE_READONLY | ATTRIBUTE_ARCHIVE };
        Ok(id)
    }

    /// Registers an open (NT share-access rules). Opens without data access
    /// (e.g. attributes only) do not take part.
    pub fn open(&mut self, id: NodeId, access: Access, share: Share) -> Result<(), FsError> {
        let node = self.node_mut(id)?;
        if node.delete_pending {
            return Err(FsError::DeletePending);
        }
        if !(access.read || access.write || access.delete) {
            node.opens.handles += 1;
            return Ok(());
        }
        let o = &node.opens;
        let conflict = (access.read && o.shared_read < o.count)
            || (access.write && o.shared_write < o.count)
            || (access.delete && o.shared_delete < o.count)
            || (o.readers > 0 && !share.read)
            || (o.writers > 0 && !share.write)
            || (o.deleters > 0 && !share.delete);
        if conflict {
            return Err(FsError::SharingViolation);
        }
        let o = &mut node.opens;
        o.handles += 1;
        o.count += 1;
        o.readers += u32::from(access.read);
        o.writers += u32::from(access.write);
        o.deleters += u32::from(access.delete);
        o.shared_read += u32::from(share.read);
        o.shared_write += u32::from(share.write);
        o.shared_delete += u32::from(share.delete);
        Ok(())
    }

    /// Ends an open; removes the node if it was the last open and a delete
    /// is pending, frees it if it was unlinked (`unlink`).
    pub fn close(&mut self, id: NodeId, access: Access, share: Share) {
        let Ok(node) = self.node_mut(id) else { return };
        node.opens.handles -= 1;
        if access.read || access.write || access.delete {
            let o = &mut node.opens;
            o.count -= 1;
            o.readers -= u32::from(access.read);
            o.writers -= u32::from(access.write);
            o.deleters -= u32::from(access.delete);
            o.shared_read -= u32::from(share.read);
            o.shared_write -= u32::from(share.write);
            o.shared_delete -= u32::from(share.delete);
        }
        if node.opens.handles == 0 {
            if node.detached {
                // Its name went at the unlink (and was recorded then).
                if let Some(node) = self.nodes[id].take() {
                    self.release(&node);
                }
            } else if node.delete_pending {
                self.record_removal(id);
                self.remove(id);
            }
        }
    }

    /// Removes `id`'s first name: the node moves to one of its hard links
    /// if it has any, else it goes. (Out of memory for the move, nothing
    /// changes: the name stays.)
    fn remove(&mut self, id: NodeId) {
        if let Some(i) = self.links.iter().position(|l| l.node == id) {
            let _ = self.promote(id, i);
            return;
        }
        let Some(node) = self.nodes.get_mut(id).and_then(Option::take) else { return };
        self.release(&node);
        self.note(node.parent, &node.name, node.is_dir(), ACTION_REMOVED);
        if let Some(Some(Node { kind: Kind::Directory(children), .. })) =
            self.nodes.get_mut(node.parent)
        {
            children.retain(|&c| c != id);
        }
    }

    /// Moves node `id` from its first name to its hard link `links[i]`,
    /// which becomes its first name.
    fn promote(&mut self, id: NodeId, i: usize) -> Result<(), FsError> {
        let to = self.links.get(i).map(|l| l.parent).ok_or(FsError::NotFound)?;
        match &mut self.node_mut(to)?.kind {
            Kind::Directory(c) => c.try_reserve(1).map_err(|_| FsError::NoMemory)?,
            _ => return Err(FsError::NotADirectory),
        }
        let link = self.links.remove(i);
        let node = self.node_mut(id)?;
        let from = node.parent;
        let old = core::mem::replace(&mut node.name, link.name);
        node.parent = link.parent;
        node.extra_links -= 1;
        node.delete_pending = false;
        let directory = node.is_dir();
        if let Kind::Directory(c) = &mut self.node_mut(from)?.kind {
            c.retain(|&x| x != id);
        }
        if let Kind::Directory(c) = &mut self.node_mut(to)?.kind {
            let _ = c.try_push(id);
        }
        self.note(from, &old, directory, ACTION_REMOVED);
        // A snapshot makes it at its new first name.
        self.mark_dirty(id);
        Ok(())
    }

    /// The full path of `id` (`\\a\\b`; `\\` for the root) within its tree
    /// (a volume's: from the volume's root).
    pub fn path_of(&self, id: NodeId) -> Result<Vec<u16>, FsError> {
        let mut chain = Vec::new();
        let mut at = id;
        while !self.is_root(at) {
            chain.try_push(at).map_err(|_| FsError::NoMemory)?;
            at = self.node(at).ok_or(FsError::NotFound)?.parent;
        }
        let mut out = Vec::new();
        if chain.is_empty() {
            out.try_push(u16::from(b'\\')).map_err(|_| FsError::NoMemory)?;
        }
        for &n in chain.iter().rev() {
            out.try_push(u16::from(b'\\')).map_err(|_| FsError::NoMemory)?;
            let name = &self.node(n).ok_or(FsError::NotFound)?.name;
            out.try_extend_from_slice(name).map_err(|_| FsError::NoMemory)?;
        }
        Ok(out)
    }

    /// The path of entry `name` of directory `dir` (`\\a\\name`).
    pub fn entry_path(&self, dir: NodeId, name: &[u16]) -> Result<Vec<u16>, FsError> {
        let mut out = self.path_of(dir)?;
        if self.is_root(dir) {
            out.clear();
        }
        out.try_push(u16::from(b'\\')).map_err(|_| FsError::NoMemory)?;
        out.try_extend_from_slice(name).map_err(|_| FsError::NoMemory)?;
        Ok(out)
    }

    /// The nodes whose first name is in directory `dir`, in `order` (its
    /// hard links are not: `entries` has every name).
    pub fn list(&self, dir: NodeId) -> Result<Vec<NodeId>, FsError> {
        let mut out = Vec::new();
        out.try_extend_from_slice(self.children(dir)?).map_err(|_| FsError::NoMemory)?;
        let name = |id: NodeId| self.node(id).map_or(&[][..], |n| n.name.as_slice());
        out.sort_unstable_by(|&a, &b| order(name(a), name(b)));
        Ok(out)
    }

    /// Whether directory `dir` has no entries (nor hard links).
    pub fn is_empty_dir(&self, dir: NodeId) -> Result<bool, FsError> {
        Ok(self.children(dir)?.is_empty() && !self.links.iter().any(|l| l.parent == dir))
    }

    /// Moves `id`'s first name to `name` in directory `parent`. An existing
    /// target is replaced only if `replace` and it is a file nobody has
    /// open.
    pub fn rename(
        &mut self,
        id: NodeId,
        parent: NodeId,
        name: &[u16],
        replace: bool,
    ) -> Result<(), FsError> {
        if self.is_root(id) {
            return Err(FsError::AccessDenied);
        }
        self.same_volume(id, parent)?;
        // A directory cannot move below itself.
        let mut up = parent;
        loop {
            if up == id {
                return Err(FsError::AccessDenied);
            }
            if self.is_root(up) {
                break;
            }
            up = self.node(up).ok_or(FsError::PathNotFound)?.parent;
        }
        let target = self.find(parent, name)?;
        if let Some(t) = target
            && t != id
        {
            if !replace {
                return Err(FsError::Exists);
            }
            let n = self.node(t).ok_or(FsError::NotFound)?;
            if n.is_dir() || n.opens.handles != 0 {
                return Err(FsError::AccessDenied);
            }
        }
        let mut owned = Vec::new();
        owned.try_extend_from_slice(name).map_err(|_| FsError::NoMemory)?;
        let old_parent = self.node(id).ok_or(FsError::NotFound)?.parent;
        if old_parent != parent {
            match &mut self.node_mut(parent)?.kind {
                Kind::Directory(c) => c.try_reserve(1).map_err(|_| FsError::NoMemory)?,
                Kind::File(_) | Kind::Symlink(_) | Kind::Socket => {
                    return Err(FsError::NotADirectory);
                }
            }
        }
        let from = if self.recording && self.logged(id) { self.path_of(id).ok() } else { None };
        if let Some(t) = target
            && t != id
        {
            self.drop_name(parent, name);
        }
        self.tombstone(id);
        if old_parent != parent {
            if let Kind::Directory(c) = &mut self.node_mut(old_parent)?.kind {
                c.retain(|&x| x != id);
            }
            if let Kind::Directory(c) = &mut self.node_mut(parent)?.kind {
                let _ = c.try_push(id);
            }
        }
        let node = self.node_mut(id)?;
        let old_name = core::mem::replace(&mut node.name, owned);
        node.parent = parent;
        let directory = node.is_dir();
        self.note(old_parent, &old_name, directory, ACTION_RENAMED_OLD_NAME);
        self.note(parent, name, directory, ACTION_RENAMED_NEW_NAME);
        if self.recording || self.replaying {
            // Everything below now lives at another path.
            self.mark_tree_dirty(id);
        }
        if let Some(from) = from {
            match self.path_of(id) {
                Ok(to) => self.push_op(Op::Rename { from, to }),
                Err(_) => self.ops_lost = true,
            }
        }
        Ok(())
    }

    /// Moves entry `from_name` of `from_dir` (a first name or a hard link)
    /// to `name` in `parent`, as `rename` does.
    pub fn rename_entry(
        &mut self,
        from_dir: NodeId,
        from_name: &[u16],
        parent: NodeId,
        name: &[u16],
        replace: bool,
    ) -> Result<(), FsError> {
        let id = self.find(from_dir, from_name)?.ok_or(FsError::NotFound)?;
        let Some(i) = self.link_at(from_dir, from_name) else {
            return self.rename(id, parent, name, replace);
        };
        self.children(parent)?;
        self.same_volume(id, parent)?;
        if let Some(t) = self.find(parent, name)?
            && t != id
        {
            if !replace {
                return Err(FsError::Exists);
            }
            let n = self.node(t).ok_or(FsError::NotFound)?;
            if n.is_dir() || n.opens.handles != 0 {
                return Err(FsError::AccessDenied);
            }
        }
        let owned = copy(name).ok_or(FsError::NoMemory)?;
        let paths = if self.recording && self.logged(parent) {
            Some((self.entry_path(from_dir, from_name), self.entry_path(parent, name)))
        } else {
            None
        };
        if self.find(parent, name)?.is_some_and(|t| t != id) {
            self.drop_name(parent, name);
        }
        // The target's removal may have moved the links.
        let i = self.link_at(from_dir, from_name).unwrap_or(i);
        let Some(link) = self.links.get_mut(i) else { return Err(FsError::NotFound) };
        let old = core::mem::replace(&mut link.name, owned);
        link.parent = parent;
        let directory = self.node(id).is_some_and(Node::is_dir);
        self.note(from_dir, &old, directory, ACTION_RENAMED_OLD_NAME);
        self.note(parent, name, directory, ACTION_RENAMED_NEW_NAME);
        match paths {
            Some((Ok(from), Ok(to))) => self.push_op(Op::Rename { from, to }),
            Some(_) => self.ops_lost = true,
            None => {}
        }
        Ok(())
    }

    /// Swaps entries `a_name` of `a_dir` and `b_name` of `b_dir` (Linux's
    /// RENAME_EXCHANGE): both must exist. Made of three renames through a
    /// free temporary name in `a_dir` (each recorded; atomic for every
    /// caller, as all run under the file system's lock).
    pub fn exchange(
        &mut self,
        a_dir: NodeId,
        a_name: &[u16],
        b_dir: NodeId,
        b_name: &[u16],
    ) -> Result<(), FsError> {
        let a = self.find(a_dir, a_name)?.ok_or(FsError::NotFound)?;
        let b = self.find(b_dir, b_name)?.ok_or(FsError::NotFound)?;
        if a == b {
            return Ok(());
        }
        let mut temp = Vec::new();
        temp.try_extend_from_slice(&[u16::from(b'.'), u16::from(b'~'), 0])
            .map_err(|_| FsError::NoMemory)?;
        let mut n = 0u16;
        loop {
            temp[2] = u16::from(b'0') + n;
            if self.find(a_dir, &temp)?.is_none() {
                break;
            }
            n += 1;
            if n > 1000 {
                return Err(FsError::Exists);
            }
        }
        self.rename_entry(a_dir, a_name, a_dir, &temp, false)?;
        if let Err(e) = self.rename_entry(b_dir, b_name, a_dir, a_name, false) {
            let _ = self.rename_entry(a_dir, &temp, a_dir, a_name, false);
            return Err(e);
        }
        self.rename_entry(a_dir, &temp, b_dir, b_name, false)
    }

    /// Removes the name `name` of directory `dir` without recording it:
    /// a hard link goes alone; a first name moves its node to a hard link,
    /// else the node goes with everything below it.
    fn drop_name(&mut self, dir: NodeId, name: &[u16]) {
        if let Some(i) = self.link_at(dir, name) {
            let link = self.links.remove(i);
            if let Ok(n) = self.node_mut(link.node) {
                n.extra_links -= 1;
            }
            self.note(dir, &link.name, false, ACTION_REMOVED);
            return;
        }
        if let Ok(Some(id)) = self.find(dir, name) {
            self.tombstone(id);
            self.remove_tree(id);
        }
    }

    /// Adds `name` in directory `parent` as another name of file (or
    /// symbolic link) `id`: a hard link.
    pub fn link(&mut self, id: NodeId, parent: NodeId, name: &[u16]) -> Result<(), FsError> {
        let node = self.node(id).ok_or(FsError::NotFound)?;
        if node.is_dir() {
            return Err(FsError::IsADirectory);
        }
        if node.detached {
            return Err(FsError::NotFound);
        }
        self.same_volume(id, parent)?;
        if self.find(parent, name)?.is_some() {
            return Err(FsError::Exists);
        }
        let owned = copy(name).ok_or(FsError::NoMemory)?;
        let paths = if self.recording && self.logged(parent) {
            Some((self.path_of(id), self.entry_path(parent, name)))
        } else {
            None
        };
        self.links
            .try_push(Link { parent, name: owned, node: id })
            .map_err(|_| FsError::NoMemory)?;
        self.node_mut(id)?.extra_links += 1;
        self.note(parent, name, false, ACTION_ADDED);
        match paths {
            Some((Ok(from), Ok(to))) => self.push_op(Op::Link { from, to }),
            Some(_) => self.ops_lost = true,
            None => {}
        }
        Ok(())
    }

    /// Removes the name `name` of directory `dir` at once (Linux's unlink
    /// and rmdir; POSIX delete): a hard link goes alone; a node's first
    /// name moves it to a hard link; a node with no other name goes, or,
    /// while open, lives on detached until its last close. A directory
    /// must be empty.
    pub fn unlink(&mut self, dir: NodeId, name: &[u16]) -> Result<(), FsError> {
        let id = self.find(dir, name)?.ok_or(FsError::NotFound)?;
        if self.is_root(id) {
            return Err(FsError::AccessDenied);
        }
        if self.node(id).is_some_and(Node::is_dir) && !self.is_empty_dir(id)? {
            return Err(FsError::DirectoryNotEmpty);
        }
        let path = if self.recording && self.logged(dir) {
            Some(self.entry_path(dir, name))
        } else {
            None
        };
        if let Some(i) = self.link_at(dir, name) {
            let link = self.links.remove(i);
            self.node_mut(id)?.extra_links -= 1;
            self.note(dir, &link.name, false, ACTION_REMOVED);
        } else if let Some(i) = self.links.iter().position(|l| l.node == id) {
            self.tombstone(id);
            self.promote(id, i)?;
        } else if self.node(id).is_some_and(|n| n.opens.handles > 0) {
            self.tombstone(id);
            let node = self.node_mut(id)?;
            node.detached = true;
            let (parent, directory) = (node.parent, node.is_dir());
            let old = core::mem::take(&mut node.name);
            if let Kind::Directory(c) = &mut self.node_mut(parent)?.kind {
                c.retain(|&x| x != id);
            }
            self.note(parent, &old, directory, ACTION_REMOVED);
            if let Ok(node) = self.node_mut(id) {
                node.name = old;
            }
        } else {
            self.tombstone(id);
            self.remove(id);
        }
        match path {
            Some(Ok(path)) => self.push_op(Op::Delete { path }),
            Some(Err(_)) => self.ops_lost = true,
            None => {}
        }
        Ok(())
    }

    /// Marks a node for deletion when its last open closes.
    pub fn set_delete_pending(&mut self, id: NodeId, pending: bool) -> Result<(), FsError> {
        if pending && self.is_root(id) {
            return Err(FsError::DirectoryNotEmpty);
        }
        if pending && self.children(id).is_ok() && !self.is_empty_dir(id)? {
            return Err(FsError::DirectoryNotEmpty);
        }
        self.node_mut(id)?.delete_pending = pending;
        Ok(())
    }

    /// Sets the POSIX permission bits of `id` (`Node::mode`).
    pub fn set_mode(&mut self, id: NodeId, mode: u16) -> Result<(), FsError> {
        let node = self.node_mut(id)?;
        // SEAL_EXEC keeps the execute bits as they are.
        let exec = 0o111;
        if node.seals.unwrap_or(0) & SEAL_EXEC != 0 && node.mode.unwrap_or(0) & exec != mode & exec
        {
            return Err(FsError::Sealed);
        }
        node.mode = Some(mode & 0o7777);
        self.record_node(id, |path, n| Some(Op::SetMode { path, mode: n.mode.unwrap_or(0) }));
        Ok(())
    }

    /// Adds `seals` (`SEAL_*`) to anonymous file `id`: NotSealable for
    /// another file, Sealed once SEAL_SEAL is on, WriteMapped for
    /// SEAL_WRITE while a section maps it writable (fcntl(2)'s EINVAL,
    /// EPERM, EBUSY). Unknown bits are the caller's to refuse.
    pub fn add_seals(&mut self, id: NodeId, seals: u32) -> Result<(), FsError> {
        let node = self.node_mut(id)?;
        let have = node.seals.ok_or(FsError::NotSealable)?;
        if have & SEAL_SEAL != 0 {
            return Err(FsError::Sealed);
        }
        if seals & SEAL_WRITE != 0 && have & SEAL_WRITE == 0 && node.write_maps > 0 {
            return Err(FsError::WriteMapped);
        }
        node.seals = Some(have | seals);
        Ok(())
    }

    /// Gives node `id` a reparse point (or replaces its own of the same
    /// tag), as NTFS's FSCTL_SET_REPARSE_POINT: a directory must be empty,
    /// a symbolic link's file must have no contents.
    pub fn set_reparse(&mut self, id: NodeId, tag: u32, data: &[u8]) -> Result<(), FsError> {
        let node = self.node(id).ok_or(FsError::NotFound)?;
        if node.reparse.as_ref().is_some_and(|r| r.tag != tag) {
            return Err(FsError::ReparseTagMismatch);
        }
        match &node.kind {
            Kind::Directory(children) if !children.is_empty() => {
                return Err(FsError::DirectoryNotEmpty);
            }
            Kind::Directory(_) => {}
            Kind::File(d) if tag == TAG_SYMLINK && d.len() != 0 => {
                return Err(FsError::ReparseHasData);
            }
            Kind::File(_) => {}
            Kind::Symlink(_) => return Err(FsError::Symlink),
            Kind::Socket => return Err(FsError::Socket),
        }
        if node.is_dir() && self.links.iter().any(|l| l.parent == id) {
            return Err(FsError::DirectoryNotEmpty);
        }
        let data = copy(data).ok_or(FsError::NoMemory)?;
        let node = self.node_mut(id)?;
        node.reparse = Some(Reparse { tag, data });
        node.attributes |= ATTRIBUTE_REPARSE_POINT;
        self.record_reparse(id);
        Ok(())
    }

    /// Removes node `id`'s reparse point of tag `tag`
    /// (FSCTL_DELETE_REPARSE_POINT).
    pub fn delete_reparse(&mut self, id: NodeId, tag: u32) -> Result<(), FsError> {
        let node = self.node_mut(id)?;
        match &node.reparse {
            None => return Err(FsError::NotReparsePoint),
            Some(r) if r.tag != tag => return Err(FsError::ReparseTagMismatch),
            Some(_) => {}
        }
        node.reparse = None;
        node.attributes &= !ATTRIBUTE_REPARSE_POINT;
        self.record_reparse(id);
        Ok(())
    }

    fn record_reparse(&mut self, id: NodeId) {
        self.record_node(id, |path, n| {
            let (tag, data) = match &n.reparse {
                Some(r) => (r.tag, copy(&r.data)?),
                None => (0, Vec::new()),
            };
            Some(Op::SetReparse { path, tag, data })
        });
    }

    pub fn delete_pending(&self, id: NodeId) -> bool {
        self.node(id).is_some_and(|n| n.delete_pending)
    }

    /// Reads at `offset`; returns the number of bytes read (0 at or past EOF).
    pub fn read(&self, id: NodeId, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let node = self.node(id).ok_or(FsError::NotFound)?;
        let data = match &node.kind {
            Kind::File(data) => data,
            Kind::Symlink(_) => return Err(FsError::Symlink),
            Kind::Socket => return Err(FsError::Socket),
            Kind::Directory(_) => return Err(FsError::IsADirectory),
        };
        let Ok(start) = usize::try_from(offset) else { return Ok(0) };
        data.read_at(start, buf).map_err(|index| self.missed(id, index))
    }

    /// Writes at `offset`, extending (zero-filling a gap) as needed.
    pub fn write(
        &mut self,
        id: NodeId,
        offset: u64,
        data: &[u8],
        now: u64,
    ) -> Result<usize, FsError> {
        let replaying = self.replaying;
        let node = self.node_mut(id)?;
        let seals = node.seals.unwrap_or(0);
        let contents = match &mut node.kind {
            Kind::File(contents) => contents,
            Kind::Symlink(_) => return Err(FsError::Symlink),
            Kind::Socket => return Err(FsError::Socket),
            Kind::Directory(_) => return Err(FsError::IsADirectory),
        };
        let start = usize::try_from(offset).map_err(|_| FsError::NoMemory)?;
        let end = start.checked_add(data.len()).ok_or(FsError::NoMemory)?;
        if seals & (SEAL_WRITE | SEAL_FUTURE_WRITE) != 0
            || (seals & SEAL_GROW != 0 && end > contents.len())
        {
            return Err(FsError::Sealed);
        }
        let pages = if data.is_empty() { 0..0 } else { start / PAGE..(end - 1) / PAGE + 1 };
        // A partial last page the write grows the file past (see set_size).
        let tail = Self::grown_tail(contents, end).filter(|t| !pages.contains(t) && !replaying);
        let tail_missing = tail.is_some_and(|t| contents.missing(t).is_some());
        let on_disk = self.on_disk(id);
        if on_disk {
            if let Some(t) = tail.filter(|_| tail_missing) {
                return Err(self.missed(id, t));
            }
            self.disk_write_check(id, start, end, pages.clone())?;
        }
        let node = self.node_mut(id)?;
        let Kind::File(contents) = &mut node.kind else { return Err(FsError::IsADirectory) };
        contents.write_at(start, data)?;
        let runs = if on_disk { self.disk_written(id, pages)? } else { Vec::new() };
        if let Some(t) = tail.filter(|_| on_disk) {
            self.page_dirty(id, t);
        }
        let node = self.node_mut(id)?;
        node.written = now;
        node.accessed = now;
        node.version = next_version();
        let (parent, name) = (node.parent, core::mem::take(&mut node.name));
        if !self.node(id).is_some_and(|n| n.detached) {
            self.note(parent, &name, false, ACTION_MODIFIED);
        }
        if let Ok(node) = self.node_mut(id) {
            node.name = name;
        }
        if on_disk {
            // The data goes to its clusters (writeback); the record says
            // where it is.
            self.record_node(id, |path, n| {
                Some(Op::Clusters { path, size: n.size(), written: now, runs })
            });
        } else if self.recording {
            for (i, chunk) in data.chunks(log::MAX_WRITE).enumerate() {
                let offset = offset + (i * log::MAX_WRITE) as u64;
                self.record_node(id, |path, _| {
                    Some(Op::Write { path, offset, data: copy(chunk)?, written: now })
                });
            }
        }
        self.mark_dirty(id);
        Ok(data.len())
    }

    /// FileBasicInformation: the times and attributes given (None keeps
    /// one). Attributes are the settable bits (read-only, hidden, system,
    /// archive...); the directory bit follows the node, and a file left
    /// with none reads as FILE_ATTRIBUTE_NORMAL's empty set.
    pub fn set_basic(
        &mut self,
        id: NodeId,
        times: Times,
        attributes: Option<u32>,
    ) -> Result<(), FsError> {
        let node = self.node_mut(id)?;
        if let Some(t) = times.created {
            node.created = t;
        }
        if let Some(t) = times.accessed {
            node.accessed = t;
        }
        if let Some(t) = times.written {
            node.written = t;
        }
        if let Some(a) = attributes {
            const SETTABLE: u32 = 0x01 | 0x02 | 0x04 | ATTRIBUTE_ARCHIVE | 0x100 | 0x2000;
            let kept = node.attributes & (ATTRIBUTE_DIRECTORY | ATTRIBUTE_REPARSE_POINT);
            node.attributes = (a & SETTABLE) | kept;
        }
        self.record_node(id, |path, n| {
            Some(Op::SetBasic {
                path,
                created: n.created,
                accessed: n.accessed,
                written: n.written,
                attributes: n.attributes,
            })
        });
        Ok(())
    }

    /// Truncates or extends (with zeros) to `size`.
    pub fn set_size(&mut self, id: NodeId, size: u64, now: u64) -> Result<(), FsError> {
        let replaying = self.replaying;
        let node = self.node_mut(id)?;
        let seals = node.seals.unwrap_or(0);
        let (mapped, mapped_size) = (node.map_count > 0, node.mapped_size);
        let contents = match &mut node.kind {
            Kind::File(contents) => contents,
            Kind::Symlink(_) => return Err(FsError::Symlink),
            Kind::Socket => return Err(FsError::Socket),
            Kind::Directory(_) => return Err(FsError::IsADirectory),
        };
        let len = contents.len() as u64;
        if (seals & SEAL_SHRINK != 0 && size < len) || (seals & SEAL_GROW != 0 && size > len) {
            return Err(FsError::Sealed);
        }
        if mapped && size < mapped_size {
            return Err(FsError::UserMapped);
        }
        let size = usize::try_from(size).map_err(|_| FsError::NoMemory)?;
        // The partial last page: what is past the end in its cluster may
        // be data cut off earlier (zeroed only when it is read back:
        // `fill`), so growing past it needs it in memory first.
        // Replayed, the disk held each page as the record found it (the
        // page was written before the record), so nothing is read then.
        let tail = Self::grown_tail(contents, size).filter(|_| !replaying);
        if let Some(t) = tail
            && contents.missing(t).is_some()
        {
            return Err(self.missed(id, t));
        }
        let cut = (size < contents.len() && !size.is_multiple_of(PAGE)).then_some(size / PAGE);
        let mut freed = Vec::new();
        let node = self.node_mut(id)?;
        let Kind::File(contents) = &mut node.kind else { return Err(FsError::IsADirectory) };
        contents.set_len(size, &mut freed)?;
        // A page whose cluster holds bytes now past (or newly inside) the
        // end goes to the disk again, cleared.
        let redo = [tail, cut]
            .map(|t| t.filter(|&t| contents.slot(t).cluster().is_some() && contents.resident(t)));
        for t in redo.into_iter().flatten() {
            self.page_dirty(id, t);
        }
        self.give_up(id, &freed);
        let node = self.node_mut(id)?;
        node.written = now;
        node.accessed = now;
        node.version = next_version();
        let (parent, name) = (node.parent, core::mem::take(&mut node.name));
        if !self.node(id).is_some_and(|n| n.detached) {
            self.note(parent, &name, false, ACTION_MODIFIED);
        }
        if let Ok(node) = self.node_mut(id) {
            node.name = name;
        }
        let size = size as u64;
        self.record_node(id, |path, _| Some(Op::SetSize { path, size, written: now }));
        Ok(())
    }

    /// Registers a section mapping the first `size` bytes of file `id`
    /// (the file must be that large): its pages stay put and the file
    /// cannot shrink below `size` until `unmap`. A `writable` one voids the
    /// version until it ends (`Node::version`).
    pub fn map(&mut self, id: NodeId, size: u64, writable: bool) -> Result<(), FsError> {
        let node = self.node_mut(id)?;
        if writable && node.seals.unwrap_or(0) & (SEAL_WRITE | SEAL_FUTURE_WRITE) != 0 {
            return Err(FsError::Sealed);
        }
        let contents = match &mut node.kind {
            Kind::File(contents) => contents,
            Kind::Symlink(_) => return Err(FsError::Symlink),
            Kind::Socket => return Err(FsError::Socket),
            Kind::Directory(_) => return Err(FsError::IsADirectory),
        };
        if size > contents.len() as u64 {
            return Err(FsError::NoMemory);
        }
        contents.paged()?;
        node.map_count += 1;
        if writable {
            node.write_maps += 1;
            node.version = next_version();
        }
        node.mapped_size = node.mapped_size.max(size);
        Ok(())
    }

    /// Ends a mapping registered with `map` (as `writable` as it was): a
    /// writable one may have changed the contents, so they get a new
    /// version.
    pub fn unmap(&mut self, id: NodeId, writable: bool) {
        if let Ok(node) = self.node_mut(id) {
            node.map_count = node.map_count.saturating_sub(1);
            if writable {
                node.write_maps = node.write_maps.saturating_sub(1);
                node.version = next_version();
            }
            if node.map_count == 0 {
                node.mapped_size = 0;
            }
        }
        // What its views changed was marked dirty as each ended
        // (`view_ended`).
    }

    /// A view of a section on file `id` showing `bytes` of it begins: the
    /// pages it shows stay in memory until it ends (a view's page tables
    /// show them by frame).
    pub fn view_started(&mut self, id: NodeId, bytes: core::ops::Range<u64>) {
        let pages = page_span(&bytes);
        if let Ok(node) = self.node_mut(id)
            && node.views.try_push(pages).is_err()
        {
            node.views_unlisted += 1;
        }
    }

    /// Whether a live view may show page `i` of `node`.
    fn viewed(node: &Node, i: usize) -> bool {
        node.views_unlisted > 0 || node.views.iter().any(|&(a, b)| (a..b).contains(&i))
    }

    /// A view of a section on file `id` ends; one that could write may
    /// have changed any page it showed, so they are written to the disk
    /// before they may leave memory.
    pub fn view_ended(&mut self, id: NodeId, writable: bool, bytes: core::ops::Range<u64>) {
        let pages = page_span(&bytes);
        if let Ok(node) = self.node_mut(id) {
            match node.views.iter().position(|&v| v == pages) {
                Some(i) => {
                    node.views.swap_remove(i);
                }
                None => node.views_unlisted = node.views_unlisted.saturating_sub(1),
            }
        }
        if writable {
            self.dirty_resident_in(id, pages.0..pages.1);
        }
    }

    /// Marks the pages of disk-backed `id` in `range` that are in memory
    /// dirty.
    fn dirty_resident_in(&mut self, id: NodeId, range: core::ops::Range<usize>) {
        if self.on_disk(id) {
            let resident = match self.node(id).map(|n| &n.kind) {
                Some(Kind::File(Data::Paged { pages, .. })) => pages.len(),
                _ => 0,
            };
            for i in range.start..range.end.min(resident) {
                if self.node(id).is_some_and(|n| matches!(&n.kind, Kind::File(d) if d.resident(i)))
                {
                    self.page_dirty(id, i);
                }
            }
        }
    }

    /// The address of page `index` of mapped file `id` (see `map`):
    /// NotResident while it is on the disk only, NoMemory if it cannot be
    /// made.
    pub fn page_address(&mut self, id: NodeId, index: usize) -> Result<usize, FsError> {
        let data = match &mut self.node_mut(id)?.kind {
            Kind::File(data) => data,
            Kind::Directory(_) | Kind::Symlink(_) | Kind::Socket => return Err(FsError::NotFound),
        };
        if data.missing(index).is_some() {
            return Err(self.missed(id, index));
        }
        data.page_address(index).ok_or(FsError::NoMemory)
    }

    // ---- The change log (fs::log) ----

    fn push_op(&mut self, op: Op) {
        // Consecutive Clusters records of one file merge (a large write
        // comes in pieces): the later size and time, the runs of both.
        if let (Op::Clusters { path, size, written, runs }, Some(Op::Clusters { path: last, .. })) =
            (&op, self.ops.last())
            && last == path
            && let Some(Op::Clusters { size: s, written: w, runs: r, .. }) = self.ops.last_mut()
            && r.len() + runs.len() <= log::MAX_RUNS
            && r.try_reserve(runs.len()).is_ok()
        {
            (*s, *w) = (*size, *written);
            for &run in runs {
                match r.last_mut() {
                    Some(l)
                        if l.page + u64::from(l.count) == run.page
                            && l.cluster + l.count == run.cluster =>
                    {
                        l.count += run.count;
                    }
                    // Room was reserved above.
                    _ => {
                        let _ = r.try_push(run);
                    }
                }
            }
            return;
        }
        if self.ops.try_push(op).is_err() {
            self.ops_lost = true;
        }
    }

    fn mark_dirty(&mut self, id: NodeId) {
        if (self.recording || self.replaying)
            && let Ok(n) = self.node_mut(id)
        {
            n.dirty = true;
        }
    }

    fn mark_tree_dirty(&mut self, id: NodeId) {
        let mut stack = Vec::new();
        if stack.try_push(id).is_err() {
            self.ops_lost = true;
            return;
        }
        while let Some(at) = stack.pop() {
            let Ok(n) = self.node_mut(at) else { continue };
            n.dirty = true;
            if let Kind::Directory(children) = &n.kind
                && stack.try_extend_from_slice(children).is_err()
            {
                self.ops_lost = true;
            }
        }
    }

    /// Records a change of node `id` (marking it dirty): `make` builds it
    /// from the node's path and the node (None: out of memory).
    fn record_node(&mut self, id: NodeId, make: impl FnOnce(Vec<u16>, &Node) -> Option<Op>) {
        if self.node(id).is_some_and(|n| n.detached) {
            // Unlinked: its path names something else now, or nothing.
            return;
        }
        if self.node(id).is_some_and(Node::is_socket) || !self.logged(id) {
            // Not kept across reboots (the module's doc).
            return;
        }
        self.mark_dirty(id);
        if !self.recording {
            return;
        }
        let op = match (self.path_of(id), self.node(id)) {
            (Ok(path), Some(n)) => make(path, n),
            _ => None,
        };
        match op {
            Some(op) => self.push_op(op),
            None => self.ops_lost = true,
        }
    }

    /// Remembers that boot node `id` (and what is below it) no longer has
    /// its boot path.
    fn tombstone(&mut self, id: NodeId) {
        if !(self.recording || self.replaying) || !self.node(id).is_some_and(|n| n.boot) {
            return;
        }
        match self.path_of(id) {
            Ok(path) => {
                if !self.tombstones.iter().any(|t| same_path(t, &path))
                    && self.tombstones.try_push(path).is_err()
                {
                    self.ops_lost = true;
                }
            }
            Err(_) => self.ops_lost = true,
        }
    }

    /// Records the removal of `id` (before it goes).
    fn record_removal(&mut self, id: NodeId) {
        self.tombstone(id);
        if self.recording && self.logged(id) {
            match self.path_of(id) {
                Ok(path) => self.push_op(Op::Delete { path }),
                Err(_) => self.ops_lost = true,
            }
        }
    }

    /// The changes recorded since the last call, and whether some could
    /// not be (then only a `snapshot` describes the changes).
    pub fn take_ops(&mut self) -> (Vec<Op>, bool) {
        let lost = core::mem::take(&mut self.ops_lost);
        (core::mem::take(&mut self.ops), lost)
    }

    /// Forgets the recorded changes: only a snapshot will describe them
    /// (the log could not take them).
    pub fn lose_ops(&mut self) {
        self.ops = Vec::new();
        self.ops_lost = true;
    }

    /// Puts back changes taken with `take_ops` that did not reach the
    /// disk, ahead of any recorded since (`lost`: they were incomplete,
    /// only a snapshot will do). Without memory to put them back, only a
    /// snapshot describes them.
    pub fn restore_ops(&mut self, mut ops: Vec<Op>, lost: bool) {
        if lost {
            self.lose_ops();
            return;
        }
        if ops.try_reserve(self.ops.len()).is_err() {
            self.lose_ops();
            return;
        }
        ops.append(&mut self.ops);
        self.ops = ops;
    }

    pub fn has_ops(&self) -> bool {
        !self.ops.is_empty()
            || self.ops_lost
            || self.area.as_ref().is_some_and(|a| !a.dirty.is_empty() || a.dirty_lost)
    }

    /// Removes `id`'s first name and, for a directory, everything below
    /// it (a file below with a hard link elsewhere moves there).
    fn remove_tree(&mut self, id: NodeId) {
        // Children first; the list is copied (fallibly) as removing edits it.
        let children = self.children(id).ok().and_then(copy);
        for c in children.unwrap_or_default() {
            self.remove_tree(c);
        }
        while let Some(i) = self.links.iter().position(|l| l.parent == id) {
            let link = self.links.remove(i);
            if let Ok(n) = self.node_mut(link.node) {
                n.extra_links -= 1;
            }
        }
        self.remove(id);
    }

    /// The directory holding `path` (`\a\b`) and the last name.
    fn split<'p>(&self, path: &'p [u16]) -> Result<(NodeId, &'p [u16]), FsError> {
        let at = path.iter().rposition(|&u| u == u16::from(b'\\')).ok_or(FsError::NotFound)?;
        let (dir, name) = (&path[..at], &path[at + 1..]);
        if name.is_empty() {
            return Err(FsError::NotFound);
        }
        Ok((self.lookup(dir)?, name))
    }

    /// The node at `path` (`\a\b`).
    fn lookup(&self, path: &[u16]) -> Result<NodeId, FsError> {
        let mut at = ROOT;
        for part in path.split(|&u| u == u16::from(b'\\')).filter(|p| !p.is_empty()) {
            at = self.find(at, part)?.ok_or(FsError::NotFound)?;
        }
        Ok(at)
    }

    /// The parent directory of `path` (made if missing) and the last name.
    fn make_parent<'p>(
        &mut self,
        path: &'p [u16],
        now: u64,
    ) -> Result<(NodeId, &'p [u16]), FsError> {
        let parts: Vec<&[u16]> = {
            let mut v = Vec::new();
            for part in path.split(|&u| u == u16::from(b'\\')).filter(|p| !p.is_empty()) {
                v.try_push(part).map_err(|_| FsError::NoMemory)?;
            }
            v
        };
        let (last, dirs) = parts.split_last().ok_or(FsError::AccessDenied)?;
        Ok((self.mkdir_all(dirs, now)?, last))
    }

    /// Replays one change from the log (marking what it touches dirty, not
    /// recording it). A change whose path is missing is skipped.
    pub fn apply(&mut self, op: &Op) -> Result<(), FsError> {
        let was = core::mem::replace(&mut self.recording, false);
        self.replaying = true;
        let result = self.apply_inner(op);
        self.replaying = false;
        self.recording = was;
        match result {
            Err(FsError::NotFound | FsError::PathNotFound) => Ok(()),
            other => other,
        }
    }

    fn apply_inner(&mut self, op: &Op) -> Result<(), FsError> {
        match op {
            Op::Create { path, directory, attributes, created, written, mode, case_sensitive } => {
                let (parent, name) = self.make_parent(path, *created)?;
                let id = match self.find(parent, name)? {
                    Some(id) if self.node(id).is_some_and(|n| n.is_dir()) == *directory => {
                        if !directory {
                            self.set_size(id, 0, *written)?;
                        }
                        id
                    }
                    other => {
                        if other.is_some() {
                            self.drop_name(parent, name);
                        }
                        self.create(parent, name, *directory, *created)?
                    }
                };
                let node = self.node_mut(id)?;
                node.mode = *mode;
                node.case_sensitive = *directory && *case_sensitive;
                // A record's file was last accessed when it was written.
                let times = Times::all(*created, *written, *written);
                self.set_basic(id, times, Some(*attributes))
            }
            Op::SetMode { path, mode } => {
                let id = self.lookup(path)?;
                self.set_mode(id, *mode)
            }
            Op::Symlink { path, target, created } => {
                let (parent, name) = self.make_parent(path, *created)?;
                if self.find(parent, name)?.is_some() {
                    self.drop_name(parent, name);
                }
                self.symlink(parent, name, target, *created).map(drop)
            }
            Op::Link { from, to } => {
                let id = self.lookup(from)?;
                let (parent, name) = self.make_parent(to, 0)?;
                if self.find(parent, name)?.is_some_and(|t| t != id) {
                    self.drop_name(parent, name);
                }
                if self.find(parent, name)?.is_some() {
                    return Ok(());
                }
                self.link(id, parent, name)
            }
            Op::Write { path, offset, data, written } => {
                let id = self.lookup(path)?;
                self.write(id, *offset, data, *written).map(drop)
            }
            Op::SetSize { path, size, written } => {
                let id = self.lookup(path)?;
                self.set_size(id, *size, *written)
            }
            Op::SetBasic { path, created, accessed, written, attributes } => {
                let id = self.lookup(path)?;
                self.set_basic(id, Times::all(*created, *accessed, *written), Some(*attributes))
            }
            Op::SetSecurity { path, descriptor } => {
                let id = self.lookup(path)?;
                let sd = copy(descriptor).ok_or(FsError::NoMemory)?;
                self.set_security(id, sd)
            }
            Op::Rename { from, to } => {
                let (from_dir, from_name) = self.split(from)?;
                let id = self.find(from_dir, from_name)?.ok_or(FsError::NotFound)?;
                let (parent, name) = self.make_parent(to, 0)?;
                if self.find(parent, name)?.is_some_and(|t| t != id) {
                    self.drop_name(parent, name);
                }
                self.rename_entry(from_dir, from_name, parent, name, true)
            }
            Op::Delete { path } => {
                let (dir, name) = self.split(path)?;
                if self.find(dir, name)?.is_none() {
                    return Err(FsError::NotFound);
                }
                self.drop_name(dir, name);
                Ok(())
            }
            Op::Clusters { path, size, written, runs } => {
                let id = self.lookup(path)?;
                self.set_clusters(id, *size, *written, runs)
            }
            Op::SetReparse { path, tag, data } => {
                let id = self.lookup(path)?;
                match self.node(id).and_then(|n| n.reparse.as_ref()).map(|r| r.tag) {
                    Some(have) if *tag == 0 => self.delete_reparse(id, have),
                    _ if *tag == 0 => Ok(()),
                    Some(have) if have != *tag => {
                        self.delete_reparse(id, have)?;
                        self.set_reparse(id, *tag, data)
                    }
                    _ => self.set_reparse(id, *tag, data),
                }
            }
        }
    }

    /// The changes since boot as a fresh log: the boot paths that are
    /// gone, then every dirty node (parents first) with its contents.
    pub fn snapshot(&self) -> Result<Vec<Op>, FsError> {
        let mut ops = Vec::new();
        let mut push = |op: Op| ops.try_push(op).map_err(|_| FsError::NoMemory);
        for t in &self.tombstones {
            push(Op::Delete { path: copy(t).ok_or(FsError::NoMemory)? })?;
        }
        let mut stack = Vec::new();
        stack.try_push(ROOT).map_err(|_| FsError::NoMemory)?;
        while let Some(id) = stack.pop() {
            let n = self.node(id).ok_or(FsError::NotFound)?;
            if n.dirty && id != ROOT && !n.is_socket() {
                let path = self.path_of(id)?;
                let p = || copy(&path).ok_or(FsError::NoMemory);
                if let Kind::Symlink(target) = &n.kind {
                    let target = copy(target).ok_or(FsError::NoMemory)?;
                    push(Op::Symlink { path: p()?, target, created: n.created })?;
                    continue;
                }
                push(Op::Create {
                    path: p()?,
                    directory: n.is_dir(),
                    attributes: n.attributes,
                    created: n.created,
                    written: n.written,
                    mode: n.mode,
                    case_sensitive: n.case_sensitive,
                })?;
                if let Some(sd) = &n.security {
                    let descriptor = copy(sd).ok_or(FsError::NoMemory)?;
                    push(Op::SetSecurity { path: p()?, descriptor })?;
                }
                // Before the contents (a symbolic link has none) and the
                // children (none either: a reparse point's directory is
                // empty).
                if let Some(r) = &n.reparse {
                    let data = copy(&r.data).ok_or(FsError::NoMemory)?;
                    push(Op::SetReparse { path: p()?, tag: r.tag, data })?;
                }
                if let Kind::File(data) = &n.kind {
                    let len = data.len();
                    // On a data area: the pages in clusters as Clusters
                    // records, the rest (a boot module's, a dirty page's
                    // data) as Write records; else all of it as Writes.
                    let area = self.area.is_some() || data.clusters().next().is_some();
                    let in_cluster = |i: usize| {
                        let slot = data.slot(i);
                        area && slot.stored && !slot.dirty && slot.cluster().is_some()
                    };
                    let mut at = 0;
                    while at < len {
                        if in_cluster(at / PAGE) {
                            at = (at / PAGE + 1) * PAGE;
                            continue;
                        }
                        // Up to MAX_WRITE, stopping at a page in a cluster.
                        let mut k = (len - at).min(log::MAX_WRITE);
                        let mut next = (at / PAGE + 1) * PAGE;
                        while next < at + k {
                            if in_cluster(next / PAGE) {
                                k = next - at;
                                break;
                            }
                            next += PAGE;
                        }
                        let mut chunk = Vec::new();
                        chunk.try_reserve_exact(k).map_err(|_| FsError::NoMemory)?;
                        chunk.resize(k, 0);
                        // Not in a cluster: in memory or the backing's.
                        data.read_at(at, &mut chunk).map_err(|_| FsError::NotResident)?;
                        push(Op::Write {
                            path: p()?,
                            offset: at as u64,
                            data: chunk,
                            written: n.written,
                        })?;
                        at += k;
                    }
                    if area {
                        let mut runs: Vec<Run> = Vec::new();
                        for i in 0..len.div_ceil(PAGE) {
                            if !in_cluster(i) {
                                continue;
                            }
                            let c = data.slot(i).cluster().unwrap_or(0);
                            match runs.last_mut() {
                                Some(r)
                                    if r.page + u64::from(r.count) == i as u64
                                        && r.cluster + r.count == c =>
                                {
                                    r.count += 1;
                                }
                                _ => {
                                    if runs.len() == log::MAX_RUNS {
                                        let full = core::mem::take(&mut runs);
                                        push(Op::Clusters {
                                            path: p()?,
                                            size: len as u64,
                                            written: n.written,
                                            runs: full,
                                        })?;
                                    }
                                    runs.try_push(Run { page: i as u64, cluster: c, count: 1 })
                                        .map_err(|_| FsError::NoMemory)?;
                                }
                            }
                        }
                        push(Op::Clusters {
                            path: p()?,
                            size: len as u64,
                            written: n.written,
                            runs,
                        })?;
                    }
                }
                // Writes set the times; the node's own come last (a
                // directory's access time is its own too).
                if !n.is_dir() || n.accessed != n.written {
                    push(Op::SetBasic {
                        path: p()?,
                        created: n.created,
                        accessed: n.accessed,
                        written: n.written,
                        attributes: n.attributes,
                    })?;
                }
            }
            if let Kind::Directory(children) = &n.kind {
                // Reversed: the stack pops them in order.
                for &c in children.iter().rev() {
                    stack.try_push(c).map_err(|_| FsError::NoMemory)?;
                }
            }
        }
        // Hard links last: every first name exists by then.
        for l in &self.links {
            push(Op::Link {
                from: self.path_of(l.node)?,
                to: self.entry_path(l.parent, &l.name)?,
            })?;
        }
        Ok(ops)
    }

    /// Creates the directories of `path` (components) that do not exist.
    pub fn mkdir_all(&mut self, components: &[&[u16]], now: u64) -> Result<NodeId, FsError> {
        let mut dir = ROOT;
        for part in components {
            dir = match self.find(dir, part)? {
                Some(next) => next,
                None => self.create(dir, part, true, now)?,
            };
        }
        Ok(dir)
    }
}

// ---- A disk-backed C: (D4): pages in the data area's clusters ----

impl RamFs {
    /// Whether file `id`'s pages live in the data area: one is attached,
    /// and the file is in the root's tree (not a Linux volume's) and not
    /// an anonymous one.
    fn on_disk(&self, id: NodeId) -> bool {
        self.area.is_some()
            && self.node(id).is_some_and(|n| {
                n.volume == ROOT && n.seals.is_none() && matches!(n.kind, Kind::File(_))
            })
    }

    /// Notes that page `index` of `id` must be read from the disk first;
    /// the error that says so.
    fn missed(&self, id: NodeId, index: usize) -> FsError {
        let req = self.node(id).and_then(|n| match &n.kind {
            Kind::File(d) => Some(PageIn {
                node: id,
                index,
                cluster: d.missing(index)?,
                serial: n.serial,
                epoch: n.epoch,
            }),
            _ => None,
        });
        self.miss.set(req);
        FsError::NotResident
    }

    /// The page the last NotResident was about (taken once).
    pub fn take_page_in(&self) -> Option<PageIn> {
        self.miss.take()
    }

    /// Installs page `req` read from its cluster if it is still wanted:
    /// the same node (not a reused id), nothing left memory or gave up a
    /// cluster since the read began, and the page is still on the disk
    /// only. Bytes past the file's end are cleared.
    pub fn fill(&mut self, req: &PageIn, mut page: Box<Page>) -> bool {
        let Ok(node) = self.node_mut(req.node) else { return false };
        if node.serial != req.serial || node.epoch != req.epoch {
            return false;
        }
        let Kind::File(data) = &mut node.kind else { return false };
        if data.missing(req.index) != Some(req.cluster) {
            return false;
        }
        let len = data.len();
        let at = req.index * PAGE;
        if len < at + PAGE {
            page.0[len.saturating_sub(at).min(PAGE)..].fill(0);
        }
        let Data::Paged { pages, .. } = data else { return false };
        let Some(slot) = pages.get_mut(req.index) else { return false };
        *slot = Some(page);
        self.note_clean(req.node, req.index);
        true
    }

    /// Before a write of [start, end) to a disk-backed file: the pages it
    /// changes only in part must be in memory, and there must be a cluster
    /// for each page that has none (Windows fails the whole write then:
    /// STATUS_DISK_FULL).
    fn disk_write_check(
        &self,
        id: NodeId,
        start: usize,
        end: usize,
        pages: core::ops::Range<usize>,
    ) -> Result<(), FsError> {
        let Some(Kind::File(data)) = self.node(id).map(|n| &n.kind) else {
            return Err(FsError::NotFound);
        };
        let mut need = 0u64;
        for i in pages {
            let whole = i * PAGE >= start && (i + 1) * PAGE <= end;
            if !whole && data.missing(i).is_some() {
                return Err(self.missed(id, i));
            }
            if data.slot(i).cluster().is_none() {
                need += 1;
            }
        }
        let free = self.area.as_ref().map_or(0, |a| a.clusters.free());
        if need > u64::from(free) { Err(FsError::DiskFull) } else { Ok(()) }
    }

    /// After a write to disk-backed `id`: a cluster for each written page
    /// without one, every written page dirty. The runs newly given.
    fn disk_written(
        &mut self,
        id: NodeId,
        pages: core::ops::Range<usize>,
    ) -> Result<Vec<Run>, FsError> {
        let mut runs: Vec<Run> = Vec::new();
        for i in pages {
            let has = match &self.node(id).ok_or(FsError::NotFound)?.kind {
                Kind::File(d) => d.slot(i).cluster().is_some(),
                _ => return Err(FsError::NotFound),
            };
            if !has {
                let c = self.give_cluster(id, i)?;
                match runs.last_mut() {
                    Some(r)
                        if r.page + u64::from(r.count) == i as u64 && r.cluster + r.count == c =>
                    {
                        r.count += 1;
                    }
                    _ => runs.try_push(Run { page: i as u64, cluster: c, count: 1 }).map_err(
                        |_| {
                            self.ops_lost = true;
                            FsError::NoMemory
                        },
                    )?,
                }
            }
            self.page_dirty(id, i);
        }
        Ok(runs)
    }

    /// The partial last page of `data` if growing it to `size` moves the
    /// end past it.
    fn grown_tail(data: &Data, size: usize) -> Option<usize> {
        let len = data.len();
        (size > len && !len.is_multiple_of(PAGE)).then_some(len / PAGE)
    }

    /// Whether a write of `len` bytes at `offset` to file `id` can have the
    /// clusters it needs (DiskFull if not; Ok for files not on the disk).
    pub fn room_for(&self, id: NodeId, offset: u64, len: u64) -> Result<(), FsError> {
        if len == 0 || !self.on_disk(id) {
            return Ok(());
        }
        let Some(Kind::File(data)) = self.node(id).map(|n| &n.kind) else { return Ok(()) };
        let end = offset.checked_add(len).ok_or(FsError::DiskFull)?;
        let (first, last) = (offset / PAGE as u64, (end - 1) / PAGE as u64);
        let mut need = 0u64;
        for i in first..=last {
            let Ok(i) = usize::try_from(i) else { return Err(FsError::DiskFull) };
            if data.slot(i).cluster().is_none() {
                need += 1;
            }
        }
        let free = self.area.as_ref().map_or(0, |a| a.clusters.free());
        if need > u64::from(free) { Err(FsError::DiskFull) } else { Ok(()) }
    }

    /// A cluster for page `i` of `id`, now its slot's (not yet stored).
    fn give_cluster(&mut self, id: NodeId, i: usize) -> Result<u32, FsError> {
        let area = self.area.as_mut().ok_or(FsError::DiskFull)?;
        let c = area.clusters.alloc().ok_or(FsError::DiskFull)?;
        let given = match &mut self.nodes.get_mut(id).and_then(Option::as_mut).map(|n| &mut n.kind)
        {
            Some(Kind::File(d)) => d.slot_mut(i).map(|slot| {
                *slot = Slot { cluster: c + 1, dirty: false, stored: false, recent: false };
            }),
            _ => Err(FsError::NotFound),
        };
        if let Err(e) = given {
            if let Some(a) = self.area.as_mut() {
                a.clusters.undo(c);
            }
            return Err(e);
        }
        Ok(c)
    }

    /// Marks page `i` of disk-backed `id` changed in memory (written to
    /// its cluster at the next sync).
    fn page_dirty(&mut self, id: NodeId, i: usize) {
        let Some(area) = self.area.as_mut() else { return };
        let Some(Kind::File(d)) =
            self.nodes.get_mut(id).and_then(Option::as_mut).map(|n| &mut n.kind)
        else {
            return;
        };
        let Ok(slot) = d.slot_mut(i) else {
            area.dirty_lost = true;
            return;
        };
        if !slot.dirty {
            slot.dirty = true;
            if area.dirty.try_push((id, i)).is_err() {
                area.dirty_lost = true;
            }
        }
    }

    /// A page now in memory and on the disk alike: it may leave memory
    /// once `evict`'s hand has passed it (no memory needed: the hand finds
    /// it in the file's slots).
    fn note_clean(&mut self, id: NodeId, i: usize) {
        if self.area.is_none() {
            return;
        }
        if let Ok(Kind::File(d)) = self.node_mut(id).map(|n| &mut n.kind)
            && let Ok(slot) = d.slot_mut(i)
        {
            slot.recent = true;
        }
    }

    /// Gives up clusters `freed` of `id` (truncated pages): free once the
    /// records saying so are on the disk.
    fn give_up(&mut self, id: NodeId, freed: &[u32]) {
        if freed.is_empty() {
            return;
        }
        if let Ok(n) = self.node_mut(id) {
            n.epoch = next_version();
        }
        if let Some(area) = self.area.as_mut() {
            for &c in freed {
                area.clusters.give_up(c);
            }
        }
    }

    /// A node that goes: its clusters are given up.
    fn release(&mut self, node: &Node) {
        if let (Some(area), Kind::File(d)) = (self.area.as_mut(), &node.kind) {
            for c in d.clusters() {
                area.clusters.give_up(c);
            }
        }
    }

    /// Replays a Clusters record: the size, then the runs' pages in their
    /// clusters (stored there; a copy in memory goes).
    fn set_clusters(
        &mut self,
        id: NodeId,
        size: u64,
        written: u64,
        runs: &[Run],
    ) -> Result<(), FsError> {
        self.set_size(id, size, written)?;
        let size = usize::try_from(size).map_err(|_| FsError::NoMemory)?;
        let mut freed = Vec::new();
        {
            let node = self.node_mut(id)?;
            let Kind::File(data) = &mut node.kind else { return Err(FsError::IsADirectory) };
            for r in runs {
                for k in 0..r.count {
                    let Ok(i) = usize::try_from(r.page + u64::from(k)) else { break };
                    if i * PAGE >= size {
                        break;
                    }
                    let slot = data.slot_mut(i)?;
                    if let Some(old) = slot.cluster()
                        && old != r.cluster + k
                    {
                        let _ = freed.try_push(old);
                    }
                    *slot = Slot {
                        cluster: r.cluster + k + 1,
                        dirty: false,
                        stored: true,
                        recent: false,
                    };
                    if let Data::Paged { pages, .. } = data
                        && let Some(p) = pages.get_mut(i)
                    {
                        *p = None;
                    }
                }
            }
            node.written = written;
        }
        self.give_up(id, &freed);
        Ok(())
    }

    /// Attaches a data area of `count` clusters after the log is replayed
    /// (`nt::persist::mount`): the clusters the files use are taken (a
    /// cluster named twice, or out of range, is dropped from the later
    /// file: its page reads as zeros), and pages in memory without one
    /// (written by replayed Write records) become dirty, to be given one
    /// at the next sync. Returns (clusters in use, mappings dropped).
    pub fn attach_area(&mut self, count: u32) -> Result<(u32, u32), FsError> {
        let mut clusters = Clusters::new(count).ok_or(FsError::NoMemory)?;
        let mut dropped = 0;
        let mut dirty = Vec::new();
        let mut dirty_lost = false;
        for (id, slot) in self.nodes.iter_mut().enumerate() {
            let Some(n) = slot.as_mut() else { continue };
            if n.volume != ROOT || n.seals.is_some() {
                continue;
            }
            let Kind::File(Data::Paged { pages, disk, .. }) = &mut n.kind else { continue };
            for s in disk.iter_mut() {
                if let Some(c) = s.cluster()
                    && !clusters.claim(c)
                {
                    *s = Slot::default();
                    dropped += 1;
                }
            }
            for (i, p) in pages.iter().enumerate() {
                let has = disk.get(i).is_some_and(|s| s.cluster().is_some());
                if p.is_some() && !has {
                    if disk.len() <= i {
                        if disk.try_reserve(pages.len() - disk.len()).is_err() {
                            dirty_lost = true;
                            continue;
                        }
                        disk.resize(pages.len(), Slot::default());
                    }
                    disk[i].dirty = true;
                    if dirty.try_push((id, i)).is_err() {
                        dirty_lost = true;
                    }
                }
            }
        }
        let used = count - clusters.free();
        self.area = Some(Area { clusters, dirty, dirty_lost, hand: (0, 0), unbacked: 0 });
        Ok((used, dropped))
    }

    /// The data area's clusters and how many are free, once attached.
    pub fn area_size(&self) -> Option<(u32, u32)> {
        self.area.as_ref().map(|a| (a.clusters.count(), a.clusters.free()))
    }

    /// Dirty pages left without a cluster (the area was full) and
    /// clusters lost to a full pending list, for the sync's report.
    pub fn area_trouble(&self) -> (u64, u32) {
        self.area.as_ref().map_or((0, 0), |a| (a.unbacked, a.clusters.leaked))
    }

    /// Dirty pages to write to their clusters, as many as `out`'s capacity
    /// and `staging` (whole pages) take: listed in `out` by cluster, each
    /// copied into `staging` at its place in the list, and marked clean
    /// and stored (it may leave memory once `evict` runs, which the sync
    /// does only after writing them). A dirty page without a cluster gets
    /// one here and its record (a page changed through a mapping, or
    /// replayed); with none free it stays dirty in memory. Allocates
    /// nothing (the caller reserved both buffers): a sync short of memory
    /// still writes what lets memory go. Whether dirty pages remain.
    pub fn writeback(&mut self, out: &mut Vec<Writeback>, staging: &mut [u8]) -> bool {
        out.clear();
        let cap = out.capacity().min(staging.len() / PAGE);
        // Pages that found no cluster go back to the list's front; at most
        // the list's length of them is looked at again.
        let mut stuck = 0usize;
        while out.len() < cap {
            let Some(area) = self.area.as_mut() else { break };
            if stuck >= area.dirty.len() {
                break;
            }
            let Some((id, i)) = area.dirty.pop() else { break };
            if !self.collect(out, id, i)
                && let Some(a) = self.area.as_mut()
            {
                // To the list's front (the pop left room for it).
                if a.dirty.try_push((id, i)).is_ok() {
                    a.dirty.rotate_right(1);
                }
                stuck += 1;
            }
        }
        if self.area.as_ref().is_some_and(|a| a.dirty_lost) && out.len() < cap {
            // The list missed some: every file's slots.
            let mut full = false;
            'nodes: for id in 0..self.nodes.len() {
                let count = match self.nodes[id].as_ref().map(|n| &n.kind) {
                    Some(Kind::File(Data::Paged { disk, .. })) => disk.len(),
                    _ => continue,
                };
                for i in 0..count {
                    if out.len() == cap {
                        full = true;
                        break 'nodes;
                    }
                    self.collect(out, id, i);
                }
            }
            if !full && let Some(a) = self.area.as_mut() {
                a.dirty_lost = false;
            }
        }
        // By cluster, so consecutive clusters are one write; the copies in
        // that order.
        out.sort_unstable_by_key(|w| w.cluster);
        for (k, w) in out.iter().enumerate() {
            let dst = &mut staging[k * PAGE..(k + 1) * PAGE];
            if let Some(Some(Node { kind: Kind::File(Data::Paged { pages, disk, .. }), .. })) =
                self.nodes.get_mut(w.node)
                && let (Some(Some(p)), Some(slot)) = (pages.get(w.index), disk.get_mut(w.index))
            {
                dst.copy_from_slice(&p.0);
                slot.stored = true;
                slot.recent = true;
            }
        }
        self.area.as_ref().is_some_and(|a| a.dirty.len() > stuck || a.dirty_lost)
    }

    /// Takes page `i` of `id` into `out` if it is dirty and in memory,
    /// clearing its dirty mark (a cluster given first if it has none);
    /// false if it needs a cluster and none is free (it stays dirty).
    fn collect(&mut self, out: &mut Vec<Writeback>, id: NodeId, i: usize) -> bool {
        let state = match self.node(id).map(|n| &n.kind) {
            Some(Kind::File(d)) if d.slot(i).dirty && d.resident(i) => d.slot(i),
            _ => return true,
        };
        let cluster = match state.cluster() {
            Some(c) => c,
            None => match self.give_cluster(id, i) {
                Ok(c) => {
                    let run = Run { page: i as u64, cluster: c, count: 1 };
                    let mut runs = Vec::new();
                    let _ = runs.try_push(run);
                    self.record_node(id, |path, n| {
                        Some(Op::Clusters { path, size: n.size(), written: n.written, runs })
                    });
                    c
                }
                Err(_) => {
                    if let Some(a) = self.area.as_mut() {
                        a.unbacked += 1;
                    }
                    return false;
                }
            },
        };
        if let Ok(Kind::File(d)) = self.node_mut(id).map(|n| &mut n.kind)
            && let Ok(slot) = d.slot_mut(i)
        {
            slot.dirty = false;
        }
        // Within the capacity the caller checked.
        let _ = out.try_push(Writeback { node: id, index: i, cluster });
        true
    }

    /// Pages whose write failed are dirty again (unless they changed
    /// cluster meanwhile).
    pub fn writeback_failed(&mut self, failed: &[Writeback]) {
        for w in failed {
            let same = matches!(self.node(w.node).map(|n| &n.kind),
                Some(Kind::File(d)) if d.slot(w.index).cluster() == Some(w.cluster));
            if same {
                if let Ok(Kind::File(d)) = self.node_mut(w.node).map(|n| &mut n.kind)
                    && let Ok(slot) = d.slot_mut(w.index)
                {
                    slot.dirty = false;
                }
                self.page_dirty(w.node, w.index);
            }
        }
    }

    /// The clusters given up so far (released after the sync that writes
    /// the records giving them up).
    pub fn take_pending(&mut self) -> Vec<u32> {
        self.area.as_mut().map(|a| a.clusters.take_pending()).unwrap_or_default()
    }

    /// Frees clusters taken with `take_pending` (their records are on the
    /// disk), or keeps them pending (`written` false: the sync failed).
    pub fn release_pending(&mut self, list: Vec<u32>, written: bool) {
        if let Some(a) = self.area.as_mut() {
            if written {
                a.clusters.release(&list);
            } else {
                a.clusters.keep_pending(list);
            }
        }
    }

    /// What the files' pages hold of memory on a disk-backed C: (zeros
    /// without a data area): for a state dump and for `persist`'s warning
    /// when letting pages go cannot bring free memory back.
    pub fn cache_stats(&self) -> CacheStats {
        let mut st = CacheStats::default();
        let Some(area) = self.area.as_ref() else { return st };
        st.queued_dirty = area.dirty.len() as u64;
        for n in self.nodes.iter().flatten() {
            let Kind::File(Data::Paged { pages, disk, .. }) = &n.kind else { continue };
            for (i, p) in pages.iter().enumerate() {
                if p.is_none() {
                    continue;
                }
                let slot = disk.get(i).copied().unwrap_or_default();
                st.resident += 1;
                if slot.dirty {
                    st.dirty += 1;
                } else if !slot.stored || slot.cluster().is_none() {
                    st.unstored += 1;
                }
                if Self::viewed(n, i) {
                    st.mapped += 1;
                }
            }
        }
        st
    }

    /// Lets up to `max` pages that are on the disk unchanged leave memory
    /// (pages of files a view shows stay). The hand goes round the
    /// files' pages from where it stopped: a page written or read back
    /// since it last passed gets a second chance (its mark cleared), so a
    /// call goes round at most twice. Allocates nothing. How many went.
    pub fn evict(&mut self, max: usize) -> usize {
        let Some(area) = self.area.as_ref() else { return 0 };
        let (mut id, mut i) = area.hand;
        let count = self.nodes.len();
        if count == 0 {
            return 0;
        }
        id %= count;
        let mut gone = 0;
        // Each node visited at most twice round, plus the one it starts in.
        let mut visits = 2 * count + 1;
        while gone < max && visits > 0 {
            if let Some(Some(node)) = self.nodes.get_mut(id)
                && node.views_unlisted == 0
                && let Kind::File(Data::Paged { pages, disk, .. }) = &mut node.kind
            {
                let views = &node.views;
                let mut dropped = false;
                while i < pages.len() && gone < max {
                    let slot = disk.get_mut(i);
                    let page = &mut pages[i];
                    i += 1;
                    let Some(slot) = slot else { continue };
                    if page.is_none() || !slot.stored || slot.dirty || slot.cluster().is_none() {
                        continue;
                    }
                    if views.iter().any(|&(a, b)| (a..b).contains(&(i - 1))) {
                        continue;
                    }
                    if slot.recent {
                        slot.recent = false;
                        continue;
                    }
                    *page = None;
                    dropped = true;
                    gone += 1;
                }
                if dropped {
                    node.epoch = next_version();
                }
                if gone == max && i < pages.len() {
                    break;
                }
            }
            id = (id + 1) % count;
            i = 0;
            visits -= 1;
        }
        if let Some(a) = self.area.as_mut() {
            a.hand = (id, i);
        }
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn fs() -> RamFs {
        RamFs::new(1).unwrap()
    }

    const RW: Access = Access { read: true, write: true, delete: false };
    const R: Access = Access { read: true, write: false, delete: false };
    const NONE: Share = Share { read: false, write: false, delete: false };
    const SHARE_R: Share = Share { read: true, write: false, delete: false };

    /// A file system recording onto a data area of `clusters`, and a
    /// "disk" of those clusters the tests write back to and read from.
    fn disk_fs(clusters: u32) -> (RamFs, Vec<[u8; PAGE]>) {
        let mut f = fs();
        f.attach_area(clusters).unwrap();
        f.recording = true;
        (f, vec![[0u8; PAGE]; clusters as usize])
    }

    /// What `nt::persist::sync` does with the data: every dirty page to
    /// its cluster, then the records, then the given-up clusters freed.
    fn sync(f: &mut RamFs, disk: &mut [[u8; PAGE]]) -> Vec<Op> {
        for (w, page) in writeback_all(f) {
            disk[w.cluster as usize] = page;
        }
        let (ops, lost) = f.take_ops();
        assert!(!lost);
        let pending = f.take_pending();
        f.release_pending(pending, true);
        ops
    }

    /// Every dirty page's writeback, through small buffers (several
    /// rounds), with its copy.
    fn writeback_all(f: &mut RamFs) -> Vec<(Writeback, [u8; PAGE])> {
        let mut all = Vec::new();
        let mut out = Vec::with_capacity(3);
        let mut staging = vec![0u8; 3 * PAGE];
        loop {
            let more = f.writeback(&mut out, &mut staging);
            for (k, w) in out.iter().enumerate() {
                all.push((*w, staging[k * PAGE..(k + 1) * PAGE].try_into().unwrap()));
            }
            if !more {
                return all;
            }
        }
    }

    /// Reads all of `id`, reading pages back from `disk` as asked.
    fn read_back(f: &mut RamFs, id: NodeId, disk: &[[u8; PAGE]]) -> (Vec<u8>, usize) {
        let mut out = vec![0u8; f.node(id).unwrap().size() as usize];
        let (mut at, mut page_ins) = (0, 0);
        while at < out.len() {
            match f.read(id, at as u64, &mut out[at..]) {
                Ok(n) => at += n,
                Err(FsError::NotResident) => {
                    let req = f.take_page_in().unwrap();
                    assert!(f.fill(&req, Box::new(Page(disk[req.cluster as usize]))));
                    page_ins += 1;
                }
                Err(e) => panic!("{e:?}"),
            }
        }
        (out, page_ins)
    }

    #[test]
    fn a_disk_backed_file_lives_in_clusters_and_leaves_memory() {
        let (mut f, mut disk) = disk_fs(8);
        let id = f.create(ROOT, &w("big"), false, 1).unwrap();
        let data: Vec<u8> = (0..3 * PAGE + 100).map(|i| (i / 7) as u8).collect();
        f.write(id, 0, &data, 2).unwrap();
        assert_eq!(f.area_size(), Some((8, 4)));
        let ops = sync(&mut f, &mut disk);
        // One Clusters record (the Create first), no Write.
        assert!(matches!(&ops[1], Op::Clusters { size, runs, .. }
            if *size == data.len() as u64 && runs == &[Run { page: 0, cluster: 0, count: 4 }]));
        assert!(!ops.iter().any(|o| matches!(o, Op::Write { .. })));
        // All four pages may go; reads bring them back.
        assert_eq!(f.evict(100), 4);
        let mut b = [0u8; 4];
        assert_eq!(f.read(id, 5, &mut b), Err(FsError::NotResident));
        let (got, page_ins) = read_back(&mut f, id, &disk);
        assert_eq!(got, data);
        assert_eq!(page_ins, 4);
        // A write into part of a page that is on the disk only needs it
        // first; a whole page does not.
        f.evict(100);
        assert_eq!(f.write(id, 10, b"x", 3), Err(FsError::NotResident));
        f.write(id, PAGE as u64, &[9; PAGE], 3).unwrap();
        // A page read while it changed is not taken.
        let req = f.take_page_in().unwrap();
        assert_eq!(req.index, 0);
        f.write(id, 0, &[1; PAGE], 4).unwrap();
        assert!(!f.fill(&req, Box::new(Page([0; PAGE]))));
        sync(&mut f, &mut disk);
        f.evict(100);
        let (got, _) = read_back(&mut f, id, &disk);
        assert_eq!(&got[..PAGE], &[1; PAGE]);
        assert_eq!(&got[PAGE..2 * PAGE], &[9; PAGE]);
        assert_eq!(&got[2 * PAGE..], &data[2 * PAGE..]);
    }

    #[test]
    fn a_full_area_refuses_writes_and_given_up_clusters_come_back_after_sync() {
        let (mut f, mut disk) = disk_fs(4);
        let id = f.create(ROOT, &w("f"), false, 1).unwrap();
        f.write(id, 0, &[1; 3 * PAGE], 2).unwrap();
        // Two more pages do not fit: nothing is written.
        assert_eq!(f.room_for(id, 3 * PAGE as u64, 2 * PAGE as u64), Err(FsError::DiskFull));
        assert_eq!(f.write(id, 3 * PAGE as u64, &[2; 2 * PAGE], 3), Err(FsError::DiskFull));
        assert_eq!(f.node(id).unwrap().size(), 3 * PAGE as u64);
        // Rewriting allocated pages needs no cluster.
        f.write(id, 0, &[3; PAGE], 4).unwrap();
        sync(&mut f, &mut disk);
        // Truncated pages' clusters are free only after the sync.
        f.set_size(id, PAGE as u64 + 1, 5).unwrap();
        assert_eq!(f.area_size(), Some((4, 1)));
        let ops = sync(&mut f, &mut disk);
        assert!(ops.iter().any(|o| matches!(o, Op::SetSize { .. })));
        assert_eq!(f.area_size(), Some((4, 2)));
        // A deleted file's too.
        f.unlink(ROOT, &w("f")).unwrap();
        assert_eq!(f.area_size(), Some((4, 2)));
        sync(&mut f, &mut disk);
        assert_eq!(f.area_size(), Some((4, 4)));
    }

    #[test]
    fn a_shrink_clears_the_cut_off_part_of_a_page_on_the_disk() {
        let (mut f, mut disk) = disk_fs(4);
        let id = f.create(ROOT, &w("f"), false, 1).unwrap();
        f.write(id, 0, &[7; PAGE], 2).unwrap();
        sync(&mut f, &mut disk);
        f.evict(10);
        // A shrink needs nothing from the disk (as when replayed); growing
        // past the cut needs the page, cleared past the end when read.
        f.set_size(id, 10, 3).unwrap();
        assert_eq!(f.set_size(id, 20, 4), Err(FsError::NotResident));
        let req = f.take_page_in().unwrap();
        assert!(f.fill(&req, Box::new(Page(disk[req.cluster as usize]))));
        f.set_size(id, 20, 4).unwrap();
        sync(&mut f, &mut disk);
        f.evict(10);
        let (got, _) = read_back(&mut f, id, &disk);
        assert_eq!(got, [&[7u8; 10][..], &[0u8; 10][..]].concat());
    }

    #[test]
    fn a_snapshot_names_clusters_and_replays_onto_a_new_boot() {
        let (mut f, mut disk) = disk_fs(8);
        let d = f.create(ROOT, &w("d"), true, 1).unwrap();
        let id = f.create(d, &w("f"), false, 1).unwrap();
        let data: Vec<u8> = (0..2 * PAGE + 5).map(|i| i as u8).collect();
        f.write(id, 0, &data, 2).unwrap();
        sync(&mut f, &mut disk);
        // A page changed since: its data goes as a Write, the rest as runs.
        f.write(id, PAGE as u64, &[5; PAGE], 3).unwrap();
        let snapshot = f.snapshot().unwrap();
        assert!(
            snapshot
                .iter()
                .any(|o| matches!(o, Op::Write { offset, .. } if *offset == PAGE as u64))
        );
        assert!(snapshot.iter().any(|o| matches!(o, Op::Clusters { runs, .. }
            if runs == &[Run { page: 0, cluster: 0, count: 1 }, Run { page: 2, cluster: 2, count: 1 }])));
        // The next boot: replay, then the area (the replayed Write's page
        // becomes dirty, to get a cluster at the first sync).
        let mut g = fs();
        for op in &snapshot {
            g.apply(op).unwrap();
        }
        assert_eq!(g.attach_area(8).unwrap(), (2, 0));
        g.recording = true;
        let gid = g.resolve(ROOT, &[&w("d"), &w("f")]).unwrap();
        let ops = sync(&mut g, &mut disk);
        assert!(ops.iter().any(|o| matches!(o, Op::Clusters { .. })));
        g.evict(10);
        let (got, page_ins) = read_back(&mut g, gid, &disk);
        assert_eq!(&got[..PAGE], &data[..PAGE]);
        assert_eq!(&got[PAGE..2 * PAGE], &[5; PAGE]);
        assert_eq!(&got[2 * PAGE..], &data[2 * PAGE..]);
        assert_eq!(page_ins, 3);
    }

    #[test]
    fn a_log_of_appends_and_a_truncation_replays_without_the_disk() {
        let (mut f, mut disk) = disk_fs(8);
        let id = f.create(ROOT, &w("log"), false, 1).unwrap();
        let mut log = Vec::new();
        // Appends across a page boundary, each sync'd (one record each),
        // then a truncation into the first page and another append.
        for (i, chunk) in [&[1u8; 3000][..], &[2; 3000], &[3; 100]].iter().enumerate() {
            let at = f.node(id).unwrap().size();
            f.write(id, at, chunk, 2 + i as u64).unwrap();
            log.extend(sync(&mut f, &mut disk));
        }
        f.set_size(id, 1000, 6).unwrap();
        f.write(id, 1000, &[4; 10], 7).unwrap();
        log.extend(sync(&mut f, &mut disk));
        let mut g = fs();
        for op in &log {
            g.apply(op).unwrap();
        }
        // Nothing was skipped: the file is as it was.
        let gid = g.resolve(ROOT, &[&w("log")]).unwrap();
        g.attach_area(8).unwrap();
        let (got, _) = read_back(&mut g, gid, &disk);
        assert_eq!(got, [&[1u8; 1000][..], &[4; 10]].concat());
    }

    #[test]
    fn pages_changed_through_a_writable_mapping_are_written_at_unmap() {
        let (mut f, mut disk) = disk_fs(4);
        let id = f.create(ROOT, &w("m"), false, 1).unwrap();
        f.set_size(id, 2 * PAGE as u64, 2).unwrap();
        f.write(id, 0, &[1; PAGE], 2).unwrap();
        sync(&mut f, &mut disk);
        f.map(id, 2 * PAGE as u64, true).unwrap();
        f.view_started(id, 0..2 * PAGE as u64);
        // Both pages in memory for the view (one made from the hole).
        f.page_address(id, 0).unwrap();
        f.page_address(id, 1).unwrap();
        // Pages a view shows stay in memory.
        assert_eq!(f.evict(10), 0);
        f.view_ended(id, true, 0..2 * PAGE as u64);
        f.unmap(id, true);
        // Either may have changed: both are written, the hole's page gets a
        // cluster and its record.
        let written = writeback_all(&mut f);
        assert_eq!(written.len(), 2);
        let (ops, _) = f.take_ops();
        assert!(ops.iter().any(|o| matches!(o, Op::Clusters { runs, .. }
            if runs == &[Run { page: 1, cluster: 1, count: 1 }])));
        assert_eq!(f.evict(10), 2);
    }

    #[test]
    fn a_mapped_file_without_views_lets_its_pages_go() {
        // A section on the file lives (`map`), its views come and go: only
        // the pages a live view shows stay in memory; a page written
        // through a writable view is written before it may leave.
        let (mut f, mut disk) = disk_fs(8);
        let id = f.create(ROOT, &w("v"), false, 1).unwrap();
        f.write(id, 0, &[3; 4 * PAGE], 2).unwrap();
        sync(&mut f, &mut disk);
        f.map(id, 4 * PAGE as u64, true).unwrap();
        f.view_started(id, PAGE as u64..2 * PAGE as u64);
        // Only the page the view shows stays.
        assert_eq!(f.evict(10), 3);
        let (got, _) = read_back(&mut f, id, &disk);
        assert_eq!(got, vec![3; 4 * PAGE]);
        f.view_ended(id, true, PAGE as u64..2 * PAGE as u64);
        // The view's page is dirty again; the three others may go.
        assert_eq!(f.evict(10), 3);
        assert_eq!(writeback_all(&mut f).len(), 1);
        assert_eq!(f.evict(10), 1);
        // Read back through the still-mapped file.
        assert_eq!(f.page_address(id, 2), Err(FsError::NotResident));
        let (got, _) = read_back(&mut f, id, &disk);
        assert_eq!(got, vec![3; 4 * PAGE]);
        f.unmap(id, true);
    }

    #[test]
    fn create_find_case_insensitive() {
        let mut f = fs();
        let users = w("Users");
        let d = f.create(ROOT, &users, true, 2).unwrap();
        let file = f.create(d, &w("Readme.TXT"), false, 2).unwrap();
        assert_eq!(f.find(d, &w("README.txt")).unwrap(), Some(file));
        assert_eq!(f.create(d, &w("readme.txt"), false, 2), Err(FsError::Exists));
        let parts = [users.as_slice(), &w("readme.txt")];
        assert_eq!(f.resolve(ROOT, &parts), Ok(file));
        assert_eq!(f.resolve(ROOT, &[&w("Nope"), &w("x")]), Err(FsError::PathNotFound));
        assert_eq!(f.resolve(ROOT, &[users.as_slice(), &w("x")]), Err(FsError::NotFound));
        assert_eq!(f.node(file).unwrap().attributes, ATTRIBUTE_ARCHIVE);
        assert!(f.node(d).unwrap().is_dir());
    }

    #[test]
    fn read_write_extend_truncate() {
        let mut f = fs();
        let id = f.create(ROOT, &w("a"), false, 1).unwrap();
        assert_eq!(f.write(id, 0, b"hello", 2), Ok(5));
        assert_eq!(f.write(id, 8, b"!", 3), Ok(1));
        let mut buf = [0xffu8; 16];
        assert_eq!(f.read(id, 0, &mut buf), Ok(9));
        assert_eq!(&buf[..9], b"hello\0\0\0!");
        assert_eq!(f.read(id, 9, &mut buf), Ok(0));
        f.set_size(id, 2, 4).unwrap();
        assert_eq!(f.node(id).unwrap().size(), 2);
        assert_eq!(f.node(id).unwrap().written, 4);
    }

    #[test]
    fn writable_static_files_are_not_read_only() {
        static DATA: &[u8] = b"master";
        let mut f = fs();
        let id = f.add_static_file(ROOT, &w("master.mdf"), DATA, true, 1).unwrap();
        assert_eq!(f.node(id).unwrap().attributes & ATTRIBUTE_READONLY, 0);
        f.write(id, 6, b"!", 2).unwrap();
        let mut buf = [0u8; 7];
        f.read(id, 0, &mut buf).unwrap();
        assert_eq!(&buf, b"master!");
        assert_eq!(DATA, b"master"); // the module's bytes are left alone
    }

    #[test]
    fn static_files_copy_on_write() {
        static DATA: &[u8] = b"boot module";
        let mut f = fs();
        let id = f.add_static_file(ROOT, &w("m.exe"), DATA, false, 1).unwrap();
        assert_eq!(f.node(id).unwrap().attributes & ATTRIBUTE_READONLY, ATTRIBUTE_READONLY);
        f.write(id, 0, b"B", 2).unwrap();
        let mut buf = [0u8; 11];
        f.read(id, 0, &mut buf).unwrap();
        assert_eq!(&buf, b"Boot module");
        assert_eq!(DATA, b"boot module");
    }

    #[test]
    fn share_access_rules() {
        let mut f = fs();
        let id = f.create(ROOT, &w("s"), false, 1).unwrap();
        f.open(id, RW, NONE).unwrap();
        assert_eq!(f.open(id, R, SHARE_R.into_full()), Err(FsError::SharingViolation));
        f.close(id, RW, NONE);
        f.open(id, R, SHARE_R).unwrap();
        f.open(id, R, SHARE_R).unwrap();
        assert_eq!(
            f.open(id, RW, Share { read: true, write: true, delete: true }),
            Err(FsError::SharingViolation)
        );
        // Attribute-only opens never conflict.
        f.open(id, Access::default(), NONE).unwrap();
    }

    #[test]
    fn delete_pending_removes_on_last_close() {
        let mut f = fs();
        let dir = f.create(ROOT, &w("d"), true, 1).unwrap();
        let id = f.create(dir, &w("x"), false, 1).unwrap();
        assert_eq!(f.set_delete_pending(dir, true), Err(FsError::DirectoryNotEmpty));
        f.open(id, RW, NONE).unwrap();
        f.set_delete_pending(id, true).unwrap();
        assert_eq!(f.open(id, R, SHARE_R), Err(FsError::DeletePending));
        f.close(id, RW, NONE);
        assert_eq!(f.find(dir, &w("x")), Ok(None));
        f.set_delete_pending(dir, true).unwrap();
    }

    impl Share {
        fn into_full(self) -> Share {
            Share { read: true, write: true, delete: true }
        }
    }

    #[test]
    fn mkdir_all() {
        let mut f = fs();
        let a = w("Windows");
        let b = w("System32");
        let id = f.mkdir_all(&[&a, &b], 1).unwrap();
        assert_eq!(f.mkdir_all(&[&a, &b], 1).unwrap(), id);
        assert_eq!(f.resolve(ROOT, &[&a, &b]), Ok(id));
    }

    #[test]
    fn paths_of_nodes() {
        let mut f = fs();
        let d = f.create(ROOT, &w("Windows"), true, 1).unwrap();
        let s = f.create(d, &w("System32"), true, 1).unwrap();
        let n = f.create(s, &w("ntdll.dll"), false, 1).unwrap();
        assert_eq!(f.path_of(n).unwrap(), w("\\Windows\\System32\\ntdll.dll"));
        assert_eq!(f.path_of(ROOT).unwrap(), w("\\"));
    }

    #[test]
    fn wildcards() {
        let m = |mask: &str, name: &str| wildcard_match(&w(mask), &w(name));
        assert!(m("*", "anything.txt"));
        assert!(m("*.txt", "B.TXT"));
        assert!(!m("*.txt", "b.txt.bak"));
        assert!(m("?.dat", "A.dat"));
        assert!(!m("?.dat", "AB.dat"));
        assert!(m("a*b*c", "aXXbYYc"));
        assert!(!m("a*b*c", "aXXbYY"));
        assert!(m("B.TXT", "b.txt"));
        assert!(m("<.txt", "x.txt"));
        // kernel32's `*.*` (`<"*`): every name, with or without a dot.
        for name in ["noext", "a.txt", "a.b.c", ".hidden", "x."] {
            assert!(m("<\"*", name), "{name}");
        }
        // `*.c` (`<.c`): DOS_STAR takes earlier periods, not the last.
        assert!(m("<.c", "a.b.c"));
        assert!(!m("<.c", "a.c.d"));
        assert!(!m("<.txt", "b.txt.bak"));
        // DOS_QM: one unit, or none at a period or the end.
        assert!(m(">.txt", "a.txt"));
        assert!(m(">>>.txt", "a.txt"));
        assert!(!m(">.txt", "ab.txt"));
        assert!(m("ab>>", "ab"));
        // DOS_DOT: a period, or none at the end.
        assert!(m("abc\"*", "abc"));
        assert!(m("abc\"*", "abc.d"));
        assert!(!m("abc\"*", "abcd"));
        // NT's plain `*.*` keeps its literal period.
        assert!(!m("*.*", "noext"));
        assert!(!has_wildcards(&w("plain.txt")));
        assert!(has_wildcards(&w("p?ain")));
    }

    #[test]
    fn list_is_sorted_case_insensitively() {
        let mut f = fs();
        for name in ["sub", "b.txt", "A.dat", "c.txt"] {
            f.create(ROOT, &w(name), false, 1).unwrap();
        }
        let names: Vec<Vec<u16>> =
            f.list(ROOT).unwrap().iter().map(|&id| f.node(id).unwrap().name().to_vec()).collect();
        assert_eq!(names, [w("A.dat"), w("b.txt"), w("c.txt"), w("sub")]);
    }

    #[test]
    fn change_journal_and_relative_names() {
        let mut f = fs();
        let d = f.create(ROOT, &w("d"), true, 1).unwrap();
        // Off by default: nothing recorded.
        assert!(f.take_changes().0.is_empty());
        f.journal = true;
        let a = f.create(d, &w("a"), false, 1).unwrap();
        f.write(a, 0, b"x", 2).unwrap();
        f.write(a, 1, b"y", 2).unwrap(); // the same change again: once
        let sub = f.create(d, &w("sub"), true, 1).unwrap();
        f.rename(a, sub, &w("b"), false).unwrap();
        let (changes, lost) = f.take_changes();
        assert!(!lost);
        let summary: Vec<(u32, String, NodeId)> = changes
            .iter()
            .map(|c| (c.action, String::from_utf16_lossy(&c.name), c.parent))
            .collect();
        assert_eq!(
            summary,
            [
                (ACTION_ADDED, "a".into(), d),
                (ACTION_MODIFIED, "a".into(), d),
                (ACTION_ADDED, "sub".into(), d),
                (ACTION_RENAMED_OLD_NAME, "a".into(), d),
                (ACTION_RENAMED_NEW_NAME, "b".into(), sub),
            ]
        );
        assert!(changes[2].directory);
        let rel = |dir, parent, tree| {
            f.relative_name(dir, parent, &w("b"), tree).map(|v| String::from_utf16_lossy(&v))
        };
        assert_eq!(rel(sub, sub, false).as_deref(), Some("b"));
        assert_eq!(rel(d, sub, false), None);
        assert_eq!(rel(d, sub, true).as_deref(), Some("sub\\b"));
        assert_eq!(rel(ROOT, sub, true).as_deref(), Some("d\\sub\\b"));
        assert_eq!(rel(sub, d, true), None);
    }

    #[test]
    fn rename_moves_and_replaces() {
        let mut f = fs();
        let d = f.create(ROOT, &w("d"), true, 1).unwrap();
        let a = f.create(ROOT, &w("a"), false, 1).unwrap();
        let b = f.create(ROOT, &w("b"), false, 1).unwrap();
        assert_eq!(f.rename(a, ROOT, &w("B"), false), Err(FsError::Exists));
        f.open(b, R, SHARE_R).unwrap();
        assert_eq!(f.rename(a, ROOT, &w("b"), true), Err(FsError::AccessDenied));
        f.close(b, R, SHARE_R);
        f.rename(a, ROOT, &w("b"), true).unwrap();
        assert!(f.node(b).is_none(), "replaced target removed");
        assert_eq!(f.resolve(ROOT, &[&w("b")]), Ok(a));
        f.rename(a, d, &w("moved"), false).unwrap();
        assert_eq!(f.resolve(ROOT, &[&w("d"), &w("MOVED")]), Ok(a));
        assert_eq!(f.list(ROOT).unwrap(), [d]);
        assert_eq!(f.rename(d, d, &w("x"), false), Err(FsError::AccessDenied));
        assert_eq!(f.rename(a, d, &w("moved"), false), Ok(()), "renaming onto itself");
    }

    #[test]
    fn paged_data_across_pages() {
        let mut f = fs();
        let id = f.create(ROOT, &w("p"), false, 1).unwrap();
        let data: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        f.write(id, 0, &data, 2).unwrap();
        let mut back = vec![0u8; 10_000];
        assert_eq!(f.read(id, 0, &mut back), Ok(10_000));
        assert_eq!(back, data);
        let mut mid = [0u8; 10];
        assert_eq!(f.read(id, 4090, &mut mid), Ok(10));
        assert_eq!(&mid[..], &data[4090..4100]);
        // Truncation keeps the tail of the last page zero.
        f.set_size(id, 5000, 3).unwrap();
        f.set_size(id, 6000, 4).unwrap();
        let mut tail = [0xffu8; 1000];
        assert_eq!(f.read(id, 5000, &mut tail), Ok(1000));
        assert!(tail.iter().all(|&b| b == 0));
    }

    #[test]
    fn mapped_pages_stay_put_and_block_truncation() {
        let mut f = fs();
        let id = f.create(ROOT, &w("m"), false, 1).unwrap();
        f.write(id, 0, &[7; 5000], 2).unwrap();
        assert_eq!(f.map(id, 0x3000, true), Err(FsError::NoMemory), "larger than the file");
        f.map(id, 5000, true).unwrap();
        let first = f.page_address(id, 0).unwrap();
        assert_eq!(first % PAGE, 0);
        f.write(id, 9000, b"grow", 3).unwrap();
        assert_eq!(f.page_address(id, 0), Ok(first), "pages do not move");
        assert!(f.page_address(id, 2).is_ok());
        assert_eq!(f.set_size(id, 100, 4), Err(FsError::UserMapped));
        f.set_size(id, 6000, 4).unwrap();
        f.unmap(id, true);
        f.set_size(id, 100, 5).unwrap();
        // A read-only mapping blocks truncation too.
        f.map(id, 100, false).unwrap();
        assert_eq!(f.set_size(id, 50, 6), Err(FsError::UserMapped));
        f.unmap(id, false);
        f.set_size(id, 50, 7).unwrap();
    }

    #[test]
    fn large_static_files_turn_into_pages_only_where_touched() {
        // Three pages and a bit of a boot module.
        static BIG: [u8; 3 * PAGE + 10] = {
            let mut b = [0u8; 3 * PAGE + 10];
            let mut i = 0;
            while i < b.len() {
                b[i] = (i / PAGE) as u8 + 1;
                i += 1;
            }
            b
        };
        let mut f = fs();
        let id = f.add_static_file(ROOT, &w("m"), &BIG, false, 1).unwrap();
        f.map(id, BIG.len() as u64, true).unwrap();
        let made = |f: &RamFs| match &f.node(id).unwrap().kind {
            Kind::File(Data::Paged { pages, .. }) => pages.iter().filter(|p| p.is_some()).count(),
            _ => usize::MAX,
        };
        // Mapping made no page; reads come from the module.
        assert_eq!(made(&f), 0);
        let mut b = [0u8; 4];
        f.read(id, 2 * PAGE as u64 - 2, &mut b).unwrap();
        assert_eq!(b, [2, 2, 3, 3]);
        // A write makes its page only; a mapped page is made when asked for.
        f.write(id, PAGE as u64, b"x", 1).unwrap();
        assert_eq!(made(&f), 1);
        let addr = f.page_address(id, 3).unwrap();
        assert_eq!(made(&f), 2);
        assert_eq!(f.page_address(id, 3), Ok(addr));
        f.read(id, PAGE as u64, &mut b).unwrap();
        assert_eq!(b, [b'x', 2, 2, 2]);
        f.unmap(id, true);
        // Shrinking ends the module's bytes there too: growing shows zeros.
        f.set_size(id, 5, 1).unwrap();
        f.set_size(id, 2 * PAGE as u64, 1).unwrap();
        f.read(id, PAGE as u64, &mut b).unwrap();
        assert_eq!(b, [0, 0, 0, 0]);
        f.read(id, 3, &mut b).unwrap();
        assert_eq!(b, [1, 1, 0, 0]);
    }

    #[test]
    fn static_files_become_pages_when_mapped() {
        static DATA: &[u8] = b"module";
        let mut f = fs();
        let id = f.add_static_file(ROOT, &w("s.dll"), DATA, false, 1).unwrap();
        assert!(f.page_address(id, 0).is_err());
        f.map(id, 6, false).unwrap();
        assert!(f.page_address(id, 0).is_ok());
        let mut b = [0u8; 6];
        f.read(id, 0, &mut b).unwrap();
        assert_eq!(&b, b"module");
    }

    #[test]
    fn set_basic_times_and_attributes() {
        let mut f = fs();
        let file = f.create(ROOT, &w("t.txt"), false, 5).unwrap();
        let written = |t| Times { written: Some(t), ..Times::default() };
        f.set_basic(file, written(99), None).unwrap();
        let n = f.node(file).unwrap();
        assert_eq!((n.created, n.accessed, n.written), (5, 5, 99));
        assert_eq!(n.attributes, ATTRIBUTE_ARCHIVE);
        let created = Times { created: Some(7), ..Times::default() };
        f.set_basic(file, created, Some(ATTRIBUTE_READONLY | ATTRIBUTE_DIRECTORY)).unwrap();
        let n = f.node(file).unwrap();
        assert_eq!((n.created, n.written, n.attributes), (7, 99, ATTRIBUTE_READONLY));
        // The access time is set alone (scp -p), and a write sets it too.
        f.set_basic(file, Times { accessed: Some(3), ..Times::default() }, None).unwrap();
        let n = f.node(file).unwrap();
        assert_eq!((n.created, n.accessed, n.written), (7, 3, 99));
        f.write(file, 0, b"x", 100).unwrap();
        let n = f.node(file).unwrap();
        assert_eq!((n.created, n.accessed, n.written), (7, 100, 100));
        let d = f.create(ROOT, &w("d"), true, 5).unwrap();
        f.set_basic(d, Times::default(), Some(0x02)).unwrap();
        assert_eq!(f.node(d).unwrap().attributes, 0x02 | ATTRIBUTE_DIRECTORY);
    }

    #[test]
    fn versions_change_with_contents() {
        let mut f = fs();
        let a = f.create(ROOT, &w("a.dll"), false, 1).unwrap();
        let b = f.create(ROOT, &w("b.dll"), false, 1).unwrap();
        let v = f.node(a).unwrap().version().unwrap();
        assert_ne!(Some(v), f.node(b).unwrap().version());
        f.write(a, 0, b"MZ", 2).unwrap();
        let v2 = f.node(a).unwrap().version().unwrap();
        assert_ne!(v, v2);
        f.set_basic(a, Times::all(9, 9, 9), None).unwrap();
        assert_eq!(f.node(a).unwrap().version(), Some(v2));
        // A read-only data section cannot change the contents: the
        // version stays (an image section of the file shares its layout).
        f.map(a, 2, false).unwrap();
        assert_eq!(f.node(a).unwrap().version(), Some(v2));
        f.unmap(a, false);
        assert_eq!(f.node(a).unwrap().version(), Some(v2));
        // A writable one can, behind the file system's back.
        f.map(a, 2, true).unwrap();
        assert_eq!(f.node(a).unwrap().version(), None);
        f.map(a, 2, false).unwrap();
        f.unmap(a, true);
        let v3 = f.node(a).unwrap().version().unwrap();
        assert_ne!(v3, v2);
        f.unmap(a, false);
        assert_eq!(f.node(a).unwrap().version(), Some(v3));
    }

    /// A boot tree: \\boot\\keep.txt and \\boot\\gone.txt (static files).
    fn booted() -> RamFs {
        let mut f = fs();
        let b = f.create(ROOT, &w("boot"), true, 1).unwrap();
        f.add_static_file(b, &w("keep.txt"), b"boot data", true, 1).unwrap();
        f.add_static_file(b, &w("gone.txt"), b"bye", true, 1).unwrap();
        f.recording = true;
        f
    }

    /// A volume (the Linux view's `/run`): out of the root's paths, its
    /// changes never in the change log nor a snapshot, its paths its
    /// own, moves and links between it and the root's tree refused.
    #[test]
    fn volumes_are_trees_of_their_own_never_logged() {
        let mut f = booted();
        let v = f.create_volume(0o755, 2).unwrap();
        assert_eq!(f.node(v).unwrap().parent(), v);
        assert!(f.list(ROOT).unwrap().iter().all(|&n| n != v));
        let user = f.create_posix(v, &w("user"), true, 0o755, 3).unwrap();
        let a = f.create_posix(user, &w("a"), false, 0o644, 3).unwrap();
        f.write(a, 0, b"x", 4).unwrap();
        f.set_mode(a, 0o600).unwrap();
        f.rename(a, user, &w("b"), false).unwrap();
        f.link(a, v, &w("c")).unwrap();
        f.unlink(v, &w("c")).unwrap();
        let s = f.create_socket(user, &w("wayland-0"), 0o755, 4).unwrap();
        assert_eq!(f.node(s).unwrap().volume(), v);
        let (ops, lost) = f.take_ops();
        assert!(ops.is_empty() && !lost);
        assert!(f.snapshot().unwrap().is_empty());
        assert_eq!(String::from_utf16(&f.path_of(a).unwrap()).unwrap(), "\\user\\b");
        assert_eq!(String::from_utf16(&f.path_of(v).unwrap()).unwrap(), "\\");
        let boot = f.lookup(&w("\\boot")).unwrap();
        assert_eq!(f.rename(a, boot, &w("b"), false), Err(FsError::CrossVolume));
        assert_eq!(f.link(a, boot, &w("b")), Err(FsError::CrossVolume));
        let keep = f.lookup(&w("\\boot\\keep.txt")).unwrap();
        assert_eq!(f.rename(keep, v, &w("k"), false), Err(FsError::CrossVolume));
        assert_eq!(f.rename(v, boot, &w("v"), false), Err(FsError::AccessDenied));
        f.unlink(user, &w("b")).unwrap();
        f.unlink(user, &w("wayland-0")).unwrap();
        f.unlink(v, &w("user")).unwrap();
        assert!(f.take_ops().0.is_empty());
    }

    fn content(f: &RamFs, path: &str) -> Option<Vec<u8>> {
        let id = f.lookup(&w(path)).ok()?;
        let mut buf = vec![0; f.node(id)?.size() as usize];
        f.read(id, 0, &mut buf).ok()?;
        Some(buf)
    }

    /// Changes a program makes after boot.
    fn change(f: &mut RamFs) {
        let b = f.lookup(&w("\\boot")).unwrap();
        let d = f.create(ROOT, &w("data"), true, 5).unwrap();
        f.set_security(d, vec![1, 0, 4, 0x80]).unwrap();
        let a = f.create(d, &w("a.txt"), false, 5).unwrap();
        f.write(a, 0, b"hello", 6).unwrap();
        f.write(a, 5, &[7u8; 70_000], 7).unwrap();
        f.set_size(a, 60_000, 8).unwrap();
        let times = Times { accessed: Some(41), written: Some(42), ..Times::default() };
        f.set_basic(a, times, Some(0x22)).unwrap();
        f.set_basic(d, Times { accessed: Some(40), ..Times::default() }, None).unwrap();
        let t = f.create(d, &w("tmp"), false, 5).unwrap();
        f.rename(t, d, &w("renamed"), false).unwrap();
        let keep = f.lookup(&w("\\boot\\keep.txt")).unwrap();
        f.write(keep, 0, b"BOOT", 9).unwrap();
        let gone = f.lookup(&w("\\boot\\gone.txt")).unwrap();
        f.open(gone, RW, NONE).unwrap();
        f.set_delete_pending(gone, true).unwrap();
        f.close(gone, RW, NONE);
        let _ = b;
    }

    fn check(f: &RamFs) {
        let a = content(f, "\\data\\a.txt").unwrap();
        assert_eq!(a.len(), 60_000);
        assert_eq!(&a[..5], b"hello");
        assert!(a[5..].iter().all(|&b| b == 7));
        let n = f.node(f.lookup(&w("\\data\\a.txt")).unwrap()).unwrap();
        assert_eq!((n.accessed, n.written, n.attributes), (41, 42, 0x22));
        let d = f.node(f.lookup(&w("\\data")).unwrap()).unwrap();
        assert_eq!(d.accessed, 40);
        assert_eq!(d.security.as_deref(), Some(&[1u8, 0, 4, 0x80][..]));
        assert!(f.lookup(&w("\\data\\tmp")).is_err());
        assert_eq!(content(f, "\\data\\renamed").unwrap(), b"");
        assert_eq!(content(f, "\\boot\\keep.txt").unwrap(), b"BOOT data");
        assert!(f.lookup(&w("\\boot\\gone.txt")).is_err());
    }

    #[test]
    fn recorded_changes_replay_over_the_boot_files() {
        let mut f = booted();
        change(&mut f);
        check(&f);
        let (ops, lost) = f.take_ops();
        assert!(!lost);
        assert!(!f.has_ops());
        // The next boot: the same boot files, then the log.
        let mut g = booted();
        g.recording = false;
        for op in &ops {
            g.apply(op).unwrap();
        }
        check(&g);
        assert!(!g.has_ops(), "replay records nothing");
    }

    #[test]
    fn a_snapshot_replays_to_the_same_state() {
        let mut f = booted();
        change(&mut f);
        let snap = f.snapshot().unwrap();
        // Only the changes: the untouched boot files are not in it.
        assert!(
            !snap.iter().any(|op| matches!(op, Op::Create { path, .. } if *path == w("\\boot")))
        );
        let mut g = booted();
        g.recording = false;
        for op in &snap {
            g.apply(op).unwrap();
        }
        check(&g);
        // And a snapshot of the replayed state is the same snapshot.
        assert_eq!(g.snapshot().unwrap(), snap);
    }

    #[test]
    fn renaming_a_boot_directory_keeps_what_is_below_it() {
        let mut f = booted();
        let b = f.lookup(&w("\\boot")).unwrap();
        f.rename(b, ROOT, &w("moved"), false).unwrap();
        let snap = f.snapshot().unwrap();
        let mut g = booted();
        g.recording = false;
        for op in &snap {
            g.apply(op).unwrap();
        }
        assert!(g.lookup(&w("\\boot")).is_err());
        assert_eq!(content(&g, "\\moved\\keep.txt").unwrap(), b"boot data");
    }

    /// A directory a Linux program makes is case-sensitive (for NT callers
    /// too, as Windows' per-directory flag is); one NT makes inherits its
    /// parent's flag; the boot's are not.
    #[test]
    fn case_sensitive_directories() {
        let mut f = fs();
        let plain = f.create(ROOT, &w("plain"), true, 1).unwrap();
        assert!(!f.node(plain).unwrap().case_sensitive);
        let d = f.create_posix(ROOT, &w("w"), true, 0o755, 1).unwrap();
        assert!(f.node(d).unwrap().case_sensitive);
        assert_eq!(f.node(d).unwrap().mode, Some(0o755));
        let upper = f.create_posix(d, &w("Case"), false, 0o644, 1).unwrap();
        let lower = f.create(d, &w("case"), false, 1).unwrap();
        assert_ne!(upper, lower);
        assert_eq!(f.find(d, &w("Case")), Ok(Some(upper)));
        assert_eq!(f.find(d, &w("case")), Ok(Some(lower)));
        assert_eq!(f.find(d, &w("CASE")), Ok(None));
        // Listed apart, in a stable order (collation, then units).
        let names: Vec<_> = f.entries(d).unwrap().iter().map(|e| e.1.to_vec()).collect();
        assert_eq!(names, [w("Case"), w("case")]);
        // NT's subdirectory inherits the flag; a boot directory's does not.
        let sub = f.create(d, &w("Sub"), true, 1).unwrap();
        assert!(f.node(sub).unwrap().case_sensitive);
        let p2 = f.create(plain, &w("x"), true, 1).unwrap();
        assert!(!f.node(p2).unwrap().case_sensitive);
        f.create(plain, &w("Name"), false, 1).unwrap();
        assert_eq!(f.create(plain, &w("NAME"), false, 1), Err(FsError::Exists));
    }

    /// Hard links: every name finds the node; nlink counts them; removing
    /// the first name moves the node to another; a link goes alone.
    #[test]
    fn hard_links() {
        let mut f = fs();
        let a = f.create(ROOT, &w("a"), true, 1).unwrap();
        let b = f.create(ROOT, &w("b"), true, 1).unwrap();
        let x = f.create(a, &w("x"), false, 1).unwrap();
        f.write(x, 0, b"data", 2).unwrap();
        f.link(x, b, &w("y")).unwrap();
        f.link(x, a, &w("z")).unwrap();
        assert_eq!(f.link(x, a, &w("Z")), Err(FsError::Exists));
        assert_eq!(f.link(a, b, &w("dir")), Err(FsError::IsADirectory));
        assert_eq!(f.find(b, &w("y")), Ok(Some(x)));
        assert_eq!(f.node(x).unwrap().nlink(), 3);
        let names: Vec<_> = f.entries(a).unwrap().iter().map(|e| e.1.to_vec()).collect();
        assert_eq!(names, [w("x"), w("z")]);
        assert_eq!(f.is_empty_dir(b), Ok(false));
        // The first name goes: the node lives on at a link.
        f.unlink(a, &w("x")).unwrap();
        assert_eq!(f.find(a, &w("x")), Ok(None));
        assert_eq!(f.node(x).unwrap().nlink(), 2);
        assert_eq!(f.path_of(x).unwrap(), w("\\b\\y"));
        // A link renamed, then removed.
        f.rename_entry(a, &w("z"), b, &w("z2"), false).unwrap();
        assert_eq!(f.find(b, &w("z2")), Ok(Some(x)));
        f.unlink(b, &w("z2")).unwrap();
        assert_eq!(f.node(x).unwrap().nlink(), 1);
        assert_eq!(f.is_empty_dir(a), Ok(true));
        let mut buf = [0u8; 4];
        f.read(x, 0, &mut buf).unwrap();
        assert_eq!(&buf, b"data");
    }

    /// Linux's unlink of an open file: the name is free at once, the node
    /// lives on (nlink 0) until its last close; NT's delete keeps the name.
    #[test]
    fn unlink_of_an_open_file_frees_the_name() {
        let mut f = fs();
        let id = f.create(ROOT, &w("u"), false, 1).unwrap();
        f.open(id, RW, NONE).unwrap();
        f.unlink(ROOT, &w("u")).unwrap();
        assert_eq!(f.find(ROOT, &w("u")), Ok(None));
        assert!(f.node(id).unwrap().detached());
        assert_eq!(f.node(id).unwrap().nlink(), 0);
        f.write(id, 0, b"still", 2).unwrap();
        let again = f.create(ROOT, &w("u"), false, 3).unwrap();
        assert_ne!(again, id);
        f.close(id, RW, NONE);
        assert!(f.node(id).is_none());
        assert_eq!(f.find(ROOT, &w("u")), Ok(Some(again)));
        // A non-empty directory is not unlinked.
        let d = f.create(ROOT, &w("d"), true, 1).unwrap();
        f.create(d, &w("in"), false, 1).unwrap();
        assert_eq!(f.unlink(ROOT, &w("d")), Err(FsError::DirectoryNotEmpty));
    }

    /// A symbolic link keeps its target; it is no file to read.
    #[test]
    fn symlinks_keep_their_target() {
        let mut f = fs();
        let id = f.symlink(ROOT, &w("l"), b"../t", 1).unwrap();
        let n = f.node(id).unwrap();
        assert!(n.is_symlink() && !n.is_dir());
        assert_eq!(n.symlink_target(), Some(&b"../t"[..]));
        assert_eq!(n.size(), 4);
        assert_eq!(n.attributes & ATTRIBUTE_REPARSE_POINT, ATTRIBUTE_REPARSE_POINT);
        assert_eq!(f.read(id, 0, &mut [0; 4]), Err(FsError::Symlink));
        assert_eq!(f.write(id, 0, b"x", 2), Err(FsError::Symlink));
        assert_eq!(f.symlink(ROOT, &w("L"), b"x", 1), Err(FsError::Exists));
    }

    #[test]
    fn exchange_swaps_two_names() {
        let mut f = fs();
        let a = f.create(ROOT, &w("a"), false, 1).unwrap();
        let d = f.create(ROOT, &w("d"), true, 1).unwrap();
        let b = f.create(d, &w("b"), false, 1).unwrap();
        f.exchange(ROOT, &w("a"), d, &w("b")).unwrap();
        assert_eq!(f.find(ROOT, &w("a")), Ok(Some(b)));
        assert_eq!(f.find(d, &w("b")), Ok(Some(a)));
        assert_eq!(f.entries(ROOT).unwrap().len(), 2, "no temporary name left");
        assert_eq!(f.exchange(ROOT, &w("a"), d, &w("none")), Err(FsError::NotFound));
    }

    /// What the Linux personality makes, as the change log keeps it: modes,
    /// case-sensitive directories with a case pair, symbolic links, hard
    /// links (one to a boot file, whose first name then goes), an unlinked
    /// open file (gone, and its later writes not recorded).
    fn change_posix(f: &mut RamFs) {
        let w_dir = f.create_posix(ROOT, &w("w"), true, 0o750, 5).unwrap();
        let up = f.create_posix(w_dir, &w("Case"), false, 0o600, 5).unwrap();
        f.write(up, 0, b"upper", 6).unwrap();
        let low = f.create_posix(w_dir, &w("case"), false, 0o644, 5).unwrap();
        f.write(low, 0, b"lower", 6).unwrap();
        f.set_mode(low, 0o640).unwrap();
        f.symlink(w_dir, &w("ln"), b"Case", 5).unwrap();
        f.link(up, w_dir, &w("Hard")).unwrap();
        let keep = f.lookup(&w("\\boot\\keep.txt")).unwrap();
        f.link(keep, w_dir, &w("kept")).unwrap();
        let b = f.lookup(&w("\\boot")).unwrap();
        f.unlink(b, &w("keep.txt")).unwrap();
        let t = f.create_posix(w_dir, &w("tmp"), false, 0o644, 5).unwrap();
        f.open(t, RW, NONE).unwrap();
        f.unlink(w_dir, &w("tmp")).unwrap();
        f.write(t, 0, b"gone", 7).unwrap();
        f.close(t, RW, NONE);
    }

    fn check_posix(f: &RamFs) {
        let w_dir = f.lookup(&w("\\w")).unwrap();
        let d = f.node(w_dir).unwrap();
        assert_eq!((d.mode, d.case_sensitive), (Some(0o750), true));
        let up = f.lookup(&w("\\w\\Case")).unwrap();
        let low = f.lookup(&w("\\w\\case")).unwrap();
        assert_ne!(up, low);
        assert_eq!(content(f, "\\w\\Case").unwrap(), b"upper");
        assert_eq!(content(f, "\\w\\case").unwrap(), b"lower");
        assert_eq!(f.node(up).unwrap().mode, Some(0o600));
        assert_eq!(f.node(low).unwrap().mode, Some(0o640));
        let ln = f.node(f.lookup(&w("\\w\\ln")).unwrap()).unwrap();
        assert_eq!(ln.symlink_target(), Some(&b"Case"[..]));
        assert_eq!(f.lookup(&w("\\w\\Hard")), Ok(up));
        assert_eq!(f.node(up).unwrap().nlink(), 2);
        assert!(f.lookup(&w("\\boot\\keep.txt")).is_err());
        assert_eq!(content(f, "\\w\\kept").unwrap(), b"boot data");
        assert!(f.lookup(&w("\\w\\tmp")).is_err());
    }

    #[test]
    fn posix_changes_replay_and_snapshot() {
        let mut f = booted();
        change_posix(&mut f);
        check_posix(&f);
        let (ops, lost) = f.take_ops();
        assert!(!lost);
        assert!(!ops.iter().any(|op| matches!(op, Op::Write { data, .. } if data == b"gone")));
        let mut g = booted();
        g.recording = false;
        for op in &ops {
            g.apply(op).unwrap();
        }
        check_posix(&g);
        let snap = f.snapshot().unwrap();
        let mut h = booted();
        h.recording = false;
        for op in &snap {
            h.apply(op).unwrap();
        }
        check_posix(&h);
        assert_eq!(h.snapshot().unwrap(), snap);
    }

    /// NTFS's rules for FSCTL_SET_REPARSE_POINT and
    /// FSCTL_DELETE_REPARSE_POINT (Wine's ntdll test_reparse_points, run on
    /// Windows): an empty directory or a file may carry one, a symbolic
    /// link's file no contents; another tag is a mismatch; the attribute
    /// shows it and survives a change of the others.
    #[test]
    fn reparse_points_follow_ntfs_rules() {
        let mut f = fs();
        let file = f.create(ROOT, &w("l"), false, 1).unwrap();
        f.set_reparse(file, TAG_SYMLINK, b"target").unwrap();
        let n = f.node(file).unwrap();
        assert_eq!(n.reparse(), Some(&Reparse { tag: TAG_SYMLINK, data: b"target".to_vec() }));
        assert_eq!(n.attributes, ATTRIBUTE_ARCHIVE | ATTRIBUTE_REPARSE_POINT);
        f.set_basic(file, Times::default(), Some(0x80)).unwrap();
        assert_eq!(f.node(file).unwrap().attributes, ATTRIBUTE_REPARSE_POINT);
        // The same tag replaces it; another is a mismatch.
        f.set_reparse(file, TAG_SYMLINK, b"other").unwrap();
        assert_eq!(f.set_reparse(file, 0xa000_0003, b"x"), Err(FsError::ReparseTagMismatch));
        assert_eq!(f.delete_reparse(file, 0xa000_0003), Err(FsError::ReparseTagMismatch));
        f.delete_reparse(file, TAG_SYMLINK).unwrap();
        assert_eq!(f.delete_reparse(file, TAG_SYMLINK), Err(FsError::NotReparsePoint));
        assert_eq!(f.node(file).unwrap().attributes & ATTRIBUTE_REPARSE_POINT, 0);
        // A symbolic link's file has no contents; another tag's may.
        f.write(file, 0, b"data", 2).unwrap();
        assert_eq!(f.set_reparse(file, TAG_SYMLINK, b"t"), Err(FsError::ReparseHasData));
        f.set_reparse(file, 0xbeef, b"guid and data").unwrap();
        // A directory must be empty.
        let dir = f.create(ROOT, &w("d"), true, 1).unwrap();
        f.create(dir, &w("x"), false, 1).unwrap();
        assert_eq!(f.set_reparse(dir, 0xa000_0003, b"j"), Err(FsError::DirectoryNotEmpty));
        f.unlink(dir, &w("x")).unwrap();
        f.set_reparse(dir, 0xa000_0003, b"j").unwrap();
        assert_eq!(f.node(dir).unwrap().attributes, ATTRIBUTE_DIRECTORY | ATTRIBUTE_REPARSE_POINT);
    }

    /// Reparse points are kept across reboots: in the change log and in a
    /// snapshot, whose replay has them again (and removed ones gone).
    #[test]
    fn reparse_points_replay_and_snapshot() {
        let mut f = booted();
        let l = f.create(ROOT, &w("link"), false, 3).unwrap();
        f.set_reparse(l, TAG_SYMLINK, b"\\??\\C:\\t").unwrap();
        let d = f.create(ROOT, &w("junction"), true, 3).unwrap();
        f.set_reparse(d, 0xa000_0003, b"j").unwrap();
        let gone = f.create(ROOT, &w("was"), false, 3).unwrap();
        f.set_reparse(gone, TAG_SYMLINK, b"x").unwrap();
        f.delete_reparse(gone, TAG_SYMLINK).unwrap();
        let check = |g: &RamFs| {
            let tag = |p: &str| g.node(g.lookup(&w(p)).unwrap()).unwrap().reparse().map(|r| r.tag);
            assert_eq!(tag("\\link"), Some(TAG_SYMLINK));
            assert_eq!(tag("\\junction"), Some(0xa000_0003));
            assert_eq!(tag("\\was"), None);
            let n = g.node(g.lookup(&w("\\link")).unwrap()).unwrap();
            assert_eq!(n.reparse().unwrap().data, b"\\??\\C:\\t");
            assert_ne!(n.attributes & ATTRIBUTE_REPARSE_POINT, 0);
        };
        let (ops, lost) = f.take_ops();
        assert!(!lost);
        let mut g = booted();
        g.recording = false;
        for op in &ops {
            g.apply(op).unwrap();
        }
        check(&g);
        let snap = f.snapshot().unwrap();
        let mut h = booted();
        h.recording = false;
        for op in &snap {
            h.apply(op).unwrap();
        }
        check(&h);
        assert_eq!(h.snapshot().unwrap(), snap);
    }

    #[test]
    fn an_anonymous_file_lives_while_open_and_is_never_recorded() {
        let mut f = booted();
        let id = f.create_anonymous(&w("memfd:x"), 0o777, Some(0), RW, NONE, 5).unwrap();
        let n = f.node(id).unwrap();
        assert!(n.detached() && n.nlink() == 0 && n.mode == Some(0o777));
        assert!(f.find(ROOT, &w("memfd:x")).unwrap().is_none());
        f.write(id, 0, b"pool", 6).unwrap();
        f.set_size(id, 8192, 6).unwrap();
        assert_eq!(f.node(id).unwrap().size(), 8192);
        let (ops, _) = f.take_ops();
        assert!(ops.is_empty(), "{ops:?}");
        assert!(f.snapshot().unwrap().iter().all(|op| !matches!(op, Op::Write { .. })));
        f.close(id, RW, NONE);
        assert!(f.node(id).is_none());
    }

    #[test]
    fn seals_refuse_what_they_seal() {
        let mut f = fs();
        let id = f.create_anonymous(&w("m"), 0o777, Some(0), RW, NONE, 1).unwrap();
        f.set_size(id, 4096, 1).unwrap();
        f.add_seals(id, SEAL_SHRINK).unwrap();
        assert_eq!(f.set_size(id, 100, 1), Err(FsError::Sealed));
        f.set_size(id, 8192, 1).unwrap();
        f.add_seals(id, SEAL_GROW).unwrap();
        assert_eq!(f.set_size(id, 12288, 1), Err(FsError::Sealed));
        assert_eq!(f.write(id, 8190, b"abc", 1), Err(FsError::Sealed));
        assert_eq!(f.write(id, 8189, b"abc", 1), Ok(3));
        // SEAL_WRITE waits for no writable mapping.
        f.map(id, 8192, true).unwrap();
        assert_eq!(f.add_seals(id, SEAL_WRITE), Err(FsError::WriteMapped));
        f.unmap(id, true);
        f.add_seals(id, SEAL_WRITE).unwrap();
        assert_eq!(f.write(id, 0, b"x", 1), Err(FsError::Sealed));
        assert_eq!(f.map(id, 8192, true), Err(FsError::Sealed));
        f.map(id, 8192, false).unwrap();
        f.unmap(id, false);
        assert_eq!(f.node(id).unwrap().seals(), Some(SEAL_SHRINK | SEAL_GROW | SEAL_WRITE));
        f.add_seals(id, SEAL_SEAL).unwrap();
        assert_eq!(f.add_seals(id, 0), Err(FsError::Sealed));
        // SEAL_EXEC keeps the execute bits.
        let x = f.create_anonymous(&w("x"), 0o666, Some(SEAL_EXEC), RW, NONE, 1).unwrap();
        assert_eq!(f.set_mode(x, 0o755), Err(FsError::Sealed));
        f.set_mode(x, 0o644).unwrap();
        // Other files cannot be sealed.
        let plain = f.create(ROOT, &w("plain"), false, 1).unwrap();
        assert_eq!(f.add_seals(plain, SEAL_SHRINK), Err(FsError::NotSealable));
        assert_eq!(f.node(plain).unwrap().seals(), None);
    }

    #[test]
    fn a_socket_node_has_a_name_but_no_contents_and_no_record() {
        let mut f = booted();
        let d = f.create_posix(ROOT, &w("run"), true, 0o755, 2).unwrap();
        let s = f.create_socket(d, &w("wayland-0"), 0o755, 3).unwrap();
        let n = f.node(s).unwrap();
        assert!(n.is_socket() && !n.is_dir() && n.size() == 0 && n.mode == Some(0o755));
        assert_ne!(n.attributes & ATTRIBUTE_REPARSE_POINT, 0);
        assert_eq!(f.create_socket(d, &w("wayland-0"), 0o755, 3), Err(FsError::Exists));
        assert_eq!(f.read(s, 0, &mut [0; 4]), Err(FsError::Socket));
        assert_eq!(f.write(s, 0, b"x", 4), Err(FsError::Socket));
        let (ops, _) = f.take_ops();
        assert!(
            !ops.iter()
                .any(|op| matches!(op, Op::Create { path, .. } if path.ends_with(&w("wayland-0"))))
        );
        assert!(
            !f.snapshot()
                .unwrap()
                .iter()
                .any(|op| matches!(op, Op::Create { path, .. } if path.ends_with(&w("wayland-0"))))
        );
        // Held open by its socket, its id is not reused after an unlink.
        f.open(s, Access::default(), Share::default()).unwrap();
        f.unlink(d, &w("wayland-0")).unwrap();
        assert!(f.node(s).is_some_and(Node::detached));
        let again = f.create_socket(d, &w("wayland-0"), 0o755, 4).unwrap();
        assert_ne!(again, s);
        f.close(s, Access::default(), Share::default());
        assert!(f.node(s).is_none());
    }
}
