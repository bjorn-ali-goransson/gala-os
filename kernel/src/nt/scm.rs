//! The service control manager (ARCHITECTURE "The service control
//! manager"): the service database is the registry's
//! `HKLM\SYSTEM\CurrentControlSet\Services`, Windows' own store (a
//! subkey with a `Type` value is a service, as the SCM loads them); the
//! SCM's and each service's security descriptor decide what a connection
//! or service handle is granted (the documented defaults of "Service
//! Security and Access Rights", or a service key's `Security\Security`);
//! the run-time state (open handles, deletion marks) is kept here. Only
//! the drivers gala's kernel itself provides are running; starting a
//! service is not supported yet (recorded). The calls are in
//! `scm_calls.rs`; advapi32 wraps them.

use alloc::vec::Vec;

use super::object::GenericMapping;
use super::registry::{self, KeyId, Registry};
use super::security::{self, Descriptor, SE_DACL_PRESENT};
use super::status::NtStatus;
use super::token::{ADMINISTRATORS, LOCAL_SYSTEM, Sid};
use crate::mm::heap::TryVec;
use crate::sync::IrqMutex;

pub const DELETE: u32 = 0x0001_0000;
pub const READ_CONTROL: u32 = 0x0002_0000;
const STANDARD_RIGHTS_REQUIRED: u32 = 0x000f_0000;

// The service control manager's rights (winsvc.h).
pub const SC_MANAGER_CONNECT: u32 = 0x0001;
pub const SC_MANAGER_CREATE_SERVICE: u32 = 0x0002;
pub const SC_MANAGER_ENUMERATE_SERVICE: u32 = 0x0004;
pub const SC_MANAGER_LOCK: u32 = 0x0008;
pub const SC_MANAGER_QUERY_LOCK_STATUS: u32 = 0x0010;
pub const SC_MANAGER_MODIFY_BOOT_CONFIG: u32 = 0x0020;
pub const SC_MANAGER_ALL_ACCESS: u32 = STANDARD_RIGHTS_REQUIRED | 0x3f;

// A service's rights.
pub const SERVICE_QUERY_CONFIG: u32 = 0x0001;
pub const SERVICE_CHANGE_CONFIG: u32 = 0x0002;
pub const SERVICE_QUERY_STATUS: u32 = 0x0004;
pub const SERVICE_ENUMERATE_DEPENDENTS: u32 = 0x0008;
pub const SERVICE_START: u32 = 0x0010;
pub const SERVICE_STOP: u32 = 0x0020;
pub const SERVICE_PAUSE_CONTINUE: u32 = 0x0040;
pub const SERVICE_INTERROGATE: u32 = 0x0080;
pub const SERVICE_USER_DEFINED_CONTROL: u32 = 0x0100;
pub const SERVICE_ALL_ACCESS: u32 = STANDARD_RIGHTS_REQUIRED | 0x1ff;

/// The documented generic mapping of the SCM's rights.
pub const SCM_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL | SC_MANAGER_ENUMERATE_SERVICE | SC_MANAGER_QUERY_LOCK_STATUS,
    write: READ_CONTROL | SC_MANAGER_CREATE_SERVICE | SC_MANAGER_MODIFY_BOOT_CONFIG,
    execute: READ_CONTROL | SC_MANAGER_CONNECT | SC_MANAGER_LOCK,
    all: SC_MANAGER_ALL_ACCESS,
};

/// The documented generic mapping of a service's rights.
pub const SERVICE_MAPPING: GenericMapping = GenericMapping {
    read: READ_CONTROL
        | SERVICE_QUERY_CONFIG
        | SERVICE_QUERY_STATUS
        | SERVICE_INTERROGATE
        | SERVICE_ENUMERATE_DEPENDENTS,
    write: READ_CONTROL | SERVICE_CHANGE_CONFIG,
    execute: READ_CONTROL
        | SERVICE_START
        | SERVICE_STOP
        | SERVICE_PAUSE_CONTINUE
        | SERVICE_USER_DEFINED_CONTROL,
    all: SERVICE_ALL_ACCESS,
};

