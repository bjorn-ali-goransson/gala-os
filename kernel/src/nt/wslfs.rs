//! `\\wsl.localhost\<distro>\...` and `\\wsl$\<distro>\...` for every
//! Windows program (ROADMAP CT step 3's file part, EX step 1;
//! ARCHITECTURE "The WSL facade", Files): a WSL distribution's tree seen
//! from NT, with no 9P protocol in between. Win32's UNC paths reach the
//! kernel as `\??\UNC\server\share\...`; `\??\UNC` is a link to
//! `\Device\Mup` (the multiple UNC provider's device, as on Windows), and
//! an open below it lands here: the server is `wsl.localhost` or `wsl$`
//! (WSL's two names, ASCII case ignored), the share a distribution
//! registered for the caller's user (its Lxss key's DistributionName,
//! case ignored as WSL does), and the rest a path in that distribution's
//! own tree (`<BasePath>\rootfs`, the nodes its Linux processes see).
//!
//! The path below the share is a Linux path: case-sensitive (the tree's
//! directories are), `/`-separated once taken from NT's `\`, walked as a
//! process rooted at the distribution would walk it (`lx::file`): its
//! symbolic links are followed inside the distribution (an absolute
//! target starts at its root, `..` of its root is itself), as WSL's
//! server follows them for Windows; the last one only when the open
//! follows links (not with FILE_OPEN_REPARSE_POINT, so deleting a link
//! deletes the link).
//!
//! Errors as Windows' redirector gives them: a server other than WSL's is
//! STATUS_BAD_NETWORK_PATH (gala has no SMB client: ledgered), an
//! unknown distribution or none STATUS_BAD_NETWORK_NAME.

use alloc::vec::Vec;

use super::file::FilePath;
use super::status::NtStatus;
use crate::fs::ramfs::NodeId;
use crate::mm::heap::TryVec;

/// WSL's server names (`\\wsl.localhost`, the older `\\wsl$`).
pub const SERVERS: [&str; 2] = ["wsl.localhost", "wsl$"];

/// The device UNC paths reach, and the DOS name that links to it.
pub const MUP_DEVICE: &str = "\\Device\\Mup";
pub const UNC_LINK: &str = "\\??\\UNC";

fn same_ascii(a: &[u16], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(&c, &d)| u8::try_from(c).is_ok_and(|c| c.eq_ignore_ascii_case(&d)))
}

/// `\server\share\rest` split: (server, share, rest without its leading
/// backslash). A missing share is empty.
pub fn split(rest: &[u16]) -> (&[u16], &[u16], &[u16]) {
    let bs = u16::from(b'\\');
    let body = rest.strip_prefix(&[bs]).unwrap_or(rest);
    let (server, after) = match body.iter().position(|&c| c == bs) {
        Some(i) => (&body[..i], &body[i + 1..]),
        None => (body, &[][..]),
    };
    let (share, path) = match after.iter().position(|&c| c == bs) {
        Some(i) => (&after[..i], &after[i + 1..]),
        None => (after, &[][..]),
    };
    (server, share, path)
}

/// Whether `server` is one of WSL's names.
pub fn is_wsl_server(server: &[u16]) -> bool {
    SERVERS.iter().any(|s| same_ascii(server, s.as_bytes()))
}

/// The tree of distribution `share` among `roots` (names compared with
/// ASCII case ignored).
pub fn find_share<T: AsRef<[u8]>>(roots: &[(T, NodeId)], share: &[u16]) -> Option<NodeId> {
    roots.iter().find(|(name, _)| same_ascii(share, name.as_ref())).map(|&(_, node)| node)
}

/// A path below the share (NT's `\` separators, UTF-16) as the Linux
/// path walked from the distribution's root (`/a/b`, UTF-8).
pub fn linux_path(rest: &[u16]) -> Result<Vec<u8>, NtStatus> {
    let mut out = Vec::new();
    out.try_push(b'/').map_err(|_| NtStatus::NO_MEMORY)?;
    for c in char::decode_utf16(rest.iter().copied()) {
        let c = c.map_err(|_| NtStatus::OBJECT_NAME_INVALID)?;
        let c = if c == '\\' { '/' } else { c };
        let mut b = [0u8; 4];
        out.try_extend_from_slice(c.encode_utf8(&mut b).as_bytes())
            .map_err(|_| NtStatus::NO_MEMORY)?;
    }
    Ok(out)
}

