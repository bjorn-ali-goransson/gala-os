//! Table-driven tests of the condition evaluator. Each row cites the SDK
//! page whose rule it checks (learn.microsoft.com/windows/win32/msi/...):
//! CSS = conditional-statement-syntax, EX = examples-of-conditional-
//! statement-syntax, UP = using-properties-in-conditional-statements.
//! No msiexec runs here: the expected values are the documented rules.

use super::*;
use std::string::ToString;

struct Ctx {
    props: BTreeMap<String, String>,
}

impl Context for Ctx {
    fn property(&self, name: &str) -> Option<String> {
        self.props.get(name).cloned()
    }
    fn environment(&self, name: &str) -> Option<String> {
        // Environment names are not case sensitive (CSS).
        match name.to_ascii_uppercase().as_str() {
            "TEMP" => Some("C:\\Temp".into()),
            "NUMBER_OF_PROCESSORS" => Some("8".into()),
            _ => None,
        }
    }
    fn component_action(&self, name: &str) -> Option<i32> {
        match name {
            "CompLocal" => Some(LOCAL),
            "CompGone" => Some(ABSENT),
            "CompSrc" => Some(SOURCE),
            "CompSame" => Some(LOCAL),
            _ => None,
        }
    }
    fn component_installed(&self, name: &str) -> Option<i32> {
        match name {
            "CompGone" => Some(LOCAL),
            "CompSame" => Some(LOCAL),
            "CompLocal" | "CompSrc" => Some(ABSENT),
            _ => None,
        }
    }
    fn feature_action(&self, name: &str) -> Option<i32> {
        match name {
            "FeatNew" => Some(LOCAL),
            "FeatGone" => Some(ABSENT),
            "FeatNone" => Some(UNKNOWN),
            _ => None,
        }
    }
    fn feature_installed(&self, name: &str) -> Option<i32> {
        match name {
            "FeatNew" => Some(ABSENT),
            "FeatGone" => Some(LOCAL),
            "FeatNone" => Some(ABSENT),
            _ => None,
        }
    }
}

fn ctx() -> Ctx {
    let mut props = BTreeMap::new();
    for (k, v) in [
        ("VersionNT", "603"),
        ("VersionNT64", "603"),
        ("Installed", ""),
        ("REMOVE", "ALL"),
        ("NAME", "Gala Installer"),
        ("LOWER", "gala"),
        ("ZERO", "0"),
        ("NEG", "-5"),
        ("BITS", "65537"),
        ("TEN", "10"),
        ("NINE", "9"),
        ("TEXT", "abc"),
        ("PATCH", "C:\\p\\fix.msp"),
        ("MEDIASRCPROPNAME", "fix.msp"),
        ("QUOTES", "\""),
        ("SERVERNAME", "srv\"1"),
        ("NAMEREF", "NAME"),
    ] {
        props.insert(k.to_string(), v.to_string());
    }
    Ctx { props }
}

