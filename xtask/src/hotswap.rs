//! Hot swapping gala's own DLLs and the kernel into a machine resumed from
//! a snapshot (ROADMAP SN steps 1d and 1e; WORKFLOW "Hot swapping DLLs and
//! the kernel into a snapshot (at your own risk)"): `boot-test
//! --from-snapshot <name> --hot-swap-unsafe [kernel,]<dll>[,<dll>...]`.
//!
//! The kernel is read from its ELF (linked with `--emit-relocs`) and lld's
//! map of it (`Image::from_elf`) into the same chunks, references and
//! diff; its plan record (machine `KERNEL_MACHINE`) is applied by the
//! kernel itself with every other CPU stopped (ARCHITECTURE "Hot swapping
//! the kernel into a resumed machine").
//!
//! **Not a real run.** What this module makes is a patch to programs that
//! are already running: their DLL is not replaced but redirected, function
//! by function, to code of a newer build placed beside it. The checks
//! below refuse what they can see is unsafe; what they cannot see (state
//! the old code built differently, invariants of data whose layout did
//! not change) is the user's risk, and every result is marked so.
//!
//! The host side (here) compares the snapshot's build of a DLL with the
//! new one, both gala's own builds, with the linker's map of each (lld's
//! `/lldmap`: every input section, its address, size and symbols):
//!
//! 1. **Chunks.** Every input section of the map is a chunk: a function
//!    (Rust and LLVM put each in a section of its own), a constant, a
//!    static. A chunk is known by its name (the demangled symbol, its
//!    crate, an occurrence number for equal names) or, when it has none
//!    (`anon.*`: string literals, jump tables, vtables), by its contents.
//! 2. **References normalized.** Code is decoded (iced-x86, our own
//!    builds only): every rip-relative operand and every rel32 branch
//!    leaving its function is a reference; the base relocations are the
//!    absolute ones; a jump table (32-bit entries relative to the table,
//!    in `.rdata`) is attached to the function using it. A reference is
//!    compared by what it names (chunk name or contents, an import's
//!    name), not by its bytes, so a function that merely moved is the
//!    same and one that changed, or into which a changed function was
//!    inlined, differs.
//! 3. **What changes.** Functions that differ or are new are copied into
//!    a blob; data the blob needs that the old image does not have (new
//!    constants, changed read-only data, new statics) too. Functions that
//!    read changed read-only data are changed with it. References from
//!    the blob are bound to the OLD image wherever the old image has the
//!    same thing: its statics (state is kept), its unchanged functions,
//!    its import address table.
//! 4. **Refusals.** A writable static that changed size, anonymous
//!    writable data, a new import, thread-local data that changed, code
//!    that does not decode or has a short branch out of its function, a
//!    function too short for the jump, identical-code folding that
//!    differs between the builds: the plan is not made, and the reason
//!    names the symbol.
//!
//! The plan (`encode`) goes to the guest through QEMU's fw_cfg
//! (`opt/gala/hotswap`); the kernel (`kernel/src/hotswap.rs`) does the rest at the
//! resumed machine's first instruction (an NMI): every program thread
//! held, the stack check, the blob mapped beside the image in every
//! process that has it, the old functions' entries made jumps, the file
//! replaced for later loads.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

use iced_x86::{Decoder, DecoderOptions, Instruction};

type Result<T = ()> = std::result::Result<T, String>;

/// The plan's first bytes (`encode`; `kernel/src/hotswap.rs` reads it).
pub const MAGIC: &[u8; 8] = b"GALAHSWP";
/// The plan format's version.
pub const VERSION: u32 = 1;
/// The fw_cfg file the plan is given to the guest as.
pub const FW_CFG_NAME: &str = "opt/gala/hotswap";
/// The bytes a patched entry needs: `jmp rel32`.
pub const JUMP_LEN: u32 = 5;
/// A plan record's machine for the kernel (1 x64 and 2 x86 are DLLs';
/// the kernel's `hotswap::KERNEL`).
pub const KERNEL_MACHINE: u8 = 3;
/// A static of the kernel's hot swap: a kernel without it cannot take
/// a kernel record.
pub const KERNEL_SUPPORT: &str = "kernel::hotswap::PATCHING";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Machine {
    X64,
    X86,
}

impl Machine {
    fn bitness(self) -> u32 {
        match self {
            Machine::X64 => 64,
            Machine::X86 => 32,
        }
    }

    fn code(self) -> u8 {
        match self {
            Machine::X64 => 1,
            Machine::X86 => 2,
        }
    }

    fn word(self) -> u8 {
        match self {
            Machine::X64 => 8,
            Machine::X86 => 4,
        }
    }
}

/// What a chunk holds (from its output section's characteristics).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Code,
    ReadOnly,
    Writable,
    /// Thread-local data's template (`.tls`).
    Tls,
    /// Unwind data and the linker's tables (`.pdata`, `.xdata`, ...):
    /// not compared, not copied.
    Other,
}

/// One input section of the linker's map.
#[derive(Clone, Debug)]
pub struct Chunk {
    pub rva: u32,
    pub size: u32,
    /// The symbols at its start (identical code folding gives one several).
    pub names: Vec<String>,
    /// The crate of its object file (`msxml6`, `core`, ...).
    pub krate: String,
    pub kind: Kind,
    /// Its input section's name (`.text._RNv...`, `.rdata`).
    pub section: String,
    /// Its alignment (a copy keeps it; 16 at least).
    pub align: u32,
    /// Code that is not a compiled function of its own (the kernel's
    /// `global_asm!`: entry stubs, the context switch, trampolines): its
    /// references cannot be known for sure, so it is never copied.
    pub asm: bool,
}

impl Chunk {
    /// Whether it is known by its contents rather than a name.
    fn anonymous(&self) -> bool {
        // (A 32-bit build decorates C names with a leading `_`.)
        self.names.iter().all(|n| {
            let n = n.strip_prefix('_').unwrap_or(n);
            n.starts_with("anon.") || n.starts_with(".L")
        })
    }

    pub fn name(&self) -> &str {
        self.names.first().map_or("<anonymous>", String::as_str)
    }
}

/// A PE image of gala's build with its linker map.
#[derive(Clone, Debug)]
pub struct Image {
    pub machine: Machine,
    pub base: u64,
    pub size_of_image: u32,
    pub timestamp: u32,
    /// The image laid out at its RVAs.
    pub mem: Vec<u8>,
    /// Sections: (name, rva, virtual size, characteristics).
    pub sections: Vec<(String, u32, u32, u32)>,
    /// Base relocation sites: rva -> width in bytes.
    pub relocs: BTreeMap<u32, u8>,
    /// Import address table slots: rva -> "dll!name" (dll lower case).
    pub imports: BTreeMap<u32, String>,
    /// Exported names.
    pub exports: BTreeSet<String>,
    /// Sorted by rva, non-empty ones only.
    pub chunks: Vec<Chunk>,
    /// The kernel (an ELF of gala's build, `from_elf`), not a DLL.
    pub kernel: bool,
    /// Relocation sites of 32-bit absolute addresses the CPU sign-extends
    /// (the kernel's `R_X86_64_32S`).
    pub signed: BTreeSet<u32>,
    /// Relocation sites that name a linker script's symbol
    /// (`__text_start`): compared and bound by that name.
    pub link_refs: BTreeMap<u32, String>,
    /// The linker script's symbols: name -> address relative to `base`.
    pub linker: BTreeMap<String, i64>,
    /// Relocation sites -> the rva of the symbol they name (not a
    /// section's): a reference is to that symbol's chunk, at whatever
    /// offset (one past a string's end is still that string).
    pub reloc_symbols: BTreeMap<u32, u32>,
}

fn u16_at(b: &[u8], at: usize) -> Result<u16> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| format!("truncated at {at:#x}"))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| format!("truncated at {at:#x}"))
}

fn u64_at(b: &[u8], at: usize) -> Result<u64> {
    b.get(at..at + 8)
        .map(|s| u64::from_le_bytes(s.try_into().unwrap_or([0; 8])))
        .ok_or_else(|| format!("truncated at {at:#x}"))
}

fn c_string(b: &[u8], at: usize) -> String {
    let s = b.get(at..).unwrap_or(&[]);
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf8_lossy(&s[..end]).into_owned()
}

/// `name` without what a build adds to it: LLVM's `(.llvm.<hash>)` on a
/// promoted local symbol.
pub fn normalize_name(name: &str) -> String {
    let name = name.trim();
    let mut out = match name.find(" (.llvm.") {
        Some(i) => name[..i].to_string(),
        None => name.to_string(),
    };
    // Undemangled (a 32-bit build's decorated names): `x.llvm.123@12`.
    if let Some(i) = out.find(".llvm.") {
        let digits = out[i + 6..].chars().take_while(char::is_ascii_digit).count();
        out.replace_range(i..i + 6 + digits, "");
    }
    // A Rust v0 symbol lld leaves mangled (a 32-bit build's `@n`
    // decoration stops it): its crate disambiguator (`Cs<base-62>_`
    // before the crate name's length) follows the crate's metadata,
    // which features and profiles change; the name does not.
    if out.contains("_R") {
        let b = out.as_bytes();
        let mut keep = String::new();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'C'
                && b.get(i + 1) == Some(&b's')
                && i > 0
                && b[i - 1].is_ascii_alphabetic()
            {
                let hash = b[i + 2..].iter().take_while(|c| c.is_ascii_alphanumeric()).count();
                let end = i + 2 + hash;
                if hash > 0
                    && b.get(end) == Some(&b'_')
                    && b.get(end + 1).is_some_and(u8::is_ascii_digit)
                {
                    keep.push('C');
                    i = end + 1;
                    continue;
                }
            }
            keep.push(char::from(b[i]));
            i += 1;
        }
        out = keep;
    }
    out
}

/// The crate an object file of the map is from: `gala_xml` of
/// `.../gala_xml-88f9.gala_xml.1c17-cgu.1.rcgu.o`, `msxml6` of
/// `.../msxml6.msxml6.f397-cgu.0.rcgu.o`.
fn crate_of(object: &str) -> String {
    // An archive's member: `.../libkernel-e55c.rlib(kernel-e55c.x.rcgu.o)`.
    let object = match object.strip_suffix(')').and_then(|o| o.rsplit_once('(')) {
        Some((_, member)) => member,
        None => object,
    };
    let file = object.rsplit('/').next().unwrap_or(object);
    let end = file.find(['-', '.']).unwrap_or(file.len());
    file[..end].to_string()
}

/// One input section as lld's map lists it: (rva, size, output section,
/// input section, crate, symbols at its start).
type MapEntry = (u32, u32, String, String, String, Vec<String>);

/// Parses lld's `/lldmap` text: `Address Size Align Out In Symbol`, an
/// output section at column 24, an input section 8 columns in, a symbol
/// 16 in.
pub fn parse_map(text: &str) -> Result<Vec<MapEntry>> {
    let mut out = String::new();
    let mut entries: Vec<MapEntry> = Vec::new();
    for (n, line) in text.lines().enumerate().skip(1) {
        if line.len() < 25 {
            continue;
        }
        let addr = u32::from_str_radix(&line[..8], 16);
        let size = u32::from_str_radix(&line[9..17], 16);
        let (Ok(addr), Ok(size)) = (addr, size) else {
            return Err(format!("map line {}: {line:?}", n + 1));
        };
        let rest = &line[24..];
        let indent = rest.len() - rest.trim_start().len();
        let text = rest.trim();
        match indent {
            0 => out = text.to_string(),
            8 => {
                let (object, input) = match text.rfind(":(") {
                    Some(i) => {
                        (&text[..i], text[i + 2..].strip_suffix(')').unwrap_or(&text[i + 2..]))
                    }
                    None => (text, text),
                };
                entries.push((
                    addr,
                    size,
                    out.clone(),
                    input.to_string(),
                    crate_of(object),
                    vec![],
                ));
            }
            _ => {
                if let Some(e) = entries.last_mut()
                    && e.0 == addr
                {
                    let name = normalize_name(text);
                    if !e.5.contains(&name) {
                        e.5.push(name);
                    }
                }
            }
        }
    }
    Ok(entries)
}

impl Image {
    /// Reads a PE image (`dll`) of gala's build and its lld map.
    pub fn read(dll: &[u8], map: &str) -> Result<Image> {
        let mut image = Image::from_pe(dll)?;
        for (rva, size, out, input, krate, names) in parse_map(map)? {
            if size == 0 {
                continue;
            }
            let ch = image.sections.iter().find(|s| s.0 == out).map_or(0, |s| s.3);
            let kind = match input.as_str() {
                ".pdata" | ".xdata" | ".reloc" | ".edata" => Kind::Other,
                i if i.starts_with(".idata") || i.starts_with(".CRT") => Kind::Other,
                _ if out == ".tls" => Kind::Tls,
                _ if ch & 0x2000_0000 != 0 => Kind::Code,
                _ if ch & 0x8000_0000 != 0 => Kind::Writable,
                _ => Kind::ReadOnly,
            };
            let (section, asm, align) = (input, false, 16);
            image.chunks.push(Chunk { rva, size, names, krate, kind, section, asm, align });
        }
        image.chunks.sort_by_key(|c| c.rva);
        Ok(image)
    }

