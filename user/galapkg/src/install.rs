//! Installing, uninstalling, upgrading and listing packages, as winget
//! does for `zip` and `portable` installers (ROADMAP PK stage 2):
//!
//! - files go to `%LOCALAPPDATA%\Microsoft\WinGet\Packages\<id>_<source>`
//!   (a portable `.exe` renamed to its command, a zip unpacked whole);
//! - each command (PortableCommandAlias, Commands, or the file's own name)
//!   gets a shim in `%LOCALAPPDATA%\Microsoft\WinGet\Links` ([`crate::shim`]),
//!   and that directory is put on the user's PATH (`HKCU\Environment`);
//!   ArchiveBinariesDependOnPath puts the package directory itself there
//!   instead, as winget does;
//! - an uninstall record (ARP entry) under
//!   `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\<id>_<source>`
//!   with winget's values (DisplayName, DisplayVersion, Publisher,
//!   InstallLocation, UninstallString, WinGetPackageIdentifier,
//!   WinGetSourceIdentifier, WinGetInstallerType...) and gala's (GalaLinks,
//!   GalaSource, GalaInstallerSha256).
//!
//! Every change goes through a [`Tx`], which journals how to undo it
//! (files and registry values removed are kept in memory): a failure
//! anywhere rolls back what was done, an upgrade's removal of the old
//! version included. Everything is checked before the first change
//! (the SHA-256, the archive's CRCs, NestedInstallerFiles), so a rollback
//! is for what cannot be checked ahead: the disk, a command another
//! package owns. Each step is a line of the install log ([`Host::log`]).

use crate::manifest::{Installer, Manifest};
use crate::source::{Source, WINGET_SOURCE_ID};
use crate::{eq_ci, sha256_hex, shim, version, zip};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt;

/// A registry root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Root {
    CurrentUser,
    LocalMachine,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegValue {
    Sz(String),
    ExpandSz(String),
    Dword(u32),
    MultiSz(Vec<String>),
}

impl RegValue {
    pub fn text(&self) -> Option<&str> {
        match self {
            RegValue::Sz(s) | RegValue::ExpandSz(s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

/// A failed system call: the Win32 error and what was being done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostError {
    pub code: u32,
}

/// A download whose server answered with an HTTP status other than 200:
/// `HTTP_STATUS_ERROR | status` (bit 29, the customer bit of Win32
/// error codes, so no system error is mistaken for it).
pub const HTTP_STATUS_ERROR: u32 = 0x2000_0000;

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.code {
            2 => f.write_str("the file does not exist (error 2)"),
            3 => f.write_str("the path does not exist (error 3)"),
            5 => f.write_str("access denied (error 5)"),
            32 => f.write_str("the file is in use (error 32)"),
            80 | 183 => write!(f, "it already exists (error {})", self.code),
            112 => f.write_str("the disk is full (error 112)"),
            126 => f.write_str("a DLL it needs is not on this machine (error 126)"),
            145 => f.write_str("the directory is not empty (error 145)"),
            // WinHTTP's (the downloads).
            12002 => f.write_str("the server did not answer in time (error 12002)"),
            12007 => f.write_str("the server's name was not resolved (error 12007)"),
            12029 => f.write_str("could not connect to the server (error 12029)"),
            12030 => f.write_str("the connection was cut (error 12030)"),
            12156 => {
                f.write_str("a redirect was refused: https to http, or too many (error 12156)")
            }
            12157 => f.write_str("the TLS handshake failed (error 12157)"),
            12175 => f.write_str(
                "the server's certificate is not valid or not trusted by the machine's root \
                 store (error 12175)",
            ),
            c if c & HTTP_STATUS_ERROR != 0 && c & !HTTP_STATUS_ERROR < 1000 => {
                write!(f, "the server answered HTTP {}", c & !HTTP_STATUS_ERROR)
            }
            c => write!(f, "error {c}"),
        }
    }
}

/// What the package manager needs from the system. `gala.exe`
/// implements it with kernel32 and advapi32; the tests in memory.
/// Paths are Windows paths (`C:\...`), keys are below the root.
pub trait Host {
    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, HostError>;
    /// Creates a new file with `data` (an error if it exists).
    fn create_file(&mut self, path: &str, data: &[u8]) -> Result<(), HostError>;
    fn delete_file(&mut self, path: &str) -> Result<(), HostError>;
    /// Creates one directory (its parent exists; an error if it exists).
    fn create_dir(&mut self, path: &str) -> Result<(), HostError>;
    /// Removes an empty directory.
    fn remove_dir(&mut self, path: &str) -> Result<(), HostError>;
    /// None if nothing is there, else whether it is a directory.
    fn exists(&mut self, path: &str) -> Option<bool>;
    fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, HostError>;
    fn reg_get(&mut self, root: Root, key: &str, name: &str) -> Option<RegValue>;
    fn reg_values(&mut self, root: Root, key: &str) -> Result<Vec<(String, RegValue)>, HostError>;
    fn reg_subkeys(&mut self, root: Root, key: &str) -> Vec<String>;
    fn reg_key_exists(&mut self, root: Root, key: &str) -> bool;
    /// Sets a value, creating the key (and its parents) if needed.
    fn reg_set(
        &mut self,
        root: Root,
        key: &str,
        name: &str,
        value: &RegValue,
    ) -> Result<(), HostError>;
    fn reg_delete_value(&mut self, root: Root, key: &str, name: &str) -> Result<(), HostError>;
    /// Deletes a key that has no subkeys.
    fn reg_delete_key(&mut self, root: Root, key: &str) -> Result<(), HostError>;
    /// Downloads a URL (`gala.exe`: http and https over winhttp.dll,
    /// redirects followed, `file:///` and paths read). A URL scheme the
    /// host cannot fetch is ERROR_NOT_SUPPORTED; an HTTP status other
    /// than 200 is `HTTP_STATUS_ERROR | status`.
    fn fetch(&mut self, url: &str) -> Result<Vec<u8>, HostError>;
    fn env(&mut self, name: &str) -> Option<String>;
    /// The path of the running `gala.exe` (copied as command shims).
    fn self_exe(&mut self) -> String;
    /// A line of the install log.
    fn log(&mut self, line: &str);
    /// Something gala cannot do yet (the ledger: `gala.exe!...`).
    fn record_missing(&mut self, _what: &str) {}
    /// Today as YYYYMMDD (InstallDate).
    fn today(&mut self) -> String;
    /// Runs a command line (the program found as CreateProcessW finds
    /// it), waits up to `timeout_ms` and returns its exit code;
    /// ERROR_TIMEOUT (1460) if it is still running. Installers (inno,
    /// nullsoft, exe) and uninstallers.
    fn run(&mut self, command_line: &str, timeout_ms: u32) -> Result<u32, HostError>;
    /// Waits (polling for an uninstaller to finish).
    fn sleep(&mut self, ms: u32);
}

/// What went wrong, with the exit code `gala` returns for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub kind: Kind,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// 1: a bad command line.
    Usage,
    /// 2: no such package, version or installed package.
    NotFound,
    /// 3: an installer type or source gala cannot handle yet.
    Unsupported,
    /// 4: the install failed (and was rolled back).
    Failed,
    /// 5: the download's SHA-256 is not the manifest's.
    HashMismatch,
}

