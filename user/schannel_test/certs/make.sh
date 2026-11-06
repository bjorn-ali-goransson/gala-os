#!/bin/sh
# Makes schannel_test's certificates (test data, not secret) with the
# host's openssl (3.0 or later): run once in this directory and check
# the results in. Regenerating changes every file, the test's expected
# subjects do not change.
#
#   ca.crt/.key, ca.cer        "gala schannel test CA" (ECDSA P-256), the
#                              root gala's machine root store trusts in the
#                              scenario (C:\Windows\System32\gala\roots)
#   server.crt/.key            CN=localhost, DNS:localhost, IP:127.0.0.1,
#                              serverAuth, issued by the CA (nginx, port 8443)
#   expired.crt/.key           the same names, valid 2020-01-01..2021-01-01
#                              (port 8445)
#   untrusted.crt/.key         CN=localhost, self-signed (port 8444)
#   server-rsa.crt/.key, .cer  CN=localhost, RSA 2048, issued by the CA:
#                              the test's own server (AcceptSecurityContext)
#   server-ec384.cer, .p8      CN=localhost, ECDSA P-384, issued by the CA:
#                              the test's own server with an ECDSA key
#   client.crt/.key, .cer, .p8 CN=gala schannel test client, ECDSA P-256,
#                              clientAuth, issued by the CA: the client
#                              certificate (nginx's ssl_verify_client, port
#                              8447, trusts ca.crt)
#   revoked.crt/.key           CN=localhost, as server.crt, serial 7,
#                              naming the CRL distribution point
#                              http://127.0.0.1:8080/ca.crl; revoked
#                              (keyCompromise) by ca.crl (port 8448)
#   notrevoked.crt/.key        the same, serial 8, not revoked (port 8449)
#   ca.crl                     the CA's CRL (DER; v2, valid 2025..2125)
#                              listing revoked.crt: in gala's CA store in
#                              the scenarios (System32\gala\ca) and served
#                              by Nginx at the distribution point
#
# `sh make.sh crl` makes only the last three with the CA that is there,
# leaving the other files as they are.
set -eu
cd "$(dirname "$0")"
days=36500
ext=ext.cnf
cat > "$ext" <<'EOF'
[leaf]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature, keyEncipherment
extendedKeyUsage = serverAuth
subjectAltName = DNS:localhost, IP:127.0.0.1
[client]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature
extendedKeyUsage = clientAuth
[ca]
basicConstraints = critical, CA:TRUE
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
[crldp]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature, keyEncipherment
extendedKeyUsage = serverAuth
subjectAltName = DNS:localhost, IP:127.0.0.1
crlDistributionPoints = URI:http://127.0.0.1:8080/ca.crl
EOF
ec() { openssl ecparam -name prime256v1 -genkey -noout | openssl pkcs8 -topk8 -nocrypt -out "$1"; }
# The revocation set: two leaves naming the CA's CRL, one of them revoked
# in it.
revocation() {
    rm -rf crl.db && mkdir crl.db && : > crl.db/index.txt && echo 01 > crl.db/crlnumber
    cat > crl.cnf <<'X'
[ca]
default_ca = test
[test]
dir = crl.db
database = crl.db/index.txt
new_certs_dir = crl.db
serial = crl.db/serial
crlnumber = crl.db/crlnumber
default_md = sha256
policy = any
copy_extensions = none
unique_subject = no
[any]
commonName = supplied
[crl]
authorityKeyIdentifier = keyid
X
    for n in revoked:07 notrevoked:08; do
        name=${n%%:*}
        ec $name.key
        openssl req -new -key $name.key -subj "/CN=localhost" -out $name.csr
        echo "${n#*:}" > crl.db/serial
        openssl ca -batch -config crl.cnf -cert ca.crt -keyfile ca.key -in $name.csr \
            -startdate 20250101000000Z -enddate 21250101000000Z -notext \
            -extfile "$ext" -extensions crldp -out $name.crt
    done
    openssl ca -config crl.cnf -cert ca.crt -keyfile ca.key -revoke revoked.crt \
        -crl_reason keyCompromise
    openssl ca -gencrl -config crl.cnf -cert ca.crt -keyfile ca.key -crlexts crl \
        -crl_lastupdate 20250101000000Z -crl_nextupdate 21250101000000Z -out ca.crl.pem
    openssl crl -in ca.crl.pem -outform der -out ca.crl
    rm -rf crl.db crl.cnf ca.crl.pem revoked.csr notrevoked.csr
}
if [ "${1:-}" = crl ]; then
    revocation
    rm -f "$ext"
    exit 0
fi
ec ca.key
openssl req -x509 -new -key ca.key -subj "/CN=gala schannel test CA" -days $days \
    -extensions ca -config "$ext" -out ca.crt
for name in server expired; do
    ec $name.key
    openssl req -new -key $name.key -subj "/CN=localhost" -out $name.csr
done
openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -set_serial 2 -days $days \
    -extfile "$ext" -extensions leaf -out server.crt
# openssl 3.0's x509 cannot set a past validity: `openssl ca` can.
mkdir -p ca.db && : > ca.db/index.txt
cat > ca.cnf <<'EOF'
[ca]
default_ca = test
[test]
dir = ca.db
database = ca.db/index.txt
new_certs_dir = ca.db
serial = ca.db/serial
default_md = sha256
policy = any
copy_extensions = none
[any]
commonName = supplied
EOF
echo 03 > ca.db/serial
openssl ca -batch -config ca.cnf -cert ca.crt -keyfile ca.key -in expired.csr \
    -startdate 20200101000000Z -enddate 20210101000000Z -notext \
    -extfile "$ext" -extensions leaf -out expired.crt
ec untrusted.key
openssl req -x509 -new -key untrusted.key -subj "/CN=localhost" -days $days \
    -extensions leaf -config "$ext" -out untrusted.crt
openssl genrsa -out server-rsa.pem 2048
openssl pkcs8 -topk8 -nocrypt -in server-rsa.pem -out server-rsa.key
openssl req -new -key server-rsa.key -subj "/CN=localhost" -out server-rsa.csr
openssl x509 -req -in server-rsa.csr -CA ca.crt -CAkey ca.key -set_serial 4 -days $days \
    -extfile "$ext" -extensions leaf -out server-rsa.crt
openssl ecparam -name secp384r1 -genkey -noout | openssl pkcs8 -topk8 -nocrypt -out server-ec384.key
openssl req -new -key server-ec384.key -subj "/CN=localhost" -out server-ec384.csr
openssl x509 -req -in server-ec384.csr -CA ca.crt -CAkey ca.key -set_serial 5 -days $days \
    -extfile "$ext" -extensions leaf -out server-ec384.crt
ec client.key
openssl req -new -key client.key -subj "/CN=gala schannel test client" -out client.csr
openssl x509 -req -in client.csr -CA ca.crt -CAkey ca.key -set_serial 6 -days $days \
    -extfile "$ext" -extensions client -out client.crt
openssl x509 -in ca.crt -outform der -out ca.cer
openssl x509 -in server-rsa.crt -outform der -out server-rsa.cer
openssl pkcs8 -topk8 -nocrypt -in server-rsa.key -outform der -out server-rsa.p8
for name in server-ec384 client; do
    openssl x509 -in $name.crt -outform der -out $name.cer
    openssl pkcs8 -topk8 -nocrypt -in $name.key -outform der -out $name.p8
done
revocation
rm -rf ca.db ca.cnf "$ext" ./*.csr server-rsa.pem server-ec384.crt server-ec384.key
