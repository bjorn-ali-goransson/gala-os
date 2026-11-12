//! The command line as wsl.exe reads it (WSL's `WslMain`): the program's
//! name skipped (a quoted part may hold spaces), then one word at a time
//! up to a space or tab (`ParseArgument`), the rest kept as written so
//! that `wsl -- <command line>` hands it to the shell unchanged. A
//! subcommand's own arguments (`--import <name> <dir> <file>`) and an
//! `--exec` command split as CommandLineToArgvW splits (its documented
//! rules: 2n backslashes before a quote are n and the quote delimits,
//! 2n+1 are n and a literal quote; `""` inside quotes is a quote).

use alloc::vec::Vec;

fn is_space(c: u16) -> bool {
    c == u16::from(b' ') || c == u16::from(b'\t')
}

/// `s` without its leading spaces and tabs.
pub fn trim_start(s: &[u16]) -> &[u16] {
    let n = s.iter().take_while(|&&c| is_space(c)).count();
    &s[n..]
}

/// The command line after the program's name.
pub fn skip_program(cmdline: &[u16]) -> &[u16] {
    let mut quoted = false;
    let mut i = 0;
    while i < cmdline.len() && (quoted || !is_space(cmdline[i])) {
        if cmdline[i] == u16::from(b'"') {
            quoted = !quoted;
        }
        i += 1;
    }
    trim_start(&cmdline[i..])
}

/// The next word (up to a space or tab) and the rest after it, its
/// leading spaces gone.
pub fn next(s: &[u16]) -> (&[u16], &[u16]) {
    let end = s.iter().position(|&c| is_space(c)).unwrap_or(s.len());
    (&s[..end], trim_start(&s[end..]))
}

/// The arguments of `s` as CommandLineToArgvW splits them (without its
/// special treatment of the first argument).
pub fn split(s: &[u16]) -> Vec<Vec<u16>> {
    let quote = u16::from(b'"');
    let backslash = u16::from(b'\\');
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        while i < s.len() && is_space(s[i]) {
            i += 1;
        }
        if i >= s.len() {
            return out;
        }
        let mut arg = Vec::new();
        let mut quoted = false;
        while i < s.len() && (quoted || !is_space(s[i])) {
            if s[i] == backslash {
                let n = s[i..].iter().take_while(|&&c| c == backslash).count();
                if s.get(i + n) == Some(&quote) {
                    arg.extend(core::iter::repeat_n(backslash, n / 2));
                    i += n;
                    if n % 2 == 1 {
                        arg.push(quote);
                        i += 1;
                    }
                } else {
                    arg.extend(core::iter::repeat_n(backslash, n));
                    i += n;
                }
            } else if s[i] == quote {
                if quoted && s.get(i + 1) == Some(&quote) {
                    arg.push(quote);
                    i += 2;
                } else {
                    quoted = !quoted;
                    i += 1;
                }
            } else {
                arg.push(s[i]);
                i += 1;
            }
        }
        out.push(arg);
    }
}

/// Whether word `w` is `text`.
pub fn is(w: &[u16], text: &str) -> bool {
    w.len() == text.len() && w.iter().zip(text.bytes()).all(|(&a, b)| a == u16::from(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::String;
    use std::vec::Vec;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn s(v: &[u16]) -> String {
        String::from_utf16(v).unwrap()
    }

    #[test]
    fn words_and_the_rest_as_written() {
        let line = w("\"C:\\Program Files\\wsl.exe\"  -d bb -- echo  \"a b\"");
        let rest = skip_program(&line);
        let (a, rest) = next(rest);
        assert_eq!(s(a), "-d");
        let (b, rest) = next(rest);
        assert_eq!(s(b), "bb");
        let (c, rest) = next(rest);
        assert!(is(c, "--"));
        assert_eq!(s(rest), "echo  \"a b\"");
        assert_eq!(next(&[]).0, &[] as &[u16]);
    }

    #[test]
    fn split_follows_commandlinetoargvw() {
        let args: Vec<String> =
            split(&w(r#"a "b c" d\"e f\\"g h" "x""y" \\\\"#)).iter().map(|a| s(a)).collect();
        assert_eq!(args, ["a", "b c", "d\"e", "f\\g h", "x\"y", "\\\\\\\\"]);
        assert!(split(&w("   ")).is_empty());
    }
}
