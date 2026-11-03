//! What each published manifest schema allows: the manifest types, the
//! ManifestVersion values, and in which schema version each field and
//! each enumeration value appeared.
//!
//! Source (specification data, not code): the JSON schemas winget-cli
//! publishes under `schemas/JSON/manifests/v<version>/manifest.<type>.
//! <version>.json` (github.com/microsoft/winget-cli, MIT; THIRD_PARTY.md),
//! read for versions 1.0.0, 1.1.0, 1.2.0, 1.4.0, 1.5.0, 1.6.0, 1.7.0,
//! 1.9.0, 1.10.0, 1.12.0 and 1.28.0 (no 1.3, 1.8, 1.11 or 1.13-1.27 were
//! published). The tables below are the differences between consecutive
//! schemas; the singleton schema is the union of the installer and
//! defaultLocale schemas in every version.
//!
//! How the reader uses them (gala's rule; winget's client is documented
//! only as "validating" manifests): a field or value newer than the
//! manifest's declared ManifestVersion, or one no schema knows, is kept
//! out of the result's data only where its meaning is unknown, and always
//! reported as a [`super::package::Warning`] with its position, so a
//! manifest the community repository's validation would reject still
//! reads, and the warning says why it is suspect.

use core::cmp::Ordering;
use core::fmt;

/// A ManifestVersion: `major.minor.patch`, each 0..=65535 (the schema's
/// pattern).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SchemaVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl SchemaVersion {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        SchemaVersion { major, minor, patch }
    }

    /// `1.10.0`: exactly three parts of decimal digits without leading
    /// zeros (as the schema's `ManifestVersion` pattern requires).
    pub fn parse(s: &str) -> Option<SchemaVersion> {
        let mut parts = s.trim().split('.');
        let mut next = || -> Option<u16> {
            let p = parts.next()?;
            if p.is_empty()
                || !p.bytes().all(|c| c.is_ascii_digit())
                || (p.len() > 1 && p.starts_with('0'))
            {
                return None;
            }
            p.parse().ok()
        };
        let v = SchemaVersion { major: next()?, minor: next()?, patch: next()? };
        if parts.next().is_some() { None } else { Some(v) }
    }

    /// Whether a published schema has this version (major.minor; the
    /// patch is not checked: every published patch is 0).
    pub fn is_published(self) -> bool {
        self.major == 1 && PUBLISHED.contains(&self.minor)
    }
}

impl fmt::Display for SchemaVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The minor versions of the published 1.x schemas.
pub const PUBLISHED: [u16; 11] = [0, 1, 2, 4, 5, 6, 7, 9, 10, 12, 28];

/// The newest schema these tables describe.
pub const LATEST: SchemaVersion = SchemaVersion::new(1, 28, 0);

/// The five manifest types (ManifestType's values; `singleton` is the
/// one-file form, the others the multi-file form).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestType {
    Singleton,
    Version,
    Installer,
    DefaultLocale,
    Locale,
}

impl ManifestType {
    /// ManifestType's value (exact case, as the schemas' `const`; any
    /// case is accepted with a warning by the reader).
    pub fn parse(s: &str) -> Option<ManifestType> {
        Some(match s {
            "singleton" => ManifestType::Singleton,
            "version" => ManifestType::Version,
            "installer" => ManifestType::Installer,
            "defaultLocale" => ManifestType::DefaultLocale,
            "locale" => ManifestType::Locale,
            _ => return None,
        })
    }

    pub fn parse_any_case(s: &str) -> Option<ManifestType> {
        ["singleton", "version", "installer", "defaultLocale", "locale"]
            .into_iter()
            .find(|t| t.eq_ignore_ascii_case(s))
            .and_then(ManifestType::parse)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ManifestType::Singleton => "singleton",
            ManifestType::Version => "version",
            ManifestType::Installer => "installer",
            ManifestType::DefaultLocale => "defaultLocale",
            ManifestType::Locale => "locale",
        }
    }
}

/// Where a field may be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// The manifest's top level.
    Root,
    /// An entry of `Installers`.
    Installer,
}

/// A field: its name, the schema minor version it appeared in, and where
/// it may be written (root, installer, or both).
pub struct Field {
    pub name: &'static str,
    pub since: u16,
    pub root: bool,
    pub installer: bool,
}

const fn f(name: &'static str, since: u16, root: bool, installer: bool) -> Field {
    Field { name, since, root, installer }
}

