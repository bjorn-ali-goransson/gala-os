//! What is installed: Add/Remove Programs (ARP) records, the uninstall
//! keys programs register (`...\CurrentVersion\Uninstall\<key>` under
//! HKLM, HKLM's WOW6432Node and HKCU: learn.microsoft.com "Uninstall
//! Registry Key"), matched to packages.
//!
//! The manifest's side (schema 1.1.0 on): `AppsAndFeaturesEntries`
//! (DisplayName, Publisher, DisplayVersion, ProductCode, UpgradeCode,
//! InstallerType: "the values the installer writes to ARP"), the
//! installer's `ProductCode` ("used for correlation of packages across
//! sources"), and the default locale's PackageName and Publisher.
//! winget list's documentation says installed programs are correlated
//! with packages by these; it does not give the precedence or the name
//! normalization, so these are gala's rules (deterministic):
//!
//! 1. `WinGetPackageIdentifier` in the record (what winget and gala
//!    write for portable/zip installs) equal to the identifier;
//! 2. the record's key or ProductCode equal to a product code (any case,
//!    braces optional);
//! 3. its UpgradeCode equal to an UpgradeCode;
//! 4. its DisplayName and Publisher equal to an entry's (or PackageName
//!    and Publisher) after normalizing: letters and digits only, lower
//!    case, the record's own DisplayVersion and architecture words
//!    (`x64`, `x86`, `64-bit`, `32-bit`, `arm64`) removed from the name,
//!    company suffixes (`inc`, `corp`, `corporation`, `llc`, `ltd`,
//!    `gmbh`, `co`) removed from the publisher. A record without a
//!    Publisher matches on the name alone only when an entry has no
//!    Publisher either.
//!
//! The installed *package* version is the PackageVersion whose
//! AppsAndFeaturesEntries lists the record's DisplayVersion (a manifest
//! writes DisplayVersion when the program's own version differs from the
//! package's), else the DisplayVersion itself, else `Unknown`
//! ([`installed_version`]); an update is available when the latest
//! package version is later ([`update_available`]).

use super::package::{InstallerType, Package, Scope};
use super::version;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Ordering;

/// One installed program's uninstall key, as read from the registry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArpRecord {
    /// The key's name (an MSI's is its ProductCode).
    pub key: String,
    pub display_name: Option<String>,
    pub publisher: Option<String>,
    pub display_version: Option<String>,
    /// An MSI's UpgradeCode, when the caller knows it (from Windows
    /// Installer's data; not an ARP value).
    pub upgrade_code: Option<String>,
    /// The `WinGetPackageIdentifier` value, if present.
    pub winget_id: Option<String>,
    /// HKCU: user; HKLM: machine.
    pub scope: Option<Scope>,
    /// `WindowsInstaller = 1`: an MSI.
    pub windows_installer: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchKind {
    /// Strongest first.
    Identifier,
    ProductCode,
    UpgradeCode,
    NameAndPublisher,
}

/// What identifies a package's installed programs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Signature {
    pub id: String,
    pub product_codes: Vec<String>,
    pub upgrade_codes: Vec<String>,
    /// (normalized name, normalized publisher or empty).
    pub names: Vec<(String, String)>,
}

fn guid_key(s: &str) -> String {
    s.trim().trim_start_matches('{').trim_end_matches('}').to_ascii_lowercase()
}

fn alnum_words(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut w = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() || c == '.' || c == '-' {
            w.extend(c.to_lowercase());
        } else if !w.is_empty() {
            words.push(core::mem::take(&mut w));
        }
    }
    if !w.is_empty() {
        words.push(w);
    }
    words
}

fn squash(words: &[String]) -> String {
    words.iter().flat_map(|w| w.chars()).filter(|c| c.is_alphanumeric()).collect()
}

/// A DisplayName normalized (see the module text); `version`: the
/// record's DisplayVersion, removed as a word.
pub fn normalize_name(name: &str, version: Option<&str>) -> String {
    let ver = version.map(|v| v.trim().to_ascii_lowercase());
    let words: Vec<String> = alnum_words(name)
        .into_iter()
        .filter(|w| {
            let bare = w.trim_start_matches('v');
            !matches!(
                w.as_str(),
                "x64" | "x86" | "64-bit" | "32-bit" | "arm64" | "x86_64" | "amd64"
            ) && ver.as_deref().is_none_or(|v| bare != v && w != v)
        })
        .collect();
    squash(&words)
}

pub fn normalize_publisher(publisher: &str) -> String {
    let words: Vec<String> = alnum_words(publisher)
        .into_iter()
        .map(|w| w.trim_end_matches('.').to_string())
        .filter(|w| {
            !matches!(
                w.as_str(),
                "inc" | "corp" | "corporation" | "llc" | "ltd" | "gmbh" | "co" | "limited"
            )
        })
        .collect();
    squash(&words)
}