    /// The headers, sections, relocations, imports and exports of a PE
    /// image (chunks come from its map: `read`).
    pub fn from_pe(pe: &[u8]) -> Result<Image> {
        if pe.get(..2) != Some(b"MZ") {
            return Err("not a PE image (no MZ)".into());
        }
        let nt = u32_at(pe, 0x3c)? as usize;
        if pe.get(nt..nt + 4) != Some(b"PE\0\0") {
            return Err("not a PE image (no PE signature)".into());
        }
        let machine = match u16_at(pe, nt + 4)? {
            0x8664 => Machine::X64,
            0x14c => Machine::X86,
            m => return Err(format!("machine {m:#x} is not x64 or x86")),
        };
        let sections_count = u16_at(pe, nt + 6)? as usize;
        let timestamp = u32_at(pe, nt + 8)?;
        let opt_size = u16_at(pe, nt + 20)? as usize;
        let opt = nt + 24;
        let (base, dirs) = match machine {
            Machine::X64 => (u64_at(pe, opt + 24)?, opt + 112),
            Machine::X86 => (u64::from(u32_at(pe, opt + 28)?), opt + 96),
        };
        let size_of_image = u32_at(pe, opt + 56)?;
        let headers = u32_at(pe, opt + 60)? as usize;
        let dir = |i: usize| -> Result<(u32, u32)> {
            Ok((u32_at(pe, dirs + i * 8)?, u32_at(pe, dirs + i * 8 + 4)?))
        };
        let mut mem = vec![0u8; size_of_image as usize];
        let h = headers.min(pe.len()).min(mem.len());
        mem[..h].copy_from_slice(&pe[..h]);
        let mut sections = Vec::new();
        let table = opt + opt_size;
        for i in 0..sections_count {
            let s = table + i * 40;
            let name = c_string(&pe[s..s + 8], 0);
            let vsize = u32_at(pe, s + 8)?;
            let rva = u32_at(pe, s + 12)?;
            let raw_size = u32_at(pe, s + 16)? as usize;
            let raw = u32_at(pe, s + 20)? as usize;
            let ch = u32_at(pe, s + 36)?;
            let n = raw_size.min(vsize as usize).min(pe.len().saturating_sub(raw));
            let at = rva as usize;
            if at + n > mem.len() {
                return Err(format!("section {name} lies outside the image"));
            }
            mem[at..at + n].copy_from_slice(&pe[raw..raw + n]);
            sections.push((name, rva, vsize, ch));
        }
        let mut image = Image {
            machine,
            base,
            size_of_image,
            timestamp,
            mem,
            sections,
            relocs: BTreeMap::new(),
            imports: BTreeMap::new(),
            exports: BTreeSet::new(),
            chunks: Vec::new(),
            kernel: false,
            signed: BTreeSet::new(),
            link_refs: BTreeMap::new(),
            linker: BTreeMap::new(),
            reloc_symbols: BTreeMap::new(),
        };
        // Base relocations (directory 5).
        let (rva, size) = dir(5)?;
        let mut at = rva as usize;
        let end = (rva + size) as usize;
        while at + 8 <= end {
            let page = u32_at(&image.mem, at)?;
            let block = u32_at(&image.mem, at + 4)? as usize;
            if block < 8 {
                break;
            }
            for e in (at + 8..at + block).step_by(2) {
                let v = u16_at(&image.mem, e)?;
                match v >> 12 {
                    0 => {}
                    3 => {
                        image.relocs.insert(page + u32::from(v & 0xfff), 4);
                    }
                    10 => {
                        image.relocs.insert(page + u32::from(v & 0xfff), 8);
                    }
                    t => return Err(format!("base relocation type {t} at {page:#x}")),
                }
            }
            at += block;
        }
        // Imports (directory 1): each IAT slot's dll!name.
        let (rva, _) = dir(1)?;
        if rva != 0 {
            let word = usize::from(machine.word());
            let mut d = rva as usize;
            loop {
                let lookup = u32_at(&image.mem, d)?;
                let name = u32_at(&image.mem, d + 12)?;
                let iat = u32_at(&image.mem, d + 16)?;
                if name == 0 && iat == 0 {
                    break;
                }
                let dll = c_string(&image.mem, name as usize).to_ascii_lowercase();
                let names = if lookup != 0 { lookup } else { iat } as usize;
                for i in 0.. {
                    let entry = if word == 8 {
                        u64_at(&image.mem, names + i * 8)?
                    } else {
                        u64::from(u32_at(&image.mem, names + i * 4)?)
                    };
                    if entry == 0 {
                        break;
                    }
                    let ordinal_flag = 1u64 << (word * 8 - 1);
                    let what = if entry & ordinal_flag != 0 {
                        format!("#{}", entry & 0xffff)
                    } else {
                        c_string(&image.mem, (entry & 0x7fff_ffff) as usize + 2)
                    };
                    image.imports.insert(iat + (i * word) as u32, format!("{dll}!{what}"));
                }
                d += 20;
            }
        }
        // Exports (directory 0): their names.
        let (rva, _) = dir(0)?;
        if rva != 0 {
            let r = rva as usize;
            let count = u32_at(&image.mem, r + 24)? as usize;
            let names = u32_at(&image.mem, r + 32)? as usize;
            for i in 0..count {
                let n = u32_at(&image.mem, names + i * 4)? as usize;
                image.exports.insert(c_string(&image.mem, n));
            }
        }
        Ok(image)
    }

    /// The chunk at or containing `rva` (or ending at it: a pointer one
    /// past an array), with the offset into it.
    fn chunk_at(&self, rva: u32) -> Option<(usize, i64)> {
        let i = self.chunks.partition_point(|c| c.rva <= rva);
        if i > 0 {
            let c = &self.chunks[i - 1];
            if rva < c.rva + c.size {
                return Some((i - 1, i64::from(rva - c.rva)));
            }
            if rva == c.rva + c.size && self.chunks.get(i).is_none_or(|n| n.rva != rva) {
                return Some((i - 1, i64::from(c.size)));
            }
        }
        None
    }

    fn first_section(&self) -> u32 {
        self.sections.iter().map(|s| s.1).min().unwrap_or(0x1000)
    }

    /// What the reference at `site` (an rva) to `va` names: a linker
    /// script's symbol by its name, else what `va` is.
    fn target_at(&self, site: u32, va: u64) -> Target {
        if let Some(name) = self.link_refs.get(&site) {
            return Target::Linker(name.clone());
        }
        // The chunk of the symbol the relocation names, at the offset the
        // reference has from it.
        if let Some(&sym) = self.reloc_symbols.get(&site)
            && let Some((idx, _)) = self.chunk_at(sym)
            && let Some(rva) = va.checked_sub(self.base)
        {
            return Target::Chunk { idx, off: rva as i64 - i64::from(self.chunks[idx].rva) };
        }
        self.target(va)
    }

    /// What `va` names.
    fn target(&self, va: u64) -> Target {
        let Some(rva) = va.checked_sub(self.base).and_then(|r| u32::try_from(r).ok()) else {
            return Target::Unknown(va);
        };
        if let Some(name) = self.imports.get(&rva) {
            return Target::Import(name.clone());
        }
        if let Some((idx, off)) = self.chunk_at(rva) {
            return Target::Chunk { idx, off };
        }
        if rva < self.first_section() {
            return Target::Header(rva);
        }
        Target::Unknown(va)
    }
}

/// One input section of lld's ELF map (`-Map`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElfMapEntry {
    pub va: u64,
    pub size: u64,
    pub align: u32,
    /// The output section (`.text`).
    pub out: String,
    /// The input section (`.text._RNvCs..._6kernel4main`).
    pub input: String,
    /// The crate of its object file (`kernel`, `core`, `<internal>`).
    pub krate: String,
    /// The symbols at its start, demangled and normalized.
    pub names: Vec<String>,
}

/// Parses lld's ELF map (`-Map`): `VMA LMA Size Align Out In Symbol`,
/// the four numbers (hex, hex, hex, decimal) then at column 49 an output
/// section, an input section 8 columns in, a symbol 16 in; a linker
/// script's assignment (`__text_start = .`) at any depth. Returns the
/// input sections and the assignments (name -> address).
pub fn parse_elf_map(text: &str) -> Result<(Vec<ElfMapEntry>, BTreeMap<String, u64>)> {
    let mut entries: Vec<ElfMapEntry> = Vec::new();
    let mut symbols = BTreeMap::new();
    let mut out = String::new();
    for (n, line) in text.lines().enumerate().skip(1) {
        // The four numbers, then (one space after them) the rest.
        let mut end = 0;
        let mut numbers = [""; 4];
        for number in &mut numbers {
            let from = end + line[end..].len() - line[end..].trim_start().len();
            let to = line[from..].find(' ').map_or(line.len(), |i| from + i);
            *number = &line[from..to];
            end = to;
        }
        let bad = || format!("map line {}: {line:?}", n + 1);
        let (Ok(va), Ok(size), Ok(align)) = (
            u64::from_str_radix(numbers[0], 16),
            u64::from_str_radix(numbers[2], 16),
            numbers[3].parse::<u32>(),
        ) else {
            return Err(bad());
        };
        let rest = line.get(end + 1..).ok_or_else(bad)?;
        let indent = rest.len() - rest.trim_start().len();
        let text = rest.trim();
        if let Some((name, _)) = text.split_once(" = ") {
            if !name.contains(' ') && !name.is_empty() && name != "." {
                symbols.insert(name.to_string(), va);
            }
            continue;
        }
        match indent {
            0 => out = text.to_string(),
            8 => {
                let (object, input) = match text.rfind(":(") {
                    Some(i) => {
                        (&text[..i], text[i + 2..].strip_suffix(')').unwrap_or(&text[i + 2..]))
                    }
                    None => (text, text),
                };
                entries.push(ElfMapEntry {
                    va,
                    size,
                    align,
                    out: out.clone(),
                    input: input.to_string(),
                    krate: crate_of(object),
                    names: vec![],
                });
            }
            _ => {
                if let Some(e) = entries.last_mut()
                    && e.va == va
                {
                    let name = normalize_name(text);
                    if !e.names.contains(&name) {
                        e.names.push(name);
                    }
                }
            }
        }
    }
    Ok((entries, symbols))
}

/// A section header of an ELF64 file.
#[derive(Clone, Debug)]
struct ElfSection {
    name: String,
    kind: u32,
    flags: u64,
    addr: u64,
    offset: u64,
    size: u64,
    link: u32,
    info: u32,
}

impl ElfSection {
    const ALLOC: u64 = 2;
    const WRITE: u64 = 1;
    const EXEC: u64 = 4;
}

fn elf_sections(elf: &[u8]) -> Result<Vec<ElfSection>> {
    if elf.get(..4) != Some(b"\x7fELF") || elf.get(4) != Some(&2) || elf.get(5) != Some(&1) {
        return Err("not a 64-bit little-endian ELF file".into());
    }
    if u16_at(elf, 18)? != 62 {
        return Err("not an x86-64 ELF file".into());
    }
    let shoff = u64_at(elf, 0x28)? as usize;
    let shentsize = usize::from(u16_at(elf, 0x3a)?);
    let shnum = usize::from(u16_at(elf, 0x3c)?);
    let shstrndx = usize::from(u16_at(elf, 0x3e)?);
    let header = |i: usize| -> Result<ElfSection> {
        let h = shoff + i * shentsize;
        Ok(ElfSection {
            name: String::new(),
            kind: u32_at(elf, h + 4)?,
            flags: u64_at(elf, h + 8)?,
            addr: u64_at(elf, h + 16)?,
            offset: u64_at(elf, h + 24)?,
            size: u64_at(elf, h + 32)?,
            link: u32_at(elf, h + 40)?,
            info: u32_at(elf, h + 44)?,
        })
        .and_then(|mut s| {
            s.name = u32_at(elf, h)?.to_string();
            Ok(s)
        })
    };
    let strtab = header(shstrndx)?.offset as usize;
    let mut out = Vec::new();
    for i in 0..shnum {
        let mut s = header(i)?;
        let at: usize = s.name.parse().unwrap_or(0);
        s.name = c_string(elf, strtab + at);
        out.push(s);
    }
    Ok(out)
}

/// The base address the kernel's `rva`s are relative to: its lowest
/// allocated section (the linker script's `__kernel_start`).
fn elf_base(sections: &[ElfSection]) -> u64 {
    sections
        .iter()
        .filter(|s| s.flags & ElfSection::ALLOC != 0 && s.addr != 0)
        .map(|s| s.addr)
        .min()
        .unwrap_or(0)
}