impl Kind {
    pub fn exit_code(self) -> u32 {
        match self {
            Kind::Usage => 1,
            Kind::NotFound => 2,
            Kind::Unsupported => 3,
            Kind::Failed => 4,
            Kind::HashMismatch => 5,
        }
    }
}

impl Error {
    pub fn usage(m: impl Into<String>) -> Error {
        Error { kind: Kind::Usage, message: m.into() }
    }
    pub fn not_found(m: impl Into<String>) -> Error {
        Error { kind: Kind::NotFound, message: m.into() }
    }
    pub fn unsupported(m: impl Into<String>) -> Error {
        Error { kind: Kind::Unsupported, message: m.into() }
    }
    pub fn failed(m: impl Into<String>) -> Error {
        Error { kind: Kind::Failed, message: m.into() }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

pub const UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
pub const ENVIRONMENT_KEY: &str = "Environment";

/// The directories of the per-user install.
pub struct Layout {
    pub packages: String,
    pub links: String,
}

impl Layout {
    pub fn new(host: &mut dyn Host) -> Result<Layout, Error> {
        let local = host
            .env("LOCALAPPDATA")
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::failed("LOCALAPPDATA is not set"))?;
        let local = local.trim_end_matches('\\');
        Ok(Layout {
            packages: format!("{local}\\Microsoft\\WinGet\\Packages"),
            links: format!("{local}\\Microsoft\\WinGet\\Links"),
        })
    }
}

/// The key of an install (the package directory's and the ARP entry's
/// name): `<PackageIdentifier>_<source identifier>`.
pub fn install_key(id: &str) -> String {
    format!("{id}_{WINGET_SOURCE_ID}")
}

// ---- The transaction --------------------------------------------------

enum Undo {
    CreatedFile(String),
    CreatedDir(String),
    CreatedKey(Root, String),
    /// A value set over what was there before (None: nothing).
    SetValue(Root, String, String, Option<RegValue>),
    RemovedFile(String, Vec<u8>),
    RemovedDir(String),
    RemovedKey(Root, String, Vec<(String, RegValue)>),
}

/// Changes with their undo journal. Dropped without [`Tx::commit`], it
/// does nothing: callers call [`Tx::rollback`] on failure.
pub struct Tx<'h> {
    pub host: &'h mut dyn Host,
    journal: Vec<Undo>,
}

fn parent(path: &str) -> Option<&str> {
    path.rfind('\\').map(|i| &path[..i]).filter(|p| !p.is_empty() && !p.ends_with(':'))
}

