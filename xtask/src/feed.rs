//! `cargo xtask feed`: the update feed (R8 step 4; ARCHITECTURE "The update
//! feed"), TUF metadata over content-addressed files, written to a
//! directory that is uploaded as it is to object storage:
//! - `publish <files> <feed>`: a release of every file under `<files>`;
//! - `refresh <feed>`: snapshot and timestamp re-signed with nothing new
//!   (on a schedule, so clients tell a frozen feed from a quiet one);
//! - `check <feed> [--root <trusted root.json>]`: the feed checked the way
//!   gala's client checks it (user/tuf's `Client`), every file included.
//!
//! The keys are test keys made on first use in `target/trust/feed-*.seed`
//! (a stopgap tracked in ROADMAP R8: the root keys belong offline, the
//! targets key in a KMS or HSM).

use crate::{Result, trust};
use ed25519_compact::KeyPair;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tuf::{Client, MetaFile, Release, RoleKeys, Target};

const DAY: i64 = 86400;
/// How long each role's metadata stays valid. Timestamp and snapshot are
/// signed with online keys and re-signed by `refresh`; targets at every
/// release. Root is short-lived too (ROADMAP R8: every key is in CI for
/// now, and a short root lets a later move of root offline reach every
/// client within weeks); CI re-signs it monthly.
const ROOT_DAYS: i64 = 90;
const TARGETS_DAYS: i64 = 30;
const SNAPSHOT_DAYS: i64 = 7;
const TIMESTAMP_DAYS: i64 = 1;
/// Root keys and how many must sign root.
const ROOT_KEYS: usize = 3;
const ROOT_THRESHOLD: u64 = 2;

pub fn cli(args: &[String]) -> Result {
    let usage = "usage: cargo xtask feed publish <files dir> <feed dir> [--release N] [--floor F]\n\
                 \x20      cargo xtask feed refresh <feed dir>\n\
                 \x20      cargo xtask feed check <feed dir> [--root <trusted root.json>]";
    let pos: Vec<&str> = positional(args);
    let flag = |name: &str| -> Result<Option<u64>> {
        match args.iter().position(|a| a == name) {
            None => Ok(None),
            Some(i) => args
                .get(i + 1)
                .and_then(|v| v.parse().ok())
                .map(Some)
                .ok_or_else(|| format!("{name} takes a number")),
        }
    };
    match pos.as_slice() {
        ["publish", files, feed] => {
            publish(Path::new(files), Path::new(feed), flag("--release")?, flag("--floor")?, now())
        }
        ["refresh", feed] => refresh(Path::new(feed), now()),
        ["check", feed] => {
            let root = args.iter().position(|a| a == "--root").and_then(|i| args.get(i + 1));
            let report = check(Path::new(feed), root.map(Path::new), now())?;
            print!("{report}");
            Ok(())
        }
        _ => Err(usage.into()),
    }
}

/// The arguments that are not flags or flag values.
fn positional(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
        } else if a.starts_with("--") {
            skip = true;
        } else {
            out.push(a.as_str());
        }
    }
    out
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

struct Keys {
    root: Vec<KeyPair>,
    targets: KeyPair,
    snapshot: KeyPair,
    timestamp: KeyPair,
}

fn keys() -> Result<Keys> {
    let root = (1..=ROOT_KEYS)
        .map(|n| trust::named_key(&format!("feed-root-{n}")))
        .collect::<Result<_>>()?;
    Ok(Keys {
        root,
        targets: trust::named_key("feed-targets")?,
        snapshot: trust::named_key("feed-snapshot")?,
        timestamp: trust::named_key("feed-timestamp")?,
    })
}

fn read(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))
}

/// Writes `bytes` to `path` whole: a reader (or an upload of the
/// directory) never sees half a file.
fn write(path: &Path, bytes: &[u8]) -> Result {
    let tmp = path.with_extension("partial");
    fs::write(&tmp, bytes).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("renaming {}: {e}", tmp.display()))
}

