//! MSIX/AppX packages and bundles (ROADMAP PK stage 4; ARCHITECTURE
//! "Installing software"), read from memory: what `gala install` will
//! check and unpack. Nothing here installs.
//!
//! - [`Package::open`]: an .msix/.appx: the zip ([`crate::zip`], ZIP64),
//!   [Content_Types].xml, AppxBlockMap.xml, and AppxManifest.xml, the
//!   latter checked against its block map entry before it is parsed;
//!   AppxSignature.p7x's presence (signatures are not validated yet).
//! - [`Package::verify`]: every file against the block map (sizes,
//!   LfhSize, each 64 KiB block's hash, the deflate runs, CRC-32), no
//!   unlisted file, a content type for every part; every problem found
//!   is reported, each naming the file and block.
//! - [`Bundle::open`], [`Bundle::verify`], [`Bundle::open_package`]: an
//!   .msixbundle/.appxbundle: AppxMetadata\AppxBundleManifest.xml, its
//!   packages at their Offset and Size, an inner package opened in place
//!   after the bundle's block map has checked it.
//! - [`Identity`]: package full and family names, the publisher id
//!   ([`publisher_id`]).
//!
//! Sources: Microsoft's App package manifest, bundle manifest and
//! package block map schema references, "An overview of Package
//! Identity in Windows apps", ECMA-376 Part 2 (OPC), PKWARE's
//! APPNOTE.TXT. No other implementation was read.

use crate::appxmanifest::{self, AppxManifest, BundleManifest, BundlePackage, ContentTypes};
use crate::blockmap::{self, BlockMap, Checked};
use crate::xml::{self, Element};
use crate::zip::{self, Archive};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// The signature's file name (presence only, for now).
pub const SIGNATURE: &str = "AppxSignature.p7x";

/// The Publisher suffix that marks an unsigned package ("Create an
/// unsigned MSIX package"; it must be the last field).
pub const UNSIGNED_MARKER: &str = "OID.2.25.311729368913984317654407730594956997722=1";

/// Why a package could not be read or did not check out. Every variant
/// names the file (a bundle's inner package as `<package>/<file>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The zip container (or one entry of it).
    Zip { file: String, err: zip::Error },
    /// A document that is not well-formed XML.
    Xml { file: String, err: xml::Error },
    /// A document whose content is wrong: the element and its line.
    Manifest { file: String, element: String, line: u32, msg: String },
    /// A file every package (or bundle) has is not there.
    Required { file: String },
    /// Listed in the block map, not in the package.
    Missing { file: String, listed_line: u32 },
    /// In the package, not in the block map.
    Unlisted { file: String },
    /// A part with no content type in [Content_Types].xml.
    NoContentType { file: String },
    /// The block map and the package disagree about a size or count.
    Mismatch { file: String, what: String, expected: String, actual: String },
    /// A block's data does not have the hash the block map gives.
    Hash { file: String, block: usize, expected: String, actual: String },
    /// A block that cannot be read as the block map describes it.
    Block { file: String, block: usize, msg: String },
    /// A bundle's package entry or identity disagrees with the bundle
    /// manifest.
    Bundle { package: String, msg: String },
    /// A file too large to hold in memory.
    OutOfMemory { file: String, bytes: usize },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Zip { file, err } if file.is_empty() => write!(f, "{err}"),
            Error::Zip { file, err } => write!(f, "{file}: {err}"),
            Error::Xml { file, err } => write!(f, "{file} {err}"),
            Error::Manifest { file, element, line, msg } => {
                write!(f, "{file} line {line}: <{element}>: {msg}")
            }
            Error::Required { file } => write!(f, "{file} is missing (every package has one)"),
            Error::Missing { file, listed_line } => write!(
                f,
                "{file}: listed in the block map (line {listed_line}) but not in the package"
            ),
            Error::Unlisted { file } => {
                write!(f, "{file}: in the package but not in the block map")
            }
            Error::NoContentType { file } => {
                write!(f, "{file}: no content type in [Content_Types].xml")
            }
            Error::Mismatch { file, what, expected, actual } => {
                write!(f, "{file}: {what}: the block map says {expected}, the package has {actual}")
            }
            Error::Hash { file, block, expected, actual } => write!(
                f,
                "{file} block {block}: hash mismatch: the block map says {expected}, \
                 the data hashes to {actual}"
            ),
            Error::Block { file, block, msg } => write!(f, "{file} block {block}: {msg}"),
            Error::Bundle { package, msg } => write!(f, "bundle package {package}: {msg}"),
            Error::OutOfMemory { file, bytes } => {
                write!(f, "{file}: no memory for its {bytes} bytes")
            }
        }
    }
}

