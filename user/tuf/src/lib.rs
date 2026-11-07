//! The update feed's metadata (R8 step 4; ARCHITECTURE "The update feed"):
//! TUF, The Update Framework (specification 1.0), with Ed25519 keys and
//! SHA-256, written from the specification.
//!
//! A feed is static files any bucket, CDN or mirror can serve untrusted:
//! - `<N>.root.json`: the keys and, per role, which keys sign it and how
//!   many must (the threshold). Root keys are kept offline; root N+1 is
//!   signed by a threshold of root N's root keys and of its own.
//! - `<N>.targets.json`: the release's files (path, length, SHA-256), with
//!   the release number and rollback floor in `custom`; the release key.
//! - `<N>.snapshot.json`: the targets metadata's version, length and hash.
//! - `timestamp.json`: the snapshot's; short expiry, re-signed on a
//!   schedule, so a client tells a frozen feed from a quiet one.
//! - `blobs/sha256/<hex>`: the files, by content.
//!
//! [`Client`] checks a feed the way the specification's client workflow
//! (section 5) does and refuses (each a [`Refusal`]): metadata past its
//! expiry (a freeze), versions below what it trusts (a rollback), metadata
//! or files whose length or hash differ from what the signed parent lists
//! (tampering, mix-and-match), too few valid signatures from the role's
//! keys (signatures by keys root does not list count for nothing), and
//! files the signed targets do not list.

#![no_std]

extern crate alloc;
// The included SHA-256 file's own tests use std.
#[cfg(test)]
extern crate std;

pub mod json;
#[path = "../../../kernel/src/trust/sha256.rs"]
pub mod sha256;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use ed25519_compact::{KeyPair, PublicKey, Signature};
use json::Value;

/// The specification version the metadata says it follows.
pub const SPEC_VERSION: &str = "1.0.31";
/// The roles, in the order a client fetches them after root.
pub const ROLES: [&str; 4] = ["root", "timestamp", "snapshot", "targets"];

/// Why a client refuses metadata or a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not JSON this crate reads.
    Json { role: &'static str, error: json::Error },
    /// JSON, but not the role's metadata: a field missing or of the wrong
    /// type, the wrong `_type`, a key id that is not its key's.
    Format { role: &'static str, what: String },
    /// Fewer valid signatures by the role's keys than its threshold.
    Signatures { role: &'static str, valid: u64, threshold: u64 },
    /// Past its expiry (seconds since 1970, UTC).
    Expired { role: &'static str, expires: i64, now: i64 },
    /// A version below the one trusted.
    Rollback { role: &'static str, trusted: u64, offered: u64 },
    /// A version other than the one its parent lists (or, for root, the
    /// next one).
    Version { role: &'static str, expected: u64, got: u64 },
    /// A length other than the signed one.
    Length { what: String, expected: u64, got: u64 },
    /// A SHA-256 other than the signed one.
    Hash { what: String, expected: String, got: String },
    /// A file the signed targets do not list.
    NotInTargets { path: String },
    /// A role fetched before the one that lists it.
    Order { role: &'static str, needs: &'static str },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::Json { role, error } => {
                write!(f, "{role}: not JSON: {} at byte {}", error.what, error.at)
            }
            Refusal::Format { role, what } => write!(f, "{role}: {what}"),
            Refusal::Signatures { role, valid, threshold } => {
                write!(f, "{role}: {valid} valid signatures of the {threshold} its keys must give")
            }
            Refusal::Expired { role, expires, now } => {
                write!(
                    f,
                    "{role}: expired at {} (now {}): a frozen feed",
                    Time(*expires),
                    Time(*now)
                )
            }
            Refusal::Rollback { role, trusted, offered } => {
                write!(f, "{role}: version {offered} below the trusted {trusted}: a rollback")
            }
            Refusal::Version { role, expected, got } => {
                write!(f, "{role}: version {got}, expected {expected}")
            }
            Refusal::Length { what, expected, got } => {
                write!(f, "{what}: {got} bytes, signed as {expected}")
            }
            Refusal::Hash { what, expected, got } => {
                write!(f, "{what}: SHA-256 {got}, signed as {expected}")
            }
            Refusal::NotInTargets { path } => write!(f, "{path}: not in the signed targets"),
            Refusal::Order { role, needs } => write!(f, "{role}: fetched before {needs}"),
        }
    }
}

pub type Result<T> = core::result::Result<T, Refusal>;

// ---- hex and time ----

pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        s.push(char::from_digit(u32::from(b & 15), 16).unwrap_or('0'));
    }
    s
}

pub fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    let b = s.as_bytes();
    if b.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (i, pair) in b.chunks_exact(2).enumerate() {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out[i] = (hi * 16 + lo) as u8;
    }
    Some(out)
}

