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
//! The program shows and hides the window; what this keeps is the list
//! and the search, which [`StartMenu::open`] clears.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use gala_ui::{Node, button, column, fr, grid, input, text};

/// The menu's size, and its gap above the taskbar.
pub const MENU_WIDTH: i32 = 520;
pub const MENU_HEIGHT: i32 = 420;
pub const MENU_GAP: i32 = 12;
/// Tiles in a row.
pub const TILES_IN_ROW: usize = 4;

const SURFACE: u32 = 0xeef1f6;
const LINE: u32 = 0xd7dce3;
const TILE: u32 = 0xffffff;
const INK: u32 = 0x1b1b1b;
const MUTED: u32 = 0x616161;

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
    /// The search's text.
    Query(String),
    /// A tile (the program's index in the list).
    Launch(usize),
}

#[derive(Default)]
pub struct StartMenu {
    pub programs: Vec<Program>,
    pub query: String,
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
        // Toggle, Deactivated and Launch are the program's to act on.
    }

    pub fn view(&self) -> Node<MenuMsg> {
        let shown = self.shown();
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
        let list = if shown.is_empty() {
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
    }
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

    #[test]
    fn tile_ids() {
        assert_eq!(program_id("Task Manager"), "ProgramTaskManager");
        assert_eq!(program_id(".NET"), "ProgramNET");
        assert_eq!(program_id("7-Zip"), "Program7Zip");
    }
}
