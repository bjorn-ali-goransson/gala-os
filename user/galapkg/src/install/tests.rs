//! The install transaction against an in-memory machine: what an install
//! writes, that uninstall takes all of it away, that upgrades replace,
//! and that a failure at any single system call leaves the machine as it
//! was (every step of install, upgrade and uninstall is made to fail in
//! turn). The packages are pk_install_test's source tree, read from the
//! repository.

use super::*;
use crate::source::Source;
use alloc::collections::BTreeMap;
use alloc::vec;
use std::path::Path;

/// A file's contents, or a directory.
type Node = Option<Vec<u8>>;

/// A key as created and its values.
type Key = (String, Vec<(String, RegValue)>);

#[derive(Clone, Default, PartialEq, Eq, Debug)]
struct Machine {
    /// Lower-case path -> (path as created, node).
    files: BTreeMap<String, (String, Node)>,
    /// (root, lower-case key) -> (key as created, values).
    keys: BTreeMap<(u8, String), Key>,
}

struct Fake {
    m: Machine,
    /// The mutating call (counted from 0) that fails, once.
    fail_at: Option<usize>,
    calls: usize,
    log: Vec<String>,
    missing: Vec<String>,
    /// Command lines run.
    runs: Vec<String>,
    /// GalaTest.InnoSetup's stand-in behaves as a real Inno Setup
    /// installer (else it registers nothing, as in pk_install_test).
    inno_registers: bool,
    /// An NSIS uninstaller's copy still running: the key it removes after
    /// this many sleeps.
    pending: Option<(String, u32)>,
    sleeps: u32,
    /// URLs fetched, in order.
    fetches: Vec<String>,
    /// The caller is an administrator (Host::is_admin).
    admin: bool,
    /// GalaTest.InnoSetup's stand-in fails as Inno does, writing its log.
    inno_fails: bool,
}

fn root_n(r: Root) -> u8 {
    match r {
        Root::CurrentUser => 0,
        Root::LocalMachine => 1,
    }
}

fn lc(s: &str) -> String {
    s.to_ascii_lowercase()
}

const LOCAL: &str = "C:\\Users\\gala\\AppData\\Local";
const PROGRAM_FILES: &str = "C:\\Program Files";
const GALA: &str = "C:\\gala\\gala.exe";
const SRC: &str = "C:\\pk_source";

impl Fake {
    fn new() -> Fake {
        let mut f = Fake {
            m: Machine::default(),
            fail_at: None,
            calls: 0,
            log: Vec::new(),
            missing: Vec::new(),
            runs: Vec::new(),
            inno_registers: false,
            pending: None,
            sleeps: 0,
            fetches: Vec::new(),
            admin: false,
            inno_fails: false,
        };
        for d in [
            "C:",
            "C:\\gala",
            "C:\\Users",
            "C:\\Users\\gala",
            "C:\\Users\\gala\\AppData",
            LOCAL,
            PROGRAM_FILES,
        ] {
            f.m.files.insert(lc(d), (d.into(), None));
        }
        f.m.files.insert(lc(GALA), (GALA.into(), Some(b"MZ gala.exe".to_vec())));
        for (root, key) in [(0, UNINSTALL_KEY), (1, UNINSTALL_KEY), (1, MACHINE_ENVIRONMENT_KEY)] {
            let mut k = key;
            loop {
                f.m.keys.insert((root, lc(k)), (k.into(), vec![]));
                if !k.contains('\\') {
                    break;
                }
                k = parent_of(k);
            }
        }
        f.m.keys.insert(
            (0, lc(ENVIRONMENT_KEY)),
            (
                ENVIRONMENT_KEY.into(),
                vec![(
                    "TEMP".into(),
                    RegValue::ExpandSz("%USERPROFILE%\\AppData\\Local\\Temp".into()),
                )],
            ),
        );
        // The machine's environment, as gala's registry seed has it.
        f.m.keys.insert(
            (1, lc(MACHINE_ENVIRONMENT_KEY)),
            (
                MACHINE_ENVIRONMENT_KEY.into(),
                vec![(
                    "Path".into(),
                    RegValue::ExpandSz("%SystemRoot%\\system32;%SystemRoot%".into()),
                )],
            ),
        );
        // A program installed by an MSI, machine-wide.
        let k = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{1234}";
        f.m.keys.insert(
            (1, lc(k)),
            (k.into(), vec![("DisplayName".into(), RegValue::Sz("Some MSI".into()))]),
        );
        f.load_source();
        f
    }

    /// pk_install_test's source tree as C:\pk_source.
    fn load_source(&mut self) {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../pk_install_test/source");
        self.m.files.insert(lc(SRC), (SRC.into(), None));
        fn walk(f: &mut Fake, host_dir: &Path, path: &str) {
            for e in std::fs::read_dir(host_dir).unwrap() {
                let e = e.unwrap();
                let name = e.file_name().into_string().unwrap();
                let p = format!("{path}\\{name}");
                if e.file_type().unwrap().is_dir() {
                    f.m.files.insert(lc(&p), (p.clone(), None));
                    walk(f, &e.path(), &p);
                } else {
                    f.m.files.insert(lc(&p), (p, Some(std::fs::read(e.path()).unwrap())));
                }
            }
        }
        walk(self, &dir, SRC);
    }

    fn mutate(&mut self) -> Result<(), HostError> {
        let n = self.calls;
        self.calls += 1;
        if self.fail_at == Some(n) { Err(HostError { code: 112 }) } else { Ok(()) }
    }

    fn file(&self, path: &str) -> Option<&Vec<u8>> {
        self.m.files.get(&lc(path)).and_then(|(_, n)| n.as_ref())
    }

    fn value(&self, root: Root, key: &str, name: &str) -> Option<RegValue> {
        let (_, values) = self.m.keys.get(&(root_n(root), lc(key)))?;
        values.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
    }
}

/// What differs between two machines (for assertion messages).
fn diff(a: &Machine, b: &Machine) -> String {
    let mut out = String::new();
    for (k, v) in &a.files {
        if b.files.get(k) != Some(v) {
            out.push_str(&format!(
                "file {k}: {:?} vs {:?}\n",
                v.1.as_ref().map(Vec::len),
                b.files.get(k).map(|x| x.1.as_ref().map(Vec::len))
            ));
        }
    }
    for k in b.files.keys().filter(|k| !a.files.contains_key(*k)) {
        out.push_str(&format!("file {k}: only in the second\n"));
    }
    for (k, v) in &a.keys {
        if b.keys.get(k) != Some(v) {
            out.push_str(&format!("key {k:?}: {v:?} vs {:?}\n", b.keys.get(k)));
        }
    }
    for k in b.keys.keys().filter(|k| !a.keys.contains_key(*k)) {
        out.push_str(&format!("key {k:?}: only in the second {:?}\n", b.keys[k]));
    }
    out
}

fn parent_of(path: &str) -> &str {
    path.rfind('\\').map_or("", |i| &path[..i])
}

