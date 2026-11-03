//! An install plan: what installing a package would do, computed from
//! its tables without running anything (ROADMAP PK stage 5,
//! ARCHITECTURE "Installing software", "MSI install plans"). Written
//! from the Windows Installer SDK documentation on learn.microsoft.com
//! (windows/win32/msi/...): the Directory, Feature, Condition,
//! FeatureComponents, Component, File, Registry, Shortcut,
//! LaunchCondition, CustomAction and sequence table pages, ALLUSERS,
//! MSIINSTALLPERUSER, "Installation Context", "Single Package
//! Authoring", INSTALLLEVEL, ADDLOCAL/REMOVE/ADDSOURCE/ADDDEFAULT/
//! ADVERTISE, the system folder properties, "Custom Action Types" and
//! their option pages, the Standard Actions Reference. No msiexec runs
//! here (none exists on the host): the tests' expected plans are those
//! rules, each test naming its page.
//!
//! [`Plan::compute`] goes the way the installer's costing does:
//!
//! 1. Properties: the Property table, then the caller's (the command
//!    line's) on top.
//! 2. The installation context (ALLUSERS: unset or "" per-user, 1
//!    per-machine, 2 with MSIINSTALLPERUSER=1 per-user and otherwise
//!    per-machine; any other value is taken as per-machine), ALLUSERS
//!    reset to "1" or "" as the installer does.
//! 3. The installer's properties: system folders for that context
//!    (per-user ProgramFilesFolder and its siblings only for the
//!    dual-purpose ALLUSERS=2 + MSIINSTALLPERUSER=1 case, as Single
//!    Package Authoring describes; a plain per-user package keeps the
//!    machine's Program Files), VersionNT and others, ROOTDRIVE and
//!    TARGETDIR when unset, SourceDir.
//! 4. Directories: a directory whose key is a set property takes it;
//!    a root takes ROOTDRIVE; others are their parent plus the
//!    DefaultDir target name (`.` adds nothing; `target:source`,
//!    `short|long`, short names with SHORTFILENAMES), and their source
//!    path likewise from SourceDir (short source names when the
//!    summary's word count has bit 0). Each directory's property is
//!    then set to its path.
//! 5. Features: the Condition table sets levels; with no ADDLOCAL,
//!    REMOVE, ADDSOURCE, ADDDEFAULT or ADVERTISE, a feature of level
//!    1..=INSTALLLEVEL is installed (local; source with favorSource,
//!    advertised with favorAdvertise, its parent's with followParent),
//!    level 0 is disabled; with them, only what they name (in that
//!    order; ALL is every enabled feature; installing one installs its
//!    parents); a feature whose parent is not installed is not either.
//! 6. Components: Component.Condition false disables; else local when a
//!    feature holding it is local (source when SourceOnly), source when
//!    one is source (local when LocalOnly), not installed otherwise.
//! 7. Files, registry values (Key, Name, Value formatted; the #x, #%,
//!    #, ## prefixes and [~] lists; Root -1 by context, Root 0 under
//!    the context's Software\Classes), shortcuts, launch conditions and
//!    the sequences with their conditions evaluated (custom actions
//!    decoded, never run).
//!
//! Every list item prints as one line ([`fmt::Display`]) for the
//! install log; errors name the table, row and column.

use super::cond::{self, Condition};
use super::format::{self, Column, Resolver};
use super::{Error as MsiError, FileSource, Package, Table, Value};
use alloc::borrow::ToOwned;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// Properties by name.
pub type Properties = BTreeMap<String, String>;

/// Why a plan could not be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanError {
    /// Reading the package.
    Msi(MsiError),
    /// A row's value is not valid: the table, the row (1-based), its
    /// key, the column, what.
    Row { table: String, row: usize, key: String, column: String, what: String },
    /// A property's value is not valid.
    Property { name: String, what: String },
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::Msi(e) => write!(f, "{e}"),
            PlanError::Row { table, row, key, column, what } => {
                write!(f, "MSI plan: table {table} row {row} ({key}) column {column}: {what}")
            }
            PlanError::Property { name, what } => write!(f, "MSI plan: property {name}: {what}"),
        }
    }
}

impl From<MsiError> for PlanError {
    fn from(e: MsiError) -> PlanError {
        PlanError::Msi(e)
    }
}

fn row_err(table: &str, row: usize, key: &str, column: &str, what: impl Into<String>) -> PlanError {
    PlanError::Row {
        table: table.into(),
        row: row + 1,
        key: key.into(),
        column: column.into(),
        what: what.into(),
    }
}

/// The known folders of the machine and user the plan is for, each
/// ending with `\` (gala.exe fills them from SHGetKnownFolderPath; the
/// tests use [`Machine::windows_defaults`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownFolders {
    pub windows: String,
    /// System32 (System64Folder on x64).
    pub system: String,
    /// SysWOW64 (SystemFolder on x64).
    pub system_x86: String,
    pub fonts: String,
    pub program_files: String,
    pub program_files_x86: String,
    pub program_files_common: String,
    pub program_files_common_x86: String,
    pub program_data: String,
    /// FOLDERID_UserProgramFiles (%LOCALAPPDATA%\Programs).
    pub user_program_files: String,
    pub user_program_files_common: String,
    pub temp: String,
    pub windows_volume: String,
    // The user's.
    pub desktop: String,
    pub programs: String,
    pub start_menu: String,
    pub startup: String,
    pub templates: String,
    pub admin_tools: String,
    pub app_data: String,
    pub local_app_data: String,
    pub personal: String,
    pub favorites: String,
    pub pictures: String,
    pub send_to: String,
    pub recent: String,
    pub net_hood: String,
    pub print_hood: String,
    // All users'.
    pub public_desktop: String,
    pub common_programs: String,
    pub common_start_menu: String,
    pub common_startup: String,
    pub common_templates: String,
    pub common_admin_tools: String,
}

/// The machine a plan is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Machine {
    pub folders: KnownFolders,
    /// ROOTDRIVE's value when the package does not set it.
    pub root_drive: String,
    /// The package's directory (SourceDir), ending with `\`.
    pub source_dir: String,
    /// The package's path (OriginalDatabase).
    pub package_path: String,
    /// The user is an administrator or the install is elevated.
    pub privileged: bool,
    /// Environment variables (names compared case-insensitively).
    pub environment: BTreeMap<String, String>,
    pub user_name: String,
    pub computer_name: String,
    /// VersionNT (the SDK's table stops at Windows 8.1, 603; later
    /// systems report 603 to an unmanifested installer: TODO: check
    /// on Windows 11) and WindowsBuild.
    pub version_nt: u32,
    pub windows_build: u32,
}

fn slash(s: &str) -> String {
    let mut s = s.to_owned();
    if !s.ends_with('\\') {
        s.push('\\');
    }
    s
}

