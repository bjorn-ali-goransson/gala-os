"""plan.py CHROOT OUT.json TRACE...: the files the traced processes used in
the chroot (`strace -f -y`: absolute paths and the paths opened descriptors
name), by owning package, as package paths (usrmerge: /lib -> usr/lib).
Files no package owns (generated caches, the users) are listed under "".
A file reached through update-alternatives, or through a link into another
package, is "<installed>=<the file in its package>". A file used through a
directory link's target is also wanted under the link (boot modules carry
no links). WORKFLOW "Linux roots from Ubuntu 26.04"."""
import json, re, subprocess, sys

chroot, out, traces = sys.argv[1], sys.argv[2], sys.argv[3:]
skip = ("/proc", "/dev", "/sys", "/tmp", "/run", "/work", "/root", "/home", "/etc/ld.so.cache")
paths = set()
for t in traces:
    for line in open(t, errors="replace"):
        # A failed call names nothing used. An unfinished one (another
        # process's line in between) counts if its file exists (below).
        if "= -1" in line:
            continue
        # strace -y: the path an opened descriptor names (relative opens).
        found = re.findall(r'"(/[^"]*)"', line) + re.findall(r"= \d+<(/[^>]*)>", line)
        for p in found:
            if not p.startswith(skip):
                paths.add(p)

def run(*args):
    return subprocess.run(["chroot", chroot, *args], capture_output=True, text=True).stdout

# Directory links (usr/share/X11/xkb -> xkeyboard-config-2): a file used
# through the target (a relative open after a chdir) is also wanted under
# the link, since boot modules carry no links: the program may name it so.
dirlinks = []
for line in run("sh", "-c", "find /usr /etc -xdev -type l -xtype d 2>/dev/null | while read l; do echo \"$l $(readlink -f $l)\"; done").splitlines():
    link, target = line.split(" ", 1)
    # Not a link to its own ancestor (/usr/bin/X11 -> /usr/bin): a loop.
    if not link.startswith(target + "/"):
        dirlinks.append((link, target))
for p in list(paths):
    for link, target in dirlinks:
        if p.startswith(target + "/"):
            paths.add(link + p[len(target):])

# Regular files only, each with its resolved target.
files = {}
for p in sorted(paths):
    out_ = run("sh", "-c", 'test -f "$1" && readlink -f "$1"', "-", p).strip()
    if out_:
        files[p] = out_

def owner(path):
    o = run("dpkg", "-S", path)
    m = re.match(r"([^:,]+)(:[a-z0-9]+)?(, [^:]+)*: ", o)
    return m.group(1) if m else ""

def norm(p):
    p = p.lstrip("/")
    return "usr/" + p if p.startswith(("lib/", "lib64/", "bin/", "sbin/")) else p

def through_alternatives(p):
    chain = run("sh", "-c", 'p="$1"; while [ -L "$p" ]; do t=$(readlink "$p"); case $t in /*) p=$t;; *) p=$(dirname "$p")/$t;; esac; echo "$p"; done', "-", p)
    return "/etc/alternatives/" in chain

by = {}
for p, real in files.items():
    if through_alternatives(p):
        # Installed by update-alternatives: the package's file under this name.
        by.setdefault(owner(real), set()).add(norm(p) + "=" + norm(real))
        continue
    if real != p and owner(p) and owner(real) and owner(p) != owner(real) and not p.startswith(tuple(l for l, _ in dirlinks)):
        # A link into another package (coreutils-from-uutils' /usr/bin/sleep
        # is rust-coreutils' multicall binary): that package's file here.
        by.setdefault(owner(real), set()).add(norm(p) + "=" + norm(real))
        continue
    pkg = owner(p) or owner(real)
    if not pkg:
        # usrmerge: dpkg may know the /usr path only.
        alt = "/" + norm(p)
        pkg = owner(alt) if alt != p else ""
    by.setdefault(pkg, set()).add(norm(p))
json.dump({k: sorted(v) for k, v in by.items()}, open(out, "w"), indent=1)
print(len(files), "files,", len(by) - ("" in by), "packages;", "unowned:", sorted(by.get("", [])))
