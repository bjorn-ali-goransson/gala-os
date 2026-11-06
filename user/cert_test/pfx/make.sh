#!/bin/sh
# Makes cert_test's PKCS #12 files (test data, not secret) with the
# host's openssl (3.0 or later, with the legacy provider): run once in
# this directory and check the results in. One self-signed RSA 2048
# certificate, CN=pfx.gala.test, valid 20 years from when it was made;
# its key is kept only inside the PFX files.
#
#   modern.pfx    OpenSSL 3's defaults: PBES2 (PBKDF2 with HMAC-SHA256,
#                 AES-256-CBC) for the certificate and the key,
#                 HMAC-SHA256 MAC; password "gala"
#   legacy.pfx    -legacy: the certificate under pbeWithSHAAnd40BitRC2-CBC,
#                 the key under pbeWithSHAAnd3-KeyTripleDES-CBC, HMAC-SHA1
#                 MAC; friendly name "gala pfx"; password "gala"
#   certonly.pfx  the certificate alone (no key), empty password
set -eu
cd "$(dirname "$0")"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$work/key.pem" \
    -out "$work/cert.pem" -days 7300 -subj "/CN=pfx.gala.test" -set_serial 7
openssl pkcs12 -export -in "$work/cert.pem" -inkey "$work/key.pem" \
    -passout pass:gala -out modern.pfx
openssl pkcs12 -export -legacy -in "$work/cert.pem" -inkey "$work/key.pem" \
    -name "gala pfx" -passout pass:gala -out legacy.pfx
openssl pkcs12 -export -nokeys -in "$work/cert.pem" -passout pass: \
    -out certonly.pfx
