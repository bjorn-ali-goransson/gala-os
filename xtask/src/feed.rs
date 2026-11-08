//! `cargo xtask feed`: the update feed (R8 step 4; ARCHITECTURE "The update
//! feed"), TUF metadata over content-addressed files, written to a
//! directory and uploaded from it to object storage:
//! - `publish <files> <feed>`: a release of every file under `<files>`;
//! - `refresh <feed>`: snapshot and timestamp re-signed with nothing new
//!   (on a schedule, so clients tell a frozen feed from a quiet one), and
//!   targets re-signed when it nears its expiry;
//! - `renew-root <feed>`: root N+1 with the same keys and a fresh expiry;
//! - `check <feed> [--over <source>] [--root <trusted root.json>]`: the
//!   feed checked the way gala's client checks it (user/tuf's `update`),
//!   every file included;
//! - `pull <source> <dir>`: the metadata a client follows, from a bucket's
//!   public URL (or a directory), verified, into an empty directory;
//! - `push <feed> <dest>`: what changed since the pull, to `s3://<bucket>`
//!   (the `aws` CLI) or a directory, in an order no client can be caught
//!   by: blobs, roots, targets, snapshot, timestamp last;
//! - `keygen`: a fresh `GALA_FEED_KEYS` value.
//!
//! The keys come from `GALA_FEED_KEYS` (CI's secret: every role's Ed25519
//! seed, root included, ROADMAP R8), else test keys made on first use in
//! `target/trust/feed-*.seed`.

use crate::{Result, trust};
use ed25519_compact::{KeyPair, Seed};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use tuf::json::Value;
use tuf::update::MAX_METADATA;
use tuf::{Client, MetaFile, Release, RoleKeys, Target, Time};

const DAY: i64 = 86400;
/// How long each role's metadata stays valid. Timestamp and snapshot are
/// signed with online keys and re-signed by `refresh`; targets at every
/// release (and by `refresh` when it nears its expiry: a quiet main must
/// not freeze the feed). Root is short-lived too (ROADMAP R8: every key is
/// in CI for now, and a short root lets a later move of root offline reach
/// every client within weeks); CI re-signs it monthly (`renew-root`).
const ROOT_DAYS: i64 = 90;
const TARGETS_DAYS: i64 = 30;
const SNAPSHOT_DAYS: i64 = 7;
const TIMESTAMP_DAYS: i64 = 1;
/// `refresh` re-signs targets that expire within this many days.
const TARGETS_RENEW_DAYS: i64 = 15;
/// A root that expires within this many days is reported as a warning:
/// the monthly renewal has not run (it renews at ~30 of 90 days).
const ROOT_WARN_DAYS: i64 = 45;
/// Root keys and how many must sign root.
const ROOT_KEYS: usize = 3;
const ROOT_THRESHOLD: u64 = 2;

/// The environment variable (CI's secret) with every role's seed.
const KEYS_VAR: &str = "GALA_FEED_KEYS";
/// The roles' names in `GALA_FEED_KEYS`, in the order `Keys` holds them.
const KEY_ROLES: [&str; ROOT_KEYS + 3] =
    ["root-1", "root-2", "root-3", "targets", "snapshot", "timestamp"];
/// What `push` and `pull` keep beside the feed: each metadata file pulled
/// and its SHA-256 (not part of the feed; never uploaded).
const PULLED: &str = ".pulled";

