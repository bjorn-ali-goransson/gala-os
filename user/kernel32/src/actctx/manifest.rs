//! Manifests read into an activation context's model: the assemblies
//! (the manifest's own and the ones it depends on), their files, window
//! classes and COM classes, the run level and the compatibility list.
//!
//! The format is Microsoft's application and assembly manifest schema
//! (learn.microsoft.com, "Manifest file schema"); which manifests Windows
//! refuses and how dependencies are found are as Wine's conformance tests
//! record Windows doing it (`dlls/kernel32/tests/actctx.c`; THIRD_PARTY.md).
//! A manifest is XML in UTF-8 or, with its byte-order mark, UTF-16; the
//! reader below knows only what manifests use: elements, attributes,
//! namespaces (default and prefixed), text content, comments and the
//! `<?xml ...?>` declaration. Entities are not expanded.
//!
//! Refused (ERROR_SXS_CANT_GEN_ACTCTX, with the reason in the ledger as
//! `kernel32.dll!CreateActCtxW: <reason>`): a root that is not
//! `<assembly>` in `urn:schemas-microsoft-com:asm.v1` (v2 and v3 count as
//! v1), no manifestVersion="1.0", an unknown attribute on `<assembly>`,
//! an unknown element of the assembly namespaces under `<assembly>` or
//! `<file>`, a `<file>` without a name or with a hash that is not 40 hex
//! digits, a second requestedExecutionLevel, anything after the root, and
//! a dependency that is not found (or is found with another version).

use crate::buf::Buf;

/// A string kept in the model's pool: `len` units at `at`.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Str {
    pub at: u32,
    pub len: u32,
}

impl Str {
    pub fn is_empty(self) -> bool {
        self.len == 0
    }
}

pub type Guid = [u8; 16];

#[derive(Clone, Copy, Default)]
pub struct Identity {
    pub name: Str,
    /// The `type` attribute (win32).
    pub kind: Str,
    pub arch: Str,
    pub key: Str,
    pub lang: Str,
    pub version: [u16; 4],
}

#[derive(Clone, Copy, Default)]
pub struct Assembly {
    pub id: Identity,
    /// The manifest's full path (empty: none, a system assembly gala
    /// provides itself).
    pub manifest: Str,
    /// The assembly's directory name (dependencies only).
    pub directory: Str,
    pub first_file: u32,
    pub files: u32,
    /// ACTCTX_REQUESTED_RUN_LEVEL and uiAccess (the root's trustInfo).
    pub run_level: u32,
    pub ui_access: u32,
}

#[derive(Clone, Copy)]
pub struct File {
    pub name: Str,
    pub assembly: u32,
}

#[derive(Clone, Copy)]
pub struct WindowClass {
    pub file: u32,
    pub name: Str,
    pub versioned: bool,
}

#[derive(Clone, Copy)]
pub struct ComClass {
    pub file: u32,
    pub clsid: Guid,
    pub tlbid: Guid,
    pub model: u32,
    pub progid: Str,
    pub first_progid: u32,
    pub progids: u32,
    /// miscStatus, miscStatusContent, miscStatusThumbnail,
    /// miscStatusIcon, miscStatusDocPrint (OLEMISC bits).
    pub misc: [u32; 5],
}

/// COMPATIBILITY_CONTEXT_ELEMENT's content.
#[derive(Clone, Copy)]
pub struct Compat {
    pub id: Guid,
    /// ACTCTX_COMPATIBILITY_ELEMENT_TYPE: 1 OS, 3 maxversiontested.
    pub kind: u32,
    pub max_version: u64,
}

#[derive(Clone, Copy)]
struct Dependency {
    id: Identity,
    /// optional="yes" or allowDelayedBinding="true": may be missing.
    optional: bool,
}

/// Manifest content gala reads but does not build sections for yet.
pub const SKIPPED_TYPELIB: u32 = 1;
pub const SKIPPED_PROXY_STUB: u32 = 2;
pub const SKIPPED_CLR: u32 = 4;
pub const SKIPPED_WINRT: u32 = 8;
pub const SKIPPED_LOAD_FROM: u32 = 16;

/// Everything a context knows; strings live in `pool`.
pub struct Model {
    pub pool: Buf<u16>,
    pub assemblies: Buf<Assembly>,
    pub files: Buf<File>,
    pub classes: Buf<WindowClass>,
    pub coms: Buf<ComClass>,
    pub progids: Buf<Str>,
    /// The root assembly's compatibility list, in manifest order.
    pub compat: Buf<Compat>,
    /// The application directory (with its final backslash).
    pub appdir: Str,
    /// SKIPPED_* bits.
    pub skipped: u32,
    deps: Buf<Dependency>,
}

/// Why a manifest was refused (the ledger's text) and where (the
/// character offset).
pub struct Refused {
    pub reason: &'static [u8],
    pub at: usize,
}

type Res<T> = Result<T, Refused>;

const ASM_V1: &[u8] = b"urn:schemas-microsoft-com:asm.v1";
const ASM_V2: &[u8] = b"urn:schemas-microsoft-com:asm.v2";
const ASM_V3: &[u8] = b"urn:schemas-microsoft-com:asm.v3";
const COMPAT_NS: &[u8] = b"urn:schemas-microsoft-com:compatibility.v1";
const WINRT_NS: &[u8] = b"urn:schemas-microsoft-com:winrt.v1";
/// Microsoft's public key token: its system assemblies (WinSxS).
const MICROSOFT_KEY: &[u8] = b"6595b64144ccf1df";

#[cfg(target_arch = "x86_64")]
const CURRENT_ARCH: &[u8] = b"amd64";
#[cfg(target_arch = "x86")]
const CURRENT_ARCH: &[u8] = b"x86";

fn eq_ascii(s: &[u16], a: &[u8]) -> bool {
    s.len() == a.len() && s.iter().zip(a).all(|(&c, &b)| c == u16::from(b))
}

fn lower(c: u16) -> u16 {
    if (0x41..=0x5a).contains(&c) { c + 0x20 } else { c }
}

