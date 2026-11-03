"""Picks the corpus's version directories from a winget-pkgs file list.

Usage (docs/WORKFLOW.md "Installing software"):
    git -C <clone> ls-tree -r HEAD --name-only manifests > paths.txt
    python3 -I pick.py paths.txt 400 > pick.txt
    git -C <clone> sparse-checkout set --no-cone --stdin < pick.txt

One version directory per package, at most six files each (to keep the
corpus small), chosen by the SHA-1 of the directory's path: deterministic
for a given snapshot, and spread over the alphabet and over the years
(so over the schema versions).
"""

import collections
import hashlib
import sys


def main():
    paths, n = sys.argv[1], int(sys.argv[2])
    dirs = collections.defaultdict(list)
    with open(paths, encoding="utf-8") as f:
        for line in f:
            p = line.strip()
            if p.endswith(".yaml"):
                d, name = p.rsplit("/", 1)
                dirs[d].append(name)
    candidates = sorted(
        (hashlib.sha1(d.encode()).hexdigest(), d) for d, names in dirs.items() if len(names) <= 6
    )
    seen, out = set(), []
    for _, d in candidates:
        package = d.rsplit("/", 1)[0]
        if package in seen:
            continue
        seen.add(package)
        out.append(d)
        if len(out) == n:
            break
    for d in sorted(out):
        print(d)


main()
