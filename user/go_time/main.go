// Time: the clock advances, Sleep is never early, tickers and AfterFunc
// fire. Prints "go_time: ok" or panics.
package main

import (
	"fmt"
	"time"
)

func main() {
	t0 := time.Now()
	if t0.Year() < 2024 {
		panic(fmt.Sprintf("implausible wall clock %v", t0))
	}
	time.Sleep(50 * time.Millisecond)
	if d := time.Since(t0); d < 50*time.Millisecond || d > 5*time.Second {
		panic(fmt.Sprintf("Sleep(50ms) took %v", d))
	}
	ticker := time.NewTicker(10 * time.Millisecond)
	for i := 0; i < 5; i++ {
		<-ticker.C
	}
	ticker.Stop()
	fired := make(chan struct{})
	time.AfterFunc(10*time.Millisecond, func() { close(fired) })
	select {
	case <-fired:
	case <-time.After(5 * time.Second):
		panic("AfterFunc did not fire")
	}
	timer := time.NewTimer(time.Hour)
	if !timer.Stop() {
		panic("Stop of a pending timer returned false")
	}
	fmt.Println("go_time: ok")
}
