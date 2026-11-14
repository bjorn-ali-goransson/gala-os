//! XSD's regular expressions (XML Schema Part 2, appendix F): what the
//! `pattern` facet holds. A pattern matches the whole value (no anchors).
//!
//! Supported: branches (`|`), groups, the quantifiers `?`, `*`, `+`,
//! `{n}`, `{n,}`, `{n,m}`, `.`, character classes with ranges and
//! negation, the single-character escapes, the multi-character escapes
//! `\s \S \d \D \w \W \i \I \c \C`, the categories `\p{L}`, `\p{Lu}`,
//! `\p{Ll}`, `\p{N}`, `\p{Nd}` (and `\P`), the blocks `\p{IsPrivateUse}`
//! and `\p{IsBasicLatin}`, and `\uXXXX` (not XSD's syntax, but in the
//! schemas App-V gives MSXML 6, which accepts them). What else a pattern
//! holds (character class subtraction, other categories and blocks) is a
//! compile error that names it, so a schema using it fails to load
//! instead of matching wrongly.

use alloc::string::String;
use alloc::vec::Vec;

/// One item of a character class.
#[derive(Clone, Debug)]
enum ClassItem {
    Range(char, char),
    Multi(Multi),
}

/// The multi-character escapes and categories.
#[derive(Clone, Copy, Debug)]
enum Multi {
    Space,
    Digit,
    Word,
    NameStart,
    NameChar,
    Letter,
    Upper,
    Lower,
    Number,
    PrivateUse,
    BasicLatin,
    Not(&'static Multi),
}

#[derive(Clone, Debug)]
enum Node {
    /// A set of characters: (items, negated).
    Class(Vec<ClassItem>, bool),
    /// `.`: anything but a line end.
    Dot,
    /// A group: its branches.
    Group(Vec<Vec<Piece>>),
}

#[derive(Clone, Debug)]
struct Piece {
    atom: Node,
    min: u32,
    /// u32::MAX: unbounded.
    max: u32,
}

/// A compiled pattern.
#[derive(Clone, Debug)]
pub struct Regex {
    branches: Vec<Vec<Piece>>,
}

static NOT_SPACE: Multi = Multi::Space;
static NOT_DIGIT: Multi = Multi::Digit;
static NOT_WORD: Multi = Multi::Word;
static NOT_NAME_START: Multi = Multi::NameStart;
static NOT_NAME_CHAR: Multi = Multi::NameChar;
static NOT_LETTER: Multi = Multi::Letter;
static NOT_UPPER: Multi = Multi::Upper;
static NOT_LOWER: Multi = Multi::Lower;
static NOT_NUMBER: Multi = Multi::Number;
static NOT_PRIVATE: Multi = Multi::PrivateUse;
static NOT_BASIC_LATIN: Multi = Multi::BasicLatin;

fn negate(m: Multi) -> Multi {
    Multi::Not(match m {
        Multi::Space => &NOT_SPACE,
        Multi::Digit => &NOT_DIGIT,
        Multi::Word => &NOT_WORD,
        Multi::NameStart => &NOT_NAME_START,
        Multi::NameChar => &NOT_NAME_CHAR,
        Multi::Letter => &NOT_LETTER,
        Multi::Upper => &NOT_UPPER,
        Multi::Lower => &NOT_LOWER,
        Multi::Number => &NOT_NUMBER,
        Multi::PrivateUse => &NOT_PRIVATE,
        Multi::BasicLatin => &NOT_BASIC_LATIN,
        Multi::Not(inner) => return *inner,
    })
}

fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == ':'
}

impl Multi {
    fn matches(self, c: char) -> bool {
        match self {
            Multi::Space => matches!(c, ' ' | '\t' | '\n' | '\r'),
            Multi::Digit | Multi::Number => c.is_numeric(),
            // \w: all but punctuation, separators and "other" (XSD F.1.1);
            // approximated as letters, marks and digits.
            Multi::Word => c.is_alphanumeric(),
            Multi::NameStart => is_name_start(c),
            Multi::NameChar => is_name_start(c) || c.is_numeric() || matches!(c, '-' | '.'),
            Multi::Letter => c.is_alphabetic(),
            Multi::Upper => c.is_uppercase(),
            Multi::Lower => c.is_lowercase(),
            Multi::PrivateUse => matches!(c as u32, 0xe000..=0xf8ff | 0xf0000..=0x10ffff),
            Multi::BasicLatin => (c as u32) < 0x80,
            Multi::Not(inner) => !inner.matches(c),
        }
    }
}

fn class_matches(items: &[ClassItem], negated: bool, c: char) -> bool {
    let inside = items.iter().any(|i| match *i {
        ClassItem::Range(a, b) => a <= c && c <= b,
        ClassItem::Multi(m) => m.matches(c),
    });
    inside != negated
}

