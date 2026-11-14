//! The start menu's "Linux distributions" section (ROADMAP LD step 7;
//! ARCHITECTURE "Start menu"): the distributions gala suggests and those
//! installed through the WSL facade, each a tile. A suggestion not
//! installed installs on a click (its rootfs fetched and imported with
//! `wsl --import` by `gala linux-install`, its progress on the tile); an
//! installed one starts its desktop (LD step 6's session); its context
//! menu opens a terminal (`wsl -d <name>`) or starts one of its
//! applications (its `.desktop` files). The section can be collapsed or
//! hidden and each suggestion dismissed: the user's choices (`Prefs`)
//! are kept in their registry hive.
//!
//! This module is the model: what the section lists (`Listing`, read by
//! the desktop's caller outside its lock: the registry and the
//! distributions' files), the choices, an install's progress, the
//! context menu, and the parsing of `.desktop` files. gala's UI says
//! "Linux", never "WSL" (ARCHITECTURE "The WSL facade", Naming).

use crate::mm::heap::TryVec;
use alloc::vec::Vec;

/// A distribution gala suggests: its name (as `wsl --import` registers
/// it), the URL of its rootfs tarball as the distribution publishes it
/// for WSL, and its tile's colours.
pub struct Suggestion {
    pub name: &'static str,
    pub url: &'static str,
    pub colors: (u32, u32),
}

/// The suggestions, in the order the section shows them. The boot option
/// `linux_mirror=<base URL>` fetches each from `<base>/<file name>`
/// instead (the scenario's host serves stand-ins there: WORKFLOW).
pub const SUGGESTIONS: &[Suggestion] = &[
    // Canonical's WSL image of Ubuntu 24.04 LTS.
    Suggestion {
        name: "Ubuntu",
        url: "https://cloud-images.ubuntu.com/wsl/releases/24.04/current/ubuntu-noble-wsl-amd64-wsl.rootfs.tar.gz",
        colors: (0x00_f0_7a_4a, 0x00_c8_46_1e),
    },
    // Alpine's mini root file system (what its WSL builds start from).
    Suggestion {
        name: "Alpine",
        url: "https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/x86_64/alpine-minirootfs-3.20.3-x86_64.tar.gz",
        colors: (0x00_3d_8f_d6, 0x00_0d_59_a3),
    },
];

/// The tiles the section shows at most (one row of the menu's grid).
pub const MAX_ENTRIES: usize = 6;
/// The applications a distribution's context menu lists at most.
pub const MAX_APPS: usize = 8;
/// A distribution's name (WSL allows letters, digits, `.`, `_`, `-`).
pub const NAME: usize = 32;
/// An application's name and command line.
pub const APP_NAME: usize = 48;
pub const EXEC: usize = 160;

/// Short text in a fixed buffer (ASCII and UTF-8 as given).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Text<const N: usize> {
    b: [u8; N],
    n: usize,
}

impl<const N: usize> core::fmt::Debug for Text<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

impl<const N: usize> Text<N> {
    pub const EMPTY: Text<N> = Text { b: [0; N], n: 0 };

    /// `s` (None when it does not fit).
    pub fn new(s: &[u8]) -> Option<Text<N>> {
        let mut t = Text { b: [0; N], n: s.len() };
        t.b.get_mut(..s.len())?.copy_from_slice(s);
        Some(t)
    }

    /// `s`, cut at a character boundary when it does not fit.
    pub fn cut(s: &str) -> Text<N> {
        let mut n = s.len().min(N);
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        let mut t = Text { b: [0; N], n };
        t.b[..n].copy_from_slice(&s.as_bytes()[..n]);
        t
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.b[..self.n]
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap_or("?")
    }
}

pub type Name = Text<NAME>;

/// An application of a distribution (a `.desktop` file's entry).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct App {
    pub name: Text<APP_NAME>,
    /// `Exec` with its field codes (`%f`, `%U`, ...) taken out.
    pub exec: Text<EXEC>,
}