impl Host for Fake {
    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, HostError> {
        self.file(path).cloned().ok_or(HostError { code: 2 })
    }
    fn create_file(&mut self, path: &str, data: &[u8]) -> Result<(), HostError> {
        if self.m.files.contains_key(&lc(path)) {
            return Err(HostError { code: 80 });
        }
        if !matches!(self.m.files.get(&lc(parent_of(path))), Some((_, None))) {
            return Err(HostError { code: 3 });
        }
        self.mutate()?;
        self.m.files.insert(lc(path), (path.into(), Some(data.to_vec())));
        Ok(())
    }
    fn delete_file(&mut self, path: &str) -> Result<(), HostError> {
        match self.m.files.get(&lc(path)) {
            Some((_, Some(_))) => {}
            _ => return Err(HostError { code: 2 }),
        }
        self.mutate()?;
        self.m.files.remove(&lc(path));
        Ok(())
    }
    fn create_dir(&mut self, path: &str) -> Result<(), HostError> {
        if self.m.files.contains_key(&lc(path)) {
            return Err(HostError { code: 183 });
        }
        if !matches!(self.m.files.get(&lc(parent_of(path))), Some((_, None))) {
            return Err(HostError { code: 3 });
        }
        self.mutate()?;
        self.m.files.insert(lc(path), (path.into(), None));
        Ok(())
    }
    fn remove_dir(&mut self, path: &str) -> Result<(), HostError> {
        let prefix = format!("{}\\", lc(path));
        if self.m.files.keys().any(|k| k.starts_with(&prefix)) {
            return Err(HostError { code: 145 });
        }
        if !matches!(self.m.files.get(&lc(path)), Some((_, None))) {
            return Err(HostError { code: 3 });
        }
        self.mutate()?;
        self.m.files.remove(&lc(path));
        Ok(())
    }
    fn exists(&mut self, path: &str) -> Option<bool> {
        self.m.files.get(&lc(path)).map(|(_, n)| n.is_none())
    }
    fn list_dir(&mut self, path: &str) -> Result<Vec<DirEntry>, HostError> {
        if !matches!(self.m.files.get(&lc(path)), Some((_, None))) {
            return Err(HostError { code: 3 });
        }
        let want = lc(path);
        Ok(self
            .m
            .files
            .iter()
            .filter(|(k, _)| parent_of(k) == want)
            .map(|(_, (orig, n))| DirEntry {
                name: orig.rsplit('\\').next().unwrap().into(),
                is_dir: n.is_none(),
            })
            .collect())
    }
    fn reg_get(&mut self, root: Root, key: &str, name: &str) -> Option<RegValue> {
        self.value(root, key, name)
    }
    fn reg_values(&mut self, root: Root, key: &str) -> Result<Vec<(String, RegValue)>, HostError> {
        self.m
            .keys
            .get(&(root_n(root), lc(key)))
            .map(|(_, v)| v.clone())
            .ok_or(HostError { code: 2 })
    }
    fn reg_subkeys(&mut self, root: Root, key: &str) -> Vec<String> {
        let want = lc(key);
        self.m
            .keys
            .iter()
            .filter(|((r, k), _)| *r == root_n(root) && parent_of(k) == want)
            .map(|(_, (orig, _))| orig.rsplit('\\').next().unwrap().into())
            .collect()
    }
    fn reg_key_exists(&mut self, root: Root, key: &str) -> bool {
        self.m.keys.contains_key(&(root_n(root), lc(key)))
    }
    fn reg_set(
        &mut self,
        root: Root,
        key: &str,
        name: &str,
        value: &RegValue,
    ) -> Result<(), HostError> {
        self.mutate()?;
        // The key and its parents, as RegCreateKeyEx makes them.
        let mut k = key;
        loop {
            self.m.keys.entry((root_n(root), lc(k))).or_insert_with(|| (k.into(), vec![]));
            if !k.contains('\\') {
                break;
            }
            k = parent_of(k);
        }
        let (_, values) = self.m.keys.get_mut(&(root_n(root), lc(key))).unwrap();
        match values.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            Some(slot) => slot.1 = value.clone(),
            None => values.push((name.into(), value.clone())),
        }
        Ok(())
    }
    fn reg_delete_value(&mut self, root: Root, key: &str, name: &str) -> Result<(), HostError> {
        let Some((_, values)) = self.m.keys.get(&(root_n(root), lc(key))) else {
            return Err(HostError { code: 2 });
        };
        if !values.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) {
            return Err(HostError { code: 2 });
        }
        self.mutate()?;
        let (_, values) = self.m.keys.get_mut(&(root_n(root), lc(key))).unwrap();
        values.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        Ok(())
    }
    fn reg_delete_key(&mut self, root: Root, key: &str) -> Result<(), HostError> {
        if !self.reg_subkeys(root, key).is_empty() {
            return Err(HostError { code: 5 });
        }
        if !self.m.keys.contains_key(&(root_n(root), lc(key))) {
            return Err(HostError { code: 2 });
        }
        self.mutate()?;
        self.m.keys.remove(&(root_n(root), lc(key)));
        Ok(())
    }
    fn fetch(&mut self, url: &str) -> Result<Vec<u8>, HostError> {
        self.fetches.push(url.into());
        // GitHub's REST API (directory listings, its JSON shape) and raw
        // files, answered from pk_install_test's snapshot as if it were
        // microsoft/winget-pkgs at master.
        if let Some(rest) =
            url.strip_prefix("https://api.github.com/repos/microsoft/winget-pkgs/contents/")
        {
            let path = rest.strip_suffix("?ref=master").ok_or(HostError { code: 12 })?;
            let dir =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../pk_install_test/source").join(path);
            return listing(&dir, |name, is_dir| {
                format!(
                    "{{\"name\": \"{name}\", \"path\": \"{path}/{name}\", \"sha\": \"0\", \"size\": 0, \"type\": \"{}\", \"_links\": {{\"self\": \"x\"}}}}",
                    if is_dir { "dir" } else { "file" }
                )
            });
        }
        if let Some(path) =
            url.strip_prefix("https://raw.githubusercontent.com/microsoft/winget-pkgs/master/")
        {
            let file =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../pk_install_test/source").join(path);
            return std::fs::read(file).map_err(|_| HostError { code: HTTP_STATUS_ERROR | 404 });
        }
        // Nginx's winget-pkgs tree (autoindex_format json) and a listing
        // that is not JSON.
        if let Some(path) = url.strip_prefix("https://localhost:8443/winget-pkgs/") {
            let local = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../pk_install_test/served/winget-pkgs")
                .join(path.trim_end_matches('/'));
            if path.ends_with('/') || path.is_empty() {
                return listing(&local, |name, is_dir| {
                    format!(
                        "{{ \"name\":\"{name}\", \"type\":\"{}\", \"mtime\":\"Tue, 03 Nov 2026 00:00:00 GMT\" }}",
                        if is_dir { "directory" } else { "file" }
                    )
                });
            }
            return std::fs::read(local).map_err(|_| HostError { code: HTTP_STATUS_ERROR | 404 });
        }
        if url.starts_with("https://localhost:8443/html-index/") {
            return Ok(b"<html><body>Index of /</body></html>".to_vec());
        }
        // What schannel_test's Nginx serves to pk_install_https (the
        // redirect is winhttp's, not seen here); 8444's certificate is
        // untrusted; another host is not reachable from the tests.
        if let Some(name) = url.strip_prefix("https://localhost:8443/releases/download/v1.0.0/") {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../pk_install_test/served/objects")
                .join(name);
            return std::fs::read(path).map_err(|_| HostError { code: HTTP_STATUS_ERROR | 404 });
        }
        if url.starts_with("https://localhost:8444/") {
            return Err(HostError { code: 12175 });
        }
        Err(HostError { code: 50 })
    }
    fn env(&mut self, name: &str) -> Option<String> {
        if name.eq_ignore_ascii_case("ProgramFiles") {
            return Some(PROGRAM_FILES.into());
        }
        if name.eq_ignore_ascii_case("SystemRoot") {
            return Some("C:\\Windows".into());
        }
        name.eq_ignore_ascii_case("LOCALAPPDATA").then(|| LOCAL.into())
    }
    fn is_admin(&mut self) -> bool {
        self.admin
    }
    fn self_exe(&mut self) -> String {
        GALA.into()
    }
    fn log(&mut self, line: &str) {
        self.log.push(line.into());
    }
    fn record_missing(&mut self, what: &str) {
        self.missing.push(what.into());
    }
    fn today(&mut self) -> String {
        "20261103".into()
    }
    fn run(&mut self, command_line: &str, _timeout_ms: u32) -> Result<u32, HostError> {
        self.mutate()?;
        self.runs.push(command_line.into());
        let (exe, args) = split_command(command_line);
        let name = lc(exe.rsplit('\\').next().unwrap());
        if name.starts_with("pknsis-") {
            // An NSIS installer: /S (no UI), /D=<dir> last.
            assert!(self.file(&exe).is_some(), "the installer is staged: {exe}");
            assert!(args.split(' ').any(|a| a == "/S"), "{command_line}");
            let version = name.trim_start_matches("pknsis-").trim_end_matches(".exe").to_string();
            let dir = match args.find("/D=") {
                Some(i) => args[i + 3..].to_string(),
                None => format!("{LOCAL}\\Programs\\PkNsis"),
            };
            self.vendor_install(
                &dir,
                &["pknsis.exe", "README.txt", "uninstall.exe"],
                "GalaTest.PkNsis",
                &[
                    ("DisplayName", "PkNsis"),
                    ("DisplayVersion", &version),
                    ("Publisher", "gala-os"),
                    ("InstallLocation", &dir),
                    ("UninstallString", &format!("\"{dir}\\uninstall.exe\"")),
                ],
            );
            return Ok(0);
        }
        if name == "innosetup-2.0.exe" && self.inno_fails {
            // Inno Setup failing to start: its /LOG file says why, exit 1.
            let i = args.find("/LOG=\"").unwrap();
            let log = args[i + 6..].split('"').next().unwrap().to_string();
            let text = "\u{feff}Log opened.\r\nSetup version: Inno Setup version 7.1.1\r\n\r\nInitialization error: no.\r\n";
            self.m.files.insert(lc(&log), (log.clone(), Some(text.as_bytes().to_vec())));
            return Ok(1);
        }
        if name == "innosetup-2.0.exe" {
            assert!(
                (args.contains("/VERYSILENT") || args.contains("/SILENT"))
                    && args.contains("/SUPPRESSMSGBOXES"),
                "{args}"
            );
            if !self.inno_registers {
                return Ok(0);
            }
            let dir = match args.find("/DIR=\"") {
                Some(i) => args[i + 6..].split('"').next().unwrap().to_string(),
                None => format!("{LOCAL}\\Programs\\InnoSetup"),
            };
            let unins = format!("\"{dir}\\unins000.exe\"");
            self.vendor_install(
                &dir,
                &["inno.exe", "unins000.exe", "unins000.dat"],
                "{8E2A0D4C-GALA}_is1",
                &[
                    ("DisplayName", "InnoSetup version 2.0"),
                    ("DisplayVersion", "2.0"),
                    ("InstallLocation", &format!("{dir}\\")),
                    ("UninstallString", &unins),
                    ("QuietUninstallString", &format!("{unins} /SILENT")),
                ],
            );
            return Ok(0);
        }
        if name == "uninstall.exe" {
            // NSIS: the copy in %TEMP% does the work after this exits.
            assert_eq!(args, "/S");
            self.pending = Some((format!("{UNINSTALL_KEY}\\GalaTest.PkNsis"), 3));
            self.vendor_remove(parent_of(&exe).to_string());
            return Ok(0);
        }
        if name == "unins000.exe" {
            assert_eq!(args, "/SILENT");
            self.vendor_remove(parent_of(&exe).to_string());
            let k = format!("{UNINSTALL_KEY}\\{{8E2A0D4C-GALA}}_is1");
            self.m.keys.remove(&(0, lc(&k)));
            return Ok(0);
        }
        Err(HostError { code: 2 })
    }
    fn sleep(&mut self, _ms: u32) {
        self.sleeps += 1;
        if let Some((k, n)) = self.pending.take() {
            if n <= 1 {
                self.m.keys.remove(&(0, lc(&k)));
            } else {
                self.pending = Some((k, n - 1));
            }
        }
    }
}

/// A JSON directory listing of a host directory, each entry made by
/// `entry(name, is_dir)`; 404 when there is no such directory.
fn listing(dir: &Path, entry: impl Fn(&str, bool) -> String) -> Result<Vec<u8>, HostError> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Err(HostError { code: HTTP_STATUS_ERROR | 404 });
    };
    let mut items: Vec<String> = rd
        .map(|e| {
            let e = e.unwrap();
            entry(&e.file_name().into_string().unwrap(), e.file_type().unwrap().is_dir())
        })
        .collect();
    items.sort();
    Ok(format!("[\n{}\n]", items.join(",\n")).into_bytes())
}

/// A command line's program (unquoted) and the rest.
fn split_command(line: &str) -> (String, &str) {
    if let Some(rest) = line.strip_prefix('"') {
        let end = rest.find('"').unwrap();
        (rest[..end].into(), rest[end + 1..].trim())
    } else {
        let end = line.find(' ').unwrap_or(line.len());
        (line[..end].into(), line[end..].trim())
    }
}

