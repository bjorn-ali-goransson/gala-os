//! The shell's keys (ROADMAP SH step 1, parts 7 and 11; ARCHITECTURE
//! "The user-mode shell", "The shell's window and the shell's keys"):
//! the keys Windows' shell answers and shasha takes for its desktop
//! window with RegisterHotKey, which only the shell's process may do.
//! What each does is the program's (`main.rs`); which keys they are, the
//! hot key ids and what a WM_HOTKEY's id means are here, without system
//! calls (host tests below).
//!
//! The system keeps the others: Win+arrows (Snap) and Ctrl+Shift+Esc
//! (Task Manager) work whether a shell runs or not, so no program
//! registers them (`kernel/src/nt/window.rs` `system_key`).

use alloc::format;
use alloc::string::String;

/// RegisterHotKey's MOD_WIN.
pub const MOD_WIN: u32 = 0x0008;
const VK_LWIN: u32 = 0x5b;

/// A key the shell holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellKey {
    /// The Windows key alone ((MOD_WIN, VK_LWIN): gala's hot key for its
    /// release with no other key): the start menu, shown or hidden.
    Start,
    /// Win+E: a File Explorer window.
    Explorer,
}

/// Every key the shell takes, in the order it registers them.
pub const SHELL_KEYS: [ShellKey; 2] = [ShellKey::Start, ShellKey::Explorer];

impl ShellKey {
    /// Its hot key's id (RegisterHotKey's, WM_HOTKEY's wParam).
    pub fn id(self) -> i32 {
        match self {
            ShellKey::Start => 1,
            ShellKey::Explorer => 2,
        }
    }

    /// The key whose WM_HOTKEY carries `id`, if it is one of the shell's.
    pub fn from_id(id: usize) -> Option<ShellKey> {
        SHELL_KEYS.into_iter().find(|k| k.id() as usize == id)
    }

    /// RegisterHotKey's modifiers and virtual key.
    pub fn keys(self) -> (u32, u32) {
        match self {
            ShellKey::Start => (MOD_WIN, VK_LWIN),
            ShellKey::Explorer => (MOD_WIN, u32::from(b'E')),
        }
    }

    /// As the log names it (the kernel's `Combo` spelling).
    pub fn name(self) -> &'static str {
        match self {
            ShellKey::Start => "Win",
            ShellKey::Explorer => "Win+E",
        }
    }

    /// What it does, for the log.
    pub fn does(self) -> &'static str {
        match self {
            ShellKey::Start => "the start menu",
            ShellKey::Explorer => "File Explorer",
        }
    }
}

/// File Explorer's program in the Windows directory `windows` (what
/// GetWindowsDirectoryW gives, with or without a closing backslash), where
/// Windows has explorer.exe.
pub fn explorer_path(windows: &str) -> String {
    format!("{}\\explorer.exe", windows.trim_end_matches('\\'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_key_has_its_own_id_and_comes_back_from_it() {
        for k in SHELL_KEYS {
            assert_eq!(ShellKey::from_id(k.id() as usize), Some(k));
        }
        assert_ne!(ShellKey::Start.id(), ShellKey::Explorer.id());
        assert_eq!(ShellKey::from_id(0), None);
        assert_eq!(ShellKey::from_id(99), None);
    }

    #[test]
    fn the_keys_are_the_windows_key_and_win_e() {
        assert_eq!(ShellKey::Start.keys(), (MOD_WIN, 0x5b));
        assert_eq!(ShellKey::Explorer.keys(), (MOD_WIN, 0x45));
        assert_eq!(ShellKey::Explorer.name(), "Win+E");
        assert_eq!(ShellKey::Start.does(), "the start menu");
    }

    #[test]
    fn file_explorer_is_in_the_windows_directory() {
        assert_eq!(explorer_path("C:\\Windows"), "C:\\Windows\\explorer.exe");
        assert_eq!(explorer_path("C:\\Windows\\"), "C:\\Windows\\explorer.exe");
    }
}
