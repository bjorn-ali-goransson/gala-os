//! The missing-features ledger: counts every request gala does not
//! support (unknown services, unknown information classes, unsupported
//! flags, known TODO paths), so that running a real program yields a work
//! list ranked by how often each gap was hit (docs/OBSERVABILITY.md).
//!
//! Entries live in a fixed table (no allocation, usable anywhere); when it
//! is full, further new kinds are only counted in `overflow`. Dumped as
//! JSON lines (`ledger: {...}`) by `dump`, which also clears the table so
//! that each dump covers one program.
//!
//! Names from user mode are kept in a second fixed table. Invariants: a
//! dump gives back the names no entry uses any more (`compact`), so the
//! table holds only the names of the gaps since the last dump, never
//! every name of the boot; and bulk reports (the registry's absent keys,
//! thousands in a long install) take at most `BULK_NAMES` names and
//! `BULK_ENTRIES` entries, so a rarer gap (a missing DLL) still finds
//! room for its name between two dumps.

use core::fmt;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::sync::IrqMutex;

const ENTRIES: usize = 512;

/// Who reported the gap: a kernel service, or a name from user mode (gala's
/// own DLLs, through `record_user`), kept in a fixed table.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Service {
    Kernel(&'static str),
    User(usize),
}

const USER_NAMES: usize = 512;
const USER_NAME_LEN: usize = 96;
/// The most names and entries bulk reports (`record_user_bulk`) may hold;
/// the rest are kept for every other gap.
const BULK_NAMES: usize = 384;
const BULK_ENTRIES: usize = 384;

struct UserNames {
    names: [[u8; USER_NAME_LEN]; USER_NAMES],
    lens: [usize; USER_NAMES],
    /// Whether each name came from a bulk report.
    bulk: [bool; USER_NAMES],
    count: usize,
    /// How many of them came from bulk reports.
    bulk_count: usize,
}

impl UserNames {
    const fn new() -> Self {
        UserNames {
            names: [[0; USER_NAME_LEN]; USER_NAMES],
            lens: [0; USER_NAMES],
            bulk: [false; USER_NAMES],
            count: 0,
            bulk_count: 0,
        }
    }

    /// The index of `name`, adding it if there is room (for a bulk name,
    /// within `BULK_NAMES`).
    fn intern(&mut self, name: &[u8], bulk: bool) -> Option<usize> {
        let name = &name[..name.len().min(USER_NAME_LEN)];
        if let Some(i) = (0..self.count).find(|&i| &self.names[i][..self.lens[i]] == name) {
            return Some(i);
        }
        let i = self.count;
        if i == USER_NAMES || (bulk && self.bulk_count == BULK_NAMES) {
            return None;
        }
        self.names[i][..name.len()].copy_from_slice(name);
        self.lens[i] = name.len();
        self.bulk[i] = bulk;
        self.count += 1;
        self.bulk_count += usize::from(bulk);
        Some(i)
    }
}

static USER: IrqMutex<UserNames> = IrqMutex::new(UserNames::new());

/// The index of `name` in the user-name table, adding it if there is room.
fn intern(name: &[u8], bulk: bool) -> Option<usize> {
    USER.lock().intern(name, bulk)
}

/// Keeps only the names `entries` use, renumbering them.
fn compact(entries: &mut [Option<Entry>], t: &mut UserNames) {
    let mut used = [false; USER_NAMES];
    for entry in entries.iter().flatten() {
        if let Service::User(i) = entry.key.service {
            used[i] = true;
        }
    }
    let mut new_index = [0usize; USER_NAMES];
    let (mut kept, mut bulk) = (0, 0);
    for i in 0..t.count {
        if !used[i] {
            continue;
        }
        t.names[kept] = t.names[i];
        t.lens[kept] = t.lens[i];
        t.bulk[kept] = t.bulk[i];
        bulk += usize::from(t.bulk[i]);
        new_index[i] = kept;
        kept += 1;
    }
    t.count = kept;
    t.bulk_count = bulk;
    for entry in entries.iter_mut().flatten() {
        if let Service::User(i) = entry.key.service {
            entry.key.service = Service::User(new_index[i]);
        }
    }
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
    /// How many entries bulk reports hold.
    bulk: usize,
}

impl Ledger {
    /// Counts one hit of `key`, adding it if there is room (for a bulk
    /// report, within `BULK_ENTRIES`); otherwise only `overflow` counts it.
    fn add(&mut self, key: Key, bulk: bool) {
        if let Some(entry) = self.entries.iter_mut().flatten().find(|e| e.key == key) {
            entry.hits += 1;
        } else if bulk && self.bulk == BULK_ENTRIES {
            self.overflow += 1;
        } else if let Some(slot) = self.entries.iter_mut().find(|e| e.is_none()) {
            *slot = Some(Entry { key, hits: 1 });
            self.bulk += usize::from(bulk);
        } else {
            self.overflow += 1;
        }
    }
}

static LEDGER: IrqMutex<Ledger> =
    IrqMutex::new(Ledger { entries: [None; ENTRIES], overflow: 0, bulk: 0 });