/// A distribution the section shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Distro {
    pub name: Name,
    /// Registered (Lxss, State 1).
    pub installed: bool,
    /// Its suggestion (SUGGESTIONS index), if gala suggests it.
    pub suggestion: Option<usize>,
    /// Its applications (an installed distribution's), `apps` of them.
    pub apps: [Option<App>; MAX_APPS],
}

impl Distro {
    pub fn new(name: Name, installed: bool) -> Distro {
        let suggestion = SUGGESTIONS.iter().position(|s| s.name.as_bytes() == name.as_bytes());
        Distro { name, installed, suggestion, apps: [None; MAX_APPS] }
    }

    pub fn colors(&self) -> (u32, u32) {
        self.suggestion
            .and_then(|i| SUGGESTIONS.get(i))
            .map_or((0x00_6a_6a_6a, 0x00_3a_3a_3a), |s| s.colors)
    }

    pub fn apps(&self) -> impl Iterator<Item = &App> {
        self.apps.iter().flatten()
    }
}

/// The user's choices, kept in their hive (`HKCU\Software\gala\StartMenu`:
/// LinuxCollapsed and LinuxHidden as REG_DWORD, LinuxDismissed as
/// REG_MULTI_SZ of the dismissed suggestions' names).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Prefs {
    pub collapsed: bool,
    pub hidden: bool,
    /// Bit i: SUGGESTIONS[i] dismissed.
    pub dismissed: u32,
}

impl Prefs {
    pub fn is_dismissed(&self, suggestion: usize) -> bool {
        suggestion < 32 && self.dismissed & (1 << suggestion) != 0
    }

    /// LinuxDismissed's names from a REG_MULTI_SZ's UTF-16 bytes.
    pub fn read_dismissed(data: &[u8]) -> u32 {
        let units: Vec<u16> = {
            let mut v = Vec::new();
            if v.try_reserve_exact(data.len() / 2).is_err() {
                return 0;
            }
            v.extend(data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])));
            v
        };
        let mut bits = 0;
        for name in units.split(|&u| u == 0).filter(|n| !n.is_empty()) {
            for (i, s) in SUGGESTIONS.iter().enumerate() {
                let same = s.name.len() == name.len()
                    && s.name.bytes().zip(name).all(|(a, &b)| u16::from(a) == b);
                if same && i < 32 {
                    bits |= 1 << i;
                }
            }
        }
        bits
    }

    /// LinuxDismissed as a REG_MULTI_SZ (UTF-16 little-endian, each name
    /// NUL-terminated, then a NUL).
    pub fn write_dismissed(&self) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        for (i, s) in SUGGESTIONS.iter().enumerate() {
            if self.is_dismissed(i) {
                for u in s.name.encode_utf16().chain(Some(0)) {
                    out.try_extend_from_slice(&u.to_le_bytes()).ok()?;
                }
            }
        }
        out.try_extend_from_slice(&[0, 0]).ok()?;
        Some(out)
    }
}

/// What the section lists, read outside the desktop's lock (the
/// registered distributions and the choices).
#[derive(Debug, Default)]
pub struct Listing {
    /// Registered distributions (Lxss), in the registry's order.
    pub installed: Vec<Distro>,
    pub prefs: Prefs,
}

/// The tiles for `listing`: each suggestion in order (installed, or not
/// dismissed), then the other registered distributions; at most
/// MAX_ENTRIES.
pub fn entries(listing: &Listing) -> Vec<Distro> {
    let mut out = Vec::new();
    for (i, s) in SUGGESTIONS.iter().enumerate() {
        let installed = listing.installed.iter().find(|d| d.name.as_bytes() == s.name.as_bytes());
        let d = match installed {
            Some(d) => *d,
            None if listing.prefs.is_dismissed(i) => continue,
            None => Distro::new(Text::cut(s.name), false),
        };
        if out.len() < MAX_ENTRIES {
            let _ = out.try_push(d);
        }
    }
    for d in listing.installed.iter().filter(|d| d.suggestion.is_none()) {
        if out.len() < MAX_ENTRIES {
            let _ = out.try_push(*d);
        }
    }
    out
}

