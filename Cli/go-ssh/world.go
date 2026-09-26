// Per-session world.md. Same semantics as zcore/src/world.rs and zdev/world.ts.
package main

import (
	"crypto/sha256"
	"encoding/hex"
	"os"
	"path/filepath"
	"strings"
)

type World struct {
	Goal      string
	Status    string
	Decisions []string
	Files     [][2]string
	Tests     [][2]string
	Questions []string
	Plan      string
	Next      string
	Focus     string
}

func emptyWorld() World { return World{} }

func worldPath(home, session string) string {
	return filepath.Join(home, "world", session+".md")
}

func sha8(b []byte) string {
	h := sha256.Sum256(b)
	return hex.EncodeToString(h[:])[:8]
}

func (w World) Render() string {
	var files []string
	for _, f := range w.Files {
		if f[1] != "" {
			files = append(files, f[0]+" ("+f[1]+")")
		} else {
			files = append(files, f[0])
		}
	}
	var tests []string
	for _, t := range w.Tests {
		if t[1] != "" {
			tests = append(tests, t[0]+" → "+t[1])
		} else {
			tests = append(tests, t[0])
		}
	}
	var b strings.Builder
	b.WriteString("# World\n")
	if w.Focus != "" {
		b.WriteString("focus: " + w.Focus + "\n")
	}
	b.WriteString("goal: " + w.Goal + "\n")
	b.WriteString("status: " + w.Status + "\n")
	b.WriteString("decisions: " + strings.Join(w.Decisions, "; ") + "\n")
	b.WriteString("files_touched: " + strings.Join(files, ", ") + "\n")
	b.WriteString("tests: " + strings.Join(tests, "; ") + "\n")
	b.WriteString("open_questions: " + strings.Join(w.Questions, "; ") + "\n")
	b.WriteString("# Plan\n" + w.Plan + "\n")
	b.WriteString("# Next\n" + w.Next + "\n")
	return b.String()
}

func parseWorld(text string) World {
	w := emptyWorld()
	section := ""
	for _, line := range strings.Split(text, "\n") {
		line = strings.TrimRight(line, "\r")
		switch {
		case line == "# World":
			section = "world"
		case line == "# Plan":
			section = "plan"
		case line == "# Next":
			section = "next"
		case strings.HasPrefix(line, "goal: "):
			w.Goal = line[6:]
		case strings.HasPrefix(line, "status: "):
			w.Status = line[8:]
		case strings.HasPrefix(line, "focus: "):
			w.Focus = line[7:]
		case section == "plan" && line != "":
			if w.Plan != "" {
				w.Plan += "\n"
			}
			w.Plan += line
		case section == "next" && line != "":
			if w.Next != "" {
				w.Next += "\n"
			}
			w.Next += line
		}
	}
	return w
}

func loadWorld(home, session string) World {
	data, err := os.ReadFile(worldPath(home, session))
	if err != nil {
		return emptyWorld()
	}
	return parseWorld(string(data))
}

func saveWorld(home, session string, w World) {
	p := worldPath(home, session)
	os.MkdirAll(filepath.Dir(p), 0o755)
	os.WriteFile(p, []byte(w.Render()), 0o644)
}

func (w *World) TouchFile(file string, bytes []byte) {
	h := sha8(bytes)
	for i, f := range w.Files {
		if f[0] == file {
			w.Files[i][1] = h
			w.Status = "touched " + file
			return
		}
	}
	w.Files = append(w.Files, [2]string{file, h})
	w.Status = "touched " + file
}

func (w *World) SetGoalIfEmpty(text string) {
	if w.Goal == "" && !strings.HasPrefix(text, "[") && !strings.HasPrefix(text, "/") {
		if len(text) > 400 {
			text = text[:400]
		}
		w.Goal = text
	}
}

func (w *World) ApplyCompactMd(md string) {
	p := parseWorld(md)
	if p.Goal != "" {
		w.Goal = p.Goal
	}
	if p.Status != "" {
		w.Status = p.Status
	}
	if len(p.Files) > 0 {
		w.Files = p.Files
	}
	if p.Plan != "" {
		w.Plan = p.Plan
	}
	if p.Next != "" {
		w.Next = p.Next
	}
	if p.Focus != "" {
		w.Focus = p.Focus
	}
}
