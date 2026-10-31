// A net/http server, the first web server on gala (ROADMAP N1c). It
// listens on port 7 of every IPv4 address, fetches a page from itself over
// loopback (net/http's client: connect, request, response), prints
// "go_http: listening on port 7", serves one request from outside (the
// go_http scenario's host client), shuts down and prints "go_http: ok".
// Run with the argument "self" it only does the loopback part (on Windows:
// no firewall prompt, no waiting for a client).
package main

import (
	"context"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"time"
)

const page = "hello from gala, a fortress for Windows programs\n"

func check(err error) {
	if err != nil {
		panic(err)
	}
}

func main() {
	self := len(os.Args) > 1 && os.Args[1] == "self"
	addr := "0.0.0.0:7"
	if self {
		addr = "127.0.0.1:7"
	}
	ln, err := net.Listen("tcp4", addr)
	check(err)
	served := make(chan string, 4)
	mux := http.NewServeMux()
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain")
		io.WriteString(w, page)
		served <- r.URL.Path
	})
	srv := &http.Server{Handler: mux}
	go srv.Serve(ln)

	// Loopback: the client side of net/http against our own server.
	resp, err := http.Get("http://127.0.0.1:7/self")
	check(err)
	body, err := io.ReadAll(resp.Body)
	check(err)
	resp.Body.Close()
	if resp.StatusCode != 200 || string(body) != page {
		panic(fmt.Sprintf("loopback GET: %d %q", resp.StatusCode, body))
	}
	if p := <-served; p != "/self" {
		panic("served " + p)
	}
	fmt.Println("go_http: loopback GET ok")

	if !self {
		fmt.Println("go_http: listening on port 7")
		select {
		case p := <-served:
			fmt.Println("go_http: served " + p)
		case <-time.After(60 * time.Second):
			panic("no request from outside")
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	check(srv.Shutdown(ctx))
	fmt.Println("go_http: ok")
}