pub fn cli(args: &[String]) -> Result {
    let usage = "usage: cargo xtask feed publish <files dir> <feed dir> [--release N] [--floor F]\n\
                 \x20      cargo xtask feed refresh <feed dir>\n\
                 \x20      cargo xtask feed renew-root <feed dir>\n\
                 \x20      cargo xtask feed check <feed> [--over <source>] [--root <trusted root.json>]\n\
                 \x20      cargo xtask feed pull <source> <empty dir>\n\
                 \x20      cargo xtask feed push <feed dir> <s3://bucket[/prefix] | dir>\n\
                 \x20      cargo xtask feed keygen > <file>   (or | gh secret set GALA_FEED_KEYS)\n\
                 (a source is a directory or an http(s) URL)";
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
    let text_flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));
    match pos.as_slice() {
        ["publish", files, feed] => publish(
            &keys()?,
            Path::new(files),
            Path::new(feed),
            flag("--release")?,
            flag("--floor")?,
            now(),
        ),
        ["refresh", feed] => refresh(&keys()?, Path::new(feed), now()),
        ["renew-root", feed] => renew_root(&keys()?, Path::new(feed), now()),
        ["check", feed] => {
            let mut sources = vec![Source::new(feed)];
            sources.extend(text_flag("--over").map(|s| Source::new(s)));
            let report = check(&sources, text_flag("--root").map(Path::new), now())?;
            print!("{report}");
            Ok(())
        }
        ["pull", source, dir] => pull(&Source::new(source), Path::new(dir)),
        ["push", feed, dest] => push(Path::new(feed), &Dest::new(dest)),
        ["keygen"] => {
            let (value, ids) = keygen(&mut urandom)?;
            eprint!("{ids}");
            eprintln!(
                "xtask: feed: a new {KEYS_VAR} value is on standard output (nowhere else): put it \
                 in the GitHub secret {KEYS_VAR} (docs/WORKFLOW.md, \"The update feed\")"
            );
            print!("{value}");
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

/// One line of what the feed commands did: on standard error, and in the
/// GitHub Actions job summary when there is one.
fn report(line: &str) {
    eprintln!("xtask: feed: {line}");
    if cfg!(test) {
        return;
    }
    if let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        use std::io::Write;
        let appended = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .and_then(|mut f| writeln!(f, "- {line}"));
        if let Err(e) = appended {
            eprintln!("xtask: feed: writing the job summary {path:?}: {e}");
        }
    }
}

// ---- keys ----

struct Keys {
    root: Vec<KeyPair>,
    targets: KeyPair,
    snapshot: KeyPair,
    timestamp: KeyPair,
    /// Where they came from, for messages.
    source: &'static str,
}

impl Keys {
    /// Each role's key ids as root lists them.
    fn role_ids(&self) -> [(&'static str, BTreeSet<String>); 4] {
        let ids = |ks: &[&KeyPair]| -> BTreeSet<String> {
            ks.iter().map(|k| tuf::key_id(&k.pk)).collect()
        };
        [
            ("root", ids(&self.root.iter().collect::<Vec<_>>())),
            ("targets", ids(&[&self.targets])),
            ("snapshot", ids(&[&self.snapshot])),
            ("timestamp", ids(&[&self.timestamp])),
        ]
    }

    /// Root's `signed` part for version `version` over these keys.
    fn root_meta(&self, version: u64, expires: i64) -> Value {
        let root_pks: Vec<_> = self.root.iter().map(|k| k.pk).collect();
        tuf::root(
            version,
            expires,
            &[
                RoleKeys { name: "root", keys: &root_pks, threshold: ROOT_THRESHOLD },
                RoleKeys { name: "targets", keys: &[self.targets.pk], threshold: 1 },
                RoleKeys { name: "snapshot", keys: &[self.snapshot.pk], threshold: 1 },
                RoleKeys { name: "timestamp", keys: &[self.timestamp.pk], threshold: 1 },
            ],
        )
    }
}

/// The feed's keys: from `GALA_FEED_KEYS` when it is set (then never the
/// test keys), else the test keys.
fn keys() -> Result<Keys> {
    keys_from(std::env::var_os(KEYS_VAR))
}

fn keys_from(var: Option<OsString>) -> Result<Keys> {
    let Some(value) = var else {
        let root = (1..=ROOT_KEYS)
            .map(|n| trust::named_key(&format!("feed-root-{n}")))
            .collect::<Result<_>>()?;
        let k = Keys {
            root,
            targets: trust::named_key("feed-targets")?,
            snapshot: trust::named_key("feed-snapshot")?,
            timestamp: trust::named_key("feed-timestamp")?,
            source: "the test keys in target/trust",
        };
        report(&format!("keys: {} ({KEYS_VAR} is not set)", k.source));
        return Ok(k);
    };
    let text = value.into_string().map_err(|_| format!("{KEYS_VAR} is not UTF-8"))?;
    let k = parse_keys(&text)?;
    let ids: Vec<String> = k
        .role_ids()
        .iter()
        .map(|(role, ids)| {
            let short: Vec<&str> = ids.iter().map(|i| &i[..16]).collect();
            format!("{role} {}", short.join(" "))
        })
        .collect();
    report(&format!("keys: from {KEYS_VAR} (key ids {})", ids.join("; ")));
    Ok(k)
}

/// Parses `GALA_FEED_KEYS`: one `<role> <64 hex digits>` line per role
/// (KEY_ROLES), the hex being the role's Ed25519 seed; blank lines and
/// `#` comments allowed. No message shows the value or any part of it.
fn parse_keys(text: &str) -> Result<Keys> {
    if text.trim().is_empty() {
        return Err(format!(
            "{KEYS_VAR} is set but empty (a secret this job cannot see?); the test keys are \
             not used in its place"
        ));
    }
    let mut seeds: [Option<[u8; 32]>; KEY_ROLES.len()] = [None; KEY_ROLES.len()];
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let (Some(role), Some(seed), None) = (words.next(), words.next(), words.next()) else {
            return Err(format!(
                "{KEYS_VAR}: line {n} is not `<role> <64 hex digits>` (the line is not shown: \
                 it may hold a key)"
            ));
        };
        let Some(slot) = KEY_ROLES.iter().position(|r| *r == role) else {
            return Err(format!(
                "{KEYS_VAR}: line {n} names none of the roles {} (the line is not shown: it may \
                 hold a key)",
                KEY_ROLES.join(", ")
            ));
        };
        if seeds[slot].is_some() {
            return Err(format!("{KEYS_VAR}: role {role} is given twice (line {n})"));
        }
        let seed = tuf::unhex::<32>(seed).ok_or_else(|| {
            format!("{KEYS_VAR}: role {role}: its seed is not 64 hex digits (line {n}; not shown)")
        })?;
        seeds[slot] = Some(seed);
    }
    let missing: Vec<&str> =
        KEY_ROLES.iter().zip(&seeds).filter(|(_, s)| s.is_none()).map(|(r, _)| *r).collect();
    if !missing.is_empty() {
        return Err(format!("{KEYS_VAR}: no key for role {}", missing.join(", ")));
    }
    let mut pairs = Vec::new();
    for (role, seed) in KEY_ROLES.iter().zip(seeds.iter().flatten()) {
        if let Some(j) = seeds.iter().flatten().position(|s| s == seed)
            && KEY_ROLES[j] != *role
        {
            return Err(format!("{KEYS_VAR}: roles {} and {role} have the same key", KEY_ROLES[j]));
        }
        let pair = Seed::from_slice(seed)
            .and_then(KeyPair::try_from_seed)
            .map_err(|e| format!("{KEYS_VAR}: role {role}: {e}"))?;
        pairs.push(pair);
    }
    let mut it = pairs.into_iter();
    let mut next = || it.next().ok_or("KEY_ROLES and Keys differ");
    Ok(Keys {
        root: vec![next()?, next()?, next()?],
        targets: next()?,
        snapshot: next()?,
        timestamp: next()?,
        source: KEYS_VAR,
    })
}

/// 32 bytes from the system's random source.
fn urandom() -> Result<[u8; 32]> {
    let mut seed = [0u8; 32];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut seed))
        .map_err(|e| format!("reading /dev/urandom: {e}"))?;
    Ok(seed)
}

/// A new `GALA_FEED_KEYS` value (seeds from `random`) and, apart, each
/// role's public key id (what to tell the keys apart by later).
fn keygen(random: &mut dyn FnMut() -> Result<[u8; 32]>) -> Result<(String, String)> {
    let mut value = String::from(
        "# gala's update feed keys (cargo xtask feed keygen): <role> <Ed25519 seed>\n",
    );
    let mut ids = String::new();
    for role in KEY_ROLES {
        let seed = random()?;
        let pair = KeyPair::try_from_seed(Seed::new(seed)).map_err(|e| e.to_string())?;
        value += &format!("{role} {}\n", tuf::hex(&seed));
        ids += &format!("xtask: feed: {role}: key id {}\n", tuf::key_id(&pair.pk));
    }
    // What is printed must be what the feed commands take.
    parse_keys(&value)?;
    Ok((value, ids))
}