/// (condition, expected, the rule and its page).
const CASES: &[(&str, bool, &str)] = &[
    // The empty condition: no condition (MsiEvaluateCondition's
    // msiEvaluateConditionNone; the sequence tables run the action).
    ("", true, "empty: none"),
    ("   ", true, "blank: none"),
    // A property alone is true when set (UP).
    ("VersionNT", true, "UP: a set property is True"),
    ("NOT VersionNT", false, "UP: Not MyProperty"),
    ("NOSUCH", false, "CSS: nonexistent property is an empty string"),
    ("NOT NOSUCH", true, "CSS/UP"),
    ("Installed", false, "UP: set to empty is not set"),
    ("NOT Installed", true, "EX: run only if not installed"),
    ("ZERO", true, "UP: \"0\" is a set property"),
    ("0", false, "integer alone: zero is false"),
    ("7", true, "integer alone: non-zero is true"),
    ("\"\"", false, "empty literal"),
    ("\"x\"", true, "non-empty literal"),
    // Comparisons (CSS, comparative operators).
    ("VersionNT = 603", true, "CSS: = integer"),
    ("VersionNT >= 600", true, "CSS: >="),
    ("VersionNT > 603", false, "CSS: >"),
    ("VersionNT < 700", true, "CSS: <"),
    ("VersionNT <= 602", false, "CSS: <="),
    ("VersionNT <> 603", false, "CSS: <>"),
    ("TEN > NINE", false, "two property values compare as text: \"10\" < \"9\""),
    ("TEN > 9", true, "property and integer compare as integers"),
    ("NEG < 0", true, "negative integers"),
    ("-5 = NEG", true, "integer on the left"),
    ("NAME = \"Gala Installer\"", true, "CSS: string ="),
    ("NAME = \"gala installer\"", false, "CSS: case sensitive"),
    ("NAME ~= \"gala installer\"", true, "CSS: ~ is not case sensitive"),
    ("NAME ~<> \"GALA INSTALLER\"", false, "CSS: ~<>"),
    ("LOWER < \"galb\"", true, "string <"),
    ("LOWER ~> \"GALA\"", false, "string ~>"),
    ("LOWER ~>= \"GALA\"", true, "string ~>="),
    ("REMOVE~=\"ALL\"", true, "conditioning actions to run during removal"),
    ("REMOVE=\"All\"", false, "case sensitive"),
    // Integer against a string that is not one: false, <> true (CSS).
    ("TEXT = 5", false, "CSS: integer vs non-integer string"),
    ("TEXT < 5", false, "CSS: integer vs non-integer string"),
    ("TEXT <> 5", true, "CSS: except <>"),
    ("NOSUCH = 0", false, "unset property vs integer"),
    ("NOSUCH <> 0", true, "unset property vs integer, <>"),
    ("NOSUCH = \"\"", true, "unset property is the empty string"),
    ("5 = \"5\"", true, "a literal holding an integer converts"),
    // Substring operators (CSS).
    ("NAME >< \"Inst\"", true, "CSS: >< contains"),
    ("NAME >< \"inst\"", false, "CSS: >< case sensitive"),
    ("NAME ~>< \"INST\"", true, "CSS: ~><"),
    ("NAME << \"Gala\"", true, "CSS: << starts with"),
    ("NAME << \"Installer\"", false, "CSS: << starts with"),
    ("NAME >> \"Installer\"", true, "CSS: >> ends with"),
    ("NAME ~>> \"INSTALLER\"", true, "CSS: ~>>"),
    ("PATCH AND PATCH >< MEDIASRCPROPNAME", true, "EX: when a patch is applied"),
    ("NOT SERVERNAME >< QUOTES", false, "CSS note: quotes through a property"),
    // Bitwise on integers (CSS).
    ("BITS >< 1", true, "CSS: >< bitwise AND"),
    ("BITS >< 2", false, "CSS: no bits in common"),
    ("BITS << 1", true, "CSS: << high 16 bits"),
    ("BITS >> 1", true, "CSS: >> low 16 bits"),
    ("BITS >> 2", false, "CSS: >> low 16 bits"),
    // Logical operators and precedence: Not, And, Or, Xor, Eqv, Imp
    // from high to low (CSS).
    ("1 AND 0", false, "CSS: And"),
    ("1 OR 0", true, "CSS: Or"),
    ("1 XOR 1", false, "CSS: Xor"),
    ("1 XOR 0", true, "CSS: Xor"),
    ("0 EQV 0", true, "CSS: Eqv"),
    ("1 EQV 0", false, "CSS: Eqv"),
    ("1 IMP 0", false, "CSS: Imp"),
    ("0 IMP 0", true, "CSS: Imp"),
    ("NOT 0 AND 0", false, "Not binds tighter than And"),
    ("NOT (0 AND 0)", true, "parentheses"),
    ("1 OR 1 AND 0", true, "And binds tighter than Or"),
    ("(1 OR 1) AND 0", false, "parentheses override"),
    ("1 OR 1 XOR 1", false, "Or binds tighter than Xor: (1 OR 1) XOR 1"),
    ("0 XOR 1 EQV 0", false, "Xor binds tighter than Eqv"),
    ("0 EQV 0 IMP 0", false, "Eqv binds tighter than Imp: (0 EQV 0) IMP 0"),
    ("0 IMP 0 IMP 0", false, "Imp left-associative: (0 IMP 0) IMP 0"),
    ("not VersionNT or ZERO", true, "CSS: operators are not case sensitive"),
    ("NOT NOT 1", true, "double negation"),
    ("VersionNT64 AND VersionNT >= 600 AND NOT Installed", true, "a typical launch condition"),
    // Environment variables (CSS: names not case sensitive).
    ("%TEMP = \"C:\\Temp\"", true, "CSS: %environment"),
    ("%temp", true, "CSS: environment names not case sensitive"),
    ("%NUMBER_OF_PROCESSORS > 4", true, "environment value as integer"),
    ("%NOSUCHVAR", false, "unset environment variable"),
    // Feature and component states (CSS table; EX).
    ("&FeatNew=3", true, "CSS: &MyFeature=3, changing to local"),
    ("(&FeatNew=3) AND NOT(!FeatNew=3)", true, "EX: installed local, not a reinstall"),
    ("(&FeatGone=2) AND (!FeatGone=3)", true, "EX: feature will be uninstalled"),
    ("(?CompGone=3) AND ($CompGone=2 OR $CompGone=4)", true, "EX: component leaving local"),
    ("?CompSame=$CompSame", true, "EX: reinstallation of a component"),
    ("?CompLocal=$CompLocal", false, "EX: not a reinstallation"),
    ("$CompSrc=4", true, "CSS: INSTALLSTATE_SOURCE"),
    ("&FeatNone=-1", true, "CSS: INSTALLSTATE_UNKNOWN"),
    ("&FeatNone", true, "a state alone is an integer: -1 is non-zero"),
    ("&NoSuchFeature=3", false, "a feature without a state reads as empty"),
    ("&NoSuchFeature<>3", true, "... and <> an integer is true"),
];

