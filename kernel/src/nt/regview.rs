//! The registry's 32-bit view (WOW64), as Microsoft Learn's "Registry
//! Redirector" and "Registry Keys Affected by WOW64" describe it for
//! Windows 7 and later: a 32-bit program's names under
//! `HKLM\SOFTWARE` lead to `HKLM\SOFTWARE\WOW6432Node`, except the shared
//! keys (the table below, with their subkeys); `SOFTWARE\Classes` is
//! shared but for its CLSID, DirectShow, Interface, Media Type and
//! MediaFoundation subkeys, which lead to `Classes\WOW6432Node\<key>`,
//! in HKLM and in each user's `Software\Classes` alike. Everything else
//! (HKCU's Software included) is shared.
//!
//! The view is chosen per open or create (registry_calls: a WOW64
//! process's names are in the 32-bit view unless the access mask has
//! KEY_WOW64_64KEY; any process's with KEY_WOW64_32KEY), on the full name
//! (the RootDirectory key's name and the relative one), so opens relative
//! to a handle are redirected as absolute ones. A name already in
//! `WOW6432Node` is taken as the 32-bit one it names: a shared key below
//! it is the shared key (so `SOFTWARE`, opened by a 32-bit program, then
//! `Microsoft\Windows NT\CurrentVersion\ProfileList` below it is the
//! machine's one list), the rest stays. Not modelled: the reflection of
//! Windows Vista and earlier, WOW6432Node's own `Classes` link to
//! `Classes\WOW6432Node` (a 32-bit name through it is taken as
//! `SOFTWARE\Classes`'s), and `HKEY_USERS\<SID>_Classes`.

use alloc::vec::Vec;

use crate::mm::heap::{AllocError, TryVec};

const BACKSLASH: u16 = b'\\' as u16;

/// The redirected node's name.
const NODE: &str = "WOW6432Node";

/// The keys under `HKLM\SOFTWARE` shared by both views (with their
/// subkeys), Windows 7 and later.
const SHARED: &[&str] = &[
    "Clients",
    "Microsoft\\COM3",
    "Microsoft\\Cryptography\\Calais\\Current",
    "Microsoft\\Cryptography\\Calais\\Readers",
    "Microsoft\\Cryptography\\Services",
    "Microsoft\\CTF\\SystemShared",
    "Microsoft\\CTF\\TIP",
    "Microsoft\\DFS",
    "Microsoft\\Driver Signing",
    "Microsoft\\EnterpriseCertificates",
    "Microsoft\\EventSystem",
    "Microsoft\\MSMQ",
    "Microsoft\\Non-Driver Signing",
    "Microsoft\\Notepad\\DefaultFonts",
    "Microsoft\\OLE",
    "Microsoft\\RAS",
    "Microsoft\\RPC",
    "Microsoft\\SOFTWARE\\Microsoft\\Shared Tools\\MSInfo",
    "Microsoft\\SystemCertificates",
    "Microsoft\\TermServLicensing",
    "Microsoft\\TransactionServer",
    "Microsoft\\Windows\\CurrentVersion\\App Paths",
    "Microsoft\\Windows\\CurrentVersion\\Control Panel\\Cursors\\Schemes",
    "Microsoft\\Windows\\CurrentVersion\\Explorer\\AutoplayHandlers",
    "Microsoft\\Windows\\CurrentVersion\\Explorer\\DriveIcons",
    "Microsoft\\Windows\\CurrentVersion\\Explorer\\KindMap",
    "Microsoft\\Windows\\CurrentVersion\\Group Policy",
    "Microsoft\\Windows\\CurrentVersion\\Policies",
    "Microsoft\\Windows\\CurrentVersion\\PreviewHandlers",
    "Microsoft\\Windows\\CurrentVersion\\Setup",
    "Microsoft\\Windows\\CurrentVersion\\Telephony\\Locations",
    "Microsoft\\Windows NT\\CurrentVersion\\Console",
    "Microsoft\\Windows NT\\CurrentVersion\\FontDpi",
    "Microsoft\\Windows NT\\CurrentVersion\\FontLink",
    "Microsoft\\Windows NT\\CurrentVersion\\FontMapper",
    "Microsoft\\Windows NT\\CurrentVersion\\Fonts",
    "Microsoft\\Windows NT\\CurrentVersion\\FontSubstitutes",
    "Microsoft\\Windows NT\\CurrentVersion\\Gre_Initialize",
    "Microsoft\\Windows NT\\CurrentVersion\\Image File Execution Options",
    "Microsoft\\Windows NT\\CurrentVersion\\Language Pack",
    "Microsoft\\Windows NT\\CurrentVersion\\NetworkCards",
    "Microsoft\\Windows NT\\CurrentVersion\\Perflib",
    "Microsoft\\Windows NT\\CurrentVersion\\Ports",
    "Microsoft\\Windows NT\\CurrentVersion\\Print",
    "Microsoft\\Windows NT\\CurrentVersion\\ProfileList",
    "Microsoft\\Windows NT\\CurrentVersion\\Time Zones",
    "Policies",
    "RegisteredApplications",
];

