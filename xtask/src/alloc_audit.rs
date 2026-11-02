//! `cargo xtask alloc-audit`: finds the kernel's infallible allocations in
//! its machine code, where clippy cannot see them.
//!
//! `kernel/clippy.toml` bans the infallible allocating APIs by name, but
//! not everything that allocates has a name clippy can ban: `Clone::clone`
//! of a `Vec`, a `String` or a `Box`, `Iterator::collect`, `Extend`...
//! Such an allocation panics ("memory allocation of N bytes failed") when
//! the heap is exhausted, or when the `heap` failure point fires
//! (failpoint_heap found `image.clone()` in GalaCreateProcess that way).
//!
//! The audit disassembles the kernel (`objdump -d -C`), builds the call
//! graph and marks every library (non-`kernel::`) function from which
//! `alloc`'s out-of-memory abort (`handle_alloc_error`,
//! `raw_vec::handle_error`) is reachable through library code. A kernel
//! function that calls one of those directly allocates infallibly, unless
//! the call is a growth the code reserved for first (`try_reserve` then
//! `extend`, `resize`, a `TryVec` method inlined): those kinds are listed
//! only with `--all`. The rest is reported, and fails the audit unless
//! allowed in [`ALLOWED`] with the reason.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Calls a reservation normally precedes (`try_reserve` then the growth,
/// which then cannot allocate): `extend`, `resize`, a push or a VecDeque
/// growth after `try_reserve`. Shown with `--all`, not failed.
const RESERVED_KINDS: &[&str] = &[
    ">::spec_extend",
    ">::extend_trusted::",
    ">::extend_desugared::",
    ">::reserve::do_reserve_and_handle::",
    ">::resize",
    ">::grow_one",
    ">::grow",
];

/// (kernel function, library call) pairs that cannot allocate, and why
/// (substrings of the demangled names).
const ALLOWED: &[(&str, &str, &str)] = &[(
    "kernel::net::tcp::Stack",
    "SocketSet>::add",
    "the socket set's storage is reserved for MAX_SOCKETS when the stack is made",
)];

/// One kernel function calling library code that may abort on OOM.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Finding {
    pub caller: String,
    pub callee: String,
    /// The callee without generic arguments (`<..>`), e.g.
    /// `<>::clone` for `<Vec<u16> as Clone>::clone`.
    pub kind: String,
    /// A reserved growth (RESERVED_KINDS) or an ALLOWED pair.
    pub excused: bool,
}

/// Whether `f` (demangled) is the kernel's own code: a `kernel::` path, an
/// inherent method of a kernel type or a kernel trait's method. Generic
/// library code instantiated with kernel types (`RawVec<kernel::..>`) is
/// library code. `TryVec`'s methods count as library code that cannot
/// abort (they reserve first).
fn is_kernel(f: &str) -> bool {
    if f.contains("TryVec") {
        return false;
    }
    if f.starts_with("kernel::") || f.starts_with("<kernel::") {
        return true;
    }
    // `<Type as kernel::Trait>::method`: the `as` at nesting depth 1.
    let mut depth = 0;
    for (i, c) in f.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ' ' if depth == 1 && f[i..].starts_with(" as kernel::") => return true,
            _ => {}
        }
    }
    false
}

