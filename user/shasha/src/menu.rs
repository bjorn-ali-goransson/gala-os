//! shasha's start menu: its state, view and update (ROADMAP SH step 1,
//! part 7; ARCHITECTURE "The user-mode shell"), as a gala_ui component,
//! without system calls (host tests below).
//!
//! A panel `MENU_WIDTH` x `MENU_HEIGHT`, centred above the taskbar (as
//! Windows 11's): the "Search" input, then a tile per installed program
//! (the desktop's list, GalaDesktopAction ACTION_PROGRAMS), four to a
//! row. Typing filters the tiles: a program is shown when its name holds
//! the search, ignoring case. A tile's AutomationId is `Program` and the
//! name's letters and digits ([`program_id`]), its name the program's.
//!
//! After the installed programs come the catalog's tiles whose program
//! is not on this machine (GalaDesktopAction ACTION_CATALOG,
//! [`parse_catalog`]), filtered by the search too: a column with the
//! entry's button (the same `Program...` id), a Text `<id>State` saying
//! where it is ([`state_text`]: "Not installed" beside an accent badge,
//! "Resolving", "Downloading N%", "Installing", "The install failed..."
//! or "The install stopped without a word.") and, while an install has
//! been asked for, its bar. A click on one not installed, failed or gone
//! quiet asks for its install ([`MenuMsg::Install`]); on one under way it
//! does nothing.
//!
//! After them comes the Linux section (GalaDesktopAction
//! ACTION_LINUX_LIST, [`crate::linux`]): a "Linux" heading and a tile
//! per entry the search shows, a distribution's with its state's Text and
//! bar as a catalog tile's, a terminal's or an application's a button. A
//! click asks for what the entry is ([`MenuMsg::Linux`],
//! [`crate::linux::LinuxEntry::click`]).
//!
//! The program shows and hides the window and reads the lists; what this
//! keeps is the lists and the search, which [`StartMenu::open`] clears.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use gala_ui::{Node, button, column, fr, grid, input, row, text};

use crate::linux::{LinuxEntry, LinuxKind, LinuxState, linux_changes, linux_id, linux_label};

/// The menu's size, and its gap above the taskbar.
pub const MENU_WIDTH: i32 = 520;
pub const MENU_HEIGHT: i32 = 560;
pub const MENU_GAP: i32 = 12;
/// Tiles in a row.
pub const TILES_IN_ROW: usize = 4;

const SURFACE: u32 = 0xeef1f6;
const LINE: u32 = 0xd7dce3;
const TILE: u32 = 0xffffff;
const INK: u32 = 0x1b1b1b;
const MUTED: u32 = 0x616161;
/// The badge and bar of an install that runs, one that failed, one gone
/// quiet.
const ACCENT: u32 = 0x005fb8;
const FAILED: u32 = 0xc42b1c;
const QUIET: u32 = 0x8a8a8a;

/// A program the menu starts: its name and the path the desktop gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub name: String,
    pub path: String,
}

#[derive(Clone, Debug)]
pub enum MenuMsg {
    /// The Start button or the Windows key: shown if hidden, else hidden.
    Toggle,
    /// Another window became active: hidden.
    Deactivated,
    /// Esc pressed in the menu: hidden (as Windows' Start).
    Escape,
    /// The search's text.
    Query(String),
    /// A tile (the program's index in the list).
    Launch(usize),
    /// A catalog tile (its index in the catalog): install it.
    Install(usize),
    /// The desktop says a catalog tile changed (WM_GALA_CATALOG, wparam
    /// 0): read the catalog again.
    CatalogChanged,
    /// A Linux section tile (its index in the section): ask for what it
    /// is.
    Linux(usize),
    /// The desktop says the Linux section changed (WM_GALA_CATALOG,
    /// wparam 1): read it again.
    LinuxChanged,
}

#[derive(Default)]
pub struct StartMenu {
    pub programs: Vec<Program>,
    pub catalog: Vec<CatalogEntry>,
    /// The Linux section, in the desktop's order.
    pub linux: Vec<LinuxEntry>,
    pub query: String,
}