/// Refuses to sign a feed whose latest root lists other keys than `k`
/// (the wrong secret, or test keys against a real feed): what it signed
/// would be refused by every client.
fn check_keys(feed: &Path, c: &Client, k: &Keys) -> Result {
    let path = feed.join(format!("{}.root.json", c.root.version));
    let v = tuf::json::parse(&read(&path)?)
        .map_err(|e| format!("{}: not JSON ({e:?})", path.display()))?;
    for (role, ours) in k.role_ids() {
        let theirs: BTreeSet<String> = v
            .get("signed")
            .and_then(|s| s.get("roles"))
            .and_then(|r| r.get(role))
            .and_then(|r| r.get("keyids"))
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|id| id.as_str().map(String::from))
            .collect();
        if theirs != ours {
            let short = |s: &BTreeSet<String>| {
                s.iter().map(|i| i.get(..16).unwrap_or(i)).collect::<Vec<_>>().join(" ")
            };
            return Err(format!(
                "{}: root {} lists other {role} keys ({}) than {} ({}): the wrong keys for this \
                 feed; nothing signed",
                path.display(),
                c.root.version,
                short(&theirs),
                k.source,
                short(&ours),
            ));
        }
    }
    Ok(())
}

// ---- the feed's directory ----

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

/// The feed's state in one line: release, each role's version and expiry,
/// and a warning when root nears its expiry (the monthly renewal did not
/// run).
fn status(feed: &Path, now: i64) -> Result {
    let Some(c) = current(feed)? else { return Ok(()) };
    let mut line = format!("root {} (expires {})", c.root.version, Time(c.root.expires));
    if let (Some(t), Some(s), Some(ts)) = (&c.targets, &c.snapshot, &c.timestamp) {
        line = format!(
            "release {} (floor {}), {} files: targets {} (expires {}), snapshot {} (expires {}), \
             timestamp {} (expires {}), {line}",
            t.release.number,
            t.release.floor,
            t.files.len(),
            t.version,
            Time(t.expires),
            s.version,
            Time(s.expires),
            ts.version,
            Time(ts.expires),
        );
    }
    report(&line);
    let left = (c.root.expires - now) / DAY;
    if left < ROOT_WARN_DAYS {
        let warning = format!(
            "warning: root {} expires in {left} days ({}); renew-root runs monthly: has it \
             stopped?",
            c.root.version,
            Time(c.root.expires)
        );
        report(&warning);
        if std::env::var_os("GITHUB_ACTIONS").is_some() && !cfg!(test) {
            println!("::warning::{warning}");
        }
    }
    Ok(())
}

/// Signs and writes snapshot and timestamp over `targets`; timestamp
/// last, as a client reads it first.
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
    report(&format!(
        "signed snapshot {snap_v} (over targets {}), timestamp {ts_v}",
        targets.version
    ));
    Ok(())
}

// ---- the commands ----