impl Image {
    /// Reads gala's kernel (an ELF linked with `--emit-relocs`: its
    /// relocations are kept) and lld's map of it. The image's `timestamp`
    /// and `size_of_image` are the CRC-32 and size of its `.text`: what the
    /// running kernel checks it is (`kernel/src/hotswap.rs`).
    pub fn from_elf(elf: &[u8], map: &str) -> Result<Image> {
        let sections = elf_sections(elf)?;
        let base = elf_base(&sections);
        let alloc: Vec<&ElfSection> = sections
            .iter()
            .filter(|s| s.flags & ElfSection::ALLOC != 0 && s.addr >= base && base != 0)
            .collect();
        let end = alloc.iter().map(|s| s.addr + s.size).max().unwrap_or(base);
        let extent = end - base;
        if extent > 1 << 30 {
            return Err(format!("the kernel spans {extent:#x} bytes"));
        }
        let mut mem = vec![0u8; extent as usize];
        let mut image_sections = Vec::new();
        for s in &alloc {
            let rva = (s.addr - base) as usize;
            if s.kind != 8 {
                // Not NOBITS: its contents.
                let from = elf
                    .get(s.offset as usize..(s.offset + s.size) as usize)
                    .ok_or_else(|| format!("section {} lies outside the file", s.name))?;
                mem[rva..rva + from.len()].copy_from_slice(from);
            }
            let ch = 0x4000_0000
                | if s.flags & ElfSection::EXEC != 0 { 0x2000_0000 } else { 0 }
                | if s.flags & ElfSection::WRITE != 0 { 0x8000_0000 } else { 0 };
            image_sections.push((s.name.clone(), rva as u32, s.size as u32, ch));
        }
        let text = alloc.iter().find(|s| s.name == ".text").ok_or("the kernel has no .text")?;
        let text_at = (text.addr - base) as usize;
        let text_bytes = &mem[text_at..text_at + text.size as usize];
        let (entries, assignments) = parse_elf_map(map)?;
        // The map must be this file's: its output sections where the
        // file's are.
        for s in &alloc {
            let listed = map.lines().any(|l| {
                let mut f = l.split_whitespace();
                let va = f.next().and_then(|v| u64::from_str_radix(v, 16).ok());
                let size = f.nth(1).and_then(|v| u64::from_str_radix(v, 16).ok());
                f.nth(1) == Some(s.name.as_str())
                    && f.next().is_none()
                    && va == Some(s.addr)
                    && size == Some(s.size)
            });
            if !listed {
                return Err(format!(
                    "the map does not list the kernel's {} at {:#x} ({:#x} bytes): it is the map \
                     of another build",
                    s.name, s.addr, s.size
                ));
            }
        }
        let linker: BTreeMap<String, i64> =
            assignments.iter().map(|(n, &va)| (n.clone(), va.wrapping_sub(base) as i64)).collect();
        let mut image = Image {
            machine: Machine::X64,
            base,
            size_of_image: text.size as u32,
            timestamp: crc32(text_bytes),
            mem,
            sections: image_sections,
            relocs: BTreeMap::new(),
            imports: BTreeMap::new(),
            exports: BTreeSet::new(),
            chunks: Vec::new(),
            kernel: true,
            signed: BTreeSet::new(),
            link_refs: BTreeMap::new(),
            linker,
            reloc_symbols: BTreeMap::new(),
        };
        // The relocations kept in the file (`.rela.<section>`), for its
        // allocated sections: absolute ones are references the decoder
        // cannot see; any naming a linker script's symbol is compared and
        // bound by that name.
        let symtab = sections.iter().find(|s| s.kind == 2);
        let symbol_name = |index: u64| -> Option<String> {
            let st = symtab?;
            let strings = sections.get(st.link as usize)?.offset as usize;
            let sym = st.offset as usize + index as usize * 24;
            Some(c_string(elf, strings + u32_at(elf, sym).ok()? as usize))
        };
        // A symbol's value, if it is one of an object (not a section's,
        // not the file's, not undefined).
        let symbol_value = |index: u64| -> Option<u64> {
            let sym = symtab?.offset as usize + index as usize * 24;
            let kind = elf.get(sym + 4)? & 0xf;
            let shndx = u16_at(elf, sym + 6).ok()?;
            let value = u64_at(elf, sym + 8).ok()?;
            (kind != 3 && kind != 4 && shndx != 0 && value >= base).then_some(value)
        };
        for rela in sections.iter().filter(|s| s.kind == 4) {
            let Some(target) = sections.get(rela.info as usize) else { continue };
            if target.flags & ElfSection::ALLOC == 0 || target.addr < base {
                continue;
            }
            let data = elf
                .get(rela.offset as usize..(rela.offset + rela.size) as usize)
                .ok_or_else(|| format!("{} lies outside the file", rela.name))?;
            for r in data.chunks_exact(24) {
                let offset = u64::from_le_bytes(r[..8].try_into().unwrap_or([0; 8]));
                let info = u64::from_le_bytes(r[8..16].try_into().unwrap_or([0; 8]));
                let Some(site) = offset.checked_sub(base).and_then(|v| u32::try_from(v).ok())
                else {
                    continue;
                };
                match info & 0xffff_ffff {
                    1 => {
                        image.relocs.insert(site, 8);
                    }
                    10 => {
                        image.relocs.insert(site, 4);
                    }
                    11 => {
                        image.relocs.insert(site, 4);
                        image.signed.insert(site);
                    }
                    _ => {}
                }
                let sym = info >> 32;
                if sym != 0
                    && let Some(name) = symbol_name(sym)
                    && image.linker.contains_key(&name)
                {
                    image.link_refs.insert(site, name);
                } else if sym != 0
                    // Relocations to the symbol itself (64, PC32, PLT32,
                    // 32, 32S, PC64), not to its GOT slot.
                    && matches!(info & 0xffff_ffff, 1 | 2 | 4 | 10 | 11 | 24)
                    && let Some(value) = symbol_value(sym)
                    && let Ok(rva) = u32::try_from(value - base)
                {
                    image.reloc_symbols.insert(site, rva);
                }
            }
        }
        // Chunks: every input section; the linker's merged constants
        // (`<internal>` `.rodata.str*`, `.rodata.cst<n>`) and its GOT split
        // into their entries, each known by its contents.
        for e in entries {
            let Some(sec) = alloc.iter().find(|s| s.name == e.out) else { continue };
            if e.size == 0 || e.va < base {
                continue;
            }
            let rva = (e.va - base) as u32;
            let kind = if sec.flags & ElfSection::EXEC != 0 {
                Kind::Code
            } else if e.out == ".got" {
                Kind::ReadOnly
            } else if sec.flags & ElfSection::WRITE != 0 {
                Kind::Writable
            } else {
                Kind::ReadOnly
            };
            let piece = |rva: u32, size: u32| Chunk {
                rva,
                size,
                names: vec![],
                krate: e.krate.clone(),
                kind,
                section: e.input.clone(),
                asm: false,
                align: 1,
            };
            let internal = e.krate == "<internal>";
            if internal && e.out == ".got" {
                for at in (0..e.size as u32).step_by(8) {
                    let value = u64_at(&image.mem, (rva + at) as usize)?;
                    if value != 0 {
                        image.relocs.insert(rva + at, 8);
                    }
                    image.chunks.push(piece(rva + at, 8.min(e.size as u32 - at)));
                }
                continue;
            }
            if internal && let Some(n) = e.input.strip_prefix(".rodata.cst") {
                let n: u32 = n.parse().unwrap_or(0);
                if n > 0 && e.size % u64::from(n) == 0 {
                    for at in (0..e.size as u32).step_by(n as usize) {
                        image.chunks.push(piece(rva + at, n));
                    }
                    continue;
                }
            }
            if internal && e.input.starts_with(".rodata.str") {
                let width = e.input.rsplit('.').nth(1).and_then(|w| w.parse::<u32>().ok());
                let width = width.unwrap_or(1).max(1) as usize;
                let bytes = &image.mem[rva as usize..(rva as usize + e.size as usize)];
                let mut start = 0usize;
                let mut at = 0usize;
                while at + width <= bytes.len() {
                    if bytes[at..at + width].iter().all(|&b| b == 0) {
                        image.chunks.push(piece(rva + start as u32, (at + width - start) as u32));
                        start = at + width;
                    }
                    at += width;
                }
                if start < bytes.len() {
                    image.chunks.push(piece(rva + start as u32, (bytes.len() - start) as u32));
                }
                continue;
            }
            // Assembly: code in a section that is not `.text.<symbol>` of
            // a compiled function.
            let asm = kind == Kind::Code
                && match e.input.strip_prefix(".text.") {
                    None => true,
                    Some(rest) => {
                        let rest = rest.strip_prefix("unlikely.").unwrap_or(rest);
                        !rest.starts_with("_R")
                            && !rest.starts_with("_ZN")
                            && !e.names.iter().any(|n| n == rest)
                    }
                };
            image.chunks.push(Chunk {
                rva,
                size: e.size as u32,
                names: e.names,
                krate: e.krate,
                kind,
                section: e.input,
                asm,
                align: e.align,
            });
        }
        image.chunks.sort_by_key(|c| c.rva);
        Ok(image)
    }
}

/// What a reference names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Chunk {
        idx: usize,
        off: i64,
    },
    Import(String),
    Header(u32),
    /// A linker script's symbol (the kernel's `__text_start`).
    Linker(String),
    Unknown(u64),
}

/// A reference inside a chunk: `width` bytes at `at`, either an absolute
/// address or (`anchor`) relative to the address of offset `anchor` of
/// the same chunk (a rip-relative operand or branch: the instruction's
/// end; a jump table's entry: the table).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ref {
    pub at: u32,
    pub width: u8,
    pub anchor: Option<u32>,
    pub target: Target,
    /// A 32-bit absolute address the CPU sign-extends.
    pub signed: bool,
}

/// A chunk with its references taken out: the bytes (each reference's
/// bytes zero) and the references.
#[derive(Clone, Debug, Default)]
pub struct Norm {
    pub bytes: Vec<u8>,
    pub refs: Vec<Ref>,
    /// Why it cannot be copied elsewhere, if it cannot.
    pub problem: Option<String>,
}

/// Normalizes chunk `idx` of `image`.
fn normalize(image: &Image, idx: usize) -> Norm {
    let c = &image.chunks[idx];
    let start = c.rva as usize;
    let mut n = Norm {
        bytes: image.mem.get(start..start + c.size as usize).unwrap_or(&[]).to_vec(),
        ..Norm::default()
    };
    if n.bytes.len() != c.size as usize {
        n.problem = Some("lies outside the image".into());
        return n;
    }
    // Absolute references: the base relocations inside it.
    for (&rva, &width) in image.relocs.range(c.rva..c.rva + c.size) {
        let at = (rva - c.rva) as usize;
        if at + usize::from(width) > n.bytes.len() {
            n.problem = Some(format!("a base relocation at +{at:#x} crosses its end"));
            continue;
        }
        let signed = image.signed.contains(&rva);
        let va = if width == 8 {
            u64::from_le_bytes(n.bytes[at..at + 8].try_into().unwrap_or([0; 8]))
        } else if signed {
            i32::from_le_bytes(n.bytes[at..at + 4].try_into().unwrap_or([0; 4])) as i64 as u64
        } else {
            u64::from(u32::from_le_bytes(n.bytes[at..at + 4].try_into().unwrap_or([0; 4])))
        };
        let target = image.target_at(rva, va);
        n.refs.push(Ref { at: at as u32, width, anchor: None, target, signed });
    }
    if c.kind == Kind::Code {
        let ip = image.base + u64::from(c.rva);
        let end = ip + u64::from(c.size);
        let mut decoder = Decoder::with_ip(
            image.machine.bitness(),
            &image.mem[start..start + c.size as usize],
            ip,
            DecoderOptions::NONE,
        );
        let mut insn = Instruction::default();
        // Data in the code: a 32-bit build puts a function's jump tables
        // after its code (`add ebp, [table + eax*4]`, the table's address
        // relocated). Decoding stops where the first such table starts;
        // its address is a reference to the chunk itself, bound to the
        // copy, and its entries are relative within the function.
        let mut data_at = c.size;
        while decoder.can_decode() && ((decoder.ip() - ip) as u32) < data_at {
            decoder.decode_out(&mut insn);
            let at = (insn.ip() - ip) as u32;
            if insn.is_invalid() {
                n.problem = Some(format!("does not decode at +{at:#x}"));
                break;
            }
            let offsets = decoder.get_constant_offsets(&insn);
            let insn_end = at + insn.len() as u32;
            if offsets.has_displacement() && !insn.is_ip_rel_memory_operand() {
                let field = c.rva + at + u32::from(offsets.displacement_offset() as u8);
                let target = insn.memory_displacement64();
                if image.relocs.contains_key(&field)
                    && (ip + u64::from(insn_end)..end).contains(&target)
                {
                    data_at = data_at.min((target - ip) as u32);
                }
            }
            if insn.is_ip_rel_memory_operand() && offsets.has_displacement() {
                let target = insn.ip_rel_memory_address();
                let field = at + u32::from(offsets.displacement_offset() as u8);
                if !(ip..end).contains(&target) {
                    n.refs.push(Ref {
                        at: field,
                        width: 4,
                        anchor: Some(insn_end),
                        target: image.target_at(c.rva + field, target),
                        signed: false,
                    });
                }
            }
            let branch = matches!(
                insn.op0_kind(),
                iced_x86::OpKind::NearBranch16
                    | iced_x86::OpKind::NearBranch32
                    | iced_x86::OpKind::NearBranch64
            );
            if branch && offsets.has_immediate() {
                let target = insn.near_branch_target();
                let field = at + u32::from(offsets.immediate_offset() as u8);
                if !(ip..end).contains(&target) {
                    if offsets.immediate_size() != 4 {
                        n.problem = Some(format!(
                            "has a {}-byte branch out of itself at +{at:#x}",
                            offsets.immediate_size()
                        ));
                    }
                    n.refs.push(Ref {
                        at: field,
                        width: offsets.immediate_size() as u8,
                        anchor: Some(insn_end),
                        target: image.target_at(c.rva + field, target),
                        signed: false,
                    });
                }
            }
        }
    }
    n.refs.sort_by_key(|r| r.at);
    for r in &n.refs {
        let at = r.at as usize;
        let end = (at + usize::from(r.width)).min(n.bytes.len());
        n.bytes[at..end].fill(0);
    }
    n
}