/// Where a catalog tile is (ACTION_CATALOG's states,
/// kernel/src/nt/action.rs `CATALOG_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogState {
    NotInstalled,
    Installed,
    Resolving,
    Downloading,
    Installing,
    /// With gala's exit code (`CatalogEntry::exit`).
    Failed,
    /// Its `gala install` ended without a word.
    Lost,
}

impl CatalogState {
    fn from_u32(v: u32) -> Option<CatalogState> {
        Some(match v {
            0 => CatalogState::NotInstalled,
            1 => CatalogState::Installed,
            2 => CatalogState::Resolving,
            3 => CatalogState::Downloading,
            4 => CatalogState::Installing,
            5 => CatalogState::Failed,
            6 => CatalogState::Lost,
            _ => return None,
        })
    }

    /// The word the log says it with.
    pub fn word(self) -> &'static str {
        match self {
            CatalogState::NotInstalled => "not installed",
            CatalogState::Installed => "installed",
            CatalogState::Resolving => "resolving",
            CatalogState::Downloading => "downloading",
            CatalogState::Installing => "installing",
            CatalogState::Failed => "failed",
            CatalogState::Lost => "gone quiet",
        }
    }

    /// An install is under way: a click does nothing.
    pub fn running(self) -> bool {
        matches!(
            self,
            CatalogState::Resolving | CatalogState::Downloading | CatalogState::Installing
        )
    }

    /// The tile has a bar (an install was asked for).
    fn has_bar(self) -> bool {
        !matches!(self, CatalogState::NotInstalled | CatalogState::Installed)
    }
}

/// A catalog tile: the entry's name and winget id, where it is, how much
/// of its bar is filled (percent) and gala's exit code when it failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogEntry {
    pub name: String,
    pub id: String,
    pub state: CatalogState,
    pub percent: u8,
    pub exit: i32,
}

/// ACTION_CATALOG's entry: the name's and the id's lengths, the state,
/// the percent, the exit code (5 x 4 bytes), the name (32 UTF-16 units),
/// the id (64).
pub const CATALOG_ENTRY: usize = 20 + 2 * 32 + 2 * 64;

/// The tiles in ACTION_CATALOG's output `b` (an 8-byte header, then
/// CATALOG_ENTRY-byte entries); an entry in a state this shell does not
/// know is left out.
pub fn parse_catalog(b: &[u8]) -> Vec<CatalogEntry> {
    const HEADER: usize = 8;
    const NAME: usize = 32;
    const ID: usize = 64;
    let u32_at = |e: &[u8], at: usize| u32::from_le_bytes([e[at], e[at + 1], e[at + 2], e[at + 3]]);
    let units = |e: &[u8], at: usize, n: usize| -> String {
        let u: Vec<u16> =
            (0..n).map(|k| u16::from_le_bytes([e[at + 2 * k], e[at + 2 * k + 1]])).collect();
        String::from_utf16_lossy(&u)
    };
    b.get(HEADER..)
        .unwrap_or(&[])
        .chunks_exact(CATALOG_ENTRY)
        .filter_map(|e| {
            let name_len = (u32_at(e, 0) as usize).min(NAME);
            let id_len = (u32_at(e, 4) as usize).min(ID);
            Some(CatalogEntry {
                state: CatalogState::from_u32(u32_at(e, 8))?,
                percent: u32_at(e, 12).min(100) as u8,
                exit: u32_at(e, 16) as i32,
                name: units(e, 20, name_len),
                id: units(e, 20 + 2 * NAME, id_len),
            })
        })
        .collect()
}

/// What gala's exit code says beyond the install having failed, as the
/// kernel's tile tells it (kernel/src/nt/desktop/pkg.rs
/// `install_failure`, galapkg's `Kind`; keep the two alike). 4 says no
/// more than that: which step failed is in gala's own line in the log.
fn install_failure(exit: i32) -> Option<&'static str> {
    match exit {
        1 => Some("gala was asked for it wrongly"),
        2 => Some("the source has no such package or version"),
        3 => Some("gala cannot handle that installer or source yet"),
        5 => Some("the download is not what the manifest says"),
        6 => Some("it needs administrator rights"),
        _ => None,
    }
}