/// Everything [`Package::verify`] or [`Bundle::verify`] found wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problems(pub Vec<Error>);

impl fmt::Display for Problems {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} problem(s):", self.0.len())?;
        for e in &self.0 {
            write!(f, "\n  {e}")?;
        }
        Ok(())
    }
}

/// A four-part version, Major.Minor.Build.Revision, each 0..=65535.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub [u16; 4]);

impl Version {
    /// "1.2.3.4" (exactly four parts, as the schemas require).
    pub fn parse(s: &str) -> Option<Version> {
        let mut v = [0u16; 4];
        let mut parts = s.split('.');
        for slot in &mut v {
            let p = parts.next()?;
            if p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            *slot = p.parse().ok()?;
        }
        parts.next().is_none().then_some(Version(v))
    }

    pub(crate) fn attr(file: &str, el: &Element, attr: &str) -> Result<Version, Error> {
        let v = el.attr(attr).ok_or_else(|| Error::Manifest {
            file: file.into(),
            element: el.qname.clone(),
            line: el.line,
            msg: format!("no {attr} attribute"),
        })?;
        Version::parse(v).ok_or_else(|| Error::Manifest {
            file: file.into(),
            element: el.qname.clone(),
            line: el.line,
            msg: format!("{attr}=\"{v}\" is not a version Major.Minor.Build.Revision (0-65535)"),
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d] = self.0;
        write!(f, "{a}.{b}.{c}.{d}")
    }
}

/// The architectures the Identity element allows.
pub const ARCHITECTURES: [&str; 6] = ["x86", "x64", "arm", "arm64", "x86a64", "neutral"];

/// A package string ("An overview of Package Identity"): letters, digits,
/// '.' and '-', not a reserved device name, not starting with one plus
/// '.', not starting with "xn--", not ending with '.', no ".xn--".
pub fn is_package_string(s: &str, min: usize, max: usize) -> bool {
    const RESERVED: [&str; 22] = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    let lower = s.to_ascii_lowercase();
    (min..=max).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && lower != "."
        && lower != ".."
        && !RESERVED.iter().any(|r| lower == *r || lower.starts_with(&format!("{r}.")))
        && !lower.starts_with("xn--")
        && !lower.ends_with('.')
        && !lower.contains(".xn--")
}

/// A package's (or bundle's) identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub publisher: String,
    pub version: Version,
    /// Lower case, one of [`ARCHITECTURES`] (`neutral` when not given).
    pub architecture: String,
    pub resource_id: Option<String>,
    /// A bundle's identity: its resource id is `~` in the full name.
    pub bundle: bool,
}