impl Machine {
    /// Windows' default known-folder locations ([KNOWNFOLDERID]'s
    /// defaults) on an x64 system with Windows on C:, for user
    /// `user`, running `package` (a full path).
    pub fn windows_defaults(user: &str, package: &str) -> Machine {
        let w = "C:\\Windows\\";
        let profile = format!("C:\\Users\\{user}\\");
        let roaming = format!("{profile}AppData\\Roaming\\");
        let local = format!("{profile}AppData\\Local\\");
        let menu = format!("{roaming}Microsoft\\Windows\\Start Menu\\");
        let common_menu = "C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\".to_owned();
        let folders = KnownFolders {
            windows: w.into(),
            system: format!("{w}System32\\"),
            system_x86: format!("{w}SysWOW64\\"),
            fonts: format!("{w}Fonts\\"),
            program_files: "C:\\Program Files\\".into(),
            program_files_x86: "C:\\Program Files (x86)\\".into(),
            program_files_common: "C:\\Program Files\\Common Files\\".into(),
            program_files_common_x86: "C:\\Program Files (x86)\\Common Files\\".into(),
            program_data: "C:\\ProgramData\\".into(),
            user_program_files: format!("{local}Programs\\"),
            user_program_files_common: format!("{local}Programs\\Common\\"),
            temp: format!("{local}Temp\\"),
            windows_volume: "C:\\".into(),
            desktop: format!("{profile}Desktop\\"),
            programs: format!("{menu}Programs\\"),
            start_menu: menu.clone(),
            startup: format!("{menu}Programs\\Startup\\"),
            templates: format!("{roaming}Microsoft\\Windows\\Templates\\"),
            admin_tools: format!("{menu}Programs\\Administrative Tools\\"),
            app_data: roaming.clone(),
            local_app_data: local.clone(),
            personal: format!("{profile}Documents\\"),
            favorites: format!("{profile}Favorites\\"),
            pictures: format!("{profile}Pictures\\"),
            send_to: format!("{roaming}Microsoft\\Windows\\SendTo\\"),
            recent: format!("{roaming}Microsoft\\Windows\\Recent\\"),
            net_hood: format!("{roaming}Microsoft\\Windows\\Network Shortcuts\\"),
            print_hood: format!("{roaming}Microsoft\\Windows\\Printer Shortcuts\\"),
            public_desktop: "C:\\Users\\Public\\Desktop\\".into(),
            common_programs: format!("{common_menu}Programs\\"),
            common_start_menu: common_menu.clone(),
            common_startup: format!("{common_menu}Programs\\Startup\\"),
            common_templates: "C:\\ProgramData\\Microsoft\\Windows\\Templates\\".into(),
            common_admin_tools: format!("{common_menu}Programs\\Administrative Tools\\"),
        };
        let mut environment = BTreeMap::new();
        for (k, v) in [
            ("TEMP", format!("{local}Temp")),
            ("TMP", format!("{local}Temp")),
            ("USERPROFILE", profile.trim_end_matches('\\').to_owned()),
            ("APPDATA", roaming.trim_end_matches('\\').to_owned()),
            ("LOCALAPPDATA", local.trim_end_matches('\\').to_owned()),
            ("ProgramFiles", "C:\\Program Files".to_owned()),
            ("ProgramFiles(x86)", "C:\\Program Files (x86)".to_owned()),
            ("SystemRoot", "C:\\Windows".to_owned()),
            ("windir", "C:\\Windows".to_owned()),
            ("USERNAME", user.to_owned()),
        ] {
            environment.insert(k.to_owned(), v);
        }
        let source_dir = match package.rfind('\\') {
            Some(i) => package[..=i].to_owned(),
            None => ".\\".to_owned(),
        };
        Machine {
            folders,
            root_drive: "C:\\".into(),
            source_dir,
            package_path: package.into(),
            privileged: false,
            environment,
            user_name: user.into(),
            computer_name: "GALA".into(),
            version_nt: 603,
            windows_build: 9600,
        }
    }
}

/// The installation context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    PerUser,
    PerMachine,
}

/// A feature's or component's action state (the values conditions see).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// No action (INSTALLSTATE_UNKNOWN): not installed.
    Null,
    Advertised,
    Absent,
    Local,
    Source,
}

impl State {
    /// The INSTALLSTATE value.
    pub fn code(self) -> i32 {
        match self {
            State::Null => cond::UNKNOWN,
            State::Advertised => cond::ADVERTISED,
            State::Absent => cond::ABSENT,
            State::Local => cond::LOCAL,
            State::Source => cond::SOURCE,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            State::Null => "null",
            State::Advertised => "advertise",
            State::Absent => "absent",
            State::Local => "local",
            State::Source => "source",
        }
    }
    /// Local or run from source.
    pub fn installed(self) -> bool {
        matches!(self, State::Local | State::Source)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanDirectory {
    pub id: String,
    /// The target path (None: unresolved, a root with no value).
    pub target: String,
    pub source: String,
    /// How the target was found: "property", "ROOTDRIVE", "parent".
    pub from: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanFeature {
    pub id: String,
    pub parent: Option<String>,
    pub authored_level: i32,
    /// After the Condition table.
    pub level: i32,
    pub attributes: i32,
    pub state: State,
    pub why: String,
}

/// What a component's key path is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyPath {
    /// The component's directory.
    Directory,
    File(String),
    Registry(String),
    OdbcDataSource(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanComponent {
    pub id: String,
    pub component_id: Option<String>,
    pub directory: String,
    /// The directory's target path.
    pub path: String,
    pub attributes: i32,
    pub win64: bool,
    pub key_path: KeyPath,
    pub features: Vec<String>,
    pub state: State,
    pub why: String,
}

impl PlanComponent {
    /// msidbComponentAttributesSourceOnly.
    pub const SOURCE_ONLY: i32 = 0x1;
    /// msidbComponentAttributesOptional.
    pub const OPTIONAL: i32 = 0x2;
    pub const REGISTRY_KEY_PATH: i32 = 0x4;
    pub const SHARED_DLL_REF_COUNT: i32 = 0x8;
    pub const PERMANENT: i32 = 0x10;
    pub const ODBC_DATA_SOURCE: i32 = 0x20;
    pub const TRANSITIVE: i32 = 0x40;
    pub const NEVER_OVERWRITE: i32 = 0x80;
    pub const WIN64: i32 = 0x100;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanFile {
    pub key: String,
    pub component: String,
    /// The path it is installed to (local), or read from (source).
    pub target: String,
    pub size: i32,
    pub version: Option<String>,
    /// Where the bytes are.
    pub source: FileSource,
    /// The file's path in the source tree (SourceDir), for an
    /// uncompressed file or a component run from source.
    pub source_path: String,
    pub state: State,
}

/// A registry hive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hive {
    ClassesRoot,
    CurrentUser,
    LocalMachine,
    Users,
}

impl Hive {
    pub fn name(self) -> &'static str {
        match self {
            Hive::ClassesRoot => "HKCR",
            Hive::CurrentUser => "HKCU",
            Hive::LocalMachine => "HKLM",
            Hive::Users => "HKU",
        }
    }
}

/// How a REG_MULTI_SZ list combines with what is there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultiMode {
    Replace,
    /// `[~]a[~]b`: after the existing strings.
    Append,
    /// `a[~]b[~]`: before them.
    Prepend,
}

/// A registry write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegValue {
    Sz(String),
    ExpandSz(String),
    Dword(u32),
    Binary(Vec<u8>),
    MultiSz(MultiMode, Vec<String>),
    /// Name `+`: the key is created.
    CreateKey,
    /// Name `-`: the key is deleted on uninstall.
    DeleteKeyOnUninstall,
    /// Name `*`: created, and deleted on uninstall.
    CreateKeyDeleteOnUninstall,
    /// No value: the key alone.
    KeyOnly,
}

impl fmt::Display for RegValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegValue::Sz(s) => write!(f, "REG_SZ {s:?}"),
            RegValue::ExpandSz(s) => write!(f, "REG_EXPAND_SZ {s:?}"),
            RegValue::Dword(d) => write!(f, "REG_DWORD {d} ({d:#x})"),
            RegValue::Binary(b) => {
                f.write_str("REG_BINARY ")?;
                for x in b {
                    write!(f, "{x:02x}")?;
                }
                Ok(())
            }
            RegValue::MultiSz(m, items) => {
                let m = match m {
                    MultiMode::Replace => "",
                    MultiMode::Append => " (appended)",
                    MultiMode::Prepend => " (prepended)",
                };
                write!(f, "REG_MULTI_SZ {items:?}{m}")
            }
            RegValue::CreateKey => f.write_str("(key created)"),
            RegValue::DeleteKeyOnUninstall => f.write_str("(key deleted on uninstall)"),
            RegValue::CreateKeyDeleteOnUninstall => {
                f.write_str("(key created, deleted on uninstall)")
            }
            RegValue::KeyOnly => f.write_str("(key only)"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanRegistry {
    pub id: String,
    pub component: String,
    pub hive: Hive,
    /// The key (formatted; Root 0 is under Software\Classes of the
    /// context's hive).
    pub key: String,
    /// The value's name (None: the default value).
    pub name: Option<String>,
    pub value: RegValue,
    /// The component is 64-bit: the registry's 64-bit view.
    pub win64: bool,
}

/// What a shortcut points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShortcutTarget {
    /// An advertised shortcut: the feature.
    Feature(String),
    /// A formatted path.
    Path(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanShortcut {
    pub id: String,
    pub component: String,
    /// The .lnk file's path.
    pub path: String,
    pub target: ShortcutTarget,
    pub arguments: Option<String>,
    pub working_dir: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub icon_index: Option<i32>,
    pub show_cmd: Option<i32>,
}

/// A LaunchCondition row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchCheck {
    pub condition: String,
    pub message: String,
    pub passes: bool,
}

/// What a sequence row is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionKind {
    /// A standard action and what it would do here.
    Standard(String),
    /// A custom action: its type bits decoded.
    Custom { ty: i32, what: String },
    /// A dialog (UI sequences).
    Dialog,
    /// Not a standard action, a custom action or a dialog.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanAction {
    pub table: String,
    pub action: String,
    pub sequence: i32,
    pub condition: Option<String>,
    /// The condition holds (with the plan's final states).
    pub runs: bool,
    pub kind: ActionKind,
}

/// What installing the package would do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub scope: Scope,
    /// Why that context.
    pub scope_why: String,
    /// Every property at the end of costing.
    pub properties: Properties,
    pub directories: Vec<PlanDirectory>,
    pub features: Vec<PlanFeature>,
    pub components: Vec<PlanComponent>,
    /// The files of installed components.
    pub files: Vec<PlanFile>,
    pub registry: Vec<PlanRegistry>,
    pub shortcuts: Vec<PlanShortcut>,
    pub launch: Vec<LaunchCheck>,
    /// InstallUISequence, InstallExecuteSequence, AdminUISequence,
    /// AdminExecuteSequence, AdvtExecuteSequence rows in order.
    pub actions: Vec<PlanAction>,
    /// What the plan leaves out or assumes.
    pub notes: Vec<String>,
}

