//! The Formatted data type, from the SDK page's own examples and rules
//! (learn.microsoft.com/windows/win32/msi/formatted).

use super::*;
use alloc::collections::BTreeMap;
use std::string::ToString;

struct R {
    props: BTreeMap<String, String>,
}

impl Resolver for R {
    fn property(&self, name: &str) -> Option<String> {
        self.props.get(name).cloned()
    }
    fn environment(&self, name: &str) -> Option<String> {
        (name.eq_ignore_ascii_case("TEMP")).then(|| "C:\\Temp".to_string())
    }
    fn file_path(&self, key: &str) -> Option<String> {
        (key == "tool.exe").then(|| "C:\\Program Files\\Gala\\Long Tool Name.exe".to_string())
    }
    fn file_short_path(&self, key: &str) -> Option<String> {
        (key == "tool.exe").then(|| "C:\\PROGRA~1\\Gala\\LONGTO~1.EXE".to_string())
    }
    fn component_directory(&self, key: &str) -> Option<String> {
        (key == "Core").then(|| "C:\\Program Files\\Gala\\".to_string())
    }
}

fn r() -> R {
    let mut props = BTreeMap::new();
    for (k, v) in [
        ("ERRORTXT", "Please contact your support personnel."),
        ("PropertyA", "PropertyB"),
        ("PropertyB", "the value of B"),
        ("PropertyC", "not a name!"),
        ("INSTALLDIR", "C:\\Program Files\\Gala\\"),
        ("EMPTY", ""),
    ] {
        props.insert(k.to_string(), v.to_string());
    }
    R { props }
}

#[test]
fn documented_rules() {
    let r = r();
    let cases: &[(&str, &str, &str)] = &[
        // [propertyname] (the page's LaunchCondition example).
        (
            "System does not meet installation requirements. [ERRORTXT]",
            "System does not meet installation requirements. Please contact your support personnel.",
            "[property]",
        ),
        (
            "System does not meet installation requirements. [NOTSET]",
            "System does not meet installation requirements. ",
            "an unset property is blank",
        ),
        ("[not a name]", "", "not a valid property name: blank"),
        // Iterated brackets, inside out.
        ("[[PropertyA]]", "the value of B", "[[PropertyA]]"),
        ("[[NOTSET]]", "", "PropertyA not set: blank"),
        ("[[PropertyC]]", "", "the value is not a valid property name: blank"),
        // Environment variables.
        ("[%TEMP]\\x", "C:\\Temp\\x", "[%environmentvariable]"),
        ("[%NOSUCH]", "", "unset environment variable"),
        // [\x]: the page's example, and only the first character kept.
        ("[\\[]Bracket Text[\\]]", "[Bracket Text]", "[\\x]"),
        ("[\\abc]", "a", "only the first character after the backslash"),
        // [~]: NUL.
        ("a[~]b[~]c", "a\0b\0c", "[~]"),
        // Braces.
        ("{plain text}", "{plain text}", "no property inside: unchanged"),
        ("{[INSTALLDIR]bin}", "C:\\Program Files\\Gala\\bin", "all valid: no braces"),
        ("x{[NOTSET]bin}y", "xy", "one invalid: the group is blank"),
        ("{[EMPTY]z}", "z", "a property set to empty is valid"),
        // Unmatched brackets and braces stay.
        ("a[b", "a[b", "unmatched ["),
        ("a]b", "a]b", "unmatched ]"),
        ("a{b", "a{b", "unmatched {"),
        ("a}b", "a}b", "unmatched }"),
        // Table keys.
        ("[#tool.exe] /setup", "C:\\Program Files\\Gala\\Long Tool Name.exe /setup", "[#filekey]"),
        ("[#nofile]", "", "[#filekey] of no file: blank"),
        ("[$Core]x", "C:\\Program Files\\Gala\\x", "[$componentkey]"),
        ("[$Other]", "", "absent component: blank"),
        (
            "[!tool.exe]",
            "C:\\Program Files\\Gala\\Long Tool Name.exe",
            "[!] outside Registry: as [#]",
        ),
    ];
    for &(text, want, rule) in cases {
        assert_eq!(format(text, &r, Column::Other), want, "{text:?} ({rule})");
    }
    assert_eq!(
        format("[!tool.exe]", &r, Column::RegistryValue),
        "C:\\PROGRA~1\\Gala\\LONGTO~1.EXE",
        "[!filekey] in the Registry table's Value column: the short path"
    );
}

#[test]
fn never_panics() {
    let r = r();
    let alphabet: std::vec::Vec<char> = "[]{}\\~#!$%ABa ".chars().collect();
    let mut x = 0x1357_9bdfu32;
    for len in 0..30 {
        for _ in 0..300 {
            let s: String = (0..len)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    alphabet[x as usize % alphabet.len()]
                })
                .collect();
            let _ = format(&s, &r, Column::RegistryValue);
        }
    }
}