/// `name` without its generic arguments.
fn kind(name: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in name.chars() {
        match c {
            '<' => {
                if depth == 0 {
                    out.push('<');
                }
                depth += 1;
            }
            '>' => {
                depth -= 1;
                if depth == 0 {
                    out.push('>');
                }
            }
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// The call graph of `objdump -d --no-show-raw-insn -C` output: each
/// function's direct callees (calls and tail jumps to function starts).
fn call_graph(disassembly: &str) -> HashMap<&str, HashSet<&str>> {
    let mut calls: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut current: Option<&str> = None;
    for line in disassembly.lines() {
        // `ffffffff80012340 <name>:`
        if let Some(rest) = line.strip_suffix(">:")
            && let Some((addr, name)) = rest.split_once(" <")
            && addr.chars().all(|c| c.is_ascii_hexdigit())
        {
            current = Some(name);
            calls.entry(name).or_default();
            continue;
        }
        // `  addr:\tcall   ffffffff80001000 <name>` (not `<name+0x10>`).
        let Some(f) = current else { continue };
        let mut words = line.split_whitespace();
        let (Some(_), Some(op)) = (words.next(), words.next()) else { continue };
        if !matches!(op, "call" | "jmp") {
            continue;
        }
        let Some(target) = line.split_once(" <").map(|(_, t)| t) else { continue };
        let Some(target) = target.strip_suffix('>') else { continue };
        if target.ends_with(|c: char| c.is_ascii_hexdigit())
            && target
                .rsplit_once("+0x")
                .is_some_and(|(_, off)| off.chars().all(|c| c.is_ascii_hexdigit()))
        {
            continue; // a jump inside a function
        }
        if target != f {
            calls.entry(f).or_default().insert(target);
        }
    }
    calls
}

/// The audit of a disassembled kernel.
pub fn audit(disassembly: &str) -> Vec<Finding> {
    let calls = call_graph(disassembly);
    let mut callers: HashMap<&str, Vec<&str>> = HashMap::new();
    for (&f, callees) in &calls {
        for &c in callees {
            callers.entry(c).or_default().push(f);
        }
    }
    let is_sink = |f: &str| f.contains("handle_alloc_error") || f.contains("raw_vec::handle_error");
    // Library functions that may abort on OOM: the sinks and what reaches
    // them through library code.
    let mut aborts: HashSet<&str> = HashSet::new();
    let mut queue: Vec<&str> = Vec::new();
    for f in calls.keys().chain(calls.values().flatten()) {
        if is_sink(f) && aborts.insert(f) {
            queue.push(f);
        }
    }
    while let Some(f) = queue.pop() {
        for &c in callers.get(f).into_iter().flatten() {
            if !is_kernel(c) && !c.contains("TryVec") && aborts.insert(c) {
                queue.push(c);
            }
        }
    }
    let mut found = BTreeSet::new();
    for (&f, callees) in &calls {
        if !is_kernel(f) {
            continue;
        }
        for &c in callees {
            if !aborts.contains(c) || is_kernel(c) {
                continue;
            }
            let kind = kind(c);
            let excused = RESERVED_KINDS.iter().any(|k| kind.contains(k))
                || ALLOWED.iter().any(|(a, b, _)| f.contains(a) && c.contains(b));
            found.insert(Finding { caller: f.into(), callee: c.into(), kind, excused });
        }
    }
    found.into_iter().collect()
}

/// A summary for the terminal: the unexcused findings (all with `all`),
/// grouped by kind.
pub fn describe(findings: &[Finding], all: bool) -> String {
    let mut by_kind: BTreeMap<&str, Vec<&Finding>> = BTreeMap::new();
    for f in findings.iter().filter(|f| all || !f.excused) {
        by_kind.entry(&f.kind).or_default().push(f);
    }
    let mut out = String::new();
    for (kind, list) in by_kind {
        let excused = if list[0].excused { " (reserved first or allowed)" } else { "" };
        out += &format!("{kind}{excused}:\n");
        for f in list {
            let callee: String = f.callee.chars().take(160).collect();
            out += &format!("    {}\n        calls {callee}\n", f.caller);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of the failure failpoint_heap found: GalaCreateProcess
    /// cloned a Vec<u16> (an infallible allocation clippy cannot ban).
    const DISASSEMBLY: &str = "\
ffffffff80001000 <alloc::raw_vec::handle_error>:
ffffffff80001000:\tud2

ffffffff80001010 <<alloc::raw_vec::RawVecInner>::with_capacity_in>:
ffffffff80001010:\tcall   ffffffff80001000 <alloc::raw_vec::handle_error>
ffffffff80001015:\tjmp    ffffffff80001019 <<alloc::raw_vec::RawVecInner>::with_capacity_in+0x9>

ffffffff80001020 <<alloc::vec::Vec<u16> as core::clone::Clone>::clone>:
ffffffff80001020:\tcall   ffffffff80001010 <<alloc::raw_vec::RawVecInner>::with_capacity_in>

ffffffff80001030 <<alloc::vec::Vec<u16> as kernel::mm::heap::TryVec<u16>>::try_push>:
ffffffff80001030:\tcall   ffffffff80001040 <<alloc::raw_vec::RawVec<u16>>::grow_one>

ffffffff80001040 <<alloc::raw_vec::RawVec<u16>>::grow_one>:
ffffffff80001040:\tjmp    ffffffff80001000 <alloc::raw_vec::handle_error>

ffffffff80001050 <kernel::nt::syscalls::gala_create_process>:
ffffffff80001050:\tcall   ffffffff80001020 <<alloc::vec::Vec<u16> as core::clone::Clone>::clone>
ffffffff80001055:\tcall   ffffffff80001030 <<alloc::vec::Vec<u16> as kernel::mm::heap::TryVec<u16>>::try_push>
ffffffff8000105a:\tcall   ffffffff80001060 <kernel::nt::syscalls::read_wide>

ffffffff80001060 <kernel::nt::syscalls::read_wide>:
ffffffff80001060:\tcall   ffffffff80001040 <<alloc::raw_vec::RawVec<u16>>::grow_one>
ffffffff80001065:\tret
";

    #[test]
    fn finds_a_cloned_vec_and_excuses_reserved_growth() {
        let found = audit(DISASSEMBLY);
        let failing: Vec<_> = found.iter().filter(|f| !f.excused).collect();
        assert_eq!(failing.len(), 1, "{found:#?}");
        assert_eq!(failing[0].caller, "kernel::nt::syscalls::gala_create_process");
        assert_eq!(failing[0].kind, "<>::clone");
        // TryVec::try_push (reserves first) is not a finding; a direct
        // grow_one (a push inlined after try_reserve) is an excused one.
        assert!(found.iter().all(|f| !f.callee.contains("try_push")));
        let grow = found.iter().find(|f| f.caller.ends_with("read_wide")).unwrap();
        assert!(grow.excused);
    }

    #[test]
    fn tells_kernel_code_from_library_code() {
        assert!(is_kernel("kernel::nt::thread::dump"));
        assert!(is_kernel("<kernel::nt::pipe::PipeEnd>::write"));
        assert!(is_kernel("<kernel::fs::Disk as kernel::platform::Storage>::read"));
        assert!(is_kernel("<core::option::Option<u8> as kernel::x::Trait>::f"));
        assert!(!is_kernel("<alloc::raw_vec::RawVec<kernel::nt::object::KObject>>::grow_one"));
        assert!(!is_kernel("<alloc::vec::Vec<u8> as kernel::mm::heap::TryVec<u8>>::try_push"));
        assert_eq!(kind("<alloc::vec::Vec<u16> as core::clone::Clone>::clone"), "<>::clone");
    }
}
