//! winget manifests, schema 1.x (the winget-cli repository's
//! `doc/ManifestSpecv1.x.md` and the JSON schemas it publishes): a
//! package version is one `singleton` manifest, or several files: a
//! `version` manifest (PackageIdentifier, PackageVersion, DefaultLocale),
//! an `installer` manifest and `defaultLocale`/`locale` manifests.
//!
//! What is read: identity (PackageIdentifier, PackageVersion), the
//! default locale's PackageName, Publisher, Moniker, Tags,
//! ShortDescription, License and PackageUrl, and the installers:
//! Architecture, InstallerType, InstallerUrl, InstallerSha256, Scope,
//! NestedInstallerType, NestedInstallerFiles (RelativeFilePath,
//! PortableCommandAlias), Commands, InstallerSwitches, ProductCode,
//! ArchiveBinariesDependOnPath. Installer fields given at the root are
//! every installer's default, as the schema says.

use crate::yaml::{self, Value};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// A file inside a zip that is the real installer (NestedInstallerFiles).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NestedFile {
    /// The path inside the archive (`/` or `\` separated).
    pub relative_path: String,
    /// The command name it is linked as (PortableCommandAlias), if any.
    pub alias: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installer {
    /// x86, x64, arm, arm64 or neutral.
    pub architecture: String,
    /// msix, msi, appx, exe, zip, inno, nullsoft, wix, burn, pwa, portable.
    pub installer_type: String,
    pub nested_installer_type: Option<String>,
    pub nested_files: Vec<NestedFile>,
    pub url: String,
    /// Upper-case hex.
    pub sha256: String,
    /// user or machine (None: either).
    pub scope: Option<String>,
    pub commands: Vec<String>,
    /// InstallerSwitches as (name, value): Silent, SilentWithProgress,
    /// Custom, Log, InstallLocation...
    pub switches: Vec<(String, String)>,
    pub product_code: Option<String>,
    pub archive_binaries_depend_on_path: bool,
}

impl Installer {
    /// The type that is really installed: a zip's NestedInstallerType.
    pub fn effective_type(&self) -> &str {
        if self.installer_type == "zip" {
            self.nested_installer_type.as_deref().unwrap_or("zip")
        } else {
            &self.installer_type
        }
    }

    /// The installer file's name: the last part of its URL, without a
    /// query or fragment.
    pub fn file_name(&self) -> &str {
        let url = self.url.split(['?', '#']).next().unwrap_or("");
        url.rsplit(['/', '\\']).next().unwrap_or(url)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Manifest {
    pub id: String,
    pub version: String,
    pub manifest_version: String,
    pub name: Option<String>,
    pub publisher: Option<String>,
    pub moniker: Option<String>,
    pub tags: Vec<String>,
    pub short_description: Option<String>,
    pub license: Option<String>,
    pub package_url: Option<String>,
    pub installers: Vec<Installer>,
}

/// Why manifests could not be read; the text names the file's problem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.map(|v| v.items().iter().filter_map(Value::as_str).map(ToString::to_string).collect())
        .unwrap_or_default()
}

fn opt(v: &Value, key: &str) -> Option<String> {
    v.str(key).map(ToString::to_string)
}

/// An installer from its node, with the manifest root's installer
/// fields as defaults.
fn installer(node: &Value, root: &Value, index: usize) -> Result<Installer, Error> {
    let field = |key: &str| node.get(key).filter(|v| **v != Value::Null).or_else(|| root.get(key));
    let text = |key: &str| {
        field(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(ToString::to_string)
    };
    let need = |key: &str| text(key).ok_or_else(|| Error(format!("installer {index}: no {key}")));
    let sha256 = need("InstallerSha256")?.to_ascii_uppercase();
    if sha256.len() != 64 || !sha256.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error(format!("installer {index}: InstallerSha256 is not 64 hex digits")));
    }
    let nested_files = field("NestedInstallerFiles")
        .map(|v| {
            v.items()
                .iter()
                .map(|f| {
                    Ok(NestedFile {
                        relative_path: f.str("RelativeFilePath").map(ToString::to_string).ok_or_else(|| {
                            Error(format!("installer {index}: a NestedInstallerFiles entry without RelativeFilePath"))
                        })?,
                        alias: opt(f, "PortableCommandAlias"),
                    })
                })
                .collect::<Result<Vec<_>, Error>>()
        })
        .transpose()?
        .unwrap_or_default();
    let switches = match field("InstallerSwitches") {
        Some(Value::Map(entries)) => entries
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect(),
        _ => Vec::new(),
    };
    Ok(Installer {
        architecture: need("Architecture")?.to_ascii_lowercase(),
        installer_type: need("InstallerType")?.to_ascii_lowercase(),
        nested_installer_type: text("NestedInstallerType").map(|s| s.to_ascii_lowercase()),
        nested_files,
        url: need("InstallerUrl")?,
        sha256,
        scope: text("Scope").map(|s| s.to_ascii_lowercase()),
        commands: strings(field("Commands")),
        switches,
        product_code: text("ProductCode"),
        archive_binaries_depend_on_path: text("ArchiveBinariesDependOnPath")
            .is_some_and(|s| s.eq_ignore_ascii_case("true")),
    })
}

