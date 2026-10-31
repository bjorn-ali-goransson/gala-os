// os/exec with pipes, as real tools use it: the program runs itself as a
// child (`go_exec <mode>`) and checks
//   - Output: the child's stdout,
//   - CombinedOutput: stdout and stderr together,
//   - an exit code through *exec.ExitError,
//   - streaming through a cat-like child (StdinPipe/StdoutPipe): 64 KiB
//     written in pieces come back unchanged.
// Prints "go_exec: ok" or panics.
package main

import (
	"bytes"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
)

func check(err error) {
	if err != nil {
		panic(err)
	}
}

func child(mode string) {
	switch mode {
	case "hello":
		fmt.Print("hello from the child\n")
	case "both":
		fmt.Fprint(os.Stdout, "out\n")
		fmt.Fprint(os.Stderr, "err\n")
	case "exit":
		os.Exit(7)
	case "cat":
		if _, err := io.Copy(os.Stdout, os.Stdin); err != nil {
			os.Exit(2)
		}
	default:
		os.Exit(3)
	}
	os.Exit(0)
}

func main() {
	if len(os.Args) == 2 {
		child(os.Args[1])
	}
	self, err := os.Executable()
	check(err)

	out, err := exec.Command(self, "hello").Output()
	check(err)
	if string(out) != "hello from the child\n" {
		panic(fmt.Sprintf("Output: %q", out))
	}

	out, err = exec.Command(self, "both").CombinedOutput()
	check(err)
	if string(out) != "out\nerr\n" {
		panic(fmt.Sprintf("CombinedOutput: %q", out))
	}

	err = exec.Command(self, "exit").Run()
	var exit *exec.ExitError
	if !errors.As(err, &exit) || exit.ExitCode() != 7 {
		panic(fmt.Sprintf("exit code: %v", err))
	}

	cmd := exec.Command(self, "cat")
	stdin, err := cmd.StdinPipe()
	check(err)
	stdout, err := cmd.StdoutPipe()
	check(err)
	check(cmd.Start())
	data := make([]byte, 64<<10)
	for i := range data {
		data[i] = byte(i * 7)
	}
	go func() {
		for i := 0; i < len(data); i += 1000 {
			end := min(i+1000, len(data))
			if _, err := stdin.Write(data[i:end]); err != nil {
				panic(err)
			}
		}
		stdin.Close()
	}()
	back, err := io.ReadAll(stdout)
	check(err)
	check(cmd.Wait())
	if !bytes.Equal(back, data) {
		panic(fmt.Sprintf("cat: %d bytes back of %d", len(back), len(data)))
	}
	fmt.Println("go_exec: ok")
}
