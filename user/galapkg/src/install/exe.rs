//! Installers that are programs: InstallerType `inno` (Inno Setup),
//! `nullsoft` (NSIS) and `exe` with InstallerSwitches (ROADMAP PK stage 3),
//! run as winget runs them:
//!
//! - the installer is staged as `%TEMP%\gala\<install key>\<file>.exe`
//!   (removed again afterwards) and started with its silent switches:
//!   InstallerSwitches.Silent if the manifest has it, else winget's
//!   defaults for the type (Inno: `/SP- /VERYSILENT /SUPPRESSMSGBOXES
//!   /NORESTART`; NSIS: `/S`), then Custom, Inno's `/LOG="<file>"` (the
//!   setup log, next to gala's install log), and with `--location` the
//!   InstallLocation switch last (Inno `/DIR="<dir>"`, NSIS `/D=<dir>`,
//!   which NSIS wants last and unquoted);
//! - gala waits for it; exit code 0 is success (3010 and 1641: success,
//!   a restart is wanted), anything else a failure;
//! - the program is found through the uninstall (ARP) entry it registered:
//!   the manifest's ProductCode if that key exists, else the entries that
//!   are new since before the run (one, or the one whose DisplayName is
//!   the package's name), else (an upgrade over the old version) the
//!   entry the old version had;
//! - gala tags that entry with its tracking values (WinGetPackageIdentifier,
//!   WinGetSourceIdentifier, WinGetInstallerType, GalaPackageVersion,
//!   GalaSource, GalaInstallerSha256), so `list`, `upgrade` and
//!   `uninstall` know it. winget keeps these in its own tracking catalog
//!   instead (TODO: a catalog of gala's own, leaving vendors' keys alone);
//! - uninstall runs QuietUninstallString, else UninstallString with the
//!   type's silent switch (Inno `/VERYSILENT /SUPPRESSMSGBOXES
//!   /NORESTART`, NSIS `/S`), then waits until the ARP entry is gone: an
//!   NSIS uninstaller copies itself to %TEMP% and returns at once, the
//!   copy doing the work.
//!
//! What a vendor's installer did cannot be rolled back (winget cannot
//! either): a failure after it ran leaves its changes and says so; gala's
//! own changes (the staged file, the tracking values) still go through a
//! [`Tx`].

use super::*;

/// Installer types that are programs gala runs.
pub fn is_program(installer_type: &str) -> bool {
    matches!(installer_type, "inno" | "nullsoft" | "exe")
}

/// How long an installer or uninstaller may run.
pub const RUN_TIMEOUT_MS: u32 = 10 * 60 * 1000;
/// How long to wait for an uninstaller's ARP entry to go after it exited.
pub const GONE_TIMEOUT_MS: u32 = 60 * 1000;
const POLL_MS: u32 = 250;
/// ERROR_TIMEOUT: [`Host::run`]'s program did not end in time.
pub const ERROR_TIMEOUT: u32 = 1460;
/// ERROR_BAD_EXE_FORMAT: not an x64 program (gala has no WOW64 yet).
pub const ERROR_BAD_EXE_FORMAT: u32 = 193;

/// winget's switches for a type: (silent, log, install location).
fn defaults(installer_type: &str) -> (&'static str, Option<&'static str>, Option<&'static str>) {
    match installer_type {
        "inno" => (
            "/SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART",
            Some("/LOG=\"<LOGPATH>\""),
            Some("/DIR=\"<INSTALLPATH>\""),
        ),
        "nullsoft" => ("/S", None, Some("/D=<INSTALLPATH>")),
        _ => ("", None, None),
    }
}

/// The silent switch an uninstaller of this type takes.
fn uninstall_switch(installer_type: &str) -> Option<&'static str> {
    match installer_type {
        "inno" => Some("/VERYSILENT /SUPPRESSMSGBOXES /NORESTART"),
        "nullsoft" => Some("/S"),
        _ => None,
    }
}