// ---------------------------------------------------------------------
// Reading tables.

/// A table's rows by column name (absent table: no rows).
struct Rows {
    table: Option<Table>,
    name: String,
}

impl Rows {
    fn read(p: &Package, name: &str) -> Result<Rows, PlanError> {
        let table = if p.columns(name).is_some() { Some(p.table(name)?) } else { None };
        Ok(Rows { table, name: name.into() })
    }
    fn len(&self) -> usize {
        self.table.as_ref().map_or(0, |t| t.rows.len())
    }
    fn get(&self, row: usize, col: &str) -> Option<&Value> {
        let t = self.table.as_ref()?;
        t.column(col).map(|c| &t.rows[row][c])
    }
    fn s(&self, row: usize, col: &str) -> Option<String> {
        self.get(row, col).and_then(Value::as_str).map(str::to_owned)
    }
    fn i(&self, row: usize, col: &str) -> Option<i32> {
        self.get(row, col).and_then(Value::as_int)
    }
    fn need_s(&self, row: usize, col: &str) -> Result<String, PlanError> {
        self.s(row, col).ok_or_else(|| {
            row_err(&self.name, row, &self.key(row), col, "NULL where a string is required")
        })
    }
    /// The row's first column, for messages.
    fn key(&self, row: usize) -> String {
        self.table.as_ref().map_or_else(String::new, |t| t.rows[row][0].to_string())
    }
}

// ---------------------------------------------------------------------
// Names.

/// `short|long` → (short, long).
fn split_name(s: &str) -> (&str, &str) {
    match s.split_once('|') {
        Some((a, b)) => (a, b),
        None => (s, s),
    }
}

/// DefaultDir → (target short, target long, source short, source long).
fn default_dir(d: &str) -> (&str, &str, &str, &str) {
    let (t, s) = d.split_once(':').unwrap_or((d, d));
    let (ts, tl) = split_name(t);
    let (ss, sl) = split_name(s);
    (ts, tl, ss, sl)
}

// ---------------------------------------------------------------------
// The evaluation context.

struct Env<'a> {
    props: &'a Properties,
    environment: &'a BTreeMap<String, String>,
    features: BTreeMap<String, State>,
    components: BTreeMap<String, State>,
    /// File key → (full path, short path), for installed files.
    files: BTreeMap<String, (String, String)>,
    /// Component → its directory by state.
    component_dirs: BTreeMap<String, String>,
}

impl cond::Context for Env<'_> {
    fn property(&self, name: &str) -> Option<String> {
        self.props.get(name).cloned()
    }
    fn environment(&self, name: &str) -> Option<String> {
        env_lookup(self.environment, name)
    }
    fn component_action(&self, name: &str) -> Option<i32> {
        self.components.get(name).map(|s| s.code())
    }
    fn component_installed(&self, name: &str) -> Option<i32> {
        // A fresh install: nothing is installed yet.
        self.components.get(name).map(|_| cond::ABSENT)
    }
    fn feature_action(&self, name: &str) -> Option<i32> {
        self.features.get(name).map(|s| s.code())
    }
    fn feature_installed(&self, name: &str) -> Option<i32> {
        self.features.get(name).map(|_| cond::ABSENT)
    }
}

impl Resolver for Env<'_> {
    fn property(&self, name: &str) -> Option<String> {
        self.props.get(name).cloned()
    }
    fn environment(&self, name: &str) -> Option<String> {
        env_lookup(self.environment, name)
    }
    fn file_path(&self, key: &str) -> Option<String> {
        self.files.get(key).map(|f| f.0.clone())
    }
    fn file_short_path(&self, key: &str) -> Option<String> {
        self.files.get(key).map(|f| f.1.clone())
    }
    fn component_directory(&self, key: &str) -> Option<String> {
        self.component_dirs.get(key).cloned()
    }
}

fn env_lookup(env: &BTreeMap<String, String>, name: &str) -> Option<String> {
    env.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
}

fn parse_condition(
    table: &str,
    row: usize,
    key: &str,
    column: &str,
    text: &str,
) -> Result<Condition, PlanError> {
    Condition::parse(text).map_err(|e| row_err(table, row, key, column, e.to_string()))
}

// ---------------------------------------------------------------------
// Standard actions and custom action types.

/// The standard actions (the SDK's Standard Actions Reference) and what
/// each does, for the plan's lines.
const STANDARD: &[(&str, &str)] = &[
    ("ADMIN", "top-level administrative install"),
    ("ADVERTISE", "top-level advertisement"),
    ("INSTALL", "top-level install"),
    ("AllocateRegistrySpace", "checks registry space"),
    ("AppSearch", "searches for files, directories and registry values (not run by the plan)"),
    ("BindImage", "binds executables to their DLLs"),
    ("CCPSearch", "searches for qualifying products"),
    ("CostFinalize", "resolves directories, selects features and components"),
    ("CostInitialize", "starts costing"),
    ("CreateFolders", "creates the CreateFolder table's directories"),
    ("CreateShortcuts", "creates shortcuts"),
    ("DeleteServices", "deletes services"),
    ("DisableRollback", "disables rollback"),
    ("DuplicateFiles", "copies files (DuplicateFile table)"),
    ("ExecuteAction", "runs the execute sequence"),
    ("FileCost", "costs files"),
    ("FindRelatedProducts", "finds related products (Upgrade table)"),
    ("ForceReboot", "reboots"),
    ("InstallAdminPackage", "copies the package to the administrative image"),
    ("InstallExecute", "runs the script so far"),
    ("InstallExecuteAgain", "runs the script so far"),
    ("InstallFiles", "copies files"),
    ("InstallFinalize", "runs the script, commits"),
    ("InstallInitialize", "starts the script"),
    ("InstallODBC", "installs ODBC drivers and data sources"),
    ("InstallServices", "installs services"),
    ("InstallSFPCatalogFile", "installs SFP catalogs"),
    ("InstallValidate", "checks disk space and running files"),
    ("IsolateComponents", "isolates components"),
    ("LaunchConditions", "checks the launch conditions"),
    ("MigrateFeatureStates", "takes feature states from related products"),
    ("MoveFiles", "moves files (MoveFile table)"),
    ("MsiConfigureServices", "configures services"),
    ("MsiPublishAssemblies", "publishes assemblies"),
    ("MsiUnpublishAssemblies", "unpublishes assemblies"),
    ("PatchFiles", "applies file patches"),
    ("ProcessComponents", "registers components"),
    ("PublishComponents", "publishes qualified components"),
    ("PublishFeatures", "publishes features"),
    ("PublishProduct", "publishes the product"),
    ("RegisterClassInfo", "registers COM classes"),
    ("RegisterComPlus", "registers COM+ applications"),
    ("RegisterExtensionInfo", "registers file extensions"),
    ("RegisterFonts", "registers fonts"),
    ("RegisterMIMEInfo", "registers MIME types"),
    ("RegisterProduct", "registers the product (Add/Remove Programs)"),
    ("RegisterProgIdInfo", "registers ProgIDs"),
    ("RegisterTypeLibraries", "registers type libraries"),
    ("RegisterUser", "registers the user"),
    ("RemoveDuplicateFiles", "removes duplicated files"),
    ("RemoveEnvironmentStrings", "removes environment strings"),
    ("RemoveExistingProducts", "uninstalls related products"),
    ("RemoveFiles", "removes files"),
    ("RemoveFolders", "removes folders"),
    ("RemoveIniValues", "removes .ini values"),
    ("RemoveODBC", "removes ODBC entries"),
    ("RemoveRegistryValues", "removes registry values"),
    ("RemoveShortcuts", "removes shortcuts"),
    ("ResolveSource", "resolves the source"),
    ("RMCCPSearch", "searches for qualifying products"),
    ("ScheduleReboot", "asks for a reboot"),
    ("SelfRegModules", "self-registers modules"),
    ("SelfUnregModules", "self-unregisters modules"),
    ("SetODBCFolders", "sets ODBC driver directories"),
    ("StartServices", "starts services"),
    ("StopServices", "stops services"),
    ("UnpublishComponents", "unpublishes qualified components"),
    ("UnpublishFeatures", "unpublishes features"),
    ("UnregisterClassInfo", "unregisters COM classes"),
    ("UnregisterComPlus", "unregisters COM+ applications"),
    ("UnregisterExtensionInfo", "unregisters file extensions"),
    ("UnregisterFonts", "unregisters fonts"),
    ("UnregisterMIMEInfo", "unregisters MIME types"),
    ("UnregisterProgIdInfo", "unregisters ProgIDs"),
    ("UnregisterTypeLibraries", "unregisters type libraries"),
    ("ValidateProductID", "validates the product ID"),
    ("WriteEnvironmentStrings", "writes environment strings"),
    ("WriteIniValues", "writes .ini values"),
    ("WriteRegistryValues", "writes registry values"),
];

