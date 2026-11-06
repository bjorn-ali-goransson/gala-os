#!/bin/sh
# Replaces user/roots/public-roots.pem, gala's public CA bundle (the
# certificates Mozilla's root program trusts for TLS servers, as curl.se
# publishes them from NSS's certdata.txt), with the dated release given:
#
#     sh user/roots/update.sh 2026-10-20
#
# The list of releases is https://curl.se/docs/caextract.html. The file's
# SHA-256 is checked against the one curl.se publishes beside it; then
# record the date and checksum in THIRD_PARTY.md and run
# `cargo test -p schannel_tls` (every certificate must be a trust anchor).
# Every image ships the bundle as C:\Windows\System32\gala\roots\
# public-roots.pem (ARCHITECTURE "Schannel (TLS)", "The machine root
# store").
set -eu
DATE=${1:?usage: update.sh YYYY-MM-DD}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
curl -fsS -o "$TMP/cacert.pem" "https://curl.se/ca/cacert-$DATE.pem"
curl -fsS -o "$TMP/cacert.pem.sha256" "https://curl.se/ca/cacert-$DATE.pem.sha256"
WANT=$(cut -d' ' -f1 "$TMP/cacert.pem.sha256")
GOT=$(sha256sum "$TMP/cacert.pem" | cut -d' ' -f1)
if [ "$WANT" != "$GOT" ]; then
    echo "update.sh: SHA-256 $GOT is not the published $WANT" >&2
    exit 1
fi
cp "$TMP/cacert.pem" "$ROOT/user/roots/public-roots.pem"
echo "public-roots.pem: cacert-$DATE.pem, SHA-256 $GOT, $(grep -c 'BEGIN CERTIFICATE' "$TMP/cacert.pem") certificates"
