#!/bin/sh
# Remakes user/roots/microsoft-roots.pem: the Microsoft roots Windows'
# root store holds for code signing and time stamping, which Mozilla's
# bundle (public-roots.pem) does not have, from Microsoft's PKI
# repository, each checked against its pinned SHA-256 (of the DER):
#
#     sh user/roots/microsoft.sh
#
# Every image ships the file as C:\Windows\System32\gala\roots\
# microsoft-roots.pem beside public-roots.pem (ARCHITECTURE "wintrust:
# Authenticode"); THIRD_PARTY.md lists the certificates.
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
OUT="$TMP/microsoft-roots.pem"
: > "$OUT"
fetch() {
    curl -fsS -o "$TMP/c.crt" "$1"
    GOT=$(sha256sum "$TMP/c.crt" | cut -d' ' -f1)
    if [ "$GOT" != "$2" ]; then
        echo "microsoft.sh: $1: SHA-256 $GOT is not the pinned $2" >&2
        exit 1
    fi
    openssl x509 -inform der -in "$TMP/c.crt" -noout -subject >> "$OUT.names"
    { echo "# $(openssl x509 -inform der -in "$TMP/c.crt" -noout -subject | sed 's/^subject=//')"
      openssl x509 -inform der -in "$TMP/c.crt"; } >> "$OUT"
}
# Microsoft Root Certificate Authority 2010 (Windows' own binaries, the
# time-stamping PCA 2010).
fetch http://www.microsoft.com/pki/certs/MicRooCerAut_2010-06-23.crt \
    df545bf919a2439c36983b54cdfc903dfa4f37d3996d8d84b4c31eec6f3c163e
# Microsoft Root Certificate Authority 2011 (Microsoft's other code
# signing PCAs).
fetch http://www.microsoft.com/pki/certs/MicRooCerAut2011_2011_03_22.crt \
    847df6a78497943f27fc72eb93f9a637320a02b561d0a91b09e87a7807ed7c61
# Microsoft Identity Verification Root Certificate Authority 2020 (Azure
# Trusted Signing's publishers, and its time stamping).
fetch "http://www.microsoft.com/pkiops/certs/Microsoft%20Identity%20Verification%20Root%20Certificate%20Authority%202020.crt" \
    5367f20c7ade0e2bca790915056d086b720c33c1fa2a2661acf787e3292e1270
cp "$OUT" "$ROOT/user/roots/microsoft-roots.pem"
echo "microsoft-roots.pem: $(grep -c 'BEGIN CERTIFICATE' "$OUT") certificates"