/// Decodes a CustomAction row's Type (the SDK's "Custom Action Types"
/// and its return, scheduling and in-script option pages) with its
/// Source and (formatted) Target.
pub fn describe_custom_action(
    ty: i32,
    source: Option<&str>,
    target: Option<&str>,
    extended: Option<i32>,
) -> String {
    let s = source.unwrap_or("");
    let t = target.unwrap_or("");
    let mut out = match ty & 0x3f {
        1 => format!("DLL in Binary {s}, entry {t}"),
        2 => format!("EXE in Binary {s}, command line {t:?}"),
        5 => format!("JScript in Binary {s}, function {t:?}"),
        6 => format!("VBScript in Binary {s}, function {t:?}"),
        7 => format!("nested install of substorage {s}, properties {t:?}"),
        17 => format!("DLL installed as file {s}, entry {t}"),
        18 => format!("EXE installed as file {s}, command line {t:?}"),
        19 => format!("fails the install with message {t:?}"),
        21 => format!("JScript installed as file {s}, function {t:?}"),
        22 => format!("VBScript installed as file {s}, function {t:?}"),
        23 => format!("nested install of package {s} in the source tree, properties {t:?}"),
        34 => format!("EXE {t:?} in working directory {s}"),
        35 => format!("sets directory {s} to {t:?}"),
        37 => "JScript text in Target".to_owned(),
        38 => "VBScript text in Target".to_owned(),
        39 => format!("nested install of advertised product {s}, properties {t:?}"),
        50 => format!("EXE named by property {s}, command line {t:?}"),
        51 => format!("sets property {s} to {t:?}"),
        53 => format!("JScript in property {s}, function {t:?}"),
        54 => format!("VBScript in property {s}, function {t:?}"),
        b => format!("type {b} (not a documented base type), source {s:?}, target {t:?}"),
    };
    let mut opts: Vec<&str> = Vec::new();
    match ty & 0xc0 {
        0x40 => opts.push("ignores its exit code"),
        0x80 => opts.push("asynchronous, waits at the end of the sequence"),
        0xc0 => opts.push("asynchronous, not waited for"),
        _ => {}
    }
    if ty & 0x400 != 0 {
        opts.push(match ty & 0x300 {
            0x100 => "deferred, on rollback only",
            0x200 => "deferred, on commit only",
            0x300 => "deferred, rollback and commit bits both set",
            _ => "deferred",
        });
        if ty & 0x800 != 0 {
            opts.push("in the system context (no impersonation)");
        }
        if ty & 0x4000 != 0 {
            opts.push("terminal-server aware");
        }
    } else {
        opts.push("immediate");
        match ty & 0x300 {
            0x100 => opts.push("once if in both sequences"),
            0x200 => opts.push("once per process"),
            0x300 => opts.push("only on the client after the UI sequence"),
            _ => {}
        }
    }
    if ty & 0x2000 != 0 {
        opts.push("target hidden from the log");
    }
    if ty & 0x1000 != 0 {
        opts.push("64-bit script");
    }
    if extended.unwrap_or(0) & 0x8000 != 0 {
        opts.push("on patch uninstall");
    }
    out.push_str("; ");
    out.push_str(&opts.join(", "));
    out
}

// ---------------------------------------------------------------------
// The plan.

/// Feature attribute bits.
const FAVOR_SOURCE: i32 = 1;
const FOLLOW_PARENT: i32 = 2;
const FAVOR_ADVERTISE: i32 = 4;

/// The sequence tables a plan lists, in the order an install meets
/// them.
pub const SEQUENCES: [&str; 5] = [
    "InstallUISequence",
    "InstallExecuteSequence",
    "AdminUISequence",
    "AdminExecuteSequence",
    "AdvtExecuteSequence",
];

fn context(props: &Properties) -> (Scope, String) {
    let all = props.get("ALLUSERS").map_or("", String::as_str);
    let per = props.get("MSIINSTALLPERUSER").map_or("", String::as_str);
    match all {
        "" => (Scope::PerUser, "ALLUSERS is not set: per-user".into()),
        "1" => (Scope::PerMachine, "ALLUSERS=1: per-machine".into()),
        "2" if per == "1" => {
            (Scope::PerUser, "ALLUSERS=2 and MSIINSTALLPERUSER=1: per-user (dual-purpose)".into())
        }
        "2" => (Scope::PerMachine, "ALLUSERS=2 without MSIINSTALLPERUSER=1: per-machine".into()),
        v => (Scope::PerMachine, format!("ALLUSERS={v} (not 1 or 2): taken as per-machine")),
    }
}

