//! The configuration manager: an in-memory registry under `\Registry`.
//!
//! Keys form one tree (`\REGISTRY`, with `MACHINE` and `USER` below it),
//! kept in an arena under one lock. A key object (an open handle) names a
//! node by index; a deleted key stays in the arena, marked deleted, while
//! key objects refer to it (they then get STATUS_KEY_DELETED) and is freed
//! with the last one. Lookups are case-insensitive and case-preserving, as
//! on Windows; subkeys are kept in name order (case-insensitive), the order
//! enumeration returns, and values in the order they were first set.
//!
//! The tree starts from `registry.seed` (the keys real programs read). It
//! lives in memory only: nothing is saved across boots, so the volatile
//! option only matters for its rule that a volatile key's children must be
//! volatile. There are no security descriptors yet (every caller may do
//! everything its handle's access allows), no symbolic link keys (the seed
//! has `CurrentControlSet` itself), no change notifications and no
//! WOW64 views. See docs/ARCHITECTURE.md, "Registry".

use alloc::vec::Vec;

use super::object::{GenericMapping, READ_CONTROL, STANDARD_RIGHTS_REQUIRED};
use super::status::NtStatus;
use crate::mm::heap::{AllocError, TryVec};
use crate::sync::IrqMutex;

pub const KEY_QUERY_VALUE: u32 = 0x0001;
pub const KEY_SET_VALUE: u32 = 0x0002;
pub const KEY_CREATE_SUB_KEY: u32 = 0x0004;
pub const KEY_ENUMERATE_SUB_KEYS: u32 = 0x0008;
pub const KEY_NOTIFY: u32 = 0x0010;
pub const KEY_CREATE_LINK: u32 = 0x0020;
/// KEY_WOW64_64KEY | KEY_WOW64_32KEY: views, not rights (there is one view).
pub const KEY_WOW64_RES: u32 = 0x0300;
pub const KEY_ALL_ACCESS: u32 = STANDARD_RIGHTS_REQUIRED | 0x3f;

/// KEY_READ / KEY_WRITE / KEY_EXECUTE / KEY_ALL_ACCESS (winnt.h).
pub const KEY_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL | KEY_QUERY_VALUE | KEY_ENUMERATE_SUB_KEYS | KEY_NOTIFY,
    write: READ_CONTROL | KEY_SET_VALUE | KEY_CREATE_SUB_KEY,
    execute: READ_CONTROL | KEY_QUERY_VALUE | KEY_ENUMERATE_SUB_KEYS | KEY_NOTIFY,
    all: KEY_ALL_ACCESS,
};

/// STATUS_KEY_DELETED: the key was deleted while open.
pub const KEY_DELETED: NtStatus = NtStatus(0xc000_017c);
/// STATUS_CANNOT_DELETE: a key with subkeys, or a hive root.
pub const CANNOT_DELETE: NtStatus = NtStatus(0xc000_0121);
/// STATUS_CHILD_MUST_BE_VOLATILE.
pub const CHILD_MUST_BE_VOLATILE: NtStatus = NtStatus(0xc000_0181);

/// REG_OPTION_VOLATILE (CreateOptions of NtCreateKey).
pub const REG_OPTION_VOLATILE: u32 = 1;
/// REG_CREATED_NEW_KEY / REG_OPENED_EXISTING_KEY (NtCreateKey's Disposition).
pub const REG_CREATED_NEW_KEY: u32 = 1;
pub const REG_OPENED_EXISTING_KEY: u32 = 2;

/// Longest key name component and value name, in characters (Windows'
/// documented limits).
const MAX_KEY_NAME: usize = 255;
const MAX_VALUE_NAME: usize = 16_383;

const BACKSLASH: u16 = b'\\' as u16;

/// A key's index in the arena.
pub type KeyId = usize;

/// The root, `\REGISTRY`.
pub const ROOT: KeyId = 0;

pub struct Value {
    pub name: Vec<u16>,
    pub kind: u32,
    pub data: Vec<u8>,
}

struct Node {
    name: Vec<u16>,
    parent: KeyId,
    /// In name order (case-insensitive).
    children: Vec<KeyId>,
    values: Vec<Value>,
    class: Vec<u16>,
    /// LastWriteTime (100 ns units since 1601).
    last_write: u64,
    volatile: bool,
    deleted: bool,
    /// Key objects referring to this node.
    opens: usize,
}

/// The registry tree.
pub struct Registry {
    nodes: Vec<Option<Node>>,
}

fn upcase(c: u16) -> u16 {
    // TODO: the full Unicode upcase table, as the configuration manager uses.
    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c }
}

/// Compares names as the registry does (case-insensitively).
fn compare(a: &[u16], b: &[u16]) -> core::cmp::Ordering {
    a.iter().map(|&c| upcase(c)).cmp(b.iter().map(|&c| upcase(c)))
}

fn same(a: &[u16], b: &[u16]) -> bool {
    compare(a, b).is_eq()
}

fn copy(s: &[u16]) -> Result<Vec<u16>, AllocError> {
    let mut v = Vec::new();
    v.try_reserve_exact(s.len()).map_err(|_| AllocError)?;
    v.try_extend_from_slice(s)?;
    Ok(v)
}