// Service types, start types, states (winnt.h, winsvc.h).
pub const SERVICE_KERNEL_DRIVER: u32 = 0x01;
pub const SERVICE_FILE_SYSTEM_DRIVER: u32 = 0x02;
pub const SERVICE_DRIVER: u32 = 0x0b;
pub const SERVICE_WIN32_OWN_PROCESS: u32 = 0x10;
pub const SERVICE_WIN32_SHARE_PROCESS: u32 = 0x20;
pub const SERVICE_WIN32: u32 = 0x30;
pub const SERVICE_INTERACTIVE_PROCESS: u32 = 0x100;
pub const SERVICE_BOOT_START: u32 = 0;
pub const SERVICE_SYSTEM_START: u32 = 1;
pub const SERVICE_DISABLED: u32 = 4;
pub const SERVICE_ACTIVE: u32 = 1;
pub const SERVICE_INACTIVE: u32 = 2;
pub const SERVICE_STOPPED: u32 = 1;
pub const SERVICE_RUNNING: u32 = 4;

// The Win32 errors the SCM answers with.
pub const ERROR_ACCESS_DENIED: u32 = 5;
pub const ERROR_INVALID_PARAMETER: u32 = 87;
pub const ERROR_INVALID_NAME: u32 = 123;
pub const ERROR_INVALID_SERVICE_CONTROL: u32 = 1052;
pub const ERROR_SERVICE_ALREADY_RUNNING: u32 = 1056;
pub const ERROR_SERVICE_DISABLED: u32 = 1058;
pub const ERROR_SERVICE_DOES_NOT_EXIST: u32 = 1060;
pub const ERROR_SERVICE_NOT_ACTIVE: u32 = 1062;
pub const ERROR_SERVICE_MARKED_FOR_DELETE: u32 = 1072;
pub const ERROR_SERVICE_EXISTS: u32 = 1073;
pub const ERROR_SERVICE_NEVER_STARTED: u32 = 1077;
pub const ERROR_NOT_SUPPORTED: u32 = 50;

/// The longest service name (characters), as documented for CreateService.
pub const MAX_NAME: usize = 256;

/// An NTSTATUS carrying Win32 error `error` (FACILITY_NTWIN32, as
/// NTSTATUS_FROM_WIN32): the SCM's errors have no NTSTATUS of their own;
/// advapi32 takes the error back out.
pub const fn win32(error: u32) -> NtStatus {
    NtStatus(0xc007_0000 | (error & 0xffff))
}

/// NT AUTHORITY\Authenticated Users.
const AUTHENTICATED_USERS: Sid = Sid::new(5, &[11]);

/// The drivers gala's kernel itself provides (`\Device\Afd`: nt::afd),
/// running for as long as the kernel runs; their keys are in the seed.
const BUILT_IN: &[&str] = &["AFD"];

fn descriptor(list: &[(u32, Sid, u8)]) -> Result<Vec<u8>, NtStatus> {
    let d = Descriptor {
        control: SE_DACL_PRESENT,
        owner: Some(LOCAL_SYSTEM),
        group: Some(LOCAL_SYSTEM),
        dacl: Some(security::allow_acl(list)?),
        sacl: None,
    };
    d.to_bytes(u32::MAX)
}

/// The SCM's descriptor ("Service Security and Access Rights"): local
/// authenticated users connect, enumerate and query the lock status;
/// SYSTEM also changes the boot configuration; Administrators get all.
pub fn manager_descriptor() -> Result<Vec<u8>, NtStatus> {
    let users = READ_CONTROL
        | SC_MANAGER_CONNECT
        | SC_MANAGER_ENUMERATE_SERVICE
        | SC_MANAGER_QUERY_LOCK_STATUS;
    descriptor(&[
        (users, AUTHENTICATED_USERS, 0),
        (users | SC_MANAGER_MODIFY_BOOT_CONFIG, LOCAL_SYSTEM, 0),
        (SC_MANAGER_ALL_ACCESS, ADMINISTRATORS, 0),
    ])
}

