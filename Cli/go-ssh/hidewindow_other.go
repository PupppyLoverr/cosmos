//go:build !windows

package main

import "os/exec"

func sysHideWindow(c *exec.Cmd) {}
