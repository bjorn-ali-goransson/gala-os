//! shasha's taskbar: its state, view and update (ROADMAP SH step 1,
//! part 6; its start menu, part 7, is [`StartMenu`]; ARCHITECTURE "The user-mode shell"), as a gala_ui component,
//! without system calls (host tests below).
//!
//! The strip is `HEIGHT` high and the screen's width, a hairline along
//! its top: the Start button (gala's mark, [`start_mark`]) and a button
//! per window the programs show, centred (as Windows 11's), and the time
//! over the date at the right. A window's button is named with its title
//! (its AutomationId `Task<hwnd in hex>`, [`task_id`]) and shows its
//! program's icon ([`Icon::of`]) and the title, an indicator along its
//! bottom: a long accent one for the active window, a short grey one
//! for the others; minimized windows' titles are dimmed.
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

use gala_ui::{Align, Justify, Node, button, column, row, rule, text};

pub mod keys;
pub mod linux;
mod menu;
pub mod tray;
pub mod wallpaper;
pub use menu::{
    CATALOG_ENTRY, CatalogEntry, CatalogState, MENU_GAP, MENU_HEIGHT, MENU_WIDTH, MenuMsg, Program,
    StartMenu, TILES_IN_ROW, parse_catalog, parse_programs, program_id, state_text,
};

/// The taskbar's height (the desktop's taskbar strip, TASKBAR).
pub const HEIGHT: i32 = 48;
/// A window's button: its width (the title cut to fit).
pub const TASK_WIDTH: i32 = 160;
/// A button's height in the strip (the Start button's and a window's).
const BUTTON: i32 = 40;
/// A program's icon: its side.
pub const ICON: i32 = 20;
/// The indicator under a window's button: the active window's width,
/// the others', its height.
pub const ACTIVE_BAR: i32 = 16;
pub const RUNNING_BAR: i32 = 6;
const BAR_HEIGHT: i32 = 3;

const SURFACE: u32 = 0xeef1f6;
const LINE: u32 = 0xd7dce3;
const INK: u32 = 0x1b1b1b;
const MUTED: u32 = 0x616161;
/// The active window's button: a shade lighter than the strip.
const ACTIVE: u32 = 0xf9fafc;
const ACTIVE_LINE: u32 = 0xe1e5eb;
const ACCENT: u32 = 0x005fb8;
const RUNNING: u32 = 0x8a8a8a;

/// The desktop's window list's flags (GalaDesktopAction's ACTION_WINDOWS).
pub const LISTED_MINIMIZED: u32 = 1;
pub const LISTED_ACTIVE: u32 = 4;

/// A window the taskbar shows a button for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub hwnd: u64,
    /// The window's process.
    pub pid: u32,
    /// Its image's file name, lower case (`notepad.exe`, [`program_name`]);
    /// empty until the program has looked it up.
    pub program: String,
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
    /// The clock's time (`HH:MM`) and date (`M/D/YYYY`).
    Clock(String, String),
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
    pub date: String,
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

/// `M/D/YYYY` (the date as Windows' en-US taskbar shows it).
pub fn date_text(year: u16, month: u16, day: u16) -> String {
    format!("{month}/{day}/{year}")
}

/// The file name of image path `path` (`C:\\Windows\\notepad.exe`), in
/// lower case: what [`Icon::of`] knows a program by.
pub fn program_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or("").to_ascii_lowercase()
}

/// What a window's button shows of its program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// A folder (File Explorer).
    Folder,
    /// A chart (Task Manager).
    Chart,
    /// A console (Command Prompt, PowerShell).
    Console,
    /// A page (Notepad).
    Page,
    /// Any other program: its initial on a colour of its name.
    Initial(char, u32),
}

/// The colours an initial is set on (white text reads on each).
const INITIAL_COLOURS: [u32; 6] = [0x0063b1, 0x8764b8, 0xc239b3, 0xca5010, 0x038387, 0x486860];

