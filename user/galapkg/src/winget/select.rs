//! Installer selection as winget does it, and the installer's command
//! line.
//!
//! Sources (documented behaviour only): learn.microsoft.com "install
//! command (winget)" (`--architecture`, `--scope`, `--locale`,
//! `--installer-type`, `--silent`, `--interactive`, `--location`,
//! `--log`, `--override`, `--custom`), "winget settings"
//! (`installBehavior.preferences` and `installBehavior.requirements`:
//! `scope`, `locale`, `architectures`, `installerTypes`), the manifest
//! schema's descriptions (MinimumOSVersion, UnsupportedOSArchitectures,
//! Platform, Markets, InstallerSwitches, UnsupportedArguments,
//! InstallLocationRequired), and the installer technologies' own command
//! line documentation for the default switches (msiexec's, the WiX Burn
//! bundle's, Inno Setup's and NSIS's). Where these are silent the rule
//! below says "gala's rule" and is deterministic.
//!
//! **Selection** = filters, then a ranking.
//! - Requirements (command-line options and `requirements` settings)
//!   exclude; preferences (`preferences` settings, the machine's own
//!   defaults) only rank. Every excluded installer keeps its reason
//!   ([`Rejected`]): "no applicable installer" says why for each one.
//! - Architecture (winget install: by default the machine's own
//!   architecture, else ones it can run): x64 runs x64, neutral, x86
//!   (WOW64); arm64 runs arm64, neutral, x64 (Windows 11's x64
//!   emulation), x86; Arm32 code no longer runs on Windows 11 24H2
//!   (build 26100, gala's pinned build: Microsoft's "Windows 11 24H2
//!   removes Arm32 support"); x86 runs x86 and neutral. The order among
//!   them is gala's rule: native first, then neutral, then emulated
//!   (x64 before x86 on arm64). UnsupportedOSArchitectures listing the
//!   machine's architecture excludes the installer.
//! - MinimumOSVersion above the machine's version excludes (compared as
//!   versions); Platform without `Windows.Desktop` excludes (gala is a
//!   desktop Windows); Markets: AllowedMarkets without the machine's
//!   market, or ExcludedMarkets with it, exclude (no market known: gala's
//!   rule is not to filter).
//! - Scope: an installer without Scope may install either way
//!   ("unknown"); a required scope excludes the other scope but keeps
//!   unknown, ranked after an exact match (gala's rule: winget's
//!   documentation does not say).
//! - Locale: exact match, then same language (`de-AT` for `de-DE`),
//!   then an installer without InstallerLocale, then another language; a
//!   required locale excludes another language. With no locale asked for
//!   the preference is the machine's locale, then `en-US` (gala's rule:
//!   a German user gets the English installer rather than the Chinese
//!   one listed first).
//! - Installer type: a required type matches InstallerType or (for a
//!   zip) NestedInstallerType; preferred types rank in the order given,
//!   unlisted types after. An unknown type, or a zip with no
//!   NestedInstallerType, is never selected.
//! - Ranking, most significant first (gala's order; the documentation
//!   lists the settings, not their precedence): for an upgrade the
//!   installed program's installer type, then its scope; architecture
//!   (the order above or the one asked for); scope; locale; installer
//!   type preference; the manifest's order.
//!
//! **Command line** ([`command_line`]): the mode's switch (silent,
//! silent with progress (winget's default mode), interactive), then Log
//! (when a log path is given, `<LOGPATH>` replaced), then Upgrade (for an
//! upgrade), then Custom and the user's `--custom`, then InstallLocation
//! (when a location is given, `<INSTALLPATH>` replaced) last, because
//! NSIS requires `/D=` to be the last parameter (NSIS documentation,
//! "Installer usage"); the order of the others is gala's rule.
//! `--override` replaces all of it. Manifest switches replace the type's
//! defaults switch by switch; a missing Silent falls back to
//! SilentWithProgress and the reverse (gala's rule).

use super::package::{Architecture, Installer, InstallerType, Package, Scope, Switches};
use super::version;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt;

/// The machine an install is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Machine {
    /// The processor architecture of the OS (x64, arm64, x86).
    pub architecture: Architecture,
    /// The Windows version, `10.0.<build>.<revision>`.
    pub os_version: String,
    /// The user's locale (BCP 47), the default locale preference.
    pub locale: String,
    /// The market (ISO 3166 two-letter region), when known.
    pub market: Option<String>,
}