impl Identity {
    /// From an Identity element; `package` for a package manifest's
    /// (ProcessorArchitecture, ResourceId), else a bundle's.
    pub(crate) fn from_element(file: &str, el: &Element, package: bool) -> Result<Identity, Error> {
        let bad = |msg: String| Error::Manifest {
            file: file.into(),
            element: el.qname.clone(),
            line: el.line,
            msg,
        };
        let get = |a: &str| el.attr(a).ok_or_else(|| bad(format!("no {a} attribute")));
        let name = get("Name")?;
        if !is_package_string(name, 3, 50) {
            return Err(bad(format!(
                "Name=\"{name}\" is not a package string of 3 to 50 characters \
                 (letters, digits, '.', '-')"
            )));
        }
        let publisher = get("Publisher")?;
        if publisher.is_empty() || publisher.chars().count() > 8192 {
            return Err(bad("Publisher must be 1 to 8192 characters".into()));
        }
        let version = Version::attr(file, el, "Version")?;
        let mut architecture = String::from("neutral");
        let mut resource_id = None;
        if package {
            if let Some(a) = el.attr("ProcessorArchitecture") {
                let lower = a.to_ascii_lowercase();
                if !ARCHITECTURES.contains(&lower.as_str()) {
                    return Err(bad(format!(
                        "ProcessorArchitecture=\"{a}\" is not one of {}",
                        ARCHITECTURES.join(", ")
                    )));
                }
                architecture = lower;
            }
            if let Some(r) = el.attr("ResourceId").filter(|r| !r.is_empty()) {
                if !is_package_string(r, 1, 30) {
                    return Err(bad(format!(
                        "ResourceId=\"{r}\" is not a package string of up to 30"
                    )));
                }
                resource_id = Some(r.into());
            }
        }
        Ok(Identity {
            name: name.into(),
            publisher: publisher.into(),
            version,
            architecture,
            resource_id,
            bundle: !package,
        })
    }

    pub fn publisher_id(&self) -> String {
        publisher_id(&self.publisher)
    }

    /// `<Name>_<PublisherId>`.
    pub fn family_name(&self) -> String {
        format!("{}_{}", self.name, self.publisher_id())
    }

    /// `<Name>_<Version>_<Architecture>_<ResourceId>_<PublisherId>` (a
    /// bundle's resource id is `~`).
    pub fn full_name(&self) -> String {
        let res = if self.bundle { "~" } else { self.resource_id.as_deref().unwrap_or("") };
        format!(
            "{}_{}_{}_{}_{}",
            self.name,
            self.version,
            self.architecture,
            res,
            self.publisher_id()
        )
    }

    /// Whether the Publisher ends with the unsigned-package marker.
    pub fn unsigned_publisher(&self) -> bool {
        self.publisher.trim_end().ends_with(UNSIGNED_MARKER)
    }
}

/// The publisher id of a Publisher string: the SHA-256 of the string in
/// UTF-16LE, its first 8 bytes (64 bits, most significant first) padded
/// with a zero bit to 65 and written as 13 base32 digits of Crockford's
/// alphabet in lower case. Checked against Microsoft's documented pair
/// (the Windows App SDK runtime's `Get-AppxPackage` output: Microsoft's
/// Publisher and `8wekyb3d8bbwe`).
pub fn publisher_id(publisher: &str) -> String {
    use sha2::Digest;
    const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let mut h = sha2::Sha256::new();
    for u in publisher.encode_utf16() {
        h.update(u.to_le_bytes());
    }
    let digest = h.finalize();
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest[..8]);
    let bits = u128::from(u64::from_be_bytes(first)) << 1;
    (0..13).map(|i| char::from(ALPHABET[(bits >> (5 * (12 - i)) & 31) as usize])).collect()
}

/// A file the block map lists, as an installer would place it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageFile {
    /// The block map's name (`\` separators).
    pub name: String,
    /// A relative Windows path that stays inside the install directory
    /// ([`zip::safe_path`]); None for a name that would leave it.
    pub path: Option<String>,
    pub size: u64,
    /// AppxManifest.xml and AppxMetadata\*: footprint, not payload.
    pub footprint: bool,
}

fn read_required(ctx: &str, a: &Archive<'_>, name: &str) -> Result<Vec<u8>, Error> {
    let file = format!("{ctx}{name}");
    let e = a.find(name).ok_or_else(|| Error::Required { file: file.clone() })?;
    a.read(e).map_err(|err| Error::Zip { file, err })
}