fn eq_ascii_ci(s: &[u16], a: &[u8]) -> bool {
    s.len() == a.len() && s.iter().zip(a).all(|(&c, &b)| lower(c) == lower(u16::from(b)))
}

pub fn eq_ci(a: &[u16], b: &[u16]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| upper(x) == upper(y))
}

/// RtlUpcaseUnicodeChar for the characters manifests use (ASCII and
/// Latin-1).
pub fn upper(c: u16) -> u16 {
    match c {
        0x61..=0x7a => c - 0x20,
        0xe0..=0xfe if c != 0xf7 => c - 0x20,
        _ => c,
    }
}

fn is_space(c: u16) -> bool {
    matches!(c, 0x20 | 0x09 | 0x0a | 0x0d)
}

/// A range of the manifest text.
#[derive(Clone, Copy, Default)]
struct Span {
    start: usize,
    end: usize,
}

#[derive(Clone, Copy, Default)]
struct Elem {
    name: Span,
    /// The namespace URI (resolved), or empty.
    ns: Span,
    /// The namespace stack's depth inside this element.
    ns_depth: usize,
}

#[derive(Clone, Copy)]
struct Attr {
    name: Span,
    value: Span,
}

const MAX_NAMESPACES: usize = 32;

/// The XML reader over one manifest's text.
struct Reader<'a> {
    s: &'a [u16],
    pos: usize,
    /// (prefix, uri) declarations in scope.
    ns: [(Span, Span); MAX_NAMESPACES],
    ns_len: usize,
}

impl<'a> Reader<'a> {
    fn text(&self, span: Span) -> &'a [u16] {
        &self.s[span.start..span.end]
    }

    fn refuse<T>(&self, reason: &'static [u8]) -> Res<T> {
        Err(Refused { reason, at: self.pos })
    }

    fn skip_space(&mut self) {
        while self.pos < self.s.len() && is_space(self.s[self.pos]) {
            self.pos += 1;
        }
    }

    /// The next attribute of the element being read; None at its end
    /// (`*end` set for `/>`).
    fn next_attr(&mut self, end: &mut bool) -> Res<Option<Attr>> {
        self.skip_space();
        let s = self.s;
        if self.pos >= s.len() {
            return self.refuse(b"unterminated element");
        }
        if s[self.pos] == u16::from(b'/') {
            self.pos += 1;
            if self.pos >= s.len() || s[self.pos] != u16::from(b'>') {
                return self.refuse(b"bad empty element");
            }
            self.pos += 1;
            *end = true;
            return Ok(None);
        }
        if s[self.pos] == u16::from(b'>') {
            self.pos += 1;
            return Ok(None);
        }
        let start = self.pos;
        while self.pos < s.len()
            && s[self.pos] != u16::from(b'=')
            && s[self.pos] != u16::from(b'>')
            && !is_space(s[self.pos])
        {
            self.pos += 1;
        }
        let name = Span { start, end: self.pos };
        self.skip_space();
        if self.pos >= s.len() || s[self.pos] != u16::from(b'=') {
            return self.refuse(b"attribute without a value");
        }
        self.pos += 1;
        self.skip_space();
        if self.pos >= s.len()
            || (s[self.pos] != u16::from(b'"') && s[self.pos] != u16::from(b'\''))
        {
            return self.refuse(b"attribute value not quoted");
        }
        let quote = s[self.pos];
        self.pos += 1;
        let vstart = self.pos;
        while self.pos < s.len() && s[self.pos] != quote {
            self.pos += 1;
        }
        if self.pos >= s.len() {
            return self.refuse(b"unterminated attribute value");
        }
        let value = Span { start: vstart, end: self.pos };
        self.pos += 1;
        Ok(Some(Attr { name, value }))
    }

    fn is_xmlns(&self, a: &Attr) -> bool {
        let n = self.text(a.name);
        n.len() >= 5 && eq_ascii(&n[..5], b"xmlns") && (n.len() == 5 || n[5] == u16::from(b':'))
    }

    /// The URI bound to `prefix` (empty if none).
    fn resolve(&self, prefix: Span) -> Span {
        let p = self.text(prefix);
        for &(name, uri) in self.ns[..self.ns_len].iter().rev() {
            if self.text(name) == p {
                return uri;
            }
        }
        Span::default()
    }

    /// The next child element of `parent`; None when `parent` ends (its
    /// closing tag consumed).
    fn next_elem(&mut self, parent: &Elem) -> Res<Option<Elem>> {
        self.ns_len = parent.ns_depth;
        let s = self.s;
        loop {
            while self.pos < s.len() && s[self.pos] != u16::from(b'<') {
                self.pos += 1;
            }
            if self.pos >= s.len() {
                return self.refuse(b"missing closing tag");
            }
            self.pos += 1;
            let rest = &s[self.pos..];
            if rest.len() >= 3 && eq_ascii(&rest[..3], b"!--") {
                let mut i = self.pos + 3;
                while i + 3 <= s.len() && !eq_ascii(&s[i..i + 3], b"-->") {
                    i += 1;
                }
                if i + 3 > s.len() {
                    return self.refuse(b"unterminated comment");
                }
                self.pos = i + 3;
                continue;
            }
            break;
        }
        if self.pos < s.len() && s[self.pos] == u16::from(b'/') {
            self.pos += 1;
            let (prefix, name) = self.read_name();
            let ns = self.resolve(prefix);
            if self.text(name) != self.text(parent.name) || self.text(ns) != self.text(parent.ns) {
                return self.refuse(b"wrong closing tag");
            }
            self.skip_space();
            if self.pos >= s.len() || s[self.pos] != u16::from(b'>') {
                return self.refuse(b"bad closing tag");
            }
            self.pos += 1;
            return Ok(None);
        }
        let (prefix, name) = self.read_name();
        if self.text(name).first() == Some(&u16::from(b'?')) {
            // A processing instruction (the XML declaration): its
            // pseudo-attributes end with "?>", read by the caller.
            return Ok(Some(Elem { name, ns: Span::default(), ns_depth: self.ns_len }));
        }
        // Its namespace declarations, read ahead (the attributes are read
        // again by the element's handler).
        let save = self.pos;
        let mut end = false;
        while let Some(a) = self.next_attr(&mut end)? {
            if self.is_xmlns(&a) {
                if self.ns_len == MAX_NAMESPACES {
                    return self.refuse(b"too many namespaces");
                }
                let n = a.name;
                let p = if n.end - n.start > 5 {
                    Span { start: n.start + 6, end: n.end }
                } else {
                    Span { start: n.start, end: n.start }
                };
                self.ns[self.ns_len] = (p, a.value);
                self.ns_len += 1;
            }
        }
        self.pos = save;
        let ns = self.resolve(prefix);
        Ok(Some(Elem { name, ns, ns_depth: self.ns_len }))
    }

    /// An element name at `pos`: (prefix, local name).
    fn read_name(&mut self) -> (Span, Span) {
        let s = self.s;
        let start = self.pos;
        let mut prefix = Span { start, end: start };
        let mut name_start = start;
        while self.pos < s.len()
            && !is_space(s[self.pos])
            && s[self.pos] != u16::from(b'>')
            && s[self.pos] != u16::from(b'/')
        {
            if s[self.pos] == u16::from(b':') {
                prefix = Span { start, end: self.pos };
                name_start = self.pos + 1;
            }
            self.pos += 1;
        }
        (prefix, Span { start: name_start, end: self.pos })
    }

    /// The text up to the next markup.
    fn content(&mut self) -> Res<Span> {
        let start = self.pos;
        while self.pos < self.s.len() && self.s[self.pos] != u16::from(b'<') {
            self.pos += 1;
        }
        if self.pos >= self.s.len() {
            return self.refuse(b"unterminated text");
        }
        Ok(Span { start, end: self.pos })
    }

    /// Reads past an element gala does not interpret (its attributes and
    /// children).
    fn skip(&mut self, elem: &Elem) -> Res<()> {
        let mut end = false;
        while self.next_attr(&mut end)?.is_some() {}
        if end {
            return Ok(());
        }
        while let Some(child) = self.next_elem(elem)? {
            self.skip(&child)?;
        }
        Ok(())
    }

    /// Reads an element's attributes, none expected but namespaces.
    fn no_attrs(&mut self) -> Res<bool> {
        let mut end = false;
        while self.next_attr(&mut end)?.is_some() {}
        Ok(end)
    }

    /// `elem` is `name` in namespace `ns` (asm.v2 and v3 stand for v1, v3
    /// for v2).
    fn is(&self, elem: &Elem, name: &[u8], ns: &[u8]) -> bool {
        if !eq_ascii(self.text(elem.name), name) {
            return false;
        }
        let e = self.text(elem.ns);
        if eq_ascii(e, ns) {
            return true;
        }
        if ns == ASM_V1 {
            return eq_ascii(e, ASM_V2) || eq_ascii(e, ASM_V3);
        }
        ns == ASM_V2 && eq_ascii(e, ASM_V3)
    }

    /// In one of the assembly namespaces (an element gala must know).
    fn in_assembly_ns(&self, elem: &Elem) -> bool {
        let e = self.text(elem.ns);
        eq_ascii(e, ASM_V1) || eq_ascii(e, ASM_V2) || eq_ascii(e, ASM_V3)
    }

    fn attr_is(&self, a: &Attr, name: &[u8]) -> bool {
        eq_ascii(self.text(a.name), name)
    }
}