/// A jump table: an anonymous read-only chunk of 32-bit entries relative
/// to itself, every one inside the one code chunk `owner` that references
/// it. Its entries become references (relative to the table) into the
/// owner, so the table can go with a copy of it.
fn jump_table(image: &Image, idx: usize, owner: usize) -> Option<Vec<Ref>> {
    let c = &image.chunks[idx];
    let o = &image.chunks[owner];
    if !c.anonymous() || c.kind != Kind::ReadOnly || !c.size.is_multiple_of(4) || c.size == 0 {
        return None;
    }
    if image.relocs.range(c.rva..c.rva + c.size).next().is_some() {
        return None;
    }
    let mut refs = Vec::new();
    for i in (0..c.size).step_by(4) {
        let e = u32_at(&image.mem, (c.rva + i) as usize).ok()? as i32;
        let target = i64::from(c.rva) + i64::from(e);
        if target < i64::from(o.rva) || target >= i64::from(o.rva + o.size) {
            return None;
        }
        refs.push(Ref {
            at: i,
            width: 4,
            anchor: Some(0),
            target: Target::Chunk { idx: owner, off: target - i64::from(o.rva) },
            signed: false,
        });
    }
    Some(refs)
}

/// A table of absolute addresses into one function (the kernel's jump
/// tables: 64-bit entries, each relocated): an anonymous read-only chunk
/// that only `owner` uses, every reference of it into `owner`. Its
/// entries are bound to wherever `owner` is, so it goes with a copy of
/// it, never bound to the old image's copy (whose entries are into the
/// old function's body).
fn absolute_table(image: &Image, n: &Norm, idx: usize, owner: usize) -> bool {
    let c = &image.chunks[idx];
    c.anonymous()
        && c.kind == Kind::ReadOnly
        && !n.refs.is_empty()
        && n.refs.iter().all(|r| {
            r.anchor.is_none() && matches!(r.target, Target::Chunk { idx, .. } if idx == owner)
        })
}

/// An image with each chunk normalized and known by a key.
struct Analyzed<'a> {
    image: &'a Image,
    norms: Vec<Norm>,
    keys: Vec<String>,
    /// A chunk known by its contents: its key's start (`A:<kind>:`, or
    /// `N:<name>@` for a name several chunks have); empty for the others.
    prefix: Vec<String>,
    /// Content hashes computed so far.
    memo: std::cell::RefCell<HashMap<usize, u64>>,
    /// Key -> chunk.
    by_key: HashMap<String, usize>,
    /// Every name (crate-qualified) -> chunk: identical code folding gives
    /// a chunk several.
    by_name: HashMap<String, usize>,
    /// Jump tables: table chunk -> owner.
    tables: HashMap<usize, usize>,
}

fn fnv(data: &[u8], mut h: u64) -> u64 {
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

impl<'a> Analyzed<'a> {
    fn new(image: &'a Image) -> Analyzed<'a> {
        let mut norms: Vec<Norm> = (0..image.chunks.len()).map(|i| normalize(image, i)).collect();
        // Jump tables: an anonymous constant one code chunk takes the
        // address of, every entry pointing into that chunk.
        let mut users: HashMap<usize, BTreeSet<usize>> = HashMap::new();
        for (i, n) in norms.iter().enumerate() {
            for r in &n.refs {
                if let Target::Chunk { idx, .. } = r.target
                    && idx != i
                {
                    users.entry(idx).or_default().insert(i);
                }
            }
        }
        let mut tables = HashMap::new();
        for (&t, u) in &users {
            if u.len() == 1 {
                let owner = *u.iter().next().unwrap_or(&0);
                if image.chunks[owner].kind != Kind::Code {
                    continue;
                }
                if let Some(refs) = jump_table(image, t, owner) {
                    let n = &mut norms[t];
                    for r in &refs {
                        n.bytes[r.at as usize..r.at as usize + 4].fill(0);
                    }
                    n.refs = refs;
                    tables.insert(t, owner);
                } else if absolute_table(image, &norms[t], t, owner) {
                    tables.insert(t, owner);
                }
            }
        }
        // Names: crate-qualified. A name several chunks have (a function
        // copied into several codegen units, generic instances) and a chunk
        // with none (string literals, vtables, jump tables) are known by
        // their contents too, references included: an order of chunks that
        // changes between builds changes nothing.
        let mut by_name = HashMap::new();
        let mut keys = vec![String::new(); image.chunks.len()];
        let mut prefix = vec![String::new(); image.chunks.len()];
        let primary = |c: &Chunk| format!("{}::{}", c.krate, c.name());
        let mut count: HashMap<String, u32> = HashMap::new();
        for c in image.chunks.iter().filter(|c| !c.anonymous() && c.kind != Kind::Other) {
            *count.entry(primary(c)).or_insert(0) += 1;
        }
        for (i, c) in image.chunks.iter().enumerate() {
            if c.kind == Kind::Other {
                continue;
            }
            if c.anonymous() {
                prefix[i] = format!("A:{:?}:", c.kind);
                continue;
            }
            let p = primary(c);
            if count.get(&p) == Some(&1) {
                keys[i] = format!("N:{p}");
            } else {
                prefix[i] = format!("N:{p}@");
            }
            for name in &c.names {
                by_name.entry(format!("{}::{name}", c.krate)).or_insert(i);
            }
        }
        let mut a = Analyzed {
            image,
            norms,
            keys,
            prefix,
            memo: std::cell::RefCell::new(HashMap::new()),
            by_key: HashMap::new(),
            by_name,
            tables,
        };
        let hashes: Vec<Option<u64>> = (0..image.chunks.len())
            .map(|i| (!a.prefix[i].is_empty()).then(|| a.content_hash(i, &mut BTreeSet::new())))
            .collect();
        for (i, h) in hashes.into_iter().enumerate() {
            if let Some(h) = h {
                a.keys[i] = format!("{}{h:016x}", a.prefix[i]);
            }
        }
        a.memo.borrow_mut().clear();
        for (i, k) in a.keys.iter().enumerate() {
            if !k.is_empty() {
                a.by_key.entry(k.clone()).or_insert(i);
            }
        }
        a
    }

    /// A hash of anonymous chunk `i`: its kind, normalized bytes and its
    /// references by what they name.
    fn content_hash(&self, i: usize, busy: &mut BTreeSet<usize>) -> u64 {
        if let Some(&h) = self.memo.borrow().get(&i) {
            return h;
        }
        let n = &self.norms[i];
        let mut h = fnv(&n.bytes, 0xcbf2_9ce4_8422_2325);
        h = fnv(&[self.image.chunks[i].kind as u8], h);
        busy.insert(i);
        for r in &n.refs {
            let key = self.target_key_with(&r.target, busy);
            h = fnv(format!("{}:{}:{:?}:{key}", r.at, r.width, r.anchor).as_bytes(), h);
        }
        busy.remove(&i);
        self.memo.borrow_mut().insert(i, h);
        h
    }

    fn target_key_with(&self, t: &Target, busy: &mut BTreeSet<usize>) -> String {
        match t {
            Target::Chunk { idx, off } => {
                let k = if !self.keys[*idx].is_empty() {
                    self.keys[*idx].clone()
                } else if busy.contains(idx) {
                    "cycle".into()
                } else {
                    format!("{}{:016x}", self.prefix[*idx], self.content_hash(*idx, busy))
                };
                format!("{k}+{off}")
            }
            Target::Import(name) => format!("I:{name}"),
            Target::Header(off) => format!("H:{off}"),
            Target::Linker(name) => format!("L:{name}"),
            Target::Unknown(va) => format!("U:{va:#x}"),
        }
    }

    fn target_key(&self, t: &Target) -> String {
        self.target_key_with(t, &mut BTreeSet::new())
    }

    /// Chunk `i` as compared: bytes and references by what they name
    /// (a reference into itself by offset).
    fn signature(&self, i: usize) -> (Vec<u8>, Vec<String>) {
        let n = &self.norms[i];
        let refs = n
            .refs
            .iter()
            .map(|r| match &r.target {
                Target::Chunk { idx, off } if *idx == i => {
                    format!("{}:{}:{:?}:self+{off}", r.at, r.width, r.anchor)
                }
                t => format!("{}:{}:{:?}:{}", r.at, r.width, r.anchor, self.target_key(t)),
            })
            .collect();
        (n.bytes.clone(), refs)
    }
}

/// What happens to a chunk of the new build.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Fate {
    /// The old image has it as it is: references go there.
    Old(usize),
    /// Copied into the blob (changed or new); `Some(old)` for a changed
    /// function whose old entry jumps to the copy.
    Copy(Option<usize>),
}

/// What changed in one DLL, for people.
#[derive(Clone, Debug, Default)]
pub struct Summary {
    /// Changed functions: (name, old size, new size).
    pub patched: Vec<(String, u32, u32)>,
    /// Why some of them count as changed (`reads changed data X`).
    pub why: Vec<(String, String)>,
    /// Functions only the new build has.
    pub new_functions: Vec<String>,
    /// Data copied into the blob: (name, bytes, kind).
    pub new_data: Vec<(String, u32, Kind)>,
    /// Things the user should know that are not refusals.
    pub notes: Vec<String>,
}

/// One DLL's patch.
#[derive(Clone, Debug, Default)]
pub struct Patch {
    pub machine: Option<Machine>,
    /// The kernel's patch (plan record machine 3), not a DLL's.
    pub kernel: bool,
    /// The guest's path of the file (`\Windows\System32\x.dll`).
    pub path: String,
    pub old_timestamp: u32,
    pub old_size_of_image: u32,
    /// The blob: code, read-only data, then (from `data_off`, a page
    /// boundary) writable data.
    pub blob: Vec<u8>,
    pub data_off: u32,
    /// (at, anchor (relative) or none, width, space (0 the old image, 1
    /// the blob), target offset in it).
    pub fixups: Vec<Fixup>,
    /// (old rva, old size, blob offset, name).
    pub entries: Vec<(u32, u32, u32, String)>,
    /// The new build's file, for later loads.
    pub file: Vec<u8>,
    pub summary: Summary,
}

/// A value the kernel writes into the blob once it knows where the blob
/// and the old image are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fixup {
    pub at: u32,
    /// Relative to the blob's address of this offset; None: absolute.
    pub anchor: Option<u32>,
    pub width: u8,
    /// 0: an RVA in the old image; 1: an offset in the blob.
    pub space: u8,
    pub target: i64,
    /// A 32-bit absolute address the CPU sign-extends (the kernel).
    pub signed: bool,
}

/// Why chunk `i` of the new build differs from chunk `j` of the old
/// (`XTASK_HOTSWAP_WHY=1`, WORKFLOW "Hot swapping DLLs and the kernel into
/// a snapshot"): the first byte and the first reference that differ, and
/// what each build's reference is to. A change that should not be there
/// (code the edit did not touch) is explained by it.
fn why_changed(o: &Analyzed, n: &Analyzed, j: usize, i: usize) -> String {
    let (a, b) = (o.signature(j), n.signature(i));
    let byte = a.0.iter().zip(&b.0).position(|(x, y)| x != y);
    let reference = a.1.iter().zip(&b.1).position(|(x, y)| x != y);
    let mut out = format!(
        "xtask: hot swap: why {} changed: first different byte {byte:?} ({} -> {} bytes), \
         first different reference {reference:?} ({} -> {} references)\n",
        n.image.chunks[i].name(),
        a.0.len(),
        b.0.len(),
        a.1.len(),
        b.1.len()
    );
    let describe = |an: &Analyzed, chunk: usize, k: usize| -> String {
        let Some(r) = an.norms[chunk].refs.get(k) else { return String::new() };
        let what = match r.target {
            Target::Chunk { idx, .. } => {
                let c = &an.image.chunks[idx];
                let norm = &an.norms[idx];
                format!(
                    " ({:?}, {} bytes, input section {}, starts {:02x?})",
                    c.kind,
                    c.size,
                    c.section,
                    &norm.bytes[..norm.bytes.len().min(16)]
                )
            }
            _ => String::new(),
        };
        format!("+{:#x}: {}{what}", r.at, an.target_key(&r.target))
    };
    if let Some(k) = reference {
        out += &format!("xtask: hot swap:   old {}\n", describe(o, j, k));
        out += &format!("xtask: hot swap:   new {}\n", describe(n, i, k));
    }
    out
}