/// The subkeys of `Software\Classes` that are redirected (to
/// `Classes\WOW6432Node\<key>`).
const CLASSES_REDIRECTED: &[&str] =
    &["CLSID", "DirectShow", "Interface", "Media Type", "MediaFoundation"];

/// A name in the 32-bit view: `path`, relative to `\REGISTRY`, and the
/// length of its prefix through the WOW6432Node component (which a create
/// makes first if it is missing; 0 when the path has none).
#[derive(Debug, PartialEq, Eq)]
pub struct Redirected {
    pub path: Vec<u16>,
    pub node: usize,
}

/// ASCII case-insensitive comparison of a name component.
fn is(component: &[u16], name: &str) -> bool {
    component.len() == name.len()
        && component
            .iter()
            .zip(name.bytes())
            .all(|(&c, b)| c < 0x80 && (c as u8).eq_ignore_ascii_case(&b))
}

/// Whether `components` start with the components of `prefix`.
fn starts_with(components: &[&[u16]], prefix: &str) -> bool {
    let mut n = 0;
    for name in prefix.split('\\') {
        match components.get(n) {
            Some(c) if is(c, name) => n += 1,
            _ => return false,
        }
    }
    true
}

/// The 32-bit view's name for the full name `full` (`\REGISTRY\...`), or
/// None when it is the same key in both views.
pub fn redirect(full: &[u16]) -> Result<Option<Redirected>, AllocError> {
    // The first components; anything below is copied as it is.
    let mut c: [&[u16]; 12] = [&[]; 12];
    let mut n = 0;
    let mut tail_at = full.len();
    let mut at = 0;
    for part in full.split(|&ch| ch == BACKSLASH) {
        if !part.is_empty() {
            if n == c.len() {
                tail_at = at;
                break;
            }
            c[n] = part;
            n += 1;
        }
        at += part.len() + 1;
    }
    let c = &c[..n];
    if c.len() < 3 || !is(c[0], "REGISTRY") {
        return Ok(None);
    }
    // Where WOW6432Node goes (a component index), if anywhere, and which
    // components the 32-bit view drops (an existing WOW6432Node taken as
    // the logical name).
    let (insert, skip) = if is(c[1], "MACHINE") && is(c[2], "SOFTWARE") {
        let had = c.get(3).is_some_and(|x| is(x, NODE));
        let logical = &c[if had { 4 } else { 3 }..];
        let skip = if had { Some(3) } else { None };
        if logical.first().is_some_and(|x| is(x, "Classes")) {
            match classes(&c[3 + usize::from(had)..], 3 + usize::from(had)) {
                Some(i) => (Some(i), skip),
                None => (None, skip),
            }
        } else if SHARED.iter().any(|s| starts_with(logical, s)) {
            (None, skip)
        } else if had {
            return Ok(None);
        } else {
            (Some(3), None)
        }
    } else if is(c[1], "USER") && c.len() > 4 && is(c[3], "Software") && is(c[4], "Classes") {
        match classes(&c[4..], 4) {
            Some(i) => (Some(i), None),
            None => return Ok(None),
        }
    } else {
        return Ok(None);
    };
    if insert.is_none() && skip.is_none() {
        return Ok(None);
    }
    let mut path = Vec::new();
    path.try_reserve(full.len() + NODE.len() + 2).map_err(|_| AllocError)?;
    let mut node = 0;
    for (i, part) in c.iter().enumerate().skip(1) {
        if Some(i) == skip {
            continue;
        }
        if Some(i) == insert {
            push(&mut path, NODE.encode_utf16())?;
            node = path.len();
        }
        push(&mut path, part.iter().copied())?;
    }
    if insert == Some(c.len()) {
        push(&mut path, NODE.encode_utf16())?;
        node = path.len();
    }
    if tail_at < full.len() {
        push(&mut path, full[tail_at..].iter().copied())?;
    }
    Ok(Some(Redirected { path, node }))
}