/// Parses "a.b.c.d" (each part below 65536, missing parts 0).
fn parse_version(s: &[u16]) -> Option<[u16; 4]> {
    let mut v = [0u32; 4];
    let mut part = 0;
    for &c in s {
        if (0x30..=0x39).contains(&c) {
            v[part] = v[part] * 10 + u32::from(c - 0x30);
            if v[part] >= 0x10000 {
                return None;
            }
        } else if c == u16::from(b'.') {
            part += 1;
            if part >= 4 {
                return None;
            }
        } else {
            return None;
        }
    }
    Some(v.map(|x| x as u16))
}

fn hex(c: u16) -> Option<u8> {
    match c {
        0x30..=0x39 => Some((c - 0x30) as u8),
        0x41..=0x46 => Some((c - 0x41 + 10) as u8),
        0x61..=0x66 => Some((c - 0x61 + 10) as u8),
        _ => None,
    }
}

/// A GUID in registry form, "{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}", as
/// its 16 bytes in memory (Data1-3 little-endian).
pub fn parse_guid(s: &[u16]) -> Option<Guid> {
    if s.len() != 38 || s[0] != u16::from(b'{') || s[37] != u16::from(b'}') {
        return None;
    }
    let s = &s[1..37];
    for &i in &[8usize, 13, 18, 23] {
        if s[i] != u16::from(b'-') {
            return None;
        }
    }
    let mut digits = [0u8; 32];
    let mut n = 0;
    for (i, &c) in s.iter().enumerate() {
        if [8usize, 13, 18, 23].contains(&i) {
            continue;
        }
        digits[n] = hex(c)?;
        n += 1;
    }
    let byte = |i: usize| digits[2 * i] << 4 | digits[2 * i + 1];
    let mut g = [0u8; 16];
    // Data1 (4 bytes), Data2, Data3 (2 each) are stored little-endian.
    for (k, &i) in [3usize, 2, 1, 0, 5, 4, 7, 6].iter().enumerate() {
        g[k] = byte(i);
    }
    for (i, b) in g.iter_mut().enumerate().skip(8) {
        *b = byte(i);
    }
    Some(g)
}

