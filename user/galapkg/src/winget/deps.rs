//! Dependencies into an install plan.
//!
//! The manifest's `Dependencies` (schema 1.0.0 on; learn.microsoft.com
//! "winget install": "winget installs a package's dependencies first"):
//! WindowsFeatures and WindowsLibraries (parts of Windows to enable or
//! have), PackageDependencies (packages of the same source, each with an
//! optional MinimumVersion), ExternalDependencies (things outside winget,
//! only reported). The dependencies are the *selected installer's*
//! (installer level, else the root's), so each package's installer is
//! selected before its dependencies are read.
//!
//! The plan (gala's rules where the documentation is silent, all
//! deterministic):
//! - packages in install order: every dependency before what needs it
//!   (depth first, in the order the manifest lists them), the requested
//!   package last; each package once;
//! - a dependency already installed at MinimumVersion or later is
//!   satisfied and not looked into further (it brought its own);
//! - otherwise its latest version in the source is planned, which must
//!   be at least every MinimumVersion asked of it, else an error naming
//!   who asked;
//! - a cycle (A needs B needs A) is an error naming the path;
//! - Windows features, Windows libraries and external dependencies are
//!   collected once each (any case), in first-seen order, with the
//!   package that needs them.

use super::package::Package;
use super::select::{self, Machine, NoInstaller, Request};
use super::version;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// Where dependencies come from: the source's packages and the machine's
/// installed ones.
pub trait Catalog {
    /// The versions of `id` the source has (any order).
    fn versions(&self, id: &str) -> Vec<String>;
    /// One version's manifests, read.
    fn load(&self, id: &str, version: &str) -> Result<Package, String>;
    /// The version of `id` installed on the machine, if any.
    fn installed(&self, id: &str) -> Option<String>;
}

/// One package in the plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub id: String,
    pub version: String,
    /// Already installed at a sufficient version: nothing to do.
    pub satisfied: bool,
    /// The selected installer's index in that package's manifest (None
    /// when satisfied).
    pub installer: Option<usize>,
    /// The package that needs it (None for the requested one).
    pub required_by: Option<String>,
}

/// A Windows feature, library or external dependency and who needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Need {
    pub name: String,
    pub required_by: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Install order; the requested package is last.
    pub packages: Vec<Step>,
    pub windows_features: Vec<Need>,
    pub windows_libraries: Vec<Need>,
    pub external: Vec<Need>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A → B → A.
    Cycle(Vec<String>),
    /// The source has no version of `id` (at least `minimum`).
    NotFound { id: String, minimum: Option<String>, required_by: String },
    /// The planned version is older than a later MinimumVersion.
    TooOld { id: String, planned: String, minimum: String, required_by: String },
    /// A dependency's manifests could not be read.
    Load { id: String, version: String, message: String },
    /// No installer of the package applies to this machine.
    NoInstaller { id: String, version: String, why: NoInstaller },
    /// Dependencies nested deeper than [`MAX_DEPTH`].
    TooDeep(Vec<String>),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Cycle(path) => write!(f, "a dependency cycle: {}", path.join(" -> ")),
            Error::NotFound { id, minimum: Some(m), required_by } => {
                write!(f, "{required_by} needs {id} {m} or later, which the source does not have")
            }
            Error::NotFound { id, minimum: None, required_by } => {
                write!(f, "{required_by} needs {id}, which the source does not have")
            }
            Error::TooOld { id, planned, minimum, required_by } => {
                write!(
                    f,
                    "{required_by} needs {id} {minimum} or later, but {planned} is the latest"
                )
            }
            Error::Load { id, version, message } => write!(f, "{id} {version}: {message}"),
            Error::NoInstaller { id, version, why } => write!(f, "{id} {version}: {why}"),
            Error::TooDeep(path) => {
                write!(f, "dependencies nested too deep: {}", path.join(" -> "))
            }
        }
    }
}

/// Dependency chains longer than this are refused (a source with a very
/// long chain is more likely broken than real).
pub const MAX_DEPTH: usize = 32;

struct Planner<'a, C: Catalog> {
    catalog: &'a C,
    machine: &'a Machine,
    request: Request,
    plan: Plan,
    stack: Vec<String>,
}

