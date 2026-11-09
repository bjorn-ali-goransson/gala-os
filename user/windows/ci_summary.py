#!/usr/bin/env python3
"""The Windows CI job's summary of a run-all.ps1 report (Markdown on stdout).

    python user/windows/ci_summary.py windows.json [--baseline FILE]

Lists how many programs passed and, for each one that did not exit 0, its
exit status, its last line and how long it ran. With --baseline (one program
name per line, `#` comments), every listed program must pass: a listed one
that fails is a regression of a test's expectation against Windows, printed
first, and the exit status is 1.
"""

import json
import sys


def main(argv):
    if len(argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    # run-all.ps1 writes UTF-8 with a byte-order mark (PowerShell 5).
    with open(argv[1], encoding="utf-8-sig") as f:
        report = json.load(f)
    baseline = []
    if "--baseline" in argv:
        with open(argv[argv.index("--baseline") + 1], encoding="utf-8") as f:
            baseline = [
                line.split("#")[0].strip()
                for line in f
                if line.split("#")[0].strip()
            ]
    results = report["results"]
    by_name = {r["program"]: r for r in results}
    passed = [r for r in results if r.get("exit") == 0]
    failed = [r for r in results if r.get("exit") != 0]

    print(f"## Test programs on {report.get('platform', 'Windows')}")
    print()
    print(f"{len(passed)} of {len(results)} exit 0.")
    print()

    regressions = [
        name for name in baseline if by_name.get(name, {}).get("exit") != 0
    ]
    if regressions:
        print(f"### {len(regressions)} that the baseline expects to pass fail")
        print()
        for name in regressions:
            r = by_name.get(name)
            if r is None:
                print(f"- `{name}`: not in the report")
            else:
                print(f"- `{name}`: exit {fmt_exit(r.get('exit'))}: {r.get('line', '')}")
        print()

    if failed:
        print("### Programs that do not exit 0")
        print()
        print("| program | exit | ms | last line |")
        print("|---|---|---|---|")
        for r in failed:
            line = r.get("line", "")
            if r.get("skipped"):
                line = f"skipped: {r['skipped']}"
            line = str(line).replace("|", "\\|")
            print(f"| `{r['program']}` | {fmt_exit(r.get('exit'))} | {r.get('ms', '')} | {line} |")
    return 1 if regressions else 0


def fmt_exit(code):
    if code is None:
        return "none (did not run)"
    return f"0x{code & 0xFFFFFFFF:08x}"


if __name__ == "__main__":
    sys.exit(main(sys.argv))
