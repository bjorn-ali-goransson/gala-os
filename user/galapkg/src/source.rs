//! A package source laid out as the winget-pkgs repository is:
//! `manifests\<first letter of the identifier, lower case>\<the
//! identifier's dot-separated parts as directories>\<version>\*.yaml`
//! (`manifests\b\BurntSushi\ripgrep\MSVC\15.2.0\`). A version directory
//! holds the YAML files of one version; a package directory may also hold
//! other packages' directories (`ripgrep\MSVC` and `ripgrep\GNU`), which
//! are told apart by holding no `<id>.*yaml` files of their own.
//!
//! Where the tree is ([`Source::open`]):
//! - a directory (`--source C:\pk_source`): a local snapshot, which may
//!   carry the installers its manifests name under `installers\<the URL's
//!   file name>`: an offline mirror, used before any download (scenarios
//!   have no network flakiness so; the SHA-256 is checked all the same);
//! - a GitHub repository (`https://github.com/<owner>/<repo>`, optionally
//!   `/tree/<branch>`; the default source is
//!   `https://github.com/microsoft/winget-pkgs`, branch `master`):
//!   directories listed through GitHub's REST API ("Get repository
//!   content": `https://api.github.com/repos/<owner>/<repo>/contents/<path>?ref=<branch>`,
//!   a JSON array of entries with `name` and `type`), files read from
//!   `https://raw.githubusercontent.com/<owner>/<repo>/<branch>/<path>`;
//! - any other `http(s)://` URL: a winget-pkgs tree served as files, each
//!   directory's listing at `<url>/<path>/` as JSON (Nginx's
//!   `autoindex_format json`), each file at `<url>/<path>`.
//!
//! Every listing and file read over HTTP(S) is a line of the install log
//! (`source list url=`, `source read url=`), and goes through the host's
//! fetcher (gala.exe: winhttp over Schannel). Walking the whole tree
//! (`search`, `list`'s matching) is refused for GitHub: winget reads its
//! source index instead (ROADMAP PK step 2).

use crate::install::{DirEntry, Error, HTTP_STATUS_ERROR, Host, HostError};
use crate::winget::{Installer, Package};
use crate::{contains_ci, eq_ci, json, version};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Ordering;

/// The identifier winget gives the community repository's source; gala
/// names packages from a winget-pkgs tree (a local snapshot included) as
/// winget does, so paths and registry keys are the ones winget makes.
pub const WINGET_SOURCE_ID: &str = "Microsoft.Winget.Source_8wekyb3d8bbwe";

/// The default source: winget's community repository.
pub const DEFAULT_SOURCE: &str = "https://github.com/microsoft/winget-pkgs";

/// A host's fetcher answers this (ERROR_NOT_SUPPORTED) for a URL scheme
/// it cannot download (gala.exe: anything but http, https and files).
pub const ERROR_NOT_SUPPORTED: u32 = 50;

/// Where the tree is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Location {
    /// A directory (a Windows path, no trailing `\`).
    Dir(String),
    /// A GitHub repository at a branch.
    GitHub { owner: String, repo: String, branch: String },
    /// A tree served over HTTP(S) (the URL, no trailing `/`).
    Http(String),
}

