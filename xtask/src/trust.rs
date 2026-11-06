//! R8, the chain of trust (ARCHITECTURE "Chain of trust"), as xtask builds
//! it into every image:
//! - steps 2-3: every path in limine.conf ends in `#<BLAKE2B>` with
//!   `hash_mismatch_panic: yes`, and the config's own BLAKE2B is enrolled
//!   in the Limine executables shipped (`limine enroll-config`), so Limine
//!   refuses a changed config, kernel or module;
//! - steps 4-5: a manifest of the boot files (size, SHA-256) signed with
//!   the test key, shipped as the boot module `/boot/gala.manifest`; the
//!   kernel is built with the key's public half (`GALA_TRUST_KEY`) and
//!   checks it.
//!
//! The test key is made on first use in `target/trust/` (never committed).
//! `XTASK_UNSIGNED=1` builds as before R8: kernels without a key, no
//! manifest, no hashes, nothing enrolled.

use crate::{Result, manifest, target_dir};
use ed25519_compact::{KeyPair, Seed};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// How one image is signed (scenarios change it to test refusals).
#[derive(Clone, Copy)]
pub struct Signing {
    /// The manifest's release number and rollback floor.
    pub release: u64,
    pub floor: u64,
    /// Boot files (names in /boot) left out of the manifest.
    pub leave_out: &'static [&'static str],
    /// A copy of the kernel with bytes appended (a valid image no
    /// manifest lists), shipped as the boot module `kernel-unsigned`.
    pub unsigned_kernel: bool,
    /// A boot file changed after the config was written (Limine refuses it).
    pub tamper: Option<&'static str>,
    /// The config changed after it was enrolled (Limine refuses it).
    pub edit_config: bool,
    /// Limine's own output on the serial port (`serial: yes`).
    pub limine_serial: bool,
}

/// The release a test image is signed as.
pub const DEFAULT: Signing = Signing {
    release: 1,
    floor: 1,
    leave_out: &[],
    unsigned_kernel: false,
    tamper: None,
    edit_config: false,
    limine_serial: false,
};

/// Scenarios signed differently (by scenario name).
pub const SCENARIO_SIGNING: &[(&str, Signing)] = &[
    // Limine refuses a module changed after the config was written.
    ("boot_tampered", Signing { tamper: Some("hello.exe"), limine_serial: true, ..DEFAULT }),
    // Limine (UEFI) refuses a config changed after it was enrolled.
    ("boot_config_edited", Signing { edit_config: true, limine_serial: true, ..DEFAULT }),
    // The kernel refuses to hand over to an image not in the manifest.
    (
        "handover_unsigned_refused",
        Signing { unsigned_kernel: true, leave_out: &["kernel-unsigned"], ..DEFAULT },
    ),
];

/// Disk scenarios whose earlier boots run another release (the image is
/// made again, signed so).
pub const EARLIER_SIGNING: &[(&str, Signing)] =
    &[("rollback_refused", Signing { release: 2, floor: 2, ..DEFAULT })];

/// The signing an image named `name` (`test-<scenario>`) gets.
pub fn signing_for(name: &str) -> Signing {
    let scenario = name.strip_prefix("test-").unwrap_or(name);
    SCENARIO_SIGNING.iter().find(|(n, _)| *n == scenario).map_or(DEFAULT, |(_, s)| *s)
}

/// `XTASK_UNSIGNED=1`: images and kernels as before R8.
pub fn unsigned() -> bool {
    env::var_os("XTASK_UNSIGNED").is_some_and(|v| v != "0")
}

fn key_dir() -> PathBuf {
    target_dir().join("trust")
}

