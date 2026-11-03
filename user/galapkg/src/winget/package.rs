//! A package version from its manifests: the singleton form (one file)
//! or the multi-file form (one `version`, one `installer`, one
//! `defaultLocale` and any `locale` manifests), any schema 1.0.0 to
//! 1.28.0 (`schema`), into one [`Package`] with every installer's fields
//! resolved.
//!
//! Rules (winget-cli `doc/ManifestSpecv1.*.md` and the JSON schemas,
//! learn.microsoft.com "Create your package manifest"):
//! - every file names the same PackageIdentifier, PackageVersion and
//!   ManifestVersion (the multi-file form is one manifest split up);
//! - the version manifest's DefaultLocale names the defaultLocale
//!   manifest's PackageLocale; locale manifests carry other locales'
//!   texts, and a field a locale leaves out is the default locale's
//!   ([`Package::localization`]);
//! - installer fields at the root are every installer's default; a field
//!   an installer sets replaces the root's (schema: "if present at both
//!   levels the installer level is used"). For `InstallerSwitches` the
//!   documentation is silent on whether this is per switch: gala merges
//!   per switch (an installer's `Custom` does not drop the root's
//!   `Silent`), the rule that loses no switch; every other field,
//!   `Dependencies` and `AppsAndFeaturesEntries` included, is replaced
//!   whole;
//! - an installer needs Architecture, InstallerType (at either level),
//!   InstallerUrl and InstallerSha256 (64 hex digits).
//!
//! What fails is an [`Error`] naming the file, line and column; what is
//! merely suspect (an unknown field, a field or value newer than the
//! declared schema, a key in the wrong case) is a [`Warning`] kept in
//! [`Package::warnings`], so `gala install --verbose` can show why a
//! manifest reads oddly (OBSERVABILITY: what explains a failure).

use super::schema::{self, EnumMatch, EnumValues, Level, ManifestType, SchemaVersion};
use super::yaml::{self, Node, Pos, Value};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// Why the manifests cannot be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// The file's name as the caller gave it (empty: the set as a whole).
    pub file: String,
    pub at: Option<Pos>,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.file.is_empty(), &self.at) {
            (false, Some(at)) => write!(f, "{}:{}: {}", self.file, at, self.message),
            (false, None) => write!(f, "{}: {}", self.file, self.message),
            (true, _) => f.write_str(&self.message),
        }
    }
}

/// Something suspect that did not stop the manifests from reading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    pub file: String,
    pub at: Pos,
    pub message: String,
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.file, self.at, self.message)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Architecture {
    X86,
    X64,
    Arm,
    Arm64,
    Neutral,
}