/// The feed's trusted state, as a client that starts from its first root
/// and takes every later one sees it (from the feed's own files, so
/// expiry is not checked: a feed being refreshed may have expired).
fn current(feed: &Path) -> Result<Option<Client>> {
    let first = feed.join("1.root.json");
    if !first.exists() {
        return Ok(None);
    }
    let mut c = Client::new(&read(&first)?).map_err(|r| format!("{}: {r}", first.display()))?;
    roots(feed, &mut c)?;
    let ts = feed.join("timestamp.json");
    if ts.exists() {
        c.update_timestamp(&read(&ts)?, None).map_err(|r| format!("{}: {r}", ts.display()))?;
        let v = c.timestamp.as_ref().map_or(0, |t| t.snapshot.version);
        let snap = feed.join(format!("{v}.snapshot.json"));
        c.update_snapshot(&read(&snap)?, None).map_err(|r| format!("{}: {r}", snap.display()))?;
        let v = c.snapshot.as_ref().map_or(0, |s| s.targets.version);
        let targets = feed.join(format!("{v}.targets.json"));
        c.update_targets(&read(&targets)?, None)
            .map_err(|r| format!("{}: {r}", targets.display()))?;
    }
    Ok(Some(c))
}

/// Takes `<N>.root.json` for every N after the trusted root's.
fn roots(feed: &Path, c: &mut Client) -> Result {
    loop {
        let next = feed.join(format!("{}.root.json", c.root.version + 1));
        if !next.exists() {
            return Ok(());
        }
        c.update_root(&read(&next)?).map_err(|r| format!("{}: {r}", next.display()))?;
    }
}