/// The properties the installer sets for `scope`.
fn system_properties(m: &Machine, scope: Scope, dual: bool) -> Vec<(&'static str, String)> {
    let f = &m.folders;
    let machine = scope == Scope::PerMachine;
    let user_pf = scope == Scope::PerUser && dual;
    let pick = |all: &str, user: &str| if machine { all.to_owned() } else { user.to_owned() };
    let mut v: Vec<(&'static str, String)> = Vec::new();
    v.push(("WindowsFolder", f.windows.clone()));
    v.push(("WindowsVolume", f.windows_volume.clone()));
    v.push(("SystemFolder", f.system_x86.clone()));
    v.push(("System64Folder", f.system.clone()));
    v.push(("System16Folder", format!("{}System\\", f.windows)));
    v.push(("FontsFolder", f.fonts.clone()));
    v.push(("TempFolder", f.temp.clone()));
    v.push(("CommonAppDataFolder", f.program_data.clone()));
    v.push(("AppDataFolder", f.app_data.clone()));
    v.push(("LocalAppDataFolder", f.local_app_data.clone()));
    v.push(("PersonalFolder", f.personal.clone()));
    v.push(("FavoritesFolder", f.favorites.clone()));
    v.push(("MyPicturesFolder", f.pictures.clone()));
    v.push(("SendToFolder", f.send_to.clone()));
    v.push(("RecentFolder", f.recent.clone()));
    v.push(("NetHoodFolder", f.net_hood.clone()));
    v.push(("PrintHoodFolder", f.print_hood.clone()));
    v.push(("DesktopFolder", pick(&f.public_desktop, &f.desktop)));
    v.push(("ProgramMenuFolder", pick(&f.common_programs, &f.programs)));
    v.push(("StartMenuFolder", pick(&f.common_start_menu, &f.start_menu)));
    v.push(("StartupFolder", pick(&f.common_startup, &f.startup)));
    v.push(("TemplateFolder", pick(&f.common_templates, &f.templates)));
    v.push(("AdminToolsFolder", pick(&f.common_admin_tools, &f.admin_tools)));
    if user_pf {
        v.push(("ProgramFilesFolder", f.user_program_files.clone()));
        v.push(("ProgramFiles64Folder", f.user_program_files.clone()));
        v.push(("CommonFilesFolder", f.user_program_files_common.clone()));
        v.push(("CommonFiles64Folder", f.user_program_files_common.clone()));
    } else {
        v.push(("ProgramFilesFolder", f.program_files_x86.clone()));
        v.push(("ProgramFiles64Folder", f.program_files.clone()));
        v.push(("CommonFilesFolder", f.program_files_common_x86.clone()));
        v.push(("CommonFiles64Folder", f.program_files_common.clone()));
    }
    let nt = m.version_nt.to_string();
    v.push(("VersionNT", nt.clone()));
    v.push(("VersionNT64", nt.clone()));
    v.push(("Msix64", nt));
    v.push(("WindowsBuild", m.windows_build.to_string()));
    v.push(("ServicePackLevel", "0".into()));
    v.push(("VersionMsi", "5.00".into()));
    v.push(("LogonUser", m.user_name.clone()));
    v.push(("ComputerName", m.computer_name.clone()));
    v.push(("UserLanguageID", "1033".into()));
    v.push(("SystemLanguageID", "1033".into()));
    v.push(("ProductState", "-1".into()));
    // gala installs without UI (msiexec /qn): INSTALLUILEVEL_NONE.
    v.push(("UILevel", "2".into()));
    v.push(("SourceDir", slash(&m.source_dir)));
    v.push(("SOURCEDIR", slash(&m.source_dir)));
    v.push(("OriginalDatabase", m.package_path.clone()));
    v.push(("DATABASE", m.package_path.clone()));
    if m.privileged {
        v.push(("Privileged", "1".into()));
        v.push(("AdminUser", "1".into()));
    }
    v
}

/// The feature-request properties, in the order the installer
/// evaluates them, with the state each asks for (None: the feature's
/// default).
const REQUESTS: [(&str, Option<State>); 5] = [
    ("ADDLOCAL", Some(State::Local)),
    ("REMOVE", Some(State::Absent)),
    ("ADDSOURCE", Some(State::Source)),
    ("ADDDEFAULT", None),
    ("ADVERTISE", Some(State::Advertised)),
];

impl Plan {
    /// The plan for installing `pkg` on `machine` with the command
    /// line's `properties`.
    pub fn compute(
        pkg: &Package,
        machine: &Machine,
        properties: &Properties,
    ) -> Result<Plan, PlanError> {
        let mut notes: Vec<String> = Vec::new();
        // 1. Properties.
        let mut props = Properties::new();
        let ptab = Rows::read(pkg, "Property")?;
        for r in 0..ptab.len() {
            let name = ptab.need_s(r, "Property")?;
            props.insert(name, ptab.s(r, "Value").unwrap_or_default());
        }
        for (k, v) in properties {
            props.insert(k.clone(), v.clone());
        }
        // 2. The context.
        let (scope, scope_why) = context(&props);
        let dual = props.get("ALLUSERS").is_some_and(|v| v == "2");
        let allusers = if scope == Scope::PerMachine { "1" } else { "" };
        props.insert("ALLUSERS".into(), allusers.into());
        if scope == Scope::PerMachine && !machine.privileged {
            notes.push(
                "per-machine install by a user who is not privileged: needs elevation".into(),
            );
        }
        // 3. The installer's properties (the command line still wins).
        for (k, v) in system_properties(machine, scope, dual) {
            if !properties.contains_key(k) {
                props.insert(k.into(), v);
            }
        }
        if !props.get("ROOTDRIVE").is_some_and(|v| !v.is_empty()) {
            props.insert("ROOTDRIVE".into(), slash(&machine.root_drive));
        }
        if !props.get("TARGETDIR").is_some_and(|v| !v.is_empty()) {
            let root = props["ROOTDRIVE"].clone();
            props.insert("TARGETDIR".into(), root);
        }
        if let Some(code) = pkg.summary().ok().and_then(|s| s.revision) {
            props.entry("PackageCode".into()).or_insert(code);
        }
        let install_level = match props.get("INSTALLLEVEL").map(String::as_str) {
            None | Some("") => 1,
            Some(v) => {
                cond::as_integer(v).filter(|&l| l > 0).ok_or_else(|| PlanError::Property {
                    name: "INSTALLLEVEL".into(),
                    what: format!("{v:?} is not a positive integer"),
                })?
            }
        };
        let word_count = pkg.summary().ok().and_then(|s| s.word_count).unwrap_or(0);
        let short_targets = props.get("SHORTFILENAMES").is_some_and(|v| !v.is_empty());
        let short_sources = word_count & 1 != 0;

        // 4. Directories.
        let dirs = pkg.directories()?;
        let directories = resolve(&dirs, &props, short_targets, short_sources)?;
        for d in &directories {
            props.insert(d.id.clone(), d.target.clone());
        }
        let dir_of = |id: &str| directories.iter().find(|d| d.id == id);
        let short_dirs = short_paths(&dirs, &directories);

        // 5. Features.
        let feats = pkg.features()?;
        let mut levels: BTreeMap<String, i32> =
            feats.iter().map(|f| (f.feature.clone(), f.level)).collect();
        let ctab = Rows::read(pkg, "Condition")?;
        let mut level_why: BTreeMap<String, String> = BTreeMap::new();
        {
            let env = Env {
                props: &props,
                environment: &machine.environment,
                features: BTreeMap::new(),
                components: BTreeMap::new(),
                files: BTreeMap::new(),
                component_dirs: BTreeMap::new(),
            };
            for r in 0..ctab.len() {
                let f = ctab.need_s(r, "Feature_")?;
                let text = ctab.s(r, "Condition").unwrap_or_default();
                let c = parse_condition("Condition", r, &f, "Condition", &text)?;
                let Some(level) = ctab.i(r, "Level") else {
                    return Err(row_err("Condition", r, &f, "Level", "NULL"));
                };
                if !levels.contains_key(&f) {
                    return Err(row_err(
                        "Condition",
                        r,
                        &f,
                        "Feature_",
                        "not in the Feature table",
                    ));
                }
                if c.evaluate(&env) {
                    levels.insert(f.clone(), level);
                    level_why.insert(f, format!("Condition table: {text} -> level {level}"));
                }
            }
        }
        let features = select_features(&feats, &levels, &level_why, install_level, &props)?;
        let feature_state: BTreeMap<String, State> =
            features.iter().map(|f| (f.id.clone(), f.state)).collect();

        // 6. Components.
        let fc = Rows::read(pkg, "FeatureComponents")?;
        let mut comp_features: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for r in 0..fc.len() {
            let f = fc.need_s(r, "Feature_")?;
            let c = fc.need_s(r, "Component_")?;
            if !feature_state.contains_key(&f) {
                return Err(row_err("FeatureComponents", r, &f, "Feature_", "not a feature"));
            }
            comp_features.entry(c).or_default().push(f);
        }
        let comps = pkg.components()?;
        let mut components = Vec::new();
        {
            let env = Env {
                props: &props,
                environment: &machine.environment,
                features: feature_state.clone(),
                components: BTreeMap::new(),
                files: BTreeMap::new(),
                component_dirs: BTreeMap::new(),
            };
            for (r, c) in comps.iter().enumerate() {
                let Some(dir) = dir_of(&c.directory) else {
                    return Err(row_err(
                        "Component",
                        r,
                        &c.component,
                        "Directory_",
                        format!("directory {} is not in the Directory table", c.directory),
                    ));
                };
                let fs = comp_features.get(&c.component).cloned().unwrap_or_default();
                let (state, why) = match c.condition.as_deref() {
                    Some(text)
                        if !parse_condition("Component", r, &c.component, "Condition", text)?
                            .evaluate(&env) =>
                    {
                        (State::Null, format!("condition {text} is false: disabled"))
                    }
                    _ => component_state(c.attributes, &fs, &feature_state),
                };
                let key_path = match c.key_path.clone() {
                    None => KeyPath::Directory,
                    Some(k) if c.attributes & PlanComponent::REGISTRY_KEY_PATH != 0 => {
                        KeyPath::Registry(k)
                    }
                    Some(k) if c.attributes & PlanComponent::ODBC_DATA_SOURCE != 0 => {
                        KeyPath::OdbcDataSource(k)
                    }
                    Some(k) => KeyPath::File(k),
                };
                components.push(PlanComponent {
                    id: c.component.clone(),
                    component_id: c.component_id.clone(),
                    directory: c.directory.clone(),
                    path: dir.target.clone(),
                    attributes: c.attributes,
                    win64: c.attributes & PlanComponent::WIN64 != 0,
                    key_path,
                    features: fs,
                    state,
                    why,
                });
            }
        }
        let comp_state: BTreeMap<String, State> =
            components.iter().map(|c| (c.id.clone(), c.state)).collect();

        // 7. Files.
        let media = pkg.media()?;
        let compressed = word_count & 2 != 0;
        let mut files = Vec::new();
        let mut file_paths: BTreeMap<String, (String, String)> = BTreeMap::new();
        for (r, f) in pkg.files()?.into_iter().enumerate() {
            let Some(c) = components.iter().find(|c| c.id == f.component) else {
                return Err(row_err(
                    "File",
                    r,
                    &f.file,
                    "Component_",
                    format!("component {} is not in the Component table", f.component),
                ));
            };
            if !c.state.installed() {
                continue;
            }
            let Some(d) = dir_of(&c.directory) else { continue };
            let (fs, fl) = split_name(&f.file_name);
            let tname = if short_targets { fs } else { fl };
            let sname = if short_sources { fs } else { fl };
            let source_path = format!("{}{sname}", d.source);
            let target = if c.state == State::Local {
                format!("{}{tname}", d.target)
            } else {
                source_path.clone()
            };
            let short = format!(
                "{}{fs}",
                short_dirs.get(&c.directory).cloned().unwrap_or_else(|| d.target.clone())
            );
            let source = pkg
                .file_source(&f, &media, compressed)
                .map_err(|e| row_err("File", r, &f.file, "Sequence", e.to_string()))?;
            file_paths.insert(f.file.clone(), (target.clone(), short));
            files.push(PlanFile {
                key: f.file.clone(),
                component: f.component.clone(),
                target,
                size: f.size,
                version: f.version.clone(),
                source,
                source_path,
                state: c.state,
            });
        }
        let component_dirs: BTreeMap<String, String> = components
            .iter()
            .filter_map(|c| {
                let d = dir_of(&c.directory)?;
                match c.state {
                    State::Local => Some((c.id.clone(), d.target.clone())),
                    State::Source => Some((c.id.clone(), d.source.clone())),
                    _ => None,
                }
            })
            .collect();
        let env = Env {
            props: &props,
            environment: &machine.environment,
            features: feature_state.clone(),
            components: comp_state.clone(),
            files: file_paths,
            component_dirs,
        };

        // Registry.
        let mut registry = Vec::new();
        for (r, row) in pkg.registry()?.into_iter().enumerate() {
            let Some(c) = components.iter().find(|c| c.id == row.component) else {
                return Err(row_err(
                    "Registry",
                    r,
                    &row.registry,
                    "Component_",
                    format!("component {} is not in the Component table", row.component),
                ));
            };
            if !c.state.installed() {
                continue;
            }
            let key = format::format(&row.key, &env, Column::Other);
            let (hive, key) = match (row.root, scope) {
                (-1, Scope::PerUser) => (Hive::CurrentUser, key),
                (-1, Scope::PerMachine) => (Hive::LocalMachine, key),
                (0, Scope::PerUser) => (Hive::CurrentUser, format!("Software\\Classes\\{key}")),
                (0, Scope::PerMachine) => (Hive::LocalMachine, format!("Software\\Classes\\{key}")),
                (1, _) => (Hive::CurrentUser, key),
                (2, _) => (Hive::LocalMachine, key),
                (3, _) => (Hive::Users, key),
                (n, _) => {
                    return Err(row_err(
                        "Registry",
                        r,
                        &row.registry,
                        "Root",
                        format!("{n} (valid: -1, 0 HKCR, 1 HKCU, 2 HKLM, 3 HKU)"),
                    ));
                }
            };
            let name = row.name.as_deref().map(|n| format::format(n, &env, Column::Other));
            let value = match row.value.as_deref() {
                None => match name.as_deref() {
                    Some("+") => RegValue::CreateKey,
                    Some("-") => RegValue::DeleteKeyOnUninstall,
                    Some("*") => RegValue::CreateKeyDeleteOnUninstall,
                    _ => RegValue::KeyOnly,
                },
                Some(v) => registry_value(v, &env)
                    .map_err(|what| row_err("Registry", r, &row.registry, "Value", what))?,
            };
            let name = match value {
                RegValue::CreateKey
                | RegValue::DeleteKeyOnUninstall
                | RegValue::CreateKeyDeleteOnUninstall => None,
                _ => name,
            };
            registry.push(PlanRegistry {
                id: row.registry.clone(),
                component: row.component.clone(),
                hive,
                key,
                name,
                value,
                win64: c.win64,
            });
        }

        // Shortcuts.
        let mut shortcuts = Vec::new();
        for (r, s) in pkg.shortcuts()?.into_iter().enumerate() {
            let advertised = feature_state.contains_key(&s.target);
            let wanted = if advertised {
                feature_state
                    .get(&s.target)
                    .is_some_and(|st| st.installed() || *st == State::Advertised)
            } else {
                comp_state.get(&s.component).is_some_and(|st| st.installed())
            };
            if !comp_state.contains_key(&s.component) {
                return Err(row_err(
                    "Shortcut",
                    r,
                    &s.shortcut,
                    "Component_",
                    format!("component {} is not in the Component table", s.component),
                ));
            }
            if !wanted {
                continue;
            }
            let Some(d) = dir_of(&s.directory) else {
                return Err(row_err(
                    "Shortcut",
                    r,
                    &s.shortcut,
                    "Directory_",
                    format!("directory {} is not in the Directory table", s.directory),
                ));
            };
            let (ns, nl) = split_name(&s.name);
            let name = if short_targets { ns } else { nl };
            let target = if advertised {
                ShortcutTarget::Feature(s.target.clone())
            } else {
                ShortcutTarget::Path(format::format(&s.target, &env, Column::Other))
            };
            let working_dir = s
                .working_dir
                .as_deref()
                .map(|w| props.get(w).cloned().unwrap_or_else(|| format!("[{w}] (not set)")));
            shortcuts.push(PlanShortcut {
                id: s.shortcut.clone(),
                component: s.component.clone(),
                path: format!("{}{name}.lnk", d.target),
                target,
                arguments: s.arguments.as_deref().map(|a| format::format(a, &env, Column::Other)),
                working_dir,
                description: s.description.clone(),
                icon: s.icon.clone(),
                icon_index: s.icon_index,
                show_cmd: s.show_cmd,
            });
        }

        // Launch conditions.
        let lc = Rows::read(pkg, "LaunchCondition")?;
        let mut launch = Vec::new();
        for r in 0..lc.len() {
            let text = lc.need_s(r, "Condition")?;
            let c = parse_condition("LaunchCondition", r, &text, "Condition", &text)?;
            let message =
                format::format(&lc.s(r, "Description").unwrap_or_default(), &env, Column::Other);
            launch.push(LaunchCheck { condition: text, message, passes: c.evaluate(&env) });
        }

        // Sequences.
        let cas = pkg.custom_actions()?;
        let dialogs = Rows::read(pkg, "Dialog")?;
        let dialog_names: BTreeSet<String> =
            (0..dialogs.len()).filter_map(|r| dialogs.s(r, "Dialog")).collect();
        let counts = [
            ("InstallFiles", files.iter().filter(|f| f.state == State::Local).count()),
            ("WriteRegistryValues", registry.len()),
            ("CreateShortcuts", shortcuts.len()),
        ];
        let mut actions = Vec::new();
        for table in SEQUENCES {
            let t = Rows::read(pkg, table)?;
            let mut rows = Vec::new();
            for r in 0..t.len() {
                let action = t.need_s(r, "Action")?;
                let condition = t.s(r, "Condition");
                let c = parse_condition(
                    table,
                    r,
                    &action,
                    "Condition",
                    condition.as_deref().unwrap_or(""),
                )?;
                let sequence = t.i(r, "Sequence").unwrap_or(0);
                let kind = if let Some(ca) = cas.iter().find(|c| c.action == action) {
                    let formatted =
                        ca.target.as_deref().map(|x| format::format(x, &env, Column::Other));
                    ActionKind::Custom {
                        ty: ca.ty,
                        what: describe_custom_action(
                            ca.ty,
                            ca.source.as_deref(),
                            formatted.as_deref(),
                            ca.extended_type,
                        ),
                    }
                } else if let Some(&(_, what)) = STANDARD.iter().find(|(n, _)| *n == action) {
                    match counts.iter().find(|(n, _)| *n == action) {
                        Some((_, n)) => ActionKind::Standard(format!("{what}: {n}")),
                        None => ActionKind::Standard(what.into()),
                    }
                } else if dialog_names.contains(&action) {
                    ActionKind::Dialog
                } else {
                    ActionKind::Unknown
                };
                rows.push(PlanAction {
                    table: table.into(),
                    action,
                    sequence,
                    condition,
                    runs: sequence > 0 && c.evaluate(&env),
                    kind,
                });
            }
            // Positive sequence numbers in order, then the exit
            // actions (-1 success, -2 user exit, -3 failure, -4
            // suspend), then 0/NULL (never run).
            rows.sort_by_key(|a| match a.sequence {
                s if s > 0 => (0, s),
                s if s < 0 => (1, -s),
                _ => (2, 0),
            });
            actions.extend(rows);
        }
        notes.push(
            "fresh install: every feature and component is installed-absent; AppSearch, \
             custom actions and dialogs are not run, so properties they would set are not \
             in this plan"
                .into(),
        );
        Ok(Plan {
            scope,
            scope_why,
            properties: props.clone(),
            directories,
            features,
            components,
            files,
            registry,
            shortcuts,
            launch,
            actions,
            notes,
        })
    }

    /// Whether every launch condition passes.
    pub fn can_install(&self) -> bool {
        self.launch.iter().all(|l| l.passes)
    }
}

/// Resolves every directory's target and source path.
fn resolve(
    dirs: &[super::Directory],
    props: &Properties,
    short_targets: bool,
    short_sources: bool,
) -> Result<Vec<PlanDirectory>, PlanError> {
    let index: BTreeMap<&str, usize> =
        dirs.iter().enumerate().map(|(i, d)| (d.directory.as_str(), i)).collect();
    let mut done: Vec<Option<PlanDirectory>> = (0..dirs.len()).map(|_| None).collect();
    let source_root = props.get("SourceDir").cloned().unwrap_or_default();
    for start in 0..dirs.len() {
        // Walk up to a resolved directory or a root, then back down.
        let mut chain = Vec::new();
        let mut cur = start;
        loop {
            if done[cur].is_some() {
                break;
            }
            chain.push(cur);
            if chain.len() > dirs.len() {
                return Err(row_err(
                    "Directory",
                    start,
                    &dirs[start].directory,
                    "Directory_Parent",
                    "the directory is its own ancestor",
                ));
            }
            let d = &dirs[cur];
            if d.is_root() {
                break;
            }
            let parent = d.parent.as_deref().unwrap_or_default();
            let Some(&p) = index.get(parent) else {
                return Err(row_err(
                    "Directory",
                    cur,
                    &d.directory,
                    "Directory_Parent",
                    format!("parent {parent} is not in the Directory table"),
                ));
            };
            cur = p;
        }
        for &i in chain.iter().rev() {
            let d = &dirs[i];
            let (ts, tl, ss, sl) = default_dir(&d.default_dir);
            let tname = if short_targets { ts } else { tl };
            let sname = if short_sources { ss } else { sl };
            let parent = if d.is_root() {
                None
            } else {
                let p = index[d.parent.as_deref().unwrap_or_default()];
                done[p].clone()
            };
            let (target, from) = match props.get(&d.directory).filter(|v| !v.is_empty()) {
                Some(v) => (slash(v), "property"),
                None => match &parent {
                    None => {
                        (slash(props.get("ROOTDRIVE").map_or("C:\\", String::as_str)), "ROOTDRIVE")
                    }
                    Some(p) if tname == "." => (p.target.clone(), "parent"),
                    Some(p) => (format!("{}{tname}\\", p.target), "parent"),
                },
            };
            let source = match &parent {
                None => slash(&source_root),
                Some(p) if sname == "." => p.source.clone(),
                Some(p) => format!("{}{sname}\\", p.source),
            };
            done[i] = Some(PlanDirectory { id: d.directory.clone(), target, source, from });
        }
    }
    Ok(done.into_iter().flatten().collect())
}

/// Each directory's short path (for `[!filekey]`): a directory whose
/// path came from a property (or ROOTDRIVE) keeps it; below it the
/// DefaultDir short names.
fn short_paths(dirs: &[super::Directory], resolved: &[PlanDirectory]) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for _ in 0..=dirs.len() {
        let mut changed = false;
        for (d, pd) in dirs.iter().zip(resolved) {
            if out.contains_key(&d.directory) {
                continue;
            }
            let short = if pd.from != "parent" {
                Some(pd.target.clone())
            } else {
                let (ts, ..) = default_dir(&d.default_dir);
                d.parent
                    .as_deref()
                    .and_then(|p| out.get(p))
                    .map(|p| if ts == "." { p.clone() } else { format!("{p}{ts}\\") })
            };
            if let Some(v) = short {
                out.insert(d.directory.clone(), v);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    out
}

/// A feature's default state when it is selected.
fn default_state(attributes: i32) -> State {
    if attributes & FAVOR_SOURCE != 0 {
        State::Source
    } else if attributes & FAVOR_ADVERTISE != 0 {
        State::Advertised
    } else {
        State::Local
    }
}

fn select_features(
    feats: &[super::Feature],
    levels: &BTreeMap<String, i32>,
    level_why: &BTreeMap<String, String>,
    install_level: i32,
    props: &Properties,
) -> Result<Vec<PlanFeature>, PlanError> {
    let by_id: BTreeMap<&str, &super::Feature> =
        feats.iter().map(|f| (f.feature.as_str(), f)).collect();
    let level = |id: &str| levels.get(id).copied().unwrap_or(0);
    let mut state: BTreeMap<String, (State, String)> = BTreeMap::new();
    let requested = REQUESTS.iter().any(|(p, _)| props.get(*p).is_some_and(|v| !v.is_empty()));
    if !requested {
        for f in feats {
            let l = level(&f.feature);
            let why_level = level_why.get(&f.feature).map_or(String::new(), |w| format!(" ({w})"));
            let s = if l == 0 {
                (State::Null, format!("level 0: disabled{why_level}"))
            } else if l > install_level {
                (State::Null, format!("level {l} is above INSTALLLEVEL {install_level}{why_level}"))
            } else {
                (
                    default_state(f.attributes),
                    format!("level {l} <= INSTALLLEVEL {install_level}{why_level}"),
                )
            };
            state.insert(f.feature.clone(), s);
        }
    } else {
        for f in feats {
            state.insert(f.feature.clone(), (State::Null, "not requested".into()));
        }
        for (prop, want) in REQUESTS {
            let Some(list) = props.get(prop).filter(|v| !v.is_empty()) else { continue };
            let names: Vec<&str> = if list == "ALL" {
                feats.iter().map(|f| f.feature.as_str()).collect()
            } else {
                list.split(',').map(str::trim).collect()
            };
            for n in names {
                let Some(f) = by_id.get(n) else {
                    return Err(PlanError::Property {
                        name: prop.into(),
                        what: format!("{n} is not in the Feature table"),
                    });
                };
                if level(n) == 0 {
                    state.insert(n.into(), (State::Null, format!("{prop}: level 0, disabled")));
                    continue;
                }
                let s = want.unwrap_or_else(|| default_state(f.attributes));
                state.insert(n.into(), (s, format!("{prop}={list}")));
                if s == State::Absent {
                    // And everything below it.
                    for g in feats {
                        if ancestors(&by_id, &g.feature).contains(&n) {
                            state.insert(
                                g.feature.clone(),
                                (State::Absent, format!("{prop}: parent {n}")),
                            );
                        }
                    }
                } else {
                    // Installing a feature installs its parents.
                    for a in ancestors(&by_id, n) {
                        if !state.get(a).is_some_and(|(s, _)| s.installed()) {
                            state.insert(a.into(), (s, format!("{prop}: child {n}")));
                        }
                    }
                }
            }
        }
    }
    // followParent, then a parent that is not installed.
    let mut out = Vec::new();
    for f in feats {
        let (mut s, mut why) =
            state.get(&f.feature).cloned().unwrap_or((State::Null, String::new()));
        let chain = ancestors(&by_id, &f.feature);
        if let Some(p) = chain.first() {
            let (ps, _) = effective(&state, &by_id, p);
            if s.installed() && f.attributes & FOLLOW_PARENT != 0 && ps.installed() {
                s = ps;
                why.push_str(&format!("; followParent: {}", ps.name()));
            }
            if s.installed() || s == State::Advertised {
                if !ps.installed() && ps != State::Advertised {
                    s = State::Null;
                    why = format!("parent {p} is not installed");
                } else if ps == State::Advertised && s != State::Advertised {
                    s = State::Advertised;
                    why.push_str(&format!("; parent {p} is advertised"));
                }
            }
        }
        out.push(PlanFeature {
            id: f.feature.clone(),
            parent: f.parent.clone().filter(|p| *p != f.feature),
            authored_level: f.level,
            level: level(&f.feature),
            attributes: f.attributes,
            state: s,
            why,
        });
    }
    Ok(out)
}

/// A feature's state once its ancestors are taken into account.
fn effective(
    state: &BTreeMap<String, (State, String)>,
    by_id: &BTreeMap<&str, &super::Feature>,
    id: &str,
) -> (State, String) {
    let own = state.get(id).cloned().unwrap_or((State::Null, String::new()));
    for a in ancestors(by_id, id) {
        let (s, _) = state.get(a).cloned().unwrap_or((State::Null, String::new()));
        if !s.installed() && s != State::Advertised {
            return (State::Null, format!("ancestor {a} is not installed"));
        }
    }
    own
}

/// The feature's parent, grandparent, ... (a loop stops the walk).
fn ancestors<'f>(by_id: &BTreeMap<&str, &'f super::Feature>, id: &str) -> Vec<&'f str> {
    let mut out: Vec<&'f str> = Vec::new();
    let mut cur = by_id.get(id).and_then(|f| f.parent.as_deref()).filter(|p| *p != id);
    while let Some(p) = cur {
        let Some(f) = by_id.get(p) else { break };
        if out.contains(&f.feature.as_str()) || f.feature == id {
            break;
        }
        out.push(f.feature.as_str());
        cur = f.parent.as_deref().filter(|q| *q != p);
    }
    out
}

/// A component's state from its features (the Component table's
/// Attributes: LocalOnly 0, SourceOnly 1, Optional 2).
fn component_state(
    attributes: i32,
    features: &[String],
    states: &BTreeMap<String, State>,
) -> (State, String) {
    if features.is_empty() {
        return (State::Null, "in no feature".into());
    }
    let has = |s: State| features.iter().find(|f| states.get(*f) == Some(&s));
    let run_from = attributes & 3;
    if let Some(f) = has(State::Local) {
        if run_from == PlanComponent::SOURCE_ONLY {
            return (State::Source, format!("feature {f} is local, the component is source-only"));
        }
        return (State::Local, format!("feature {f} is local"));
    }
    if let Some(f) = has(State::Source) {
        if run_from == 0 {
            return (State::Local, format!("feature {f} is source, the component is local-only"));
        }
        return (State::Source, format!("feature {f} runs from source"));
    }
    if let Some(f) = has(State::Advertised) {
        return (State::Null, format!("feature {f} is advertised: installed on first use"));
    }
    (State::Null, format!("no feature of {} is installed", features.join(", ")))
}

/// A Registry row's Value (the Registry table page: `#x` binary, `#%`
/// expandable, `#` integer, `##` a string starting with `#`, `[~]`
/// lists), formatted after its prefix is taken off.
fn registry_value(raw: &str, env: &Env) -> Result<RegValue, String> {
    let f = |s: &str| format::format(s, env, Column::RegistryValue);
    if let Some(rest) = raw.strip_prefix("##") {
        return Ok(RegValue::Sz(f(&format!("#{rest}"))));
    }
    if let Some(rest) = raw.strip_prefix("#x").or_else(|| raw.strip_prefix("#X")) {
        let hex = f(rest);
        let digits: Vec<u8> = hex.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        if !digits.len().is_multiple_of(2) {
            return Err(format!("#x{hex}: an odd number of hex digits"));
        }
        let mut out = Vec::new();
        for pair in digits.chunks(2) {
            let s = core::str::from_utf8(pair).unwrap_or("");
            match u8::from_str_radix(s, 16) {
                Ok(b) => out.push(b),
                Err(_) => return Err(format!("#x{hex}: {s:?} is not hex")),
            }
        }
        return Ok(RegValue::Binary(out));
    }
    if let Some(rest) = raw.strip_prefix("#%") {
        return Ok(RegValue::ExpandSz(f(rest)));
    }
    if let Some(rest) = raw.strip_prefix('#') {
        let v = f(rest);
        return match cond::as_integer(v.trim()) {
            Some(i) => Ok(RegValue::Dword(i as u32)),
            None => match v.trim().parse::<u32>() {
                Ok(u) => Ok(RegValue::Dword(u)),
                Err(_) => Err(format!("#{v}: not an integer")),
            },
        };
    }
    if raw.contains("[~]") {
        let v = f(raw);
        let mut items: Vec<&str> = v.split('\0').collect();
        let first = items.first().is_some_and(|s| s.is_empty());
        let last = items.len() > 1 && items.last().is_some_and(|s| s.is_empty());
        let mode = match (first, last) {
            (true, false) => MultiMode::Append,
            (false, true) => MultiMode::Prepend,
            _ => MultiMode::Replace,
        };
        if first {
            items.remove(0);
        }
        if last {
            items.pop();
        }
        return Ok(RegValue::MultiSz(mode, items.into_iter().map(str::to_owned).collect()));
    }
    Ok(RegValue::Sz(f(raw)))
}

// ---------------------------------------------------------------------
// One line per item.

impl fmt::Display for PlanDirectory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "directory {} = {} ({}; source {})", self.id, self.target, self.from, self.source)
    }
}