impl Architecture {
    pub fn parse(s: &str) -> Option<Architecture> {
        Some(match s.to_ascii_lowercase().as_str() {
            "x86" => Architecture::X86,
            "x64" => Architecture::X64,
            "arm" => Architecture::Arm,
            "arm64" => Architecture::Arm64,
            "neutral" => Architecture::Neutral,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Architecture::X86 => "x86",
            Architecture::X64 => "x64",
            Architecture::Arm => "arm",
            Architecture::Arm64 => "arm64",
            Architecture::Neutral => "neutral",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InstallerType {
    Msix,
    Msi,
    Appx,
    Exe,
    Inno,
    Nullsoft,
    Wix,
    Burn,
    Pwa,
    Portable,
    Zip,
    Font,
}

impl InstallerType {
    pub fn parse(s: &str) -> Option<InstallerType> {
        use InstallerType::*;
        Some(match s.to_ascii_lowercase().as_str() {
            "msix" => Msix,
            "msi" => Msi,
            "appx" => Appx,
            "exe" => Exe,
            "inno" => Inno,
            "nullsoft" => Nullsoft,
            "wix" => Wix,
            "burn" => Burn,
            "pwa" => Pwa,
            "portable" => Portable,
            "zip" => Zip,
            "font" => Font,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        use InstallerType::*;
        match self {
            Msix => "msix",
            Msi => "msi",
            Appx => "appx",
            Exe => "exe",
            Inno => "inno",
            Nullsoft => "nullsoft",
            Wix => "wix",
            Burn => "burn",
            Pwa => "pwa",
            Portable => "portable",
            Zip => "zip",
            Font => "font",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    User,
    Machine,
}

impl Scope {
    pub fn parse(s: &str) -> Option<Scope> {
        match s.to_ascii_lowercase().as_str() {
            "user" => Some(Scope::User),
            "machine" => Some(Scope::Machine),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::Machine => "machine",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpgradeBehavior {
    Install,
    UninstallPrevious,
    Deny,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elevation {
    Required,
    Prohibited,
    ElevatesSelf,
}

/// A file inside an archive that is the real installer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NestedFile {
    pub relative_path: String,
    /// PortableCommandAlias (portable nested installers only).
    pub alias: Option<String>,
}

/// InstallerSwitches as written (None: not given; the defaults per
/// installer type are `select::switches`').
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Switches {
    pub silent: Option<String>,
    pub silent_with_progress: Option<String>,
    pub interactive: Option<String>,
    pub install_location: Option<String>,
    pub log: Option<String>,
    pub upgrade: Option<String>,
    pub custom: Option<String>,
    /// 1.7.0 and later.
    pub repair: Option<String>,
}

impl Switches {
    fn slot(&mut self, key: &str) -> Option<&mut Option<String>> {
        Some(match key {
            "Silent" => &mut self.silent,
            "SilentWithProgress" => &mut self.silent_with_progress,
            "Interactive" => &mut self.interactive,
            "InstallLocation" => &mut self.install_location,
            "Log" => &mut self.log,
            "Upgrade" => &mut self.upgrade,
            "Custom" => &mut self.custom,
            "Repair" => &mut self.repair,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageDependency {
    pub id: String,
    pub minimum_version: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dependencies {
    pub windows_features: Vec<String>,
    pub windows_libraries: Vec<String>,
    pub packages: Vec<PackageDependency>,
    pub external: Vec<String>,
}

impl Dependencies {
    pub fn is_empty(&self) -> bool {
        self.windows_features.is_empty()
            && self.windows_libraries.is_empty()
            && self.packages.is_empty()
            && self.external.is_empty()
    }
}

/// An AppsAndFeaturesEntries entry: what the installer writes into its
/// Add/Remove Programs (ARP) entry, for detecting it afterwards.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArpEntry {
    pub display_name: Option<String>,
    pub publisher: Option<String>,
    pub display_version: Option<String>,
    pub product_code: Option<String>,
    pub upgrade_code: Option<String>,
    pub installer_type: Option<InstallerType>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedReturnCode {
    pub code: i64,
    /// ReturnResponse (`packageInUse`, `rebootRequiredToFinish`...).
    pub response: String,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Markets {
    pub allowed: Vec<String>,
    pub excluded: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstalledFile {
    pub relative_path: String,
    pub sha256: Option<String>,
    /// launch, uninstall or other.
    pub file_type: Option<String>,
    pub invocation_parameter: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstallationMetadata {
    pub default_install_location: Option<String>,
    pub files: Vec<InstalledFile>,
}

/// One installer with the root's defaults applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installer {
    /// 0-based position in `Installers`, and where its entry starts.
    pub index: usize,
    pub at: Pos,
    pub architecture: Architecture,
    /// None: a type no known schema has (the raw text is in a warning);
    /// such an installer is never selected.
    pub installer_type: Option<InstallerType>,
    pub nested_installer_type: Option<InstallerType>,
    pub nested_installer_files: Vec<NestedFile>,
    pub url: String,
    /// Upper-case hex.
    pub sha256: String,
    pub signature_sha256: Option<String>,
    /// InstallerLocale (BCP 47).
    pub locale: Option<String>,
    pub platform: Vec<String>,
    pub minimum_os_version: Option<String>,
    pub scope: Option<Scope>,
    pub install_modes: Vec<String>,
    pub switches: Switches,
    pub success_codes: Vec<i64>,
    pub expected_return_codes: Vec<ExpectedReturnCode>,
    pub upgrade_behavior: Option<UpgradeBehavior>,
    pub commands: Vec<String>,
    pub protocols: Vec<String>,
    pub file_extensions: Vec<String>,
    pub dependencies: Dependencies,
    pub package_family_name: Option<String>,
    pub product_code: Option<String>,
    pub capabilities: Vec<String>,
    pub restricted_capabilities: Vec<String>,
    pub markets: Markets,
    pub aborts_terminal: bool,
    pub release_date: Option<String>,
    pub install_location_required: bool,
    pub require_explicit_upgrade: bool,
    pub display_install_warnings: bool,
    pub unsupported_os_architectures: Vec<Architecture>,
    /// `log`, `location`.
    pub unsupported_arguments: Vec<String>,
    pub apps_and_features: Vec<ArpEntry>,
    pub elevation: Option<Elevation>,
    pub installation_metadata: Option<InstallationMetadata>,
    pub download_command_prohibited: bool,
    /// modify, uninstaller, installer.
    pub repair_behavior: Option<String>,
    pub archive_binaries_depend_on_path: bool,
    /// Authentication's AuthenticationType (`none`, `microsoftEntraId`...).
    pub authentication: Option<String>,
}

impl Installer {
    /// The type that is really installed: an archive's
    /// NestedInstallerType (winget installs the nested installer).
    pub fn effective_type(&self) -> Option<InstallerType> {
        match self.installer_type {
            Some(InstallerType::Zip) => self.nested_installer_type,
            t => t,
        }
    }

    /// The installer file's name: the URL's last part without query or
    /// fragment.
    pub fn file_name(&self) -> &str {
        let url = self.url.split(['?', '#']).next().unwrap_or("");
        url.rsplit(['/', '\\']).next().unwrap_or(url)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agreement {
    pub label: Option<String>,
    pub text: Option<String>,
    pub url: Option<String>,
}

/// A locale's texts (defaultLocale or locale manifest, or the singleton).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Localization {
    pub locale: String,
    pub publisher: Option<String>,
    pub publisher_url: Option<String>,
    pub publisher_support_url: Option<String>,
    pub privacy_url: Option<String>,
    pub author: Option<String>,
    pub package_name: Option<String>,
    pub package_url: Option<String>,
    pub license: Option<String>,
    pub license_url: Option<String>,
    pub copyright: Option<String>,
    pub copyright_url: Option<String>,
    pub short_description: Option<String>,
    pub description: Option<String>,
    /// defaultLocale only.
    pub moniker: Option<String>,
    pub tags: Vec<String>,
    pub agreements: Vec<Agreement>,
    pub release_notes: Option<String>,
    pub release_notes_url: Option<String>,
    pub purchase_url: Option<String>,
    pub installation_notes: Option<String>,
    /// (DocumentLabel, DocumentUrl).
    pub documentations: Vec<(Option<String>, Option<String>)>,
}

/// How the package was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    Singleton,
    MultiFile,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub id: String,
    pub version: String,
    /// ManifestVersion as written and parsed.
    pub manifest_version: String,
    pub schema: SchemaVersion,
    pub form: Form,
    pub channel: Option<String>,
    pub default_locale: Localization,
    /// The other locales, in the order given.
    pub locales: Vec<Localization>,
    pub installers: Vec<Installer>,
    pub warnings: Vec<Warning>,
}

impl Package {
    /// The texts for the preferred locales (BCP 47, best first): for the
    /// first preferred locale any locale matches (exactly, else by
    /// language; the default locale first among equals: gala's rule),
    /// that locale, with every field it leaves out taken from the default
    /// locale; the default locale when none matches. (Fallback per
    /// field: the locale manifest's fields are optional and winget shows
    /// the default locale's where a locale has none.)
    pub fn localization(&self, preferred: &[&str]) -> Localization {
        // The default locale is a candidate too, first among equals.
        let all = || core::iter::once(&self.default_locale).chain(self.locales.iter());
        let found = preferred.iter().find_map(|want| {
            all()
                .find(|l| l.locale.eq_ignore_ascii_case(want))
                .or_else(|| all().find(|l| same_language(&l.locale, want)))
        });
        let Some(l) = found.filter(|l| !core::ptr::eq(*l, &self.default_locale)) else {
            return self.default_locale.clone();
        };
        let d = &self.default_locale;
        let pick = |a: &Option<String>, b: &Option<String>| a.clone().or_else(|| b.clone());
        Localization {
            locale: l.locale.clone(),
            publisher: pick(&l.publisher, &d.publisher),
            publisher_url: pick(&l.publisher_url, &d.publisher_url),
            publisher_support_url: pick(&l.publisher_support_url, &d.publisher_support_url),
            privacy_url: pick(&l.privacy_url, &d.privacy_url),
            author: pick(&l.author, &d.author),
            package_name: pick(&l.package_name, &d.package_name),
            package_url: pick(&l.package_url, &d.package_url),
            license: pick(&l.license, &d.license),
            license_url: pick(&l.license_url, &d.license_url),
            copyright: pick(&l.copyright, &d.copyright),
            copyright_url: pick(&l.copyright_url, &d.copyright_url),
            short_description: pick(&l.short_description, &d.short_description),
            description: pick(&l.description, &d.description),
            moniker: d.moniker.clone(),
            tags: if l.tags.is_empty() { d.tags.clone() } else { l.tags.clone() },
            agreements: if l.agreements.is_empty() {
                d.agreements.clone()
            } else {
                l.agreements.clone()
            },
            release_notes: pick(&l.release_notes, &d.release_notes),
            release_notes_url: pick(&l.release_notes_url, &d.release_notes_url),
            purchase_url: pick(&l.purchase_url, &d.purchase_url),
            installation_notes: pick(&l.installation_notes, &d.installation_notes),
            documentations: if l.documentations.is_empty() {
                d.documentations.clone()
            } else {
                l.documentations.clone()
            },
        }
    }

    /// PackageName, else the identifier.
    pub fn display_name(&self) -> &str {
        self.default_locale.package_name.as_deref().unwrap_or(&self.id)
    }

    /// One package version from its files, given as (name, text): one
    /// singleton manifest, or the multi-file set in any order. The names
    /// are only for messages.
    pub fn from_files(files: &[(&str, &str)]) -> Result<Package, Error> {
        read(files)
    }

    /// [`Package::from_files`] with the files named `manifest 1`, ...
    pub fn from_texts(texts: &[&str]) -> Result<Package, Error> {
        let names: Vec<String> = (1..=texts.len()).map(|i| format!("manifest {i}")).collect();
        let files: Vec<(&str, &str)> =
            names.iter().map(String::as_str).zip(texts.iter().copied()).collect();
        read(&files)
    }
}

/// `de-DE` and `de-AT`, `de` and `de-DE`: the same primary language
/// subtag (BCP 47's first part).
pub fn same_language(a: &str, b: &str) -> bool {
    let lang = |s: &str| s.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    !a.is_empty() && lang(a) == lang(b)
}

/// One file being read: its name, its root node, its type, and the
/// warnings it gives.
struct Doc<'a> {
    name: &'a str,
    root: Node,
    kind: ManifestType,
}

struct Reader<'w> {
    schema: SchemaVersion,
    warnings: &'w mut Vec<Warning>,
}

fn error(file: &str, at: Option<Pos>, message: String) -> Error {
    Error { file: file.to_string(), at, message }
}

fn read(files: &[(&str, &str)]) -> Result<Package, Error> {
    if files.is_empty() {
        return Err(error("", None, "no manifest files".to_string()));
    }
    let mut warnings = Vec::new();
    let mut docs: Vec<Doc> = Vec::new();
    // Identity, from the first file; every other must agree.
    let mut identity: Option<(String, String, String)> = None;
    for &(name, text) in files {
        let root = yaml::parse(text).map_err(|e| error(name, Some(e.at), e.message.to_string()))?;
        if !matches!(root.value, Value::Map(_)) {
            return Err(error(
                name,
                Some(root.at),
                format!("the manifest is {}, not a mapping", root.kind()),
            ));
        }
        let need = |key: &str| -> Result<String, Error> {
            match root.get(key) {
                Some(n) => n.as_str().map(ToString::to_string).ok_or_else(|| {
                    error(name, Some(n.at), format!("{key} is {}, not text", n.kind()))
                }),
                None => Err(error(name, None, format!("no {key}"))),
            }
        };
        let id = need("PackageIdentifier")?;
        let version = need("PackageVersion")?;
        let mv = need("ManifestVersion")?;
        let kind_text = need("ManifestType")?;
        let kind_at = root.get("ManifestType").map(|n| n.at).unwrap_or_default();
        let kind = match ManifestType::parse(&kind_text) {
            Some(k) => k,
            None => match ManifestType::parse_any_case(&kind_text) {
                Some(k) => {
                    warnings.push(Warning {
                        file: name.to_string(),
                        at: kind_at,
                        message: format!(
                            "ManifestType {kind_text:?} is written {:?} in the schema",
                            k.as_str()
                        ),
                    });
                    k
                }
                None => {
                    return Err(error(
                        name,
                        Some(kind_at),
                        format!("unknown ManifestType {kind_text:?}"),
                    ));
                }
            },
        };
        match &identity {
            None => identity = Some((id, version, mv)),
            Some((i, v, m)) => {
                if *i != id || *v != version {
                    return Err(error(
                        name,
                        None,
                        format!("this file is {id} {version}, the first one {i} {v}"),
                    ));
                }
                if *m != mv {
                    return Err(error(
                        name,
                        root.get("ManifestVersion").map(|n| n.at),
                        format!("ManifestVersion {mv} differs from the first file's {m}"),
                    ));
                }
            }
        }
        docs.push(Doc { name, root, kind });
    }
    let (id, version, manifest_version) = identity.unwrap_or_default();
    let first = &docs[0];
    let mv_at = first.root.get("ManifestVersion").map(|n| n.at);
    let schema = SchemaVersion::parse(&manifest_version).ok_or_else(|| {
        error(
            first.name,
            mv_at,
            format!("ManifestVersion {manifest_version:?} is not major.minor.patch"),
        )
    })?;
    if schema.major != 1 {
        return Err(error(
            first.name,
            mv_at,
            format!("ManifestVersion {schema} is not 1.x (not supported)"),
        ));
    }
    if !schema.is_published() {
        warnings.push(Warning {
            file: first.name.to_string(),
            at: mv_at.unwrap_or_default(),
            message: if schema > schema::LATEST {
                format!(
                    "ManifestVersion {schema} is newer than the newest known schema {}",
                    schema::LATEST
                )
            } else {
                format!("ManifestVersion {schema} is not a published schema version")
            },
        });
    }
    if id.is_empty() || version.is_empty() {
        return Err(error(
            first.name,
            None,
            "an empty PackageIdentifier or PackageVersion".to_string(),
        ));
    }

    let mut r = Reader { schema, warnings: &mut warnings };
    for d in &docs {
        r.check_fields(d, &d.root, Level::Root);
    }

    // The files' roles.
    let count = |k: ManifestType| docs.iter().filter(|d| d.kind == k).count();
    let form;
    let installer_doc;
    let default_doc;
    let mut locale_docs: Vec<&Doc> = Vec::new();
    if count(ManifestType::Singleton) > 0 {
        if docs.len() != 1 {
            return Err(error("", None, "a singleton manifest must be the only file".to_string()));
        }
        form = Form::Singleton;
        installer_doc = &docs[0];
        default_doc = &docs[0];
    } else {
        form = Form::MultiFile;
        for (k, what) in [
            (ManifestType::Version, "version"),
            (ManifestType::Installer, "installer"),
            (ManifestType::DefaultLocale, "defaultLocale"),
        ] {
            match count(k) {
                1 => {}
                0 => return Err(error("", None, format!("no {what} manifest"))),
                n => return Err(error("", None, format!("{n} {what} manifests (one expected)"))),
            }
        }
        let find = |k| docs.iter().find(|d| d.kind == k).unwrap_or(&docs[0]);
        let version_doc = find(ManifestType::Version);
        installer_doc = find(ManifestType::Installer);
        default_doc = find(ManifestType::DefaultLocale);
        let want = version_doc.root.get("DefaultLocale");
        let have = default_doc.root.get("PackageLocale");
        match (want.and_then(Node::as_str), have.and_then(Node::as_str)) {
            (Some(w), Some(h)) if w.eq_ignore_ascii_case(h) => {}
            (Some(w), Some(h)) => {
                return Err(error(
                    version_doc.name,
                    want.map(|n| n.at),
                    format!("DefaultLocale {w} but the defaultLocale manifest is {h}"),
                ));
            }
            (None, _) => return Err(error(version_doc.name, None, "no DefaultLocale".to_string())),
            (_, None) => return Err(error(default_doc.name, None, "no PackageLocale".to_string())),
        }
        locale_docs.extend(docs.iter().filter(|d| d.kind == ManifestType::Locale));
    }

    let default_locale = r.localization(default_doc)?;
    let mut locales: Vec<Localization> = Vec::new();
    for d in locale_docs {
        let l = r.localization(d)?;
        if l.locale.eq_ignore_ascii_case(&default_locale.locale)
            || locales.iter().any(|x| x.locale.eq_ignore_ascii_case(&l.locale))
        {
            return Err(error(d.name, None, format!("locale {} given twice", l.locale)));
        }
        locales.push(l);
    }

    let root = &installer_doc.root;
    let channel = r.text(installer_doc, root, "Channel");
    let Some(list) = root.get("Installers") else {
        return Err(error(installer_doc.name, None, "no Installers".to_string()));
    };
    if !matches!(list.value, Value::Seq(_)) || list.items().is_empty() {
        return Err(error(
            installer_doc.name,
            Some(list.at),
            "Installers is not a non-empty sequence".to_string(),
        ));
    }
    let mut installers = Vec::new();
    for (index, node) in list.items().iter().enumerate() {
        installers.push(r.installer(installer_doc, root, node, index)?);
    }
    r.check_duplicates(installer_doc, &installers);
    Ok(Package {
        id,
        version,
        manifest_version,
        schema,
        form,
        channel,
        default_locale,
        locales,
        installers,
        warnings,
    })
}

fn is_true(s: &str) -> Option<bool> {
    // YAML 1.2 core schema booleans.
    match s {
        "true" | "True" | "TRUE" => Some(true),
        "false" | "False" | "FALSE" => Some(false),
        _ => None,
    }
}

/// An integer as YAML's core schema writes one: decimal with a sign, or
/// `0x` hexadecimal, or `0o` octal.
fn integer(s: &str) -> Option<i64> {
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let v = if let Some(h) = digits.strip_prefix("0x").or_else(|| digits.strip_prefix("0X")) {
        i64::from_str_radix(h, 16).ok()?
    } else if let Some(o) = digits.strip_prefix("0o") {
        i64::from_str_radix(o, 8).ok()?
    } else {
        digits.parse::<i64>().ok()?
    };
    Some(if neg { -v } else { v })
}

impl Reader<'_> {
    fn warn(&mut self, d: &Doc, at: Pos, message: String) {
        self.warnings.push(Warning { file: d.name.to_string(), at, message });
    }

    /// Warnings for the keys of `node` that its schema does not have.
    fn check_fields(&mut self, d: &Doc, node: &Node, level: Level) {
        for e in node.entries() {
            match schema::field(d.kind, level, &e.key) {
                Some(f) if schema::newer(f.since, self.schema) => {
                    let m = format!(
                        "{} is from schema 1.{}.0, newer than the declared {}",
                        e.key, f.since, self.schema
                    );
                    self.warn(d, e.at, m);
                }
                Some(_) => {}
                None => {
                    let other_case = [
                        schema::IDENTITY,
                        schema::VERSION_FIELDS,
                        schema::INSTALLER_FIELDS,
                        schema::LOCALE_FIELDS,
                    ]
                    .iter()
                    .flat_map(|t| t.iter())
                    .find(|f| f.name.eq_ignore_ascii_case(&e.key) && f.name != e.key);
                    let m = match other_case {
                        Some(f) if schema::field(d.kind, level, f.name).is_some() => {
                            format!("{} is written {} in the schema", e.key, f.name)
                        }
                        _ => {
                            let where_ = match level {
                                Level::Root => "",
                                Level::Installer => " in an installer",
                            };
                            format!(
                                "{} is not a field of a {} manifest{where_}",
                                e.key,
                                d.kind.as_str()
                            )
                        }
                    };
                    self.warn(d, e.at, m);
                }
            }
        }
    }

    /// A scalar field (empty text is absent); a non-scalar is a warning.
    fn text(&mut self, d: &Doc, node: &Node, key: &str) -> Option<String> {
        let n = node.get(key)?;
        match n.as_str() {
            Some("") => None,
            Some(s) => Some(s.to_string()),
            None => {
                self.warn(d, n.at, format!("{key} is {}, not text (ignored)", n.kind()));
                None
            }
        }
    }

    /// A sequence of scalars; a lone scalar is taken as a one-item list
    /// with a warning.
    fn list(&mut self, d: &Doc, node: &Node, key: &str) -> Vec<String> {
        let Some(n) = node.get(key) else { return Vec::new() };
        match &n.value {
            Value::Seq(items) => {
                let mut out = Vec::new();
                for i in items {
                    match i.as_str() {
                        Some(s) if !s.is_empty() => out.push(s.to_string()),
                        Some(_) => {}
                        None => self.warn(
                            d,
                            i.at,
                            format!("an item of {key} is {}, not text (ignored)", i.kind()),
                        ),
                    }
                }
                out
            }
            Value::Str(s) => {
                self.warn(d, n.at, format!("{key} is a scalar, not a sequence"));
                alloc::vec![s.clone()]
            }
            _ => {
                self.warn(d, n.at, format!("{key} is {}, not a sequence (ignored)", n.kind()));
                Vec::new()
            }
        }
    }

    fn flag(&mut self, d: &Doc, node: &Node, key: &str) -> bool {
        let Some(n) = node.get(key) else { return false };
        match n.as_str().and_then(is_true) {
            Some(b) => b,
            None => {
                self.warn(d, n.at, format!("{key} is not true or false (taken as false)"));
                false
            }
        }
    }

    /// An enumeration value in its canonical spelling; None (with a
    /// warning) when unknown. A value newer than the declared schema or
    /// in another case is kept with a warning.
    fn enumerated(
        &mut self,
        d: &Doc,
        n: &Node,
        key: &str,
        values: EnumValues,
    ) -> Option<&'static str> {
        let Some(s) = n.as_str() else {
            self.warn(d, n.at, format!("{key} is {}, not text (ignored)", n.kind()));
            return None;
        };
        match schema::enum_value(values, s, self.schema) {
            EnumMatch::Ok(v) => Some(v),
            EnumMatch::OtherCase(v) => {
                self.warn(d, n.at, format!("{key} {s:?} is written {v:?} in the schema"));
                Some(v)
            }
            EnumMatch::Newer(v, since) => {
                let m = format!(
                    "{key} {v:?} is from schema 1.{since}.0, newer than the declared {}",
                    self.schema
                );
                self.warn(d, n.at, m);
                Some(v)
            }
            EnumMatch::Unknown => {
                self.warn(d, n.at, format!("{key} {s:?} is not a known value (ignored)"));
                None
            }
        }
    }

    fn enum_list(
        &mut self,
        d: &Doc,
        node: &Node,
        key: &str,
        values: EnumValues,
    ) -> Vec<&'static str> {
        let Some(n) = node.get(key) else { return Vec::new() };
        let items: Vec<Node> = match &n.value {
            Value::Seq(items) => items.clone(),
            Value::Str(_) => alloc::vec![n.clone()],
            _ => Vec::new(),
        };
        let mut out = Vec::new();
        for i in &items {
            if let Some(v) = self.enumerated(d, i, key, values)
                && !out.contains(&v)
            {
                out.push(v);
            }
        }
        out
    }

    fn localization(&mut self, d: &Doc) -> Result<Localization, Error> {
        let n = &d.root;
        let locale = self
            .text(d, n, "PackageLocale")
            .ok_or_else(|| error(d.name, None, "no PackageLocale".to_string()))?;
        let mut agreements = Vec::new();
        if let Some(a) = n.get("Agreements") {
            for item in a.items() {
                agreements.push(Agreement {
                    label: self.text(d, item, "AgreementLabel"),
                    text: self.text(d, item, "Agreement"),
                    url: self.text(d, item, "AgreementUrl"),
                });
            }
        }
        let mut documentations = Vec::new();
        if let Some(a) = n.get("Documentations") {
            for item in a.items() {
                documentations
                    .push((self.text(d, item, "DocumentLabel"), self.text(d, item, "DocumentUrl")));
            }
        }
        let moniker = self.text(d, n, "Moniker");
        Ok(Localization {
            locale,
            publisher: self.text(d, n, "Publisher"),
            publisher_url: self.text(d, n, "PublisherUrl"),
            publisher_support_url: self.text(d, n, "PublisherSupportUrl"),
            privacy_url: self.text(d, n, "PrivacyUrl"),
            author: self.text(d, n, "Author"),
            package_name: self.text(d, n, "PackageName"),
            package_url: self.text(d, n, "PackageUrl"),
            license: self.text(d, n, "License"),
            license_url: self.text(d, n, "LicenseUrl"),
            copyright: self.text(d, n, "Copyright"),
            copyright_url: self.text(d, n, "CopyrightUrl"),
            short_description: self.text(d, n, "ShortDescription").map(|s| s.trim().to_string()),
            description: self.text(d, n, "Description"),
            moniker,
            tags: self.list(d, n, "Tags"),
            agreements,
            release_notes: self.text(d, n, "ReleaseNotes"),
            release_notes_url: self.text(d, n, "ReleaseNotesUrl"),
            purchase_url: self.text(d, n, "PurchaseUrl"),
            installation_notes: self.text(d, n, "InstallationNotes"),
            documentations,
        })
    }

    fn installer(
        &mut self,
        d: &Doc,
        root: &Node,
        node: &Node,
        index: usize,
    ) -> Result<Installer, Error> {
        if !matches!(node.value, Value::Map(_)) {
            return Err(error(
                d.name,
                Some(node.at),
                format!("installer {} is {}, not a mapping", index + 1, node.kind()),
            ));
        }
        self.check_fields(d, node, Level::Installer);
        // The field from the installer, else from the root.
        let src = |key: &str| -> &Node { if node.get(key).is_some() { node } else { root } };
        let what = |key: &str| format!("installer {}: no {key}", index + 1);

        let arch_node = node
            .get("Architecture")
            .ok_or_else(|| error(d.name, Some(node.at), what("Architecture")))?;
        let arch_text = arch_node.as_str().unwrap_or("");
        let architecture =
            match self.enumerated(d, arch_node, "Architecture", schema::ARCHITECTURES) {
                Some(a) => Architecture::parse(a).unwrap_or(Architecture::Neutral),
                None => {
                    return Err(error(
                        d.name,
                        Some(arch_node.at),
                        format!("installer {}: unknown Architecture {arch_text:?}", index + 1),
                    ));
                }
            };
        let type_node = src("InstallerType")
            .get("InstallerType")
            .ok_or_else(|| error(d.name, Some(node.at), what("InstallerType")))?;
        let installer_type = self
            .enumerated(d, type_node, "InstallerType", schema::INSTALLER_TYPES)
            .and_then(InstallerType::parse);
        let nested_installer_type = src("NestedInstallerType")
            .get("NestedInstallerType")
            .and_then(|n| {
                self.enumerated(d, n, "NestedInstallerType", schema::NESTED_INSTALLER_TYPES)
            })
            .and_then(InstallerType::parse);
        if installer_type == Some(InstallerType::Zip) && nested_installer_type.is_none() {
            self.warn(
                d,
                node.at,
                format!("installer {}: a zip without NestedInstallerType", index + 1),
            );
        }
        let mut nested_installer_files = Vec::new();
        if let Some(files) = src("NestedInstallerFiles").get("NestedInstallerFiles") {
            for f in files.items() {
                let Some(path) = self.text(d, f, "RelativeFilePath") else {
                    return Err(error(
                        d.name,
                        Some(f.at),
                        format!(
                            "installer {}: a NestedInstallerFiles entry without RelativeFilePath",
                            index + 1
                        ),
                    ));
                };
                nested_installer_files.push(NestedFile {
                    relative_path: path,
                    alias: self.text(d, f, "PortableCommandAlias"),
                });
            }
        }
        let url = self
            .text(d, src("InstallerUrl"), "InstallerUrl")
            .ok_or_else(|| error(d.name, Some(node.at), what("InstallerUrl")))?;
        let sha_node = src("InstallerSha256").get("InstallerSha256");
        let sha256 = self
            .text(d, src("InstallerSha256"), "InstallerSha256")
            .ok_or_else(|| error(d.name, Some(node.at), what("InstallerSha256")))?
            .to_ascii_uppercase();
        if sha256.len() != 64 || !sha256.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(error(
                d.name,
                sha_node.map(|n| n.at),
                format!("installer {}: InstallerSha256 is not 64 hexadecimal digits", index + 1),
            ));
        }
        let scope = src("Scope")
            .get("Scope")
            .and_then(|n| self.enumerated(d, n, "Scope", schema::SCOPES))
            .and_then(Scope::parse);
        let upgrade_behavior = src("UpgradeBehavior")
            .get("UpgradeBehavior")
            .and_then(|n| self.enumerated(d, n, "UpgradeBehavior", schema::UPGRADE_BEHAVIORS))
            .map(|v| match v {
                "install" => UpgradeBehavior::Install,
                "uninstallPrevious" => UpgradeBehavior::UninstallPrevious,
                _ => UpgradeBehavior::Deny,
            });
        let elevation = src("ElevationRequirement")
            .get("ElevationRequirement")
            .and_then(|n| {
                self.enumerated(d, n, "ElevationRequirement", schema::ELEVATION_REQUIREMENTS)
            })
            .map(|v| match v {
                "elevationRequired" => Elevation::Required,
                "elevationProhibited" => Elevation::Prohibited,
                _ => Elevation::ElevatesSelf,
            });
        let repair_behavior = src("RepairBehavior")
            .get("RepairBehavior")
            .and_then(|n| self.enumerated(d, n, "RepairBehavior", schema::REPAIR_BEHAVIORS))
            .map(ToString::to_string);
        let switches = self.switches(d, root, node);
        let success_codes = self.codes(d, src("InstallerSuccessCodes"), "InstallerSuccessCodes");
        let expected_return_codes = self.expected_codes(d, src("ExpectedReturnCodes"));
        let dependencies = self.dependencies(d, src("Dependencies"));
        let apps_and_features = self.arp_entries(d, src("AppsAndFeaturesEntries"));
        let markets = match src("Markets").get("Markets") {
            Some(m) => Markets {
                allowed: self.list(d, m, "AllowedMarkets"),
                excluded: self.list(d, m, "ExcludedMarkets"),
            },
            None => Markets::default(),
        };
        let installation_metadata =
            src("InstallationMetadata").get("InstallationMetadata").map(|m| {
                let mut files = Vec::new();
                if let Some(list) = m.get("Files") {
                    for f in list.items() {
                        files.push(InstalledFile {
                            relative_path: self.text(d, f, "RelativeFilePath").unwrap_or_default(),
                            sha256: self.text(d, f, "FileSha256"),
                            file_type: self.text(d, f, "FileType"),
                            invocation_parameter: self.text(d, f, "InvocationParameter"),
                            display_name: self.text(d, f, "DisplayName"),
                        });
                    }
                }
                InstallationMetadata {
                    default_install_location: self.text(d, m, "DefaultInstallLocation"),
                    files,
                }
            });
        let authentication = src("Authentication")
            .get("Authentication")
            .and_then(|a| self.text(d, a, "AuthenticationType"));
        let unsupported_os_architectures = self
            .enum_list(
                d,
                src("UnsupportedOSArchitectures"),
                "UnsupportedOSArchitectures",
                schema::UNSUPPORTED_OS_ARCHITECTURES,
            )
            .into_iter()
            .filter_map(Architecture::parse)
            .collect();
        let release_date =
            src("ReleaseDate").get("ReleaseDate").and_then(|n| n.as_str()).map(ToString::to_string);
        Ok(Installer {
            index,
            at: node.at,
            architecture,
            installer_type,
            nested_installer_type,
            nested_installer_files,
            url,
            sha256,
            signature_sha256: self.text(d, node, "SignatureSha256"),
            locale: self.text(d, src("InstallerLocale"), "InstallerLocale"),
            platform: self
                .enum_list(d, src("Platform"), "Platform", schema::PLATFORMS)
                .into_iter()
                .map(ToString::to_string)
                .collect(),
            minimum_os_version: self.text(d, src("MinimumOSVersion"), "MinimumOSVersion"),
            scope,
            install_modes: self
                .enum_list(d, src("InstallModes"), "InstallModes", schema::INSTALL_MODES)
                .into_iter()
                .map(ToString::to_string)
                .collect(),
            switches,
            success_codes,
            expected_return_codes,
            upgrade_behavior,
            commands: self.list(d, src("Commands"), "Commands"),
            protocols: self.list(d, src("Protocols"), "Protocols"),
            file_extensions: self.list(d, src("FileExtensions"), "FileExtensions"),
            dependencies,
            package_family_name: self.text(d, src("PackageFamilyName"), "PackageFamilyName"),
            product_code: self.text(d, src("ProductCode"), "ProductCode"),
            capabilities: self.list(d, src("Capabilities"), "Capabilities"),
            restricted_capabilities: self.list(
                d,
                src("RestrictedCapabilities"),
                "RestrictedCapabilities",
            ),
            markets,
            aborts_terminal: self.flag(
                d,
                src("InstallerAbortsTerminal"),
                "InstallerAbortsTerminal",
            ),
            release_date,
            install_location_required: self.flag(
                d,
                src("InstallLocationRequired"),
                "InstallLocationRequired",
            ),
            require_explicit_upgrade: self.flag(
                d,
                src("RequireExplicitUpgrade"),
                "RequireExplicitUpgrade",
            ),
            display_install_warnings: self.flag(
                d,
                src("DisplayInstallWarnings"),
                "DisplayInstallWarnings",
            ),
            unsupported_os_architectures,
            unsupported_arguments: self
                .enum_list(
                    d,
                    src("UnsupportedArguments"),
                    "UnsupportedArguments",
                    schema::UNSUPPORTED_ARGUMENTS,
                )
                .into_iter()
                .map(ToString::to_string)
                .collect(),
            apps_and_features,
            elevation,
            installation_metadata,
            download_command_prohibited: self.flag(
                d,
                src("DownloadCommandProhibited"),
                "DownloadCommandProhibited",
            ),
            repair_behavior,
            archive_binaries_depend_on_path: self.flag(
                d,
                src("ArchiveBinariesDependOnPath"),
                "ArchiveBinariesDependOnPath",
            ),
            authentication,
        })
    }

    /// InstallerSwitches: the root's, then the installer's per switch.
    fn switches(&mut self, d: &Doc, root: &Node, node: &Node) -> Switches {
        let mut s = Switches::default();
        for level in [root, node] {
            let Some(m) = level.get("InstallerSwitches") else { continue };
            for e in m.entries() {
                let Some(key) = self.enumerated(
                    d,
                    &Node { value: Value::Str(e.key.clone()), at: e.at },
                    "InstallerSwitches key",
                    schema::SWITCHES,
                ) else {
                    continue;
                };
                let value = match &e.value.value {
                    Value::Str(v) => Some(v.clone()),
                    Value::Null => None,
                    _ => {
                        self.warn(
                            d,
                            e.value.at,
                            format!("switch {key} is {}, not text (ignored)", e.value.kind()),
                        );
                        continue;
                    }
                };
                if let (Some(slot), Some(v)) = (s.slot(key), value) {
                    *slot = Some(v);
                }
            }
        }
        s
    }

    fn codes(&mut self, d: &Doc, node: &Node, key: &str) -> Vec<i64> {
        let mut out = Vec::new();
        for s in self.list(d, node, key) {
            match integer(&s) {
                Some(c) => out.push(c),
                None => {
                    let at = node.get(key).map(|n| n.at).unwrap_or_default();
                    self.warn(d, at, format!("{key}: {s:?} is not an integer (ignored)"));
                }
            }
        }
        out
    }

    fn expected_codes(&mut self, d: &Doc, node: &Node) -> Vec<ExpectedReturnCode> {
        let mut out = Vec::new();
        let Some(list) = node.get("ExpectedReturnCodes") else { return out };
        for item in list.items() {
            let code = item.get("InstallerReturnCode").and_then(Node::as_str).and_then(integer);
            let response = item
                .get("ReturnResponse")
                .and_then(|n| self.enumerated(d, n, "ReturnResponse", schema::RETURN_RESPONSES));
            match (code, response) {
                (Some(code), Some(response)) => out.push(ExpectedReturnCode {
                    code,
                    response: response.to_string(),
                    url: self.text(d, item, "ReturnResponseUrl"),
                }),
                _ => self.warn(
                    d,
                    item.at,
                    "an ExpectedReturnCodes entry without a valid code and response (ignored)"
                        .to_string(),
                ),
            }
        }
        out
    }

    fn dependencies(&mut self, d: &Doc, node: &Node) -> Dependencies {
        let Some(n) = node.get("Dependencies") else { return Dependencies::default() };
        let mut packages = Vec::new();
        if let Some(list) = n.get("PackageDependencies") {
            for item in list.items() {
                match self.text(d, item, "PackageIdentifier") {
                    Some(id) => packages.push(PackageDependency {
                        id,
                        minimum_version: self.text(d, item, "MinimumVersion"),
                    }),
                    None => self.warn(
                        d,
                        item.at,
                        "a PackageDependencies entry without PackageIdentifier (ignored)"
                            .to_string(),
                    ),
                }
            }
        }
        Dependencies {
            windows_features: self.list(d, n, "WindowsFeatures"),
            windows_libraries: self.list(d, n, "WindowsLibraries"),
            packages,
            external: self.list(d, n, "ExternalDependencies"),
        }
    }

    fn arp_entries(&mut self, d: &Doc, node: &Node) -> Vec<ArpEntry> {
        let mut out = Vec::new();
        let Some(list) = node.get("AppsAndFeaturesEntries") else { return out };
        for item in list.items() {
            let installer_type = item
                .get("InstallerType")
                .and_then(|n| self.enumerated(d, n, "InstallerType", schema::INSTALLER_TYPES))
                .and_then(InstallerType::parse);
            out.push(ArpEntry {
                display_name: self.text(d, item, "DisplayName"),
                publisher: self.text(d, item, "Publisher"),
                display_version: self.text(d, item, "DisplayVersion"),
                product_code: self.text(d, item, "ProductCode"),
                upgrade_code: self.text(d, item, "UpgradeCode"),
                installer_type,
            });
        }
        out
    }

    /// Two installers winget could not tell apart (same architecture,
    /// type, scope, locale): the community repository's validation
    /// rejects them; a warning here (selection keeps the first).
    fn check_duplicates(&mut self, d: &Doc, installers: &[Installer]) {
        for (i, a) in installers.iter().enumerate() {
            let key = |x: &Installer| {
                (
                    x.architecture,
                    x.installer_type,
                    x.scope,
                    x.locale.as_ref().map(|l| l.to_ascii_lowercase()),
                )
            };
            if let Some(b) = installers[..i].iter().find(|b| key(b) == key(a)) {
                let m = format!(
                    "installer {} repeats installer {} (same architecture, type, scope and locale)",
                    i + 1,
                    b.index + 1
                );
                self.warn(d, a.at, m);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "71B2FEF860ABE467217A538FF31DE02F5258807C0129F771846F87BD029AAFC5";

    fn singleton(mv: &str, body: &str) -> String {
        format!(
            "PackageIdentifier: A.B\nPackageVersion: 1.0\nPackageLocale: en-US\nPublisher: P\nPackageName: N\nLicense: MIT\nShortDescription: d\nManifestType: singleton\nManifestVersion: {mv}\n{body}"
        )
    }

    #[test]
    fn ripgrep_multi_file_real() {
        let files = [
            (
                "v",
                include_str!(
                    "../../../pk_install_test/source/manifests/b/BurntSushi/ripgrep/MSVC/15.2.0/BurntSushi.ripgrep.MSVC.yaml"
                ),
            ),
            (
                "i",
                include_str!(
                    "../../../pk_install_test/source/manifests/b/BurntSushi/ripgrep/MSVC/15.2.0/BurntSushi.ripgrep.MSVC.installer.yaml"
                ),
            ),
            (
                "l",
                include_str!(
                    "../../../pk_install_test/source/manifests/b/BurntSushi/ripgrep/MSVC/15.2.0/BurntSushi.ripgrep.MSVC.locale.en-US.yaml"
                ),
            ),
        ];
        let p = Package::from_files(&files).unwrap();
        assert_eq!(p.id, "BurntSushi.ripgrep.MSVC");
        assert_eq!(p.form, Form::MultiFile);
        assert_eq!(p.schema, SchemaVersion::new(1, 12, 0));
        assert_eq!(p.display_name(), "RipGrep MSVC");
        assert_eq!(p.default_locale.moniker.as_deref(), Some("rg"));
        assert_eq!(p.installers.len(), 3);
        let x64 = p.installers.iter().find(|i| i.architecture == Architecture::X64).unwrap();
        assert_eq!(x64.installer_type, Some(InstallerType::Zip));
        assert_eq!(x64.effective_type(), Some(InstallerType::Portable));
        assert_eq!(x64.nested_installer_files[0].alias.as_deref(), Some("rg"));
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        // Any order.
        let back = Package::from_files(&[files[2], files[0], files[1]]).unwrap();
        assert_eq!(back.installers, p.installers);
    }

    #[test]
    fn root_defaults_and_switch_merge() {
        let body = format!(
            "InstallerType: inno\nScope: user\nInstallerSwitches:\n  Silent: /VERYSILENT\n  Custom: /ROOT\nUpgradeBehavior: install\nCommands: [a]\nInstallers:\n- Architecture: x64\n  InstallerUrl: https://h/a.exe\n  InstallerSha256: {SHA}\n  InstallerSwitches:\n    Custom: /MINE\n  Scope: machine\n- Architecture: x86\n  InstallerType: nullsoft\n  InstallerUrl: https://h/b.exe\n  InstallerSha256: {}\n  Commands: []\n",
            SHA.to_ascii_lowercase()
        );
        let p = Package::from_texts(&[&singleton("1.6.0", &body)]).unwrap();
        let a = &p.installers[0];
        assert_eq!(a.installer_type, Some(InstallerType::Inno));
        assert_eq!(a.scope, Some(Scope::Machine));
        assert_eq!(a.switches.silent.as_deref(), Some("/VERYSILENT"));
        assert_eq!(a.switches.custom.as_deref(), Some("/MINE"));
        assert_eq!(a.upgrade_behavior, Some(UpgradeBehavior::Install));
        assert_eq!(a.commands, ["a"]);
        let b = &p.installers[1];
        assert_eq!(b.installer_type, Some(InstallerType::Nullsoft));
        assert_eq!(b.scope, Some(Scope::User));
        assert_eq!(b.sha256, SHA);
        // An installer's empty list replaces the root's (gala's reading of
        // "installer level is used").
        assert!(b.commands.is_empty());
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    }

    #[test]
    fn fields_by_schema_version_warn() {
        let body = format!(
            "Installers:\n- Architecture: x64\n  InstallerType: zip\n  NestedInstallerType: portable\n  NestedInstallerFiles:\n  - RelativeFilePath: a.exe\n  InstallerUrl: https://h/a.zip\n  InstallerSha256: {SHA}\n  UpgradeBehavior: deny\n  Frobnicate: 1\n  scope: user\n"
        );
        let p = Package::from_texts(&[&singleton("1.0.0", &body)]).unwrap();
        let w: Vec<String> = p.warnings.iter().map(|w| w.to_string()).collect();
        assert!(
            w.iter().any(|w| w.contains(
                "NestedInstallerType is from schema 1.4.0, newer than the declared 1.0.0"
            )),
            "{w:?}"
        );
        assert!(
            w.iter().any(|w| w.contains("InstallerType \"zip\" is from schema 1.4.0")),
            "{w:?}"
        );
        assert!(
            w.iter().any(|w| w.contains("UpgradeBehavior \"deny\" is from schema 1.6.0")),
            "{w:?}"
        );
        assert!(
            w.iter()
                .any(|w| w
                    .contains("Frobnicate is not a field of a singleton manifest in an installer")),
            "{w:?}"
        );
        assert!(
            w.iter().any(|w| w.starts_with("manifest 1:20:3: scope is written Scope")),
            "{w:?}"
        );
        // Still read, as winget's reader would.
        let i = &p.installers[0];
        assert_eq!(i.effective_type(), Some(InstallerType::Portable));
        assert_eq!(i.scope, Some(Scope::User));
        assert_eq!(i.upgrade_behavior, Some(UpgradeBehavior::Deny));
        // In 1.6.0 only the unknown field and the case remain.
        let p = Package::from_texts(&[&singleton("1.6.0", &body)]).unwrap();
        assert_eq!(p.warnings.len(), 2, "{:?}", p.warnings);
    }

    #[test]
    fn unknown_type_kept_unselectable() {
        let body = format!(
            "Installers:\n- Architecture: x64\n  InstallerType: msstore\n  InstallerUrl: u\n  InstallerSha256: {SHA}\n"
        );
        let p = Package::from_texts(&[&singleton("1.28.0", &body)]).unwrap();
        assert_eq!(p.installers[0].installer_type, None);
        assert!(p.warnings[0].message.contains("not a known value"));
    }

    #[test]
    fn every_field_read() {
        let body = format!(
            "Channel: beta\nInstallers:\n- Architecture: arm64\n  InstallerType: msi\n  InstallerUrl: https://h/a.msi\n  InstallerSha256: {SHA}\n  SignatureSha256: {SHA}\n  InstallerLocale: de-DE\n  Platform: [Windows.Desktop]\n  MinimumOSVersion: 10.0.17763.0\n  InstallModes: [silent, interactive]\n  InstallerSuccessCodes: [3010, -1, 0x10]\n  ExpectedReturnCodes:\n  - InstallerReturnCode: 1618\n    ReturnResponse: installInProgress\n    ReturnResponseUrl: https://h/r\n  Protocols: [x]\n  FileExtensions: [txt]\n  Dependencies:\n    WindowsFeatures: [NetFx3]\n    WindowsLibraries: [Microsoft.UI.Xaml]\n    PackageDependencies:\n    - PackageIdentifier: Microsoft.VCRedist.2015+.x64\n      MinimumVersion: 14.30\n    ExternalDependencies: [Java]\n  ProductCode: '{{ABC}}'\n  Markets:\n    AllowedMarkets: [US]\n  InstallerAbortsTerminal: true\n  ReleaseDate: 2024-01-02\n  InstallLocationRequired: true\n  RequireExplicitUpgrade: True\n  DisplayInstallWarnings: false\n  UnsupportedOSArchitectures: [arm]\n  UnsupportedArguments: [log]\n  AppsAndFeaturesEntries:\n  - DisplayName: N\n    Publisher: P\n    DisplayVersion: 1.0.0.5\n    ProductCode: '{{ABC}}'\n    UpgradeCode: '{{DEF}}'\n    InstallerType: msi\n  ElevationRequirement: elevatesSelf\n  InstallationMetadata:\n    DefaultInstallLocation: '%ProgramFiles%\\N'\n    Files:\n    - RelativeFilePath: n.exe\n      FileType: launch\n  DownloadCommandProhibited: true\n  RepairBehavior: modify\n  ArchiveBinariesDependOnPath: true\n  Authentication:\n    AuthenticationType: none\n  InstallerSwitches:\n    Repair: /r\n    Log: /l <LOGPATH>\n"
        );
        let p = Package::from_texts(&[&singleton("1.10.0", &body)]).unwrap();
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        assert_eq!(p.channel.as_deref(), Some("beta"));
        let i = &p.installers[0];
        assert_eq!(i.locale.as_deref(), Some("de-DE"));
        assert_eq!(i.platform, ["Windows.Desktop"]);
        assert_eq!(i.install_modes, ["silent", "interactive"]);
        assert_eq!(i.success_codes, [3010, -1, 16]);
        assert_eq!(i.expected_return_codes[0].code, 1618);
        assert_eq!(i.dependencies.packages[0].minimum_version.as_deref(), Some("14.30"));
        assert_eq!(i.dependencies.external, ["Java"]);
        assert_eq!(i.product_code.as_deref(), Some("{ABC}"));
        assert_eq!(i.markets.allowed, ["US"]);
        assert!(i.aborts_terminal && i.install_location_required && i.require_explicit_upgrade);
        assert!(!i.display_install_warnings);
        assert_eq!(i.unsupported_os_architectures, [Architecture::Arm]);
        assert_eq!(i.apps_and_features[0].upgrade_code.as_deref(), Some("{DEF}"));
        assert_eq!(i.elevation, Some(Elevation::ElevatesSelf));
        assert_eq!(
            i.installation_metadata.as_ref().unwrap().files[0].file_type.as_deref(),
            Some("launch")
        );
        assert!(i.download_command_prohibited && i.archive_binaries_depend_on_path);
        assert_eq!(i.repair_behavior.as_deref(), Some("modify"));
        assert_eq!(i.authentication.as_deref(), Some("none"));
        assert_eq!(i.switches.repair.as_deref(), Some("/r"));
        assert_eq!(i.release_date.as_deref(), Some("2024-01-02"));
    }

    fn multi(locale_files: &[&str]) -> Result<Package, Error> {
        let v = "PackageIdentifier: A.B\nPackageVersion: 2.0\nDefaultLocale: en-US\nManifestType: version\nManifestVersion: 1.9.0\n";
        let i = format!(
            "PackageIdentifier: A.B\nPackageVersion: 2.0\nInstallers:\n- Architecture: x64\n  InstallerType: exe\n  InstallerUrl: u\n  InstallerSha256: {SHA}\nManifestType: installer\nManifestVersion: 1.9.0\n"
        );
        let mut texts = alloc::vec![v, i.as_str()];
        texts.extend_from_slice(locale_files);
        Package::from_texts(&texts)
    }

    const EN: &str = "PackageIdentifier: A.B\nPackageVersion: 2.0\nPackageLocale: en-US\nPublisher: P\nPackageName: Name\nLicense: MIT\nShortDescription: English\nMoniker: ab\nTags: [one]\nManifestType: defaultLocale\nManifestVersion: 1.9.0\n";
    const DE: &str = "PackageIdentifier: A.B\nPackageVersion: 2.0\nPackageLocale: de-DE\nShortDescription: Deutsch\nManifestType: locale\nManifestVersion: 1.9.0\n";

    #[test]
    fn locales_fall_back_to_default() {
        let p = multi(&[EN, DE]).unwrap();
        let de = p.localization(&["de-AT"]);
        assert_eq!(de.locale, "de-DE");
        assert_eq!(de.short_description.as_deref(), Some("Deutsch"));
        assert_eq!(de.package_name.as_deref(), Some("Name"));
        assert_eq!(de.moniker.as_deref(), Some("ab"));
        assert_eq!(de.tags, ["one"]);
        assert_eq!(p.localization(&["fr-FR"]).locale, "en-US");
        assert_eq!(p.localization(&["en-GB", "de-DE"]).locale, "en-US");
        assert_eq!(p.localization(&["fr-FR", "de-DE"]).locale, "de-DE");
        assert_eq!(p.localization(&["en-US", "de-DE"]).locale, "en-US");
    }

    #[test]
    fn multi_file_errors() {
        let e = |r: Result<Package, Error>| r.unwrap_err().to_string();
        assert_eq!(e(multi(&[])), "no defaultLocale manifest");
        assert_eq!(e(multi(&[EN, EN])), "2 defaultLocale manifests (one expected)");
        let de_default = DE.replace("locale\n", "defaultLocale\n").replace(
            "ShortDescription",
            "Publisher: P\nPackageName: N\nLicense: L\nShortDescription",
        );
        assert!(
            e(multi(&[&de_default]))
                .contains("DefaultLocale en-US but the defaultLocale manifest is de-DE")
        );
        let other = DE.replace("2.0", "2.1");
        assert!(e(multi(&[EN, &other])).contains("this file is A.B 2.1, the first one A.B 2.0"));
        let mv = DE.replace("1.9.0", "1.10.0");
        assert_eq!(
            e(multi(&[EN, &mv])),
            "manifest 4:6:18: ManifestVersion 1.10.0 differs from the first file's 1.9.0"
        );
        assert!(e(multi(&[EN, DE, DE])).contains("locale de-DE given twice"));
    }

    #[test]
    fn errors_name_file_and_position() {
        let e = |t: &str| Package::from_texts(&[t]).unwrap_err().to_string();
        assert_eq!(e("a: [\n"), "manifest 1:2:1: an unterminated flow collection");
        assert_eq!(e("- x\n"), "manifest 1:1:1: the manifest is a sequence, not a mapping");
        assert_eq!(e("PackageIdentifier: A\n"), "manifest 1: no PackageVersion");
        let body = "Installers:\n- Architecture: x64\n  InstallerType: exe\n  InstallerUrl: u\n  InstallerSha256: 12\n".to_string();
        assert_eq!(
            e(&singleton("1.6.0", &body)),
            "manifest 1:14:20: installer 1: InstallerSha256 is not 64 hexadecimal digits"
        );
        assert_eq!(
            e(&singleton("2.0.0", &body)),
            "manifest 1:9:18: ManifestVersion 2.0.0 is not 1.x (not supported)"
        );
        assert_eq!(
            e(&singleton("1.6", &body)),
            "manifest 1:9:18: ManifestVersion \"1.6\" is not major.minor.patch"
        );
        let body = format!(
            "Installers:\n- Architecture: sparc\n  InstallerType: exe\n  InstallerUrl: u\n  InstallerSha256: {SHA}\n"
        );
        assert_eq!(
            e(&singleton("1.6.0", &body)),
            "manifest 1:11:17: installer 1: unknown Architecture \"sparc\""
        );
        let body = format!(
            "Installers:\n- Architecture: x64\n  InstallerUrl: u\n  InstallerSha256: {SHA}\n"
        );
        assert_eq!(e(&singleton("1.6.0", &body)), "manifest 1:11:3: installer 1: no InstallerType");
        assert_eq!(
            e(&singleton("1.6.0", "Installers: []\n")),
            "manifest 1:10:13: Installers is not a non-empty sequence"
        );
        assert_eq!(
            e(&singleton("1.6.0", "ManifestType: x\n")),
            "manifest 1:10:1: a key given twice"
        );
        assert!(
            e(&singleton("1.6.0", "").replace("singleton", "bogus"))
                .contains("unknown ManifestType \"bogus\"")
        );
    }

    #[test]
    fn newer_schema_warns() {
        let body = format!(
            "Installers:\n- Architecture: x64\n  InstallerType: exe\n  InstallerUrl: u\n  InstallerSha256: {SHA}\n"
        );
        let p = Package::from_texts(&[&singleton("1.99.0", &body)]).unwrap();
        assert!(p.warnings[0].message.contains("newer than the newest known schema 1.28.0"));
        let p = Package::from_texts(&[&singleton("1.3.0", &body)]).unwrap();
        assert!(p.warnings[0].message.contains("not a published schema version"));
    }

    #[test]
    fn duplicate_installers_warn() {
        let body = format!(
            "Installers:\n- Architecture: x64\n  InstallerType: exe\n  InstallerUrl: u\n  InstallerSha256: {SHA}\n- Architecture: x64\n  InstallerType: exe\n  InstallerUrl: v\n  InstallerSha256: {SHA}\n"
        );
        let p = Package::from_texts(&[&singleton("1.6.0", &body)]).unwrap();
        assert!(p.warnings[0].message.contains("installer 2 repeats installer 1"));
    }
}