/// Reads one block-map file, checked.
fn read_listed(ctx: &str, a: &Archive<'_>, map: &BlockMap, name: &str) -> Result<Vec<u8>, Error> {
    let key = blockmap::file_key(name);
    let f = map
        .files
        .iter()
        .find(|f| blockmap::file_key(&f.name.replace('\\', "/")) == key)
        .ok_or_else(|| Error::Unlisted { file: format!("{ctx}{name}") })?;
    let e = blockmap::index(a)
        .get(&key)
        .map(|&i| &a.entries[i])
        .ok_or_else(|| Error::Missing { file: format!("{ctx}{}", f.name), listed_line: f.line })?;
    blockmap::check_file(ctx, a, e, map.hash_method, f)
}

/// The parts with no content type.
fn content_type_problems(ctx: &str, a: &Archive<'_>, ct: &ContentTypes) -> Vec<Error> {
    a.entries
        .iter()
        .filter(|e| !e.is_dir() && !e.name.eq_ignore_ascii_case(appxmanifest::CONTENT_TYPES))
        .filter(|e| ct.lookup(&e.name).is_none())
        .map(|e| Error::NoContentType { file: format!("{ctx}{}", e.name) })
        .collect()
}

/// The footprint files common to packages and bundles.
fn footprint(ctx: &str, a: &Archive<'_>) -> Result<(ContentTypes, BlockMap, Option<u32>), Error> {
    let ct_name = format!("{ctx}{}", appxmanifest::CONTENT_TYPES);
    let ct = ContentTypes::parse(&ct_name, &read_required(ctx, a, appxmanifest::CONTENT_TYPES)?)?;
    let bm_name = format!("{ctx}{}", blockmap::FILE);
    let bm = BlockMap::parse(&bm_name, &read_required(ctx, a, blockmap::FILE)?)?;
    let sig = a.find(SIGNATURE).map(|e| e.size);
    Ok((ct, bm, sig))
}

/// An .msix/.appx package.
pub struct Package<'a> {
    pub archive: Archive<'a>,
    pub manifest: AppxManifest,
    pub block_map: BlockMap,
    pub content_types: ContentTypes,
    /// AppxSignature.p7x's size when present (not validated).
    pub signature: Option<u32>,
    /// "" or "<inner package file name>/" for messages.
    ctx: String,
}