impl Fake {
    /// What a vendor's installer does: its files and its ARP entry (not
    /// through `mutate`: it is another program).
    fn vendor_install(&mut self, dir: &str, files: &[&str], key: &str, values: &[(&str, &str)]) {
        let mut d = String::new();
        for part in dir.split('\\') {
            if !d.is_empty() {
                d.push('\\');
            }
            d.push_str(part);
            self.m.files.entry(lc(&d)).or_insert_with(|| (d.clone(), None));
        }
        for f in files {
            let p = format!("{dir}\\{f}");
            self.m.files.insert(lc(&p), (p, Some(b"MZ vendor".to_vec())));
        }
        let k = format!("{UNINSTALL_KEY}\\{key}");
        let values =
            values.iter().map(|(n, v)| (n.to_string(), RegValue::Sz(v.to_string()))).collect();
        self.m.keys.insert((0, lc(&k)), (k, values));
    }

    fn vendor_remove(&mut self, dir: String) {
        let prefix = format!("{}\\", lc(&dir));
        self.m.files.retain(|k, _| *k != lc(&dir) && !k.starts_with(&prefix));
    }
}

fn source() -> Source {
    Source::open(SRC).unwrap()
}

fn install_id(f: &mut Fake, id: &str, version: Option<&str>) -> Result<Outcome, Error> {
    install_with(f, id, version, Options::default())
}

fn install_with(
    f: &mut Fake,
    id: &str,
    version: Option<&str>,
    options: Options,
) -> Result<Outcome, Error> {
    install(f, &source(), &Request { id, version, source_location: SRC, location: None, options })
}

fn upgrade_id(f: &mut Fake, what: &str) -> Result<Outcome, Error> {
    upgrade(f, None, what, None, &Options::default())
}

const KEY: &str = "GalaTest.PkHello_Microsoft.Winget.Source_8wekyb3d8bbwe";

fn pkg_dir() -> String {
    format!("{LOCAL}\\Microsoft\\WinGet\\Packages\\{KEY}")
}

fn links() -> String {
    format!("{LOCAL}\\Microsoft\\WinGet\\Links")
}

#[test]
fn source_versions_and_search() {
    let mut f = Fake::new();
    let s = source();
    assert_eq!(
        s.versions(&mut f, "galatest.pkhello").unwrap(),
        vec!["1.1.0".to_string(), "1.0.0".to_string()]
    );
    // ripgrep's directory holds only the MSVC package, not versions.
    assert!(s.versions(&mut f, "BurntSushi.ripgrep").is_err());
    assert_eq!(s.versions(&mut f, "BurntSushi.ripgrep.MSVC").unwrap(), vec!["15.2.0".to_string()]);
    assert_eq!(s.find(&mut f, "GalaTest.PkHello", Some("1.0")).unwrap().version, "1.0.0");
    assert_eq!(s.find(&mut f, "GalaTest.PkHello", Some("9")).unwrap_err().kind, Kind::NotFound);
    assert_eq!(s.find(&mut f, "No.Such", None).unwrap_err().kind, Kind::NotFound);
    assert_eq!(s.find(&mut f, "../x", None).unwrap_err().kind, Kind::Usage);
    let hits = s.search(&mut f, "grep").unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "BurntSushi.ripgrep.MSVC");
    assert_eq!(hits[0].matched, "Id");
    let hits = s.search(&mut f, "json").unwrap();
    assert!(hits.is_empty(), "{hits:?}");
    let hits = s.search(&mut f, "hello").unwrap();
    assert_eq!(hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec!["GalaTest.PkHello"]);
    assert_eq!(hits[0].version, "1.1.0");
    let hits = s.search(&mut f, "galatest.pkhello").unwrap();
    assert_eq!(hits[0].id, "GalaTest.PkHello");
}

#[test]
fn every_test_package_manifest_parses() {
    let mut f = Fake::new();
    let s = source();
    for (id, v) in [
        ("GalaTest.PkHello", "1.0.0"),
        ("GalaTest.PkHello", "1.1.0"),
        ("GalaTest.Conflict", "1.0.0"),
        ("GalaTest.BadHash", "1.0.0"),
        ("GalaTest.InnoSetup", "2.0"),
        ("GalaTest.Msix", "1.0.0"),
        ("GalaTest.PkNsis", "1.0.0"),
        ("GalaTest.PkNsis", "1.1.0"),
        ("GalaTest.PkLib", "1.0.0"),
        ("GalaTest.PkDep", "1.0.0"),
        ("GalaTest.PkDepMissing", "1.0.0"),
        ("GalaTest.PkScoped", "1.0.0"),
        ("GalaTest.PkScoped", "1.1.0"),
        ("jqlang.jq", "1.8.2"),
    ] {
        let m = s.manifest(&mut f, id, v).unwrap_or_else(|e| panic!("{id} {v}: {e}"));
        assert!(m.warnings.is_empty(), "{id} {v}: {:?}", m.warnings);
    }
}

#[test]
fn install_list_run_uninstall() {
    let mut f = Fake::new();
    let before = f.m.clone();
    let out = install_id(&mut f, "galatest.pkhello", Some("1.0.0")).unwrap();
    assert_eq!(
        out,
        Outcome::Installed {
            id: "GalaTest.PkHello".into(),
            version: "1.0.0".into(),
            commands: vec!["pkhello".into()],
            location: pkg_dir(),
            dependencies: vec![],
        }
    );
    let hello =
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../bin/hello.exe")).unwrap();
    assert_eq!(f.file(&format!("{}\\pkhello-1.0.0\\pkhello.exe", pkg_dir())), Some(&hello));
    assert!(
        f.file(&format!("{}\\pkhello-1.0.0\\README.txt", pkg_dir()))
            .unwrap()
            .starts_with(b"PkHello 1.0.0")
    );
    assert!(f.file(&format!("{}\\pkhello-1.0.0\\doc\\LICENSE.txt", pkg_dir())).is_some());
    // The command: a copy of gala.exe and its shim.
    assert_eq!(f.file(&format!("{}\\pkhello.exe", links())), Some(&b"MZ gala.exe".to_vec()));
    let shim_text =
        String::from_utf8(f.file(&format!("{}\\pkhello.shim", links())).unwrap().clone()).unwrap();
    assert_eq!(shim::parse(&shim_text), Some(format!("{}\\pkhello-1.0.0\\pkhello.exe", pkg_dir())));
    // PATH.
    assert_eq!(
        f.value(Root::CurrentUser, ENVIRONMENT_KEY, "Path"),
        Some(RegValue::ExpandSz(links()))
    );
    // The uninstall record.
    let arp = format!("{UNINSTALL_KEY}\\{KEY}");
    let v = |f: &Fake, n: &str| f.value(Root::CurrentUser, &arp, n);
    assert_eq!(v(&f, "DisplayName"), Some(RegValue::Sz("PkHello".into())));
    assert_eq!(v(&f, "DisplayVersion"), Some(RegValue::Sz("1.0.0".into())));
    assert_eq!(v(&f, "Publisher"), Some(RegValue::Sz("gala-os".into())));
    assert_eq!(v(&f, "InstallLocation"), Some(RegValue::Sz(pkg_dir())));
    assert_eq!(v(&f, "WinGetPackageIdentifier"), Some(RegValue::Sz("GalaTest.PkHello".into())));
    assert_eq!(v(&f, "WinGetInstallerType"), Some(RegValue::Sz("portable".into())));
    assert_eq!(v(&f, "InstallDate"), Some(RegValue::Sz("20261103".into())));
    assert_eq!(
        v(&f, "UninstallString"),
        Some(RegValue::Sz(format!("\"{GALA}\" uninstall --product-code {KEY}")))
    );
    // Listed, with the MSI.
    let list = installed(&mut f);
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id.as_deref(), Some("GalaTest.PkHello"));
    assert_eq!(list[1].name, "Some MSI");
    assert_eq!(list[1].root, Root::LocalMachine);
    // Installing again changes nothing.
    let again = install_id(&mut f, "GalaTest.PkHello", Some("1.0")).unwrap();
    assert_eq!(
        again,
        Outcome::AlreadyInstalled { id: "GalaTest.PkHello".into(), version: "1.0.0".into() }
    );
    // Another version is upgrade's job.
    assert_eq!(install_id(&mut f, "GalaTest.PkHello", None).unwrap_err().kind, Kind::Failed);
    // An MSI's entry is not gala's to remove.
    assert_eq!(uninstall(&mut f, "Some MSI").unwrap_err().kind, Kind::Unsupported);
    // Uninstall: everything but the Links directory and the PATH entry
    // (winget keeps both).
    let out = uninstall(&mut f, "GalaTest.PkHello").unwrap();
    assert_eq!(
        out,
        Outcome::Uninstalled { id: "GalaTest.PkHello".into(), version: "1.0.0".into() }
    );
    let mut after = f.m.clone();
    for d in [
        links(),
        format!("{LOCAL}\\Microsoft\\WinGet\\Packages"),
        format!("{LOCAL}\\Microsoft\\WinGet"),
        format!("{LOCAL}\\Microsoft"),
    ] {
        assert!(after.files.remove(&lc(&d)).is_some(), "{d}");
    }
    let env = after.keys.get_mut(&(0, lc(ENVIRONMENT_KEY))).unwrap();
    env.1.retain(|(n, _)| n != "Path");
    assert!(after == before, "{}", diff(&after, &before));
    assert_eq!(uninstall(&mut f, "GalaTest.PkHello").unwrap_err().kind, Kind::NotFound);
    // By product code, too.
    install_id(&mut f, "GalaTest.PkHello", None).unwrap();
    uninstall(&mut f, KEY).unwrap();
}

