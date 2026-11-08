#!/usr/bin/env python3
"""Builds user/windows/triage.json from a run-all.ps1 report.

For every program that did not exit 0, this fills in the mechanical part
of docs/windows-triage-brief.md step 2: which check failed, where it is in
the source and what it asserts. A program reports the failing check as its
exit status, so the status is the check number (the report line's own
number agrees, in hex or decimal depending on the test); where the line
carries a source line, that line wins, since it is exact even inside loops.

The classification is a first pass from evidence that needs no probe:

  crash        the status is an NTSTATUS (0xc0000005 and friends)
  loader       the status is STATUS_ENTRYPOINT_NOT_FOUND; imports.py says
               which import is missing and whether it is a Gala* export
  environment  the value the check got is this machine rather than a
               difference: the build (26200 where 26100 is pinned) or
               ERROR_LOGON_FAILURE from the missing galatest account
  skipped      the runner did not run it (no administrator, timed out)
  expectation  everything else: the test asserts something this machine
               does not do, which needs a probe to say what it does

    python triage.py <windows.json> [--out user/windows/triage.json]

Run from the repository root: the test sources are read from
user/<program>/src/main.rs.
"""

import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import checks as checks_mod  # noqa: E402  (same directory)

# A test may print more than one value after "got" (wow64_test prints a
# 64-bit value as two words), so every one of them is considered.
GOT = re.compile(r"\bgot\b([0-9a-fA-Fx \t]+)")
LINE = re.compile(r"\bline\s+(?:0x)?([0-9a-fA-F]+)")
NTSTATUS_MIN = 0xC0000000

BUILD_THIS = 26200          # this machine
BUILD_PINNED = 26100        # what the tests assert
ERROR_LOGON_FAILURE = 0x52E

GALA_ONLY_HINT = "Gala"


def source_of(program):
    """The program's source. The GUI and COM tests are C, not Rust."""
    for p in (os.path.join("user", program, "src", "main.rs"),
              os.path.join("user", program, "main.c"),
              os.path.join("user", program, "src", "lib.rs")):
        if os.path.exists(p):
            return p
    return None


def text_at(src, line):
    with open(src, encoding="utf-8", errors="replace") as f:
        for i, t in enumerate(f, 1):
            if i == line:
                return t.strip()
    return None


def assertion(program, check, report_line):
    """(source_line, assertion text, how certain)."""
    src = source_of(program)
    if not src:
        return None, None, "no source found under user/%s/" % program
    # A source line in the report is exact, inside a loop as much as outside,
    # and is all the C tests give: read that line whatever it holds.
    m = LINE.search(report_line or "")
    if m:
        # The tests do not agree on a base: wow64_test prints "line
        # 0x0000018a", the C tests print "line 447", and the track_caller
        # convention prints "(line L)". Take an 0x prefix or a non-decimal
        # digit as hex, zero-padded as hex (the existing convention), and
        # anything else as decimal. Never let a line nobody can parse end
        # the run.
        raw = m.group(1)
        try:
            if "0x" in m.group(0).lower() or not raw.isdigit():
                want = int(raw, 16)
            elif raw.startswith("0") and len(raw) > 3:
                want = int(raw, 16)
            else:
                want = int(raw, 10)
        except ValueError:
            return None, None, "cannot read a line number out of %r" % m.group(0)
        t = text_at(src, want)
        if t is not None:
            return want, t, "exact (%s:%d, the line the report names)" % (src, want)
        return want, None, "the report names line %d, past the end of %s" % (want, src)
    if not src.endswith(".rs"):
        return None, None, "%s is C and the report names no line" % src
    sites, first_loop, methods = checks_mod.sites(src)
    if not methods:
        return None, None, "no counting method found in %s" % src
    if check is None or check < 1 or check > len(sites):
        return None, None, "check %s is outside the %d call sites" % (check, len(sites))
    _, line, in_loop, in_branch, text = sites[check - 1]
    if first_loop and check >= first_loop:
        how = ("uncertain: #%d is at or past the first call site inside a loop (#%d), "
               "so source order is no longer the run order" % (check, first_loop))
    elif in_loop:
        how = "uncertain: inside a loop"
    elif in_branch:
        how = ("likely: source order is the run order this far, but this call site "
               "is inside a branch, which shifts the count when it is not taken")
    else:
        how = "exact (source order is the run order this far)"
    return line, text, how


def got_values(report_line):
    """Every number printed after "got" in the report line."""
    out = []
    for blob in GOT.findall(report_line or ""):
        for tok in blob.replace("0x", " ").split():
            try:
                out.append(int(tok, 16))
            except ValueError:
                pass
    return out