fn nomem(_: AllocError) -> NtStatus {
    NtStatus::NO_MEMORY
}

impl Registry {
    /// A tree with only `\REGISTRY`, `\REGISTRY\MACHINE` and `\REGISTRY\USER`.
    pub fn new() -> Result<Registry, AllocError> {
        let mut r = Registry { nodes: Vec::new() };
        r.nodes.try_push(Some(Node {
            name: copy(&wide::<8>("REGISTRY"))?,
            parent: ROOT,
            children: Vec::new(),
            values: Vec::new(),
            class: Vec::new(),
            last_write: 0,
            volatile: false,
            deleted: false,
            opens: 1, // never freed
        }))?;
        for name in ["MACHINE", "USER"] {
            let w = wide::<8>(name);
            r.create_child(ROOT, &w[..name.len()], false, &[], 0).map_err(|_| AllocError)?;
        }
        Ok(r)
    }

    fn node(&self, id: KeyId) -> Result<&Node, NtStatus> {
        match self.nodes.get(id) {
            Some(Some(n)) if !n.deleted => Ok(n),
            Some(Some(_)) => Err(KEY_DELETED),
            _ => Err(NtStatus::INVALID_HANDLE),
        }
    }

    fn node_mut(&mut self, id: KeyId) -> Result<&mut Node, NtStatus> {
        match self.nodes.get_mut(id) {
            Some(Some(n)) if !n.deleted => Ok(n),
            Some(Some(_)) => Err(KEY_DELETED),
            _ => Err(NtStatus::INVALID_HANDLE),
        }
    }

    /// The child of `parent` called `name`.
    fn child(&self, parent: KeyId, name: &[u16]) -> Result<Option<KeyId>, NtStatus> {
        let node = self.node(parent)?;
        let found = node.children.binary_search_by(|&c| match &self.nodes[c] {
            Some(n) => compare(&n.name, name),
            None => core::cmp::Ordering::Less,
        });
        Ok(found.ok().map(|i| node.children[i]))
    }

    /// Splits `path` into its components; an empty component (`a\\b`) or one
    /// over 255 characters is invalid.
    fn components(path: &[u16]) -> impl Iterator<Item = Result<&[u16], NtStatus>> {
        let trimmed = match path.last() {
            Some(&BACKSLASH) => &path[..path.len() - 1],
            _ => path,
        };
        let empty = trimmed.is_empty();
        trimmed.split(|&c| c == BACKSLASH).filter(move |_| !empty).map(|c| {
            if c.is_empty() || c.len() > MAX_KEY_NAME {
                Err(NtStatus::OBJECT_NAME_INVALID)
            } else {
                Ok(c)
            }
        })
    }

    /// The key `path` names below `start` (an empty path is `start`).
    pub fn lookup(&self, start: KeyId, path: &[u16]) -> Result<KeyId, NtStatus> {
        self.node(start)?;
        let mut at = start;
        for component in Self::components(path) {
            at = self.child(at, component?)?.ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
        }
        Ok(at)
    }

    /// Resolves an absolute name (`\Registry\...`) to (start, rest): the
    /// first component must be `REGISTRY`.
    pub fn absolute(path: &[u16]) -> Result<&[u16], NtStatus> {
        let Some((&BACKSLASH, rest)) = path.split_first() else {
            return Err(NtStatus::OBJECT_PATH_SYNTAX_BAD);
        };
        let end = rest.iter().position(|&c| c == BACKSLASH).unwrap_or(rest.len());
        if !same(&rest[..end], &wide::<8>("REGISTRY")[..8]) {
            return Err(NtStatus::OBJECT_PATH_NOT_FOUND);
        }
        Ok(rest.get(end + 1..).unwrap_or(&[]))
    }

    fn create_child(
        &mut self,
        parent: KeyId,
        name: &[u16],
        volatile: bool,
        class: &[u16],
        now: u64,
    ) -> Result<KeyId, NtStatus> {
        let parent_volatile = self.node(parent)?.volatile;
        if parent_volatile && !volatile {
            return Err(CHILD_MUST_BE_VOLATILE);
        }
        let node = Node {
            name: copy(name).map_err(nomem)?,
            parent,
            children: Vec::new(),
            values: Vec::new(),
            class: copy(class).map_err(nomem)?,
            last_write: now,
            volatile,
            deleted: false,
            opens: 0,
        };
        // Room first, so that a failure leaves the tree as it was.
        self.node_mut(parent)?.children.try_reserve(1).map_err(|_| NtStatus::NO_MEMORY)?;
        let id = match self.nodes.iter().position(Option::is_none) {
            Some(free) => {
                self.nodes[free] = Some(node);
                free
            }
            None => {
                self.nodes.try_push(Some(node)).map_err(nomem)?;
                self.nodes.len() - 1
            }
        };
        let at = {
            let p = self.node(parent)?;
            p.children.partition_point(|&c| match &self.nodes[c] {
                Some(n) => compare(&n.name, name).is_lt(),
                None => true,
            })
        };
        let p = self.node_mut(parent)?;
        let _ = p.children.try_insert(at, id);
        p.last_write = now;
        Ok(id)
    }