fn publish(
    k: &Keys,
    files_dir: &Path,
    feed: &Path,
    release: Option<u64>,
    floor: Option<u64>,
    now: i64,
) -> Result {
    fs::create_dir_all(feed.join("blobs/sha256"))
        .map_err(|e| format!("creating {}: {e}", feed.display()))?;
    let old = current(feed)?;
    match &old {
        Some(c) => check_keys(feed, c, k)?,
        None => {
            let signed = k.root_meta(1, now + ROOT_DAYS * DAY);
            let all: Vec<&KeyPair> = k.root.iter().collect();
            write(&feed.join("1.root.json"), &tuf::sign(&signed, &all))?;
            report(&format!(
                "a new feed: root 1 ({ROOT_THRESHOLD} of {ROOT_KEYS} root keys, {})",
                k.source
            ));
        }
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
    let mut bytes_total = 0u64;
    for (name, path) in files(files_dir)? {
        let bytes = read(&path)?;
        let t = Target::of(&bytes);
        bytes_total += t.length;
        let blob = feed.join(t.blob_path());
        // Blobs are not pulled (content-addressed, they never change): one
        // absent here is written, and `push` uploads only those the
        // bucket lacks.
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
    if list.is_empty() {
        return Err(format!("{}: no files: an empty release is refused", files_dir.display()));
    }
    let refs: Vec<(&str, Target)> = list.iter().map(|(n, t)| (n.as_str(), t.clone())).collect();
    let targets_v = prev.map_or(0, |t| t.version) + 1;
    let release = Release { number, floor };
    let targets = tuf::sign(
        &tuf::targets(targets_v, now + TARGETS_DAYS * DAY, release, &refs),
        &[&k.targets],
    );
    write(&feed.join(format!("{targets_v}.targets.json")), &targets)?;
    report(&format!(
        "release {number} (floor {floor}) as targets {targets_v}: {} files, {bytes_total} bytes, \
         {new_blobs} blobs not in {} before",
        list.len(),
        feed.display()
    ));
    sign_snapshot_and_timestamp(feed, k, old.as_ref(), MetaFile::of(targets_v, &targets), now)?;
    status(feed, now)
}

fn refresh(k: &Keys, feed: &Path, now: i64) -> Result {
    let Some(c) = current(feed)? else {
        return Err(format!("{}: no feed (no 1.root.json)", feed.display()));
    };
    check_keys(feed, &c, k)?;
    let Some(t) = &c.targets else {
        return Err(format!("{}: nothing published yet", feed.display()));
    };
    let targets = if t.expires - now < TARGETS_RENEW_DAYS * DAY {
        // The same release, signed again as a new version: the feed stays
        // valid however long main is quiet.
        let v = t.version + 1;
        let files: Vec<(&str, Target)> =
            t.files.iter().map(|(n, t)| (n.as_str(), t.clone())).collect();
        let bytes =
            tuf::sign(&tuf::targets(v, now + TARGETS_DAYS * DAY, t.release, &files), &[&k.targets]);
        write(&feed.join(format!("{v}.targets.json")), &bytes)?;
        report(&format!(
            "targets {v}: release {} signed again (targets {} expires {}, within \
             {TARGETS_RENEW_DAYS} days)",
            t.release.number,
            t.version,
            Time(t.expires)
        ));
        MetaFile::of(v, &bytes)
    } else {
        let path = feed.join(format!("{}.targets.json", t.version));
        MetaFile::of(t.version, &read(&path)?)
    };
    sign_snapshot_and_timestamp(feed, k, Some(&c), targets, now)?;
    status(feed, now)
}

/// Root N+1: the same keys and thresholds, a fresh expiry, signed by all
/// root keys (a threshold of root N's and of its own, as a client
/// requires), then taken by a client at root N before it is kept.
fn renew_root(k: &Keys, feed: &Path, now: i64) -> Result {
    let Some(mut c) = current(feed)? else {
        return Err(format!("{}: no feed (no 1.root.json)", feed.display()));
    };
    check_keys(feed, &c, k)?;
    let (old, old_expires) = (c.root.version, c.root.expires);
    let v = old + 1;
    let all: Vec<&KeyPair> = k.root.iter().collect();
    let bytes = tuf::sign(&k.root_meta(v, now + ROOT_DAYS * DAY), &all);
    c.update_root(&bytes)
        .map_err(|r| format!("root {v} as a client at root {old} sees it: {r}"))?;
    write(&feed.join(format!("{v}.root.json")), &bytes)?;
    report(&format!(
        "root {v} (expires {}; root {old} expires {}), signed by all {ROOT_KEYS} root keys",
        Time(now + ROOT_DAYS * DAY),
        Time(old_expires)
    ));
    status(feed, now)
}

// ---- where the feed's files come from: a directory or a URL ----

/// A feed to read: a directory, or a bucket's public URL (read with curl).
#[derive(Clone, Debug)]
enum Source {
    Dir(PathBuf),
    Url(String),
}

impl Source {
    fn new(s: &str) -> Source {
        if s.starts_with("https://") || s.starts_with("http://") {
            Source::Url(s.trim_end_matches('/').to_string())
        } else {
            Source::Dir(PathBuf::from(s))
        }
    }

    /// The file at `path`, at most `max` + 1 bytes (a longer one is cut
    /// there, for the client to refuse); None if the feed has no such
    /// file; an error for anything else (unreachable, refused, HTTP 5xx):
    /// an absent file and a failed read must not look alike.
    fn get(&self, path: &str, max: u64) -> Result<Option<Vec<u8>>> {
        match self {
            Source::Dir(dir) => {
                let full = dir.join(path);
                let file = match fs::File::open(&full) {
                    Ok(f) => f,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(e) => return Err(format!("reading {}: {e}", full.display())),
                };
                let mut bytes = Vec::new();
                file.take(max + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| format!("reading {}: {e}", full.display()))?;
                Ok(Some(bytes))
            }
            Source::Url(base) => http_get(&format!("{base}/{path}"), max),
        }
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::Dir(d) => write!(f, "{}", d.display()),
            Source::Url(u) => f.write_str(u),
        }
    }
}

/// GET `url` with curl: Some on 200, None on 404, an error otherwise. At
/// most `max` + 1 bytes are kept.
fn http_get(url: &str, max: u64) -> Result<Option<Vec<u8>>> {
    let tmp = std::env::temp_dir().join(format!("gala-feed-{}", std::process::id()));
    fs::create_dir_all(&tmp).map_err(|e| format!("creating {}: {e}", tmp.display()))?;
    let out = tmp.join(url.rsplit('/').next().unwrap_or("file"));
    let _ = fs::remove_file(&out);
    let result = Command::new("curl")
        .args(["--silent", "--show-error", "--location", "--noproxy", "127.0.0.1,localhost"])
        .args(["--retry", "3", "--max-filesize"])
        .arg((max + 1).to_string())
        .arg("--output")
        .arg(&out)
        .args(["--write-out", "%{http_code}"])
        .arg(url)
        .output()
        .map_err(|e| format!("GET {url}: running curl: {e}"))?;
    let code = String::from_utf8_lossy(&result.stdout).trim().to_string();
    let got = match code.as_str() {
        "200" if result.status.success() => {
            let mut bytes = Vec::new();
            fs::File::open(&out)
                .and_then(|f| f.take(max + 1).read_to_end(&mut bytes))
                .map_err(|e| format!("GET {url}: reading {}: {e}", out.display()))?;
            Ok(Some(bytes))
        }
        "404" => Ok(None),
        _ => Err(format!(
            "GET {url}: HTTP {code}, curl {}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        )),
    };
    let _ = fs::remove_file(&out);
    got
}

/// Sources read in order (a directory over a bucket: the feed as it will
/// be once the directory is uploaded), as the client's remote. A failed
/// read is logged and kept for the error (the client sees a missing file).
struct Layered<'a> {
    sources: &'a [Source],
    errors: Vec<String>,
}

impl tuf::update::Remote for Layered<'_> {
    fn fetch(&mut self, path: &str, max: u64) -> Option<Vec<u8>> {
        for s in self.sources {
            match s.get(path, max) {
                Ok(Some(bytes)) => return Some(bytes),
                Ok(None) => {}
                Err(e) => {
                    eprintln!("xtask: feed: {e}");
                    self.errors.push(e);
                    return None;
                }
            }
        }
        None
    }
}

/// A client's store that starts empty (a machine that never updated).
#[derive(Default)]
struct MemStore {
    meta: BTreeMap<String, Vec<u8>>,
    blobs: BTreeSet<[u8; 32]>,
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
/// code) over `sources` (the first that has a file gives it), from `root`
/// (default: the feed's own first root) at time `now`, as a machine that
/// never updated: every file fetched and checked. A summary, or the first
/// refusal.
fn check(sources: &[Source], root: Option<&Path>, now: i64) -> Result<String> {
    let names: Vec<String> = sources.iter().map(Source::to_string).collect();
    let name = names.join(" over ");
    let mut remote = Layered { sources, errors: Vec::new() };
    let shipped = match root {
        Some(path) => read(path)?,
        None => tuf::update::Remote::fetch(&mut remote, "1.root.json", MAX_METADATA)
            .ok_or_else(|| format!("{name}: no 1.root.json ({})", remote.errors.join("; ")))?,
    };
    let mut log = String::new();
    let outcome =
        tuf::update::update(&mut remote, &mut MemStore::default(), &shipped, now, &mut |e| {
            log += &format!("  {e}\n");
        })
        .map_err(|e| {
            let errors: String =
                remote.errors.iter().map(|e| format!("  read error: {e}\n")).collect();
            format!("{name}: {e}\n{errors}{log}")
        })?;
    let [root, ts, snap, targets] = outcome.versions;
    let summary = format!(
        "checked {name} as a client: root {root}, timestamp {ts}, snapshot {snap}, targets \
         {targets}: release {} (floor {}), {} files, {} bytes, all match",
        outcome.release.number, outcome.release.floor, outcome.files, outcome.bytes,
    );
    report(&summary);
    Ok(format!("{summary}\n{log}"))
}

// ---- pull and push ----

/// Downloads the metadata a client follows from `src` into the empty
/// directory `dir`, each file verified as the client does (signatures,
/// versions, lengths and hashes; not expiry: an expired feed is what
/// `refresh` is for): every root, the timestamp, the snapshot it lists,
/// the targets that lists. Blobs are not downloaded (`publish` writes the
/// ones it needs). A source without `1.root.json` is an empty feed (the
/// first publish makes root 1). Records what it pulled for `push`.
fn pull(src: &Source, dir: &Path) -> Result {
    fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    if fs::read_dir(dir).map_err(|e| format!("reading {}: {e}", dir.display()))?.next().is_some() {
        return Err(format!("{}: not empty (pull writes into an empty directory)", dir.display()));
    }
    let mut pulled: Vec<(String, Vec<u8>)> = Vec::new();
    let mut keep = |name: String, bytes: Vec<u8>| -> Result {
        write(&dir.join(&name), &bytes)?;
        eprintln!("xtask: feed: pulled {name} ({} bytes)", bytes.len());
        pulled.push((name, bytes));
        Ok(())
    };
    let get = |name: &str, max: u64| -> Result<Option<Vec<u8>>> {
        src.get(name, max)?
            .map(|b| {
                if b.len() as u64 > max {
                    Err(format!("{src}/{name}: longer than {max} bytes"))
                } else {
                    Ok(b)
                }
            })
            .transpose()
    };
    let refused = |name: &str, r: tuf::Refusal| format!("{src}/{name}: refused: {r}");
    // Expiry is not checked; the hashes against the parent are.
    const ANY_TIME: Option<i64> = Some(i64::MIN);
    let done = 'pull: {
        let Some(first) = get("1.root.json", MAX_METADATA)? else {
            break 'pull format!("{src}: an empty feed (no 1.root.json): publish makes root 1");
        };
        let mut c = Client::new(&first).map_err(|r| refused("1.root.json", r))?;
        keep("1.root.json".into(), first)?;
        loop {
            let name = format!("{}.root.json", c.root.version + 1);
            let Some(bytes) = get(&name, MAX_METADATA)? else { break };
            c.update_root(&bytes).map_err(|r| refused(&name, r))?;
            keep(name, bytes)?;
        }
        let Some(ts) = get("timestamp.json", MAX_METADATA)? else {
            break 'pull format!("{src}: root {}, nothing published yet", c.root.version);
        };
        c.update_timestamp(&ts, ANY_TIME).map_err(|r| refused("timestamp.json", r))?;
        keep("timestamp.json".into(), ts)?;
        let listed = c.timestamp.as_ref().map(|t| t.snapshot).ok_or("no timestamp")?;
        let name = format!("{}.snapshot.json", listed.version);
        let bytes = get(&name, listed.length)?.ok_or(format!("{src}/{name}: not in the feed"))?;
        c.update_snapshot(&bytes, ANY_TIME).map_err(|r| refused(&name, r))?;
        keep(name, bytes)?;
        let listed = c.snapshot.as_ref().map(|s| s.targets).ok_or("no snapshot")?;
        let name = format!("{}.targets.json", listed.version);
        let bytes = get(&name, listed.length)?.ok_or(format!("{src}/{name}: not in the feed"))?;
        c.update_targets(&bytes, ANY_TIME).map_err(|r| refused(&name, r))?;
        keep(name, bytes)?;
        let t = c.targets.as_ref().ok_or("no targets")?;
        format!(
            "pulled {src}: root {}, timestamp {}, snapshot {}, targets {} (release {})",
            c.root.version,
            c.timestamp.as_ref().map_or(0, |t| t.version),
            c.snapshot.as_ref().map_or(0, |s| s.version),
            t.version,
            t.release.number
        )
    };
    let list: String = pulled
        .iter()
        .map(|(name, bytes)| format!("{name} {}\n", tuf::hex(&tuf::sha256::digest(bytes))))
        .collect();
    write(&dir.join(PULLED), list.as_bytes())?;
    report(&done);
    Ok(())
}

/// Where `push` uploads: a bucket (the `aws` CLI, its endpoint and
/// credentials from the environment) or a directory (a stand-in).
#[derive(Clone, Debug)]
enum Dest {
    S3 { bucket: String, prefix: String },
    Dir(PathBuf),
}

impl Dest {
    fn new(s: &str) -> Dest {
        match s.strip_prefix("s3://") {
            Some(rest) => {
                let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
                let prefix = prefix.trim_matches('/');
                let prefix = if prefix.is_empty() { String::new() } else { format!("{prefix}/") };
                Dest::S3 { bucket: bucket.to_string(), prefix }
            }
            None => Dest::Dir(PathBuf::from(s)),
        }
    }
}

impl std::fmt::Display for Dest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Dest::S3 { bucket, prefix } => write!(f, "s3://{bucket}/{prefix}"),
            Dest::Dir(d) => write!(f, "{}", d.display()),
        }
    }
}