impl<'h> Tx<'h> {
    pub fn new(host: &'h mut dyn Host) -> Tx<'h> {
        Tx { host, journal: Vec::new() }
    }

    fn io<T>(&mut self, r: Result<T, HostError>, what: &str, path: &str) -> Result<T, Error> {
        r.map_err(|e| {
            self.host.log(&format!("error {what} path={path} {e}"));
            Error::failed(format!("{what} {path}: {e}"))
        })
    }

    /// Creates `path` and the missing directories above it.
    pub fn create_dir_all(&mut self, path: &str) -> Result<(), Error> {
        match self.host.exists(path) {
            Some(true) => return Ok(()),
            Some(false) => return Err(Error::failed(format!("{path} is a file, not a directory"))),
            None => {}
        }
        if let Some(p) = parent(path) {
            self.create_dir_all(p)?;
        }
        let r = self.host.create_dir(path);
        self.io(r, "creating directory", path)?;
        self.journal.push(Undo::CreatedDir(path.to_string()));
        Ok(())
    }

    pub fn create_file(&mut self, path: &str, data: &[u8]) -> Result<(), Error> {
        let r = self.host.create_file(path, data);
        self.io(r, "writing", path)?;
        self.journal.push(Undo::CreatedFile(path.to_string()));
        Ok(())
    }

    /// Removes a file, keeping its contents to put back.
    pub fn remove_file(&mut self, path: &str) -> Result<(), Error> {
        let r = self.host.read_file(path);
        let data = self.io(r, "reading (to remove)", path)?;
        let r = self.host.delete_file(path);
        self.io(r, "removing", path)?;
        self.journal.push(Undo::RemovedFile(path.to_string(), data));
        Ok(())
    }

    /// Removes a directory and everything below it.
    pub fn remove_tree(&mut self, path: &str) -> Result<(), Error> {
        let r = self.host.list_dir(path);
        let entries = self.io(r, "listing", path)?;
        for e in entries {
            let p = format!("{path}\\{}", e.name);
            if e.is_dir { self.remove_tree(&p)? } else { self.remove_file(&p)? }
        }
        let r = self.host.remove_dir(path);
        self.io(r, "removing directory", path)?;
        self.journal.push(Undo::RemovedDir(path.to_string()));
        Ok(())
    }

    /// Sets a registry value, creating its key if needed.
    pub fn set_value(
        &mut self,
        root: Root,
        key: &str,
        name: &str,
        value: RegValue,
    ) -> Result<(), Error> {
        // Keys that do not exist yet (the key and its parents) are
        // deleted again on rollback, deepest first.
        let mut missing = Vec::new();
        let mut k = Some(key);
        while let Some(cur) = k {
            if self.host.reg_key_exists(root, cur) {
                break;
            }
            missing.push(cur.to_string());
            k = parent(cur);
        }
        let old = self.host.reg_get(root, key, name);
        let r = self.host.reg_set(root, key, name, &value);
        self.io(r, "setting registry value", &format!("{key}\\{name}"))?;
        for m in missing.into_iter().rev() {
            self.journal.push(Undo::CreatedKey(root, m));
        }
        self.journal.push(Undo::SetValue(root, key.to_string(), name.to_string(), old));
        Ok(())
    }

    /// Deletes a key (without subkeys), keeping its values to put back.
    pub fn delete_key(&mut self, root: Root, key: &str) -> Result<(), Error> {
        let r = self.host.reg_values(root, key);
        let values = self.io(r, "reading registry key", key)?;
        let r = self.host.reg_delete_key(root, key);
        self.io(r, "deleting registry key", key)?;
        self.journal.push(Undo::RemovedKey(root, key.to_string(), values));
        Ok(())
    }

    /// Keeps the changes.
    pub fn commit(mut self) {
        self.journal.clear();
    }

    /// Undoes every change, newest first; logs each step. Returns how
    /// many steps could not be undone.
    pub fn rollback(self) -> usize {
        self.undo("rollback")
    }

    /// Undoes temporary changes after they served (a staged installer),
    /// logged as `cleanup`.
    pub fn cleanup(self) -> usize {
        self.undo("cleanup")
    }

    fn undo(mut self, label: &str) -> usize {
        let mut failures = 0;
        let n = self.journal.len();
        self.host.log(&format!("{label} steps={n}"));
        while let Some(u) = self.journal.pop() {
            let (what, path, r) = match u {
                Undo::CreatedFile(p) => {
                    let r = self.host.delete_file(&p);
                    ("remove file", p, r)
                }
                Undo::CreatedDir(p) => {
                    let r = self.host.remove_dir(&p);
                    ("remove directory", p, r)
                }
                Undo::CreatedKey(root, k) => {
                    let r = self.host.reg_delete_key(root, &k);
                    ("delete key", k, r)
                }
                Undo::SetValue(root, k, name, old) => {
                    let r = match &old {
                        Some(v) => self.host.reg_set(root, &k, &name, v),
                        None => match self.host.reg_delete_value(root, &k, &name) {
                            // The key is gone already (a CreatedKey undone after it).
                            Err(HostError { code: 2 }) => Ok(()),
                            r => r,
                        },
                    };
                    ("restore value", format!("{k}\\{name}"), r)
                }
                Undo::RemovedFile(p, data) => {
                    let r = self.host.create_file(&p, &data);
                    ("restore file", p, r)
                }
                Undo::RemovedDir(p) => {
                    let r = self.host.create_dir(&p);
                    ("restore directory", p, r)
                }
                Undo::RemovedKey(root, k, values) => {
                    let mut r = Ok(());
                    for (name, v) in &values {
                        if let Err(e) = self.host.reg_set(root, &k, name, v) {
                            r = Err(e);
                        }
                    }
                    ("restore key", k, r)
                }
            };
            match r {
                Ok(()) => self.host.log(&format!("{label} {what} path={path}")),
                Err(e) => {
                    failures += 1;
                    self.host.log(&format!("{label} FAILED {what} path={path} {e}"));
                }
            }
        }
        failures
    }
}

// ---- What an install puts where ---------------------------------------

/// A command to link: its name and the file (relative to the package
/// directory) it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub alias: String,
    pub target: String,
}

