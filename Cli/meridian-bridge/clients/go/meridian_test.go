package meridian

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func openTemp(t *testing.T) *Bridge {
	t.Helper()
	b, err := Open("", filepath.Join(t.TempDir(), "vault"))
	if err != nil {
		t.Fatalf("open: %v", err)
	}
	t.Cleanup(b.Close)
	return b
}

func TestRoundtrip(t *testing.T) {
	b := openTemp(t)
	id, err := b.MemoryWrite("decision", "go client roundtrip works", "repo:go", "gotest/1")
	if err != nil {
		t.Fatalf("write: %v", err)
	}
	if id == "" {
		t.Fatal("empty id")
	}
	hits, err := b.MemorySearch("roundtrip works", 3, "", nil)
	if err != nil {
		t.Fatalf("search: %v", err)
	}
	if len(hits) == 0 || hits[0].Text != "go client roundtrip works" {
		t.Fatalf("unexpected hits: %+v", hits)
	}

	dir := t.TempDir()
	src := filepath.Join(dir, "f.go")
	if err := os.WriteFile(src, []byte("package main\n\nfunc main() {}\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	sum, err := b.CacheRead(src, 200)
	if err != nil {
		t.Fatalf("cache.read: %v", err)
	}
	if !strings.Contains(sum, "language: go") {
		t.Fatalf("summary missing language: %s", sum)
	}

	ctx, err := b.BootContext(dir, "zagy")
	if err != nil {
		t.Fatalf("boot.context: %v", err)
	}
	if ctx.Tokens > 2000 {
		t.Fatalf("boot tokens %d > 2000", ctx.Tokens)
	}
	if !strings.Contains(ctx.Text, "Cosmos") {
		t.Fatal("boot context missing system snippet")
	}
}

func TestSecretRejected(t *testing.T) {
	b := openTemp(t)
	_, err := b.MemoryWrite("decision", "key sk-abcdef123456", "repo:go", "gotest/1")
	if err == nil || !strings.Contains(err.Error(), "rejected") {
		t.Fatalf("expected rejection, got %v", err)
	}
}