#[test]
fn upgrade_replaces_the_old_version() {
    let mut f = Fake::new();
    install_id(&mut f, "GalaTest.PkHello", Some("1.0.0")).unwrap();
    let ups = upgrades(&mut f, None, None).unwrap();
    assert_eq!(ups.len(), 1);
    assert_eq!(ups[0].available, "1.1.0");
    let out = upgrade_id(&mut f, "GalaTest.PkHello").unwrap();
    assert_eq!(
        out,
        Outcome::Upgraded {
            id: "GalaTest.PkHello".into(),
            from: "1.0.0".into(),
            to: "1.1.0".into()
        }
    );
    assert!(f.file(&format!("{}\\pkhello-1.0.0\\README.txt", pkg_dir())).is_none());
    assert!(
        f.file(&format!("{}\\pkhello-1.1.0\\README.txt", pkg_dir()))
            .unwrap()
            .starts_with(b"PkHello 1.1.0")
    );
    let shim_text =
        String::from_utf8(f.file(&format!("{}\\pkhello.shim", links())).unwrap().clone()).unwrap();
    assert!(shim_text.contains("pkhello-1.1.0\\pkhello.exe"));
    let arp = format!("{UNINSTALL_KEY}\\{KEY}");
    assert_eq!(
        f.value(Root::CurrentUser, &arp, "DisplayVersion"),
        Some(RegValue::Sz("1.1.0".into()))
    );
    // PATH has the Links directory once.
    assert_eq!(
        f.value(Root::CurrentUser, ENVIRONMENT_KEY, "Path"),
        Some(RegValue::ExpandSz(links()))
    );
    assert_eq!(
        upgrade_id(&mut f, "GalaTest.PkHello").unwrap(),
        Outcome::UpToDate { id: "GalaTest.PkHello".into(), version: "1.1.0".into() }
    );
    assert!(upgrades(&mut f, None, None).unwrap().is_empty());
}

#[test]
fn checks_before_any_change() {
    let mut f = Fake::new();
    let before = f.m.clone();
    let e = install_id(&mut f, "GalaTest.BadHash", None).unwrap_err();
    assert_eq!(e.kind, Kind::HashMismatch);
    assert!(e.message.contains("SHA-256"), "{}", e.message);
    let e = install_id(&mut f, "GalaTest.Msix", None).unwrap_err();
    assert_eq!(e.kind, Kind::Unsupported);
    assert_eq!(f.missing, vec!["gala.exe!install(InstallerType msix)".to_string()]);
    // No mirrored installer: a download, which fails here.
    assert_eq!(install_id(&mut f, "jqlang.jq", None).unwrap_err().kind, Kind::Unsupported);
    assert_eq!(f.m, before);
    assert!(f.log.iter().any(|l| l.starts_with("sha256 expected=")));
}

#[test]
fn an_installer_not_mirrored_is_downloaded() {
    let mut f = Fake::new();
    let before = f.m.clone();
    // Fetched through the host (gala.exe: winhttp), its SHA-256 checked.
    install_id(&mut f, "GalaTest.PkHttps", None).unwrap();
    assert!(
        f.log.iter().any(|l| l
            == "download url=https://localhost:8443/releases/download/v1.0.0/pkhttps-1.0.0.zip")
    );
    assert!(f.log.iter().any(|l| l.starts_with("download bytes=")));
    assert!(f.exists(&format!("{LOCAL}\\Microsoft\\WinGet\\Links\\pkhttps.exe")) == Some(false));
    // A download that fails is the install's failure, with the reason
    // and the error in the log; nothing changes.
    let mut f = Fake::new();
    let e = install_id(&mut f, "GalaTest.PkUntrusted", None).unwrap_err();
    assert_eq!(e.kind, Kind::Failed);
    assert!(e.message.contains("not trusted") && e.message.contains("12175"), "{}", e.message);
    assert!(f.log.iter().any(|l| l.starts_with("error download code=12175")));
    assert_eq!(f.m, before);
    assert_eq!(
        HostError { code: HTTP_STATUS_ERROR | 404 }.to_string(),
        "the server answered HTTP 404"
    );
}

#[test]
fn a_taken_command_rolls_the_install_back() {
    let mut f = Fake::new();
    install_id(&mut f, "GalaTest.PkHello", None).unwrap();
    let before = f.m.clone();
    let e = install_id(&mut f, "GalaTest.Conflict", None).unwrap_err();
    assert_eq!(e.kind, Kind::Failed);
    assert!(e.message.contains("the command pkhello exists already"), "{}", e.message);
    assert!(e.message.contains("rolled back"), "{}", e.message);
    assert_eq!(f.m, before);
    assert!(f.log.iter().any(|l| l.starts_with("rollback remove file")));
}

/// Runs `op` with the n-th mutating call failing, for every n until it
/// succeeds: each failure must leave the machine exactly as before.
fn fail_each_step(
    setup: impl Fn(&mut Fake),
    op: impl Fn(&mut Fake) -> Result<Outcome, Error>,
) -> usize {
    for n in 0.. {
        let mut f = Fake::new();
        setup(&mut f);
        let before = f.m.clone();
        f.calls = 0;
        f.fail_at = Some(n);
        match op(&mut f) {
            Ok(_) => return n,
            Err(e) => {
                assert_eq!(e.kind, Kind::Failed, "step {n}: {e}");
                assert!(e.message.contains("rolled back"), "step {n}: {e}");
                assert!(f.m == before, "step {n}: {e}\n{}\n{:#?}", diff(&f.m, &before), f.log);
            }
        }
    }
    unreachable!()
}

#[test]
fn a_failure_at_any_step_rolls_back() {
    let steps = fail_each_step(|_| {}, |f| install_id(f, "GalaTest.PkHello", Some("1.0.0")));
    assert!(steps > 20, "{steps}");
    let setup = |f: &mut Fake| {
        install_id(f, "GalaTest.PkHello", Some("1.0.0")).unwrap();
    };
    fail_each_step(setup, |f| upgrade_id(f, "GalaTest.PkHello"));
    fail_each_step(setup, |f| uninstall(f, "GalaTest.PkHello"));
}

/// One installer (YAML of an `Installers` entry, indented by two) of a
/// singleton manifest, read by the winget reader.
fn installer_of(entry: &str) -> crate::winget::Installer {
    let text = format!(
        "PackageIdentifier: A.B\nPackageVersion: 1.0\nPackageLocale: en-US\nPublisher: P\nPackageName: N\nLicense: MIT\nShortDescription: d\nManifestType: singleton\nManifestVersion: 1.10.0\nInstallerSha256: {}\nInstallers:\n{entry}",
        "0".repeat(64)
    );
    crate::winget::Package::from_texts(&[&text]).unwrap().installers.remove(0)
}

#[test]
fn x86_portable_is_installable() {
    // WOW64 runs 32-bit programs: an x86 portable one is gala's to install,
    // as winget's choice (no fallback to another installer).
    let x86 = installer_of(
        "- Architecture: x86\n  InstallerType: portable\n  InstallerUrl: https://h/jq-windows-i386.exe\n",
    );
    assert!(gap(&x86).is_none());
}

#[test]
fn plans() {
    let portable = installer_of(
        "- Architecture: x64\n  InstallerType: portable\n  InstallerUrl: https://h/jq-windows-amd64.exe\n",
    );
    let p = plan(&portable, b"MZ").unwrap();
    assert_eq!(p.files[0].0, "jq-windows-amd64.exe");
    assert_eq!(
        p.commands,
        vec![Command { alias: "jq-windows-amd64".into(), target: "jq-windows-amd64.exe".into() }]
    );
    let named = installer_of(
        "- Architecture: x64\n  InstallerType: portable\n  InstallerUrl: https://h/jq-windows-amd64.exe\n  Commands: [jq]\n",
    );
    let p = plan(&named, b"MZ").unwrap();
    assert_eq!(p.files[0].0, "jq.exe");
    assert_eq!(p.commands[0].alias, "jq");
    let gala = installer_of(
        "- Architecture: x64\n  InstallerType: portable\n  InstallerUrl: https://h/g.exe\n  Commands: [gala]\n",
    );
    assert!(plan(&gala, b"MZ").is_err());
    // A zip with a path out of the package directory.
    let evil = crate::zip::writer::build(&[("../evil.exe", b"MZ")], true);
    let z = installer_of(
        "- Architecture: x64\n  InstallerType: zip\n  NestedInstallerType: portable\n  NestedInstallerFiles:\n  - RelativeFilePath: evil.exe\n  InstallerUrl: https://h/z.zip\n",
    );
    assert!(plan(&z, &evil).unwrap_err().message.contains("unsafe path"));
    let ok = crate::zip::writer::build(&[("a/b/tool.exe", b"MZ"), ("c.txt", b"c")], true);
    let z = installer_of(
        "- Architecture: x64\n  InstallerType: zip\n  NestedInstallerType: portable\n  NestedInstallerFiles:\n  - RelativeFilePath: a\\b\\TOOL.exe\n  ArchiveBinariesDependOnPath: true\n  InstallerUrl: https://h/z.zip\n",
    );
    let p = plan(&z, &ok).unwrap();
    assert_eq!(p.dirs, vec!["a".to_string(), "a\\b".to_string()]);
    assert_eq!(p.commands, vec![Command { alias: "TOOL".into(), target: "a\\b\\tool.exe".into() }]);
    assert!(p.dir_on_path);
    let msi = installer_of(
        "- Architecture: x64\n  InstallerType: msi\n  InstallerUrl: https://h/x.msi\n",
    );
    assert_eq!(plan(&msi, b"").unwrap_err().kind, Kind::Unsupported);
}

// ---- Installers that are programs (inno, nullsoft) --------------------

const NSIS_KEY: &str = "GalaTest.PkNsis";

fn nsis_arp() -> String {
    format!("{UNINSTALL_KEY}\\{NSIS_KEY}")
}