/// Compares `old` with `new` (both gala's builds of the DLL at `path` in
/// the guest) and makes the patch, or says why it cannot be made.
pub fn diff(old: &Image, new: &Image, path: &str, file: &[u8]) -> Result<Patch> {
    if old.machine != new.machine {
        return Err(format!(
            "{path}: the machine changed ({:?} -> {:?})",
            old.machine, new.machine
        ));
    }
    let o = Analyzed::new(old);
    let n = Analyzed::new(new);
    let mut refusals: Vec<String> = Vec::new();
    let mut summary = Summary::default();
    // Thread-local data's template and layout: offsets into it are
    // immediates in the code, invisible as references.
    let tls = |img: &Image| {
        img.sections
            .iter()
            .find(|s| s.0 == ".tls")
            .map(|s| img.mem[s.1 as usize..(s.1 + s.2) as usize].to_vec())
    };
    if tls(old) != tls(new) {
        refusals
            .push("its thread-local data (.tls) changed: offsets into it are in the code".into());
    }
    // The old chunk of each new one, by key.
    let old_of = |i: usize| -> Option<usize> { o.by_key.get(&n.keys[i]).copied() };
    let mut fates: Vec<Option<Fate>> = vec![None; new.chunks.len()];
    let mut changed_data: BTreeSet<usize> = BTreeSet::new();
    for (i, c) in new.chunks.iter().enumerate() {
        if c.kind == Kind::Other || n.tables.contains_key(&i) {
            continue;
        }
        let old_i = old_of(i);
        let same = old_i.is_some_and(|j| o.signature(j) == n.signature(i));
        if !same
            && let Some(j) = old_i
            && std::env::var_os("XTASK_HOTSWAP_WHY").is_some()
        {
            eprint!("{}", why_changed(&o, &n, j, i));
        }
        fates[i] = Some(match (c.kind, old_i, same) {
            (_, Some(j), true) => Fate::Old(j),
            (Kind::Code, Some(j), false) => Fate::Copy(Some(j)),
            (Kind::Code, None, _) => Fate::Copy(None),
            (Kind::Writable, Some(j), false) => {
                let was = &old.chunks[j];
                if was.size != c.size {
                    refusals.push(format!(
                        "static {} changed size ({} -> {} bytes): the running copy keeps \
                         the old layout",
                        c.name(),
                        was.size,
                        c.size
                    ));
                } else {
                    summary.notes.push(format!(
                        "static {}'s initial value differs; the program keeps the value it has",
                        c.name()
                    ));
                }
                Fate::Old(j)
            }
            (Kind::Tls, ..) => continue,
            (_, Some(_), false) => {
                changed_data.insert(i);
                Fate::Copy(None)
            }
            (_, None, _) => Fate::Copy(None),
        });
    }
    // Functions (and read-only data) that read changed read-only data
    // change with it.
    let mut readers: HashMap<usize, BTreeSet<usize>> = HashMap::new();
    for (i, nm) in n.norms.iter().enumerate() {
        for r in &nm.refs {
            if let Target::Chunk { idx, .. } = r.target {
                readers.entry(idx).or_default().insert(i);
            }
        }
    }
    let mut work: Vec<usize> = changed_data.iter().copied().collect();
    while let Some(d) = work.pop() {
        for &u in readers.get(&d).into_iter().flatten() {
            let u = n.tables.get(&u).copied().unwrap_or(u);
            match (&fates[u], new.chunks[u].kind) {
                (Some(Fate::Old(j)), Kind::Code) => {
                    summary.why.push((
                        new.chunks[u].name().to_string(),
                        format!("reads changed data {}", new.chunks[d].name()),
                    ));
                    fates[u] = Some(Fate::Copy(Some(*j)));
                }
                (Some(Fate::Old(_)), Kind::ReadOnly) => {
                    fates[u] = Some(Fate::Copy(None));
                    changed_data.insert(u);
                    work.push(u);
                }
                _ => {}
            }
        }
    }
    // Identical code folding that differs between the builds: an old
    // chunk whose names are now in several chunks (or the reverse) cannot
    // be patched for one of them only.
    for (i, c) in new.chunks.iter().enumerate() {
        if c.kind != Kind::Code {
            continue;
        }
        let olds: BTreeSet<usize> = c
            .names
            .iter()
            .filter_map(|name| o.by_name.get(&format!("{}::{name}", c.krate)).copied())
            .collect();
        let patched = matches!(fates[i], Some(Fate::Copy(Some(_))));
        if olds.len() > 1 && patched {
            refusals.push(format!(
                "{} was {} functions in the old build (identical code folding) and is one now",
                c.name(),
                olds.len()
            ));
        }
        if let Some(Fate::Copy(Some(j))) = fates[i] {
            let names = &old.chunks[j].names;
            let news: BTreeSet<usize> = names
                .iter()
                .filter_map(|name| n.by_name.get(&format!("{}::{name}", old.chunks[j].krate)))
                .copied()
                .collect();
            if news.len() > 1 {
                refusals.push(format!(
                    "{} was one function with {} in the old build (identical code folding) \
                     and they differ now",
                    c.name(),
                    names.iter().filter(|x| *x != c.name()).cloned().collect::<Vec<_>>().join(", ")
                ));
            }
        }
    }
    // What the blob holds: the copied functions and everything they
    // reach that the old image does not have as it is.
    let mut in_blob: BTreeSet<usize> = BTreeSet::new();
    let mut work: Vec<usize> = (0..new.chunks.len())
        .filter(|&i| matches!(fates[i], Some(Fate::Copy(_))) && new.chunks[i].kind == Kind::Code)
        .collect();
    while let Some(i) = work.pop() {
        if !in_blob.insert(i) {
            continue;
        }
        if let Some(t) = n.tables.iter().find(|&(_, &owner)| owner == i).map(|(&t, _)| t) {
            work.push(t);
        }
        for r in &n.norms[i].refs {
            let Target::Chunk { idx, .. } = r.target else { continue };
            if idx == i || in_blob.contains(&idx) {
                continue;
            }
            let c = &new.chunks[idx];
            match (&fates[idx], c.kind) {
                (Some(Fate::Copy(_)), _) => work.push(idx),
                (None, _) if n.tables.contains_key(&idx) => {}
                (None, Kind::Tls) | (Some(Fate::Old(_)), _) => {}
                (None, k) => refusals.push(format!(
                    "{} references {} ({k:?}), which cannot be bound",
                    new.chunks[i].name(),
                    c.name()
                )),
            }
        }
    }
    for &i in &in_blob {
        let c = &new.chunks[i];
        if let Some(p) = &n.norms[i].problem {
            refusals.push(format!("{} {p}", c.name()));
        }
        if c.asm {
            refusals.push(format!(
                "{} is assembly (input section {}: global_asm, an entry stub or a trampoline), \
                 not a compiled function: what it references cannot be known for sure",
                c.name(),
                c.section
            ));
        }
        if c.kind == Kind::Writable && c.anonymous() {
            refusals.push(format!(
                "anonymous writable data ({} bytes at {:#x}) is needed by the new code and \
                 cannot be told from the old image's",
                c.size, c.rva
            ));
        }
        match &fates[i] {
            Some(Fate::Copy(Some(j))) => {
                let was = &old.chunks[*j];
                if was.size < JUMP_LEN {
                    refusals.push(format!(
                        "{} is {} bytes in the old build, too short for the {JUMP_LEN}-byte jump",
                        c.name(),
                        was.size
                    ));
                }
                summary.patched.push((c.name().to_string(), was.size, c.size));
            }
            Some(Fate::Copy(None)) if c.kind == Kind::Code => {
                summary.new_functions.push(c.name().to_string())
            }
            _ if c.kind != Kind::Code => {
                summary.new_data.push((c.name().to_string(), c.size, c.kind))
            }
            _ => {}
        }
    }
    // The image base plus an RVA held as a plain number (no relocation)
    // is invisible to the binding: said, not refused.
    for &i in &in_blob {
        if n.norms[i].refs.iter().any(|r| r.target == Target::Header(0)) {
            summary.notes.push(format!(
                "{} takes the image's base address (__ImageBase): RVAs it adds to it as plain \
                 numbers are the new build's, unchecked",
                new.chunks[i].name()
            ));
        }
    }
    let new_exports: Vec<&String> = new.exports.difference(&old.exports).collect();
    if !new_exports.is_empty() {
        summary.notes.push(format!(
            "new exports are not in the running image's export table (GetProcAddress does not \
             see them): {}",
            new_exports.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }
    if !refusals.is_empty() {
        refusals.sort();
        refusals.dedup();
        return Err(format!("{path}: refused: {}", refusals.join("; ")));
    }
    // Layout: code first, then read-only data (jump tables with their
    // function), then writable data from a page boundary.
    let mut place: HashMap<usize, u32> = HashMap::new();
    let mut blob: Vec<u8> = Vec::new();
    let order: Vec<usize> = {
        let mut v: Vec<usize> = in_blob.iter().copied().collect();
        v.sort_by_key(|&i| {
            let k = match new.chunks[i].kind {
                Kind::Code => 0,
                Kind::ReadOnly => 1,
                _ => 2,
            };
            (k, new.chunks[i].rva)
        });
        v
    };
    let mut data_off = None;
    for &i in &order {
        let c = &new.chunks[i];
        if matches!(c.kind, Kind::Writable) && data_off.is_none() {
            let at = blob.len().next_multiple_of(4096);
            blob.resize(at, 0xcc);
            data_off = Some(at as u32);
        }
        let at = blob.len().next_multiple_of(c.align.clamp(16, 4096) as usize);
        let fill = if c.kind == Kind::Code { 0xcc } else { 0 };
        blob.resize(at, fill);
        place.insert(i, at as u32);
        blob.extend_from_slice(&n.norms[i].bytes);
    }
    let data_off = data_off.unwrap_or_else(|| blob.len().next_multiple_of(4096) as u32);
    // References out of the blob, bound.
    let mut fixups = Vec::new();
    for &i in &order {
        let base = place[&i];
        for r in &n.norms[i].refs {
            let (space, target) = match &r.target {
                Target::Chunk { idx, off } => match (place.get(idx), &fates[*idx]) {
                    (Some(&p), _) => (1, i64::from(p) + off),
                    (None, Some(Fate::Old(j))) => (0, i64::from(old.chunks[*j].rva) + off),
                    _ => {
                        return Err(format!(
                            "{path}: {} references {}, neither copied nor in the old image",
                            new.chunks[i].name(),
                            new.chunks[*idx].name()
                        ));
                    }
                },
                Target::Import(name) => match old.imports.iter().find(|(_, v)| *v == name) {
                    Some((&rva, _)) => (0, i64::from(rva)),
                    None => {
                        return Err(format!(
                            "{path}: refused: {} imports {name}, which the old image does not \
                             (its import table cannot grow)",
                            new.chunks[i].name()
                        ));
                    }
                },
                Target::Header(off) => (0, i64::from(*off)),
                Target::Linker(name) => match old.linker.get(name) {
                    Some(&at) => (0, at),
                    None => {
                        return Err(format!(
                            "{path}: refused: {} references the linker script's {name}, which \
                             the old build does not have",
                            new.chunks[i].name()
                        ));
                    }
                },
                Target::Unknown(va) => {
                    return Err(format!(
                        "{path}: refused: {} references {va:#x}, which is in no chunk of the map",
                        new.chunks[i].name()
                    ));
                }
            };
            fixups.push(Fixup {
                at: base + r.at,
                anchor: r.anchor.map(|a| base + a),
                width: r.width,
                space,
                target,
                signed: r.signed,
            });
        }
    }
    let mut entries = Vec::new();
    for &i in &order {
        if let Some(Fate::Copy(Some(j))) = fates[i] {
            let was = &old.chunks[j];
            entries.push((was.rva, was.size, place[&i], new.chunks[i].name().to_string()));
        }
    }
    summary.patched.sort();
    summary.new_functions.sort();
    Ok(Patch {
        machine: Some(new.machine),
        kernel: new.kernel,
        path: path.to_string(),
        old_timestamp: old.timestamp,
        old_size_of_image: old.size_of_image,
        blob,
        data_off,
        fixups,
        entries,
        file: file.to_vec(),
        summary,
    })
}

impl Summary {
    /// The lines the run prints for one DLL.
    pub fn describe(&self, dll: &str) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "xtask: hot swap {dll}: {} patched function(s), {} new function(s), {} data \
             chunk(s) copied",
            self.patched.len(),
            self.new_functions.len(),
            self.new_data.len()
        );
        for (name, was, now) in &self.patched {
            let why = self.why.iter().find(|(n, _)| n == name).map_or("", |(_, w)| w.as_str());
            let why = if why.is_empty() { String::new() } else { format!(" ({why})") };
            let _ =
                writeln!(s, "xtask: hot swap {dll}:   patched {name} ({was} -> {now} bytes){why}");
        }
        for name in &self.new_functions {
            let _ = writeln!(s, "xtask: hot swap {dll}:   new {name}");
        }
        for (name, size, kind) in &self.new_data {
            let _ = writeln!(s, "xtask: hot swap {dll}:   data {name} ({size} bytes, {kind:?})");
        }
        for note in &self.notes {
            let _ = writeln!(s, "xtask: hot swap {dll}:   note: {note}");
        }
        s
    }
}