/// The files of a package (relative paths, `\` separated) and its
/// commands, checked before anything is written.
#[derive(Debug, Default)]
pub struct Plan {
    pub dirs: Vec<String>,
    pub files: Vec<(String, Vec<u8>)>,
    pub commands: Vec<Command>,
    /// ArchiveBinariesDependOnPath: the package directory goes on PATH
    /// instead of shims.
    pub dir_on_path: bool,
}

/// Installer types gala installs (the rest: ROADMAP PK stages 4 and 5):
/// portable, zip of portable, and installers that are programs (inno,
/// nullsoft, exe: [`exe`]).
pub fn supported(i: &Installer) -> bool {
    i.installer_type == "portable"
        || (i.installer_type == "zip" && i.nested_installer_type.as_deref() == Some("portable"))
        || exe::is_program(&i.installer_type)
}

/// A command's name from a file name: without directory and `.exe`.
fn stem(path: &str) -> &str {
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    match name.len().checked_sub(4) {
        Some(i) if name.is_char_boundary(i) && name[i..].eq_ignore_ascii_case(".exe") => &name[..i],
        _ => name,
    }
}

fn valid_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias != "."
        && alias != ".."
        && !alias.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|'])
        && !eq_ci(alias, "gala")
}

/// What installing `installer` (whose bytes are `data`, already checked
/// against the SHA-256) puts in the package directory.
pub fn plan(installer: &Installer, data: &[u8]) -> Result<Plan, Error> {
    let mut p = Plan::default();
    match installer.installer_type.as_str() {
        "portable" => {
            let alias = installer
                .commands
                .first()
                .map(String::as_str)
                .unwrap_or_else(|| stem(installer.file_name()));
            if !valid_alias(alias) {
                return Err(Error::failed(format!("{alias:?} cannot be a command name")));
            }
            let file = format!("{alias}.exe");
            p.files.push((file.clone(), data.to_vec()));
            p.commands.push(Command { alias: alias.to_string(), target: file });
        }
        "zip" => {
            match installer.nested_installer_type.as_deref() {
                Some("portable") => {}
                Some(other) => {
                    return Err(Error::unsupported(format!(
                        "a zip whose NestedInstallerType is {other} (gala installs portable ones; ROADMAP PK)"
                    )));
                }
                None => return Err(Error::failed("a zip installer without NestedInstallerType")),
            }
            let archive = zip::Archive::open(data)
                .map_err(|e| Error::failed(format!("{}: {e}", installer.file_name())))?;
            for e in &archive.entries {
                let Some(path) = zip::safe_path(&e.name) else {
                    return Err(Error::failed(format!(
                        "the archive has an unsafe path: {:?}",
                        e.name
                    )));
                };
                if e.is_dir() {
                    p.dirs.push(path);
                } else {
                    let bytes = archive
                        .read(e)
                        .map_err(|e| Error::failed(format!("{}: {e}", installer.file_name())))?;
                    p.files.push((path, bytes));
                }
            }
            if installer.nested_files.is_empty() {
                return Err(Error::failed("a portable zip without NestedInstallerFiles"));
            }
            for f in &installer.nested_files {
                let path = zip::safe_path(&f.relative_path).ok_or_else(|| {
                    Error::failed(format!("unsafe RelativeFilePath {:?}", f.relative_path))
                })?;
                let Some((stored, _)) = p.files.iter().find(|(name, _)| eq_ci(name, &path)) else {
                    return Err(Error::failed(format!(
                        "NestedInstallerFiles names {}, which is not in {}",
                        f.relative_path,
                        installer.file_name()
                    )));
                };
                let alias = f.alias.clone().unwrap_or_else(|| stem(&path).to_string());
                if !valid_alias(&alias) {
                    return Err(Error::failed(format!("{alias:?} cannot be a command name")));
                }
                p.commands.push(Command { alias, target: stored.clone() });
            }
            p.dir_on_path = installer.archive_binaries_depend_on_path;
        }
        other => {
            return Err(Error::unsupported(format!(
                "InstallerType {other} is not supported yet (gala installs zip, portable, inno, nullsoft and exe; ROADMAP PK stages 4-5)"
            )));
        }
    }
    // Directories every file needs, parents first.
    let mut dirs: Vec<String> = Vec::new();
    for (f, _) in &p.files {
        let mut at = 0;
        while let Some(i) = f[at..].find('\\') {
            let d = &f[..at + i];
            if !dirs.iter().any(|x| eq_ci(x, d)) {
                dirs.push(d.to_string());
            }
            at += i + 1;
        }
    }
    for d in core::mem::take(&mut p.dirs) {
        if !dirs.iter().any(|x| eq_ci(x, &d)) {
            dirs.push(d);
        }
    }
    dirs.sort_by_key(|d| d.matches('\\').count());
    p.dirs = dirs;
    Ok(p)
}

// ---- PATH -------------------------------------------------------------

fn path_entries(path: &str) -> impl Iterator<Item = &str> {
    path.split(';')
        .map(|e| e.trim().trim_matches('"').trim_end_matches('\\'))
        .filter(|e| !e.is_empty())
}

