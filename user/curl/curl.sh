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
# verification failure. --ssl-no-revoke (Schannel's revocation check):
# the test certificates name no CRL distribution point, so there is
# nothing to ask.
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

echo "curl.sh: done"
