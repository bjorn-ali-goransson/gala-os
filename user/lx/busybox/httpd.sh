# busybox httpd and wget over loopback on gala's Linux personality
# (ROADMAP H11, LX): `busybox sh /lx/httpd.sh` with PATH=/lx. httpd serves
# a directory on 127.0.0.1:8080; wget fetches files from it and they are
# compared byte for byte. One labelled line per check; the transcript is
# lx_httpd.txt.
cd /lx || exit 1
busybox mkdir www && cd www || exit 2
echo "hello from httpd" > index.html
busybox seq 1 20000 > big.txt
cd ..
busybox httpd -f -p 127.0.0.1:8080 -h www &
server=$!
# Ready once a request is answered (the server binds after it starts).
until busybox wget -q -O /dev/null http://127.0.0.1:8080/index.html 2>/dev/null; do
	busybox usleep 20000
done
echo "server answers: yes"
busybox wget -q -O got.html http://127.0.0.1:8080/index.html
echo "wget index.html: status $?"
busybox cmp www/index.html got.html && echo "index.html the same: yes"
busybox wget -q -O got.txt http://127.0.0.1:8080/big.txt
echo "wget big.txt: status $?"
busybox cmp www/big.txt got.txt && echo "big.txt the same: yes ($(busybox wc -c < got.txt) bytes)"
echo "wget -O - to a pipe: $(busybox wget -q -O - http://127.0.0.1:8080/index.html | busybox tr a-z A-Z)"
busybox wget -q -O missing.html http://127.0.0.1:8080/missing.html 2>/dev/null
echo "wget missing.html: status $?"
busybox kill $server
wait $server
echo "server ended by SIGTERM: status $?"
busybox rm -r www got.html got.txt
echo "httpd.sh: ok"