impl fmt::Display for PlanFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "feature {} level {} -> {}: {}", self.id, self.level, self.state.name(), self.why)
    }
}

impl fmt::Display for PlanComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "component {} -> {} in {}", self.id, self.state.name(), self.path)?;
        match &self.key_path {
            KeyPath::Directory => f.write_str(", key path the directory")?,
            KeyPath::File(k) => write!(f, ", key path file {k}")?,
            KeyPath::Registry(k) => write!(f, ", key path registry {k}")?,
            KeyPath::OdbcDataSource(k) => write!(f, ", key path ODBC data source {k}")?,
        }
        if self.win64 {
            f.write_str(", 64-bit")?;
        }
        write!(f, ": {}", self.why)
    }
}

impl fmt::Display for PlanFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verb = if self.state == State::Local { "->" } else { "run from source" };
        write!(f, "file {} {verb} {} ({} bytes", self.key, self.target, self.size)?;
        if let Some(v) = &self.version {
            write!(f, ", version {v}")?;
        }
        match &self.source {
            FileSource::Embedded { cabinet, .. } => write!(f, ", from embedded cabinet {cabinet})"),
            FileSource::External { cabinet, .. } => write!(f, ", from cabinet {cabinet})"),
            FileSource::Loose { .. } => write!(f, ", from {})", self.source_path),
        }
    }
}