/// The words under a catalog tile's name. A download's percent is the
/// bar's (the kernel's `Progress::percent`: the download is the bar's
/// middle three fifths), so that the words and the bar agree.
pub fn state_text(e: &CatalogEntry) -> String {
    match e.state {
        CatalogState::NotInstalled => "Not installed".into(),
        CatalogState::Installed => "Installed".into(),
        CatalogState::Resolving => "Resolving".into(),
        CatalogState::Downloading => format!("Downloading {}%", e.percent),
        CatalogState::Installing => "Installing".into(),
        CatalogState::Failed => match install_failure(e.exit) {
            Some(why) => format!("The install failed: {why}."),
            None => "The install failed.".into(),
        },
        CatalogState::Lost => "The install stopped without a word.".into(),
    }
}

/// The AutomationId of program `name`'s tile.
pub fn program_id(name: &str) -> String {
    let mut id = String::from("Program");
    id.extend(name.chars().filter(|c| c.is_ascii_alphanumeric()));
    id
}

/// The programs in ACTION_PROGRAMS's output `b` (an 8-byte header, then
/// 8 + 64 + 256-byte records: the name's and the path's lengths in
/// UTF-16 units, the name (32 units), the path (128 units)).
pub fn parse_programs(b: &[u8]) -> Vec<Program> {
    const HEADER: usize = 8;
    const NAME: usize = 32;
    const PATH: usize = 128;
    const ENTRY: usize = 8 + 2 * NAME + 2 * PATH;
    let u32_at = |e: &[u8], at: usize| u32::from_le_bytes([e[at], e[at + 1], e[at + 2], e[at + 3]]);
    let units = |e: &[u8], at: usize, n: usize| -> String {
        let u: Vec<u16> =
            (0..n).map(|k| u16::from_le_bytes([e[at + 2 * k], e[at + 2 * k + 1]])).collect();
        String::from_utf16_lossy(&u)
    };
    b.get(HEADER..)
        .unwrap_or(&[])
        .chunks_exact(ENTRY)
        .map(|e| {
            let name_len = (u32_at(e, 0) as usize).min(NAME);
            let path_len = (u32_at(e, 4) as usize).min(PATH);
            Program { name: units(e, 8, name_len), path: units(e, 8 + 2 * NAME, path_len) }
        })
        .collect()
}

impl StartMenu {
    /// Opened afresh with the desktop's `programs`: the search cleared.
    pub fn open(&mut self, programs: Vec<Program>) {
        self.programs = programs;
        self.query.clear();
    }

    /// The desktop's catalog, read again; a line for the log for each
    /// tile whose state changed (`catalog: 7-Zip resolving`, `catalog:
    /// 7-Zip failed (gala exit 4)`), every tile the first time. A bar
    /// that moves is no line.
    pub fn set_catalog(&mut self, catalog: Vec<CatalogEntry>) -> Vec<String> {
        let mut lines = Vec::new();
        for e in &catalog {
            let before = self.catalog.iter().find(|o| o.id == e.id);
            if before.is_some_and(|o| o.state == e.state && o.exit == e.exit) {
                continue;
            }
            let mut line = format!("catalog: {} {}", e.name, e.state.word());
            if e.state == CatalogState::Failed {
                line.push_str(&format!(" (gala exit {})", e.exit));
            }
            lines.push(line);
        }
        self.catalog = catalog;
        lines
    }

    /// The desktop's Linux section, read again; a line for the log for
    /// each distribution whose state changed and each terminal or
    /// application that came ([`linux_changes`]).
    pub fn set_linux(&mut self, linux: Vec<LinuxEntry>) -> Vec<String> {
        let lines = linux_changes(&self.linux, &linux);
        self.linux = linux;
        lines
    }

    /// The Linux tiles the search shows (their button's name holds it,
    /// ignoring case), as indices into the section.
    pub fn shown_linux(&self) -> Vec<usize> {
        let q = self.query.to_lowercase();
        (0..self.linux.len())
            .filter(|&i| linux_label(&self.linux[i]).to_lowercase().contains(q.as_str()))
            .collect()
    }

