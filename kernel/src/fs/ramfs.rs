//! An in-memory file system: a tree of directories and files, with
//! case-insensitive (case-preserving) names and NT-style share access.
//!
//! Pure data structure (host-tested); the NT layer maps it to file objects.

use alloc::vec::Vec;

use crate::mm::heap::TryVec;

pub type NodeId = usize;

/// The root directory.
pub const ROOT: NodeId = 0;

pub const ATTRIBUTE_READONLY: u32 = 0x01;
pub const ATTRIBUTE_DIRECTORY: u32 = 0x10;
pub const ATTRIBUTE_ARCHIVE: u32 = 0x20;

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
    NoMemory,
}

enum Data {
    /// Immutable contents owned elsewhere (boot modules).
    Static(&'static [u8]),
    Owned(Vec<u8>),
}

impl Data {
    fn bytes(&self) -> &[u8] {
        match self {
            Data::Static(s) => s,
            Data::Owned(v) => v,
        }
    }

    /// Turns static data into owned data before modification.
    fn owned(&mut self) -> Result<&mut Vec<u8>, FsError> {
        if let Data::Static(s) = *self {
            let mut v = Vec::new();
            v.try_extend_from_slice(s).map_err(|_| FsError::NoMemory)?;
            *self = Data::Owned(v);
        }
        match self {
            Data::Owned(v) => Ok(v),
            Data::Static(_) => unreachable!(),
        }
    }
}

enum Kind {
    Directory(Vec<NodeId>),
    File(Data),
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

pub struct Node {
    parent: NodeId,
    name: Vec<u16>,
    kind: Kind,
    pub attributes: u32,
    /// NT times (100 ns since 1601).
    pub created: u64,
    pub written: u64,
    opens: Opens,
    delete_pending: bool,
}

impl Node {
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, Kind::Directory(_))
    }

    pub fn name(&self) -> &[u16] {
        &self.name
    }

    pub fn parent(&self) -> NodeId {
        self.parent
    }