/// The path an open of `rest` (what follows `\Device\Mup`) names, for a
/// caller whose token user's SID string is `sid`: the distribution's
/// root and the path below it, still to be walked (`resolve`).
pub fn share_path(sid: &[u8], rest: &[u16]) -> Result<FilePath, NtStatus> {
    let (server, share, path) = split(rest);
    if !is_wsl_server(server) {
        super::ledger::record("UNC path", "server other than WSL's (no SMB client)", 0);
        return Err(NtStatus::BAD_NETWORK_PATH);
    }
    if share.is_empty() {
        return Err(NtStatus::BAD_NETWORK_NAME);
    }
    let mut hive: Vec<u8> = Vec::new();
    hive.try_extend_from_slice(b"USER\\").map_err(|_| NtStatus::NO_MEMORY)?;
    hive.try_extend_from_slice(sid).map_err(|_| NtStatus::NO_MEMORY)?;
    let hive = core::str::from_utf8(&hive).map_err(|_| NtStatus::INVALID_PARAMETER)?;
    let roots = super::desktop::distro_roots_in(hive);
    let names: Vec<(&[u8], NodeId)> = {
        let mut v = Vec::new();
        for (n, node) in &roots {
            v.try_push((n.as_bytes(), *node)).map_err(|_| NtStatus::NO_MEMORY)?;
        }
        v
    };
    let Some(root) = find_share(&names, share) else {
        return Err(NtStatus::BAD_NETWORK_NAME);
    };
    let name = crate::mm::heap::try_to_vec(path).map_err(|_| NtStatus::NO_MEMORY)?;
    Ok(FilePath { start: root, name, linux: Some(root) })
}

/// Walks a path made by `share_path` inside its distribution (Linux
/// links followed there; the last one if `follow_final`): the directory
/// that holds what it names and that name (empty: the directory itself).
pub fn resolve(path: &FilePath, follow_final: bool) -> Result<FilePath, NtStatus> {
    let root = path.linux.ok_or(NtStatus::INVALID_PARAMETER)?;
    let linux = linux_path(&path.name)?;
    let found = crate::lx::file::resolve_in(root, &linux, follow_final).map_err(|e| match e {
        crate::lx::errno::ENOENT | crate::lx::errno::ENOTDIR => NtStatus::OBJECT_PATH_NOT_FOUND,
        crate::lx::errno::ENOMEM => NtStatus::NO_MEMORY,
        crate::lx::errno::ELOOP => NtStatus::REPARSE_POINT_NOT_RESOLVED,
        _ => NtStatus::OBJECT_NAME_INVALID,
    })?;
    Ok(match (found.name, found.node) {
        (Some(name), _) => FilePath { start: found.dir, name, linux: None },
        (None, Some(node)) => FilePath { start: node, name: Vec::new(), linux: None },
        (None, None) => return Err(NtStatus::OBJECT_PATH_NOT_FOUND),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn splits_server_share_and_rest() {
        let p = w("\\wsl.localhost\\bb\\etc\\passwd");
        let (server, share, rest) = split(&p);
        assert_eq!(
            (server, share, rest),
            (&w("wsl.localhost")[..], &w("bb")[..], &w("etc\\passwd")[..])
        );
        let p = w("\\wsl$\\bb");
        assert_eq!(split(&p), (&w("wsl$")[..], &w("bb")[..], &[][..]));
        let p = w("\\wsl.localhost");
        assert_eq!(split(&p).1, &[][..]);
    }

    #[test]
    fn wsl_servers_ignore_ascii_case() {
        assert!(is_wsl_server(&w("WSL.LocalHost")));
        assert!(is_wsl_server(&w("WSL$")));
        assert!(!is_wsl_server(&w("wsl")));
        assert!(!is_wsl_server(&w("fileserver")));
    }

    #[test]
    fn shares_are_distributions_by_name() {
        let roots: [(&[u8], NodeId); 2] = [(b"bb", 7), (b"Ubuntu", 9)];
        assert_eq!(find_share(&roots, &w("BB")), Some(7));
        assert_eq!(find_share(&roots, &w("ubuntu")), Some(9));
        assert_eq!(find_share(&roots, &w("alpine")), None);
    }

    #[test]
    fn rest_becomes_a_linux_path() {
        assert_eq!(linux_path(&w("etc\\passwd")).unwrap(), b"/etc/passwd");
        assert_eq!(linux_path(&[]).unwrap(), b"/");
        assert_eq!(linux_path(&w("caf\u{e9}")).unwrap(), "/caf\u{e9}".as_bytes());
        assert!(linux_path(&[0xd800]).is_err());
    }
}