/// Puts `dir` on the user's PATH (HKCU\Environment\Path) if it is not
/// there; whether it was added.
fn add_to_path(tx: &mut Tx, dir: &str) -> Result<bool, Error> {
    let old = tx.host.reg_get(Root::CurrentUser, ENVIRONMENT_KEY, "Path");
    let text = old.as_ref().and_then(RegValue::text).unwrap_or("");
    let local = tx.host.env("LOCALAPPDATA").unwrap_or_default();
    let expanded = |e: &str| e.replace("%LOCALAPPDATA%", &local).replace("%localappdata%", &local);
    if path_entries(text).any(|e| eq_ci(&expanded(e), dir)) {
        return Ok(false);
    }
    let new = if text.is_empty() {
        dir.to_string()
    } else {
        format!("{};{dir}", text.trim_end_matches(';'))
    };
    let value = match old {
        Some(RegValue::Sz(_)) => RegValue::Sz(new),
        _ => RegValue::ExpandSz(new),
    };
    tx.set_value(Root::CurrentUser, ENVIRONMENT_KEY, "Path", value)?;
    tx.host.log(&format!("path added dir={dir}"));
    Ok(true)
}

/// Takes `dir` off the user's PATH.
fn remove_from_path(tx: &mut Tx, dir: &str) -> Result<(), Error> {
    let Some(old) = tx.host.reg_get(Root::CurrentUser, ENVIRONMENT_KEY, "Path") else {
        return Ok(());
    };
    let Some(text) = old.text() else { return Ok(()) };
    let kept: Vec<&str> = text
        .split(';')
        .filter(|e| !e.is_empty() && !eq_ci(e.trim().trim_end_matches('\\'), dir))
        .collect();
    let new = kept.join(";");
    if new != text {
        let value = match old {
            RegValue::Sz(_) => RegValue::Sz(new),
            _ => RegValue::ExpandSz(new),
        };
        tx.set_value(Root::CurrentUser, ENVIRONMENT_KEY, "Path", value)?;
        tx.host.log(&format!("path removed dir={dir}"));
    }
    Ok(())
}

// ---- Installed packages ----------------------------------------------

/// An ARP entry (installed by gala, winget or anything else).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub root: Root,
    /// The ARP key's name (winget's product code for portable installs).
    pub key: String,
    pub name: String,
    pub version: String,
    pub publisher: Option<String>,
    /// WinGetPackageIdentifier (None: not installed by winget or gala).
    pub id: Option<String>,
    pub source: Option<String>,
    pub install_location: Option<String>,
}

fn sz(host: &mut dyn Host, root: Root, key: &str, name: &str) -> Option<String> {
    host.reg_get(root, key, name)
        .and_then(|v| v.text().map(ToString::to_string))
        .filter(|s| !s.is_empty())
}

/// Every ARP entry, the current user's first, then the machine's.
pub fn installed(host: &mut dyn Host) -> Vec<Installed> {
    let mut out = Vec::new();
    for root in [Root::CurrentUser, Root::LocalMachine] {
        let root_key = if root == Root::LocalMachine {
            "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall"
        } else {
            UNINSTALL_KEY
        };
        for key in host.reg_subkeys(root, root_key) {
            let k = format!("{root_key}\\{key}");
            let Some(name) = sz(host, root, &k, "DisplayName") else { continue };
            out.push(Installed {
                root,
                // A program installer's own DisplayVersion may differ from
                // the manifest's version: gala's tag wins.
                version: sz(host, root, &k, "GalaPackageVersion")
                    .or_else(|| sz(host, root, &k, "DisplayVersion"))
                    .unwrap_or_else(|| "Unknown".to_string()),
                publisher: sz(host, root, &k, "Publisher"),
                id: sz(host, root, &k, "WinGetPackageIdentifier"),
                source: sz(host, root, &k, "GalaSource"),
                install_location: sz(host, root, &k, "InstallLocation"),
                key,
                name,
            });
        }
    }
    out
}

/// The installed package `what` names: its identifier, its ARP key
/// (`--product-code`) or its name (any case). Only the user's gala or
/// winget installs can be removed by gala (stage 2).
pub fn find_installed(host: &mut dyn Host, what: &str) -> Result<Installed, Error> {
    let all = installed(host);
    let by_id = all.iter().find(|i| i.id.as_deref().is_some_and(|id| eq_ci(id, what)));
    let found = by_id
        .or_else(|| all.iter().find(|i| eq_ci(&i.key, what)))
        .or_else(|| all.iter().find(|i| eq_ci(&i.name, what)))
        .ok_or_else(|| Error::not_found(format!("no installed package matches {what}")))?;
    Ok(found.clone())
}

// ---- Install, uninstall, upgrade ---------------------------------------

/// Options of an install.
pub struct Request<'a> {
    pub id: &'a str,
    pub version: Option<&'a str>,
    /// The source's location as given (kept in the ARP entry for
    /// `gala upgrade`).
    pub source_location: &'a str,
    /// `--location`: where a program installer installs (its
    /// InstallLocation switch).
    pub location: Option<&'a str>,
}

/// What an install did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Installed { id: String, version: String, commands: Vec<String>, location: String },
    AlreadyInstalled { id: String, version: String },
    Upgraded { id: String, from: String, to: String },
    Uninstalled { id: String, version: String },
    UpToDate { id: String, version: String },
}

/// The checked download and the plan: everything before the first change.
struct Prepared {
    manifest: Manifest,
    installer: Installer,
    plan: Plan,
    /// A program installer's bytes (to run); empty for the others.
    data: Vec<u8>,
}

