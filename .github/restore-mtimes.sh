#!/bin/sh
# Makes cargo's freshness checks right for a target/ restored from a CI
# cache (WORKFLOW "CI build caches"). Cargo rebuilds a crate when one of its
# sources is newer than its artifacts; a fresh checkout dates every file
# now, which rebuilds everything, and dating every file in the past would
# keep a changed file's stale artifact. So: every tracked file is dated
# 2000-01-01, then the files changed since the commit the cache was built
# from (its stamp) are dated now. Without a stamp, or when that commit
# cannot be fetched, every file is dated now: a full build, never a stale
# one. Afterwards the stamp names HEAD, for the cache saved after the build.
#
# Usage: .github/restore-mtimes.sh <stamp file>
set -eu
stamp=$1
now_all() {
	echo "restore-mtimes: $1: every file dated now (a full build)"
	git ls-files -z | xargs -0 touch -c --
}
if [ ! -f "$stamp" ]; then
	now_all "no cache stamp"
else
	built=$(cat "$stamp")
	if git cat-file -e "$built^{commit}" 2>/dev/null ||
		git fetch -q --depth=1 origin "$built" 2>/dev/null; then
		git ls-files -z | xargs -0 touch -c -d '2000-01-01 00:00:00' --
		changed=$(git diff --name-only --diff-filter=d "$built" HEAD | wc -l)
		git diff --name-only --diff-filter=d -z "$built" HEAD | xargs -0 -r touch -c --
		echo "restore-mtimes: cache built from $built; $changed file(s) changed since"
	else
		now_all "the cache's commit $built cannot be fetched"
	fi
fi
mkdir -p "$(dirname "$stamp")"
git rev-parse HEAD > "$stamp"
