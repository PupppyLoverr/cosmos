// session — append-only JSONL at COSMOS_CLI_HOME/sessions/<id>.jsonl (§6).
package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

type Session struct {
	ID      string
	File    string
	Leaf    *string
	Harness string
}

func sessionsDir(home string) string { return filepath.Join(home, "sessions") }

func sessionCreate(home, harness string) *Session {
	os.MkdirAll(sessionsDir(home), 0o755)
	id := newID()
	return &Session{
		ID:      id,
		File:    filepath.Join(sessionsDir(home), id+".jsonl"),
		Harness: harness,
	}
}

func sessionOpen(home, id string) (*Session, error) {
	s := &Session{
		ID:   id,
		File: filepath.Join(sessionsDir(home), id+".jsonl"),
	}
	evs, err := s.events()
	if err != nil || len(evs) == 0 {
		return nil, fmt.Errorf("no session %s", id)
	}
	s.Leaf = &evs[len(evs)-1].ID
	s.Harness = evs[0].Harness
	return s, nil
}

func (s *Session) events() ([]Event, error) {
	f, err := os.Open(s.File)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	var out []Event
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 1<<20), 1<<20)
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if line == "" {
			continue
		}
		var e Event
		if json.Unmarshal([]byte(line), &e) == nil {
			out = append(out, e)
		}
	}
	return out, nil
}

func (s *Session) append(kind string, data any) Event {
	ev := newEvent(s.ID, s.Harness, s.Leaf, kind, data)
	f, err := os.OpenFile(s.File, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
	if err == nil {
		f.WriteString(ev.Line() + "\n")
		f.Close()
	}
	s.Leaf = &ev.ID
	return ev
}

func (s *Session) branch(at string) Event {
	s.Leaf = &at
	return s.append("checkout", map[string]string{"at": at})
}

// chain follows parent links back from the last-appended event (checkout rewinds).
func (s *Session) chain() []Event {
	evs, _ := s.events()
	byID := map[string]*Event{}
	for i := range evs {
		byID[evs[i].ID] = &evs[i]
	}
	var out []Event
	var cur *Event
	if len(evs) > 0 {
		cur = &evs[len(evs)-1]
	}
	for cur != nil {
		out = append([]Event{*cur}, out...)
		if cur.Parent == nil {
			break
		}
		cur = byID[*cur.Parent]
	}
	return out
}

// replay rebuilds provider messages [{role, content:[{type,...}]}].
func (s *Session) replay() []map[string]any {
	var msgs []map[string]any
	for _, e := range s.chain() {
		d := map[string]any{}
		json.Unmarshal(e.Data, &d)
		switch e.Kind {
		case "user":
			if t, ok := d["text"].(string); ok {
				msgs = append(msgs, map[string]any{
					"role":    "user",
					"content": []any{map[string]any{"type": "text", "text": t}},
				})
			}
		case "assistant":
			var blocks []any
			if t, ok := d["text"].(string); ok && t != "" {
				blocks = append(blocks, map[string]any{"type": "text", "text": t})
			}
			for _, c := range anySlice(d["tool_calls"]) {
				if cm, ok := c.(map[string]any); ok {
					blocks = append(blocks, map[string]any{
						"type": "tool_use", "id": cm["id"],
						"name": cm["name"], "input": cm["input"],
					})
				}
			}
			if len(blocks) > 0 {
				msgs = append(msgs, map[string]any{"role": "assistant", "content": blocks})
			}
		case "tool_result":
			msgs = append(msgs, map[string]any{
				"role": "user",
				"content": []any{map[string]any{
					"type":        "tool_result",
					"tool_use_id": d["tool_use_id"],
					"content":     d["content"],
					"is_error":    d["is_error"],
				}},
			})
		}
	}
	return msgs
}

func anySlice(v any) []any {
	switch a := v.(type) {
	case []any:
		return a
	case []map[string]any:
		out := make([]any, len(a))
		for i, m := range a {
			out[i] = m
		}
		return out
	case []string:
		out := make([]any, len(a))
		for i, s := range a {
			out[i] = s
		}
		return out
	}
	return nil
}

func sessionList(home string) []map[string]any {
	var out []map[string]any
	entries, _ := os.ReadDir(sessionsDir(home))
	for _, en := range entries {
		if !strings.HasSuffix(en.Name(), ".jsonl") {
			continue
		}
		s := &Session{File: filepath.Join(sessionsDir(home), en.Name())}
		evs, err := s.events()
		if err != nil || len(evs) == 0 {
			continue
		}
		preview := ""
		cwd := ""
		for _, e := range evs {
			if e.Kind == "session_start" {
				cwd = dataStr(e, "cwd")
			}
			if e.Kind == "user" {
				preview = dataStr(e, "text")
				break
			}
		}
		if len(preview) > 80 {
			preview = preview[:80]
		}
		out = append(out, map[string]any{
			"id": strings.TrimSuffix(en.Name(), ".jsonl"), "harness": evs[0].Harness,
			"started": evs[0].TS, "events": len(evs), "preview": preview,
			"cwd": cwd,
		})
	}
	sort.Slice(out, func(i, j int) bool {
		return fmt.Sprint(out[i]["started"]) < fmt.Sprint(out[j]["started"])
	})
	return out
}

func sessionExport(home, id, format string) (string, error) {
	f := filepath.Join(sessionsDir(home), id+".jsonl")
	raw, err := os.ReadFile(f)
	if err != nil {
		return "", err
	}
	if format == "jsonl" {
		return string(raw), nil
	}
	s := &Session{File: f}
	evs, err := s.events()
	if err != nil {
		return "", err
	}
	var md strings.Builder
	fmt.Fprintf(&md, "# session %s\n\n", id)
	for _, e := range evs {
		switch e.Kind {
		case "user":
			fmt.Fprintf(&md, "## user · %s\n\n%s\n\n", e.TS, dataStr(e, "text"))
		case "assistant":
			fmt.Fprintf(&md, "## assistant · %s\n\n%s\n", e.TS, dataStr(e, "text"))
			for _, c := range anySlice(dataGet(e, "tool_calls")) {
				if cm, ok := c.(map[string]any); ok {
					inp, _ := json.Marshal(cm["input"])
					fmt.Fprintf(&md, "\n- `%v` %s\n", cm["name"], inp)
				}
			}
			md.WriteString("\n")
		case "tool_result":
			content := dataStr(e, "content")
			if len(content) > 200 {
				content = content[:200]
			}
			fmt.Fprintf(&md, "_%v_ → `%s`\n\n", dataStr(e, "name"), content)
		}
	}
	return md.String(), nil
}
