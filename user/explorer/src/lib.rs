//! gala's File Explorer (ROADMAP EX; ARCHITECTURE "The file explorer"):
//! the parts without system calls, so they run under `cargo test` on the
//! host.
//!
//! - [`Entry`] is one item of the folder shown (from shell32's
//!   namespace: its parsing and display names, attributes, size, time);
//!   [`rows`] are the view's rows: filtered by the search (a name that
//!   holds the text, ASCII case ignored), folders first, then sorted by
//!   the chosen [`Column`] (names as StrCmpLogicalW compares them);
//! - the numbers' and dates' formats as Windows' Details view shows them
//!   ([`size_text`]: KB rounded up with thousands separators;
//!   [`time_text`]: `M/D/YYYY h:mm AM`);
//! - [`type_text`]: the Type column ("File folder", "Application",
//!   "<EXT> File");
//! - [`breadcrumbs`]: the address bar's text when not editing;
//! - [`mode_text`]: a Linux file's permissions (`-rw-r--r--`), shown in
//!   Properties;
//! - [`new_folder_name`]: "New folder", then "New folder (2)", ...;
//! - [`icon_grid`]: the icon view's layout.
//!
//! `no_std` with `alloc`: `explorer.exe` is freestanding (`gala_heap`).

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt::Write;

/// What an entry is (its icon).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// "This PC".
    Computer,
    /// The Linux node.
    Linux,
    Drive,
    /// A WSL distribution.
    Distro,
    Folder,
    File,
}

impl Kind {
    pub fn is_folder(self) -> bool {
        self != Kind::File
    }
}

/// One item of the folder shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// SIGDN_NORMALDISPLAY.
    pub name: String,
    /// SIGDN_DESKTOPABSOLUTEPARSING: what navigation and opening use.
    pub parsing: String,
    pub kind: Kind,
    /// Bytes (files).
    pub size: u64,
    /// Last write, NT time (100 ns since 1601; 0: none).
    pub modified: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Name,
    Modified,
    Type,
    Size,
}

/// The Details view's columns and widths.
pub const COLUMNS: [(Column, &str, i32); 4] = [
    (Column::Name, "Name", 300),
    (Column::Modified, "Date modified", 170),
    (Column::Type, "Type", 140),
    (Column::Size, "Size", 100),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub column: Column,
    pub descending: bool,
}

impl Sort {
    pub const NAME: Sort = Sort { column: Column::Name, descending: false };

    /// The order after a click on `column`'s header: that column
    /// ascending, or the other way when it already sorts.
    pub fn clicked(self, column: Column) -> Sort {
        if self.column == column {
            Sort { column, descending: !self.descending }
        } else {
            Sort { column, descending: false }
        }
    }
}

fn fold(c: char) -> char {
    c.to_ascii_lowercase()
}

/// Whether `name` holds `search` (ASCII case ignored; empty: all).
pub fn matches(name: &str, search: &str) -> bool {
    if search.is_empty() {
        return true;
    }
    let n: Vec<char> = name.chars().map(fold).collect();
    let s: Vec<char> = search.chars().map(fold).collect();
    n.windows(s.len()).any(|w| w == s.as_slice())
}

fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// The view's rows (indices into `entries`): those that match `search`,
/// folders first, in `sort`'s order (ties by name).
pub fn rows(entries: &[Entry], sort: Sort, search: &str) -> Vec<usize> {
    let mut out: Vec<usize> =
        (0..entries.len()).filter(|&i| matches(&entries[i].name, search)).collect();
    out.sort_by(|&a, &b| {
        let (x, y) = (&entries[a], &entries[b]);
        let by_name = || shellns::compare_names(&units(&x.name), &units(&y.name));
        let order = match sort.column {
            Column::Name => by_name(),
            Column::Modified => x.modified.cmp(&y.modified).then_with(by_name),
            Column::Type => type_text(x).cmp(&type_text(y)).then_with(by_name),
            Column::Size => x.size.cmp(&y.size).then_with(by_name),
        };
        let order = if sort.descending { order.reverse() } else { order };
        // Roots, drives and distributions keep their place; folders first.
        let rank = |e: &Entry| match e.kind {
            Kind::Computer | Kind::Linux => 0,
            Kind::Drive | Kind::Distro => 1,
            Kind::Folder => 2,
            Kind::File => 3,
        };
        match rank(x).cmp(&rank(y)) {
            Ordering::Equal if rank(x) <= 1 => a.cmp(&b),
            Ordering::Equal => order,
            other => other,
        }
    });
    out
}

/// The Size column: KB rounded up, thousands separated (`1,234 KB`); a
/// folder has none.
pub fn size_text(e: &Entry) -> String {
    if e.kind.is_folder() {
        return String::new();
    }
    let kb = e.size.div_ceil(1024);
    let mut digits = String::new();
    let _ = write!(digits, "{kb}");
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out.push_str(" KB");
    out
}