/// Where an install is (`gala linux-install` tells it through
/// GalaDesktopAction's LinuxProgress).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// The rootfs is being fetched: percent done (of Content-Length).
    Downloading(u8),
    /// `wsl --import` is making the tree.
    Importing,
    /// It failed (the tile shows a red bar until the menu lists again).
    Failed,
}

impl Progress {
    /// LinuxProgress's state and percent (0 downloading, 1 importing, 3
    /// failed; 2, done, is not a progress: None).
    pub fn from_state(state: i32, percent: i32) -> Option<Progress> {
        match state {
            0 => Some(Progress::Downloading(percent.clamp(0, 100) as u8)),
            1 => Some(Progress::Importing),
            3 => Some(Progress::Failed),
            _ => None,
        }
    }

    /// How much of the tile's bar is filled, in percent.
    pub fn percent(self) -> u8 {
        match self {
            // The download is most of the wait; the import the rest.
            Progress::Downloading(p) => (u16::from(p.min(100)) * 4 / 5) as u8,
            Progress::Importing => 90,
            Progress::Failed => 100,
        }
    }
}

/// What the start menu asks the system to do for a distribution
/// (`nt::action::start_linux`). An application's variant is the largest
/// (a few hundred bytes): one request at a time is made and handed on.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Install suggestion `n`'s distribution: `gala linux-install`.
    Install(Name, usize),
    /// Start its desktop (LD step 6's session).
    Desktop(Name),
    /// A terminal: `wsl -d <name>` in a console window.
    Terminal(Name),
    /// One of its applications.
    App(Name, App),
}

/// A context menu's item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Terminal,
    App(usize),
    /// Show the hidden section again (the menu's own context menu).
    ShowSection,
}

/// A context menu open in the start menu: for a distribution (its tile
/// index) or for the menu itself (None), at (x, y) in the menu, its
/// items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Context {
    pub distro: Option<usize>,
    pub x: i64,
    pub y: i64,
    pub items: [Option<Item>; MAX_APPS + 1],
}

/// A context menu's size: items this tall, this wide.
pub const ITEM_H: i64 = 32;
pub const CONTEXT_W: i64 = 220;

impl Context {
    pub fn count(&self) -> usize {
        self.items.iter().flatten().count()
    }

    pub fn height(&self) -> i64 {
        8 + self.count() as i64 * ITEM_H
    }
}

/// `line`'s value for `key` in a `.desktop` file (`Key=value`, spaces
/// around `=` allowed; a localized `Key[xx]=` is another key).
fn value<'a>(line: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let rest = line.strip_prefix(key)?;
    let rest = trim(rest);
    let rest = rest.strip_prefix(b"=")?;
    Some(trim(rest))
}

fn trim(mut s: &[u8]) -> &[u8] {
    while let [b' ' | b'\t', rest @ ..] = s {
        s = rest;
    }
    while let [rest @ .., b' ' | b'\t' | b'\r'] = s {
        s = rest;
    }
    s
}

