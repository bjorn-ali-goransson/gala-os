//! Whole-machine snapshots (ROADMAP SN step 1; WORKFLOW "Machine
//! snapshots"): a boot run is saved when a line appears in its serial log
//! (`boot-test <scenario> --snapshot-at <text> --snapshot-name <name>`),
//! and later runs start from it (`boot-test --from-snapshot <name>`).
//!
//! The mechanism is QEMU's own, driven over QMP: `stop`, then (with a
//! disk) `blockdev-snapshot-sync`, which freezes the disk as it is at the
//! stop and sends later writes to a new qcow2 layer, then a migration of
//! the stopped machine (memory and devices) into a gzip stream. A restore
//! starts QEMU with the same machine and `-incoming` from that stream, on
//! a qcow2 layer of its own over the frozen disk, and continues it. Why
//! not `savevm`: it keeps memory inside a qcow2 disk image, so a machine
//! without a disk cannot be saved, the raw disk images of the scenarios
//! would have to be converted first, and two restores of one internal
//! snapshot would write the same image. A migration stream is read-only
//! once written, and each restore stacks its own layer.
//!
//! What a snapshot needs to match is exactly what is in the guest's
//! memory: the kernel, the boot modules and the command line (all loaded
//! by Limine at boot; gala's DLLs are boot modules, held in memory, not
//! files on the disk), and the virtual machine (QEMU's version, the
//! accelerator, memory, processors). `check_build` compares them and names
//! each difference; nothing else is compared, since nothing else is in the
//! saved state (the disk is part of the snapshot itself).
//!
//! A snapshot is a directory `target/snapshots/<name>/` (a symbolic link
//! `target/snapshots` to another file system holds them there):
//! `manifest` (this module's text format), `state.gz` (the migration
//! stream), `disk.img` (the frozen disk, raw, for disk scenarios),
//! `serial.log` (the run's log up to the save) and `runs/` (each restored
//! run's disk layer while it runs, kept after a failed run).

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

type Result<T = ()> = std::result::Result<T, String>;

/// The manifest's first line (its format's version).
const MAGIC: &str = "gala machine snapshot 1";
/// The start of the line a saved snapshot leaves in its run's log.
pub const SAVED: &str = "xtask: snapshot: saved ";
/// The kernel's path in a boot image (as Limine's config names it).
pub const KERNEL: &str = "boot/kernel";

/// What `boot-test` was asked to do with snapshots.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// Save the run when `at` appears in its serial log; `stop`: end the
    /// run there instead of letting it go on.
    Save { at: String, name: String, stop: bool },
    /// Start the run from snapshot `name`; `hotswap`: the DLLs to swap in
    /// (`--hot-swap-unsafe`, hotswap.rs: never a real run).
    Restore { name: String, hotswap: Option<Vec<String>> },
}

static REQUEST: OnceLock<Option<Request>> = OnceLock::new();
/// Boot images record their files' digests (`write_boot_digests`) when a
/// snapshot is saved or restored, or a scenario takes one itself.
static DIGESTS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Reads `--snapshot-at <text> --snapshot-name <name> [--snapshot-stop]`
/// or `--from-snapshot <name>` from boot-test's arguments.
pub fn parse_request(args: &[String]) -> Result<Option<Request>> {
    let value = |flag: &str| -> Result<Option<String>> {
        match args.iter().position(|a| a == flag) {
            None => Ok(None),
            Some(i) => match args.get(i + 1) {
                Some(v) if !v.starts_with("--") => Ok(Some(v.clone())),
                _ => Err(format!("{flag} needs a value")),
            },
        }
    };
    let at = value("--snapshot-at")?;
    let name = value("--snapshot-name")?;
    let from = value("--from-snapshot")?;
    let stop = args.iter().any(|a| a == "--snapshot-stop");
    if from.is_none() {
        // Refused without a snapshot to resume.
        crate::hotswap::parse_flag(args)?;
    }
    match (at, name, from) {
        (None, None, None) if !stop => Ok(None),
        (Some(at), Some(name), None) => {
            check_name(&name)?;
            if at.is_empty() || at.contains('\n') {
                return Err("--snapshot-at needs one line of text".into());
            }
            Ok(Some(Request::Save { at, name, stop }))
        }
        (None, None, Some(name)) if !stop => {
            check_name(&name)?;
            Ok(Some(Request::Restore { name, hotswap: crate::hotswap::parse_flag(args)? }))
        }
        (_, _, Some(_)) => {
            Err("--from-snapshot cannot be combined with taking a snapshot (not supported yet: \
                 a snapshot of a restored run would stack on its snapshot's disk)"
                .into())
        }
        _ => Err("a snapshot needs both --snapshot-at <text> and --snapshot-name <name>".into()),
    }
}

/// Records boot-test's request (once per xtask run).
pub fn set_request(request: Option<Request>) {
    if request.is_some() {
        want_digests();
    }
    let _ = REQUEST.set(request);
}

pub fn request() -> Option<&'static Request> {
    REQUEST.get().and_then(Option::as_ref)
}

/// Boot images made from now on record their digests.
pub fn want_digests() {
    DIGESTS.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn digests_wanted() -> bool {
    DIGESTS.load(std::sync::atomic::Ordering::Relaxed)
}

/// A snapshot's name: a directory name, nothing a shell or a path would
/// read differently.
pub fn check_name(name: &str) -> Result {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "snapshot name {name:?}: use 1-64 letters, digits, '-', '_' or '.', not starting with '.'"
        ))
    }
}