/// The identity fields every manifest type has.
pub const IDENTITY: &[Field] = &[
    f("PackageIdentifier", 0, true, false),
    f("PackageVersion", 0, true, false),
    f("ManifestType", 0, true, false),
    f("ManifestVersion", 0, true, false),
];

/// The version manifest's own field.
pub const VERSION_FIELDS: &[Field] = &[f("DefaultLocale", 0, true, false)];

/// The installer manifest's fields (root and/or per installer).
pub const INSTALLER_FIELDS: &[Field] = &[
    // 1.0.0
    f("Channel", 0, true, false),
    f("Installers", 0, true, false),
    f("Architecture", 0, false, true),
    f("InstallerUrl", 0, false, true),
    f("InstallerSha256", 0, false, true),
    f("SignatureSha256", 0, false, true),
    f("InstallerLocale", 0, true, true),
    f("Platform", 0, true, true),
    f("MinimumOSVersion", 0, true, true),
    f("InstallerType", 0, true, true),
    f("Scope", 0, true, true),
    f("InstallModes", 0, true, true),
    f("InstallerSwitches", 0, true, true),
    f("InstallerSuccessCodes", 0, true, true),
    f("UpgradeBehavior", 0, true, true),
    f("Commands", 0, true, true),
    f("Protocols", 0, true, true),
    f("FileExtensions", 0, true, true),
    f("Dependencies", 0, true, true),
    f("PackageFamilyName", 0, true, true),
    f("ProductCode", 0, true, true),
    f("Capabilities", 0, true, true),
    f("RestrictedCapabilities", 0, true, true),
    // 1.1.0
    f("AppsAndFeaturesEntries", 1, true, true),
    f("ElevationRequirement", 1, true, true),
    f("ExpectedReturnCodes", 1, true, true),
    f("InstallLocationRequired", 1, true, true),
    f("InstallerAbortsTerminal", 1, true, true),
    f("Markets", 1, true, true),
    f("ReleaseDate", 1, true, true),
    f("RequireExplicitUpgrade", 1, true, true),
    f("UnsupportedOSArchitectures", 1, true, true),
    // 1.2.0
    f("DisplayInstallWarnings", 2, true, true),
    f("UnsupportedArguments", 2, true, true),
    // 1.4.0
    f("InstallationMetadata", 4, true, true),
    f("NestedInstallerFiles", 4, true, true),
    f("NestedInstallerType", 4, true, true),
    // 1.6.0
    f("DownloadCommandProhibited", 6, true, true),
    // 1.7.0
    f("RepairBehavior", 7, true, true),
    // 1.9.0
    f("ArchiveBinariesDependOnPath", 9, true, true),
    // 1.10.0
    f("Authentication", 10, true, true),
    // 1.28.0
    f("DesiredStateConfiguration", 28, true, true),
];

/// The locale manifests' fields (defaultLocale; `locale` has all but
/// Moniker).
pub const LOCALE_FIELDS: &[Field] = &[
    f("PackageLocale", 0, true, false),
    f("Publisher", 0, true, false),
    f("PublisherUrl", 0, true, false),
    f("PublisherSupportUrl", 0, true, false),
    f("PrivacyUrl", 0, true, false),
    f("Author", 0, true, false),
    f("PackageName", 0, true, false),
    f("PackageUrl", 0, true, false),
    f("License", 0, true, false),
    f("LicenseUrl", 0, true, false),
    f("Copyright", 0, true, false),
    f("CopyrightUrl", 0, true, false),
    f("ShortDescription", 0, true, false),
    f("Description", 0, true, false),
    f("Moniker", 0, true, false),
    f("Tags", 0, true, false),
    // 1.1.0
    f("Agreements", 1, true, false),
    f("ReleaseNotes", 1, true, false),
    f("ReleaseNotesUrl", 1, true, false),
    // 1.2.0
    f("PurchaseUrl", 2, true, false),
    f("InstallationNotes", 2, true, false),
    f("Documentations", 2, true, false),
    // 1.5.0
    f("Icons", 5, true, false),
];