fn prepare(
    host: &mut dyn Host,
    source: &Source,
    id: &str,
    version: Option<&str>,
) -> Result<Prepared, Error> {
    let manifest = source.find(host, id, version)?;
    host.log(&format!(
        "manifest id={} version={} manifest_version={}",
        manifest.id, manifest.version, manifest.manifest_version
    ));
    let Some(installer) = manifest.select_installer(supported).cloned() else {
        let arches: Vec<&str> =
            manifest.installers.iter().map(|i| i.architecture.as_str()).collect();
        return Err(Error::unsupported(format!(
            "{} {} has no x64 or neutral installer (it has {})",
            manifest.id,
            manifest.version,
            arches.join(", ")
        )));
    };
    host.log(&format!(
        "installer type={} nested={} arch={} url={}",
        installer.installer_type,
        installer.nested_installer_type.as_deref().unwrap_or("-"),
        installer.architecture,
        installer.url
    ));
    if !supported(&installer) {
        host.record_missing(&format!(
            "gala.exe!install(InstallerType {})",
            installer.effective_type()
        ));
        return Err(Error::unsupported(format!(
            "{} {}: InstallerType {} is not supported yet (gala installs zip, portable, inno, nullsoft and exe; ROADMAP PK stages 4-5)",
            manifest.id,
            manifest.version,
            installer.effective_type()
        )));
    }
    let data = source.installer_bytes(host, &installer)?;
    let got = sha256_hex(&data);
    host.log(&format!("sha256 expected={} got={} bytes={}", installer.sha256, got, data.len()));
    if got != installer.sha256 {
        return Err(Error {
            kind: Kind::HashMismatch,
            message: format!(
                "the installer's SHA-256 is {got}, the manifest says {}: not installed",
                installer.sha256
            ),
        });
    }
    if exe::is_program(&installer.installer_type) {
        host.log(&format!("plan run-installer type={}", installer.installer_type));
        return Ok(Prepared { manifest, installer, plan: Plan::default(), data });
    }
    let plan = plan(&installer, &data)?;
    host.log(&format!(
        "plan files={} dirs={} commands={}",
        plan.files.len(),
        plan.dirs.len(),
        plan.commands.len()
    ));
    Ok(Prepared { manifest, installer, plan, data: Vec::new() })
}

/// Writes a prepared package (inside `tx`).
fn apply(
    tx: &mut Tx,
    layout: &Layout,
    p: &Prepared,
    source_location: &str,
) -> Result<String, Error> {
    let m = &p.manifest;
    let key = install_key(&m.id);
    let dir = format!("{}\\{key}", layout.packages);
    if tx.host.exists(&dir).is_some() {
        return Err(Error::failed(format!(
            "{dir} is in the way (left by an install gala does not know of); remove it first"
        )));
    }
    tx.create_dir_all(&dir)?;
    for d in &p.plan.dirs {
        tx.create_dir_all(&format!("{dir}\\{d}"))?;
    }
    for (f, data) in &p.plan.files {
        tx.create_file(&format!("{dir}\\{f}"), data)?;
    }
    tx.host.log(&format!("unpacked dir={dir} files={}", p.plan.files.len()));
    let mut links = Vec::new();
    let mut added_dir_to_path = false;
    if p.plan.dir_on_path {
        added_dir_to_path = add_to_path(tx, &dir)?;
    } else if !p.plan.commands.is_empty() {
        tx.create_dir_all(&layout.links)?;
        let exe_path = tx.host.self_exe();
        let r = tx.host.read_file(&exe_path);
        let exe = tx.io(r, "reading", &exe_path)?;
        for c in &p.plan.commands {
            let link = format!("{}\\{}.exe", layout.links, c.alias);
            let shim_file = format!("{}\\{}.shim", layout.links, c.alias);
            if tx.host.exists(&link).is_some() || tx.host.exists(&shim_file).is_some() {
                let owner = tx
                    .host
                    .read_file(&shim_file)
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
                    .and_then(|t| shim::parse(&t))
                    .unwrap_or_else(|| "something else".to_string());
                return Err(Error::failed(format!(
                    "the command {} exists already ({link}, which runs {owner})",
                    c.alias
                )));
            }
            let target = format!("{dir}\\{}", c.target);
            tx.create_file(&link, &exe)?;
            tx.create_file(&shim_file, shim::format(&target).as_bytes())?;
            tx.host.log(&format!("link command={} target={target}", c.alias));
            links.push(link);
            links.push(shim_file);
        }
        add_to_path(tx, &layout.links)?;
    }
    // The uninstall record.
    let arp = format!("{UNINSTALL_KEY}\\{key}");
    let gala = tx.host.self_exe();
    let today = tx.host.today();
    let mut values = alloc::vec![
        ("DisplayName", RegValue::Sz(m.display_name().to_string())),
        ("DisplayVersion", RegValue::Sz(m.version.clone())),
        ("InstallDate", RegValue::Sz(today)),
        ("InstallLocation", RegValue::Sz(dir.clone())),
        ("UninstallString", RegValue::Sz(format!("\"{gala}\" uninstall --product-code {key}"))),
        (
            "QuietUninstallString",
            RegValue::Sz(format!("\"{gala}\" uninstall --product-code {key}"))
        ),
        ("NoModify", RegValue::Dword(1)),
        ("NoRepair", RegValue::Dword(1)),
        ("WinGetPackageIdentifier", RegValue::Sz(m.id.clone())),
        ("WinGetSourceIdentifier", RegValue::Sz(WINGET_SOURCE_ID.to_string())),
        ("WinGetInstallerType", RegValue::Sz(p.installer.effective_type().to_string())),
        ("InstallDirectoryAddedToPath", RegValue::Dword(u32::from(added_dir_to_path))),
        ("GalaLinks", RegValue::MultiSz(links)),
        ("GalaSource", RegValue::Sz(source_location.to_string())),
        ("GalaInstallerSha256", RegValue::Sz(p.installer.sha256.clone())),
    ];
    if let Some(publisher) = &m.publisher {
        values.push(("Publisher", RegValue::Sz(publisher.clone())));
    }
    if let Some(url) = &m.package_url {
        values.push(("URLInfoAbout", RegValue::Sz(url.clone())));
    }
    for (name, v) in values {
        tx.set_value(Root::CurrentUser, &arp, name, v)?;
    }
    tx.host.log(&format!("registered key=HKCU\\{arp}"));
    Ok(dir)
}