/// The `Classes` components `c` (c[0] is `Classes`, at index `base` of the
/// full name): where WOW6432Node goes for a redirected subkey, None for a
/// shared one or one already in `Classes\WOW6432Node`.
fn classes(c: &[&[u16]], base: usize) -> Option<usize> {
    let sub = c.get(1)?;
    CLASSES_REDIRECTED.iter().any(|name| is(sub, name)).then_some(base + 1)
}

/// Appends `\` (unless at the start) and `part` to `path`.
fn push(path: &mut Vec<u16>, part: impl Iterator<Item = u16>) -> Result<(), AllocError> {
    if !path.is_empty() {
        path.try_push(BACKSLASH)?;
    }
    for ch in part {
        path.try_push(ch)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    fn view(full: &str) -> Option<(String, String)> {
        let w: Vec<u16> = full.encode_utf16().collect();
        redirect(&w).unwrap().map(|r| {
            (String::from_utf16_lossy(&r.path), String::from_utf16_lossy(&r.path[..r.node]))
        })
    }

    fn to(path: &str, node: &str) -> Option<(String, String)> {
        Some((path.into(), node.into()))
    }

    #[test]
    fn software_is_redirected() {
        let n = "MACHINE\\SOFTWARE\\WOW6432Node";
        assert_eq!(view("\\REGISTRY\\MACHINE\\SOFTWARE"), to(n, n));
        assert_eq!(
            view("\\REGISTRY\\Machine\\software\\Vendor\\App"),
            to("Machine\\software\\WOW6432Node\\Vendor\\App", "Machine\\software\\WOW6432Node")
        );
        assert_eq!(
            view("\\REGISTRY\\MACHINE\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"),
            to(&format!("{n}\\Microsoft\\Windows NT\\CurrentVersion"), n)
        );
    }

    #[test]
    fn already_in_the_node_stays() {
        assert_eq!(view("\\REGISTRY\\MACHINE\\SOFTWARE\\WOW6432Node"), None);
        assert_eq!(view("\\REGISTRY\\MACHINE\\SOFTWARE\\wow6432node\\Vendor"), None);
    }

    #[test]
    fn shared_keys_are_not() {
        assert_eq!(view("\\REGISTRY\\MACHINE\\SOFTWARE\\Policies\\Microsoft"), None);
        assert_eq!(
            view(
                "\\REGISTRY\\MACHINE\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\ProfileList\\S-1-5-18"
            ),
            None
        );
        assert_eq!(view("\\REGISTRY\\MACHINE\\SOFTWARE\\Classes\\.txt"), None);
        assert_eq!(view("\\REGISTRY\\MACHINE\\SYSTEM\\CurrentControlSet"), None);
        assert_eq!(view("\\REGISTRY\\USER\\S-1-5-18\\Software\\Vendor"), None);
        // A shared key named through WOW6432Node is the shared key.
        assert_eq!(
            view(
                "\\REGISTRY\\MACHINE\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows NT\\CurrentVersion\\Time Zones\\UTC"
            ),
            to("MACHINE\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Time Zones\\UTC", "")
        );
    }

    #[test]
    fn classes_redirect_only_their_listed_subkeys() {
        assert_eq!(
            view("\\REGISTRY\\MACHINE\\SOFTWARE\\Classes\\CLSID\\{x}"),
            to(
                "MACHINE\\SOFTWARE\\Classes\\WOW6432Node\\CLSID\\{x}",
                "MACHINE\\SOFTWARE\\Classes\\WOW6432Node"
            )
        );
        assert_eq!(
            view("\\REGISTRY\\USER\\S-1-5-18\\Software\\Classes\\Interface"),
            to(
                "USER\\S-1-5-18\\Software\\Classes\\WOW6432Node\\Interface",
                "USER\\S-1-5-18\\Software\\Classes\\WOW6432Node"
            )
        );
        assert_eq!(view("\\REGISTRY\\MACHINE\\SOFTWARE\\Classes\\WOW6432Node\\CLSID"), None);
        assert_eq!(view("\\REGISTRY\\USER\\S-1-5-18\\Software\\Classes"), None);
    }

    #[test]
    fn deep_names_keep_their_tail() {
        let deep = "\\REGISTRY\\MACHINE\\SOFTWARE\\a\\b\\c\\d\\e\\f\\g\\h\\i\\j\\k";
        assert_eq!(
            view(deep),
            to(
                "MACHINE\\SOFTWARE\\WOW6432Node\\a\\b\\c\\d\\e\\f\\g\\h\\i\\j\\k",
                "MACHINE\\SOFTWARE\\WOW6432Node"
            )
        );
    }
}
