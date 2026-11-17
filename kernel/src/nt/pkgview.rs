//! A process's view of a Click-to-Run package's VFS tree (ROADMAP OF,
//! ARCHITECTURE "A package's VFS view").
//!
//! An Office Click-to-Run install keeps the files its programs are to
//! find elsewhere on the machine under `<root>\root\VFS\<token>`, the
//! token naming the place (App-V's tokens: `ProgramFilesCommonX64` is
//! `C:\Program Files\Common Files`, and so on -- `TOKENS` below, which
//! is what an install's `root\VFS` holds). The package manifest writes
//! every such path with its token, and `HKLM\SOFTWARE\Microsoft\AppVISV`
//! holds one value per package, its name the package's root and its data
//! the package's id.
//!
//! Measured on Windows 11 with Office 16 installed: `C:\Program
//! Files\Common Files\Microsoft Shared\Office16` is on no listing of
//! `Common Files` and no process finds it, while a running WINWORD.EXE
//! reports `mso.dll` and five more of Office's shared libraries loaded
//! from exactly that directory -- and the files are in the install's
//! `root\VFS\ProgramFilesCommonX64\Microsoft Shared\Office16`. None of
//! the App-V services or drivers runs, and those modules sit at
//! addresses outside the loader's own range: on Windows the view is
//! Office's loader naming what it mapped itself, in the one process.
//!
//! gala gives the process the same files at the same paths from the
//! kernel instead, which is what a program can tell apart: an open that
//! the machine itself has nothing for is tried again in the package's
//! tree, for a process whose image is under a registered package's
//! `root`. So a file the machine has keeps its own contents -- Windows
//! never shows which way round that is, since Office asks for the files
//! it ships by their real `root\Office16` path -- and a directory the
//! machine has is its own listing, while one only the package has is
//! listed from the package.

use alloc::vec::Vec;

use super::file::{self, FilePath};
use super::registry;
use crate::fs::ramfs;
use crate::mm::heap::TryVec;

const BACKSLASH: u16 = b'\\' as u16;

/// Where each `VFS\<token>` tree answers, as a path relative to the
/// volume (App-V's tokens; an Office 16 install has exactly these).
const TOKENS: &[(&str, &str)] = &[
    ("ProgramFilesCommonX64", "\\Program Files\\Common Files"),
    ("ProgramFilesCommonX86", "\\Program Files (x86)\\Common Files"),
    ("ProgramFilesX64", "\\Program Files"),
    ("ProgramFilesX86", "\\Program Files (x86)"),
    ("Fonts", "\\Windows\\Fonts"),
    ("System", "\\Windows\\System32"),
    ("SystemX86", "\\Windows\\SysWOW64"),
    ("Windows", "\\Windows"),
    ("Common AppData", "\\ProgramData"),
];

/// The directory of a package root that holds both the programs and the
/// `VFS` tree (Click-to-Run's layout: `<root>\root\Office16\WINWORD.EXE`
/// beside `<root>\root\VFS\<token>`).
const ROOT_DIRECTORY: &str = "root";

/// Whether `image` is a program of the package at `root` (a path under
/// `<root>\root\`), both Win32 paths, case-insensitively.
fn in_package(root: &[u8], image: &str) -> bool {
    let image = image.as_bytes();
    let mut n = root.len();
    while n > 0 && root[n - 1] == b'\\' {
        n -= 1;
    }
    let root = &root[..n];
    if image.len() <= root.len() || !starts_with_ascii(image, root) {
        return false;
    }
    let rest = &image[root.len()..];
    let mut below = Vec::new();
    if below.try_push(b'\\').is_err() {
        return false;
    }
    if below.try_extend_from_slice(ROOT_DIRECTORY.as_bytes()).is_err() {
        return false;
    }
    if below.try_push(b'\\').is_err() {
        return false;
    }
    starts_with_ascii(rest, &below)
}

fn starts_with_ascii(text: &[u8], prefix: &[u8]) -> bool {
    text.len() >= prefix.len() && text.iter().zip(prefix).all(|(a, b)| a.eq_ignore_ascii_case(b))
}