    /// The catalog tiles the menu shows, as indices into the catalog:
    /// those whose program is not on this machine (an installed one is
    /// among the programs), which the search shows.
    pub fn shown_catalog(&self) -> Vec<usize> {
        let q = self.query.to_lowercase();
        (0..self.catalog.len())
            .filter(|&i| {
                let e = &self.catalog[i];
                e.state != CatalogState::Installed
                    && !self.programs.iter().any(|p| p.name == e.name)
                    && e.name.to_lowercase().contains(q.as_str())
            })
            .collect()
    }

    /// The programs the search shows, as indices into the list.
    pub fn shown(&self) -> Vec<usize> {
        let q = self.query.to_lowercase();
        (0..self.programs.len())
            .filter(|&i| self.programs[i].name.to_lowercase().contains(q.as_str()))
            .collect()
    }

    pub fn update(&mut self, msg: MenuMsg) {
        if let MenuMsg::Query(q) = msg {
            self.query = q;
        }
        // Toggle, Deactivated, Launch, Install, CatalogChanged, Linux and
        // LinuxChanged are the program's to act on.
    }

    pub fn view(&self) -> Node<MenuMsg> {
        let shown = self.shown();
        let catalog = self.shown_catalog();
        let tiles = shown.iter().map(|&i| {
            let p = &self.programs[i];
            button(program_id(&p.name), p.name.clone())
                .height(72)
                .size(13)
                .bg(TILE)
                .border(LINE)
                .radius(6)
                .on_click(MenuMsg::Launch(i))
        });
        let tiles = tiles.chain(catalog.iter().map(|&i| catalog_tile(&self.catalog[i], i)));
        let linux = self.shown_linux();
        let list = if shown.is_empty() && catalog.is_empty() && linux.is_empty() {
            column().child(
                text(format!("No program matches \"{}\"", self.query)).id("NoMatch").color(MUTED),
            )
        } else {
            grid((0..TILES_IN_ROW).map(|_| fr(1))).id("Programs").gap(8).children(tiles)
        };
        column()
            .id("StartMenu")
            .bg(SURFACE)
            .border(LINE)
            .radius(8)
            .color(INK)
            .padding(24)
            .gap(16)
            .child(input("Search", &self.query).placeholder("Search").on_input(MenuMsg::Query))
            .child(text("Programs").id("ProgramsTitle").bold())
            .child(list)
            .child_if(!linux.is_empty(), text("Linux").id("LinuxTitle").bold())
            .child_if(
                !linux.is_empty(),
                grid((0..TILES_IN_ROW).map(|_| fr(1)))
                    .id("Linux")
                    .gap(8)
                    .children(linux.iter().map(|&i| linux_tile(&self.linux[i], i))),
            )
    }
}

/// A Linux section tile: a distribution's is its button, a badge while
/// it is not installed, its state's words and, once an install was asked
/// for, its bar; a terminal's or an application's is a button.
fn linux_tile(e: &LinuxEntry, i: usize) -> Node<MenuMsg> {
    let id = linux_id(e);
    let b = button(id.clone(), linux_label(e)).size(13).bg(TILE).on_click(MenuMsg::Linux(i));
    if e.kind != LinuxKind::Distribution {
        return b.height(72).border(LINE).radius(6);
    }
    let words = text(crate::linux::linux_state_text(e))
        .id(format!("{id}State"))
        .size(11)
        .color(MUTED)
        .ellipsis();
    let state = row().gap(4).align(gala_ui::Align::Center).child_if(
        e.state == LinuxState::NotInstalled,
        column().id(format!("{id}Badge")).width(10).height(10).radius(5).bg(ACCENT),
    );
    let colour = match e.state {
        LinuxState::Failed => FAILED,
        LinuxState::Lost => QUIET,
        _ => ACCENT,
    };
    column()
        .bg(TILE)
        .border(LINE)
        .radius(6)
        .padding(4)
        .gap(2)
        .height(72)
        .child(b)
        .child(state.child(words))
        .child_if(e.state.has_bar(), bar(&id, e.percent, colour))
}