struct Parser<'a> {
    s: &'a [char],
    pos: usize,
}

type Result<T> = core::result::Result<T, String>;

fn err<T>(what: &str) -> Result<T> {
    Err(what.into())
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek();
        self.pos += 1;
        c
    }

    fn branches(&mut self) -> Result<Vec<Vec<Piece>>> {
        let mut out = Vec::new();
        loop {
            let mut branch = Vec::new();
            while let Some(c) = self.peek() {
                if c == '|' || c == ')' {
                    break;
                }
                branch.push(self.piece()?);
            }
            out.push(branch);
            if self.peek() == Some('|') {
                self.pos += 1;
            } else {
                return Ok(out);
            }
        }
    }

    fn number(&mut self) -> Result<u32> {
        let start = self.pos;
        let mut n: u32 = 0;
        while let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
            n = n.checked_mul(10).and_then(|n| n.checked_add(d)).ok_or("a quantity too large")?;
            self.pos += 1;
        }
        if self.pos == start {
            return err("a quantifier without a number");
        }
        Ok(n)
    }

    fn piece(&mut self) -> Result<Piece> {
        let atom = self.atom()?;
        let (min, max) = match self.peek() {
            Some('?') => (0, 1),
            Some('*') => (0, u32::MAX),
            Some('+') => (1, u32::MAX),
            Some('{') => {
                self.pos += 1;
                let min = self.number()?;
                let max = if self.peek() == Some(',') {
                    self.pos += 1;
                    if self.peek() == Some('}') { u32::MAX } else { self.number()? }
                } else {
                    min
                };
                if self.peek() != Some('}') || max < min {
                    return err("a malformed quantifier");
                }
                (min, max)
            }
            _ => return Ok(Piece { atom, min: 1, max: 1 }),
        };
        self.pos += 1;
        Ok(Piece { atom, min, max })
    }

    fn atom(&mut self) -> Result<Node> {
        match self.next() {
            Some('(') => {
                let b = self.branches()?;
                if self.next() != Some(')') {
                    return err("an unclosed group");
                }
                Ok(Node::Group(b))
            }
            Some('.') => Ok(Node::Dot),
            Some('[') => self.class(),
            Some('\\') => Ok(match self.escape()? {
                ClassItem::Range(a, b) => Node::Class(alloc::vec![ClassItem::Range(a, b)], false),
                m => Node::Class(alloc::vec![m], false),
            }),
            Some(c @ ('?' | '*' | '+' | '{' | '}' | ')' | ']')) => {
                let mut s = String::from("a stray '");
                s.push(c);
                s.push('\'');
                Err(s)
            }
            Some(c) => Ok(Node::Class(alloc::vec![ClassItem::Range(c, c)], false)),
            None => err("an unexpected end"),
        }
    }

    /// After a backslash: one character (a range of one) or a multi.
    fn escape(&mut self) -> Result<ClassItem> {
        let single = |c| Ok(ClassItem::Range(c, c));
        let multi = |m| Ok(ClassItem::Multi(m));
        match self.next() {
            Some('n') => single('\n'),
            Some('r') => single('\r'),
            Some('t') => single('\t'),
            Some(
                c @ ('\\' | '|' | '.' | '-' | '^' | '?' | '*' | '+' | '{' | '}' | '(' | ')' | '['
                | ']'),
            ) => single(c),
            Some('s') => multi(Multi::Space),
            Some('S') => multi(negate(Multi::Space)),
            Some('d') => multi(Multi::Digit),
            Some('D') => multi(negate(Multi::Digit)),
            Some('w') => multi(Multi::Word),
            Some('W') => multi(negate(Multi::Word)),
            Some('i') => multi(Multi::NameStart),
            Some('I') => multi(negate(Multi::NameStart)),
            Some('c') => multi(Multi::NameChar),
            Some('C') => multi(negate(Multi::NameChar)),
            Some('u') => {
                let mut v = 0u32;
                for _ in 0..4 {
                    let d = self.next().and_then(|c| c.to_digit(16)).ok_or("a malformed \\u")?;
                    v = v * 16 + d;
                }
                single(char::from_u32(v).unwrap_or('\u{fffd}'))
            }
            Some(p @ ('p' | 'P')) => {
                if self.next() != Some('{') {
                    return err("a malformed \\p");
                }
                let mut name = String::new();
                loop {
                    match self.next() {
                        Some('}') => break,
                        Some(c) => name.push(c),
                        None => return err("a malformed \\p"),
                    }
                }
                let m = match name.as_str() {
                    "L" => Multi::Letter,
                    "Lu" => Multi::Upper,
                    "Ll" => Multi::Lower,
                    "N" => Multi::Number,
                    "Nd" => Multi::Digit,
                    "IsPrivateUse" => Multi::PrivateUse,
                    "IsBasicLatin" => Multi::BasicLatin,
                    _ => {
                        let mut s = String::from("the category or block \\p{");
                        s.push_str(&name);
                        s.push_str("} (not supported)");
                        return Err(s);
                    }
                };
                multi(if p == 'P' { negate(m) } else { m })
            }
            Some(c) => {
                let mut s = String::from("the escape \\");
                s.push(c);
                s.push_str(" (not supported)");
                Err(s)
            }
            None => err("an unexpected end"),
        }
    }

    /// After `[`: a class to its `]`.
    fn class(&mut self) -> Result<Node> {
        let negated = self.peek() == Some('^');
        if negated {
            self.pos += 1;
        }
        let mut items = Vec::new();
        let mut first = true;
        loop {
            let c = self.next().ok_or("an unclosed character class")?;
            match c {
                ']' if !first => return Ok(Node::Class(items, negated)),
                '[' => return err("a nested character class (not supported)"),
                '-' if !first && self.peek() == Some('[') => {
                    return err("character class subtraction (not supported)");
                }
                _ => {}
            }
            first = false;
            let from = if c == '\\' {
                match self.escape()? {
                    ClassItem::Range(a, _) => a,
                    m => {
                        items.push(m);
                        continue;
                    }
                }
            } else {
                c
            };
            // A range `a-b` (a '-' before ']' is itself).
            if self.peek() == Some('-')
                && self.s.get(self.pos + 1).is_some_and(|&n| n != ']' && n != '[')
            {
                self.pos += 1;
                let to = match self.next() {
                    Some('\\') => match self.escape()? {
                        ClassItem::Range(a, _) => a,
                        _ => return err("a range ending in a multi-character escape"),
                    },
                    Some(t) => t,
                    None => return err("an unclosed character class"),
                };
                if to < from {
                    return err("a range whose end is before its start");
                }
                items.push(ClassItem::Range(from, to));
            } else {
                items.push(ClassItem::Range(from, from));
            }
        }
    }
}