/// Days since 1970-01-01 of a proleptic Gregorian date: Howard Hinnant's
/// days-from-civil algorithm (THIRD_PARTY.md), 400-year eras of 146097
/// days, years starting in March so the leap day ends one.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// TUF's time format, `YYYY-MM-DDTHH:MM:SSZ`, as seconds since 1970.
pub fn parse_time(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' {
        return None;
    }
    if b[16] != b':' || b[19] != b'Z' {
        return None;
    }
    let num = |r: core::ops::Range<usize>| -> Option<i64> {
        b[r].iter().try_fold(0i64, |n, &c| c.is_ascii_digit().then(|| n * 10 + i64::from(c - b'0')))
    };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || d < 1 || h > 23 || mi > 59 || sec > 59 {
        return None;
    }
    let days = days_from_civil(y, mo, d);
    // A day past the month's end (2025-02-30) does not round-trip.
    if civil_from_days(days) != (y, mo, d) {
        return None;
    }
    Some(days * 86400 + h * 3600 + mi * 60 + sec)
}

/// Seconds since 1970 in TUF's time format.
#[derive(Clone, Copy)]
pub struct Time(pub i64);

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (days, secs) = (self.0.div_euclid(86400), self.0.rem_euclid(86400));
        let (y, m, d) = civil_from_days(days);
        let (h, mi, s) = (secs / 3600, secs / 60 % 60, secs % 60);
        write!(f, "{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
    }
}

// ---- keys ----

/// The key's JSON object (`keytype`, `scheme`, `keyval`).
pub fn key_value(key: &PublicKey) -> Value {
    let mut keyval = BTreeMap::new();
    keyval.insert("public".to_string(), Value::Str(hex(key.as_ref())));
    let mut m = BTreeMap::new();
    m.insert("keytype".to_string(), Value::Str("ed25519".into()));
    m.insert("scheme".to_string(), Value::Str("ed25519".into()));
    m.insert("keyval".to_string(), Value::Object(keyval));
    Value::Object(m)
}

/// A key's id: the SHA-256 of its JSON object's canonical form, in hex.
pub fn key_id(key: &PublicKey) -> String {
    hex(&sha256::digest(&key_value(key).canonical()))
}

// ---- writing ----

fn obj<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Object(fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn int(n: u64) -> Value {
    Value::Int(i64::try_from(n).unwrap_or(i64::MAX))
}

fn common<const N: usize>(ty: &str, version: u64, expires: i64, rest: [(&str, Value); N]) -> Value {
    let mut v = obj([
        ("_type", Value::Str(ty.into())),
        ("spec_version", Value::Str(SPEC_VERSION.into())),
        ("version", int(version)),
        ("expires", Value::Str(Time(expires).to_string())),
    ]);
    if let Value::Object(m) = &mut v {
        for (k, x) in rest {
            m.insert(k.to_string(), x);
        }
    }
    v
}

/// The `length` and `hashes` of a file or of metadata.
fn file_fields(length: u64, sha: &[u8; 32]) -> [(&'static str, Value); 2] {
    [("length", int(length)), ("hashes", obj([("sha256", Value::Str(hex(sha)))]))]
}

/// One role in root: its keys and threshold.
pub struct RoleKeys<'a> {
    pub name: &'static str,
    pub keys: &'a [PublicKey],
    pub threshold: u64,
}

/// Root's `signed` part.
pub fn root(version: u64, expires: i64, roles: &[RoleKeys<'_>]) -> Value {
    let mut keys = BTreeMap::new();
    let mut role_map = BTreeMap::new();
    for r in roles {
        let mut ids = Vec::new();
        for k in r.keys {
            let id = key_id(k);
            keys.insert(id.clone(), key_value(k));
            ids.push(Value::Str(id));
        }
        role_map.insert(
            r.name.to_string(),
            obj([("keyids", Value::Array(ids)), ("threshold", int(r.threshold))]),
        );
    }
    common(
        "root",
        version,
        expires,
        [
            ("consistent_snapshot", Value::Bool(true)),
            ("keys", Value::Object(keys)),
            ("roles", Value::Object(role_map)),
        ],
    )
}

/// What a parent lists about metadata it covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetaFile {
    pub version: u64,
    pub length: u64,
    pub sha256: [u8; 32],
}

impl MetaFile {
    /// Describes signed metadata `bytes` of `version`.
    pub fn of(version: u64, bytes: &[u8]) -> MetaFile {
        MetaFile { version, length: bytes.len() as u64, sha256: sha256::digest(bytes) }
    }
    fn value(&self) -> Value {
        let [l, h] = file_fields(self.length, &self.sha256);
        obj([("version", int(self.version)), l, h])
    }
}

/// Timestamp's `signed` part.
pub fn timestamp(version: u64, expires: i64, snapshot: &MetaFile) -> Value {
    common("timestamp", version, expires, [("meta", obj([("snapshot.json", snapshot.value())]))])
}

/// Snapshot's `signed` part.
pub fn snapshot(version: u64, expires: i64, targets: &MetaFile) -> Value {
    common("snapshot", version, expires, [("meta", obj([("targets.json", targets.value())]))])
}

/// One file of a release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub length: u64,
    pub sha256: [u8; 32],
}

