//! `cargo xtask stub-audit`: the 32-bit system call stubs agree with
//! their callers and with wow64.dll's argument table.
//!
//! The 32-bit ntdll's stubs (user/testntdll, `syscall_stubs!`) are
//! stdcall: each pops the arguments its own declaration names (`ret N`,
//! 4 bytes each, 8 for a u64). A DLL that declares the same function with
//! a different size (a 4-byte handle where the stub has a u64) has its
//! stack pointer moved on every call, which corrupts the caller's locals
//! far from the cause. And wow64.dll converts each call from its table
//! (user/wow64/src/generic.rs), one entry per 4-byte word: an entry list
//! of a different length reads the wrong words. Both are checked here
//! from the sources, on every `cargo xtask test`.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

type Result = std::result::Result<(), String>;

/// The x86 stack size of an argument of type `ty` (bytes).
fn size(ty: &str) -> u32 {
    match ty.trim() {
        "u64" | "i64" | "f64" => 8,
        _ => 4,
    }
}

/// `src` without `//` comments.
fn strip_comments(src: &str) -> String {
    src.lines().map(|l| l.split("//").next().unwrap_or("")).collect::<Vec<_>>().join("\n")
}

/// The types of a parameter list (`a: u32, b: *mut u8`), split at its
/// top-level commas.
fn param_types(list: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0i32);
    for c in list.chars() {
        match c {
            '(' | '[' | '<' => depth += 1,
            ')' | ']' | '>' => depth -= 1,
            _ => {}
        }
        if c == ',' && depth == 0 {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out.iter()
        .filter_map(|p| p.split_once(':').map(|(_, t)| t.trim().to_string()))
        .filter(|t| !t.is_empty())
        .collect()
}

/// `text` (just after an opening parenthesis) up to the matching
/// closing one.
fn balanced(text: &str) -> Option<&str> {
    let mut depth = 1;
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[..i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// The stubs: name -> (number, argument types), from `Name = 0xN, fn(..);`.
fn stubs(src: &str) -> BTreeMap<String, (u32, Vec<String>)> {
    let src = strip_comments(src);
    let mut out = BTreeMap::new();
    for chunk in src.split(';') {
        let chunk = chunk.trim();
        let Some((head, rest)) = chunk.split_once(", fn(") else { continue };
        let Some((name, number)) = head.rsplit_once('=') else { continue };
        let name = name.split_whitespace().last().unwrap_or("");
        let Some(hex) = number.trim().strip_prefix("0x") else { continue };
        let (Ok(n), Some(params)) = (u32::from_str_radix(hex, 16), balanced(rest)) else {
            continue;
        };
        if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            out.insert(name.to_string(), (n, param_types(params)));
        }
    }
    out
}

/// wow64.dll's table: number -> entry count, from `(0xN, &[K, ...]),`.
fn table(src: &str) -> BTreeMap<u32, usize> {
    let src = strip_comments(src);
    let mut out = BTreeMap::new();
    for (at, _) in src.match_indices("(0x") {
        let rest = &src[at + 3..];
        let Some((hex, after)) = rest.split_once(',') else { continue };
        let Some(list) = after.trim_start().strip_prefix("&[") else { continue };
        let (Ok(n), Some((entries, _))) = (u32::from_str_radix(hex, 16), list.split_once(']'))
        else {
            continue;
        };
        out.insert(n, entries.split(',').filter(|e| !e.trim().is_empty()).count());
    }
    out
}

/// Every `fn Name(` declaration of a stub's name in `src`: its types.
fn declarations(
    src: &str,
    names: &BTreeMap<String, (u32, Vec<String>)>,
) -> Vec<(String, Vec<String>)> {
    let src = strip_comments(src);
    let mut out = Vec::new();
    for (at, _) in src.match_indices("fn ") {
        let rest = &src[at + 3..];
        let Some((name, after)) = rest.split_once('(') else { continue };
        if !names.contains_key(name) {
            continue;
        }
        if let Some(params) = balanced(after) {
            out.push((name.to_string(), param_types(params)));
        }
    }
    out
}

fn sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

pub fn stub_audit(root: &Path) -> Result {
    let read =
        |p: &Path| fs::read_to_string(p).map_err(|e| format!("reading {}: {e}", p.display()));
    let stubs = stubs(&read(&root.join("user/testntdll/src/lib.rs"))?);
    let table = table(&read(&root.join("user/wow64/src/generic.rs"))?);
    let words = |types: &[String]| types.iter().map(|t| size(t)).sum::<u32>() / 4;
    let mut problems = Vec::new();
    for (name, (n, types)) in &stubs {
        if let Some(&count) = table.get(n)
            && count as u32 != words(types)
        {
            problems.push(format!(
                "wow64's table has {count} words for {name} (0x{n:x}); its stub pops {}",
                words(types)
            ));
        }
    }
    let mut files = Vec::new();
    let user = root.join("user");
    for e in fs::read_dir(&user).map_err(|e| format!("reading {}: {e}", user.display()))?.flatten()
    {
        if e.file_name() != "testntdll" {
            sources(&e.path().join("src"), &mut files);
        }
    }
    for f in &files {
        for (name, types) in declarations(&read(f)?, &stubs) {
            let theirs = &stubs[&name].1;
            let sizes = |t: &[String]| t.iter().map(|t| size(t)).collect::<Vec<_>>();
            if sizes(&types) != sizes(theirs) {
                problems.push(format!(
                    "{}: {name}({}) differs on x86 from the stub's ({})",
                    f.strip_prefix(root).unwrap_or(f).display(),
                    types.join(", "),
                    theirs.join(", ")
                ));
            }
        }
    }
    eprintln!(
        "xtask: stub-audit: {} 32-bit stubs, {} wow64 table entries, {} source files",
        stubs.len(),
        table.len(),
        files.len()
    );
    if problems.is_empty() {
        return Ok(());
    }
    for p in &problems {
        eprintln!("stub-audit: {p}");
    }
    Err(format!("stub-audit: {} mismatch(es) (see above)", problems.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STUBS: &str = "syscall_stubs! {\n    NtA = 0x0001, fn(h: usize, v: u64); // c\n    NtB = 0x00ff, fn(cb: Option<extern \"system\" fn(u32, u32)>, x: *mut u8);\n}";

    #[test]
    fn parses_stubs_and_table() {
        let s = stubs(STUBS);
        assert_eq!(s["NtA"], (1, vec!["usize".to_string(), "u64".to_string()]));
        assert_eq!(s["NtB"].1.len(), 2);
        let t = table(
            "pub static TABLE = &[\n    (0x0001, &[H, U, Hi]), // NtA\n    (0x00ff, &[U]),\n];",
        );
        assert_eq!(t[&1], 3);
        assert_eq!(t[&0xff], 1);
    }

    #[test]
    fn finds_a_caller_with_other_sizes() {
        let s = stubs(STUBS);
        let d = declarations(
            "unsafe extern \"system\" {\n    fn NtA(h: Handle, v: u32) -> u32;\n}",
            &s,
        );
        assert_eq!(d, vec![("NtA".to_string(), vec!["Handle".to_string(), "u32".to_string()])]);
    }
}