/// Dumps printing their copy of the table (whose user names must keep
/// their indices until they are done).
static PRINTING: AtomicUsize = AtomicUsize::new(0);

/// Counts one hit of a missing feature.
pub fn record(service: &'static str, what: &'static str, detail: u64) {
    add(Key { service: Service::Kernel(service), what, detail }, false);
}

/// What a user-mode report is about (`GalaRecordMissing`'s code).
pub fn user_what(code: u32) -> &'static str {
    match code {
        1 => "missing export",
        2 => "unsupported",
        3 => "not implemented",
        4 => "missing dll",
        _ => "other",
    }
}

/// Counts a gap reported by gala's user-mode DLLs (names like
/// `kernel32.dll!CreateFileW`; printable ASCII only).
pub fn record_user(name: &[u8], what: &'static str, detail: u64) {
    record_named(name, what, detail, false);
}

/// `record_user` for reports that come in thousands (the registry's
/// absent keys): they take at most `BULK_NAMES` names and `BULK_ENTRIES`
/// entries, beyond which they count as `?user` / `ledger full`.
pub fn record_user_bulk(name: &[u8], what: &'static str, detail: u64) {
    record_named(name, what, detail, true);
}

fn record_named(name: &[u8], what: &'static str, detail: u64, bulk: bool) {
    let clean = name.iter().all(|&b| (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\');
    match if clean { intern(name, bulk) } else { None } {
        Some(i) => add(Key { service: Service::User(i), what, detail }, bulk),
        None => add(Key { service: Service::Kernel("?user"), what, detail }, bulk),
    }
}

fn add(key: Key, bulk: bool) {
    LEDGER.lock().add(key, bulk);
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
    PRINTING.fetch_add(1, Ordering::SeqCst);
    let (entries, overflow) = {
        let mut ledger = LEDGER.lock();
        let snapshot = (ledger.entries, ledger.overflow);
        ledger.entries = [None; ENTRIES];
        ledger.overflow = 0;
        ledger.bulk = 0;
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
    // The last dump out gives back the names only its copy used. A dump
    // that starts later takes its copy under LEDGER after this, so it
    // sees the new indices; one that started earlier keeps PRINTING up.
    if PRINTING.fetch_sub(1, Ordering::SeqCst) == 1 {
        let mut ledger = LEDGER.lock();
        let mut names = USER.lock();
        if PRINTING.load(Ordering::SeqCst) == 0 {
            compact(&mut ledger.entries, &mut names);
        }
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
        let a = intern(b"kernel32.dll!Foo", false).unwrap();
        assert_eq!(intern(b"kernel32.dll!Foo", false), Some(a));
        assert_ne!(intern(b"kernel32.dll!Bar", false), Some(a));
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

    fn name(t: &UserNames, i: usize) -> &[u8] {
        &t.names[i][..t.lens[i]]
    }

    /// Bulk names stop at BULK_NAMES and leave the rest for other gaps; a
    /// compaction keeps only the names entries use, renumbered.
    #[test]
    fn bulk_names_leave_room_and_compaction_frees() {
        let mut t = alloc::boxed::Box::new(UserNames::new());
        for i in 0..BULK_NAMES {
            let key = alloc::format!("registry:k{i}");
            assert!(t.intern(key.as_bytes(), true).is_some());
        }
        assert_eq!(t.intern(b"registry:one more", true), None);
        let dll = t.intern(b"osppc.dll", false).unwrap();
        assert_eq!(dll, BULK_NAMES);
        let mut entries = [None; 4];
        entries[0] = Some(Entry {
            key: Key { service: Service::User(dll), what: "missing dll", detail: 0 },
            hits: 2,
        });
        entries[1] = Some(Entry {
            key: Key { service: Service::User(7), what: "absent key", detail: 0 },
            hits: 1,
        });
        compact(&mut entries, &mut t);
        assert_eq!((t.count, t.bulk_count), (2, 1));
        assert_eq!(name(&t, 0), b"registry:k7");
        assert_eq!(name(&t, 1), b"osppc.dll");
        assert!(entries[0].unwrap().key.service == Service::User(1));
        assert!(entries[1].unwrap().key.service == Service::User(0));
        assert!(t.intern(b"registry:one more", true).is_some());
    }

    #[test]
    fn bulk_entries_leave_room() {
        let mut ledger =
            alloc::boxed::Box::new(Ledger { entries: [None; ENTRIES], overflow: 0, bulk: 0 });
        for i in 0..BULK_ENTRIES + 10 {
            ledger.add(
                Key { service: Service::Kernel("k"), what: "absent key", detail: i as u64 },
                true,
            );
        }
        assert_eq!((ledger.bulk, ledger.overflow), (BULK_ENTRIES, 10));
        ledger.add(Key { service: Service::Kernel("dll"), what: "missing dll", detail: 0 }, false);
        assert_eq!(ledger.entries.iter().flatten().count(), BULK_ENTRIES + 1);
    }

    #[test]
    fn class_arguments() {
        assert_eq!(class_argument("NtQuerySystemInformation"), Some(0));
        assert_eq!(class_argument("NtQueryInformationProcess"), Some(1));
        assert_eq!(class_argument("NtClose"), None);
    }
}