impl Machine {
    /// gala's pinned build (Windows 11 24H2, 26100) on `architecture`.
    pub fn gala(architecture: Architecture, locale: &str) -> Machine {
        Machine {
            architecture,
            os_version: "10.0.26100.0".to_string(),
            locale: locale.to_string(),
            market: None,
        }
    }

    /// The architectures this machine runs, best first.
    pub fn architectures(&self) -> &'static [Architecture] {
        use Architecture::*;
        match self.architecture {
            X64 => &[X64, Neutral, X86],
            Arm64 => &[Arm64, Neutral, X64, X86],
            X86 => &[X86, Neutral],
            Arm => &[Arm, Neutral],
            Neutral => &[Neutral],
        }
    }
}

/// A list of acceptable values, best first; `required` makes it a
/// filter, otherwise it only ranks. Empty: no preference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice<T> {
    pub values: Vec<T>,
    pub required: bool,
}

impl<T> Default for Choice<T> {
    fn default() -> Self {
        Choice { values: Vec::new(), required: false }
    }
}

impl<T> Choice<T> {
    pub fn prefer(values: Vec<T>) -> Self {
        Choice { values, required: false }
    }

    pub fn require(values: Vec<T>) -> Self {
        Choice { values, required: true }
    }
}

/// What an installed copy was installed with (for an upgrade), from its
/// ARP record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Installed {
    pub installer_type: Option<InstallerType>,
    pub scope: Option<Scope>,
}

/// The user's options and settings for one install.
#[derive(Clone, Debug, Default)]
pub struct Request {
    /// `--scope` (required) or `preferences.scope`.
    pub scope: Choice<Scope>,
    /// `--locale` or `preferences.locale`/`requirements.locale`.
    pub locales: Choice<String>,
    /// `--architecture` or the settings' `architectures`; empty: the
    /// machine's own list.
    pub architectures: Choice<Architecture>,
    /// `--installer-type` or the settings' `installerTypes`.
    pub installer_types: Choice<InstallerType>,
    /// Set for an upgrade.
    pub installed: Option<Installed>,
}

/// Why an installer was not selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejected {
    pub index: usize,
    pub reason: String,
}

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "installer {}: {}", self.index + 1, self.reason)
    }
}

/// The chosen installer and why the others were not.
#[derive(Clone, Debug)]
pub struct Selection<'p> {
    pub installer: &'p Installer,
    /// The installers that passed the filters but ranked lower, best
    /// first.
    pub runners_up: Vec<usize>,
    pub rejected: Vec<Rejected>,
}

/// No installer is applicable; each one's reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoInstaller {
    pub rejected: Vec<Rejected>,
}

impl fmt::Display for NoInstaller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("no applicable installer")?;
        for (i, r) in self.rejected.iter().enumerate() {
            f.write_str(if i == 0 { ": " } else { "; " })?;
            write!(f, "{r}")?;
        }
        Ok(())
    }
}

/// Locale closeness: 3 exact, 2 same language, 1 installer locale
/// unknown, 0 another language.
fn locale_score(installer: Option<&str>, wanted: &str) -> u8 {
    match installer {
        None => 1,
        Some(l) if l.eq_ignore_ascii_case(wanted) => 3,
        Some(l) if super::package::same_language(l, wanted) => 2,
        Some(_) => 0,
    }
}

/// Type families for matching an upgrade to what is installed (an MSI
/// written by WiX is an MSI; gala's rule).
fn same_family(a: InstallerType, b: InstallerType) -> bool {
    use InstallerType::*;
    let fam = |t| match t {
        Msi | Wix => Msi,
        Msix | Appx => Msix,
        t => t,
    };
    fam(a) == fam(b)
}

fn type_matches(i: &Installer, t: InstallerType) -> bool {
    i.installer_type == Some(t) || i.effective_type() == Some(t)
}