impl Signature {
    /// From every installer's AppsAndFeaturesEntries and ProductCode and
    /// the default locale's PackageName and Publisher.
    pub fn of(package: &Package) -> Signature {
        let mut s = Signature { id: package.id.clone(), ..Signature::default() };
        let add = |list: &mut Vec<String>, v: &Option<String>| {
            if let Some(v) = v {
                let k = guid_key(v);
                if !k.is_empty() && !list.contains(&k) {
                    list.push(k);
                }
            }
        };
        for i in &package.installers {
            add(&mut s.product_codes, &i.product_code);
            for e in &i.apps_and_features {
                add(&mut s.product_codes, &e.product_code);
                add(&mut s.upgrade_codes, &e.upgrade_code);
            }
        }
        let mut push_name = |name: &str, publisher: Option<&str>, version: Option<&str>| {
            let n = normalize_name(name, version);
            let p = publisher.map(normalize_publisher).unwrap_or_default();
            if !n.is_empty() && !s.names.contains(&(n.clone(), p.clone())) {
                s.names.push((n, p));
            }
        };
        let default_publisher = package.default_locale.publisher.as_deref();
        for i in &package.installers {
            for e in &i.apps_and_features {
                if let Some(n) = &e.display_name {
                    push_name(
                        n,
                        e.publisher.as_deref().or(default_publisher),
                        e.display_version.as_deref(),
                    );
                }
            }
        }
        if let Some(n) = &package.default_locale.package_name {
            push_name(n, default_publisher, Some(&package.version));
        }
        s
    }

    /// How `record` matches, if it does.
    pub fn matches(&self, record: &ArpRecord) -> Option<MatchKind> {
        if record.winget_id.as_deref().is_some_and(|w| w.eq_ignore_ascii_case(&self.id)) {
            return Some(MatchKind::Identifier);
        }
        if self.product_codes.contains(&guid_key(&record.key)) {
            return Some(MatchKind::ProductCode);
        }
        if record.upgrade_code.as_deref().is_some_and(|u| self.upgrade_codes.contains(&guid_key(u)))
        {
            return Some(MatchKind::UpgradeCode);
        }
        let name =
            normalize_name(record.display_name.as_deref()?, record.display_version.as_deref());
        if name.is_empty() {
            return None;
        }
        let publisher = record.publisher.as_deref().map(normalize_publisher).unwrap_or_default();
        self.names
            .iter()
            .any(|(n, p)| *n == name && *p == publisher)
            .then_some(MatchKind::NameAndPublisher)
    }
}

/// The records `signature` matches, strongest match first (then in the
/// records' order).
pub fn find<'r>(
    signature: &Signature,
    records: &'r [ArpRecord],
) -> Vec<(&'r ArpRecord, MatchKind)> {
    let mut out: Vec<(&ArpRecord, MatchKind)> =
        records.iter().filter_map(|r| signature.matches(r).map(|k| (r, k))).collect();
    out.sort_by_key(|&(_, k)| k);
    out
}

/// The package version an installed record is, given the package's
/// known versions (each read package with its AppsAndFeaturesEntries).
pub fn installed_version(record: &ArpRecord, versions: &[&Package]) -> String {
    let Some(dv) = record.display_version.as_deref().map(str::trim).filter(|v| !v.is_empty())
    else {
        return "Unknown".to_string();
    };
    for p in versions {
        let lists = p
            .installers
            .iter()
            .flat_map(|i| i.apps_and_features.iter())
            .any(|e| e.display_version.as_deref().is_some_and(|v| v.trim() == dv));
        if lists {
            return p.version.clone();
        }
    }
    dv.to_string()
}

/// The installer type a record shows (for keeping it on upgrade): an
/// AppsAndFeaturesEntries' InstallerType for the matched entry, else MSI
/// when Windows Installer registered it, else unknown.
pub fn installed_type(record: &ArpRecord, package: &Package) -> Option<InstallerType> {
    let key = guid_key(&record.key);
    for i in &package.installers {
        for e in &i.apps_and_features {
            if e.product_code.as_deref().is_some_and(|p| guid_key(p) == key) {
                return e.installer_type.or(i.installer_type);
            }
        }
        if i.product_code.as_deref().is_some_and(|p| guid_key(p) == key) {
            return i.installer_type;
        }
    }
    record.windows_installer.then_some(InstallerType::Msi)
}

