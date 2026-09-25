package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestReadHashlines(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "a.txt"), []byte("alpha\nbeta\ngamma\n"), 0o644)
	ctx := NewToolCtx(dir)
	out := runTool(ctx, "read", map[string]any{"path": "a.txt"})
	if !strings.Contains(out, "file_sha256=") || !strings.Contains(out, "lines=3") {
		t.Fatalf("missing header: %s", out)
	}
	if !strings.Contains(out, "|alpha") {
		t.Fatalf("missing anchor: %s", out)
	}
}

func TestWriteReadEdit(t *testing.T) {
	dir := t.TempDir()
	ctx := NewToolCtx(dir)
	runTool(ctx, "write", map[string]any{"path": "b.txt", "content": "one\ntwo\nthree\n"})
	r := runTool(ctx, "read", map[string]any{"path": "b.txt"})
	var anchor string
	for _, l := range strings.Split(r, "\n") {
		if strings.Contains(l, "|two") {
			anchor = strings.Split(l, "|")[0]
		}
	}
	e := runTool(ctx, "edit", map[string]any{"path": "b.txt", "edits": []any{
		map[string]any{"start": anchor, "end": anchor, "new_text": "TWO"}}})
	if !strings.HasPrefix(e, "ok") {
		t.Fatalf("edit: %s", e)
	}
	body, _ := os.ReadFile(filepath.Join(dir, "b.txt"))
	if string(body) != "one\nTWO\nthree\n" {
		t.Fatalf("got %q", body)
	}
}

func TestStaleAndEscape(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "c.txt"), []byte("x\ny\n"), 0o644)
	ctx := NewToolCtx(dir)
	e := runTool(ctx, "edit", map[string]any{"path": "c.txt", "file_sha256": "deadbeef",
		"edits": []any{map[string]any{"start": "1:aaaaaa", "end": "1:aaaaaa", "new_text": "z"}}})
	if !strings.Contains(e, "STALE") {
		t.Fatalf("expected STALE: %s", e)
	}
	e2 := runTool(ctx, "write", map[string]any{"path": "c.txt", "content": "clobber"})
	if !strings.Contains(e2, "STALE") {
		t.Fatalf("expected STALE: %s", e2)
	}
	esc := runTool(ctx, "read", map[string]any{"path": "../outside.txt"})
	if !strings.Contains(esc, "escapes workspace") {
		t.Fatalf("expected escape denial: %s", esc)
	}
}

func TestSessionRoundtrip(t *testing.T) {
	home := t.TempDir()
	s := sessionCreate(home, "zagy")
	s.append("session_start", map[string]string{"model": "fixture"})
	s.append("user", map[string]string{"text": "ping"})
	s.append("assistant", map[string]any{"text": "pong", "tool_calls": []any{}})

	s2, err := sessionOpen(home, s.ID)
	if err != nil {
		t.Fatal(err)
	}
	if s2.Harness != "zagy" {
		t.Fatalf("harness %q", s2.Harness)
	}
	msgs := s2.replay()
	if len(msgs) != 2 || msgs[0]["role"] != "user" {
		t.Fatalf("replay: %v", msgs)
	}
	// branch back past the assistant turn
	evs, _ := s2.events()
	s2.branch(evs[1].ID)
	msgs2, _ := sessionOpen(home, s.ID)
	if got := msgs2.replay(); len(got) != 1 {
		t.Fatalf("branch replay len=%d", len(got))
	}
}

func TestEventShape(t *testing.T) {
	s := sessionCreate(t.TempDir(), "zagy")
	ev := s.append("user", map[string]string{"text": "hi"})
	var m map[string]any
	json.Unmarshal([]byte(ev.Line()), &m)
	for _, k := range []string{"v", "id", "parent", "ts", "session", "harness", "kind", "data"} {
		if _, ok := m[k]; !ok {
			t.Fatalf("missing %s", k)
		}
	}
	if m["harness"] != "zagy" || m["v"].(float64) != 1 {
		t.Fatalf("bad shape: %v", m)
	}
}

func TestToolsDumpMatchesContract(t *testing.T) {
	canonical, err := os.ReadFile(filepath.Join("..", "contracts", "tools.json"))
	if err != nil {
		t.Skip("contracts/tools.json not reachable from test cwd")
	}
	if ToolsJSON != string(canonical) {
		t.Fatal("tools.json drifted from contracts/tools.json")
	}
}

func TestAllowlist(t *testing.T) {
	allowed := []string{"api.openai.com"}
	if err := hostAllowed("https://api.openai.com/x", allowed); err != nil {
		t.Fatal(err)
	}
	for _, bad := range []string{"https://evil.com", "https://api.openai.com.evil.com"} {
		if err := hostAllowed(bad, allowed); err == nil {
			t.Fatalf("%s allowed", bad)
		}
	}
}

func TestFixturePingPong(t *testing.T) {
	cfg := ProviderCfg{Kind: "fixture", Model: "fixture"}
	req := map[string]any{
		"system": []any{"sys"},
		"messages": []any{map[string]any{"role": "user",
			"content": []any{map[string]any{"type": "text", "text": "ping"}}}},
		"session_id": "s1", "max_tokens": 128,
	}
	var text string
	var usage Usage
	complete(cfg, req, func(c map[string]any) {
		if c["type"] == "text" {
			text += c["text"].(string)
		}
		if c["type"] == "done" {
			u := c["usage"].(Usage)
			usage.Input += u.Input
			usage.Output += u.Output
		}
	}, nil)
	if text != "pong" {
		t.Fatalf("got %q", text)
	}
	if usage.Input == 0 || usage.Output == 0 {
		t.Fatalf("usage zero: %+v", usage)
	}
}