/// CRC-32 (zlib's), as the kernel checks the plan with.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// The plan's bytes (ARCHITECTURE "Hot swapping DLLs into a resumed
/// machine", the
/// kernel's `hotswap::Plan`): a 32-byte header (magic, version,
/// image count, nonce, total length, CRC-32 of the rest with its field
/// 0), then per image a record.
pub fn encode(patches: &[Patch], nonce: u64) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(patches.len() as u32).to_le_bytes());
    out.extend_from_slice(&nonce.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // length
    out.extend_from_slice(&0u32.to_le_bytes()); // crc
    for p in patches {
        let mut strings = Vec::new();
        let mut entries = Vec::new();
        for (rva, size, at, name) in &p.entries {
            let off = strings.len() as u32;
            strings.extend(name.bytes().take(200));
            strings.push(0);
            for v in [*rva, *size, *at, off] {
                entries.extend_from_slice(&v.to_le_bytes());
            }
        }
        let mut fixups = Vec::new();
        for f in &p.fixups {
            fixups.extend_from_slice(&f.at.to_le_bytes());
            fixups.extend_from_slice(&f.anchor.unwrap_or(u32::MAX).to_le_bytes());
            fixups.extend_from_slice(&f.target.to_le_bytes());
            fixups.push(f.width);
            fixups.push(f.space);
            fixups.push(u8::from(f.signed));
            fixups.extend_from_slice(&[0; 5]);
        }
        let path = p.path.as_bytes();
        let mut rec = Vec::new();
        rec.push(if p.kernel { KERNEL_MACHINE } else { p.machine.map_or(0, Machine::code) });
        rec.extend_from_slice(&[0; 3]);
        for v in [
            p.old_timestamp,
            p.old_size_of_image,
            path.len() as u32,
            p.blob.len() as u32,
            p.data_off,
            p.fixups.len() as u32,
            p.entries.len() as u32,
            strings.len() as u32,
            p.file.len() as u32,
        ] {
            rec.extend_from_slice(&v.to_le_bytes());
        }
        rec.extend_from_slice(path);
        rec.extend_from_slice(&p.blob);
        rec.extend_from_slice(&fixups);
        rec.extend_from_slice(&entries);
        rec.extend_from_slice(&strings);
        rec.extend_from_slice(&p.file);
        out.extend_from_slice(&((rec.len() + 4) as u32).to_le_bytes());
        out.extend_from_slice(&rec);
    }
    let len = out.len() as u32;
    out[24..28].copy_from_slice(&len.to_le_bytes());
    let crc = crc32(&out);
    out[28..32].copy_from_slice(&crc.to_le_bytes());
    out
}

/// The banner a hot-swapped run starts with (standard error).
pub fn banner(dlls: &[String]) -> String {
    let line = "!".repeat(76);
    format!(
        "{line}\n\
         !!  HOT SWAP (--hot-swap-unsafe {}): THIS IS NOT A REAL RUN.\n\
         !!  Running programs (and the kernel, when named) get code of another\n\
         !!  build spliced into them. The checks refuse what they can see; logic\n\
         !!  that depends on state the old code built, and invariants of data\n\
         !!  whose layout did not change, are not checked. Results are at your\n\
         !!  own risk and never count as a pass: every fix still gets a full run\n\
         !!  from boot before its pull request.\n\
         {line}\n",
        dlls.join(",")
    )
}

/// The result line of a hot-swapped scenario (never a pass).
pub const NOT_A_REAL_RUN: &str = "hot-swapped (not a real run)";

/// The exit status of a hot-swapped `boot-test` whose scenario printed
/// what it expects: distinct from a pass (0) and a failure (1).
pub const EXIT_HOT_SWAPPED: u8 = 3;

/// The name `--hot-swap-unsafe` takes for the kernel.
pub const KERNEL: &str = "kernel";

/// Reads `--hot-swap-unsafe <dll>[,<dll>...]` (only with
/// `--from-snapshot`; never in CI).
pub fn parse_flag(args: &[String]) -> Result<Option<Vec<String>>> {
    let Some(i) = args.iter().position(|a| a == "--hot-swap-unsafe") else {
        return Ok(None);
    };
    let value = args
        .get(i + 1)
        .filter(|v| !v.starts_with("--"))
        .ok_or("--hot-swap-unsafe needs the DLLs (a.dll,b.dll)")?;
    if !args.iter().any(|a| a == "--from-snapshot") {
        return Err("--hot-swap-unsafe works only on a run resumed with --from-snapshot".into());
    }
    if std::env::var_os("CI").is_some() || std::env::var_os("GITHUB_ACTIONS").is_some() {
        return Err(
            "--hot-swap-unsafe is refused in CI: a hot-swapped run is not a real run".into()
        );
    }
    let dlls: Vec<String> = value.split(',').map(|d| d.trim().to_ascii_lowercase()).collect();
    if dlls
        .iter()
        .any(|d| (d != KERNEL && !d.ends_with(".dll")) || d.contains('/') || d.contains('\\'))
    {
        return Err(format!(
            "--hot-swap-unsafe {value}: give `kernel` and DLL file names (kernel,msxml6.dll)"
        ));
    }
    Ok(Some(dlls))
}

/// A hot swap made ready for a resumed run.
#[derive(Debug, Clone)]
pub struct Prepared {
    /// The DLLs swapped (those whose build changed).
    pub dlls: Vec<String>,
    /// The plan's file, given to QEMU as fw_cfg's `opt/gala/hotswap`.
    pub plan: std::path::PathBuf,
}

/// The modules of Limine's config: (path under the image root, the
/// module string: the directory the kernel puts the file in).
fn conf_modules(conf: &str) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = Vec::new();
    for line in conf.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("module_path: boot():/") {
            out.push((rest.split('#').next().unwrap_or(rest).to_string(), None));
        } else if let (Some(dir), Some(last)) =
            (line.strip_prefix("module_string: "), out.last_mut())
        {
            last.1 = Some(dir.to_string());
        }
    }
    out
}