/// winget's choice of installer for `machine` and `request`.
pub fn select<'p>(
    package: &'p Package,
    machine: &Machine,
    request: &Request,
) -> Result<Selection<'p>, NoInstaller> {
    let arch_order: &[Architecture] = if request.architectures.values.is_empty() {
        machine.architectures()
    } else {
        &request.architectures.values
    };
    let default_locale = [machine.locale.clone(), "en-US".to_string()];
    let locales: &[String] =
        if request.locales.values.is_empty() { &default_locale } else { &request.locales.values };
    let mut rejected = Vec::new();
    // (rank key, index) of the installers that pass.
    let mut ok: Vec<([u32; 7], usize)> = Vec::new();
    for (index, i) in package.installers.iter().enumerate() {
        match rank(i, machine, request, arch_order, locales) {
            Ok(key) => ok.push((key, index)),
            Err(reason) => rejected.push(Rejected { index, reason }),
        }
    }
    ok.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let Some(&(_, best)) = ok.first() else { return Err(NoInstaller { rejected }) };
    Ok(Selection {
        installer: &package.installers[best],
        runners_up: ok.iter().skip(1).map(|&(_, i)| i).collect(),
        rejected,
    })
}

/// The filters (Err: why not) and the ranking key (lower is better).
fn rank(
    i: &Installer,
    machine: &Machine,
    request: &Request,
    arch_order: &[Architecture],
    locales: &[String],
) -> Result<[u32; 7], String> {
    let Some(ty) = i.installer_type else {
        return Err("an installer type no schema knows".to_string());
    };
    if ty == InstallerType::Zip && i.nested_installer_type.is_none() {
        return Err("a zip without NestedInstallerType".to_string());
    }
    // Architecture.
    let runs = machine.architectures();
    if !runs.contains(&i.architecture) {
        return Err(format!(
            "{} does not run on {}",
            i.architecture.as_str(),
            machine.architecture.as_str()
        ));
    }
    let arch_rank = match arch_order.iter().position(|a| *a == i.architecture) {
        Some(p) => p as u32,
        None if request.architectures.required => {
            return Err(format!("architecture {} was not asked for", i.architecture.as_str()));
        }
        None => {
            arch_order.len() as u32
                + runs.iter().position(|a| *a == i.architecture).unwrap_or(0) as u32
        }
    };
    if i.unsupported_os_architectures.contains(&machine.architecture) {
        return Err(format!("UnsupportedOSArchitectures lists {}", machine.architecture.as_str()));
    }
    // The OS.
    if let Some(min) = &i.minimum_os_version
        && version::compare(&machine.os_version, min) == Ordering::Less
    {
        return Err(format!("needs Windows {min} (this is {})", machine.os_version));
    }
    if !i.platform.is_empty() && !i.platform.iter().any(|p| p == "Windows.Desktop") {
        return Err(format!("Platform is {} only", i.platform.join(", ")));
    }
    if let Some(market) = &machine.market {
        if !i.markets.allowed.is_empty()
            && !i.markets.allowed.iter().any(|m| m.eq_ignore_ascii_case(market))
        {
            return Err(format!("not offered in market {market}"));
        }
        if i.markets.excluded.iter().any(|m| m.eq_ignore_ascii_case(market)) {
            return Err(format!("excluded in market {market}"));
        }
    }
    // Scope: 0 exact (or nothing asked), 1 unknown, 2 the other one.
    let scope_rank = match (request.scope.values.first(), i.scope) {
        (None, _) => 0,
        (Some(_), None) => 1,
        (Some(w), Some(s)) if *w == s => 0,
        (Some(w), Some(s)) => {
            if request.scope.required {
                return Err(format!("scope {} (asked for {})", s.as_str(), w.as_str()));
            }
            2
        }
    };
    // Locale: the best score over the wanted locales, earlier locales
    // first.
    let mut locale_rank = u32::MAX;
    for (p, want) in locales.iter().enumerate() {
        let score = locale_score(i.locale.as_deref(), want);
        if score > 0 {
            locale_rank = locale_rank.min(p as u32 * 4 + (3 - score as u32));
        }
    }
    if locale_rank == u32::MAX {
        if request.locales.required {
            return Err(format!(
                "locale {} (asked for {})",
                i.locale.as_deref().unwrap_or("?"),
                request.locales.values.join(", ")
            ));
        }
        locale_rank = locales.len() as u32 * 4;
    }
    // Installer type.
    let types = &request.installer_types;
    let type_rank = match types.values.iter().position(|t| type_matches(i, *t)) {
        Some(p) => p as u32,
        None if types.required && !types.values.is_empty() => {
            return Err(format!("installer type {} was not asked for", ty.as_str()));
        }
        None => types.values.len() as u32,
    };
    // An upgrade keeps what is installed.
    let (installed_type, installed_scope) = match &request.installed {
        Some(inst) => (
            match inst.installer_type {
                Some(t)
                    if i.effective_type().is_some_and(|e| same_family(e, t))
                        || same_family(ty, t) =>
                {
                    0
                }
                Some(_) => 1,
                None => 0,
            },
            match (inst.scope, i.scope) {
                (Some(a), Some(b)) if a != b => 1,
                _ => 0,
            },
        ),
        None => (0, 0),
    };
    Ok([installed_type, installed_scope, arch_rank, scope_rank, locale_rank, type_rank, 0])
}