/// OLEMISC flag names (oleidl.h's values), for comClass's miscStatus*.
const OLEMISC: &[(&[u8], u32)] = &[
    (b"recomposeonresize", 0x1),
    (b"onlyiconic", 0x2),
    (b"insertnotreplace", 0x4),
    (b"static", 0x8),
    (b"cantlinkinside", 0x10),
    (b"canlinkbyole1", 0x20),
    (b"islinkobject", 0x40),
    (b"insideout", 0x80),
    (b"activatewhenvisible", 0x100),
    (b"renderingisdeviceindependent", 0x200),
    (b"invisibleatruntime", 0x400),
    (b"alwaysrun", 0x800),
    (b"actslikebutton", 0x1000),
    (b"actslikelabel", 0x2000),
    (b"nouiactivate", 0x4000),
    (b"alignable", 0x8000),
    (b"simpleframe", 0x10000),
    (b"setclientsitefirst", 0x20000),
    (b"imemode", 0x40000),
    (b"ignoreactivatewhenvisible", 0x80000),
    (b"wantstomenumerge", 0x100000),
    (b"supportsmultilevelundo", 0x200000),
];

fn parse_misc(s: &[u16]) -> u32 {
    s.split(|&c| c == u16::from(b','))
        .filter_map(|word| OLEMISC.iter().find(|(n, _)| eq_ascii(word, n)).map(|&(_, v)| v))
        .fold(0, |a, b| a | b)
}

/// comClass threadingModel: 1 Apartment, 2 Free, 3 none, 4 Both,
/// 5 Neutral.
fn parse_model(s: &[u16]) -> u32 {
    if eq_ascii(s, b"Apartment") {
        1
    } else if eq_ascii(s, b"Free") {
        2
    } else if eq_ascii(s, b"Both") {
        4
    } else if eq_ascii(s, b"Neutral") {
        5
    } else {
        3
    }
}

/// A manifest's text as UTF-16: with a byte-order mark (either order)
/// UTF-16, otherwise UTF-8 (its mark skipped).
pub fn decode(bytes: &[u8], out: &mut Buf<u16>) -> bool {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        let be = bytes[0] == 0xfe;
        if !out.reserve(bytes.len() / 2) {
            return false;
        }
        for pair in bytes[2..].chunks_exact(2) {
            let c = if be {
                u16::from_be_bytes([pair[0], pair[1]])
            } else {
                u16::from_le_bytes([pair[0], pair[1]])
            };
            out.push(c);
        }
        return true;
    }
    let b = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    if !out.reserve(b.len()) {
        return false;
    }
    for chunk in b.utf8_chunks() {
        for c in chunk.valid().encode_utf16() {
            out.push(c);
        }
        if !chunk.invalid().is_empty() {
            out.push(0xfffd);
        }
    }
    true
}

impl Model {
    pub fn new() -> Model {
        Model {
            pool: Buf::new(),
            assemblies: Buf::new(),
            files: Buf::new(),
            classes: Buf::new(),
            coms: Buf::new(),
            progids: Buf::new(),
            compat: Buf::new(),
            appdir: Str::default(),
            skipped: 0,
            deps: Buf::new(),
        }
    }

    pub fn str(&self, s: Str) -> &[u16] {
        &self.pool.as_slice()[s.at as usize..(s.at + s.len) as usize]
    }

    /// Keeps `s` in the pool (NUL-terminated there, for callers that hand
    /// out pointers).
    pub fn keep(&mut self, s: &[u16]) -> Option<Str> {
        let at = self.pool.len() as u32;
        (self.pool.push_all(s) && self.pool.push(0)).then_some(Str { at, len: s.len() as u32 })
    }

    fn keep_ascii(&mut self, s: &[u8]) -> Option<Str> {
        let at = self.pool.len() as u32;
        if !self.pool.reserve(s.len() + 1) {
            return None;
        }
        for &b in s {
            self.pool.push(u16::from(b));
        }
        self.pool.push(0);
        Some(Str { at, len: s.len() as u32 })
    }

    /// Parses manifest `text` as a new assembly (the root, or the
    /// dependency `expected`), whose manifest is at `path`.
    pub fn parse(
        &mut self,
        text: &[u16],
        path: &[u16],
        directory: &[u16],
        expected: Option<&Identity>,
    ) -> Res<()> {
        let no_memory = Refused { reason: b"no memory", at: 0 };
        let manifest = self.keep(path).ok_or(Refused { reason: b"no memory", at: 0 })?;
        let directory = self.keep(directory).ok_or(no_memory)?;
        let index = self.assemblies.len() as u32;
        let assembly = Assembly {
            manifest,
            directory,
            first_file: self.files.len() as u32,
            ..Default::default()
        };
        if !self.assemblies.push(assembly) {
            return Err(Refused { reason: b"no memory", at: 0 });
        }
        let mut r =
            Reader { s: text, pos: 0, ns: [(Span::default(), Span::default()); 32], ns_len: 0 };
        let top = Elem::default();
        // A leading byte-order mark (a UTF-8 file's, decoded).
        if r.s.first() == Some(&0xfeff) {
            r.pos = 1;
        }
        let mut root = r.next_elem(&top)?.ok_or(Refused { reason: b"no root element", at: 0 })?;
        if eq_ascii(r.text(root.name), b"?xml") {
            // The declaration: up to its "?>".
            let s = r.s;
            while r.pos + 1 < s.len()
                && !(s[r.pos] == u16::from(b'?') && s[r.pos + 1] == u16::from(b'>'))
            {
                r.pos += 1;
            }
            if r.pos + 1 >= s.len() {
                return r.refuse(b"bad XML declaration");
            }
            r.pos += 2;
            root = r.next_elem(&top)?.ok_or(Refused { reason: b"no root element", at: r.pos })?;
        }
        if !r.is(&root, b"assembly", ASM_V1) {
            return r.refuse(b"root is not asm.v1 <assembly>");
        }
        self.assembly_elem(&mut r, &root, index, expected)?;
        // Nothing but comments and blanks may follow.
        loop {
            r.skip_space();
            let rest = &r.s[r.pos..];
            if rest.is_empty() {
                break;
            }
            if rest.len() >= 4 && eq_ascii(&rest[..4], b"<!--") {
                let mut i = r.pos + 4;
                while i + 3 <= r.s.len() && !eq_ascii(&r.s[i..i + 3], b"-->") {
                    i += 1;
                }
                if i + 3 > r.s.len() {
                    return r.refuse(b"unterminated comment");
                }
                r.pos = i + 3;
                continue;
            }
            // A NUL terminator some resources carry.
            if rest.iter().all(|&c| c == 0) {
                break;
            }
            return r.refuse(b"content after the root element");
        }
        let files = self.files.len() as u32 - self.assemblies.as_slice()[index as usize].first_file;
        self.assemblies.as_mut_slice()[index as usize].files = files;
        Ok(())
    }