impl<'a> Package<'a> {
    /// Opens a package: the footprint files read and parsed, the manifest
    /// checked against the block map. Call [`Package::verify`] before
    /// trusting any other file.
    pub fn open(data: &'a [u8]) -> Result<Package<'a>, Error> {
        Package::open_in(data, String::new())
    }

    fn open_in(data: &'a [u8], ctx: String) -> Result<Package<'a>, Error> {
        let archive = Archive::open(data)
            .map_err(|err| Error::Zip { file: ctx.trim_end_matches('/').into(), err })?;
        if archive.find(appxmanifest::MANIFEST).is_none()
            && archive.find(appxmanifest::BUNDLE_MANIFEST).is_some()
        {
            return Err(Error::Required {
                file: format!("{ctx}{} (this is a bundle)", appxmanifest::MANIFEST),
            });
        }
        let (content_types, block_map, signature) = footprint(&ctx, &archive)?;
        if archive.find(appxmanifest::MANIFEST).is_none() {
            return Err(Error::Required { file: format!("{ctx}{}", appxmanifest::MANIFEST) });
        }
        let bytes = read_listed(&ctx, &archive, &block_map, appxmanifest::MANIFEST)?;
        let manifest = AppxManifest::parse(&format!("{ctx}{}", appxmanifest::MANIFEST), &bytes)?;
        Ok(Package { archive, manifest, block_map, content_types, signature, ctx })
    }

    pub fn identity(&self) -> &Identity {
        &self.manifest.identity
    }

    /// Every file against the block map and every part's content type;
    /// all problems are returned.
    pub fn verify(&self) -> Result<Checked, Problems> {
        let mut problems = Vec::new();
        let checked = match blockmap::check_archive(&self.ctx, &self.archive, &self.block_map) {
            Ok(c) => c,
            Err(p) => {
                problems = p;
                Checked::default()
            }
        };
        problems.extend(content_type_problems(&self.ctx, &self.archive, &self.content_types));
        if problems.is_empty() { Ok(checked) } else { Err(Problems(problems)) }
    }

    /// The files of the block map.
    pub fn files(&self) -> Vec<PackageFile> {
        self.block_map
            .files
            .iter()
            .map(|f| {
                let lower = f.name.to_ascii_lowercase();
                PackageFile {
                    name: f.name.clone(),
                    path: zip::safe_path(&f.name),
                    size: f.size,
                    footprint: lower == "appxmanifest.xml" || lower.starts_with("appxmetadata\\"),
                }
            })
            .collect()
    }

    /// A block-map file's contents (by its block map name or zip name),
    /// checked block by block.
    pub fn read(&self, name: &str) -> Result<Vec<u8>, Error> {
        read_listed(&self.ctx, &self.archive, &self.block_map, &name.replace('\\', "/"))
    }
}

/// An .msixbundle/.appxbundle.
pub struct Bundle<'a> {
    data: &'a [u8],
    pub archive: Archive<'a>,
    pub manifest: BundleManifest,
    pub block_map: BlockMap,
    pub content_types: ContentTypes,
    pub signature: Option<u32>,
}

impl<'a> Bundle<'a> {
    /// Opens a bundle: footprint files, and the bundle manifest checked
    /// against the block map.
    pub fn open(data: &'a [u8]) -> Result<Bundle<'a>, Error> {
        let archive = Archive::open(data).map_err(|err| Error::Zip { file: String::new(), err })?;
        let (content_types, block_map, signature) = footprint("", &archive)?;
        if archive.find(appxmanifest::BUNDLE_MANIFEST).is_none() {
            return Err(Error::Required { file: appxmanifest::BUNDLE_MANIFEST.replace('/', "\\") });
        }
        let bytes = read_listed("", &archive, &block_map, appxmanifest::BUNDLE_MANIFEST)?;
        let manifest =
            BundleManifest::parse(&appxmanifest::BUNDLE_MANIFEST.replace('/', "\\"), &bytes)?;
        Ok(Bundle { data, archive, manifest, block_map, content_types, signature })
    }

    pub fn identity(&self) -> &Identity {
        &self.manifest.identity
    }

    /// The zip entry holding `p`, stored, at the manifest's Offset and
    /// Size.
    fn entry_of(&self, p: &BundlePackage) -> Result<&zip::Entry, Error> {
        let bundle_err = |msg: String| Error::Bundle { package: p.file_name.clone(), msg };
        let e = self
            .archive
            .find(&p.file_name)
            .ok_or_else(|| bundle_err(format!("not in the bundle (manifest line {})", p.line)))?;
        if e.method != 0 {
            return Err(bundle_err(format!("compressed (method {}), not stored", e.method)));
        }
        let at = self
            .archive
            .data_offset(e)
            .map_err(|err| Error::Zip { file: p.file_name.clone(), err })?;
        if at as u64 != p.offset || u64::from(e.compressed_size) != p.size {
            return Err(bundle_err(format!(
                "the manifest (line {}) says Offset {} Size {}, the zip entry's data is at {} \
                 and {} bytes",
                p.line, p.offset, p.size, at, e.compressed_size
            )));
        }
        Ok(e)
    }

    /// The block map and content types, and every package's entry.
    pub fn verify(&self) -> Result<Checked, Problems> {
        let mut problems = Vec::new();
        let checked = match blockmap::check_archive("", &self.archive, &self.block_map) {
            Ok(c) => c,
            Err(p) => {
                problems = p;
                Checked::default()
            }
        };
        problems.extend(content_type_problems("", &self.archive, &self.content_types));
        for p in &self.manifest.packages {
            if let Err(e) = self.entry_of(p) {
                problems.push(e);
            }
        }
        if problems.is_empty() { Ok(checked) } else { Err(Problems(problems)) }
    }

    /// The application package for `architecture` (x64 before neutral
    /// when asked for x64, as an installer would choose).
    pub fn package_for(&self, architecture: &str) -> Option<&BundlePackage> {
        let apps = || self.manifest.packages.iter().filter(|p| p.kind == "application");
        apps()
            .find(|p| p.architecture.eq_ignore_ascii_case(architecture))
            .or_else(|| apps().find(|p| p.architecture.eq_ignore_ascii_case("neutral")))
    }

    /// Opens an inner package in place, after the bundle's block map has
    /// checked its bytes; its identity must agree with the bundle's
    /// (Name, Publisher) and with its Package element (Version,
    /// Architecture, ResourceId).
    pub fn open_package(&self, p: &BundlePackage) -> Result<Package<'a>, Error> {
        let e = self.entry_of(p)?;
        read_listed("", &self.archive, &self.block_map, &e.name)?;
        let bytes = usize::try_from(p.offset)
            .ok()
            .and_then(|start| self.data.get(start..start.checked_add(p.size as usize)?))
            .ok_or_else(|| Error::Bundle {
                package: p.file_name.clone(),
                msg: format!("Offset {} Size {} beyond the bundle", p.offset, p.size),
            })?;
        let pkg = Package::open_in(bytes, format!("{}/", p.file_name))?;
        let id = pkg.identity();
        let bundle = self.identity();
        let mut wrong = Vec::new();
        if !id.name.eq_ignore_ascii_case(&bundle.name) {
            wrong.push(format!("Name {} (the bundle's: {})", id.name, bundle.name));
        }
        if id.publisher != bundle.publisher {
            wrong.push(format!("Publisher {} (the bundle's: {})", id.publisher, bundle.publisher));
        }
        if id.version != p.version {
            wrong.push(format!("Version {} (the bundle manifest's: {})", id.version, p.version));
        }
        if !id.architecture.eq_ignore_ascii_case(&p.architecture) {
            wrong.push(format!(
                "ProcessorArchitecture {} (the bundle manifest's: {})",
                id.architecture, p.architecture
            ));
        }
        if id.resource_id.as_deref().unwrap_or("") != p.resource_id.as_deref().unwrap_or("") {
            wrong.push(format!(
                "ResourceId {:?} (the bundle manifest's: {:?})",
                id.resource_id, p.resource_id
            ));
        }
        if !wrong.is_empty() {
            return Err(Error::Bundle {
                package: p.file_name.clone(),
                msg: format!("its identity disagrees: {}", wrong.join("; ")),
            });
        }
        Ok(pkg)
    }
}

/// A package or a bundle (made once per install: the size difference
/// of the variants does not matter).
#[allow(clippy::large_enum_variant)]
pub enum Opened<'a> {
    Package(Package<'a>),
    Bundle(Bundle<'a>),
}

/// Opens whichever `data` is: a bundle when it has
/// AppxMetadata\AppxBundleManifest.xml and no AppxManifest.xml.
pub fn open(data: &[u8]) -> Result<Opened<'_>, Error> {
    let a = Archive::open(data).map_err(|err| Error::Zip { file: String::new(), err })?;
    if a.find(appxmanifest::MANIFEST).is_none() && a.find(appxmanifest::BUNDLE_MANIFEST).is_some() {
        Bundle::open(data).map(Opened::Bundle)
    } else {
        Package::open(data).map(Opened::Package)
    }
}

impl Checked {
    /// "N files, M blocks, B bytes" (for the install log).
    pub fn summary(&self) -> String {
        format!("{} files, {} blocks, {} bytes", self.files, self.blocks, self.bytes).to_string()
    }
}

#[cfg(test)]
mod tests;
