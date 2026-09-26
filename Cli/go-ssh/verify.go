package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

func afterWrite(workspace, file string) string {
	ext := filepath.Ext(file)
	run := func(cmd string, args ...string) string {
		c := exec.Command(cmd, args...)
		c.Dir = workspace
		done := make(chan []byte, 1)
		go func() {
			out, _ := c.CombinedOutput()
			done <- out
		}()
		select {
		case out := <-done:
			s := strings.TrimSpace(string(out))
			if len(s) > 4000 {
				s = s[:4000]
			}
			return s
		case <-time.After(8 * time.Second):
			c.Process.Kill()
			return ""
		}
	}
	switch ext {
	case ".rs":
		if _, err := os.Stat(filepath.Join(workspace, "Cargo.toml")); err == nil {
			return run("cargo", "check", "-q")
		}
	case ".ts", ".tsx", ".js":
		if _, err := os.Stat(filepath.Join(workspace, "tsconfig.json")); err == nil {
			return run("tsc", "--noEmit", "--pretty", "false")
		}
	case ".go":
		return run("go", "build", "./...")
	}
	return ""
}
