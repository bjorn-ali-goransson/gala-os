#!/bin/sh
# Makes cert_test's chain certificates (test data, not secret) with the
# host's openssl (3.0 or later): run once in this directory and check
# the DER results in. Every certificate is valid 2025-01-01..2045-01-01
# (`openssl ca` sets the dates), so the test's times are fixed. The keys
# are not kept: nothing signs with them again.
#
#   root.cer        "gala chain test root", RSA 2048, sha256WithRSA,
#                   self-signed, CA (no authority key identifier)
#   inter.cer       "gala chain test intermediate", ECDSA P-384, CA,
#                   issued by root (sha256WithRSA)
#   leaf.cer        CN=chain.test, ECDSA P-256, issued by inter
#                   (ecdsa-with-SHA384): DNS:www.chain.test,
#                   DNS:*.wild.chain.test, IP:10.0.0.1, serverAuth
#   client.cer      CN=client.chain.test, issued by inter, clientAuth only
#   notca.cer       CN=not a CA, issued by inter, CA:FALSE
#   underling.cer   CN=underling.chain.test, issued by notca (whose
#                   basic constraints forbid it)
set -eu
cd "$(dirname "$0")"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cat > "$work/ext.cnf" <<'X'
[root]
basicConstraints = critical, CA:TRUE
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
[inter]
basicConstraints = critical, CA:TRUE
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
[leaf]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature
extendedKeyUsage = serverAuth
subjectAltName = DNS:www.chain.test, DNS:*.wild.chain.test, IP:10.0.0.1
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
[client]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature
extendedKeyUsage = clientAuth
subjectAltName = DNS:client.chain.test
authorityKeyIdentifier = keyid
[notca]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
[underling]
basicConstraints = critical, CA:FALSE
subjectAltName = DNS:underling.chain.test
authorityKeyIdentifier = keyid
X
# sign <name> <issuer> <section> <digest>: name.csr signed by issuer.
sign() {
    mkdir -p "$work/db.$1" && : > "$work/db.$1/index.txt"
    cat > "$work/ca.cnf" <<X
[ca]
default_ca = test
[test]
dir = $work/db.$1
database = $work/db.$1/index.txt
new_certs_dir = $work/db.$1
serial = $work/db.$1/serial
default_md = $4
policy = any
copy_extensions = none
unique_subject = no
[any]
commonName = supplied
X
    echo "$5" > "$work/db.$1/serial"
    if [ "$2" = self ]; then
        openssl ca -batch -selfsign -config "$work/ca.cnf" -keyfile "$work/$1.key" \
            -in "$work/$1.csr" -startdate 20250101000000Z -enddate 20450101000000Z -notext \
            -extfile "$work/ext.cnf" -extensions "$3" -out "$work/$1.crt" 2>/dev/null
    else
        openssl ca -batch -config "$work/ca.cnf" -cert "$work/$2.crt" -keyfile "$work/$2.key" \
            -in "$work/$1.csr" -startdate 20250101000000Z -enddate 20450101000000Z -notext \
            -extfile "$work/ext.cnf" -extensions "$3" -out "$work/$1.crt" 2>/dev/null
    fi
    openssl x509 -in "$work/$1.crt" -outform der -out "$1.cer"
}
csr() { openssl req -new -key "$work/$1.key" -subj "/CN=$2" -out "$work/$1.csr"; }
openssl genrsa -out "$work/root.key" 2048 2>/dev/null
csr root "gala chain test root"
sign root self root sha256 10
openssl ecparam -name secp384r1 -genkey -noout -out "$work/inter.key"
csr inter "gala chain test intermediate"
sign inter root inter sha256 11
for n in leaf:chain.test client:client.chain.test notca:"not a CA"; do
    openssl ecparam -name prime256v1 -genkey -noout -out "$work/${n%%:*}.key"
    csr "${n%%:*}" "${n#*:}"
done
sign leaf inter leaf sha384 12
sign client inter client sha384 13
sign notca inter notca sha384 14
openssl ecparam -name prime256v1 -genkey -noout -out "$work/underling.key"
csr underling underling.chain.test
sign underling notca underling sha256 15