/// A service's descriptor when its key has none (the documented default
/// CreateService gives): local authenticated users query and interrogate;
/// SYSTEM also starts, stops, pauses and continues; Administrators get all.
pub fn default_service_descriptor() -> Result<Vec<u8>, NtStatus> {
    let users = READ_CONTROL
        | SERVICE_ENUMERATE_DEPENDENTS
        | SERVICE_INTERROGATE
        | SERVICE_QUERY_CONFIG
        | SERVICE_QUERY_STATUS
        | SERVICE_USER_DEFINED_CONTROL;
    descriptor(&[
        (users, AUTHENTICATED_USERS, 0),
        (users | SERVICE_START | SERVICE_STOP | SERVICE_PAUSE_CONTINUE, LOCAL_SYSTEM, 0),
        (SERVICE_ALL_ACCESS, ADMINISTRATORS, 0),
    ])
}

/// A handle to the service control manager (OpenSCManager).
pub struct ScManager;

/// A handle to a service (OpenService, CreateService): its key's name.
/// Counted in `TABLE` while it exists, so a service marked for deletion
/// goes when its last handle does.
pub struct ServiceObject {
    pub name: Vec<u16>,
}

/// A service with open handles.
struct Entry {
    name: Vec<u16>,
    opens: usize,
    marked: bool,
}

/// The services with open handles (most have none, and no entry).
static TABLE: IrqMutex<Vec<Entry>> = IrqMutex::new(Vec::new());

fn same(a: &[u16], b: &[u16]) -> bool {
    let up = |c: u16| if (u16::from(b'a')..=u16::from(b'z')).contains(&c) { c - 32 } else { c };
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| up(x) == up(y))
}

fn same_text(a: &[u16], b: &str) -> bool {
    a.len() == b.len()
        && a.iter().zip(b.bytes()).all(|(&x, y)| x < 0x80 && (x as u8).eq_ignore_ascii_case(&y))
}

fn copy(s: &[u16]) -> Result<Vec<u16>, NtStatus> {
    let mut v = Vec::new();
    v.try_extend_from_slice(s).map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(v)
}

impl ServiceObject {
    pub fn new(name: &[u16]) -> Result<ServiceObject, NtStatus> {
        let object = ServiceObject { name: copy(name)? };
        let mut table = TABLE.lock();
        if let Some(e) = table.iter_mut().find(|e| same(&e.name, name)) {
            e.opens += 1;
        } else {
            table
                .try_push(Entry { name: copy(name)?, opens: 1, marked: false })
                .map_err(|_| NtStatus::NO_MEMORY)?;
        }
        Ok(object)
    }
}

impl Drop for ServiceObject {
    /// The handle's object is gone: a service marked for deletion leaves
    /// the database with its last one (its key and every subkey).
    fn drop(&mut self) {
        let delete = {
            let mut table = TABLE.lock();
            let Some(at) = table.iter().position(|e| same(&e.name, &self.name)) else { return };
            table[at].opens -= 1;
            if table[at].opens > 0 {
                return;
            }
            table.swap_remove(at).marked
        };
        if !delete {
            return;
        }
        let now = super::platform::system_time();
        let result = registry::with(|r| {
            let id = find(r, &self.name)?;
            delete_tree(r, id, now, 0)
        });
        let name = super::winsta::Plain(&self.name);
        match result {
            Ok(()) => info!("scm: service \"{name}\" deleted (its last handle closed)"),
            Err(e) => warn!(
                "scm: service \"{name}\" marked for deletion could not be deleted: {:#010x}",
                e.0
            ),
        }
        crate::events::emit(
            crate::events::Category::Service,
            "service",
            format_args!(
                r#""op":"deleted","name":{},"status":{}"#,
                crate::events::Utf16(&self.name),
                result.err().map_or(0, |e| e.0)
            ),
        );
    }
}

/// Whether service `name` is marked for deletion.
pub fn marked(name: &[u16]) -> bool {
    TABLE.lock().iter().any(|e| e.marked && same(&e.name, name))
}

/// Marks service `name` (which has an open handle) for deletion
/// (ERROR_SERVICE_MARKED_FOR_DELETE if it already is).
pub fn mark(name: &[u16]) -> Result<(), NtStatus> {
    let mut table = TABLE.lock();
    let e = table
        .iter_mut()
        .find(|e| same(&e.name, name))
        .ok_or(win32(ERROR_SERVICE_DOES_NOT_EXIST))?;
    if e.marked {
        return Err(win32(ERROR_SERVICE_MARKED_FOR_DELETE));
    }
    e.marked = true;
    Ok(())
}

