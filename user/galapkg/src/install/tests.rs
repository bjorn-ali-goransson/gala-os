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
        };
        for d in
            ["C:", "C:\\gala", "C:\\Users", "C:\\Users\\gala", "C:\\Users\\gala\\AppData", LOCAL]
        {
            f.m.files.insert(lc(d), (d.into(), None));
        }
        f.m.files.insert(lc(GALA), (GALA.into(), Some(b"MZ gala.exe".to_vec())));
        let mut k = UNINSTALL_KEY;
        loop {
            f.m.keys.insert((0, lc(k)), (k.into(), vec![]));
            if !k.contains('\\') {
                break;
            }
            k = parent_of(k);
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
    fn fetch(&mut self, _url: &str) -> Result<Vec<u8>, HostError> {
        Err(HostError { code: 50 })
    }
    fn env(&mut self, name: &str) -> Option<String> {
        name.eq_ignore_ascii_case("LOCALAPPDATA").then(|| LOCAL.into())
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
        if name == "innosetup-2.0.exe" {
            assert!(args.contains("/VERYSILENT") && args.contains("/SUPPRESSMSGBOXES"), "{args}");
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
    install(f, &source(), &Request { id, version, source_location: SRC, location: None })
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
    assert!(Source::open("https://github.com/microsoft/winget-pkgs").is_err());
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
        ("jqlang.jq", "1.8.2"),
    ] {
        s.manifest(&mut f, id, v).unwrap_or_else(|e| panic!("{id} {v}: {e}"));
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
    let out = upgrade(&mut f, None, "GalaTest.PkHello", None).unwrap();
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
        upgrade(&mut f, None, "GalaTest.PkHello", None).unwrap(),
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
    fail_each_step(setup, |f| upgrade(f, None, "GalaTest.PkHello", None));
    fail_each_step(setup, |f| uninstall(f, "GalaTest.PkHello"));
}

#[test]
fn plans() {
    let portable = Installer {
        architecture: "x64".into(),
        installer_type: "portable".into(),
        nested_installer_type: None,
        nested_files: vec![],
        url: "https://h/jq-windows-amd64.exe".into(),
        sha256: "0".repeat(64),
        scope: None,
        commands: vec![],
        switches: vec![],
        product_code: None,
        archive_binaries_depend_on_path: false,
    };
    let p = plan(&portable, b"MZ").unwrap();
    assert_eq!(p.files[0].0, "jq-windows-amd64.exe");
    assert_eq!(
        p.commands,
        vec![Command { alias: "jq-windows-amd64".into(), target: "jq-windows-amd64.exe".into() }]
    );
    let named = Installer { commands: vec!["jq".into()], ..portable.clone() };
    let p = plan(&named, b"MZ").unwrap();
    assert_eq!(p.files[0].0, "jq.exe");
    assert_eq!(p.commands[0].alias, "jq");
    let gala = Installer { commands: vec!["gala".into()], ..portable.clone() };
    assert!(plan(&gala, b"MZ").is_err());
    // A zip with a path out of the package directory.
    let evil = crate::zip::writer::build(&[("../evil.exe", b"MZ")], true);
    let z = Installer {
        installer_type: "zip".into(),
        nested_installer_type: Some("portable".into()),
        nested_files: vec![crate::manifest::NestedFile {
            relative_path: "evil.exe".into(),
            alias: None,
        }],
        ..portable.clone()
    };
    assert!(plan(&z, &evil).unwrap_err().message.contains("unsafe path"));
    let ok = crate::zip::writer::build(&[("a/b/tool.exe", b"MZ"), ("c.txt", b"c")], true);
    let z = Installer {
        nested_files: vec![crate::manifest::NestedFile {
            relative_path: "a\\b\\TOOL.exe".into(),
            alias: None,
        }],
        archive_binaries_depend_on_path: true,
        ..z
    };
    let p = plan(&z, &ok).unwrap();
    assert_eq!(p.dirs, vec!["a".to_string(), "a\\b".to_string()]);
    assert_eq!(p.commands, vec![Command { alias: "TOOL".into(), target: "a\\b\\tool.exe".into() }]);
    assert!(p.dir_on_path);
    let msi = Installer { installer_type: "msi".into(), ..portable };
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
    let out = upgrade(&mut f, None, "GalaTest.PkNsis", None).unwrap();
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
    };
    let out = install(&mut f, &source(), &req).unwrap();
    assert!(matches!(&out, Outcome::Installed { location, .. } if location == "C:\\Apps\\Pk Nsis"));
    // /D= last and unquoted (as NSIS wants it).
    assert!(f.runs[0].ends_with("\" /S /D=C:\\Apps\\Pk Nsis"), "{}", f.runs[0]);
    assert!(f.file("C:\\Apps\\Pk Nsis\\pknsis.exe").is_some());
    // A zip package has no location to choose.
    let req = Request { id: "GalaTest.PkHello", ..req };
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
            "\"{staged}\" /SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=\"{LOCAL}\\gala\\logs\\GalaTest.InnoSetup-2.0.setup.log\""
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
    let nsis = Installer {
        architecture: "x64".into(),
        installer_type: "nullsoft".into(),
        nested_installer_type: None,
        nested_files: vec![],
        url: "https://h/setup.exe".into(),
        sha256: "0".repeat(64),
        scope: None,
        commands: vec![],
        switches: vec![],
        product_code: None,
        archive_binaries_depend_on_path: false,
    };
    assert_eq!(
        exe::install_command(&nsis, "C:\\t\\s.exe", Some("L"), None).unwrap(),
        "\"C:\\t\\s.exe\" /S"
    );
    // The manifest's switches: Silent replaces, Custom is added.
    let custom = Installer {
        switches: vec![("Silent".into(), "/S /quiet".into()), ("Custom".into(), "/X=1".into())],
        ..nsis.clone()
    };
    assert_eq!(
        exe::install_command(&custom, "s.exe", None, Some("D:\\p")).unwrap(),
        "\"s.exe\" /S /quiet /X=1 /D=D:\\p"
    );
    let inno = Installer { installer_type: "inno".into(), ..nsis.clone() };
    assert_eq!(
        exe::install_command(&inno, "s.exe", None, Some("D:\\p q")).unwrap(),
        "\"s.exe\" /SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /DIR=\"D:\\p q\""
    );
    // A plain exe needs the manifest's Silent switch.
    let exe_plain = Installer { installer_type: "exe".into(), ..nsis.clone() };
    assert_eq!(
        exe::install_command(&exe_plain, "s.exe", None, None).unwrap_err().kind,
        Kind::Unsupported
    );
    let exe_silent = Installer { switches: vec![("Silent".into(), "-q".into())], ..exe_plain };
    assert_eq!(exe::install_command(&exe_silent, "s.exe", None, None).unwrap(), "\"s.exe\" -q");
    assert!(exe::install_command(&exe_silent, "s.exe", None, Some("D:\\")).is_err());
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