    pub fn size(&self) -> u64 {
        match &self.kind {
            Kind::File(d) => d.bytes().len() as u64,
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
            written: now,
            opens: Opens::default(),
            delete_pending: false,
        };
        nodes.try_push(Some(root)).map_err(|_| FsError::NoMemory)?;
        Ok(Self { nodes })
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id)?.as_ref()
    }

    fn node_mut(&mut self, id: NodeId) -> Result<&mut Node, FsError> {
        self.nodes.get_mut(id).and_then(Option::as_mut).ok_or(FsError::NotFound)
    }

    fn children(&self, dir: NodeId) -> Result<&[NodeId], FsError> {
        match &self.node(dir).ok_or(FsError::NotFound)?.kind {
            Kind::Directory(c) => Ok(c),
            Kind::File(_) => Err(FsError::NotADirectory),
        }
    }

    /// Finds `name` in directory `dir`.
    pub fn find(&self, dir: NodeId, name: &[u16]) -> Result<Option<NodeId>, FsError> {
        let children = self.children(dir)?;
        Ok(children
            .iter()
            .copied()
            .find(|&c| self.node(c).is_some_and(|n| same_name(&n.name, name))))
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

    /// Creates `name` in `parent`.
    pub fn create(
        &mut self,
        parent: NodeId,
        name: &[u16],
        directory: bool,
        now: u64,
    ) -> Result<NodeId, FsError> {
        if self.find(parent, name)?.is_some() {
            return Err(FsError::Exists);
        }
        let mut owned_name = Vec::new();
        owned_name.try_extend_from_slice(name).map_err(|_| FsError::NoMemory)?;
        let (kind, attributes) = if directory {
            (Kind::Directory(Vec::new()), ATTRIBUTE_DIRECTORY)
        } else {
            (Kind::File(Data::Owned(Vec::new())), ATTRIBUTE_ARCHIVE)
        };
        let node = Node {
            parent,
            name: owned_name,
            kind,
            attributes,
            created: now,
            written: now,
            opens: Opens::default(),
            delete_pending: false,
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
        Ok(id)
    }

    /// Adds a read-only file whose contents live elsewhere.
    pub fn add_static_file(
        &mut self,
        parent: NodeId,
        name: &[u16],
        data: &'static [u8],
        now: u64,
    ) -> Result<NodeId, FsError> {
        let id = self.create(parent, name, false, now)?;
        let node = self.node_mut(id)?;
        node.kind = Kind::File(Data::Static(data));
        node.attributes = ATTRIBUTE_READONLY | ATTRIBUTE_ARCHIVE;
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
    /// is pending.
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
        if node.opens.handles == 0 && node.delete_pending {
            self.remove(id);
        }
    }

    fn remove(&mut self, id: NodeId) {
        let Some(node) = self.nodes.get_mut(id).and_then(Option::take) else { return };
        if let Some(Some(Node { kind: Kind::Directory(children), .. })) =
            self.nodes.get_mut(node.parent)
        {
            children.retain(|&c| c != id);
        }
    }

    /// The entries of directory `dir` in directory order.
    pub fn list(&self, dir: NodeId) -> Result<Vec<NodeId>, FsError> {
        let mut out = Vec::new();
        out.try_extend_from_slice(self.children(dir)?).map_err(|_| FsError::NoMemory)?;
        let name = |id: NodeId| self.node(id).map_or(&[][..], |n| n.name.as_slice());
        out.sort_unstable_by(|&a, &b| collate(name(a), name(b)));
        Ok(out)
    }

    /// Moves `id` to `name` in directory `parent`. An existing target is
    /// replaced only if `replace` and it is a file nobody has open.
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
                Kind::File(_) => return Err(FsError::NotADirectory),
            }
        }
        if let Some(t) = target
            && t != id
        {
            self.remove(t);
        }
        if old_parent != parent {
            if let Kind::Directory(c) = &mut self.node_mut(old_parent)?.kind {
                c.retain(|&x| x != id);
            }
            if let Kind::Directory(c) = &mut self.node_mut(parent)?.kind {
                let _ = c.try_push(id);
            }
        }
        let node = self.node_mut(id)?;
        node.parent = parent;
        node.name = owned;
        Ok(())
    }

    /// Marks a node for deletion when its last open closes.
    pub fn set_delete_pending(&mut self, id: NodeId, pending: bool) -> Result<(), FsError> {
        if pending && id == ROOT {
            return Err(FsError::DirectoryNotEmpty);
        }
        if pending && !self.children(id).map_or(true, <[NodeId]>::is_empty) {
            return Err(FsError::DirectoryNotEmpty);
        }
        self.node_mut(id)?.delete_pending = pending;
        Ok(())
    }

    pub fn delete_pending(&self, id: NodeId) -> bool {
        self.node(id).is_some_and(|n| n.delete_pending)
    }

    /// Reads at `offset`; returns the number of bytes read (0 at or past EOF).
    pub fn read(&self, id: NodeId, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let node = self.node(id).ok_or(FsError::NotFound)?;
        let Kind::File(data) = &node.kind else { return Err(FsError::IsADirectory) };
        let bytes = data.bytes();
        let Ok(start) = usize::try_from(offset) else { return Ok(0) };
        if start >= bytes.len() {
            return Ok(0);
        }
        let n = buf.len().min(bytes.len() - start);
        buf[..n].copy_from_slice(&bytes[start..start + n]);
        Ok(n)
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
        let Kind::File(contents) = &mut node.kind else { return Err(FsError::IsADirectory) };
        let v = contents.owned()?;
        let start = usize::try_from(offset).map_err(|_| FsError::NoMemory)?;
        let end = start.checked_add(data.len()).ok_or(FsError::NoMemory)?;
        if end > v.len() {
            v.try_reserve(end - v.len()).map_err(|_| FsError::NoMemory)?;
            v.resize(end, 0);
        }
        v[start..end].copy_from_slice(data);
        node.written = now;
        Ok(data.len())
    }

    /// Truncates or extends (with zeros) to `size`.
    pub fn set_size(&mut self, id: NodeId, size: u64, now: u64) -> Result<(), FsError> {
        let node = self.node_mut(id)?;
        let Kind::File(contents) = &mut node.kind else { return Err(FsError::IsADirectory) };
        let v = contents.owned()?;
        let size = usize::try_from(size).map_err(|_| FsError::NoMemory)?;
        if size > v.len() {
            v.try_reserve(size - v.len()).map_err(|_| FsError::NoMemory)?;
        }
        v.resize(size, 0);
        node.written = now;
        Ok(())
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
    fn static_files_copy_on_write() {
        static DATA: &[u8] = b"boot module";
        let mut f = fs();
        let id = f.add_static_file(ROOT, &w("m.exe"), DATA, 1).unwrap();
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
}
