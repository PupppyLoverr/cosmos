// Composer helpers shared with zdev menus.ts: @file attach, !shell, #remember.
package main

import (
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"unicode/utf8"
)

var mentionRe = regexp.MustCompile(`(?:^|\s)@([^\s@]+)`)

func expandMentions(text, cwd string) string {
	root, err := filepath.Abs(cwd)
	if err != nil {
		return text
	}
	root = filepath.Clean(root)
	seen := map[string]bool{}
	var extra strings.Builder
	for _, m := range mentionRe.FindAllStringSubmatch(text, -1) {
		rel := strings.TrimRight(m[1], ".,;:!?)")
		if seen[rel] {
			continue
		}
		seen[rel] = true
		p := filepath.Clean(filepath.Join(root, rel))
		if p != root && !strings.HasPrefix(strings.ToLower(p), strings.ToLower(root)+string(os.PathSeparator)) {
			continue
		}
		st, err := os.Stat(p)
		if err != nil || !st.Mode().IsRegular() {
			continue
		}
		body, err := os.ReadFile(p)
		if err != nil {
			continue
		}
		s := string(body)
		lines := strings.Split(s, "\n")
		shown := s
		if len(lines) > 400 {
			shown = strings.Join(lines[:400], "\n") + "\n… (" + itoa(len(lines)-400) + " more lines)"
		}
		extra.WriteString("\n\n<file path=\"")
		extra.WriteString(rel)
		extra.WriteString("\">\n")
		extra.WriteString(shown)
		extra.WriteString("\n</file>")
	}
	return text + extra.String()
}

func itoa(n int) string {
	if n == 0 {
		return "0"
	}
	var b [12]byte
	i := len(b)
	neg := n < 0
	if neg {
		n = -n
	}
	for n > 0 {
		i--
		b[i] = byte('0' + n%10)
		n /= 10
	}
	if neg {
		i--
		b[i] = '-'
	}
	return string(b[i:])
}

func lastAssistantText(items []tItem) string {
	for i := len(items) - 1; i >= 0; i-- {
		if items[i].kind == "assistant" && strings.TrimSpace(items[i].text) != "" {
			return items[i].text
		}
	}
	return ""
}

func shortQuote(s string, n int) string {
	s = strings.TrimSpace(s)
	if utf8.RuneCountInString(s) <= n {
		return s
	}
	var b strings.Builder
	i := 0
	for _, r := range s {
		if i >= n {
			break
		}
		b.WriteRune(r)
		i++
	}
	return b.String()
}