    /// NtCreateKey: opens `path` below `start`, or creates its last
    /// component (the others must exist). Returns (key, created).
    pub fn create(
        &mut self,
        start: KeyId,
        path: &[u16],
        options: u32,
        class: &[u16],
        now: u64,
    ) -> Result<(KeyId, bool), NtStatus> {
        self.node(start)?;
        let mut components = Self::components(path).peekable();
        let mut at = start;
        while let Some(component) = components.next() {
            let component = component?;
            match self.child(at, component)? {
                Some(next) => at = next,
                None if components.peek().is_none() => {
                    let volatile = options & REG_OPTION_VOLATILE != 0;
                    return Ok((self.create_child(at, component, volatile, class, now)?, true));
                }
                None => return Err(NtStatus::OBJECT_NAME_NOT_FOUND),
            }
        }
        Ok((at, false))
    }

    /// A key object now refers to `id`.
    pub fn retain(&mut self, id: KeyId) {
        if let Some(Some(n)) = self.nodes.get_mut(id) {
            n.opens += 1;
        }
    }

    /// A key object referring to `id` is gone; frees a deleted key with it.
    pub fn release(&mut self, id: KeyId) {
        if let Some(Some(n)) = self.nodes.get_mut(id) {
            n.opens = n.opens.saturating_sub(1);
            if n.deleted && n.opens == 0 {
                self.nodes[id] = None;
            }
        }
    }

    /// NtDeleteKey: only a key without subkeys, and not a hive root.
    pub fn delete_key(&mut self, id: KeyId, now: u64) -> Result<(), NtStatus> {
        let node = self.node(id)?;
        if !node.children.is_empty() || id == ROOT || node.parent == ROOT {
            return Err(CANNOT_DELETE);
        }
        let parent = node.parent;
        let p = self.node_mut(parent)?;
        p.children.retain(|&c| c != id);
        p.last_write = now;
        if let Some(Some(n)) = self.nodes.get_mut(id) {
            n.deleted = true;
            n.values = Vec::new();
            if n.opens == 0 {
                self.nodes[id] = None;
            }
        }
        Ok(())
    }