/// Limine's config without the hashes of `paths`' lines (what a hot swap
/// of them changes in it).
fn conf_without(conf: &str, paths: &[String]) -> String {
    conf.lines()
        .map(|l| match l.split_once("boot():/") {
            Some((head, rest)) if paths.iter().any(|p| rest.split('#').next() == Some(p)) => {
                format!("{head}boot():/{}", rest.split('#').next().unwrap_or(rest))
            }
            _ => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Makes the plan for swapping `dlls` (file names) of the image `iso`'s
/// build into snapshot `dir` (whose `modules/` hold its build of them);
/// `differ`: the boot image's files that differ from the snapshot's
/// (`snapshot::check_build_except`). Prints the banner and what each DLL
/// gets to standard error.
pub fn prepare(
    dir: &std::path::Path,
    iso: &std::path::Path,
    dlls: &[String],
    differ: &[String],
) -> Result<Prepared> {
    let old_dir = dir.join("modules");
    let new_dir = crate::snapshot::modules_dir(iso);
    if !old_dir.is_dir() {
        return Err(format!(
            "--hot-swap-unsafe: snapshot {} has no copy of its DLLs and their maps \
             ({} is missing: it was taken before hot swapping existed); take it again",
            dir.display(),
            old_dir.display()
        ));
    }
    let read_text = |p: &std::path::Path| {
        std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
    };
    let old_conf = read_text(&old_dir.join("limine.conf"))?;
    let new_conf = read_text(&new_dir.join("limine.conf"))?;
    let mut swapped: Vec<String> =
        dlls.iter().flat_map(|d| [format!("boot/{d}"), format!("boot/x86/{d}")]).collect();
    swapped.push("boot/gala.manifest".into());
    if dlls.iter().any(|d| d == KERNEL) {
        swapped.push(crate::snapshot::KERNEL.into());
    }
    if conf_without(&old_conf, &swapped) != conf_without(&new_conf, &swapped) {
        return Err(
            "--hot-swap-unsafe: Limine's config differs in more than the swapped DLLs' hashes \
             (the command line or the modules changed): not resumable"
                .into(),
        );
    }
    let modules = conf_modules(&new_conf);
    let mut patches = Vec::new();
    let mut names = Vec::new();
    let mut text = banner(dlls);
    for dll in dlls {
        if dll == KERNEL {
            if !differ.iter().any(|d| d == crate::snapshot::KERNEL) {
                text += "xtask: hot swap kernel: the same in this build as in the snapshot\n";
                continue;
            }
            let patch = kernel_patch(&old_dir, &new_dir)?;
            text += &patch.summary.describe(KERNEL);
            patches.push(patch);
            names.push(KERNEL.to_string());
            continue;
        }
        let mut any = false;
        for path in [format!("boot/{dll}"), format!("boot/x86/{dll}")] {
            if !differ.contains(&path) {
                continue;
            }
            any = true;
            let Some((_, Some(guest_dir))) = modules.iter().find(|(p, _)| *p == path) else {
                return Err(format!("--hot-swap-unsafe: {path} is no module with a directory"));
            };
            let guest = format!("\\{}\\{dll}", guest_dir.replace('/', "\\"));
            let load = |d: &std::path::Path| -> Result<(Vec<u8>, String)> {
                let file = d.join(&path);
                let map = file.with_file_name(format!("{dll}.map"));
                let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
                let map = std::fs::read_to_string(&map).map_err(|e| {
                    format!(
                        "{}: {e} (no linker map: a DLL gala's build did not make, or made \
                         without one, cannot be diffed)",
                        map.display()
                    )
                })?;
                Ok((bytes, map))
            };
            let (old_bytes, old_map) = load(&old_dir)?;
            let (new_bytes, new_map) = load(&new_dir)?;
            let old = Image::read(&old_bytes, &old_map)?;
            let new = Image::read(&new_bytes, &new_map)?;
            let patch = diff(&old, &new, &guest, &new_bytes)?;
            let label = if path.contains("/x86/") { format!("{dll} (x86)") } else { dll.clone() };
            text += &patch.summary.describe(&label);
            patches.push(patch);
        }
        if any {
            names.push(dll.clone());
        } else {
            text += &format!("xtask: hot swap {dll}: the same in this build as in the snapshot\n");
        }
    }
    eprint!("{text}");
    if patches.is_empty() {
        return Err("--hot-swap-unsafe: none of the named DLLs differs from the snapshot's".into());
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64)
        ^ u64::from(std::process::id());
    let plan = iso.with_extension("hotswap.plan");
    std::fs::write(&plan, encode(&patches, nonce.max(1)))
        .map_err(|e| format!("{}: {e}", plan.display()))?;
    Ok(Prepared { dlls: names, plan })
}

/// The kernel's patch: the snapshot's kernel (`old_dir`, its modules)
/// against this build's (`new_dir`), each with its map (`boot/kernel`,
/// `boot/kernel.map`).
fn kernel_patch(old_dir: &std::path::Path, new_dir: &std::path::Path) -> Result<Patch> {
    let load = |d: &std::path::Path| -> Result<(Vec<u8>, String)> {
        let file = d.join(crate::snapshot::KERNEL);
        let map = file.with_extension("map");
        let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let map = std::fs::read_to_string(&map).map_err(|e| {
            format!(
                "{}: {e} (no linker map of the kernel: it is kept beside a snapshot's kernel \
                 since SN step 1e; take the snapshot again)",
                map.display()
            )
        })?;
        Ok((bytes, map))
    };
    let (old_bytes, old_map) = load(old_dir)?;
    let (new_bytes, new_map) = load(new_dir)?;
    if !old_map.lines().any(|l| l.trim_end().ends_with(KERNEL_SUPPORT)) {
        return Err(format!(
            "--hot-swap-unsafe kernel: the snapshot's kernel cannot take a kernel hot swap (it has \
             no {KERNEL_SUPPORT}: it predates SN step 1e); take the snapshot again"
        ));
    }
    let old =
        Image::from_elf(&old_bytes, &old_map).map_err(|e| format!("the snapshot's kernel: {e}"))?;
    let new =
        Image::from_elf(&new_bytes, &new_map).map_err(|e| format!("this build's kernel: {e}"))?;
    let mut patch = diff(&old, &new, KERNEL, &[])?;
    patch.path = KERNEL.into();
    Ok(patch)
}

/// The verdict of a hot-swapped run (never a pass): the guest's refusal,
/// a swap it never applied, or the scenario's own verdict marked as not a
/// real run. The second value: whether the scenario printed what it
/// expects with the swap applied.
pub fn verdict(serial: &str, dlls: &[String], scenario: Result) -> (Result, bool) {
    if let Some(line) = serial.lines().find(|l| l.contains("hotswap: refused:")) {
        let why = line.split("hotswap: refused:").nth(1).unwrap_or(line).trim();
        return (Err(format!("the guest refused the hot swap: {why}")), false);
    }
    for dll in dlls {
        if !serial.contains(&format!("hotswap: applied: {dll}")) {
            return (
                Err(format!(
                    "the guest never applied the hot swap of {dll} (no `hotswap: applied: \
                     {dll}` line; a kernel without hot swap support, or the NMI never came)"
                )),
                false,
            );
        }
    }
    match scenario {
        Ok(()) => (
            Err(format!(
                "{NOT_A_REAL_RUN}: the scenario printed what it expects with {} swapped in; a \
                 hot-swapped run never passes (run it from boot)",
                dlls.join(",")
            )),
            true,
        ),
        Err(e) => (Err(format!("{NOT_A_REAL_RUN}: {e}")), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limine_configs_differ_only_in_the_swapped_hashes() {
        let a = "cmdline: x\n    module_path: boot():/boot/a.dll#11\n    module_string: gala\n    \
                 module_path: boot():/boot/b.dll#22\n";
        let b = a.replace("#11", "#33");
        let swapped = vec!["boot/a.dll".to_string()];
        assert_eq!(conf_without(a, &swapped), conf_without(&b, &swapped));
        let c = a.replace("#22", "#44");
        assert_ne!(conf_without(a, &swapped), conf_without(&c, &swapped));
        assert_eq!(
            conf_modules(a),
            vec![("boot/a.dll".to_string(), Some("gala".to_string())), ("boot/b.dll".into(), None)]
        );
    }

    #[test]
    fn a_hot_swapped_run_never_passes() {
        let dlls = vec!["x.dll".to_string()];
        let applied = "hotswap: applied: x.dll in 1 process(es)\n";
        let (v, ok) = verdict(applied, &dlls, Ok(()));
        assert!(ok);
        assert!(v.unwrap_err().starts_with(NOT_A_REAL_RUN));
        let (v, ok) = verdict(applied, &dlls, Err("lacks y".into()));
        assert!(!ok);
        assert_eq!(v.unwrap_err(), format!("{NOT_A_REAL_RUN}: lacks y"));
        let refused = "hotswap: refused: x.dll!f (changed) is on the stack\n";
        let (v, _) = verdict(refused, &dlls, Ok(()));
        assert!(v.unwrap_err().contains("refused the hot swap: x.dll!f (changed) is on the stack"));
        let (v, _) = verdict("nothing\n", &dlls, Ok(()));
        assert!(v.unwrap_err().contains("never applied"));
    }

    const BASE: u64 = 0x1_8000_0000;

    /// An x64 image with chunks at the given RVAs: (name, kind, bytes).
    fn image(chunks: &[(&str, Kind, u32, &[u8])], relocs: &[(u32, u8)]) -> Image {
        let size = 0x4000u32;
        let mut mem = vec![0u8; size as usize];
        let mut cs = Vec::new();
        for &(name, kind, rva, bytes) in chunks {
            mem[rva as usize..rva as usize + bytes.len()].copy_from_slice(bytes);
            let names = if name.is_empty() { vec![] } else { vec![name.to_string()] };
            let section = String::new();
            let size = bytes.len() as u32;
            let krate = "t".to_string();
            cs.push(Chunk { rva, size, names, krate, kind, section, asm: false, align: 16 });
        }
        cs.sort_by_key(|c| c.rva);
        Image {
            machine: Machine::X64,
            base: BASE,
            size_of_image: size,
            timestamp: 1,
            mem,
            sections: vec![
                (".text".into(), 0x1000, 0x1000, 0x6000_0020),
                (".rdata".into(), 0x2000, 0x1000, 0x4000_0040),
                (".data".into(), 0x3000, 0x1000, 0xc000_0040),
            ],
            relocs: relocs.iter().copied().collect(),
            imports: BTreeMap::from([(0x2ff0, "kernel32.dll!Sleep".to_string())]),
            exports: BTreeSet::new(),
            chunks: cs,
            kernel: false,
            signed: BTreeSet::new(),
            link_refs: BTreeMap::new(),
            linker: BTreeMap::new(),
            reloc_symbols: BTreeMap::new(),
        }
    }

    /// `call rel32` at `at` to `to`.
    fn call(at: u32, to: u32) -> [u8; 5] {
        let rel = (to as i64 - (at as i64 + 5)) as i32;
        let b = rel.to_le_bytes();
        [0xe8, b[0], b[1], b[2], b[3]]
    }

    /// `mov eax, [rip+disp]` at `at` reading `to`, then `ret`.
    fn load(at: u32, to: u32) -> Vec<u8> {
        let rel = (to as i64 - (at as i64 + 6)) as i32;
        let mut v = vec![0x8b, 0x05];
        v.extend_from_slice(&rel.to_le_bytes());
        v.push(0xc3);
        v
    }

    /// A: calls B, pads. B: reads G. G a static.
    fn v1(b_at: u32) -> Image {
        let mut a = call(0x1000, b_at).to_vec();
        a.extend_from_slice(&[0xc3, 0x90, 0x90]);
        let b = load(b_at, 0x3000);
        image(
            &[
                ("A", Kind::Code, 0x1000, &a),
                ("B", Kind::Code, b_at, &b),
                ("G", Kind::Writable, 0x3000, &[1, 0, 0, 0, 0, 0, 0, 0]),
            ],
            &[],
        )
    }

    #[test]
    fn moved_code_is_the_same_and_changed_code_is_patched() {
        let old = v1(0x1010);
        // B moved: A's call bytes differ, its target does not.
        let moved = v1(0x1020);
        let p = diff(&old, &moved, "\\x.dll", b"MZ").unwrap();
        assert!(p.entries.is_empty(), "{:?}", p.entries);
        assert!(p.blob.is_empty());
        // B changed (reads G, adds one): patched; A is not.
        let mut b = load(0x1010, 0x3000);
        b.pop();
        b.extend_from_slice(&[0x83, 0xc0, 0x01, 0xc3]); // add eax, 1; ret
        let mut a = call(0x1000, 0x1010).to_vec();
        a.extend_from_slice(&[0xc3, 0x90, 0x90]);
        let new = image(
            &[
                ("A", Kind::Code, 0x1000, &a),
                ("B", Kind::Code, 0x1010, &b),
                ("G", Kind::Writable, 0x3000, &[1, 0, 0, 0, 0, 0, 0, 0]),
            ],
            &[],
        );
        let p = diff(&old, &new, "\\x.dll", b"MZ").unwrap();
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.entries[0].3, "B");
        assert_eq!(p.summary.patched, vec![("B".to_string(), 7, 10)]);
        // B's copy reads the OLD G: an RVA in the old image.
        assert_eq!(
            p.fixups,
            vec![Fixup {
                at: 2,
                anchor: Some(6),
                width: 4,
                space: 0,
                target: 0x3000,
                signed: false
            }]
        );
        // The displacement's bytes are left for the kernel.
        assert_eq!(&p.blob[..10], &[0x8b, 0x05, 0, 0, 0, 0, 0x83, 0xc0, 0x01, 0xc3]);
    }

    #[test]
    fn a_change_inlined_into_a_caller_changes_the_caller() {
        let old = v1(0x1010);
        // A no longer calls B: it does B's work itself (B inlined).
        let mut a = load(0x1000, 0x3000);
        a.pop();
        a.extend_from_slice(&[0x83, 0xc0, 0x02, 0xc3]);
        let b = load(0x1010, 0x3000);
        let new = image(
            &[
                ("A", Kind::Code, 0x1000, &a),
                ("B", Kind::Code, 0x1010, &b),
                ("G", Kind::Writable, 0x3000, &[1, 0, 0, 0, 0, 0, 0, 0]),
            ],
            &[],
        );
        let p = diff(&old, &new, "\\x.dll", b"MZ").unwrap();
        let names: Vec<&str> = p.entries.iter().map(|e| e.3.as_str()).collect();
        assert_eq!(names, ["A"]);
    }

    #[test]
    fn a_new_function_and_a_new_static_go_into_the_blob() {
        let old = v1(0x1010);
        // B now calls N (new) and N reads H (a new static).
        let mut b = call(0x1010, 0x1030).to_vec();
        b.push(0xc3);
        let n = load(0x1030, 0x3008);
        let mut a = call(0x1000, 0x1010).to_vec();
        a.extend_from_slice(&[0xc3, 0x90, 0x90]);
        let new = image(
            &[
                ("A", Kind::Code, 0x1000, &a),
                ("B", Kind::Code, 0x1010, &b),
                ("N", Kind::Code, 0x1030, &n),
                ("G", Kind::Writable, 0x3000, &[1, 0, 0, 0, 0, 0, 0, 0]),
                ("H", Kind::Writable, 0x3008, &[7, 0, 0, 0, 0, 0, 0, 0]),
            ],
            &[],
        );
        let p = diff(&old, &new, "\\x.dll", b"MZ").unwrap();
        assert_eq!(p.summary.new_functions, vec!["N".to_string()]);
        assert_eq!(p.summary.new_data, vec![("H".to_string(), 8, Kind::Writable)]);
        assert_eq!(p.data_off, 0x1000);
        assert_eq!(&p.blob[0x1000..0x1008], &[7, 0, 0, 0, 0, 0, 0, 0]);
        // B's call goes to N's copy; N's load to H's copy (blob space).
        let n_at = p.fixups.iter().find(|f| f.at == 1).unwrap();
        assert_eq!((n_at.space, n_at.anchor), (1, Some(5)));
        assert!(p.fixups.iter().any(|f| f.space == 1 && f.target == 0x1000));
    }

    #[test]
    fn a_static_that_changed_size_is_refused() {
        let old = v1(0x1010);
        let mut new = v1(0x1010);
        new.chunks[2].size = 16;
        // B changed too, so the static is needed.
        new.mem[0x1016] = 0x90;
        let e = diff(&old, &new, "\\x.dll", b"MZ").unwrap_err();
        assert!(e.contains("static G changed size (8 -> 16 bytes)"), "{e}");
    }

    #[test]
    fn a_function_reading_changed_constants_changes_with_them() {
        // B reads constant K (named, read-only); only K's value changes.
        let b = load(0x1010, 0x2000);
        let mut a = call(0x1000, 0x1010).to_vec();
        a.extend_from_slice(&[0xc3, 0x90, 0x90]);
        let make = |k: u8| {
            image(
                &[
                    ("A", Kind::Code, 0x1000, &a),
                    ("B", Kind::Code, 0x1010, &b),
                    ("K", Kind::ReadOnly, 0x2000, &[k, 0, 0, 0]),
                ],
                &[],
            )
        };
        let p = diff(&make(1), &make(2), "\\x.dll", b"MZ").unwrap();
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.summary.why, vec![("B".to_string(), "reads changed data K".to_string())]);
        assert_eq!(p.summary.new_data, vec![("K".to_string(), 4, Kind::ReadOnly)]);
    }

    #[test]
    fn imports_bind_to_the_old_iat_and_a_new_one_is_refused() {
        // B calls [rip+IAT slot of Sleep].
        let mut b = vec![0xff, 0x15];
        b.extend_from_slice(&((0x2ff0i64 - (0x1010 + 6)) as i32).to_le_bytes());
        b.push(0xc3);
        let mut a = call(0x1000, 0x1010).to_vec();
        a.extend_from_slice(&[0xc3, 0x90, 0x90]);
        let old = image(&[("A", Kind::Code, 0x1000, &a), ("B", Kind::Code, 0x1010, &b)], &[]);
        let mut b2 = b.clone();
        b2.insert(6, 0x90);
        // The same call, one byte later: still Sleep's slot.
        b2[2..6].copy_from_slice(&((0x2ff0i64 - (0x1010 + 6)) as i32).to_le_bytes());
        let b2 = [&[0x90u8][..], &b2[..b2.len() - 1]].concat();
        let mut b2 = b2;
        b2[3..7].copy_from_slice(&((0x2ff0i64 - (0x1011 + 6)) as i32).to_le_bytes());
        b2.push(0xc3);
        let new = image(&[("A", Kind::Code, 0x1000, &a), ("B", Kind::Code, 0x1010, &b2)], &[]);
        let p = diff(&old, &new, "\\x.dll", b"MZ").unwrap();
        assert_eq!(p.fixups.len(), 1);
        assert_eq!((p.fixups[0].space, p.fixups[0].target), (0, 0x2ff0));
        let mut new2 = new.clone();
        new2.imports = BTreeMap::from([(0x2ff0, "kernel32.dll!SleepEx".to_string())]);
        let e = diff(&old, &new2, "\\x.dll", b"MZ").unwrap_err();
        assert!(e.contains("imports kernel32.dll!SleepEx"), "{e}");
    }

    #[test]
    fn a_jump_table_goes_with_its_function() {
        // F: lea rcx,[rip+T]; movsxd rax,[rcx+rax*4]; add rax,rcx; jmp rax;
        // two cases (nop; ret) and T: two entries relative to T.
        let mut f = vec![0x48, 0x8d, 0x0d];
        f.extend_from_slice(&((0x2000i64 - (0x1010 + 7)) as i32).to_le_bytes());
        f.extend_from_slice(&[0x48, 0x63, 0x04, 0x81, 0x48, 0x01, 0xc8, 0xff, 0xe0]);
        let case0 = 0x1010 + f.len() as u32;
        f.extend_from_slice(&[0x90, 0xc3]);
        let case1 = 0x1010 + f.len() as u32;
        f.extend_from_slice(&[0x31, 0xc0, 0xc3]);
        let t: Vec<u8> = [case0, case1]
            .iter()
            .flat_map(|c| ((*c as i64 - 0x2000) as i32).to_le_bytes())
            .collect();
        let mut a = call(0x1000, 0x1010).to_vec();
        a.extend_from_slice(&[0xc3, 0x90, 0x90]);
        let old = image(
            &[
                ("A", Kind::Code, 0x1000, &a),
                ("F", Kind::Code, 0x1010, &f),
                ("", Kind::ReadOnly, 0x2000, &t),
            ],
            &[],
        );
        // F changed in its last case: copied with its table.
        let mut f2 = f.clone();
        let n = f2.len();
        f2[n - 2] = 0xc8;
        let new = image(
            &[
                ("A", Kind::Code, 0x1000, &a),
                ("F", Kind::Code, 0x1010, &f2),
                ("", Kind::ReadOnly, 0x2000, &t),
            ],
            &[],
        );
        let p = diff(&old, &new, "\\x.dll", b"MZ").unwrap();
        assert_eq!(p.entries.len(), 1);
        // The table is in the blob after F, its entries relative to it
        // and pointing into F's copy (offsets case0-F, case1-F).
        let table = p.fixups.iter().filter(|f| f.anchor == Some(f.at - f.at % 8)).count();
        assert!(table >= 1, "{:?}", p.fixups);
        let into_f: Vec<i64> = p
            .fixups
            .iter()
            .filter(|f| f.space == 1 && f.width == 4 && f.at >= 32)
            .map(|f| f.target)
            .collect();
        assert_eq!(into_f, vec![i64::from(case0 - 0x1010), i64::from(case1 - 0x1010)]);
        // F unchanged but moved: the table's entries moved with it, still
        // the same.
        let same = diff(&old, &old, "\\x.dll", b"MZ").unwrap();
        assert!(same.entries.is_empty());
    }

    #[test]
    fn a_short_function_cannot_take_the_jump() {
        let old = image(&[("A", Kind::Code, 0x1000, &[0x31, 0xc0, 0xc3])], &[]);
        let new = image(&[("A", Kind::Code, 0x1000, &[0x33, 0xc0, 0xc3])], &[]);
        let e = diff(&old, &new, "\\x.dll", b"MZ").unwrap_err();
        assert!(e.contains("A is 3 bytes in the old build, too short"), "{e}");
    }

    #[test]
    fn code_that_does_not_decode_is_refused() {
        let old = image(&[("A", Kind::Code, 0x1000, &[0x31, 0xc0, 0x90, 0x90, 0x90, 0xc3])], &[]);
        let new = image(&[("A", Kind::Code, 0x1000, &[0x31, 0xc0, 0x90, 0x90, 0x0f, 0xff])], &[]);
        let e = diff(&old, &new, "\\x.dll", b"MZ").unwrap_err();
        assert!(e.contains("does not decode"), "{e}");
    }

    #[test]
    fn maps_are_read_with_their_names_normalized() {
        let map = "Address  Size     Align Out     In      Symbol\n\
00001000 00000039  4096 .text\n\
00001000 00000005    16         /t/deps/x.x.1526-cgu.0.rcgu.o:(.text)\n\
00001000 00000000     0                 x_add\n\
00001000 00000000     0                 x_add\n\
00001010 00000008    16         /t/deps/core-1234.core.99-cgu.0.rcgu.o:(.text$unlikely)\n\
00001010 00000000     0                 core::f (.llvm.15847438787998897310)\n\
00002000 00000106  4096 .rdata\n\
00002000 00000008     8         /t/deps/x.x.1526-cgu.0.rcgu.o:(.rdata)\n";
        let e = parse_map(map).unwrap();
        assert_eq!(e.len(), 3);
        assert_eq!(
            e[0],
            (0x1000, 5, ".text".into(), ".text".into(), "x".into(), vec!["x_add".into()])
        );
        assert_eq!(e[1].3, ".text$unlikely");
        assert_eq!(e[1].4, "core");
        assert_eq!(e[1].5, vec!["core::f".to_string()]);
        assert_eq!(normalize_name("_anon.22.3.llvm.1900421780943794266"), "_anon.22.3");
        assert_eq!(normalize_name("f (.llvm.10850011815101013523@12)"), "f");
        assert_eq!(normalize_name("_g.llvm.77@8"), "_g@8");
        // A v0 name's crate disambiguator differs between builds with
        // other features; the rest is the name.
        assert_eq!(
            normalize_name("__RNvCsk24xeMuKz3_11hotswap_dll7obj_get@4"),
            normalize_name("__RNvCs1t9zePC0vaa_11hotswap_dll7obj_get@4")
        );
        assert_eq!(
            normalize_name("__RNvCsk24xeMuKz3_11hotswap_dll7obj_get@4"),
            "__RNvC11hotswap_dll7obj_get@4"
        );
        assert_eq!(e[2].2, ".rdata");
        assert!(e[2].5.is_empty());
    }

    #[test]
    fn the_plan_has_its_length_and_checksum() {
        let old = v1(0x1010);
        let mut new = v1(0x1010);
        new.mem[0x1016] = 0x90;
        let p = diff(&old, &new, "\\Windows\\System32\\x.dll", b"MZfile").unwrap();
        let bytes = encode(&[p], 7);
        assert_eq!(&bytes[..8], MAGIC);
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()) as usize, bytes.len());
        let mut zeroed = bytes.clone();
        zeroed[28..32].fill(0);
        assert_eq!(u32::from_le_bytes(bytes[28..32].try_into().unwrap()), crc32(&zeroed));
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn the_flag_needs_a_snapshot_and_is_refused_in_ci() {
        let args = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert_eq!(parse_flag(&args("boot-test x")), Ok(None));
        assert!(parse_flag(&args("boot-test x --hot-swap-unsafe a.dll")).is_err());
        assert!(parse_flag(&args("boot-test --from-snapshot s --hot-swap-unsafe a.exe")).is_err());
        if std::env::var_os("CI").is_none() && std::env::var_os("GITHUB_ACTIONS").is_none() {
            assert_eq!(
                parse_flag(&args("boot-test --from-snapshot s --hot-swap-unsafe A.dll,b.dll")),
                Ok(Some(vec!["a.dll".to_string(), "b.dll".to_string()]))
            );
            assert_eq!(
                parse_flag(&args("boot-test --from-snapshot s --hot-swap-unsafe kernel,a.dll")),
                Ok(Some(vec!["kernel".to_string(), "a.dll".to_string()]))
            );
            assert!(
                parse_flag(&args("boot-test --from-snapshot s --hot-swap-unsafe kernel.exe"))
                    .is_err()
            );
        }
    }

    const KBASE: u64 = 0xffff_ffff_8000_0000;

    /// `image`'s chunks as the kernel's: its base in the top 2 GiB,
    /// `signed` the sites of sign-extended 32-bit absolute addresses.
    fn kernel(chunks: &[(&str, Kind, u32, &[u8])], relocs: &[(u32, u8)], signed: &[u32]) -> Image {
        let mut i = image(chunks, relocs);
        i.base = KBASE;
        i.kernel = true;
        i.imports.clear();
        i.signed = signed.iter().copied().collect();
        i
    }

    /// `mov eax, [abs32]` (sign-extended) reading `to`, then `ret`: the
    /// kernel code model's absolute addressing.
    fn load_abs(to: u32) -> Vec<u8> {
        let mut v = vec![0x8b, 0x04, 0x25];
        v.extend_from_slice(&((KBASE + u64::from(to)) as u32).to_le_bytes());
        v.push(0xc3);
        v
    }

    #[test]
    fn the_kernel_binds_sign_extended_addresses_and_marks_its_record() {
        // B reads G through a sign-extended 32-bit absolute address (its
        // relocation at +3); B changes (one more instruction).
        let make = |extra: bool| {
            let mut b = load_abs(0x3000);
            if extra {
                b.insert(0, 0x90);
            }
            let at = if extra { 0x1014 } else { 0x1013 };
            let mut a = call(0x1000, 0x1010).to_vec();
            a.extend_from_slice(&[0xc3, 0x90, 0x90]);
            kernel(
                &[
                    ("A", Kind::Code, 0x1000, &a),
                    ("B", Kind::Code, 0x1010, &b),
                    ("G", Kind::Writable, 0x3000, &[1, 0, 0, 0, 0, 0, 0, 0]),
                ],
                &[(at, 4)],
                &[at],
            )
        };
        let p = diff(&make(false), &make(true), KERNEL, &[]).unwrap();
        assert!(p.kernel);
        assert_eq!(p.entries.len(), 1);
        // B's copy reads the OLD G: an RVA of the old kernel, sign-extended.
        assert_eq!(
            p.fixups,
            vec![Fixup { at: 4, anchor: None, width: 4, space: 0, target: 0x3000, signed: true }]
        );
        let plan = encode(&[p], 1);
        // The record's machine byte: after the header and its length.
        assert_eq!(plan[32 + 4], KERNEL_MACHINE);
        // The fixup's flag.
        assert!(plan.windows(2).any(|w| w == [4, 0]));
    }

    #[test]
    fn a_table_of_absolute_addresses_goes_with_its_function() {
        // F: jmp [abs32 + rax*8] through T (two 64-bit entries into F);
        // T anonymous, only F uses it. F changes in a case, T does not
        // (same offsets): T must be copied with F, never bound to the old
        // T (whose entries are into the old F).
        let make = |case: u8| {
            let mut f = vec![0xff, 0x24, 0xc5];
            f.extend_from_slice(&((KBASE + 0x2000) as u32).to_le_bytes());
            let case0 = 0x1010 + f.len() as u32;
            f.extend_from_slice(&[0x90, 0xc3]);
            let case1 = 0x1010 + f.len() as u32;
            f.extend_from_slice(&[0x31, case, 0xc3]);
            let t: Vec<u8> =
                [case0, case1].iter().flat_map(|c| (KBASE + u64::from(*c)).to_le_bytes()).collect();
            let mut a = call(0x1000, 0x1010).to_vec();
            a.extend_from_slice(&[0xc3, 0x90, 0x90]);
            kernel(
                &[
                    ("A", Kind::Code, 0x1000, &a),
                    ("F", Kind::Code, 0x1010, &f),
                    ("", Kind::ReadOnly, 0x2000, &t),
                ],
                &[(0x1013, 4), (0x2000, 8), (0x2008, 8)],
                &[0x1013],
            )
        };
        let p = diff(&make(0xc0), &make(0xc8), KERNEL, &[]).unwrap();
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.summary.new_data.len(), 1, "{:?}", p.summary);
        // Both entries are bound into F's copy (blob space, offsets 7 and 9).
        let entries: Vec<(u8, i64)> =
            p.fixups.iter().filter(|f| f.width == 8).map(|f| (f.space, f.target)).collect();
        assert_eq!(entries, vec![(1, 7), (1, 9)]);
        // F's table reference is to the copy too.
        assert!(p.fixups.iter().any(|f| f.at == 3 && f.space == 1 && f.signed));
    }

    #[test]
    fn assembly_is_never_copied() {
        let make = |b: u8| {
            let mut i = kernel(
                &[("stub", Kind::Code, 0x1000, &[0x90, b, 0x90, 0x90, 0x90, 0xc3])],
                &[],
                &[],
            );
            i.chunks[0].asm = true;
            i.chunks[0].section = ".text.context".into();
            i
        };
        let e = diff(&make(0x90), &make(0xcc), KERNEL, &[]).unwrap_err();
        assert!(e.contains("stub is assembly (input section .text.context"), "{e}");
    }

    #[test]
    fn functions_of_one_name_are_known_by_their_contents() {
        // Two copies of D (as codegen units make of an #[inline] function),
        // in either order: nothing changed.
        let d1 = [0x31, 0xc0, 0x90, 0x90, 0x90, 0xc3];
        let d2 = [0x31, 0xc9, 0x90, 0x90, 0x90, 0xc3];
        let make = |first: &[u8], second: &[u8]| {
            kernel(&[("D", Kind::Code, 0x1000, first), ("D", Kind::Code, 0x1010, second)], &[], &[])
        };
        let p = diff(&make(&d1, &d2), &make(&d2, &d1), KERNEL, &[]).unwrap();
        assert!(p.entries.is_empty() && p.blob.is_empty(), "{:?}", p.summary);
    }

    #[test]
    fn a_reference_one_past_a_string_is_to_that_string() {
        // C takes the end of the 1-byte string S (S + 1 = where T starts):
        // the relocation names S, so the reference is S+1, not T+0, and a
        // build where T moves away is the same.
        let make = |t_at: u32| {
            let mut c = vec![0x48, 0xc7, 0xc0];
            c.extend_from_slice(&((KBASE + 0x2001) as u32).to_le_bytes());
            c.push(0xc3);
            let mut i = kernel(
                &[
                    ("C", Kind::Code, 0x1000, &c),
                    ("", Kind::ReadOnly, 0x2000, &[0]),
                    ("", Kind::ReadOnly, t_at, b"t\0"),
                ],
                &[(0x1003, 4)],
                &[0x1003],
            );
            i.reloc_symbols.insert(0x1003, 0x2000);
            i
        };
        let p = diff(&make(0x2001), &make(0x2010), KERNEL, &[]).unwrap();
        assert!(p.entries.is_empty(), "{:?}", p.summary);
    }

    #[test]
    fn the_kernels_map_is_read() {
        let map = "             VMA              LMA     Size Align Out     In      Symbol\n\
               0                0 ffffffff80000000     1 . = 0xffffffff80000000\n\
ffffffff80000000                0        0     1 __kernel_start = .\n\
ffffffff800cb000 ffffffff800cb000      312    16 .text\n\
ffffffff800cb000 ffffffff800cb000        9    16         /t/deps/kernel-ceb3.ab6.1nm.rcgu.o:(.text.kmain)\n\
ffffffff800cb000 ffffffff800cb000        9     1                 kmain\n\
ffffffff800cb010 ffffffff800cb010       26    16         /t/deps/libkernel-e55c.rlib(kernel-e55c.00q.1gk.rcgu.o):(.text._RNvCs8_6kernel4main)\n\
ffffffff800cb010 ffffffff800cb010       26     1                 kernel::main (.llvm.15847438787998897310)\n\
ffffffff800cb040 ffffffff800cb040        a    16         <internal>:(.text)\n";
        let (entries, symbols) = parse_elf_map(map).unwrap();
        assert_eq!(symbols.get("__kernel_start"), Some(&KBASE));
        assert!(!symbols.contains_key("."));
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries[1],
            ElfMapEntry {
                va: 0xffff_ffff_800c_b010,
                size: 0x26,
                align: 16,
                out: ".text".into(),
                input: ".text._RNvCs8_6kernel4main".into(),
                krate: "kernel".into(),
                names: vec!["kernel::main".into()],
            }
        );
        assert_eq!(entries[0].krate, "kernel");
        assert_eq!(entries[2].krate, "<internal>");
    }
}