/// Removes an installed package (inside `tx`).
fn remove(tx: &mut Tx, layout: &Layout, i: &Installed) -> Result<(), Error> {
    if i.root != Root::CurrentUser || i.id.is_none() {
        return Err(Error::unsupported(format!(
            "{} was not installed by gala or winget for this user; gala runs only the uninstallers of programs it installed (TODO: any entry's QuietUninstallString)",
            i.name
        )));
    }
    let arp = format!("{UNINSTALL_KEY}\\{}", i.key);
    let links = match tx.host.reg_get(Root::CurrentUser, &arp, "GalaLinks") {
        Some(RegValue::MultiSz(v)) => v,
        _ => Vec::new(),
    };
    for l in &links {
        // Only what is in the Links directory (the record could be edited).
        if parent(l).is_some_and(|p| eq_ci(p, &layout.links)) && tx.host.exists(l) == Some(false) {
            tx.remove_file(l)?;
        }
    }
    if let Some(dir) = &i.install_location {
        let inside = parent(dir).is_some_and(|p| eq_ci(p, &layout.packages));
        if !inside {
            return Err(Error::failed(format!(
                "InstallLocation {dir} is not in {}: not removing it",
                layout.packages
            )));
        }
        if tx.host.exists(dir) == Some(true) {
            tx.remove_tree(dir)?;
        }
        if tx.host.reg_get(Root::CurrentUser, &arp, "InstallDirectoryAddedToPath")
            == Some(RegValue::Dword(1))
        {
            remove_from_path(tx, dir)?;
        }
    }
    tx.delete_key(Root::CurrentUser, &arp)?;
    tx.host.log(&format!("unregistered key=HKCU\\{arp}"));
    Ok(())
}

/// `gala install`.
pub fn install(host: &mut dyn Host, source: &Source, req: &Request) -> Result<Outcome, Error> {
    host.log(&format!(
        "install id={} version={} source={}",
        req.id,
        req.version.unwrap_or("latest"),
        source.root
    ));
    let layout = Layout::new(host)?;
    if let Some(i) = find_installed(host, req.id).ok().filter(|i| i.id.is_some()) {
        let want = match req.version {
            Some(v) => v.to_string(),
            None => source.versions(host, req.id)?.remove(0),
        };
        if version::compare(&i.version, &want) == Ordering::Equal {
            host.log(&format!("already-installed id={} version={}", req.id, i.version));
            return Ok(Outcome::AlreadyInstalled {
                id: i.id.unwrap_or_default(),
                version: i.version,
            });
        }
        return Err(Error::failed(format!(
            "{} {} is installed; `gala upgrade {}` installs {want} in its place",
            req.id, i.version, req.id
        )));
    }
    let p = prepare(host, source, req.id, req.version)?;
    if exe::is_program(&p.installer.installer_type) {
        let location = exe::install_program(host, &p, req.source_location, req.location, None)?;
        host.log(&format!("installed id={} version={}", p.manifest.id, p.manifest.version));
        return Ok(Outcome::Installed {
            id: p.manifest.id.clone(),
            version: p.manifest.version.clone(),
            commands: Vec::new(),
            location,
        });
    }
    if req.location.is_some() {
        return Err(Error::usage(format!(
            "--location: {} installs into its package directory (winget's portable layout)",
            p.installer.effective_type()
        )));
    }
    let mut tx = Tx::new(host);
    match apply(&mut tx, &layout, &p, req.source_location) {
        Ok(location) => {
            tx.commit();
            host.log(&format!("installed id={} version={}", p.manifest.id, p.manifest.version));
            Ok(Outcome::Installed {
                id: p.manifest.id.clone(),
                version: p.manifest.version.clone(),
                commands: p.plan.commands.iter().map(|c| c.alias.clone()).collect(),
                location,
            })
        }
        Err(e) => Err(fail(tx, e)),
    }
}

/// Rolls a failed transaction back; the error says how it ended.
fn fail(tx: Tx, mut e: Error) -> Error {
    let host_failures = tx.rollback();
    if host_failures == 0 {
        e.message.push_str(" (rolled back: nothing was changed)");
    } else {
        e.message.push_str(&format!(
            " (rollback left {host_failures} change(s) behind: see the install log)"
        ));
    }
    e.kind = match e.kind {
        Kind::HashMismatch | Kind::Unsupported => e.kind,
        _ => Kind::Failed,
    };
    e
}