/// The test signing key: its 32-byte seed in `target/trust/test-signing.seed`,
/// made from the system's random source the first time.
pub fn test_key() -> Result<KeyPair> {
    let path = key_dir().join("test-signing.seed");
    let seed = match fs::read(&path) {
        Ok(seed) => seed,
        Err(_) => {
            let mut seed = vec![0u8; 32];
            fs::File::open("/dev/urandom")
                .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut seed))
                .map_err(|e| format!("reading /dev/urandom: {e}"))?;
            fs::create_dir_all(key_dir()).map_err(|e| format!("creating {:?}: {e}", key_dir()))?;
            write_private(&path, &seed)?;
            let pair = KeyPair::try_from_seed(Seed::from_slice(&seed).map_err(|e| e.to_string())?)
                .map_err(|e| format!("test key: {e}"))?;
            let public = hex(&pair.pk[..]);
            let _ = fs::write(key_dir().join("test-signing.pub"), format!("{public}\n"));
            eprintln!(
                "xtask: made the test signing key {} (id {:016x})",
                path.display(),
                manifest::key_id(&pair.pk)
            );
            seed
        }
    };
    let seed = Seed::from_slice(&seed).map_err(|e| format!("{}: {e}", path.display()))?;
    KeyPair::try_from_seed(seed).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> Result {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut f| f.write_all(bytes))
        .map_err(|e| format!("writing {path:?}: {e}"))
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> Result {
    fs::write(path, bytes).map_err(|e| format!("writing {path:?}: {e}"))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Builds the kernel with the test key's public half (`GALA_TRUST_KEY`),
/// unless `XTASK_UNSIGNED` is set.
pub fn kernel_env(cmd: &mut Command) -> Result {
    if unsigned() {
        cmd.env_remove("GALA_TRUST_KEY");
    } else {
        cmd.env("GALA_TRUST_KEY", hex(&test_key()?.pk[..]));
    }
    Ok(())
}

/// Hashes `files` with a coreutils tool (`b2sum`, `sha256sum`): one
/// process for all of them, hex digests in order.
fn hash_files(tool: &str, files: &[PathBuf]) -> Result<Vec<String>> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let out = Command::new(tool)
        .arg("--")
        .args(files)
        .output()
        .map_err(|e| format!("running {tool}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{tool} failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let digests: Vec<String> =
        text.lines().filter_map(|l| l.split_whitespace().next()).map(str::to_string).collect();
    if digests.len() != files.len() {
        return Err(format!("{tool}: {} digests for {} files", digests.len(), files.len()));
    }
    Ok(digests)
}

fn unhex32(s: &str) -> Result<[u8; 32]> {
    let mut out = [0u8; 32];
    if s.len() != 64 {
        return Err(format!("not a SHA-256: {s}"));
    }
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Seals the image tree `iso_root` whose limine.conf is `config` (not
/// written yet): the signed manifest, the config's hashes, the enrolled
/// executables; then what `signing` changes on purpose.
pub fn seal(iso_root: &Path, config: &str, signing: &Signing) -> Result<String> {
    let boot = iso_root.join("boot");
    let conf_path = boot.join("limine/limine.conf");
    if unsigned() {
        fs::write(&conf_path, config).map_err(|e| format!("writing {conf_path:?}: {e}"))?;
        return Ok(config.to_string());
    }
    let mut config = config.to_string();
    if signing.unsigned_kernel {
        let mut image =
            fs::read(boot.join("kernel")).map_err(|e| format!("reading kernel: {e}"))?;
        image.extend_from_slice(b"UNSIGNED");
        fs::write(boot.join("kernel-unsigned"), image)
            .map_err(|e| format!("writing kernel-unsigned: {e}"))?;
        config += "    module_path: boot():/boot/kernel-unsigned\n";
    }
    // Step 4-5: the manifest of every file in /boot (the kernel and the
    // modules), but those left out on purpose.
    let mut names: Vec<String> = fs::read_dir(&boot)
        .map_err(|e| format!("reading {boot:?}: {e}"))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !signing.leave_out.contains(&n.as_str()))
        .collect();
    // The 32-bit DLLs (boot/x86: their names are often the 64-bit ones').
    if let Ok(dir) = fs::read_dir(boot.join("x86")) {
        names.extend(
            dir.filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .filter_map(|e| e.file_name().into_string().ok())
                .map(|n| format!("x86/{n}")),
        );
    }
    names.sort();
    let paths: Vec<PathBuf> = names.iter().map(|n| boot.join(n)).collect();
    let sha = hash_files("sha256sum", &paths)?;
    let mut entries = Vec::new();
    let full_paths: Vec<String> = names.iter().map(|n| format!("/boot/{n}")).collect();
    for ((path, file), hash) in full_paths.iter().zip(&paths).zip(&sha) {
        let size = fs::metadata(file).map_err(|e| format!("{file:?}: {e}"))?.len();
        entries.push(manifest::Entry { path, size, sha256: unhex32(hash)? });
    }
    let key = test_key()?;
    let header = manifest::Header {
        release: signing.release,
        floor: signing.floor,
        key_id: manifest::key_id(&key.pk),
    };
    let mut bytes = vec![0u8; manifest::encoded_len(&entries)];
    manifest::encode(&mut bytes, &header, &entries, |m| *key.sk.sign(m, None))
        .ok_or("manifest: a path is too long")?;
    fs::write(boot.join("gala.manifest"), &bytes)
        .map_err(|e| format!("writing gala.manifest: {e}"))?;
    config += "    module_path: boot():/boot/gala.manifest\n    module_string: manifest\n";

    // Steps 2-3: `#<BLAKE2B>` after every path, Limine refusing a mismatch.
    let mut referenced = Vec::new();
    for line in config.lines() {
        if let Some((_, rest)) = line.split_once("boot():/") {
            referenced.push(iso_root.join(rest));
        }
    }
    let b2 = hash_files("b2sum", &referenced)?;
    let mut hashes = b2.iter();
    let mut sealed = String::new();
    let header_lines = if signing.limine_serial {
        "hash_mismatch_panic: yes\nserial: yes\n"
    } else {
        "hash_mismatch_panic: yes\n"
    };
    for (i, line) in config.lines().enumerate() {
        sealed += line;
        if line.contains("boot():/") {
            sealed += "#";
            sealed += hashes.next().ok_or("b2sum: a digest is missing")?;
        }
        sealed += "\n";
        if i == 0 {
            sealed += header_lines;
        }
    }
    fs::write(&conf_path, &sealed).map_err(|e| format!("writing {conf_path:?}: {e}"))?;
    let conf_hash = hash_files("b2sum", std::slice::from_ref(&conf_path))?.remove(0);
    enroll(iso_root, &conf_hash)?;

    // What a scenario changes after sealing, for Limine to find.
    if let Some(name) = signing.tamper {
        let path = boot.join(name);
        let mut b = fs::read(&path).map_err(|e| format!("reading {path:?}: {e}"))?;
        let at = b.len() / 2;
        b[at] ^= 0x01;
        fs::write(&path, b).map_err(|e| format!("writing {path:?}: {e}"))?;
    }
    if signing.edit_config {
        let edited = sealed.replacen("timeout: 0", "timeout: 1", 1);
        fs::write(&conf_path, &edited).map_err(|e| format!("writing {conf_path:?}: {e}"))?;
        return Ok(edited);
    }
    Ok(sealed)
}

/// Where Limine keeps the enrolled config hash: right after this marker in
/// each of its executables (`limine enroll-config` writes there).
const CONFIG_HASH_MARKER: &[u8] = b"++CONFIG_B2SUM_SIGNATURE++";

/// Enrolls the config's BLAKE2B (128 hex digits) in the image's Limine
/// executables: `limine enroll-config` on BOOTX64.EFI (from the ISO's
/// file system) and limine-bios.sys (BIOS); limine-uefi-cd.bin (the EFI
/// system partition image that UEFI boots from a CD or USB stick) holds
/// the executables for every architecture, each patched the same way.
fn enroll(iso_root: &Path, hash: &str) -> Result {
    let limine = crate::fetch_limine()?.join("limine");
    for exe in [iso_root.join("EFI/BOOT/BOOTX64.EFI"), iso_root.join("boot/limine/limine-bios.sys")]
    {
        crate::run_quiet(
            Command::new(&limine).arg("enroll-config").arg("--quiet").arg(&exe).arg(hash),
        )?;
    }
    let esp = iso_root.join("boot/limine/limine-uefi-cd.bin");
    let mut image = fs::read(&esp).map_err(|e| format!("reading {esp:?}: {e}"))?;
    let n = enroll_bytes(&mut image, hash.as_bytes());
    if n == 0 {
        return Err(format!("{}: no Limine executable to enroll", esp.display()));
    }
    fs::write(&esp, image).map_err(|e| format!("writing {esp:?}: {e}"))
}

/// Writes `hash` after every CONFIG_HASH_MARKER in `image`; how many.
fn enroll_bytes(image: &mut [u8], hash: &[u8]) -> usize {
    let mut n = 0;
    let mut at = 0;
    while let Some(i) =
        image[at..].windows(CONFIG_HASH_MARKER.len()).position(|w| w == CONFIG_HASH_MARKER)
    {
        let start = at + i + CONFIG_HASH_MARKER.len();
        if start + hash.len() > image.len() {
            break;
        }
        image[start..start + hash.len()].copy_from_slice(hash);
        n += 1;
        at = start + hash.len();
    }
    n
}
