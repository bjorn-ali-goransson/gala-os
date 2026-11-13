//! XSD's simple types: the built-in types gala knows (XML Schema Part
//! 2's string and number families, boolean, the binary types, anyURI
//! and the name types), white space handling, and checking a value
//! against a type and the facets of each derivation step.
//!
//! The date and time types, NOTATION, ENTITY and the list types are not
//! here: a schema naming one fails to compile (`Builtin::from_name`).

use alloc::string::String;
use alloc::vec::Vec;

use super::{Schema, Set, Type, TypeRef, collapse};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhiteSpace {
    Preserve,
    Replace,
    Collapse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    AnyType,
    AnySimpleType,
    String,
    NormalizedString,
    Token,
    Language,
    Name,
    NCName,
    NmToken,
    Id,
    IdRef,
    QName,
    AnyUri,
    Boolean,
    Decimal,
    Float,
    Double,
    /// An integer type: its bounds (inclusive).
    Integer(i128, i128),
    Base64Binary,
    HexBinary,
}

const I128_MIN: i128 = i128::MIN;
const I128_MAX: i128 = i128::MAX;

impl Builtin {
    pub fn from_name(name: &str) -> Option<Builtin> {
        use Builtin::*;
        Some(match name {
            "anyType" => AnyType,
            "anySimpleType" => AnySimpleType,
            "string" => String,
            "normalizedString" => NormalizedString,
            "token" => Token,
            "language" => Language,
            "Name" => Name,
            "NCName" => NCName,
            "NMTOKEN" => NmToken,
            "ID" => Id,
            "IDREF" => IdRef,
            "QName" => QName,
            "anyURI" => AnyUri,
            "boolean" => Boolean,
            "decimal" => Decimal,
            "float" => Float,
            "double" => Double,
            "integer" => Integer(I128_MIN, I128_MAX),
            "nonNegativeInteger" => Integer(0, I128_MAX),
            "positiveInteger" => Integer(1, I128_MAX),
            "nonPositiveInteger" => Integer(I128_MIN, 0),
            "negativeInteger" => Integer(I128_MIN, -1),
            "long" => Integer(i64::MIN.into(), i64::MAX.into()),
            "int" => Integer(i32::MIN.into(), i32::MAX.into()),
            "short" => Integer(i16::MIN.into(), i16::MAX.into()),
            "byte" => Integer(i8::MIN.into(), i8::MAX.into()),
            "unsignedLong" => Integer(0, u64::MAX.into()),
            "unsignedInt" => Integer(0, u32::MAX.into()),
            "unsignedShort" => Integer(0, u16::MAX.into()),
            "unsignedByte" => Integer(0, u8::MAX.into()),
            "base64Binary" => Base64Binary,
            "hexBinary" => HexBinary,
            _ => return None,
        })
    }

    fn white(self) -> WhiteSpace {
        match self {
            Builtin::String | Builtin::AnySimpleType | Builtin::AnyType => WhiteSpace::Preserve,
            Builtin::NormalizedString => WhiteSpace::Replace,
            _ => WhiteSpace::Collapse,
        }
    }

    /// Whether `v` (white space already handled) is in the type's
    /// lexical space; the reason when not.
    fn check(self, v: &str) -> Result<(), &'static str> {
        let name_start = |c: char| c.is_alphabetic() || c == '_' || c == ':';
        let name_char = |c: char| name_start(c) || c.is_numeric() || c == '-' || c == '.';
        let is_name =
            |s: &str| s.chars().next().is_some_and(name_start) && s.chars().all(name_char);
        let ok = match self {
            Builtin::AnyType
            | Builtin::AnySimpleType
            | Builtin::String
            | Builtin::NormalizedString
            | Builtin::Token
            | Builtin::AnyUri => true,
            Builtin::Language => {
                let mut parts = v.split('-');
                parts.next().is_some_and(|p| {
                    (1..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphabetic())
                }) && parts.all(|p| {
                    (1..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_alphanumeric())
                })
            }
            Builtin::Name => is_name(v),
            Builtin::NCName | Builtin::Id | Builtin::IdRef => is_name(v) && !v.contains(':'),
            Builtin::QName => {
                is_name(v) && v.split(':').count() <= 2 && !v.starts_with(':') && !v.ends_with(':')
            }
            Builtin::NmToken => !v.is_empty() && v.chars().all(name_char),
            Builtin::Boolean => matches!(v, "true" | "false" | "1" | "0"),
            Builtin::Decimal => is_decimal(v),
            Builtin::Float | Builtin::Double => matches!(v, "INF" | "-INF" | "NaN") || is_float(v),
            Builtin::Integer(lo, hi) => match parse_integer(v) {
                Some(n) => lo <= n && n <= hi,
                None => false,
            },
            Builtin::Base64Binary => base64_len(v).is_some(),
            Builtin::HexBinary => {
                v.len().is_multiple_of(2) && v.chars().all(|c| c.is_ascii_hexdigit())
            }
        };
        if ok { Ok(()) } else { Err("is not a valid value of its type") }
    }

    /// A value's length for the length facets: octets of the binary
    /// types, characters of the others.
    fn length(self, v: &str) -> usize {
        match self {
            Builtin::HexBinary => v.len() / 2,
            Builtin::Base64Binary => base64_len(v).unwrap_or(0),
            _ => v.chars().count(),
        }
    }
}