/// Cache headers: every file but timestamp.json is immutable (its name
/// is its version or its digest); timestamp.json is read fresh.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const FRESH: &str = "no-cache";

fn aws(args: &[&str]) -> Result<String> {
    let out = Command::new("aws")
        .args(args)
        .output()
        .map_err(|e| format!("running aws {}: {e}", args.join(" ")))?;
    if !out.status.success() {
        return Err(format!(
            "aws {}: {}: {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The metadata files of a feed directory in upload order: roots, targets,
/// snapshots (each by version), timestamp last.
fn upload_order(names: &[String]) -> Result<Vec<String>> {
    let mut keyed = Vec::new();
    for name in names {
        let key = if name == "timestamp.json" {
            (3, 0)
        } else {
            let (v, role) = name
                .strip_suffix(".json")
                .and_then(|n| n.split_once('.'))
                .and_then(|(v, role)| Some((v.parse::<u64>().ok()?, role)))
                .ok_or_else(|| format!("{name}: not a file of the feed"))?;
            match role {
                "root" => (0, v),
                "targets" => (1, v),
                "snapshot" => (2, v),
                _ => return Err(format!("{name}: not a file of the feed")),
            }
        };
        keyed.push((key, name.clone()));
    }
    keyed.sort();
    Ok(keyed.into_iter().map(|(_, n)| n).collect())
}

/// Uploads what changed in `feed` since its `pull` to `dest`: the blobs
/// the destination lacks, then each new metadata file in upload_order
/// (timestamp last), so a client reading mid-upload only ever meets files
/// that name files already there. A published version (any file but
/// timestamp.json) is never overwritten: one the destination has already
/// stops the push (a feed pulled from elsewhere than it is pushed to, or
/// two runs at once).
fn push(feed: &Path, dest: &Dest) -> Result {
    let pulled_path = feed.join(PULLED);
    let pulled: BTreeMap<String, String> = fs::read_to_string(&pulled_path)
        .map_err(|e| {
            format!(
                "{}: {e} (push uploads what changed since `feed pull` into this directory)",
                pulled_path.display()
            )
        })?
        .lines()
        .filter_map(|l| l.split_once(' ').map(|(n, h)| (n.to_string(), h.to_string())))
        .collect();
    let mut blobs = Vec::new();
    let mut meta = Vec::new();
    for (name, path) in files(feed)? {
        if name == PULLED {
            continue;
        }
        if name.starts_with("blobs/") {
            blobs.push((name, path));
            continue;
        }
        let hash = tuf::hex(&tuf::sha256::digest(&read(&path)?));
        match pulled.get(&name) {
            Some(h) if *h == hash => {}
            Some(_) if name != "timestamp.json" => {
                return Err(format!(
                    "{name}: changed since it was pulled; a published version is never \
                     rewritten"
                ));
            }
            _ => meta.push(name),
        }
    }
    let meta = upload_order(&meta)?;
    let (mut new_blobs, mut had_blobs) = (0, 0);
    match dest {
        Dest::Dir(d) => {
            for (name, path) in &blobs {
                let to = d.join(name);
                let len = fs::metadata(path).map_err(|e| e.to_string())?.len();
                if fs::metadata(&to).is_ok_and(|m| m.len() == len) {
                    had_blobs += 1;
                    continue;
                }
                fs::create_dir_all(to.parent().unwrap_or(d)).map_err(|e| e.to_string())?;
                write(&to, &read(path)?)?;
                new_blobs += 1;
            }
            for name in &meta {
                let to = d.join(name);
                if name != "timestamp.json" && to.exists() {
                    return Err(format!(
                        "{}: there already; a published version is never rewritten",
                        to.display()
                    ));
                }
                write(&to, &read(&feed.join(name))?)?;
                eprintln!("xtask: feed: pushed {name} to {dest}");
            }
        }
        Dest::S3 { bucket, prefix } => {
            if !blobs.is_empty() {
                // Content-addressed: a blob of the same name and size in
                // the bucket is this one (`--size-only`).
                let out = aws(&[
                    "s3",
                    "sync",
                    &feed.join("blobs").to_string_lossy(),
                    &format!("s3://{bucket}/{prefix}blobs"),
                    "--size-only",
                    "--no-progress",
                    "--cache-control",
                    IMMUTABLE,
                    "--content-type",
                    "application/octet-stream",
                ])?;
                for line in out.lines() {
                    eprintln!("xtask: feed: {line}");
                }
                new_blobs = out.lines().filter(|l| l.starts_with("upload:")).count();
                had_blobs = blobs.len() - new_blobs;
            }
            for name in &meta {
                let key = format!("{prefix}{name}");
                let fresh = name == "timestamp.json";
                if !fresh && s3_exists(bucket, &key)? {
                    return Err(format!(
                        "s3://{bucket}/{key}: there already; a published version is never \
                         rewritten (was the feed pulled from this bucket's public URL?)"
                    ));
                }
                aws(&[
                    "s3",
                    "cp",
                    &feed.join(name).to_string_lossy(),
                    &format!("s3://{bucket}/{key}"),
                    "--no-progress",
                    "--cache-control",
                    if fresh { FRESH } else { IMMUTABLE },
                    "--content-type",
                    "application/json",
                ])?;
                eprintln!("xtask: feed: pushed {name} to s3://{bucket}/{key}");
            }
        }
    }
    report(&format!(
        "pushed to {dest}: {new_blobs} new blobs ({had_blobs} there already), then {}",
        if meta.is_empty() { "no metadata".to_string() } else { meta.join(", ") }
    ));
    Ok(())
}

/// Whether the bucket has `key` (HEAD): an error for anything but found
/// or not found.
fn s3_exists(bucket: &str, key: &str) -> Result<bool> {
    match aws(&["s3api", "head-object", "--bucket", bucket, "--key", key]) {
        Ok(_) => Ok(true),
        Err(e) if e.contains("Not Found") || e.contains("(404)") => Ok(false),
        Err(e) => Err(e),
    }
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

    /// A GALA_FEED_KEYS value with fixed seeds (`tag` tells sets apart).
    fn keys_text(tag: u8) -> String {
        KEY_ROLES
            .iter()
            .enumerate()
            .map(|(i, r)| format!("{r} {}\n", tuf::hex(&[tag * 16 + i as u8; 32])))
            .collect()
    }

    fn test_keys(tag: u8) -> Keys {
        parse_keys(&keys_text(tag)).unwrap()
    }

    #[test]
    fn publish_refresh_check() {
        let k = test_keys(1);
        let d = scratch("publish");
        let (files, feed) = (d.join("files"), d.join("feed"));
        let at = |s: &Path| vec![Source::Dir(s.to_path_buf())];
        fs::write(files.join("boot/kernel"), b"kernel 1").unwrap();
        fs::write(files.join("boot/gala.manifest"), b"manifest 1").unwrap();
        publish(&k, &files, &feed, None, None, NOW).unwrap();
        let r = check(&at(&feed), None, NOW).unwrap();
        assert!(
            r.contains("root 1, timestamp 1, snapshot 1, targets 1: release 1 (floor 1), 2 files"),
            "{r}"
        );

        // A day later the timestamp has expired: a frozen feed...
        let later = NOW + 2 * DAY;
        let e = check(&at(&feed), None, later).unwrap_err();
        assert!(e.contains("timestamp.json: refused: timestamp: expired"), "{e}");
        // ...until refreshed.
        refresh(&k, &feed, later).unwrap();
        let r = check(&at(&feed), None, later).unwrap();
        assert!(r.contains("timestamp 2, snapshot 2, targets 1: release 1"), "{r}");

        // Release 2 shares the unchanged blob.
        fs::write(files.join("boot/kernel"), b"kernel 2").unwrap();
        publish(&k, &files, &feed, None, Some(2), later).unwrap();
        let r = check(&at(&feed), None, later).unwrap();
        assert!(r.contains("targets 2: release 2 (floor 2)"), "{r}");
        assert_eq!(fs::read_dir(feed.join("blobs/sha256")).unwrap().count(), 3);
        // Releases only move forward.
        let e = publish(&k, &files, &feed, Some(2), None, later).unwrap_err();
        assert!(e.contains("does not follow release 2"), "{e}");

        // Targets near their expiry are signed again by refresh (a quiet
        // main does not freeze the feed); the release stays.
        refresh(&k, &feed, later + 10 * DAY).unwrap();
        let r = check(&at(&feed), None, later + 10 * DAY).unwrap();
        assert!(r.contains("snapshot 4, targets 2: release 2"), "{r}");
        refresh(&k, &feed, later + 20 * DAY).unwrap();
        let r = check(&at(&feed), None, later + 20 * DAY).unwrap();
        assert!(r.contains("snapshot 5, targets 3: release 2 (floor 2)"), "{r}");
        // Past targets 2's expiry, the feed is still valid.
        let quiet = later + 35 * DAY;
        refresh(&k, &feed, quiet).unwrap();
        let r = check(&at(&feed), None, quiet).unwrap();
        assert!(r.contains("snapshot 6, targets 3: release 2"), "{r}");

        // A tampered blob.
        let blob = feed.join(Target::of(b"kernel 2").blob_path());
        fs::write(&blob, b"kernel X").unwrap();
        let e = check(&at(&feed), None, quiet).unwrap_err();
        assert!(e.contains("refused: boot/kernel: SHA-256"), "{e}");
    }

    #[test]
    fn keys_from_the_environment() {
        let k = keys_from(Some(keys_text(1).into())).unwrap();
        assert_eq!(k.source, KEYS_VAR);
        assert_eq!(k.targets.pk, test_keys(1).targets.pk);
        // Set but empty (a secret the job cannot see): never the test keys.
        let e = keys_from(Some("".into())).err().unwrap();
        assert!(e.contains("set but empty"), "{e}");

        // Each mistake named by role or line, the value never shown.
        let secret = "ab".repeat(32);
        let with = |line: &str| format!("{}{line}\n", keys_text(1));
        let without = |role: &str| -> String {
            keys_text(1)
                .lines()
                .filter(|l| !l.starts_with(role))
                .map(|l| l.to_string() + "\n")
                .collect()
        };
        for (text, expect) in [
            (without("targets"), "no key for role targets"),
            (without("root-2"), "no key for role root-2"),
            (with(&format!("rooot-1 {secret}")), "line 7 names none of the roles"),
            (with(&format!("{secret} targets")), "line 7 names none of the roles"),
            (with(&format!("snapshot {secret}")), "role snapshot is given twice"),
            (with(&format!("timestamp {secret} x")), "line 7 is not"),
            (
                format!("{}timestamp {}\n", without("timestamp"), &secret[..60]),
                "role timestamp: its seed is not 64 hex digits",
            ),
            (
                format!(
                    "{}timestamp {}\n",
                    without("timestamp"),
                    tuf::hex(&[16 + 3; 32]) // the targets seed
                ),
                "roles targets and timestamp have the same key",
            ),
        ] {
            let e = parse_keys(&text).err().unwrap();
            assert!(e.contains(expect), "{e}");
            assert!(!e.contains(&secret[..60]) && !e.contains(&tuf::hex(&[16; 32])), "{e}");
        }
        // Comments and blank lines are fine.
        parse_keys(&format!("# feed keys\n\n{}", keys_text(2))).unwrap();
    }

    #[test]
    fn keygen_makes_keys_the_feed_takes() {
        let mut n = 0u8;
        let (value, ids) = keygen(&mut || {
            n += 1;
            Ok([n; 32])
        })
        .unwrap();
        let k = parse_keys(&value).unwrap();
        assert_eq!(ids.lines().count(), KEY_ROLES.len());
        assert!(ids.contains(&format!("targets: key id {}", tuf::key_id(&k.targets.pk))), "{ids}");
        // The ids go to standard error, the seeds only to the value.
        assert!(!ids.contains(&tuf::hex(&[1; 32])));
        // Real seeds differ.
        let (a, _) = keygen(&mut urandom).unwrap();
        let (b, _) = keygen(&mut urandom).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn other_keys_refused() {
        let d = scratch("other-keys");
        let (files, feed) = (d.join("files"), d.join("feed"));
        fs::write(files.join("boot/kernel"), b"kernel 1").unwrap();
        publish(&test_keys(1), &files, &feed, None, None, NOW).unwrap();
        let other = test_keys(2);
        for e in [
            refresh(&other, &feed, NOW).unwrap_err(),
            renew_root(&other, &feed, NOW).unwrap_err(),
            publish(&other, &files, &feed, None, None, NOW).unwrap_err(),
        ] {
            assert!(e.contains("lists other root keys") && e.contains("nothing signed"), "{e}");
        }
        // Only targets differs.
        let mixed = parse_keys(&keys_text(1).replace(
            &format!("targets {}", tuf::hex(&[16 + 3; 32])),
            &format!("targets {}", tuf::hex(&[99; 32])),
        ))
        .unwrap();
        let e = refresh(&mixed, &feed, NOW).unwrap_err();
        assert!(e.contains("lists other targets keys"), "{e}");
    }

    #[test]
    fn renewed_root_followed() {
        let k = test_keys(1);
        let d = scratch("renew");
        let (files, feed) = (d.join("files"), d.join("feed"));
        let at = vec![Source::Dir(feed.clone())];
        fs::write(files.join("boot/kernel"), b"kernel 1").unwrap();
        publish(&k, &files, &feed, None, None, NOW).unwrap();
        let root1 = feed.join("1.root.json");
        renew_root(&k, &feed, NOW + 30 * DAY).unwrap();
        refresh(&k, &feed, NOW + 30 * DAY).unwrap();
        // A client trusting root 1 (shipped in its image) takes root 2.
        let r = check(&at, Some(&root1), NOW + 30 * DAY).unwrap();
        assert!(r.contains("root 2 taken") && r.contains("root 2, timestamp 2"), "{r}");
        // After root 1's expiry it still follows the chain to root 2.
        let late = NOW + 100 * DAY;
        refresh(&k, &feed, late).unwrap();
        let r = check(&at, Some(&root1), late).unwrap();
        assert!(r.contains("root 2, timestamp 3"), "{r}");
        // Publish keeps going on root 2.
        fs::write(files.join("boot/kernel"), b"kernel 2").unwrap();
        publish(&k, &files, &feed, None, None, late).unwrap();
        let r = check(&at, Some(&root1), late).unwrap();
        assert!(r.contains("root 2, timestamp 4, snapshot 4") && r.contains("release 2"), "{r}");
        // Past root 2's expiry (90 days after its renewal) a client stops.
        let e = check(&at, Some(&root1), NOW + 121 * DAY).unwrap_err();
        assert!(e.contains("root: expired"), "{e}");
    }

    #[test]
    fn upload_order_is_roots_targets_snapshot_timestamp() {
        let names: Vec<String> = [
            "timestamp.json",
            "3.snapshot.json",
            "10.root.json",
            "2.targets.json",
            "9.root.json",
            "3.targets.json",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            upload_order(&names).unwrap(),
            [
                "9.root.json",
                "10.root.json",
                "2.targets.json",
                "3.targets.json",
                "3.snapshot.json",
                "timestamp.json"
            ]
        );
        assert!(upload_order(&["x.partial".into()]).is_err());
    }

    /// The CI workflows' steps against a directory standing in for the
    /// bucket: each run starts from an empty directory, pulls, signs,
    /// checks the result over the bucket, pushes, checks the bucket.
    #[test]
    fn workflows_against_a_local_bucket() {
        let k = test_keys(3);
        let d = scratch("e2e");
        let bucket = d.join("bucket");
        fs::create_dir_all(&bucket).unwrap();
        let src = Source::Dir(bucket.clone());
        let mut run = 0;
        let mut step = |now: i64, op: &dyn Fn(&Path) -> Result| -> String {
            run += 1;
            let work = d.join(format!("run{run}"));
            pull(&src, &work).unwrap();
            op(&work).unwrap();
            check(&[Source::Dir(work.clone()), src.clone()], None, now).unwrap();
            push(&work, &Dest::Dir(bucket.clone())).unwrap();
            // Pushing the same run again would rewrite published files.
            let e = push(&work, &Dest::Dir(bucket.clone())).unwrap_err();
            assert!(e.contains("never rewritten"), "{e}");
            check(std::slice::from_ref(&src), None, now).unwrap()
        };
        let files = d.join("files");
        fs::write(files.join("boot/kernel"), b"kernel 1").unwrap();
        fs::write(files.join("boot/ntdll.dll"), b"ntdll 1").unwrap();
        // publish: an empty bucket gets root 1 and release 1.
        let r = step(NOW, &|w| publish(&k, &files, w, None, None, NOW));
        assert!(r.contains("root 1, timestamp 1, snapshot 1, targets 1: release 1"), "{r}");
        // refresh, a day later.
        let t = NOW + 2 * DAY;
        let r = step(t, &|w| refresh(&k, w, t));
        assert!(r.contains("timestamp 2, snapshot 2, targets 1: release 1"), "{r}");
        // renew-root and refresh, a month later (targets signed again).
        let t = NOW + 31 * DAY;
        let r = step(t, &|w| renew_root(&k, w, t).and_then(|()| refresh(&k, w, t)));
        assert!(r.contains("root 2, timestamp 3, snapshot 3, targets 2: release 1"), "{r}");
        // publish on root 2 after root 1 expired: one changed file, one
        // new blob.
        let t = NOW + 95 * DAY;
        fs::write(files.join("boot/kernel"), b"kernel 2").unwrap();
        let r = step(t, &|w| publish(&k, &files, w, None, None, t));
        assert!(r.contains("root 2, timestamp 4, snapshot 4, targets 3: release 2"), "{r}");
        assert_eq!(fs::read_dir(bucket.join("blobs/sha256")).unwrap().count(), 3);
        // Every version is kept in the bucket; only timestamp.json is one.
        for f in ["1.root.json", "2.root.json", "3.targets.json", "4.snapshot.json"] {
            assert!(bucket.join(f).exists(), "{f}");
        }

        // A run that pulled from somewhere else (an empty feed) must not
        // overwrite the bucket's root 1.
        let elsewhere = d.join("elsewhere");
        let stray = d.join("stray");
        pull(&Source::Dir(elsewhere), &stray).unwrap();
        publish(&k, &files, &stray, None, None, t).unwrap();
        let e = push(&stray, &Dest::Dir(bucket.clone())).unwrap_err();
        assert!(e.contains("1.root.json: there already"), "{e}");
        // Pull refuses a directory with something in it.
        let e = pull(&src, &stray).unwrap_err();
        assert!(e.contains("not empty"), "{e}");
    }

    /// A pull over HTTP (curl) from a server standing in for the bucket's
    /// public URL: 200 and 404 told apart, other answers an error.
    #[test]
    fn pull_and_check_over_http() {
        if Command::new("curl").arg("--version").output().is_err() {
            eprintln!("no curl: skipped");
            return;
        }
        let k = test_keys(4);
        let d = scratch("http");
        let (files, feed) = (d.join("files"), d.join("feed"));
        fs::write(files.join("boot/kernel"), b"kernel 1").unwrap();
        publish(&k, &files, &feed, None, None, NOW).unwrap();
        let url = serve(feed.clone());
        let src = Source::new(&url);
        let work = d.join("work");
        pull(&src, &work).unwrap();
        assert!(work.join("1.root.json").exists() && work.join("1.targets.json").exists());
        assert!(!work.join("blobs").exists());
        let r = check(std::slice::from_ref(&src), None, NOW).unwrap();
        assert!(r.contains("release 1 (floor 1), 1 files"), "{r}");
        // A refreshed work directory over the URL: the blobs come from it.
        refresh(&k, &work, NOW + DAY).unwrap();
        let r = check(&[Source::Dir(work.clone()), src.clone()], None, NOW + DAY).unwrap();
        assert!(r.contains("timestamp 2"), "{r}");
        // Only 404 is an absent file; a 403 (a bucket not public) is an error.
        assert_eq!(src.get("nothing.json", 10).unwrap(), None);
        let e = src.get("fail", 10).unwrap_err();
        assert!(e.contains("HTTP 403"), "{e}");
    }

    /// Serves `dir` on 127.0.0.1 (a thread that lives as long as the test
    /// process); `fail` answers 403. Its base URL.
    fn serve(dir: PathBuf) -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let mut reader = BufReader::new(&conn);
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let path = line.split_whitespace().nth(1).unwrap_or("/").trim_start_matches('/');
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).map_or(true, |n| n == 0) || h == "\r\n" {
                        break;
                    }
                }
                let (status, body) = match (path, fs::read(dir.join(path))) {
                    ("fail", _) => ("403 Forbidden", Vec::new()),
                    (_, Ok(b)) => ("200 OK", b),
                    (_, Err(_)) => ("404 Not Found", Vec::new()),
                };
                let mut conn = &conn;
                let _ = write!(
                    conn,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = conn.write_all(&body);
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    #[test]
    fn positional_skips_flags() {
        let a: Vec<String> =
            ["publish", "--release", "3", "a", "b"].iter().map(|s| s.to_string()).collect();
        assert_eq!(positional(&a), ["publish", "a", "b"]);
    }
}
