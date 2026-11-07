//! The client's checks against feeds made with this crate's writer: one
//! that passes, and each refusal the update scenarios rely on.

use super::*;
use alloc::vec;
use ed25519_compact::Seed;

const NOW: i64 = 1_800_000_000; // 2027-01-15
const DAY: i64 = 86400;

fn key(n: u8) -> KeyPair {
    KeyPair::from_seed(Seed::new([n; 32]))
}

/// The keys of a feed: three root keys (threshold 2), one each for the
/// other roles.
struct Keys {
    root: [KeyPair; 3],
    targets: KeyPair,
    snapshot: KeyPair,
    timestamp: KeyPair,
}

fn keys() -> Keys {
    Keys { root: [key(1), key(2), key(3)], targets: key(4), snapshot: key(5), timestamp: key(6) }
}

fn root_signed(k: &Keys, version: u64, expires: i64) -> Value {
    let root_pks: Vec<PublicKey> = k.root.iter().map(|k| k.pk).collect();
    root(
        version,
        expires,
        &[
            RoleKeys { name: "root", keys: &root_pks, threshold: 2 },
            RoleKeys { name: "targets", keys: &[k.targets.pk], threshold: 1 },
            RoleKeys { name: "snapshot", keys: &[k.snapshot.pk], threshold: 1 },
            RoleKeys { name: "timestamp", keys: &[k.timestamp.pk], threshold: 1 },
        ],
    )
}

/// One published state of a feed.
struct Feed {
    root: Vec<u8>,
    targets: Vec<u8>,
    snapshot: Vec<u8>,
    timestamp: Vec<u8>,
    blob: Vec<u8>,
}

/// A feed at `version` of every role, with one file, `boot/kernel`.
fn feed(k: &Keys, version: u64) -> Feed {
    let blob = vec![version as u8; 1000];
    let release = Release { number: version, floor: 1 };
    let t = targets(version, NOW + 30 * DAY, release, &[("boot/kernel", Target::of(&blob))]);
    let targets = sign(&t, &[&k.targets]);
    let s = snapshot(version, NOW + 7 * DAY, &MetaFile::of(version, &targets));
    let snapshot = sign(&s, &[&k.snapshot]);
    let ts = timestamp(version, NOW + DAY, &MetaFile::of(version, &snapshot));
    let timestamp = sign(&ts, &[&k.timestamp]);
    let root = sign(&root_signed(k, 1, NOW + 365 * DAY), &[&k.root[0], &k.root[1]]);
    Feed { root, targets, snapshot, timestamp, blob }
}

/// A client that took `f` through every step.
fn updated(f: &Feed) -> Client {
    let mut c = Client::new(&f.root).unwrap();
    c.check_root(NOW).unwrap();
    assert_eq!(c.update_timestamp(&f.timestamp, Some(NOW)), Ok(Updated::Newer));
    assert_eq!(c.update_snapshot(&f.snapshot, Some(NOW)), Ok(Updated::Newer));
    assert_eq!(c.update_targets(&f.targets, Some(NOW)), Ok(Updated::Newer));
    c
}

#[test]
fn a_feed_passes() {
    let k = keys();
    let f = feed(&k, 1);
    let c = updated(&f);
    c.check_target("boot/kernel", &f.blob).unwrap();
    let t = c.targets.as_ref().unwrap();
    assert_eq!(t.release, Release { number: 1, floor: 1 });
    assert_eq!(
        c.target("boot/kernel").unwrap().blob_path(),
        format!("blobs/sha256/{}", hex(&sha256::digest(&f.blob)))
    );
}

#[test]
fn a_quiet_feed_changes_nothing_and_a_newer_one_moves_on() {
    let k = keys();
    let mut c = updated(&feed(&k, 1));
    let f1 = feed(&k, 1);
    assert_eq!(c.update_timestamp(&f1.timestamp, Some(NOW)), Ok(Updated::Same));
    let f2 = feed(&k, 2);
    assert_eq!(c.update_timestamp(&f2.timestamp, Some(NOW)), Ok(Updated::Newer));
    assert_eq!(c.update_snapshot(&f2.snapshot, Some(NOW)), Ok(Updated::Newer));
    assert_eq!(c.update_targets(&f2.targets, Some(NOW)), Ok(Updated::Newer));
    c.check_target("boot/kernel", &f2.blob).unwrap();
    // The old file no longer matches.
    assert!(matches!(c.check_target("boot/kernel", &f1.blob), Err(Refusal::Hash { .. })));
}