impl Icon {
    /// The icon of `program` (its [`program_name`]); a program it knows
    /// no picture for gets the initial of `title` (or of its name).
    pub fn of(program: &str, title: &str) -> Icon {
        match program {
            "explorer.exe" => Icon::Folder,
            "taskmgr.exe" => Icon::Chart,
            "cmd.exe" | "powershell.exe" | "pwsh.exe" | "conhost.exe" => Icon::Console,
            "notepad.exe" => Icon::Page,
            _ => {
                let initial = title
                    .chars()
                    .chain(program.chars())
                    .find(|c| c.is_alphanumeric())
                    .map_or('?', |c| c.to_uppercase().next().unwrap_or(c));
                // The same program, the same colour, from boot to boot.
                let hash =
                    program.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b.into()));
                Icon::Initial(initial, INITIAL_COLOURS[hash as usize % INITIAL_COLOURS.len()])
            }
        }
    }

    /// The icon as elements: an `ICON`-square box of drawn shapes.
    pub fn view<M: Clone>(self) -> Node<M> {
        let at = |x: i32, y: i32, w: i32, h: i32, c: u32| row().pin(x, y).width(w).height(h).bg(c);
        let square = column().width(ICON).height(ICON);
        match self {
            Icon::Folder => square
                .child(at(1, 3, 8, 4, 0xd99a1e).radius(1))
                .child(at(1, 5, 18, 13, 0xf5c242).radius(2))
                .child(at(1, 8, 18, 10, 0xffd666).radius(2)),
            Icon::Chart => square
                .child(at(1, 1, 18, 18, 0xffffff).border(0xb9bec6).radius(3))
                .child(at(5, 10, 3, 6, 0x0f7b0f))
                .child(at(9, 5, 3, 11, 0x0f7b0f))
                .child(at(13, 8, 3, 8, 0x0f7b0f)),
            Icon::Console => square
                .child(at(0, 1, 20, 18, 0x1f1f1f).radius(3))
                .child(text(">_").pin(3, 3).width(16).size(11).bold().color(0xffffff)),
            Icon::Page => square
                .child(at(3, 0, 14, 20, 0xffffff).border(0x8a9099).radius(2))
                .child(at(6, 5, 8, 1, 0x8a9099))
                .child(at(6, 9, 8, 1, 0x8a9099))
                .child(at(6, 13, 5, 1, 0x8a9099)),
            Icon::Initial(c, bg) => square
                .bg(bg)
                .radius(4)
                .align(Align::Center)
                .justify(Justify::Center)
                .child(text(String::from(c)).size(11).bold().color(0xffffff)),
        }
    }
}