    pub fn value(&self, id: KeyId, name: &[u16]) -> Result<&Value, NtStatus> {
        self.node(id)?
            .values
            .iter()
            .find(|v| same(&v.name, name))
            .ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)
    }

    pub fn value_at(&self, id: KeyId, index: usize) -> Result<&Value, NtStatus> {
        self.node(id)?.values.get(index).ok_or(NtStatus::NO_MORE_ENTRIES)
    }

    /// NtSetValueKey: replaces the value's type and data, or adds it.
    pub fn set_value(
        &mut self,
        id: KeyId,
        name: &[u16],
        kind: u32,
        data: &[u8],
        now: u64,
    ) -> Result<(), NtStatus> {
        if name.len() > MAX_VALUE_NAME {
            return Err(NtStatus::INVALID_PARAMETER);
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(data.len()).map_err(|_| NtStatus::NO_MEMORY)?;
        let _ = bytes.try_extend_from_slice(data);
        let node = self.node_mut(id)?;
        node.last_write = now;
        if let Some(v) = node.values.iter_mut().find(|v| same(&v.name, name)) {
            v.kind = kind;
            v.data = bytes;
            return Ok(());
        }
        let value = Value { name: copy(name).map_err(nomem)?, kind, data: bytes };
        node.values.try_push(value).map_err(nomem)
    }

    /// NtDeleteValueKey.
    pub fn delete_value(&mut self, id: KeyId, name: &[u16], now: u64) -> Result<(), NtStatus> {
        let node = self.node_mut(id)?;
        let at = node
            .values
            .iter()
            .position(|v| same(&v.name, name))
            .ok_or(NtStatus::OBJECT_NAME_NOT_FOUND)?;
        node.values.remove(at);
        node.last_write = now;
        Ok(())
    }

    /// The `index`th subkey (in name order).
    pub fn subkey(&self, id: KeyId, index: usize) -> Result<KeyId, NtStatus> {
        self.node(id)?.children.get(index).copied().ok_or(NtStatus::NO_MORE_ENTRIES)
    }

    /// The full name, `\REGISTRY\...`.
    pub fn full_name(&self, id: KeyId) -> Result<Vec<u16>, NtStatus> {
        let mut chain = Vec::new();
        let mut at = id;
        loop {
            chain.try_push(at).map_err(nomem)?;
            if at == ROOT {
                break;
            }
            at = self.node(at)?.parent;
        }
        let mut name = Vec::new();
        for &k in chain.iter().rev() {
            name.try_push(BACKSLASH).map_err(nomem)?;
            name.try_extend_from_slice(&self.node(k)?.name).map_err(nomem)?;
        }
        Ok(name)
    }

    /// NtQueryKey / NtEnumerateKey information about key `id`, in class
    /// `class`: (the whole structure, the size of its fixed part).
    pub fn key_information(&self, id: KeyId, class: u32) -> Result<(Vec<u8>, usize), NtStatus> {
        let node = self.node(id)?;
        let mut b = Bytes::new();
        let name_bytes = 2 * node.name.len() as u32;
        let class_bytes = 2 * node.class.len() as u32;
        match class {
            // KEY_BASIC_INFORMATION
            0 => {
                b.u64(node.last_write)?;
                b.u32(0)?;
                b.u32(name_bytes)?;
                b.wide(&node.name)?;
                Ok((b.0, 16))
            }
            // KEY_NODE_INFORMATION: the class follows the name.
            1 => {
                b.u64(node.last_write)?;
                b.u32(0)?;
                let offset = if node.class.is_empty() { u32::MAX } else { 24 + name_bytes };
                b.u32(offset)?;
                b.u32(class_bytes)?;
                b.u32(name_bytes)?;
                b.wide(&node.name)?;
                b.wide(&node.class)?;
                Ok((b.0, 24))
            }
            // KEY_FULL_INFORMATION
            2 => {
                let (max_name, max_class) = self.subkey_maxima(node);
                let (max_value_name, max_data) = value_maxima(node);
                b.u64(node.last_write)?;
                b.u32(0)?;
                b.u32(if node.class.is_empty() { u32::MAX } else { 44 })?;
                b.u32(class_bytes)?;
                b.u32(node.children.len() as u32)?;
                b.u32(max_name)?;
                b.u32(max_class)?;
                b.u32(node.values.len() as u32)?;
                b.u32(max_value_name)?;
                b.u32(max_data)?;
                b.wide(&node.class)?;
                Ok((b.0, 44))
            }
            // KEY_NAME_INFORMATION: the full name.
            3 => {
                let full = self.full_name(id)?;
                b.u32(2 * full.len() as u32)?;
                b.wide(&full)?;
                Ok((b.0, 4))
            }
            // KEY_CACHED_INFORMATION
            4 => {
                let (max_name, _) = self.subkey_maxima(node);
                let (max_value_name, max_data) = value_maxima(node);
                b.u64(node.last_write)?;
                b.u32(0)?;
                b.u32(node.children.len() as u32)?;
                b.u32(max_name)?;
                b.u32(node.values.len() as u32)?;
                b.u32(max_value_name)?;
                b.u32(max_data)?;
                b.u32(name_bytes)?;
                Ok((b.0, 36))
            }
            // KEY_FLAGS_INFORMATION (Wow64Flags, KeyFlags, ControlFlags) and
            // KeyHandleTagsInformation: none set.
            5 => Ok((zeros(12)?, 12)),
            7 => Ok((zeros(4)?, 4)),
            _ => Err(NtStatus::INVALID_INFO_CLASS),
        }
    }

    /// Longest subkey name and class, in bytes.
    fn subkey_maxima(&self, node: &Node) -> (u32, u32) {
        node.children.iter().filter_map(|&c| self.nodes[c].as_ref()).fold((0, 0), |(n, c), k| {
            (n.max(2 * k.name.len() as u32), c.max(2 * k.class.len() as u32))
        })
    }

    /// Adds `\`-separated `path` below the root as needed (for the seed).
    fn ensure(&mut self, path: &[u16], now: u64) -> Result<KeyId, NtStatus> {
        let mut at = ROOT;
        for component in Self::components(path) {
            let component = component?;
            at = match self.child(at, component)? {
                Some(next) => next,
                None => self.create_child(at, component, false, &[], now)?,
            };
        }
        Ok(at)
    }

    /// Loads `seed` (see `registry.seed` for the format).
    pub fn load(&mut self, seed: &str, now: u64) -> Result<(), NtStatus> {
        let mut key = None;
        let mut name = Vec::new();
        let mut data = Vec::new();
        for line in seed.lines().map(str::trim) {
            if line.is_empty() || line.starts_with(';') {
                continue;
            }
            if let Some(path) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                let path = utf16(path)?;
                key = Some(self.ensure(Self::absolute(&path)?, now)?);
                continue;
            }
            let id = key.ok_or(NtStatus::INVALID_PARAMETER)?;
            let (value_name, rest) = line.split_once('=').ok_or(NtStatus::INVALID_PARAMETER)?;
            let value_name = value_name.trim();
            name.clear();
            if value_name != "@" {
                let quoted = value_name.strip_prefix('"').and_then(|n| n.strip_suffix('"'));
                name = utf16(quoted.ok_or(NtStatus::INVALID_PARAMETER)?)?;
            }
            let (kind, text) = rest.trim().split_once(':').ok_or(NtStatus::INVALID_PARAMETER)?;
            data.clear();
            let kind = seed_data(kind, text, &mut data)?;
            self.set_value(id, &name, kind, &data, now)?;
        }
        Ok(())
    }
}

/// Longest value name and data, in bytes.
fn value_maxima(node: &Node) -> (u32, u32) {
    node.values
        .iter()
        .fold((0, 0), |(n, d), v| (n.max(2 * v.name.len() as u32), d.max(v.data.len() as u32)))
}