    fn assembly_elem(
        &mut self,
        r: &mut Reader<'_>,
        elem: &Elem,
        index: u32,
        expected: Option<&Identity>,
    ) -> Res<()> {
        let mut end = false;
        let mut version = false;
        while let Some(a) = r.next_attr(&mut end)? {
            if r.attr_is(&a, b"manifestVersion") {
                if !eq_ascii(r.text(a.value), b"1.0") {
                    return r.refuse(b"manifestVersion is not 1.0");
                }
                version = true;
            } else if !r.is_xmlns(&a) {
                return r.refuse(b"unknown attribute of <assembly>");
            }
        }
        if !version {
            return r.refuse(b"no manifestVersion");
        }
        if end {
            return Ok(());
        }
        let mut trust_seen = false;
        while let Some(child) = r.next_elem(elem)? {
            if r.is(&child, b"assemblyIdentity", ASM_V1) {
                let id = self.identity_elem(r, &child)?;
                if let Some(want) = expected
                    && id.version != want.version
                {
                    return r.refuse(b"dependency's version differs");
                }
                self.assemblies.as_mut_slice()[index as usize].id = id;
            } else if r.is(&child, b"dependency", ASM_V1) {
                self.dependency_elem(r, &child)?;
            } else if r.is(&child, b"file", ASM_V1) {
                self.file_elem(r, &child, index)?;
            } else if r.is(&child, b"trustInfo", ASM_V1) {
                self.trust_elem(r, &child, index, &mut trust_seen)?;
            } else if r.is(&child, b"compatibility", COMPAT_NS) {
                self.compat_elem(r, &child, index)?;
            } else if r.is(&child, b"comInterfaceExternalProxyStub", ASM_V1) {
                self.skipped |= SKIPPED_PROXY_STUB;
                r.skip(&child)?;
            } else if r.is(&child, b"clrClass", ASM_V1) || r.is(&child, b"clrSurrogate", ASM_V1) {
                self.skipped |= SKIPPED_CLR;
                r.skip(&child)?;
            } else if r.is(&child, b"description", ASM_V1)
                || r.is(&child, b"noInherit", ASM_V1)
                || r.is(&child, b"noInheritable", ASM_V1)
                || r.is(&child, b"application", ASM_V1)
            {
                r.skip(&child)?;
            } else if r.in_assembly_ns(&child) {
                return r.refuse(b"unknown element in <assembly>");
            } else {
                r.skip(&child)?;
            }
        }
        Ok(())
    }

    fn identity_elem(&mut self, r: &mut Reader<'_>, elem: &Elem) -> Res<Identity> {
        let mut id = Identity::default();
        let mut end = false;
        let oom = || Refused { reason: b"no memory", at: 0 };
        while let Some(a) = r.next_attr(&mut end)? {
            let v = r.text(a.value);
            if r.attr_is(&a, b"name") {
                id.name = self.keep(v).ok_or_else(oom)?;
            } else if r.attr_is(&a, b"type") {
                id.kind = self.keep(v).ok_or_else(oom)?;
            } else if r.attr_is(&a, b"version") {
                id.version = match parse_version(v) {
                    Some(v) => v,
                    None => return r.refuse(b"bad version"),
                };
            } else if r.attr_is(&a, b"processorArchitecture") {
                id.arch = self.keep(v).ok_or_else(oom)?;
            } else if r.attr_is(&a, b"publicKeyToken") {
                id.key = self.keep(v).ok_or_else(oom)?;
            } else if r.attr_is(&a, b"language") {
                id.lang = self.keep(v).ok_or_else(oom)?;
            }
        }
        if !end {
            while let Some(child) = r.next_elem(elem)? {
                r.skip(&child)?;
            }
        }
        Ok(id)
    }

    fn dependency_elem(&mut self, r: &mut Reader<'_>, elem: &Elem) -> Res<()> {
        let mut end = false;
        let mut optional = false;
        while let Some(a) = r.next_attr(&mut end)? {
            if r.attr_is(&a, b"optional") {
                optional = eq_ascii_ci(r.text(a.value), b"yes");
            }
        }
        if end {
            return Ok(());
        }
        while let Some(child) = r.next_elem(elem)? {
            if !r.is(&child, b"dependentAssembly", ASM_V1) {
                r.skip(&child)?;
                continue;
            }
            let mut end = false;
            let mut delayed = false;
            while let Some(a) = r.next_attr(&mut end)? {
                if r.attr_is(&a, b"allowDelayedBinding") {
                    delayed = eq_ascii(r.text(a.value), b"true");
                }
            }
            if end {
                continue;
            }
            while let Some(sub) = r.next_elem(&child)? {
                if !r.is(&sub, b"assemblyIdentity", ASM_V1) {
                    r.skip(&sub)?;
                    continue;
                }
                let mut id = self.identity_elem(r, &sub)?;
                if eq_ascii(self.str(id.arch), b"*") {
                    id.arch = self
                        .keep_ascii(CURRENT_ARCH)
                        .ok_or(Refused { reason: b"no memory", at: 0 })?;
                }
                if !self.deps.push(Dependency { id, optional: optional || delayed }) {
                    return Err(Refused { reason: b"no memory", at: 0 });
                }
            }
        }
        Ok(())
    }