fn delete_tree(r: &mut Registry, id: KeyId, now: u64, depth: usize) -> Result<(), NtStatus> {
    if depth > 32 {
        return Err(NtStatus::INVALID_PARAMETER);
    }
    while let Ok(child) = r.subkey(id, 0) {
        delete_tree(r, child, now, depth + 1)?;
    }
    r.delete_key(id, now)
}

fn utf16(s: &str) -> Result<Vec<u16>, NtStatus> {
    let mut v = Vec::new();
    for c in s.encode_utf16() {
        v.try_push(c).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(v)
}

/// The database: `\Registry\Machine\SYSTEM\CurrentControlSet\Services`.
fn services(r: &Registry) -> Result<KeyId, NtStatus> {
    r.lookup(registry::ROOT, &utf16("MACHINE\\SYSTEM\\CurrentControlSet\\Services")?)
}

/// Whether `name` can name a service: 1 to 256 characters, no slashes.
pub fn valid_name(name: &[u16]) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && !name.iter().any(|&c| c == u16::from(b'\\') || c == u16::from(b'/'))
}

fn dword(r: &Registry, id: KeyId, name: &str) -> Option<u32> {
    let v = r.value(id, &utf16(name).ok()?).ok()?;
    (v.kind == 4 && v.data.len() >= 4)
        .then(|| u32::from_le_bytes([v.data[0], v.data[1], v.data[2], v.data[3]]))
}

/// A string value's characters, without the NULs at its end (REG_SZ,
/// REG_EXPAND_SZ, unexpanded; REG_MULTI_SZ with its inner NULs).
fn text(r: &Registry, id: KeyId, name: &str) -> Result<Option<Vec<u16>>, NtStatus> {
    let Ok(v) = r.value(id, &utf16(name)?) else { return Ok(None) };
    if !matches!(v.kind, 1 | 2 | 7) {
        return Ok(None);
    }
    let mut out = Vec::new();
    for pair in v.data.chunks_exact(2) {
        out.try_push(u16::from_le_bytes([pair[0], pair[1]])).map_err(|_| NtStatus::NO_MEMORY)?;
    }
    while out.last() == Some(&0) {
        out.pop();
    }
    Ok(Some(out))
}

/// The key of service `name`: a subkey of the database with a `Type`
/// value (ERROR_SERVICE_DOES_NOT_EXIST otherwise).
pub fn find(r: &Registry, name: &[u16]) -> Result<KeyId, NtStatus> {
    let missing = win32(ERROR_SERVICE_DOES_NOT_EXIST);
    if !valid_name(name) {
        return Err(missing);
    }
    let id = r.lookup(services(r)?, name).map_err(|_| missing)?;
    dword(r, id, "Type").ok_or(missing)?;
    Ok(id)
}

/// The name of service key `id` as the key has it.
pub fn key_name(r: &Registry, id: KeyId) -> Result<Vec<u16>, NtStatus> {
    copy(r.key_name(id)?)
}

/// The descriptor kept in the key's `Security\Security` value (as the SCM
/// stores it), else the default.
pub fn service_descriptor(r: &Registry, id: KeyId) -> Result<Vec<u8>, NtStatus> {
    let kept = r
        .lookup(id, &utf16("Security")?)
        .ok()
        .and_then(|s| r.value(s, &utf16("Security").ok()?).ok())
        .filter(|v| v.kind == 3 && Descriptor::parse(&v.data).is_ok());
    match kept {
        Some(v) => {
            let mut b = Vec::new();
            b.try_extend_from_slice(&v.data).map_err(|_| NtStatus::NO_MEMORY)?;
            Ok(b)
        }
        None => default_service_descriptor(),
    }
}

/// Keeps `descriptor` as service `name`'s, in its key's
/// `Security\Security` value (REG_BINARY), as the SCM stores it.
pub fn store_descriptor(
    r: &mut Registry,
    name: &[u16],
    descriptor: &[u8],
    now: u64,
) -> Result<(), NtStatus> {
    let id = find(r, name)?;
    let security = utf16("Security")?;
    let (sub, _) = r.create(id, &security, 0, &[], now)?;
    r.set_value(sub, &security, 3, descriptor, now)
}

