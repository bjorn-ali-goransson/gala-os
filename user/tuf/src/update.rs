//! The update client's whole flow (the specification's section 5) over a
//! feed and a store the caller provides: gala's updater gives a feed over
//! HTTPS and a store on its disk, the tests a map of each.
//!
//! 1. Root: the store's (else the one shipped in the image), then each
//!    `<N+1>.root.json` the feed has; an expired last root stops here.
//! 2. Timestamp, snapshot, targets, each checked against what the store
//!    trusts (loaded first, without expiry: what was trusted stays the
//!    floor for rollbacks) and against its signed parent; each one taken
//!    is saved at once.
//! 3. The release's files the store lacks, by content (SHA-256), each
//!    checked against the signed targets before it is kept.
//!
//! Nothing a check refuses is saved. Every step is an `Event` for the
//! caller's log (gala's event stream).

use crate::{Client, Refusal, Release, Updated};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// Largest metadata file fetched whose length nothing signed gives
/// (root, timestamp): TUF's own files are a few KiB.
pub const MAX_METADATA: u64 = 512 * 1024;

/// Where the feed's files come from (a bucket, a CDN, a mirror; none of
/// them trusted).
pub trait Remote {
    /// The file at `path` (relative to the feed's root), at most `max`
    /// bytes; None if the feed has no such file. A longer file is
    /// refused (the caller stops reading at `max` + 1).
    fn fetch(&mut self, path: &str, max: u64) -> Option<Vec<u8>>;
}

/// What the client keeps between runs.
pub trait Store {
    /// The trusted metadata of `role` (root, timestamp, snapshot, targets).
    fn load(&self, role: &str) -> Option<Vec<u8>>;
    fn save(&mut self, role: &str, bytes: &[u8]) -> core::result::Result<(), String>;
    /// Whether the file whose SHA-256 is `sha256` is kept already.
    fn has_blob(&self, sha256: &[u8; 32]) -> bool;
    fn put_blob(&mut self, sha256: &[u8; 32], bytes: &[u8]) -> core::result::Result<(), String>;
}

/// One step, for the caller's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Root version N taken (from the feed).
    Root { version: u64 },
    /// A role checked: newer and taken, or the same as trusted.
    Role { role: &'static str, version: u64, newer: bool },
    /// A metadata file the store held was not valid any more (and is
    /// fetched again).
    Dropped { role: &'static str, refusal: Refusal },
    /// A file of the release fetched and checked.
    Fetched { path: String, bytes: u64 },
    /// A file of the release the store holds already.
    Kept { path: String },
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Event::Root { version } => write!(f, "root {version} taken"),
            Event::Role { role, version, newer: true } => write!(f, "{role} {version} taken"),
            Event::Role { role, version, newer: false } => write!(f, "{role} {version} unchanged"),
            Event::Dropped { role, refusal } => write!(f, "{role} in the store dropped: {refusal}"),
            Event::Fetched { path, bytes } => {
                write!(f, "{path}: {bytes} bytes fetched and checked")
            }
            Event::Kept { path } => write!(f, "{path}: kept already"),
        }
    }
}

/// Why an update stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A file failed a check (the feed is not trusted for it).
    Refused { file: String, refusal: Refusal },
    /// The feed lacks a file the signed metadata names.
    Missing { file: String },
    /// The store could not keep something.
    Store(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Refused { file, refusal } => write!(f, "{file}: refused: {refusal}"),
            Error::Missing { file } => write!(f, "{file}: not in the feed"),
            Error::Store(e) => write!(f, "store: {e}"),
        }
    }
}

/// What an update found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The release the signed targets describe, all of its files kept.
    pub release: Release,
    /// Whether the targets were newer than the ones trusted before.
    pub newer: bool,
    /// Files fetched this time (the rest were kept already).
    pub fetched: usize,
    /// The release's files and their total size.
    pub files: usize,
    pub bytes: u64,
    /// The versions trusted now: root, timestamp, snapshot, targets.
    pub versions: [u64; 4],
}

fn refused(file: &str, refusal: Refusal) -> Error {
    Error::Refused { file: String::from(file), refusal }
}

fn fetch(remote: &mut dyn Remote, file: &str, max: u64) -> Result<Vec<u8>, Error> {
    remote.fetch(file, max).ok_or_else(|| Error::Missing { file: String::from(file) })
}