/// The field `name` (exact case) allowed for `kind` at `level`, if any.
pub fn field(kind: ManifestType, level: Level, name: &str) -> Option<&'static Field> {
    let ok = |x: &&Field| {
        !x.name.is_empty()
            && x.name == name
            && match level {
                Level::Root => x.root,
                Level::Installer => x.installer,
            }
    };
    let identity = || IDENTITY.iter().find(ok);
    let installer = || INSTALLER_FIELDS.iter().find(ok);
    let locale = || {
        LOCALE_FIELDS
            .iter()
            .find(ok)
            .filter(|x| !(kind == ManifestType::Locale && x.name == "Moniker"))
    };
    match (kind, level) {
        (_, Level::Root) if identity().is_some() => identity(),
        (ManifestType::Version, Level::Root) => VERSION_FIELDS.iter().find(ok),
        (ManifestType::Installer, _) => installer(),
        (ManifestType::DefaultLocale | ManifestType::Locale, Level::Root) => locale(),
        (ManifestType::Singleton, Level::Root) => installer().or_else(locale),
        (ManifestType::Singleton, Level::Installer) => installer(),
        _ => None,
    }
}

/// An enumeration value and the schema minor version it appeared in.
pub type EnumValues = &'static [(&'static str, u16)];

pub const INSTALLER_TYPES: EnumValues = &[
    ("msix", 0),
    ("msi", 0),
    ("appx", 0),
    ("exe", 0),
    ("inno", 0),
    ("nullsoft", 0),
    ("wix", 0),
    ("burn", 0),
    ("pwa", 0),
    ("portable", 2),
    ("zip", 4),
    ("font", 12),
];
/// NestedInstallerType: InstallerType without zip and pwa.
pub const NESTED_INSTALLER_TYPES: EnumValues = &[
    ("msix", 4),
    ("msi", 4),
    ("appx", 4),
    ("exe", 4),
    ("inno", 4),
    ("nullsoft", 4),
    ("wix", 4),
    ("burn", 4),
    ("portable", 4),
    ("font", 12),
];
pub const ARCHITECTURES: EnumValues =
    &[("x86", 0), ("x64", 0), ("arm", 0), ("arm64", 0), ("neutral", 0)];
pub const UNSUPPORTED_OS_ARCHITECTURES: EnumValues =
    &[("x86", 1), ("x64", 1), ("arm", 1), ("arm64", 1)];
pub const SCOPES: EnumValues = &[("user", 0), ("machine", 0)];
pub const PLATFORMS: EnumValues = &[("Windows.Desktop", 0), ("Windows.Universal", 0)];
pub const INSTALL_MODES: EnumValues =
    &[("interactive", 0), ("silent", 0), ("silentWithProgress", 0)];
pub const UPGRADE_BEHAVIORS: EnumValues = &[("install", 0), ("uninstallPrevious", 0), ("deny", 6)];
pub const ELEVATION_REQUIREMENTS: EnumValues =
    &[("elevationRequired", 1), ("elevationProhibited", 1), ("elevatesSelf", 1)];
pub const REPAIR_BEHAVIORS: EnumValues = &[("modify", 7), ("uninstaller", 7), ("installer", 7)];
pub const UNSUPPORTED_ARGUMENTS: EnumValues = &[("log", 2), ("location", 2)];
/// InstallerSwitches' keys (`Repair` from 1.7.0).
pub const SWITCHES: EnumValues = &[
    ("Silent", 0),
    ("SilentWithProgress", 0),
    ("Interactive", 0),
    ("InstallLocation", 0),
    ("Log", 0),
    ("Upgrade", 0),
    ("Custom", 0),
    ("Repair", 7),
];
/// ExpectedReturnCodes' ReturnResponse values (15 in 1.1.0, `custom` in
/// 1.2.0, three more in 1.4.0).
pub const RETURN_RESPONSES: EnumValues = &[
    ("packageInUse", 1),
    ("installInProgress", 1),
    ("fileInUse", 1),
    ("missingDependency", 1),
    ("diskFull", 1),
    ("insufficientMemory", 1),
    ("noNetwork", 1),
    ("contactSupport", 1),
    ("rebootRequiredToFinish", 1),
    ("rebootRequiredForInstall", 1),
    ("rebootInitiated", 1),
    ("cancelledByUser", 1),
    ("alreadyInstalled", 1),
    ("downgrade", 1),
    ("blockedByPolicy", 1),
    ("custom", 2),
    ("packageInUseByApplication", 4),
    ("invalidParameter", 4),
    ("systemNotSupported", 4),
];

