//! The missing-features ledger: counts every request gala does not
//! support (unknown services, unknown information classes, unsupported
//! flags, known TODO paths), so that running a real program yields a work
//! list ranked by how often each gap was hit (docs/OBSERVABILITY.md).
//!
//! Entries live in a fixed table (no allocation, usable anywhere); when it
//! is full, further new kinds are only counted, by kind, in `dropped`, so
//! that a dump still says what it lost (a `?full` or `?bulk` line per
//! kind). Dumped as JSON lines (`ledger: {...}`) by `dump`, which also
//! clears the table so that each dump covers one program.
//!
//! Names from user mode are kept in a second fixed table. Invariants: a
//! dump gives back the names no entry uses any more (`compact`), so the
//! table holds only the names of the gaps since the last dump, never
//! every name of the boot; and bulk reports (the registry's absent keys,
//! thousands in a long install) take at most `BULK_NAMES` names and
//! `BULK_ENTRIES` entries, so a rarer gap (a missing DLL) still finds
//! room for its name between two dumps. Whether those caps are right is
//! answered by the dump itself: `?bulk` lines are the caps working,
//! while a `?full` line of a rarer kind says a gap was lost.

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
pub(super) const USER_NAME_LEN: usize = 96;
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
    /// When it was first and last hit (uptime in ms): a gap's count says
    /// how often, never when, and what explains a program's end is what
    /// it hit just before it. A dump's lines carry both.
    first: u64,
    last: u64,
}

/// The kinds a dump names the dropped records under. There are more
/// `what` literals than this; one that finds no slot is only counted in
/// `overflow`, which a dump reports as the kinds it could not name.
const DROPPED_KINDS: usize = 32;

/// What a table with no room left dropped, by kind: enough to tell a
/// gap that was lost (a missing export, say, which the next run needs
/// the caps raised to see) from the bulk reports whose cap is doing its
/// job.
#[derive(Clone, Copy)]
struct Dropped {
    what: &'static str,
    /// Whether the drops came from bulk reports (`record_user_bulk`,
    /// capped at `BULK_ENTRIES`) rather than from a table that is full.
    bulk: bool,
    hits: u64,
}

struct Ledger {
    entries: [Option<Entry>; ENTRIES],
    /// Every dropped record, counted by kind.
    dropped: [Option<Dropped>; DROPPED_KINDS],
    /// How many records were dropped in all (`dropped`'s hits plus the
    /// kinds it had no room for).
    overflow: u64,
    /// How many entries bulk reports hold.
    bulk: usize,
}

impl Ledger {
    /// Counts one hit of `key` at `at` (uptime in ms), adding it if
    /// there is room (for a bulk report, within `BULK_ENTRIES`);
    /// otherwise only `overflow` counts it.
    fn add(&mut self, key: Key, bulk: bool, at: u64) {
        if let Some(entry) = self.entries.iter_mut().flatten().find(|e| e.key == key) {
            entry.hits += 1;
            entry.last = at;
        } else if bulk && self.bulk == BULK_ENTRIES {
            self.drop_record(key.what, true);
        } else if let Some(slot) = self.entries.iter_mut().find(|e| e.is_none()) {
            *slot = Some(Entry { key, hits: 1, first: at, last: at });
            self.bulk += usize::from(bulk);
        } else {
            self.drop_record(key.what, bulk);
        }
    }

    /// Counts one record the table had no room for, under its kind.
    fn drop_record(&mut self, what: &'static str, bulk: bool) {
        self.overflow += 1;
        if let Some(d) =
            self.dropped.iter_mut().flatten().find(|d| d.what == what && d.bulk == bulk)
        {
            d.hits += 1;
        } else if let Some(slot) = self.dropped.iter_mut().find(|d| d.is_none()) {
            *slot = Some(Dropped { what, bulk, hits: 1 });
        }
    }
}

static LEDGER: IrqMutex<Ledger> = IrqMutex::new(Ledger {
    entries: [None; ENTRIES],
    dropped: [None; DROPPED_KINDS],
    overflow: 0,
    bulk: 0,
});

/// Dumps printing their copy of the table (whose user names must keep
/// their indices until they are done).
static PRINTING: AtomicUsize = AtomicUsize::new(0);

/// Counts one hit of a missing feature.
pub fn record(service: &'static str, what: &'static str, detail: u64) {
    add(Key { service: Service::Kernel(service), what, detail }, false);
}