impl Manifest {
    /// One package version from its manifest files' texts (one singleton
    /// manifest, or the multi-file set in any order).
    pub fn from_documents(texts: &[&str]) -> Result<Manifest, Error> {
        let mut m = Manifest::default();
        let mut have_installer = false;
        let mut default_locale: Option<String> = None;
        let mut locales: Vec<Value> = Vec::new();
        for (i, text) in texts.iter().enumerate() {
            let doc = yaml::parse(text).map_err(|e| Error(format!("manifest {}: {e}", i + 1)))?;
            let kind = doc.str("ManifestType").unwrap_or("").to_ascii_lowercase();
            let id = doc
                .str("PackageIdentifier")
                .ok_or_else(|| Error(format!("manifest {}: no PackageIdentifier", i + 1)))?;
            let version = doc
                .str("PackageVersion")
                .ok_or_else(|| Error(format!("manifest {}: no PackageVersion", i + 1)))?;
            let mv = doc.str("ManifestVersion").unwrap_or("");
            if !mv.starts_with("1.") {
                return Err(Error(format!("{id} {version}: ManifestVersion {mv:?} is not 1.x")));
            }
            if m.id.is_empty() {
                m.id = id.to_string();
                m.version = version.to_string();
                m.manifest_version = mv.to_string();
            } else if m.id != id || m.version != version {
                return Err(Error(format!(
                    "manifests of different packages: {} {} and {id} {version}",
                    m.id, m.version
                )));
            }
            match kind.as_str() {
                "singleton" => {
                    have_installer = true;
                    m.add_installers(&doc)?;
                    locales.push(doc);
                }
                "version" => default_locale = opt(&doc, "DefaultLocale"),
                "installer" => {
                    have_installer = true;
                    m.add_installers(&doc)?;
                }
                "defaultlocale" | "locale" => locales.push(doc),
                _ => return Err(Error(format!("{id} {version}: unknown ManifestType {kind:?}"))),
            }
        }
        if m.id.is_empty() {
            return Err(Error("no manifest".to_string()));
        }
        if !have_installer || m.installers.is_empty() {
            return Err(Error(format!("{} {}: no installers", m.id, m.version)));
        }
        // The default locale's texts: the singleton, the defaultLocale
        // manifest, or the locale DefaultLocale names.
        let locale = locales
            .iter()
            .find(|d| {
                d.str("ManifestType").is_some_and(|t| {
                    t.eq_ignore_ascii_case("singleton") || t.eq_ignore_ascii_case("defaultLocale")
                })
            })
            .or_else(|| {
                let want = default_locale.as_deref()?;
                locales
                    .iter()
                    .find(|d| d.str("PackageLocale").is_some_and(|l| l.eq_ignore_ascii_case(want)))
            });
        if let Some(l) = locale {
            m.name = opt(l, "PackageName");
            m.publisher = opt(l, "Publisher");
            m.moniker = opt(l, "Moniker");
            m.tags = strings(l.get("Tags"));
            m.short_description = opt(l, "ShortDescription").map(|s| s.trim().to_string());
            m.license = opt(l, "License");
            m.package_url = opt(l, "PackageUrl");
        }
        Ok(m)
    }

    fn add_installers(&mut self, doc: &Value) -> Result<(), Error> {
        for (i, node) in doc.get("Installers").map(Value::items).unwrap_or(&[]).iter().enumerate() {
            self.installers.push(installer(node, doc, i + 1)?);
        }
        Ok(())
    }