impl Regex {
    /// Compiles `pattern`; the error says what was not understood.
    pub fn compile(pattern: &str) -> Result<Regex> {
        let chars: Vec<char> = pattern.chars().collect();
        let mut p = Parser { s: &chars, pos: 0 };
        let branches = p.branches()?;
        if p.pos != chars.len() {
            return err("an unmatched ')'");
        }
        Ok(Regex { branches })
    }

    /// Whether the whole of `s` matches.
    pub fn is_match(&self, s: &str) -> bool {
        let chars: Vec<char> = s.chars().collect();
        let mut steps = 0u32;
        alt(&self.branches, &chars, 0, &mut steps, &mut |end, _| end == chars.len())
    }
}

/// The work bound of one match (backtracking can be exponential; the
/// patterns of schemas are small and their values short).
const MAX_STEPS: u32 = 1 << 20;

type Cont<'c> = dyn FnMut(usize, &mut u32) -> bool + 'c;

/// Whether one of `branches` matches at `pos` and `k` accepts its end.
fn alt(branches: &[Vec<Piece>], s: &[char], pos: usize, steps: &mut u32, k: &mut Cont) -> bool {
    branches.iter().any(|b| seq(b, s, pos, steps, k))
}

fn seq(pieces: &[Piece], s: &[char], pos: usize, steps: &mut u32, k: &mut Cont) -> bool {
    match pieces.split_first() {
        None => k(pos, steps),
        Some((p, rest)) => {
            repeat(p, 0, s, pos, steps, &mut |e: usize, st: &mut u32| seq(rest, s, e, st, k))
        }
    }
}

/// `p` repeated (`done` times so far) from `pos`, greedy.
fn repeat(p: &Piece, done: u32, s: &[char], pos: usize, steps: &mut u32, k: &mut Cont) -> bool {
    *steps += 1;
    if *steps > MAX_STEPS {
        return false;
    }
    if done < p.max {
        let more = atom(&p.atom, s, pos, steps, &mut |e: usize, st: &mut u32| {
            // An empty iteration past the minimum adds nothing.
            (e != pos || done < p.min) && repeat(p, done + 1, s, e, st, k)
        });
        if more {
            return true;
        }
    }
    done >= p.min && k(pos, steps)
}

fn atom(a: &Node, s: &[char], pos: usize, steps: &mut u32, k: &mut Cont) -> bool {
    match a {
        Node::Class(items, negated) => {
            s.get(pos).is_some_and(|&c| class_matches(items, *negated, c)) && k(pos + 1, steps)
        }
        Node::Dot => s.get(pos).is_some_and(|&c| c != '\n' && c != '\r') && k(pos + 1, steps),
        Node::Group(b) => alt(b, s, pos, steps, k),
    }
}
