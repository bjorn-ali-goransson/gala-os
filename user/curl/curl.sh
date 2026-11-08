# curl for Windows, unmodified, against schannel_test's Nginx
# (user/schannel_test/nginx.conf: 8080 plain HTTP, 8443 the test CA's
# localhost certificate, 8444 self-signed, 8445 expired), which the
# scenario starts with `start=`. Run by busybox sh in the `curl` scenario
# (curl.se's build: LibreSSL, with the Windows ROOT store as its CA list)
# and `curl_schannel` (Git for Windows' build: Schannel), with C:\curl
# holding that curl. Both validate through the machine's root store
# (gala: System32\gala\roots, where the scenario puts the test CA;
# Windows: LocalMachine\Root), since no --cacert is given.
#
# Each line names its step and gives curl's exit code: 0, 60
# (CURLE_PEER_FAILED_VERIFICATION) for an untrusted root, and for an
# expired certificate 60 on LibreSSL, 35 (CURLE_SSL_CONNECT_ERROR) on
# Schannel, whose SEC_E_CERT_EXPIRED curl does not count as a
# verification failure. --ssl-no-revoke (no revocation check by
# Schannel) in those steps: their certificates name no CRL distribution
# point, so Windows has nothing to check them by (Schannel would refuse
# them with CRYPT_E_NO_REVOCATION_CHECK).
#
# Revocation: 8448 and 8449 serve certificates naming the test CA's CRL
# (http://127.0.0.1:8080/ca.crl), which lists 8448's. Without
# --ssl-no-revoke Schannel's curl asks for the chain's revocation
# (SCH_CRED_REVOCATION_CHECK_CHAIN): 8449 passes, 8448 fails with 35
# (CRYPT_E_REVOKED, which curl does not count as a verification
# failure); with it 8448 passes. gala checks offline, with the CRL in
# its CA store (System32\gala\ca); Windows fetches it from Nginx.
# LibreSSL's curl does not check revocation here: 0 for all three.
#
# Then with --cacert (C:\gala\ca.crt, the test CA in PEM; the CA file is
# the only trust, as documented for both backends): Schannel's curl
# checks the chain itself, through crypt32's chain engine with the file
# as an exclusive root store (SCH_CRED_MANUAL_CRED_VALIDATION). The test
# CA's certificate passes; a self-signed one, a CA file that does not
# hold the issuer (C:\gala\untrusted.crt, the self-signed certificate)
# and the expired certificate give 60.
#
# Session resumption: two requests in one run, each on its own
# connection (Connection: close), to /session, whose body is Nginx's
# $ssl_session_reused ("." for a full handshake, "r" for a resumed one).
# Schannel's curl keeps its credential handle for the second connection
# (curl's session cache), and Schannel resumes the session cached under
# it for that target name: TLS 1.3, then TLS 1.2 (--tls-max 1.2: Nginx's
# TLS 1.2 session tickets). Checked in curl_schannel only: curl.se's
# LibreSSL build resumes neither here (". ." twice), which is that
# library's and curl's business, not gala's TLS (not compared with
# Windows yet).
curl=C:/curl/curl.exe

echo "version: $($curl -V | head -n 1)"

# Nginx is started with the boot: wait for its port.
out=$($curl -sS --retry 30 --retry-delay 1 --retry-connrefused http://127.0.0.1:8080/)
echo "http: exit $? $(echo "$out" | grep -o '<title>[^<]*</title>')"

out=$($curl -sS --ssl-no-revoke https://localhost:8443/)
echo "https: exit $? $(echo "$out" | grep -o '<title>[^<]*</title>')"

out=$($curl -sS --ssl-no-revoke -o C:/Users/Public/body.html -w '%{http_code} %{size_download}' https://127.0.0.1:8443/index.html)
echo "https ip: exit $? $out"

$curl -sS --ssl-no-revoke https://localhost:8444/ >/dev/null
echo "untrusted: exit $?"

$curl -sS --ssl-no-revoke https://localhost:8445/ >/dev/null
echo "expired: exit $?"

out=$($curl -sS --ssl-no-revoke --cacert C:/gala/ca.crt https://localhost:8443/)
echo "cacert: exit $? $(echo "$out" | grep -o '<title>[^<]*</title>')"

$curl -sS --ssl-no-revoke --cacert C:/gala/ca.crt https://localhost:8444/ >/dev/null
echo "cacert untrusted: exit $?"

$curl -sS --ssl-no-revoke --cacert C:/gala/untrusted.crt https://localhost:8443/ >/dev/null
echo "cacert other: exit $?"

$curl -sS --ssl-no-revoke --cacert C:/gala/ca.crt https://localhost:8445/ >/dev/null
echo "cacert expired: exit $?"

out=$($curl -sS https://localhost:8449/)
echo "revocation: exit $? $(echo "$out" | grep -o '<title>[^<]*</title>')"

$curl -sS https://localhost:8448/ >/dev/null
echo "revoked: exit $?"

$curl -sS --ssl-no-revoke https://localhost:8448/ >/dev/null
echo "revoked no-revoke: exit $?"

out=$($curl -sS --ssl-no-revoke -H 'Connection: close' https://localhost:8443/session https://localhost:8443/session)
echo "resumption: exit $? $(echo $out)"

out=$($curl -sS --ssl-no-revoke --tls-max 1.2 -H 'Connection: close' https://localhost:8443/session https://localhost:8443/session)
echo "resumption tls1.2: exit $? $(echo $out)"

echo "curl.sh: done"