#[test]
fn an_expired_timestamp_is_a_frozen_feed() {
    let k = keys();
    let f = feed(&k, 1);
    let mut c = Client::new(&f.root).unwrap();
    let later = NOW + 2 * DAY;
    assert_eq!(
        c.update_timestamp(&f.timestamp, Some(later)),
        Err(Refusal::Expired { role: "timestamp", expires: NOW + DAY, now: later })
    );
    assert!(c.timestamp.is_none());
    // From the client's own store, expiry is not checked.
    assert_eq!(c.update_timestamp(&f.timestamp, None), Ok(Updated::Newer));
}

#[test]
fn rollbacks_are_refused() {
    let k = keys();
    let mut c = updated(&feed(&k, 3));
    let old = feed(&k, 2);
    assert_eq!(
        c.update_timestamp(&old.timestamp, Some(NOW)),
        Err(Refusal::Rollback { role: "timestamp", trusted: 3, offered: 2 })
    );
    // A newer timestamp and snapshot that list an older targets.
    let t = sign(&targets(2, NOW + DAY, Release { number: 2, floor: 1 }, &[]), &[&k.targets]);
    let s = sign(&snapshot(4, NOW + DAY, &MetaFile::of(2, &t)), &[&k.snapshot]);
    let ts = sign(&timestamp(4, NOW + DAY, &MetaFile::of(4, &s)), &[&k.timestamp]);
    assert_eq!(c.update_timestamp(&ts, Some(NOW)), Ok(Updated::Newer));
    assert_eq!(
        c.update_snapshot(&s, Some(NOW)),
        Err(Refusal::Rollback { role: "targets", trusted: 3, offered: 2 })
    );
    assert_eq!(c.snapshot.as_ref().unwrap().version, 3);
    // A timestamp listing an older snapshot.
    let ts = sign(&timestamp(5, NOW + DAY, &MetaFile::of(2, &old.snapshot)), &[&k.timestamp]);
    assert_eq!(
        c.update_timestamp(&ts, Some(NOW)),
        Err(Refusal::Rollback { role: "snapshot", trusted: 4, offered: 2 })
    );
}

#[test]
fn tampered_files_and_metadata_are_refused() {
    let k = keys();
    let f = feed(&k, 1);
    let c = updated(&f);
    let mut blob = f.blob.clone();
    blob[10] ^= 1;
    assert!(matches!(c.check_target("boot/kernel", &blob), Err(Refusal::Hash { .. })));
    blob.push(0);
    assert_eq!(
        c.check_target("boot/kernel", &blob),
        Err(Refusal::Length { what: "boot/kernel".into(), expected: 1000, got: 1001 })
    );
    assert_eq!(
        c.check_target("boot/other", &f.blob),
        Err(Refusal::NotInTargets { path: "boot/other".into() })
    );

    // A snapshot other than the one timestamp lists (same length).
    let mut c = Client::new(&f.root).unwrap();
    c.update_timestamp(&f.timestamp, Some(NOW)).unwrap();
    let mut s = f.snapshot.clone();
    let at = s.len() - 3;
    s[at] = if s[at] == b'0' { b'1' } else { b'0' };
    assert!(matches!(c.update_snapshot(&s, Some(NOW)), Err(Refusal::Hash { .. })));

    // A signed field changed after signing (the store's copy, unhashed).
    let text =
        core::str::from_utf8(&f.timestamp).unwrap().replace("\"version\":1}", "\"version\":9}");
    let mut c = Client::new(&f.root).unwrap();
    assert_eq!(
        c.update_timestamp(text.as_bytes(), None),
        Err(Refusal::Signatures { role: "timestamp", valid: 0, threshold: 1 })
    );
}

#[test]
fn keys_not_in_root_count_for_nothing() {
    let k = keys();
    let f = feed(&k, 1);
    let mut c = Client::new(&f.root).unwrap();
    let stranger = key(9);
    let ts = sign(&timestamp(1, NOW + DAY, &MetaFile::of(1, &f.snapshot)), &[&stranger]);
    assert_eq!(
        c.update_timestamp(&ts, Some(NOW)),
        Err(Refusal::Signatures { role: "timestamp", valid: 0, threshold: 1 })
    );
    // Another role's key does not sign for this one either.
    let ts = sign(&timestamp(1, NOW + DAY, &MetaFile::of(1, &f.snapshot)), &[&k.snapshot]);
    assert!(matches!(c.update_timestamp(&ts, Some(NOW)), Err(Refusal::Signatures { .. })));
    // Two signatures by one key are one.
    let root = sign(&root_signed(&k, 1, NOW + DAY), &[&k.root[0], &k.root[0]]);
    assert_eq!(
        Client::new(&root).err(),
        Some(Refusal::Signatures { role: "root", valid: 1, threshold: 2 })
    );
}