/// The application a `.desktop` file describes (the Desktop Entry
/// Specification's `[Desktop Entry]` group): Type=Application with a Name
/// and an Exec, not NoDisplay, Hidden or Terminal (which would need a
/// terminal of its own). Exec's field codes are taken out (no files or
/// URLs are passed); `%%` is a `%`.
pub fn parse_desktop(text: &[u8]) -> Option<App> {
    let mut in_entry = false;
    let (mut name, mut exec, mut app) = (None, None, false);
    for line in text.split(|&b| b == b'\n') {
        let line = trim(line);
        if line.starts_with(b"[") {
            in_entry = line == b"[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with(b"#") {
            continue;
        }
        if let Some(v) = value(line, b"Name") {
            name = Some(v);
        } else if let Some(v) = value(line, b"Exec") {
            exec = Some(v);
        } else if let Some(v) = value(line, b"Type") {
            app = v == b"Application";
        } else if let Some(v) =
            value(line, b"NoDisplay").or(value(line, b"Hidden")).or(value(line, b"Terminal"))
            && v == b"true"
        {
            return None;
        }
    }
    let (name, exec) = (name?, exec?);
    if !app {
        return None;
    }
    let mut cleaned = [0u8; EXEC];
    let mut n = 0;
    let mut words = 0;
    for word in exec.split(|&b| b == b' ').filter(|w| !w.is_empty()) {
        // A field code alone (`%f`) goes; `%%` is a literal %.
        if word.len() == 2 && word[0] == b'%' && word[1] != b'%' {
            continue;
        }
        if words > 0 {
            *cleaned.get_mut(n)? = b' ';
            n += 1;
        }
        let mut i = 0;
        while i < word.len() {
            let b = word[i];
            if b == b'%' && word.get(i + 1) == Some(&b'%') {
                i += 1;
            }
            *cleaned.get_mut(n)? = b;
            n += 1;
            i += 1;
        }
        words += 1;
    }
    if n == 0 {
        return None;
    }
    let name = core::str::from_utf8(name).ok()?;
    Some(App { name: Text::cut(name), exec: Text::new(&cleaned[..n])? })
}

/// Exec's words (double quotes group, as the specification allows), for
/// wsl.exe's `--exec`.
pub fn exec_words(exec: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < exec.len() {
        while exec.get(i) == Some(&b' ') {
            i += 1;
        }
        if i >= exec.len() {
            break;
        }
        let (start, end) = if exec[i] == b'"' {
            let s = i + 1;
            let e = exec[s..].iter().position(|&b| b == b'"').map_or(exec.len(), |p| s + p);
            i = e + 1;
            (s, e)
        } else {
            let e = exec[i..].iter().position(|&b| b == b' ').map_or(exec.len(), |p| i + p);
            let s = i;
            i = e;
            (s, e)
        };
        if out.try_push(&exec[start..end]).is_err() {
            break;
        }
    }
    out
}

/// A distribution's name as WSL accepts it (letters, digits, `.`, `_`,
/// `-`): the only names put on a command line.
pub fn valid_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= NAME
        && name.iter().all(|&b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// The URL suggestion `s` is fetched from: its own, or `<mirror>/<its
/// file name>` with a mirror (`linux_mirror=`).
pub fn source_url<'b>(s: &Suggestion, mirror: Option<&str>, buf: &'b mut [u8]) -> Option<&'b str> {
    let mirror = match mirror {
        Some(m) if !m.is_empty() => m.trim_end_matches('/'),
        _ => return copy(buf, &[s.url.as_bytes()]),
    };
    let file = s.url.rsplit('/').next().unwrap_or(s.url);
    copy(buf, &[mirror.as_bytes(), b"/", file.as_bytes()])
}

