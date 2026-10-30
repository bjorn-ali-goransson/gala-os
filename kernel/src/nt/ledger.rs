//! The missing-features ledger: counts every request gala does not
//! support (unknown services, unknown information classes, unsupported
//! flags, known TODO paths), so that running a real program yields a work
//! list ranked by how often each gap was hit (docs/OBSERVABILITY.md).
//!
//! Entries live in a fixed table (no allocation, usable anywhere); when it
//! is full, further new kinds are only counted in `overflow`. Dumped as
//! JSON lines (`ledger: {...}`) by `dump`, which also clears the table so
//! that each dump covers one program.

use core::fmt;

use crate::sync::IrqMutex;

const ENTRIES: usize = 64;

/// Who reported the gap: a kernel service, or a name from user mode (gala's
/// own DLLs, through `record_user`), kept in a fixed table.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Service {
    Kernel(&'static str),
    User(usize),
}

const USER_NAMES: usize = 128;
const USER_NAME_LEN: usize = 64;

struct UserNames {
    names: [[u8; USER_NAME_LEN]; USER_NAMES],
    lens: [usize; USER_NAMES],
    count: usize,
}

static USER: IrqMutex<UserNames> = IrqMutex::new(UserNames {
    names: [[0; USER_NAME_LEN]; USER_NAMES],
    lens: [0; USER_NAMES],
    count: 0,
});

/// The index of `name` in the user-name table, adding it if there is room.
fn intern(name: &[u8]) -> Option<usize> {
    let name = &name[..name.len().min(USER_NAME_LEN)];
    let mut table = USER.lock();
    let t = &mut *table;
    if let Some(i) = (0..t.count).find(|&i| &t.names[i][..t.lens[i]] == name) {
        return Some(i);
    }
    let i = t.count;
    if i == USER_NAMES {
        return None;
    }
    t.names[i][..name.len()].copy_from_slice(name);
    t.lens[i] = name.len();
    t.count += 1;
    Some(i)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    /// The service (or kernel area) name; "?" for an unknown service.
    service: Service,
    /// What was missing, e.g. "info class", "unknown service", "flag".
    what: &'static str,
    /// The class, flag bits or service number.
    detail: u64,
}

#[derive(Clone, Copy)]
struct Entry {
    key: Key,
    hits: u64,
}

struct Ledger {
    entries: [Option<Entry>; ENTRIES],
    overflow: u64,
}

static LEDGER: IrqMutex<Ledger> = IrqMutex::new(Ledger { entries: [None; ENTRIES], overflow: 0 });

/// Counts one hit of a missing feature.
pub fn record(service: &'static str, what: &'static str, detail: u64) {
    add(Key { service: Service::Kernel(service), what, detail });
}

/// What a user-mode report is about (`GalaRecordMissing`'s code).
pub fn user_what(code: u32) -> &'static str {
    match code {
        1 => "missing export",
        2 => "unsupported",
        3 => "not implemented",
        _ => "other",
    }
}

/// Counts a gap reported by gala's user-mode DLLs (names like
/// `kernel32.dll!CreateFileW`; printable ASCII only).
pub fn record_user(name: &[u8], what: &'static str, detail: u64) {
    let clean = name.iter().all(|&b| (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\');
    match if clean { intern(name) } else { None } {
        Some(i) => add(Key { service: Service::User(i), what, detail }),
        None => add(Key { service: Service::Kernel("?user"), what, detail }),
    }
}

fn add(key: Key) {
    let mut ledger = LEDGER.lock();
    if let Some(entry) = ledger.entries.iter_mut().flatten().find(|e| e.key == key) {
        entry.hits += 1;
    } else if let Some(slot) = ledger.entries.iter_mut().find(|e| e.is_none()) {
        *slot = Some(Entry { key, hits: 1 });
    } else {
        ledger.overflow += 1;
    }
}

/// One ledger entry as a JSON object.
struct Json<'a>(&'a Entry);

impl fmt::Display for Json<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Entry { key, hits } = self.0;
        // Kernel names and kinds are literals without quotes or backslashes;
        // user names are checked for that in `record_user`.
        f.write_str(r#"{"service":""#)?;
        match key.service {
            Service::Kernel(name) => f.write_str(name)?,
            Service::User(i) => {
                let table = USER.lock();
                let name = &table.names[i][..table.lens[i]];
                f.write_str(core::str::from_utf8(name).unwrap_or("?"))?;
            }
        }
        write!(f, r#"","what":"{}","detail":{},"hits":{}}}"#, key.what, key.detail, hits)
    }
}

/// Prints every entry as a `ledger: {...}` line and clears the table.
pub fn dump() {
    let (entries, overflow) = {
        let mut ledger = LEDGER.lock();
        let snapshot = (ledger.entries, ledger.overflow);
        ledger.entries = [None; ENTRIES];
        ledger.overflow = 0;
        snapshot
    };
    for entry in entries.iter().flatten() {
        crate::kprintln!("ledger: {}", Json(entry));
    }
    if overflow > 0 {
        crate::kprintln!(
            r#"ledger: {{"service":"?","what":"ledger full","detail":0,"hits":{overflow}}}"#
        );
    }
}

/// Where the information class is among a service's arguments, for
/// recording STATUS_INVALID_INFO_CLASS generically.
pub fn class_argument(service: &str) -> Option<usize> {
    Some(match service {
        "NtQuerySystemInformation" | "NtSetSystemInformation" => 0,
        "NtQueryInformationProcess"
        | "NtSetInformationProcess"
        | "NtQueryInformationThread"
        | "NtSetInformationThread"
        | "NtQueryObject"
        | "NtSetInformationObject"
        | "NtQueryEvent"
        | "NtQueryMutant"
        | "NtQuerySemaphore"
        | "NtQueryTimer"
        | "NtQueryIoCompletion"
        | "NtQuerySection" => 1,
        "NtQueryVirtualMemory" => 2,
        "NtQueryInformationFile"
        | "NtSetInformationFile"
        | "NtQueryVolumeInformationFile"
        | "NtSetVolumeInformationFile" => 4,
        "NtQueryDirectoryFile" => 7,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_form() {
        let entry = Entry {
            key: Key { service: Service::Kernel("NtX"), what: "info class", detail: 7 },
            hits: 2,
        };
        let text = alloc::format!("{}", Json(&entry));
        assert_eq!(text, r#"{"service":"NtX","what":"info class","detail":7,"hits":2}"#);
    }

    #[test]
    fn user_names_are_interned() {
        let a = intern(b"kernel32.dll!Foo").unwrap();
        assert_eq!(intern(b"kernel32.dll!Foo"), Some(a));
        assert_ne!(intern(b"kernel32.dll!Bar"), Some(a));
        let entry = Entry {
            key: Key { service: Service::User(a), what: "missing export", detail: 0 },
            hits: 1,
        };
        let text = alloc::format!("{}", Json(&entry));
        assert!(
            text.starts_with(r#"{"service":"kernel32.dll!Foo","what":"missing export""#),
            "{text}"
        );
    }

    #[test]
    fn class_arguments() {
        assert_eq!(class_argument("NtQuerySystemInformation"), Some(0));
        assert_eq!(class_argument("NtQueryInformationProcess"), Some(1));
        assert_eq!(class_argument("NtClose"), None);
    }
}