/// A catalog tile: its button, a badge while it is not installed, the
/// words of its state and, once an install was asked for, its bar.
fn catalog_tile(e: &CatalogEntry, i: usize) -> Node<MenuMsg> {
    let id = program_id(&e.name);
    let words = text(state_text(e)).id(format!("{id}State")).size(11).color(MUTED).ellipsis();
    let state = row().gap(4).align(gala_ui::Align::Center).child_if(
        e.state == CatalogState::NotInstalled,
        column().id(format!("{id}Badge")).width(10).height(10).radius(5).bg(ACCENT),
    );
    let colour = match e.state {
        CatalogState::Failed => FAILED,
        CatalogState::Lost => QUIET,
        _ => ACCENT,
    };
    column()
        .bg(TILE)
        .border(LINE)
        .radius(6)
        .padding(4)
        .gap(2)
        .height(72)
        .child(button(id.clone(), e.name.clone()).size(13).bg(TILE).on_click(MenuMsg::Install(i)))
        .child(state.child(words))
        .child_if(e.state.has_bar(), bar(&id, e.percent, colour))
}

/// A bar `percent` filled in `colour`, the rest in the line's.
fn bar(id: &str, percent: u8, colour: u32) -> Node<MenuMsg> {
    let p = i32::from(percent.min(100));
    let part = |c: u32| column().height(4).bg(c);
    let bar = grid([fr(p), fr(100 - p)]).id(format!("{id}Bar")).height(4).radius(2);
    bar.child(part(colour)).child(part(LINE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn program(name: &str, path: &str) -> Program {
        Program { name: name.to_string(), path: path.to_string() }
    }

    fn entry(name: &str, path: &str) -> Vec<u8> {
        let mut e = vec![0u8; 8 + 64 + 256];
        let (n, p): (Vec<u16>, Vec<u16>) =
            (name.encode_utf16().collect(), path.encode_utf16().collect());
        e[0..4].copy_from_slice(&(n.len() as u32).to_le_bytes());
        e[4..8].copy_from_slice(&(p.len() as u32).to_le_bytes());
        for (k, u) in n.iter().enumerate() {
            e[8 + 2 * k..10 + 2 * k].copy_from_slice(&u.to_le_bytes());
        }
        for (k, u) in p.iter().enumerate() {
            e[72 + 2 * k..74 + 2 * k].copy_from_slice(&u.to_le_bytes());
        }
        e
    }

    #[test]
    fn parses_the_program_list() {
        let mut b = vec![1, 0, 0, 0, 2, 0, 0, 0];
        b.extend(entry("Command Prompt", "C:\\Windows\\System32\\cmd.exe"));
        b.extend(entry("Task Manager", "C:\\Windows\\System32\\Taskmgr.exe"));
        let p = parse_programs(&b);
        assert_eq!(
            p,
            [
                program("Command Prompt", "C:\\Windows\\System32\\cmd.exe"),
                program("Task Manager", "C:\\Windows\\System32\\Taskmgr.exe"),
            ]
        );
        assert!(parse_programs(&[1, 0]).is_empty());
    }

    #[test]
    fn the_search_filters_by_name_ignoring_case_and_opening_clears_it() {
        let mut m = StartMenu::default();
        m.open(vec![program("Command Prompt", "a"), program("Task Manager", "b")]);
        assert_eq!(m.shown(), [0, 1]);
        m.update(MenuMsg::Query("TASK".to_string()));
        assert_eq!(m.shown(), [1]);
        m.update(MenuMsg::Query("zz".to_string()));
        assert!(m.shown().is_empty());
        m.open(vec![program("Command Prompt", "a")]);
        assert_eq!((m.query.as_str(), m.shown()), ("", vec![0]));
    }

    fn catalog_entry(name: &str, id: &str, state: u32, percent: u32, exit: i32) -> Vec<u8> {
        let mut e = vec![0u8; CATALOG_ENTRY];
        let (n, i): (Vec<u16>, Vec<u16>) =
            (name.encode_utf16().collect(), id.encode_utf16().collect());
        for (k, v) in
            [n.len() as u32, i.len() as u32, state, percent, exit as u32].iter().enumerate()
        {
            e[4 * k..4 * k + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (k, u) in n.iter().enumerate() {
            e[20 + 2 * k..22 + 2 * k].copy_from_slice(&u.to_le_bytes());
        }
        for (k, u) in i.iter().enumerate() {
            e[84 + 2 * k..86 + 2 * k].copy_from_slice(&u.to_le_bytes());
        }
        e
    }

    fn tile(name: &str, id: &str, state: CatalogState, percent: u8, exit: i32) -> CatalogEntry {
        CatalogEntry { name: name.to_string(), id: id.to_string(), state, percent, exit }
    }

    #[test]
    fn parses_the_catalog() {
        let mut b = vec![1, 0, 0, 0, 3, 0, 0, 0];
        b.extend(catalog_entry("7-Zip", "7zip.7zip", 5, 100, 4));
        b.extend(catalog_entry("Git", "Git.Git", 3, 40, 0));
        // A state this shell does not know is left out.
        b.extend(catalog_entry("Python", "Python.Python.3.13", 99, 0, 0));
        assert_eq!(
            parse_catalog(&b),
            [
                tile("7-Zip", "7zip.7zip", CatalogState::Failed, 100, 4),
                tile("Git", "Git.Git", CatalogState::Downloading, 40, 0),
            ]
        );
        assert!(parse_catalog(&[1, 0]).is_empty());
    }

    #[test]
    fn a_catalog_tile_says_where_its_install_is() {
        let say =
            |state, percent, exit| state_text(&tile("7-Zip", "7zip.7zip", state, percent, exit));
        assert_eq!(say(CatalogState::NotInstalled, 0, 0), "Not installed");
        assert_eq!(say(CatalogState::Resolving, 5, 0), "Resolving");
        assert_eq!(say(CatalogState::Downloading, 40, 0), "Downloading 40%");
        assert_eq!(say(CatalogState::Installing, 85, 0), "Installing");
        assert_eq!(say(CatalogState::Failed, 100, 4), "The install failed.");
        assert_eq!(
            say(CatalogState::Failed, 100, 2),
            "The install failed: the source has no such package or version."
        );
        assert_eq!(say(CatalogState::Lost, 40, 0), "The install stopped without a word.");
    }

    /// The catalog's tiles come after the installed programs, the search
    /// filters them, one installed or listed among the programs is not
    /// shown twice, and a change of state is a line for the log.
    #[test]
    fn catalog_tiles_follow_the_programs_and_their_changes_are_logged() {
        let mut m = StartMenu::default();
        m.open(vec![program("Command Prompt", "a"), program("Git", "g")]);
        let lines = m.set_catalog(vec![
            tile("7-Zip", "7zip.7zip", CatalogState::NotInstalled, 0, 0),
            tile("Git", "Git.Git", CatalogState::Installed, 0, 0),
            tile("Python", "Python.Python.3.13", CatalogState::NotInstalled, 0, 0),
        ]);
        assert_eq!(
            lines,
            [
                "catalog: 7-Zip not installed",
                "catalog: Git installed",
                "catalog: Python not installed"
            ]
        );
        assert_eq!(m.shown_catalog(), [0, 2]);
        m.update(MenuMsg::Query("zip".to_string()));
        assert_eq!((m.shown(), m.shown_catalog()), (vec![], vec![0]));
        m.update(MenuMsg::Query(String::new()));
        // Only what changed is told, a bar that moves is not.
        let zip = |state, percent, exit| tile("7-Zip", "7zip.7zip", state, percent, exit);
        let rest = |m: &StartMenu| m.catalog[1..].to_vec();
        let again = |m: &mut StartMenu, e: CatalogEntry| {
            let mut c = vec![e];
            c.extend(rest(m));
            m.set_catalog(c)
        };
        assert_eq!(again(&mut m, zip(CatalogState::Resolving, 5, 0)), ["catalog: 7-Zip resolving"]);
        assert_eq!(
            again(&mut m, zip(CatalogState::Downloading, 10, 0)),
            ["catalog: 7-Zip downloading"]
        );
        assert!(again(&mut m, zip(CatalogState::Downloading, 40, 0)).is_empty());
        assert_eq!(
            again(&mut m, zip(CatalogState::Failed, 100, 4)),
            ["catalog: 7-Zip failed (gala exit 4)"]
        );
        assert!(!m.catalog[0].state.running());
        assert!(CatalogState::Installing.running());
    }

    /// 7 pixels a character, lines of 18.
    struct Fixed;
    impl gala_ui::Measure for Fixed {
        fn text(&mut self, t: &str, _: f32, _: bool, _: Option<f32>) -> (f32, f32) {
            (t.chars().count() as f32 * 7.0, 18.0)
        }
    }

    fn lay(m: &StartMenu) -> Vec<gala_ui::Placed<MenuMsg>> {
        let (w, h) = (MENU_WIDTH as f32, Some(MENU_HEIGHT as f32));
        gala_ui::place(&m.view(), w, h, &mut Fixed).unwrap()
    }

    fn find<'a>(
        p: &'a [gala_ui::Placed<MenuMsg>],
        id: &str,
    ) -> Option<&'a gala_ui::Placed<MenuMsg>> {
        p.iter().find(|x| x.id.as_deref() == Some(id))
    }

    /// A catalog tile: its button with the program's id and name, a click
    /// asking for the install; its state's Text; the badge before an
    /// install was asked for, the bar after, coloured by how it went.
    #[test]
    fn a_catalog_tile_is_a_button_its_state_and_its_bar() {
        let mut m = StartMenu::default();
        m.open(vec![program("Command Prompt", "a")]);
        m.set_catalog(vec![tile("7-Zip", "7zip.7zip", CatalogState::NotInstalled, 0, 0)]);
        let placed = lay(&m);
        let button = find(&placed, "Program7Zip").expect("the tile's button");
        assert_eq!((button.kind, button.text.as_str()), (gala_ui::Kind::Button, "7-Zip"));
        assert!(matches!(button.on_click, Some(MenuMsg::Install(0))));
        let state = find(&placed, "Program7ZipState").map(|s| s.text.as_str());
        assert_eq!(state, Some("Not installed"));
        assert!(find(&placed, "Program7ZipBadge").is_some());
        assert!(find(&placed, "Program7ZipBar").is_none());
        let cmd = find(&placed, "ProgramCommandPrompt").expect("the program's tile").rect;
        assert!(cmd[1] < button.rect[1] || (cmd[1] == button.rect[1] && cmd[0] < button.rect[0]));
        m.set_catalog(vec![tile("7-Zip", "7zip.7zip", CatalogState::Failed, 100, 4)]);
        let placed = lay(&m);
        let at = placed.iter().position(|p| p.id.as_deref() == Some("Program7ZipBar"));
        let parts: Vec<Option<u32>> = placed
            .iter()
            .filter(|p| p.parent == at && at.is_some())
            .map(|p| p.background)
            .collect();
        assert_eq!(parts, [Some(FAILED), Some(LINE)]);
        assert!(find(&placed, "Program7ZipBadge").is_none());
    }

    fn linux(
        kind: LinuxKind,
        state: LinuxState,
        percent: u8,
        index: u32,
        label: &str,
    ) -> LinuxEntry {
        LinuxEntry {
            kind,
            state,
            percent,
            index,
            name: "Ubuntu".to_string(),
            label: label.to_string(),
        }
    }

    /// The Linux section: its heading and a tile per entry after the
    /// programs and the catalog, a distribution's with its state's Text
    /// and, once an install was asked for, its bar; a terminal's and an
    /// application's a button; a click on each says which; the search
    /// filters them.
    #[test]
    fn the_linux_section_follows_the_catalog() {
        use LinuxKind::*;
        let mut m = StartMenu::default();
        m.open(vec![program("Command Prompt", "a")]);
        m.set_catalog(vec![tile("7-Zip", "7zip.7zip", CatalogState::NotInstalled, 0, 0)]);
        let lines =
            m.set_linux(vec![linux(Distribution, LinuxState::Downloading, 48, 0, "Ubuntu")]);
        assert_eq!(lines, ["linux: Ubuntu downloading"]);
        let placed = lay(&m);
        let b = find(&placed, "LinuxUbuntu").expect("the distribution's button");
        assert_eq!((b.kind, b.text.as_str()), (gala_ui::Kind::Button, "Ubuntu"));
        assert!(matches!(b.on_click, Some(MenuMsg::Linux(0))));
        let state = find(&placed, "LinuxUbuntuState").map(|s| s.text.as_str());
        assert_eq!(state, Some("Downloading 48%"));
        assert!(find(&placed, "LinuxUbuntuBar").is_some());
        assert!(find(&placed, "LinuxUbuntuBadge").is_none());
        let zip = find(&placed, "Program7Zip").expect("the catalog tile").rect;
        let title = find(&placed, "LinuxTitle").expect("the heading");
        assert_eq!(title.text, "Linux");
        assert!(zip[1] < title.rect[1] && title.rect[1] < b.rect[1]);
        m.set_linux(vec![
            linux(Distribution, LinuxState::Installed, 0, 0, "Ubuntu"),
            linux(Terminal, LinuxState::Installed, 0, 0, "Terminal"),
            linux(App, LinuxState::Installed, 0, 0, "XTerm"),
        ]);
        let placed = lay(&m);
        assert_eq!(find(&placed, "LinuxUbuntuState").map(|s| s.text.as_str()), Some("Installed"));
        assert!(find(&placed, "LinuxUbuntuBar").is_none());
        let t = find(&placed, "LinuxTerminalUbuntu").expect("the terminal's button");
        assert_eq!(t.text, "Terminal (Ubuntu)");
        assert!(matches!(t.on_click, Some(MenuMsg::Linux(1))));
        let a = find(&placed, "LinuxAppUbuntu_0").expect("the application's button");
        assert_eq!(a.text, "XTerm (Ubuntu)");
        assert!(matches!(a.on_click, Some(MenuMsg::Linux(2))));
        m.update(MenuMsg::Query("xterm".to_string()));
        assert_eq!(m.shown_linux(), [2]);
        m.update(MenuMsg::Query("zz".to_string()));
        let placed = lay(&m);
        assert!(find(&placed, "LinuxTitle").is_none() && find(&placed, "NoMatch").is_some());
    }

    /// The whole menu fits: the desktop's programs and catalog as the
    /// linux_menu image has them (every catalog tile not installed) and
    /// an installed distribution's tiles beside a suggested one's.
    #[test]
    fn a_full_menu_fits() {
        use LinuxKind::*;
        let mut m = StartMenu::default();
        let names = ["Command Prompt", "Hello window", "Linux", "Task Manager", "File Explorer"];
        m.open(names.iter().map(|n| program(n, "p")).collect());
        let ids = ["Office", "FileZilla", "NET", "Python", "Chromium", "Git", "7-Zip"];
        m.set_catalog(ids.iter().map(|n| tile(n, n, CatalogState::NotInstalled, 0, 0)).collect());
        let mut alpine = linux(Distribution, LinuxState::Failed, 100, 1, "Alpine");
        alpine.name = "Alpine".to_string();
        m.set_linux(vec![
            linux(Distribution, LinuxState::Installed, 0, 0, "Ubuntu"),
            linux(Terminal, LinuxState::Installed, 0, 0, "Terminal"),
            linux(App, LinuxState::Installed, 0, 0, "XTerm"),
            linux(App, LinuxState::Installed, 0, 1, "Files"),
            alpine,
        ]);
        let placed = lay(&m);
        let last = find(&placed, "LinuxAlpineBar").expect("the last tile's bar").rect;
        assert!(last[1] + last[3] <= MENU_HEIGHT - 24, "{last:?}");
    }

    #[test]
    fn tile_ids() {
        assert_eq!(program_id("Task Manager"), "ProgramTaskManager");
        assert_eq!(program_id(".NET"), "ProgramNET");
        assert_eq!(program_id("7-Zip"), "Program7Zip");
    }
}