/// What a user-mode report is about (`GalaRecordMissing`'s code). A
/// "stub" is an export that exists only to report itself and then ends
/// the process (`gala_stubs`); "not implemented" is a gap the caller is
/// told about and lives with (a method of an interface, a type the
/// reader cannot read), so only the first ends a program. "load failed"
/// is a DLL the loader found and could not load (its detail the status),
/// which is what names the DLL behind a process that dies
/// STATUS_DLL_INIT_FAILED. A "missing export" is one a program asked for
/// at run time and lives with (GetProcAddress); a "missing import" is one
/// the loader could not bind, which ends the load, and is named with the
/// module whose import it is (`<dll>!<export>(imported by <module>)`), so
/// that a program's probes and the gaps that stop it are told apart.
/// "reached" is not a gap at all: it is the first call that reached a
/// method gala answers, recorded so that a program which stops in the
/// middle of a path leaves the path it took (OBSERVABILITY
/// "Missing-features ledger").
pub fn user_what(code: u32) -> &'static str {
    match code {
        1 => "missing export",
        2 => "unsupported",
        3 => "not implemented",
        4 => "missing dll",
        6 => "stub",
        7 => "load failed",
        8 => "missing import",
        9 => "reached",
        // 5 and anything else: a gap a DLL reports without a kind of its
        // own (gala's DLLs pass 5).
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
/// entries, beyond which they count as `?user` / a `?bulk` line.
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
    LEDGER.lock().add(key, bulk, now_ms());
}

/// Uptime in ms, for when a gap was hit; the machine's clock as the rest
/// of nt reads it (host unit tests give `add` the time themselves).
fn now_ms() -> u64 {
    super::platform::monotonic_ns() / 1_000_000
}

/// One ledger entry as a JSON object.
struct Json<'a>(&'a Entry);

impl fmt::Display for Json<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Entry { key, hits, first, last } = self.0;
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
        write!(
            f,
            r#"","what":"{}","detail":{},"hits":{},"first":{first},"last":{last}}}"#,
            key.what, key.detail, hits
        )
    }
}

