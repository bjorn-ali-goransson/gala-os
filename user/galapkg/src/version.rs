//! Version ordering as winget documents and applies it to PackageVersion
//! (winget-cli's documentation of `winget upgrade` and the manifest
//! schema): a version is parts separated by `.`; each part is a leading
//! number and the text after it; parts compare by number, then by text
//! (any case), a part without text after its number being the later one
//! (`1.0` after `1.0-beta`); missing parts count as `0`, so `1.2` equals
//! `1.2.0`. A leading `v` is ignored. `Unknown` (winget's name for a
//! version it could not read) is earlier than every other version.

use core::cmp::Ordering;

/// One part: its number (None when it has none, as in `beta`) and the
/// text after the number.
fn part(p: &str) -> (Option<u64>, &str) {
    let digits = p.bytes().take_while(u8::is_ascii_digit).count();
    let number = if digits == 0 { None } else { p[..digits].parse().ok().or(Some(u64::MAX)) };
    (number, &p[digits..])
}

fn compare_parts(a: &str, b: &str) -> Ordering {
    let (an, at) = part(a);
    let (bn, bt) = part(b);
    let by_number = an.unwrap_or(0).cmp(&bn.unwrap_or(0));
    if by_number != Ordering::Equal {
        return by_number;
    }
    match (at.is_empty(), bt.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => {
            let (x, y) = (
                at.bytes().map(|c| c.to_ascii_lowercase()),
                bt.bytes().map(|c| c.to_ascii_lowercase()),
            );
            x.cmp(y)
        }
    }
}

fn normalize(v: &str) -> &str {
    let v = v.trim();
    match v.as_bytes().first() {
        Some(b'v' | b'V') if v.as_bytes().get(1).is_some_and(u8::is_ascii_digit) => &v[1..],
        _ => v,
    }
}

/// winget's ordering of two versions.
pub fn compare(a: &str, b: &str) -> Ordering {
    let (a, b) = (normalize(a), normalize(b));
    match (a.eq_ignore_ascii_case("unknown"), b.eq_ignore_ascii_case("unknown")) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        _ => {}
    }
    let mut x = a.split('.');
    let mut y = b.split('.');
    loop {
        match (x.next(), y.next()) {
            (None, None) => return Ordering::Equal,
            (p, q) => {
                let o = compare_parts(p.unwrap_or("0"), q.unwrap_or("0"));
                if o != Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

/// The latest of `versions` (None if empty).
pub fn latest<'a>(versions: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    versions.into_iter().max_by(|a, b| compare(a, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use Ordering::*;

    #[test]
    fn ordering() {
        assert_eq!(compare("1.2", "1.10"), Less);
        assert_eq!(compare("1.2", "1.2.0"), Equal);
        assert_eq!(compare("1.2.0.0", "1.2"), Equal);
        assert_eq!(compare("1.0", "1.0-beta"), Greater);
        assert_eq!(compare("1.0-alpha", "1.0-beta"), Less);
        assert_eq!(compare("v2.0", "1.9"), Greater);
        assert_eq!(compare("15.2.0", "15.10.0"), Less);
        assert_eq!(compare("Unknown", "0.0.1"), Less);
        assert_eq!(compare("1.0.0", "1.0.0.1"), Less);
        assert_eq!(compare("2024.01.05", "2024.1.5"), Equal);
        assert_eq!(compare("1.0RC", "1.0rc"), Equal);
        assert_eq!(compare("99999999999999999999999", "1"), Greater);
    }

    #[test]
    fn latest_version() {
        assert_eq!(latest(["1.0.0", "1.10.0", "1.9.9"]), Some("1.10.0"));
        assert_eq!(latest([]), None);
    }
}