/// `target/snapshots`.
pub fn root(target: &Path) -> PathBuf {
    target.join("snapshots")
}

/// What a snapshot holds and needs, its `manifest` file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Manifest {
    pub name: String,
    pub scenario: String,
    /// When it was taken (seconds since 1970, UTC).
    pub taken: u64,
    /// The serial line it was taken at.
    pub pattern: String,
    /// `qemu-system-x86_64 --version`'s first line.
    pub qemu: String,
    /// "kvm" or "tcg".
    pub accel: String,
    pub memory_mib: u64,
    pub cpus: u32,
    /// The frozen disk's file in the snapshot ("disk.img"), if any.
    pub disk: Option<String>,
    /// The migration stream's size, compressed.
    pub state_bytes: u64,
    /// What the frozen disk takes on the host (sparse).
    pub disk_bytes: u64,
    /// How long the machine was stopped for the save.
    pub save_ms: u64,
    /// The boot image's files the guest holds in memory, `(path, sha256)`,
    /// sorted by path: the kernel, the boot modules, Limine's config (the
    /// command line).
    pub boot: Vec<(String, String)>,
    /// Taken from a run that had DLLs hot-swapped into it (their names):
    /// never a base for a normal resume.
    pub hot_swapped: Option<String>,
}

impl Manifest {
    pub fn to_text(&self) -> String {
        let mut t = format!("{MAGIC}\n");
        let mut field = |k: &str, v: &str| t += &format!("{k}: {v}\n");
        field("name", &self.name);
        field("scenario", &self.scenario);
        field("taken", &self.taken.to_string());
        field("pattern", &self.pattern);
        field("qemu", &self.qemu);
        field("accel", &self.accel);
        field("memory_mib", &self.memory_mib.to_string());
        field("cpus", &self.cpus.to_string());
        field("disk", self.disk.as_deref().unwrap_or("none"));
        field("state_bytes", &self.state_bytes.to_string());
        field("disk_bytes", &self.disk_bytes.to_string());
        field("save_ms", &self.save_ms.to_string());
        if let Some(dlls) = &self.hot_swapped {
            field("hot_swapped", dlls);
        }
        for (path, sha) in &self.boot {
            t += &format!("boot: {sha} {path}\n");
        }
        t
    }

    pub fn parse(text: &str) -> Result<Manifest> {
        let mut lines = text.lines();
        if lines.next() != Some(MAGIC) {
            return Err(format!("not a snapshot manifest (its first line is not {MAGIC:?})"));
        }
        let mut m = Manifest::default();
        for line in lines {
            let (key, value) =
                line.split_once(": ").ok_or_else(|| format!("manifest line {line:?}"))?;
            let number = |v: &str| v.parse::<u64>().map_err(|e| format!("manifest {key}: {e}"));
            match key {
                "name" => m.name = value.into(),
                "scenario" => m.scenario = value.into(),
                "taken" => m.taken = number(value)?,
                "pattern" => m.pattern = value.into(),
                "qemu" => m.qemu = value.into(),
                "accel" => m.accel = value.into(),
                "memory_mib" => m.memory_mib = number(value)?,
                "cpus" => m.cpus = number(value)? as u32,
                "disk" => m.disk = (value != "none").then(|| value.into()),
                "state_bytes" => m.state_bytes = number(value)?,
                "disk_bytes" => m.disk_bytes = number(value)?,
                "save_ms" => m.save_ms = number(value)?,
                "hot_swapped" => m.hot_swapped = Some(value.into()),
                "boot" => {
                    let (sha, path) =
                        value.split_once(' ').ok_or_else(|| format!("manifest line {line:?}"))?;
                    m.boot.push((path.into(), sha.into()));
                }
                // A later version's field: read what is known.
                _ => {}
            }
        }
        if m.name.is_empty() || m.scenario.is_empty() || m.boot.is_empty() {
            return Err("manifest lacks its name, scenario or boot image".into());
        }
        Ok(m)
    }

    pub fn read(dir: &Path) -> Result<Manifest> {
        let path = dir.join("manifest");
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Manifest::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn kernel(&self) -> Option<&str> {
        self.boot.iter().find(|(p, _)| p == KERNEL).map(|(_, s)| s.as_str())
    }
}

/// What a run would start from, compared with what a snapshot needs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Machine {
    pub scenario: String,
    pub qemu: String,
    pub memory_mib: u64,
    pub cpus: u32,
    pub boot: Vec<(String, String)>,
}

/// Whether a snapshot can resume on `now`: the same scenario, kernel, boot
/// modules and command line, QEMU and machine. The error names every
/// difference (the accelerator is checked once QEMU runs: `resume`).
pub fn check_build(m: &Manifest, now: &Machine) -> Result {
    check_build_except(m, now, &[]).map(drop)
}

/// The files a hot swap of `dlls` may change in the boot image: the DLLs
/// (64- and 32-bit), Limine's config (their hashes; `hotswap::prepare`
/// checks that nothing else in it changed) and the signed manifest (their
/// digests).
pub fn hotswap_allowed(dlls: &[String]) -> Vec<String> {
    let mut v: Vec<String> = dlls
        .iter()
        .filter(|d| *d != crate::hotswap::KERNEL)
        .flat_map(|d| [format!("boot/{d}"), format!("boot/x86/{d}")])
        .collect();
    v.push("boot/limine/limine.conf".into());
    v.push("boot/gala.manifest".into());
    // The kernel too, for `--hot-swap-unsafe kernel` (SN step 1e).
    if dlls.iter().any(|d| d == crate::hotswap::KERNEL) {
        v.push(KERNEL.into());
    }
    v
}

