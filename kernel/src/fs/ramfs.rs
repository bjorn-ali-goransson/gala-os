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
//! - **Unlink of an open node** (`unlink`, Linux's rule): the name goes at
//!   once and the node lives on, *detached*, until its last close; NT's
//!   delete (`set_delete_pending`) keeps the name until the last close.
//!
//! Pure data structure (host-tested); the NT layer maps it to file objects.

use alloc::vec::Vec;

use alloc::boxed::Box;

use super::log::{self, Op};
use crate::mm::heap::{TryVec, try_box};

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
}

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
    /// held the file system for many seconds).
    Paged { pages: Vec<Option<Box<Page>>>, len: usize, backing: &'static [u8] },
}

/// Page `i` made if it was not (from `backing`, else zero).
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
    const EMPTY: Data = Data::Paged { pages: Vec::new(), len: 0, backing: &[] };

    fn len(&self) -> usize {
        match self {
            Data::Static(s) => s.len(),
            Data::Paged { len, .. } => *len,
        }
    }

    fn read_at(&self, start: usize, buf: &mut [u8]) -> usize {
        let len = self.len();
        if start >= len {
            return 0;
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
        n
    }

    /// The pages, after turning static data into (lazy) pages.
    #[allow(clippy::type_complexity)]
    fn paged(
        &mut self,
    ) -> Result<(&mut Vec<Option<Box<Page>>>, &mut usize, &mut &'static [u8]), FsError> {
        if let Data::Static(s) = *self {
            let mut pages = Vec::new();
            let n = s.len().div_ceil(PAGE);
            pages.try_reserve_exact(n).map_err(|_| FsError::NoMemory)?;
            pages.extend(core::iter::repeat_with(|| None).take(n));
            *self = Data::Paged { pages, len: s.len(), backing: s };
        }
        match self {
            Data::Paged { pages, len, backing } => Ok((pages, len, backing)),
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

    fn write_at(&mut self, start: usize, data: &[u8]) -> Result<(), FsError> {
        let end = start.checked_add(data.len()).ok_or(FsError::NoMemory)?;
        let (pages, len, backing) = self.paged()?;
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

    fn set_len(&mut self, size: usize) -> Result<(), FsError> {
        let (pages, len, backing) = self.paged()?;
        Self::cover(pages, size)?;
        if size < *len {
            // Keep bytes past the end zero (mapped views show them): pages
            // not made yet read from the backing, so it ends there too.
            *backing = &backing[..size.min(backing.len())];
            let keep = size.div_ceil(PAGE);
            pages.truncate(keep);
            if !size.is_multiple_of(PAGE)
                && let Some(Some(last)) = pages.get_mut(keep - 1)
            {
                last.0[size % PAGE..].fill(0);
            }
        }
        *len = size;
        Ok(())
    }

    /// The address of page `index`, made if it was not (for mapping).
    fn page_address(&mut self, index: usize) -> Option<usize> {
        match self {
            Data::Paged { pages, backing, .. } => {
                page_mut(pages, backing, index).ok().map(|p| p as *const Page as usize)
            }
            Data::Static(_) => None,
        }
    }
}

enum Kind {
    /// The nodes whose first link is in this directory (more names in it
    /// may be hard links: `RamFs::links`).
    Directory(Vec<NodeId>),
    File(Data),
    /// A symbolic link and its target, as given (bytes of a Linux path).
    Symlink(Vec<u8>),
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
    /// Its names beyond the first (entries of `RamFs::links`).
    extra_links: u32,
    /// Unlinked while open (`unlink`): in no directory, freed at its last
    /// close, never recorded again.
    detached: bool,
    /// Made by the boot (modules), not by a program or the change log.
    boot: bool,
    /// Created or changed since boot (or by the change log): part of a
    /// snapshot of the changes (`snapshot`).
    dirty: bool,
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

    /// A symbolic link's target.
    pub fn symlink_target(&self) -> Option<&[u8]> {
        match &self.kind {
            Kind::Symlink(t) => Some(t),
            _ => None,
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

    pub fn size(&self) -> u64 {
        match &self.kind {
            Kind::File(d) => d.len() as u64,
            Kind::Symlink(t) => t.len() as u64,
            Kind::Directory(_) => 0,
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

/// Case-insensitive wildcard match: `*` any run, `?` one unit, and the DOS
/// forms `<` (as `*`), `>` (as `?`) and `"` (as `.`).
pub fn wildcard_match(mask: &[u16], name: &[u16]) -> bool {
    const STAR: [u16; 2] = [b'*' as u16, b'<' as u16];
    const QM: [u16; 2] = [b'?' as u16, b'>' as u16];
    let (mut m, mut n) = (0, 0);
    // Backtracking point: (mask index after the star, name index).
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        let unit = |c: u16| if c == u16::from(b'"') { u16::from(b'.') } else { c };
        if m < mask.len() && STAR.contains(&mask[m]) {
            star = Some((m + 1, n));
            m += 1;
        } else if m < mask.len() && (QM.contains(&mask[m]) || fold(unit(mask[m])) == fold(name[n]))
        {
            m += 1;
            n += 1;
        } else if let Some((sm, sn)) = star {
            m = sm;
            n = sn + 1;
            star = Some((sm, sn + 1));
        } else {
            return false;
        }
    }
    mask[m..].iter().all(|c| STAR.contains(c))
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
            write_maps: 0,
            mapped_size: 0,
            security: None,
            mode: None,
            case_sensitive: false,
            extra_links: 0,
            detached: false,
            boot: true,
            dirty: false,
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
            if !tree || at == ROOT {
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
            Kind::File(_) | Kind::Symlink(_) => Err(FsError::NotADirectory),
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
        let attributes = match kind {
            Kind::Directory(_) => ATTRIBUTE_DIRECTORY,
            Kind::File(_) => ATTRIBUTE_ARCHIVE,
            Kind::Symlink(_) => ATTRIBUTE_ARCHIVE | ATTRIBUTE_REPARSE_POINT,
        };
        let directory = matches!(kind, Kind::Directory(_));
        let node = Node {
            parent,
            name: owned_name,
            kind,
            attributes,
            created: now,
            accessed: now,
            written: now,
            version: next_version(),
            opens: Opens::default(),
            delete_pending: false,
            map_count: 0,
            write_maps: 0,
            mapped_size: 0,
            security: None,
            mode,
            case_sensitive,
            extra_links: 0,
            detached: false,
            boot: !self.recording && !self.replaying,
            dirty: self.recording || self.replaying,
        };
        let id = match self.nodes.iter().position(Option::is_none) {
            Some(free) => {
                self.nodes[free] = Some(node);
                free
            }
            None => {
                self.nodes.try_push(Some(node)).map_err(|_| FsError::NoMemory)?;
                self.nodes.len() - 1
            }
        };
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
                self.nodes[id] = None;
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

    /// The full path of `id` (`\\a\\b`; `\\` for the root).
    pub fn path_of(&self, id: NodeId) -> Result<Vec<u16>, FsError> {
        let mut chain = Vec::new();
        let mut at = id;
        while at != ROOT {
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
        if dir == ROOT {
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
        if id == ROOT {
            return Err(FsError::AccessDenied);
        }
        // A directory cannot move below itself.
        let mut up = parent;
        loop {
            if up == id {
                return Err(FsError::AccessDenied);
            }
            if up == ROOT {
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
                Kind::File(_) | Kind::Symlink(_) => return Err(FsError::NotADirectory),
            }
        }
        let from = if self.recording { self.path_of(id).ok() } else { None };
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
        let paths = if self.recording {
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
        if self.find(parent, name)?.is_some() {
            return Err(FsError::Exists);
        }
        let owned = copy(name).ok_or(FsError::NoMemory)?;
        let paths = if self.recording {
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
        if id == ROOT {
            return Err(FsError::AccessDenied);
        }
        if self.node(id).is_some_and(Node::is_dir) && !self.is_empty_dir(id)? {
            return Err(FsError::DirectoryNotEmpty);
        }
        let path = if self.recording { Some(self.entry_path(dir, name)) } else { None };
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
        if pending && id == ROOT {
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
        self.node_mut(id)?.mode = Some(mode & 0o7777);
        self.record_node(id, |path, n| Some(Op::SetMode { path, mode: n.mode.unwrap_or(0) }));
        Ok(())
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
            Kind::Directory(_) => return Err(FsError::IsADirectory),
        };
        let Ok(start) = usize::try_from(offset) else { return Ok(0) };
        Ok(data.read_at(start, buf))
    }

    /// Writes at `offset`, extending (zero-filling a gap) as needed.
    pub fn write(
        &mut self,
        id: NodeId,
        offset: u64,
        data: &[u8],
        now: u64,
    ) -> Result<usize, FsError> {
        let node = self.node_mut(id)?;
        let contents = match &mut node.kind {
            Kind::File(contents) => contents,
            Kind::Symlink(_) => return Err(FsError::Symlink),
            Kind::Directory(_) => return Err(FsError::IsADirectory),
        };
        let start = usize::try_from(offset).map_err(|_| FsError::NoMemory)?;
        contents.write_at(start, data)?;
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
        if self.recording {
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
            let dir = node.attributes & ATTRIBUTE_DIRECTORY;
            node.attributes = (a & SETTABLE) | dir;
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
        let node = self.node_mut(id)?;
        if node.map_count > 0 && size < node.mapped_size {
            return Err(FsError::UserMapped);
        }
        let contents = match &mut node.kind {
            Kind::File(contents) => contents,
            Kind::Symlink(_) => return Err(FsError::Symlink),
            Kind::Directory(_) => return Err(FsError::IsADirectory),
        };
        let size = usize::try_from(size).map_err(|_| FsError::NoMemory)?;
        contents.set_len(size)?;
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
        let contents = match &mut node.kind {
            Kind::File(contents) => contents,
            Kind::Symlink(_) => return Err(FsError::Symlink),
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
    }

    /// The address of page `index` of mapped file `id` (see `map`).
    pub fn page_address(&mut self, id: NodeId, index: usize) -> Option<usize> {
        match &mut self.node_mut(id).ok()?.kind {
            Kind::File(data) => data.page_address(index),
            Kind::Directory(_) | Kind::Symlink(_) => None,
        }
    }

    // ---- The change log (fs::log) ----

    fn push_op(&mut self, op: Op) {
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
        if self.recording {
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

    pub fn has_ops(&self) -> bool {
        !self.ops.is_empty() || self.ops_lost
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
            if n.dirty && id != ROOT {
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
                if let Kind::File(data) = &n.kind {
                    let len = data.len();
                    let mut at = 0;
                    while at < len {
                        let k = (len - at).min(log::MAX_WRITE);
                        let mut chunk = Vec::new();
                        chunk.try_reserve_exact(k).map_err(|_| FsError::NoMemory)?;
                        chunk.resize(k, 0);
                        data.read_at(at, &mut chunk);
                        push(Op::Write {
                            path: p()?,
                            offset: at as u64,
                            data: chunk,
                            written: n.written,
                        })?;
                        at += k;
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
        assert_eq!(f.page_address(id, 0), Some(first), "pages do not move");
        assert!(f.page_address(id, 2).is_some());
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
        assert_eq!(f.page_address(id, 3), Some(addr));
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
        assert_eq!(f.page_address(id, 0), None);
        f.map(id, 6, false).unwrap();
        assert!(f.page_address(id, 0).is_some());
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
}