/// A service's configuration (QUERY_SERVICE_CONFIG), from its key.
pub struct Config {
    pub kind: u32,
    pub start: u32,
    pub error_control: u32,
    pub tag: u32,
    pub path: Vec<u16>,
    pub group: Vec<u16>,
    /// Each name NUL-terminated; groups with a leading `+`.
    pub dependencies: Vec<u16>,
    pub start_name: Vec<u16>,
    pub display: Vec<u16>,
}

pub fn config(r: &Registry, id: KeyId) -> Result<Config, NtStatus> {
    let kind = dword(r, id, "Type").unwrap_or(0);
    let mut dependencies = Vec::new();
    for (value, prefix) in [("DependOnService", None), ("DependOnGroup", Some(u16::from(b'+')))] {
        let Some(list) = text(r, id, value)? else { continue };
        for item in list.split(|&c| c == 0).filter(|s| !s.is_empty()) {
            if let Some(p) = prefix {
                dependencies.try_push(p).map_err(|_| NtStatus::NO_MEMORY)?;
            }
            dependencies.try_extend_from_slice(item).map_err(|_| NtStatus::NO_MEMORY)?;
            dependencies.try_push(0).map_err(|_| NtStatus::NO_MEMORY)?;
        }
    }
    let start_name = match text(r, id, "ObjectName")? {
        Some(n) => n,
        None if kind & SERVICE_WIN32 != 0 => utf16("LocalSystem")?,
        None => Vec::new(),
    };
    let display = match text(r, id, "DisplayName")? {
        Some(d) if !d.is_empty() => d,
        _ => key_name(r, id)?,
    };
    Ok(Config {
        kind,
        start: dword(r, id, "Start").unwrap_or(SERVICE_DISABLED),
        error_control: dword(r, id, "ErrorControl").unwrap_or(0),
        tag: dword(r, id, "Tag").unwrap_or(0),
        path: text(r, id, "ImagePath")?.unwrap_or_default(),
        group: text(r, id, "Group")?.unwrap_or_default(),
        dependencies,
        start_name,
        display,
    })
}

/// SERVICE_STATUS_PROCESS of service `name` of type `kind`: the built-in
/// drivers run (no process, no controls accepted); every other service
/// is stopped and was never started.
pub fn status(name: &[u16], kind: u32) -> [u32; 9] {
    if BUILT_IN.iter().any(|b| same_text(name, b)) {
        [kind, SERVICE_RUNNING, 0, 0, 0, 0, 0, 0, 0]
    } else {
        [kind, SERVICE_STOPPED, 0, ERROR_SERVICE_NEVER_STARTED, 0, 0, 0, 0, 0]
    }
}

/// A service as EnumServicesStatusEx lists it.
pub struct Listed {
    pub name: Vec<u16>,
    pub display: Vec<u16>,
    pub group: Vec<u16>,
    pub status: [u32; 9],
}

/// The services whose type has a bit of `kinds` and whose state is in
/// `states` (SERVICE_ACTIVE, SERVICE_INACTIVE), in name order.
pub fn list(r: &Registry, kinds: u32, states: u32) -> Result<Vec<Listed>, NtStatus> {
    let db = services(r)?;
    let mut out = Vec::new();
    let mut index = 0;
    while let Ok(id) = r.subkey(db, index) {
        index += 1;
        let Some(kind) = dword(r, id, "Type") else { continue };
        if kind & kinds & (SERVICE_DRIVER | SERVICE_WIN32) == 0 {
            continue;
        }
        let name = key_name(r, id)?;
        let status = status(&name, kind);
        let state = if status[1] == SERVICE_STOPPED { SERVICE_INACTIVE } else { SERVICE_ACTIVE };
        if states & state == 0 {
            continue;
        }
        let c = config(r, id)?;
        out.try_push(Listed { name, display: c.display, group: c.group, status })
            .map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(out)
}

/// What CreateService asks for.
pub struct NewService<'a> {
    pub name: &'a [u16],
    pub display: &'a [u16],
    pub path: &'a [u16],
    pub group: &'a [u16],
    /// NUL-separated, groups with a leading `+`.
    pub dependencies: &'a [u16],
    pub start_name: &'a [u16],
    pub kind: u32,
    pub start: u32,
    pub error_control: u32,
}