impl Target {
    pub fn of(bytes: &[u8]) -> Target {
        Target { length: bytes.len() as u64, sha256: sha256::digest(bytes) }
    }
    /// Where the feed keeps the file: by content.
    pub fn blob_path(&self) -> String {
        format!("blobs/sha256/{}", hex(&self.sha256))
    }
}

/// The release a targets metadata describes (its `custom` field).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Release {
    /// Strictly increasing between releases (the GALAMANI manifest's).
    pub number: u64,
    /// The oldest release that may run after this one.
    pub floor: u64,
}

/// Targets' `signed` part.
pub fn targets(version: u64, expires: i64, release: Release, files: &[(&str, Target)]) -> Value {
    let mut m = BTreeMap::new();
    for (path, t) in files {
        let [l, h] = file_fields(t.length, &t.sha256);
        m.insert(path.to_string(), obj([l, h]));
    }
    common(
        "targets",
        version,
        expires,
        [
            ("targets", Value::Object(m)),
            ("custom", obj([("release", int(release.number)), ("floor", int(release.floor))])),
        ],
    )
}

/// Metadata as published: `signed` and each key's signature over its
/// canonical form, itself in canonical form.
pub fn sign(signed: &Value, keys: &[&KeyPair]) -> Vec<u8> {
    let msg = signed.canonical();
    let sigs = keys
        .iter()
        .map(|k| {
            obj([
                ("keyid", Value::Str(key_id(&k.pk))),
                ("sig", Value::Str(hex(k.sk.sign(&msg, None).as_ref()))),
            ])
        })
        .collect();
    obj([("signatures", Value::Array(sigs)), ("signed", signed.clone())]).canonical()
}

// ---- reading ----

struct Doc<'a> {
    role: &'static str,
    signed: &'a Value,
}

impl Doc<'_> {
    fn err(&self, what: impl Into<String>) -> Refusal {
        Refusal::Format { role: self.role, what: what.into() }
    }
    fn field(&self, v: &Value, name: &str) -> Result<Value> {
        v.get(name).cloned().ok_or_else(|| self.err(format!("no field {name:?}")))
    }
    fn uint(&self, v: &Value, name: &str) -> Result<u64> {
        self.field(v, name)?
            .as_int()
            .and_then(|n| u64::try_from(n).ok())
            .ok_or_else(|| self.err(format!("{name:?} is not a non-negative integer")))
    }
    fn sha(&self, v: &Value) -> Result<[u8; 32]> {
        let hashes = self.field(v, "hashes")?;
        hashes
            .get("sha256")
            .and_then(Value::as_str)
            .and_then(unhex::<32>)
            .ok_or_else(|| self.err("no SHA-256 in \"hashes\""))
    }
    /// `_type`, `version` (at least 1) and `expires`.
    fn common(&self) -> Result<(u64, i64)> {
        let ty = self.field(self.signed, "_type")?;
        if ty.as_str() != Some(self.role) {
            return Err(self.err(format!("_type {ty:?}, expected {:?}", self.role)));
        }
        let version = self.uint(self.signed, "version")?;
        if version == 0 {
            return Err(self.err("version 0"));
        }
        let expires = self.field(self.signed, "expires")?;
        let expires = expires
            .as_str()
            .and_then(parse_time)
            .ok_or_else(|| self.err("\"expires\" is not YYYY-MM-DDTHH:MM:SSZ"))?;
        Ok((version, expires))
    }
    fn meta(&self, name: &str) -> Result<MetaFile> {
        let meta = self.field(self.signed, "meta")?;
        let m = meta.get(name).ok_or_else(|| self.err(format!("no {name} in \"meta\"")))?;
        Ok(MetaFile {
            version: self.uint(m, "version")?,
            length: self.uint(m, "length")?,
            sha256: self.sha(m)?,
        })
    }
}