/// `check_build`, but files in `allowed` may differ (`--hot-swap-unsafe`):
/// the ones that do.
pub fn check_build_except(m: &Manifest, now: &Machine, allowed: &[String]) -> Result<Vec<String>> {
    let mut why = Vec::new();
    let mut differ = Vec::new();
    if m.scenario != now.scenario {
        why.push(format!("it is of scenario {}, not {}", m.scenario, now.scenario));
    }
    let short = |s: &str| s.chars().take(12).collect::<String>();
    let kernel_now = now.boot.iter().find(|(p, _)| p == KERNEL).map(|(_, s)| s.as_str());
    if m.kernel() != kernel_now && allowed.iter().any(|a| a == KERNEL) && kernel_now.is_some() {
        differ.push(KERNEL.to_string());
    } else if m.kernel() != kernel_now {
        why.push(format!(
            "the kernel differs (snapshot {}, this build {})",
            m.kernel().map_or("none".into(), short),
            kernel_now.map_or("none".into(), short)
        ));
    }
    let (mut changed, mut gone, mut added) = (Vec::new(), Vec::new(), Vec::new());
    for (path, sha) in m.boot.iter().filter(|(p, _)| p != KERNEL) {
        match now.boot.iter().find(|(p, _)| p == path) {
            Some((_, s)) if s != sha && allowed.contains(path) => differ.push(path.clone()),
            Some((_, s)) if s != sha => changed.push(path.as_str()),
            Some(_) => {}
            None => gone.push(path.as_str()),
        }
    }
    for (path, _) in now.boot.iter().filter(|(p, _)| p != KERNEL) {
        if !m.boot.iter().any(|(p, _)| p == path) {
            added.push(path.as_str());
        }
    }
    let list = |v: &[&str]| {
        let shown: Vec<&str> = v.iter().take(8).copied().collect();
        let more = v.len().saturating_sub(shown.len());
        let more = if more > 0 { format!(" and {more} more") } else { String::new() };
        format!("{}{more}", shown.join(", "))
    };
    if !changed.is_empty() {
        why.push(format!("boot image files differ: {}", list(&changed)));
    }
    if !gone.is_empty() {
        why.push(format!("boot image files gone: {}", list(&gone)));
    }
    if !added.is_empty() {
        why.push(format!("boot image files new: {}", list(&added)));
    }
    if m.qemu != now.qemu {
        why.push(format!("QEMU differs ({:?}, now {:?})", m.qemu, now.qemu));
    }
    if (m.memory_mib, m.cpus) != (now.memory_mib, now.cpus) {
        why.push(format!(
            "the machine differs ({} MiB and {} CPUs, now {} MiB and {} CPUs)",
            m.memory_mib, m.cpus, now.memory_mib, now.cpus
        ));
    }
    if why.is_empty() {
        Ok(differ)
    } else {
        Err(format!(
            "snapshot {} cannot resume on this build: {} (the guest holds the kernel, \
             its boot modules (gala's DLLs among them) and its command line in memory, so a \
             snapshot resumes only on the build it was taken from; rebuild that one, or take a \
             new snapshot)",
            m.name,
            why.join("; ")
        ))
    }
}