    fn file_elem(&mut self, r: &mut Reader<'_>, elem: &Elem, index: u32) -> Res<()> {
        let mut end = false;
        let mut name = None;
        while let Some(a) = r.next_attr(&mut end)? {
            let v = r.text(a.value);
            if r.attr_is(&a, b"name") {
                name = Some(self.keep(v).ok_or(Refused { reason: b"no memory", at: 0 })?);
            } else if r.attr_is(&a, b"hash") {
                if v.len() != 40 || v.iter().any(|&c| hex(c).is_none()) {
                    return r.refuse(b"<file> hash is not 40 hex digits");
                }
            } else if r.attr_is(&a, b"loadFrom") {
                self.skipped |= SKIPPED_LOAD_FROM;
            }
        }
        let Some(name) = name else { return r.refuse(b"<file> without a name") };
        let file = self.files.len() as u32;
        if !self.files.push(File { name, assembly: index }) {
            return Err(Refused { reason: b"no memory", at: 0 });
        }
        if end {
            return Ok(());
        }
        while let Some(child) = r.next_elem(elem)? {
            if r.is(&child, b"windowClass", ASM_V1) {
                self.window_class_elem(r, &child, file)?;
            } else if r.is(&child, b"comClass", ASM_V1) {
                self.com_class_elem(r, &child, file)?;
            } else if r.is(&child, b"typelib", ASM_V1) {
                self.skipped |= SKIPPED_TYPELIB;
                r.skip(&child)?;
            } else if r.is(&child, b"comInterfaceProxyStub", ASM_V1) {
                self.skipped |= SKIPPED_PROXY_STUB;
                r.skip(&child)?;
            } else if r.is(&child, b"activatableClass", WINRT_NS) {
                self.skipped |= SKIPPED_WINRT;
                r.skip(&child)?;
            } else if r.is(&child, b"hash", ASM_V2) {
                r.skip(&child)?;
            } else if r.in_assembly_ns(&child) {
                return r.refuse(b"unknown element in <file>");
            } else {
                r.skip(&child)?;
            }
        }
        Ok(())
    }

    fn window_class_elem(&mut self, r: &mut Reader<'_>, elem: &Elem, file: u32) -> Res<()> {
        let mut end = false;
        let mut versioned = true;
        while let Some(a) = r.next_attr(&mut end)? {
            if r.attr_is(&a, b"versioned") {
                let v = r.text(a.value);
                if eq_ascii(v, b"no") {
                    versioned = false;
                } else if !eq_ascii(v, b"yes") {
                    return r.refuse(b"bad windowClass versioned");
                }
            }
        }
        if end {
            return r.refuse(b"empty windowClass");
        }
        let text = r.content()?;
        let name = self.keep(r.text(text)).ok_or(Refused { reason: b"no memory", at: 0 })?;
        if r.next_elem(elem)?.is_some() {
            return r.refuse(b"element inside windowClass");
        }
        if !self.classes.push(WindowClass { file, name, versioned }) {
            return Err(Refused { reason: b"no memory", at: 0 });
        }
        Ok(())
    }

    fn com_class_elem(&mut self, r: &mut Reader<'_>, elem: &Elem, file: u32) -> Res<()> {
        let mut end = false;
        let mut c = ComClass {
            file,
            clsid: [0; 16],
            tlbid: [0; 16],
            model: 3,
            progid: Str::default(),
            first_progid: self.progids.len() as u32,
            progids: 0,
            misc: [0; 5],
        };
        let mut clsid = false;
        while let Some(a) = r.next_attr(&mut end)? {
            let v = r.text(a.value);
            if r.attr_is(&a, b"clsid") {
                c.clsid = match parse_guid(v) {
                    Some(g) => g,
                    None => return r.refuse(b"bad comClass clsid"),
                };
                clsid = true;
            } else if r.attr_is(&a, b"tlbid") {
                c.tlbid = parse_guid(v).unwrap_or([0; 16]);
            } else if r.attr_is(&a, b"progid") {
                c.progid = self.keep(v).ok_or(Refused { reason: b"no memory", at: 0 })?;
            } else if r.attr_is(&a, b"threadingModel") {
                c.model = parse_model(v);
            } else if r.attr_is(&a, b"miscStatus") {
                c.misc[0] = parse_misc(v);
            } else if r.attr_is(&a, b"miscStatusContent") {
                c.misc[1] = parse_misc(v);
            } else if r.attr_is(&a, b"miscStatusThumbnail") {
                c.misc[2] = parse_misc(v);
            } else if r.attr_is(&a, b"miscStatusIcon") {
                c.misc[3] = parse_misc(v);
            } else if r.attr_is(&a, b"miscStatusDocPrint") {
                c.misc[4] = parse_misc(v);
            }
        }
        if !clsid {
            return r.refuse(b"comClass without a clsid");
        }
        if !end {
            while let Some(child) = r.next_elem(elem)? {
                if r.is(&child, b"progid", ASM_V1) {
                    if r.no_attrs()? {
                        return r.refuse(b"empty progid");
                    }
                    let text = r.content()?;
                    let p =
                        self.keep(r.text(text)).ok_or(Refused { reason: b"no memory", at: 0 })?;
                    if !self.progids.push(p) {
                        return Err(Refused { reason: b"no memory", at: 0 });
                    }
                    c.progids += 1;
                    if r.next_elem(&child)?.is_some() {
                        return r.refuse(b"element inside progid");
                    }
                } else {
                    r.skip(&child)?;
                }
            }
        }
        if !self.coms.push(c) {
            return Err(Refused { reason: b"no memory", at: 0 });
        }
        Ok(())
    }

