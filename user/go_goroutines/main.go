// Goroutines on several threads: 1000 goroutines with channels, a
// WaitGroup and a mutex, GOMAXPROCS = the CPU count, select with a timeout.
// Prints "go_goroutines: ok" or panics.
package main

import (
	"fmt"
	"runtime"
	"sync"
	"time"
)

func main() {
	if runtime.GOMAXPROCS(0) != runtime.NumCPU() || runtime.NumCPU() < 1 {
		panic(fmt.Sprintf("GOMAXPROCS %d, NumCPU %d", runtime.GOMAXPROCS(0), runtime.NumCPU()))
	}
	var (
		wg      sync.WaitGroup
		mu      sync.Mutex
		counter int
	)
	results := make(chan int, 1000)
	for i := 0; i < 1000; i++ {
		wg.Add(1)
		go func(n int) {
			defer wg.Done()
			mu.Lock()
			counter++
			mu.Unlock()
			results <- n * 2
		}(i)
	}
	wg.Wait()
	close(results)
	sum := 0
	for r := range results {
		sum += r
	}
	if counter != 1000 || sum != 999*1000 {
		panic(fmt.Sprintf("counter %d sum %d", counter, sum))
	}
	// A pipeline of unbuffered channels.
	in := make(chan int)
	out := make(chan int)
	go func() {
		for v := range in {
			out <- v + 1
		}
		close(out)
	}()
	go func() {
		for i := 0; i < 100; i++ {
			in <- i
		}
		close(in)
	}()
	total := 0
	for v := range out {
		total += v
	}
	if total != 5050 {
		panic(fmt.Sprintf("pipeline total %d", total))
	}
	// select with a timeout on a channel nobody sends on.
	never := make(chan int)
	select {
	case <-never:
		panic("received from a silent channel")
	case <-time.After(20 * time.Millisecond):
	}
	fmt.Println("go_goroutines: ok", runtime.NumCPU(), "CPUs")
}
