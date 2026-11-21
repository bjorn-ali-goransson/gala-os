//! shasha's taskbar: its state, view and update (ROADMAP SH step 1,
//! part 6; its start menu, part 7, is [`StartMenu`]; ARCHITECTURE "The user-mode shell"), as a gala_ui component,
//! without system calls (host tests below).
//!
//! The strip is `HEIGHT` high and the screen's width: the Start button
//! and a button per window the programs show, centred (as Windows 11's),
//! and the clock at the right. A window's button is named with its title
//! (its AutomationId `Task<hwnd in hex>`, [`task_id`]) and marked when the
//! window is active; minimized windows' are dimmed.
//!
//! What a click does is [`Taskbar::click`]'s answer, which the program
//! asks the desktop for (GalaDesktopAction): the active window's button
//! minimizes it, any other's activates (and restores) its window, as
//! Windows' taskbar does.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use gala_ui::{Align, Justify, Node, button, row, text};

mod menu;
pub mod tray;
pub mod wallpaper;
pub use menu::{
    CATALOG_ENTRY, CatalogEntry, CatalogState, MENU_GAP, MENU_HEIGHT, MENU_WIDTH, MenuMsg, Program,
    StartMenu, TILES_IN_ROW, parse_catalog, parse_programs, program_id, state_text,
};

/// The taskbar's height (the desktop's own taskbar's, TASKBAR).
pub const HEIGHT: i32 = 48;
/// A window's button: its width (the title cut to fit).
pub const TASK_WIDTH: i32 = 160;

const SURFACE: u32 = 0xeef1f6;
const LINE: u32 = 0xd7dce3;
const INK: u32 = 0x1b1b1b;
const MUTED: u32 = 0x616161;
const ACTIVE: u32 = 0xffffff;
const ACCENT: u32 = 0x005fb8;

/// The desktop's window list's flags (GalaDesktopAction's ACTION_WINDOWS).
pub const LISTED_MINIMIZED: u32 = 1;
pub const LISTED_ACTIVE: u32 = 4;

/// A window the taskbar shows a button for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub hwnd: u64,
    pub title: String,
    pub active: bool,
    pub minimized: bool,
}

/// What a click on a window's button asks of the desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    Activate(u64),
    Minimize(u64),
}

#[derive(Clone, Debug)]
pub enum Msg {
    /// The Start button.
    Start,
    /// A window's button (its HWND).
    Task(u64),
    /// The desktop's window list, read again.
    Windows(Vec<Task>),
    /// The clock's text (`HH:MM`).
    Clock(String),
    /// A click on a notification-area icon (its window and id).
    TrayClick(u64, u32),
    /// A right click on one.
    TrayRightClick(u64, u32),
    /// The notification area changed (a Shell_NotifyIcon, a balloon's
    /// time, an icon's window gone): the view is built again.
    TrayChanged,
}

#[derive(Default)]
pub struct Taskbar {
    pub tasks: Vec<Task>,
    pub clock: String,
    /// The notification area, left of the clock.
    pub tray: tray::Tray,
}

/// The AutomationId of window `hwnd`'s button.
pub fn task_id(hwnd: u64) -> String {
    format!("Task{hwnd:x}")
}

/// `HH:MM` for `hour` and `minute`.
pub fn clock_text(hour: u16, minute: u16) -> String {
    format!("{hour:02}:{minute:02}")
}

/// The windows in ACTION_WINDOWS's output `b` (an 8-byte header, then
/// `WINDOW_ENTRY`-byte records: hwnd, pid, screen id, flags, rectangle,
/// title length, title), but those with no HWND (no record in the window
/// registry: nothing to act on).
pub fn parse_windows(b: &[u8]) -> Vec<Task> {
    const HEADER: usize = 8;
    const TITLE: usize = 128;
    const ENTRY: usize = 40 + 2 * TITLE;
    let u32_at = |e: &[u8], at: usize| u32::from_le_bytes([e[at], e[at + 1], e[at + 2], e[at + 3]]);
    let mut out = Vec::new();
    for e in b.get(HEADER..).unwrap_or(&[]).chunks_exact(ENTRY) {
        let hwnd = u64::from(u32_at(e, 0)) | u64::from(u32_at(e, 4)) << 32;
        if hwnd == 0 {
            continue;
        }
        let flags = u32_at(e, 16);
        let n = (u32_at(e, 36) as usize).min(TITLE);
        let units: Vec<u16> =
            (0..n).map(|k| u16::from_le_bytes([e[40 + 2 * k], e[41 + 2 * k]])).collect();
        out.push(Task {
            hwnd,
            title: String::from_utf16_lossy(&units),
            active: flags & LISTED_ACTIVE != 0,
            minimized: flags & LISTED_MINIMIZED != 0,
        });
    }
    out
}