#[derive(Debug)]
pub struct Source {
    /// The location as given, normalized (no trailing separator): what
    /// the ARP entry keeps for `gala upgrade`.
    pub root: String,
    pub location: Location,
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

/// `a\b\c` as `a/b/c`.
fn url_path(rel: &str) -> String {
    rel.replace('\\', "/")
}

impl Source {
    /// A source from `--source`: a directory, a GitHub repository or a
    /// tree served over HTTP(S) (see the module text).
    pub fn open(location: &str) -> Result<Source, Error> {
        let lower = location.to_ascii_lowercase();
        if lower.starts_with("https://") || lower.starts_with("http://") {
            let url = location.trim_end_matches('/').to_string();
            let rest = url.split_once("://").map_or("", |(_, r)| r);
            let mut parts = rest.split('/');
            let host = parts.next().unwrap_or("");
            if host.eq_ignore_ascii_case("github.com") {
                let parts: Vec<&str> = parts.collect();
                let (owner, repo, branch) = match parts.as_slice() {
                    [o, r] => (*o, *r, "master"),
                    [o, r, "tree", b] => (*o, *r, *b),
                    _ => {
                        return Err(Error::usage(format!(
                            "source {location}: a GitHub source is https://github.com/<owner>/<repo>[/tree/<branch>]"
                        )));
                    }
                };
                let repo = repo.trim_end_matches(".git");
                if [owner, repo, branch].iter().any(|p| p.is_empty() || p.contains(['?', '#'])) {
                    return Err(Error::usage(format!("source {location}: not a repository URL")));
                }
                return Ok(Source {
                    root: url.clone(),
                    location: Location::GitHub {
                        owner: owner.to_string(),
                        repo: repo.to_string(),
                        branch: branch.to_string(),
                    },
                });
            }
            if url.contains(['?', '#']) {
                return Err(Error::usage(format!(
                    "source {location}: a tree's URL has no query or fragment"
                )));
            }
            return Ok(Source { root: url.clone(), location: Location::Http(url) });
        }
        let root = location.trim_end_matches(['\\', '/']).replace('/', "\\");
        Ok(Source { location: Location::Dir(root.clone()), root })
    }

    /// Whether the source is read over HTTP(S).
    pub fn is_remote(&self) -> bool {
        !matches!(self.location, Location::Dir(_))
    }

    /// The URL of a directory's listing, or of a file (`rel`: below the
    /// tree's root, `\` separated).
    fn url(&self, rel: &str, listing: bool) -> Option<String> {
        let path = url_path(rel);
        match &self.location {
            Location::Dir(_) => None,
            Location::GitHub { owner, repo, branch } if listing => Some(format!(
                "https://api.github.com/repos/{owner}/{repo}/contents/{path}?ref={branch}"
            )),
            Location::GitHub { owner, repo, branch } => {
                Some(format!("https://raw.githubusercontent.com/{owner}/{repo}/{branch}/{path}"))
            }
            Location::Http(base) if listing => Some(format!("{base}/{path}/")),
            Location::Http(base) => Some(format!("{base}/{path}")),
        }
    }

    /// A fetch through the host, logged.
    fn get(&self, host: &mut dyn Host, what: &str, url: &str) -> Result<Vec<u8>, HostError> {
        host.log(&format!("source {what} url={url}"));
        let r = host.fetch(url);
        match &r {
            Ok(data) => host.log(&format!("source {what} bytes={}", data.len())),
            Err(e) => host.log(&format!("error source {what} code={} ({e}) url={url}", e.code)),
        }
        r
    }

    /// A directory's entries (`rel` below the root). Over HTTP(S) a
    /// listing that is not JSON fails as ERROR_INVALID_DATA (13), logged.
    fn list(&self, host: &mut dyn Host, rel: &str) -> Result<Vec<DirEntry>, HostError> {
        let Some(url) = self.url(rel, true) else {
            let Location::Dir(root) = &self.location else { unreachable!() };
            return host.list_dir(&format!("{root}\\{rel}"));
        };
        let bytes = self.get(host, "list", &url)?;
        let text = String::from_utf8_lossy(&bytes);
        match json::listing(&text) {
            Ok(entries) => {
                Ok(entries.into_iter().map(|(name, is_dir)| DirEntry { name, is_dir }).collect())
            }
            Err(e) => {
                host.log(&format!("error source list {e} url={url}"));
                Err(HostError { code: 13 })
            }
        }
    }

