//go:build !windows

package main

func keyringEnabled() bool             { return false }
func wincredRead(string) string        { return "" }
func wincredWrite(string, string) bool { return false }
func wincredRemove(string)             {}