/// Parsed metadata: the envelope and the canonical bytes its signatures
/// cover.
struct Envelope {
    signed: Value,
    msg: Vec<u8>,
    /// (key id, signature)
    sigs: Vec<(String, [u8; 64])>,
}

fn envelope(role: &'static str, bytes: &[u8]) -> Result<Envelope> {
    let v = json::parse(bytes).map_err(|error| Refusal::Json { role, error })?;
    let bad = |what: &str| Refusal::Format { role, what: what.to_string() };
    let signed = v.get("signed").cloned().ok_or_else(|| bad("no field \"signed\""))?;
    let list = v
        .get("signatures")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("no \"signatures\" list"))?;
    let mut sigs = Vec::new();
    for s in list {
        let id = s
            .get("keyid")
            .and_then(Value::as_str)
            .ok_or_else(|| bad("a signature without \"keyid\""))?;
        // A signature that is not 64 bytes of hex is one that does not
        // verify, not a refusal of the whole file (it may be another
        // scheme's, from a key this client does not use).
        if let Some(sig) = s.get("sig").and_then(Value::as_str).and_then(unhex::<64>) {
            sigs.push((id.to_string(), sig));
        }
    }
    Ok(Envelope { msg: signed.canonical(), signed, sigs })
}

/// A role's keys and threshold, from root.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Role {
    keyids: Vec<String>,
    threshold: u64,
}

/// Trusted root metadata.
#[derive(Clone, Debug)]
pub struct Root {
    pub version: u64,
    pub expires: i64,
    keys: BTreeMap<String, PublicKey>,
    roles: BTreeMap<String, Role>,
}

impl Root {
    fn parse(env: &Envelope) -> Result<Root> {
        let d = Doc { role: "root", signed: &env.signed };
        let (version, expires) = d.common()?;
        let mut keys = BTreeMap::new();
        let key_map = d.field(&env.signed, "keys")?;
        for (id, k) in key_map.as_object().ok_or_else(|| d.err("\"keys\" is not an object"))? {
            if k.get("keytype").and_then(Value::as_str) != Some("ed25519")
                || k.get("scheme").and_then(Value::as_str) != Some("ed25519")
            {
                // Keys of other types are not this client's to use; a role
                // must reach its threshold with the ones it can check.
                continue;
            }
            let public = k
                .get("keyval")
                .and_then(|kv| kv.get("public"))
                .and_then(Value::as_str)
                .and_then(unhex::<32>)
                .and_then(|b| PublicKey::from_slice(&b).ok())
                .ok_or_else(|| d.err(format!("key {id}: no 32-byte \"public\"")))?;
            if key_id(&public) != *id {
                return Err(d.err(format!("key {id}: not that key's id")));
            }
            keys.insert(id.clone(), public);
        }
        let mut roles = BTreeMap::new();
        let role_map = d.field(&env.signed, "roles")?;
        for name in ROLES {
            let r = role_map.get(name).ok_or_else(|| d.err(format!("no role {name:?}")))?;
            let keyids: Vec<String> = r
                .get("keyids")
                .and_then(Value::as_array)
                .ok_or_else(|| d.err(format!("role {name}: no \"keyids\"")))?
                .iter()
                .map(|k| {
                    k.as_str()
                        .map(String::from)
                        .ok_or_else(|| d.err(format!("role {name}: a key id not a string")))
                })
                .collect::<Result<_>>()?;
            let threshold = d.uint(r, "threshold")?;
            if threshold == 0 {
                return Err(d.err(format!("role {name}: threshold 0")));
            }
            roles.insert(name.to_string(), Role { keyids, threshold });
        }
        Ok(Root { version, expires, keys, roles })
    }