/// The package root whose programs include `image`, if one is
/// registered: the process's view for its life (`package_of` runs once,
/// when the process is made). The roots are the value names under
/// `HKLM\SOFTWARE\Microsoft\AppVISV`, where Windows keeps them and
/// Office's own `AppVIsvSubsystems64.dll` reads them; a name that is not
/// ASCII is passed over (every package root is a file system path).
pub fn package_of(image: &'static str) -> Option<&'static str> {
    let mut key = Vec::new();
    for c in "\\Registry\\Machine\\SOFTWARE\\Microsoft\\AppVISV".encode_utf16() {
        key.try_push(c).ok()?;
    }
    let found = registry::with(|r| {
        let relative = registry::Registry::absolute(&key)?;
        let Ok(id) = r.lookup(registry::ROOT, relative) else { return Ok(None) };
        let mut best: Option<Vec<u8>> = None;
        for index in 0.. {
            let Ok(value) = r.value_at(id, index) else { break };
            let mut name = Vec::new();
            let mut ascii = true;
            for &c in &value.name {
                match u8::try_from(c) {
                    Ok(b) if c >= 0x20 => {
                        name.try_push(b).map_err(|_| super::status::NtStatus::NO_MEMORY)?
                    }
                    _ => {
                        ascii = false;
                        break;
                    }
                }
            }
            if !ascii || !in_package(&name, image) {
                continue;
            }
            if best.as_ref().is_none_or(|b| b.len() < name.len()) {
                best = Some(name);
            }
        }
        Ok(best)
    })
    .ok()??;
    let text = core::str::from_utf8(&found).ok()?;
    let root = super::process::intern(text).ok()?;
    crate::info!("pkgview: {image} runs in the package at {root}");
    Some(root)
}

/// The path in `root`'s VFS tree that `name` names, if one of the tokens'
/// places holds it: both relative to the volume, `name` as
/// `\Program Files\Common Files\...` and the answer as
/// `\<root>\root\VFS\ProgramFilesCommonX64\...`. The longest token wins
/// (`Common Files` before `Program Files`), and only at a whole
/// component (`\Program Filesx` is not in `\Program Files`).
pub fn vfs_path(root: &str, name: &[u16]) -> Option<Vec<u16>> {
    // The root without its drive letter, as the volume's paths have none.
    let relative = root.get(2..).filter(|_| root.as_bytes().get(1) == Some(&b':')).unwrap_or(root);
    let (token, place) = TOKENS
        .iter()
        .filter(|(_, place)| under(name, place))
        .max_by_key(|(_, place)| place.len())?;
    let mut out = Vec::new();
    for part in [relative, "\\", ROOT_DIRECTORY, "\\VFS\\", token] {
        for c in part.encode_utf16() {
            out.try_push(c).ok()?;
        }
    }
    out.try_extend_from_slice(&name[place.encode_utf16().count()..]).ok()?;
    Some(out)
}

/// Whether `name` is `place` itself or something below it, case
/// insensitively (`place` is ASCII).
fn under(name: &[u16], place: &str) -> bool {
    let mut n = 0;
    for c in place.encode_utf16() {
        match name.get(n) {
            Some(&a) if wide_lower(a) == wide_lower(c) => n += 1,
            _ => return false,
        }
    }
    matches!(name.get(n), None | Some(&BACKSLASH))
}

fn wide_lower(c: u16) -> u16 {
    if (b'A' as u16..=b'Z' as u16).contains(&c) { c + 0x20 } else { c }
}

/// The path an open reaches in a process that has a package's view: the
/// package's copy, when the machine itself has nothing at `path`. None
/// leaves the open as it was asked.
pub fn redirect(root: &str, path: &FilePath, pid: u64) -> Option<FilePath> {
    if path.start != ramfs::ROOT || path.linux.is_some() {
        return None;
    }
    // A name the machine has is the machine's.
    if matches!(file::lookup_path(path), Ok((Some(_), _))) {
        return None;
    }
    let name = vfs_path(root, &path.name)?;
    let served = FilePath { start: ramfs::ROOT, name, linux: None };
    if !matches!(file::lookup_path(&served), Ok((Some(_), _))) {
        return None;
    }
    package_event(pid, &path.name, &served.name);
    Some(served)
}

/// The `file` event of a path served from the package (`events=file`):
/// what was asked and what answered, so a run shows every file a program
/// found only in its package.
fn package_event(pid: u64, asked: &[u16], served: &[u16]) {
    use crate::events::{Category, Str, emit, enabled};
    if !enabled(Category::File) {
        return;
    }
    let mut asked_text = [0u8; 256];
    let mut served_text = [0u8; 256];
    emit(
        Category::File,
        "package",
        format_args!(
            r#""pid":{pid},"asked":{},"served":{}"#,
            Str(narrow(asked, &mut asked_text)),
            Str(narrow(served, &mut served_text))
        ),
    );
}

/// `path` as text, cut to what `text` holds.
fn narrow<'a>(path: &[u16], text: &'a mut [u8]) -> &'a str {
    let mut n = 0;
    for c in char::decode_utf16(path.iter().copied()) {
        let c = c.unwrap_or('?');
        if n + c.len_utf8() > text.len() {
            break;
        }
        n += c.encode_utf8(&mut text[n..]).len();
    }
    core::str::from_utf8(&text[..n]).unwrap_or("?")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn mapped(root: &str, name: &str) -> Option<String> {
        vfs_path(root, &wide(name)).map(|p| String::from_utf16_lossy(&p))
    }

    #[test]
    fn a_token_leads_to_its_tree() {
        assert_eq!(
            mapped(
                "C:\\Program Files\\Microsoft Office",
                "\\Program Files\\Common Files\\Microsoft Shared\\Office16\\mso.dll"
            )
            .as_deref(),
            Some(
                "\\Program Files\\Microsoft Office\\root\\VFS\\ProgramFilesCommonX64\\Microsoft Shared\\Office16\\mso.dll"
            )
        );
        assert_eq!(
            mapped("C:\\pkg", "\\Windows\\System32\\mfc140u.dll").as_deref(),
            Some("\\pkg\\root\\VFS\\System\\mfc140u.dll")
        );
        assert_eq!(
            mapped("C:\\pkg", "\\ProgramData\\Microsoft\\x").as_deref(),
            Some("\\pkg\\root\\VFS\\Common AppData\\Microsoft\\x")
        );
    }

    #[test]
    fn the_longest_token_wins() {
        // Common Files is its own token, not a name below Program Files.
        assert_eq!(
            mapped("C:\\pkg", "\\Program Files\\Common Files\\x").as_deref(),
            Some("\\pkg\\root\\VFS\\ProgramFilesCommonX64\\x")
        );
        assert_eq!(
            mapped("C:\\pkg", "\\Program Files (x86)\\Common Files\\x").as_deref(),
            Some("\\pkg\\root\\VFS\\ProgramFilesCommonX86\\x")
        );
        // Fonts and System32 before Windows itself.
        assert_eq!(
            mapped("C:\\pkg", "\\Windows\\Fonts\\x.ttf").as_deref(),
            Some("\\pkg\\root\\VFS\\Fonts\\x.ttf")
        );
        assert_eq!(
            mapped("C:\\pkg", "\\Windows\\PkgView\\x").as_deref(),
            Some("\\pkg\\root\\VFS\\Windows\\PkgView\\x")
        );
    }

    #[test]
    fn case_does_not_count_and_a_component_is_whole() {
        assert_eq!(
            mapped("C:\\pkg", "\\PROGRAM FILES\\COMMON FILES\\X").as_deref(),
            Some("\\pkg\\root\\VFS\\ProgramFilesCommonX64\\X")
        );
        // Not a token's place: a longer first component.
        assert_eq!(mapped("C:\\pkg", "\\Program Filesx\\y"), None);
        assert_eq!(mapped("C:\\pkg", "\\Users\\gala\\x"), None);
    }

    #[test]
    fn the_token_tree_itself_is_the_place() {
        assert_eq!(
            mapped("C:\\pkg", "\\Program Files\\Common Files").as_deref(),
            Some("\\pkg\\root\\VFS\\ProgramFilesCommonX64")
        );
    }

    #[test]
    fn a_program_of_the_package_is_under_its_root_directory() {
        assert!(in_package(b"C:\\pkg", "C:\\pkg\\root\\app\\x.exe"));
        assert!(in_package(
            b"c:\\program files\\microsoft office",
            "C:\\Program Files\\Microsoft Office\\root\\Office16\\WINWORD.EXE"
        ));
        // A trailing backslash on the registered root is the same root.
        assert!(in_package(b"C:\\pkg\\", "C:\\pkg\\root\\app\\x.exe"));
        // Beside the package, not in it.
        assert!(!in_package(b"C:\\pkg", "C:\\pkg\\x.exe"));
        assert!(!in_package(b"C:\\pkg", "C:\\pkgx\\root\\x.exe"));
        assert!(!in_package(b"C:\\pkg", "C:\\other\\root\\x.exe"));
        assert!(!in_package(b"C:\\pkg", "C:\\pkg"));
    }
}