#[test]
fn documented_rules() {
    let c = ctx();
    let mut failed = std::vec::Vec::new();
    for &(cond, want, rule) in CASES {
        match evaluate(cond, &c) {
            Ok(got) if got == want => {}
            Ok(got) => failed.push(std::format!("{cond:?}: {got}, want {want} ({rule})")),
            Err(e) => failed.push(std::format!("{cond:?}: {e} ({rule})")),
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

#[test]
fn syntax_errors_name_the_place() {
    for (cond, at, what) in [
        ("NAME = ", 7, "end"),
        ("NAME = \"open", 7, "closing quotation"),
        ("(1 AND 0", 8, "expected )"),
        ("1 2", 2, "after a complete expression"),
        ("A + B", 2, "unexpected character"),
        ("~ A", 0, "~ must be followed"),
        ("$ = 1", 0, "not followed by a name"),
        ("99999999999 > 1", 0, "out of range"),
        ("AND 1", 0, "expected a value"),
        ("A = = B", 4, "expected a value"),
        ("1.5 > 1", 0, "runs into a name"),
    ] {
        let e = Condition::parse(cond).unwrap_err();
        assert_eq!(e.at, at, "{cond:?}: {e}");
        assert!(e.to_string().contains(what), "{cond:?}: {e}");
    }
}

#[test]
fn never_panics_on_any_input() {
    let c = ctx();
    let alphabet: std::vec::Vec<char> =
        "AN OTRXEQVIMP()\"~=<>!$?&%-0123456789aZ_.\t".chars().collect();
    let mut x = 0x2545_f491u32;
    for len in 0..40 {
        for _ in 0..200 {
            let s: String = (0..len)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    alphabet[x as usize % alphabet.len()]
                })
                .collect();
            let _ = evaluate(&s, &c);
        }
    }
}