    /// The name to show: PackageName, else the identifier.
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    /// The installer gala would use on an x64 machine for a user-scope
    /// install: architecture x64, else neutral (gala runs no 32-bit or
    /// ARM code); scope user or unspecified before machine; a type gala
    /// installs (`supported`) before one it does not. None when no
    /// installer has a usable architecture.
    pub fn select_installer(&self, supported: impl Fn(&Installer) -> bool) -> Option<&Installer> {
        self.installers
            .iter()
            .filter(|i| i.architecture == "x64" || i.architecture == "neutral")
            .min_by_key(|i| {
                (!supported(i), i.architecture != "x64", i.scope.as_deref() == Some("machine"))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const RG: [&str; 3] = [
        include_str!(
            "../../pk_install_test/source/manifests/b/BurntSushi/ripgrep/MSVC/15.2.0/BurntSushi.ripgrep.MSVC.yaml"
        ),
        include_str!(
            "../../pk_install_test/source/manifests/b/BurntSushi/ripgrep/MSVC/15.2.0/BurntSushi.ripgrep.MSVC.installer.yaml"
        ),
        include_str!(
            "../../pk_install_test/source/manifests/b/BurntSushi/ripgrep/MSVC/15.2.0/BurntSushi.ripgrep.MSVC.locale.en-US.yaml"
        ),
    ];

    #[test]
    fn ripgrep_multi_file() {
        let m = Manifest::from_documents(&RG).unwrap();
        assert_eq!(m.id, "BurntSushi.ripgrep.MSVC");
        assert_eq!(m.version, "15.2.0");
        assert_eq!(m.manifest_version, "1.12.0");
        assert_eq!(m.name.as_deref(), Some("RipGrep MSVC"));
        assert_eq!(m.publisher.as_deref(), Some("BurntSushi"));
        assert_eq!(m.moniker.as_deref(), Some("rg"));
        assert!(m.tags.iter().any(|t| t == "ripgrep"));
        assert_eq!(m.installers.len(), 3);
        let i = m.select_installer(|_| true).unwrap();
        assert_eq!(i.architecture, "x64");
        assert_eq!(i.installer_type, "zip");
        assert_eq!(i.effective_type(), "portable");
        assert_eq!(
            i.nested_files,
            vec![NestedFile {
                relative_path: "ripgrep-15.2.0-x86_64-pc-windows-msvc/rg.exe".into(),
                alias: Some("rg".into()),
            }]
        );
        assert_eq!(i.sha256, "71B2FEF860ABE467217A538FF31DE02F5258807C0129F771846F87BD029AAFC5");
        assert_eq!(i.file_name(), "ripgrep-15.2.0-x86_64-pc-windows-msvc.zip");
        // The order of the files does not matter.
        let reordered = Manifest::from_documents(&[RG[2], RG[1], RG[0]]).unwrap();
        assert_eq!(reordered, m);
    }

    #[test]
    fn jq_portable() {
        let m = Manifest::from_documents(&[
            include_str!("../../pk_install_test/source/manifests/j/jqlang/jq/1.8.2/jqlang.jq.yaml"),
            include_str!("../../pk_install_test/source/manifests/j/jqlang/jq/1.8.2/jqlang.jq.installer.yaml"),
            include_str!("../../pk_install_test/source/manifests/j/jqlang/jq/1.8.2/jqlang.jq.locale.en-US.yaml"),
        ])
        .unwrap();
        let i = m.select_installer(|_| true).unwrap();
        assert_eq!(i.installer_type, "portable");
        assert_eq!(i.commands, vec!["jq".to_string()]);
        assert_eq!(i.file_name(), "jq-windows-amd64.exe");
        assert_eq!(
            m.short_description.as_deref(),
            Some("jq is a lightweight and flexible command-line JSON processor.")
        );
    }

    #[test]
    fn singleton_with_root_defaults() {
        let m = Manifest::from_documents(&[include_str!(
            "../../pk_install_test/source/manifests/g/GalaTest/PkHello/1.0.0/GalaTest.PkHello.yaml"
        )])
        .unwrap();
        assert_eq!(m.id, "GalaTest.PkHello");
        assert_eq!(m.name.as_deref(), Some("PkHello"));
        let i = m.select_installer(|_| true).unwrap();
        assert_eq!(i.installer_type, "zip");
        assert_eq!(i.nested_installer_type.as_deref(), Some("portable"));
        assert_eq!(i.nested_files[0].alias.as_deref(), Some("pkhello"));
    }

    #[test]
    fn selection_prefers_x64_user_supported() {
        let doc = "PackageIdentifier: A.B\nPackageVersion: 1\nManifestType: singleton\nManifestVersion: 1.6.0\nInstallerSha256: 0000000000000000000000000000000000000000000000000000000000000000\nInstallers:\n- Architecture: x86\n  InstallerType: zip\n  InstallerUrl: https://h/x86.zip\n- Architecture: x64\n  InstallerType: msi\n  InstallerUrl: https://h/x64.msi\n- Architecture: x64\n  InstallerType: zip\n  Scope: machine\n  InstallerUrl: https://h/m.zip\n- Architecture: x64\n  InstallerType: zip\n  Scope: user\n  InstallerUrl: https://h/u.zip\n";
        let m = Manifest::from_documents(&[doc]).unwrap();
        let zip_only = |i: &Installer| i.installer_type == "zip";
        assert_eq!(m.select_installer(zip_only).unwrap().url, "https://h/u.zip");
        let none = |_: &Installer| false;
        assert_eq!(m.select_installer(none).unwrap().url, "https://h/x64.msi");
        let arm = doc.replace("x64", "arm64");
        let m = Manifest::from_documents(&[&arm]).unwrap();
        assert!(m.select_installer(zip_only).is_none());
    }

    #[test]
    fn errors() {
        let bad_hash = "PackageIdentifier: A.B\nPackageVersion: 1\nManifestType: singleton\nManifestVersion: 1.6.0\nInstallers:\n- Architecture: x64\n  InstallerType: zip\n  InstallerUrl: u\n  InstallerSha256: 12\n";
        assert!(Manifest::from_documents(&[bad_hash]).unwrap_err().0.contains("64 hex"));
        let v2 = bad_hash.replace("1.6.0", "2.0.0");
        assert!(Manifest::from_documents(&[&v2]).unwrap_err().0.contains("not 1.x"));
        assert!(Manifest::from_documents(&[RG[0]]).unwrap_err().0.contains("no installers"));
        let other = RG[1].replace("15.2.0", "15.1.0");
        assert!(
            Manifest::from_documents(&[RG[0], &other])
                .unwrap_err()
                .0
                .contains("different packages")
        );
    }
}
