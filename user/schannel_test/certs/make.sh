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
[ca]
basicConstraints = critical, CA:TRUE
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
EOF
ec() { openssl ecparam -name prime256v1 -genkey -noout | openssl pkcs8 -topk8 -nocrypt -out "$1"; }
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
openssl x509 -in ca.crt -outform der -out ca.cer
openssl x509 -in server-rsa.crt -outform der -out server-rsa.cer
openssl pkcs8 -topk8 -nocrypt -in server-rsa.key -outform der -out server-rsa.p8
rm -rf ca.db ca.cnf "$ext" ./*.csr server-rsa.pem