fn switch<'a>(i: &'a Installer, name: &str) -> Option<&'a str> {
    i.switches
        .iter()
        .find(|(k, _)| eq_ci(k, name))
        .map(|(_, v)| v.as_str())
        .filter(|v| !v.is_empty())
}

/// The command line that runs `exe` (the staged installer) silently.
pub fn install_command(
    i: &Installer,
    exe: &str,
    log_path: Option<&str>,
    location: Option<&str>,
) -> Result<String, Error> {
    let (silent, log, install_location) = defaults(&i.installer_type);
    let silent = switch(i, "Silent").unwrap_or(silent);
    if silent.is_empty() {
        return Err(Error::unsupported(format!(
            "InstallerType {} without InstallerSwitches.Silent: gala does not run installers \
             interactively",
            i.installer_type
        )));
    }
    let mut parts = alloc::vec![format!("\"{exe}\""), silent.to_string()];
    if let Some(custom) = switch(i, "Custom") {
        parts.push(custom.to_string());
    }
    if let (Some(t), Some(p)) = (switch(i, "Log").or(log), log_path) {
        parts.push(t.replace("<LOGPATH>", p));
    }
    if let Some(dir) = location {
        let t = switch(i, "InstallLocation").or(install_location).ok_or_else(|| {
            Error::unsupported(format!(
                "--location: this {} installer has no InstallLocation switch",
                i.installer_type
            ))
        })?;
        parts.push(t.replace("<INSTALLPATH>", dir));
    }
    Ok(parts.join(" "))
}

/// The command line that removes an installed program silently, and
/// which value it came from.
pub fn uninstall_command(
    installer_type: &str,
    quiet: Option<&str>,
    uninstall: Option<&str>,
) -> Result<(String, &'static str), Error> {
    if let Some(q) = quiet.filter(|q| !q.trim().is_empty()) {
        return Ok((q.trim().to_string(), "QuietUninstallString"));
    }
    let Some(u) = uninstall.filter(|u| !u.trim().is_empty()) else {
        return Err(Error::failed("the uninstall entry has no UninstallString"));
    };
    let Some(s) = uninstall_switch(installer_type) else {
        return Err(Error::unsupported(format!(
            "an {installer_type} installer's UninstallString without QuietUninstallString: gala \
             does not run uninstallers interactively"
        )));
    };
    Ok((format!("{} {s}", u.trim()), "UninstallString"))
}

/// Every ARP entry's (root, key name).
pub(super) fn arp_keys(host: &mut dyn Host) -> Vec<(Root, String)> {
    let mut out = Vec::new();
    for root in [Root::CurrentUser, Root::LocalMachine] {
        for k in host.reg_subkeys(root, UNINSTALL_KEY) {
            out.push((root, k));
        }
    }
    out
}

fn root_name(root: Root) -> &'static str {
    match root {
        Root::CurrentUser => "HKCU",
        Root::LocalMachine => "HKLM",
    }
}

/// The ARP entry the installer registered (see the module comment):
/// (root, key name, how it was found).
fn find_entry(
    host: &mut dyn Host,
    before: &[(Root, String)],
    m: &Manifest,
    i: &Installer,
    previous: Option<&Installed>,
) -> Option<(Root, String, &'static str)> {
    let after = arp_keys(host);
    if let Some(pc) = &i.product_code
        && let Some((r, k)) = after.iter().find(|(_, k)| eq_ci(k, pc))
    {
        return Some((*r, k.clone(), "ProductCode"));
    }
    let new: Vec<&(Root, String)> = after
        .iter()
        .filter(|(r, k)| !before.iter().any(|(br, bk)| br == r && eq_ci(bk, k)))
        .collect();
    let name = m.display_name().to_string();
    let named = |host: &mut dyn Host, r: Root, k: &str| {
        sz(host, r, &format!("{UNINSTALL_KEY}\\{k}"), "DisplayName")
            .is_some_and(|d| eq_ci(&d, &name) || crate::contains_ci(&d, &name))
    };
    if new.len() == 1 {
        return Some((new[0].0, new[0].1.clone(), "the one new entry"));
    }
    for (r, k) in &new {
        if named(host, *r, k) {
            return Some((*r, k.clone(), "a new entry with the package's name"));
        }
    }
    if let Some(p) = previous
        && after.iter().any(|(r, k)| *r == p.root && eq_ci(k, &p.key))
    {
        return Some((p.root, p.key.clone(), "the previous version's entry"));
    }
    if new.is_empty() {
        // Installed over an entry gala did not know (no new key).
        for (r, k) in &after {
            if named(host, *r, k) {
                return Some((*r, k.clone(), "an existing entry with the package's name"));
            }
        }
    }
    None
}

