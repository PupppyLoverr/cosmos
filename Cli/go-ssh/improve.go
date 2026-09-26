package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

var improveForbidden = []string{"CONTRACT.md", "zsandbox", "auth_cmd.rs", "creds.rs", "oauth.ts", "wincred.ts"}

func forbiddenImprove(p string) bool {
	for _, f := range improveForbidden {
		if strings.Contains(p, f) {
			return true
		}
	}
	return false
}

func skillify(home, name string, w World, draft, allowSelfMutate bool) (string, error) {
	if !allowSelfMutate && forbiddenImprove(name) {
		return "", fmt.Errorf("self-improve refused: target is fenced")
	}
	dir := filepath.Join(home, "skills", name)
	if draft {
		dir = filepath.Join(home, "skills", "drafts", name)
	}
	dest := filepath.Join(dir, "SKILL.md")
	if !draft {
		if _, err := os.Stat(dest); err == nil {
			return "", fmt.Errorf("skill %s exists — use /refine", name)
		}
	}
	os.MkdirAll(dir, 0o755)
	desc := w.Goal
	if len(desc) > 120 {
		desc = desc[:120]
	}
	desc = strings.ReplaceAll(desc, `"`, "'")
	body := fmt.Sprintf("---\nname: %s\ndescription: \"%s\"\n---\n\n# %s\n\n## Goal\n%s\n\n## World\n%s\n",
		name, desc, name, w.Goal, w.Render())
	low := strings.ToLower(body)
	if strings.Contains(low, "sk-") && strings.Contains(low, "api") ||
		strings.Contains(low, "begin private key") ||
		strings.Contains(low, "aws_secret") {
		return "", fmt.Errorf("self-improve refused: secret scanner")
	}
	if err := os.WriteFile(dest, []byte(body), 0o644); err != nil {
		return "", err
	}
	return dest, nil
}

func improveEvent(surface string, detail any) map[string]any {
	return map[string]any{"surface": surface, "detail": detail}
}

func correctionRow(command, fix string) string {
	return fmt.Sprintf("command `%s` failed twice; fix: %s", command, fix)
}