/// `gala uninstall`.
pub fn uninstall(host: &mut dyn Host, what: &str) -> Result<Outcome, Error> {
    host.log(&format!("uninstall what={what}"));
    let layout = Layout::new(host)?;
    let i = find_installed(host, what)?;
    if let Some(t) = exe::installed_type(host, &i).filter(|t| exe::is_program(t)) {
        exe::uninstall_program(host, &i, &t)?;
        host.log(&format!("uninstalled key={}", i.key));
        return Ok(Outcome::Uninstalled { id: i.id.unwrap_or(i.key), version: i.version });
    }
    let mut tx = Tx::new(host);
    match remove(&mut tx, &layout, &i) {
        Ok(()) => {
            tx.commit();
            host.log(&format!("uninstalled key={}", i.key));
            Ok(Outcome::Uninstalled { id: i.id.unwrap_or(i.key), version: i.version })
        }
        Err(e) => Err(fail(tx, e)),
    }
}

/// An installed package with a later version in its source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Available {
    pub installed: Installed,
    pub available: String,
}

/// The source to use for an installed package: `--source`, else the one
/// it was installed from.
fn source_for<'a>(
    given: Option<&'a Source>,
    i: &Installed,
    owned: &'a mut Option<Source>,
) -> Result<&'a Source, Error> {
    if let Some(s) = given {
        return Ok(s);
    }
    let loc = i
        .source
        .as_deref()
        .ok_or_else(|| Error::usage(format!("{}: which source? (--source <dir>)", i.name)))?;
    *owned = Some(Source::open(loc)?);
    Ok(owned.as_ref().unwrap_or_else(|| unreachable!()))
}

/// The upgrades available (all gala/winget installs, or `only`).
pub fn upgrades(
    host: &mut dyn Host,
    source: Option<&Source>,
    only: Option<&str>,
) -> Result<Vec<Available>, Error> {
    let mut out = Vec::new();
    let candidates: Vec<Installed> = match only {
        Some(what) => alloc::vec![find_installed(host, what)?],
        None => installed(host)
            .into_iter()
            .filter(|i| i.id.is_some() && i.root == Root::CurrentUser)
            .collect(),
    };
    for i in candidates {
        let Some(id) = i.id.clone() else { continue };
        let mut owned = None;
        let s = match source_for(source, &i, &mut owned) {
            Ok(s) => s,
            Err(e) if only.is_some() => return Err(e),
            Err(_) => continue,
        };
        let versions = match s.versions(host, &id) {
            Ok(v) => v,
            Err(e) if only.is_some() => return Err(e),
            Err(_) => continue,
        };
        if version::compare(&versions[0], &i.version) == Ordering::Greater {
            out.push(Available { installed: i, available: versions[0].clone() });
        }
    }
    Ok(out)
}

/// `gala upgrade <id>`: the old version is removed and the new one
/// installed in one transaction (a failure puts the old one back).
pub fn upgrade(
    host: &mut dyn Host,
    source: Option<&Source>,
    what: &str,
    version: Option<&str>,
) -> Result<Outcome, Error> {
    host.log(&format!("upgrade what={what}"));
    let layout = Layout::new(host)?;
    let i = find_installed(host, what)?;
    let Some(id) = i.id.clone() else {
        return Err(Error::unsupported(format!("{} was not installed by gala or winget", i.name)));
    };
    let mut owned = None;
    let s = source_for(source, &i, &mut owned)?;
    let target = match version {
        Some(v) => v.to_string(),
        None => s.versions(host, &id)?.remove(0),
    };
    if version.is_none() && version::compare(&target, &i.version) != Ordering::Greater {
        host.log(&format!("up-to-date id={id} version={}", i.version));
        return Ok(Outcome::UpToDate { id, version: i.version });
    }
    let p = prepare(host, s, &id, Some(&target))?;
    let location = s.root.clone();
    let old_type = exe::installed_type(host, &i).unwrap_or_default();
    let (old_program, new_program) =
        (exe::is_program(&old_type), exe::is_program(&p.installer.installer_type));
    if old_program || new_program {
        if !(old_program && new_program) {
            return Err(Error::unsupported(format!(
                "upgrading {id} from a {old_type} install to a {} one",
                p.installer.installer_type
            )));
        }
        // winget's default for these: the new installer runs over the old
        // version (UpgradeBehavior install).
        exe::install_program(host, &p, &location, None, Some(&i))?;
        host.log(&format!("upgraded id={id} from={} to={}", i.version, p.manifest.version));
        return Ok(Outcome::Upgraded { id, from: i.version, to: p.manifest.version.clone() });
    }
    let mut tx = Tx::new(host);
    let r = remove(&mut tx, &layout, &i).and_then(|()| apply(&mut tx, &layout, &p, &location));
    match r {
        Ok(_) => {
            tx.commit();
            host.log(&format!("upgraded id={id} from={} to={}", i.version, p.manifest.version));
            Ok(Outcome::Upgraded { id, from: i.version, to: p.manifest.version.clone() })
        }
        Err(e) => Err(fail(tx, e)),
    }
}

pub mod exe;
#[cfg(test)]
mod tests;
