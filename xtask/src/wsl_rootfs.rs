//! The WSL scenarios' distribution (ROADMAP CT step 2): a busybox root
//! file system as a rootfs tarball, `target/wsl/bb-rootfs.tar` and the
//! same gzip'd (`.tar.gz`), as `wsl --import` takes them. Made from the
//! `busybox-linux` PREBUILT (busybox.net's static musl build): /bin/busybox,
//! its applets as symbolic links to it (and one hard link, `bin/bb`),
//! /etc/passwd (root, shell /bin/sh, home /root), /etc/group,
//! /etc/profile (the prompt), /root (0700), /tmp (1777). Deterministic:
//! fixed times, owners root, entries in a fixed order, gzip without a
//! name or time; user/wsl_test documents what it expects of it.

use std::fs;
use std::path::PathBuf;

/// The applets linked in /bin.
const APPLETS: &[&str] = &[
    "sh", "ash", "cat", "echo", "env", "false", "grep", "head", "hostname", "id", "ls", "mkdir",
    "printf", "pwd", "rm", "sleep", "test", "touch", "true", "uname", "wc", "whoami",
];

/// 2024-01-01 00:00:00 UTC.
const MTIME: u64 = 1_704_067_200;

enum Member<'a> {
    Dir(u32),
    File(u32, &'a [u8]),
    Symlink(&'a str),
    HardLink(&'a str),
}

fn octal(field: &mut [u8], v: u64) {
    let s = format!("{v:0w$o}", w = field.len() - 1);
    field[..s.len()].copy_from_slice(s.as_bytes());
}

fn member(out: &mut Vec<u8>, path: &str, m: &Member<'_>) {
    let mut h = [0u8; 512];
    let name = if matches!(m, Member::Dir(_)) { format!("{path}/") } else { path.to_string() };
    h[..name.len()].copy_from_slice(name.as_bytes());
    let (mode, size, kind, link): (u32, usize, u8, &str) = match m {
        Member::Dir(mode) => (*mode, 0, b'5', ""),
        Member::File(mode, data) => (*mode, data.len(), b'0', ""),
        Member::Symlink(to) => (0o777, 0, b'2', to),
        Member::HardLink(to) => (0o755, 0, b'1', to),
    };
    octal(&mut h[100..108], u64::from(mode));
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    octal(&mut h[124..136], size as u64);
    octal(&mut h[136..148], MTIME);
    h[156] = kind;
    h[157..157 + link.len()].copy_from_slice(link.as_bytes());
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[265..269].copy_from_slice(b"root");
    h[297..301].copy_from_slice(b"root");
    h[148..156].copy_from_slice(b"        ");
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    let s = format!("{sum:06o}\0 ");
    h[148..156].copy_from_slice(s.as_bytes());
    out.extend_from_slice(&h);
    if let Member::File(_, data) = m {
        out.extend_from_slice(data);
        out.resize(out.len().next_multiple_of(512), 0);
    }
}

/// Makes `target/wsl/bb-rootfs.tar` and `bb-rootfs.tar.gz` (again each
/// time: they are cheap and follow the busybox download).
pub fn make(root: &std::path::Path, busybox: &[u8]) -> Result<PathBuf, String> {
    let mut tar = Vec::new();
    member(&mut tar, ".", &Member::Dir(0o755));
    member(&mut tar, "./bin", &Member::Dir(0o755));
    member(&mut tar, "./bin/busybox", &Member::File(0o755, busybox));
    for applet in APPLETS {
        member(&mut tar, &format!("./bin/{applet}"), &Member::Symlink("busybox"));
    }
    member(&mut tar, "./bin/bb", &Member::HardLink("./bin/busybox"));
    member(&mut tar, "./etc", &Member::Dir(0o755));
    member(&mut tar, "./etc/passwd", &Member::File(0o644, b"root:x:0:0:root:/root:/bin/sh\n"));
    member(&mut tar, "./etc/group", &Member::File(0o644, b"root:x:0:\n"));
    member(&mut tar, "./etc/profile", &Member::File(0o644, b"export PS1='root@bb:\\w# '\n"));
    member(&mut tar, "./root", &Member::Dir(0o700));
    member(&mut tar, "./tmp", &Member::Dir(0o1777));
    tar.extend_from_slice(&[0u8; 1024]);
    let dir = root.join("target/wsl");
    fs::create_dir_all(&dir).map_err(|e| format!("creating {dir:?}: {e}"))?;
    let plain = dir.join("bb-rootfs.tar");
    fs::write(&plain, &tar).map_err(|e| format!("writing {plain:?}: {e}"))?;
    let gz = {
        use flate2::Compression;
        use flate2::GzBuilder;
        use std::io::Write;
        let mut z = GzBuilder::new().mtime(0).write(Vec::new(), Compression::default());
        z.write_all(&tar).and_then(|()| z.finish()).map_err(|e| format!("gzip: {e}"))?
    };
    let gz_path = dir.join("bb-rootfs.tar.gz");
    fs::write(&gz_path, gz).map_err(|e| format!("writing {gz_path:?}: {e}"))?;
    Ok(dir)
}

/// A rootfs tarball (`.tar.gz`, deterministic as `make`'s) of `files`:
/// (path in the root, its bytes), with every parent directory (0755)
/// before its entries, plus what a distribution has: /etc/passwd (root,
/// shell /bin/sh, home /root; the user `gala`, uid 1000, home
/// /home/gala), /etc/group, /root (0700), /tmp (1777).
/// Programs (`usr/bin`, `bin`, ld.so) are 0755, the rest 0644.
pub fn make_tree(files: &[(String, Vec<u8>)], out: &std::path::Path) -> Result<(), String> {
    use std::collections::BTreeSet;
    let mut dirs = BTreeSet::new();
    for (path, _) in files {
        let mut at = path.as_str();
        while let Some((parent, _)) = at.rsplit_once('/') {
            dirs.insert(parent.to_string());
            at = parent;
        }
    }
    for d in ["etc", "home", "home/gala", "root", "tmp"] {
        dirs.insert(d.to_string());
    }
    let mut tar = Vec::new();
    member(&mut tar, ".", &Member::Dir(0o755));
    for d in &dirs {
        let mode = match d.as_str() {
            "root" => 0o700,
            "tmp" => 0o1777,
            _ => 0o755,
        };
        member(&mut tar, &format!("./{d}"), &Member::Dir(mode));
    }
    let mut sorted: Vec<&(String, Vec<u8>)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, data) in sorted {
        let program = path.starts_with("usr/bin/")
            || path.starts_with("bin/")
            || path.ends_with("ld-linux-x86-64.so.2")
            || path.ends_with(".sh");
        let mode = if program { 0o755 } else { 0o644 };
        member(&mut tar, &format!("./{path}"), &Member::File(mode, data));
    }
    // The user a WSL distribution's setup adds (uid 1000), as whom gala
    // runs its processes (the Windows boot user's Linux ids): programs
    // that look their user up (Xwayland's local access) find it.
    member(
        &mut tar,
        "./etc/passwd",
        &Member::File(
            0o644,
            b"root:x:0:0:root:/root:/bin/sh\ngala:x:1000:1000:gala:/home/gala:/bin/sh\n",
        ),
    );
    member(&mut tar, "./etc/group", &Member::File(0o644, b"root:x:0:\ngala:x:1000:\n"));
    tar.extend_from_slice(&[0u8; 1024]);
    let gz = {
        use flate2::Compression;
        use flate2::GzBuilder;
        use std::io::Write;
        let mut z = GzBuilder::new().mtime(0).write(Vec::new(), Compression::default());
        z.write_all(&tar).and_then(|()| z.finish()).map_err(|e| format!("gzip: {e}"))?
    };
    fs::write(out, gz).map_err(|e| format!("writing {out:?}: {e}"))
}