/// The install plan for `package` (already read) on `machine` with
/// `request` (the user's options; dependencies are selected with the
/// same machine and preferences, without the upgrade's installed state
/// or a required installer type, which were asked of the package only:
/// gala's rule).
pub fn plan<C: Catalog>(
    package: &Package,
    machine: &Machine,
    request: &Request,
    catalog: &C,
) -> Result<Plan, Error> {
    let mut dep_request = request.clone();
    dep_request.installed = None;
    if dep_request.installer_types.required {
        dep_request.installer_types = Default::default();
    }
    let mut p = Planner {
        catalog,
        machine,
        request: dep_request,
        plan: Plan::default(),
        stack: Vec::new(),
    };
    let selected = select::select(package, machine, request).map_err(|why| Error::NoInstaller {
        id: package.id.clone(),
        version: package.version.clone(),
        why,
    })?;
    p.visit(package, selected.installer.index, None)?;
    Ok(p.plan)
}

fn push_need(list: &mut Vec<Need>, name: &str, by: &str) {
    if !list.iter().any(|n| n.name.eq_ignore_ascii_case(name)) {
        list.push(Need { name: name.to_string(), required_by: by.to_string() });
    }
}

impl<C: Catalog> Planner<'_, C> {
    fn visit(
        &mut self,
        package: &Package,
        installer: usize,
        required_by: Option<&str>,
    ) -> Result<(), Error> {
        self.stack.push(package.id.clone());
        if self.stack.len() > MAX_DEPTH {
            return Err(Error::TooDeep(self.stack.clone()));
        }
        let deps = &package.installers[installer].dependencies;
        let me = package.id.as_str();
        for f in &deps.windows_features {
            push_need(&mut self.plan.windows_features, f, me);
        }
        for l in &deps.windows_libraries {
            push_need(&mut self.plan.windows_libraries, l, me);
        }
        for x in &deps.external {
            push_need(&mut self.plan.external, x, me);
        }
        for d in &deps.packages {
            self.dependency(&d.id, d.minimum_version.as_deref(), me)?;
        }
        self.stack.pop();
        self.plan.packages.push(Step {
            id: package.id.clone(),
            version: package.version.clone(),
            satisfied: false,
            installer: Some(installer),
            required_by: required_by.map(ToString::to_string),
        });
        Ok(())
    }

    fn dependency(&mut self, id: &str, minimum: Option<&str>, by: &str) -> Result<(), Error> {
        if let Some(at) = self.stack.iter().position(|s| s.eq_ignore_ascii_case(id)) {
            let mut path = self.stack[at..].to_vec();
            path.push(id.to_string());
            return Err(Error::Cycle(path));
        }
        if let Some(step) = self.plan.packages.iter().find(|s| s.id.eq_ignore_ascii_case(id)) {
            if version::at_least(&step.version, minimum) {
                return Ok(());
            }
            return Err(Error::TooOld {
                id: id.to_string(),
                planned: step.version.clone(),
                minimum: minimum.unwrap_or("").to_string(),
                required_by: by.to_string(),
            });
        }
        if let Some(v) = self.catalog.installed(id)
            && version::at_least(&v, minimum)
        {
            self.plan.packages.push(Step {
                id: id.to_string(),
                version: v,
                satisfied: true,
                installer: None,
                required_by: Some(by.to_string()),
            });
            return Ok(());
        }
        let versions = self.catalog.versions(id);
        let Some(latest) = version::latest(versions.iter().map(String::as_str)) else {
            return Err(Error::NotFound {
                id: id.to_string(),
                minimum: minimum.map(ToString::to_string),
                required_by: by.to_string(),
            });
        };
        if !version::at_least(latest, minimum) {
            return Err(Error::TooOld {
                id: id.to_string(),
                planned: latest.to_string(),
                minimum: minimum.unwrap_or("").to_string(),
                required_by: by.to_string(),
            });
        }
        let latest = latest.to_string();
        let package = self.catalog.load(id, &latest).map_err(|message| Error::Load {
            id: id.to_string(),
            version: latest.clone(),
            message,
        })?;
        let selected = select::select(&package, self.machine, &self.request).map_err(|why| {
            Error::NoInstaller { id: id.to_string(), version: latest.clone(), why }
        })?;
        let index = selected.installer.index;
        self.visit(&package, index, Some(by))
    }
}