impl fmt::Display for PlanRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "registry {}\\{}", self.hive.name(), self.key)?;
        match &self.name {
            Some(n) => write!(f, " {n:?}")?,
            None if matches!(
                self.value,
                RegValue::CreateKey
                    | RegValue::DeleteKeyOnUninstall
                    | RegValue::CreateKeyDeleteOnUninstall
                    | RegValue::KeyOnly
            ) => {}
            None => f.write_str(" (default)")?,
        }
        write!(f, " = {}", self.value)?;
        if self.win64 {
            f.write_str(" [64-bit view]")?;
        }
        write!(f, " (component {})", self.component)
    }
}

impl fmt::Display for PlanShortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "shortcut {} {} -> ", self.id, self.path)?;
        match &self.target {
            ShortcutTarget::Feature(x) => write!(f, "feature {x} (advertised)")?,
            ShortcutTarget::Path(p) => f.write_str(p)?,
        }
        if let Some(a) = &self.arguments {
            write!(f, " args {a:?}")?;
        }
        if let Some(w) = &self.working_dir {
            write!(f, " in {w}")?;
        }
        Ok(())
    }
}

impl fmt::Display for LaunchCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let r = if self.passes { "passes" } else { "FAILS" };
        write!(f, "launch condition {} {r}", self.condition)?;
        if !self.passes {
            write!(f, ": {}", self.message)?;
        }
        Ok(())
    }
}

