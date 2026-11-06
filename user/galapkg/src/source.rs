//! A package source laid out as the winget-pkgs repository is:
//! `manifests\<first letter of the identifier, lower case>\<the
//! identifier's dot-separated parts as directories>\<version>\*.yaml`
//! (`manifests\b\BurntSushi\ripgrep\MSVC\15.2.0\`). A version directory
//! holds the YAML files of one version; a package directory may also hold
//! other packages' directories (`ripgrep\MSVC` and `ripgrep\GNU`), which
//! are told apart by holding no `<id>.*yaml` files of their own.
//!
//! A local snapshot (`gala install --source <dir>`) may carry the
//! installers its manifests name under `installers\<the URL's file
//! name>`: an offline mirror, used before any download (scenarios have no
//! network flakiness so; the SHA-256 is checked all the same).

use crate::install::{Error, Host};
use crate::manifest::{Installer, Manifest};
use crate::{contains_ci, eq_ci, version};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Ordering;

/// The identifier winget gives the community repository's source; gala
/// names packages from a winget-pkgs tree (a local snapshot included) as
/// winget does, so paths and registry keys are the ones winget makes.
pub const WINGET_SOURCE_ID: &str = "Microsoft.Winget.Source_8wekyb3d8bbwe";

/// A host's fetcher answers this (ERROR_NOT_SUPPORTED) for a URL scheme
/// it cannot download (gala.exe: anything but http, https and files).
pub const ERROR_NOT_SUPPORTED: u32 = 50;

pub struct Source {
    /// The tree's root (holding `manifests\`), a Windows path (an
    /// `https://` source is not supported yet: [`Source::open`]).
    pub root: String,
}

/// A search hit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub id: String,
    pub name: String,
    pub version: String,
    /// What matched: "Id", "Name", "Moniker" or "Tag: <tag>".
    pub matched: String,
}

impl Source {
    /// A source from `--source`: a directory. URLs are refused with what
    /// is missing (ARCHITECTURE "Installing software": manifests over
    /// HTTPS, ROADMAP PK step 2).
    pub fn open(location: &str) -> Result<Source, Error> {
        let lower = location.to_ascii_lowercase();
        if lower.starts_with("https://") || lower.starts_with("http://") {
            return Err(Error::unsupported(format!(
                "source {location}: reading manifests over HTTP(S) is not built yet (ROADMAP PK step 2: \
                 the GitHub raw and contents URLs); use a local snapshot: --source <dir>"
            )));
        }
        let root = location.trim_end_matches(['\\', '/']).replace('/', "\\");
        Ok(Source { root })
    }

    fn package_dir(&self, id: &str) -> Result<String, Error> {
        let first = id.chars().next().ok_or_else(|| Error::usage("an empty package identifier"))?;
        if id.split('.').any(|p| p.is_empty() || p == "..") || id.contains(['\\', '/', ':']) {
            return Err(Error::usage(format!(
                "{id:?} is not a package identifier (Publisher.Name)"
            )));
        }
        Ok(format!(
            "{}\\manifests\\{}\\{}",
            self.root,
            first.to_ascii_lowercase(),
            id.replace('.', "\\")
        ))
    }