/// How the installer is run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    /// `--silent`.
    Silent,
    /// winget's default.
    #[default]
    SilentWithProgress,
    /// `--interactive`.
    Interactive,
}

/// The user's options that shape the command line.
#[derive(Clone, Debug, Default)]
pub struct CommandOptions {
    pub mode: Mode,
    /// `--log <path>`.
    pub log: Option<String>,
    /// `--location <path>`.
    pub location: Option<String>,
    /// `--custom <args>`: appended to the manifest's Custom.
    pub custom: Option<String>,
    /// `--override <args>`: the whole command line instead.
    pub override_args: Option<String>,
    pub upgrade: bool,
}

/// The default switches of an installer type, from each technology's
/// command-line documentation (msiexec: `/quiet`, `/passive`,
/// `/norestart`, `/log`, the TARGETDIR property; WiX Burn bundles: the
/// same `/quiet /passive /norestart /log`; Inno Setup: `/SP-`,
/// `/VERYSILENT`, `/SILENT`, `/SUPPRESSMSGBOXES`, `/NORESTART`,
/// `/LOG="file"`, `/DIR="x:\dir"`; NSIS: `/S`, `/D=dir` unquoted and
/// last). Other types (exe, portable, msix, appx, pwa, font) have none:
/// an `exe` must give its own.
pub fn default_switches(t: InstallerType) -> Switches {
    let s = |x: &str| Some(x.to_string());
    match t {
        InstallerType::Msi | InstallerType::Wix => Switches {
            silent: s("/quiet /norestart"),
            silent_with_progress: s("/passive /norestart"),
            log: s("/log \"<LOGPATH>\""),
            install_location: s("TARGETDIR=\"<INSTALLPATH>\""),
            ..Switches::default()
        },
        InstallerType::Burn => Switches {
            silent: s("/quiet /norestart"),
            silent_with_progress: s("/passive /norestart"),
            log: s("/log \"<LOGPATH>\""),
            ..Switches::default()
        },
        InstallerType::Inno => Switches {
            silent: s("/SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART"),
            silent_with_progress: s("/SP- /SILENT /SUPPRESSMSGBOXES /NORESTART"),
            log: s("/LOG=\"<LOGPATH>\""),
            install_location: s("/DIR=\"<INSTALLPATH>\""),
            ..Switches::default()
        },
        InstallerType::Nullsoft => Switches {
            silent: s("/S"),
            silent_with_progress: s("/S"),
            install_location: s("/D=<INSTALLPATH>"),
            ..Switches::default()
        },
        _ => Switches::default(),
    }
}

/// The effective switches: the type's defaults (of the nested installer
/// for an archive) with the manifest's replacing them one by one.
pub fn switches(i: &Installer) -> Switches {
    let mut s = i.effective_type().map(default_switches).unwrap_or_default();
    let m = &i.switches;
    for (slot, v) in [
        (&mut s.silent, &m.silent),
        (&mut s.silent_with_progress, &m.silent_with_progress),
        (&mut s.interactive, &m.interactive),
        (&mut s.install_location, &m.install_location),
        (&mut s.log, &m.log),
        (&mut s.upgrade, &m.upgrade),
        (&mut s.custom, &m.custom),
        (&mut s.repair, &m.repair),
    ] {
        if v.is_some() {
            slot.clone_from(v);
        }
    }
    s
}