/// How a value matched an enumeration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnumMatch {
    /// The canonical spelling, allowed in the declared version.
    Ok(&'static str),
    /// A known value written in another case (winget's reader compares
    /// enumeration values without regard to case; the schema does not).
    OtherCase(&'static str),
    /// A known value newer than the declared schema.
    Newer(&'static str, u16),
    Unknown,
}

/// Looks `value` up in `values` for a manifest of schema `declared`.
pub fn enum_value(values: EnumValues, value: &str, declared: SchemaVersion) -> EnumMatch {
    let found =
        values.iter().find(|(v, _)| *v == value).map(|x| (x, false)).or_else(|| {
            values.iter().find(|(v, _)| v.eq_ignore_ascii_case(value)).map(|x| (x, true))
        });
    match found {
        None => EnumMatch::Unknown,
        Some(((v, since), _)) if declared.major == 1 && declared.minor < *since => {
            EnumMatch::Newer(v, *since)
        }
        Some(((v, _), false)) => EnumMatch::Ok(v),
        Some(((v, _), true)) => EnumMatch::OtherCase(v),
    }
}

/// Orders schema versions (for "newer than declared" checks).
pub fn newer(since_minor: u16, declared: SchemaVersion) -> bool {
    declared.major == 1 && SchemaVersion::new(1, since_minor, 0).cmp(&declared) == Ordering::Greater
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_versions() {
        assert_eq!(SchemaVersion::parse("1.9.0"), Some(SchemaVersion::new(1, 9, 0)));
        assert_eq!(SchemaVersion::parse(" 1.10.0 "), Some(SchemaVersion::new(1, 10, 0)));
        assert_eq!(SchemaVersion::parse("1.9"), None);
        assert_eq!(SchemaVersion::parse("1.09.0"), None);
        assert_eq!(SchemaVersion::parse("1.9.0.0"), None);
        assert_eq!(SchemaVersion::parse("1.70000.0"), None);
        assert!(SchemaVersion::new(1, 10, 0) > SchemaVersion::new(1, 9, 0));
        assert!(SchemaVersion::new(1, 28, 0).is_published());
        assert!(!SchemaVersion::new(1, 3, 0).is_published());
        assert!(!SchemaVersion::new(2, 0, 0).is_published());
    }

    #[test]
    fn fields_by_version_and_level() {
        let v = |k, l, n| field(k, l, n).map(|f| f.since);
        use Level::{Installer as In, Root};
        use ManifestType::{DefaultLocale, Installer, Locale, Singleton, Version};
        assert_eq!(v(Installer, Root, "InstallerType"), Some(0));
        assert_eq!(v(Installer, Root, "InstallerUrl"), None);
        assert_eq!(v(Installer, In, "InstallerUrl"), Some(0));
        assert_eq!(v(Installer, In, "Channel"), None);
        assert_eq!(v(Installer, In, "NestedInstallerFiles"), Some(4));
        assert_eq!(v(Installer, Root, "ArchiveBinariesDependOnPath"), Some(9));
        assert_eq!(v(Singleton, Root, "Moniker"), Some(0));
        assert_eq!(v(Singleton, Root, "Icons"), Some(5));
        assert_eq!(v(Locale, Root, "Moniker"), None);
        assert_eq!(v(DefaultLocale, Root, "Moniker"), Some(0));
        assert_eq!(v(Version, Root, "DefaultLocale"), Some(0));
        assert_eq!(v(Version, Root, "Installers"), None);
        assert_eq!(v(Locale, Root, "PackageIdentifier"), Some(0));
        assert_eq!(v(Installer, Root, ""), None);
    }

    #[test]
    fn enumerations_by_version() {
        let v1_0 = SchemaVersion::new(1, 0, 0);
        let v1_6 = SchemaVersion::new(1, 6, 0);
        assert_eq!(enum_value(INSTALLER_TYPES, "zip", v1_0), EnumMatch::Newer("zip", 4));
        assert_eq!(enum_value(INSTALLER_TYPES, "zip", v1_6), EnumMatch::Ok("zip"));
        assert_eq!(enum_value(INSTALLER_TYPES, "Nullsoft", v1_6), EnumMatch::OtherCase("nullsoft"));
        assert_eq!(enum_value(INSTALLER_TYPES, "msstore", v1_6), EnumMatch::Unknown);
        assert_eq!(
            enum_value(UPGRADE_BEHAVIORS, "deny", SchemaVersion::new(1, 5, 0)),
            EnumMatch::Newer("deny", 6)
        );
        assert_eq!(enum_value(SWITCHES, "Repair", v1_6), EnumMatch::Newer("Repair", 7));
        assert!(newer(4, v1_0));
        assert!(!newer(4, v1_6));
    }
}
