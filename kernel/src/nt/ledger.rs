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

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    /// The service (or kernel area) name; "?" for an unknown service.
    service: &'static str,
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
    let key = Key { service, what, detail };
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
        // Names and kinds are kernel string literals without quotes or
        // backslashes, so they need no escaping.
        write!(
            f,
            r#"{{"service":"{}","what":"{}","detail":{},"hits":{}}}"#,
            key.service, key.what, key.detail, hits
        )
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
        let entry = Entry { key: Key { service: "NtX", what: "info class", detail: 7 }, hits: 2 };
        let text = alloc::format!("{}", Json(&entry));
        assert_eq!(text, r#"{"service":"NtX","what":"info class","detail":7,"hits":2}"#);
    }

    #[test]
    fn class_arguments() {
        assert_eq!(class_argument("NtQuerySystemInformation"), Some(0));
        assert_eq!(class_argument("NtQueryInformationProcess"), Some(1));
        assert_eq!(class_argument("NtClose"), None);
    }
}
