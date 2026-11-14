"""rows.py INDEX_DIR PLAN.json OUT.rs [--drop PKG,...] [--exclude FILELIST] [--add PKG=FILE ...]:
the LinuxRoot `Deb` rows for a plan (plan.py's output), each package
pinned to the release pocket's version and SHA-256 from INDEX_DIR's
Packages-main and Packages-universe (resolute's, unpacked from the
Packages.xz whose hashes the gpgv-checked InRelease lists). Files listed in FILELIST (one per line: other roots') and the
packages in --drop are left out; --add adds a file of a package. Prints the
root's files (one per line) to OUT.rs.files for the next root's --exclude."""
import json, re, sys

args = sys.argv[1:]
index = args.pop(0)
plan, out = json.load(open(args[0])), args[1]
drop, exclude, add = set(), set(), []
i = 2
while i < len(args):
    if args[i] == "--drop":
        drop |= set(args[i + 1].split(",")); i += 2
    elif args[i] == "--exclude":
        exclude |= set(l.strip() for l in open(args[i + 1]) if l.strip()); i += 2
    elif args[i] == "--add":
        k, f = args[i + 1].split("=", 1); add.append((k, f)); i += 2
    else:
        sys.exit("unknown " + args[i])
idx = {}
for c in ("main", "universe"):
    for para in open(f"{index}/Packages-{c}").read().split("\n\n"):
        f = dict(re.findall(r"^(Package|Version|Filename|SHA256): (.*)$", para, re.M))
        if "Package" in f:
            idx[f["Package"]] = f
plan.pop("", None)
for k, f in add:
    plan.setdefault(k, []).append(f)
rows, files = [], []
for pkg in sorted(plan):
    if pkg in drop:
        continue
    fs = sorted(f for f in {re.sub("/+", "/", f) for f in plan[pkg]} if f.split("=")[0] not in exclude)
    if not fs:
        continue
    rel = idx[pkg]
    files += [f.split("=")[0] for f in fs]
    body = (f'files: &["{fs[0]}"],' if len(fs) == 1 else
            "files: &[\n" + "".join(f'                    "{f}",\n' for f in fs) + "                ],")
    rows.append(f'''            Deb {{
                pool: "{rel["Filename"].removeprefix("pool/")}",
                sha256: "{rel["SHA256"]}",
                {body}
            }},''')
open(out, "w").write("\n".join(rows) + "\n")
open(out + ".files", "w").write("\n".join(files) + "\n")
print(len(rows), "debs,", len(files), "files")
