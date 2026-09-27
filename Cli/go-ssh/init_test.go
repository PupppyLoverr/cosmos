package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestInitProjectTemplateAndNoop(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "go.mod"), []byte("module example\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "CLAUDE.md"), []byte("instructions\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	msg, err := initProject(dir)
	if err != nil || msg != "created AGENTS.md" {
		t.Fatalf("init: %q %v", msg, err)
	}
	body, _ := os.ReadFile(filepath.Join(dir, "AGENTS.md"))
	if !strings.Contains(string(body), "Project summary") || !strings.Contains(string(body), "CLAUDE.md") {
		t.Fatalf("template missing expected sections: %s", body)
	}
	msg, err = initProject(dir)
	if err != nil || msg != "AGENTS.md already exists" {
		t.Fatalf("noop: %q %v", msg, err)
	}
}

func TestCostFormatter(t *testing.T) {
	text := formatCostReport(100, 20, 30, 4, 0.15, map[string]float64{"alpha": 0.12, "beta": 0.03}, 2, 61*time.Second)
	for _, want := range []string{"Input: 100", "USD total: $0.1500", "alpha: $0.1200", "Turns: 2", "Wall time: 1.0m"} {
		if !strings.Contains(text, want) {
			t.Errorf("missing %q in %s", want, text)
		}
	}
}