/// NtQueryValueKey / NtEnumerateValueKey information about `value` in
/// class `class`: (the whole structure, the size of its fixed part).
pub fn value_information(value: &Value, class: u32) -> Result<(Vec<u8>, usize), NtStatus> {
    let mut b = Bytes::new();
    let name_bytes = 2 * value.name.len() as u32;
    let data_bytes = value.data.len() as u32;
    match class {
        // KEY_VALUE_BASIC_INFORMATION
        0 => {
            b.u32(0)?;
            b.u32(value.kind)?;
            b.u32(name_bytes)?;
            b.wide(&value.name)?;
            Ok((b.0, 12))
        }
        // KEY_VALUE_FULL_INFORMATION(_ALIGN64): the data follows the name,
        // aligned to 4 (8).
        1 | 3 => {
            let align = if class == 1 { 4 } else { 8 };
            let offset = (20 + name_bytes).next_multiple_of(align);
            b.u32(0)?;
            b.u32(value.kind)?;
            b.u32(offset)?;
            b.u32(data_bytes)?;
            b.u32(name_bytes)?;
            b.wide(&value.name)?;
            b.pad_to(offset as usize)?;
            b.bytes(&value.data)?;
            Ok((b.0, 20))
        }
        // KEY_VALUE_PARTIAL_INFORMATION
        2 => {
            b.u32(0)?;
            b.u32(value.kind)?;
            b.u32(data_bytes)?;
            b.bytes(&value.data)?;
            Ok((b.0, 12))
        }
        // KEY_VALUE_PARTIAL_INFORMATION_ALIGN64: Type, DataLength, Data.
        4 => {
            b.u32(value.kind)?;
            b.u32(data_bytes)?;
            b.bytes(&value.data)?;
            Ok((b.0, 8))
        }
        _ => Err(NtStatus::INVALID_INFO_CLASS),
    }
}

/// How much of an information structure goes to a caller's buffer of
/// `length` bytes: (bytes to copy, status). Less than the fixed part is
/// STATUS_BUFFER_TOO_SMALL (nothing copied); less than the whole is
/// STATUS_BUFFER_OVERFLOW (what fits is copied).
pub fn fit(total: usize, fixed: usize, length: usize) -> (usize, NtStatus) {
    if length < fixed {
        (0, NtStatus::BUFFER_TOO_SMALL)
    } else if length < total {
        (length, NtStatus::BUFFER_OVERFLOW)
    } else {
        (total, NtStatus::SUCCESS)
    }
}

struct Bytes(Vec<u8>);

impl Bytes {
    fn new() -> Bytes {
        Bytes(Vec::new())
    }
    fn bytes(&mut self, b: &[u8]) -> Result<(), NtStatus> {
        self.0.try_extend_from_slice(b).map_err(nomem)
    }
    fn u32(&mut self, v: u32) -> Result<(), NtStatus> {
        self.bytes(&v.to_le_bytes())
    }
    fn u64(&mut self, v: u64) -> Result<(), NtStatus> {
        self.bytes(&v.to_le_bytes())
    }
    fn wide(&mut self, s: &[u16]) -> Result<(), NtStatus> {
        self.0.try_reserve(2 * s.len()).map_err(|_| NtStatus::NO_MEMORY)?;
        for c in s {
            self.bytes(&c.to_le_bytes())?;
        }
        Ok(())
    }
    fn pad_to(&mut self, len: usize) -> Result<(), NtStatus> {
        while self.0.len() < len {
            self.0.try_push(0).map_err(nomem)?;
        }
        Ok(())
    }
}

fn zeros(n: usize) -> Result<Vec<u8>, NtStatus> {
    let mut b = Bytes::new();
    b.pad_to(n)?;
    Ok(b.0)
}

/// `s` in UTF-16, for names of up to N ASCII characters (zero-padded).
fn wide<const N: usize>(s: &str) -> [u16; N] {
    let mut w = [0u16; N];
    for (d, c) in w.iter_mut().zip(s.bytes()) {
        *d = c.into();
    }
    w
}

fn utf16(s: &str) -> Result<Vec<u16>, NtStatus> {
    let mut v = Vec::new();
    v.try_reserve_exact(s.len()).map_err(|_| NtStatus::NO_MEMORY)?;
    for c in s.encode_utf16() {
        let _ = v.try_push(c);
    }
    Ok(v)
}

fn push_wide(data: &mut Vec<u8>, s: &str) -> Result<(), NtStatus> {
    for c in s.encode_utf16() {
        data.try_extend_from_slice(&c.to_le_bytes()).map_err(nomem)?;
    }
    Ok(())
}

fn number(text: &str) -> Result<u64, NtStatus> {
    let parsed = match text.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => text.parse(),
    };
    parsed.map_err(|_| NtStatus::INVALID_PARAMETER)
}

/// A seed value's data; returns its REG_* type.
fn seed_data(kind: &str, text: &str, data: &mut Vec<u8>) -> Result<u32, NtStatus> {
    let nul = |data: &mut Vec<u8>| data.try_extend_from_slice(&[0, 0]).map_err(nomem);
    Ok(match kind {
        "sz" | "expand" => {
            push_wide(data, text)?;
            nul(data)?;
            if kind == "sz" { 1 } else { 2 }
        }
        "multi" => {
            for item in text.split('|').filter(|s| !s.is_empty()) {
                push_wide(data, item)?;
                nul(data)?;
            }
            nul(data)?;
            7
        }
        "dword" => {
            let v = u32::try_from(number(text)?).map_err(|_| NtStatus::INVALID_PARAMETER)?;
            data.try_extend_from_slice(&v.to_le_bytes()).map_err(nomem)?;
            4
        }
        "qword" => {
            data.try_extend_from_slice(&number(text)?.to_le_bytes()).map_err(nomem)?;
            11
        }
        "hex" => {
            for byte in text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                let b = u8::from_str_radix(byte, 16).map_err(|_| NtStatus::INVALID_PARAMETER)?;
                data.try_push(b).map_err(nomem)?;
            }
            3
        }
        _ => return Err(NtStatus::INVALID_PARAMETER),
    })
}