/// Runs one update at time `now` (seconds since 1970), starting from
/// `shipped_root` when the store has no root.
pub fn update(
    remote: &mut dyn Remote,
    store: &mut dyn Store,
    shipped_root: &[u8],
    now: i64,
    log: &mut dyn FnMut(&Event),
) -> Result<Outcome, Error> {
    let save = |store: &mut dyn Store, role: &str, bytes: &[u8]| {
        store.save(role, bytes).map_err(Error::Store)
    };
    // 1. Root.
    let stored_root = store.load("root");
    let (root_file, root_bytes) = match &stored_root {
        Some(b) => ("the stored root", b.as_slice()),
        None => ("the shipped root", shipped_root),
    };
    let mut client = Client::new(root_bytes).map_err(|r| refused(root_file, r))?;
    loop {
        let file = format!("{}.root.json", client.root.version + 1);
        let Some(bytes) = remote.fetch(&file, MAX_METADATA) else { break };
        client.update_root(&bytes).map_err(|r| refused(&file, r))?;
        save(store, "root", &bytes)?;
        log(&Event::Root { version: client.root.version });
    }
    client.check_root(now).map_err(|r| refused("root", r))?;

    // The store's trusted metadata, the floor for rollbacks. One that no
    // longer checks out (a rotated key, a new root's role change) is
    // dropped and fetched again.
    for (role, step) in [
        (
            "timestamp",
            Client::update_timestamp
                as fn(&mut Client, &[u8], Option<i64>) -> crate::Result<Updated>,
        ),
        ("snapshot", Client::update_snapshot),
        ("targets", Client::update_targets),
    ] {
        if let Some(bytes) = store.load(role)
            && let Err(refusal) = step(&mut client, &bytes, None)
        {
            log(&Event::Dropped { role, refusal });
        }
    }

    // 2. Timestamp, snapshot, targets.
    let ts_bytes = fetch(remote, "timestamp.json", MAX_METADATA)?;
    let ts =
        client.update_timestamp(&ts_bytes, Some(now)).map_err(|r| refused("timestamp.json", r))?;
    let ts_version = client.timestamp.as_ref().map_or(0, |t| t.version);
    log(&Event::Role { role: "timestamp", version: ts_version, newer: ts == Updated::Newer });
    if ts == Updated::Newer {
        save(store, "timestamp", &ts_bytes)?;
    }
    let listed = client.timestamp.as_ref().map(|t| t.snapshot);
    let have = client.snapshot.as_ref().map(|s| s.version);
    if let Some(listed) = listed
        && have != Some(listed.version)
    {
        let file = format!("{}.snapshot.json", listed.version);
        let bytes = fetch(remote, &file, listed.length)?;
        client.update_snapshot(&bytes, Some(now)).map_err(|r| refused(&file, r))?;
        save(store, "snapshot", &bytes)?;
        log(&Event::Role { role: "snapshot", version: listed.version, newer: true });
    }
    let listed = client.snapshot.as_ref().map(|s| s.targets);
    let had = client.targets.as_ref().map(|t| t.version);
    let mut newer = false;
    if let Some(listed) = listed
        && had != Some(listed.version)
    {
        let file = format!("{}.targets.json", listed.version);
        let bytes = fetch(remote, &file, listed.length)?;
        client.update_targets(&bytes, Some(now)).map_err(|r| refused(&file, r))?;
        save(store, "targets", &bytes)?;
        newer = true;
        log(&Event::Role { role: "targets", version: listed.version, newer: true });
    }
    let targets =
        client.targets.as_ref().ok_or_else(|| Error::Missing { file: String::from("targets") })?;
    // Trusted metadata still has to be current: a stored snapshot or
    // targets past its expiry is refused even when unchanged.
    crate::not_expired("snapshot", client.snapshot.as_ref().map_or(0, |s| s.expires), Some(now))
        .map_err(|r| refused("snapshot", r))?;
    crate::not_expired("targets", targets.expires, Some(now)).map_err(|r| refused("targets", r))?;

    // 3. The files the store lacks.
    let mut fetched = 0;
    for (path, target) in &targets.files {
        if store.has_blob(&target.sha256) {
            log(&Event::Kept { path: path.clone() });
            continue;
        }
        let file = target.blob_path();
        let bytes = fetch(remote, &file, target.length)?;
        client.check_target(path, &bytes).map_err(|r| refused(&file, r))?;
        store.put_blob(&target.sha256, &bytes).map_err(Error::Store)?;
        fetched += 1;
        log(&Event::Fetched { path: path.clone(), bytes: target.length });
    }
    let versions = [
        client.root.version,
        client.timestamp.as_ref().map_or(0, |t| t.version),
        client.snapshot.as_ref().map_or(0, |s| s.version),
        targets.version,
    ];
    let bytes = targets.files.values().map(|t| t.length).sum();
    Ok(Outcome {
        release: targets.release,
        newer,
        fetched,
        files: targets.files.len(),
        bytes,
        versions,
    })
}