    fn trust_elem(
        &mut self,
        r: &mut Reader<'_>,
        elem: &Elem,
        index: u32,
        seen: &mut bool,
    ) -> Res<()> {
        if r.no_attrs()? {
            return Ok(());
        }
        while let Some(security) = r.next_elem(elem)? {
            if !r.is(&security, b"security", ASM_V1) {
                r.skip(&security)?;
                continue;
            }
            if r.no_attrs()? {
                continue;
            }
            while let Some(privileges) = r.next_elem(&security)? {
                if !r.is(&privileges, b"requestedPrivileges", ASM_V1) {
                    r.skip(&privileges)?;
                    continue;
                }
                if r.no_attrs()? {
                    continue;
                }
                while let Some(level) = r.next_elem(&privileges)? {
                    if !r.is(&level, b"requestedExecutionLevel", ASM_V1) {
                        r.skip(&level)?;
                        continue;
                    }
                    if *seen {
                        return r.refuse(b"a second requestedExecutionLevel");
                    }
                    *seen = true;
                    let mut end = false;
                    let a = &mut self.assemblies.as_mut_slice()[index as usize];
                    while let Some(attr) = r.next_attr(&mut end)? {
                        let v = r.text(attr.value);
                        if r.attr_is(&attr, b"level") {
                            // ACTCTX_RUN_LEVEL_*.
                            if eq_ascii_ci(v, b"asInvoker") {
                                a.run_level = 1;
                            } else if eq_ascii_ci(v, b"highestAvailable") {
                                a.run_level = 2;
                            } else if eq_ascii_ci(v, b"requireAdministrator") {
                                a.run_level = 3;
                            }
                        } else if r.attr_is(&attr, b"uiAccess") {
                            if eq_ascii_ci(v, b"true") {
                                a.ui_access = 1;
                            } else if eq_ascii_ci(v, b"false") {
                                a.ui_access = 0;
                            }
                        }
                    }
                    if !end {
                        while let Some(x) = r.next_elem(&level)? {
                            r.skip(&x)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn compat_elem(&mut self, r: &mut Reader<'_>, elem: &Elem, index: u32) -> Res<()> {
        if r.no_attrs()? {
            return Ok(());
        }
        while let Some(app) = r.next_elem(elem)? {
            if !r.is(&app, b"application", COMPAT_NS) {
                r.skip(&app)?;
                continue;
            }
            if r.no_attrs()? {
                continue;
            }
            while let Some(item) = r.next_elem(&app)? {
                let os = r.is(&item, b"supportedOS", COMPAT_NS);
                let max = r.is(&item, b"maxversiontested", COMPAT_NS);
                if !os && !max {
                    r.skip(&item)?;
                    continue;
                }
                let mut end = false;
                let mut entry = None;
                while let Some(a) = r.next_attr(&mut end)? {
                    if !r.attr_is(&a, b"Id") {
                        continue;
                    }
                    let v = r.text(a.value);
                    entry = if os {
                        parse_guid(v).map(|id| Compat { id, kind: 1, max_version: 0 })
                    } else {
                        parse_version(v).map(|p| Compat {
                            id: [0; 16],
                            kind: 3,
                            max_version: p.iter().fold(0u64, |acc, &x| acc << 16 | u64::from(x)),
                        })
                    };
                }
                if !end {
                    while let Some(x) = r.next_elem(&item)? {
                        r.skip(&x)?;
                    }
                }
                // Only the root's list is reported.
                if index == 0
                    && let Some(e) = entry
                    && !self.compat.push(e)
                {
                    return Err(Refused { reason: b"no memory", at: 0 });
                }
            }
        }
        Ok(())
    }

    /// Whether assembly identity `have` satisfies the request `want`:
    /// names, architectures and keys alike (case aside), languages alike
    /// or either "*", the same major and minor version and no older build
    /// and revision.
    fn satisfies(&self, have: &Identity, want: &Identity) -> bool {
        let same = |a: Str, b: Str| eq_ci(self.str(a), self.str(b));
        if !same(have.name, want.name) || !same(have.arch, want.arch) || !same(have.key, want.key) {
            return false;
        }
        let (hl, wl) = (self.str(have.lang), self.str(want.lang));
        if !hl.is_empty()
            && !wl.is_empty()
            && !eq_ci(hl, wl)
            && !eq_ascii(hl, b"*")
            && !eq_ascii(wl, b"*")
        {
            return false;
        }
        let (h, w) = (have.version, want.version);
        h[0] == w[0] && h[1] == w[1] && (h[2], h[3]) >= (w[2], w[3])
    }

    /// Finds the dependencies (and theirs) in order: one already in the
    /// context, a system assembly (Microsoft's key: gala provides its
    /// DLLs itself), or a private assembly beside the application or
    /// the root manifest (`<dir>\<name>.dll` with a manifest resource,
    /// `<dir>\<name>.manifest`, then the same in `<dir>\<name>\`).
    pub fn resolve(&mut self, root_dir: &[u16]) -> Res<()> {
        let mut i = 0;
        while i < self.deps.len() {
            let dep = self.deps.as_slice()[i];
            i += 1;
            let loaded = (0..self.assemblies.len())
                .any(|a| self.satisfies(&self.assemblies.as_slice()[a].id, &dep.id));
            if loaded {
                continue;
            }
            if self.load_private(&dep.id, root_dir)? {
                continue;
            }
            if eq_ascii_ci(self.str(dep.id.key), MICROSOFT_KEY) {
                self.add_system(&dep.id)?;
                continue;
            }
            if dep.optional {
                continue;
            }
            super::note_missing_dependency(self.str(dep.id.name));
            return Err(Refused { reason: b"dependency not found", at: 0 });
        }
        Ok(())
    }

    /// A system assembly: its identity, no manifest file or files.
    fn add_system(&mut self, id: &Identity) -> Res<()> {
        let mut dir = Buf::<u16>::new();
        if !self.directory_name(id, &mut dir) {
            return Err(Refused { reason: b"no memory", at: 0 });
        }
        let directory = self.keep(dir.as_slice()).ok_or(Refused { reason: b"no memory", at: 0 })?;
        // Where Windows keeps such a manifest:
        // %SystemRoot%\WinSxS\Manifests\<directory>.manifest.
        let mut path = Buf::<u16>::new();
        let mut windows = [0u16; 260];
        let n = crate::files::GetWindowsDirectoryW(windows.as_mut_ptr(), 260) as usize;
        let ok = n > 0
            && n < 260
            && path.push_all(&windows[..n])
            && b"\\WinSxS\\Manifests\\".iter().all(|&b| path.push(u16::from(b)))
            && path.push_all(dir.as_slice())
            && b".manifest".iter().all(|&b| path.push(u16::from(b)));
        if !ok {
            return Err(Refused { reason: b"no memory", at: 0 });
        }
        let manifest = self.keep(path.as_slice()).ok_or(Refused { reason: b"no memory", at: 0 })?;
        let a = Assembly {
            id: *id,
            manifest,
            directory,
            first_file: self.files.len() as u32,
            ..Default::default()
        };
        if !self.assemblies.push(a) {
            return Err(Refused { reason: b"no memory", at: 0 });
        }
        Ok(())
    }

    /// Looks for a private assembly; true if found and parsed.
    fn load_private(&mut self, id: &Identity, root_dir: &[u16]) -> Res<bool> {
        let mut dirs = [Buf::<u16>::new(), Buf::<u16>::new()];
        let appdir = self.appdir;
        let name_s = id.name;
        let ok = dirs[0].push_all(self.str(appdir)) && dirs[1].push_all(root_dir);
        if !ok {
            return Err(Refused { reason: b"no memory", at: 0 });
        }
        let lang = self.str(id.lang);
        let lang_dir = !lang.is_empty() && !eq_ascii(lang, b"*") && !eq_ascii_ci(lang, b"neutral");
        for d in &dirs {
            for sub in [false, true] {
                let mut path = Buf::<u16>::new();
                let mut ok = path.push_all(d.as_slice());
                if sub {
                    ok = ok && path.push_all(self.str(name_s)) && path.push(u16::from(b'\\'));
                }
                if lang_dir {
                    ok = ok && path.push_all(self.str(id.lang)) && path.push(u16::from(b'\\'));
                }
                ok = ok && path.push_all(self.str(name_s));
                if !ok {
                    return Err(Refused { reason: b"no memory", at: 0 });
                }
                for ext in [&b".dll"[..], b".manifest"] {
                    let mut p = Buf::<u16>::new();
                    if !p.push_all(path.as_slice()) {
                        return Err(Refused { reason: b"no memory", at: 0 });
                    }
                    for &b in ext {
                        p.push(u16::from(b));
                    }
                    if !p.push(0) {
                        return Err(Refused { reason: b"no memory", at: 0 });
                    }
                    let full = &p.as_slice()[..p.len() - 1];
                    let mut bytes = Buf::<u8>::new();
                    let found = if ext == b".dll" {
                        super::read_pe_manifest(p.as_slice().as_ptr(), 1, &mut bytes)
                    } else {
                        super::read_file(p.as_slice().as_ptr(), &mut bytes)
                    };
                    if !found {
                        continue;
                    }
                    let mut text = Buf::<u16>::new();
                    if !decode(bytes.as_slice(), &mut text) {
                        return Err(Refused { reason: b"no memory", at: 0 });
                    }
                    let mut dir = Buf::<u16>::new();
                    if !self.directory_name(id, &mut dir) {
                        return Err(Refused { reason: b"no memory", at: 0 });
                    }
                    self.parse(text.as_slice(), full, dir.as_slice(), Some(id))?;
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// An assembly's directory name, in WinSxS's form:
    /// `<arch>_<name>_<key>_<version>_<language>_<hash>` (lower case,
    /// "none" for what the identity lacks; the hash is of the identity).
    fn directory_name(&self, id: &Identity, out: &mut Buf<u16>) -> bool {
        let mut hash: u32 = 0;
        let mut part = |out: &mut Buf<u16>, s: &[u16], first: bool| {
            let none = [u16::from(b'n'), u16::from(b'o'), u16::from(b'n'), u16::from(b'e')];
            let s = if s.is_empty() { &none[..] } else { s };
            let mut ok = first || out.push(u16::from(b'_'));
            for &c in s {
                let c = lower(c);
                hash = hash.wrapping_mul(65599).wrapping_add(u32::from(c));
                ok = ok && out.push(c);
            }
            ok
        };
        let mut version = Buf::<u16>::new();
        if !version_text(id.version, &mut version) {
            return false;
        }
        let ok = part(out, self.str(id.arch), true)
            && part(out, self.str(id.name), false)
            && part(out, self.str(id.key), false)
            && part(out, version.as_slice(), false)
            && part(out, self.str(id.lang), false)
            && out.push(u16::from(b'_'));
        if !ok {
            return false;
        }
        for i in (0..8).rev() {
            let d = (hash >> (4 * i)) & 0xf;
            let c =
                if d < 10 { u16::from(b'0') + d as u16 } else { u16::from(b'a') + d as u16 - 10 };
            if !out.push(c) {
                return false;
            }
        }
        true
    }

    /// The assembly's encoded identity:
    /// `name,processorArchitecture="..",publicKeyToken="..",type="..",version=".."`
    /// (the attributes it has, in that order).
    pub fn encoded_identity(&self, id: &Identity, out: &mut Buf<u16>) -> bool {
        let mut ok = out.push_all(self.str(id.name));
        let mut attr = |out: &mut Buf<u16>, name: &[u8], value: &[u16]| {
            if value.is_empty() {
                return;
            }
            ok = ok && out.push(u16::from(b','));
            for &b in name {
                ok = ok && out.push(u16::from(b));
            }
            ok = ok
                && out.push(u16::from(b'='))
                && out.push(u16::from(b'"'))
                && out.push_all(value)
                && out.push(u16::from(b'"'));
        };
        attr(out, b"processorArchitecture", self.str(id.arch));
        attr(out, b"publicKeyToken", self.str(id.key));
        attr(out, b"type", self.str(id.kind));
        let mut v = Buf::<u16>::new();
        let _ = version_text(id.version, &mut v);
        attr(out, b"version", v.as_slice());
        ok
    }
}

/// "a.b.c.d".
pub fn version_text(v: [u16; 4], out: &mut Buf<u16>) -> bool {
    let mut ok = true;
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            ok = ok && out.push(u16::from(b'.'));
        }
        let mut digits = [0u16; 5];
        let mut k = 0;
        let mut x = *x;
        loop {
            digits[k] = u16::from(b'0') + x % 10;
            k += 1;
            x /= 10;
            if x == 0 {
                break;
            }
        }
        for d in digits[..k].iter().rev() {
            ok = ok && out.push(*d);
        }
    }
    ok
}
