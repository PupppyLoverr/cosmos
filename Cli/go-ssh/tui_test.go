package main

import (
	"bytes"
	"io"
	"strings"
	"sync"
	"testing"
	"time"

	tea "github.com/charmbracelet/bubbletea"
)

type safeBuf struct {
	mu sync.Mutex
	b  bytes.Buffer
}

func (s *safeBuf) Write(p []byte) (int, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.b.Write(p)
}

func (s *safeBuf) String() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.b.String()
}

func TestSlashFilter(t *testing.T) {
	rows := filterSlash(slashCmds, "/a")
	var names []string
	for _, r := range rows {
		names = append(names, r.name)
	}
	if len(names) != 2 || names[0] != "/agent" || names[1] != "/approve" {
		t.Fatalf("/a → %v", names)
	}
	if got := filterSlash(slashCmds, "/zz"); len(got) != 0 {
		t.Fatalf("/zz → %v", got)
	}
	if got := filterSlash(slashCmds, "hello"); got != nil {
		t.Fatalf("non-slash → %v", got)
	}
	if got := filterSlash(slashCmds, "/"); len(got) != len(slashCmds) {
		t.Fatalf("bare / → %d rows", len(got))
	}
}

func TestShortcutRowsShape(t *testing.T) {
	rows := shortcutRows()
	if len(rows) < 8 {
		t.Fatalf("shortcut table too small: %d", len(rows))
	}
	var keys []string
	for _, r := range rows {
		keys = append(keys, r.left)
		if r.right == "" {
			t.Fatalf("missing desc for %q", r.left)
		}
	}
	for _, want := range []string{"shift+tab", "ctrl+r", "esc", "enter"} {
		found := false
		for _, k := range keys {
			if strings.Contains(k, want) {
				found = true
			}
		}
		if !found {
			t.Fatalf("shortcut table missing %q", want)
		}
	}
}

func TestMarkdownRender(t *testing.T) {
	out := renderMarkdown("**bold** and `code`\n- item one\n- item two", 80)
	if len(out) != 3 {
		t.Fatalf("want 3 lines, got %d: %v", len(out), out)
	}
	// markers must be consumed by styling
	joined := strings.Join(out, "\n")
	if strings.Contains(joined, "**") || strings.Contains(joined, "`code`") {
		t.Fatalf("markers left in output: %q", joined)
	}
	if !strings.HasPrefix(out[1], "• ") {
		t.Fatalf("bullet not normalized: %q", out[1])
	}
}

func TestShortArgs(t *testing.T) {
	got := shortArgs(map[string]any{"path": "a.go", "edits": []any{1, 2}})
	if !strings.Contains(got, `path="a.go"`) || !strings.Contains(got, "edits=<2>") {
		t.Fatalf("got %q", got)
	}
	if shortArgs(nil) != "" {
		t.Fatal("nil input")
	}
}

func TestTUISmoke(t *testing.T) {
	home := t.TempDir()
	cfg := &Config{
		Providers: map[string]map[string]string{"fixture": {"kind": "fixture"}},
		Models: map[string]ModelEntry{"fixture": {
			Provider: "fixture", ID: "fixture", Context: 128000,
			Price: map[string]float64{}}},
		DefaultModel: "fixture",
	}
	a := &Agent{
		Opts: &RunOpts{MaxTurns: 4}, Cfg: cfg,
		Mode: "agent", ModelAlias: "fixture (fixture)",
		Ctx:     NewToolCtx(home),
		Session: &Session{ID: "0123456789abcdefghjkmnpqrs", Harness: "zagy"},
	}
	br := &tuiBridge{done: make(chan struct{})}
	m := newTUIModel(a, a.Opts, home, br)
	tm, _ := m.Update(tea.WindowSizeMsg{Width: 100, Height: 30})
	m = tm.(tuiModel)
	v := m.View()
	if !strings.Contains(v, "zagy") || !strings.Contains(v, "fixture (fixture)") {
		t.Fatalf("banner missing: %q", v)
	}
	if !strings.Contains(v, "? for shortcuts") {
		t.Fatalf("footer missing: %q", v)
	}
	// "/" opens the dropdown, "?" opens the shortcuts overlay
	m.ta.SetValue("/")
	if !m.slashVisible() || len(m.slashRows()) != len(slashCmds) {
		t.Fatal("slash menu did not open")
	}
	m.ta.Reset()
	tm, _ = m.onKey(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("?")})
	m = tm.(tuiModel)
	if m.overlay != overlayHelp || !strings.Contains(m.View(), "Keyboard Shortcuts") {
		t.Fatalf("help overlay did not open")
	}
}

// TestTUIEndToEnd drives a real bubbletea program over pipes: types "ping",
// expects the fixture provider's "pong" to land in the transcript, then /quit.
func TestTUIEndToEnd(t *testing.T) {
	home := t.TempDir()
	t.Setenv("COSMOS_CLI_HOME", home)
	opts := &RunOpts{MaxTurns: 4}
	a, err := NewAgent(opts, "zagy", home)
	if err != nil {
		t.Fatal(err)
	}
	if a.Provider.Kind != "fixture" {
		t.Skip("fixture provider unavailable")
	}
	inR, inW := io.Pipe()
	var out safeBuf
	br := &tuiBridge{done: make(chan struct{})}
	m := newTUIModel(a, opts, home, br)
	p := tea.NewProgram(m, tea.WithInput(inR), tea.WithOutput(&out),
		tea.WithoutSignalHandler())
	br.prog = p
	br.emit = func(ev Event) { p.Send(eventMsg{ev}) }
	a.Asker = func(tool string) bool {
		ch := make(chan int, 1)
		p.Send(askMsg{tool: tool, reply: ch})
		select {
		case r := <-ch:
			return r != permNo
		case <-br.done:
			return false
		}
	}
	done := make(chan struct{})
	go func() { p.Run(); close(done) }()
	defer p.Kill()

	if _, err := io.WriteString(inW, "ping\r"); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(15 * time.Second)
	for !strings.Contains(out.String(), "pong") {
		if time.Now().After(deadline) {
			t.Fatalf("no pong rendered; tail=%q", out.String()[maxI(0, len(out.String())-400):])
		}
		time.Sleep(50 * time.Millisecond)
	}
	if _, err := io.WriteString(inW, "/quit\r"); err != nil {
		t.Fatal(err)
	}
	select {
	case <-done:
	case <-time.After(10 * time.Second):
		t.Fatal("program did not exit on /quit")
	}
	evs, _ := a.Session.events()
	if len(evs) == 0 || evs[len(evs)-1].Kind != "session_end" {
		t.Fatalf("missing session_end: %v", evs)
	}
}

func TestRenderPermCard(t *testing.T) {
	m := tuiModel{width: 80, permIdx: 1}
	it := tItem{kind: "perm", text: "bash", resolved: -1}
	out := m.renderPerm(it, 80)
	if len(out) != 3 || !strings.Contains(out[0], "Allow bash?") {
		t.Fatalf("card: %v", out)
	}
	it.resolved = permAlways
	out = m.renderPerm(it, 80)
	if len(out) != 1 || !strings.Contains(out[0], "always") {
		t.Fatalf("resolved: %v", out)
	}
}