/// A human-readable plan, one line per step (for the install log).
pub fn describe(plan: &Plan) -> Vec<String> {
    let mut out = Vec::new();
    for n in &plan.windows_features {
        out.push(format!("windows feature {} (for {})", n.name, n.required_by));
    }
    for n in &plan.windows_libraries {
        out.push(format!("windows library {} (for {})", n.name, n.required_by));
    }
    for n in &plan.external {
        out.push(format!("external {} (for {}; not installed by gala)", n.name, n.required_by));
    }
    for s in &plan.packages {
        let by = s.required_by.as_deref().map(|b| format!(" (for {b})")).unwrap_or_default();
        if s.satisfied {
            out.push(format!("installed {} {}{by}", s.id, s.version));
        } else {
            out.push(format!(
                "install {} {} installer {}{by}",
                s.id,
                s.version,
                s.installer.map_or(0, |i| i + 1)
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::winget::package::Architecture;

    const SHA: &str = "71B2FEF860ABE467217A538FF31DE02F5258807C0129F771846F87BD029AAFC5";

    /// A package whose x64 installer has `deps` (YAML under
    /// Dependencies) and an x86 one without.
    fn pkg(id: &str, version: &str, deps: &str) -> Package {
        let mut d = String::new();
        for l in deps.lines() {
            d.push_str("    ");
            d.push_str(l);
            d.push('\n');
        }
        let deps_block =
            if deps.is_empty() { String::new() } else { format!("  Dependencies:\n{d}") };
        let text = format!(
            "PackageIdentifier: {id}\nPackageVersion: {version}\nPackageLocale: en-US\nPublisher: P\nPackageName: N\nLicense: MIT\nShortDescription: d\nManifestType: singleton\nManifestVersion: 1.10.0\nInstallers:\n- Architecture: x64\n  InstallerType: exe\n  InstallerUrl: https://h/{id}\n  InstallerSha256: {SHA}\n{deps_block}"
        );
        Package::from_texts(&[&text]).unwrap()
    }

    struct Src {
        pkgs: Vec<(String, String, String)>,
        installed: Vec<(String, String)>,
    }

    impl Catalog for Src {
        fn versions(&self, id: &str) -> Vec<String> {
            self.pkgs.iter().filter(|p| p.0.eq_ignore_ascii_case(id)).map(|p| p.1.clone()).collect()
        }
        fn load(&self, id: &str, version: &str) -> Result<Package, String> {
            let p = self
                .pkgs
                .iter()
                .find(|p| p.0.eq_ignore_ascii_case(id) && p.1 == version)
                .ok_or("missing")?;
            Ok(pkg(&p.0, &p.1, &p.2))
        }
        fn installed(&self, id: &str) -> Option<String> {
            self.installed.iter().find(|p| p.0.eq_ignore_ascii_case(id)).map(|p| p.1.clone())
        }
    }

    fn src(pkgs: &[(&str, &str, &str)], installed: &[(&str, &str)]) -> Src {
        Src {
            pkgs: pkgs
                .iter()
                .map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()))
                .collect(),
            installed: installed.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
        }
    }

    fn run(root: &Package, s: &Src) -> Result<Vec<String>, String> {
        let m = Machine::gala(Architecture::X64, "en-US");
        plan(root, &m, &Request::default(), s).map(|p| describe(&p)).map_err(|e| e.to_string())
    }

    #[test]
    fn ordered_depth_first_once_each() {
        let s = src(
            &[
                ("B", "1.0", "PackageDependencies:\n- PackageIdentifier: D"),
                (
                    "B",
                    "2.0",
                    "PackageDependencies:\n- PackageIdentifier: D\n  MinimumVersion: '1.5'",
                ),
                (
                    "C",
                    "1.0",
                    "PackageDependencies:\n- PackageIdentifier: D\nWindowsLibraries: [Microsoft.UI.Xaml.2.8]",
                ),
                ("D", "1.10", "WindowsFeatures: [netfx3]"),
                ("D", "1.9", ""),
            ],
            &[],
        );
        let root = pkg(
            "A",
            "1",
            "PackageDependencies:\n- PackageIdentifier: B\n- PackageIdentifier: C\nWindowsFeatures: [NetFx3]\nExternalDependencies: [Java 8]",
        );
        assert_eq!(
            run(&root, &s).unwrap(),
            [
                "windows feature NetFx3 (for A)",
                "windows library Microsoft.UI.Xaml.2.8 (for C)",
                "external Java 8 (for A; not installed by gala)",
                "install D 1.10 installer 1 (for B)",
                "install B 2.0 installer 1 (for A)",
                "install C 1.0 installer 1 (for A)",
                "install A 1 installer 1",
            ]
        );
    }

    #[test]
    fn installed_satisfies_minimum() {
        let s = src(&[("VC", "14.40", ""), ("VC", "14.20", "")], &[("vc", "14.38.33130.0")]);
        let root =
            pkg("A", "1", "PackageDependencies:\n- PackageIdentifier: VC\n  MinimumVersion: 14.30");
        assert_eq!(
            run(&root, &s).unwrap(),
            ["installed VC 14.38.33130.0 (for A)", "install A 1 installer 1"]
        );
        let root =
            pkg("A", "1", "PackageDependencies:\n- PackageIdentifier: VC\n  MinimumVersion: 14.39");
        assert_eq!(
            run(&root, &s).unwrap(),
            ["install VC 14.40 installer 1 (for A)", "install A 1 installer 1"]
        );
    }

    #[test]
    fn errors() {
        let s = src(
            &[
                ("B", "1", "PackageDependencies:\n- PackageIdentifier: C"),
                ("C", "1", "PackageDependencies:\n- PackageIdentifier: b"),
                ("Old", "1.0", ""),
            ],
            &[],
        );
        let root = pkg("A", "1", "PackageDependencies:\n- PackageIdentifier: B");
        assert_eq!(run(&root, &s).unwrap_err(), "a dependency cycle: B -> C -> b");
        let root = pkg("A", "1", "PackageDependencies:\n- PackageIdentifier: A");
        assert_eq!(run(&root, &s).unwrap_err(), "a dependency cycle: A -> A");
        let root =
            pkg("A", "1", "PackageDependencies:\n- PackageIdentifier: Nope\n  MinimumVersion: 2");
        assert_eq!(
            run(&root, &s).unwrap_err(),
            "A needs Nope 2 or later, which the source does not have"
        );
        let root =
            pkg("A", "1", "PackageDependencies:\n- PackageIdentifier: Old\n  MinimumVersion: 1.1");
        assert_eq!(run(&root, &s).unwrap_err(), "A needs Old 1.1 or later, but 1.0 is the latest");
        // A second, higher minimum for a package already planned.
        let s2 = src(
            &[("B", "1", "PackageDependencies:\n- PackageIdentifier: Old"), ("Old", "1.0", "")],
            &[],
        );
        let root = pkg(
            "A",
            "1",
            "PackageDependencies:\n- PackageIdentifier: B\n- PackageIdentifier: Old\n  MinimumVersion: '2.0'",
        );
        assert_eq!(run(&root, &s2).unwrap_err(), "A needs Old 2.0 or later, but 1.0 is the latest");
    }

    #[test]
    fn dependency_without_applicable_installer() {
        // The dependency has only an x64 installer; on an x86 machine it
        // cannot be installed.
        let s = src(&[("B", "1", "")], &[]);
        let root = Package::from_texts(&[&format!(
            "PackageIdentifier: A\nPackageVersion: 1\nPackageLocale: en-US\nPublisher: P\nPackageName: N\nLicense: MIT\nShortDescription: d\nManifestType: singleton\nManifestVersion: 1.10.0\nInstallers:\n- Architecture: x86\n  InstallerType: exe\n  InstallerUrl: u\n  InstallerSha256: {SHA}\n  Dependencies:\n    PackageDependencies:\n    - PackageIdentifier: B\n"
        )])
        .unwrap();
        let m = Machine::gala(Architecture::X86, "en-US");
        let e = plan(&root, &m, &Request::default(), &s).unwrap_err().to_string();
        assert_eq!(e, "B 1: no applicable installer: installer 1: x64 does not run on x86");
    }

    #[test]
    fn deep_chain_refused() {
        let mut pkgs = Vec::new();
        let names: Vec<String> = (0..40).map(|i| format!("P{i}")).collect();
        let deps: Vec<String> = (0..40)
            .map(|i| format!("PackageDependencies:\n- PackageIdentifier: P{}", i + 1))
            .collect();
        for i in 0..40 {
            pkgs.push((names[i].as_str(), "1", deps[i].as_str()));
        }
        let s = src(&pkgs, &[]);
        let root = pkg("P0", "1", &deps[0]);
        assert!(run(&root, &s).unwrap_err().starts_with("dependencies nested too deep: P0 -> P1"));
    }
}