/// The installer's arguments for `opts` (see the module text for the
/// order). Err: the options cannot be honoured (a location the installer
/// does not take, a required location missing).
pub fn command_line(i: &Installer, opts: &CommandOptions) -> Result<String, String> {
    if let Some(o) = &opts.override_args {
        return Ok(o.clone());
    }
    if i.install_location_required && opts.location.is_none() {
        return Err("the installer requires an install location (--location)".to_string());
    }
    let s = switches(i);
    let mut parts: Vec<String> = Vec::new();
    let mode = match opts.mode {
        Mode::Silent => s.silent.as_ref().or(s.silent_with_progress.as_ref()),
        Mode::SilentWithProgress => s.silent_with_progress.as_ref().or(s.silent.as_ref()),
        Mode::Interactive => s.interactive.as_ref(),
    };
    parts.extend(mode.cloned());
    if let Some(log) = &opts.log
        && !i.unsupported_arguments.iter().any(|a| a == "log")
        && let Some(sw) = &s.log
    {
        parts.push(sw.replace("<LOGPATH>", log));
    }
    if opts.upgrade {
        parts.extend(s.upgrade.clone());
    }
    parts.extend(s.custom.clone());
    parts.extend(opts.custom.clone());
    if let Some(location) = &opts.location {
        if i.unsupported_arguments.iter().any(|a| a == "location") {
            return Err("the installer does not support an install location".to_string());
        }
        match &s.install_location {
            Some(sw) => parts.push(sw.replace("<INSTALLPATH>", location)),
            None => return Err("the installer has no InstallLocation switch".to_string()),
        }
    }
    parts.retain(|p| !p.trim().is_empty());
    Ok(parts.iter().map(|p| p.trim()).collect::<Vec<_>>().join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const SHA: &str = "71B2FEF860ABE467217A538FF31DE02F5258807C0129F771846F87BD029AAFC5";

    fn package(installers: &str) -> Package {
        let text = format!(
            "PackageIdentifier: A.B\nPackageVersion: 1.0\nPackageLocale: en-US\nPublisher: P\nPackageName: N\nLicense: MIT\nShortDescription: d\nManifestType: singleton\nManifestVersion: 1.10.0\nInstallerSha256: {SHA}\nInstallers:\n{installers}"
        );

        Package::from_texts(&[&text]).unwrap()
    }

    fn inst(arch: &str, ty: &str, url: &str, extra: &str) -> String {
        let mut s =
            format!("- Architecture: {arch}\n  InstallerType: {ty}\n  InstallerUrl: {url}\n");
        for l in extra.lines() {
            s.push_str("  ");
            s.push_str(l);
            s.push('\n');
        }
        s
    }

    fn pick(p: &Package, m: &Machine, r: &Request) -> String {
        match select(p, m, r) {
            Ok(s) => s.installer.url.clone(),
            Err(e) => e.to_string(),
        }
    }

    fn x64() -> Machine {
        Machine::gala(Architecture::X64, "en-US")
    }

    fn arm64() -> Machine {
        Machine::gala(Architecture::Arm64, "en-US")
    }

    #[test]
    fn architecture_native_neutral_emulated() {
        let all = [
            inst("x86", "exe", "x86", ""),
            inst("neutral", "exe", "neutral", ""),
            inst("x64", "exe", "x64", ""),
            inst("arm64", "exe", "arm64", ""),
            inst("arm", "exe", "arm", ""),
        ]
        .concat();
        let p = package(&all);
        let r = Request::default();
        assert_eq!(pick(&p, &x64(), &r), "x64");
        assert_eq!(pick(&p, &arm64(), &r), "arm64");
        let s = select(&p, &arm64(), &r).unwrap();
        // neutral, x64, x86; arm (32-bit) is rejected on 24H2.
        assert_eq!(s.runners_up, [1, 2, 0]);
        assert_eq!(s.rejected, [Rejected { index: 4, reason: "arm does not run on arm64".into() }]);
        let s = select(&p, &x64(), &r).unwrap();
        assert_eq!(s.runners_up, [1, 0]);
        assert_eq!(s.rejected.len(), 2);
        // Without native: neutral, then emulation.
        let p = package(&[inst("x86", "exe", "x86", ""), inst("x64", "exe", "x64", "")].concat());
        assert_eq!(pick(&p, &arm64(), &r), "x64");
        let p = package(&[inst("x86", "exe", "x86", ""), inst("neutral", "exe", "n", "")].concat());
        assert_eq!(pick(&p, &x64(), &r), "n");
        // --architecture x86 (required).
        let r = Request {
            architectures: Choice::require(vec![Architecture::X86]),
            ..Request::default()
        };
        assert_eq!(pick(&p, &x64(), &r), "x86");
        let p = package(&inst("x64", "exe", "x64", ""));
        assert_eq!(
            pick(&p, &x64(), &r),
            "no applicable installer: installer 1: architecture x64 was not asked for"
        );
        // A preferred order only ranks.
        let r = Request {
            architectures: Choice::prefer(vec![Architecture::X86]),
            ..Request::default()
        };
        assert_eq!(pick(&p, &x64(), &r), "x64");
    }

    #[test]
    fn unsupported_os_and_minimum_version() {
        let p = package(
            &[
                inst("x64", "exe", "a", "UnsupportedOSArchitectures: [x64]"),
                inst("x64", "exe", "b", "MinimumOSVersion: 10.0.26200.0"),
                inst("x64", "exe", "c", "Platform: [Windows.Universal]"),
                inst("x64", "exe", "d", "MinimumOSVersion: 10.0.22000.0"),
            ]
            .concat(),
        );
        let s = select(&p, &x64(), &Request::default()).unwrap();
        assert_eq!(s.installer.url, "d");
        let reasons: Vec<String> = s.rejected.iter().map(|r| r.to_string()).collect();
        assert_eq!(
            reasons,
            [
                "installer 1: UnsupportedOSArchitectures lists x64",
                "installer 2: needs Windows 10.0.26200.0 (this is 10.0.26100.0)",
                "installer 3: Platform is Windows.Universal only",
            ]
        );
    }

    #[test]
    fn markets() {
        let p = package(
            &[
                inst("x64", "exe", "us", "Markets:\n  AllowedMarkets: [US]"),
                inst("x64", "exe", "notcn", "Markets:\n  ExcludedMarkets: [CN]\nScope: user"),
            ]
            .concat(),
        );
        let mut m = x64();
        assert_eq!(pick(&p, &m, &Request::default()), "us");
        m.market = Some("CN".into());
        assert_eq!(
            pick(&p, &m, &Request::default()),
            "no applicable installer: installer 1: not offered in market CN; installer 2: excluded in market CN"
        );
        m.market = Some("DE".into());
        assert_eq!(pick(&p, &m, &Request::default()), "notcn");
    }

    #[test]
    fn scope_preference_and_requirement() {
        let p = package(
            &[
                inst("x64", "exe", "machine", "Scope: machine"),
                inst("x64", "exe", "unknown", ""),
                inst("x64", "exe", "user", "Scope: user"),
            ]
            .concat(),
        );
        let m = x64();
        assert_eq!(pick(&p, &m, &Request::default()), "machine");
        let user = Request { scope: Choice::prefer(vec![Scope::User]), ..Request::default() };
        assert_eq!(pick(&p, &m, &user), "user");
        let req = Request { scope: Choice::require(vec![Scope::User]), ..Request::default() };
        let s = select(&p, &m, &req).unwrap();
        assert_eq!(s.installer.url, "user");
        assert_eq!(s.runners_up, [1]);
        assert_eq!(s.rejected[0].reason, "scope machine (asked for user)");
        // Required scope with only unknown/other: unknown is kept.
        let p = package(
            &[inst("x64", "exe", "machine", "Scope: machine"), inst("x64", "exe", "unknown", "")]
                .concat(),
        );
        assert_eq!(pick(&p, &m, &req), "unknown");
        // Architecture outranks scope.
        let p = package(
            &[
                inst("x86", "exe", "x86user", "Scope: user"),
                inst("x64", "exe", "x64machine", "Scope: machine"),
            ]
            .concat(),
        );
        assert_eq!(pick(&p, &m, &user), "x64machine");
    }

    #[test]
    fn locale_matching() {
        let p = package(
            &[
                inst("x64", "exe", "en", "InstallerLocale: en-US"),
                inst("x64", "exe", "de", "InstallerLocale: de-DE"),
                inst("x64", "exe", "none", ""),
            ]
            .concat(),
        );
        let en = Machine::gala(Architecture::X64, "en-US");
        let de = Machine::gala(Architecture::X64, "de-DE");
        let at = Machine::gala(Architecture::X64, "de-AT");
        let fr = Machine::gala(Architecture::X64, "fr-FR");
        let r = Request::default();
        assert_eq!(pick(&p, &en, &r), "en");
        assert_eq!(pick(&p, &de, &r), "de");
        assert_eq!(pick(&p, &at, &r), "de");
        assert_eq!(pick(&p, &fr, &r), "none");
        // No match for the machine's locale: en-US before another one.
        let p2 = package(
            &[
                inst("x64", "exe", "zh", "InstallerLocale: zh-CN"),
                inst("x64", "exe", "en", "InstallerLocale: en-US"),
            ]
            .concat(),
        );
        assert_eq!(pick(&p2, &de, &r), "en");
        let req = Request { locales: Choice::require(vec!["fr-FR".into()]), ..Request::default() };
        let s = select(&p, &en, &req).unwrap();
        assert_eq!(s.installer.url, "none");
        assert_eq!(s.rejected[0].reason, "locale en-US (asked for fr-FR)");
        let p = package(&inst("x64", "exe", "en", "InstallerLocale: en-US"));
        assert!(select(&p, &en, &req).is_err());
        // A preference list: the first that matches wins.
        let pref = Request {
            locales: Choice::prefer(vec!["ja-JP".into(), "de-CH".into()]),
            ..Request::default()
        };
        let p = package(
            &[
                inst("x64", "exe", "en", "InstallerLocale: en-US"),
                inst("x64", "exe", "de", "InstallerLocale: de-DE"),
            ]
            .concat(),
        );
        assert_eq!(pick(&p, &en, &pref), "de");
    }

    #[test]
    fn installer_type_preference_and_requirement() {
        let p = package(
            &[
                inst("x64", "exe", "exe", "InstallerSwitches:\n  Silent: /s"),
                inst("x64", "msi", "msi", ""),
                inst("x64", "zip", "zip", "NestedInstallerType: portable\nNestedInstallerFiles:\n- RelativeFilePath: a.exe"),
                inst("x64", "zip", "badzip", ""),
            ]
            .concat(),
        );
        let m = x64();
        assert_eq!(pick(&p, &m, &Request::default()), "exe");
        let r = Request {
            installer_types: Choice::prefer(vec![InstallerType::Msi]),
            ..Request::default()
        };
        assert_eq!(pick(&p, &m, &r), "msi");
        let r = Request {
            installer_types: Choice::require(vec![InstallerType::Portable]),
            ..Request::default()
        };
        let s = select(&p, &m, &r).unwrap();
        assert_eq!(s.installer.url, "zip");
        assert_eq!(
            s.rejected.iter().map(|r| r.reason.as_str()).collect::<Vec<_>>(),
            [
                "installer type exe was not asked for",
                "installer type msi was not asked for",
                "a zip without NestedInstallerType"
            ]
        );
    }

    #[test]
    fn upgrade_keeps_installed_type_and_scope() {
        let p = package(
            &[
                inst("x64", "exe", "exe-user", "Scope: user"),
                inst("x64", "wix", "msi-machine", "Scope: machine"),
                inst("x64", "msi", "msi-user", "Scope: user"),
            ]
            .concat(),
        );
        let m = x64();
        let installed =
            Installed { installer_type: Some(InstallerType::Msi), scope: Some(Scope::Machine) };
        let r = Request { installed: Some(installed), ..Request::default() };
        assert_eq!(pick(&p, &m, &r), "msi-machine");
        let installed = Installed { installer_type: Some(InstallerType::Msi), scope: None };
        let r = Request { installed: Some(installed), ..Request::default() };
        assert_eq!(pick(&p, &m, &r), "msi-machine");
        let r = Request { installed: Some(Installed::default()), ..Request::default() };
        assert_eq!(pick(&p, &m, &r), "exe-user");
    }

    #[test]
    fn unknown_type_never_selected() {
        let p = package(&inst("x64", "msstore", "s", ""));
        assert_eq!(
            pick(&p, &x64(), &Request::default()),
            "no applicable installer: installer 1: an installer type no schema knows"
        );
    }

    fn one(ty: &str, extra: &str) -> Installer {
        let mut e = String::from(extra);
        if ty == "zip" {
            e.push_str(
                "\nNestedInstallerType: inno\nNestedInstallerFiles:\n- RelativeFilePath: setup.exe",
            );
        }
        package(&inst("x64", ty, "u", &e)).installers.remove(0)
    }

    #[test]
    fn default_switches_per_type() {
        let o = CommandOptions::default();
        let silent = CommandOptions { mode: Mode::Silent, ..CommandOptions::default() };
        assert_eq!(command_line(&one("msi", ""), &o).unwrap(), "/passive /norestart");
        assert_eq!(command_line(&one("wix", ""), &silent).unwrap(), "/quiet /norestart");
        assert_eq!(command_line(&one("burn", ""), &silent).unwrap(), "/quiet /norestart");
        assert_eq!(
            command_line(&one("inno", ""), &silent).unwrap(),
            "/SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART"
        );
        assert_eq!(
            command_line(&one("inno", ""), &o).unwrap(),
            "/SP- /SILENT /SUPPRESSMSGBOXES /NORESTART"
        );
        assert_eq!(command_line(&one("nullsoft", ""), &o).unwrap(), "/S");
        assert_eq!(command_line(&one("exe", ""), &o).unwrap(), "");
        assert_eq!(command_line(&one("portable", ""), &o).unwrap(), "");
        // A zip's nested installer's defaults.
        assert_eq!(
            command_line(&one("zip", ""), &silent).unwrap(),
            "/SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART"
        );
        let interactive = CommandOptions { mode: Mode::Interactive, ..CommandOptions::default() };
        assert_eq!(command_line(&one("msi", ""), &interactive).unwrap(), "");
    }

    #[test]
    fn manifest_switches_and_options() {
        let exe = one(
            "exe",
            "InstallerSwitches:\n  Silent: /quiet\n  Log: --log=<LOGPATH>\n  Upgrade: --upgrade\n  Custom: --no-shortcut\n  InstallLocation: --dir=<INSTALLPATH>",
        );
        let all = CommandOptions {
            mode: Mode::SilentWithProgress,
            log: Some("C:\\l.txt".into()),
            location: Some("D:\\App".into()),
            custom: Some("--extra".into()),
            upgrade: true,
            ..CommandOptions::default()
        };
        // SilentWithProgress falls back to Silent.
        assert_eq!(
            command_line(&exe, &all).unwrap(),
            "/quiet --log=C:\\l.txt --upgrade --no-shortcut --extra --dir=D:\\App"
        );
        let over = CommandOptions { override_args: Some("/x".into()), ..all.clone() };
        assert_eq!(command_line(&exe, &over).unwrap(), "/x");
        // NSIS: /D= last.
        let nsis = one("nullsoft", "InstallerSwitches:\n  Custom: /NCRC");
        assert_eq!(command_line(&nsis, &all).unwrap(), "/S /NCRC --extra /D=D:\\App");
        // A manifest switch replaces the default one only.
        let msi = one("msi", "InstallerSwitches:\n  Silent: /qn");
        let silent_log = CommandOptions {
            mode: Mode::Silent,
            log: Some("l".into()),
            ..CommandOptions::default()
        };
        assert_eq!(command_line(&msi, &silent_log).unwrap(), "/qn /log \"l\"");
        // UnsupportedArguments and InstallLocationRequired.
        let nolog = one("msi", "UnsupportedArguments: [log, location]");
        assert_eq!(command_line(&nolog, &silent_log).unwrap(), "/quiet /norestart");
        let loc = CommandOptions { location: Some("x".into()), ..CommandOptions::default() };
        assert_eq!(
            command_line(&nolog, &loc).unwrap_err(),
            "the installer does not support an install location"
        );
        assert_eq!(
            command_line(&one("burn", ""), &loc).unwrap_err(),
            "the installer has no InstallLocation switch"
        );
        let required = one("inno", "InstallLocationRequired: true");
        assert!(
            command_line(&required, &CommandOptions::default())
                .unwrap_err()
                .contains("requires an install location")
        );
        assert_eq!(
            command_line(&required, &loc).unwrap(),
            "/SP- /SILENT /SUPPRESSMSGBOXES /NORESTART /DIR=\"x\""
        );
    }
}
