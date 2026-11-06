#!/bin/sh
# Makes cert_test's chain certificates (test data, not secret) with the
# host's openssl (3.0 or later): run once in this directory and check
# the DER results in. Every certificate is valid 2025-01-01..2045-01-01
# (`openssl ca` sets the dates), so the test's times are fixed. The keys
# are not kept: nothing signs with them again (a new CRL means running
# it all again).
#
#   root.cer        "gala chain test root", RSA 2048, sha256WithRSA,
#                   self-signed, CA (no authority key identifier)
#   inter.cer       "gala chain test intermediate", ECDSA P-384, CA,
#                   issued by root (sha256WithRSA), naming root's CRL
#                   (http://crl.chain.test/root.crl)
#   leaf.cer        CN=chain.test, ECDSA P-256, issued by inter
#                   (ecdsa-with-SHA384): DNS:www.chain.test,
#                   DNS:*.wild.chain.test, IP:10.0.0.1, serverAuth,
#                   naming inter's CRL (http://crl.chain.test/inter.crl)
#   client.cer      CN=client.chain.test, issued by inter, clientAuth
#                   only, naming no CRL
#   notca.cer       CN=not a CA, issued by inter, CA:FALSE
#   underling.cer   CN=underling.chain.test, issued by notca (whose
#                   basic constraints forbid it)
#   revoked.cer     CN=revoked.chain.test, issued by inter (serial 0x16),
#                   naming inter's CRL, revoked by it
#   (crl.chain.test does not resolve: a CRL is what the test gives.)
#   inter.crl       inter's CRL (v2: authority key identifier, CRL
#                   number): revoked.cer, keyCompromise; 2025..2045
#   inter-old.crl   inter's CRL of 2020..2021 (out of date), empty
#   root.crl        root's CRL, empty (inter is not revoked); 2025..2045
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
crlDistributionPoints = URI:http://crl.chain.test/root.crl
[leaf]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature
extendedKeyUsage = serverAuth
subjectAltName = DNS:www.chain.test, DNS:*.wild.chain.test, IP:10.0.0.1
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
crlDistributionPoints = URI:http://crl.chain.test/inter.crl
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
[revoked]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature
extendedKeyUsage = serverAuth
subjectAltName = DNS:revoked.chain.test
authorityKeyIdentifier = keyid
crlDistributionPoints = URI:http://crl.chain.test/inter.crl
X
# config <issuer> <digest>: $work/ca.cnf for the issuer's database (one
# per issuer, so its CRLs list what it revoked).
config() {
    db="$work/db.$1"
    [ -d "$db" ] || { mkdir -p "$db" && : > "$db/index.txt" && echo 01 > "$db/crlnumber"; }
    cat > "$work/ca.cnf" <<X
[ca]
default_ca = test
[test]
dir = $db
database = $db/index.txt
new_certs_dir = $db
serial = $db/serial
crlnumber = $db/crlnumber
default_md = $2
default_crl_days = 30
policy = any
copy_extensions = none
unique_subject = no
[any]
commonName = supplied
[crl]
authorityKeyIdentifier = keyid
X
}
# sign <name> <issuer> <section> <digest> <serial>: name.csr signed by
# issuer.
sign() {
    if [ "$2" = self ]; then config "$1" "$4"; else config "$2" "$4"; fi
    echo "$5" > "$db/serial"
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
openssl ecparam -name prime256v1 -genkey -noout -out "$work/revoked.key"
csr revoked revoked.chain.test
sign revoked inter revoked sha384 16
# crl <issuer> <out> <lastUpdate> <nextUpdate>: the issuer's CRL.
crl() {
    config "$1" sha256
    openssl ca -gencrl -config "$work/ca.cnf" -cert "$work/$1.crt" -keyfile "$work/$1.key" \
        -crl_lastupdate "$3" -crl_nextupdate "$4" -crlexts crl -out "$work/$2.pem" 2>/dev/null
    openssl crl -in "$work/$2.pem" -outform der -out "$2"
}
crl inter inter-old.crl 20200101000000Z 20210101000000Z
config inter sha256
openssl ca -config "$work/ca.cnf" -cert "$work/inter.crt" -keyfile "$work/inter.key" \
    -revoke "$work/revoked.crt" -crl_reason keyCompromise 2>/dev/null
crl inter inter.crl 20250101000000Z 20450101000000Z
crl root root.crl 20250101000000Z 20450101000000Z