    /// Checks that `env` carries `role`'s threshold of valid signatures.
    fn verify(&self, role: &'static str, env: &Envelope) -> Result<()> {
        let r = &self.roles[role];
        let mut valid: Vec<&str> = Vec::new();
        for (id, sig) in &env.sigs {
            if valid.contains(&id.as_str()) || !r.keyids.contains(id) {
                continue;
            }
            let Some(key) = self.keys.get(id) else { continue };
            if key.verify(&env.msg, &Signature::new(*sig)).is_ok() {
                valid.push(id);
            }
        }
        let valid = valid.len() as u64;
        if valid < r.threshold {
            return Err(Refusal::Signatures { role, valid, threshold: r.threshold });
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Timestamp {
    pub version: u64,
    pub expires: i64,
    pub snapshot: MetaFile,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub version: u64,
    pub expires: i64,
    pub targets: MetaFile,
}

#[derive(Clone, Debug)]
pub struct Targets {
    pub version: u64,
    pub expires: i64,
    pub release: Release,
    pub files: BTreeMap<String, Target>,
}

fn check_meta(what: &str, meta: &MetaFile, bytes: &[u8]) -> Result<()> {
    if bytes.len() as u64 != meta.length {
        return Err(Refusal::Length {
            what: what.to_string(),
            expected: meta.length,
            got: bytes.len() as u64,
        });
    }
    let got = sha256::digest(bytes);
    if got != meta.sha256 {
        return Err(Refusal::Hash {
            what: what.to_string(),
            expected: hex(&meta.sha256),
            got: hex(&got),
        });
    }
    Ok(())
}

fn not_expired(role: &'static str, expires: i64, now: Option<i64>) -> Result<()> {
    match now {
        Some(now) if expires <= now => Err(Refusal::Expired { role, expires, now }),
        _ => Ok(()),
    }
}

/// The client's trusted metadata, updated one role at a time in the
/// specification's order: root (each next version), timestamp, snapshot,
/// targets, then the files. Each `update_*` checks what it is given
/// against what is trusted and takes it only if every check passes.
///
/// `now` is `None` for metadata loaded from the client's own store (what
/// it trusted before: signatures are checked again, expiry is not, and it
/// is still the floor for rollbacks); the time for anything fetched.
#[derive(Clone, Debug)]
pub struct Client {
    pub root: Root,
    pub timestamp: Option<Timestamp>,
    pub snapshot: Option<Snapshot>,
    pub targets: Option<Targets>,
}

/// What an update did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Updated {
    /// A newer version is now trusted.
    Newer,
    /// The same version as the trusted one (a quiet feed); nothing changed.
    Same,
}

impl Client {
    /// Starts from a root the client trusts already (shipped in its image):
    /// it must carry its own root role's threshold of signatures.
    pub fn new(trusted_root: &[u8]) -> Result<Client> {
        let env = envelope("root", trusted_root)?;
        let root = Root::parse(&env)?;
        root.verify("root", &env)?;
        Ok(Client { root, timestamp: None, snapshot: None, targets: None })
    }

    /// Takes root version N+1, signed by a threshold of the trusted root's
    /// root keys and of its own. A rotation of the timestamp or snapshot
    /// keys drops the trusted timestamp and snapshot, so a feed
    /// whose old keys leaked can recover from versions pushed far ahead.
    pub fn update_root(&mut self, bytes: &[u8]) -> Result<()> {
        let env = envelope("root", bytes)?;
        self.root.verify("root", &env)?;
        let new = Root::parse(&env)?;
        new.verify("root", &env)?;
        if new.version != self.root.version + 1 {
            return Err(Refusal::Version {
                role: "root",
                expected: self.root.version + 1,
                got: new.version,
            });
        }
        if new.roles["timestamp"] != self.root.roles["timestamp"]
            || new.roles["snapshot"] != self.root.roles["snapshot"]
        {
            self.timestamp = None;
            self.snapshot = None;
        }
        self.root = new;
        Ok(())
    }

    /// After the last root: refuses an expired one (the client stops,
    /// the specification's 5.3.10).
    pub fn check_root(&self, now: i64) -> Result<()> {
        not_expired("root", self.root.expires, Some(now))
    }

    pub fn update_timestamp(&mut self, bytes: &[u8], now: Option<i64>) -> Result<Updated> {
        let env = envelope("timestamp", bytes)?;
        self.root.verify("timestamp", &env)?;
        let d = Doc { role: "timestamp", signed: &env.signed };
        let (version, expires) = d.common()?;
        let new = Timestamp { version, expires, snapshot: d.meta("snapshot.json")? };
        let mut same = false;
        if let Some(old) = &self.timestamp {
            if new.version < old.version {
                return Err(Refusal::Rollback {
                    role: "timestamp",
                    trusted: old.version,
                    offered: new.version,
                });
            }
            same = new.version == old.version;
            if new.snapshot.version < old.snapshot.version {
                return Err(Refusal::Rollback {
                    role: "snapshot",
                    trusted: old.snapshot.version,
                    offered: new.snapshot.version,
                });
            }
        }
        not_expired("timestamp", new.expires, now)?;
        if same {
            return Ok(Updated::Same);
        }
        self.timestamp = Some(new);
        Ok(Updated::Newer)
    }

    pub fn update_snapshot(&mut self, bytes: &[u8], now: Option<i64>) -> Result<Updated> {
        let Some(ts) = &self.timestamp else {
            return Err(Refusal::Order { role: "snapshot", needs: "timestamp" });
        };
        let listed = ts.snapshot;
        // From the feed, the bytes must be the ones timestamp signed for;
        // from the store, they were checked so when fetched.
        if now.is_some() {
            check_meta("snapshot", &listed, bytes)?;
        }
        let env = envelope("snapshot", bytes)?;
        self.root.verify("snapshot", &env)?;
        let d = Doc { role: "snapshot", signed: &env.signed };
        let (version, expires) = d.common()?;
        let new = Snapshot { version, expires, targets: d.meta("targets.json")? };
        if new.version != listed.version {
            return Err(Refusal::Version {
                role: "snapshot",
                expected: listed.version,
                got: new.version,
            });
        }
        if let Some(old) = &self.snapshot {
            if new.targets.version < old.targets.version {
                return Err(Refusal::Rollback {
                    role: "targets",
                    trusted: old.targets.version,
                    offered: new.targets.version,
                });
            }
            if new.version == old.version {
                not_expired("snapshot", new.expires, now)?;
                return Ok(Updated::Same);
            }
        }
        not_expired("snapshot", new.expires, now)?;
        self.snapshot = Some(new);
        Ok(Updated::Newer)
    }

    pub fn update_targets(&mut self, bytes: &[u8], now: Option<i64>) -> Result<Updated> {
        let Some(snap) = &self.snapshot else {
            return Err(Refusal::Order { role: "targets", needs: "snapshot" });
        };
        let listed = snap.targets;
        if now.is_some() {
            check_meta("targets", &listed, bytes)?;
        }
        let env = envelope("targets", bytes)?;
        self.root.verify("targets", &env)?;
        let d = Doc { role: "targets", signed: &env.signed };
        let (version, expires) = d.common()?;
        if version != listed.version {
            return Err(Refusal::Version {
                role: "targets",
                expected: listed.version,
                got: version,
            });
        }
        let custom = d.field(&env.signed, "custom")?;
        let release =
            Release { number: d.uint(&custom, "release")?, floor: d.uint(&custom, "floor")? };
        if release.floor > release.number {
            return Err(d.err(format!(
                "rollback floor {} above release {}",
                release.floor, release.number
            )));
        }
        let mut files = BTreeMap::new();
        let list = d.field(&env.signed, "targets")?;
        for (path, t) in list.as_object().ok_or_else(|| d.err("\"targets\" is not an object"))? {
            files.insert(path.clone(), Target { length: d.uint(t, "length")?, sha256: d.sha(t)? });
        }
        not_expired("targets", expires, now)?;
        if self.targets.as_ref().is_some_and(|old| old.version == version) {
            return Ok(Updated::Same);
        }
        self.targets = Some(Targets { version, expires, release, files });
        Ok(Updated::Newer)
    }

    /// The signed description of `path`, or the refusal of a file the
    /// targets do not list.
    pub fn target(&self, path: &str) -> Result<&Target> {
        let Some(t) = &self.targets else {
            return Err(Refusal::Order { role: "a file", needs: "targets" });
        };
        t.files.get(path).ok_or_else(|| Refusal::NotInTargets { path: path.to_string() })
    }

    /// Checks a fetched file against the signed targets.
    pub fn check_target(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let t = self.target(path)?;
        let meta = MetaFile { version: 0, length: t.length, sha256: t.sha256 };
        check_meta(path, &meta, bytes)
    }
}

#[cfg(test)]
mod tests;