/// Prints every entry as a `ledger: {...}` line and clears the table.
pub fn dump() {
    PRINTING.fetch_add(1, Ordering::SeqCst);
    let (entries, dropped, overflow) = {
        let mut ledger = LEDGER.lock();
        let snapshot = (ledger.entries, ledger.dropped, ledger.overflow);
        ledger.entries = [None; ENTRIES];
        ledger.dropped = [None; DROPPED_KINDS];
        ledger.overflow = 0;
        ledger.bulk = 0;
        snapshot
    };
    for entry in entries.iter().flatten() {
        crate::kprintln!("ledger: {}", Json(entry));
    }
    // What was dropped, by kind, so that a lost gap is told from the
    // bulk reports the caps are there to drop: "?full" is a table with
    // no room left, "?bulk" the bulk cap, "?" the kinds `dropped` itself
    // had no room to name.
    let mut named = 0;
    for d in dropped.iter().flatten() {
        let service = if d.bulk { "?bulk" } else { "?full" };
        let (what, hits) = (d.what, d.hits);
        named += hits;
        crate::kprintln!(
            r#"ledger: {{"service":"{service}","what":"{what}","detail":0,"hits":{hits},"first":0,"last":0}}"#
        );
    }
    if overflow > named {
        let rest = overflow - named;
        crate::kprintln!(
            r#"ledger: {{"service":"?","what":"ledger full","detail":0,"hits":{rest},"first":0,"last":0}}"#
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

    /// Every hit moves the entry's last time; the first stays. A gap's
    /// count says how often, and these say when, which is what tells the
    /// gaps a program hit before it gave up from the thousands before.
    #[test]
    fn hits_keep_when_they_were_first_and_last() {
        let mut ledger = empty();
        let key = Key { service: Service::Kernel("NtX"), what: "info class", detail: 7 };
        ledger.add(key, false, 100);
        ledger.add(key, false, 2500);
        ledger.add(key, false, 2600);
        let entry = ledger.entries.iter().flatten().next().copied().unwrap();
        assert_eq!((entry.hits, entry.first, entry.last), (3, 100, 2600));
    }

    #[test]
    fn json_form() {
        let entry = Entry {
            key: Key { service: Service::Kernel("NtX"), what: "info class", detail: 7 },
            hits: 2,
            first: 11,
            last: 2200,
        };
        let text = alloc::format!("{}", Json(&entry));
        assert_eq!(
            text,
            r#"{"service":"NtX","what":"info class","detail":7,"hits":2,"first":11,"last":2200}"#
        );
    }

    #[test]
    fn user_names_are_interned() {
        let a = intern(b"kernel32.dll!Foo", false).unwrap();
        assert_eq!(intern(b"kernel32.dll!Foo", false), Some(a));
        assert_ne!(intern(b"kernel32.dll!Bar", false), Some(a));
        let entry = Entry {
            key: Key { service: Service::User(a), what: "missing export", detail: 0 },
            hits: 1,
            first: 0,
            last: 0,
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
            first: 0,
            last: 0,
        });
        entries[1] = Some(Entry {
            key: Key { service: Service::User(7), what: "absent key", detail: 0 },
            hits: 1,
            first: 0,
            last: 0,
        });
        compact(&mut entries, &mut t);
        assert_eq!((t.count, t.bulk_count), (2, 1));
        assert_eq!(name(&t, 0), b"registry:k7");
        assert_eq!(name(&t, 1), b"osppc.dll");
        assert!(entries[0].unwrap().key.service == Service::User(1));
        assert!(entries[1].unwrap().key.service == Service::User(0));
        assert!(t.intern(b"registry:one more", true).is_some());
    }

    fn empty() -> alloc::boxed::Box<Ledger> {
        alloc::boxed::Box::new(Ledger {
            entries: [None; ENTRIES],
            dropped: [None; DROPPED_KINDS],
            overflow: 0,
            bulk: 0,
        })
    }

    #[test]
    fn bulk_entries_leave_room() {
        let mut ledger = empty();
        for i in 0..BULK_ENTRIES + 10 {
            ledger.add(
                Key { service: Service::Kernel("k"), what: "absent key", detail: i as u64 },
                true,
                0,
            );
        }
        assert_eq!((ledger.bulk, ledger.overflow), (BULK_ENTRIES, 10));
        ledger.add(
            Key { service: Service::Kernel("dll"), what: "missing dll", detail: 0 },
            false,
            0,
        );
        assert_eq!(ledger.entries.iter().flatten().count(), BULK_ENTRIES + 1);
    }

    /// A dropped record is still counted under its kind, and bulk drops
    /// are told apart from a table with no room left, so that a dump says
    /// what it lost.
    #[test]
    fn drops_are_counted_by_kind() {
        let mut ledger = empty();
        for i in 0..BULK_ENTRIES + 3 {
            ledger.add(
                Key { service: Service::Kernel("k"), what: "absent key", detail: i as u64 },
                true,
                0,
            );
        }
        // The bulk reports hold BULK_ENTRIES entries; these fill the rest.
        for i in 0..ENTRIES - BULK_ENTRIES {
            ledger.add(
                Key { service: Service::Kernel("f"), what: "flag", detail: i as u64 },
                false,
                0,
            );
        }
        // The table is full now, so these two find no room of their own.
        ledger.add(
            Key { service: Service::Kernel("x"), what: "missing export", detail: 1 },
            false,
            0,
        );
        ledger.add(
            Key { service: Service::Kernel("y"), what: "missing export", detail: 2 },
            false,
            0,
        );
        let dropped: alloc::vec::Vec<_> =
            ledger.dropped.iter().flatten().map(|d| (d.what, d.bulk, d.hits)).collect();
        assert_eq!(dropped, [("absent key", true, 3), ("missing export", false, 2)]);
        assert_eq!(ledger.overflow, 5);
    }

    /// A kind the `dropped` table has no room for is still counted in the
    /// total, which is what a dump's "ledger full" line reports.
    #[test]
    fn kinds_beyond_the_table_still_count() {
        // A kind is matched by its text, so one letter each is enough.
        const KINDS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOP";
        assert!(KINDS.len() > DROPPED_KINDS);
        let mut ledger = empty();
        for i in 0..ENTRIES {
            ledger.add(
                Key { service: Service::Kernel("f"), what: "flag", detail: i as u64 },
                false,
                0,
            );
        }
        for i in 0..DROPPED_KINDS + 1 {
            let what: &'static str = &KINDS[i..i + 1];
            ledger.add(Key { service: Service::Kernel("k"), what, detail: 0 }, false, 0);
        }
        assert_eq!(ledger.dropped.iter().flatten().count(), DROPPED_KINDS);
        let named: u64 = ledger.dropped.iter().flatten().map(|d| d.hits).sum();
        assert_eq!((named, ledger.overflow), (DROPPED_KINDS as u64, DROPPED_KINDS as u64 + 1));
    }

    #[test]
    fn class_arguments() {
        assert_eq!(class_argument("NtQuerySystemInformation"), Some(0));
        assert_eq!(class_argument("NtQueryInformationProcess"), Some(1));
        assert_eq!(class_argument("NtClose"), None);
    }
}
