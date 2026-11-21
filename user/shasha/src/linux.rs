//! The start menu's Linux section (ROADMAP SH step 1, part 11;
//! ARCHITECTURE "The user-mode shell"): the desktop's list of it
//! (GalaDesktopAction ACTION_LINUX_LIST, [`parse_linux`]), what its
//! tiles say and what a click on one asks for, without system calls
//! (host tests below; the menu's view places the tiles).
//!
//! Each distribution the section lists is a tile: its button
//! (`Linux<name>`, named with the distribution's name), a Text
//! `Linux<name>State` saying where it is ([`linux_state_text`]: "Not
//! installed" beside an accent badge, "Downloading N%", "Importing",
//! "The install failed.", "The install stopped without a word." or
//! "Installed") and, while an install has been asked for, its bar
//! (`Linux<name>Bar`). An installed distribution's tile is followed by its
//! terminal's (`LinuxTerminal<name>`, "Terminal (<name>)") and its
//! applications' (`LinuxApp<name>_<index>`, "<application> (<name>)", as
//! WSL's shortcuts are named).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// What a Linux section entry is (ACTION_LINUX_LIST's kinds,
/// kernel/src/nt/action.rs `LINUX_KIND_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinuxKind {
    Distribution,
    Terminal,
    App,
}

/// Where a distribution is (`LINUX_STATE_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinuxState {
    NotInstalled,
    Installed,
    Downloading,
    Importing,
    Failed,
    /// Its `gala linux-install` ended without a word.
    Lost,
}

impl LinuxState {
    fn from_u32(v: u32) -> Option<LinuxState> {
        Some(match v {
            0 => LinuxState::NotInstalled,
            1 => LinuxState::Installed,
            2 => LinuxState::Downloading,
            3 => LinuxState::Importing,
            4 => LinuxState::Failed,
            5 => LinuxState::Lost,
            _ => return None,
        })
    }

    /// The word the log says it with.
    pub fn word(self) -> &'static str {
        match self {
            LinuxState::NotInstalled => "not installed",
            LinuxState::Installed => "installed",
            LinuxState::Downloading => "downloading",
            LinuxState::Importing => "importing",
            LinuxState::Failed => "failed",
            LinuxState::Lost => "gone quiet",
        }
    }

    /// An install is under way: a click does nothing.
    pub fn running(self) -> bool {
        matches!(self, LinuxState::Downloading | LinuxState::Importing)
    }

    /// The tile has a bar (an install was asked for).
    pub fn has_bar(self) -> bool {
        !matches!(self, LinuxState::NotInstalled | LinuxState::Installed)
    }
}

/// An entry of the Linux section: its kind, its distribution's state
/// and bar (percent), the index the Linux action takes with it (a
/// distribution's suggestion, an application's index), the
/// distribution's name and the entry's label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinuxEntry {
    pub kind: LinuxKind,
    pub state: LinuxState,
    pub percent: u8,
    pub index: u32,
    pub name: String,
    pub label: String,
}

/// ACTION_LINUX_LIST's entry: kind, state, percent, index, the name's
/// and the label's lengths (6 x 4 bytes), the name (32 UTF-16 units),
/// the label (48).
pub const LINUX_ENTRY: usize = 24 + 2 * NAME + 2 * LABEL;
const NAME: usize = 32;
const LABEL: usize = 48;
/// A distribution entry's index when gala does not suggest it (no
/// install to ask for).
pub const NOT_SUGGESTED: u32 = u32::MAX;

/// The Linux action's operations (kernel/src/nt/action.rs `LINUX_*`).
pub const LINUX_INSTALL: i32 = 0;
pub const LINUX_DESKTOP: i32 = 1;
pub const LINUX_TERMINAL: i32 = 2;
pub const LINUX_APP: i32 = 3;