/// The registry's keys real programs read at startup.
const SEED: &str = include_str!("registry.seed");

static REGISTRY: IrqMutex<Option<Registry>> = IrqMutex::new(None);

/// Builds the registry from the seed.
pub fn init(now: u64) -> Result<(), NtStatus> {
    let mut registry = Registry::new().map_err(nomem)?;
    registry.load(SEED, now)?;
    let cpu = Processor::this();
    for index in 0..super::platform::online_count() {
        registry.add_processor(index, &cpu, now)?;
    }
    *REGISTRY.lock() = Some(registry);
    Ok(())
}

/// What `HARDWARE\DESCRIPTION\System\CentralProcessor\<n>` says about
/// a processor, from CPUID.
pub struct Processor {
    /// The brand string (leaves 0x80000002-4).
    pub name: [u8; 48],
    /// The vendor (leaf 0), e.g. "GenuineIntel".
    pub vendor: [u8; 12],
    pub family: u32,
    pub model: u32,
    pub stepping: u32,
    /// The base frequency in MHz (leaf 0x16), 0 if unknown.
    pub mhz: u32,
}

impl Processor {
    fn this() -> Processor {
        use super::platform::cpuid;
        let (max, b, c, d) = cpuid(0, 0);
        let mut vendor = [0u8; 12];
        vendor[..4].copy_from_slice(&b.to_le_bytes());
        vendor[4..8].copy_from_slice(&d.to_le_bytes());
        vendor[8..].copy_from_slice(&c.to_le_bytes());
        let (signature, ..) = cpuid(1, 0);
        let base_family = (signature >> 8) & 0xf;
        let mut family = base_family;
        let mut model = (signature >> 4) & 0xf;
        if base_family == 0xf {
            family += (signature >> 20) & 0xff;
        }
        if base_family == 0x6 || base_family == 0xf {
            model |= ((signature >> 16) & 0xf) << 4;
        }
        let mut name = [0u8; 48];
        if cpuid(0x8000_0000, 0).0 >= 0x8000_0004 {
            for (i, leaf) in (0x8000_0002u32..=0x8000_0004).enumerate() {
                let (a, b, c, d) = cpuid(leaf, 0);
                for (j, r) in [a, b, c, d].iter().enumerate() {
                    name[16 * i + 4 * j..16 * i + 4 * j + 4].copy_from_slice(&r.to_le_bytes());
                }
            }
        }
        let mhz = if max >= 0x16 { cpuid(0x16, 0).0 & 0xffff } else { 0 };
        Processor { name, vendor, family, model, stepping: signature & 0xf, mhz }
    }
}

