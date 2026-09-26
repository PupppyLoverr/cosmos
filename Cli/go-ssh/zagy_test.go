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

func TestRepairReadAlias(t *testing.T) {
	r := repairTool("Read", map[string]any{"path": "a.txt"})
	if r.Name != "read" {
		t.Fatalf("name %s", r.Name)
	}
	if r.Reject != "" {
		t.Fatalf("reject %s", r.Reject)
	}
	found := false
	for _, x := range r.Repairs {
		if x.Reason == "alias" {
			found = true
		}
	}
	if !found {
		t.Fatal("expected alias repair")
	}
}

func TestRepairMissingRequired(t *testing.T) {
	r := repairTool("read", map[string]any{})
	if !strings.Contains(r.Reject, "missing field path") {
		t.Fatalf("reject %s", r.Reject)
	}
}

func TestCompactL1Persist(t *testing.T) {
	home := t.TempDir()
	big := strings.Repeat("x", 9000) + strings.Repeat("\n", 50)
	msgs := []map[string]any{{"role": "user", "content": []any{
		map[string]any{"type": "tool_result", "tool_use_id": "t1", "content": big},
	}}}
	layer1(msgs, home, 200)
	c := strv(msgs[0]["content"].([]any)[0].(map[string]any), "content")
	if !strings.Contains(c, "[tool_result persisted") {
		t.Fatalf("no persist: %s", c[:80])
	}
}

func TestCkptRestore(t *testing.T) {
	home := t.TempDir()
	f := filepath.Join(t.TempDir(), "w.txt")
	os.WriteFile(f, []byte("v1"), 0o644)
	if ckptSave(home, "s1", f) == "" {
		t.Fatal("save failed")
	}
	os.WriteFile(f, []byte("v2"), 0o644)
	got, err := ckptRestoreLast(home, "s1")
	if err != nil {
		t.Fatal(err)
	}
	if got != f {
		t.Fatalf("path %s", got)
	}
	body, _ := os.ReadFile(f)
	if string(body) != "v1" {
		t.Fatalf("body %q", body)
	}
}

func TestWorldRoundTrip(t *testing.T) {
	home := t.TempDir()
	w := emptyWorld()
	w.SetGoalIfEmpty("ship the login form")
	w.TouchFile("a.ts", []byte("abc"))
	saveWorld(home, "sess", w)
	loaded := loadWorld(home, "sess")
	if loaded.Goal != "ship the login form" {
		t.Fatalf("goal %q", loaded.Goal)
	}
	if !strings.Contains(loaded.Render(), "a.ts") {
		t.Fatal(loaded.Render())
	}
}

func TestRepairLockstep(t *testing.T) {
	r := repairTool("Read", map[string]any{"path": "a.txt"})
	if r.Name != "read" || r.Reject != "" {
		t.Fatalf("%+v", r)
	}
	r = repairTool("memory.search", map[string]any{"query": "x"})
	if r.Name != "memory_search" {
		t.Fatalf("alias %s", r.Name)
	}
	r = repairTool("read", map[string]any{"path": "a.txt", "start": "12"})
	if r.Input["start"] != 12 {
		t.Fatalf("coerce %#v", r.Input["start"])
	}
	r = repairTool("READ", map[string]any{"path": "a.txt"})
	if r.Name != "read" {
		t.Fatalf("case %s", r.Name)
	}
}

func TestSandboxDeny(t *testing.T) {
	dir := t.TempDir()
	ctx := NewToolCtx(dir)
	ctx.SandboxMode = "read-only"
	if out := runTool(ctx, "write", map[string]any{"path": "a.txt", "content": "x"}); !strings.Contains(out, "read-only") {
		t.Fatalf("read-only: %s", out)
	}
	ctx.SandboxMode = "workspace-write"
	if out := runTool(ctx, "write", map[string]any{"path": ".env", "content": "SECRET=1"}); !strings.Contains(out, "sandbox deny") {
		t.Fatalf("env: %s", out)
	}
	if out := runTool(ctx, "bash", map[string]any{"command": "type ~/.ssh/id_rsa"}); !strings.Contains(out, "sandbox deny") {
		t.Fatalf("ssh: %s", out)
	}
}

func TestExpandMentions(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "a.txt"), []byte("hello\n"), 0o644)
	got := expandMentions("read @a.txt please", dir)
	if !strings.Contains(got, `<file path="a.txt">`) || !strings.Contains(got, "hello") {
		t.Fatalf("got %q", got)
	}
	if got := expandMentions("escape @../x", dir); strings.Contains(got, "<file") {
		t.Fatalf("escaped: %q", got)
	}
}
