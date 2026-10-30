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

    pub fn size(&self) -> u64 {
        match &self.kind {
            Kind::File(d) => d.bytes().len() as u64,
            Kind::Directory(_) => 0,
        }
    }
}

/// Case-insensitive comparison (ASCII folding).
// TODO: the full Unicode upcase table NTFS uses.
fn same_name(a: &[u16], b: &[u16]) -> bool {
    let fold = |c: u16| if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c };
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| fold(x) == fold(y))
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

    /// Resolves all but the last of `components` as directories, returning
    /// that directory (for create/open of the last component).
    pub fn parent_of(&self, components: &[&[u16]]) -> Result<NodeId, FsError> {
        let mut dir = ROOT;
        for part in components.iter().take(components.len().saturating_sub(1)) {
            match self.find(dir, part) {
                Ok(Some(next)) if self.node(next).is_some_and(Node::is_dir) => dir = next,
                _ => return Err(FsError::PathNotFound),
            }
        }
        Ok(dir)
    }

    /// Resolves a full path.
    pub fn resolve(&self, components: &[&[u16]]) -> Result<NodeId, FsError> {
        let Some(last) = components.last() else { return Ok(ROOT) };
        let parent = self.parent_of(components)?;
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
        assert_eq!(f.resolve(&parts), Ok(file));
        assert_eq!(f.resolve(&[&w("Nope"), &w("x")]), Err(FsError::PathNotFound));
        assert_eq!(f.resolve(&[users.as_slice(), &w("x")]), Err(FsError::NotFound));
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
        assert_eq!(f.resolve(&[&a, &b]), Ok(id));
    }
}