#[test]
fn nsis_install_upgrade_uninstall() {
    let mut f = Fake::new();
    let before = f.m.clone();
    let out = install_id(&mut f, "GalaTest.PkNsis", Some("1.0.0")).unwrap();
    let dir = format!("{LOCAL}\\Programs\\PkNsis");
    assert_eq!(
        out,
        Outcome::Installed {
            id: "GalaTest.PkNsis".into(),
            version: "1.0.0".into(),
            commands: vec![],
            location: dir.clone(),
            dependencies: vec![],
        }
    );
    // Run as winget runs NSIS: the staged installer with /S.
    let staged = format!(
        "{LOCAL}\\Temp\\gala\\GalaTest.PkNsis_Microsoft.Winget.Source_8wekyb3d8bbwe\\pknsis-1.0.0.exe"
    );
    assert_eq!(f.runs, vec![format!("\"{staged}\" /S")]);
    // The staged copy and its directories are gone.
    assert!(f.exists(&format!("{LOCAL}\\Temp")).is_none(), "{:#?}", f.log);
    assert!(f.file(&format!("{dir}\\pknsis.exe")).is_some());
    // The installer's entry, tagged by gala.
    let v = |f: &Fake, n: &str| f.value(Root::CurrentUser, &nsis_arp(), n);
    assert_eq!(v(&f, "DisplayVersion"), Some(RegValue::Sz("1.0.0".into())));
    assert_eq!(v(&f, "WinGetPackageIdentifier"), Some(RegValue::Sz("GalaTest.PkNsis".into())));
    assert_eq!(v(&f, "WinGetInstallerType"), Some(RegValue::Sz("nullsoft".into())));
    assert_eq!(v(&f, "GalaPackageVersion"), Some(RegValue::Sz("1.0.0".into())));
    assert_eq!(v(&f, "GalaSource"), Some(RegValue::Sz(SRC.into())));
    assert!(f.log.iter().any(|l| l == "exit code=0 (0x0)"), "{:#?}", f.log);
    assert!(
        f.log
            .iter()
            .any(|l| l.contains("arp-entry key=HKCU\\") && l.contains("found-by=the one new entry")),
        "{:#?}",
        f.log
    );
    let list = installed(&mut f);
    assert_eq!(list[0].id.as_deref(), Some("GalaTest.PkNsis"));
    assert_eq!(list[0].key, NSIS_KEY);
    // Again: already installed, nothing run.
    assert!(matches!(
        install_id(&mut f, "GalaTest.PkNsis", Some("1.0.0")).unwrap(),
        Outcome::AlreadyInstalled { .. }
    ));
    assert_eq!(f.runs.len(), 1);
    // Upgrade: the new installer over the old one; the same entry.
    let out = upgrade_id(&mut f, "GalaTest.PkNsis").unwrap();
    assert_eq!(
        out,
        Outcome::Upgraded {
            id: "GalaTest.PkNsis".into(),
            from: "1.0.0".into(),
            to: "1.1.0".into()
        }
    );
    assert_eq!(f.runs.len(), 2);
    assert!(f.runs[1].ends_with("pknsis-1.1.0.exe\" /S"), "{}", f.runs[1]);
    assert!(f.log.iter().any(|l| l.contains("found-by=the previous version's entry")));
    assert_eq!(v(&f, "DisplayVersion"), Some(RegValue::Sz("1.1.0".into())));
    assert_eq!(v(&f, "GalaPackageVersion"), Some(RegValue::Sz("1.1.0".into())));
    // Uninstall: UninstallString with /S; the entry goes a little later
    // (the uninstaller's copy), and gala waits for it.
    let out = uninstall(&mut f, "GalaTest.PkNsis").unwrap();
    assert_eq!(out, Outcome::Uninstalled { id: "GalaTest.PkNsis".into(), version: "1.1.0".into() });
    assert_eq!(f.runs[2], format!("\"{dir}\\uninstall.exe\" /S"));
    assert_eq!(f.sleeps, 3);
    assert!(f.log.iter().any(|l| l.starts_with("arp-entry-gone") && l.contains("waited=750ms")));
    // The machine as before, but the Programs directory NSIS made.
    let mut after = f.m.clone();
    let programs = format!("{LOCAL}\\Programs");
    assert!(after.files.remove(&lc(&programs)).is_some());
    assert!(after == before, "{}", diff(&after, &before));
}

#[test]
fn nsis_install_location() {
    let mut f = Fake::new();
    let req = Request {
        id: "GalaTest.PkNsis",
        version: None,
        source_location: SRC,
        location: Some("C:\\Apps\\Pk Nsis"),
        options: Options::default(),
    };
    let out = install(&mut f, &source(), &req).unwrap();
    assert!(matches!(&out, Outcome::Installed { location, .. } if location == "C:\\Apps\\Pk Nsis"));
    // /D= last and unquoted (as NSIS wants it).
    assert!(f.runs[0].ends_with("\" /S /D=C:\\Apps\\Pk Nsis"), "{}", f.runs[0]);
    assert!(f.file("C:\\Apps\\Pk Nsis\\pknsis.exe").is_some());
    // A zip package has no location to choose.
    let req = Request { id: "GalaTest.PkHello", options: Options::default(), ..req };
    assert_eq!(install(&mut f, &source(), &req).unwrap_err().kind, Kind::Usage);
}

#[test]
fn inno_install_and_uninstall() {
    let mut f = Fake::new();
    f.inno_registers = true;
    install_id(&mut f, "GalaTest.InnoSetup", None).unwrap();
    // winget's Inno switches, and the setup log next to gala's.
    let staged = format!(
        "{LOCAL}\\Temp\\gala\\GalaTest.InnoSetup_Microsoft.Winget.Source_8wekyb3d8bbwe\\innosetup-2.0.exe"
    );
    assert_eq!(
        f.runs[0],
        format!(
            "\"{staged}\" /SP- /SILENT /SUPPRESSMSGBOXES /NORESTART /LOG=\"{LOCAL}\\gala\\logs\\GalaTest.InnoSetup-2.0.setup.log\""
        )
    );
    let k = format!("{UNINSTALL_KEY}\\{{8E2A0D4C-GALA}}_is1");
    assert_eq!(
        f.value(Root::CurrentUser, &k, "WinGetPackageIdentifier"),
        Some(RegValue::Sz("GalaTest.InnoSetup".into()))
    );
    assert_eq!(installed(&mut f)[0].version, "2.0");
    // QuietUninstallString as it is.
    uninstall(&mut f, "GalaTest.InnoSetup").unwrap();
    assert_eq!(f.runs[1], format!("\"{LOCAL}\\Programs\\InnoSetup\\unins000.exe\" /SILENT"));
    assert!(!f.reg_key_exists(Root::CurrentUser, &k));
    assert!(f.exists(&format!("{LOCAL}\\Programs\\InnoSetup")).is_none());
}

#[test]
fn entry_found_by_apps_and_features_product_code() {
    // A manifest whose ProductCode is only in its AppsAndFeaturesEntries
    // (winget-pkgs' JRSoftware.InnoSetup): the entry is found by it, as
    // winget correlates, even when the install made other entries too.
    let p = crate::winget::Package::from_texts(&[&format!(
        "PackageIdentifier: A.B\nPackageVersion: 1.0\nPackageLocale: en-US\nPublisher: P\nPackageName: N\nLicense: MIT\nShortDescription: d\nManifestType: singleton\nManifestVersion: 1.10.0\nInstallerType: inno\nAppsAndFeaturesEntries:\n- ProductCode: App 6_is1\nInstallers:\n- Architecture: x86\n  InstallerUrl: https://h/a.exe\n  InstallerSha256: {}\n",
        "0".repeat(64)
    )])
    .unwrap();
    let mut f = Fake::new();
    for k in ["Helper_is1", "App 6_is1"] {
        f.reg_set(
            Root::CurrentUser,
            &format!("{UNINSTALL_KEY}\\{k}"),
            "DisplayName",
            &RegValue::Sz(k.into()),
        )
        .unwrap();
    }
    let found = exe::find_entry(&mut f, &[], &p, &p.installers[0], None);
    assert_eq!(found, Some((Root::CurrentUser, "App 6_is1".to_string(), "ProductCode")));
}

#[test]
fn an_installer_that_registers_nothing() {
    let mut f = Fake::new();
    let before = f.m.clone();
    let e = install_id(&mut f, "GalaTest.InnoSetup", None).unwrap_err();
    assert_eq!(e.kind, Kind::Failed);
    assert!(e.message.contains("registered no uninstall entry"), "{}", e.message);
    assert!(f.log.iter().any(|l| l.starts_with("error no-arp-entry")));
    assert!(f.m == before, "{}", diff(&f.m, &before));
}

#[test]
fn program_install_failures() {
    // Each of gala's own steps failing in turn: before the installer
    // runs, nothing is changed; after it, the installer's changes stay
    // (no vendor rollback) but gala's tags and staged file do not.
    let mut n = 0;
    loop {
        let mut f = Fake::new();
        let before = f.m.clone();
        f.fail_at = Some(n);
        match install_id(&mut f, "GalaTest.PkNsis", Some("1.0.0")) {
            Ok(_) => break,
            Err(e) => {
                assert_eq!(e.kind, Kind::Failed, "step {n}: {e}");
                assert!(f.exists(&format!("{LOCAL}\\Temp")).is_none(), "step {n}: {:#?}", f.log);
                if f.runs.is_empty() {
                    assert!(f.m == before, "step {n}: {e}\n{}", diff(&f.m, &before));
                } else {
                    assert!(e.message.contains("stays installed"), "step {n}: {e}");
                    assert!(f.reg_key_exists(Root::CurrentUser, &nsis_arp()));
                    let v = f.value(Root::CurrentUser, &nsis_arp(), "WinGetPackageIdentifier");
                    assert_eq!(v, None, "step {n}");
                }
            }
        }
        n += 1;
    }
    assert!(n > 4, "{n}");
}