    /// A file's bytes (`rel` below the root).
    fn read(&self, host: &mut dyn Host, rel: &str) -> Result<Vec<u8>, HostError> {
        match self.url(rel, false) {
            Some(url) => self.get(host, "read", &url),
            None => {
                let Location::Dir(root) = &self.location else { unreachable!() };
                host.read_file(&format!("{root}\\{rel}"))
            }
        }
    }

    /// Where `rel` is, for messages.
    fn shown(&self, rel: &str) -> String {
        match self.url(rel, false) {
            Some(url) => url,
            None => format!("{}\\{rel}", self.root),
        }
    }

    fn package_dir(&self, id: &str) -> Result<String, Error> {
        let first = id.chars().next().ok_or_else(|| Error::usage("an empty package identifier"))?;
        if id.split('.').any(|p| p.is_empty() || p == "..")
            || id.contains(['\\', '/', ':', '?', '#', '%', ' '])
        {
            return Err(Error::usage(format!(
                "{id:?} is not a package identifier (Publisher.Name)"
            )));
        }
        Ok(format!("manifests\\{}\\{}", first.to_ascii_lowercase(), id.replace('.', "\\")))
    }

    /// The YAML files of a directory (names).
    fn yaml_files(&self, host: &mut dyn Host, dir: &str) -> Vec<String> {
        self.list(host, dir)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| !e.is_dir && e.name.to_ascii_lowercase().ends_with(".yaml"))
            .map(|e| e.name)
            .collect()
    }

    /// The versions of package `id` in this source, latest first.
    pub fn versions(&self, host: &mut dyn Host, id: &str) -> Result<Vec<String>, Error> {
        let dir = self.package_dir(id)?;
        let not_found = || Error::not_found(format!("no package {id} in source {}", self.root));
        let entries = match self.list(host, &dir) {
            Ok(e) => e,
            Err(e) if e.code == 2 || e.code == 3 || e.code == HTTP_STATUS_ERROR | 404 => {
                return Err(not_found());
            }
            Err(e) if self.is_remote() => {
                return Err(Error::failed(format!(
                    "source {}: listing {}: {e}",
                    self.root,
                    self.url(&dir, true).unwrap_or_default()
                )));
            }
            Err(_) => return Err(not_found()),
        };
        let prefix = format!("{}.", id.to_ascii_lowercase());
        let mut versions = Vec::new();
        for e in entries.into_iter().filter(|e| e.is_dir) {
            let files = self.yaml_files(host, &format!("{dir}\\{}", e.name));
            if files.iter().any(|f| f.to_ascii_lowercase().starts_with(&prefix)) {
                versions.push(e.name);
            }
        }
        if versions.is_empty() {
            return Err(not_found());
        }
        versions.sort_by(|a, b| version::compare(b, a));
        Ok(versions)
    }

    /// The manifest of one version (its directory's YAML files), read by
    /// the winget reader ([`crate::winget::package`]).
    pub fn manifest(&self, host: &mut dyn Host, id: &str, version: &str) -> Result<Package, Error> {
        let dir = format!("{}\\{version}", self.package_dir(id)?);
        let m = self.read_manifest(host, &dir)?;
        if !eq_ci(&m.id, id) {
            return Err(Error::failed(format!(
                "{}: the manifest is for {}, not {id}",
                self.shown(&dir),
                m.id
            )));
        }
        Ok(m)
    }

    fn read_manifest(&self, host: &mut dyn Host, dir: &str) -> Result<Package, Error> {
        let mut files = Vec::new();
        for f in self.yaml_files(host, dir) {
            let rel = format!("{dir}\\{f}");
            let path = self.shown(&rel);
            let bytes =
                self.read(host, &rel).map_err(|e| Error::failed(format!("reading {path}: {e}")))?;
            let text = String::from_utf8(bytes)
                .map_err(|_| Error::failed(format!("{path} is not UTF-8")))?;
            files.push((path, text));
        }
        let refs: Vec<(&str, &str)> = files.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
        // The error names the file, line and column.
        Package::from_files(&refs).map_err(|e| Error::failed(format!("{e}")))
    }

