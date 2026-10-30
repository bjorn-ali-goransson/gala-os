// os.Exit's code reaches the process exit status (the scenario expects 3).
package main

import (
	"fmt"
	"os"
)

func main() {
	fmt.Println("go_exit: exiting with 3")
	os.Exit(3)
}