def classify(program, r, got, text):
    exit_ = r.get("exit")
    if r.get("skipped"):
        return "skipped", r["skipped"]
    if exit_ is not None and exit_ >= NTSTATUS_MIN:
        if exit_ == 0xC0000139:
            return "loader", "STATUS_ENTRYPOINT_NOT_FOUND: run imports.py for the missing import"
        return "crash", "status 0x%08x; run fault.py for the module, offset and access" % exit_
    if BUILD_THIS in got:
        return "environment", (
            "got %d, this machine's build, where the test pins %d"
            % (BUILD_THIS, BUILD_PINNED))
    if ERROR_LOGON_FAILURE in got:
        return "environment", (
            "got 0x52e ERROR_LOGON_FAILURE: the galatest account "
            "(password Gala-Test-1) does not exist here")
    if text and GALA_ONLY_HINT in text:
        return "gala-only", "the assertion names a Gala* facility: %s" % text.strip()
    return "expectation", "needs a probe: what does this machine do here?"


def main(argv):
    if not argv:
        print(__doc__)
        return 2
    report, out = argv[0], os.path.join("user", "windows", "triage.json")
    if "--out" in argv:
        out = argv[argv.index("--out") + 1]
    with open(report, encoding="utf-8-sig") as f:
        d = json.load(f)
    entries = []
    for r in d["results"]:
        exit_ = r.get("exit")
        if exit_ == 0 and not r.get("skipped"):
            continue
        line_text = r.get("line") or ""
        got = got_values(line_text)
        check = exit_ if (exit_ is not None and exit_ < NTSTATUS_MIN) else None
        src_line, text, how = assertion(r["program"], check, line_text)
        cls, why = classify(r["program"], r, got, text)
        entries.append({
            "program": r["program"],
            "exit": None if exit_ is None else "0x%08x" % exit_,
            "check": check,
            "source_line": src_line,
            "asserts": (text or "").strip() or None,
            "certainty": how,
            "report_line": line_text or None,
            "windows": None,
            "class": cls,
            "evidence": why,
            "note": None,
        })
    # What was actually observed here, kept by hand so that a new run of
    # run-all.ps1 does not lose it.
    found_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "findings.json")
    found, notes = {}, []
    if os.path.exists(found_path):
        with open(found_path, encoding="utf-8") as f:
            raw = json.load(f)
        notes = raw.get("_notes", [])
        found = {k: v for k, v in raw.items() if not k.startswith("_")}
    for e in entries:
        f_ = found.get(e["program"])
        if not f_:
            continue
        for key in ("windows", "class", "evidence", "note"):
            if f_.get(key) is not None:
                e[key] = f_[key]
    entries.sort(key=lambda e: (e["class"], e["program"]))
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        json.dump({"platform": d["platform"], "source_report": os.path.basename(report),
                   "entries": entries}, f, indent=2)
        f.write("\n")

    by = {}
    for e in entries:
        by.setdefault(e["class"], []).append(e)
    md = os.path.splitext(out)[0] + ".md"
    probed = sum(1 for e in entries if e["windows"])
    with open(md, "w", encoding="utf-8", newline="\n") as f:
        f.write("# The test programs on real Windows: triage\n\n")
        f.write("Machine: %s. From `%s` (%d programs did not exit 0 or did not "
                "run).\nWhat this machine does is recorded for %d of them; the rest "
                "name the failing\ncheck and what it asserts, and still need a probe "
                "(docs/windows-triage-brief.md).\n\n"
                % (d["platform"], os.path.basename(report), len(entries), probed))
        f.write("Generated by `user/windows/triage.py`; the observations live in\n"
                "`user/windows/findings.json`.\n\n")
        if notes:
            f.write("## About these results\n\n")
            for n in notes:
                f.write("- %s\n" % n)
            f.write("\n")
        for k in sorted(by):
            f.write("## %s (%d)\n\n" % (k, len(by[k])))
            for e in sorted(by[k], key=lambda x: x["program"]):
                f.write("### %s (%s)\n\n" % (e["program"], e["exit"]))
                if e["check"] is not None:
                    f.write("- check %s" % e["check"])
                    if e["source_line"]:
                        f.write(", source line %s" % e["source_line"])
                    f.write("\n")
                if e["asserts"]:
                    f.write("- asserts: `%s`\n" % e["asserts"])
                f.write("- windows: %s\n" % (e["windows"] or "*not probed*"))
                if e["evidence"]:
                    f.write("- evidence: %s\n" % e["evidence"])
                if e["note"]:
                    f.write("- note: %s\n" % e["note"])
                f.write("\n")
    print("wrote %s: %d entries (%d with an observation)" % (out, len(entries), probed))
    print("wrote %s" % md)
    for k in sorted(by):
        n = sum(1 for e in by[k] if e["windows"])
        print("  %-12s %3d  (%d observed)" % (k, len(by[k]), n))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