#[test]
fn switches() {
    use crate::winget::select::{CommandOptions, Mode};
    let silent = CommandOptions { mode: Mode::Silent, ..CommandOptions::default() };
    let nsis = installer_of(
        "- Architecture: x64\n  InstallerType: nullsoft\n  InstallerUrl: https://h/setup.exe\n",
    );
    // winget's default mode is silent with progress (its documentation:
    // "--silent ... The default experience shows installer progress");
    // NSIS has one silent switch for both.
    let opts = CommandOptions { log: Some("L".into()), ..CommandOptions::default() };
    assert_eq!(exe::install_command(&nsis, "C:\\t\\s.exe", &opts).unwrap(), "\"C:\\t\\s.exe\" /S");
    // The manifest's switches replace the type's one by one: Silent is
    // the --silent mode's; Custom is added; /D= last.
    let custom = installer_of(
        "- Architecture: x64\n  InstallerType: nullsoft\n  InstallerUrl: https://h/setup.exe\n  InstallerSwitches:\n    Silent: /S /quiet\n    Custom: /X=1\n",
    );
    let at = |mode: Mode, dir: &str| CommandOptions {
        mode,
        location: Some(dir.into()),
        ..CommandOptions::default()
    };
    assert_eq!(
        exe::install_command(&custom, "s.exe", &at(Mode::SilentWithProgress, "D:\\p")).unwrap(),
        "\"s.exe\" /S /X=1 /D=D:\\p"
    );
    assert_eq!(
        exe::install_command(&custom, "s.exe", &at(Mode::Silent, "D:\\p")).unwrap(),
        "\"s.exe\" /S /quiet /X=1 /D=D:\\p"
    );
    // Inno Setup: /SILENT shows progress, /VERYSILENT nothing.
    let inno = installer_of(
        "- Architecture: x64\n  InstallerType: inno\n  InstallerUrl: https://h/setup.exe\n",
    );
    assert_eq!(
        exe::install_command(&inno, "s.exe", &at(Mode::SilentWithProgress, "D:\\p q")).unwrap(),
        "\"s.exe\" /SP- /SILENT /SUPPRESSMSGBOXES /NORESTART /DIR=\"D:\\p q\""
    );
    assert_eq!(
        exe::install_command(&inno, "s.exe", &at(Mode::Silent, "D:\\p q")).unwrap(),
        "\"s.exe\" /SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /DIR=\"D:\\p q\""
    );
    // A plain exe needs the manifest's Silent or SilentWithProgress,
    // unless run interactively.
    let exe_plain = installer_of(
        "- Architecture: x64\n  InstallerType: exe\n  InstallerUrl: https://h/setup.exe\n",
    );
    assert_eq!(
        exe::install_command(&exe_plain, "s.exe", &silent).unwrap_err().kind,
        Kind::Unsupported
    );
    let interactive = CommandOptions { mode: Mode::Interactive, ..CommandOptions::default() };
    assert_eq!(exe::install_command(&exe_plain, "s.exe", &interactive).unwrap(), "\"s.exe\"");
    let exe_silent = installer_of(
        "- Architecture: x64\n  InstallerType: exe\n  InstallerUrl: https://h/setup.exe\n  InstallerSwitches:\n    Silent: -q\n",
    );
    assert_eq!(
        exe::install_command(&exe_silent, "s.exe", &CommandOptions::default()).unwrap(),
        "\"s.exe\" -q"
    );
    assert_eq!(
        exe::install_command(&exe_silent, "s.exe", &at(Mode::Silent, "D:\\")).unwrap_err().kind,
        Kind::Unsupported
    );
    // --override replaces everything.
    let o = CommandOptions { override_args: Some("/x /y".into()), ..CommandOptions::default() };
    assert_eq!(exe::install_command(&exe_plain, "s.exe", &o).unwrap(), "\"s.exe\" /x /y");
    // Uninstall commands.
    assert_eq!(
        exe::uninstall_command("inno", None, Some("\"C:\\a\\unins000.exe\"")).unwrap(),
        (
            "\"C:\\a\\unins000.exe\" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART".into(),
            "UninstallString"
        )
    );
    assert_eq!(
        exe::uninstall_command("nullsoft", Some("u.exe /S"), Some("u.exe")).unwrap(),
        ("u.exe /S".into(), "QuietUninstallString")
    );
    assert_eq!(
        exe::uninstall_command("exe", None, Some("u.exe")).unwrap_err().kind,
        Kind::Unsupported
    );
}

// ---- The winget resolver on the install path ---------------------------
//
// Expected results are winget's documented behaviour (learn.microsoft.com
// "install command (winget)", "upgrade command", "list command", the
// manifest schema's Dependencies and Scope), as galapkg::winget
// implements it; see ARCHITECTURE "The winget resolver".

fn log_has(f: &Fake, what: &str) -> bool {
    f.log.iter().any(|l| l.contains(what))
}

fn log_pos(f: &Fake, what: &str) -> usize {
    f.log.iter().position(|l| l.contains(what)).unwrap_or_else(|| panic!("{what}: {:#?}", f.log))
}

fn arp_of(id: &str) -> String {
    format!("{UNINSTALL_KEY}\\{}", install_key(id))
}

#[test]
fn the_selection_is_logged() {
    let mut f = Fake::new();
    install_id(&mut f, "GalaTest.PkHello", None).unwrap();
    // 1.1.0 lists an x86 installer first: on x64 winget takes x64.
    assert!(
        log_has(
            &f,
            "selected installer=2 type=zip nested=portable arch=x64 scope=- locale=- url=https://packages.gala.invalid/pkhello-1.1.0.zip"
        ),
        "{:#?}",
        f.log
    );
    assert!(log_has(&f, "runner-up installer=1 arch=x86"), "{:#?}", f.log);
    assert!(log_has(
        &f,
        "manifest id=GalaTest.PkHello version=1.1.0 manifest_version=1.12.0 form=multi-file"
    ));
}

#[test]
fn dependencies_are_installed_first() {
    let mut f = Fake::new();
    let out = install_id(&mut f, "GalaTest.PkDep", None).unwrap();
    assert_eq!(
        out,
        Outcome::Installed {
            id: "GalaTest.PkDep".into(),
            version: "1.0.0".into(),
            commands: vec!["pkdep".into()],
            location: format!(
                "{LOCAL}\\Microsoft\\WinGet\\Packages\\{}",
                install_key("GalaTest.PkDep")
            ),
            dependencies: vec!["GalaTest.PkLib 1.0.0".into()],
        }
    );
    assert!(log_has(&f, "plan install GalaTest.PkLib 1.0.0 installer 1 (for GalaTest.PkDep)"));
    assert!(log_has(&f, "plan install GalaTest.PkDep 1.0.0 installer 1"));
    assert!(
        log_pos(&f, "installed id=GalaTest.PkLib") < log_pos(&f, "installed id=GalaTest.PkDep")
    );
    for id in ["GalaTest.PkLib", "GalaTest.PkDep"] {
        assert_eq!(
            f.value(Root::CurrentUser, &arp_of(id), "WinGetPackageIdentifier"),
            Some(RegValue::Sz(id.into()))
        );
    }
    assert!(f.file(&format!("{}\\pklib.shim", links())).is_some());
    // Again: already installed; nothing planned.
    assert!(matches!(
        install_id(&mut f, "GalaTest.PkDep", None).unwrap(),
        Outcome::AlreadyInstalled { .. }
    ));
    // Uninstalling the dependant leaves the dependency (winget does not
    // remove dependencies).
    uninstall(&mut f, "GalaTest.PkDep").unwrap();
    assert!(f.reg_key_exists(Root::CurrentUser, &arp_of("GalaTest.PkLib")));
    // Installed again: the dependency is there at its MinimumVersion, so
    // it is satisfied and not installed again.
    f.log.clear();
    let out = install_id(&mut f, "GalaTest.PkDep", None).unwrap();
    assert!(matches!(&out, Outcome::Installed { dependencies, .. } if dependencies.is_empty()));
    assert!(
        log_has(&f, "plan installed GalaTest.PkLib 1.0.0 (for GalaTest.PkDep)"),
        "{:#?}",
        f.log
    );
    assert!(!log_has(&f, "installed id=GalaTest.PkLib"));
    // --skip-dependencies.
    let mut f = Fake::new();
    let skip = Options { skip_dependencies: true, ..Options::default() };
    install_with(&mut f, "GalaTest.PkDep", None, skip).unwrap();
    assert!(!f.reg_key_exists(Root::CurrentUser, &arp_of("GalaTest.PkLib")));
    assert!(log_has(&f, "dependencies skipped (--skip-dependencies)"));
}

#[test]
fn a_missing_dependency_changes_nothing() {
    let mut f = Fake::new();
    let before = f.m.clone();
    let e = install_id(&mut f, "GalaTest.PkDepMissing", None).unwrap_err();
    assert_eq!(e.kind, Kind::NotFound);
    assert!(
        e.message.contains(
            "GalaTest.PkDepMissing needs GalaTest.NoSuchLib, which the source does not have"
        ),
        "{}",
        e.message
    );
    assert!(f.m == before, "{}", diff(&f.m, &before));
}

#[test]
fn a_failed_dependant_keeps_its_installed_dependencies() {
    // winget installs each dependency as an install of its own: when the
    // package itself then fails, the dependencies stay installed; the
    // package's own transaction is rolled back.
    let mut f = Fake::new();
    // PkDep's command taken by something else.
    install_id(&mut f, "GalaTest.PkHello", None).unwrap();
    f.m.files.insert(
        lc(&format!("{}\\pkdep.exe", links())),
        (format!("{}\\pkdep.exe", links()), Some(b"MZ other".to_vec())),
    );
    let e = install_id(&mut f, "GalaTest.PkDep", None).unwrap_err();
    assert_eq!(e.kind, Kind::Failed);
    assert!(e.message.contains("pkdep exists already") && e.message.contains("rolled back"));
    assert!(f.reg_key_exists(Root::CurrentUser, &arp_of("GalaTest.PkLib")));
    assert!(!f.reg_key_exists(Root::CurrentUser, &arp_of("GalaTest.PkDep")));
}