/// Days since 1970-01-01 as (year, month, day) (the proleptic Gregorian
/// calendar's civil date).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// The Date modified column (`11/12/2026 7:32 PM`, as en-US Windows shows
/// it; UTC: gala's clock has no time zone yet); empty for no time.
pub fn time_text(nt: u64) -> String {
    if nt == 0 {
        return String::new();
    }
    const EPOCH: u64 = 116_444_736_000_000_000;
    let secs = (nt.saturating_sub(EPOCH) / 10_000_000) as i64;
    let (y, m, d) = civil(secs.div_euclid(86_400));
    let in_day = secs.rem_euclid(86_400);
    let (h, min) = (in_day / 3600, in_day % 3600 / 60);
    let (h12, half) = match h {
        0 => (12, "AM"),
        1..=11 => (h, "AM"),
        12 => (12, "PM"),
        _ => (h - 12, "PM"),
    };
    let mut s = String::new();
    let _ = write!(s, "{m}/{d}/{y} {h12}:{min:02} {half}");
    s
}

/// The Type column.
pub fn type_text(e: &Entry) -> String {
    match e.kind {
        Kind::Computer | Kind::Linux => String::from("System Folder"),
        Kind::Drive => String::from("Local Disk"),
        Kind::Distro => String::from("Linux distribution"),
        Kind::Folder => String::from("File folder"),
        Kind::File => match e.name.rfind('.') {
            Some(i) if i > 0 && i + 1 < e.name.len() => {
                let ext = &e.name[i + 1..];
                if ext.eq_ignore_ascii_case("exe") {
                    String::from("Application")
                } else {
                    let mut s = ext.to_ascii_uppercase();
                    s.push_str(" File");
                    s
                }
            }
            _ => String::from("File"),
        },
    }
}

/// The address bar's text when not editing: the folder's names from the
/// root, `>`-separated (`This PC > Local Disk (C:) > Windows`).
pub fn breadcrumbs(names: &[String]) -> String {
    let mut s = String::new();
    for (i, n) in names.iter().enumerate() {
        if i > 0 {
            s.push_str("  >  ");
        }
        s.push_str(n);
    }
    s
}

/// A Linux mode as `ls -l` shows it (`drwxr-xr-x`, `-rw-r--r--`,
/// `lrwxrwxrwx`), with the set-id and sticky bits.
pub fn mode_text(mode: u32) -> String {
    let kind = match mode & 0o170_000 {
        0o040_000 => 'd',
        0o120_000 => 'l',
        0o140_000 => 's',
        0o010_000 => 'p',
        0o020_000 => 'c',
        0o060_000 => 'b',
        _ => '-',
    };
    let mut s = String::new();
    s.push(kind);
    for (shift, special, set) in [(6, 0o4000, 's'), (3, 0o2000, 's'), (0, 0o1000, 't')] {
        let bits = (mode >> shift) & 7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        let x = bits & 1 != 0;
        s.push(match (mode & special != 0, x) {
            (true, true) => set,
            (true, false) => set.to_ascii_uppercase(),
            (false, true) => 'x',
            (false, false) => '-',
        });
    }
    s
}

/// The name of a new folder among `existing` names: "New folder", else
/// "New folder (2)", "(3)", ... (ASCII case ignored, as on Windows).
pub fn new_folder_name(existing: &[String]) -> String {
    let taken = |n: &str| existing.iter().any(|e| e.eq_ignore_ascii_case(n));
    let base = "New folder";
    if !taken(base) {
        return String::from(base);
    }
    let mut i = 2;
    loop {
        let mut n = String::from(base);
        let _ = write!(n, " ({i})");
        if !taken(&n) {
            return n;
        }
        i += 1;
    }
}

/// The icon view: tiles of `TILE_W` x `TILE_H` in rows as wide as the
/// pane; tile `i`'s top-left corner in a pane `width` wide.
pub const TILE_W: i32 = 112;
pub const TILE_H: i32 = 96;

pub fn icon_grid(i: usize, width: i32) -> (i32, i32) {
    let per_row = (width / TILE_W).max(1) as usize;
    (((i % per_row) as i32) * TILE_W, ((i / per_row) as i32) * TILE_H)
}

/// The tile under (x, y) in a pane `width` wide (None between rows'
/// ends).
pub fn icon_at(x: i32, y: i32, width: i32) -> Option<usize> {
    if x < 0 || y < 0 {
        return None;
    }
    let per_row = (width / TILE_W).max(1);
    let col = x / TILE_W;
    if col >= per_row {
        return None;
    }
    Some(((y / TILE_H) * per_row + col) as usize)
}