fn is_decimal(v: &str) -> bool {
    let v = v.strip_prefix(['+', '-']).unwrap_or(v);
    let (a, b) = v.split_once('.').unwrap_or((v, ""));
    (!a.is_empty() || !b.is_empty())
        && a.chars().all(|c| c.is_ascii_digit())
        && b.chars().all(|c| c.is_ascii_digit())
}

fn is_float(v: &str) -> bool {
    let (m, e) = match v.find(['e', 'E']) {
        Some(i) => (&v[..i], Some(&v[i + 1..])),
        None => (v, None),
    };
    is_decimal(m)
        && e.is_none_or(|e| {
            let e = e.strip_prefix(['+', '-']).unwrap_or(e);
            !e.is_empty() && e.chars().all(|c| c.is_ascii_digit())
        })
}

pub fn parse_integer(v: &str) -> Option<i128> {
    let digits = v.strip_prefix('+').unwrap_or(v);
    if digits.is_empty() || digits == "-" {
        return None;
    }
    let body = digits.strip_prefix('-').unwrap_or(digits);
    if !body.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse::<i128>().ok()
}

/// The octets of base64 text (white space allowed), or None when it is
/// not base64.
fn base64_len(v: &str) -> Option<usize> {
    let chars: Vec<char> = v.chars().filter(|c| !matches!(c, ' ' | '\t' | '\n' | '\r')).collect();
    if !chars.len().is_multiple_of(4) {
        return None;
    }
    let pad = chars.iter().rev().take_while(|&&c| c == '=').count();
    if pad > 2 {
        return None;
    }
    let body = &chars[..chars.len() - pad];
    if !body.iter().all(|c| c.is_ascii_alphanumeric() || *c == '+' || *c == '/') {
        return None;
    }
    Some(chars.len() / 4 * 3 - pad)
}

fn apply_white(v: &str, w: WhiteSpace) -> String {
    match w {
        WhiteSpace::Preserve => v.into(),
        WhiteSpace::Replace => {
            v.chars().map(|c| if matches!(c, '\t' | '\n' | '\r') { ' ' } else { c }).collect()
        }
        WhiteSpace::Collapse => collapse(v),
    }
}

/// Checks `value` against the simple type `ty` of schema `s`. The error
/// is a reason (gala's words).
pub fn check(set: &Set, s: &Schema, ty: &TypeRef, value: &str) -> Result<(), String> {
    // The derivation chain, most derived first: each step's facets, and
    // the built-in type at its root.
    let mut steps: Vec<&super::Facets> = Vec::new();
    let (mut schema, mut cur) = (s, ty.clone());
    let builtin = loop {
        let st = match &cur {
            TypeRef::Builtin(b) => break *b,
            TypeRef::Local(i) => match schema.types.get(*i) {
                Some(Type::Simple(st)) => st,
                _ => return Err("an element type where a simple type is needed".into()),
            },
            TypeRef::Named(q) => match set.named_type(q) {
                Some((s2, Type::Simple(st))) => {
                    schema = s2;
                    st
                }
                Some(_) => return Err("a complex type where a simple type is needed".into()),
                None => {
                    let mut why = String::from("the type '");
                    why.push_str(&q.local);
                    why.push_str("' is not declared");
                    return Err(why);
                }
            },
        };
        if steps.len() > 64 {
            return Err("a type derived from itself".into());
        }
        steps.push(&st.facets);
        cur = st.base.clone();
    };
    let white = steps.iter().find_map(|f| f.white).unwrap_or(builtin.white());
    let v = apply_white(value, white);
    let mut why = String::new();
    let fail = |why: &mut String, what: &str| {
        why.push_str("The value '");
        why.push_str(&v);
        why.push_str("' ");
        why.push_str(what);
        why.push('.');
        Err(core::mem::take(why))
    };
    if let Err(what) = builtin.check(&v) {
        return fail(&mut why, what);
    }
    let len = builtin.length(&v);
    let number = match builtin {
        Builtin::Integer(..) => parse_integer(&v),
        _ => None,
    };
    for f in &steps {
        if !f.enums.is_empty() && !f.enums.iter().any(|e| apply_white(e, white) == v) {
            return fail(&mut why, "is not one of the enumeration's values");
        }
        for alternatives in &f.patterns {
            if !alternatives.iter().any(|r| r.is_match(&v)) {
                return fail(&mut why, "does not match the type's pattern");
            }
        }
        if f.length.is_some_and(|n| len != n)
            || f.min_length.is_some_and(|n| len < n)
            || f.max_length.is_some_and(|n| len > n)
        {
            return fail(&mut why, "has a length the type does not allow");
        }
        if f.min.is_some() || f.max.is_some() {
            let Some(n) = number else {
                return fail(&mut why, "is bounded by facets gala checks only on integers");
            };
            let low = f.min.is_none_or(|(m, incl)| if incl { n >= m } else { n > m });
            let high = f.max.is_none_or(|(m, incl)| if incl { n <= m } else { n < m });
            if !low || !high {
                return fail(&mut why, "is out of the type's range");
            }
        }
    }
    Ok(())
}
