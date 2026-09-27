package main

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
)

func initProject(workspace string) (string, error) {
	path := filepath.Join(workspace, "AGENTS.md")
	if _, err := os.Stat(path); err == nil {
		return "AGENTS.md already exists", nil
	} else if !os.IsNotExist(err) {
		return "", err
	}
	var commands []string
	if _, err := os.Stat(filepath.Join(workspace, "package.json")); err == nil {
		commands = append(commands, "Inspect package.json scripts for build and test commands.")
	}
	if _, err := os.Stat(filepath.Join(workspace, "Cargo.toml")); err == nil {
		commands = append(commands, "`cargo build` and `cargo test`.")
	}
	if _, err := os.Stat(filepath.Join(workspace, "go.mod")); err == nil {
		commands = append(commands, "`go build ./...` and `go test ./...`.")
	}
	if _, err := os.Stat(filepath.Join(workspace, "Makefile")); err == nil {
		commands = append(commands, "Inspect Makefile targets for build and test commands.")
	}
	if len(commands) == 0 {
		commands = append(commands, "Document the repository's build and test commands.")
	}
	claude := ""
	if _, err := os.Stat(filepath.Join(workspace, "CLAUDE.md")); err == nil {
		claude = "\n\nSee `CLAUDE.md` for additional repository instructions."
	}
	body := fmt.Sprintf("# Agent instructions\n\n## Project summary\n\n<!-- Describe this project's purpose and key directories. -->\n\n## Build and test\n\n- %s%s\n", joinLines(commands), claude)
	return "created AGENTS.md", os.WriteFile(path, []byte(body), 0o644)
}

func joinLines(lines []string) string {
	out := ""
	for i, line := range lines {
		if i > 0 {
			out += "\n- "
		}
		out += line
	}
	return out
}

func readPackageScripts(path string) map[string]string {
	var pkg struct {
		Scripts map[string]string `json:"scripts"`
	}
	raw, err := os.ReadFile(path)
	if err != nil || json.Unmarshal(raw, &pkg) != nil {
		return nil
	}
	return pkg.Scripts
}