/// The entries in ACTION_LINUX_LIST's output `b` (an 8-byte header, then
/// LINUX_ENTRY-byte entries); an entry of a kind or state this shell does
/// not know is left out.
pub fn parse_linux(b: &[u8]) -> Vec<LinuxEntry> {
    const HEADER: usize = 8;
    let u32_at = |e: &[u8], at: usize| u32::from_le_bytes([e[at], e[at + 1], e[at + 2], e[at + 3]]);
    let units = |e: &[u8], at: usize, n: usize| -> String {
        let u: Vec<u16> =
            (0..n).map(|k| u16::from_le_bytes([e[at + 2 * k], e[at + 2 * k + 1]])).collect();
        String::from_utf16_lossy(&u)
    };
    b.get(HEADER..)
        .unwrap_or(&[])
        .chunks_exact(LINUX_ENTRY)
        .filter_map(|e| {
            let kind = match u32_at(e, 0) {
                0 => LinuxKind::Distribution,
                1 => LinuxKind::Terminal,
                2 => LinuxKind::App,
                _ => return None,
            };
            let name_len = (u32_at(e, 16) as usize).min(NAME);
            let label_len = (u32_at(e, 20) as usize).min(LABEL);
            Some(LinuxEntry {
                kind,
                state: LinuxState::from_u32(u32_at(e, 4))?,
                percent: u32_at(e, 8).min(100) as u8,
                index: u32_at(e, 12),
                name: units(e, 24, name_len),
                label: units(e, 24 + 2 * NAME, label_len),
            })
        })
        .collect()
}

/// The words under a distribution's name (its bar's percent, as the
/// catalog's tiles say theirs).
pub fn linux_state_text(e: &LinuxEntry) -> String {
    match e.state {
        LinuxState::NotInstalled => "Not installed".into(),
        LinuxState::Installed => "Installed".into(),
        LinuxState::Downloading => format!("Downloading {}%", e.percent),
        LinuxState::Importing => "Importing".into(),
        LinuxState::Failed => "The install failed.".into(),
        LinuxState::Lost => "The install stopped without a word.".into(),
    }
}

/// The letters and digits of `s` (an AutomationId's part).
fn id_part(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// An entry's AutomationId: `Linux<name>`, `LinuxTerminal<name>`,
/// `LinuxApp<name>_<index>`.
pub fn linux_id(e: &LinuxEntry) -> String {
    let name = id_part(&e.name);
    match e.kind {
        LinuxKind::Distribution => format!("Linux{name}"),
        LinuxKind::Terminal => format!("LinuxTerminal{name}"),
        LinuxKind::App => format!("LinuxApp{name}_{}", e.index),
    }
}

/// An entry's button's name: the distribution's own, or the terminal's
/// or application's label and its distribution's.
pub fn linux_label(e: &LinuxEntry) -> String {
    match e.kind {
        LinuxKind::Distribution => e.name.clone(),
        LinuxKind::Terminal | LinuxKind::App => format!("{} ({})", e.label, e.name),
    }
}

/// What a click on an entry asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinuxClick {
    /// Its install (LINUX_INSTALL with the suggestion): one not
    /// installed, failed or gone quiet. The menu stays.
    Install(u32),
    /// An install is under way: nothing.
    Busy,
    /// Not installed, and not one gala suggests: nothing to install it
    /// from.
    NotSuggested,
    /// An installed distribution's desktop (LINUX_DESKTOP), its terminal
    /// (LINUX_TERMINAL), one of its applications (LINUX_APP with its
    /// index). The menu closes.
    Desktop,
    Terminal,
    App(u32),
}

impl LinuxClick {
    /// The Linux action's operation and index for it (None: nothing to
    /// ask for), and whether the menu closes.
    pub fn request(self) -> Option<(i32, u32, bool)> {
        match self {
            LinuxClick::Install(s) => Some((LINUX_INSTALL, s, false)),
            LinuxClick::Desktop => Some((LINUX_DESKTOP, 0, true)),
            LinuxClick::Terminal => Some((LINUX_TERMINAL, 0, true)),
            LinuxClick::App(i) => Some((LINUX_APP, i, true)),
            LinuxClick::Busy | LinuxClick::NotSuggested => None,
        }
    }

    /// The word the log says it with.
    pub fn word(self) -> &'static str {
        match self {
            LinuxClick::Install(_) => "install",
            LinuxClick::Desktop => "desktop",
            LinuxClick::Terminal => "terminal",
            LinuxClick::App(_) => "application",
            LinuxClick::Busy => "busy",
            LinuxClick::NotSuggested => "not suggested",
        }
    }
}

