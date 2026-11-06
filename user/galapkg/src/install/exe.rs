//! Installers that are programs: InstallerType `inno` (Inno Setup),
//! `nullsoft` (NSIS) and `exe` with InstallerSwitches (ROADMAP PK stage 3),
//! run as winget runs them:
//!
//! - the installer is staged as `%TEMP%\gala\<install key>\<file>.exe`
//!   (removed again afterwards) and started with the arguments the
//!   resolver builds ([`wsel::command_line`]): the mode's switch (winget's
//!   default is silent with progress, `--silent` silent: the manifest's
//!   InstallerSwitches, else the type's defaults, Inno `/SP- /SILENT|
//!   /VERYSILENT /SUPPRESSMSGBOXES /NORESTART`, NSIS `/S`), the Log switch
//!   with the setup log (`--log`, else
//!   `%LOCALAPPDATA%\gala\logs\<id>-<version>.setup.log`), Upgrade on an
//!   upgrade, Custom and `--custom`, and with `--location` the
//!   InstallLocation switch last (Inno `/DIR="<dir>"`, NSIS `/D=<dir>`,
//!   which NSIS wants last and unquoted); `--override` replaces them all.
//!   An `exe` with neither Silent nor SilentWithProgress is refused unless
//!   `--interactive` (gala does not run installers interactively on its
//!   own);
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
use crate::winget::select::{CommandOptions, Mode};

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

/// The silent switch an uninstaller of this type takes.
fn uninstall_switch(installer_type: &str) -> Option<&'static str> {
    match installer_type {
        "inno" => Some("/VERYSILENT /SUPPRESSMSGBOXES /NORESTART"),
        "nullsoft" => Some("/S"),
        _ => None,
    }
}

/// The command line that runs `exe` (the staged installer) as `opts`
/// asks.
pub fn install_command(i: &Installer, exe: &str, opts: &CommandOptions) -> Result<String, Error> {
    let t = effective_name(i);
    if opts.override_args.is_none() && opts.mode != Mode::Interactive {
        let s = wsel::switches(i);
        if s.silent.is_none() && s.silent_with_progress.is_none() {
            return Err(Error::unsupported(format!(
                "InstallerType {t} without InstallerSwitches.Silent: gala does not run installers \
                 interactively unless asked to (--interactive)"
            )));
        }
    }
    let args = wsel::command_line(i, opts)
        .map_err(|e| Error::unsupported(format!("this {t} installer: {e}")))?;
    Ok(if args.is_empty() { format!("\"{exe}\"") } else { format!("\"{exe}\" {args}") })
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
    m: &Package,
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

/// Stages the installer, runs it as [`install_command`] says and checks
/// its exit code.
fn run_installer(host: &mut dyn Host, p: &Prepared) -> Result<(), Error> {
    let m = &p.package;
    let key = install_key(&m.id);
    let dir = format!("{}\\gala\\{key}", temp_dir(host)?);
    let mut file = p.installer.file_name().to_string();
    if !file.to_ascii_lowercase().ends_with(".exe") {
        file.push_str(".exe");
    }
    let exe = format!("{dir}\\{file}");
    let log_path = p.command.log.clone().or_else(|| {
        host.env("LOCALAPPDATA").map(|l| {
            format!("{}\\gala\\logs\\{}-{}.setup.log", l.trim_end_matches('\\'), m.id, m.version)
        })
    });
    let command = CommandOptions { log: log_path.clone(), ..p.command.clone() };
    let cmd = install_command(&p.installer, &exe, &command)?;
    let mut staging = Tx::new(host);
    let staged = staging.create_dir_all(&dir).and_then(|()| staging.create_file(&exe, &p.data));
    if let Err(e) = staged {
        return Err(fail(staging, e));
    }
    let sw = &p.installer.switches;
    let from = if command.override_args.is_some() {
        "override"
    } else if sw.silent.is_some() || sw.silent_with_progress.is_some() {
        "manifest"
    } else {
        "winget-default"
    };
    let mode = match command.mode {
        Mode::Silent => "silent",
        Mode::SilentWithProgress => "silent-with-progress",
        Mode::Interactive => "interactive",
    };
    staging.host.log(&format!(
        "run type={} mode={mode} switches={from} cmd={cmd}",
        effective_name(&p.installer)
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
    previous: Option<&Installed>,
) -> Result<String, Error> {
    let before = arp_keys(host);
    run_installer(host, p)?;
    let m = &p.package;
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
        ("WinGetInstallerType", RegValue::Sz(effective_name(&p.installer).to_string())),
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