/// Whether `account` is LocalSystem (empty: the default).
fn local_system(account: &[u16]) -> bool {
    account.is_empty()
        || same_text(account, "LocalSystem")
        || same_text(account, ".\\LocalSystem")
        || same_text(account, "NT AUTHORITY\\SYSTEM")
}

/// CreateService's checks after the access check, in Windows' order: the
/// name (ERROR_INVALID_NAME), then the path, the type and its
/// combinations, the start type, the error control
/// (ERROR_INVALID_PARAMETER). User services (0x50, 0x60) are refused
/// as a type gala does not create.
pub fn validate(s: &NewService<'_>) -> Result<(), u32> {
    if !valid_name(s.name) {
        return Err(ERROR_INVALID_NAME);
    }
    if s.path.is_empty() {
        return Err(ERROR_INVALID_PARAMETER);
    }
    let base = s.kind & !SERVICE_INTERACTIVE_PROCESS;
    let program = matches!(base, SERVICE_WIN32_OWN_PROCESS | SERVICE_WIN32_SHARE_PROCESS);
    let driver = matches!(base, SERVICE_KERNEL_DRIVER | SERVICE_FILE_SYSTEM_DRIVER);
    if !program && !driver {
        return Err(ERROR_INVALID_PARAMETER);
    }
    if s.kind & SERVICE_INTERACTIVE_PROCESS != 0 && !(program && local_system(s.start_name)) {
        return Err(ERROR_INVALID_PARAMETER);
    }
    if s.start > SERVICE_DISABLED || (program && s.start <= SERVICE_SYSTEM_START) {
        return Err(ERROR_INVALID_PARAMETER);
    }
    if s.error_control > 3 {
        return Err(ERROR_INVALID_PARAMETER);
    }
    Ok(())
}

