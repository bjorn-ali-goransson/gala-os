//! An activation context's sections: the blocks FindActCtxSectionString
//! and FindActCtxSectionGuid hand out (ACTCTX_SECTION_KEYED_DATA's
//! lpSectionBase, lpData pointing inside). Their layouts are Windows'
//! undocumented ones as Wine's conformance tests check them on Windows
//! (`dlls/kernel32/tests/actctx.c`: strsection_header, string_index,
//! guidsection_header, guid_index, the per-section data records;
//! THIRD_PARTY.md). Built once, when the context is made; read-only after.
//!
//! - A string section starts with a 44-byte header (magic "SsHd"
//!   0x64487353, size, count, index_offset, global_offset, global_len),
//!   then the index (24 bytes per key: hash, name_offset, name_len,
//!   data_offset, data_len, roster index), then names and data. The hash
//!   is RtlHashUnicodeString's X65599 of the upper-cased key.
//! - A GUID section's header is 40 bytes (magic "GsHd" 0x64487347, size,
//!   count, index_offset, names_offset, names_len), its index 28 bytes
//!   per key (GUID, data_offset, data_len, roster index).
//! - DLL redirection data: 20 bytes (size, flags 2
//!   PATH_OMITS_ASSEMBLY_ROOT, total path length, path count, path
//!   offset); window class data: 24 bytes (size, 0, versioned name's
//!   length and offset from the record, module name's length and offset
//!   from the section) with the versioned name ("<version>!<class>" or
//!   the class) and the module after it; COM server data: 120 bytes
//!   (size, flags with the misc mask in bits 8-15, threading model,
//!   clsid, alias, clsid again, tlbid, module name length and section
//!   offset, progid length and record offset, CLR data, five OLEMISC
//!   words) with the progid after it, each class indexed twice (its
//!   clsid, then a generated alias GUID the ProgID section points to);
//!   ProgID data: 12 bytes (size, 0, section offset of the alias GUID in
//!   the global area after the header).

use super::manifest::{Model, upper};
use crate::buf::Buf;

const STRING_MAGIC: u32 = 0x6448_7353;
const GUID_MAGIC: u32 = 0x6448_7347;
pub const STRING_HEADER: usize = 44;
const STRING_INDEX: usize = 24;
const GUID_HEADER: usize = 40;
const GUID_INDEX: usize = 28;
const DLL_DATA: usize = 20;
const CLASS_DATA: usize = 24;
pub const COM_DATA: usize = 120;
const PROGID_DATA: usize = 12;

/// ACTIVATION_CONTEXT_DATA_DLL_REDIRECTION_PATH_OMITS_ASSEMBLY_ROOT.
const OMITS_ASSEMBLY_ROOT: u32 = 2;

fn aligned(len: usize) -> usize {
    (len + 3) & !3
}

/// RtlHashUnicodeString(HASH_STRING_ALGORITHM_X65599, case-insensitive).
pub fn hash(s: &[u16]) -> u32 {
    s.iter().fold(0u32, |h, &c| h.wrapping_mul(65599).wrapping_add(u32::from(upper(c))))
}

/// A section being written: zeroed, its size known in advance.
struct Out(Buf<u8>);

impl Out {
    fn new(len: usize) -> Option<Out> {
        let mut b = Buf::new();
        if !b.reserve(len) {
            return None;
        }
        for _ in 0..len {
            b.push(0);
        }
        Some(Out(b))
    }