#[test]
fn scope_is_required_and_kept_on_upgrade() {
    use crate::winget::Scope;
    let user = Options { scope: Some(Scope::User), ..Options::default() };
    let machine = Options { scope: Some(Scope::Machine), ..Options::default() };
    // --scope user: the user installer, though the machine one is first.
    let mut f = Fake::new();
    install_with(&mut f, "GalaTest.PkScoped", Some("1.0.0"), user.clone()).unwrap();
    assert!(log_has(
        &f,
        "scope=user locale=- url=https://packages.gala.invalid/pkscoped-1.0.0-user.zip"
    ));
    // --scope is a requirement: the machine installer is rejected.
    assert!(log_has(&f, "rejected installer=1 scope machine (asked for user)"), "{:#?}", f.log);
    // The upgrade keeps the installed (user) scope: winget would
    // otherwise take the machine installer, listed first.
    f.log.clear();
    let out = upgrade_id(&mut f, "GalaTest.PkScoped").unwrap();
    assert!(matches!(out, Outcome::Upgraded { .. }), "{out:?}");
    assert!(log_has(&f, "installed scope=user type=portable"), "{:#?}", f.log);
    assert!(
        log_has(&f, "url=https://packages.gala.invalid/pkscoped-1.1.0-user.zip"),
        "{:#?}",
        f.log
    );
    // --scope machine without administrator rights: refused before any
    // change (winget's APPINSTALLER_CLI_ERROR_COMMAND_REQUIRES_ADMIN).
    let mut f = Fake::new();
    let before = f.m.clone();
    let e = install_with(&mut f, "GalaTest.PkScoped", None, machine).unwrap_err();
    assert_eq!(e.kind, Kind::RequiresAdmin);
    assert_eq!(e.kind.exit_code(), 6);
    assert!(e.message.contains("administrator"), "{}", e.message);
    assert!(log_has(&f, "error requires-admin administrator=no"), "{:#?}", f.log);
    assert!(f.missing.is_empty(), "{:?}", f.missing);
    assert!(f.m == before);
    // No scope asked: winget's order (no preference: the manifest's), the
    // machine installer, refused the same way.
    let e = install_id(&mut f, "GalaTest.PkScoped", None).unwrap_err();
    assert_eq!(e.kind, Kind::RequiresAdmin);
    assert!(f.m == before);
    // A scope no installer has (PkHello's has none: the asked scope).
    let only_machine = Options { scope: Some(Scope::Machine), ..Options::default() };
    let e = install_with(&mut f, "GalaTest.PkHello", None, only_machine).unwrap_err();
    assert_eq!(e.kind, Kind::RequiresAdmin);
    let _ = user;
}

const PF_PACKAGES: &str = "C:\\Program Files\\WinGet\\Packages";
const PF_LINKS: &str = "C:\\Program Files\\WinGet\\Links";

fn machine_path(f: &Fake) -> String {
    match f.value(Root::LocalMachine, MACHINE_ENVIRONMENT_KEY, "Path") {
        Some(RegValue::ExpandSz(p)) => p,
        other => panic!("the machine's Path: {other:?}"),
    }
}

#[test]
fn machine_scope_portable_install_list_upgrade_uninstall() {
    use crate::winget::Scope;
    let machine = Options { scope: Some(Scope::Machine), ..Options::default() };
    let mut f = Fake::new();
    f.admin = true;
    let before = f.m.clone();
    let out = install_with(&mut f, "GalaTest.PkScoped", Some("1.0.0"), machine.clone()).unwrap();
    assert!(
        matches!(&out, Outcome::Installed { location, .. }
        if location == &format!("{PF_PACKAGES}\\GalaTest.PkScoped_Microsoft.Winget.Source_8wekyb3d8bbwe")),
        "{out:?}"
    );
    assert!(log_has(&f, "elevation administrator=yes"), "{:#?}", f.log);
    assert!(log_has(&f, "layout scope=machine packages=C:\\Program Files\\WinGet\\Packages"));
    assert!(
        log_has(&f, "url=https://packages.gala.invalid/pkscoped-1.0.0-machine.zip"),
        "{:#?}",
        f.log
    );
    // Program Files, the machine's Links on the machine's PATH, the
    // entry in HKLM; nothing per user.
    assert!(f.file(&format!("{PF_LINKS}\\pkscoped.exe")).is_some());
    assert!(f.file(&format!("{PF_LINKS}\\pkscoped.shim")).is_some());
    assert_eq!(machine_path(&f), format!("%SystemRoot%\\system32;%SystemRoot%;{PF_LINKS}"));
    assert!(f.value(Root::CurrentUser, ENVIRONMENT_KEY, "Path").is_none());
    let key = arp_of("GalaTest.PkScoped");
    assert!(!f.reg_key_exists(Root::CurrentUser, &key));
    assert_eq!(
        f.value(Root::LocalMachine, &key, "WinGetPackageIdentifier"),
        Some(RegValue::Sz("GalaTest.PkScoped".into()))
    );
    assert!(log_has(&f, "registered key=HKLM\\"), "{:#?}", f.log);
    // Listed as machine-wide; again is "already installed".
    let l = list(&mut f, None);
    let row = l.iter().find(|r| r.id.as_deref() == Some("GalaTest.PkScoped")).unwrap();
    assert_eq!(row.installed.root, Root::LocalMachine);
    assert!(matches!(
        install_with(&mut f, "GalaTest.PkScoped", Some("1.0.0"), machine.clone()).unwrap(),
        Outcome::AlreadyInstalled { .. }
    ));
    // Upgrade keeps machine scope (1.1.0's machine installer).
    f.log.clear();
    let out = upgrade_id(&mut f, "GalaTest.PkScoped").unwrap();
    assert!(matches!(out, Outcome::Upgraded { .. }), "{out:?}");
    assert!(log_has(&f, "installed scope=machine type=portable"), "{:#?}", f.log);
    assert!(
        log_has(&f, "url=https://packages.gala.invalid/pkscoped-1.1.0-machine.zip"),
        "{:#?}",
        f.log
    );
    assert_eq!(
        f.value(Root::LocalMachine, &key, "DisplayVersion"),
        Some(RegValue::Sz("1.1.0".into()))
    );
    // Without administrator rights neither uninstall nor upgrade touches it.
    f.admin = false;
    let installed = f.m.clone();
    let e = uninstall(&mut f, "GalaTest.PkScoped").unwrap_err();
    assert_eq!(e.kind, Kind::RequiresAdmin);
    assert!(f.m == installed);
    // Uninstall: files, links, the PATH entry and the entry gone; the
    // machine as before.
    f.admin = true;
    uninstall(&mut f, "GalaTest.PkScoped").unwrap();
    assert!(f.file(&format!("{PF_LINKS}\\pkscoped.exe")).is_none());
    assert!(!f.reg_key_exists(Root::LocalMachine, &key));
    // The Links directory stays on PATH and on disk, as for user installs.
    let mut after = f.m.clone();
    for k in [PF_LINKS, "C:\\Program Files\\WinGet", PF_PACKAGES] {
        after.files.remove(&lc(k));
    }
    after.keys.insert(
        (1, lc(MACHINE_ENVIRONMENT_KEY)),
        before.keys[&(1, lc(MACHINE_ENVIRONMENT_KEY))].clone(),
    );
    assert!(after == before, "{}", diff(&after, &before));
}

#[test]
fn machine_scope_failures_roll_back() {
    use crate::winget::Scope;
    let machine = || Options { scope: Some(Scope::Machine), ..Options::default() };
    let admin = |f: &mut Fake| f.admin = true;
    let n =
        fail_each_step(admin, |f| install_with(f, "GalaTest.PkScoped", Some("1.0.0"), machine()));
    assert!(n > 10, "{n}");
    let setup = |f: &mut Fake| {
        f.admin = true;
        install_with(f, "GalaTest.PkScoped", Some("1.0.0"), machine()).unwrap();
    };
    fail_each_step(setup, |f| upgrade_id(f, "GalaTest.PkScoped"));
    fail_each_step(setup, |f| uninstall(f, "GalaTest.PkScoped"));
}

#[test]
fn no_applicable_installer_says_why() {
    use crate::winget::Architecture;
    let mut f = Fake::new();
    let arm = Options { architecture: Some(Architecture::Arm64), ..Options::default() };
    let e = install_with(&mut f, "GalaTest.PkHello", None, arm).unwrap_err();
    assert_eq!(e.kind, Kind::NotFound);
    assert!(e.message.contains("no applicable installer"), "{}", e.message);
    assert!(e.message.contains("architecture x64 was not asked for"), "{}", e.message);
    assert!(log_has(&f, "rejected installer=2 architecture x64 was not asked for"), "{:#?}", f.log);
}

#[test]
fn a_type_gala_cannot_install_falls_back_with_a_ledger_line() {
    // MSIX and MSI are later stages: where a manifest also has an
    // installer gala can use, that one is used (and the gap recorded).
    let mut f = Fake::new();
    let e = install_id(&mut f, "GalaTest.Msix", None).unwrap_err();
    assert_eq!(e.kind, Kind::Unsupported);
    assert!(e.message.contains("msix"), "{}", e.message);
    assert_eq!(f.missing, vec!["gala.exe!install(InstallerType msix)".to_string()]);
}

#[test]
fn list_and_upgrade_match_programs_installed_without_gala() {
    // PkNsis 1.0.0 installed by running its installer directly (not
    // through gala): its uninstall entry has no WinGet tags. winget list
    // and upgrade match it to the source's package by its name and
    // publisher (ARCHITECTURE "The winget resolver", arp).
    let mut f = Fake::new();
    f.run("\"C:\\pk_source\\installers\\pknsis-1.0.0.exe\" /S", 1000).unwrap();
    let s = source();
    let listed = list(&mut f, None);
    let nsis = listed.iter().find(|l| l.installed.key == NSIS_KEY).unwrap();
    assert_eq!(nsis.id, None);
    let listed = list(&mut f, Some(&s));
    let nsis = listed.iter().find(|l| l.installed.key == NSIS_KEY).unwrap();
    assert_eq!(nsis.id.as_deref(), Some("GalaTest.PkNsis"));
    assert_eq!(nsis.installed.version, "1.0.0");
    assert_eq!(nsis.available.as_deref(), Some("1.1.0"));
    // The MSI matches nothing.
    let msi = listed.iter().find(|l| l.installed.name == "Some MSI").unwrap();
    assert_eq!((msi.id.as_deref(), msi.available.as_deref()), (None, None));
    let ups = upgrades(&mut f, Some(&s), None).unwrap();
    assert_eq!(ups.len(), 1);
    assert_eq!(ups[0].available, "1.1.0");
    assert_eq!(ups[0].installed.id.as_deref(), Some("GalaTest.PkNsis"));
    // Upgraded: 1.1.0's installer over it, the entry then tagged.
    let out = upgrade(&mut f, Some(&s), "GalaTest.PkNsis", None, &Options::default()).unwrap();
    assert_eq!(
        out,
        Outcome::Upgraded {
            id: "GalaTest.PkNsis".into(),
            from: "1.0.0".into(),
            to: "1.1.0".into()
        }
    );
    assert!(log_has(&f, "matched key=GalaTest.PkNsis by=NameAndPublisher"), "{:#?}", f.log);
    assert_eq!(
        f.value(Root::CurrentUser, &nsis_arp(), "WinGetPackageIdentifier"),
        Some(RegValue::Sz("GalaTest.PkNsis".into()))
    );
    // Without a source, an untagged entry cannot be upgraded by name.
    let mut f = Fake::new();
    f.run("\"C:\\pk_source\\installers\\pknsis-1.0.0.exe\" /S", 1000).unwrap();
    assert!(upgrade_id(&mut f, "GalaTest.PkNsis").is_err());
}