impl Taskbar {
    /// What a click on window `hwnd`'s button asks for: minimize the
    /// active (shown) window, else activate it. None for a window no
    /// longer listed.
    pub fn click(&self, hwnd: u64) -> Option<Click> {
        let t = self.tasks.iter().find(|t| t.hwnd == hwnd)?;
        Some(if t.active && !t.minimized { Click::Minimize(hwnd) } else { Click::Activate(hwnd) })
    }

    pub fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Windows(tasks) => self.tasks = tasks,
            Msg::Clock(c) => self.clock = c,
            // The program asks the desktop; the list it reads back after
            // the WinEvents says what came of it.
            Msg::Start
            | Msg::Task(_)
            | Msg::TrayClick(..)
            | Msg::TrayRightClick(..)
            | Msg::TrayChanged => {}
        }
    }

    pub fn view(&self) -> Node<Msg> {
        let tasks = self.tasks.iter().map(|t| {
            let b = button(task_id(t.hwnd), t.title.clone())
                .width(TASK_WIDTH)
                .height(40)
                .size(13)
                .on_click(Msg::Task(t.hwnd));
            if t.active && !t.minimized {
                b.bg(ACTIVE).border(ACCENT).color(INK)
            } else if t.minimized {
                b.bg(SURFACE).color(MUTED)
            } else {
                b.bg(SURFACE).color(INK)
            }
        });
        row()
            .id("Taskbar")
            .bg(SURFACE)
            .border(LINE)
            .color(INK)
            .height(HEIGHT)
            .padding((4, 8))
            .align(Align::Center)
            .child(row().grow(1))
            .child(
                row()
                    .id("Tasks")
                    .gap(4)
                    .align(Align::Center)
                    .child(
                        button("StartButton", "Start")
                            .width(64)
                            .height(40)
                            .bold()
                            .color(ACCENT)
                            .on_click(Msg::Start),
                    )
                    .children(tasks),
            )
            .child(
                row()
                    .grow(1)
                    .justify(Justify::End)
                    .align(Align::Center)
                    .child(self.tray.view(Msg::TrayClick, Msg::TrayRightClick))
                    .child(text(self.clock.clone()).id("Clock").size(13).width(56)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn entry(hwnd: u64, flags: u32, title: &str) -> Vec<u8> {
        let mut e = vec![0u8; 40 + 256];
        e[0..8].copy_from_slice(&hwnd.to_le_bytes());
        e[16..20].copy_from_slice(&flags.to_le_bytes());
        let units: Vec<u16> = title.encode_utf16().collect();
        e[36..40].copy_from_slice(&(units.len() as u32).to_le_bytes());
        for (k, u) in units.iter().enumerate() {
            e[40 + 2 * k..42 + 2 * k].copy_from_slice(&u.to_le_bytes());
        }
        e
    }

    #[test]
    fn parses_the_window_list() {
        let mut b = vec![1, 0, 0, 0, 3, 0, 0, 0];
        b.extend(entry(0x20010, LISTED_ACTIVE, "One"));
        b.extend(entry(0, 0, "No record"));
        b.extend(entry(0x20020, LISTED_MINIMIZED, "Two"));
        let t = parse_windows(&b);
        assert_eq!(t.len(), 2);
        assert_eq!(
            t[0],
            Task { hwnd: 0x20010, title: "One".to_string(), active: true, minimized: false }
        );
        assert!(t[1].minimized && !t[1].active);
        assert!(parse_windows(&[1, 0]).is_empty());
    }

    #[test]
    fn a_click_minimizes_the_active_window_else_activates() {
        let mut bar = Taskbar::default();
        bar.update(Msg::Windows(vec![
            Task { hwnd: 1, title: "a".to_string(), active: true, minimized: false },
            Task { hwnd: 2, title: "b".to_string(), active: false, minimized: false },
            Task { hwnd: 3, title: "c".to_string(), active: true, minimized: true },
        ]));
        assert_eq!(bar.click(1), Some(Click::Minimize(1)));
        assert_eq!(bar.click(2), Some(Click::Activate(2)));
        assert_eq!(bar.click(3), Some(Click::Activate(3)));
        assert_eq!(bar.click(4), None);
    }

    #[test]
    fn ids_and_clock() {
        assert_eq!(task_id(0x20010), "Task20010");
        assert_eq!(clock_text(9, 5), "09:05");
    }
}
