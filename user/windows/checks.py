#!/usr/bin/env python3
"""Locate "check N" in a test program's source.

A test program counts its checks at run time: a call on its Checks value
increments the counter, and the program reports the count that failed (and
returns it as its exit status). This lists those call sites in source order
so that a reported number can be read back to the assertion it belongs to.

    python checks.py user/peb_test/src/main.rs          # all call sites
    python checks.py user/peb_test/src/main.rs 15       # just check 15

Which methods count is worked out per file rather than assumed: the tests
use `eq` and `ok` mostly, but also `st`, `ok0`, `fails` and wrappers of
their own (`gala`, `text`, `run`, `status`, `step`, ...). A method counts
when its body increments the counter (`self.0 += 1`, or `self.0.set(...)`
for the Cell variants), or when it calls a method that does; that is
resolved to a fixpoint. A call on `self` is the implementation calling
itself and is not a check.

A call site inside a loop runs more than once, and one inside a branch
that is not taken does not run at all, so source order is the run order
only so far: the last line of the output says where counting stops being
exact, and a call site in a branch is marked. Where a test prints its
source line beside the count, that line is the answer and this is only a
cross-check.
"""

import re
import sys

# `fn eq<T: PartialEq + Copy + Into<i64>>(&mut self, ...)`: the generic list
# may hold nested angle brackets, so it is matched up to the parameter list.
FN = re.compile(r"\bfn\s+([a-z_0-9]+)\s*(?:<[^(]*>)?\s*\(\s*&\s*(?:mut\s+)?self\b")
SELF_CALL = re.compile(r"\bself\.([a-z_0-9]+)\s*\(")
# The counter is `self.0` in most tests and a named field in others
# (session_test's `n`), a plain integer or a Cell.
BUMP = re.compile(r"self\.[a-z_0-9]+\s*\+=\s*1"
                  r"|self\.[a-z_0-9]+\s*\.\s*set\s*\(")
LOOP = re.compile(r"^\s*(\}\s*)?(for|while|loop)\b")
BRANCH = re.compile(r"^\s*(\}\s*)?(if|match)\b")


def _body(lines, start):
    """The text of the block that opens on or after line index `start`."""
    depth, out, began = 0, [], False
    for i in range(start, len(lines)):
        out.append(lines[i])
        depth += lines[i].count("{") - lines[i].count("}")
        if "{" in lines[i]:
            began = True
        if began and depth <= 0:
            break
    return "".join(out)


def counting_methods(lines):
    """Names of the methods that increment the check counter."""
    bodies = {}
    for i, text in enumerate(lines):
        m = FN.search(text)
        if m:
            bodies[m.group(1)] = _body(lines, i)
    counting = {n for n, b in bodies.items() if BUMP.search(b)}
    # A wrapper that calls one of them counts too; repeat until settled.
    while True:
        grown = set(counting)
        for n, b in bodies.items():
            if n in grown:
                continue
            if any(c in counting for c in SELF_CALL.findall(b)):
                grown.add(n)
        if grown == counting:
            return counting
        counting = grown


def sites(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        lines = f.readlines()
    names = counting_methods(lines)
    if not names:
        return [], None, names
    call = re.compile(r"\b(?!self\b)[a-z_0-9]+\.(" + "|".join(sorted(names)) + r")\s*\(")
    out, depth, loops, branches, first_loop = [], 0, [], [], None
    for i, text in enumerate(lines, 1):
        if LOOP.search(text):
            loops.append(depth + text.count("{") - text.count("}"))
        elif BRANCH.search(text):
            branches.append(depth + text.count("{") - text.count("}"))
        for _ in call.finditer(text):
            if loops and first_loop is None:
                first_loop = len(out) + 1
            out.append((len(out) + 1, i, len(loops), len(branches), text.strip()))
        depth += text.count("{") - text.count("}")
        while loops and depth < loops[-1]:
            loops.pop()
        while branches and depth < branches[-1]:
            branches.pop()
    return out, first_loop, names


def main(argv):
    if not argv:
        print(__doc__)
        return 2
    path = argv[0]
    want = int(argv[1]) if len(argv) > 1 else None
    out, first_loop, names = sites(path)
    for idx, line, in_loop, in_branch, text in out:
        if want is not None and idx != want:
            continue
        mark = ""
        if in_loop:
            mark += f" [in {in_loop} loop(s)]"
        if in_branch:
            mark += f" [in {in_branch} branch(es)]"
        print(f"#{idx:4} line {line:5}{mark}  {text[:100]}")
    print(f"--- {len(out)} call sites in {path}")
    print(f"--- counting methods: {', '.join(sorted(names)) or '(none found)'}")
    if first_loop:
        print(f"--- source order is exact only up to #{first_loop - 1}: "
              f"#{first_loop} is the first inside a loop")
    elif out:
        print("--- no call site is inside a loop; a branch not taken still shifts the count")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