// ---- Sources over HTTP(S) ---------------------------------------------

#[test]
fn source_locations() {
    use crate::source::Location;
    let gh = |owner: &str, repo: &str, branch: &str| Location::GitHub {
        owner: owner.into(),
        repo: repo.into(),
        branch: branch.into(),
    };
    for (given, want) in [
        ("https://github.com/microsoft/winget-pkgs", gh("microsoft", "winget-pkgs", "master")),
        ("https://GitHub.com/microsoft/winget-pkgs/", gh("microsoft", "winget-pkgs", "master")),
        ("https://github.com/me/pkgs.git", gh("me", "pkgs", "master")),
        ("https://github.com/me/pkgs/tree/main", gh("me", "pkgs", "main")),
        (
            "https://localhost:8443/winget-pkgs/",
            Location::Http("https://localhost:8443/winget-pkgs".into()),
        ),
        ("http://mirror/x", Location::Http("http://mirror/x".into())),
        ("C:\\pk_source\\", Location::Dir("C:\\pk_source".into())),
        ("C:/pk_source", Location::Dir("C:\\pk_source".into())),
    ] {
        assert_eq!(Source::open(given).unwrap().location, want, "{given}");
    }
    for bad in ["https://github.com/microsoft", "https://github.com/a/b/blob/x", "https://h/x?y=1"]
    {
        assert_eq!(Source::open(bad).unwrap_err().kind, Kind::Usage, "{bad}");
    }
    assert_eq!(
        Source::open(crate::source::DEFAULT_SOURCE).unwrap().location,
        gh("microsoft", "winget-pkgs", "master")
    );
}

#[test]
fn a_github_source_reads_through_the_contents_api_and_raw_files() {
    let mut f = Fake::new();
    let s = Source::open("https://github.com/microsoft/winget-pkgs").unwrap();
    assert_eq!(
        s.versions(&mut f, "GalaTest.PkHello").unwrap(),
        vec!["1.1.0".to_string(), "1.0.0".to_string()]
    );
    assert_eq!(
        f.fetches[0],
        "https://api.github.com/repos/microsoft/winget-pkgs/contents/manifests/g/GalaTest/PkHello?ref=master"
    );
    // ripgrep's directory holds the MSVC package, not versions.
    assert_eq!(s.versions(&mut f, "BurntSushi.ripgrep").unwrap_err().kind, Kind::NotFound);
    assert_eq!(s.versions(&mut f, "No.Such").unwrap_err().kind, Kind::NotFound);
    f.fetches.clear();
    let p = s.find(&mut f, "GalaTest.PkHello", Some("1.1")).unwrap();
    assert_eq!(p.version, "1.1.0");
    assert!(f.fetches.iter().any(|u| u
        == "https://raw.githubusercontent.com/microsoft/winget-pkgs/master/manifests/g/GalaTest/PkHello/1.1.0/GalaTest.PkHello.installer.yaml"));
    // Installed from it: the installer is downloaded (no mirror over
    // HTTP), the source's URL kept for upgrade.
    let req = Request {
        id: "GalaTest.PkHttps",
        version: None,
        source_location: &s.root,
        location: None,
        options: Options::default(),
    };
    install(&mut f, &s, &req).unwrap();
    assert!(log_has(
        &f,
        "source list url=https://api.github.com/repos/microsoft/winget-pkgs/contents/manifests/g/GalaTest/PkHttps?ref=master"
    ));
    assert!(log_has(&f, "source read url=https://raw.githubusercontent.com/"));
    assert_eq!(
        f.value(Root::CurrentUser, &arp_of("GalaTest.PkHttps"), "GalaSource"),
        Some(RegValue::Sz("https://github.com/microsoft/winget-pkgs".into()))
    );
    // `gala upgrade` reads that source again: nothing newer.
    assert_eq!(upgrades(&mut f, None, None).unwrap(), vec![]);
    // Searching GitHub needs winget's index.
    assert_eq!(s.search(&mut f, "hello").unwrap_err().kind, Kind::Unsupported);
    // No installers\ mirror over HTTP: PkLib's URL (packages.gala.invalid)
    // is fetched, which this host cannot.
    let req = Request { id: "GalaTest.PkLib", ..req };
    assert_eq!(install(&mut f, &s, &req).unwrap_err().kind, Kind::Unsupported);
}

#[test]
fn a_tree_served_over_https() {
    let mut f = Fake::new();
    let s = Source::open("https://localhost:8443/winget-pkgs").unwrap();
    assert_eq!(s.versions(&mut f, "GalaTest.PkHttps").unwrap(), vec!["1.0.0".to_string()]);
    assert_eq!(f.fetches[0], "https://localhost:8443/winget-pkgs/manifests/g/GalaTest/PkHttps/");
    let hits = s.search(&mut f, "pkhttps").unwrap();
    assert_eq!(hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec!["GalaTest.PkHttps"]);
    let req = Request {
        id: "GalaTest.PkHttps",
        version: None,
        source_location: &s.root,
        location: None,
        options: Options::default(),
    };
    install(&mut f, &s, &req).unwrap();
    // A listing that is not JSON fails with what was read.
    let bad = Source::open("https://localhost:8443/html-index").unwrap();
    let e = bad.versions(&mut f, "GalaTest.PkHttps").unwrap_err();
    assert_eq!(e.kind, Kind::Failed);
    assert!(
        e.message.contains("https://localhost:8443/html-index/manifests/g/GalaTest/PkHttps/"),
        "{}",
        e.message
    );
    assert!(log_has(&f, "error source list JSON at byte 0: not a JSON value"), "{:#?}", f.log);
}

#[test]
fn inno_setup_6_x86_beside_the_7_subpackage() {
    // winget-pkgs keeps JRSoftware.InnoSetup's versions beside the
    // JRSoftware.InnoSetup.7 package's directory (InnoSetup/7): 6.5.4 is
    // found, its x86 installers chosen on x64 (WOW64), no fallback.
    use crate::winget::Architecture;
    use crate::winget::Scope;
    use crate::winget::select::{CommandOptions, Mode};
    let mut f = Fake::new();
    let p = source().find(&mut f, "JRSoftware.InnoSetup", None).unwrap();
    assert_eq!(p.version, "6.5.4");
    let o = Options { scope: Some(Scope::User), ..Options::default() };
    let user = select_installer(&mut f, &p, &o.request(None)).unwrap();
    let i = &p.installers[user];
    assert_eq!(i.architecture, Architecture::X86);
    // Its ProductCode comes from the root AppsAndFeaturesEntries (ARP).
    assert_eq!(i.apps_and_features[0].product_code.as_deref(), Some("Inno Setup 6_is1"));
    assert!(gap(i).is_none(), "an x86 inno installer is gala's to run");
    let opts = CommandOptions { mode: Mode::Silent, log: Some("L".into()), ..Default::default() };
    assert_eq!(
        exe::install_command(i, "C:\\t\\innosetup-6.5.4.exe", &opts).unwrap(),
        "\"C:\\t\\innosetup-6.5.4.exe\" /SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART \
         /LOG=\"L\" /CURRENTUSER"
    );
}

#[test]
fn inno_setup_7_runs_with_winget_switches_per_scope() {
    // winget-pkgs' real manifest of Inno Setup 7.1.1 (pk_install_inno
    // installs it): one x64 installer per scope, told apart by Custom.
    use crate::winget::Scope;
    use crate::winget::select::{CommandOptions, Mode};
    let mut f = Fake::new();
    let p = source().find(&mut f, "JRSoftware.InnoSetup.7", None).unwrap();
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    let pick = |f: &mut Fake, scope| {
        let o = Options { scope: Some(scope), ..Options::default() };
        select_installer(f, &p, &o.request(None)).unwrap()
    };
    let user = pick(&mut f, Scope::User);
    let machine = pick(&mut f, Scope::Machine);
    assert_eq!((user, machine), (1, 0));
    let i = &p.installers[user];
    assert_eq!(i.product_code.as_deref(), Some("Inno Setup 7_is1"));
    assert!(gap(i).is_none(), "an x64 inno installer is gala's to run");
    let opts = CommandOptions { mode: Mode::Silent, log: Some("L".into()), ..Default::default() };
    assert_eq!(
        exe::install_command(i, "C:\\t\\innosetup-7.1.1-x64.exe", &opts).unwrap(),
        "\"C:\\t\\innosetup-7.1.1-x64.exe\" /SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART \
         /LOG=\"L\" /CURRENTUSER"
    );
    assert_eq!(
        exe::install_command(&p.installers[machine], "s.exe", &opts).unwrap(),
        "\"s.exe\" /SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=\"L\" /ALLUSERS"
    );
}

#[test]
fn a_failed_installer_s_own_log_is_in_the_install_log() {
    let mut f = Fake::new();
    f.inno_fails = true;
    let e = install_id(&mut f, "GalaTest.InnoSetup", None).unwrap_err();
    assert_eq!(e.kind, Kind::Failed);
    assert!(e.message.contains("exited with code 1"), "{e}");
    assert!(log_has(&f, "setup-log path="), "{:#?}", f.log);
    assert!(log_has(&f, "setup-log: Setup version: Inno Setup version 7.1.1"), "{:#?}", f.log);
    assert!(log_has(&f, "setup-log: Initialization error: no."), "{:#?}", f.log);
}
