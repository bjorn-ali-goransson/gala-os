// The os package: arguments, environment, files and directories in the
// temporary directory, the working directory. Prints "go_os: ok" or
// panics.
package main

import (
	"fmt"
	"os"
	"path/filepath"
)

func check(err error) {
	if err != nil {
		panic(err)
	}
}

func main() {
	if len(os.Args) < 1 {
		panic("no os.Args")
	}
	check(os.Setenv("GALA_TEST", "qala"))
	if v := os.Getenv("GALA_TEST"); v != "qala" {
		panic("Getenv after Setenv: " + v)
	}
	if os.Getenv("SystemRoot") == "" {
		panic("no SystemRoot in the environment")
	}
	wd, err := os.Getwd()
	check(err)
	dir, err := os.MkdirTemp(os.TempDir(), "gala")
	check(err)
	path := filepath.Join(dir, "hello.txt")
	check(os.WriteFile(path, []byte("hello, gala"), 0o644))
	data, err := os.ReadFile(path)
	check(err)
	if string(data) != "hello, gala" {
		panic("read back " + string(data))
	}
	info, err := os.Stat(path)
	check(err)
	if info.Size() != 11 || info.IsDir() {
		panic(fmt.Sprintf("stat: size %d dir %v", info.Size(), info.IsDir()))
	}
	entries, err := os.ReadDir(dir)
	check(err)
	if len(entries) != 1 || entries[0].Name() != "hello.txt" {
		panic(fmt.Sprintf("ReadDir: %v", entries))
	}
	check(os.Remove(path))
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		panic(fmt.Sprintf("after Remove: %v", err))
	}
	check(os.Remove(dir))
	fmt.Println("go_os: ok", wd)
}