    /// The YAML files of a directory (names).
    fn yaml_files(host: &mut dyn Host, dir: &str) -> Vec<String> {
        host.list_dir(dir)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| !e.is_dir && e.name.to_ascii_lowercase().ends_with(".yaml"))
            .map(|e| e.name)
            .collect()
    }

    /// The versions of package `id` in this source.
    pub fn versions(&self, host: &mut dyn Host, id: &str) -> Result<Vec<String>, Error> {
        let dir = self.package_dir(id)?;
        let entries = host
            .list_dir(&dir)
            .map_err(|_| Error::not_found(format!("no package {id} in source {}", self.root)))?;
        let prefix = format!("{}.", id.to_ascii_lowercase());
        let mut versions = Vec::new();
        for e in entries.into_iter().filter(|e| e.is_dir) {
            let files = Self::yaml_files(host, &format!("{dir}\\{}", e.name));
            if files.iter().any(|f| f.to_ascii_lowercase().starts_with(&prefix)) {
                versions.push(e.name);
            }
        }
        if versions.is_empty() {
            return Err(Error::not_found(format!("no package {id} in source {}", self.root)));
        }
        versions.sort_by(|a, b| version::compare(b, a));
        Ok(versions)
    }

    /// The manifest of one version (its directory's YAML files).
    pub fn manifest(
        &self,
        host: &mut dyn Host,
        id: &str,
        version: &str,
    ) -> Result<Manifest, Error> {
        let dir = format!("{}\\{version}", self.package_dir(id)?);
        let m = Self::read_manifest(host, &dir)?;
        if !eq_ci(&m.id, id) {
            return Err(Error::failed(format!("{dir}: the manifest is for {}, not {id}", m.id)));
        }
        Ok(m)
    }

    fn read_manifest(host: &mut dyn Host, dir: &str) -> Result<Manifest, Error> {
        let mut texts = Vec::new();
        for f in Self::yaml_files(host, dir) {
            let path = format!("{dir}\\{f}");
            let bytes =
                host.read_file(&path).map_err(|e| Error::failed(format!("reading {path}: {e}")))?;
            let text = String::from_utf8(bytes)
                .map_err(|_| Error::failed(format!("{path} is not UTF-8")))?;
            texts.push(text);
        }
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        Manifest::from_documents(&refs).map_err(|e| Error::failed(format!("{dir}: {e}")))
    }

    /// Package `id` at `version` (one that compares equal: `1.2` finds
    /// `1.2.0`), or its latest version.
    pub fn find(
        &self,
        host: &mut dyn Host,
        id: &str,
        version: Option<&str>,
    ) -> Result<Manifest, Error> {
        let versions = self.versions(host, id)?;
        let chosen = match version {
            None => &versions[0],
            Some(want) => versions
                .iter()
                .find(|v| version::compare(v, want) == Ordering::Equal)
                .ok_or_else(|| {
                    Error::not_found(format!(
                        "{id} has no version {want} (it has {})",
                        versions.join(", ")
                    ))
                })?,
        };
        self.manifest(host, id, chosen)
    }

    /// Packages whose identifier, name, moniker or tags contain `query`
    /// (any case), each at its latest version; an exact identifier first.
    pub fn search(&self, host: &mut dyn Host, query: &str) -> Result<Vec<Found>, Error> {
        let mut found = Vec::new();
        let mut dirs = Vec::new();
        let top = format!("{}\\manifests", self.root);
        let letters = host
            .list_dir(&top)
            .map_err(|e| Error::failed(format!("source {}: {top}: {e}", self.root)))?;
        for l in letters.into_iter().filter(|e| e.is_dir) {
            dirs.push(format!("{top}\\{}", l.name));
        }
        // Depth first; a directory with YAML files is a version.
        let mut packages: Vec<(String, Vec<String>)> = Vec::new();
        while let Some(dir) = dirs.pop() {
            let Ok(entries) = host.list_dir(&dir) else { continue };
            for e in entries.into_iter().filter(|e| e.is_dir) {
                let sub = format!("{dir}\\{}", e.name);
                if Self::yaml_files(host, &sub).is_empty() {
                    dirs.push(sub);
                } else if let Some(p) = packages.iter_mut().find(|p| p.0 == dir) {
                    p.1.push(e.name);
                } else {
                    packages.push((dir.clone(), alloc::vec![e.name]));
                }
            }
        }
        for (dir, versions) in packages {
            let Some(latest) = version::latest(versions.iter().map(String::as_str)) else {
                continue;
            };
            let Ok(m) = Self::read_manifest(host, &format!("{dir}\\{latest}")) else {
                host.log(&format!("search: skipped {dir}\\{latest}: its manifest does not parse"));
                continue;
            };
            let matched = if contains_ci(&m.id, query) {
                Some("Id".to_string())
            } else if m.name.as_deref().is_some_and(|n| contains_ci(n, query)) {
                Some("Name".to_string())
            } else if m.moniker.as_deref().is_some_and(|n| contains_ci(n, query)) {
                Some("Moniker".to_string())
            } else {
                m.tags.iter().find(|t| contains_ci(t, query)).map(|t| format!("Tag: {t}"))
            };
            if let Some(matched) = matched {
                found.push(Found {
                    name: m.display_name().to_string(),
                    id: m.id,
                    version: m.version,
                    matched,
                });
            }
        }
        found.sort_by(|a, b| {
            (!eq_ci(&a.id, query), a.id.to_ascii_lowercase())
                .cmp(&(!eq_ci(&b.id, query), b.id.to_ascii_lowercase()))
        });
        Ok(found)
    }

    /// The installer's bytes: from the snapshot's `installers\` mirror,
    /// else downloaded (the host's fetcher).
    pub fn installer_bytes(
        &self,
        host: &mut dyn Host,
        installer: &Installer,
    ) -> Result<Vec<u8>, Error> {
        let mirrored = format!("{}\\installers\\{}", self.root, installer.file_name());
        if host.exists(&mirrored) == Some(false) {
            host.log(&format!("download from-mirror path={mirrored}"));
            return host
                .read_file(&mirrored)
                .map_err(|e| Error::failed(format!("reading {mirrored}: {e}")));
        }
        host.log(&format!("download url={}", installer.url));
        let fetched = host.fetch(&installer.url);
        match &fetched {
            Ok(data) => host.log(&format!("download bytes={}", data.len())),
            Err(e) => host.log(&format!("error download code={} ({e})", e.code)),
        }
        fetched.map_err(|e| {
            if e.code == ERROR_NOT_SUPPORTED {
                Error::unsupported(format!(
                    "downloading {}: this URL's scheme cannot be downloaded (http, https and \
                     file:/// can); put the file in the source's installers\\ directory",
                    installer.url
                ))
            } else {
                Error::failed(format!("downloading {}: {e}", installer.url))
            }
        })
    }
}