/// Every file under `dir`, by its path relative to it (`/`-separated),
/// sorted.
fn files(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in fs::read_dir(&d).map_err(|e| format!("reading {}: {e}", d.display()))? {
            let p = e.map_err(|e| e.to_string())?.path();
            if p.is_dir() {
                todo.push(p);
            } else {
                let rel = p.strip_prefix(dir).map_err(|e| e.to_string())?;
                let name = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push((name, p));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Signs and writes snapshot and timestamp over `targets` (bytes of
/// version `targets_version`); timestamp last, as a client reads it first.
fn sign_snapshot_and_timestamp(
    feed: &Path,
    k: &Keys,
    old: Option<&Client>,
    targets: MetaFile,
    now: i64,
) -> Result {
    let snap_v = old.and_then(|c| c.snapshot.as_ref()).map_or(0, |s| s.version) + 1;
    let snap =
        tuf::sign(&tuf::snapshot(snap_v, now + SNAPSHOT_DAYS * DAY, &targets), &[&k.snapshot]);
    write(&feed.join(format!("{snap_v}.snapshot.json")), &snap)?;
    let ts_v = old.and_then(|c| c.timestamp.as_ref()).map_or(0, |t| t.version) + 1;
    let ts = tuf::sign(
        &tuf::timestamp(ts_v, now + TIMESTAMP_DAYS * DAY, &MetaFile::of(snap_v, &snap)),
        &[&k.timestamp],
    );
    write(&feed.join("timestamp.json"), &ts)?;
    eprintln!("xtask: feed: snapshot {snap_v}, timestamp {ts_v}");
    Ok(())
}

pub fn publish(
    files_dir: &Path,
    feed: &Path,
    release: Option<u64>,
    floor: Option<u64>,
    now: i64,
) -> Result {
    let k = keys()?;
    fs::create_dir_all(feed.join("blobs/sha256"))
        .map_err(|e| format!("creating {}: {e}", feed.display()))?;
    let old = current(feed)?;
    if old.is_none() {
        let root_pks: Vec<_> = k.root.iter().map(|k| k.pk).collect();
        let signed = tuf::root(
            1,
            now + ROOT_DAYS * DAY,
            &[
                RoleKeys { name: "root", keys: &root_pks, threshold: ROOT_THRESHOLD },
                RoleKeys { name: "targets", keys: &[k.targets.pk], threshold: 1 },
                RoleKeys { name: "snapshot", keys: &[k.snapshot.pk], threshold: 1 },
                RoleKeys { name: "timestamp", keys: &[k.timestamp.pk], threshold: 1 },
            ],
        );
        let all: Vec<&KeyPair> = k.root.iter().collect();
        write(&feed.join("1.root.json"), &tuf::sign(&signed, &all))?;
        eprintln!("xtask: feed: root 1 ({ROOT_THRESHOLD} of {ROOT_KEYS} root keys)");
    }
    let prev = old.as_ref().and_then(|c| c.targets.as_ref());
    let number = release.unwrap_or(prev.map_or(1, |t| t.release.number + 1));
    let floor = floor.unwrap_or(prev.map_or(1, |t| t.release.floor));
    if let Some(p) = prev
        && (number <= p.release.number || floor < p.release.floor)
    {
        return Err(format!(
            "release {number} (floor {floor}) does not follow release {} (floor {})",
            p.release.number, p.release.floor
        ));
    }
    let mut list = Vec::new();
    let mut new_blobs = 0;
    for (name, path) in files(files_dir)? {
        let bytes = read(&path)?;
        let t = Target::of(&bytes);
        let blob = feed.join(t.blob_path());
        match fs::read(&blob) {
            // Content-addressed: one already there must be these bytes.
            Ok(have) if Target::of(&have) == t => {}
            Ok(_) => {
                return Err(format!("{}: not the file its name is the digest of", blob.display()));
            }
            Err(_) => {
                write(&blob, &bytes)?;
                new_blobs += 1;
            }
        }
        list.push((name, t));
    }
    let refs: Vec<(&str, Target)> = list.iter().map(|(n, t)| (n.as_str(), t.clone())).collect();
    let targets_v = prev.map_or(0, |t| t.version) + 1;
    let release = Release { number, floor };
    let targets = tuf::sign(
        &tuf::targets(targets_v, now + TARGETS_DAYS * DAY, release, &refs),
        &[&k.targets],
    );
    write(&feed.join(format!("{targets_v}.targets.json")), &targets)?;
    eprintln!(
        "xtask: feed: release {number} (floor {floor}) as targets {targets_v}: {} files, {new_blobs} new blobs",
        list.len()
    );
    sign_snapshot_and_timestamp(feed, &k, old.as_ref(), MetaFile::of(targets_v, &targets), now)
}

pub fn refresh(feed: &Path, now: i64) -> Result {
    let k = keys()?;
    let Some(c) = current(feed)? else {
        return Err(format!("{}: no feed (no 1.root.json)", feed.display()));
    };
    let Some(t) = &c.targets else {
        return Err(format!("{}: nothing published yet", feed.display()));
    };
    let path = feed.join(format!("{}.targets.json", t.version));
    let targets = MetaFile::of(t.version, &read(&path)?);
    sign_snapshot_and_timestamp(feed, &k, Some(&c), targets, now)
}

/// The feed's directory as the client's remote (what gala fetches over
/// HTTPS).
struct DirRemote<'a>(&'a Path);

impl tuf::update::Remote for DirRemote<'_> {
    fn fetch(&mut self, path: &str, max: u64) -> Option<Vec<u8>> {
        use std::io::Read;
        let mut bytes = Vec::new();
        let file = fs::File::open(self.0.join(path)).ok()?;
        file.take(max + 1).read_to_end(&mut bytes).ok()?;
        Some(bytes)
    }
}

/// A client's store that starts empty (a machine that never updated).
#[derive(Default)]
struct MemStore {
    meta: std::collections::BTreeMap<String, Vec<u8>>,
    blobs: std::collections::BTreeSet<[u8; 32]>,
}

impl tuf::update::Store for MemStore {
    fn load(&self, role: &str) -> Option<Vec<u8>> {
        self.meta.get(role).cloned()
    }
    fn save(&mut self, role: &str, bytes: &[u8]) -> std::result::Result<(), String> {
        self.meta.insert(role.to_string(), bytes.to_vec());
        Ok(())
    }
    fn has_blob(&self, sha256: &[u8; 32]) -> bool {
        self.blobs.contains(sha256)
    }
    fn put_blob(&mut self, sha256: &[u8; 32], _bytes: &[u8]) -> std::result::Result<(), String> {
        self.blobs.insert(*sha256);
        Ok(())
    }
}

/// Checks a feed as gala's client does (`tuf::update::update`, the same
/// code), from `root` (default: the feed's own first root) at time `now`,
/// as a machine that never updated: every file fetched and checked. A
/// summary, or the first refusal.
pub fn check(feed: &Path, root: Option<&Path>, now: i64) -> Result<String> {
    let root_path = root.map_or_else(|| feed.join("1.root.json"), Path::to_path_buf);
    let shipped = read(&root_path)?;
    let mut log = String::new();
    let outcome = tuf::update::update(
        &mut DirRemote(feed),
        &mut MemStore::default(),
        &shipped,
        now,
        &mut |e| {
            log += &format!("  {e}\n");
        },
    )
    .map_err(|e| format!("{}: {e}\n{log}", feed.display()))?;
    let [root, ts, snap, targets] = outcome.versions;
    Ok(format!(
        "feed {}: root {root}, timestamp {ts}, snapshot {snap}, targets {targets}: release {} (floor {}), \
         {} files, {} bytes, all match\n{log}",
        feed.display(),
        outcome.release.number,
        outcome.release.floor,
        outcome.files,
        outcome.bytes,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target_dir;

    fn scratch(name: &str) -> PathBuf {
        let d = target_dir().join("feed-tests").join(name);
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("files/boot")).unwrap();
        d
    }

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn publish_refresh_check() {
        let d = scratch("publish");
        let (files, feed) = (d.join("files"), d.join("feed"));
        fs::write(files.join("boot/kernel"), b"kernel 1").unwrap();
        fs::write(files.join("boot/gala.manifest"), b"manifest 1").unwrap();
        publish(&files, &feed, None, None, NOW).unwrap();
        let r = check(&feed, None, NOW).unwrap();
        assert!(
            r.contains("root 1, timestamp 1, snapshot 1, targets 1: release 1 (floor 1), 2 files"),
            "{r}"
        );

        // A day later the timestamp has expired: a frozen feed...
        let later = NOW + 2 * DAY;
        let e = check(&feed, None, later).unwrap_err();
        assert!(e.contains("timestamp.json: refused: timestamp: expired"), "{e}");
        // ...until refreshed.
        refresh(&feed, later).unwrap();
        let r = check(&feed, None, later).unwrap();
        assert!(r.contains("timestamp 2, snapshot 2, targets 1: release 1"), "{r}");

        // Release 2 shares the unchanged blob.
        fs::write(files.join("boot/kernel"), b"kernel 2").unwrap();
        publish(&files, &feed, None, Some(2), later).unwrap();
        let r = check(&feed, None, later).unwrap();
        assert!(r.contains("targets 2: release 2 (floor 2)"), "{r}");
        assert_eq!(fs::read_dir(feed.join("blobs/sha256")).unwrap().count(), 3);
        // Releases only move forward.
        let e = publish(&files, &feed, Some(2), None, later).unwrap_err();
        assert!(e.contains("does not follow release 2"), "{e}");

        // A tampered blob.
        let blob = feed.join(Target::of(b"kernel 2").blob_path());
        fs::write(&blob, b"kernel X").unwrap();
        let e = check(&feed, None, later).unwrap_err();
        assert!(e.contains("refused: boot/kernel: SHA-256"), "{e}");
    }

    #[test]
    fn positional_skips_flags() {
        let a: Vec<String> =
            ["publish", "--release", "3", "a", "b"].iter().map(|s| s.to_string()).collect();
        assert_eq!(positional(&a), ["publish", "a", "b"]);
    }
}