impl fmt::Display for PlanAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let when = match self.sequence {
            s if s > 0 => format!("{s}"),
            -1 => "on success".into(),
            -2 => "on user exit".into(),
            -3 => "on failure".into(),
            -4 => "on suspend".into(),
            s => format!("{s} (never)"),
        };
        write!(f, "action {} {when} {}", self.table, self.action)?;
        match &self.kind {
            ActionKind::Standard(w) => write!(f, " (standard: {w})")?,
            ActionKind::Custom { ty, what } => write!(f, " (custom type {ty}: {what})")?,
            ActionKind::Dialog => f.write_str(" (dialog)")?,
            ActionKind::Unknown => {
                f.write_str(" (not a standard action, custom action or dialog)")?
            }
        }
        if let Some(c) = &self.condition {
            write!(f, " if {c}")?;
        }
        f.write_str(if self.runs { ": runs" } else { ": skipped" })
    }
}

impl fmt::Display for Plan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scope = match self.scope {
            Scope::PerUser => "per-user",
            Scope::PerMachine => "per-machine",
        };
        writeln!(f, "context {scope}: {}", self.scope_why)?;
        for l in &self.launch {
            writeln!(f, "{l}")?;
        }
        for d in &self.directories {
            writeln!(f, "{d}")?;
        }
        for x in &self.features {
            writeln!(f, "{x}")?;
        }
        for x in &self.components {
            writeln!(f, "{x}")?;
        }
        for x in &self.files {
            writeln!(f, "{x}")?;
        }
        for x in &self.registry {
            writeln!(f, "{x}")?;
        }
        for x in &self.shortcuts {
            writeln!(f, "{x}")?;
        }
        for x in &self.actions {
            writeln!(f, "{x}")?;
        }
        for n in &self.notes {
            writeln!(f, "note: {n}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
