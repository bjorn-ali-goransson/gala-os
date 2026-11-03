//! Install plans of wixl-built packages (testdata/msi: plan.wxs,
//! dual.wxs, basic.wxs, finished with msibuild by make_fixtures.sh).
//! There is no msiexec on the host: every expected value below is the
//! documented rule, and each test names the SDK page it follows
//! (learn.microsoft.com/windows/win32/msi/<page>). The packages' tables
//! themselves are checked against msiinfo's export first.

use super::*;
use std::string::ToString;
use std::vec::Vec as StdVec;
use std::{format, vec};

fn fixture(name: &str) -> StdVec<u8> {
    std::fs::read(format!("{}/testdata/msi/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn props(kv: &[(&str, &str)]) -> Properties {
    kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn machine(name: &str, privileged: bool) -> Machine {
    let mut m = Machine::windows_defaults("ann", &format!("D:\\pkgs\\{name}"));
    m.privileged = privileged;
    m
}

fn try_plan(name: &str, cmd: &[(&str, &str)], privileged: bool) -> Result<Plan, PlanError> {
    let data = fixture(name);
    let pkg = Package::open(&data).unwrap();
    Plan::compute(&pkg, &machine(name, privileged), &props(cmd))
}

fn plan_of(name: &str, cmd: &[(&str, &str)], privileged: bool) -> Plan {
    try_plan(name, cmd, privileged).unwrap_or_else(|e| panic!("{name} {cmd:?}: {e}"))
}

fn feature(p: &Plan, id: &str) -> State {
    p.features.iter().find(|f| f.id == id).unwrap_or_else(|| panic!("feature {id}")).state
}

fn component(p: &Plan, id: &str) -> State {
    p.components.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("component {id}")).state
}

fn dir(p: &Plan, id: &str) -> String {
    p.directories.iter().find(|d| d.id == id).unwrap_or_else(|| panic!("dir {id}")).target.clone()
}

fn lines<T: fmt::Display>(items: &[T]) -> StdVec<String> {
    items.iter().map(|x| x.to_string()).collect()
}

fn runs(p: &Plan, table: &str, action: &str) -> bool {
    p.actions
        .iter()
        .find(|a| a.table == table && a.action == action)
        .unwrap_or_else(|| panic!("{table} {action}"))
        .runs
}

/// The rows of an `msiinfo export` file, sorted.
fn idt_rows(name: &str) -> StdVec<String> {
    let path = format!("{}/testdata/msi/expected/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(path).unwrap();
    let mut rows: StdVec<String> =
        text.split("\r\n").skip(3).filter(|l| !l.is_empty()).map(String::from).collect();
    rows.sort();
    rows
}

#[test]
fn plan_packages_read_as_msiinfo_exports_them() {
    // msibuild rewrote these packages' string pools and tables: the
    // reader still agrees with msiinfo.
    for (msi, tables) in [
        (
            "plan.msi",
            &[
                "Property",
                "Directory",
                "Component",
                "Feature",
                "FeatureComponents",
                "Condition",
                "Registry",
                "Shortcut",
                "CustomAction",
                "InstallExecuteSequence",
                "InstallUISequence",
                "LaunchCondition",
                "File",
            ][..],
        ),
        ("dual.msi", &["Property", "Directory", "Component", "Registry", "Shortcut"][..]),
    ] {
        let data = fixture(msi);
        let pkg = Package::open(&data).unwrap();
        for t in tables {
            let table = pkg.table(t).unwrap();
            let mut got: StdVec<String> = table
                .rows
                .iter()
                .map(|r| r.iter().map(|v| v.to_string()).collect::<StdVec<_>>().join("\t"))
                .collect();
            got.sort();
            let stem = msi.trim_end_matches(".msi");
            assert_eq!(got, idt_rows(&format!("{stem}.{t}.idt")), "{msi} table {t}");
        }
    }
}

/// ALLUSERS ("An ALLUSERS property value of 1 specifies the
/// per-machine installation context"), Installation Context (the
/// per-machine folders: ProgramFiles64Folder, the all-users Program
/// Menu and Desktop), Feature table (Level 0 disables; a level above
/// INSTALLLEVEL is not installed), Registry table (value prefixes,
/// [~] lists, Root), Formatted, Shortcut table, sequence tables.
#[test]
fn per_machine_defaults() {
    let p = plan_of("plan.msi", &[], true);
    assert_eq!(p.scope, Scope::PerMachine);
    assert_eq!(p.properties["ALLUSERS"], "1");
    assert!(p.notes.iter().all(|n| !n.contains("elevation")));
    assert!(p.can_install());
    assert_eq!(dir(&p, "INSTALLDIR"), "C:\\Program Files\\Gala Plan\\");
    assert_eq!(dir(&p, "DOCS"), "C:\\Program Files\\Gala Plan\\docs\\");
    assert_eq!(
        dir(&p, "MenuDir"),
        "C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\Gala Plan\\"
    );
    assert_eq!(dir(&p, "DesktopFolder"), "C:\\Users\\Public\\Desktop\\");
    // The directory's property is its path once costed (Directory
    // table: a directory's key is a property).
    assert_eq!(p.properties["INSTALLDIR"], "C:\\Program Files\\Gala Plan\\");
    assert_eq!(
        lines(&p.features),
        [
            "feature Main level 1 -> local: level 1 <= INSTALLLEVEL 1",
            "feature Documentation level 3 -> null: level 3 is above INSTALLLEVEL 1",
            "feature Extras level 200 -> null: level 200 is above INSTALLLEVEL 1",
            "feature Never level 0 -> null: level 0: disabled",
        ]
    );
    for (c, s) in [
        ("Core", State::Local),
        ("Shortcuts", State::Local),
        ("Docs", State::Null),
        ("Extra", State::Null),
        ("NeverComp", State::Null),
    ] {
        assert_eq!(component(&p, c), s, "component {c}");
    }
    let core = p.components.iter().find(|c| c.id == "Core").unwrap();
    assert_eq!(core.key_path, KeyPath::File("core.txt".into()));
    assert!(core.win64);
    let sc = p.components.iter().find(|c| c.id == "Shortcuts").unwrap();
    assert!(matches!(&sc.key_path, KeyPath::Registry(_)), "{:?}", sc.key_path);
    assert_eq!(
        lines(&p.files),
        [
            "file core.txt -> C:\\Program Files\\Gala Plan\\core.txt (6 bytes, from embedded \
             cabinet plan.cab)",
            "file tool.exe -> C:\\Program Files\\Gala Plan\\tool.exe (23 bytes, from embedded \
             cabinet plan.cab)",
        ]
    );
    assert_eq!(
        lines(&p.registry),
        [
            r#"registry HKLM\Software\Gala\Plan "InstallDir" = REG_SZ "C:\\Program Files\\Gala Plan\\" [64-bit view] (component Core)"#,
            r#"registry HKLM\Software\Gala\Plan\Settings "Tool" = REG_SZ "C:\\Program Files\\Gala Plan\\tool.exe" [64-bit view] (component Core)"#,
            r#"registry HKLM\Software\Gala\Plan "Count" = REG_DWORD 42 (0x2a) [64-bit view] (component Core)"#,
            r#"registry HKLM\Software\Gala\Plan "Blob" = REG_BINARY 0a0b0c [64-bit view] (component Core)"#,
            r#"registry HKLM\Software\Gala\Plan "Path" = REG_EXPAND_SZ "%ProgramFiles%\\Gala Plan" [64-bit view] (component Core)"#,
            r#"registry HKLM\Software\Gala\Plan "List" = REG_MULTI_SZ ["one", "hello"] [64-bit view] (component Core)"#,
            // Root -1 per-machine is HKLM; [$Core] the component's
            // directory, [!core.txt] its short path, [%TEMP], a {group}
            // with an unset property blank, {plain} kept, [\[] escapes.
            r#"registry HKLM\Software\Gala\Plan "Where" = REG_SZ "C:\\Program Files\\Gala Plan\\|C:\\Program Files\\Gala Plan\\core.txt|C:\\Users\\ann\\AppData\\Local\\Temp||{plain}|[x]" [64-bit view] (component Core)"#,
            r#"registry HKCU\Software\Gala\Plan "Shortcuts" = REG_DWORD 1 (0x1) [64-bit view] (component Shortcuts)"#,
        ]
    );
    assert_eq!(
        lines(&p.shortcuts),
        [
            r#"shortcut ToolMenu C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Gala Plan\Gala Tool.lnk -> C:\Program Files\Gala Plan\tool.exe args "--greet hello" in C:\Program Files\Gala Plan\"#,
            r#"shortcut ToolDesk C:\Users\Public\Desktop\Gala Tool.lnk -> C:\Program Files\Gala Plan\tool.exe"#,
        ]
    );
    assert_eq!(p.shortcuts[0].description.as_deref(), Some("Runs the tool"));
    // The execute sequence in Sequence order, conditions evaluated
    // with the final states.
    let exec: StdVec<(&str, i32, bool)> = p
        .actions
        .iter()
        .filter(|a| a.table == "InstallExecuteSequence")
        .map(|a| (a.action.as_str(), a.sequence, a.runs))
        .collect();
    assert_eq!(
        exec,
        [
            ("ValidateProductID", 700, true),
            ("CostInitialize", 800, true),
            ("FileCost", 900, true),
            ("CostFinalize", 1000, true),
            ("SetGreeting", 1001, true),
            ("Rollback", 1002, false),
            ("InstallValidate", 1400, true),
            ("InstallInitialize", 1500, true),
            ("ProcessComponents", 1600, true),
            ("UnpublishFeatures", 1800, true),
            ("RemoveRegistryValues", 2600, true),
            ("RemoveShortcuts", 3200, true),
            ("RemoveFiles", 3500, true),
            ("InstallFiles", 4000, true),
            ("Deferred", 4001, true),
            ("CreateShortcuts", 4500, true),
            ("WriteRegistryValues", 5000, true),
            ("RegisterUser", 6000, true),
            ("RegisterProduct", 6100, true),
            ("PublishFeatures", 6300, true),
            ("PublishProduct", 6400, true),
            ("InstallFinalize", 6600, true),
            ("RunTool", 6601, true),
        ]
    );
    let line = |name: &str| {
        p.actions
            .iter()
            .find(|a| a.table == "InstallExecuteSequence" && a.action == name)
            .unwrap()
            .to_string()
    };
    assert_eq!(
        line("SetGreeting"),
        "action InstallExecuteSequence 1001 SetGreeting (custom type 51: sets property SHOUT to \
         \"hello!\"; immediate) if NOT Installed: runs"
    );
    assert_eq!(
        line("Deferred"),
        "action InstallExecuteSequence 4001 Deferred (custom type 3073: DLL in Binary CaDll, \
         entry Go; deferred, in the system context (no impersonation)) if $Core=3 AND NOT \
         REMOVE: runs"
    );
    assert!(line("RunTool").contains("asynchronous, not waited for"), "{}", line("RunTool"));
    assert_eq!(
        line("InstallFiles"),
        "action InstallExecuteSequence 4000 InstallFiles (standard: copies files: 2): runs"
    );
    // UILevel is 2 (no UI): the UI sequence's SetGreeting row is false.
    assert!(!runs(&p, "InstallUISequence", "SetGreeting"));
    // Every plan line is one item with its kind first.
    for l in p.to_string().lines() {
        let kind = l.split(' ').next().unwrap();
        assert!(
            [
                "context",
                "launch",
                "directory",
                "feature",
                "component",
                "file",
                "registry",
                "shortcut",
                "action",
                "note:"
            ]
            .contains(&kind),
            "{l}"
        );
    }
}

/// Installation Context / Single Package Authoring: a per-machine
/// install needs a privileged user (UAC).
#[test]
fn per_machine_without_privileges_says_so() {
    let p = plan_of("plan.msi", &[], false);
    assert!(p.notes.iter().any(|n| n.contains("needs elevation")), "{:?}", p.notes);
}

/// INSTALLLEVEL and the Feature table's Attributes
/// (msidbFeatureAttributesFavorSource) with the Component table's
/// Attributes (msidbComponentAttributesOptional: may run from source).
#[test]
fn install_level_and_run_from_source() {
    let p = plan_of("plan.msi", &[("INSTALLLEVEL", "3")], true);
    assert_eq!(feature(&p, "Documentation"), State::Source);
    assert_eq!(feature(&p, "Extras"), State::Null);
    assert_eq!(component(&p, "Docs"), State::Source);
    let f = p.files.iter().find(|f| f.key == "readme.txt").unwrap();
    assert_eq!(f.state, State::Source);
    assert_eq!(f.target, "D:\\pkgs\\Gala Plan\\docs\\readme.txt");
    assert!(f.to_string().starts_with("file readme.txt run from source D:\\pkgs\\"), "{f}");
    // InstallFiles copies only local files.
    assert!(
        p.actions.iter().any(|a| a.action == "InstallFiles"
            && a.kind == ActionKind::Standard("copies files: 2".into()))
    );
    // INSTALLLEVEL that is not a positive integer.
    let e = try_plan("plan.msi", &[("INSTALLLEVEL", "high")], true).unwrap_err();
    assert!(e.to_string().contains("property INSTALLLEVEL"), "{e}");
}

/// Condition table ("If the expression evaluates True, the Level column
/// of the Feature table is set to the value in the Level column"),
/// Component table's Condition ("If the condition evaluates to False,
/// then the component is disabled"), &feature in a sequence condition.
#[test]
fn condition_table_and_component_condition() {
    let p = plan_of("plan.msi", &[("WANTEXTRAS", "1")], true);
    let extras = p.features.iter().find(|f| f.id == "Extras").unwrap();
    assert_eq!((extras.authored_level, extras.level, extras.state), (200, 1, State::Local));
    assert!(extras.why.contains("Condition table: WANTEXTRAS -> level 1"), "{}", extras.why);
    assert_eq!(component(&p, "Extra"), State::Local);
    assert!(runs(&p, "InstallExecuteSequence", "Rollback"), "&Extras=3");
    let p = plan_of("plan.msi", &[("WANTEXTRAS", "1"), ("NOEXTRA", "1")], true);
    assert_eq!(feature(&p, "Extras"), State::Local);
    let extra = p.components.iter().find(|c| c.id == "Extra").unwrap();
    assert_eq!(extra.state, State::Null);
    assert_eq!(extra.why, "condition NOT NOEXTRA is false: disabled");
    assert!(p.files.iter().all(|f| f.key != "extra.txt"));
}

/// ADDLOCAL (a comma-separated list of features installed locally, or
/// ALL), REMOVE, ADDSOURCE, ADDDEFAULT, ADVERTISE, evaluated in that
/// order (the SDK's order for the feature state properties); a feature
/// installed makes its parent installed, and a level-0 feature stays
/// disabled.
#[test]
fn feature_request_properties() {
    let p = plan_of("plan.msi", &[("ADDLOCAL", "Extras")], true);
    assert_eq!(feature(&p, "Extras"), State::Local);
    assert_eq!(feature(&p, "Main"), State::Local, "the parent of Extras");
    assert_eq!(feature(&p, "Documentation"), State::Null, "not requested");
    let p = plan_of("plan.msi", &[("ADDLOCAL", "ALL"), ("REMOVE", "Documentation")], true);
    assert_eq!(feature(&p, "Main"), State::Local);
    assert_eq!(feature(&p, "Extras"), State::Local);
    assert_eq!(feature(&p, "Documentation"), State::Absent);
    assert_eq!(feature(&p, "Never"), State::Null, "level 0 stays disabled");
    assert_eq!(component(&p, "NeverComp"), State::Null);
    assert!(!runs(&p, "InstallExecuteSequence", "Deferred"), "NOT REMOVE is false");
    // ADDSOURCE on a feature whose component is local-only (Attributes
    // 0: "Component cannot be run from source"): local.
    let p = plan_of(
        "plan.msi",
        &[("ADDSOURCE", "Main"), ("ADDDEFAULT", "Documentation"), ("ADVERTISE", "Extras")],
        true,
    );
    assert_eq!(feature(&p, "Main"), State::Source);
    assert_eq!(component(&p, "Core"), State::Local);
    assert_eq!(feature(&p, "Documentation"), State::Source, "favorSource default");
    assert_eq!(feature(&p, "Extras"), State::Advertised);
    assert_eq!(component(&p, "Extra"), State::Null, "advertised: installed on first use");
    // REMOVE=ALL alone: nothing is installed.
    let p = plan_of("plan.msi", &[("REMOVE", "ALL")], true);
    assert!(p.components.iter().all(|c| c.state == State::Null), "{p}");
    // A feature that does not exist.
    let e = try_plan("plan.msi", &[("ADDLOCAL", "Main,NoSuch")], true).unwrap_err();
    assert_eq!(e.to_string(), "MSI plan: property ADDLOCAL: NoSuch is not in the Feature table");
}

/// Single Package Authoring ("Use ALLUSERS value of 2 and a
/// MSIINSTALLPERUSER value of 1 ... per-user installation"; per-user
/// ProgramFilesFolder is %LocalAppData%\Programs; Root -1 goes to
/// HKCU, Root 0 to HKCU\Software\Classes), MSIINSTALLPERUSER="" for
/// per-machine (Program Files (x86) for ProgramFilesFolder on 64-bit
/// Windows, HKLM, HKLM\Software\Classes), ALLUSERS ("If the value of
/// ALLUSERS does not equal 2, the Windows Installer ignores
/// MSIINSTALLPERUSER").
#[test]
fn dual_purpose_package() {
    let p = plan_of("dual.msi", &[], false);
    assert_eq!(p.scope, Scope::PerUser);
    assert_eq!(p.properties["ALLUSERS"], "");
    assert_eq!(dir(&p, "APPDIR"), "C:\\Users\\ann\\AppData\\Local\\Programs\\Gala Dual\\");
    assert_eq!(
        lines(&p.registry),
        [
            r#"registry HKCU\Software\Gala\Dual "Dir" = REG_SZ "C:\\Users\\ann\\AppData\\Local\\Programs\\Gala Dual\\" (component App)"#,
            r#"registry HKCU\Software\Classes\.galadual (default) = REG_SZ "GalaDual.File" (component App)"#,
        ]
    );
    assert_eq!(
        p.shortcuts[0].path,
        "C:\\Users\\ann\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Gala Dual.lnk"
    );
    assert!(p.notes.iter().all(|n| !n.contains("elevation")));
    for cmd in [&[("MSIINSTALLPERUSER", "")][..], &[("ALLUSERS", "1")][..]] {
        let p = plan_of("dual.msi", cmd, true);
        assert_eq!(p.scope, Scope::PerMachine, "{cmd:?}");
        assert_eq!(p.properties["ALLUSERS"], "1");
        assert_eq!(dir(&p, "APPDIR"), "C:\\Program Files (x86)\\Gala Dual\\");
        assert_eq!(
            lines(&p.registry),
            [
                r#"registry HKLM\Software\Gala\Dual "Dir" = REG_SZ "C:\\Program Files (x86)\\Gala Dual\\" (component App)"#,
                r#"registry HKLM\Software\Classes\.galadual (default) = REG_SZ "GalaDual.File" (component App)"#,
            ]
        );
        assert_eq!(
            p.shortcuts[0].path,
            "C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\Gala Dual.lnk"
        );
    }
    // Exit actions after the others, Sequence 0 never runs (sequence
    // table pages: -3 runs on a fatal error; 0 or NULL is not run).
    let p = plan_of("dual.msi", &[], false);
    let ui = p_actions(&p, "InstallUISequence");
    assert_eq!(&ui[ui.len() - 2..], [("FatalErrorDlg", -3, false), ("NeverRuns", 0, false)]);
}

fn p_actions<'p>(p: &'p Plan, table: &str) -> StdVec<(&'p str, i32, bool)> {
    p.actions
        .iter()
        .filter(|a| a.table == table)
        .map(|a| (a.action.as_str(), a.sequence, a.runs))
        .collect()
}

/// ALLUSERS ("If the ALLUSERS property value is not set, the system
/// uses the per-user installation context") with a plain per-user
/// package: LocalAppDataFolder; the files agree with install_files
/// (which is checked against msiextract).
#[test]
fn plain_per_user_package() {
    let p = plan_of("basic.msi", &[], false);
    assert_eq!(p.scope, Scope::PerUser);
    let data = fixture("basic.msi");
    let pkg = Package::open(&data).unwrap();
    let local = "C:\\Users\\ann\\AppData\\Local\\";
    let want =
        pkg.install_files(&|d| (d == "LocalAppDataFolder").then(|| local.to_string())).unwrap();
    let got: StdVec<(&str, &str)> =
        p.files.iter().map(|f| (f.key.as_str(), f.target.as_str())).collect();
    let want: StdVec<(&str, &str)> =
        want.iter().map(|f| (f.file.file.as_str(), f.target.as_str())).collect();
    assert_eq!(got, want);
    assert!(p.registry.iter().any(|r| r.hive == Hive::CurrentUser
        && r.name.as_deref() == Some("Café")
        && r.value == RegValue::Sz("crème brûlée".into())));
    // Its custom action: type 65 = DLL in the Binary table that
    // ignores its exit code ("Custom Action Return Processing
    // Options": msidbCustomActionTypeContinue 0x40).
    let ca = p.actions.iter().find(|a| a.action == "RunIt").unwrap();
    assert_eq!(
        ca.kind,
        ActionKind::Custom {
            ty: 65,
            what: "DLL in Binary Script, entry Main; ignores its exit code, immediate".into()
        }
    );
}

/// Directory table: a directory whose key is a property set on the
/// command line takes that path, and those below it follow.
#[test]
fn command_line_directory() {
    let p = plan_of("plan.msi", &[("INSTALLDIR", "E:\\Apps\\Plan")], true);
    assert_eq!(dir(&p, "INSTALLDIR"), "E:\\Apps\\Plan\\");
    assert_eq!(dir(&p, "DOCS"), "E:\\Apps\\Plan\\docs\\");
    assert!(p.files.iter().any(|f| f.target == "E:\\Apps\\Plan\\core.txt"));
    assert!(p.registry.iter().any(|r| r.value == RegValue::Sz("E:\\Apps\\Plan\\".into())));
}

/// LaunchCondition table: the Description (formatted) is shown when the
/// condition is false, and the install stops.
#[test]
fn launch_conditions() {
    let p = plan_of("plan.msi", &[("BLOCKME", "yes")], true);
    assert!(!p.can_install());
    assert_eq!(
        lines(&p.launch),
        [
            "launch condition VersionNT >= 600 passes",
            "launch condition NOT BLOCKME FAILS: Gala Plan is blocked: BLOCKME=yes",
        ]
    );
}

/// Errors name the table, the row, its key and the column.
#[test]
fn errors_name_table_row_and_column() {
    let e = try_plan("badcond.msi", &[], false).unwrap_err();
    let s = e.to_string();
    assert!(
        s.starts_with("MSI plan: table InstallExecuteSequence row ")
            && s.contains("(Broken) column Condition: condition syntax error at character 5"),
        "{s}"
    );
}

fn dirrow(id: &str, parent: Option<&str>, default: &str) -> super::super::Directory {
    super::super::Directory {
        directory: id.into(),
        parent: parent.map(Into::into),
        default_dir: default.into(),
    }
}

/// Directory table: DefaultDir is `[targetshort|]targetlong[:[sourceshort|]sourcelong]`,
/// "." for "the parent itself"; a root without a value is ROOTDRIVE
/// (ROOTDRIVE, TARGETDIR pages); SHORTFILENAMES picks the short target
/// names; the summary's word count bit 0 the short source names.
#[test]
fn directory_names_and_roots() {
    let dirs = vec![
        dirrow("TARGETDIR", None, "SourceDir"),
        dirrow("A", Some("TARGETDIR"), "ALPHA~1|Alpha Dir:SRCA~1|Source A"),
        dirrow("B", Some("A"), "."),
        dirrow("C", Some("B"), "CEE|Cee Long"),
        dirrow("OTHER", None, "Other"),
        dirrow("D", Some("OTHER"), "d:."),
    ];
    let p = props(&[("ROOTDRIVE", "R:\\"), ("SourceDir", "S:\\pkg\\")]);
    let r = resolve(&dirs, &p, false, false).unwrap();
    let get = |r: &[PlanDirectory], id: &str| {
        let d = r.iter().find(|d| d.id == id).unwrap();
        (d.target.clone(), d.source.clone())
    };
    assert_eq!(get(&r, "TARGETDIR"), ("R:\\".into(), "S:\\pkg\\".into()));
    assert_eq!(get(&r, "A"), ("R:\\Alpha Dir\\".into(), "S:\\pkg\\Source A\\".into()));
    assert_eq!(get(&r, "B"), ("R:\\Alpha Dir\\".into(), "S:\\pkg\\Source A\\".into()));
    assert_eq!(
        get(&r, "C"),
        ("R:\\Alpha Dir\\Cee Long\\".into(), "S:\\pkg\\Source A\\Cee Long\\".into())
    );
    assert_eq!(get(&r, "OTHER"), ("R:\\".into(), "S:\\pkg\\".into()));
    assert_eq!(get(&r, "D"), ("R:\\d\\".into(), "S:\\pkg\\".into()));
    let r = resolve(&dirs, &p, true, true).unwrap();
    assert_eq!(get(&r, "C"), ("R:\\ALPHA~1\\CEE\\".into(), "S:\\pkg\\SRCA~1\\CEE\\".into()));
    // A property set for a directory wins over its parent.
    let p2 = props(&[("ROOTDRIVE", "R:\\"), ("B", "Q:\\b")]);
    let r = resolve(&dirs, &p2, false, false).unwrap();
    assert_eq!(get(&r, "C").0, "Q:\\b\\Cee Long\\");
    // Loops and missing parents name the row.
    let bad = vec![dirrow("X", Some("Y"), "x"), dirrow("Y", Some("X"), "y")];
    let e = resolve(&bad, &p, false, false).unwrap_err().to_string();
    assert!(e.contains("table Directory") && e.contains("its own ancestor"), "{e}");
    let bad = vec![dirrow("X", Some("NOPE"), "x")];
    let e = resolve(&bad, &p, false, false).unwrap_err().to_string();
    assert_eq!(
        e,
        "MSI plan: table Directory row 1 (X) column Directory_Parent: parent NOPE is not in the \
         Directory table"
    );
}

/// Registry table, Value column: "#x" REG_BINARY, "#%" REG_EXPAND_SZ,
/// "#" REG_DWORD, "##" a string starting with "#", [~] lists: "[~]"
/// first appends, last prepends, both or neither replaces.
#[test]
fn registry_value_prefixes() {
    let p = props(&[("A", "alpha"), ("N", "7")]);
    let env = Env {
        props: &p,
        environment: &BTreeMap::new(),
        features: BTreeMap::new(),
        components: BTreeMap::new(),
        files: BTreeMap::new(),
        component_dirs: BTreeMap::new(),
    };
    let v = |raw: &str| registry_value(raw, &env);
    let ms =
        |m, items: &[&str]| Ok(RegValue::MultiSz(m, items.iter().map(|s| s.to_string()).collect()));
    assert_eq!(v("plain [A]"), Ok(RegValue::Sz("plain alpha".into())));
    assert_eq!(v("##x not hex"), Ok(RegValue::Sz("#x not hex".into())));
    assert_eq!(v("#x01FF"), Ok(RegValue::Binary(vec![1, 0xff])));
    assert_eq!(v("#%[A]\\%PATH%"), Ok(RegValue::ExpandSz("alpha\\%PATH%".into())));
    assert_eq!(v("#[N]"), Ok(RegValue::Dword(7)));
    assert_eq!(v("#-1"), Ok(RegValue::Dword(0xffff_ffff)));
    assert_eq!(v("#4294967295"), Ok(RegValue::Dword(0xffff_ffff)));
    assert!(v("#seven").unwrap_err().contains("not an integer"));
    assert!(v("#x123").unwrap_err().contains("odd number"));
    assert!(v("#xZZ").unwrap_err().contains("not hex"));
    assert_eq!(v("a[~]b[~]c"), ms(MultiMode::Replace, &["a", "b", "c"]));
    assert_eq!(v("[~]a[~]b"), ms(MultiMode::Append, &["a", "b"]));
    assert_eq!(v("a[~]b[~]"), ms(MultiMode::Prepend, &["a", "b"]));
    assert_eq!(v("[~]a[~]"), ms(MultiMode::Replace, &["a"]));
}

/// Custom Action Types and the option pages: the documented decimal
/// values decode to what their pages say.
#[test]
fn custom_action_types() {
    let d = |ty| describe_custom_action(ty, Some("S"), Some("T"), None);
    for (ty, want) in [
        (1, "DLL in Binary S, entry T; immediate"),
        (2, "EXE in Binary S, command line \"T\"; immediate"),
        (19, "fails the install with message \"T\"; immediate"),
        (34, "EXE \"T\" in working directory S; immediate"),
        (35, "sets directory S to \"T\"; immediate"),
        (51, "sets property S to \"T\"; immediate"),
        (65, "DLL in Binary S, entry T; ignores its exit code, immediate"),
        (1025, "DLL in Binary S, entry T; deferred"),
        (1281, "DLL in Binary S, entry T; deferred, on rollback only"),
        (1537, "DLL in Binary S, entry T; deferred, on commit only"),
        (
            3072 + 2,
            "EXE in Binary S, command line \"T\"; deferred, in the system context (no impersonation)",
        ),
        (
            3328 + 1,
            "DLL in Binary S, entry T; deferred, on rollback only, in the system context (no impersonation)",
        ),
        (17408 + 1, "DLL in Binary S, entry T; deferred, terminal-server aware"),
        (256 + 51, "sets property S to \"T\"; immediate, once if in both sequences"),
        (512 + 51, "sets property S to \"T\"; immediate, once per process"),
        (768 + 51, "sets property S to \"T\"; immediate, only on the client after the UI sequence"),
        (
            128 + 18,
            "EXE installed as file S, command line \"T\"; asynchronous, waits at the end of the sequence, immediate",
        ),
        (8192 + 1025, "DLL in Binary S, entry T; deferred, target hidden from the log"),
        (4096 + 1030, "VBScript in Binary S, function \"T\"; deferred, 64-bit script"),
        (63, "type 63 (not a documented base type), source \"S\", target \"T\"; immediate"),
    ] {
        assert_eq!(d(ty), want, "type {ty}");
    }
    assert!(describe_custom_action(1, None, None, Some(0x8000)).ends_with("on patch uninstall"));
}

#[test]
fn damaged_plan_packages_never_panic() {
    for name in ["plan.msi", "dual.msi"] {
        let data = fixture(name);
        let m = machine(name, true);
        let mut x = 0x0bad_5eedu32;
        for i in 0..300 {
            let mut d = data.clone();
            for _ in 0..1 + i % 3 {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let at = x as usize % d.len();
                d[at] ^= 1 << (x % 8);
            }
            if let Ok(pkg) = Package::open(&d) {
                let _ = Plan::compute(&pkg, &m, &Properties::new());
            }
        }
    }
}
