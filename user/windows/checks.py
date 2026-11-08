#!/usr/bin/env python3
"""Locate "check N" in a test program's source.

A test program counts its checks at run time: every `c.eq(...)`/`c.ok(...)`
call increments the counter (user/<program>/src/main.rs), and the report
line names the count that failed. This lists the call sites in source
order with the loops around them, so that a reported number can be read
back to the assertion it belongs to.

    python checks.py user/peb_test/src/main.rs          # all call sites
    python checks.py user/peb_test/src/main.rs 15       # just check 15

A call site inside a loop runs more than once, and one inside a branch that
is not taken does not run at all, so source order is the run order only so
far: the last line of the output says where counting stops being exact,
and a call site in a branch is marked. Where a test prints its source line
instead of (or beside) the count, that line is the answer and this is only
a cross-check.
"""

import re
import sys

# A call on `self` is the Checks implementation calling itself (eq calls ok),
# not a check in the program, and must not be counted.
CALL = re.compile(r"\b(?!self\b)[a-z_]+\.(eq|ok|is|at|eq64|line)\s*\(")
LOOP = re.compile(r"^\s*(\}\s*)?(for|while|loop)\b")


def sites(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        lines = f.readlines()
    out, depth, loop_stack, first_loop = [], 0, [], None
    for i, text in enumerate(lines, 1):
        if LOOP.search(text):
            loop_stack.append(depth + text.count("{") - text.count("}"))
        opens, closes = text.count("{"), text.count("}")
        for m in CALL.finditer(text):
            in_loop = len(loop_stack)
            if in_loop and first_loop is None:
                first_loop = len(out) + 1
            out.append((len(out) + 1, i, in_loop, text.strip()))
        depth += opens - closes
        while loop_stack and depth < loop_stack[-1]:
            loop_stack.pop()
    return out, first_loop


def main(argv):
    if not argv:
        print(__doc__)
        return 2
    path = argv[0]
    want = int(argv[1]) if len(argv) > 1 else None
    out, first_loop = sites(path)
    for idx, line, in_loop, text in out:
        if want is not None and idx != want:
            continue
        mark = f" [in {in_loop} loop(s)]" if in_loop else ""
        print(f"#{idx:4} line {line:5}{mark}  {text[:110]}")
    print(f"--- {len(out)} call sites in {path}")
    if first_loop:
        print(f"--- source order is exact only up to #{first_loop - 1}: "
              f"#{first_loop} is the first inside a loop")
    else:
        print("--- no call site is inside a loop: source order is the run order")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