/// Writes a new service's key (validated): Type, Start, ErrorControl,
/// ImagePath, DisplayName, ObjectName (programs: LocalSystem unless
/// another account is named), Group, DependOnService/DependOnGroup.
/// ERROR_SERVICE_EXISTS, or ERROR_SERVICE_MARKED_FOR_DELETE for one still
/// waiting for its handles to close. Returns the key's name.
pub fn create(r: &mut Registry, s: &NewService<'_>, now: u64) -> Result<Vec<u16>, NtStatus> {
    if let Ok(id) = find(r, s.name) {
        let name = key_name(r, id)?;
        return Err(win32(if marked(&name) {
            ERROR_SERVICE_MARKED_FOR_DELETE
        } else {
            ERROR_SERVICE_EXISTS
        }));
    }
    let db = services(r)?;
    let (id, _) = r.create(db, s.name, 0, &[], now)?;
    let sz = |v: &[u16]| -> Result<Vec<u8>, NtStatus> {
        let mut b = Vec::new();
        for &c in v.iter().chain([0u16].iter()) {
            b.try_extend_from_slice(&c.to_le_bytes()).map_err(|_| NtStatus::NO_MEMORY)?;
        }
        Ok(b)
    };
    let mut values: Vec<(&str, u32, Vec<u8>)> = Vec::new();
    let mut add = |name: &'static str, kind: u32, data: Vec<u8>| -> Result<(), NtStatus> {
        values.try_push((name, kind, data)).map_err(|_| NtStatus::NO_MEMORY)
    };
    let mut d = Vec::new();
    d.try_extend_from_slice(&s.kind.to_le_bytes()).map_err(|_| NtStatus::NO_MEMORY)?;
    add("Type", 4, d)?;
    let mut d = Vec::new();
    d.try_extend_from_slice(&s.start.to_le_bytes()).map_err(|_| NtStatus::NO_MEMORY)?;
    add("Start", 4, d)?;
    let mut d = Vec::new();
    d.try_extend_from_slice(&s.error_control.to_le_bytes()).map_err(|_| NtStatus::NO_MEMORY)?;
    add("ErrorControl", 4, d)?;
    add("ImagePath", 2, sz(s.path)?)?;
    if !s.display.is_empty() {
        add("DisplayName", 1, sz(s.display)?)?;
    }
    if s.kind & SERVICE_WIN32 != 0 {
        let account =
            if local_system(s.start_name) { utf16("LocalSystem")? } else { copy(s.start_name)? };
        add("ObjectName", 1, sz(&account)?)?;
    }
    if !s.group.is_empty() {
        add("Group", 1, sz(s.group)?)?;
    }
    let (mut on_service, mut on_group) = (Vec::new(), Vec::new());
    for item in s.dependencies.split(|&c| c == 0).filter(|i| !i.is_empty()) {
        let (list, item) = match item.split_first() {
            Some((&p, rest)) if p == u16::from(b'+') => (&mut on_group, rest),
            _ => (&mut on_service, item),
        };
        for &c in item.iter().chain([0u16].iter()) {
            list.try_extend_from_slice(&c.to_le_bytes()).map_err(|_| NtStatus::NO_MEMORY)?;
        }
    }
    for (name, mut list) in [("DependOnService", on_service), ("DependOnGroup", on_group)] {
        if !list.is_empty() {
            list.try_extend_from_slice(&[0, 0]).map_err(|_| NtStatus::NO_MEMORY)?;
            add(name, 7, list)?;
        }
    }
    for (name, kind, data) in &values {
        r.set_value(id, &utf16(name)?, *kind, data, now)?;
    }
    key_name(r, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn new<'a>(name: &'a [u16], path: &'a [u16], kind: u32, start: u32) -> NewService<'a> {
        NewService {
            name,
            display: &[],
            path,
            group: &[],
            dependencies: &[],
            start_name: &[],
            kind,
            start,
            error_control: 0,
        }
    }

    #[test]
    fn create_checks_as_windows_orders_them() {
        let (n, p, e) = (w("svc"), w("x.exe"), w(""));
        assert_eq!(validate(&new(&e, &p, 0, 0)), Err(ERROR_INVALID_NAME));
        assert_eq!(validate(&new(&e, &e, 0, 0)), Err(ERROR_INVALID_NAME));
        assert_eq!(validate(&new(&n, &e, 0x10, 4)), Err(ERROR_INVALID_PARAMETER));
        assert_eq!(validate(&new(&w("a\\b"), &p, 0x10, 4)), Err(ERROR_INVALID_NAME));
        // Mixed types; interactive only for programs; one start type;
        // boot and system start only for drivers.
        assert_eq!(validate(&new(&n, &p, 0x30, 4)), Err(ERROR_INVALID_PARAMETER));
        assert_eq!(validate(&new(&n, &p, 0x102, 4)), Err(ERROR_INVALID_PARAMETER));
        assert_eq!(validate(&new(&n, &p, 0x10, 6)), Err(ERROR_INVALID_PARAMETER));
        assert_eq!(validate(&new(&n, &p, 0x10, 0)), Err(ERROR_INVALID_PARAMETER));
        assert_eq!(validate(&new(&n, &p, 0x03, 0)), Err(ERROR_INVALID_PARAMETER));
        assert_eq!(validate(&new(&n, &p, 0x110, 4)), Ok(()));
        assert_eq!(validate(&new(&n, &p, 0x01, 0)), Ok(()));
        let other = w(".\\someone");
        let mut s = new(&n, &p, 0x110, 3);
        s.start_name = &other;
        assert_eq!(validate(&s), Err(ERROR_INVALID_PARAMETER));
    }

    #[test]
    fn built_in_drivers_run_and_others_never_started() {
        assert_eq!(status(&w("afd"), 1)[1], SERVICE_RUNNING);
        let s = status(&w("ClickToRunSvc"), 0x10);
        assert_eq!((s[1], s[3], s[7]), (SERVICE_STOPPED, ERROR_SERVICE_NEVER_STARTED, 0));
    }

    #[test]
    fn names() {
        assert!(valid_name(&w("ClickToRunSvc")));
        assert!(!valid_name(&w("")) && !valid_name(&w("a/b")));
        assert!(!valid_name(&alloc::vec![u16::from(b'a'); 257]));
        assert_eq!(win32(1060).0, 0xc007_0424);
    }
}