/// The files of a boot image's tree (`iso_root`) the guest holds in
/// memory, with their SHA-256: Limine's config (the command line), the
/// kernel and every module it names (`path:`, `module_path:`).
pub fn boot_digests(iso_root: &Path) -> Result<Vec<(String, String)>> {
    let conf = "boot/limine/limine.conf";
    let text = fs::read_to_string(iso_root.join(conf)).map_err(|e| format!("{conf}: {e}"))?;
    let mut paths = vec![conf.to_string()];
    for line in text.lines() {
        if let Some((_, rest)) = line.split_once("boot():/") {
            // A signed config has `#<BLAKE2B>` after the path.
            let path = rest.split('#').next().unwrap_or(rest).trim();
            if !paths.iter().any(|p| p == path) {
                paths.push(path.to_string());
            }
        }
    }
    paths.sort();
    let files: Vec<PathBuf> = paths.iter().map(|p| iso_root.join(p)).collect();
    let out = std::process::Command::new("sha256sum")
        .arg("--")
        .args(&files)
        .output()
        .map_err(|e| format!("sha256sum: {e}"))?;
    if !out.status.success() {
        return Err(format!("sha256sum: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let sums: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .collect();
    if sums.len() != paths.len() {
        return Err(format!("sha256sum: {} digests for {} files", sums.len(), paths.len()));
    }
    Ok(paths.into_iter().zip(sums).collect())
}

/// Where a boot image keeps a copy of its DLL modules, each with its
/// linker map (`<path>.map`), and Limine's config: `<image>.modules/`. A
/// snapshot takes it along (`modules/`): `--hot-swap-unsafe` compares a
/// new build of a DLL with the one the snapshot's memory holds
/// (hotswap.rs).
pub fn modules_dir(iso: &Path) -> PathBuf {
    iso.with_extension("modules")
}

/// Fills `modules_dir(iso)` from the image tree `iso_root`: Limine's config
/// and each DLL module `(path in the tree, the build output it came from)`
/// with the map lld wrote beside that output (`deps/<stem>.map`, taken
/// only when `deps/<stem>.dll` is the very file: a map of another build
/// would mislead the diff).
pub fn keep_modules(
    iso: &Path,
    iso_root: &Path,
    dlls: &[(String, PathBuf)],
    kernel: &Path,
) -> Result {
    let dir = modules_dir(iso);
    let _ = fs::remove_dir_all(&dir);
    let conf = "boot/limine/limine.conf";
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    fs::copy(iso_root.join(conf), dir.join("limine.conf")).map_err(|e| format!("{conf}: {e}"))?;
    // The kernel (as the image has it: without debug sections) and lld's
    // map of its build (`<kernel>.map` beside the build output, written by
    // the link: kernel/build.rs), for `--hot-swap-unsafe kernel`.
    let kept = dir.join(KERNEL);
    if let Some(parent) = kept.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::copy(iso_root.join(KERNEL), &kept).map_err(|e| format!("{KERNEL}: {e}"))?;
    let map = kernel.with_extension("map");
    if map.is_file() {
        fs::copy(&map, kept.with_extension("map"))
            .map_err(|e| format!("{}: {e}", map.display()))?;
    }
    for (path, source) in dlls {
        let to = dir.join(path);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let bytes = fs::read(iso_root.join(path)).map_err(|e| format!("{path}: {e}"))?;
        fs::write(&to, &bytes).map_err(|e| format!("{}: {e}", to.display()))?;
        let (Some(stem), Some(out)) = (source.file_stem(), source.parent()) else { continue };
        let deps = out.join("deps");
        let stem = stem.to_string_lossy();
        if fs::read(deps.join(format!("{stem}.dll"))).is_ok_and(|b| b == bytes)
            && let Ok(map) = fs::read(deps.join(format!("{stem}.map")))
        {
            fs::write(to.with_file_name(format!("{}.map", file_name(path))), map)
                .map_err(|e| format!("{path}.map: {e}"))?;
        }
    }
    Ok(())
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Copies directory tree `from` to `to`.
fn copy_tree(from: &Path, to: &Path) -> Result {
    fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for e in fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let e = e.map_err(|e| e.to_string())?;
        let target = to.join(e.file_name());
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            copy_tree(&e.path(), &target)?;
        } else {
            fs::copy(e.path(), &target).map_err(|err| format!("{}: {err}", target.display()))?;
        }
    }
    Ok(())
}

/// Where an image's digests are kept: `<image>.boot.sha256`.
pub fn digests_path(iso: &Path) -> PathBuf {
    iso.with_extension("boot.sha256")
}

pub fn write_boot_digests(iso: &Path, digests: &[(String, String)]) -> Result {
    let text: String = digests.iter().map(|(p, s)| format!("{s}  {p}\n")).collect();
    let path = digests_path(iso);
    fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn read_boot_digests(iso: &Path) -> Result<Vec<(String, String)>> {
    let path = digests_path(iso);
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("{}: {e} (the image was made without its digests)", path.display()))?;
    Ok(text
        .lines()
        .filter_map(|l| l.split_once("  "))
        .map(|(s, p)| (p.to_string(), s.to_string()))
        .collect())
}

/// `qemu --version`'s first line.
pub fn qemu_version(qemu: &Path) -> String {
    std::process::Command::new(qemu)
        .arg("--version")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("").to_string())
        .unwrap_or_default()
}

/// A JSON string's contents.
fn json_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            c if (c as u32) < 0x20 => out += &format!("\\u{:04x}", c as u32),
            c => out.push(c),
        }
    }
    out
}

/// A path for `/bin/sh` inside QEMU's `exec:` migration URIs.
fn shell_path(p: &Path) -> Result<String> {
    let s = p.to_str().ok_or_else(|| format!("{}: not UTF-8", p.display()))?;
    if s.contains('\'') {
        return Err(format!("{s}: a path with a quote cannot go in QEMU's exec: URI"));
    }
    Ok(format!("'{s}'"))
}