fn copy<'b>(buf: &'b mut [u8], parts: &[&[u8]]) -> Option<&'b str> {
    let mut n = 0;
    for p in parts {
        buf.get_mut(n..n + p.len())?.copy_from_slice(p);
        n += p.len();
    }
    core::str::from_utf8(&buf[..n]).ok()
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn a_desktop_file_gives_its_name_and_command_without_field_codes() {
        let xterm = b"[Desktop Entry]\nName=XTerm\nGenericName=Terminal\n\
            Comment=standard terminal emulator for the X window system\n\
            Exec=xterm\nTerminal=false\nType=Application\n\
            Icon=mini.xterm\nCategories=System;TerminalEmulator;\n";
        let app = parse_desktop(xterm).unwrap();
        assert_eq!(app.name.as_str(), "XTerm");
        assert_eq!(app.exec.as_str(), "xterm");
        let gedit = b"# a comment\n[Desktop Entry]\nName = Text Editor\nName[de]=Texteditor\n\
            Exec=gedit --new-window %U\nType=Application\n[Desktop Action new]\nExec=other\n";
        let app = parse_desktop(gedit).unwrap();
        assert_eq!(app.name.as_str(), "Text Editor");
        assert_eq!(app.exec.as_str(), "gedit --new-window");
        assert_eq!(
            parse_desktop(b"[Desktop Entry]\nName=x\nExec=50%% %f\nType=Application\n")
                .unwrap()
                .exec
                .as_str(),
            "50%"
        );
        // Not listed: hidden, terminal programs, links, incomplete.
        for text in [
            &b"[Desktop Entry]\nName=a\nExec=a\nType=Application\nNoDisplay=true\n"[..],
            b"[Desktop Entry]\nName=a\nExec=a\nType=Application\nTerminal=true\n",
            b"[Desktop Entry]\nName=a\nURL=x\nType=Link\n",
            b"[Desktop Entry]\nName=a\nType=Application\n",
            b"[Other]\nName=a\nExec=a\nType=Application\n",
        ] {
            assert_eq!(parse_desktop(text), None);
        }
        assert_eq!(exec_words(b"a \"b c\"  d"), [&b"a"[..], b"b c", b"d"]);
    }

    #[test]
    fn the_section_lists_suggestions_then_other_distributions() {
        let mut listing = Listing::default();
        let names = |l: &Listing| -> Vec<(&'static str, bool)> {
            entries(l)
                .iter()
                .map(|d| {
                    let n = match d.name.as_str() {
                        "Ubuntu" => "Ubuntu",
                        "Alpine" => "Alpine",
                        "bb" => "bb",
                        _ => "?",
                    };
                    (n, d.installed)
                })
                .collect()
        };
        assert_eq!(names(&listing), [("Ubuntu", false), ("Alpine", false)]);
        listing.prefs.dismissed = 1 << 1;
        assert_eq!(names(&listing), [("Ubuntu", false)]);
        listing.installed.push(Distro::new(Name::new(b"bb").unwrap(), true));
        listing.installed.push(Distro::new(Name::new(b"Ubuntu").unwrap(), true));
        assert_eq!(names(&listing), [("Ubuntu", true), ("bb", true)]);
        // A dismissed suggestion that is installed is listed.
        listing.prefs.dismissed = 1;
        assert_eq!(names(&listing), [("Ubuntu", true), ("Alpine", false), ("bb", true)]);
    }

    #[test]
    fn dismissed_names_go_to_the_registry_and_back() {
        let p = Prefs { collapsed: false, hidden: false, dismissed: 0b10 };
        let data = p.write_dismissed().unwrap();
        let text: Vec<u16> =
            data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(String::from_utf16(&text).unwrap(), "Alpine\0\0");
        assert_eq!(Prefs::read_dismissed(&data), 0b10);
        assert_eq!(Prefs::read_dismissed(&[]), 0);
        let empty = Prefs::default().write_dismissed().unwrap();
        assert_eq!(empty, [0, 0]);
    }

    #[test]
    fn a_mirror_serves_the_suggestions_by_file_name() {
        let mut buf = [0u8; 256];
        let u = source_url(&SUGGESTIONS[0], Some("http://10.0.2.100/"), &mut buf).unwrap();
        assert_eq!(u, "http://10.0.2.100/ubuntu-noble-wsl-amd64-wsl.rootfs.tar.gz");
        let u = source_url(&SUGGESTIONS[1], None, &mut buf).unwrap();
        assert_eq!(u, SUGGESTIONS[1].url);
        assert!(valid_name(b"Ubuntu-24.04"));
        assert_eq!(Progress::Downloading(100).percent(), 80);
        assert_eq!(Progress::from_state(0, 250), Some(Progress::Downloading(100)));
        assert!(!valid_name(b"a b") && !valid_name(b"a\"") && !valid_name(b""));
    }
}