/// Whether `latest` is later than what is installed (winget upgrade).
pub fn update_available(installed: &str, latest: &str) -> bool {
    version::compare(latest, installed) == Ordering::Greater
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "71B2FEF860ABE467217A538FF31DE02F5258807C0129F771846F87BD029AAFC5";

    fn pkg(version: &str, arp: &str) -> Package {
        let text = alloc::format!(
            "PackageIdentifier: Vendor.App\nPackageVersion: {version}\nPackageLocale: en-US\nPublisher: Vendor, Inc.\nPackageName: Vendor App\nLicense: MIT\nShortDescription: d\nManifestType: singleton\nManifestVersion: 1.10.0\nInstallers:\n- Architecture: x64\n  InstallerType: wix\n  InstallerUrl: u\n  InstallerSha256: {SHA}\n  ProductCode: '{{AAAA-1111}}'\n  AppsAndFeaturesEntries:\n{arp}"
        );
        Package::from_texts(&[&text]).unwrap()
    }

    fn rec(key: &str, name: &str, publisher: Option<&str>, dv: Option<&str>) -> ArpRecord {
        ArpRecord {
            key: key.into(),
            display_name: Some(name.into()),
            publisher: publisher.map(Into::into),
            display_version: dv.map(Into::into),
            ..ArpRecord::default()
        }
    }

    #[test]
    fn match_precedence() {
        let p = pkg(
            "2.1",
            "  - DisplayName: Vendor App 2.1 (x64)\n    DisplayVersion: 2.1.0.7\n    UpgradeCode: '{BBBB-2222}'\n    ProductCode: '{CCCC-3333}'\n",
        );
        let s = Signature::of(&p);
        assert_eq!(s.product_codes, ["aaaa-1111", "cccc-3333"]);
        assert_eq!(s.upgrade_codes, ["bbbb-2222"]);
        let by_id = ArpRecord {
            key: "Vendor.App_Microsoft.Winget.Source_8wekyb3d8bbwe".into(),
            winget_id: Some("vendor.app".into()),
            ..ArpRecord::default()
        };
        assert_eq!(s.matches(&by_id), Some(MatchKind::Identifier));
        assert_eq!(s.matches(&rec("{aaaa-1111}", "x", None, None)), Some(MatchKind::ProductCode));
        assert_eq!(s.matches(&rec("CCCC-3333", "x", None, None)), Some(MatchKind::ProductCode));
        let up = ArpRecord {
            key: "{zzz}".into(),
            upgrade_code: Some("{bbbb-2222}".into()),
            ..ArpRecord::default()
        };
        assert_eq!(s.matches(&up), Some(MatchKind::UpgradeCode));
        // Name and publisher, normalized: version, architecture, company
        // suffix and punctuation removed.
        assert_eq!(
            s.matches(&rec("x", "Vendor App 2.0.5 (64-bit)", Some("VENDOR Inc"), Some("2.0.5"))),
            Some(MatchKind::NameAndPublisher)
        );
        assert_eq!(
            s.matches(&rec("x", "Vendor App", Some("Vendor"), None)),
            Some(MatchKind::NameAndPublisher)
        );
        assert_eq!(s.matches(&rec("x", "Vendor App", Some("Other"), None)), None);
        assert_eq!(s.matches(&rec("x", "Vendor App", None, None)), None);
        assert_eq!(s.matches(&rec("x", "Vendor Apps", Some("Vendor"), None)), None);
        let records = [
            rec("x", "Vendor App", Some("Vendor"), None),
            rec("{AAAA-1111}", "y", None, None),
            rec("q", "Other", None, None),
        ];
        let found = find(&s, &records);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].1, MatchKind::ProductCode);
        assert_eq!(found[1].1, MatchKind::NameAndPublisher);
    }

    #[test]
    fn versions_and_types() {
        let v20 = pkg(
            "2.0",
            "  - DisplayVersion: 2.0.0.3\n    ProductCode: '{CCCC-3333}'\n    InstallerType: msi\n",
        );
        let v21 = pkg("2.1", "  - DisplayVersion: 2.1.0.7\n");
        let known = [&v20, &v21];
        assert_eq!(installed_version(&rec("k", "n", None, Some("2.1.0.7")), &known), "2.1");
        assert_eq!(installed_version(&rec("k", "n", None, Some("2.0.0.3 ")), &known), "2.0");
        assert_eq!(installed_version(&rec("k", "n", None, Some("1.9")), &known), "1.9");
        assert_eq!(installed_version(&rec("k", "n", None, None), &known), "Unknown");
        assert!(update_available("2.0", "2.1"));
        assert!(!update_available("2.1", "2.1.0"));
        assert!(update_available("Unknown", "0.1"));
        assert_eq!(
            installed_type(&rec("{cccc-3333}", "n", None, None), &v20),
            Some(InstallerType::Msi)
        );
        assert_eq!(
            installed_type(&rec("{AAAA-1111}", "n", None, None), &v20),
            Some(InstallerType::Wix)
        );
        let msi = ArpRecord { windows_installer: true, ..rec("{other}", "n", None, None) };
        assert_eq!(installed_type(&msi, &v20), Some(InstallerType::Msi));
        assert_eq!(installed_type(&rec("other", "n", None, None), &v20), None);
    }
}