/// Appends `v` in decimal.
fn push_decimal(out: &mut Vec<u8>, v: u32) -> Result<(), NtStatus> {
    let mut digits = [0u8; 10];
    let mut n = 0;
    let mut v = v;
    loop {
        digits[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    for &d in digits[..n].iter().rev() {
        out.try_push(d).map_err(nomem)?;
    }
    Ok(())
}

/// A CPUID string without its NUL padding and leading spaces.
fn trim(s: &[u8]) -> &[u8] {
    let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
    let s = &s[..end];
    let start = s.iter().position(|&b| b != b' ').unwrap_or(s.len());
    &s[start..]
}

fn ascii_sz(text: &[u8]) -> Result<Vec<u8>, NtStatus> {
    let mut data = Vec::new();
    data.try_reserve_exact(2 * text.len() + 2).map_err(|_| NtStatus::NO_MEMORY)?;
    for &b in text.iter().chain(&[0]) {
        let _ = data.try_extend_from_slice(&[b, 0]);
    }
    Ok(data)
}

impl Registry {
    /// Adds `HARDWARE\DESCRIPTION\System\CentralProcessor\<index>` with
    /// the values Windows keeps there.
    pub fn add_processor(&mut self, index: u32, cpu: &Processor, now: u64) -> Result<(), NtStatus> {
        let mut path = Vec::new();
        path.try_extend_from_slice(b"MACHINE\\HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\")
            .map_err(nomem)?;
        push_decimal(&mut path, index)?;
        let wide_path = utf16(core::str::from_utf8(&path).unwrap_or(""))?;
        let key = self.ensure(&wide_path, now)?;
        // "Intel64 Family 6 Model 85 Stepping 7", "AMD64 Family 25 ...".
        let mut identifier = Vec::new();
        let arch: &[u8] = if &cpu.vendor == b"GenuineIntel" { b"Intel64" } else { b"AMD64" };
        for (part, v) in [
            (b" Family " as &[u8], cpu.family),
            (b" Model ", cpu.model),
            (b" Stepping ", cpu.stepping),
        ] {
            if identifier.is_empty() {
                identifier.try_extend_from_slice(arch).map_err(nomem)?;
            }
            identifier.try_extend_from_slice(part).map_err(nomem)?;
            push_decimal(&mut identifier, v)?;
        }
        let name = |s: &str| utf16(s);
        self.set_value(key, &name("ProcessorNameString")?, 1, &ascii_sz(trim(&cpu.name))?, now)?;
        self.set_value(key, &name("VendorIdentifier")?, 1, &ascii_sz(trim(&cpu.vendor))?, now)?;
        self.set_value(key, &name("Identifier")?, 1, &ascii_sz(&identifier)?, now)?;
        self.set_value(key, &name("~MHz")?, 4, &cpu.mhz.to_le_bytes(), now)
    }
}

/// Runs `f` on the registry.
pub fn with<R>(f: impl FnOnce(&mut Registry) -> Result<R, NtStatus>) -> Result<R, NtStatus> {
    match REGISTRY.lock().as_mut() {
        Some(r) => f(r),
        None => Err(NtStatus::OBJECT_PATH_NOT_FOUND),
    }
}

/// The body of a key object: a reference to a node. Made after
/// `Registry::retain` (under the registry lock); dropping it releases the
/// node, which takes the lock, so a key object must never be dropped
/// while the registry is held.
pub struct KeyObject {
    pub id: KeyId,
}

impl Drop for KeyObject {
    fn drop(&mut self) {
        let id = self.id;
        let _ = with(|r| {
            r.release(id);
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn s(v: &[u16]) -> String {
        String::from_utf16_lossy(v)
    }

    fn reg() -> Registry {
        let mut r = Registry::new().unwrap();
        r.load(
            "[\\Registry\\Machine\\SOFTWARE\\Microsoft]\n\"a\"=sz:x\n@=dword:0x10\n\
             [\\REGISTRY\\MACHINE\\software\\Beta]\n",
            7,
        )
        .unwrap();
        r
    }

    #[test]
    fn lookup_is_case_insensitive_and_preserving() {
        let r = reg();
        let path = w("\\registry\\machine\\SOFTWARE\\microsoft");
        let rest = Registry::absolute(&path).unwrap();
        let id = r.lookup(ROOT, rest).unwrap();
        assert_eq!(s(&r.full_name(id).unwrap()), "\\REGISTRY\\MACHINE\\SOFTWARE\\Microsoft");
        assert_eq!(
            r.lookup(ROOT, &w("MACHINE\\SOFTWARE\\")).unwrap(),
            r.lookup(ROOT, &w("machine\\software")).unwrap()
        );
        assert_eq!(r.lookup(ROOT, &w("MACHINE\\nope\\x")), Err(NtStatus::OBJECT_NAME_NOT_FOUND));
        assert_eq!(r.lookup(ROOT, &w("MACHINE\\\\x")), Err(NtStatus::OBJECT_NAME_INVALID));
        assert_eq!(Registry::absolute(&w("\\Device\\x")), Err(NtStatus::OBJECT_PATH_NOT_FOUND));
        assert_eq!(Registry::absolute(&w("Registry")), Err(NtStatus::OBJECT_PATH_SYNTAX_BAD));
        assert_eq!(Registry::absolute(&w("\\Registry")).unwrap(), &[] as &[u16]);
    }

    #[test]
    fn subkeys_in_name_order_values_in_set_order() {
        let mut r = reg();
        let sw = r.lookup(ROOT, &w("MACHINE\\SOFTWARE")).unwrap();
        r.create(sw, &w("alpha"), 0, &[], 1).unwrap();
        r.create(sw, &w("Zed"), 0, &[], 1).unwrap();
        let names: Vec<String> = (0..4)
            .map(|i| {
                let (b, _) = r.key_information(r.subkey(sw, i).unwrap(), 0).unwrap();
                s(&b[16..].chunks(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect::<Vec<_>>())
            })
            .collect();
        assert_eq!(names, ["alpha", "Beta", "Microsoft", "Zed"]);
        assert_eq!(r.subkey(sw, 4), Err(NtStatus::NO_MORE_ENTRIES));
        let ms = r.lookup(sw, &w("MICROSOFT")).unwrap();
        r.set_value(ms, &w("b"), 3, &[1], 2).unwrap();
        r.set_value(ms, &w("A"), 4, &[2, 0, 0, 0], 2).unwrap(); // replaces "a"
        let order: Vec<String> = (0..3).map(|i| s(&r.value_at(ms, i).unwrap().name)).collect();
        assert_eq!(order, ["a", "", "b"]);
        assert_eq!(r.value(ms, &w("a")).unwrap().kind, 4);
        r.delete_value(ms, &w(""), 3).unwrap();
        assert_eq!(r.delete_value(ms, &w(""), 3), Err(NtStatus::OBJECT_NAME_NOT_FOUND));
    }

    #[test]
    fn create_rules() {
        let mut r = reg();
        let sw = r.lookup(ROOT, &w("MACHINE\\SOFTWARE")).unwrap();
        assert_eq!(r.create(sw, &w("x\\y"), 0, &[], 1), Err(NtStatus::OBJECT_NAME_NOT_FOUND));
        let (v, created) = r.create(sw, &w("v"), REG_OPTION_VOLATILE, &w("cls"), 1).unwrap();
        assert!(created);
        assert_eq!(r.create(sw, &w("V"), 0, &[], 1).unwrap(), (v, false));
        assert_eq!(r.create(v, &w("child"), 0, &[], 1), Err(CHILD_MUST_BE_VOLATILE));
        assert!(r.create(v, &w("child"), REG_OPTION_VOLATILE, &[], 1).unwrap().1);
        let (b, fixed) = r.key_information(v, 1).unwrap();
        assert_eq!(fixed, 24);
        assert_eq!(u32::from_le_bytes(b[12..16].try_into().unwrap()), 24 + 2); // ClassOffset
        assert_eq!(u32::from_le_bytes(b[16..20].try_into().unwrap()), 6); // ClassLength
    }

    #[test]
    fn deleted_keys_and_open_references() {
        let mut r = reg();
        let sw = r.lookup(ROOT, &w("MACHINE\\SOFTWARE")).unwrap();
        let ms = r.lookup(sw, &w("Microsoft")).unwrap();
        assert_eq!(r.delete_key(sw, 1), Err(CANNOT_DELETE));
        let machine = r.lookup(ROOT, &w("MACHINE")).unwrap();
        assert_eq!(r.delete_key(machine, 1), Err(CANNOT_DELETE));
        r.retain(ms);
        r.delete_key(ms, 1).unwrap();
        assert_eq!(r.value(ms, &w("a")).err(), Some(KEY_DELETED));
        assert_eq!(r.set_value(ms, &w("a"), 1, &[], 1), Err(KEY_DELETED));
        assert_eq!(r.lookup(sw, &w("Microsoft")), Err(NtStatus::OBJECT_NAME_NOT_FOUND));
        r.release(ms);
        assert_eq!(r.value(ms, &w("a")).err(), Some(NtStatus::INVALID_HANDLE));
        // The freed slot is reused.
        let (again, _) = r.create(sw, &w("New"), 0, &[], 1).unwrap();
        assert_eq!(again, ms);
    }

    #[test]
    fn information_layouts() {
        let r = reg();
        let ms = r.lookup(ROOT, &w("MACHINE\\SOFTWARE\\Microsoft")).unwrap();
        let v = r.value(ms, &w("a")).unwrap();
        let (partial, fixed) = value_information(v, 2).unwrap();
        assert_eq!((partial.len(), fixed), (12 + 4, 12));
        assert_eq!(&partial[4..12], &[1, 0, 0, 0, 4, 0, 0, 0]);
        let (full, _) = value_information(v, 1).unwrap();
        assert_eq!(u32::from_le_bytes(full[8..12].try_into().unwrap()), 24); // 20 + 2 -> 24
        assert_eq!(&full[24..28], &[b'x', 0, 0, 0]);
        let (full64, _) = value_information(v, 3).unwrap();
        assert_eq!(u32::from_le_bytes(full64[8..12].try_into().unwrap()), 24);
        let (b, _) = r.key_information(ms, 2).unwrap();
        assert_eq!(u32::from_le_bytes(b[32..36].try_into().unwrap()), 2); // values
        assert_eq!(u32::from_le_bytes(b[40..44].try_into().unwrap()), 4); // max data
        assert_eq!(fit(24, 12, 4), (0, NtStatus::BUFFER_TOO_SMALL));
        assert_eq!(fit(24, 12, 12), (12, NtStatus::BUFFER_OVERFLOW));
        assert_eq!(fit(24, 12, 64), (24, NtStatus::SUCCESS));
        assert_eq!(r.key_information(ms, 99).err(), Some(NtStatus::INVALID_INFO_CLASS));
    }

    #[test]
    fn processors() {
        let mut r = Registry::new().unwrap();
        let mut name = [0u8; 48];
        name[..9].copy_from_slice(b"  Gala Q9");
        let cpu = Processor {
            name,
            vendor: *b"GenuineIntel",
            family: 6,
            model: 85,
            stepping: 7,
            mhz: 2100,
        };
        r.add_processor(1, &cpu, 0).unwrap();
        let key = r
            .lookup(ROOT, &w("MACHINE\\HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\1"))
            .unwrap();
        let text = |v: &str| {
            let d = &r.value(key, &w(v)).unwrap().data;
            s(&d.chunks(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect::<Vec<_>>())
        };
        assert_eq!(text("ProcessorNameString"), "Gala Q9\0");
        assert_eq!(text("Identifier"), "Intel64 Family 6 Model 85 Stepping 7\0");
        assert_eq!(text("VendorIdentifier"), "GenuineIntel\0");
        assert_eq!(r.value(key, &w("~MHz")).unwrap().data, 2100u32.to_le_bytes());
    }

    #[test]
    fn the_seed_loads() {
        let mut r = Registry::new().unwrap();
        r.load(SEED, 0).unwrap();
        let cv =
            r.lookup(ROOT, &w("MACHINE\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion")).unwrap();
        assert_eq!(
            r.value(cv, &w("CurrentBuild")).unwrap().data,
            b"2\x006\x001\x000\x000\x00\x00\x00"
        );
    }
}