/// gala's mark on the Start button: a rounded accent square holding a
/// white "g" (an original mark, docs/CLEAN_ROOM.md).
pub fn start_mark<M: Clone>() -> Node<M> {
    column()
        .width(24)
        .height(24)
        .radius(6)
        .bg(ACCENT)
        .align(Align::Center)
        .justify(Justify::Center)
        .child(text("g").size(16).bold().color(0xffffff))
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
            pid: u32_at(e, 8),
            program: String::new(),
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
            Msg::Clock(c, d) => {
                self.clock = c;
                self.date = d;
            }
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
            let shown = t.active && !t.minimized;
            let (bar, bar_colour) =
                if shown { (ACTIVE_BAR, ACCENT) } else { (RUNNING_BAR, RUNNING) };
            let b = button(task_id(t.hwnd), t.title.clone())
                .width(TASK_WIDTH)
                .height(BUTTON)
                .radius(6)
                .padding((0, 10))
                .gap(8)
                .align(Align::Center)
                .size(13)
                .on_click(Msg::Task(t.hwnd))
                .child(Icon::of(&t.program, &t.title).view())
                .child(text(t.title.clone()).ellipsis().grow(1))
                .child(
                    row()
                        .pin((TASK_WIDTH - bar) / 2, BUTTON - BAR_HEIGHT)
                        .width(bar)
                        .height(BAR_HEIGHT)
                        .radius(1)
                        .bg(bar_colour),
                );
            if shown {
                b.bg(ACTIVE).border(ACTIVE_LINE).color(INK)
            } else if t.minimized {
                b.color(MUTED)
            } else {
                b.color(INK)
            }
        });
        let strip = row()
            .grow(1)
            .padding((0, 8))
            .align(Align::Center)
            .child(row().grow(1))
            .child(
                row()
                    .id("Tasks")
                    .gap(4)
                    .align(Align::Center)
                    .child(
                        button("StartButton", "Start")
                            .width(BUTTON)
                            .height(BUTTON)
                            .radius(6)
                            .align(Align::Center)
                            .justify(Justify::Center)
                            .on_click(Msg::Start)
                            .child(start_mark()),
                    )
                    .children(tasks),
            )
            .child(
                row()
                    .grow(1)
                    .gap(4)
                    .justify(Justify::End)
                    .align(Align::Center)
                    .child(self.tray.view(Msg::TrayClick, Msg::TrayRightClick))
                    .child(
                        column()
                            .id("TimeAndDate")
                            .padding((0, 8))
                            .align(Align::End)
                            .child(text(self.clock.clone()).id("Clock").size(12))
                            .child(text(self.date.clone()).id("Date").size(12)),
                    ),
            );
        column()
            .id("Taskbar")
            .bg(SURFACE)
            .color(INK)
            .height(HEIGHT)
            .child(rule().color(LINE))
            .child(strip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn task(hwnd: u64, program: &str, title: &str, active: bool, minimized: bool) -> Task {
        Task {
            hwnd,
            pid: 0,
            program: program.to_string(),
            title: title.to_string(),
            active,
            minimized,
        }
    }

    fn entry(hwnd: u64, flags: u32, title: &str) -> Vec<u8> {
        let mut e = vec![0u8; 40 + 256];
        e[0..8].copy_from_slice(&hwnd.to_le_bytes());
        e[8..12].copy_from_slice(&0x44u32.to_le_bytes());
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
            Task {
                hwnd: 0x20010,
                pid: 0x44,
                program: String::new(),
                title: "One".to_string(),
                active: true,
                minimized: false
            }
        );
        assert!(t[1].minimized && !t[1].active);
        assert_eq!(t[1].pid, 0x44);
        assert!(parse_windows(&[1, 0]).is_empty());
    }

    #[test]
    fn a_click_minimizes_the_active_window_else_activates() {
        let mut bar = Taskbar::default();
        bar.update(Msg::Windows(vec![
            task(1, "", "a", true, false),
            task(2, "", "b", false, false),
            task(3, "", "c", true, true),
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
        assert_eq!(date_text(2026, 3, 7), "3/7/2026");
        assert_eq!(program_name("C:\\Windows\\System32\\Taskmgr.EXE"), "taskmgr.exe");
        assert_eq!(program_name("notepad.exe"), "notepad.exe");
    }

    #[test]
    fn a_program_gets_its_icon_or_its_initial() {
        assert_eq!(Icon::of("explorer.exe", "File Explorer"), Icon::Folder);
        assert_eq!(Icon::of("taskmgr.exe", "Task Manager"), Icon::Chart);
        assert_eq!(Icon::of("cmd.exe", "Command Prompt"), Icon::Console);
        assert_eq!(Icon::of("notepad.exe", "a.txt - Notepad"), Icon::Page);
        let Icon::Initial(c, bg) = Icon::of("tray_test.exe", "  tray test") else { panic!() };
        assert_eq!(c, 'T');
        assert!(INITIAL_COLOURS.contains(&bg));
        // The same program, the same colour; a title with no letter, the program's.
        assert_eq!(Icon::of("tray_test.exe", "other"), Icon::Initial('O', bg));
        assert!(matches!(Icon::of("x.exe", "--"), Icon::Initial('X', _)));
    }

    /// 7 pixels a character, lines of 18.
    struct Fixed;
    impl gala_ui::Measure for Fixed {
        fn text(&mut self, t: &str, _: f32, _: bool, _: Option<f32>) -> (f32, f32) {
            (t.chars().count() as f32 * 7.0, 18.0)
        }
    }

    #[test]
    fn the_strip_shows_buttons_with_icons_bars_and_the_time_over_the_date() {
        let mut bar = Taskbar::default();
        bar.update(Msg::Windows(vec![
            task(0x10, "notepad.exe", "a.txt - Notepad", true, false),
            task(0x20, "explorer.exe", "File Explorer", false, true),
        ]));
        bar.update(Msg::Clock("09:05".to_string(), "3/7/2026".to_string()));
        let p = gala_ui::place(&bar.view(), 1280.0, Some(HEIGHT as f32), &mut Fixed).unwrap();
        let find = |id: &str| p.iter().position(|x| x.id.as_deref() == Some(id)).unwrap();
        // Buttons named for UIA, drawn by what they hold.
        let (start, active, other) = (find("StartButton"), find("Task10"), find("Task20"));
        assert_eq!(gala_ui::uia::name(&p[start]), "Start");
        assert_eq!(gala_ui::uia::name(&p[active]), "a.txt - Notepad");
        assert!(p[start].content && p[active].content);
        assert_eq!(&p[active].rect[2..], &[TASK_WIDTH, 40]);
        // In the strip, centred upright.
        let [_, y, _, h] = p[active].rect;
        assert!(y >= 1 && y + h <= HEIGHT, "{:?}", p[active].rect);
        // The indicator: long and accent for the active, short for the other.
        let indicator = |b: usize| {
            p.iter()
                .find(|x| x.parent == Some(b) && x.rect[3] == BAR_HEIGHT)
                .map(|x| (x.rect[2], x.background))
                .unwrap()
        };
        assert_eq!(indicator(active), (ACTIVE_BAR, Some(ACCENT)));
        assert_eq!(indicator(other), (RUNNING_BAR, Some(RUNNING)));
        // The title, cut to fit, the minimized one's dimmed.
        let title = |b: usize| p.iter().find(|x| x.parent == Some(b) && x.ellipsis).unwrap();
        assert_eq!(title(active).text, "a.txt - Notepad");
        assert_eq!(title(other).color, MUTED);
        // The time above the date, at the right.
        let (clock, date) = (&p[find("Clock")], &p[find("Date")]);
        assert_eq!((clock.text.as_str(), date.text.as_str()), ("09:05", "3/7/2026"));
        assert!(clock.rect[1] < date.rect[1]);
        assert!(clock.rect[0] + clock.rect[2] > 1280 - 40);
        // A click anywhere on a window's button is that window's.
        let [x, y, ..] = p[other].rect;
        assert!(matches!(gala_ui::click(&p, x + 5, y + 10), Some(("Task20", Msg::Task(0x20)))));
    }
}