fn temp_dir(host: &mut dyn Host) -> Result<String, Error> {
    if let Some(t) = host.env("TEMP").filter(|t| !t.is_empty()) {
        return Ok(t.trim_end_matches('\\').to_string());
    }
    let local = host
        .env("LOCALAPPDATA")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::failed("neither TEMP nor LOCALAPPDATA is set"))?;
    Ok(format!("{}\\Temp", local.trim_end_matches('\\')))
}

/// Stages the installer, runs it silently and checks its exit code.
fn run_installer(host: &mut dyn Host, p: &Prepared, location: Option<&str>) -> Result<(), Error> {
    let m = &p.manifest;
    let key = install_key(&m.id);
    let dir = format!("{}\\gala\\{key}", temp_dir(host)?);
    let mut file = p.installer.file_name().to_string();
    if !file.to_ascii_lowercase().ends_with(".exe") {
        file.push_str(".exe");
    }
    let exe = format!("{dir}\\{file}");
    let log_path = host.env("LOCALAPPDATA").map(|l| {
        format!("{}\\gala\\logs\\{}-{}.setup.log", l.trim_end_matches('\\'), m.id, m.version)
    });
    let cmd = install_command(&p.installer, &exe, log_path.as_deref(), location)?;
    let mut staging = Tx::new(host);
    let staged = staging.create_dir_all(&dir).and_then(|()| staging.create_file(&exe, &p.data));
    if let Err(e) = staged {
        return Err(fail(staging, e));
    }
    staging.host.log(&format!(
        "run type={} switches={} cmd={cmd}",
        p.installer.installer_type,
        if switch(&p.installer, "Silent").is_some() { "manifest" } else { "winget-default" }
    ));
    let r = staging.host.run(&cmd, RUN_TIMEOUT_MS);
    let left = staging.cleanup();
    if left != 0 {
        host_log(host, &format!("staged installer left behind path={exe}"));
    }
    match r {
        Err(e) if e.code == ERROR_BAD_EXE_FORMAT => {
            host.log(&format!("error run {e} (not an x64 program?)"));
            host.record_missing("gala.exe!install(32-bit installer: WOW64)");
            Err(Error::unsupported(format!(
                "{file} cannot be started: {e}; a 32-bit installer needs WOW64, which gala does \
                 not have yet"
            )))
        }
        Err(e) => {
            host.log(&format!("error run {e}"));
            Err(Error::failed(format!("{file} cannot be run: {e} (nothing was changed)")))
        }
        Ok(code) => {
            host.log(&format!("exit code={code} ({code:#x})"));
            match code {
                0 => Ok(()),
                3010 | 1641 => {
                    host.log("restart-required (the installer says so; gala does not restart)");
                    Ok(())
                }
                _ => Err(Error::failed(format!(
                    "the installer {file} exited with code {code} ({code:#x}); what it changed \
                     stays (gala cannot undo a vendor's installer){}",
                    log_path.map(|l| format!("; its log, if any: {l}")).unwrap_or_default()
                ))),
            }
        }
    }
}

fn host_log(host: &mut dyn Host, line: &str) {
    host.log(line);
}