#[test]
fn root_rotation() {
    let k = keys();
    let f = feed(&k, 1);
    let mut c = updated(&f);
    // Root 2 rotates the timestamp key and one root key.
    let mut k2 = keys();
    k2.root[2] = key(7);
    k2.timestamp = key(8);
    let r2 = root_signed(&k2, 2, NOW + 365 * DAY);
    // Signed only by the new root keys: root 1's threshold is not met
    // (key 7 is not root 1's).
    let only_new = sign(&r2, &[&k2.root[2]]);
    assert_eq!(
        c.update_root(&only_new),
        Err(Refusal::Signatures { role: "root", valid: 0, threshold: 2 })
    );
    // Signed by two of root 1's keys of which only one stays: root 2's
    // own threshold is not met.
    let only_old = sign(&r2, &[&k.root[1], &k.root[2]]);
    assert_eq!(
        c.update_root(&only_old),
        Err(Refusal::Signatures { role: "root", valid: 1, threshold: 2 })
    );
    // A version skipped.
    let r3 = sign(&root_signed(&k2, 3, NOW + DAY), &[&k.root[0], &k.root[1]]);
    assert_eq!(c.update_root(&r3), Err(Refusal::Version { role: "root", expected: 2, got: 3 }));
    let both = sign(&r2, &[&k.root[0], &k.root[1], &k2.root[2]]);
    c.update_root(&both).unwrap();
    assert_eq!(c.root.version, 2);
    // The timestamp key rotated: what the old keys signed is dropped, so
    // the new timestamp may restart at version 1.
    assert!(c.timestamp.is_none() && c.snapshot.is_none());
    let ts = sign(&timestamp(1, NOW + DAY, &MetaFile::of(1, &f.snapshot)), &[&k2.timestamp]);
    assert_eq!(c.update_timestamp(&ts, Some(NOW)), Ok(Updated::Newer));
    assert!(matches!(c.update_timestamp(&f.timestamp, Some(NOW)), Err(Refusal::Signatures { .. })));
}

#[test]
fn an_expired_root_stops_the_client() {
    let k = keys();
    let root = sign(&root_signed(&k, 1, NOW - 1), &[&k.root[0], &k.root[1]]);
    let c = Client::new(&root).unwrap();
    assert_eq!(
        c.check_root(NOW),
        Err(Refusal::Expired { role: "root", expires: NOW - 1, now: NOW })
    );
}

#[test]
fn roles_in_order() {
    let k = keys();
    let f = feed(&k, 1);
    let mut c = Client::new(&f.root).unwrap();
    assert_eq!(
        c.update_snapshot(&f.snapshot, Some(NOW)),
        Err(Refusal::Order { role: "snapshot", needs: "timestamp" })
    );
    assert_eq!(
        c.update_targets(&f.targets, Some(NOW)),
        Err(Refusal::Order { role: "targets", needs: "snapshot" })
    );
    // Metadata of one role offered as another.
    assert!(matches!(c.update_timestamp(&f.root, Some(NOW)), Err(Refusal::Signatures { .. })));
}

#[test]
fn a_key_listed_under_another_id_is_refused() {
    let k = keys();
    let mut signed = root_signed(&k, 1, NOW + DAY);
    let Value::Object(m) = &mut signed else { panic!() };
    let Some(Value::Object(keys)) = m.get_mut("keys") else { panic!() };
    let (id, v) = keys.pop_first().unwrap();
    keys.insert(format!("0{}", &id[1..]), v);
    let root = sign(&signed, &[&k.root[0], &k.root[1]]);
    assert!(matches!(Client::new(&root), Err(Refusal::Format { role: "root", .. })));
}

#[test]
fn time() {
    assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(parse_time("2027-01-15T08:00:00Z"), Some(NOW));
    assert_eq!(parse_time("2024-02-29T23:59:59Z"), Some(1_709_251_199));
    assert_eq!(parse_time("2025-02-29T00:00:00Z"), None);
    assert_eq!(parse_time("2025-13-01T00:00:00Z"), None);
    assert_eq!(parse_time("2025-01-01 00:00:00Z"), None);
    assert_eq!(parse_time("2025-01-01T00:00:00+01:00"), None);
    for t in [0, NOW, 1_709_251_199, 253_402_300_799] {
        assert_eq!(parse_time(&Time(t).to_string()), Some(t));
    }
}

#[test]
fn key_ids_are_the_canonical_keys_hash() {
    let k = key(1);
    let text = format!(
        r#"{{"keytype":"ed25519","keyval":{{"public":"{}"}},"scheme":"ed25519"}}"#,
        hex(k.pk.as_ref())
    );
    assert_eq!(key_id(&k.pk), hex(&sha256::digest(text.as_bytes())));
}