/// A QMP connection (QEMU's JSON monitor).
struct Qmp {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Qmp {
    /// Connects to `socket`, waiting up to `wait` for QEMU to make it.
    fn connect(socket: &Path, wait: Duration) -> Result<Qmp> {
        let start = Instant::now();
        let stream = loop {
            match UnixStream::connect(socket) {
                Ok(s) => break s,
                Err(_) if start.elapsed() < wait => std::thread::sleep(Duration::from_millis(20)),
                Err(e) => return Err(format!("QMP {}: {e}", socket.display())),
            }
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
        let writer = stream.try_clone().map_err(|e| e.to_string())?;
        let mut q = Qmp { reader: BufReader::new(stream), writer };
        let mut greeting = String::new();
        q.reader.read_line(&mut greeting).map_err(|e| format!("QMP greeting: {e}"))?;
        q.cmd("qmp_capabilities", "{}")?;
        Ok(q)
    }

    /// Runs `command` with `args` (a JSON object); its "return", compact,
    /// or the error QEMU gave.
    fn cmd(&mut self, command: &str, args: &str) -> Result<String> {
        let line = format!("{{\"execute\":\"{command}\",\"arguments\":{args}}}\n");
        self.writer.write_all(line.as_bytes()).map_err(|e| format!("QMP {command}: {e}"))?;
        loop {
            let mut reply = String::new();
            match self.reader.read_line(&mut reply) {
                Ok(0) => return Err(format!("QMP {command}: QEMU closed the connection")),
                Ok(_) => {}
                Err(e) => return Err(format!("QMP {command}: {e}")),
            }
            let compact: String = reply.chars().filter(|c| !c.is_whitespace()).collect();
            // Asynchronous events ({"event": ...}) come in between.
            if compact.starts_with("{\"return\"") {
                return Ok(compact);
            }
            if compact.starts_with("{\"error\"") {
                return Err(format!("QMP {command}: {}", reply.trim()));
            }
        }
    }
}

/// The string value of `key` in compact JSON.
fn json_str<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let at = json.find(&format!("\"{key}\":\""))? + key.len() + 4;
    json[at..].split('"').next()
}

/// "kvm" or "tcg", as QEMU says.
fn accel(q: &mut Qmp) -> Result<String> {
    let r = q.cmd("query-kvm", "{}")?;
    Ok(if r.contains("\"enabled\":true") { "kvm" } else { "tcg" }.into())
}

/// Waits for the migration QEMU is doing to end; its final status.
fn wait_migration(q: &mut Qmp, limit: Duration) -> Result<String> {
    let start = Instant::now();
    loop {
        let r = q.cmd("query-migrate", "{}")?;
        match json_str(&r, "status") {
            Some("completed") => return Ok(r),
            Some("failed") | Some("cancelled") => {
                return Err(format!(
                    "the migration failed: {}",
                    json_str(&r, "error-desc").unwrap_or(&r)
                ));
            }
            _ if start.elapsed() > limit => {
                return Err(format!("the migration did not end within {}s", limit.as_secs()));
            }
            _ => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// What a run that saves needs to know.
pub struct Save<'a> {
    pub target: &'a Path,
    pub name: &'a str,
    pub at: &'a str,
    pub stop: bool,
    pub scenario: &'a str,
    pub machine: &'a Machine,
    /// The image's `modules_dir`, taken along into the snapshot.
    pub modules: Option<PathBuf>,
}

/// The new layer a run that was saved goes on writing to (next to its
/// disk image: the same file system), deleted when the run ends.
pub fn after_layer(disk: &Path, name: &str) -> Result<PathBuf> {
    let real = fs::canonicalize(disk).map_err(|e| format!("{}: {e}", disk.display()))?;
    let file = real.file_name().and_then(|n| n.to_str()).unwrap_or("disk");
    Ok(real.with_file_name(format!("{file}.after-{name}.qcow2")))
}

/// Saves the machine QEMU runs (QMP at `socket`) as snapshot `s.name`:
/// stops it, freezes its disk `disk` (moved into the snapshot; the run
/// writes to `after_layer` from then on), writes its memory and devices,
/// the manifest and the serial log so far (`log`), then lets it go on
/// (or leaves it stopped with `s.stop`). Returns the line for the log.
pub fn save(socket: &Path, s: &Save, disk: Option<&Path>, log: &Path) -> Result<String> {
    let dir = root(s.target).join(s.name);
    let partial = root(s.target).join(format!(".{}.partial", s.name));
    let _ = fs::remove_dir_all(&partial);
    fs::create_dir_all(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
    let result = save_into(socket, s, disk, log, &partial);
    match result {
        Ok(m) => {
            fs::rename(&partial, &dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            Ok(format!(
                "xtask: snapshot: saved {} at {:?} in {:.1}s ({} MiB of memory and devices, \
                 gzip; disk {}) into {}{}\n",
                s.name,
                s.at,
                m.save_ms as f64 / 1000.0,
                m.state_bytes >> 20,
                match &m.disk {
                    Some(_) => format!("{} MiB on the host", m.disk_bytes >> 20),
                    None => "none".into(),
                },
                dir.display(),
                if s.stop { "; the run stops here (--snapshot-stop)" } else { "; the run goes on" }
            ))
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&partial);
            Err(e)
        }
    }
}

fn save_into(
    socket: &Path,
    s: &Save,
    disk: Option<&Path>,
    log: &Path,
    partial: &Path,
) -> Result<Manifest> {
    let mut q = Qmp::connect(socket, Duration::from_secs(10))?;
    let t = Instant::now();
    // `stop` drains the disk's requests: what the guest has written is in
    // the image, and what it has not is in its memory.
    q.cmd("stop", "{}")?;
    let accel = accel(&mut q)?;
    let _ = fs::copy(log, partial.join("serial.log"));
    let mut disk_bytes = 0;
    if let Some(disk) = disk {
        let real = fs::canonicalize(disk).map_err(|e| format!("{}: {e}", disk.display()))?;
        let after = after_layer(disk, s.name)?;
        let _ = fs::remove_file(&after);
        q.cmd(
            "blockdev-snapshot-sync",
            &format!(
                "{{\"device\":\"d0\",\"snapshot-file\":\"{}\",\"format\":\"qcow2\"}}",
                json_escape(&after.to_string_lossy())
            ),
        )?;
        // The frozen image moves into the snapshot (QEMU keeps it open as
        // the new layer's backing file).
        let kept = partial.join("disk.img");
        fs::rename(&real, &kept).map_err(|e| {
            format!(
                "moving the disk image {} into {}: {e} (they must be on one file system: \
                 make target/snapshots a symbolic link to a directory beside the image)",
                real.display(),
                partial.display()
            )
        })?;
        disk_bytes = host_bytes(&kept);
    }
    // Migration's default bandwidth limit is for live migration; a stopped
    // machine goes as fast as gzip takes it.
    q.cmd("migrate-set-parameters", "{\"max-bandwidth\":68719476736}")?;
    let state = partial.join("state.gz");
    q.cmd(
        "migrate",
        &format!(
            "{{\"uri\":\"{}\"}}",
            json_escape(&format!("exec:gzip -1 > {}", shell_path(&state)?))
        ),
    )?;
    wait_migration(&mut q, Duration::from_secs(1800))?;
    let save_ms = t.elapsed().as_millis() as u64;
    let m = Manifest {
        name: s.name.into(),
        scenario: s.scenario.into(),
        taken: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        pattern: s.at.into(),
        qemu: s.machine.qemu.clone(),
        accel,
        memory_mib: s.machine.memory_mib,
        cpus: s.machine.cpus,
        disk: disk.map(|_| "disk.img".into()),
        state_bytes: fs::metadata(&state).map_or(0, |m| m.len()),
        disk_bytes,
        save_ms,
        boot: s.machine.boot.clone(),
        hot_swapped: None,
    };
    // The DLLs and their maps, for a hot swap into this snapshot later.
    if let Some(modules) = s.modules.as_ref().filter(|d| d.is_dir()) {
        copy_tree(modules, &partial.join("modules"))?;
    }
    fs::write(partial.join("manifest"), m.to_text()).map_err(|e| e.to_string())?;
    if !s.stop {
        q.cmd("cont", "{}")?;
    }
    Ok(m)
}

/// What a file takes on the host (sparse files less than their size).
fn host_bytes(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).map_or(0, |m| m.blocks() * 512)
}

/// A run about to start from a snapshot.
pub struct Restore {
    pub dir: PathBuf,
    pub manifest: Manifest,
    /// The run's own disk layer (qcow2 over the snapshot's disk).
    pub layer: Option<PathBuf>,
    /// A hot swap into it (`--hot-swap-unsafe`): never a real run.
    pub hotswap: Option<crate::hotswap::Prepared>,
}

/// Checks that snapshot `name` can resume on `now` and makes the run's
/// own disk layer (`qemu_img`: the qemu-img to use). With `hotswap` (the
/// DLLs, the image `now` was made from), the named DLLs may differ and
/// their plan is made (hotswap.rs).
pub fn prepare_restore(
    target: &Path,
    name: &str,
    now: &Machine,
    qemu_img: &Path,
    hotswap: Option<(&[String], &Path)>,
) -> Result<Restore> {
    let dir = root(target).join(name);
    if !dir.is_dir() {
        return Err(format!("no snapshot {name} ({} lists them)", "cargo xtask snapshots"));
    }
    let manifest = Manifest::read(&dir)?;
    if let (Some(dlls), None) = (&manifest.hot_swapped, &hotswap) {
        return Err(format!(
            "snapshot {name} was taken from a run with {dlls} hot-swapped into it: it is not a \
             base for a normal resume (not a real run)"
        ));
    }
    let hotswap = match hotswap {
        None => {
            check_build(&manifest, now)?;
            None
        }
        Some((dlls, iso)) => {
            let differ = check_build_except(&manifest, now, &hotswap_allowed(dlls))?;
            Some(crate::hotswap::prepare(&dir, iso, dlls, &differ)?)
        }
    };
    let layer = match &manifest.disk {
        None => None,
        Some(file) => {
            let runs = dir.join("runs");
            fs::create_dir_all(&runs).map_err(|e| format!("{}: {e}", runs.display()))?;
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let layer = runs.join(format!("{}-{n}.qcow2", std::process::id()));
            let backing = fs::canonicalize(dir.join(file)).map_err(|e| e.to_string())?;
            let out = std::process::Command::new(qemu_img)
                .args(["create", "-q", "-f", "qcow2", "-F", "raw", "-b"])
                .arg(&backing)
                .arg(&layer)
                .output()
                .map_err(|e| format!("{}: {e}", qemu_img.display()))?;
            if !out.status.success() {
                return Err(format!(
                    "qemu-img create {}: {}",
                    layer.display(),
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
            Some(layer)
        }
    };
    Ok(Restore { dir, manifest, layer, hotswap })
}

/// QEMU's arguments that make a run start from `r`: paused, its memory
/// and devices from the migration stream; a hot swap's plan as a fw_cfg
/// file (`opt/gala/hotswap`: not part of the migrated state).
pub fn incoming_args(r: &Restore) -> Result<Vec<String>> {
    let state = shell_path(&r.dir.join("state.gz"))?;
    let mut args = vec!["-S".into(), "-incoming".into(), format!("exec:gzip -dc {state}")];
    if let Some(h) = &r.hotswap {
        let plan = h.plan.to_str().ok_or("the plan's path is not UTF-8")?;
        if plan.contains(',') {
            return Err(format!("{plan}: a path with a comma cannot go in QEMU's -fw_cfg"));
        }
        args.push("-fw_cfg".into());
        args.push(format!("name={},file={plan}", crate::hotswap::FW_CFG_NAME));
    }
    Ok(args)
}

/// Waits for QEMU (QMP at `socket`) to have loaded the snapshot, checks
/// its accelerator, and starts the machine. Returns the line for the log.
pub fn resume(socket: &Path, r: &Restore) -> Result<String> {
    let t = Instant::now();
    let mut q = Qmp::connect(socket, Duration::from_secs(30))?;
    let accel = accel(&mut q)?;
    if accel != r.manifest.accel {
        return Err(format!(
            "snapshot {} was taken under {} and this QEMU runs {accel}: a machine's processor \
             differs between the two, so it is not resumed",
            r.manifest.name, r.manifest.accel
        ));
    }
    loop {
        let st = q.cmd("query-status", "{}")?;
        match json_str(&st, "status") {
            Some("inmigrate") if t.elapsed() < Duration::from_secs(1800) => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Some("inmigrate") => return Err("loading the snapshot took over 1800s".into()),
            Some("paused") | Some("prelaunch") | Some("postmigrate") | Some("running") => break,
            other => return Err(format!("QEMU's state after loading the snapshot: {other:?}")),
        }
    }
    let migrated = q.cmd("query-migrate", "{}")?;
    if let Some(status) = json_str(&migrated, "status").filter(|s| *s != "completed") {
        return Err(format!("loading the snapshot: migration status {status}"));
    }
    // A hot swap: an NMI to every CPU, taken before the first instruction
    // the resumed machine runs (the kernel's `hotswap::on_nmi`).
    if r.hotswap.is_some() {
        q.cmd("inject-nmi", "{}")?;
    }
    q.cmd("cont", "{}")?;
    let m = &r.manifest;
    let age = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    Ok(format!(
        "xtask: snapshot: resumed {} (scenario {}, taken at {:?}, {} ago) in {:.1}s{}\n",
        m.name,
        m.scenario,
        m.pattern,
        duration_words(age.saturating_sub(m.taken)),
        t.elapsed().as_secs_f64(),
        match &r.layer {
            Some(l) => format!(", on its own disk layer {}", l.display()),
            None => String::new(),
        }
    ) + &match &r.hotswap {
        Some(h) => format!(
            "xtask: hotswap: {} asked to be swapped in at the resume (NOT A REAL RUN; the \
             guest's `hotswap:` lines say what it did; plan {})\n",
            h.dlls.join(","),
            h.plan.display()
        ),
        None => String::new(),
    })
}

fn duration_words(secs: u64) -> String {
    match secs {
        s if s < 120 => format!("{s}s"),
        s if s < 7200 => format!("{}min", s / 60),
        s if s < 172_800 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86400),
    }
}

/// `YYYY-MM-DD hh:mm` UTC of `secs` since 1970 (the civil-from-days
/// algorithm: days to a proleptic Gregorian date).
pub fn utc(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let (h, mi) = ((secs % 86400) / 3600, (secs % 3600) / 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}")
}

/// What a directory takes on the host.
fn dir_bytes(dir: &Path) -> u64 {
    fs::read_dir(dir).map_or(0, |entries| {
        entries
            .filter_map(|e| e.ok())
            .map(|e| match e.file_type() {
                Ok(t) if t.is_dir() => dir_bytes(&e.path()),
                _ => host_bytes(&e.path()),
            })
            .sum()
    })
}

/// `cargo xtask snapshots [rm <name>]`.
pub fn cli(target: &Path, args: &[String]) -> Result {
    let root = root(target);
    match args.first().map(String::as_str) {
        None | Some("list") => {
            let mut names: Vec<String> = fs::read_dir(&root)
                .map(|d| {
                    d.filter_map(|e| e.ok())
                        .filter(|e| e.path().is_dir())
                        .filter_map(|e| e.file_name().into_string().ok())
                        .filter(|n| !n.starts_with('.'))
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            if names.is_empty() {
                println!("no snapshots in {}", root.display());
            }
            for name in names {
                let dir = root.join(&name);
                let size = dir_bytes(&dir) >> 20;
                match Manifest::read(&dir) {
                    Ok(m) => println!(
                        "{name}: scenario {}, taken {} UTC at {:?}, {size} MiB on the host \
                         (state {} MiB, disk {}), kernel {}, {} boot files, {} {} MiB {} CPUs",
                        m.scenario,
                        utc(m.taken),
                        m.pattern,
                        m.state_bytes >> 20,
                        m.disk
                            .as_ref()
                            .map_or("none".into(), |_| format!("{} MiB", m.disk_bytes >> 20)),
                        m.kernel().map_or("?".into(), |k| k.chars().take(12).collect::<String>()),
                        m.boot.len(),
                        m.accel,
                        m.memory_mib,
                        m.cpus
                    ),
                    Err(e) => println!("{name}: {size} MiB, unreadable: {e}"),
                }
            }
            Ok(())
        }
        Some("rm") => {
            let name = args.get(1).ok_or("usage: cargo xtask snapshots rm <name>")?;
            check_name(name)?;
            let dir = root.join(name);
            if !dir.is_dir() {
                return Err(format!("no snapshot {name} in {}", root.display()));
            }
            let size = dir_bytes(&dir) >> 20;
            fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            println!("removed snapshot {name} ({size} MiB)");
            Ok(())
        }
        Some(other) => {
            Err(format!("snapshots: unknown {other:?} (usage: cargo xtask snapshots [rm <name>])"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            name: "office-merge".into(),
            scenario: "office_install_local".into(),
            taken: 1_763_000_000,
            pattern: "process integrator.exe (pid".into(),
            qemu: "QEMU emulator version 11.1.3".into(),
            accel: "tcg".into(),
            memory_mib: 1300,
            cpus: 2,
            disk: Some("disk.img".into()),
            state_bytes: 300 << 20,
            disk_bytes: 8 << 30,
            save_ms: 12_000,
            boot: vec![
                (KERNEL.into(), "a".repeat(64)),
                ("boot/limine/limine.conf".into(), "b".repeat(64)),
                ("boot/msxml6.dll".into(), "c".repeat(64)),
                ("boot/x86/msxml6.dll".into(), "d".repeat(64)),
            ],
            hot_swapped: None,
        }
    }

    fn machine(m: &Manifest) -> Machine {
        Machine {
            scenario: m.scenario.clone(),
            qemu: m.qemu.clone(),
            memory_mib: m.memory_mib,
            cpus: m.cpus,
            boot: m.boot.clone(),
        }
    }

    #[test]
    fn manifest_round_trip() {
        let m = manifest();
        assert_eq!(Manifest::parse(&m.to_text()), Ok(m.clone()));
        let none = Manifest { disk: None, ..m };
        assert_eq!(Manifest::parse(&none.to_text()), Ok(none));
        assert!(Manifest::parse("not a manifest\n").is_err());
        assert!(Manifest::parse(&format!("{MAGIC}\nname: x\n")).is_err());
    }

    #[test]
    fn same_build_resumes() {
        let m = manifest();
        assert_eq!(check_build(&m, &machine(&m)), Ok(()));
    }

    #[test]
    fn another_build_is_refused_with_its_differences() {
        let m = manifest();
        // A changed DLL (a boot module, in the guest's memory).
        let mut now = machine(&m);
        now.boot[2].1 = "e".repeat(64);
        let e = check_build(&m, &now).unwrap_err();
        assert!(e.contains("boot image files differ: boot/msxml6.dll"), "{e}");
        assert!(!e.contains("kernel differs"), "{e}");
        // A changed kernel, named as such.
        let mut now = machine(&m);
        now.boot[0].1 = "f".repeat(64);
        let e = check_build(&m, &now).unwrap_err();
        assert!(e.contains("the kernel differs (snapshot aaaaaaaaaaaa, this build ffffffffffff)"));
        assert!(!e.contains("boot image files"), "{e}");
        // A module gone and one added; another scenario; another machine.
        let mut now = machine(&m);
        now.boot.remove(3);
        now.boot.push(("boot/new.dll".into(), "0".repeat(64)));
        now.scenario = "hello".into();
        now.memory_mib = 512;
        let e = check_build(&m, &now).unwrap_err();
        for part in [
            "boot image files gone: boot/x86/msxml6.dll",
            "boot image files new: boot/new.dll",
            "it is of scenario office_install_local, not hello",
            "the machine differs (1300 MiB and 2 CPUs, now 512 MiB and 2 CPUs)",
        ] {
            assert!(e.contains(part), "{part:?} not in {e}");
        }
        let mut now = machine(&m);
        now.qemu = "QEMU emulator version 8.2.2".into();
        assert!(check_build(&m, &now).unwrap_err().contains("QEMU differs"));
    }

    #[test]
    fn a_hot_swap_allows_only_its_dlls_to_differ() {
        let m = manifest();
        let mut now = machine(&m);
        now.boot[1].1 = "1".repeat(64); // limine.conf (the DLL's hash)
        now.boot[2].1 = "2".repeat(64); // msxml6.dll
        let allowed = hotswap_allowed(&["msxml6.dll".to_string()]);
        assert_eq!(
            check_build_except(&m, &now, &allowed),
            Ok(vec!["boot/limine/limine.conf".to_string(), "boot/msxml6.dll".to_string()])
        );
        // Another DLL, or the kernel, is still refused.
        let other = hotswap_allowed(&["xmllite.dll".to_string()]);
        assert!(check_build_except(&m, &now, &other).unwrap_err().contains("boot/msxml6.dll"));
        now.boot[0].1 = "f".repeat(64);
        assert!(check_build_except(&m, &now, &allowed).unwrap_err().contains("kernel differs"));
        // With the kernel named too (SN step 1e), the kernel may differ:
        // it is one of those that do.
        let both = hotswap_allowed(&["kernel".to_string(), "msxml6.dll".to_string()]);
        assert!(both.contains(&KERNEL.to_string()) && !both.contains(&"boot/kernel.dll".into()));
        assert_eq!(
            check_build_except(&m, &now, &both),
            Ok(vec![
                KERNEL.to_string(),
                "boot/limine/limine.conf".to_string(),
                "boot/msxml6.dll".to_string()
            ])
        );
        // A snapshot of a hot-swapped run keeps the mark.
        let marked = Manifest { hot_swapped: Some("msxml6.dll".into()), ..m };
        assert_eq!(Manifest::parse(&marked.to_text()), Ok(marked));
    }

    #[test]
    fn requests() {
        let args = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert_eq!(parse_request(&args("boot-test hello")), Ok(None));
        assert_eq!(
            parse_request(&args("boot-test x --snapshot-at phase --snapshot-name s1")),
            Ok(Some(Request::Save { at: "phase".into(), name: "s1".into(), stop: false }))
        );
        assert_eq!(
            parse_request(&args("boot-test --from-snapshot s1")),
            Ok(Some(Request::Restore { name: "s1".into(), hotswap: None }))
        );
        assert!(parse_request(&args("boot-test x --snapshot-at phase")).is_err());
        assert!(parse_request(&args("boot-test x --snapshot-name ../x --snapshot-at p")).is_err());
        assert!(parse_request(&args("boot-test --from-snapshot s1 --snapshot-stop")).is_err());
    }

    #[test]
    fn dates() {
        assert_eq!(utc(0), "1970-01-01 00:00");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00");
        assert_eq!(utc(1_763_000_000), "2025-11-13 02:13");
    }
}