    /// Package `id` at `version` (one that compares equal: `1.2` finds
    /// `1.2.0`), or its latest version.
    pub fn find(
        &self,
        host: &mut dyn Host,
        id: &str,
        version: Option<&str>,
    ) -> Result<Package, Error> {
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

    /// Every package of the source at its latest version (a walk of the
    /// whole tree: fine for snapshots and small mirrors; GitHub's
    /// repository is refused, as walking it would take tens of thousands
    /// of requests: winget reads its source index, ROADMAP PK step 2). A
    /// version whose manifest does not read is skipped and logged.
    pub fn packages(&self, host: &mut dyn Host) -> Result<Vec<Package>, Error> {
        if matches!(self.location, Location::GitHub { .. }) {
            return Err(Error::unsupported(format!(
                "source {}: searching a GitHub repository needs winget's source index (not built \
                 yet, ROADMAP PK step 2); install by identifier, or search a snapshot: --source <dir>",
                self.root
            )));
        }
        let mut dirs = Vec::new();
        let letters = self.list(host, "manifests").map_err(|e| {
            Error::failed(format!("source {}: {}: {e}", self.root, self.shown("manifests")))
        })?;
        for l in letters.into_iter().filter(|e| e.is_dir) {
            dirs.push(format!("manifests\\{}", l.name));
        }
        // Depth first; a directory with YAML files is a version.
        let mut packages: Vec<(String, Vec<String>)> = Vec::new();
        while let Some(dir) = dirs.pop() {
            let Ok(entries) = self.list(host, &dir) else { continue };
            for e in entries.into_iter().filter(|e| e.is_dir) {
                let sub = format!("{dir}\\{}", e.name);
                if self.yaml_files(host, &sub).is_empty() {
                    dirs.push(sub);
                } else if let Some(p) = packages.iter_mut().find(|p| p.0 == dir) {
                    p.1.push(e.name);
                } else {
                    packages.push((dir.clone(), alloc::vec![e.name]));
                }
            }
        }
        packages.sort();
        let mut out = Vec::new();
        for (dir, versions) in packages {
            let Some(latest) = version::latest(versions.iter().map(String::as_str)) else {
                continue;
            };
            let at = format!("{dir}\\{latest}");
            match self.read_manifest(host, &at) {
                Ok(m) => out.push(m),
                Err(e) => host.log(&format!("source: skipped {}: {e}", self.shown(&at))),
            }
        }
        Ok(out)
    }

    /// Packages whose identifier, name, moniker or tags contain `query`
    /// (any case), each at its latest version; an exact identifier first.
    pub fn search(&self, host: &mut dyn Host, query: &str) -> Result<Vec<Found>, Error> {
        let mut found = Vec::new();
        for m in self.packages(host)? {
            let l = &m.default_locale;
            let matched = if contains_ci(&m.id, query) {
                Some("Id".to_string())
            } else if l.package_name.as_deref().is_some_and(|n| contains_ci(n, query)) {
                Some("Name".to_string())
            } else if l.moniker.as_deref().is_some_and(|n| contains_ci(n, query)) {
                Some("Moniker".to_string())
            } else {
                l.tags.iter().find(|t| contains_ci(t, query)).map(|t| format!("Tag: {t}"))
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

    /// The installer's bytes: from a local snapshot's `installers\`
    /// mirror, else downloaded (the host's fetcher).
    pub fn installer_bytes(
        &self,
        host: &mut dyn Host,
        installer: &Installer,
    ) -> Result<Vec<u8>, Error> {
        if let Location::Dir(root) = &self.location {
            let mirrored = format!("{root}\\installers\\{}", installer.file_name());
            if host.exists(&mirrored) == Some(false) {
                host.log(&format!("download from-mirror path={mirrored}"));
                return host
                    .read_file(&mirrored)
                    .map_err(|e| Error::failed(format!("reading {mirrored}: {e}")));
            }
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