/// Runs a program installer (install or upgrade over `previous`) and
/// tags the ARP entry it registered. Returns the entry's InstallLocation.
pub(super) fn install_program(
    host: &mut dyn Host,
    p: &Prepared,
    source_location: &str,
    location: Option<&str>,
    previous: Option<&Installed>,
) -> Result<String, Error> {
    let before = arp_keys(host);
    run_installer(host, p, location)?;
    let m = &p.manifest;
    let Some((root, key, how)) = find_entry(host, &before, m, &p.installer, previous) else {
        host.log("error no-arp-entry (no new uninstall entry, none named like the package)");
        return Err(Error::failed(format!(
            "the installer exited with 0 but registered no uninstall entry for {} (looked for \
             new keys under {UNINSTALL_KEY} in HKCU and HKLM{}): gala cannot track or remove \
             it; whatever it installed stays",
            m.display_name(),
            p.installer.product_code.as_ref().map(|c| format!(", and {c}")).unwrap_or_default()
        )));
    };
    let arp = format!("{UNINSTALL_KEY}\\{key}");
    host.log(&format!("arp-entry key={}\\{arp} found-by={how}", root_name(root)));
    let mut tx = Tx::new(host);
    let values = [
        ("WinGetPackageIdentifier", RegValue::Sz(m.id.clone())),
        ("WinGetSourceIdentifier", RegValue::Sz(WINGET_SOURCE_ID.to_string())),
        ("WinGetInstallerType", RegValue::Sz(p.installer.installer_type.clone())),
        ("GalaPackageVersion", RegValue::Sz(m.version.clone())),
        ("GalaSource", RegValue::Sz(source_location.to_string())),
        ("GalaInstallerSha256", RegValue::Sz(p.installer.sha256.clone())),
    ];
    for (name, v) in values {
        if let Err(e) = tx.set_value(root, &arp, name, v) {
            let mut e = fail(tx, e);
            e.message
                .push_str(" (the program itself stays installed: its own uninstaller removes it)");
            return Err(e);
        }
    }
    tx.commit();
    host.log(&format!("registered key={}\\{arp}", root_name(root)));
    Ok(sz(host, root, &arp, "InstallLocation").unwrap_or_default())
}

/// The installer type gala recorded for an installed package.
pub(super) fn installed_type(host: &mut dyn Host, i: &Installed) -> Option<String> {
    sz(host, i.root, &format!("{UNINSTALL_KEY}\\{}", i.key), "WinGetInstallerType")
}

/// Runs a program's uninstaller silently and waits for its ARP entry to go.
pub(super) fn uninstall_program(
    host: &mut dyn Host,
    i: &Installed,
    installer_type: &str,
) -> Result<(), Error> {
    let arp = format!("{UNINSTALL_KEY}\\{}", i.key);
    let quiet = sz(host, i.root, &arp, "QuietUninstallString");
    let plain = sz(host, i.root, &arp, "UninstallString");
    let (cmd, from) = uninstall_command(installer_type, quiet.as_deref(), plain.as_deref())?;
    host.log(&format!("run-uninstaller type={installer_type} from={from} cmd={cmd}"));
    let code = host.run(&cmd, RUN_TIMEOUT_MS).map_err(|e| {
        host_log(host, &format!("error run-uninstaller {e}"));
        Error::failed(format!("the uninstaller ({cmd}) cannot be run: {e} (nothing was changed)"))
    })?;
    host.log(&format!("exit code={code} ({code:#x})"));
    if code != 0 && code != 3010 && code != 1641 {
        return Err(Error::failed(format!(
            "the uninstaller ({cmd}) exited with code {code} ({code:#x})"
        )));
    }
    let mut waited = 0;
    while host.reg_key_exists(i.root, &arp) {
        if waited >= GONE_TIMEOUT_MS {
            host.log(&format!(
                "error arp-entry-still-there key={}\\{arp} waited={waited}ms",
                root_name(i.root)
            ));
            return Err(Error::failed(format!(
                "the uninstaller exited with 0 but {}\\{arp} is still there after {} s",
                root_name(i.root),
                waited / 1000
            )));
        }
        host.sleep(POLL_MS);
        waited += POLL_MS;
    }
    host.log(&format!("arp-entry-gone key={}\\{arp} waited={waited}ms", root_name(i.root)));
    Ok(())
}
