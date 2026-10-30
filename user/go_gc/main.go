// The garbage collector: ~200 MB allocated in short-lived chunks, forced
// collections, bounded heap. Prints "go_gc: ok" or panics.
package main

import (
	"fmt"
	"runtime"
)

var sink []byte

func main() {
	for i := 0; i < 200; i++ {
		b := make([]byte, 1<<20)
		b[i] = byte(i)
		sink = b
	}
	runtime.GC()
	var m runtime.MemStats
	runtime.ReadMemStats(&m)
	if m.NumGC == 0 {
		panic("no collection ran")
	}
	if m.HeapAlloc > 64<<20 {
		panic(fmt.Sprintf("heap still %d bytes after GC", m.HeapAlloc))
	}
	// Many small objects with pointers.
	type node struct{ next *node; v [8]int }
	var head *node
	for i := 0; i < 100000; i++ {
		head = &node{next: head}
		if i%1000 == 0 {
			head = nil
		}
	}
	runtime.GC()
	fmt.Println("go_gc: ok", m.NumGC, "collections")
}