impl LinuxEntry {
    /// What a click on it asks for.
    pub fn click(&self) -> LinuxClick {
        match self.kind {
            LinuxKind::Terminal => LinuxClick::Terminal,
            LinuxKind::App => LinuxClick::App(self.index),
            LinuxKind::Distribution if self.state == LinuxState::Installed => LinuxClick::Desktop,
            LinuxKind::Distribution if self.state.running() => LinuxClick::Busy,
            LinuxKind::Distribution if self.index == NOT_SUGGESTED => LinuxClick::NotSuggested,
            LinuxKind::Distribution => LinuxClick::Install(self.index),
        }
    }
}

/// A line for the log for each distribution whose state changed from
/// `before` to `now` (`linux: Ubuntu downloading`), every one the first
/// time, and for each terminal or application that came
/// (`linux: Ubuntu lists XTerm`). A bar that moves is no line.
pub fn linux_changes(before: &[LinuxEntry], now: &[LinuxEntry]) -> Vec<String> {
    let mut lines = Vec::new();
    for e in now {
        let same = |o: &&LinuxEntry| o.kind == e.kind && o.name == e.name && o.index == e.index;
        let old = before.iter().find(same);
        match e.kind {
            LinuxKind::Distribution if old.is_none_or(|o| o.state != e.state) => {
                lines.push(format!("linux: {} {}", e.name, e.state.word()));
            }
            LinuxKind::Terminal | LinuxKind::App if old.is_none_or(|o| o.label != e.label) => {
                lines.push(format!("linux: {} lists {}", e.name, e.label));
            }
            _ => {}
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn raw(kind: u32, state: u32, percent: u32, index: u32, name: &str, label: &str) -> Vec<u8> {
        let mut e = vec![0u8; LINUX_ENTRY];
        let (n, l): (Vec<u16>, Vec<u16>) =
            (name.encode_utf16().collect(), label.encode_utf16().collect());
        for (k, v) in
            [kind, state, percent, index, n.len() as u32, l.len() as u32].iter().enumerate()
        {
            e[4 * k..4 * k + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (k, u) in n.iter().enumerate() {
            e[24 + 2 * k..26 + 2 * k].copy_from_slice(&u.to_le_bytes());
        }
        for (k, u) in l.iter().enumerate() {
            e[88 + 2 * k..90 + 2 * k].copy_from_slice(&u.to_le_bytes());
        }
        e
    }

    pub(crate) fn entry(
        kind: LinuxKind,
        state: LinuxState,
        percent: u8,
        index: u32,
        name: &str,
        label: &str,
    ) -> LinuxEntry {
        LinuxEntry { kind, state, percent, index, name: name.to_string(), label: label.to_string() }
    }

    #[test]
    fn parses_the_linux_section() {
        let mut b = vec![1, 0, 0, 0, 4, 0, 0, 0];
        b.extend(raw(0, 1, 0, 0, "Ubuntu", "Ubuntu"));
        b.extend(raw(2, 1, 0, 3, "Ubuntu", "XTerm"));
        b.extend(raw(0, 2, 48, 1, "Alpine", "Alpine"));
        // A kind this shell does not know is left out, as is a state.
        b.extend(raw(9, 1, 0, 0, "Ubuntu", "?"));
        b.extend(raw(0, 99, 0, 0, "Debian", "Debian"));
        use LinuxKind::*;
        assert_eq!(
            parse_linux(&b),
            [
                entry(Distribution, LinuxState::Installed, 0, 0, "Ubuntu", "Ubuntu"),
                entry(App, LinuxState::Installed, 0, 3, "Ubuntu", "XTerm"),
                entry(Distribution, LinuxState::Downloading, 48, 1, "Alpine", "Alpine"),
            ]
        );
        assert!(parse_linux(&[1, 0]).is_empty());
    }

    #[test]
    fn ids_labels_and_words() {
        use LinuxKind::*;
        let d = entry(Distribution, LinuxState::Downloading, 48, 0, "Ubuntu-24.04", "Ubuntu-24.04");
        assert_eq!(
            (linux_id(&d), linux_label(&d)),
            ("LinuxUbuntu2404".into(), "Ubuntu-24.04".into())
        );
        assert_eq!(linux_state_text(&d), "Downloading 48%");
        let t = entry(Terminal, LinuxState::Installed, 0, 0, "Ubuntu", "Terminal");
        assert_eq!(
            (linux_id(&t), linux_label(&t)),
            ("LinuxTerminalUbuntu".into(), "Terminal (Ubuntu)".into())
        );
        let a = entry(App, LinuxState::Installed, 0, 2, "Ubuntu", "XTerm");
        assert_eq!(
            (linux_id(&a), linux_label(&a)),
            ("LinuxAppUbuntu_2".into(), "XTerm (Ubuntu)".into())
        );
        let say = |state| linux_state_text(&entry(Distribution, state, 0, 0, "U", "U"));
        assert_eq!(say(LinuxState::NotInstalled), "Not installed");
        assert_eq!(say(LinuxState::Installed), "Installed");
        assert_eq!(say(LinuxState::Importing), "Importing");
        assert_eq!(say(LinuxState::Failed), "The install failed.");
        assert_eq!(say(LinuxState::Lost), "The install stopped without a word.");
    }

    /// A click: an install for a suggestion not installed, failed or gone
    /// quiet (the menu stays), nothing while one runs or for one gala
    /// does not suggest, and the desktop, terminal or application of an
    /// installed one (the menu closes).
    #[test]
    fn a_click_asks_for_what_the_entry_is() {
        use LinuxKind::*;
        let d = |state, index| entry(Distribution, state, 0, index, "Ubuntu", "Ubuntu").click();
        assert_eq!(d(LinuxState::NotInstalled, 0), LinuxClick::Install(0));
        assert_eq!(d(LinuxState::Failed, 1), LinuxClick::Install(1));
        assert_eq!(d(LinuxState::Lost, 1), LinuxClick::Install(1));
        assert_eq!(d(LinuxState::Downloading, 0), LinuxClick::Busy);
        assert_eq!(d(LinuxState::Importing, 0), LinuxClick::Busy);
        assert_eq!(d(LinuxState::NotInstalled, NOT_SUGGESTED), LinuxClick::NotSuggested);
        assert_eq!(d(LinuxState::Installed, NOT_SUGGESTED), LinuxClick::Desktop);
        let t = entry(Terminal, LinuxState::Installed, 0, 0, "Ubuntu", "Terminal");
        assert_eq!(t.click(), LinuxClick::Terminal);
        let a = entry(App, LinuxState::Installed, 0, 2, "Ubuntu", "XTerm");
        assert_eq!(a.click(), LinuxClick::App(2));
        assert_eq!(LinuxClick::Install(1).request(), Some((LINUX_INSTALL, 1, false)));
        assert_eq!(LinuxClick::Desktop.request(), Some((LINUX_DESKTOP, 0, true)));
        assert_eq!(LinuxClick::Terminal.request(), Some((LINUX_TERMINAL, 0, true)));
        assert_eq!(LinuxClick::App(2).request(), Some((LINUX_APP, 2, true)));
        assert_eq!(LinuxClick::Busy.request(), None);
    }

    /// Each distribution's state change is one line, every one the first
    /// time; a bar that moves is none; a terminal or application is told
    /// once, when it comes.
    #[test]
    fn changes_are_told_once() {
        use LinuxKind::*;
        let ubuntu = |state, percent| entry(Distribution, state, percent, 0, "Ubuntu", "Ubuntu");
        let alpine = entry(Distribution, LinuxState::NotInstalled, 0, 1, "Alpine", "Alpine");
        let first = vec![ubuntu(LinuxState::NotInstalled, 0), alpine.clone()];
        assert_eq!(
            linux_changes(&[], &first),
            ["linux: Ubuntu not installed", "linux: Alpine not installed"]
        );
        let down = vec![ubuntu(LinuxState::Downloading, 0), alpine.clone()];
        assert_eq!(linux_changes(&first, &down), ["linux: Ubuntu downloading"]);
        let moved = vec![ubuntu(LinuxState::Downloading, 48), alpine.clone()];
        assert!(linux_changes(&down, &moved).is_empty());
        let done = vec![
            ubuntu(LinuxState::Installed, 0),
            entry(Terminal, LinuxState::Installed, 0, 0, "Ubuntu", "Terminal"),
            entry(App, LinuxState::Installed, 0, 0, "Ubuntu", "XTerm"),
            alpine,
        ];
        assert_eq!(
            linux_changes(&moved, &done),
            [
                "linux: Ubuntu installed",
                "linux: Ubuntu lists Terminal",
                "linux: Ubuntu lists XTerm"
            ]
        );
        assert!(linux_changes(&done, &done).is_empty());
    }
}