/// The Linux path of a file below `\\wsl.localhost\<distro>` (its
/// distribution, then `/a/b`), for "Open in Terminal" and Properties.
pub fn linux_path(parsing: &str) -> Option<(String, String)> {
    let rest = parsing
        .strip_prefix("\\\\wsl.localhost\\")
        .or_else(|| parsing.strip_prefix("\\\\wsl$\\"))?;
    let (distro, path) = match rest.find('\\') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let mut p: String = path.chars().map(|c| if c == '\\' { '/' } else { c }).collect();
    if p.is_empty() {
        p.push('/');
    }
    Some((String::from(distro), p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    fn e(name: &str, kind: Kind, size: u64, modified: u64) -> Entry {
        Entry { name: String::from(name), parsing: String::from(name), kind, size, modified }
    }

    #[test]
    fn rows_folders_first_then_sorted() {
        let es = vec![
            e("b.txt", Kind::File, 10, 3),
            e("Zeta", Kind::Folder, 0, 1),
            e("a10.txt", Kind::File, 5, 2),
            e("a2.txt", Kind::File, 50, 1),
            e("alpha", Kind::Folder, 0, 2),
        ];
        assert_eq!(rows(&es, Sort::NAME, ""), vec![4, 1, 3, 2, 0]);
        let by_size = Sort::NAME.clicked(Column::Size);
        assert_eq!(rows(&es, by_size, ""), vec![4, 1, 2, 0, 3]);
        assert_eq!(rows(&es, by_size.clicked(Column::Size), ""), vec![1, 4, 3, 0, 2]);
        assert_eq!(rows(&es, Sort::NAME, "A"), vec![4, 1, 3, 2]);
        assert_eq!(rows(&es, Sort::NAME, "txt"), vec![3, 2, 0]);
    }

    #[test]
    fn roots_and_drives_keep_their_order() {
        let es = vec![e("This PC", Kind::Computer, 0, 0), e("Linux", Kind::Linux, 0, 0)];
        assert_eq!(rows(&es, Sort::NAME, ""), vec![0, 1]);
        assert_eq!(rows(&es, Sort::NAME.clicked(Column::Name), ""), vec![0, 1]);
    }

    #[test]
    fn sizes_in_kb() {
        assert_eq!(size_text(&e("x", Kind::File, 0, 0)), "0 KB");
        assert_eq!(size_text(&e("x", Kind::File, 1, 0)), "1 KB");
        assert_eq!(size_text(&e("x", Kind::File, 1024, 0)), "1 KB");
        assert_eq!(size_text(&e("x", Kind::File, 1025, 0)), "2 KB");
        assert_eq!(size_text(&e("x", Kind::File, 1_263_616, 0)), "1,234 KB");
        assert_eq!(size_text(&e("x", Kind::Folder, 9, 0)), "");
    }

    #[test]
    fn times_as_en_us() {
        // 2026-11-12 19:32:00 UTC.
        let unix = 1_794_511_920u64;
        let nt = unix * 10_000_000 + 116_444_736_000_000_000;
        assert_eq!(time_text(nt), "11/12/2026 7:32 PM");
        let midnight = (unix - 19 * 3600 - 32 * 60) * 10_000_000 + 116_444_736_000_000_000;
        assert_eq!(time_text(midnight), "11/12/2026 12:00 AM");
        assert_eq!(time_text(0), "");
    }

    #[test]
    fn types() {
        assert_eq!(type_text(&e("x.exe", Kind::File, 0, 0)), "Application");
        assert_eq!(type_text(&e("notes.galatxt", Kind::File, 0, 0)), "GALATXT File");
        assert_eq!(type_text(&e("Makefile", Kind::File, 0, 0)), "File");
        assert_eq!(type_text(&e(".profile", Kind::File, 0, 0)), "File");
        assert_eq!(type_text(&e("etc", Kind::Folder, 0, 0)), "File folder");
    }

    #[test]
    fn linux_modes() {
        assert_eq!(mode_text(0o100_644), "-rw-r--r--");
        assert_eq!(mode_text(0o040_755), "drwxr-xr-x");
        assert_eq!(mode_text(0o041_777), "drwxrwxrwt");
        assert_eq!(mode_text(0o120_777), "lrwxrwxrwx");
        assert_eq!(mode_text(0o104_755), "-rwsr-xr-x");
    }

    #[test]
    fn new_folders_are_numbered() {
        assert_eq!(new_folder_name(&[]), "New folder");
        let taken = vec![String::from("new folder"), String::from("New folder (2)")];
        assert_eq!(new_folder_name(&taken), "New folder (3)");
    }

    #[test]
    fn the_icon_grid() {
        assert_eq!(icon_grid(0, 500), (0, 0));
        assert_eq!(icon_grid(5, 500), (TILE_W, TILE_H));
        assert_eq!(icon_at(TILE_W + 3, TILE_H + 3, 500), Some(5));
        assert_eq!(icon_at(4 * TILE_W + 3, 0, 500), None);
    }

    #[test]
    fn linux_paths() {
        assert_eq!(
            linux_path("\\\\wsl.localhost\\bb\\tmp\\x"),
            Some((String::from("bb"), String::from("/tmp/x")))
        );
        assert_eq!(
            linux_path("\\\\wsl.localhost\\bb"),
            Some((String::from("bb"), String::from("/")))
        );
        assert_eq!(linux_path("C:\\x"), None);
    }

    #[test]
    fn crumbs() {
        let names = vec![String::from("This PC"), String::from("Local Disk (C:)")];
        assert_eq!(breadcrumbs(&names), "This PC  >  Local Disk (C:)");
    }
}