    fn u32(&mut self, at: usize, v: u32) {
        self.0.as_mut_slice()[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn bytes(&mut self, at: usize, v: &[u8]) {
        self.0.as_mut_slice()[at..at + v.len()].copy_from_slice(v);
    }

    /// `s` and its NUL at `at`.
    fn wide(&mut self, at: usize, s: &[u16]) {
        let b = self.0.as_mut_slice();
        for (i, &c) in s.iter().enumerate() {
            b[at + 2 * i..at + 2 * i + 2].copy_from_slice(&c.to_le_bytes());
        }
        b[at + 2 * s.len()..at + 2 * s.len() + 2].fill(0);
    }

    fn string_header(&mut self, count: usize, index_offset: usize, global: (usize, usize)) {
        self.u32(0, STRING_MAGIC);
        self.u32(4, STRING_HEADER as u32);
        self.u32(20, count as u32);
        self.u32(24, index_offset as u32);
        self.u32(36, global.0 as u32);
        self.u32(40, global.1 as u32);
    }

    /// String index entry `i`: (hash, name at, name bytes, data at, data
    /// bytes, roster index).
    fn string_index(&mut self, index_offset: usize, i: usize, e: [u32; 6]) {
        let at = index_offset + i * STRING_INDEX;
        for (k, v) in e.iter().enumerate() {
            self.u32(at + 4 * k, *v);
        }
    }

    fn guid_index(
        &mut self,
        index_offset: usize,
        i: usize,
        guid: &[u8; 16],
        data: (usize, usize),
        roster: u32,
    ) {
        let at = index_offset + i * GUID_INDEX;
        self.bytes(at, guid);
        self.u32(at + 16, data.0 as u32);
        self.u32(at + 20, data.1 as u32);
        self.u32(at + 24, roster);
    }
}

/// The sections of a context (empty Bufs where it has nothing).
pub struct Sections {
    pub dll: Buf<u8>,
    pub classes: Buf<u8>,
    pub com: Buf<u8>,
    pub progid: Buf<u8>,
}

/// The roster index (1-based) of the assembly a file belongs to.
fn roster(model: &Model, file: u32) -> u32 {
    model.files.as_slice()[file as usize].assembly + 1
}

pub fn build(model: &Model, seed: u32) -> Option<Sections> {
    let com = com_section(model, seed)?;
    let progid = progid_section(model, &com)?;
    Some(Sections { dll: dll_section(model)?, classes: class_section(model)?, com, progid })
}

fn dll_section(model: &Model) -> Option<Buf<u8>> {
    let files = model.files.as_slice();
    if files.is_empty() {
        return Some(Buf::new());
    }
    let names: usize = files.iter().map(|f| aligned(2 * (f.name.len as usize + 1))).sum();
    let index_offset = STRING_HEADER;
    let total = index_offset + files.len() * (STRING_INDEX + DLL_DATA) + names;
    let mut o = Out::new(total)?;
    o.string_header(files.len(), index_offset, (0, 0));
    let mut at = index_offset + files.len() * STRING_INDEX;
    for (i, f) in files.iter().enumerate() {
        let name = model.str(f.name);
        let data = at + aligned(2 * (name.len() + 1));
        o.wide(at, name);
        o.string_index(
            index_offset,
            i,
            [
                hash(name),
                at as u32,
                2 * name.len() as u32,
                data as u32,
                DLL_DATA as u32,
                f.assembly + 1,
            ],
        );
        o.u32(data, DLL_DATA as u32);
        o.u32(data + 4, OMITS_ASSEMBLY_ROOT);
        at = data + DLL_DATA;
    }
    Some(o.0)
}

/// A window class's redirected name: "<assembly version>!<class>" when
/// versioned.
fn versioned_name(
    model: &Model,
    file: u32,
    name: &[u16],
    versioned: bool,
    out: &mut Buf<u16>,
) -> bool {
    if versioned {
        let a =
            &model.assemblies.as_slice()[model.files.as_slice()[file as usize].assembly as usize];
        if !super::manifest::version_text(a.id.version, out) || !out.push(u16::from(b'!')) {
            return false;
        }
    }
    out.push_all(name)
}

fn class_section(model: &Model) -> Option<Buf<u8>> {
    let classes = model.classes.as_slice();
    if classes.is_empty() {
        return Some(Buf::new());
    }
    let mut total = STRING_HEADER;
    for c in classes {
        let mut v = Buf::new();
        if !versioned_name(model, c.file, model.str(c.name), c.versioned, &mut v) {
            return None;
        }
        let module = model.str(model.files.as_slice()[c.file as usize].name);
        total += STRING_INDEX
            + CLASS_DATA
            + aligned(2 * (c.name.len as usize + 1))
            + aligned(2 * (v.len() + 1 + module.len() + 1));
    }
    let mut o = Out::new(total)?;
    let index_offset = STRING_HEADER;
    o.string_header(classes.len(), index_offset, (0, 0));
    let mut at = index_offset + classes.len() * STRING_INDEX;
    for (i, c) in classes.iter().enumerate() {
        let name = model.str(c.name);
        let mut v = Buf::new();
        if !versioned_name(model, c.file, name, c.versioned, &mut v) {
            return None;
        }
        let module = model.str(model.files.as_slice()[c.file as usize].name);
        let data = at + aligned(2 * (name.len() + 1));
        let (vlen, mlen) = (2 * v.len(), 2 * module.len());
        o.wide(at, name);
        o.string_index(
            index_offset,
            i,
            [
                hash(name),
                at as u32,
                2 * name.len() as u32,
                data as u32,
                (CLASS_DATA + vlen + mlen + 4) as u32,
                roster(model, c.file),
            ],
        );
        let module_at = data + CLASS_DATA + vlen + 2;
        o.u32(data, CLASS_DATA as u32);
        o.u32(data + 8, vlen as u32);
        o.u32(data + 12, CLASS_DATA as u32);
        o.u32(data + 16, mlen as u32);
        o.u32(data + 20, module_at as u32);
        o.wide(data + CLASS_DATA, v.as_slice());
        o.wide(module_at, module);
        at = data + CLASS_DATA + aligned(vlen + mlen + 4);
    }
    Some(o.0)
}

/// xorshift32: the alias GUIDs' bits.
fn next(seed: &mut u32) -> u32 {
    let mut x = *seed;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *seed = x;
    x
}

fn com_section(model: &Model, seed: u32) -> Option<Buf<u8>> {
    let coms = model.coms.as_slice();
    if coms.is_empty() {
        return Some(Buf::new());
    }
    let names: usize = coms
        .iter()
        .map(|c| 2 * (model.files.as_slice()[c.file as usize].name.len as usize + 1))
        .sum();
    let names_len = aligned(names);
    let index_offset = GUID_HEADER + names_len;
    let records: usize = coms
        .iter()
        .map(|c| {
            COM_DATA
                + if c.progid.is_empty() { 0 } else { aligned(2 * (c.progid.len as usize + 1)) }
        })
        .sum();
    let total = index_offset + 2 * coms.len() * GUID_INDEX + records;
    let mut o = Out::new(total)?;
    o.u32(0, GUID_MAGIC);
    o.u32(4, GUID_HEADER as u32);
    o.u32(20, 2 * coms.len() as u32);
    o.u32(24, index_offset as u32);
    o.u32(32, GUID_HEADER as u32);
    o.u32(36, names_len as u32);
    let mut module_at = GUID_HEADER;
    let mut at = index_offset + 2 * coms.len() * GUID_INDEX;
    let mut seed = seed | 1;
    for (i, c) in coms.iter().enumerate() {
        let module = model.str(model.files.as_slice()[c.file as usize].name);
        let progid = model.str(c.progid);
        let mut alias = [0u8; 16];
        for k in 0..4 {
            alias[4 * k..4 * k + 4].copy_from_slice(&next(&mut seed).to_le_bytes());
        }
        // A version 4 (random) GUID, RFC 4122 variant.
        alias[7] = (alias[7] & 0x0f) | 0x40;
        alias[8] = (alias[8] & 0x3f) | 0x80;
        let strings = if progid.is_empty() { 0 } else { aligned(2 * (progid.len() + 1)) };
        let len = COM_DATA + strings;
        let r = roster(model, c.file);
        o.guid_index(index_offset, i, &c.clsid, (at, len), r);
        o.guid_index(index_offset, coms.len() + i, &alias, (at, len), r);
        let misc_mask = [1u32, 4, 8, 2, 16]
            .iter()
            .zip(c.misc)
            .filter(|&(_, v)| v != 0)
            .fold(0, |m, (&bit, _)| m | bit);
        o.u32(at, COM_DATA as u32);
        o.u32(at + 4, misc_mask << 8);
        o.u32(at + 8, c.model);
        o.bytes(at + 12, &c.clsid);
        o.bytes(at + 28, &alias);
        o.bytes(at + 44, &c.clsid);
        o.bytes(at + 60, &c.tlbid);
        o.u32(at + 76, 2 * module.len() as u32);
        o.u32(at + 80, module_at as u32);
        if !progid.is_empty() {
            o.u32(at + 84, 2 * progid.len() as u32);
            o.u32(at + 88, COM_DATA as u32);
            o.wide(at + COM_DATA, progid);
        }
        // miscstatus, miscstatuscontent, miscstatusthumbnail,
        // miscstatusicon, miscstatusdocprint.
        for (k, v) in c.misc.iter().enumerate() {
            o.u32(at + 100 + 4 * k, *v);
        }
        o.wide(module_at, module);
        module_at += 2 * (module.len() + 1);
        at += len;
    }
    Some(o.0)
}

/// The alias GUID the COM section generated for class `i`.
fn alias_of(com: &[u8], i: usize) -> [u8; 16] {
    let rd =
        |at: usize| u32::from_le_bytes([com[at], com[at + 1], com[at + 2], com[at + 3]]) as usize;
    let data = rd(rd(24) + i * GUID_INDEX + 16);
    let mut g = [0u8; 16];
    g.copy_from_slice(&com[data + 28..data + 44]);
    g
}

fn progid_section(model: &Model, com: &Buf<u8>) -> Option<Buf<u8>> {
    let coms = model.coms.as_slice();
    // Each class's progid attribute, then its <progid> elements.
    let mut keys = Buf::<(super::manifest::Str, usize)>::new();
    for (i, c) in coms.iter().enumerate() {
        if !c.progid.is_empty() && !keys.push((c.progid, i)) {
            return None;
        }
        for p in &model.progids.as_slice()
            [c.first_progid as usize..(c.first_progid + c.progids) as usize]
        {
            if !keys.push((*p, i)) {
                return None;
            }
        }
    }
    let keys = keys.as_slice();
    if keys.is_empty() {
        return Some(Buf::new());
    }
    let global = keys.len() * 16;
    let index_offset = STRING_HEADER + global;
    let strings: usize = keys.iter().map(|(s, _)| aligned(2 * (s.len as usize + 1))).sum();
    let total = index_offset + keys.len() * (STRING_INDEX + PROGID_DATA) + strings;
    let mut o = Out::new(total)?;
    o.string_header(keys.len(), index_offset, (STRING_HEADER, global));
    let mut at = index_offset + keys.len() * STRING_INDEX;
    for (k, &(s, class)) in keys.iter().enumerate() {
        let name = model.str(s);
        let data = at + aligned(2 * (name.len() + 1));
        let guid_at = STRING_HEADER + 16 * k;
        o.wide(at, name);
        o.string_index(
            index_offset,
            k,
            [
                hash(name),
                at as u32,
                2 * name.len() as u32,
                data as u32,
                PROGID_DATA as u32,
                roster(model, coms[class].file),
            ],
        );
        o.u32(data, PROGID_DATA as u32);
        o.u32(data + 8, guid_at as u32);
        o.bytes(guid_at, &alias_of(com.as_slice(), class));
        at = data + PROGID_DATA;
    }
    Some(o.0)
}

/// Looks `key` up in string section `s`: (index entry's data offset,
/// data length, roster index).
pub fn find_string(s: &[u8], key: &[u16]) -> Option<(usize, usize, u32)> {
    if s.len() < STRING_HEADER {
        return None;
    }
    let rd = |at: usize| u32::from_le_bytes([s[at], s[at + 1], s[at + 2], s[at + 3]]);
    let (count, index) = (rd(20) as usize, rd(24) as usize);
    let h = hash(key);
    (0..count).map(|i| index + i * STRING_INDEX).find_map(|e| {
        if rd(e) != h {
            return None;
        }
        let (at, len) = (rd(e + 4) as usize, rd(e + 8) as usize / 2);
        let name = (0..len).map(|k| u16::from_le_bytes([s[at + 2 * k], s[at + 2 * k + 1]]));
        let same = len == key.len() && name.zip(key).all(|(a, &b)| upper(a) == upper(b));
        same.then(|| (rd(e + 12) as usize, rd(e + 16) as usize, rd(e + 20)))
    })
}

/// Looks `guid` up in GUID section `s`: (data offset, data length,
/// roster index).
pub fn find_guid(s: &[u8], guid: &[u8; 16]) -> Option<(usize, usize, u32)> {
    if s.len() < GUID_HEADER {
        return None;
    }
    let rd = |at: usize| u32::from_le_bytes([s[at], s[at + 1], s[at + 2], s[at + 3]]);
    let (count, index) = (rd(20) as usize, rd(24) as usize);
    (0..count).map(|i| index + i * GUID_INDEX).find_map(|e| {
        (s[e..e + 16] == guid[..]).then(|| (rd(e + 16) as usize, rd(e + 20) as usize, rd(e + 24)))
    })
}
