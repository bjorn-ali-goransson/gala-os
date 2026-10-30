// A real Go program for gala (ROADMAP G2): the standard Go runtime and fmt,
// cross-compiled for windows/amd64 with the pinned toolchain by
// `cargo xtask` (see GO_PROGRAMS in xtask/src/main.rs).
package main

import "fmt"

func main() {
	fmt.Println("Hello from Go on gala!")
}
