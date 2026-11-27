"""Gap ranking: the gaps one run left, ordered, each told what the static
survey knows about it.

A run's missing-features ledger (OBSERVABILITY) is the measured list of
what a program asked gala for and did not get. This puts that list in the
order the work wants it in -- the gaps nearest the stop first, with their
hit counts -- and annotates each name the static survey can speak about
with how many of the program's start-up modules import it, statically (a
module does not load without it) or by delay load (only a call wants it).
Breadth is what the ledger cannot say: a gap one module imports is one
program's path, a gap twelve import is the platform's.

So the order of the work is: survey statically (tools/office_survey.py on
a Windows machine with the program installed, tools/import_survey.py
--static for what loading needs), run the scenario ONCE, rank what it
left with this, implement a batch of the ranking, and only then spend a
run from boot to verify. A run per gap is the expensive way round.

What it cannot rank, and what therefore needs the run: a COM class or an
interface method, which no import table names (a class is sixteen bytes
of data). Those rows come out with no breadth at all -- `detail` is the
one thing that identifies them, the first dword of the CLSID or IID, and
a public header (`mlang.h`, `netlistmgr.h`) is where the name of that
dword is. The ledger stays the only source for those.

usage:
  python3 -I tools/gap_rank.py <run log or ledger.jsonl>
      [--dumps <imports dir> --roots <a.exe,b.dll>]

The log may be a boot-test serial log (its `ledger: {...}` lines are
read) or those lines on their own. Without `--dumps` it ranks the ledger
alone, which is what a scenario with no survey behind it gets.
(docs/WORKFLOW.md "Ranking a run's gaps".)
"""
import json
import os
import re
import sys
from collections import defaultdict

# The ledger kinds that name work to do. "waited" and "reached" are not
# gaps (a lock a DLL took, a method it answered), and a bulk row ("?user",
# "?bulk") names no one function.
GAPS = (
    "missing export",
    "missing import",
    "missing dll",
    "stub",
    "stub refused",
    "unsupported",
    "not implemented",
    "load failed",
    "other",
)

SUFFIX = (".dll", ".drv", ".exe", ".sys", ".ocx")


def ledger(path):
    """The ledger rows of a run log, or of those lines on their own."""
    out = []
    with open(path, "rb") as f:
        for raw in f:
            line = raw.decode("utf-8", "replace").strip()
            at = line.find("ledger: {")
            if at >= 0:
                line = line[at + len("ledger: "):]
            if not line.startswith("{"):
                continue
            try:
                out.append(json.loads(line))
            except ValueError:
                pass
    return out


def survey(imports_dir, roots):
    """(dll, symbol) -> {"static": modules, "delay": modules} over the
    closure of `roots`, as tools/import_survey.py computes it: a module
    reached only through a delay import contributes delay imports only."""
    by_name, deps, syms = defaultdict(list), {}, {}
    for dirpath, _, names in os.walk(imports_dir):
        for n in names:
            rel = os.path.relpath(os.path.join(dirpath, n), imports_dir)[: -len(".txt")]
            by_name[os.path.basename(rel).lower()].append(rel)
            kind, dll, d, s = None, None, [], []
            with open(os.path.join(dirpath, n), encoding="utf-8", errors="replace") as f:
                for line in f:
                    t = line.strip()
                    if line.startswith("Import {"):
                        kind = "static"
                    elif line.startswith("DelayImport {"):
                        kind = "delay"
                    elif t.startswith("Name:"):
                        dll = t[5:].strip().lower()
                        d.append((kind, dll))
                    elif t.startswith("Symbol:") and dll:
                        m = re.match(r"Symbol: (\S*) \((\d+)\)", t)
                        if m:
                            s.append((kind, dll, m.group(1) or ("#" + m.group(2))))
            deps[rel] = d
            syms[rel] = s

    seen, todo = {}, [(r, "static") for root in roots for r in by_name.get(root.lower(), [])]
    while todo:
        rel, how = todo.pop()
        if seen.get(rel) == "static" or seen.get(rel) == how:
            continue
        seen[rel] = how
        for kind, dll in deps.get(rel, []):
            for r in by_name.get(dll, []):
                todo.append((r, "static" if how == "static" and kind == "static" else "delay"))

    need = defaultdict(lambda: defaultdict(set))
    for rel, how in seen.items():
        for kind, dll, sym in syms.get(rel, []):
            need[(dll, sym)]["static" if how == "static" and kind == "static" else "delay"].add(rel)
    loaded = sum(1 for v in seen.values() if v == "static")
    return need, loaded, len(seen) - loaded


def service(name):
    """`dll.dll!what` -> (dll, what). A row that names no module (a
    registry key, a lock, a kernel service) gives (None, name)."""
    dll, sep, what = name.partition("!")
    if not sep or not dll.lower().endswith(SUFFIX):
        return None, name
    return dll.lower(), what


def main():
    rows = ledger(sys.argv[1])
    need, loaded, delayed = {}, None, None
    if "--dumps" in sys.argv and "--roots" in sys.argv:
        d = sys.argv[sys.argv.index("--dumps") + 1]
        roots = sys.argv[sys.argv.index("--roots") + 1].split(",")
        need, loaded, delayed = survey(d, roots)
        if not loaded:
            # Silence here would rank the whole run with no breadth beside
            # any gap and look like a survey that knows nothing, which is
            # also what a COM-heavy run honestly looks like.
            sys.exit(
                f"gap_rank: nothing named {' or '.join(roots)} under {d!r}. "
                "--dumps wants the `imports` directory of the survey "
                "artifact (docs/WORKFLOW.md \"The Office survey\") and "
                "--roots the program as it is named there. Leave both out "
                "to rank the ledger alone."
            )
        print(f"== survey: {loaded} modules loaded at start, {delayed} more through delay imports")

    out, other = [], 0
    for r in rows:
        if r.get("what") not in GAPS:
            continue
        dll, what = service(r.get("service", ""))
        if dll is None:
            other += 1
            continue
        # A row whose `what` is a bare name is one the survey can speak
        # about; anything in brackets or with `::` is a method, a class or
        # a case of a function, which no import table carries.
        m = re.fullmatch(r"[A-Za-z_][A-Za-z0-9_@]*", what)
        e = need.get((dll, m.group(0))) if m else None
        out.append(
            (
                r.get("last", 0),
                r.get("hits", 0),
                r.get("what"),
                dll,
                what,
                len(e.get("static", ())) if e else 0,
                len(e.get("delay", ())) if e else 0,
                r.get("detail", 0),
            )
        )
    out.sort(key=lambda x: (-x[0], -x[1]))

    print(f"== {len(out)} gaps naming a module ({other} rows name none: keys, locks, services)")
    print("-- latest first: ms, hits, kind, gap [static importers/delay importers], detail")
    for last, hits, kind, dll, what, st, dl, detail in out:
        breadth = f"  [{st}s/{dl}d]" if st or dl else ""
        d = f"  detail 0x{detail:x}" if detail else ""
        print(f"{last:8} x{hits:<5} {kind:16} {dll}!{what}{breadth}{d}")

    per = defaultdict(lambda: [0, 0, 0])
    for last, hits, _, dll, _, _, _, _ in out:
        p = per[dll]
        p[0] += 1
        p[1] += hits
        p[2] = max(p[2], last)
    print("-- by module: gaps, hits, latest")
    for dll, (n, hits, last) in sorted(per.items(), key=lambda kv: -kv[1][1]):
        print(f"{last:8} {n:4} gaps  x{hits:<6} {dll}")


if __name__ == "__main__":
    main()
